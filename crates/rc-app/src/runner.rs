//! Build, run and test Rust projects and show their output in native consoles.

#[path = "wasm_pack.rs"]
pub(crate) mod wasm_pack;

use rc_core::{Inbox, Waker};
use rc_dotnet::{BuildAction, BuildEvent, BuildResult, OpHandle, OutputLine, RunEvent, RunHandle, Stream, TestEvent, TestFilter, TestOutcome, TestRequest, TestResult};
use rc_kit::i18n::{tr, trf};
use rc_term::TermSession;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigKind {
    /// Every test of a Cargo workspace.
    Tests { target: PathBuf },
    /// Several application configurations started together (by name).
    Compound { members: Vec<String> },
    /// A binary of a Cargo package.
    Cargo { package: String, bin: String },
    /// Builds a cdylib package with wasm-pack (does not launch a browser).
    WasmPack { manifest: PathBuf },
    /// An external tool of the settings, started as it is (never listed as a configuration).
    External { tool: String },
    /// A command the IDE runs for the user (`rustup update`…), started again by Rerun.
    Command { program: PathBuf, args: Vec<String> },
    /// A Rust scratch file, built with `rustc` and run (Rerun builds it again).
    Scratch { file: PathBuf },
    /// Some tests of a project (a gutter run, kept as a temporary or saved configuration).
    TestSelection { target: PathBuf, filter: TestFilter },
}

/// Longest folder name kept.
pub const MAX_FOLDER_CHARS: usize = 80;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunConfig {
    pub name: String,
    pub kind: ConfigKind,
}

impl RunConfig {
    pub fn icon(&self) -> rc_kit::Ico {
        match &self.kind {
            ConfigKind::WasmPack { .. } => rc_kit::ico::CONFIG_WEB,
            ConfigKind::Cargo { .. } => rc_kit::ico::CONFIG_APP,
            ConfigKind::Tests { .. } => rc_kit::ico::RUN_ALL,
            ConfigKind::TestSelection { .. } => rc_kit::ico::TEST_UNKNOWN,
            ConfigKind::Compound { .. } => rc_kit::ico::TW_SERVICES,
            ConfigKind::External { .. } | ConfigKind::Command { .. } => rc_kit::ico::RUN_ANYTHING,
            ConfigKind::Scratch { .. } => rc_kit::ico::CONFIG_APP,
        }
    }
}

/// A console fed with process output (an output-only terminal, so ANSI colours work).
pub struct Console {
    pub term: TermSession,
}

impl Console {
    pub fn new(waker: &Waker) -> Self {
        Console { term: TermSession::output_only(160, 40, waker.clone()) }
    }

    /// Raw program output (may end mid-line).
    pub fn raw(&self, text: &str, stream: Stream) {
        let mut s = text.replace("\r\n", "\n").replace('\n', "\r\n");
        if stream == Stream::Stderr && !s.contains('\x1b') {
            s = format!("\x1b[31m{s}\x1b[0m");
        }
        self.term.feed(s.as_bytes());
    }

    pub fn line(&self, line: &OutputLine) {
        let mut s = line.text.replace('\n', "\r\n");
        if line.stream == Stream::Stderr && !s.contains('\x1b') {
            s = format!("\x1b[31m{s}\x1b[0m");
        }
        if line.newline {
            s.push_str("\r\n");
        }
        self.term.feed(s.as_bytes());
    }

    /// A message from the IDE itself (grey).
    pub fn note(&self, text: &str) {
        self.term.feed(format!("\x1b[90m{}\x1b[0m\r\n", text.replace('\n', "\r\n")).as_bytes());
    }
}

pub struct BuildState {
    pub handle: Option<OpHandle>,
    inbox: Inbox<BuildEvent>,
    pub console: Console,
    pub diagnostics: Vec<rc_dotnet::Diagnostic>,
    pub result: Option<BuildResult>,
    pub failed: Option<String>,
    pub started: Instant,
    pub title: String,
    /// A command of the Cargo window: its exit code, not a compile result, tells how it went.
    pub command: bool,
}

pub struct RunTab {
    pub name: String,
    pub config: RunConfig,
    pub handle: Option<RunHandle>,
    inbox: Inbox<RunEvent>,
    pub console: Console,
    pub exit: Option<Option<i32>>,
    pub urls: Vec<String>,
    /// "Emulate terminal": the build, then the program in a pseudo-terminal.
    pty: Option<PtyRun>,
}

struct PtyRun {
    prepare: Option<OpHandle>,
    inbox: Inbox<rc_cargo::PrepareEvent>,
    /// The program runs in the console's own pseudo-terminal.
    started: bool,
}

impl RunTab {
    /// "Emulate terminal": shows the build, starts the program in a pseudo-terminal when it is
    /// ready and notices when it ends.
    fn update_pty(&mut self, waker: &Waker, out: &mut Vec<RunnerOutput>) {
        let Some(p) = &mut self.pty else { return };
        for ev in p.inbox.drain() {
            match ev {
                rc_cargo::PrepareEvent::Build(BuildEvent::Task(line)) => self.console.line(&line),
                rc_cargo::PrepareEvent::Build(BuildEvent::Diagnostic(d)) if d.severity == rc_dotnet::Severity::Error => {
                    self.console.note(&format!("{}({},{}): {}", d.file.as_ref().map(|f| f.display().to_string()).unwrap_or_default(), d.line.unwrap_or(0), d.column.unwrap_or(0), d.message));
                }
                rc_cargo::PrepareEvent::Build(BuildEvent::Finished(res)) if !res.success => out.push(RunnerOutput::BuildDiagnostics(res.diagnostics.clone())),
                rc_cargo::PrepareEvent::Build(_) => {}
                rc_cargo::PrepareEvent::Ready(plan) => {
                    p.prepare = None;
                    let mut spec = rc_term::SpawnSpec::command(plan.program.display().to_string(), plan.args.clone(), Some(plan.working_dir.clone())).size(160, 40);
                    for (k, v) in &plan.env {
                        spec = spec.env(k, v);
                    }
                    match TermSession::spawn(&spec, waker.clone()) {
                        Ok(term) => {
                            term.feed(format!("\x1b[2m{}\x1b[0m\r\n", plan.command_line()).as_bytes());
                            self.console.term = term;
                            p.started = true;
                        }
                        Err(e) => {
                            let e = format!("{e:#}");
                            self.console.note(&e);
                            self.exit = Some(None);
                            out.push(RunnerOutput::Notify(e, true));
                        }
                    }
                }
                rc_cargo::PrepareEvent::Failed(e) => {
                    p.prepare = None;
                    self.console.note(&e);
                    self.exit = Some(None);
                    out.push(RunnerOutput::Notify(e, true));
                }
                rc_cargo::PrepareEvent::Cancelled => {
                    p.prepare = None;
                    if self.exit.is_none() {
                        self.console.note(tr("Process stopped"));
                        self.exit = Some(None);
                    }
                }
            }
        }
        // The terminal prints "Process finished with exit code N" itself.
        if p.started
            && self.exit.is_none()
            && let Some(info) = self.console.term.exit_status()
        {
            self.exit = Some(if info.signal.is_some() { None } else { Some(info.code as i32) });
        }
    }

