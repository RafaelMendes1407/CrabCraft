//! WSL adapter. All Linux paths stay in the selected distribution; no UNC/local-file aliasing.
use crate::remote_development_transport::Error;
use rc_core::Runtime;
use rc_dotnet::process::CancelToken;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub distribution: String,
    pub user: String,
    #[cfg(test)]
    pub fixture: Option<PathBuf>,
}
impl Config {
    pub fn new(distribution: String, user: String) -> Self {
        Self {
            distribution,
            user,
            #[cfg(test)]
            fixture: None,
        }
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.distribution.is_empty()
            || self.distribution.len() > 256
            || self.distribution.starts_with('-')
            || self.distribution.chars().any(char::is_control)
            || self.user.is_empty()
            || self.user.len() > 64
            || self.user.starts_with('-')
            || !self.user.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        {
            return Err(Error::Input);
        }
        Ok(())
    }
    pub fn program(&self) -> Result<PathBuf, Error> {
        self.validate()?;
        #[cfg(test)]
        if let Some(path) = &self.fixture {
            return Ok(path.clone());
        }
        program()
    }
    pub fn exec(&self, linux: Vec<String>) -> Result<Spec, Error> {
        self.validate()?;
        if linux.is_empty()
            || !linux[0].starts_with('/')
            || linux[0].len() > 4096
            || linux[0].chars().any(char::is_control)
            || linux.len() > 32
            || linux.iter().any(|s| s.contains('\0'))
        {
            return Err(Error::Input);
        }
        let mut args = vec!["--distribution".into(), self.distribution.clone(), "--user".into(), self.user.clone(), "--exec".into()];
        args.extend(linux);
        Ok(Spec { program: self.program()?, args })
    }
    pub fn script(&self, script: String) -> Result<Spec, Error> {
        self.exec(vec![
            "/usr/bin/env".into(),
            "-u".into(),
            "BASH_ENV".into(),
            "-u".into(),
            "ENV".into(),
            "/bin/bash".into(),
            "--noprofile".into(),
            "--norc".into(),
            "-c".into(),
            script,
        ])
    }
}
#[derive(Clone, Debug)]
pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<String>,
}
pub fn available() -> bool {
    cfg!(windows)
}
fn program() -> Result<PathBuf, Error> {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).filter(|p| p.is_absolute()).ok_or(Error::Input)?;
        Ok(root.join("System32/wsl.exe"))
    }
    #[cfg(not(windows))]
    {
        Err(Error::Unsupported)
    }
}
async fn capture(spec: Spec, input: Vec<u8>, cancel: &CancelToken, limit: usize) -> Result<Vec<u8>, Error> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let mut command = tokio::process::Command::new(spec.program);
    crate::container_workspace::clean(&mut command);
    command
        .args(spec.args)
        .env_remove("WSLENV")
        .env_remove("BASH_ENV")
        .env_remove("ENV")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn()?;
    let pid = child.id().ok_or(Error::Input)?;
    let mut stdin = child.stdin.take().ok_or(Error::Input)?;
    let stdout = child.stdout.take().ok_or(Error::Input)?;
    let stderr = child.stderr.take().ok_or(Error::Input)?;
    async fn read(r: impl tokio::io::AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        r.take(limit as u64 + 1).read_to_end(&mut bytes).await?;
        if bytes.len() > limit {
            return Err(Error::Limit);
        }
        Ok(bytes)
    }
    let io = async {
        let output = read(stdout, limit);
        let error = read(stderr, 64 * 1024);
        let write = async {
            stdin.write_all(&input).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<(), Error>(())
        };
        let (out, err, ()) = tokio::try_join!(output, error, write)?;
        let status = child.wait().await?;
        if !status.success() {
            let text = decode_console(&err).unwrap_or_else(|_| String::from_utf8_lossy(&err).replace('\0', ""));
            return Err(Error::Process(text.chars().take(4096).collect()));
        }
        Ok(out)
    };
    let result =
        tokio::select! {result=io=>result,()=cancel.cancelled()=>Err(Error::Cancelled),()=tokio::time::sleep(Duration::from_secs(15))=>Err(Error::Timeout)};
    if result.is_err() {
        rc_dotnet::process::kill_tree(pid);
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    }
    result
}
pub fn distributions(cancel: CancelToken) -> Result<Vec<String>, Error> {
    let spec = Spec { program: program()?, args: vec!["--list".into(), "--quiet".into()] };
    let bytes = Runtime::block_on(capture(spec, Vec::new(), &cancel, 64 * 1024))?;
    decode_distributions(&bytes)
}
fn decode_console(bytes: &[u8]) -> Result<String, Error> {
    if bytes.len() > 64 * 1024 {
        return Err(Error::Limit);
    }
    Ok(if bytes.starts_with(&[0xff, 0xfe]) || bytes.contains(&0) {
        let bytes = bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes);
        if !bytes.len().is_multiple_of(2) {
            return Err(Error::Input);
        }
        let words: Vec<_> = bytes.as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
        String::from_utf16(&words).map_err(|_| Error::Input)?
    } else {
        std::str::from_utf8(bytes).map_err(|_| Error::Input)?.trim_start_matches('\u{feff}').to_owned()
    })
}
fn decode_distributions(bytes: &[u8]) -> Result<Vec<String>, Error> {
    let text = decode_console(bytes)?;
    let mut list = Vec::new();
    for line in text.lines() {
        let name = line.trim_end_matches('\r');
        if name.is_empty() {
            continue;
        }
        Config::new(name.into(), "user".into()).validate()?;
        if list.len() >= 64 {
            return Err(Error::Limit);
        }
        list.push(name.into());
    }
    list.sort();
    list.dedup();
    Ok(list)
}
/// A fixed Linux-side bridge reads JSON stdin. User text is never embedded in an argv/shell script.
const BRIDGE: &str = r#"import sys,json,os,stat,tempfile
MAX=524288
LIMIT=65536
class Failure(Exception):
 pass
