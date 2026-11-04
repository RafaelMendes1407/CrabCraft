//! Three-way merge of a conflicted file for the merge tool (V-08): the changed regions
//! ("chunks") of ours and theirs against the base, and a result being built from them that
//! the user also edits freely. Pure line logic, no UI and no git.

use crate::textdiff::{Change, diff};
use std::ops::Range;

/// Which sides changed a chunk against the base.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChunkKind {
    /// Only ours changed it.
    Ours,
    /// Only theirs changed it.
    Theirs,
    /// Both changed it the same way.
    Same,
    /// Both changed it differently.
    Conflict,
}

/// What is taken for a chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Take {
    Ours,
    Theirs,
    /// Ours, then theirs.
    Both,
    /// The base text (neither change).
    Base,
}

/// One changed region: its lines in each version and in the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub kind: ChunkKind,
    pub base: Range<usize>,
    pub ours: Range<usize>,
    pub theirs: Range<usize>,
    /// Its lines in the result as it is now.
    pub result: Range<usize>,
    /// Taken from a side, or edited by hand.
    pub resolved: bool,
}

/// The three versions of a file, the chunks where they differ and the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merge3 {
    pub base: Vec<String>,
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
    pub result: Vec<String>,
    pub chunks: Vec<Chunk>,
}

/// Lines with their `\n` kept, so joining them gives the text back.
pub fn lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_string).collect()
}

/// Overlapping changes of both sides: their base lines and (side, change) pairs.
type Group<'a> = (Range<usize>, Vec<(bool, &'a Change)>);

/// The changes of one side, and the line shift of everything after them.
fn delta(c: &Change) -> isize {
    c.new.len() as isize - c.old.len() as isize
}

fn shift(at: usize, by: isize) -> usize {
    (at as isize + by).max(0) as usize
}

impl Merge3 {
    /// Splits the versions into chunks. The result starts as the base, every chunk unresolved.
    pub fn new(base: &str, ours: &str, theirs: &str) -> Merge3 {
        let (base, ours, theirs) = (lines(base), lines(ours), lines(theirs));
        let a = normalize(diff(&base, &ours), &base, &ours);
        let b = normalize(diff(&base, &theirs), &base, &theirs);
        // Changes of both sides by base position; overlapping or touching ones form a chunk.
        let mut all: Vec<(bool, &Change)> = a.iter().map(|c| (false, c)).chain(b.iter().map(|c| (true, c))).collect();
        all.sort_by_key(|(_, c)| (c.old.start, c.old.end));
        let mut groups: Vec<Group> = Vec::new();
        for (side, c) in all {
            match groups.last_mut() {
                Some((r, members)) if c.old.start <= r.end && !(c.old.start == r.end && members.iter().all(|(s, _)| *s == side)) => {
                    r.end = r.end.max(c.old.end);
                    members.push((side, c));
                }
                _ => groups.push((c.old.clone(), vec![(side, c)])),
            }
        }
        let range_on = |changes: &[Change], members: &[(bool, &Change)], side: bool, r: &Range<usize>| {
            let before: isize = changes.iter().filter(|c| c.old.end <= r.start && !members.iter().any(|(s, m)| *s == side && *m == *c)).map(delta).sum();
            let inside: isize = members.iter().filter(|(s, _)| *s == side).map(|(_, c)| delta(c)).sum();
            shift(r.start, before)..shift(r.end, before + inside)
        };
        let chunks = groups
            .into_iter()
            .map(|(r, members)| {
                let o = range_on(&a, &members, false, &r);
                let t = range_on(&b, &members, true, &r);
                let has = |side: bool| members.iter().any(|(s, _)| *s == side);
                let kind = match (has(false), has(true)) {
                    (true, false) => ChunkKind::Ours,
                    (false, true) => ChunkKind::Theirs,
                    _ if ours.get(o.clone()) == theirs.get(t.clone()) => ChunkKind::Same,
                    _ => ChunkKind::Conflict,
                };
                Chunk { kind, base: r.clone(), ours: o, theirs: t, result: r, resolved: false }
            })
            .collect();
        let result = base.clone();
        Merge3 { base, ours, theirs, result, chunks }
    }

    /// The result as text.
    pub fn result_text(&self) -> String {
        self.result.concat()
    }

    /// Chunks still unresolved.
    pub fn unresolved(&self) -> usize {
        self.chunks.iter().filter(|c| !c.resolved).count()
    }

    /// Conflicting chunks still unresolved.
    pub fn conflicts_left(&self) -> usize {
        self.chunks.iter().filter(|c| !c.resolved && c.kind == ChunkKind::Conflict).count()
    }

