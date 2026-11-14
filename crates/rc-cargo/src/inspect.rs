//! Offline inspections of a `Cargo.toml` (I-22): unknown keys with a likely meant one, a
//! dependency declared again in `[dev-dependencies]` with another requirement, a dependency
//! with no source, features that name what does not exist, an unknown edition, and workspace
//! members or path dependencies with no package.
//!
//! The manifest is untrusted text: one that is too big or does not parse gives no issues, never
//! an error or a panic. Ranges are char offsets into the text. With a directory, member and path
//! dependency directories are looked at on disk, so callers run [`inspect`] off the UI thread.

use crate::manifest_complete::{DEPENDENCY_KEYS, INHERITABLE, PACKAGE_KEYS, WORKSPACE_KEYS};
use semver::{Op, VersionReq};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

/// The editions Cargo knows, oldest first.
pub const EDITIONS: &[&str] = &["2015", "2018", "2021", "2024"];
/// The edition an unknown one is changed to.
pub const LATEST_EDITION: &str = "2024";
/// Bigger manifests are not inspected.
const MAX_BYTES: usize = 2 * 1024 * 1024;

const TOP_KEYS: &[&str] = &[
    "cargo-features",
    "package",
    "project",
    "lib",
    "bin",
    "example",
    "test",
    "bench",
    "dependencies",
    "dev-dependencies",
    "dev_dependencies",
    "build-dependencies",
    "build_dependencies",
    "target",
    "features",
    "badges",
    "lints",
    "hints",
    "patch",
    "replace",
    "profile",
    "workspace",
];
/// Package keys the completion list leaves out (rare or unstable ones Cargo still reads).
const PACKAGE_EXTRA: &[&str] = &["metadata", "autolib", "default-target", "forced-target", "im-a-teapot", "metabuild"];
/// Dependency keys the completion list leaves out.
const DEPENDENCY_EXTRA: &[&str] = &["default_features", "registry-index", "public", "artifact", "lib", "target", "base"];
const DEP_TABLES: &[&str] = &["dependencies", "dev-dependencies", "dev_dependencies", "build-dependencies", "build_dependencies"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warning,
}

/// What is wrong; the UI words it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// A key Cargo does not read in `table` (`""`: the top level), with the known key it is
    /// most likely a typo of.
    UnknownKey {
        key: String,
        table: String,
        suggestion: Option<String>,
    },
    /// A `[dev-dependencies]` entry whose requirement is semver-incompatible with the
    /// `[dependencies]` one (`other`).
    DuplicateDependency {
        name: String,
        other: String,
    },
    /// A dependency with no version, path, git repository or `workspace = true`.
    NoSource {
        name: String,
    },
    /// `value` in feature `feature` is neither a feature nor an optional dependency.
    UnknownFeature {
        feature: String,
        value: String,
        suggestion: Option<String>,
    },
    /// `value` in `feature` names `dep`, which is not a dependency.
    NotADependency {
        feature: String,
        value: String,
        dep: String,
    },
    /// `value` in `feature` needs `dep` to be optional.
    NotOptional {
        feature: String,
        value: String,
        dep: String,
    },
    /// `feature` names optional dependency `dep`, whose implicit feature is gone because
    /// `dep:<dep>` is used.
    NoImplicitFeature {
        feature: String,
        dep: String,
    },
    InvalidEdition {
        value: String,
    },
    EditionNotString,
    MissingMember {
        path: String,
    },
    MissingPathDependency {
        name: String,
        path: String,
    },
}

/// What a quick fix does, for its title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FixKind {
    /// Replaces the range with the text shown.
    ChangeTo(String),
    /// Adds `optional = true` to the dependency.
    MakeOptional(String),
}

/// A quick fix: replace the chars `start..end`, which must still read `old`, with `new`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub start: usize,
    pub end: usize,
    pub old: String,
    pub new: String,
    pub kind: FixKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// Char range in the text.
    pub start: usize,
    pub end: usize,
    pub level: Level,
    pub problem: Problem,
    pub fix: Option<Fix>,
}

type Value<'i> = Spanned<DeValue<'i>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Normal,
    Dev,
    Build,
}

