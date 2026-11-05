//! Every breakpoint of the project in one list, and breakpoint groups (D-27). Breakpoints live
//! in the editors of open files and in `Docs::closed_breakpoints` for the others; conditions,
//! log messages and groups in the debugger's maps. These helpers read and change both alike, so
//! the Breakpoints dialog works for files that are not open, and a running debug session
//! picks the changes up on its next sync.

use crate::app::{CrabCraftApp, clean_group};
use rc_editor::BreakpointKind;
use std::path::{Path, PathBuf};

/// One row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BreakpointRow {
    pub path: PathBuf,
    /// 0-based.
    pub line: usize,
    pub enabled: bool,
    pub condition: Option<String>,
    pub log: Option<String>,
    pub group: Option<String>,
}

impl CrabCraftApp {
    /// Every breakpoint: by group (named groups first, by name; then those in none), then by
    /// file and line.
    pub fn breakpoint_rows(&self) -> Vec<BreakpointRow> {
        let mut rows: Vec<BreakpointRow> = self
            .docs
            .all_breakpoints()
            .into_iter()
            .flat_map(|(path, lines)| {
                lines.into_iter().map(move |(line, kind)| (path.clone(), line, kind)).collect::<Vec<_>>()
            })
            .map(|(path, line, kind)| {
                let key = (path.clone(), line);
                BreakpointRow {
                    enabled: kind != BreakpointKind::Disabled,
                    condition: self.debugger.conditions.get(&key).cloned(),
                    log: self.debugger.logs.get(&key).cloned(),
                    group: self.debugger.groups.get(&key).cloned(),
                    path,
                    line,
                }
            })
            .collect();
        rows.sort_by(|a, b| (a.group.is_none(), a.group.as_ref().map(|g| g.to_lowercase()), &a.path, a.line).cmp(&(b.group.is_none(), b.group.as_ref().map(|g| g.to_lowercase()), &b.path, b.line)));
        rows
    }

    /// The groups in use, sorted.
    pub fn breakpoint_groups(&self) -> Vec<String> {
        let mut groups: Vec<String> = self.breakpoint_rows().into_iter().filter_map(|r| r.group).collect();
        groups.dedup();
        groups
    }

    /// Turns a breakpoint on or off.
    pub fn set_breakpoint_enabled(&mut self, path: &Path, line: usize, enabled: bool) {
        let key = (path.to_path_buf(), line);
        let kind = if !enabled {
            BreakpointKind::Disabled
        } else if self.debugger.logs.contains_key(&key) {
            BreakpointKind::Log
        } else if self.debugger.conditions.contains_key(&key) {
            BreakpointKind::Conditional
        } else {
            BreakpointKind::Normal
        };
        if let Some(k) = self.docs.breakpoints_mut(path).and_then(|b| b.get_mut(&line)) {
            // Keep what the debugger said of an enabled breakpoint that stays enabled.
            if !enabled || *k == BreakpointKind::Disabled {
                *k = kind;
            }
            self.session_dirty = true;
        }
    }

    /// Removes a breakpoint and what was set for it.
    pub fn remove_breakpoint(&mut self, path: &Path, line: usize) {
        self.docs.remove_breakpoint(path, line);
        self.debugger.forget_breakpoint(path, line);
        self.session_dirty = true;
    }

    /// Puts a breakpoint in a group (`None` or a blank name: in none).
    pub fn set_breakpoint_group(&mut self, path: &Path, line: usize, group: Option<&str>) {
        let key = (path.to_path_buf(), line);
        match group.and_then(clean_group) {
            Some(g) => {
                self.debugger.groups.insert(key, g);
            }
            None => {
                self.debugger.groups.remove(&key);
            }
        }
        self.session_dirty = true;
    }

    /// Turns every breakpoint of a group (`None`: those in no group) on or off.
    pub fn set_group_enabled(&mut self, group: Option<&str>, enabled: bool) {
        for r in self.breakpoint_rows().into_iter().filter(|r| r.group.as_deref() == group) {
            self.set_breakpoint_enabled(&r.path, r.line, enabled);
        }
    }

    /// Renames a group; its breakpoints go with it.
    pub fn rename_breakpoint_group(&mut self, old: &str, new: &str) {
        let Some(new) = clean_group(new) else { return };
        for g in self.debugger.groups.values_mut().filter(|g| g.as_str() == old) {
            *g = new.clone();
        }
        self.session_dirty = true;
    }

    /// Ungroups the breakpoints of a group (they stay set).
    pub fn remove_breakpoint_group(&mut self, group: &str) {
        self.debugger.groups.retain(|_, g| g != group);
        self.session_dirty = true;
    }

    /// Opens the file of a breakpoint at its line.
    pub fn go_to_breakpoint(&mut self, path: &Path, line: usize) {
        self.open_at(path,line,0);
    }
}
