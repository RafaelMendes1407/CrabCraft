//! Find in Files replacements: bounded background preparation/disk writes, followed by
//! version-checked undoable edits of open documents on the UI thread.

use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use rc_search::replace::{FileStamp, ReplaceError, ReplaceLimits};

const MAX_FILES: usize = 5000;
const TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_EDITS: usize = 100_000;
const MAX_ERRORS: usize = 100;

enum Target { Disk(FileStamp), Buffer { id: u64, version: u64, rope: rc_text::Rope, read_only: bool } }
struct Item { path: PathBuf, rel: String, target: Target }

enum Outcome {
    Disk(usize),
    Buffer { id: u64, version: u64, edits: Vec<rc_text::Change> },
    Failed(String),
}
enum Event { File(PathBuf, Outcome), Done(bool), Failed(String) }

#[derive(Default)]
pub struct Report {
    pub files: usize,
    pub matches: usize,
    pub skipped: usize,
    pub processed: usize,
    pub total: usize,
    pub cancelled: bool,
    pub errors: Vec<(PathBuf, String)>,
}

pub struct ReplaceFiles { inbox: Inbox<Event>, cancel: Option<Arc<AtomicBool>>, pub report: Report }
impl Drop for ReplaceFiles { fn drop(&mut self) { self.cancel(); } }

impl ReplaceFiles {
    pub fn new(waker: Waker) -> Self { Self { inbox: Inbox::new(waker), cancel: None, report: Report::default() } }
    pub fn running(&self) -> bool { self.cancel.is_some() }
    pub fn cancelling(&self) -> bool { self.cancel.as_ref().is_some_and(|cancel| cancel.load(Ordering::Relaxed)) }
    pub fn cancel(&self) { if let Some(cancel) = &self.cancel { cancel.store(true, Ordering::Relaxed); } }

