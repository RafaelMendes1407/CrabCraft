//! Command line: `crabcraft [--line N] [--column N] [path[:line[:column]]]...`.
//!
//! Paths are made absolute against the caller's folder, since they may be handed to an instance
//! that is already running somewhere else.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rc_kit::i18n::{tr, trf};

/// A file or folder to open, with an optional 1-based caret position.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpenRequest {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl OpenRequest {
    pub fn path(path: impl Into<PathBuf>) -> OpenRequest {
        OpenRequest { path: path.into(), line: None, column: None }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Open(Vec<OpenRequest>),
    Tool(ToolRequest),
    Http(crate::http_cli::HttpCommand),
    Compare(crate::compare_cli::CompareRequest),
    Help,
    Version,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CliError {
    UnknownOption(String),
    MissingNumber(String),
    InvalidNumber { option: String, value: String },
    MissingFile,
    MultipleProjects,
    UntrustedProject,
    InvalidManifest,
    HttpFile,
    ComparisonPaths { merge: bool },
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::UnknownOption(option) => trf("Unknown option: {option}", &[("option", option)]),
            Self::MissingNumber(option) => trf("{option} needs a number", &[("option", option)]),
            Self::InvalidNumber { option, value } => trf("{option} needs a number from 1 up, not {value}", &[("option", option), ("value", value)]),
            Self::MissingFile => tr("--line and --column must come before a file").to_string(),
            Self::MultipleProjects => tr("Specify one Cargo.toml or project folder").to_string(),
            Self::UntrustedProject => tr("Inspection may execute build scripts and procedural macros. Pass --trust-project only if you trust this project.").to_string(),
            Self::ComparisonPaths { merge: false } => tr("Use: crabcraft diff LEFT RIGHT").to_string(),
            Self::ComparisonPaths { merge: true } => tr("Use: crabcraft merge LEFT RIGHT BASE OUTPUT").to_string(),
            Self::HttpFile => tr("Specify one HTTP request file").to_string(),
            Self::InvalidManifest => tr("Specify an existing Cargo.toml or project folder").to_string(),
        };
        f.write_str(&message)
    }
}

impl std::error::Error for CliError {}

/// `--help`, in the user's language.
pub fn usage() -> String {
    let rows = [
        ("diff LEFT RIGHT", tr("Open a native comparison window for two text files")),
        ("merge LEFT RIGHT BASE OUTPUT", tr("Open a native three-way merge; write OUTPUT only when saved")),
        ("http FILE", tr("Send an HTTP request without a window; select with --request N; print JSON")),
        ("format [path]", tr("Format a Cargo workspace; --check reports changes without writing")),
        ("inspect [path]", tr("Run Clippy; requires --trust-project because project code may run")),
        ("--offline", tr("Run workspace tools without accessing the network")),
        ("-l, --line N", tr("Put the caret on line N of the next file")),
        ("-c, --column N", tr("Put the caret on column N of the next file")),
        ("-h, --help", tr("Show this help")),
        ("-V, --version", tr("Show the version")),
    ];
    let mut out = format!(
        "{} crabcraft [options] [path[:line[:column]]]...\n\n{}\n\n{}\n",
        tr("Usage:"),
        tr("Opens folders, Cargo projects and files. Files go to the CrabCraft window that is already open, if any; a folder or a Cargo.toml opens in a new window."),
        tr("Options:")
    );
    for (option, text) in rows {
        out.push_str(&format!("  {option:<16} {text}\n"));
    }
    out
}

/// Reads the arguments after the program name. `cwd` makes relative paths absolute.
pub fn parse(args: impl IntoIterator<Item = OsString>, cwd: &Path) -> Result<Command, CliError> {
    let mut args = args.into_iter().peekable();
    if args.peek().is_some_and(|arg| arg == "diff" || arg == "merge") {
        let merge = args.next().is_some_and(|arg| arg == "merge");
        return crate::compare_cli::parse(merge, args, cwd);
    }
    if args.peek().is_some_and(|arg| arg == "http") {
        args.next();
        return crate::http_cli::parse(args, cwd);
    }
    if args.peek().is_some_and(|arg| arg == "format" || arg == "inspect") {
        let inspect = args.next().is_some_and(|arg| arg == "inspect");
        return parse_tool(inspect, args, cwd);
    }
    let mut out = Vec::new();
    let (mut line, mut column) = (None, None);
    let mut only_paths = false;
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        // macOS adds `-psn_0_12345` when the app is launched from Finder.
        if text.starts_with("-psn_") {
            continue;
        }
        if !only_paths && text.starts_with('-') && text.len() > 1 {
            match text.as_ref() {
                "--" => only_paths = true,
                "-h" | "--help" => return Ok(Command::Help),
                "-V" | "--version" => return Ok(Command::Version),
                "-l" | "--line" => line = Some(number(&text, args.next())?),
                "-c" | "--column" => column = Some(number(&text, args.next())?),
                _ => return Err(CliError::UnknownOption(text.to_string())),
            }
            continue;
        }
        let path = absolute(PathBuf::from(arg), cwd);
        let mut request = split_position(path);
        if line.is_some() {
            request.line = line.take();
            request.column = column.take().or(request.column);
        } else if column.is_some() {
            request.line = request.line.or(Some(1));
            request.column = column.take();
        }
        out.push(request);
    }
    if line.is_some() || column.is_some() {
        return Err(CliError::MissingFile);
    }
    Ok(Command::Open(out))
}

