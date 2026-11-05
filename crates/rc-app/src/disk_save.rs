//! Serial, bounded snapshot saves. A cancelled worker remains owned until acknowledgement.
use crate::docs::{Doc, Docs};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Cancelled,
    Changed,
    TooLarge,
    Full,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => f.write_str(&trf(
                "Could not save the document: {error}",
                &[("error", e)],
            )),
            Self::Cancelled => f.write_str(tr("Saving cancelled; unsaved edits were kept")),
            Self::Changed => f.write_str(tr(
                "The disk file changed before saving; unsaved edits were kept",
            )),
            Self::TooLarge => f.write_str(tr("Background saving supports documents up to 128 MB")),
            Self::Full => f.write_str(tr("The save queue is full; unsaved edits were kept")),
        }
    }
}
impl std::error::Error for Error {}
#[derive(Clone)]
pub struct Ticket {
    pub id: u64,
    pub path: PathBuf,
    pub version: u64,
    pub epoch: u64,
    rope: rc_text::Rope,
    ending: rc_text::LineEnding,
    bom: bool,
    modified: Option<SystemTime>,
    force: bool,
}
impl Ticket {
    pub fn capture(d: &Doc) -> Self {
        Self {
            id: d.editor.id,
            path: d.path.clone(),
            version: d.editor.buffer.version(),
            epoch: d.disk_epoch,
            rope: d.editor.buffer.snapshot(),
            ending: d.editor.buffer.line_ending(),
            bom: d.bom,
            modified: d.mtime,
            force: false,
        }
    }
}
pub struct Written {
    pub ticket: Ticket,
    modified: Option<SystemTime>,
}
struct Job {
    ticket: Ticket,
    cancel: Arc<AtomicBool>,
    inbox: Inbox<Result<Written, Error>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub struct Saves {
    pending: std::collections::VecDeque<Ticket>,
    active: Option<Job>,
    #[cfg(test)]
    gate: Option<Arc<AtomicBool>>,
}
impl Saves {
    pub fn busy(&self) -> bool {
        self.active.is_some() || !self.pending.is_empty()
    }
    pub fn contains(&self, path: &std::path::Path) -> bool {
        self.pending.iter().any(|t| t.path == path)
            || self.active.as_ref().is_some_and(|j| j.ticket.path == path)
    }
    pub fn cancel(&mut self) {
        self.pending.clear();
        if let Some(j) = &self.active {
            j.cancel.store(true, Ordering::Release);
        }
    }
    pub fn queue(&mut self, doc: &Doc) -> Result<(), Error> {
        self.queue_with_force(doc, false)
    }
    pub fn queue_with_force(&mut self, doc: &Doc, force: bool) -> Result<(), Error> {
        if doc.generated || doc.editor.read_only {
            return Ok(());
        }
        if doc.editor.buffer.len_chars() > 128 * 1024 * 1024 {
            return Err(Error::TooLarge);
        }
        let mut ticket = Ticket::capture(doc);
        ticket.force = force;
        if self.active.as_ref().is_some_and(|j| {
            j.ticket.id == ticket.id
                && j.ticket.version == ticket.version
                && j.ticket.epoch == ticket.epoch
                && !j.cancel.load(Ordering::Acquire)
        }) {
            return Ok(());
        }
        if let Some(old) = self.pending.iter_mut().find(|t| t.path == ticket.path) {
            *old = ticket;
            return Ok(());
        }
        if self.pending.len() >= 256 {
            return Err(Error::Full);
        }
        self.pending.push_back(ticket);
        Ok(())
    }
    pub fn poll(&mut self, docs: &mut Docs, waker: Waker) -> Vec<Result<PathBuf, Error>> {
        let mut done = Vec::new();
        if let Some(result) = self
            .active
            .as_ref()
            .and_then(|j| j.inbox.drain().into_iter().next())
        {
            let Some(job) = self.active.take() else {
                return done;
            };
            match result {
                Ok(written) => {
                    let t = &written.ticket;
                    if let Some(d) = docs
                        .list
                        .iter_mut()
                        .find(|d| d.editor.id == t.id && d.path == t.path && !d.generated)
                    {
                        // Epoch mismatch can mean a reload/conflict: never overwrite that state.
                        if d.disk_epoch == t.epoch {
                            let previous = std::mem::replace(&mut d.saved, t.rope.clone());
                            d.editor.buffer.mark_saved_snapshot(t.version);
                            d.mtime = written.modified;
                            d.disk_epoch = d.disk_epoch.saturating_add(1);
                            d.conflict = false;
                            for queued in &mut self.pending {
                                if queued.id == t.id
                                    && queued.path == t.path
                                    && queued.epoch == t.epoch
                                {
                                    queued.epoch = d.disk_epoch;
                                    queued.modified = d.mtime;
                                }
                            }
                            if let Some(h) = &docs.history {
                                h.record(&d.path, previous);
                                h.record(&d.path, t.rope.clone());
                            }
                        }
                    }
                    if job.cancel.load(Ordering::Acquire) {
                        done.push(Err(Error::Cancelled));
                    } else {
                        done.push(Ok(t.path.clone()));
                    }
                }
                Err(e) => done.push(Err(e)),
            }
        }
        if self.active.is_none() {
            while let Some(mut ticket) = self.pending.pop_front() {
                let Some(d) = docs.list.iter().find(|d| {
                    d.editor.id == ticket.id
                        && d.path == ticket.path
                        && d.disk_epoch == ticket.epoch
                        && !d.generated
                }) else {
                    done.push(Err(Error::Changed));
                    continue;
                };
                // A previous queued save of this path advanced its epoch. Only one worker writes.
                ticket.modified = d.mtime;
                let cancel = Arc::new(AtomicBool::new(false));
                let token = cancel.clone();
                if let Some(doc) = docs
                    .list
                    .iter_mut()
                    .find(|d| d.editor.id == ticket.id && d.path == ticket.path)
                {
                    doc.pending_save.0 = Some(cancel.clone());
                }
                let inbox = Inbox::new(waker.clone());
                let out = inbox.sender();
                let work = ticket.clone();
                #[cfg(test)]
                let gate = self.gate.clone();
                std::thread::spawn(move || {
                    #[cfg(test)]
                    if let Some(gate) = gate {
                        while !gate.load(Ordering::Acquire) && !token.load(Ordering::Acquire) {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                    out.send(write(work, &token));
                });
                self.active = Some(Job {
                    ticket,
                    cancel,
                    inbox,
                });
                break;
            }
        }
        done
    }
}
fn write(ticket: Ticket, cancel: &AtomicBool) -> Result<Written, Error> {
    if cancel.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    let metadata = match std::fs::symlink_metadata(&ticket.path) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(Error::Io(e)),
    };
    if metadata.as_ref().is_some_and(|m| !m.file_type().is_file()) {
        return Err(Error::Io(std::io::ErrorKind::InvalidInput.into()));
    }
    let modified = metadata.as_ref().and_then(|m| m.modified().ok());
    if !ticket.force && modified != ticket.modified {
        return Err(Error::Changed);
    }
    let mut bytes = Vec::new();
    if ticket.bom {
        bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    // Encode in bounded chunks with cancellation checks; no rope-to-String on the UI.
    for chunk in ticket.rope.chunks() {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if ticket.ending == rc_text::LineEnding::CrLf {
            bytes.extend_from_slice(chunk.replace('\n', "\r\n").as_bytes());
        } else {
            bytes.extend_from_slice(chunk.as_bytes());
        }
        if bytes.len() > 128 * 1024 * 1024 {
            return Err(Error::TooLarge);
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    rc_core::paths::write_atomic_cancellable(&ticket.path, &bytes, cancel).map_err(|e| {
        if e.kind() == std::io::ErrorKind::Interrupted {
            Error::Cancelled
        } else {
            Error::Io(e)
        }
    })?;
    let modified = std::fs::metadata(&ticket.path)
        .and_then(|m| m.modified())
        .ok();
    Ok(Written { ticket, modified })
}

pub type Continuation = Box<dyn FnOnce(&mut crate::app::CrabCraftApp)>;
#[derive(Default)]
pub struct Flow {
    pub saves: Saves,
    pub auxiliary: AuxiliarySaves,
    pub exit_prepared: bool,
    pub pending_workspace: Option<PathBuf>,
    pub transition_prepared: bool,
    pub format_pending: std::collections::BTreeSet<u64>,
    pub force_paths: std::collections::BTreeSet<PathBuf>,
    pub continuation: Option<(Option<PathBuf>, bool, Continuation)>,
    pub failed: bool,
    pub exit_ready: bool,
}
impl Flow {
    pub fn fail(&mut self) {
        self.continuation = None;
        self.failed = true;
        self.exit_prepared = false;
        self.pending_workspace = None;
        self.transition_prepared = false;
    }
    pub fn cancel(&mut self) {
        self.saves.cancel();
        self.auxiliary.cancel();
        self.continuation = None;
        self.failed = true;
        self.exit_prepared = false;
        self.pending_workspace = None;
        self.transition_prepared = false;
        self.format_pending.clear();
        self.force_paths.clear();
    }
}
impl crate::app::CrabCraftApp {
    /// Queue a snapshot after style edits. Formatting is scheduled before the write.
    pub fn save_document_async(&mut self, index: usize, format: bool) {
        let Some(doc) = self.docs.list.get(index) else {
            return;
        };
        if doc.generated || !doc.is_modified() {
            return;
        }
        if doc.editor.read_only {
            self.disk_save.fail();
            self.notify(
                tr("A read-only document has unsaved edits; the action was stopped"),
                true,
            );
            return;
        }
        let edits = crate::docs::style_fixes(&doc.editor, &doc.style);
        if !edits.is_empty() {
            self.docs.list[index].editor.apply_text_edits(edits);
        }
        if format && self.settings.format_on_save {
            if !self.rustfmt_jobs.is_empty() {
                if self.disk_save.format_pending.len() >= 256
                    && !self
                        .disk_save
                        .format_pending
                        .contains(&self.docs.list[index].editor.id)
                {
                    self.disk_save.fail();
                    self.notify(Error::Full.to_string(), true);
                    return;
                }
                self.disk_save
                    .format_pending
                    .insert(self.docs.list[index].editor.id);
                return;
            }
            if self.rustfmt_document(index, true) {
                return;
            }
        }
        self.disk_watch.suspend(&self.docs.list[index].path);
        let force = self
            .disk_save
            .force_paths
            .remove(&self.docs.list[index].path);
        if let Err(error) = self
            .disk_save
            .saves
            .queue_with_force(&self.docs.list[index], force)
        {
            self.disk_save.fail();
            self.notify(error.to_string(), true);
        }
    }
    pub fn keep_mine_after_save(&mut self, index: usize) {
        if let Some(doc) = self.docs.list.get(index) {
            self.disk_save.force_paths.insert(doc.path.clone());
        }
        self.save_document_async(index, true);
    }
    pub(crate) fn defer_save(
        &mut self,
        trust: bool,
        action: impl FnOnce(&mut Self) + 'static,
    ) -> bool {
        if !self.file_edits.busy()
            && !self.disk_save.saves.busy()
            && !self.disk_save.auxiliary.busy()
            && self.rustfmt_jobs.is_empty()
            && self.disk_save.format_pending.is_empty()
            && !self
                .docs
                .list
                .iter()
                .any(|d| d.is_modified() && !d.generated)
        {
            return false;
        }
        if self.disk_save.continuation.is_some() {
            self.notify(tr("Another action is waiting for documents to save"), false);
            return true;
        }
        if trust && !self.disk_trust_valid() {
            self.notify(
                tr("Trust this project before running project tools or code"),
                true,
            );
            return true;
        }
        let root = self.workspace.as_ref().map(|w| w.ws.root.clone());
        self.disk_save.failed = false;
        self.disk_save.continuation = Some((root, trust, Box::new(action)));
        self.save_all_docs();
        true
    }
    fn disk_trust_valid(&self) -> bool {
        self.runner.require_trust().is_ok()
            && self
                .workspace
                .as_ref()
                .is_none_or(|w| self.settings.trusted_projects.contains(&w.ws.root))
    }
    pub fn cancel_disk_save(&mut self) {
        self.workspace_open.cancel();
        self.file_edits.cancel(); self.file_edits.take_cancelled();
        self.disk_save.cancel();
        self.rustfmt_jobs = Default::default();
    }
    pub(crate) fn prepare_workspace_save(&mut self) {
        if !self.disk_save.transition_prepared && self.workspace.is_some() {
            self.save_session();
            self.disk_save.transition_prepared = true;
        }
    }
    pub fn pending_workspace_target(&self) -> Option<&std::path::Path> {
        self.disk_save.pending_workspace.as_deref()
    }
    pub(crate) fn poll_disk_save(&mut self) {
        if self.file_edits.take_cancelled(){self.cancel_disk_save();}
        if self.rustfmt_jobs.is_empty()
            && let Some(id) = self.disk_save.format_pending.pop_first()
            && let Some(i) = self.docs.list.iter().position(|d| d.editor.id == id)
        {
            self.save_document_async(i, true);
        }
        let auxiliary_result = if self.disk_save.saves.active.is_none() {
            self.disk_save.auxiliary.poll(self.waker.clone())
        } else {
            None
        };
        if let Some(Err(error)) = auxiliary_result {
            self.disk_save.fail();
            self.disk_save.exit_prepared = false;
            self.notify(error, true);
        }
        // Session/settings and document writers never overlap, including when a user
        // opens an IDE settings/session JSON file in an editor.
        let results = if self.disk_save.auxiliary.active.is_none() {
            self.disk_save
                .saves
                .poll(&mut self.docs, self.waker.clone())
        } else {
            Vec::new()
        };
        for result in results {
            match result {
                Ok(path) => {
                    self.runner.test_failures.remove(&path);
                    self.insight.did_save(&path);
                }
                Err(error) => {
                    self.disk_save.fail();
                    self.notify(error.to_string(), true);
                }
            }
        }
        if self
            .disk_save
            .continuation
            .as_ref()
            .is_some_and(|(root, trust, _)| {
                *root != self.workspace.as_ref().map(|w| w.ws.root.clone())
                    || (*trust && !self.disk_trust_valid())
            })
        {
            self.cancel_disk_save();
        }
        if self.file_edits.busy()
            || self.disk_save.continuation.is_none()
            || self.disk_save.failed
            || self.disk_save.saves.busy()
            || self.disk_save.auxiliary.busy()
            || !self.rustfmt_jobs.is_empty()
            || !self.disk_save.format_pending.is_empty()
        {
            return;
        }
        // Any edits made during the previous write get their own snapshot before execution.
        if self
            .docs
            .list
            .iter()
            .any(|d| d.is_modified() && !d.generated)
        {
            self.save_all_docs();
            return;
        }
        if let Some((root, trust, action)) = self.disk_save.continuation.take() {
            if root != self.workspace.as_ref().map(|w| w.ws.root.clone())
                || (trust && !self.disk_trust_valid())
            {
                self.disk_save.cancel();
                return;
            }
            action(self);
        }
    }
    #[cfg(test)]
    pub(crate) fn save_all_and_wait(&mut self) {
        self.save_all_docs();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while self.file_edits.busy()
            || self.disk_save.continuation.is_some()
            || self.disk_save.saves.busy()
            || self.disk_save.auxiliary.busy()
            || !self.rustfmt_jobs.is_empty()
            || !self.disk_save.format_pending.is_empty()
        {
            assert!(std::time::Instant::now() < end, "background save timed out");
            self.poll_file_edits();
            self.poll_rustfmt();
            self.poll_disk_save();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!self.disk_save.failed, "background save failed");
    }
}

struct Auxiliary {
    session: Option<(PathBuf, rc_core::session::Session)>,
    settings: Option<rc_core::settings::Settings>,
}
struct AuxiliaryJob {
    cancel: Arc<AtomicBool>,
    inbox: Inbox<Result<(), String>>,
}
impl Drop for AuxiliaryJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub struct AuxiliarySaves {
    pending: std::collections::VecDeque<Auxiliary>,
    active: Option<AuxiliaryJob>,
}
impl AuxiliarySaves {
    pub fn queue(
        &mut self,
        session: Option<(PathBuf, rc_core::session::Session)>,
        settings: Option<rc_core::settings::Settings>,
    ) -> Result<(), Error> {
        if session.is_none() && settings.is_none() {
            return Ok(());
        }
        let key = session.as_ref().map(|(path, _)| path.clone());
        if let Some(old) = self
            .pending
            .iter_mut()
            .find(|p| p.session.as_ref().map(|(path, _)| path) == key.as_ref())
        {
            old.session = session;
            if settings.is_some() {
                old.settings = settings;
            }
            return Ok(());
        }
        if self.pending.len() >= 256 {
            return Err(Error::Full);
        }
        self.pending.push_back(Auxiliary { session, settings });
        Ok(())
    }
    pub fn busy(&self) -> bool {
        self.active.is_some() || !self.pending.is_empty()
    }
    fn cancel(&mut self) {
        self.pending.clear();
        if let Some(j) = &self.active {
            j.cancel.store(true, Ordering::Release);
        }
    }
    fn poll(&mut self, waker: Waker) -> Option<Result<(), String>> {
        let result = self
            .active
            .as_ref()
            .and_then(|j| j.inbox.drain().into_iter().next());
        if result.is_some() {
            self.active = None;
        }
        if self.active.is_none()
            && let Some(work) = self.pending.pop_front()
        {
            let cancel = Arc::new(AtomicBool::new(false));
            let token = cancel.clone();
            let inbox = Inbox::new(waker);
            let out = inbox.sender();
            std::thread::spawn(move || {
                let result = (|| {
                    if token.load(Ordering::Acquire) {
                        return Err(tr("Saving cancelled; unsaved edits were kept").to_string());
                    }
                    if let Some(settings) = work.settings {
                        settings.save().map_err(|e| e.to_string())?;
                    }
                    if token.load(Ordering::Acquire) {
                        return Err(tr("Saving cancelled; unsaved edits were kept").to_string());
                    }
                    if let Some((path, session)) = work.session {
                        session.save(&path).map_err(|e| e.to_string())?;
                    }
                    Ok(())
                })();
                out.send(result);
            });
            self.active = Some(AuxiliaryJob { cancel, inbox });
        }
        result
    }
}

impl crate::app::CrabCraftApp {
    pub fn build_project_after_save(
        &mut self,
        target: &std::path::Path,
        action: rc_dotnet::BuildAction,
    ) {
        let path = target.to_path_buf();
        if self.defer_save(true, move |app| app.build_project_after_save(&path, action)) {
            return;
        }
        let outputs = self.runner.build(target, action);
        crate::ui::run_windows::handle_outputs(self, outputs);
    }
    pub fn run_scratch_after_save(&mut self, path: &std::path::Path) {
        let queued = path.to_path_buf();
        if self.defer_save(false, move |app| app.run_scratch_after_save(&queued)) {
            return;
        }
        let outputs = self.runner.run_scratch(path);
        crate::ui::run_windows::handle_outputs(self, outputs);
    }
    pub fn run_command_after_save(&mut self, program: PathBuf, args: Vec<String>) {
        let p = program.clone();
        let a = args.clone();
        if self.defer_save(false, move |app| app.run_command_after_save(p, a)) {
            return;
        }
        let outputs = self.runner.run_command(program, args);
        crate::ui::run_windows::handle_outputs(self, outputs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn docs(path: &std::path::Path) -> Docs {
        let mut docs = Docs::new(rc_syntax::ParserPool::new(Waker::noop().as_fn()));
        docs.open(path).unwrap();
        docs
    }
    fn drain(queue: &mut Saves, docs: &mut Docs) -> Vec<Result<PathBuf, Error>> {
        let end = Instant::now() + Duration::from_secs(5);
        let mut out = Vec::new();
        while queue.busy() {
            out.extend(queue.poll(docs, Waker::noop()));
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        }
        out
    }
    #[test]
    fn serial_snapshots_preserve_later_edits_encoding_permissions_and_history_baseline() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.rs");
        std::fs::write(&path, b"\xef\xbb\xbffn main() {}\r\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        let mut docs = docs(&path);
        docs.history = Some(crate::local_history::HistoryService::start(
            rc_core::history::LocalHistory::new(root.path().join("history")),
        ));
        docs.list[0].editor.replace(0, 0, "// snapshot\n");
        let gate = Arc::new(AtomicBool::new(false));
        let mut queue = Saves {
            gate: Some(gate.clone()),
            ..Default::default()
        };
        queue.queue(&docs.list[0]).unwrap();
        queue.poll(&mut docs, Waker::noop());
        docs.list[0].editor.replace(0, 0, "// later edit\n");
        gate.store(true, Ordering::Release);
        assert!(drain(&mut queue, &mut docs).iter().all(Result::is_ok));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"\xef\xbb\xbf// snapshot\r\nfn main() {}\r\n"
        );
        assert!(docs.list[0].is_modified());
        assert!(docs.list[0].saved.to_string().contains("snapshot"));
        assert!(!docs.list[0].saved.to_string().contains("later edit"));
        queue.queue(&docs.list[0]).unwrap();
        assert!(drain(&mut queue, &mut docs).iter().all(Result::is_ok));
        assert!(!docs.list[0].is_modified());
        let inbox = Inbox::new(Waker::noop());
        docs.history.as_ref().unwrap().list(&path, inbox.sender());
        let end = Instant::now() + Duration::from_secs(5);
        let revisions = loop {
            if let Some(crate::local_history::Loaded::List(items)) =
                inbox.drain().into_iter().next()
            {
                break items;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(
            revisions.len() >= 3,
            "original, first save and later save must enter Local History"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
    }
    #[test]
    fn cancellation_retains_edits_and_newer_pending_write_never_runs_concurrently() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let mut docs = docs(&path);
        docs.list[0].editor.replace(0, 0, "// first\n");
        let gate = Arc::new(AtomicBool::new(false));
        let mut queue = Saves {
            gate: Some(gate.clone()),
            ..Default::default()
        };
        queue.queue(&docs.list[0]).unwrap();
        queue.poll(&mut docs, Waker::noop());
        docs.list[0].editor.replace(0, 0, "// second\n");
        queue.queue(&docs.list[0]).unwrap();
        assert_eq!(queue.pending.len(), 1);
        queue.cancel();
        assert!(queue.busy());
        let results = drain(&mut queue, &mut docs);
        assert!(matches!(results.as_slice(), [Err(Error::Cancelled)]));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn main() {}\n");
        assert!(docs.list[0].is_modified());
        gate.store(true, Ordering::Release);
        queue.queue(&docs.list[0]).unwrap();
        queue.poll(&mut docs, Waker::noop());
        docs.list[0].editor.replace(0, 0, "// third\n");
        queue.queue(&docs.list[0]).unwrap();
        assert!(drain(&mut queue, &mut docs).iter().all(Result::is_ok));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .starts_with("// third\n// second\n")
        );
        assert!(!docs.list[0].is_modified());
    }
    #[test]
    #[cfg(unix)]
    fn changed_special_and_symlink_destinations_fail_without_losing_buffers() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.rs");
        let target = root.path().join("target.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        std::fs::write(&target, "untouched").unwrap();
        let mut docs = docs(&path);
        docs.list[0].editor.replace(0, 0, "// user\n");
        let mut queue = Saves::default();
        queue.queue(&docs.list[0]).unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&target, &path).unwrap();
        assert!(drain(&mut queue, &mut docs).iter().any(Result::is_err));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
        assert!(docs.list[0].is_modified());
        std::fs::remove_file(&path).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        queue.queue(&docs.list[0]).unwrap();
        assert!(drain(&mut queue, &mut docs).iter().any(Result::is_err));
        assert!(docs.list[0].is_modified());
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "external edit\n").unwrap();
        queue.queue(&docs.list[0]).unwrap();
        assert!(matches!(
            drain(&mut queue, &mut docs).as_slice(),
            [Err(Error::Changed)]
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external edit\n");
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn fixture(root: &std::path::Path) -> PathBuf {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"),"[package]\nname=\"save_barrier_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(root.join("build.rs"),"fn main() { let text=std::fs::read_to_string(\"src/main.rs\").unwrap(); std::fs::write(\"build-observed.rs\",text).unwrap(); }\n").unwrap();
        let path = root.join("src/main.rs");
        std::fs::write(
            &path,
            "fn main() { std::fs::write(\"program-observed\",\"old\").unwrap(); }\n",
        )
        .unwrap();
        path
    }
    fn app(
        home: &std::path::Path,
        root: &std::path::Path,
        path: &std::path::Path,
    ) -> crate::app::CrabCraftApp {
        let mut app = crate::screens::test_app(home, false);
        app.open_workspace(root);
        app.trust_project_tools_for_test();
        app.open_file_and_wait(path);
        app.settings.format_on_save = false;
        let end = Instant::now() + Duration::from_secs(20);
        while app.runner.cargo_workspace.is_none() {
            app.poll_cargo_workspace();
            assert!(Instant::now() < end, "fixture metadata did not load");
            std::thread::sleep(Duration::from_millis(5));
        }
        app
    }
    fn poll(app: &mut crate::app::CrabCraftApp, until: impl Fn(&crate::app::CrabCraftApp) -> bool) {
        let end = Instant::now() + Duration::from_secs(30);
        while !until(app) {
            app.poll_cargo_workspace();
            app.poll_rustfmt();
            app.poll_disk_save();
            let outputs = app.runner.update();
            crate::ui::run_windows::handle_outputs(app, outputs);
            assert!(Instant::now() < end, "save barrier timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn real_cargo_build_and_run_observe_final_saved_text_before_spawn() {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = fixture(root.path());
        let mut app = app(home.path(), root.path(), &path);
        let text = "fn main() { std::fs::write(\"program-observed\",\"latest edited buffer\").unwrap(); }\n";
        let len = app.docs.list[0].editor.buffer.len_chars();
        app.docs.list[0].editor.replace(0, len, text);
        app.run_cargo_command("run --offline --quiet");
        assert!(app.runner.build.is_none());
        assert!(app.disk_save.continuation.is_some());
        poll(&mut app, |_| root.path().join("program-observed").exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join("program-observed")).unwrap(),
            "latest edited buffer"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("build-observed.rs")).unwrap(),
            text
        );
        assert!(!app.docs.list[0].is_modified());
    }
    #[test]
    fn failed_save_cancel_trust_revocation_and_provider_switch_never_spawn_or_lose_buffers() {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = fixture(root.path());
        let mut app = app(home.path(), root.path(), &path);
        app.docs.list[0].editor.replace(0, 0, "// unsaved buffer\n");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        app.run_cargo_command("run --offline --quiet");
        poll(&mut app, |app| {
            app.disk_save.failed && !app.disk_save.saves.busy()
        });
        assert!(app.runner.build.is_none());
        assert!(app.docs.list[0].is_modified());
        assert!(!root.path().join("build-observed.rs").exists());
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "fn main() {}\n").unwrap();
        app.docs.list[0].mtime = std::fs::metadata(&path).unwrap().modified().ok();
        app.run_cargo_command("run --offline --quiet");
        app.cancel_disk_save();
        app.poll_disk_save();
        assert!(app.docs.list[0].is_modified());
        assert!(app.runner.build.is_none());
        app.run_cargo_command("run --offline --quiet");
        app.settings.trusted_projects.clear();
        app.poll_disk_save();
        assert!(app.disk_save.continuation.is_none());
        assert!(app.runner.build.is_none());
        app.trust_project_tools_for_test();
        app.run_cargo_command("run --offline --quiet");
        app.change_cargo_provider(crate::criticalup_provider::Change::Default);
        assert!(app.disk_save.continuation.is_none());
        assert!(app.runner.build.is_none());
        assert!(!root.path().join("build-observed.rs").exists());
    }
    #[test]
    fn cancelled_close_or_workspace_switch_keeps_buffers_and_successful_close_waits_for_write() {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let path = fixture(root.path());
        fixture(other.path());
        let mut app = app(home.path(), root.path(), &path);
        app.docs.list[0].editor.replace(0, 0, "// keep on cancel\n");
        app.close_tab(0);
        assert_eq!(app.docs.list.len(), 1);
        app.cancel_disk_save();
        assert_eq!(app.docs.list.len(), 1);
        assert!(app.docs.list[0].is_modified());
        app.open_workspace(other.path());
        assert_eq!(app.pending_workspace_target(), Some(other.path()));
        assert_eq!(app.docs.list.len(), 1);
        assert_eq!(app.workspace.as_ref().unwrap().ws.root, root.path());
        app.cancel_disk_save();
        assert!(app.pending_workspace_target().is_none());
        assert_eq!(app.docs.list.len(), 1);
        app.close_tab(0);
        poll(&mut app, |app| app.docs.list.is_empty());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .starts_with("// keep on cancel")
        );
    }
    #[test]
    fn rustfmt_on_save_finishes_before_real_cargo_and_keeps_a_separate_undo_step() {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = fixture(root.path());
        let mut app = app(home.path(), root.path(), &path);
        app.settings.format_on_save = true;
        let text = "fn main(){let answer=6*7;std::fs::write(\"program-observed\",answer.to_string()).unwrap();}\n";
        let len = app.docs.list[0].editor.buffer.len_chars();
        app.docs.list[0].editor.replace(0, len, text);
        app.docs.list[0].editor.buffer.seal_undo();
        app.run_cargo_command("run --offline --quiet");
        poll(&mut app, |_| root.path().join("program-observed").exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join("program-observed")).unwrap(),
            "42"
        );
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("fn main() {\n"));
        assert_eq!(
            saved,
            std::fs::read_to_string(root.path().join("build-observed.rs")).unwrap()
        );
        assert!(!app.docs.list[0].is_modified());
        app.docs.list[0].editor.undo();
        assert_eq!(app.docs.list[0].editor.buffer.rope().to_string(), text);
        assert!(app.docs.list[0].is_modified());
    }
}

impl crate::app::CrabCraftApp {
    pub fn rerun_tests_after_save(&mut self, failed: bool) {
        if self.defer_save(true, move |app| app.rerun_tests_after_save(failed)) {
            return;
        }
        let outputs = if failed {
            self.runner.rerun_failed()
        } else {
            self.runner.rerun_tests()
        };
        crate::ui::run_windows::handle_outputs(self, outputs);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn reload_and_close_cancel_the_held_writer_before_it_can_restore_an_obsolete_path() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.rs");
        let original = "fn main() {}\n";
        std::fs::write(&path, original).unwrap();
        let mut docs = Docs::new(rc_syntax::ParserPool::new(Waker::noop().as_fn()));
        docs.open(&path).unwrap();
        docs.list[0].editor.replace(0, 0, "// pending\n");
        let mut saves = Saves {
            gate: Some(Arc::new(AtomicBool::new(false))),
            ..Default::default()
        };
        saves.queue(&docs.list[0]).unwrap();
        saves.poll(&mut docs, Waker::noop());
        let modified = docs.list[0].mtime;
        docs.reloaded(0, "fn main() { /* reloaded */ }\n", false, modified);
        let end = Instant::now() + Duration::from_secs(5);
        while saves.busy() {
            saves.poll(&mut docs, Waker::noop());
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(
            docs.list[0]
                .editor
                .buffer
                .rope()
                .to_string()
                .contains("reloaded")
        );
        docs.list[0].editor.replace(0, 0, "// close pending\n");
        saves.queue(&docs.list[0]).unwrap();
        saves.poll(&mut docs, Waker::noop());
        drop(docs.close(0));
        while saves.busy() {
            saves.poll(&mut docs, Waker::noop());
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(docs.list.is_empty());
    }
    #[test]
    fn native_cancel_button_keeps_the_real_rust_buffer_and_prevents_deferred_cargo() {
        use egui_kittest::kittest::Queryable;
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname=\"cancel_save_ui\"\nversion=\"0.1.0\"\n[workspace]\n",
        )
        .unwrap();
        let path = root.path().join("src/main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace(root.path());
        app.trust_project_tools_for_test();
        app.open_file_and_wait(&path);
        app.settings.format_on_save = false;
        app.docs.list[0].editor.replace(0, 0, "// retained\n");
        app.disk_save.saves.gate = Some(Arc::new(AtomicBool::new(false)));
        app.run_cargo_command("run --offline");
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 900.0));
        h.get_by_label("Cancel saving").click();
        h.run_steps(3);
        assert!(h.state().disk_save.continuation.is_none());
        assert!(h.state().docs.list[0].is_modified());
        assert!(
            h.state().docs.list[0]
                .editor
                .buffer
                .rope()
                .to_string()
                .starts_with("// retained")
        );
        assert!(h.state().runner.build.is_none());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "fn main() {}\n");
    }
}
