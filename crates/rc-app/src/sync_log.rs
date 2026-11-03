//! The Sync log: every `cargo metadata` run that loaded the project's model or its
//! dependencies, with what Cargo printed, shown in the Build window's Sync tab.

use rc_kit::i18n::tr;

/// Older runs are dropped past this many.
const MAX_ENTRIES: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncKind {
    /// Members and targets (`cargo metadata --no-deps`).
    Project,
    /// Dependencies from `Cargo.lock` (`--offline --locked`).
    Dependencies,
    /// An attached Cargo project's members and targets.
    Attached,
}

impl SyncKind {
    pub fn title(self) -> &'static str {
        match self {
            SyncKind::Project => tr("Cargo project"),
            SyncKind::Dependencies => tr("Dependencies"),
            SyncKind::Attached => tr("Attached project"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SyncEntry {
    pub kind: SyncKind,
    /// When it finished, in milliseconds since the Unix epoch.
    pub time_ms: u64,
    pub log: rc_cargo::LoadLog,
}

#[derive(Default)]
pub struct SyncLog {
    pub entries: Vec<SyncEntry>,
    /// Index into `entries` of the run whose output is shown.
    pub selected: Option<usize>,
    /// The Build window shows the Sync tab instead of the build output.
    pub tab_open: bool,
}

impl SyncLog {
    pub fn push(&mut self, kind: SyncKind, log: rc_cargo::LoadLog) {
        let time_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        self.entries.push(SyncEntry { kind, time_ms, log });
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.selected = Some(self.entries.len() - 1);
    }

    pub fn clear(&mut self) {
        *self = SyncLog::default();
    }

    /// The newest load of the project's model failed: the targets shown may be stale. (The
    /// dependencies fail on their own whenever `Cargo.lock` is missing or stale, which is
    /// normal before the first build, so they do not count.)
    pub fn last_failed(&self) -> bool {
        self.entries.iter().rev().find(|e| e.kind == SyncKind::Project).is_some_and(|e| !e.log.success)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(success: bool) -> rc_cargo::LoadLog {
        rc_cargo::LoadLog { command: "cargo metadata".into(), stderr: String::new(), duration: std::time::Duration::from_millis(5), success }
    }

    #[test]
    fn keeps_the_newest_runs_and_selects_the_last() {
        let mut s = SyncLog::default();
        for _ in 0..MAX_ENTRIES + 5 {
            s.push(SyncKind::Project, log(true));
        }
        assert_eq!(s.entries.len(), MAX_ENTRIES);
        assert_eq!(s.selected, Some(MAX_ENTRIES - 1));
        assert!(!s.last_failed());
        s.push(SyncKind::Dependencies, log(false));
        assert!(!s.last_failed(), "a missing Cargo.lock is not a failure of the project");
        s.push(SyncKind::Project, log(false));
        assert!(s.last_failed());
        s.push(SyncKind::Project, log(true));
        assert!(!s.last_failed(), "only the newest run counts");
        s.clear();
        assert!(s.entries.is_empty() && s.selected.is_none());
    }
}
