//! Explicit conditional GET/PUT of portable preferences. No account, secret or project trust
//! is exported, and downloaded preferences always go through the existing import preview.
use rc_core::{
    Inbox, Settings, Waker,
    settings_transfer::{Backup, MAX_BYTES},
};
use rc_kit::i18n::tr;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Revision {
    Unknown,
    Absent,
    Present(String),
}
#[derive(Debug)]
enum Error {
    Url,
    Auth,
    Network,
    Status(u16),
    Conflict,
    Revision,
    Backup,
    Limit,
}
impl Error {
    fn text(&self) -> String {
        match self {
            Self::Url => tr("Use an HTTPS preferences URL, or HTTP on loopback, without embedded credentials or fragments.").into(),
            Self::Auth => tr("The synchronization token is invalid.").into(),
            Self::Network => tr("The preferences server could not be reached. Check the connection and authentication.").into(),
            Self::Status(status) => rc_kit::i18n::trf("The preferences server returned HTTP {status}", &[("status", status)]),
            Self::Conflict => tr("Remote preferences changed. Download and review them before uploading again.").into(),
            Self::Revision => tr("Download first. The server must provide a strong ETag and honor conditional writes.").into(),
            Self::Backup => tr("The remote preferences contain invalid or unsupported settings.").into(),
            Self::Limit => tr("The preferences backup exceeds 1 MB.").into(),
        }
    }
}
enum Reply {
    Downloaded(Backup, Revision),
    Missing,
    Uploaded(Revision),
}
fn endpoint(text: &str) -> Result<reqwest::Url, Error> {
    if text.len() > 8192 {
        return Err(Error::Url);
    }
    let url = reqwest::Url::parse(text).map_err(|_| Error::Url)?;
    let loopback =
        url.host_str().is_some_and(|host| host == "localhost" || host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()));
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(Error::Url);
    }
    Ok(url)
}
fn revision(headers: &reqwest::header::HeaderMap) -> Revision {
    headers
        .get(reqwest::header::ETAG)
        .and_then(|h| h.to_str().ok())
        .filter(|v| v.len() <= 1024 && v.starts_with('"') && v.ends_with('"') && !v.contains(['\r', '\n']))
        .map(|v| Revision::Present(v.into()))
        .unwrap_or(Revision::Unknown)
}
async fn transfer(url: String, token: String, upload: Option<(Backup, Revision)>) -> Result<Reply, Error> {
    let url = endpoint(&url)?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_| Error::Network)?;
    let mut request = if let Some((backup, revision)) = &upload {
        let bytes = backup.to_bytes().map_err(|_| Error::Backup)?;
        let request = client.put(url).header(reqwest::header::CONTENT_TYPE, "application/json").body(bytes);
        match revision {
            Revision::Absent => request.header(reqwest::header::IF_NONE_MATCH, "*"),
            Revision::Present(etag) => request.header(reqwest::header::IF_MATCH, etag),
            Revision::Unknown => return Err(Error::Revision),
        }
    } else {
        client.get(url)
    };
    if !token.is_empty() {
        let value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| Error::Auth)?;
        request = request.header(reqwest::header::AUTHORIZATION, value);
    }
    let mut response = request.send().await.map_err(|_| Error::Network)?;
    let status = response.status().as_u16();
    if status == 409 || status == 412 {
        return Err(Error::Conflict);
    }
    if upload.is_none() && status == 404 {
        return Ok(Reply::Missing);
    }
    if !response.status().is_success() {
        return Err(Error::Status(status));
    }
    let revision = revision(response.headers());
    if upload.is_some() {
        return Ok(Reply::Uploaded(revision));
    }
    if response.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
        return Err(Error::Limit);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
        if chunk.len() > MAX_BYTES - bytes.len() {
            return Err(Error::Limit);
        }
        bytes.extend_from_slice(&chunk);
    }
    Backup::from_bytes(&bytes).map(|backup| Reply::Downloaded(backup, revision)).map_err(|_| Error::Backup)
}

