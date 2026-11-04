//! Dependency versions in an open `Cargo.toml` (C-03), the docs.rs icon of each crates.io
//! dependency (C-11) and the dependency checks against crates.io (C-04).
//!
//! Each dependency line gets an end-of-line hint with the version `Cargo.lock` resolved and, when
//! the online check is on, the newest version on crates.io if it is newer. With the check on,
//! a dependency on a crate crates.io does not have gets an error, a requirement no published
//! version matches a warning, and one the newest stable version is outside a weak warning with a
//! quick fix. The lock file and the workspace manifest are read on a background thread; crates.io
//! is asked on another, once per crate for the session, with only the crate name in the request.
//! Names not in the lock (being typed) are asked only after the text rests. Nothing here blocks
//! the UI.

use crate::docs::Docs;
use rc_cargo::{DepProblem, DepSource, Lockfile, ManifestDep, Published};
use rc_core::{Inbox, Waker};
use rc_editor::{Diagnostic, GutterIcon, Severity};
use rc_kit::i18n::trf;
use semver::Version;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// How long one crates.io request may take.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the text must rest before a crate not in the lock is looked up.
const SETTLE: Duration = Duration::from_millis(800);
/// The `code` of the diagnostics made here, to tell them from the language server's.
pub const DIAG_CODE: &str = "crates.io";

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

/// What crates.io said about a crate.
#[derive(Clone, Debug)]
pub enum Lookup {
    Asking,
    /// The request failed; not asked again this session.
    Failed,
    /// The index has no such crate.
    Missing,
    Found(Arc<Published>),
}

pub struct DepVersions {
    waker: Waker,
    locks: HashMap<PathBuf, LockEntry>,
    /// crates.io answers by crate name.
    published: HashMap<String, Lookup>,
    fetched: Inbox<(String, Lookup)>,
    /// Bumped whenever a lock or a crates.io answer arrives.
    generation: u64,
    /// The `(text version, generation, online, settled)` each editor got its hints for.
    given: HashMap<u64, (u64, u64, bool, bool)>,
    /// The text version of each editor and when it changed (`None`: before it was seen).
    edited: HashMap<u64, (u64, Option<Instant>)>,
    /// The docs.rs page of each line with an icon, per editor.
    pub links: HashMap<u64, BTreeMap<usize, String>>,
    /// The crates.io diagnostics of each editor.
    /// With their inspection id, for the inspection profile (I-18).
    diags: HashMap<u64, Vec<(Diagnostic, &'static str)>>,
    /// The version an outdated dependency can be updated to, by line, per editor.
    fixes: HashMap<u64, BTreeMap<usize, Version>>,
}

impl DepVersions {
    pub fn new(waker: Waker) -> Self {
        DepVersions {
            fetched: Inbox::new(waker.clone()),
            waker,
            locks: HashMap::new(),
            published: HashMap::new(),
            generation: 0,
            given: HashMap::new(),
            edited: HashMap::new(),
            links: HashMap::new(),
            diags: HashMap::new(),
            fixes: HashMap::new(),
        }
    }

    /// Records a crates.io answer without the network (tests).
    #[cfg(test)]
    pub fn set_published(&mut self, name: &str, published: Published) {
        self.published.insert(name.to_string(), Lookup::Found(Arc::new(published)));
        self.generation += 1;
    }

    /// Records that crates.io has no such crate, without the network (tests).
    #[cfg(test)]
    pub fn set_missing(&mut self, name: &str) {
        self.published.insert(name.to_string(), Lookup::Missing);
        self.generation += 1;
    }

    /// Whether a lock read is still running (tests).
    #[cfg(test)]
    pub fn loading(&self) -> bool {
        self.locks.values().any(|l| l.loading.is_some() || l.info.is_none())
    }

    /// What crates.io said about `name`, if it was asked.
    pub fn lookup(&self, name: &str) -> Option<&Lookup> {
        self.published.get(name)
    }

    /// Asks crates.io about `name` unless it was asked this session.
    pub fn request(&mut self, name: &str) {
        self.ask_crates_io(vec![name.to_string()]);
    }

