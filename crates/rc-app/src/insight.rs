//! Code insight for C# through the Roslyn language server: starting/downloading the server,
//! document sync, diagnostics, completion, hover, parameter info, navigation, rename, context
//! actions, formatting, semantic colours, inlay hints and the file structure.
//!
//! Requests never block: each one is a `Pending` polled once per frame.

use crate::convert;
use crate::docs::{Doc, Docs};
use rc_core::{Inbox, Runtime, Waker};
use rc_editor::{Completion, EditorEvent, Hover, Signature, SignatureHelp};
use rc_kit::i18n::{tr, trf};
use rc_lsp::lsp_types as lsp;
use rc_lsp::roslyn::{self, DownloadEvent};
use rc_lsp::{LanguageSession, LspEvent, Pending, ServerState, path_to_uri, uri_to_path};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What the status bar shows about C# support.
#[derive(Clone, Debug, PartialEq)]
pub enum InsightStatus {
    /// No C# in the workspace, or not started yet.
    Idle,
    /// No server installed: the status bar offers the download.
    NotInstalled,
    /// The project is not trusted yet, so its code must not run.
    Untrusted,
    /// rust-analyzer is missing; it comes with the toolchain.
    RustAnalyzerMissing,
    Downloading { done: u64, total: Option<u64> },
    Starting,
    Loading(String),
    Ready,
    Failed(String),
}

/// Where a navigation request was made: (path, line, word start col, word end col).
type NavOrigin = (PathBuf, usize, usize, usize);

/// A navigation target.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    /// The line's text, for result lists.
    pub preview: String,
}

/// Results the UI shows outside the editor.
#[derive(Clone, Debug)]
pub enum InsightOutput {
    /// A web page to open.
    OpenUrl(String),
    /// The expansion of the macro call under the caret.
    MacroExpansion { name: String, text: String },
    /// The call tree is ready to show.
    ShowCallTree,
    /// Jump to this place (single result of go to declaration / implementation…).
    Navigate(Target),
    /// Several results: show them in a chooser / the Find window.
    Usages { title: String, targets: Vec<Target> },
    /// Context actions (Alt+Enter) for the active editor.
    Actions(Vec<lsp::CodeActionOrCommand>),
    /// Rename: the symbol's current name (from prepareRename), to ask for the new one.
    AskRename { doc: u64, pos: usize, name: String },
    Message(String),
    /// Reformatting on save changed a document, which must be written again.
    SaveFormatted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKind {
    Declaration,
    Implementation,
    TypeDeclaration,
    /// From an implementation to what it implements.
    Super,
    Usages,
}

fn action_title(action: &lsp::CodeActionOrCommand) -> &str {
    match action {
        lsp::CodeActionOrCommand::Command(c) => &c.title,
        lsp::CodeActionOrCommand::CodeAction(c) => &c.title,
    }
}

/// Whether a code action belongs in the Generate popup.
fn generates_code(title: &str) -> bool {
    ["Generate", "Implement", "Add missing"].iter().any(|start| title.starts_with(start))
}

/// The command of the Rename row of the Refactor This popup, run by the IDE.
pub const RENAME_COMMAND: &str = "crabcraft.rename";
/// The rows of the Select In popup, run by the IDE.
pub const SELECT_IN_EXPLORER: &str = "crabcraft.select_in_explorer";
pub const REVEAL_FILE: &str = "crabcraft.reveal_file";
/// The Derive row of the Generate popup, run by the IDE.
pub const DERIVE_COMMAND: &str = "crabcraft.derive";
/// The Suppress rows of Alt+Enter (arguments: the lint, then `item` or `file`), run by the IDE.
pub const SUPPRESS_COMMAND: &str = "crabcraft.suppress";

/// The Refactor This popup: Rename first, then the server's refactorings.
fn refactorings(list: Vec<lsp::CodeActionOrCommand>) -> Vec<lsp::CodeActionOrCommand> {
    let rename = lsp::Command { title: tr("Rename…").to_string(), command: RENAME_COMMAND.to_string(), arguments: None };
    let found = list.into_iter().filter(|a| matches!(a, lsp::CodeActionOrCommand::CodeAction(c) if c.kind.as_ref().is_some_and(|k| k.as_str().starts_with("refactor"))));
    std::iter::once(lsp::CodeActionOrCommand::Command(rename)).chain(found).collect()
}

/// Document structure entry (Structure window, ⌘F12).
#[derive(Clone, Debug)]
pub struct SymbolNode {
    pub name: String,
    pub detail: String,
    pub kind: lsp::SymbolKind,
    pub line: usize,
    pub col: usize,
    /// The lines the whole item covers, with its attributes and body.
    pub lines: std::ops::RangeInclusive<usize>,
    pub children: Vec<SymbolNode>,
}

/// The chain of symbols around `line`, from the outermost one.
pub fn symbols_at(nodes: &[SymbolNode], line: usize) -> Vec<&SymbolNode> {
    let mut path = Vec::new();
    let mut level = nodes;
    while let Some(node) = level.iter().find(|n| n.lines.contains(&line)) {
        path.push(node);
        level = &node.children;
    }
    path
}

struct CompletionRequest {
    doc: u64,
    anchor: usize,
    version: u64,
    pending: Pending<Value>,
}

pub struct Insight {
    pub session: Option<LanguageSession>,
    pub status: InsightStatus,
    downloads: Inbox<DownloadEvent>,
    waker: Waker,
    root: Option<PathBuf>,
    solution: Option<PathBuf>,
    projects: Vec<PathBuf>,
    dotnet: PathBuf,
    /// The open workspace is a Rust one: rust-analyzer instead of Roslyn.
    rust: bool,
    clippy: bool,
    /// Folders and files rust-analyzer leaves out (root-relative, `/`-separated).
    exclude: Vec<String>,
    /// The saved document being reformatted, which is saved again when its edits apply.
    format_on_save: Option<u64>,
    /// Reformatted on save, waiting to be written.
    save_again: Option<u64>,
    configured: Option<PathBuf>,
    /// The project's environment, given to rust-analyzer.
    env: Vec<(String, String)>,
    /// Diagnostics per file, for the Problems window (also closed files the server reports).
    pub problems: BTreeMap<PathBuf, Vec<rc_editor::Diagnostic>>,
    /// The text the positions of `problems` refer to, for files that are not open.
    pub problem_text: BTreeMap<PathBuf, rc_text::Rope>,
    raw_items: Vec<lsp::CompletionItem>,
    completion: Option<CompletionRequest>,
    /// The completion item whose documentation was asked for (`None` pending: resolved).
    resolve: Option<(usize, Option<Pending<lsp::CompletionItem>>)>,
    hover: Option<(u64, usize, egui::Pos2, Pending<Option<lsp::Hover>>)>,
    signature: Option<(u64, usize, Pending<Option<lsp::SignatureHelp>>)>,
    /// The `bool`: a single usage is jumped to (⌘+click on a declaration) instead of listed.
    nav: Option<(NavKind, NavOrigin, bool, Pending<Value>)>,
    /// Callers (`true`) or callees of what is under the caret, in two requests.
    external_docs: Option<Pending<Value>>,
    expand_macro: Option<Pending<Value>>,
    types: Option<(Target, Pending<Value>)>,
    /// The first step of a call hierarchy.
    calls: Option<(bool, Pending<Value>)>,
    /// The levels being loaded, with the tree they belong to.
    call_levels: Vec<(u64, Vec<usize>, Pending<Value>)>,
    call_generation: u64,
    pub call_tree: Option<crate::call_tree::CallTree>,
    /// The pending code actions are for Alt+Insert.
    generate_only: bool,
    refactor_only: bool,
    /// A navigation asked for while the projects were still loading: repeated once they are.
    nav_retry: Option<(NavKind, NavOrigin)>,
    rename_prepare: Option<(u64, usize, Pending<Value>)>,
    rename: Option<Pending<Option<lsp::WorkspaceEdit>>>,
    actions: Option<Pending<Option<lsp::CodeActionResponse>>>,
    action_resolve: Option<Pending<lsp::CodeAction>>,
    optimizing: Option<Optimizing>,
    /// Imports of an accepted completion: document, and where the inserted text starts.
    accepted_resolve: Option<(u64, usize, Pending<lsp::CompletionItem>)>,
    /// Where the last list of items starts.
    completion_anchor: usize,
    /// Formatting of (document, version).
    format: Option<(u64, Versioned<Option<Vec<lsp::TextEdit>>>)>,
    /// Formatting after typing `;` or `}` (document, version).
    type_format: Option<(u64, Versioned<Option<Vec<lsp::TextEdit>>>)>,
    /// `///` doc comment skeletons and their continuation on Enter (document, version).
    auto_insert: Option<(u64, Versioned<Value>)>,
    /// Add Missing Usings: the fixes asked for each unresolved name (document, version).
    usings: Option<UsingsRequest>,
    /// Documents to import usings for once their diagnostics catch up (after a paste).
    auto_usings: HashMap<u64, u64>,
    semantic: HashMap<u64, Versioned<Option<lsp::SemanticTokensResult>>>,
    inlay: HashMap<u64, Versioned<Option<Vec<lsp::InlayHint>>>>,
    runnables_request: HashMap<u64, Versioned<Value>>,
    parent_request: HashMap<u64, Versioned<Value>>,
    /// Rust documents no module declares, so the server cannot analyze them.
    pub unlinked: std::collections::HashSet<u64>,
    /// What can be run in each Rust document, with the text version it describes.
    pub runnables: HashMap<u64, (u64, Vec<rc_lsp::rust_analyzer::Runnable>)>,
    diag_pull: HashMap<u64, (u64, Pending<Value>)>,
    symbols: HashMap<u64, (u64, Pending<Option<lsp::DocumentSymbolResponse>>)>,
    /// Document structure of each open document.
    pub structure: HashMap<u64, Vec<SymbolNode>>,
    /// Last edit time per document, to debounce refreshes.
    dirty: HashMap<u64, Instant>,
    /// Version each refresh was last requested for.
    refreshed: HashMap<u64, u64>,
    pub last_completion_ms: Option<u128>,
    pending_apply: Vec<(Value, lsp::WorkspaceEdit)>,
}

/// Add Missing Usings in flight: document, version, fixes still pending, fixes received.
type UsingsRequest = (u64, u64, Vec<Pending<Value>>, Vec<Value>);

/// A request made for one version of a document.
type Versioned<T> = (u64, Pending<T>);

const REFRESH_DELAY: Duration = Duration::from_millis(250);

impl Insight {
    pub fn new(waker: Waker) -> Self {
        Insight {
            session: None,
            status: InsightStatus::Idle,
            downloads: Inbox::new(waker.clone()),
            waker,
            root: None,
            solution: None,
            projects: Vec::new(),
            dotnet: PathBuf::from("dotnet"),
            rust: false,
            configured: None,
            env: Vec::new(),
            problems: BTreeMap::new(),
            problem_text: BTreeMap::new(),
            raw_items: Vec::new(),
            completion: None,
            resolve: None,
            hover: None,
            signature: None,
            nav: None,
            calls: None,
            types: None,
            external_docs: None,
            expand_macro: None,
            call_levels: Vec::new(),
            call_generation: 0,
            call_tree: None,
            generate_only: false,
            refactor_only: false,
            nav_retry: None,
            rename_prepare: None,
            rename: None,
            actions: None,
            action_resolve: None,
            optimizing: None,
            accepted_resolve: None,
            clippy: false,
            exclude: Vec::new(),
            format_on_save: None,
            save_again: None,
            completion_anchor: 0,
            format: None,
            type_format: None,
            auto_insert: None,
            usings: None,
            auto_usings: HashMap::new(),
            semantic: HashMap::new(),
            inlay: HashMap::new(),
            runnables_request: HashMap::new(),
            parent_request: HashMap::new(),
            unlinked: std::collections::HashSet::new(),
            runnables: HashMap::new(),
            diag_pull: HashMap::new(),
            symbols: HashMap::new(),
            structure: HashMap::new(),
            dirty: HashMap::new(),
            refreshed: HashMap::new(),
            last_completion_ms: None,
            pending_apply: Vec::new(),
        }
    }

