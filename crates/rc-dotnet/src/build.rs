//! Shared build events and results used by Cargo.

use crate::diagnostics::{Diagnostic, Severity};
use crate::process::OutputLine;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuildAction {
    Build,
    Rebuild,
    Clean,
    Restore,
}

/// Progress of a build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildEvent {
    /// The command line, once the process started.
    Started {
        command: String,
    },
    Output(OutputLine),
    /// Output of a task run before the build (a run configuration's "before launch" tool):
    /// shown even where the build's own output is hidden.
    Task(OutputLine),
    Diagnostic(Box<Diagnostic>),
    /// Always the last event (also when cancelled).
    Finished(BuildResult),
    /// The process could not be started; also the last event.
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildResult {
    pub success: bool,
    pub cancelled: bool,
    pub exit_code: Option<i32>,
    pub errors: usize,
    pub warnings: usize,
    /// Unique diagnostics in output order.
    pub diagnostics: Vec<Diagnostic>,
    pub duration: Duration,
}

impl BuildResult {
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Error)
    }
}
