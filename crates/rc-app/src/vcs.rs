//! Git integration: the repository of the workspace, its status, branches and history, the
//! gutter change marks of open files, commits and remote operations. Every git call runs on the
//! shared runtime; results come back through an inbox.

use crate::docs::Docs;
use rc_core::{Inbox, Runtime, Waker};
use rc_kit::i18n::{tr, trf};
use rc_vcs::{Blame, Branch, Commit, CommitOptions, FileState, LineChangeKind, LineChanges, LogOptions, PatchPreview, PullOptions, PushOptions, Repository, Revision, Stash, StashOptions, Status, StatusOptions, Tag, Worktree, WorktreeCheckout};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

enum VcsEvent {
    Repo(Option<Repository>),
    Status(Result<Status, String>),
    Branches(Vec<Branch>),
    Log(Vec<Commit>),
    Head(PathBuf, Option<String>),
    Blame(PathBuf, Result<Blame, String>),
    /// An operation finished: (what, error).
    Done(&'static str, Option<String>),
    CommitFiles(String, Vec<rc_vcs::ChangedFile>),
    /// Texts for the diff viewer: (title, path, left, right).
    Diff(String, PathBuf, String, String),
    Tags(Vec<Tag>),
    Stashes(Vec<Stash>),
    StashFiles(String, Vec<rc_vcs::ChangedFile>),
    Remotes(Vec<String>),
    Output(OpOutput),
    /// A created patch to put on the clipboard.
    PatchClipboard(String),
    /// A patch read for applying (generation, file, text and its preview, or why it failed).
    PatchLoaded(u64, Option<PathBuf>, Result<(Arc<Vec<u8>>, Result<PatchPreview, String>), String>),
    /// A new preview of the loaded patch (generation, preview).
    PatchPreview(u64, Result<PatchPreview, String>),
    /// What applying the loaded patch printed (generation, output).
    PatchApplied(u64, OpOutput),
    Worktrees(Vec<Worktree>),
    /// Removing this worktree was refused because it has local changes.
    RemoveRefused(PathBuf),
}

/// What a new patch is made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchSource {
    /// The local changes of these files (relative to the repository): a `git diff`.
    Changes(Vec<String>),
    /// These commits, newest first: mails from `git format-patch`.
    Commits(Vec<String>),
}

/// A patch loaded to be applied, and what applying it would do.
pub struct PendingPatch {
    /// The file it was read from (`None`: the clipboard).
    pub path: Option<PathBuf>,
    pub text: Arc<Vec<u8>>,
    /// The files it touches and whether it applies; `None` while git checks it.
    pub preview: Option<Result<PatchPreview, String>>,
    /// What git printed when it was applied.
    pub result: Option<OpOutput>,
}

/// The views of the Git tool window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GitTab {
    #[default]
    Log,
    Tags,
    Stashes,
    Worktrees,
}

/// What git printed for the last explicit network operation (a tag push), or why it failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpOutput {
    pub text: String,
    pub error: bool,
}

/// The changed files of the work tree, as shown by the Commit window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub state: FileState,
    pub untracked: bool,
}

/// A side-by-side diff to show in the editor area.
pub struct DiffView {
    pub title: String,
    pub path: PathBuf,
    pub left: rc_editor::EditorState,
    pub right: rc_editor::EditorState,
}

pub struct Vcs {
    pub repo: Option<Repository>,
    pub status: Option<Status>,
    pub branches: Vec<Branch>,
    pub log: Vec<Commit>,
    pub selected_commit: Option<String>,
    pub commit_files: Vec<rc_vcs::ChangedFile>,
    pub message: String,
    pub amend: bool,
    pub excluded: BTreeSet<String>,
    pub busy: Option<&'static str>,
    pub error: Option<String>,
    pub tab: GitTab,
    pub tags: Vec<Tag>,
    pub selected_tag: Option<String>,
    pub remotes: Vec<String>,
    pub stashes: Vec<Stash>,
    /// Hash of the stash shown in the Stashes view.
    pub selected_stash: Option<String>,
    /// The files the selected stash saved (untracked ones have status `?`).
    pub stash_files: Vec<rc_vcs::ChangedFile>,
    /// Apply and Pop also restore what was staged (`--index`).
    pub restore_index: bool,
    /// Output of the last tag push or worktree prune.
    pub output: Option<OpOutput>,
    /// Commits of the log marked with Ctrl/Cmd- or Shift-click (for a patch of several).
    pub marked_commits: BTreeSet<String>,
    /// The patch of the Apply Patch dialog.
    pub patch: Option<PendingPatch>,
    /// Apply patches with a three-way merge fallback (`--3way`).
    pub patch_three_way: bool,
    /// A patch is being read and checked.
    pub patch_loading: bool,
    /// Why the last patch could not be loaded.
    pub patch_error: Option<String>,
    patch_gen: u64,
    /// Text for the app to put on the clipboard.
    pub clipboard: Option<String>,
    pub worktrees: Vec<Worktree>,
    /// Path of the worktree shown in the Worktrees view.
    pub selected_worktree: Option<PathBuf>,
    /// A worktree whose removal was refused for its local changes: the Git window offers to
    /// force it.
    pub force_remove: Option<PathBuf>,
    inbox: Inbox<VcsEvent>,
    waker: Waker,
    heads: HashMap<PathBuf, Option<String>>,
    head_pending: BTreeSet<PathBuf>,
    marks_version: HashMap<u64, u64>,
    status_requested: Option<Instant>,
    status_dirty: bool,
    pub blame_for: Option<PathBuf>,
    pub diff: Option<DiffView>,
    next_doc_id: u64,
    /// Goes up with every new status (the Changed Files scope follows it).
    pub status_version: u64,
}

