//! Whole-file local changelists. The empty key is the built-in default list; Git's index
//! remains untouched until the user commits, stashes or explicitly stages files.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Changelists {
    /// List name → draft commit message. The default list has an empty name.
    pub lists: BTreeMap<String, String>,
    /// Git-root-relative file path → list name. Unassigned files belong to the default.
    pub assignments: BTreeMap<String, String>,
    pub active: String,
}

impl Default for Changelists {
    fn default() -> Self {
        Self { lists: BTreeMap::from([(String::new(), String::new())]), assignments: BTreeMap::new(), active: String::new() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangelistError {
    InvalidName,
    AlreadyExists,
    NotFound,
    DefaultList,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 400 && name.chars().count() <= 100 && name.trim() == name && !name.chars().any(char::is_control)
}
fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains(['\\', '\0'])
        && !path.starts_with('/')
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains(':'))
}

impl Changelists {
    /// Session files are untrusted. Unknown lists, invalid paths, oversized names and
    /// messages are discarded before the state is used to select files for Git commands.
    pub fn clean(mut self) -> Self {
        self.lists = self.lists.into_iter().filter(|(name, message)| (name.is_empty() || valid_name(name)) && message.len() <= 64 * 1024).take(100).collect();
        self.lists.entry(String::new()).or_default();
        self.assignments =
            self.assignments.into_iter().filter(|(path, name)| valid_path(path) && !name.is_empty() && self.lists.contains_key(name)).take(100_000).collect();
        if !self.lists.contains_key(&self.active) {
            self.active.clear();
        }
        self
    }
    pub fn list_of<'a>(&'a self, path: &str) -> &'a str {
        self.assignments.get(path).map(String::as_str).unwrap_or("")
    }
    pub fn in_active(&self, path: &str) -> bool {
        self.list_of(path) == self.active
    }
    pub fn create(&mut self, name: &str) -> Result<(), ChangelistError> {
        if !valid_name(name) || self.lists.len() >= 100 {
            return Err(ChangelistError::InvalidName);
        }
        if self.lists.contains_key(name) {
            return Err(ChangelistError::AlreadyExists);
        }
        self.lists.insert(name.to_string(), String::new());
        Ok(())
    }
    pub fn select(&mut self, name: &str, message: &mut String) -> Result<(), ChangelistError> {
        if !self.lists.contains_key(name) {
            return Err(ChangelistError::NotFound);
        }
        if let Some(old) = self.lists.get_mut(&self.active) {
            *old = std::mem::take(message);
        }
        *message = self.lists.get(name).cloned().unwrap_or_default();
        self.active = name.to_string();
        Ok(())
    }
    pub fn move_file(&mut self, path: &str, list: &str) -> Result<(), ChangelistError> {
        if !self.lists.contains_key(list) || !valid_path(path) {
            return Err(ChangelistError::NotFound);
        }
        if list.is_empty() {
            self.assignments.remove(path);
        } else {
            self.assignments.insert(path.to_string(), list.to_string());
        }
        Ok(())
    }
    pub fn rename_active(&mut self, name: &str) -> Result<(), ChangelistError> {
        if self.active.is_empty() {
            return Err(ChangelistError::DefaultList);
        }
        if !valid_name(name) {
            return Err(ChangelistError::InvalidName);
        }
        if self.lists.contains_key(name) {
            return Err(ChangelistError::AlreadyExists);
        }
        let old = self.active.clone();
        let message = self.lists.remove(&old).ok_or(ChangelistError::NotFound)?;
        self.lists.insert(name.to_string(), message);
        for value in self.assignments.values_mut() {
            if *value == old {
                *value = name.to_string();
            }
        }
        self.active = name.to_string();
        Ok(())
    }
    pub fn delete_active(&mut self, message: &mut String) -> Result<(), ChangelistError> {
        if self.active.is_empty() {
            return Err(ChangelistError::DefaultList);
        }
        let old = self.active.clone();
        self.select("", message)?;
        self.lists.remove(&old);
        self.assignments.retain(|_, list| *list != old);
        Ok(())
    }
    pub fn retain_changed(&mut self, paths: &BTreeSet<String>) {
        self.assignments.retain(|path, _| paths.contains(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lists_keep_messages_and_move_files_without_changing_the_index() {
        let mut lists = Changelists::default();
        let mut message = "default message".to_string();
        lists.create("Fix parser").unwrap();
        lists.move_file("src/lib.rs", "Fix parser").unwrap();
        assert!(!lists.in_active("src/lib.rs"));
        assert!(lists.in_active("Cargo.toml"));
        lists.select("Fix parser", &mut message).unwrap();
        assert!(message.is_empty());
        message = "Fix lifetime parsing".to_string();
        lists.rename_active("Parser").unwrap();
        assert_eq!(lists.list_of("src/lib.rs"), "Parser");
        lists.select("", &mut message).unwrap();
        assert_eq!(message, "default message");
        lists.select("Parser", &mut message).unwrap();
        assert_eq!(message, "Fix lifetime parsing");
        lists.delete_active(&mut message).unwrap();
        assert_eq!(message, "default message");
        assert_eq!(lists.list_of("src/lib.rs"), "");
        assert_eq!(lists.delete_active(&mut message), Err(ChangelistError::DefaultList));
    }
    #[test]
    fn untrusted_sessions_and_invalid_edits_are_rejected() {
        let mut lists = Changelists::default();
        assert_eq!(lists.create(" bad "), Err(ChangelistError::InvalidName));
        lists.create("Work").unwrap();
        assert_eq!(lists.create("Work"), Err(ChangelistError::AlreadyExists));
        lists.lists.insert("\nunsafe".into(), String::new());
        lists.lists.insert("Too large".into(), "x".repeat(65 * 1024));
        for path in ["../outside", "/tmp/outside", "C:\\outside", "src//file", "./src/lib.rs"] {
            lists.assignments.insert(path.into(), "Work".into());
        }
        lists.assignments.insert("src/lib.rs".into(), "Missing".into());
        lists.assignments.insert("src/main.rs".into(), "Work".into());
        lists.active = "Missing".into();
        let mut lists = lists.clean();
        assert_eq!(lists.lists.len(), 2);
        assert_eq!(lists.active, "");
        assert_eq!(lists.assignments.len(), 1);
        lists.retain_changed(&BTreeSet::new());
        assert!(lists.assignments.is_empty());
    }
}
