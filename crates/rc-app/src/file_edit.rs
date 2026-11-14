//! Guarded asynchronous opening for compiler/manifest edits. Reading never runs project code.
use crate::{app::CrabCraftApp, disk_open, docs::TextReadError, feature_fix::FeatureFix};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::tr;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
const LIMIT: usize = 8 * 1024 * 1024;
#[derive(Clone)]
pub enum Intent {
    Feature(FeatureFix),
    Build(rc_dotnet::diagnostics::Fix),
}
#[derive(Clone)]
struct Existing {
    id: u64,
    version: u64,
    epoch: u64,
    rope: rc_text::Rope,
}
#[derive(Debug)]
pub enum Error {
    Read(TextReadError),
    Changed,
    ReadOnly,
    Limit,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => error.fmt(f),
            Self::Changed => f.write_str(tr("The document changed while preparing the edit; unsaved edits were kept")),
            Self::ReadOnly => f.write_str(tr("This file is read-only")),
            Self::Limit => f.write_str(tr("The file edit exceeds its size or change limit")),
        }
    }
}
struct Input {
    path: PathBuf,
    request: disk_open::Request,
    intent: Intent,
    existing: Option<Existing>,
    origin: Option<(u64, u64)>,
    workspace: Option<PathBuf>,
}
impl Input {
    fn capture(app: &mut CrabCraftApp, path: &Path, intent: Intent) -> Result<Self, Error> {
        let existing = match app.docs.index_of(path).and_then(|i| app.docs.list.get(i)) {
            Some(doc) => {
                if doc.generated || doc.editor.read_only {
                    return Err(Error::ReadOnly);
                }
                if matches!(intent, Intent::Build(_)) && doc.is_modified() {
                    return Err(Error::Changed);
                }
                if doc.editor.buffer.rope().len_bytes() > LIMIT {
                    return Err(Error::Limit);
                }
                Some(Existing { id: doc.editor.id, version: doc.editor.buffer.version(), epoch: doc.disk_epoch, rope: doc.editor.buffer.snapshot() })
            }
            None => None,
        };
        let origin = app.docs.active_doc().map(|doc| (doc.editor.id, doc.editor.buffer.version()));
        let workspace = app.workspace.as_ref().map(|w| w.ws.root.clone());
        let request = disk_open::Request::new(&mut app.docs, path, None);
        Ok(Self { path: path.to_path_buf(), request, intent, existing, origin, workspace })
    }
}
pub struct Plan {
    path: PathBuf,
    closed: Option<disk_open::Opened>,
    intent: Intent,
    existing: Option<Existing>,
    origin: Option<(u64, u64)>,
    workspace: Option<PathBuf>,
    changes: Vec<rc_text::Change>,
}
fn prepare(input: Input, stop: &AtomicBool) -> Result<Plan, Error> {
    let Input { path, request, intent, existing, origin, workspace } = input;
    if stop.load(Ordering::Acquire) {
        return Err(Error::Read(TextReadError::Cancelled));
    }
    let (path, closed, text) = match (&intent, &existing) {
        (Intent::Feature(_), Some(snapshot)) => (path, None, snapshot.rope.to_string()),
        _ => {
            let path = path.canonicalize().map_err(|error| Error::Read(TextReadError::Io(error)))?;
            if std::fs::metadata(&path).map_err(|error| Error::Read(TextReadError::Io(error)))?.len() > LIMIT as u64 {
                return Err(Error::Limit);
            }
            let (text, bom, mtime) = crate::docs::read_text_cancellable(&path, Some(stop)).map_err(Error::Read)?;
            if text.len() > LIMIT {
                return Err(Error::Limit);
            }
            if let Intent::Build(fix) = &intent {
                let mut bytes = Vec::with_capacity(text.len() + 3);
                if bom {
                    bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
                }
                bytes.extend_from_slice(text.as_bytes());
                if rc_dotnet::diagnostics::source_hash(&bytes) != fix.source {
                    return Err(Error::Changed);
                }
            }
            let normalized = text.replace("\r\n", "\n");
            if existing.as_ref().is_some_and(|snapshot| snapshot.rope != normalized.as_str()) {
                return Err(Error::Changed);
            }
            let closed =
                if existing.is_none() { Some(disk_open::prepare_loaded(&request, path.clone(), text, bom, mtime, stop).map_err(Error::Read)?) } else { None };
            (path, closed, normalized)
        }
    };
    let changes = match &intent {
        Intent::Feature(fix) => {
            if text.len() > 1024 * 1024 {
                return Err(Error::Limit);
            }
            fix.manifest_edit(&text).map(|edit| vec![rc_text::Change::new(edit.start, edit.end, edit.text)]).unwrap_or_default()
        }
        Intent::Build(fix) => {
            // Convert the compiler's original byte offsets using a separate checked raw read.
            let (raw, bom, _) = crate::docs::read_text_cancellable(&path, Some(stop)).map_err(Error::Read)?;
            let raw = if bom { format!("\u{feff}{raw}") } else { raw };
            if rc_dotnet::diagnostics::source_hash(raw.as_bytes()) != fix.source || raw.trim_start_matches('\u{feff}').replace("\r\n", "\n") != text {
                return Err(Error::Changed);
            }
            crate::app::fix_changes(&raw, fix).ok_or(Error::Changed)?
        }
    };
    if changes.len() > 256 || changes.iter().try_fold(0usize, |total, edit| total.checked_add(edit.text.len())).is_none_or(|total| total > 1024 * 1024) {
        return Err(Error::Limit);
    }
    let len = text.chars().count();
    let mut end = 0;
    for change in &changes {
        if change.start < end || change.end < change.start || change.end > len {
            return Err(Error::Changed);
        }
        end = change.end;
    }
    if stop.load(Ordering::Acquire) {
        return Err(Error::Read(TextReadError::Cancelled));
    }
    Ok(Plan { path, closed, intent, existing, origin, workspace, changes })
}
impl Plan {
    pub fn apply(self, app: &mut CrabCraftApp) -> Result<(), Error> {
        if app.workspace.as_ref().map(|w| w.ws.root.clone()) != self.workspace
            || app.docs.active_doc().map(|doc| (doc.editor.id, doc.editor.buffer.version())) != self.origin
        {
            return Err(Error::Changed);
        }
        let index = match self.existing {
            Some(expected) => app
                .docs
                .list
                .iter()
                .position(|doc| {
                    doc.editor.id == expected.id
                        && doc.path == self.path
                        && doc.editor.buffer.version() == expected.version
                        && doc.disk_epoch == expected.epoch
                        && !doc.editor.read_only
                        && !doc.generated
                })
                .ok_or(Error::Changed)?,
            None => {
                if app.docs.index_of(&self.path).is_some() {
                    return Err(Error::Changed);
                }
                let opened = self.closed.ok_or(Error::Changed)?;
                if opened.doc.editor.read_only || app.docs.read_only_roots.iter().any(|root| self.path.starts_with(root)) {
                    return Err(Error::ReadOnly);
                }
                app.docs.insert_prepared(opened.doc, false, opened.parse)
            }
        };
        app.docs.activate(index);
        let caret = self.changes.first().map(|edit| edit.start + edit.text.chars().count());
        if self.changes.is_empty() {
            app.notify(tr("The feature is already turned on in Cargo.toml"), false);
        } else {
            app.docs.list[index].editor.apply_text_edits(self.changes);
            if let Some(caret) = caret {
                app.docs.list[index].editor.set_selection(rc_text::Selection::point(caret));
            }
        }
        if let Intent::Build(fix) = self.intent
            && let Some(build) = &mut app.runner.build
        {
            for diagnostic in build.diagnostics.iter_mut().filter(|d| d.fix.as_ref() == Some(&fix)) {
                diagnostic.fix = None;
            }
        }
        app.focus_editor = true;
        app.session_dirty = true;
        Ok(())
    }
}
struct Job {
    generation: u64,
    stop: Arc<AtomicBool>,
    inbox: Inbox<Result<Plan, Error>>,
}
#[derive(Default)]
pub struct Edits {
    generation: u64,
    jobs: Vec<Job>,
    pending: Option<Input>,
    cancelled: bool,
    #[cfg(test)]
    gate: Option<Arc<AtomicBool>>,
}
impl Edits {
    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.jobs.iter().any(|job| job.generation == self.generation)
    }
    pub fn cancel(&mut self) {
        self.cancelled |= self.busy();
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        for job in &self.jobs {
            job.stop.store(true, Ordering::Release);
        }
    }
    pub fn take_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.cancelled)
    }
    fn launch(&mut self, waker: Waker) -> Result<(), Error> {
        if self.jobs.len() >= 2 {
            return Ok(());
        }
        let Some(input) = self.pending.take() else { return Ok(()) };
        let inbox = Inbox::new(waker);
        let tx = inbox.sender();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        #[cfg(test)]
        let gate = self.gate.clone();
        std::thread::Builder::new()
            .name("rc-file-edit".into())
            .spawn(move || {
                #[cfg(test)]
                if let Some(gate) = gate {
                    while !gate.load(Ordering::Acquire) && !flag.load(Ordering::Acquire) {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                }
                tx.send(prepare(input, &flag))
            })
            .map_err(|error| Error::Read(TextReadError::Io(error)))?;
        self.jobs.push(Job { generation: self.generation, stop, inbox });
        Ok(())
    }
    pub fn poll(&mut self, waker: Waker) -> Vec<Result<Plan, Error>> {
        let mut results = Vec::new();
        let mut index = 0;
        while index < self.jobs.len() {
            let Some(result) = self.jobs[index].inbox.drain().into_iter().next() else {
                index += 1;
                continue;
            };
            let job = self.jobs.remove(index);
            if job.generation == self.generation {
                results.push(result);
            }
        }
        if let Err(error) = self.launch(waker) {
            results.push(Err(error));
        }
        results
    }
    pub fn show(&mut self, ctx: &egui::Context) {
        if self.busy() {
            egui::Window::new(tr("Preparing file edit")).id(egui::Id::unique("file-edit-progress")).collapsible(false).show(ctx, |ui| {
                ui.spinner();
                if ui.button(tr("Cancel")).clicked() {
                    self.cancel();
                }
            });
        }
    }
}
impl Drop for Edits {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl CrabCraftApp {
    pub(crate) fn request_file_edit(&mut self, path: &Path, intent: Intent) {
        if self.file_edits.busy() {
            self.cancel_disk_save();
        }
        self.file_edits.cancel();
        self.file_edits.cancelled = false;
        self.disk_open.cancel();
        self.push_nav();
        let input = match Input::capture(self, path, intent) {
            Ok(input) => input,
            Err(error) => {
                self.notify(error.to_string(), true);
                return;
            }
        };
        self.file_edits.pending = Some(input);
        if let Err(error) = self.file_edits.launch(self.waker.clone()) {
            self.notify(error.to_string(), true);
        }
    }
    pub(crate) fn poll_file_edits(&mut self) {
        if self.file_edits.take_cancelled() {
            self.cancel_disk_save();
        }
        for result in self.file_edits.poll(self.waker.clone()) {
            if let Err(error) = result.and_then(|plan| plan.apply(self)) {
                self.cancel_disk_save();
                self.notify(error.to_string(), true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};
    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname=\"editable_file_proof\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n[features]\nstd = []\ndefault = []\n",
        )
        .unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main(){println!(\"done\");}\n").unwrap();
        root
    }
    fn frames(h: &mut egui_kittest::Harness<'_, CrabCraftApp>, done: impl Fn(&CrabCraftApp) -> bool) {
        let until = Instant::now() + Duration::from_secs(30);
        while !done(h.state()) {
            h.run_steps(1);
            assert!(Instant::now() < until, "file edit did not finish: {:?}", h.state().notices.iter().map(|n| &n.text).collect::<Vec<_>>());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn closed_manifest_edit_precedes_real_cargo_without_running_during_reading() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = fixture();
        std::fs::write(
            root.path().join("build.rs"),
            "fn main(){assert!(std::env::var_os(\"CARGO_FEATURE_STD\").is_some());std::fs::write(\"build-observed\",\"latest feature\").unwrap();}",
        )
        .unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(root.path());
        app.trust_project_tools_for_test();
        app.open_file_and_wait(&root.path().join("src/main.rs"));
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 850.0));
        frames(&mut h, |app| app.runner.cargo_workspace.is_some());
        let manifest = root.path().join("Cargo.toml");
        h.state_mut().apply_feature_fix(FeatureFix::Default { manifest: manifest.clone(), package: "editable_file_proof".into(), feature: "std".into() });
        assert!(h.state().docs.index_of(&manifest).is_none());
        assert!(!root.path().join("build-observed").exists());
        let ctx = h.ctx.clone();
        h.state_mut().run_ide_action(&ctx, rc_editor::Action::Build);
        frames(&mut h, |app| !app.file_edits.busy() && app.runner.build.as_ref().is_some_and(|build| build.result.is_some()));
        assert_eq!(std::fs::read_to_string(root.path().join("build-observed")).unwrap(), "latest feature");
        assert!(std::fs::read_to_string(&manifest).unwrap().contains("default = [\"std\"]"));
    }
    #[test]
    fn closed_bom_crlf_compiler_edits_are_undoable_and_stale_open_edits_are_kept() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = fixture();
        let main = root.path().join("src/main.rs");
        let text = "\u{feff}fn main() {\r\n    // café\r\n    println!(\"{}\", 1);\r\n}\r\n";
        std::fs::write(&main, text).unwrap();
        let start = text.find("1").unwrap();
        let fix = rc_dotnet::diagnostics::Fix {
            label: "use 42".into(),
            source: rc_dotnet::diagnostics::source_hash(text.as_bytes()),
            edits: vec![(start, start + 1, "42".into())],
        };
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(root.path());
        app.open_file_and_wait(&root.path().join("Cargo.toml"));
        app.apply_build_fix(&main, &fix);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 850.0));
        frames(&mut h, |app| !app.file_edits.busy());
        let doc = h.state().docs.active_doc().unwrap();
        assert_eq!(doc.path, main);
        assert!(doc.bom && doc.is_modified());
        assert_eq!(doc.editor.buffer.line_ending(), rc_text::LineEnding::CrLf);
        assert!(doc.editor.buffer.rope().to_string().contains(", 42)"));
        assert_eq!(std::fs::read_to_string(&main).unwrap(), text);
        h.state_mut().docs.active_doc_mut().unwrap().editor.undo();
        assert!(!h.state().docs.active_doc().unwrap().is_modified());
        h.state_mut().apply_build_fix(&main, &fix);
        h.state_mut().docs.active_doc_mut().unwrap().editor.replace(0, 0, "// later edit\n");
        frames(&mut h, |app| !app.file_edits.busy());
        assert!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string().starts_with("// later edit"));
        assert!(!h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string().contains(", 42)"));
        assert_eq!(std::fs::read_to_string(main).unwrap(), text);
    }
    #[test]
    fn native_cancel_of_pending_edit_aborts_waiting_cargo_and_keeps_buffers() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = fixture();
        std::fs::write(root.path().join("build.rs"), "fn main(){std::fs::write(\"build-observed\",\"unexpected\").unwrap();}").unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(root.path());
        app.trust_project_tools_for_test();
        app.open_file_and_wait(&root.path().join("src/main.rs"));
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 850.0));
        frames(&mut h, |app| app.runner.cargo_workspace.is_some());
        h.state_mut().file_edits.gate = Some(Arc::new(AtomicBool::new(false)));
        let manifest = root.path().join("Cargo.toml");
        h.state_mut().apply_feature_fix(FeatureFix::Default { manifest: manifest.clone(), package: "editable_file_proof".into(), feature: "std".into() });
        let ctx = h.ctx.clone();
        h.state_mut().run_ide_action(&ctx, rc_editor::Action::Build);
        h.run_steps(3);
        h.get_by_label(tr("Cancel")).click();
        h.run_steps(3);
        assert!(!h.state().file_edits.busy());
        assert!(h.state().disk_save.continuation.is_none());
        assert!(h.state().runner.build.is_none());
        assert!(!root.path().join("build-observed").exists());
        assert!(h.state().docs.index_of(&manifest).is_none());
        assert_eq!(h.state().docs.list.len(), 1);
    }
}
