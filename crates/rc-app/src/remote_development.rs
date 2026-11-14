//! Explicit remote workspace mode: independent buffers and URI namespace, never local project paths.
use crate::{app::CrabCraftApp, remote_development_transport as transport, remote_transfer};
use rc_core::{Inbox, Waker};
use rc_dotnet::process::CancelToken;
use rc_editor::{Action, EditorState, EditorView};
use rc_kit::i18n::{tr, trf};
use rc_lsp::{Incoming, Pending, lsp_types as lsp};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use transport::{Error, Session, Settings, Task};
#[derive(Clone, Debug, PartialEq, Eq)]
struct Ticket {
    id: u64,
    version: u64,
    path: String,
}
struct Tab {
    path: String,
    editor: EditorState,
    baseline: String,
    synced: u64,
    lsp_version: i32,
    last_edit: Instant,
}
impl Tab {
    fn ticket(&self) -> Ticket {
        Ticket { id: self.editor.id, version: self.editor.buffer.version(), path: self.path.clone() }
    }
    fn text(&self) -> String {
        let text = self.editor.buffer.rope().to_string();
        let mut text = if self.editor.buffer.line_ending() == rc_text::LineEnding::CrLf { text.replace('\n', "\r\n") } else { text };
        if self.baseline.starts_with('\u{feff}') {
            text.insert(0, '\u{feff}');
        }
        text
    }
    fn dirty(&self) -> bool {
        self.text() != self.baseline
    }
    fn sync(&mut self, client: &rc_lsp::Client) {
        let version = self.editor.buffer.version();
        if version == self.synced {
            return;
        }
        self.lsp_version = self.lsp_version.saturating_add(1);
        client.notify_raw("textDocument/didChange",json!({"textDocument":{"uri":rc_lsp::path_to_uri(Path::new(&self.path)).as_str(),"version":self.lsp_version},"contentChanges":[{"text":self.editor.buffer.rope().to_string()}]}));
        self.synced = version;
    }
}
#[derive(Clone, Copy)]
enum Location {
    Lsp(lsp::Position),
    Cargo { line: u32, column: u32 },
}
fn caret(rope: &rc_text::Rope, location: Location, closed_bom: bool) -> Option<usize> {
    match location {
        Location::Lsp(mut position) => {
            if closed_bom && position.line == 0 {
                position.character = position.character.checked_sub(1)?;
            }
            let caret = crate::convert::char_of(rope, position);
            (crate::convert::lsp_position(rope, caret) == position).then_some(caret)
        }
        Location::Cargo { line, column } => {
            let line = line as usize;
            if line >= rope.len_lines() {
                return None;
            }
            let start = rope.line_to_char(line);
            let length = rope.line(line).len_chars();
            if column as usize > length {
                return None;
            }
            Some(start + column as usize)
        }
    }
}
enum Reply {
    Started(Box<Session>),
    Distributions(Vec<String>),
    Read { path: String, text: String, origin: Option<Ticket>, location: Option<Location> },
    Preview { ticket: Ticket, preview: Box<transport::Preview> },
    Uploaded { ticket: Ticket, text: String },
    Reload { ticket: Ticket, text: String },
    CargoLine(String),
    CargoFinished { code: i32, tickets: Vec<Ticket> },
}
struct Job {
    cancel: CancelToken,
    requires_trust: bool,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
enum QueryKind {
    Definition,
    Completion,
    Hover,
}
struct Query {
    ticket: Ticket,
    kind: QueryKind,
    pending: Pending<Value>,
    started: Instant,
}
struct Completion {
    ticket: Ticket,
    label: String,
    edit: lsp::TextEdit,
}
pub struct RemoteDevelopment {
    pub open: bool,
    pub wsl: bool,
    pub container: bool,
    pub docker_cli: String,
    pub docker_socket: String,
    pub container_id: String,
    pub distribution: String,
    distributions: Vec<String>,
    pub host: String,
    pub port: String,
    pub user: String,
    pub identity: String,
    pub known_hosts: String,
    pub root: String,
    pub cargo: String,
    pub analyzer: String,
    pub trusted: bool,
    session: Option<Session>,
    buffer_settings: Option<Settings>,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    inbox: Inbox<(u64, Result<Reply, Error>)>,
    generation: u64,
    job: Option<Job>,
    query: Option<Query>,
    completions: Vec<Completion>,
    hover: String,
    preview: Option<(Ticket, transport::Preview)>,
    error: Option<Error>,
    status: String,
    output: Vec<String>,
    diagnostics: Vec<transport::Diagnostic>,
    path: String,
    pool: rc_syntax::ParserPool,
    discard: bool,
    reload: Option<Ticket>,
    indexed: bool,
    cargo_exit: Option<i32>,
    waker: Waker,
}
impl RemoteDevelopment {
    pub fn new(waker: Waker) -> Self {
        Self {
            open: false,
            wsl: false,
            container: false,
            docker_cli: String::new(),
            docker_socket: "unix:///var/run/docker.sock".into(),
            container_id: String::new(),
            distribution: String::new(),
            distributions: Vec::new(),
            host: String::new(),
            port: "22".into(),
            user: String::new(),
            identity: String::new(),
            known_hosts: String::new(),
            root: String::new(),
            cargo: String::new(),
            analyzer: String::new(),
            trusted: false,
            session: None,
            buffer_settings: None,
            tabs: Vec::new(),
            active: 0,
            next_id: 1u64 << 62,
            inbox: Inbox::new(waker.clone()),
            generation: 0,
            job: None,
            query: None,
            completions: Vec::new(),
            hover: String::new(),
            preview: None,
            error: None,
            status: String::new(),
            output: Vec::new(),
            diagnostics: Vec::new(),
            path: "src/main.rs".into(),
            pool: rc_syntax::ParserPool::new(waker.as_fn()),
            discard: false,
            reload: None,
            indexed: false,
            cargo_exit: None,
            waker,
        }
    }
    fn settings(&self) -> Result<Settings, Error> {
        let settings = Settings {
            ssh: remote_transfer::Config {
                host: self.host.clone(),
                port: if self.container { 22 } else { self.port.parse().ok().filter(|p| *p > 0).ok_or(Error::Input)? },
                user: self.user.clone(),
                identity: PathBuf::from(&self.identity),
                known_hosts: PathBuf::from(&self.known_hosts),
            },
            wsl: self.wsl.then(|| crate::wsl_workspace::Config::new(self.distribution.clone(), self.user.clone())),
            container: self.container.then(|| crate::container_workspace::Config {
                cli: PathBuf::from(&self.docker_cli),
                socket: self.docker_socket.clone(),
                container: self.container_id.clone(),
                user: self.user.clone(),
                root: self.root.clone(),
            }),
            root: self.root.clone(),
            cargo: self.cargo.clone(),
            analyzer: self.analyzer.clone(),
        };
        settings.validate()?;
        Ok(settings)
    }
    fn current(&self, ticket: &Ticket) -> bool {
        self.tabs.iter().any(|tab| tab.ticket() == *ticket)
    }
    fn spawn(&mut self, work: impl FnOnce(CancelToken) -> Result<Reply, Error> + Send + 'static) {
        if self.job.is_some() {
            return;
        }
        self.error = None;
        self.preview = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let sender = self.inbox.sender();
        match std::thread::Builder::new().name("rc-remote-workspace".into()).spawn(move || sender.send((generation, work(token)))) {
            Ok(_) => self.job = Some(Job { cancel, requires_trust: true }),
            Err(e) => self.error = Some(e.into()),
        }
    }
    fn cancel(&mut self) {
        self.job = None;
        self.query = None;
        self.generation = self.generation.wrapping_add(1);
        self.preview = None;
        self.completions.clear();
    }
    pub(crate) fn disconnect(&mut self) {
        self.cancel();
        self.session = None;
        self.indexed = false;
        self.status = tr("Remote workspace disconnected. Buffers remain in memory.").into();
    }
    fn list_distributions(&mut self) {
        if !crate::wsl_workspace::available() {
            self.error = Some(Error::Unsupported);
            return;
        }
        self.spawn(|cancel| crate::wsl_workspace::distributions(cancel).map(Reply::Distributions));
        if let Some(job) = &mut self.job {
            job.requires_trust = false;
        }
    }
    fn start(&mut self) {
        if self.wsl && !crate::wsl_workspace::available() {
            self.error = Some(Error::Unsupported);
            return;
        }
        if !self.trusted {
            self.error = Some(Error::Unsupported);
            return;
        }
        match self.settings() {
            Ok(settings) => {
                if self.buffer_settings.as_ref() != Some(&settings) {
                    if self.tabs.iter().any(Tab::dirty) {
                        self.error = Some(Error::Stale);
                        return;
                    }
                    self.tabs.clear();
                    self.active = 0;
                }
                self.diagnostics.clear();
                self.output.clear();
                let waker = self.waker.clone();
                self.spawn(move |token| transport::start(settings, waker, token).map(|session| Reply::Started(Box::new(session))));
            }
            Err(e) => self.error = Some(e),
        }
    }
    fn load(&mut self, path: String, origin: Option<Ticket>, location: Option<Location>) {
        let Some(session) = &self.session else {
            return;
        };
        let path = match session.settings.file(&path) {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if let Some(index) = self.tabs.iter().position(|t| t.path == path) {
            self.active = index;
            if let Some(position) = location {
                let tab = &mut self.tabs[index];
                let Some(caret) = caret(tab.editor.buffer.rope(), position, false) else {
                    self.error = Some(Error::Analysis);
                    return;
                };
                tab.editor.select_range(caret, caret);
                tab.editor.request_scroll_to_caret();
            }
            return;
        }
        if self.tabs.len() >= 16 {
            self.error = Some(Error::Limit);
            return;
        }
        let settings = session.settings.clone();
        self.spawn(move |cancel| transport::read(&settings, &path, cancel).map(|text| Reply::Read { path, text, origin, location }));
    }
    fn reload_active(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let settings = session.settings.clone();
        let path = tab.path.clone();
        let ticket = tab.ticket();
        self.spawn(move |cancel| transport::read(&settings, &path, cancel).map(|text| Reply::Reload { ticket, text }));
    }
    fn preview_upload(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if !tab.dirty() {
            self.error = Some(Error::Unsupported);
            return;
        }
        let settings = session.settings.clone();
        let path = tab.path.clone();
        let ticket = tab.ticket();
        let before = tab.baseline.clone();
        let after = tab.text();
        self.spawn(move |cancel| {
            transport::preview(&settings, &path, before, after, cancel).map(|preview| Reply::Preview { ticket, preview: Box::new(preview) })
        });
    }
    fn apply_upload(&mut self) {
        let Some((ticket, preview)) = self.preview.take() else {
            return;
        };
        if !self.current(&ticket) {
            self.error = Some(Error::Stale);
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let settings = session.settings.clone();
        self.spawn(move |cancel| transport::apply(&settings, preview, cancel).map(|text| Reply::Uploaded { ticket, text }));
    }
    fn run(&mut self, task: Task) {
        let Some(session) = &self.session else {
            return;
        };
        if self.tabs.iter().any(Tab::dirty) {
            self.error = Some(Error::Stale);
            return;
        }
        if self.job.is_some() {
            return;
        }
        self.query = None;
        self.preview = None;
        self.output.clear();
        self.cargo_exit = None;
        self.diagnostics.clear();
        let tickets = self.tabs.iter().map(Tab::ticket).collect();
        let settings = session.settings.clone();
        let sender = self.inbox.sender();
        let generation = self.generation.wrapping_add(1);
        self.spawn(move |cancel| {
            let code = transport::cargo(settings, task, cancel, &mut |_, line| {
                sender.send((generation, Ok(Reply::CargoLine(line.to_owned()))));
            })?;
            Ok(Reply::CargoFinished { code, tickets })
        });
    }
    fn request(&mut self, kind: QueryKind) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        tab.sync(&session.client);
        let position = crate::convert::lsp_position(tab.editor.buffer.rope(), tab.editor.caret());
        let method = match kind {
            QueryKind::Definition => "textDocument/definition",
            QueryKind::Completion => "textDocument/completion",
            QueryKind::Hover => "textDocument/hover",
        };
        let pending =
            session.client.request_raw(method, json!({"textDocument":{"uri":rc_lsp::path_to_uri(Path::new(&tab.path)).as_str()},"position":position}));
        self.query = Some(Query { ticket: tab.ticket(), kind, pending, started: Instant::now() });
        self.completions.clear();
        self.hover.clear();
    }
    fn apply_completion(&mut self, index: usize) {
        let Some(completion) = self.completions.get(index) else {
            return;
        };
        if !self.current(&completion.ticket) {
            self.error = Some(Error::Stale);
            self.completions.clear();
            return;
        }
        let Some(tab) = self.tabs.iter_mut().find(|t| t.editor.id == completion.ticket.id) else {
            return;
        };
        let Some(change) = checked_edit(tab.editor.buffer.rope(), &completion.edit) else {
            self.error = Some(Error::Analysis);
            return;
        };
        tab.editor.apply_text_edits(vec![change]);
        self.completions.clear();
        self.preview = None;
        tab.editor.set_diagnostics(Vec::new());
    }
    fn poll(&mut self) {
        if !self.trusted && (self.session.is_some() || self.job.as_ref().is_some_and(|job| job.requires_trust)) {
            self.disconnect();
        }
        for result in self.pool.poll() {
            if let Some(tab) = self.tabs.iter_mut().find(|t| t.editor.id == result.doc) {
                tab.editor.syntax.accept(result);
            }
        }
        for (generation, result) in self.inbox.drain() {
            if generation != self.generation {
                continue;
            }
            if !matches!(result, Ok(Reply::CargoLine(_))) {
                self.job = None;
            }
            match result {
                Ok(Reply::Distributions(list)) => self.distributions = list,
                Ok(Reply::Started(session)) => {
                    self.buffer_settings = Some(session.settings.clone());
                    for tab in &mut self.tabs {
                        tab.synced = tab.editor.buffer.version();
                        tab.lsp_version = 1;
                        session.client.notify_raw("textDocument/didOpen",json!({"textDocument":{"uri":rc_lsp::path_to_uri(Path::new(&tab.path)).as_str(),"languageId":if tab.path.ends_with(".rs"){"rust"}else{"toml"},"version":1,"text":tab.editor.buffer.rope().to_string()}}));
                    }
                    self.session = Some(*session);
                    self.status = if self.container {
                        tr("Container analyzer connected; indexing inside the container…")
                    } else if self.wsl {
                        tr("WSL analyzer connected; indexing inside the distribution…")
                    } else {
                        tr("Remote analyzer connected; indexing on the SSH host…")
                    }
                    .into();
                }
                Ok(Reply::Read { path, text, origin, location }) => {
                    if origin.as_ref().is_some_and(|t| !self.current(t)) {
                        self.error = Some(Error::Stale);
                        continue;
                    }
                    if self.tabs.iter().any(|t| t.path == path) {
                        continue;
                    }
                    let normalized = text.strip_prefix('\u{feff}').unwrap_or(&text);
                    let mut editor = EditorState::new(self.next_id, Some(PathBuf::from(&path)), normalized);
                    self.next_id = self.next_id.saturating_add(1);
                    editor.read_only = text.len() > 64 * 1024;
                    editor.request_parse(&self.pool);
                    if let Some(position) = location {
                        let Some(caret) = caret(editor.buffer.rope(), position, origin.is_some() && text.starts_with('\u{feff}')) else {
                            self.error = Some(Error::Analysis);
                            continue;
                        };
                        editor.select_range(caret, caret);
                        editor.request_scroll_to_caret();
                    }
                    if let Some(session) = &self.session {
                        session.client.notify_raw("textDocument/didOpen",json!({"textDocument":{"uri":rc_lsp::path_to_uri(Path::new(&path)).as_str(),"languageId":if path.ends_with(".rs"){"rust"}else{"toml"},"version":1,"text":editor.buffer.rope().to_string()}}));
                    }
                    let synced = editor.buffer.version();
                    self.tabs.push(Tab { path, editor, baseline: text, synced, lsp_version: 1, last_edit: Instant::now() });
                    self.active = self.tabs.len() - 1;
                }
                Ok(Reply::Reload { ticket, text }) => {
                    if !self.current(&ticket) {
                        self.error = Some(Error::Stale);
                        continue;
                    }
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.editor.id == ticket.id) {
                        tab.editor.reload(text.strip_prefix('\u{feff}').unwrap_or(&text));
                        tab.baseline = text;
                        tab.editor.set_diagnostics(Vec::new());
                        tab.editor.request_parse(&self.pool);
                        if let Some(session) = &self.session {
                            tab.sync(&session.client);
                        }
                    }
                    self.reload = None;
                    self.preview = None;
                    self.completions.clear();
                }
                Ok(Reply::Preview { ticket, preview }) => {
                    if self.current(&ticket) {
                        self.preview = Some((ticket, *preview));
                    } else {
                        self.error = Some(Error::Stale);
                    }
                }
                Ok(Reply::Uploaded { ticket, text }) => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.editor.id == ticket.id && t.path == ticket.path) {
                        tab.baseline = text;
                        if tab.editor.buffer.version() != ticket.version {
                            self.error = Some(Error::Stale);
                        }
                        if let Some(session) = &self.session {
                            tab.sync(&session.client);
                            session
                                .client
                                .notify_raw("textDocument/didSave", json!({"textDocument":{"uri":rc_lsp::path_to_uri(Path::new(&tab.path)).as_str()}}));
                        }
                    }
                    self.status = if self.wsl {
                        tr("WSL write completed. Cargo still requires an explicit action.")
                    } else {
                        tr("Remote upload completed. Cargo still requires an explicit action.")
                    }
                    .into();
                }
                Ok(Reply::CargoLine(line)) => {
                    if let Some(session) = &self.session
                        && let Some(diagnostic) = transport::cargo_diagnostic(&session.settings, &line)
                        && self.diagnostics.len() < 1000
                    {
                        self.diagnostics.push(diagnostic);
                    }
                    if self.output.len() < 4096 {
                        self.output.push(line.chars().take(4096).collect());
                    }
                }
                Ok(Reply::CargoFinished { code, tickets }) => {
                    self.cargo_exit = Some(code);
                    if tickets.iter().any(|ticket| !self.current(ticket)) {
                        self.diagnostics.clear();
                        self.error = Some(Error::Stale);
                    }
                    self.status = trf("Remote Cargo finished with exit code {code}", &[("code", &code)]);
                }
                Err(e) => self.error = Some(e),
            }
        }
        let mut events = Vec::new();
        if let Some(session) = &self.session {
            events = session.incoming.drain();
        }
        for event in events {
            match event {
                Incoming::Request { id, method, params } => {
                    if let Some(session) = &self.session {
                        session.respond(id, &method, &params);
                    }
                }
                Incoming::Notification { method, params } if method == "experimental/serverStatus" => {
                    if rc_lsp::rust_analyzer::is_quiescent(&params) {
                        self.indexed = true;
                        self.status = tr("Remote analyzer is ready.").into();
                    }
                }
                Incoming::Notification { method, params } if method == "textDocument/publishDiagnostics" => self.publish(params),
                Incoming::Exited(_) => {
                    self.disconnect();
                    self.error = Some(Error::Analysis);
                }
                Incoming::Stderr(line) if self.output.len() < 4096 => {
                    self.output.push(line.chars().take(4096).collect());
                }
                _ => {}
            }
        }
        if let Some(session) = &self.session {
            for tab in &mut self.tabs {
                if tab.synced != tab.editor.buffer.version() && tab.last_edit.elapsed() > Duration::from_millis(200) {
                    tab.sync(&session.client);
                }
            }
        }
        if self.query.as_ref().is_some_and(|q| q.started.elapsed() > Duration::from_secs(20) || !self.current(&q.ticket)) {
            self.query = None;
            self.error = Some(Error::Stale);
        }
        if let Some(result) = self.query.as_mut().and_then(|q| q.pending.poll()) {
            let Some(query) = self.query.take() else {
                return;
            };
            match result {
                Ok(value) => self.resolve(query, value),
                Err(_) => self.error = Some(Error::Analysis),
            }
        }
        if self.preview.as_ref().is_some_and(|(ticket, _)| !self.current(ticket)) {
            self.preview = None;
            self.error = Some(Error::Stale);
        }
    }
    fn publish(&mut self, params: Value) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(path) = params.get("uri").and_then(Value::as_str).and_then(|uri| transport::uri_path(&session.settings, uri)) else {
            return;
        };
        let Some(tab) = self.tabs.iter_mut().find(|t| t.path == path) else {
            return;
        };
        if tab.synced != tab.editor.buffer.version() || params.get("version").and_then(Value::as_i64) != Some(i64::from(tab.lsp_version)) {
            return;
        }
        let Some(items) = params.get("diagnostics").and_then(Value::as_array) else {
            return;
        };
        let diagnostics = items.iter().take(1000).filter_map(bounded_diagnostic).map(|d| crate::convert::diagnostic(tab.editor.buffer.rope(), &d)).collect();
        tab.editor.set_diagnostics(diagnostics);
    }
    fn resolve(&mut self, query: Query, value: Value) {
        if !self.current(&query.ticket) {
            self.error = Some(Error::Stale);
            return;
        }
        match query.kind {
            QueryKind::Definition => {
                let first = value.as_array().and_then(|a| a.first()).unwrap_or(&value);
                let path = first
                    .get("targetUri")
                    .or_else(|| first.get("uri"))
                    .and_then(Value::as_str)
                    .and_then(|uri| self.session.as_ref().and_then(|s| transport::uri_path(&s.settings, uri)));
                let range = first.get("targetSelectionRange").or_else(|| first.get("range")).and_then(bounded_range);
                match (path, range) {
                    (Some(path), Some(range)) => self.load(path, Some(query.ticket), Some(Location::Lsp(range.start))),
                    _ => self.error = Some(Error::Unsupported),
                }
            }
            QueryKind::Hover => {
                self.hover = value
                    .get("contents")
                    .map(|v| v.as_str().or_else(|| v.get("value").and_then(Value::as_str)).map(str::to_owned).unwrap_or_else(|| v.to_string()))
                    .unwrap_or_default()
                    .chars()
                    .take(8192)
                    .collect();
            }
            QueryKind::Completion => {
                let items = value.as_array().or_else(|| value.get("items").and_then(Value::as_array));
                if let Some(items) = items {
                    for item in items.iter().take(500) {
                        if item.get("additionalTextEdits").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
                            || item.get("insertTextFormat").and_then(Value::as_i64) == Some(2)
                        {
                            continue;
                        }
                        let Some(label) = item.get("label").and_then(Value::as_str).filter(|s| s.len() <= 2048) else {
                            continue;
                        };
                        let Some(edit) = item.get("textEdit") else {
                            continue;
                        };
                        let Some(new_text) = edit.get("newText").and_then(Value::as_str).filter(|text| text.len() <= 16 * 1024) else {
                            continue;
                        };
                        let Some(range) = edit.get("range").and_then(bounded_range) else {
                            continue;
                        };
                        let edit = lsp::TextEdit { range, new_text: new_text.into() };
                        self.completions.push(Completion { ticket: query.ticket.clone(), label: label.into(), edit });
                    }
                }
                if self.completions.is_empty() {
                    self.status = tr("No supported remote completion edits. Snippets and import edits are not applied.").into();
                }
            }
        }
    }
}
fn bounded_range(value: &Value) -> Option<lsp::Range> {
    fn position(value: &Value) -> Option<lsp::Position> {
        Some(lsp::Position { line: u32::try_from(value.get("line")?.as_u64()?).ok()?, character: u32::try_from(value.get("character")?.as_u64()?).ok()? })
    }
    let range = lsp::Range { start: position(value.get("start")?)?, end: position(value.get("end")?)? };
    (range.start <= range.end).then_some(range)
}
fn bounded_diagnostic(value: &Value) -> Option<lsp::Diagnostic> {
    let range = value.get("range").and_then(bounded_range)?;
    let severity = match value.get("severity").and_then(Value::as_u64) {
        Some(1) => Some(lsp::DiagnosticSeverity::ERROR),
        Some(2) => Some(lsp::DiagnosticSeverity::WARNING),
        Some(3) => Some(lsp::DiagnosticSeverity::INFORMATION),
        Some(4) => Some(lsp::DiagnosticSeverity::HINT),
        _ => None,
    };
    Some(lsp::Diagnostic { range, severity, message: value.get("message")?.as_str()?.chars().take(4096).collect(), ..Default::default() })
}
fn checked_edit(rope: &rc_text::Rope, edit: &lsp::TextEdit) -> Option<rc_text::Change> {
    let start = crate::convert::char_of(rope, edit.range.start);
    let end = crate::convert::char_of(rope, edit.range.end);
    if start > end || crate::convert::lsp_position(rope, start) != edit.range.start || crate::convert::lsp_position(rope, end) != edit.range.end {
        return None;
    }
    Some(rc_text::Change::new(start, end, edit.new_text.clone()))
}

