//! Explicit reference Dev Container CLI integration; this does not move IDE services into containers.
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;
#[derive(Clone, Debug, PartialEq)]
enum Error {
    Trust,
    Input,
    Confirm,
    Stale,
    Io,
    Output,
    Cancelled,
    Exit(String),
}
impl Error {
    fn message(&self) -> String {
        match self{Self::Trust=>tr("Trust this project before running project tools or code").into(),Self::Input=>tr("Choose Dev Container and Docker executables and a command with separate arguments").into(),Self::Confirm=>tr("Review and confirm the Dev Container command before execution").into(),Self::Stale=>tr("Dev Container settings or project changed; review the command again").into(),Self::Io=>tr("Could not start or communicate with the Dev Container CLI").into(),Self::Output=>tr("Dev Container output or operation time limit exceeded").into(),Self::Cancelled=>tr("Dev Container CLI cancelled; containers or lifecycle processes may remain and need explicit management").into(),Self::Exit(message)=>trf("Dev Container CLI failed: {message}",&[("message",message)])}
    }
}
#[derive(Clone, Debug, PartialEq)]
struct Config {
    program: String,
    docker: String,
    host: String,
}
fn program(s: &str) -> bool {
    let p = Path::new(s);
    !s.is_empty()
        && s.len() <= 4096
        && !s.contains('\0')
        && (p.is_absolute() || p.components().count() == 1)
}
impl Config {
    fn validate(&self) -> Result<(), Error> {
        if !program(&self.program)
            || !program(&self.docker)
            || self.host.len() > 4096
            || self.host.contains(['\0', '\r', '\n'])
            || (!self.host.is_empty()
                && !self.host.starts_with("unix://")
                && !self.host.starts_with("npipe://"))
        {
            Err(Error::Input)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
enum Action {
    Version,
    Read,
    Build,
    Up,
    Exec { command: String, args: Vec<String> },
}
impl Action {
    fn write(&self) -> bool {
        matches!(self, Self::Build | Self::Up | Self::Exec { .. })
    }
}
#[derive(Clone)]
struct Plan {
    config: Config,
    root: PathBuf,
    action: Action,
    args: Vec<String>,
}
impl Plan {
    fn new(config: Config, root: PathBuf, action: Action) -> Result<Self, Error> {
        config.validate()?;
        if !root.is_absolute() {
            return Err(Error::Input);
        }
        let mut args = match action {
            Action::Version => vec!["--version".into()],
            Action::Read => vec!["read-configuration".into()],
            Action::Build => vec!["build".into()],
            Action::Up => vec!["up".into()],
            Action::Exec { .. } => vec!["exec".into()],
        };
        if !matches!(action, Action::Version) {
            args.extend([
                "--workspace-folder".into(),
                root.to_str().ok_or(Error::Input)?.into(),
                "--docker-path".into(),
                config.docker.clone(),
            ]);
        }
        if let Action::Exec {
            command,
            args: command_args,
        } = &action
        {
            if command.is_empty()
                || command.starts_with('-')
                || command.len() > 4096
                || command.contains('\0')
                || command_args.len() > 32
                || command_args
                    .iter()
                    .any(|arg| arg.len() > 4096 || arg.contains('\0'))
            {
                return Err(Error::Input);
            }
            args.push("--".into());
            args.push(command.clone());
            args.extend(command_args.clone());
        }
        Ok(Self {
            config,
            root,
            action,
            args,
        })
    }
    fn preview(&self) -> String {
        let mut parts = vec![self.config.program.clone()];
        parts.extend(self.args.clone());
        let command = parts
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(" ");
        if self.config.host.is_empty() {
            command
        } else {
            format!("DOCKER_HOST={:?}\n{command}", self.config.host)
        }
    }
}
struct Group(Option<u32>);
impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            rc_dotnet::kill_tree(pid);
        }
    }
}
async fn limited<R: tokio::io::AsyncRead + Unpin>(reader: R) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    reader
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut output)
        .await
        .map_err(|_| Error::Io)?;
    if output.len() > 4 * 1024 * 1024 {
        return Err(Error::Output);
    }
    Ok(output)
}
async fn execute(
    plan: Plan,
    trusted: bool,
    confirmed: bool,
    stop: rc_dotnet::CancelToken,
) -> Result<String, Error> {
    if !trusted {
        return Err(Error::Trust);
    }
    if plan.action.write() && !confirmed {
        return Err(Error::Confirm);
    }
    if stop.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let root = plan.root.canonicalize().map_err(|_| Error::Input)?;
    if root != plan.root || !root.is_dir() {
        return Err(Error::Input);
    }
    let neutral = tempfile::tempdir().map_err(|_| Error::Io)?;
    let mut cmd = tokio::process::Command::new(&plan.config.program);
    cmd.args(&plan.args)
        .current_dir(neutral.path())
        .env_remove("DOCKER_CONTEXT")
        .env_remove("DOCKER_HOST")
        .env_remove("DOCKER_TLS_VERIFY")
        .env_remove("DOCKER_CERT_PATH");
    if !plan.config.host.is_empty() {
        cmd.env("DOCKER_HOST", &plan.config.host);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|_| Error::Io)?;
    let group = Group(child.id());
    let stdout = child.stdout.take().ok_or(Error::Io)?;
    let stderr = child.stderr.take().ok_or(Error::Io)?;
    let work = async {
        let (out, err) = tokio::try_join!(limited(stdout), limited(stderr))?;
        let status = child.wait().await.map_err(|_| Error::Io)?;
        if !status.success() {
            return Err(Error::Exit(
                String::from_utf8_lossy(&err).chars().take(4096).collect(),
            ));
        }
        let mut output = String::from_utf8(out).map_err(|_| Error::Output)?;
        if !err.is_empty() {
            output.push('\n');
            output.extend(String::from_utf8_lossy(&err).chars().take(16384));
        }
        Ok(output)
    };
    let result = tokio::select! {result=tokio::time::timeout(Duration::from_secs(if plan.action.write(){600}else{60}),work)=>result.map_err(|_|Error::Output)?,()=stop.cancelled()=>Err(Error::Cancelled)};
    drop(group);
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    result
}
struct Event(Result<String, Error>);
pub struct DevContainers {
    pub program: String,
    pub docker: String,
    pub host: String,
    pub command: String,
    pub args: String,
    inbox: Inbox<Event>,
    plan: Option<Plan>,
    confirm: bool,
    stop: Option<rc_dotnet::CancelToken>,
    output: String,
    error: Option<String>,
}
impl DevContainers {
    pub fn new(waker: Waker) -> Self {
        Self {
            program: "devcontainer".into(),
            docker: "docker".into(),
            host: String::new(),
            command: "cargo".into(),
            args: "test\n--offline".into(),
            inbox: Inbox::new(waker),
            plan: None,
            confirm: false,
            stop: None,
            output: String::new(),
            error: None,
        }
    }
    fn config(&self) -> Config {
        Config {
            program: self.program.clone(),
            docker: self.docker.clone(),
            host: self.host.clone(),
        }
    }
    fn prepare(&mut self, root: &Path, action: Action) {
        self.confirm = false;
        match Plan::new(self.config(), root.to_path_buf(), action) {
            Ok(plan) => {
                self.plan = Some(plan);
                self.error = None;
            }
            Err(error) => self.error = Some(error.message()),
        }
    }
    fn start(&mut self, root: &Path, trusted: bool) {
        let Some(plan) = self.plan.take() else {
            return;
        };
        if plan.config != self.config() || plan.root != root {
            self.error = Some(Error::Stale.message());
            return;
        }
        let stop = rc_dotnet::CancelToken::new();
        self.stop = Some(stop.clone());
        let confirmed = self.confirm;
        self.confirm = false;
        let out = self.inbox.sender();
        rc_core::Runtime::spawn(async move {
            out.send(Event(execute(plan, trusted, confirmed, stop).await));
        });
    }
    pub fn poll(&mut self, trusted: bool) {
        if !trusted {
            if let Some(stop) = &self.stop {
                stop.cancel();
            }
            self.plan = None;
            self.confirm = false;
        }
        for Event(result) in self.inbox.drain() {
            self.stop = None;
            match result {
                Ok(output) => {
                    self.output = output.chars().take(16384).collect();
                    self.error = None;
                }
                Err(error) => self.error = Some(error.message()),
            }
        }
    }
    fn show(&mut self, ui: &mut egui::Ui, root: Option<PathBuf>, trusted: bool) {
        egui::ScrollArea::vertical().id_salt("devcontainers_body").show(ui,|ui|{
ui.label(tr("Dev Container actions use the separately installed reference CLI. Nothing runs on project open."));ui.colored_label(rc_kit::pal(ui).warn,tr("Build, Up and Exec can download images, bind host directories, run host initializeCommand and container lifecycle hooks, and execute project code. Review devcontainer.json and trust its entire configuration."));ui.label(tr("Container-backed IDE editing, language servers, debugging, installation and automatic attach are unsupported. Manage remaining containers explicitly in Docker."));if !trusted{ui.colored_label(rc_kit::pal(ui).warn,tr("Trust this project before running project tools or code"));}
ui.add_enabled_ui(self.stop.is_none(),|ui|{ui.label(tr("Dev Container CLI executable"));ui.add(egui::TextEdit::singleline(&mut self.program).char_limit(4096));ui.label(tr("Docker executable"));ui.add(egui::TextEdit::singleline(&mut self.docker).char_limit(4096));ui.label(tr("Docker local socket URI (empty uses the CLI default)"));ui.add(egui::TextEdit::singleline(&mut self.host).char_limit(4096));});
if let Some(root)=&root{ui.add_enabled_ui(trusted&&self.stop.is_none(),|ui|{ui.horizontal_wrapped(|ui|{for(action,label)in[(Action::Version,tr("Check Dev Container CLI")),(Action::Read,tr("Read Dev Container Configuration"))]{if ui.button(label).clicked(){self.prepare(root,action);self.start(root,trusted);}}});ui.horizontal_wrapped(|ui|{if ui.button(tr("Review Dev Container Build")).clicked(){self.prepare(root,Action::Build);}if ui.button(tr("Review Dev Container Up")).clicked(){self.prepare(root,Action::Up);}});ui.label(tr("Command to execute inside the development container"));ui.add(egui::TextEdit::singleline(&mut self.command).char_limit(4096));ui.label(tr("Container command arguments (one per line; no host shell)"));ui.add(egui::TextEdit::multiline(&mut self.args).char_limit(32*4096).desired_rows(4));if ui.button(tr("Review Dev Container Exec")).clicked(){self.prepare(root,Action::Exec{command:self.command.clone(),args:self.args.lines().map(str::to_string).collect()});}});if let Some(plan)=&self.plan{let mut preview=plan.preview();ui.add(egui::TextEdit::multiline(&mut preview).interactive(false).font(egui::TextStyle::Monospace));ui.checkbox(&mut self.confirm,tr("I reviewed the command and configuration; authorize Dev Container execution"));if ui.add_enabled(trusted&&self.confirm&&self.stop.is_none(),egui::Button::new(tr("Execute Reviewed Dev Container Command"))).clicked(){self.start(root,trusted);}}}
if self.stop.is_some(){ui.spinner();if ui.button(tr("Cancel Dev Container Request")).clicked(){if let Some(stop)=&self.stop{stop.cancel();}}}if let Some(error)=&self.error{ui.colored_label(rc_kit::pal(ui).error,error);}ui.label(tr("Dev Container output preview is limited to 16,384 characters"));ui.add(egui::TextEdit::multiline(&mut self.output).interactive(false).font(egui::TextStyle::Monospace).desired_rows(14));});
    }
}
impl Drop for DevContainers {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.cancel();
        }
    }
}
pub fn show(app: &mut crate::app::CrabCraftApp, ui: &mut egui::Ui) {
    if crate::ui::toolwindows::header(ui, tr("Dev Containers")) {
        app.toggle_tool(crate::toolwindow::ToolWindow::DevContainers);
    }
    let root = app.workspace.as_ref().map(|w| w.ws.root.clone());
    let trusted = root
        .as_ref()
        .is_some_and(|r| app.settings.trusted_projects.contains(r));
    app.dev_containers.show(ui, root, trusted);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path) {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::create_dir(root.join(".devcontainer")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"devcontainer_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "fn main(){println!(\"real Rust workspace\");}",
        )
        .unwrap();
        std::fs::write(
            root.join("build.rs"),
            "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}",
        )
        .unwrap();
        let config = format!(
            "{{\n// Real JSONC configuration\n\"name\":\"Rust native Dev Container fixture\",\"image\":\"local-rust:latest\",\"initializeCommand\":[\"/usr/bin/touch\",{}]}}",
            serde_json::to_string(&root.join("host-hook-ran").to_string_lossy()).unwrap()
        );
        std::fs::write(root.join(".devcontainer/devcontainer.json"), config).unwrap();
    }
    #[cfg(unix)]
    fn cli(root: &Path, slow: bool) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join(if slow {
            "slow-devcontainer"
        } else {
            "fake-devcontainer"
        });
        let text = format!(
            "#!/usr/bin/python3\nimport json,sys,time\nwith open({:?},'a') as f:f.write(json.dumps(sys.argv[1:])+'\\n')\n{}\nprint(json.dumps({{'outcome':'success','configuration':{{'name':'Rust native Dev Container fixture'}}}}))\n",
            root.join("devcontainer-wire").to_string_lossy(),
            if slow { "time.sleep(60)" } else { "" }
        );
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path.to_string_lossy().into()
    }
    #[test]
    fn devcontainer_argv_keeps_commands_literal_and_forbids_remote_daemon_uri() {
        let config = Config {
            program: "devcontainer".into(),
            docker: "docker".into(),
            host: String::new(),
        };
        let root = PathBuf::from("/tmp/project");
        let plan = Plan::new(
            config.clone(),
            root.clone(),
            Action::Exec {
                command: "cargo".into(),
                args: vec!["$(touch host); literal".into()],
            },
        )
        .unwrap();
        assert_eq!(plan.args.last().unwrap(), "$(touch host); literal");
        assert_eq!(plan.args[plan.args.len() - 3], "--");
        assert!(
            Plan::new(
                config.clone(),
                root,
                Action::Exec {
                    command: "--privileged".into(),
                    args: vec![]
                }
            )
            .is_err()
        );
        assert!(
            Config {
                host: "tcp://host:2375".into(),
                ..config
            }
            .validate()
            .is_err()
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn devcontainer_backend_trust_confirmation_and_cancellation_do_not_run_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fixture(&root);
        let config = Config {
            program: cli(&root, false),
            docker: "docker".into(),
            host: String::new(),
        };
        let plan = Plan::new(config, root.clone(), Action::Up).unwrap();
        assert_eq!(
            execute(plan.clone(), false, true, rc_dotnet::CancelToken::new()).await,
            Err(Error::Trust)
        );
        assert_eq!(
            execute(plan.clone(), true, false, rc_dotnet::CancelToken::new()).await,
            Err(Error::Confirm)
        );
        assert!(!root.join("devcontainer-wire").exists());
        execute(plan, true, true, rc_dotnet::CancelToken::new())
            .await
            .unwrap();
        assert!(
            std::fs::read_to_string(root.join("devcontainer-wire"))
                .unwrap()
                .contains("up")
        );
        assert!(!root.join("build-ran").exists() && !root.join("host-hook-ran").exists());
        let config = Config {
            program: cli(&root, true),
            docker: "docker".into(),
            host: String::new(),
        };
        let plan = Plan::new(config, root, Action::Read).unwrap();
        let stop = rc_dotnet::CancelToken::new();
        let task = tokio::spawn(execute(plan, true, false, stop.clone()));
        tokio::time::sleep(Duration::from_millis(30)).await;
        stop.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .unwrap()
                .unwrap(),
            Err(Error::Cancelled)
        );
    }
    #[cfg(unix)]
    #[test]
    fn native_devcontainer_up_requires_explicit_trust_and_separate_confirmation() {
        use egui_kittest::kittest::Queryable;
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fixture(&root);
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace(&root);
        app.dev_containers.program = cli(&root, false);
        app.dock
            .sizes
            .insert(crate::toolwindow::ToolWindow::DevContainers, 900.0);
        let mut h = crate::screens::harness(app, egui::vec2(1900.0, 1500.0));
        h.get_by_label(tr("Dev Containers")).click();
        h.run_steps(2);
        h.get_by_label(tr("Check Dev Container CLI")).click();
        h.run_steps(2);
        assert!(!root.join("devcontainer-wire").exists());
        h.state_mut().trust_project_tools_for_test();
        h.run_steps(2);
        h.get_by_label(tr("Review Dev Container Up")).click();
        h.run_steps(2);
        h.get_by_label(tr("Execute Reviewed Dev Container Command"))
            .click();
        h.run_steps(2);
        assert!(!root.join("devcontainer-wire").exists());
        h.get_by_label(tr(
            "I reviewed the command and configuration; authorize Dev Container execution",
        ))
        .click();
        h.run_steps(2);
        h.get_by_label(tr("Execute Reviewed Dev Container Command"))
            .click();
        h.run_steps(2);
        let start = std::time::Instant::now();
        while h.state().dev_containers.stop.is_some() && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            h.state().dev_containers.error.is_none(),
            "{:?}",
            h.state().dev_containers.error
        );
        assert!(
            std::fs::read_to_string(root.join("devcontainer-wire"))
                .unwrap()
                .contains("up")
        );
        assert!(!root.join("host-hook-ran").exists() && !root.join("build-ran").exists());
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "Requires separately installed official Dev Container CLI; reads JSONC using a deterministic empty Docker catalog, never starts a container"]
    fn official_cli_reads_real_rust_jsonc_without_running_lifecycle_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let program =
            std::env::var("CRABCRAFT_TEST_DEVCONTAINER").expect("explicit official CLI fixture");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fixture(&root);
        let docker = root.join("empty-docker");
        std::fs::write(
            &docker,
            "#!/usr/bin/python3\nimport sys\nif 'version' in sys.argv: print('29.9.3')\n",
        )
        .unwrap();
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = Config {
            program,
            docker: docker.to_string_lossy().into(),
            host: String::new(),
        };
        let invoke = |action| {
            rc_core::Runtime::block_on(execute(
                Plan::new(config.clone(), root.clone(), action).unwrap(),
                true,
                false,
                rc_dotnet::CancelToken::new(),
            ))
            .unwrap()
        };
        let version = invoke(Action::Version);
        assert!(!version.trim().is_empty());
        let output = invoke(Action::Read);
        assert!(output.contains("Rust native Dev Container fixture"));
        assert!(output.contains("initializeCommand"));
        assert!(!root.join("host-hook-ran").exists() && !root.join("build-ran").exists());
    }
}
