//! Explicit, bounded HTTP environment snapshots. No automatic discovery or ambient env access.
use super::{HttpError, HttpRequest};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::{collections::BTreeMap, io::Read, path::Path, sync::{Arc, atomic::{AtomicBool, Ordering}}};
const MAX: usize = 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error { Read, NotFile, TooLarge, Invalid, Missing, Cancelled, NotLoaded }
impl Error {
    pub fn text(&self) -> String { tr(match self {
        Self::Read => "Cannot read the HTTP environment file.",
        Self::NotFile => "The HTTP environment path must be a regular file.",
        Self::TooLarge => "The HTTP environment exceeds its size or variable limit.",
        Self::Invalid => "HTTP environments must contain named objects with string values.",
        Self::Missing => "The selected HTTP environment does not exist.",
        Self::Cancelled => "Cancelled",
        Self::NotLoaded => "Load the selected HTTP environment before sending a request.",
    }).to_string() }
}
#[derive(Clone, Debug)]
pub struct Snapshot { pub names: Vec<String>, pub variables: BTreeMap<String, String> }
pub fn parse(text: &str, selected: &str) -> Result<Snapshot, Error> {
    if text.len() > MAX { return Err(Error::TooLarge); }
    let root: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(text).map_err(|_| Error::Invalid)?;
    if root.len() > 64 || root.iter().any(|(name, values)| name.is_empty() || name.len() > 128 || values.len() > 256 || values.keys().any(|key| key.is_empty() || key.len() > 128)) { return Err(Error::TooLarge); }
    let values = root.get(selected).filter(|_| selected != "$shared").ok_or(Error::Missing)?;
    let mut variables = root.get("$shared").cloned().unwrap_or_default();
    variables.extend(values.clone());
    if variables.len() > 256 { return Err(Error::TooLarge); }
    Ok(Snapshot { names: root.keys().filter(|name| *name != "$shared").cloned().collect(), variables })
}
pub fn read(path: &Path, selected: &str, cancel: &AtomicBool) -> Result<Snapshot, Error> {
    if cancel.load(Ordering::Relaxed) { return Err(Error::Cancelled); }
    let metadata = std::fs::metadata(path).map_err(|_| Error::Read)?;
    if !metadata.is_file() { return Err(Error::NotFile); }
    let mut options = std::fs::OpenOptions::new(); options.read(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.custom_flags(libc::O_NONBLOCK); }
    let mut file = options.open(path).map_err(|_| Error::Read)?;
    if !file.metadata().map_err(|_| Error::Read)?.is_file() { return Err(Error::NotFile); }
    let mut bytes = Vec::new(); let mut chunk = [0u8; 8192];
    loop {
        if cancel.load(Ordering::Relaxed) { return Err(Error::Cancelled); }
        let n = file.read(&mut chunk).map_err(|_| Error::Read)?;
        if n == 0 { break; }
        if bytes.len() + n > MAX { return Err(Error::TooLarge); }
        bytes.extend_from_slice(&chunk[..n]);
    }
    parse(std::str::from_utf8(&bytes).map_err(|_| Error::Invalid)?, selected)
}
pub fn merge(request: &HttpRequest, snapshot: &Snapshot) -> Result<HttpRequest, HttpError> {
    let mut request = request.clone();
    let mut values = snapshot.variables.clone(); values.extend(request.variables.iter().map(|(key, value)| (key.clone(), value.clone())));
    if values.len() > 256 { return Err(HttpError::Environment(Error::TooLarge)); }
    request.variables = values.into(); Ok(request)
}
pub struct State {
    pub path: String, pub selected: String,
    inbox: Inbox<(u64, String, String, Result<Snapshot, Error>)>, generation: u64,
    cancel: Option<Arc<AtomicBool>>, loaded: Option<(String, String, Snapshot)>, error: Option<Error>,
}
impl State {
    pub fn new(waker: Waker) -> Self { Self { path: String::new(), selected: String::new(), inbox: Inbox::new(waker), generation: 0, cancel: None, loaded: None, error: None } }
    pub fn load(&mut self) {
        self.stop(); self.loaded = None; self.error = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation; let path = self.path.clone(); let selected = self.selected.clone();
        let cancel = Arc::new(AtomicBool::new(false)); self.cancel = Some(cancel.clone());
        let tx = self.inbox.sender();
        std::thread::spawn(move || { let result = read(Path::new(&path), &selected, &cancel); tx.send((generation, path, selected, result)); });
    }
    fn stop(&mut self) { if let Some(cancel) = self.cancel.take() { cancel.store(true, Ordering::Relaxed); } }
    pub fn poll(&mut self) -> bool {
        let results = self.inbox.drain(); let changed = !results.is_empty();
        for (generation, path, selected, result) in results {
            if generation != self.generation || self.cancel.is_none() { continue; }
            if path != self.path || selected != self.selected { self.stop(); continue; }
            self.cancel = None;
            match result { Ok(snapshot) => self.loaded = Some((self.path.clone(), self.selected.clone(), snapshot)), Err(error) => self.error = Some(error) }
        }
        changed
    }
    pub fn resolve(&self, request: &HttpRequest) -> Result<HttpRequest, HttpError> {
        if self.path.is_empty() && self.selected.is_empty() { return Ok(request.clone()); }
        let (_, _, snapshot) = self.loaded.as_ref().filter(|(path, selected, _)| *path == self.path && *selected == self.selected).ok_or(HttpError::Environment(Error::NotLoaded))?;
        merge(request, snapshot)
    }
    pub fn show(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(tr("HTTP environment")).show(ui, |ui| {
            ui.label(tr("Choose a JSON file and environment explicitly. Values stay in this session; file variables take precedence."));
            let changed = ui.horizontal(|ui| { ui.label(tr("Environment file")); ui.add(egui::TextEdit::singleline(&mut self.path).char_limit(4096)).changed() }).inner
                | ui.horizontal(|ui| { ui.label(tr("Environment name")); ui.add(egui::TextEdit::singleline(&mut self.selected).char_limit(128)).changed() }).inner;
            if changed { self.stop(); self.loaded = None; self.error = None; }
            ui.horizontal(|ui| {
                if ui.add_enabled(self.cancel.is_none() && !self.path.is_empty() && !self.selected.is_empty(), egui::Button::new(tr("Load HTTP environment"))).clicked() { self.load(); }
                if ui.add_enabled(self.cancel.is_some(), egui::Button::new(tr("Cancel"))).clicked() { self.stop(); self.error = Some(Error::Cancelled); }
                if ui.button(tr("Clear HTTP environment")).clicked() { self.stop(); self.path.clear(); self.selected.clear(); self.loaded = None; self.error = None; }
            });
            if let Some((_, _, snapshot)) = &self.loaded { ui.label(trf("Loaded {count} HTTP variables. Available environments: {names}", &[("count", &snapshot.variables.len()), ("names", &snapshot.names.join(", "))])); }
            if let Some(error) = &self.error { ui.colored_label(rc_kit::pal(ui).error, error.text()); }
        });
    }
}
impl Drop for State { fn drop(&mut self) { self.stop(); } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_selected_and_file_precedence_are_literal_bounded_and_fail_closed() {
        let snapshot = parse(r#"{"$shared":{"host":"unused","token":"shared"},"local":{"host":"127.0.0.1","base":"http://{{host}}"},"production":{"host":"example.test"}}"#, "local").unwrap();
        let request = rc_syntax::http::parse_requests("@token = file\nGET {{base}}/café\nAuthorization: Bearer {{token}}\n").remove(0);
        let prepared = super::super::prepare(&merge(&request, &snapshot).unwrap()).unwrap();
        assert_eq!(prepared.url, "http://127.0.0.1/caf%C3%A9");
        assert_eq!(prepared.headers, [("Authorization".into(), "Bearer file".into())]);
        assert_eq!(parse("{\"local\":{\"port\":80}}", "local").unwrap_err(), Error::Invalid);
        assert_eq!(parse("{\"local\":{}}", "absent").unwrap_err(), Error::Missing);
        assert_eq!(parse("{\"$shared\":{}}", "$shared").unwrap_err(), Error::Missing);
        assert_eq!(parse(&"x".repeat(MAX+1), "local").unwrap_err(), Error::TooLarge);
        let mut state = State::new(Waker::noop()); state.path = "missing.json".into(); state.selected = "local".into();
        assert!(matches!(state.resolve(&request), Err(HttpError::Environment(Error::NotLoaded))));
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("env.json");
        std::fs::write(&path, r#"{"local":{}}"#).unwrap();
        assert_eq!(read(&path, "local", &AtomicBool::new(true)).unwrap_err(), Error::Cancelled);
        #[cfg(unix)] {
            use std::os::unix::ffi::OsStrExt;
            let fifo = dir.path().join("fifo"); let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            assert_eq!(read(&fifo, "local", &AtomicBool::new(false)).unwrap_err(), Error::NotFile);
        }
    }
    #[test]
    fn native_environment_loading_sends_only_after_explicit_request_in_a_rust_project() {
        use std::io::{Read, Write};
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap(); let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[package]\nname=\"http_environment_proof\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(project.path().join("src/main.rs"), "fn main() { println!(\"environment proof\"); }\n").unwrap();
        std::fs::write(project.path().join("build.rs"), "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); listener.set_nonblocking(true).unwrap();
        let path = project.path().join("http-client.env.json");
        std::fs::write(&path, serde_json::json!({"local":{"base":format!("http://{}", listener.local_addr().unwrap()),"token":"private-fixture"}}).to_string()).unwrap();
        let mut app = crate::screens::test_app(home.path(), true); app.open_workspace(project.path());
        app.http.environment.path = path.to_string_lossy().into_owned(); app.http.environment.selected = "local".into();
        app.toggle_tool(crate::toolwindow::ToolWindow::Http);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 850.0));
        use egui_kittest::kittest::Queryable;
        h.get_by_label(tr("HTTP environment")).click(); h.run_steps(2);
        h.get_by_label(tr("Load HTTP environment")).click(); h.run_steps(2);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while h.state().http.environment.loaded.is_none() { h.run_steps(1); assert!(std::time::Instant::now()<until); std::thread::sleep(std::time::Duration::from_millis(10)); }
        assert!(listener.accept().is_err()); assert!(!project.path().join("build-ran").exists());
        listener.set_nonblocking(false).unwrap();
        let server = std::thread::spawn(move || { let (mut peer,_) = listener.accept().unwrap(); peer.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap(); let mut bytes=[0;4096]; let n=peer.read(&mut bytes).unwrap(); let request=String::from_utf8_lossy(&bytes[..n]); assert!(request.starts_with("GET /verified HTTP/1.1")); assert!(request.to_ascii_lowercase().contains("authorization: bearer private-fixture")); peer.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap(); });
        let req=rc_syntax::http::parse_requests("GET {{base}}/verified\nAuthorization: Bearer {{token}}\n").remove(0);
        h.state_mut().http.start(&req, project.path().join("api.http"));
        while h.state().http.running() { h.run_steps(1); assert!(std::time::Instant::now()<until); std::thread::sleep(std::time::Duration::from_millis(10)); }
        server.join().unwrap(); assert!(matches!(h.state().http.history[0].outcome, super::super::Outcome::Done { .. }));
        assert!(!project.path().join("build-ran").exists());
        h.state_mut().http.environment.selected = "other".into(); assert!(h.state().http.environment.resolve(&req).is_err());
        let output = std::process::Command::new("cargo").args(["run","--offline","--quiet"]).current_dir(project.path()).output().unwrap(); assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    }
}