impl RemoteDevelopment {
    fn action(&mut self, action: Action) {
        match action {
            Action::Save | Action::SaveAll => self.preview_upload(),
            Action::Build => self.run(Task::Build),
            Action::RunTests => self.run(Task::Test),
            Action::StopRun => self.cancel(),
            Action::Complete => self.request(QueryKind::Completion),
            Action::GotoDeclaration => self.request(QueryKind::Definition),
            Action::QuickDoc => self.request(QueryKind::Hover),
            _ => self.error = Some(Error::Unsupported),
        }
    }
    fn ui(&mut self, app: &CrabCraftApp, ctx: &egui::Context) {
        let connected = self.session.is_some();
        let busy = self.job.is_some();
        let mut edited = false;
        let distribution_before = self.distribution.clone();
        let container_before = (self.container, self.docker_cli.clone(), self.docker_socket.clone(), self.container_id.clone());
        let before = (
            self.host.clone(),
            self.port.clone(),
            self.user.clone(),
            self.identity.clone(),
            self.known_hosts.clone(),
            self.root.clone(),
            self.cargo.clone(),
            self.analyzer.clone(),
        );
        egui::Modal::new(egui::Id::unique("remote_development_workspace")).show(ctx,|ui|{ui.set_width((ctx.content_rect().width()-80.).clamp(500.,1500.));ui.set_max_height((ctx.content_rect().height()-80.).max(300.));
 ui.horizontal(|ui|{ui.heading(if self.container{tr("Running container Rust workspace")}else if self.wsl{tr("WSL Rust workspace")}else{tr("Remote Rust workspace")});
if ui.button(if self.wsl{tr("Close WSL workspace")}else{tr("Close remote workspace")}).clicked(){if self.tabs.iter().any(Tab::dirty){self.discard=true;}else{self.disconnect();self.open=false;}}});
 ui.small(if self.container{tr("Explicit connection to an already-running Linux container. Start executes Cargo and rust-analyzer inside it. No container is created, started or pulled.")}else if self.wsl{tr("Explicit Windows-to-WSL session. Start may execute Linux Cargo metadata, build scripts and procedural macros. Windows files are not used as aliases.")}else{tr("Explicit SSH session on a Linux host. Start may execute remote Cargo metadata, build scripts and procedural macros. Local projects are not used.")});
 if self.session.is_none(){ui.add_enabled_ui(!busy,|ui|{if self.wsl {ui.small(tr("WSL is not yet validated on a real Windows/WSL host. Existing distributions only; no installation is performed."));
if !crate::wsl_workspace::available(){ui.colored_label(rc_kit::pal(ui).warn,tr("WSL workspaces are unavailable on this platform. Use Windows with an existing WSL distribution."));}}
 if !self.wsl {let old=self.container;ui.add_enabled(!self.tabs.iter().any(Tab::dirty),egui::Checkbox::new(&mut self.container,tr("Use an already-running Docker container")));
if old!=self.container{self.trusted=false;self.buffer_settings=None;}}
 egui::Grid::new("remote_workspace_settings").num_columns(4).show(ui,|ui|{
 if self.container {field(ui,tr("Docker CLI executable"),&mut self.docker_cli,4096);field(ui,tr("Local Docker socket URI"),&mut self.docker_socket,4096);ui.end_row();field(ui,tr("Running container ID"),&mut self.container_id,64);field(ui,tr("Container user or UID:GID"),&mut self.user,64);ui.end_row();ui.small(tr("Docker runtime validation is pending. Only local Unix sockets and existing Linux containers are supported."));ui.end_row();}
 else if self.wsl {field(ui,tr("WSL distribution"),&mut self.distribution,256);field(ui,tr("User"),&mut self.user,64);ui.end_row();
if ui.add_enabled(crate::wsl_workspace::available(),egui::Button::new(tr("List installed WSL distributions"))).clicked(){self.list_distributions();}ui.end_row();let mut choice=None;for name in &self.distributions {if ui.selectable_label(self.distribution==*name,name).clicked(){choice=Some(name.clone());}}
if let Some(name)=choice{self.distribution=name;}ui.end_row();}
 else{field(ui,tr("Host"),&mut self.host,253);field(ui,tr("Port"),&mut self.port,5);ui.end_row();field(ui,tr("User"),&mut self.user,253);field(ui,tr("SSH private key"),&mut self.identity,8192);ui.end_row();field(ui,tr("Verified known_hosts file"),&mut self.known_hosts,8192);ui.end_row();}
 field(ui,if self.wsl{tr("Linux workspace root")}else{tr("Remote workspace root")},&mut self.root,4096);ui.end_row();field(ui,if self.wsl{tr("Linux Cargo executable")}else{tr("Remote Cargo executable")},&mut self.cargo,4096);field(ui,if self.wsl{tr("Linux rust-analyzer executable")}else{tr("Remote rust-analyzer executable")},&mut self.analyzer,4096);ui.end_row();});
 if before!=(self.host.clone(),self.port.clone(),self.user.clone(),self.identity.clone(),self.known_hosts.clone(),self.root.clone(),self.cargo.clone(),self.analyzer.clone())||distribution_before!=self.distribution||container_before!=(self.container,self.docker_cli.clone(),self.docker_socket.clone(),self.container_id.clone()){self.trusted=false;}
 ui.add_enabled(!self.wsl||crate::wsl_workspace::available(),egui::Checkbox::new(&mut self.trusted,if self.container{tr("I trust this Docker daemon, container and workspace to execute project tools")}else if self.wsl{tr("I trust this WSL distribution and workspace to execute project tools")}else{tr("I trust this SSH host and remote workspace to execute project tools")}));
if ui.add_enabled(self.trusted&&(!self.wsl||crate::wsl_workspace::available()),egui::Button::new(if self.container{tr("Start container workspace")}else if self.wsl{tr("Start WSL workspace")}else{tr("Start remote workspace")})).clicked(){self.start();}});}else{ui.horizontal(|ui|{ui.label(if self.container{format!("{} / {} {}",self.container_id,self.user,self.root)}else if self.wsl{format!("WSL: {} / {} {}",self.distribution,self.user,self.root)}else{format!("{}@{}:{} {}",self.user,self.host,self.port,self.root)});
if ui.button(if self.wsl{tr("Disconnect WSL workspace")}else{tr("Disconnect remote workspace")} ).clicked(){self.disconnect();}});}
 if busy||self.query.is_some(){ui.horizontal(|ui|{ui.spinner();ui.label(tr("Remote operation in progress…"));
if ui.button(tr("Cancel remote operation")).clicked(){self.cancel();}});}
 if let Some(error)=&self.error{ui.colored_label(rc_kit::pal(ui).error,if self.container{match error{Error::Input=>tr("Invalid container settings or Linux source path").into(),Error::Timeout=>tr("Container operation timed out").into(),Error::Io(e)|Error::Process(e)=>trf(tr("Container error: {error}"),&[("error",e)]),_=>error_text(error,false)}}else{error_text(error,self.wsl)});}
if !self.status.is_empty(){ui.label(&self.status);}
 if connected{ui.horizontal(|ui|{field(ui,tr("Remote source path"),&mut self.path,4096);
if ui.add_enabled(!busy,egui::Button::new(tr("Open remote source"))).clicked(){self.load(self.path.clone(),None,None);}
if ui.add_enabled(!busy,egui::Button::new(tr("Remote Cargo check"))).clicked(){self.run(Task::Check);}
if ui.add_enabled(!busy,egui::Button::new(tr("Remote Cargo build"))).clicked(){self.run(Task::Build);}
if ui.add_enabled(!busy,egui::Button::new(tr("Remote Cargo test"))).clicked(){self.run(Task::Test);}});
 ui.collapsing(tr("Remote source files"),|ui|{let sources=self.session.as_ref().map(|s|s.sources.clone()).unwrap_or_default();let mut selected=None;egui::ScrollArea::vertical().id_salt("remote_source_files").max_height(90.).show_rows(ui,22.,sources.len(),|ui,rows|{for row in rows{if let Some(path)=sources.get(row)&& ui.selectable_label(false,path).clicked(){selected=Some(path.clone());}}});
if let Some(path)=selected{self.path=path.clone();self.load(path,None,None);}});}
 ui.horizontal_wrapped(|ui|{for(index,tab)in self.tabs.iter().enumerate(){let relative=tab.path.strip_prefix(&format!("{}/",self.root)).unwrap_or(&tab.path);let label=format!("{relative}{}",if tab.dirty(){" *"}else{""});
if ui.selectable_label(index==self.active,label).clicked(){self.active=index;}}});
 if !self.tabs.is_empty(){ui.horizontal(|ui|{if ui.add_enabled(connected&&!busy,egui::Button::new(if self.wsl{tr("Preview WSL write")}else{tr("Preview remote upload")})).clicked(){self.preview_upload();}
if ui.add_enabled(connected&&!busy,egui::Button::new(tr("Go to remote definition"))).clicked(){self.request(QueryKind::Definition);}
if ui.add_enabled(connected&&!busy,egui::Button::new(tr("Complete remote symbol"))).clicked(){self.request(QueryKind::Completion);}
if ui.add_enabled(connected&&!busy,egui::Button::new(tr("Remote symbol documentation"))).clicked(){self.request(QueryKind::Hover);}
if ui.add_enabled(connected&&!busy,egui::Button::new(tr("Reload remote source"))).clicked()&& let Some(tab)=self.tabs.get(self.active){if tab.dirty(){self.reload=Some(tab.ticket());}else{self.reload_active();}}});
 if let Some(tab)=self.tabs.get_mut(self.active){tab.editor.read_only=!connected||tab.baseline.len()>64*1024;
if tab.editor.read_only{ui.small(tr("Remote source is read-only while disconnected or above the 64 KiB editing limit."));}let version=tab.editor.buffer.version();let height=(ctx.content_rect().height()*0.33).clamp(140.,400.);let(size,rect)=ui.allocate_space(egui::vec2(ui.available_width(),height));let _=size;let mut editor_ui=ui.new_child(egui::UiBuilder::new().max_rect(rect));editor_ui.set_clip_rect(rect.intersect(ui.clip_rect()));let out=EditorView::new(&mut tab.editor,&app.editor_config,&app.keymap).show(&mut editor_ui);
if tab.editor.buffer.version()!=version{tab.last_edit=Instant::now();tab.editor.set_diagnostics(Vec::new());tab.editor.request_parse(&self.pool);self.preview=None;self.completions.clear();self.hover.clear();self.diagnostics.clear();edited=true;}for action in out.actions{self.action(action);}}
 }
 if !self.completions.is_empty(){let mut choice=None;egui::ScrollArea::vertical().id_salt("remote_completions").max_height(100.).show_rows(ui,22.,self.completions.len(),|ui,rows|{for index in rows{if let Some(item)=self.completions.get(index)&& ui.button(&item.label).clicked(){choice=Some(index);}}});
if let Some(index)=choice{self.apply_completion(index);}}
 if !self.hover.is_empty(){egui::ScrollArea::vertical().id_salt("remote_hover").max_height(100.).show(ui,|ui|{ui.label(&self.hover);});}
 if let Some((_,preview))=&self.preview{ui.separator();ui.label(if self.wsl{tr("Review WSL overwrite")}else{tr("Review remote overwrite")});ui.columns(2,|cols|{cols[0].label(tr("Before"));let mut before=preview.before.as_str();cols[0].add(egui::TextEdit::multiline(&mut before).code_editor().desired_rows(3).desired_width(f32::INFINITY));cols[1].label(tr("After"));let mut after=preview.after.as_str();cols[1].add(egui::TextEdit::multiline(&mut after).code_editor().desired_rows(3).desired_width(f32::INFINITY));});ui.small(if self.wsl||self.container{tr("Linux content is checked again before overwrite. Concurrent writers can still race the final check; cancellation may leave a completed write.")}else{tr("Remote content is checked again before upload. SFTP cannot lock out concurrent writers; cancellation may leave a temporary file or an already committed upload.")});
if ui.button(if self.wsl{tr("Confirm WSL overwrite")}else{tr("Confirm remote overwrite")}).clicked(){self.apply_upload();}}
 if !self.diagnostics.is_empty(){ui.label(tr("Remote Cargo diagnostics"));let mut selected=None;egui::ScrollArea::vertical().id_salt("remote_cargo_diagnostics").max_height(100.).show_rows(ui,22.,self.diagnostics.len(),|ui,rows|{for index in rows{if let Some(d)=self.diagnostics.get(index){let label=format!("{}:{}:{} {}",d.path.strip_prefix(&format!("{}/",self.root)).unwrap_or(&d.path),d.line+1,d.column+1,d.message);
if ui.selectable_label(false,egui::RichText::new(label).color(if d.error {rc_kit::pal(ui).error} else {rc_kit::pal(ui).warn})).clicked(){selected=Some((d.path.clone(),d.line,d.column));}}}});
if let Some((path,line,column))=selected{self.load(path,None,Some(Location::Cargo{line,column}));}}
 ui.collapsing(tr("Remote process output"),|ui|{egui::ScrollArea::vertical().id_salt("remote_output").max_height(120.).show_rows(ui,20.,self.output.len(),|ui,rows|{for index in rows{if let Some(line)=self.output.get(index){ui.monospace(line);}}});});
 if let Some(ticket)=self.reload.clone(){ui.separator();ui.colored_label(rc_kit::pal(ui).warn,tr("Discard edits and reload this remote source?"));ui.horizontal(|ui|{if ui.button(tr("Keep remote buffers")).clicked(){self.reload=None;}
if ui.button(tr("Discard edits and reload")).clicked(){if self.current(&ticket){self.reload_active();}else{self.error=Some(Error::Stale);self.reload=None;}}});}
 if self.discard{ui.separator();ui.colored_label(rc_kit::pal(ui).warn,tr("Disconnect and discard unsaved remote buffers?"));ui.horizontal(|ui|{if ui.button(tr("Keep remote buffers")).clicked(){self.discard=false;}
if ui.button(tr("Discard remote buffers and close")).clicked(){self.disconnect();self.tabs.clear();self.open=false;self.discard=false;}});}
 });
        let after = (&self.host, &self.port, &self.user, &self.identity, &self.known_hosts, &self.root, &self.cargo, &self.analyzer);
        if before != (after.0.clone(), after.1.clone(), after.2.clone(), after.3.clone(), after.4.clone(), after.5.clone(), after.6.clone(), after.7.clone()) {
            self.trusted = false;
        }
        if edited {
            self.query = None;
        }
    }
}
fn field(ui: &mut egui::Ui, label: &str, value: &mut String, limit: usize) {
    let label = ui.label(label);
    ui.add(egui::TextEdit::singleline(value).char_limit(limit).desired_width(230.)).labelled_by(label.id);
}
fn error_text(error: &Error, wsl: bool) -> String {
    match error {
        Error::Input => {
            if wsl { tr("Invalid WSL distribution, tools or Linux source path") } else { tr("Invalid SSH settings or unsupported remote text/path") }.into()
        }
        Error::Timeout => if wsl { tr("WSL operation timed out") } else { tr("SSH operation timed out") }.into(),
        Error::Io(e) | Error::Process(e) => trf(if wsl { tr("WSL error: {error}") } else { tr("SSH error: {error}") }, &[("error", e)]),
        Error::Limit => tr("Remote operation exceeded its transfer or output limit").into(),
        Error::Cancelled => tr("Cancelled").into(),
        Error::Stale => tr("Remote buffers or sources changed. Review uploads again; save all remote buffers before running Cargo.").into(),
        Error::Unsupported => tr("This action is unsupported in the remote workspace. Use its explicit source, upload, Cargo and analysis controls.").into(),
        Error::Analysis => tr("Remote rust-analyzer stopped or could not answer this request.").into(),
    }
}

