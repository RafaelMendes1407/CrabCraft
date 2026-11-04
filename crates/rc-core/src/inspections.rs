//! Inspection profiles (I-18): the level the user gave each inspection, by id. An id is one of
//! CrabCraft's own inspections (`cargo-toml:unknown-key`), or the code of a compiler, Clippy or
//! rust-analyzer diagnostic (`unused_variables`, `clippy::needless_return`, `inactive-code`);
//! `prefix::*` covers every code that starts with `prefix::`.
//!
//! Profiles come from the settings file and from project sessions, which are untrusted: levels
//! are kept as text and read with [`Level::parse`], and [`InspectionProfile::clean`] drops what
//! does not fit.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What an inspection's findings become.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error,
    Warning,
    /// A thin underline; not in the Problems window or on the scroll bar.
    WeakWarning,
    /// Not highlighted; the finding's quick fixes stay in Alt+Enter.
    Info,
    /// Not reported at all.
    Off,
}

impl Level {
    pub const ALL: [Level; 5] = [Level::Error, Level::Warning, Level::WeakWarning, Level::Info, Level::Off];

    /// The name written in settings.
    pub fn id(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::WeakWarning => "weak_warning",
            Level::Info => "info",
            Level::Off => "off",
        }
    }

    pub fn parse(text: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.id() == text.trim())
    }
}

/// Longest inspection id kept, and most entries kept, from an untrusted file.
const MAX_ID: usize = 120;
const MAX_ENTRIES: usize = 500;

/// Whether `id` can name an inspection or a diagnostic code: ASCII letters, digits, `_`, `-`,
/// `.`, `:` and `/`, optionally ending in `::*` for a group.
pub fn valid_id(id: &str) -> bool {
    let body = id.strip_suffix("::*").unwrap_or(id);
    !body.is_empty() && id.len() <= MAX_ID && body.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/'))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InspectionProfile {
    /// Inspection id → level name; an inspection that is not here keeps its own level.
    pub levels: BTreeMap<String, String>,
}

impl InspectionProfile {
    /// The level given to `id`: its own entry, else that of its group (`clippy::*` for
    /// `clippy::needless_return`).
    pub fn level(&self, id: &str) -> Option<Level> {
        if let Some(l) = self.levels.get(id).and_then(|l| Level::parse(l)) {
            return Some(l);
        }
        let (group, _) = id.rsplit_once("::")?;
        self.levels.get(&format!("{group}::*")).and_then(|l| Level::parse(l))
    }

    /// Gives `id` a level (`None`: back to its own). Invalid ids are ignored.
    pub fn set(&mut self, id: &str, level: Option<Level>) {
        if !valid_id(id) {
            return;
        }
        match level {
            Some(l) => {
                self.levels.insert(id.to_string(), l.id().to_string());
            }
            None => {
                self.levels.remove(id);
            }
        }
    }

    /// The profile without invalid ids or unknown levels, within a sane size.
    pub fn clean(&self) -> InspectionProfile {
        let levels = self.levels.iter().filter(|(id, l)| valid_id(id) && Level::parse(l).is_some()).take(MAX_ENTRIES).map(|(id, l)| (id.clone(), l.trim().to_string())).collect();
        InspectionProfile { levels }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_come_from_the_id_or_its_group() {
        let mut p = InspectionProfile::default();
        p.set("unused_variables", Some(Level::Off));
        p.set("clippy::*", Some(Level::WeakWarning));
        p.set("clippy::needless_return", Some(Level::Error));
        assert_eq!(p.level("unused_variables"), Some(Level::Off));
        assert_eq!(p.level("clippy::needless_return"), Some(Level::Error));
        assert_eq!(p.level("clippy::len_zero"), Some(Level::WeakWarning));
        assert_eq!(p.level("dead_code"), None);
        p.set("unused_variables", None);
        assert_eq!(p.level("unused_variables"), None);
        // Not a valid id: nothing is stored.
        p.set("bad id", Some(Level::Error));
        assert!(!p.levels.contains_key("bad id"));
    }

    #[test]
    fn untrusted_profiles_are_cleaned() {
        let raw: InspectionProfile = serde_json::from_str(r#"{"levels": {"dead_code": " off ", "x": "loud", "": "error", "a b": "error", "clippy::*": "info", "unknown_field_is_fine": "warning"}, "other": 1}"#).unwrap();
        let clean = raw.clean();
        assert_eq!(clean.levels.len(), 3, "{clean:?}");
        assert_eq!(clean.level("dead_code"), Some(Level::Off));
        assert_eq!(clean.level("clippy::foo"), Some(Level::Info));
        assert_eq!(clean.level("x"), None);
        let long = "a".repeat(MAX_ID + 1);
        assert!(!valid_id(&long) && !valid_id("::*") && !valid_id("*") && valid_id("rust-analyzer::*"));
        let many = InspectionProfile { levels: (0..MAX_ENTRIES + 10).map(|i| (format!("code{i}"), "off".to_string())).collect() };
        assert_eq!(many.clean().levels.len(), MAX_ENTRIES);
    }
}
