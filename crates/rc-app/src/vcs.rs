//! Git integration: the repository of the workspace, its status, branches and history, the
//! gutter change marks of open files, commits and remote operations. Every git call runs on the
//! shared runtime; results come back through an inbox.

use crate::docs::Docs;
use rc_core::{Inbox, Runtime, Waker};
use rc_kit::i18n::{tr, trf};
use rc_vcs::{
    Blame, Branch, Commit, CommitOptions, FileState, LineChangeKind, LineChanges, LogOptions, PatchPreview, PullOptions, PushOptions, Repository, Revision,
    Stash, StashOptions, Status, StatusOptions, Tag, Worktree, WorktreeCheckout,
};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

type LoadedPatch = Result<(Arc<Vec<u8>>, Result<PatchPreview, String>), String>;

enum VcsEvent {
    Repo(Option<Repository>),
    Status(Result<Status, String>),
    Branches(Vec<Branch>),
    /// A page of the log: (generation of the filter, appended to the loaded commits, commits).
    Log(u64, bool, Vec<Commit>),
    Head(PathBuf, Option<String>),
    Blame(PathBuf, Result<Blame, String>),
    /// An operation finished: (what, error).
    Done(&'static str, Option<String>),
    CommitFiles(String, Vec<rc_vcs::ChangedFile>),
    /// The signature of a commit of the log (V-14).
    Signature(String, rc_vcs::Signature),
    /// The repository's signing settings.
    Signing(rc_vcs::SigningConfig),
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
    PatchLoaded(u64, Option<PathBuf>, LoadedPatch),
    /// A new preview of the loaded patch (generation, preview).
    PatchPreview(u64, Result<PatchPreview, String>),
    /// What applying the loaded patch printed (generation, output).
    PatchApplied(u64, OpOutput),
    Worktrees(Vec<Worktree>),
    /// Removing this worktree was refused because it has local changes.
    RemoveRefused(PathBuf),
    /// The merge, rebase, cherry-pick or revert in progress (V-07), with the number of the
    /// status request that read it.
    Operation(u64, rc_vcs::Operation),
    /// A progress line of the running merge, rebase or cherry-pick.
    OpProgress(String),
    /// What the last merge, rebase or cherry-pick printed.
    OpOutput(OpOutput),
    /// The base, ours and theirs of a conflicted file for the merge tool (V-08), or why not.
    MergeLoaded(String, Result<(String, String, String), String>),
    /// The merge tool's result was written and staged (or why not).
    MergeSaved(String, Option<String>),
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

/// Commits read per page of the log.
pub const LOG_PAGE: usize = 500;

/// What the Log shows (V-06): every commit by default, or those matching the filters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogFilter {
    /// Words of the commit message (`--grep`, case-insensitive).
    pub text: String,
    /// Part of the author's name or email (`--author`).
    pub author: String,
    /// One branch (its short name) instead of every ref.
    pub branch: Option<String>,
    /// One file or folder, relative to the repository; a file's renames are followed.
    pub path: Option<String>,
}

impl LogFilter {
    pub fn is_empty(&self) -> bool {
        *self == LogFilter::default()
    }

