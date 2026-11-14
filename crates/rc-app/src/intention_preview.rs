//! Intention preview (I-33): while the Alt+Enter list is open, the highlighted action's changes
//! are shown beside it as a diff, before anything is applied.
//!
//! A server action whose edit is resolved lazily (`codeAction/resolve`, as rust-analyzer does)
//! is resolved in the background; moving the selection drops the request (which cancels it on
//! the server) and any diff still being worked out, so a late answer never shows for the wrong
//! row. The resolved action replaces the row, so applying it needs no second request. The IDE's
//! own quick fixes (Cargo.toml fixes, feature fixes, `#[allow]`) are previewed from the same text
//! edits they make. The diff runs on a thread: closed files are read there.

use crate::app::CrabCraftApp;
use crate::convert;
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use rc_lsp::Pending;
use rc_lsp::lsp_types as lsp;
use rc_text::Rope;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Lines of diff kept per file; the rest is summed up in the counts.
const LINES_PER_FILE: usize = 80;
/// Files previewed; an edit touching more lists the first ones.
const MAX_FILES: usize = 20;

/// A local edit worked out from a file's current text (chars `start..end` become `text`), or
/// `None` when it no longer applies.
pub type MakeEdit = Box<dyn FnOnce(&str) -> Option<(usize, usize, String)> + Send>;

/// The edits to preview in one file.
pub enum Edits {
    /// A language server's edits.
    Lsp(Vec<lsp::TextEdit>),
    /// An IDE fix, made from the file's text once it is read.
    Make(MakeEdit),
}

pub struct FileJob {
    pub path: PathBuf,
    /// The open editor's text, if the file is open.
    pub open: Option<Rope>,
    pub edits: Edits,
}

/// How the highlighted action can be previewed.
pub enum Plan {
    /// Text edits, plus notes about what else it does (file operations, a server command).
    Edits(Vec<FileJob>, Vec<String>),
    /// A server action whose edit must be resolved first.
    Resolve(Box<lsp::CodeAction>),
    /// Nothing to diff: why.
    Note(String),
}

/// The diff of one file.
#[derive(Clone, Debug, PartialEq)]
pub struct FilePreview {
    pub path: PathBuf,
    /// `'+'`, `'-'` or `'@'` (hunk header, with the line number) lines.
    pub lines: Vec<crate::local_history::PreviewLine>,
    pub added: usize,
    pub removed: usize,
    /// The file could not be read, or the fix no longer applies.
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    /// Waiting for `codeAction/resolve`.
    Resolving,
    /// The diff is being worked out.
    Computing,
    Ready {
        files: Vec<FilePreview>,
        notes: Vec<String>,
    },
    /// Nothing to diff.
    Note(String),
    Failed(String),
}

/// Identifies what is previewed: row, title, and the document and text version it was asked in.
type Key = (usize, String, u64, u64);
type PreviewJob = (Arc<AtomicBool>, Inbox<Vec<FilePreview>>, Vec<String>);

pub struct IntentionPreview {
    key: Option<Key>,
    pub state: State,
    resolve: Option<Pending<lsp::CodeAction>>,
    /// The open texts when the preview started, for a resolved edit.
    open: Vec<(PathBuf, Rope)>,
    job: Option<PreviewJob>,
    resolved: Option<lsp::CodeAction>,
    waker: Waker,
}

impl Drop for IntentionPreview {
    fn drop(&mut self) {
        self.clear();
    }
}

impl IntentionPreview {
    pub fn new(waker: Waker) -> IntentionPreview {
        IntentionPreview { key: None, state: State::Idle, resolve: None, open: Vec::new(), job: None, resolved: None, waker }
    }

    pub fn is_for(&self, key: &Key) -> bool {
        self.key.as_ref() == Some(key)
    }

    /// Stops all work (a pending resolve is cancelled on the server) and forgets the preview.
    pub fn clear(&mut self) {
        if let Some((cancel, _, _)) = self.job.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.resolve = None;
        self.resolved = None;
        self.open.clear();
        self.key = None;
        self.state = State::Idle;
    }

