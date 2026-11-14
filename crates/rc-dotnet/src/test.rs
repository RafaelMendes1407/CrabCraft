//! Shared test requests and results used by Cargo.

use crate::build::{BuildEvent, BuildResult};
use crate::process::OutputLine;
use crate::test_result::{TestOutcome, TestResult};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestFilter {
    /// Fully qualified names (`Ns.Class.Method`, every data case; or one NUnit case `M(9,0)`).
    Tests(Vec<String>),
    /// Every test of a class (`Ns.Class`).
    Class(String),
    /// Every test under a namespace.
    Namespace(String),
    /// A filter expression passed as is.
    Raw(String),
}

/// A Cargo test run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestRequest {
    pub target: PathBuf,
    pub filter: Option<TestFilter>,
}

impl TestRequest {
    pub fn new(target: impl Into<PathBuf>) -> Self {
        Self { target: target.into(), filter: None }
    }
}

/// One finished test as reported by the console logger while running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestProgress {
    pub project: PathBuf,
    pub outcome: TestOutcome,
    /// The display name printed by the framework.
    pub name: String,
    /// As printed (`12 ms`, `< 1 ms`).
    pub duration: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestRunResult {
    /// Every test result.
    pub results: Vec<TestResult>,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Build ok, every project exited with 0 and no test failed.
    pub success: bool,
    pub cancelled: bool,
    /// `None` with `no_build`.
    pub build: Option<BuildResult>,
    pub duration: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestEvent {
    /// Events of the build that precedes the run.
    Build(BuildEvent),
    ProjectStarted {
        project: PathBuf,
    },
    Output {
        project: PathBuf,
        line: OutputLine,
    },
    Progress(TestProgress),
    /// Always the last event.
    Finished(TestRunResult),
    /// The run could not start; also the last event.
    Failed(String),
}
