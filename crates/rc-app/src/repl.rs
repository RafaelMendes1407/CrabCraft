//! Evcxr is hosted by the native terminal. Starting a PTY never blocks the UI.
use rc_core::{Inbox, Waker};
use rc_term::{SpawnSpec, TermSession};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug)]
pub enum StartError {
    Thread(std::io::Error),
    Process(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Thread(error) => error.fmt(f),
            Self::Process(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StartError {}

/// Dropping a pending startup prevents its process from being retained (including when the
/// user closes the project while startup is still in progress).
pub struct Starting {
    inbox: Inbox<Result<TermSession, StartError>>,
    cancelled: Arc<AtomicBool>,
}

impl Starting {
    pub fn new(spec: SpawnSpec, waker: Waker) -> Result<Self, StartError> {
        let inbox = Inbox::new(waker.clone());
        let out = inbox.sender();
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = cancelled.clone();
        std::thread::Builder::new()
            .name("rc-repl-start".into())
            .spawn(move || {
                if token.load(Ordering::Acquire) {
                    return;
                }
                let result = TermSession::spawn(&spec, waker).map_err(|error| StartError::Process(error.to_string()));
                if !token.load(Ordering::Acquire) {
                    out.send(result);
                }
            })
            .map_err(StartError::Thread)?;
        Ok(Self { inbox, cancelled })
    }

    pub fn poll(&self) -> Option<Result<TermSession, StartError>> {
        self.inbox.drain().into_iter().next()
    }
}

impl Drop for Starting {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(starting: &Starting) -> Result<TermSession, StartError> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = starting.poll() {
                return result;
            }
            assert!(Instant::now() < deadline, "PTY startup timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn unavailable_repl_is_a_typed_error() {
        let spec = SpawnSpec::command("/crabcraft/nonexistent/evcxr", std::iter::empty::<String>(), None);
        let starting = Starting::new(spec, Waker::noop()).unwrap();
        assert!(matches!(wait(&starting), Err(StartError::Process(_))));
    }

    #[test]
    fn repl_requires_a_trusted_cargo_project() {
        use crate::app::{CrabCraftApp, WorkspaceState};
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
        let mut app = CrabCraftApp::with_settings(rc_core::Settings::default(), Waker::noop(), None);
        app.open_rust_repl();
        assert!(app.repl_starting.is_none());
        app.workspace = Some(WorkspaceState { ws: rc_core::Workspace::open(root.path()).unwrap() });
        app.open_rust_repl();
        assert!(app.repl_starting.is_none());
        assert!(!app.can_open_rust_repl());
        let canonical = root.path().canonicalize().unwrap();
        app.settings.trusted_projects.push(canonical);
        assert!(app.can_open_rust_repl());
    }

    /// `CRABCRAFT_TEST_EVCXR=/path/to/evcxr cargo test -p rc-app repl::tests::real -- --ignored`
    #[test]
    #[ignore = "Requires evcxr and rust-src; compiles a real Rust project in the REPL"]
    fn real_repl_evaluates_rust_and_loads_a_local_cargo_dependency() {
        let program = std::env::var("CRABCRAFT_TEST_EVCXR").unwrap_or_else(|_| "evcxr".into());
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[package]\nname = \"repl_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n")
            .unwrap();
        std::fs::write(project.path().join("src/lib.rs"), "pub fn answer() -> i32 { 42 }\n").unwrap();
        let mut spec = SpawnSpec::rust_repl(Some(project.path().to_path_buf()))
            .size(120, 40)
            .env("CARGO_NET_OFFLINE", "true")
            .env("XDG_CONFIG_HOME", project.path().join("config").to_string_lossy());
        if let rc_term::Program::Command { program: executable, .. } = &mut spec.program {
            *executable = program;
        }
        let starting = Starting::new(spec, Waker::noop()).unwrap();
        let session = wait(&starting).unwrap();
        let timeout = Duration::from_secs(20);
        assert!(session.wait_for_text(">>", timeout), "{:?}", session.screen_lines());
        session.write(b"let crabcraft_value = 6 * 7;\r:vars\r".to_vec());
        assert!(session.wait_for_text("crabcraft_value: i32", timeout), "{:?}", session.screen_lines());
        // A local Cargo library is a real dependency, resolved without publishing or network.
        let path = serde_json::to_string(&project.path().to_string_lossy()).unwrap();
        session.write(format!(":dep repl_fixture = {{ path = {path} }}\rprintln!(\"fixture answer: {{}}\", repl_fixture::answer());\r").into_bytes());
        assert!(session.wait_until(timeout, |state| (0..40).any(|row| state.row_text(row).ends_with("fixture answer: 42"))), "{:?}", session.screen_lines());
        session.write(b":quit\r".to_vec());
        assert!(session.wait_until(timeout, |state| state.exit().is_some()));
        assert!(session.exit_status().unwrap().success());
    }
}