    /// Starts previewing `plan` for `key`. `resolve` asks the server to resolve an action
    /// (`None`: no server).
    pub fn start(&mut self, key: Key, plan: Plan, open: Vec<(PathBuf, Rope)>, resolve: impl FnOnce(&lsp::CodeAction) -> Option<Pending<lsp::CodeAction>>) {
        self.clear();
        self.key = Some(key);
        match plan {
            Plan::Note(n) => self.state = State::Note(n),
            Plan::Edits(jobs, notes) => self.compute(jobs, notes),
            Plan::Resolve(action) => match resolve(&action) {
                Some(p) => {
                    self.open = open;
                    self.resolve = Some(p);
                    self.state = State::Resolving;
                }
                None => self.state = State::Note(tr("No preview: the language server is not running").into()),
            },
        }
    }

    fn compute(&mut self, jobs: Vec<FileJob>, notes: Vec<String>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        let stop = cancel.clone();
        let spawned = std::thread::Builder::new().name("rc-intention-preview".into()).spawn(move || {
            let found = previews(jobs, &stop);
            if !stop.load(Ordering::Relaxed) {
                out.send(found);
            }
        });
        match spawned {
            Ok(_) => {
                self.job = Some((cancel, inbox, notes));
                self.state = State::Computing;
            }
            Err(e) => self.state = State::Failed(e.to_string()),
        }
    }

    /// Takes in finished work. Returns the resolved action once, to replace its row with.
    pub fn update(&mut self) -> Option<lsp::CodeAction> {
        if let Some(p) = &mut self.resolve
            && let Some(r) = p.poll()
        {
            self.resolve = None;
            match r {
                Ok(action) => {
                    let open = std::mem::take(&mut self.open);
                    match action.edit.as_ref() {
                        Some(edit) => {
                            let (jobs, mut notes) = edit_jobs(edit, &open);
                            if action.command.is_some() {
                                notes.push(tr("Also runs a command of the language server").into());
                            }
                            self.compute(jobs, notes);
                        }
                        None if action.command.is_some() => {
                            self.state = State::Note(tr("Runs a command of the language server; there is no text to preview").into())
                        }
                        None => self.state = State::Note(tr("This action changes no text").into()),
                    }
                    self.resolved = Some(action);
                }
                Err(e) => self.state = State::Failed(e.message),
            }
        }
        if let Some((_, inbox, notes)) = &self.job
            && let Some(files) = inbox.drain().pop()
        {
            let notes = notes.clone();
            self.job = None;
            self.state = State::Ready { files, notes };
        }
        self.resolved.take()
    }

    pub fn busy(&self) -> bool {
        matches!(self.state, State::Resolving | State::Computing)
    }
}

/// The text edits of a workspace edit as preview jobs, with notes for its file operations.
pub fn edit_jobs(edit: &lsp::WorkspaceEdit, open: &[(PathBuf, Rope)]) -> (Vec<FileJob>, Vec<String>) {
    let jobs = crate::insight::workspace_edit_files(edit)
        .into_iter()
        .map(|(path, edits)| FileJob { open: open.iter().find(|(p, _)| *p == path).map(|(_, r)| r.clone()), path, edits: Edits::Lsp(edits) })
        .collect();
    let mut notes = Vec::new();
    if let Some(lsp::DocumentChanges::Operations(ops)) = &edit.document_changes {
        let name = |uri: &lsp::Uri| rc_lsp::uri_to_path(uri).map_or_else(|| uri.as_str().to_string(), |p| p.display().to_string());
        for op in ops {
            match op {
                lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Create(c)) => notes.push(trf("Creates {file}", &[("file", &name(&c.uri))])),
                lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Rename(r)) => {
                    notes.push(trf("Renames {from} to {to}", &[("from", &name(&r.old_uri)), ("to", &name(&r.new_uri))]))
                }
                lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Delete(d)) => notes.push(trf("Deletes {file}", &[("file", &name(&d.uri))])),
                lsp::DocumentChangeOperation::Edit(_) => {}
            }
        }
    }
    (jobs, notes)
}

