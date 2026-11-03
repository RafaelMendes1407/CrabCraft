//! Renaming a file or folder (F2 in the Explorer, or a tab's menu). Before anything changes,
//! the language server is asked for the edits the rename needs (`workspace/willRenameFiles`:
//! rust-analyzer renames `mod util;` and the `util::` paths) and the project is searched for
//! the old name as plain text (`#[path]`, `include_str!`, `Cargo.toml`, documentation). The
//! user picks which changes to make; the rename and the picked edits are applied together.

use crate::app::CrabCraftApp;
use crate::convert;
use rc_core::{Inbox, Waker};
use rc_kit::i18n::tr;
use rc_lsp::Pending;
use rc_lsp::lsp_types as lsp;
use rc_search::{FindEvent, FindOptions, QueryOptions};
use rc_text::Rope;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Text matches past this many are not listed.
const MAX_TEXT_MATCHES: usize = 500;

/// One change in one file, shown as its line before and after.
#[derive(Clone, Debug)]
pub struct Change {
    pub path: PathBuf,
    /// 0-based line where the change starts.
    pub line: usize,
    pub before: String,
    pub after: String,
    pub edit: lsp::TextEdit,
    /// A code reference from the language server, not a text match.
    pub from_server: bool,
    pub checked: bool,
}

/// A rename waiting for the user: the changes it would make, found in the background.
pub struct FileRename {
    pub from: PathBuf,
    pub to: PathBuf,
    pub changes: Vec<Change>,
    /// A language server was asked for the code references.
    pub asked_server: bool,
    /// Why the server gave no edits (the text matches are still listed).
    pub server_error: Option<String>,
    /// Applying failed: shown in the dialog.
    pub error: Option<String>,
    root: Option<PathBuf>,
    server: Option<Pending<Option<lsp::WorkspaceEdit>>>,
    edit: Option<lsp::WorkspaceEdit>,
    search: Option<(Arc<AtomicBool>, Inbox<Vec<Change>>)>,
    waker: Waker,
    done: bool,
}

