//! Dependency versions of a `Cargo.toml`: the lines that declare dependencies, the version
//! `Cargo.lock` resolved for each, and the newest version on crates.io.
//!
//! Everything here reads untrusted text (the manifest, the lock file, a network reply) and gives
//! fewer results on bad input, never an error that stops the caller or a panic. The network part
//! ([`fetch_crates_io_versions`]) blocks, so callers run it on a background thread; the request
//! carries only the crate name.

use crate::manifest_complete::split_key;
use semver::{Version, VersionReq};
use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where a dependency comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DepSource {
    /// crates.io.
    CratesIo,
    /// Another registry (`registry = "name"`).
    Registry(String),
    Path(String),
    Git(String),
    /// `workspace = true`: the entry of `[workspace.dependencies]` decides.
    Workspace,
}

/// One dependency of a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestDep {
    /// 0-based line of its key (or of its `[dependencies.name]` header).
    pub line: usize,
    /// The key in the manifest (what the code calls it).
    pub key: String,
    /// The package name (`package = "…"` renames it), what crates.io and the lock know.
    pub name: String,
    /// The version requirement, as written.
    pub req: Option<String>,
    pub source: DepSource,
    /// Declared in `[workspace.dependencies]`, for the members to inherit.
    pub in_workspace: bool,
}

/// Whether `s` is a name crates.io could have: ASCII letters, digits, `-` and `_`, 1 to 64
/// characters. Only such names reach a URL.
pub fn valid_crate_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `table` (the parts of a header) holds dependencies: `[dependencies]`, the dev and
/// build ones, their `target.<cfg>` forms and `[workspace.dependencies]`.
fn is_dep_table(table: &[String]) -> bool {
    let is_deps = |s: &str| matches!(s, "dependencies" | "dev-dependencies" | "build-dependencies" | "dev_dependencies" | "build_dependencies");
    match table {
        [t] => is_deps(t),
        [w, t] => w == "workspace" && t == "dependencies",
        [target, _, t] => target == "target" && is_deps(t),
        _ => false,
    }
}

/// A dependency spec read from TOML: a version string or a table.
fn dep_from_value(line: usize, key: &str, value: &toml::Value, in_workspace: bool) -> Option<ManifestDep> {
    if !valid_crate_name(key) {
        return None;
    }
    let mut dep = ManifestDep { line, key: key.to_string(), name: key.to_string(), req: None, source: DepSource::CratesIo, in_workspace };
    match value {
        toml::Value::String(s) => dep.req = Some(s.clone()),
        toml::Value::Table(t) => {
            let text = |k: &str| t.get(k).and_then(toml::Value::as_str).map(str::to_string);
            dep.req = text("version");
            if let Some(p) = text("package") {
                if !valid_crate_name(&p) {
                    return None;
                }
                dep.name = p;
            }
            if t.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                dep.source = DepSource::Workspace;
            } else if let Some(p) = text("path") {
                dep.source = DepSource::Path(p);
            } else if let Some(g) = text("git") {
                dep.source = DepSource::Git(g);
            } else if let Some(r) = text("registry") {
                dep.source = DepSource::Registry(r);
            }
        }
        _ => return None,
    }
    Some(dep)
}

/// Merges `b` (a dotted key such as `serde.features = […]`) into `a`.
fn merge(a: &mut toml::Value, b: toml::Value) {
    match (a, b) {
        (toml::Value::Table(a), toml::Value::Table(b)) => {
            for (k, v) in b {
                match a.get_mut(&k) {
                    Some(old) => merge(old, v),
                    None => {
                        a.insert(k, v);
                    }
                }
            }
        }
        (a, b) => *a = b,
    }
}

/// The parts of a table header line (`[target.'cfg(unix)'.dependencies]`), `None` for other
/// lines and for array headers (`[[bin]]`).
fn header(line: &str) -> Option<Option<Vec<String>>> {
    let t = line.trim();
    if !t.starts_with('[') {
        return None;
    }
    if t.starts_with("[[") {
        return Some(None);
    }
    let end = t.find(']')?;
    Some(Some(split_key(&t[1..end])))
}