/// The diffs of `jobs`, in order (closed files are read here). Stops early when cancelled.
pub fn previews(jobs: Vec<FileJob>, cancel: &AtomicBool) -> Vec<FilePreview> {
    let mut out = Vec::new();
    for job in jobs.into_iter().take(MAX_FILES) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        out.push(preview_file(job));
    }
    out
}

fn preview_file(job: FileJob) -> FilePreview {
    let failed = |path: PathBuf, e: String| FilePreview { path, lines: Vec::new(), added: 0, removed: 0, error: Some(e) };
    let rope = match job.open {
        Some(r) => r,
        None => match crate::docs::read_text(&job.path) {
            Ok((text, _, _)) => rc_text::Buffer::new(&text).rope().clone(),
            Err(e) => return failed(job.path, e),
        },
    };
    let old = rope.to_string();
    let changes: Vec<rc_text::Change> = match job.edits {
        Edits::Lsp(edits) => edits
            .iter()
            .map(|e| {
                let (a, b) = convert::range_chars(&rope, &e.range);
                rc_text::Change::new(a, b, e.new_text.replace("\r\n", "\n"))
            })
            .collect(),
        Edits::Make(make) => match make(&old) {
            Some((a, b, text)) => vec![rc_text::Change::new(a, b, text)],
            None => return failed(job.path, tr("The fix no longer applies to this text").into()),
        },
    };
    let len = rope.len_chars();
    if changes.iter().any(|c| c.start > c.end || c.end > len) {
        return failed(job.path, tr("The edit does not fit the file's text").into());
    }
    let mut buf = rc_text::Buffer::new(&old);
    buf.apply(&rc_text::ChangeSet::new(changes), &rc_text::Selection::point(0), &rc_text::Selection::point(0), rc_text::EditKind::Other);
    let new = buf.rope().to_string();
    let (lines, added, removed) = crate::local_history::preview(&old, &new, LINES_PER_FILE);
    FilePreview { path: job.path, lines, added, removed, error: None }
}

/// A plan with one IDE fix, made from the file's text.
fn local_job(path: PathBuf, open: Option<Rope>, make: MakeEdit) -> Plan {
    Plan::Edits(vec![FileJob { path, open, edits: Edits::Make(make) }], Vec::new())
}

impl CrabCraftApp {
    /// The open documents' texts, for previews.
    pub fn open_texts(&self) -> Vec<(PathBuf, Rope)> {
        self.docs.list.iter().map(|d| (d.path.clone(), d.editor.buffer.rope().clone())).collect()
    }