    pub fn ready(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.is_initialized())
    }

    /// Starts C# support for a workspace (does nothing without C# projects).
    pub fn start(&mut self, root: &Path, solution: Option<&Path>, projects: Vec<PathBuf>, dotnet: PathBuf, configured: Option<PathBuf>) {
        self.stop();
        self.rust = false;
        self.root = Some(root.to_path_buf());
        self.solution = solution.map(Path::to_path_buf);
        self.projects = projects;
        self.dotnet = dotnet;
        self.configured = configured;
        if self.solution.is_none() && self.projects.is_empty() {
            self.status = InsightStatus::Idle;
            return;
        }
        self.launch();
    }

    /// Starts rust-analyzer for a trusted project.
    pub fn start_rust(&mut self, root: &Path, configured: Option<PathBuf>, clippy: bool) {
        self.stop();
        self.root = Some(root.to_path_buf());
        self.rust = true;
        self.clippy = clippy;
        self.configured = configured;
        self.launch();
    }

    /// Sets the project's environment; a running rust-analyzer restarts to take it.
    pub fn set_env(&mut self, env: Vec<(String, String)>) {
        if self.env == env {
            return;
        }
        self.env = env;
        if self.rust && self.session.is_some() {
            self.restart();
        }
    }

    /// Switches the linter that runs on save between Clippy and `cargo check`.
    pub fn set_clippy(&mut self, clippy: bool) {
        self.clippy = clippy;
        if let Some(s) = &mut self.session {
            s.set_clippy(clippy);
        }
    }

    /// The folders and files rust-analyzer leaves out (`files.exclude`). A running server is
    /// restarted: rust-analyzer 1.99 keeps its file set after `workspace/didChangeConfiguration`.
    pub fn set_exclude(&mut self, exclude: Vec<String>) {
        if exclude == self.exclude {
            return;
        }
        self.exclude = exclude;
        if self.rust && self.session.is_some() {
            self.restart();
        }
    }

    /// Stays off until the user trusts `root`.
    pub fn wait_for_trust(&mut self, root: &Path) {
        self.stop();
        self.root = Some(root.to_path_buf());
        self.rust = true;
        self.status = InsightStatus::Untrusted;
    }

    fn launch(&mut self) {
        let Some(root) = self.root.clone() else { return };
        if self.rust {
            let Some(program) = rc_cargo::toolchain::locate_rust_analyzer(self.configured.as_deref()) else {
                self.status = InsightStatus::RustAnalyzerMissing;
                return;
            };
            match LanguageSession::start_rust(&program, &root, self.clippy, self.exclude.clone(), &self.env, self.waker.clone()) {
                Ok(s) => {
                    self.session = Some(s);
                    self.status = InsightStatus::Starting;
                }
                Err(e) => self.status = InsightStatus::Failed(e.to_string()),
            }
            return;
        }
        match roslyn::locate(self.configured.as_deref(), &self.dotnet) {
            None => self.status = InsightStatus::NotInstalled,
            Some(cmd) => {
                let dotnet_root = self.dotnet.canonicalize().ok().and_then(|p| p.parent().map(Path::to_path_buf));
                match LanguageSession::start(&cmd, &root, self.solution.as_deref(), self.projects.clone(), dotnet_root.as_deref(), self.waker.clone()) {
                    Ok(s) => {
                        self.session = Some(s);
                        self.status = InsightStatus::Starting;
                    }
                    Err(e) => self.status = InsightStatus::Failed(e.to_string()),
                }
            }
        }
    }

    /// Downloads the server (status bar button), then starts it.
    pub fn download(&mut self) {
        if matches!(self.status, InsightStatus::Downloading { .. }) {
            return;
        }
        self.status = InsightStatus::Downloading { done: 0, total: None };
        let out = self.downloads.sender();
        Runtime::spawn(roslyn::download(out));
    }

    pub fn restart(&mut self) {
        if let Some(mut s) = self.session.take() {
            s.stop();
        }
        self.reset_requests();
        self.launch();
    }

    pub fn stop(&mut self) {
        if let Some(mut s) = self.session.take() {
            s.stop();
        }
        self.reset_requests();
        self.problems.clear();
        self.problem_text.clear();
        self.structure.clear();
        self.call_tree = None;
        self.call_levels.clear();
        self.status = InsightStatus::Idle;
    }

    fn reset_requests(&mut self) {
        self.completion = None;
        self.resolve = None;
        self.hover = None;
        self.signature = None;
        self.nav = None;
        self.calls = None;
        self.nav_retry = None;
        self.rename_prepare = None;
        self.rename = None;
        self.actions = None;
        self.action_resolve = None;
        self.optimizing = None;
        self.accepted_resolve = None;
        self.format = None;
        self.type_format = None;
        self.auto_insert = None;
        self.usings = None;
        self.auto_usings.clear();
        self.semantic.clear();
        self.inlay.clear();
        self.runnables_request.clear();
        self.parent_request.clear();
        self.unlinked.clear();
        self.runnables.clear();
        self.diag_pull.clear();
        self.symbols.clear();
        self.refreshed.clear();
    }

    // ------------------------------------------------------------------ per frame

    /// Pumps the server, syncs documents and applies finished requests. Returns what the
    /// rest of the UI must show.
    pub fn update(&mut self, docs: &mut Docs, show_inlays: bool) -> Vec<InsightOutput> {
        let mut out = Vec::new();
        for ev in self.downloads.drain() {
            match ev {
                DownloadEvent::Progress { done, total } => self.status = InsightStatus::Downloading { done, total },
                DownloadEvent::Done(_) => self.launch(),
                DownloadEvent::Failed(e) => self.status = InsightStatus::Failed(trf("Could not download the C# language server: {error}", &[("error", &e)])),
            }
        }
        let Some(session) = &mut self.session else { return out };
        let mut retry = None;
        for ev in session.pump() {
            match ev {
                LspEvent::StateChanged(ServerState::LoadingProjects) => self.status = InsightStatus::Loading(tr("Loading projects…").into()),
                LspEvent::StateChanged(ServerState::Ready) => {
                    self.status = InsightStatus::Ready;
                    retry = self.nav_retry.take();
                    // Everything is loaded: colours and problems become accurate now.
                    self.refreshed.clear();
                }
                LspEvent::StateChanged(ServerState::Failed(why)) => self.status = InsightStatus::Failed(why),
                LspEvent::StateChanged(_) => {}
                LspEvent::Progress { title, message, done, .. } => {
                    if !done && !matches!(self.status, InsightStatus::Ready) {
                        let text = match message {
                            Some(m) if !m.is_empty() => format!("{title} {m}"),
                            _ => title,
                        };
                        if !text.trim().is_empty() {
                            self.status = InsightStatus::Loading(text);
                        }
                    }
                }
                LspEvent::Diagnostics { path, diagnostics, .. } => {
                    let doc = docs.list.iter_mut().find(|d| d.path == path);
                    let converted: Vec<rc_editor::Diagnostic> = match &doc {
                        Some(d) => diagnostics.iter().map(|x| convert::diagnostic(d.editor.buffer.rope(), x)).collect(),
                        None => match std::fs::read_to_string(&path) {
                            Ok(text) => {
                                let rope = rc_text::Rope::from_str(&text.replace("\r\n", "\n"));
                                let list = diagnostics.iter().map(|x| convert::diagnostic(&rope, x)).collect();
                                self.problem_text.insert(path.clone(), rope);
                                list
                            }
                            Err(_) => Vec::new(),
                        },
                    };
                    if let Some(d) = doc {
                        d.editor.set_diagnostics(converted.clone());
                    }
                    self.store_problems(path, converted);
                }
                LspEvent::ApplyEdit { id, edit } => self.pending_apply.push((id, edit)),
                LspEvent::Refresh(_) => self.refreshed.clear(),
                LspEvent::ShowMessage { error: true, text } => out.push(InsightOutput::Message(text)),
                _ => {}
            }
        }
        if let Some((kind, (path, line, from, _))) = retry
            && let Some(doc) = docs.list.iter().find(|d| d.path == path)
        {
            let pos = doc.editor.buffer.pos_at(line, from);
            self.navigate(doc, pos, kind);
        }
        let applies = std::mem::take(&mut self.pending_apply);
        for (id, edit) in applies {
            let n = apply_workspace_edit(docs, &edit);
            if let Some(s) = &self.session {
                s.reply(id, json!({ "applied": n.is_ok() }));
            }
        }
        if !self.ready() {
            return out;
        }
        self.sync_documents(docs);
        self.poll_requests(docs, show_inlays, &mut out);
        if let Some(doc) = self.save_again.take()
            && docs.list.iter().any(|d| d.editor.id == doc && d.is_modified())
        {
            out.push(InsightOutput::SaveFormatted);
        }
        out
    }

    /// The line of a problem, read from the open editor or from the text it was found in.
    pub fn problem_line(&self, docs: &Docs, path: &std::path::Path, start: usize) -> usize {
        let line = |rope: &rc_text::Rope| rope.char_to_line(start.min(rope.len_chars()));
        docs.list.iter().find(|d| d.path == path).map(|d| line(d.editor.buffer.rope())).or_else(|| self.problem_text.get(path).map(line)).unwrap_or(0)
    }

    fn store_problems(&mut self, path: PathBuf, list: Vec<rc_editor::Diagnostic>) {
        let relevant: Vec<rc_editor::Diagnostic> = list.into_iter().filter(|d| d.severity <= rc_editor::Severity::Warning).collect();
        if relevant.is_empty() {
            self.problems.remove(&path);
        } else {
            self.problems.insert(path, relevant);
        }
    }

    fn sync_documents(&mut self, docs: &mut Docs) {
        let Some(session) = &mut self.session else { return };
        let now = Instant::now();
        let open: Vec<PathBuf> = docs.list.iter().filter(|d| has_insight(d)).map(|d| d.path.clone()).collect();
        for path in session.docs.paths() {
            if !open.contains(&path) {
                session.did_close(&path);
            }
        }
        for d in docs.list.iter_mut().filter(|d| has_insight(d)) {
            if !session.docs.is_open(&d.path) {
                d.editor.take_edits();
                let language = if d.editor.lang() == rc_syntax::Lang::Rust { "rust" } else { "csharp" };
                session.did_open(&d.path, language, &d.editor.buffer.rope().to_string());
                self.dirty.insert(d.editor.id, now - REFRESH_DELAY);
                continue;
            }
            let edits = d.editor.take_edits();
            if !edits.is_empty() {
                session.did_change(&d.path, &edits);
                self.dirty.insert(d.editor.id, now);
            }
        }
    }

