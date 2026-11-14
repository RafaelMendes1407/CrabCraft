//! Linux PolicyKit launch of an explicitly reviewed compiled artifact.
//! The privileged helper owns the child and cancels it on control-pipe EOF; user killpg alone cannot stop root children.
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const MAX_BINARY: usize = 64 * 1024 * 1024;
const MAX_SPEC: usize = 64 * 1024;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Error {
    Platform,
    Configuration,
    Changed,
    Trust,
    Confirm,
    Input,
    Io,
    Hash,
    Privilege,
    Denied,
    Cancelled,
    Limit,
    Exit { code: i32, output: String },
}
impl Error {
    fn message(&self) -> String {
        match self{Self::Configuration=>tr("Administrator build review supports Cargo binaries without PTY, parallel runs, stdin files, environment overrides, custom working directories or before-launch tasks; arguments must be nonempty single-line values").into(),Self::Changed=>tr("The run configuration, provider or saved source changed; build and review again").into(),Self::Platform=>tr("Privileged execution is supported only on Linux with PolicyKit").into(),Self::Trust=>tr("Trust this project before running project tools or code").into(),Self::Confirm=>tr("Inspect and confirm the exact compiled program before requesting administrator authorization").into(),Self::Input=>tr("Choose a regular compiled executable inside this project and at most 32 separate arguments (64 KB total)").into(),Self::Io=>tr("Could not inspect or launch the privileged program").into(),Self::Hash=>tr("The compiled executable changed after review; inspect it again").into(),Self::Privilege=>tr("The privileged helper did not receive administrator privileges").into(),Self::Denied=>tr("Administrator authorization was denied, dismissed or unavailable; no password is collected by the IDE").into(),Self::Cancelled=>tr("Privileged program cancelled").into(),Self::Limit=>tr("Privileged operation input, output or time limit exceeded").into(),Self::Exit{code,..}=>trf("Privileged program exited with code {code}",&[("code",code)])}
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    root: PathBuf,
    program: PathBuf,
    args: Vec<String>,
    sha256: String,
}
impl Plan {
    fn validate(&self) -> Result<(), Error> {
        if !self.root.is_absolute()
            || !self.program.is_absolute()
            || !self.program.starts_with(&self.root)
            || self.args.len() > 32
            || self.args.iter().any(|s| s.len() > 4096 || s.contains('\0'))
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::Input);
        }
        let bytes = serde_json::to_vec(self).map_err(|_| Error::Input)?;
        if bytes.len() > MAX_SPEC {
            return Err(Error::Limit);
        }
        Ok(())
    }
    fn preview(&self) -> String {
        let mut parts = vec![self.program.to_string_lossy().into_owned()];
        parts.extend(self.args.clone());
        format!(
            "{}\nSHA-256: {}\n{}: {}",
            parts.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>().join(" "),
            self.sha256,
            tr("Working directory"),
            self.root.display()
        )
    }
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn check(stop: &rc_dotnet::CancelToken) -> Result<(), Error> {
    if stop.is_cancelled() { Err(Error::Cancelled) } else { Ok(()) }
}
pub(crate) fn read_program(path: &Path, stop: &rc_dotnet::CancelToken) -> Result<Vec<u8>, Error> {
    check(stop)?;
    if !std::fs::symlink_metadata(path).map_err(|_| Error::Io)?.is_file() {
        return Err(Error::Input);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path).map_err(|_| Error::Input)?;
    let metadata = file.metadata().map_err(|_| Error::Io)?;
    if !metadata.is_file() || metadata.len() > MAX_BINARY as u64 {
        return Err(Error::Limit);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(Error::Input);
        }
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        check(stop)?;
        let n = file.read(&mut chunk).map_err(|_| Error::Io)?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > MAX_BINARY {
            return Err(Error::Limit);
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    check(stop)?;
    if !bytes.starts_with(b"\x7fELF") {
        return Err(Error::Input);
    }
    Ok(bytes)
}
fn inspect(root: PathBuf, program: PathBuf, args: Vec<String>, stop: &rc_dotnet::CancelToken) -> Result<Plan, Error> {
    check(stop)?;
    let root = root.canonicalize().map_err(|_| Error::Input)?;
    if !program.is_absolute() || program.canonicalize().map_err(|_| Error::Input)? != program || !program.starts_with(&root) {
        return Err(Error::Input);
    }
    let bytes = read_program(&program, stop)?;
    let plan = Plan { root, program, args, sha256: digest(&bytes) };
    plan.validate()?;
    Ok(plan)
}
struct Group(Option<u32>);
impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            rc_dotnet::kill_tree(pid);
        }
    }
}
async fn output<R: tokio::io::AsyncRead + Unpin>(reader: R) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    reader.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes).await.map_err(|_| Error::Io)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(Error::Limit);
    }
    Ok(bytes)
}
// Called only by our early private CLI dispatch after PolicyKit has authorized this executable.
async fn supervise<R: tokio::io::AsyncRead + Unpin>(plan: Plan, mut control: R, require_root: bool) -> Result<i32, Error> {
    plan.validate()?;
    #[cfg(unix)]
    if require_root && unsafe { libc::geteuid() } != 0 {
        return Err(Error::Privilege);
    }
    #[cfg(not(unix))]
    if require_root {
        return Err(Error::Platform);
    }
    let stop = rc_dotnet::CancelToken::new();
    let root = plan.root.canonicalize().map_err(|_| Error::Input)?;
    if root != plan.root || plan.program.canonicalize().map_err(|_| Error::Input)? != plan.program {
        return Err(Error::Input);
    }
    let bytes = read_program(&plan.program, &stop)?;
    if digest(&bytes) != plan.sha256 {
        return Err(Error::Hash);
    }
    let directory = tempfile::Builder::new().prefix("crabcraft-admin-").tempdir().map_err(|_| Error::Io)?;
    let executable = directory.path().join("program");
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o500).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&executable).map_err(|_| Error::Io)?;
        file.write_all(&bytes).map_err(|_| Error::Io)?;
        file.sync_all().map_err(|_| Error::Io)?;
    }
    // A request already cancelled while inspecting/copying must never start the program.
    let mut command_byte = [0u8; 1];
    let pending = tokio::time::timeout(Duration::from_millis(1), control.read(&mut command_byte)).await;
    if pending.is_ok() {
        return Err(Error::Cancelled);
    }
    let mut cmd = tokio::process::Command::new(executable);
    cmd.args(&plan.args)
        .current_dir(&root)
        .env_clear()
        .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|_| Error::Io)?;
    let group = Group(child.id());
    let result = tokio::select! {status=child.wait()=>Ok(status.map_err(|_|Error::Io)?.code().unwrap_or(-1)),_=control.read(&mut command_byte)=>Err(Error::Cancelled),_=tokio::time::sleep(Duration::from_secs(3600))=>Err(Error::Limit)};
    drop(group);
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    result
}
fn control_input() -> Result<tokio::fs::File, Error> {
    #[cfg(unix)]
    {
        use std::os::fd::FromRawFd;
        // SAFETY: fcntl duplicates our existing stdin as a newly owned close-on-exec descriptor.
        let fd = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(Error::Io);
        }
        // SAFETY: the successful fcntl result is a unique descriptor owned by this File.
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        Ok(tokio::fs::File::from_std(file))
    }
    #[cfg(not(unix))]
    {
        Err(Error::Platform)
    }
}
pub fn internal() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--internal-privileged-run")) {
        return None;
    }
    let result = if cfg!(target_os = "linux") {
        args.next().and_then(|s| s.into_string().ok()).filter(|s| s.len() <= MAX_SPEC).ok_or(Error::Input).and_then(|json| {
            if args.next().is_some() {
                return Err(Error::Input);
            }
            let plan: Plan = serde_json::from_str(&json).map_err(|_| Error::Input)?;
            rc_core::Runtime::block_on(supervise(plan, control_input()?, true))
        })
    } else {
        Err(Error::Platform)
    };
    if let Ok(status) = serde_json::to_string(&result) {
        eprintln!("CRABCRAFT_ROOT_RESULT:{status}");
    }
    Some(match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{}", error.message());
            if error == Error::Cancelled { 130 } else { 125 }
        }
    })
}
async fn launch(plan: Plan, trusted: bool, confirmed: bool, stop: rc_dotnet::CancelToken) -> Result<String, Error> {
    if !cfg!(target_os = "linux") {
        return Err(Error::Platform);
    }
    if !trusted {
        return Err(Error::Trust);
    }
    if !confirmed {
        return Err(Error::Confirm);
    }
    plan.validate()?;
    check(&stop)?;
    let json = serde_json::to_string(&plan).map_err(|_| Error::Input)?;
    let helper = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|_| Error::Io)?;
    let neutral = tempfile::tempdir().map_err(|_| Error::Io)?;
    let mut cmd = tokio::process::Command::new("/usr/bin/pkexec");
    cmd.args([
        std::ffi::OsStr::new("--disable-internal-agent"),
        helper.as_os_str(),
        std::ffi::OsStr::new("--internal-privileged-run"),
        std::ffi::OsStr::new(&json),
    ])
    .current_dir(neutral.path())
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|_| Error::Io)?;
    let group = Group(child.id());
    let mut stdin = child.stdin.take().ok_or(Error::Io)?;
    let stdout = child.stdout.take().ok_or(Error::Io)?;
    let stderr = child.stderr.take().ok_or(Error::Io)?;
    let work = async {
        let (out, err) = tokio::try_join!(output(stdout), output(stderr))?;
        let status = child.wait().await.map_err(|_| Error::Io)?;
        let code = status.code().unwrap_or(-1);
        let errors = String::from_utf8_lossy(&err);
        let result = errors
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("CRABCRAFT_ROOT_RESULT:").and_then(|value| serde_json::from_str::<Result<i32, Error>>(value).ok()));
        let result = match result {
            Some(result) => result,
            None => {
                return Err(if matches!(code, 126 | 127) { Error::Denied } else { Error::Io });
            }
        };
        let program_code = result?;
        let mut text = String::from_utf8_lossy(&out).into_owned();
        let visible = errors.lines().filter(|line| !line.starts_with("CRABCRAFT_ROOT_RESULT:")).collect::<Vec<_>>().join("\n");
        if !visible.is_empty() {
            text.push('\n');
            text.extend(visible.chars().take(16384));
        }
        if program_code != 0 {
            return Err(Error::Exit { code: program_code, output: text.chars().take(16384).collect() });
        }
        Ok(text)
    };
    let result = tokio::select! {result=tokio::time::timeout(Duration::from_secs(3660),work)=>result.map_err(|_|Error::Limit)?,()=stop.cancelled()=>Err(Error::Cancelled)};
    // EOF is the authoritative root-owned watchdog signal, including output-limit/error paths.
    let _ = tokio::time::timeout(Duration::from_millis(100), stdin.write_all(b"cancel\n")).await;
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
    drop(group);
    result
}

