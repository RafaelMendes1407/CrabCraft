//! Local-only conservative dependency proposals. No Cargo command or registry request is
//! made here. The existing dependency editor owns explicit, trusted changes.
use rc_cargo::{Package, TargetKind, dependencies::Scope};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const CATALOG: &[(&str, &str)] = &[
    ("serde", "serde"),
    ("serde_json", "serde_json"),
    ("tokio", "tokio"),
    ("anyhow", "anyhow"),
    ("thiserror", "thiserror"),
    ("regex", "regex"),
    ("clap", "clap"),
    ("reqwest", "reqwest"),
    ("tracing", "tracing"),
    ("axum", "axum"),
    ("uuid", "uuid"),
    ("chrono", "chrono"),
    ("rand", "rand"),
    ("rayon", "rayon"),
    ("bytes", "bytes"),
    ("url", "url"),
    ("tempfile", "tempfile"),
    ("futures", "futures"),
    ("parking_lot", "parking_lot"),
    ("crossbeam_channel", "crossbeam-channel"),
];
#[derive(Clone, Debug)]
pub struct Proposal {
    pub root: String,
    pub name: String,
    pub manifest: PathBuf,
    pub package: String,
    pub scope: Scope,
    pub local: Option<PathBuf>,
    pub pasted: bool,
    /// A syntactic crate root outside the local catalog needs an explicit registry search.
    pub lookup: bool,
}
#[derive(Default)]
pub struct Report {
    pub proposals: Vec<Proposal>,
    pub notices: Vec<String>,
}
#[derive(Clone)]
struct Member {
    package: Package,
    workspace: PathBuf,
}
#[derive(Clone, PartialEq, Eq)]
struct Key {
    id: u64,
    version: u64,
    epoch: u64,
    manifests: Vec<(u64, u64, u64)>,
}
struct Input {
    path: PathBuf,
    source: rc_text::Rope,
    members: Vec<Member>,
    manifests: BTreeMap<PathBuf, rc_text::Rope>,
    paste: Option<(usize, usize)>,
}
pub struct MissingCrates {
    key: Option<Key>,
    due: Instant,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    inbox: Inbox<(u64, Report)>,
    pub report: Report,
    pasted: Option<(u64, u64, (usize, usize))>,
}
impl MissingCrates {
    pub fn new(waker: Waker) -> Self {
        Self {
            key: None,
            due: Instant::now(),
            generation: 0,
            cancel: None,
            inbox: Inbox::new(waker),
            report: Report::default(),
            pasted: None,
        }
    }
    pub fn pasted(&mut self, id: u64, version: u64, range: Option<(usize, usize)>) {
        self.pasted = range.map(|range| (id, version, range));
        self.key = None;
    }
    fn invalidate(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.generation = self.generation.wrapping_add(1);
        self.report = Report::default();
    }
    pub fn update(
        &mut self,
        docs: &crate::docs::Docs,
        runner: &crate::runner::Runner,
        epoch: u64,
        ctx: &egui::Context,
    ) {
        let Some(doc) = docs.active_doc().filter(|doc| {
            doc.path.extension().is_some_and(|ext| ext == "rs")
                && !doc.scratch
                && !doc.generated
                && !doc.editor.read_only
                && doc.editor.buffer.rope().len_bytes() <= 1024 * 1024
        }) else {
            if self.key.take().is_some() {
                self.invalidate();
            }
            return;
        };
        let manifest_docs: Vec<_> = docs
            .list
            .iter()
            .filter(|doc| {
                doc.path
                    .file_name()
                    .is_some_and(|name| name == "Cargo.toml")
                    || doc.path.extension().is_some_and(|ext| ext == "rs")
            })
            .take(65)
            .collect();
        let key = Key {
            id: doc.editor.id,
            version: doc.editor.buffer.version(),
            epoch,
            manifests: manifest_docs
                .iter()
                .map(|doc| (doc.editor.id, doc.editor.buffer.version(), doc.disk_epoch))
                .collect(),
        };
        if self.key.as_ref() != Some(&key) {
            self.invalidate();
            self.key = Some(key.clone());
            self.due = Instant::now()
                + Duration::from_millis(
                    if self
                        .pasted
                        .is_some_and(|(id, version, _)| id == key.id && version == key.version)
                    {
                        0
                    } else {
                        450
                    },
                );
        }
        for (generation, report) in self.inbox.drain() {
            if generation == self.generation {
                self.report = report;
            }
        }
        if self.cancel.is_some() {
            return;
        }
        let remaining = self.due.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            ctx.request_repaint_after(remaining);
            return;
        }
        // A completed job retains its cancel token, so each key is scanned only once.
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let members: Vec<_> = runner
            .cargo_workspace
            .iter()
            .chain(&runner.attached)
            .flat_map(|workspace| {
                workspace.packages.iter().map(|package| Member {
                    package: package.clone(),
                    workspace: workspace.manifest.clone(),
                })
            })
            .take(257)
            .collect();
        if members.len() > 256
            || manifest_docs.len() > 64
            || manifest_docs
                .iter()
                .map(|doc| doc.editor.buffer.rope().len_bytes())
                .sum::<usize>()
                > 8 * 1024 * 1024
        {
            return;
        }
        let input = Input {
            path: doc.path.clone(),
            source: doc.editor.buffer.snapshot(),
            members,
            manifests: manifest_docs
                .into_iter()
                .map(|doc| (doc.path.clone(), doc.editor.buffer.snapshot()))
                .collect(),
            paste: self
                .pasted
                .filter(|(id, version, _)| *id == key.id && *version == key.version)
                .map(|(_, _, range)| range),
        };
        let generation = self.generation;
        let out = self.inbox.sender();
        std::thread::spawn(move || {
            let report = scan(input, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                out.send((generation, report));
            }
        });
    }
}
impl Drop for MissingCrates {
    fn drop(&mut self) {
        self.invalidate();
    }
}
fn read(path: &Path, input: &Input, limit: usize) -> Option<String> {
    if let Some(rope) = input.manifests.get(path) {
        return (rope.len_bytes() <= limit).then(|| rope.to_string());
    }
    use std::io::Read;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > limit as u64 {
        return None;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(limit as u64 + 1).read_to_string(&mut text).ok()?;
    (text.len() <= limit).then_some(text)
}
fn notice(report: &mut Report, message: String) {
    if report.notices.len() < 8 && !report.notices.contains(&message) {
        report.notices.push(message);
    }
}
fn scan(input: Input, cancel: &AtomicBool) -> Report {
    let mut report = Report::default();
    let Some(path) = input.path.canonicalize().ok() else {
        return report;
    };
    let mut members = input.members.clone();
    members.sort_by_key(|member| std::cmp::Reverse(member.package.manifest.components().count()));
    members.dedup_by(|a, b| a.package.manifest == b.package.manifest);
    let exact: Vec<_> = members
        .iter()
        .filter(|member| {
            member
                .package
                .targets
                .iter()
                .any(|target| target.src.canonicalize().ok().as_ref() == Some(&path))
        })
        .collect();
    let owner = if exact.len() == 1 {
        exact.first().copied()
    } else if exact.len() > 1 {
        None
    } else {
        members.iter().find(|member| {
            member
                .package
                .manifest
                .parent()
                .and_then(|root| root.canonicalize().ok())
                .is_some_and(|root| path.starts_with(root))
        })
    };
    let Some(owner) = owner else { return report };
    let Some(root) = owner.package.manifest.parent() else {
        return report;
    };
    let target = owner
        .package
        .targets
        .iter()
        .find(|target| target.src.canonicalize().ok().as_ref() == Some(&path));
    let scope = match target.map(|target| target.kind) {
        Some(TargetKind::Other) => Scope::Build,
        Some(TargetKind::Test | TargetKind::Bench | TargetKind::Example) => Scope::Dev,
        Some(TargetKind::Lib | TargetKind::Bin) => Scope::Normal,
        None if ["tests", "benches", "examples"]
            .iter()
            .any(|dir| input.path.starts_with(root.join(dir))) =>
        {
            Scope::Dev
        }
        None if input.path.starts_with(root.join("src")) => Scope::Normal,
        _ => return report,
    };
    let Some(manifest) = read(&owner.package.manifest, &input, 4 * 1024 * 1024) else {
        notice(
            &mut report,
            tr("Dependency proposals could not read the package manifest").into(),
        );
        return report;
    };
    let workspace = read(&owner.workspace, &input, 4 * 1024 * 1024);
    let Ok(names) = rc_cargo::dependency_names::dependency_names(&manifest, workspace.as_deref())
    else {
        notice(
            &mut report,
            tr("Dependency proposals could not read the package manifest").into(),
        );
        return report;
    };
    let source = input.source.to_string();
    let mut references = rc_syntax::crate_refs::references(&source, cancel);
    if references.limited {
        notice(
            &mut report,
            tr("Dependency proposal analysis reached its limit").into(),
        );
        return report;
    }
    let mut locals = references.local_names.clone();
    // Inspect target roots and conventional ancestor modules, bounded and never execute code.
    let mut files: BTreeSet<_> = owner
        .package
        .targets
        .iter()
        .map(|target| target.src.clone())
        .collect();
    for dir in input
        .path
        .ancestors()
        .skip(1)
        .take(16)
        .take_while(|dir| dir.starts_with(root))
    {
        for name in ["mod.rs", "lib.rs", "main.rs"] {
            files.insert(dir.join(name));
        }
    }
    if files.len() > 64 {
        notice(
            &mut report,
            tr("Dependency proposal analysis reached its limit").into(),
        );
        return report;
    }
    let mut budget = 4 * 1024 * 1024;
    for file in files {
        if cancel.load(Ordering::Relaxed) {
            return Report::default();
        }
        if file == input.path {
            continue;
        }
        if !file.exists() {
            continue;
        }
        if let Some(text) = read(&file, &input, budget.min(1024 * 1024)) {
            budget = budget.saturating_sub(text.len());
            let parsed = rc_syntax::crate_refs::references(&text, cancel);
            if parsed.limited {
                return report;
            }
            locals.extend(parsed.local_names);
            locals.extend(parsed.imported.into_keys());
        } else {
            notice(
                &mut report,
                tr("Dependency proposal analysis reached its limit").into(),
            );
            return report;
        }
    }
    let paste = input.paste.map(|(start, end)| {
        (
            input
                .source
                .char_to_byte(start.min(input.source.len_chars())),
            input.source.char_to_byte(end.min(input.source.len_chars())),
        )
    });
    let mut seen = BTreeSet::new();
    references
        .references
        .sort_by_key(|reference| reference.test_only);
    for reference in references.references {
        if cancel.load(Ordering::Relaxed) || report.proposals.len() >= 8 {
            break;
        }
        let key = &reference.root;
        if locals.contains(key)
            || references.imported.get(key).is_some_and(|base| base != key)
            || owner.package.targets.iter().any(|target| {
                target.kind == TargetKind::Lib && target.name.replace('-', "_") == *key
            })
        {
            continue;
        }
        let scope = if scope == Scope::Normal && reference.test_only {
            Scope::Dev
        } else {
            scope
        };
        if (scope == Scope::Dev && seen.contains(&(key.clone(), Scope::Normal.table())))
            || !seen.insert((key.clone(), scope.table()))
            || names.scope(scope).roots.contains(key)
        {
            continue;
        }
        let local: Vec<_> = members
            .iter()
            .filter(|member| {
                member.package.manifest != owner.package.manifest
                    && member.package.targets.iter().any(|target| {
                        target.kind == TargetKind::Lib && target.name.replace('-', "_") == *key
                    })
            })
            .collect();
        let (name, local_path, lookup) =
            if local.len() == 1 && local[0].package.name.replace('-', "_") == *key {
                (
                    local[0].package.name.clone(),
                    local[0].package.manifest.parent().map(Path::to_path_buf),
                    false,
                )
            } else if !local.is_empty() {
                notice(
                    &mut report,
                    trf(
                        "Dependency {crate} is ambiguous; review the manifest manually",
                        &[("crate", key)],
                    ),
                );
                continue;
            } else if let Some((_, package)) = CATALOG.iter().find(|(root, _)| root == key) {
                (package.to_string(), None, false)
            } else if !references.glob && rc_cargo::valid_crate_name(key)
                && key.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
            {
                // Import roots normalize package hyphens to underscores. They are search
                // terms, not an asserted registry identity: require selecting a search hit.
                (key.clone(), None, true)
            } else {
                continue;
            };
        if references.glob
            || names
                .scope(scope)
                .packages
                .contains_key(&name.replace('-', "_"))
        {
            notice(
                &mut report,
                trf(
                    "Dependency {crate} may be available through an alias or glob; review the manifest manually",
                    &[("crate", key)],
                ),
            );
            continue;
        }
        // A sibling module could be loaded through a different declaration or cfg branch.
        if input
            .path
            .ancestors()
            .skip(1)
            .take(16)
            .take_while(|dir| dir.starts_with(root))
            .any(|dir| {
                dir.join(format!("{key}.rs")).exists() || dir.join(key).join("mod.rs").exists()
            })
        {
            continue;
        }
        report.proposals.push(Proposal {
            root: key.clone(),
            name,
            manifest: owner.package.manifest.clone(),
            package: owner.package.name.clone(),
            scope,
            local: local_path,
            lookup,
            pasted: paste
                .is_some_and(|(start, end)| reference.start < end && reference.end > start),
        });
    }
    report
}

pub fn banner(app: &mut crate::app::CrabCraftApp, ui: &mut egui::Ui) {
    let mut selected = None;
    for proposal in &app.missing_crates.report.proposals {
        ui.horizontal_wrapped(|ui| {
            ui.label(trf(
                if proposal.pasted {
                    "Pasted code may need {crate} in {package} [{scope}]"
                } else {
                    "Code may need {crate} in {package} [{scope}]"
                },
                &[
                    ("crate", &proposal.root),
                    ("package", &proposal.package),
                    ("scope", &proposal.scope.table()),
                ],
            ));
            if ui.button(if proposal.lookup { tr("Look up crate…") } else { tr("Review dependency…") }).clicked() {
                selected = Some(proposal.clone());
            }
        });
    }
    for notice in &app.missing_crates.report.notices {
        ui.small(notice);
    }
    if let Some(proposal) = selected {
        app.cargo_dependencies.review(
            &proposal.manifest,
            &proposal.name,
            proposal.scope,
            proposal.local.as_deref(),
        );
        if proposal.lookup { app.cargo_dependencies.name.clear(); }
        app.dialog = Some(crate::ui::dialogs::Dialog::CargoDependencies);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Input) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers=[\"app\",\"helper-lib\"]\nresolver=\"2\"\n",
        )
        .unwrap();
        let mut members = Vec::new();
        for name in ["app", "helper-lib"] {
            let dir = root.join(name);
            std::fs::create_dir_all(dir.join("src")).unwrap();
            std::fs::write(
                dir.join("Cargo.toml"),
                format!("[package]\nname=\"{name}\"\nversion=\"0.1.0\"\nedition=\"2021\"\n"),
            )
            .unwrap();
            std::fs::write(dir.join("src/lib.rs"), "pub fn value() -> u32 {42}\n").unwrap();
            members.push(Member {
                package: Package {
                    name: name.into(),
                    manifest: dir.join("Cargo.toml"),
                    edition: "2021".into(),
                    cdylib: false,
                    features: Default::default(),
                    targets: vec![rc_cargo::Target {
                        name: name.replace('-', "_"),
                        kind: TargetKind::Lib,
                        src: dir.join("src/lib.rs"),
                        required_features: vec![],
                    }],
                },
                workspace: root.join("Cargo.toml"),
            });
        }
        let input = Input {
            path: root.join("app/src/lib.rs"),
            source: rc_text::Rope::from_str(
                "use serde::de::DeserializeOwned; use helper_lib::value;",
            ),
            members,
            manifests: Default::default(),
            paste: Some((0, 30)),
        };
        (temp, input)
    }
    #[test]
    fn actual_workspace_routes_catalog_and_local_library_without_changing_files() {
        let (_temp, input) = fixture();
        let manifest = input.members[0].package.manifest.clone();
        let before = std::fs::read(&manifest).unwrap();
        let report = scan(input, &AtomicBool::new(false));
        assert_eq!(report.proposals.len(), 2);
        let serde = report
            .proposals
            .iter()
            .find(|proposal| proposal.name == "serde")
            .unwrap();
        assert!(serde.pasted);
        assert_eq!(serde.scope, Scope::Normal);
        assert!(serde.local.is_none());
        assert_eq!(serde.manifest, manifest);
        let local = report
            .proposals
            .iter()
            .find(|proposal| proposal.name == "helper-lib")
            .unwrap();
        assert!(local.local.as_ref().unwrap().ends_with("helper-lib"));
        assert_eq!(std::fs::read(&manifest).unwrap(), before);
    }
    #[test]
    fn aliases_modules_and_globs_refuse_guesses_and_scopes_use_member_manifest() {
        let (_temp, mut input) = fixture();
        input.source = rc_text::Rope::from_str(
            "mod serde {} use serde::Value; use crate as regex; fn f(){regex::value();} use other::*; use tokio::task;",
        );
        let report = scan(input, &AtomicBool::new(false));
        assert!(report.proposals.is_empty());
        assert_eq!(report.notices.len(), 1);
        let (_temp, mut input) = fixture();
        let manifest = input.members[0].package.manifest.clone();
        let text = std::fs::read_to_string(&manifest).unwrap()
            + "\n[dependencies]\nserde_alias={package=\"serde\",version=\"1\"}\n";
        input
            .manifests
            .insert(manifest, rc_text::Rope::from_str(&text));
        input.source = rc_text::Rope::from_str("use serde::Serialize;");
        let report = scan(input, &AtomicBool::new(false));
        assert!(report.proposals.is_empty());
        assert_eq!(report.notices.len(), 1);
        let (_temp, mut input) = fixture();
        input.source = rc_text::Rope::from_str("#[cfg(test)] mod tests { use tempfile::tempdir; }");
        let report = scan(input, &AtomicBool::new(false));
        assert_eq!(report.proposals[0].scope, Scope::Dev);
        let (_temp, mut input) = fixture();
        let path = input
            .path
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("build.rs");
        std::fs::write(&path, "fn main() {} ").unwrap();
        input.members[0].package.targets.push(rc_cargo::Target {
            name: "build-script-build".into(),
            kind: TargetKind::Other,
            src: path.clone(),
            required_features: vec![],
        });
        input.path = path;
        input.source = rc_text::Rope::from_str("fn main(){regex::Regex::new(\"x\");}");
        let report = scan(input, &AtomicBool::new(false));
        assert_eq!(report.proposals[0].scope, Scope::Build);
    }
    #[test]
    fn cancellation_and_existing_dependency_do_not_offer_actions() {
        let (_temp, input) = fixture();
        assert!(scan(input, &AtomicBool::new(true)).proposals.is_empty());
        let (_temp, mut input) = fixture();
        input.source = rc_text::Rope::from_str("use serde::Serialize;");
        let manifest = input.members[0].package.manifest.clone();
        let text = std::fs::read_to_string(&manifest).unwrap()
            + "\n[dependencies]\nserde={version=\"1\",optional=true}\n";
        input
            .manifests
            .insert(manifest, rc_text::Rope::from_str(&text));
        assert!(scan(input, &AtomicBool::new(false)).proposals.is_empty());
    }

    #[test]
    fn arbitrary_import_roots_offer_lookup_without_asserting_a_package_identity() {
        let (_temp, mut input) = fixture();
        input.source = rc_text::Rope::from_str("fn f() { let mut b = itoa::Buffer::new(); let _ = b.format(42); }\nuse custom_external_crate::Thing;\nuse std::path::Path;\nstruct Local; impl Local { fn make() {} } fn g(){ Local::make(); }");
        let report = scan(input, &AtomicBool::new(false));
        assert_eq!(report.proposals.len(), 2);
        assert!(report.proposals.iter().all(|p| p.lookup && p.local.is_none()));
        assert!(report.proposals.iter().any(|p| p.root == "itoa"));
        assert!(report.proposals.iter().any(|p| p.root == "custom_external_crate"));
    }


}
