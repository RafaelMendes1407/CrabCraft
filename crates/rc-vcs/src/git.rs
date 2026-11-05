//! The `git` executable: finding it, checking its version and running it asynchronously
//! (stdin/stdout/stderr piped, killed on cancel or drop, progress lines streamed).

use crate::cancel::CancelToken;
use crate::error::{GitError, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Oldest git CrabCraft supports (`switch`/`restore`, porcelain v2, `blame --contents -`).
pub const MIN_VERSION: GitVersion = GitVersion { major: 2, minor: 25, patch: 0 };

/// A git version as printed by `git --version`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    /// Parses `git version 2.45.1`, `git version 2.39.3 (Apple Git-146)`,
    /// `git version 2.45.1.windows.1`.
    pub fn parse(text: &str) -> Option<GitVersion> {
        let v = text.trim().strip_prefix("git version ")?.split_whitespace().next()?;
        let mut parts = v.split('.').map(|p| p.parse::<u32>().ok());
        Some(GitVersion { major: parts.next()??, minor: parts.next()??, patch: parts.next().flatten().unwrap_or(0) })
    }
}

impl std::fmt::Display for GitVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// One progress line of a network operation (`Receiving objects:  45% (45/100)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub text: String,
    /// The percentage in the line, when there is one.
    pub percent: Option<u8>,
}

impl Progress {
    pub fn parse(line: &str) -> Progress {
        let percent = line.find('%').and_then(|at| {
            let digits: String = line[..at].chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect();
            digits.parse::<u8>().ok().filter(|p| *p <= 100)
        });
        Progress { text: line.trim_end().to_string(), percent }
    }
}

/// Receives progress lines while fetch/pull/push run (called from the runtime's threads).
pub type ProgressSink = Arc<dyn Fn(Progress) + Send + Sync>;

/// A progress sink that forwards every line to the UI through an outbox (which wakes it).
pub fn progress_to(outbox: rc_core::inbox::Outbox<Progress>) -> ProgressSink {
    Arc::new(move |p| {
        outbox.send(p);
    })
}

/// A located, version-checked git executable.
#[derive(Clone, Debug)]
pub struct Git {
    path: PathBuf,
    version: GitVersion,
}

impl Git {
    /// Looks for `git` in `PATH`, then in the usual install locations (Git for Windows,
    /// Homebrew, Xcode command line tools) that GUI apps may not have in their `PATH`.
    pub fn find_executable() -> Option<PathBuf> {
        let exe = if cfg!(windows) { "git.exe" } else { "git" };
        let mut candidates: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join(exe)).collect()).unwrap_or_default();
        if cfg!(windows) {
            for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
                if let Some(dir) = std::env::var_os(var) {
                    candidates.push(Path::new(&dir).join("Git").join("cmd").join(exe));
                }
            }
            if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
                candidates.push(Path::new(&dir).join("Programs").join("Git").join("cmd").join(exe));
            }
            candidates.push(PathBuf::from(r"C:\Program Files\Git\cmd\git.exe"));
        } else {
            candidates.extend(["/opt/homebrew/bin/git", "/usr/local/bin/git", "/usr/bin/git"].map(PathBuf::from));
        }
        candidates.into_iter().find(|p| p.is_file())
    }

    /// Checks the executable at `path` (runs `git --version`).
    pub async fn open(path: PathBuf) -> Result<Git> {
        let out = tokio::process::Command::new(&path).arg("--version").stdin(Stdio::null()).output().await.map_err(|_| GitError::GitNotFound)?;
        let text = String::from_utf8_lossy(&out.stdout);
        let version = GitVersion::parse(&text).ok_or_else(|| GitError::Parse(text.to_string()))?;
        if version < MIN_VERSION {
            return Err(GitError::UnsupportedVersion { found: version.to_string(), required: MIN_VERSION.to_string() });
        }
        Ok(Git { path, version })
    }

    /// The git found by [`Git::find_executable`], checked once and shared afterwards.
    pub async fn detect() -> Result<Arc<Git>> {
        static GIT: tokio::sync::OnceCell<Arc<Git>> = tokio::sync::OnceCell::const_new();
        GIT.get_or_try_init(|| async {
            let path = Git::find_executable().ok_or(GitError::GitNotFound)?;
            Git::open(path).await.map(Arc::new)
        })
        .await
        .cloned()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn version(&self) -> GitVersion {
        self.version
    }

    /// A git command run in `cwd`.
    pub fn command(&self, cwd: &Path) -> GitCommand<'_> {
        GitCommand { git: self, cwd: cwd.to_path_buf(), args: Vec::new(), stdin: None, cancel: None, ok_codes: vec![0], trusted: false }
    }
}