#[derive(Clone, PartialEq, Eq)]
struct RunSource {
    root: PathBuf,
    config: crate::runner::RunConfig,
    options: Option<rc_core::session::RunOptions>,
    cargo: rc_cargo::Cargo,
    epoch: u64,
    docs: Vec<(u64, PathBuf, u64, bool)>,
}
impl RunSource {
    fn capture(app: &crate::app::CrabCraftApp) -> Result<Self, Error> {
        if app.docs.list.len() > 256 {
            return Err(Error::Limit);
        }
        let root = app.workspace.as_ref().map(|workspace| workspace.ws.root.clone()).ok_or(Error::Input)?;
        if !app.settings.trusted_projects.contains(&root) {
            return Err(Error::Trust);
        }
        let config = app.runner.selected_config().cloned().ok_or(Error::Configuration)?;
        if !matches!(config.kind, crate::runner::ConfigKind::Cargo { .. }) {
            return Err(Error::Configuration);
        }
        let options = app.runner.options.get(&config.name).cloned();
        if options.as_ref().is_some_and(|options| {
            options.emulate_terminal
                || options.allow_parallel
                || !options.env.trim().is_empty()
                || !options.stdin_file.trim().is_empty()
                || !options.working_dir.trim().is_empty()
                || !options.before_launch.is_empty()
        }) {
            return Err(Error::Configuration);
        }
        let cargo = app.runner.cargo_tool().map_err(|_| Error::Input)?;
        let docs = app
            .docs
            .list
            .iter()
            .filter(|doc| !doc.generated)
            .map(|doc| (doc.editor.id, doc.path.clone(), doc.editor.buffer.version(), doc.is_modified()))
            .collect();
        Ok(Self { root, config, options, cargo, epoch: app.cfg_epoch, docs })
    }
    fn same_configuration(&self, other: &Self) -> bool {
        self.root == other.root && self.config == other.config && self.options == other.options && self.cargo == other.cargo && self.epoch == other.epoch
    }
}
enum Event {
    Inspected(u64, Result<Plan, Error>),
    Finished(u64, Result<String, Error>),
}
pub struct PrivilegedRun {
    generation: u64,
    source: Option<RunSource>,
    pub program: String,
    pub args: String,
    inbox: Inbox<Event>,
    plan: Option<Plan>,
    confirm: bool,
    stop: Option<rc_dotnet::CancelToken>,
    running: bool,
    error: Option<String>,
    output: String,
}
impl PrivilegedRun {
    pub fn new(waker: Waker) -> Self {
        Self {
            generation: 0,
            source: None,
            program: String::new(),
            args: String::new(),
            inbox: Inbox::new(waker),
            plan: None,
            confirm: false,
            stop: None,
            running: false,
            error: None,
            output: String::new(),
        }
    }
    pub fn cancel(&mut self) {
        self.plan = None;
        self.source = None;
        self.confirm = false;
        if self.running {
            if let Some(stop) = &self.stop {
                stop.cancel();
            }
            return;
        }
        if let Some(stop) = self.stop.take() {
            stop.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
    }
    fn inspect(&mut self, root: PathBuf) {
        self.cancel();
        let generation = self.generation;
        let program = PathBuf::from(&self.program);
        let args = self.args.lines().map(str::to_string).collect();
        let stop = rc_dotnet::CancelToken::new();
        self.stop = Some(stop.clone());
        self.running = false;
        self.plan = None;
        self.confirm = false;
        self.error = None;
        let out = self.inbox.sender();
        rc_core::Runtime::spawn(async move {
            let result = tokio::task::spawn_blocking(move || inspect(root, program, args, &stop)).await.unwrap_or(Err(Error::Io));
            out.send(Event::Inspected(generation, result));
        });
    }
    fn start(&mut self, root: &Path, trusted: bool) {
        let Some(plan) = self.plan.take() else {
            return;
        };
        if plan.root != root || plan.program != PathBuf::from(&self.program) || plan.args != self.args.lines().map(str::to_string).collect::<Vec<_>>() {
            self.error = Some(Error::Hash.message());
            return;
        }
        self.source = None;
        let generation = self.generation;
        let confirmed = self.confirm;
        self.confirm = false;
        self.running = true;
        let stop = rc_dotnet::CancelToken::new();
        self.stop = Some(stop.clone());
        let out = self.inbox.sender();
        rc_core::Runtime::spawn(async move {
            out.send(Event::Finished(generation, launch(plan, trusted, confirmed, stop).await));
        });
    }
    pub fn poll(&mut self, trusted: bool) {
        if !trusted && self.source.is_some() {
            self.cancel();
        }
        if !trusted {
            if self.running
                && let Some(stop) = &self.stop
            {
                stop.cancel();
            }
            self.confirm = false;
        }
        for event in self.inbox.drain() {
            let generation = match &event {
                Event::Inspected(generation, _) | Event::Finished(generation, _) => *generation,
            };
            if generation != self.generation {
                continue;
            }
            let cancelled = self.stop.as_ref().is_some_and(|stop| stop.is_cancelled());
            self.stop = None;
            self.running = false;
            if cancelled {
                self.plan = None;
                self.confirm = false;
                self.source = None;
                self.error = Some(Error::Cancelled.message());
                continue;
            }
            let result = match event {
                Event::Inspected(_, result) => result.map(|plan| {
                    if self.source.is_some() {
                        self.program = plan.program.to_string_lossy().into_owned();
                        self.args = plan.args.join("\n");
                    }
                    self.plan = Some(plan)
                }),
                Event::Finished(_, result) => result.map(|output| self.output = output.chars().take(16384).collect()),
            };
            if let Err(error) = result {
                if let Error::Exit { output, .. } = &error {
                    self.output = output.clone();
                }
                self.error = Some(error.message());
            }
        }
    }
    fn show(&mut self, ui: &mut egui::Ui, root: Option<PathBuf>, trusted: bool) {
        egui::ScrollArea::vertical().id_salt("privileged_run").show(ui,|ui|{
ui.colored_label(rc_kit::pal(ui).warn,tr("Administrator execution gives this program full system privileges. Build it as your normal user first and inspect the exact executable and arguments before authorizing."));ui.label(tr("Only the operating-system PolicyKit agent requests credentials. CrabCraft never asks for or stores an administrator password, changes authorization policy, or elevates on project open."));ui.label(tr("This launches a prebuilt console executable only. Interactive stdin, GUI programs, project environment overrides and automatic rerun are unsupported."));ui.label(tr("The OS dialog authorizes CrabCraft's supervisor for the reviewed program. A private verified copy runs with the project as working directory; files it creates may be owned by root."));if !cfg!(target_os="linux"){ui.colored_label(rc_kit::pal(ui).warn,Error::Platform.message());}if !trusted{ui.colored_label(rc_kit::pal(ui).warn,tr("Trust this project before running project tools or code"));}
ui.add_enabled_ui(self.stop.is_none(),|ui|{ui.label(tr("Compiled executable inside this project"));let changed=ui.add(egui::TextEdit::singleline(&mut self.program).char_limit(4096)).changed();ui.label(tr("Administrator program arguments (one per line; no shell)"));let changed=ui.add(egui::TextEdit::multiline(&mut self.args).char_limit(MAX_SPEC).desired_rows(4)).changed()||changed;if changed{self.plan=None;self.source=None;self.confirm=false;}});
if ui.add_enabled(cfg!(target_os="linux")&&root.is_some()&&self.stop.is_none()&&!self.program.is_empty(),egui::Button::new(tr("Inspect Compiled Program"))).clicked(){if let Some(root)=root.clone(){self.inspect(root);}}
if let Some(plan)=&self.plan{let mut preview=plan.preview();ui.add(egui::TextEdit::multiline(&mut preview).interactive(false).font(egui::TextStyle::Monospace));ui.checkbox(&mut self.confirm,tr("I reviewed this exact program and authorize requesting administrator privileges"));if ui.add_enabled(cfg!(target_os="linux")&&trusted&&self.confirm&&self.stop.is_none(),egui::Button::new(tr("Request Administrator Execution"))).clicked(){if let Some(root)=&root{self.start(root,trusted);}}}
if self.stop.is_some(){ui.spinner();if ui.button(tr("Cancel Administrator Operation")).clicked(){self.cancel();}}if let Some(error)=&self.error{ui.colored_label(rc_kit::pal(ui).error,error);}ui.label(tr("Administrator program output preview is limited to 16,384 characters"));ui.add(egui::TextEdit::multiline(&mut self.output).interactive(false).font(egui::TextStyle::Monospace).desired_rows(12));});
    }
}
impl PrivilegedRun {
    fn build_configuration(&mut self, source: RunSource, target: rc_cargo::RunTarget) {
        self.cancel();
        self.error = None;
        self.output.clear();
        let generation = self.generation;
        let stop = rc_dotnet::CancelToken::new();
        self.stop = Some(stop.clone());
        self.source = Some(source.clone());
        self.running = false;
        let out = self.inbox.sender();
        rc_core::Runtime::spawn(async move {
            let result = async {
                if target.args.iter().any(|arg| arg.is_empty() || arg.len() > 4096 || arg.contains(['\0', '\r', '\n']))
                    || target.args.len() > 32
                    || target.test.is_some()
                    || !target.before.is_empty()
                    || !target.env.is_empty()
                    || target.working_dir.is_some()
                    || target.options.stdin_file.is_some()
                {
                    return Err(Error::Configuration);
                }
                let root = source.root.clone();
                let manifest = target.manifest.clone();
                let token = stop.clone();
                tokio::task::spawn_blocking(move || {
                    check(&token)?;
                    let root = root.canonicalize().map_err(|_| Error::Input)?;
                    let manifest = manifest.canonicalize().map_err(|_| Error::Input)?;
                    if !manifest.starts_with(root) {
                        return Err(Error::Input);
                    }
                    Ok(())
                })
                .await
                .map_err(|_| Error::Io)??;
                let launch = rc_cargo::prepare(&source.cargo, &target, &stop, &mut |_| {}).await.map_err(|_| Error::Io)?.ok_or(Error::Cancelled)?;
                check(&stop)?;
                let token = stop.clone();
                tokio::task::spawn_blocking(move || inspect(source.root, launch.program, launch.args, &token)).await.map_err(|_| Error::Io)?
            }
            .await;
            out.send(Event::Inspected(generation, result));
        });
    }
}
impl crate::app::CrabCraftApp {
    fn prepare_privileged_run(&mut self) {
        let source = match RunSource::capture(self) {
            Ok(source) => source,
            Err(error) => {
                self.notify(error.message(), true);
                return;
            }
        };
        let expected = source.clone();
        if self.defer_save(true, move |app| app.prepare_privileged_run_saved(expected)) {
            return;
        }
        self.prepare_privileged_run_saved(source);
    }
    fn prepare_privileged_run_saved(&mut self, expected: RunSource) {
        let result = (|| {
            if !cfg!(target_os = "linux") {
                return Err(Error::Platform);
            }
            let source = RunSource::capture(self)?;
            if !source.same_configuration(&expected) || source.docs.iter().any(|(_, _, _, modified)| *modified) {
                return Err(Error::Changed);
            }
            let (cargo, target) = self.runner.cargo_debug_target(&source.config).ok_or(Error::Configuration)?.map_err(|_| Error::Input)?;
            if cargo != source.cargo {
                return Err(Error::Changed);
            }
            Ok((source, target))
        })();
        match result {
            Ok((source, target)) => self.privileged_run.build_configuration(source, target),
            Err(error) => self.notify(error.message(), true),
        }
    }
    fn validate_privileged_run_review(&mut self) {
        let Some(old) = self.privileged_run.source.as_ref() else {
            return;
        };
        if RunSource::capture(self).as_ref().ok() != Some(old) {
            self.privileged_run.cancel();
            self.privileged_run.error = Some(Error::Changed.message());
        }
    }
}
impl Drop for PrivilegedRun {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.cancel();
        }
    }
}
pub fn show(app: &mut crate::app::CrabCraftApp, ui: &mut egui::Ui) {
    if crate::ui::toolwindows::header(ui, tr("Administrator Run")) {
        app.toggle_tool(crate::toolwindow::ToolWindow::PrivilegedRun);
    }
    let root = app.workspace.as_ref().map(|w| w.ws.root.clone());
    let trusted = root.as_ref().is_some_and(|r| app.settings.trusted_projects.contains(r));
    ui.horizontal(|ui| {
        ui.selectable_value(&mut app.privileged_debug.show_debug, false, tr("Administrator Run"));
        ui.selectable_value(&mut app.privileged_debug.show_debug, true, tr("Administrator Debug"));
    });
    if app.privileged_debug.show_debug {
        egui::ScrollArea::vertical().id_salt("privileged_debug").show(ui, |ui| crate::privileged_debug::show(app, ui));
    } else {
        app.validate_privileged_run_review();
        ui.label(tr("Build the selected Cargo configuration as your normal user, then review its exact artifact and arguments; no authorization is requested by preparing it"));
        let selected = app.runner.selected_config().map(|config| config.name.clone()).unwrap_or_default();
        ui.label(selected);
        let prepare = ui
            .add_enabled(
                cfg!(target_os = "linux") && trusted && app.privileged_run.stop.is_none(),
                egui::Button::new(tr("Build and Review Selected Administrator Run")),
            )
            .clicked();
        if prepare {
            app.prepare_privileged_run();
        }
        app.privileged_run.show(ui, root, trusted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path) -> PathBuf {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"privileged_run_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n").unwrap();
        let source = "fn main(){std::fs::write(\"privileged-ran\",std::process::id().to_string()).unwrap();if std::env::args().any(|s|s==\"wait\"){std::thread::sleep(std::time::Duration::from_secs(30));std::fs::write(\"unexpected-finish\",\"yes\").unwrap();}}";
        std::fs::write(root.join("src/main.rs"), source).unwrap();
        std::fs::write(root.join("build.rs"), "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}").unwrap();
        let program = root.join("fixture");
        let output = std::process::Command::new("rustc").arg(root.join("src/main.rs")).arg("-o").arg(&program).current_dir(root).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        program
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn root_helper_watchdog_stops_a_real_compiled_rust_child_on_control_eof() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let program = fixture(&root);
        let plan = inspect(root.clone(), program, vec!["wait".into()], &rc_dotnet::CancelToken::new()).unwrap();
        let (controller, control) = tokio::io::duplex(64);
        let task = tokio::spawn(supervise(plan, control, false));
        for _ in 0..300 {
            if root.join("privileged-ran").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(root.join("privileged-ran").exists());
        let pid: usize = std::fs::read_to_string(root.join("privileged-ran")).unwrap().parse().unwrap();
        drop(controller);
        assert_eq!(tokio::time::timeout(Duration::from_secs(3), task).await.unwrap().unwrap(), Err(Error::Cancelled));
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
        assert!(!root.join("unexpected-finish").exists());
        assert!(!root.join("build-ran").exists());
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn administrator_backend_enforces_trust_confirmation_hash_privilege_and_early_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let program = fixture(&root);
        let plan = inspect(root.clone(), program.clone(), vec![], &rc_dotnet::CancelToken::new()).unwrap();
        assert_eq!(launch(plan.clone(), false, true, rc_dotnet::CancelToken::new()).await, Err(Error::Trust));
        assert_eq!(launch(plan.clone(), true, false, rc_dotnet::CancelToken::new()).await, Err(Error::Confirm));
        assert!(!root.join("privileged-ran").exists());
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(supervise(plan.clone(), tokio::io::empty(), true).await, Err(Error::Privilege));
        }
        assert_eq!(supervise(plan.clone(), tokio::io::empty(), false).await, Err(Error::Cancelled));
        assert!(!root.join("privileged-ran").exists());
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).open(program).unwrap().write_all(b"changed").unwrap();
        assert_eq!(supervise(plan, tokio::io::empty(), false).await, Err(Error::Hash));
        assert!(!root.join("privileged-ran").exists());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn selected_cargo_configuration_builds_after_save_and_stops_at_unarmed_review() {
        use egui_kittest::kittest::{NodeT, Queryable};
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname=\"administrator_config_evidence\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n[features]\nreviewed=[]\n",
        )
        .unwrap();
        let path = root.path().join("src/main.rs");
        let original = "#[cfg(not(feature=\"reviewed\"))] compile_error!(\"missing reviewed feature\");\nfn main(){std::fs::write(\"artifact-ran\",\"unexpected\").unwrap();let value=1;println!(\"{}\",value);}";
        std::fs::write(&path, original).unwrap();
        let wrapper = tools.path().join("configured-cargo");
        let log = tools.path().join("cargo-invocations");
        let cargo = rc_cargo::Cargo::locate().unwrap();
        let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
        std::fs::write(&wrapper, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexec {} \"$@\"\n", quote(&log), quote(cargo.path()))).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(root.path());
        app.open_file_and_wait(&path);
        app.dock.show(crate::toolwindow::ToolWindow::PrivilegedRun);
        let mut h = crate::screens::harness(app, egui::vec2(1500.0, 1200.0));
        h.run_steps(2);
        assert!(h.get_by_label(tr("Build and Review Selected Administrator Run")).accesskit_node().is_disabled());
        assert!(!log.exists());
        h.state_mut().trust_project_tools_for_test();
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while h.state().runner.cargo_workspace.is_none() {
            h.run_steps(2);
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        let config = h.state().runner.selected_config().unwrap().clone();
        h.state_mut().runner.set_cargo_tool(Some(rc_cargo::Cargo::new(&wrapper)));
        h.state_mut().runner.options.entry(config.name.clone()).or_default().args = "--message \"two words\"".into();
        {
            let options = h.state_mut().runner.options.get_mut(&config.name).unwrap();
            options.features = "reviewed".into();
            options.cargo_profile = "release".into();
        }
        let at = original.find("value=1").unwrap() + "value=".len();
        h.state_mut().docs.active_doc_mut().unwrap().editor.replace(at, at + 1, "42");
        assert!(h.state().docs.active_doc().unwrap().is_modified());
        h.get_by_label(tr("Build and Review Selected Administrator Run")).click();
        loop {
            h.run_steps(2);
            if h.state().privileged_run.plan.is_some() {
                break;
            }
            assert!(std::time::Instant::now() < until, "{:?}", h.state().privileged_run.error);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!h.state().docs.active_doc().unwrap().is_modified());
        assert!(std::fs::read_to_string(&path).unwrap().contains("value=42"));
        assert!(std::fs::read_to_string(&log).unwrap().contains("build"));
        let invoked = std::fs::read_to_string(&log).unwrap();
        assert!(invoked.contains("reviewed") && invoked.contains("release"));
        let plan = h.state().privileged_run.plan.as_ref().unwrap();
        assert_eq!(plan.args, ["--message", "two words"]);
        assert!(plan.program.starts_with(root.path()));
        assert!(!h.state().privileged_run.confirm);
        assert!(h.get_by_label(tr("Request Administrator Execution")).accesskit_node().is_disabled());
        assert!(!root.path().join("artifact-ran").exists());
        h.state_mut().runner.options.get_mut(&config.name).unwrap().args = "changed".into();
        h.run_steps(2);
        assert!(h.state().privileged_run.plan.is_none());
        h.state_mut().runner.options.get_mut(&config.name).unwrap().emulate_terminal = true;
        assert!(matches!(RunSource::capture(h.state()), Err(Error::Configuration)));
        h.state_mut().runner.options.get_mut(&config.name).unwrap().emulate_terminal = false;
        h.get_by_label(tr("Build and Review Selected Administrator Run")).click();
        h.run_steps(2);
        h.state_mut().change_cargo_provider(crate::criticalup_provider::Change::Default);
        h.run_steps(5);
        assert!(h.state().privileged_run.source.is_none());
        assert!(h.state().privileged_run.plan.is_none());
        assert!(!root.path().join("artifact-ran").exists());
    }
    #[test]
    fn cancelled_inspection_cannot_reactivate_a_late_review() {
        let mut state = PrivilegedRun::new(Waker::noop());
        let old = state.generation;
        state.cancel();
        state.inbox.sender().send(Event::Inspected(
            old,
            Ok(Plan { root: PathBuf::from("/tmp/project"), program: PathBuf::from("/tmp/project/program"), args: Vec::new(), sha256: "0".repeat(64) }),
        ));
        state.poll(true);
        assert!(state.plan.is_none());
        assert!(!state.confirm);
    }
    #[test]
    fn compiled_program_inspection_rejects_scripts_symlinks_traversal_and_unknown_helper_fields() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let program = fixture(&root);
        let stop = rc_dotnet::CancelToken::new();
        let plan = inspect(root.clone(), program.clone(), vec![], &stop).unwrap();
        let mut value = serde_json::to_value(&plan).unwrap();
        value["allow_non_root"] = true.into();
        assert!(serde_json::from_value::<Plan>(value).is_err());
        symlink(&program, root.join("alias")).unwrap();
        assert!(matches!(inspect(root.clone(), root.join("alias"), vec![], &stop), Err(Error::Input)));
        std::fs::write(root.join("script"), "#!/bin/sh\ntouch unexpected\n").unwrap();
        std::fs::set_permissions(root.join("script"), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(inspect(root.clone(), root.join("script"), vec![], &stop), Err(Error::Input)));
        assert!(matches!(inspect(root.clone(), PathBuf::from("/bin/sh"), vec![], &stop), Err(Error::Input)));
        let mut invalid = plan;
        invalid.args = vec!["x".repeat(4097)];
        assert_eq!(invalid.validate(), Err(Error::Input));
        stop.cancel();
        assert!(matches!(inspect(root, program, vec![], &stop), Err(Error::Cancelled)));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn native_administrator_preview_never_requests_os_credentials_before_explicit_confirmation() {
        use egui_kittest::kittest::Queryable;
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let program = fixture(&root);
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(&root);
        app.privileged_run.program = program.to_string_lossy().into();
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::PrivilegedRun, 900.0);
        let mut h = crate::screens::harness(app, egui::vec2(1900.0, 1500.0));
        h.get_by_label(tr("Administrator Run")).click();
        h.run_steps(2);
        h.get_by_label(tr("Inspect Compiled Program")).click();
        h.run_steps(2);
        let start = std::time::Instant::now();
        while h.state().privileged_run.stop.is_some() && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(h.state().privileged_run.plan.is_some(), "{:?}", h.state().privileged_run.error);
        assert!(!root.join("privileged-ran").exists());
        h.get_by_label(tr("Request Administrator Execution")).click();
        h.run_steps(2);
        assert!(!h.state().privileged_run.running);
        h.state_mut().trust_project_tools_for_test();
        h.run_steps(2);
        h.get_by_label(tr("Request Administrator Execution")).click();
        h.run_steps(2);
        assert!(!h.state().privileged_run.running);
        assert!(!root.join("privileged-ran").exists());
        h.get_by_label(tr("I reviewed this exact program and authorize requesting administrator privileges")).click();
        h.run_steps(2);
        assert!(h.state().privileged_run.confirm);
        assert!(!h.state().privileged_run.running);
        assert!(!root.join("build-ran").exists()); /* Intentionally stop before the enabled authorization button: tests must never request real OS credentials. */
    }
}
