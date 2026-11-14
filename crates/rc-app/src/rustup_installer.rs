//! Explicit native rustup installation. Downloading, verification and execution only begin
//! after the user accepts the displayed source, destination and command.
use rc_cargo::rustup_install::{self, Destinations, InstallError, Prepared};
use rc_core::{Inbox, Waker};
use rc_dotnet::{CancelToken, RunEvent, RunHandle};
use rc_kit::i18n::{tr, trf};

pub struct RustupInstaller {
    destinations: Result<Destinations, InstallError>,
    pub stable: bool,
    accepted: bool,
    download: Option<CancelToken>,
    prepared: Option<Prepared>,
    run: Option<RunHandle>,
    inbox: Inbox<Result<Prepared, InstallError>>,
    events: Inbox<RunEvent>,
    discovery: Inbox<Option<rc_cargo::Cargo>>,
    pub output: String,
    error: Option<String>,
    pub rediscover: bool,
    finished: bool,
}
impl RustupInstaller {
    pub fn new(waker: Waker) -> Self {
        Self {
            destinations: rustup_install::destinations(),
            stable: true,
            accepted: false,
            download: None,
            prepared: None,
            run: None,
            inbox: Inbox::new(waker.clone()),
            events: Inbox::new(waker.clone()),
            discovery: Inbox::new(waker),
            output: String::new(),
            error: None,
            rediscover: false,
            finished: false,
        }
    }
    fn busy(&self) -> bool {
        self.download.is_some() || self.run.is_some()
    }
    fn start(&mut self) {
        if !self.accepted || self.busy() || self.destinations.is_err() {
            return;
        }
        let cancel = CancelToken::new();
        self.download = Some(cancel.clone());
        self.output.clear();
        self.error = None;
        self.finished = false;
        let out = self.inbox.sender();
        std::thread::spawn(move || {
            out.send(rustup_install::prepare(&cancel));
        });
    }
    fn cancel(&self) {
        if let Some(cancel) = &self.download {
            cancel.cancel();
        }
        if let Some(run) = &self.run {
            run.stop();
        }
    }
    pub fn poll(&mut self) {
        for result in self.inbox.drain() {
            let cancelled = self.download.take().is_some_and(|token| token.is_cancelled());
            if cancelled {
                self.error = Some(tr("Cancelled").to_string());
                self.accepted = false;
                continue;
            }
            match result {
                Ok(prepared) => {
                    if let Ok(destinations) = &self.destinations {
                        let plan = prepared.plan(self.stable, destinations);
                        self.run = Some(rc_cargo::start_process(plan, self.events.sender()));
                        self.prepared = Some(prepared);
                    }
                }
                Err(error) => {
                    self.error = Some(error_text(&error));
                    self.accepted = false;
                }
            }
        }
        let mut finished = false;
        for event in self.events.drain() {
            match event {
                RunEvent::Started { plan, .. } => append(&mut self.output, &plan.command_line()),
                RunEvent::Output(line) => append(&mut self.output, &line.text),
                RunEvent::Exited { code, cancelled, .. } => {
                    finished = true;
                    self.finished = !cancelled && code == Some(0);
                    append(
                        &mut self.output,
                        if cancelled {
                            tr("Cancelled")
                        } else if self.finished {
                            tr("rustup installation finished")
                        } else {
                            tr("rustup installation failed; inspect the output")
                        },
                    );
                }
                RunEvent::Failed(error) => {
                    finished = true;
                    self.error = Some(error);
                }
                _ => {}
            }
        }
        if finished {
            self.run = None;
            self.prepared = None;
            self.accepted = false;
            self.rediscover = true;
            let out = self.discovery.sender();
            std::thread::spawn(move || {
                out.send(rc_cargo::Cargo::locate());
            });
        }
    }
    pub fn take_cargo(&mut self) -> Option<Option<rc_cargo::Cargo>> {
        self.discovery.drain().into_iter().last()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.busy();
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(if self.download.is_some() { tr("Downloading and verifying rustup-init…") } else { tr("Installing rustup…") });
                if ui.button(tr("Cancel installation")).clicked() {
                    self.cancel();
                }
            });
        } else if self.finished {
            ui.label(tr("rustup installation finished. Shell PATH was not changed; restart your shell or configure PATH if needed."));
        }
        ui.collapsing(tr("Install rustup…"), |ui| {
            if let Ok(url) = rustup_install::url() {
                ui.label(tr("Downloads the official installer and its SHA-256 checksum over HTTPS. The checksum must match before execution."));
                ui.monospace(&url);
            } else {
                ui.label(tr("Automatic rustup installation is unsupported on this platform."));
                return;
            }
            match &self.destinations {
                Ok(destinations) => {
                    ui.label(tr("Installation directories:"));
                    ui.monospace(format!("CARGO_HOME={}\nRUSTUP_HOME={}", destinations.cargo.display(), destinations.rustup.display()));
                }
                Err(error) => { ui.label(error_text(error)); return; }
            }
            ui.add_enabled_ui(!busy, |ui| {
                if ui.checkbox(&mut self.stable, tr("Install minimal stable Rust with rust-src and rust-analyzer")).changed() { self.accepted = false; }
                ui.monospace(format!("rustup-init {}", rustup_install::arguments(self.stable).join(" ")));
                ui.label(tr("Installs tools in the displayed directories and may replace existing rustup proxies or update the default stable toolchain. Shell PATH is not changed. Cancellation keeps changes already made."));
                ui.checkbox(&mut self.accepted, tr("I confirm this rustup installation and its displayed changes"));
                if ui.add_enabled(self.accepted, egui::Button::new(tr("Download and install rustup"))).clicked() { self.start(); }
            });
        });
        if let Some(error) = &self.error {
            ui.colored_label(rc_kit::pal(ui).error, error);
        }
        if !self.output.is_empty() {
            egui::ScrollArea::vertical().id_salt("rustup_install_output").max_height(180.0).show(ui, |ui| {
                ui.monospace(&self.output);
            });
        }
    }
}
impl Drop for RustupInstaller {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn append(output: &mut String, text: &str) {
    if output.len() < 64 * 1024 {
        output.extend(text.chars().take(4096));
        output.push('\n');
    }
}
fn error_text(error: &InstallError) -> String {
    match error {
        InstallError::Unsupported => tr("Automatic rustup installation is unsupported on this platform.").to_string(),
        InstallError::Home => tr("Could not determine the rustup installation directories").to_string(),
        InstallError::Cancelled => tr("Cancelled").to_string(),
        InstallError::Network(error) => trf("Could not download the official rustup installer: {error}", &[("error", error)]),
        InstallError::TooLarge => tr("The rustup installer exceeds the download size limit").to_string(),
        InstallError::Checksum => tr("The rustup installer checksum is missing, invalid or does not match. Nothing was executed.").to_string(),
        InstallError::Io(error) => trf("Could not prepare the rustup installer: {error}", &[("error", error)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    use std::time::{Duration, Instant};
    #[test]
    fn installation_requires_the_native_confirmation_and_option_changes_clear_it() {
        let mut installer = RustupInstaller::new(Waker::noop());
        installer.start();
        assert!(installer.download.is_none());
        let mut h = Harness::builder().with_size(egui::vec2(900.0, 800.0)).build_ui_state(
            |ui, installer: &mut RustupInstaller| {
                installer.poll();
                installer.ui(ui);
            },
            installer,
        );
        h.run_steps(2);
        h.get_by_label(tr("Install rustup…")).click();
        h.run_steps(2);
        h.get_by_label(tr("I confirm this rustup installation and its displayed changes")).click();
        h.run_steps(2);
        assert!(h.state().accepted);
        h.get_by_label(tr("Install minimal stable Rust with rust-src and rust-analyzer")).click();
        h.run_steps(2);
        assert!(!h.state().accepted);
        assert!(h.state().download.is_none());
    }
    #[test]
    #[cfg(unix)]
    #[ignore = "Downloads and installs official rustup only into temporary homes via native controls"]
    fn native_confirmation_installs_verified_rustup_in_isolated_homes() {
        let root = tempfile::tempdir().unwrap();
        let mut installer = RustupInstaller::new(Waker::noop());
        installer.destinations = Ok(Destinations { cargo: root.path().join("cargo"), rustup: root.path().join("rustup") });
        installer.stable = false;
        let mut h = Harness::builder().with_size(egui::vec2(1000.0, 900.0)).build_ui_state(
            |ui, installer: &mut RustupInstaller| {
                installer.poll();
                installer.ui(ui);
            },
            installer,
        );
        h.run_steps(2);
        h.get_by_label(tr("Install rustup…")).click();
        h.run_steps(2);
        h.get_by_label(tr("I confirm this rustup installation and its displayed changes")).click();
        h.run_steps(2);
        h.get_by_label(tr("Download and install rustup")).click();
        h.run_steps(2);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(150) && !h.state().rediscover {
            h.run_steps(2);
            if h.state().error.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(h.state().finished, "{:?}\n{}", h.state().error, h.state().output);
        assert!(root.path().join("cargo/bin").join(format!("rustup{}", std::env::consts::EXE_SUFFIX)).is_file());
    }
}
