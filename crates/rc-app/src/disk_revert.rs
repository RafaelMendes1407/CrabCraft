//! Explicit Revert from Disk: bounded cancellable I/O and guarded, undoable application.
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::SystemTime;

const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Ticket {
    pub id: u64,
    pub path: PathBuf,
    pub version: u64,
    pub disk_epoch: u64,
}
impl Ticket {
    pub fn capture(doc: &crate::docs::Doc) -> Self {
        Self { id: doc.editor.id, path: doc.path.clone(), version: doc.editor.buffer.version(), disk_epoch: doc.disk_epoch }
    }
    pub fn index(&self, docs: &crate::docs::Docs) -> Option<usize> {
        docs.list.iter().position(|doc| doc.editor.id == self.id && doc.path == self.path && doc.editor.buffer.version() == self.version && doc.disk_epoch == self.disk_epoch && !doc.generated)
    }
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    NotRegular,
    TooLarge,
    InvalidText,
    ChangedOnDisk,
    Stale,
    Cancelled,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&match self {
            Self::Io(error) => trf("Could not read the disk version: {error}", &[("error", error)]),
            Self::NotRegular => tr("Revert from Disk requires a regular file").to_string(),
            Self::TooLarge => tr("Revert from Disk supports files up to 8 MB").to_string(),
            Self::InvalidText => tr("Revert from Disk requires a UTF-8 text file").to_string(),
            Self::ChangedOnDisk => tr("The file changed while reading it; try Revert from Disk again").to_string(),
            Self::Stale => tr("The document changed or closed while loading; its edits were kept").to_string(),
            Self::Cancelled => tr("Disk reload cancelled").to_string(),
        })
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error { fn from(error: std::io::Error) -> Self { Self::Io(error) } }

