//! Sharing code on the Rust Playground (N-18): the code goes to `play.rust-lang.org/meta/gist`,
//! which stores it as a public GitHub gist and answers with its id; the share link points the
//! Playground at that gist.
//!
//! Building the request and reading the reply are pure functions; [`upload_gist`] does the HTTP
//! call against any endpoint, so tests use a local server and never publish anything.

use std::time::Duration;

/// Where the Playground creates gists.
pub const GIST_ENDPOINT: &str = "https://play.rust-lang.org/meta/gist";
/// Sent with every request; no personal data.
pub const USER_AGENT: &str = "CrabCraft (https://github.com/RafaelMendes1407/CrabCraft)";
/// The most code shared at once.
pub const MAX_CODE_BYTES: usize = 512 * 1024;
/// The editions the Playground knows, oldest first.
pub const EDITIONS: [&str; 4] = ["2015", "2018", "2021", "2024"];

/// The toolchain the Playground runs the code with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Channel {
    #[default]
    Stable,
    Beta,
    Nightly,
}

impl Channel {
    pub const ALL: [Channel; 3] = [Channel::Stable, Channel::Beta, Channel::Nightly];

    /// The name in the Playground's URL (and rustup's).
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
            Channel::Nightly => "nightly",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PlaygroundError {
    /// More than [`MAX_CODE_BYTES`].
    TooLarge,
    /// The server answered with this HTTP status.
    Status(u16),
    /// The reply was larger than expected.
    ReplyTooLarge,
    /// The reply had no usable gist id.
    BadReply,
    /// No answer in time.
    TimedOut,
    /// The connection failed.
    Network(String),
}

/// The JSON body of the gist request.
pub fn gist_request_body(code: &str) -> String {
    serde_json::json!({ "code": code }).to_string()
}

/// The gist id in the Playground's reply (`{"id": "...", ...}`). Ids are checked to be short and
/// alphanumeric, since they go into a URL.
pub fn parse_gist_reply(body: &[u8]) -> Result<String, PlaygroundError> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| PlaygroundError::BadReply)?;
    let id = v.get("id").and_then(serde_json::Value::as_str).ok_or(PlaygroundError::BadReply)?;
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(PlaygroundError::BadReply);
    }
    Ok(id.to_string())
}

/// The link that opens the gist in the Playground. An unknown edition falls back to the newest.
pub fn share_url(channel: Channel, edition: &str, id: &str) -> String {
    let edition = if EDITIONS.contains(&edition) { edition } else { EDITIONS[EDITIONS.len() - 1] };
    format!("https://play.rust-lang.org/?version={}&mode=debug&edition={edition}&gist={id}", channel.as_str())
}

/// The largest reply accepted for `code`: the Playground echoes the code back, escaped.
pub fn max_reply_bytes(code: &str) -> usize {
    code.len().saturating_mul(6).saturating_add(64 * 1024)
}