/// One dependency declaration.
struct Decl<'a, 'i> {
    key: &'a str,
    key_span: Range<usize>,
    kind: Kind,
    /// In a `target.<cfg>` table.
    target: bool,
    value: &'a Value<'i>,
}

impl Decl<'_, '_> {
    fn table(&self) -> Option<&DeTable<'_>> {
        table(self.value)
    }

    fn optional(&self) -> bool {
        self.table().and_then(|t| get(t, "optional")).is_some_and(|v| matches!(v.get_ref(), DeValue::Boolean(true)))
    }

    /// The requirement and the range of its string.
    fn req(&self) -> Option<(&str, Range<usize>)> {
        match self.value.get_ref() {
            DeValue::String(s) => Some((s.as_ref(), self.value.span())),
            DeValue::Table(t) => get(t, "version").and_then(|v| str_of(v).map(|s| (s, v.span()))),
            _ => None,
        }
    }

    fn package(&self) -> &str {
        self.table().and_then(|t| get(t, "package")).and_then(str_of).unwrap_or(self.key)
    }
}

fn get<'a, 'i>(t: &'a DeTable<'i>, key: &str) -> Option<&'a Value<'i>> {
    t.iter().find(|(k, _)| k.get_ref().as_ref() == key).map(|(_, v)| v)
}

fn table<'a, 'i>(v: &'a Value<'i>) -> Option<&'a DeTable<'i>> {
    match v.get_ref() {
        DeValue::Table(t) => Some(t),
        _ => None,
    }
}

fn str_of<'a>(v: &'a Value<'_>) -> Option<&'a str> {
    match v.get_ref() {
        DeValue::String(s) => Some(s.as_ref()),
        _ => None,
    }
}

fn array<'a, 'i>(v: &'a Value<'i>) -> &'a [Value<'i>] {
    match v.get_ref() {
        DeValue::Array(a) => a,
        _ => &[],
    }
}

/// The semver-compatible line (`1`, `0.7`, `0.0.3`) a requirement asks for, when it is plain
/// (`1.2`, `^1.2`, `~1.2`, `=1.2.3`); `None` for ranges, wildcards and anything that does not
/// parse.
fn compat_line(req: &str) -> Option<(u64, u64, u64)> {
    let req = VersionReq::parse(req).ok()?;
    let [c] = req.comparators.as_slice() else { return None };
    if c.major == 0 && c.minor.is_none() {
        return None;
    }
    if !matches!(c.op, Op::Caret | Op::Tilde | Op::Exact) {
        return None;
    }
    let (minor, patch) = (c.minor.unwrap_or(0), c.patch.unwrap_or(0));
    Some(match (c.major, minor) {
        (0, 0) => (0, 0, patch),
        (0, m) => (0, m, 0),
        (m, _) => (m, 0, 0),
    })
}

/// Edit distance with transpositions (optimal string alignment).
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    d[a.len()][b.len()]
}

/// The candidate `word` is most likely a typo of, if one is close enough.
pub fn closest<'c>(word: &str, candidates: impl IntoIterator<Item = &'c str>) -> Option<&'c str> {
    let norm = |s: &str| s.to_lowercase().replace('_', "-");
    let w = norm(word);
    let limit = (w.chars().count() / 3).clamp(1, 3);
    candidates.into_iter().filter(|c| *c != word).map(|c| (distance(&w, &norm(c)), c)).filter(|(d, _)| *d <= limit).min_by_key(|(d, _)| *d).map(|(_, c)| c)
}

/// `inner` written as a TOML string with the quote `raw` uses.
fn quoted(raw: &str, inner: &str) -> String {
    let q = if raw.starts_with('\'') && !inner.contains('\'') { '\'' } else { '"' };
    if q == '"' && inner.contains(['"', '\\']) {
        return format!("\"{}\"", inner.replace('\\', "\\\\").replace('"', "\\\""));
    }
    format!("{q}{inner}{q}")
}

/// `name` written as a TOML key in the style of `raw`.
fn key_text(raw: &str, name: &str) -> String {
    if raw.starts_with(['"', '\'']) { quoted(raw, name) } else { name.to_string() }
}

struct Cx<'t> {
    text: &'t str,
    out: Vec<Issue>,
}