/// Captured result of a git run.
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub code: Option<i32>,
}

impl Output {
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// A live command owns its descendants until normal completion. Cancellation, future
/// drop and I/O failures kill the process group, including editors and hook children.
struct ProcessGroup { pid: Option<u32> }
impl ProcessGroup {
    fn new(child: &tokio::process::Child) -> Self { Self { pid: child.id() } }
    fn disarm(&mut self) { self.pid = None; }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        let Some(pid) = self.pid else { return };
        #[cfg(unix)] { unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL); } }
        #[cfg(windows)] {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).creation_flags(0x0800_0000).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }
}

/// Builder for one git invocation.
#[derive(Clone)]
pub struct GitCommand<'a> {
    git: &'a Git,
    cwd: PathBuf,
    args: Vec<OsString>,
    stdin: Option<Vec<u8>>,
    cancel: Option<CancelToken>,
    ok_codes: Vec<i32>,
    trusted: bool,
}

impl GitCommand<'_> {
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    /// Explicitly allow configured helpers and writes after the caller obtains project trust.
    pub fn trusted(mut self, trusted: bool) -> Self { self.trusted = trusted; self }

    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(mut self, args: I) -> Self {
        self.args.extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    /// Bytes written to git's stdin.
    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }

    pub fn cancel(mut self, token: Option<&CancelToken>) -> Self {
        self.cancel = token.cloned();
        self
    }

    /// Accepts another exit code as success (e.g. 1 for `diff --no-index`).
    pub fn ok_code(mut self, code: i32) -> Self {
        self.ok_codes.push(code);
        self
    }

    fn build(&self, overrides: &[OsString]) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.git.path);
        if !self.trusted {
            cmd.arg("--no-pager").args(["-c","core.fsmonitor=","-c","core.hooksPath=/dev/null","-c","log.showSignature=false","-c","submodule.recurse=false","-c","diff.ignoreSubmodules=all","-c","status.submoduleSummary=false","-c","maintenance.auto=false","-c","gc.auto=0","-c","core.alternateRefsCommand=","-c","diff.external=","-c","gpg.program=/dev/null","-c","gpg.openpgp.program=/dev/null","-c","gpg.ssh.program=/dev/null","-c","gpg.x509.program=/dev/null"])
                .args(overrides).env("GIT_ALLOW_PROTOCOL", "").env("GIT_NO_LAZY_FETCH", "1").env_remove("GIT_EXTERNAL_DIFF").env_remove("GIT_DIFF_OPTS");
        }
        cmd.current_dir(&self.cwd)
            .args(["-c", "core.quotepath=false", "-c", "color.ui=false", "-c", "core.pager=cat", "-c", "advice.detachedHead=false"])
            .args(self.safe_args())
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_PAGER", "cat")
            .env("GIT_EDITOR", ":")
            .stdin(if self.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        cmd
    }

    fn safe_args(&self) -> Vec<OsString> {
        let mut args=self.args.clone(); if self.trusted {return args;}
        let options: &[&str]=match args.first().and_then(|arg|arg.to_str()) {
            Some("diff"|"diff-tree"|"log"|"show")=>&["--no-ext-diff","--no-textconv"],
            Some("blame")=>&["--no-textconv"], Some("status")=>&["--ignore-submodules=all"], _=>&[],
        };
        args.splice(1..1,options.iter().map(OsString::from));args
    }

    fn read_only(&self) -> bool {
        let Some(command) = self.args.first().and_then(|arg| arg.to_str()) else { return false };
        // Read-looking subcommands can still redirect output to disk. Refuse output and
        // write switches before spawning anything (including abbreviated long options).
        if self.args.iter().any(|arg| arg.to_str().is_some_and(|arg| arg.starts_with("--out") || arg.starts_with("--write") || arg.starts_with("--save") || arg.starts_with("-o"))) { return false; }

        if self.args.iter().any(|arg| arg.to_str().is_some_and(|arg| ["--ext-diff","--textconv","--filters","--recurse-submodules","--show-signature"].iter().any(|bad| arg==*bad || arg.starts_with(&format!("{bad}="))))) { return false; }
        match command {
            "status" | "log" | "show" | "diff" | "diff-tree" | "rev-parse" | "rev-list" | "cat-file" | "ls-tree" | "ls-files" | "blame" | "for-each-ref" | "check-ref-format" => true,
            "config" => self.args.iter().any(|arg| arg=="--list") && self.args.iter().skip(1).all(|arg| ["--list","--null","-z","--name-only","--includes"].iter().any(|allowed| arg==*allowed)),
            "remote" => self.args.len()==2 && self.args[1]=="-v",
            "worktree" | "stash" => self.args.get(1).is_some_and(|arg| arg=="list"),
            "symbolic-ref" => self.args.len()==4 && self.args[1]=="--quiet" && self.args[2]=="--short" && self.args[3]=="HEAD",
            "apply" => self.args.iter().any(|arg| arg=="--numstat" || arg=="--summary" || arg=="--check") && self.args.iter().skip(1).all(|arg| ["--numstat","-z","--summary","--check","--whitespace=nowarn","--3way","-"].iter().any(|allowed| arg==*allowed)),
            _ => false,
        }
    }

    /// Config parsing itself invokes no configured helpers. Enumerate arbitrary driver names
    /// before the real read, then override every clean/smudge/process driver. No values leak.
    async fn read_overrides(&self) -> Result<Vec<OsString>> {
        let mut query=self.clone(); query.args=["config","--null","--name-only","--list"].map(OsString::from).to_vec(); query.stdin=None;
        let mut child=query.build(&[]).spawn().map_err(GitError::Io)?;
        let mut group=ProcessGroup::new(&child);
        let stdout=child.stdout.take().ok_or(GitError::UnsafeReadConfig)?; let stderr=child.stderr.take().ok_or(GitError::UnsafeReadConfig)?;
        let work=async {
            let read=async { let mut names=Vec::new(); stdout.take(1024*1024+1).read_to_end(&mut names).await.map_err(|_|GitError::UnsafeReadConfig)?; if names.len()>1024*1024 {return Err(GitError::UnsafeReadConfig);} Ok(names) };
            let errors=async {let mut bytes=Vec::new();stderr.take(64*1024+1).read_to_end(&mut bytes).await.map_err(|_|GitError::UnsafeReadConfig)?;if bytes.len()>64*1024 {return Err(GitError::UnsafeReadConfig);}Ok(())};
            let wait=async {let status=child.wait().await.map_err(|_|GitError::UnsafeReadConfig)?;if !status.success(){return Err(GitError::UnsafeReadConfig);}Ok(())};
            let(names,(),())=tokio::try_join!(read,errors,wait)?;Ok::<_,GitError>(names)
        };
        let names=match &self.cancel {
            Some(token)=>tokio::select! {result=tokio::time::timeout(std::time::Duration::from_secs(5),work)=>result.map_err(|_|GitError::UnsafeReadConfig)??,()=token.cancelled()=>return Err(GitError::Cancelled)},
            None=>tokio::time::timeout(std::time::Duration::from_secs(5),work).await.map_err(|_|GitError::UnsafeReadConfig)??,
        };
        group.disarm();
        let mut filters=std::collections::BTreeSet::new(); let mut diffs=std::collections::BTreeSet::new();
        for name in names.split(|byte|*byte==0) {
            let name=std::str::from_utf8(name).map_err(|_|GitError::UnsafeReadConfig)?;
            if name.len()>8192 {return Err(GitError::UnsafeReadConfig);}
            if let Some(rest)=name.strip_prefix("filter.") && let Some((driver,_))=rest.rsplit_once('.') {filters.insert(driver.to_string());}
            if let Some(rest)=name.strip_prefix("diff.") && let Some((driver,_))=rest.rsplit_once('.') {diffs.insert(driver.to_string());}
        }
        if filters.len()+diffs.len()>256 {return Err(GitError::UnsafeReadConfig);}
        let mut overrides=Vec::new();
        for driver in filters {for property in ["clean=","smudge=","process=","required=false"] {overrides.push(OsString::from("-c"));overrides.push(OsString::from(format!("filter.{driver}.{property}")));}}
        for driver in diffs {for property in ["command=","textconv="] {overrides.push(OsString::from("-c"));overrides.push(OsString::from(format!("diff.{driver}.{property}")));}}
        Ok(overrides)
    }

    fn arg_strings(&self) -> Vec<String> {
        self.args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    fn finish(&self, out: Output) -> Result<Output> {
        if out.code.is_some_and(|c| self.ok_codes.contains(&c)) {
            Ok(out)
        } else {
            Err(GitError::classify(&self.arg_strings(), out.code, &String::from_utf8_lossy(&out.stdout), &out.stderr))
        }
    }

    /// Runs git to completion; non-accepted exit codes become classified errors.
    pub async fn output(self) -> Result<Output> {
        self.streamed(None).await
    }

    /// Runs git and returns stdout as (lossy) UTF-8 text.
    pub async fn text(self) -> Result<String> {
        Ok(self.output().await?.stdout_text())
    }

    /// Runs git, sending every stderr line (split on `\r` and `\n`) to `progress`.
    pub async fn streamed(self, progress: Option<ProgressSink>) -> Result<Output> {
        if self.cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
            return Err(GitError::Cancelled);
        }
        let overrides = if self.trusted { Vec::new() } else {
            if !self.read_only() { return Err(GitError::TrustRequired); }
            self.read_overrides().await?
        };
        let mut child = self.build(&overrides).spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { GitError::GitNotFound } else { GitError::Io(e) })?;
        let mut group = ProcessGroup::new(&child);
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let input = self.stdin.clone();
        let work = async {
            let write = async {
                if let (Some(mut pipe), Some(bytes)) = (stdin, input) {
                    // git may exit before reading everything (e.g. on errors); that is fine.
                    let _ = pipe.write_all(&bytes).await;
                    let _ = pipe.shutdown().await;
                }
            };
            let read_out = async {
                let mut buf = Vec::new();
                stdout.take(32 * 1024 * 1024 + 1).read_to_end(&mut buf).await.map_err(GitError::Io)?;
                if buf.len() > 32 * 1024 * 1024 { return Err(GitError::OutputLimit); }
                Ok::<_, GitError>(buf)
            };
            let read_err = async {
                let mut all = Vec::new();
                let mut line = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = stderr.read(&mut chunk).await.map_err(GitError::Io)?;
                    if n == 0 {
                        break;
                    }
                    if all.len() + n > 4 * 1024 * 1024 { return Err(GitError::OutputLimit); }
                    all.extend_from_slice(&chunk[..n]);
                    if let Some(sink) = &progress {
                        for &b in &chunk[..n] {
                            if b == b'\r' || b == b'\n' {
                                if !line.is_empty() {
                                    sink(Progress::parse(&String::from_utf8_lossy(&line)));
                                    line.clear();
                                }
                            } else {
                                line.push(b);
                            }
                        }
                    }
                }
                if let Some(sink) = &progress
                    && !line.is_empty()
                {
                    sink(Progress::parse(&String::from_utf8_lossy(&line)));
                }
                Ok::<_, GitError>(all)
            };
            let ((), out, err) = tokio::try_join!(async { write.await; Ok::<_, GitError>(()) }, read_out, read_err)?;
            let status = child.wait().await.map_err(GitError::Io)?;
            Ok::<_, GitError>(Output { stdout: out, stderr: String::from_utf8_lossy(&err).into_owned(), code: status.code() })
        };
        let out = match &self.cancel {
            Some(token) => tokio::select! {
                out = work => out?,
                // The process-group guard kills editor/hook descendants as well.
                () = token.cancelled() => return Err(GitError::Cancelled),
            },
            None => work.await?,
        };
        group.disarm();
        self.finish(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(GitVersion::parse("git version 2.39.3 (Apple Git-146)\n"), Some(GitVersion { major: 2, minor: 39, patch: 3 }));
        assert_eq!(GitVersion::parse("git version 2.45.1.windows.1"), Some(GitVersion { major: 2, minor: 45, patch: 1 }));
        assert_eq!(GitVersion::parse("git version 3.0"), Some(GitVersion { major: 3, minor: 0, patch: 0 }));
        assert!(GitVersion::parse("hello").is_none());
        assert!(GitVersion { major: 2, minor: 24, patch: 9 } < MIN_VERSION);
    }

    #[test]
    fn parses_progress() {
        assert_eq!(Progress::parse("Receiving objects:  45% (45/100), 1.2 MiB").percent, Some(45));
        assert_eq!(Progress::parse("remote: Counting objects: 100% (3/3), done.").percent, Some(100));
        assert_eq!(Progress::parse("From /tmp/x").percent, None);
    }
}
