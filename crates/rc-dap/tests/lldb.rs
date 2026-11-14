//! Real Rust build, breakpoint, locals, step, output and exit through LLVM DAP.
use rc_core::Waker;
use rc_dap::{BreakpointSpec, Breakpoints, DebugEvent, DebugOptions, DebugSession, DebugTarget, SessionState};
use std::time::{Duration, Instant};

fn wait(session: &mut DebugSession, done: impl Fn(&DebugSession) -> bool) -> Vec<DebugEvent> {
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut events = Vec::new();
    loop {
        for event in session.pump() {
            assert!(!matches!(&event, DebugEvent::Failed(_)), "{event:?}, {events:?}");
            events.push(event);
        }
        if done(session) {
            return events;
        }
        assert!(Instant::now() < deadline, "{:?}, {events:?}", session.state());
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn llvm_dap_debugs_a_real_rust_project() {
    let configured = std::env::var_os("CRABCRAFT_TEST_LLDB_DAP").map(std::path::PathBuf::from);
    let Some(adapter) = rc_dap::lldb::locate(configured.as_deref()) else {
        assert!(configured.is_none(), "explicit test adapter must exist");
        eprintln!("LLDB DAP not installed; set CRABCRAFT_TEST_LLDB_DAP to run real integration");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname='lldb-audit'\nversion='0.1.0'\nedition='2024'\n[workspace]\n").unwrap();
    let source = dir.path().join("src/main.rs");
    std::fs::write(&source, "fn main() {\n    let answer: u32 = 42;\n    let next = answer + 1;\n    println!(\"lldb: {next}\");\n}\n").unwrap();
    let target = rc_cargo::RunTarget {
        before: vec![],
        test: None,
        manifest: dir.path().join("Cargo.toml"),
        package: "lldb-audit".into(),
        bin: "lldb-audit".into(),
        args: vec![],
        env: vec![],
        working_dir: None,
        options: Default::default(),
    };
    let mut bps = Breakpoints::default();
    bps.set(BreakpointSpec::new(&source, 3));
    let mut session = DebugSession::start(
        adapter,
        DebugTarget::Cargo { cargo: rc_cargo::Cargo::locate().unwrap(), target: Box::new(target) },
        DebugOptions::default(),
        bps,
        Waker::noop(),
    );
    wait(&mut session, |s| s.state().is_stopped());
    wait(&mut session, |s| !s.threads().is_empty());
    let thread = session.threads()[0].id;
    session.request_frames(thread);
    wait(&mut session, |s| s.frames(thread).is_some_and(|f| !f.frames.is_empty()));
    let frame = session.frames(thread).unwrap().frames[0].id;
    assert_eq!(session.frames(thread).unwrap().frames[0].line, 3);
    session.request_scopes(frame);
    wait(&mut session, |s| s.scopes(frame).is_some_and(|v| !v.is_empty()));
    let references: Vec<_> = session.scopes(frame).unwrap().iter().map(|s| s.variables_reference).collect();
    let mut answer = false;
    for reference in references {
        session.request_variables(reference);
        wait(&mut session, |s| s.variables(reference).is_some());
        answer |= session.variables(reference).unwrap().iter().any(|v| v.name == "answer" && v.value == "42");
    }
    assert!(answer, "the Rust local must be inspectable");
    session.step_over();
    wait(&mut session, |s| s.state().is_stopped());
    session.continue_();
    let events = wait(&mut session, |s| s.state().is_ended());
    assert!(matches!(session.state(), SessionState::Exited(0)), "{:?}", session.state());
    assert!(events.iter().any(|event| matches!(event, DebugEvent::Output { text, .. } if text.contains("lldb: 43"))), "{events:?}");
}