/// The dependencies declared in `text`, in line order. A plain scan, so it works on unsaved and
/// half-typed text: statements that do not parse are skipped.
pub fn dependency_lines(text: &str) -> Vec<ManifestDep> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<ManifestDep> = Vec::new();
    // Specs gathered per table, so that dotted keys of one dependency merge.
    let mut specs: Vec<(usize, String, toml::Value, bool)> = Vec::new();
    let mut table: Option<Vec<String>> = None;
    let mut i = 0;
    let flush = |specs: &mut Vec<(usize, String, toml::Value, bool)>, out: &mut Vec<ManifestDep>| {
        out.extend(specs.drain(..).filter_map(|(line, key, value, ws)| dep_from_value(line, &key, &value, ws)));
    };
    let in_ws = |t: &[String]| t.first().is_some_and(|w| w == "workspace");
    while i < lines.len() {
        let line = lines[i];
        if let Some(h) = header(line) {
            flush(&mut specs, &mut out);
            table = h;
            // `[dependencies.serde]`: the header line is the dependency's line.
            if let Some(t) = &table
                && t.len() >= 2
                && is_dep_table(&t[..t.len() - 1])
                && let Some(name) = t.last()
            {
                specs.push((i, name.clone(), toml::Value::Table(toml::Table::new()), in_ws(t)));
            }
            i += 1;
            continue;
        }
        let Some(t) = &table else {
            i += 1;
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || !trimmed.contains('=') {
            i += 1;
            continue;
        }
        // One statement, which may span lines (an array or an inline table).
        let mut stmt = line.to_string();
        let mut parsed = toml::from_str::<toml::Table>(&stmt).ok();
        let mut end = i;
        while parsed.is_none() && end + 1 < lines.len() && end - i < 64 && header(lines[end + 1]).is_none() {
            end += 1;
            stmt.push('\n');
            stmt.push_str(lines[end]);
            parsed = toml::from_str::<toml::Table>(&stmt).ok();
        }
        let Some(parsed) = parsed else {
            i += 1;
            continue;
        };
        let start = i;
        i = end + 1;
        if is_dep_table(t) {
            for (key, value) in parsed {
                match specs.iter_mut().find(|(_, k, _, _)| *k == key) {
                    Some((_, _, old, _)) => merge(old, value),
                    None => specs.push((start, key, value, in_ws(t))),
                }
            }
        } else if t.len() >= 2 && is_dep_table(&t[..t.len() - 1])
            && let Some((_, _, old, _)) = specs.last_mut()
        {
            merge(old, toml::Value::Table(parsed));
        }
    }
    flush(&mut specs, &mut out);
    out.sort_by_key(|d| d.line);
    out
}

/// The most a `Cargo.lock` or an index reply may weigh.
const MAX_LOCK_BYTES: u64 = 32 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;

/// A package locked in `Cargo.lock`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Locked {
    pub version: Version,
    /// `registry+…`, `sparse+…`, `git+…`; `None` for path packages.
    pub source: Option<String>,
}

impl Locked {
    pub fn is_from_crates_io(&self) -> bool {
        self.source.as_deref().is_some_and(|s| s == "registry+https://github.com/rust-lang/crates.io-index" || s == "sparse+https://index.crates.io/")
    }

    fn is_from_registry(&self) -> bool {
        self.source.as_deref().is_some_and(|s| s.starts_with("registry+") || s.starts_with("sparse+"))
    }
}

/// The packages of a `Cargo.lock`, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lockfile {
    pub packages: HashMap<String, Vec<Locked>>,
}

impl Lockfile {
    /// Parses a `Cargo.lock`; entries with a bad name or version are skipped.
    pub fn parse(text: &str) -> anyhow::Result<Lockfile> {
        #[derive(serde::Deserialize)]
        struct Raw {
            #[serde(default)]
            package: Vec<toml::Value>,
        }
        let raw: Raw = toml::from_str(text)?;
        let mut packages: HashMap<String, Vec<Locked>> = HashMap::new();
        for p in raw.package {
            let text = |k: &str| p.get(k).and_then(toml::Value::as_str);
            let (Some(name), Some(version)) = (text("name"), text("version")) else { continue };
            let Ok(version) = Version::parse(version) else { continue };
            if !valid_crate_name(name) {
                continue;
            }
            packages.entry(name.to_string()).or_default().push(Locked { version, source: text("source").map(str::to_string) });
        }
        Ok(Lockfile { packages })
    }

    /// Reads and parses the `Cargo.lock` at `path`, refusing huge files.
    pub fn read(path: &Path) -> anyhow::Result<Lockfile> {
        let mut text = String::new();
        std::fs::File::open(path)?.take(MAX_LOCK_BYTES + 1).read_to_string(&mut text)?;
        anyhow::ensure!(text.len() as u64 <= MAX_LOCK_BYTES, "Cargo.lock is too large");
        Self::parse(&text)
    }

    /// The locked registry version of `name` for the requirement `req`: of several, the newest
    /// that matches it, else the newest.
    pub fn pick(&self, name: &str, req: Option<&str>) -> Option<&Locked> {
        let all: Vec<&Locked> = self.packages.get(name)?.iter().filter(|l| l.is_from_registry()).collect();
        let req = req.and_then(|r| VersionReq::parse(r.trim()).ok());
        let matching = req.and_then(|r| all.iter().filter(|l| r.matches(&l.version)).max_by(|a, b| a.version.cmp(&b.version)).copied());
        matching.or_else(|| all.iter().max_by(|a, b| a.version.cmp(&b.version)).copied())
    }
}

