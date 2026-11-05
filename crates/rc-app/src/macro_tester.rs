//! A declarative-macro laboratory isolated from the project's build graph.
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use rc_lsp::{Client, Incoming, Pending};
use serde_json::{Value, json};
use std::{path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};

const INPUT_LIMIT: usize = 16 * 1024;
const OUTPUT_LIMIT: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
const SAMPLE: &str = "macro_rules! twice {\n    ($value:expr) => { ($value) + ($value) };\n}";

#[derive(Debug)]
pub enum Error { Trust, Input, Limit, MissingAnalyzer, Cancelled, Timeout, NoExpansion, Io(std::io::Error), Server(String) }
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trust => f.write_str(tr("Trust this project before testing declarative macros")),
            Self::Input => f.write_str(tr("Enter macro_rules definitions and one invocation of a defined macro")),
            Self::Limit => f.write_str(tr("The macro test exceeds its input or expansion limit")),
            Self::MissingAnalyzer => f.write_str(tr("rust-analyzer was not found for the macro tester")),
            Self::Cancelled => f.write_str(tr("Macro test cancelled")),
            Self::Timeout => f.write_str(tr("The macro test timed out")),
            Self::NoExpansion => f.write_str(tr("The macro invocation could not be expanded")),
            Self::Io(error) => f.write_str(&trf("Macro tester I/O error: {error}", &[("error", &error.to_string())])),
            Self::Server(error) => f.write_str(&trf("Macro analyzer error: {error}", &[("error", error)])),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error { fn from(error: std::io::Error) -> Self { Self::Io(error) } }

#[derive(Debug, Clone)]
pub struct Expansion { pub name: String, pub text: String }
struct Source { text: String, line: usize, column: usize }
fn source(definition: &str, invocation: &str) -> Result<Source, Error> {
    if definition.len() > INPUT_LIMIT || invocation.len() > INPUT_LIMIT { return Err(Error::Limit); }
    let rope = rc_text::Rope::from_str(definition);
    let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, &rope, None).ok_or(Error::Input)?;
    if tree.root_node().has_error() { return Err(Error::Input); }
    let mut names = Vec::new();
    let mut cursor = tree.walk();
    for node in tree.root_node().named_children(&mut cursor) {
        if matches!(node.kind(), "line_comment" | "block_comment") { continue; }
        if node.kind() != "macro_definition" { return Err(Error::Input); }
        let name = node.child_by_field_name("name").and_then(|node| definition.get(node.byte_range())).ok_or(Error::Input)?;
        names.push(name.to_string());
    }
    if names.is_empty() || names.len() > 16 { return Err(Error::Input); }
    let call = invocation.trim().trim_end_matches(';').trim();
    let text = format!("{definition}\n{call};\n");
    let rope = rc_text::Rope::from_str(&text);
    let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, &rope, None).ok_or(Error::Input)?;
    if tree.root_node().has_error() { return Err(Error::Input); }
    let mut stack = vec![tree.root_node()]; let mut calls = Vec::new(); let mut visited = 0;
    while let Some(node) = stack.pop() {
        visited += 1; if visited > 16_384 { return Err(Error::Limit); }
        if node.start_byte() > definition.len() && node.kind() == "macro_invocation" { calls.push(node); continue; }
        let mut cursor = node.walk(); stack.extend(node.named_children(&mut cursor));
    }
    if calls.len() != 1 { return Err(Error::Input); }
    let node = calls[0];
    let macro_name = node.child_by_field_name("macro").and_then(|node| text.get(node.byte_range())).ok_or(Error::Input)?;
    if !names.iter().any(|name| name == macro_name) || text[definition.len()..node.start_byte()].trim().len() != 0
        || text[node.end_byte()..].trim().trim_matches(';').trim().len() != 0 { return Err(Error::Input); }
    let position = node.start_position();
    Ok(Source { text, line: position.row, column: position.column })
}
fn configuration(file: &Path) -> Value {
    json!({
        "linkedProjects": [{ "crates": [{ "root_module": file, "edition": "2024", "deps": [], "cfg": [], "env": {}, "is_workspace_member": true }] }],
        "checkOnSave": false, "check": { "enable": false },
        "cargo": { "buildScripts": { "enable": false } }, "procMacro": { "enable": false }
    })
}
struct Server(Client);
impl Drop for Server { fn drop(&mut self) { self.0.kill(); } }

