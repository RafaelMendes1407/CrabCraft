//! Merge, rebase and cherry-pick, and the operation git leaves in progress when one stops on
//! conflicts (continue, skip, abort).

use crate::cancel::CancelToken;
use crate::error::{GitError, Result};
use crate::git::ProgressSink;
use crate::repo::{RepoState, Repository};
use std::path::Path;

/// How a merge may move the branch (`--ff`, `--no-ff`, `--ff-only`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FastForward {
    /// Fast-forward when possible, else create a merge commit.
    #[default]
    Allow,
    /// Always create a merge commit.
    Never,
    /// Only fast-forward; fail otherwise.
    Only,
}

/// Options of [`Repository::merge_with`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeOptions {
    pub fast_forward: FastForward,
    /// Put the merged changes in the index without a merge commit (`--squash`).
    pub squash: bool,
    /// Merge but stop before committing (`--no-commit`).
    pub no_commit: bool,
}

/// What git left in progress: the state and what it works on, read from the git dir.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Operation {
    pub state: Option<RepoState>,
    /// The branch being rebased (short name).
    pub branch: Option<String>,
    /// The commit merged in, rebased onto, or being cherry-picked or reverted (full hash).
    pub target: Option<String>,
    /// Rebase progress: (current step, steps).
    pub step: Option<(usize, usize)>,
    /// The commit a rebase stopped on (full hash).
    pub commit: Option<String>,
}

impl Repository {
    /// Merges `rev` into the current branch and returns what git printed. Conflicts fail
    /// with [`GitError::Conflicts`] and leave the merge in progress.
    pub async fn merge_with(&self, rev: &str, opts: &MergeOptions, cancel: Option<&CancelToken>, progress: Option<ProgressSink>) -> Result<String> {
        self.no_operation().await?;
        let mut cmd = self.cmd().args(["merge", "--no-edit", "--stat"]).cancel(cancel);
        cmd = match opts.fast_forward {
            FastForward::Allow => cmd,
            FastForward::Never => cmd.arg("--no-ff"),
            FastForward::Only => cmd.arg("--ff-only"),
        };
        if opts.squash {
            cmd = cmd.arg("--squash");
        }
        if opts.no_commit {
            cmd = cmd.arg("--no-commit");
        }
        Ok(cmd.args(["--end-of-options", rev]).streamed(progress).await?.stdout_text())
    }

    /// Rebases the current branch onto `upstream`; `autostash` stashes local changes first
    /// and puts them back afterwards. Conflicts fail with [`GitError::Conflicts`] and leave
    /// the rebase in progress.
    pub async fn rebase(&self, upstream: &str, autostash: bool, cancel: Option<&CancelToken>, progress: Option<ProgressSink>) -> Result<String> {
        self.no_operation().await?;
        let out =
            self.cmd().arg("rebase").args(autostash.then_some("--autostash")).args(["--end-of-options", upstream]).cancel(cancel).streamed(progress).await?;
        Ok(rebase_report(&out))
    }

    /// Applies `commits` (oldest first) on top of the current branch. Conflicts fail with
    /// [`GitError::Conflicts`] and leave the cherry-pick in progress.
    pub async fn cherry_pick(&self, commits: &[String], cancel: Option<&CancelToken>) -> Result<String> {
        if commits.is_empty() {
            return Ok(String::new());
        }
        self.no_operation().await?;
        Ok(self.cmd().args(["cherry-pick", "--end-of-options"]).args(commits).cancel(cancel).output().await?.stdout_text())
    }

    /// Continues the stopped operation once its conflicts are resolved and staged.
    pub async fn continue_operation(&self, cancel: Option<&CancelToken>) -> Result<String> {
        let sub = match self.state().await {
            RepoState::Merging => "merge",
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::Clean | RepoState::Bisecting => return Err(GitError::NoOperation),
        };
        let out = self.cmd().args([sub, "--continue"]).cancel(cancel).output().await?;
        Ok(rebase_report(&out))
    }

    /// Skips the commit a rebase, cherry-pick or revert stopped on.
    pub async fn skip_operation(&self, cancel: Option<&CancelToken>) -> Result<String> {
        let sub = match self.state().await {
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::Merging | RepoState::Clean | RepoState::Bisecting => return Err(GitError::NoOperation),
        };
        let out = self.cmd().args([sub, "--skip"]).cancel(cancel).output().await?;
        Ok(rebase_report(&out))
    }

    /// Abandons the operation in progress, putting the branch back as it was.
    pub async fn abort_operation(&self) -> Result<()> {
        let sub = match self.state().await {
            RepoState::Merging => "merge",
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::Clean | RepoState::Bisecting => return Err(GitError::NoOperation),
        };
        self.cmd().args([sub, "--abort"]).output().await.map(drop)
    }

