//! Explicit remote-file workflow. Credentials and previews are memory-only.
use crate::remote_transfer::{self as transfer, Action, Config, Error, Preview, Reply};
use rc_core::{Inbox, Waker};
use rc_dotnet::process::CancelToken;
use rc_kit::i18n::{tr, trf};
use std::path::PathBuf;
pub struct Remote {
    pub host: String,
    pub port: String,
    pub user: String,
    pub identity: String,
    pub known_hosts: String,
    pub path: String,
    pub deployment: crate::deployment_ui::Deployments,
    pub original: String,
    pub edited: String,
    pub preview: Option<Preview>,
    pub error: Option<String>,
    pub status: String,
    inbox: Inbox<(u64, Result<Reply, Error>)>,
    generation: u64,
    cancel: Option<CancelToken>,
    terminal: Option<rc_term::SpawnSpec>,
    files: Vec<tempfile::TempDir>,
}
impl Remote {
    pub fn new(waker: Waker) -> Self {
        Self {
            host: String::new(),
            port: "22".into(),
            user: String::new(),
            identity: String::new(),
            known_hosts: String::new(),
            path: String::new(),
            deployment: crate::deployment_ui::Deployments::new(waker.clone()),
            original: String::new(),
            edited: String::new(),
            preview: None,
            error: None,
            status: String::new(),
            inbox: Inbox::new(waker),
            generation: 0,
            cancel: None,
            terminal: None,
            files: Vec::new(),
        }
    }
    pub fn cancel(&mut self) {
        self.deployment.cancel();
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
        self.preview = None;
    }
    fn config(&self) -> Result<Config, Error> {
        Ok(Config {
            host: self.host.clone(),
            port: self.port.parse().ok().filter(|port| *port > 0).ok_or(Error::Input)?,
            user: self.user.clone(),
            identity: PathBuf::from(&self.identity),
            known_hosts: PathBuf::from(&self.known_hosts),
        })
    }
    fn start(&mut self, action: Action, trusted: bool) {
        if !trusted {
            self.error = Some(tr("Trust the project before connecting to SSH").into());
            return;
        }
        if self.cancel.is_some() {
            return;
        }
        let config = match self.config() {
            Ok(config) => config,
            Err(error) => {
                self.error = Some(error_text(error));
                return;
            }
        };
        self.preview = None;
        self.error = None;
        self.status.clear();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let cancel = CancelToken::new();
        self.cancel = Some(cancel.clone());
        let sender = self.inbox.sender();
        if let Err(error) = std::thread::Builder::new().name("rc-sftp".into()).spawn(move || sender.send((generation, transfer::run(config, action, cancel)))) {
            self.cancel = None;
            self.error = Some(error_text(Error::from(error)));
        }
    }
    pub fn poll(&mut self, trusted: bool) {
        self.deployment.poll(trusted);
        if !trusted {
            self.cancel();
            self.terminal = None;
            return;
        }
        for (generation, result) in self.inbox.drain() {
            if generation != self.generation {
                continue;
            }
            self.cancel = None;
            match result {
                Ok(Reply::Read(text)) => {
                    self.original = text.clone();
                    self.edited = text;
                    self.status = tr("Remote file loaded; no changes uploaded").into();
                }
                Ok(Reply::Preview(preview)) => self.preview = Some(preview),
                Ok(Reply::Applied(text)) => {
                    self.original = text.clone();
                    self.edited = text;
                    self.status = tr("Remote transfer completed; refresh before another edit").into();
                }
                Ok(Reply::Terminal(spec, files)) => {
                    if self.files.len() < 8 {
                        self.files.push(files);
                        self.terminal = Some(spec);
                    } else {
                        self.error = Some(error_text(Error::Limit));
                    }
                }
                Err(error) => self.error = Some(error_text(error)),
            }
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, trusted: bool) {
        ui.small(tr("SSH uses keys and a verified known_hosts file. Interactive SSH password prompts are unsupported."));
        let busy = self.cancel.is_some() || self.deployment.pending();
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("Host"));
                ui.add(egui::TextEdit::singleline(&mut self.host).char_limit(253).desired_width(180.0));
                ui.label(tr("Port"));
                ui.add(egui::TextEdit::singleline(&mut self.port).char_limit(5).desired_width(55.0));
                ui.label(tr("User"));
                ui.add(egui::TextEdit::singleline(&mut self.user).char_limit(253).desired_width(130.0));
            });
            ui.horizontal(|ui| {
                ui.label(tr("SSH private key"));
                ui.add(egui::TextEdit::singleline(&mut self.identity).char_limit(8192).desired_width(440.0));
            });
            ui.horizontal(|ui| {
                ui.label(tr("Verified known_hosts file"));
                ui.add(egui::TextEdit::singleline(&mut self.known_hosts).char_limit(8192).desired_width(440.0));
            });
            ui.horizontal(|ui| {
                ui.label(tr("Remote absolute path"));
                ui.add(egui::TextEdit::singleline(&mut self.path).char_limit(4096).desired_width(440.0));
            });
        });
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(trusted && !busy, egui::Button::new(tr("Load remote file"))).clicked() {
                self.start(Action::Read(self.path.clone()), trusted);
            }
            if ui.add_enabled(trusted && !busy, egui::Button::new(tr("Preview remote edit"))).clicked() {
                self.start(Action::PreviewEdit { remote: self.path.clone(), before: self.original.clone(), after: self.edited.clone() }, trusted);
            }
            if ui.add_enabled(trusted && !busy, egui::Button::new(tr("Open SSH terminal"))).clicked() {
                self.start(Action::Terminal, trusted);
            }
            if busy && ui.button(tr("Cancel")).clicked() {
                self.cancel();
                self.status = tr("Cancellation requested; a completed write may already be committed").into();
            }
        });
        if !trusted {
            ui.colored_label(rc_kit::pal(ui).warn, tr("Trust the project before connecting to SSH"));
        }
        if let Some(error) = &self.error {
            ui.colored_label(rc_kit::pal(ui).error, error);
        }
        ui.label(&self.status);
        ui.small(tr("The remote text editor supports existing UTF-8 files up to 2 MB. Use Reviewed file deployment for new files and binaries."));
        ui.small(tr("Text previews show up to 16,384 characters. In-pane editing is limited to 64 KB."));
        ui.columns(2, |columns| {
            columns[0].label(tr("Remote snapshot"));
            let mut snapshot = self.original.chars().take(16384).collect::<String>();
            columns[0].add(egui::TextEdit::multiline(&mut snapshot).code_editor().interactive(false).desired_rows(7).desired_width(f32::INFINITY));
            columns[1].label(tr("Proposed remote text"));
            if self.edited.len() <= 64 * 1024 {
                columns[1].add(egui::TextEdit::multiline(&mut self.edited).code_editor().char_limit(16384).desired_rows(7).desired_width(f32::INFINITY));
            } else {
                let mut excerpt = self.edited.chars().take(16384).collect::<String>();
                columns[1].add(egui::TextEdit::multiline(&mut excerpt).code_editor().interactive(false).desired_rows(7).desired_width(f32::INFINITY));
            }
        });
        if let Some(preview) = &self.preview {
            ui.separator();
            ui.label(tr("Review upload before confirmation"));
            ui.monospace(format!("{}@{}:{} → {}", preview.config.user, preview.config.host, preview.config.port, preview.remote));
            ui.columns(2, |columns| {
                columns[0].label(tr("Before"));
                let mut text = preview.before.chars().take(16384).collect::<String>();
                columns[0].add(egui::TextEdit::multiline(&mut text).code_editor().interactive(false).desired_rows(5).desired_width(f32::INFINITY));
                columns[1].label(tr("After"));
                let mut text = preview.after.chars().take(16384).collect::<String>();
                columns[1].add(egui::TextEdit::multiline(&mut text).code_editor().interactive(false).desired_rows(5).desired_width(f32::INFINITY));
            });
            ui.colored_label(rc_kit::pal(ui).warn,tr("Remote content is checked again before upload. SFTP cannot lock out concurrent writers; cancellation may leave a temporary file or an already committed upload."));
            if ui.add_enabled(trusted && !busy, egui::Button::new(tr("Confirm remote overwrite"))).clicked() {
                self.start(Action::Apply(preview.clone()), trusted);
            }
        }
        let ssh = self.config().ok();
        self.deployment.ui(ui, ssh, trusted, self.cancel.is_some());
    }
}
fn error_text(error: Error) -> String {
    match error {
        Error::Input => tr("Invalid SSH settings or unsupported remote text/path").into(),
        Error::Limit => tr("Remote operation exceeded its transfer or output limit").into(),
        Error::Cancelled => tr("Cancelled").into(),
        Error::Timeout => tr("SSH operation timed out").into(),
        Error::Stale => tr("Local or remote content changed; load and preview again").into(),
        Error::Io(error) | Error::Process(error) => trf("SSH error: {error}", &[("error", &error)]),
    }
}
pub fn show(app: &mut crate::app::CrabCraftApp, ui: &mut egui::Ui) {
    if crate::ui::toolwindows::header(ui, tr("Remote files")) {
        app.toggle_tool(crate::toolwindow::ToolWindow::Remote);
    }
    let trusted = app.workspace.as_ref().is_some_and(|workspace| app.settings.trusted_projects.contains(&workspace.ws.root));
    egui::ScrollArea::vertical().show(ui, |ui| app.remote.ui(ui, trusted));
    if let Some(spec) = app.remote.terminal.take() {
        app.open_ssh_terminal(spec);
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{HOME_LOCK, harness, test_app};
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};
    #[track_caller]
    fn pump(h: &mut egui_kittest::Harness<'static, crate::app::CrabCraftApp>, done: impl Fn(&crate::app::CrabCraftApp) -> bool) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            h.run_steps(2);
            if done(h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("remote UI timeout: {:?}", h.state().remote.error);
    }
    #[test]
    #[ignore = "Requires the real private SSH server; clicks native controls and runs an explicit SSH terminal"]
    fn native_remote_preview_confirmation_stale_guard_and_explicit_ssh_terminal() {
        let _home = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _ssh = crate::remote_transfer::tests::SSH_TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let config = crate::remote_transfer::tests::config();
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"native_ssh_evidence\"\nversion=\"0.1.0\"\nedition=\"2021\"\n").unwrap();
        let file = root.join("src/main.rs");
        let first = "fn main(){println!(\"before\");}\n";
        let after = "fn main(){println!(\"remote edit 🦀\");}\n";
        std::fs::write(&file, first).unwrap();
        let mut app = test_app(home.path(), true);
        app.open_workspace_and_wait(&root);
        app.dock.show(crate::toolwindow::ToolWindow::Remote);
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::Remote, 1050.0);
        app.remote.host = config.host;
        app.remote.port = config.port.to_string();
        app.remote.user = config.user;
        app.remote.identity = config.identity.display().to_string();
        app.remote.known_hosts = config.known_hosts.display().to_string();
        app.remote.path = file.display().to_string();
        let mut h = harness(app, egui::vec2(1800.0, 1500.0));
        h.get_by_label(tr("Load remote file")).click();
        h.run_steps(2);
        assert!(h.state().remote.cancel.is_none());
        assert!(h.state().remote.original.is_empty());
        h.state_mut().settings.trusted_projects.push(root.clone());
        h.state_mut().runner.trusted_project = Some(root.clone());
        h.run_steps(2);
        h.get_by_label(tr("Load remote file")).click();
        h.run_steps(2);
        pump(&mut h, |app| app.remote.original == first);
        h.state_mut().remote.edited = after.into();
        h.run_steps(2);
        h.get_by_label(tr("Preview remote edit")).click();
        h.run_steps(2);
        pump(&mut h, |app| app.remote.preview.is_some());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), first);
        std::fs::write(&file, "external change").unwrap();
        h.get_by_label(tr("Confirm remote overwrite")).click();
        h.run_steps(2);
        pump(&mut h, |app| app.remote.error.is_some());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "external change");
        std::fs::write(&file, first).unwrap();
        h.get_by_label(tr("Load remote file")).click();
        h.run_steps(2);
        pump(&mut h, |app| app.remote.cancel.is_none() && app.remote.error.is_none());
        h.state_mut().remote.edited = after.into();
        h.run_steps(2);
        h.get_by_label(tr("Preview remote edit")).click();
        h.run_steps(2);
        pump(&mut h, |app| app.remote.preview.is_some());
        h.get_by_label(tr("Confirm remote overwrite")).click();
        h.run_steps(2);
        pump(&mut h, |app| app.remote.cancel.is_none() && app.remote.original == after);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), after);
        h.get_by_label(tr("Open SSH terminal")).click();
        h.run_steps(2);
        pump(&mut h, |app| app.terminals.active().is_some());
        let marker = root.join("ssh-terminal-ran");
        h.state().terminals.active().unwrap().session.write(format!("printf '%s' verified > '{}'; exit\n", marker.display()).into_bytes());
        pump(&mut h, |_| marker.is_file());
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "verified");
    }
}
