//! Stashes: list, push, apply, pop, drop, and the files a stash saved.

use crate::error::{GitError, Result};
use crate::log::ChangedFile;
use crate::repo::Repository;

/// One stash entry (`stash@{index}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stash {
    pub index: usize,
    pub hash: String,
    /// Unix seconds.
    pub time: i64,
    /// `On main: message` / `WIP on main: abc123 subject`.
    pub message: String,
    /// Branch the stash was made on, when known.
    pub branch: Option<String>,
}

/// Options of [`Repository::stash_push`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StashOptions {
    pub message: Option<String>,
    pub include_untracked: bool,
    /// Keep staged changes in the index (`--keep-index`).
    pub keep_index: bool,
    /// Stash only these paths (empty = everything).
    pub paths: Vec<String>,
}

impl Repository {
    /// Stashes, newest first.
    pub async fn stashes(&self) -> Result<Vec<Stash>> {
        let out = self.cmd().args(["stash", "list", "--format=%gd%x00%H%x00%ct%x00%gs"]).text().await?;
        Ok(parse_stashes(&out))
    }

    /// Saves local changes into a new stash ([`crate::GitError::NothingToStash`] when clean).
    pub async fn stash_push(&self, opts: &StashOptions) -> Result<()> {
        let mut cmd = self.cmd().args(["stash", "push"]);
        if let Some(m) = &opts.message {
            cmd = cmd.arg("--message").arg(m);
        }
        if opts.include_untracked {
            cmd = cmd.arg("--include-untracked");
        }
        if opts.keep_index {
            cmd = cmd.arg("--keep-index");
        }
        let out = cmd.arg("--").args(&opts.paths).output().await?;
        // Older gits exit 0 when there is nothing to stash.
        if out.stdout_text().contains("No local changes to save") || out.stderr.contains("No local changes to save") {
            return Err(crate::GitError::NothingToStash);
        }
        Ok(())
    }

    /// Applies a stash and drops it; conflicts keep it and fail with `Conflicts`.
    pub async fn stash_pop(&self, index: usize, restore_index: bool) -> Result<()> {
        self.cmd().args(["stash", "pop", "--quiet"]).args(restore_index.then_some("--index")).arg(format!("stash@{{{index}}}")).output().await.map(drop)
    }

    /// Applies a stash and keeps it.
    pub async fn stash_apply(&self, index: usize, restore_index: bool) -> Result<()> {
        self.cmd().args(["stash", "apply", "--quiet"]).args(restore_index.then_some("--index")).arg(format!("stash@{{{index}}}")).output().await.map(drop)
    }

    /// Fails with [`GitError::StashChanged`] unless `stash@{index}` is still the stash `hash`
    /// (another tool may have pushed or dropped one since the list was read).
    pub async fn check_stash(&self, index: usize, hash: &str) -> Result<()> {
        match self.resolve(&format!("stash@{{{index}}}")).await {
            Ok(h) if h == hash => Ok(()),
            Ok(_) | Err(GitError::NotFound(_)) => Err(GitError::StashChanged),
            Err(e) => Err(e),
        }
    }

    /// The files a stash saved: tracked changes against the commit it was made on, then the
    /// untracked files it holds (status `?`, from `--include-untracked`).
    pub async fn stash_files(&self, hash: &str) -> Result<Vec<ChangedFile>> {
        // A stash is a merge of HEAD and the index; its tree against HEAD is the work tree's changes.
        let out = self.cmd().args(["diff-tree", "--no-commit-id", "-r", "--find-renames", "--name-status", "-z", "--end-of-options"]).arg(format!("{hash}^1")).arg(hash).output().await?;
        let mut files = crate::log::parse_name_status(&out.stdout)?;
        if let Ok(untracked) = self.resolve(&format!("{hash}^3")).await {
            let out = self.cmd().args(["ls-tree", "-r", "-z", "--name-only", "--end-of-options", &untracked]).output().await?;
            files.extend(out.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| ChangedFile { status: '?', path: String::from_utf8_lossy(s).into_owned(), orig_path: None }));
        }
        Ok(files)
    }

    /// Deletes a stash.
    pub async fn stash_drop(&self, index: usize) -> Result<()> {
        self.cmd().args(["stash", "drop", "--quiet"]).arg(format!("stash@{{{index}}}")).output().await.map(drop)
    }
}

/// Parses `git stash list` in this module's format.
pub fn parse_stashes(out: &str) -> Vec<Stash> {
    out.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.splitn(4, '\0').collect();
            if f.len() != 4 {
                return None;
            }
            let index = f[0].strip_prefix("stash@{")?.strip_suffix('}')?.parse().ok()?;
            let branch = f[3].strip_prefix("WIP on ").or_else(|| f[3].strip_prefix("On ")).and_then(|r| r.split_once(':')).map(|(b, _)| b.to_string());
            Some(Stash { index, hash: f[1].to_string(), time: f[2].parse().unwrap_or(0), message: f[3].to_string(), branch })
        })
        .collect()
}
