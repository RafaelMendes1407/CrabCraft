//! The long explanation of a rustc error code (`E0308`), from `rustc --explain`.
//!
//! The code comes from diagnostics, which are untrusted: it is checked before it reaches the
//! command line. `rustc --explain` only prints text bundled with the compiler; it compiles
//! nothing and never reads the project, so it is safe on a project the user has not trusted.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Explanations are a few KB; more than this is not rustc's text.
const MAX_OUTPUT: u64 = 1024 * 1024;

/// Why an explanation could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplainError {
    /// The code is not `E` followed by four digits.
    InvalidCode,
    /// `rustc` could not be started.
    NoRustc(String),
    /// `rustc` did not answer in time and was stopped.
    TimedOut,
    /// `rustc` failed for another reason (its message).
    Failed(String),
}

impl std::fmt::Display for ExplainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExplainError::InvalidCode => f.write_str("not a rustc error code"),
            ExplainError::NoRustc(e) => write!(f, "rustc could not be started: {e}"),
            ExplainError::TimedOut => f.write_str("rustc --explain timed out"),
            ExplainError::Failed(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for ExplainError {}

/// The rustc error code in a diagnostic's code: exactly `E` and four ASCII digits, after
/// trimming. Lints (`unused_variables`, `clippy::len_zero`) and anything else give `None`.
pub fn error_code(code: &str) -> Option<&str> {
    let code = code.trim();
    let bytes = code.as_bytes();
    (bytes.len() == 5 && bytes[0] == b'E' && bytes[1..].iter().all(u8::is_ascii_digit)).then_some(code)
}

/// `rustc`, next to `cargo` (where rustup and the standalone installers put it).
pub fn rustc_path(cargo: &crate::Cargo) -> PathBuf {
    cargo.rustc_path()
}

/// How to run `rustc --explain`.
#[derive(Clone, Debug)]
pub struct Explainer {
    pub rustc: PathBuf,
    pub cargo: Option<crate::Cargo>,
    /// The folder whose `rust-toolchain.toml` applies (`None`: the default toolchain).
    pub dir: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
}

impl Explainer {
    pub fn new(rustc: impl Into<PathBuf>) -> Explainer {
        Explainer { rustc: rustc.into(), cargo: None, dir: None, env: Vec::new(), timeout: Duration::from_secs(10) }
    }

    pub fn for_cargo(cargo: &crate::Cargo) -> Self {
        Self { cargo: Some(cargo.clone()), ..Self::new(cargo.rustc_path()) }
    }

    /// The text rustc prints for `code` (Markdown), or `None` when rustc has no explanation for
    /// it. Blocks for up to `timeout`: call it from a background thread.
    pub fn explain(&self, code: &str) -> Result<Option<String>, ExplainError> {
        let code = error_code(code).ok_or(ExplainError::InvalidCode)?;
        let mut cmd = self.cargo.as_ref().map_or_else(|| Command::new(&self.rustc), |cargo| cargo.std_tool_command("rustc"));
        cmd.arg("--explain").arg(code).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if self.cargo.as_ref().is_none_or(|cargo| !cargo.is_criticalup()) {
            cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        }
        // A toolchain file naming a missing toolchain must not start a download.
        cmd.env("RUSTUP_AUTO_INSTALL", "0");
        if let Some(dir) = self.dir.as_deref().filter(|d| d.is_dir()) {
            cmd.current_dir(dir);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let neutral = if self.dir.is_none() { Some(tempfile::tempdir().map_err(|e| ExplainError::NoRustc(e.to_string()))?) } else { None };
        if let Some(dir) = &neutral {
            cmd.current_dir(dir.path()).env_remove("RUSTUP_TOOLCHAIN");
        }
        let mut child = cmd.spawn().map_err(|e| ExplainError::NoRustc(e.to_string()))?;
        let _group = ExplainGroup(child.id());
        let stdout = child.stdout.take().map(read_limited);
        let stderr = child.stderr.take().map(read_limited);
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ExplainError::TimedOut);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(ExplainError::Failed(e.to_string())),
            }
        };
        let take = |h: Option<std::thread::JoinHandle<Vec<u8>>>| h.and_then(|h| h.join().ok()).unwrap_or_default();
        let out = String::from_utf8_lossy(&take(stdout)).into_owned();
        let err = String::from_utf8_lossy(&take(stderr)).trim().to_string();
        interpret(status.success(), &out, &err)
    }
}