struct Job {
    inbox: Inbox<Result<Reply, Error>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub struct Sync {
    url: String,
    token: String,
    revision: Revision,
    bound: String,
    job: Option<Job>,
    message: Option<String>,
}
impl Default for Sync {
    fn default() -> Self {
        Self { url: String::new(), token: String::new(), revision: Revision::Unknown, bound: String::new(), job: None, message: None }
    }
}
impl Sync {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    fn start(&mut self, upload: Option<Backup>, waker: Waker) {
        if self.job.is_some() {
            return;
        }
        self.bound = self.url.clone();
        self.message = None;
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let url = self.url.clone();
        let token = self.token.clone();
        let upload = upload.map(|backup| (backup, self.revision.clone()));
        let task = rc_core::Runtime::spawn(async move {
            out.send(transfer(url, token, upload).await);
        });
        self.job = Some(Job { inbox, task });
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, settings: &Settings, waker: Waker, enabled: bool) -> Option<Backup> {
        let mut preview = None;
        if let Some(result) = self.job.as_ref().and_then(|job| job.inbox.drain().into_iter().next()) {
            self.job = None;
            match result {
                Ok(Reply::Downloaded(backup, revision)) => {
                    self.revision = revision;
                    preview = Some(backup);
                    self.message = Some(tr("Remote preferences downloaded. Review the preview before applying.").into());
                }
                Ok(Reply::Missing) => {
                    self.revision = Revision::Absent;
                    self.message = Some(tr("No remote preferences exist. Upload can create them.").into());
                }
                Ok(Reply::Uploaded(revision)) => {
                    self.revision = revision;
                    self.message = Some(tr("Portable preferences uploaded.").into());
                }
                Err(error) => {
                    if matches!(error, Error::Conflict) {
                        self.revision = Revision::Unknown;
                    }
                    self.message = Some(error.text());
                }
            }
        }
        ui.separator();
        ui.heading(tr("Preferences synchronization"));
        ui.label(tr("Use a private HTTPS storage URL supporting GET, PUT and strong ETags. Only portable preferences are sent after Upload; Download always opens a preview. URL and token stay in memory until this dialog closes."));
        let busy = self.job.is_some();
        ui.add_enabled_ui(!busy && enabled, |ui| {
            let label = ui.label(tr("Preferences storage URL"));
            if ui.add(egui::TextEdit::singleline(&mut self.url).char_limit(8192).desired_width(450.0)).labelled_by(label.id).changed() {
                self.revision = Revision::Unknown;
                self.bound.clear();
            }
            let label = ui.label(tr("Bearer token (optional)"));
            if ui.add(egui::TextEdit::singleline(&mut self.token).password(true).char_limit(4096).desired_width(450.0)).labelled_by(label.id).changed() {
                self.revision = Revision::Unknown;
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(endpoint(&self.url).is_ok(), egui::Button::new(tr("Download remote preferences"))).clicked() {
                    self.start(None, waker.clone());
                }
                if ui
                    .add_enabled(
                        endpoint(&self.url).is_ok() && self.bound == self.url && self.revision != Revision::Unknown,
                        egui::Button::new(tr("Upload portable preferences")),
                    )
                    .clicked()
                {
                    match Backup::from_settings(settings) {
                        Ok(backup) => self.start(Some(backup), waker),
                        Err(_) => self.message = Some(Error::Backup.text()),
                    }
                }
            });
        });
        if busy {
            ui.spinner();
            if ui.button(tr("Cancel synchronization")).clicked() {
                self.job = None;
                self.revision = Revision::Unknown;
            }
        }
        if let Some(message) = &self.message {
            ui.label(message);
        }
        preview
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };
    #[derive(Default)]
    struct Store {
        body: Option<Vec<u8>>,
        revision: u64,
        requests: usize,
    }
    struct Server {
        url: String,
        store: Arc<Mutex<Store>>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }
    fn server() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/preferences", listener.local_addr().unwrap());
        let store = Arc::new(Mutex::new(Store::default()));
        let shared = store.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let worker = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    bytes.push(byte[0]);
                    assert!(bytes.len() < 16384);
                }
                let headers = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
                let length = headers.lines().find_map(|line| line.strip_prefix("content-length: ").and_then(|v| v.parse::<usize>().ok())).unwrap_or(0);
                assert!(length <= MAX_BYTES);
                let mut incoming = vec![0; length];
                if stream.read_exact(&mut incoming).is_err() {
                    continue;
                }
                let mut store = shared.lock().unwrap();
                store.requests += 1;
                let etag = format!("\"v{}\"", store.revision);
                let (status, body) = if !headers.contains("authorization: bearer fixture-token\r\n") {
                    (403, Vec::new())
                } else if headers.starts_with("get ") {
                    match &store.body {
                        Some(body) => (200, body.clone()),
                        None => (404, Vec::new()),
                    }
                } else if headers.starts_with("put ") {
                    let allowed =
                        if store.body.is_none() { headers.contains("if-none-match: *\r\n") } else { headers.contains(&format!("if-match: {etag}\r\n")) };
                    if allowed {
                        store.body = Some(incoming);
                        store.revision += 1;
                        (204, Vec::new())
                    } else {
                        (412, Vec::new())
                    }
                } else {
                    (405, Vec::new())
                };
                let response =
                    format!("HTTP/1.1 {status} Fixture\r\nETag: \"v{}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", store.revision, body.len());
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Server { url, store, stop, worker: Some(worker) }
    }

    #[test]
    fn conditional_sync_never_overwrites_a_newer_remote_revision_or_sends_local_secrets() {
        let server = server();
        rc_core::Runtime::block_on(async {
            assert!(matches!(transfer(server.url.clone(), "fixture-token".into(), None).await.unwrap(), Reply::Missing));
            let settings = Settings {
                editor_font_size: 20.0,
                trusted_projects: vec!["/private/project".into()],
                rust_analyzer_path: Some("/private/tool".into()),
                ..Default::default()
            };
            let backup = Backup::from_settings(&settings).unwrap();
            let Reply::Uploaded(rev) = transfer(server.url.clone(), "fixture-token".into(), Some((backup.clone(), Revision::Absent))).await.unwrap() else {
                panic!("not uploaded");
            };
            let body = server.store.lock().unwrap().body.clone().unwrap();
            let text = String::from_utf8(body).unwrap();
            assert!(!text.contains("/private") && !text.contains("trusted_projects") && !text.contains("fixture-token"));
            let Reply::Downloaded(downloaded, current) = transfer(server.url.clone(), "fixture-token".into(), None).await.unwrap() else {
                panic!("not downloaded");
            };
            assert_eq!(rev, current);
            assert_eq!(downloaded.apply(&Settings::default()).unwrap().editor_font_size, 20.0);
            server.store.lock().unwrap().revision += 1;
            assert!(matches!(transfer(server.url.clone(), "fixture-token".into(), Some((backup, rev))).await, Err(Error::Conflict)));
            assert_eq!(server.store.lock().unwrap().revision, 2);
        });
    }

    #[test]
    fn unsafe_urls_missing_revisions_invalid_backups_and_tokens_are_rejected() {
        for url in
            ["file:///tmp/preferences", "http://example.com/settings", "https://user:password@example.com/settings", "https://example.com/settings#fragment"]
        {
            assert!(endpoint(url).is_err());
        }
        assert!(endpoint("https://example.com/settings").is_ok());
        assert!(endpoint("http://[::1]:1234/settings").is_ok());
        let server = server();
        rc_core::Runtime::block_on(async {
            let backup = Backup::from_settings(&Settings::default()).unwrap();
            assert!(matches!(transfer(server.url.clone(), "fixture-token".into(), Some((backup, Revision::Unknown))).await, Err(Error::Revision)));
            assert_eq!(server.store.lock().unwrap().requests, 0);
            assert!(matches!(transfer(server.url.clone(), "bad\r\nheader".into(), None).await, Err(Error::Auth)));
            server.store.lock().unwrap().body = Some(br#"{"format":"CrabCraft preferences","version":1,"preferences":{"trusted_projects":["/tmp"]}}"#.to_vec());
            assert!(matches!(transfer(server.url.clone(), "fixture-token".into(), None).await, Err(Error::Backup)));
            server.store.lock().unwrap().body = Some(vec![b'x'; MAX_BYTES + 1]);
            assert!(matches!(transfer(server.url.clone(), "fixture-token".into(), None).await, Err(Error::Limit)));
        });
    }

    #[test]
    fn closing_sync_cancels_the_worker_and_forgets_credentials() {
        let inbox = Inbox::new(Waker::noop());
        let out = inbox.sender();
        let task = rc_core::Runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            out.send(Ok(Reply::Missing));
        });
        let handle = task.abort_handle();
        let mut sync =
            Sync { url: "https://example.com/preferences".into(), token: "fixture-token".into(), job: Some(Job { inbox, task }), ..Default::default() };
        sync.clear();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !handle.is_finished() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert!(sync.url.is_empty() && sync.token.is_empty() && sync.job.is_none());
        assert_eq!(sync.revision, Revision::Unknown);
    }

    #[test]
    fn native_download_requires_preview_and_ok_in_a_real_untrusted_rust_project() {
        use egui_kittest::kittest::Queryable;
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[package]\nname='sync-audit'\nversion='0.1.0'\nedition='2024'\n[workspace]\n").unwrap();
        std::fs::write(project.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(project.path().join("build.rs"), "fn main() { std::fs::write(\"executed\", \"bad\").unwrap(); }\n").unwrap();
        let server = server();
        server.store.lock().unwrap().body =
            Some(Backup::from_settings(&Settings { editor_font_size: 27.0, ..Default::default() }).unwrap().to_bytes().unwrap());
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(project.path());
        app.open_settings(crate::ui::dialogs::SettingsPage::Backup);
        let mut h = crate::screens::harness(app, egui::vec2(1440.0, 1100.0));
        h.get_by_label(tr("Preferences storage URL")).click();
        h.run_steps(2);
        h.get_by_label(tr("Preferences storage URL")).type_text(&server.url);
        h.run_steps(2);
        h.get_by_label(tr("Bearer token (optional)")).click();
        h.run_steps(2);
        h.get_by_label(tr("Bearer token (optional)")).type_text("fixture-token");
        h.run_steps(4);
        assert_eq!(server.store.lock().unwrap().requests, 0);
        h.get_by_label(tr("Download remote preferences")).click();
        h.run_steps(3);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while h.state().settings_backup.preview.is_none() {
            h.run_steps(2);
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(h.state().settings.editor_font_size, 13.0);
        h.run_steps(4);
        h.get_by_label(tr("Apply imported preferences")).click();
        h.run_steps(3);
        assert_eq!(h.state().settings.editor_font_size, 13.0);
        h.get_by_label(tr("OK")).click();
        h.run_steps(3);
        assert_eq!(h.state().settings.editor_font_size, 27.0);
        assert!(h.state().settings.trusted_projects.is_empty());
        assert!(!project.path().join("executed").exists() && !project.path().join("target").exists());
        assert!(!std::fs::read_to_string(rc_core::Settings::file()).unwrap().contains("fixture-token"));
    }
}