    /// Requests colours, hints, diagnostics and structure for `doc` once its edits settled.
    fn refresh(&mut self, doc: &Doc, show_inlays: bool) {
        let id = doc.editor.id;
        let version = doc.editor.buffer.version();
        if self.refreshed.get(&id) == Some(&version) {
            return;
        }
        if self.dirty.get(&id).is_some_and(|t| t.elapsed() < REFRESH_DELAY) {
            return;
        }
        let Some(session) = &self.session else { return };
        let uri = path_to_uri(&doc.path);
        let td = json!({ "uri": uri.as_str() });
        self.semantic.insert(id, (version, session.client.request_raw("textDocument/semanticTokens/full", json!({ "textDocument": td }))));
        // rust-analyzer publishes its diagnostics, including those of `cargo check`.
        if self.rust {
            self.runnables_request.insert(id, (version, session.client.request_raw("experimental/runnables", json!({ "textDocument": td }))));
            let start = json!({ "textDocument": td, "position": { "line": 0, "character": 0 } });
            self.parent_request.insert(id, (version, session.client.request_raw("experimental/parentModule", start)));
        } else {
            self.diag_pull.insert(id, (version, session.client.request_raw("textDocument/diagnostic", json!({ "textDocument": td }))));
        }
        self.symbols.insert(id, (version, session.client.request_raw("textDocument/documentSymbol", json!({ "textDocument": td }))));
        if show_inlays {
            let end = convert::lsp_position(doc.editor.buffer.rope(), doc.editor.buffer.len_chars());
            self.inlay.insert(id, (version, session.client.request_raw("textDocument/inlayHint", json!({ "textDocument": td, "range": { "start": { "line": 0, "character": 0 }, "end": end } }))));
        }
        self.refreshed.insert(id, version);
    }

    fn poll_requests(&mut self, docs: &mut Docs, show_inlays: bool, out: &mut Vec<InsightOutput>) {
        // Refreshes for the visible document (and any other open C# document once).
        if let Some(i) = docs.active
            && has_insight(&docs.list[i])
        {
            self.refresh(&docs.list[i], show_inlays);
        }
        // Semantic tokens.
        let legend = self.session.as_ref().map(|s| s.semantic_legend.clone()).unwrap_or_default();
        let mut done = Vec::new();
        for (&id, (version, p)) in &mut self.semantic {
            if let Some(r) = p.poll() {
                done.push(id);
                if let (Ok(Some(lsp::SemanticTokensResult::Tokens(t))), Some(d)) = (r, docs.list.iter_mut().find(|d| d.editor.id == id))
                    && d.editor.buffer.version() == *version
                {
                    let data: Vec<u32> = t.data.iter().flat_map(|x| [x.delta_line, x.delta_start, x.length, x.token_type, x.token_modifiers_bitset]).collect();
                    let spans = rc_lsp::semantic::decode(&data, &legend, d.editor.buffer.rope());
                    d.editor.set_semantic(spans);
                }
            }
        }
        for id in done.drain(..) {
            self.semantic.remove(&id);
        }
        // Runnables.
        let mut finished = Vec::new();
        for (&id, (version, p)) in &mut self.runnables_request {
            if let Some(r) = p.poll() {
                finished.push(id);
                if let Ok(reply) = r {
                    self.runnables.insert(id, (*version, rc_lsp::rust_analyzer::parse_runnables(&reply)));
                }
            }
        }
        for id in finished {
            self.runnables_request.remove(&id);
        }
        // Parent modules: no answer means that nothing declares the file.
        let mut finished = Vec::new();
        for (&id, (_, p)) in &mut self.parent_request {
            if let Some(r) = p.poll() {
                finished.push(id);
                if let Ok(reply) = r {
                    if reply.as_array().is_none_or(Vec::is_empty) {
                        self.unlinked.insert(id);
                    } else {
                        self.unlinked.remove(&id);
                    }
                }
            }
        }
        for id in finished {
            self.parent_request.remove(&id);
        }
        // Inlay hints.
        for (&id, (version, p)) in &mut self.inlay {
            if let Some(r) = p.poll() {
                done.push(id);
                if let (Ok(hints), Some(d)) = (r, docs.list.iter_mut().find(|d| d.editor.id == id))
                    && d.editor.buffer.version() == *version
                {
                    let rope = d.editor.buffer.rope().clone();
                    d.editor.inlays = hints
                        .unwrap_or_default()
                        .into_iter()
                        .map(|h| rc_editor::InlayHint {
                            pos: convert::char_of(&rope, h.position),
                            label: match h.label {
                                lsp::InlayHintLabel::String(s) => s,
                                lsp::InlayHintLabel::LabelParts(parts) => parts.into_iter().map(|p| p.value).collect(),
                            },
                            pad_left: h.padding_left.unwrap_or(false),
                            pad_right: h.padding_right.unwrap_or(false),
                        })
                        .collect();
                }
            }
        }
        for id in done.drain(..) {
            self.inlay.remove(&id);
        }
        // Pulled diagnostics.
        let mut reports = Vec::new();
        for (&id, (version, p)) in &mut self.diag_pull {
            if let Some(r) = p.poll() {
                done.push(id);
                if let Ok(report) = r {
                    reports.push((id, *version, report));
                }
            }
        }
        for (id, version, report) in reports {
            let Some(d) = docs.list.iter_mut().find(|d| d.editor.id == id) else { continue };
            if d.editor.buffer.version() != version || report.get("kind").and_then(Value::as_str) == Some("unchanged") {
                continue;
            }
            let items: Vec<lsp::Diagnostic> = report.get("items").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
            let converted: Vec<rc_editor::Diagnostic> = items.iter().map(|x| convert::diagnostic(d.editor.buffer.rope(), x)).collect();
            d.editor.set_diagnostics(converted.clone());
            let path = d.path.clone();
            self.store_problems(path, converted);
        }
        for id in done.drain(..) {
            self.diag_pull.remove(&id);
        }
        // Document symbols.
        for (&id, (_, p)) in &mut self.symbols {
            if let Some(r) = p.poll() {
                done.push(id);
                if let (Ok(Some(resp)), Some(d)) = (r, docs.list.iter().find(|d| d.editor.id == id)) {
                    let rope = d.editor.buffer.rope().clone();
                    self.structure.insert(id, symbols(&rope, resp));
                }
            }
        }
        for id in done.drain(..) {
            self.symbols.remove(&id);
        }
        self.poll_completion(docs);
        self.poll_popups(docs);
        self.poll_navigation(docs, out);
    }

    fn poll_completion(&mut self, docs: &mut Docs) {
        if let Some((doc, start, p)) = &mut self.accepted_resolve
            && let Some(r) = p.poll()
        {
            let (doc, start) = (*doc, *start);
            self.accepted_resolve = None;
            if let (Ok(item), Some(d)) = (r, docs.list.iter_mut().find(|d| d.editor.id == doc)) {
                let rope = d.editor.buffer.rope().clone();
                // The offsets describe the text before the insertion, so only edits above it hold.
                let changes: Vec<_> = item
                    .additional_text_edits
                    .unwrap_or_default()
                    .iter()
                    .map(|e| (convert::range_chars(&rope, &e.range), e.new_text.replace("\r\n", "\n")))
                    .filter(|((_, b), _)| *b <= start)
                    .map(|((a, b), text)| rc_text::Change::new(a, b, text))
                    .collect();
                if !changes.is_empty() {
                    d.editor.apply_text_edits(changes);
                }
            }
        }
        if let Some(req) = &mut self.completion
            && let Some(r) = req.pending.poll()
        {
            let req = self.completion.take().expect("checked");
            self.completion_anchor = req.anchor;
            self.last_completion_ms = Some(req.pending.started.elapsed().as_millis());
            let Some(d) = docs.list.iter_mut().find(|d| d.editor.id == req.doc) else { return };
            let (items, default_range) = parse_completion(r.unwrap_or(Value::Null));
            let rope = d.editor.buffer.rope().clone();
            let mut sorted = items;
            sorted.sort_by(|a, b| a.sort_text.as_deref().unwrap_or(&a.label).cmp(b.sort_text.as_deref().unwrap_or(&b.label)));
            // `data` is the index in `raw_items`, to find the server's item again on accept.
            let editor_items = sorted.iter().enumerate().map(|(i, it)| convert::completion_item(&rope, it, i, default_range.as_ref())).collect();
            self.raw_items = sorted;
            let caret = d.editor.caret();
            if caret < req.anchor || d.editor.buffer.version() < req.version {
                return;
            }
            let prefix = rope.slice(req.anchor..caret).to_string();
            let mut c = Completion::new(req.anchor, editor_items, false);
            c.filter(&prefix);
            if c.filtered.is_empty() {
                d.editor.popups.completion = None;
            } else {
                d.editor.popups.completion = Some(c);
                self.resolve = None;
            }
        }
        // Documentation of the selected item.
        let fence = if self.rust { "rust" } else { "csharp" };
        let Some(i) = docs.active else { return };
        let Some(c) = docs.list[i].editor.popups.completion.as_ref() else { return };
        let Some(&idx) = c.filtered.get(c.selected) else { return };
        match &mut self.resolve {
            Some((at, slot)) if *at == idx => {
                if let Some(p) = slot
                    && let Some(r) = p.poll()
                {
                    *slot = None;
                    if let (Ok(item), Some(c)) = (r, docs.list[i].editor.popups.completion.as_mut()) {
                        let doc = item.documentation.as_ref().map(convert::documentation).map(|d| clean_hover(&d));
                        let detail = item.detail.clone().filter(|d| !d.is_empty());
                        c.doc = match (detail, doc) {
                            (Some(d), Some(x)) => Some(format!("```{fence}\n{d}\n```\n{x}")),
                            (Some(d), None) => Some(format!("```{fence}\n{d}\n```")),
                            (None, x) => x,
                        };
                        if let Some(raw) = self.raw_items.get_mut(idx) {
                            *raw = item;
                        }
                    }
                }
            }
            _ => {
                if let (Some(session), Some(item)) = (&self.session, self.raw_items.get(c.items[idx].data)) {
                    let p = session.client.request_raw("completionItem/resolve", serde_json::to_value(item).unwrap_or(Value::Null));
                    self.resolve = Some((idx, Some(p)));
                    if let Some(c) = docs.list[i].editor.popups.completion.as_mut() {
                        c.doc = None;
                    }
                }
            }
        }
    }

    fn poll_popups(&mut self, docs: &mut Docs) {
        self.poll_usings(docs);
        if let Some((doc, pos, screen, p)) = &mut self.hover
            && let Some(r) = p.poll()
        {
            let (doc, pos, screen) = (*doc, *pos, *screen);
            self.hover = None;
            if let Some(d) = docs.list.iter_mut().find(|d| d.editor.id == doc) {
                let text = r.ok().flatten().map(|h| convert::markup_text(&h.contents)).unwrap_or_default();
                let problem = d.editor.diagnostic_at(pos).map(|x| (x.severity, x.message.clone()));
                if !text.trim().is_empty() || problem.is_some() {
                    d.editor.popups.hover = Some(Hover { pos, text: clean_hover(&text), problem, screen });
                }
            }
        }
        if let Some((doc, anchor, p)) = &mut self.signature
            && let Some(r) = p.poll()
        {
            let (doc, anchor) = (*doc, *anchor);
            self.signature = None;
            if let Some(d) = docs.list.iter_mut().find(|d| d.editor.id == doc) {
                d.editor.popups.signature = r.ok().flatten().filter(|h| !h.signatures.is_empty()).map(|h| signature_help(h, anchor));
            }
        }
        if let Some((doc, (version, p))) = &mut self.type_format
            && let Some(r) = p.poll()
        {
            let (doc, version) = (*doc, *version);
            self.type_format = None;
            if let (Ok(Some(edits)), Some(d)) = (r, docs.list.iter_mut().find(|d| d.editor.id == doc))
                && d.editor.buffer.version() == version
                && !edits.is_empty()
            {
                let rope = d.editor.buffer.rope().clone();
                let changes = edits.iter().map(|e| {
                    let (a, b) = convert::range_chars(&rope, &e.range);
                    rc_text::Change::new(a, b, e.new_text.replace("\r\n", "\n"))
                });
                d.editor.apply_text_edits(changes.collect());
            }
        }
        if let Some((doc, (version, p))) = &mut self.auto_insert
            && let Some(r) = p.poll()
        {
            let (doc, version) = (*doc, *version);
            self.auto_insert = None;
            if let (Ok(v), Some(d)) = (r, docs.list.iter_mut().find(|d| d.editor.id == doc))
                && d.editor.buffer.version() == version
                && let Some(edit) = v.get("_vs_textEdit").cloned().and_then(|e| serde_json::from_value::<lsp::TextEdit>(e).ok())
            {
                apply_auto_insert(d, &edit);
            }
        }
        if let Some((doc, (version, p))) = &mut self.format
            && let Some(r) = p.poll()
        {
            let (doc, version) = (*doc, *version);
            self.format = None;
            if self.format_on_save.take() == Some(doc) {
                self.save_again = Some(doc);
            }
            if let (Ok(Some(edits)), Some(d)) = (r, docs.list.iter_mut().find(|d| d.editor.id == doc))
                && d.editor.buffer.version() == version
            {
                let rope = d.editor.buffer.rope().clone();
                let changes = edits.iter().map(|e| {
                    let (a, b) = convert::range_chars(&rope, &e.range);
                    rc_text::Change::new(a, b, e.new_text.replace("\r\n", "\n"))
                });
                d.editor.apply_text_edits(changes.collect());
            }
        }
    }

