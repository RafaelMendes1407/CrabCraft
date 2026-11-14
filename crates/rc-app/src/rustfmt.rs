//! rustfmt on open Rust documents: Reformat Code and reformatting on save. rustfmt runs in the
//! background with the user's settings (the same command as at commit time); its text comes back
//! as line edits, applied only if the document did not change meanwhile.

use crate::docs::Docs;
use rc_cargo::FormatJob;

struct Job {
    doc: u64,
    version: u64,
    save: bool,
    /// Only the changes touching these lines (0-based, inclusive) are applied: Reformat Code
    /// with a selection.
    lines: Option<(usize, usize)>,
    job: FormatJob,
}

/// A finished job.
pub enum Done {
    Skipped {
        index: usize,
        save: bool,
    },
    /// The document at `index` is formatted; `save` when it must be written again.
    Applied {
        index: usize,
        save: bool,
    },
    Failed {
        name: String,
        error: String,
        save: bool,
    },
}

/// The documents being formatted; dropping a job stops its rustfmt.
#[derive(Default)]
pub struct Jobs(Vec<Job>);

impl Jobs {
    /// A newer request replaces the one running on the same document. With `lines`, only
    /// the changes touching them are kept.
    pub fn push_lines(&mut self, doc: u64, version: u64, save: bool, lines: Option<(usize, usize)>, job: FormatJob) {
        let save = save || self.0.iter().any(|j| j.doc == doc && j.save);
        self.0.retain(|j| j.doc != doc);
        self.0.push(Job { doc, version, save, lines, job });
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn poll(&mut self, docs: &mut Docs) -> Vec<Done> {
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.0.len() {
            let Some(result) = self.0[i].job.poll() else {
                i += 1;
                continue;
            };
            let job = self.0.remove(i);
            let Some(index) = docs.list.iter().position(|d| d.editor.id == job.doc) else { continue };
            let d = &mut docs.list[index];
            match result {
                Err(error) => done.push(Done::Failed { name: d.name(), error, save: job.save }),
                // Typed into meanwhile: the text no longer matches.
                Ok(_) if d.editor.buffer.version() != job.version => done.push(Done::Skipped { index, save: job.save }),
                Ok(text) => {
                    let old = d.editor.buffer.rope().to_string();
                    let new = text.replace("\r\n", "\n");
                    let changes = match job.lines {
                        Some(lines) => line_edits_within(&old, &new, lines),
                        None => line_edits(&old, &new),
                    };
                    if !changes.is_empty() {
                        d.editor.apply_text_edits(changes);
                    }
                    done.push(Done::Applied { index, save: job.save });
                }
            }
        }
        done
    }
}

/// The changes, in characters of `old`, that turn it into `new` line by line, so the caret and
/// the undo history only see the lines rustfmt changed.
pub fn line_edits(old: &str, new: &str) -> Vec<rc_text::Change> {
    line_edits_filtered(old, new, |_| true)
}

/// Only the blocks of changes that touch `old`'s lines `first..=last` (a block that crosses
/// the edge is taken whole, so the code stays consistent).
pub fn line_edits_within(old: &str, new: &str, (first, last): (usize, usize)) -> Vec<rc_text::Change> {
    line_edits_filtered(old, new, |old_lines| {
        if old_lines.is_empty() { (first..=last + 1).contains(&old_lines.start) } else { old_lines.start <= last && old_lines.end > first }
    })
}

fn line_edits_filtered(old: &str, new: &str, keep: impl Fn(std::ops::Range<usize>) -> bool) -> Vec<rc_text::Change> {
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    let mut starts = Vec::with_capacity(a.len() + 1);
    let mut at = 0;
    starts.push(0);
    for line in &a {
        at += line.chars().count();
        starts.push(at);
    }
    rc_vcs::textdiff::diff(&a, &b)
        .into_iter()
        .filter(|c| keep(c.old.clone()))
        .map(|c| rc_text::Change::new(starts[c.old.start], starts[c.old.end], b[c.new].concat()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{line_edits, line_edits_within};

    fn apply(old: &str, new: &str) -> String {
        let mut editor = rc_editor::EditorState::new(1, None, old);
        editor.apply_text_edits(line_edits(old, new));
        editor.buffer.rope().to_string()
    }

    #[test]
    fn only_the_changed_lines_are_edited() {
        let old = "fn main(){\nlet a=1;\n    let b = 2;\n}\n";
        let new = "fn main() {\n    let a = 1;\n    let b = 2;\n}\n";
        assert_eq!(apply(old, new), new);
        assert_eq!(line_edits(old, new).len(), 1, "the untouched lines are kept");
        assert!(line_edits(new, new).is_empty());
        for (old, new) in [("", "fn x() {}\n"), ("a\nb", "a\n"), ("é\nx\nñ\n", "é\ny\nñ\nz\n"), ("x\n\n\n", "x\n")] {
            assert_eq!(apply(old, new), new);
        }
    }

    #[test]
    fn a_selection_keeps_the_changes_outside_it() {
        let old = "fn a(){1}\n\nfn b(){2}\n\nfn c(){3}\n";
        let new = "fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n\nfn c() {\n    3\n}\n";
        let mut editor = rc_editor::EditorState::new(1, None, old);
        editor.apply_text_edits(line_edits_within(old, new, (2, 2)));
        assert_eq!(editor.buffer.rope().to_string(), "fn a(){1}\n\nfn b() {\n    2\n}\n\nfn c(){3}\n");
        assert!(line_edits_within(old, old, (0, 4)).is_empty());
    }
}