    /// Replaces a chunk's lines in the result and marks it resolved.
    pub fn take(&mut self, chunk: usize, take: Take) {
        let Some(c) = self.chunks.get(chunk) else { return };
        let part = |v: &[String], r: &Range<usize>| v.get(r.clone()).map(<[String]>::to_vec).unwrap_or_default();
        let lines: Vec<String> = match take {
            Take::Ours => part(&self.ours, &c.ours),
            Take::Theirs => part(&self.theirs, &c.theirs),
            Take::Both => [part(&self.ours, &c.ours), part(&self.theirs, &c.theirs)].concat(),
            Take::Base => part(&self.base, &c.base),
        };
        let range = c.result.start.min(self.result.len())..c.result.end.min(self.result.len());
        let by = lines.len() as isize - range.len() as isize;
        self.result.splice(range.clone(), lines);
        for (i, other) in self.chunks.iter_mut().enumerate() {
            if i == chunk {
                other.result = range.start..shift(range.end, by);
                other.resolved = true;
            } else if other.result.start >= range.end && i > chunk {
                other.result = shift(other.result.start, by)..shift(other.result.end, by);
            }
        }
    }

    /// Takes the side that changed for every chunk only one side (or both alike) changed.
    /// Returns how many were taken.
    pub fn take_non_conflicting(&mut self) -> usize {
        let mut n = 0;
        for i in 0..self.chunks.len() {
            let c = &self.chunks[i];
            if c.resolved {
                continue;
            }
            let take = match c.kind {
                ChunkKind::Ours | ChunkKind::Same => Take::Ours,
                ChunkKind::Theirs => Take::Theirs,
                ChunkKind::Conflict => continue,
            };
            self.take(i, take);
            n += 1;
        }
        n
    }

    /// One whole side for the file: the result becomes ours or theirs, every chunk resolved.
    pub fn take_file(&mut self, theirs: bool) {
        self.result = if theirs { self.theirs.clone() } else { self.ours.clone() };
        for c in &mut self.chunks {
            c.result = if theirs { c.theirs.clone() } else { c.ours.clone() };
            c.resolved = true;
        }
    }

    /// The result was edited by hand to `text`: chunks follow their lines, and a chunk whose
    /// lines were edited counts as resolved.
    pub fn edited(&mut self, text: &str) {
        let new = lines(text);
        let changes = diff(&self.result, &new);
        for c in &mut self.chunks {
            let (range, touched) = map_range(&c.result, &changes);
            c.result = range;
            c.resolved |= touched;
        }
        self.result = new;
    }

    /// The line of `to` that matches `line` of `from` (for scrolling the panes together).
    pub fn map_line(&self, from: Pane, line: usize, to: Pane) -> usize {
        let at = |c: &Chunk, p: Pane| match p {
            Pane::Ours => c.ours.clone(),
            Pane::Theirs => c.theirs.clone(),
            Pane::Result => c.result.clone(),
        };
        // The last chunk starting at or before the line decides the offset.
        match self.chunks.iter().rev().find(|c| at(c, from).start <= line) {
            None => line,
            Some(c) => {
                let (f, t) = (at(c, from), at(c, to));
                if line < f.end {
                    // Inside the chunk: proportionally.
                    t.start + (line - f.start) * t.len().max(1) / f.len().max(1)
                } else {
                    t.end + (line - f.end)
                }
            }
        }
    }
}

/// Makes a diff's blocks as small and as late as they can be: equal lines at a block's ends
/// leave it, and pure insertions or deletions slide down past repeated lines (so an added
/// function sits after the closing brace before it, not across it). Two sides' changes then
/// only meet where they really touch the same lines.
fn normalize(changes: Vec<Change>, old: &[String], new: &[String]) -> Vec<Change> {
    let mut out: Vec<Change> = Vec::with_capacity(changes.len());
    let n = changes.len();
    for i in 0..n {
        let mut c = changes[i].clone();
        while !c.old.is_empty() && !c.new.is_empty() && old.get(c.old.start) == new.get(c.new.start) {
            c.old.start += 1;
            c.new.start += 1;
        }
        while !c.old.is_empty() && !c.new.is_empty() && old.get(c.old.end - 1) == new.get(c.new.end - 1) {
            c.old.end -= 1;
            c.new.end -= 1;
        }
        let next_old = changes.get(i + 1).map_or(old.len(), |x| x.old.start);
        if c.old.is_empty() && !c.new.is_empty() {
            while c.old.start + 1 < next_old && new.get(c.new.start).is_some() && new.get(c.new.start) == new.get(c.new.end) && old.get(c.old.start) == new.get(c.new.end) {
                c.old = c.old.start + 1..c.old.end + 1;
                c.new = c.new.start + 1..c.new.end + 1;
            }
        } else if c.new.is_empty() && !c.old.is_empty() {
            while c.old.end + 1 < next_old && old.get(c.old.start) == old.get(c.old.end) && new.get(c.new.start) == old.get(c.old.end) {
                c.old = c.old.start + 1..c.old.end + 1;
                c.new = c.new.start + 1..c.new.end + 1;
            }
        }
        if !c.old.is_empty() || !c.new.is_empty() {
            out.push(c);
        }
    }
    out
}