    /// Asks who calls what is at `pos`, or (`callers` false) what it calls.
    pub fn call_hierarchy(&mut self, doc: &Doc, pos: usize, callers: bool) {
        let Some(session) = &self.session else { return };
        let params = json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "position": convert::lsp_position(doc.editor.buffer.rope(), pos) });
        self.calls = Some((callers, session.client.request_raw("textDocument/prepareCallHierarchy", params)));
    }

    fn poll_external_docs(&mut self, out: &mut Vec<InsightOutput>) {
        if let Some(p) = &mut self.external_docs
            && let Some(reply) = p.poll()
        {
            self.external_docs = None;
            out.push(match reply.ok().as_ref().and_then(web_docs) {
                Some(url) => InsightOutput::OpenUrl(url),
                None => InsightOutput::Message(tr("No online documentation for this item").into()),
            });
        }
    }

    fn poll_expand_macro(&mut self, out: &mut Vec<InsightOutput>) {
        if let Some(p) = &mut self.expand_macro
            && let Some(reply) = p.poll()
        {
            self.expand_macro = None;
            out.push(match reply.ok().as_ref().and_then(macro_expansion) {
                Some((name, text)) => InsightOutput::MacroExpansion { name, text },
                None => InsightOutput::Message(tr("No macro call at the caret").into()),
            });
        }
    }

    fn poll_calls(&mut self, docs: &Docs, out: &mut Vec<InsightOutput>) {
        use crate::call_tree::{CallNode, CallTree, entries};
        if let Some((_, p)) = &mut self.calls
            && let Some(reply) = p.poll()
            && let Some((callers, _)) = self.calls.take()
        {
            let reply = reply.unwrap_or(Value::Null);
            match reply.as_array().and_then(|a| a.first()).cloned() {
                Some(item) => {
                    let place = json!([{ "uri": item.get("uri"), "range": item.get("selectionRange") }]);
                    let target = locations(&place, docs).into_iter().next();
                    self.call_tree = Some(CallTree { callers, types: false, root: CallNode::new(item, target, 1) });
                    self.call_generation += 1;
                    self.toggle_call(&[]);
                    out.push(InsightOutput::ShowCallTree);
                }
                None => out.push(InsightOutput::Message(tr("Place the caret on a function").into())),
            }
        }
        let mut finished = Vec::new();
        for (i, (_, _, p)) in self.call_levels.iter_mut().enumerate() {
            if let Some(reply) = p.poll() {
                finished.push((i, reply.unwrap_or(Value::Null)));
            }
        }
        for (i, reply) in finished.into_iter().rev() {
            let (generation, path, _) = self.call_levels.remove(i);
            let tree = self.call_tree.as_mut().filter(|_| generation == self.call_generation);
            let Some(tree) = tree else { continue };
            let children = entries(&reply, tree.callers).into_iter().map(|(item, range, calls)| {
                let place = json!([{ "uri": item.get("uri"), "range": range }]);
                CallNode::new(item, locations(&place, docs).into_iter().next(), calls)
            });
            let children = children.collect();
            if let Some(node) = tree.node_mut(&path) {
                node.children = Some(children);
            }
        }
    }

    /// What implements the trait, or is implemented for the type, at `pos`.
    pub fn type_hierarchy(&mut self, doc: &Doc, pos: usize) {
        let Some(session) = &self.session else { return };
        let rope = doc.editor.buffer.rope();
        let params = json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "position": convert::lsp_position(rope, pos) });
        let (line, col) = doc.editor.buffer.line_col(pos);
        let root = Target { path: doc.path.clone(), line, col, preview: word_at(&doc.editor.buffer.line_text(line), col) };
        self.types = Some((root, session.client.request_raw("textDocument/implementation", params)));
    }

    fn poll_types(&mut self, docs: &Docs, out: &mut Vec<InsightOutput>) {
        use crate::call_tree::{CallNode, CallTree};
        let Some((_, p)) = &mut self.types else { return };
        let Some(reply) = p.poll() else { return };
        let Some((root, _)) = self.types.take() else { return };
        let children = locations(&reply.unwrap_or(Value::Null), docs).into_iter().map(|t| {
            let file = t.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut node = CallNode::new(json!({ "name": impl_title(&t.preview), "detail": format!("{file}:{}", t.line + 1) }), Some(t), 1);
            node.children = Some(Vec::new());
            node
        });
        let mut node = CallNode::new(json!({ "name": root.preview }), Some(root), 1);
        node.children = Some(children.collect());
        node.open = true;
        self.call_generation += 1;
        self.call_tree = Some(CallTree { callers: false, types: true, root: node });
        out.push(InsightOutput::ShowCallTree);
    }

    /// Opens or closes a node of the call tree, loading its children the first time.
    pub fn toggle_call(&mut self, path: &[usize]) {
        let callers = self.call_tree.as_ref().is_some_and(|t| t.callers);
        let Some(node) = self.call_tree.as_mut().and_then(|t| t.node_mut(path)) else { return };
        node.open = !node.open;
        let loaded = node.children.is_some() || !node.open;
        let item = node.item.clone();
        let loading = self.call_levels.iter().any(|(g, p, _)| *g == self.call_generation && p == path);
        if let (false, false, Some(session)) = (loaded, loading, &self.session) {
            let method = if callers { "callHierarchy/incomingCalls" } else { "callHierarchy/outgoingCalls" };
            self.call_levels.push((self.call_generation, path.to_vec(), session.client.request_raw(method, json!({ "item": item }))));
        }
    }

    /// Shows the other direction for the same function.
    pub fn switch_calls(&mut self) {
        let Some(tree) = self.call_tree.take() else { return };
        let root = crate::call_tree::CallNode::new(tree.root.item, tree.root.target, 1);
        self.call_tree = Some(crate::call_tree::CallTree { callers: !tree.callers, types: false, root });
        self.call_generation += 1;
        self.toggle_call(&[]);
    }

    /// Go to Declaration / Implementation / Usages results.
    fn poll_goto(&mut self, docs: &Docs, out: &mut Vec<InsightOutput>) {
        let Some((_, _, _, p)) = &mut self.nav else { return };
        let Some(r) = p.poll() else { return };
        let Some((kind, origin, jump, _)) = self.nav.take() else { return };
        let targets = r.map(|v| locations(&v, docs)).unwrap_or_default();
        // Nothing yet because the solution is still loading: go there once it has loaded.
        if targets.is_empty() && self.status != InsightStatus::Ready {
            self.nav_retry = Some((kind, origin));
            out.push(InsightOutput::Message(tr("The C# server is still loading the solution; navigation continues when it is ready").into()));
            return;
        }
        let (path, line, from, to) = origin;
        // ⌘+click on a declaration itself shows its usages (Rider).
        if kind == NavKind::Declaration
            && let [t] = targets.as_slice()
            && t.path == path
            && t.line == line
            && (from..=to).contains(&t.col)
            && let Some(doc) = docs.list.iter().find(|d| d.path == path)
        {
            let pos = doc.editor.buffer.pos_at(line, from);
            self.navigate(doc, pos, NavKind::Usages);
            if let Some(nav) = &mut self.nav {
                nav.2 = true;
            }
            return;
        }
        match (kind, targets.len()) {
            (_, 0) => {
                out.push(InsightOutput::Message(match kind {
                    NavKind::Usages => tr("No usages found").into(),
                    NavKind::Implementation => tr("No implementations found").into(),
                    _ => tr("Cannot find declaration to go to").into(),
                }));
                // Find Usages never leaves the list of another symbol in the Find window.
                if kind == NavKind::Usages && !jump {
                    out.push(InsightOutput::Usages { title: tr("Usages").into(), targets });
                }
            }
            (NavKind::Usages, _) if !jump || targets.len() > 1 => out.push(InsightOutput::Usages { title: tr("Usages").into(), targets }),
            (_, 1) => out.push(InsightOutput::Navigate(targets.into_iter().next().expect("one"))),
            (NavKind::Implementation, _) => out.push(InsightOutput::Usages { title: tr("Implementations").into(), targets }),
            _ => out.push(InsightOutput::Usages { title: tr("Declarations").into(), targets }),
        }
    }

    fn poll_navigation(&mut self, docs: &mut Docs, out: &mut Vec<InsightOutput>) {
        self.poll_goto(docs, out);
        self.poll_calls(docs, out);
        self.poll_types(docs, out);
        self.poll_external_docs(out);
        self.poll_expand_macro(out);
        if let Some((doc, pos, p)) = &mut self.rename_prepare
            && let Some(r) = p.poll()
        {
            let (doc, pos) = (*doc, *pos);
            self.rename_prepare = None;
            match r {
                Ok(v) if !v.is_null() => {
                    let name = v.get("placeholder").and_then(Value::as_str).map(str::to_string).or_else(|| {
                        let range: lsp::Range = serde_json::from_value(v.get("range").cloned().unwrap_or(v.clone())).ok()?;
                        let d = docs.list.iter().find(|d| d.editor.id == doc)?;
                        let (a, b) = convert::range_chars(d.editor.buffer.rope(), &range);
                        Some(d.editor.buffer.rope().slice(a..b).to_string())
                    });
                    match name {
                        Some(name) => out.push(InsightOutput::AskRename { doc, pos, name }),
                        None => out.push(InsightOutput::Message(tr("This element cannot be renamed").into())),
                    }
                }
                Ok(_) => out.push(InsightOutput::Message(tr("This element cannot be renamed").into())),
                Err(e) => out.push(InsightOutput::Message(e.message)),
            }
        }
        if let Some(p) = &mut self.rename
            && let Some(r) = p.poll()
        {
            self.rename = None;
            match r {
                Ok(Some(edit)) => match apply_workspace_edit(docs, &edit) {
                    Ok(files) => out.push(InsightOutput::Message(trf("Renamed in {n} files", &[("n", &files)]))),
                    Err(e) => out.push(InsightOutput::Message(e)),
                },
                Ok(None) => out.push(InsightOutput::Message(tr("Nothing to rename").into())),
                Err(e) => out.push(InsightOutput::Message(e.message)),
            }
        }
        if let Some(p) = &mut self.actions
            && let Some(r) = p.poll()
        {
            self.actions = None;
            let list = r.ok().flatten().unwrap_or_default();
            if self.optimizing.is_some() {
                self.optimize_found(docs, list, out);
            } else if std::mem::take(&mut self.refactor_only) {
                out.push(InsightOutput::Actions(refactorings(list)));
            } else if list.is_empty() && !(self.generate_only && docs.active_doc().is_some_and(|d| derive_target_of(d).is_some())) && (self.generate_only || docs.active_doc().is_none_or(|d| suppress_commands(d).is_empty())) {
                self.generate_only = false;
                out.push(InsightOutput::Message(tr("No context actions here").into()));
            } else {
                if std::mem::take(&mut self.generate_only) {
                    let mut list: Vec<_> = list.into_iter().filter(|a| generates_code(action_title(a))).collect();
                    // On a Rust type: its derive list, written by the IDE.
                    if docs.active_doc().is_some_and(|d| derive_target_of(d).is_some()) {
                        list.insert(0, lsp::CodeActionOrCommand::Command(lsp::Command { title: tr("Derive…").into(), command: DERIVE_COMMAND.into(), arguments: None }));
                    }
                    out.push(if list.is_empty() { InsightOutput::Message(tr("Nothing to generate here").into()) } else { InsightOutput::Actions(list) });
                } else {
                    // The server's fixes first, then allowing the warning's lint.
                    let mut list = list;
                    list.extend(docs.active_doc().map(suppress_commands).unwrap_or_default());
                    out.push(InsightOutput::Actions(list));
                }
            }
        }
        if let Some(p) = &mut self.action_resolve
            && let Some(r) = p.poll()
        {
            self.action_resolve = None;
            match r {
                Ok(action) => self.run_code_action_now(docs, lsp::CodeActionOrCommand::CodeAction(action), out),
                Err(e) => out.push(InsightOutput::Message(e.message)),
            }
            self.optimize_next(docs, out);
        }
    }

    /// Ctrl+Alt+O: removes the unused imports of a Rust document and merges the rest.
    pub fn optimize_imports(&mut self, docs: &mut Docs, out: &mut Vec<InsightOutput>) {
        let Some(id) = docs.active_doc().map(|d| d.editor.id) else { return };
        if !self.rust || self.session.is_none() {
            out.push(InsightOutput::Message(tr("The Rust analysis is not running").into()));
            return;
        }
        self.optimizing = Some(Optimizing { doc: id, steps: OPTIMIZE_STEPS.to_vec(), changed: false });
        self.optimize_next(docs, out);
    }

    /// Asks the server for the actions of the import block, for the next step.
    fn optimize_next(&mut self, docs: &mut Docs, out: &mut Vec<InsightOutput>) {
        let Some(id) = self.optimizing.as_ref().map(|o| o.doc) else { return };
        let done = self.optimizing.as_ref().is_some_and(|o| o.steps.is_empty());
        let doc = docs.list.iter_mut().find(|d| d.editor.id == id);
        let block = doc.as_ref().and_then(|d| use_block(&d.editor.buffer.rope().to_string()));
        let (false, Some(doc), Some((start, end))) = (done, doc, block) else {
            let changed = self.optimizing.take().is_some_and(|o| o.changed);
            out.push(InsightOutput::Message(tr(if changed { "Imports optimized" } else { "Nothing to optimize in the imports" }).into()));
            return;
        };
        self.flush(doc);
        let Some(session) = &self.session else { return };
        let rope = doc.editor.buffer.rope();
        let range = lsp::Range { start: convert::lsp_position(rope, start), end: convert::lsp_position(rope, end) };
        let params = json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "range": range, "context": { "diagnostics": [], "triggerKind": 1 } });
        self.actions = Some(session.client.request_raw("textDocument/codeAction", params));
    }

    /// Runs the action of the current step, or goes on when the server offers none.
    fn optimize_found(&mut self, docs: &mut Docs, list: Vec<lsp::CodeActionOrCommand>, out: &mut Vec<InsightOutput>) {
        let Some(o) = &mut self.optimizing else { return };
        let step = if o.steps.is_empty() { "" } else { o.steps.remove(0) };
        match list.into_iter().find(|a| action_title(a) == step) {
            Some(action) => {
                o.changed = true;
                let resolves = matches!(&action, lsp::CodeActionOrCommand::CodeAction(a) if a.edit.is_none() && a.command.is_none());
                self.run_code_action(docs, action, out);
                if !resolves {
                    self.optimize_next(docs, out);
                }
            }
            None => self.optimize_next(docs, out),
        }
    }

    // ------------------------------------------------------------------ editor events

    /// Sends `doc`'s pending edits now, so a request about to be made sees the current text.
    pub fn flush(&mut self, doc: &mut Doc) {
        let Some(session) = &mut self.session else { return };
        if !session.is_initialized() || !session.docs.is_open(&doc.path) {
            return;
        }
        let edits = doc.editor.take_edits();
        if !edits.is_empty() {
            session.did_change(&doc.path, &edits);
            self.dirty.insert(doc.editor.id, Instant::now());
        }
    }

    /// Reacts to what the active editor did this frame.
    pub fn on_editor_events(&mut self, doc: &mut Doc, events: &[EditorEvent]) {
        if !self.ready() || !has_insight(doc) {
            return;
        }
        if !events.is_empty() {
            self.flush(doc);
        }
        for ev in events {
            match ev {
                EditorEvent::Typed(c) => self.on_typed(doc, *c),
                // Pasted code gets its usings once the server has seen it (Rider, VS).
                EditorEvent::Pasted => {
                    self.auto_usings.insert(doc.editor.id, doc.editor.buffer.version());
                }
                EditorEvent::HoverAt { pos, screen } => self.request_hover(doc, *pos, *screen),
                EditorEvent::HoverEnd => self.hover = None,
                EditorEvent::NavigateAt(pos) => self.navigate(doc, *pos, NavKind::Declaration),
                EditorEvent::CaretMoved => {
                    // Leaving the call's parentheses closes parameter info.
                    if let Some(sig) = &doc.editor.popups.signature {
                        let caret = doc.editor.caret();
                        let rope = doc.editor.buffer.rope();
                        if caret <= sig.anchor || !inside_call(rope, sig.anchor, caret) {
                            doc.editor.popups.signature = None;
                        } else {
                            self.request_signature(doc, sig.anchor);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn on_typed(&mut self, doc: &mut Doc, c: char) {
        let ident = c.is_alphanumeric() || c == '_';
        if doc.editor.popups.completion.is_none() && (c == '.' || ident) {
            let caret = doc.editor.caret();
            let rope = doc.editor.buffer.rope();
            // Start completion at the beginning of an identifier (IntelliJ autopopup).
            let prev = if caret >= 2 { Some(rope.char(caret - 2)) } else { None };
            let starts_word = c == '.' || prev.is_none_or(|p| !(p.is_alphanumeric() || p == '_'));
            if starts_word {
                self.complete(doc, Some(c));
            }
        }
        if c == '/' || c == '\n' {
            self.request_auto_insert(doc, c);
        } else if c == ';' || c == '}' {
            self.request_type_format(doc, c);
        }
        if c == '(' || c == ',' {
            if let Some(anchor) = call_open_paren(doc.editor.buffer.rope(), doc.editor.caret()) {
                self.request_signature(doc, anchor);
            }
        } else if c == ')' {
            doc.editor.popups.signature = None;
        }
    }

    /// Roslyn's `_vs_onAutoInsert` (what VS and VS Code use): the doc comment skeleton after the
    /// third `/` of `///`, and `/// ` on the line after Enter inside one.
    fn request_auto_insert(&mut self, doc: &Doc, c: char) {
        let Some(session) = &self.session else { return };
        let rope = doc.editor.buffer.rope();
        let caret = doc.editor.caret();
        // Only where it can apply: after `///`, or on a line following a `///` line.
        let line = rope.char_to_line(caret);
        let text_of = |l: usize| rope.line(l).to_string();
        let applies = match c {
            '/' => rope.slice(rope.line_to_char(line)..caret).to_string().trim_start() == "///",
            _ => line > 0 && text_of(line - 1).trim_start().starts_with("///"),
        };
        if !applies {
            return;
        }
        let options = json!({ "tabSize": doc.editor.indent.tab_width, "insertSpaces": doc.editor.indent.unit != "\t" });
        let p = session.client.request_raw(
            "textDocument/_vs_onAutoInsert",
            json!({ "_vs_textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "_vs_position": convert::lsp_position(rope, caret), "_vs_ch": c.to_string(), "_vs_options": options }),
        );
        self.auto_insert = Some((doc.editor.id, (doc.editor.buffer.version(), p)));
    }

    /// `textDocument/onTypeFormatting` after `;` or `}`: Roslyn re-indents the statement or block.
    fn request_type_format(&mut self, doc: &Doc, c: char) {
        let Some(session) = &self.session else { return };
        let rope = doc.editor.buffer.rope();
        let options = json!({ "tabSize": doc.editor.indent.tab_width, "insertSpaces": doc.editor.indent.unit != "\t" });
        let p = session.client.request_raw(
            "textDocument/onTypeFormatting",
            json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "position": convert::lsp_position(rope, doc.editor.caret()), "ch": c.to_string(), "options": options }),
        );
        self.type_format = Some((doc.editor.id, (doc.editor.buffer.version(), p)));
    }

    /// Add Missing Usings: asks the server for the fixes of every unresolved name; the ones with a
    /// single `using` are added (see `poll_usings`).
    pub fn add_missing_usings(&mut self, doc: &Doc) {
        let Some(session) = &self.session else { return };
        let rope = doc.editor.buffer.rope();
        let mut seen = std::collections::HashSet::new();
        let mut pending = Vec::new();
        for d in &doc.editor.diagnostics {
            // Type or namespace not found, name not found, missing member (extension methods).
            if !matches!(d.code.as_deref(), Some("CS0246" | "CS0103" | "CS1061" | "CS0234" | "CS1935")) {
                continue;
            }
            let name = rope.slice(d.start..d.end).to_string();
            if !seen.insert((name, d.code.clone())) {
                continue;
            }
            let range = json!({ "start": convert::lsp_position(rope, d.start), "end": convert::lsp_position(rope, d.end) });
            let diag = json!({ "range": range, "severity": 1, "code": d.code, "message": d.message });
            pending.push(session.client.request_raw(
                "textDocument/codeAction",
                json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "range": range, "context": { "diagnostics": [diag], "triggerKind": 1 } }),
            ));
        }
        if !pending.is_empty() {
            self.usings = Some((doc.editor.id, doc.editor.buffer.version(), pending, Vec::new()));
        }
    }

    fn poll_usings(&mut self, docs: &mut Docs) {
        // After a paste: once the server has published diagnostics for the pasted text.
        let ready: Vec<u64> = self.auto_usings.iter().filter(|(id, v)| self.refreshed.get(id).is_some_and(|r| r >= v) && !self.diag_pull.contains_key(id)).map(|(id, _)| *id).collect();
        for id in ready {
            self.auto_usings.remove(&id);
            if self.usings.is_none()
                && let Some(d) = docs.list.iter().find(|d| d.editor.id == id)
            {
                self.add_missing_usings(d);
            }
        }
        let Some((doc, version, pending, results)) = &mut self.usings else { return };
        let mut i = 0;
        while i < pending.len() {
            match pending[i].poll() {
                Some(r) => {
                    pending.remove(i);
                    if let Ok(v) = r {
                        results.push(v);
                    }
                }
                None => i += 1,
            }
        }
        if !pending.is_empty() {
            return;
        }
        let (doc, version, results) = (*doc, *version, std::mem::take(results));
        self.usings = None;
        let Some(d) = docs.list.iter_mut().find(|d| d.editor.id == doc).filter(|d| d.editor.buffer.version() == version) else { return };
        let namespaces: Vec<String> = results.iter().filter_map(single_using).collect();
        insert_usings(d, &namespaces);
    }

    /// Code completion at the caret (⌃Space, or typing).
    pub fn complete(&mut self, doc: &mut Doc, trigger: Option<char>) {
        let Some(session) = &self.session else { return };
        let caret = doc.editor.caret();
        let rope = doc.editor.buffer.rope();
        let (anchor, _) = rc_text::commands::word_at(rope, caret);
        let anchor = anchor.min(caret);
        let context = match trigger {
            Some('.') => json!({ "triggerKind": 2, "triggerCharacter": "." }),
            _ => json!({ "triggerKind": 1 }),
        };
        let pending = session.client.request_raw(
            "textDocument/completion",
            json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "position": convert::lsp_position(rope, caret), "context": context }),
        );
        self.completion = Some(CompletionRequest { doc: doc.editor.id, anchor, version: doc.editor.buffer.version(), pending });
        if trigger.is_none() {
            doc.editor.popups.completion = Some(Completion::new(anchor, Vec::new(), true));
        }
    }

    /// After the editor inserted the chosen item: additional edits (usings) and commands.
    pub fn completion_accepted(&mut self, doc: &mut Doc, index_in_items: usize) {
        let Some(item) = self.raw_items.get(index_in_items).cloned() else { return };
        // Not resolved yet. The server has not seen the insertion, so it can still resolve the item.
        if self.rust
            && item.additional_text_edits.is_none()
            && item.data.is_some()
            && let Some(session) = &self.session
        {
            let start = doc.editor.buffer.len_chars().min(self.completion_anchor);
            let p = session.client.request_raw("completionItem/resolve", serde_json::to_value(&item).unwrap_or(Value::Null));
            self.accepted_resolve = Some((doc.editor.id, start, p));
        }
        if let Some(extra) = item.additional_text_edits.filter(|e| !e.is_empty()) {
            let rope = doc.editor.buffer.rope().clone();
            let changes = extra
                .iter()
                .map(|e| {
                    let (a, b) = convert::range_chars(&rope, &e.range);
                    rc_text::Change::new(a, b, e.new_text.replace("\r\n", "\n"))
                })
                .collect();
            doc.editor.apply_text_edits(changes);
        }
        if let (Some(cmd), Some(session)) = (item.command, &self.session) {
            let _ = session.client.request_raw::<Value>("workspace/executeCommand", json!({ "command": cmd.command, "arguments": cmd.arguments }));
        }
    }

    fn request_hover(&mut self, doc: &Doc, pos: usize, screen: egui::Pos2) {
        let Some(session) = &self.session else { return };
        let p = session.client.request_raw(
            "textDocument/hover",
            json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "position": convert::lsp_position(doc.editor.buffer.rope(), pos) }),
        );
        self.hover = Some((doc.editor.id, pos, screen, p));
    }

    /// F1 / ⌃J: documentation at the caret, shown under the caret.
    pub fn quick_doc(&mut self, doc: &Doc, screen: egui::Pos2) {
        self.request_hover(doc, doc.editor.caret(), screen);
    }

    pub fn request_signature(&mut self, doc: &Doc, anchor: usize) {
        let Some(session) = &self.session else { return };
        let p = session.client.request_raw(
            "textDocument/signatureHelp",
            json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "position": convert::lsp_position(doc.editor.buffer.rope(), doc.editor.caret()), "context": { "triggerKind": 1, "isRetrigger": false } }),
        );
        self.signature = Some((doc.editor.id, anchor, p));
    }

    /// ⌃⇧Space: parameter info for the call around the caret.
    pub fn parameter_info(&mut self, doc: &mut Doc) {
        match call_open_paren(doc.editor.buffer.rope(), doc.editor.caret()) {
            Some(anchor) => self.request_signature(doc, anchor),
            None => doc.editor.popups.signature = None,
        }
    }

    pub fn navigate(&mut self, doc: &Doc, pos: usize, kind: NavKind) {
        let Some(session) = &self.session else { return };
        let method = match kind {
            NavKind::Declaration => "textDocument/definition",
            NavKind::Implementation => "textDocument/implementation",
            NavKind::TypeDeclaration => "textDocument/typeDefinition",
            NavKind::Super => "textDocument/declaration",
            NavKind::Usages => "textDocument/references",
        };
        let mut params = json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "position": convert::lsp_position(doc.editor.buffer.rope(), pos) });
        if kind == NavKind::Usages {
            params["context"] = json!({ "includeDeclaration": false });
        }
        let (from, to) = rc_text::commands::word_at(doc.editor.buffer.rope(), pos);
        let (line, start) = doc.editor.buffer.line_col(from);
        let origin = (doc.path.clone(), line, start, start + (to - from));
        self.nav = Some((kind, origin, false, session.client.request_raw(method, params)));
    }

    /// Shift+F6: asks the server what can be renamed at the caret.
    pub fn prepare_rename(&mut self, doc: &Doc) {
        let Some(session) = &self.session else { return };
        let pos = doc.editor.caret();
        let p = session.client.request_raw(
            "textDocument/prepareRename",
            json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "position": convert::lsp_position(doc.editor.buffer.rope(), pos) }),
        );
        self.rename_prepare = Some((doc.editor.id, pos, p));
    }

    pub fn rename(&mut self, doc: &Doc, pos: usize, new_name: &str) {
        let Some(session) = &self.session else { return };
        let p = session.client.request_raw(
            "textDocument/rename",
            json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "position": convert::lsp_position(doc.editor.buffer.rope(), pos), "newName": new_name }),
        );
        self.rename = Some(p);
    }

    /// ⌥⏎: quick fixes and refactorings at the caret / selection.
    /// Shift+F1: asks for the web page of the documentation of the item at `pos`.
    pub fn external_docs(&mut self, doc: &Doc, pos: usize) {
        let Some(session) = self.session.as_ref().filter(|_| self.rust) else { return };
        let params = json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "position": convert::lsp_position(doc.editor.buffer.rope(), pos) });
        self.external_docs = Some(session.client.request_raw("experimental/externalDocs", params));
    }

    /// The recursive expansion of the macro call at `pos` (rust-analyzer only expands fully).
    pub fn expand_macro(&mut self, doc: &Doc, pos: usize) {
        let Some(session) = self.session.as_ref().filter(|_| self.rust) else { return };
        let params = json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "position": convert::lsp_position(doc.editor.buffer.rope(), pos) });
        self.expand_macro = Some(session.client.request_raw("rust-analyzer/expandMacro", params));
    }

    /// The context actions that write code (Alt+Insert).
    pub fn generate_actions(&mut self, doc: &Doc) {
        self.code_actions(doc);
        self.generate_only = true;
    }

    /// Rename and the server's refactorings (Ctrl+Alt+Shift+T).
    pub fn refactor_actions(&mut self, doc: &Doc) {
        self.code_actions(doc);
        self.refactor_only = true;
    }

    pub fn code_actions(&mut self, doc: &Doc) {
        self.generate_only = false;
        self.refactor_only = false;
        let Some(session) = &self.session else { return };
        let rope = doc.editor.buffer.rope();
        let sel = doc.editor.selection.primary();
        let range = lsp::Range { start: convert::lsp_position(rope, sel.start()), end: convert::lsp_position(rope, sel.end()) };
        // The diagnostics around the caret, so the server offers their fixes.
        let line = doc.editor.caret_line_col().0;
        let diags: Vec<Value> = doc
            .editor
            .diagnostics
            .iter()
            .filter(|d| doc.editor.buffer.line_col(d.start).0 <= line && line <= doc.editor.buffer.line_col(d.end).0)
            .map(|d| {
                json!({
                    "range": { "start": convert::lsp_position(rope, d.start), "end": convert::lsp_position(rope, d.end) },
                    "severity": match d.severity { rc_editor::Severity::Error => 1, rc_editor::Severity::Warning => 2, rc_editor::Severity::Info => 3, rc_editor::Severity::Hint => 4 },
                    "code": d.code,
                    "message": d.message,
                })
            })
            .collect();
        let p = session.client.request_raw(
            "textDocument/codeAction",
            json!({ "textDocument": { "uri": path_to_uri(&doc.path).as_str() }, "range": range, "context": { "diagnostics": diags, "triggerKind": 1 } }),
        );
        self.actions = Some(p);
    }

    /// Runs a chosen context action: resolves its edit first when needed.
    pub fn run_code_action(&mut self, docs: &mut Docs, action: lsp::CodeActionOrCommand, out: &mut Vec<InsightOutput>) {
        if let lsp::CodeActionOrCommand::CodeAction(a) = &action
            && a.edit.is_none()
            && a.command.is_none()
            && let Some(session) = &self.session
        {
            self.action_resolve = Some(session.client.request_raw("codeAction/resolve", serde_json::to_value(a).unwrap_or(Value::Null)));
            return;
        }
        self.run_code_action_now(docs, action, out);
    }

    fn run_code_action_now(&mut self, docs: &mut Docs, action: lsp::CodeActionOrCommand, out: &mut Vec<InsightOutput>) {
        let command = match action {
            lsp::CodeActionOrCommand::Command(c) => Some(c),
            lsp::CodeActionOrCommand::CodeAction(a) => {
                if let Some(edit) = &a.edit
                    && let Err(e) = apply_workspace_edit(docs, edit)
                {
                    out.push(InsightOutput::Message(e));
                }
                a.command
            }
        };
        if let (Some(cmd), Some(session)) = (command, &self.session) {
            let _ = session.client.request_raw::<Value>("workspace/executeCommand", json!({ "command": cmd.command, "arguments": cmd.arguments }));
        }
    }

    /// ⌥⌘L: formats the document, or the selection.
    /// Reformats a Rust file that was just saved.
    pub fn format_saved(&mut self, doc: &Doc) {
        if self.rust && self.session.is_some() && doc.editor.selection.primary().is_empty() {
            self.format_on_save = Some(doc.editor.id);
            self.format(doc);
        }
    }

    pub fn format(&mut self, doc: &Doc) {
        let Some(session) = &self.session else { return };
        let rope = doc.editor.buffer.rope();
        let sel = doc.editor.selection.primary();
        let uri = path_to_uri(&doc.path);
        let options = json!({ "tabSize": doc.editor.indent.tab_width, "insertSpaces": doc.editor.indent.unit != "\t" });
        let p = if sel.is_empty() {
            session.client.request_raw("textDocument/formatting", json!({ "textDocument": { "uri": uri.as_str() }, "options": options }))
        } else {
            let range = lsp::Range { start: convert::lsp_position(rope, sel.start()), end: convert::lsp_position(rope, sel.end()) };
            session.client.request_raw("textDocument/rangeFormatting", json!({ "textDocument": { "uri": uri.as_str() }, "range": range, "options": options }))
        };
        self.format = Some((doc.editor.id, (doc.editor.buffer.version(), p)));
    }

    /// `workspace/symbol` for Search Everywhere (`None` when the server is not ready). By
    /// default rust-analyzer returns only types of the workspace; `searchKind` and
    /// `searchScope` (its extension of the request) ask for every symbol and for dependencies.
    pub fn workspace_symbols(&self, query: &str, all_symbols: bool, libraries: bool) -> Option<Pending<Value>> {
        let s = self.session.as_ref().filter(|s| s.is_initialized())?;
        let kind = if all_symbols { "allSymbols" } else { "onlyTypes" };
        let scope = if libraries { "workspaceAndDependencies" } else { "workspace" };
        Some(s.client.request_raw("workspace/symbol", json!({ "query": query, "searchKind": kind, "searchScope": scope })))
    }

    pub fn did_save(&mut self, path: &Path) {
        if let Some(s) = &mut self.session {
            s.did_save(path);
        }
    }

    /// Asks the server for the edits that renaming `from` to `to` needs
    /// (`workspace/willRenameFiles`; rust-analyzer renames the module and its paths). `None`
    /// when no started server offers it.
    pub fn will_rename_files(&self, from: &Path, to: &Path) -> Option<Pending<Option<lsp::WorkspaceEdit>>> {
        let session = self.session.as_ref().filter(|s| s.is_initialized())?;
        let ops = session.capabilities.as_ref()?.workspace.as_ref()?.file_operations.as_ref()?;
        ops.will_rename.as_ref()?;
        Some(session.client.request_raw("workspace/willRenameFiles", json!({ "files": [{ "oldUri": path_to_uri(from).as_str(), "newUri": path_to_uri(to).as_str() }] })))
    }

    /// Tells the server a file or folder was renamed, when it asked to know.
    pub fn did_rename_files(&self, from: &Path, to: &Path) {
        let Some(session) = self.session.as_ref().filter(|s| s.is_initialized()) else { return };
        let wanted = session.capabilities.as_ref().and_then(|c| c.workspace.as_ref()).and_then(|w| w.file_operations.as_ref()).is_some_and(|o| o.did_rename.is_some());
        if wanted {
            session.client.notify_raw("workspace/didRenameFiles", json!({ "files": [{ "oldUri": path_to_uri(from).as_str(), "newUri": path_to_uri(to).as_str() }] }));
        }
    }

    /// Tells the server about files changed on disk (closed files it has loaded).
    pub fn files_changed(&self, changes: &[(PathBuf, u8)]) {
        if let Some(s) = &self.session
            && s.is_initialized()
        {
            s.did_change_watched(changes);
        }
    }
}