    /// The registry versions of `name` locked in the `Cargo.lock` of `manifest`, newest first.
    pub fn locked_versions(&self, manifest: &Path, name: &str) -> Vec<Version> {
        let info = self.locks.get(manifest).and_then(|e| e.info.as_ref());
        let mut out: Vec<Version> = info.and_then(|i| i.lock.as_ref()).and_then(|l| l.packages.get(name)).map(|v| v.iter().filter(|l| l.source.is_some()).map(|l| l.version.clone()).collect()).unwrap_or_default();
        out.sort_unstable_by(|a, b| b.cmp(a));
        out.dedup();
        out
    }

    /// The names of the registry packages in the `Cargo.lock` of `manifest`.
    pub fn locked_names(&self, manifest: &Path) -> Vec<String> {
        let info = self.locks.get(manifest).and_then(|e| e.info.as_ref());
        let mut out: Vec<String> = info.and_then(|i| i.lock.as_ref()).map(|l| l.packages.iter().filter(|(_, v)| v.iter().any(|l| l.source.is_some())).map(|(n, _)| n.clone()).collect()).unwrap_or_default();
        out.sort();
        out
    }

    /// The version the outdated dependency on `line` can be updated to.
    pub fn fix_at(&self, editor: u64, line: usize) -> Option<&Version> {
        self.fixes.get(&editor)?.get(&line)
    }

    /// Takes the finished reads and refreshes the hints, icons and diagnostics of the active
    /// `Cargo.toml`. `epoch` changes when the Cargo project is read again; `online` allows asking
    /// crates.io. The diagnostics also go to `problems` (the Problems window). Returns how soon
    /// to call again when a look-up waits for the text to rest.
    pub fn update(&mut self, docs: &mut Docs, problems: &mut BTreeMap<PathBuf, Vec<Diagnostic>>, epoch: u64, online: bool, profile: &rc_core::inspections::InspectionProfile) -> Option<Duration> {
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
        let i = docs.active?;
        let doc = &docs.list[i];
        if doc.path.file_name().is_none_or(|n| n != "Cargo.toml") {
            return None;
        }
        let key = (doc.mtime, epoch);
        if self.locks.get(&doc.path).is_none_or(|e| e.key != key && e.loading.is_none()) {
            let info = self.locks.remove(&doc.path).and_then(|e| e.info);
            let loading = Some(self.read_lock(doc.path.clone()));
            self.locks.insert(doc.path.clone(), LockEntry { key, info, loading });
        }
        let version = doc.editor.buffer.version();
        let now = Instant::now();
        let edited = self.edited.entry(doc.editor.id).or_insert((version, None));
        if edited.0 != version {
            *edited = (version, Some(now));
        }
        let rest = edited.1.map_or(SETTLE, |t| now.duration_since(t));
        let settled = rest >= SETTLE;
        let stamp = (version, self.generation, online, settled);
        if self.given.get(&doc.editor.id) != Some(&stamp) {
            self.given.insert(doc.editor.id, stamp);
            self.refresh(docs, i, online, settled);
        }
        let doc = &mut docs.list[i];
        let ours = crate::inspection_profile::apply_own(profile, self.diags.get(&doc.editor.id).map(Vec::as_slice).unwrap_or_default());
        merge_into_editor(&mut doc.editor, DIAG_CODE, &ours);
        merge_into_problems(problems, &doc.path, DIAG_CODE, &ours);
        (!settled).then(|| SETTLE - rest)
    }

