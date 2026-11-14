//! Worktrees: listing (`git worktree list --porcelain`), adding one on a new or an existing
//! branch, removing (refused while it has local changes unless forced), locking, unlocking and
//! pruning. Paths come from git or from the user: every command passes them after `--`, and
//! branch names are checked with `git check-ref-format` first.

use crate::error::{GitError, Result};
use crate::git::GitVersion;
use crate::repo::Repository;
use std::path::{Path, PathBuf};

/// One entry of `git worktree list --porcelain`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The commit checked out (`None` for a bare repository or an unborn branch).
    pub head: Option<String>,
    /// Short name of the branch checked out; `None` when HEAD is detached or bare.
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    /// Locked, with the reason given (may be empty).
    pub locked: Option<String>,
    /// Its folder is gone: `git worktree prune` would remove it. Carries git's reason.
    pub prunable: Option<String>,
    /// The main work tree (the first entry), which cannot be removed.
    pub main: bool,
}

impl Worktree {
    pub fn short_head(&self) -> &str {
        self.head.as_deref().map_or("", |h| &h[..h.len().min(8)])
    }

    /// The folder's name, for lists.
    pub fn name(&self) -> String {
        self.path.file_name().map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }
}

/// What a new worktree checks out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorktreeCheckout {
    /// A new branch `name` started at `start` (default HEAD).
    NewBranch { name: String, start: Option<String> },
    /// A local branch that exists and is not checked out elsewhere.
    Branch(String),
    /// A commit, with a detached HEAD.
    Detached(String),
}

/// `-z` came with git 2.36; older gits separate fields with newlines.
const NUL_LIST: GitVersion = GitVersion { major: 2, minor: 36, patch: 0 };

impl Repository {
    /// The worktrees of the repository, the main one first.
    pub async fn worktrees(&self) -> Result<Vec<Worktree>> {
        let nul = self.git().version() >= NUL_LIST;
        let out = self.cmd().args(["worktree", "list", "--porcelain"]).args(nul.then_some("-z")).output().await?;
        Ok(parse_worktrees(&String::from_utf8_lossy(&out.stdout), if nul { '\0' } else { '\n' }))
    }

    /// Fails with [`GitError::InvalidBranchName`] unless `name` is a valid branch name.
    pub async fn check_branch_name(&self, name: &str) -> Result<()> {
        if name.is_empty() || name.starts_with('-') || name.trim() != name || name == "HEAD" {
            return Err(GitError::InvalidBranchName(name.to_string()));
        }
        let out = self.cmd().args(["check-ref-format"]).arg(format!("refs/heads/{name}")).ok_code(1).output().await?;
        if out.code == Some(0) { Ok(()) } else { Err(GitError::InvalidBranchName(name.to_string())) }
    }

    /// Creates a worktree at `path` (absolute; a new or empty folder) checking out `what`.
    pub async fn add_worktree(&self, path: &Path, what: &WorktreeCheckout) -> Result<()> {
        if !path.is_absolute() {
            return Err(GitError::NotFound(path.display().to_string()));
        }
        let mut cmd = self.cmd().args(["worktree", "add", "--quiet"]);
        match what {
            WorktreeCheckout::NewBranch { name, start } => {
                self.check_branch_name(name).await?;
                if self.cmd().args(["rev-parse", "--verify", "--quiet"]).arg(format!("refs/heads/{name}")).ok_code(1).output().await?.code == Some(0) {
                    return Err(GitError::BranchExists(name.clone()));
                }
                let start = self.resolve(start.as_deref().unwrap_or("HEAD")).await?;
                cmd = cmd.args(["-b", name, "--"]).arg(path).arg(start);
            }
            WorktreeCheckout::Branch(name) => {
                self.check_branch_name(name).await?;
                if self.cmd().args(["rev-parse", "--verify", "--quiet"]).arg(format!("refs/heads/{name}")).ok_code(1).output().await?.code != Some(0) {
                    return Err(GitError::NotFound(name.clone()));
                }
                // A short name that is a local branch checks it out (a full ref would detach).
                cmd = cmd.arg("--").arg(path).arg(name);
            }
            WorktreeCheckout::Detached(rev) => {
                let commit = self.resolve(rev).await?;
                cmd = cmd.args(["--detach", "--"]).arg(path).arg(commit);
            }
        }
        cmd.output().await.map(drop).map_err(|e| worktree_error(e, path))
    }

    /// Removes a linked worktree and its folder. Without `force`, git refuses when it has
    /// modified or untracked files ([`GitError::WorktreeDirty`]) or is locked
    /// ([`GitError::WorktreeLocked`]); `force` overrides only the first.
    pub async fn remove_worktree(&self, path: &Path, force: bool) -> Result<()> {
        self.cmd()
            .args(["worktree", "remove"])
            .args(force.then_some("--force"))
            .arg("--")
            .arg(path)
            .output()
            .await
            .map(drop)
            .map_err(|e| worktree_error(e, path))
    }