#[derive(PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: SystemTime,
    #[cfg(unix)] device: u64,
    #[cfg(unix)] inode: u64,
}
impl Stamp {
    fn of(metadata: &std::fs::Metadata) -> Result<Self, Error> {
        #[cfg(unix)] use std::os::unix::fs::MetadataExt;
        Ok(Self { length: metadata.len(), modified: metadata.modified()?, #[cfg(unix)] device: metadata.dev(), #[cfg(unix)] inode: metadata.ino() })
    }
}

pub struct Snapshot { pub text: String, pub bom: bool, pub modified: SystemTime }

pub(crate) fn read(path: &Path, cancel: &AtomicBool) -> Result<Snapshot, Error> {
    if cancel.load(Ordering::Acquire) { return Err(Error::Cancelled); }
    if !std::fs::metadata(path)?.is_file() { return Err(Error::NotRegular); }
    let mut options = std::fs::OpenOptions::new(); options.read(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.custom_flags(libc::O_NONBLOCK); }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() { return Err(Error::NotRegular); }
    let before = Stamp::of(&metadata)?;
    if before.length > MAX_BYTES as u64 { return Err(Error::TooLarge); }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Acquire) { return Err(Error::Cancelled); }
        let count = match file.read(&mut chunk) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 { break; }
        if bytes.len() + count > MAX_BYTES { return Err(Error::TooLarge); }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let current_path = std::fs::metadata(path)?;
    if !current_path.is_file() || before != Stamp::of(&file.metadata()?)? || before != Stamp::of(&current_path)? { return Err(Error::ChangedOnDisk); }
    if cancel.load(Ordering::Acquire) { return Err(Error::Cancelled); }
    if bytes.contains(&0) { return Err(Error::InvalidText); }
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    if bom { bytes.drain(..3); }
    let text = String::from_utf8(bytes).map_err(|_| Error::InvalidText)?;
    Ok(Snapshot { text, bom, modified: before.modified })
}

pub struct Job {
    pub ticket: Ticket,
    inbox: Inbox<Result<Snapshot, Error>>,
    cancel: Arc<AtomicBool>,
}
impl Job {
    pub fn start(ticket: Ticket, waker: Waker) -> Result<Self, Error> {
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let path = ticket.path.clone();
        std::thread::Builder::new().name("rc-revert-disk".into()).spawn(move || { out.send(read(&path, &token)); })?;
        Ok(Self { ticket, inbox, cancel })
    }
    pub fn poll(&self) -> Option<Result<Snapshot, Error>> { self.inbox.drain().into_iter().next() }
}
impl Drop for Job { fn drop(&mut self) { self.cancel.store(true, Ordering::Release); } }

pub fn apply(docs: &mut crate::docs::Docs, ticket: &Ticket, snapshot: Snapshot) -> Result<usize, Error> {
    let index = ticket.index(docs).ok_or(Error::Stale)?;
    // Preserve the discarded text in Local History and the undo stack. Undo restores text
    // as an unsaved edit; encoding metadata follows the disk snapshot, as for normal reload.
    docs.remember(index);
    docs.reloaded(index, &snapshot.text, snapshot.bom, Some(snapshot.modified));
    docs.list[index].editor.request_parse(&docs.pool);
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"revert_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main() { println!(\"old\"); }\n").unwrap();
        root
    }
    fn docs(root: &Path) -> crate::docs::Docs {
        let mut docs = crate::docs::Docs::new(rc_syntax::ParserPool::new(Waker::noop().as_fn()));
        docs.open(&root.join("src/main.rs")).unwrap();
        docs
    }
    fn frames(harness: &mut egui_kittest::Harness<'_, crate::app::CrabCraftApp>, label: &str, done: impl Fn(&crate::app::CrabCraftApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(harness.state()) {
            harness.run_steps(1);
            assert!(Instant::now() < deadline, "{label} did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn snapshot(path: &Path) -> Snapshot { read(path, &AtomicBool::new(false)).unwrap() }
    fn wait(job: &Job) -> Result<Snapshot, Error> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = job.poll() { return result; }
            assert!(Instant::now() < deadline, "disk worker did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn delayed_disk_result_preserves_new_edits_renames_saves_and_reopened_tabs() {
        let root = fixture();
        let path = root.path().join("src/main.rs");
        let mut docs = docs(root.path());
        let ticket = Ticket::capture(&docs.list[0]);
        let job = Job::start(ticket.clone(), Waker::noop()).unwrap();
        docs.list[0].editor.replace(0, 0, "// typed while disk read was pending\n");
        assert!(matches!(apply(&mut docs, &ticket, wait(&job).unwrap()), Err(Error::Stale)));
        assert!(docs.list[0].editor.buffer.rope().to_string().contains("typed while"));
        let ticket = Ticket::capture(&docs.list[0]);
        let loaded = snapshot(&path);
        let version = docs.list[0].editor.buffer.version();
        docs.save(0).unwrap();
        assert_eq!(docs.list[0].editor.buffer.version(), version);
        assert!(matches!(apply(&mut docs, &ticket, loaded), Err(Error::Stale)), "saving invalidates a pending disk result even without text edits");
        let ticket = Ticket::capture(&docs.list[0]);
        let loaded = snapshot(&path);
        docs.list[0].path = root.path().join("renamed.rs");
        assert!(matches!(apply(&mut docs, &ticket, loaded), Err(Error::Stale)));
        docs.list[0].path = path.clone();
        let ticket = Ticket::capture(&docs.list[0]);
        let loaded = snapshot(&path);
        docs.close(0);
        docs.open(&path).unwrap();
        assert!(matches!(apply(&mut docs, &ticket, loaded), Err(Error::Stale)));
    }

    #[test]
    fn watcher_events_invalidate_pending_results_and_unsafe_inputs_are_rejected() {
        let root = fixture();
        let path = root.path().join("src/main.rs");
        let mut docs = docs(root.path());
        docs.list[0].editor.replace(0, 0, "// unsaved\n");
        let ticket = Ticket::capture(&docs.list[0]);
        let old_disk = snapshot(&path);
        std::fs::write(&path, "fn main() { println!(\"external change\"); }\n").unwrap();
        docs.on_disk_change(&path);
        assert!(docs.list[0].conflict);
        assert!(matches!(apply(&mut docs, &ticket, old_disk), Err(Error::Stale)));
        std::fs::write(&path, [0xFF]).unwrap();
        assert!(matches!(read(&path, &AtomicBool::new(false)), Err(Error::InvalidText)));
        std::fs::write(&path, b"text\0binary").unwrap();
        assert!(matches!(read(&path, &AtomicBool::new(false)), Err(Error::InvalidText)));
        std::fs::File::create(&path).unwrap().set_len((MAX_BYTES + 1) as u64).unwrap();
        assert!(matches!(read(&path, &AtomicBool::new(false)), Err(Error::TooLarge)));
        assert!(matches!(read(&path, &AtomicBool::new(true)), Err(Error::Cancelled)));
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_rejected_without_waiting_for_a_writer() {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        use std::io::Write;
        let root = fixture();
        let path = root.path().join("fifo.rs");
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: this is a valid NUL-terminated temporary path.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let (send, receive) = std::sync::mpsc::channel();
        let fifo = path.clone();
        std::thread::spawn(move || { let _ = send.send((read(&fifo, &AtomicBool::new(false)), crate::docs::read_text_checked(&fifo))); });
        let result = receive.recv_timeout(Duration::from_secs(1));
        if result.is_err() {
            if let Ok(mut writer) = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&path) { let _ = writer.write_all(b"text\n"); }
        }
        assert!(matches!(result, Ok((Err(Error::NotRegular), Err(crate::docs::TextReadError::NotRegular)))));
    }

    #[test]
    fn clean_document_action_loads_the_real_disk_version_outside_conflicts() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = fixture();
        let path = root.path().join("src/main.rs");
        let mut app = crate::screens::test_app(home.path(), false);
        app.workspace = Some(crate::app::WorkspaceState { ws: rc_core::Workspace::open(root.path()).unwrap() });
        app.open_file_and_wait(&path);
        let disk = b"\xEF\xBB\xBFfn main() { println!(\"current disk\"); }\r\n";
        std::fs::write(&path, disk).unwrap();
        assert!(!app.docs.list[0].is_modified() && !app.docs.list[0].conflict);
        app.run_ide_action(&egui::Context::default(), rc_editor::Action::RevertFromDisk);
        assert!(app.dialog.is_none());
        assert!(app.disk_revert.is_some());
        let mut harness = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
        frames(&mut harness, "clean disk snapshot", |app| app.disk_revert.is_none());
        let doc = &harness.state().docs.list[0];
        assert!(doc.bom);
        assert_eq!(doc.editor.buffer.to_disk_string().as_bytes(), &disk[3..]);
        assert_eq!(doc.mtime, std::fs::metadata(&path).unwrap().modified().ok());
        assert!(!doc.is_modified() && !doc.conflict);
        assert_eq!(std::fs::read(path).unwrap(), disk);
    }

    #[test]
    fn native_confirmation_cancel_and_reload_preserve_bom_crlf_and_undo() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = fixture();
        let path = root.path().join("src/main.rs");
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace_and_wait(root.path());
        app.open_file_and_wait(&path);
        app.docs.list[0].editor.replace(0, 0, "// unsaved local change\n");
        let dirty_text = app.docs.list[0].editor.buffer.rope().to_string();
        let disk = b"\xEF\xBB\xBFfn main() {\r\n    println!(\"new disk version\");\r\n}\r\n";
        std::fs::write(&path, disk).unwrap();
        let mut harness = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
        frames(&mut harness, "external disk change", |app| app.docs.list[0].conflict);
        harness.state_mut().request_disk_revert(0); harness.run_steps(2);
        harness.get_by_label(tr("Cancel")).click(); harness.run_steps(2);
        assert!(harness.state().disk_revert.is_none());
        assert_eq!(harness.state().docs.list[0].editor.buffer.rope().to_string(), dirty_text);
        assert_eq!(std::fs::read(&path).unwrap(), disk);
        harness.state_mut().request_disk_revert(0); harness.run_steps(2);
        harness.get_by_label(tr("Discard and Reload")).click(); harness.run_steps(2);
        frames(&mut harness, "disk snapshot", |app| app.disk_revert.is_none() && !app.docs.list[0].is_modified());
        let doc = &harness.state().docs.list[0];
        assert!(doc.bom);
        assert_eq!(doc.editor.buffer.to_disk_string().as_bytes(), &disk[3..]);
        assert!(!doc.conflict);
        harness.state_mut().docs.list[0].editor.undo();
        assert_eq!(harness.state().docs.list[0].editor.buffer.rope().to_string(), dirty_text);
        assert!(harness.state().docs.list[0].is_modified());
        assert_eq!(std::fs::read(&path).unwrap(), disk);
        harness.state_mut().docs.list[0].editor.redo();
        assert!(!harness.state().docs.list[0].is_modified());
        let output = std::process::Command::new("cargo").args(["check", "--offline"]).current_dir(root.path()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
}
