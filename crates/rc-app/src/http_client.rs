//! HTTP Client (F-03): sends a request of an `.http` file and keeps the responses for the HTTP
//! Client window.
//!
//! Requests come from [`rc_syntax::http`]. Each one is sent on its own thread with a total
//! timeout and a cap on the body read; it can be cancelled at any time. Replies are untrusted:
//! the body is cut at [`MAX_BODY_BYTES`], shown as text only when it is UTF-8, and JSON is
//! re-indented without being rebuilt (key order and numbers stay as sent).
//!
//! Variables, environments, response scripts and bodies from files are F-04: a request that
//! uses them is not sent, and the window says why.

use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use rc_syntax::http::{HttpRequest, UnsupportedKind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long a request may take, from connecting to the end of the body.
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// The most body bytes kept; the rest is not read.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Sent unless the request sets its own `User-Agent`.
pub const USER_AGENT: &str = concat!("CrabCraft/", env!("CARGO_PKG_VERSION"));
/// Responses kept in the window.
const HISTORY: usize = 30;
/// The most text shown at once in the response view.
pub const MAX_SHOWN_CHARS: usize = 512 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpError {
    /// The request uses something not supported yet (texts for the user).
    Unsupported(Vec<String>),
    /// The URL cannot be read, or is not `http`/`https`.
    BadUrl(String),
    BadMethod(String),
    BadHeader(String),
    TimedOut,
    Cancelled,
    Network(String),
}

impl HttpError {
    /// The text shown in the response view.
    pub fn text(&self) -> String {
        match self {
            HttpError::Unsupported(what) => format!("{}\n{}", tr("This request was not sent: it uses features that are not supported yet."), what.iter().map(|w| format!("• {w}")).collect::<Vec<_>>().join("\n")),
            HttpError::BadUrl(url) => trf("Not an http or https URL: {url}", &[("url", url)]),
            HttpError::BadMethod(m) => trf("Not a valid HTTP method: {method}", &[("method", m)]),
            HttpError::BadHeader(h) => trf("Not a valid header: {header}", &[("header", h)]),
            HttpError::TimedOut => trf("No complete answer within {seconds} s", &[("seconds", &TIMEOUT.as_secs())]),
            HttpError::Cancelled => tr("Cancelled").to_string(),
            HttpError::Network(e) => trf("The request failed: {error}", &[("error", e)]),
        }
    }
}

/// What the user is told about something not supported yet.
pub fn unsupported_text(kind: &UnsupportedKind) -> String {
    match kind {
        UnsupportedKind::Variable(name) => trf("Variables are not supported yet: {variable}", &[("variable", &format!("{{{{{name}}}}}"))]),
        UnsupportedKind::FileVariable(name) => trf("File variables are not supported yet: @{name}", &[("name", name)]),
        UnsupportedKind::BodyFile => tr("Bodies read from a file (< file) are not supported yet").to_string(),
        UnsupportedKind::Handler => tr("Response handler scripts are not supported yet and will not run").to_string(),
        UnsupportedKind::Redirect => tr("Saving the response to a file (>> file) is not supported yet").to_string(),
        UnsupportedKind::Method(m) => trf("{method} requests are not supported", &[("method", m)]),
        UnsupportedKind::Directive(d) => trf("The @{directive} option is not supported yet and is ignored", &[("directive", d)]),
    }
}

/// A request checked and ready to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub method: String,
    /// Absolute `http`/`https` URL.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Checks `req` and makes its URL absolute: `/path` uses the `Host` header, a URL without a
/// scheme gets `http://`.
pub fn prepare(req: &HttpRequest) -> Result<Prepared, HttpError> {
    let blockers: Vec<String> = req.blockers().map(|u| unsupported_text(&u.kind)).collect();
    if !blockers.is_empty() {
        return Err(HttpError::Unsupported(blockers));
    }
    let mut url = req.url.trim().to_string();
    if url.starts_with('/') {
        let host = req.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("host")).map(|(_, v)| v.trim()).filter(|h| !h.is_empty());
        let Some(host) = host else { return Err(HttpError::BadUrl(url)) };
        url = format!("http://{host}{url}");
    } else if !url.contains("://") {
        url = format!("http://{url}");
    }
    let parsed = reqwest::Url::parse(&url).map_err(|_| HttpError::BadUrl(req.url.clone()))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none_or(str::is_empty) {
        return Err(HttpError::BadUrl(req.url.clone()));
    }
    reqwest::Method::from_bytes(req.method.as_bytes()).map_err(|_| HttpError::BadMethod(req.method.clone()))?;
    for (k, v) in &req.headers {
        let ok = reqwest::header::HeaderName::from_bytes(k.as_bytes()).is_ok() && reqwest::header::HeaderValue::from_str(v).is_ok();
        if !ok {
            return Err(HttpError::BadHeader(format!("{k}: {v}")));
        }
    }
    Ok(Prepared { method: req.method.clone(), url: parsed.to_string(), headers: req.headers.clone(), body: req.body.clone() })
}

