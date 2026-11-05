//! Static Axum endpoint discovery and Reqwest snippets; never evaluates a project.
use crate::app::CrabCraftApp;
use crate::toolwindow::ToolWindow;
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

#[derive(Clone, Debug)]
pub struct Endpoint { pub file: PathBuf, pub route: rc_syntax::axum::Route, search: String }
#[derive(Default)]
pub struct Snapshot { pub routes: Vec<Endpoint>, hashes: BTreeMap<PathBuf, u64>, skipped: usize, unresolved: usize, limited: bool }
#[derive(Debug)]
enum ScanError { Cancelled }
fn hash(text: &str) -> u64 { text.bytes().fold(0xcbf29ce484222325u64, |value, byte| (value ^ byte as u64).wrapping_mul(0x100000001b3)) }
fn scan(roots: Vec<PathBuf>, workspace: &Path, excluded: &[String], cancel: &AtomicBool) -> Result<Snapshot, ScanError> {
    use std::io::Read;
    let mut snapshot = Snapshot::default(); let mut stack: Vec<_> = roots.into_iter().map(|p| (p, 0usize)).collect(); let mut seen = HashSet::new(); let mut files = 0usize; let mut bytes = 0usize; let mut entries_seen = 0usize;
    while let Some((directory, depth)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) { return Err(ScanError::Cancelled); }
        if depth > 32 { snapshot.limited = true; continue; }
        if seen.len() >= 5000 { snapshot.limited = true; return Ok(snapshot); }
        if !seen.insert(directory.clone()) { continue; }
        let Ok(entries) = std::fs::read_dir(&directory) else { snapshot.skipped += 1; continue };
        for entry in entries.flatten() {
            entries_seen += 1;
            if entries_seen > 50_000 { snapshot.limited = true; return Ok(snapshot); }
            if cancel.load(Ordering::Relaxed) { return Err(ScanError::Cancelled); }
            let path = entry.path(); let Ok(kind) = entry.file_type() else { snapshot.skipped += 1; continue };
            if kind.is_symlink() { continue; }
            if rc_core::workspace::relative_path(workspace, &path).is_some_and(|p| rc_core::workspace::is_excluded(&p, excluded)) { continue; }
            if kind.is_dir() {
                if !matches!(entry.file_name().to_str(), Some("target" | ".git" | "node_modules" | ".crabcraft")) { if stack.len() < 5000 { stack.push((path, depth + 1)); } else { snapshot.limited = true; } }
                continue;
            }
            if !kind.is_file() || path.extension().is_none_or(|ext| ext != "rs") { continue; }
            files += 1;
            if files > 5000 || bytes > 64 * 1024 * 1024 || snapshot.routes.len() >= 10_000 { snapshot.limited = true; return Ok(snapshot); }
            let mut options = std::fs::OpenOptions::new(); options.read(true);
            #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.custom_flags(libc::O_NONBLOCK); }
            let read = options.open(&path).and_then(|file| {
                let metadata = file.metadata()?;
                if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 { return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a bounded regular Rust source file")); }
                let mut text = String::new(); file.take(2 * 1024 * 1024 + 1).read_to_string(&mut text)?; Ok(text)
            });
            let Ok(text) = read else { snapshot.skipped += 1; continue };
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            if bytes.saturating_add(text.len()) > 64 * 1024 * 1024 { snapshot.limited = true; return Ok(snapshot); }
            bytes = bytes.saturating_add(text.len()); snapshot.hashes.insert(path.clone(), hash(&text.replace("\r\n", "\n")));
            let report = rc_syntax::axum::routes(&text);
            snapshot.unresolved = snapshot.unresolved.saturating_add(report.unresolved); snapshot.limited |= report.limited;
            if snapshot.routes.len().saturating_add(report.routes.len()) > 10_000 { snapshot.limited = true; }
            snapshot.routes.extend(report.routes.into_iter().map(|route| Endpoint { file: path.clone(), search: format!("{} {} {}", route.method, route.path, route.handler).to_lowercase(), route }));
            snapshot.routes.truncate(10_000);
        }
    }
    snapshot.routes.sort_by(|a, b| (&a.route.path, &a.route.method, &a.file, a.route.line).cmp(&(&b.route.path, &b.route.method, &b.file, b.route.line)));
    Ok(snapshot)
}
const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerateError { BaseUrl, Parameter, Method }
fn parameters(path: &str) -> Result<BTreeMap<String, bool>, GenerateError> {
    let mut out = BTreeMap::new(); let mut cursor = 0usize;
    while let Some(offset) = path.get(cursor..).and_then(|p| p.find('{')) {
        let start = cursor + offset;
        if path.get(start..).is_some_and(|s| s.starts_with("{{")) { cursor = start + 2; continue; }
        let end = path.get(start + 1..).and_then(|s| s.find('}')).map(|n| start + 1 + n).ok_or(GenerateError::Parameter)?;
        let raw = &path[start + 1..end]; let wildcard = raw.starts_with('*'); let name = raw.trim_start_matches('*');
        if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { return Err(GenerateError::Parameter); }
        out.insert(name.to_string(), wildcard); cursor = end + 1;
    }
    for segment in path.split('/') {
        if let Some(name) = segment.strip_prefix(':').or_else(|| segment.strip_prefix('*')) {
            if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { return Err(GenerateError::Parameter); }
            out.insert(name.to_string(), segment.starts_with('*'));
        }
    }
    if out.len() > 32 { return Err(GenerateError::Parameter); } Ok(out)
}
fn encode(value: &str, wildcard: bool) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() { if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || (wildcard && byte == b'/') { encoded.push(byte as char); } else { encoded.push_str(&format!("%{byte:02X}")); } }
    encoded
}
pub fn reqwest_call(base: &str, method: &str, path: &str, values: &BTreeMap<String, String>) -> Result<String, GenerateError> {
    if base.len() > 4096 || path.len() > 4096 { return Err(GenerateError::BaseUrl); }
    let base = reqwest::Url::parse(base).map_err(|_| GenerateError::BaseUrl)?;
    if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() || base.query().is_some() || base.fragment().is_some() { return Err(GenerateError::BaseUrl); }
    if !METHODS.contains(&method) { return Err(GenerateError::Method); }
    if !path.starts_with('/') || path.contains(['?', '#']) { return Err(GenerateError::Parameter); }
    let mut path = path.to_string();
    for (name, wildcard) in parameters(&path)? {
        let value = values.get(&name).filter(|value| !value.is_empty() && value.len() <= 1024).ok_or(GenerateError::Parameter)?;
        if (wildcard && value.split('/').any(|s| matches!(s, "." | ".."))) || (!wildcard && matches!(value.as_str(), "." | "..")) { return Err(GenerateError::Parameter); }
        let value = encode(value, wildcard);
        path = path.replace(&format!("{{{name}}}"), &value).replace(&format!("{{*{name}}}"), &value);
        let segments: Vec<_> = path.split('/').map(|segment| if segment == format!(":{name}") || segment == format!("*{name}") { value.clone() } else { segment.to_string() }).collect(); path = segments.join("/");
    }
    if path.len() > 8192 { return Err(GenerateError::Parameter); }
    let url = reqwest::Url::parse(&format!("{}{}", base.as_str().trim_end_matches('/'), path)).map_err(|_| GenerateError::BaseUrl)?;
    Ok(format!("pub async fn call_endpoint() -> Result<String, reqwest::Error> {{\n    let response = reqwest::Client::new()\n        .request(reqwest::Method::{method}, {:?})\n        .timeout(std::time::Duration::from_secs(30))\n        .send().await?\n        .error_for_status()?;\n    response.text().await\n}}\n", url.as_str()))
}