    /// How the Alt+Enter row `action` can be previewed.
    pub fn preview_plan(&self, action: &lsp::CodeActionOrCommand) -> Plan {
        let server_command = || Plan::Note(tr("Runs a command of the language server; there is no text to preview").into());
        let command = match action {
            lsp::CodeActionOrCommand::CodeAction(a) => match (&a.edit, &a.command) {
                (Some(edit), command) => {
                    let (jobs, mut notes) = edit_jobs(edit, &self.open_texts());
                    if command.is_some() {
                        notes.push(tr("Also runs a command of the language server").into());
                    }
                    return Plan::Edits(jobs, notes);
                }
                (None, None) => return Plan::Resolve(Box::new(a.clone())),
                (None, Some(c)) => c,
            },
            lsp::CodeActionOrCommand::Command(c) => c,
        };
        let args = command.arguments.clone().unwrap_or_default();
        let active = self.docs.active_doc();
        let active_job = |make: MakeEdit| match active {
            Some(d) => local_job(d.path.clone(), Some(d.editor.buffer.rope().clone()), make),
            None => Plan::Note(tr("No preview: no file is open").into()),
        };
        match command.command.as_str() {
            crate::insight::UPDATE_DEPENDENCY_COMMAND => {
                let (Some(line), Some(version)) =
                    (args.first().and_then(|v| v.as_u64()).and_then(|n| usize::try_from(n).ok()), args.get(1).and_then(|v| v.as_str()).map(str::to_string))
                else {
                    return Plan::Note(tr("This action changes no text").into());
                };
                active_job(Box::new(move |text| crate::app::dependency_update_edit(text, line, &version)))
            }
            crate::insight::MANIFEST_FIX_COMMAND => {
                let num = |i: usize| args.get(i).and_then(|v| v.as_u64()).and_then(|n| usize::try_from(n).ok());
                let text = |i: usize| args.get(i).and_then(|v| v.as_str()).map(str::to_string);
                let (Some(start), Some(end), Some(old), Some(new)) = (num(0), num(1), text(2), text(3)) else {
                    return Plan::Note(tr("This action changes no text").into());
                };
                active_job(Box::new(move |t| {
                    let rope = Rope::from_str(t);
                    (start <= end && end <= rope.len_chars() && rope.slice(start..end) == old).then_some((start, end, new))
                }))
            }
            crate::feature_fix::ENABLE_FEATURE_COMMAND => {
                let fix = args.first().and_then(|v| serde_json::from_value::<crate::feature_fix::FeatureFix>(v.clone()).ok());
                match fix {
                    Some(fix) => match fix.manifest().map(PathBuf::from) {
                        Some(manifest) => {
                            let open = self.docs.list.iter().find(|d| d.path == manifest).map(|d| d.editor.buffer.rope().clone());
                            local_job(manifest, open, Box::new(move |text| fix.manifest_edit(text).map(|e| (e.start, e.end, e.text))))
                        }
                        None => Plan::Note(tr("Turns the feature on for code analysis; no file changes").into()),
                    },
                    None => Plan::Note(tr("This action changes no text").into()),
                }
            }
            crate::insight::SUPPRESS_COMMAND => {
                let arg = |i: usize| args.get(i).and_then(|v| v.as_str()).unwrap_or_default().to_string();
                match active.and_then(|d| crate::insight::suppress_edit(d, &arg(0), &arg(1))) {
                    Some(edit) => active_job(Box::new(move |_| Some(edit))),
                    None => Plan::Note(tr("This action changes no text").into()),
                }
            }
            crate::insight::RENAME_COMMAND | crate::insight::DERIVE_COMMAND => Plan::Note(tr("Opens a dialog to choose the changes").into()),
            crate::insight::SELECT_IN_EXPLORER | crate::insight::REVEAL_FILE => Plan::Note(tr("This action changes no text").into()),
            _ => server_command(),
        }
    }

    /// Keeps the preview of the Alt+Enter row `selected` of `list` current; a resolved action
    /// replaces its row.
    pub fn update_intention_preview(&mut self, list: &mut [lsp::CodeActionOrCommand], selected: usize) {
        let Some(action) = list.get(selected) else {
            self.intention_preview.clear();
            return;
        };
        let title = match action {
            lsp::CodeActionOrCommand::Command(c) => c.title.clone(),
            lsp::CodeActionOrCommand::CodeAction(a) => a.title.clone(),
        };
        let (doc, version) = self.docs.active_doc().map_or((0, 0), |d| (d.editor.id, d.editor.buffer.version()));
        let key = (selected, title, doc, version);
        if !self.intention_preview.is_for(&key) {
            let plan = self.preview_plan(action);
            let open = if matches!(plan, Plan::Resolve(_)) { self.open_texts() } else { Vec::new() };
            let insight = &self.insight;
            self.intention_preview.start(key, plan, open, |a| insight.resolve_code_action(a));
        }
        if let Some(resolved) = self.intention_preview.update()
            && let Some(lsp::CodeActionOrCommand::CodeAction(a)) = list.get_mut(selected)
            && a.title == resolved.title
        {
            *a = resolved;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(line: u32, from: u32, to: u32, text: &str) -> lsp::TextEdit {
        lsp::TextEdit { range: lsp::Range::new(lsp::Position::new(line, from), lsp::Position::new(line, to)), new_text: text.into() }
    }

    #[test]
    fn edits_of_open_and_closed_files_become_diffs() {
        let dir = tempfile::tempdir().unwrap();
        let closed = dir.path().join("b.rs");
        std::fs::write(&closed, "fn a() {}\nfn b() {}\n").unwrap();
        let open = dir.path().join("a.rs");
        let jobs = vec![
            FileJob {
                path: open.clone(),
                open: Some(Rope::from_str("use x;\nfn main() {}\n")),
                edits: Edits::Lsp(vec![edit(0, 0, 0, "use std::collections::HashMap;\n")]),
            },
            FileJob { path: closed.clone(), open: None, edits: Edits::Lsp(vec![edit(1, 3, 4, "c")]) },
            FileJob { path: dir.path().join("missing.rs"), open: None, edits: Edits::Lsp(vec![edit(0, 0, 0, "x")]) },
        ];
        let found = previews(jobs, &AtomicBool::new(false));
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].lines, vec![('@', "1".to_string()), ('+', "use std::collections::HashMap;".to_string())]);
        assert_eq!((found[0].added, found[0].removed), (1, 0));
        assert_eq!(found[1].lines, vec![('@', "2".to_string()), ('-', "fn b() {}".to_string()), ('+', "fn c() {}".to_string())]);
        assert!(found[2].error.is_some() && found[2].lines.is_empty());
    }

