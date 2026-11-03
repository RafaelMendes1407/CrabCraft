//! Dependency versions in an open `Cargo.toml` (C-03) and the docs.rs icon of each crates.io
//! dependency (C-11).
//!
//! Each dependency line gets an end-of-line hint with the version `Cargo.lock` resolved and, when
//! the online check is on, the newest version on crates.io if it is newer. The lock file and the
//! workspace manifest are read on a background thread; crates.io is asked on another, once per
//! crate for the session, with only the crate name in the request. Nothing here blocks the UI.

use crate::docs::Docs;
use rc_cargo::{DepSource, Lockfile, ManifestDep, Published};
use rc_core::{Inbox, Waker};
use rc_editor::GutterIcon;
use rc_kit::i18n::trf;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// How long one crates.io request may take.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// What the background read of a manifest's surroundings found.
#[derive(Debug, Default)]
pub struct LockInfo {
    /// `None` without a `Cargo.lock` (or with one that does not parse).
    pub lock: Option<Lockfile>,
    /// `[workspace.dependencies]` of the manifest next to the lock, by key.
    pub workspace_deps: HashMap<String, ManifestDep>,
}

/// The lock file of one manifest: the last one read and, while reading, the next.
struct LockEntry {
    /// `(manifest mtime, Cargo epoch)` the read was started for.
    key: (Option<SystemTime>, u64),
    info: Option<Arc<LockInfo>>,
    loading: Option<Inbox<LockInfo>>,
}

pub struct DepVersions {
    waker: Waker,
    locks: HashMap<PathBuf, LockEntry>,
    /// crates.io answers by crate name; `None` while asked or when the request failed (it is not
    /// asked again this session).
    published: HashMap<String, Option<Published>>,
    fetched: Inbox<(String, Option<Published>)>,
    /// Bumped whenever a lock or a crates.io answer arrives.
    generation: u64,
    /// The `(text version, generation, online)` each editor got its hints for.
    given: HashMap<u64, (u64, u64, bool)>,
    /// The docs.rs page of each line with an icon, per editor.
    pub links: HashMap<u64, BTreeMap<usize, String>>,
}

impl DepVersions {
    pub fn new(waker: Waker) -> Self {
        DepVersions { fetched: Inbox::new(waker.clone()), waker, locks: HashMap::new(), published: HashMap::new(), generation: 0, given: HashMap::new(), links: HashMap::new() }
    }

    /// Records a crates.io answer without the network (tests).
    #[cfg(test)]
    pub fn set_published(&mut self, name: &str, published: Published) {
        self.published.insert(name.to_string(), Some(published));
        self.generation += 1;
    }

    /// Whether a lock read is still running (tests).
    #[cfg(test)]
    pub fn loading(&self) -> bool {
        self.locks.values().any(|l| l.loading.is_some() || l.info.is_none())
    }

    /// Takes the finished reads and refreshes the hints and icons of the active `Cargo.toml`.
    /// `epoch` changes when the Cargo project is read again; `online` allows asking crates.io.
    pub fn update(&mut self, docs: &mut Docs, epoch: u64, online: bool) {
        for entry in self.locks.values_mut() {
            if let Some(info) = entry.loading.as_ref().and_then(|i| i.drain().into_iter().last()) {
                entry.info = Some(Arc::new(info));
                entry.loading = None;
                self.generation += 1;
            }
        }
        for (name, published) in self.fetched.drain() {
            self.published.insert(name, published);
            self.generation += 1;
        }
        let Some(i) = docs.active else { return };
        let doc = &docs.list[i];
        if doc.path.file_name().is_none_or(|n| n != "Cargo.toml") {
            return;
        }
        let key = (doc.mtime, epoch);
        if self.locks.get(&doc.path).is_none_or(|e| e.key != key && e.loading.is_none()) {
            let info = self.locks.remove(&doc.path).and_then(|e| e.info);
            let loading = Some(self.read_lock(doc.path.clone()));
            self.locks.insert(doc.path.clone(), LockEntry { key, info, loading });
        }
        let stamp = (doc.editor.buffer.version(), self.generation, online);
        if self.given.get(&doc.editor.id) == Some(&stamp) {
            return;
        }
        self.given.insert(doc.editor.id, stamp);
        let info = self.locks.get(&doc.path).and_then(|e| e.info.clone()).unwrap_or_default();
        let deps = rc_cargo::dependency_lines(&doc.editor.buffer.rope().to_string());
        let mut hints = BTreeMap::new();
        let mut links = BTreeMap::new();
        let mut ask = Vec::new();
        for dep in &deps {
            let dep = resolve(dep, &info);
            let crates_io = dep.source == DepSource::CratesIo;
            if !crates_io && !matches!(dep.source, DepSource::Registry(_)) {
                continue;
            }
            let locked = info.lock.as_ref().and_then(|l| l.pick(&dep.name, dep.req.as_deref()));
            // A crates.io requirement whose lock entry comes from elsewhere ([patch]) is not
            // checked online, and its docs.rs page is the newest.
            let on_crates_io = crates_io && locked.is_none_or(|l| l.is_from_crates_io());
            if on_crates_io {
                let version = locked.map_or_else(|| "latest".to_string(), |l| l.version.to_string());
                links.insert(dep.line, format!("https://docs.rs/{}/{version}", dep.name));
            }
            let Some(locked) = locked else { continue };
            let newer = match self.published.get(&dep.name) {
                Some(Some(p)) if online && on_crates_io => p.newer_than(&locked.version).cloned(),
                None if online && on_crates_io => {
                    ask.push(dep.name.clone());
                    None
                }
                _ => None,
            };
            let text = match newer {
                Some(latest) => trf("{locked} · {latest} available", &[("locked", &locked.version.to_string()), ("latest", &latest.to_string())]),
                None => locked.version.to_string(),
            };
            hints.insert(dep.line, text);
        }
        let doc = &mut docs.list[i];
        doc.editor.end_hints = hints;
        doc.editor.gutter_icons.retain(|_, icon| *icon != GutterIcon::DocsRs);
        // Feature switches and other icons keep their line.
        for line in links.keys() {
            doc.editor.gutter_icons.entry(*line).or_insert(GutterIcon::DocsRs);
        }
        self.links.insert(doc.editor.id, links);
        ask.sort();
        ask.dedup();
        self.ask_crates_io(ask);
    }

