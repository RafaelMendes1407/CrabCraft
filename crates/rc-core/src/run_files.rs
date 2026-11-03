//! Run configurations kept in the project (`.crabcraft/runConfigurations/*.json`), so they can
//! be committed and shared; the session keeps the others.
//!
//! The files come with the project, so they are untrusted: big or broken ones are skipped.

use crate::session::RunOptions;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAX_FILE: u64 = 256 * 1024;

#[derive(serde::Serialize, serde::Deserialize)]
struct RunFile {
    name: String,
    options: RunOptions,
}

pub fn dir(root: &Path) -> PathBuf {
    root.join(".crabcraft").join("runConfigurations")
}

/// A file name for a configuration: letters, digits, `-` and `_`; anything else becomes `_`.
fn file_name(name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{}.json", if safe.is_empty() { "_".to_string() } else { safe })
}

/// Every configuration stored in the project, by name, marked `in_project`.
pub fn load(root: &Path) -> BTreeMap<String, RunOptions> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir(root)) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") || entry.metadata().map_or(true, |m| !m.is_file() || m.len() > MAX_FILE) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(mut file) = serde_json::from_str::<RunFile>(&text) else { continue };
        file.options.in_project = true;
        out.insert(file.name, file.options);
    }
    out
}

/// Writes the configuration `name` into the project.
pub fn save(root: &Path, name: &str, options: &RunOptions) -> std::io::Result<PathBuf> {
    let path = dir(root).join(file_name(name));
    std::fs::create_dir_all(dir(root))?;
    let mut options = options.clone();
    // Being in the project is what the file itself says.
    options.in_project = false;
    let text = serde_json::to_string_pretty(&RunFile { name: name.to_string(), options }).map_err(std::io::Error::other)?;
    crate::paths::write_atomic(&path, format!("{text}\n").as_bytes())?;
    Ok(path)
}

/// Removes the project's file of `name`, if there is one.
pub fn remove(root: &Path, name: &str) -> std::io::Result<()> {
    match std::fs::remove_file(dir(root).join(file_name(name))) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configurations_round_trip_through_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(load(root).is_empty());
        let options = RunOptions { args: "--port 8080".into(), allow_parallel: true, in_project: true, ..Default::default() };
        let path = save(root, "server: dev/1", &options).unwrap();
        assert_eq!(path.file_name().unwrap(), "server__dev_1.json");
        assert!(!std::fs::read_to_string(&path).unwrap().contains("in_project\": true"));
        let loaded = load(root);
        assert_eq!(loaded.get("server: dev/1"), Some(&options));
        remove(root, "server: dev/1").unwrap();
        remove(root, "server: dev/1").unwrap();
        assert!(load(root).is_empty());
    }

    #[test]
    fn broken_and_big_files_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(super::dir(root)).unwrap();
        std::fs::write(super::dir(root).join("a.json"), "{ not json").unwrap();
        std::fs::write(super::dir(root).join("b.json"), format!("{{\"name\":\"b\",\"options\":{{}},\"pad\":\"{}\"}}", "x".repeat(MAX_FILE as usize))).unwrap();
        std::fs::write(super::dir(root).join("c.txt"), "{\"name\":\"c\",\"options\":{}}").unwrap();
        std::fs::write(super::dir(root).join("d.json"), "{\"name\":\"d\",\"options\":{\"args\":\"x\"}}").unwrap();
        let loaded = load(root);
        assert_eq!(loaded.keys().collect::<Vec<_>>(), ["d"]);
        assert_eq!(loaded["d"].args, "x");
    }
}