    #[allow(clippy::too_many_arguments)]
    pub fn start(&mut self, root: &Path, files: &[rc_search::FileMatches], docs: &crate::docs::Docs, options: rc_search::QueryOptions, replacement: String, mask: String, excluded: Vec<String>, only: Option<rc_search::ScopeFilter>) -> Result<(), &'static str> {
        if self.running() { return Err("A replacement is already running"); }
        if files.len() > MAX_FILES || files.iter().any(|file| file.path.as_os_str().len() > 8192 || file.rel.len() > 8192) || excluded.len() > 4096 || excluded.iter().any(|path| path.len() > 8192) || options.pattern.len() > 64 * 1024 || replacement.len() > 64 * 1024 || mask.len() > 64 * 1024 { return Err("Replacement exceeded its file, edit or output limit"); }
        let mut snapshot_bytes = 0_usize;
        let items: Vec<_> = files.iter().map(|file| {
            let target = match docs.list.iter().find(|doc| doc.path == file.path) {
                Some(doc) => { snapshot_bytes = snapshot_bytes.saturating_add(doc.editor.buffer.rope().len_bytes()); Target::Buffer { id: doc.editor.id, version: doc.editor.buffer.version(), rope: doc.editor.buffer.snapshot(), read_only: doc.editor.read_only } },
                None => Target::Disk(file.stamp.clone()),
            };
            Item { path: file.path.clone(), rel: file.rel.clone(), target }
        }).collect();
        if snapshot_bytes > TOTAL_BYTES { return Err("Replacement exceeded its file, edit or output limit"); }
        self.report = Report { total: items.len(), ..Default::default() };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let out = self.inbox.sender();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let query = match rc_search::Query::new(options) { Ok(query) => query, Err(error) => { out.send(Event::Failed(trf("Cannot replace files: {error}", &[("error", &error.to_string().chars().take(2048).collect::<String>())]))); return; } };
            let mask = match rc_search::FileMask::parse(&mask) { Ok(mask) => mask, Err(error) => { out.send(Event::Failed(trf("Cannot replace files: {error}", &[("error", &error)]))); return; } };
            let root = match root.canonicalize() { Ok(root) => root, Err(error) => { out.send(Event::Failed(error.to_string())); return; } };
            let (mut remaining_edits, mut remaining_bytes) = (MAX_EDITS, TOTAL_BYTES);
            for item in items {
                if cancel.load(Ordering::Relaxed) { break; }
                let eligible = mask.matches(&item.rel) && !rc_core::workspace::is_excluded(&item.rel, &excluded) && only.as_ref().is_none_or(|scope| scope.matches(&item.rel));
                let outcome = if !eligible || item.path.canonicalize().ok().is_none_or(|path| !path.starts_with(&root)) {
                    Outcome::Failed(tr("File is no longer in the selected scope; skipped").into())
                } else {
                    let limits = ReplaceLimits { bytes: (8 * 1024 * 1024).min(remaining_bytes), edits: remaining_edits };
                    match item.target {
                        Target::Disk(stamp) => match rc_search::replace::replace_in_file_guarded(&item.path, &stamp, &query, &replacement, &cancel, limits) {
                            Ok((count, bytes)) => { remaining_edits = remaining_edits.saturating_sub(count); remaining_bytes = remaining_bytes.saturating_sub(bytes); Outcome::Disk(count) },
                            Err(error) => Outcome::Failed(error_text(&error)),
                        },
                        Target::Buffer { read_only: true, .. } => Outcome::Failed(tr("The document is read-only; skipped").into()),
                        Target::Buffer { id, version, rope, .. } => {
                            let text = rope.to_string();
                            match rc_search::replace::prepare_replacement(&text, &query, &replacement, &cancel, ReplaceLimits { edits: limits.edits.min(10_000), ..limits }) {
                                Ok(plan) => { remaining_edits = remaining_edits.saturating_sub(plan.edits.len()); remaining_bytes = remaining_bytes.saturating_sub(if plan.edits.is_empty() { 0 } else { plan.output.len() }); Outcome::Buffer { id, version, edits: plan.edits.into_iter().map(|edit| rc_text::Change::new(edit.range.start, edit.range.end, edit.text)).collect() } },
                                Err(error) => Outcome::Failed(error_text(&error)),
                            }
                        }
                    }
                };
                if !out.send(Event::File(item.path, outcome)) { return; }
            }
            out.send(Event::Done(cancel.load(Ordering::Relaxed)));
        });
        Ok(())
    }

    fn failure(&mut self, path: PathBuf, message: String) {
        self.report.skipped += 1;
        if self.report.errors.len() < MAX_ERRORS { self.report.errors.push((path, message.chars().take(2048).collect())); }
    }

    /// Returns true once when the job finishes. No file I/O or regex matching runs here.
    pub fn poll(&mut self, docs: &mut crate::docs::Docs) -> bool {
        let mut finished = false;
        for event in self.inbox.drain() {
            match event {
                Event::File(path, outcome) => {
                    self.report.processed += 1;
                    match outcome {
                        Outcome::Disk(count) => { if count > 0 { self.report.files += 1; self.report.matches += count; } },
                        Outcome::Failed(message) => self.failure(path, message),
                        Outcome::Buffer { id, version, edits } => {
                            if self.cancelling() { self.failure(path, tr("Cancelled").into()); continue; }
                            let Some(doc) = docs.list.iter_mut().find(|doc| doc.path == path && doc.editor.id == id) else { self.failure(path, tr("The document was closed or reopened; skipped").into()); continue };
                            if doc.editor.read_only { self.failure(path, tr("The document is read-only; skipped").into()); continue; }
                            if doc.editor.buffer.version() != version { self.failure(path, tr("The open document changed; skipped").into()); continue; }
                            if !edits.is_empty() {
                                self.report.files += 1;
                                self.report.matches += edits.len();
                                doc.editor.apply_text_edits(edits);
                            }
                        }
                    }
                }
                Event::Done(cancelled) => {
                    self.report.cancelled = cancelled || self.cancelling();
                    if self.report.cancelled { self.report.skipped += self.report.total.saturating_sub(self.report.processed); }
                    self.cancel = None; finished = true;
                },
                Event::Failed(message) => {
                    self.failure(PathBuf::new(), message);
                    self.report.skipped += self.report.total.saturating_sub(1);
                    self.report.processed = self.report.total;
                    self.report.cancelled = self.cancelling();
                    self.cancel = None; finished = true;
                },
            }
        }
        finished
    }
}