    #[test]
    fn a_local_fix_is_made_from_the_current_text_and_bad_edits_are_reported() {
        let path = PathBuf::from("Cargo.toml");
        let text = "[dependencies]\nserde = \"1.0\"\n";
        let ok = FileJob {
            path: path.clone(),
            open: Some(Rope::from_str(text)),
            edits: Edits::Make(Box::new(|t: &str| t.find("1.0").map(|i| (i, i + 3, "1.2".to_string())))),
        };
        let gone = FileJob { path: path.clone(), open: Some(Rope::from_str(text)), edits: Edits::Make(Box::new(|_: &str| None)) };
        let outside = FileJob { path, open: Some(Rope::from_str(text)), edits: Edits::Make(Box::new(|_: &str| Some((5, 900, String::new())))) };
        let found = previews(vec![ok, gone, outside], &AtomicBool::new(false));
        assert_eq!(found[0].lines, vec![('@', "2".to_string()), ('-', "serde = \"1.0\"".to_string()), ('+', "serde = \"1.2\"".to_string())]);
        assert!(found[1].error.is_some());
        assert!(found[2].error.is_some());
        // Cancelled: nothing more is worked out.
        assert!(
            previews(vec![FileJob { path: PathBuf::from("x"), open: Some(Rope::from_str("")), edits: Edits::Lsp(Vec::new()) }], &AtomicBool::new(true))
                .is_empty()
        );
    }

    #[test]
    fn file_operations_are_listed_as_notes() {
        let uri = rc_lsp::path_to_uri(std::path::Path::new("/p/src/new.rs"));
        let edit = lsp::WorkspaceEdit {
            document_changes: Some(lsp::DocumentChanges::Operations(vec![lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Create(lsp::CreateFile {
                uri,
                options: None,
                annotation_id: None,
            }))])),
            ..Default::default()
        };
        let (jobs, notes) = edit_jobs(&edit, &[]);
        assert!(jobs.is_empty());
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("new.rs"), "{notes:?}");
    }

    #[test]
    fn a_new_selection_drops_the_previous_preview() {
        let mut p = IntentionPreview::new(Waker::noop());
        p.start((0, "A".into(), 1, 1), Plan::Resolve(Box::new(lsp::CodeAction { title: "A".into(), ..Default::default() })), Vec::new(), |_| None);
        assert!(matches!(p.state, State::Note(_)));
        p.start(
            (1, "B".into(), 1, 1),
            Plan::Edits(
                vec![FileJob { path: PathBuf::from("a.rs"), open: Some(Rope::from_str("a\n")), edits: Edits::Lsp(vec![edit(0, 0, 1, "b")]) }],
                Vec::new(),
            ),
            Vec::new(),
            |_| None,
        );
        assert!(!p.is_for(&(0, "A".into(), 1, 1)));
        let start = std::time::Instant::now();
        while p.busy() && start.elapsed() < std::time::Duration::from_secs(10) {
            p.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        match &p.state {
            State::Ready { files, .. } => assert_eq!(files[0].lines, vec![('@', "1".to_string()), ('-', "a".to_string()), ('+', "b".to_string())]),
            other => panic!("{other:?}"),
        }
        p.clear();
        assert_eq!(p.state, State::Idle);
    }
}