/// URL path span of an actual request line. Parse only a bounded prefix from the start of
/// the file so body text and response scripts cannot masquerade as request lines.
struct HttpContext { method: String, anchor: usize, end: usize, prefix: String }
fn http_context(doc: &crate::docs::Doc) -> Option<HttpContext> {
    let editor = &doc.editor;
    if editor.read_only || editor.selection.len() != 1 || !editor.selection.primary().is_empty() { return None; }
    let rope = editor.buffer.rope(); let caret = editor.caret();
    if caret > rope.len_chars() { return None; }
    let line_index = rope.char_to_line(caret);
    if line_index > 2000 { return None; }
    let mut lines = Vec::new(); let mut bytes = 0usize;
    for line in 0..=line_index {
        let text = rope.line(line);
        bytes = bytes.saturating_add(text.len_bytes());
        if text.len_bytes() > 8192 || bytes > 64 * 1024 { return None; }
        lines.push(text.to_string());
    }
    if rc_syntax::http::line_roles(lines.iter().map(String::as_str)).last() != Some(&rc_syntax::http::LineRole::Request) { return None; }
    let line = lines.last()?.trim_end_matches(['\r', '\n']);
    let first_start = line.find(|c: char| !c.is_whitespace())?;
    let first_end = line.get(first_start..)?.find(char::is_whitespace).map_or(line.len(), |n| first_start + n);
    let first = line.get(first_start..first_end)?;
    let (method, url_start) = if METHODS.contains(&first) {
        if first_end == line.len() { return None; }
        (first.to_string(), line.get(first_end..)?.find(|c: char| !c.is_whitespace()).map_or(line.len(), |n| first_end + n))
    } else { ("GET".to_string(), first_start) };
    let url_end = line.get(url_start..)?.find(char::is_whitespace).map_or(line.len(), |n| url_start + n);
    let url = line.get(url_start..url_end)?;
    let path_start = if url.starts_with('/') || url.is_empty() { 0 }
        else if let Some(scheme_end) = url.find("://") {
            if !matches!(url.get(..scheme_end)?, "http" | "https") { return None; }
            let authority = scheme_end + 3;
            let end = url.get(authority..)?.find(['/', '?', '#']).map_or(url.len(), |n| authority + n);
            if authority == end { return None; } end
        } else { url.find(['/', '?', '#']).unwrap_or(url.len()) };
    let path_end = url.get(path_start..)?.find(['?', '#']).map_or(url.len(), |n| path_start + n);
    let column = caret.checked_sub(rope.line_to_char(line_index))?;
    let caret_byte = line.char_indices().nth(column).map_or(line.len(), |(byte, _)| byte);
    let start = url_start + path_start; let end = url_start + path_end;
    if caret_byte < start || caret_byte > end { return None; }
    let line_start = rope.line_to_char(line_index);
    Some(HttpContext { method, anchor: line_start + line.get(..start)?.chars().count(), end: line_start + line.get(..end)?.chars().count(), prefix: line.get(start..caret_byte)?.to_string() })
}
fn snippet_literal(text: &str) -> String { text.replace('\\', "\\\\").replace('$', "\\$").replace('}', "\\}") }
/// Segment-aware matching retains already typed captures and creates editable snippet stops
/// for captures that are still missing. Static path text is escaped as data, never a snippet.
fn completed_path(path: &str, prefix: &str) -> Option<(String, String)> {
    if !path.starts_with('/') || path.len() > 4096 || path.contains(['?', '#', '\r', '\n']) { return None; }
    let fixed = path.split(['{', ':', '*']).next().unwrap_or(path);
    if !fixed.starts_with(prefix) && !prefix.starts_with(fixed) { return None; }
    let parts: Vec<_> = path.split('/').collect(); let typed: Vec<_> = prefix.split('/').collect();
    let mut labels = Vec::new(); let mut inserted = Vec::new(); let mut stop = 0usize; let mut wildcard_seen = false;
    for (index, part) in parts.iter().enumerate() {
        let capture = part.strip_prefix('{').and_then(|s| s.strip_suffix('}')).or_else(|| part.strip_prefix(':')).or_else(|| part.strip_prefix('*'));
        if let Some(capture) = capture {
            let wildcard = capture.starts_with('*') || part.starts_with('*'); let name = capture.trim_start_matches('*');
            if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || stop >= 32 { return None; }
            let value = if wildcard { typed.get(index..).map(|p| p.join("/")).unwrap_or_default() } else { typed.get(index).copied().unwrap_or("").to_string() };
            if value.is_empty() {
                stop += 1; labels.push(part.to_string()); inserted.push(format!("${{{stop}:{}}}", snippet_literal(name)));
            } else { labels.push(value.clone()); inserted.push(snippet_literal(&value)); }
            if wildcard { wildcard_seen = true; break; }
        } else {
            if part.contains(['{', '}']) || typed.get(index).is_some_and(|p| !part.starts_with(*p)) { return None; }
            labels.push(part.to_string()); inserted.push(snippet_literal(part));
        }
    }
    if typed.len() > parts.len() && !wildcard_seen { return None; }
    Some((labels.join("/"), format!("{}$0", inserted.join("/"))))
}

