//! Bounded data-only workspace/session preparation. Old buffers remain installed until commit.
use crate::{docs::Doc, runner::Runner};
use rc_core::{
    Inbox, Waker, Workspace, WorkspaceKind,
    session::Session,
    watch::{FileWatcher, FsEvent},
};
use rc_kit::i18n::{tr, trf};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Invalid,
    Limit,
    Cancelled,
    Stale,
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Invalid => f.write_str(tr(
                "Workspace and session inputs must be regular bounded files",
            )),
            Self::Limit => f.write_str(tr(
                "Workspace restoration supports 256 files and 512 MB of text",
            )),
            Self::Cancelled => f.write_str(tr(
                "Workspace opening cancelled; existing buffers were kept",
            )),
            Self::Stale => f.write_str(tr(
                "Documents changed while opening the workspace; existing buffers were kept",
            )),
        }
    }
}
impl std::error::Error for Error {}
fn check(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
fn read(path: &Path, cancel: &AtomicBool) -> Result<String, Error> {
    check(cancel)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(Error::Invalid);
    }
    if before.len() > 4 * 1024 * 1024 {
        return Err(Error::Limit);
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check(cancel)?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > 4 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(Error::Stale);
    }
    let current = std::fs::symlink_metadata(path)?;
    if !current.is_file()
        || current.len() != after.len()
        || current.modified().ok() != after.modified().ok()
    {
        return Err(Error::Stale);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if current.ino() != after.ino() || current.dev() != after.dev() {
            return Err(Error::Stale);
        }
    }
    check(cancel)?;
    String::from_utf8(bytes)
        .map(|s| s.trim_start_matches('\u{feff}').to_string())
        .map_err(|_| Error::Invalid)
}
fn workspace(path: &Path, cancel: &AtomicBool) -> Result<Workspace, Error> {
    check(cancel)?;
    let path = path.canonicalize()?;
    let meta = std::fs::metadata(&path)?;
    if meta.is_dir() {
        let manifest = path.join("Cargo.toml");
        match std::fs::metadata(&manifest) {
            Ok(metadata) if metadata.is_file() => {
                return Ok(Workspace {
                    root: path,
                    kind: WorkspaceKind::Cargo(manifest),
                });
            }
            Ok(_) => return Err(Error::Invalid),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        let kind = WorkspaceKind::Folder;
        Ok(Workspace { root: path, kind })
    } else {
        if !meta.is_file() {
            return Err(Error::Invalid);
        }
        let root = path.parent().ok_or(Error::Invalid)?.to_path_buf();
        let kind = if path.file_name().is_some_and(|p| p == "Cargo.toml") {
            WorkspaceKind::Cargo(path)
        } else {
            return Err(Error::Invalid);
        };
        Ok(Workspace { root, kind })
    }
}
fn options(
    root: &Path,
    cancel: &AtomicBool,
) -> Result<std::collections::BTreeMap<String, rc_core::session::RunOptions>, Error> {
    #[derive(serde::Deserialize)]
    struct Config {
        name: String,
        options: rc_core::session::RunOptions,
    }
    let mut options = std::collections::BTreeMap::new();
    let entries = match std::fs::read_dir(rc_core::run_files::dir(root)) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(options),
        Err(e) => return Err(e.into()),
    };
    for (index, entry) in entries.enumerate() {
        check(cancel)?;
        if index >= 1024 {
            return Err(Error::Limit);
        }
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(text) = read(&path, cancel) else {
            check(cancel)?;
            continue;
        };
        let Ok(mut config) = serde_json::from_str::<Config>(&text) else {
            continue;
        };
        if config.name.len() > 1024 {
            continue;
        }
        config.options.in_project = true;
        options.insert(config.name, config.options);
    }
    Ok(options)
}
#[derive(Clone, PartialEq)]
pub struct Fingerprint(Vec<(u64, PathBuf, u64, u64, bool, bool)>);
impl Fingerprint {
    pub fn capture(docs: &crate::docs::Docs) -> Self {
        Self(
            docs.list
                .iter()
                .map(|d| {
                    (
                        d.editor.id,
                        d.path.clone(),
                        d.editor.buffer.version(),
                        d.disk_epoch,
                        d.is_modified(),
                        d.conflict,
                    )
                })
                .collect(),
        )
    }
}
pub struct Request {
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
    pub ids: Vec<u64>,
    pub scratch: PathBuf,
}
pub struct Prepared {
    pub ws: Workspace,
    pub session: Session,
    pub docs: Vec<(usize, Doc, bool)>,
    pub attached: Vec<PathBuf>,
    pub linked: Vec<PathBuf>,
    pub runner: Runner,
    pub watcher: Option<FileWatcher>,
    pub events: Inbox<FsEvent>,
    pub warnings: Vec<String>,
    pub fingerprint: Fingerprint,
}
fn prepare(request: Request, cancel: &AtomicBool, waker: Waker) -> Result<Prepared, Error> {
    let ws = workspace(&request.path, cancel)?;
    let mut warnings = Vec::new();
    let mut session = match read(&Session::file_for(ws.key_path()), cancel) {
        Ok(text) => serde_json::from_str::<Session>(&text).map_err(|_| Error::Invalid)?,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Session::default(),
        Err(error) => return Err(error),
    };
    check(cancel)?;
    if session.files.len() > 256 {
        return Err(Error::Limit);
    }
    session.expanded.truncate(4096);
    session.editor_windows.truncate(64);
    for window in &mut session.editor_windows {
        if let Some(rect) = &mut window.rect {
            if rect.iter().any(|value| !value.is_finite()) {
                window.rect = None;
            } else {
                rect[0] = rect[0].clamp(-1e6, 1e6);
                rect[1] = rect[1].clamp(-1e6, 1e6);
                rect[2] = rect[2].clamp(64.0, 16384.0);
                rect[3] = rect[3].clamp(64.0, 16384.0);
            }
        }
    }
    session.attached_projects.truncate(256);
    if session.breakpoints.len() > 1024
        || session.bookmarks.len() > 1024
        || session.run_options.len() > 1024
        || session.breakpoints.values().map(Vec::len).sum::<usize>() > 16384
    {
        return Err(Error::Limit);
    }
    let auxiliary_entries = session.bookmarks.values().map(Vec::len).sum::<usize>()
        + session.logpoints.values().map(Vec::len).sum::<usize>()
        + session
            .disabled_breakpoints
            .values()
            .map(Vec::len)
            .sum::<usize>()
        + session
            .remove_once_hit
            .values()
            .map(Vec::len)
            .sum::<usize>()
        + session
            .breakpoint_groups
            .values()
            .map(Vec::len)
            .sum::<usize>();
    if auxiliary_entries > 16384
        || session.logpoints.len() > 1024
        || session.disabled_breakpoints.len() > 1024
        || session.remove_once_hit.len() > 1024
        || session.breakpoint_groups.len() > 1024
    {
        return Err(Error::Limit);
    }
    let mut attached = Vec::new();
    for relative in &session.attached_projects {
        check(cancel)?;
        let path = ws.root.join(relative);
        if std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
            attached.push(path);
        }
    }
    let mut linked = Vec::new();
    if !attached.is_empty() && ws.cargo_manifest().is_some() {
        linked.push(ws.root.join("Cargo.toml"));
    }
    linked.extend(attached.iter().cloned());
    let mut runner = Runner::new(waker.clone());
    runner.project_root = Some(ws.root.clone());
    runner.project_env = session.project_env.clone();
    runner.compounds = session.compounds.clone();
    runner.test_configs = session.test_configs.clone();
    runner.folders = session
        .config_folders
        .iter()
        .filter(|(_, f)| {
            !f.trim().is_empty() && f.chars().count() <= crate::runner::MAX_FOLDER_CHARS
        })
        .map(|(c, f)| (c.clone(), f.trim().to_string()))
        .collect();
    runner.options = session.run_options.clone();
    runner.options.extend(options(&ws.root, cancel)?);
    check(cancel)?;
    let mut nodes: Vec<&rc_core::session::EditorNode> = session
        .editor_layout
        .iter()
        .chain(session.editor_windows.iter().map(|window| &window.root))
        .collect();
    let mut count = 0usize;
    while let Some(node) = nodes.pop() {
        count += 1;
        if count > 512 {
            return Err(Error::Limit);
        }
        match node {
            rc_core::session::EditorNode::Group { tabs, .. } if tabs.len() > 256 => {
                return Err(Error::Limit);
            }
            rc_core::session::EditorNode::Split { a, b, ratio, .. } => {
                if !ratio.is_finite() {
                    return Err(Error::Invalid);
                }
                nodes.push(a);
                nodes.push(b);
            }
            _ => (),
        }
    }
    let mut docs = Vec::new();
    let mut bytes = 0usize;
    for (index, file) in session.files.iter().enumerate() {
        check(cancel)?;
        let Some(id) = request.ids.get(index).copied() else {
            return Err(Error::Limit);
        };
        let input = crate::disk_open::Request::for_restore(
            id,
            file.path.clone(),
            ws.root.clone(),
            request.scratch.clone(),
        );
        match crate::disk_open::prepare(&input, cancel) {
            Ok(mut opened) => {
                bytes = bytes.saturating_add(opened.doc.editor.buffer.rope().len_bytes());
                if bytes > 512 * 1024 * 1024 {
                    return Err(Error::Limit);
                }
                opened.doc.pinned = file.pinned;
                opened.doc.editor.goto(file.line, file.col);
                if !file.folded.is_empty()
                    && !opened.doc.editor.read_only
                    && opened.doc.editor.syntax.tree().is_none()
                {
                    opened.doc.editor.parse_blocking();
                }
                for line in file
                    .folded
                    .iter()
                    .take(4096)
                    .filter(|line| **line < opened.doc.editor.buffer.len_lines())
                    .copied()
                    .collect::<Vec<_>>()
                {
                    opened.doc.editor.toggle_fold(line);
                }
                opened.doc.editor.set_scroll_offset(egui::vec2(
                    0.0,
                    if file.scroll_y.is_finite() {
                        file.scroll_y.clamp(0.0, 1e8)
                    } else {
                        0.0
                    },
                ));
                docs.push((index, opened.doc, opened.parse));
            }
            Err(crate::docs::TextReadError::Cancelled) => return Err(Error::Cancelled),
            Err(error) => {
                if warnings.len() < 64 {
                    warnings.push(trf(
                        "Could not open {path}: {error}",
                        &[("path", &file.path.display()), ("error", &error)],
                    ));
                }
            }
        }
    }
    let scratches: Vec<_> = docs
        .iter()
        .filter(|(_, doc, _)| doc.scratch && doc.editor.lang() == rc_syntax::Lang::Rust)
        .map(|(_, doc, _)| doc.path.clone())
        .collect();
    if !scratches.is_empty() && linked.is_empty() && ws.cargo_manifest().is_some() {
        linked.push(ws.root.join("Cargo.toml"));
    }
    linked.extend(scratches);
    let events = Inbox::new(waker);
    let watcher = FileWatcher::new(&ws.root, events.sender()).ok();
    check(cancel)?;
    Ok(Prepared {
        ws,
        session,
        docs,
        attached,
        linked,
        runner,
        watcher,
        events,
        warnings,
        fingerprint: request.fingerprint,
    })
}
struct Job {
    cancel: Arc<AtomicBool>,
    inbox: Inbox<Result<Prepared, Error>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub struct State {
    active: Option<Job>,
    pub ready: Option<Prepared>,
    pub reload_session: bool,
    pending: Option<Request>,
    pub target: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    pub positions: std::collections::BTreeMap<PathBuf, crate::disk_open::Position>,
    pub previews: std::collections::BTreeSet<PathBuf>,
    #[cfg(test)]
    pub gate: Option<Arc<AtomicBool>>,
}
impl State {
    pub fn busy(&self) -> bool {
        self.active.is_some() || self.pending.is_some() || self.ready.is_some()
    }
    pub fn cancel(&mut self) {
        if let Some(job) = &self.active {
            job.cancel.store(true, Ordering::Release);
        }
        self.pending = None;
        self.ready = None;
        self.reload_session = false;
        self.target = None;
        self.files.clear();
        self.positions.clear();
        self.previews.clear();
    }
    pub fn target(&mut self, path: &Path) {
        if self.target.as_deref() != Some(path) {
            self.cancel();
            self.target = Some(path.to_path_buf());
        }
    }
    pub fn request(&mut self, request: Request) {
        self.ready = None;
        self.target(&request.path);
        if let Some(job) = &self.active {
            job.cancel.store(true, Ordering::Release);
        }
        self.pending = Some(request);
    }
    pub fn queue_files(&mut self, paths: Vec<PathBuf>) -> Result<(), Error> {
        if self.files.len().saturating_add(paths.len()) > 256 {
            return Err(Error::Limit);
        }
        self.files.extend(paths);
        Ok(())
    }
    pub fn poll(&mut self, waker: Waker) -> Option<Result<Prepared, Error>> {
        let mut result = None;
        if let Some(done) = self
            .active
            .as_ref()
            .and_then(|j| j.inbox.drain().into_iter().next())
        {
            if let Some(job) = self.active.take() {
                if !job.cancel.load(Ordering::Acquire) {
                    result = Some(done);
                }
            }
        }
        if self.active.is_none()
            && let Some(request) = self.pending.take()
        {
            let inbox = Inbox::new(waker.clone());
            let out = inbox.sender();
            let cancel = Arc::new(AtomicBool::new(false));
            let token = cancel.clone();
            #[cfg(test)]
            let gate = self.gate.clone();
            let spawned = std::thread::Builder::new()
                .name("crabcraft-workspace-open".into())
                .spawn(move || {
                    #[cfg(test)]
                    if let Some(gate) = gate {
                        while !gate.load(Ordering::Acquire) && !token.load(Ordering::Acquire) {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                    let result = prepare(request, &token, waker);
                    if token.load(Ordering::Acquire) {
                        drop(result);
                        out.send(Err(Error::Cancelled));
                    } else {
                        out.send(result);
                    }
                });
            if let Err(error) = spawned {
                return Some(Err(Error::Io(error)));
            }
            self.active = Some(Job { cancel, inbox });
        }
        result
    }
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        if !self.busy() {
            return false;
        }
        let mut cancelled = false;
        egui::Window::new(tr("Opening workspace…"))
            .id(egui::Id::unique("workspace_open_progress"))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(tr(
                    "Existing buffers stay open until workspace preparation finishes",
                ));
                if ui.button(tr("Cancel")).clicked() {
                    self.cancel();
                    cancelled = true;
                }
            });
        cancelled
    }
}

