//! The application: workspace, open documents, tool window layout, settings and session, and
//! the frame loop that ties them together.

use crate::docs::Docs;
use crate::insight::{Insight, InsightOutput, NavKind, Target};
use crate::runner::Runner;
use crate::native_menu::{MenuCommand, NativeMenu};
use crate::toolwindow::{Dock, ToolWindow};
use crate::ui::dialogs::Dialog;
use crate::ui::explorer::{ExplorerAction, ExplorerState};
use egui::{Context, Ui};
use rc_core::session::{OpenFile, Session};
use rc_core::settings::{KeymapChoice, ThemeChoice};
use rc_core::watch::{FileWatcher, FsEvent};
use rc_core::{Inbox, Settings, Waker, Workspace};
use rc_dotnet::Solution;
use rc_editor::{Action, EditorConfig, Keymap, KeymapKind};
use rc_kit::i18n::{self, Lang, tr, trf};
use std::path::{Path, PathBuf};

/// A message in the status bar / notification balloon.
#[derive(Clone, Debug)]
pub struct Notice {
    pub text: String,
    pub error: bool,
    pub at: f64,
}

pub struct WorkspaceState {
    pub ws: Workspace,
    pub solution: Option<Solution>,
}

pub struct CrabCraftApp {
    pub settings: Settings,
    pub keymap: Keymap,
    pub editor_config: EditorConfig,
    pub workspace: Option<WorkspaceState>,
    pub docs: Docs,
    pub dock: Dock,
    pub explorer: ExplorerState,
    pub dialog: Option<Dialog>,
    pub notices: Vec<Notice>,
    pub insight: Insight,
    pub runner: Runner,
    pub debugger: crate::debugger::Debugger,
    window_title: String,
    pub files: crate::search::Files,
    pub vcs: crate::vcs::Vcs,
    pub nuget: crate::ui::nuget::NugetManager,
    /// What is typed in the Cargo window.
    pub cargo_input: String,
    /// Members folded in the Cargo window.
    pub cargo_folded: std::collections::BTreeSet<String>,
    /// Editor groups, splits and floating editor windows.
    pub layout: crate::layout::Layout,
    /// A tab being dragged to another place.
    pub tab_drag: Option<crate::ui::editor_area::TabDrag>,
    /// Keyboard focus and menu actions for the active editor, wherever it is shown this frame.
    pub editor_frame: Option<(bool, Vec<Action>)>,
    pub new_project: Option<crate::ui::new_project::NewProjectDialog>,
    pub search_everywhere: Option<crate::search::SearchEverywhere>,
    pub find_bar: Option<crate::search::FindBar>,
    pub find_in_files: Option<crate::search::FindInFiles>,
    pub terminals: rc_term::Terminals,
    pub focus_terminal: bool,
    /// Results shown in the Find tool window (usages, implementations…).
    pub find_results: Option<(String, Vec<Target>)>,
    /// Screen position under the active caret (popups that belong to the editor).
    pub caret_screen: Option<egui::Pos2>,
    pub test_markers: Vec<crate::runner::TestMarker>,
    /// The Rust test being debugged, for Restart.
    debug_test: Option<(crate::runner::RunConfig, rc_cargo::RunTarget)>,
    /// The Run/Debug menu of a gutter icon: its line and where it opens.
    pub gutter_menu: Option<(usize, egui::Pos2)>,
    /// Binary target run by the icon of each line of the active Rust document.
    pub run_markers: std::collections::BTreeMap<usize, String>,
    /// Lines of the active document that run a benchmark, with the `cargo` arguments.
    pub bench_markers: std::collections::BTreeMap<usize, Vec<String>>,
    pub native_menu: Option<NativeMenu>,
    pub waker: Waker,
    fs_inbox: Inbox<FsEvent>,
    _watcher: Option<FileWatcher>,
    cargo_load: Option<Inbox<Result<rc_cargo::CargoWorkspace, String>>>,
    /// What Settings › Tools shows, read once it is needed.
    pub toolchain: Option<rc_cargo::ToolchainInfo>,
    libraries_load: Option<Inbox<Vec<rc_cargo::Library>>>,
    toolchain_load: Option<Inbox<rc_cargo::ToolchainInfo>>,
    cargo_remembered: Option<String>,
    /// Actions waiting for the UI pass (from the native menu or global shortcuts).
    pub pending: Vec<Action>,
    /// Give the editor keyboard focus on the next frame.
    pub focus_editor: bool,
    /// Back/forward navigation: (file, char offset).
    pub nav_back: Vec<(PathBuf, usize)>,
    pub nav_forward: Vec<(PathBuf, usize)>,
    /// Parsed Markdown and loaded images of the previews.
    pub md_cache: egui_commonmark::CommonMarkCache,
    applied_dark: Option<bool>,
    applied_zoom: Option<f32>,
    pub shift_taps: (f64, u8, bool),
    pub ctrl_taps: (f64, u8, bool),
    pub session_dirty: bool,
    pub last_session_save: f64,
    /// Editor keeps the focus when the window regains it.
    pub had_focus: bool,
}

/// The edits of a compiler suggestion in characters of the editor, which reads `\r\n` as `\n`.
fn fix_changes(text: &str, fix: &rc_dotnet::diagnostics::Fix) -> Option<Vec<rc_text::Change>> {
    let chars = |byte: usize| text.get(..byte).map(|t| t.chars().count() - t.matches("\r\n").count());
    let changes = fix.edits.iter().map(|(start, end, new)| Some(rc_text::Change { start: chars(*start)?, end: chars(*end)?, text: new.clone() }));
    changes.collect()
}

/// Text or a key other than the modifiers, which egui also reports as keys.
fn interrupts_tap(event: &egui::Event) -> bool {
    use egui::Key::{AltLeft, AltRight, ControlLeft, ControlRight, ShiftLeft, ShiftRight};
    match event {
        egui::Event::Key { key, .. } => !matches!(key, AltLeft | AltRight | ControlLeft | ControlRight | ShiftLeft | ShiftRight),
        egui::Event::Text(_) => true,
        _ => false,
    }
}

/// Follows a modifier: `(time of the last press, presses in a row, whether it is down)`. Anything
/// else pressed meanwhile cancels the count. True on the second press.
fn double_tap(state: &mut (f64, u8, bool), down: bool, interrupted: bool, now: f64) -> bool {
    let (last, taps, was_down) = *state;
    if interrupted {
        *state = (0.0, 0, down);
    } else if down && !was_down {
        let taps = if now - last < 0.4 { taps + 1 } else { 1 };
        *state = (now, taps, true);
        if taps >= 2 {
            *state = (0.0, 0, true);
            return true;
        }
    } else if !down && was_down {
        state.2 = false;
    }
    false
}