pub fn open(app: &mut CrabCraftApp) {
    open_mode(app, false);
}
pub fn open_wsl(app: &mut CrabCraftApp) {
    open_mode(app, true);
}
fn open_mode(app: &mut CrabCraftApp, wsl: bool) {
    if let Some(state) = &mut app.remote_development
        && (state.wsl != wsl || state.container)
    {
        if state.tabs.iter().any(Tab::dirty) {
            state.error = Some(Error::Stale);
            state.open = true;
            return;
        }
        state.disconnect();
        app.remote_development = None;
    }
    if app.remote_development.is_none() {
        let mut state = RemoteDevelopment::new(app.waker.clone());
        state.wsl = wsl;
        if !wsl {
            state.host = app.remote.host.clone();
            state.port = app.remote.port.clone();
            state.user = app.remote.user.clone();
            state.identity = app.remote.identity.clone();
            state.known_hosts = app.remote.known_hosts.clone();
        }
        app.remote_development = Some(state);
    }
    if let Some(state) = &mut app.remote_development {
        state.open = true;
    }
}
pub fn route(app: &mut CrabCraftApp, action: Action) -> bool {
    let Some(state) = app.remote_development.as_mut().filter(|s| s.open) else {
        return false;
    };
    if matches!(action, Action::RemoteDevelopment | Action::WslWorkspace) {
        return false;
    }
    state.action(action);
    true
}
pub fn show(app: &mut CrabCraftApp, ctx: &egui::Context) {
    let Some(mut state) = app.remote_development.take() else {
        return;
    };
    if state.open {
        state.poll();
        state.ui(app, ctx);
    }
    app.remote_development = Some(state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    #[cfg(not(windows))]
    #[test]
    fn wsl_mode_is_visibly_unavailable_without_execution_and_keeps_namespace_buffers() {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = transport::tests::fixture();
        let mut app = crate::screens::test_app(home.path(), false);
        let local = project.path().join("src/main.rs");
        let original = std::fs::read_to_string(&local).unwrap();
        let index = app.docs.insert_compiler_source(local.clone(), &original, false, None, Default::default());
        app.docs.list[index].editor.replace(0, 0, "// dirty local buffer\n");
        app.run_ide_action(&egui::Context::default(), Action::WslWorkspace);
        let mut h = crate::screens::harness(app, egui::vec2(1600., 1100.));
        h.get_by_label(tr("WSL workspaces are unavailable on this platform. Use Windows with an existing WSL distribution."));
        h.get_by_label(tr("Start WSL workspace")).click();
        h.get_by_label(tr("List installed WSL distributions")).click();
        h.run_steps(3);
        let state = h.state().remote_development.as_ref().unwrap();
        assert!(state.wsl && state.session.is_none() && state.job.is_none());
        assert!(state.distributions.is_empty());
        assert!(h.state().docs.list[0].is_modified());
        assert_eq!(std::fs::read_to_string(&local).unwrap(), original);
        assert!(!project.path().join("target").exists());
        let state = h.state_mut().remote_development.as_mut().unwrap();
        state.trusted = true;
        state.start();
        assert!(matches!(state.error, Some(Error::Unsupported)));
        assert!(state.job.is_none());
        let path = "/home/developer/project/src/main.rs".to_owned();
        let baseline = "fn main(){}\n".to_owned();
        let mut editor = EditorState::new(state.next_id, Some(PathBuf::from(&path)), &baseline);
        editor.replace(0, 0, "// unsaved WSL buffer\n");
        state.tabs.push(Tab { path, editor, baseline, synced: 0, lsp_version: 1, last_edit: Instant::now() });
        let ticket = state.tabs[0].ticket();
        h.state_mut().run_ide_action(&egui::Context::default(), Action::RemoteDevelopment);
        let state = h.state().remote_development.as_ref().unwrap();
        assert!(state.wsl && state.current(&ticket) && state.tabs[0].dirty());
        assert!(matches!(state.error, Some(Error::Stale)));
        assert!(state.session.is_none() && state.job.is_none());
        assert!(h.state().docs.list[0].is_modified());
    }
    #[test]
    fn remote_navigation_keeps_utf16_and_cargo_columns_distinct() {
        let rope = rc_text::Rope::from_str("🦀 answer\n");
        assert_eq!(caret(&rope, Location::Lsp(lsp::Position { line: 0, character: 3 }), false), Some(2));
        assert_eq!(caret(&rope, Location::Cargo { line: 0, column: 2 }, false), Some(2));
        assert_eq!(caret(&rope, Location::Lsp(lsp::Position { line: 0, character: 4 }), true), Some(2));
        assert!(caret(&rope, Location::Lsp(lsp::Position { line: 0, character: 1 }), false).is_none());
    }
    #[test]
    fn local_workspace_switch_and_close_keep_dirty_independent_remote_buffers() {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let first = transport::tests::fixture();
        let second = transport::tests::fixture();
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace_and_wait(first.path());
        open(&mut app);
        let state = app.remote_development.as_mut().unwrap();
        state.root = "/remote/independent".into();
        let path = "/remote/independent/src/main.rs".to_owned();
        let baseline = "fn main(){let answer=41;}\n".to_owned();
        let mut editor = EditorState::new(state.next_id, Some(PathBuf::from(&path)), &baseline);
        editor.replace(0, 0, "// unsaved remote edit\n");
        state.tabs.push(Tab { path, editor, baseline, synced: 0, lsp_version: 1, last_edit: Instant::now() });
        let ticket = state.tabs[0].ticket();
        let dirty = state.tabs[0].text();
        // Simulate CLI/IPC project switching, which bypasses the modal action dispatcher.
        app.open_workspace_and_wait(second.path());
        let mut h = crate::screens::harness(app, egui::vec2(1600., 1100.));
        let deadline = Instant::now() + Duration::from_secs(10);
        while h.state().workspace.as_ref().is_none_or(|w| w.ws.root != second.path()) {
            assert!(Instant::now() < deadline);
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            h.state()
                .remote_development
                .as_ref()
                .is_some_and(|state| state.current(&ticket) && state.tabs[0].dirty() && state.tabs[0].text() == dirty && state.session.is_none())
        );
        h.run_steps(3);
        assert!(h.state().remote_development.as_ref().unwrap().tabs[0].editor.read_only);
        assert_eq!(h.state().remote_development.as_ref().unwrap().tabs[0].text(), dirty);
        assert!(h.state().docs.list.is_empty());
        assert!(h.state().insight.session.is_none());
        h.state_mut().close_workspace();
        let deadline = Instant::now() + Duration::from_secs(10);
        while h.state().workspace.is_some() {
            assert!(Instant::now() < deadline);
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(5));
        }
        h.run_steps(3);
        let state = h.state().remote_development.as_ref().unwrap();
        assert!(state.current(&ticket));
        assert_eq!(state.tabs[0].text(), dirty);
        assert_eq!(state.tabs[0].baseline, "fn main(){let answer=41;}\n");
        assert!(state.tabs[0].dirty());
        assert!(state.session.is_none() && state.job.is_none());
        assert!(h.state().workspace.is_none());
        assert!(h.state().docs.list.is_empty());
        assert!(!first.path().join("target").exists());
        assert!(!second.path().join("target").exists());
    }
    #[test]
    fn remote_mode_requires_explicit_trust_and_routes_save_away_from_local_buffers() {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let dir = transport::tests::fixture();
        let mut app = crate::screens::test_app(home.path(), false);
        let local = dir.path().join("local.rs");
        std::fs::write(&local, "fn local(){}\n").unwrap();
        let index = app.docs.insert_compiler_source(local.clone(), "fn local(){}\n", false, None, Default::default());
        app.docs.list[index].editor.replace(0, 0, "// unsaved local\n");
        app.run_ide_action(&egui::Context::default(), Action::RemoteDevelopment);
        let state = app.remote_development.as_mut().unwrap();
        let s = transport::tests::settings(dir.path());
        state.host = s.ssh.host;
        state.port = s.ssh.port.to_string();
        state.user = s.ssh.user;
        state.identity = s.ssh.identity.display().to_string();
        state.known_hosts = s.ssh.known_hosts.display().to_string();
        state.root = s.root;
        state.cargo = s.cargo;
        state.analyzer = s.analyzer;
        let mut h = crate::screens::harness(app, egui::vec2(1600., 1100.));
        h.get_by_label(tr("Start remote workspace")).click();
        h.run_steps(2);
        assert!(h.state().remote_development.as_ref().unwrap().session.is_none());
        assert!(h.state().remote_development.as_ref().unwrap().job.is_none());
        assert!(h.state().insight.session.is_none());
        assert!(!dir.path().join("target").exists());
        h.state_mut().run_ide_action(&egui::Context::default(), Action::Save);
        assert_eq!(h.state().docs.list.len(), 1);
        assert!(h.state().docs.list[0].is_modified());
        assert_eq!(std::fs::read_to_string(&local).unwrap(), "fn local(){}\n");
        assert!(h.state().remote_development.as_ref().unwrap().session.is_none());
        h.get_by_label(tr("Close remote workspace")).click();
        h.run_steps(2);
        assert!(!h.state().remote_development.as_ref().unwrap().open);
    }
    #[test]
    #[ignore = "Requires explicit private SSH fixture and remote rust-analyzer/Cargo"]
    fn real_native_remote_start_definition_completion_review_upload_cargo_and_disconnect() {
        native_workspace_evidence(false);
    }
    #[cfg(unix)]
    #[test]
    fn native_container_shim_uses_real_rust_analyzer_cargo_review_and_namespace() {
        native_workspace_evidence(true);
    }
    fn native_workspace_evidence(container: bool) {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ssh = (!container).then(|| crate::remote_transfer::tests::SSH_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner()));
        let home = tempfile::tempdir().unwrap();
        let dir = transport::tests::fixture();
        let main = dir.path().join("src/main.rs");
        const SOURCE: &str = "fn main(){let result=remote_development_evidence::answer();assert_eq!(result,42);}\n";
        std::fs::write(&main, SOURCE).unwrap();
        std::fs::write(dir.path().join("build.rs"), "fn main(){std::fs::write(\"remote-ui-build-ran\",\"yes\").unwrap();}\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        app.run_ide_action(&egui::Context::default(), Action::RemoteDevelopment);
        let s = transport::tests::settings(dir.path());
        let state = app.remote_development.as_mut().unwrap();
        state.host = s.ssh.host;
        state.port = s.ssh.port.to_string();
        state.user = s.ssh.user;
        state.identity = s.ssh.identity.display().to_string();
        state.known_hosts = s.ssh.known_hosts.display().to_string();
        state.root = s.root;
        state.cargo = s.cargo;
        state.analyzer = s.analyzer;
        let (_docker_config, docker_log) = crate::container_workspace::tests::fixture(home.path(), dir.path());
        if container {
            state.container = true;
            state.port = "not an SSH port".into();
            state.docker_cli = _docker_config.cli.display().to_string();
            state.docker_socket = _docker_config.socket;
            state.container_id = _docker_config.container;
            state.user = _docker_config.user;
        }
        assert!(!docker_log.exists());
        let start_label = if container { tr("Start container workspace") } else { tr("Start remote workspace") };

        let mut h = crate::screens::harness(app, egui::vec2(1600., 1200.));
        let wait = |h: &mut egui_kittest::Harness<'static, CrabCraftApp>, ready: &dyn Fn(&RemoteDevelopment) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(90);
            loop {
                let state = h.state().remote_development.as_ref().unwrap();
                if ready(state) {
                    break;
                }
                assert!(state.error.is_none(), "remote error {:?} output {:?}", state.error, state.output);
                assert!(Instant::now() < deadline, "remote UI timeout status {}", state.status);
                h.run_steps(1);
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        assert!(!dir.path().join("remote-ui-build-ran").exists());
        h.get_by_label(start_label).click();
        h.run_steps(2);
        assert!(!docker_log.exists());
        assert!(h.state().remote_development.as_ref().unwrap().job.is_none());
        h.get_by_label(if container {
            tr("I trust this Docker daemon, container and workspace to execute project tools")
        } else {
            tr("I trust this SSH host and remote workspace to execute project tools")
        })
        .click();
        h.run_steps(2);
        h.get_by_label(start_label).click();
        h.run_steps(2);
        wait(&mut h, &|s| s.session.is_some() && s.indexed);
        eprintln!("remote UI connected");
        h.run_steps(2);
        h.get_by_label(tr("Open remote source")).click();
        h.run_steps(2);
        wait(&mut h, &|s| s.tabs.len() == 1);
        eprintln!("remote UI source");
        h.run_steps(3);
        assert_eq!(h.state().remote_development.as_ref().unwrap().tabs[0].baseline, SOURCE);
        assert!(h.state().docs.list.is_empty());
        {
            let s = h.state_mut().remote_development.as_mut().unwrap();
            let tab = &mut s.tabs[0];
            let at = SOURCE.find("answer()").unwrap();
            tab.editor.select_range(at, at);
        }
        h.get_by_label(tr("Go to remote definition")).click();
        h.run_steps(2);
        {
            let state = h.state().remote_development.as_ref().unwrap();
            assert!(state.query.is_some() || state.job.is_some() || state.tabs.len() == 2, "definition action was not dispatched: status {}", state.status);
        }
        wait(&mut h, &|s| s.tabs.len() == 2);
        eprintln!("remote UI definition");
        h.run_steps(3);
        assert!(h.state().remote_development.as_ref().unwrap().tabs[1].path.ends_with("/src/lib.rs"));
        {
            let s = h.state_mut().remote_development.as_mut().unwrap();
            s.active = 0;
            let tab = &mut s.tabs[0];
            let start = SOURCE.rfind("result").unwrap();
            tab.editor.replace(start, start + 6, "res");
            tab.editor.select_range(start + 3, start + 3);
        }
        h.run_steps(2);
        h.get_by_label(tr("Complete remote symbol")).click();
        h.run_steps(2);
        wait(&mut h, &|s| s.completions.iter().any(|c| c.label == "result"));
        h.run_steps(2);
        h.get_by_label("result").click();
        h.run_steps(2);
        assert_eq!(h.state().remote_development.as_ref().unwrap().tabs[0].editor.buffer.rope().to_string(), SOURCE);
        {
            let s = h.state_mut().remote_development.as_mut().unwrap();
            let tab = &mut s.tabs[0];
            let at = SOURCE.find("let result").unwrap();
            tab.editor.replace(at, at, "// reviewed on remote host\n");
        }
        h.run_steps(2);
        h.get_by_label(tr("Preview remote upload")).click();
        h.run_steps(2);
        wait(&mut h, &|s| s.preview.is_some());
        eprintln!("remote UI preview");
        assert_eq!(std::fs::read_to_string(&main).unwrap(), SOURCE);
        h.run_steps(2);
        h.get_by_label(tr("Confirm remote overwrite")).click();
        h.run_steps(2);
        wait(&mut h, &|s| s.job.is_none());
        eprintln!("remote UI uploaded");
        h.run_steps(3);
        assert!(std::fs::read_to_string(&main).unwrap().contains("reviewed on remote host"));
        assert!(!h.state().remote_development.as_ref().unwrap().tabs[0].dirty());
        h.get_by_label(tr("Remote Cargo test")).click();
        h.run_steps(2);
        {
            let state = h.state().remote_development.as_ref().unwrap();
            assert!(state.job.is_some() || state.cargo_exit.is_some(), "Cargo action was not dispatched: {}", state.status);
        }
        wait(&mut h, &|s| s.job.is_none() && s.cargo_exit == Some(0));
        assert!(dir.path().join("remote-ui-build-ran").exists());
        assert!(h.state().remote_development.as_ref().unwrap().output.iter().any(|line| line.contains("test result: ok")));
        // Review an actual type error, run Cargo check, then navigate its native diagnostic.
        let valid = h.state().remote_development.as_ref().unwrap().tabs[0].baseline.clone();
        {
            let state = h.state_mut().remote_development.as_mut().unwrap();
            let tab = &mut state.tabs[0];
            let at = valid.find("result,42").unwrap() + 7;
            tab.editor.replace(at, at + 2, "\"wrong\"");
        }
        h.run_steps(3);
        h.get_by_label(tr("Preview remote upload")).click();
        h.run_steps(2);
        wait(&mut h, &|s| s.preview.is_some());
        h.run_steps(3);
        h.get_by_label(tr("Confirm remote overwrite")).click();
        h.run_steps(2);
        wait(&mut h, &|s| s.job.is_none());
        h.run_steps(3);
        h.get_by_label(tr("Remote Cargo check")).click();
        h.run_steps(2);
        wait(&mut h, &|s| s.job.is_none() && s.cargo_exit.is_some_and(|code| code != 0));
        let state = h.state().remote_development.as_ref().unwrap();
        let diagnostic = state.diagnostics.iter().find(|d| d.path.ends_with("/src/main.rs") && d.error).unwrap().clone();
        let label = format!(
            "{}:{}:{} {}",
            diagnostic.path.strip_prefix(&format!("{}/", state.root)).unwrap(),
            diagnostic.line + 1,
            diagnostic.column + 1,
            diagnostic.message
        );
        h.run_steps(3);
        h.get_by_label(&label).click();
        h.run_steps(2);
        assert_eq!(h.state().remote_development.as_ref().unwrap().active, 0);
        assert_eq!(h.state().remote_development.as_ref().unwrap().tabs[0].editor.caret_line_col().0, diagnostic.line as usize);
        // Disconnect keeps unsaved corrections; reconnect to the same host does not discard them.
        {
            let tab = &mut h.state_mut().remote_development.as_mut().unwrap().tabs[0];
            tab.editor.undo();
            assert!(tab.dirty());
        }
        h.run_steps(2);
        h.get_by_label(tr("Disconnect remote workspace")).click();
        h.run_steps(2);
        assert!(h.state().remote_development.as_ref().unwrap().session.is_none());
        assert!(h.state().docs.list.is_empty());
        if container {
            let state = h.state_mut().remote_development.as_mut().unwrap();
            let id = state.container_id.clone();
            state.container_id = "ffffffffffffffff".into();
            state.start();
            assert!(matches!(state.error, Some(Error::Stale)));
            assert!(state.session.is_none() && state.job.is_none());
            assert!(state.tabs[0].dirty());
            state.container_id = id;
            state.error = None;
        }
        h.run_steps(3);
        h.get_by_label(start_label).click();
        h.run_steps(2);
        wait(&mut h, &|s| s.session.is_some());
        assert!(h.state().remote_development.as_ref().unwrap().tabs[0].dirty());
        assert_eq!(h.state().remote_development.as_ref().unwrap().tabs[0].editor.buffer.rope().to_string(), valid);
        h.run_steps(3);
        h.get_by_label(tr("Reload remote source")).click();
        h.run_steps(3);
        h.get_by_label(tr("Keep remote buffers")).click();
        h.run_steps(3);
        assert!(h.state().remote_development.as_ref().unwrap().tabs[0].dirty());
        h.get_by_label(tr("Reload remote source")).click();
        h.run_steps(3);
        h.get_by_label(tr("Discard edits and reload")).click();
        h.run_steps(2);
        wait(&mut h, &|s| s.job.is_none() && !s.tabs[0].dirty());
        h.run_steps(3);
        h.get_by_label(tr("Close remote workspace")).click();
        h.run_steps(2);
        assert!(!h.state().remote_development.as_ref().unwrap().open);
    }
}