pub struct Endpoints {
    snapshot: Snapshot,
    cancel: Option<Arc<AtomicBool>>,
    inbox: Inbox<Result<Snapshot, ScanError>>,
    started: bool,
    pub selected: Option<usize>,
    pub filter: String,
    filter_cache: String,
    visible: Option<Vec<usize>>,
    pub base: String,
    method: String,
    values: BTreeMap<String, String>,
    show_hints: bool,
    applied: BTreeMap<u64, u64>,
    error: Option<String>,
}
impl Endpoints {
    pub fn new(waker: Waker) -> Self { Self { snapshot: Default::default(), cancel: None, inbox: Inbox::new(waker), started: false, selected: None, filter: String::new(), filter_cache: String::new(), visible: None, base: "http://127.0.0.1:3000".into(), method: "GET".into(), values: BTreeMap::new(), show_hints: true, applied: BTreeMap::new(), error: None } }
    pub fn cancel(&mut self) { if let Some(cancel) = &self.cancel { cancel.store(true, Ordering::Relaxed); } }
    fn start(&mut self, roots: Vec<PathBuf>, workspace: PathBuf, excluded: Vec<String>) {
        if self.cancel.is_some() { return; }
        self.started = true; self.error = None; let cancel = Arc::new(AtomicBool::new(false)); self.cancel = Some(cancel.clone()); let out = self.inbox.sender();
        std::thread::spawn(move || { out.send(scan(roots, &workspace, &excluded, &cancel)); });
    }
    pub fn poll(&mut self, docs: &mut crate::docs::Docs) {
        let live: HashSet<_> = docs.list.iter().map(|doc| doc.editor.id).collect();
        self.applied.retain(|id, _| live.contains(id));
        for result in self.inbox.drain() {
            let cancelled = self.cancel.take().is_some_and(|flag| flag.load(Ordering::Relaxed));
            if cancelled { self.error = Some(tr("Endpoint scan cancelled").into()); continue; }
            match result {
                Ok(snapshot) => { self.snapshot = snapshot; self.visible = None; self.selected = None; self.values.clear(); self.applied.clear(); for doc in &mut docs.list { if doc.path.extension().is_some_and(|e| e == "rs") { doc.editor.end_hints.clear(); } } }
                Err(ScanError::Cancelled) => self.error = Some(tr("Endpoint scan cancelled").into()),
            }
        }
        for doc in &mut docs.list {
            if doc.path.extension().is_none_or(|e| e != "rs") { continue; }
            let version = doc.editor.buffer.version();
            if let Some(previous) = self.applied.get(&doc.editor.id) {
                if *previous != version { doc.editor.end_hints.clear(); self.applied.insert(doc.editor.id, version); }
                continue;
            }
            self.applied.insert(doc.editor.id, version);
            if !self.show_hints || self.snapshot.hashes.get(&doc.path).is_none_or(|h| *h != hash(&doc.editor.buffer.rope().to_string())) { continue; }
            for endpoint in self.snapshot.routes.iter().filter(|e| e.file == doc.path) {
                let hint = format!("{} {}", endpoint.route.method, endpoint.route.path);
                let line = doc.editor.end_hints.entry(endpoint.route.line).or_default();
                if !line.contains(&hint) && line.len() < 500 { if !line.is_empty() { line.push_str(" · "); } line.push_str(&hint); }
            }
        }
    }
    /// Handles only .http/.rest requests. This is a local provider: no scan, process,
    /// registry request, server resolution or project execution is started here.
    pub fn complete_http(&self, doc: &mut crate::docs::Doc) -> bool {
        if doc.editor.lang() != rc_syntax::Lang::Http { return false; }
        let Some(context) = http_context(doc) else { doc.editor.popups.completion = None; return true; };
        let mut items = Vec::new(); let mut seen = HashSet::new();
        for endpoint in &self.snapshot.routes {
            let route = &endpoint.route;
            if route.method != context.method && route.method != "ANY" && !(context.method == "HEAD" && route.method == "GET") { continue; }
            let Some((label, insert)) = completed_path(&route.path, &context.prefix) else { continue; };
            if !seen.insert(label.clone()) { continue; }
            items.push(rc_editor::CompletionItem { label, detail: format!("{} · {} · {}", context.method, route.path, route.handler), type_text: tr("Axum route").to_string(), icon: rc_kit::ico::CONFIG_WEB,
                insert, replace: Some((context.anchor, context.end)), deprecated: false, data: crate::manifest_completion::LOCAL_ITEM,
                doc: Some(tr("Cached Axum path. Fill capture placeholders before sending; refresh Endpoints after source changes.").to_string()) });
            if items.len() >= 100 { break; }
        }
        let mut completion = rc_editor::Completion::new(context.anchor, items, false);
        completion.extra_word_chars = "/-{}%.*~@!$&'()+,;=:";
        completion.filter(""); completion.prefix = context.prefix;
        if completion.items.is_empty() && self.snapshot.routes.is_empty() { completion.doc = Some(tr("No cached Axum routes. Open Endpoints and refresh the scan.").to_string()); }
        doc.editor.popups.completion = Some(completion);
        true
    }
    fn select(&mut self, index: usize) {
        self.selected = Some(index); self.values.clear();
        if let Some(endpoint) = self.snapshot.routes.get(index) {
            self.method = if endpoint.route.method == "ANY" { "GET".into() } else { endpoint.route.method.clone() };
            if let Ok(parameters) = parameters(&endpoint.route.path) { for (name, wildcard) in parameters { self.values.insert(name, if wildcard { "example/path".into() } else { "example".into() }); } }
        }
    }
}
impl Drop for Endpoints { fn drop(&mut self) { self.cancel(); } }

