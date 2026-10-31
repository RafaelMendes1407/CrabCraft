//! rustfmt over whole files, for formatting at commit time.

use crate::toolchain::Cargo;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Longest time rustfmt may take on one file.
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatFile {
    pub path: PathBuf,
    pub edition: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// The `rustfmt` component is not installed.
    Missing,
    /// This file could not be formatted; the message is rustfmt's or the system's.
    File { path: PathBuf, message: String },
}

/// Formats every file or none: all are formatted in memory first, so a syntax error in one leaves
/// them as they were. Text goes through stdin because given a path rustfmt also rewrites the
/// modules that file declares, which the caller did not choose.
pub async fn format_files(cargo: &Cargo, files: &[FormatFile]) -> Result<(), FormatError> {
    let program = cargo.path().with_file_name(if cfg!(windows) { "rustfmt.exe" } else { "rustfmt" });
    if !program.is_file() {
        return Err(FormatError::Missing);
    }
    let mut changed = Vec::new();
    for file in files {
        let failed = |message: String| FormatError::File { path: file.path.clone(), message };
        let text = std::fs::read_to_string(&file.path).map_err(|e| failed(e.to_string()))?;
        let formatted = format_text(&program, file, &text).await.map_err(failed)?;
        if formatted != text {
            changed.push((file, formatted));
        }
    }
    for (file, formatted) in changed {
        replace(&file.path, &formatted).map_err(|e| FormatError::File { path: file.path.clone(), message: e.to_string() })?;
    }
    Ok(())
}

async fn format_text(program: &Path, file: &FormatFile, text: &str) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(["--edition", &file.edition, "--emit", "stdout"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    // rustfmt.toml is searched from here.
    if let Some(dir) = file.path.parent() {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("rustfmt has no input")?;
    let input = text.as_bytes().to_vec();
    tokio::spawn(async move {
        let _ = stdin.write_all(&input).await;
    });
    let out = tokio::time::timeout(TIMEOUT, child.wait_with_output()).await.map_err(|_| format!("rustfmt did not finish in {} s", TIMEOUT.as_secs()))?.map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

/// Replaces a file through a sibling, so a failed write leaves the original whole.
fn replace(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = dir.join(format!(".{name}.{}.rustfmt", std::process::id()));
    let written = (|| {
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        out.write_all(text.as_bytes())?;
        out.sync_all()?;
        std::fs::set_permissions(&temp, std::fs::metadata(path)?.permissions())?;
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cargo: &Cargo, files: &[FormatFile]) -> Result<(), FormatError> {
        rc_core::Runtime::block_on(format_files(cargo, files))
    }

    fn file(path: PathBuf) -> FormatFile {
        FormatFile { path, edition: "2024".into() }
    }

    #[test]
    fn formats_the_file_with_its_config_and_leaves_its_modules_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (main, child) = (dir.path().join("main.rs"), dir.path().join("child.rs"));
        std::fs::write(dir.path().join("rustfmt.toml"), "tab_spaces = 2\n").unwrap();
        std::fs::write(&main, "mod child;\nfn main(){\nlet a=1;\n}\n").unwrap();
        std::fs::write(&child, "fn  x( ){}\n").unwrap();
        run(&Cargo::locate().unwrap(), &[file(main.clone())]).unwrap();
        assert_eq!(std::fs::read_to_string(&main).unwrap(), "mod child;\nfn main() {\n  let a = 1;\n}\n");
        assert_eq!(std::fs::read_to_string(&child).unwrap(), "fn  x( ){}\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3, "no temporary file is left");
    }

    #[test]
    fn one_bad_file_leaves_every_file_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let (good, bad) = (dir.path().join("good.rs"), dir.path().join("bad.rs"));
        std::fs::write(&good, "fn  x( ){}\n").unwrap();
        std::fs::write(&bad, "fn broken( {\n").unwrap();
        let error = run(&Cargo::locate().unwrap(), &[file(good.clone()), file(bad.clone())]).unwrap_err();
        assert!(matches!(&error, FormatError::File { path, message } if *path == bad && message.contains("unclosed")), "{error:?}");
        assert_eq!(std::fs::read_to_string(&good).unwrap(), "fn  x( ){}\n");
        assert!(run(&Cargo::locate().unwrap(), &[file(dir.path().join("none.rs"))]).is_err());
    }

    #[test]
    fn a_toolchain_without_rustfmt_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(run(&Cargo::new(dir.path().join("cargo")), &[]), Err(FormatError::Missing));
    }
}
