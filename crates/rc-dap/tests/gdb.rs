//! A real Rust program under GDB: breakpoints, variables with the Rust pretty printers, stepping,
//! expression evaluation, program output and exit.

use rc_cargo::{Cargo, RunTarget};
use rc_core::Waker;
use rc_dap::{BreakpointSpec, Breakpoints, DebugEvent, DebugOptions, DebugSession, DebugTarget, OutputCategory, SessionState};
use std::path::Path;
use std::time::{Duration, Instant};

const MAIN: &str = "fn add(a: u32, b: u32) -> u32 {\n    let sum = a + b;\n    sum\n}\n\nfn main() {\n    let items = vec![10, 20, 30];\n    let name = String::from(\"crab\");\n    println!(\"hello {name}\");\n    let total = add(items[0], items[1]);\n    println!(\"total {total}\");\n}\n";

fn project(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"crabby\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    std::fs::write(dir.join("src/main.rs"), MAIN).unwrap();
}

fn line_of(needle: &str) -> u32 {
    MAIN.lines().position(|l| l.contains(needle)).unwrap() as u32 + 1
}

struct Run {
    s: DebugSession,
    events: Vec<DebugEvent>,
}

impl Run {
    fn until(&mut self, what: &str, mut done: impl FnMut(&mut DebugSession, &[DebugEvent]) -> bool) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(90) {
            for e in self.s.pump() {
                if let DebugEvent::Failed(f) = &e {
                    panic!("session failed: {f}\nevents: {:#?}", self.events);
                }
                self.events.push(e);
            }
            if done(&mut self.s, &self.events) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}: state {:?}\nevents: {:#?}", self.s.state(), self.events);
    }

    /// The variables of the top frame, as `name = value`.
    fn locals(&mut self) -> Vec<(String, String)> {
        self.until("threads", |s, _| !s.threads().is_empty());
        let thread = self.s.threads()[0].id;
        self.s.request_frames(thread);
        self.until("frames", |s, _| s.frames(thread).is_some_and(|f| !f.frames.is_empty()));
        let frame = self.s.frames(thread).unwrap().frames[0].id;
        self.s.request_scopes(frame);
        self.until("scopes", |s, _| s.scopes(frame).is_some_and(|x| !x.is_empty()));
        let references: Vec<i64> = self.s.scopes(frame).unwrap().iter().map(|x| x.variables_reference).collect();
        let mut out = Vec::new();
        for r in references {
            self.s.request_variables(r);
            self.until("variables", |s, _| s.variables(r).is_some());
            out.extend(self.s.variables(r).unwrap().iter().map(|v| (v.name.clone(), v.value.clone())));
        }
        out
    }
}

#[test]
fn debugs_a_rust_program() {
    let Some(adapter) = rc_dap::gdb::locate() else {
        eprintln!("gdb is not installed: skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    let root = dir.path().canonicalize().unwrap();
    let main = root.join("src/main.rs");
    let target = RunTarget {
        before: Vec::new(),
        test: None,
        manifest: root.join("Cargo.toml"),
        package: "crabby".into(),
        bin: "crabby".into(),
        args: Vec::new(),
        env: Vec::new(),
        working_dir: None,
        options: Default::default(),
    };
    let mut bps = Breakpoints::default();
    bps.set(BreakpointSpec::new(main.clone(), line_of("let total")));
    bps.set(BreakpointSpec::new(main.clone(), line_of("let sum")));
    let s = DebugSession::start(
        adapter,
        DebugTarget::Cargo { cargo: Cargo::locate().unwrap(), target: Box::new(target) },
        DebugOptions::default(),
        bps,
        Waker::noop(),
    );
    let mut run = Run { s, events: Vec::new() };

    // Stops at the breakpoint, past the first println.
    run.until("the breakpoint", |s, _| s.state().is_stopped());
    let locals = run.locals();
    let value = |n: &str| locals.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()).unwrap_or_else(|| panic!("no {n} in {locals:?}"));
    assert!(value("name").contains("crab"), "{locals:?}");
    assert!(!locals.iter().any(|(k, _)| k == "rax"), "registers stay out of the variables: {locals:?}");
    assert!(value("items").contains("10") || value("items").contains("size=3") || value("items").contains("len"), "pretty printed Vec: {locals:?}");

    // A second breakpoint inside the function shows its parameters.
    run.s.continue_();
    let before = run.events.iter().filter(|e| matches!(e, DebugEvent::Continued)).count();
    run.until("the second breakpoint", |s, ev| s.state().is_stopped() && ev.iter().filter(|e| matches!(e, DebugEvent::Continued)).count() >= before);
    run.until("the stop in add", |s, _| matches!(s.state(), SessionState::Stopped(_)));
    let locals = run.locals();
    assert!(locals.iter().any(|(k, v)| k == "a" && v == "10") && locals.iter().any(|(k, v)| k == "b" && v == "20"), "{locals:?}");

    // Stepping over a line stays in the function.
    run.s.step_over();
    run.until("the step", |s, _| s.state().is_stopped());
    let locals = run.locals();
    assert!(locals.iter().any(|(k, v)| k == "a" && v == "10"), "{locals:?}");
    let thread = run.s.threads()[0].id;
    assert_eq!(run.s.frames(thread).unwrap().frames[0].line, 4, "the step reached the end of add");

    // Runs to the end with its output and exit code.
    run.s.continue_();
    run.until("the end", |s, _| s.state().is_ended());
    let output: String = run
        .events
        .iter()
        .filter_map(|e| if let DebugEvent::Output { category: OutputCategory::Stdout, text } = e { Some(text.as_str()) } else { None })
        .collect();
    assert!(output.contains("hello crab") && output.contains("total 30"), "output {output:?}");
    assert!(matches!(run.s.state(), SessionState::Exited(0)), "{:?}", run.s.state());
}
