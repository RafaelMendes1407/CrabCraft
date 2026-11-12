//! Shared process, build and test types. The crate name is retained for upstream merges.
//! Cargo uses these types; .NET SDK, Roslyn and MSBuild integration have been removed.

pub mod build;
pub mod diagnostics;
pub mod process;
pub mod run;
pub mod test;
pub mod test_result;

pub use build::{BuildAction, BuildEvent, BuildResult};
pub use diagnostics::{Diagnostic, Severity};
pub use process::{CancelToken, OpHandle, OutputLine, Stream, kill_tree, strip_ansi};
pub use run::{LaunchPlan, RunEvent, RunHandle};
pub use test::{TestEvent, TestFilter, TestRequest, TestRunResult};
pub use test_result::{TestOutcome, TestResult};