impl Cx<'_> {
    fn chars(&self, r: &Range<usize>) -> (usize, usize) {
        let count = |b: usize| self.text.get(..b).map_or(0, |s| s.chars().count());
        (count(r.start), count(r.end))
    }

    fn raw(&self, r: &Range<usize>) -> &str {
        self.text.get(r.clone()).unwrap_or_default()
    }

    fn push(&mut self, r: &Range<usize>, level: Level, problem: Problem, fix: Option<(Range<usize>, String, FixKind)>) {
        let (start, end) = self.chars(r);
        let fix = fix.map(|(fr, new, kind)| {
            let (s, e) = self.chars(&fr);
            Fix { start: s, end: e, old: self.raw(&fr).to_string(), new, kind }
        });
        self.out.push(Issue { start, end, level, problem, fix });
    }

    /// Warns about the keys of `t` that are not `known`.
    fn keys(&mut self, t: &DeTable<'_>, path: &str, known: &[&str]) {
        for (k, _) in t.iter() {
            let key = k.get_ref().as_ref();
            if known.contains(&key) {
                continue;
            }
            // A suggestion the table already has would only make a duplicate key.
            let suggestion = closest(key, known.iter().copied().filter(|c| get(t, c).is_none())).map(str::to_string);
            let fix = suggestion.as_ref().map(|s| {
                let new = key_text(self.raw(&k.span()), s);
                (k.span(), new, FixKind::ChangeTo(s.clone()))
            });
            self.push(&k.span(), Level::Warning, Problem::UnknownKey { key: key.to_string(), table: path.to_string(), suggestion }, fix);
        }
    }

    fn edition(&mut self, t: &DeTable<'_>) {
        let Some(v) = get(t, "edition") else { return };
        match v.get_ref() {
            DeValue::String(s) if !EDITIONS.contains(&s.as_ref()) => {
                let new = quoted(self.raw(&v.span()), LATEST_EDITION);
                self.push(
                    &v.span(),
                    Level::Error,
                    Problem::InvalidEdition { value: s.to_string() },
                    Some((v.span(), new, FixKind::ChangeTo(LATEST_EDITION.to_string()))),
                );
            }
            DeValue::Integer(i) => {
                let fix = EDITIONS.contains(&i.as_str()).then(|| (v.span(), format!("\"{}\"", i.as_str()), FixKind::ChangeTo(format!("\"{}\"", i.as_str()))));
                self.push(&v.span(), Level::Error, Problem::EditionNotString, fix);
            }
            _ => {}
        }
    }

    /// Key, source and path checks of one dependency.
    fn dependency(&mut self, d: &Decl<'_, '_>, path: &str, in_workspace: bool, dir: Option<&Path>) {
        let Some(t) = d.table() else { return };
        let known: Vec<&str> =
            DEPENDENCY_KEYS.iter().map(|k| k.name).chain(DEPENDENCY_EXTRA.iter().copied()).filter(|k| !in_workspace || *k != "workspace").collect();
        self.keys(t, &format!("{path}.{}", d.key), &known);
        if !["version", "path", "git", "workspace"].iter().any(|k| get(t, k).is_some()) {
            self.push(&d.key_span, Level::Error, Problem::NoSource { name: d.key.to_string() }, None);
        }
        if let (Some(dir), Some(p), None) = (dir, get(t, "path"), get(t, "base"))
            && let Some(s) = str_of(p)
            && !dir.join(s).join("Cargo.toml").is_file()
        {
            self.push(&p.span(), Level::Error, Problem::MissingPathDependency { name: d.key.to_string(), path: s.to_string() }, None);
        }
    }

    fn members(&mut self, ws: &DeTable<'_>, dir: &Path) {
        for list in ["members", "default-members"] {
            let Some(v) = get(ws, list) else { continue };
            for m in array(v) {
                let Some(s) = str_of(m) else { continue };
                let found = match s.find(['*', '?', '[']) {
                    // A glob: the directories before the first pattern must exist.
                    Some(at) => {
                        let prefix = s[..at].rsplit_once('/').map_or("", |(p, _)| p);
                        prefix.is_empty() || dir.join(prefix).is_dir()
                    }
                    None => dir.join(s).join("Cargo.toml").is_file(),
                };
                if !found {
                    self.push(&m.span(), Level::Error, Problem::MissingMember { path: s.to_string() }, None);
                }
            }
        }
    }

    /// `optional = true` added to the one declaration of `dep`, when it is written inline.
    fn make_optional(&self, decls: &[Decl<'_, '_>], dep: &str) -> Option<(Range<usize>, String, FixKind)> {
        let mut found = decls.iter().filter(|d| d.key == dep && d.kind != Kind::Dev);
        let d = found.next()?;
        if found.next().is_some() {
            return None;
        }
        let span = d.value.span();
        let raw = self.raw(&span);
        let new = if raw.starts_with(['"', '\'']) && str_of(d.value).is_some() {
            format!("{{ version = {raw}, optional = true }}")
        } else if raw.starts_with('{') && raw.ends_with('}') && d.table().is_some_and(|t| get(t, "optional").is_none()) {
            let inner = raw[1..raw.len() - 1].trim().trim_end_matches(',').trim_end();
            if inner.is_empty() { "{ optional = true }".to_string() } else { format!("{{ {inner}, optional = true }}") }
        } else {
            return None;
        };
        Some((span, new, FixKind::MakeOptional(dep.to_string())))
    }

    fn features(&mut self, features: &DeTable<'_>, decls: &[Decl<'_, '_>]) {
        let names: BTreeSet<&str> = features.iter().map(|(k, _)| k.get_ref().as_ref()).collect();
        let any: HashSet<&str> = decls.iter().map(|d| d.key).collect();
        let non_dev: HashSet<&str> = decls.iter().filter(|d| d.kind != Kind::Dev).map(|d| d.key).collect();
        let optional: HashSet<&str> = decls.iter().filter(|d| d.kind != Kind::Dev && d.optional()).map(|d| d.key).collect();
        let values = || features.iter().flat_map(|(k, v)| array(v).iter().filter_map(move |e| str_of(e).map(|s| (k.get_ref().as_ref(), s, e.span()))));
        let explicit: HashSet<&str> = values().filter_map(|(_, s, _)| s.strip_prefix("dep:")).collect();
        let found: Vec<(&str, &str, Range<usize>)> = values().collect();
        for (feature, value, span) in found {
            let (feature_s, value_s) = (feature.to_string(), value.to_string());
            if let Some(dep) = value.strip_prefix("dep:") {
                if !non_dev.contains(dep) {
                    self.push(&span, Level::Error, Problem::NotADependency { feature: feature_s, value: value_s, dep: dep.to_string() }, None);
                } else if !optional.contains(dep) {
                    let fix = self.make_optional(decls, dep);
                    self.push(&span, Level::Error, Problem::NotOptional { feature: feature_s, value: value_s, dep: dep.to_string() }, fix);
                }
            } else if let Some((dep, _)) = value.split_once('/') {
                let (dep, weak) = match dep.strip_suffix('?') {
                    Some(d) => (d, true),
                    None => (dep, false),
                };
                if !any.contains(dep) {
                    self.push(&span, Level::Error, Problem::NotADependency { feature: feature_s, value: value_s, dep: dep.to_string() }, None);
                } else if weak && !optional.contains(dep) {
                    let fix = self.make_optional(decls, dep);
                    self.push(&span, Level::Error, Problem::NotOptional { feature: feature_s, value: value_s, dep: dep.to_string() }, fix);
                }
            } else if names.contains(value) {
                continue;
            } else if optional.contains(value) {
                if explicit.contains(value) {
                    let new = format!("dep:{value}");
                    let fix = (span.clone(), quoted(self.raw(&span), &new), FixKind::ChangeTo(new));
                    self.push(&span, Level::Error, Problem::NoImplicitFeature { feature: feature_s, dep: value_s }, Some(fix));
                }
            } else if non_dev.contains(value) {
                let fix = self.make_optional(decls, value);
                self.push(&span, Level::Error, Problem::NotOptional { feature: feature_s, value: value_s.clone(), dep: value_s }, fix);
            } else {
                let implicit = optional.iter().copied().filter(|d| !explicit.contains(d));
                let suggestion = closest(value, names.iter().copied().filter(|n| *n != feature).chain(implicit)).map(str::to_string);
                let fix = suggestion.as_ref().map(|s| (span.clone(), quoted(self.raw(&span), s), FixKind::ChangeTo(s.clone())));
                self.push(&span, Level::Error, Problem::UnknownFeature { feature: feature_s, value: value_s, suggestion }, fix);
            }
        }
    }

    fn duplicates(&mut self, decls: &[Decl<'_, '_>]) {
        let normal: HashMap<&str, &Decl<'_, '_>> = decls.iter().filter(|d| d.kind == Kind::Normal && !d.target).map(|d| (d.key, d)).collect();
        for dev in decls.iter().filter(|d| d.kind == Kind::Dev && !d.target) {
            let Some(n) = normal.get(dev.key) else { continue };
            let (Some((a, _)), Some((b, span))) = (n.req(), dev.req()) else { continue };
            if n.package() != dev.package() {
                continue;
            }
            let (Some(ra), Some(rb)) = (compat_line(a), compat_line(b)) else { continue };
            if ra != rb {
                self.push(&span, Level::Warning, Problem::DuplicateDependency { name: dev.key.to_string(), other: a.to_string() }, None);
            }
        }
    }
}