impl CrabCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let settings = Settings::load();
        i18n::set_lang(settings.language.as_deref().map(Lang::from_locale).unwrap_or_else(Lang::system));
        rc_kit::theme::install_fonts(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let waker = Waker::new(move || ctx.request_repaint());
        let keymap = Keymap::new(keymap_kind(settings.keymap));
        let native_menu = NativeMenu::install(&cc.egui_ctx, &keymap);
        let mut app = Self::with_settings(settings, waker, native_menu);
        crate::open_files::set_context(&cc.egui_ctx);
        // Files sent by Finder at launch arrive before the first frame: open them instead of
        // the last project.
        let mut from_system = crate::open_files::take();
        from_system.extend(open);
        for path in startup_paths(&app.settings, from_system) {
            app.open_path(&path);
        }
        app
    }

    /// Builds the app without a window (tests and headless captures).
    pub fn with_settings(settings: Settings, waker: Waker, native_menu: Option<NativeMenu>) -> Self {
        let keymap = Keymap::new(keymap_kind(settings.keymap));
        let editor_config = editor_config(&settings);
        let pool = rc_syntax::ParserPool::new(waker.as_fn());
        CrabCraftApp {
            keymap,
            editor_config,
            workspace: None,
            docs: Docs::new(pool),
            dock: Dock::default(),
            explorer: ExplorerState::default(),
            dialog: None,
            notices: Vec::new(),
            insight: Insight::new(waker.clone()),
            runner: {
                let mut runner = Runner::new(waker.clone());
                runner.nextest = settings.use_nextest;
                runner
            },
            debugger: crate::debugger::Debugger::new(waker.clone()),
            window_title: String::new(),
            files: crate::search::Files::new(),
            vcs: crate::vcs::Vcs::new(waker.clone()),
            nuget: crate::ui::nuget::NugetManager::new(waker.clone()),
            cargo_input: String::new(),
            toolchain: None,
            libraries_load: None,
            toolchain_load: None,
            cargo_folded: Default::default(),
            layout: Default::default(),
            tab_drag: None,
            editor_frame: None,
            new_project: None,
            search_everywhere: None,
            find_bar: None,
            find_in_files: None,
            terminals: rc_term::Terminals::new(),
            focus_terminal: false,
            find_results: None,
            caret_screen: None,
            test_markers: Vec::new(),
            gutter_menu: None,
            debug_test: None,
            run_markers: std::collections::BTreeMap::new(),
            bench_markers: std::collections::BTreeMap::new(),
            native_menu,
            fs_inbox: Inbox::new(waker.clone()),
            waker,
            _watcher: None,
            cargo_load: None,
            cargo_remembered: None,
            pending: Vec::new(),
            focus_editor: false,
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            md_cache: Default::default(),
            applied_dark: None,
            applied_zoom: None,
            shift_taps: (0.0, 0, false),
            ctrl_taps: (0.0, 0, false),
            session_dirty: false,
            last_session_save: 0.0,
            had_focus: true,
            settings,
        }
    }

    pub fn dark(&self, ctx: &Context) -> bool {
        match self.settings.theme {
            ThemeChoice::Dark => true,
            ThemeChoice::Light => false,
            ThemeChoice::System => ctx.system_theme().is_none_or(|t| t == egui::Theme::Dark),
        }
    }

    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notices.push(Notice { text: text.into(), error, at: 0.0 });
    }

    // ------------------------------------------------------------------ workspace

    /// Opens a folder, solution or project, restoring its session.
    pub fn open_workspace(&mut self, path: &Path) {
        let ws = match Workspace::open(path) {
            Ok(ws) => ws,
            Err(e) => {
                self.notify(trf("Could not open {path}: {error}", &[("path", &path.display()), ("error", &e)]), true);
                return;
            }
        };
        self.close_workspace();
        self.nuget = crate::ui::nuget::NugetManager::new(self.waker.clone());
        let (solution, solution_error) = match ws.solution() {
            Some(sln) => match Solution::load(sln) {
                Ok(s) => (Some(s), None),
                Err(e) => (None, Some(e.to_string())),
            },
            None => (None, None),
        };
        if let Some(err) = &solution_error {
            self.notify(trf("Could not read the solution: {error}", &[("error", err)]), true);
        }
        self.settings.add_recent(ws.key_path().to_path_buf());
        self.settings.last_workspace = Some(ws.key_path().to_path_buf());
        let _ = self.settings.save();
        self._watcher = FileWatcher::new(&ws.root, self.fs_inbox.sender()).ok();
        self.files.open(&ws.root, self.waker.clone());
        self.vcs.open(&ws.root);
        let session = Session::load(ws.key_path());
        let projects = match &ws.kind {
            rc_core::WorkspaceKind::Project(p) => vec![p.clone()],
            rc_core::WorkspaceKind::Folder => find_projects(&ws.root, 4),
            rc_core::WorkspaceKind::Solution(_) | rc_core::WorkspaceKind::Cargo(_) => Vec::new(),
        };
        let solution_path = if solution.is_some() { ws.solution().map(Path::to_path_buf) } else { None };
        if ws.cargo_manifest().is_some() {
            if self.settings.trusted_projects.iter().any(|p| p == &ws.root) {
                self.insight.start_rust(&ws.root, self.settings.rust_analyzer_path.clone(), self.settings.use_clippy);
            } else {
                self.insight.wait_for_trust(&ws.root);
            }
        } else {
            self.insight.start(&ws.root, solution_path.as_deref(), projects.clone(), dotnet_path(&self.settings), self.settings.roslyn_path.clone());
        }
        self.runner.compounds = session.compounds.clone();
        self.runner.options = session.run_options.clone();
        self.runner.set_workspace(solution.as_ref(), &projects, &dotnet_path(&self.settings), session.run_configuration.as_deref());
        self.cargo_remembered = session.run_configuration.clone();
        let manifest = ws.cargo_manifest().map(Path::to_path_buf);
        self.workspace = Some(WorkspaceState { ws, solution });
        self.restore_session(session);
        if let Some(manifest) = manifest {
            self.load_cargo_workspace(manifest);
        }
    }

    /// Reads the Rust workspace in the background; `cargo metadata` compiles nothing, so this is
    /// safe on a project that is not trusted.
    fn load_cargo_workspace(&mut self, manifest: PathBuf) {
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        std::thread::spawn(move || {
            let result = match rc_cargo::Cargo::locate() {
                Some(cargo) => rc_cargo::load_workspace(&cargo, &manifest).map_err(|e| e.to_string()),
                None => Err(tr("Cargo was not found. Install Rust with rustup.").to_string()),
            };
            out.send(result);
        });
        self.cargo_load = Some(inbox);
    }

    pub(crate) fn poll_cargo_workspace(&mut self) {
        if let Some(libraries) = self.libraries_load.as_ref().and_then(|inbox| inbox.drain().into_iter().last()) {
            self.libraries_load = None;
            // Canonical like the documents' paths (`\\?\C:\…` on Windows).
            self.docs.read_only_roots = libraries.iter().filter(|l| l.read_only).map(|l| l.root.canonicalize().unwrap_or_else(|_| l.root.clone())).collect();
            self.explorer.libraries = libraries;
        }
        let Some(inbox) = &self.cargo_load else { return };
        for result in inbox.drain() {
            match result {
                Ok(workspace) => {
                    self.runner.set_cargo_workspace(workspace, self.cargo_remembered.as_deref());
                    self.load_libraries();
                }
                Err(e) => self.notify(trf("Could not read the Cargo project: {error}", &[("error", &e)]), true),
            }
            self.cargo_load = None;
            return;
        }
    }

    /// Reads the dependencies in the background; without an up-to-date `Cargo.lock` only the
    /// standard library is known.
    fn load_libraries(&mut self) {
        let Some(manifest) = self.runner.cargo_workspace.as_ref().map(|w| w.manifest.clone()) else { return };
        let project = self.workspace.as_ref().map(|w| w.ws.root.clone()).filter(|r| self.settings.trusted_projects.contains(r));
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        std::thread::spawn(move || {
            let Some(cargo) = rc_cargo::Cargo::locate() else { return };
            let mut list = rc_cargo::load_libraries(&cargo, &manifest).unwrap_or_default();
            let info = rc_cargo::detect_toolchain(Some(&cargo), None, project.as_deref());
            if let Some(root) = info.stdlib {
                let version = info.rustc.as_deref().and_then(|v| v.split_whitespace().nth(1)).unwrap_or_default().to_string();
                list.insert(0, rc_cargo::Library { name: "stdlib".into(), version, root, read_only: true });
            }
            out.send(list);
        });
        self.libraries_load = Some(inbox);
    }

    /// Saves and commits, with the Rust files of the commit formatted if that was asked for.
    pub fn commit_changes(&mut self, push: bool) {
        self.save_all_docs();
        let format = self.commit_format();
        self.vcs.commit(push, format);
    }

    /// The tool and the files to format before a commit.
    fn commit_format(&self) -> Option<(rc_cargo::Cargo, Vec<rc_cargo::FormatFile>)> {
        if !self.settings.format_on_commit {
            return None;
        }
        let workspace = self.runner.cargo_workspace.as_ref()?;
        let root = self.vcs.root()?;
        let files = self.vcs.included().into_iter().filter_map(|relative| {
            let path = root.join(relative);
            let rust = path.extension().is_some_and(|e| e == "rs") && path.is_file();
            let edition = workspace.edition_of(&path).filter(|_| rust)?.to_string();
            Some(rc_cargo::FormatFile { path, edition })
        });
        Some((self.runner.cargo_tool().ok()?, files.collect()))
    }

    /// Reads the versions of the toolchain in the background, once.
    pub fn load_toolchain(&mut self) {
        if let Some(inbox) = &self.toolchain_load {
            if let Some(info) = inbox.drain().into_iter().last() {
                self.toolchain = Some(info);
                self.toolchain_load = None;
            }
            return;
        }
        if self.toolchain.is_some() {
            return;
        }
        let project = self.workspace.as_ref().map(|w| w.ws.root.clone()).filter(|r| self.settings.trusted_projects.contains(r));
        let configured = self.settings.rust_analyzer_path.clone();
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        std::thread::spawn(move || {
            let analyzer = rc_cargo::locate_rust_analyzer(configured.as_deref());
            out.send(rc_cargo::detect_toolchain(rc_cargo::Cargo::locate().as_ref(), analyzer.as_deref(), project.as_deref()));
        });
        self.toolchain_load = Some(inbox);
    }

    /// Reads the Cargo project again.
    pub fn reload_cargo(&mut self) {
        if let Some(manifest) = self.runner.cargo_workspace.as_ref().map(|w| w.manifest.clone()) {
            self.load_cargo_workspace(manifest);
        }
    }

    /// Runs a target of the Cargo window: binaries go through their run configuration.
    pub fn run_cargo_target(&mut self, ctx: &Context, package: &str, target: &rc_cargo::Target) {
        use crate::runner::ConfigKind;
        let config = self.runner.configs.iter().position(|c| matches!(&c.kind, ConfigKind::Cargo { package: p, bin } if p == package && *bin == target.name));
        if let (rc_cargo::TargetKind::Bin, Some(index)) = (target.kind, config) {
            self.runner.selected = index;
            self.session_dirty = true;
            self.run_ide_action(ctx, Action::Run);
            return;
        }
        match rc_cargo::target_command(package, target) {
            Some(args) => self.run_cargo_args(args),
            None => self.notify(tr("Cargo cannot run this target"), false),
        }
    }

    /// Runs what the user typed in the Cargo window.
    pub fn run_cargo_command(&mut self, text: &str) {
        let args = rc_cargo::command_line(text);
        if !args.is_empty() {
            self.run_cargo_args(args);
        }
    }

    fn run_cargo_args(&mut self, args: Vec<String>) {
        self.save_all_docs();
        let outs = self.runner.cargo_command(args);
        crate::ui::run_windows::handle_outputs(self, outs);
    }

    /// The bookmarks for the list, with the text of their lines.
    pub fn bookmark_list(&self) -> Vec<crate::insight::Target> {
        let mut out = Vec::new();
        for (path, lines) in self.docs.bookmarks() {
            let text = match self.docs.index_of(&path) {
                Some(i) => self.docs.list[i].editor.buffer.rope().to_string(),
                None => std::fs::read_to_string(&path).unwrap_or_default(),
            };
            let source: Vec<&str> = text.lines().collect();
            for line in lines {
                let preview = source.get(line).map_or(String::new(), |l| l.trim().to_string());
                out.push(crate::insight::Target { path: path.clone(), line, col: 0, preview });
            }
        }
        out
    }

    /// Runs what Run Anything offered.
    pub fn run_anything(&mut self, ctx: &Context, item: crate::run_anything::Item) {
        match item {
            crate::run_anything::Item::Config(index) => {
                self.runner.selected = index;
                self.session_dirty = true;
                self.run_ide_action(ctx, Action::Run);
            }
            crate::run_anything::Item::Command(text) => {
                crate::run_anything::remember(&mut self.settings.run_anything_history, &text);
                let _ = self.settings.save();
                self.run_cargo_command(&text);
            }
        }
    }

    /// Switches the linter that runs on save.
    pub fn toggle_linter(&mut self) {
        self.settings.use_clippy = !self.settings.use_clippy;
        let _ = self.settings.save();
        self.insight.set_clippy(self.settings.use_clippy);
    }

    /// The user trusts the open project: its code may now run for code analysis.
    pub fn trust_project(&mut self) {
        let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) else { return };
        if !self.settings.trusted_projects.contains(&root) {
            self.settings.trusted_projects.push(root.clone());
            let _ = self.settings.save();
        }
        self.insight.start_rust(&root, self.settings.rust_analyzer_path.clone(), self.settings.use_clippy);
    }

    /// Every project of the workspace (its solution's, or the ones found in the folder).
    pub fn solution_projects(&self) -> Vec<PathBuf> {
        let Some(w) = &self.workspace else { return Vec::new() };
        match (&w.solution, &w.ws.kind) {
            (Some(s), _) => s.projects().into_iter().map(|p| p.path.clone()).filter(|p| rc_core::workspace::is_project(p)).collect(),
            (None, rc_core::WorkspaceKind::Project(p)) => vec![p.clone()],
            _ => find_projects(&w.ws.root, 4),
        }
    }

    /// Opens the workspace again (after adding a project: run configurations, explorer and the
    /// language server pick it up).
    pub fn reload_workspace(&mut self) {
        if let Some(path) = self.workspace.as_ref().map(|w| w.ws.key_path().to_path_buf()) {
            self.open_workspace(&path);
        }
    }

    /// Edit the run configuration `name` (the selected one when `None`): an application's
    /// options or a compound's members.
    pub fn edit_run_config(&mut self, name: Option<String>) {
        use crate::runner::{ConfigKind, RunConfig};
        let cfg = match name {
            Some(n) => self.runner.configs.iter().find(|c| c.name == n).cloned(),
            None => self.runner.selected_config().cloned(),
        };
        match cfg {
            Some(RunConfig { name, kind: ConfigKind::Cargo { .. } }) => {
                let Some(project) = self.runner.cargo_workspace.as_ref().map(|w| w.manifest.clone()) else { return };
                let options = self.runner.options.get(&name).cloned().unwrap_or_default();
                self.dialog = Some(Dialog::RunOptions { name, project, profiles: Vec::new(), options, web: false });
            }
            Some(RunConfig { name, kind: ConfigKind::App { project, web, .. } }) => {
                let profiles = rc_dotnet::LaunchSettings::load(&project).ok().flatten().map(|s| s.profiles.iter().filter(|p| p.is_project()).map(|p| p.name.clone()).collect()).unwrap_or_default();
                let options = self.runner.options.get(&name).cloned().unwrap_or_default();
                self.dialog = Some(Dialog::RunOptions { name, project, profiles, options, web });
            }
            Some(RunConfig { name, kind: ConfigKind::Compound { members } }) => {
                let all = self.runner.app_names().into_iter().map(|n| {
                    let on = members.contains(&n);
                    (n, on)
                });
                self.dialog = Some(Dialog::Compound { old: Some(name.clone()), name, members: all.collect() });
            }
            Some(_) => self.notify(tr("Test configurations have nothing to edit"), false),
            None => self.notify(tr("There is no run configuration to edit"), false),
        }
    }

    /// New Solution, or Add New Project when `solution` is given.
    pub fn new_project_dialog(&mut self, _solution: Option<PathBuf>) {
        self.new_project = Some(crate::ui::new_project::NewProjectDialog::new(self, self.waker.clone()));
    }

    /// File › Close Project: back to the welcome screen, which is also where the next start
    /// begins.
    pub fn close_project(&mut self) {
        self.close_workspace();
        self.settings.last_workspace = None;
        if let Err(e) = self.settings.save() {
            self.notify(trf("Could not save the settings: {error}", &[("error", &e)]), true);
        }
    }

    pub fn close_workspace(&mut self) {
        if self.workspace.is_some() {
            self.save_all_docs();
            self.save_session();
        }
        self.workspace = None;
        self._watcher = None;
        self.insight.stop();
        self.runner.stop_all();
        self.runner.clear();
        self.files.close();
        self.vcs.close();
        self.find_in_files = None;
        self.search_everywhere = None;
        self.find_bar = None;
        self.terminals = rc_term::Terminals::new();
        self.find_results = None;
        self.docs.list.clear();
        self.docs.active = None;
        self.layout = Default::default();
        self.tab_drag = None;
        self.explorer = ExplorerState::default();
        self.docs.read_only_roots.clear();
        self.libraries_load = None;
        self.dock = Dock::default();
        self.nav_back.clear();
        self.nav_forward.clear();
    }

    fn restore_session(&mut self, s: Session) {
        // A solution without a layout of its own starts with the last one used.
        self.dock = Dock::from_session(if s.tool_windows.is_empty() { &self.settings.tool_windows } else { &s.tool_windows });
        self.explorer.expanded = s.expanded.iter().cloned().collect();
        if self.explorer.expanded.is_empty() {
            // First open: the solution and its top folders expanded, like Rider.
            self.explorer.expanded.insert("sln".into());
            if let Some(sln) = self.workspace.as_ref().and_then(|w| w.solution.as_ref()) {
                for item in &sln.items {
                    if let rc_dotnet::SolutionItem::Folder { name, .. } = item {
                        self.explorer.expanded.insert(format!("sf:/{name}"));
                    }
                }
            }
        }
        // Editor id of each file of the session, for the layout.
        let mut ids = Vec::with_capacity(s.files.len());
        for f in &s.files {
            ids.push(None);
            if !f.path.exists() {
                continue;
            }
            if let Ok(i) = self.docs.open(&f.path) {
                if let Some(last) = ids.last_mut() {
                    *last = Some(self.docs.list[i].editor.id);
                }
                let d = &mut self.docs.list[i];
                d.pinned = f.pinned;
                let pos = d.editor.buffer.pos_at(f.line, f.col);
                d.editor.selection = rc_text::Selection::point(pos);
                if d.editor.syntax.tree().is_none() {
                    d.editor.parse_blocking();
                }
                for &line in &f.folded {
                    d.editor.toggle_fold(line);
                }
                d.editor.set_scroll_offset(egui::vec2(0.0, f.scroll_y));
            }
        }
        self.docs.active = s.active.filter(|&a| a < self.docs.list.len()).or(if self.docs.list.is_empty() { None } else { Some(0) });
        self.layout = crate::layout::Layout::from_session(s.editor_layout.as_ref(), &s.editor_windows, |i| ids.get(i).copied().flatten());
        self.sync_layout();
        for (path, lines) in &s.bookmarks {
            let lines: std::collections::BTreeSet<usize> = lines.iter().copied().collect();
            match self.docs.index_of(path) {
                Some(i) => self.docs.list[i].editor.bookmarks = lines,
                None => {
                    self.docs.closed_bookmarks.insert(path.clone(), lines);
                }
            }
        }
        for (path, bps) in &s.breakpoints {
            if let Some(i) = self.docs.index_of(path) {
                for (line, cond) in bps {
                    let cond = cond.as_ref().filter(|c| !c.is_empty());
                    let kind = if cond.is_some() { rc_editor::BreakpointKind::Conditional } else { rc_editor::BreakpointKind::Normal };
                    self.docs.list[i].editor.breakpoints.insert(*line, kind);
                    if let Some(c) = cond {
                        self.debugger.conditions.insert((path.clone(), *line), c.clone());
                    }
                }
                for (line, message) in s.logpoints.get(path).into_iter().flatten() {
                    if self.docs.list[i].editor.breakpoints.insert(*line, rc_editor::BreakpointKind::Log).is_some() {
                        self.debugger.logs.insert((path.clone(), *line), message.clone());
                    } else {
                        self.docs.list[i].editor.breakpoints.remove(line);
                    }
                }
                for line in s.remove_once_hit.get(path).into_iter().flatten() {
                    self.debugger.once.insert((path.clone(), *line));
                }
                for line in s.disabled_breakpoints.get(path).into_iter().flatten() {
                    if let Some(k) = self.docs.list[i].editor.breakpoints.get_mut(line) {
                        *k = rc_editor::BreakpointKind::Disabled;
                    }
                }
            }
        }
        self.focus_editor = true;
    }

    pub fn session(&self) -> Session {
        let files = self
            .docs
            .list
            .iter()
            .map(|d| {
                let (line, col) = d.editor.caret_line_col();
                let folded = d.editor.syntax.folds().iter().filter(|f| d.editor.is_folded(f.start_line)).map(|f| f.start_line).collect();
                OpenFile { path: d.path.clone(), line, col, scroll_y: d.editor.scroll_offset().y, folded, pinned: d.pinned }
            })
            .collect();
        let breakpoints = self
            .docs
            .list
            .iter()
            .filter(|d| !d.editor.breakpoints.is_empty())
            .map(|d| (d.path.clone(), d.editor.breakpoints.keys().map(|l| (*l, self.debugger.conditions.get(&(d.path.clone(), *l)).cloned())).collect()))
            .collect();
        let (editor_layout, editor_windows) = self.layout.to_session(|id| self.docs.list.iter().position(|d| d.editor.id == id));
        Session {
            files,
            active: self.docs.active,
            editor_layout: Some(editor_layout),
            editor_windows,
            tool_windows: self.dock.to_session(),
            expanded: {
                let mut v: Vec<String> = self.explorer.expanded.iter().cloned().collect();
                v.sort();
                v
            },
            run_configuration: self.runner.selected_config().map(|c| c.name.clone()),
            compounds: self.runner.compounds.clone(),
            run_options: self.runner.options.clone(),
            breakpoints,
            logpoints: group_lines(self.debugger.logs.iter().map(|((p, l), m)| (p.clone(), (*l, m.clone())))),
            remove_once_hit: group_lines(self.debugger.once.iter().map(|(p, l)| (p.clone(), *l))),
            bookmarks: self.docs.bookmarks().into_iter().map(|(path, lines)| (path, lines.into_iter().collect())).collect(),
            disabled_breakpoints: self
                .docs
                .list
                .iter()
                .map(|d| (d.path.clone(), d.editor.breakpoints.iter().filter(|(_, k)| **k == rc_editor::BreakpointKind::Disabled).map(|(l, _)| *l).collect::<Vec<_>>()))
                .filter(|(_, v)| !v.is_empty())
                .collect(),
        }
    }

    pub fn save_session(&mut self) {
        let layout = self.dock.to_session();
        if self.workspace.is_some() && self.settings.tool_windows != layout {
            self.settings.tool_windows = layout;
            let _ = self.settings.save();
        }
        if let Some(w) = &self.workspace
            && let Err(e) = self.session().save(w.ws.key_path())
        {
            eprintln!("could not save the session: {e:#}");
        }
        self.session_dirty = false;
    }

    pub fn save_all_docs(&mut self) {
        let modified: Vec<PathBuf> = self.docs.list.iter().filter(|d| d.is_modified()).map(|d| d.path.clone()).collect();
        for e in self.docs.save_all() {
            self.notify(e, true);
        }
        for p in modified {
            self.runner.test_failures.remove(&p);
            self.insight.did_save(&p);
            if self.settings.format_on_save
                && let Some(i) = self.docs.index_of(&p)
            {
                self.insight.format_saved(&self.docs.list[i]);
            }
        }
    }

    /// Code-insight results that leave the editor: navigation, result lists, dialogs.
    pub fn handle_insight(&mut self, outputs: Vec<InsightOutput>) {
        for o in outputs {
            match o {
                InsightOutput::SaveFormatted => {
                    for e in self.docs.save_all() {
                        self.notify(e, true);
                    }
                }
                InsightOutput::Navigate(t) => self.open_at(&t.path, t.line, t.col),
                InsightOutput::OpenUrl(url) => crate::ui::run_windows::open_url(&url),
                InsightOutput::MacroExpansion { name, text } => self.show_macro_expansion(&name, &text),
                InsightOutput::ShowCallTree => self.dock.show(crate::toolwindow::ToolWindow::Hierarchy),
                InsightOutput::Usages { title, targets } => {
                    if targets.len() == 1 {
                        let t = &targets[0];
                        self.open_at(&t.path.clone(), t.line, t.col);
                    } else {
                        self.find_results = Some((title, targets));
                        self.dock.show(ToolWindow::Find);
                    }
                }
                InsightOutput::Actions(list) => self.dialog = Some(Dialog::CodeActions { list, selected: 0 }),
                InsightOutput::AskRename { doc, pos, name } => self.dialog = Some(Dialog::RenameSymbol { doc, pos, original: name.clone(), name, focus: true }),
                InsightOutput::Message(m) => self.notify(m, false),
            }
        }
    }

    /// Runs a code-insight action from the editor's keymap on the active document.
    pub fn run_insight_action(&mut self, action: Action, caret_screen: egui::Pos2) {
        let Some(i) = self.docs.active else { return };
        // JSON and XML are formatted here; C# by the server.
        if action == Action::Reformat && !crate::insight::has_insight(&self.docs.list[i]) {
            self.format_locally(i);
            return;
        }
        if !self.insight.ready() || !crate::insight::has_insight(&self.docs.list[i]) {
            let msg = match &self.insight.status {
                crate::insight::InsightStatus::NotInstalled => tr("Download the C# language server from the status bar to use this action"),
                crate::insight::InsightStatus::Failed(_) => tr("The C# language server stopped; restart it from the status bar"),
                _ if !crate::insight::has_insight(&self.docs.list[i]) => tr("Language server actions are not available in this file"),
                _ => tr("The Rust language server is still analysing the project"),
            };
            self.notify(msg, false);
            return;
        }
        let doc = &mut self.docs.list[i];
        self.insight.flush(doc);
        let caret = doc.editor.caret();
        match action {
            Action::Complete => self.insight.complete(doc, None),
            Action::ParameterInfo => self.insight.parameter_info(doc),
            Action::QuickDoc => self.insight.quick_doc(doc, caret_screen),
            Action::ExternalDocs => self.insight.external_docs(doc, caret),
            Action::ExpandMacro => self.insight.expand_macro(doc, caret),
            Action::ShowIntentions => self.insight.code_actions(doc),
            Action::Generate => self.insight.generate_actions(doc),
            Action::RefactorThis => self.insight.refactor_actions(doc),
            Action::GotoDeclaration => {
                self.push_nav();
                let doc = &self.docs.list[i];
                self.insight.navigate(doc, caret, NavKind::Declaration);
            }
            Action::GotoImplementation => {
                self.push_nav();
                let doc = &self.docs.list[i];
                self.insight.navigate(doc, caret, NavKind::Implementation);
            }
            Action::GotoTypeDeclaration => {
                self.push_nav();
                let doc = &self.docs.list[i];
                self.insight.navigate(doc, caret, NavKind::TypeDeclaration);
            }
            Action::FindUsages => self.insight.navigate(doc, caret, NavKind::Usages),
            Action::Rename => self.insight.prepare_rename(doc),
            Action::ShowCallers => self.insight.call_hierarchy(doc, caret, true),
            Action::TypeHierarchy => self.insight.type_hierarchy(doc, caret),
            Action::ShowCallees => self.insight.call_hierarchy(doc, caret, false),
            Action::GotoSuperMethod => {
                self.push_nav();
                let doc = &self.docs.list[i];
                self.insight.navigate(doc, caret, NavKind::Super);
            }
            Action::Reformat if self.runner.cargo_workspace.is_some() && !doc.editor.selection.primary().is_empty() => {
                self.notify(tr("rustfmt formats whole files. Clear the selection to reformat this one."), false);
            }
            Action::Reformat => self.insight.format(doc),
            Action::FileStructure => self.dialog = Some(Dialog::FileStructure { filter: String::new(), selected: 0 }),
            _ => {}
        }
    }

    /// Reformat Code for JSON and XML-like files: the document, or the selected lines.
    fn format_locally(&mut self, i: usize) {
        use rc_syntax::Lang;
        let d = &mut self.docs.list[i];
        let lang = d.editor.syntax.lang;
        let unit = d.editor.indent.unit.clone();
        let format = |text: &str| match lang {
            Lang::Json => rc_text::format::json(text, &unit),
            Lang::Xml => rc_text::format::xml(text, &unit),
            _ => None,
        };
        if !matches!(lang, Lang::Json | Lang::Xml) {
            self.notify(tr("Reformat Code is not available for this file"), false);
            return;
        }
        let rope = d.editor.buffer.rope().clone();
        let sel = d.editor.selection.primary();
        let (start, end) = if sel.is_empty() {
            (0, rope.len_chars())
        } else {
            // Whole lines of the selection; XML keeps the depth of the lines above.
            let (l0, l1) = (rope.char_to_line(sel.start()), rope.char_to_line(sel.end().saturating_sub(1).max(sel.start())));
            (rope.line_to_char(l0), rope.line_to_char((l1 + 1).min(rope.len_lines())))
        };
        let text = rope.slice(start..end).to_string();
        let formatted = if sel.is_empty() || lang == Lang::Json {
            format(&text)
        } else {
            // Format from the top so the selection gets its real depth, then take its lines.
            let before = rope.slice(0..start).to_string();
            let n_before = before.lines().count();
            let n_sel = text.lines().count();
            format(&format!("{before}{text}")).map(|all| all.lines().skip(n_before).take(n_sel).map(|l| format!("{l}\n")).collect())
        };
        match formatted {
            Some(f) if f != text => d.editor.apply_text_edits(vec![rc_text::Change::new(start, end, f)]),
            Some(_) => {}
            None => self.notify(tr("The file could not be formatted: check its syntax"), true),
        }
    }

    // ------------------------------------------------------------------ documents

    /// Opens a file in an editor tab, remembering the current place for "Back".
    pub fn open_file(&mut self, path: &Path) {
        self.push_nav();
        match self.docs.open(path) {
            Ok(_) => {
                self.focus_editor = true;
                self.session_dirty = true;
            }
            Err(e) => self.notify(e, true),
        }
    }

    /// Opens `path` and puts the caret at (line, col).
    pub fn open_at(&mut self, path: &Path, line: usize, col: usize) {
        self.open_file(path);
        if let Some(d) = self.docs.active_doc_mut() {
            d.editor.goto(line, col);
        }
    }

    pub fn push_nav(&mut self) {
        if let Some(d) = self.docs.active_doc() {
            self.push_nav_at(d.path.clone(), d.editor.caret());
        }
    }

    /// Records a spot to come back to with Back (⌘[ or the toolbar arrow).
    pub fn push_nav_at(&mut self, path: PathBuf, pos: usize) {
        let here = (path, pos);
        if self.nav_back.last() != Some(&here) {
            self.nav_back.push(here);
            if self.nav_back.len() > 100 {
                self.nav_back.remove(0);
            }
        }
        self.nav_forward.clear();
    }

    pub fn navigate(&mut self, back: bool) {
        let (from, to) = if back { (&mut self.nav_back, &mut self.nav_forward) } else { (&mut self.nav_forward, &mut self.nav_back) };
        let Some((path, pos)) = from.pop() else { return };
        if let Some(d) = self.docs.active_doc() {
            to.push((d.path.clone(), d.editor.caret()));
        }
        if self.docs.open(&path).is_ok()
            && let Some(d) = self.docs.active_doc_mut()
        {
            let pos = pos.min(d.editor.buffer.len_chars());
            d.editor.set_selection(rc_text::Selection::point(pos));
            self.focus_editor = true;
        }
    }

    /// Closes tab `i`, saving it first (IntelliJ autosaves; nothing is ever lost on close).
    /// Shows a macro expansion in a read-only tab, written to the data folder so it has a path.
    pub fn show_macro_expansion(&mut self, name: &str, text: &str) {
        // Only the name's word characters reach the file name.
        let safe: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '_').take(64).collect();
        let path = rc_core::paths::expansions_dir().join(format!("{}.rs", if safe.is_empty() { "macro" } else { &safe }));
        let body = format!("// {}

{text}
", trf("Recursive expansion of {name}!", &[("name", &name)]));
        if let Err(e) = rc_core::paths::write_atomic(&path, body.as_bytes()) {
            self.notify(trf("Could not show the expansion: {error}", &[("error", &e.to_string())]), true);
            return;
        }
        match self.docs.open(&path) {
            Ok(i) => {
                let d = &mut self.docs.list[i];
                d.generated = true;
                // Already open from an earlier expansion: show the new text.
                if d.editor.buffer.rope() != body.as_str() {
                    d.editor.reload(&body);
                    d.editor.parse_blocking();
                }
                d.editor.read_only = true;
                d.mtime = std::fs::metadata(&d.path).and_then(|m| m.modified()).ok();
                self.focus_editor = true;
            }
            Err(e) => self.notify(e, true),
        }
    }

    pub fn close_tab(&mut self, i: usize) {
        if i < self.docs.list.len() && self.docs.list[i].is_modified()
            && let Err(e) = self.docs.save(i)
        {
            self.notify(e, true);
            return;
        }
        // The tab beside it in the same group takes its place.
        let id = self.docs.list.get(i).map(|d| d.editor.id);
        let next = id.filter(|_| self.docs.active == Some(i)).and_then(|id| self.layout.after_close(id));
        self.docs.close(i);
        if let Some(n) = next.and_then(|n| self.docs.list.iter().position(|d| d.editor.id == n)) {
            self.docs.active = Some(n);
        }
        self.sync_layout();
        self.session_dirty = true;
    }

    /// Brings the editor layout in step with the open documents and the active one.
    pub fn sync_layout(&mut self) {
        let docs: Vec<(u64, bool)> = self.docs.list.iter().map(|d| (d.editor.id, d.pinned)).collect();
        let active = self.docs.active_doc().map(|d| d.editor.id);
        self.layout.sync(&docs, active);
    }

    // ------------------------------------------------------------------ actions

    /// Runs an IDE action (global shortcuts, menus, toolbar buttons).
    pub fn run_ide_action(&mut self, ctx: &Context, action: Action) {
        match action {
            Action::SaveAll | Action::Save => {
                self.save_all_docs();
                self.save_session();
            }
            Action::CloseTab => {
                if let Some(i) = self.docs.active {
                    self.close_tab(i);
                }
            }
            Action::NextTab | Action::PreviousTab => {
                // Within the group of the active tab.
                let next = self.docs.active_doc().and_then(|d| self.layout.next_tab(d.editor.id, action == Action::NextTab));
                if let Some(i) = next.and_then(|n| self.docs.list.iter().position(|d| d.editor.id == n)) {
                    self.docs.activate(i);
                    self.focus_editor = true;
                }
            }
            Action::NavigateBack => self.navigate(true),
            Action::NavigateForward => self.navigate(false),
            Action::ShowSettings => self.dialog = Some(Dialog::settings(&self.settings)),
            Action::ToggleProjectWindow => self.toggle_tool(ToolWindow::Explorer),
            Action::ToggleTerminal => self.toggle_tool(ToolWindow::Terminal),
            Action::ToggleGitWindow => self.toggle_tool(ToolWindow::Git),
            Action::ToggleProblems => self.toggle_tool(ToolWindow::Problems),
            Action::HideToolWindows => {
                self.dock.hide_all();
                self.focus_editor = true;
                self.session_dirty = true;
            }
            Action::OpenFolder => self.pick_and_open(),
            Action::Build => {
                self.save_all_docs();
                if let Some(target) = self.workspace.as_ref().map(|w| w.ws.key_path().to_path_buf()) {
                    let outs = self.runner.build(&target, rc_dotnet::BuildAction::Build);
                    crate::ui::run_windows::handle_outputs(self, outs);
                }
            }
            Action::Run => {
                self.save_all_docs();
                let outs = self.runner.run_selected();
                crate::ui::run_windows::handle_outputs(self, outs);
            }
            Action::RunTests => {
                self.save_all_docs();
                if let Some(target) = self.workspace.as_ref().map(|w| w.ws.key_path().to_path_buf()) {
                    let outs = self.runner.run_all_tests(&target);
                    crate::ui::run_windows::handle_outputs(self, outs);
                }
            }
            Action::StopRun => {
                if self.debugger.active() {
                    self.debugger.stop();
                } else {
                    self.runner.stop_all();
                }
            }
            // F5 while paused continues (Visual Studio).
            Action::Debug if self.debugger.stopped() => {
                if let Some(s) = &mut self.debugger.session {
                    s.continue_();
                }
            }
            Action::Debug => self.start_debugging(),
            Action::Resume | Action::StepOver | Action::StepInto | Action::StepOut => {
                if let Some(s) = self.debugger.session.as_mut().filter(|s| s.state().is_stopped()) {
                    match action {
                        Action::Resume => s.continue_(),
                        Action::StepOver => s.step_over(),
                        Action::StepInto => s.step_in(),
                        _ => s.step_out(),
                    }
                }
            }
            Action::AttachToProcess => {
                if self.runner.cargo_workspace.is_some() {
                    self.dialog = Some(crate::ui::dialogs::Dialog::Attach { filter: String::new(), processes: rc_core::processes::list() });
                } else {
                    self.notify(tr("Attaching needs an open Rust project"), false);
                }
            }
            Action::RunToCursor => {
                if let Some((path, line)) = self.docs.active_doc().map(|d| (d.path.clone(), d.editor.caret_line_col().0)) {
                    self.debugger.run_to_cursor(&path, line);
                }
            }
            Action::Evaluate => {
                if self.debugger.stopped() {
                    let text = self.docs.active_doc().map(|d| d.editor.selected_text()).filter(|t| !t.contains('\n')).unwrap_or_default();
                    if !text.is_empty() {
                        self.debugger.evaluate(text.clone());
                    }
                    self.dialog = Some(Dialog::Evaluate { expression: text, focus: true });
                }
            }
            Action::Commit => self.dock.show(ToolWindow::Commit),
            Action::OptimizeImports => {
                let mut out = Vec::new();
                self.insight.optimize_imports(&mut self.docs, &mut out);
                self.handle_insight(out);
            }
            Action::ShowBookmarks => self.dialog = Some(Dialog::Bookmarks { list: self.bookmark_list(), selected: 0 }),
            Action::RunAnything => self.dialog = Some(Dialog::RunAnything { text: String::new(), selected: 0 }),
            Action::SearchEverywhere | Action::FindAction | Action::GotoFile | Action::GotoClass | Action::GotoSymbol => {
                if self.workspace.is_some() {
                    let tab = match action {
                        Action::GotoFile => crate::search::SeTab::Files,
                        Action::GotoClass => crate::search::SeTab::Types,
                        Action::GotoSymbol => crate::search::SeTab::Symbols,
                        Action::FindAction => crate::search::SeTab::Actions,
                        _ => crate::search::SeTab::All,
                    };
                    // The selected text (or the word at the caret) pre-fills the search.
                    let text = self.docs.active_doc().map(|d| d.editor.selected_text()).filter(|t| !t.contains('\n')).unwrap_or_default();
                    self.search_everywhere = Some(crate::search::SearchEverywhere::new(tab, text));
                }
            }
            Action::FindInFiles | Action::ReplaceInFiles => {
                if let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) {
                    let text = self.docs.active_doc().map(|d| d.editor.selected_text()).filter(|t| !t.is_empty() && !t.contains('\n'));
                    let fif = self.find_in_files.get_or_insert_with(|| crate::search::FindInFiles::new(&root, self.waker.clone(), String::new()));
                    if let Some(t) = text {
                        fif.query = t;
                    }
                    fif.show_replace = action == Action::ReplaceInFiles;
                    fif.focus = true;
                    fif.search();
                    self.find_results = None;
                    self.dock.show(ToolWindow::Find);
                }
            }
            Action::ZoomIn => self.set_zoom(ctx, self.settings.ui_zoom + 0.1),
            Action::ZoomOut => self.set_zoom(ctx, self.settings.ui_zoom - 0.1),
            Action::ZoomReset => self.set_zoom(ctx, 1.0),
            Action::SelectIn if self.docs.active_doc().is_some() => {
                let row = |title: &str, command: &str| {
                    rc_lsp::lsp_types::CodeActionOrCommand::Command(rc_lsp::lsp_types::Command { title: title.to_string(), command: command.to_string(), arguments: None })
                };
                let reveal = if cfg!(target_os = "macos") { tr("Reveal in Finder") } else { tr("Open in Explorer") };
                let list = vec![row(tr("Select in Explorer"), crate::insight::SELECT_IN_EXPLORER), row(reveal, crate::insight::REVEAL_FILE)];
                self.dialog = Some(Dialog::CodeActions { list, selected: 0 });
            }
            Action::NewScratchFile => self.dialog = Some(Dialog::Scratches { selected: 0, filter: String::new(), files: rc_core::scratch::list(&rc_core::paths::scratches_dir()) }),
            Action::RecentFiles => self.dialog = Some(Dialog::RecentFiles { selected: 1.min(self.docs.recent.len().saturating_sub(1)), filter: String::new() }),
            Action::RecentLocations => {
                if !self.nav_back.is_empty() {
                    self.dialog = Some(Dialog::RecentLocations { selected: 0 });
                }
            }
            Action::GotoLine => {
                if let Some(d) = self.docs.active_doc() {
                    let (l, c) = d.editor.caret_line_col();
                    self.dialog = Some(Dialog::GotoLine { text: format!("{}:{}", l + 1, c + 1), focus: true });
                }
            }
            Action::Escape => {
                // Esc in a tool window returns to the editor (IntelliJ).
                self.focus_editor = true;
            }
            _ => {}
        }
    }

    pub fn handle_debug(&mut self, outs: Vec<crate::debugger::DebugOutput>) {
        use crate::debugger::DebugOutput;
        for o in outs {
            match o {
                DebugOutput::Notify(m, err) => self.notify(m, err),
                DebugOutput::BreakpointsChanged => self.session_dirty = true,
                DebugOutput::ShowDebugWindow => self.dock.show(ToolWindow::Debug),
                DebugOutput::ShowLine(path, line) => {
                    if self.docs.active_doc().is_none_or(|d| d.path != path) || self.docs.active_doc().is_some_and(|d| d.editor.caret_line_col().0 != line) {
                        self.open_at(&path, line, 0);
                    }
                }
            }
        }
    }

    /// Build errors and warnings go to the Problems window next to the server's diagnostics.
    pub fn set_build_problems(&mut self, list: Vec<rc_dotnet::Diagnostic>) {
        let mut per_file: std::collections::BTreeMap<PathBuf, Vec<rc_editor::Diagnostic>> = std::collections::BTreeMap::new();
        for d in list {
            let Some(file) = d.file.clone() else { continue };
            let text = match self.docs.index_of(&file) {
                Some(i) => self.docs.list[i].editor.buffer.rope().clone(),
                None => rc_text::Rope::from_str(&std::fs::read_to_string(&file).unwrap_or_default().replace("\r\n", "\n")),
            };
            let line = d.line.unwrap_or(1).saturating_sub(1) as usize;
            let line = line.min(text.len_lines().saturating_sub(1));
            let start = text.line_to_char(line) + d.column.unwrap_or(1).saturating_sub(1) as usize;
            let start = start.min(text.len_chars());
            if self.docs.index_of(&file).is_none() {
                self.insight.problem_text.insert(file.clone(), text);
            }
            per_file.entry(file).or_default().push(rc_editor::Diagnostic {
                start,
                end: start,
                severity: if d.severity == rc_dotnet::Severity::Error { rc_editor::Severity::Error } else { rc_editor::Severity::Warning },
                message: d.message,
                code: (!d.code.is_empty()).then_some(d.code),
                unnecessary: false,
                deprecated: false,
            });
        }
        for (file, diags) in per_file {
            self.insight.problems.insert(file, diags);
        }
    }

    /// Writes `mod name;` in `parent`: in its editor when it is open, so unsaved text stays.
    pub fn declare_module(&mut self, parent: &Path, name: &str, public: bool) {
        use rc_cargo::modules::declaration;
        if let Some(i) = self.docs.index_of(parent) {
            let editor = &mut self.docs.list[i].editor;
            let text = editor.buffer.rope().to_string();
            if let Some((at, line)) = declaration(&text, name, public) {
                let at = text[..at].chars().count();
                editor.replace(at, at, &line);
            }
            return;
        }
        let written = std::fs::read_to_string(parent).and_then(|mut text| match declaration(&text, name, public) {
            Some((at, line)) => {
                text.insert_str(at, &line);
                std::fs::write(parent, text)
            }
            None => Ok(()),
        });
        if let Err(e) = written {
            self.notify(trf("Could not declare the module: {error}", &[("error", &e)]), true);
        }
    }

    /// Applies the compiler's suggestion to its file, if that is still the text the compiler read.
    pub fn apply_build_fix(&mut self, file: &Path, fix: &rc_dotnet::diagnostics::Fix) {
        let stale = tr("The file changed after the build: build again to apply this fix");
        let source = std::fs::read(file).unwrap_or_default();
        let same = rc_dotnet::diagnostics::source_hash(&source) == fix.source;
        let open_changed = self.docs.index_of(file).is_some_and(|i| self.docs.list[i].is_modified());
        let changes = String::from_utf8(source).ok().and_then(|text| fix_changes(&text, fix));
        let (true, false, Some(changes)) = (same, open_changed, changes) else {
            self.notify(stale, true);
            return;
        };
        self.open_file(file);
        if let Some(doc) = self.docs.active_doc_mut() {
            doc.editor.apply_text_edits(changes);
        }
        if let Some(b) = &mut self.runner.build {
            for d in b.diagnostics.iter_mut().filter(|d| d.fix.as_ref() == Some(fix)) {
                d.fix = None;
            }
        }
    }

    /// Opens the declaration of a test from its fully qualified name (Tests window).
    pub fn goto_test(&mut self, name: &str) {
        if self.runner.cargo_workspace.is_some() {
            self.goto_rust_test(name);
            return;
        }
        let parts: Vec<&str> = name.rsplitn(3, '.').collect();
        let (method, class) = (parts.first().copied().unwrap_or(""), parts.get(1).copied().unwrap_or(""));
        let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) else { return };
        // The class's file: `Class.cs` under the workspace, else any file declaring it.
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for n in rc_core::workspace::list_dir(&dir) {
                if n.is_dir {
                    stack.push(n.path);
                } else if n.name.ends_with(".cs")
                    && let Ok(text) = std::fs::read_to_string(&n.path)
                    && text.contains(&format!("class {class}"))
                    && let Some(line) = text.lines().position(|l| l.contains(&format!(" {method}(")))
                {
                    self.open_at(&n.path, line, 0);
                    return;
                }
            }
        }
    }

    /// Opens the Rust test `module::name`: the failure's place when it panicked in the project,
    /// else the first `fn name` in a file that fits the module.
    fn goto_rust_test(&mut self, name: &str) {
        let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) else { return };
        let panicked = self.runner.tests.as_ref().and_then(|t| t.results.get(name)).and_then(|r| {
            r.iter().flat_map(|x| &x.stack_trace).find_map(|f| Some((f.file.clone().filter(|p| p.is_file() && p.starts_with(&root))?, f.line?)))
        });
        if let Some((file, line)) = panicked {
            self.open_at(&file, line.saturating_sub(1) as usize, 0);
            return;
        }
        let (module, method) = name.rsplit_once("::").unwrap_or(("", name));
        let last_module = module.rsplit("::").next().unwrap_or("");
        let declared = format!("fn {method}(");
        let mut found: Option<(PathBuf, usize)> = None;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for n in rc_core::workspace::list_dir(&dir) {
                if n.is_dir {
                    stack.push(n.path);
                } else if n.name.ends_with(".rs")
                    && let Ok(text) = std::fs::read_to_string(&n.path)
                    && let Some(line) = text.lines().position(|l| l.contains(&declared))
                {
                    let fits = !last_module.is_empty() && (text.contains(&format!("mod {last_module}")) || n.path.file_stem().is_some_and(|s| s == last_module));
                    if fits {
                        self.open_at(&n.path, line, 0);
                        return;
                    }
                    found.get_or_insert((n.path, line));
                }
            }
        }
        if let Some((file, line)) = found {
            self.open_at(&file, line, 0);
        }
    }

    /// A new terminal tab in `dir` (the workspace folder by default).
    pub fn open_terminal(&mut self, dir: Option<PathBuf>) {
        let cwd = dir.or_else(|| self.workspace.as_ref().map(|w| w.ws.root.clone()));
        match self.terminals.open(&rc_term::SpawnSpec::shell(cwd), self.waker.clone()) {
            Ok(_) => {
                self.dock.show(ToolWindow::Terminal);
                self.focus_terminal = true;
            }
            Err(e) => self.notify(trf("Could not start the terminal: {error}", &[("error", &e)]), true),
        }
    }

    /// Run icons next to test classes and methods, with their last outcome.
    pub(crate) fn update_test_markers(&mut self) {
        let Some(i) = self.docs.active else { return };
        let doc = &mut self.docs.list[i];
        if doc.editor.lang() == rc_syntax::Lang::Rust {
            // Line numbers of the last run mean nothing once the text changes.
            let notes = self.runner.test_failures.get(&doc.path).filter(|_| !doc.is_modified());
            if notes.map_or(!doc.editor.failure_notes.is_empty(), |n| *n != doc.editor.failure_notes) {
                doc.editor.failure_notes = notes.cloned().unwrap_or_default();
            }
            self.update_rust_markers(i);
            return;
        }
        if doc.editor.lang() != rc_syntax::Lang::CSharp {
            return;
        }
        let Some(tree) = doc.editor.syntax.tree() else { return };
        let markers = crate::runner::test_markers(doc.editor.buffer.rope(), tree);
        doc.editor.gutter_icons.retain(|_, icon| !matches!(icon, rc_editor::GutterIcon::RunTest | rc_editor::GutterIcon::TestPassed | rc_editor::GutterIcon::TestFailed));
        for m in &markers {
            let icon = match self.runner.test_outcomes.get(&m.name) {
                Some(crate::runner::TestState::Passed) => rc_editor::GutterIcon::TestPassed,
                Some(crate::runner::TestState::Failed) => rc_editor::GutterIcon::TestFailed,
                _ => rc_editor::GutterIcon::RunTest,
            };
            doc.editor.gutter_icons.insert(m.line, icon);
        }
        self.test_markers = markers;
    }

    /// Run icons of a Rust document, from the server. An older reply is left alone: the editor
    /// keeps the icons on their lines while the text changes.
    fn update_rust_markers(&mut self, i: usize) {
        use rc_editor::GutterIcon;
        use rc_lsp::rust_analyzer::RunnableKind;
        let doc = &mut self.docs.list[i];
        let Some((version, runnables)) = self.insight.runnables.get(&doc.editor.id) else { return };
        if *version != doc.editor.buffer.version() {
            return;
        }
        doc.editor.gutter_icons.retain(|_, icon| !matches!(icon, GutterIcon::RunTest | GutterIcon::TestPassed | GutterIcon::TestFailed | GutterIcon::RunApp));
        self.test_markers.clear();
        self.run_markers.clear();
        self.bench_markers.clear();
        for r in runnables {
            let state = match &r.kind {
                RunnableKind::Binary(name) => {
                    doc.editor.gutter_icons.insert(r.line, GutterIcon::RunApp);
                    self.run_markers.insert(r.line, name.clone());
                    continue;
                }
                RunnableKind::Bench(args) => {
                    doc.editor.gutter_icons.insert(r.line, GutterIcon::RunApp);
                    self.bench_markers.insert(r.line, args.clone());
                    continue;
                }
                RunnableKind::Test(name) => {
                    self.test_markers.push(crate::runner::TestMarker { line: r.line, name: name.clone(), is_class: false });
                    self.runner.test_outcomes.get(name).copied()
                }
                RunnableKind::Module(name) => {
                    self.test_markers.push(crate::runner::TestMarker { line: r.line, name: name.clone(), is_class: true });
                    module_state(&self.runner.test_outcomes, name)
                }
            };
            let icon = match state {
                Some(crate::runner::TestState::Passed) => GutterIcon::TestPassed,
                Some(crate::runner::TestState::Failed) => GutterIcon::TestFailed,
                _ => GutterIcon::RunTest,
            };
            doc.editor.gutter_icons.insert(r.line, icon);
        }
    }

    /// Click on the run icon of a `fn main`: selects its configuration and runs it.
    pub fn run_binary_at(&mut self, ctx: &Context, line: usize) {
        if let Some(args) = self.bench_markers.get(&line).cloned() {
            self.run_cargo_args(args);
            return;
        }
        let Some(name) = self.run_markers.get(&line) else { return };
        let Some(index) = self.runner.configs.iter().position(|c| c.name == *name) else {
            self.notify(tr("This file is not part of a runnable target"), false);
            return;
        };
        self.runner.selected = index;
        self.session_dirty = true;
        self.run_ide_action(ctx, Action::Run);
    }

    /// Starts the selected configuration under the debugger.
    fn start_debugging(&mut self) {
        self.debug_test = None;
        self.save_all_docs();
        match self.runner.selected_config().cloned() {
            Some(config) if matches!(config.kind, crate::runner::ConfigKind::Cargo { .. }) => match self.runner.cargo_debug_target(&config) {
                Some(Ok((cargo, target))) => {
                    let outs = self.debugger.start_cargo(config, cargo, target, &self.docs);
                    self.handle_debug(outs);
                }
                Some(Err(e)) => self.notify(e, true),
                None => {}
            },
            Some(config) => {
                let dotnet = dotnet_path(&self.settings);
                let request = self.runner.request_for(&config);
                let outs = self.debugger.start(config, request, &dotnet, self.settings.netcoredbg_path.as_deref(), &self.docs);
                self.handle_debug(outs);
            }
            None => self.notify(tr("There is no run configuration to debug"), false),
        }
    }

    /// The debugger's Restart: what is being debugged, from the beginning.
    pub fn restart_debugging(&mut self) {
        self.debugger.stop();
        match self.debug_test.clone() {
            Some((config, target)) => self.debug_rust_test(config, target),
            None => self.start_debugging(),
        }
    }

    fn debug_rust_test(&mut self, config: crate::runner::RunConfig, target: rc_cargo::RunTarget) {
        let cargo = match self.runner.cargo_tool() {
            Ok(cargo) => cargo,
            Err(e) => {
                self.notify(e, true);
                return;
            }
        };
        self.save_all_docs();
        self.debug_test = Some((config.clone(), target.clone()));
        let outs = self.debugger.start_cargo(config, cargo, target, &self.docs);
        self.handle_debug(outs);
    }

    /// Debugs what the icon of `line` runs, in the active Rust document.
    pub fn debug_runnable_at(&mut self, ctx: &Context, line: usize) {
        use rc_lsp::rust_analyzer::RunnableKind;
        let Some(id) = self.docs.active_doc().map(|d| d.editor.id) else { return };
        let Some(runnable) = self.insight.runnables.get(&id).and_then(|(_, list)| list.iter().find(|r| r.line == line)).cloned() else { return };
        let (name, mut args) = match &runnable.kind {
            RunnableKind::Binary(name) => {
                let Some(index) = self.runner.configs.iter().position(|c| c.name == *name) else {
                    self.notify(tr("This file is not part of a runnable target"), false);
                    return;
                };
                self.runner.selected = index;
                self.session_dirty = true;
                self.run_ide_action(ctx, Action::Debug);
                return;
            }
            RunnableKind::Bench(_) => {
                self.notify(tr("Benchmarks cannot be debugged"), false);
                return;
            }
            RunnableKind::Test(name) => (name.clone(), vec![name.clone(), "--exact".to_string()]),
            RunnableKind::Module(name) => (name.clone(), vec![format!("{name}::")]),
        };
        // One thread, so stepping stays in the test being debugged.
        args.extend(["--nocapture".to_string(), "--test-threads=1".to_string()]);
        let Some(manifest) = self.runner.cargo_workspace.as_ref().map(|w| w.manifest.clone()) else { return };
        let target = rc_cargo::RunTarget { manifest, package: String::new(), bin: name.clone(), args, env: Vec::new(), working_dir: None, test: Some(runnable.cargo_args), options: Default::default() };
        let config = crate::runner::RunConfig { name: name.clone(), kind: crate::runner::ConfigKind::Cargo { package: String::new(), bin: name } };
        self.debug_rust_test(config, target);
    }

    /// Click on a test's gutter icon: runs that test (or class) in its project.
    pub fn run_test_at(&mut self, line: usize) {
        let Some(m) = self.test_markers.iter().find(|m| m.line == line).cloned() else { return };
        let Some(path) = self.docs.active_doc().map(|d| d.path.clone()) else { return };
        let manifest = self.runner.cargo_workspace.as_ref().map(|w| w.manifest.clone());
        let Some(project) = manifest.or_else(|| crate::runner::project_of(&path, &self.runner.configs)) else {
            self.notify(tr("This file is not part of a test project"), false);
            return;
        };
        self.save_all_docs();
        let filter = if m.is_class { rc_dotnet::TestFilter::Class(m.name.clone()) } else { rc_dotnet::TestFilter::Tests(vec![m.name.clone()]) };
        let outs = self.runner.run_tests(&project, Some(filter));
        crate::ui::run_windows::handle_outputs(self, outs);
    }

    pub fn toggle_tool(&mut self, tw: ToolWindow) {
        let was_open = self.dock.is_open(tw);
        self.dock.toggle(tw);
        if was_open {
            self.focus_editor = true;
        }
        self.session_dirty = true;
    }

    fn set_zoom(&mut self, ctx: &Context, z: f32) {
        self.settings.ui_zoom = (z * 10.0).round().clamp(5.0, 30.0) / 10.0;
        let _ = self.settings.save();
        ctx.request_repaint();
    }

    /// Opens a folder, solution or project as the workspace, and any other file in an editor
    /// (without a workspace it is edited on its own, like Rider's LightEdit).
    fn opens_workspace(&self, path: &Path) -> bool {
        use rc_core::workspace::{is_project, is_solution};
        path.is_dir() || is_solution(path) || is_project(path)
    }

    /// The project a `Cargo.toml` opens. Inside the open project it is just a file to edit.
    fn project_of_manifest(&self, path: &Path) -> Option<PathBuf> {
        let inside = self.workspace.as_ref().is_some_and(|w| path.starts_with(&w.ws.root));
        (!inside && path.is_file() && path.file_name().is_some_and(|n| n == "Cargo.toml")).then(|| path.parent().map(Path::to_path_buf)).flatten()
    }

    pub fn open_path(&mut self, path: &Path) {
        if let Some(project) = self.project_of_manifest(path) {
            self.open_workspace(&project);
        } else if self.opens_workspace(path) {
            self.open_workspace(path);
        } else if path.is_file() {
            self.open_file(path);
        } else {
            self.notify(trf("Could not open {path}: {error}", &[("path", &path.display()), ("error", &tr("the file does not exist"))]), true);
        }
    }

    pub fn pick_file(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title(tr("Open File"));
        if let Some(dir) = self.docs.active_doc().and_then(|d| d.path.parent().map(Path::to_path_buf)).or_else(|| self.workspace.as_ref().map(|w| w.ws.root.clone())) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(paths) = dialog.pick_files() {
            for p in paths {
                self.open_path(&p);
            }
        }
    }

    /// File › New File: asks for a name in the active file's folder (or the solution's), or for a
    /// path when nothing is open.
    pub fn new_file_dialog(&mut self) {
        let root = self.workspace.as_ref().map(|w| w.ws.root.clone());
        let active = self.docs.active_doc().and_then(|d| d.path.parent().map(Path::to_path_buf));
        if let Some(root) = root {
            let dir = active.filter(|d| d.starts_with(&root)).unwrap_or(root);
            let kind = if self.runner.cargo_workspace.is_some() { crate::item_templates::ItemKind::RustFile } else { crate::item_templates::ItemKind::File };
            self.dialog = Some(Dialog::NewFile { dir, name: String::new(), kind, attach: true, public: false, error: None });
            return;
        }
        let mut dialog = rfd::FileDialog::new().set_title(tr("New File"));
        if let Some(dir) = active {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.save_file() {
            if !path.exists()
                && let Err(e) = std::fs::write(&path, "")
            {
                self.notify(trf("Could not create {path}: {error}", &[("path", &path.display()), ("error", &e)]), true);
                return;
            }
            self.open_path(&path);
        }
    }

    /// Open…: a folder, solution or project opens as the workspace, any other file in the
    /// editor (macOS lets one panel pick both; elsewhere it picks a folder).
    pub fn pick_and_open(&mut self) {
        let dialog = rfd::FileDialog::new().set_title(tr("Open"));
        #[cfg(target_os = "macos")]
        let picked = dialog.pick_files_or_folders();
        #[cfg(not(target_os = "macos"))]
        let picked = dialog.set_title(tr("Open Folder or Project")).pick_folder().map(|p| vec![p]);
        if let Some(paths) = picked {
            self.open_paths(paths);
        }
    }

    /// Opens the first folder, solution or project among `paths` as the workspace and the other
    /// files in the editor (dropped files, Open…).
    pub fn open_paths(&mut self, paths: Vec<PathBuf>) {
        let (workspaces, files): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| self.opens_workspace(p) || self.project_of_manifest(p).is_some());
        if let Some(ws) = workspaces.first() {
            self.open_path(ws);
        }
        for f in files {
            self.open_path(&f);
        }
    }

    pub fn pick_solution(&mut self) {
        let dialog = rfd::FileDialog::new().set_title(tr("Open Cargo.toml…")).add_filter("Cargo.toml", &["toml"]);
        if let Some(path) = dialog.pick_file() {
            self.open_workspace(&path);
        }
    }

    pub fn apply_settings(&mut self, ctx: &Context, new: Settings) {
        let lang_changed = new.language != self.settings.language;
        let keymap_changed = new.keymap != self.settings.keymap;
        self.settings = new;
        self.runner.nextest = self.settings.use_nextest;
        self.editor_config = editor_config(&self.settings);
        if keymap_changed {
            self.keymap = Keymap::new(keymap_kind(self.settings.keymap));
        }
        if lang_changed {
            i18n::set_lang(self.settings.language.as_deref().map(Lang::from_locale).unwrap_or_else(Lang::system));
            if let Some(m) = &self.native_menu {
                m.relabel();
            }
        }
        if let Err(e) = self.settings.save() {
            self.notify(trf("Could not save the settings: {error}", &[("error", &e)]), true);
        }
        self.applied_dark = None;
        ctx.request_repaint();
    }

    // ------------------------------------------------------------------ explorer actions

    pub fn handle_explorer(&mut self, ctx: &Context, actions: Vec<ExplorerAction>) {
        for a in actions {
            match a {
                ExplorerAction::Open(p) => self.open_file(&p),
                ExplorerAction::Preview(p) => {
                    self.push_nav();
                    match self.docs.open_preview(&p) {
                        Ok(_) => self.session_dirty = true,
                        Err(e) => self.notify(e, true),
                    }
                }
                ExplorerAction::Collapse => {
                    self.explorer.expanded.retain(|k| k == "sln");
                    self.session_dirty = true;
                }
                ExplorerAction::LocateActive => self.locate_active(),
                ExplorerAction::NewCrate(dir) => self.new_project = Some(crate::ui::new_project::NewProjectDialog::member(&dir, self.waker.clone())),
                ExplorerAction::NewItem(dir, kind) => self.dialog = Some(Dialog::NewFile { dir, name: String::new(), kind, attach: true, public: false, error: None }),
                ExplorerAction::Rename(path) => {
                    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    self.dialog = Some(Dialog::Rename { path, name, error: None });
                }
                ExplorerAction::Delete(path) => self.dialog = Some(Dialog::ConfirmDelete { path }),
                ExplorerAction::CopyPath(path) => ctx.copy_text(path.display().to_string()),
                ExplorerAction::Reveal(path) => reveal_in_file_manager(&path),
                ExplorerAction::OpenInTerminal(dir) => self.open_terminal(Some(dir)),
                ExplorerAction::AddProject(sln) => self.new_project_dialog(Some(sln)),
                ExplorerAction::ManagePackages(project) => {
                    self.nuget.focus_project(&project);
                    self.dock.show(ToolWindow::Nuget);
                }
            }
        }
    }

    pub fn locate_active(&mut self) {
        let Some(path) = self.docs.active_doc().map(|d| d.path.clone()) else { return };
        if let Some(w) = &self.workspace {
            self.explorer.locate(&path, w.solution.as_ref(), &w.ws.root);
            self.dock.show(ToolWindow::Explorer);
        }
    }

    // ------------------------------------------------------------------ frame

    fn poll_background(&mut self) {
        self.poll_cargo_workspace();
        let mut changed: Vec<PathBuf> = Vec::new();
        let events = self.fs_inbox.drain();
        self.report_fs_to_server(&events);
        if let Some(index) = &self.files.index
            && !events.is_empty()
        {
            let outside_git: Vec<FsEvent> = events.iter().filter(|e| !e.path().components().any(|c| c.as_os_str() == ".git")).cloned().collect();
            index.apply(&outside_git);
        }
        for e in &events {
            self.vcs.on_fs_change(e.path());
        }
        for ev in events {
            let p = ev.path().to_path_buf();
            self.explorer.invalidate(&p);
            if !changed.contains(&p) {
                changed.push(p);
            }
        }
        for p in changed {
            self.docs.on_disk_change(&p);
            // A changed solution file reloads the explorer's model.
            if let Some(w) = &mut self.workspace
                && w.ws.solution() == Some(p.as_path())
                && let Ok(s) = Solution::load(&p)
            {
                // Projects added or removed by hand: new run configurations too.
                let remembered = self.runner.selected_config().map(|c| c.name.clone());
                self.runner.set_workspace(Some(&s), &[], &dotnet_path(&self.settings), remembered.as_deref());
                w.solution = Some(s);
            }
        }
        self.docs.poll_parses();
    }

    /// Changed source files tell the language server to reload them (1 created, 2 changed,
    /// 3 deleted), and a changed `Cargo.toml` reloads the Rust workspace.
    pub(crate) fn report_fs_to_server(&mut self, events: &[FsEvent]) {
        if self.runner.cargo_workspace.is_some() && events.iter().any(|e| e.path().file_name().is_some_and(|n| n == "Cargo.lock")) {
            self.load_libraries();
        }
        if let Some(manifest) = self.workspace.as_ref().and_then(|w| w.ws.cargo_manifest()).map(Path::to_path_buf)
            && events.iter().any(|e| e.path().file_name().is_some_and(|n| n == "Cargo.toml"))
        {
            self.load_cargo_workspace(manifest);
        }
        let changes: Vec<(PathBuf, u8)> = events
            .iter()
            .filter(|e| e.path().extension().is_some_and(|x| x == "rs" || x == "toml" || x == "cs" || x == "csproj" || x == "props" || x == "targets"))
            .map(|e| match e {
                FsEvent::Created(p) => (p.clone(), 1),
                FsEvent::Changed(p) => (p.clone(), 2),
                FsEvent::Removed(p) => (p.clone(), 3),
            })
            .collect();
        if !changes.is_empty() {
            self.insight.files_changed(&changes);
        }
    }

    /// IDE shortcuts work whatever has the focus; they are taken out of the input so text
    /// fields don't see them.
    fn global_shortcuts(&mut self, ctx: &Context) {
        let now = ctx.input(|i| i.time);
        // Shift, Shift: Search Everywhere.
        let shift = ctx.input(|i| i.modifiers.shift_only());
        // Ctrl, Ctrl: Run Anything. Ctrl also goes with clicks and the wheel.
        let ctrl = ctx.input(|i| i.modifiers.matches_exact(egui::Modifiers::CTRL));
        let any_key = ctx.input(|i| i.events.iter().any(interrupts_tap));
        let any_input = any_key || ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::PointerButton { .. } | egui::Event::MouseWheel { .. })));
        if double_tap(&mut self.shift_taps, shift, any_key, now) {
            self.pending.push(Action::SearchEverywhere);
        }
        if double_tap(&mut self.ctrl_taps, ctrl, any_input, now) {
            self.pending.push(Action::RunAnything);
        }
        let keymap = &self.keymap;
        let mut found = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = e
                    && let Some(a) = keymap.lookup(*key, *modifiers)
                    && a.is_ide()
                {
                    found.push(a);
                    return false;
                }
                true
            })
        });
        self.pending.extend(found);
        // The mouse's back / forward buttons.
        let (back, forward) = ctx.input(|i| (i.pointer.button_pressed(egui::PointerButton::Extra1), i.pointer.button_pressed(egui::PointerButton::Extra2)));
        if back {
            self.pending.push(Action::NavigateBack);
        } else if forward {
            self.pending.push(Action::NavigateForward);
        }
    }
}