/// Reads a pipe on its own thread (so a full pipe never blocks rustc), up to [`MAX_OUTPUT`].
fn read_limited(pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.take(MAX_OUTPUT).read_to_end(&mut buf);
        buf
    })
}

/// What a finished `rustc --explain` means.
pub fn interpret(success: bool, stdout: &str, stderr: &str) -> Result<Option<String>, ExplainError> {
    if success {
        let text = stdout.trim();
        return Ok((!text.is_empty()).then(|| text.to_string()));
    }
    // `error: E9999 is not a valid error code`, `error: no extended information for E0000`.
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("not a valid error code") || lower.contains("no extended information") {
        return Ok(None);
    }
    Err(ExplainError::Failed(if stderr.is_empty() { "rustc --explain failed".to_string() } else { stderr.lines().take(5).collect::<Vec<_>>().join("\n") }))
}

/// `rustc` for `--explain`: next to the `cargo` found on this machine.
pub fn locate_rustc() -> Option<PathBuf> {
    crate::Cargo::locate().map(|c| rustc_path(&c)).filter(|p| p.is_file())
}

struct ExplainGroup(u32);
impl Drop for ExplainGroup {
    fn drop(&mut self) {
        rc_dotnet::kill_tree(self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_e_and_four_digits_are_error_codes() {
        assert_eq!(error_code("E0308"), Some("E0308"));
        assert_eq!(error_code(" E0502 "), Some("E0502"));
        for bad in ["", "E", "E030", "E03080", "e0308", "E03O8", "unused_variables", "clippy::len_zero", "E0308; rm -rf /", "--help", "E０３０８", "E-308"]
        {
            assert_eq!(error_code(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn reads_what_rustc_answers() {
        assert_eq!(interpret(true, "Expected type…\n\n", ""), Ok(Some("Expected type…".into())));
        assert_eq!(interpret(true, "  \n", ""), Ok(None));
        assert_eq!(interpret(false, "", "error: E9999 is not a valid error code"), Ok(None));
        assert_eq!(interpret(false, "", "error: no extended information for E0000"), Ok(None));
        assert!(matches!(interpret(false, "", "error: toolchain 'x' is not installed"), Err(ExplainError::Failed(m)) if m.contains("not installed")));
        assert!(matches!(interpret(false, "", ""), Err(ExplainError::Failed(_))));
    }

    #[test]
    fn an_invalid_code_never_reaches_rustc() {
        let e = Explainer::new("/does/not/exist/rustc");
        assert_eq!(e.explain("--version"), Err(ExplainError::InvalidCode));
        assert!(matches!(e.explain("E0308"), Err(ExplainError::NoRustc(_))));
    }

    #[test]
    fn a_rustc_that_hangs_is_stopped() {
        if cfg!(windows) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("rustc");
        std::fs::write(&fake, "#!/bin/sh\nsleep 30\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut e = Explainer::new(&fake);
        e.timeout = Duration::from_millis(300);
        let started = Instant::now();
        assert_eq!(e.explain("E0308"), Err(ExplainError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn explanations_without_a_trusted_project_use_a_neutral_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("rustc");
        std::fs::write(&program, "#!/bin/sh\npwd\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cwd = Explainer::new(program).explain("E0308").unwrap().unwrap();
        assert_ne!(PathBuf::from(&cwd), std::env::current_dir().unwrap());
        assert!(PathBuf::from(cwd).starts_with(std::env::temp_dir()));
    }

    #[test]
    fn the_real_rustc_explains_mismatched_types() {
        let rustc = locate_rustc().expect("rustc is installed next to cargo");
        let e = Explainer { dir: Some(std::env::current_dir().unwrap()), ..Explainer::new(rustc) };
        let text = e.explain("E0308").unwrap().expect("E0308 has an explanation");
        // E0308 is the "mismatched types" error.
        assert!(text.contains("Expected type did not match the received type"), "{text}");
        assert_eq!(e.explain("E9999"), Ok(None));
    }
}
