//! Commit signatures (V-14): the repository's signing settings and the signature of a commit
//! as git verifies it. Signing itself is [`crate::CommitOptions::sign`]; git runs the signing
//! program (`gpg.program`, GPG by default) with the user's key and agent.

use crate::error::Result;
use crate::repo::Repository;

/// How the repository signs commits (from `git config`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SigningConfig {
    /// `commit.gpgsign`: commits are signed unless asked not to be.
    pub sign_by_default: bool,
    /// `user.signingkey`, when set (otherwise the program picks the key of the committer).
    pub key: Option<String>,
    /// `gpg.format`: `openpgp` (the default), `x509` or `ssh`.
    pub format: String,
}

/// What git says of a commit's signature (`%G?`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureStatus {
    /// `G`: a good, valid signature.
    Good,
    /// `U`: good, but the key's validity is unknown (not trusted).
    GoodUntrusted,
    /// `X`: good, but the signature has expired.
    Expired,
    /// `Y`: good, made by a key that has expired.
    ExpiredKey,
    /// `R`: good, made by a key that was revoked.
    RevokedKey,
    /// `B`: a bad signature (the commit does not match it).
    Bad,
    /// `E`: it cannot be checked (usually the key is missing).
    CannotCheck,
    /// `N`: not signed.
    Unsigned,
}

impl SignatureStatus {
    pub fn from_code(code: &str) -> SignatureStatus {
        match code.trim() {
            "G" => SignatureStatus::Good,
            "U" => SignatureStatus::GoodUntrusted,
            "X" => SignatureStatus::Expired,
            "Y" => SignatureStatus::ExpiredKey,
            "R" => SignatureStatus::RevokedKey,
            "B" => SignatureStatus::Bad,
            "E" => SignatureStatus::CannotCheck,
            _ => SignatureStatus::Unsigned,
        }
    }
}

/// A commit's signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub status: SignatureStatus,
    /// The signer (`%GS`), e.g. `Name <mail>`; empty when unknown.
    pub signer: String,
    /// The key (`%GK`); empty when unknown.
    pub key: String,
}

/// Git's boolean values (`true`, `yes`, `on`, `1`).
fn git_bool(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1")
}

/// Parses `git config --list -z` (`key\nvalue\0` records; the last value of a key wins).
pub fn parse_signing_config(out: &[u8]) -> SigningConfig {
    let mut cfg = SigningConfig { format: "openpgp".into(), ..Default::default() };
    for rec in String::from_utf8_lossy(out).split('\0') {
        let (key, value) = rec.split_once('\n').unwrap_or((rec, "true"));
        match key.to_ascii_lowercase().as_str() {
            "commit.gpgsign" => cfg.sign_by_default = git_bool(value),
            "user.signingkey" => cfg.key = Some(value.trim().to_string()).filter(|k| !k.is_empty()),
            "gpg.format" if !value.trim().is_empty() => cfg.format = value.trim().to_string(),
            _ => {}
        }
    }
    cfg
}

/// Parses `%G?%x00%GS%x00%GK`.
pub fn parse_signature(out: &str) -> Signature {
    let mut f = out.trim_end_matches(['\n', '\r']).splitn(3, '\0');
    let status = SignatureStatus::from_code(f.next().unwrap_or(""));
    Signature { status, signer: f.next().unwrap_or("").trim().to_string(), key: f.next().unwrap_or("").trim().to_string() }
}

impl Repository {
    /// The signing settings that apply to this repository.
    pub async fn signing_config(&self) -> Result<SigningConfig> {
        let out = self.cmd().args(["config", "--list", "-z"]).output().await?;
        Ok(parse_signing_config(&out.stdout))
    }

    /// Verifies the signature of `rev` (git runs the signing program to do it).
    pub async fn signature(&self, rev: &str) -> Result<Signature> {
        let out = self.cmd().args(["log", "-1", "--no-show-signature", "--format=%G?%x00%GS%x00%GK", "--end-of-options", rev]).text().await?;
        Ok(parse_signature(&out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_signing_settings() {
        let cfg = parse_signing_config(b"user.name\nA\0commit.gpgsign\nfalse\0user.signingkey\nABCD1234\0commit.gpgsign\nyes\0");
        assert_eq!(cfg, SigningConfig { sign_by_default: true, key: Some("ABCD1234".into()), format: "openpgp".into() });
        let cfg = parse_signing_config(b"gpg.format\nssh\0commit.gpgSign\0");
        assert!(cfg.sign_by_default, "a key without a value is true");
        assert_eq!((cfg.key, cfg.format.as_str()), (None, "ssh"));
        assert_eq!(parse_signing_config(b""), SigningConfig { format: "openpgp".into(), ..Default::default() });
    }

    #[test]
    fn reads_signatures() {
        assert_eq!(parse_signature("G\0Test <t@e.x>\0ABCD\n"), Signature { status: SignatureStatus::Good, signer: "Test <t@e.x>".into(), key: "ABCD".into() });
        assert_eq!(parse_signature("N\0\0\n").status, SignatureStatus::Unsigned);
        assert_eq!(parse_signature("").status, SignatureStatus::Unsigned);
        assert_eq!(parse_signature("E\0\0K1").key, "K1");
        assert_eq!(SignatureStatus::from_code("B"), SignatureStatus::Bad);
    }
}
