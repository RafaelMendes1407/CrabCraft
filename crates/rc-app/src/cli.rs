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
    Help,
    Version,
}

/// `--help`, in the user's language.
pub fn usage() -> String {
    let rows = [
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
pub fn parse(args: impl IntoIterator<Item = OsString>, cwd: &Path) -> Result<Command, String> {
    let mut out = Vec::new();
    let (mut line, mut column) = (None, None);
    let mut args = args.into_iter();
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
                _ => return Err(trf("Unknown option: {option}", &[("option", &text)])),
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
        return Err(tr("--line and --column must come before a file").to_string());
    }
    Ok(Command::Open(out))
}

fn number(option: &str, value: Option<OsString>) -> Result<usize, String> {
    let value = value.ok_or_else(|| trf("{option} needs a number", &[("option", &option)]))?;
    let text = value.to_string_lossy();
    text.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| trf("{option} needs a number from 1 up, not {value}", &[("option", &option), ("value", &text)]))
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

    fn run(args: &[&str], cwd: &Path) -> Result<Command, String> {
        parse(args.iter().map(OsString::from), cwd)
    }

    fn opens(args: &[&str], cwd: &Path) -> Vec<OpenRequest> {
        match run(args, cwd) {
            Ok(Command::Open(list)) => list,
            other => panic!("{other:?}"),
        }
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
