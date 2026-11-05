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
/// The findings of Inspect Code (Clippy over the workspace).
pub struct Inspection {
    pub running: bool,
    pub diagnostics: Vec<rc_dotnet::Diagnostic>,
    /// Why it did not finish (Clippy missing, cancelled).
    pub failed: Option<String>,
}

pub struct Notice {
    pub text: String,
    pub error: bool,
    pub at: f64,
    /// A link the status bar offers to open in the browser.
    pub url: Option<String>,
}

pub struct WorkspaceState {
    pub ws: Workspace,
    pub solution: Option<Solution>,
}

/// What `rustc --explain` answered: its text, or `None` when it has none.
type ExplainAnswer = Result<Option<String>, rc_cargo::ExplainError>;

/// Breakpoints read back per file from the session, at most.
const MAX_BREAKPOINTS_PER_FILE: usize = 10_000;
/// Longest breakpoint group name kept.
pub const MAX_GROUP_CHARS: usize = 80;

/// A breakpoint group name as typed or read from the session: trimmed, not empty, not too long.
pub fn clean_group(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && name.chars().count() <= MAX_GROUP_CHARS).then(|| name.to_string())
}

pub struct CrabCraftApp {
    pub settings: Settings,
    pub keymap: Keymap,
    pub editor_config: EditorConfig,
    pub workspace: Option<WorkspaceState>,
    pub docs: Docs,
    pub disk_revert: Option<crate::disk_revert::Job>,
    pub disk_watch: crate::disk_watch::Watch,
    pub dock: Dock,
    pub explorer: ExplorerState,
    pub dialog: Option<Dialog>,
    pub notices: Vec<Notice>,
    pub insight: Insight,
    pub runner: Runner,
    pub debugger: crate::debugger::Debugger,
    pub debug_sessions: crate::debug_sessions::Sessions,
    window_title: String,
    pub files: crate::search::Files,
    pub vcs: crate::vcs::Vcs,
    pub nuget: crate::ui::nuget::NugetManager,
    /// What is typed in the Cargo window.
    pub cargo_input: String,
    pub hosting: crate::hosting::Hosting,
    pub issue_tracker: crate::issue_tracker::Tracker,
    pub history_edit: crate::history_edit::HistoryEdit,
    pub shelves: crate::shelves::Shelves,
    pub database: crate::database::Database,
    pub endpoints: crate::endpoints::Endpoints,
    pub clone_project: crate::clone_project::CloneProject,
    pub borrow_errors: crate::borrow_errors::BorrowErrors,
    pub missing_crates: crate::missing_crates::MissingCrates,
    pub cargo_dependencies: crate::cargo_dependencies::CargoDependencies,
    /// Members folded in the Cargo window.
    pub cargo_folded: std::collections::BTreeSet<String>,
    /// Dependency trees of the Cargo window, by `(manifest, package)`.
    pub cargo_trees: std::collections::BTreeMap<(PathBuf, String), crate::ui::cargo_window::DepTree>,
    /// What `rustc --print cfg` printed for the project, and its load.
    rustc_cfg: Option<String>,
    rustc_cfg_load: Option<Inbox<Option<String>>>,
    /// Bumped when the Cargo model or the rustc cfg change; each document remembers the epoch
    /// its `cfg` set was given for.
    pub(crate) cfg_epoch: u64,
    cfg_given: std::collections::HashMap<u64, u64>,
    /// Cargo features the user turned on, as `package/feature` (saved with the session).
    pub cargo_features: Vec<String>,
    /// The `(text version, cfg_epoch)` each `Cargo.toml` editor got its feature icons for.
    feature_icons_given: std::collections::HashMap<u64, (u64, u64)>,
    /// The target triple the code is analysed for; `None`: the host (saved with the session).
    pub cargo_target: Option<String>,
    /// The host triple and the targets offered in the status bar, once read.
    pub host_triple: Option<String>,
    pub triple_choices: Vec<rc_cargo::TripleChoice>,
    triple_load: Option<Inbox<(Option<String>, Vec<rc_cargo::TripleChoice>)>>,
    /// What is typed in the filter of the target list.
    pub triple_filter: String,
    /// Locked and newest versions of the dependencies of an open `Cargo.toml`, with their
    /// docs.rs icons.
    pub dep_versions: crate::dep_versions::DepVersions,
    /// Offline inspections of the open `Cargo.toml` (I-22).
    pub manifest_inspections: crate::manifest_inspect::ManifestInspections,
    /// The diff of the highlighted Alt+Enter action (I-33).
    pub intention_preview: crate::intention_preview::IntentionPreview,
    /// The project's inspection profile and which profile it uses (I-18).
    pub inspections: crate::inspection_profile::Profiles,
    /// The scan for the `cfg` uses of a Cargo feature (N-13).
    pub feature_search: crate::feature_nav::FeatureSearch,
    /// The project's copyright header profile (saved with the session).
    pub copyright: crate::copyright::Copyright,
    /// Crate names (crates.io search, local registry) for `Cargo.toml` completion.
    pub crate_names: crate::manifest_completion::CrateNames,
    /// The last external page opened from the editor (docs.rs).
    pub opened_url: Option<String>,
    /// Share in Playground uploads (N-18).
    pub playground: crate::playground::Playground,
    /// Requests sent from `.http` files and their responses (F-03).
    pub http: crate::http_client::HttpClient,
    /// perf recordings of run configurations (A-01).
    pub profiler: crate::profiler::Profiler,
    pub coverage: crate::coverage::Coverage,
    /// The text version each `.http` editor got its run icons and diagnostics for.
    http_given: std::collections::HashMap<u64, u64>,
    /// Rows unfolded in the Cargo window's dependency trees (they start folded).
    pub cargo_open: std::collections::BTreeSet<String>,
    /// Editor groups, splits and floating editor windows.
    pub layout: crate::layout::Layout,
    /// A tab being dragged to another place.
    pub tab_drag: Option<crate::ui::editor_area::TabDrag>,
    /// Keyboard focus and menu actions for the active editor, wherever it is shown this frame.
    pub editor_frame: Option<(bool, Vec<Action>)>,
    pub new_project: Option<crate::ui::new_project::NewProjectDialog>,
    pub search_everywhere: Option<crate::search::SearchEverywhere>,
    /// The scope Search Everywhere used last (kept for the session).
    pub se_scope: crate::search::SeScope,
    pub find_bar: Option<crate::search::FindBar>,
    pub find_in_files: Option<crate::search::FindInFiles>,
    pub terminals: rc_term::Terminals,
    pub settings_backup: crate::settings_backup::State,
    pub repl_starting: Option<crate::repl::Starting>,
    pub repl_variables: crate::repl_variables::State,
    pub macro_tester: crate::macro_tester::Tester,
    pub local_ai: crate::local_ai::Assistant,
    pub terminal_starting: Option<crate::repl::Starting>,
    pub terminal_tools: crate::terminal_tools::Tools,
    pub terminal_attach: crate::terminal_attach::Picker,
    pub focus_terminal: bool,
    /// Results shown in the Find tool window (usages, implementations…).
    pub find_results: Option<(String, Vec<Target>)>,
    /// Screen position under the active caret (popups that belong to the editor).
    pub caret_screen: Option<egui::Pos2>,
    pub test_markers: Vec<crate::runner::TestMarker>,
    /// The Rust test being debugged, for Restart.
    pub(crate) debug_test: Option<(crate::runner::RunConfig, rc_cargo::RunTarget)>,
    /// The Run/Debug menu of a gutter icon: its line and where it opens.
    pub gutter_menu: Option<(usize, egui::Pos2)>,
    /// Binary target run by the icon of each line of the active Rust document.
    pub run_markers: std::collections::BTreeMap<usize, String>,
    /// Lines of the active document that run a benchmark, with the `cargo` arguments.
    pub bench_markers: std::collections::BTreeMap<usize, Vec<String>>,
    pub native_menu: Option<NativeMenu>,
    pub waker: Waker,
    statement_completion: Option<crate::complete_statement::Job>,
    fs_inbox: Inbox<FsEvent>,
    _watcher: Option<FileWatcher>,
    cargo_load: Option<Inbox<(Result<rc_cargo::CargoWorkspace, String>, Option<rc_cargo::LoadLog>)>>,
    /// Every load of the Cargo model, newest last (the Build window's Sync tab).
    pub sync_log: crate::sync_log::SyncLog,
    /// What Settings › Tools shows, read once it is needed.
    pub toolchain: Option<rc_cargo::ToolchainInfo>,
    libraries_load: Option<Inbox<(Vec<rc_cargo::Library>, Option<rc_cargo::LoadLog>)>>,
    /// The attached Cargo projects being read.
    attached_load: Option<Inbox<Vec<(Result<rc_cargo::CargoWorkspace, String>, rc_cargo::LoadLog)>>>,
    /// rustfmt running on open documents (Reformat Code, reformatting on save).
    pub rustfmt_jobs: crate::rustfmt::Jobs,
    toolchain_load: Option<Inbox<rc_cargo::ToolchainInfo>>,
    /// What the last `rustup check` said (Settings › Tools).
    pub toolchain_updates: Option<Result<Vec<rc_cargo::ToolchainUpdate>, String>>,
    toolchain_updates_load: Option<Inbox<Result<Vec<rc_cargo::ToolchainUpdate>, String>>>,
    /// What `rustc --explain` said per error code (`None`: it has no explanation).
    pub explanations: std::collections::HashMap<String, Option<String>>,
    /// The error code whose explanation rustc is printing.
    pub(crate) explain_load: Option<(String, Inbox<ExplainAnswer>)>,
    /// The Run tab of a rustup command: when it ends, the toolchain is read again.
    rustup_run: Option<String>,
    pub rustup_installer: crate::rustup_installer::RustupInstaller,
    /// The last Inspect Code run.
    pub inspection: Option<Inspection>,
    /// The Problems window shows the inspection instead of the open files' problems.
    pub problems_tab_inspection: bool,
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
    /// The patterns of automatic attach (D-15), as typed, and the watch while it runs.
    pub auto_attach_text: String,
    pub auto_attach: Option<crate::auto_attach::Watch>,
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
    pub fn new(cc: &eframe::CreationContext<'_>, open: Vec<crate::cli::OpenRequest>) -> Self {
        let settings = Settings::load();
        i18n::set_lang(settings.language.as_deref().map(Lang::from_locale).unwrap_or_else(Lang::system));
        rc_kit::theme::install_fonts(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let waker = Waker::new(move || ctx.request_repaint());
        let keymap = build_keymap(&settings);
        let native_menu = NativeMenu::install(&cc.egui_ctx, &keymap);
        let mut app = Self::with_settings(settings, waker, native_menu);
        crate::open_files::set_context(&cc.egui_ctx);
        // Files sent by Finder at launch arrive before the first frame: open them instead of
        // the last project.
        let mut from_system = crate::open_files::take();
        from_system.extend(open);
        for request in startup_paths(&app.settings, from_system) {
            app.open_request(&request);
        }
        app
    }

    /// Builds the app without a window (tests and headless captures).
    pub fn with_settings(settings: Settings, waker: Waker, native_menu: Option<NativeMenu>) -> Self {
        let keymap = build_keymap(&settings);
        let editor_config = editor_config(&settings);
        let pool = rc_syntax::ParserPool::new(waker.as_fn());
        CrabCraftApp {
            disk_revert: None,
            disk_watch: Default::default(),
            keymap,
            editor_config,
            workspace: None,
            docs: {
                let mut docs = Docs::new(pool);
                docs.history = Some(crate::local_history::HistoryService::start(rc_core::history::LocalHistory::open_default()));
                docs
            },
            dock: Dock::default(),
            explorer: ExplorerState::default(),
            dialog: None,
            notices: Vec::new(),
            insight: {
                let mut insight = Insight::new(waker.clone());
                insight.set_autoimport_exclude(clean_paths(&settings.autoimport_exclude));
                insight.imports_on_paste = settings.imports_on_paste;
                insight.profile = settings.inspection_profile.clean();
                insight
            },
            runner: {
                let mut runner = Runner::new(waker.clone());
                runner.nextest = settings.use_nextest;
                runner.external_tools = settings.external_tools.clone();
                runner
            },
            debugger: {
                let mut debugger = crate::debugger::Debugger::new(waker.clone());
                debugger.hex = settings.debugger_hex;
                debugger.rust_backend = settings.rust_debugger;
                debugger.gdb_path = settings.gdb_path.clone();
                debugger.lldb_dap_path = settings.lldb_dap_path.clone();
                debugger
            },
            debug_sessions: Default::default(),
            window_title: String::new(),
            files: crate::search::Files::new(),
            vcs: crate::vcs::Vcs::new(waker.clone()),
            nuget: crate::ui::nuget::NugetManager::new(waker.clone()),
            cargo_input: String::new(),
            hosting: crate::hosting::Hosting::new(waker.clone()),
            issue_tracker: crate::issue_tracker::Tracker::new(waker.clone()),
            history_edit: crate::history_edit::HistoryEdit::new(waker.clone()),
            shelves: crate::shelves::Shelves::new(waker.clone()),
            database: crate::database::Database::new(waker.clone()),
            endpoints: crate::endpoints::Endpoints::new(waker.clone()),
            clone_project: crate::clone_project::CloneProject::new(waker.clone()),
            borrow_errors: crate::borrow_errors::BorrowErrors::new(waker.clone()),
            missing_crates: crate::missing_crates::MissingCrates::new(waker.clone()),
            cargo_dependencies: crate::cargo_dependencies::CargoDependencies::new(waker.clone()),
            toolchain: None,
            attached_load: None,
            libraries_load: None,
            rustfmt_jobs: Default::default(),
            inspection: None,
            problems_tab_inspection: false,
            toolchain_updates: None,
            toolchain_updates_load: None,
            explanations: Default::default(),
            explain_load: None,
            rustup_run: None,
            rustup_installer: crate::rustup_installer::RustupInstaller::new(waker.clone()),
            toolchain_load: None,
            cargo_folded: Default::default(),
            cargo_trees: Default::default(),
            rustc_cfg: None,
            rustc_cfg_load: None,
            cfg_epoch: 0,
            cfg_given: Default::default(),
            cargo_features: Vec::new(),
            feature_icons_given: Default::default(),
            cargo_target: None,
            host_triple: None,
            triple_choices: Vec::new(),
            triple_load: None,
            triple_filter: String::new(),
            dep_versions: crate::dep_versions::DepVersions::new(waker.clone()),
            manifest_inspections: crate::manifest_inspect::ManifestInspections::new(waker.clone()),
            intention_preview: crate::intention_preview::IntentionPreview::new(waker.clone()),
            inspections: Default::default(),
            feature_search: Default::default(),
            copyright: Default::default(),
            crate_names: crate::manifest_completion::CrateNames::new(waker.clone()),
            opened_url: None,
            playground: crate::playground::Playground::new(waker.clone()),
            http: crate::http_client::HttpClient::new(waker.clone()),
            profiler: crate::profiler::Profiler::new(waker.clone()),
            coverage: crate::coverage::Coverage::new(waker.clone()),
            http_given: Default::default(),
            cargo_open: Default::default(),
            layout: Default::default(),
            tab_drag: None,
            editor_frame: None,
            new_project: None,
            search_everywhere: None,
            se_scope: crate::search::SeScope::Project,
            find_bar: None,
            find_in_files: None,
            terminals: rc_term::Terminals::new(),
            settings_backup: Default::default(),
            repl_starting: None,
            repl_variables: Default::default(),
            macro_tester: Default::default(),
            local_ai: Default::default(),
            terminal_starting: None,
            terminal_tools: Default::default(),
            terminal_attach: Default::default(),
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
            statement_completion: None,
            _watcher: None,
            cargo_load: None,
            sync_log: Default::default(),
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
            auto_attach_text: String::new(),
            auto_attach: None,
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
        self.notices.push(Notice { text: text.into(), error, at: 0.0, url: None });
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
        self.borrow_errors = crate::borrow_errors::BorrowErrors::new(self.waker.clone());
        self.missing_crates = crate::missing_crates::MissingCrates::new(self.waker.clone());
        self.cargo_dependencies = crate::cargo_dependencies::CargoDependencies::new(self.waker.clone());
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
        let session = Session::load(ws.key_path());
        // The user's exclusions come first: the index, the analysis and the explorer follow them.
        self.explorer.excluded = rc_core::workspace::clean_excluded(&session.excluded);
        self.insight.set_exclude(self.explorer.excluded.clone());
        // Attached Cargo projects that still exist (the session is from an earlier run).
        self.explorer.attached = session.attached_projects.iter().map(|r| ws.root.join(r)).filter(|p| p.is_file()).collect();
        self.insight.set_linked(linked_projects(&ws.root, &self.explorer.attached, &[]));
        self.files.open(&ws.root, self.waker.clone(), self.explorer.excluded.clone());
        self.vcs.open(&ws.root, self.settings.trusted_projects.contains(&ws.root));
        self.vcs.changelists = session.changelists.clone().clean();
        self.vcs.message = self.vcs.changelists.lists.get(&self.vcs.changelists.active).cloned().unwrap_or_default();
        let projects = match &ws.kind {
            rc_core::WorkspaceKind::Project(p) => vec![p.clone()],
            rc_core::WorkspaceKind::Folder => find_projects(&ws.root, 4),
            rc_core::WorkspaceKind::Solution(_) | rc_core::WorkspaceKind::Cargo(_) => Vec::new(),
        };
        let solution_path = if solution.is_some() { ws.solution().map(Path::to_path_buf) } else { None };
        self.runner.project_root = Some(ws.root.clone());
        self.runner.trusted_project = self.settings.trusted_projects.iter().find(|root| *root == &ws.root).cloned();
        self.runner.project_env = session.project_env.clone();
        self.insight.set_env(self.runner.project_env_pairs());
        self.cargo_features = clean_features(&session.cargo_features);
        self.insight.set_features(self.cargo_features.clone());
        self.cargo_target = Some(session.cargo_target.trim().to_string()).filter(|t| rc_cargo::valid_triple(t));
        self.copyright.open(&ws.root, session.copyright.clone(), self.waker.clone());
        self.explorer.scopes.open(&ws.root, &session.scopes, &session.file_colors);
        self.inspections = crate::inspection_profile::Profiles { project: session.inspection_profile.clean(), use_project: session.use_project_profile, ..Default::default() };
        self.refresh_inspection_profile();
        self.insight.set_target(self.cargo_target.clone());
        self.cfg_epoch += 1;
        if ws.cargo_manifest().is_some() {
            if self.settings.trusted_projects.iter().any(|p| p == &ws.root) {
                self.insight.start_rust(&ws.root, self.settings.rust_analyzer_path.clone(), self.settings.use_clippy);
            } else {
                self.insight.wait_for_trust(&ws.root);
            }
        } else if self.settings.trusted_projects.contains(&ws.root) {
            self.insight.start(&ws.root, solution_path.as_deref(), projects.clone(), dotnet_path(&self.settings), self.settings.roslyn_path.clone());
        } else {
            self.insight.wait_for_trust(&ws.root);
        }
        self.runner.compounds = session.compounds.clone();
        // The session file is the user's to edit: empty or overlong folder names are dropped.
        self.runner.folders = session.config_folders.iter().filter(|(_, f)| !f.trim().is_empty() && f.chars().count() <= crate::runner::MAX_FOLDER_CHARS).map(|(c, f)| (c.clone(), f.trim().to_string())).collect();
        self.auto_attach_text = session.auto_attach.chars().take(crate::auto_attach::MAX_PATTERNS * (crate::auto_attach::MAX_PATTERN_CHARS + 2)).collect();
        self.runner.test_configs = session.test_configs.clone();
        self.runner.options = session.run_options.clone();
        // Configurations stored in the project win over the session's copy.
        self.runner.options.extend(rc_core::run_files::load(&ws.root));
        self.runner.set_workspace(solution.as_ref(), &projects, &dotnet_path(&self.settings), session.run_configuration.as_deref());
        self.cargo_remembered = session.run_configuration.clone();
        let manifest = ws.cargo_manifest().map(Path::to_path_buf);
        self.docs.style_root = Some(ws.root.canonicalize().unwrap_or_else(|_| ws.root.clone()));
        self.workspace = Some(WorkspaceState { ws, solution });
        self.restore_session(session);
        // Scratches reopened with the session are analysed too.
        self.update_linked();
        if let Some(manifest) = manifest {
            self.load_cargo_workspace(manifest);
        }
    }

    /// Cargo metadata can execute project-selected tools and wrappers, even with --no-deps.
    /// Defer all project tool invocations until the application trust decision.
    fn load_cargo_workspace(&mut self, manifest: PathBuf) {
        if self.runner.require_trust().is_err() { return; }
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        let env = self.runner.project_env_pairs();
        std::thread::spawn(move || {
            let result = match rc_cargo::Cargo::locate().map(|c| c.with_env(env)) {
                Some(cargo) => {
                    let (result, log) = rc_cargo::load_workspace_logged(&cargo, &manifest);
                    (result.map_err(|e| e.to_string()), Some(log))
                }
                None => (Err(tr("Cargo was not found. Install Rust with rustup.").to_string()), None),
            };
            out.send(result);
        });
        self.cargo_load = Some(inbox);
    }

    pub(crate) fn poll_cargo_workspace(&mut self) {
        if let Some((libraries, log)) = self.libraries_load.as_ref().and_then(|inbox| inbox.drain().into_iter().last()) {
            self.libraries_load = None;
            if let Some(log) = log {
                self.sync_log.push(crate::sync_log::SyncKind::Dependencies, log);
            }
            // Canonical like the documents' paths (`\\?\C:\…` on Windows).
            self.docs.read_only_roots = libraries.iter().filter(|l| l.read_only).map(|l| l.root.canonicalize().unwrap_or_else(|_| l.root.clone())).collect();
            self.explorer.libraries = libraries;
        }
        if let Some(loaded) = self.attached_load.as_ref().and_then(|inbox| inbox.drain().into_iter().last()) {
            self.attached_load = None;
            let mut workspaces = Vec::new();
            for (result, log) in loaded {
                self.sync_log.push(crate::sync_log::SyncKind::Attached, log);
                match result {
                    Ok(w) => workspaces.push(w),
                    Err(e) => self.notify(trf("Could not read the Cargo project: {error}", &[("error", &e)]), true),
                }
            }
            self.runner.set_attached(workspaces);
            self.prune_features();
            self.cfg_epoch += 1;
        }
        let Some(inbox) = &self.cargo_load else { return };
        for (result, log) in inbox.drain() {
            if let Some(log) = log {
                self.sync_log.push(crate::sync_log::SyncKind::Project, log);
            }
            match result {
                Ok(workspace) => {
                    let root = workspace.root.clone();
                    self.runner.set_cargo_workspace(workspace, self.cargo_remembered.as_deref());
                    self.prune_features();
                    self.cfg_epoch += 1;
                    self.load_rustc_cfg(root.clone());
                    self.load_triples(root);
                    self.load_libraries();
                    self.load_attached();
                }
                Err(e) => self.notify(trf("Could not read the Cargo project: {error}", &[("error", &e)]), true),
            }
            self.cargo_load = None;
            return;
        }
    }

    /// Reads the attached Cargo projects in the background (their binaries become run
    /// configurations).
    fn load_attached(&mut self) {
        if self.runner.require_trust().is_err() { return; }
        let manifests = self.explorer.attached.clone();
        if manifests.is_empty() {
            self.attached_load = None;
            self.runner.set_attached(Vec::new());
            return;
        }
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        let env = self.runner.project_env_pairs();
        std::thread::spawn(move || {
            let Some(cargo) = rc_cargo::Cargo::locate().map(|c| c.with_env(env)) else { return };
            out.send(manifests.iter().map(|m| {
                let (result, log) = rc_cargo::load_workspace_logged(&cargo, m);
                (result.map_err(|e| e.to_string()), log)
            }).collect());
        });
        self.attached_load = Some(inbox);
    }

    /// Reads the dependencies in the background; without an up-to-date `Cargo.lock` only the
    /// standard library is known.
    fn load_libraries(&mut self) {
        if self.runner.require_trust().is_err() { return; }
        let Some(manifest) = self.runner.cargo_workspace.as_ref().map(|w| w.manifest.clone()) else { return };
        let project = self.workspace.as_ref().map(|w| w.ws.root.clone()).filter(|r| self.settings.trusted_projects.contains(r));
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        let env = self.runner.project_env_pairs();
        std::thread::spawn(move || {
            let Some(cargo) = rc_cargo::Cargo::locate().map(|c| c.with_env(env)) else { return };
            let (list, log) = rc_cargo::load_libraries_logged(&cargo, &manifest);
            let mut list = list.unwrap_or_default();
            let info = rc_cargo::detect_toolchain(Some(&cargo), None, project.as_deref());
            if let Some(root) = info.stdlib {
                let version = info.rustc.as_deref().and_then(|v| v.split_whitespace().nth(1)).unwrap_or_default().to_string();
                list.insert(0, rc_cargo::Library { name: "stdlib".into(), version, root, read_only: true });
            }
            out.send((list, Some(log)));
        });
        self.libraries_load = Some(inbox);
    }

    /// Saves and commits, with the Rust files of the commit formatted if that was asked for.
    /// Git › Show History: the Log of the Git window, for this file or folder only (V-06).
    pub fn show_git_history(&mut self, path: &Path) {
        self.vcs.show_file_history(path);
        self.dock.show(ToolWindow::Git);
    }

    pub fn commit_changes(&mut self, push: bool) {
        self.save_all_docs();
        let format = self.commit_format();
        self.vcs.commit(push, format);
    }

    /// The tool and the files to format before a commit.
    fn commit_format(&self) -> Option<(Result<rc_cargo::Rustfmt, String>, Vec<rc_cargo::FormatFile>)> {
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
        Some((self.rustfmt_tool(), files.collect()))
    }

    /// rustfmt as the settings say, or why it cannot run.
    fn rustfmt_tool(&self) -> Result<rc_cargo::Rustfmt, String> {
        let cargo = self.runner.cargo_tool()?;
        rc_cargo::Rustfmt::new(&cargo, &self.settings.rustfmt).map_err(crate::vcs::rustfmt_failure)
    }

    /// Runs rustfmt in the background on a Rust document of the Cargo workspace; `save` writes it
    /// again once formatted. False when the file is not in the workspace, so another formatter
    /// applies.
    pub fn rustfmt_document(&mut self, i: usize, save: bool) -> bool {
        let doc = &self.docs.list[i];
        let rust = doc.path.extension().is_some_and(|e| e == "rs");
        let Some(edition) = self.runner.cargo_workspace.as_ref().and_then(|w| w.edition_of(&doc.path)).filter(|_| rust).map(str::to_string) else { return false };
        match self.rustfmt_tool() {
            Ok(rustfmt) => {
                let doc = &self.docs.list[i];
                let job = rustfmt.start(doc.path.clone(), edition, doc.editor.buffer.rope().to_string(), self.waker.clone());
                // Reformat Code with a selection: rustfmt formats the file and only the changes
                // touching the selected lines are kept.
                let sel = doc.editor.selection.primary();
                let lines = (!save && !sel.is_empty()).then(|| {
                    let rope = doc.editor.buffer.rope();
                    let last = if sel.end() > sel.start() && rope.char_to_line(sel.end()) > rope.char_to_line(sel.start()) && rope.line_to_char(rope.char_to_line(sel.end())) == sel.end() { rope.char_to_line(sel.end()) - 1 } else { rope.char_to_line(sel.end()) };
                    (rope.char_to_line(sel.start()), last)
                });
                self.rustfmt_jobs.push_lines(doc.editor.id, doc.editor.buffer.version(), save, lines, job);
            }
            Err(e) => self.notify(e, true),
        }
        true
    }

    /// Applies what rustfmt returned for open documents.
    pub fn poll_rustfmt(&mut self) {
        if self.rustfmt_jobs.is_empty() {
            return;
        }
        for done in self.rustfmt_jobs.poll(&mut self.docs) {
            match done {
                crate::rustfmt::Done::Applied { index, save } => {
                    self.insight.flush(&mut self.docs.list[index]);
                    if save && self.docs.list[index].is_modified() {
                        match self.docs.save(index) {
                            Ok(()) => {
                                let path = self.docs.list[index].path.clone();
                                self.insight.did_save(&path);
                            }
                            Err(e) => self.notify(e, true),
                        }
                    }
                }
                // Saving half-written code is common; only an explicit Reformat Code reports.
                crate::rustfmt::Done::Failed { save: true, .. } => {}
                crate::rustfmt::Done::Failed { name, error, save: false } => {
                    let first = error.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().to_string();
                    self.notify(trf("rustfmt failed on {file}: {error}", &[("file", &name), ("error", &first)]), true);
                }
            }
        }
    }

    /// Reads the versions of the toolchain in the background, once.
    pub fn load_toolchain(&mut self) {
        if self.runner.require_trust().is_err() { return; }
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

    /// `rustup`, if Rust was installed with it.
    pub fn rustup(&self) -> Option<PathBuf> {
        rc_cargo::Cargo::locate().and_then(|c| rc_cargo::rustup_path(&c))
    }

    /// Asks rustup for toolchain updates in the background (it reads the network).
    pub fn check_toolchain_updates(&mut self) {
        let Some(rustup) = self.rustup() else {
            self.toolchain_updates = Some(Err(tr("rustup was not found: this toolchain was not installed with it").to_string()));
            return;
        };
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        std::thread::spawn(move || out.send(rc_cargo::rustup_check(&rustup).map_err(|e| e.to_string())));
        self.toolchain_updates = None;
        self.toolchain_updates_load = Some(inbox);
    }

    pub fn checking_toolchain_updates(&self) -> bool {
        self.toolchain_updates_load.is_some()
    }

    /// Runs `rustup <args>` in the Run window; the versions are read again when it ends.
    pub fn run_rustup(&mut self, args: &[&str]) {
        let Some(rustup) = self.rustup() else {
            self.notify(tr("rustup was not found: this toolchain was not installed with it"), true);
            return;
        };
        let outs = self.runner.run_command(rustup, args.iter().map(|a| a.to_string()).collect());
        self.rustup_run = self.runner.runs.last().map(|r| r.name.clone());
        crate::ui::run_windows::handle_outputs(self, outs);
    }

    /// Picks up `rustup check` results and the end of rustup commands.
    pub(crate) fn poll_toolchain_updates(&mut self) {
        self.rustup_installer.poll();
        if let Some(cargo) = self.rustup_installer.take_cargo() {
            self.toolchain = None;
            self.toolchain_load = None;
            self.toolchain_updates = None;
            self.runner.set_cargo_tool(cargo);
            if let Some(manifest) = self.workspace.as_ref().and_then(|w| w.ws.cargo_manifest()).map(Path::to_path_buf) {
                self.load_cargo_workspace(manifest);
            }
        }
        if let Some(result) = self.toolchain_updates_load.as_ref().and_then(|i| i.drain().into_iter().last()) {
            self.toolchain_updates = Some(result);
            self.toolchain_updates_load = None;
        }
        if let Some(name) = &self.rustup_run
            && self.runner.runs.iter().find(|r| &r.name == name).is_none_or(|r| r.exit.is_some())
        {
            self.rustup_run = None;
            self.toolchain = None;
            self.toolchain_updates = None;
        }
    }

    /// Changes the project's environment: rust-analyzer restarts with it and the Cargo project
    /// is read again (nothing restarts when it did not change).
    pub fn set_project_env(&mut self, text: String) {
        if self.runner.project_env == text {
            return;
        }
        self.runner.project_env = text;
        self.session_dirty = true;
        self.insight.set_env(self.runner.project_env_pairs());
        self.reload_cargo();
    }

    /// Turns the Cargo feature `feature` of `package` on or off for the analysis: rust-analyzer
    /// gets it as `cargo.features` and the code under `#[cfg(feature = …)]` is faded again.
    pub fn set_cargo_feature(&mut self, package: &str, feature: &str, on: bool) {
        let entry = format!("{package}/{feature}");
        if !rc_cargo::valid_feature_entry(&entry) || self.cargo_features.contains(&entry) == on {
            return;
        }
        if on {
            self.cargo_features.push(entry);
            self.cargo_features.sort();
        } else {
            self.cargo_features.retain(|e| *e != entry);
        }
        self.session_dirty = true;
        self.cfg_epoch += 1;
        self.insight.set_features(self.cargo_features.clone());
    }

    /// Whether `feature` of `package` is on: chosen by the user, or enabled by `default`.
    /// `None` when the package has no such feature.
    pub fn feature_state(&self, package: &rc_cargo::Package, feature: &str) -> Option<FeatureState> {
        if !package.features.contains_key(feature) {
            return None;
        }
        let chosen = package.chosen_features(&self.cargo_features);
        Some(if chosen.iter().any(|f| f == feature) {
            FeatureState::Chosen
        } else if package.enabled_features(&chosen).iter().any(|f| f == feature) {
            FeatureState::Implied
        } else {
            FeatureState::Off
        })
    }

    /// Click on the feature icon of a `Cargo.toml` line: turns that feature on or off. One that
    /// another enabled feature (or `default`) turns on cannot be turned off from here.
    pub fn toggle_feature_at(&mut self, line: usize) {
        let Some(doc) = self.docs.active_doc() else { return };
        let Some(package) = self.package_of_manifest(&doc.path) else { return };
        let Some(name) = rc_cargo::feature_lines(&doc.editor.buffer.rope().to_string()).into_iter().find(|(l, _)| *l == line).map(|(_, n)| n) else { return };
        match self.feature_state(&package, &name) {
            Some(FeatureState::Off) => self.set_cargo_feature(&package.name, &name, true),
            Some(FeatureState::Chosen) => self.set_cargo_feature(&package.name, &name, false),
            Some(FeatureState::Implied) => self.notify(trf("The feature {name} is turned on by default or by another enabled feature", &[("name", &name)]), false),
            None => {}
        }
    }

    /// Drops chosen features that a loaded package no longer has: Cargo, and so rust-analyzer,
    /// refuse a `--features` entry the package does not define. Entries of packages not loaded
    /// (yet) are kept.
    fn prune_features(&mut self) {
        let packages: Vec<&rc_cargo::Package> = self.runner.cargo_workspace.iter().chain(&self.runner.attached).flat_map(|w| &w.packages).collect();
        let before = self.cargo_features.len();
        self.cargo_features.retain(|e| {
            let Some((p, f)) = e.split_once('/') else { return false };
            packages.iter().filter(|pk| pk.name == p).all(|pk| pk.features.contains_key(f))
        });
        if self.cargo_features.len() != before {
            self.session_dirty = true;
            self.insight.set_features(self.cargo_features.clone());
        }
    }

    /// The member (or attached package) whose manifest is `path`.
    pub fn package_of_manifest(&self, path: &Path) -> Option<rc_cargo::Package> {
        let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let path = canon(path);
        self.runner.cargo_workspace.iter().chain(&self.runner.attached).flat_map(|w| &w.packages).find(|p| canon(&p.manifest) == path).cloned()
    }

    /// Feature switches in the gutter of an open `Cargo.toml`, one per `[features]` entry.
    pub(crate) fn update_feature_icons(&mut self) {
        use rc_editor::GutterIcon;
        let Some(i) = self.docs.active else { return };
        let doc = &self.docs.list[i];
        if doc.path.file_name().is_none_or(|n| n != "Cargo.toml") {
            return;
        }
        let stamp = (doc.editor.buffer.version(), self.cfg_epoch);
        if self.feature_icons_given.get(&doc.editor.id) == Some(&stamp) {
            return;
        }
        self.feature_icons_given.insert(doc.editor.id, stamp);
        let package = self.package_of_manifest(&doc.path);
        let lines = rc_cargo::feature_lines(&doc.editor.buffer.rope().to_string());
        let icons: Vec<(usize, GutterIcon)> = match &package {
            Some(p) => lines.into_iter().filter_map(|(line, name)| self.feature_state(p, &name).map(|s| (line, if s == FeatureState::Off { GutterIcon::FeatureOff } else { GutterIcon::FeatureOn }))).collect(),
            None => Vec::new(),
        };
        let doc = &mut self.docs.list[i];
        doc.editor.gutter_icons.retain(|_, icon| !matches!(icon, GutterIcon::FeatureOn | GutterIcon::FeatureOff));
        doc.editor.gutter_icons.extend(icons);
    }

    /// The quick fix of an outdated dependency: writes `version` as the requirement of the
    /// dependency declared on `line` of the active `Cargo.toml`, in its editor (undoable).
    pub fn update_dependency(&mut self, line: usize, version: &str) {
        let Some(doc) = self.docs.active_doc_mut() else { return };
        let text = doc.editor.buffer.rope().to_string();
        if let Some((start, end, req)) = dependency_update_edit(&text, line, version) {
            doc.editor.replace(start, end, &req);
        }
    }

    /// Run icons on the requests of the active `.http` file, and its parts that are not
    /// supported yet as warnings.
    pub(crate) fn update_http_file(&mut self) {
        use rc_editor::{Diagnostic, GutterIcon, Severity};
        let Some(i) = self.docs.active else { return };
        let doc = &mut self.docs.list[i];
        if doc.editor.lang() != rc_syntax::Lang::Http {
            return;
        }
        let version = doc.editor.buffer.version();
        if self.http_given.get(&doc.editor.id) == Some(&version) {
            return;
        }
        self.http_given.insert(doc.editor.id, version);
        let text = doc.editor.buffer.rope().to_string();
        doc.editor.gutter_icons.retain(|_, icon| *icon != GutterIcon::HttpRequest);
        for req in rc_syntax::http::parse_requests(&text) {
            doc.editor.gutter_icons.insert(req.line, GutterIcon::HttpRequest);
        }
        let diags = rc_syntax::http::unsupported_in(&text)
            .into_iter()
            .map(|u| Diagnostic {
                start: u.start,
                end: u.end.max(u.start + 1),
                severity: if u.kind.blocks_sending() { Severity::Warning } else { Severity::Info },
                message: crate::http_client::unsupported_text(&u.kind),
                code: Some("http".to_string()),
                unnecessary: false,
                deprecated: false,
            })
            .collect();
        doc.editor.set_diagnostics(diags);
    }

    /// Click on the run icon of a request in an `.http` file: sends it and shows the HTTP
    /// Client window.
    pub fn send_http_at(&mut self, line: usize) {
        let Some(doc) = self.docs.active_doc() else { return };
        let text = doc.editor.buffer.rope().to_string();
        let path = doc.path.clone();
        let Some(req) = rc_syntax::http::parse_requests(&text).into_iter().find(|r| r.line == line) else { return };
        self.http.start(&req, path);
        self.dock.show(crate::toolwindow::ToolWindow::Http);
    }

    /// Click on the docs.rs icon of a `Cargo.toml` dependency: opens its documentation for the
    /// locked version in the browser.
    pub fn open_docs_rs_at(&mut self, line: usize) {
        let Some(doc) = self.docs.active_doc() else { return };
        let Some(url) = self.dep_versions.links.get(&doc.editor.id).and_then(|l| l.get(&line)).cloned() else { return };
        #[cfg(not(test))]
        crate::ui::run_windows::open_url(&url);
        self.opened_url = Some(url);
    }

    /// Share in Playground: asks to confirm uploading the selection, or the whole Rust file.
    pub fn share_in_playground(&mut self) {
        let Some(doc) = self.docs.active_doc() else {
            self.notify(tr("Open a Rust file to share it on the Rust Playground"), false);
            return;
        };
        if doc.path.extension().is_none_or(|e| e != "rs") {
            self.notify(tr("Only Rust files can be shared on the Rust Playground"), false);
            return;
        }
        let selected = doc.editor.selected_text();
        let code = if selected.trim().is_empty() { doc.editor.buffer.rope().to_string() } else { selected };
        if code.trim().is_empty() {
            self.notify(tr("There is no code to share"), false);
            return;
        }
        if code.len() > rc_cargo::playground::MAX_CODE_BYTES {
            self.notify(crate::playground::error_text(&rc_cargo::playground::PlaygroundError::TooLarge), true);
            return;
        }
        if self.playground.running() {
            self.notify(tr("The code is still being uploaded to the Rust Playground"), false);
            return;
        }
        let edition = self.runner.cargo_workspace.as_ref().and_then(|w| w.edition_of(&doc.path));
        let share = crate::playground::Share::new(code, edition);
        self.dialog = Some(Dialog::SharePlayground(Box::new(share)));
    }

    /// The finished Playground uploads: the link is copied and offered in the status bar.
    fn poll_playground(&mut self, ctx: &Context) {
        for done in self.playground.poll() {
            match done {
                Ok(url) => {
                    ctx.copy_text(url.clone());
                    let text = trf("Shared on the Rust Playground; link copied: {url}", &[("url", &url)]);
                    self.notices.push(Notice { text, error: false, at: 0.0, url: Some(url) });
                }
                Err(e) => self.notify(e, true),
            }
        }
    }

    /// Opens a link offered by a notice in the browser.
    pub fn open_notice_url(&mut self, url: String) {
        #[cfg(not(test))]
        crate::ui::run_windows::open_url(&url);
        self.opened_url = Some(url);
    }

    /// Reads the Cargo project again.
    pub fn reload_cargo(&mut self) {
        self.cargo_trees.clear();
        if let Some(manifest) = self.runner.cargo_workspace.as_ref().map(|w| w.manifest.clone()) {
            self.load_cargo_workspace(manifest);
        }
    }

    /// Asks rustc for the `cfg` of the chosen target (or the host) in the background; it
    /// compiles nothing.
    fn load_rustc_cfg(&mut self, dir: PathBuf) {
        if self.runner.require_trust().is_err() { return; }
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        let target = self.cargo_target.clone();
        std::thread::spawn(move || {
            // Without a choice, what Cargo and rust-analyzer build for: the project's
            // `[build] target`, else the host.
            let target = target.or_else(|| rc_cargo::project_build_targets(&dir).into_iter().next());
            out.send(rc_cargo::Cargo::locate().and_then(|c| rc_cargo::print_cfg(&c, &dir, target.as_deref())))
        });
        self.rustc_cfg_load = Some(inbox);
    }

    /// Reads the host triple and the targets to offer (rustup's, the project's `[build] target`)
    /// in the background. Nothing of the project runs.
    fn load_triples(&mut self, root: PathBuf) {
        if self.runner.require_trust().is_err() { return; }
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        std::thread::spawn(move || out.send(rc_cargo::load_triple_choices(rc_cargo::Cargo::locate().as_ref(), &root)));
        self.triple_load = Some(inbox);
    }

    /// Analyses the code for `target` (`None` or the default one: the default): rust-analyzer
    /// gets it as `cargo.target` and the code under `#[cfg]` is faded for that target. An
    /// invalid triple is ignored.
    pub fn set_cargo_target(&mut self, target: Option<String>) {
        let target = target.filter(|t| self.default_triple() != Some(t.as_str()));
        if target.as_deref().is_some_and(|t| !rc_cargo::valid_triple(t)) || target == self.cargo_target {
            return;
        }
        self.cargo_target = target;
        self.session_dirty = true;
        self.insight.set_target(self.cargo_target.clone());
        if let Some(root) = self.runner.cargo_workspace.as_ref().map(|w| w.root.clone()) {
            self.load_rustc_cfg(root);
        }
    }

    /// The target triple shown in the status bar: the chosen one or the default one.
    pub fn analysis_triple(&self) -> Option<&str> {
        self.cargo_target.as_deref().or(self.default_triple())
    }

    /// What the code is built and analysed for without a choice: the project's
    /// `[build] target`, else the host.
    pub fn default_triple(&self) -> Option<&str> {
        self.triple_choices.iter().find(|c| c.from_config).map(|c| c.triple.as_str()).or(self.host_triple.as_deref())
    }

    /// Gives each Rust document the `cfg` set of its crate, so the code it turns off is faded.
    pub(crate) fn update_cfg_sets(&mut self) {
        if let Some((host, list)) = self.triple_load.as_ref().and_then(|i| i.drain().into_iter().last()) {
            self.triple_load = None;
            self.host_triple = host;
            self.triple_choices = list;
            // The target chosen in an earlier session may be the default now.
            if self.cargo_target.is_some() && self.cargo_target.as_deref() == self.default_triple() {
                self.cargo_target = None;
                self.insight.set_target(None);
            }
        }
        if let Some(cfg) = self.rustc_cfg_load.as_ref().and_then(|i| i.drain().into_iter().last()) {
            self.rustc_cfg_load = None;
            self.rustc_cfg = cfg;
            self.cfg_epoch += 1;
        }
        let epoch = self.cfg_epoch;
        let mut sets: std::collections::BTreeMap<PathBuf, std::sync::Arc<rc_syntax::CfgSet>> = Default::default();
        for doc in &mut self.docs.list {
            if self.cfg_given.get(&doc.editor.id) == Some(&epoch) || doc.editor.syntax.lang != rc_syntax::Lang::Rust {
                continue;
            }
            self.cfg_given.insert(doc.editor.id, epoch);
            let package = self.runner.cargo_workspace.iter().chain(&self.runner.attached).find_map(|w| w.package_of(&doc.path));
            let set = match (package, &self.rustc_cfg) {
                (Some(p), Some(cfg)) => Some(sets.entry(p.manifest.clone()).or_insert_with(|| std::sync::Arc::new(rc_syntax::CfgSet::from_rustc(cfg, &p.enabled_features(&p.chosen_features(&self.cargo_features))))).clone()),
                _ => None,
            };
            doc.editor.set_cfg(set);
        }
    }

    /// Reads the dependency tree of `package` in the background; `cargo tree` builds nothing.
    pub fn load_dep_tree(&mut self, manifest: PathBuf, package: String) {
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        let env = self.runner.project_env_pairs();
        let (m, p) = (manifest.clone(), package.clone());
        std::thread::spawn(move || {
            let result = match rc_cargo::Cargo::locate().map(|c| c.with_env(env)) {
                Some(cargo) => {
                    let (result, log) = rc_cargo::load_tree_logged(&cargo, &m, &p);
                    (result.map_err(|e| e.to_string()), Some(log))
                }
                None => (Err(tr("Cargo was not found. Install Rust with rustup.").to_string()), None),
            };
            out.send(result);
        });
        self.cargo_trees.insert((manifest, package), crate::ui::cargo_window::DepTree::Loading(inbox));
    }

    /// Takes the dependency trees that finished loading.
    pub(crate) fn poll_dep_trees(&mut self) {
        use crate::ui::cargo_window::DepTree;
        let mut logs = Vec::new();
        for tree in self.cargo_trees.values_mut() {
            let DepTree::Loading(inbox) = tree else { continue };
            let Some((result, log)) = inbox.drain().into_iter().last() else { continue };
            logs.extend(log);
            *tree = match result {
                Ok(nodes) => DepTree::Ready(nodes),
                Err(e) => DepTree::Failed(e),
            };
        }
        for log in logs {
            self.sync_log.push(crate::sync_log::SyncKind::Dependencies, log);
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

    /// Runs a target of an attached Cargo project: a binary through its run configuration,
    /// the rest with Cargo and the project's own manifest.
    pub fn run_attached_target(&mut self, ctx: &Context, manifest: &Path, package: &str, target: &rc_cargo::Target) {
        let folder = manifest.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let name = format!("{} ({folder})", target.name);
        if target.kind == rc_cargo::TargetKind::Bin
            && let Some(index) = self.runner.configs.iter().position(|c| c.name == name)
        {
            self.runner.selected = index;
            self.session_dirty = true;
            self.run_ide_action(ctx, Action::Run);
            return;
        }
        match rc_cargo::target_command(package, target) {
            Some(args) => {
                self.save_all_docs();
                let outs = self.runner.cargo_command_in(manifest, args);
                crate::ui::run_windows::handle_outputs(self, outs);
            }
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
        self.runner.trusted_project = Some(root.clone());
        if !self.vcs.trusted {
            // The deferred discovery must keep the session's changelists and draft.
            let changelists = self.vcs.changelists.clone();
            let message = self.vcs.message.clone();
            self.vcs.open(&root, true);
            self.vcs.changelists = changelists;
            self.vcs.message = message;
        }
        if let Some(manifest) = self.workspace.as_ref().and_then(|w| w.ws.cargo_manifest().map(Path::to_path_buf)) {
            self.insight.start_rust(&root, self.settings.rust_analyzer_path.clone(), self.settings.use_clippy);
            self.load_cargo_workspace(manifest);
        } else {
            let solution = self.workspace.as_ref().and_then(|w| w.solution.as_ref().map(|s| s.path.clone()));
            let projects = self.solution_projects();
            self.insight.start(&root, solution.as_deref(), projects, dotnet_path(&self.settings), self.settings.roslyn_path.clone());
        }
        self.toolchain = None;
    }

    /// Metadata/build fixtures opt in explicitly without launching a concurrent language
    /// server. Trust-boundary tests use the production trust_project method instead.
    #[cfg(test)]
    pub(crate) fn trust_project_tools_for_test(&mut self) {
        let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) else { return };
        if !self.settings.trusted_projects.contains(&root) { self.settings.trusted_projects.push(root.clone()); }
        self.runner.trusted_project = Some(root.clone());
        if !self.vcs.trusted {
            let changelists = self.vcs.changelists.clone();
            let message = self.vcs.message.clone();
            self.vcs.open(&root, true);
            self.vcs.changelists = changelists;
            self.vcs.message = message;
        }
        if let Some(manifest) = self.workspace.as_ref().and_then(|w| w.ws.cargo_manifest().map(Path::to_path_buf)) { self.load_cargo_workspace(manifest); }
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

    /// Throws away what the IDE derived from the project and builds it again: open files from
    /// disk, the file index, Git status, the Cargo model and rust-analyzer. Tabs, terminals and
    /// running programs stay. `clean` also deletes Cargo's build output.
    pub fn invalidate_caches(&mut self, clean: bool) {
        let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) else { return };
        let paths: Vec<PathBuf> = self.docs.list.iter().map(|doc| doc.path.clone()).collect();
        for path in paths {
            if let Err(error) = self.disk_watch.queue(&mut self.docs, &path) { self.notify(error.to_string(), true); }
        }
        for doc in &mut self.docs.list { doc.editor.request_parse(&self.docs.pool); }
        self.files.open(&root, self.waker.clone(), self.explorer.excluded.clone());
        self.vcs.refresh();
        let manifest = self.workspace.as_ref().and_then(|w| w.ws.cargo_manifest().map(Path::to_path_buf));
        if let Some(manifest) = manifest {
            if self.settings.trusted_projects.contains(&root) {
                self.insight.start_rust(&root, self.settings.rust_analyzer_path.clone(), self.settings.use_clippy);
            }
            if clean {
                self.save_all_docs();
                let outs = self.runner.cargo_command_in(&manifest, vec!["clean".into()]);
                crate::ui::run_windows::handle_outputs(self, outs);
            }
            self.load_cargo_workspace(manifest);
        } else if self.insight.session.is_some() {
            self.insight.restart();
        }
        self.notify(tr("Caches invalidated"), false);
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
            Some(RunConfig { name, kind: ConfigKind::WasmPack { manifest } }) => {
                let options = self.runner.options.get(&name).cloned().unwrap_or_default();
                self.dialog = Some(Dialog::WasmOptions { name, manifest, options });
            }
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
        self.disk_watch = Default::default();
        self.terminal_attach = Default::default();
        self.statement_completion = None;
        self.stop_all_debug_sessions();
        self.debug_sessions = Default::default();
        if self.workspace.is_some() {
            self.save_all_docs();
            self.save_session();
        }
        self.workspace = None;
        self.auto_attach = None;
        self._watcher = None;
        self.insight.stop();
        self.runner.stop_all();
        self.runner.clear();
        self.files.close();
        self.vcs.close();
        self.hosting = crate::hosting::Hosting::new(self.waker.clone());
        self.issue_tracker = crate::issue_tracker::Tracker::new(self.waker.clone());
        self.history_edit = crate::history_edit::HistoryEdit::new(self.waker.clone());
        self.shelves = crate::shelves::Shelves::new(self.waker.clone());
        self.database = crate::database::Database::new(self.waker.clone());
        self.endpoints = crate::endpoints::Endpoints::new(self.waker.clone());
        self.borrow_errors = crate::borrow_errors::BorrowErrors::new(self.waker.clone());
        self.missing_crates = crate::missing_crates::MissingCrates::new(self.waker.clone());
        self.cargo_dependencies = crate::cargo_dependencies::CargoDependencies::new(self.waker.clone());
        self.find_in_files = None;
        self.search_everywhere = None;
        self.find_bar = None;
        self.disk_revert = None;
        self.repl_starting = None;
        self.repl_variables = Default::default();
        self.macro_tester = Default::default();
        self.local_ai = Default::default();
        self.terminal_starting = None;
        self.terminal_tools = Default::default();
        self.coverage = crate::coverage::Coverage::new(self.waker.clone());
        self.terminals = rc_term::Terminals::new();
        self.find_results = None;
        self.docs.list.clear();
        self.docs.active = None;
        self.layout = Default::default();
        self.tab_drag = None;
        self.explorer = ExplorerState::default();
        self.docs.read_only_roots.clear();
        self.docs.style_root = None;
        self.sync_log.clear();
        self.inspection = None;
        self.libraries_load = None;
        self.rustfmt_jobs = Default::default();
        self.dock = Dock::default();
        self.nav_back.clear();
        self.nav_forward.clear();
        self.copyright.close();
        self.explorer.scopes.close();
        self.inspections = Default::default();
        self.refresh_inspection_profile();
    }

    fn restore_session(&mut self, s: Session) {
        self.hosting.restore(s.hosting.clone());
        self.issue_tracker.restore(s.issue_tracker.clone());
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
            // Breakpoints of files that are not open are kept until the file opens (D-27).
            if self.docs.index_of(path).is_none() {
                let disabled = s.disabled_breakpoints.get(path);
                let logs = s.logpoints.get(path);
                let mut kept = std::collections::BTreeMap::new();
                for (line, cond) in bps.iter().take(MAX_BREAKPOINTS_PER_FILE) {
                    let cond = cond.as_ref().filter(|c| !c.is_empty());
                    let log = logs.and_then(|l| l.iter().find(|(n, _)| n == line)).map(|(_, m)| m.clone());
                    let kind = if disabled.is_some_and(|d| d.contains(line)) {
                        rc_editor::BreakpointKind::Disabled
                    } else if log.is_some() {
                        rc_editor::BreakpointKind::Log
                    } else if cond.is_some() {
                        rc_editor::BreakpointKind::Conditional
                    } else {
                        rc_editor::BreakpointKind::Normal
                    };
                    if let Some(c) = cond {
                        self.debugger.conditions.insert((path.clone(), *line), c.clone());
                    }
                    if let Some(m) = log {
                        self.debugger.logs.insert((path.clone(), *line), m);
                    }
                    kept.insert(*line, kind);
                }
                for line in s.remove_once_hit.get(path).into_iter().flatten() {
                    self.debugger.once.insert((path.clone(), *line));
                }
                if !kept.is_empty() {
                    self.docs.closed_breakpoints.insert(path.clone(), kept);
                }
            }
            for (line, group) in s.breakpoint_groups.get(path).into_iter().flatten() {
                if let Some(g) = clean_group(group) {
                    self.debugger.groups.insert((path.clone(), *line), g);
                }
            }
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
        let all_breakpoints = self.docs.all_breakpoints();
        let breakpoints = all_breakpoints.iter().map(|(p, b)| (p.clone(), b.keys().map(|l| (*l, self.debugger.conditions.get(&(p.clone(), *l)).cloned())).collect())).collect();
        let (editor_layout, editor_windows) = self.layout.to_session(|id| self.docs.list.iter().position(|d| d.editor.id == id));
        Session {
            hosting: Some(self.hosting.config.clone()),
            issue_tracker: Some(self.issue_tracker.config.clone()),
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
            config_folders: self.runner.folders.clone(),
            auto_attach: self.auto_attach_text.clone(),
            test_configs: self.runner.test_configs.clone(),
            run_options: self.runner.options.clone(),
            changelists: {
                let mut lists = self.vcs.changelists.clone();
                lists.lists.insert(lists.active.clone(), self.vcs.message.clone());
                lists
            },
            excluded: self.explorer.excluded.clone(),
            attached_projects: self.workspace.as_ref().map(|w| self.explorer.attached.iter().filter_map(|p| rc_core::workspace::relative_path(&w.ws.root, p)).collect()).unwrap_or_default(),
            project_env: self.runner.project_env.clone(),
            cargo_features: self.cargo_features.clone(),
            cargo_target: self.cargo_target.clone().unwrap_or_default(),
            copyright: self.copyright.profile.clone(),
            scopes: self.explorer.scopes.user.clone(),
            file_colors: self.explorer.scopes.colors.clone(),
            inspection_profile: self.inspections.project.clone(),
            use_project_profile: self.inspections.use_project,
            breakpoints,
            logpoints: group_lines(self.debugger.logs.iter().map(|((p, l), m)| (p.clone(), (*l, m.clone())))),
            remove_once_hit: group_lines(self.debugger.once.iter().map(|(p, l)| (p.clone(), *l))),
            bookmarks: self.docs.bookmarks().into_iter().map(|(path, lines)| (path, lines.into_iter().collect())).collect(),
            disabled_breakpoints: all_breakpoints
                .iter()
                .map(|(p, b)| (p.clone(), b.iter().filter(|(_, k)| **k == rc_editor::BreakpointKind::Disabled).map(|(l, _)| *l).collect::<Vec<_>>()))
                .filter(|(_, v)| !v.is_empty())
                .collect(),
            breakpoint_groups: group_lines(self.debugger.groups.iter().filter(|((p, l), _)| all_breakpoints.get(p).is_some_and(|b| b.contains_key(l))).map(|((p, l), g)| (p.clone(), (*l, g.clone())))),
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

    /// Opens the Local History dialog for document `i`; its versions load in the background.
    pub fn show_local_history(&mut self, i: usize) {
        let (Some(doc), Some(h)) = (self.docs.list.get(i), &self.docs.history) else { return };
        let inbox = rc_core::Inbox::new(self.waker.clone());
        h.list(&doc.path, inbox.sender());
        self.dialog = Some(Dialog::LocalHistory(Box::new(crate::ui::dialogs::LocalHistoryView::new(doc.path.clone(), inbox))));
    }

    pub fn request_disk_revert(&mut self, index: usize) {
        let Some(doc) = self.docs.list.get(index) else { return; };
        if doc.generated { self.notify(tr("Generated documents cannot be reverted from disk"), false); return; }
        let ticket = crate::disk_revert::Ticket::capture(doc);
        let dirty = doc.is_modified();
        self.disk_revert = None;
        if dirty { self.dialog = Some(Dialog::RevertDisk(ticket)); }
        else { self.start_disk_revert(ticket); }
    }

    pub fn start_disk_revert(&mut self, ticket: crate::disk_revert::Ticket) {
        if ticket.index(&self.docs).is_none() {
            self.notify(crate::disk_revert::Error::Stale.to_string(), true);
            return;
        }
        match crate::disk_revert::Job::start(ticket, self.waker.clone()) {
            Ok(job) => self.disk_revert = Some(job),
            Err(error) => self.notify(error.to_string(), true),
        }
    }

    fn poll_disk_revert(&mut self) {
        let Some(result) = self.disk_revert.as_ref().and_then(crate::disk_revert::Job::poll) else { return; };
        let Some(job) = self.disk_revert.take() else { return; };
        match result.and_then(|snapshot| crate::disk_revert::apply(&mut self.docs, &job.ticket, snapshot)) {
            Ok(_) => { self.session_dirty = true; }
            Err(error) => self.notify(error.to_string(), true),
        }
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
                && !self.rustfmt_document(i, true)
            {
                self.insight.format_saved(&self.docs.list[i]);
            }
        }
    }

    pub(crate) fn poll_disk_watch(&mut self) {
        for error in self.disk_watch.poll(&mut self.docs, self.waker.clone()) { self.notify(error, true); }
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
                // Every new list replaces the previous one, even with a single result or none.
                InsightOutput::Usages { title, targets } if targets.is_empty() => {
                    if self.find_results.is_some() {
                        self.find_results = Some((title, targets));
                    }
                }
                InsightOutput::Usages { title, targets } => {
                    self.find_results = Some((title, targets));
                    self.dock.show(ToolWindow::Find);
                }
                InsightOutput::Actions(list) => self.dialog = Some(Dialog::CodeActions { list, selected: 0 }),
                InsightOutput::RefactorPreview { title, edit } => {
                    let open = self.docs.list.iter().map(|d| (d.path.clone(), d.editor.buffer.rope().clone())).collect();
                    let preview = crate::refactor_preview::RefactorPreview::start(title, &edit, open, self.waker.clone());
                    self.dialog = Some(Dialog::RefactorPreview(Box::new(preview)));
                }
                InsightOutput::RefactorConflicts(review) => self.dialog = Some(Dialog::RefactorConflicts(review)),
                InsightOutput::AskRename { doc, pos, name } => self.dialog = Some(Dialog::RenameSymbol { doc, pos, original: name.clone(), name, focus: true }),
                InsightOutput::Message(m) => self.notify(m, false),
                InsightOutput::ApplyFileOps(edit) => {
                    if let Err(e) = crate::file_ops::apply(self, &edit) {
                        self.notify(e, true);
                    }
                }
            }
        }
    }

    /// "Continue anyway" on a rename's conflicts: applies its edits, or previews them.
    pub fn continue_refactoring(&mut self, review: crate::refactor_conflicts::Review) {
        let Some(edit) = review.edit else { return };
        let mut out = Vec::new();
        crate::insight::finish_rename(&mut self.docs, edit, review.preview, &mut out);
        self.handle_insight(out);
    }

    /// Runs a code-insight action from the editor's keymap on the active document.
    pub fn run_insight_action(&mut self, action: Action, caret_screen: egui::Pos2) {
        let Some(i) = self.docs.active else { return };
        if action == Action::CompleteStatement {
            self.statement_completion = None;
            match crate::complete_statement::Job::start(&self.docs, self.waker.clone()) {
                Ok(job) => self.statement_completion = Some(job),
                Err(message) => self.notify(message, false),
            }
            return;
        }
        // Rust files of the workspace go to rustfmt with the user's settings, ready server or not.
        if action == Action::Reformat && self.runner.cargo_workspace.is_some() && self.docs.list[i].path.extension().is_some_and(|e| e == "rs") {
            if self.rustfmt_document(i, false) {
                return;
            }
        }
        if action == Action::Complete && crate::sql_injection::complete(&mut self.docs.list[i], &self.database.tables) {
            self.insight.cancel_local_completion_requests();
            return;
        }
        if action == Action::Complete && self.endpoints.complete_http(&mut self.docs.list[i]) {
            return;
        }
        // Cargo.toml is completed here: the Rust server does not complete TOML.
        if action == Action::Complete && crate::manifest_completion::is_manifest(&self.docs.list[i]) {
            let mut cx = crate::manifest_completion::Cx { names: &mut self.crate_names, deps: &mut self.dep_versions, online: self.settings.check_crates_io };
            crate::manifest_completion::complete(&mut self.docs.list[i], true, &mut cx);
            return;
        }
        // Cargo features and their `cfg` uses: rust-analyzer does not go between them.
        if matches!(action, Action::GotoDeclaration | Action::FindUsages) {
            let caret = self.docs.list[i].editor.caret();
            if self.feature_navigate(i, caret, action == Action::FindUsages) {
                return;
            }
            if crate::manifest_completion::is_manifest(&self.docs.list[i]) {
                self.notify(tr("Place the caret on a feature name to go to it or find its uses"), false);
                return;
            }
        }
        // The quick fixes of an outdated dependency (C-04) and of the manifest inspections (I-22)
        // are made here, not by a server.
        if action == Action::ShowIntentions && crate::manifest_completion::is_manifest(&self.docs.list[i]) {
            let doc = &self.docs.list[i];
            let line = doc.editor.caret_line_col().0;
            let quick_fix = |title: String, command: &str, arguments: Vec<serde_json::Value>| {
                let command = rc_lsp::lsp_types::Command { title: title.clone(), command: command.to_string(), arguments: Some(arguments) };
                rc_lsp::lsp_types::CodeActionOrCommand::CodeAction(rc_lsp::lsp_types::CodeAction {
                    title,
                    kind: Some(rc_lsp::lsp_types::CodeActionKind::QUICKFIX),
                    command: Some(command),
                    ..Default::default()
                })
            };
            let mut list = Vec::new();
            if let Some(version) = self.dep_versions.fix_at(doc.editor.id, line) {
                let version = version.to_string();
                list.push(quick_fix(trf("Update to {version}", &[("version", &version)]), crate::insight::UPDATE_DEPENDENCY_COMMAND, vec![serde_json::json!(line), serde_json::json!(version)]));
            }
            for fix in self.manifest_inspections.fixes_on_line(doc, line) {
                let args = vec![serde_json::json!(fix.start), serde_json::json!(fix.end), serde_json::json!(fix.old), serde_json::json!(fix.new)];
                list.push(quick_fix(crate::manifest_inspect::fix_title(&fix), crate::insight::MANIFEST_FIX_COMMAND, args));
            }
            if !list.is_empty() {
                self.dialog = Some(Dialog::CodeActions { list, selected: 0 });
                return;
            }
        }
        // A name hidden behind a Cargo feature (C-10): the IDE's fixes, then the server's.
        if action == Action::ShowIntentions {
            let fixes: Vec<_> = self.feature_fixes(i).iter().map(crate::feature_fix::FeatureFix::action).collect();
            if !fixes.is_empty() && !self.insight.ready() {
                self.dialog = Some(Dialog::CodeActions { list: fixes, selected: 0 });
                return;
            }
            self.insight.local_fixes = fixes;
        }
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
            Action::Reformat => self.insight.format(doc),
            Action::FileStructure => self.dialog = Some(Dialog::FileStructure { filter: String::new(), selected: 0 }),
            _ => {}
        }
    }

    /// Code › Update Copyright: the profile's header added to the active file, or the year
    /// range of the header it has brought up to date (one undoable edit).
    pub fn update_copyright(&mut self) {
        if self.workspace.is_none() {
            self.notify(tr("Open a project to use its copyright profile"), false);
            return;
        }
        let Some(d) = self.docs.active_doc() else { return };
        let text = d.editor.buffer.rope().to_string();
        let path = d.path.clone();
        let Some(edit) = self.copyright.edit_for(&path, &text) else {
            self.notify(tr("This file cannot have a copyright header"), false);
            return;
        };
        let message = match edit.change {
            crate::copyright::Change::Added => tr("Copyright header added"),
            crate::copyright::Change::Updated => tr("Copyright year updated"),
            crate::copyright::Change::Current => tr("The copyright header is up to date"),
        };
        if edit.change != crate::copyright::Change::Current
            && let Some(d) = self.docs.active_doc_mut()
        {
            let rope = d.editor.buffer.rope();
            let (start, end) = (rope.byte_to_char(edit.start), rope.byte_to_char(edit.end));
            d.editor.replace(start, end, &edit.text);
        }
        self.notify(message, false);
    }

    /// Settings › Scopes: the project's scopes and file colours, saved with its session.
    pub fn set_scopes(&mut self, draft: crate::scopes::Draft) {
        if self.explorer.scopes.draft() != draft {
            self.explorer.scopes.set(&draft.user, &draft.colors);
            self.session_dirty = true;
        }
    }

    /// Keeps the Changed Files scope in step with git's status (workspace-relative paths).
    fn sync_changed_scope(&mut self) {
        let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) else { return };
        let vcs = &self.vcs;
        self.explorer.scopes.sync_changed(vcs.status_version, || {
            let Some(repo) = vcs.root() else { return Default::default() };
            vcs.changes().iter().filter_map(|c| rc_core::workspace::relative_path(&root, &repo.join(&c.path))).collect()
        });
    }

    /// Settings › Copyright: the profile of the project, saved with its session.
    /// Puts the inspection profile in force (the project's or the Default one) everywhere
    /// findings are shown (I-18).
    pub fn refresh_inspection_profile(&mut self) {
        let profile = self.inspections.effective(&self.settings.inspection_profile).clone();
        self.insight.set_profile(&mut self.docs, profile);
    }

    /// Settings › Inspections, OK: whether the project uses its own profile, and that profile.
    pub fn set_project_inspections(&mut self, use_project: bool, profile: rc_core::inspections::InspectionProfile) {
        if self.workspace.is_none() {
            return;
        }
        let profile = profile.clean();
        if self.inspections.use_project != use_project || self.inspections.project != profile {
            self.inspections.use_project = use_project;
            self.inspections.project = profile;
            self.session_dirty = true;
        }
        self.refresh_inspection_profile();
    }

    pub fn set_copyright_profile(&mut self, profile: rc_core::session::CopyrightProfile) {
        if self.copyright.profile != profile {
            self.copyright.profile = profile;
            self.session_dirty = true;
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
            Ok(i) => {
                // A Rust scratch gets analysed on its own.
                if self.docs.list.get(i).is_some_and(|d| d.scratch && d.editor.lang() == rc_syntax::Lang::Rust) {
                    self.update_linked();
                }
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

    /// Shows rustc's explanation of an error code (`E0308`) in a read-only tab. `rustc --explain`
    /// runs on a thread, with a timeout; its answer is kept for the next time.
    pub fn explain_error(&mut self, code: &str) {
        let Some(code) = rc_cargo::error_code(code).map(str::to_string) else {
            self.notify(tr("Only rustc error codes (like E0308) have an explanation"), true);
            return;
        };
        if let Some(answer) = self.explanations.get(&code).cloned() {
            self.show_explanation(&code, answer.as_deref());
            return;
        }
        if self.explain_load.as_ref().is_some_and(|(c, _)| *c == code) {
            return;
        }
        // The project's toolchain file applies only once the project is trusted: naming a
        // missing toolchain would otherwise make rustup reach the network.
        let dir = self.workspace.as_ref().map(|w| w.ws.root.clone()).filter(|r| self.settings.trusted_projects.contains(r));
        let env = self.runner.project_env_pairs();
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        let asked = code.clone();
        std::thread::spawn(move || {
            let result = match rc_cargo::Cargo::locate() {
                Some(cargo) => rc_cargo::Explainer { dir, env, ..rc_cargo::Explainer::new(rc_cargo::explain::rustc_path(&cargo)) }.explain(&asked),
                None => Err(rc_cargo::ExplainError::NoRustc(tr("Cargo was not found. Install Rust with rustup.").to_string())),
            };
            out.send(result);
        });
        self.explain_load = Some((code, inbox));
    }

    /// Picks up the answer of `rustc --explain`.
    pub(crate) fn poll_explanation(&mut self) {
        let Some(result) = self.explain_load.as_ref().and_then(|(_, i)| i.drain().into_iter().last()) else { return };
        let Some((code, _)) = self.explain_load.take() else { return };
        match result {
            Ok(answer) => {
                self.explanations.insert(code.clone(), answer.clone());
                self.show_explanation(&code, answer.as_deref());
            }
            Err(e) => {
                let error = match e {
                    rc_cargo::ExplainError::InvalidCode => tr("Only rustc error codes (like E0308) have an explanation").to_string(),
                    rc_cargo::ExplainError::TimedOut => tr("rustc did not answer in time").to_string(),
                    rc_cargo::ExplainError::NoRustc(m) => trf("rustc could not be started: {error}", &[("error", &m)]),
                    rc_cargo::ExplainError::Failed(m) => m,
                };
                self.notify(trf("Could not explain {code}: {error}", &[("code", &code), ("error", &error)]), true);
            }
        }
    }

    fn show_explanation(&mut self, code: &str, text: Option<&str>) {
        let Some(text) = text else {
            self.notify(trf("rustc has no explanation for {code}", &[("code", &code)]), false);
            return;
        };
        // `code` was checked: `E` and four digits, safe as a file name.
        let path = rc_core::paths::explanations_dir().join(format!("{code}.md"));
        let body = format!("# {code}\n\n{text}\n");
        if let Err(e) = rc_core::paths::write_atomic(&path, body.as_bytes()) {
            self.notify(trf("Could not show the explanation: {error}", &[("error", &e.to_string())]), true);
            return;
        }
        match self.docs.open(&path) {
            Ok(i) => {
                let d = &mut self.docs.list[i];
                d.generated = true;
                if d.editor.buffer.rope() != body.as_str() {
                    d.editor.reload(&body);
                }
                d.editor.read_only = true;
                d.md_view = Some(crate::docs::MdView::Preview);
                d.mtime = std::fs::metadata(&d.path).and_then(|m| m.modified()).ok();
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
        let scratch = self.docs.list.get(i).is_some_and(|d| d.scratch && d.editor.lang() == rc_syntax::Lang::Rust);
        self.docs.close(i);
        if scratch {
            self.update_linked();
        }
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
        if matches!(action, Action::Run | Action::Debug) {
            // "Before launch" tools see the file and caret of this moment.
            self.runner.macro_context = self.macro_context();
            self.runner.external_tools = self.settings.external_tools.clone();
        }
        match action {
            Action::RevertFromDisk => {
                if let Some(index) = self.docs.active { self.request_disk_revert(index); }
            }
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
            Action::ShowSettings => {
                self.copyright.draft = None;
                self.dialog = Some(Dialog::settings(&self.settings));
            }
            Action::ProjectEnvironment => match self.workspace {
                Some(_) => self.dialog = Some(Dialog::ProjectEnv { text: self.runner.project_env.clone() }),
                None => self.notify(tr("Open a project to set its environment variables"), false),
            },
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
            Action::Profile => self.start_profiling(),
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
            Action::ViewBreakpoints => self.dialog = Some(Dialog::Breakpoints { selected: None, group: String::new() }),
            Action::RunToCursor | Action::ForceRunToCursor => {
                if let Some((path, line)) = self.docs.active_doc().map(|d| (d.path.clone(), d.editor.caret_line_col().0)) {
                    self.debugger.run_to_cursor(&path, line, action == Action::ForceRunToCursor);
                }
            }
            Action::SetExecutionPoint => {
                if let Some((path, line)) = self.docs.active_doc().map(|d| (d.path.clone(), d.editor.caret_line_col().0)) {
                    self.debugger.set_execution_point(&path, line);
                }
            }
            Action::ForceStepOver => {
                if let Some(s) = self.debugger.session.as_mut().filter(|s| s.state().is_stopped()) {
                    s.force_step_over();
                }
            }
            Action::ForceStepInto => {
                if let Some(s) = self.debugger.session.as_mut().filter(|s| s.state().is_stopped()) {
                    if s.can_force_step_in() {
                        s.force_step_in();
                    } else {
                        self.notify(tr("This debugger cannot force step into"), false);
                    }
                }
            }
            Action::ToggleHexValues => self.toggle_hex_values(),
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
            Action::ShowLocalHistory => {
                if let Some(i) = self.docs.active {
                    self.show_local_history(i);
                }
            }
            Action::OptimizeImports => {
                let mut out = Vec::new();
                self.insight.optimize_imports(&mut self.docs, &mut out);
                self.handle_insight(out);
            }
            Action::ShowBookmarks => self.dialog = Some(Dialog::Bookmarks { list: self.bookmark_list(), selected: 0 }),
            Action::RunAnything => self.dialog = Some(Dialog::RunAnything { text: String::new(), selected: 0 }),
            Action::SearchEverywhere | Action::FindAction | Action::GotoFile | Action::GotoClass | Action::GotoSymbol => {
                if let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) {
                    let tab = match action {
                        Action::GotoFile => crate::search::SeTab::Files,
                        Action::GotoClass => crate::search::SeTab::Classes,
                        Action::GotoSymbol => crate::search::SeTab::Symbols,
                        Action::FindAction => crate::search::SeTab::Actions,
                        _ => crate::search::SeTab::All,
                    };
                    if let Some(se) = &mut self.search_everywhere {
                        // Already open: the same shortcut again adds (or drops) the libraries,
                        // another one switches the tab and keeps the text.
                        if se.tab == tab {
                            se.scope = if se.scope.libraries() { crate::search::SeScope::Project } else { crate::search::SeScope::ProjectAndLibraries };
                        } else {
                            se.tab = tab;
                        }
                        se.selected = 0;
                        se.focus = true;
                    } else {
                        // The selected text (or the word at the caret) pre-fills the search.
                        let text = self.docs.active_doc().map(|d| d.editor.selected_text()).filter(|t| !t.contains('\n')).unwrap_or_default();
                        let packages = crate::search::package_scopes(&root, self.runner.cargo_workspace.as_ref());
                        self.search_everywhere = Some(crate::search::SearchEverywhere::new(tab, text).with_scope(self.se_scope.clone(), packages));
                    }
                }
            }
            Action::FindInFiles | Action::ReplaceInFiles => {
                if let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) {
                    let text = self.docs.active_doc().map(|d| d.editor.selected_text()).filter(|t| !t.is_empty() && !t.contains('\n'));
                    let fif = self.find_in_files.get_or_insert_with(|| crate::search::FindInFiles::new(&root, self.waker.clone(), String::new()));
                    fif.excluded = self.explorer.excluded.clone();
                    fif.only = self.explorer.scopes.filter(&fif.scope).map_err(|e| e.text());
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
            Action::ShareInPlayground => self.share_in_playground(),
            Action::SelectIn if self.docs.active_doc().is_some() => {
                let row = |title: &str, command: &str| {
                    rc_lsp::lsp_types::CodeActionOrCommand::Command(rc_lsp::lsp_types::Command { title: title.to_string(), command: command.to_string(), arguments: None })
                };
                let reveal = if cfg!(target_os = "macos") { tr("Reveal in Finder") } else { tr("Open in Explorer") };
                let list = vec![row(tr("Select in Explorer"), crate::insight::SELECT_IN_EXPLORER), row(reveal, crate::insight::REVEAL_FILE)];
                self.dialog = Some(Dialog::CodeActions { list, selected: 0 });
            }
            Action::InspectCode => self.inspect_code(),
            Action::UpdateCopyright => self.update_copyright(),
            Action::InvalidateCaches if self.workspace.is_some() => self.dialog = Some(Dialog::InvalidateCaches { clean: false }),
            Action::TestDeclarativeMacro => crate::macro_tester::open(self),
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
        if let Err(error) = self.runner.require_trust() { self.notify(error.to_string(), true); return; }
        if self.terminal_starting.is_some() { return; }
        let cwd = dir.or_else(|| self.workspace.as_ref().map(|w| w.ws.root.clone()));
        match crate::repl::Starting::new(rc_term::SpawnSpec::shell(cwd), self.waker.clone()) {
            Ok(starting) => {
                self.terminal_starting = Some(starting);
                self.dock.show(ToolWindow::Terminal);
            }
            Err(error) => self.notify(trf("Could not start the terminal: {error}", &[("error", &error)]), true),
        }
    }

    pub fn split_terminal(&mut self, stacked: bool) {
        if self.terminal_starting.is_some() { return; }
        let Some((id, cwd)) = self.terminals.active().map(|tab| (tab.id(), tab.session.cwd().map(Path::to_path_buf))) else { return; };
        self.terminal_tools.split_after_start = Some((id, stacked));
        self.open_terminal(cwd);
        if self.terminal_starting.is_none() { self.terminal_tools.split_after_start = None; }
    }

    fn poll_terminal_startup(&mut self) {
        let Some(result) = self.terminal_starting.as_ref().and_then(crate::repl::Starting::poll) else { return; };
        self.terminal_starting = None;
        match result {
            Ok(session) => {
                let second = self.terminals.push(session);
                if let Some((first, stacked)) = self.terminal_tools.split_after_start.take() {
                    if self.terminals.get(first).is_some() {
                        self.terminal_tools.split = Some(crate::terminal_tools::Split { first, second, stacked, ratio: 0.5 });
                    }
                }
                self.dock.show(ToolWindow::Terminal);
                self.focus_terminal = true;
            }
            Err(error) => {
                self.terminal_tools.split_after_start = None;
                self.notify(trf("Could not start the terminal: {error}", &[("error", &error)]), true);
            }
        }
    }

    /// Evcxr may compile code and load Cargo configuration from the current project.
    pub fn can_open_rust_repl(&self) -> bool {
        self.workspace.as_ref().is_some_and(|w| {
            matches!(w.ws.kind, rc_core::WorkspaceKind::Cargo(_))
                && self.settings.trusted_projects.contains(&w.ws.root)
        })
    }

    pub fn open_rust_repl(&mut self) {
        if !self.can_open_rust_repl() || self.repl_starting.is_some() { return; }
        let cwd = self.workspace.as_ref().map(|w| w.ws.root.clone());
        let mut spec = rc_term::SpawnSpec::rust_repl(cwd);
        spec.env.extend(self.runner.project_env_pairs());
        match crate::repl::Starting::new(spec, self.waker.clone()) {
            Ok(starting) => self.repl_starting = Some(starting),
            Err(error) => self.notify(trf("Could not start the Rust REPL: {error}", &[("error", &error)]), true),
        }
    }

    fn poll_repl(&mut self) {
        let Some(result) = self.repl_starting.as_ref().and_then(crate::repl::Starting::poll) else { return; };
        self.repl_starting = None;
        match result {
            Ok(session) => {
                let id = self.terminals.push(session);
                self.terminals.rename(id, Some(tr("Rust REPL").to_string()));
                self.repl_variables.register(id);
                self.dock.show(ToolWindow::Terminal);
                self.focus_terminal = true;
            }
            Err(error) => self.notify(trf("Could not start the Rust REPL: {error}", &[("error", &error)]), true),
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
    pub(crate) fn start_debugging(&mut self) {
        if let Err(error) = self.runner.require_trust() { self.notify(error.to_string(), true); return; }
        self.debug_test = None;
        self.save_all_docs();
        match self.runner.selected_config().cloned() {
            Some(crate::runner::RunConfig { kind: crate::runner::ConfigKind::WasmPack { .. }, .. }) => {
                self.notify(tr("Debugging wasm-pack configurations is not supported"), false);
            }
            Some(config) if matches!(config.kind, crate::runner::ConfigKind::Cargo { .. }) => match self.runner.cargo_debug_target(&config) {
                Some(Ok((cargo, target))) => {
                    let outs = self.debugger.start_cargo(config, cargo, target, &self.docs);
                    self.handle_debug(outs);
                }
                Some(Err(e)) => self.notify(e, true),
                None => {}
            },
            Some(crate::runner::RunConfig { kind: crate::runner::ConfigKind::TestSelection { .. }, .. }) => {
                self.notify(tr("Test configurations cannot be debugged yet: use Debug in the gutter of the test"), false);
            }
            Some(config) => {
                let dotnet = dotnet_path(&self.settings);
                let request = self.runner.request_for(&config);
                let outs = self.debugger.start(config, request, &dotnet, self.settings.netcoredbg_path.as_deref(), &self.docs);
                self.handle_debug(outs);
            }
            None => self.notify(tr("There is no run configuration to debug"), false),
        }
    }

    pub fn start_coverage(&mut self) {
        self.coverage.show = true;
        self.dock.show(ToolWindow::Tests);
        let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) else { return; };
        if !self.settings.trusted_projects.contains(&root) {
            self.coverage.error = Some(tr("Trust this Cargo project before measuring coverage").to_string());
            return;
        }
        let Some(manifest) = self.runner.cargo_workspace.as_ref().filter(|workspace| workspace.root == root).map(|workspace| workspace.manifest.clone()) else { return; };
        self.save_all_docs();
        match self.runner.cargo_tool() {
            Ok(cargo) => self.coverage.start(root, manifest, cargo.path().to_path_buf(), self.runner.project_env_pairs()),
            Err(error) => self.coverage.error = Some(error),
        }
    }

    /// Profile (A-01): builds the selected Rust binary and records it with perf.
    pub fn start_profiling(&mut self) {
        self.dock.show(ToolWindow::Profiler);
        let config = self.runner.selected_config().cloned();
        let Some(config) = config.filter(|c| matches!(c.kind, crate::runner::ConfigKind::Cargo { .. })) else {
            self.notify(tr("Choose the run configuration of a Rust binary to profile it"), false);
            return;
        };
        self.save_all_docs();
        match self.runner.cargo_debug_target(&config) {
            Some(Ok((cargo, target))) => self.profiler.start(config.name.clone(), &cargo, target),
            Some(Err(e)) => self.notify(e, true),
            None => {}
        }
    }

    /// Integers in hexadecimal or decimal in the debugger, kept in the settings.
    pub fn toggle_hex_values(&mut self) {
        let hex = !self.debugger.hex;
        self.debugger.set_hex(hex);
        self.settings.debugger_hex = hex;
        let _ = self.settings.save();
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
        if let Err(error) = self.runner.require_trust() { self.notify(error.to_string(), true); return; }
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
        let target = rc_cargo::RunTarget { before: Vec::new(), manifest, package: String::new(), bin: name.clone(), args, env: Vec::new(), working_dir: None, test: Some(runnable.cargo_args), options: Default::default() };
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
        // The run becomes a temporary configuration: Run and Debug start it again.
        let short = m.name.rsplit("::").next().unwrap_or(&m.name).rsplit('.').next().unwrap_or(&m.name).to_string();
        self.runner.select_tests(short, project.clone(), filter.clone());
        self.session_dirty = true;
        let outs = self.runner.run_tests(&project, Some(filter));
        crate::ui::run_windows::handle_outputs(self, outs);
    }

    pub fn toggle_tool(&mut self, tw: ToolWindow) {
        let was_open = self.dock.is_open(tw);
        if was_open && tw == ToolWindow::Database { self.database.cancel(); }
        if was_open && tw == ToolWindow::Endpoints { self.endpoints.cancel(); }
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

    /// Opens the settings on `page`.
    pub fn open_settings(&mut self, page: crate::ui::dialogs::SettingsPage) {
        self.copyright.draft = None;
        self.explorer.scopes.draft = None;
        self.dialog = Some(Dialog::Settings { draft: Box::new(self.settings.clone()), page });
    }

    /// Inspect Code: runs Clippy over the whole Cargo workspace; its findings go to the
    /// Problems window's Inspection tab, grouped by lint.
    pub fn inspect_code(&mut self) {
        let Some(manifest) = self.runner.cargo_workspace.as_ref().map(|w| w.manifest.clone()) else {
            self.notify(tr("Open a Cargo project to inspect its code"), false);
            return;
        };
        self.save_all_docs();
        let outs = self.runner.cargo_command_in(&manifest, rc_cargo::inspect_args(&manifest));
        if self.runner.build.as_ref().is_some_and(|b| b.handle.is_some()) {
            self.inspection = Some(Inspection { running: true, diagnostics: Vec::new(), failed: None });
        }
        crate::ui::run_windows::handle_outputs(self, outs);
    }

    /// Picks up the end of an inspection.
    pub(crate) fn poll_inspection(&mut self) {
        let Some(i) = self.inspection.as_mut().filter(|i| i.running) else { return };
        let Some(b) = &self.runner.build else { return };
        if b.handle.is_some() && b.result.is_none() && b.failed.is_none() {
            return;
        }
        i.running = false;
        i.diagnostics = b.diagnostics.clone();
        i.failed = b.failed.clone().or_else(|| b.result.as_ref().filter(|r| r.cancelled).map(|_| tr("Inspection cancelled").to_string()));
        self.problems_tab_inspection = true;
        self.dock.show(ToolWindow::Problems);
    }

    /// What the external tool macros stand for now: the active file, its caret and selection.
    pub fn macro_context(&self) -> rc_core::external_tools::MacroContext {
        let doc = self.docs.active_doc().filter(|d| !d.generated);
        let caret = doc.map(|d| d.editor.caret_line_col());
        rc_core::external_tools::MacroContext {
            file: doc.map(|d| d.path.clone()),
            project: self.workspace.as_ref().map(|w| w.ws.root.clone()),
            line: caret.map(|(l, _)| l + 1),
            column: caret.map(|(_, c)| c + 1),
            selection: doc.map(|d| d.editor.selected_text()).unwrap_or_default(),
        }
    }

    /// Runs the external tool called `name` in the Run window. Unsaved files are saved first,
    /// since the tool reads them from disk.
    pub fn run_external_tool(&mut self, name: &str) {
        let Some(tool) = self.settings.external_tools.iter().find(|t| t.name == name).cloned() else {
            self.notify(trf("There is no external tool called {name}", &[("name", &name)]), true);
            return;
        };
        self.save_all_docs();
        match external_plan(&tool, &self.macro_context(), self.runner.project_env_pairs()) {
            Ok(plan) => {
                let outs = self.runner.run_external(&tool.name, plan);
                crate::ui::run_windows::handle_outputs(self, outs);
            }
            Err(e) => self.notify(trf("Could not run {name}: {error}", &[("name", &tool.name), ("error", &e)]), true),
        }
    }

    /// Opens what the command line or the system asked for, at its line and column (1-based).
    pub fn open_request(&mut self, request: &crate::cli::OpenRequest) {
        match request.line {
            Some(line) if request.path.is_file() && self.project_of_manifest(&request.path).is_none() && !self.opens_workspace(&request.path) => {
                self.open_at(&request.path, line - 1, request.column.unwrap_or(1).saturating_sub(1));
            }
            _ => self.open_path(&request.path),
        }
    }

    /// Opens a folder in a CrabCraft window of its own: a new process started as
    /// `crabcraft -- <folder>`, which asks for trust like any project it opens.
    pub fn open_in_new_window(&mut self, path: &Path) {
        if !path.is_absolute() || !path.is_dir() {
            self.notify(trf("Could not open {path}: {error}", &[("path", &path.display()), ("error", &tr("the folder does not exist"))]), true);
            return;
        }
        let spawned = std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe).arg("--").arg(path).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn()
        });
        match spawned {
            // Reap it when it exits so it does not linger as a zombie.
            Ok(mut child) => drop(std::thread::Builder::new().name("new-window".into()).spawn(move || child.wait())),
            Err(e) => self.notify(trf("Could not open {path}: {error}", &[("path", &path.display()), ("error", &e)]), true),
        }
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
        let keymap_changed = new.keymap != self.settings.keymap || new.shortcuts != self.settings.shortcuts;
        let imports_changed = new.autoimport_exclude != self.settings.autoimport_exclude;
        self.settings = new;
        self.debugger.rust_backend = self.settings.rust_debugger;
        self.debugger.gdb_path = self.settings.gdb_path.clone();
        self.debugger.lldb_dap_path = self.settings.lldb_dap_path.clone();
        if imports_changed {
            for doc in &mut self.docs.list { doc.editor.popups.completion = None; }
        }
        self.runner.nextest = self.settings.use_nextest;
        self.runner.external_tools = self.settings.external_tools.clone();
        self.insight.set_autoimport_exclude(clean_paths(&self.settings.autoimport_exclude));
        self.insight.imports_on_paste = self.settings.imports_on_paste;
        self.settings.inspection_profile = self.settings.inspection_profile.clean();
        self.refresh_inspection_profile();
        self.editor_config = editor_config(&self.settings);
        if keymap_changed {
            self.keymap = build_keymap(&self.settings);
            Keymap::cancel_sequence(ctx, egui::Id::new("app_key_sequence"));
            if self.native_menu.is_some() && let Some(menu) = NativeMenu::install(ctx, &self.keymap) { self.native_menu = Some(menu); }
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
                ExplorerAction::ShowHistory(path) => self.show_git_history(&path),
                ExplorerAction::OpenInTerminal(dir) => self.open_terminal(Some(dir)),
                ExplorerAction::AddProject(sln) => self.new_project_dialog(Some(sln)),
                ExplorerAction::ManagePackages(project) => {
                    self.nuget.focus_project(&project);
                    self.dock.show(ToolWindow::Nuget);
                }
                ExplorerAction::SetExcluded(path, excluded) => self.set_excluded(&path, excluded),
                ExplorerAction::SetAttached(path, on) => self.set_attached(&path, on),
            }
        }
    }

    /// Excludes a folder or file of the workspace from the file index (Go to File, Search
    /// Everywhere), Find in Files and rust-analyzer (`excluded`), or includes it again. The
    /// index is rebuilt in the background and the server reloads with the new list.
    pub fn set_excluded(&mut self, path: &Path, excluded: bool) {
        let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) else { return };
        let Some(rel) = rc_core::workspace::relative_path(&root, path) else { return };
        let mut list = self.explorer.excluded.clone();
        if excluded {
            list.push(rel);
        } else {
            list.retain(|e| *e != rel);
        }
        let list = rc_core::workspace::clean_excluded(&list);
        if list == self.explorer.excluded {
            return;
        }
        self.explorer.excluded = list.clone();
        self.files.open(&root, self.waker.clone(), list.clone());
        if let Some(f) = &mut self.find_in_files {
            f.replacements.cancel();
            f.excluded = list.clone();
        }
        self.insight.set_exclude(list);
        self.session_dirty = true;
    }

    /// Tells rust-analyzer what to analyse: the attached projects and the open Rust scratches.
    pub fn update_linked(&mut self) {
        let Some(root) = self.workspace.as_ref().map(|w| w.ws.root.clone()) else { return };
        let scratches: Vec<PathBuf> = self.docs.list.iter().filter(|d| d.scratch && d.editor.lang() == rc_syntax::Lang::Rust).map(|d| d.path.clone()).collect();
        self.insight.set_linked(linked_projects(&root, &self.explorer.attached, &scratches));
    }

    /// Attaches the Cargo project of `manifest` (analysed by rust-analyzer with the workspace,
    /// though not a member), or detaches it. rust-analyzer restarts with the new list.
    pub fn set_attached(&mut self, manifest: &Path, on: bool) {
        if self.workspace.is_none() {
            return;
        }
        let mut list = self.explorer.attached.clone();
        list.retain(|p| p != manifest);
        if on {
            list.push(manifest.to_path_buf());
        }
        if list == self.explorer.attached {
            return;
        }
        self.explorer.attached = list;
        self.update_linked();
        self.load_attached();
        self.session_dirty = true;
        let name = manifest.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.notify(if on { trf("Attached the Cargo project {name}", &[("name", &name)]) } else { trf("Detached the Cargo project {name}", &[("name", &name)]) }, false);
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
        self.hosting.poll();
        if let Some(result) = self.statement_completion.as_ref().and_then(|job| job.poll(&mut self.docs)) {
            self.statement_completion = None;
            if let Err(message) = result { self.notify(message, false); }
        }
        self.issue_tracker.poll();
        if self.history_edit.poll() { self.vcs.refresh(); }
        if self.shelves.poll() { self.vcs.refresh(); }
        self.endpoints.poll(&mut self.docs);
        if let Some(mut find) = self.find_in_files.take() {
            find.poll();
            if find.replacements.poll(&mut self.docs) {
                self.session_dirty = true;
                find.search();
            }
            self.find_in_files = Some(find);
        }
        self.poll_cargo_workspace();
        self.poll_dep_trees();
        self.update_cfg_sets();
        self.poll_rustfmt();
        self.copyright.poll();
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
            if let Err(error) = self.disk_watch.queue(&mut self.docs, &p) { self.notify(error.to_string(), true); }
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
        self.poll_disk_watch();
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
        let root = self.workspace.as_ref().map(|w| w.ws.root.clone());
        let excluded = &self.explorer.excluded;
        let changes: Vec<(PathBuf, u8)> = events
            .iter()
            // Excluded folders are not the server's business.
            .filter(|e| root.as_deref().and_then(|r| rc_core::workspace::relative_path(r, e.path())).is_none_or(|rel| !rc_core::workspace::is_excluded(&rel, excluded)))
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
        if ctx.input(|input| input.events.iter().any(|event| matches!(event, egui::Event::PointerButton { pressed: true, .. } | egui::Event::WindowFocused(false) | egui::Event::Ime(_)))) {
            Keymap::cancel_sequence(ctx, egui::Id::new("app_key_sequence"));
        }
        // Settings › Keymap is recording a shortcut: the key belongs to it.
        if ctx.data(|d| d.get_temp::<String>(egui::Id::new("recording_shortcut")).is_some()) {
            if matches!(&self.dialog, Some(Dialog::Settings { page: crate::ui::dialogs::SettingsPage::Keymap, .. })) { return; }
            crate::ui::dialogs::cancel_shortcut_recording(ctx);
        }
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
        let editor_focused = self.docs.active_doc().is_some_and(|doc| ctx.memory(|memory| memory.focused()) == Some(rc_editor::EditorView::id_for(doc.editor.id)));
        let events = ctx.input(|input| input.events.clone());
        let mut consumed = std::collections::BTreeSet::new();
        let mut found = Vec::new();
        for (index,event) in events.iter().enumerate() {
            if let egui::Event::Key { key, pressed: true, modifiers, repeat, .. } = event {
                if *repeat && Keymap::sequence_repeat(ctx, egui::Id::new("app_key_sequence"), *key) { consumed.insert(index); continue; }
                if !repeat {
                    match keymap.sequence_key(ctx, egui::Id::new("app_key_sequence"), *key, *modifiers, editor_focused) {
                        rc_editor::keymap::SequenceKey::Waiting => { consumed.insert(index); continue; }
                        rc_editor::keymap::SequenceKey::Action(action) => { found.push(action); consumed.insert(index); continue; }
                        rc_editor::keymap::SequenceKey::Pass => {},
                    }
                }
                if let Some(action) = keymap.lookup(*key,*modifiers).filter(|action| action.is_ide()) {
                    found.push(action); consumed.insert(index);
                }
            } else if let egui::Event::Text(text) = event
                && Keymap::sequence_text(ctx, egui::Id::new("app_key_sequence"), text) { consumed.insert(index); }
        }
        ctx.input_mut(|input| { let mut index = 0; input.events.retain(|_| { let keep = !consumed.contains(&index); index += 1; keep }); });
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
/// The program, arguments and folder of an external tool, with its macros expanded. The
/// arguments are split first, so a path with spaces stays one argument.
pub fn external_plan(tool: &rc_core::external_tools::ExternalTool, ctx: &rc_core::external_tools::MacroContext, env: Vec<(String, String)>) -> Result<rc_dotnet::LaunchPlan, String> {
    use rc_core::external_tools::{expand, working_dir};
    let missing = |m: String| trf("{macro} has no value here (open a file or a project)", &[("macro", &m)]);
    let program = expand(tool.program.trim(), ctx).map_err(missing)?;
    if program.is_empty() {
        return Err(tr("the tool has no program").to_string());
    }
    let args = rc_cargo::split_args(&tool.args).ok_or_else(|| tr("the arguments have an unclosed quote").to_string())?;
    let args = args.iter().map(|a| expand(a, ctx)).collect::<Result<Vec<_>, _>>().map_err(missing)?;
    let dir = working_dir(tool, ctx).map_err(missing)?.or_else(|| std::env::current_dir().ok()).unwrap_or_default();
    if !dir.is_dir() {
        return Err(trf("the folder {path} does not exist", &[("path", &dir.display())]));
    }
    Ok(rc_dotnet::LaunchPlan { program: PathBuf::from(program), args, env, working_dir: dir, urls: Vec::new(), profile: None, browser_url: None })
}

/// Whether a Cargo feature of a package is on for the analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureState {
    /// Turned on by the user.
    Chosen,
    /// On because `default` or another enabled feature turns it on.
    Implied,
    Off,
}

/// The session's chosen features that are valid `package/feature` entries, sorted, once each.
fn clean_features(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = list.iter().filter(|e| rc_cargo::valid_feature_entry(e)).cloned().collect();
    out.sort();
    out.dedup();
    out
}

/// What rust-analyzer analyses: nothing listed (its own discovery) without attached projects
/// or scratch files; otherwise the workspace's `Cargo.toml`, when there is one, the attached
/// ones and the open Rust scratch files (analysed on their own).
pub fn linked_projects(root: &Path, attached: &[PathBuf], scratches: &[PathBuf]) -> Vec<PathBuf> {
    if attached.is_empty() && scratches.is_empty() {
        return Vec::new();
    }
    let own = root.join("Cargo.toml");
    own.is_file().then_some(own).into_iter().chain(attached.iter().cloned()).chain(scratches.iter().cloned()).collect()
}

/// Import paths as typed: trimmed, without blanks or `::` at either end, each once.
pub fn clean_paths(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in list {
        let p = p.trim().trim_matches(':').trim().to_string();
        if !p.is_empty() && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

pub fn startup_paths(settings: &Settings, from_system: Vec<crate::cli::OpenRequest>) -> Vec<crate::cli::OpenRequest> {
    if !from_system.is_empty() {
        return from_system;
    }
    settings.last_workspace.clone().filter(|p| settings.reopen_last && p.exists()).map(crate::cli::OpenRequest::path).into_iter().collect()
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
        self.poll_toolchain_updates();
        self.poll_explanation();
        for request in crate::open_files::take() {
            self.open_request(&request);
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
        self.poll_inspection();
        self.poll_background_debug_sessions();
        let outs = self.debugger.update(&mut self.docs);
        self.handle_debug(outs);
        self.poll_auto_attach();
        self.poll_repl();
        self.poll_disk_revert();
        self.poll_terminal_startup();
        for (_, ev) in self.terminals.poll() {
            if let rc_term::SessionEvent::Clipboard(text) = ev {
                ctx.copy_text(text);
            }
        }
        self.update_test_markers();
        self.update_feature_icons();
        let online = self.settings.check_crates_io;
        let waits = [
            self.dep_versions.update(&mut self.docs, &mut self.insight.problems, self.cfg_epoch, online, &self.insight.profile),
            crate::manifest_completion::CrateNames::update(&mut self.docs, &mut crate::manifest_completion::Cx { names: &mut self.crate_names, deps: &mut self.dep_versions, online }),
        ];
        if let Some(wait) = waits.into_iter().flatten().min() {
            ctx.request_repaint_after(wait);
        }
        if self.borrow_errors.poll(&mut self.docs) { self.focus_editor = true; self.session_dirty = true; }
        self.missing_crates.update(&self.docs, &self.runner, self.cfg_epoch, &ctx);
        self.manifest_inspections.update(&mut self.docs, &mut self.insight.problems, self.cfg_epoch, &self.insight.profile);
        self.poll_feature_search();
        self.poll_playground(&ctx);
        let database_trusted = self.workspace.as_ref().is_some_and(|workspace| self.settings.trusted_projects.contains(&workspace.ws.root));
        self.database.poll(database_trusted);
        self.http.poll();
        self.profiler.poll();
        self.coverage.poll();
        self.update_http_file();
        let dark = self.dark(&ctx);
        for n in self.vcs.update(&mut self.docs, dark) {
            self.notify(n, false);
        }
        if let Some(text) = self.vcs.clipboard.take() {
            ctx.copy_text(text);
        }
        self.sync_changed_scope();
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
        self.http.cancel_all();
        self.profiler.cancel();
        self.stop_all_debug_sessions();
        self.insight.stop();
        self.save_all_docs();
        self.save_session();
    }
}

/// The keymap of the settings with the user's shortcuts. Unknown actions or shortcuts in the
/// settings file are skipped.
pub fn build_keymap(settings: &Settings) -> Keymap {
    let actions = crate::search::action_items();
    let sequences: Vec<_> = settings.shortcuts.iter().filter_map(|(id,text)| {
        let action = actions.iter().map(|(_,a)| *a).find(|a| format!("{a:?}") == *id)?;
        let (first,second) = rc_editor::keymap::parse_sequence(text)?;
        Some((action,first,second))
    }).collect();
    Keymap::with_overrides(keymap_kind(settings.keymap), &shortcut_overrides(&settings.shortcuts)).with_sequences(&sequences)
}

/// The user's shortcuts as (action, shortcut) pairs; only actions of Find Action can change.
pub fn shortcut_overrides(map: &std::collections::BTreeMap<String, String>) -> Vec<(Action, Option<rc_editor::keymap::Chord>)> {
    let actions = crate::search::action_items();
    map.iter()
        .filter_map(|(id, chord)| {
            let action = actions.iter().map(|(_, a)| *a).find(|a| format!("{a:?}") == *id)?;
            if chord.is_empty() {
                return Some((action, None));
            }
            rc_editor::keymap::parse_chord(chord).map(|c| (action, Some(c)))
        })
        .collect()
}

pub fn keymap_kind(choice: KeymapChoice) -> KeymapKind { keymap_kind_for(choice, cfg!(target_os = "macos")) }

pub(crate) fn keymap_kind_for(choice: KeymapChoice, mac: bool) -> KeymapKind {
    match choice {
        KeymapChoice::RustRover if mac => KeymapKind::RustRoverMac,
        KeymapChoice::RustRover => KeymapKind::RustRover,
        KeymapChoice::VisualStudio => KeymapKind::VisualStudio,
        KeymapChoice::VsCode if mac => KeymapKind::VsCodeMac,
        KeymapChoice::VsCode => KeymapKind::VsCode,
        KeymapChoice::Rider if mac => KeymapKind::RiderMac,
        KeymapChoice::Rider => KeymapKind::Rider,
    }
}

pub fn editor_config(s: &Settings) -> EditorConfig {
    EditorConfig {
        font: rc_editor::layout::EditorFont { size: s.editor_font_size.clamp(8.0, 32.0), line_spacing: s.editor_line_spacing.clamp(1.0, 2.0) },
        show_indent_guides: s.show_indent_guides,
        show_inlays: s.show_inlay_hints,
        sticky_lines: if s.sticky_lines { 5 } else { 0 },
        scrollbar_lens: s.scrollbar_lens,
        right_margin: (s.right_margin > 0).then_some(s.right_margin),
        live_templates: rc_core::live_templates::for_editor(&s.live_templates),
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

/// The edit that updates the requirement of the dependency on `line` of a `Cargo.toml` to
/// `version`: chars `start..end` and the new requirement.
pub fn dependency_update_edit(text: &str, line: usize, version: &str) -> Option<(usize, usize, String)> {
    let version = semver::Version::parse(version).ok()?;
    let dep = rc_cargo::dependency_lines(text).into_iter().find(|d| d.line == line)?;
    let (start, end) = rc_cargo::versions::dep_spans(text, &dep).req?;
    Some((start, end, rc_cargo::versions::updated_requirement(dep.req.as_deref().unwrap_or_default(), &version)))
}
