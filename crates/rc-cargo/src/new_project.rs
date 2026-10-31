//! `cargo new`: the New Project dialog's templates.

use crate::toolchain::Cargo;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    Binary,
    Library,
}

/// What to create.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProject {
    pub template: Template,
    pub name: String,
    /// The folder that receives the project's folder.
    pub location: PathBuf,
    pub git: bool,
}

/// What Cargo accepts as a new package name, and is not a Rust keyword.
pub fn valid_name(name: &str) -> bool {
    const RESERVED: [&str; 12] = ["as", "crate", "fn", "impl", "let", "mod", "self", "struct", "super", "test", "type", "use"];
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !RESERVED.contains(&name)
}

impl NewProject {
    pub fn project_dir(&self) -> PathBuf {
        self.location.join(&self.name)
    }

    /// Why the project cannot be created, as a text to translate.
    pub fn problem(&self) -> Option<&'static str> {
        if !valid_name(&self.name) {
            Some("Use letters, digits, - and _, starting with a letter")
        } else if !self.location.is_dir() {
            Some("The location does not exist")
        } else if self.project_dir().exists() {
            Some("That folder already exists")
        } else {
            None
        }
    }

    /// Arguments after `cargo`.
    pub fn args(&self) -> Vec<String> {
        let kind = match self.template {
            Template::Binary => "--bin",
            Template::Library => "--lib",
        };
        vec!["new".into(), kind.into(), "--vcs".into(), if self.git { "git" } else { "none" }.into(), "--name".into(), self.name.clone(), self.project_dir().to_string_lossy().into_owned()]
    }

    /// Creates the project and returns its folder. The folder is removed if Cargo fails half way.
    pub async fn create(&self, cargo: &Cargo) -> anyhow::Result<PathBuf> {
        if let Some(p) = self.problem() {
            anyhow::bail!("{p}");
        }
        let dir = self.project_dir();
        let mut cmd: Command = cargo.command();
        let out = cmd.args(self.args()).current_dir(&self.location).output().await?;
        if !out.status.success() {
            if dir.is_dir() && !was_left_with_content(&dir) {
                let _ = std::fs::remove_dir(&dir);
            }
            anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(dir)
    }
}

fn was_left_with_content(dir: &Path) -> bool {
    std::fs::read_dir(dir).map_or(true, |mut e| e.next().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(location: &Path, name: &str) -> NewProject {
        NewProject { template: Template::Library, name: name.into(), location: location.to_path_buf(), git: false }
    }

    #[test]
    fn names_follow_cargo() {
        for ok in ["demo", "my-app", "my_app2", "a"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "2fast", "-a", "a b", "café", "self", "../x", "a/b"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn problems_are_found_before_creating() {
        let dir = tempfile::tempdir().unwrap();
        assert!(request(dir.path(), "1bad").problem().is_some());
        assert!(request(&dir.path().join("missing"), "ok").problem().is_some());
        std::fs::create_dir(dir.path().join("taken")).unwrap();
        assert!(request(dir.path(), "taken").problem().is_some());
        assert_eq!(request(dir.path(), "free").problem(), None);
    }

    #[test]
    fn creates_a_real_project() {
        let dir = tempfile::tempdir().unwrap();
        let cargo = Cargo::locate().expect("cargo is installed");
        let lib = request(dir.path(), "made_here");
        let path = rc_core::Runtime::block_on(lib.create(&cargo)).unwrap();
        assert!(path.join("src/lib.rs").is_file() && path.join("Cargo.toml").is_file());
        assert!(!path.join(".git").exists());
        let bin = NewProject { template: Template::Binary, name: "made_too".into(), location: dir.path().to_path_buf(), git: true };
        let path = rc_core::Runtime::block_on(bin.create(&cargo)).unwrap();
        assert!(path.join("src/main.rs").is_file() && path.join(".git").exists());
        assert!(rc_core::Runtime::block_on(bin.create(&cargo)).is_err(), "the folder exists now");
    }
}
