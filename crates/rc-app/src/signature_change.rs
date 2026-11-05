//! Native Change Signature window. Workers prepare/verify; the UI only edits checked buffers.
use crate::{app::CrabCraftApp, signature_backend as backend, signature_model as model};
use rc_core::{Inbox, Waker};
use rc_kit::{
    i18n::{tr, trf},
    ico, theme,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[derive(Clone)]
struct Guard {
    capture: backend::Capture,
    features: Vec<String>,
    target: Option<String>,
    env: String,
}
impl Guard {
    fn capture(app: &mut CrabCraftApp) -> Result<(Self, backend::Key), backend::Error> {
        app.runner
            .require_trust()
            .map_err(|_| backend::Error::Unsupported)?;
        if app.cargo_features.len() > 128 || app.runner.project_env.len() > 64 * 1024 {
            return Err(backend::Error::Limit);
        }
        for index in 0..app.docs.list.len() {
            if app.docs.list[index].editor.lang() == rc_syntax::Lang::Rust
                && !app.docs.list[index].generated
                && !app.docs.list[index].scratch
            {
                app.insight.flush(&mut app.docs.list[index]);
            }
        }
        let workspace = app
            .runner
            .cargo_workspace
            .as_ref()
            .ok_or(backend::Error::Analysis)?;
        let session = app
            .insight
            .session
            .as_ref()
            .ok_or(backend::Error::Analysis)?;
        let doc = app
            .docs
            .active_doc()
            .filter(|doc| {
                doc.editor.lang() == rc_syntax::Lang::Rust
                    && !doc.editor.read_only
                    && !doc.generated
                    && !doc.scratch
                    && !doc.conflict
            })
            .ok_or(backend::Error::Unsupported)?;
        let position = crate::convert::lsp_position(doc.editor.buffer.rope(), doc.editor.caret());
        let origin = backend::Key {
            path: doc.path.clone(),
            line: position.line,
            column: position.character,
        };
        let mut overlays = Vec::new();
        let mut bytes = 0;
        for doc in app.docs.list.iter().filter(|doc| {
            doc.editor.lang() == rc_syntax::Lang::Rust
                && !doc.editor.read_only
                && !doc.generated
                && !doc.scratch
        }) {
            if doc.conflict {
                return Err(backend::Error::Stale);
            }
            bytes += doc.editor.buffer.rope().len_bytes();
            if overlays.len() >= 64
                || bytes > 16 * 1024 * 1024
                || doc.editor.buffer.rope().len_bytes() > 2 * 1024 * 1024
            {
                return Err(backend::Error::Limit);
            }
            overlays.push(backend::Overlay {
                ticket: crate::disk_revert::Ticket::capture(doc),
                rope: doc.editor.buffer.snapshot(),
                saved: doc.saved.clone(),
                bom: doc.bom,
                ending: doc.editor.buffer.line_ending(),
                modified: doc.mtime,
                server_version: session.docs.version(&doc.path),
            });
        }
        let members = workspace
            .packages
            .iter()
            .filter_map(|package| package.manifest.parent().map(std::path::Path::to_path_buf))
            .collect();
        Ok((
            Self {
                capture: backend::Capture {
                    root: workspace.root.clone(),
                    members,
                    overlays,
                    client: session.client.clone(),
                },
                features: app.cargo_features.clone(),
                target: app.cargo_target.clone(),
                env: app.runner.project_env.clone(),
            },
            origin,
        ))
    }
    fn current(&self, app: &CrabCraftApp) -> bool {
        app.runner.require_trust().is_ok()
            && app
                .insight
                .session
                .as_ref()
                .is_some_and(|session| session.client.same_connection(&self.capture.client))
            && app
                .runner
                .cargo_workspace
                .as_ref()
                .is_some_and(|workspace| {
                    workspace.root == self.capture.root
                        && workspace.packages.len() == self.capture.members.len()
                        && workspace.packages.iter().zip(&self.capture.members).all(
                            |(package, root)| package.manifest.parent() == Some(root.as_path()),
                        )
                })
            && self.features == app.cargo_features
            && self.target == app.cargo_target
            && self.env == app.runner.project_env
            && app
                .docs
                .list
                .iter()
                .filter(|doc| {
                    doc.editor.lang() == rc_syntax::Lang::Rust
                        && !doc.editor.read_only
                        && !doc.generated
                        && !doc.scratch
                })
                .count()
                == self.capture.overlays.len()
            && self.capture.overlays.iter().all(|overlay| {
                overlay.ticket.index(&app.docs).is_some_and(|index| {
                    let doc = &app.docs.list[index];
                    !doc.editor.read_only
                        && !doc.scratch
                        && !doc.conflict
                        && doc.editor.lang() == rc_syntax::Lang::Rust
                        && doc.bom == overlay.bom
                        && doc.editor.buffer.line_ending() == overlay.ending
                })
            })
    }
}
enum Reply {
    Loaded(Arc<backend::Prepared>),
    Preview(Arc<backend::Plan>),
    Verified(Arc<backend::Plan>),
}
struct Job {
    inbox: Inbox<Result<Reply, backend::Error>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
enum Request {
    Load(Guard, backend::Key),
    Preview(Arc<backend::Prepared>, model::Input),
    Verify(Arc<backend::Plan>),
}
#[derive(Clone)]
struct Row {
    id: u64,
    value: model::Parameter,
}
#[derive(Default)]
pub struct State {
    pub open: bool,
    guard: Option<Guard>,
    job: Option<Job>,
    prepared: Option<Arc<backend::Prepared>>,
    plan: Option<Arc<backend::Plan>>,
    verified: Option<Arc<backend::Plan>>,
    rows: Vec<Row>,
    return_type: String,
    next_row: u64,
    error: Option<backend::Error>,
    selected: usize,
    undo: Vec<crate::disk_revert::Ticket>,
}
impl State {
    fn start(&mut self, request: Request, waker: Waker) {
        self.job = None;
        self.error = None;
        self.plan = None;
        self.verified = None;
        let inbox = Inbox::new(waker);
        let output = inbox.sender();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let client = match &request {
            Request::Load(guard, _) => guard.capture.client.clone(),
            Request::Preview(prepared, _) => prepared.project.capture.client.clone(),
            Request::Verify(plan) => plan.prepared.project.capture.client.clone(),
        };
        let result = std::thread::Builder::new()
            .name("rc-change-signature".into())
            .spawn(move || {
                let context = backend::Context::new(client, token.clone());
                let result = match request {
                    Request::Load(guard, origin) => {
                        backend::prepare(guard.capture, origin, &context)
                            .map(|prepared| Reply::Loaded(Arc::new(prepared)))
                    }
                    Request::Preview(prepared, input) => {
                        backend::preview(prepared, input, &context)
                            .map(|plan| Reply::Preview(Arc::new(plan)))
                    }
                    Request::Verify(plan) => plan
                        .prepared
                        .project
                        .validate(&context)
                        .map(|()| Reply::Verified(plan)),
                };
                if !token.load(Ordering::Acquire) {
                    output.send(result);
                }
            });
        match result {
            Ok(_) => self.job = Some(Job { inbox, cancel }),
            Err(_) => self.error = Some(backend::Error::Io),
        }
    }
    fn load(&mut self, app: &mut CrabCraftApp) {
        self.job = None;
        self.prepared = None;
        self.plan = None;
        self.verified = None;
        self.guard = None;
        self.undo.clear();
        self.error = None;
        match Guard::capture(app) {
            Ok((guard, origin)) => {
                self.guard = Some(guard.clone());
                self.start(Request::Load(guard, origin), app.waker.clone());
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn poll(&mut self, app: &CrabCraftApp) {
        if self.guard.as_ref().is_some_and(|guard| !guard.current(app)) {
            self.job = None;
            self.prepared = None;
            self.plan = None;
            self.verified = None;
            self.guard = None;
            self.error = Some(backend::Error::Stale);
        }
        if let Some(result) = self
            .job
            .as_ref()
            .and_then(|job| job.inbox.drain().into_iter().next())
        {
            self.job = None;
            match result {
                Ok(Reply::Loaded(prepared)) => {
                    let input = prepared.function.input();
                    self.rows = input
                        .parameters
                        .into_iter()
                        .enumerate()
                        .map(|(index, value)| Row {
                            id: index as u64,
                            value,
                        })
                        .collect();
                    self.next_row = self.rows.len() as u64;
                    self.return_type = input.return_type;
                    self.prepared = Some(prepared);
                }
                Ok(Reply::Preview(plan)) if plan.input == self.input() => {
                    self.plan = Some(plan);
                    self.selected = 0;
                }
                Ok(Reply::Verified(plan)) if plan.input == self.input() => {
                    self.verified = Some(plan)
                }
                Ok(Reply::Preview(_) | Reply::Verified(_)) => {
                    self.error = Some(backend::Error::Stale);
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    fn input(&self) -> model::Input {
        model::Input {
            parameters: self
                .rows
                .iter()
                .map(|row| {
                    let mut value = row.value.clone();
                    value.name = value.name.trim().into();
                    value.ty = value.ty.trim().into();
                    value.argument = value.argument.trim().into();
                    value
                })
                .collect(),
            return_type: if self.return_type.trim().is_empty() {
                "()".into()
            } else {
                self.return_type.trim().into()
            },
        }
    }
    fn reset(&mut self) {
        if let Some(prepared) = &self.prepared {
            let input = prepared.function.input();
            self.rows = input
                .parameters
                .into_iter()
                .enumerate()
                .map(|(index, value)| Row {
                    id: index as u64,
                    value,
                })
                .collect();
            self.next_row = self.rows.len() as u64;
            self.return_type = input.return_type;
            self.plan = None;
            self.error = None;
        }
    }
    fn apply(
        &mut self,
        app: &mut CrabCraftApp,
        plan: Arc<backend::Plan>,
    ) -> Result<(), backend::Error> {
        if !self.guard.as_ref().is_some_and(|guard| guard.current(app)) {
            return Err(backend::Error::Stale);
        }
        let mut changes = Vec::new();
        for changed in &plan.files {
            let file = plan
                .prepared
                .project
                .files
                .get(&changed.path)
                .ok_or(backend::Error::Analysis)?;
            if let Some(open) = &file.open {
                let index = open.ticket.index(&app.docs).ok_or(backend::Error::Stale)?;
                if app.docs.list[index].editor.read_only {
                    return Err(backend::Error::Stale);
                }
            } else if app.docs.index_of(&changed.path).is_some()
                || app
                    .docs
                    .read_only_roots
                    .iter()
                    .any(|root| changed.path.starts_with(root))
            {
                return Err(backend::Error::Stale);
            }
            let mut edits = Vec::new();
            for edit in &changed.edits {
                if file.normal.get(edit.range.clone()).is_none() {
                    return Err(backend::Error::Analysis);
                }
                edits.push(rc_text::Change::new(
                    file.rope.byte_to_char(edit.range.start),
                    file.rope.byte_to_char(edit.range.end),
                    edit.text.clone(),
                ));
            }
            changes.push((file, edits));
        }
        let active = app.docs.active_doc().map(|doc| doc.editor.id);
        let mut tickets = Vec::new();
        for (file, edits) in changes {
            let index = match app.docs.index_of(&file.path) {
                Some(index) => index,
                None => app.docs.insert_compiler_source(
                    file.path.clone(),
                    &file.raw,
                    file.bom,
                    Some(file.modified),
                    Default::default(),
                ),
            };
            let doc = &mut app.docs.list[index];
            doc.editor.apply_text_edits(edits);
            tickets.push(crate::disk_revert::Ticket::capture(doc));
        }
        if let Some(active) =
            active.and_then(|id| app.docs.list.iter().position(|doc| doc.editor.id == id))
        {
            app.docs.activate(active);
        }
        self.undo = tickets;
        self.guard = None;
        self.prepared = None;
        self.plan = None;
        self.verified = None;
        app.focus_editor = true;
        Ok(())
    }
    fn undo(&mut self, app: &mut CrabCraftApp) -> Result<(), backend::Error> {
        let indices = self
            .undo
            .iter()
            .map(|ticket| {
                ticket
                    .index(&app.docs)
                    .filter(|index| !app.docs.list[*index].editor.read_only)
                    .ok_or(backend::Error::Stale)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if indices.is_empty() {
            return Err(backend::Error::NoChanges);
        }
        for index in indices {
            app.docs.list[index].editor.undo();
        }
        self.undo.clear();
        Ok(())
    }
    fn ui(&mut self, app: &mut CrabCraftApp, ctx: &egui::Context) {
        let mut open = true;
        let busy = self.job.is_some() || self.verified.is_some();
        let mut changed = false;
        let mut move_row = None;
        let mut remove = None;
        egui::Window::new(tr("Change Rust Function Signature")).id(egui::Id::new("change_rust_signature")).open(&mut open).default_width(880.0).default_height(600.0).show(ctx,|ui|{
   ui.label(tr("Change an ordinary free Rust function and its workspace callers. Function values, generated contexts and unsafe argument reordering are refused. Nothing is saved automatically."));
   ui.label(tr("External callers are not updated."));
   if let Some(prepared)=&self.prepared{ui.strong(&prepared.function.name);ui.label(prepared.key.path.strip_prefix(&prepared.project.capture.root).unwrap_or(&prepared.key.path).display().to_string());}
   ui.add_enabled_ui(!busy&&self.prepared.is_some(),|ui|{
    egui::ScrollArea::vertical().id_salt("signature_parameters").max_height(260.0).show(ui,|ui|{
     let count=self.rows.len();for (index,row) in self.rows.iter_mut().enumerate(){ui.push_id(row.id,|ui|{ui.horizontal_wrapped(|ui|{
      let name_label=trf("Parameter {n} name",&[("n",&(index+1))]);let label=ui.label(&name_label);changed|=ui.add(egui::TextEdit::singleline(&mut row.value.name).desired_width(110.0).char_limit(128)).labelled_by(label.id).changed();
      let type_label=trf("Parameter {n} type",&[("n",&(index+1))]);let label=ui.label(&type_label);changed|=ui.add(egui::TextEdit::singleline(&mut row.value.ty).desired_width(170.0).char_limit(2048)).labelled_by(label.id).changed();
      if row.value.original.is_none(){let argument_label=trf("New parameter {n} caller argument",&[("n",&(index+1))]);let label=ui.label(&argument_label);changed|=ui.add(egui::TextEdit::singleline(&mut row.value.argument).desired_width(100.0).char_limit(2048)).labelled_by(label.id).changed();}
      if theme::icon_button_sized(ui,ico::CHEVRON_UP,index>0,&trf("Move parameter {n} up",&[("n",&(index+1))]),24.0).clicked(){move_row=Some((index,index-1));}
      if theme::icon_button_sized(ui,ico::CHEVRON_DOWN,index+1<count,&trf("Move parameter {n} down",&[("n",&(index+1))]),24.0).clicked(){move_row=Some((index,index+1));}
      if ui.button(trf("Remove parameter {n}",&[("n",&(index+1))])).clicked(){remove=Some(index);}
     });});}
    });
    ui.horizontal(|ui|{if ui.add_enabled(self.rows.len()<32,egui::Button::new(tr("Add parameter"))).clicked(){self.rows.push(Row{id:self.next_row,value:model::Parameter{original:None,name:format!("parameter{}",self.next_row.saturating_add(1)),ty:"i32".into(),argument:"0".into()}});self.next_row=self.next_row.saturating_add(1);changed=true;}let label=ui.label(tr("Return type"));changed|=ui.add(egui::TextEdit::singleline(&mut self.return_type).desired_width(220.0).char_limit(2048)).labelled_by(label.id).changed();});
    ui.horizontal(|ui|{if ui.button(tr("Reset signature")).clicked(){self.reset();}if ui.button(tr("Preview signature changes")).clicked(){if let Some(prepared)=self.prepared.clone(){self.start(Request::Preview(prepared,self.input()),app.waker.clone());}}});
   });
   if let Some(error)=&self.error{ui.colored_label(rc_kit::pal(ui).error,error_text(error));}
   if busy{ui.horizontal(|ui|{ui.spinner();ui.label(tr("Resolving and checking signature changes…"));if ui.button(tr("Cancel signature operation")).clicked(){self.job=None;self.verified=None;}});}
   if let Some(plan)=self.plan.clone(){ui.separator();ui.label(trf("{files} files will be edited without saving",&[("files",&plan.files.len())]));
    let entries:Vec<_>=plan.files.iter().flat_map(|file|file.edits.iter().map(move|edit|(file,edit))).collect();
    egui::ScrollArea::vertical().id_salt("signature_edits").max_height(130.0).show_rows(ui,24.0,entries.len(),|ui,rows|{for index in rows{if let Some((file,edit))=entries.get(index){let Some(source)=plan.prepared.project.files.get(&file.path)else{continue;};let line=source.rope.byte_to_line(edit.range.start)+1;let label=format!("{}:{line}",file.path.strip_prefix(&plan.prepared.project.capture.root).unwrap_or(&file.path).display());if ui.selectable_label(index==self.selected,label).clicked(){self.selected=index;}}}});
    if let Some((file,edit))=entries.get(self.selected) && let Some(source)=plan.prepared.project.files.get(&file.path){ui.label(tr("Before"));let mut before=source.normal.get(edit.range.clone()).unwrap_or("");ui.add(egui::TextEdit::multiline(&mut before).code_editor().desired_rows(2).desired_width(f32::INFINITY));ui.label(tr("After"));let mut after=edit.text.as_str();ui.add(egui::TextEdit::multiline(&mut after).code_editor().desired_rows(2).desired_width(f32::INFINITY));}
    if ui.add_enabled(!busy,egui::Button::new(tr("Apply signature changes"))).clicked(){self.start(Request::Verify(plan),app.waker.clone());}
   }
   ui.horizontal(|ui|{if ui.add_enabled(!busy,egui::Button::new(tr("Reload function at caret"))).clicked(){self.load(app);}if !self.undo.is_empty()&&ui.button(tr("Undo signature change")).clicked(){if let Err(error)=self.undo(app){self.error=Some(error);}}});
  });
        if let Some(index) = remove {
            if index < self.rows.len() {
                self.rows.remove(index);
                changed = true;
            }
        }
        if let Some((a, b)) = move_row {
            if a < self.rows.len() && b < self.rows.len() {
                self.rows.swap(a, b);
                changed = true;
            }
        }
        if changed {
            self.plan = None;
            self.verified = None;
            self.error = None;
        }
        if !open {
            self.job = None;
            self.verified = None;
            self.open = false;
        }
        if self.open {
            if let Some(plan) = self.verified.take() {
                if let Err(error) = self.apply(app, plan) {
                    self.error = Some(error);
                }
            }
        }
    }
}
fn error_text(error: &backend::Error) -> &'static str {
    match error {
        backend::Error::Io => {
            tr("The workspace snapshot could not be read. Check the files and try again.")
        }
        backend::Error::Limit | backend::Error::Model(model::Error::Limit) => {
            tr("The signature change exceeds its source, reference, edit or time limits.")
        }
        backend::Error::Stale => {
            tr("Source or analysis changed. Reload the signature; your edits were kept.")
        }
        backend::Error::UsedParameter => tr("The removed parameter is used in the function body."),
        backend::Error::NameConflict | backend::Error::Model(model::Error::Conflict) => {
            tr("Parameter names must be distinct and must not capture existing body names.")
        }
        backend::Error::FunctionValue => tr(
            "This function is used as a value or in an unsupported call context. Its signature was not changed.",
        ),
        backend::Error::Analysis => tr(
            "rust-analyzer could not resolve every required definition or edit. Try again after indexing.",
        ),
        backend::Error::Cancelled => tr("Signature change cancelled."),
        backend::Error::NoChanges => tr("There are no signature changes to apply."),
        backend::Error::Model(model::Error::Input | model::Error::Syntax) => {
            tr("Enter valid Rust parameter names, types and literal caller arguments.")
        }
        backend::Error::Unsupported | backend::Error::Model(_) => tr(
            "Only ordinary free functions with analyzable Rust usages are supported. Methods, generated code, unknown contexts and unsafe argument changes are refused.",
        ),
    }
}
pub fn open(app: &mut CrabCraftApp) {
    let mut state = std::mem::take(&mut app.signature_change);
    state.open = true;
    if app.runner.require_trust().is_err() {
        state.error = Some(backend::Error::Unsupported);
        app.notify(
            tr("Trust this project before changing Rust signatures."),
            false,
        );
    } else {
        state.load(app);
    }
    app.signature_change = state;
}
pub fn show(app: &mut CrabCraftApp, ctx: &egui::Context) {
    let mut state = std::mem::take(&mut app.signature_change);
    if state.open {
        state.poll(app);
        state.ui(app, ctx);
    }
    app.signature_change = state;
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    #[test]
    fn native_preview_apply_and_undo_preserve_real_rust_files() {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        let lib = root.join("src/lib.rs");
        let main = root.join("src/main.rs");
        const LIB: &str = "pub fn answer(left:i32,right:i32)->i32{left+right}\n";
        const MAIN: &str = "use demo::answer as alias;\nfn main(){assert_eq!(alias(40,2),42);}\n";
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"demo\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[workspace]\n",
        )
        .unwrap();
        std::fs::write(root.join("Cargo.lock"),"# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(&lib, LIB).unwrap();
        let disk_main = format!("\u{feff}{}", MAIN.replace("\n", "\r\n"));
        std::fs::write(&main, &disk_main).unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        app.settings.trusted_projects.push(root.clone());
        app.open_workspace(&root);
        app.open_file(&lib);
        let mut h = crate::screens::harness(app, egui::vec2(1400., 1000.));
        let wait = |h: &mut egui_kittest::Harness<'static, CrabCraftApp>,
                    ready: &dyn Fn(&CrabCraftApp) -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while !ready(h.state()) {
                assert!(
                    h.state().signature_change.error.is_none(),
                    "signature error: {:?}",
                    h.state().signature_change.error
                );
                assert!(
                    std::time::Instant::now() < deadline,
                    "timed out: {:?}",
                    h.state().signature_change.error
                );
                h.run_steps(1);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        wait(&mut h, &|app| {
            app.insight.status == crate::insight::InsightStatus::Ready
                && app
                    .docs
                    .active_doc()
                    .is_some_and(|d| app.insight.runnables.contains_key(&d.editor.id))
        });
        h.state_mut()
            .docs
            .active_doc_mut()
            .unwrap()
            .editor
            .goto(0, 9);
        h.state_mut().run_ide_action(
            &egui::Context::default(),
            rc_editor::Action::ChangeSignature,
        );
        h.run_steps(2);
        wait(&mut h, &|a| a.signature_change.prepared.is_some());
        h.state_mut().signature_change.rows.swap(0, 1);
        h.state_mut().signature_change.rows[0].value.name = "rhs".into();
        h.state_mut().signature_change.rows[1].value.name = "lhs".into();
        for row in &mut h.state_mut().signature_change.rows {
            row.value.ty = "i64".into();
        }
        h.state_mut().signature_change.return_type = "i64".into();
        h.run_steps(2);
        h.get_by_label(tr("Preview signature changes")).click();
        h.run_steps(2);
        wait(&mut h, &|a| a.signature_change.plan.is_some());
        assert_eq!(
            h.state()
                .signature_change
                .plan
                .as_ref()
                .unwrap()
                .files
                .len(),
            2
        );
        assert_eq!(std::fs::read_to_string(&lib).unwrap(), LIB);
        assert_eq!(std::fs::read_to_string(&main).unwrap(), disk_main);
        h.get_by_label(tr("Apply signature changes")).click();
        h.run_steps(2);
        wait(&mut h, &|a| !a.signature_change.undo.is_empty());
        let app = h.state();
        let library = &app.docs.list[app.docs.index_of(&lib).unwrap()];
        let caller = &app.docs.list[app.docs.index_of(&main).unwrap()];
        assert!(
            library
                .editor
                .buffer
                .rope()
                .to_string()
                .contains("rhs:i64,lhs:i64")
        );
        assert!(
            caller
                .editor
                .buffer
                .rope()
                .to_string()
                .contains("alias(2,40)")
        );
        assert!(library.is_modified() && caller.is_modified());
        let proof = tempfile::tempdir().unwrap();
        let proof_lib = proof.path().join("lib.rs");
        let proof_main = proof.path().join("main.rs");
        std::fs::write(&proof_lib, library.editor.buffer.rope().to_string()).unwrap();
        std::fs::write(&proof_main, caller.editor.buffer.rope().to_string()).unwrap();
        let artifact = proof.path().join("libdemo.rlib");
        let result = std::process::Command::new("rustc")
            .args([
                "--crate-name",
                "demo",
                "--crate-type",
                "rlib",
                "--edition=2021",
            ])
            .arg(&proof_lib)
            .arg("-o")
            .arg(&artifact)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let binary = proof.path().join("proof");
        let result = std::process::Command::new("rustc")
            .args(["--edition=2021", "--extern"])
            .arg(format!("demo={}", artifact.display()))
            .arg(&proof_main)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            std::process::Command::new(&binary)
                .status()
                .unwrap()
                .success()
        );
        assert!(caller.bom);
        assert_eq!(
            caller.editor.buffer.line_ending(),
            rc_text::LineEnding::CrLf
        );
        assert_eq!(std::fs::read_to_string(&lib).unwrap(), LIB);
        assert_eq!(std::fs::read_to_string(&main).unwrap(), disk_main);
        h.run_steps(2);
        h.get_by_label(tr("Undo signature change")).click();
        h.run_steps(2);
        assert_eq!(
            h.state().docs.list[h.state().docs.index_of(&lib).unwrap()]
                .editor
                .buffer
                .rope()
                .to_string(),
            LIB
        );
        assert_eq!(
            h.state().docs.list[h.state().docs.index_of(&main).unwrap()]
                .editor
                .buffer
                .rope()
                .to_string(),
            MAIN
        );
        // A buffer edit after preview invalidates the entire proposal before applying.
        h.get_by_label(tr("Reload function at caret")).click();
        h.run_steps(2);
        wait(&mut h, &|a| a.signature_change.prepared.is_some());
        h.state_mut().signature_change.rows.swap(0, 1);
        h.run_steps(2);
        h.get_by_label(tr("Preview signature changes")).click();
        h.run_steps(2);
        wait(&mut h, &|a| a.signature_change.plan.is_some());
        let index = h.state().docs.index_of(&main).unwrap();
        h.state_mut().docs.list[index]
            .editor
            .replace(0, 0, "// user edit\n");
        h.run_steps(2);
        assert!(h.state().signature_change.plan.is_none());
        assert!(matches!(
            h.state().signature_change.error,
            Some(backend::Error::Stale)
        ));
        assert!(
            h.state().docs.list[index]
                .editor
                .buffer
                .rope()
                .to_string()
                .starts_with("// user edit")
        );
        assert_eq!(std::fs::read_to_string(&main).unwrap(), disk_main);
        h.state_mut().insight.stop();
    }
}
