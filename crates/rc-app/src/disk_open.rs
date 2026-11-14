//! Bounded worker preparation for interactive file navigation. Latest navigation wins.
use crate::docs::{Doc, MdView, TextReadError};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::tr;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone, Copy)]
pub enum Position {
    LineColumn(usize, usize),
    Offset(usize),
}
impl Position {
    pub fn apply(self, editor: &mut rc_editor::EditorState) {
        match self {
            Self::LineColumn(line, column) => editor.goto(line, column),
            Self::Offset(offset) => editor.set_selection(rc_text::Selection::point(offset.min(editor.buffer.len_chars()))),
        }
    }
}

pub struct Request {
    id: u64,
    path: PathBuf,
    style_root: Option<PathBuf>,
    read_only_roots: Vec<PathBuf>,
    scratch_root: PathBuf,
    pub position: Option<Position>,
    pub preview: bool,
    pub active: Option<u64>,
}
impl Request {
    pub(crate) fn for_restore(id: u64, path: PathBuf, style_root: PathBuf, scratch_root: PathBuf) -> Self {
        Self { id, path, style_root: Some(style_root), read_only_roots: Vec::new(), scratch_root, position: None, preview: false, active: None }
    }
    pub fn new(docs: &mut crate::docs::Docs, path: &Path, position: Option<Position>) -> Self {
        Self {
            id: docs.reserve_id(),
            path: path.to_path_buf(),
            style_root: docs.style_root.clone(),
            read_only_roots: docs.read_only_roots.clone(),
            scratch_root: rc_core::paths::scratches_dir(),
            position,
            preview: false,
            active: docs.active_doc().map(|doc| doc.editor.id),
        }
    }
}
pub struct Opened {
    pub doc: Doc,
    pub position: Option<Position>,
    pub preview: bool,
    pub active: Option<u64>,
    pub parse: bool,
}
pub(crate) fn prepare(request: &Request, cancel: &AtomicBool) -> Result<Opened, TextReadError> {
    if cancel.load(Ordering::Acquire) {
        return Err(TextReadError::Cancelled);
    }
    let path = request.path.canonicalize()?;
    let (text, bom, mtime) = crate::docs::read_text_cancellable(&path, Some(cancel))?;
    prepare_loaded(request, path, text, bom, mtime, cancel)
}
pub(crate) fn prepare_loaded(
    request: &Request,
    path: PathBuf,
    text: String,
    bom: bool,
    mtime: Option<std::time::SystemTime>,
    cancel: &AtomicBool,
) -> Result<Opened, TextReadError> {
    let large = text.len().saturating_add(if bom { 3 } else { 0 }) > 8 * 1024 * 1024;
    let scratch = rc_core::scratch::is_in(&request.scratch_root, &path);
    let style = match &request.style_root {
        Some(root) if !scratch && path.starts_with(root) => rc_core::code_style::for_file(&path, Some(root)),
        _ => Default::default(),
    };
    let light_project = if request.style_root.is_none() && !scratch { crate::docs::cargo_project_of(&path) } else { None };
    if cancel.load(Ordering::Acquire) {
        return Err(TextReadError::Cancelled);
    }
    let mut editor = rc_editor::EditorState::new(request.id, Some(path.clone()), &text);
    editor.read_only =
        large || path.components().any(|part| part.as_os_str() == "MetadataAsSource") || request.read_only_roots.iter().any(|root| path.starts_with(root));
    crate::docs::apply_style(&mut editor, &style);
    if !large && text.len() < 64 * 1024 {
        editor.parse_blocking();
    }
    let saved = editor.buffer.snapshot();
    let md_view =
        path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown")).then_some(MdView::Split);
    if cancel.load(Ordering::Acquire) {
        return Err(TextReadError::Cancelled);
    }
    Ok(Opened {
        doc: Doc {
            editor,
            path,
            mtime,
            disk_epoch: 0,
            pending_save: Default::default(),
            bom,
            pinned: false,
            preview: false,
            conflict: false,
            md_view,
            saved,
            scratch,
            generated: false,
            style,
            light_project,
        },
        position: request.position,
        preview: request.preview,
        active: request.active,
        parse: !large,
    })
}
struct Job {
    generation: u64,
    cancel: Arc<AtomicBool>,
    inbox: Inbox<Result<Opened, TextReadError>>,
}
pub struct State {
    generation: u64,
    jobs: Vec<Job>,
    pending: std::collections::VecDeque<Request>,
    path: PathBuf,
    remaining: usize,
}
impl Default for State {
    fn default() -> Self {
        Self { generation: 0, jobs: Vec::new(), pending: Default::default(), path: PathBuf::new(), remaining: 512 * 1024 * 1024 }
    }
}
impl State {
    pub fn request(&mut self, request: Request, waker: Waker) -> Result<(), TextReadError> {
        self.cancel();
        self.path = request.path.clone();
        self.pending.push_back(request);
        self.launch(waker)
    }
    pub fn request_many(&mut self, requests: Vec<Request>, waker: Waker) -> Result<(), TextReadError> {
        self.cancel();
        if requests.len() > 256 {
            return Err(TextReadError::BatchLimit);
        }
        self.pending.extend(requests);
        self.launch(waker)
    }
    pub fn set_active(&mut self, active: Option<u64>) {
        for request in &mut self.pending {
            request.active = active;
        }
    }
    fn launch(&mut self, waker: Waker) -> Result<(), TextReadError> {
        if self.jobs.len() >= 2 || self.jobs.iter().any(|job| job.generation == self.generation) {
            return Ok(());
        }
        let Some(request) = self.pending.pop_front() else {
            return Ok(());
        };
        self.path = request.path.clone();
        let inbox = Inbox::new(waker);
        let tx = inbox.sender();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::Builder::new().name("rc-open-source".into()).spawn(move || tx.send(prepare(&request, &flag)))?;
        self.jobs.push(Job { generation: self.generation, cancel, inbox });
        Ok(())
    }
    pub fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending.clear();
        self.remaining = 512 * 1024 * 1024;
        for job in &self.jobs {
            job.cancel.store(true, Ordering::Release);
        }
    }
    pub fn busy(&self) -> bool {
        !self.pending.is_empty() || self.jobs.iter().any(|job| job.generation == self.generation)
    }
    pub fn poll(&mut self, waker: Waker) -> Vec<Result<Opened, TextReadError>> {
        let mut results = Vec::new();
        if let Err(error) = self.launch(waker) {
            results.push(Err(error));
        }
        let mut i = 0;
        while i < self.jobs.len() {
            let Some(result) = self.jobs[i].inbox.drain().into_iter().next() else {
                i += 1;
                continue;
            };
            let job = self.jobs.remove(i);
            if job.generation == self.generation {
                let bytes = result.as_ref().ok().map_or(0, |opened| opened.doc.editor.buffer.rope().len_bytes());
                if bytes > self.remaining {
                    self.cancel();
                    results.push(Err(TextReadError::BatchLimit));
                } else {
                    self.remaining -= bytes;
                    results.push(result);
                }
            }
        }
        results
    }
    pub fn show(&mut self, ctx: &egui::Context) {
        if self.busy() {
            egui::Window::new(tr("Opening file")).id(egui::Id::unique("disk-open-progress")).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(self.path.file_name().unwrap_or_default().to_string_lossy());
                });
                if ui.button(tr("Cancel")).clicked() {
                    self.cancel();
                }
            });
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn wait(h: &mut egui_kittest::Harness<'_, crate::app::CrabCraftApp>) {
        let until = Instant::now() + Duration::from_secs(10);
        while h.state().disk_open.busy() {
            h.run_steps(1);
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn native_worker_open_preserves_encoding_edits_and_latest_navigation_in_a_real_cargo_project() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("src");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"worker_open_proof\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        let main = source.join("main.rs");
        let helper = source.join("helper.rs");
        std::fs::write(&main, "mod helper;\nfn main(){println!(\"{}\",helper::value());}\n").unwrap();
        std::fs::write(&helper, "\u{feff}pub fn value() -> i32 {\r\n    // café\r\n    42\r\n}\r\n").unwrap();
        std::fs::write(root.path().join("build.rs"), "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}").unwrap();
        std::fs::write(root.path().join(".editorconfig"), "root = true\n[*.rs]\nindent_style = space\nindent_size = 2\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(root.path());
        app.open_file_and_wait(&main);
        app.docs.active_doc_mut().unwrap().editor.replace(0, 0, "// unsaved\n");
        let before = app.docs.active_doc().unwrap().editor.buffer.rope().to_string();
        app.request_open_at(&helper, 2, 4);
        assert_eq!(app.docs.active_doc().unwrap().path, main);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 850.0));
        wait(&mut h);
        let doc = h.state().docs.active_doc().unwrap();
        assert_eq!(doc.path, helper);
        assert!(doc.bom);
        assert_eq!(doc.editor.buffer.line_ending(), rc_text::LineEnding::CrLf);
        assert_eq!(doc.editor.caret(), doc.editor.buffer.rope().line_to_char(2) + 4);
        assert_eq!(doc.style.indent_size, Some(2));
        assert_eq!(h.state().docs.list[h.state().docs.index_of(&main).unwrap()].editor.buffer.rope().to_string(), before);
        assert!(!root.path().join("build-ran").exists());
        h.state_mut().request_open_file(&main);
        h.state_mut().request_open_file(&helper);
        h.state_mut().request_open_file(&main);
        wait(&mut h);
        assert_eq!(h.state().docs.active_doc().unwrap().path, main);
        assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string(), before);
        let other = source.join("cancelled.rs");
        std::fs::write(&other, "pub const CANCELLED: i32 = 0;\n").unwrap();
        h.state_mut().request_open_file(&other);
        let helper_index = h.state().docs.index_of(&helper).unwrap();
        h.state_mut().docs.activate(helper_index);
        wait(&mut h);
        assert!(h.state().docs.index_of(&other).is_none());
        h.state_mut().request_open_file(&other);
        h.state_mut().disk_open.cancel();
        for _ in 0..10 {
            h.run_steps(1);
        }
        assert!(h.state().docs.index_of(&other).is_none());
        #[cfg(unix)]
        {
            let alias = source.join("alias.rs");
            std::os::unix::fs::symlink(&main, &alias).unwrap();
            h.state_mut().request_open_file(&alias);
            wait(&mut h);
            assert_eq!(h.state().docs.list.len(), 2);
            assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string(), before);
        }
        h.state_mut().request_open_file(&main);
        let index = h.state().docs.index_of(&helper).unwrap();
        h.state_mut().close_tab(index);
        h.state_mut().save_all_and_wait();
        let offset = 26usize;
        h.state_mut().nav_back = vec![(helper.clone(), offset)];
        h.state_mut().nav_forward.clear();
        h.state_mut().navigate(true);
        wait(&mut h);
        assert_eq!(h.state().docs.active_doc().unwrap().path, helper);
        assert_eq!(h.state().docs.active_doc().unwrap().editor.caret(), offset);
        assert_eq!(h.state().nav_forward.len(), 1);
        h.state_mut().navigate(false);
        wait(&mut h);
        assert_eq!(h.state().docs.active_doc().unwrap().path, main);
        assert!(h.state().nav_forward.is_empty());
        assert_eq!(h.state().nav_back.len(), 1);
        let preview_path = source.join("preview.rs");
        std::fs::write(&preview_path, "pub fn preview() {}\n").unwrap();
        let ctx = h.ctx.clone();
        h.state_mut().handle_explorer(&ctx, vec![crate::ui::explorer::ExplorerAction::Preview(preview_path.clone())]);
        wait(&mut h);
        assert!(h.state().docs.active_doc().unwrap().preview);
        let first_id = h.state().docs.active_doc().unwrap().editor.id;
        let next_path = source.join("next_preview.rs");
        std::fs::write(&next_path, "pub fn next() {}\n").unwrap();
        h.state_mut().handle_explorer(&ctx, vec![crate::ui::explorer::ExplorerAction::Preview(next_path.clone())]);
        wait(&mut h);
        assert!(
            h.state().docs.index_of(&preview_path).is_none(),
            "{:?}",
            h.state().docs.list.iter().map(|doc| (&doc.path, doc.preview, doc.is_modified())).collect::<Vec<_>>()
        );
        assert!(h.state().docs.active_doc().unwrap().preview);
        assert_ne!(h.state().docs.active_doc().unwrap().editor.id, first_id);
        h.state_mut().docs.active_doc_mut().unwrap().editor.replace(0, 0, "// unsaved preview\n");
        h.state_mut().handle_explorer(&ctx, vec![crate::ui::explorer::ExplorerAction::Preview(preview_path.clone())]);
        wait(&mut h);
        assert!(h.state().docs.list[h.state().docs.index_of(&next_path).unwrap()].is_modified());
        let output = std::process::Command::new("cargo").args(["run", "--offline", "--quiet"]).current_dir(root.path()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42");
    }
    #[test]
    fn native_batch_opens_all_files_in_order_and_preserves_unsaved_aliases() {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"batch_demo\"\nversion=\"0.1.0\"\nedition=\"2021\"\n").unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("build.rs"), "fn main(){ std::fs::write(\"BUILD_RAN\",\"\").unwrap(); }").unwrap();
        let paths: Vec<_> = (0..4)
            .map(|i| {
                let path = root.path().join(format!("src/file{i}.rs"));
                std::fs::write(&path, format!("pub fn value{i}()->u8{{{i}}}\n")).unwrap();
                path
            })
            .collect();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_file_and_wait(&paths[0]);
        app.docs.active_doc_mut().unwrap().editor.replace(0, 0, "// unsaved\n");
        app.open_requests(paths.iter().map(|path| crate::cli::OpenRequest { path: path.clone(), line: Some(1), column: Some(8) }).collect());
        let mut h = crate::screens::harness(app, egui::vec2(1200.0, 800.0));
        wait(&mut h);
        assert_eq!(h.state().docs.list.len(), 4);
        assert!(h.state().docs.list[0].editor.buffer.rope().to_string().starts_with("// unsaved"));
        assert_eq!(h.state().docs.active_doc().unwrap().path, paths[3]);
        assert_eq!(h.state().docs.active_doc().unwrap().editor.caret(), 7);
        assert!(!root.path().join("BUILD_RAN").exists());
        h.state_mut().request_open_files(vec![paths[0].clone(), paths[1].clone()]);
        h.state_mut().disk_open.cancel();
        h.run_steps(3);
        assert_eq!(h.state().docs.active_doc().unwrap().path, paths[3]);
        h.state_mut().request_open_files(vec![paths[0].clone(); 257]);
        assert!(!h.state().disk_open.busy());
        assert_eq!(h.state().docs.list.len(), 4);
        let loose = home.path().join("loose.rs");
        std::fs::write(&loose, "pub fn outside() {}\n").unwrap();
        h.state_mut().open_requests(vec![
            crate::cli::OpenRequest::path(root.path()),
            crate::cli::OpenRequest { path: paths[1].clone(), line: Some(1), column: Some(5) },
            crate::cli::OpenRequest { path: loose.clone(), line: Some(1), column: Some(10) },
        ]);
        let until = Instant::now() + Duration::from_secs(10);
        while h.state().workspace_open.busy() || h.state().disk_open.busy() || h.state().disk_save.continuation.is_some() {
            h.run_steps(1);
            assert!(Instant::now() < until, "CLI workspace/file batch did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(h.state().workspace.as_ref().unwrap().ws.root, root.path());
        assert_eq!(h.state().docs.active_doc().unwrap().path, loose);
        assert_eq!(h.state().docs.active_doc().unwrap().editor.caret(), 9);
        assert_eq!(h.state().docs.list[h.state().docs.index_of(&paths[1]).unwrap()].editor.caret(), 4);
        assert!(!root.path().join("BUILD_RAN").exists());
    }
    #[test]
    fn worker_reader_cancels_and_rejects_special_files_without_starting_project_tools() {
        let mut docs = crate::docs::Docs::new(rc_syntax::ParserPool::new(Waker::noop().as_fn()));
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("lib.rs");
        std::fs::write(&path, "pub fn value(){}\n").unwrap();
        let request = Request::new(&mut docs, &path, None);
        assert!(matches!(prepare(&request, &AtomicBool::new(true)), Err(TextReadError::Cancelled)));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let fifo = root.path().join("fifo.rs");
            let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            let request = Request::new(&mut docs, &fifo, None);
            let start = Instant::now();
            assert!(matches!(prepare(&request, &AtomicBool::new(false)), Err(TextReadError::NotRegular)));
            assert!(start.elapsed() < Duration::from_secs(1));
        }
        let large = root.path().join("large.rs");
        std::fs::write(&large, format!("//{}\n", "x".repeat(9 * 1024 * 1024))).unwrap();
        let request = Request::new(&mut docs, &large, None);
        let opened = prepare(&request, &AtomicBool::new(false)).unwrap_or_else(|error| panic!("{error}"));
        assert!(opened.doc.editor.read_only);
        assert!(!opened.parse);
        let mut request = Request::new(&mut docs, &path, None);
        request.read_only_roots.push(root.path().to_path_buf());
        let opened = prepare(&request, &AtomicBool::new(false)).unwrap_or_else(|error| panic!("{error}"));
        assert!(opened.doc.editor.read_only);
        assert!(opened.parse);
        let huge = root.path().join("huge.rs");
        let file = std::fs::File::create(&huge).unwrap();
        file.set_len(129 * 1024 * 1024).unwrap();
        let request = Request::new(&mut docs, &huge, None);
        assert!(matches!(prepare(&request, &AtomicBool::new(false)), Err(TextReadError::TooLarge(_))));
    }
}