/// Headless workspace tools never contact the running GUI or inherit its trust decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRequest {
    pub manifest: PathBuf,
    pub inspect: bool,
    pub check: bool,
    pub offline: bool,
}

fn parse_tool(inspect: bool, args: impl Iterator<Item = OsString>, cwd: &Path) -> Result<Command, CliError> {
    let (mut path, mut check, mut offline, mut trust) = (None, false, false, false);
    let mut only_paths = false;
    for arg in args {
        match arg.to_str() {
            Some("--") if !only_paths => only_paths = true,
            Some("--help" | "-h") if !only_paths => return Ok(Command::Help),
            Some("--check") if !only_paths && !inspect => check = true,
            Some("--trust-project") if !only_paths && inspect => trust = true,
            Some("--offline") if !only_paths => offline = true,
            Some(text) if !only_paths && text.starts_with('-') => return Err(CliError::UnknownOption(text.to_string())),
            _ if path.is_none() => path = Some(absolute(arg.into(), cwd)),
            _ => return Err(CliError::MultipleProjects),
        }
    }
    if inspect && !trust {
        return Err(CliError::UntrustedProject);
    }
    let path = path.unwrap_or_else(|| cwd.to_path_buf());
    let manifest = if path.is_dir() { path.join("Cargo.toml") } else { path };
    if manifest.file_name().is_none_or(|name| name != "Cargo.toml") || !manifest.is_file() {
        return Err(CliError::InvalidManifest);
    }
    Ok(Command::Tool(ToolRequest { manifest, inspect, check, offline }))
}

impl ToolRequest {
    fn command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new("cargo");
        cmd.arg(if self.inspect { "clippy" } else { "fmt" });
        cmd.arg("--manifest-path").arg(&self.manifest);
        if let Some(dir) = self.manifest.parent() {
            cmd.current_dir(dir);
        }
        cmd.env("RUSTUP_AUTO_INSTALL", "0");
        // Formatting does not build a project. Clippy runs only with explicit CLI trust.
        if self.inspect {
            cmd.args(["--workspace", "--all-targets"]);
            if self.offline { cmd.arg("--offline"); }
            cmd.args(["--", "-D", "warnings"]);
        } else {
            cmd.arg("--all");
            if self.check { cmd.arg("--check"); }
            if self.offline { cmd.env("CARGO_NET_OFFLINE", "true"); }
        }
        cmd
    }

    /// Runs before any window is created. Inherited streams keep output live and bounded;
    /// the foreground Cargo process receives terminal cancellation (Ctrl+C) directly.
    pub fn run(&self) -> Result<i32, std::io::Error> {
        self.command().status().map(|status| status.code().unwrap_or(1))
    }
}

pub(super) fn number(option: &str, value: Option<OsString>) -> Result<usize, CliError> {
    let value = value.ok_or_else(|| CliError::MissingNumber(option.to_string()))?;
    let text = value.to_string_lossy();
    text.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| CliError::InvalidNumber { option: option.to_string(), value: text.into_owned() })
}

fn absolute(path: PathBuf, cwd: &Path) -> PathBuf {
    if path.is_absolute() { path } else { cwd.join(path) }
}

