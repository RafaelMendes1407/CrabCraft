//! File and Rust module templates for the project explorer.

use rc_kit::{Ico, ico};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    File,
    Json,
    Text,
    Folder,
    RustFile,
    RustModule,
}

impl ItemKind {
    pub const RUST_MENU: &[Option<ItemKind>] =
        &[Some(ItemKind::RustFile), Some(ItemKind::RustModule), None, Some(ItemKind::File), Some(ItemKind::Json), Some(ItemKind::Text), Some(ItemKind::Folder)];

    pub fn title(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Json => "JSON File",
            Self::Text => "Text File",
            Self::Folder => "Directory",
            Self::RustFile => "Rust File",
            Self::RustModule => "Rust Module",
        }
    }

    pub fn icon(self) -> Ico {
        match self {
            Self::File | Self::RustFile => ico::NEW_FILE,
            Self::Json => ico::FILE_JSON,
            Self::Text => ico::FILE_TEXT,
            Self::Folder | Self::RustModule => ico::FOLDER,
        }
    }

    pub fn is_folder(self) -> bool { self == Self::Folder }
    pub fn is_rust(self) -> bool { matches!(self, Self::RustFile | Self::RustModule) }

    pub fn module_name(typed: &str) -> &str {
        let name = typed.trim();
        name.strip_suffix(".rs").unwrap_or(name)
    }

    pub fn files(self, dir: &Path, typed: &str) -> Vec<(PathBuf, String)> {
        let name = typed.trim();
        match self {
            Self::File => vec![(dir.join(name), "$0".into())],
            Self::Folder => vec![(dir.join(name), String::new())],
            Self::RustFile => vec![(dir.join(format!("{}.rs", Self::module_name(name))), "$0".into())],
            Self::RustModule => vec![(dir.join(Self::module_name(name)).join("mod.rs"), "$0".into())],
            Self::Json | Self::Text => {
                let extension = if self == Self::Json { "json" } else { "txt" };
                let file = if name.contains('.') { name.to_string() } else { format!("{name}.{extension}") };
                let content = if self == Self::Json { "{\n  $0\n}\n" } else { "$0" };
                vec![(dir.join(file), content.into())]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rust_and_plain_files_use_the_selected_directory() {
        let dir = Path::new("/project/src");
        assert_eq!(ItemKind::RustFile.files(dir, " net ")[0].0, dir.join("net.rs"));
        assert_eq!(ItemKind::RustModule.files(dir, "net")[0].0, dir.join("net/mod.rs"));
        assert_eq!(ItemKind::File.files(dir, "Dockerfile")[0].0, dir.join("Dockerfile"));
        assert_eq!(ItemKind::Json.files(dir, "settings")[0].0, dir.join("settings.json"));
    }
}
