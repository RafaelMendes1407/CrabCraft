//! Explicit OAuth client-credentials exchange. Tokens never enter project/settings storage.
use super::{HttpError, Prepared};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::tr;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub endpoint: String,
    pub client: String,
    pub secret: String,
    pub scope: String,
    pub resource: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Configuration,
    Network,
    Response,
    Cancelled,
    Missing,
    Origin,
    Authorization,
}
impl Error {
    pub fn text(&self) -> String {
        tr(match self {
            Self::Configuration => "OAuth requires HTTPS endpoints, a resource origin, and client credentials; loopback HTTP is allowed for local testing.",
            Self::Network => "The OAuth token request failed. Credentials and server details are hidden.",
            Self::Response => "The OAuth server returned an invalid or unsupported token response.",
            Self::Cancelled => "Cancelled",
            Self::Missing => "Acquire a current OAuth token before sending this request.",
            Self::Origin => "The request origin does not match the reviewed OAuth resource origin.",
            Self::Authorization => "Remove the request Authorization header before enabling the OAuth token.",
        })
        .to_string()
    }
}
fn url(text: &str) -> Result<reqwest::Url, Error> {
    let url = reqwest::Url::parse(text).map_err(|_| Error::Configuration)?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1"));
    if url.host_str().is_none()
        || (!matches!(url.scheme(), "https") && !(url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Configuration);
    }
    Ok(url)
}
fn validate(config: &Config) -> Result<(reqwest::Url, reqwest::Url), Error> {
    if config.endpoint.len() > 4096
        || config.resource.len() > 4096
        || config.client.is_empty()
        || config.client.len() > 4096
        || config.secret.is_empty()
        || config.secret.len() > 8192
        || config.scope.len() > 4096
    {
        return Err(Error::Configuration);
    }
    let endpoint = url(&config.endpoint)?;
    let resource = url(&config.resource)?;
    if endpoint.query().is_some() || resource.query().is_some() || resource.path() != "/" {
        return Err(Error::Configuration);
    }
    Ok((endpoint, resource))
}
fn encoded(text: &str) -> String {
    let mut output = String::new();
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => output.push(char::from(byte)),
            b' ' => output.push('+'),
            _ => {
                output.push('%');
                output.push(char::from(HEX[usize::from(byte >> 4)]));
                output.push(char::from(HEX[usize::from(byte & 15)]));
            }
        }
    }
    output
}
pub struct Token {
    value: String,
    deadline: Instant,
}
impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token([redacted])")
    }
}
pub fn acquire(config: &Config, cancel: &AtomicBool) -> Result<Token, Error> {
    let (endpoint, _) = validate(config)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_| Error::Network)?;
    runtime.block_on(async {
        let work = async {
            let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(30)).build().map_err(|_| Error::Network)?;
            let body=format!("grant_type=client_credentials&scope={}",encoded(&config.scope));
            let mut response=client.post(endpoint).basic_auth(encoded(&config.client),Some(encoded(&config.secret))).header("Content-Type","application/x-www-form-urlencoded").header("Accept","application/json").body(body).send().await.map_err(|_| Error::Network)?;
            if !response.status().is_success() { return Err(Error::Response); }
            if response.content_length().is_some_and(|n|n>65536) { return Err(Error::Response); }
            let mut bytes=Vec::new();
            while let Some(chunk)=response.chunk().await.map_err(|_|Error::Network)? { if bytes.len()+chunk.len()>65536 {return Err(Error::Response);} bytes.extend_from_slice(&chunk); }
            #[derive(serde::Deserialize)] struct Reply { access_token: String, token_type: String, expires_in: u64 }
            let reply:Reply=serde_json::from_slice(&bytes).map_err(|_|Error::Response)?;
            if !reply.token_type.eq_ignore_ascii_case("Bearer") || reply.access_token.is_empty() || reply.access_token.len()>8192 || !reply.access_token.bytes().all(|b|b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b)) || !(1..=86400).contains(&reply.expires_in) { return Err(Error::Response); }
            // Expiry is anchored before the exchange below.
            Ok((reply.access_token,reply.expires_in))
        };
        let started=Instant::now();
        tokio::select! {
            result=work => { let (value,seconds)=result?; let deadline=started.checked_add(Duration::from_secs(seconds)).ok_or(Error::Response)?; if Instant::now()>=deadline {return Err(Error::Response);} Ok(Token {value,deadline}) },
            _=async {while !cancel.load(Ordering::Relaxed) {tokio::time::sleep(Duration::from_millis(20)).await;}} => Err(Error::Cancelled),
        }
    })
}
pub struct State {
    pub config: Config,
    pub enabled: bool,
    inbox: Inbox<(u64, Config, Result<Token, Error>)>,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    token: Option<(Config, Token)>,
    error: Option<Error>,
}
impl State {
    pub fn new(waker: Waker) -> Self {
        Self { config: Config::default(), enabled: false, inbox: Inbox::new(waker), generation: 0, cancel: None, token: None, error: None }
    }
    fn stop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    pub fn acquire(&mut self) {
        self.stop();
        self.token = None;
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let config = self.config.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let tx = self.inbox.sender();
        std::thread::spawn(move || {
            let result = acquire(&config, &cancel);
            tx.send((generation, config, result));
        });
    }
    pub fn poll(&mut self) -> bool {
        let results = self.inbox.drain();
        let changed = !results.is_empty();
        for (generation, config, result) in results {
            if generation != self.generation || self.cancel.is_none() {
                continue;
            }
            self.cancel = None;
            if config != self.config {
                continue;
            }
            match result {
                Ok(token) => self.token = Some((config, token)),
                Err(error) => self.error = Some(error),
            }
        }
        changed
    }
    pub fn authorize(&self, mut prepared: Prepared) -> Result<Prepared, HttpError> {
        if !self.enabled {
            return Ok(prepared);
        }
        let (config, token) =
            self.token.as_ref().filter(|(config, token)| *config == self.config && Instant::now() < token.deadline).ok_or(HttpError::OAuth(Error::Missing))?;
        let (_, resource) = validate(config).map_err(HttpError::OAuth)?;
        let request = url(&prepared.url).map_err(HttpError::OAuth)?;
        if request.origin() != resource.origin() {
            return Err(HttpError::OAuth(Error::Origin));
        }
        if prepared.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("Authorization")) {
            return Err(HttpError::OAuth(Error::Authorization));
        }
        prepared.headers.push(("Authorization".into(), format!("Bearer {}", token.value)));
        // The sender disables redirects for credentials acquired by this panel.
        prepared.oauth = true;
        Ok(prepared)
    }
    pub fn show(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(tr("OAuth 2.0 client credentials")).show(ui, |ui| {
            ui.label(tr("Acquire explicitly, then enable for one resource origin. Credentials and tokens stay in memory; no refresh or browser login."));
            let mut changed = false;
            for (label, value, secret) in [
                (tr("Token endpoint"), &mut self.config.endpoint, false),
                (tr("Client ID"), &mut self.config.client, false),
                (tr("Client secret"), &mut self.config.secret, true),
                (tr("Scope"), &mut self.config.scope, false),
                (tr("Resource origin"), &mut self.config.resource, false),
            ] {
                ui.horizontal(|ui| {
                    ui.label(label);
                    changed |= ui.add(egui::TextEdit::singleline(value).password(secret).char_limit(8192)).changed();
                });
            }
            if changed {
                self.stop();
                self.token = None;
                self.error = None;
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(self.cancel.is_none(), egui::Button::new(tr("Acquire OAuth token"))).clicked() {
                    self.acquire();
                }
                if ui.add_enabled(self.cancel.is_some(), egui::Button::new(tr("Cancel"))).clicked() {
                    self.stop();
                    self.error = Some(Error::Cancelled);
                }
                if ui.button(tr("Clear OAuth credentials")).clicked() {
                    self.stop();
                    self.token = None;
                    self.config = Config::default();
                    self.enabled = false;
                    self.error = None;
                }
            });
            ui.checkbox(&mut self.enabled, tr("Use this OAuth token for matching requests"));
            if self.token.as_ref().is_some_and(|(config, token)| *config == self.config && Instant::now() < token.deadline) {
                ui.label(tr("OAuth token ready; value hidden"));
            }
            if let Some(error) = &self.error {
                ui.colored_label(rc_kit::pal(ui).error, error.text());
            }
        });
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(endpoint: &str, resource: &str) -> Config {
        Config { endpoint: endpoint.into(), resource: resource.into(), client: "client".into(), secret: "secret".into(), scope: "read write".into() }
    }
    fn response(listener: std::net::TcpListener, body: String, status: &str) -> std::thread::JoinHandle<()> {
        use std::io::{Read, Write};
        let status = status.to_string();
        std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut buffer = [0; 4096];
            let _ = peer.read(&mut buffer);
            let _ = write!(peer, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        })
    }
    #[test]
    fn credentials_transport_rejects_plaintext_redirects_invalid_responses_and_cancels() {
        assert_eq!(encoded("client:é a+"), "client%3A%C3%A9+a%2B");
        for endpoint in
            ["http://example.test/token", "https://user:password@example.test/token", "https://example.test/token?secret=value", "file:///tmp/token"]
        {
            assert!(validate(&config(endpoint, "https://example.test")).is_err());
        }
        assert!(validate(&config("https://example.test/token", "https://example.test/api")).is_err());
        assert!(matches!(acquire(&config("http://127.0.0.1:1/token", "http://127.0.0.1:1"), &AtomicBool::new(true)), Err(Error::Cancelled)));
        for body in [
            r#"{"access_token":"bad\r\nheader","token_type":"Bearer","expires_in":30}"#.to_string(),
            r#"{"access_token":"abc","token_type":"Other","expires_in":30}"#.to_string(),
            r#"{"access_token":"abc","token_type":"Bearer"}"#.to_string(),
            "x".repeat(65537),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let server = response(listener, body, "200 OK");
            assert!(matches!(acquire(&config(&format!("{origin}/token"), &origin), &AtomicBool::new(false)), Err(Error::Response)));
            server.join().unwrap();
        }
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            let _ = peer.read(&mut buffer);
            peer.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        assert!(matches!(acquire(&config(&format!("{origin}/token"), &origin), &AtomicBool::new(false)), Err(Error::Response)));
        server.join().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut buffer = [0; 4096];
            let _ = peer.read(&mut buffer);
            let mut byte = [0];
            assert_eq!(peer.read(&mut byte).unwrap(), 0);
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let c = config(&format!("{origin}/token"), &origin);
        let worker = std::thread::spawn(move || acquire(&c, &flag));
        std::thread::sleep(Duration::from_millis(100));
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
        server.join().unwrap();
    }
    #[test]
    fn native_acquisition_and_origin_bound_use_are_explicit_on_a_real_rust_project() {
        use egui_kittest::kittest::Queryable;
        use std::io::{BufRead, BufReader, Read, Write};
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[package]\nname=\"oauth_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(project.path().join("src/main.rs"), "fn main(){}\n").unwrap();
        std::fs::write(project.path().join("build.rs"), "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(project.path());
        app.http.oauth.config = config(&format!("{origin}/token"), &origin);
        app.toggle_tool(crate::toolwindow::ToolWindow::Http);
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::Http, 600.0);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 850.0));
        h.get_by_label(tr("OAuth 2.0 client credentials")).click();
        h.run_steps(3);
        assert!(listener.accept().is_err());
        assert!(!project.path().join("build-ran").exists());
        listener.set_nonblocking(false).unwrap();
        let server = std::thread::spawn(move || {
            for index in 0..2 {
                let (peer, _) = listener.accept().unwrap();
                peer.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut reader = BufReader::new(peer);
                let mut headers = String::new();
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    headers.push_str(&line);
                    assert!(headers.len() < 8192);
                }
                assert!(length < 8192);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let mut peer = reader.into_inner();
                let reply = if index == 0 {
                    assert!(headers.starts_with("POST /token HTTP/1.1"));
                    assert!(headers.to_ascii_lowercase().contains("authorization: basic y2xpzw50onnly3jlda=="));
                    assert_eq!(String::from_utf8(body).unwrap(), "grant_type=client_credentials&scope=read+write");
                    r#"{"access_token":"local.fixture-token","token_type":"Bearer","expires_in":60}"#
                } else {
                    assert!(headers.starts_with("GET /resource HTTP/1.1"));
                    assert!(headers.to_ascii_lowercase().contains("authorization: bearer local.fixture-token"));
                    "ok"
                };
                write!(peer, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len())
                    .unwrap();
            }
        });
        h.get_by_label(tr("Acquire OAuth token")).click();
        h.run_steps(2);
        let until = Instant::now() + Duration::from_secs(10);
        while h.state().http.oauth.token.is_none() {
            h.run_steps(1);
            assert!(Instant::now() < until, "{:?}", h.state().http.oauth.error);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!project.path().join("build-ran").exists());
        h.get_by_label(tr("Use this OAuth token for matching requests")).click();
        h.run_steps(2);
        let parse = |text: &str| rc_syntax::http::parse_requests(text).remove(0);
        let foreign = super::super::prepare(&parse("GET https://example.test/resource\n")).unwrap();
        assert!(matches!(h.state().http.oauth.authorize(foreign), Err(HttpError::OAuth(Error::Origin))));
        let duplicate = super::super::prepare(&parse(&format!("GET {origin}/resource\nAuthorization: Bearer other\n"))).unwrap();
        assert!(matches!(h.state().http.oauth.authorize(duplicate), Err(HttpError::OAuth(Error::Authorization))));
        h.state_mut().http.start(&parse(&format!("GET {origin}/resource\n")), project.path().join("api.http"));
        while h.state().http.running() {
            h.run_steps(1);
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        server.join().unwrap();
        assert!(matches!(h.state().http.history[0].outcome, super::super::Outcome::Done { .. }));
        h.state_mut().http.oauth.config.scope = "changed".into();
        let prepared = super::super::prepare(&parse(&format!("GET {origin}/resource\n"))).unwrap();
        assert!(matches!(h.state().http.oauth.authorize(prepared), Err(HttpError::OAuth(Error::Missing))));
        assert!(!project.path().join("build-ran").exists());
        let output = std::process::Command::new("cargo").args(["test", "--offline", "--quiet"]).current_dir(project.path()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
}

#[cfg(test)]
mod credential_redirect_tests {
    use super::*;
    #[test]
    fn bearer_requests_do_not_follow_resource_redirects_and_expired_tokens_block() {
        use std::io::{Read, Write};
        let destination = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let location = format!("http://{}/secret", destination.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut bytes = [0; 4096];
            let count = peer.read(&mut bytes).unwrap();
            assert!(String::from_utf8_lossy(&bytes[..count]).to_ascii_lowercase().contains("authorization: bearer fixture"));
            write!(peer, "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let mut state = State::new(Waker::noop());
        state.config =
            Config { endpoint: format!("{origin}/token"), client: "client".into(), secret: "secret".into(), scope: String::new(), resource: origin.clone() };
        state.enabled = true;
        state.token = Some((state.config.clone(), Token { value: "fixture".into(), deadline: Instant::now() + Duration::from_secs(60) }));
        let request = rc_syntax::http::parse_requests(&format!("GET {origin}/resource\n")).remove(0);
        let prepared = super::super::prepare(&request).unwrap();
        let prepared = state.authorize(prepared.clone()).unwrap();
        let response = super::super::send(&prepared, Duration::from_secs(5), 4096, &AtomicBool::new(false)).unwrap();
        assert_eq!(response.status, 302);
        server.join().unwrap();
        assert!(destination.accept().is_err());
        state.token.as_mut().unwrap().1.deadline = Instant::now() - Duration::from_secs(1);
        assert!(matches!(state.authorize(prepared), Err(HttpError::OAuth(Error::Missing))));
    }
}
