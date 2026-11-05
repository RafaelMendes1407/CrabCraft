//! Open documents: one editor per file, opened from disk, saved back with the file's line
//! endings and BOM, reloaded when changed outside, and parsed in the background. Saved and
//! reloaded versions go to the local history.

use rc_editor::EditorState;
use rc_kit::i18n::trf;
use rc_syntax::ParserPool;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Files bigger than this open read-only (no syntax tree).
const LARGE_FILE: u64 = 8 * 1024 * 1024;
/// Files bigger than this do not open at all.
const HUGE_FILE: u64 = 64 * 1024 * 1024;

pub struct Doc {
    pub editor: EditorState,
    pub path: PathBuf,
    pub mtime: Option<SystemTime>,
    pub bom: bool,
    pub pinned: bool,
    /// Opened with a single click and not edited yet: the next single-click open replaces it
    /// (title in italics, like Rider and VS Code).
    pub preview: bool,
    /// The file changed on disk while it had unsaved edits.
    pub conflict: bool,
    /// Markdown files: editor, preview or both side by side.
    pub md_view: Option<MdView>,
    /// The text as last loaded or saved (an O(1) copy): the version the local history keeps
    /// when the file is saved or reloaded.
    pub saved: rc_text::Rope,
    /// A scratch file (outside every project): rust-analyzer is not asked about it.
    pub scratch: bool,
    /// Written by the IDE (a macro expansion): read-only and never sent to the language server.
    pub generated: bool,
    /// What the project's `.editorconfig` and `rustfmt.toml` say about this file.
    pub style: rc_core::code_style::CodeStyle,
    /// Opened with no project (LightEdit): the Cargo project the file belongs to, if any.
    pub light_project: Option<PathBuf>,
}

/// How a Markdown file is shown (Rider's editor / editor and preview / preview switch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MdView {
    Editor,
    Split,
    Preview,
}

impl Doc {
    pub fn name(&self) -> String {
        self.path.file_name().map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }

    pub fn is_modified(&self) -> bool {
        self.editor.buffer.is_modified()
    }
}

pub struct Docs {
    pub list: Vec<Doc>,
    pub active: Option<usize>,
    next_id: u64,
    pub pool: ParserPool,
    /// Bookmarks of files that are not open.
    pub closed_bookmarks: BTreeMap<PathBuf, BTreeSet<usize>>,
    /// Breakpoints of files that are not open (D-27): they stay set, and come back with the file.
    pub closed_breakpoints: BTreeMap<PathBuf, BTreeMap<usize, rc_editor::BreakpointKind>>,
    /// Recently opened files, most recent first (⌘E).
    pub recent: Vec<PathBuf>,
    /// Folders whose files open read-only: the dependencies Cargo downloaded.
    pub read_only_roots: Vec<PathBuf>,
    /// Where saved and reloaded versions go (none in unit tests).
    pub history: Option<crate::local_history::HistoryService>,
    /// The open project: code style files are read up to it.
    pub style_root: Option<PathBuf>,
}

/// The folder of the Cargo project `file` belongs to: the outermost `Cargo.toml` above it that
/// declares a `[workspace]`, or else the nearest one.
pub fn cargo_project_of(file: &Path) -> Option<PathBuf> {
    let mut nearest = None;
    let mut workspace = None;
    for dir in file.ancestors().skip(1) {
        let manifest = dir.join("Cargo.toml");
        let Ok(meta) = std::fs::metadata(&manifest) else { continue };
        if !meta.is_file() || meta.len() > 1 << 20 {
            continue;
        }
        nearest.get_or_insert_with(|| dir.to_path_buf());
        if std::fs::read_to_string(&manifest).is_ok_and(|t| t.lines().any(|l| l.trim() == "[workspace]")) {
            workspace = Some(dir.to_path_buf());
        }
    }
    workspace.or(nearest)
}

/// The indentation and right margin of the project's style; detection decides what it leaves
/// unset.
pub fn apply_style(editor: &mut EditorState, style: &rc_core::code_style::CodeStyle) {
    if let Some((unit, tab_width)) = style.indent() {
        editor.indent = rc_text::commands::Indent { unit, tab_width };
    }
    editor.right_margin = style.max_line_length;
    if let Some(eol) = style.end_of_line
        && editor.buffer.len_chars() == 0
    {
        // A new file gets the project's line ending; existing ones keep theirs.
        editor.buffer.set_line_ending(match eol {
            rc_core::code_style::EndOfLine::Lf => rc_text::LineEnding::Lf,
            rc_core::code_style::EndOfLine::CrLf => rc_text::LineEnding::CrLf,
        });
    }
}

