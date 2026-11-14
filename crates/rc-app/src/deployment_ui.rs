//! Native reviewed deployments, separate from the existing remote text editor and SSH terminal.
use crate::{
    deploy_ftps,
    deployment::{self, Config, Error, Preview, Receipt},
    remote_transfer,
};
use rc_core::{Inbox, Waker};
use rc_dotnet::process::CancelToken;
use rc_kit::i18n::{tr, trf};
use std::path::PathBuf;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Sftp,
    Ftps,
}
enum Reply {
    Preview(Box<Preview>),
    Applied(Receipt),
}
struct Job {
    cancel: CancelToken,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
pub struct Deployments {
    pub protocol: Protocol,
    pub local: String,
    pub remote: String,
    pub create: bool,
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub ca: String,
    pub pin: String,
    pub preview: Option<Preview>,
    pub receipt: Option<Receipt>,
    pub error: Option<Error>,
    job: Option<Job>,
    inbox: Inbox<(u64, Result<Reply, Error>)>,
    generation: u64,
    acknowledged: bool,
}
impl Deployments {
    pub fn new(waker: Waker) -> Self {
        Self {
            protocol: Protocol::Sftp,
            local: String::new(),
            remote: String::new(),
            create: true,
            host: String::new(),
            port: "21".into(),
            user: String::new(),
            password: String::new(),
            ca: String::new(),
            pin: String::new(),
            preview: None,
            receipt: None,
            error: None,
            job: None,
            inbox: Inbox::new(waker),
            generation: 0,
            acknowledged: false,
        }
    }
    pub fn pending(&self) -> bool {
        self.job.is_some()
    }
    pub fn cancel(&mut self) {
        self.job = None;
        self.generation = self.generation.wrapping_add(1);
        self.preview = None;
        self.acknowledged = false;
    }
    pub fn poll(&mut self, trusted: bool) {
        if !trusted {
            self.cancel();
            return;
        }
        for (generation, result) in self.inbox.drain() {
            if generation != self.generation {
                continue;
            }
            self.job = None;
            match result {
                Ok(Reply::Preview(preview)) => {
                    self.preview = Some(*preview);
                    self.acknowledged = false;
                }
                Ok(Reply::Applied(receipt)) => {
                    self.receipt = Some(receipt);
                    self.preview = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    fn config(&self, ssh: Option<remote_transfer::Config>) -> Result<Config, Error> {
        match self.protocol {
            Protocol::Sftp => ssh.map(Config::Sftp).ok_or(Error::Input),
            Protocol::Ftps => Ok(Config::Ftps(deploy_ftps::Config::new(
                self.host.clone(),
                self.port.parse().ok().filter(|p| *p > 0).ok_or(Error::Input)?,
                self.user.clone(),
                self.password.clone(),
                if self.ca.is_empty() { None } else { Some(PathBuf::from(&self.ca)) },
                self.pin.clone(),
            ))),
        }
    }
    fn spawn(&mut self, work: impl FnOnce(CancelToken) -> Result<Reply, Error> + Send + 'static) {
        if self.job.is_some() {
            return;
        }
        self.error = None;
        self.receipt = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let sender = self.inbox.sender();
        let cancel = CancelToken::new();
        let token = cancel.clone();
        match std::thread::Builder::new().name("rc-deployment".into()).spawn(move || sender.send((generation, work(token)))) {
            Ok(_) => self.job = Some(Job { cancel }),
            Err(_) => self.error = Some(Error::Io),
        }
    }
    fn preview(&mut self, config: Config, trusted: bool) {
        if !trusted {
            self.error = Some(Error::Trust);
            return;
        }
        let local = PathBuf::from(&self.local);
        let remote = self.remote.clone();
        let create = self.create;
        self.preview = None;
        self.spawn(move |cancel| deployment::preview(config, &local, remote, create, trusted, cancel).map(|preview| Reply::Preview(Box::new(preview))));
    }
    fn apply(&mut self, config: Config, trusted: bool) {
        if !trusted {
            self.error = Some(Error::Trust);
            return;
        }
        if !self.acknowledged {
            return;
        }
        let Some(preview) = self.preview.take() else {
            return;
        };
        self.spawn(move |cancel| deployment::apply(config, preview, trusted, cancel).map(Reply::Applied));
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, ssh: Option<remote_transfer::Config>, trusted: bool, legacy_busy: bool) {
        ui.separator();
        ui.heading(tr("Reviewed file deployment"));
        ui.small(tr("Existing or new regular files, including binaries, up to 32 MiB. Preview never uploads; confirmation transfers the frozen snapshot. No directory sync or automatic deployment."));
        ui.small(tr("Deployment reads disk files. Save editor buffers first to include unsaved edits."));
        let busy = self.pending() || legacy_busy;
        let before = (
            self.protocol,
            self.local.clone(),
            self.remote.clone(),
            self.create,
            self.host.clone(),
            self.port.clone(),
            self.user.clone(),
            self.password.clone(),
            self.ca.clone(),
            self.pin.clone(),
        );
        ui.add_enabled_ui(!busy,|ui|{ui.horizontal(|ui|{ui.selectable_value(&mut self.protocol,Protocol::Sftp,tr("SFTP deployment"));ui.selectable_value(&mut self.protocol,Protocol::Ftps,tr("FTPS deployment (explicit TLS)"));});
 if self.protocol==Protocol::Sftp{ui.small(tr("SFTP deployment uses the verified SSH host, key and known_hosts settings above."));}else{ui.small(tr("Plain FTP and implicit FTPS are unavailable. Verified TLS is mandatory on the control and data connections."));egui::Grid::new("deployment_ftps_config").num_columns(4).show(ui,|ui|{field(ui,tr("FTPS host"),&mut self.host,253,false);field(ui,tr("FTPS port"),&mut self.port,5,false);ui.end_row();field(ui,tr("FTPS user"),&mut self.user,512,false);field(ui,tr("FTPS password"),&mut self.password,4096,true);ui.end_row();field(ui,tr("FTPS CA certificate (optional)"),&mut self.ca,8192,false);field(ui,tr("FTPS SPKI pin (optional)"),&mut self.pin,64,false);ui.end_row();});ui.small(tr("With no CA file, the system certificate store is used. A sha256// SPKI pin is additional verification, never a replacement for CA and hostname checks. Credentials stay in memory."));}
 field(ui,tr("Local deployment source"),&mut self.local,8192,false);field(ui,tr("Deployment destination path"),&mut self.remote,4096,false);ui.checkbox(&mut self.create,if self.protocol==Protocol::Sftp{tr("Create new SFTP destination (must remain absent)")}else{tr("Create new FTPS file (server chooses a unique final name)")});
 });
        let changed = before
            != (
                self.protocol,
                self.local.clone(),
                self.remote.clone(),
                self.create,
                self.host.clone(),
                self.port.clone(),
                self.user.clone(),
                self.password.clone(),
                self.ca.clone(),
                self.pin.clone(),
            );
        let current = self.config(ssh);
        if changed || self.preview.as_ref().is_some_and(|preview| current.as_ref().map_or(true, |config| *config != preview.config)) {
            self.preview = None;
            self.acknowledged = false;
            self.receipt = None;
            self.error = None;
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(trusted && !busy, egui::Button::new(tr("Preview deployment"))).clicked() {
                match current.clone() {
                    Ok(config) => self.preview(config, trusted),
                    Err(error) => self.error = Some(error),
                }
            }
            if self.pending() {
                ui.spinner();
                if ui.button(tr("Cancel deployment")).clicked() {
                    self.cancel();
                    self.error = Some(Error::Cancelled);
                }
            }
            if ui.add_enabled(!busy && !self.password.is_empty(), egui::Button::new(tr("Clear FTPS password"))).clicked() {
                self.password.clear();
                self.preview = None;
            }
        });
        if !trusted {
            ui.colored_label(rc_kit::pal(ui).warn, tr("Trust the project before connecting or deploying files."));
        }
        if let Some(error) = &self.error {
            ui.colored_label(rc_kit::pal(ui).error, error_text(error));
        }
        if let Some(preview) = &self.preview {
            ui.separator();
            ui.label(tr("Review deployment before confirmation"));
            ui.monospace(preview.source.path.display().to_string());
            ui.monospace(match &preview.config {
                Config::Sftp(config) => format!("SFTP {}:{}", config.host, config.port),
                Config::Ftps(config) => format!("FTPS {}:{}", config.host, config.port),
            });
            ui.monospace(&preview.remote);
            ui.label(trf("Source: {bytes} bytes, SHA-256 {hash}", &[("bytes", &preview.source.bytes.len()), ("hash", &preview.source.hash)]));
            match &preview.before {
                deployment::RemoteImage::Missing => {
                    ui.label(tr("Remote destination was absent at preview."));
                }
                deployment::RemoteImage::Bytes { bytes, hash, .. } => {
                    ui.label(trf("Remote: {bytes} bytes, SHA-256 {hash}", &[("bytes", &bytes.len()), ("hash", hash)]));
                }
            }
            if preview.source_text.is_some() || preview.remote_text.is_some() {
                ui.columns(2, |columns| {
                    columns[0].label(tr("Before"));
                    let mut before = preview.remote_text.as_deref().unwrap_or("");
                    columns[0].add(egui::TextEdit::multiline(&mut before).code_editor().desired_rows(3).desired_width(f32::INFINITY));
                    columns[1].label(tr("After"));
                    let mut after = preview.source_text.as_deref().unwrap_or("");
                    columns[1].add(egui::TextEdit::multiline(&mut after).code_editor().desired_rows(3).desired_width(f32::INFINITY));
                });
            } else {
                ui.small(tr("Binary deployment: compare the size and SHA-256 hashes; no text conversion is applied."));
            }
            if matches!(preview.config, Config::Ftps(_)) && preview.create {
                ui.small(tr("FTPS STOU never overwrites an existing file. The final server-generated name is shown after transfer; cancellation may leave a partial new file."));
            } else {
                ui.small(tr("Replacements are checked again before publishing, but the protocols provide no atomic content compare-and-swap. Concurrent changes can race the last check. Cancellation may leave temporary files or an already committed transfer."));
            }
            ui.checkbox(&mut self.acknowledged, tr("I reviewed the source, destination, hashes and transfer limitations"));
            if ui.add_enabled(trusted && !busy && self.acknowledged, egui::Button::new(tr("Confirm deployment"))).clicked() {
                match current {
                    Ok(config) => self.apply(config, trusted),
                    Err(error) => self.error = Some(error),
                }
            }
        }
        if let Some(receipt) = &self.receipt {
            ui.separator();
            ui.label(tr("Deployment completed and verified"));
            ui.monospace(&receipt.remote);
            ui.label(trf("Transferred {bytes} bytes, SHA-256 {hash}", &[("bytes", &receipt.size), ("hash", &receipt.hash)]));
        }
    }
}
fn field(ui: &mut egui::Ui, text: &str, value: &mut String, limit: usize, password: bool) {
    let label = ui.label(text);
    ui.add(egui::TextEdit::singleline(value).password(password).char_limit(limit).desired_width(if password { 200. } else { 280. })).labelled_by(label.id);
}
fn error_text(error: &Error) -> String {
    match error {
        Error::Trust => tr("Trust the project before connecting or deploying files.").into(),
        Error::Input => tr("Invalid deployment settings, unsupported path or non-regular file.").into(),
        Error::Limit => tr("Deployment exceeded its file, response or metadata limit.").into(),
        Error::Cancelled => tr("Deployment cancelled. A transfer may already be committed or leave a partial or temporary remote file.").into(),
        Error::Timeout => tr("Deployment timed out. Inspect the destination before retrying.").into(),
        Error::Stale => tr("Local source, remote destination or connection settings changed. Preview the deployment again.").into(),
        Error::Protocol => tr("The server returned an invalid or unsupported transfer response.").into(),
        Error::Unsupported => tr("This server does not support the required safe transfer operation.").into(),
        Error::Tls => tr("FTPS certificate, hostname or public-key verification failed.").into(),
        Error::Authentication => tr("FTPS authentication was refused.").into(),
        Error::Remote(code) => trf("The transfer server refused an operation (code {code}).", &[("code", code)]),
        Error::Transport(code) => trf("The verified transfer connection failed (code {code}).", &[("code", code)]),
        Error::Io => tr("Could not read, snapshot or transfer this regular file.").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};
    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"native_deployment\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[workspace]\n").unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main(){println!(\"native deployment Rust 🦀\");}\n").unwrap();
        std::fs::write(dir.path().join("build.rs"), "fn main(){std::fs::write(\"code-ran\",\"yes\").unwrap();}\n").unwrap();
        dir
    }
    #[test]
    fn native_deployment_is_unavailable_without_trust_and_never_runs_project_code() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = project();
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace_and_wait(project.path());
        app.dock.show(crate::toolwindow::ToolWindow::Remote);
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::Remote, 1200.);
        let mut h = crate::screens::harness(app, egui::vec2(1900., 1800.));
        h.get_by_label(tr("Preview deployment")).click();
        h.run_steps(3);
        let state = &h.state().remote.deployment;
        assert!(!state.pending());
        assert!(state.preview.is_none() && state.receipt.is_none());
        assert!(h.state().insight.session.is_none());
        assert!(!project.path().join("code-ran").exists());
        assert!(!project.path().join("target").exists());
    }
    #[track_caller]
    fn wait(h: &mut egui_kittest::Harness<'static, crate::app::CrabCraftApp>, condition: impl Fn(&Deployments) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let state = &h.state().remote.deployment;
            if condition(state) {
                break;
            }
            assert!(state.error.is_none(), "deployment error {:?}", state.error);
            assert!(
                Instant::now() < deadline,
                "native deployment timeout protocol={:?} pending={} preview={} receipt={} destination={}",
                state.protocol,
                state.pending(),
                state.preview.is_some(),
                state.receipt.is_some(),
                state.remote
            );
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn configure_ssh(app: &mut crate::app::CrabCraftApp) {
        let config = crate::remote_transfer::tests::config();
        app.remote.host = config.host;
        app.remote.port = config.port.to_string();
        app.remote.user = config.user;
        app.remote.identity = config.identity.display().to_string();
        app.remote.known_hosts = config.known_hosts.display().to_string();
    }
    #[test]
    #[ignore = "Requires private OpenSSH and FTPS servers; clicks the actual native deployment review/confirmation controls"]
    fn real_native_sftp_and_ftps_deployment_require_trust_hash_review_and_confirmation() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ssh = crate::remote_transfer::tests::SSH_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ftps = crate::deployment::tests::FTPS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for protocol in [Protocol::Sftp, Protocol::Ftps] {
            let home = tempfile::tempdir().unwrap();
            let project = project();
            let root = project.path().canonicalize().unwrap();
            let binary = root.join("artifact");
            let output = std::process::Command::new("rustc").arg(root.join("src/main.rs")).arg("-o").arg(&binary).output().unwrap();
            assert!(output.status.success());
            let (config, ftps_root) = crate::deployment::tests::ftps();
            let remote_dir = tempfile::Builder::new().prefix("native-ftps-").tempdir_in(&ftps_root).unwrap();
            let destination = match protocol {
                Protocol::Sftp => root.join("deployed.bin").display().to_string(),
                Protocol::Ftps => format!("/{}/artifact.bin", remote_dir.path().file_name().unwrap().to_str().unwrap()),
            };
            let mut app = crate::screens::test_app(home.path(), false);
            app.open_workspace_and_wait(&root);
            app.dock.show(crate::toolwindow::ToolWindow::Remote);
            app.dock.sizes.insert(crate::toolwindow::ToolWindow::Remote, 1200.);
            let mut h = crate::screens::harness(app, egui::vec2(1900., 2100.));
            let deadline = Instant::now() + Duration::from_secs(5);
            while h.state().workspace.as_ref().is_none_or(|w| w.ws.root != root) {
                assert!(Instant::now() < deadline);
                h.run_steps(1);
                std::thread::sleep(Duration::from_millis(5));
            }
            configure_ssh(h.state_mut());
            let state = &mut h.state_mut().remote.deployment;
            state.protocol = protocol;
            state.local = binary.display().to_string();
            state.remote = destination.clone();
            if protocol == Protocol::Ftps {
                state.host = config.host.clone();
                state.port = config.port.to_string();
                state.user = config.user.clone();
                state.password = config.password.clone();
                state.ca = config.ca.as_ref().unwrap().display().to_string();
                state.pin = config.pin.clone();
            }
            h.run_steps(4);
            h.get_by_label(tr("Preview deployment")).click();
            h.run_steps(2);
            assert!(!h.state().remote.deployment.pending());
            assert!(h.state().remote.deployment.preview.is_none());
            assert!(!root.join("code-ran").exists());
            h.state_mut().settings.trusted_projects.push(root.clone());
            h.state_mut().runner.trusted_project = Some(root.clone());
            h.run_steps(5);
            eprintln!("native {:?} preview", protocol);
            h.get_by_label(tr("Preview deployment")).click();
            h.run_steps(2);
            {
                let state = &h.state().remote.deployment;
                assert!(state.pending() || state.preview.is_some() || state.error.is_some(), "preview was not dispatched");
            }
            wait(&mut h, |s| s.preview.is_some());
            eprintln!("native {:?} preview ready", protocol);
            h.run_steps(12);
            assert!(matches!(h.state().remote.deployment.preview.as_ref().unwrap().before, deployment::RemoteImage::Missing));
            assert!(h.state().remote.deployment.receipt.is_none());
            h.get_by_label(tr("Confirm deployment")).click();
            h.run_steps(12);
            assert!(h.state().remote.deployment.receipt.is_none());
            assert!(!h.state().remote.deployment.pending());
            h.get_by_label(tr("I reviewed the source, destination, hashes and transfer limitations")).scroll_to_me();
            h.run_steps(12);
            h.get_by_label(tr("I reviewed the source, destination, hashes and transfer limitations")).click();
            h.run_steps(4);
            assert!(h.state().remote.deployment.acknowledged, "review checkbox was not toggled");
            eprintln!("native {:?} confirm", protocol);
            h.get_by_label(tr("Confirm deployment")).scroll_to_me();
            h.run_steps(6);
            h.get_by_label(tr("Confirm deployment")).click();
            h.run_steps(2);
            {
                let state = &h.state().remote.deployment;
                assert!(state.pending() || state.receipt.is_some() || state.error.is_some(), "confirm was not dispatched");
            }
            wait(&mut h, |s| s.receipt.is_some());
            eprintln!("native {:?} completed", protocol);
            let receipt = h.state().remote.deployment.receipt.as_ref().unwrap();
            let remote = match protocol {
                Protocol::Sftp => PathBuf::from(&receipt.remote),
                Protocol::Ftps => ftps_root.join(receipt.remote.trim_start_matches('/')),
            };
            assert_eq!(std::fs::read(&remote).unwrap(), std::fs::read(&binary).unwrap());
            assert!(!root.join("code-ran").exists());
            assert!(!root.join("target").exists());
            let serialized = serde_json::to_string(&h.state().session()).unwrap();
            assert!(!serialized.contains(&config.password));
            assert!(h.state().insight.session.is_none());
        }
    }
}