def fail(kind):
 raise Failure(kind)
def regular(path):
 flags=os.O_RDONLY|os.O_NONBLOCK|getattr(os,'O_NOFOLLOW',0)
 fd=os.open(path,flags)
 try:
  before=os.fstat(fd)
  if not stat.S_ISREG(before.st_mode):fail('input')
  if before.st_size>MAX:fail('limit')
  with os.fdopen(fd,'rb',closefd=False) as source:data=source.read(MAX+1)
  after=os.fstat(fd)
  if len(data)>MAX:fail('limit')
  stamp=lambda s:(s.st_dev,s.st_ino,s.st_mtime_ns,s.st_size)
  if stamp(before)!=stamp(after) or stamp(after)!=stamp(os.stat(path,follow_symlinks=False)):fail('stale')
  text=data.decode('utf-8')
  if '\0' in text:fail('input')
  return text,before
 finally:os.close(fd)
try:
 raw=sys.stdin.buffer.read(1048577)
 if len(raw)>1048576:fail('limit')
 request=json.loads(raw)
 root=request['root'];path=request['path'];action=request['action']
 if not isinstance(root,str) or not isinstance(path,str) or not root.startswith('/') or os.path.normpath(root)!=root or os.path.normpath(path)!=path or not path.startswith(root+'/'):fail('input')
 if os.path.commonpath([os.path.realpath(root),os.path.realpath(path)])!=os.path.realpath(root):fail('input')
 text,info=regular(path)
 if action=='read':result=text
 elif action in ('preview','apply'):
  before=request['before'];after=request['after']
  if not isinstance(before,str) or not isinstance(after,str) or len(before.encode('utf-8'))>LIMIT or len(after.encode('utf-8'))>LIMIT:fail('limit')
  if '\0' in after:fail('input')
  if text!=before:fail('stale')
  if action=='apply':
   fd,temporary=tempfile.mkstemp(prefix='.crabcraft-wsl-',dir=os.path.dirname(path))
   try:
    os.fchmod(fd,stat.S_IMODE(info.st_mode)&0o777)
    with os.fdopen(fd,'wb') as target:
     target.write(after.encode('utf-8'));target.flush();os.fsync(target.fileno())
    current,current_info=regular(path)
    if current!=before or (current_info.st_dev,current_info.st_ino,current_info.st_mtime_ns)!=(info.st_dev,info.st_ino,info.st_mtime_ns):fail('stale')
    os.replace(temporary,path)
   finally:
    if os.path.lexists(temporary):os.unlink(temporary)
  result=after
 else:fail('input')
 sys.stdout.buffer.write((json.dumps({'text':result},ensure_ascii=False)+'\n').encode('utf-8'))
