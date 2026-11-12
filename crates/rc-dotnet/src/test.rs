//! Shared test requests and results used by Cargo.

use std::path::PathBuf;
use std::time::Duration;
use crate::build::{BuildEvent, BuildResult};
use crate::process::OutputLine;
use crate::trx::{TestOutcome, TestResult};

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

/// A test run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestRequest {
    /// Solution or test project.
    pub target: PathBuf,
    pub configuration: String,
    pub target_framework: Option<String>,
    pub filter: Option<TestFilter>,
    /// Skip the build (the assemblies must be up to date).
    pub no_build: bool,
    /// Where TRX files go (a new temporary folder when `None`).
    pub results_dir: Option<PathBuf>,
    /// Test projects run at the same time.
    pub max_parallel: usize,
}

impl TestRequest {
    pub fn new(target: impl Into<PathBuf>) -> Self {
        TestRequest {
            target: target.into(),
            configuration: "Debug".into(),
            target_framework: None,
            filter: None,
            no_build: false,
            results_dir: None,
            max_parallel: 4,
        }
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

/// Results of one test project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRun {
    pub project: PathBuf,
    pub exit_code: Option<i32>,
    pub trx: Option<PathBuf>,
    pub results: Vec<TestResult>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestRunResult {
    /// Every result of every project.
    pub results: Vec<TestResult>,
    pub projects: Vec<ProjectRun>,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Build ok, every project exited with 0 and no test failed.
    pub success: bool,
    pub cancelled: bool,
    /// `None` with `no_build`.
    pub build: Option<BuildResult>,
    pub results_dir: PathBuf,
    pub duration: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestEvent {
    /// Events of the build that precedes the run.
    Build(BuildEvent),
    ProjectStarted { project: PathBuf },
    Output { project: PathBuf, line: OutputLine },
    Progress(TestProgress),
    ProjectFinished(ProjectRun),
    /// Always the last event.
    Finished(TestRunResult),
    /// The run could not start; also the last event.
    Failed(String),
}