/// What a start opens: files sent by Finder or the command line, otherwise the project open
/// at the last quit (none after Close Project, as in Rider).
pub fn startup_paths(settings: &Settings, from_system: Vec<PathBuf>) -> Vec<PathBuf> {
    if !from_system.is_empty() {
        return from_system;
    }
    settings.last_workspace.clone().filter(|p| settings.reopen_last && p.exists()).into_iter().collect()
}

impl eframe::App for CrabCraftApp {
    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Window title: active file and workspace (Mission Control, the Window menu).
        let title = match (self.docs.active_doc(), &self.workspace) {
            (Some(d), Some(w)) => format!("{} – {} – CrabCraft", d.name(), w.ws.name()),
            (Some(d), None) => format!("{} – CrabCraft", d.name()),
            (None, Some(w)) => format!("{} – CrabCraft", w.ws.name()),
            (None, None) => "CrabCraft".to_string(),
        };
        if self.window_title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        for path in crate::open_files::take() {
            self.open_path(&path);
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if let Some(menu) = &self.native_menu {
            for cmd in menu.poll() {
                match cmd {
                    MenuCommand::Act(a) => self.pending.push(a),
                    MenuCommand::About => self.dialog = Some(Dialog::About),
                    MenuCommand::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    MenuCommand::OpenProject => self.pick_and_open(),
                    MenuCommand::OpenFile => self.pick_file(),
                    MenuCommand::NewFile => self.new_file_dialog(),
                    MenuCommand::NewSolution => self.new_project_dialog(None),
                    MenuCommand::EditRunConfig => self.edit_run_config(None),
                    MenuCommand::AddUsings => {
                        if let Some(i) = self.docs.active {
                            self.insight.add_missing_usings(&self.docs.list[i]);
                        }
                    }
                    MenuCommand::CloseProject => self.close_project(),
                    MenuCommand::ThemeLight | MenuCommand::ThemeDark => {
                        let mut s = self.settings.clone();
                        s.theme = if cmd == MenuCommand::ThemeDark { ThemeChoice::Dark } else { ThemeChoice::Light };
                        self.apply_settings(ctx, s);
                    }
                    _ => {}
                }
            }
        }
        self.global_shortcuts(ctx);
        self.poll_background();
        let dark = self.dark(ctx);
        if self.applied_dark != Some(dark) {
            rc_kit::theme::apply(ctx, dark);
            self.applied_dark = Some(dark);
        }
        if let Some(m) = &self.native_menu {
            m.sync(dark);
        }
        if self.applied_zoom != Some(self.settings.ui_zoom) {
            ctx.set_zoom_factor(self.settings.ui_zoom);
            self.applied_zoom = Some(self.settings.ui_zoom);
        }
        // Autosave when the window loses focus (IntelliJ's "save on frame deactivation").
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if self.had_focus && !focused && self.settings.autosave {
            self.save_all_docs();
            self.save_session();
        }
        self.had_focus = focused;
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let pending = std::mem::take(&mut self.pending);
        let mut editor_actions = Vec::new();
        for a in pending {
            if a.is_ide() {
                self.run_ide_action(&ctx, a);
            } else {
                editor_actions.push(a);
            }
        }
        let outputs = self.insight.update(&mut self.docs, self.settings.show_inlay_hints);
        self.handle_insight(outputs);
        let outs = self.runner.update();
        crate::ui::run_windows::handle_outputs(self, outs);
        let outs = self.debugger.update(&mut self.docs);
        self.handle_debug(outs);
        for (_, ev) in self.terminals.poll() {
            if let rc_term::SessionEvent::Clipboard(text) = ev {
                ctx.copy_text(text);
            }
        }
        self.update_test_markers();
        let dark = self.dark(&ctx);
        for n in self.vcs.update(&mut self.docs, dark) {
            self.notify(n, false);
        }
        crate::ui::frame(self, ui, editor_actions);
        self.docs.reparse_edited();
        let now = ctx.input(|i| i.time);
        if self.session_dirty && now - self.last_session_save > 2.0 {
            self.save_session();
            self.last_session_save = now;
        }
    }