/// The dependency declarations of the dependency tables of `t` (the top level or a target).
fn collect<'a, 'i>(t: &'a DeTable<'i>, target: bool, out: &mut Vec<(Decl<'a, 'i>, String)>, prefix: &str) {
    for table_name in DEP_TABLES {
        let Some(deps) = get(t, table_name).and_then(table) else { continue };
        let kind = match *table_name {
            "dependencies" => Kind::Normal,
            "dev-dependencies" | "dev_dependencies" => Kind::Dev,
            _ => Kind::Build,
        };
        for (k, v) in deps.iter() {
            out.push((Decl { key: k.get_ref().as_ref(), key_span: k.span(), kind, target, value: v }, format!("{prefix}{table_name}")));
        }
    }
}

/// The issues of the manifest `text`. With `dir` (the manifest's directory), workspace members
/// and path dependencies are looked for on disk.
pub fn inspect(text: &str, dir: Option<&Path>) -> Vec<Issue> {
    if text.len() > MAX_BYTES {
        return Vec::new();
    }
    let Ok(doc) = DeTable::parse(text) else { return Vec::new() };
    let root = doc.get_ref();
    let mut cx = Cx { text, out: Vec::new() };
    cx.keys(root, "", TOP_KEYS);
    let unstable = get(root, "cargo-features").is_some();
    let package_keys: Vec<&str> = PACKAGE_KEYS.iter().map(|k| k.name).chain(PACKAGE_EXTRA.iter().copied()).collect();
    for name in ["package", "project"] {
        if let Some(p) = get(root, name).and_then(table) {
            if !unstable {
                cx.keys(p, name, &package_keys);
            }
            cx.edition(p);
        }
    }
    if let Some(lib) = get(root, "lib").and_then(table) {
        cx.edition(lib);
    }
    for kind in ["bin", "example", "test", "bench"] {
        for t in get(root, kind).map(array).unwrap_or_default().iter().filter_map(table) {
            cx.edition(t);
        }
    }
    let mut decls = Vec::new();
    collect(root, false, &mut decls, "");
    if let Some(targets) = get(root, "target").and_then(table) {
        for (cfg, t) in targets.iter() {
            let Some(t) = table(t) else { continue };
            cx.keys(t, &format!("target.{}", cx.raw(&cfg.span())), DEP_TABLES);
            collect(t, true, &mut decls, &format!("target.{}.", cx.raw(&cfg.span())));
        }
    }
    for (d, path) in &decls {
        cx.dependency(d, path, false, dir);
    }
    let decls: Vec<Decl<'_, '_>> = decls.into_iter().map(|(d, _)| d).collect();
    cx.duplicates(&decls);
    if let Some(f) = get(root, "features").and_then(table) {
        cx.features(f, &decls);
    }
    if let Some(ws) = get(root, "workspace").and_then(table) {
        cx.keys(ws, "workspace", &WORKSPACE_KEYS.iter().map(|k| k.name).collect::<Vec<_>>());
        if let Some(p) = get(ws, "package").and_then(table) {
            cx.keys(p, "workspace.package", &INHERITABLE.iter().copied().chain(["badges"]).collect::<Vec<_>>());
            cx.edition(p);
        }
        if let Some(deps) = get(ws, "dependencies").and_then(table) {
            for (k, v) in deps.iter() {
                let d = Decl { key: k.get_ref().as_ref(), key_span: k.span(), kind: Kind::Normal, target: false, value: v };
                cx.dependency(&d, "workspace.dependencies", true, dir);
            }
        }
        if let Some(dir) = dir {
            cx.members(ws, dir);
        }
    }
    cx.out.sort_by_key(|i| (i.start, i.end));
    cx.out
}