/// Items of a completion response with the list's `itemDefaults` (LSP 3.17) folded in: the
/// shared `data` (needed to resolve), insert format and edit range, which Roslyn sends once.
fn parse_completion(v: Value) -> (Vec<lsp::CompletionItem>, Option<lsp::Range>) {
    let (items, defaults) = match v {
        Value::Array(a) => (a, Value::Null),
        Value::Object(mut o) => (
            o.remove("items").and_then(|i| if let Value::Array(a) = i { Some(a) } else { None }).unwrap_or_default(),
            o.remove("itemDefaults").unwrap_or(Value::Null),
        ),
        _ => (Vec::new(), Value::Null),
    };
    let range = defaults.get("editRange").and_then(|r| {
        if r.get("start").is_some() { serde_json::from_value::<lsp::Range>(r.clone()).ok() } else { r.get("replace").and_then(|x| serde_json::from_value(x.clone()).ok()) }
    });
    let items = items
        .into_iter()
        .filter_map(|mut it| {
            if let Value::Object(o) = &mut it {
                for key in ["data", "insertTextFormat", "commitCharacters", "insertTextMode"] {
                    if !o.contains_key(key)
                        && let Some(d) = defaults.get(key)
                    {
                        o.insert(key.to_string(), d.clone());
                    }
                }
            }
            serde_json::from_value::<lsp::CompletionItem>(it).ok()
        })
        .collect();
    (items, range)
}

