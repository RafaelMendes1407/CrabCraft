//! Refactoring preview: the edits a refactoring (a symbol rename) would make, listed with each
//! line before and after, so the user can leave some out before applying the rest.
//!
//! The lines of closed files are read on a thread, so a rename over many files does not stall
//! the window.

use crate::file_rename::Change;
use rc_core::{Inbox, Waker};
use rc_lsp::lsp_types as lsp;
use rc_text::Rope;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct RefactorPreview {
    pub title: String,
    pub changes: Vec<Change>,
    /// Applying failed: shown in the dialog.
    pub error: Option<String>,
    loading: Option<Inbox<Vec<Change>>>,
}

impl RefactorPreview {
    /// Starts listing the edits of `edit`; `open` are the open documents' texts.
    pub fn start(title: String, edit: &lsp::WorkspaceEdit, open: Vec<(PathBuf, Rope)>, waker: Waker) -> RefactorPreview {
        let files = crate::insight::workspace_edit_files(edit);
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let spawned = std::thread::Builder::new().name("rc-refactor-preview".into()).spawn(move || out.send(changes_of(files, &open)));
        RefactorPreview { title, changes: Vec::new(), error: spawned.err().map(|e| e.to_string()), loading: Some(inbox) }
    }

    pub fn loading(&self) -> bool {
        self.loading.is_some()
    }

    pub fn update(&mut self) {
        if let Some(found) = self.loading.as_ref().and_then(|i| i.drain().pop()) {
            self.changes = found;
            self.loading = None;
        }
    }

    /// The checked edits, per file.
    pub fn checked(&self) -> BTreeMap<PathBuf, Vec<lsp::TextEdit>> {
        let mut out: BTreeMap<PathBuf, Vec<lsp::TextEdit>> = BTreeMap::new();
        for c in self.changes.iter().filter(|c| c.checked) {
            out.entry(c.path.clone()).or_default().push(c.edit.clone());
        }
        out
    }
}

/// One change per edit, in file and line order. A file that cannot be read is left out.
fn changes_of(files: BTreeMap<PathBuf, Vec<lsp::TextEdit>>, open: &[(PathBuf, Rope)]) -> Vec<Change> {
    let mut out = Vec::new();
    for (path, edits) in files {
        let rope = match open.iter().find(|(p, _)| *p == path) {
            Some((_, rope)) => rope.clone(),
            None => match crate::docs::read_text(&path) {
                Ok((text, _, _)) => Rope::from_str(&text),
                Err(_) => continue,
            },
        };
        let mut list: Vec<Change> = edits.into_iter().map(|e| crate::file_rename::change(&rope, &path, e, true)).collect();
        list.sort_by_key(|c| (c.line, c.edit.range.start.character));
        out.extend(list);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(line: u32, from: u32, to: u32, text: &str) -> lsp::TextEdit {
        lsp::TextEdit { range: lsp::Range::new(lsp::Position::new(line, from), lsp::Position::new(line, to)), new_text: text.into() }
    }

    #[test]
    fn edits_are_listed_per_file_with_their_lines() {
        let dir = tempfile::tempdir().unwrap();
        let closed = dir.path().join("b.rs");
        std::fs::write(&closed, "use crate::a::old;\nfn f() { old() }\n").unwrap();
        let open = dir.path().join("a.rs");
        let mut files = BTreeMap::new();
        files.insert(closed.clone(), vec![edit(1, 9, 12, "new"), edit(0, 14, 17, "new")]);
        files.insert(open.clone(), vec![edit(0, 7, 10, "new")]);
        files.insert(dir.path().join("missing.rs"), vec![edit(0, 0, 1, "x")]);
        let list = changes_of(files, &[(open.clone(), Rope::from_str("pub fn old() {}\n"))]);
        let shown: Vec<(String, usize, &str, &str)> =
            list.iter().map(|c| (c.path.file_name().unwrap().to_string_lossy().into_owned(), c.line, c.before.as_str(), c.after.as_str())).collect();
        assert_eq!(
            shown,
            [
                ("a.rs".to_string(), 0, "pub fn old() {}", "pub fn new() {}"),
                ("b.rs".to_string(), 0, "use crate::a::old;", "use crate::a::new;"),
                ("b.rs".to_string(), 1, "fn f() { old() }", "fn f() { new() }"),
            ]
        );
        assert!(list.iter().all(|c| c.checked));
    }
}
