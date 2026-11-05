//! User-initiated dependency search and Cargo edits in a native modal. All file and network
//! work stays off the UI thread; closing the modal cancels the running search/command.
use egui::{Context, Ui};
use rc_cargo::dependencies::{self, Add, DependencyError, Installed, Scope};
use rc_core::{Inbox, Waker};
use rc_dotnet::{BuildEvent, OpHandle};
use rc_kit::i18n::tr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

pub struct CargoDependencies {
    waker: Waker,
    manifest: Option<PathBuf>,
    generation: u64,
    files: Inbox<(u64, Result<Vec<Installed>, DependencyError>)>,
    searches: Inbox<(u64, Result<Vec<rc_cargo::crates_io::CrateHit>, String>)>,
    events: Inbox<BuildEvent>,
    search_cancel: Option<Arc<AtomicBool>>,
    operation: Option<OpHandle>,
    installed: Vec<Installed>,
    hits: Vec<rc_cargo::crates_io::CrateHit>,
    pub query: String,
    pub name: String,
    pub version: String,
    pub features: String,
    pub path: String,
    scope: Scope,
    optional: bool,
    defaults: bool,
    offline: bool,
    editable: bool,
    pub output: String,
    error: Option<String>,
    loading: bool,
    reload_project: bool,
    remove: Option<(String, Scope)>,
}
impl CargoDependencies {
    pub fn new(waker: Waker) -> Self {
        Self { waker: waker.clone(), manifest: None, generation: 0, files: Inbox::new(waker.clone()), searches: Inbox::new(waker.clone()), events: Inbox::new(waker), search_cancel: None, operation: None,
            installed: Vec::new(), hits: Vec::new(), query: String::new(), name: String::new(), version: String::new(), features: String::new(), path: String::new(), scope: Scope::Normal,
            optional: false, defaults: true, offline: false, editable: true, output: String::new(), error: None, loading: false, reload_project: false, remove: None }
    }
    /// Prefill a local proposal. This reads the manifest but never searches or changes it.
    pub fn review(&mut self, manifest: &Path, name: &str, scope: Scope, local: Option<&Path>) {
        self.cancel(); self.form(); self.scope = scope; self.query = name.to_string(); self.name = name.to_string();
        self.path = local.map(|path| path.to_string_lossy().into_owned()).unwrap_or_default();
        self.load(manifest.to_path_buf());
    }
    pub fn cancel(&mut self) {
        if let Some(cancel) = self.search_cancel.take() { cancel.store(true, Ordering::Relaxed); }
        if let Some(operation) = self.operation.take() { operation.cancel(); }
        self.generation = self.generation.wrapping_add(1);
        self.events = Inbox::new(self.waker.clone());
        self.loading = false;
        self.manifest = None;
    }
    fn load(&mut self, manifest: PathBuf) {
        if let Some(cancel) = self.search_cancel.take() { cancel.store(true, Ordering::Relaxed); }
        self.manifest = Some(manifest.clone());
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.loading = true;
        self.installed.clear();
        self.remove = None;
        let out = self.files.sender();
        std::thread::spawn(move || {
            use std::io::Read;
            let result = (|| {
                let metadata = std::fs::symlink_metadata(&manifest).map_err(|_| DependencyError::Manifest)?;
                if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 { return Err(DependencyError::Manifest); }
                let mut options = std::fs::OpenOptions::new(); options.read(true);
                #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW); }
                let file = options.open(manifest).map_err(|_| DependencyError::Manifest)?;
                if !file.metadata().map_err(|_| DependencyError::Manifest)?.is_file() { return Err(DependencyError::Manifest); }
                let mut text = String::new();
                file.take(4 * 1024 * 1024 + 1).read_to_string(&mut text).map_err(|_| DependencyError::Manifest)?;
                dependencies::installed(&text)
            })();
            out.send((generation, result));
        });
    }
    fn search(&mut self) {
        if let Some(cancel) = self.search_cancel.take() { cancel.store(true, Ordering::Relaxed); }
        let cancel = Arc::new(AtomicBool::new(false));
        self.search_cancel = Some(cancel.clone());
        self.generation = self.generation.wrapping_add(1);
        let (generation, query, out) = (self.generation, self.query.trim().to_string(), self.searches.sender());
        self.hits.clear();
        self.error = None;
        std::thread::spawn(move || {
            let result = rc_cargo::crates_io::search_crates_io(&query, Duration::from_secs(12), &cancel).map_err(|e| format!("{e:#}"));
            if !cancel.load(Ordering::Relaxed) { out.send((generation, result)); }
        });
    }
    fn poll(&mut self) {
        for (generation, result) in self.files.drain() {
            if generation != self.generation { continue; }
            self.loading = false;
            match result { Ok(deps) => self.installed = deps, Err(error) => self.error = Some(error_text(error).to_string()) }
        }
        for (generation, result) in self.searches.drain() {
            if generation != self.generation { continue; }
            self.search_cancel = None;
            match result { Ok(hits) => self.hits = hits, Err(error) => self.error = Some(error) }
        }
        let mut finished = false;
        for event in self.events.drain() {
            match event {
                BuildEvent::Started { command } => append(&mut self.output, &command),
                BuildEvent::Output(line) => append(&mut self.output, &line.text),
                BuildEvent::Finished(result) => {
                    finished = true;
                    self.reload_project = true;
                    append(&mut self.output, if result.cancelled { tr("Cancelled") } else if result.success { tr("Dependency updated") } else { tr("Dependency command failed; inspect Cargo output and the manifest") });
                }
                BuildEvent::Failed(error) => { finished = true; self.reload_project = true; self.error = Some(error); }
                _ => {}
            }
        }
        if finished {
            self.operation = None;
            if let Some(manifest) = self.manifest.clone() { self.load(manifest); }
        }
    }
    fn start(&mut self, cargo: &rc_cargo::Cargo, args: Result<Vec<String>, DependencyError>, trusted: bool) {
        if !trusted || self.operation.is_some() { return; }
        let Some(manifest) = self.manifest.clone() else { return };
        match args {
            Ok(args) => {
                self.output.clear(); self.error = None;
                self.events = Inbox::new(self.waker.clone());
                self.operation = Some(rc_cargo::start_command(cargo, &manifest, args, self.events.sender()));
            }
            Err(error) => self.error = Some(error_text(error).to_string()),
        }
    }
    fn form(&mut self) {
        self.name.clear(); self.version.clear(); self.features.clear(); self.path.clear(); self.optional = false; self.defaults = true; self.editable = true; self.remove = None;
    }
    fn ui(&mut self, ui: &mut Ui, packages: &[rc_cargo::Package], cargo: Option<&rc_cargo::Cargo>, trusted: bool, clean: bool) {
        let busy = self.operation.is_some();
        let mut manifest = self.manifest.clone();
        ui.horizontal(|ui| {
            ui.label(tr("Package"));
            egui::ComboBox::from_id_salt("dependency_package").selected_text(packages.iter().find(|p| Some(&p.manifest) == manifest.as_ref()).map(|p| p.name.as_str()).unwrap_or("")).show_ui(ui, |ui| {
                for package in packages { ui.add_enabled_ui(!busy, |ui| { ui.selectable_value(&mut manifest, Some(package.manifest.clone()), &package.name); }); }
            });
            if ui.add_enabled(!busy, egui::Button::new(tr("Refresh"))).clicked() && let Some(path) = &manifest { self.load(path.clone()); }
            ui.checkbox(&mut self.offline, tr("Offline"));
        });
        if manifest != self.manifest && let Some(path) = manifest { self.form(); self.load(path); }
        if let Some(path) = &self.manifest { ui.small(path.display().to_string()); }
        if cargo.is_none() { ui.label(tr("Cargo was not found. Install Rust with rustup.")); }
        if !trusted { ui.label(tr("Trust the project before changing dependencies.")); }
        ui.label(tr("Search contacts crates.io only when you press Search. Cargo edits may access configured registries unless Offline is enabled."));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(tr("Crate name to search")).char_limit(64));
            if ui.add_enabled(self.search_cancel.is_none() && !self.loading && rc_cargo::valid_crate_name(self.query.trim()), egui::Button::new(tr("Search"))).clicked() { self.search(); }
            if self.search_cancel.is_some() {
                ui.spinner();
                if ui.button(tr("Cancel Search")).clicked() { if let Some(c) = self.search_cancel.take() { c.store(true, Ordering::Relaxed); } self.generation = self.generation.wrapping_add(1); }
            }
        });
        egui::ScrollArea::vertical().id_salt("crate_search_results").max_height(115.0).show(ui, |ui| {
            for hit in self.hits.clone() {
                if ui.selectable_label(false, format!("{}  {}", hit.name, hit.version.as_deref().unwrap_or(""))).clicked() && !busy {
                    self.form(); self.name = hit.name; self.version = hit.version.unwrap_or_default();
                }
                if let Some(description) = hit.description { ui.small(description); }
            }
        });
        ui.separator();
        ui.label(tr("Installed dependencies"));
        if self.loading { ui.spinner(); }
        egui::ScrollArea::vertical().id_salt("installed_crate_dependencies").max_height(160.0).show(ui, |ui| {
            for dep in self.installed.clone() {
                ui.horizontal(|ui| {
                    let label = format!("{}  {}  [{}]", dep.name, dep.version, dep.scope.table());
                    if ui.selectable_label(false, label).clicked() && !busy {
                        self.name = dep.name.clone(); self.version = dep.version; self.features = dep.features.join(","); self.scope = dep.scope; self.optional = dep.optional; self.defaults = dep.default_features;
                        self.path = dep.path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(); self.editable = dep.editable;
                    }
                    if ui.add_enabled(trusted && clean && !busy && cargo.is_some(), egui::Button::new(tr("Remove…"))).clicked() { self.remove = Some((dep.name, dep.scope)); }
                });
                if !dep.source.is_empty() { ui.small(dep.source); }
            }
        });
        if let Some((name, scope)) = self.remove.clone() {
            ui.horizontal(|ui| {
                ui.label(rc_kit::i18n::trf("Remove {name} from {scope}?", &[("name", &name), ("scope", &scope.table())]));
                if ui.add_enabled(trusted && clean && !busy, egui::Button::new(tr("Remove"))).clicked() && let (Some(cargo), Some(manifest)) = (cargo, self.manifest.as_ref()) {
                    self.start(cargo, dependencies::remove_args(manifest, scope, &name, self.offline), trusted); self.remove = None;
                }
                if ui.button(tr("Cancel")).clicked() { self.remove = None; }
            });
        }
        ui.separator();
        ui.add_enabled_ui(!busy, |ui| {
            if ui.button(tr("New dependency")).clicked() { self.form(); }
            egui::Grid::new("dependency_form").num_columns(2).show(ui, |ui| {
                ui.label(tr("Crate name")); ui.add(egui::TextEdit::singleline(&mut self.name).char_limit(64)); ui.end_row();
                ui.label(tr("Version requirement")); ui.add(egui::TextEdit::singleline(&mut self.version).hint_text("1.0").char_limit(200)); ui.end_row();
                ui.label(tr("Features (comma-separated)")); ui.add(egui::TextEdit::singleline(&mut self.features).char_limit(2048)); ui.end_row();
                ui.label(tr("Local dependency path (optional)")); ui.add(egui::TextEdit::singleline(&mut self.path).char_limit(4096)); ui.end_row();
                ui.label(tr("Dependency scope")); egui::ComboBox::from_id_salt("dep_scope").selected_text(self.scope.table()).show_ui(ui, |ui| {
                    for scope in [Scope::Normal, Scope::Dev, Scope::Build] { ui.selectable_value(&mut self.scope, scope, scope.table()); }
                }); ui.end_row();
            });
            if self.scope == Scope::Dev { self.optional = false; }
            ui.horizontal(|ui| { ui.add_enabled_ui(self.scope != Scope::Dev, |ui| { ui.checkbox(&mut self.optional, tr("Optional dependency")); }); ui.checkbox(&mut self.defaults, tr("Default features")); });
        });
        ui.small(tr("Cargo add keeps previously enabled features. Remove feature entries in Cargo.toml."));
        if !self.editable { ui.label(tr("Edit inherited, Git and alternate-registry dependency settings in Cargo.toml. Removal is supported here.")); }
        ui.label(tr("Target-specific and workspace-level dependency tables must be edited in Cargo.toml."));
        if ui.add_enabled(trusted && clean && !busy && self.editable && cargo.is_some() && !self.name.is_empty(), egui::Button::new(tr("Add or update dependency"))).clicked()
            && let (Some(cargo), Some(manifest)) = (cargo, self.manifest.as_ref()) {
                let path = (!self.path.trim().is_empty()).then(|| {
                    let path = Path::new(self.path.trim());
                    if path.is_absolute() { path.to_path_buf() } else { manifest.parent().unwrap_or(Path::new(".")).join(path) }
                });
                let add = Add { name: self.name.trim().into(), version: self.version.trim().into(), features: self.features.split([',', ' ']).filter(|s| !s.is_empty()).map(str::to_string).collect(), optional: self.optional, no_default_features: !self.defaults, path };
                self.start(cargo, dependencies::add_args(manifest, self.scope, &add, self.offline), trusted);
            }
        if busy && ui.button(tr("Cancel dependency change")).clicked() && let Some(operation) = &self.operation { operation.cancel(); }
        if let Some(error) = &self.error { ui.colored_label(rc_kit::pal(ui).error, error); }
        if !self.output.is_empty() { egui::ScrollArea::vertical().id_salt("dependency_output").max_height(100.0).show(ui, |ui| { ui.monospace(&self.output); }); }
    }
}
impl Drop for CargoDependencies { fn drop(&mut self) { self.cancel(); } }
fn append(output: &mut String, line: &str) {
    if output.len() < 64 * 1024 { output.extend(line.chars().take(2048)); output.push('\n'); }
}
fn error_text(error: DependencyError) -> &'static str {
    match error {
        DependencyError::OptionalDev => tr("Development dependencies cannot be optional"),
        DependencyError::Name => tr("Enter a valid crate name"),
        DependencyError::Version => tr("Enter a valid Cargo version requirement"),
        DependencyError::Features => tr("Enter comma-separated Cargo feature names"),
        DependencyError::Manifest => tr("Could not read dependencies from Cargo.toml"),
        DependencyError::TooLarge => tr("Cargo.toml exceeds the dependency editor's 4 MB limit"),
    }
}