    fn on_exit(&mut self) {
        self.runner.stop_all();
        self.debugger.stop();
        self.insight.stop();
        self.save_all_docs();
        self.save_session();
    }
}

pub fn keymap_kind(choice: KeymapChoice) -> KeymapKind {
    match choice {
        KeymapChoice::VisualStudio => KeymapKind::VisualStudio,
        KeymapChoice::Rider if cfg!(target_os = "macos") => KeymapKind::RiderMac,
        KeymapChoice::Rider => KeymapKind::Rider,
    }
}

pub fn editor_config(s: &Settings) -> EditorConfig {
    EditorConfig {
        font: rc_editor::layout::EditorFont { size: s.editor_font_size.clamp(8.0, 32.0), line_spacing: s.editor_line_spacing.clamp(1.0, 2.0) },
        show_indent_guides: s.show_indent_guides,
        show_inlays: s.show_inlay_hints,
        sticky_lines: if s.sticky_lines { 5 } else { 0 },
        right_margin: (s.right_margin > 0).then_some(s.right_margin),
        ..EditorConfig::default()
    }
}

/// The configured `dotnet`, or the default install location, or `dotnet` from PATH.
pub fn dotnet_path(s: &Settings) -> PathBuf {
    if let Some(p) = s.dotnet_path.clone().filter(|p| p.exists()) {
        return p;
    }
    let candidates: &[&str] = if cfg!(windows) { &["C:\\Program Files\\dotnet\\dotnet.exe"] } else { &["/usr/local/share/dotnet/dotnet", "/opt/homebrew/bin/dotnet", "/usr/local/bin/dotnet", "/usr/share/dotnet/dotnet"] };
    candidates.iter().map(PathBuf::from).find(|p| p.exists()).unwrap_or_else(|| PathBuf::from("dotnet"))
}