/// The three editable panes of the merge tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    Ours,
    Result,
    Theirs,
}

/// Where `range` of the old text is in the new one, and whether a change touched it.
fn map_range(range: &Range<usize>, changes: &[Change]) -> (Range<usize>, bool) {
    let mut by_start = 0isize;
    let mut by_end = 0isize;
    let mut start = None;
    let mut end = None;
    let mut touched = false;
    for c in changes {
        let overlaps = c.old.start < range.end && c.old.end > range.start
            // An insertion inside the range, or into an empty range at its position.
            || (c.old.is_empty() && c.old.start > range.start && c.old.start < range.end)
            || (range.is_empty() && c.old.start == range.start);
        if overlaps {
            touched = true;
            if c.old.start <= range.start && start.is_none() {
                start = Some(c.new.start);
            }
            if c.old.end >= range.end {
                end = Some(c.new.end);
            }
        }
        if c.old.end <= range.start && !overlaps {
            by_start += delta(c);
        }
        if c.old.end <= range.end && !(overlaps && c.old.end >= range.end) {
            by_end += delta(c);
        }
    }
    let s = start.unwrap_or_else(|| shift(range.start, by_start));
    let e = end.unwrap_or_else(|| shift(range.end, by_end)).max(s);
    (s..e, touched)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "a\nb\nc\nd\ne\nf\ng\n";

    #[test]
    fn chunks_by_side() {
        // Ours changes b, theirs changes f, both change d differently.
        let m = Merge3::new(BASE, "a\nB\nc\nD1\ne\nf\ng\n", "a\nb\nc\nD2\ne\nF\ng\n");
        let kinds: Vec<_> = m.chunks.iter().map(|c| (c.kind, c.base.clone(), c.ours.clone(), c.theirs.clone())).collect();
        assert_eq!(kinds, vec![(ChunkKind::Ours, 1..2, 1..2, 1..2), (ChunkKind::Conflict, 3..4, 3..4, 3..4), (ChunkKind::Theirs, 5..6, 5..6, 5..6)]);
        assert_eq!(m.result_text(), BASE);
        assert_eq!(m.unresolved(), 3);
    }

    #[test]
    fn insertions_deletions_and_same_changes() {
        // Ours inserts two lines after a and deletes e; theirs deletes e too and appends h.
        let m = Merge3::new(BASE, "a\nx\ny\nb\nc\nd\nf\ng\n", "a\nb\nc\nd\nf\ng\nh\n");
        let kinds: Vec<_> = m.chunks.iter().map(|c| (c.kind, c.base.clone(), c.ours.clone(), c.theirs.clone())).collect();
        assert_eq!(kinds, vec![(ChunkKind::Ours, 1..1, 1..3, 1..1), (ChunkKind::Same, 4..5, 6..6, 4..4), (ChunkKind::Theirs, 7..7, 8..8, 6..7)]);
        let mut m = m;
        assert_eq!(m.take_non_conflicting(), 3);
        assert_eq!(m.result_text(), "a\nx\ny\nb\nc\nd\nf\ng\nh\n");
        assert_eq!(m.unresolved(), 0);
    }

    #[test]
    fn taking_sides_shifts_later_chunks() {
        let mut m = Merge3::new(BASE, "a\nB\nB2\nc\nD1\ne\nf\ng\n", "a\nb\nc\nD2\nD3\ne\nF\ng\n");
        assert_eq!(m.take_non_conflicting(), 2);
        assert_eq!(m.conflicts_left(), 1);
        let conflict = m.chunks.iter().position(|c| c.kind == ChunkKind::Conflict).unwrap();
        assert_eq!(m.chunks[conflict].result, 4..5);
        m.take(conflict, Take::Both);
        assert_eq!(m.result_text(), "a\nB\nB2\nc\nD1\nD2\nD3\ne\nF\ng\n");
        assert_eq!(m.chunks[conflict].result, 4..7);
        assert_eq!(m.chunks[2].result, 8..9);
        m.take(conflict, Take::Theirs);
        assert_eq!(m.result_text(), "a\nB\nB2\nc\nD2\nD3\ne\nF\ng\n");
        assert_eq!(m.chunks[2].result, 7..8);
        assert_eq!(m.unresolved(), 0);
        m.take_file(false);
        assert_eq!(m.result_text(), "a\nB\nB2\nc\nD1\ne\nf\ng\n");
    }

    #[test]
    fn hand_edits_move_and_resolve_chunks() {
        let mut m = Merge3::new(BASE, "a\nB\nc\nD1\ne\nf\ng\n", "a\nb\nc\nD2\ne\nF\ng\n");
        // Two lines added at the top: every chunk moves down, none is touched.
        m.edited("top\ntop2\na\nb\nc\nd\ne\nf\ng\n");
        assert_eq!(m.chunks.iter().map(|c| c.result.clone()).collect::<Vec<_>>(), vec![3..4, 5..6, 7..8]);
        assert_eq!(m.unresolved(), 3);
        // Rewriting the conflict's line resolves it.
        m.edited("top\ntop2\na\nb\nc\nmine\nmore\ne\nf\ng\n");
        assert_eq!(m.chunks[1].result, 5..7);
        assert!(m.chunks[1].resolved && !m.chunks[0].resolved);
        assert_eq!(m.chunks[2].result, 8..9);
        // Taking a side afterwards still lands on the right lines.
        m.take(2, Take::Theirs);
        assert_eq!(m.result_text(), "top\ntop2\na\nb\nc\nmine\nmore\ne\nF\ng\n");
    }

    #[test]
    fn an_added_function_does_not_conflict_with_a_change_above_it() {
        let base = "fn tax() {\n    5\n}\n\n#[cfg(test)]\n";
        let ours = "fn tax() {\n    4\n}\n\n#[cfg(test)]\n";
        let theirs = "fn tax() {\n    5\n}\n\nfn discount() {\n    10\n}\n\n#[cfg(test)]\n";
        let mut m = Merge3::new(base, ours, theirs);
        let kinds: Vec<_> = m.chunks.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, [ChunkKind::Ours, ChunkKind::Theirs]);
        assert_eq!(m.take_non_conflicting(), 2);
        assert_eq!(m.result_text(), "fn tax() {\n    4\n}\n\nfn discount() {\n    10\n}\n\n#[cfg(test)]\n");
    }

    #[test]
    fn panes_scroll_together() {
        let m = Merge3::new(BASE, "a\nx\ny\nb\nc\nd\ne\nf\ng\n", BASE);
        assert_eq!(m.map_line(Pane::Ours, 0, Pane::Theirs), 0);
        assert_eq!(m.map_line(Pane::Ours, 5, Pane::Theirs), 3);
        assert_eq!(m.map_line(Pane::Theirs, 3, Pane::Ours), 5);
    }

    proptest::proptest! {
        /// Taking one side for every chunk gives exactly that side's text.
        #[test]
        fn taking_a_side_everywhere_gives_that_side(b in "[abc\n]{0,40}", o in "[abc\n]{0,40}", t in "[abc\n]{0,40}") {
            for theirs in [false, true] {
                let mut m = Merge3::new(&b, &o, &t);
                for i in 0..m.chunks.len() {
                    m.take(i, if theirs { Take::Theirs } else { Take::Ours });
                }
                proptest::prop_assert_eq!(m.result_text(), if theirs { t.clone() } else { o.clone() });
                proptest::prop_assert_eq!(m.unresolved(), 0);
            }
        }
    }

    #[test]
    fn odd_inputs_do_not_panic() {
        for (b, o, t) in [("", "", ""), ("", "x", "y"), ("a", "", "b"), ("a\nb", "a\nc", "a\nd"), ("x\n", "x\n", "x\n")] {
            let mut m = Merge3::new(b, o, t);
            m.take_non_conflicting();
            for i in 0..m.chunks.len() {
                m.take(i, Take::Both);
            }
            m.edited("whatever\n");
            m.take(99, Take::Ours);
            let _ = m.map_line(Pane::Result, 50, Pane::Ours);
        }
        let m = Merge3::new("", "x", "y");
        assert_eq!(m.chunks.len(), 1);
        assert_eq!(m.chunks[0].kind, ChunkKind::Conflict);
    }
}
