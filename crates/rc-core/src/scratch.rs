//! Scratch files: throwaway files kept outside every project, in CrabCraft's data directory, so
//! they survive restarts and are the same in every workspace.

use std::path::{Path, PathBuf};

/// The kinds of scratch file the New Scratch File list offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Rust,
    Toml,
    Markdown,
    Json,
    Text,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Rust, Kind::Toml, Kind::Markdown, Kind::Json, Kind::Text];

    pub fn extension(self) -> &'static str {
        match self {
            Kind::Rust => "rs",
            Kind::Toml => "toml",
            Kind::Markdown => "md",
            Kind::Json => "json",
            Kind::Text => "txt",
        }
    }

    /// What a new file of this kind starts with.
    pub fn template(self) -> &'static str {
        match self {
            Kind::Rust => "fn main() {\n    \n}\n",
            _ => "",
        }
    }
}

/// Creates the first free `scratch_N.ext` in `dir` (creating `dir` too) and returns its path.
pub fn create(dir: &Path, kind: Kind) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for n in 1..10_000 {
        let path = dir.join(format!("scratch_{n}.{}", kind.extension()));
        // `create_new` fails when the file exists, so two windows never pick the same name.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                std::io::Write::write_all(&mut file, kind.template().as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many scratch files"))
}

/// The scratch files in `dir`, most recently changed first. A missing directory has none.
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), e.path()))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    files.into_iter().map(|(_, p)| p).collect()
}

/// Whether `path` (canonical or not) is a scratch file kept in `dir`.
pub fn is_in(dir: &Path, path: &Path) -> bool {
    path.starts_with(dir) || dir.canonicalize().is_ok_and(|d| path.starts_with(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_files_get_free_numbered_names_and_a_template() {
        let dir = std::env::temp_dir().join(format!("crabcraft-scratch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(list(&dir).is_empty());
        let a = create(&dir, Kind::Rust).unwrap();
        let b = create(&dir, Kind::Rust).unwrap();
        let c = create(&dir, Kind::Markdown).unwrap();
        assert_eq!(a.file_name().unwrap(), "scratch_1.rs");
        assert_eq!(b.file_name().unwrap(), "scratch_2.rs");
        assert_eq!(c.file_name().unwrap(), "scratch_1.md");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "fn main() {\n    \n}\n");
        assert_eq!(std::fs::read_to_string(&c).unwrap(), "");
        let mut all = list(&dir);
        all.sort();
        assert!(is_in(&dir, &a.canonicalize().unwrap()) && !is_in(&dir, &std::env::temp_dir()));
        assert_eq!(all, vec![c, a, b]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