/// Project files under `root` (for folders without a solution), skipping build output.
pub fn find_projects(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, d)) = stack.pop() {
        for n in rc_core::workspace::list_dir(&dir) {
            if n.is_dir && d < depth {
                stack.push((n.path, d + 1));
            } else if rc_core::workspace::is_project(&n.path) {
                out.push(n.path);
            }
        }
    }
    out.sort();
    out
}

pub fn reveal_in_file_manager(path: &Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
}

/// The outcome of a module's tests: failed if any failed, passed once some ran and none failed.
fn module_state(outcomes: &std::collections::BTreeMap<String, crate::runner::TestState>, module: &str) -> Option<crate::runner::TestState> {
    use crate::runner::TestState;
    let prefix = format!("{module}::");
    let mut found = None;
    for (_, state) in outcomes.iter().filter(|(n, _)| n.starts_with(&prefix)) {
        match state {
            TestState::Failed => return Some(TestState::Failed),
            TestState::Passed => found = found.or(Some(TestState::Passed)),
            TestState::Running => found = Some(TestState::Running),
            TestState::Skipped => {}
        }
    }
    found
}

/// Groups per-file entries into the sorted form the session saves.
fn group_lines<T: Ord>(entries: impl Iterator<Item = (PathBuf, T)>) -> std::collections::BTreeMap<PathBuf, Vec<T>> {
    let mut map = std::collections::BTreeMap::<PathBuf, Vec<T>>::new();
    for (path, entry) in entries {
        map.entry(path).or_default().push(entry);
    }
    map.values_mut().for_each(|v| v.sort());
    map
}

