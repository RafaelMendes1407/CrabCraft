//! Explicit OpenSSH transfers. No SSH configuration, local shell, host discovery or login on open.
use rc_dotnet::process::{CancelToken, StreamOpts, run_streaming};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
const MAX: usize = 2 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity: PathBuf,
    pub known_hosts: PathBuf,
}
#[derive(Debug)]
pub enum Error {
    Input,
    Limit,
    Cancelled,
    Timeout,
    Stale,
    Io(String),
    Process(String),
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string().chars().take(2048).collect())
    }
}
#[derive(Clone, Debug)]
pub struct Preview {
    pub config: Config,
    pub remote: String,
    pub before: String,
    pub after: String,
    pub local: Option<(PathBuf, Vec<u8>)>,
}
pub enum Action {
    Read(String),
    PreviewEdit {
        remote: String,
        before: String,
        after: String,
    },
    PreviewDeploy {
        remote: String,
        local: PathBuf,
    },
    Apply(Preview),
    Terminal,
}
pub enum Reply {
    Read(String),
    Preview(Preview),
    Applied(String),
    Terminal(rc_term::SpawnSpec, tempfile::TempDir),
}
fn valid_word(value: &str) -> bool {
    !value.is_empty()
        && value.len() < 254
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"._-".contains(&ch))
        && !value.starts_with('-')
}
fn remote_path(path: &str) -> Result<String, Error> {
    if !path.starts_with('/')
        || path.len() > 4096
        || path
            .chars()
            .any(|ch| ch.is_control() || "\"\\*?[]{}".contains(ch))
        || path.split('/').any(|part| part == "..")
        || path.ends_with('/')
    {
        return Err(Error::Input);
    }
    Ok(format!("\"{path}\""))
}
pub fn bounded_file(path: &Path) -> Result<Vec<u8>, Error> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(Error::Input);
    }
    if meta.len() > MAX as u64 {
        return Err(Error::Limit);
    }
    let mut bytes = Vec::new();
    file.take(MAX as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX {
        return Err(Error::Limit);
    }
    Ok(bytes)
}
fn utf8(bytes: Vec<u8>) -> Result<String, Error> {
    String::from_utf8(bytes)
        .ok()
        .filter(|text| !text.contains('\0'))
        .ok_or(Error::Input)
}
pub(crate) fn args(config: &Config, dir: &Path) -> Result<Vec<String>, Error> {
    if !valid_word(&config.host) || !valid_word(&config.user) || config.port == 0 {
        return Err(Error::Input);
    }
    let identity = bounded_file(&config.identity)?;
    let known = bounded_file(&config.known_hosts)?;
    if identity.len() > 64 * 1024 || known.len() > 256 * 1024 {
        return Err(Error::Limit);
    }
    let key = dir.join("identity");
    let hosts = dir.join("known_hosts");
    std::fs::write(&key, identity)?;
    std::fs::write(&hosts, known)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600))?;
    }
    let mut args = vec![
        "-F".into(),
        "none".into(),
        "-i".into(),
        key.display().to_string(),
    ];
    for option in [
        "BatchMode=yes",
        "IdentitiesOnly=yes",
        "IdentityAgent=none",
        "StrictHostKeyChecking=yes",
        "GlobalKnownHostsFile=none",
        "ForwardAgent=no",
        "ForwardX11=no",
        "ClearAllForwardings=yes",
        "PermitLocalCommand=no",
        "ProxyCommand=none",
        "ProxyJump=none",
        "ControlMaster=no",
        "ControlPath=none",
        "ConnectTimeout=5",
        "ConnectionAttempts=1",
        "ServerAliveInterval=3",
        "ServerAliveCountMax=2",
    ] {
        args.extend(["-o".into(), option.into()]);
    }
    args.extend([
        "-o".into(),
        format!("UserKnownHostsFile={}", hosts.display()),
    ]);
    Ok(args)
}
async fn batch(
    config: &Config,
    dir: &Path,
    commands: &str,
    watched: Option<&Path>,
    cancel: &CancelToken,
) -> Result<(), Error> {
    let mut arguments = args(config, dir)?;
    let file = dir.join("batch");
    std::fs::write(&file, commands)?;
    arguments.extend([
        "-P".into(),
        config.port.to_string(),
        "-R".into(),
        "1".into(),
        "-B".into(),
        "32768".into(),
        "-b".into(),
        file.display().to_string(),
        format!("{}@{}", config.user, config.host),
    ]);
    let mut command = tokio::process::Command::new("sftp");
    command
        .args(arguments)
        .current_dir(dir)
        .env_remove("SSH_ASKPASS")
        .env("SSH_ASKPASS_REQUIRE", "never");
    let child_cancel = CancelToken::new();
    let mut output = String::new();
    let mut overflow = false;
    let (result, stop) = {
        let mut on_start = |_| {};
        let mut on_line = |line: rc_dotnet::process::OutputLine| {
            if output.len() + line.text.len() > 32 * 1024 {
                overflow = true;
                child_cancel.cancel();
            } else {
                output.push_str(&line.text);
                output.push('\n');
            }
        };
        let process = run_streaming(
            command,
            &child_cancel,
            StreamOpts::default(),
            &mut on_start,
            &mut on_line,
        );
        tokio::pin!(process);
        let start = std::time::Instant::now();
        let mut stop = None;
        let result = loop {
            tokio::select! {result=&mut process=>break result?,()=tokio::time::sleep(Duration::from_millis(10))=>{if stop.is_none(){if cancel.is_cancelled(){stop=Some(Error::Cancelled);}else if start.elapsed()>Duration::from_secs(15){stop=Some(Error::Timeout);}else if watched.and_then(|path|std::fs::metadata(path).ok()).is_some_and(|meta|meta.len()>MAX as u64){stop=Some(Error::Limit);}if stop.is_some(){child_cancel.cancel();}}}}
        };
        (result, stop)
    };
    if let Some(error) = stop {
        return Err(error);
    }
    if overflow {
        return Err(Error::Limit);
    }
    if result.cancelled {
        return Err(Error::Cancelled);
    }
    if result.code != Some(0) {
        return Err(Error::Process(output.chars().take(4096).collect()));
    }
    Ok(())
}
async fn read(
    config: &Config,
    remote: &str,
    dir: &Path,
    cancel: &CancelToken,
) -> Result<String, Error> {
    let path = remote_path(remote)?;
    let local = dir.join("download");
    batch(
        config,
        dir,
        &format!("get {path} \"{}\"\n", local.display()),
        Some(&local),
        cancel,
    )
    .await?;
    utf8(bounded_file(&local)?)
}
pub fn run(config: Config, action: Action, cancel: CancelToken) -> Result<Reply, Error> {
    let dir = tempfile::tempdir()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        match action {
            Action::Read(remote) => read(&config, &remote, dir.path(), &cancel)
                .await
                .map(Reply::Read),
            Action::PreviewEdit {
                remote,
                before,
                after,
            } => {
                if before.len() > 64 * 1024 || after.len() > 64 * 1024 || after.contains('\0') {
                    return Err(Error::Limit);
                }
                let current = read(&config, &remote, dir.path(), &cancel).await?;
                if current != before {
                    return Err(Error::Stale);
                }
                Ok(Reply::Preview(Preview {
                    config,
                    remote,
                    before,
                    after,
                    local: None,
                }))
            }
            Action::PreviewDeploy { remote, local } => {
                let bytes = bounded_file(&local)?;
                let after = utf8(bytes.clone())?;
                let before = read(&config, &remote, dir.path(), &cancel).await?;
                Ok(Reply::Preview(Preview {
                    config,
                    remote,
                    before,
                    after,
                    local: Some((local, bytes)),
                }))
            }
            Action::Apply(preview) => {
                if preview.config != config
                    || preview.before.len() > MAX
                    || preview.after.len() > MAX
                {
                    return Err(Error::Input);
                }
                if let Some((path, bytes)) = &preview.local {
                    if bounded_file(path)? != *bytes {
                        return Err(Error::Stale);
                    }
                }
                let current = read(&config, &preview.remote, dir.path(), &cancel).await?;
                if current != preview.before {
                    return Err(Error::Stale);
                }
                let target = remote_path(&preview.remote)?;
                let nonce = dir
                    .path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or(Error::Input)?;
                let temporary = format!("{}.crabcraft-{nonce}", preview.remote);
                let temporary_quoted = remote_path(&temporary)?;
                let upload = dir.path().join("upload");
                std::fs::write(&upload, preview.after.as_bytes())?;
                // Server-side rename is atomic, but SFTP has no content compare-and-swap. A peer can race
                // the preflight check. Never present this as a lock or an unconditional stale guarantee.
                batch(
                    &config,
                    dir.path(),
                    &format!(
                        "put \"{}\" {temporary_quoted}\nrename {temporary_quoted} {target}\n",
                        upload.display()
                    ),
                    None,
                    &cancel,
                )
                .await?;
                Ok(Reply::Applied(preview.after))
            }
            Action::Terminal => {
                let mut arguments = args(&config, dir.path())?;
                arguments.extend([
                    "-p".into(),
                    config.port.to_string(),
                    "-t".into(),
                    format!("{}@{}", config.user, config.host),
                ]);
                Ok(Reply::Terminal(
                    rc_term::SpawnSpec::command("ssh", arguments, None),
                    dir,
                ))
            }
        }
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) static SSH_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    pub(crate) fn config() -> Config {
        Config {
            host: "127.0.0.1".into(),
            port: 55222,
            user: "rafael".into(),
            identity: "/tmp/crabcraft-ssh/client_key".into(),
            known_hosts: "/tmp/crabcraft-ssh/known_hosts".into(),
        }
    }
    fn request(config: &Config, action: Action) -> Reply {
        run(config.clone(), action, CancelToken::new())
            .unwrap_or_else(|error| panic!("SFTP: {error:?}"))
    }
    fn preview(config: &Config, remote: &str, before: &str, after: &str) -> Preview {
        match request(
            config,
            Action::PreviewEdit {
                remote: remote.into(),
                before: before.into(),
                after: after.into(),
            },
        ) {
            Reply::Preview(preview) => preview,
            _ => panic!("preview"),
        }
    }
    #[test]
    fn remote_command_paths_refuse_batch_injection_and_file_reads_refuse_special_files() {
        for path in [
            "relative",
            "/tmp/../secret",
            "/tmp/a\nb",
            "/tmp/\";!touch x",
            "/tmp/a*",
            "/tmp/a\\x",
        ] {
            assert!(remote_path(path).is_err());
        }
        assert_eq!(remote_path("/tmp/a file 🦀").unwrap(), "\"/tmp/a file 🦀\"");
        assert!(!valid_word("-oProxyCommand=touch"));
        assert!(!valid_word("x;touch"));
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(bounded_file(dir.path()), Err(Error::Input)));
    }
    #[test]
    #[ignore = "Requires the explicit private SSH fixture on 127.0.0.1:55222"]
    fn real_sftp_rust_file_preview_stale_deploy_hostkey_and_cancel() {
        let _lock = SSH_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let config = config();
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"ssh_rust_evidence\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
        )
        .unwrap();
        let remote = root.join("src/main.rs");
        let name = remote.display().to_string();
        let first = "fn main(){println!(\"first\");}\n";
        let second = "fn main(){println!(\"edited 🦀\");}\n";
        std::fs::write(&remote, first).unwrap();
        std::fs::write(
            root.join("build.rs"),
            "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}",
        )
        .unwrap();
        match request(&config, Action::Read(name.clone())) {
            Reply::Read(text) => assert_eq!(text, first),
            _ => panic!("read"),
        };
        let pending = preview(&config, &name, first, second);
        assert_eq!(std::fs::read_to_string(&remote).unwrap(), first);
        std::fs::write(&remote, "external").unwrap();
        assert!(matches!(
            run(config.clone(), Action::Apply(pending), CancelToken::new()),
            Err(Error::Stale)
        ));
        std::fs::write(&remote, first).unwrap();
        request(
            &config,
            Action::Apply(preview(&config, &name, first, second)),
        );
        assert_eq!(std::fs::read_to_string(&remote).unwrap(), second);
        assert!(!root.join("build-ran").exists());
        let local = root.join("deploy.rs");
        std::fs::write(&local, "fn main(){println!(\"deployed Rust\");}\n").unwrap();
        let pending = match request(
            &config,
            Action::PreviewDeploy {
                remote: name.clone(),
                local: local.clone(),
            },
        ) {
            Reply::Preview(preview) => preview,
            _ => panic!("preview"),
        };
        std::fs::write(&local, "changed since preview").unwrap();
        assert!(matches!(
            run(config.clone(), Action::Apply(pending), CancelToken::new()),
            Err(Error::Stale)
        ));
        std::fs::write(&local, "fn main(){println!(\"deployed Rust\");}\n").unwrap();
        let pending = match request(
            &config,
            Action::PreviewDeploy {
                remote: name.clone(),
                local,
            },
        ) {
            Reply::Preview(preview) => preview,
            _ => panic!("preview"),
        };
        request(&config, Action::Apply(pending));
        let built = std::process::Command::new("cargo")
            .args(["run", "--offline", "--quiet"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&built.stdout).trim(),
            "deployed Rust"
        );
        let bad_hosts = root.join("wrong_hosts");
        std::fs::write(&bad_hosts, "").unwrap();
        let mut unknown = config.clone();
        unknown.known_hosts = bad_hosts;
        assert!(matches!(
            run(unknown, Action::Read(name.clone()), CancelToken::new()),
            Err(Error::Process(_))
        ));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut stalled = config.clone();
        stalled.port = listener.local_addr().unwrap().port();
        let cancel = CancelToken::new();
        let stop = cancel.clone();
        let start = std::time::Instant::now();
        let worker = std::thread::spawn(move || run(stalled, Action::Read(name), cancel));
        let (connection, _) = listener.accept().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        stop.cancel();
        assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
        drop(connection);
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