#[cfg(test)]
mod probe {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let n = e.file_name();
            if n == "target" || n.to_string_lossy().starts_with('.') {
                continue;
            }
            if p.is_dir() {
                if depth < 6 {
                    walk(&p, out, depth + 1);
                }
            } else if n == "Cargo.toml" {
                out.push(p);
            }
        }
    }

    #[test]
    fn probe_manifests() {
        let Ok(list) = std::env::var("PROBE") else { return };
        let mut files = Vec::new();
        for d in list.split(':') {
            walk(std::path::Path::new(d), &mut files, 0);
        }
        println!("{} manifests", files.len());
        for path in &files {
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            for i in super::inspect(&text, path.parent()) {
                let s: String = text.chars().skip(i.start).take(i.end - i.start).collect();
                println!("{}: {:?} {:?} [{s}] fix={:?}", path.display(), i.level, i.problem, i.fix.map(|f| f.new));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(text under the range, level, problem, fix replacement)` of each issue.
    fn run(text: &str, dir: Option<&Path>) -> Vec<(String, Level, Problem, Option<String>)> {
        inspect(text, dir)
            .into_iter()
            .map(|i| {
                if let Some(f) = &i.fix {
                    let old: String = text.chars().skip(f.start).take(f.end - f.start).collect();
                    assert_eq!(old, f.old, "the fix range reads its old text");
                }
                (text.chars().skip(i.start).take(i.end - i.start).collect(), i.level, i.problem, i.fix.map(|f| f.new))
            })
            .collect()
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn a_clean_manifest_has_no_issues() {
        let text = "cargo-features = []\n[package]\nname = \"démo\"\nversion.workspace = true\nedition = \"2024\"\nautolib = false\n[package.metadata.docs]\nall = true\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"], optional = true }\nlog.workspace = true\nrand07 = { package = \"rand\", version = \"0.7\" }\n[dependencies.tokio]\nversion = \"1\"\noptional = true\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n[dev-dependencies]\nserde = \"1.0.100\"\nrand = \"0.8\"\n[features]\ndefault = [\"std\"]\nstd = [\"serde\", \"tokio?/rt\", \"rand07/std\", \"libc/extra\"]\nasync = [\"dep:tokio\"]\n[lints]\nworkspace = true\n[profile.release]\nlto = true\n";
        assert_eq!(run(text, None), vec![]);
    }

    #[test]
    fn unknown_keys_suggest_the_likely_one() {
        let text = "[featurs]\na = []\n[package]\nname = \"a\"\nversoin = \"1\"\nzzz = 1\n[dependencies]\nserde = { version = \"1\", featrues = [] }\n[dependencies.\"log\"]\nversion = \"0.4\"\n\"optinal\" = true\n[target.'cfg(unix)'.dependecies]\n[workspace]\nmemebrs = []\n[workspace.package]\nname = \"x\"\n";
        let got = run(text, None);
        let pick = |key: &str| got.iter().find(|g| matches!(&g.2, Problem::UnknownKey { key: k, .. } if k == key)).cloned();
        assert_eq!(pick("featurs").map(|g| (g.0, g.1, g.3)), Some((s("featurs"), Level::Warning, Some(s("features")))));
        assert_eq!(
            pick("versoin").map(|g| (g.2, g.3)),
            Some((Problem::UnknownKey { key: s("versoin"), table: s("package"), suggestion: Some(s("version")) }, Some(s("version"))))
        );
        assert_eq!(pick("zzz").map(|g| g.3), Some(None), "nothing close");
        assert_eq!(
            pick("featrues").map(|g| g.2),
            Some(Problem::UnknownKey { key: s("featrues"), table: s("dependencies.serde"), suggestion: Some(s("features")) })
        );
        assert_eq!(pick("optinal").map(|g| (g.0, g.3)), Some((s("\"optinal\""), Some(s("\"optional\"")))), "the quotes stay");
        assert_eq!(pick("dependecies").map(|g| g.3), Some(Some(s("dependencies"))));
        assert_eq!(pick("memebrs").map(|g| g.3), Some(Some(s("members"))));
        // `name` cannot be inherited, and is not a typo of an inheritable key.
        assert_eq!(pick("name").map(|g| g.2), Some(Problem::UnknownKey { key: s("name"), table: s("workspace.package"), suggestion: None }));
        assert_eq!(got.len(), 8, "{got:?}");
    }

    #[test]
    fn a_suggestion_the_table_has_is_not_made() {
        let got = run("[package]\nname = \"a\"\nnmae = \"b\"\n", None);
        assert_eq!(got, vec![(s("nmae"), Level::Warning, Problem::UnknownKey { key: s("nmae"), table: s("package"), suggestion: None }, None)]);
    }

    #[test]
    fn dependencies_need_a_source() {
        let text = "[dependencies]\na = { features = [\"x\"] }\nb = { git = \"https://example.com/b\" }\nc = { workspace = true }\n[dependencies.d]\noptional = true\n[workspace.dependencies]\ne = { default-features = false }\n";
        let got = run(text, None);
        assert_eq!(got.iter().map(|g| (g.0.as_str(), g.1)).collect::<Vec<_>>(), vec![("a", Level::Error), ("d", Level::Error), ("e", Level::Error)]);
        assert_eq!(got[0].2, Problem::NoSource { name: s("a") });
    }

    #[test]
    fn dev_dependencies_with_an_incompatible_version_are_flagged() {
        let text = "[dependencies]\nrand = \"0.7\"\nserde = \"1\"\ntokio = { version = \"1.2\" }\nother = { package = \"x\", version = \"1\" }\n[dev-dependencies]\nrand = \"0.8\"\nserde = \"1.0.200\"\ntokio = { version = \"2\", features = [] }\nother = { package = \"y\", version = \"2\" }\n";
        let got = run(text, None);
        assert_eq!(
            got,
            vec![
                (s("\"0.8\""), Level::Warning, Problem::DuplicateDependency { name: s("rand"), other: s("0.7") }, None),
                (s("\"2\""), Level::Warning, Problem::DuplicateDependency { name: s("tokio"), other: s("1.2") }, None),
            ]
        );
    }

    #[test]
    fn features_must_name_what_exists() {
        let text = "[dependencies]\nserde = \"1\"\ntokio = { version = \"1\", optional = true }\nlog = { version = \"0.4\", optional = true }\n[dev-dependencies]\ncriterion = \"0.5\"\n[features]\ndefault = [\"stdd\", \"serde\", \"dep:serde\", \"dep:nope\", \"nope/x\", \"serde?/x\", \"tokio\", \"criterion/x\"]\nstd = []\nasync = [\"dep:tokio\", \"lgo\"]\n";
        let got = run(text, None);
        let rows: Vec<(String, Problem, Option<String>)> = got.into_iter().map(|g| (g.0, g.2, g.3)).collect();
        let optional = || Some(s("{ version = \"1\", optional = true }"));
        assert_eq!(
            rows,
            vec![
                (s("\"stdd\""), Problem::UnknownFeature { feature: s("default"), value: s("stdd"), suggestion: Some(s("std")) }, Some(s("\"std\""))),
                (s("\"serde\""), Problem::NotOptional { feature: s("default"), value: s("serde"), dep: s("serde") }, optional()),
                (s("\"dep:serde\""), Problem::NotOptional { feature: s("default"), value: s("dep:serde"), dep: s("serde") }, optional()),
                (s("\"dep:nope\""), Problem::NotADependency { feature: s("default"), value: s("dep:nope"), dep: s("nope") }, None),
                (s("\"nope/x\""), Problem::NotADependency { feature: s("default"), value: s("nope/x"), dep: s("nope") }, None),
                (s("\"serde?/x\""), Problem::NotOptional { feature: s("default"), value: s("serde?/x"), dep: s("serde") }, optional()),
                (s("\"tokio\""), Problem::NoImplicitFeature { feature: s("default"), dep: s("tokio") }, Some(s("\"dep:tokio\""))),
                (s("\"lgo\""), Problem::UnknownFeature { feature: s("async"), value: s("lgo"), suggestion: Some(s("log")) }, Some(s("\"log\""))),
            ]
        );
    }

    #[test]
    fn making_an_inline_table_optional_keeps_its_keys() {
        let text = "[dependencies]\nserde = { version = \"1\", features = [\"derive\"], }\n[features]\nx = [\"dep:serde\"]\n";
        assert_eq!(run(text, None)[0].3.as_deref(), Some("{ version = \"1\", features = [\"derive\"], optional = true }"));
        // A `[dependencies.serde]` table gets no fix.
        let text = "[dependencies.serde]\nversion = \"1\"\n[features]\nx = [\"dep:serde\"]\n";
        assert_eq!(run(text, None)[0].3, None);
    }

    #[test]
    fn editions_must_be_known_strings() {
        let text = "[package]\nedition = \"2020\"\n[lib]\nedition = 2021\n[[bin]]\nname = \"b\"\nedition = '2027'\n[workspace.package]\nedition = \"2024\"\n";
        let got = run(text, None);
        assert_eq!(
            got,
            vec![
                (s("\"2020\""), Level::Error, Problem::InvalidEdition { value: s("2020") }, Some(s("\"2024\""))),
                (s("2021"), Level::Error, Problem::EditionNotString, Some(s("\"2021\""))),
                (s("'2027'"), Level::Error, Problem::InvalidEdition { value: s("2027") }, Some(s("'2024'"))),
            ]
        );
    }

    #[test]
    fn members_and_path_dependencies_must_hold_a_package() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["crates/a", "crates/b", "tools/x"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
            std::fs::write(dir.path().join(d).join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
        }
        let text = "[workspace]\nmembers = [\"crates/*\", \"tools/x\", \"tools/y\", \"plugins/*\"]\ndefault-members = [\"crates/c\"]\nexclude = [\"gone\"]\n[workspace.dependencies]\na = { path = \"crates/a\" }\nz = { path = \"crates/z\" }\n[dependencies]\nb = { path = \"crates/b\", version = \"1\" }\nq = { path = \"../q\" }\n";
        let got: Vec<(String, Problem)> = run(text, Some(dir.path())).into_iter().map(|g| (g.0, g.2)).collect();
        assert_eq!(
            got,
            vec![
                (s("\"tools/y\""), Problem::MissingMember { path: s("tools/y") }),
                (s("\"plugins/*\""), Problem::MissingMember { path: s("plugins/*") }),
                (s("\"crates/c\""), Problem::MissingMember { path: s("crates/c") }),
                (s("\"crates/z\""), Problem::MissingPathDependency { name: s("z"), path: s("crates/z") }),
                (s("\"../q\""), Problem::MissingPathDependency { name: s("q"), path: s("../q") }),
            ]
        );
        // Without a directory, nothing is looked at on disk.
        assert_eq!(run(text, None), vec![]);
    }

    #[test]
    fn bad_input_gives_no_issues() {
        assert_eq!(run("[package\nname = ", None), vec![]);
        let odd = run(
            "[dependencies]\na = { version = 1 }\nb = 3\nc = []\n[features]\nx = 3\ny = [1, \"dep:\"]\n[workspace]\nmembers = 3\n",
            Some(Path::new("/nonexistent")),
        );
        assert_eq!(
            odd.iter().map(|g| g.2.clone()).collect::<Vec<_>>(),
            vec![Problem::NotADependency { feature: s("y"), value: s("dep:"), dep: s("") }],
            "only `dep:` names nothing"
        );
        assert_eq!(closest("", ["a"]), Some("a"));
        assert_eq!(distance("ab", "ba"), 1);
    }
}