/// What saving changes for the style: trailing whitespace goes (except on the lines with a
/// caret, so typing is not disturbed) and the file ends with exactly one newline.
pub fn style_fixes(editor: &EditorState, style: &rc_core::code_style::CodeStyle) -> Vec<rc_text::Change> {
    let rope = editor.buffer.rope();
    let mut edits = Vec::new();
    if style.trim_trailing_whitespace == Some(true) {
        let caret_lines: BTreeSet<usize> = editor.selection.ranges().iter().map(|r| rope.char_to_line(r.head)).collect();
        for line in 0..rope.len_lines() {
            if caret_lines.contains(&line) {
                continue;
            }
            let text = rope.line(line);
            let len = rc_text::buffer::line_len(rope, line);
            let content = text.chars().take(len);
            let trailing = content.collect::<Vec<char>>().iter().rev().take_while(|c| **c == ' ' || **c == '\t').count();
            if trailing > 0 {
                let end = rope.line_to_char(line) + len;
                edits.push(rc_text::Change::new(end - trailing, end, String::new()));
            }
        }
    }
    let len = rope.len_chars();
    match style.insert_final_newline {
        Some(true) if len > 0 && rope.char(len - 1) != '\n' => edits.push(rc_text::Change::new(len, len, "\n".to_string())),
        Some(false) if len > 0 && rope.char(len - 1) == '\n' => {
            let keep = len - rope.chars_at(len).reversed().take_while(|c| *c == '\n' || *c == '\r').count();
            edits.push(rc_text::Change::new(keep, len, String::new()));
        }
        _ => {}
    }
    edits
}

/// Reads a text file: (text without BOM, had BOM). Binary files are refused.
pub fn read_text(path: &Path) -> Result<(String, bool, Option<SystemTime>), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > HUGE_FILE {
        return Err(trf("The file is too large to open ({size} MB)", &[("size", &(meta.len() / 1024 / 1024))]));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let (bytes, bom) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (rest.to_vec(), true),
        None => (bytes, false),
    };
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return Err(rc_kit::i18n::tr("This is a binary file and cannot be edited").to_string());
    }
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    Ok((text, bom, meta.modified().ok()))
}

impl Docs {
    pub fn new(pool: ParserPool) -> Self {
        Docs { list: Vec::new(), active: None, next_id: 1, pool, closed_bookmarks: BTreeMap::new(), closed_breakpoints: BTreeMap::new(), recent: Vec::new(), read_only_roots: Vec::new(), history: None, style_root: None }
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.list.iter().position(|d| d.path == path)
    }

    pub fn active_doc(&self) -> Option<&Doc> {
        self.active.and_then(|i| self.list.get(i))
    }

    pub fn active_doc_mut(&mut self) -> Option<&mut Doc> {
        self.active.and_then(|i| self.list.get_mut(i))
    }

