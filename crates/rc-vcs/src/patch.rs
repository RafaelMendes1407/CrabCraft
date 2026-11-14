//! Patches: creating them from local changes (`git diff`) or from commits
//! (`git format-patch`), previewing what a patch touches (`git apply --numstat/--summary/--check`)
//! and applying it (`git apply`, or `git am` for format-patch mails).
//!
//! Patch text is untrusted: it only ever reaches git through stdin, and both created and
//! applied patches are limited to [`MAX_PATCH_BYTES`].

use crate::error::{GitError, Result};
use crate::repo::{RepoState, Repository};

/// Largest patch created or applied (32 MiB).
pub const MAX_PATCH_BYTES: usize = 32 << 20;

/// The empty tree, the base of diffs in a repository without commits.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// How a patch must be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchKind {
    /// A plain diff (`git diff`): applied to the work tree with `git apply`.
    Diff,
    /// Mails from `git format-patch`: applied as commits with `git am`.
    Mail,
}

/// One file a patch touches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchFile {
    pub path: String,
    /// `A` created, `D` deleted, `R` renamed or copied, `M` modified.
    pub status: char,
    /// Added and removed lines; `None` for binary files.
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

/// What applying a patch would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchPreview {
    pub kind: PatchKind,
    /// Number of mails (commits) in a [`PatchKind::Mail`] patch.
    pub commits: usize,
    pub files: Vec<PatchFile>,
    /// `None` when `git apply --check` accepts the patch, else git's message.
    pub problem: Option<String>,
}

/// Fails with [`GitError::PatchTooLarge`] when `len` bytes exceed [`MAX_PATCH_BYTES`].
pub fn check_patch_size(len: u64) -> Result<()> {
    if len > MAX_PATCH_BYTES as u64 { Err(GitError::PatchTooLarge(MAX_PATCH_BYTES / (1 << 20))) } else { Ok(()) }
}

/// Whether `patch` is a series of mails from `git format-patch` (or another mbox) rather than a
/// plain diff: a mail header block (`From <hash> …` line or `From:`/`Subject:` headers) before
/// the first `diff --git`.
pub fn patch_kind(patch: &[u8]) -> PatchKind {
    let head = String::from_utf8_lossy(&patch[..patch.len().min(64 << 10)]);
    let head = head.split("\ndiff --git ").next().unwrap_or("");
    let mbox = head.lines().next().is_some_and(|l| is_mbox_line(l.as_bytes()));
    let (mut from, mut subject) = (false, false);
    for l in head.lines().take_while(|l| !l.is_empty()) {
        from |= l.starts_with("From: ");
        subject |= l.starts_with("Subject: ");
    }
    if mbox || (from && subject) { PatchKind::Mail } else { PatchKind::Diff }
}

/// `From <commit hash> <date>`: the first line of each mail of `git format-patch`.
fn is_mbox_line(line: &[u8]) -> bool {
    line.strip_prefix(b"From ").is_some_and(|r| {
        let hash = r.split(|b| *b == b' ').next().unwrap_or(b"");
        hash.len() >= 40 && hash.iter().all(u8::is_ascii_hexdigit)
    })
}

/// Number of mails in a [`PatchKind::Mail`] patch.
fn mail_count(patch: &[u8]) -> usize {
    patch.split(|b| *b == b'\n').filter(|l| is_mbox_line(l)).count().max(1)
}

/// Parses `git apply --numstat -z` and `git apply --summary` output.
pub fn parse_patch_files(numstat: &[u8], summary: &str) -> Vec<PatchFile> {
    let mut files: Vec<PatchFile> = Vec::new();
    for rec in numstat.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let rec = String::from_utf8_lossy(rec);
        let mut f = rec.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (f.next(), f.next(), f.next()) else { continue };
        let path = path.trim_start_matches('\n').to_string();
        if path.is_empty() {
            continue;
        }
        let (added, removed): (Option<u64>, Option<u64>) = (a.parse().ok(), r.parse().ok());
        // A series of mails can touch a file more than once.
        match files.iter_mut().find(|f| f.path == path) {
            Some(f) => {
                f.added = f.added.zip(added).map(|(x, y)| x.saturating_add(y));
                f.removed = f.removed.zip(removed).map(|(x, y)| x.saturating_add(y));
            }
            None => files.push(PatchFile { status: 'M', added, removed, path }),
        }
    }
    for line in summary.lines().map(str::trim) {
        let set = |files: &mut Vec<PatchFile>, path: &str, status: char| {
            if let Some(f) = files.iter_mut().find(|f| f.path == path) {
                f.status = status;
            }
        };
        if let Some(rest) = line.strip_prefix("create mode ") {
            set(&mut files, rest.split_once(' ').map_or("", |(_, p)| p), 'A');
        } else if let Some(rest) = line.strip_prefix("delete mode ") {
            set(&mut files, rest.split_once(' ').map_or("", |(_, p)| p), 'D');
        } else if let Some(rest) = line.strip_prefix("rename ").or_else(|| line.strip_prefix("copy ")) {
            // `rename src/{a.rs => b.rs} (90%)`: the new path is not spelled out; match its name.
            let rest = rest.rsplit_once(" (").map_or(rest, |(r, _)| r);
            let new_name = rest.rsplit(" => ").next().unwrap_or("").trim_end_matches('}');
            let new_name = new_name.rsplit('/').next().unwrap_or(new_name);
            for f in files.iter_mut().filter(|f| f.status == 'M' && f.path.rsplit('/').next() == Some(new_name)) {
                f.status = 'R';
            }
        }
    }
    files
}

