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
const MAX_INDEX_BYTES: usize = 16 * 1024 * 1024;

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

/// The versions of a crate in its sparse index file, without the yanked ones.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Published {
    pub stable: Option<Version>,
    pub any: Option<Version>,
    /// Every version that is not yanked, newest first.
    pub versions: Vec<Version>,
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
            out.any = Some(version.clone());
        }
        out.versions.push(version);
    }
    out.versions.sort_unstable_by(|a, b| b.cmp(a));
    out.versions.dedup();
    out
}

/// The crates.io sparse index.
pub const CRATES_IO_INDEX: &str = "https://index.crates.io";

/// Asks the crates.io sparse index for the versions of `name`. Blocks up to `timeout`; run it on
/// a background thread. The request carries only the crate name. `Ok(None)`: the index has no
/// such crate.
pub fn fetch_crates_io_versions(name: &str, timeout: Duration) -> anyhow::Result<Option<Published>> {
    fetch_index_versions(CRATES_IO_INDEX, name, timeout)
}

/// [`fetch_crates_io_versions`] against the sparse index at `base` (no trailing `/`).
pub fn fetch_index_versions(base: &str, name: &str, timeout: Duration) -> anyhow::Result<Option<Published>> {
    let path = index_path(name).ok_or_else(|| anyhow::anyhow!("not a crate name"))?;
    let url = format!("{base}/{path}");
    let body = crate::crates_io::get_capped(&url, timeout, MAX_INDEX_BYTES, None)?;
    Ok(body.map(|b| parse_index(&b)))
}

/// What is wrong with a crates.io dependency (C-04).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DepProblem {
    /// No crate of that name on crates.io.
    Missing,
    /// The requirement matches no published version.
    NoMatch,
    /// The newest stable version is newer than every version the requirement accepts.
    Outdated(Version),
}

/// Checks the requirement `req` of a crate against its published versions. `None` when it is
/// fine or cannot be read (Cargo reports bad requirements itself).
pub fn check_requirement(req: &str, published: &Published) -> Option<DepProblem> {
    let parsed = VersionReq::parse(req.trim()).ok()?;
    let newest_match = published.versions.iter().find(|v| parsed.matches(v));
    let Some(newest_match) = newest_match else {
        return (!published.versions.is_empty()).then_some(DepProblem::NoMatch);
    };
    let stable = published.stable.as_ref()?;
    (stable > newest_match && !parsed.matches(stable)).then(|| DepProblem::Outdated(stable.clone()))
}

/// The requirement that replaces `old` to accept `version`: the version, after the operator of a
/// single `^`, `~` or `=` comparator.
pub fn updated_requirement(old: &str, version: &Version) -> String {
    let old = old.trim();
    let op = if old.contains(',') { "" } else { ["^", "~", "="].into_iter().find(|op| old.starts_with(op)).unwrap_or("") };
    format!("{op}{version}")
}

/// The version strings to suggest for a requirement, newest first: the newest stable version
/// with its shorter forms (`1.0.229`, `1.0`, `1`; `0.8.5`, `0.8` below 1.0), then the other
/// versions, up to `limit`. The flag marks a shorter form.
pub fn version_choices(published: &Published, limit: usize) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    if let Some(s) = &published.stable {
        out.push((s.to_string(), false));
        out.push((format!("{}.{}", s.major, s.minor), true));
        if s.major > 0 {
            out.push((s.major.to_string(), true));
        }
    }
    for v in &published.versions {
        if out.len() >= limit {
            break;
        }
        let text = v.to_string();
        if !out.iter().any(|(t, _)| *t == text) {
            out.push((text, false));
        }
    }
    out
}

/// Char ranges of a dependency in the manifest text: its key and the inside of its version
/// string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DepSpans {
    pub key: Option<(usize, usize)>,
    pub req: Option<(usize, usize)>,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Finds the key and the version string of `dep` (from [`dependency_lines`] on the same text).
