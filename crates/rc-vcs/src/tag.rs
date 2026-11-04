//! Tags: listing, creating (lightweight or annotated), deleting, pushing and checking out.
//! Names typed by the user are checked with `git check-ref-format` before any other use, and
//! every command addresses tags by their full ref so a name can never be read as an option.

use crate::cancel::CancelToken;
use crate::error::{GitError, Result};
use crate::git::ProgressSink;
use crate::repo::Repository;

/// A tag from `git for-each-ref refs/tags`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    /// Short name: `v1.0`.
    pub name: String,
    /// The commit the tag points to (peeled through annotated tag objects).
    pub target: String,
    /// An annotated tag (a tag object with a message), not a lightweight one.
    pub annotated: bool,
    /// The annotation's message (empty for lightweight tags).
    pub message: String,
    /// Unix seconds: the tagger date of annotated tags, the commit date of lightweight ones.
    pub time: i64,
}

impl Tag {
    pub fn full_ref(&self) -> String {
        format!("refs/tags/{}", self.name)
    }

    pub fn short_target(&self) -> &str {
        &self.target[..self.target.len().min(8)]
    }
}

const FORMAT: &str = "--format=%(refname)%00%(objecttype)%00%(objectname)%00%(*objectname)%00%(creatordate:unix)%00%(contents)%1e";

impl Repository {
    /// Tags, newest first.
    pub async fn tags(&self) -> Result<Vec<Tag>> {
        let out = self.cmd().args(["for-each-ref", "--sort=-v:refname", "--sort=-creatordate", FORMAT, "refs/tags"]).text().await?;
        Ok(parse_tags(&out))
    }

    /// Fails with [`GitError::InvalidRefName`] unless `name` is a valid tag name.
    pub async fn check_tag_name(&self, name: &str) -> Result<()> {
        if name.is_empty() || name.starts_with('-') || name.trim() != name {
            return Err(GitError::InvalidRefName(name.to_string()));
        }
        let out = self.cmd().args(["check-ref-format"]).arg(format!("refs/tags/{name}")).ok_code(1).output().await?;
        if out.code == Some(0) { Ok(()) } else { Err(GitError::InvalidRefName(name.to_string())) }
    }

    /// Creates a tag on `target` (default HEAD); annotated when `message` is given.
    pub async fn create_tag(&self, name: &str, target: Option<&str>, message: Option<&str>) -> Result<()> {
        self.check_tag_name(name).await?;
        let target = match target {
            Some(t) => self.resolve(t).await?,
            None => self.resolve("HEAD").await?,
        };
        if self.cmd().args(["rev-parse", "--verify", "--quiet"]).arg(format!("refs/tags/{name}")).ok_code(1).output().await?.code == Some(0) {
            return Err(GitError::TagExists(name.to_string()));
        }
        let mut cmd = self.cmd().arg("tag");
        if let Some(m) = message {
            cmd = cmd.args(["--annotate", "--cleanup=whitespace", "--file=-"]).stdin(m.as_bytes().to_vec());
        }
        cmd.args(["--end-of-options", name, &target]).output().await.map(drop)
    }

    /// Deletes a local tag.
    pub async fn delete_tag(&self, name: &str) -> Result<()> {
        self.check_tag_name(name).await?;
        self.cmd().args(["tag", "--delete", "--end-of-options", name]).output().await.map(drop)
    }

    /// Pushes one tag (`Some(name)`) or every tag (`None`) to `remote` and returns git's
    /// report (its porcelain output and messages).
    pub async fn push_tags(&self, remote: &str, name: Option<&str>, progress: Option<ProgressSink>, cancel: Option<&CancelToken>) -> Result<String> {
        if remote.is_empty() || remote.starts_with('-') {
            return Err(GitError::NotFound(remote.to_string()));
        }
        let mut cmd = self.cmd().args(["push", "--progress", "--porcelain"]).cancel(cancel);
        match name {
            Some(n) => {
                self.check_tag_name(n).await?;
                cmd = cmd.args(["--end-of-options", remote]).arg(format!("refs/tags/{n}:refs/tags/{n}"));
            }
            None => cmd = cmd.args(["--tags", "--end-of-options", remote]),
        }
        let out = cmd.streamed(progress).await?;
        let mut report = out.stdout_text().trim().to_string();
        let err = out.stderr.lines().filter(|l| !l.contains('%') && !l.trim().is_empty()).collect::<Vec<_>>().join("\n");
        if !err.is_empty() {
            if !report.is_empty() {
                report.push('\n');
            }
            report.push_str(&err);
        }
        Ok(report)
    }

    /// Checks out the commit of a tag with a detached HEAD.
    pub async fn checkout_tag(&self, name: &str) -> Result<()> {
        self.check_tag_name(name).await?;
        self.checkout_detached(&format!("refs/tags/{name}")).await
    }
}

/// Parses `git for-each-ref` output in this module's format.
pub fn parse_tags(out: &str) -> Vec<Tag> {
    out.split('\x1e')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let f: Vec<&str> = rec.splitn(6, '\0').collect();
            if f.len() != 6 {
                return None;
            }
            let name = f[0].strip_prefix("refs/tags/")?;
            let annotated = f[1] == "tag";
            let target = if annotated && !f[3].is_empty() { f[3] } else { f[2] };
            Some(Tag {
                name: name.to_string(),
                target: target.to_string(),
                annotated,
                message: if annotated { strip_signature(f[5]).trim().to_string() } else { String::new() },
                time: f[4].parse().unwrap_or(0),
            })
        })
        .collect()
}

fn strip_signature(message: &str) -> &str {
    message.find("-----BEGIN PGP SIGNATURE-----").or_else(|| message.find("-----BEGIN SSH SIGNATURE-----")).map_or(message, |at| &message[..at])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags() {
        let out = "refs/tags/v2\0tag\0aaa\0ccc\x00200\0Release 2\n\nNotes\n\x1e\nrefs/tags/v1\0commit\0bbb\0\x00100\0Subject of the commit\n\x1e\nbroken\x1e\n";
        let t = parse_tags(out);
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].name.as_str(), t[0].target.as_str(), t[0].annotated, t[0].message.as_str(), t[0].time), ("v2", "ccc", true, "Release 2\n\nNotes", 200));
        assert_eq!((t[1].name.as_str(), t[1].target.as_str(), t[1].annotated, t[1].message.as_str()), ("v1", "bbb", false, ""));
        assert_eq!(t[1].full_ref(), "refs/tags/v1");
    }
}