    /// The `git log` options for a page starting at `skip`.
    pub fn options(&self, skip: usize) -> LogOptions {
        let text = self.text.trim();
        let author = self.author.trim();
        let mut opts = LogOptions { skip, max_count: LOG_PAGE, ..Default::default() };
        // Typed text is matched literally, not as a regular expression.
        opts.grep = (!text.is_empty()).then(|| regex_escape(text));
        opts.author = (!author.is_empty()).then(|| regex_escape(author));
        match &self.branch {
            Some(b) => opts.revs = vec![b.clone()],
            // `--follow` works from one starting point.
            None if self.path.is_some() => opts.revs = vec!["HEAD".into()],
            None => opts.all = true,
        }
        if let Some(p) = &self.path {
            // A folder (written with a trailing `/`) has no renames to follow.
            opts.follow = !p.ends_with('/');
            opts.paths = vec![p.trim_end_matches('/').to_string()].into_iter().map(|p| if p.is_empty() { ".".into() } else { p }).collect();
        }
        opts
    }
}

/// `text` as an extended regular expression that matches it literally.
fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The views of the Git tool window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GitTab {
    #[default]
    Log,
    Tags,
    Stashes,
    Shelves,
    HistoryEdit,
    Issues,
    Hosting,
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
    /// Set only by the application trust store; untrusted open starts no Git process.
    pub trusted: bool,
    pub repo: Option<Repository>,
    pub status: Option<Status>,
    pub branches: Vec<Branch>,
    pub log: Vec<Commit>,
    /// What the Log shows (V-06).
    pub log_filter: LogFilter,
    /// The last page was full: there may be older commits.
    pub log_more: bool,
    /// A page of the log is being read.
    pub log_loading: bool,
    /// The text and author fields of the Log as typed, applied on Enter or focus loss.
    pub log_text_draft: String,
    pub log_author_draft: String,
    log_gen: u64,
    pub selected_commit: Option<String>,
    pub commit_files: Vec<rc_vcs::ChangedFile>,
    /// The signature of the selected commit, once git has verified it.
    pub signature: Option<(String, rc_vcs::Signature)>,
    /// The repository's signing settings (`commit.gpgsign`, `user.signingkey`).
    pub signing: Option<rc_vcs::SigningConfig>,
    /// Sign the next commit; starts as `commit.gpgsign` says until the user changes it.
    pub sign: bool,
    pub sign_touched: bool,
    pub message: String,
    pub amend: bool,
    pub excluded: BTreeSet<String>,
    pub changelists: rc_core::changelists::Changelists,
    pub changelist_name: String,
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
    /// The merge, rebase, cherry-pick or revert git has in progress (V-07).
    pub operation: rc_vcs::Operation,
    /// The last progress line of the running merge, rebase or cherry-pick.
    pub op_progress: Option<String>,
    /// What the last merge, rebase or cherry-pick printed, or why it stopped.
    pub op_output: Option<OpOutput>,
    /// Cancels the running merge, rebase or cherry-pick (kills git).
    op_cancel: Option<rc_vcs::CancelToken>,
    /// The three-pane merge tool shown in the editor area (V-08).
    pub merge_tool: Option<crate::merge_tool::MergeTool>,
    /// A conflicted file is being read for the merge tool.
    pub merge_loading: Option<String>,
    /// Status requests made, and the newest whose operation was applied (reads can overlap).
    status_seq: u64,
    operation_seq: u64,
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
            trusted: false,
            repo: None,
            status: None,
            branches: Vec::new(),
            log: Vec::new(),
            log_filter: LogFilter::default(),
            log_more: false,
            log_loading: false,
            log_text_draft: String::new(),
            log_author_draft: String::new(),
            log_gen: 0,
            selected_commit: None,
            commit_files: Vec::new(),
            signature: None,
            signing: None,
            sign: false,
            sign_touched: false,
            message: String::new(),
            amend: false,
            excluded: BTreeSet::new(),
            changelists: Default::default(),
            changelist_name: String::new(),
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
            operation: rc_vcs::Operation::default(),
            op_progress: None,
            op_output: None,
            op_cancel: None,
            merge_tool: None,
            merge_loading: None,
            status_seq: 0,
            operation_seq: 0,
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

    pub fn open(&mut self, root: &Path, trusted: bool) {
        *self = Vcs::new(self.waker.clone());
        self.trusted = trusted;
        if !trusted {
            return;
        }
        let out = self.inbox.sender();
        let root = root.to_path_buf();
        Runtime::spawn(async move {
            out.send(VcsEvent::Repo(Repository::discover(&root).await.ok().map(|repo| repo.with_trust(true))));
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
        if s.contains("/.git/") && (s.ends_with("HEAD") || s.contains("/refs/") || s.ends_with("index")) {
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
        self.status_seq += 1;
        let seq = self.status_seq;
        self.status_dirty = false;
        let out = self.inbox.sender();
        Runtime::spawn(async move {
            let st = repo.status(&StatusOptions::default()).await.map_err(|e| e.to_string());
            out.send(VcsEvent::Status(st));
            out.send(VcsEvent::Operation(seq, repo.operation().await));
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
            if let Ok(c) = repo.signing_config().await {
                out.send(VcsEvent::Signing(c));
            }
        });
    }

    /// Reads the first page of the log for the current filter.
    pub fn load_log(&mut self) {
        self.log_gen += 1;
        self.read_log(0);
    }

    /// Reads the next page of the log (Load More).
    pub fn load_more_log(&mut self) {
        if self.log_more && !self.log_loading {
            self.read_log(self.log.len());
        }
    }

    fn read_log(&mut self, skip: usize) {
        let Some(repo) = self.repo.clone() else { return };
        let out = self.inbox.sender();
        let generation = self.log_gen;
        let opts = self.log_filter.options(skip);
        self.log_loading = true;
        Runtime::spawn(async move {
            // A filter git refuses (a bad pattern) shows an empty log.
            let log = repo.log(&opts).await.unwrap_or_default();
            out.send(VcsEvent::Log(generation, skip > 0, log));
        });
    }

    /// Changes what the Log shows and reads it again.
    pub fn set_log_filter(&mut self, filter: LogFilter) {
        self.log_text_draft = filter.text.clone();
        self.log_author_draft = filter.author.clone();
        if filter != self.log_filter {
            self.log_filter = filter;
            self.selected_commit = None;
            self.marked_commits.clear();
            self.load_log();
        }
    }

    /// The history of one file (`rel`, relative to the repository), renames followed.
    pub fn show_file_history(&mut self, path: &Path) {
        let Some(mut rel) = self.repo.as_ref().and_then(|r| r.rel_path(path)) else { return };
        if path.is_dir() {
            rel.push('/');
        }
        self.tab = GitTab::Log;
        self.set_log_filter(LogFilter { path: Some(rel), ..LogFilter::default() });
    }

    pub fn select_commit(&mut self, hash: &str) {
        self.selected_commit = Some(hash.to_string());
        self.commit_files.clear();
        self.signature = None;
        let Some(repo) = self.repo.clone() else { return };
        let out = self.inbox.sender();
        let h = hash.to_string();
        Runtime::spawn(async move {
            if let Ok(files) = repo.commit_files(&h).await {
                out.send(VcsEvent::CommitFiles(h.clone(), files));
            }
            // Checking a signature runs the signing program: after the files.
            if let Ok(sig) = repo.signature(&h).await {
                out.send(VcsEvent::Signature(h, sig));
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
        if !self.trusted {
            self.error = Some(tr("Trust this project before running project tools or code").into());
            return;
        }
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

    // ------------------------------------------------------------------ merge, rebase, cherry-pick

    /// Paths with merge conflicts, as of the last status.
    pub fn conflicts(&self) -> Vec<String> {
        self.status.as_ref().map(|s| s.conflicts().map(|e| e.path.clone()).collect()).unwrap_or_default()
    }

    /// A cancellable Git operation is running.
    pub fn sequence_running(&self) -> bool {
        self.op_cancel.is_some()
    }

    /// Merges `rev` into the current branch.
    pub fn merge(&mut self, rev: &str, opts: rc_vcs::MergeOptions) {
        let Some(repo) = self.repo.clone() else { return };
        let rev = rev.to_string();
        self.sequence("merge", move |cancel, progress| async move { repo.merge_with(&rev, &opts, Some(&cancel), Some(progress)).await });
    }

    /// Rebases the current branch onto `onto`.
    pub fn rebase(&mut self, onto: &str, autostash: bool) {
        let Some(repo) = self.repo.clone() else { return };
        let onto = onto.to_string();
        self.sequence("rebase", move |cancel, progress| async move { repo.rebase(&onto, autostash, Some(&cancel), Some(progress)).await });
    }

    /// Applies commits (oldest first) on top of the current branch.
    pub fn cherry_pick(&mut self, commits: Vec<String>) {
        let Some(repo) = self.repo.clone() else { return };
        self.sequence("cherry_pick", move |cancel, _| async move { repo.cherry_pick(&commits, Some(&cancel)).await });
    }

    /// Continues the stopped merge, rebase or cherry-pick once its conflicts are resolved.
    pub fn continue_operation(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        self.sequence("op_continue", move |cancel, _| async move { repo.continue_operation(Some(&cancel)).await });
    }

    /// Skips the commit a rebase or cherry-pick stopped on.
    pub fn skip_operation(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        self.sequence("op_skip", move |cancel, _| async move { repo.skip_operation(Some(&cancel)).await });
    }

    /// Abandons the operation in progress.
    pub fn abort_operation(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        self.op_output = None;
        self.op("op_abort", async move { repo.abort_operation().await });
    }

    /// Stops the running merge, rebase or cherry-pick; what git had done stays in progress.
    pub fn cancel_operation(&mut self) {
        if let Some(c) = &self.op_cancel {
            c.cancel();
        }
    }

    /// Resolves conflicted files by taking our or their whole version, and marks them resolved.
    pub fn resolve_conflicts(&mut self, paths: Vec<String>, theirs: bool) {
        let Some(repo) = self.repo.clone() else { return };
        self.op("resolve", async move { repo.resolve_with(&paths, theirs).await });
    }

    /// Marks conflicted files resolved as they are in the work tree; refused while one still
    /// has conflict markers.
    pub fn mark_resolved(&mut self, paths: Vec<String>) {
        let Some(repo) = self.repo.clone() else { return };
        self.op("resolve", async move {
            let files: Vec<(String, PathBuf)> = paths.iter().map(|p| (p.clone(), repo.abs_path(p))).collect();
            let marked = Runtime::handle()
                .spawn_blocking(move || files.into_iter().find(|(_, abs)| has_conflict_markers(abs)).map(|(p, _)| p))
                .await
                .map_err(std::io::Error::other)?;
            if let Some(path) = marked {
                return Err(rc_vcs::GitError::BeforeCommit(trf("{file} still has conflict markers", &[("file", &path)])));
            }
            repo.mark_resolved(&paths).await
        });
    }

    /// Opens the three-pane merge tool on a conflicted file (V-08).
    pub fn open_merge_tool(&mut self, rel: &str) {
        let Some(repo) = self.repo.clone() else { return };
        let out = self.inbox.sender();
        let rel = rel.to_string();
        self.merge_loading = Some(rel.clone());
        Runtime::spawn(async move {
            let loaded = match repo.conflict_versions(&rel).await {
                Err(e) => Err(user_error(&e)),
                Ok(v) if [&v.base, &v.ours, &v.theirs].iter().any(|c| c.as_ref().is_some_and(|c| c.binary)) => {
                    Err(tr("Binary files cannot be merged here; accept one side instead").to_string())
                }
                Ok(rc_vcs::ConflictVersions { ours: None, .. } | rc_vcs::ConflictVersions { theirs: None, .. }) => {
                    Err(tr("One side deleted the file; accept one side instead").to_string())
                }
                Ok(v) => {
                    let text = |c: Option<rc_vcs::FileContent>| c.map(|c| c.text().into_owned()).unwrap_or_default();
                    Ok((text(v.base), text(v.ours), text(v.theirs)))
                }
            };
            out.send(VcsEvent::MergeLoaded(rel, loaded));
        });
    }

    /// Writes the merge tool's result to the file and marks it resolved.
    pub fn save_merge_tool(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        let Some(tool) = &mut self.merge_tool else { return };
        tool.saving = true;
        tool.error = None;
        let (rel, abs, text) = (tool.rel.clone(), tool.abs.clone(), tool.output());
        let out = self.inbox.sender();
        Runtime::spawn(async move {
            let written = Runtime::handle().spawn_blocking(move || std::fs::write(abs, text)).await.map_err(std::io::Error::other).and_then(|r| r);
            let result = match written {
                Err(e) => Some(e.to_string()),
                Ok(()) => repo.mark_resolved(std::slice::from_ref(&rel)).await.err().map(|e| user_error(&e)),
            };
            out.send(VcsEvent::MergeSaved(rel, result));
        });
    }

    /// Runs a cancellable Git operation, showing its progress and keeping what git
    /// printed (or why it stopped) kept in [`Vcs::op_output`].
    fn sequence<F, Fut>(&mut self, what: &'static str, f: F)
    where
        F: FnOnce(rc_vcs::CancelToken, rc_vcs::ProgressSink) -> Fut,
        Fut: std::future::Future<Output = Result<String, rc_vcs::GitError>> + Send + 'static,
    {
        if !self.trusted {
            self.error = Some(tr("Trust this project before running project tools or code").into());
            return;
        }
        let cancel = rc_vcs::CancelToken::new();
        self.op_cancel = Some(cancel.clone());
        self.op_progress = None;
        self.op_output = None;
        self.busy = Some(what);
        self.error = None;
        let out = self.inbox.sender();
        let lines = out.clone();
        let progress: rc_vcs::ProgressSink = Arc::new(move |p: rc_vcs::Progress| {
            lines.send(VcsEvent::OpProgress(p.text));
        });
        let work = f(cancel, progress);
        Runtime::spawn(async move {
            match work.await {
                Ok(text) => {
                    out.send(VcsEvent::OpOutput(OpOutput { text: text.trim().to_string(), error: false }));
                    out.send(VcsEvent::Done(what, None));
                }
                Err(e) => {
                    let summary = match &e {
                        rc_vcs::GitError::Conflicts(_) => tr("Stopped on conflicts: resolve them, then continue").to_string(),
                        e => user_error(e),
                    };
                    let text = match e.details().filter(|d| !d.trim().is_empty()) {
                        Some(d) => format!("{summary}\n{}", d.trim()),
                        None => summary.clone(),
                    };
                    out.send(VcsEvent::OpOutput(OpOutput { text, error: true }));
                    out.send(VcsEvent::Done(what, Some(summary)));
                }
            }
        });
    }

    // ------------------------------------------------------------------ stashes

    /// Stashes the work tree's changes (`--include-untracked`, `--keep-index` on request).
    pub fn stash_push(&mut self, message: &str, include_untracked: bool, keep_index: bool) {
        self.stash_push_paths(message, include_untracked, keep_index, Vec::new());
    }

    pub fn stash_push_paths(&mut self, message: &str, include_untracked: bool, keep_index: bool, mut paths: Vec<String>) {
        let Some(repo) = self.repo.clone() else { return };
        let message = Some(message.trim().to_string()).filter(|m| !m.is_empty());
        // A rename is one row in Commit but two paths in Git's index.
        if let Some(status) = &self.status {
            let old: Vec<String> = status.entries.iter().filter(|entry| paths.contains(&entry.path)).filter_map(|entry| entry.orig_path.clone()).collect();
            paths.extend(old);
            paths.sort();
            paths.dedup();
        }
        let opts = StashOptions { message, include_untracked, keep_index, paths };
        self.sequence("stash", move |cancel, _| async move {
            repo.stash_push_cancellable(&opts, Some(&cancel)).await?;
            Ok(String::new())
        });
    }

    pub fn stash_branch(&mut self, stash: &Stash, name: &str) {
        let Some(repo) = self.repo.clone() else { return };
        let (stash, name) = (stash.clone(), name.to_string());
        self.sequence("stash_branch", move |cancel, _| async move { repo.stash_branch(&stash, &name, Some(&cancel)).await });
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
        self.changes().into_iter().map(|c| c.path).filter(|p| !self.excluded.contains(p) && self.changelists.in_active(p)).collect()
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
        let opts = CommitOptions { message: self.message.trim().to_string(), amend: self.amend, only, sign: Some(self.sign), ..Default::default() };
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
        self.sequence("push", move |cancel, progress| async move {
            let opts = if upstream.is_none() { PushOptions { refspec: branch, set_upstream: true, ..Default::default() } } else { PushOptions::default() };
            repo.push(&opts, Some(progress), Some(&cancel)).await?;
            Ok(String::new())
        });
    }

    pub fn pull(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        self.sequence("pull", move |cancel, progress| async move {
            repo.pull(&PullOptions::default(), Some(progress), Some(&cancel)).await?;
            Ok(String::new())
        });
    }

    pub fn fetch(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        self.sequence("fetch", move |cancel, progress| async move {
            repo.fetch(&rc_vcs::FetchOptions { prune: true, ..Default::default() }, Some(progress), Some(&cancel)).await?;
            Ok(String::new())
        });
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
                    // Keep assignments when Git reports a rename as one changed row.
                    for entry in &s.entries {
                        if let Some(old) = &entry.orig_path
                            && let Some(list) = self.changelists.assignments.remove(old)
                        {
                            self.changelists.assignments.entry(entry.path.clone()).or_insert(list);
                        }
                    }
                    self.changelists.retain_changed(&paths);
                    self.status = Some(s);
                    self.status_version += 1;
                }
                VcsEvent::Status(Err(e)) => self.error = Some(e),
                VcsEvent::Branches(b) => self.branches = b,
                VcsEvent::Log(generation, append, l) if generation == self.log_gen => {
                    self.log_loading = false;
                    self.log_more = l.len() >= LOG_PAGE;
                    if append {
                        let known: std::collections::HashSet<String> = self.log.iter().map(|c| c.hash.clone()).collect();
                        self.log.extend(l.into_iter().filter(|c| !known.contains(&c.hash)));
                    } else {
                        self.log = l;
                    }
                }
                VcsEvent::Log(..) => {}
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
                                    if c.is_uncommitted() {
                                        tr("Not committed yet").to_string()
                                    } else {
                                        format!("{}  {}", format_date(c.author_time), short_author(&c.author))
                                    }
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
                    self.op_cancel = None;
                    self.op_progress = None;
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
                VcsEvent::Signature(h, sig) => {
                    if self.selected_commit.as_deref() == Some(h.as_str()) {
                        self.signature = Some((h, sig));
                    }
                }
                VcsEvent::Signing(c) => {
                    if !self.sign_touched {
                        self.sign = c.sign_by_default;
                    }
                    self.signing = Some(c);
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
                VcsEvent::Operation(seq, op) => {
                    if seq >= self.operation_seq {
                        self.operation_seq = seq;
                        self.operation = op;
                    }
                }
                VcsEvent::OpProgress(line) => {
                    if self.op_cancel.is_some() {
                        self.op_progress = Some(line);
                    }
                }
                VcsEvent::OpOutput(o) => self.op_output = Some(o),
                VcsEvent::MergeLoaded(rel, loaded) => {
                    if self.merge_loading.as_deref() != Some(rel.as_str()) {
                        continue;
                    }
                    self.merge_loading = None;
                    match (loaded, self.repo.clone()) {
                        (Ok((base, ours, theirs)), Some(repo)) => {
                            let names = crate::ui::git::side_names(self);
                            let abs = repo.abs_path(&rel);
                            self.diff = None;
                            self.merge_tool = Some(crate::merge_tool::MergeTool::new(&mut self.next_doc_id, rel, abs, &base, &ours, &theirs, names, dark));
                        }
                        (Err(e), _) => {
                            self.error = Some(e.clone());
                            notices.push(e);
                        }
                        (Ok(_), None) => {}
                    }
                }
                VcsEvent::MergeSaved(rel, err) => {
                    if let Some(tool) = self.merge_tool.as_mut().filter(|t| t.rel == rel) {
                        tool.saving = false;
                        match err {
                            Some(e) => tool.error = Some(e),
                            None => {
                                self.merge_tool = None;
                                notices.push(trf("{file} resolved", &[("file", &rel)]));
                            }
                        }
                    }
                    self.refresh();
                }
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

/// Whether a file has a conflict block left (`<<<<<<<` … `>>>>>>>` at line starts). Only the
/// first 64 MB are read; unreadable files have none.
pub fn has_conflict_markers(path: &Path) -> bool {
    use std::io::Read;
    let mut text = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(64 << 20).read_to_end(&mut text));
    if read.is_err() {
        return false;
    }
    let mut open = false;
    for line in text.split(|b| *b == b'\n') {
        if line.starts_with(b"<<<<<<< ") || line == b"<<<<<<<" {
            open = true;
        } else if open && (line.starts_with(b">>>>>>> ") || line == b">>>>>>>") {
            return true;
        }
    }
    false
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
        "stash_branch" => tr("Branch created; stash kept"),
        "patch_save" => tr("Patch saved"),
        "patch_copy" => tr("Patch copied to the clipboard"),
        "patch_apply" => tr("Patch applied"),
        "worktree_add" => tr("Worktree created"),
        "worktree_remove" => tr("Worktree removed"),
        "worktree_lock" => tr("Worktree locked"),
        "worktree_unlock" => tr("Worktree unlocked"),
        "worktree_prune" => tr("Worktrees pruned"),
        "merge" => tr("Merged"),
        "rebase" => tr("Rebased"),
        "cherry_pick" => tr("Cherry-picked"),
        "op_continue" => tr("Continued"),
        "op_skip" => tr("Commit skipped"),
        "op_abort" => tr("Aborted"),
        "resolve" => tr("Conflicts resolved"),
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
        rc_cargo::FormatError::CriticalupMissing => tr("This CriticalUp installation has no rustfmt; install the matching Ferrocene package externally").into(),
        rc_cargo::FormatError::CriticalupChannel => {
            tr("Rustup rustfmt channels are unsupported by CriticalUp; clear the channel to use the selected formatter").into()
        }
        rc_cargo::FormatError::Arguments => tr("The rustfmt arguments have a quote that is not closed (Settings › Tools).").to_string(),
        rc_cargo::FormatError::Channel(channel) => {
            trf("The rustfmt channel \"{channel}\" is not a toolchain name (Settings › Tools).", &[("channel", &channel)])
        }
        rc_cargo::FormatError::Environment(line) => {
            trf("The rustfmt environment takes NAME=value lines, not \"{line}\" (Settings › Tools).", &[("line", &line)])
        }
        rc_cargo::FormatError::File { path, message } => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let first = message.lines().next().unwrap_or_default().to_string();
            trf("rustfmt failed on {file}: {error}", &[("file", &name), ("error", &first)])
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(super::format_date(0), "1970-01-01");
        assert_eq!(super::format_date(1_700_000_000), "2023-11-14");
    }
}