    /// Opens `path` (or switches to it) and makes it active.
    pub fn open(&mut self, path: &Path) -> Result<usize, String> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.index_of(&path) {
            self.activate(i);
            return Ok(i);
        }
        let (text, bom, mtime) = read_text(&path)?;
        let id = self.next_id;
        self.next_id += 1;
        let mut editor = EditorState::new(id, Some(path.clone()), &text);
        let large = std::fs::metadata(&path).map(|m| m.len() > LARGE_FILE).unwrap_or(false);
        if large {
            editor.read_only = true;
        } else if text.len() < 64 * 1024 {
            // Small files get their colours on the first frame.
            editor.parse_blocking();
        } else {
            editor.request_parse(&self.pool);
        }
        // Sources Roslyn decompiled from an assembly (⌘+click on a framework type) are read-only.
        if path.components().any(|c| c.as_os_str() == "MetadataAsSource") || self.read_only_roots.iter().any(|r| path.starts_with(r)) {
            editor.read_only = true;
        }
        let md_view = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown")).then_some(MdView::Split);
        editor.bookmarks = self.closed_bookmarks.remove(&path).unwrap_or_default();
        editor.breakpoints = self.closed_breakpoints.remove(&path).unwrap_or_default();
        let saved = editor.buffer.snapshot();
        let scratch = rc_core::scratch::is_in(&rc_core::paths::scratches_dir(), &path);
        // Only files of the project follow its style (scratches and dependencies do not).
        let style = match &self.style_root {
            Some(root) if !scratch && path.starts_with(root) => rc_core::code_style::for_file(&path, Some(root)),
            _ => Default::default(),
        };
        apply_style(&mut editor, &style);
        let light_project = if self.style_root.is_none() && !scratch { cargo_project_of(&path) } else { None };
        self.list.push(Doc { editor, path: path.clone(), mtime, bom, pinned: false, preview: false, conflict: false, md_view, scratch, generated: false, saved, style, light_project });
        let i = self.list.len() - 1;
        self.activate(i);
        Ok(i)
    }

    /// Single-click open: shows `path` in the preview tab, replacing the previous preview.
    pub fn open_preview(&mut self, path: &Path) -> Result<usize, String> {
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.index_of(&canon) {
            self.activate(i);
            return Ok(i);
        }
        let old = self.list.iter().position(|d| d.preview && !d.is_modified());
        let i = self.open(path)?;
        self.list[i].preview = true;
        match old {
            Some(o) => {
                let doc = self.list.remove(i);
                let replaced = std::mem::replace(&mut self.list[o], doc);
                self.keep_bookmarks(&replaced);
                self.activate(o);
                Ok(o)
            }
            None => Ok(i),
        }
    }

    pub fn activate(&mut self, i: usize) {
        if i < self.list.len() {
            self.active = Some(i);
            let p = self.list[i].path.clone();
            self.recent.retain(|r| r != &p);
            self.recent.insert(0, p);
            self.recent.truncate(50);
        }
    }

    fn keep_bookmarks(&mut self, doc: &Doc) {
        if !doc.editor.bookmarks.is_empty() {
            self.closed_bookmarks.insert(doc.path.clone(), doc.editor.bookmarks.clone());
        }
        if !doc.editor.breakpoints.is_empty() {
            // What the debugger said of them (verified or not) no longer holds once closed.
            let kept = doc.editor.breakpoints.iter().map(|(l, k)| (*l, settled(*k))).collect();
            self.closed_breakpoints.insert(doc.path.clone(), kept);
        }
    }

    /// Every breakpoint, open file or not, by file.
    pub fn all_breakpoints(&self) -> BTreeMap<PathBuf, BTreeMap<usize, rc_editor::BreakpointKind>> {
        let mut all = self.closed_breakpoints.clone();
        all.extend(self.list.iter().filter(|d| !d.editor.breakpoints.is_empty()).map(|d| (d.path.clone(), d.editor.breakpoints.clone())));
        all
    }

    /// The breakpoints of `path`, in its editor or kept for the closed file.
    pub fn breakpoints_mut(&mut self, path: &Path) -> Option<&mut BTreeMap<usize, rc_editor::BreakpointKind>> {
        match self.index_of(path) {
            Some(i) => Some(&mut self.list[i].editor.breakpoints),
            None => self.closed_breakpoints.get_mut(path),
        }
    }

    /// Removes a breakpoint, in a closed file too.
    pub fn remove_breakpoint(&mut self, path: &Path, line: usize) {
        if let Some(bps) = self.breakpoints_mut(path) {
            bps.remove(&line);
        }
        if self.closed_breakpoints.get(path).is_some_and(BTreeMap::is_empty) {
            self.closed_breakpoints.remove(path);
        }
    }

    /// Every bookmark, open file or not, by file.
    pub fn bookmarks(&self) -> BTreeMap<PathBuf, BTreeSet<usize>> {
        let mut all = self.closed_bookmarks.clone();
        all.extend(self.list.iter().filter(|d| !d.editor.bookmarks.is_empty()).map(|d| (d.path.clone(), d.editor.bookmarks.clone())));
        all
    }

    /// Removes a bookmark, in a closed file too.
    pub fn remove_bookmark(&mut self, path: &Path, line: usize) {
        if let Some(i) = self.index_of(path) {
            self.list[i].editor.bookmarks.remove(&line);
        } else if let Some(lines) = self.closed_bookmarks.get_mut(path) {
            lines.remove(&line);
            if lines.is_empty() {
                self.closed_bookmarks.remove(path);
            }
        }
    }

    /// Closes tab `i`; the tab to its left (or the new first) becomes active, like Rider.
    pub fn close(&mut self, i: usize) -> Option<Doc> {
        if i >= self.list.len() {
            return None;
        }
        let doc = self.list.remove(i);
        self.keep_bookmarks(&doc);
        self.active = match self.active {
            _ if self.list.is_empty() => None,
            Some(a) if a > i => Some(a - 1),
            Some(a) if a == i => Some(i.saturating_sub(1).min(self.list.len() - 1)),
            other => other,
        };
        Some(doc)
    }

    pub fn save(&mut self, i: usize) -> Result<(), String> {
        let doc = &mut self.list[i];
        if !doc.editor.read_only {
            let edits = style_fixes(&doc.editor, &doc.style);
            if !edits.is_empty() {
                doc.editor.apply_text_edits(edits);
            }
        }
        let mut bytes = Vec::new();
        if doc.bom {
            bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        bytes.extend_from_slice(doc.editor.buffer.to_disk_string().as_bytes());
        rc_core::paths::write_atomic(&doc.path, &bytes).map_err(|e| trf("Could not save {file}: {error}", &[("file", &doc.name()), ("error", &e)]))?;
        doc.editor.buffer.mark_saved();
        doc.mtime = std::fs::metadata(&doc.path).and_then(|m| m.modified()).ok();
        doc.conflict = false;
        // The version on disk before this save, then the saved one (equal ones are skipped).
        let previous = std::mem::replace(&mut doc.saved, doc.editor.buffer.snapshot());
        if let Some(h) = &self.history {
            h.record(&doc.path, previous);
            h.record(&doc.path, doc.saved.clone());
        }
        Ok(())
    }

    /// Keeps the current text of document `i` (saved or not) in the local history: before it is
    /// replaced by the version on disk.
    pub fn remember(&self, i: usize) {
        if let (Some(h), Some(doc)) = (&self.history, self.list.get(i)) {
            h.record(&doc.path, doc.editor.buffer.snapshot());
        }
    }

    /// Document `i` now shows `text`, loaded from disk.
    pub fn reloaded(&mut self, i: usize, text: &str, bom: bool, mtime: Option<SystemTime>) {
        let Some(doc) = self.list.get_mut(i) else { return };
        let previous = doc.saved.clone();
        doc.editor.reload(text);
        doc.bom = bom;
        doc.mtime = mtime;
        doc.conflict = false;
        doc.saved = doc.editor.buffer.snapshot();
        if let Some(h) = &self.history {
            h.record(&doc.path, previous);
            h.record(&doc.path, doc.saved.clone());
        }
    }

    /// Saves every modified document; returns the errors.
    pub fn save_all(&mut self) -> Vec<String> {
        let modified: Vec<usize> = (0..self.list.len()).filter(|&i| self.list[i].is_modified()).collect();
        modified.into_iter().filter_map(|i| self.save(i).err()).collect()
    }

    /// A file changed on disk: unmodified documents reload, modified ones are flagged.
    pub fn on_disk_change(&mut self, path: &Path) {
        let Some(i) = self.index_of(path) else { return };
        let doc = &mut self.list[i];
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if mtime.is_some() && mtime == doc.mtime {
            return; // our own save
        }
        let Ok((text, bom, mtime)) = read_text(path) else { return };
        if *doc.editor.buffer.rope() == text.replace("\r\n", "\n").replace('\r', "\n") {
            // A BOM or line-ending-only external edit still changes how the next save must
            // be encoded. Do not silently rewrite it with the old document's encoding.
            if !doc.is_modified() {
                doc.bom = bom;
                doc.editor.buffer.set_line_ending(rc_text::LineEnding::detect(&text));
                doc.editor.buffer.mark_saved();
            }
            doc.mtime = mtime;
            return;
        }
        if doc.is_modified() {
            doc.conflict = true;
            // Keep the version on disk too: the user may keep theirs and overwrite it.
            if let Some(h) = &self.history {
                h.record(path, rc_text::Rope::from_str(&text.replace("\r\n", "\n")));
            }
            return;
        }
        self.reloaded(i, &text, bom, mtime);
        self.list[i].editor.request_parse(&self.pool);
    }

    /// Reads every document from disk again (the unmodified ones change; the modified ones are
    /// flagged when the file differs) and parses them all afresh.
    pub fn reload_all(&mut self) {
        let paths: Vec<PathBuf> = self.list.iter().map(|d| d.path.clone()).collect();
        for path in paths {
            if let Some(i) = self.index_of(&path) {
                self.list[i].mtime = None;
            }
            self.on_disk_change(&path);
        }
        for d in &mut self.list {
            d.editor.request_parse(&self.pool);
        }
    }

    /// Takes finished background parses; returns whether any arrived (repaint).
    pub fn poll_parses(&mut self) -> bool {
        let results = self.pool.poll();
        let any = !results.is_empty();
        for r in results {
            if let Some(doc) = self.list.iter_mut().find(|d| d.editor.id == r.doc) {
                doc.editor.syntax.accept(r);
            }
        }
        any
    }

    /// Queues reparses for documents edited this frame.
    pub fn reparse_edited(&mut self) {
        for d in &mut self.list {
            if d.editor.syntax.lang != rc_syntax::Lang::Plain && !d.editor.syntax.is_current() && !d.editor.read_only {
                let pending = d.editor.syntax.tree().is_none() || d.editor.buffer.version() > 0;
                if pending {
                    d.editor.request_parse(&self.pool);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn open_save_keeps_bom_and_crlf_and_closing_activates_the_left_tab() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A.cs");
        let b = dir.path().join("B.cs");
        std::fs::write(&a, b"\xEF\xBB\xBFclass A {}\r\nclass A2 {}\r\n").unwrap();
        std::fs::write(&b, "class B {}\n").unwrap();
        let mut docs = Docs::new(ParserPool::new(Arc::new(|| {})));
        let ia = docs.open(&a).unwrap();
        let ib = docs.open(&b).unwrap();
        assert_eq!((ia, ib, docs.active), (0, 1, Some(1)));
        assert_eq!(docs.open(&a).unwrap(), 0);
        let d = &mut docs.list[0];
        d.editor.replace(0, 0, "// x\n");
        assert!(d.is_modified());
        docs.save(0).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"\xEF\xBB\xBF// x\r\nclass A {}\r\nclass A2 {}\r\n");
        assert!(!docs.list[0].is_modified());
        docs.active = Some(1);
        docs.close(1);
        assert_eq!(docs.active, Some(0));
    }

    #[test]
    fn binary_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.dll");
        std::fs::write(&p, [0u8, 1, 2, 3]).unwrap();
        assert!(read_text(&p).is_err());
    }

    #[test]
    fn preview_tabs_are_replaced_until_kept() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["a.cs", "b.cs", "c.cs", "d.cs"] {
            std::fs::write(dir.path().join(f), "class X {}\n").unwrap();
        }
        let mut docs = Docs::new(ParserPool::new(Arc::new(|| {})));
        docs.open(&dir.path().join("a.cs")).unwrap();
        docs.open_preview(&dir.path().join("b.cs")).unwrap();
        assert!(docs.list[1].preview);
        // The next single-click open replaces the preview in place.
        docs.open_preview(&dir.path().join("c.cs")).unwrap();
        assert_eq!(docs.list.iter().map(|d| d.name()).collect::<Vec<_>>(), ["a.cs", "c.cs"]);
        assert_eq!(docs.active, Some(1));
        // Kept (edited or double-clicked): the next preview opens a new tab.
        docs.list[1].preview = false;
        docs.open_preview(&dir.path().join("d.cs")).unwrap();
        assert_eq!(docs.list.len(), 3);
        assert_eq!(docs.active_doc().unwrap().name(), "d.cs");
    }
}

/// A breakpoint kind without what a debug session added (verified or not bound).
pub fn settled(kind: rc_editor::BreakpointKind) -> rc_editor::BreakpointKind {
    match kind {
        rc_editor::BreakpointKind::Verified | rc_editor::BreakpointKind::Invalid => rc_editor::BreakpointKind::Normal,
        k => k,
    }
}