/// A response, as far as it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    /// `OK`, `Not Found`… (from the status code).
    pub reason: String,
    pub version: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The body was longer than [`MAX_BODY_BYTES`] and was cut.
    pub truncated: bool,
    /// Until the headers arrived.
    pub to_headers: Duration,
    /// Until the end of the body.
    pub elapsed: Duration,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Sends `req` and reads the answer. Blocks until done, `timeout` passes or `cancel` is set;
/// run it on a background thread. At most `max` body bytes are kept.
pub fn send(req: &Prepared, timeout: Duration, max: usize, cancel: &AtomicBool) -> Result<HttpResponse, HttpError> {
    let net = |e: reqwest::Error| if e.is_timeout() { HttpError::TimedOut } else { HttpError::Network(error_chain(&e)) };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| HttpError::Network(e.to_string()))?;
    let work = async {
        let url = reqwest::Url::parse(&req.url).map_err(|_| HttpError::BadUrl(req.url.clone()))?;
        let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        let mut builder = reqwest::Client::builder().user_agent(USER_AGENT).connect_timeout(timeout).timeout(timeout);
        if loopback {
            builder = builder.no_proxy();
        }
        let http = builder.build().map_err(net)?;
        let method = reqwest::Method::from_bytes(req.method.as_bytes()).map_err(|_| HttpError::BadMethod(req.method.clone()))?;
        let mut call = http.request(method, url);
        for (k, v) in &req.headers {
            call = call.header(k.as_str(), v.as_str());
        }
        if !req.body.is_empty() {
            call = call.body(req.body.clone());
        }
        let started = Instant::now();
        let mut resp = call.send().await.map_err(net)?;
        let to_headers = started.elapsed();
        let status = resp.status();
        let version = format!("{:?}", resp.version());
        let headers = resp.headers().iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
        let mut body = Vec::new();
        let mut truncated = false;
        while let Some(chunk) = resp.chunk().await.map_err(net)? {
            let room = max.saturating_sub(body.len());
            if chunk.len() > room {
                body.extend_from_slice(&chunk[..room]);
                truncated = true;
                break;
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse { status: status.as_u16(), reason: status.canonical_reason().unwrap_or_default().to_string(), version, headers, body, truncated, to_headers, elapsed: started.elapsed() })
    };
    runtime.block_on(async {
        tokio::select! {
            r = work => r,
            _ = async {
                while !cancel.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            } => Err(HttpError::Cancelled),
        }
    })
}

/// `e` and its causes ("error sending request: connection refused").
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = s.source();
    }
    out
}

