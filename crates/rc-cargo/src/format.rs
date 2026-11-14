//! rustfmt over whole files: Reformat Code, formatting on save and at commit time, all run the
//! same command, with the arguments, channel and environment of the settings.

use crate::toolchain::Cargo;
use rc_core::{Inbox, RustfmtSettings, Waker};
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
    CriticalupMissing,
    CriticalupChannel,
    /// The extra arguments end inside a quote.
    Arguments,
    /// The channel is not a toolchain name.
    Channel(String),
    /// This line of the environment is not `NAME=value`.
    Environment(String),
    /// This file could not be formatted; the message is rustfmt's or the system's.
    File {
        path: PathBuf,
        message: String,
    },
}

/// How rustfmt runs: its program, the channel and extra arguments, and the environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rustfmt {
    cargo: Cargo,
    /// `+channel` first (read by rustup), then the user's arguments.
    args: Vec<String>,
    env: Vec<(String, String)>,
}

impl Rustfmt {
    /// The rustfmt next to `cargo`, run as the settings say. Settings are checked here, so a
    /// mistake is reported before any file is touched.
    pub fn new(cargo: &Cargo, settings: &RustfmtSettings) -> Result<Rustfmt, FormatError> {
        let (args, env) = Self::read(settings)?;
        if cargo.is_criticalup() && !settings.channel.trim().is_empty() {
            return Err(FormatError::CriticalupChannel);
        }
        let missing = if cargo.is_criticalup() { FormatError::CriticalupMissing } else { FormatError::Missing };
        let program = cargo.tool_path("rustfmt").ok_or_else(|| missing.clone())?;
        if !program.is_file() {
            return Err(missing);
        }
        let mut merged = cargo.env().to_vec();
        merged.extend(env.clone());
        Ok(Rustfmt { cargo: cargo.clone().with_env(merged), args, env })
    }

    /// Whether the settings can be used, without looking for the program.
    pub fn check(settings: &RustfmtSettings) -> Result<(), FormatError> {
        Self::read(settings).map(|_| ())
    }

    #[allow(clippy::type_complexity)]
    fn read(settings: &RustfmtSettings) -> Result<(Vec<String>, Vec<(String, String)>), FormatError> {
        let mut args = Vec::new();
        let channel = settings.channel.trim();
        let channel = channel.strip_prefix('+').unwrap_or(channel);
        if !channel.is_empty() {
            if !is_toolchain_name(channel) {
                return Err(FormatError::Channel(channel.to_string()));
            }
            args.push(format!("+{channel}"));
        }
        args.extend(split_args(&settings.args).ok_or(FormatError::Arguments)?);
        let env = parse_env(&settings.env).map_err(FormatError::Environment)?;
        Ok((args, env))
    }

    /// `text` formatted as the file at `path`, of a crate of `edition`. The file is not read
    /// or written: the text goes through stdin, and rustfmt.toml is searched from its folder.
    pub async fn format(&self, path: &Path, edition: &str, text: &str) -> Result<String, String> {
        let mut cmd = self.cargo.tool_command("rustfmt");
        cmd.args(&self.args);
        // The user's own `--edition` wins; rustfmt refuses it twice.
        if !self.args.iter().any(|a| a == "--edition" || a.starts_with("--edition=")) {
            cmd.args(["--edition", edition]);
        }
        cmd.args(["--emit", "stdout"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        // A channel that is not installed must fail, not start a download.
        cmd.env("RUSTUP_AUTO_INSTALL", "0");
        if !self.cargo.is_criticalup() {
            cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        }
        if let Some(dir) = path.parent().filter(|d| d.is_dir()) {
            cmd.current_dir(dir);
        }
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let _group = FormatGroup(child.id());
        let mut stdin = child.stdin.take().ok_or("rustfmt has no input")?;
        let input = text.as_bytes().to_vec();
        tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
        });
        let out = tokio::time::timeout(TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| format!("rustfmt did not finish in {} s", TIMEOUT.as_secs()))?
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            let error = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if error.is_empty() { format!("rustfmt exited with {}", out.status) } else { error });
        }
        String::from_utf8(out.stdout).map_err(|e| e.to_string())
    }
}