impl Vcs {
    pub fn new(waker: Waker) -> Self {
        Vcs {
            repo: None,
            status: None,
            branches: Vec::new(),
            log: Vec::new(),
            selected_commit: None,
            commit_files: Vec::new(),
            message: String::new(),
            amend: false,
            excluded: BTreeSet::new(),
            busy: None,
            error: None,
            tab: GitTab::Log,
            tags: Vec::new(),
            selected_tag: None,
            remotes: Vec::new(),
            stashes: Vec::new(),
            selected_stash: None,
            stash_files: Vec::new(),
            restore_index: false,
            output: None,
            marked_commits: BTreeSet::new(),
            patch: None,
            patch_three_way: false,
            patch_loading: false,
            patch_error: None,
            patch_gen: 0,
            clipboard: None,
            worktrees: Vec::new(),
            selected_worktree: None,
            force_remove: None,
            inbox: Inbox::new(waker.clone()),
            waker,
            heads: HashMap::new(),
            head_pending: BTreeSet::new(),
            marks_version: HashMap::new(),
            status_requested: None,
            status_dirty: false,
            blame_for: None,
            diff: None,
            next_doc_id: 1 << 40,
            status_version: 0,
        }
    }

    pub fn open(&mut self, root: &Path) {
        *self = Vcs::new(self.waker.clone());
        let out = self.inbox.sender();
        let root = root.to_path_buf();
        Runtime::spawn(async move {
            out.send(VcsEvent::Repo(Repository::discover(&root).await.ok()));
        });
    }

    pub fn close(&mut self) {
        *self = Vcs::new(self.waker.clone());
    }

    pub fn branch_name(&self) -> Option<String> {
        self.status.as_ref().map(|s| s.branch.head.clone().unwrap_or_else(|| tr("Detached HEAD").to_string()))
    }

    /// The files of the work tree that changed (the Commit window's list).
    pub fn changes(&self) -> Vec<Change> {
        let Some(st) = &self.status else { return Vec::new() };
        st.entries
            .iter()
            .filter(|e| e.worktree != FileState::Ignored)
            .map(|e| {
                let untracked = e.worktree == FileState::Untracked;
                let state = if e.worktree != FileState::Unmodified { e.worktree } else { e.index };
                Change { path: e.path.clone(), state, untracked }
            })
            .collect()
    }

    /// A file changed on disk: refresh the status soon and forget its HEAD text when it is
    /// git's own state that changed.
    pub fn on_fs_change(&mut self, path: &Path) {
        if self.repo.is_none() {
            return;
        }
        let s = path.to_string_lossy();
        if s.contains("/.git/")
            && (s.ends_with("HEAD") || s.contains("/refs/") || s.ends_with("index")) {
                self.heads.clear();
                self.marks_version.clear();
            }
        self.status_dirty = true;
    }

    pub fn refresh(&mut self) {
        self.status_dirty = true;
        self.status_requested = None;
        self.heads.clear();
        self.marks_version.clear();
        self.load_log();
    }

    fn request_status(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        self.status_requested = Some(Instant::now());
        self.status_dirty = false;
        let out = self.inbox.sender();
        Runtime::spawn(async move {
            let st = repo.status(&StatusOptions::default()).await.map_err(|e| e.to_string());
            out.send(VcsEvent::Status(st));
            if let Ok(b) = repo.branches().await {
                out.send(VcsEvent::Branches(b));
            }
            if let Ok(t) = repo.tags().await {
                out.send(VcsEvent::Tags(t));
            }
            if let Ok(s) = repo.stashes().await {
                out.send(VcsEvent::Stashes(s));
            }
            if let Ok(r) = repo.remotes().await {
                out.send(VcsEvent::Remotes(r.into_iter().map(|r| r.name).collect()));
            }
            if let Ok(w) = repo.worktrees().await {
                out.send(VcsEvent::Worktrees(w));
            }
        });
    }

    pub fn load_log(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        let out = self.inbox.sender();
        Runtime::spawn(async move {
            let opts = LogOptions { all: true, max_count: 500, ..Default::default() };
            if let Ok(log) = repo.log(&opts).await {
                out.send(VcsEvent::Log(log));
            }
        });
    }

    pub fn select_commit(&mut self, hash: &str) {
        self.selected_commit = Some(hash.to_string());
        self.commit_files.clear();
        let Some(repo) = self.repo.clone() else { return };
        let out = self.inbox.sender();
        let h = hash.to_string();
        Runtime::spawn(async move {
            if let Ok(files) = repo.commit_files(&h).await {
                out.send(VcsEvent::CommitFiles(h, files));
            }
        });
    }

    // ------------------------------------------------------------------ operations