/// `text` re-indented with two spaces when it is valid JSON. Strings, numbers and the order of
/// keys are kept exactly as sent.
pub fn pretty_json(text: &str) -> Option<String> {
    let t = text.trim();
    if !(t.starts_with('{') || t.starts_with('[')) {
        return None;
    }
    serde_json::from_str::<serde::de::IgnoredAny>(t).ok()?;
    let mut out = String::with_capacity(t.len() + t.len() / 4);
    let mut depth = 0usize;
    let mut chars = t.chars().peekable();
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        for _ in 0..depth {
            out.push_str("  ");
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push('"');
                while let Some(s) = chars.next() {
                    out.push(s);
                    match s {
                        '\\' => {
                            if let Some(e) = chars.next() {
                                out.push(e);
                            }
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '{' | '[' => {
                out.push(c);
                // Skip the whitespace: `{}` and `[]` stay on one line.
                while chars.next_if(|c| c.is_whitespace()).is_some() {}
                if chars.peek().is_some_and(|&n| n == '}' || n == ']') {
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(',');
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    Some(out)
}

/// How a response body is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyView {
    Empty,
    /// At most [`MAX_SHOWN_CHARS`]; `cut` when there was more.
    Text { text: String, json: bool, cut: bool },
    /// Not UTF-8: only its size is shown.
    Binary(usize),
}

pub fn body_view(resp: &HttpResponse) -> BodyView {
    if resp.body.is_empty() {
        return BodyView::Empty;
    }
    // A body cut in the middle of a character is still text.
    let text = match std::str::from_utf8(&resp.body) {
        Ok(t) => t,
        Err(e) if resp.truncated && e.error_len().is_none() => std::str::from_utf8(&resp.body[..e.valid_up_to()]).unwrap_or_default(),
        Err(_) => return BodyView::Binary(resp.body.len()),
    };
    let declared = resp.header("content-type").is_some_and(|c| c.to_ascii_lowercase().contains("json"));
    let (text, json) = match (!resp.truncated).then(|| pretty_json(text)).flatten() {
        Some(pretty) => (pretty, true),
        None => (text.to_string(), declared),
    };
    match text.char_indices().nth(MAX_SHOWN_CHARS) {
        Some((at, _)) => BodyView::Text { text: text[..at].to_string(), json, cut: true },
        None => BodyView::Text { text, json, cut: false },
    }
}

/// One request sent from the editor and what came back.
#[derive(Debug)]
pub struct Exchange {
    pub id: u64,
    pub title: String,
    pub method: String,
    pub url: String,
    /// The `.http` file and the request line.
    pub file: PathBuf,
    pub line: usize,
    /// What was not sent or done (shown above the response).
    pub notes: Vec<String>,
    pub outcome: Outcome,
    pub started: Instant,
}

#[derive(Debug)]
pub enum Outcome {
    Running(Arc<AtomicBool>),
    Done { response: HttpResponse, body: BodyView },
    Failed(HttpError),
}

/// The requests sent and their responses, newest first.
pub struct HttpClient {
    inbox: Inbox<(u64, Result<HttpResponse, HttpError>)>,
    pub history: Vec<Exchange>,
    /// Index into `history` shown in the window.
    pub selected: usize,
    /// Headers instead of the body.
    pub show_headers: bool,
    next_id: u64,
}

impl HttpClient {
    pub fn new(waker: Waker) -> Self {
        HttpClient { inbox: Inbox::new(waker), history: Vec::new(), selected: 0, show_headers: false, next_id: 1 }
    }

    pub fn running(&self) -> bool {
        self.history.iter().any(|e| matches!(e.outcome, Outcome::Running(_)))
    }

    /// Sends `req` (from `file`) in the background; a request that cannot be sent is kept with
    /// its reason. The new exchange is selected.
    pub fn start(&mut self, req: &HttpRequest, file: PathBuf) {
        self.start_with(req, file, TIMEOUT, MAX_BODY_BYTES);
    }

    pub fn start_with(&mut self, req: &HttpRequest, file: PathBuf, timeout: Duration, max: usize) {
        let id = self.next_id;
        self.next_id += 1;
        let notes = req.unsupported.iter().filter(|u| !u.kind.blocks_sending()).map(|u| unsupported_text(&u.kind)).collect();
        let outcome = match prepare(req) {
            Ok(prepared) => {
                let cancel = Arc::new(AtomicBool::new(false));
                let flag = cancel.clone();
                let tx = self.inbox.sender();
                std::thread::spawn(move || {
                    tx.send((id, send(&prepared, timeout, max, &flag)));
                });
                Outcome::Running(cancel)
            }
            Err(e) => Outcome::Failed(e),
        };
        let exchange = Exchange { id, title: req.title(), method: req.method.clone(), url: req.url.clone(), file, line: req.line, notes, outcome, started: Instant::now() };
        self.history.insert(0, exchange);
        // The oldest finished ones go first; running requests are kept until they end.
        while self.history.len() > HISTORY {
            match self.history.iter().rposition(|e| !matches!(e.outcome, Outcome::Running(_))) {
                Some(i) => {
                    self.history.remove(i);
                }
                None => break,
            }
        }
        self.selected = 0;
    }

    /// Stops the request of `history[i]`.
    pub fn cancel(&mut self, i: usize) {
        if let Some(Exchange { outcome: Outcome::Running(flag), .. }) = self.history.get(i) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn cancel_all(&mut self) {
        for i in 0..self.history.len() {
            self.cancel(i);
        }
    }

    /// Takes the finished requests; true when something changed.
    pub fn poll(&mut self) -> bool {
        let done = self.inbox.drain();
        let changed = !done.is_empty();
        for (id, result) in done {
            let Some(e) = self.history.iter_mut().find(|e| e.id == id) else { continue };
            e.outcome = match result {
                Ok(response) => {
                    let body = body_view(&response);
                    Outcome::Done { response, body }
                }
                Err(err) => Outcome::Failed(err),
            };
        }
        changed
    }
}

/// `n` bytes as "512 B", "3.4 KB", "1.2 MB".
pub fn size_text(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_syntax::http::parse_requests;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    fn one(text: &str) -> HttpRequest {
        parse_requests(text).into_iter().next().unwrap()
    }

    #[test]
    fn urls_become_absolute_and_unsupported_requests_are_refused() {
        assert_eq!(prepare(&one("GET localhost:8080/a?b=1\n")).unwrap().url, "http://localhost:8080/a?b=1");
        assert_eq!(prepare(&one("GET /items\nHost: example.test:81\n")).unwrap().url, "http://example.test:81/items");
        assert_eq!(prepare(&one("GET /items\n")), Err(HttpError::BadUrl("/items".into())));
        assert_eq!(prepare(&one("GET file:///etc/passwd\n")), Err(HttpError::BadUrl("file:///etc/passwd".into())));
        assert!(matches!(prepare(&one("GET http://x/\nBad Header: v\n")), Err(HttpError::BadHeader(_))));
        assert!(matches!(prepare(&one("GET http://x/\nX: a\u{7f}b\n")), Err(HttpError::BadHeader(_))));
        let Err(HttpError::Unsupported(why)) = prepare(&one("GET http://{{host}}/\n")) else { panic!() };
        assert_eq!(why.len(), 1);
        assert!(why[0].contains("{{host}}"), "{why:?}");
        assert!(HttpError::Unsupported(why).text().contains("not supported yet"));
        // A response handler does not stop the request, it is noted instead.
        assert!(prepare(&one("GET http://x/\n\n> {% client.log(1) %}\n")).is_ok());
    }

    #[test]
    fn json_is_reindented_without_reordering() {
        let pretty = pretty_json(r#"{"z":1,"a":[1,2.50,{"s":"x, \"y\": {z}"}],"e":{},"l":[ ]}"#).unwrap();
        assert_eq!(pretty, "{\n  \"z\": 1,\n  \"a\": [\n    1,\n    2.50,\n    {\n      \"s\": \"x, \\\"y\\\": {z}\"\n    }\n  ],\n  \"e\": {},\n  \"l\": []\n}");
        assert_eq!(pretty_json("[]").as_deref(), Some("[]"));
        for bad in ["", "plain", "{", "{\"a\":}", "[1,]", "\"str\"", "12"] {
            assert_eq!(pretty_json(bad), None, "{bad}");
        }
    }

    #[test]
    fn bodies_are_text_json_or_binary() {
        let resp = |body: &[u8], ct: &str, truncated| HttpResponse {
            status: 200,
            reason: "OK".into(),
            version: "HTTP/1.1".into(),
            headers: vec![("content-type".into(), ct.into())],
            body: body.to_vec(),
            truncated,
            to_headers: Duration::ZERO,
            elapsed: Duration::ZERO,
        };
        assert_eq!(body_view(&resp(b"", "text/plain", false)), BodyView::Empty);
        assert_eq!(body_view(&resp(b"{\"a\":1}", "application/json", false)), BodyView::Text { text: "{\n  \"a\": 1\n}".into(), json: true, cut: false });
        assert_eq!(body_view(&resp(b"hello", "text/plain", false)), BodyView::Text { text: "hello".into(), json: false, cut: false });
        assert_eq!(body_view(&resp(&[0xff, 0xfe, 0x00], "image/png", false)), BodyView::Binary(3));
        // Cut inside a two-byte character: the whole characters are text.
        assert_eq!(body_view(&resp("aé".as_bytes()[..2].as_ref(), "text/plain", true)), BodyView::Text { text: "a".into(), json: false, cut: false });
        assert_eq!(size_text(10), "10 B");
        assert_eq!(size_text(2048), "2.0 KB");
    }

    /// A local HTTP server for one connection: answers `reply` (whole HTTP response) and hands
    /// back the request it read.
    fn server(reply: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                head.push_str(&line);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let mut stream = reader.into_inner();
            let _ = stream.write_all(&reply);
            let _ = stream.flush();
            format!("{head}\r\n{}", String::from_utf8_lossy(&body))
        });
        (base, handle)
    }

    fn wait(client: &mut HttpClient) {
        let until = Instant::now() + Duration::from_secs(20);
        while client.running() && Instant::now() < until {
            client.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_request_reaches_a_local_server_and_its_json_answer_is_shown() {
        let body = r#"{"id":7,"name":"Ferris"}"#;
        let (base, server) = server(format!("HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nX-Trace: abc\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes());
        let file = format!("### Create\nPOST {base}/users?notify=1\nContent-Type: application/json\nX-Client: test\n\n{{\"name\": \"Ferris\"}}\n");
        let mut client = HttpClient::new(Waker::noop());
        client.start(&one(&file), PathBuf::from("api.http"));
        wait(&mut client);
        let seen = server.join().unwrap();
        assert!(seen.starts_with("POST /users?notify=1 HTTP/1.1\r\n"), "{seen}");
        assert!(seen.contains("x-client: test\r\n") || seen.contains("X-Client: test\r\n"), "{seen}");
        assert!(seen.to_ascii_lowercase().contains(&format!("user-agent: {}", USER_AGENT.to_ascii_lowercase())), "{seen}");
        assert!(seen.ends_with("\r\n\r\n{\"name\": \"Ferris\"}"), "{seen}");
        let e = &client.history[0];
        assert_eq!((e.title.as_str(), e.method.as_str(), e.line), ("Create", "POST", 1));
        let Outcome::Done { response, body } = &e.outcome else { panic!("{:?}", e.outcome) };
        assert_eq!((response.status, response.reason.as_str(), response.version.as_str()), (201, "Created", "HTTP/1.1"));
        assert_eq!(response.header("x-trace"), Some("abc"));
        assert!(!response.truncated);
        assert_eq!(body, &BodyView::Text { text: "{\n  \"id\": 7,\n  \"name\": \"Ferris\"\n}".into(), json: true, cut: false });
    }

    #[test]
    fn large_bodies_are_cut_and_slow_servers_time_out_or_are_cancelled() {
        let big = "x".repeat(10_000);
        let (base, server) = server(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{big}", big.len()).into_bytes());
        let mut client = HttpClient::new(Waker::noop());
        client.start_with(&one(&format!("GET {base}/big\n")), PathBuf::new(), Duration::from_secs(10), 1000);
        wait(&mut client);
        server.join().unwrap();
        let Outcome::Done { response, .. } = &client.history[0].outcome else { panic!("{:?}", client.history[0].outcome) };
        assert_eq!((response.body.len(), response.truncated), (1000, true));

        // A server that never answers: the timeout ends the request.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let prepared = prepare(&one(&format!("GET {base}/slow\n"))).unwrap();
        let started = Instant::now();
        assert_eq!(send(&prepared, Duration::from_millis(300), 1000, &AtomicBool::new(false)), Err(HttpError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(5));

        // Cancel stops it long before its timeout.
        client.start_with(&one(&format!("GET {base}/slow\n")), PathBuf::new(), Duration::from_secs(60), 1000);
        assert!(client.running());
        let started = Instant::now();
        client.cancel(0);
        wait(&mut client);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(matches!(client.history[0].outcome, Outcome::Failed(HttpError::Cancelled)), "{:?}", client.history[0].outcome);
        drop(listener);

        // Nothing listens on a closed port.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let prepared = prepare(&one(&format!("GET http://127.0.0.1:{port}/\n"))).unwrap();
        assert!(matches!(send(&prepared, Duration::from_secs(5), 1000, &AtomicBool::new(false)), Err(HttpError::Network(_))));
    }

    #[test]
    fn requests_with_variables_are_not_sent() {
        let mut client = HttpClient::new(Waker::noop());
        client.start(&one("GET http://127.0.0.1:9/{{path}}\n\n> {% client.test() %}\n"), PathBuf::new());
        assert!(!client.running());
        let e = &client.history[0];
        assert!(matches!(&e.outcome, Outcome::Failed(HttpError::Unsupported(w)) if w.len() == 1));
        assert_eq!(e.notes.len(), 1, "the handler is noted: {:?}", e.notes);
    }
}