pub fn show(app: &mut crate::app::CrabCraftApp, ctx: &Context) -> bool {
    let packages: Vec<_> = app.runner.cargo_workspace.iter().chain(&app.runner.attached).flat_map(|w| w.packages.clone()).collect();
    let cargo = app.runner.cargo_tool().ok();
    let clean = !app.docs.list.iter().any(|d| d.path.file_name().is_some_and(|n| n == "Cargo.toml") && d.is_modified());
    let manager = &mut app.cargo_dependencies;
    if manager.manifest.is_none() && let Some(package) = packages.first() { manager.load(package.manifest.clone()); }
    manager.poll();
    let reload = std::mem::take(&mut manager.reload_project);
    let trusted = app.workspace.as_ref().is_some_and(|w| app.settings.trusted_projects.contains(&w.ws.root));
    let mut close = false;
    egui::Modal::new(egui::Id::new("cargo_dependency_manager")).show(ctx, |ui| {
        ui.set_width(720.0);
        ui.heading(tr("Cargo Dependencies"));
        if !clean { ui.label(tr("Save edited Cargo.toml files before changing dependencies.")); }
        egui::ScrollArea::vertical().id_salt("cargo_dependencies_dialog").max_height((ctx.content_rect().height() - 160.0).max(200.0)).show(ui, |ui| {
            manager.ui(ui, &packages, cargo.as_ref(), trusted, clean);
        });
        if ui.button(tr("Close")).clicked() { close = true; }
    });
    close |= ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if close { manager.cancel(); }
    if reload { app.reload_cargo(); }
    close
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{HOME_LOCK, harness, test_app};
    use egui_kittest::kittest::Queryable;
    use std::time::Instant;
    fn pump(h: &mut egui_kittest::Harness<'static, crate::app::CrabCraftApp>, done: impl Fn(&crate::app::CrabCraftApp) -> bool) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            h.run_steps(2);
            if done(h.state()) { return; }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("dependency operation timed out: {:?} {}", h.state().cargo_dependencies.error, h.state().cargo_dependencies.output);
    }
    #[test]
    fn native_manager_adds_and_removes_a_local_dependency_in_a_real_workspace() {
        let _lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"app\", \"dep\"]\nresolver = \"2\"\n").unwrap();
        for name in ["app", "dep"] {
            let dir = root.join(name);
            std::fs::create_dir_all(dir.join("src")).unwrap();
            std::fs::write(dir.join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[features]\nextra = []\n")).unwrap();
            std::fs::write(dir.join("src/lib.rs"), "pub fn value() -> u32 { 42 }\n").unwrap();
            std::fs::write(dir.join("build.rs"), "fn main() { std::fs::write(\"build-ran\", \"yes\").unwrap(); }\n").unwrap();
        }
        let mut app = test_app(home.path(), true);
        app.open_workspace(&root);
        app.dock.show(crate::toolwindow::ToolWindow::Cargo);
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::Cargo, 500.0);
        let mut h = harness(app, egui::vec2(1440.0, 1100.0));
        pump(&mut h, |app| app.runner.cargo_workspace.is_some());
        h.get_by_label(tr("Manage Cargo Dependencies…")).click();
        h.run_steps(2);
        pump(&mut h, |app| !app.cargo_dependencies.loading);
        assert!(h.state().cargo_dependencies.search_cancel.is_none());
        let cargo = rc_cargo::Cargo::locate().unwrap();
        let manifest = root.join("app/Cargo.toml");
        let add = Add { name: "dep".into(), path: Some(root.join("dep")), ..Default::default() };
        h.state_mut().cargo_dependencies.start(&cargo, dependencies::add_args(&manifest, Scope::Normal, &add, true), false);
        assert!(h.state().cargo_dependencies.operation.is_none());
        assert!(!root.join("app/build-ran").exists());
        // Marking trust here is an explicit test action; do not start rust-analyzer.
        h.state_mut().settings.trusted_projects.push(root.clone());
        {
            let manager = &mut h.state_mut().cargo_dependencies;
            manager.name = "dep".into(); manager.path = root.join("dep").to_string_lossy().into_owned(); manager.features = "extra".into(); manager.offline = true;
        }
        h.run_steps(2);
        h.get_by_label(tr("Add or update dependency")).click();
        h.run_steps(2);
        pump(&mut h, |app| app.cargo_dependencies.operation.is_none() && !app.cargo_dependencies.loading && app.cargo_dependencies.installed.iter().any(|d| d.name == "dep"));
        assert!(std::fs::read_to_string(&manifest).unwrap().contains("dep ="));
        assert!(!root.join("app/build-ran").exists());
        assert!(!root.join("dep/build-ran").exists());
        h.run_steps(4);
        h.get_by_label(tr("Remove…")).click();
        h.run_steps(2);
        assert!(h.state().cargo_dependencies.remove.is_some());
        h.get_by_label(tr("Remove")).click();
        h.run_steps(2);
        pump(&mut h, |app| app.cargo_dependencies.operation.is_none() && !app.cargo_dependencies.loading && app.cargo_dependencies.installed.is_empty());
        assert!(!std::fs::read_to_string(&manifest).unwrap().contains("dep ="));
        h.run_steps(4);
        h.get_by_label(tr("Close")).click();
        h.run_steps(2);
        assert!(h.state().dialog.is_none());
        assert!(h.state().cargo_dependencies.manifest.is_none());
    }
    #[test]
    fn pasted_imports_review_then_explicit_offline_add_in_the_correct_rust_member() {
        let _lock = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home=tempfile::tempdir().unwrap(); let project=tempfile::tempdir().unwrap(); let root=project.path().canonicalize().unwrap();
        std::fs::write(root.join("Cargo.toml"),"[workspace]\nmembers=[\"app\",\"helper-lib\"]\nresolver=\"2\"\n").unwrap();
        for name in ["app","helper-lib"] {
            let dir=root.join(name); std::fs::create_dir_all(dir.join("src")).unwrap();
            std::fs::write(dir.join("Cargo.toml"),format!("[package]\nname=\"{name}\"\nversion=\"0.1.0\"\nedition=\"2021\"\n")).unwrap();
            std::fs::write(dir.join("src/lib.rs"),"pub fn value() -> u32 { 42 }\n").unwrap();
            std::fs::write(dir.join("build.rs"),"fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}").unwrap();
        }
        let manifest=root.join("app/Cargo.toml"); let before=std::fs::read(&manifest).unwrap(); let helper_before=std::fs::read(root.join("helper-lib/Cargo.toml")).unwrap();
        let mut app=test_app(home.path(),true); app.open_workspace(&root); let mut h=harness(app,egui::vec2(1440.0,1100.0));
        pump(&mut h,|app| app.runner.cargo_workspace.is_some()); h.state_mut().open_file(&root.join("app/src/lib.rs")); h.state_mut().docs.active_doc_mut().unwrap().editor.selection=rc_text::Selection::point(0); h.run_steps(3);
        h.event(egui::Event::Paste("use serde::de::DeserializeOwned;\nfn decode<T: DeserializeOwned>() {}\n#[cfg(test)] mod tests { use helper_lib::value; #[test] fn local(){assert_eq!(value(),42);} }\n".into())); h.run_steps(2);
        pump(&mut h,|app| app.missing_crates.report.proposals.len()==2);
        assert!(h.state().missing_crates.report.proposals.iter().all(|proposal| proposal.pasted));
        assert_eq!(std::fs::read(&manifest).unwrap(),before); assert!(h.state().cargo_dependencies.search_cancel.is_none()); assert!(h.state().cargo_dependencies.operation.is_none());
        // Review is safe before trust: no search or edit starts. The existing Add stays gated.
        let proposal=h.state().missing_crates.report.proposals.iter().find(|proposal| proposal.name=="serde").unwrap().clone();
        h.state_mut().cargo_dependencies.review(&proposal.manifest,&proposal.name,proposal.scope,None); h.state_mut().dialog=Some(crate::ui::dialogs::Dialog::CargoDependencies); h.run_steps(3); pump(&mut h,|app| !app.cargo_dependencies.loading);
        assert_eq!(h.state().cargo_dependencies.manifest.as_ref(),Some(&manifest)); assert_eq!(h.state().cargo_dependencies.scope,Scope::Normal); assert!(h.state().cargo_dependencies.search_cancel.is_none());
        let cargo=rc_cargo::Cargo::locate().unwrap(); h.state_mut().cargo_dependencies.start(&cargo,Ok(vec![]),false); assert!(h.state().cargo_dependencies.operation.is_none());
        h.state_mut().settings.trusted_projects.push(root.clone()); h.state_mut().cargo_dependencies.offline=true; h.run_steps(2); h.get_by_label(tr("Add or update dependency")).click(); h.run_steps(2);
        pump(&mut h,|app| app.cargo_dependencies.operation.is_none() && !app.cargo_dependencies.loading && app.cargo_dependencies.installed.iter().any(|dependency| dependency.name=="serde"));
        assert!(!root.join("app/build-ran").exists()); assert!(!root.join("helper-lib/build-ran").exists());
        h.get_by_label(tr("Close")).click(); h.run_steps(2);
        pump(&mut h,|app| app.missing_crates.report.proposals.len()==1 && app.missing_crates.report.proposals[0].name=="helper-lib");
        h.get_by_label(tr("Review dependency…")).click(); h.run_steps(2); pump(&mut h,|app| !app.cargo_dependencies.loading);
        assert_eq!(h.state().cargo_dependencies.scope,Scope::Dev); assert_eq!(h.state().cargo_dependencies.path,root.join("helper-lib").to_string_lossy());
        h.get_by_label(tr("Add or update dependency")).click(); h.run_steps(2); pump(&mut h,|app| app.cargo_dependencies.operation.is_none() && !app.cargo_dependencies.loading && app.cargo_dependencies.installed.iter().any(|dependency| dependency.name=="helper-lib"));
        assert_eq!(std::fs::read(root.join("helper-lib/Cargo.toml")).unwrap(),helper_before); assert!(!root.join("app/build-ran").exists());
        let active=h.state().docs.active.unwrap(); h.state_mut().docs.save(active).unwrap();
        let output=std::process::Command::new("cargo").args(["check","--offline","--tests","--manifest-path"]).arg(&manifest).output().unwrap(); assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        h.get_by_label(tr("Close")).click(); h.run_steps(2);
        h.event(egui::Event::Text("use regex::Regex;\n".into())); h.run_steps(2);
        pump(&mut h,|app| app.missing_crates.report.proposals.iter().any(|proposal| proposal.name=="regex" && !proposal.pasted));
        assert!(h.state().cargo_dependencies.operation.is_none()); assert!(h.state().cargo_dependencies.search_cancel.is_none());

    }

}