/// Whether the language server covers this document.
/// Alt+Enter on a rustc or Clippy warning: allow its lint for the statement or item around the
/// caret, or for the whole file.
pub fn suppress_commands(d: &Doc) -> Vec<lsp::CodeActionOrCommand> {
    if d.editor.lang() != rc_syntax::Lang::Rust || d.editor.read_only {
        return Vec::new();
    }
    let line = d.editor.caret_line_col().0;
    let buffer = &d.editor.buffer;
    let mut lints: Vec<String> = d
        .editor
        .diagnostics
        .iter()
        .filter(|x| x.severity != rc_editor::Severity::Error && buffer.line_col(x.start).0 <= line && line <= buffer.line_col(x.end).0)
        .filter_map(|x| x.code.clone())
        .filter(|c| rc_syntax::is_lint_name(c))
        .collect();
    lints.sort();
    lints.dedup();
    let target = d.editor.syntax.tree().and_then(|t| rc_syntax::allow_target(t, buffer.rope(), d.editor.caret()));
    let command = |title: String, lint: &str, scope: &str| lsp::CodeActionOrCommand::Command(lsp::Command { title, command: SUPPRESS_COMMAND.into(), arguments: Some(vec![json!(lint), json!(scope)]) });
    let mut out = Vec::new();
    for lint in &lints {
        match &target {
            Some(t) if t.name.is_empty() => out.push(command(trf("Suppress `{lint}` for the statement", &[("lint", lint)]), lint, "item")),
            Some(t) => out.push(command(trf("Suppress `{lint}` for {name}", &[("lint", lint), ("name", &t.name)]), lint, "item")),
            None => {}
        }
        out.push(command(trf("Suppress `{lint}` for the file", &[("lint", lint)]), lint, "file"));
    }
    out
}

