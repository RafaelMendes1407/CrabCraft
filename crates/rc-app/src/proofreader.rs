//! Explicit local proofreading with reviewed, version-guarded comment corrections.
use crate::{
    docs::{Doc, Docs},
    proofreader_core::{self, Error, Issue},
};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone)]
struct Snapshot {
    id: u64,
    path: PathBuf,
    version: u64,
    epoch: u64,
    rope: rc_text::Rope,
}
impl Snapshot {
    fn take(doc: &Doc) -> Result<Self, Error> {
        if doc.editor.lang() != rc_syntax::Lang::Rust || doc.editor.read_only || doc.generated {
            return Err(Error::Document);
        }
        if doc.editor.buffer.rope().len_bytes() > 1024 * 1024 {
            return Err(Error::Limit);
        }
        Ok(Self { id: doc.editor.id, path: doc.path.clone(), version: doc.editor.buffer.version(), epoch: doc.disk_epoch, rope: doc.editor.buffer.snapshot() })
    }
    fn matches(&self, doc: &Doc) -> bool {
        self.id == doc.editor.id
            && self.path == doc.path
            && self.version == doc.editor.buffer.version()
            && self.epoch == doc.disk_epoch
            && !doc.editor.read_only
            && !doc.generated
    }
}
struct Job {
    inbox: Inbox<Result<Vec<Issue>, Error>>,
    cancel: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.task.abort();
    }
}
pub struct Proofreader {
    pub endpoint: String,
    pub language: String,
    job: Option<Job>,
    snapshot: Option<Snapshot>,
    issues: Vec<Issue>,
    selected: usize,
    replacement: usize,
    error: Option<Error>,
    finished: bool,
}
impl Default for Proofreader {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8081".into(),
            language: "en-US".into(),
            job: None,
            snapshot: None,
            issues: Vec::new(),
            selected: 0,
            replacement: 0,
            error: None,
            finished: false,
        }
    }
}
impl Proofreader {
    pub fn cancel(&mut self) {
        self.job = None;
        self.snapshot = None;
        self.issues.clear();
        self.error = None;
        self.finished = false;
    }
    fn start(&mut self, docs: &Docs, trusted: bool, waker: Waker) -> Result<(), Error> {
        self.cancel();
        if !trusted {
            return Err(Error::Trust);
        }
        crate::local_ai_client::endpoint(&self.endpoint).map_err(|_| Error::Endpoint)?;
        let snapshot = Snapshot::take(docs.active_doc().ok_or(Error::Document)?)?;
        let source = snapshot.rope.clone();
        let endpoint = self.endpoint.clone();
        let language = self.language.clone();
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let task = rc_core::Runtime::spawn(async move {
            let result = async {
                let check = token.clone();
                let input = tokio::task::spawn_blocking(move || proofreader_core::prepare(&source, &check)).await.map_err(|_| Error::Invalid)??;
                if token.load(Ordering::Acquire) {
                    return Err(Error::Cancelled);
                }
                proofreader_core::check(&endpoint, &language, &input).await
            }
            .await;
            out.send(result);
        });
        self.snapshot = Some(snapshot);
        self.selected = 0;
        self.replacement = 0;
        self.job = Some(Job { inbox, cancel, task });
        Ok(())
    }
    pub fn poll(&mut self, docs: &Docs, trusted: bool) {
        if self.snapshot.as_ref().is_some_and(|snapshot| !trusted || !docs.active_doc().is_some_and(|doc| snapshot.matches(doc))) {
            self.cancel();
            self.error = Some(Error::Stale);
            return;
        }
        if let Some(result) = self.job.as_ref().and_then(|job| job.inbox.drain().into_iter().next()) {
            self.job = None;
            self.finished = true;
            match result {
                Ok(issues) => self.issues = issues,
                Err(Error::Cancelled) => (),
                Err(error) => self.error = Some(error),
            }
        }
    }
    fn apply(&mut self, docs: &mut Docs, trusted: bool) -> Result<(), Error> {
        if !trusted {
            return Err(Error::Trust);
        }
        let snapshot = self.snapshot.as_ref().ok_or(Error::Stale)?;
        let doc = docs.active_doc_mut().filter(|doc| snapshot.matches(doc)).ok_or(Error::Stale)?;
        let issue = self.issues.get(self.selected).ok_or(Error::Invalid)?;
        let value = issue.replacements.get(self.replacement).ok_or(Error::Invalid)?;
        if issue.range.end > doc.editor.buffer.len_chars()
            || issue.range.start > issue.range.end
            || doc.editor.buffer.rope().slice(issue.range.clone()).to_string() != issue.original
        {
            return Err(Error::Stale);
        }
        if !proofreader_core::safe_in_source(doc.editor.buffer.rope(), &issue.range, value, issue.at_start) {
            return Err(Error::Invalid);
        }
        doc.editor.replace(issue.range.start, issue.range.end, value);
        doc.editor.selection = rc_text::Selection::point(issue.range.start + value.chars().count());
        doc.editor.request_scroll_to_caret();
        self.cancel();
        Ok(())
    }
    fn ui(&mut self, ui: &mut egui::Ui, docs: &mut Docs, trusted: bool, waker: Waker) {
        ui.label(tr(
            "Check Rust comments with a separately installed local LanguageTool server. Only an explicit check sends comment text; no project code is run.",
        ));
        let mut changed = false;
        egui::Grid::new("proofreading_settings").num_columns(2).show(ui, |ui| {
            let label = ui.label(tr("Local proofreading server"));
            changed |= ui.add(egui::TextEdit::singleline(&mut self.endpoint).char_limit(2048)).labelled_by(label.id).changed();
            ui.end_row();
            let label = ui.label(tr("Text language code"));
            changed |= ui.add(egui::TextEdit::singleline(&mut self.language).char_limit(24).hint_text("en-US / pt-BR")).labelled_by(label.id).changed();
            ui.end_row();
        });
        if changed {
            self.cancel();
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(trusted && self.job.is_none(), egui::Button::new(tr("Check Comments"))).clicked() {
                if let Err(error) = self.start(docs, trusted, waker) {
                    self.error = Some(error);
                }
            }
            if self.job.is_some() {
                ui.spinner();
                if ui.button(tr("Cancel")).clicked() {
                    self.cancel();
                }
            }
        });
        if !trusted {
            ui.label(tr("Trust this project before checking comments"));
        }
        if let Some(error) = &self.error {
            ui.colored_label(rc_kit::pal(ui).error, error_text(error));
        }
        if let Some(snapshot) = &self.snapshot {
            ui.label(snapshot.path.display().to_string());
        }
        if self.finished && self.issues.is_empty() && self.error.is_none() {
            ui.label(tr("No spelling or grammar issues found"));
        }
        let mut navigate = None;
        egui::ScrollArea::vertical().id_salt("proofreading_issues").max_height(220.0).show(ui, |ui| {
            for (index, issue) in self.issues.iter().enumerate() {
                let category = if issue.spelling { tr("Spelling") } else { tr("Grammar") };
                if ui.selectable_label(index == self.selected, format!("{category}: {}", issue.message)).clicked() {
                    self.selected = index;
                    self.replacement = 0;
                    navigate = Some(issue.range.clone());
                }
            }
        });
        if let Some(range) = navigate
            && let Some(snapshot) = &self.snapshot
            && let Some(doc) = docs.active_doc_mut().filter(|doc| snapshot.matches(doc))
        {
            doc.editor.selection = rc_text::Selection::single(rc_text::Range::new(range.start, range.end));
            doc.editor.request_scroll_to_caret();
        }
        let mut apply = false;
        if let Some(issue) = self.issues.get(self.selected) {
            ui.label(format!("{}: {}", tr("Original text"), issue.original));
            egui::ComboBox::from_id_salt("proofreading_replacement")
                .selected_text(issue.replacements.get(self.replacement).map(String::as_str).unwrap_or(""))
                .show_ui(ui, |ui| {
                    for (index, value) in issue.replacements.iter().enumerate() {
                        ui.selectable_value(&mut self.replacement, index, value);
                    }
                });
            apply = ui.add_enabled(!issue.replacements.is_empty(), egui::Button::new(tr("Apply Comment Correction"))).clicked();
        }
        if apply {
            if let Err(error) = self.apply(docs, trusted) {
                self.error = Some(error);
            }
        }
    }
}
pub fn show(app: &mut crate::app::CrabCraftApp, ui: &mut egui::Ui) {
    if crate::ui::toolwindows::header(ui, tr("Spelling and Grammar")) {
        app.toggle_tool(crate::toolwindow::ToolWindow::Proofreading);
    }
    let trusted = app.workspace.as_ref().is_some_and(|w| app.settings.trusted_projects.contains(&w.ws.root));
    app.proofreader.ui(ui, &mut app.docs, trusted, app.waker.clone());
}
fn error_text(error: &Error) -> String {
    match error {
        Error::Endpoint => tr("Choose an HTTP proofreading server on loopback without credentials or proxy paths").into(),
        Error::Language => tr("Enter a LanguageTool language code such as en-US or pt-BR").into(),
        Error::Limit => tr("Proofreading exceeded its source, comment or response limits").into(),
        Error::NoComments => tr("No prose comments found outside code fences").into(),
        Error::Invalid => tr("The proofreading server returned an invalid or unsafe correction").into(),
        Error::Network => tr("Could not reach the local proofreading server").into(),
        Error::Status(status) => trf("Proofreading server returned HTTP {status}", &[("status", status)]),
        Error::Cancelled => tr("Cancelled").into(),
        Error::Stale => tr("The source changed; check comments again").into(),
        Error::Trust => tr("Trust this project before checking comments").into(),
        Error::Document => tr("Open an editable Rust source file with valid syntax").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};
    fn ready(h: &mut egui_kittest::Harness<'_, crate::app::CrabCraftApp>) {
        let end = Instant::now() + Duration::from_secs(35);
        loop {
            h.run_steps(1);
            if h.state().proofreader.job.is_none() {
                assert!(h.state().proofreader.error.is_none(), "{:?}", h.state().proofreader.error);
                return;
            }
            assert!(Instant::now() < end, "proofreading did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    #[ignore = "requires a separately running local LanguageTool server"]
    fn real_local_grammar_spelling_native_review_stale_guard_and_rust_project() {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"proofreading_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(root.path().join("build.rs"), "fn main() { std::fs::write(\"executed\",\"yes\").unwrap(); }\n").unwrap();
        let source = "// 🦀 This are a test. This is a speling mistake.\nfn main() { assert_eq!(6*7,42); }\n";
        let path = root.path().join("src/main.rs");
        std::fs::write(&path, source).unwrap();
        let endpoint = std::env::var("CRABCRAFT_TEST_LANGUAGETOOL").unwrap_or_else(|_| "http://127.0.0.1:8086".into());
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace_and_wait(root.path());
        app.open_file_and_wait(&path);
        app.proofreader.endpoint = endpoint.clone();
        assert!(matches!(app.proofreader.start(&app.docs, false, Waker::noop()), Err(Error::Trust)));
        let workspace = app.workspace.as_ref().unwrap().ws.root.clone();
        app.settings.trusted_projects.push(workspace);
        app.dock.show(crate::toolwindow::ToolWindow::Proofreading);
        let mut h = crate::screens::harness(app, egui::vec2(1400.0, 1000.0));
        h.run_steps(2);
        h.get_by_label(tr("Check Comments")).click();
        h.run_steps(2);
        ready(&mut h);
        assert!(h.state().proofreader.issues.iter().any(|issue| !issue.spelling));
        let (index, issue) = h.state().proofreader.issues.iter().enumerate().find(|(_, issue)| issue.original == "speling").unwrap();
        assert!(issue.spelling);
        assert_eq!(issue.replacements[0], "spelling");
        let label = format!("{}: {}", tr("Spelling"), issue.message);
        let _ = index;
        h.get_by_label(&label).click();
        h.run_steps(2);
        h.get_by_label(tr("Apply Comment Correction")).click();
        h.run_steps(2);
        let corrected = h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();
        assert!(corrected.contains("spelling mistake"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        assert!(!root.path().join("executed").exists());
        h.state_mut().docs.active_doc_mut().unwrap().editor.undo();
        assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string(), source);
        h.get_by_label(tr("Check Comments")).click();
        h.run_steps(2);
        ready(&mut h);
        h.state_mut().docs.active_doc_mut().unwrap().editor.replace(0, 0, "// newest source\n");
        h.run_steps(2);
        assert!(h.state().proofreader.issues.is_empty());
        assert!(matches!(h.state().proofreader.error, Some(Error::Stale)));
        assert!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string().contains("speling"));
        let input = proofreader_core::prepare(&rc_text::Rope::from_str("// Este é um ezemplo.\nfn main() {}\n"), &AtomicBool::new(false)).unwrap();
        let portuguese = rc_core::Runtime::block_on(proofreader_core::check(&endpoint, "pt-BR", &input)).unwrap();
        assert!(portuguese.iter().any(|issue| issue.original == "ezemplo" && issue.replacements.iter().any(|r| r == "exemplo")));
        std::fs::write(&path, corrected).unwrap();
        let output = std::process::Command::new("cargo").args(["run", "--offline", "--quiet"]).current_dir(root.path()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(root.path().join("executed").exists());
    }
}