fn limit(out: Vec<u8>) -> Result<Vec<u8>> {
    check_patch_size(out.len() as u64)?;
    Ok(out)
}

impl Repository {
    /// A patch (`git diff --binary`) of the local changes of `paths` against HEAD, staged or
    /// not; untracked files among them are included as new files.
    pub async fn diff_patch(&self, paths: &[String]) -> Result<Vec<u8>> {
        if paths.is_empty() {
            return Err(GitError::NothingToCommit);
        }
        let base = self.resolve("HEAD").await.unwrap_or_else(|_| EMPTY_TREE.to_string());
        let out = self
            .cmd()
            .args([
                "diff",
                "--binary",
                "--find-renames",
                "--no-ext-diff",
                "--no-textconv",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "--end-of-options",
                &base,
                "--",
            ])
            .args(paths)
            .output()
            .await?;
        let mut patch = out.stdout;
        let untracked = self.cmd().args(["ls-files", "--others", "--exclude-standard", "-z", "--"]).args(paths).output().await?;
        for path in untracked.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            check_patch_size(patch.len() as u64)?;
            let path = String::from_utf8_lossy(path).into_owned();
            // `--no-index` exits 1 when the files differ, which they always do here.
            let out = self
                .cmd()
                .args(["diff", "--no-index", "--binary", "--no-ext-diff", "--no-textconv", "--src-prefix=a/", "--dst-prefix=b/", "--", "/dev/null"])
                .arg(&path)
                .ok_code(1)
                .output()
                .await?;
            patch.extend_from_slice(&out.stdout);
        }
        if patch.is_empty() {
            return Err(GitError::NothingToCommit);
        }
        limit(patch)
    }

    /// Mails (`git format-patch --stdout`) of `commits`, given newest first (in the order of
    /// the log) and written oldest first; each one becomes a commit again with `git am`.
    pub async fn format_patch(&self, commits: &[String]) -> Result<Vec<u8>> {
        if commits.is_empty() {
            return Err(GitError::NotFound(String::new()));
        }
        let mut hashes = Vec::with_capacity(commits.len());
        for c in commits {
            let h = self.resolve(c).await?;
            if !hashes.contains(&h) {
                hashes.push(h);
            }
        }
        let mut patch = Vec::new();
        for h in hashes.iter().rev() {
            let out =
                self.cmd().args(["format-patch", "--stdout", "--binary", "--no-signature", "--no-ext-diff", "-1", "--end-of-options", h]).output().await?;
            patch.extend_from_slice(&out.stdout);
            check_patch_size(patch.len() as u64)?;
        }
        limit(patch)
    }

    /// The files `patch` touches and whether it applies (with `three_way`, as `git apply --3way`
    /// would). Nothing is changed.
    pub async fn preview_patch(&self, patch: &[u8], three_way: bool) -> Result<PatchPreview> {
        check_patch_size(patch.len() as u64)?;
        let kind = patch_kind(patch);
        let numstat = self.cmd().args(["apply", "--numstat", "-z", "-"]).stdin(patch.to_vec()).output().await?;
        let summary = self.cmd().args(["apply", "--summary", "-"]).stdin(patch.to_vec()).text().await?;
        let files = parse_patch_files(&numstat.stdout, &summary);
        if files.is_empty() {
            return Err(GitError::PatchFailed(String::new()));
        }
        let check = self
            .cmd()
            .args(["apply", "--check", "--whitespace=nowarn"])
            .args(three_way.then_some("--3way"))
            .arg("-")
            .stdin(patch.to_vec())
            .ok_code(1)
            .ok_code(128)
            .output()
            .await?;
        let problem = (check.code != Some(0)).then(|| {
            let text = check.stderr.trim();
            if text.is_empty() { check.stdout_text().trim().to_string() } else { text.to_string() }
        });
        let commits = if kind == PatchKind::Mail { mail_count(patch) } else { 0 };
        Ok(PatchPreview { kind, commits, files, problem })
    }

    /// Applies `patch` and returns what git printed. A plain diff goes to the work tree
    /// (`git apply`; with `three_way`, conflicts are left to resolve and reported as
    /// [`GitError::Conflicts`]); mails become commits (`git am`), and a failed `git am` is
    /// aborted so the repository is left as it was.
    pub async fn apply_patch(&self, patch: &[u8], three_way: bool) -> Result<String> {
        check_patch_size(patch.len() as u64)?;
        let report = |out: crate::git::Output| {
            let mut text = out.stdout_text().trim().to_string();
            if !out.stderr.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(out.stderr.trim());
            }
            text
        };
        match patch_kind(patch) {
            PatchKind::Diff => {
                let out = self
                    .cmd()
                    .args(["apply", "--whitespace=nowarn", "--verbose"])
                    .args(three_way.then_some("--3way"))
                    .arg("-")
                    .stdin(patch.to_vec())
                    .ok_code(1)
                    .ok_code(128)
                    .output()
                    .await?;
                match out.code {
                    Some(0) => Ok(report(out)),
                    _ if three_way && out.stderr.contains("with conflicts") => Err(GitError::Conflicts(report(out))),
                    _ => Err(GitError::PatchFailed(report(out))),
                }
            }
            PatchKind::Mail => {
                if self.state().await != RepoState::Clean {
                    return Err(GitError::OperationInProgress);
                }
                let out = self.cmd().args(["am", "--quiet"]).args(three_way.then_some("--3way")).stdin(patch.to_vec()).ok_code(1).ok_code(128).output().await?;
                if out.code == Some(0) {
                    return Ok(report(out));
                }
                let text = report(out);
                // Leave nothing half-applied behind (the state was clean before `am`).
                if self.state().await == RepoState::Rebasing {
                    let _ = self.cmd().args(["am", "--abort"]).output().await;
                }
                Err(match GitError::classify(&["am".into()], Some(1), "", &text) {
                    e @ (GitError::IdentityUnknown | GitError::Locked(_)) => e,
                    _ => GitError::PatchFailed(text),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_mails_from_diffs() {
        let mail = "From 0123456789abcdef0123456789abcdef01234567 Mon Sep 17 00:00:00 2001\nFrom: A <a@b>\nSubject: [PATCH] x\n\n---\ndiff --git a/x b/x\n";
        assert_eq!(patch_kind(mail.as_bytes()), PatchKind::Mail);
        assert_eq!(patch_kind(b"From: A <a@b>\nSubject: x\n\ndiff --git a/x b/x\n"), PatchKind::Mail);
        assert_eq!(patch_kind(b"diff --git a/x b/x\n--- a/x\n+++ b/x\n"), PatchKind::Diff);
        assert_eq!(patch_kind(b"From the docs\ndiff --git a/x b/x\n"), PatchKind::Diff);
        assert_eq!(patch_kind(b""), PatchKind::Diff);
        assert_eq!(mail_count(format!("{mail}{mail}").as_bytes()), 2);
    }

    #[test]
    fn parses_numstat_and_summary() {
        let numstat = b"1\t1\tsrc/b.rs\x003\t0\tnew.txt\x00-\t-\timg.png\x000\t2\told.txt\x00";
        let summary = " rename src/{a.rs => b.rs} (66%)\n create mode 100644 new.txt\n create mode 100644 img.png\n delete mode 100644 old.txt\n";
        let f = parse_patch_files(numstat, summary);
        let short: Vec<_> = f.iter().map(|f| (f.path.as_str(), f.status, f.added, f.removed)).collect();
        assert_eq!(
            short,
            [("src/b.rs", 'R', Some(1), Some(1)), ("new.txt", 'A', Some(3), Some(0)), ("img.png", 'A', None, None), ("old.txt", 'D', Some(0), Some(2))]
        );
        assert!(parse_patch_files(b"garbage\x00", "").is_empty());
    }

    #[test]
    fn limits_the_size() {
        assert!(check_patch_size(10).is_ok());
        assert!(matches!(check_patch_size(MAX_PATCH_BYTES as u64 + 1), Err(GitError::PatchTooLarge(32))));
    }
}
