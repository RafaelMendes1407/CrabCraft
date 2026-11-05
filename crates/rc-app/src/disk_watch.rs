//! Bounded, coalesced external-file reads. No watcher file I/O runs on the UI thread.
use crate::{
    disk_revert::{self, Error, Snapshot, Ticket},
    docs::Docs,
};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub enum QueueError {
    Full,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn drain(watch: &mut Watch, docs: &mut Docs) -> Vec<String> {
        let end = Instant::now() + Duration::from_secs(5);
        let mut errors = Vec::new();
        loop {
            errors.extend(watch.poll(docs, Waker::noop()));
            if watch.active.is_empty() && watch.pending.is_empty() {
                return errors;
            }
            assert!(Instant::now() < end, "external read did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn coalesced_external_reads_keep_new_edits_and_closed_document_identity() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.rs");
        let old = "fn main() { println!(\"old\"); }\n";
        std::fs::write(&path, old).unwrap();
        let mut docs = Docs::new(rc_syntax::ParserPool::new(Waker::noop().as_fn()));
        docs.open(&path).unwrap();
        let mut watch = Watch::default();
        std::fs::write(&path, "fn main() { println!(\"disk\"); }\n").unwrap();
        for _ in 0..1000 {
            watch
                .queue(&mut docs, &path)
                .unwrap_or_else(|_| panic!("queue unexpectedly full"));
        }
        assert_eq!(watch.pending.len(), 1);
        assert!(watch.poll(&mut docs, Waker::noop()).is_empty());
        docs.list[0].editor.replace(0, 0, "// user edit\n");
        assert!(drain(&mut watch, &mut docs).is_empty());
        assert!(docs.list[0].conflict);
        assert!(
            docs.list[0]
                .editor
                .buffer
                .rope()
                .to_string()
                .contains("user edit")
        );
        watch.queue(&mut docs, &path).unwrap_or_else(|_| panic!());
        watch.poll(&mut docs, Waker::noop());
        let old_id = docs.list[0].editor.id;
        docs.close(0);
        docs.open(&path).unwrap();
        docs.list[0].editor.replace(0, 0, "// reopened\n");
        assert!(drain(&mut watch, &mut docs).is_empty());
        assert_ne!(docs.list[0].editor.id, old_id);
        assert!(
            docs.list[0]
                .editor
                .buffer
                .rope()
                .to_string()
                .contains("reopened")
        );
        assert!(!docs.list[0].conflict);
    }

    #[test]
    #[cfg(unix)]
    fn replaced_fifo_reports_conflict_without_waiting_for_a_writer() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.rs");
        let text = "fn main() {}\n";
        std::fs::write(&path, text).unwrap();
        let mut docs = Docs::new(rc_syntax::ParserPool::new(Waker::noop().as_fn()));
        docs.open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let mut watch = Watch::default();
        let start = Instant::now();
        watch.queue(&mut docs, &path).unwrap_or_else(|_| panic!());
        assert_eq!(drain(&mut watch, &mut docs).len(), 1);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(docs.list[0].conflict);
        assert_eq!(docs.list[0].editor.buffer.rope().to_string(), text);
    }

    #[test]
    fn native_watcher_reloads_real_rust_with_bom_crlf_without_running_project_code() {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"),"[package]\nname=\"watcher_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(
            root.path().join("build.rs"),
            "fn main() { std::fs::write(\"ran-build\",\"yes\").unwrap(); }\n",
        )
        .unwrap();
        let path = root.path().join("src/main.rs");
        std::fs::write(&path, "fn main() { println!(\"old\"); }\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace(root.path());
        app.open_file_and_wait(&path);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 900.0));
        h.run_steps(2);
        let text = "fn main() {\r\n    assert_eq!(6*7,42); // external update\r\n}\r\n";
        let mut bytes = vec![0xef, 0xbb, 0xbf];
        bytes.extend_from_slice(text.as_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            h.run_steps(1);
            if h.state()
                .docs
                .active_doc()
                .unwrap()
                .editor
                .buffer
                .rope()
                .to_string()
                .contains("external update")
            {
                break;
            }
            assert!(Instant::now() < end, "watcher failed to reload");
            std::thread::sleep(Duration::from_millis(10));
        }
        let doc = h.state().docs.active_doc().unwrap();
        assert!(doc.bom);
        assert_eq!(doc.editor.buffer.line_ending(), rc_text::LineEnding::CrLf);
        assert!(!doc.is_modified());
        assert!(!root.path().join("ran-build").exists());
        h.state_mut().docs.save(0).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        h.state_mut()
            .docs
            .active_doc_mut()
            .unwrap()
            .editor
            .replace(0, 0, "// unsaved\n");
        std::fs::write(
            &path,
            "fn main() { println!(\"another external update\"); }\n",
        )
        .unwrap();
        loop {
            h.run_steps(1);
            if h.state().docs.active_doc().unwrap().conflict {
                break;
            }
            assert!(Instant::now() < end, "conflict was not reported");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            h.state()
                .docs
                .active_doc()
                .unwrap()
                .editor
                .buffer
                .rope()
                .to_string()
                .contains("// unsaved")
        );
        // Compile the checked external snapshot explicitly; watcher/import never executed it.
        std::fs::write(&path, &bytes).unwrap();
        let output = std::process::Command::new("cargo")
            .args(["run", "--offline", "--quiet"])
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(root.path().join("ran-build").exists());
    }
}
impl std::fmt::Display for QueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(tr(
            "External file update queue is full; reopen the affected file",
        ))
    }
}
struct Loaded {
    snapshot: Snapshot,
    ending: rc_text::LineEnding,
    equal: bool,
}
struct Active {
    ticket: Ticket,
    cancel: Arc<AtomicBool>,
    inbox: Inbox<Result<Loaded, Error>>,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub struct Watch {
    pending: BTreeMap<PathBuf, Ticket>,
    active: Vec<Active>,
}
impl Watch {
    /// A pending save owns this path until it reports completion. Old read snapshots must
    /// not be applied after the write, even when the writer replaced the file atomically.
    pub fn suspend(&mut self,path:&Path) {
        self.pending.remove(path);
        for job in &self.active {if job.ticket.path==path{job.cancel.store(true,Ordering::Release);}}
    }
    pub fn queue(&mut self, docs: &mut Docs, path: &Path) -> Result<(), QueueError> {
        let Some(doc) = docs
            .list
            .iter_mut()
            .find(|d| d.path == path && !d.generated)
        else {
            return Ok(());
        };
        doc.cancel_pending_save();
        doc.disk_epoch = doc.disk_epoch.saturating_add(1);
        if self.pending.len() >= 256 && !self.pending.contains_key(path) {
            doc.conflict = true;
            return Err(QueueError::Full);
        }
        self.pending
            .insert(path.to_path_buf(), Ticket::capture(doc));
        for active in &self.active {
            if active.ticket.path == path {
                active.cancel.store(true, Ordering::Release);
            }
        }
        Ok(())
    }
    pub fn poll(&mut self, docs: &mut Docs, waker: Waker) -> Vec<String> {
        let mut errors = Vec::new();
        // Keep cancelled workers counted until they acknowledge cancellation. Bursts must
        // never spawn an unbounded number of threads, even on a slow filesystem.
        for job in &self.active {
            if job.ticket.index(docs).is_none() {
                job.cancel.store(true, Ordering::Release);
                if let Some(doc) = docs.list.iter().find(|d| {
                    d.editor.id == job.ticket.id
                        && d.path == job.ticket.path
                        && d.disk_epoch == job.ticket.disk_epoch
                        && !d.generated
                }) {
                    self.pending
                        .entry(doc.path.clone())
                        .or_insert_with(|| Ticket::capture(doc));
                }
            }
        }
        let mut completed = Vec::new();
        for (i, job) in self.active.iter().enumerate() {
            if let Some(result) = job.inbox.drain().into_iter().next() {
                completed.push((i, result));
            }
        }
        for (i, result) in completed.into_iter().rev() {
            let job = self.active.remove(i);
            if job.cancel.load(Ordering::Acquire) {
                continue;
            }
            let Some(index) = job.ticket.index(docs) else {
                continue;
            };
            match result {
                Ok(loaded) => {
                    let doc = &mut docs.list[index];
                    if loaded.equal {
                        if !doc.is_modified() {
                            doc.bom = loaded.snapshot.bom;
                            doc.editor.buffer.set_line_ending(loaded.ending);
                            doc.editor.buffer.mark_saved();
                        }
                        doc.mtime = Some(loaded.snapshot.modified);
                    } else if doc.is_modified() {
                        doc.conflict = true;
                        if let Some(history) = &docs.history {
                            history
                                .record(&doc.path, rc_text::Rope::from_str(&loaded.snapshot.text));
                        }
                    } else {
                        docs.reloaded(
                            index,
                            &loaded.snapshot.text,
                            loaded.snapshot.bom,
                            Some(loaded.snapshot.modified),
                        );
                        docs.list[index]
                            .editor
                            .buffer
                            .set_line_ending(loaded.ending);
                        docs.list[index].editor.buffer.mark_saved();
                        docs.list[index].editor.request_parse(&docs.pool);
                    }
                }
                Err(Error::Cancelled) => (),
                Err(error) => {
                    docs.list[index].conflict = true;
                    let message = match error {
                        Error::TooLarge => {
                            tr("External reload supports files up to 8 MB").to_string()
                        }
                        Error::NotRegular => {
                            tr("Only regular files can be opened in the editor").to_string()
                        }
                        other => other.to_string(),
                    };
                    errors.push(trf(
                        "Could not reload {file}: {error}",
                        &[
                            ("file", &job.ticket.path.display().to_string()),
                            ("error", &message),
                        ],
                    ));
                }
            }
        }
        self.pending.retain(|_, ticket| {
            if let Some(doc) = docs.list.iter().find(|d| {
                d.editor.id == ticket.id
                    && d.path == ticket.path
                    && d.disk_epoch == ticket.disk_epoch
                    && !d.generated
            }) {
                ticket.version = doc.editor.buffer.version();
                true
            } else {
                false
            }
        });
        while self.active.len() < 2 {
            let Some(path) = self
                .pending
                .keys()
                .find(|p| !self.active.iter().any(|j| j.ticket.path == **p))
                .cloned()
            else {
                break;
            };
            let Some(ticket) = self.pending.remove(&path) else {
                continue;
            };
            let Some(index) = ticket.index(docs) else {
                continue;
            };
            let rope = docs.list[index].editor.buffer.snapshot();
            let inbox = Inbox::new(waker.clone());
            let out = inbox.sender();
            let cancel = Arc::new(AtomicBool::new(false));
            let token = cancel.clone();
            match std::thread::Builder::new()
                .name("rc-external-file-read".into())
                .spawn(move || {
                    let result = disk_revert::read(&path, &token).and_then(|mut snapshot| {
                        let ending = rc_text::LineEnding::detect(&snapshot.text);
                        snapshot.text = snapshot.text.replace("\r\n", "\n").replace('\r', "\n");
                        if token.load(Ordering::Acquire) {
                            return Err(Error::Cancelled);
                        }
                        let equal = rope == snapshot.text;
                        Ok(Loaded {
                            snapshot,
                            ending,
                            equal,
                        })
                    });
                    out.send(result);
                }) {
                Ok(_) => self.active.push(Active {
                    ticket,
                    cancel,
                    inbox,
                }),
                Err(error) => errors.push(trf(
                    "Could not reload {file}: {error}",
                    &[
                        ("file", &ticket.path.display().to_string()),
                        ("error", &error.to_string()),
                    ],
                )),
            }
        }
        errors
    }
}
