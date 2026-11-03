//! Local history: earlier versions of files, kept by the IDE whatever the version control.
//!
//! Each file gets a folder named after a hash of its path, holding a `path` file (the full
//! path, to tell hash collisions apart) and one file per revision named
//! `<unix millis>-<content hash>.txt`. The hash in the name lets a new snapshot be compared
//! with the newest one without reading it. Folders and names that do not parse are ignored:
//! the history folder is untrusted like any other file.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Revisions kept per file.
pub const MAX_REVISIONS: usize = 100;
/// Revisions older than this are dropped.
pub const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Bigger texts are not kept (and bigger revision files are not read).
pub const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug)]
pub enum HistoryError {
    /// The text is bigger than [`MAX_BYTES`].
    TooLarge,
    /// The revision is not valid UTF-8 (a damaged history file).
    Corrupt,
    Io(std::io::Error),
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HistoryError::TooLarge => f.write_str("the file is too large for the local history"),
            HistoryError::Corrupt => f.write_str("the history file is damaged"),
            HistoryError::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for HistoryError {}

impl From<std::io::Error> for HistoryError {
    fn from(e: std::io::Error) -> Self {
        HistoryError::Io(e)
    }
}

/// One stored version of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    /// When it was recorded, in Unix milliseconds.
    pub time_ms: u64,
    /// Size of the text in bytes.
    pub len: u64,
    /// FNV-1a hash of the text.
    pub hash: u64,
    pub file: PathBuf,
}

#[derive(Clone, Debug)]
pub struct LocalHistory {
    root: PathBuf,
    pub max_revisions: usize,
    pub max_age: Duration,
}