    /// Recomputes the hints, icons and diagnostics of the `Cargo.toml` in `docs.list[i]`.
    fn refresh(&mut self, docs: &mut Docs, i: usize, online: bool, settled: bool) {
        let doc = &docs.list[i];
        let info = self.locks.get(&doc.path).and_then(|e| e.info.clone()).unwrap_or_default();
        let text = doc.editor.buffer.rope().to_string();
        let deps = rc_cargo::dependency_lines(&text);
        let mut hints = BTreeMap::new();
        let mut links = BTreeMap::new();
        let mut diags = Vec::new();
        let mut fixes = BTreeMap::new();
        let mut ask = Vec::new();
        for declared in &deps {
            let dep = resolve(declared, &info);
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
            let lookup = if online && on_crates_io { self.published.get(&dep.name) } else { None };
            // A name not in the lock may still be being typed: asked once the text rests.
            if online && on_crates_io && lookup.is_none() && (locked.is_some() || settled) {
                ask.push(dep.name.clone());
            }
            // Checked where it is declared: a member's `workspace = true` is checked in the
            // workspace manifest.
            if online && on_crates_io && declared.source == DepSource::CratesIo {
                let problem = match lookup {
                    Some(Lookup::Missing) => Some(DepProblem::Missing),
                    Some(Lookup::Found(p)) => dep.req.as_deref().and_then(|r| rc_cargo::versions::check_requirement(r, p)),
                    _ => None,
                };
                if let Some(problem) = problem {
                    let spans = rc_cargo::versions::dep_spans(&text, declared);
                    let req = dep.req.as_deref().unwrap_or_default();
                    use crate::inspection_profile as ip;
                    let (span, severity, message, id) = match &problem {
                        DepProblem::Missing => (spans.key, Severity::Error, trf("No crate named `{name}` on crates.io", &[("name", &dep.name)]), ip::UNKNOWN_CRATE),
                        DepProblem::NoMatch => (spans.req.or(spans.key), Severity::Warning, trf("No version of `{name}` on crates.io matches `{req}`", &[("name", &dep.name), ("req", &req)]), ip::NO_MATCHING_VERSION),
                        DepProblem::Outdated(v) => {
                            fixes.insert(dep.line, v.clone());
                            (spans.req.or(spans.key), Severity::Info, trf("Newer version {version} is outside `{req}`", &[("version", &v.to_string()), ("req", &req)]), ip::NEWER_VERSION)
                        }
                    };
                    if let Some((start, end)) = span {
                        diags.push((Diagnostic { start, end, severity, message, code: Some(DIAG_CODE.to_string()), unnecessary: false, deprecated: false }, id));
                    }
                }
            }
            let Some(locked) = locked else { continue };
            let newer = match lookup {
                Some(Lookup::Found(p)) => p.newer_than(&locked.version).cloned(),
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
        self.diags.insert(doc.editor.id, diags);
        self.fixes.insert(doc.editor.id, fixes);
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
            self.published.insert(n.clone(), Lookup::Asking);
        }
        let out = self.fetched.sender();
        std::thread::spawn(move || {
            for name in names {
                let lookup = match rc_cargo::fetch_crates_io_versions(&name, FETCH_TIMEOUT) {
                    Ok(Some(p)) => Lookup::Found(Arc::new(p)),
                    Ok(None) => Lookup::Missing,
                    Err(_) => Lookup::Failed,
                };
                // The app is gone: stop asking.
                if !out.send((name, lookup)) {
                    return;
                }
            }
        });
    }
}

fn is_ours(d: &Diagnostic, code: &str) -> bool {
    d.code.as_deref() == Some(code)
}

/// Whether `list` holds exactly `ours` among the diagnostics with `code`.
fn holds(list: &[Diagnostic], code: &str, ours: &[Diagnostic]) -> bool {
    list.iter().filter(|d| is_ours(d, code)).count() == ours.len() && ours.iter().all(|o| list.contains(o))
}

/// Puts `ours` (made locally, all with `code`) among the editor's diagnostics, keeping the
/// language server's and the other local ones: they are put back when a server report replaces
/// the list.
pub(crate) fn merge_into_editor(editor: &mut rc_editor::EditorState, code: &str, ours: &[Diagnostic]) {
    if holds(&editor.diagnostics, code, ours) {
        return;
    }
    let mut list: Vec<Diagnostic> = editor.diagnostics.iter().filter(|d| !is_ours(d, code)).cloned().collect();
    list.extend(ours.iter().cloned());
    editor.set_diagnostics(list);
}

/// The same for the Problems window, which lists errors and warnings.
pub(crate) fn merge_into_problems(problems: &mut BTreeMap<PathBuf, Vec<Diagnostic>>, path: &Path, code: &str, ours: &[Diagnostic]) {
    let ours: Vec<Diagnostic> = ours.iter().filter(|d| d.severity <= Severity::Warning).cloned().collect();
    let current = problems.get(path).map(Vec::as_slice).unwrap_or_default();
    if holds(current, code, &ours) {
        return;
    }
    let mut list: Vec<Diagnostic> = current.iter().filter(|d| !is_ours(d, code)).cloned().collect();
    list.extend(ours);
    if list.is_empty() {
        problems.remove(path);
    } else {
        problems.insert(path.to_path_buf(), list);
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
