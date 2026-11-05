//! Explicit administrator-debug build/review. Preparation always runs as the desktop user.
use crate::privileged_debug_backend::{self as backend, Error, Frozen};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::tr;
use std::path::PathBuf;
struct Prepared {
    frozen: Frozen,
    name: String,
    versions: Vec<(u64, PathBuf, u64)>,
}
pub struct PrivilegedDebug {
    pub show_debug: bool,
    inbox: Inbox<Result<Prepared, Error>>,
    stop: Option<rc_dotnet::CancelToken>,
    ready: Option<Prepared>,
    pub running: bool,
    approved: bool,
    error: Option<String>,
}
impl PrivilegedDebug {
    pub fn new(waker: Waker) -> Self {
        Self {
            show_debug: false,
            inbox: Inbox::new(waker),
            stop: None,
            ready: None,
            running: false,
            approved: false,
            error: None,
        }
    }
    pub fn cancel(&mut self) {
        if let Some(stop) = &self.stop {
            stop.cancel();
        }
        self.ready = None;
        self.approved = false;
        self.running = false;
    }
    fn versions(docs: &crate::docs::Docs) -> Vec<(u64, PathBuf, u64)> {
        docs.list
            .iter()
            .filter(|d| !d.generated)
            .map(|d| (d.editor.id, d.path.clone(), d.editor.buffer.version()))
            .collect()
    }
    fn unchanged(prepared: &Prepared, docs: &crate::docs::Docs) -> bool {
        prepared.versions.iter().all(|(id, path, version)| {
            docs.list.iter().any(|d| {
                d.editor.id == *id
                    && d.path == *path
                    && d.editor.buffer.version() == *version
                    && !d.is_modified()
            })
        })
    }
    pub fn poll(
        &mut self,
        trusted: bool,
        root: Option<&std::path::Path>,
        docs: &crate::docs::Docs,
    ) {
        if !trusted {
            self.cancel();
        }
        let cancelled = self.stop.as_ref().is_some_and(|stop| stop.is_cancelled());
        for result in self.inbox.drain() {
            self.stop = None;
            if cancelled {
                self.ready = None;
                continue;
            }
            match result {
                Ok(prepared)
                    if trusted
                        && root == Some(prepared.frozen.spec.root.as_path())
                        && Self::unchanged(&prepared, docs) =>
                {
                    self.ready = Some(prepared);
                    self.approved = false;
                    self.error = None;
                }
                Ok(_) => {
                    self.ready = None;
                    self.error = Some(Error::Changed.to_string());
                }
                Err(error) => {
                    self.ready = None;
                    self.error = Some(error.to_string());
                }
            }
        }
    }
    fn build(
        &mut self,
        root: PathBuf,
        name: String,
        cargo: rc_cargo::Cargo,
        target: rc_cargo::RunTarget,
        docs: &crate::docs::Docs,
    ) {
        if self.stop.is_some() {
            return;
        }
        self.ready = None;
        self.approved = false;
        self.error = None;
        let versions = Self::versions(docs);
        let stop = rc_dotnet::CancelToken::new();
        let token = stop.clone();
        let out = self.inbox.sender();
        self.stop = Some(stop);
        rc_core::Runtime::spawn(async move {
            let result = async {
                let plan = rc_cargo::prepare(&cargo, &target, &token, &mut |_| {})
                    .await
                    .map_err(|e| Error::Io(e.to_string()))?
                    .ok_or(Error::Cancelled)?;
                if token.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                // Reading/hashing/copying the executable is blocking preparation, not UI work.
                let args = plan.args;
                let artifact = plan.program;
                let copy_token = token.clone();
                let frozen = tokio::task::spawn_blocking(move || {
                    Frozen::capture(root, artifact, args, &copy_token)
                })
                .await
                .map_err(|e| Error::Io(e.to_string()))??;
                if token.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                Ok(Prepared {
                    frozen,
                    name,
                    versions,
                })
            }
            .await;
            out.send(result);
        });
    }
}
impl Drop for PrivilegedDebug {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl crate::app::CrabCraftApp {
    pub(crate) fn prepare_privileged_debug(&mut self) {
        if self.defer_save(true, |app| app.prepare_privileged_debug()) {
            return;
        }
        let trusted = self
            .workspace
            .as_ref()
            .is_some_and(|w| self.settings.trusted_projects.contains(&w.ws.root));
        if !trusted {
            self.notify(Error::Trust.to_string(), true);
            return;
        }
        if !cfg!(target_os = "linux") {
            self.notify(Error::Platform.to_string(), true);
            return;
        }
        let Some(config) = self.runner.selected_config().cloned() else {
            self.notify(
                tr("Choose a Rust binary configuration before preparing administrator debugging"),
                true,
            );
            return;
        };
        if !matches!(config.kind, crate::runner::ConfigKind::Cargo { .. }) {
            self.notify(
                tr("Administrator debugging currently supports Cargo binary configurations only"),
                true,
            );
            return;
        }
        let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) else {
            return;
        };
        match self.runner.cargo_debug_target(&config) {
            Some(Ok((cargo, target))) => {
                self.privileged_debug
                    .build(root, config.name, cargo, target, &self.docs)
            }
            Some(Err(error)) => self.notify(error, true),
            None => self.notify(
                tr("This run configuration has no Rust binary to debug"),
                true,
            ),
        }
    }
    pub(crate) fn start_privileged_debug(&mut self) {
        if self.defer_save(true, |app| app.start_privileged_debug()) {
            return;
        }
        let trusted = self
            .workspace
            .as_ref()
            .is_some_and(|w| self.settings.trusted_projects.contains(&w.ws.root));
        let result = (|| {
            let prepared = self
                .privileged_debug
                .ready
                .as_ref()
                .ok_or(Error::Approval)?;
            if self.workspace.as_ref().map(|w| w.ws.root.as_path())
                != Some(prepared.frozen.spec.root.as_path())
                || !PrivilegedDebug::unchanged(prepared, &self.docs)
            {
                return Err(Error::Changed);
            }
            let adapter = backend::adapter(
                &prepared.frozen.spec,
                trusted,
                self.privileged_debug.approved,
            )?;
            let launch = rc_dap::LaunchConfig {
                program: prepared.frozen.spec.artifact.clone(),
                args: prepared.frozen.spec.args.clone(),
                cwd: prepared.frozen.spec.root.clone(),
                env: Vec::new(),
            };
            Ok((adapter, launch, prepared.name.clone()))
        })();
        match result {
            Ok((adapter, launch, name)) => {
                self.privileged_debug.running = true;
                self.privileged_debug.approved = false;
                let outputs = self
                    .debugger
                    .start_privileged(adapter, launch, &name, &self.docs);
                self.handle_debug(outputs);
            }
            Err(error) => {
                self.privileged_debug.approved = false;
                self.privileged_debug.error = Some(error.to_string());
            }
        }
    }
}
pub fn show(app: &mut crate::app::CrabCraftApp, ui: &mut egui::Ui) {
    let trusted = app
        .workspace
        .as_ref()
        .is_some_and(|w| app.settings.trusted_projects.contains(&w.ws.root));
    let mut prepare = false;
    let mut start = false;
    let mut cancel = false;
    ui.separator();
    ui.collapsing(tr("Administrator Debugging (Linux)"),|ui|{
        ui.label(tr("Partial support: an authenticated administrator debug session has not been verified on this host"));
        ui.label(tr("Build runs as your normal user. Only the reviewed frozen executable and system GDB request administrator privileges."));
        ui.label(tr("GDB init files, auto-loading, project environment, expressions, conditional breakpoints, logpoints and debugger-console commands are disabled."));
        ui.label(tr("The debugged program and debugger have full administrator permissions; shared libraries and external resources are not frozen."));
        if !trusted{ui.label(tr("Trust this project before running project tools or code"));}
        prepare=ui.add_enabled(cfg!(target_os="linux")&&trusted&&app.privileged_debug.stop.is_none()&&!app.privileged_debug.running,egui::Button::new(tr("Build and Review Administrator Debugging"))).clicked();
        if app.privileged_debug.stop.is_some(){ui.spinner();cancel|=ui.button(tr("Cancel Administrator Debug Preparation")).clicked();}
        if let Some(prepared)=&app.privileged_debug.ready{
            let mut preview=format!("{}\n{}\nSHA-256: {}\n{}: {}\n{}",prepared.name,prepared.frozen.spec.artifact.display(),prepared.frozen.spec.sha256,tr("Working directory"),prepared.frozen.spec.root.display(),prepared.frozen.spec.args.iter().map(|a|format!("{a:?}")).collect::<Vec<_>>().join(" "));
            ui.add(egui::TextEdit::multiline(&mut preview).interactive(false).font(egui::TextStyle::Monospace));
            ui.checkbox(&mut app.privileged_debug.approved,tr("I reviewed this executable, hash and arguments and authorize requesting administrator debugging"));
            start=ui.add_enabled(trusted&&app.privileged_debug.approved&&!app.privileged_debug.running&&app.privileged_debug.stop.is_none(),egui::Button::new(tr("Request Administrator Debugging"))).clicked();
        }
        if app.privileged_debug.running{cancel|=ui.button(tr("Stop Administrator Debugging")).clicked();}
        if let Some(error)=&app.privileged_debug.error{ui.colored_label(rc_kit::pal(ui).error,error);}
    });
    if cancel {
        app.privileged_debug.cancel();
        app.debugger.stop();
    }
    if prepare {
        app.prepare_privileged_debug();
    }
    if start {
        app.start_privileged_debug();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::Path,
        time::{Duration, Instant},
    };
    fn project(root: &Path) -> PathBuf {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"),"[package]\nname=\"privileged_debug_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(root.join("build.rs"),"fn main(){ let status=std::fs::read_to_string(\"/proc/self/status\").unwrap(); std::fs::write(\"builder-status\",status).unwrap(); std::fs::write(\"build-observed.rs\",std::fs::read_to_string(\"src/main.rs\").unwrap()).unwrap(); }\n").unwrap();
        let path = root.join("src/main.rs");
        std::fs::write(&path, "fn main() { println!(\"old\"); }\n").unwrap();
        path
    }
    fn pump(app: &mut crate::app::CrabCraftApp, done: impl Fn(&crate::app::CrabCraftApp) -> bool) {
        let end = Instant::now() + Duration::from_secs(30);
        while !done(app) {
            app.poll_cargo_workspace();
            app.poll_rustfmt();
            app.poll_disk_save();
            let trusted = app
                .workspace
                .as_ref()
                .is_some_and(|w| app.settings.trusted_projects.contains(&w.ws.root));
            app.privileged_debug.poll(
                trusted,
                app.workspace.as_ref().map(|w| w.ws.root.as_path()),
                &app.docs,
            );
            assert!(
                Instant::now() < end,
                "administrator debug preparation timed out: {:?}",
                app.privileged_debug.error
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn normal_user_cargo_build_sees_saved_text_and_authorization_stays_disabled_until_review() {
        use egui_kittest::kittest::Queryable;
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = project(root.path());
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace(root.path());
        app.open_file_and_wait(&path);
        app.prepare_privileged_debug();
        assert!(app.privileged_debug.stop.is_none());
        assert!(!root.path().join("builder-status").exists());
        app.trust_project_tools_for_test();
        pump(&mut app, |a| a.runner.cargo_workspace.is_some());
        let len = app.docs.list[0].editor.buffer.len_chars();
        let text = "fn main(){println!(\"last edited text before preparing debug\");}\n";
        app.docs.list[0].editor.replace(0, len, text);
        app.settings.format_on_save = true;
        app.prepare_privileged_debug();
        assert!(app.disk_save.continuation.is_some());
        pump(&mut app, |a| {
            a.privileged_debug.ready.is_some() || a.privileged_debug.error.is_some()
        });
        assert!(
            app.privileged_debug.ready.is_some(),
            "{:?}",
            app.privileged_debug.error
        );
        let source = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            source,
            std::fs::read_to_string(root.path().join("build-observed.rs")).unwrap()
        );
        assert!(source.contains("last edited text"));
        let status = std::fs::read_to_string(root.path().join("builder-status")).unwrap();
        let builder = status
            .lines()
            .find(|line| line.starts_with("Uid:"))
            .unwrap()
            .split_whitespace()
            .nth(2)
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assert_eq!(
            builder,
            unsafe { libc::geteuid() },
            "Cargo/build.rs must retain the desktop test user's EUID"
        );
        app.start_privileged_debug();
        assert!(!app.debugger.active());
        assert!(
            app.privileged_debug
                .error
                .as_ref()
                .unwrap()
                .contains("approve")
        );
        app.privileged_debug.show_debug = true;
        app.dock.show(crate::toolwindow::ToolWindow::PrivilegedRun);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 1100.0));
        h.get_by_label("Administrator Debugging (Linux)").click();
        h.run_steps(2);
        assert!(
            h.query_by_label("Request Administrator Debugging")
                .is_some()
        );
        assert!(h.query_by_label("I reviewed this executable, hash and arguments and authorize requesting administrator debugging").is_some());
        assert!(!h.state().privileged_debug.approved);
        assert!(!h.state().debugger.active());
        h.state_mut().docs.list[0]
            .editor
            .replace(0, 0, "// changed after review\n");
        h.state_mut().settings.format_on_save = false;
        h.state_mut().start_privileged_debug();
        pump(h.state_mut(), |a| {
            a.disk_save.continuation.is_none() && !a.disk_save.saves.busy()
        });
        assert!(!h.state().debugger.active());
        assert!(!h.state().privileged_debug.approved);
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn cancelled_preparation_or_provider_change_cannot_reactivate_a_late_review() {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = project(root.path());
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace(root.path());
        app.trust_project_tools_for_test();
        app.open_file_and_wait(&path);
        pump(&mut app, |a| a.runner.cargo_workspace.is_some());
        app.prepare_privileged_debug();
        assert!(app.privileged_debug.stop.is_some());
        app.privileged_debug.cancel();
        pump(&mut app, |a| a.privileged_debug.stop.is_none());
        assert!(app.privileged_debug.ready.is_none());
        assert!(!app.debugger.active());
        app.prepare_privileged_debug();
        app.change_cargo_provider(crate::criticalup_provider::Change::Default);
        pump(&mut app, |a| a.privileged_debug.stop.is_none());
        assert!(app.privileged_debug.ready.is_none());
        assert!(!app.debugger.active());
    }
}