    pub fn running(&self) -> bool {
        self.exit.is_none()
    }

    /// Whether the console takes keyboard input itself (a pseudo-terminal).
    pub fn is_terminal(&self) -> bool {
        self.pty.as_ref().is_some_and(|p| p.started)
    }

    /// Stops the build or the program, whichever is going.
    pub fn stop(&mut self) {
        if let Some(h) = &self.handle {
            h.stop();
        }
        if let Some(p) = &mut self.pty {
            if let Some(h) = p.prepare.take() {
                h.cancel();
            }
            if p.started {
                self.console.term.kill();
            } else if self.exit.is_none() {
                self.console.note(tr("Process stopped"));
                self.exit = Some(None);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestState {
    Running,
    Passed,
    Failed,
    Skipped,
}

pub struct TestsState {
    pub handle: Option<OpHandle>,
    inbox: Inbox<TestEvent>,
    pub console: Console,
    /// Results by fully qualified name (every data case listed).
    pub results: BTreeMap<String, Vec<TestResult>>,
    pub running: bool,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration: Option<std::time::Duration>,
    pub failure: Option<String>,
    pub request: TestRequest,
    pub selected: Option<String>,
    pub show_passed: bool,
    /// Text typed to narrow the results.
    pub filter: String,
}

/// What the runner tells the rest of the UI.
pub enum RunnerOutput {
    ShowTool(crate::toolwindow::ToolWindow),
    Notify(String, bool),
    /// A build finished: its diagnostics for the Problems window.
    BuildDiagnostics(Vec<rc_dotnet::Diagnostic>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectExecutionError { Untrusted }
impl std::fmt::Display for ProjectExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(tr("Trust this project before running project tools or code"))
    }
}
impl std::error::Error for ProjectExecutionError {}

pub struct Runner {
    /// The external tools of the settings, for "before launch" tasks.
    pub external_tools: Vec<rc_core::external_tools::ExternalTool>,
    /// What the macros of those tools stand for, set by the app before each run.
    pub macro_context: rc_core::external_tools::MacroContext,
    pub configs: Vec<RunConfig>,
    pub selected: usize,
    pub build: Option<BuildState>,
    pub runs: Vec<RunTab>,
    pub active_run: usize,
    pub tests: Option<TestsState>,
    /// Last outcome per test (fully qualified name), for gutter icons.
    pub test_outcomes: BTreeMap<String, TestState>,
    /// The lines where tests failed in the last run, with why.
    pub test_failures: BTreeMap<PathBuf, BTreeMap<usize, String>>,
    waker: Waker,
    cargo: Option<rc_cargo::Cargo>,
    cargo_unavailable:Option<String>,
    /// The open Rust workspace.
    pub cargo_workspace: Option<rc_cargo::CargoWorkspace>,
    /// Set from the application trust store, never from project session files.
    pub trusted_project: Option<PathBuf>,
    /// Application-owned open root; never populated from project settings.
    pub project_root: Option<PathBuf>,
    /// Compound configurations of the workspace (name, member names), kept in the session.
    pub compounds: Vec<(String, Vec<String>)>,
    /// Saved test configurations (from the session).
    pub test_configs: Vec<rc_core::session::SavedTestConfig>,
    /// Temporary test configurations, oldest first.
    pub temporary: Vec<RunConfig>,
    /// Folders of the configurations (B-21): configuration name → folder.
    pub folders: BTreeMap<String, String>,
    /// Attached Cargo projects (analysed and run with the workspace, not members of it).
    pub attached: Vec<rc_cargo::CargoWorkspace>,
    /// Names of the configurations made for their binaries.
    attached_configs: Vec<String>,
    /// Edited application configurations, by name (kept in the session).
    pub options: BTreeMap<String, rc_core::session::RunOptions>,
    /// Environment of every Cargo command, `NAME=value` per line (kept in the session).
    pub project_env: String,
    /// Rust tests run with cargo-nextest (a setting).
    pub nextest: bool,
}

impl Runner {
    pub fn new(waker: Waker) -> Self {
        Runner { external_tools: Vec::new(), macro_context: Default::default(), configs: Vec::new(), selected: 0, build: None, runs: Vec::new(), active_run: 0, tests: None, test_outcomes: BTreeMap::new(), test_failures: BTreeMap::new(), waker, cargo: None, cargo_unavailable:None, cargo_workspace: None, trusted_project: None, project_root: None, compounds: Vec::new(), test_configs: Vec::new(), temporary: Vec::new(), folders: BTreeMap::new(), attached: Vec::new(), attached_configs: Vec::new(), options: BTreeMap::new(), project_env: String::new(), nextest: false }
    }

    /// Builds the run configurations of a Rust workspace: its binaries, then all its tests.
    pub fn set_cargo_workspace(&mut self, workspace: rc_cargo::CargoWorkspace, remembered: Option<&str>) {
        self.set_cargo_workspace_with_tool(workspace,rc_cargo::Cargo::locate(),remembered);
    }
    pub fn set_cargo_workspace_with_tool(&mut self,workspace:rc_cargo::CargoWorkspace,cargo:Option<rc_cargo::Cargo>,remembered:Option<&str>){
        let selected = self.selected_config().map(|c| c.name.clone());
        self.cargo=cargo;self.cargo_unavailable=None;
        self.configs = workspace.binaries().map(|(p, t)| RunConfig { name: t.name.clone(), kind: ConfigKind::Cargo { package: p.name.clone(), bin: t.name.clone() } }).collect();
        for package in workspace.packages.iter().filter(|p| p.cdylib) {
            self.configs.push(RunConfig { name: format!("wasm-pack: {}", package.name), kind: ConfigKind::WasmPack { manifest: package.manifest.clone() } });
        }
        self.configs.push(RunConfig { name: tr("All Tests").to_string(), kind: ConfigKind::Tests { target: workspace.manifest.clone() } });
        self.cargo_workspace = Some(workspace);
        self.insert_attached_configs();
        self.insert_compounds();
        self.insert_test_configs();
        let wanted = selected.as_deref().or(remembered);
        self.selected = wanted.and_then(|r| self.configs.iter().position(|c| c.name == r)).unwrap_or(0);
    }

    pub fn set_cargo_tool(&mut self, cargo: Option<rc_cargo::Cargo>) { self.cargo = cargo;self.cargo_unavailable=None; }
    pub fn set_cargo_unavailable(&mut self,message:String){self.cargo=None;self.cargo_unavailable=Some(message);}

    pub fn cargo_tool(&self) -> Result<rc_cargo::Cargo, String> {
        self.cargo()
    }

    fn cargo(&self) -> Result<rc_cargo::Cargo, String> {
        self.require_trust().map_err(|e| e.to_string())?;
        self.cargo.clone().map(|c| c.with_env(self.project_env_pairs())).ok_or_else(|| self.cargo_unavailable.clone().unwrap_or_else(||tr("Cargo was not found. Install Rust with rustup.").to_string()))
    }

    /// The project's environment; nothing when the session holds an invalid one.
    pub fn project_env_pairs(&self) -> Vec<(String, String)> {
        rc_core::session::parse_env(&self.project_env).unwrap_or_default()
    }

    pub fn clear(&mut self) {
        self.cargo=None;self.cargo_unavailable=None;
        self.cargo_workspace = None;
        self.trusted_project = None;
        self.project_root = None;
        self.configs.clear();
        self.temporary.clear();
        self.attached.clear();
        self.attached_configs.clear();
        self.build = None;
        self.runs.clear();
        self.tests = None;
        self.test_outcomes.clear();
    }

    pub fn selected_config(&self) -> Option<&RunConfig> {
        self.configs.get(self.selected)
    }

    pub fn busy(&self) -> bool {
        self.build.as_ref().is_some_and(|b| b.handle.is_some()) || self.tests.as_ref().is_some_and(|t| t.running) || self.runs.iter().any(|r| r.running())
    }

    /// Every project subprocess, including metadata and wrappers, requires the application
    /// trust store. Standalone user-invoked scratches have no project root.
    pub fn require_trust(&self) -> Result<(), ProjectExecutionError> {
        if self.project_root.as_ref().or_else(|| self.cargo_workspace.as_ref().map(|w| &w.root)).is_some_and(|root| self.trusted_project.as_ref() != Some(root)) {
            Err(ProjectExecutionError::Untrusted)
        } else { Ok(()) }
    }

    // ------------------------------------------------------------------ build

    pub fn build(&mut self, target: &Path, action: BuildAction) -> Vec<RunnerOutput> {
        if let Err(error) = self.require_trust() { return vec![RunnerOutput::Notify(error.to_string(), true)]; }
        let inbox = Inbox::new(self.waker.clone());
        let console = Console::new(&self.waker);
        let title = match action {
            BuildAction::Rebuild => tr("Rebuild"),
            BuildAction::Clean => tr("Clean"),
            BuildAction::Restore => tr("Restore"),
            BuildAction::Build => tr("Build"),
        }
        .to_string();
        let manifest = self.cargo_workspace.as_ref().map_or(target, |workspace| workspace.manifest.as_path());
        let handle = match self.cargo() {
            Ok(cargo) => rc_cargo::start_build(&cargo, manifest, action, inbox.sender()),
            Err(error) => return vec![RunnerOutput::Notify(error, true)],
        };
        self.build = Some(BuildState { handle: Some(handle), inbox, console, diagnostics: Vec::new(), result: None, failed: None, started: Instant::now(), title, command: false });
        vec![RunnerOutput::ShowTool(crate::toolwindow::ToolWindow::Build)]
    }

    /// Runs a Cargo command in the workspace, shown in the Build window.
    pub fn cargo_command(&mut self, args: Vec<String>) -> Vec<RunnerOutput> {
        if let Err(error) = self.require_trust() { return vec![RunnerOutput::Notify(error.to_string(), true)]; }
        let Some(manifest) = self.cargo_workspace.as_ref().map(|w| w.manifest.clone()) else { return Vec::new() };
        self.cargo_command_in(&manifest, args)
    }

    /// Runs `cargo <args>` for `manifest`, which need not be loaded yet.
    pub fn cargo_command_in(&mut self, manifest: &Path, args: Vec<String>) -> Vec<RunnerOutput> {
        if let Err(error) = self.require_trust() { return vec![RunnerOutput::Notify(error.to_string(), true)]; }
        if self.build.as_ref().is_some_and(|b| b.handle.is_some()) {
            return vec![RunnerOutput::Notify(tr("Wait for the running build to finish").into(), false)];
        }
        let manifest = manifest.to_path_buf();
        let cargo = match self.cargo() {
            Ok(cargo) => cargo,
            Err(e) => return vec![RunnerOutput::Notify(e, true)],
        };
        let inbox = Inbox::new(self.waker.clone());
        let console = Console::new(&self.waker);
        if !cargo.supports_cargo_args(&args){return vec![RunnerOutput::Notify(crate::criticalup_provider::unsupported(),true)];}
        let title = format!("cargo {}", args.first().map(String::as_str).unwrap_or_default());
        let handle = rc_cargo::start_command(&cargo, &manifest, args, inbox.sender());
        self.build = Some(BuildState { handle: Some(handle), inbox, console, diagnostics: Vec::new(), result: None, failed: None, started: Instant::now(), title, command: true });
        vec![RunnerOutput::ShowTool(crate::toolwindow::ToolWindow::Build)]
    }

    pub fn cancel_build(&mut self) {
        if let Some(b) = &mut self.build
            && let Some(h) = b.handle.take()
        {
            h.cancel();
        }
    }

    // ------------------------------------------------------------------ run

    /// Starts the selected configuration (apps run in the Run window, tests in Tests).
    pub fn run_selected(&mut self) -> Vec<RunnerOutput> {
        if let Err(error) = self.require_trust() { return vec![RunnerOutput::Notify(error.to_string(), true)]; }
        let Some(cfg) = self.selected_config().cloned() else {
            return vec![RunnerOutput::Notify(tr("There is nothing to run: the workspace has no executable or test projects").into(), false)];
        };
        match cfg.kind.clone() {
            ConfigKind::Cargo { package, bin } => self.run_cargo(cfg, package, bin),
            ConfigKind::WasmPack { manifest } => self.run_wasm_pack(cfg, &manifest),
            ConfigKind::Tests { target } => self.run_tests(&target, None),
            ConfigKind::TestSelection { target, filter } => self.run_tests(&target, Some(filter)),
            // Tools start from their menu, with the file and caret of that moment.
            ConfigKind::External { .. } => Vec::new(),
            ConfigKind::Scratch { file } => self.run_scratch(&file),
            ConfigKind::Command { program, args } => {
                let plan = self.command_plan(program, args);
                self.run_process(cfg, plan)
            }
            ConfigKind::Compound { members } => {
                let apps: Vec<RunConfig> = members.iter().filter_map(|m| self.configs.iter().find(|c| &c.name == m && matches!(c.kind, ConfigKind::Cargo { .. })).cloned()).collect();
                if apps.is_empty() {
                    return vec![RunnerOutput::Notify(tr("The compound configuration has no applications to run").into(), true)];
                }
                let mut out = Vec::new();
                for a in apps {
                    if let ConfigKind::Cargo { package, bin } = a.kind.clone() {
                        out.extend(self.run_cargo(a, package, bin));
                    }
                }
                out
            }
        }
    }

    /// The binaries of the attached projects go after the workspace's, named with their
    /// project (`gen (tools/gen)` style: `bin (folder)`).
    pub fn set_attached(&mut self, attached: Vec<rc_cargo::CargoWorkspace>) {
        self.attached = attached;
        self.insert_attached_configs();
    }

    fn insert_attached_configs(&mut self) {
        let selected = self.selected_config().map(|c| c.name.clone());
        let old = std::mem::take(&mut self.attached_configs);
        self.configs.retain(|c| !(matches!(c.kind, ConfigKind::Cargo { .. }) && old.contains(&c.name)));
        self.attached_configs = self.attached_bins().map(|(name, ..)| name).collect();
        let at = self.configs.iter().rposition(|c| matches!(c.kind, ConfigKind::Cargo { .. })).map_or(0, |i| i + 1);
        let new: Vec<RunConfig> = self.attached_bins().map(|(name, package, bin, _)| RunConfig { name, kind: ConfigKind::Cargo { package, bin } }).collect();
        for (i, c) in new.into_iter().enumerate() {
            self.configs.insert(at + i, c);
        }
        if let Some(n) = selected
            && let Some(i) = self.configs.iter().position(|c| c.name == n)
        {
            self.selected = i;
        }
    }

    /// (configuration name, package, binary, manifest) of each attached project's binaries.
    fn attached_bins(&self) -> impl Iterator<Item = (String, String, String, PathBuf)> + '_ {
        self.attached.iter().flat_map(|w| {
            let folder = w.manifest.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            w.binaries().map(move |(p, t)| (format!("{} ({folder})", t.name), p.name.clone(), t.name.clone(), w.manifest.clone()))
        })
    }

    /// The manifest an attached project's configuration builds with.
    fn cargo_manifest_of(&self, config: &str) -> Option<PathBuf> {
        self.attached_bins().find(|(name, ..)| name == config).map(|(.., manifest)| manifest)
    }

    /// The most temporary configurations kept; older ones go.
    const MAX_TEMPORARY: usize = 5;

    /// Saved and temporary test configurations go last.
    fn insert_test_configs(&mut self) {
        let selected = self.selected_config().map(|c| c.name.clone());
        self.configs.retain(|c| !matches!(c.kind, ConfigKind::TestSelection { .. }));
        for t in &self.test_configs {
            let filter = if t.class { TestFilter::Class(t.tests.first().cloned().unwrap_or_default()) } else { TestFilter::Tests(t.tests.clone()) };
            self.configs.push(RunConfig { name: t.name.clone(), kind: ConfigKind::TestSelection { target: t.target.clone(), filter } });
        }
        self.configs.extend(self.temporary.iter().cloned());
        if let Some(n) = selected
            && let Some(i) = self.configs.iter().position(|c| c.name == n)
        {
            self.selected = i;
        }
    }

    /// Selects the configuration for these tests, making a temporary one when there is none.
    pub fn select_tests(&mut self, name: String, target: PathBuf, filter: TestFilter) {
        let kind = ConfigKind::TestSelection { target, filter };
        if let Some(i) = self.configs.iter().position(|c| c.kind == kind) {
            self.selected = i;
            return;
        }
        // Another selection with the same name (in another project) gets the next free name.
        let name = (1..).map(|n| if n == 1 { name.clone() } else { format!("{name} ({n})") }).find(|n| !self.configs.iter().any(|c| &c.name == n)).unwrap_or(name);
        if self.temporary.len() >= Self::MAX_TEMPORARY {
            let oldest = self.temporary.remove(0);
            self.configs.retain(|c| *c != oldest);
        }
        let cfg = RunConfig { name, kind };
        self.temporary.push(cfg.clone());
        self.configs.push(cfg);
        self.selected = self.configs.len() - 1;
    }

    pub fn is_temporary(&self, name: &str) -> bool {
        self.temporary.iter().any(|c| c.name == name)
    }

    /// Makes a temporary test configuration permanent (kept in the session).
    pub fn save_temporary(&mut self, name: &str) {
        let Some(cfg) = self.temporary.iter().find(|c| c.name == name).cloned() else { return };
        let ConfigKind::TestSelection { target, filter } = cfg.kind else { return };
        let (tests, class) = match filter {
            TestFilter::Class(c) => (vec![c], true),
            TestFilter::Tests(t) => (t, false),
            TestFilter::Namespace(n) | TestFilter::Raw(n) => (vec![n], true),
        };
        self.temporary.retain(|c| c.name != name);
        self.test_configs.push(rc_core::session::SavedTestConfig { name: cfg.name, target, tests, class });
        self.insert_test_configs();
    }

    /// Removes a saved or temporary test configuration.
    /// The folder a configuration is in.
    pub fn folder_of(&self, name: &str) -> Option<&str> {
        self.folders.get(name).map(String::as_str)
    }

    /// The folders that hold at least one of the current configurations, sorted.
    pub fn folder_names(&self) -> Vec<String> {
        let mut out: Vec<String> = self.configs.iter().filter_map(|c| self.folders.get(&c.name).cloned()).collect();
        out.sort_by_key(|f| f.to_lowercase());
        out.dedup();
        out
    }

    /// Puts a configuration in a folder (`None`: at the top level). A name that is empty
    /// after trimming leaves it at the top level.
    pub fn move_to_folder(&mut self, name: &str, folder: Option<&str>) {
        match folder.map(str::trim).filter(|f| !f.is_empty()) {
            Some(f) => {
                self.folders.insert(name.to_string(), f.chars().take(MAX_FOLDER_CHARS).collect());
            }
            None => {
                self.folders.remove(name);
            }
        }
    }

    /// Renames a folder: its configurations go with it (into an existing folder, they merge).
    pub fn rename_folder(&mut self, old: &str, new: &str) {
        let new: String = new.trim().chars().take(MAX_FOLDER_CHARS).collect();
        if new.is_empty() {
            return;
        }
        for f in self.folders.values_mut().filter(|f| f.as_str() == old) {
            *f = new.clone();
        }
    }

    pub fn remove_test_config(&mut self, name: &str) {
        self.folders.remove(name);
        self.temporary.retain(|c| c.name != name);
        self.test_configs.retain(|t| t.name != name);
        self.configs.retain(|c| !(c.name == name && matches!(c.kind, ConfigKind::TestSelection { .. })));
        self.selected = self.selected.min(self.configs.len().saturating_sub(1));
    }

    /// Compound configurations go right after the applications (before the tests).
    fn insert_compounds(&mut self) {
        let selected = self.selected_config().map(|c| c.name.clone());
        self.configs.retain(|c| !matches!(c.kind, ConfigKind::Compound { .. }));
        let at = self.configs.iter().rposition(|c| matches!(c.kind, ConfigKind::Cargo { .. })).map_or(0, |i| i + 1);
        for (i, (name, members)) in self.compounds.iter().enumerate() {
            self.configs.insert(at + i, RunConfig { name: name.clone(), kind: ConfigKind::Compound { members: members.clone() } });
        }
        if let Some(n) = selected
            && let Some(i) = self.configs.iter().position(|c| c.name == n)
        {
            self.selected = i;
        }
    }

    /// Adds or replaces (by `old` name) a compound configuration and selects it.
    pub fn save_compound(&mut self, old: Option<&str>, name: String, members: Vec<String>) {
        match old.and_then(|o| self.compounds.iter().position(|(n, _)| n == o)) {
            Some(i) => self.compounds[i] = (name.clone(), members),
            None => self.compounds.push((name.clone(), members)),
        }
        self.insert_compounds();
        if let Some(i) = self.configs.iter().position(|c| c.name == name) {
            self.selected = i;
        }
    }

    pub fn remove_compound(&mut self, name: &str) {
        self.folders.remove(name);
        self.compounds.retain(|(n, _)| n != name);
        self.insert_compounds();
        self.selected = self.selected.min(self.configs.len().saturating_sub(1));
    }

    /// Names of the application configurations (members a compound can have).
    pub fn app_names(&self) -> Vec<String> {
        self.configs.iter().filter(|c| matches!(c.kind, ConfigKind::Cargo { .. })).map(|c| c.name.clone()).collect()
    }

    /// A binary of the Rust workspace, with the user's edits applied.
    fn cargo_target(&self, name: &str, package: String, bin: String) -> Option<rc_cargo::RunTarget> {
        let attached = self.cargo_manifest_of(name);
        let manifest = match &attached {
            Some(m) => m.clone(),
            None => self.cargo_workspace.as_ref()?.manifest.clone(),
        };
        let mut target = rc_cargo::RunTarget { before: Vec::new(), test: None, manifest, package, bin, args: Vec::new(), env: Vec::new(), working_dir: None, options: Default::default() };
        if let Some(o) = self.options.get(name) {
            // A tool that cannot start (renamed, a macro without a value) stops the run with
            // its reason.
            for tool_name in &o.before_launch {
                let plan = match self.external_tools.iter().find(|t| &t.name == tool_name) {
                    Some(tool) => crate::app::external_plan(tool, &self.macro_context, self.project_env_pairs()),
                    None => Err(trf("There is no external tool called {name}", &[("name", tool_name)])),
                };
                target.before.push(rc_cargo::BeforeLaunch { name: tool_name.clone(), plan });
            }
            target.args = rc_dotnet::process::split_command_line(&o.args);
            target.env = o.env_pairs();
            if !o.working_dir.trim().is_empty() {
                target.working_dir = Some(PathBuf::from(o.working_dir.trim()));
            }
            target.options = rc_cargo::CargoOptions {
                channel: o.channel.trim().to_string(),
                profile: o.cargo_profile.trim().to_string(),
                features: o.features.split([',', ' ']).filter(|f| !f.is_empty()).map(str::to_string).collect(),
                backtrace: o.backtrace.clone(),
                stdin_file: (!o.stdin_file.trim().is_empty()).then(|| PathBuf::from(o.stdin_file.trim())),
            };
        }
        // What the binary cannot be built without.
        let workspace = match &attached {
            Some(m) => self.attached.iter().find(|w| &w.manifest == m),
            None => self.cargo_workspace.as_ref(),
        };
        let packages = workspace.map_or(&[][..], |w| &w.packages);
        let required = packages.iter().filter(|p| p.name == target.package).flat_map(|p| &p.targets).find(|t| t.kind == rc_cargo::TargetKind::Bin && t.name == target.bin).map_or(&[][..], |t| &t.required_features);
        for feature in required {
            if !target.options.features.contains(feature) {
                target.options.features.push(feature.clone());
            }
        }
        Some(target)
    }

    /// What the debugger needs to start `config`, when it is a Rust binary.
    pub fn cargo_debug_target(&self, config: &RunConfig) -> Option<Result<(rc_cargo::Cargo, rc_cargo::RunTarget), String>> {
        let ConfigKind::Cargo { package, bin } = &config.kind else { return None };
        let target = self.cargo_target(&config.name, package.clone(), bin.clone())?;
        Some(self.cargo().map(|cargo| (cargo, target)))
    }

    /// Starts a binary of the Rust workspace, with the user's edits applied.
    fn run_cargo(&mut self, cfg: RunConfig, package: String, bin: String) -> Vec<RunnerOutput> {
        let cargo = match self.cargo() {
            Ok(c) => c,
            Err(e) => return vec![RunnerOutput::Notify(e, true)],
        };
        let Some(target) = self.cargo_target(&cfg.name, package, bin) else { return Vec::new() };
        let tab = self.free_tab(&cfg);
        let inbox = Inbox::new(self.waker.clone());
        let console = Console::new(&self.waker);
        console.note(&trf("Building {name}…", &[("name", &cfg.name)]));
        if self.options.get(&cfg.name).is_some_and(|o| o.emulate_terminal) {
            let prep_inbox = Inbox::new(self.waker.clone());
            let prepare = rc_cargo::start_prepare(&cargo, target, prep_inbox.sender());
            let pty = PtyRun { prepare: Some(prepare), inbox: prep_inbox, started: false };
            self.runs.push(RunTab { name: tab, config: cfg, handle: None, inbox, console, exit: None, urls: Vec::new(), pty: Some(pty) });
            self.active_run = self.runs.len() - 1;
            return vec![RunnerOutput::ShowTool(crate::toolwindow::ToolWindow::Run)];
        }
        let handle = rc_cargo::start_run(&cargo, target, inbox.sender());
        self.runs.push(RunTab { name: tab, config: cfg, handle: Some(handle), inbox, console, exit: None, urls: Vec::new(), pty: None });
        self.active_run = self.runs.len() - 1;
        vec![RunnerOutput::ShowTool(crate::toolwindow::ToolWindow::Run)]
    }

    /// Makes room for a new run of `cfg` and returns its tab's name. Running a configuration
    /// again restarts it (Rider's "Rerun"), unless it allows several instances: then the
    /// finished tabs go and the new one is numbered (`server (2)`).
    fn free_tab(&mut self, cfg: &RunConfig) -> String {
        if !self.options.get(&cfg.name).is_some_and(|o| o.allow_parallel) {
            if let Some(i) = self.runs.iter().position(|r| r.config == *cfg) {
                self.runs.remove(i).stop();
            }
            return cfg.name.clone();
        }
        self.runs.retain(|r| r.config != *cfg || r.running());
        let taken: Vec<&str> = self.runs.iter().filter(|r| r.config == *cfg).map(|r| r.name.as_str()).collect();
        if !taken.contains(&cfg.name.as_str()) {
            return cfg.name.clone();
        }
        (2..).map(|n| format!("{} ({n})", cfg.name)).find(|n| !taken.contains(&n.as_str())).unwrap_or_else(|| cfg.name.clone())
    }

    fn run_wasm_pack(&mut self, config: RunConfig, manifest: &Path) -> Vec<RunnerOutput> {
        if self.cargo.as_ref().is_some_and(rc_cargo::Cargo::is_criticalup){return vec![RunnerOutput::Notify(crate::criticalup_provider::unsupported(),true)];}

        let trusted = self.cargo_workspace.as_ref().is_some_and(|w| self.trusted_project.as_ref() == Some(&w.root) && w.packages.iter().any(|p| p.manifest == manifest));
        if !trusted {
            return vec![RunnerOutput::Notify(tr("Trust this Cargo project before running wasm-pack").into(), true)];
        }
        let options = self.options.get(&config.name).cloned().unwrap_or_default();
        match wasm_pack::plan(manifest, &options, self.project_env_pairs()) {
            Ok(plan) => self.run_process(config, plan),
            Err(error) => vec![RunnerOutput::Notify(error.to_string(), true)],
        }
    }

    /// Starts an external tool in a tab of the Run window; running it again restarts the tab.
    pub fn run_external(&mut self, tool: &str, plan: rc_dotnet::LaunchPlan) -> Vec<RunnerOutput> {
        if let Err(error) = self.require_trust() { return vec![RunnerOutput::Notify(error.to_string(), true)]; }
        self.run_process(RunConfig { name: tool.to_string(), kind: ConfigKind::External { tool: tool.to_string() } }, plan)
    }

    /// Builds a Rust scratch file with `rustc` into a temporary folder and runs it.
    pub fn run_scratch(&mut self, file: &Path) -> Vec<RunnerOutput> {
        if let Err(error) = self.require_trust() { return vec![RunnerOutput::Notify(error.to_string(), true)]; }
        let cargo = match self.cargo.clone().or_else(rc_cargo::Cargo::locate) {
            Some(c) => c.with_env(self.project_env_pairs()),
            None => return vec![RunnerOutput::Notify(tr("Cargo was not found. Install Rust with rustup.").into(), true)],
        };
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let cfg = RunConfig { name: name.clone(), kind: ConfigKind::Scratch { file: file.to_path_buf() } };
        let tab = self.free_tab(&cfg);
        let out_dir = rc_core::paths::data_dir().join("scratch-builds");
        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            return vec![RunnerOutput::Notify(e.to_string(), true)];
        }
        let stem = file.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "scratch".into());
        let out = out_dir.join(if cfg!(windows) { format!("{stem}.exe") } else { stem });
        let inbox = Inbox::new(self.waker.clone());
        let console = Console::new(&self.waker);
        let handle = rc_cargo::start_scratch(&cargo, file.to_path_buf(), out, inbox.sender());
        self.runs.push(RunTab { name: tab, config: cfg, handle: Some(handle), inbox, console, exit: None, urls: Vec::new(), pty: None });
        self.active_run = self.runs.len() - 1;
        vec![RunnerOutput::ShowTool(crate::toolwindow::ToolWindow::Run)]
    }

    /// Runs a command of the IDE (`rustup update`…) in a tab of the Run window.
    pub fn run_command(&mut self, program: PathBuf, args: Vec<String>) -> Vec<RunnerOutput> {
        let name = std::iter::once(program.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()).chain(args.iter().cloned()).collect::<Vec<_>>().join(" ");
        let plan = self.command_plan(program.clone(), args.clone());
        self.run_process(RunConfig { name, kind: ConfigKind::Command { program, args } }, plan)
    }

    /// IDE commands run in the home folder: a project's `rust-toolchain` file must not change
    /// which toolchain `rustup update` sees.
    fn command_plan(&self, program: PathBuf, args: Vec<String>) -> rc_dotnet::LaunchPlan {
        let dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        rc_dotnet::LaunchPlan { program, args, env: Vec::new(), working_dir: dir, urls: Vec::new(), profile: None, browser_url: None }
    }

    fn run_process(&mut self, cfg: RunConfig, plan: rc_dotnet::LaunchPlan) -> Vec<RunnerOutput> {
        if let Some(i) = self.runs.iter().position(|r| r.config == cfg) {
            self.runs.remove(i).stop();
        }
        let inbox = Inbox::new(self.waker.clone());
        let console = Console::new(&self.waker);
        let handle = rc_cargo::start_process(plan, inbox.sender());
        self.runs.push(RunTab { name: cfg.name.clone(), config: cfg, handle: Some(handle), inbox, console, exit: None, urls: Vec::new(), pty: None });
        self.active_run = self.runs.len() - 1;
        vec![RunnerOutput::ShowTool(crate::toolwindow::ToolWindow::Run)]
    }

    pub fn stop_all(&mut self) {
        for r in &mut self.runs {
            r.stop();
        }
        self.cancel_build();
        if let Some(t) = &mut self.tests
            && let Some(h) = t.handle.take()
        {
            h.cancel();
        }
    }

    pub fn close_run(&mut self, i: usize) {
        if i < self.runs.len() {
            self.runs.remove(i).stop();
            self.active_run = self.active_run.min(self.runs.len().saturating_sub(1));
        }
    }

    // ------------------------------------------------------------------ tests

    pub fn run_tests(&mut self, target: &Path, filter: Option<TestFilter>) -> Vec<RunnerOutput> {
        if self.nextest&&self.cargo.as_ref().is_some_and(rc_cargo::Cargo::is_criticalup){return vec![RunnerOutput::Notify(crate::criticalup_provider::unsupported(),true)];}
        if let Err(error) = self.require_trust() { return vec![RunnerOutput::Notify(error.to_string(), true)]; }
        let target = self.cargo_workspace.as_ref().map_or(target, |workspace| workspace.manifest.as_path());
        if let Some(t) = &mut self.tests
            && let Some(h) = t.handle.take()
        {
            h.cancel();
        }
        let mut req = TestRequest::new(target);
        req.filter = filter;
        let inbox = Inbox::new(self.waker.clone());
        let console = Console::new(&self.waker);
        let started = self.cargo().map(|cargo| rc_cargo::start_tests(&cargo, req.clone(), self.nextest, inbox.sender()));
        let handle = match started {
            Ok(h) => h,
            Err(e) => return vec![RunnerOutput::Notify(e, true)],
        };
        let show_passed = self.tests.as_ref().is_none_or(|t| t.show_passed);
        let filter = self.tests.as_ref().map(|t| t.filter.clone()).unwrap_or_default();
        // Tests about to run show a running icon in the gutter.
        if let Some(TestFilter::Tests(names)) = &req.filter {
            for n in names {
                self.test_outcomes.insert(n.clone(), TestState::Running);
            }
        }
        self.tests = Some(TestsState {
            handle: Some(handle),
            inbox,
            console,
            results: BTreeMap::new(),
            running: true,
            passed: 0,
            failed: 0,
            skipped: 0,
            duration: None,
            failure: None,
            request: req,
            selected: None,
            show_passed,
            filter,
        });
        vec![RunnerOutput::ShowTool(crate::toolwindow::ToolWindow::Tests)]
    }

    /// Runs all tests of the Cargo workspace.
    pub fn run_all_tests(&mut self, workspace: &Path) -> Vec<RunnerOutput> {
        let target = self.cargo_workspace.as_ref().map_or(workspace, |cargo| cargo.manifest.as_path()).to_path_buf();
        self.run_tests(&target, None)
    }

    /// Runs the tests that failed in the last run.
    pub fn rerun_failed(&mut self) -> Vec<RunnerOutput> {
        let Some(t) = &self.tests else { return Vec::new() };
        let failed: Vec<String> = t.results.iter().filter(|(_, r)| r.iter().any(|x| x.outcome == TestOutcome::Failed)).map(|(k, _)| k.clone()).collect();
        if failed.is_empty() {
            return vec![RunnerOutput::Notify(tr("No failed tests to run again").into(), false)];
        }
        let target = t.request.target.clone();
        self.run_tests(&target, Some(TestFilter::Tests(failed)))
    }

    pub fn rerun_tests(&mut self) -> Vec<RunnerOutput> {
        let Some(t) = &self.tests else { return Vec::new() };
        let (target, filter) = (t.request.target.clone(), t.request.filter.clone());
        self.run_tests(&target, filter)
    }

    // ------------------------------------------------------------------ per frame

    pub fn update(&mut self) -> Vec<RunnerOutput> {
        let mut out = Vec::new();
        if let Some(b) = &mut self.build {
            for ev in b.inbox.drain() {
                match ev {
                    BuildEvent::Started { command } => b.console.note(&command),
                    BuildEvent::Output(line) | BuildEvent::Task(line) => b.console.line(&line),
                    BuildEvent::Diagnostic(d) => b.diagnostics.push(d),
                    BuildEvent::Finished(r) => {
                        b.handle = None;
                        let msg = if b.command { command_summary(&b.title, &r) } else { build_summary(&r) };
                        b.console.note(&msg);
                        out.push(RunnerOutput::Notify(msg, !r.success && !r.cancelled));
                        out.push(RunnerOutput::BuildDiagnostics(r.diagnostics.clone()));
                        b.diagnostics = r.diagnostics.clone();
                        b.result = Some(r);
                    }
                    BuildEvent::Failed(e) => {
                        b.handle = None;
                        b.console.note(&e);
                        out.push(RunnerOutput::Notify(e.clone(), true));
                        b.failed = Some(e);
                    }
                }
            }
        }
        let waker = self.waker.clone();
        for r in &mut self.runs {
            r.update_pty(&waker, &mut out);
        }
        for r in &mut self.runs {
            for ev in r.inbox.drain() {
                match ev {
                    RunEvent::Build(BuildEvent::Diagnostic(d)) => {
                        if d.severity == rc_dotnet::Severity::Error {
                            r.console.note(&format!("{}({},{}): {}", d.file.as_ref().map(|f| f.display().to_string()).unwrap_or_default(), d.line.unwrap_or(0), d.column.unwrap_or(0), d.message));
                        }
                    }
                    RunEvent::Build(BuildEvent::Finished(res)) if !res.success => {
                        out.push(RunnerOutput::BuildDiagnostics(res.diagnostics.clone()));
                    }
                    RunEvent::Build(BuildEvent::Task(line)) => r.console.line(&line),
                    RunEvent::Build(_) => {}
                    RunEvent::Started { plan, .. } => {
                        r.console.note(&plan.command_line());
                        if !plan.urls.is_empty() {
                            r.console.note(&trf("Listening on {urls}", &[("urls", &plan.urls.join(", "))]));
                        }
                        r.urls = plan.urls.clone();
                    }
                    RunEvent::Output(line) => r.console.line(&line),
                    RunEvent::Exited { code, cancelled, .. } => {
                        r.console.note(&match (cancelled, code) {
                            (true, _) => tr("Process stopped").to_string(),
                            (false, Some(c)) => trf("Process finished with exit code {code}", &[("code", &c)]),
                            (false, None) => tr("Process finished").to_string(),
                        });
                        r.exit = Some(code);
                        r.handle = None;
                    }
                    RunEvent::Failed(e) => {
                        r.console.note(&e);
                        r.exit = Some(None);
                        r.handle = None;
                        out.push(RunnerOutput::Notify(e, true));
                    }
                }
            }
        }
        if let Some(t) = &mut self.tests {
            for ev in t.inbox.drain() {
                match ev {
                    TestEvent::Build(BuildEvent::Output(line)) => t.console.line(&line),
                    TestEvent::Build(BuildEvent::Finished(res)) if !res.success => {
                        t.console.note(&build_summary(&res));
                        out.push(RunnerOutput::BuildDiagnostics(res.diagnostics.clone()));
                    }
                    TestEvent::Build(_) => {}
                    TestEvent::ProjectStarted { project } => t.console.note(&trf("Running tests of {project}…", &[("project", &project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())])),
                    TestEvent::Output { line, .. } => t.console.line(&line),
                    TestEvent::Progress(p) => match p.outcome {
                        TestOutcome::Passed => t.passed += 1,
                        TestOutcome::Failed => t.failed += 1,
                        TestOutcome::Skipped => t.skipped += 1,
                    },
                    TestEvent::ProjectFinished(run) => {
                        for r in run.results {
                            t.results.entry(r.fully_qualified_name.clone()).or_default().push(r);
                        }
                    }
                    TestEvent::Finished(res) => {
                        t.running = false;
                        t.handle = None;
                        t.passed = res.passed;
                        t.failed = res.failed;
                        t.skipped = res.skipped;
                        t.duration = Some(res.duration);
                        t.results.clear();
                        for r in res.results {
                            t.results.entry(r.fully_qualified_name.clone()).or_default().push(r);
                        }
                        let msg = if res.cancelled {
                            tr("Tests stopped").to_string()
                        } else if res.failed > 0 {
                            trf("Tests failed: {failed}, passed: {passed}", &[("failed", &res.failed), ("passed", &res.passed)])
                        } else if res.passed + res.skipped == 0 {
                            tr("No tests were run").to_string()
                        } else {
                            trf("Tests passed: {passed}", &[("passed", &res.passed)])
                        };
                        t.console.note(&msg);
                        out.push(RunnerOutput::Notify(msg, res.failed > 0));
                        if t.selected.is_none() {
                            t.selected = t.results.iter().find(|(_, r)| r.iter().any(|x| x.outcome == TestOutcome::Failed)).map(|(k, _)| k.clone());
                        }
                    }
                    TestEvent::Failed(e) => {
                        t.running = false;
                        t.handle = None;
                        t.console.note(&e);
                        t.failure = Some(e.clone());
                        out.push(RunnerOutput::Notify(e, true));
                    }
                }
            }
            if !t.running {
                // The gutter remembers the last outcome of every test that ran.
                self.test_failures.clear();
                for result in t.results.values().flatten().filter(|r| r.outcome == TestOutcome::Failed) {
                    let frame = result.stack_trace.iter().find_map(|f| Some((f.file.clone()?, f.line?)));
                    if let Some(((file, line), message)) = frame.zip(result.error_message.as_deref()) {
                        self.test_failures.entry(file).or_default().insert(line.saturating_sub(1) as usize, failure_note(message));
                    }
                }
                for (name, results) in &t.results {
                    let state = if results.iter().any(|r| r.outcome == TestOutcome::Failed) {
                        TestState::Failed
                    } else if results.iter().all(|r| r.outcome == TestOutcome::Skipped) {
                        TestState::Skipped
                    } else {
                        TestState::Passed
                    };
                    self.test_outcomes.insert(name.clone(), state);
                }
                self.test_outcomes.retain(|_, s| *s != TestState::Running);
            }
        }
        out
    }
}

/// The sentence of a test's failure: what follows `thread 'x' panicked at src/lib.rs:1:2:`.
fn failure_note(message: &str) -> String {
    let after = message.lines().skip_while(|l| !l.contains(" panicked at ")).skip(1);
    let mut words: Vec<&str> = after.take_while(|l| !l.is_empty() && !l.starts_with("note:")).map(str::trim).collect();
    if words.is_empty() {
        words.extend(message.lines().next());
    }
    words.join("  ").chars().take(160).collect()
}

/// How a command of the Cargo window ended.
pub fn command_summary(title: &str, r: &BuildResult) -> String {
    let secs = format!("{:.1}", r.duration.as_secs_f32());
    if r.cancelled {
        trf("{title} cancelled", &[("title", &title)])
    } else if r.success {
        trf("{title} finished in {secs} s", &[("title", &title), ("secs", &secs)])
    } else {
        let code = r.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
        trf("{title} failed with exit code {code}", &[("title", &title), ("code", &code)])
    }
}

fn build_summary(r: &BuildResult) -> String {
    let secs = format!("{:.1}", r.duration.as_secs_f32());
    if r.cancelled {
        tr("Build cancelled").to_string()
    } else if r.success {
        trf("Build succeeded with {warnings} warnings in {secs} s", &[("warnings", &r.warnings), ("secs", &secs)])
    } else {
        trf("Build failed: {errors} errors, {warnings} warnings", &[("errors", &r.errors), ("warnings", &r.warnings)])
    }
}

/// A test (or test class) declared in a file, found with the syntax tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestMarker {
    pub line: usize,
    /// `Namespace.Class.Method`, or `Namespace.Class` for a class.
    pub name: String,
    pub is_class: bool,
}
