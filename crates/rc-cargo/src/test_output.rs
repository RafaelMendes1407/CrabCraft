//! Parses the human output of libtest (its JSON format needs a nightly compiler) and of
//! cargo-nextest.

use rc_dotnet::trx::StackFrame;
use rc_dotnet::{TestOutcome, TestResult};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Follows a `cargo test` or `cargo nextest run` run line by line.
#[derive(Default)]
pub struct TestParser {
    suite: Option<PathBuf>,
    results: Vec<TestResult>,
    /// Output of failed tests, by test name (nextest: their stderr).
    failures: BTreeMap<String, String>,
    /// nextest: the stdout of failed tests, by test name.
    stdout: BTreeMap<String, String>,
    section: Option<String>,
    nextest: Option<Nextest>,
}

/// Where a nextest run is.
#[derive(Default)]
struct Nextest {
    /// The failed test whose output follows, and whether it is its stdout.
    failed: Option<String>,
    in_stdout: bool,
    /// After the summary, which repeats the failures.
    done: bool,
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

    /// For the output of `cargo nextest run`.
    pub fn nextest() -> Self {
        TestParser { nextest: Some(Nextest::default()), ..Self::default() }
    }

    /// Feeds one line of output.
    pub fn line(&mut self, line: &str) -> Option<Finished> {
        if self.nextest.is_some() {
            return self.nextest_line(line);
        }
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

    fn nextest_line(&mut self, line: &str) -> Option<Finished> {
        let text = line.trim_end();
        let trimmed = text.trim_start();
        let state = self.nextest.as_mut()?;
        if state.done {
            return None;
        }
        if trimmed.starts_with("Summary [") {
            state.done = true;
            return None;
        }
        if let Some((binary, name, outcome)) = parse_nextest_status(trimmed) {
            state.failed = (outcome == TestOutcome::Failed).then(|| name.clone());
            state.in_stdout = false;
            // A retried test reports again: the last report counts.
            self.results.retain(|r| r.fully_qualified_name != name);
            self.results.push(result(&name, outcome, Some(PathBuf::from(binary))));
            return Some(Finished { name, outcome });
        }
        let name = state.failed.clone()?;
        // The rule that closes a failure's output.
        if !trimmed.is_empty() && trimmed.chars().all(|c| c == '─') {
            state.failed = None;
            return None;
        }
        // `  stdout ───` (older versions: `--- STDOUT:   demo a::b ---`).
        if trimmed.ends_with('─') || trimmed.ends_with("---") {
            let heading = trimmed.trim_start_matches(['-', ' ']).split([' ', ':']).next().unwrap_or("").to_ascii_lowercase();
            match heading.as_str() {
                "stdout" => {
                    state.in_stdout = true;
                    return None;
                }
                "stderr" | "output" => {
                    state.in_stdout = false;
                    return None;
                }
                _ => {}
            }
        }
        let content = text.strip_prefix("    ").unwrap_or(text);
        let map = if state.in_stdout { &mut self.stdout } else { &mut self.failures };
        let entry = map.entry(name).or_default();
        entry.push_str(content);
        entry.push('\n');
        None
    }

    /// Every result, with the output of the failed ones and where they panicked. Relative paths
    /// in panic messages are resolved against `root`, the folder Cargo ran in.
    pub fn finish(mut self, root: &Path) -> Vec<TestResult> {
        for r in &mut self.results {
            if r.outcome == TestOutcome::Failed {
                let text = self.failures.get(&r.fully_qualified_name).map(|t| t.trim().to_string()).unwrap_or_default();
                r.stack_trace = text.lines().filter_map(|l| panic_frame(l, root)).collect();
                r.error_message = (!text.is_empty()).then_some(text);
                r.stdout = self.stdout.get(&r.fully_qualified_name).map(|t| t.trim().to_string()).unwrap_or_default();
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

/// A status line of nextest: `PASS [   0.019s] (1/3) demo::it it_works`, `SKIP [         ] (───)
/// demo a::b`, `TRY 2 FAIL [   0.1s] demo a::b`. Returns the binary, the test and its outcome.
pub fn parse_nextest_status(line: &str) -> Option<(String, String, TestOutcome)> {
    let mut rest = line;
    if let Some(after) = rest.strip_prefix("TRY ") {
        rest = after.split_once(' ')?.1;
    }
    let (status, after) = rest.split_once(' ')?;
    let outcome = match status {
        "PASS" | "LEAK" | "FLAKY" => TestOutcome::Passed,
        "SKIP" => TestOutcome::Skipped,
        "FAIL" | "LEAK-FAIL" | "TIMEOUT" | "ABORT" => TestOutcome::Failed,
        s if s.len() > 3 && s.starts_with("SIG") && s.chars().all(|c| c.is_ascii_uppercase()) => TestOutcome::Failed,
        _ => return None,
    };
    let after = after.trim_start().strip_prefix('[')?;
    let (_, after) = after.split_once(']')?;
    let mut after = after.trim_start();
    if after.starts_with('(') {
        after = after.split_once(')')?.1.trim_start();
    }
    let (binary, name) = after.split_once(' ')?;
    let name = name.trim();
    (!binary.is_empty() && !name.is_empty() && !name.contains(' ')).then(|| (binary.to_string(), name.to_string(), outcome))
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
    fn reads_nextest_status_lines() {
        let p = parse_nextest_status;
        assert_eq!(p("PASS [   0.019s] (1/3) demo::it it_works"), Some(("demo::it".into(), "it_works".into(), TestOutcome::Passed)));
        assert_eq!(p("FAIL [   0.024s] (3/3) demo checks::bad"), Some(("demo".into(), "checks::bad".into(), TestOutcome::Failed)));
        assert_eq!(p("SKIP [         ] (───) demo checks::skipped").map(|t| t.2), Some(TestOutcome::Skipped));
        assert_eq!(p("PASS [   0.004s] demo::bin/demo tests::x").map(|t| t.1), Some("tests::x".into()));
        assert_eq!(p("TRY 2 FAIL [   0.1s] (1/1) demo a").map(|t| t.2), Some(TestOutcome::Failed));
        assert_eq!(p("SIGSEGV [   0.1s] (1/1) demo a").map(|t| t.2), Some(TestOutcome::Failed));
        for bad in ["", "SLOW [> 60.000s] demo a", "Starting 3 tests across 2 binaries", "PASS", "PASS [0.1s]", "Summary [   0.025s] 3 tests run: 2 passed"] {
            assert_eq!(p(bad), None, "{bad}");
        }
    }

    #[test]
    fn follows_a_failing_nextest_run() {
        // As cargo-nextest 0.9.150 prints it (stderr).
        let text = "────────────\n Nextest run ID 2225 with nextest profile: default\n    Starting 3 tests across 2 binaries (1 test skipped)\n        PASS [   0.019s] (1/3) demo::it it_works\n        PASS [   0.021s] (2/3) demo checks::good\n        FAIL [   0.024s] (3/3) demo checks::bad\n  stdout ───\n\n    running 1 test\n    hello out\n    test checks::bad ... FAILED\n\n  stderr ───\n\n    thread 'checks::bad' (22348) panicked at src/lib.rs:15:9:\n    assertion `left == right` failed\n      left: 1\n     right: 2\n\n────────────\n     Summary [   0.025s] 3 tests run: 2 passed, 1 failed, 1 skipped\n        FAIL [   0.024s] (3/3) demo checks::bad\nerror: test run failed\n";
        let mut p = TestParser::nextest();
        let finished: Vec<_> = text.lines().filter_map(|l| p.line(l)).collect();
        assert_eq!(finished.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["it_works", "checks::good", "checks::bad"]);
        let results = p.finish(Path::new("/w"));
        assert_eq!(results.len(), 3);
        let bad = &results[2];
        assert_eq!((bad.outcome, bad.source.clone()), (TestOutcome::Failed, Some(PathBuf::from("demo"))));
        let msg = bad.error_message.as_deref().unwrap();
        assert!(msg.starts_with("thread 'checks::bad'") && msg.ends_with("right: 2"), "{msg}");
        assert_eq!(bad.stack_trace.first().and_then(|f| f.line), Some(15));
        assert!(bad.stdout.contains("hello out") && !msg.contains("hello out"));
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