/// The first `Cargo.lock` at or above the folder of `manifest`.
pub fn find_lockfile(manifest: &Path) -> Option<PathBuf> {
    manifest.parent()?.ancestors().take(16).map(|d| d.join("Cargo.lock")).find(|p| p.is_file())
}

/// The newest stable and the newest of all versions of a crate in its sparse index file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Published {
    pub stable: Option<Version>,
    pub any: Option<Version>,
}

impl Published {
    /// The newest version worth offering against `current`: pre-releases only when `current`
    /// is one. `None` when it is not newer than `current`.
    pub fn newer_than(&self, current: &Version) -> Option<&Version> {
        let best = if current.pre.is_empty() { self.stable.as_ref() } else { self.any.as_ref() };
        best.filter(|v| *v > current)
    }
}

/// The path of a crate's file in the sparse index: `1/a`, `2/ab`, `3/a/abc`, `se/rd/serde`.
pub fn index_path(name: &str) -> Option<String> {
    if !valid_crate_name(name) {
        return None;
    }
    let n = name.to_ascii_lowercase();
    Some(match n.len() {
        1 => format!("1/{n}"),
        2 => format!("2/{n}"),
        3 => format!("3/{}/{n}", &n[..1]),
        _ => format!("{}/{}/{n}", &n[..2], &n[2..4]),
    })
}

/// Reads a sparse index file (one JSON object per line, with `vers` and `yanked`): yanked
/// versions and lines that do not parse are skipped.
pub fn parse_index(body: &str) -> Published {
    let mut out = Published::default();
    for line in body.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v.get("yanked").and_then(serde_json::Value::as_bool) == Some(true) {
            continue;
        }
        let Some(Ok(version)) = v.get("vers").and_then(serde_json::Value::as_str).map(Version::parse) else { continue };
        if version.pre.is_empty() && out.stable.as_ref().is_none_or(|s| version > *s) {
            out.stable = Some(version.clone());
        }
        if out.any.as_ref().is_none_or(|s| version > *s) {
            out.any = Some(version);
        }
    }
    out
}