#[cfg(test)]
impl crate::app::CrabCraftApp {
    pub(crate) fn save_session_and_wait(&mut self) {
        self.wait_workspace_open();
        self.save_session();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while self.disk_save.auxiliary.busy() || self.disk_save.saves.busy() {
            self.poll_disk_save();
            assert!(std::time::Instant::now() < end, "session saving timed out");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!self.disk_save.failed, "session saving failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn project(root: &Path) -> PathBuf {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"),"[package]\nname=\"restored_workspace\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(
            root.join("build.rs"),
            "fn main(){std::fs::write(\"project-code-ran\",\"yes\").unwrap();}\n",
        )
        .unwrap();
        let path = root.join("src/main.rs");
        std::fs::write(&path,b"\xef\xbb\xbffn main() {\r\n    let value = 42;\r\n    println!(\"{}\", value);\r\n}\r\n").unwrap();
        path
    }
    fn pump(app: &mut crate::app::CrabCraftApp, done: impl Fn(&crate::app::CrabCraftApp) -> bool) {
        let end = Instant::now() + Duration::from_secs(15);
        while !done(app) {
            app.poll_cargo_workspace();
            app.poll_rustfmt();
            app.poll_disk_save();
            app.poll_workspace_open();
            app.poll_disk_open();
            let outputs = app.runner.update();
            crate::ui::run_windows::handle_outputs(app, outputs);
            assert!(Instant::now() < end, "workspace worker timed out");
            std::thread::sleep(Duration::from_millis(3));
        }
    }
    #[test]
    fn native_session_restores_real_rust_encoding_layout_positions_and_breakpoints_without_project_tools()
     {
        use rc_core::session::{EditorNode, OpenFile};
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let source = project(root.path());
        let asset = root.path().join("notes.txt");
        std::fs::write(&asset, "notes\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        let session = Session {
            files: vec![
                OpenFile {
                    path: root.path().join("missing.rs"),
                    ..Default::default()
                },
                OpenFile {
                    path: source.clone(),
                    line: 0,
                    col: 3,
                    scroll_y: 24.0,
                    pinned: true,
                    folded: vec![0],
                    ..Default::default()
                },
                OpenFile {
                    path: asset.clone(),
                    ..Default::default()
                },
            ],
            active: Some(1),
            editor_layout: Some(EditorNode::Split {
                vertical: true,
                ratio: 0.5,
                a: Box::new(EditorNode::Group {
                    tabs: vec![1],
                    active: Some(1),
                }),
                b: Box::new(EditorNode::Group {
                    tabs: vec![2],
                    active: Some(2),
                }),
            }),
            bookmarks: std::collections::BTreeMap::from([(source.clone(), vec![1])]),
            breakpoints: std::collections::BTreeMap::from([(
                source.clone(),
                vec![(2, Some("value == 42".into()))],
            )]),
            project_env: "RESTORED=yes".into(),
            ..Default::default()
        };
        session.save(root.path()).unwrap();
        app.open_workspace(root.path());
        assert!(app.workspace.is_none());
        assert!(app.docs.list.is_empty());
        app.wait_workspace_open();
        assert_eq!(app.docs.list.len(), 2);
        let doc = app.docs.active_doc().unwrap();
        assert_eq!(doc.path, source);
        assert!(doc.bom);
        assert_eq!(doc.editor.buffer.line_ending(), rc_text::LineEnding::CrLf);
        assert!(doc.pinned);
        assert_eq!(doc.editor.caret_line_col(), (0, 3));
        assert!(doc.editor.is_folded(0));
        assert!(doc.editor.bookmarks.contains(&1));
        assert!(doc.editor.breakpoints.contains_key(&2));
        assert_eq!(app.runner.project_env, "RESTORED=yes");
        assert!(!doc.is_modified());
        assert!(!root.path().join("project-code-ran").exists());
        assert!(app.runner.cargo_workspace.is_none());
        let ids = app
            .docs
            .list
            .iter()
            .map(|d| d.editor.id)
            .collect::<Vec<_>>();
        assert!(app.docs.reserve_id() > *ids.iter().max().unwrap());
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 900.0));
        h.run_steps(2);
        assert!(!root.path().join("project-code-ran").exists());
        h.state_mut().trust_project_tools_for_test();
        pump(h.state_mut(), |app| app.runner.cargo_workspace.is_some());
        h.state_mut().run_cargo_command("run --offline --quiet");
        pump(h.state_mut(), |app| {
            app.runner
                .build
                .as_ref()
                .is_some_and(|build| build.result.is_some())
        });
        assert!(
            h.state()
                .runner
                .build
                .as_ref()
                .unwrap()
                .result
                .as_ref()
                .unwrap()
                .success
        );
        assert!(root.path().join("project-code-ran").exists());
    }
    #[test]
    fn cancel_stale_edits_and_latest_workspace_requests_preserve_original_buffers_and_batch_positions()
     {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let old = tempfile::tempdir().unwrap();
        let next = tempfile::tempdir().unwrap();
        let last = tempfile::tempdir().unwrap();
        let original = project(old.path());
        let next_file = project(next.path());
        let last_file = project(last.path());
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace_and_wait(old.path());
        app.open_file_and_wait(&original);
        let gate = Arc::new(AtomicBool::new(false));
        app.workspace_open.gate = Some(gate.clone());
        app.open_workspace(next.path());
        app.queue_workspace_files(vec![next_file.clone()]);
        pump(&mut app, |a| a.workspace_open.active.is_some());
        assert_eq!(app.workspace.as_ref().unwrap().ws.root, old.path());
        app.docs.list[0]
            .editor
            .replace(0, 0, "// keep newest edit\n");
        gate.store(true, Ordering::Release);
        pump(&mut app, |a| !a.workspace_open.busy());
        assert_eq!(app.workspace.as_ref().unwrap().ws.root, old.path());
        assert!(app.docs.list[0].is_modified());
        assert!(
            app.docs.list[0]
                .editor
                .buffer
                .rope()
                .to_string()
                .starts_with("// keep newest edit")
        );
        app.open_workspace(next.path());
        app.queue_workspace_files(vec![next_file]);
        app.cancel_disk_save();
        assert!(app.workspace_open.target.is_none());
        assert!(app.workspace_open.files.is_empty());
        assert_eq!(app.workspace.as_ref().unwrap().ws.root, old.path());
        app.open_workspace(next.path());
        app.open_workspace(last.path());
        app.open_at(&last_file, 2, 4);
        app.wait_workspace_open();
        assert_eq!(app.workspace.as_ref().unwrap().ws.root, last.path());
        let doc = app.docs.active_doc().unwrap();
        assert_eq!(doc.path, last_file);
        assert_eq!(doc.editor.caret_line_col(), (2, 4));
        assert!(!last.path().join("project-code-ran").exists());
        assert!(
            std::fs::read_to_string(&original)
                .unwrap()
                .contains("keep newest edit")
        );
    }
    #[test]
    #[cfg(unix)]
    fn session_fifo_symlink_and_oversized_inputs_do_not_wait_for_writers_or_run_code() {
        use std::os::unix::fs::symlink;
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        project(root.path());
        let mut app = crate::screens::test_app(home.path(), false);
        let session = Session::file_for(root.path());
        std::fs::create_dir_all(session.parent().unwrap()).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&session)
                .status()
                .unwrap()
                .success()
        );
        let started = Instant::now();
        app.open_workspace_and_wait(root.path());
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(app.docs.list.is_empty());
        assert!(!root.path().join("project-code-ran").exists());
        std::fs::remove_file(&session).unwrap();
        let target = root.path().join("external-session.json");
        std::fs::write(&target, "{}").unwrap();
        symlink(&target, &session).unwrap();
        assert!(matches!(
            read(&session, &AtomicBool::new(false)),
            Err(Error::Io(_))
        ));
        std::fs::remove_file(&session).unwrap();
        std::fs::File::create(&session)
            .unwrap()
            .set_len(4 * 1024 * 1024 + 1)
            .unwrap();
        assert!(matches!(
            read(&session, &AtomicBool::new(false)),
            Err(Error::Limit)
        ));
    }
}