    /// Reads the `Cargo.lock` above `manifest` and the workspace manifest next to it.
    fn read_lock(&self, manifest: PathBuf) -> Inbox<LockInfo> {
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        std::thread::spawn(move || {
            out.send(read_lock_info(&manifest));
        });
        inbox
    }

    /// Asks crates.io for the versions of `names` on one background thread, in turn.
    fn ask_crates_io(&mut self, names: Vec<String>) {
        let names: Vec<String> = names.into_iter().filter(|n| rc_cargo::valid_crate_name(n) && !self.published.contains_key(n)).collect();
        if names.is_empty() {
            return;
        }
        for n in &names {
            self.published.insert(n.clone(), None);
        }
        let out = self.fetched.sender();
        std::thread::spawn(move || {
            for name in names {
                let published = rc_cargo::fetch_crates_io_versions(&name, FETCH_TIMEOUT).ok();
                // The app is gone: stop asking.
                if !out.send((name, published)) {
                    return;
                }
            }
        });
    }
}

/// A dependency with `workspace = true` takes its source and requirement from the workspace.
fn resolve(dep: &ManifestDep, info: &LockInfo) -> ManifestDep {
    if dep.source != DepSource::Workspace {
        return dep.clone();
    }
    match info.workspace_deps.get(&dep.key) {
        Some(ws) => ManifestDep { line: dep.line, key: dep.key.clone(), in_workspace: false, ..ws.clone() },
        None => dep.clone(),
    }
}

/// The most a workspace manifest may weigh.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

fn read_lock_info(manifest: &Path) -> LockInfo {
    use std::io::Read as _;
    let Some(lock_path) = rc_cargo::find_lockfile(manifest) else { return LockInfo::default() };
    let lock = Lockfile::read(&lock_path).ok();
    let mut text = String::new();
    let root = lock_path.with_file_name("Cargo.toml");
    let read = std::fs::File::open(&root).and_then(|f| f.take(MAX_MANIFEST_BYTES).read_to_string(&mut text));
    let workspace_deps = if read.is_ok() { rc_cargo::dependency_lines(&text).into_iter().filter(|d| d.in_workspace).map(|d| (d.key.clone(), d)).collect() } else { HashMap::new() };
    LockInfo { lock, workspace_deps }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_dependencies_take_the_workspace_entry() {
        let ws = rc_cargo::dependency_lines("[workspace.dependencies]\nserde = { version = \"1.0.100\", package = \"serde\" }\n").remove(0);
        let info = LockInfo { lock: None, workspace_deps: [("serde".to_string(), ws)].into() };
        let member = rc_cargo::dependency_lines("[dependencies]\n\nserde = { workspace = true, features = [\"derive\"] }\n").remove(0);
        let r = resolve(&member, &info);
        assert_eq!((r.line, r.req.as_deref(), r.source, r.in_workspace), (2, Some("1.0.100"), DepSource::CratesIo, false));
        // Unknown in the workspace: left as it is.
        let other = rc_cargo::dependency_lines("[dependencies]\nlog.workspace = true\n").remove(0);
        assert_eq!(resolve(&other, &info).source, DepSource::Workspace);
    }
}
