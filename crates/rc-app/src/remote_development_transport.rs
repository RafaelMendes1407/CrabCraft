//! Trusted Linux workspace transports. Only explicit session actions launch SSH or WSL.
use crate::remote_transfer::{self, Config};
use rc_core::{Inbox, Runtime, Waker};
use rc_dotnet::process::{CancelToken, Stream};
use rc_lsp::{
    lsp_types,
    rpc::{Client, Incoming},
};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};
const OUTPUT: usize = 2 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub ssh: Config,
    pub wsl: Option<crate::wsl_workspace::Config>,
    pub container: Option<crate::container_workspace::Config>,
    pub root: String,
    pub cargo: String,
    pub analyzer: String,
}
#[derive(Clone, Debug)]
pub enum Error {
    Input,
    Limit,
    Cancelled,
    Timeout,
    Io(String),
    Process(String),
    Analysis,
    Stale,
    Unsupported,
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string().chars().take(2048).collect())
    }
}
impl From<remote_transfer::Error> for Error {
    fn from(e: remote_transfer::Error) -> Self {
        match e {
            remote_transfer::Error::Input => Self::Input,
            remote_transfer::Error::Limit => Self::Limit,
            remote_transfer::Error::Cancelled => Self::Cancelled,
            remote_transfer::Error::Timeout => Self::Timeout,
            remote_transfer::Error::Stale => Self::Stale,
            remote_transfer::Error::Io(e) => Self::Io(e),
            remote_transfer::Error::Process(e) => Self::Process(e),
        }
    }
}
pub fn absolute(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 4096
        && !value.chars().any(char::is_control)
        && !value.split('/').any(|p| p == ".." || p == ".")
        && !value.contains('\0')
}
pub fn quote(value: &str) -> Result<String, Error> {
    if !absolute(value) {
        return Err(Error::Input);
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}
impl Settings {
    pub fn validate(&self) -> Result<(), Error> {
        if self.wsl.is_some() && self.container.is_some() {
            return Err(Error::Input);
        }
        if let Some(container) = &self.container {
            container.validate()?;
            if container.root != self.root {
                return Err(Error::Input);
            }
        }
        if let Some(wsl) = &self.wsl {
            wsl.validate()?;
        }
        for p in [&self.root, &self.cargo, &self.analyzer] {
            quote(p)?;
        }
        if self.root == "/" || self.root.ends_with('/') || self.cargo.ends_with('/') || self.analyzer.ends_with('/') {
            return Err(Error::Input);
        }
        Ok(())
    }
    pub fn file(&self, value: &str) -> Result<String, Error> {
        if value.len() > 4096 {
            return Err(Error::Input);
        }
        let path = if value.starts_with('/') { value.to_owned() } else { format!("{}/{value}", self.root) };
        if !absolute(&path) || !path.starts_with(&format!("{}/", self.root)) || path.ends_with('/') || path.chars().any(|c| "\"\\*?[]{}".contains(c)) {
            return Err(Error::Input);
        }
        Ok(path)
    }
    fn environment(&self) -> Result<String, Error> {
        let parent = Path::new(&self.cargo).parent().and_then(Path::to_str).ok_or(Error::Input)?;
        let path = format!("{parent}:/usr/bin:/bin");
        let compiler = if self.wsl.is_some() || self.container.is_some() {
            format!("export RUSTC={} RUSTDOC={} && ", quote(&format!("{parent}/rustc"))?, quote(&format!("{parent}/rustdoc"))?)
        } else {
            String::new()
        };
        Ok(format!("cd -- {} && export PATH={} && {compiler}", quote(&self.root)?, quote(&path)?))
    }
}
fn execution(settings: &Settings, dir: &Path, script: String) -> Result<(std::path::PathBuf, Vec<String>, Vec<(String, String)>), Error> {
    settings.validate()?;
    if let Some(container) = &settings.container {
        let spec = container.script(script)?;
        return Ok((spec.program, spec.args, vec![]));
    }
    if let Some(wsl) = &settings.wsl {
        let spec = wsl.script(script)?;
        return Ok((spec.program, spec.args, vec![("WSLENV".into(), String::new())]));
    }
    let mut args = remote_transfer::args(&settings.ssh, dir)?;
    args.extend(["-p".into(), settings.ssh.port.to_string(), "-T".into(), format!("{}@{}", settings.ssh.user, settings.ssh.host), script]);
    Ok(("ssh".into(), args, vec![("SSH_ASKPASS_REQUIRE".into(), "never".into())]))
}
fn command(settings: &Settings, dir: &Path, script: String) -> Result<tokio::process::Command, Error> {
    let (program, args, env) = execution(settings, dir, script)?;
    let mut command = tokio::process::Command::new(program);
    crate::container_workspace::clean(&mut command);
    command.args(args).envs(env).current_dir(dir).env_remove("SSH_ASKPASS");
    Ok(command)
}
/// Holds SSH credential snapshots when used; dropping the session stops its transport process.
pub struct Session {
    pub settings: Settings,
    pub client: Client,
    pub incoming: Inbox<Incoming>,
    pub sources: Vec<String>,
    _credentials: tempfile::TempDir,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.client.kill();
    }
}
async fn collect(
    settings: &Settings,
    script: String,
    cancel: &CancelToken,
    timeout: Duration,
    on_line: &mut (dyn FnMut(Stream, &str) + Send),
    keep_stdin: bool,
) -> Result<(i32, String), Error> {
    use tokio::io::AsyncReadExt;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let dir = tempfile::tempdir()?;
    let mut cmd = command(settings, dir.path(), script)?;
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(if keep_stdin { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn()?;
    let pid = child.id().ok_or(Error::Io("SSH process did not start".into()))?;
    let _stdin = child.stdin.take();
    let stdout = child.stdout.take().ok_or(Error::Input)?;
    let stderr = child.stderr.take().ok_or(Error::Input)?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(usize, Vec<u8>)>(8);
    async fn pump(mut r: impl tokio::io::AsyncRead + Unpin, stream: usize, tx: tokio::sync::mpsc::Sender<(usize, Vec<u8>)>) {
        let mut bytes = [0u8; 8192];
        loop {
            match r.read(&mut bytes).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send((stream, bytes[..n].to_vec())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
    let tasks = [tokio::spawn(pump(stdout, 0, tx.clone())), tokio::spawn(pump(stderr, 1, tx.clone()))];
    drop(tx);
    let start = Instant::now();
    let mut out = String::new();
    let mut pending = [Vec::new(), Vec::new()];
    let mut bytes = 0usize;
    let mut status = None;
    let result=async {loop{tokio::select!{exit=child.wait(),if status.is_none()=>{status=Some(exit?);},chunk=rx.recv()=>{match chunk{Some((stream,data))=>{bytes=bytes.saturating_add(data.len());if bytes>OUTPUT{return Err(Error::Limit);}pending[stream].extend(data);while let Some(end)=pending[stream].iter().position(|b|*b==b'\n'){let raw:Vec<u8>=pending[stream].drain(..=end).collect();let line=String::from_utf8_lossy(&raw);let line=line.trim_end_matches(['\r','\n']);on_line(if stream==0{Stream::Stdout}else{Stream::Stderr},line);if stream==0{out.push_str(line);out.push('\n');}}if pending[stream].len()>128*1024{return Err(Error::Limit);}},None=>{let exit=if let Some(status)=status{status}else{child.wait().await?};for(stream,raw)in pending.iter().enumerate(){if !raw.is_empty(){let line=String::from_utf8_lossy(raw);on_line(if stream==0{Stream::Stdout}else{Stream::Stderr},&line);if stream==0{out.push_str(&line);}}}return Ok((exit.code().unwrap_or(-1),out));}}},()=tokio::time::sleep(Duration::from_millis(10))=>{if cancel.is_cancelled(){return Err(Error::Cancelled);}if start.elapsed()>timeout{return Err(Error::Timeout);}}}}}.await;
    if result.is_err() {
        rc_dotnet::process::kill_tree(pid);
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    }
    for task in tasks {
        task.abort();
    }
    result
}

fn wait<T>(mut request: rc_lsp::rpc::Pending<T>, cancel: &CancelToken, deadline: Instant) -> Result<T, Error> {
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if Instant::now() > deadline {
            return Err(Error::Timeout);
        }
        if let Some(reply) = request.poll() {
            return reply.map_err(|_| Error::Analysis);
        }
        std::thread::sleep(Duration::from_millis(3));
    }
}
pub fn start(settings: Settings, waker: Waker, cancel: CancelToken) -> Result<Session, Error> {
    settings.validate()?;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let mut noop = |_, _: &str| {};
    if settings.wsl.is_some() || settings.container.is_some() {
        let parent = Path::new(&settings.cargo).parent().and_then(Path::to_str).ok_or(Error::Input)?;
        let script = format!(
            "{}{} --version && {} --version && {} --version && /usr/bin/python3 -I --version",
            settings.environment()?,
            quote(&settings.cargo)?,
            quote(&format!("{parent}/rustc"))?,
            quote(&settings.analyzer)?
        );
        let (code, output) = Runtime::block_on(collect(&settings, script, &cancel, Duration::from_secs(15), &mut noop, false))?;
        let lines: Vec<_> = output.lines().collect();
        if code != 0
            || lines.len() != 4
            || !lines[0].starts_with("cargo ")
            || !lines[1].starts_with("rustc ")
            || !lines[2].starts_with("rust-analyzer ")
            || !lines[3].starts_with("Python 3.")
        {
            return Err(Error::Process(output));
        }
    }
    let script = format!(
        "{}test -f Cargo.toml && test -x {} && test -x {} && find . -type d \\( -name target -o -name .git -o -name node_modules \\) -prune -o -type f \\( -name '*.rs' -o -name Cargo.toml \\) -print",
        settings.environment()?,
        quote(&settings.cargo)?,
        quote(&settings.analyzer)?
    );
    let (code, list) = Runtime::block_on(collect(&settings, script, &cancel, Duration::from_secs(15), &mut noop, false))?;
    if code != 0 {
        return Err(Error::Process(list));
    }
    let mut sources = Vec::new();
    for line in list.lines() {
        let relative = line.strip_prefix("./").ok_or(Error::Input)?;
        let path = settings.file(relative)?;
        if path.len() > 4096 || sources.len() >= 512 {
            return Err(Error::Limit);
        }
        sources.push(relative.to_owned());
    }
    sources.sort();
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let credentials = tempfile::tempdir()?;
    let script = format!("{}exec /bin/bash -c {}", settings.environment()?, shell_quote(&analyzer_script(&settings)?));
    let (program, args, env) = execution(&settings, credentials.path(), script)?;
    let incoming = Inbox::new(waker);
    let client = {
        let _entered = Runtime::handle().enter();
        Client::spawn_with_env_removals(&program, &args, credentials.path(), &env, crate::container_workspace::HOST_ENV, incoming.sender())?
    };
    let session = Session { settings, client, incoming, sources, _credentials: credentials };
    let ra = ra_config();
    let uri = rc_lsp::path_to_uri(Path::new(&session.settings.root));
    let init=session.client.request_raw::<lsp_types::InitializeResult>("initialize",json!({"processId":null,"clientInfo":{"name":"CrabCraft remote","version":env!("CARGO_PKG_VERSION")},"rootUri":uri.as_str(),"workspaceFolders":[{"uri":uri.as_str(),"name":"remote"}],"capabilities":{"workspace":{"configuration":true},"textDocument":{"completion":{"completionItem":{"snippetSupport":false}},"publishDiagnostics":{"versionSupport":true}},"experimental":{"serverStatusNotification":true}},"initializationOptions":ra}));
    let _ = wait(init, &cancel, Instant::now() + Duration::from_secs(45))?;
    session.client.notify_raw("initialized", json!({}));
    session.client.notify_raw("workspace/didChangeConfiguration", json!({"settings":ra}));
    // No UI waits for indexing. Session poll responds to future configuration requests.
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(session)
}
fn analyzer_script(settings: &Settings) -> Result<String, Error> {
    let analyzer = quote(&settings.analyzer)?;
    Ok(format!(
        "set -u; child=; feed=; temp=$(mktemp -d /tmp/crabcraft-lsp.XXXXXXXX) || exit 1; cleanup() {{ trap - EXIT HUP INT TERM; if [ -n \"$child\" ]; then kill -TERM -- -\"$child\" 2>/dev/null || :; fi; if [ -n \"$feed\" ]; then kill -TERM -- -\"$feed\" 2>/dev/null || :; fi; rm -f -- \"$temp/input\"; rmdir -- \"$temp\"; }}; trap cleanup EXIT; trap 'exit 130' HUP INT TERM; mkfifo -- \"$temp/input\" || exit 1; setsid {analyzer} < \"$temp/input\" & child=$!; setsid /bin/bash -c 'cat > \"$1\"; kill -TERM -- -\"$2\" 2>/dev/null || :' bash \"$temp/input\" \"$child\" <&0 & feed=$!; code=0; wait \"$child\" || code=$?; exit \"$code\""
    ))
}
pub fn ra_config() -> Value {
    let mut config = rc_lsp::rust_analyzer::config(&Default::default());
    config["checkOnSave"] = json!(false);
    config["check"]["enable"] = json!(false);
    config
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    Check,
    Build,
    Test,
}
impl Task {
    pub fn word(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Build => "build",
            Self::Test => "test",
        }
    }
}
/// The stdin watchdog kills the isolated remote process group when SSH disconnects.
/// Both normal exit and HUP/TERM also kill remaining children; a remote timeout is a final bound.
pub fn cargo(settings: Settings, task: Task, cancel: CancelToken, on_line: &mut (dyn FnMut(Stream, &str) + Send)) -> Result<i32, Error> {
    settings.validate()?;
    let script = cargo_script(&settings, task)?;
    let (code, _) = Runtime::block_on(collect(&settings, script, &cancel, Duration::from_secs(125), on_line, true))?;
    Ok(code)
}
pub(crate) fn cargo_script(settings: &Settings, task: Task) -> Result<String, Error> {
    settings.validate()?;
    Ok(format!(
        "{}exec /bin/bash -c {}",
        settings.environment()?,
        shell_quote(&format!(
            "set -u; child=; guard=; cleanup() {{ trap - EXIT HUP INT TERM; if [ -n \"$child\" ]; then kill -TERM -- -\"$child\" 2>/dev/null || :; fi; if [ -n \"$guard\" ]; then kill -TERM -- -\"$guard\" 2>/dev/null || :; fi; }}; trap cleanup EXIT; trap 'exit 130' HUP INT TERM; setsid timeout --signal=TERM --kill-after=3s 120s {} {} --message-format=json --color=never & child=$!; setsid /bin/bash -c 'cat >/dev/null; kill -TERM -- -\"$1\" 2>/dev/null || :' bash \"$child\" <&0 & guard=$!; code=0; wait \"$child\" || code=$?; exit \"$code\"",
            quote(&settings.cargo)?,
            task.word()
        ))
    ))
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
#[derive(Clone)]
pub struct Preview {
    pub settings: Settings,
    pub remote: String,
    pub before: String,
    pub after: String,
    ssh: Option<remote_transfer::Preview>,
}
pub fn read(settings: &Settings, path: &str, cancel: CancelToken) -> Result<String, Error> {
    let path = settings.file(path)?;
    if let Some(container) = &settings.container {
        return container.file(&path, "read", None, None, cancel);
    }
    if let Some(wsl) = &settings.wsl {
        return crate::wsl_workspace::file(wsl, &settings.root, &path, "read", None, None, cancel);
    }
    match remote_transfer::run(settings.ssh.clone(), remote_transfer::Action::Read(path), cancel)? {
        remote_transfer::Reply::Read(text) if text.len() <= 512 * 1024 => Ok(text),
        remote_transfer::Reply::Read(_) => Err(Error::Limit),
        _ => Err(Error::Input),
    }
}
pub fn preview(settings: &Settings, path: &str, before: String, after: String, cancel: CancelToken) -> Result<Preview, Error> {
    let path = settings.file(path)?;
    if let Some(container) = &settings.container {
        container.file(&path, "preview", Some(&before), Some(&after), cancel)?;
        return Ok(Preview { settings: settings.clone(), remote: path, before, after, ssh: None });
    }
    if let Some(wsl) = &settings.wsl {
        crate::wsl_workspace::file(wsl, &settings.root, &path, "preview", Some(&before), Some(&after), cancel)?;
        return Ok(Preview { settings: settings.clone(), remote: path, before, after, ssh: None });
    }
    match remote_transfer::run(settings.ssh.clone(), remote_transfer::Action::PreviewEdit { remote: path, before, after }, cancel)? {
        remote_transfer::Reply::Preview(p) => {
            Ok(Preview { settings: settings.clone(), remote: p.remote.clone(), before: p.before.clone(), after: p.after.clone(), ssh: Some(p) })
        }
        _ => Err(Error::Input),
    }
}
pub fn apply(settings: &Settings, preview: Preview, cancel: CancelToken) -> Result<String, Error> {
    settings.file(&preview.remote)?;
    if preview.settings != *settings {
        return Err(Error::Input);
    }
    if let Some(container) = &settings.container {
        return container.file(&preview.remote, "apply", Some(&preview.before), Some(&preview.after), cancel);
    }
    if let Some(wsl) = &settings.wsl {
        return crate::wsl_workspace::file(wsl, &settings.root, &preview.remote, "apply", Some(&preview.before), Some(&preview.after), cancel);
    }
    let preview = preview.ssh.ok_or(Error::Input)?;
    match remote_transfer::run(settings.ssh.clone(), remote_transfer::Action::Apply(preview), cancel)? {
        remote_transfer::Reply::Applied(text) => Ok(text),
        _ => Err(Error::Input),
    }
}
pub fn uri_path(settings: &Settings, value: &str) -> Option<String> {
    let uri: lsp_types::Uri = value.parse().ok()?;
    let path = rc_lsp::sync::uri_to_unix_path(&uri)?;
    settings.file(&path).ok()
}
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
    pub error: bool,
}
pub fn cargo_diagnostic(settings: &Settings, line: &str) -> Option<Diagnostic> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("reason")?.as_str()? != "compiler-message" {
        return None;
    }
    let message = value.get("message")?;
    let span = message.get("spans")?.as_array()?.iter().find(|s| s.get("is_primary").and_then(Value::as_bool) == Some(true))?;
    let path = settings.file(span.get("file_name")?.as_str()?).ok()?;
    Some(Diagnostic {
        path,
        line: u32::try_from(span.get("line_start")?.as_u64()?).ok()?.saturating_sub(1),
        column: u32::try_from(span.get("column_start")?.as_u64()?).ok()?.saturating_sub(1),
        message: message.get("message")?.as_str()?.chars().take(4096).collect(),
        error: message.get("level")?.as_str()? == "error",
    })
}

impl Session {
    pub fn respond(&self, id: Value, method: &str, params: &Value) {
        let result = match method {
            "workspace/configuration" => Value::Array(vec![ra_config(); params.get("items").and_then(Value::as_array).map_or(0, Vec::len).min(128)]),
            "workspace/applyEdit" => {
                json!({"applied":false,"failureReason":"Remote workspace edits require an explicit reviewed operation"})
            }
            _ => Value::Null,
        };
        self.client.reply(id, Ok(result));
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub fn settings(root: &Path) -> Settings {
        Settings {
            ssh: crate::remote_transfer::tests::config(),
            wsl: None,
            container: None,
            root: root.display().to_string(),
            cargo: "/home/rafael/.cargo/bin/cargo".into(),
            analyzer: "/home/rafael/.cargo/bin/rust-analyzer".into(),
        }
    }
    pub fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"remote_development_evidence\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[workspace]\n")
            .unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn answer()->i32{42}\n#[test]fn works(){assert_eq!(answer(),42);}\n").unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main(){assert_eq!(remote_development_evidence::answer(),42);}\n").unwrap();
        dir
    }
    #[test]
    fn paths_and_remote_shell_arguments_refuse_namespace_escape() {
        let s = settings(Path::new("/tmp/remote project"));
        assert!(s.validate().is_ok());
        for bad in ["../src/main.rs", "/other/file.rs", "src/../../file.rs", "src/file\n.rs", "src/*.rs"] {
            assert!(s.file(bad).is_err(), "{bad}");
        }
        assert_eq!(s.file("src/main.rs").unwrap(), "/tmp/remote project/src/main.rs");
        assert_eq!(quote("/tmp/a'b").unwrap(), "'/tmp/a'\\''b'");
        assert!(quote("relative").is_err());
        assert!(uri_path(&s, "file:///tmp/remote%20project/src/main.rs").is_some());
        assert!(uri_path(&s, "file:///tmp/remote%20project-other/main.rs").is_none());
    }
    #[test]
    #[ignore = "Requires explicit private OpenSSH fixture at 127.0.0.1:55222"]
    fn real_remote_analyzer_definition_cargo_diagnostics_upload_and_test() {
        let _lock = crate::remote_transfer::tests::SSH_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = fixture();
        let s = settings(dir.path());
        let marker = dir.path().join("build-ran");
        std::fs::write(dir.path().join("build.rs"), "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}\n").unwrap();
        s.validate().unwrap();
        assert!(!marker.exists());
        let bad_hosts = dir.path().join("bad_known_hosts");
        std::fs::write(&bad_hosts, "").unwrap();
        let mut bad = s.clone();
        bad.ssh.known_hosts = bad_hosts;
        assert!(matches!(start(bad, Waker::noop(), CancelToken::new()), Err(Error::Process(_))));
        assert!(!marker.exists());
        let session = start(s.clone(), Waker::noop(), CancelToken::new()).unwrap();
        assert!(session.sources.iter().any(|p| p == "src/main.rs"));
        let text = read(&s, "src/main.rs", CancelToken::new()).unwrap();
        let path = s.file("src/main.rs").unwrap();
        let uri = rc_lsp::path_to_uri(Path::new(&path));
        session.client.notify_raw("textDocument/didOpen", json!({"textDocument":{"uri":uri.as_str(),"languageId":"rust","version":1,"text":text}}));
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut ready = false;
        while !ready {
            assert!(Instant::now() < deadline);
            for event in session.incoming.drain() {
                match event {
                    Incoming::Request { id, method, params } => session.respond(id, &method, &params),
                    Incoming::Notification { method, params } if method == "experimental/serverStatus" => ready = rc_lsp::rust_analyzer::is_quiescent(&params),
                    Incoming::Exited(code) => panic!("SSH RA exited {code:?}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let position = rc_lsp::lsp_types::Position { line: 0, character: text.find("answer()").unwrap() as u32 };
        let result = wait(
            session.client.request_raw::<Value>("textDocument/definition", json!({"textDocument":{"uri":uri.as_str()},"position":position})),
            &CancelToken::new(),
            Instant::now() + Duration::from_secs(15),
        )
        .unwrap();
        let first = result.as_array().and_then(|a| a.first()).unwrap();
        let target = first.get("targetUri").or_else(|| first.get("uri")).and_then(Value::as_str).unwrap();
        assert_eq!(uri_path(&s, target), Some(s.file("src/lib.rs").unwrap()));
        let mut output = Vec::new();
        assert_eq!(cargo(s.clone(), Task::Check, CancelToken::new(), &mut |_, line| output.push(line.to_owned())).unwrap(), 0);
        assert!(marker.exists());
        let library = read(&s, "src/lib.rs", CancelToken::new()).unwrap();
        let broken = library.replace("->i32{42}", "->i32{\"wrong\"}");
        let proposal = preview(&s, "src/lib.rs", library.clone(), broken.clone(), CancelToken::new()).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(), library);
        apply(&s, proposal, CancelToken::new()).unwrap();
        output.clear();
        assert_ne!(cargo(s.clone(), Task::Check, CancelToken::new(), &mut |_, line| output.push(line.to_owned())).unwrap(), 0);
        assert!(output.iter().filter_map(|line| cargo_diagnostic(&s, line)).any(|d| d.path.ends_with("/src/lib.rs") && d.error));
        let proposal = preview(&s, "src/lib.rs", broken, library, CancelToken::new()).unwrap();
        apply(&s, proposal, CancelToken::new()).unwrap();
        assert_eq!(cargo(s.clone(), Task::Build, CancelToken::new(), &mut |_, _| {}).unwrap(), 0);
        assert_eq!(cargo(s, Task::Test, CancelToken::new(), &mut |_, _| {}).unwrap(), 0);
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "Requires explicit private OpenSSH fixture; verifies cancellation kills the remote build process"]
    fn real_remote_cargo_cancel_kills_build_script() {
        let _lock = crate::remote_transfer::tests::SSH_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = fixture();
        let s = settings(dir.path());
        std::fs::write(
            dir.path().join("build.rs"),
            "fn main(){std::fs::write(\"remote-build.pid\",std::process::id().to_string()).unwrap();std::thread::sleep(std::time::Duration::from_secs(60));}\n",
        )
        .unwrap();
        let cancel = CancelToken::new();
        let worker = cancel.clone();
        let handle = std::thread::spawn(move || cargo(s, Task::Check, worker, &mut |_, _| {}));
        let deadline = Instant::now() + Duration::from_secs(20);
        let pidfile = dir.path().join("remote-build.pid");
        while !pidfile.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid: libc::pid_t = std::fs::read_to_string(pidfile).unwrap().parse().unwrap();
        cancel.cancel();
        assert!(matches!(handle.join().unwrap(), Err(Error::Cancelled)));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let gone = unsafe { libc::kill(pid, 0) } != 0;
            let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.contains(") Z "));
            if gone || zombie {
                break;
            }
            assert!(Instant::now() < deadline, "remote build {pid} survived SSH cancellation");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "Requires private SSH fixture; verifies disconnect kills an analyzer build script"]
    fn real_remote_analyzer_disconnect_kills_build_script() {
        let _lock = crate::remote_transfer::tests::SSH_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = fixture();
        let s = settings(dir.path());
        std::fs::write(dir.path().join("build.rs"),"fn main(){std::fs::write(\"analyzer-build.pid\",std::process::id().to_string()).unwrap();std::thread::sleep(std::time::Duration::from_secs(60));}\n").unwrap();
        let session = start(s, Waker::noop(), CancelToken::new()).unwrap();
        let pidfile = dir.path().join("analyzer-build.pid");
        let deadline = Instant::now() + Duration::from_secs(45);
        while !pidfile.exists() {
            assert!(Instant::now() < deadline, "analyzer did not invoke the trusted build script");
            for event in session.incoming.drain() {
                match event {
                    Incoming::Request { id, method, params } => session.respond(id, &method, &params),
                    Incoming::Exited(code) => panic!("analyzer exited {code:?}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid: libc::pid_t = std::fs::read_to_string(pidfile).unwrap().parse().unwrap();
        drop(session);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let gone = unsafe { libc::kill(pid, 0) } != 0;
            let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.contains(") Z "));
            if gone || zombie {
                break;
            }
            assert!(Instant::now() < deadline, "remote analyzer build {pid} survived disconnect");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
