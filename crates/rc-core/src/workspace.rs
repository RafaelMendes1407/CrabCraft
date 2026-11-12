//! The open workspace: a folder or Cargo manifest. Directory listings for the
//! explorer are read lazily and skip build output and tool folders.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceKind {
    Folder,
    /// A folder with a `Cargo.toml`, the path of that manifest.
    Cargo(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    /// Folder that contains everything.
    pub root: PathBuf,
    pub kind: WorkspaceKind,
}

impl Workspace {
    /// Opens a folder or Cargo manifest.
    pub fn open(path: &Path) -> std::io::Result<Workspace> {
        let path = path.canonicalize()?;
        if path.is_dir() {
            let manifest = path.join("Cargo.toml");
            let kind = if manifest.is_file() { WorkspaceKind::Cargo(manifest) } else { WorkspaceKind::Folder };
            return Ok(Workspace { root: path, kind });
        }
        let root = path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let kind = if path.file_name().is_some_and(|n| n == "Cargo.toml") {
            WorkspaceKind::Cargo(path)
        } else {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a folder or Cargo manifest"));
        };
        Ok(Workspace { root, kind })
    }

    /// Display name of the folder.
    pub fn name(&self) -> String {
        let p = match &self.kind {
            WorkspaceKind::Folder | WorkspaceKind::Cargo(_) => &self.root,
        };
        p.file_stem().or_else(|| p.file_name()).map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
    }

    /// The path the session and the recent list are keyed by.
    pub fn key_path(&self) -> &Path {
        match &self.kind {
            WorkspaceKind::Folder | WorkspaceKind::Cargo(_) => &self.root,
        }
    }

    pub fn cargo_manifest(&self) -> Option<&Path> {
        match &self.kind {
            WorkspaceKind::Cargo(m) => Some(m),
            _ => None,
        }
    }

}

/// Folders never shown in the explorer.
pub const HIDDEN_DIRS: &[&str] = &["target", "bin", "obj", ".git", ".vs", ".idea", "node_modules", ".crabcraft", "TestResults"];
const HIDDEN_FILES: &[&str] = &[".DS_Store", "Thumbs.db"];

pub fn is_hidden(name: &str, is_dir: bool) -> bool {
    if is_dir { HIDDEN_DIRS.contains(&name) } else { HIDDEN_FILES.contains(&name) || name.ends_with(".user") || name.ends_with(".suo") }
}

/// Whether the root-relative, `/`-separated path `rel` is one of the folders or files the user
/// excluded, or lies inside one of them.
pub fn is_excluded(rel: &str, excluded: &[String]) -> bool {
    excluded.iter().any(|e| !e.is_empty() && rel.strip_prefix(e.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with('/')))
}

/// Cleans a list of excluded paths read from a session file: each must be relative to the root,
/// without `.`/`..` parts; backslashes count as `/`. Duplicates and paths inside another entry go.
pub fn clean_excluded(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = list
        .iter()
        .filter_map(|e| {
            let e = e.replace('\\', "/");
            let parts: Vec<&str> = e.split('/').filter(|p| !p.is_empty()).collect();
            let valid = !parts.is_empty() && !e.starts_with('/') && !e.contains(':') && parts.iter().all(|p| *p != "." && *p != "..");
            valid.then(|| parts.join("/"))
        })
        .collect();
    out.sort();
    out.dedup();
    let all = out.clone();
    out.retain(|e| !all.iter().any(|o| o != e && is_excluded(e, std::slice::from_ref(o))));
    out
}

/// The root-relative, `/`-separated form of `path` (`None` outside `root` or for the root itself).
pub fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileNode {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// Children of `dir`: folders first, then files, each sorted case-insensitively.
pub fn list_dir(dir: &Path) -> Vec<FileNode> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<FileNode> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().ok()?.is_dir();
            (!is_hidden(&name, is_dir)).then(|| FileNode { path: e.path(), name, is_dir })
        })
        .collect();
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_opens_as_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("App.sln"), "").unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        std::fs::create_dir(dir.path().join("Src")).unwrap();
        std::fs::write(dir.path().join("a.cs"), "").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        assert!(matches!(ws.kind, WorkspaceKind::Folder));
        assert_eq!(ws.name(), dir.path().file_name().unwrap().to_string_lossy());
        let names: Vec<String> = list_dir(dir.path()).into_iter().map(|n| n.name).collect();
        assert_eq!(names, vec!["Src", "a.cs", "App.sln"]);
    }

    #[test]
    fn excluded_paths_cover_their_contents_only() {
        let ex = vec!["gen".to_string(), "docs/old.md".to_string()];
        assert!(is_excluded("gen", &ex));
        assert!(is_excluded("gen/a/b.rs", &ex));
        assert!(!is_excluded("generated/b.rs", &ex));
        assert!(is_excluded("docs/old.md", &ex));
        assert!(!is_excluded("docs/old.md.bak", &ex));
        assert!(!is_excluded("src/gen", &ex));
        assert!(!is_excluded("x", &[String::new()]));
    }

    #[test]
    fn excluded_paths_from_a_session_are_cleaned() {
        let raw: Vec<String> = ["gen\\sub", "gen", "../outside", "/abs", "C:/x", "a/./b", "", "docs/", "docs"].into_iter().map(String::from).collect();
        assert_eq!(clean_excluded(&raw), vec!["docs".to_string(), "gen".to_string()]);
        let root = Path::new("/ws");
        assert_eq!(relative_path(root, Path::new("/ws/a/b.rs")).as_deref(), Some("a/b.rs"));
        assert_eq!(relative_path(root, root), None);
        assert_eq!(relative_path(root, Path::new("/other/a")), None);
    }
}