    fn op<F>(&mut self, what: &'static str, f: F)
    where
        F: std::future::Future<Output = Result<(), rc_vcs::GitError>> + Send + 'static,
    {
        self.run(what, false, async move { f.await.map(|()| None) });
    }

    /// Runs an operation in the background. Text it returns becomes [`Vcs::output`]; with
    /// `report`, so does git's message when it fails.
    fn run<F>(&mut self, what: &'static str, report: bool, f: F)
    where
        F: std::future::Future<Output = Result<Option<String>, rc_vcs::GitError>> + Send + 'static,
    {
        self.busy = Some(what);
        self.error = None;
        if report {
            self.output = None;
        }
        let out = self.inbox.sender();
        Runtime::spawn(async move {
            match f.await {
                Ok(text) => {
                    if let Some(text) = text {
                        out.send(VcsEvent::Output(OpOutput { text, error: false }));
                    }
                    out.send(VcsEvent::Done(what, None));
                }
                Err(e) => {
                    let summary = user_error(&e);
                    if report {
                        let text = match e.details().filter(|d| !d.trim().is_empty()) {
                            Some(d) => format!("{summary}\n{}", d.trim()),
                            None => summary.clone(),
                        };
                        out.send(VcsEvent::Output(OpOutput { text, error: true }));
                    }
                    out.send(VcsEvent::Done(what, Some(summary)));
                }
            }
        });
    }

    // ------------------------------------------------------------------ tags

    /// Creates a tag on `target` (default HEAD), annotated when `message` is not empty.
    pub fn create_tag(&mut self, name: &str, target: Option<String>, message: Option<String>) {
        let Some(repo) = self.repo.clone() else { return };
        let name = name.to_string();
        let message = message.filter(|m| !m.trim().is_empty());
        self.op("tag", async move { repo.create_tag(&name, target.as_deref(), message.as_deref()).await });
    }

    pub fn delete_tag(&mut self, name: &str) {
        let Some(repo) = self.repo.clone() else { return };
        let name = name.to_string();
        self.op("tag_delete", async move { repo.delete_tag(&name).await });
    }

    /// Pushes one tag, or every tag (`None`), to `remote`; git's report goes to [`Vcs::output`].
    pub fn push_tags(&mut self, remote: &str, name: Option<String>) {
        let Some(repo) = self.repo.clone() else { return };
        let remote = remote.to_string();
        self.run("tag_push", true, async move { repo.push_tags(&remote, name.as_deref(), None, None).await.map(Some) });
    }

    /// Checks out the commit of a tag (detached HEAD).
    pub fn checkout_tag(&mut self, name: &str) {
        let Some(repo) = self.repo.clone() else { return };
        let name = name.to_string();
        self.op("tag_checkout", async move { repo.checkout_tag(&name).await });
    }

    // ------------------------------------------------------------------ worktrees

    /// Creates a worktree at `path` and selects it.
    pub fn add_worktree(&mut self, path: PathBuf, what: WorktreeCheckout) {
        let Some(repo) = self.repo.clone() else { return };
        self.selected_worktree = Some(path.clone());
        self.op("worktree_add", async move { repo.add_worktree(&path, &what).await });
    }

    /// Removes a worktree. Refused for local changes unless `force`; then
    /// [`Vcs::force_remove`] asks whether to force it.
    pub fn remove_worktree(&mut self, path: &Path, force: bool) {
        let Some(repo) = self.repo.clone() else { return };
        let path = path.to_path_buf();
        let out = self.inbox.sender();
        self.op("worktree_remove", async move {
            let r = repo.remove_worktree(&path, force).await;
            if let Err(rc_vcs::GitError::WorktreeDirty(p)) = &r {
                out.send(VcsEvent::RemoveRefused(p.clone()));
            }
            r
        });
    }

    pub fn lock_worktree(&mut self, path: &Path, reason: &str) {
        let Some(repo) = self.repo.clone() else { return };
        let (path, reason) = (path.to_path_buf(), reason.to_string());
        self.op("worktree_lock", async move { repo.lock_worktree(&path, Some(&reason)).await });
    }

    pub fn unlock_worktree(&mut self, path: &Path) {
        let Some(repo) = self.repo.clone() else { return };
        let path = path.to_path_buf();
        self.op("worktree_unlock", async move { repo.unlock_worktree(&path).await });
    }

    /// Forgets the worktrees whose folders are gone; git's report goes to [`Vcs::output`].
    pub fn prune_worktrees(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        self.run("worktree_prune", true, async move { repo.prune_worktrees().await.map(Some) });
    }

    /// The worktree this window has open.
    pub fn current_worktree(&self) -> Option<&Worktree> {
        let root = self.repo.as_ref()?.root();
        self.worktrees.iter().find(|w| w.path == root)
    }

    // ------------------------------------------------------------------ stashes

    /// Stashes the work tree's changes (`--include-untracked`, `--keep-index` on request).
    pub fn stash_push(&mut self, message: &str, include_untracked: bool, keep_index: bool) {
        let Some(repo) = self.repo.clone() else { return };
        let message = Some(message.trim().to_string()).filter(|m| !m.is_empty());
        let opts = StashOptions { message, include_untracked, keep_index, paths: Vec::new() };
        self.op("stash", async move { repo.stash_push(&opts).await });
    }

    /// Applies a stash to the work tree; `pop` also drops it when it applied cleanly.
    pub fn stash_apply(&mut self, stash: &Stash, pop: bool) {
        let Some(repo) = self.repo.clone() else { return };
        let (index, hash, restore) = (stash.index, stash.hash.clone(), self.restore_index);
        self.op(if pop { "stash_pop" } else { "stash_apply" }, async move {
            repo.check_stash(index, &hash).await?;
            if pop { repo.stash_pop(index, restore).await } else { repo.stash_apply(index, restore).await }
        });
    }

    pub fn stash_drop(&mut self, stash: &Stash) {
        let Some(repo) = self.repo.clone() else { return };
        let (index, hash) = (stash.index, stash.hash.clone());
        self.op("stash_drop", async move {
            repo.check_stash(index, &hash).await?;
            repo.stash_drop(index).await
        });
    }

    /// Shows a stash and loads the files it saved.
    pub fn select_stash(&mut self, hash: &str) {
        self.selected_stash = Some(hash.to_string());
        self.stash_files.clear();
        let Some(repo) = self.repo.clone() else { return };
        let out = self.inbox.sender();
        let h = hash.to_string();
        Runtime::spawn(async move {
            if let Ok(files) = repo.stash_files(&h).await {
                out.send(VcsEvent::StashFiles(h, files));
            }
        });
    }

    /// Opens the diff of a file saved in a stash: the commit it was made on ↔ the stash
    /// (an untracked file is compared with nothing).
    pub fn open_stash_diff(&mut self, hash: &str, file: &rc_vcs::ChangedFile) {
        let Some(repo) = self.repo.clone() else { return };
        let out = self.inbox.sender();
        let (h, rel, untracked) = (hash.to_string(), file.path.clone(), file.status == '?');
        let base = file.orig_path.clone().unwrap_or_else(|| rel.clone());
        Runtime::spawn(async move {
            let text = |c: Option<rc_vcs::FileContent>| c.map(|c| c.text().into_owned()).unwrap_or_default();
            let left = if untracked { String::new() } else { text(repo.show_file(&Revision::Commit(format!("{h}^1")), &base).await.ok().flatten()) };
            let right_rev = if untracked { format!("{h}^3") } else { h.clone() };
            let right = text(repo.show_file(&Revision::Commit(right_rev), &rel).await.ok().flatten());
            out.send(VcsEvent::Diff(format!("{rel} ({})", tr("Stash")), repo.abs_path(&rel), left, right));
        });
    }

    // ------------------------------------------------------------------ patches

    /// The commits a patch made from `clicked` covers: the marked ones when it is among them,
    /// else just `clicked`; newest first, as in the log.
    pub fn patch_commits(&self, clicked: &str) -> Vec<String> {
        if self.marked_commits.len() > 1 && self.marked_commits.contains(clicked) {
            self.log.iter().filter(|c| self.marked_commits.contains(&c.hash)).map(|c| c.hash.clone()).collect()
        } else {
            vec![clicked.to_string()]
        }
    }

    /// Marks commits of the log before `hash` is selected: `toggle` (Ctrl/Cmd-click) adds or
    /// removes one, `range` (Shift-click) marks every commit between the selected one and
    /// `hash`, and a plain click clears the marks.
    pub fn mark_commit(&mut self, hash: &str, toggle: bool, range: bool) {
        if range && let Some(anchor) = self.selected_commit.clone() {
            let at = |h: &str| self.log.iter().position(|c| c.hash == h);
            if let (Some(a), Some(b)) = (at(&anchor), at(hash)) {
                let (lo, hi) = (a.min(b), a.max(b));
                self.marked_commits = self.log[lo..=hi].iter().map(|c| c.hash.clone()).collect();
                return;
            }
        }
        if toggle {
            if self.marked_commits.is_empty()
                && let Some(s) = &self.selected_commit
            {
                self.marked_commits.insert(s.clone());
            }
            if !self.marked_commits.remove(hash) {
                self.marked_commits.insert(hash.to_string());
            }
        } else {
            self.marked_commits.clear();
        }
    }

    /// Creates a patch and saves it to `file`, or puts it on the clipboard (`None`).
    pub fn create_patch(&mut self, source: PatchSource, file: Option<PathBuf>) {
        let Some(repo) = self.repo.clone() else { return };
        let out = self.inbox.sender();
        let what = if file.is_some() { "patch_save" } else { "patch_copy" };
        self.op(what, async move {
            let patch = match &source {
                PatchSource::Changes(paths) => repo.diff_patch(paths).await?,
                PatchSource::Commits(commits) => repo.format_patch(commits).await?,
            };
            match file {
                Some(path) => Runtime::handle().spawn_blocking(move || std::fs::write(path, patch)).await.map_err(std::io::Error::other)??,
                None => {
                    out.send(VcsEvent::PatchClipboard(String::from_utf8_lossy(&patch).into_owned()));
                }
            }
            Ok(())
        });
    }

    /// Reads a patch file in the background and checks what applying it would do.
    pub fn load_patch_file(&mut self, path: PathBuf) {
        let Some(repo) = self.repo.clone() else { return };
        let generation = self.start_patch_load();
        let three_way = self.patch_three_way;
        let out = self.inbox.sender();
        Runtime::spawn(async move {
            let file = path.clone();
            let text = match Runtime::handle().spawn_blocking(move || read_patch_file(&file)).await {
                Ok(Ok(t)) => Arc::new(t),
                Ok(Err(e)) => {
                    out.send(VcsEvent::PatchLoaded(generation, Some(path), Err(detailed_error(&e))));
                    return;
                }
                Err(e) => {
                    out.send(VcsEvent::PatchLoaded(generation, Some(path), Err(e.to_string())));
                    return;
                }
            };
            let preview = repo.preview_patch(&text, three_way).await.map_err(|e| detailed_error(&e));
            out.send(VcsEvent::PatchLoaded(generation, Some(path), Ok((text, preview))));
        });
    }

    /// Takes a patch pasted from the clipboard and checks what applying it would do.
    pub fn load_patch_text(&mut self, text: String) {
        let Some(repo) = self.repo.clone() else { return };
        let generation = self.start_patch_load();
        if let Err(e) = rc_vcs::check_patch_size(text.len() as u64) {
            self.patch_loading = false;
            self.patch_error = Some(user_error(&e));
            return;
        }
        let three_way = self.patch_three_way;
        let out = self.inbox.sender();
        let text = Arc::new(text.into_bytes());
        Runtime::spawn(async move {
            let preview = repo.preview_patch(&text, three_way).await.map_err(|e| detailed_error(&e));
            out.send(VcsEvent::PatchLoaded(generation, None, Ok((text, preview))));
        });
    }

    fn start_patch_load(&mut self) -> u64 {
        self.patch_gen += 1;
        self.patch = None;
        self.patch_loading = true;
        self.patch_error = None;
        self.patch_gen
    }

    /// Checks the loaded patch again (after the three-way option changed).
    pub fn recheck_patch(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        let Some(p) = &mut self.patch else { return };
        p.preview = None;
        p.result = None;
        let (text, three_way, generation) = (p.text.clone(), self.patch_three_way, self.patch_gen);
        let out = self.inbox.sender();
        Runtime::spawn(async move {
            let preview = repo.preview_patch(&text, three_way).await.map_err(|e| detailed_error(&e));
            out.send(VcsEvent::PatchPreview(generation, preview));
        });
    }

    /// Applies the loaded patch (`git apply`, or `git am` for mails); what git printed goes to
    /// the patch's `result`.
    pub fn apply_patch(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        let Some(p) = &mut self.patch else { return };
        p.result = None;
        let (text, three_way, generation) = (p.text.clone(), self.patch_three_way, self.patch_gen);
        self.busy = Some("patch_apply");
        self.error = None;
        let out = self.inbox.sender();
        Runtime::spawn(async move {
            match repo.apply_patch(&text, three_way).await {
                Ok(text) => {
                    out.send(VcsEvent::PatchApplied(generation, OpOutput { text, error: false }));
                    out.send(VcsEvent::Done("patch_apply", None));
                }
                Err(e) => {
                    out.send(VcsEvent::PatchApplied(generation, OpOutput { text: detailed_error(&e), error: true }));
                    out.send(VcsEvent::Done("patch_apply", Some(user_error(&e))));
                }
            }
        });
    }

    /// Forgets the patch of the Apply Patch dialog.
    pub fn close_patch(&mut self) {
        self.patch_gen += 1;
        self.patch = None;
        self.patch_loading = false;
        self.patch_error = None;
    }

    /// The changes that go into the next commit, relative to the repository.
    pub fn included(&self) -> Vec<String> {
        self.changes().into_iter().map(|c| c.path).filter(|p| !self.excluded.contains(p)).collect()
    }

    /// The folder of the repository.
    pub fn root(&self) -> Option<&Path> {
        self.repo.as_ref().map(|r| r.root())
    }

    /// Commits the included changes (and pushes when `push`), formatting `format` first; if
    /// that fails nothing is committed.
    pub fn commit(&mut self, push: bool, format: Option<(Result<rc_cargo::Rustfmt, String>, Vec<rc_cargo::FormatFile>)>) {
        let Some(repo) = self.repo.clone() else { return };
        let only = self.included();
        if only.is_empty() && !self.amend {
            self.error = Some(tr("Select the files to commit").into());
            return;
        }
        if self.message.trim().is_empty() {
            self.error = Some(tr("Write a commit message").into());
            return;
        }
        let opts = CommitOptions { message: self.message.trim().to_string(), amend: self.amend, only, ..Default::default() };
        self.op(if push { "commit_push" } else { "commit" }, async move {
            if let Some((rustfmt, files)) = format {
                let rustfmt = rustfmt.map_err(|e| rc_vcs::GitError::BeforeCommit(trf("Not committed: {error}", &[("error", &e)])))?;
                rc_cargo::format_files(&rustfmt, &files).await.map_err(|e| rc_vcs::GitError::BeforeCommit(format_failure(e)))?;
            }
            repo.commit(&opts).await?;
            if push {
                repo.push(&PushOptions::default(), None, None).await?;
            }
            Ok(())
        });
    }

    pub fn push(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        let upstream = self.status.as_ref().and_then(|s| s.branch.upstream.clone());
        let branch = self.status.as_ref().and_then(|s| s.branch.head.clone());
        self.op("push", async move {
            let opts = if upstream.is_none() { PushOptions { refspec: branch, set_upstream: true, ..Default::default() } } else { PushOptions::default() };
            repo.push(&opts, None, None).await
        });
    }

    pub fn pull(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        self.op("pull", async move { repo.pull(&PullOptions::default(), None, None).await });
    }

    pub fn fetch(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        self.op("fetch", async move { repo.fetch(&rc_vcs::FetchOptions { prune: true, ..Default::default() }, None, None).await });
    }

    pub fn checkout(&mut self, branch: &Branch) {
        let Some(repo) = self.repo.clone() else { return };
        let b = branch.clone();
        self.op("checkout", async move {
            if b.kind == rc_vcs::BranchKind::Remote {
                let local = b.name.split_once('/').map_or(b.name.clone(), |(_, n)| n.to_string());
                repo.checkout_remote_branch(&b.name, Some(&local)).await
            } else {
                repo.checkout_branch(&b.name).await
            }
        });
    }

    pub fn new_branch(&mut self, name: &str) {
        let Some(repo) = self.repo.clone() else { return };
        let n = name.to_string();
        self.op("branch", async move { repo.create_branch(&n, None, true).await });
    }

    /// Rollback: restores files from HEAD (new files are only unstaged).
    pub fn rollback(&mut self, paths: Vec<String>) {
        let Some(repo) = self.repo.clone() else { return };
        self.op("rollback", async move { repo.rollback(&paths).await });
    }

    // ------------------------------------------------------------------ diffs and blame

    /// Opens the diff of a working-tree file against HEAD.
    pub fn open_diff(&mut self, rel: &str) {
        let Some(repo) = self.repo.clone() else { return };
        let out = self.inbox.sender();
        let rel = rel.to_string();
        Runtime::spawn(async move {
            let abs = repo.abs_path(&rel);
            let left = repo.show_file(&Revision::Head, &rel).await.ok().flatten().map(|c| c.text().into_owned()).unwrap_or_default();
            let right = std::fs::read_to_string(&abs).unwrap_or_default();
            out.send(VcsEvent::Diff(format!("{rel} ({} ↔ {})", tr("HEAD"), tr("Working tree")), abs, left, right));
        });
    }

    /// Opens the diff of a file in a commit (parent ↔ commit).
    pub fn open_commit_diff(&mut self, hash: &str, rel: &str) {
        let Some(repo) = self.repo.clone() else { return };
        let out = self.inbox.sender();
        let (h, rel) = (hash.to_string(), rel.to_string());
        Runtime::spawn(async move {
            let left = repo.show_file(&Revision::Commit(format!("{h}^")), &rel).await.ok().flatten().map(|c| c.text().into_owned()).unwrap_or_default();
            let right = repo.show_file(&Revision::Commit(h.clone()), &rel).await.ok().flatten().map(|c| c.text().into_owned()).unwrap_or_default();
            let short = &h[..h.len().min(8)];
            out.send(VcsEvent::Diff(format!("{rel} ({short})"), repo.abs_path(&rel), left, right));
        });
    }

    /// Shows two texts side by side in the editor area (the local history's versions).
    pub fn show_diff(&mut self, title: String, path: PathBuf, left: &str, right: &str, dark: bool) {
        self.diff = Some(self.make_diff(title, path, left, right, dark));
    }

    pub fn toggle_blame(&mut self, path: &Path, text: String, docs: &mut Docs) {
        if self.blame_for.as_deref() == Some(path) {
            self.blame_for = None;
            if let Some(i) = docs.index_of(path) {
                docs.list[i].editor.annotations = None;
            }
            return;
        }
        let Some(repo) = self.repo.clone() else { return };
        let Some(rel) = repo.rel_path(path) else { return };
        self.blame_for = Some(path.to_path_buf());
        let out = self.inbox.sender();
        let p = path.to_path_buf();
        Runtime::spawn(async move {
            let b = repo.blame(&rel, Some(&text), None, None).await.map_err(|e| e.to_string());
            out.send(VcsEvent::Blame(p, b));
        });
    }

    // ------------------------------------------------------------------ per frame

    pub fn update(&mut self, docs: &mut Docs, dark: bool) -> Vec<String> {
        let mut notices = Vec::new();
        for ev in self.inbox.drain() {
            match ev {
                VcsEvent::Repo(r) => {
                    self.repo = r;
                    if self.repo.is_some() {
                        self.request_status();
                        self.load_log();
                    }
                }
                VcsEvent::Status(Ok(s)) => {
                    // Files that left the change list lose their exclusion.
                    let paths: BTreeSet<String> = s.entries.iter().map(|e| e.path.clone()).collect();
                    self.excluded.retain(|p| paths.contains(p));
                    self.status = Some(s);
                    self.status_version += 1;
                }
                VcsEvent::Status(Err(e)) => self.error = Some(e),
                VcsEvent::Branches(b) => self.branches = b,
                VcsEvent::Log(l) => self.log = l,
                VcsEvent::Head(p, t) => {
                    self.head_pending.remove(&p);
                    self.heads.insert(p, t);
                    self.marks_version.clear();
                }
                VcsEvent::Blame(p, Ok(b)) => {
                    if self.blame_for.as_deref() == Some(p.as_path())
                        && let Some(i) = docs.index_of(&p)
                    {
                        docs.list[i].editor.annotations = Some(
                            b.lines
                                .iter()
                                .map(|l| {
                                    let c = &b.commits[l.commit];
                                    if c.is_uncommitted() { tr("Not committed yet").to_string() } else { format!("{}  {}", format_date(c.author_time), short_author(&c.author)) }
                                })
                                .collect(),
                        );
                    }
                }
                VcsEvent::Blame(_, Err(e)) => {
                    self.blame_for = None;
                    notices.push(e);
                }
                VcsEvent::Done(what, err) => {
                    self.busy = None;
                    match err {
                        Some(e) => {
                            self.error = Some(e.clone());
                            notices.push(e);
                            // A failed operation may still have changed something (a pop
                            // with conflicts applies the changes), or found the lists stale.
                            self.refresh();
                        }
                        None => {
                            if what == "commit" || what == "commit_push" {
                                self.message.clear();
                                self.amend = false;
                                notices.push(if what == "commit" { tr("Committed").to_string() } else { tr("Committed and pushed").to_string() });
                            } else if what == "push" {
                                notices.push(tr("Pushed").to_string());
                            } else if what == "pull" {
                                notices.push(tr("Project updated").to_string());
                            } else if let Some(text) = done_notice(what) {
                                notices.push(text.to_string());
                            }
                            self.refresh();
                        }
                    }
                }
                VcsEvent::CommitFiles(h, f) => {
                    if self.selected_commit.as_deref() == Some(h.as_str()) {
                        self.commit_files = f;
                    }
                }
                VcsEvent::Diff(title, path, left, right) => self.diff = Some(self.make_diff(title, path, &left, &right, dark)),
                VcsEvent::Tags(t) => {
                    if self.selected_tag.as_ref().is_some_and(|s| !t.iter().any(|x| &x.name == s)) {
                        self.selected_tag = None;
                    }
                    self.tags = t;
                }
                VcsEvent::Stashes(list) => {
                    if self.selected_stash.as_ref().is_some_and(|h| !list.iter().any(|x| &x.hash == h)) {
                        self.selected_stash = None;
                        self.stash_files.clear();
                    }
                    self.stashes = list;
                }
                VcsEvent::StashFiles(h, f) => {
                    if self.selected_stash.as_deref() == Some(h.as_str()) {
                        self.stash_files = f;
                    }
                }
                VcsEvent::Remotes(r) => self.remotes = r,
                VcsEvent::Output(o) => self.output = Some(o),
                VcsEvent::PatchClipboard(text) => self.clipboard = Some(text),
                VcsEvent::PatchLoaded(generation, path, loaded) if generation == self.patch_gen => {
                    self.patch_loading = false;
                    match loaded {
                        Ok((text, preview)) => self.patch = Some(PendingPatch { path, text, preview: Some(preview), result: None }),
                        Err(e) => self.patch_error = Some(e),
                    }
                }
                VcsEvent::PatchPreview(generation, preview) if generation == self.patch_gen => {
                    if let Some(p) = &mut self.patch {
                        p.preview = Some(preview);
                    }
                }
                VcsEvent::PatchApplied(generation, output) if generation == self.patch_gen => {
                    if let Some(p) = &mut self.patch {
                        p.result = Some(output);
                    }
                }
                VcsEvent::PatchLoaded(..) | VcsEvent::PatchPreview(..) | VcsEvent::PatchApplied(..) => {}
                VcsEvent::Worktrees(list) => {
                    // A worktree being added is selected before it is listed.
                    if self.busy != Some("worktree_add") && self.selected_worktree.as_ref().is_some_and(|p| !list.iter().any(|w| &w.path == p)) {
                        self.selected_worktree = None;
                    }
                    self.worktrees = list;
                }
                VcsEvent::RemoveRefused(p) => self.force_remove = Some(p),
            }
        }
        if self.repo.is_some() && self.status_dirty && self.status_requested.is_none_or(|t| t.elapsed() > Duration::from_millis(700)) {
            self.request_status();
        }
        self.update_marks(docs);
        notices
    }

    /// Gutter marks of the active document against its HEAD version.
    fn update_marks(&mut self, docs: &mut Docs) {
        let Some(repo) = self.repo.clone() else { return };
        let Some(i) = docs.active else { return };
        let doc = &mut docs.list[i];
        let path = doc.path.clone();
        if !self.heads.contains_key(&path) {
            if self.head_pending.insert(path.clone()) {
                let out = self.inbox.sender();
                Runtime::spawn(async move {
                    let text = match repo.rel_path(&path) {
                        Some(rel) => repo.show_file(&Revision::Head, &rel).await.ok().flatten().map(|c| c.text().into_owned()),
                        None => None,
                    };
                    out.send(VcsEvent::Head(path, text));
                });
            }
            return;
        }
        let version = doc.editor.buffer.version();
        if self.marks_version.get(&doc.editor.id) == Some(&version) {
            return;
        }
        self.marks_version.insert(doc.editor.id, version);
        doc.editor.line_changes.clear();
        let Some(Some(head)) = self.heads.get(&path) else {
            // Not in HEAD: a new file is all added (when it is inside the repository).
            if repo.rel_path(&path).is_some() && self.heads.get(&path).is_some_and(|h| h.is_none()) && !self.is_ignored(&path) {
                for l in 0..doc.editor.buffer.len_lines() {
                    doc.editor.line_changes.insert(l, rc_editor::LineChange::Added);
                }
            }
            return;
        };
        let changes = LineChanges::compute(head, &doc.editor.buffer.rope().to_string());
        for c in &changes.changes {
            match c.kind {
                LineChangeKind::Added => c.new.clone().for_each(|l| {
                    doc.editor.line_changes.insert(l, rc_editor::LineChange::Added);
                }),
                LineChangeKind::Modified => c.new.clone().for_each(|l| {
                    doc.editor.line_changes.insert(l, rc_editor::LineChange::Modified);
                }),
                LineChangeKind::Deleted => {
                    doc.editor.line_changes.insert(c.new.start.saturating_sub(1), rc_editor::LineChange::Deleted);
                }
            }
        }
    }

    fn is_ignored(&self, path: &Path) -> bool {
        let Some(repo) = &self.repo else { return true };
        let Some(rel) = repo.rel_path(path) else { return true };
        // Untracked files show in the status; files missing from it are clean or ignored.
        !self.status.as_ref().is_some_and(|s| s.entries.iter().any(|e| e.path == rel))
    }

    fn make_diff(&mut self, title: String, path: PathBuf, left: &str, right: &str, dark: bool) -> DiffView {
        let sbs = rc_vcs::SideBySide::compute(left, right);
        let ll: Vec<&str> = left.split('\n').collect();
        let rl: Vec<&str> = right.split('\n').collect();
        let (mut lt, mut rt) = (Vec::new(), Vec::new());
        let s = rc_kit::theme::scheme(dark);
        let removed = s.deleted_lines.gamma_multiply(0.22);
        let added = s.added_lines.gamma_multiply(if dark { 0.55 } else { 0.6 });
        let modified = s.modified_lines.gamma_multiply(if dark { 0.55 } else { 0.6 });
        let filler = rc_kit::theme::palette(dark).header_bg;
        let mut lbg = std::collections::BTreeMap::new();
        let left_numbers: Vec<Option<usize>> = sbs.rows.iter().map(|r| r.left).collect();
        let right_numbers: Vec<Option<usize>> = sbs.rows.iter().map(|r| r.right).collect();
        let mut rbg = std::collections::BTreeMap::new();
        for (row, r) in sbs.rows.iter().enumerate() {
            lt.push(r.left.and_then(|i| ll.get(i)).map(|s| s.trim_end_matches('\r')).unwrap_or(""));
            rt.push(r.right.and_then(|i| rl.get(i)).map(|s| s.trim_end_matches('\r')).unwrap_or(""));
            match r.kind {
                rc_vcs::RowKind::Equal => {}
                rc_vcs::RowKind::Added => {
                    lbg.insert(row, filler);
                    rbg.insert(row, added);
                }
                rc_vcs::RowKind::Deleted => {
                    lbg.insert(row, removed);
                    rbg.insert(row, filler);
                }
                rc_vcs::RowKind::Modified => {
                    lbg.insert(row, modified);
                    rbg.insert(row, modified);
                }
            }
        }
        let mut make = |text: String, bg, numbers| {
            self.next_doc_id += 1;
            let mut e = rc_editor::EditorState::new(self.next_doc_id, Some(path.clone()), &text);
            e.read_only = true;
            e.line_backgrounds = bg;
            e.display_numbers = Some(numbers);
            e.parse_blocking();
            e
        };
        DiffView { title, path: path.clone(), left: make(lt.join("\n"), lbg, left_numbers), right: make(rt.join("\n"), rbg, right_numbers) }
    }
}

/// The translated summary of a git failure.
fn user_error(e: &rc_vcs::GitError) -> String {
    let (tpl, args) = e.user_message();
    let args: Vec<(&str, &dyn std::fmt::Display)> = args.iter().map(|(k, v)| (*k, v as &dyn std::fmt::Display)).collect();
    trf(tpl, &args)
}

/// The summary of a git failure followed by git's own message, if any.
fn detailed_error(e: &rc_vcs::GitError) -> String {
    let summary = user_error(e);
    match e.details().filter(|d| !d.trim().is_empty()) {
        Some(d) => format!("{summary}\n{}", d.trim()),
        None => summary,
    }
}

/// Reads a patch file, refusing files larger than [`rc_vcs::MAX_PATCH_BYTES`].
fn read_patch_file(path: &Path) -> Result<Vec<u8>, rc_vcs::GitError> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    rc_vcs::check_patch_size(file.metadata()?.len())?;
    let mut text = Vec::new();
    file.take(rc_vcs::MAX_PATCH_BYTES as u64 + 1).read_to_end(&mut text)?;
    rc_vcs::check_patch_size(text.len() as u64)?;
    Ok(text)
}

/// What to tell the user when an operation of the Git window succeeds.
fn done_notice(what: &str) -> Option<&'static str> {
    Some(match what {
        "tag" => tr("Tag created"),
        "tag_delete" => tr("Tag deleted"),
        "tag_push" => tr("Tags pushed"),
        "tag_checkout" => tr("Checked out the tag; HEAD is detached"),
        "stash" => tr("Changes stashed"),
        "stash_apply" => tr("Stash applied"),
        "stash_pop" => tr("Stash applied and dropped"),
        "stash_drop" => tr("Stash dropped"),
        "patch_save" => tr("Patch saved"),
        "patch_copy" => tr("Patch copied to the clipboard"),
        "patch_apply" => tr("Patch applied"),
        "worktree_add" => tr("Worktree created"),
        "worktree_remove" => tr("Worktree removed"),
        "worktree_lock" => tr("Worktree locked"),
        "worktree_unlock" => tr("Worktree unlocked"),
        "worktree_prune" => tr("Worktrees pruned"),
        _ => return None,
    })
}

