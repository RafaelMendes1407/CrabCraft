//! Runs against the real toolchain with a tiny generated project.

use rc_cargo::{Cargo, RunTarget, TargetKind, load_workspace, start_build, start_run, start_tests};
use rc_core::inbox::Inbox;
use rc_core::Waker;
use rc_dotnet::{BuildAction, BuildEvent, RunEvent, Severity, TestEvent, TestOutcome, TestRequest};
use std::path::Path;
use std::time::{Duration, Instant};

fn project(dir: &Path, main: &str, lib: &str) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"crabby\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    std::fs::write(dir.join("src/main.rs"), main).unwrap();
    std::fs::write(dir.join("src/lib.rs"), lib).unwrap();
}

fn drain<T, F: FnMut(&T) -> bool>(inbox: &Inbox<T>, mut last: F) -> Vec<T> {
    let started = Instant::now();
    let mut out = Vec::new();
    loop {
        let batch = inbox.drain();
        let done = batch.iter().any(&mut last);
        out.extend(batch);
        if done {
            return out;
        }
        assert!(started.elapsed() < Duration::from_secs(120), "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn cargo() -> Cargo {
    Cargo::locate().expect("cargo is installed")
}

#[test]
fn loads_a_real_workspace_without_building_it() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), "fn main() {}\n", "");
    // A build script that would leave a mark if opening the project ran it.
    std::fs::write(dir.path().join("build.rs"), "fn main() { std::fs::write(\"RAN\", \"\").unwrap(); }\n").unwrap();
    let w = load_workspace(&cargo(), &dir.path().join("Cargo.toml")).unwrap();
    assert_eq!(w.packages[0].name, "crabby");
    assert!(w.packages[0].targets.iter().any(|t| t.kind == TargetKind::Bin && t.name == "crabby"));
    assert!(w.packages[0].targets.iter().any(|t| t.kind == TargetKind::Lib));
    assert!(!dir.path().join("RAN").exists());
    assert!(!dir.path().join("target").exists());
}

#[test]
fn a_broken_manifest_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "not toml [").unwrap();
    assert!(load_workspace(&cargo(), &dir.path().join("Cargo.toml")).is_err());
}

#[test]
fn build_reports_the_position_of_an_error() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), "fn main() {\n    let x: u32 = \"text\";\n}\n", "");
    let inbox = Inbox::new(Waker::noop());
    let _op = start_build(&cargo(), &dir.path().join("Cargo.toml"), BuildAction::Build, inbox.sender());
    let events = drain(&inbox, |e| matches!(e, BuildEvent::Finished(_) | BuildEvent::Failed(_)));
    let Some(BuildEvent::Finished(r)) = events.last() else { panic!("{events:?}") };
    assert!(!r.success);
    let d = r.errors().next().expect("an error");
    assert_eq!(d.code, "E0308");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.file.as_deref(), Some(dir.path().join("src/main.rs").as_path()));
    assert_eq!((d.line, d.column), (Some(2), Some(18)));
    assert!(events.iter().any(|e| matches!(e, BuildEvent::Output(l) if l.text.contains("E0308"))));
}

#[test]
fn runs_the_binary_and_reports_its_output_and_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), "fn main() {\n    println!(\"hello {}\", std::env::args().nth(1).unwrap());\n    std::process::exit(7);\n}\n", "");
    let inbox = Inbox::new(Waker::noop());
    let target = RunTarget { test: None, manifest: dir.path().join("Cargo.toml"), package: "crabby".into(), bin: "crabby".into(), args: vec!["crab".into()], env: Vec::new(), working_dir: None };
    let handle = start_run(&cargo(), target, inbox.sender());
    let events = drain(&inbox, |e| matches!(e, RunEvent::Exited { .. } | RunEvent::Failed(_)));
    assert!(handle.pid().is_some());
    assert!(events.iter().any(|e| matches!(e, RunEvent::Started { plan, .. } if plan.program.ends_with("crabby"))), "{events:?}");
    assert!(events.iter().any(|e| matches!(e, RunEvent::Output(l) if l.text.contains("hello crab"))), "{events:?}");
    assert!(matches!(events.last(), Some(RunEvent::Exited { code: Some(7), cancelled: false, .. })), "{events:?}");
}

#[test]
fn a_build_error_stops_the_run() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), "fn main() { nope }\n", "");
    let inbox = Inbox::new(Waker::noop());
    let target = RunTarget { test: None, manifest: dir.path().join("Cargo.toml"), package: "crabby".into(), bin: "crabby".into(), args: Vec::new(), env: Vec::new(), working_dir: None };
    let _handle = start_run(&cargo(), target, inbox.sender());
    let events = drain(&inbox, |e| matches!(e, RunEvent::Exited { .. } | RunEvent::Failed(_)));
    assert!(matches!(events.last(), Some(RunEvent::Failed(_))), "{events:?}");
}

#[test]
fn tests_report_each_outcome_with_the_failure_output() {
    let dir = tempfile::tempdir().unwrap();
    let lib = "/// ```\n/// assert_eq!(crabby::two(), 2);\n/// ```\npub fn two() -> u32 { 2 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn good() { assert_eq!(super::two(), 2); }\n    #[test]\n    fn bad() { assert_eq!(super::two(), 3, \"not three\"); }\n    #[test]\n    #[ignore]\n    fn skipped() {}\n}\n";
    project(dir.path(), "fn main() {}\n", lib);
    let inbox = Inbox::new(Waker::noop());
    let _op = start_tests(&cargo(), TestRequest::new(dir.path().join("Cargo.toml")), inbox.sender());
    let events = drain(&inbox, |e| matches!(e, TestEvent::Finished(_) | TestEvent::Failed(_)));
    let Some(TestEvent::Finished(r)) = events.last() else { panic!("{events:?}") };
    assert!(!r.success);
    assert_eq!((r.failed, r.skipped), (1, 1), "{:?}", r.results);
    assert_eq!(r.passed, 2, "the unit test and the doctest: {:?}", r.results);
    let bad = r.results.iter().find(|t| t.method_name == "bad").unwrap();
    assert_eq!(bad.outcome, TestOutcome::Failed);
    assert!(bad.error_message.as_deref().unwrap_or("").contains("not three"), "{bad:?}");
    assert!(r.results.iter().any(|t| t.fully_qualified_name.contains("src/lib.rs") && t.outcome == TestOutcome::Passed));
}

#[test]
fn tests_that_do_not_compile_report_a_failed_build() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), "fn main() {}\n", "#[test]\nfn t() { nope }\n");
    let inbox = Inbox::new(Waker::noop());
    let _op = start_tests(&cargo(), TestRequest::new(dir.path().join("Cargo.toml")), inbox.sender());
    let events = drain(&inbox, |e| matches!(e, TestEvent::Finished(_) | TestEvent::Failed(_)));
    assert!(events.iter().any(|e| matches!(e, TestEvent::Build(BuildEvent::Finished(r)) if !r.success && r.errors > 0)), "{events:?}");
}
