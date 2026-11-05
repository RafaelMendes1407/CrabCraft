//! Versioned, portable preferences. Machine paths, trust, commands and history never cross
//! this boundary, even when a hand-edited backup contains them.
use crate::Settings;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{io::{Read, Write}, path::Path};

pub const MAX_BYTES: usize = 1024 * 1024;
const KEYS: &[&str] = &[
    "theme", "language", "keymap", "editor_font_size", "editor_line_spacing", "ui_zoom",
    "show_inlay_hints", "show_indent_guides", "sticky_lines", "scrollbar_lens", "right_margin",
    "autosave", "single_click_open", "tabs_multirow", "tab_colors", "use_clippy", "use_nextest",
    "format_on_save", "format_on_commit", "live_templates", "autoimport_exclude", "imports_on_paste",
    "debugger_hex", "check_crates_io", "shortcuts", "inspection_profile",
];

#[derive(Debug)]
pub enum TransferError {
    Io(std::io::Error),
    Json(serde_json::Error),
    TooLarge,
    Version,
    Invalid,
}
impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The UI translates the typed error; underlying OS/JSON details stay available.
        match self {
            Self::Io(e) => e.fmt(f), Self::Json(e) => e.fmt(f),
            Self::TooLarge => f.write_str("backup exceeds size limit"),
            Self::Version => f.write_str("unsupported backup version"),
            Self::Invalid => f.write_str("invalid portable preferences"),
        }
    }
}
impl std::error::Error for TransferError {}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Backup {
    format: String,
    version: u32,
    preferences: Map<String, Value>,
}
impl Backup {
    pub fn from_settings(settings: &Settings) -> Result<Self, TransferError> {
        let value = serde_json::to_value(settings).map_err(TransferError::Json)?;
        let preferences = value.as_object().ok_or(TransferError::Invalid)?.iter()
            .filter(|(key, _)| KEYS.contains(&key.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
        let backup = Self { format: "CrabCraft preferences".into(), version: 1, preferences };
        backup.validate()?;
        Ok(backup)
    }

    fn validate(&self) -> Result<(), TransferError> {
        if self.version != 1 || self.format != "CrabCraft preferences" { return Err(TransferError::Version); }
        if self.preferences.keys().any(|key| !KEYS.contains(&key.as_str())) { return Err(TransferError::Invalid); }
        let settings: Settings = serde_json::from_value(Value::Object(self.preferences.clone())).map_err(TransferError::Json)?;
        if !(6.0..=72.0).contains(&settings.editor_font_size)
            || !(0.5..=4.0).contains(&settings.editor_line_spacing)
            || !(0.5..=3.0).contains(&settings.ui_zoom)
            || settings.right_margin > 1000 { return Err(TransferError::Invalid); }
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self, TransferError> {
        let metadata = std::fs::metadata(path).map_err(TransferError::Io)?;
        if !metadata.is_file() { return Err(TransferError::Invalid); }
        if metadata.len() > MAX_BYTES as u64 { return Err(TransferError::TooLarge); }
        let file = std::fs::File::open(path).map_err(TransferError::Io)?;
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(TransferError::Io)?;
        if bytes.len() > MAX_BYTES { return Err(TransferError::TooLarge); }
        let backup: Self = serde_json::from_slice(&bytes).map_err(TransferError::Json)?;
        backup.validate()?;
        Ok(backup)
    }

    pub fn write(&self, path: &Path) -> Result<(), TransferError> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(TransferError::Json)?;
        if bytes.len() > MAX_BYTES { return Err(TransferError::TooLarge); }
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(TransferError::Io)?;
        file.write_all(&bytes).map_err(TransferError::Io)?;
        file.as_file().sync_all().map_err(TransferError::Io)?;
        file.persist(path).map_err(|error| TransferError::Io(error.error))?;
        Ok(())
    }

    /// Apply only the fields actually present. Every machine-specific setting is retained.
    pub fn apply(&self, settings: &Settings) -> Result<Settings, TransferError> {
        self.validate()?;
        let mut value = serde_json::to_value(settings).map_err(TransferError::Json)?;
        value.as_object_mut().ok_or(TransferError::Invalid)?.extend(self.preferences.clone());
        serde_json::from_value(value).map_err(TransferError::Json)
    }

    pub fn fields(&self) -> impl Iterator<Item = &str> {
        self.preferences.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_changes_preferences_without_transferring_trust_or_commands() {
        let dir = tempfile::tempdir().unwrap();
        let source = Settings { editor_font_size: 19.0, trusted_projects: vec!["/danger".into()],
            rust_analyzer_path: Some("/evil".into()), ..Settings::default() };
        let backup = Backup::from_settings(&source).unwrap();
        let path = dir.path().join("backup.json");
        backup.write(&path).unwrap();
        let target = Settings { trusted_projects: vec!["/safe".into()], ..Settings::default() };
        let result = Backup::read(&path).unwrap().apply(&target).unwrap();
        assert_eq!(result.editor_font_size, 19.0);
        assert_eq!(result.trusted_projects, target.trusted_projects);
        assert_eq!(result.rust_analyzer_path, None);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("/danger"));
    }
    #[test]
    fn hostile_and_oversized_backups_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup.json");
        for text in [
            r#"{"format":"CrabCraft preferences","version":2,"preferences":{}}"#,
            r#"{"format":"CrabCraft preferences","version":1,"preferences":{"trusted_projects":["/evil"]}}"#,
            r#"{"format":"CrabCraft preferences","version":1,"preferences":{"ui_zoom":0}}"#,
            r#"{"format":"CrabCraft preferences","version":1,"preferences":{"theme":"invalid"}}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(Backup::read(&path).is_err(), "{text}");
        }
        std::fs::write(&path, vec![b' '; MAX_BYTES + 1]).unwrap();
        assert!(matches!(Backup::read(&path), Err(TransferError::TooLarge)));
    }
}