pub fn show(app: &mut CrabCraftApp, ui: &mut egui::Ui) {
    if crate::ui::toolwindows::header(ui, tr("Endpoints")) { app.toggle_tool(ToolWindow::Endpoints); }
    let Some(workspace) = app.workspace.as_ref().map(|w| w.ws.root.clone()) else { ui.label(tr("Open a Cargo project to scan Axum endpoints")); return };
    let packages: Vec<_> = app.runner.cargo_workspace.iter().chain(&app.runner.attached).flat_map(|w| &w.packages).filter_map(|p| p.manifest.parent().map(Path::to_path_buf)).collect();
    if packages.is_empty() { ui.label(tr("Open a Cargo project to scan Axum endpoints")); return; }
    let state = &mut app.endpoints;
    state.poll(&mut app.docs);
    let mut refresh = !state.started;
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(state.cancel.is_none(), egui::Button::new(tr("Refresh Endpoints"))).clicked() { refresh = true; }
        if state.cancel.is_some() { ui.spinner(); if ui.button(tr("Cancel Scan")).clicked() { state.cancel(); } }
        ui.add(egui::TextEdit::singleline(&mut state.filter).hint_text(tr("Filter method, path or handler")).char_limit(200));
        if ui.checkbox(&mut state.show_hints, tr("Route hints")).changed() { state.applied.clear(); for doc in &mut app.docs.list { if doc.path.extension().is_some_and(|e| e == "rs") { doc.editor.end_hints.clear(); } } }
    });
    if refresh { state.start(packages, workspace.clone(), app.explorer.excluded.clone()); }
    ui.small(tr("Static saved-source snapshot, including inactive cfg branches. Refresh after edits. Macros, runtime paths and cross-file router composition are unsupported."));
    ui.small(trf("Found {n} routes; {skipped} files skipped, {unresolved} unresolved expressions.", &[("n", &state.snapshot.routes.len()), ("skipped", &state.snapshot.skipped), ("unresolved", &state.snapshot.unresolved)]));
    if state.snapshot.limited { ui.colored_label(rc_kit::pal(ui).error, tr("Endpoint scan reached its safety limits; results are incomplete")); }
    if let Some(error) = &state.error { ui.colored_label(rc_kit::pal(ui).error, error); }
    if state.visible.is_none() || state.filter_cache != state.filter {
        let filter = state.filter.to_lowercase();
        state.visible = Some(state.snapshot.routes.iter().enumerate().filter(|(_, e)| e.search.contains(&filter)).map(|(i, _)| i).collect());
        state.filter_cache = state.filter.clone();
    }
    let shown = state.visible.as_deref().unwrap_or(&[]);
    let mut selected = None; let mut navigate = None;
    egui::ScrollArea::vertical().id_salt("endpoint_routes").max_height((ui.available_height() * 0.45).max(80.0)).show_rows(ui, 24.0, shown.len(), |ui, rows| {
        for index in rows.filter_map(|row| shown.get(row).copied()) {
            let endpoint = &state.snapshot.routes[index];
            let label = format!("{} {} → {}", endpoint.route.method, endpoint.route.path, endpoint.route.handler);
            let response = ui.selectable_label(state.selected == Some(index), &label).on_hover_text(format!("{}:{}", endpoint.file.display(), endpoint.route.line + 1));
            if response.clicked() { selected = Some(index); }
            if response.double_clicked() { navigate = Some((endpoint.file.clone(), endpoint.route.line, endpoint.route.column)); }
        }
    });
    if let Some(index) = selected { state.select(index); }
    if let Some(endpoint) = state.selected.and_then(|i| state.snapshot.routes.get(i)).cloned() {
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(tr("Go to Route")).clicked() { navigate = Some((endpoint.file.clone(), endpoint.route.line, endpoint.route.column)); }
            if ui.button(tr("Go to Handler")).clicked() { navigate = Some((endpoint.file.clone(), endpoint.route.handler_line, endpoint.route.handler_column)); }
            ui.label(tr("Generate Reqwest Call"));
        });
        egui::ScrollArea::vertical().id_salt("reqwest_endpoint_preview").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("Base URL")); ui.add(egui::TextEdit::singleline(&mut state.base).char_limit(4096).desired_width(300.0));
                egui::ComboBox::from_id_salt("endpoint_method").selected_text(&state.method).show_ui(ui, |ui| { for method in METHODS { ui.selectable_value(&mut state.method, method.to_string(), *method); } });
            });
            for (name, value) in &mut state.values { ui.horizontal(|ui| { ui.label(name); ui.add(egui::TextEdit::singleline(value).char_limit(1024)); }); }
            ui.small(tr("Requires reqwest in Cargo.toml and an async runtime. This only generates code; nothing is sent."));
            match reqwest_call(&state.base, &state.method, &endpoint.route.path, &state.values) {
                Ok(mut code) => { if ui.button(tr("Copy Reqwest Call")).clicked() { ui.ctx().copy_text(code.clone()); } ui.add(egui::TextEdit::multiline(&mut code).code_editor().desired_width(f32::INFINITY).desired_rows(9).interactive(false)); }
                Err(error) => { ui.colored_label(rc_kit::pal(ui).error, match error { GenerateError::BaseUrl => tr("Enter an HTTP or HTTPS base URL without a query or fragment"), GenerateError::Parameter => tr("Fill all route parameters; unsupported parameter syntax cannot be generated"), GenerateError::Method => tr("Choose a supported HTTP method") }); }
            }
        });
    }
    if let Some((path, line, column)) = navigate { app.open_at(&path, line, column); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn fixture() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/RustAxum").canonicalize().unwrap() }
    fn http_doc(text: &str, extension: &str) -> (tempfile::TempDir, crate::docs::Docs, usize) {
        let place = tempfile::tempdir().unwrap(); let path = place.path().join(format!("api.{extension}"));
        let caret = text.find("<caret>").unwrap(); let position = text[..caret].chars().count();
        std::fs::write(&path, text.replace("<caret>", "")).unwrap();
        let mut docs = crate::docs::Docs::new(rc_syntax::ParserPool::new(Arc::new(|| {})));
        let index = docs.open(&path).unwrap(); docs.list[index].editor.selection = rc_text::Selection::point(position);
        (place, docs, index)
    }
    fn cached() -> Endpoints {
        let mut state = Endpoints::new(Waker::noop()); let root = fixture();
        state.snapshot = scan(vec![root.clone()], &root, &[], &AtomicBool::new(false)).unwrap(); state
    }
    #[test]
    fn http_path_completion_is_method_aware_and_preserves_origin_query_and_other_text() {
        let state = cached();
        let (_place, mut docs, i) = http_doc("# préface\nGET https://example.test:8443/api/us<caret>?keep=$1&name=é HTTP/2\nX-Trace: unchanged\n\n", "http");
        assert!(state.complete_http(&mut docs.list[i]));
        let list = docs.list[i].editor.popups.completion.as_ref().unwrap();
        assert_eq!(list.items.len(), 1); assert_eq!(list.items[0].label, "/api/users/{id}");
        assert_eq!(list.items[0].data, crate::manifest_completion::LOCAL_ITEM);
        rc_editor::accept_completion(&mut docs.list[i].editor, 0, false);
        assert_eq!(docs.list[i].editor.buffer.rope().to_string(), "# préface\nGET https://example.test:8443/api/users/id?keep=$1&name=é HTTP/2\nX-Trace: unchanged\n\n");
        assert!(docs.list[i].editor.snippet.is_some());
        assert!(state.cancel.is_none()); assert!(!state.started);
        for (request, expected) in [("POST /cr<caret> HTTP/1.1\n", Some("/create")), ("GET /cr<caret>\n", None), ("HEAD /admin/he<caret>\n", Some("/admin/health")), ("GET http://{{host}}/admin/he<caret>?x=1\n", Some("/admin/health"))] {
            let (_place, mut docs, i) = http_doc(request, "rest"); state.complete_http(&mut docs.list[i]);
            let completion = docs.list[i].editor.popups.completion.as_ref().unwrap();
            assert_eq!(completion.items.first().map(|item| item.label.as_str()), expected, "{request}");
        }
        let (_place, mut docs, i) = http_doc("GET http://localhost:3000/api/users/42<caret>?x=1\n", "http");
        state.complete_http(&mut docs.list[i]); rc_editor::accept_completion(&mut docs.list[i].editor, 0, true);
        assert_eq!(docs.list[i].editor.buffer.rope().to_string(), "GET http://localhost:3000/api/users/42?x=1\n");
    }
    #[test]
    fn http_completion_rejects_headers_body_comments_scripts_query_and_rust_strings() {
        let state = cached();
        for source in [
            "GET /\nLocation: https://example.test/api/us<caret>\n", "GET /\n\nGET https://example.test/api/us<caret>\n",
            "# GET https://example.test/api/us<caret>\n", "@url = https://example.test/api/us<caret>\n",
            "GET /\n\n> {%\n### fake separator inside script\nGET https://example.test/api/us<caret>\n%}\n",
            "GET https://example.test/path?search=/api/us<caret>\n", "GE<caret>T https://example.test/api/us\n",
        ] {
            let (_place, mut docs, i) = http_doc(source, "http"); assert!(state.complete_http(&mut docs.list[i]));
            assert!(docs.list[i].editor.popups.completion.is_none(), "{source}");
        }
        let (_place, mut docs, i) = http_doc("fn f() { let s = \"https://example.test/api/us<caret>\"; }", "rs");
        assert!(!state.complete_http(&mut docs.list[i]));
        let text = format!("{}GET https://example.test/api/us<caret>\n", "# bounded context\n".repeat(2001));
        let (_place, mut docs, i) = http_doc(&text, "http"); state.complete_http(&mut docs.list[i]); assert!(docs.list[i].editor.popups.completion.is_none());
        assert_eq!(completed_path("/cost/$1", "/cost/"), Some(("/cost/$1".into(), "/cost/\\$1$0".into())));
        assert_eq!(completed_path("/files/{*rest}", "/files/a/b"), Some(("/files/a/b".into(), "/files/a/b$0".into())));
    }
    #[test]
    fn scans_saved_axum_sources_and_respects_exclusions_and_cancellation() {
        let fixture = fixture();
        let snapshot = scan(vec![fixture.clone()], &fixture, &[], &AtomicBool::new(false)).unwrap();
        assert_eq!(snapshot.routes.len(), 4);
        assert!(snapshot.routes.iter().any(|e| e.route.method == "GET" && e.route.path == "/api/users/{id}" && e.route.handler == "users"));
        assert!(snapshot.routes.iter().all(|e| e.file.ends_with("src/lib.rs")));
        assert!(scan(vec![fixture.clone()], &fixture, &["src".into()], &AtomicBool::new(false)).unwrap().routes.is_empty());
        assert!(matches!(scan(vec![fixture.clone()], &fixture, &[], &AtomicBool::new(true)), Err(ScanError::Cancelled)));
        #[cfg(unix)] {
            let place = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(fixture.join("src/lib.rs"), place.path().join("outside.rs")).unwrap();
            assert!(scan(vec![place.path().to_path_buf()], place.path(), &[], &AtomicBool::new(false)).unwrap().routes.is_empty());
        }
    }
    #[test]
    fn generated_calls_encode_values_and_reject_unsupported_urls_and_parameters() {
        let values = BTreeMap::from([("id".into(), "a b/quote\"".into())]);
        let code = reqwest_call("https://example.test", "GET", "/api/users/{id}", &values).unwrap();
        assert!(code.contains("https://example.test/api/users/a%20b%2Fquote%22"));
        let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, &rc_text::Rope::from_str(&code), None).unwrap();
        assert!(!tree.root_node().has_error());
        assert!(matches!(reqwest_call("file:///tmp/test", "GET", "/", &BTreeMap::new()), Err(GenerateError::BaseUrl)));
        assert!(matches!(reqwest_call("https://example.test?q=x", "GET", "/", &BTreeMap::new()), Err(GenerateError::BaseUrl)));
        assert!(matches!(reqwest_call("https://example.test", "INVALID", "/", &BTreeMap::new()), Err(GenerateError::Method)));
        assert!(matches!(reqwest_call("https://example.test", "GET", "/{missing}", &BTreeMap::new()), Err(GenerateError::Parameter)));
        assert!(matches!(reqwest_call("https://example.test", "GET", "/{id}", &BTreeMap::from([("id".into(), "..".into())])), Err(GenerateError::Parameter)));
        let code = reqwest_call("https://example.test", "POST", "/{*tail}", &BTreeMap::from([("tail".into(), "a/b c".into())])).unwrap();
        assert!(code.contains("/a/b%20c"));
    }
    #[test]
    #[ignore = "Requires the real RustAxum fixture dependencies; native scan, navigation, hints and clipboard"]
    fn native_endpoint_window_navigates_hints_and_copies_a_reqwest_call() {
        use egui_kittest::kittest::Queryable;
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap(); let project = tempfile::tempdir().unwrap(); let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        for file in ["Cargo.toml", "Cargo.lock", "src/lib.rs"] { std::fs::copy(fixture().join(file), root.join(file)).unwrap(); }
        std::fs::write(root.join("build.rs"), "fn main() { std::fs::write(\"build-ran\", \"yes\").unwrap(); }\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace(&root);
        assert!(app.runner.cargo_workspace.is_none());
        // Explicit metadata trust, without a language-server build in this static scan test.
        app.trust_project_tools_for_test();
        app.dock.sizes.insert(ToolWindow::Endpoints, 650.0);
        let mut h = crate::screens::harness(app, egui::vec2(1500.0, 1200.0));
        let start = Instant::now();
        while h.state().runner.cargo_workspace.is_none() && start.elapsed() < Duration::from_secs(20) { h.run_steps(2); std::thread::sleep(Duration::from_millis(20)); }
        assert!(h.state().runner.cargo_workspace.is_some());
        h.get_by_label(tr("Endpoints")).click(); h.run_steps(2);
        while (h.state().endpoints.cancel.is_some() || !h.state().endpoints.started) && start.elapsed() < Duration::from_secs(20) { h.run_steps(2); std::thread::sleep(Duration::from_millis(20)); }
        assert_eq!(h.state().endpoints.snapshot.routes.len(), 4);
        assert!(!root.join("build-ran").exists());
        h.run_steps(4);
        h.get_by_label("GET /api/users/{id} → users").click(); h.run_steps(3);
        h.state_mut().endpoints.values.insert("id".into(), "42".into()); h.run_steps(3);
        h.get_by_label(tr("Copy Reqwest Call")).click();
        let mut copied = Vec::new();
        for _ in 0..3 { h.run_steps(1); for command in &h.output().platform_output.commands { if let egui::OutputCommand::CopyText(text) = command { copied.push(text.clone()); } } }
        assert!(copied.iter().any(|code| code.contains("/api/users/42") && code.contains("reqwest::Method::GET")));
        h.get_by_label(tr("Go to Handler")).click(); h.run_steps(4);
        let doc = h.state().docs.active_doc().unwrap();
        assert!(doc.path.ends_with("src/lib.rs"));
        let source = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert_eq!(doc.editor.caret_line_col().0, source.lines().position(|line| line.contains("async fn users")).unwrap());
        assert!(doc.editor.end_hints.values().any(|hint| hint.contains("GET /api/users/{id}")));
        h.state_mut().docs.active_doc_mut().unwrap().editor.replace(0, 0, "// edited\n"); h.run_steps(3);
        assert!(h.state().docs.active_doc().unwrap().editor.end_hints.is_empty());
        assert!(!root.join("build-ran").exists());
    }
    #[test]
    #[ignore = "Requires cached RustAxum dependencies and a local HTTP server; exercises manual Ctrl+Space"]
    fn native_http_path_completion_uses_cached_axum_routes_and_sends_the_inserted_url() {
        use egui_kittest::kittest::Queryable;
        use std::io::{Read, Write};
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap(); let project = tempfile::tempdir().unwrap(); let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        for file in ["Cargo.toml", "Cargo.lock", "src/lib.rs"] { std::fs::copy(fixture().join(file), root.join(file)).unwrap(); }
        std::fs::write(root.join("build.rs"), "fn main() { std::fs::write(\"build-ran\", \"yes\").unwrap(); }\n").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); listener.set_nonblocking(true).unwrap(); let address = listener.local_addr().unwrap();
        let original = format!("### User\nGET http://{address}/api/us?keep=one HTTP/1.1\nX-Trace: untouched\n\n");
        let path = root.join("api.http"); std::fs::write(&path, &original).unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace(&root);
        assert!(app.runner.cargo_workspace.is_none());
        // Explicit metadata trust, without a language-server build in this static scan test.
        app.trust_project_tools_for_test();
        let mut h = crate::screens::harness(app, egui::vec2(1500.0, 1200.0));
        let start = Instant::now();
        while h.state().runner.cargo_workspace.is_none() && start.elapsed() < Duration::from_secs(20) { h.run_steps(2); std::thread::sleep(Duration::from_millis(20)); }
        assert!(h.state().runner.cargo_workspace.is_some());
        h.get_by_label(tr("Endpoints")).click(); h.run_steps(2);
        while (h.state().endpoints.cancel.is_some() || !h.state().endpoints.started) && start.elapsed() < Duration::from_secs(20) { h.run_steps(2); std::thread::sleep(Duration::from_millis(20)); }
        assert_eq!(h.state().endpoints.snapshot.routes.len(), 4);
        h.state_mut().open_at(&root.join("src/lib.rs"), 0, 0); h.run_steps(3);
        assert!(h.state().docs.active_doc().unwrap().editor.end_hints.values().any(|hint| hint.contains("GET /api/users/{id}")));
        h.state_mut().open_file(&path); h.run_steps(3);
        let caret = original[..original.find("?keep").unwrap()].chars().count();
        h.state_mut().docs.active_doc_mut().unwrap().editor.selection = rc_text::Selection::point(caret);
        let chord = h.state().keymap.chord(rc_editor::Action::Complete).unwrap();
        h.key_press_modifiers(chord.mods, chord.key); h.run_steps(3);
        let doc = h.state().docs.active_doc().unwrap();
        assert_eq!(doc.editor.popups.completion.as_ref().unwrap().selected_item().unwrap().label, "/api/users/{id}");
        h.key_press(egui::Key::Enter); h.run_steps(2);
        h.event(egui::Event::Text("42".into())); h.run_steps(2); h.key_press(egui::Key::Tab); h.run_steps(2);
        let text = h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();
        assert_eq!(text, original.replace("/api/us?", "/api/users/42?"));
        assert!(h.state().http.history.is_empty()); assert!(h.state().endpoints.cancel.is_none()); assert!(!root.join("build-ran").exists());
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        listener.set_nonblocking(false).unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap(); stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut bytes = [0u8; 8192]; let n = stream.read(&mut bytes).unwrap(); let request = String::from_utf8_lossy(&bytes[..n]).to_string();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nuser:42").unwrap(); request
        });
        h.state_mut().send_http_at(1);
        while h.state().http.running() && start.elapsed() < Duration::from_secs(20) { h.run_steps(2); std::thread::sleep(Duration::from_millis(20)); }
        let request = worker.join().unwrap(); assert!(request.starts_with("GET /api/users/42?keep=one HTTP/1.1")); assert!(request.to_lowercase().contains("x-trace: untouched"));
        assert!(matches!(&h.state().http.history[0].outcome, crate::http_client::Outcome::Done { response, .. } if response.status == 200));
        assert!(!root.join("build-ran").exists());
    }
    #[test]
    #[ignore = "Compiles generated Reqwest code using cached fixture dependencies and calls a local HTTP server"]
    fn generated_reqwest_code_compiles_and_calls_a_real_http_endpoint() {
        use std::io::{Read, Write};
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); let address = server.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap(); stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut request = [0u8; 8192]; let count = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /api/users/a%20b%2Fquote%22 HTTP/1.1"));
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\ngenerated").unwrap();
        });
        let place = tempfile::tempdir().unwrap(); std::fs::create_dir(place.path().join("src")).unwrap();
        let manifest = std::fs::read_to_string(fixture().join("Cargo.toml")).unwrap().replace("crabcraft-axum-fixture", "crabcraft-generated-reqwest-fixture");
        std::fs::write(place.path().join("Cargo.toml"), manifest).unwrap();
        let code = reqwest_call(&format!("http://{address}"), "GET", "/api/users/{id}", &BTreeMap::from([("id".into(), "a b/quote\"".into())])).unwrap();
        std::fs::write(place.path().join("src/lib.rs"), format!("{code}\n#[tokio::test] async fn generated_request() {{ assert_eq!(call_endpoint().await.unwrap(), \"generated\"); }}\n")).unwrap();
        let output = rc_cargo::Cargo::locate().unwrap().std_command().args(["test", "--offline"]).env("CARGO_TARGET_DIR", fixture().join("target")).current_dir(place.path()).output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        worker.join().unwrap();
    }
}