fn pump(client: &Client, inbox: &Inbox<Incoming>, config: &Value, diagnostics: &mut Vec<String>) -> Result<(), Error> {
    for message in inbox.drain() {
        match message {
            Incoming::Request { id, method, params } => {
                let answer = if method == "workspace/configuration" {
                    Value::Array(params.get("items").and_then(Value::as_array).into_iter().flatten().map(|item| {
                        match item.get("section").and_then(Value::as_str) {
                            Some("rust-analyzer") | None => config.clone(),
                            Some(section) => section.strip_prefix("rust-analyzer.").map(|section| section.split('.').fold(config, |value, key| value.get(key).unwrap_or(&Value::Null))).cloned().unwrap_or(Value::Null),
                        }
                    }).collect())
                } else if method == "workspace/applyEdit" { json!({ "applied": false }) } else { Value::Null };
                client.reply(id, Ok(answer));
            },
            Incoming::Notification { method, params } if method == "textDocument/publishDiagnostics" => {
                *diagnostics = params.get("diagnostics").and_then(Value::as_array).into_iter().flatten()
                    .filter(|diagnostic| diagnostic.get("severity").and_then(Value::as_i64) == Some(1))
                    .filter_map(|diagnostic| diagnostic.get("message").and_then(Value::as_str))
                    .take(8).map(|text| text.chars().take(512).collect()).collect();
            },
            Incoming::Exited(_) => return Err(Error::Server(tr("The macro analyzer exited").into())),
            _ => {},
        }
    }
    Ok(())
}
fn check(cancel: &AtomicBool, started: Instant) -> Result<(), Error> {
    if cancel.load(Ordering::Acquire) { Err(Error::Cancelled) }
    else if started.elapsed() > TIMEOUT { Err(Error::Timeout) } else { Ok(()) }
}
fn wait<T>(request: &mut Pending<T>, server: &Server, inbox: &Inbox<Incoming>, config: &Value, cancel: &AtomicBool, started: Instant, diagnostics: &mut Vec<String>) -> Result<T, Error> {
    loop {
        check(cancel, started)?; pump(&server.0, inbox, config, diagnostics)?;
        if let Some(result) = request.poll() { return result.map_err(|error| Error::Server(error.message.chars().take(4096).collect())); }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn expand(definition: &str, invocation: &str, configured: Option<&Path>, cancel: &AtomicBool) -> Result<Expansion, Error> {
    check(cancel, Instant::now())?;
    let source = source(definition, invocation)?;
    // Discovery must not run a potentially hanging --version probe before cancellation is
    // installed. A missing/broken rustup component reports its process failure normally.
    let executable = if cfg!(windows) { "rust-analyzer.exe" } else { "rust-analyzer" };
    let program = configured.map(Path::to_path_buf).filter(|path| path.is_file()).or_else(|| {
        std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).take(128)
            .take_while(|_| !cancel.load(Ordering::Acquire))
            .map(|dir| dir.join(executable)).find(|path| path.is_file()))
    }).ok_or(Error::MissingAnalyzer)?;
    check(cancel, Instant::now())?;
    let dir = tempfile::Builder::new().prefix("crabcraft-macro-test-").tempdir()?;
    let file = dir.path().join("macro_test.rs"); std::fs::write(&file, &source.text)?;
    let config = configuration(&file); let inbox = Inbox::new(Waker::noop());
    let _runtime = rc_core::Runtime::handle().enter();
    let server = Server(rc_lsp::rust_analyzer::start(&program, dir.path(), &[("RUSTUP_AUTO_INSTALL".into(), "0".into())], inbox.sender())?);
    let started = Instant::now(); let mut diagnostics = Vec::new();
    let uri = rc_lsp::path_to_uri(&file);
    let mut initialize = server.0.request_raw::<Value>("initialize", json!({
        "processId": std::process::id(), "rootUri": rc_lsp::path_to_uri(dir.path()),
        "capabilities": { "workspace": { "configuration": true } }, "initializationOptions": config
    }));
    wait(&mut initialize, &server, &inbox, &config, cancel, started, &mut diagnostics)?;
    server.0.notify_raw("initialized", json!({}));
    server.0.notify_raw("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": "rust", "version": 1, "text": source.text } }));
    loop {
        check(cancel, started)?;
        let mut request = server.0.request_raw::<Value>("rust-analyzer/expandMacro", json!({
            "textDocument": { "uri": uri }, "position": { "line": source.line, "character": source.column }
        }));
        let value = wait(&mut request, &server, &inbox, &config, cancel, started, &mut diagnostics)?;
        if let Some(text) = value.get("expansion").and_then(Value::as_str) {
            if text.len() > OUTPUT_LIMIT { return Err(Error::Limit); }
            let name = value.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.len() > 256 { return Err(Error::Limit); }
            return Ok(Expansion { name: name.into(), text: text.into() });
        }
        if !diagnostics.is_empty() { return Err(Error::Server(diagnostics.join("\n"))); }
        if started.elapsed() > Duration::from_secs(5) { return Err(Error::NoExpansion); }
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct Job { inbox: Inbox<Result<Expansion, Error>>, cancel: Arc<AtomicBool>, definition: String, invocation: String }
impl Drop for Job { fn drop(&mut self) { self.cancel.store(true, Ordering::Release); } }
pub struct Tester {
    pub open: bool, pub definition: String, pub invocation: String,
    pub result: Option<Expansion>, pub error: Option<String>, job: Option<Job>,
}
impl Default for Tester {
    fn default() -> Self { Self { open: false, definition: SAMPLE.into(), invocation: "twice!(21)".into(), result: None, error: None, job: None } }
}
impl Tester {
    pub fn cancel(&mut self) { self.job = None; }
    pub fn pending(&self) -> bool { self.job.is_some() }
    pub fn start(&mut self, trusted: bool, analyzer: Option<PathBuf>, waker: Waker) {
        self.cancel(); self.result = None; self.error = None;
        if !trusted { self.error = Some(Error::Trust.to_string()); return; }
        if self.definition.len() > INPUT_LIMIT || self.invocation.len() > INPUT_LIMIT { self.error = Some(Error::Limit.to_string()); return; }
        let definition = self.definition.clone(); let invocation = self.invocation.clone();
        let inbox = Inbox::new(waker); let out = inbox.sender();
        let token = Arc::new(AtomicBool::new(false)); let cancel = token.clone();
        let inputs = (definition.clone(), invocation.clone());
        match std::thread::Builder::new().name("rc-macro-test".into()).spawn(move || {
            let result = expand(&inputs.0, &inputs.1, analyzer.as_deref(), &cancel);
            if !cancel.load(Ordering::Acquire) { out.send(result); }
        }) {
            Ok(_) => self.job = Some(Job { inbox, cancel: token, definition, invocation }),
            Err(error) => self.error = Some(Error::Io(error).to_string()),
        }
    }
    pub fn poll(&mut self) {
        if self.job.as_ref().is_some_and(|job| job.definition != self.definition || job.invocation != self.invocation) {
            self.cancel(); self.result = None; self.error = None; return;
        }
        let reply = self.job.as_ref().and_then(|job| job.inbox.drain().into_iter().next());
        if let Some(reply) = reply {
            self.job = None;
            match reply { Ok(result) => self.result = Some(result), Err(error) => self.error = Some(error.to_string()) }
        }
    }
    pub fn show(&mut self, ctx: &egui::Context, trusted: bool, analyzer: Option<PathBuf>, waker: Waker) {
        self.poll(); if !self.open { return; }
        let mut open = self.open;
        egui::Window::new(tr("Declarative Macro Tester")).id(egui::Id::new("declarative_macro_tester"))
            .open(&mut open).default_width(800.0).default_height(600.0)
            .max_height((ctx.content_rect().height() - 40.0).max(160.0)).vscroll(true).resizable(true).show(ctx, |ui| {
                ui.set_min_height(540.0);
                ui.label(tr("Edit local macro_rules definitions and one invocation. Expansion only: project code is not executed."));
                if !trusted { ui.colored_label(ui.visuals().warn_fg_color, Error::Trust.to_string()); }
                ui.label(tr("Macro definitions"));
                let changed_definition = field(ui, &mut self.definition, tr("Macro definitions"), 7);
                ui.label(tr("Macro invocation"));
                let changed_invocation = field(ui, &mut self.invocation, tr("Macro invocation"), 2);
                if changed_definition || changed_invocation { self.cancel(); self.result = None; self.error = None; }
                ui.horizontal(|ui| {
                    if ui.add_enabled(trusted && !self.pending(), egui::Button::new(tr("Test macro expansion"))).clicked() { self.start(trusted, analyzer.clone(), waker.clone()); }
                    if self.pending() { ui.spinner(); if ui.button(tr("Cancel macro test")).clicked() { self.cancel(); self.error = Some(Error::Cancelled.to_string()); } }
                });
                ui.small(tr("Isolated Rust 2024 macros only; project dependencies, procedural macros and runtime evaluation are unsupported."));
                if let Some(error) = &self.error { ui.colored_label(ui.visuals().error_fg_color, error); }
                if let Some(result) = &self.result {
                    ui.label(trf("Expansion of {macro}", &[("macro", &result.name)]));
                    let mut text: &str = &result.text;
                    egui::ScrollArea::vertical().id_salt("macro_test_expansion").max_height(260.0).show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut text).code_editor().desired_width(f32::INFINITY).desired_rows(10));
                    });
                }
                if self.pending() { ctx.request_repaint_after(Duration::from_millis(30)); }
            });
        self.open = open; if !open { self.cancel(); }
    }
}
fn field(ui: &mut egui::Ui, text: &mut String, label: &str, rows: usize) -> bool {
    let response = ui.add(egui::TextEdit::multiline(text).code_editor().desired_width(f32::INFINITY).desired_rows(rows).char_limit(INPUT_LIMIT));
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::text_edit(ui.is_enabled(), text.as_str(), text.as_str(), label);
        info.label = Some(label.into()); info
    });
    response.changed()
}