    /// Fails with [`GitError::OperationInProgress`] while a merge, rebase, cherry-pick or
    /// revert waits to be finished.
    async fn no_operation(&self) -> Result<()> {
        match self.state().await {
            RepoState::Clean | RepoState::Bisecting => Ok(()),
            _ => Err(GitError::OperationInProgress),
        }
    }

    /// The operation in progress and what it works on (`state: None` when there is none).
    pub async fn operation(&self) -> Operation {
        let state = self.state().await;
        let dir = self.git_dir().to_path_buf();
        tokio::task::spawn_blocking(move || read_operation(&dir, state)).await.unwrap_or_default()
    }
}

/// What `rebase`/`--continue` printed: stdout, or the last progress line of stderr when
/// stdout is empty (rebase reports on stderr).
fn rebase_report(out: &crate::git::Output) -> String {
    let stdout = out.stdout_text();
    if !stdout.trim().is_empty() {
        return stdout;
    }
    out.stderr.rsplit(['\r', '\n']).map(|l| l.trim().trim_start_matches("\u{1b}[K")).find(|l| !l.is_empty()).unwrap_or_default().to_string()
}

/// Reads at most a few hundred bytes of a state file of the git dir.
fn small_file(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path).ok()?.take(512).read_to_string(&mut text).ok()?;
    let line = text.lines().next()?.trim().to_string();
    (!line.is_empty()).then_some(line)
}

fn hash_in(path: &Path) -> Option<String> {
    small_file(path).filter(|h| (4..=64).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn number_in(path: &Path) -> Option<usize> {
    small_file(path)?.parse().ok()
}

/// Reads what the operation in progress works on from the files git keeps in `git_dir`.
pub fn read_operation(git_dir: &Path, state: RepoState) -> Operation {
    let mut op = Operation { state: (state != RepoState::Clean && state != RepoState::Bisecting).then_some(state), ..Default::default() };
    match state {
        RepoState::Merging => op.target = hash_in(&git_dir.join("MERGE_HEAD")),
        RepoState::CherryPicking => op.target = hash_in(&git_dir.join("CHERRY_PICK_HEAD")),
        RepoState::Reverting => op.target = hash_in(&git_dir.join("REVERT_HEAD")),
        RepoState::Rebasing => {
            // The merge backend keeps `rebase-merge/`, the apply backend `rebase-apply/`.
            let (dir, step, total) = if git_dir.join("rebase-merge").is_dir() {
                (git_dir.join("rebase-merge"), "msgnum", "end")
            } else {
                (git_dir.join("rebase-apply"), "next", "last")
            };
            op.branch = small_file(&dir.join("head-name")).map(|h| h.strip_prefix("refs/heads/").unwrap_or(&h).to_string()).filter(|h| h != "detached HEAD");
            op.target = hash_in(&dir.join("onto"));
            op.commit = hash_in(&git_dir.join("REBASE_HEAD"));
            if let (Some(n), Some(t)) = (number_in(&dir.join(step)), number_in(&dir.join(total))) {
                op.step = Some((n.min(t), t));
            }
        }
        RepoState::Clean | RepoState::Bisecting => {}
    }
    op
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_rebase_in_progress_and_ignores_junk() {
        let dir = tempfile::tempdir().unwrap();
        let rm = dir.path().join("rebase-merge");
        std::fs::create_dir(&rm).unwrap();
        std::fs::write(rm.join("head-name"), "refs/heads/feature/x\n").unwrap();
        std::fs::write(rm.join("onto"), "8d3d9c88c3572e56f2ee17f88baac8870c0c445e\n").unwrap();
        std::fs::write(rm.join("msgnum"), "2\n").unwrap();
        std::fs::write(rm.join("end"), "3\n").unwrap();
        let op = read_operation(dir.path(), RepoState::Rebasing);
        assert_eq!(op.state, Some(RepoState::Rebasing));
        assert_eq!(op.branch.as_deref(), Some("feature/x"));
        assert_eq!(op.target.as_deref(), Some("8d3d9c88c3572e56f2ee17f88baac8870c0c445e"));
        assert_eq!(op.step, Some((2, 3)));
        // Garbage instead of a hash or numbers is ignored.
        std::fs::write(rm.join("onto"), "$(rm -rf /)\n").unwrap();
        std::fs::write(rm.join("msgnum"), "many\n").unwrap();
        let op = read_operation(dir.path(), RepoState::Rebasing);
        assert_eq!((op.target, op.step), (None, None));
        std::fs::write(dir.path().join("MERGE_HEAD"), vec![b'a'; 4096]).unwrap();
        assert_eq!(read_operation(dir.path(), RepoState::Merging).target, None);
        assert_eq!(read_operation(dir.path(), RepoState::Clean), Operation::default());
    }
}