impl Rustfmt {
    /// Formats `text` in the background, for an open document; the UI polls the job.
    pub fn start(self, path: PathBuf, edition: String, text: String, waker: Waker) -> FormatJob {
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let task = rc_core::Runtime::spawn(async move {
            out.send(self.format(&path, &edition, &text).await);
        });
        FormatJob { task, inbox }
    }
}

/// rustfmt running on one text; dropping the job stops it.
pub struct FormatJob {
    task: tokio::task::JoinHandle<()>,
    inbox: Inbox<Result<String, String>>,
}

impl FormatJob {
    /// Resolve sibling tools and encode the rope away from the UI before spawning rustfmt.
    pub fn configured(
        cargo: Result<Cargo, String>,
        settings: RustfmtSettings,
        path: PathBuf,
        edition: String,
        text: impl FnOnce() -> String + Send + 'static,
        failure: fn(FormatError) -> String,
        waker: Waker,
    ) -> Self {
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let task = rc_core::Runtime::spawn(async move {
            let prepared = tokio::task::spawn_blocking(move || {
                let cargo = cargo?;
                let formatter = Rustfmt::new(&cargo, &settings).map_err(failure)?;
                Ok::<_, String>((formatter, text()))
            })
            .await;
            let result = match prepared {
                Ok(Ok((formatter, text))) => formatter.format(&path, &edition, &text).await,
                Ok(Err(error)) => Err(error),
                Err(error) => Err(error.to_string()),
            };
            out.send(result);
        });
        Self { task, inbox }
    }
    /// The formatted text or rustfmt's error, once it finished.
    pub fn poll(&self) -> Option<Result<String, String>> {
        self.inbox.drain().into_iter().next()
    }
}

impl Drop for FormatJob {
    fn drop(&mut self) {
        // The child process is killed with the future.
        self.task.abort();
    }
}

/// A rustup toolchain name: `stable`, `nightly-2025-01-01`, `1.85.0-x86_64-pc-windows-msvc`.
fn is_toolchain_name(name: &str) -> bool {
    !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

/// Splits arguments at spaces; single or double quotes keep spaces inside one argument.
/// Backslashes are plain characters, as in Windows paths. `None` when a quote is left open.
pub fn split_args(text: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let (mut current, mut started, mut quote) = (String::new(), false, None);
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        args.push(current);
    }
    Some(args)
}

/// `NAME=value` lines; blank lines and `#` comments are skipped. The error is the bad line.
pub fn parse_env(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut env = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let (name, value) = line.split_once('=').ok_or_else(|| line.trim().to_string())?;
        let name = name.trim();
        if name.is_empty() || name.contains(char::is_whitespace) || name.contains('\0') || value.contains('\0') {
            return Err(line.trim().to_string());
        }
        env.push((name.to_string(), value.to_string()));
    }
    Ok(env)
}

