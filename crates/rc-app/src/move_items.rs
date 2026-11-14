//! Explicit, frozen multi-buffer move preview. The original API is retained by a re-export.
use crate::{
    app::CrabCraftApp,
    move_items_core::{self as core, Budget, Error, Frozen, Plan},
};
use rc_core::Inbox;
use rc_kit::i18n::{tr, trf};
use rc_lsp::Client;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
struct Job {
    cancel: Arc<AtomicBool>,
    inbox: Inbox<Result<Arc<Plan>, Error>>,
    applying: bool,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
pub struct MoveItems {
    origin: u64,
    source: PathBuf,
    version: u64,
    caret: usize,
    pub target: PathBuf,
    pub preview: Option<Arc<Plan>>,
    pub error: Option<String>,
    job: Option<Job>,
    client: Client,
    epoch: u64,
}
fn frozen(app: &CrabCraftApp) -> Result<Vec<Frozen>, Error> {
    let root = app.workspace.as_ref().map(|workspace| &workspace.ws.root).ok_or(Error::Unsupported)?;
    let mut total = 0;
    let mut result = Vec::new();
    for doc in app.docs.list.iter().filter(|doc| !doc.generated && !doc.scratch && doc.path.starts_with(root)) {
        if doc.path.file_name().is_some_and(|name| name == "Cargo.toml") && doc.is_modified() {
            return Err(Error::Changed);
        }
        if doc.editor.lang() != rc_syntax::Lang::Rust {
            continue;
        }
        total += doc.editor.buffer.rope().len_bytes();
        if doc.editor.buffer.rope().len_bytes() > 1024 * 1024 || total > 8 * 1024 * 1024 || result.len() >= 128 {
            return Err(Error::Limit);
        }
        result.push(Frozen {
            id: doc.editor.id,
            version: doc.editor.buffer.version(),
            path: doc.path.clone(),
            text: doc.editor.buffer.snapshot(),
            readonly: doc.editor.read_only,
        });
    }
    Ok(result)
}
fn current(plan: &Plan, app: &CrabCraftApp) -> bool {
    plan.frozen.iter().all(|old| {
        app.docs
            .list
            .iter()
            .any(|doc| doc.editor.id == old.id && doc.path == old.path && doc.editor.buffer.version() == old.version && doc.editor.read_only == old.readonly)
    })
}
impl MoveItems {
    fn start(&mut self, app: &mut CrabCraftApp, applying: bool) {
        self.error = None;
        if self.job.is_some() {
            return;
        }
        if self.client_changed(app) {
            self.error = Some(error_text(Error::Changed));
            return;
        }
        let inbox = Inbox::new(app.waker.clone());
        let sender = inbox.sender();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let client = self.client.clone();
        let job = if applying {
            let Some(plan) = self.preview.clone() else {
                return;
            };
            if !current(&plan, app) {
                self.preview = None;
                self.error = Some(error_text(Error::Changed));
                return;
            }
            std::thread::Builder::new().name("rc-move-item-verify".into()).spawn(move || {
                let budget = Budget { cancel: token, deadline: Instant::now() + Duration::from_secs(15) };
                sender.send(core::verify(&plan, &budget).map(|_| plan));
            })
        } else {
            let Some(source) =
                app.docs.list.iter().find(|doc| doc.editor.id == self.origin && doc.editor.buffer.version() == self.version && doc.path == self.source)
            else {
                self.error = Some(error_text(Error::Changed));
                return;
            };
            if source.editor.read_only {
                self.error = Some(error_text(Error::Visibility));
                return;
            }
            let snapshots = match frozen(app) {
                Ok(snapshots) => snapshots,
                Err(error) => {
                    self.error = Some(error_text(error));
                    return;
                }
            };
            let Some(model) = app.runner.cargo_workspace.as_ref() else {
                self.error = Some(error_text(Error::Graph));
                return;
            };
            let Some(package) = model
                .packages
                .iter()
                .filter(|package| package.manifest.parent().is_some_and(|dir| source.path.starts_with(dir)))
                .max_by_key(|package| package.manifest.components().count())
            else {
                self.error = Some(error_text(Error::Graph));
                return;
            };
            if package.targets.len() != 1 {
                self.error = Some(error_text(Error::Graph));
                return;
            }
            let workspace = rc_cargo::CargoWorkspace {
                root: model.root.clone(),
                manifest: model.manifest.clone(),
                target_dir: model.target_dir.clone(),
                packages: vec![rc_cargo::Package {
                    cdylib: package.cdylib,
                    name: package.name.clone(),
                    manifest: package.manifest.clone(),
                    edition: package.edition.clone(),
                    targets: package
                        .targets
                        .iter()
                        .map(|target| rc_cargo::Target { name: target.name.clone(), kind: target.kind, src: target.src.clone(), required_features: Vec::new() })
                        .collect(),
                    features: Default::default(),
                }],
            };
            for doc in &mut app.docs.list {
                if doc.editor.lang() == rc_syntax::Lang::Rust {
                    app.insight.flush(doc);
                }
            }
            let origin = self.origin;
            let caret = self.caret;
            let target = self.target.clone();
            self.preview = None;
            std::thread::Builder::new().name("rc-move-item-plan".into()).spawn(move || {
                let budget = Budget { cancel: token, deadline: Instant::now() + Duration::from_secs(20) };
                sender.send(core::plan(origin, caret, target, snapshots, workspace, &client, &budget).map(Arc::new));
            })
        };
        match job {
            Ok(_) => self.job = Some(Job { cancel, inbox, applying }),
            Err(_) => self.error = Some(error_text(Error::Io)),
        }
    }
    fn client_changed(&self, app: &CrabCraftApp) -> bool {
        self.epoch != app.cfg_epoch
            || !app.workspace.as_ref().is_some_and(|workspace| app.settings.trusted_projects.contains(&workspace.ws.root))
            || !app.insight.session.as_ref().is_some_and(|session| session.client.same_connection(&self.client))
    }
    fn poll(&mut self, app: &mut CrabCraftApp) -> bool {
        if self.client_changed(app) {
            self.job = None;
            self.preview = None;
            self.error = Some(error_text(Error::Changed));
            return false;
        }
        if self.preview.as_ref().is_some_and(|plan| !current(plan, app)) {
            self.preview = None;
            self.error = Some(error_text(Error::Changed));
        }
        let result = self.job.as_ref().and_then(|job| job.inbox.drain().into_iter().next());
        if let Some(result) = result {
            let applying = self.job.take().is_some_and(|job| job.applying);
            match result {
                Ok(plan) if current(&plan, app) => {
                    if applying {
                        let valid = plan
                            .edits
                            .iter()
                            .all(|edit| app.docs.list.iter().any(|doc| doc.editor.id == edit.id && doc.path == edit.path && !doc.editor.read_only));
                        if !valid {
                            self.error = Some(error_text(Error::Changed));
                            return false;
                        }
                        for edit in &plan.edits {
                            if let Some(doc) = app.docs.list.iter_mut().find(|doc| doc.editor.id == edit.id) {
                                doc.editor.replace(edit.start, edit.end, &edit.text);
                            }
                        }
                        app.notify(trf("Moved {name} in two buffers; save or undo each file explicitly", &[("name", &plan.name)]), false);
                        return true;
                    }
                    self.preview = Some(plan);
                }
                Ok(_) => {
                    self.preview = None;
                    self.error = Some(error_text(Error::Changed));
                }
                Err(error) => self.error = Some(error_text(error)),
            }
        }
        false
    }
}
pub fn open(app: &mut CrabCraftApp) {
    let error = || tr("Trust the Rust project, wait for rust-analyzer, and place one caret in a public item").to_string();
    let Some(doc) = app.docs.active_doc().filter(|doc| {
        !doc.generated
            && !doc.scratch
            && !doc.editor.read_only
            && doc.editor.lang() == rc_syntax::Lang::Rust
            && doc.editor.selection.ranges().len() == 1
            && doc.editor.selection.primary().is_empty()
    }) else {
        app.notify(error(), false);
        return;
    };
    if !app.workspace.as_ref().is_some_and(|workspace| app.settings.trusted_projects.contains(&workspace.ws.root))
        || !app.insight.ready()
        || app.runner.cargo_workspace.is_none()
    {
        app.notify(error(), false);
        return;
    }
    let Some(client) = app.insight.session.as_ref().map(|session| session.client.clone()) else {
        return;
    };
    let target = app
        .docs
        .list
        .iter()
        .find(|candidate| {
            candidate.path != doc.path
                && !candidate.editor.read_only
                && candidate.path.parent() == doc.path.parent()
                && candidate.editor.lang() == rc_syntax::Lang::Rust
                && !matches!(candidate.path.file_name().and_then(|name| name.to_str()), Some("main.rs" | "lib.rs"))
        })
        .map(|doc| doc.path.clone())
        .unwrap_or_default();
    app.move_items = Some(MoveItems {
        origin: doc.editor.id,
        source: doc.path.clone(),
        version: doc.editor.buffer.version(),
        caret: doc.editor.caret(),
        target,
        preview: None,
        error: None,
        job: None,
        client,
        epoch: app.cfg_epoch,
    });
}
pub fn show(app: &mut CrabCraftApp, ctx: &egui::Context) {
    let Some(mut state) = app.move_items.take() else {
        return;
    };
    if state.poll(app) {
        return;
    }
    let mut open = true;
    let mut preview = false;
    let mut apply = false;
    let mut cancel = false;
    egui::Window::new(tr("Move public Rust item")).id(egui::Id::unique("move-public-rust-item")).open(&mut open).default_width(900.0).resizable(true).show(ctx,|ui|{
 ui.label(tr("Moves primitive-only public functions, structs with public fields, enums, constants and type aliases between existing sibling crate modules. The old path remains a re-export."));ui.small(tr("Unsupported: private items or fields, generics, dependent names, methods, macros, attributes, doc comments, glob imports and multiple Cargo targets. No files are saved automatically."));ui.monospace(state.source.display().to_string());
 let previous_target=state.target.clone();
 ui.horizontal(|ui|{ui.label(tr("Destination open Rust module"));egui::ComboBox::from_id_salt("move-item-target").selected_text(state.target.display().to_string()).show_ui(ui,|ui|{for doc in app.docs.list.iter().filter(|doc|doc.editor.id!=state.origin&&doc.editor.lang()==rc_syntax::Lang::Rust&&!doc.editor.read_only&&doc.path.parent()==state.source.parent()&&!matches!(doc.path.file_name().and_then(|name|name.to_str()),Some("main.rs"|"lib.rs"))){ui.selectable_value(&mut state.target,doc.path.clone(),doc.path.display().to_string());}});});
 if previous_target!=state.target{state.preview=None;state.job=None;state.error=None;}
 ui.small(tr("Open the crate root and both module files first. The crate root must match its saved text."));if let Some(error)=&state.error{ui.colored_label(rc_kit::pal(ui).error,error);}
 if let Some(plan)=&state.preview{ui.label(trf("Frozen move preview: {name}",&[("name",&plan.name)]));egui::ScrollArea::vertical().max_height(420.0).show(ui,|ui|{for edit in &plan.edits{ui.monospace(edit.path.display().to_string());ui.columns(2,|columns|{columns[0].label(tr("Before"));let mut text=edit.before.clone();columns[0].add(egui::TextEdit::multiline(&mut text).code_editor().interactive(false).desired_rows(7).desired_width(f32::INFINITY));columns[1].label(tr("After"));let mut text=edit.after.clone();columns[1].add(egui::TextEdit::multiline(&mut text).code_editor().interactive(false).desired_rows(7).desired_width(f32::INFINITY));});}});ui.small(tr("Preview text is limited to 16,384 characters per pane; apply checks every captured buffer and source file again."));}
 ui.horizontal(|ui|{preview=ui.add_enabled(state.job.is_none()&&!state.target.as_os_str().is_empty(),egui::Button::new(tr("Preview item move"))).clicked();apply=ui.add_enabled(state.job.is_none()&&state.preview.is_some(),egui::Button::new(tr("Apply move to buffers"))).clicked();cancel=ui.button(tr("Cancel")).clicked();if state.job.is_some(){ui.spinner();ui.label(tr("Checking Rust item identity and frozen sources…"));}});
 });
    if !open || cancel {
        return;
    }
    if preview {
        state.start(app, false);
    }
    if apply {
        state.start(app, true);
    }
    app.move_items = Some(state);
}
fn error_text(error: Error) -> String {
    tr(match error {
        Error::Unsupported => "This item is outside the supported move scope; no changes were made",
        Error::Context => "The move cannot prove independent names or namespace stability; macros, attributes, doc comments and glob imports are refused",
        Error::Visibility => "Only pub or pub(crate) items with equally public struct fields can move",
        Error::Collision => "The destination already contains the item name or a reference that could be captured",
        Error::Graph => "Open sibling module files and their saved crate root in a single-target Cargo package",
        Error::Changed => "Sources, metadata, trust or the Rust server changed; reopen and preview the move",
        Error::Server => "rust-analyzer could not establish an unambiguous item identity",
        Error::Cancelled => "Cancelled",
        Error::Timeout => "The Rust item move exceeded its time limit",
        Error::Limit => "The Rust item move exceeded its file, text or identity-request limit",
        Error::Io => "Could not read the regular Rust source files for the move",
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{HOME_LOCK, harness, test_app};
    use egui_kittest::kittest::Queryable;
    use rc_text::Selection;
    #[track_caller]
    fn pump(h: &mut egui_kittest::Harness<'static, CrabCraftApp>, done: impl Fn(&CrabCraftApp) -> bool) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(25) {
            h.run_steps(2);
            if done(h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "move timeout: error={:?}; preview={:?}; job={:?}; RA={:?}",
            h.state().move_items.as_ref().and_then(|state| state.error.as_ref()),
            h.state().move_items.as_ref().and_then(|state| state.preview.as_ref()).map(|plan| &plan.name),
            h.state().move_items.as_ref().and_then(|state| state.job.as_ref()).map(|job| job.applying),
            h.state().insight.status
        );
    }
    fn select(app: &mut CrabCraftApp, path: &std::path::Path, needle: &str) {
        app.open_file_and_wait(path);
        let doc = app.docs.active_doc_mut().unwrap();
        let source = doc.editor.buffer.rope().to_string();
        let byte = source.find(needle).unwrap() + needle.rfind(' ').map_or(0, |space| space + 1);
        doc.editor.set_selection(Selection::point(source[..byte].chars().count()));
    }
    fn window(app: &mut CrabCraftApp, target: &std::path::Path) {
        open(app);
        app.move_items.as_mut().unwrap().target = target.to_path_buf();
    }
    #[test]
    fn native_real_rust_item_moves_keep_aliases_types_impls_and_disk_unchanged_until_explicit_save() {
        let _lock = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"item_move_evidence\"\nversion=\"0.1.0\"\nedition=\"2021\"\n").unwrap();
        let main = root.join("src/main.rs");
        let from = root.join("src/source.rs");
        let to = root.join("src/destination.rs");
        let source = "pub fn score(/* 🦀 */ café:i32)->i32 {let doubled=café*2;doubled+1}\npub(crate) fn internal(n:i32)->i32{n+2}\npub struct Point {pub x:i32,pub y:i32}\nimpl Point {pub fn sum(&self)->i32{self.x+self.y}}\npub enum Mode {Fast,Value(i32)}\npub const OFFSET:i32=3;\npub type Count=i32;\npub fn dependent(n:i32)->i32{n+OFFSET}\n";
        let caller = "mod source;\nmod destination;\nuse source::score as compute_score;\nfn main(){let point=source::Point{x:2,y:3};let mode=source::Mode::Value(7);let enum_ok=match mode{source::Mode::Fast=>false,source::Mode::Value(value)=>value==7};let count:source::Count=source::OFFSET;let ok=compute_score(4)==9&&source::score(2)==5&&point.sum()==5&&count==3&&source::internal(2)==4&&enum_ok;std::fs::write(\"project-ran\",\"yes\").ok();std::process::exit(if ok{0}else{1});}\n";
        std::fs::write(&main, caller).unwrap();
        std::fs::write(&from, source).unwrap();
        std::fs::write(&to, "").unwrap();
        let mut app = test_app(home.path(), true);
        app.open_workspace_and_wait(&root);
        for path in [&main, &from, &to] {
            app.open_file_and_wait(path);
        }
        select(&mut app, &from, "fn score");
        open(&mut app);
        assert!(app.move_items.is_none());
        assert!(!root.join("project-ran").exists());
        app.trust_project_tools_for_test();
        app.insight.start_rust(&root, None, false);
        let mut h = harness(app, egui::vec2(1800.0, 1500.0));
        pump(&mut h, |app| app.runner.cargo_workspace.is_some() && app.insight.status == crate::insight::InsightStatus::Ready);
        // Cancelling a real identity job does not apply any buffered edits.
        window(h.state_mut(), &to);
        h.run_steps(2);
        h.get_by_label(tr("Preview item move")).click();
        h.run_steps(1);
        h.get_by_label(tr("Cancel")).click();
        h.run_steps(3);
        assert!(h.state().move_items.is_none());
        assert_eq!(h.state().docs.list.iter().find(|doc| doc.path == from).unwrap().editor.buffer.rope(), source);
        // A stale buffer invalidates the complete multi-buffer preview before any edit.
        window(h.state_mut(), &to);
        h.run_steps(2);
        h.get_by_label(tr("Preview item move")).click();
        h.run_steps(2);
        pump(&mut h, |app| app.move_items.as_ref().is_some_and(|state| state.preview.is_some() || state.error.is_some()));
        assert!(h.state().move_items.as_ref().unwrap().error.is_none(), "{:?}", h.state().move_items.as_ref().unwrap().error);
        let captured = h.state().move_items.as_ref().unwrap().preview.clone().unwrap();
        std::fs::write(&to, "// external disk edit\n").unwrap();
        let checked = std::thread::spawn(move || {
            core::verify(&captured, &Budget { cancel: Arc::new(AtomicBool::new(false)), deadline: Instant::now() + Duration::from_secs(5) })
        })
        .join()
        .unwrap();
        assert_eq!(checked, Err(Error::Changed));
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "// external disk edit\n");
        std::fs::write(&to, "").unwrap();
        h.state_mut().docs.list.iter_mut().find(|doc| doc.path == from).unwrap().editor.replace(0, 0, "// changed\n");
        h.run_steps(2);
        assert!(h.state().move_items.as_ref().unwrap().preview.is_none());
        assert!(h.state().move_items.as_ref().unwrap().error.is_some());
        assert!(h.state().docs.list.iter().find(|doc| doc.path == to).unwrap().editor.buffer.rope().len_chars() == 0);
        h.get_by_label(tr("Cancel")).click();
        h.run_steps(2);
        h.state_mut().docs.list.iter_mut().find(|doc| doc.path == from).unwrap().editor.undo();
        h.run_steps(2);
        for (needle, name, visibility) in [
            ("fn score", "score", "pub"),
            ("fn internal", "internal", "pub(crate)"),
            ("struct Point", "Point", "pub"),
            ("enum Mode", "Mode", "pub"),
            ("const OFFSET", "OFFSET", "pub"),
            ("type Count", "Count", "pub"),
        ] {
            select(h.state_mut(), &from, needle);
            window(h.state_mut(), &to);
            h.run_steps(2);
            h.get_by_label(tr("Preview item move")).click();
            h.run_steps(2);
            pump(&mut h, |app| app.move_items.as_ref().is_some_and(|state| state.preview.is_some() || state.error.is_some()));
            let state = h.state().move_items.as_ref().unwrap();
            assert!(state.error.is_none(), "{name}: {:?}", state.error);
            assert_eq!(state.preview.as_ref().unwrap().name, name);
            assert_eq!(std::fs::read_to_string(&from).unwrap(), source);
            assert!(std::fs::read_to_string(&to).unwrap().is_empty());
            h.run_steps(3);
            h.get_by_label(tr("Apply move to buffers")).click();
            h.run_steps(2);
            pump(&mut h, |app| app.move_items.is_none());
            assert!(
                h.state()
                    .docs
                    .list
                    .iter()
                    .find(|doc| doc.path == from)
                    .unwrap()
                    .editor
                    .buffer
                    .rope()
                    .to_string()
                    .contains(&format!("{visibility} use super::destination::{name};"))
            );
            assert_eq!(std::fs::read_to_string(&from).unwrap(), source);
        }
        // Each buffer has one undo record for the last accepted move; the earlier moves remain.
        for path in [&from, &to] {
            h.state_mut().docs.list.iter_mut().find(|doc| doc.path == *path).unwrap().editor.undo();
        }
        h.run_steps(2);
        let changed = h.state().docs.list.iter().find(|doc| doc.path == from).unwrap().editor.buffer.rope().to_string();
        assert!(changed.contains("pub type Count=i32;"));
        assert!(changed.contains("pub use super::destination::score;"));
        for path in [&from, &to] {
            h.state_mut().docs.list.iter_mut().find(|doc| doc.path == *path).unwrap().editor.redo();
        }
        h.run_steps(2);
        // An outer constant is a real RA identity outside the function and is refused.
        select(h.state_mut(), &from, "fn dependent");
        window(h.state_mut(), &to);
        h.run_steps(2);
        h.get_by_label(tr("Preview item move")).click();
        h.run_steps(2);
        pump(&mut h, |app| app.move_items.as_ref().is_some_and(|state| state.error.is_some()));
        assert!(h.state().move_items.as_ref().unwrap().preview.is_none());
        h.get_by_label(tr("Cancel")).click();
        h.run_steps(2);
        assert!(!root.join("project-ran").exists());
        assert_eq!(std::fs::read_to_string(&main).unwrap(), caller);
        assert!(h.state_mut().docs.save_all().is_empty());
        let result = std::process::Command::new("cargo").args(["run", "--offline", "--quiet"]).current_dir(&root).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        assert!(root.join("project-ran").exists());
        select(h.state_mut(), &main, "compute_score(4)");
        h.run_steps(3);
        h.state_mut().run_insight_action(rc_editor::Action::GotoDeclaration, egui::Pos2::ZERO);
        pump(&mut h, |app| app.docs.active_doc().is_some_and(|doc| doc.path == to));
    }
}