/// The edit a Suppress row makes in `d`: `#[allow]` on the item at the caret, or `#![allow]`.
pub fn suppress_edit(d: &Doc, lint: &str, scope: &str) -> Option<(usize, usize, String)> {
    if !rc_syntax::is_lint_name(lint) {
        return None;
    }
    if scope == "file" {
        return Some(rc_syntax::allow_in_file(lint));
    }
    rc_syntax::allow_target(d.editor.syntax.tree()?, d.editor.buffer.rope(), d.editor.caret())?.add(lint)
}

/// The Rust struct, enum or union the caret of `d` is in.
pub fn derive_target_of(d: &Doc) -> Option<rc_syntax::DeriveTarget> {
    if d.editor.lang() != rc_syntax::Lang::Rust || d.editor.read_only {
        return None;
    }
    rc_syntax::derive_target(d.editor.syntax.tree()?, d.editor.buffer.rope(), d.editor.caret())
}

pub fn has_insight(d: &Doc) -> bool {
    !d.generated && !d.scratch && matches!(d.editor.lang(), rc_syntax::Lang::Rust | rc_syntax::Lang::CSharp)
}

/// Removes Roslyn's markdown escapes that render oddly in our small viewer.
fn clean_hover(s: &str) -> String {
    s.replace("\\.", ".").replace("\\(", "(").replace("\\)", ")").replace("&nbsp;", " ")
}

fn signature_help(h: lsp::SignatureHelp, anchor: usize) -> SignatureHelp {
    let active = h.active_signature.unwrap_or(0) as usize;
    let signatures = h
        .signatures
        .iter()
        .map(|s| {
            let label_chars: Vec<char> = s.label.chars().collect();
            let params = s
                .parameters
                .clone()
                .unwrap_or_default()
                .iter()
                .map(|p| match &p.label {
                    lsp::ParameterLabel::LabelOffsets([a, b]) => {
                        // Offsets are UTF-16 units into the label.
                        let to_char = |u: u32| s.label.encode_utf16().take(u as usize).count().min(label_chars.len());
                        let a16: String = String::from_utf16_lossy(&s.label.encode_utf16().take(*a as usize).collect::<Vec<u16>>());
                        let b16: String = String::from_utf16_lossy(&s.label.encode_utf16().take(*b as usize).collect::<Vec<u16>>());
                        let _ = to_char;
                        (a16.chars().count(), b16.chars().count())
                    }
                    lsp::ParameterLabel::Simple(text) => s.label.find(text.as_str()).map_or((0, 0), |i| {
                        let a = s.label[..i].chars().count();
                        (a, a + text.chars().count())
                    }),
                })
                .collect();
            let active_param = s.active_parameter.or(h.active_parameter).map(|p| p as usize);
            Signature { label: s.label.clone(), params, active_param, doc: s.documentation.as_ref().map(convert::documentation) }
        })
        .collect();
    SignatureHelp { anchor, signatures, active }
}

/// Offset of the `(` of the innermost call around `caret` (skipping nested parentheses).
pub fn call_open_paren(rope: &rc_text::Rope, caret: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = caret;
    let mut scanned = 0;
    while i > 0 && scanned < 4000 {
        i -= 1;
        scanned += 1;
        match rope.char(i) {
            ')' => depth += 1,
            '(' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            ';' | '{' | '}' => return None,
            _ => {}
        }
    }
    None
}

fn inside_call(rope: &rc_text::Rope, anchor: usize, caret: usize) -> bool {
    call_open_paren(rope, caret) == Some(anchor)
}

/// Locations from a definition/references response (Location, Location[] or LocationLink[]).
fn locations(v: &Value, docs: &Docs) -> Vec<Target> {
    let mut out = Vec::new();
    let mut push = |uri: &str, range: &Value| {
        let Ok(uri) = uri.parse::<lsp::Uri>() else { return };
        let Some(path) = uri_to_path(&uri) else { return };
        let Ok(r) = serde_json::from_value::<lsp::Range>(range.clone()) else { return };
        // Open documents know their exact text; others are read from disk.
        let (line, col, preview) = match docs.list.iter().find(|d| d.path == path) {
            Some(d) => {
                let rope = d.editor.buffer.rope();
                let pos = convert::char_of(rope, r.start);
                let (l, c) = d.editor.buffer.line_col(pos);
                (l, c, d.editor.buffer.line_text(l))
            }
            None => {
                let text = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
                let rope = rc_text::Rope::from_str(&text);
                let pos = convert::char_of(&rope, r.start);
                let l = rope.char_to_line(pos);
                let c = pos - rope.line_to_char(l);
                let mut t = rope.line(l).to_string();
                if t.ends_with('\n') {
                    t.pop();
                }
                (l, c, t)
            }
        };
        out.push(Target { path, line, col, preview });
    };
    let items: Vec<Value> = match v {
        Value::Array(a) => a.clone(),
        Value::Null => Vec::new(),
        other => vec![other.clone()],
    };
    for it in items {
        if let (Some(uri), Some(range)) = (it.get("uri").and_then(Value::as_str), it.get("range")) {
            push(uri, range);
        } else if let (Some(uri), Some(range)) = (it.get("targetUri").and_then(Value::as_str), it.get("targetSelectionRange")) {
            push(uri, range);
        }
    }
    out.sort_by(|a, b| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));
    out.dedup();
    out
}

fn symbols(rope: &rc_text::Rope, resp: lsp::DocumentSymbolResponse) -> Vec<SymbolNode> {
    fn conv(rope: &rc_text::Rope, s: lsp::DocumentSymbol) -> SymbolNode {
        let pos = convert::char_of(rope, s.selection_range.start);
        let line = rope.char_to_line(pos);
        SymbolNode {
            name: s.name,
            detail: s.detail.unwrap_or_default(),
            kind: s.kind,
            line,
            col: pos - rope.line_to_char(line),
            lines: rope.char_to_line(convert::char_of(rope, s.range.start))..=rope.char_to_line(convert::char_of(rope, s.range.end)),
            children: s.children.unwrap_or_default().into_iter().map(|c| conv(rope, c)).collect(),
        }
    }
    match resp {
        lsp::DocumentSymbolResponse::Nested(list) => list.into_iter().map(|s| conv(rope, s)).collect(),
        lsp::DocumentSymbolResponse::Flat(list) => list
            .into_iter()
            .map(|s| {
                let pos = convert::char_of(rope, s.location.range.start);
                let line = rope.char_to_line(pos);
                SymbolNode { name: s.name, detail: s.container_name.unwrap_or_default(), kind: s.kind, line, col: pos - rope.line_to_char(line), lines: line..=line, children: Vec::new() }
            })
            .collect(),
    }
}