#[cfg(test)]
mod live_state_tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn setup(root: &Path) -> PathBuf {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"),"[package]\nname=\"live_workspace\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(
            root.join("build.rs"),
            "fn main(){std::fs::write(\"code-ran\",\"yes\").unwrap();}\n",
        )
        .unwrap();
        let path = root.join("src/main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        path
    }
    fn active(app: &mut crate::app::CrabCraftApp) {
        let end = Instant::now() + Duration::from_secs(10);
        while app.workspace_open.active.is_none() {
            app.poll_disk_save();
            app.poll_workspace_open();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn trust_revocation_and_new_settings_during_preparation_are_revalidated_live() {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        setup(root.path());
        let mut app = crate::screens::test_app(home.path(), false);
        app.settings
            .trusted_projects
            .push(root.path().to_path_buf());
        let gate = Arc::new(AtomicBool::new(false));
        app.workspace_open.gate = Some(gate.clone());
        app.open_workspace(root.path());
        active(&mut app);
        app.settings.trusted_projects.clear();
        app.settings.use_nextest = true;
        app.settings.new_project_location = Some(root.path().join("chosen-later"));
        gate.store(true, Ordering::Release);
        app.wait_workspace_open();
        assert!(app.runner.trusted_project.is_none());
        assert!(app.runner.cargo_workspace.is_none());
        assert!(app.runner.nextest);
        assert_eq!(
            app.settings.new_project_location,
            Some(root.path().join("chosen-later"))
        );
        assert!(!root.path().join("code-ran").exists());
    }
    #[test]
    fn same_workspace_reload_preserves_run_options_changed_while_the_worker_is_busy() {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = setup(root.path());
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace_and_wait(root.path());
        app.open_file_and_wait(&path);
        let gate = Arc::new(AtomicBool::new(false));
        app.workspace_open.gate = Some(gate.clone());
        app.open_workspace(root.path());
        active(&mut app);
        app.runner.options.insert(
            "manual".into(),
            rc_core::session::RunOptions {
                args: "--latest-option".into(),
                ..Default::default()
            },
        );
        app.session_dirty = true;
        gate.store(true, Ordering::Release);
        app.wait_workspace_open();
        assert_eq!(
            app.runner.options.get("manual").unwrap().args,
            "--latest-option"
        );
        assert_eq!(app.docs.active_doc().unwrap().path, path);
        assert!(!root.path().join("code-ran").exists());
    }
}

#[cfg(test)]
mod edit_barrier_test {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn late_compiler_edit_finishes_before_workspace_commit_and_keeps_its_dirty_buffer() {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let old = tempfile::tempdir().unwrap();
        let next = tempfile::tempdir().unwrap();
        for root in [old.path(), next.path()] {
            std::fs::create_dir(root.join("src")).unwrap();
            std::fs::write(
                root.join("Cargo.toml"),
                "[package]\nname=\"late_edit\"\nversion=\"0.1.0\"\n[workspace]\n",
            )
            .unwrap();
            std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        }
        let source = old.path().join("src/main.rs");
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace_and_wait(old.path());
        app.open_file_and_wait(&source);
        let gate = Arc::new(AtomicBool::new(false));
        app.workspace_open.gate = Some(gate.clone());
        app.open_workspace(next.path());
        let end = Instant::now() + Duration::from_secs(10);
        while app.workspace_open.active.is_none() {
            app.poll_disk_save();
            app.poll_workspace_open();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        }
        let fix = rc_dotnet::diagnostics::Fix {
            label: "keep late edit".into(),
            source: rc_dotnet::diagnostics::source_hash(&std::fs::read(&source).unwrap()),
            edits: vec![(0, 0, "// late compiler edit\n".into())],
        };
        app.request_file_edit(&source, crate::file_edit::Intent::Build(fix));
        assert!(app.file_edits.busy());
        gate.store(true, Ordering::Release);
        while app.workspace_open.ready.is_none() {
            app.poll_workspace_open();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(app.workspace.as_ref().unwrap().ws.root, old.path());
        assert!(app.file_edits.busy());
        while app.file_edits.busy() {
            app.poll_file_edits();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        }
        app.poll_workspace_open();
        assert_eq!(app.workspace.as_ref().unwrap().ws.root, old.path());
        assert!(app.docs.active_doc().unwrap().is_modified());
        assert!(
            app.docs
                .active_doc()
                .unwrap()
                .editor
                .buffer
                .rope()
                .to_string()
                .starts_with("// late compiler edit")
        );
        assert!(app.workspace_open.target.is_none());
    }
}
