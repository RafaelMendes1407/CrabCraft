//! Parses the human output of libtest. Its JSON format needs a nightly compiler.

use rc_dotnet::trx::StackFrame;
use rc_dotnet::{TestOutcome, TestResult};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Follows a `cargo test` run line by line.
#[derive(Default)]
pub struct TestParser {
    suite: Option<PathBuf>,
    results: Vec<TestResult>,
    /// Output of failed tests, by test name.
    failures: BTreeMap<String, String>,
    section: Option<String>,
}

/// A test that just finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    pub name: String,
    pub outcome: TestOutcome,
}

impl TestParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one line of output.
    pub fn line(&mut self, line: &str) -> Option<Finished> {
        let text = line.trim_end();
        let trimmed = text.trim_start();
        if let Some(rest) = trimmed.strip_prefix("Running ") {
            self.suite = rest.split('(').nth(1).map(|p| PathBuf::from(p.trim_end_matches(')')));
            self.section = None;
            return None;
        }
        if trimmed.starts_with("Doc-tests ") {
            self.suite = Some(PathBuf::from(trimmed));
            self.section = None;
            return None;
        }
        if let Some(name) = text.strip_prefix("---- ").and_then(|t| t.strip_suffix(" stdout ----").or_else(|| t.strip_suffix(" stderr ----"))) {
            self.section = Some(name.to_string());
            return None;
        }
        if text == "failures:" || text.starts_with("test result:") {
            self.section = None;
            return None;
        }
        if let Some(name) = &self.section {
            let entry = self.failures.entry(name.clone()).or_default();
            entry.push_str(text);
            entry.push('\n');
            return None;
        }
        let (name, outcome) = parse_result(text)?;
        self.results.push(result(&name, outcome, self.suite.clone()));
        Some(Finished { name, outcome })
    }

    /// Every result, with the output of the failed ones and where they panicked. Relative paths
    /// in panic messages are resolved against `root`, the folder Cargo ran in.
    pub fn finish(mut self, root: &Path) -> Vec<TestResult> {
        for r in &mut self.results {
            if r.outcome == TestOutcome::Failed {
                let text = self.failures.get(&r.fully_qualified_name).map(|t| t.trim().to_string()).unwrap_or_default();
                r.stack_trace = text.lines().filter_map(|l| panic_frame(l, root)).collect();
                r.error_message = (!text.is_empty()).then_some(text);
            }
        }
        self.results
    }
}

/// The place in `thread 'x' (123) panicked at src/lib.rs:14:9:`.
fn panic_frame(line: &str, root: &Path) -> Option<StackFrame> {
    let (_, place) = line.split_once(" panicked at ")?;
    let place = place.trim_end().trim_end_matches(':');
    let (rest, column) = place.rsplit_once(':')?;
    let (file, number) = rest.rsplit_once(':')?;
    column.parse::<u32>().ok()?;
    let number = number.parse::<u32>().ok()?;
    (!file.is_empty()).then(|| StackFrame { text: place.to_string(), file: Some(root.join(file)), line: Some(number) })
}

/// `test a::b ... ok`, `test a::b - should panic ... ok`, `test src/lib.rs - f (line 3) ... FAILED`,
/// `test a ... ignored, needs a database`.
pub fn parse_result(line: &str) -> Option<(String, TestOutcome)> {
    let rest = line.strip_prefix("test ")?;
    let (name, status) = rest.rsplit_once(" ... ")?;
    let outcome = match status {
        "ok" => TestOutcome::Passed,
        "FAILED" => TestOutcome::Failed,
        s if s == "ignored" || s.starts_with("ignored,") => TestOutcome::Skipped,
        _ => return None,
    };
    let name = name.strip_suffix(" - should panic").unwrap_or(name).to_string();
    (!name.is_empty()).then_some((name, outcome))
}

fn result(name: &str, outcome: TestOutcome, source: Option<PathBuf>) -> TestResult {
    let (class, method) = name.rsplit_once("::").unwrap_or(("", name));
    TestResult {
        fully_qualified_name: name.to_string(),
        display_name: name.to_string(),
        class_name: class.to_string(),
        method_name: method.to_string(),
        outcome,
        duration: Duration::ZERO,
        error_message: None,
        stack_trace: Vec::new(),
        stdout: String::new(),
        stderr: String::new(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_kind_of_result_line() {
        assert_eq!(parse_result("test a::b ... ok"), Some(("a::b".into(), TestOutcome::Passed)));
        assert_eq!(parse_result("test a::b ... FAILED"), Some(("a::b".into(), TestOutcome::Failed)));
        assert_eq!(parse_result("test a ... ignored"), Some(("a".into(), TestOutcome::Skipped)));
        assert_eq!(parse_result("test a ... ignored, needs a database"), Some(("a".into(), TestOutcome::Skipped)));
        assert_eq!(parse_result("test a::b - should panic ... ok"), Some(("a::b".into(), TestOutcome::Passed)));
        assert_eq!(parse_result("test src/lib.rs - add (line 3) ... ok"), Some(("src/lib.rs - add (line 3)".into(), TestOutcome::Passed)));
    }

    #[test]
    fn finds_where_a_test_panicked() {
        let root = Path::new("/w");
        let f = panic_frame("thread 'a::b' (4242) panicked at src/lib.rs:14:9:", root).unwrap();
        assert_eq!((f.file.as_deref(), f.line), (Some(Path::new("/w/src/lib.rs")), Some(14)));
        assert_eq!(f.text, "src/lib.rs:14:9");
        let f = panic_frame("thread 'a' panicked at /rust/lib/core.rs:7:1:", root).unwrap();
        assert_eq!(f.file.as_deref(), Some(Path::new("/rust/lib/core.rs")));
        for bad in ["", "panicked at", "thread 'a' panicked at src/lib.rs:", "x panicked at a:b:c:", "x panicked at :1:2:"] {
            assert!(panic_frame(bad, root).is_none(), "{bad}");
        }
    }

    #[test]
    fn other_lines_are_not_results() {
        for l in ["", "running 2 tests", "test result: ok. 2 passed", "test  ... ok", "test a ... maybe", "testing"] {
            assert_eq!(parse_result(l), None, "{l}");
        }
    }

    #[test]
    fn follows_a_failing_run() {
        let text = "     Running unittests src/lib.rs (target/debug/deps/demo-abc)\n\nrunning 2 tests\ntest tests::bad ... FAILED\ntest tests::good ... ok\n\nfailures:\n\n---- tests::bad stdout ----\n\nthread 'tests::bad' panicked at src/lib.rs:9:9:\nassertion failed\n\n\nfailures:\n    tests::bad\n\ntest result: FAILED. 1 passed; 1 failed\n";
        let mut p = TestParser::new();
        let finished: Vec<_> = text.lines().filter_map(|l| p.line(l)).collect();
        assert_eq!(finished.len(), 2);
        let results = p.finish(Path::new("/w"));
        assert_eq!(results[0].fully_qualified_name, "tests::bad");
        assert_eq!((results[0].class_name.as_str(), results[0].method_name.as_str()), ("tests", "bad"));
        assert_eq!(results[0].source, Some(PathBuf::from("target/debug/deps/demo-abc")));
        let msg = results[0].error_message.as_deref().unwrap();
        assert!(msg.starts_with("thread 'tests::bad' panicked") && msg.ends_with("assertion failed"), "{msg}");
        assert_eq!(results[1].error_message, None);
    }
}
