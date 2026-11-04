//! Search: the file index, Search Everywhere (classes, files, symbols, actions, text), the editor's
//! find/replace bar and Find in Files.

use rc_core::{Inbox, Waker};
use rc_editor::Action;
use rc_kit::i18n::tr;
use rc_lsp::Pending;
use rc_search::{FileIndex, FindEvent, FindOptions, FindSummary, Finder, FileMatches, QueryOptions, ScanHandle};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeTab {
    All,
    /// Structs, enums, traits, unions and type aliases.
    Classes,
    Files,
    Symbols,
    Actions,
    /// Full-text search of the project's files.
    Text,
}

impl SeTab {
    pub const ALL: [SeTab; 6] = [SeTab::All, SeTab::Classes, SeTab::Files, SeTab::Symbols, SeTab::Actions, SeTab::Text];

    pub fn title(self) -> &'static str {
        match self {
            SeTab::All => tr("All"),
            SeTab::Classes => tr("Classes"),
            SeTab::Files => tr("Files"),
            SeTab::Symbols => tr("Symbols"),
            SeTab::Actions => tr("Actions"),
            SeTab::Text => tr("Text"),
        }
    }

    /// The action that opens the popup on this tab (its shortcut is shown as a hint).
    pub fn action(self) -> Option<Action> {
        match self {
            SeTab::All => Some(Action::SearchEverywhere),
            SeTab::Classes => Some(Action::GotoClass),
            SeTab::Files => Some(Action::GotoFile),
            SeTab::Symbols => Some(Action::GotoSymbol),
            SeTab::Actions => Some(Action::FindAction),
            SeTab::Text => None,
        }
    }
}

/// Where Search Everywhere looks.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum SeScope {
    /// The workspace's own files and symbols.
    #[default]
    Project,
    /// Also the symbols of dependencies and the standard library.
    ProjectAndLibraries,
    /// One package of the Cargo workspace: its name and folder relative to the root
    /// (`/`-separated, empty for a package at the root).
    Package { name: String, rel: String },
}

impl SeScope {
    pub fn title(&self) -> String {
        match self {
            SeScope::Project => tr("Workspace").to_string(),
            SeScope::ProjectAndLibraries => tr("Workspace and Dependencies").to_string(),
            SeScope::Package { name, .. } => rc_kit::i18n::trf("Crate {name}", &[("name", name)]),
        }
    }

    pub fn libraries(&self) -> bool {
        *self == SeScope::ProjectAndLibraries
    }
}