/// Uploads `code` to `endpoint` and returns the gist id. Blocks up to `timeout`; run it on a
/// background thread. A loopback endpoint (tests) never goes through a proxy.
pub fn upload_gist(endpoint: &str, code: &str, timeout: Duration) -> Result<String, PlaygroundError> {
    if code.len() > MAX_CODE_BYTES {
        return Err(PlaygroundError::TooLarge);
    }
    let net = |e: reqwest::Error| if e.is_timeout() { PlaygroundError::TimedOut } else { PlaygroundError::Network(e.to_string()) };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| PlaygroundError::Network(e.to_string()))?;
    let limit = max_reply_bytes(code);
    let body = runtime.block_on(async {
        let mut builder = reqwest::Client::builder().user_agent(USER_AGENT).connect_timeout(timeout).timeout(timeout);
        if endpoint.starts_with("http://127.0.0.1") || endpoint.starts_with("http://localhost") {
            builder = builder.no_proxy();
        }
        let http = builder.build().map_err(net)?;
        let mut resp = http.post(endpoint).header("Content-Type", "application/json").body(gist_request_body(code)).send().await.map_err(net)?;
        let status = resp.status();
        if !status.is_success() {
            return Err(PlaygroundError::Status(status.as_u16()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(net)? {
            body.extend_from_slice(&chunk);
            if body.len() > limit {
                return Err(PlaygroundError::ReplyTooLarge);
            }
        }
        Ok(body)
    })?;
    parse_gist_reply(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn the_request_is_the_code_as_json() {
        let body = gist_request_body("fn main() {\n    println!(\"hi \\\" there\");\n}\n");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v, serde_json::json!({ "code": "fn main() {\n    println!(\"hi \\\" there\");\n}\n" }));
        assert_eq!(v.as_object().unwrap().len(), 1, "only the code is sent");
    }

    #[test]
    fn replies_give_a_checked_gist_id() {
        assert_eq!(parse_gist_reply(br#"{"id":"0123abcDEF","url":"https://gist.github.com/x","code":"fn main(){}"}"#), Ok("0123abcDEF".into()));
        for bad in [&b""[..], b"null", b"[]", b"{}", br#"{"id":5}"#, br#"{"id":""}"#, br#"{"id":"a/../b"}"#, br#"{"id":"a&gist=b"}"#, b"<html>"] {
            assert_eq!(parse_gist_reply(bad), Err(PlaygroundError::BadReply), "{}", String::from_utf8_lossy(bad));
        }
        let long = format!(r#"{{"id":"{}"}}"#, "a".repeat(65));
        assert_eq!(parse_gist_reply(long.as_bytes()), Err(PlaygroundError::BadReply));
    }

    #[test]
    fn the_share_link_names_channel_edition_and_gist() {
        assert_eq!(share_url(Channel::Stable, "2021", "abc"), "https://play.rust-lang.org/?version=stable&mode=debug&edition=2021&gist=abc");
        assert_eq!(share_url(Channel::Nightly, "2024", "f00"), "https://play.rust-lang.org/?version=nightly&mode=debug&edition=2024&gist=f00");
        assert_eq!(share_url(Channel::Beta, "2027&x=y", "f00"), "https://play.rust-lang.org/?version=beta&mode=debug&edition=2024&gist=f00");
    }

    /// A one-request HTTP server: returns its URL and a handle with the request it read.
    fn fake_server(status: &'static str, reply: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/meta/gist", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers, then as many body bytes as Content-Length says.
            loop {
                let n = s.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req);
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text[..end].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                    if req.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let head = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len());
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&reply);
            String::from_utf8_lossy(&req).into_owned()
        });
        (url, handle)
    }

    #[test]
    fn uploads_post_the_code_and_read_the_id() {
        let (url, server) = fake_server("200 OK", br#"{"id":"deadbeef42","url":"https://gist.github.com/deadbeef42","code":"fn main() {}"}"#.to_vec());
        assert_eq!(upload_gist(&url, "fn main() {}", Duration::from_secs(10)), Ok("deadbeef42".into()));
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /meta/gist HTTP/1.1\r\n"), "{req}");
        let lower = req.to_ascii_lowercase();
        assert!(lower.contains(&format!("user-agent: {}", USER_AGENT.to_ascii_lowercase())), "{req}");
        assert!(lower.contains("content-type: application/json"), "{req}");
        assert!(req.ends_with(r#"{"code":"fn main() {}"}"#), "{req}");
    }

    #[test]
    fn upload_failures_are_typed() {
        let (url, server) = fake_server("500 Internal Server Error", b"{}".to_vec());
        assert_eq!(upload_gist(&url, "x", Duration::from_secs(10)), Err(PlaygroundError::Status(500)));
        server.join().unwrap();
        let (url, server) = fake_server("200 OK", b"not json".to_vec());
        assert_eq!(upload_gist(&url, "x", Duration::from_secs(10)), Err(PlaygroundError::BadReply));
        server.join().unwrap();
        let (url, server) = fake_server("200 OK", vec![b' '; max_reply_bytes("x") + 10]);
        assert_eq!(upload_gist(&url, "x", Duration::from_secs(10)), Err(PlaygroundError::ReplyTooLarge));
        let _ = server.join();
        assert_eq!(upload_gist("http://127.0.0.1:9/", &"x".repeat(MAX_CODE_BYTES + 1), Duration::from_secs(1)), Err(PlaygroundError::TooLarge));
        // Nothing listens on a port that was just freed.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(matches!(upload_gist(&format!("http://127.0.0.1:{port}/"), "x", Duration::from_secs(5)), Err(PlaygroundError::Network(_) | PlaygroundError::TimedOut)));
    }

    /// Publishes a real public gist: never run by default.
    #[test]
    #[ignore = "Publishes a public gist on play.rust-lang.org"]
    fn live_upload_to_the_playground() {
        let id = upload_gist(GIST_ENDPOINT, "fn main() {\n    println!(\"Hello from CrabCraft\");\n}\n", Duration::from_secs(20)).unwrap();
        println!("{}", share_url(Channel::Stable, "2021", &id));
    }
}
