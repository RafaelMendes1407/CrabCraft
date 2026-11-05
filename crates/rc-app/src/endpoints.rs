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
        let mut app = crate::screens::test_app(home.path(), true); app.open_workspace(&root);
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