pub fn open(app: &mut crate::app::CrabCraftApp) {
    let definition = app.docs.active_doc().and_then(|doc| {
        if doc.editor.lang() != rc_syntax::Lang::Rust { return None; }
        let tree = doc.editor.syntax.tree()?;
        if tree.root_node().has_changes() { return None; }
        let rope = doc.editor.buffer.rope();
        let byte = rope.char_to_byte(doc.editor.caret());
        let mut node = tree.root_node().descendant_for_byte_range(byte, byte)?;
        while node.kind() != "macro_definition" { node = node.parent()?; }
        if node.end_byte() > rope.len_bytes() || node.byte_range().len() > INPUT_LIMIT { return None; }
        let name = node.child_by_field_name("name")?;
        Some((rope.byte_slice(node.byte_range()).to_string(), rope.byte_slice(name.byte_range()).to_string()))
    });
    app.macro_tester.cancel(); app.macro_tester.result = None; app.macro_tester.error = None;
    if let Some((definition, name)) = definition {
        app.macro_tester.definition = definition; app.macro_tester.invocation = format!("{name}!()");
    }
    app.macro_tester.open = true;
}

pub fn show(app: &mut crate::app::CrabCraftApp, ctx: &egui::Context) {
    let trusted = app.workspace.is_some() && app.runner.require_trust().is_ok();
    app.macro_tester.show(ctx, trusted, app.settings.rust_analyzer_path.clone(), app.waker.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_local_declarative_definitions_and_one_matching_call_are_accepted() {
        assert!(source(SAMPLE, "twice!(21)").is_ok());
        assert!(source(&format!("// note\n{SAMPLE}"), "twice!{\n21\n};").is_ok());
        assert!(matches!(source("fn evil() {}", "evil!()"), Err(Error::Input)));
        assert!(matches!(source(SAMPLE, "other!(21)"), Err(Error::Input)));
        assert!(matches!(source(SAMPLE, "twice!(1); twice!(2)"), Err(Error::Input)));
        assert!(matches!(source(SAMPLE, "twice!(1); fn other() {}"), Err(Error::Input)));
        assert!(matches!(source(SAMPLE, &"x".repeat(INPUT_LIMIT + 1)), Err(Error::Limit)));
        let mut tester = Tester::default();
        tester.start(false, None, Waker::noop());
        assert!(!tester.pending()); assert!(tester.error.as_deref().unwrap().contains("Trust"));
        let cancelled = AtomicBool::new(true);
        assert!(matches!(expand(SAMPLE, "twice!(2)", None, &cancelled), Err(Error::Cancelled)));
        assert!(configuration(Path::new("/tmp/macro.rs"))["procMacro"]["enable"] == false);
    }

    #[test]
    fn real_rust_analyzer_expands_repetition_and_nested_declarative_macros() {
        let definition = "macro_rules! add { ($a:expr, $b:expr) => { ($a) + ($b) }; }\nmacro_rules! twice { ($a:expr) => { add!($a, $a) }; }";
        let expansion = expand(definition, "twice!(21)", None, &AtomicBool::new(false)).unwrap();
        assert!(expansion.text.contains("21"), "{expansion:?}");
        assert!(!expansion.text.contains("add!"), "{expansion:?}");
        let repetition = expand("macro_rules! tuple { ($($x:expr),*) => { ($($x,)*) }; }", "tuple!(1, 2, 3)", None, &AtomicBool::new(false)).unwrap();
        assert!(repetition.text.contains('3'), "{repetition:?}");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, format!("fn main() {{ assert_eq!({}, 42); assert_eq!({}, (1, 2, 3)); }}", expansion.text, repetition.text)).unwrap();
        let binary = dir.path().join(if cfg!(windows) { "expanded.exe" } else { "expanded" });
        let rustc = std::process::Command::new("rustc").args(["--edition=2024"]).arg(&file).arg("-o").arg(&binary).current_dir(dir.path()).output().unwrap();
        assert!(rustc.status.success(), "{}", String::from_utf8_lossy(&rustc.stderr));
        assert!(std::process::Command::new(binary).status().unwrap().success());
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_a_hanging_macro_analyzer_kills_its_process_and_discards_the_reply() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("analyzer");
        let pid = dir.path().join("pid");
        std::fs::write(&program, format!("#!/bin/sh\nprintf '%s' $$ > '{}'\nexec sleep 30\n", pid.display())).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut tester = Tester::default(); tester.start(true, Some(program), Waker::noop());
        let end = Instant::now() + Duration::from_secs(5);
        while !pid.is_file() { assert!(Instant::now() < end); std::thread::sleep(Duration::from_millis(10)); }
        let pid: i32 = std::fs::read_to_string(pid).unwrap().parse().unwrap();
        tester.invocation = "twice!(22)".into(); tester.poll();
        assert!(!tester.pending()); assert!(tester.result.is_none());
        loop {
            // SAFETY: signal zero probes this test-owned process without sending a signal.
            if unsafe { libc::kill(pid, 0) } != 0 { break; }
            assert!(Instant::now() < end, "cancelled analyzer is still running");
            std::thread::sleep(Duration::from_millis(10));
        }
        tester.poll(); assert!(tester.result.is_none());
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    #[test]
    fn native_macro_lab_tests_unsaved_rust_definitions_without_running_the_project() {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap(); let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"macro_lab_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(root.path().join("build.rs"), "fn main(){std::fs::write(\"build-ran\", \"yes\").unwrap();}").unwrap();
        let path = root.path().join("src/main.rs");
        let original = format!("{SAMPLE}\nfn main() {{ assert_eq!(twice!(21), 42); }}\n");
        std::fs::write(&path, &original).unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace(root.path()); app.open_file_and_wait(&path);
        let unsaved = original.replacen("($value) + ($value)", "($value) + ($value) + 1", 1);
        let doc = app.docs.active_doc_mut().unwrap();
        doc.editor.replace(0, doc.editor.buffer.len_chars(), &unsaved);
        doc.editor.goto(0, 0);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 900.0));
        let end = Instant::now() + Duration::from_secs(10);
        while h.state().docs.active_doc().unwrap().editor.syntax.tree().is_none_or(|tree| tree.root_node().has_changes()) {
            h.run_steps(1); assert!(Instant::now() < end); std::thread::sleep(Duration::from_millis(10));
        }
        h.state_mut().run_ide_action(&egui::Context::default(), rc_editor::Action::TestDeclarativeMacro);
        h.run_steps(3);
        assert_eq!(h.state().macro_tester.definition, SAMPLE.replace("($value) + ($value)", "($value) + ($value) + 1"));
        assert_eq!(h.state().macro_tester.invocation, "twice!()");
        h.get_by_label(tr("Test macro expansion")).click(); h.run_steps(2);
        assert!(!h.state().macro_tester.pending()); assert!(h.state().macro_tester.result.is_none());
        assert!(!root.path().join("build-ran").exists());
        // Metadata is explicitly authorized without starting the project's analyzer.
        h.state_mut().trust_project_tools_for_test(); h.run_steps(2);
        let label = tr("Macro invocation");
        h.get_all_by_label(label).last().unwrap().click(); h.run_steps(2);
        let ctrl = egui::Modifiers { command: true, ctrl: !cfg!(target_os="macos"), mac_cmd: cfg!(target_os="macos"), ..Default::default() };
        h.key_press_modifiers(ctrl, egui::Key::A); h.run_steps(1);
        h.get_all_by_label(label).last().unwrap().type_text("twice!(21)"); h.run_steps(2);
        assert_eq!(h.state().macro_tester.invocation, "twice!(21)");
        h.get_by_label(tr("Test macro expansion")).click();
        let end = Instant::now() + Duration::from_secs(25);
        loop {
            h.run_steps(1);
            if !h.state().macro_tester.pending() && (h.state().macro_tester.result.is_some() || h.state().macro_tester.error.is_some()) { break; }
            assert!(Instant::now() < end); std::thread::sleep(Duration::from_millis(10));
        }
        let tester = &h.state().macro_tester;
        let result = tester.result.as_ref().unwrap_or_else(|| panic!("{:?}", tester.error));
        assert!(result.text.contains("21") && result.text.chars().filter(|c| !c.is_whitespace()).collect::<String>().contains("+1"), "{result:?}");
        h.get_by_label(&trf("Expansion of {macro}", &[("macro", &result.name)]));
        assert!(h.state().insight.session.is_none(), "tester does not start project analysis");
        assert!(!root.path().join("build-ran").exists()); assert!(!root.path().join("target").exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string(), unsaved);
        // Editing input immediately invalidates the rendered result.
        h.get_all_by_label(label).last().unwrap().click(); h.run_steps(1);
        h.key_press_modifiers(ctrl, egui::Key::A); h.run_steps(1);
        h.get_all_by_label(label).last().unwrap().type_text("twice!(1, 2)"); h.run_steps(2);
        assert!(h.state().macro_tester.result.is_none());
        h.get_by_label(tr("Test macro expansion")).click(); h.run_steps(1);
        assert!(h.state().macro_tester.pending() || h.state().macro_tester.error.is_some());
        let end = Instant::now() + Duration::from_secs(25);
        while h.state().macro_tester.pending() { h.run_steps(1); assert!(Instant::now() < end); std::thread::sleep(Duration::from_millis(10)); }
        // A matcher error must remain visible, not turn into an old successful expansion.
        assert!(h.state().macro_tester.error.is_some() || h.state().macro_tester.result.as_ref().is_some_and(|result| result.text.contains("error")), "{:?} {:?}", h.state().macro_tester.result, h.state().macro_tester.error);
        assert!(!root.path().join("build-ran").exists());
        h.set_size(egui::vec2(900.0, 560.0)); h.run_steps(4);
        let button = h.get_by_label(tr("Test macro expansion")).rect();
        assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 560.0)).contains_rect(button), "{button:?}");
    }
}
