//! Explicit connection to an already-running Linux container. No inspect/up/pull on open.
use crate::{
    remote_development_transport::{Error, absolute},
    wsl_workspace::Spec,
};
use std::path::PathBuf;
/// Host Docker selectors and credentials never configure an explicit workspace transport.
pub(crate) const HOST_ENV: &[&str] = &[
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_TLS",
    "DOCKER_TLS_VERIFY",
    "DOCKER_CERT_PATH",
    "DOCKER_API_VERSION",
    "DOCKER_CONFIG",
];
pub(crate) fn clean(command: &mut tokio::process::Command) {
    for name in HOST_ENV {
        command.env_remove(name);
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub cli: PathBuf,
    pub socket: String,
    pub container: String,
    pub user: String,
    pub root: String,
}
impl Config {
    pub fn validate(&self) -> Result<(), Error> {
        if !self.cli.is_absolute()
            || self.cli.as_os_str().len() > 4096
            || self
                .cli
                .to_str()
                .is_none_or(|p| p.chars().any(char::is_control))
            || !self.socket.starts_with("unix:///")
            || self.socket.len() > 4096
            || self.socket.chars().any(char::is_control)
            || !absolute(&self.socket[7..])
            || !(12..=64).contains(&self.container.len())
            || !self.container.bytes().all(|b| b.is_ascii_hexdigit())
            || self.user.is_empty()
            || self.user.len() > 64
            || !self
                .user
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-:".contains(&b))
            || self.user.starts_with('-')
            || !absolute(&self.root)
            || self.root == "/"
            || self.root.ends_with('/')
        {
            return Err(Error::Input);
        }
        Ok(())
    }
    pub fn exec(&self, linux: Vec<String>) -> Result<Spec, Error> {
        self.validate()?;
        if linux.is_empty()
            || !absolute(&linux[0])
            || linux.len() > 32
            || linux
                .iter()
                .any(|s| s.len() > 256 * 1024 || s.contains('\0'))
        {
            return Err(Error::Input);
        }
        let mut args = vec![
            "--host".into(),
            self.socket.clone(),
            "exec".into(),
            "-i".into(),
            "--workdir".into(),
            self.root.clone(),
            "--user".into(),
            self.user.clone(),
            self.container.clone(),
        ];
        args.extend(linux);
        Ok(Spec {
            program: self.cli.clone(),
            args,
        })
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
    pub fn file(
        &self,
        path: &str,
        action: &str,
        before: Option<&str>,
        after: Option<&str>,
        cancel: rc_dotnet::process::CancelToken,
    ) -> Result<String, Error> {
        crate::wsl_workspace::file_with(
            self.root.as_str(),
            path,
            action,
            before,
            after,
            cancel,
            |args| self.exec(args),
        )
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn config() -> Config {
        Config {
            cli: "/usr/bin/docker".into(),
            socket: "unix:///var/run/docker.sock".into(),
            container: "0123456789abcdef".into(),
            user: "1000:1000".into(),
            root: "/work space".into(),
        }
    }
    pub(crate) fn fixture(dir: &std::path::Path, root: &std::path::Path) -> (Config, PathBuf) {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let cli = dir.join("docker argv fixture");
        let log = dir.join("docker-argv.jsonl");
        let config = Config {
            cli: cli.clone(),
            root: root.to_str().unwrap().into(),
            ..config()
        };
        let prefix = config.exec(vec!["/bin/true".into()]).unwrap().args[..9].to_vec();
        let code = format!(
            "#!/usr/bin/python3\nimport os,sys,json\nassert all(k not in os.environ for k in ['DOCKER_HOST', 'DOCKER_CONTEXT', 'DOCKER_TLS', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH', 'DOCKER_API_VERSION', 'DOCKER_CONFIG'])\nexpected=json.loads({:?})\nassert sys.argv[1:10]==expected,sys.argv[1:10]\nwith open({:?},'a') as out:out.write(json.dumps(sys.argv[1:])+'\\n')\nargs=sys.argv[10:];os.execv(args[0],args)\n",
            serde_json::to_string(&prefix).unwrap(),
            log.to_str().unwrap()
        );
        std::fs::write(&cli, code).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        (config, log)
    }
    #[test]
    fn argv_keeps_linux_paths_and_scripts_separate_and_rejects_remote_endpoints() {
        let c = config();
        let s = c
            .script("printf '%s' 'literal;$(echo nope)'".into())
            .unwrap();
        assert_eq!(
            &s.args[..9],
            &[
                "--host",
                "unix:///var/run/docker.sock",
                "exec",
                "-i",
                "--workdir",
                "/work space",
                "--user",
                "1000:1000",
                "0123456789abcdef"
            ]
        );
        assert!(!s.args.iter().any(|a| a == "-t" || a == "--privileged"));
        assert_eq!(s.args.last().unwrap(), "printf '%s' 'literal;$(echo nope)'");
        for host in [
            "tcp://127.0.0.1:2375",
            "ssh://host",
            "unix:///tmp/../socket",
        ] {
            let mut bad = c.clone();
            bad.socket = host.into();
            assert!(bad.validate().is_err());
        }
        let mut bad = c;
        bad.container = "--privileged".into();
        assert!(bad.validate().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn shim_cargo_cancellation_stops_real_build_script_and_bridge_rejects_stale() {
        use crate::remote_development_transport as transport;
        use rc_dotnet::process::CancelToken;
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let project = transport::tests::fixture();
        let (config, log) = fixture(dir.path(), project.path());
        let mut settings = transport::tests::settings(project.path());
        settings.container = Some(config);
        settings.validate().unwrap();
        assert!(!log.exists());
        assert!(transport::uri_path(&settings, "file:///C:/work/src/main.rs").is_none());
        assert!(transport::uri_path(&settings, "file:///etc/passwd").is_none());
        let before = transport::read(&settings, "src/lib.rs", CancelToken::new()).unwrap();
        let preview = transport::preview(
            &settings,
            "src/lib.rs",
            before.clone(),
            before.replace("42", "41+1"),
            CancelToken::new(),
        )
        .unwrap();
        std::fs::write(
            project.path().join("src/lib.rs"),
            format!("{before}// external change\n"),
        )
        .unwrap();
        assert!(matches!(
            transport::apply(&settings, preview, CancelToken::new()),
            Err(Error::Stale)
        ));
        std::fs::write(project.path().join("build.rs"),"fn main(){std::fs::write(\"container-build.pid\",std::process::id().to_string()).unwrap();std::thread::sleep(std::time::Duration::from_secs(60));}\n").unwrap();
        let token = CancelToken::new();
        let worker = token.clone();
        let handle = std::thread::spawn(move || {
            transport::cargo(settings, transport::Task::Check, worker, &mut |_, _| {})
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        let pidfile = project.path().join("container-build.pid");
        while !pidfile.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid: i32 = std::fs::read_to_string(pidfile).unwrap().parse().unwrap();
        token.cancel();
        assert!(matches!(handle.join().unwrap(), Err(Error::Cancelled)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while unsafe { libc::kill(pid, 0) } == 0
            && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .is_ok_and(|s| s.contains(") Z "))
        {
            assert!(
                Instant::now() < deadline,
                "shim build survived cancellation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(unix)]
    #[test]
    fn shim_stdin_eof_watchdog_stops_linux_build_without_killing_cli() {
        use crate::remote_development_transport as transport;
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let dir = tempfile::tempdir().unwrap();
        let project = transport::tests::fixture();
        let (config, _) = fixture(dir.path(), project.path());
        let mut settings = transport::tests::settings(project.path());
        settings.container = Some(config.clone());
        std::fs::write(project.path().join("build.rs"),"fn main(){std::fs::write(\"eof-build.pid\",std::process::id().to_string()).unwrap();std::thread::sleep(std::time::Duration::from_secs(60));}\n").unwrap();
        let spec = config
            .script(transport::cargo_script(&settings, transport::Task::Check).unwrap())
            .unwrap();
        let mut child = Command::new(spec.program)
            .args(spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let pidfile = project.path().join("eof-build.pid");
        while !pidfile.exists() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                panic!("build did not start");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid: i32 = std::fs::read_to_string(pidfile).unwrap().parse().unwrap();
        drop(stdin); // Deliberately do not kill the CLI: EOF alone must trigger Linux cleanup.
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                panic!("EOF watchdog did not finish");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            unsafe { libc::kill(pid, 0) } != 0
                || std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    .is_ok_and(|s| s.contains(") Z "))
        );
    }
    #[cfg(unix)]
    #[test]
    fn host_docker_environment_is_removed_in_a_separate_process() {
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "container_workspace::tests::poisoned_environment_child",
                "--nocapture",
            ])
            .env("CRABCRAFT_CONTAINER_ENV_CHILD", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        for key in HOST_ENV {
            command.env(key, "poisoned-host-context");
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "poisoned environment subprocess failed");
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("environment subprocess timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(unix)]
    #[test]
    fn poisoned_environment_child() {
        if std::env::var("CRABCRAFT_CONTAINER_ENV_CHILD").as_deref() != Ok("1") {
            return;
        }
        assert!(
            HOST_ENV
                .iter()
                .all(|key| std::env::var(key).as_deref() == Ok("poisoned-host-context"))
        );
        use crate::remote_development_transport as transport;
        use rc_dotnet::process::CancelToken;
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let project = transport::tests::fixture();
        let (config, log) = fixture(dir.path(), project.path());
        let mut settings = transport::tests::settings(project.path());
        settings.container = Some(config);
        assert!(!log.exists());
        assert!(
            transport::read(&settings, "src/lib.rs", CancelToken::new())
                .unwrap()
                .contains("answer")
        );
        assert_eq!(
            transport::cargo(
                settings.clone(),
                transport::Task::Check,
                CancelToken::new(),
                &mut |_, _| {}
            )
            .unwrap(),
            0
        );
        let session =
            transport::start(settings, rc_core::Waker::noop(), CancelToken::new()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut ready = false;
        while !ready {
            assert!(Instant::now() < deadline);
            for event in session.incoming.drain() {
                match event {
                    rc_lsp::Incoming::Request { id, method, params } => {
                        session.respond(id, &method, &params)
                    }
                    rc_lsp::Incoming::Notification { method, params }
                        if method == "experimental/serverStatus" =>
                    {
                        ready = rc_lsp::rust_analyzer::is_quiescent(&params)
                    }
                    rc_lsp::Incoming::Exited(code) => panic!("analyzer exited {code:?}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(std::fs::read_to_string(log).unwrap().lines().count() >= 5);
    }
}