fn short_author(a: &str) -> String {
    a.split_whitespace().next().unwrap_or(a).to_string()
}

/// `2026-10-24` from Unix seconds (UTC).
pub fn format_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(super::format_date(0), "1970-01-01");
        assert_eq!(super::format_date(1_700_000_000), "2023-11-14");
    }
}

fn format_failure(error: rc_cargo::FormatError) -> String {
    match error {
        rc_cargo::FormatError::File { path, message } => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let first = message.lines().next().unwrap_or_default().to_string();
            trf("Not committed: rustfmt failed on {file}: {error}", &[("file", &name), ("error", &first)])
        }
        other => rustfmt_failure(other),
    }
}

/// Why rustfmt cannot run: the tool is missing or its settings are wrong.
pub fn rustfmt_failure(error: rc_cargo::FormatError) -> String {
    match error {
        rc_cargo::FormatError::Missing => tr("rustfmt was not found. Install it with: rustup component add rustfmt").to_string(),
        rc_cargo::FormatError::Arguments => tr("The rustfmt arguments have a quote that is not closed (Settings › Tools).").to_string(),
        rc_cargo::FormatError::Channel(channel) => trf("The rustfmt channel \"{channel}\" is not a toolchain name (Settings › Tools).", &[("channel", &channel)]),
        rc_cargo::FormatError::Environment(line) => trf("The rustfmt environment takes NAME=value lines, not \"{line}\" (Settings › Tools).", &[("line", &line)]),
        rc_cargo::FormatError::File { path, message } => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let first = message.lines().next().unwrap_or_default().to_string();
            trf("rustfmt failed on {file}: {error}", &[("file", &name), ("error", &first)])
        }
    }
}