/// Asks the crates.io sparse index for the versions of `name`. Blocks up to `timeout`; run it on
/// a background thread. The request carries only the crate name.
pub fn fetch_crates_io_versions(name: &str, timeout: Duration) -> anyhow::Result<Published> {
    let path = index_path(name).ok_or_else(|| anyhow::anyhow!("not a crate name"))?;
    let url = format!("https://index.crates.io/{path}");
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let body = runtime.block_on(async {
        let http = reqwest::Client::builder().user_agent(concat!("CrabCraft/", env!("CARGO_PKG_VERSION"))).connect_timeout(timeout).timeout(timeout).build()?;
        let mut resp = http.get(&url).send().await?;
        let status = resp.status();
        anyhow::ensure!(status.is_success(), "GET {url}: {status}");
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            body.extend_from_slice(&chunk);
            anyhow::ensure!(body.len() as u64 <= MAX_INDEX_BYTES, "the index reply is too large");
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    })?;
    Ok(parse_index(&body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn dependency_lines_cover_every_form() {
        let text = r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
serde = "1"   # inline comment
tokio = { version = "1.40", features = ["rt"] }
local = { path = "../local" }
remote = { git = "https://example.com/r.git" }
json = { package = "serde_json", version = "1.0" }
multi = { version = "0.3",
  features = ["a"] }
anyhow.workspace = true
private = { version = "2", registry = "corp" }
"bad name" = "1"
arr = [

[dev-dependencies]
tempfile = "3"

[target.'cfg(unix)'.dependencies]
libc = "0.2"

[dependencies.regex]
version = "1.10"
default-features = false

[workspace.dependencies]
log = "0.4"

[features]
serde = ["dep:serde"]
"#;
        let deps = dependency_lines(text);
        let got: Vec<(usize, &str, &str, Option<&str>, DepSource)> = deps.iter().map(|d| (d.line, d.key.as_str(), d.name.as_str(), d.req.as_deref(), d.source.clone())).collect();
        assert_eq!(
            got,
            vec![
                (5, "serde", "serde", Some("1"), DepSource::CratesIo),
                (6, "tokio", "tokio", Some("1.40"), DepSource::CratesIo),
                (7, "local", "local", None, DepSource::Path("../local".into())),
                (8, "remote", "remote", None, DepSource::Git("https://example.com/r.git".into())),
                (9, "json", "serde_json", Some("1.0"), DepSource::CratesIo),
                (10, "multi", "multi", Some("0.3"), DepSource::CratesIo),
                (12, "anyhow", "anyhow", None, DepSource::Workspace),
                (13, "private", "private", Some("2"), DepSource::Registry("corp".into())),
                (18, "tempfile", "tempfile", Some("3"), DepSource::CratesIo),
                (21, "libc", "libc", Some("0.2"), DepSource::CratesIo),
                (23, "regex", "regex", Some("1.10"), DepSource::CratesIo),
                (28, "log", "log", Some("0.4"), DepSource::CratesIo),
            ]
        );
        assert_eq!(deps.iter().filter(|d| d.in_workspace).map(|d| d.key.as_str()).collect::<Vec<_>>(), vec!["log"]);
        assert!(dependency_lines("").is_empty());
        assert!(dependency_lines("[dependencies]\nserde = \n[dependencies\nx = {").is_empty());
    }

    #[test]
    fn the_lock_gives_the_version_that_matches_the_requirement() {
        let lock = Lockfile::parse(
            r#"version = 4

[[package]]
name = "rand"
version = "0.7.3"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "rand"
version = "0.8.5"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "demo"
version = "0.1.0"

[[package]]
name = "broken"
version = "not a version"
"#,
        )
        .unwrap();
        assert_eq!(lock.pick("rand", Some("0.7")).map(|l| l.version.clone()), Some(v("0.7.3")));
        assert_eq!(lock.pick("rand", Some("0.8")).map(|l| l.version.clone()), Some(v("0.8.5")));
        assert_eq!(lock.pick("rand", None).map(|l| l.version.clone()), Some(v("0.8.5")));
        assert_eq!(lock.pick("rand", Some("9")).map(|l| l.version.clone()), Some(v("0.8.5")), "no match: the newest");
        assert!(lock.pick("rand", Some("0.8")).unwrap().is_from_crates_io());
        // Path packages are not registry versions.
        assert!(lock.pick("demo", None).is_none());
        assert!(lock.pick("broken", None).is_none());
        assert!(Lockfile::parse("[[package]\n").is_err());
        assert!(Lockfile::parse("package = 3").is_err());
    }

    #[test]
    fn index_paths_follow_the_sparse_layout() {
        assert_eq!(index_path("a").as_deref(), Some("1/a"));
        assert_eq!(index_path("cc").as_deref(), Some("2/cc"));
        assert_eq!(index_path("syn").as_deref(), Some("3/s/syn"));
        assert_eq!(index_path("Serde").as_deref(), Some("se/rd/serde"));
        for bad in ["", "../x", "a/b", "a b", "é", &"x".repeat(65)] {
            assert_eq!(index_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_index_skips_yanked_versions_and_pre_releases() {
        let body = concat!(
            "{\"name\":\"x\",\"vers\":\"1.0.3\",\"yanked\":false}\n",
            "{\"name\":\"x\",\"vers\":\"1.2.0\",\"yanked\":false}\n",
            "{\"name\":\"x\",\"vers\":\"1.3.0\",\"yanked\":true}\n",
            "{\"name\":\"x\",\"vers\":\"2.0.0-beta.1\",\"yanked\":false}\n",
            "not json\n",
            "{\"vers\":\"bad\"}\n",
        );
        let p = parse_index(body);
        assert_eq!(p.stable, Some(v("1.2.0")));
        assert_eq!(p.any, Some(v("2.0.0-beta.1")));
        assert_eq!(p.newer_than(&v("1.0.3")), Some(&v("1.2.0")));
        assert_eq!(p.newer_than(&v("1.2.0")), None);
        assert_eq!(p.newer_than(&v("2.0.0-alpha.1")), Some(&v("2.0.0-beta.1")));
        assert_eq!(parse_index(""), Published::default());
    }

    #[test]
    fn the_lockfile_is_found_above_a_member() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("crates/a")).unwrap();
        std::fs::write(dir.path().join("Cargo.lock"), "version = 4\n").unwrap();
        assert_eq!(find_lockfile(&dir.path().join("crates/a/Cargo.toml")), Some(dir.path().join("Cargo.lock")));
        assert_eq!(Lockfile::read(&dir.path().join("Cargo.lock")).unwrap(), Lockfile::default());
    }

    /// Live check against crates.io (needs the network).
    #[test]
    #[ignore = "Needs the network"]
    fn crates_io_knows_serde() {
        let p = fetch_crates_io_versions("serde", Duration::from_secs(20)).unwrap();
        let stable = p.stable.unwrap();
        eprintln!("serde: newest stable {stable}, newest {:?}", p.any);
        assert!(stable >= v("1.0.200"));
    }
}