#[cfg(test)]
mod double_tap_tests {
    use super::{double_tap, interrupts_tap};

    #[test]
    fn modifiers_do_not_interrupt_their_own_taps() {
        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        assert!(!interrupts_tap(&key(egui::Key::ControlLeft)));
        assert!(!interrupts_tap(&key(egui::Key::ShiftRight)));
        assert!(interrupts_tap(&key(egui::Key::A)));
        assert!(interrupts_tap(&egui::Event::Text("a".into())));
    }

    fn run(steps: &[(bool, bool, f64)]) -> Vec<bool> {
        let mut state = (0.0, 0, false);
        steps.iter().map(|(down, other, now)| double_tap(&mut state, *down, *other, *now)).collect()
    }

    #[test]
    fn two_quick_presses_count_once() {
        let fired = run(&[(true, false, 1.0), (false, false, 1.1), (true, false, 1.2), (true, false, 1.3)]);
        assert_eq!(fired, [false, false, true, false]);
    }

    #[test]
    fn slow_presses_and_chords_do_not_count() {
        assert!(!run(&[(true, false, 1.0), (false, false, 1.1), (true, false, 2.0)]).contains(&true));
        // Ctrl+click, then Ctrl alone.
        assert!(!run(&[(true, false, 1.0), (true, true, 1.05), (false, false, 1.1), (true, false, 1.2)]).contains(&true));
    }
}