/// `src/main.rs:12:5` → the file with line 12, column 5. Only when the whole text is not an
/// existing path, so a file really named `a:1` (or `C:\x` on Windows) still opens.
fn split_position(path: PathBuf) -> OpenRequest {
    if path.exists() {
        return OpenRequest::path(path);
    }
    let text = path.to_string_lossy().into_owned();
    let mut parts = text.rsplitn(3, ':');
    let (Some(last), Some(middle)) = (parts.next(), parts.next()) else { return OpenRequest::path(path) };
    let num = |s: &str| s.parse::<usize>().ok().filter(|n| *n > 0);
    match (parts.next(), num(middle), num(last)) {
        (Some(file), Some(line), Some(column)) if Path::new(file).exists() => OpenRequest { path: file.into(), line: Some(line), column: Some(column) },
        _ => {
            let file = text.rsplit_once(':').map(|(f, _)| f).unwrap_or(&text);
            match num(last) {
                Some(line) if Path::new(file).exists() => OpenRequest { path: file.into(), line: Some(line), column: None },
                _ => OpenRequest::path(path),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str], cwd: &Path) -> Result<Command, CliError> {
        parse(args.iter().map(OsString::from), cwd)
    }

    fn opens(args: &[&str], cwd: &Path) -> Vec<OpenRequest> {
        match run(args, cwd) {
            Ok(Command::Open(list)) => list,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn workspace_tools_validate_options_and_require_explicit_trust() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        assert!(run(&["inspect"], dir.path()).is_err());
        assert!(run(&["format", "--trust-project"], dir.path()).is_err());
        assert!(run(&["inspect", "--trust-project", "--check"], dir.path()).is_err());
        assert!(run(&["format", "a", "b"], dir.path()).is_err());
        assert!(run(&["format", "missing"], dir.path()).is_err());
        assert_eq!(run(&["inspect", "--help"], dir.path()), Ok(Command::Help));
        assert_eq!(run(&["format", "--check", "--offline"], dir.path()), Ok(Command::Tool(ToolRequest {
            manifest: dir.path().join("Cargo.toml"), inspect: false, check: true, offline: true,
        })));
    }

    #[test]
    fn workspace_tools_format_and_inspect_a_real_rust_project() {
        let dir = tempfile::Builder::new().prefix("crabcraft cli ").tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"cli_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
        std::fs::write(dir.path().join("build.rs"), "fn main() { std::fs::write(\"build-ran\", \"yes\").unwrap(); }\n").unwrap();
        let source = dir.path().join("src/main.rs");
        std::fs::write(&source, "fn main(){println!(\"hello\");}\n").unwrap();
        let tool = |args: &[&str]| match run(args, dir.path()).unwrap() {
            Command::Tool(tool) => tool,
            other => panic!("{other:?}"),
        };
        assert_ne!(tool(&["format", "--check", "--offline"]).run().unwrap(), 0);
        assert_eq!(tool(&["format", "--offline"]).run().unwrap(), 0);
        assert_eq!(tool(&["format", "--check", "--offline"]).run().unwrap(), 0);
        assert!(!dir.path().join("build-ran").exists());
        assert!(run(&["inspect", "--offline"], dir.path()).is_err());
        assert!(!dir.path().join("build-ran").exists());
        assert_eq!(tool(&["inspect", "--trust-project", "--offline"]).run().unwrap(), 0);
        assert!(dir.path().join("build-ran").exists());
        std::fs::write(&source, "fn main() { let unused = 42; }\n").unwrap();
        assert_ne!(tool(&["inspect", "--trust-project", "--offline"]).run().unwrap(), 0);
    }

    #[test]
    fn relative_paths_become_absolute() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(opens(&["src/main.rs", "/abs/x.rs"], dir.path()), vec![OpenRequest::path(dir.path().join("src/main.rs")), OpenRequest::path("/abs/x.rs")]);
        assert_eq!(opens(&[], dir.path()), vec![]);
    }

    #[test]
    fn line_and_column_options_apply_to_the_next_file() {
        let dir = tempfile::tempdir().unwrap();
        let list = opens(&["--line", "12", "-c", "5", "a.rs", "b.rs", "-l", "3", "c.rs"], dir.path());
        assert_eq!(list[0], OpenRequest { path: dir.path().join("a.rs"), line: Some(12), column: Some(5) });
        assert_eq!(list[1], OpenRequest::path(dir.path().join("b.rs")));
        assert_eq!(list[2], OpenRequest { path: dir.path().join("c.rs"), line: Some(3), column: None });
    }

    #[test]
    fn a_position_after_the_file_name_is_read_when_the_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        let file = dir.path().join("main.rs");
        assert_eq!(opens(&["main.rs:7:3"], dir.path()), vec![OpenRequest { path: file.clone(), line: Some(7), column: Some(3) }]);
        assert_eq!(opens(&["main.rs:7"], dir.path()), vec![OpenRequest { path: file.clone(), line: Some(7), column: None }]);
        // Not a position: kept as the path it is.
        assert_eq!(opens(&["main.rs:x"], dir.path()), vec![OpenRequest::path(dir.path().join("main.rs:x"))]);
        assert_eq!(opens(&["other.rs:7"], dir.path()), vec![OpenRequest::path(dir.path().join("other.rs:7"))]);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_whose_name_looks_like_a_position_opens_as_is() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a:1"), "").unwrap();
        assert_eq!(opens(&["a:1"], dir.path()), vec![OpenRequest::path(dir.path().join("a:1"))]);
    }

    #[test]
    fn help_version_and_bad_options() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(run(&["x", "--help"], dir.path()), Ok(Command::Help));
        assert_eq!(run(&["-V"], dir.path()), Ok(Command::Version));
        assert!(run(&["--bogus"], dir.path()).is_err());
        assert!(run(&["--line"], dir.path()).is_err());
        assert!(run(&["--line", "0", "a.rs"], dir.path()).is_err());
        assert!(run(&["--line", "ten", "a.rs"], dir.path()).is_err());
        assert!(run(&["a.rs", "--line", "3"], dir.path()).is_err());
        assert_eq!(opens(&["-psn_0_123", "--", "-x.rs"], dir.path()), vec![OpenRequest::path(dir.path().join("-x.rs"))]);
    }
}