/// Assists of rust-analyzer that make up Optimize Imports, in order.
const OPTIMIZE_STEPS: [&str; 2] = ["Remove all unused imports", "Merge imports"];

/// The macro's name and its expansion in a `rust-analyzer/expandMacro` reply (`null` when the
/// caret is not on a macro call).
fn macro_expansion(v: &Value) -> Option<(String, String)> {
    // rust-analyzer writes the name with its `!`.
    let name = v.get("name")?.as_str()?.trim_end_matches('!').to_string();
    // On Windows the expansion comes with `\r\n`, and with blank lines around it.
    let text = v.get("expansion")?.as_str()?.replace("\r\n", "\n").trim_matches('\n').to_string();
    Some((name, text))
}

/// The address in an `experimental/externalDocs` reply: a string, or `{ web, local }`.
fn web_docs(reply: &Value) -> Option<String> {
    let url = reply.as_str().or_else(|| reply.get("web")?.as_str())?;
    (url.starts_with("https://") || url.starts_with("http://")).then(|| url.to_string())
}

/// The word of `line` around the character `col`.
fn word_at(line: &str, col: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
    let col = col.min(chars.len());
    let start = chars[..col].iter().rev().take_while(|c| is_word(c)).count();
    let end = chars[col..].iter().take_while(|c| is_word(c)).count();
    chars[col - start..col + end].iter().collect()
}

/// `impl Speak for A {` as a title.
fn impl_title(line: &str) -> String {
    line.trim().trim_end_matches('{').trim().to_string()
}

/// Optimize Imports in progress.
struct Optimizing {
    doc: u64,
    steps: Vec<&'static str>,
    changed: bool,
}

/// The characters from the first top-level `use` to the end of the last one.
fn use_block(text: &str) -> Option<(usize, usize)> {
    let is_use = |line: &str| line.starts_with("use ") || line.starts_with("pub use ") || line.starts_with("pub(crate) use ");
    let (mut first, mut last) = (None, None);
    let mut at = 0;
    let mut inside = false;
    for line in text.split_inclusive('\n') {
        let chars = line.chars().count();
        if is_use(line) || inside {
            first = first.or(Some(at));
            inside = !line.contains(';');
            last = Some(at + line.trim_end().chars().count());
        }
        at += chars;
    }
    first.zip(last)
}

/// Applies a workspace edit: open documents through their editor (one undo step each),
/// closed files on disk. Returns the number of files changed.
pub fn apply_workspace_edit(docs: &mut Docs, edit: &lsp::WorkspaceEdit) -> Result<usize, String> {
    apply_file_edits(docs, workspace_edit_files(edit))
}

/// The text edits of a workspace edit, per file (file operations are left out).
pub fn workspace_edit_files(edit: &lsp::WorkspaceEdit) -> BTreeMap<PathBuf, Vec<lsp::TextEdit>> {
    let mut per_file: BTreeMap<PathBuf, Vec<lsp::TextEdit>> = BTreeMap::new();
    if let Some(changes) = &edit.changes {
        for (uri, edits) in changes {
            if let Some(p) = uri_to_path(uri) {
                per_file.entry(p).or_default().extend(edits.iter().cloned());
            }
        }
    }
    if let Some(dc) = &edit.document_changes {
        let edits: Vec<&lsp::TextDocumentEdit> = match dc {
            lsp::DocumentChanges::Edits(e) => e.iter().collect(),
            lsp::DocumentChanges::Operations(ops) => ops
                .iter()
                .filter_map(|o| match o {
                    lsp::DocumentChangeOperation::Edit(e) => Some(e),
                    lsp::DocumentChangeOperation::Op(_) => None,
                })
                .collect(),
        };
        for e in edits {
            if let Some(p) = uri_to_path(&e.text_document.uri) {
                per_file.entry(p).or_default().extend(e.edits.iter().map(|x| match x {
                    lsp::OneOf::Left(t) => t.clone(),
                    lsp::OneOf::Right(a) => a.text_edit.clone(),
                }));
            }
        }
    }
    per_file
}

/// Applies text edits per file: open documents through their editor (one undo step each),
/// closed files on disk. Returns the number of files changed.
pub fn apply_file_edits(docs: &mut Docs, per_file: BTreeMap<PathBuf, Vec<lsp::TextEdit>>) -> Result<usize, String> {
    let mut count = 0;
    for (path, edits) in per_file {
        if let Some(d) = docs.list.iter_mut().find(|d| d.path == path) {
            let rope = d.editor.buffer.rope().clone();
            let changes = edits
                .iter()
                .map(|e| {
                    let (a, b) = convert::range_chars(&rope, &e.range);
                    rc_text::Change::new(a, b, e.new_text.replace("\r\n", "\n"))
                })
                .collect();
            d.editor.apply_text_edits(changes);
        } else {
            let (text, bom, _) = crate::docs::read_text(&path)?;
            let mut buf = rc_text::Buffer::new(&text);
            let rope = buf.rope().clone();
            let changes: Vec<rc_text::Change> = edits
                .iter()
                .map(|e| {
                    let (a, b) = convert::range_chars(&rope, &e.range);
                    rc_text::Change::new(a, b, e.new_text.replace("\r\n", "\n"))
                })
                .collect();
            buf.apply(&rc_text::ChangeSet::new(changes), &rc_text::Selection::point(0), &rc_text::Selection::point(0), rc_text::EditKind::Other);
            let mut bytes = Vec::new();
            if bom {
                bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            bytes.extend_from_slice(buf.to_disk_string().as_bytes());
            rc_core::paths::write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
        }
        count += 1;
    }
    Ok(count)
}

/// Applies an auto-insert edit: a snippet whose `$0` (or `${0}`) marks where the caret goes.
fn apply_auto_insert(d: &mut Doc, edit: &lsp::TextEdit) {
    let rope = d.editor.buffer.rope().clone();
    let (a, b) = convert::range_chars(&rope, &edit.range);
    let text = edit.new_text.replace("\r\n", "\n");
    let (text, caret) = match text.find("$0").map(|i| (i, 2)).or_else(|| text.find("${0}").map(|i| (i, 4))) {
        Some((i, n)) => {
            let caret = a + text[..i].chars().count();
            (format!("{}{}", &text[..i], &text[i + n..]), caret)
        }
        None => {
            let n = text.chars().count();
            (text, a + n)
        }
    };
    d.editor.apply_text_edits(vec![rc_text::Change::new(a, b, text)]);
    d.editor.selection = rc_text::Selection::point(caret);
}

/// The namespace of the only `using X;` fix among `actions` (Roslyn titles them exactly so);
/// `None` when there is none or the name is ambiguous.
fn single_using(actions: &Value) -> Option<String> {
    let mut found: Vec<String> = actions
        .as_array()?
        .iter()
        .filter_map(|a| a.get("title")?.as_str())
        .filter_map(|t| t.strip_prefix("using ")?.strip_suffix(';').map(str::to_string))
        .filter(|ns| !ns.contains(' ') && !ns.contains('='))
        .collect();
    found.sort();
    found.dedup();
    (found.len() == 1).then(|| found.remove(0))
}

/// Adds `using ns;` lines that are missing, in order among the file's usings (or at the top).
fn insert_usings(d: &mut Doc, namespaces: &[String]) {
    let rope = d.editor.buffer.rope().clone();
    let lines: Vec<String> = (0..rope.len_lines()).map(|l| rope.line(l).to_string()).collect();
    let is_using = |l: &str| {
        let t = l.trim();
        t.starts_with("using ") && t.ends_with(';') && !t.contains('=') && !t.starts_with("using static") && !t.contains('(')
    };
    let existing: Vec<String> = lines.iter().filter(|l| is_using(l)).map(|l| l.trim().trim_start_matches("using ").trim_end_matches(';').to_string()).collect();
    let mut add: Vec<&String> = namespaces.iter().filter(|n| !existing.contains(n)).collect();
    add.sort();
    add.dedup();
    if add.is_empty() {
        return;
    }
    let mut changes = Vec::new();
    match lines.iter().rposition(|l| is_using(l)) {
        Some(_) => {
            // Each new using goes before the first existing one that sorts after it (System first).
            let key = |n: &str| (!(n == "System" || n.starts_with("System.")), n.to_string());
            for n in add {
                let at = lines.iter().position(|l| is_using(l) && key(l.trim().trim_start_matches("using ").trim_end_matches(';')) > key(n));
                let line = at.unwrap_or_else(|| lines.iter().rposition(|l| is_using(l)).unwrap() + 1);
                changes.push(rc_text::Change::insert(rope.line_to_char(line), format!("using {n};\n")));
            }
        }
        None => {
            let text: String = add.iter().map(|n| format!("using {n};\n")).collect();
            changes.push(rc_text::Change::insert(0, format!("{text}\n")));
        }
    }
    d.editor.apply_text_edits(changes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_paren_skips_nested_calls() {
        let rope = rc_text::Rope::from_str("Foo(a, Bar(b), c");
        assert_eq!(call_open_paren(&rope, 16), Some(3));
        assert_eq!(call_open_paren(&rope, 12), Some(10));
        assert_eq!(call_open_paren(&rc_text::Rope::from_str("x; y"), 4), None);
    }
}

#[cfg(test)]
mod symbols_at_tests {
    use super::{SymbolNode, symbols_at};

    fn node(name: &str, lines: std::ops::RangeInclusive<usize>, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode { name: name.into(), detail: String::new(), kind: rc_lsp::lsp_types::SymbolKind::FUNCTION, line: *lines.start(), col: 0, lines, children }
    }

    #[test]
    fn the_path_follows_the_items_around_the_line() {
        let tree = [node("a", 0..=1, vec![]), node("impl B", 3..=9, vec![node("f", 4..=5, vec![]), node("g", 7..=8, vec![])])];
        let names = |line| symbols_at(&tree, line).iter().map(|n| n.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(1), ["a"]);
        assert_eq!(names(2), [""; 0]);
        assert_eq!(names(6), ["impl B"]);
        assert_eq!(names(8), ["impl B", "g"]);
    }
}

#[cfg(test)]
mod web_docs_tests {
    use super::web_docs;
    use serde_json::json;

    #[test]
    fn only_web_addresses_are_opened() {
        assert_eq!(web_docs(&json!("https://docs.rs/a")).as_deref(), Some("https://docs.rs/a"));
        assert_eq!(web_docs(&json!({ "web": "https://docs.rs/b", "local": "file:///x" })).as_deref(), Some("https://docs.rs/b"));
        assert_eq!(web_docs(&json!("file:///etc/passwd")), None);
        assert_eq!(web_docs(&json!(null)), None);
    }
}

#[cfg(test)]
mod type_text_tests {
    use super::{impl_title, word_at};

    #[test]
    fn titles_and_words_come_from_the_line() {
        assert_eq!(impl_title("    impl Speak for A {"), "impl Speak for A");
        assert_eq!(word_at("trait Speak {", 8), "Speak");
        assert_eq!(word_at("trait Speak {", 11), "Speak");
        assert_eq!(word_at("a b", 99), "b");
        assert_eq!(word_at("", 0), "");
    }
}

#[cfg(test)]
mod use_block_tests {
    use super::use_block;

    #[test]
    fn the_block_goes_from_the_first_use_to_the_last() {
        let text = "//! Docs.\nuse a::b;\n\nmod m;\npub use c::{\n    d,\n};\nfn f() {\n    use e;\n}\n";
        let (start, end) = use_block(text).unwrap();
        assert_eq!(&text[start..end], "use a::b;\n\nmod m;\npub use c::{\n    d,\n};");
        assert_eq!(use_block("fn f() {}\n"), None);
    }
}
