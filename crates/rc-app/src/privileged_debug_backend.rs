//! Linux privileged GDB: DAP owns stdio; a private Unix socket owns watchdog lifetime.
use rc_dotnet::CancelToken;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
const MAX_SPEC: usize = 64 * 1024;
const MAX_FRAME: usize = 1024 * 1024;
#[derive(Clone, Debug)]
pub enum Error {
    Platform,
    Trust,
    Approval,
    Input,
    Changed,
    Io(String),
    Protocol,
    Debugger,
    Cancelled,
    Limit,
    Privilege,
    Denied,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use rc_kit::i18n::{tr, trf};
        f.write_str(&match self {
            Self::Platform=>tr("Administrator debugging requires Linux, PolicyKit and system GDB with DAP").into(),
            Self::Trust=>tr("Trust this project before running project tools or code").into(),
            Self::Approval=>tr("Review the frozen executable and approve administrator debugging first").into(),
            Self::Input=>tr("Administrator debugging requires a regular project ELF and at most 32 separate arguments").into(),
            Self::Changed=>tr("The reviewed debug executable changed; build and review it again").into(),
            Self::Io(error)=>trf("Administrator debugging failed: {error}",&[("error",error)]),
            Self::Protocol=>tr("Unsupported administrator debug protocol request; the session was stopped").into(),
            Self::Debugger=>tr("Administrator debugging requires a root-owned, non-writable /usr/bin/gdb; user wrappers are unsupported").into(),
            Self::Cancelled=>tr("Administrator debug session cancelled").into(),
            Self::Limit=>tr("Administrator debug input, output or time limit exceeded").into(),
            Self::Privilege=>tr("The privileged helper did not receive administrator privileges").into(),
            Self::Denied=>tr("Administrator authorization was denied, dismissed or unavailable; no password is collected by the IDE").into(),
        })
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub root: PathBuf,
    pub artifact: PathBuf,
    pub frozen: PathBuf,
    pub sha256: String,
    pub args: Vec<String>,
}
impl Spec {
    pub fn validate(&self) -> Result<(), Error> {
        let unsafe_path = [&self.root, &self.artifact, &self.frozen]
            .iter()
            .any(|path| {
                path.to_str()
                    .is_none_or(|text| text.chars().any(char::is_control))
            });
        if unsafe_path
            || !self.root.is_absolute()
            || !self.artifact.is_absolute()
            || !self.artifact.starts_with(&self.root)
            || !self.frozen.is_absolute()
            || self.args.len() > 32
            || self
                .args
                .iter()
                .any(|a| a.len() > 4096 || a.chars().any(char::is_control))
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::Input);
        }
        if serde_json::to_vec(self).map_err(|_| Error::Input)?.len() > MAX_SPEC {
            return Err(Error::Limit);
        }
        Ok(())
    }
    pub fn json(&self) -> Result<String, Error> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| Error::Input)
    }
}
pub struct Frozen {
    pub spec: Spec,
    _directory: tempfile::TempDir,
}
impl Frozen {
    pub fn capture(
        root: PathBuf,
        artifact: PathBuf,
        args: Vec<String>,
        stop: &CancelToken,
    ) -> Result<Self, Error> {
        if stop.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let root = root.canonicalize()?;
        let canonical = artifact.canonicalize()?;
        if canonical != artifact || !canonical.starts_with(&root) {
            return Err(Error::Input);
        }
        let bytes =
            crate::privileged_run::read_program(&artifact, stop).map_err(|_| Error::Input)?;
        let directory = tempfile::Builder::new()
            .prefix("crabcraft-debug-review-")
            .tempdir()?;
        let frozen = directory.path().join("program");
        write_frozen(&frozen, &bytes)?;
        let spec = Spec {
            root,
            artifact,
            frozen,
            sha256: crate::privileged_run::digest(&bytes),
            args,
        };
        spec.validate()?;
        Ok(Self {
            spec,
            _directory: directory,
        })
    }
}
fn write_frozen(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o500).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
pub fn authorize(spec: &Spec, trusted: bool, approved: bool) -> Result<(), Error> {
    if !cfg!(target_os = "linux") {
        return Err(Error::Platform);
    }
    if !trusted {
        return Err(Error::Trust);
    }
    if !approved {
        return Err(Error::Approval);
    }
    spec.validate()
}
fn system_gdb() -> Result<PathBuf, Error> {
    let path = PathBuf::from("/usr/bin/gdb");
    let canonical = path.canonicalize().map_err(|_| Error::Debugger)?;
    if canonical != path {
        return Err(Error::Debugger);
    }
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| Error::Debugger)?;
    if !metadata.is_file() {
        return Err(Error::Debugger);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(Error::Debugger);
        }
    }
    Ok(path)
}
fn gdb_args() -> Vec<&'static str> {
    vec![
        "-nx",
        "-nh",
        "-quiet",
        "-i=dap",
        "-iex",
        "set auto-load off",
        "-iex",
        "set debuginfod enabled off",
        "-iex",
        "set startup-with-shell off",
    ]
}
async fn frame<R: AsyncRead + Unpin>(input: &mut R) -> Result<Option<serde_json::Value>, Error> {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let count = input.read(&mut byte).await?;
        if count == 0 {
            return if header.is_empty() {
                Ok(None)
            } else {
                Err(Error::Protocol)
            };
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
        if header.len() > 8192 {
            return Err(Error::Limit);
        }
    }
    let text = std::str::from_utf8(&header).map_err(|_| Error::Protocol)?;
    let mut length = None;
    for line in text.split("\r\n").filter(|l| !l.is_empty()) {
        let Some((key, value)) = line.split_once(':') else {
            return Err(Error::Protocol);
        };
        if key.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(Error::Protocol);
            }
            length = Some(value.trim().parse::<usize>().map_err(|_| Error::Protocol)?);
        }
    }
    let length = length
        .filter(|n| *n > 0 && *n <= MAX_FRAME)
        .ok_or(Error::Limit)?;
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| Error::Protocol)
}
async fn send<W: AsyncWrite + Unpin>(
    output: &mut W,
    value: &serde_json::Value,
) -> Result<(), Error> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Protocol)?;
    if bytes.len() > MAX_FRAME {
        return Err(Error::Limit);
    }
    output
        .write_all(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes())
        .await?;
    output.write_all(&bytes).await?;
    output.flush().await?;
    Ok(())
}
fn sanitize(
    value: &mut serde_json::Value,
    spec: &Spec,
    executable: &Path,
    launched: &mut bool,
) -> Result<(), Error> {
    if value.get("type").and_then(|v| v.as_str()) != Some("request") {
        return Err(Error::Protocol);
    }
    let command = value
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or(Error::Protocol)?;
    match command {
        "launch" => {
            if *launched {
                return Err(Error::Protocol);
            }
            let requested = value
                .pointer("/arguments/program")
                .and_then(|v| v.as_str())
                .ok_or(Error::Protocol)?;
            if Path::new(requested) != spec.artifact {
                return Err(Error::Changed);
            }
            let stop = value
                .pointer("/arguments/stopOnEntry")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            value["arguments"] = serde_json::json!({"program":executable,"cwd":spec.root,"args":spec.args,"env":{"PATH":"/usr/bin:/bin"},"stopOnEntry":stop});
            *launched = true;
        }
        "setBreakpoints" => {
            let source = value
                .pointer("/arguments/source/path")
                .and_then(|v| v.as_str())
                .ok_or(Error::Protocol)?;
            if !Path::new(source).is_absolute() || source.chars().any(char::is_control) {
                return Err(Error::Protocol);
            }
            let arguments = value
                .get_mut("arguments")
                .and_then(|v| v.as_object_mut())
                .ok_or(Error::Protocol)?;
            if let Some(points) = arguments
                .get_mut("breakpoints")
                .and_then(|v| v.as_array_mut())
            {
                for point in points {
                    let line = point
                        .get("line")
                        .and_then(|v| v.as_u64())
                        .filter(|line| *line > 0 && *line <= u32::MAX as u64)
                        .ok_or(Error::Protocol)?;
                    *point = serde_json::json!({"line":line});
                }
            }
            arguments.remove("lines");
        }
        "setFunctionBreakpoints" => {
            let points = value
                .pointer("/arguments/breakpoints")
                .and_then(|v| v.as_array())
                .ok_or(Error::Protocol)?;
            if points.len() != 1
                || points[0].get("name").and_then(|v| v.as_str())
                    != Some("std::panicking::rust_panic")
            {
                return Err(Error::Protocol);
            }
            value["arguments"] =
                serde_json::json!({"breakpoints":[{"name":"std::panicking::rust_panic"}]});
        }
        "setExceptionBreakpoints" => {
            value["arguments"] = serde_json::json!({"filters":[]});
        }
        "initialize"
        | "configurationDone"
        | "disconnect"
        | "terminate"
        | "cancel"
        | "threads"
        | "stackTrace"
        | "scopes"
        | "variables"
        | "continue"
        | "next"
        | "stepIn"
        | "stepOut"
        | "pause"
        | "exceptionInfo"
        | "modules"
        | "loadedSources"
        | "source"
        | "readMemory"
        | "disassemble"
        | "gotoTargets"
        | "goto"
        | "stepInTargets"
        | "breakpointLocations" => (),
        _ => return Err(Error::Protocol),
    }
    Ok(())
}
async fn input_pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut input: R,
    mut output: W,
    spec: Spec,
    program: PathBuf,
) -> Result<(), Error> {
    let mut launched = false;
    while let Some(mut value) = frame(&mut input).await? {
        sanitize(&mut value, &spec, &program, &mut launched)?;
        send(&mut output, &value).await?;
    }
    Err(Error::Cancelled)
}
#[cfg(target_os = "linux")]
struct Tree {
    pid: u32,
    handle: Option<std::os::fd::OwnedFd>,
    subreaper: i32,
    adopted: bool,
}
#[cfg(target_os = "linux")]
impl Tree {
    fn prepare(adopted: bool) -> Result<i32, Error> {
        let mut previous = 0;
        // SAFETY: these prctl operations affect this dedicated helper only.
        if adopted
            && unsafe { libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut previous, 0, 0, 0) } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        if adopted && unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(previous)
    }
    fn handle(pid: u32) -> Result<std::os::fd::OwnedFd, Error> {
        use std::os::fd::FromRawFd;
        // SAFETY: pidfd_open returns a new descriptor; it is owned exactly once here.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) })
    }
    fn signal(fd: &std::os::fd::OwnedFd, signal: i32) {
        use std::os::fd::AsRawFd;
        // SAFETY: pidfd_send_signal refers to the captured process, never a reused PID.
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
    }
    fn exited(fd: &std::os::fd::OwnedFd) -> bool {
        use std::os::fd::AsRawFd;
        let mut poll = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll checks only our owned pidfd and never blocks.
        unsafe { libc::poll(&mut poll, 1, 0) > 0 && poll.revents & libc::POLLIN != 0 }
    }
    fn children(pid: u32) -> Vec<u32> {
        let mut children = std::collections::BTreeSet::new();
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            return Vec::new();
        };
        for task in tasks.take(256).flatten() {
            if let Ok(text) = std::fs::read_to_string(task.path().join("children")) {
                children.extend(
                    text.split_whitespace()
                        .filter_map(|s| s.parse::<u32>().ok())
                        .filter(|p| *p > 1)
                        .take(4096),
                );
            }
        }
        children.into_iter().take(4096).collect()
    }
    fn parent(pid: u32) -> Option<u32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let end = stat.rfind(')')?;
        stat.get(end + 1..)?.split_whitespace().nth(1)?.parse().ok()
    }
}
#[cfg(target_os = "linux")]
impl Drop for Tree {
    fn drop(&mut self) {
        let Some(root) = self.handle.take() else {
            return;
        };
        let mut targets = std::collections::BTreeMap::from([(self.pid, root)]);
        // Inferiors can create new process groups. Track descendants through /proc, pin
        // them with pidfds and stop them before collecting children, then kill each group
        // member individually. The private helper adopts orphaned descendants as subreaper.
        for _ in 0..8 {
            for fd in targets.values() {
                Self::signal(fd, libc::SIGSTOP);
            }
            let mut fresh = Vec::new();
            for (pid, fd) in &targets {
                if Self::exited(fd) {
                    continue;
                }
                for child in Self::children(*pid) {
                    fresh.push((child, *pid));
                }
            }
            if self.adopted {
                for child in Self::children(std::process::id()) {
                    fresh.push((child, std::process::id()));
                }
            }
            for (pid, parent) in fresh.into_iter().take(4096) {
                if !targets.contains_key(&pid)
                    && let Ok(fd) = Self::handle(pid)
                    && Self::parent(pid) == Some(parent)
                {
                    targets.insert(pid, fd);
                }
            }
        }
        for fd in targets.values().rev() {
            Self::signal(fd, libc::SIGKILL);
        }
        let end = std::time::Instant::now() + Duration::from_secs(2);
        while !targets.is_empty() && std::time::Instant::now() < end {
            // Killing an ancestor can orphan a deeper/new process group after the
            // initial scan. The dedicated subreaper owns those children too.
            if self.adopted {
                for pid in Self::children(std::process::id()) {
                    if !targets.contains_key(&pid)
                        && let Ok(fd) = Self::handle(pid)
                        && Self::parent(pid) == Some(std::process::id())
                    {
                        Self::signal(&fd, libc::SIGSTOP);
                        Self::signal(&fd, libc::SIGKILL);
                        targets.insert(pid, fd);
                    }
                }
            }
            targets.retain(|pid, fd| {
                let mut status = 0;
                // SAFETY: only captured descendants are reaped, never another job's child.
                let result = unsafe { libc::waitpid(*pid as i32, &mut status, libc::WNOHANG) };
                result == 0
                    || (result < 0
                        && (std::io::Error::last_os_error().raw_os_error() != Some(libc::ECHILD)
                            || !Self::exited(fd)))
            });
            if !targets.is_empty() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if self.adopted {
            unsafe {
                libc::prctl(libc::PR_SET_CHILD_SUBREAPER, self.subreaper, 0, 0, 0);
            }
        }
    }
}
async fn output_pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut input: R,
    mut output: W,
) -> Result<(), Error> {
    let mut total = 0usize;
    while let Some(mut value) = frame(&mut input).await? {
        total = total.saturating_add(
            serde_json::to_vec(&value)
                .map_err(|_| Error::Protocol)?
                .len(),
        );
        if total > 16 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        if value.get("command").and_then(|v| v.as_str()) == Some("initialize")
            && let Some(body) = value.get_mut("body").and_then(|v| v.as_object_mut())
        {
            for capability in [
                "supportsEvaluateForHovers",
                "supportsSetVariable",
                "supportsSetExpression",
                "supportsDataBreakpoints",
                "supportsFunctionBreakpoints",
                "supportsInstructionBreakpoints",
                "supportsRestartRequest",
                "supportsWriteMemoryRequest",
            ] {
                body.insert(capability.into(), serde_json::Value::Bool(false));
            }
        }
        send(&mut output, &value).await?;
    }
    Ok(())
}
#[cfg(target_os = "linux")]
async fn supervise<R: AsyncRead + Unpin, W: AsyncWrite + Unpin, C: AsyncRead + Unpin>(
    spec: Spec,
    input: R,
    output: W,
    mut control: C,
    require_root: bool,
) -> Result<(), Error> {
    spec.validate()?;
    if require_root && unsafe { libc::geteuid() } != 0 {
        return Err(Error::Privilege);
    }
    if spec.root.canonicalize()? != spec.root || spec.frozen.canonicalize()? != spec.frozen {
        return Err(Error::Input);
    }
    let bytes = crate::privileged_run::read_program(&spec.frozen, &CancelToken::new())
        .map_err(|_| Error::Input)?;
    if crate::privileged_run::digest(&bytes) != spec.sha256 {
        return Err(Error::Changed);
    }
    let directory = tempfile::Builder::new()
        .prefix("crabcraft-root-debug-")
        .tempdir()?;
    let program = directory.path().join("program");
    write_frozen(&program, &bytes)?;
    let gdb = system_gdb()?;
    let mut byte = [0u8; 1];
    if tokio::time::timeout(Duration::from_millis(1), control.read(&mut byte))
        .await
        .is_ok()
    {
        return Err(Error::Cancelled);
    }
    let previous = Tree::prepare(true)?;
    let mut command = tokio::process::Command::new(gdb);
    command
        .args(gdb_args())
        .current_dir(directory.path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", directory.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command.spawn()?;
    let pid = child
        .id()
        .ok_or(Error::Io("missing debugger process identifier".into()))?;
    let handle = match Tree::handle(pid) {
        Ok(fd) => fd,
        Err(error) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(error);
        }
    };
    let tree = Tree {
        pid,
        handle: Some(handle),
        subreaper: previous,
        adopted: true,
    };
    let stdin = child.stdin.take().ok_or(Error::Protocol)?;
    let mut stdout = child.stdout.take().ok_or(Error::Protocol)?;
    let incoming = input_pump(input, stdin, spec, program);
    tokio::pin!(incoming);
    let outgoing = output_pump(&mut stdout, output);
    tokio::pin!(outgoing);
    let result = tokio::select! {
        result=child.wait()=>{let status=result?;if status.success(){Ok(())}else{Err(Error::Io(format!("GDB exited: {status}")))}},
        result=&mut incoming=>result,
        result=&mut outgoing=>result,
        _=control.read(&mut byte)=>Err(Error::Cancelled),
        _=tokio::time::sleep(Duration::from_secs(3600))=>Err(Error::Limit),
    };
    drop(tree);
    let _ = child.wait().await;
    result
}
#[cfg(target_os = "linux")]
async fn proxy(spec: Spec) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    spec.validate()?;
    let directory = tempfile::Builder::new()
        .prefix("crabcraft-debug-control-")
        .tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = directory.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let json = spec.json()?;
    let mut command = tokio::process::Command::new("/usr/bin/pkexec");
    command
        .arg("--disable-internal-agent")
        .arg(executable)
        .arg("--internal-privileged-debug-helper")
        .arg(json)
        .arg(&socket)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(directory.path())
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let control = tokio::select! {status=child.wait()=>{let _=status?;return Err(Error::Denied);},result=listener.accept()=>result?.0,_=tokio::time::sleep(Duration::from_secs(180))=>{let _=child.start_kill();return Err(Error::Limit);}};
    if control.peer_cred()?.uid() != 0 {
        let _ = child.start_kill();
        return Err(Error::Privilege);
    }
    let status = child.wait().await?;
    drop(control);
    if status.success() {
        Ok(())
    } else {
        Err(Error::Io(format!("privileged debugger exited: {status}")))
    }
}
pub fn internal() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    let flag = args.next()?;
    if flag != "--internal-privileged-debug-proxy" && flag != "--internal-privileged-debug-helper" {
        return None;
    }
    let parsed = (|| {
        let raw = args
            .next()
            .and_then(|s| s.into_string().ok())
            .ok_or(Error::Input)?;
        if raw.len() > MAX_SPEC {
            return Err(Error::Limit);
        }
        let spec = serde_json::from_str::<Spec>(&raw).map_err(|_| Error::Input)?;
        spec.validate()?;
        Ok(spec)
    })();
    let result = match parsed {
        Err(e) => Err(e),
        Ok(spec) => {
            #[cfg(target_os = "linux")]
            {
                if flag == "--internal-privileged-debug-proxy" {
                    if args.next().is_some() {
                        Err(Error::Input)
                    } else {
                        rc_core::Runtime::block_on(proxy(spec))
                    }
                } else {
                    let socket = args.next().map(PathBuf::from);
                    if args.next().is_some() {
                        Err(Error::Input)
                    } else if let Some(socket) = socket {
                        rc_core::Runtime::block_on(async {
                            let control = tokio::net::UnixStream::connect(socket).await?;
                            let expected = std::env::var("PKEXEC_UID")
                                .ok()
                                .and_then(|s| s.parse::<u32>().ok())
                                .ok_or(Error::Privilege)?;
                            if control.peer_cred()?.uid() != expected {
                                return Err(Error::Privilege);
                            }
                            supervise(spec, tokio::io::stdin(), tokio::io::stdout(), control, true)
                                .await
                        })
                    } else {
                        Err(Error::Input)
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = spec;
                Err(Error::Platform)
            }
        }
    };
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    })
}
pub fn adapter(
    spec: &Spec,
    trusted: bool,
    approved: bool,
) -> Result<rc_dap::AdapterCommand, Error> {
    authorize(spec, trusted, approved)?;
    let mut adapter = rc_dap::AdapterCommand::new(
        std::env::current_exe()?.canonicalize()?,
        rc_dap::netcoredbg::Origin::Gdb,
    );
    adapter.prefix = vec!["--internal-privileged-debug-proxy".into(), spec.json()?];
    Ok(adapter)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    fn compile(root: &Path, source: &str) -> PathBuf {
        let file = root.join("main.rs");
        std::fs::write(&file, source).unwrap();
        let executable = root.join("program");
        let output = std::process::Command::new("rustc")
            .arg("-g")
            .arg(&file)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        executable
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn frozen_artifact_is_independent_of_later_rebuilds_and_trust_approval_are_mandatory() {
        let root = tempfile::tempdir().unwrap();
        let program = compile(root.path(), "fn main() {println!(\"reviewed\");}\n");
        let snapshot = Frozen::capture(
            root.path().to_path_buf(),
            program.clone(),
            vec!["argument with spaces".into()],
            &CancelToken::new(),
        )
        .unwrap();
        assert!(matches!(
            authorize(&snapshot.spec, false, true),
            Err(Error::Trust)
        ));
        assert!(matches!(
            authorize(&snapshot.spec, true, false),
            Err(Error::Approval)
        ));
        assert!(authorize(&snapshot.spec, true, true).is_ok());
        let old = std::fs::read(&snapshot.spec.frozen).unwrap();
        compile(root.path(), "fn main() {println!(\"later build\");}\n");
        assert_ne!(std::fs::read(&program).unwrap(), old);
        assert_eq!(
            crate::privileged_run::digest(&std::fs::read(&snapshot.spec.frozen).unwrap()),
            snapshot.spec.sha256
        );
        let mut invalid = snapshot.spec.clone();
        invalid.args = vec!["x".into(); 33];
        assert!(matches!(invalid.validate(), Err(Error::Input)));
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn launch_is_pinned_and_project_configuration_cannot_enable_root_expressions_or_logpoints() {
        let spec = Spec {
            root: PathBuf::from("/project"),
            artifact: PathBuf::from("/project/target/program"),
            frozen: PathBuf::from("/tmp/frozen"),
            sha256: "a".repeat(64),
            args: vec!["literal $(touch marker)".into()],
        };
        let program = Path::new("/root/private/program");
        let mut launched = false;
        let mut launch = serde_json::json!({"seq":1,"type":"request","command":"launch","arguments":{"program":spec.artifact,"env":{"LD_PRELOAD":"/project/evil.so"},"args":["evil"],"initCommands":["shell evil"],"cwd":"/project","stopOnEntry":true}});
        sanitize(&mut launch, &spec, program, &mut launched).unwrap();
        assert_eq!(launch["arguments"]["program"], program.to_str().unwrap());
        assert!(launch.pointer("/arguments/env/LD_PRELOAD").is_none());
        assert!(launch.pointer("/arguments/initCommands").is_none());
        assert_eq!(launch["arguments"]["args"], serde_json::json!(spec.args));
        for command in [
            "evaluate",
            "attach",
            "setVariable",
            "setExpression",
            "restart",
            "writeMemory",
            "customGdbCommand",
        ] {
            let mut request = serde_json::json!({"seq":2,"type":"request","command":command,"arguments":{"context":"repl","expression":"shell evil"}});
            assert!(matches!(
                sanitize(&mut request, &spec, program, &mut launched),
                Err(Error::Protocol)
            ));
        }
        let mut points = serde_json::json!({"seq":3,"type":"request","command":"setBreakpoints","arguments":{"source":{"path":"/project/main.rs"},"breakpoints":[{"line":3,"condition":"system(\"evil\")","logMessage":"{evil()}","hitCondition":"evil()"}]}});
        sanitize(&mut points, &spec, program, &mut launched).unwrap();
        assert_eq!(
            points["arguments"]["breakpoints"],
            serde_json::json!([{"line":3}])
        );
        let mut unsafe_spec = spec.clone();
        unsafe_spec.root = PathBuf::from("/project\nshell touch marker");
        assert!(matches!(unsafe_spec.validate(), Err(Error::Input)));
        let flags = gdb_args();
        assert!(flags.contains(&"-nx"));
        assert!(flags.contains(&"-nh"));
        assert!(flags.contains(&"set auto-load off"));
        assert!(flags.contains(&"set startup-with-shell off"));
    }
    #[tokio::test]
    async fn bounded_dap_frames_reject_duplicate_lengths_and_oversize_input() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        writer
            .write_all(b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}")
            .await
            .unwrap();
        assert!(matches!(frame(&mut reader).await, Err(Error::Protocol)));
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        writer
            .write_all(b"Content-Length: 1048577\r\n\r\n")
            .await
            .unwrap();
        assert!(matches!(frame(&mut reader).await, Err(Error::Limit)));
    }
    /// Runs only as a dedicated unprivileged subprocess of the opt-in GDB tests. No pkexec.
    #[test]
    #[ignore = "private fixture entry point, not a root authorization test"]
    #[cfg(target_os = "linux")]
    fn fixture_helper() {
        let spec: Spec =
            serde_json::from_str(&std::env::var("CRABCRAFT_TEST_DEBUG_SPEC").unwrap()).unwrap();
        let protocol = std::env::var("CRABCRAFT_TEST_DEBUG_PROTOCOL").unwrap();
        let control = std::env::var("CRABCRAFT_TEST_DEBUG_CONTROL").unwrap();
        rc_core::Runtime::block_on(async {
            let protocol = tokio::net::UnixStream::connect(protocol).await.unwrap();
            let (input, output) = protocol.into_split();
            let control = tokio::net::UnixStream::connect(control).await.unwrap();
            let result = supervise(spec, input, output, control, false).await;
            assert!(
                matches!(result, Ok(()) | Err(Error::Cancelled)),
                "{result:?}"
            );
        });
    }
    #[cfg(target_os = "linux")]
    async fn fixture(
        spec: &Spec,
        root: &Path,
    ) -> (
        tokio::process::Child,
        tokio::net::unix::OwnedReadHalf,
        tokio::net::unix::OwnedWriteHalf,
        tokio::net::UnixStream,
    ) {
        let protocol = root.join("protocol.sock");
        let control = root.join("watchdog.sock");
        let listener = tokio::net::UnixListener::bind(&protocol).unwrap();
        let watchdog = tokio::net::UnixListener::bind(&control).unwrap();
        let child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "privileged_debug_backend::tests::fixture_helper",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("CRABCRAFT_TEST_DEBUG_SPEC", spec.json().unwrap())
            .env("CRABCRAFT_TEST_DEBUG_PROTOCOL", &protocol)
            .env("CRABCRAFT_TEST_DEBUG_CONTROL", &control)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (protocol, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let (control, _) = watchdog.accept().await.unwrap();
        let (input, output) = protocol.into_split();
        (child, input, output, control)
    }
    #[cfg(target_os = "linux")]
    async fn until(
        reader: &mut tokio::net::unix::OwnedReadHalf,
        description: &str,
        done: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let message = frame(reader)
                    .await
                    .unwrap()
                    .unwrap_or_else(|| panic!("helper closed while waiting for {description}"));
                if message.get("success") == Some(&serde_json::Value::Bool(false)) {
                    panic!("GDB rejected {description}: {message}");
                }
                if done(&message) {
                    return message;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
    }
    #[tokio::test]
    #[ignore = "requires real system GDB DAP and ptrace; unprivileged fixture only, never pkexec"]
    #[cfg(target_os = "linux")]
    async fn real_gdb_dap_reads_rust_stack_and_separate_watchdog_eof_reaps_inferior() {
        let root = tempfile::tempdir().unwrap();
        let source = "fn main() {\n    let answer = 42u32;\n    println!(\"answer {}\", answer);\n    std::thread::sleep(std::time::Duration::from_secs(60));\n}\n";
        let program = compile(root.path(), source);
        std::fs::write(
            root.path().join(".gdbinit"),
            format!(
                "shell touch {}\n",
                root.path().join("gdbinit-ran").display()
            ),
        )
        .unwrap();
        let snapshot = Frozen::capture(
            root.path().to_path_buf(),
            program,
            Vec::new(),
            &CancelToken::new(),
        )
        .unwrap();
        let (mut child, mut reader, mut writer, control) =
            fixture(&snapshot.spec, root.path()).await;
        send(&mut writer,&serde_json::json!({"seq":1,"type":"request","command":"initialize","arguments":{"adapterID":"gdb","linesStartAt1":true,"columnsStartAt1":true}})).await.unwrap();
        until(&mut reader, "initialize", |v| v["request_seq"] == 1).await;
        // GDB 14/15 launches immediately: configure before launch, as rc-dap does.
        send(&mut writer,&serde_json::json!({"seq":2,"type":"request","command":"setBreakpoints","arguments":{"source":{"path":root.path().join("main.rs")},"breakpoints":[{"line":3}]}})).await.unwrap();
        until(&mut reader, "breakpoint configuration", |v| {
            v["request_seq"] == 2
        })
        .await;
        send(&mut writer,&serde_json::json!({"seq":3,"type":"request","command":"configurationDone","arguments":{}})).await.unwrap();
        until(&mut reader, "configurationDone", |v| v["request_seq"] == 3).await;
        send(&mut writer,&serde_json::json!({"seq":4,"type":"request","command":"launch","arguments":{"program":snapshot.spec.artifact}})).await.unwrap();
        let stopped = until(&mut reader, "Rust breakpoint", |v| v["event"] == "stopped").await;
        let thread = stopped["body"]["threadId"].as_i64().unwrap();
        send(&mut writer,&serde_json::json!({"seq":5,"type":"request","command":"stackTrace","arguments":{"threadId":thread}})).await.unwrap();
        let stack = until(&mut reader, "Rust stack", |v| v["request_seq"] == 5).await;
        assert!(stack.to_string().contains("main.rs"), "{stack}");
        assert!(!root.path().join("gdbinit-ran").exists());
        drop(control);
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success(), "helper failed: {status}");
    }
    #[tokio::test]
    #[ignore = "requires real system GDB DAP and ptrace; verifies different process groups without root"]
    #[cfg(target_os = "linux")]
    async fn real_watchdog_cancel_kills_and_reaps_rust_child_in_a_different_process_group() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("child.pid");
        let source = format!(
            "use std::os::unix::process::CommandExt;\nfn main(){{let child=std::process::Command::new(\"/bin/sleep\").arg(\"60\").process_group(0).spawn().unwrap();std::fs::write({:?},child.id().to_string()).unwrap();std::thread::sleep(std::time::Duration::from_secs(60));}}\n",
            marker
        );
        let program = compile(root.path(), &source);
        let snapshot = Frozen::capture(
            root.path().to_path_buf(),
            program,
            Vec::new(),
            &CancelToken::new(),
        )
        .unwrap();
        let (mut child, mut reader, mut writer, mut control) =
            fixture(&snapshot.spec, root.path()).await;
        send(&mut writer,&serde_json::json!({"seq":1,"type":"request","command":"initialize","arguments":{"adapterID":"gdb"}})).await.unwrap();
        until(&mut reader, "initialize", |v| v["request_seq"] == 1).await;
        send(&mut writer,&serde_json::json!({"seq":2,"type":"request","command":"configurationDone","arguments":{}})).await.unwrap();
        until(&mut reader, "configurationDone", |v| v["request_seq"] == 2).await;
        send(&mut writer,&serde_json::json!({"seq":3,"type":"request","command":"launch","arguments":{"program":snapshot.spec.artifact}})).await.unwrap();
        let read =
            tokio::spawn(async move { while frame(&mut reader).await.ok().flatten().is_some() {} });
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < end,
                "Rust inferior did not start"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pid = std::fs::read_to_string(&marker)
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assert!(Path::new(&format!("/proc/{pid}")).exists());
        control.write_all(&[1]).await.unwrap();
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success());
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "descendant must be killed and reaped, not left as an orphan/zombie"
        );
        read.abort();
    }
}