fn error_text(error: &ReplaceError) -> String {
    match error {
        ReplaceError::Io(error) => error.to_string(),
        ReplaceError::NotUtf8 => tr("File is not valid UTF-8").into(),
        ReplaceError::Changed => tr("File changed after the search; skipped").into(),
        ReplaceError::NotRegular => tr("Only regular files can be replaced").into(),
        ReplaceError::ReadOnly => tr("The document is read-only; skipped").into(),
        ReplaceError::Limit => tr("Replacement exceeded its file, edit or output limit").into(),
        ReplaceError::Cancelled => tr("Cancelled").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};

    const MAIN: &str = "mod other;\nfn main() {\n    let value = 1;\n    assert_eq!(other::number() + value, 3);\n}\n";
    const OTHER: &str = "pub fn number() -> i32 { 2 }\n";
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace with spaces");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"replace-fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(root.join("src/main.rs"), MAIN).unwrap();
        std::fs::write(root.join("src/other.rs"), OTHER).unwrap();
        std::fs::write(root.join("build.rs"), r#"fn main() { std::fs::write("build-ran", "yes").unwrap(); }"#).unwrap();
        (dir, root.canonicalize().unwrap())
    }
    fn options() -> rc_search::QueryOptions { rc_search::QueryOptions { pattern: r"\b(value|number)\b".into(), regex: true, case_sensitive: true, whole_word: false } }
    fn results(root: &Path) -> Vec<rc_search::FileMatches> {
        let inbox = Inbox::new(Waker::noop());
        let handle = rc_search::find_in_files(root, rc_search::FindOptions { query: options(), mask: rc_search::FileMask::parse("*.rs").unwrap(), ..Default::default() }, inbox.sender()).unwrap();
        let mut files = Vec::new();
        loop {
            match inbox.recv_timeout(Duration::from_secs(10)).expect("search completes") {
                rc_search::FindEvent::File { file, .. } => files.push(file),
                rc_search::FindEvent::Done { summary, .. } => { assert!(!summary.truncated && !summary.cancelled); break; }
            }
        }
        drop(handle);
        files.sort_by(|a,b| a.path.cmp(&b.path));
        files
    }
    fn wait(job: &mut ReplaceFiles, docs: &mut crate::docs::Docs) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            job.poll(docs);
            if !job.running() { return; }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("replacement did not complete");
    }

    #[test]
    fn native_replace_all_uses_latest_open_buffer_and_closed_rust_file_with_regex_and_undo() {
        let _guard = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (dir, root) = fixture();
        let main = root.join("src/main.rs");
        let other = root.join("src/other.rs");
        let mut app = crate::screens::test_app(&dir.path().join("home"), true);
        app.open_workspace(&root);
        app.open_file(&main);
        let index = app.docs.index_of(&main).unwrap();
        app.docs.list[index].editor.replace(0, 0, "// unsaved café\n");
        let before = app.docs.list[index].editor.buffer.rope().to_string();
        let mut find = crate::search::FindInFiles::new(&root, Waker::noop(), options().pattern);
        find.regex = true;
        find.case = true;
        find.mask = "*.rs".into();
        find.replace = "${1}_new".into();
        find.show_replace = true;
        find.search();
        app.find_in_files = Some(find);
        app.dock.show(crate::toolwindow::ToolWindow::Find);
        let mut harness = crate::screens::harness(app, egui::vec2(1600.0, 950.0));
        let start = Instant::now();
        while harness.state().find_in_files.as_ref().unwrap().summary.is_none() {
            assert!(start.elapsed() < Duration::from_secs(10));
            harness.run_steps(1);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(harness.state().find_in_files.as_ref().unwrap().results.len(), 2);
        harness.get_by_label("Replace All").click();
        for _ in 0..500 {
            harness.run_steps(1);
            let replacement = &harness.state().find_in_files.as_ref().unwrap().replacements;
            if replacement.report.total == 2 && !replacement.running() { break; }
            std::thread::sleep(Duration::from_millis(5));
        }
        let replacement = &harness.state().find_in_files.as_ref().unwrap().replacements;
        assert!(!replacement.running());
        assert_eq!((replacement.report.files, replacement.report.matches, replacement.report.skipped), (2, 4, 0));
        assert!(!root.join("build-ran").exists(), "search and replacement must not execute build.rs");
        assert_eq!(std::fs::read_to_string(&main).unwrap(), MAIN, "open files are not saved implicitly");
        assert_eq!(std::fs::read_to_string(&other).unwrap(), OTHER.replace("number", "number_new"));
        let doc = &mut harness.state_mut().docs.list[index];
        let changed = doc.editor.buffer.rope().to_string();
        assert!(changed.contains("unsaved café") && changed.contains("value_new") && changed.contains("number_new"));
        doc.editor.undo();
        assert_eq!(doc.editor.buffer.rope().to_string(), before);
        doc.editor.redo();
        assert_eq!(doc.editor.buffer.rope().to_string(), changed);
        harness.state_mut().docs.save(index).unwrap();
        let output = std::process::Command::new("cargo").args(["check", "--offline"]).current_dir(&root).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(root.join("build-ran").exists());
    }

    #[test]
    fn changed_open_versions_and_changed_search_time_disk_files_are_preserved_and_reported() {
        let (_dir, root) = fixture();
        let files = results(&root);
        let mut docs = crate::docs::Docs::new(rc_syntax::ParserPool::new(Arc::new(|| {})));
        let index = docs.open(&root.join("src/main.rs")).unwrap();
        let external = "pub fn number() -> i32 { 99 }\n";
        std::fs::write(root.join("src/other.rs"), external).unwrap();
        let mut job = ReplaceFiles::new(Waker::noop());
        job.start(&root, &files, &docs, options(), "${1}_new".into(), "*.rs".into(), Vec::new(), None).unwrap();
        docs.list[index].editor.replace(0, 0, "// typed after Replace All\n");
        let expected = docs.list[index].editor.buffer.rope().to_string();
        wait(&mut job, &mut docs);
        assert_eq!(job.report.files, 0);
        assert_eq!(job.report.skipped, 2);
        assert_eq!(job.report.errors.len(), 2);
        assert_eq!(docs.list[index].editor.buffer.rope().to_string(), expected);
        assert_eq!(std::fs::read_to_string(root.join("src/other.rs")).unwrap(), external);
        assert_eq!(std::fs::read_to_string(root.join("src/main.rs")).unwrap(), MAIN);
    }

    #[test]
    fn cancellation_discards_pending_buffer_edits_and_closed_reopened_buffers_are_not_touched() {
        let (_dir, root) = fixture();
        let files = results(&root);
        let mut docs = crate::docs::Docs::new(rc_syntax::ParserPool::new(Arc::new(|| {})));
        let index = docs.open(&root.join("src/main.rs")).unwrap();
        let open: Vec<_> = files.into_iter().filter(|file| file.path == root.join("src/main.rs")).collect();
        let mut job = ReplaceFiles::new(Waker::noop());
        job.start(&root, &open, &docs, options(), "${1}_new".into(), "*.rs".into(), Vec::new(), None).unwrap();
        job.cancel();
        wait(&mut job, &mut docs);
        assert!(job.report.cancelled);
        assert_eq!(docs.list[index].editor.buffer.rope().to_string(), MAIN);
        job.start(&root, &open, &docs, options(), "${1}_new".into(), "*.rs".into(), Vec::new(), None).unwrap();
        docs.close(index);
        let reopened = docs.open(&root.join("src/main.rs")).unwrap();
        wait(&mut job, &mut docs);
        assert_eq!(job.report.skipped, 1);
        assert_eq!(docs.list[reopened].editor.buffer.rope().to_string(), MAIN);
        assert!(!root.join("build-ran").exists());
    }
}
