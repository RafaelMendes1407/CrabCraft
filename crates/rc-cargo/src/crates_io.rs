//! Crate names for `Cargo.toml` completion (C-02): a search on the crates.io API, and the names
//! already on this machine (`Cargo.lock`, the local registry cache) for when the online check is
//! off.
//!
//! The sparse index has no search, so names come from `https://crates.io/api/v1/crates`. The
//! request carries only the typed text and a User-Agent naming CrabCraft and its version, as the
//! crates.io policy asks (nothing about the user). Replies are untrusted: time- and size-capped,
//! and entries with a bad name or version are dropped.

use crate::versions::valid_crate_name;
use semver::Version;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The crates.io data policy asks for a User-Agent that says which program is asking.
pub const USER_AGENT: &str = concat!("CrabCraft/", env!("CARGO_PKG_VERSION"));
/// The crates.io web API.
pub const CRATES_IO_API: &str = "https://crates.io/api/v1";
/// The most a search reply may weigh.
const MAX_SEARCH_BYTES: usize = 2 * 1024 * 1024;
/// The most local names read.
const MAX_LOCAL_NAMES: usize = 20_000;

/// One crate found by a search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrateHit {
    pub name: String,
    /// The newest stable version (or the newest, when there is no stable one).
    pub version: Option<String>,
    pub description: Option<String>,
    pub downloads: u64,
}

/// Reads a reply of `/api/v1/crates`.
pub fn parse_search(body: &str) -> Vec<CrateHit> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return Vec::new() };
    let Some(list) = v.get("crates").and_then(serde_json::Value::as_array) else { return Vec::new() };
    let text = |c: &serde_json::Value, k: &str| c.get(k).and_then(serde_json::Value::as_str).map(str::to_string);
    list.iter()
        .take(100)
        .filter_map(|c| {
            let name = text(c, "name").filter(|n| valid_crate_name(n))?;
            let version = text(c, "max_stable_version").or_else(|| text(c, "max_version")).filter(|v| Version::parse(v).is_ok());
            let description = text(c, "description").map(|d| {
                let d = d.split_whitespace().collect::<Vec<_>>().join(" ");
                if d.chars().count() > 300 { format!("{}…", d.chars().take(300).collect::<String>()) } else { d }
            });
            let downloads = c.get("downloads").and_then(serde_json::Value::as_u64).unwrap_or(0);
            Some(CrateHit { name, version, description, downloads })
        })
        .collect()
}

/// GETs `url` with a total `timeout`, reading at most `max` bytes. `Ok(None)` when the server
/// says the file does not exist (404, 410, 451, and 403 as Cargo reads it for sparse indexes).
/// Gives up, with an error, as soon as `cancel` is set. Blocks; run it on a background thread.
pub(crate) fn get_capped(url: &str, timeout: Duration, max: usize, cancel: Option<&AtomicBool>) -> anyhow::Result<Option<String>> {
    let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let http = reqwest::Client::builder().user_agent(USER_AGENT).connect_timeout(timeout).timeout(timeout).build()?;
        anyhow::ensure!(!cancelled(), "cancelled");
        let mut resp = http.get(url).header("Accept", "application/json").send().await?;
        let status = resp.status();
        if matches!(status.as_u16(), 403 | 404 | 410 | 451) {
            return Ok(None);
        }
        anyhow::ensure!(status.is_success(), "GET {url}: {status}");
        anyhow::ensure!(resp.content_length().is_none_or(|n| n <= max as u64), "the reply of {url} is too large");
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            anyhow::ensure!(!cancelled(), "cancelled");
            anyhow::ensure!(body.len() + chunk.len() <= max, "the reply of {url} is too large");
            body.extend_from_slice(&chunk);
        }
        Ok(Some(String::from_utf8_lossy(&body).into_owned()))
    })
}

/// Searches crates.io for `query` (a crate-name prefix). Blocks up to `timeout`; run it on a
/// background thread. Gives up early, with an error, when `cancel` is set.
pub fn search_crates_io(query: &str, timeout: Duration, cancel: &AtomicBool) -> anyhow::Result<Vec<CrateHit>> {
    search_at(CRATES_IO_API, query, timeout, cancel)
}

/// [`search_crates_io`] against the API at `base` (no trailing `/`).
pub fn search_at(base: &str, query: &str, timeout: Duration, cancel: &AtomicBool) -> anyhow::Result<Vec<CrateHit>> {
    // A crate name is only ASCII letters, digits, `-` and `_`: safe in a URL as it is.
    anyhow::ensure!(valid_crate_name(query), "not a crate name");
    let url = format!("{base}/crates?q={query}&per_page=20");
    let body = get_capped(&url, timeout, MAX_SEARCH_BYTES, Some(cancel))?;
    Ok(body.map(|b| parse_search(&b)).unwrap_or_default())
}

/// The crate name of a registry file or folder, `serde_json-1.0.140(.crate)`.
fn name_of_package(file: &str) -> Option<&str> {
    let stem = file.strip_suffix(".crate").unwrap_or(file);
    stem.match_indices('-').map(|(i, _)| i).find(|&i| Version::parse(&stem[i + 1..]).is_ok()).map(|i| &stem[..i]).filter(|n| valid_crate_name(n))
}

