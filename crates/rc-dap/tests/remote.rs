//! A real Rust executable behind gdbserver, including non-destructive detach.
use rc_core::Waker;
use rc_dap::{BreakpointSpec, Breakpoints, DebugEvent, DebugOptions, DebugSession, DebugTarget};
use std::{io::{BufRead, BufReader}, process::{Child, Command, Stdio}, time::{Duration, Instant}};

struct Server(Child);
impl Drop for Server { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
fn wait(session: &mut DebugSession, done: impl Fn(&DebugSession) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut events = Vec::new();
    loop {
        for event in session.pump() {
            assert!(!matches!(&event, DebugEvent::Failed(_)), "{event:?}; {events:?}");
            events.push(event);
        }
        if done(session) { return; }
        assert!(Instant::now() < deadline, "{:?}; {events:?}", session.state());
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn remote_rust_breakpoints_locals_and_detach_preserve_the_remote_process() {
    let Some(server_path) = std::env::var_os("CRABCRAFT_TEST_GDBSERVER") else {
        eprintln!("set CRABCRAFT_TEST_GDBSERVER to run real remote Rust integration"); return;
    };
    let adapter = rc_dap::gdb::locate().expect("GDB required with explicit gdbserver fixture");
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join("src")).unwrap();
    std::fs::write(project.path().join("Cargo.toml"), "[package]\nname='remote-audit'\nversion='0.1.0'\nedition='2024'\n[workspace]\n").unwrap();
    let source = project.path().join("src/main.rs");
    std::fs::write(&source, "fn main() {\n    let answer: u32 = 42;\n    let next = answer + 1;\n    std::thread::sleep(std::time::Duration::from_millis(300));\n    std::fs::write(std::env::args().nth(1).unwrap(), next.to_string()).unwrap();\n}\n").unwrap();
    let build = Command::new("cargo").args(["build", "--offline", "--manifest-path"]).arg(project.path().join("Cargo.toml")).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let program = project.path().join("target/debug/remote-audit");
    for (detach, running) in [(false, false), (true, false), (true, true)] {
        let marker = project.path().join(if running { "detached-running" } else if detach { "detached" } else { "continued" });
        let child = Command::new(&server_path).args(["--once", "127.0.0.1:0"]).arg(&program).arg(&marker).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
        let mut server = Server(child);
        let mut lines = BufReader::new(server.0.stderr.take().unwrap());
        let mut port = None;
        for _ in 0..10 {
            let mut line = String::new();
            assert_ne!(lines.read_line(&mut line).unwrap(), 0, "gdbserver exited before listening");
            if let Some(value) = line.trim().strip_prefix("Listening on port ") { port = Some(value.parse::<u16>().unwrap()); break; }
        }
        let remote = rc_dap::remote::RemoteTarget::new("127.0.0.1", port.unwrap(), program.clone()).unwrap();
        let mut bps = Breakpoints::default(); bps.set(BreakpointSpec::new(&source, 3));
        let mut session = DebugSession::start(adapter.clone(), DebugTarget::Remote(remote), DebugOptions::default(), bps, Waker::noop());
        loop {
            wait(&mut session, |s| s.state().is_stopped());
            wait(&mut session, |s| !s.threads().is_empty());
            let thread = session.threads()[0].id; session.request_frames(thread);
            wait(&mut session, |s| s.frames(thread).is_some_and(|f| !f.frames.is_empty()));
            if session.frames(thread).unwrap().frames[0].line == 3 { break; }
            session.continue_();
        }
        let thread = session.threads()[0].id;
        let frame = session.frames(thread).unwrap().frames[0].id;
        session.request_scopes(frame); wait(&mut session, |s| s.scopes(frame).is_some_and(|v| !v.is_empty()));
        let references: Vec<_> = session.scopes(frame).unwrap().iter().map(|s| s.variables_reference).collect();
        let mut found = false;
        for reference in references {
            session.request_variables(reference); wait(&mut session, |s| s.variables(reference).is_some());
            found |= session.variables(reference).unwrap().iter().any(|v| v.name == "answer" && v.value == "42");
        }
        assert!(found);
        if detach { if running { session.continue_(); } session.stop(); } else { session.continue_(); wait(&mut session, |s| s.state().is_ended()); }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() { assert!(Instant::now() < deadline, "remote process did not survive detach/continue: detach={detach}, state={:?}, server={:?}", session.state(), server.0.try_wait().unwrap()); std::thread::sleep(Duration::from_millis(5)); }
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "43");
    }
}
