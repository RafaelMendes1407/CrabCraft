//! The three-pane merge tool (V-08): ours on the left, the result in the middle (editable),
//! theirs on the right. Changed regions are coloured in each pane; a region's side is taken
//! with the arrows beside it or the toolbar, and the result is saved and marked resolved.

use rc_vcs::{ChunkKind, Merge3, Pane, Take};
use std::path::PathBuf;

pub struct MergeTool {
    /// The file, relative to the repository and absolute.
    pub rel: String,
    pub abs: PathBuf,
    pub model: Merge3,
    pub left: rc_editor::EditorState,
    pub center: rc_editor::EditorState,
    pub right: rc_editor::EditorState,
    /// Names of the left (stage 2) and right (stage 3) sides.
    pub names: (String, String),
    /// The selected change.
    pub current: Option<usize>,
    /// Save was pressed with changes left unresolved: ask first.
    pub confirm_save: bool,
    /// The result is being written and staged.
    pub saving: bool,
    pub error: Option<String>,
    /// Write the result with CRLF line endings (as ours had them).
    crlf: bool,
    center_version: u64,
    dark: bool,
    /// Scroll the panes to the current change on the next frame.
    pub reveal: bool,
    /// The changes as they were for each result text seen (hash of the text), so undo and
    /// redo in the result bring their state back too.
    states: Vec<(u64, Vec<rc_vcs::Chunk>)>,
}

/// How many result states are remembered for undo.
const STATES: usize = 200;

fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl MergeTool {
    /// Builds the tool from the three versions; changes only one side made are taken already
    /// (as git does), conflicts start with the base text.
    #[allow(clippy::too_many_arguments)]
    pub fn new(ids: &mut u64, rel: String, abs: PathBuf, base: &str, ours: &str, theirs: &str, names: (String, String), dark: bool) -> MergeTool {
        let crlf = ours.contains("\r\n");
        let norm = |t: &str| t.replace("\r\n", "\n");
        let mut model = Merge3::new(&norm(base), &norm(ours), &norm(theirs));
        model.take_non_conflicting();
        let mut make = |text: &str, read_only: bool| {
            *ids += 1;
            let mut e = rc_editor::EditorState::new(*ids, Some(abs.clone()), text);
            e.read_only = read_only;
            e.parse_blocking();
            e
        };
        let left = make(&model.ours.concat(), true);
        let right = make(&model.theirs.concat(), true);
        let center = make(&model.result_text(), false);
        let center_version = center.buffer.version();
        let current = model.chunks.iter().position(|c| !c.resolved);
        let mut tool = MergeTool { rel, abs, model, left, center, right, names, current, confirm_save: false, saving: false, error: None, crlf, center_version, dark, reveal: true, states: Vec::new() };
        tool.remember();
        tool.paint();
        tool
    }

    /// Follows edits typed into the result.
    pub fn sync(&mut self) {
        let v = self.center.buffer.version();
        if v != self.center_version {
            self.center_version = v;
            let text = self.center.buffer.rope().to_string();
            let h = text_hash(&text);
            match self.states.iter().rev().find(|(k, _)| *k == h) {
                // Undo or redo back to a text seen before: its changes as they were then.
                Some((_, chunks)) => {
                    self.model.chunks = chunks.clone();
                    self.model.result = rc_vcs::merge3::lines(&text);
                }
                None => self.model.edited(&text),
            }
            self.remember();
            self.confirm_save = false;
            self.paint();
        }
    }

    /// Takes a side for one change (an undoable edit of the result).
    pub fn take(&mut self, chunk: usize, take: Take) {
        self.sync();
        let Some(before) = self.model.chunks.get(chunk).map(|c| c.result.clone()) else { return };
        self.model.take(chunk, take);
        let after = self.model.chunks[chunk].result.clone();
        let lines: String = self.model.result.get(after).map(<[String]>::concat).unwrap_or_default();
        let pos = |e: &rc_editor::EditorState, line: usize| if line >= e.buffer.len_lines() { e.buffer.len_chars() } else { e.buffer.pos_at(line, 0) };
        let (start, end) = (pos(&self.center, before.start), pos(&self.center, before.end));
        self.center.replace(start, end, &lines);
        if self.center.buffer.rope() != self.model.result_text().as_str() {
            // Line ends differed (a last line without a newline): put the whole result back.
            let len = self.center.buffer.len_chars();
            self.center.replace(0, len, &self.model.result_text());
        }
        self.center_version = self.center.buffer.version();
        self.remember();
        self.current = Some(chunk);
        self.confirm_save = false;
        self.paint();
    }

    /// Takes the changes only one side made.
    pub fn take_non_conflicting(&mut self) {
        let pending: Vec<(usize, Take)> = self
            .model
            .chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.resolved)
            .filter_map(|(i, c)| match c.kind {
                ChunkKind::Ours | ChunkKind::Same => Some((i, Take::Ours)),
                ChunkKind::Theirs => Some((i, Take::Theirs)),
                ChunkKind::Conflict => None,
            })
            .collect();
        for (i, t) in pending {
            self.take(i, t);
        }
    }

    /// One whole side for the file.
    pub fn take_file(&mut self, theirs: bool) {
        self.sync();
        self.model.take_file(theirs);
        let len = self.center.buffer.len_chars();
        self.center.replace(0, len, &self.model.result_text());
        self.center_version = self.center.buffer.version();
        self.remember();
        self.confirm_save = false;
        self.paint();
    }

    /// Keeps the changes' state for the current result text.
    fn remember(&mut self) {
        let h = text_hash(&self.model.result_text());
        self.states.retain(|(k, _)| *k != h);
        self.states.push((h, self.model.chunks.clone()));
        if self.states.len() > STATES {
            self.states.remove(0);
        }
    }

    /// Selects the next (or previous) change, unresolved ones first.
    pub fn step(&mut self, forward: bool) {
        let n = self.model.chunks.len();
        if n == 0 {
            return;
        }
        let from = self.current.unwrap_or(if forward { n - 1 } else { 0 });
        let order: Vec<usize> = (1..=n).map(|k| if forward { (from + k) % n } else { (from + n * 2 - k) % n }).collect();
        self.current = order.iter().copied().find(|&i| !self.model.chunks[i].resolved).or_else(|| order.first().copied());
        self.reveal = true;
        self.paint();
    }

    /// The text to write: the result, with CRLF when ours used it.
    pub fn output(&self) -> String {
        let text = self.center.buffer.rope().to_string();
        if self.crlf { text.replace('\n', "\r\n") } else { text }
    }

    /// The first line of a change in a pane.
    pub fn line_of(&self, chunk: usize, pane: Pane) -> usize {
        let Some(c) = self.model.chunks.get(chunk) else { return 0 };
        match pane {
            Pane::Ours => c.ours.start,
            Pane::Theirs => c.theirs.start,
            Pane::Result => c.result.start,
        }
    }

    /// Colours the changes in the three panes.
    fn paint(&mut self) {
        let s = rc_kit::theme::scheme(self.dark);
        let p = rc_kit::theme::palette(self.dark);
        let k = if self.dark { 0.55 } else { 0.6 };
        self.left.line_backgrounds.clear();
        self.center.line_backgrounds.clear();
        self.right.line_backgrounds.clear();
        for (i, c) in self.model.chunks.iter().enumerate() {
            let base = match c.kind {
                ChunkKind::Conflict => p.error.gamma_multiply(0.35),
                ChunkKind::Ours | ChunkKind::Theirs => s.modified_lines.gamma_multiply(k),
                ChunkKind::Same => s.added_lines.gamma_multiply(k),
            };
            // Taken changes turn green; conflicts stay red until then.
            let mut color = if c.resolved { s.added_lines.gamma_multiply(k * 0.7) } else { base };
            if self.current == Some(i) {
                color = color.gamma_multiply(1.6);
            }
            let ours_changed = matches!(c.kind, ChunkKind::Ours | ChunkKind::Same | ChunkKind::Conflict);
            let theirs_changed = matches!(c.kind, ChunkKind::Theirs | ChunkKind::Same | ChunkKind::Conflict);
            if ours_changed {
                c.ours.clone().for_each(|l| {
                    self.left.line_backgrounds.insert(l, color);
                });
            }
            if theirs_changed {
                c.theirs.clone().for_each(|l| {
                    self.right.line_backgrounds.insert(l, color);
                });
            }
            c.result.clone().for_each(|l| {
                self.center.line_backgrounds.insert(l, color);
            });
        }
    }
}