/// The names of the crates Cargo has downloaded into `cargo_home` (its `registry/cache` and
/// `registry/src`), sorted and without repeats. Reads folder listings only.
pub fn local_crate_names(cargo_home: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for sub in ["registry/cache", "registry/src"] {
        let Ok(registries) = std::fs::read_dir(cargo_home.join(sub)) else { continue };
        for registry in registries.flatten().take(32) {
            let Ok(entries) = std::fs::read_dir(registry.path()) else { continue };
            for e in entries.flatten() {
                if names.len() >= MAX_LOCAL_NAMES {
                    break;
                }
                if let Some(n) = e.file_name().to_str().and_then(name_of_package) {
                    names.push(n.to_string());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Cargo's home: `$CARGO_HOME`, else `~/.cargo`.
pub fn cargo_home() -> Option<std::path::PathBuf> {
    std::env::var_os("CARGO_HOME").filter(|h| !h.is_empty()).map(std::path::PathBuf::from).or_else(|| crate::toolchain::home().map(|h| h.join(".cargo")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_replies_are_read_defensively() {
        let body = r#"{"crates":[
            {"name":"serde","max_version":"1.0.229","max_stable_version":"1.0.229","description":"A generic\n serialization framework","downloads":700000000},
            {"name":"serde_json","max_version":"2.0.0-rc.1","max_stable_version":null,"description":null,"downloads":"many"},
            {"name":"../evil","max_version":"1.0.0"},
            {"name":"odd","max_version":"not a version"}
        ],"meta":{"total":4}}"#;
        let hits = parse_search(body);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0], CrateHit { name: "serde".into(), version: Some("1.0.229".into()), description: Some("A generic serialization framework".into()), downloads: 700_000_000 });
        assert_eq!((hits[1].version.as_deref(), hits[1].downloads), (Some("2.0.0-rc.1"), 0));
        assert_eq!(hits[2].version, None);
        assert!(parse_search("not json").is_empty());
        assert!(parse_search("{\"crates\":3}").is_empty());
    }

    #[test]
    fn local_names_come_from_the_registry_folders() {
        assert_eq!(name_of_package("serde_json-1.0.140.crate"), Some("serde_json"));
        assert_eq!(name_of_package("foo-bar-1.0.0-alpha.1"), Some("foo-bar"));
        assert_eq!(name_of_package("README"), None);
        let home = tempfile::tempdir().unwrap();
        let cache = home.path().join("registry/cache/index.crates.io-1949cf8c6b5b557f");
        let src = home.path().join("registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.40.0");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&src).unwrap();
        for f in ["serde-1.0.1.crate", "serde-1.0.2.crate", "anyhow-1.0.0.crate", "junk.txt"] {
            std::fs::write(cache.join(f), "").unwrap();
        }
        assert_eq!(local_crate_names(home.path()), vec!["anyhow", "serde", "tokio"]);
        assert!(local_crate_names(&home.path().join("missing")).is_empty());
    }

    /// A one-thread HTTP server on localhost that answers each request with `reply(path)`:
    /// `(status, body)`. Returns its base URL.
    fn serve(reply: impl Fn(&str) -> (u16, String) + Send + 'static) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, body) = reply(&path);
                let mut stream = reader.into_inner();
                let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        base
    }

    #[test]
    fn search_and_index_requests_reach_the_server() {
        let base = serve(|path| match path {
            "/crates?q=ser&per_page=20" => (200, r#"{"crates":[{"name":"serde","max_stable_version":"1.0.1","downloads":5}]}"#.to_string()),
            "/se/rd/serde" => (200, "{\"vers\":\"1.0.0\"}\n{\"vers\":\"1.0.1\"}\n".to_string()),
            "/big/" | "/crates?q=big&per_page=20" => (200, "x".repeat(MAX_SEARCH_BYTES + 1)),
            "/crates?q=boom&per_page=20" => (500, String::new()),
            _ => (404, String::new()),
        });
        let t = Duration::from_secs(10);
        let no = AtomicBool::new(false);
        let hits = search_at(&base, "ser", t, &no).unwrap();
        assert_eq!(hits.iter().map(|h| (h.name.as_str(), h.version.as_deref())).collect::<Vec<_>>(), vec![("serde", Some("1.0.1"))]);
        assert!(search_at(&base, "big", t, &no).is_err(), "too large");
        assert!(search_at(&base, "boom", t, &no).is_err(), "server error");
        assert!(search_at(&base, "a b", t, &no).is_err(), "not a crate name");
        assert!(search_at(&base, "ser", t, &AtomicBool::new(true)).is_err(), "cancelled");
        let p = crate::versions::fetch_index_versions(&base, "serde", t).unwrap().unwrap();
        assert_eq!(p.stable.map(|v| v.to_string()).as_deref(), Some("1.0.1"));
        assert_eq!(crate::versions::fetch_index_versions(&base, "nope", t).unwrap(), None, "404 is no such crate");
        // Nothing listens there: offline is an error, not a hang.
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        assert!(search_at(&closed, "ser", Duration::from_secs(2), &no).is_err());
    }

    /// Live check against crates.io (needs the network).
    #[test]
    #[ignore = "Needs the network"]
    fn crates_io_finds_serde() {
        let hits = search_crates_io("serde", Duration::from_secs(20), &AtomicBool::new(false)).unwrap();
        eprintln!("{:?}", hits.iter().take(5).map(|h| (&h.name, &h.version, h.downloads)).collect::<Vec<_>>());
        assert!(hits.iter().any(|h| h.name == "serde"));
        assert!(search_crates_io("serde", Duration::from_secs(20), &AtomicBool::new(true)).is_err(), "cancelled");
    }
}
