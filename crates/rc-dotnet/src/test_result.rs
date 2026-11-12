//! Shared test outcomes and failure details used by Cargo.

use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TestOutcome {
    Passed,
    Failed,
    Skipped,
}

/// One frame of a test failure stack trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackFrame {
    pub text: String,
    pub file: Option<PathBuf>,
    pub line: Option<u32>,
}

/// The result of one test (one row per data case of a theory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestResult {
    /// `Namespace.Class.Method`, without data arguments.
    pub fully_qualified_name: String,
    /// Display name shown by the test runner.
    pub display_name: String,
    pub class_name: String,
    pub method_name: String,
    pub outcome: TestOutcome,
    pub duration: Duration,
    pub error_message: Option<String>,
    pub stack_trace: Vec<StackFrame>,
    pub stdout: String,
    pub stderr: String,
    /// Source containing the test, when available.
    pub source: Option<PathBuf>,
}