    /// Locks a worktree so that prune, move and remove leave it alone.
    pub async fn lock_worktree(&self, path: &Path, reason: Option<&str>) -> Result<()> {
        let mut cmd = self.cmd().args(["worktree", "lock"]);
        if let Some(r) = reason.map(str::trim).filter(|r| !r.is_empty()) {
            cmd = cmd.arg("--reason").arg(r);
        }
        cmd.arg("--").arg(path).output().await.map(drop).map_err(|e| worktree_error(e, path))
    }

    pub async fn unlock_worktree(&self, path: &Path) -> Result<()> {
        self.cmd().args(["worktree", "unlock", "--"]).arg(path).output().await.map(drop).map_err(|e| worktree_error(e, path))
    }

    /// Forgets the worktrees whose folders are gone (locked ones are kept) and returns what
    /// git reports having removed.
    pub async fn prune_worktrees(&self) -> Result<String> {
        let out = self.cmd().args(["worktree", "prune", "--verbose"]).output().await?;
        let mut text = out.stdout_text().trim().to_string();
        let err = out.stderr.trim();
        if !err.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(err);
        }
        Ok(text)
    }
}

/// Gives git's worktree refusals their own errors.
fn worktree_error(e: GitError, path: &Path) -> GitError {
    let GitError::Failed { stderr, .. } = &e else { return e };
    let path = path.to_path_buf();
    if stderr.contains("contains modified or untracked files") {
        GitError::WorktreeDirty(path)
    } else if stderr.contains("cannot remove a locked working tree") || stderr.contains("is locked") {
        GitError::WorktreeLocked(path)
    } else if stderr.contains("is a main working tree") {
        GitError::MainWorktree
    } else if stderr.contains("already exists") {
        GitError::WorktreePathExists(path)
    } else if stderr.contains("is already checked out at") || stderr.contains("is already used by worktree at") {
        let branch = stderr.split('\'').nth(1).unwrap_or_default().to_string();
        GitError::BranchInUse(branch)
    } else if stderr.contains("is not a working tree") {
        GitError::NotFound(path.display().to_string())
    } else {
        e
    }
}

/// Parses `git worktree list --porcelain` whose fields end with `sep` (`\0` with `-z`, else
/// `\n`); an empty field ends an entry.
pub fn parse_worktrees(out: &str, sep: char) -> Vec<Worktree> {
    let mut list = Vec::new();
    let mut cur: Option<Worktree> = None;
    for field in out.split(sep) {
        let field = if sep == '\n' { field.trim_end_matches('\r') } else { field };
        if field.is_empty() {
            list.extend(cur.take());
            continue;
        }
        let (key, value) = field.split_once(' ').unwrap_or((field, ""));
        if key == "worktree" {
            list.extend(cur.take());
            cur = Some(Worktree { path: PathBuf::from(value), ..Worktree::default() });
            continue;
        }
        let Some(w) = cur.as_mut() else { continue };
        match key {
            "HEAD" => w.head = Some(value.to_string()).filter(|h| !h.is_empty() && h.bytes().any(|b| b != b'0')),
            "branch" => w.branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value).to_string()),
            "detached" => w.detached = true,
            "bare" => w.bare = true,
            "locked" => w.locked = Some(value.to_string()),
            "prunable" => w.prunable = Some(value.to_string()),
            _ => {}
        }
    }
    list.extend(cur);
    if let Some(first) = list.first_mut() {
        first.main = true;
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_worktrees() {
        let z = "worktree /r\0HEAD aaa\0branch refs/heads/main\0\0worktree /w 1\0HEAD bbb\0detached\0locked on usb\0\0worktree /gone\0HEAD ccc\0branch refs/heads/x\0prunable gitdir file points to non-existent location\0\0";
        let w = parse_worktrees(z, '\0');
        assert_eq!(w.len(), 3);
        assert!(w[0].main && !w[1].main);
        assert_eq!((w[0].path.as_path(), w[0].branch.as_deref(), w[0].head.as_deref()), (Path::new("/r"), Some("main"), Some("aaa")));
        assert_eq!((w[1].path.as_path(), w[1].detached, w[1].locked.as_deref()), (Path::new("/w 1"), true, Some("on usb")));
        assert_eq!(w[1].name(), "w 1");
        assert!(w[2].prunable.is_some() && w[2].locked.is_none());

        let lines = "worktree /bare\nbare\n\nworktree /a\nHEAD 0000000000000000000000000000000000000000\nbranch refs/heads/new\nlocked\n";
        let w = parse_worktrees(lines, '\n');
        assert_eq!(w.len(), 2);
        assert!(w[0].bare && w[0].head.is_none());
        assert_eq!((w[1].head.as_deref(), w[1].locked.as_deref()), (None, Some("")));
        // Garbage before an entry is ignored.
        assert!(parse_worktrees("HEAD x\0branch y\0\0", '\0').is_empty());
    }
}