impl Drop for FileRename {
    fn drop(&mut self) {
        if let Some((cancel, _)) = &self.search {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl FileRename {
    /// Starts looking for what renaming `from` to `to` changes: asks the server now, then
    /// searches the text once it answered.
    pub fn start(app: &CrabCraftApp, from: PathBuf, to: PathBuf) -> FileRename {
        let server = app.insight.will_rename_files(&from, &to);
        FileRename {
            asked_server: server.is_some(),
            from,
            to,
            changes: Vec::new(),
            server_error: None,
            error: None,
            root: app.workspace.as_ref().map(|w| w.ws.root.clone()),
            server,
            edit: None,
            search: None,
            waker: app.waker.clone(),
            done: false,
        }
    }

    pub fn searching(&self) -> bool {
        !self.done
    }

    /// Takes the server's answer and the search results; never blocks.
    pub fn update(&mut self, docs: &crate::docs::Docs) {
        if self.done {
            return;
        }
        if let Some(p) = &mut self.server {
            match p.poll() {
                None => return,
                Some(Ok(edit)) => self.edit = edit,
                Some(Err(e)) => self.server_error = Some(e.message),
            }
            self.server = None;
        }
        if self.search.is_none() {
            let cancel = Arc::new(AtomicBool::new(false));
            let inbox = Inbox::new(self.waker.clone());
            let job = Job {
                from: self.from.clone(),
                to: self.to.clone(),
                edit: self.edit.take(),
                root: self.root.clone(),
                open: docs.list.iter().map(|d| (d.path.clone(), d.editor.buffer.rope().clone())).collect(),
            };
            let (flag, out) = (cancel.clone(), inbox.sender());
            let started = std::thread::Builder::new().name("rc-file-rename".into()).spawn(move || {
                let found = collect(job, &flag);
                if !flag.load(Ordering::Relaxed) {
                    out.send(found);
                }
            });
            if started.is_err() {
                self.done = true;
                return;
            }
            self.search = Some((cancel, inbox));
            return;
        }
        if let Some((_, inbox)) = &self.search
            && let Some(found) = inbox.drain().pop()
        {
            self.changes = found;
            self.search = None;
            self.done = true;
        }
    }
}

struct Job {
    from: PathBuf,
    to: PathBuf,
    edit: Option<lsp::WorkspaceEdit>,
    root: Option<PathBuf>,
    open: Vec<(PathBuf, Rope)>,
}

/// The server's edits and the text matches of the old name, with their lines.
fn collect(job: Job, cancel: &AtomicBool) -> Vec<Change> {
    let mut texts: BTreeMap<PathBuf, Option<Rope>> = BTreeMap::new();
    let mut text_of = |path: &Path| -> Option<Rope> {
        texts
            .entry(path.to_path_buf())
            .or_insert_with(|| match job.open.iter().find(|(p, _)| p == path) {
                Some((_, rope)) => Some(rope.clone()),
                None => crate::docs::read_text(path).ok().map(|(t, _, _)| Rope::from_str(&t.replace("\r\n", "\n"))),
            })
            .clone()
    };
    let mut out = Vec::new();
    if let Some(edit) = &job.edit {
        for (path, edits) in crate::insight::workspace_edit_files(edit) {
            let Some(rope) = text_of(&path) else { continue };
            for e in edits {
                out.push(change(&rope, &path, e, true));
            }
        }
    }
    let Some((old, new)) = mention(&job.from, &job.to) else { return out };
    let Some(root) = &job.root else { return out };
    let inbox = Inbox::new(Waker::noop());
    let options = FindOptions { query: QueryOptions { pattern: old.clone(), case_sensitive: true, ..Default::default() }, max_results: MAX_TEXT_MATCHES, ..Default::default() };
    let Ok(handle) = rc_search::find_in_files(root, options, inbox.sender()) else { return out };
    while !handle.is_finished() {
        if cancel.load(Ordering::Relaxed) {
            handle.cancel();
            return Vec::new();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut files: Vec<PathBuf> = inbox
        .drain()
        .into_iter()
        .filter_map(|e| match e {
            FindEvent::File { file, .. } => Some(file.path),
            FindEvent::Done { .. } => None,
        })
        .collect();
    // Unsaved text of open files counts too.
    for (path, rope) in &job.open {
        if path.starts_with(root) && !files.contains(path) && rope.to_string().contains(&old) {
            files.push(path.clone());
        }
    }
    files.sort();
    files.dedup();
    let mut found = 0;
    for path in files {
        if cancel.load(Ordering::Relaxed) || found >= MAX_TEXT_MATCHES {
            break;
        }
        let Some(rope) = text_of(&path) else { continue };
        let taken: Vec<(usize, usize)> = out.iter().filter(|c| c.path == path).map(|c| convert::range_chars(&rope, &c.edit.range)).collect();
        for (a, b) in occurrences(&rope.to_string(), &old) {
            if taken.iter().any(|&(x, y)| a < y.max(x + 1) && x < b) {
                continue;
            }
            let edit = lsp::TextEdit { range: lsp::Range { start: convert::lsp_position(&rope, a), end: convert::lsp_position(&rope, b) }, new_text: new.clone() };
            out.push(change(&rope, &path, edit, false));
            found += 1;
        }
    }
    out
}

/// The text that names the renamed item and its replacement: the file name (`util.rs`), or
/// a folder's name followed by `/`.
fn mention(from: &Path, to: &Path) -> Option<(String, String)> {
    let old = from.file_name()?.to_string_lossy().into_owned();
    let new = to.file_name()?.to_string_lossy().into_owned();
    if old.is_empty() || old == new {
        return None;
    }
    if from.is_dir() || to.is_dir() { Some((format!("{old}/"), format!("{new}/"))) } else { Some((old, new)) }
}

/// Char ranges of `needle` in `text` that are not part of a longer name.
fn occurrences(text: &str, needle: &str) -> Vec<(usize, usize)> {
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let n = needle.chars().count();
    let mut out = Vec::new();
    for (byte, _) in text.match_indices(needle) {
        let before = text[..byte].chars().next_back();
        let after = text[byte + needle.len()..].chars().next();
        if before.is_some_and(word) || needle.chars().next_back().is_some_and(word) && after.is_some_and(word) {
            continue;
        }
        let a = text[..byte].chars().count();
        out.push((a, a + n));
    }
    out
}

/// The change of `edit`: its lines before and after.
fn change(rope: &Rope, path: &Path, edit: lsp::TextEdit, from_server: bool) -> Change {
    let (a, b) = convert::range_chars(rope, &edit.range);
    let (a, b) = (a.min(b), a.max(b));
    let line = rope.char_to_line(a);
    let start = rope.line_to_char(line);
    let last = rope.char_to_line(b);
    let end = (rope.line_to_char(last) + rc_text::buffer::line_len(rope, last)).max(b);
    let before = rope.slice(start..end).to_string();
    let after = format!("{}{}{}", rope.slice(start..a), edit.new_text.replace("\r\n", "\n"), rope.slice(b..end));
    Change { path: path.to_path_buf(), line, before, after, edit, from_server, checked: from_server }
}

/// Renames `from` to `to` on disk, then applies `changes` (open files in their editor as one
/// undo step, closed files on disk). Returns the number of files edited; fails only when the
/// rename itself fails, with nothing changed.
pub fn apply(app: &mut CrabCraftApp, from: &Path, to: &Path, changes: &[Change]) -> Result<usize, String> {
    if to.exists() {
        return Err(tr("A file or folder with this name already exists").to_string());
    }
    let was_file = from.is_file();
    std::fs::rename(from, to).map_err(|e| e.to_string())?;
    let moved = |p: &Path| -> PathBuf {
        match p.strip_prefix(from) {
            Ok(rest) if rest.as_os_str().is_empty() => to.to_path_buf(),
            Ok(rest) => to.join(rest),
            Err(_) => p.to_path_buf(),
        }
    };
    // Open editors and remembered places follow the file.
    for d in &mut app.docs.list {
        let p = moved(&d.path);
        if p != d.path {
            d.path = p;
            d.editor.path = Some(d.path.clone());
        }
    }
    for p in &mut app.docs.recent {
        *p = moved(p);
    }
    app.docs.closed_bookmarks = std::mem::take(&mut app.docs.closed_bookmarks).into_iter().map(|(p, b)| (moved(&p), b)).collect();
    for (p, _) in app.nav_back.iter_mut().chain(app.nav_forward.iter_mut()) {
        *p = moved(p);
    }
    app.explorer.invalidate(from);
    app.explorer.invalidate(to);
    app.session_dirty = true;
    app.insight.did_rename_files(from, to);
    if was_file && from.extension().is_some_and(|x| x == "rs") {
        app.insight.files_changed(&[(from.to_path_buf(), 3), (to.to_path_buf(), 1)]);
    }
    let mut per_file: BTreeMap<PathBuf, Vec<lsp::TextEdit>> = BTreeMap::new();
    for c in changes.iter().filter(|c| c.checked) {
        per_file.entry(moved(&c.path)).or_default().push(c.edit.clone());
    }
    // Closed files are written on disk: the server reloads them.
    let closed: Vec<(PathBuf, u8)> = per_file.keys().filter(|p| app.docs.index_of(p).is_none()).map(|p| (p.clone(), 2)).collect();
    let n = crate::insight::apply_file_edits(&mut app.docs, per_file);
    if !closed.is_empty() {
        app.insight.files_changed(&closed);
    }
    // The file is renamed already: a failed edit is reported, not retried.
    Ok(n.unwrap_or_else(|e| {
        app.notify(e, true);
        0
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_skip_longer_names() {
        let text = "a util.rs b myutil.rs c util.rsx d src/util.rs";
        let found = occurrences(text, "util.rs");
        let words: Vec<&str> = found.iter().map(|&(a, b)| &text[a..b]).collect();
        assert_eq!(words, ["util.rs", "util.rs"]);
        assert_eq!(found[1].0, text.find("src/util.rs").unwrap() + 4);
        // A folder's mention ends with `/`: anything may follow.
        assert_eq!(occurrences("src/util/mod.rs", "util/").len(), 1);
    }

    #[test]
    fn a_change_shows_its_line_before_and_after() {
        let rope = Rope::from_str("mod util;\npub use util::helper;\n");
        let edit = lsp::TextEdit { range: lsp::Range { start: lsp::Position { line: 1, character: 8 }, end: lsp::Position { line: 1, character: 12 } }, new_text: "tools".into() };
        let c = change(&rope, Path::new("lib.rs"), edit, true);
        assert_eq!((c.line, c.before.as_str(), c.after.as_str(), c.checked), (1, "pub use util::helper;", "pub use tools::helper;", true));
        // Out of range positions from a server are clamped.
        let edit = lsp::TextEdit { range: lsp::Range { start: lsp::Position { line: 9, character: 99 }, end: lsp::Position { line: 0, character: 0 } }, new_text: "x".into() };
        let c = change(&rope, Path::new("lib.rs"), edit, false);
        assert!(!c.checked);
    }
}