/// 64-bit FNV-1a: stable across runs and Rust versions (unlike `DefaultHasher`).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl LocalHistory {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        LocalHistory { root: root.into(), max_revisions: MAX_REVISIONS, max_age: MAX_AGE }
    }

    /// The history in the data folder.
    pub fn open_default() -> Self {
        Self::new(crate::paths::history_dir())
    }

    fn dir_of(&self, path: &Path) -> PathBuf {
        self.root.join(format!("{:016x}", fnv1a(path.to_string_lossy().as_bytes())))
    }

    /// Whether `dir` belongs to `path` (and not to another path with the same hash).
    fn owns(dir: &Path, path: &Path) -> bool {
        std::fs::read(dir.join("path")).is_ok_and(|p| p == path.to_string_lossy().as_bytes())
    }

    /// Stores `text` as the newest version of `path`, unless it equals the newest one.
    /// Returns whether a revision was written. Old revisions are pruned.
    pub fn record(&self, path: &Path, text: &str, now_ms: u64) -> Result<bool, HistoryError> {
        if text.len() > MAX_BYTES {
            return Err(HistoryError::TooLarge);
        }
        let dir = self.dir_of(path);
        if dir.exists() && !Self::owns(&dir, path) {
            // A hash collision or a damaged folder: start over for this path.
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        if !dir.join("path").exists() {
            std::fs::write(dir.join("path"), path.to_string_lossy().as_bytes())?;
        }
        let hash = fnv1a(text.as_bytes());
        let revisions = Self::list(&dir);
        if revisions.first().is_some_and(|r| r.hash == hash) {
            return Ok(false);
        }
        // Never before the newest revision (clock changes), and never two at the same time.
        let mut time = now_ms.max(revisions.first().map_or(0, |r| r.time_ms + 1));
        while dir.join(Self::name(time, hash)).exists() {
            time += 1;
        }
        crate::paths::write_atomic(&dir.join(Self::name(time, hash)), text.as_bytes())?;
        self.prune(&dir, time);
        Ok(true)
    }

    fn name(time: u64, hash: u64) -> String {
        format!("{time:013}-{hash:016x}.txt")
    }

    /// Drops revisions beyond the count limit or older than the age limit; the newest one stays.
    fn prune(&self, dir: &Path, now_ms: u64) {
        let max_age = self.max_age.as_millis() as u64;
        for (i, r) in Self::list(dir).into_iter().enumerate() {
            if i > 0 && (i >= self.max_revisions || now_ms.saturating_sub(r.time_ms) > max_age) {
                let _ = std::fs::remove_file(&r.file);
            }
        }
    }

    /// The revisions in `dir`, newest first. Unknown files are skipped.
    fn list(dir: &Path) -> Vec<Revision> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut out: Vec<Revision> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name();
                let name = name.to_str()?;
                let (time, hash) = name.strip_suffix(".txt")?.split_once('-')?;
                let (time_ms, hash) = (time.parse().ok()?, u64::from_str_radix(hash, 16).ok()?);
                let len = e.metadata().ok().filter(|m| m.is_file())?.len();
                Some(Revision { time_ms, len, hash, file: e.path() })
            })
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.time_ms));
        out
    }

    /// The stored versions of `path`, newest first.
    pub fn revisions(&self, path: &Path) -> Vec<Revision> {
        let dir = self.dir_of(path);
        if !Self::owns(&dir, path) {
            return Vec::new();
        }
        Self::list(&dir)
    }

    /// The text of a revision.
    pub fn read(&self, revision: &Revision) -> Result<String, HistoryError> {
        let len = std::fs::metadata(&revision.file)?.len();
        if len > MAX_BYTES as u64 {
            return Err(HistoryError::TooLarge);
        }
        String::from_utf8(std::fs::read(&revision.file)?).map_err(|_| HistoryError::Corrupt)
    }

    /// Removes the folders of files whose every revision is older than the age limit.
    pub fn prune_all(&self, now_ms: u64) {
        let max_age = self.max_age.as_millis() as u64;
        let Ok(entries) = std::fs::read_dir(&self.root) else { return };
        for e in entries.flatten() {
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            let newest = Self::list(&dir).first().map(|r| r.time_ms);
            if newest.is_none_or(|t| now_ms.saturating_sub(t) > max_age) {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 60 * 60 * 1000;

    #[test]
    fn local_history_records_dedups_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let h = LocalHistory::new(dir.path().join("history"));
        let file = dir.path().join("src/main.rs");
        assert!(h.revisions(&file).is_empty());
        assert!(h.record(&file, "fn main() {}\n", 1_000).unwrap());
        // The same text again is not a new revision.
        assert!(!h.record(&file, "fn main() {}\n", 2_000).unwrap());
        assert!(h.record(&file, "fn main() { println!(); }\n", 3_000).unwrap());
        // Same millisecond: still two distinct revisions, in order.
        assert!(h.record(&file, "fn main() {}\n", 3_000).unwrap());
        let revs = h.revisions(&file);
        assert_eq!(revs.iter().map(|r| r.time_ms).collect::<Vec<_>>(), [3_001, 3_000, 1_000]);
        assert_eq!(h.read(&revs[1]).unwrap(), "fn main() { println!(); }\n");
        assert_eq!(revs[2].len, 13);
        // Another file has its own history.
        assert!(h.revisions(&dir.path().join("src/lib.rs")).is_empty());
    }

    #[test]
    fn local_history_prunes_by_count_and_age() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = LocalHistory::new(dir.path());
        h.max_revisions = 3;
        let file = dir.path().join("a.rs");
        for i in 0..5u64 {
            h.record(&file, &format!("v{i}"), 10 + i).unwrap();
        }
        let texts: Vec<String> = h.revisions(&file).iter().map(|r| h.read(r).unwrap()).collect();
        assert_eq!(texts, ["v4", "v3", "v2"]);
        // Forty days later only the new revision stays.
        h.record(&file, "new", 40 * DAY).unwrap();
        assert_eq!(h.revisions(&file).len(), 1);
        // Whole folders of files untouched for longer than the limit go away.
        let other = dir.path().join("b.rs");
        h.record(&other, "old", 1).unwrap();
        h.prune_all(40 * DAY);
        assert!(h.revisions(&other).is_empty());
        assert_eq!(h.revisions(&file).len(), 1);
    }

    #[test]
    fn local_history_survives_damaged_files() {
        let dir = tempfile::tempdir().unwrap();
        let h = LocalHistory::new(dir.path());
        let file = dir.path().join("a.rs");
        h.record(&file, "one", 1).unwrap();
        let folder = h.dir_of(&file);
        std::fs::write(folder.join("garbage.txt"), "x").unwrap();
        std::fs::write(folder.join("12-zz.txt"), "x").unwrap();
        std::fs::create_dir(folder.join("0000000000005-0000000000000000.txt")).unwrap();
        std::fs::write(folder.join("0000000000002-0000000000000001.txt"), [0xff, 0xfe, 0x00]).unwrap();
        let revs = h.revisions(&file);
        assert_eq!(revs.len(), 2);
        assert!(matches!(h.read(&revs[0]), Err(HistoryError::Corrupt)));
        assert_eq!(h.read(&revs[1]).unwrap(), "one");
        // A folder that belongs to another path (or lost its `path` file) is not shown, and is
        // replaced on the next record.
        std::fs::write(folder.join("path"), "elsewhere").unwrap();
        assert!(h.revisions(&file).is_empty());
        assert!(h.record(&file, "two", 3).unwrap());
        assert_eq!(h.revisions(&file).len(), 1);
        assert!(matches!(h.record(&file, &"x".repeat(MAX_BYTES + 1), 4), Err(HistoryError::TooLarge)));
    }
}