except Failure as error:print(json.dumps({'error':str(error)}))
except (OSError,ValueError,KeyError,TypeError,UnicodeError):print(json.dumps({'error':'input'}))
"#;
pub fn file(config: &Config, root: &str, path: &str, action: &str, before: Option<&str>, after: Option<&str>, cancel: CancelToken) -> Result<String, Error> {
    file_with(root, path, action, before, after, cancel, |args| config.exec(args))
}
pub(crate) fn file_with(
    root: &str,
    path: &str,
    action: &str,
    before: Option<&str>,
    after: Option<&str>,
    cancel: CancelToken,
    execute: impl FnOnce(Vec<String>) -> Result<Spec, Error>,
) -> Result<String, Error> {
    if !crate::remote_development_transport::absolute(root) || !crate::remote_development_transport::absolute(path) || !path.starts_with(&format!("{root}/")) {
        return Err(Error::Input);
    }
    if before.is_some_and(|t| t.len() > 64 * 1024) || after.is_some_and(|t| t.len() > 64 * 1024 || t.contains('\0')) {
        return Err(Error::Limit);
    }
    let request = json!({"root":root,"path":path,"action":action,"before":before,"after":after});
    let input = serde_json::to_vec(&request).map_err(|_| Error::Input)?;
    if input.len() > 1024 * 1024 {
        return Err(Error::Limit);
    }
    let spec = execute(vec![
        "/usr/bin/env".into(),
        "-u".into(),
        "BASH_ENV".into(),
        "-u".into(),
        "ENV".into(),
        "/usr/bin/python3".into(),
        "-I".into(),
        "-c".into(),
        BRIDGE.into(),
    ])?;
    let bytes = Runtime::block_on(capture(spec, input, &cancel, 4 * 1024 * 1024))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Input)?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(match error {
            "stale" => Error::Stale,
            "limit" => Error::Limit,
            _ => Error::Input,
        });
    }
    value.get("text").and_then(Value::as_str).filter(|text| text.len() <= 512 * 1024 && !text.contains('\0')).map(str::to_owned).ok_or(Error::Input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_development_transport as transport;
    use std::{path::Path, time::Instant};
    #[test]
    fn distribution_decoding_and_arguments_keep_user_values_as_separate_data() {
        let mut config = Config::new("Debian Test 🦀 ' $(no-shell)".into(), "developer".into());
        #[cfg(not(windows))]
        {
            config.fixture = Some("/tmp/fake-wsl".into());
        }
        let spec = config.script("printf '%s' '/tmp/source with spaces'".into()).unwrap();
        assert_eq!(&spec.args[..5], ["--distribution", "Debian Test 🦀 ' $(no-shell)", "--user", "developer", "--exec"]);
        assert_eq!(spec.args.last().unwrap(), "printf '%s' '/tmp/source with spaces'");
        assert!(spec.args.iter().any(|a| a == "--noprofile"));
        for bad in ["", "-oProxyCommand=x", "wrong\nname"] {
            assert!(Config::new(bad.into(), "user".into()).validate().is_err());
        }
        for bad in ["", "-root", "user name", "a;touch"] {
            assert!(Config::new("Ubuntu".into(), bad.into()).validate().is_err());
        }
        let text = "Ubuntu-24.04\r\nDebian Test\r\n";
        let utf16: Vec<u8> = [0xff, 0xfe].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(decode_distributions(&utf16).unwrap(), vec!["Debian Test", "Ubuntu-24.04"]);
        assert_eq!(decode_distributions(text.as_bytes()).unwrap(), vec!["Debian Test", "Ubuntu-24.04"]);
        assert!(decode_distributions(&[0xff, 0xfe, 0x00, 0xd8]).is_err());
        assert!(decode_distributions(&[b'a'; 65537]).is_err());
    }
    #[cfg(unix)]
    fn fixture(dir: &Path) -> (Config, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let program = dir.join("wsl fixture");
        let log = dir.join("argv.jsonl");
        let code = format!(
            "#!/usr/bin/python3\nimport os,sys,json\nwith open({},'a') as log:log.write(json.dumps(sys.argv[1:])+'\\n')\nif sys.argv[1:]==['--list','--quiet']:\n sys.stdout.buffer.write('Ubuntu-24.04\\r\\nDebian Test\\r\\n'.encode('utf-16'));sys.exit(0)\nassert sys.argv[1:5]==['--distribution','Fixture distro','--user','developer']\ni=sys.argv.index('--exec');args=sys.argv[i+1:];os.execv(args[0],args)\n",
            serde_json::to_string(log.to_str().unwrap()).unwrap()
        );
        std::fs::write(&program, code).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut config = Config::new("Fixture distro".into(), "developer".into());
        config.fixture = Some(program);
        (config, log)
    }
    #[cfg(unix)]
    #[test]
    fn fixture_bridge_preserves_bytes_previews_and_conflicts_without_interpreting_source() {
        let dir = tempfile::tempdir().unwrap();
        let (config, log) = fixture(dir.path());
        let root = dir.path().join("project ' $(literal)");
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.rs");
        let root = root.to_str().unwrap();
        let path = source.to_str().unwrap();
        const BEFORE: &str = "\u{feff}fn main(){let text=\"$(touch /tmp/do-not-run) 🦀\";}\r\n";
        const AFTER: &str = "\u{feff}fn main(){let answer=42;}\r\n";
        std::fs::write(&source, BEFORE).unwrap();
        assert!(!log.exists());
        assert_eq!(file(&config, root, path, "read", None, None, CancelToken::new()).unwrap(), BEFORE);
        assert_eq!(file(&config, root, path, "preview", Some(BEFORE), Some(AFTER), CancelToken::new()).unwrap(), AFTER);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), BEFORE);
        assert!(matches!(file(&config, root, path, "apply", Some("stale"), Some(AFTER), CancelToken::new()), Err(Error::Stale)));
        assert_eq!(file(&config, root, path, "apply", Some(BEFORE), Some(AFTER), CancelToken::new()).unwrap(), AFTER);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), AFTER);
        std::os::unix::fs::symlink(&source, dir.path().join("alias.rs")).unwrap();
        assert!(matches!(
            file(&config, dir.path().to_str().unwrap(), dir.path().join("alias.rs").to_str().unwrap(), "read", None, None, CancelToken::new()),
            Err(Error::Input)
        ));
        let before_log = std::fs::read_to_string(&log).unwrap();
        let cancelled = CancelToken::new();
        cancelled.cancel();
        assert!(matches!(file(&config, root, path, "read", None, None, cancelled), Err(Error::Cancelled)));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), before_log);
        let args: Vec<Value> = before_log.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert!(args.iter().all(|args| { args.as_array().unwrap().iter().all(|value| !value.as_str().unwrap().contains(BEFORE)) }));
        assert!(args.iter().all(|args| { args.as_array().unwrap().iter().any(|value| value.as_str() == Some("-I")) }));
        let fifo = root.to_owned() + "/fifo.rs";
        let name = std::ffi::CString::new(fifo.clone()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let start = Instant::now();
        assert!(file(&config, root, &fifo, "read", None, None, CancelToken::new()).is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[cfg(unix)]
    #[test]
    fn fixture_enumeration_is_explicit_and_cancellation_bounds_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let (config, log) = fixture(dir.path());
        assert!(!log.exists());
        let spec = Spec { program: config.program().unwrap(), args: vec!["--list".into(), "--quiet".into()] };
        let bytes = Runtime::block_on(capture(spec, Vec::new(), &CancelToken::new(), 65536)).unwrap();
        assert_eq!(decode_distributions(&bytes).unwrap(), vec!["Debian Test", "Ubuntu-24.04"]);
        let script = "echo $$ > '{}' ; exec sleep 60".replace("{}", dir.path().join("child.pid").to_str().unwrap());
        let spec = config.script(script).unwrap();
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let handle = std::thread::spawn(move || Runtime::block_on(capture(spec, Vec::new(), &token, 65536)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !dir.path().join("child.pid").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.cancel();
        assert!(matches!(handle.join().unwrap(), Err(Error::Cancelled)));
    }
    #[cfg(unix)]
    #[test]
    fn fixture_shared_workspace_uses_linux_uri_lsp_cargo_and_reviewed_edits() {
        // This is a Linux argv shim, deliberately not evidence of a real Windows/WSL installation.
        let dir = tempfile::tempdir().unwrap();
        let (config, _) = fixture(dir.path());
        let project = transport::tests::fixture();
        let mut settings = transport::tests::settings(project.path());
        settings.wsl = Some(config);
        assert!(transport::uri_path(&settings, "file:///C:/project/main.rs").is_none());
        let session = transport::start(settings.clone(), rc_core::Waker::noop(), CancelToken::new()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut ready = false;
        while !ready {
            assert!(Instant::now() < deadline);
            for event in session.incoming.drain() {
                match event {
                    rc_lsp::Incoming::Request { id, method, params } => session.respond(id, &method, &params),
                    rc_lsp::Incoming::Notification { method, params } if method == "experimental/serverStatus" => {
                        ready = rc_lsp::rust_analyzer::is_quiescent(&params)
                    }
                    rc_lsp::Incoming::Exited(code) => panic!("fixture analyzer exited {code:?}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let before = transport::read(&settings, "src/lib.rs", CancelToken::new()).unwrap();
        let after = before.replace("42", "41+1");
        let preview = transport::preview(&settings, "src/lib.rs", before.clone(), after.clone(), CancelToken::new()).unwrap();
        assert_eq!(std::fs::read_to_string(project.path().join("src/lib.rs")).unwrap(), before);
        transport::apply(&settings, preview, CancelToken::new()).unwrap();
        assert_eq!(transport::read(&settings, "src/lib.rs", CancelToken::new()).unwrap(), after);
        assert_eq!(transport::cargo(settings, transport::Task::Test, CancelToken::new(), &mut |_, _| {}).unwrap(), 0);
    }
}
