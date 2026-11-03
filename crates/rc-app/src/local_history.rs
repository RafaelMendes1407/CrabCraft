//! Local history in the app: snapshots of files are written on one background thread, in the
//! order they were taken, and the Local History dialog loads revisions through the same queue
//! (so it sees every snapshot taken before it opened). The UI never touches the disk for it.

use rc_core::history::{LocalHistory, Revision};
use rc_core::inbox::Outbox;
use rc_text::Rope;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};

enum Job {
    Record(PathBuf, Rope),
    List(PathBuf, Outbox<Loaded>),
    Read(Revision, Outbox<Loaded>),
}

/// Answers for the Local History dialog.
pub enum Loaded {
    List(Vec<Revision>),
    /// The text of the revision recorded at that time (or why it could not be read).
    Text(u64, Result<String, String>),
}

pub struct HistoryService {
    tx: Sender<Job>,
}

impl HistoryService {
    /// Starts the worker; it ends when the service is dropped.
    pub fn start(history: LocalHistory) -> Self {
        let (tx, rx) = channel::<Job>();
        let spawned = std::thread::Builder::new().name("local-history".into()).spawn(move || {
            history.prune_all(rc_core::history::now_ms());
            while let Ok(job) = rx.recv() {
                match job {
                    Job::Record(path, text) => {
                        if text.len_bytes() <= rc_core::history::MAX_BYTES
                            && let Err(e) = history.record(&path, &text.to_string(), rc_core::history::now_ms())
                        {
                            eprintln!("local history: {}: {e}", path.display());
                        }
                    }
                    Job::List(path, out) => {
                        out.send(Loaded::List(history.revisions(&path)));
                    }
                    Job::Read(rev, out) => {
                        out.send(Loaded::Text(rev.time_ms, history.read(&rev).map_err(|e| e.to_string())));
                    }
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("local history: could not start: {e}");
        }
        HistoryService { tx }
    }

    /// Keeps `text` (an O(1) copy of the buffer) as a version of `path`.
    pub fn record(&self, path: &Path, text: Rope) {
        let _ = self.tx.send(Job::Record(path.to_path_buf(), text));
    }

    pub fn list(&self, path: &Path, out: Outbox<Loaded>) {
        let _ = self.tx.send(Job::List(path.to_path_buf(), out));
    }

    pub fn read(&self, revision: &Revision, out: Outbox<Loaded>) {
        let _ = self.tx.send(Job::Read(revision.clone(), out));
    }
}

/// One line of the diff preview: `'+'`, `'-'` or `'@'` (a hunk header with the line number).
pub type PreviewLine = (char, String);

/// The changes from `old` (a revision) to `new` (the current text): preview lines, at most
/// `limit`, and the number of added and removed lines.
pub fn preview(old: &str, new: &str, limit: usize) -> (Vec<PreviewLine>, usize, usize) {
    let changes = rc_vcs::LineChanges::compute(old, new);
    let (ol, nl): (Vec<&str>, Vec<&str>) = (old.split('\n').collect(), new.split('\n').collect());
    let (mut lines, mut added, mut removed) = (Vec::new(), 0, 0);
    for c in &changes.changes {
        added += c.new.len();
        removed += c.old.len();
        if lines.len() < limit {
            lines.push(('@', (c.new.start + 1).to_string()));
            lines.extend(c.old.clone().filter_map(|i| ol.get(i)).map(|l| ('-', l.trim_end_matches('\r').to_string())));
            lines.extend(c.new.clone().filter_map(|i| nl.get(i)).map(|l| ('+', l.trim_end_matches('\r').to_string())));
        }
    }
    lines.truncate(limit);
    (lines, added, removed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn local_history_preview_lists_the_changed_lines() {
        let (lines, added, removed) = super::preview("a\nb\nc\n", "a\nB\nc\nd\n", 100);
        assert_eq!((added, removed), (2, 1));
        assert!(lines.contains(&('-', "b".to_string())) && lines.contains(&('+', "B".to_string())) && lines.contains(&('+', "d".to_string())), "{lines:?}");
        assert_eq!(super::preview("x", "x", 100).0, vec![]);
    }
}
