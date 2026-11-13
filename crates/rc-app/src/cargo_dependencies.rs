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

/// What the left pane of the manager lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane { Installed, Search }

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
    pub pane: Pane,
    /// Height the output section took last frame; the panes give it back while it is hidden so
    /// the dialog keeps its size and its buttons never move.
    output_height: f32,
}
impl CargoDependencies {
    pub fn new(waker: Waker) -> Self {
        Self { waker: waker.clone(), manifest: None, generation: 0, files: Inbox::new(waker.clone()), searches: Inbox::new(waker.clone()), events: Inbox::new(waker), search_cancel: None, operation: None,
            installed: Vec::new(), hits: Vec::new(), query: String::new(), name: String::new(), version: String::new(), features: String::new(), path: String::new(), scope: Scope::Normal,
            optional: false, defaults: true, offline: false, editable: true, output: String::new(), error: None, loading: false, reload_project: false, remove: None, pane: Pane::Installed, output_height: 150.0 }
    }
    /// Prefill a local proposal. This reads the manifest but never searches or changes it.
    pub fn review(&mut self, manifest: &Path, name: &str, scope: Scope, local: Option<&Path>) {
        self.cancel(); self.form(); self.pane = Pane::Installed; self.scope = scope; self.query = name.to_string(); self.name = name.to_string();
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
        self.pane = Pane::Search;
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
    /// The installed dependency the form currently describes, if any.
    fn current(&self) -> Option<&Installed> {
        let name = self.name.trim();
        self.installed.iter().find(|dep| dep.name == name && dep.scope == self.scope)
    }
    fn pick(&mut self, dep: Installed) {
        self.name = dep.name; self.version = dep.version; self.features = dep.features.join(","); self.scope = dep.scope; self.optional = dep.optional; self.defaults = dep.default_features;
        self.path = dep.path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(); self.editable = dep.editable; self.remove = None;
    }
    /// Package selector, refresh and the offline switch.
    fn toolbar(&mut self, ui: &mut Ui, packages: &[rc_cargo::Package], busy: bool) {
        let mut manifest = self.manifest.clone();
        ui.horizontal(|ui| {
            ui.label(tr("Package"));
            egui::ComboBox::from_id_salt("dependency_package").width(220.0).selected_text(packages.iter().find(|p| Some(&p.manifest) == manifest.as_ref()).map(|p| p.name.as_str()).unwrap_or("")).show_ui(ui, |ui| {
                for package in packages { ui.add_enabled_ui(!busy, |ui| { ui.selectable_value(&mut manifest, Some(package.manifest.clone()), &package.name); }); }
            });
            if ui.add_enabled(!busy, egui::Button::new(tr("Refresh"))).clicked() && let Some(path) = &manifest { self.load(path.clone()); }
            if self.loading { ui.spinner(); }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut self.offline, tr("Offline"));
            });
        });
        if manifest != self.manifest && let Some(path) = manifest { self.form(); self.load(path); }
        if let Some(path) = &self.manifest {
            ui.add(egui::Label::new(egui::RichText::new(path.display().to_string()).small().color(rc_kit::pal(ui).muted)).truncate());
        }
    }
    /// Left pane: installed dependencies grouped by table, or crates.io search.
    fn list(&mut self, ui: &mut Ui, busy: bool) {
        let p = rc_kit::pal(ui);
        ui.horizontal(|ui| {
            let installed = format!("{} ({})", tr("Installed"), self.installed.len());
            if ui.selectable_label(self.pane == Pane::Installed, installed).clicked() { self.pane = Pane::Installed; }
            if ui.selectable_label(self.pane == Pane::Search, tr("Search crates.io")).clicked() { self.pane = Pane::Search; }
        });
        ui.separator();
        let height = (ui.available_height() - 36.0).max(80.0);
        match self.pane {
            Pane::Installed => {
                egui::ScrollArea::vertical().id_salt("installed_crate_dependencies").max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show(ui, |ui| {
                    if self.installed.is_empty() && !self.loading {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(tr("No dependencies in this package yet.")).color(p.muted));
                    }
                    let current = self.current().map(|dep| (dep.name.clone(), dep.scope));
                    for scope in [Scope::Normal, Scope::Dev, Scope::Build] {
                        let deps: Vec<Installed> = self.installed.iter().filter(|dep| dep.scope == scope).cloned().collect();
                        if deps.is_empty() { continue; }
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(format!("{}  ·  {}", scope.table(), deps.len())).small().strong().color(p.muted));
                        for dep in deps {
                            let selected = current.as_ref().is_some_and(|(name, scope)| *name == dep.name && *scope == dep.scope);
                            let trailing = if dep.version.is_empty() { dep.path.as_ref().map(|_| tr("local").to_string()).unwrap_or_default() } else { dep.version.clone() };
                            let subtitle = (!dep.source.is_empty()).then_some(dep.source.as_str());
                            let label = format!("{}  {}  [{}]", dep.name, dep.version, dep.scope.table());
                            if row(ui, selected, &dep.name, &trailing, subtitle, &label).clicked() && !busy { self.pick(dep); }
                        }
                    }
                });
            }
            Pane::Search => {
                ui.horizontal(|ui| {
                    let searching = self.search_cancel.is_some();
                    let width = (ui.available_width() - 90.0).max(80.0);
                    let edit = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(tr("Crate name to search")).char_limit(64).desired_width(width));
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let can = !searching && !self.loading && rc_cargo::valid_crate_name(self.query.trim());
                    if searching {
                        ui.spinner();
                        if ui.button(tr("Cancel Search")).clicked() { if let Some(c) = self.search_cancel.take() { c.store(true, Ordering::Relaxed); } self.generation = self.generation.wrapping_add(1); }
                    } else if ui.add_enabled(can, egui::Button::new(tr("Search"))).clicked() || (enter && can) { self.search(); }
                });
                ui.label(egui::RichText::new(tr("Search contacts crates.io only when you press Search. Cargo edits may access configured registries unless Offline is enabled.")).small().color(p.muted));
                let height = (ui.available_height() - 36.0).max(60.0);
                egui::ScrollArea::vertical().id_salt("crate_search_results").max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show(ui, |ui| {
                    if self.hits.is_empty() && self.search_cancel.is_none() && !self.query.trim().is_empty() {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(tr("No results")).color(p.muted));
                    }
                    for hit in self.hits.clone() {
                        let version = hit.version.clone().unwrap_or_default();
                        let label = format!("{}  {}", hit.name, version);
                        let selected = self.name.trim() == hit.name;
                        if row(ui, selected, &hit.name, &version, hit.description.as_deref(), &label).clicked() && !busy {
                            let installed = self.installed.iter().find(|dep| dep.name == hit.name).cloned();
                            match installed {
                                Some(dep) => self.pick(dep),
                                None => { self.form(); self.name = hit.name; self.version = version; }
                            }
                        }
                    }
                });
            }
        }
        ui.add_space(4.0);
        if ui.add_enabled(!busy, egui::Button::new(format!("+  {}", tr("New dependency")))).clicked() { self.form(); }
    }
    /// Right pane: the selected or new dependency and its actions.
    fn details(&mut self, ui: &mut Ui, cargo: Option<&rc_cargo::Cargo>, trusted: bool, clean: bool, busy: bool) {
        let p = rc_kit::pal(ui);
        let current = self.current().cloned();
        let title = if self.name.trim().is_empty() { tr("New dependency").to_string() } else { self.name.trim().to_string() };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(title).font(rc_kit::theme::bold(16.0)));
            if let Some(dep) = &current { ui.label(egui::RichText::new(format!(" {} ", dep.scope.table())).small().color(p.badge_fg).background_color(p.badge_bg)); }
        });
        if let Some(dep) = &current && !dep.source.is_empty() { ui.label(egui::RichText::new(&dep.source).small().color(p.muted)); }
        ui.add_space(6.0);
        ui.add_enabled_ui(!busy, |ui| {
            let width = (ui.available_width() - 180.0).max(120.0);
            egui::Grid::new("dependency_form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(tr("Crate name")); ui.add(egui::TextEdit::singleline(&mut self.name).char_limit(64).desired_width(width)); ui.end_row();
                ui.label(tr("Version requirement")); ui.add(egui::TextEdit::singleline(&mut self.version).hint_text("1.0").char_limit(200).desired_width(width)); ui.end_row();
                ui.label(tr("Features (comma-separated)")); ui.add(egui::TextEdit::singleline(&mut self.features).char_limit(2048).desired_width(width)); ui.end_row();
                ui.label(tr("Local dependency path (optional)")); ui.add(egui::TextEdit::singleline(&mut self.path).char_limit(4096).desired_width(width)); ui.end_row();
                ui.label(tr("Dependency scope")); egui::ComboBox::from_id_salt("dep_scope").width(width - 6.0).selected_text(self.scope.table()).show_ui(ui, |ui| {
                    for scope in [Scope::Normal, Scope::Dev, Scope::Build] { ui.selectable_value(&mut self.scope, scope, scope.table()); }
                }); ui.end_row();
                if self.scope == Scope::Dev { self.optional = false; }
                ui.label("");
                ui.vertical(|ui| {
                    ui.add_enabled_ui(self.scope != Scope::Dev, |ui| { ui.checkbox(&mut self.optional, tr("Optional dependency")); });
                    ui.checkbox(&mut self.defaults, tr("Default features"));
                });
                ui.end_row();
            });
        });
        ui.add_space(6.0);
        if !self.editable { ui.colored_label(p.warn, tr("Edit inherited, Git and alternate-registry dependency settings in Cargo.toml. Removal is supported here.")); }
        ui.label(egui::RichText::new(tr("Cargo add keeps previously enabled features. Remove feature entries in Cargo.toml.")).small().color(p.muted));
        ui.label(egui::RichText::new(tr("Target-specific and workspace-level dependency tables must be edited in Cargo.toml.")).small().color(p.muted));
        ui.add_space(8.0);
        let can_edit = trusted && clean && !busy && cargo.is_some();
        ui.horizontal(|ui| {
            let add = ui.add_enabled_ui(can_edit && self.editable && !self.name.trim().is_empty(), |ui| rc_kit::theme::primary_button(ui, tr("Add or update dependency"))).inner;
            if add.clicked() && let (Some(cargo), Some(manifest)) = (cargo, self.manifest.as_ref()) {
                let path = (!self.path.trim().is_empty()).then(|| {
                    let path = Path::new(self.path.trim());
                    if path.is_absolute() { path.to_path_buf() } else { manifest.parent().unwrap_or(Path::new(".")).join(path) }
                });
                let add = Add { name: self.name.trim().into(), version: self.version.trim().into(), features: self.features.split([',', ' ']).filter(|s| !s.is_empty()).map(str::to_string).collect(), optional: self.optional, no_default_features: !self.defaults, path };
                self.start(cargo, dependencies::add_args(manifest, self.scope, &add, self.offline), trusted);
            }
            if let Some(dep) = &current && self.remove.is_none() && ui.add_enabled(can_edit, egui::Button::new(tr("Remove…")).min_size(egui::vec2(72.0, 28.0))).clicked() {
                self.remove = Some((dep.name.clone(), dep.scope));
            }
        });
        if let Some((name, scope)) = self.remove.clone() {
            ui.add_space(6.0);
            notice(ui, p.error, |ui| {
                ui.label(rc_kit::i18n::trf("Remove {name} from {scope}?", &[("name", &name), ("scope", &scope.table())]));
                if ui.add_enabled(can_edit, egui::Button::new(tr("Remove"))).clicked() && let (Some(cargo), Some(manifest)) = (cargo, self.manifest.as_ref()) {
                    self.start(cargo, dependencies::remove_args(manifest, scope, &name, self.offline), trusted); self.remove = None;
                }
                if ui.button(tr("Cancel")).clicked() { self.remove = None; }
            });
        }
    }
    fn ui(&mut self, ui: &mut Ui, packages: &[rc_cargo::Package], cargo: Option<&rc_cargo::Cargo>, trusted: bool, clean: bool, body: f32) {
        let busy = self.operation.is_some();
        let p = rc_kit::pal(ui);
        self.toolbar(ui, packages, busy);
        if cargo.is_none() { notice(ui, p.warn, |ui| { ui.label(tr("Cargo was not found. Install Rust with rustup.")); }); }
        if !trusted { notice(ui, p.warn, |ui| { ui.label(tr("Trust the project before changing dependencies.")); }); }
        if !clean { notice(ui, p.warn, |ui| { ui.label(tr("Save edited documents before changing dependencies.")); }); }
        ui.add_space(4.0);
        let show_output = !self.output.is_empty() || busy;
        let body = if show_output { body } else { body + self.output_height };
        // Two fixed panes side by side; each scrolls inside its own rectangle.
        let width = ui.available_width().max(200.0);
        let left = (width * 0.38).clamp(180.0, 340.0);
        let gap = 10.0;
        let area = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width, body));
        let left_rect = egui::Rect::from_min_size(area.min, egui::vec2(left, body));
        let right_rect = egui::Rect::from_min_max(egui::pos2(left_rect.right() + gap, area.top()), area.max);
        for (rect, is_left) in [(left_rect, true), (right_rect, false)] {
            ui.painter().rect(rect, 8.0, p.island_bg, egui::Stroke::new(1.0, p.island_border), egui::StrokeKind::Inside);
            let inner = rect.shrink(10.0);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
            child.set_clip_rect(inner.expand(2.0).intersect(ui.clip_rect()));
            if is_left { self.list(&mut child, busy); } else {
                egui::ScrollArea::vertical().id_salt("dependency_details").auto_shrink([false, false]).max_height(inner.height()).show(&mut child, |ui| self.details(ui, cargo, trusted, clean, busy));
            }
        }
        ui.advance_cursor_after_rect(area);
        if let Some(error) = &self.error { ui.add_space(4.0); notice(ui, p.error, |ui| { ui.label(error); }); }
        if show_output {
            let top = ui.cursor().min.y;
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tr("Cargo output")).strong());
            egui::Frame::new().fill(p.editor_bg).stroke(egui::Stroke::new(1.0, p.island_border)).corner_radius(6).inner_margin(8).show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("dependency_output").max_height(100.0).min_scrolled_height(100.0).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&self.output).monospace().size(12.0)).wrap());
                });
            });
            self.output_height = (ui.cursor().min.y - top).clamp(0.0, 400.0);
        }
    }
}
/// A list row: bold name, trailing version and an optional muted line below.
fn row(ui: &mut Ui, selected: bool, title: &str, trailing: &str, subtitle: Option<&str>, label: &str) -> egui::Response {
    let p = rc_kit::pal(ui);
    let height = if subtitle.is_some() { 42.0 } else { 26.0 };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click());
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label);
        info.selected = Some(selected);
        info
    });

    if ui.is_rect_visible(rect) {
        if selected { ui.painter().rect_filled(rect, 5.0, p.selection); } else if response.hovered() { ui.painter().rect_filled(rect, 5.0, p.row_hover); }
        let painter = ui.painter().with_clip_rect(rect.shrink(1.0).intersect(ui.clip_rect()));
        let trailing = painter.layout_no_wrap(trailing.to_string(), egui::FontId::proportional(12.0), p.muted);
        let top = rect.top() + 13.0;
        painter.galley(egui::pos2(rect.right() - 8.0 - trailing.size().x, top - trailing.size().y / 2.0), trailing.clone(), p.muted);
        let title_clip = painter.with_clip_rect(egui::Rect::from_min_max(rect.min, egui::pos2(rect.right() - 16.0 - trailing.size().x, rect.bottom())).intersect(painter.clip_rect()));
        let title = title_clip.layout_no_wrap(title.to_string(), rc_kit::theme::bold(13.0), p.text);
        title_clip.galley(egui::pos2(rect.left() + 8.0, top - title.size().y / 2.0), title, p.text);
        if let Some(subtitle) = subtitle {
            let line = subtitle.lines().next().unwrap_or("");
            let galley = painter.layout_no_wrap(line.to_string(), egui::FontId::proportional(11.5), p.muted);
            painter.galley(egui::pos2(rect.left() + 8.0, rect.top() + 29.0 - galley.size().y / 2.0), galley, p.muted);
        }
    }
    response
}
/// A tinted single-line banner (warnings, errors and confirmations).
fn notice(ui: &mut Ui, color: egui::Color32, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(color.gamma_multiply(0.12)).stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.5))).corner_radius(6).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(add);
    });
}
impl Drop for CargoDependencies { fn drop(&mut self) { self.cancel(); } }
fn append(output: &mut String, line: &str) {
    if output.len() < 64 * 1024 { output.extend(rc_dotnet::strip_ansi(line).chars().take(2048)); output.push('\n'); }
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
    let clean = !app.docs.list.iter().any(|d|d.is_modified()&&!d.generated)&&!app.disk_save.saves.busy()&&app.rustfmt_jobs.is_empty();
    let manager = &mut app.cargo_dependencies;
    if manager.manifest.is_none() && let Some(package) = packages.first() { manager.load(package.manifest.clone()); }
    manager.poll();
    let reload = std::mem::take(&mut manager.reload_project);
    let trusted = app.workspace.as_ref().is_some_and(|w| app.settings.trusted_projects.contains(&w.ws.root));
    let mut close = false;
    let screen = ctx.content_rect().size();
    let width = (screen.x - 48.0).clamp(320.0, 980.0);
    let body = (screen.y - 330.0).clamp(220.0, 460.0);
    egui::Modal::new(egui::Id::new("cargo_dependency_manager")).show(ctx, |ui| {
        ui.set_width(width);
        ui.heading(tr("Cargo Dependencies"));
        ui.add_space(4.0);
        manager.ui(ui, &packages, cargo.as_ref(), trusted, clean, body);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if manager.operation.is_some() {
                ui.spinner();
                if ui.button(tr("Cancel dependency change")).clicked() && let Some(operation) = &manager.operation { operation.cancel(); }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if rc_kit::theme::secondary_button(ui, tr("Close")).clicked() { close = true; }
            });
        });
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
    #[track_caller]
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
        app.open_workspace_and_wait(&root);
        app.trust_project_tools_for_test();
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
        let mut app=test_app(home.path(),true); app.open_workspace_and_wait(&root); app.trust_project_tools_for_test(); let mut h=harness(app,egui::vec2(1440.0,1100.0));
        pump(&mut h,|app| app.runner.cargo_workspace.is_some()); h.state_mut().open_file_and_wait(&root.join("app/src/lib.rs")); h.state_mut().docs.active_doc_mut().unwrap().editor.selection=rc_text::Selection::point(0); h.run_steps(3);
        h.event(egui::Event::Paste("use serde::de::DeserializeOwned;\nfn decode<T: DeserializeOwned>() {}\n#[cfg(test)] mod tests { use helper_lib::value; #[test] fn local(){assert_eq!(value(),42);} }\n".into())); h.run_steps(2);
        pump(&mut h,|app| app.missing_crates.report.proposals.len()==2);
        assert!(h.state().missing_crates.report.proposals.iter().all(|proposal| proposal.pasted));
        assert_eq!(std::fs::read(&manifest).unwrap(),before); assert!(h.state().cargo_dependencies.search_cancel.is_none()); assert!(h.state().cargo_dependencies.operation.is_none());
        // Review is safe before trust: no search or edit starts. The existing Add stays gated.
        let proposal=h.state().missing_crates.report.proposals.iter().find(|proposal| proposal.name=="serde").unwrap().clone();
        h.state_mut().cargo_dependencies.review(&proposal.manifest,&proposal.name,proposal.scope,None); h.state_mut().dialog=Some(crate::ui::dialogs::Dialog::CargoDependencies); h.run_steps(3); pump(&mut h,|app| !app.cargo_dependencies.loading);
        assert_eq!(h.state().cargo_dependencies.manifest.as_ref(),Some(&manifest)); assert_eq!(h.state().cargo_dependencies.scope,Scope::Normal); assert!(h.state().cargo_dependencies.search_cancel.is_none());
        let cargo=rc_cargo::Cargo::locate().unwrap(); h.state_mut().cargo_dependencies.start(&cargo,Ok(vec![]),false); assert!(h.state().cargo_dependencies.operation.is_none());
        h.state_mut().save_all_and_wait();
        h.state_mut().settings.trusted_projects.push(root.clone()); h.state_mut().cargo_dependencies.offline=true; h.run_steps(2); h.get_by_label(tr("Add or update dependency")).click(); h.run_steps(2);
        pump(&mut h,|app| app.cargo_dependencies.operation.is_none() && !app.cargo_dependencies.loading && app.cargo_dependencies.installed.iter().any(|dependency| dependency.name=="serde"));
        assert!(!root.join("app/build-ran").exists()); assert!(!root.join("helper-lib/build-ran").exists());
        h.get_by_label(tr("Close")).click(); h.run_steps(2);
        pump(&mut h,|app| app.missing_crates.report.proposals.len()==1 && app.missing_crates.report.proposals[0].name=="helper-lib");
        assert_eq!(h.state().missing_crates.report.proposals[0].scope,Scope::Dev);
        h.run_steps(5);
        h.get_by_label(tr("Review dependency…")).click(); h.run_steps(2); pump(&mut h,|app| !app.cargo_dependencies.loading);
        assert_eq!(h.state().cargo_dependencies.scope,Scope::Dev); assert_eq!(h.state().cargo_dependencies.path,root.join("helper-lib").to_string_lossy());
        h.run_steps(5);
        h.get_by_label(tr("Add or update dependency")).click(); h.run_steps(5); pump(&mut h,|app| app.cargo_dependencies.operation.is_none() && !app.cargo_dependencies.loading && app.cargo_dependencies.installed.iter().any(|dependency| dependency.name=="helper-lib"));
        assert_eq!(std::fs::read(root.join("helper-lib/Cargo.toml")).unwrap(),helper_before); assert!(!root.join("app/build-ran").exists());
        let active=h.state().docs.active.unwrap(); h.state_mut().docs.save(active).unwrap();
        let output=std::process::Command::new("cargo").args(["check","--offline","--tests","--manifest-path"]).arg(&manifest).output().unwrap(); assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        h.get_by_label(tr("Close")).click(); h.run_steps(2);
        h.event(egui::Event::Text("use regex::Regex;\n".into())); h.run_steps(2);
        pump(&mut h,|app| app.missing_crates.report.proposals.iter().any(|proposal| proposal.name=="regex" && !proposal.pasted));
        assert!(h.state().cargo_dependencies.operation.is_none()); assert!(h.state().cargo_dependencies.search_cancel.is_none());

    }

    #[test]
    #[ignore = "Explicit public crates.io lookup; requires network and cached itoa for offline Cargo add"]
    fn native_lookup_finds_a_non_catalog_crate_and_checks_the_real_rust_project() {
        use egui_kittest::kittest::Queryable;
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("src")).unwrap();
        let manifest = project.path().join("Cargo.toml");
        std::fs::write(&manifest, "[package]\nname='lookup-audit'\nversion='0.1.0'\nedition='2024'\n[workspace]\n").unwrap();
        let source = project.path().join("src/main.rs");
        std::fs::write(&source, "fn main() { let mut buffer = itoa::Buffer::new(); assert_eq!(buffer.format(42), \"42\"); }\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(project.path()); app.trust_project_tools_for_test(); app.open_file_and_wait(&source);
        let mut h = crate::screens::harness(app, egui::vec2(1440.0, 1100.0));
        let wait = |h: &mut egui_kittest::Harness<'_, crate::app::CrabCraftApp>, done: &dyn Fn(&crate::app::CrabCraftApp) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(25);
            while !done(h.state()) { h.run_steps(2); assert!(Instant::now() < deadline, "native lookup did not settle"); std::thread::sleep(Duration::from_millis(10)); }
        };
        wait(&mut h, &|a| a.missing_crates.report.proposals.iter().any(|p| p.root == "itoa"));
        assert!(!project.path().join("target").exists());
        h.get_by_label(tr("Look up crate…")).click(); h.run_steps(2);
        assert!(h.state().cargo_dependencies.name.is_empty());
        assert_eq!(h.state().cargo_dependencies.query, "itoa");
        // Review itself never contacts the registry or changes the manifest.
        assert!(!std::fs::read_to_string(&manifest).unwrap().contains("[dependencies]"));
        // Loading is asynchronous; native Search becomes enabled after the manifest read.
        wait(&mut h, &|a| !a.cargo_dependencies.loading);
        h.run_steps(4);
        h.get_by_label(tr("Search")).click(); h.run_steps(4);
        wait(&mut h, &|a| a.cargo_dependencies.search_cancel.is_none());
        let manager = &h.state().cargo_dependencies;
        let hit = manager.hits.iter().find(|hit| hit.name == "itoa").unwrap_or_else(|| panic!("search failed: {:?}; {:?}", manager.error, manager.hits));
        let label = format!("itoa  {}", hit.version.as_deref().unwrap_or_default());
        h.run_steps(5);
        h.get_by_label(&label).click(); h.run_steps(5);
        assert_eq!(h.state().cargo_dependencies.name, "itoa");
        h.state_mut().cargo_dependencies.version = "1.0".into();
        h.get_by_label(tr("Offline")).click(); h.run_steps(1);
        h.run_steps(5);
        h.get_by_label(tr("Add or update dependency")).click(); h.run_steps(5);
        wait(&mut h, &|a| a.cargo_dependencies.operation.is_none() && std::fs::read_to_string(&manifest).unwrap().contains("itoa"));
        let result = std::process::Command::new("cargo").args(["run", "--offline", "--manifest-path"]).arg(&manifest).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }
}
