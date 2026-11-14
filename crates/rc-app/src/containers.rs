//! Native, explicit Docker CLI controls. No daemon access on project open.
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Trust,
    Input,
    Confirm,
    Io,
    Exit(String),
    Output,
    Cancelled,
    Stale,
}
impl Error {
    fn message(&self) -> String {
        match self {
            Self::Trust => tr("Trust this project before running project tools or code").into(),
            Self::Input => tr("Choose a Docker executable, valid resource ID and project-local build paths").into(),
            Self::Confirm => tr("Review and confirm this Docker command before executing it").into(),
            Self::Io => tr("Could not start or communicate with the Docker CLI").into(),
            Self::Exit(text) => trf("Docker CLI failed: {message}", &[("message", text)]),
            Self::Output => tr("Docker output or operation time limit exceeded").into(),
            Self::Cancelled => tr("Docker request cancelled; daemon work or containers may still need explicit Stop or Remove").into(),
            Self::Stale => tr("Docker settings or project changed; prepare the command again").into(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
struct Config {
    program: String,
    host: String,
}
impl Config {
    fn validate(&self) -> Result<(), Error> {
        let path = Path::new(&self.program);
        if self.program.is_empty()
            || self.program.len() > 4096
            || self.program.contains('\0')
            || (!path.is_absolute() && path.components().count() != 1)
            || self.host.len() > 4096
            || self.host.contains(['\0', '\n', '\r'])
            || (!self.host.is_empty() && !self.host.starts_with("unix://") && !self.host.starts_with("npipe://"))
        {
            return Err(Error::Input);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq)]
enum Action {
    Info,
    Containers,
    Images,
    Build { context: String, dockerfile: String, tag: String },
    Run { image: String, args: Vec<String>, network: bool },
    Logs(String),
    Stop(String),
    Remove(String),
}
impl Action {
    fn write(&self) -> bool {
        matches!(self, Self::Build { .. } | Self::Run { .. } | Self::Stop(_) | Self::Remove(_))
    }
}
fn resource(s: &str) -> bool {
    (12..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn image(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.starts_with('-') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-/:@".contains(&b))
}
#[derive(Clone)]
struct Plan {
    config: Config,
    root: PathBuf,
    action: Action,
    args: Vec<String>,
    context: Option<(PathBuf, PathBuf)>,
}
impl Plan {
    fn prepare(config: Config, root: PathBuf, action: Action) -> Result<Self, Error> {
        config.validate()?;
        if !root.is_absolute() {
            return Err(Error::Input);
        }
        let mut context = None;
        let args: Vec<String> = match &action {
            Action::Info => vec!["version".into(), "--format".into(), "{{json .}}".into()],
            Action::Containers => vec!["container".into(), "ls".into(), "--all".into(), "--no-trunc".into(), "--format".into(), "{{json .}}".into()],
            Action::Images => vec!["image".into(), "ls".into(), "--no-trunc".into(), "--format".into(), "{{json .}}".into()],
            Action::Build { context: dir, dockerfile, tag } => {
                if !image(tag) || dir.is_empty() || dockerfile.is_empty() {
                    return Err(Error::Input);
                }
                let dir = PathBuf::from(dir);
                let file = PathBuf::from(dockerfile);
                if !dir.is_absolute() || !file.is_absolute() {
                    return Err(Error::Input);
                }
                context = Some((dir.clone(), file.clone()));
                vec![
                    "build".into(),
                    "--network=none".into(),
                    "--pull=false".into(),
                    "--tag".into(),
                    tag.clone(),
                    "--file".into(),
                    file.to_string_lossy().into(),
                    dir.to_string_lossy().into(),
                ]
            }
            Action::Run { image: reference, args, network } => {
                if !image(reference) || args.len() > 32 || args.iter().any(|s| s.len() > 4096 || s.contains('\0')) {
                    return Err(Error::Input);
                }
                let mut argv = vec![
                    "run".into(),
                    "--detach".into(),
                    "--pull=never".into(),
                    if *network { "--network=bridge".into() } else { "--network=none".into() },
                    "--label=dev.crabcraft.explicit=true".into(),
                    "--".into(),
                    reference.clone(),
                ];
                argv.extend(args.clone());
                argv
            }
            Action::Logs(id) | Action::Stop(id) | Action::Remove(id) => {
                if !resource(id) {
                    return Err(Error::Input);
                }
                match &action {
                    Action::Logs(_) => vec!["container".into(), "logs".into(), "--tail=200".into(), "--timestamps".into(), id.clone()],
                    Action::Stop(_) => vec!["container".into(), "stop".into(), "--time=10".into(), id.clone()],
                    _ => vec!["container".into(), "rm".into(), id.clone()],
                }
            }
        };
        Ok(Self { config, root, action, args, context })
    }
    fn preview(&self) -> String {
        let mut argv = vec![self.config.program.clone()];
        if !self.config.host.is_empty() {
            argv.extend(["--host".into(), self.config.host.clone()]);
        }
        argv.extend(self.args.clone());
        argv.into_iter().map(|s| format!("{s:?}")).collect::<Vec<_>>().join(" ")
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
async fn bytes<R: tokio::io::AsyncRead + Unpin>(reader: R) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    reader.take(4 * 1024 * 1024 + 1).read_to_end(&mut out).await.map_err(|_| Error::Io)?;
    if out.len() > 4 * 1024 * 1024 {
        return Err(Error::Output);
    }
    Ok(out)
}
async fn execute(plan: Plan, trusted: bool, confirmed: bool, stop: rc_dotnet::CancelToken) -> Result<String, Error> {
    if !trusted {
        return Err(Error::Trust);
    }
    if plan.action.write() && !confirmed {
        return Err(Error::Confirm);
    }
    if stop.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if let Some((context, file)) = &plan.context {
        let root = plan.root.canonicalize().map_err(|_| Error::Input)?;
        let context = context.canonicalize().map_err(|_| Error::Input)?;
        let file = file.canonicalize().map_err(|_| Error::Input)?;
        if !context.starts_with(&root) || !file.starts_with(&context) || !context.is_dir() || !std::fs::metadata(&file).map_err(|_| Error::Input)?.is_file() {
            return Err(Error::Input);
        }
    }
    let neutral = tempfile::tempdir().map_err(|_| Error::Io)?;
    let mut cmd = tokio::process::Command::new(&plan.config.program);
    if !plan.config.host.is_empty() {
        cmd.arg("--host").arg(&plan.config.host);
    }
    cmd.args(&plan.args)
        .current_dir(neutral.path())
        .env_remove("DOCKER_CONTEXT")
        .env_remove("DOCKER_HOST")
        .env_remove("DOCKER_TLS_VERIFY")
        .env_remove("DOCKER_CERT_PATH")
        .stdin(Stdio::null())
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
        let (out, err) = tokio::try_join!(bytes(stdout), bytes(stderr))?;
        let status = child.wait().await.map_err(|_| Error::Io)?;
        if !status.success() {
            return Err(Error::Exit(String::from_utf8_lossy(&err).chars().take(4096).collect()));
        }
        let mut text = String::from_utf8(out).map_err(|_| Error::Output)?;
        let err = String::from_utf8_lossy(&err);
        if !err.is_empty() {
            text.push('\n');
            text.extend(err.chars().take(16384));
        }
        Ok(text)
    };
    let result = tokio::select! {result=tokio::time::timeout(Duration::from_secs(if matches!(plan.action,Action::Build{..}){600}else{60}),work)=>result.map_err(|_|Error::Output)?,()=stop.cancelled()=>Err(Error::Cancelled)};
    drop(group);
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    result
}
#[derive(Clone)]
struct Row {
    id: String,
    name: String,
    image: String,
    status: String,
}
fn rows(text: &str) -> Result<Vec<Row>, Error> {
    let mut result = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        if result.len() >= 1000 {
            return Err(Error::Output);
        }
        let value: Value = serde_json::from_str(line).map_err(|_| Error::Output)?;
        let field = |key| value.get(key).and_then(Value::as_str).filter(|s| s.len() <= 4096).ok_or(Error::Output).map(str::to_string);
        let id = field("ID")?;
        if !resource(&id) {
            return Err(Error::Output);
        }
        result.push(Row { id, name: field("Names")?, image: field("Image")?, status: field("Status")? });
    }
    Ok(result)
}
struct Event {
    action: Action,
    result: Result<String, Error>,
}
pub struct Containers {
    pub program: String,
    pub host: String,
    pub context: String,
    pub dockerfile: String,
    pub tag: String,
    pub image: String,
    pub args: String,
    pub network: bool,
    inbox: Inbox<Event>,
    stop: Option<rc_dotnet::CancelToken>,
    plan: Option<Plan>,
    confirm: bool,
    output: String,
    error: Option<String>,
    rows: Vec<Row>,
    selected: String,
}
impl Containers {
    pub fn new(waker: Waker) -> Self {
        Self {
            program: "docker".into(),
            host: String::new(),
            context: String::new(),
            dockerfile: String::new(),
            tag: "crabcraft-local:latest".into(),
            image: String::new(),
            args: String::new(),
            network: false,
            inbox: Inbox::new(waker),
            stop: None,
            plan: None,
            confirm: false,
            output: String::new(),
            error: None,
            rows: Vec::new(),
            selected: String::new(),
        }
    }
    fn config(&self) -> Config {
        Config { program: self.program.clone(), host: self.host.clone() }
    }
    fn prepare(&mut self, root: &Path, action: Action) {
        self.confirm = false;
        match Plan::prepare(self.config(), root.to_path_buf(), action) {
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
        let action = plan.action.clone();
        rc_core::Runtime::spawn(async move {
            out.send(Event { action, result: execute(plan, trusted, confirmed, stop).await });
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
        for event in self.inbox.drain() {
            self.stop = None;
            match event.result {
                Ok(output) => {
                    self.error = None;
                    if matches!(event.action, Action::Containers) {
                        match rows(&output) {
                            Ok(rows) => self.rows = rows,
                            Err(error) => self.error = Some(error.message()),
                        }
                    }
                    self.output = output.chars().take(16_384).collect();
                }
                Err(error) => self.error = Some(error.message()),
            }
        }
    }
    fn show(&mut self, ui: &mut egui::Ui, root: Option<PathBuf>, trusted: bool) {
        egui::ScrollArea::vertical().id_salt("docker_body").show(ui,|ui|{
ui.label(tr("Docker commands are explicit. Access to a Docker daemon can grant host-level privileges; use only a daemon and images you trust."));ui.label(tr("No Docker process starts on project open. Docker installation, Compose, registry login, volumes and port forwarding are unsupported here."));if !trusted{ui.colored_label(rc_kit::pal(ui).warn,tr("Trust this project before running project tools or code"));}
ui.add_enabled_ui(self.stop.is_none(),|ui|{ui.label(tr("Docker executable"));ui.add(egui::TextEdit::singleline(&mut self.program).char_limit(4096));ui.label(tr("Docker local socket URI (empty uses the CLI default)"));ui.add(egui::TextEdit::singleline(&mut self.host).char_limit(4096));});
if let Some(root)=&root{ui.add_enabled_ui(trusted&&self.stop.is_none(),|ui|{ui.horizontal_wrapped(|ui|{for(action,label)in[(Action::Info,tr("Check Docker Engine")),(Action::Containers,tr("List Docker Containers")),(Action::Images,tr("List Docker Images"))]{if ui.button(label).clicked(){self.prepare(root,action);self.start(root,trusted);}}});
ui.collapsing(tr("Build Docker image"),|ui|{if self.context.is_empty(){self.context=root.to_string_lossy().into();self.dockerfile=root.join("Dockerfile").to_string_lossy().into();}ui.label(tr("Absolute build context path"));ui.add(egui::TextEdit::singleline(&mut self.context).char_limit(4096));ui.label(tr("Absolute Dockerfile path"));ui.add(egui::TextEdit::singleline(&mut self.dockerfile).char_limit(4096));ui.label(tr("Docker image tag"));ui.add(egui::TextEdit::singleline(&mut self.tag).char_limit(256));if ui.button(tr("Review Docker Build")).clicked(){self.prepare(root,Action::Build{context:self.context.clone(),dockerfile:self.dockerfile.clone(),tag:self.tag.clone()});}});
ui.collapsing(tr("Run local Docker image"),|ui|{ui.label(tr("Existing local image (pull is disabled)"));ui.add(egui::TextEdit::singleline(&mut self.image).char_limit(256));ui.label(tr("Container command arguments (one per line; no host shell)"));ui.add(egui::TextEdit::multiline(&mut self.args).char_limit(32*4096).desired_rows(3));ui.checkbox(&mut self.network,tr("Allow container bridge networking"));if ui.button(tr("Review Docker Run")).clicked(){self.prepare(root,Action::Run{image:self.image.clone(),args:self.args.lines().map(str::to_string).collect(),network:self.network});}});
for row in self.rows.clone(){if ui.selectable_label(self.selected==row.id,format!("{} | {} | {}",row.name,row.image,row.status)).clicked(){self.selected=row.id;}}if resource(&self.selected){ui.horizontal_wrapped(|ui|{if ui.button(tr("Docker Container Logs")).clicked(){self.prepare(root,Action::Logs(self.selected.clone()));self.start(root,trusted);}
if ui.button(tr("Review Docker Stop")).clicked(){self.prepare(root,Action::Stop(self.selected.clone()));}
if ui.button(tr("Review Docker Remove")).clicked(){self.prepare(root,Action::Remove(self.selected.clone()));}});}
});if let Some(plan)=&self.plan{ui.separator();let mut preview=plan.preview();ui.add(egui::TextEdit::multiline(&mut preview).interactive(false).font(egui::TextStyle::Monospace));ui.label(tr("Build can send the whole context to the daemon and execute Dockerfile instructions. Run executes image code. Stop and Remove affect the selected daemon resource."));ui.checkbox(&mut self.confirm,tr("I reviewed and authorize this Docker command"));if ui.add_enabled(trusted&&self.confirm&&self.stop.is_none(),egui::Button::new(tr("Execute Reviewed Docker Command"))).clicked(){self.start(root,trusted);}}}
if self.stop.is_some(){ui.spinner();if ui.button(tr("Cancel Docker Request")).clicked()&& let Some(stop)=&self.stop{stop.cancel();}}
if let Some(error)=&self.error{ui.colored_label(rc_kit::pal(ui).error,error);}ui.separator();ui.label(tr("Docker output preview is limited to 16,384 characters"));ui.add(egui::TextEdit::multiline(&mut self.output).interactive(false).font(egui::TextStyle::Monospace).desired_rows(12));});
    }
}
impl Drop for Containers {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.cancel();
        }
    }
}
pub fn show(app: &mut crate::app::CrabCraftApp, ui: &mut egui::Ui) {
    if crate::ui::toolwindows::header(ui, tr("Docker")) {
        app.toggle_tool(crate::toolwindow::ToolWindow::Containers);
    }
    let root = app.workspace.as_ref().map(|w| w.ws.root.clone());
    let trusted = root.as_ref().is_some_and(|r| app.settings.trusted_projects.contains(r));
    app.containers.show(ui, root, trusted);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rust_project(root: &Path) {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"docker_native_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n").unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "fn main(){println!(\"42\");if std::env::args().any(|arg|arg==\"wait\"){std::thread::sleep(std::time::Duration::from_secs(120));}}\n",
        )
        .unwrap();
        std::fs::write(root.join("build.rs"), "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}").unwrap();
        std::fs::write(root.join("Dockerfile"), "FROM scratch\nCOPY fixture /fixture\nENTRYPOINT [\"/fixture\"]\n").unwrap();
    }
    #[cfg(unix)]
    fn fake(root: &Path, sleep: bool) -> Config {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join(if sleep { "slow-docker" } else { "fake-docker" });
        let marker = serde_json::to_string(&root.join("docker-wire").to_string_lossy()).unwrap();
        let script = format!(
            "#!/usr/bin/python3\nimport sys,json,time\nwith open({marker},'a') as f: f.write(json.dumps(sys.argv[1:])+'\\n')\n{}\nif 'ls' in sys.argv and 'container' in sys.argv: print(json.dumps({{'ID':'{}','Names':'rust-fixture','Image':'local:latest','Status':'Exited (0)'}}))\nelse: print(json.dumps({{'Client':{{'Version':'fixture'}},'Server':{{'Version':'fixture'}}}}))\n",
            if sleep { "time.sleep(60)" } else { "" },
            "a".repeat(64)
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Config { program: path.to_string_lossy().into(), host: String::new() }
    }
    #[test]
    fn docker_plans_use_typed_argv_without_shell_or_remote_contexts() {
        let root = PathBuf::from("/tmp/project");
        let config = Config { program: "docker".into(), host: "unix:///tmp/docker.sock".into() };
        assert!(Plan::prepare(config.clone(), root.clone(), Action::Stop("--all".into())).is_err());
        assert!(Plan::prepare(config.clone(), root.clone(), Action::Run { image: "--privileged".into(), args: vec![], network: false }).is_err());
        let plan = Plan::prepare(
            config.clone(),
            root.clone(),
            Action::Run { image: "local:latest".into(), args: vec!["$(touch marker); literal".into()], network: false },
        )
        .unwrap();
        assert!(plan.args.contains(&"--pull=never".into()));
        assert!(plan.args.contains(&"--network=none".into()));
        assert_eq!(plan.args.last().unwrap(), "$(touch marker); literal");
        assert!(Config { host: "tcp://untrusted:2375".into(), ..config }.validate().is_err());
        assert!(rows("{bad").is_err());
        assert!(rows(&serde_json::json!({"ID":"--all","Names":"bad","Image":"bad","Status":"bad"}).to_string()).is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn docker_trust_confirmation_build_scope_and_cancel_are_enforced_in_backend() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        rust_project(&root);
        let config = fake(&root, false);
        let build = Plan::prepare(
            config.clone(),
            root.clone(),
            Action::Build { context: root.to_string_lossy().into(), dockerfile: root.join("Dockerfile").to_string_lossy().into(), tag: "local:latest".into() },
        )
        .unwrap();
        assert_eq!(execute(build.clone(), false, true, rc_dotnet::CancelToken::new()).await, Err(Error::Trust));
        assert_eq!(execute(build.clone(), true, false, rc_dotnet::CancelToken::new()).await, Err(Error::Confirm));
        assert!(!root.join("docker-wire").exists());
        let outside = tempfile::tempdir().unwrap();
        let bad = Plan::prepare(
            config.clone(),
            root.clone(),
            Action::Build {
                context: outside.path().to_string_lossy().into(),
                dockerfile: root.join("Dockerfile").to_string_lossy().into(),
                tag: "local:latest".into(),
            },
        )
        .unwrap();
        assert_eq!(execute(bad, true, true, rc_dotnet::CancelToken::new()).await, Err(Error::Input));
        execute(build, true, true, rc_dotnet::CancelToken::new()).await.unwrap();
        let wire = std::fs::read_to_string(root.join("docker-wire")).unwrap();
        assert!(wire.contains("--network=none") && wire.contains("--pull=false"));
        assert!(!root.join("build-ran").exists());
        let stop = rc_dotnet::CancelToken::new();
        let plan = Plan::prepare(fake(&root, true), root, Action::Info).unwrap();
        let task = tokio::spawn(execute(plan, true, false, stop.clone()));
        tokio::time::sleep(Duration::from_millis(30)).await;
        stop.cancel();
        assert_eq!(tokio::time::timeout(Duration::from_secs(3), task).await.unwrap().unwrap(), Err(Error::Cancelled));
    }
    #[cfg(unix)]
    #[test]
    fn native_docker_actions_are_disabled_untrusted_and_writes_require_review() {
        use egui_kittest::kittest::Queryable;
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        rust_project(&root);
        let config = fake(&root, false);
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(&root);
        app.containers.program = config.program;
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::Containers, 900.0);
        let mut h = crate::screens::harness(app, egui::vec2(1900.0, 1500.0));
        h.get_by_label(tr("Docker")).click();
        h.run_steps(2);
        h.get_by_label(tr("Check Docker Engine")).click();
        h.run_steps(2);
        assert!(!root.join("docker-wire").exists());
        h.state_mut().trust_project_tools_for_test();
        h.run_steps(2);
        let wait = |h: &mut egui_kittest::Harness<'_, crate::app::CrabCraftApp>| {
            let start = std::time::Instant::now();
            while h.state().containers.stop.is_some() && start.elapsed() < Duration::from_secs(15) {
                h.run_steps(2);
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(h.state().containers.stop.is_none());
            assert!(h.state().containers.error.is_none(), "{:?}", h.state().containers.error);
        };
        h.get_by_label(tr("List Docker Containers")).click();
        h.run_steps(2);
        wait(&mut h);
        assert_eq!(h.state().containers.rows.len(), 1);
        h.state_mut().containers.selected = "a".repeat(64);
        h.run_steps(2);
        let before = std::fs::read_to_string(root.join("docker-wire")).unwrap();
        h.get_by_label(tr("Review Docker Stop")).click();
        h.run_steps(2);
        h.get_by_label(tr("Execute Reviewed Docker Command")).click();
        h.run_steps(2);
        assert_eq!(std::fs::read_to_string(root.join("docker-wire")).unwrap(), before);
        h.get_by_label(tr("I reviewed and authorize this Docker command")).click();
        h.run_steps(2);
        h.get_by_label(tr("Execute Reviewed Docker Command")).click();
        h.run_steps(2);
        wait(&mut h);
        assert!(std::fs::read_to_string(root.join("docker-wire")).unwrap().contains("stop"));
        assert!(!root.join("build-ran").exists());
    }
    #[test]
    #[ignore = "Requires an explicitly provided disposable rootless Docker daemon; never pulls an image"]
    fn real_rootless_engine_builds_and_runs_a_static_rust_project() {
        let program = std::env::var("CRABCRAFT_TEST_DOCKER").expect("explicit fixture CLI");
        let host = std::env::var("CRABCRAFT_TEST_DOCKER_HOST").expect("explicit fixture Unix socket");
        assert!(host.starts_with("unix:///tmp/"));
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        rust_project(&root);
        let output = std::process::Command::new("rustc")
            .args(["-C", "target-feature=+crt-static", "-C", "relocation-model=static"])
            .arg(root.join("src/main.rs"))
            .arg("-o")
            .arg(root.join("fixture"))
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let config = Config { program, host };
        let invoke = |action| {
            let plan = Plan::prepare(config.clone(), root.clone(), action).unwrap();
            rc_core::Runtime::block_on(execute(plan, true, true, rc_dotnet::CancelToken::new())).unwrap()
        };
        let version = invoke(Action::Info);
        assert!(version.contains("Server"));
        let tag = format!("crabcraft-rust-fixture:{}", std::process::id());
        invoke(Action::Build { context: root.to_string_lossy().into(), dockerfile: root.join("Dockerfile").to_string_lossy().into(), tag: tag.clone() });
        let id = invoke(Action::Run { image: tag, args: vec![], network: false }).trim().to_string();
        assert!(resource(&id), "{id}");
        let mut found = false;
        for _ in 0..100 {
            let output = invoke(Action::Containers);
            if rows(&output).unwrap().iter().any(|row| row.id == id && row.status.contains("Exited (0)")) {
                found = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(found);
        assert!(invoke(Action::Logs(id.clone())).contains("42"));
        invoke(Action::Remove(id));
        assert!(!root.join("build-ran").exists());
    }
}