/// The packages of a Cargo workspace as `(name, folder relative to root)`, by folder.
pub fn package_scopes(root: &Path, workspace: Option<&rc_cargo::CargoWorkspace>) -> Vec<(String, String)> {
    let Some(w) = workspace else { return Vec::new() };
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root_c = canon(root);
    let mut out: Vec<(String, String)> = w
        .packages
        .iter()
        .filter_map(|p| {
            let dir = p.manifest.parent()?;
            let rel = dir.strip_prefix(root).ok().map(Path::to_path_buf).or_else(|| canon(dir).strip_prefix(&root_c).ok().map(Path::to_path_buf))?;
            Some((p.name.clone(), rel_string(&rel)))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup();
    out
}

fn rel_string(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

/// The package whose folder holds `rel` (the innermost one when packages are nested).
fn owner<'a>(packages: &'a [(String, String)], rel: &str) -> Option<&'a str> {
    packages
        .iter()
        .filter(|(_, dir)| dir.is_empty() || rel == dir || rel.strip_prefix(dir.as_str()).is_some_and(|r| r.starts_with('/')))
        .max_by_key(|(_, dir)| if dir.is_empty() { 0 } else { dir.split('/').count() })
        .map(|(name, _)| name.as_str())
}

/// A line found by the Text tab.
#[derive(Clone, Debug, PartialEq)]
pub struct TextHit {
    pub path: PathBuf,
    pub rel: String,
    /// 0-based line and char column of the first match.
    pub line: usize,
    pub col: usize,
    /// The line, trimmed and shortened around the match.
    pub preview: String,
    /// Chars of `preview` inside a match.
    pub highlight: Vec<usize>,
}

/// Lines kept by the Text tab.
pub const MAX_TEXT_HITS: usize = 100;
/// Time after the last keystroke before the Text tab searches.
const TEXT_DEBOUNCE: f64 = 0.15;

fn text_hit(file: &FileMatches, line: &rc_search::LineMatch) -> TextHit {
    let chars: Vec<char> = line.text.chars().map(|c| if c == '\t' { ' ' } else { c }).collect();
    let lead = chars.iter().take_while(|c| c.is_whitespace()).count();
    let first = line.ranges.first().map_or(0, |r| r.start as usize);
    let start = if first.saturating_sub(lead) > 60 { first - 30 } else { lead.min(first) };
    let end = chars.len().min(start + 120);
    let start = start.min(end);
    let mut preview: String = chars[start..end].iter().collect();
    let mut shift = 0;
    if start > lead {
        preview.insert(0, '…');
        shift = 1;
    }
    let highlight = line.ranges.iter().flat_map(|r| r.start as usize..r.end as usize).filter(|k| (start..end).contains(k)).map(|k| k - start + shift).collect();
    TextHit { path: file.path.clone(), rel: file.rel.clone(), line: line.line as usize, col: first, preview, highlight }
}

/// A symbol from `workspace/symbol`.
#[derive(Clone, Debug, PartialEq)]
pub struct SymbolHit {
    pub name: String,
    pub container: String,
    pub kind: rc_lsp::lsp_types::SymbolKind,
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

impl SymbolHit {
    pub fn is_type(&self) -> bool {
        use rc_lsp::lsp_types::SymbolKind as K;
        matches!(self.kind, K::CLASS | K::INTERFACE | K::STRUCT | K::ENUM | K::TYPE_PARAMETER)
    }
}

/// The Text tab's search: one [`Finder`] whose stale events are dropped.
struct TextSearch {
    finder: Finder,
    inbox: Inbox<FindEvent>,
}

pub struct SearchEverywhere {
    pub text: String,
    pub tab: SeTab,
    pub selected: usize,
    /// Scroll the results to the selected row on the next frame. Set when the keyboard or a new
    /// query moves the selection, so a wheel scroll stays where the user left it.
    pub reveal: bool,
    pub focus: bool,
    pub scope: SeScope,
    /// The Cargo packages offered as scopes (only with two or more).
    pub packages: Vec<(String, String)>,
    pub symbols: Vec<SymbolHit>,
    /// Query, all symbols (not only types) and libraries of the last request.
    symbol_query: Option<(String, bool, bool)>,
    symbol_pending: Option<Pending<Value>>,
    pub text_hits: Vec<TextHit>,
    /// Query and scope of the running or finished text search.
    text_started: Option<(String, SeScope)>,
    /// Query and scope typed but not searched yet, with when they changed.
    text_typed: Option<((String, SeScope), f64)>,
    text_done: bool,
    text_search: Option<TextSearch>,
}

impl SearchEverywhere {
    pub fn new(tab: SeTab, text: String) -> Self {
        SearchEverywhere {
            text,
            tab,
            selected: 0,
            reveal: false,
            focus: true,
            scope: SeScope::Project,
            packages: Vec::new(),
            symbols: Vec::new(),
            symbol_query: None,
            symbol_pending: None,
            text_hits: Vec::new(),
            text_started: None,
            text_typed: None,
            text_done: false,
            text_search: None,
        }
    }

    /// Opens with a remembered scope; a package that is gone falls back to the project.
    pub fn with_scope(mut self, scope: SeScope, packages: Vec<(String, String)>) -> Self {
        self.packages = if packages.len() >= 2 { packages } else { Vec::new() };
        self.scope = match scope {
            SeScope::Package { name, rel } if self.packages.iter().any(|(n, r)| *n == name && *r == rel) => SeScope::Package { name, rel },
            SeScope::Package { .. } => SeScope::Project,
            other => other,
        };
        self
    }

    /// The scopes offered by the selector.
    pub fn scopes(&self) -> Vec<SeScope> {
        let mut v = vec![SeScope::Project, SeScope::ProjectAndLibraries];
        v.extend(self.packages.iter().map(|(name, rel)| SeScope::Package { name: name.clone(), rel: rel.clone() }));
        v
    }

    /// Whether a root-relative path (`/`-separated) is inside the scope.
    pub fn in_scope(&self, rel: &str) -> bool {
        match &self.scope {
            SeScope::Package { name, .. } => owner(&self.packages, rel) == Some(name.as_str()),
            _ => true,
        }
    }

    /// Whether a symbol is inside the scope (`root` is the workspace folder).
    pub fn symbol_in_scope(&self, root: &Path, path: &Path) -> bool {
        if !matches!(self.scope, SeScope::Package { .. }) {
            return true;
        }
        let rel = path.strip_prefix(root).ok().map(Path::to_path_buf).or_else(|| root.canonicalize().ok().and_then(|r| path.strip_prefix(r).ok().map(Path::to_path_buf)));
        rel.is_some_and(|r| self.in_scope(&rel_string(&r)))
    }

    /// Asks the language server for symbols matching the text (latest query only).
    pub fn update_symbols(&mut self, insight: &crate::insight::Insight) {
        if let Some(p) = &mut self.symbol_pending
            && let Some(r) = p.poll()
        {
            self.symbol_pending = None;
            self.symbols = r.map(|v| parse_symbols(&v)).unwrap_or_default();
        }
        let wants = matches!(self.tab, SeTab::All | SeTab::Classes | SeTab::Symbols) && self.text.trim().len() >= 2;
        let key = (self.text.trim().to_string(), self.tab != SeTab::Classes, self.scope.libraries());
        if wants && self.symbol_query.as_ref() != Some(&key) && self.symbol_pending.is_none() {
            self.symbol_pending = insight.workspace_symbols(&key.0, key.1, key.2);
            self.symbol_query = Some(key);
        }
        if !wants {
            self.symbols.clear();
            self.symbol_query = None;
        }
    }

    /// Runs the Text tab's search off the UI thread, `TEXT_DEBOUNCE` after the last change;
    /// a new query cancels the running one. Returns whether it waits for something.
    pub fn update_text(&mut self, root: &Path, waker: &Waker, now: f64) -> bool {
        if let Some(t) = &self.text_search {
            let current = t.finder.current();
            for ev in t.inbox.drain() {
                if Some(ev.search()) != current {
                    continue;
                }
                match ev {
                    FindEvent::File { file, .. } => {
                        if self.text_hits.len() < MAX_TEXT_HITS && self.in_scope(&file.rel) {
                            self.text_hits.extend(file.lines.iter().map(|l| text_hit(&file, l)));
                            self.text_hits.sort_by(|a, b| (&a.rel, a.line).cmp(&(&b.rel, b.line)));
                            self.text_hits.truncate(MAX_TEXT_HITS);
                        }
                    }
                    FindEvent::Done { .. } => self.text_done = true,
                }
            }
        }
        let query = self.text.trim().to_string();
        if self.tab != SeTab::Text || query.is_empty() {
            if let Some(t) = &mut self.text_search {
                t.finder.cancel();
            }
            self.text_hits.clear();
            self.text_started = None;
            self.text_typed = None;
            self.text_done = false;
            return false;
        }
        let key = (query, self.scope.clone());
        if self.text_started.as_ref() == Some(&key) {
            self.text_typed = None;
            return !self.text_done;
        }
        let since = match &self.text_typed {
            Some((k, t)) if *k == key => *t,
            _ => {
                self.text_typed = Some((key, now));
                return true;
            }
        };
        if now - since < TEXT_DEBOUNCE {
            return true;
        }
        let search = self.text_search.get_or_insert_with(|| {
            let inbox = Inbox::new(waker.clone());
            TextSearch { finder: Finder::new(root, inbox.sender()), inbox }
        });
        let scope = match &key.1 {
            SeScope::Package { rel, .. } if !rel.is_empty() => Some(root.join(rel)),
            _ => None,
        };
        let opts = FindOptions { query: QueryOptions::text(&key.0), scope, max_results: MAX_TEXT_HITS * 2, ..Default::default() };
        self.text_hits.clear();
        self.text_done = search.finder.start(opts).is_err();
        self.text_started = Some(key);
        self.text_typed = None;
        true
    }

    /// The text search is still running.
    pub fn text_searching(&self) -> bool {
        self.tab == SeTab::Text && !self.text_done && (self.text_started.is_some() || self.text_typed.is_some())
    }
}

fn parse_symbols(v: &Value) -> Vec<SymbolHit> {
    let Some(list) = v.as_array() else { return Vec::new() };
    list.iter()
        .filter_map(|s| {
            let name = s.get("name")?.as_str()?.to_string();
            let kind: rc_lsp::lsp_types::SymbolKind = serde_json::from_value(s.get("kind")?.clone()).ok()?;
            let loc = s.get("location")?;
            let uri: rc_lsp::lsp_types::Uri = loc.get("uri")?.as_str()?.parse().ok()?;
            let path = rc_lsp::uri_to_path(&uri)?;
            let (line, col) = loc
                .get("range")
                .and_then(|r| r.get("start"))
                .map(|p| (p.get("line").and_then(Value::as_u64).unwrap_or(0) as usize, p.get("character").and_then(Value::as_u64).unwrap_or(0) as usize))
                .unwrap_or((0, 0));
            Some(SymbolHit { name, container: s.get("containerName").and_then(Value::as_str).unwrap_or("").to_string(), kind, path, line, col })
        })
        .collect()
}

/// The IDE actions offered by Search Everywhere's Actions tab.
pub fn action_items() -> Vec<(&'static str, Action)> {
    use Action::*;
    vec![
        (tr("Build Project"), Build),
        (tr("Run"), Run),
        (tr("Debug"), Debug),
        (tr("Stop"), StopRun),
        (tr("Run Tests"), RunTests),
        (tr("Attach to Process…"), AttachToProcess),
        (tr("Force Step Into"), ForceStepInto),
        (tr("Force Step Over"), ForceStepOver),
        (tr("Force Run to Cursor"), ForceRunToCursor),
        (tr("Show Integers in Hex"), ToggleHexValues),
        (tr("Settings…"), ShowSettings),
        (tr("Project Environment Variables…"), ProjectEnvironment),
        (tr("Find in Files…"), FindInFiles),
        (tr("Replace in Files…"), ReplaceInFiles),
        (tr("Go to File…"), GotoFile),
        (tr("Go to Type…"), GotoClass),
        (tr("Go to Symbol…"), GotoSymbol),
        (tr("Go to Line…"), GotoLine),
        (tr("Recent Files"), RecentFiles),
        (tr("Invalidate Caches…"), InvalidateCaches),
        (tr("Inspect Code"), InspectCode),
        (tr("Update Copyright"), UpdateCopyright),
        (tr("New Scratch File…"), NewScratchFile),
        (tr("Select In…"), SelectIn),
        (tr("Reformat Code"), Reformat),
        (tr("Share in Playground…"), ShareInPlayground),
        (tr("Generate…"), Generate),
        (tr("Refactor This…"), RefactorThis),
        (tr("Go to Declaration"), GotoDeclaration),
        (tr("Go to Implementation"), GotoImplementation),
        (tr("Go to Type Declaration"), GotoTypeDeclaration),
        (tr("Go to Super Method"), GotoSuperMethod),
        (tr("Find Usages"), FindUsages),
        (tr("Show Callers"), ShowCallers),
        (tr("Show Callees"), ShowCallees),
        (tr("File Structure"), FileStructure),
        (tr("Show Context Info"), ShowContextInfo),
        (tr("Explorer"), ToggleProjectWindow),
        (tr("Terminal"), ToggleTerminal),
        (tr("Problems"), ToggleProblems),
        (tr("Git"), ToggleGitWindow),
        (tr("Commit…"), Commit),
        (tr("Show Local History"), ShowLocalHistory),
        (tr("Hide All Tool Windows"), HideToolWindows),
        (tr("Save All"), SaveAll),
        (tr("Zoom In"), ZoomIn),
        (tr("Zoom Out"), ZoomOut),
        (tr("Reset Zoom"), ZoomReset),
        (tr("Collapse All"), FoldAll),
        (tr("Expand All"), UnfoldAll),
    ]
}

/// Another name an action is found by: what other editors call it.
pub fn action_synonym(action: Action) -> Option<&'static str> {
    match action {
        Action::GotoDeclaration => Some(tr("Go to Definition")),
        _ => None,
    }
}

/// The workspace file index and its background scan.
pub struct Files {
    pub index: Option<FileIndex>,
    _scan: Option<ScanHandle>,
}

impl Files {
    pub fn new() -> Self {
        Files { index: None, _scan: None }
    }

    /// Indexes `root` in the background, leaving out the `excluded` folders and files; an index
    /// already open is replaced (its scan is cancelled).
    pub fn open(&mut self, root: &Path, waker: Waker, excluded: Vec<String>) {
        let index = FileIndex::with_excluded(root, excluded);
        self._scan = Some(index.start_scan(waker));
        self.index = Some(index);
    }

    pub fn close(&mut self) {
        self.index = None;
        self._scan = None;
    }
}

/// The editor's find / replace bar.
#[derive(Clone, Debug, Default)]
pub struct FindBar {
    pub query: String,
    pub replace: Option<String>,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    pub error: Option<String>,
    pub focus: bool,
    /// Query and document version the matches were computed for.
    pub computed: Option<(String, bool, bool, bool, u64, u64)>,
}

impl FindBar {
    pub fn options(&self) -> QueryOptions {
        QueryOptions { pattern: self.query.clone(), regex: self.regex, case_sensitive: self.case, whole_word: self.word }
    }
}

/// Find in Files (the Find tool window's text search).
pub struct FindInFiles {
    pub query: String,
    pub replace: String,
    pub show_replace: bool,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    pub mask: String,
    /// Folders and files the user excluded (root-relative); never searched.
    pub excluded: Vec<String>,
    pub results: Vec<FileMatches>,
    pub summary: Option<FindSummary>,
    pub error: Option<String>,
    pub focus: bool,
    finder: Option<Finder>,
    inbox: Inbox<FindEvent>,
    root: PathBuf,
}

impl FindInFiles {
    pub fn new(root: &Path, waker: Waker, query: String) -> Self {
        let inbox = Inbox::new(waker);
        let finder = Some(Finder::new(root, inbox.sender()));
        FindInFiles {
            query,
            replace: String::new(),
            show_replace: false,
            case: false,
            word: false,
            regex: false,
            mask: String::new(),
            excluded: Vec::new(),
            results: Vec::new(),
            summary: None,
            error: None,
            focus: true,
            finder,
            inbox,
            root: root.to_path_buf(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn options(&self) -> QueryOptions {
        QueryOptions { pattern: self.query.clone(), regex: self.regex, case_sensitive: self.case, whole_word: self.word }
    }

    /// Starts (or restarts) the search with the current fields.
    pub fn search(&mut self) {
        self.results.clear();
        self.summary = None;
        self.error = None;
        if self.query.is_empty() {
            if let Some(f) = &mut self.finder {
                f.cancel();
            }
            return;
        }
        let mask = match rc_search::FileMask::parse(&self.mask) {
            Ok(m) => m,
            Err(e) => {
                self.error = Some(format!("{e:?}"));
                return;
            }
        };
        let opts = FindOptions { query: self.options(), mask, max_results: 5000, excluded: self.excluded.clone(), ..Default::default() };
        if let Some(f) = &mut self.finder
            && let Err(e) = f.start(opts)
        {
            self.error = Some(format!("{e:?}"));
        }
    }

    pub fn searching(&self) -> bool {
        self.summary.is_none() && self.finder.as_ref().is_some_and(|f| f.current().is_some()) && !self.query.is_empty() && self.error.is_none()
    }

    pub fn poll(&mut self) {
        let current = self.finder.as_ref().and_then(|f| f.current());
        for ev in self.inbox.drain() {
            if Some(ev.search()) != current {
                continue;
            }
            match ev {
                FindEvent::File { file, .. } => self.results.push(file),
                FindEvent::Done { summary, .. } => self.summary = Some(summary),
            }
        }
    }

    pub fn match_count(&self) -> usize {
        self.results.iter().map(|f| f.match_count()).sum()
    }
}

/// The icon of an IDE action, shared by the native menu, popup menus and Search Everywhere.
pub fn action_icon(action: Action) -> Option<rc_kit::Ico> {
    use Action::*;
    use rc_kit::ico;
    Some(match action {
        Build => ico::BUILD,
        Run => ico::RUN,
        Debug => ico::DEBUG,
        StopRun => ico::STOP,
        RunTests => ico::RUN_ALL,
        StepOver | ForceStepOver => ico::STEP_OVER,
        StepInto | ForceStepInto => ico::STEP_INTO,
        StepOut => ico::STEP_OUT,
        RunToCursor | ForceRunToCursor => ico::RUN_TO_CURSOR,
        AttachToProcess => ico::DEBUG,
        Resume => ico::RESUME,
        Evaluate => ico::EVALUATE,
        ToggleHexValues => ico::VAR_PRIMITIVE,
        ToggleBreakpoint => ico::BREAKPOINT,
        ToggleBookmark | ShowBookmarks => ico::BOOKMARK,
        ShowSettings | ProjectEnvironment => ico::SETTINGS,
        Find | SearchEverywhere => ico::SEARCH,
        Replace | ReplaceInFiles => ico::REPLACE,
        FindInFiles => ico::TW_FIND,
        GotoFile => ico::FILE_TEXT,
        InvalidateCaches => ico::REFRESH,
        InspectCode => ico::WARNING,
        UpdateCopyright => ico::EDIT,
        NewScratchFile => ico::NEW_FILE,
        GotoClass => ico::CLASS,
        GotoSymbol => ico::METHOD,
        GotoLine => ico::ARROW_RIGHT,
        NavigateBack => ico::CHEVRON_LEFT,
        NavigateForward => ico::CHEVRON_RIGHT,
        RecentFiles | RecentLocations => ico::HISTORY,
        SelectIn => ico::LOCATE,
        Reformat => ico::REFORMAT,
        FileStructure => ico::TW_STRUCTURE,
        ToggleProjectWindow => ico::TW_PROJECT,
        ToggleTerminal => ico::FILE_SHELL,
        ToggleProblems => ico::TW_PROBLEMS,
        ToggleGitWindow => ico::TW_VCS,
        Commit => ico::COMMIT,
        ShowLocalHistory => ico::HISTORY,
        HideToolWindows => ico::HIDE,
        SaveAll | Save => ico::SAVE,
        CloseTab => ico::CLOSE,
        FoldAll | Fold => ico::COLLAPSE_ALL,
        UnfoldAll | Unfold => ico::EXPAND_ALL,
        Rename => ico::EDIT,
        FindUsages => ico::TW_FIND,
        GotoDeclaration | GotoImplementation | GotoTypeDeclaration | GotoSuperMethod | ShowCallers | ShowCallees => ico::ARROW_RIGHT,
        ShowIntentions | Generate | RefactorThis => ico::BULB,
        QuickDoc | ExternalDocs | ExpandMacro | ShowContextInfo => ico::INFO,
        Complete => ico::METHOD,
        CommentLine | CommentBlock => ico::EDIT,
        DuplicateLine => ico::COPY,
        NextError | PreviousError => ico::ERROR,
        OpenFolder => ico::OPEN,
        ShareInPlayground => ico::EXTERNAL_LINK,
        _ => return None,
    })
}
