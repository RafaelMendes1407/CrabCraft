//! Explicit per-project CriticalUp selection. Failed selection never falls back to rustup.
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::path::{Path, PathBuf};
type Error = rc_cargo::criticalup::Error;
fn message(error: &Error) -> String {
    match error{Error::Trust=>tr("Trust this project before running project tools or code").into(),Error::Path=>tr("Choose the CriticalUp manager executable and a regular workspace-root criticalup.toml; rustup and hardlinked tool proxies are unsupported").into(),Error::Manifest=>tr("Unsupported criticalup.toml: require manifest-version 1, one Ferrocene product and Cargo/rustc packages").into(),Error::Version=>tr("CriticalUp version 1.4 or newer is required for strict toolchain execution").into(),Error::Io=>tr("Could not inspect the CriticalUp manifest or local installation").into(),Error::Limit=>tr("CriticalUp manifest, output or time limit exceeded").into(),Error::Cancelled=>tr("CriticalUp selection cancelled; choose a provider explicitly before running Cargo").into(),Error::ToolMismatch=>tr("CriticalUp did not resolve matching installed Cargo and rustc binaries; rustup fallback is refused").into(),Error::Manager{code,message}=>trf("CriticalUp failed ({code}): {message}",&[("code",&code.map_or_else(||"?".into(),|code|code.to_string())),("message",message)])}
}
#[derive(Clone)]
pub enum Choice {
    Default,
    Criticalup(Option<rc_cargo::Cargo>),
}
impl Choice {
    pub fn resolve(&self, env: Vec<(String, String)>) -> Result<rc_cargo::Cargo, String> {
        match self{Self::Default=>rc_cargo::Cargo::locate().ok_or_else(||tr("Cargo was not found. Install Rust with rustup.").to_string()),Self::Criticalup(Some(cargo))=>Ok(cargo.clone()),Self::Criticalup(None)=>Err(tr("CriticalUp is selected but not ready; select an installed toolchain or explicitly return to Rustup / PATH").into())}.map(|cargo|cargo.with_env(env))
    }
}
#[derive(Clone, Copy)]
pub enum Change {
    Blocked,
    Ready,
    Default,
}
enum EventKind {
    Inspected(Result<rc_cargo::criticalup::Snapshot, Error>),
    Activated(Result<rc_cargo::Cargo, Error>),
}
struct Event(u64, EventKind);
pub struct Provider {
    epoch: u64,
    pub program: String,
    pub selected: bool,
    cargo: Option<rc_cargo::Cargo>,
    snapshot: Option<rc_cargo::criticalup::Snapshot>,
    inbox: Inbox<Event>,
    stop: Option<rc_dotnet::CancelToken>,
    activating: bool,
    error: Option<String>,
}
impl Provider {
    pub fn new(waker: Waker) -> Self {
        Self {
            epoch: 0,
            program: String::new(),
            selected: false,
            cargo: None,
            snapshot: None,
            inbox: Inbox::new(waker),
            stop: None,
            activating: false,
            error: None,
        }
    }
    pub fn choice(&self) -> Choice {
        if self.selected {
            Choice::Criticalup(self.cargo.clone())
        } else {
            Choice::Default
        }
    }
    pub fn label(&self) -> String {
        if self.selected {
            self.cargo
                .as_ref()
                .and_then(rc_cargo::Cargo::criticalup_release)
                .map_or_else(
                    || tr("CriticalUp unavailable").to_string(),
                    |release| format!("CriticalUp: {release}"),
                )
        } else {
            tr("Rustup / PATH").into()
        }
    }
    fn inspect(&mut self, root: PathBuf) {
        self.epoch = self.epoch.saturating_add(1);
        let epoch = self.epoch;
        let stop = rc_dotnet::CancelToken::new();
        self.stop = Some(stop.clone());
        self.activating = false;
        self.snapshot = None;
        self.error = None;
        let out = self.inbox.sender();
        rc_core::Runtime::spawn(async move {
            let result =
                tokio::task::spawn_blocking(move || rc_cargo::criticalup::inspect(&root, &stop))
                    .await
                    .unwrap_or(Err(Error::Io));
            out.send(Event(epoch, EventKind::Inspected(result)));
        });
    }
    fn activate(&mut self, trusted: bool) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        self.selected = true;
        self.cargo = None;
        self.error = None;
        self.activating = true;
        self.epoch = self.epoch.saturating_add(1);
        let epoch = self.epoch;
        let program = PathBuf::from(&self.program);
        let stop = rc_dotnet::CancelToken::new();
        self.stop = Some(stop.clone());
        let out = self.inbox.sender();
        rc_core::Runtime::spawn(async move {
            out.send(Event(
                epoch,
                EventKind::Activated(
                    rc_cargo::criticalup::activate(program, snapshot, trusted, stop).await,
                ),
            ));
        });
    }
    pub fn poll(&mut self, trusted: bool) -> Vec<Change> {
        if self.activating && !trusted {
            if let Some(stop) = &self.stop {
                stop.cancel();
            }
        }
        let mut changes = Vec::new();
        for Event(epoch, event) in self.inbox.drain() {
            if epoch != self.epoch {
                continue;
            }
            self.stop = None;
            self.activating = false;
            match event {
                EventKind::Inspected(Ok(snapshot)) => self.snapshot = Some(snapshot),
                EventKind::Activated(Ok(cargo)) if trusted => {
                    self.cargo = Some(cargo);
                    changes.push(Change::Ready);
                }
                EventKind::Activated(Ok(_)) => {
                    self.cargo = None;
                    self.error = Some(message(&Error::Trust));
                    changes.push(Change::Blocked);
                }
                EventKind::Inspected(Err(error)) | EventKind::Activated(Err(error)) => {
                    self.error = Some(message(&error))
                }
            }
        }
        changes
    }
    fn show(&mut self, ui: &mut egui::Ui, root: Option<PathBuf>, trusted: bool) -> Vec<Change> {
        let mut changes = Vec::new();
        egui::ScrollArea::vertical().id_salt("criticalup_body").show(ui,|ui|{
ui.label(tr("Choose the Rust provider explicitly. CriticalUp activation never authenticates, installs a toolchain or downloads a compiler."));ui.label(tr("CriticalUp uses a private snapshot of the validated workspace-root criticalup.toml. Inspect again and select after editing it; selection is not restored automatically after restarting the IDE."));ui.label(tr("Cargo and rustc use the same strict manifest snapshot. Rustup/compiler/wrapper environment overrides are ignored; failed selection blocks Cargo until you explicitly choose another provider."));ui.label(tr("Rust language services, doctest completion, REPL, wasm-pack, coverage and third-party Cargo extensions are unsupported in CriticalUp mode. No Ferrocene compiler or qualification claim is made without an installed toolchain."));ui.label(tr("Selection affects IDE Cargo actions only; terminals, plugins and external agents keep their own tool environments."));ui.label(tr("Selecting a provider stops build, run, test and language services, and discards stale metadata results."));ui.label(self.label());
ui.add_enabled_ui(self.stop.is_none(),|ui|{ui.label(tr("CriticalUp manager executable (absolute path)"));ui.add(egui::TextEdit::singleline(&mut self.program).char_limit(4096));});
ui.horizontal_wrapped(|ui|{if ui.add_enabled(self.stop.is_none()&&root.is_some(),egui::Button::new(tr("Inspect criticalup.toml"))).clicked(){if let Some(root)=root.clone(){self.inspect(root);}}if ui.add_enabled(self.selected,egui::Button::new(tr("Use Rustup / PATH"))).clicked(){if let Some(stop)=&self.stop{stop.cancel();}self.epoch=self.epoch.saturating_add(1);self.stop=None;self.selected=false;self.cargo=None;self.error=None;changes.push(Change::Default);}});
if let Some(snapshot)=&self.snapshot{ui.label(trf("Ferrocene manifest release: {release}",&[("release",&snapshot.release())]));ui.monospace(format!("SHA-256: {}",snapshot.digest()));for package in snapshot.packages(){ui.label(package);}if ui.add_enabled(trusted&&self.stop.is_none()&&!self.program.is_empty(),egui::Button::new(tr("Select CriticalUp Provider"))).clicked(){self.activate(trusted);changes.push(Change::Blocked);}}
if !trusted{ui.colored_label(rc_kit::pal(ui).warn,tr("Trust this project before running project tools or code"));}if self.stop.is_some(){ui.spinner();if ui.button(tr("Cancel CriticalUp Selection")).clicked(){if let Some(stop)=&self.stop{stop.cancel();}}}if let Some(error)=&self.error{ui.colored_label(rc_kit::pal(ui).error,error);}ui.label(tr("Authenticate and install licensed Ferrocene toolchains with CriticalUp outside the IDE when required. Missing tools are reported without invoking rustup."));});
        changes
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.cancel();
        }
    }
}
pub fn show(app: &mut crate::app::CrabCraftApp, ui: &mut egui::Ui) {
    if crate::ui::toolwindows::header(ui, tr("Rust Toolchain Provider")) {
        app.toggle_tool(crate::toolwindow::ToolWindow::RustProvider);
    }
    let root = app
        .workspace
        .as_ref()
        .filter(|w| w.ws.cargo_manifest().is_some())
        .map(|w| w.ws.root.clone());
    let trusted = root
        .as_ref()
        .is_some_and(|root| app.settings.trusted_projects.contains(root));
    for change in app.criticalup_provider.show(ui, root, trusted) {
        app.change_cargo_provider(change);
    }
}
pub fn unavailable() -> String {
    tr("CriticalUp is selected but not ready; select an installed toolchain or explicitly return to Rustup / PATH").into()
}
pub fn unsupported() -> String {
    tr("This operation is unsupported by the selected CriticalUp provider; explicitly switch to Rustup / PATH to use it").into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    fn fixture(root: &Path, missing: bool, gate: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"criticalup_native_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "fn main(){std::fs::write(\"program-ran\",\"yes\").unwrap();println!(\"42\");}",
        )
        .unwrap();
        std::fs::write(
            root.join("build.rs"),
            "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}",
        )
        .unwrap();
        std::fs::write(root.join("criticalup.toml"),"manifest-version = 1\n[products.ferrocene]\nrelease=\"fixture-1\"\npackages=[\"cargo-${rustc-host}\",\"rustc-${rustc-host}\",\"rustfmt-${rustc-host}\"]\n").unwrap();
        let rustup = rc_cargo::Cargo::locate()
            .unwrap()
            .path()
            .with_file_name("rustup");
        let mut tools = std::collections::BTreeMap::new();
        for name in ["cargo", "rustc", "rustfmt", "rustdoc"] {
            let output = std::process::Command::new(&rustup)
                .args(["which", name])
                .current_dir(std::env::temp_dir())
                .env("RUSTUP_AUTO_INSTALL", "0")
                .output()
                .unwrap();
            if output.status.success() {
                tools.insert(
                    name,
                    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
                        .canonicalize()
                        .unwrap(),
                );
            }
        }
        let program = root.join("criticalup");
        let tools = serde_json::to_string(&tools).unwrap();
        let script = format!(
            r#"#!/usr/bin/python3
import os,sys,json,time
with open({wire:?},'a') as f:f.write(json.dumps(sys.argv[1:])+'\n')
a=sys.argv[1:];tools=json.loads({tools:?})
if a==['--version']:print('criticalup 1.6.0')
elif a[0]=='which':
 while os.path.exists({gate:?}):time.sleep(.01)
 if {missing} or a[-1] not in tools:print('Ferrocene toolchain not installed',file=sys.stderr);sys.exit(1)
 print(tools[a[-1]])
elif a[0]=='run':
 if '--strict' not in a or '--project' not in a:sys.exit(3)
 i=a.index('--project');manifest=a[i+1];name=a[i+2]
 if 'fixture-1' not in open(manifest).read():sys.exit(4)
 if name not in tools:sys.exit(5)
 os.execve(tools[name],[tools[name]]+a[i+3:],os.environ)
else:sys.exit(6)
"#,
            wire = root.join("criticalup-wire").to_string_lossy(),
            gate = root.join("gate").to_string_lossy(),
            missing = if missing { "True" } else { "False" }
        );
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        if gate {
            std::fs::write(root.join("gate"), "wait").unwrap();
        }
        program
    }
    #[cfg(unix)]
    #[track_caller]
    fn wait(
        h: &mut egui_kittest::Harness<'_, crate::app::CrabCraftApp>,
        test: impl Fn(&crate::app::CrabCraftApp) -> bool,
    ) {
        let start = std::time::Instant::now();
        while !test(h.state()) && start.elapsed() < std::time::Duration::from_secs(20) {
            h.run_steps(2);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(test(h.state()), "provider={:?}; selected={}; workspace={}; saves={}; pending_action={}; notices={:?}; sync={:?}", h.state().criticalup_provider.error, h.state().criticalup_provider.selected, h.state().runner.cargo_workspace.is_some(), h.state().disk_save.saves.busy(), h.state().disk_save.continuation.is_some(), h.state().notices.iter().map(|notice| &notice.text).collect::<Vec<_>>(), h.state().sync_log.entries.iter().map(|entry| &entry.log).collect::<Vec<_>>());
    }
    #[cfg(unix)]
    #[test]
    fn native_provider_blocks_pending_then_routes_metadata_build_run_and_reload_without_fallback() {
        use egui_kittest::kittest::Queryable;
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let program = fixture(&root, false, true);
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(&root);
        app.criticalup_provider.program = program.to_string_lossy().into();
        app.dock
            .sizes
            .insert(crate::toolwindow::ToolWindow::RustProvider, 950.0);
        let mut h = crate::screens::harness(app, egui::vec2(2000.0, 1500.0));
        h.get_by_label(tr("Rust Toolchain Provider")).click();
        h.run_steps(2);
        h.get_by_label(tr("Inspect criticalup.toml")).click();
        h.run_steps(2);
        wait(&mut h, |app| app.criticalup_provider.snapshot.is_some());
        assert!(!root.join("criticalup-wire").exists());
        h.get_by_label(tr("Select CriticalUp Provider")).click();
        h.run_steps(2);
        assert!(!h.state().criticalup_provider.selected);
        assert!(!root.join("criticalup-wire").exists());
        h.state_mut().trust_project_tools_for_test();
        wait(&mut h, |app| app.runner.cargo_workspace.is_some());
        assert!(!root.join("build-ran").exists());
        h.get_by_label(tr("Select CriticalUp Provider")).click();
        h.run_steps(2);
        assert!(h.state().criticalup_provider.selected);
        assert!(h.state().runner.cargo_tool().is_err());
        assert!(h.state().runner.cargo_workspace.is_none());
        assert!(!root.join("build-ran").exists());
        let poison = root.join("poison");
        std::fs::write(
            &poison,
            format!(
                "#!/bin/sh\ntouch {:?}\nexit 70\n",
                root.join("wrong-compiler")
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&poison, std::fs::Permissions::from_mode(0o700)).unwrap();
        h.state_mut().runner.project_env = format!(
            "CARGO_NET_OFFLINE=true\nRUSTC={}\nRUSTUP_TOOLCHAIN=never-use-rustup\nRUSTC_WRAPPER={}",
            poison.display(),
            poison.display()
        );
        std::fs::remove_file(root.join("gate")).unwrap();
        wait(&mut h, |app| {
            app.runner.cargo_workspace.is_some()
                && app
                    .runner
                    .cargo_tool()
                    .is_ok_and(|cargo| cargo.is_criticalup())
        });
        assert!(!root.join("wrong-compiler").exists());
        assert!(!h.state().can_open_rust_repl());
        assert!(h.state().rustup().is_none());
        let outputs = h.state_mut().runner.run_selected();
        crate::ui::run_windows::handle_outputs(h.state_mut(), outputs);
        wait(&mut h, |app| {
            app.runner.runs.last().is_some_and(|run| run.exit.is_some())
        });
        assert_eq!(h.state().runner.runs.last().unwrap().exit, Some(Some(0)));
        assert!(root.join("build-ran").exists() && root.join("program-ran").exists());
        assert!(!root.join("wrong-compiler").exists());
        std::fs::write(root.join("criticalup.toml"), "invalid after selection").unwrap();
        h.state_mut().reload_workspace();
        h.state_mut().wait_workspace_open();
        wait(&mut h, |app| {
            app.runner.cargo_workspace.is_some()
                && app
                    .runner
                    .cargo_tool()
                    .is_ok_and(|cargo| cargo.is_criticalup())
        });
        assert_eq!(
            h.state().runner.cargo_tool().unwrap().criticalup_release(),
            Some("fixture-1")
        );
        h.state_mut().runner.project_env = "CARGO_NET_OFFLINE=true".into();
        h.run_steps(2);
        h.get_by_label(tr("Use Rustup / PATH")).click();
        h.run_steps(2);
        wait(&mut h, |app| {
            !app.criticalup_provider.selected && app.runner.cargo_workspace.is_some()
        });
        assert!(!h.state().runner.cargo_tool().unwrap().is_criticalup());
    }
    #[cfg(unix)]
    #[test]
    fn failed_or_cancelled_selection_does_not_restore_rustup_implicitly() {
        use egui_kittest::kittest::Queryable;
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let program = fixture(&root, true, false);
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(&root);
        app.trust_project_tools_for_test();
        app.criticalup_provider.program = program.to_string_lossy().into();
        app.dock
            .sizes
            .insert(crate::toolwindow::ToolWindow::RustProvider, 950.0);
        let mut h = crate::screens::harness(app, egui::vec2(2000.0, 1500.0));
        h.get_by_label(tr("Rust Toolchain Provider")).click();
        h.run_steps(2);
        h.get_by_label(tr("Inspect criticalup.toml")).click();
        h.run_steps(2);
        wait(&mut h, |app| app.criticalup_provider.snapshot.is_some());
        h.get_by_label(tr("Select CriticalUp Provider")).click();
        h.run_steps(2);
        wait(&mut h, |app| app.criticalup_provider.error.is_some());
        assert!(h.state().criticalup_provider.selected);
        assert!(h.state().runner.cargo_tool().is_err());
        assert!(h.state().runner.cargo_workspace.is_none());
        assert!(!root.join("build-ran").exists());
        std::fs::write(root.join("gate"), "wait").unwrap();
        h.get_by_label(tr("Select CriticalUp Provider")).click();
        h.run_steps(2);
        assert!(h.state().criticalup_provider.stop.is_some());
        h.get_by_label(tr("Use Rustup / PATH")).click();
        h.run_steps(2);
        wait(&mut h, |app| {
            !app.criticalup_provider.selected && app.runner.cargo_workspace.is_some()
        });
        h.run_steps(10);
        assert!(!h.state().criticalup_provider.selected);
        assert!(h.state().criticalup_provider.error.is_none());
        assert!(!h.state().runner.cargo_tool().unwrap().is_criticalup());
    }
}