pub fn dep_spans(text: &str, dep: &ManifestDep) -> DepSpans {
    let mut out = DepSpans::default();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut starts = Vec::with_capacity(lines.len());
    let mut start = 0usize;
    for line in &lines {
        starts.push(start);
        start += line.chars().count() + 1;
    }
    let Some(first) = lines.get(dep.line) else { return out };
    let chars: Vec<char> = first.chars().collect();
    let key: Vec<char> = dep.key.chars().collect();
    out.key = (0..chars.len())
        .find(|&i| chars[i..].starts_with(&key) && (i == 0 || !is_word_char(chars[i - 1])) && chars.get(i + key.len()).is_none_or(|c| !is_word_char(*c)))
        .map(|i| (starts[dep.line] + i, starts[dep.line] + i + key.len()));
    let Some(req) = &dep.req else { return out };
    let req: Vec<char> = req.chars().collect();
    for (n, line) in lines.iter().enumerate().skip(dep.line).take(64) {
        if n > dep.line && header(line).is_some() {
            break;
        }
        let chars: Vec<char> = line.chars().collect();
        for (i, &q) in chars.iter().enumerate() {
            if !(q == '"' || q == '\'') || !chars[i + 1..].starts_with(&req) || chars.get(i + 1 + req.len()) != Some(&q) {
                continue;
            }
            let before: String = chars[..i].iter().collect();
            let Some(lhs) = before.trim_end().strip_suffix('=') else { continue };
            let lhs = lhs.rsplit(['{', ',']).next().unwrap_or(lhs);
            let parts = split_key(lhs);
            if parts.last().is_some_and(|p| p == "version") || parts == [dep.key.clone()] {
                out.req = Some((starts[n] + i + 1, starts[n] + i + 1 + req.len()));
                return out;
            }
        }
    }
    out
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
        assert_eq!(p.versions, vec![v("2.0.0-beta.1"), v("1.2.0"), v("1.0.3")]);
    }

    #[test]
    fn requirements_are_checked_against_the_published_versions() {
        let p = parse_index(concat!(
            "{\"vers\":\"1.0.3\",\"yanked\":false}\n",
            "{\"vers\":\"1.2.0\",\"yanked\":false}\n",
            "{\"vers\":\"2.0.1\",\"yanked\":false}\n",
            "{\"vers\":\"3.0.0\",\"yanked\":true}\n",
            "{\"vers\":\"4.0.0-rc.1\",\"yanked\":false}\n",
        ));
        assert_eq!(check_requirement("2", &p), None);
        assert_eq!(check_requirement(">=1, <3", &p), None);
        assert_eq!(check_requirement("1", &p), Some(DepProblem::Outdated(v("2.0.1"))));
        assert_eq!(check_requirement("1.2", &p), Some(DepProblem::Outdated(v("2.0.1"))));
        assert_eq!(check_requirement("3", &p), Some(DepProblem::NoMatch), "only a yanked one");
        assert_eq!(check_requirement("1.5", &p), Some(DepProblem::NoMatch));
        assert_eq!(check_requirement("4.0.0-rc.1", &p), None, "ahead of the newest stable");
        assert_eq!(check_requirement("*", &p), None);
        assert_eq!(check_requirement("not a req", &p), None);
        assert_eq!(check_requirement("1", &Published::default()), None);
        assert_eq!(updated_requirement("1", &v("2.0.1")), "2.0.1");
        assert_eq!(updated_requirement(" ~1.2", &v("2.0.1")), "~2.0.1");
        assert_eq!(updated_requirement(">=1, <2", &v("2.0.1")), "2.0.1");
    }

    #[test]
    fn version_choices_start_with_the_newest_stable_and_its_short_forms() {
        let p = parse_index("{\"vers\":\"1.0.228\"}\n{\"vers\":\"1.0.229\"}\n{\"vers\":\"1.1.0-rc.1\"}\n{\"vers\":\"1.0.230\",\"yanked\":true}");
        let got: Vec<(String, bool)> = version_choices(&p, 50);
        let labels: Vec<&str> = got.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(labels, vec!["1.0.229", "1.0", "1", "1.1.0-rc.1", "1.0.228"]);
        assert_eq!(got.iter().filter(|(_, short)| *short).count(), 2);
        let zero = parse_index("{\"vers\":\"0.8.5\"}");
        assert_eq!(version_choices(&zero, 50).into_iter().map(|(t, _)| t).collect::<Vec<_>>(), vec!["0.8.5", "0.8"]);
        assert_eq!(version_choices(&p, 2).len(), 3, "the short forms always come");
    }

    #[test]
    fn spans_find_the_key_and_the_version_string() {
        let text = "[dependencies]\nserde = \"1\"\ntokio = { features = [\"rt\"], version = '1.40' }\nx.version = \"0.1\"\n\n[dependencies.regex]\nfeatures = [\"1.10\"]\nversion = \"1.10\"\n";
        let deps = dependency_lines(text);
        let take = |r: Option<(usize, usize)>| r.map(|(a, b)| text.chars().skip(a).take(b - a).collect::<String>());
        let span = |name: &str| {
            let d = deps.iter().find(|d| d.key == name).unwrap();
            let s = dep_spans(text, d);
            (take(s.key), take(s.req))
        };
        assert_eq!(span("serde"), (Some("serde".into()), Some("1".into())));
        assert_eq!(span("tokio"), (Some("tokio".into()), Some("1.40".into())));
        assert_eq!(span("x"), (Some("x".into()), Some("0.1".into())));
        assert_eq!(span("regex"), (Some("regex".into()), Some("1.10".into())));
        let d = deps.iter().find(|d| d.key == "regex").unwrap();
        let req = dep_spans(text, d).req.unwrap();
        assert_eq!(text.chars().take(req.0).filter(|&c| c == '\n').count(), 7, "not the feature named 1.10");
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
        let p = fetch_crates_io_versions("serde", Duration::from_secs(20)).unwrap().unwrap();
        let stable = p.stable.unwrap();
        eprintln!("serde: newest stable {stable}, newest {:?}", p.any);
        assert!(stable >= v("1.0.200"));
        let missing = fetch_crates_io_versions("crabcraft-no-such-crate-x7q", Duration::from_secs(20)).unwrap();
        assert_eq!(missing, None, "404 for a crate that does not exist");
    }
}