/// Formats every file or none: all are formatted in memory first, so a syntax error in one leaves
/// them as they were. Text goes through stdin because given a path rustfmt also rewrites the
/// modules that file declares, which the caller did not choose.
pub async fn format_files(rustfmt: &Rustfmt, files: &[FormatFile]) -> Result<(), FormatError> {
    let mut changed = Vec::new();
    for file in files {
        let failed = |message: String| FormatError::File { path: file.path.clone(), message };
        let text = std::fs::read_to_string(&file.path).map_err(|e| failed(e.to_string()))?;
        let formatted = rustfmt.format(&file.path, &file.edition, &text).await.map_err(failed)?;
        if formatted != text {
            changed.push((file, formatted));
        }
    }
    for (file, formatted) in changed {
        replace(&file.path, &formatted).map_err(|e| FormatError::File { path: file.path.clone(), message: e.to_string() })?;
    }
    Ok(())
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
        run_with(cargo, &RustfmtSettings::default(), files)
    }

    fn run_with(cargo: &Cargo, settings: &RustfmtSettings, files: &[FormatFile]) -> Result<(), FormatError> {
        let rustfmt = Rustfmt::new(cargo, settings)?;
        rc_core::Runtime::block_on(format_files(&rustfmt, files))
    }

    fn settings(args: &str, channel: &str, env: &str) -> RustfmtSettings {
        RustfmtSettings { args: args.into(), channel: channel.into(), env: env.into() }
    }

    #[test]
    fn arguments_split_like_a_shell_without_escapes() {
        assert_eq!(split_args(""), Some(vec![]));
        assert_eq!(split_args("  --config  tab_spaces=2 "), Some(vec!["--config".to_string(), "tab_spaces=2".to_string()]));
        assert_eq!(
            split_args(r#"--config-path "C:\My Files\rustfmt.toml" '' x"#),
            Some(vec!["--config-path".to_string(), r"C:\My Files\rustfmt.toml".to_string(), String::new(), "x".to_string()])
        );
        assert_eq!(split_args("a\"b c\"d"), Some(vec!["ab cd".to_string()]));
        assert_eq!(split_args("--config 'x"), None);
    }

    #[test]
    fn the_environment_is_name_value_lines() {
        assert_eq!(
            parse_env("A=1\r\n\n  # note\nB = two words\nC=\nD=x=y"),
            Ok(vec![("A".into(), "1".into()), ("B".into(), " two words".into()), ("C".into(), String::new()), ("D".into(), "x=y".into())])
        );
        assert_eq!(parse_env("A=1\nnot a pair"), Err("not a pair".into()));
        assert_eq!(parse_env("=1"), Err("=1".into()));
        assert_eq!(parse_env("MY VAR=1"), Err("MY VAR=1".into()));
    }

    #[test]
    fn bad_settings_are_reported_before_anything_runs() {
        let cargo = Cargo::locate().unwrap();
        assert_eq!(Rustfmt::new(&cargo, &settings("'open", "", "")), Err(FormatError::Arguments));
        assert_eq!(Rustfmt::new(&cargo, &settings("", "night ly", "")), Err(FormatError::Channel("night ly".into())));
        assert_eq!(Rustfmt::new(&cargo, &settings("", "--help", "")), Err(FormatError::Channel("--help".into())));
        assert_eq!(Rustfmt::new(&cargo, &settings("", "", "oops")), Err(FormatError::Environment("oops".into())));
        let ok = Rustfmt::new(&cargo, &settings("--config tab_spaces=2", "+stable", "A=1")).unwrap();
        assert_eq!(ok.args, ["+stable", "--config", "tab_spaces=2"]);
        assert_eq!(ok.env, [("A".to_string(), "1".to_string())]);
    }

    #[test]
    fn arguments_channel_and_environment_reach_rustfmt() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.rs");
        let cargo = Cargo::locate().unwrap();
        std::fs::write(&main, "fn main(){\nlet a=1;\n}\n").unwrap();
        run_with(&cargo, &settings("--config tab_spaces=3", "stable", "RUSTFMT_UNUSED=1"), &[file(main.clone())]).unwrap();
        assert_eq!(std::fs::read_to_string(&main).unwrap(), "fn main() {\n   let a = 1;\n}\n");
        // rustup reads the variable, so a toolchain that does not exist proves it arrived.
        let error = run_with(&cargo, &settings("", "", "RUSTUP_TOOLCHAIN=crabcraft-missing"), &[file(main.clone())]).unwrap_err();
        assert!(matches!(&error, FormatError::File { message, .. } if message.contains("crabcraft-missing")), "{error:?}");
        let error = run_with(&cargo, &settings("", "crabcraft-missing-channel", ""), &[file(main.clone())]).unwrap_err();
        assert!(matches!(&error, FormatError::File { message, .. } if message.contains("crabcraft-missing-channel")), "{error:?}");
        // The user's edition replaces the crate's: `async` is a keyword only since 2018.
        std::fs::write(&main, "fn f(){ let async=1; }\n").unwrap();
        assert!(run_with(&cargo, &settings("--edition 2015", "", ""), &[file(main.clone())]).is_ok());
        assert_eq!(std::fs::read_to_string(&main).unwrap(), "fn f() {\n    let async = 1;\n}\n");
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

struct FormatGroup(Option<u32>);
impl Drop for FormatGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            rc_dotnet::kill_tree(pid);
        }
    }
}
