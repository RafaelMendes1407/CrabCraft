//! Quick fixes that turn on a Cargo feature (C-10).
//!
//! rustc says which feature hides an item the code uses: an unresolved name or import gets a note
//! "found an item that was configured out" whose span label reads "the item is gated behind the
//! `x` feature", in the file that declares it. [`feature_gates`] reads that from the rendered
//! message (rust-analyzer passes `cargo check`'s rendering along in the diagnostic's `data`), and
//! [`add_dependency_feature`] / [`add_default_feature`] compute the `Cargo.toml` edit that turns
//! the feature on.
//!
//! Everything here reads untrusted text: bad input gives no gate or no edit, never a panic.

use crate::manifest_complete::split_key;
use crate::versions::ManifestDep;
use std::path::{Component, Path, PathBuf};

/// A feature that hides an item, and the file where that item is declared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureGate {
    pub feature: String,
    /// As rustc printed it: relative to the workspace root for the workspace's own files.
    pub file: PathBuf,
}

/// An edit of a text: replace the chars `start..end` with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

const GATED: &str = "the item is gated behind the `";

fn valid_feature(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'))
}

/// The location line of a rendered message (`--> file:1:2` or `::: file:1:2`): the file.
fn location(line: &str) -> Option<PathBuf> {
    let t = line.trim_start();
    let rest = t.strip_prefix("--> ").or_else(|| t.strip_prefix("::: "))?.trim_end();
    let mut parts = rest.rsplitn(3, ':');
    let col = parts.next()?;
    let row = parts.next()?;
    let file = parts.next()?;
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    (digits(col) && digits(row) && !file.is_empty()).then(|| PathBuf::from(file))
}

/// The features rustc names as hiding the items a message is about, each with the file of the
/// `cfg` that does it. Without such a note, nothing.
pub fn feature_gates(rendered: &str) -> Vec<FeatureGate> {
    let mut out: Vec<FeatureGate> = Vec::new();
    let mut file: Option<PathBuf> = None;
    for line in rendered.lines().take(10_000) {
        if let Some(f) = location(line) {
            file = Some(f);
            continue;
        }
        let Some(at) = line.find(GATED) else { continue };
        let rest = &line[at + GATED.len()..];
        let Some(feature) = rest.strip_suffix("` feature").or_else(|| rest.split_once("` feature").map(|(f, _)| f)) else { continue };
        let Some(file) = &file else { continue };
        let gate = FeatureGate { feature: feature.to_string(), file: file.clone() };
        if valid_feature(feature) && !out.contains(&gate) {
            out.push(gate);
        }
    }
    out
}

/// The crate a file of Cargo's registry cache belongs to:
/// `…/registry/src/<index>/tokio-1.54.1/src/fs.rs` is `tokio`.
pub fn registry_crate_of(file: &Path) -> Option<String> {
    let parts: Vec<String> = file
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str().map(str::to_string),
            _ => None,
        })
        .collect();
    let at = parts.windows(2).position(|w| w[0] == "registry" && w[1] == "src")?;
    let dir = parts.get(at + 3)?;
    // The name may hold `-`, and so may the version (`1.0.0-rc.1`): the version is what parses.
    dir.match_indices('-')
        .map(|(i, _)| (&dir[..i], &dir[i + 1..]))
        .find(|(name, version)| crate::valid_crate_name(name) && semver::Version::parse(version).is_ok())
        .map(|(name, _)| name.to_string())
}

/// Char offsets of the start of each `\n`-separated line, and the lines' chars.
struct Text {
    chars: Vec<char>,
    starts: Vec<usize>,
}

impl Text {
    fn new(text: &str) -> Text {
        let chars: Vec<char> = text.chars().collect();
        let mut starts = vec![0];
        starts.extend(chars.iter().enumerate().filter(|(_, c)| **c == '\n').map(|(i, _)| i + 1));
        Text { chars, starts }
    }

    fn lines(&self) -> usize {
        self.starts.len()
    }

    /// The chars of line `n`, without its `\n` (nor a `\r` before it).
    fn line(&self, n: usize) -> (usize, usize) {
        let start = self.starts[n];
        let mut end = self.starts.get(n + 1).map_or(self.chars.len(), |s| s - 1);
        if end > start && self.chars[end - 1] == '\r' {
            end -= 1;
        }
        (start, end)
    }

    fn line_text(&self, n: usize) -> String {
        let (s, e) = self.line(n);
        self.chars[s..e].iter().collect()
    }

    /// Whether line `n` is a table header (`[x]` or `[[x]]`).
    fn is_header(&self, n: usize) -> bool {
        self.line_text(n).trim_start().starts_with('[')
    }

    /// The line after the last line of the table that line `n` is in (or heads).
    fn table_end(&self, n: usize) -> usize {
        (n + 1..self.lines()).find(|&l| self.is_header(l)).unwrap_or(self.lines())
    }
}

/// Skips a string starting at `i` (a quote); returns the index after it.
fn skip_string(c: &[char], i: usize) -> usize {
    let q = c[i];
    let triple = c.get(i + 1) == Some(&q) && c.get(i + 2) == Some(&q);
    let mut j = if triple { i + 3 } else { i + 1 };
    while j < c.len() {
        if q == '"' && c[j] == '\\' {
            j += 2;
            continue;
        }
        if c[j] == q && (!triple || (c.get(j + 1) == Some(&q) && c.get(j + 2) == Some(&q))) {
            return if triple { j + 3 } else { j + 1 };
        }
        if !triple && c[j] == '\n' {
            return j;
        }
        j += 1;
    }
    c.len()
}

/// The index of the `]` or `}` that closes the bracket at `open`, skipping strings and comments.
fn closing(c: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < c.len() {
        match c[i] {
            '"' | '\'' => {
                i = skip_string(c, i);
                continue;
            }
            '#' => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '[' | '{' => depth += 1,
            ']' | '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Adds `"feature"` to the array opening at `open`; `None` when it is already there.
fn append_to_array(c: &[char], open: usize, feature: &str) -> Option<TextEdit> {
    let close = closing(c, open)?;
    let mut last = None;
    let mut i = open + 1;
    while i < close {
        match c[i] {
            '"' | '\'' => {
                let end = skip_string(c, i);
                let value: String = c[(i + 1).min(end)..end.saturating_sub(1).max(i + 1)].iter().collect();
                if value == feature {
                    return None;
                }
                last = Some(end - 1);
                i = end;
                continue;
            }
            '#' => {
                while i < close && c[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            ch if !ch.is_whitespace() => last = Some(i),
            _ => {}
        }
        i += 1;
    }
    let quoted = format!("\"{feature}\"");
    Some(match last {
        None => TextEdit { start: open + 1, end: open + 1, text: quoted },
        Some(l) if c[l] == ',' => TextEdit { start: l + 1, end: l + 1, text: format!(" {quoted},") },
        Some(l) => TextEdit { start: l + 1, end: l + 1, text: format!(", {quoted}") },
    })
}

/// Skips spaces and tabs from `i`.
fn skip_blank(c: &[char], mut i: usize, end: usize) -> usize {
    while i < end && (c[i] == ' ' || c[i] == '\t') {
        i += 1;
    }
    i
}

/// The key of a `key = value` statement starting at `i` (dotted parts) and the index of its value.
fn statement_at(c: &[char], i: usize, end: usize) -> Option<(Vec<String>, usize)> {
    let mut j = i;
    while j < end && c[j] != '=' {
        if c[j] == '"' || c[j] == '\'' {
            j = skip_string(c, j);
            continue;
        }
        if matches!(c[j], '\n' | '#' | '[' | '{' | ',' | '}') {
            return None;
        }
        j += 1;
    }
    if j >= end {
        return None;
    }
    let key: String = c[i..j].iter().collect();
    let parts = split_key(&key);
    parts.iter().all(|p| !p.is_empty()).then(|| (parts, skip_blank(c, j + 1, end)))
}

/// In an inline table opening at `open`: the value of its `features` key.
fn inline_features(c: &[char], open: usize, close: usize) -> Option<usize> {
    let mut i = open + 1;
    let mut expect_key = true;
    while i < close {
        match c[i] {
            '"' | '\'' if !expect_key => i = skip_string(c, i),
            '[' | '{' => i = closing(c, i)? + 1,
            ',' => {
                expect_key = true;
                i += 1;
            }
            ch if ch.is_whitespace() => i += 1,
            _ if expect_key => {
                let (key, value) = statement_at(c, i, close)?;
                if key == ["features"] && c.get(value) == Some(&'[') {
                    return Some(value);
                }
                expect_key = false;
                i = value;
            }
            _ => i += 1,
        }
    }
    None
}

/// Lines `from..to` of `t`: the value of the first statement whose key is `key`, if it is an array.
fn line_array(t: &Text, from: usize, to: usize, key: &[&str]) -> Option<usize> {
    (from..to).find_map(|n| {
        let (s, e) = t.line(n);
        let i = skip_blank(&t.chars, s, e);
        let (k, value) = statement_at(&t.chars, i, e)?;
        (k == key && t.chars.get(value) == Some(&'[')).then_some(value)
    })
}

/// The edit that adds `feature` to the `features` of the dependency `dep` (from
/// [`crate::dependency_lines`] on the same text). `None` when it is already there or the entry
/// cannot be edited safely.
pub fn add_dependency_feature(text: &str, dep: &ManifestDep, feature: &str) -> Option<TextEdit> {
    let t = Text::new(text);
    if !valid_feature(feature) || dep.line >= t.lines() {
        return None;
    }
    let c = &t.chars;
    let quoted = format!("\"{feature}\"");
    let end = t.table_end(dep.line);
    // `[dependencies.tokio]`: a `features` line of that table, or a new one after its last line.
    if t.is_header(dep.line) {
        if let Some(open) = line_array(&t, dep.line + 1, end, &["features"]) {
            return append_to_array(c, open, feature);
        }
        let last = (dep.line..end).rev().find(|&n| {
            let l = t.line_text(n);
            let l = l.trim();
            !l.is_empty() && !l.starts_with('#')
        })?;
        let at = t.line(last).1;
        // Inside an unfinished value the new line would land in it: only after whole statements.
        if last > dep.line {
            let body: String = c[t.starts[dep.line + 1]..at].iter().collect();
            toml::from_str::<toml::Table>(&body).ok()?;
        }
        return Some(TextEdit { start: at, end: at, text: format!("\nfeatures = [{quoted}]") });
    }
    let (s, e) = t.line(dep.line);
    let (key, value) = statement_at(c, skip_blank(c, s, e), c.len())?;
    if key.first() != Some(&dep.key) {
        return None;
    }
    match key.len() {
        // `tokio = "1"` or `tokio = { … }`.
        1 => match c.get(value)? {
            '"' | '\'' => {
                let close = skip_string(c, value);
                if close > e || close < value + 2 || c.get(close - 1) != c.get(value) {
                    return None;
                }
                let version: String = c[value..close].iter().collect();
                Some(TextEdit { start: value, end: close, text: format!("{{ version = {version}, features = [{quoted}] }}") })
            }
            '{' => {
                let close = closing(c, value)?;
                if let Some(open) = inline_features(c, value, close) {
                    return append_to_array(c, open, feature);
                }
                let last = (value + 1..close).rev().find(|&i| !c[i].is_whitespace());
                Some(match last {
                    None => TextEdit { start: value + 1, end: close, text: format!(" features = [{quoted}] ") },
                    Some(l) if c[l] == ',' => TextEdit { start: l + 1, end: l + 1, text: format!(" features = [{quoted}]") },
                    Some(l) => TextEdit { start: l + 1, end: l + 1, text: format!(", features = [{quoted}]") },
                })
            }
            _ => None,
        },
        // Dotted keys: `tokio.workspace = true`, then `tokio.features = [ … ]`.
        _ => {
            let table_start = (0..dep.line).rev().find(|&n| t.is_header(n)).map_or(0, |n| n + 1);
            let dotted = [dep.key.as_str(), "features"];
            if let Some(open) = line_array(&t, table_start, end, &dotted) {
                return append_to_array(c, open, feature);
            }
            // After the last single-line statement of this dependency.
            let last = (dep.line..end).rev().find(|&n| {
                let (s, e) = t.line(n);
                statement_at(c, skip_blank(c, s, e), e).is_some_and(|(k, _)| k.first() == Some(&dep.key))
            })?;
            let at = t.line(last).1;
            let mut name = dep.key.clone();
            if !crate::valid_crate_name(&name) {
                name = format!("\"{name}\"");
            }
            Some(TextEdit { start: at, end: at, text: format!("\n{name}.features = [{quoted}]") })
        }
    }
}

/// The edit that adds `feature` to `default` in the `[features]` table of a manifest (a new
/// `default` entry when there is none). `None` when it is already there or there is no
/// `[features]` table.
pub fn add_default_feature(text: &str, feature: &str) -> Option<TextEdit> {
    if !valid_feature(feature) || feature == "default" {
        return None;
    }
    let t = Text::new(text);
    let header = (0..t.lines()).find(|&n| t.line_text(n).split('#').next().is_some_and(|l| l.trim() == "[features]"))?;
    let end = t.table_end(header);
    if let Some(open) = line_array(&t, header + 1, end, &["default"]) {
        return append_to_array(&t.chars, open, feature);
    }
    let at = t.line(header).1;
    Some(TextEdit { start: at, end: at, text: format!("\ndefault = [\"{feature}\"]") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, e: &TextEdit) -> String {
        let c: Vec<char> = text.chars().collect();
        let mut out: String = c[..e.start].iter().collect();
        out.push_str(&e.text);
        out.extend(&c[e.end..]);
        out
    }

    /// Adds `feature` to the first dependency named `key` and checks the result parses.
    fn add(text: &str, key: &str, feature: &str) -> Option<String> {
        let dep = crate::dependency_lines(text).into_iter().find(|d| d.key == key).expect("the dependency");
        let out = apply(text, &add_dependency_feature(text, &dep, feature)?);
        let parsed: toml::Table = toml::from_str(&out).unwrap_or_else(|e| panic!("{out}\n{e}"));
        let again = crate::dependency_lines(&out).into_iter().find(|d| d.key == key).expect("still there");
        assert_eq!(again.req, dep.req, "{out}");
        drop(parsed);
        Some(out)
    }

    /// rustc 1.99's rendering of the two notes (`cargo check --message-format=json`).
    const IMPORT: &str = "error[E0432]: unresolved import `tokio::fs`\n   --> src/main.rs:1:12\n    |\n  1 | use tokio::fs::File;\n    |            ^^ could not find `fs` in `tokio`\n    |\nnote: found an item that was configured out\n   --> /home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.54.1/src/lib.rs:568:13\n    |\n568 |     pub mod fs;\n    |             ^^\n    |\n   ::: /home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.54.1/src/macros/cfg.rs:132:19\n    |\n132 |             #[cfg(feature = \"fs\")]\n    |                   -------------- the item is gated behind the `fs` feature\n";
    const OWN: &str = "error[E0425]: cannot find function `helper` in this scope\n --> src/main.rs:7:13\n  |\n7 |     let _ = helper();\n  |             ^^^^^^ not found in this scope\n  |\nnote: found an item that was configured out\n --> src/main.rs:4:4\n  |\n3 | #[cfg(feature = \"extra\")]\n  |       ----------------- the item is gated behind the `extra` feature\n4 | fn helper() -> u32 { 1 }\n  |    ^^^^^^\n";

    #[test]
    fn gates_come_from_the_rendered_notes() {
        let tokio = feature_gates(IMPORT);
        assert_eq!(
            tokio,
            vec![FeatureGate {
                feature: "fs".into(),
                file: "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.54.1/src/macros/cfg.rs".into()
            }]
        );
        assert_eq!(registry_crate_of(&tokio[0].file).as_deref(), Some("tokio"));
        assert_eq!(feature_gates(OWN), vec![FeatureGate { feature: "extra".into(), file: "src/main.rs".into() }]);
        // Other messages, a label without a location or a bad name: nothing.
        assert!(feature_gates("error[E0425]: cannot find value `x`\n --> src/main.rs:1:1\n").is_empty());
        assert!(feature_gates("  | the item is gated behind the `fs` feature\n").is_empty());
        assert!(feature_gates(" --> a.rs:1:1\n  | the item is gated behind the `f s` feature\n").is_empty());
        assert!(feature_gates(" --> C:\\p\\a.rs:3:4\n  | the item is gated behind the `x` feature\n")[0].file == Path::new("C:\\p\\a.rs"));
    }

    #[test]
    fn registry_paths_name_their_crate() {
        let p = |s: &str| registry_crate_of(Path::new(s));
        assert_eq!(p("/c/registry/src/index.crates.io-1/serde_json-1.0.140/src/lib.rs").as_deref(), Some("serde_json"));
        assert_eq!(p("/c/registry/src/i-1/wasm-bindgen-0.2.100-rc.1/src/lib.rs").as_deref(), Some("wasm-bindgen"));
        assert_eq!(p("/home/u/project/src/lib.rs"), None);
        assert_eq!(p("/c/registry/src/index/no_version/src/lib.rs"), None);
    }

    #[test]
    fn a_feature_is_added_to_each_form_of_dependency() {
        let deps = "[dependencies]\n";
        assert_eq!(add(&format!("{deps}tokio = \"1.54\"\n"), "tokio", "fs").unwrap(), format!("{deps}tokio = {{ version = \"1.54\", features = [\"fs\"] }}\n"));
        assert_eq!(
            add(&format!("{deps}tokio = {{ version = \"1\", default-features = false }}\n"), "tokio", "fs").unwrap(),
            format!("{deps}tokio = {{ version = \"1\", default-features = false, features = [\"fs\"] }}\n")
        );
        assert_eq!(
            add(&format!("{deps}tokio = {{ version = \"1\", features = [\"rt\"] }}\n"), "tokio", "fs").unwrap(),
            format!("{deps}tokio = {{ version = \"1\", features = [\"rt\", \"fs\"] }}\n")
        );
        assert_eq!(
            add(&format!("{deps}tokio = {{ version = \"1\", features = [] }}\n"), "tokio", "fs").unwrap(),
            format!("{deps}tokio = {{ version = \"1\", features = [\"fs\"] }}\n")
        );
        assert_eq!(
            add(&format!("{deps}tokio = {{ workspace = true }}\n"), "tokio", "fs").unwrap(),
            format!("{deps}tokio = {{ workspace = true, features = [\"fs\"] }}\n")
        );
        // Multi-line arrays keep their trailing comma and comments.
        let multi = format!("{deps}tokio = {{ version = \"1\", features = [\n    \"rt\", # the runtime\n    \"net\",\n] }}\nlog = \"0.4\"\n");
        assert_eq!(add(&multi, "tokio", "fs").unwrap(), multi.replace("\"net\",", "\"net\", \"fs\","));
        // A table of its own.
        let table = "[dependencies.tokio]\nversion = \"1\"\n\n[dev-dependencies]\n";
        assert_eq!(add(table, "tokio", "fs").unwrap(), "[dependencies.tokio]\nversion = \"1\"\nfeatures = [\"fs\"]\n\n[dev-dependencies]\n");
        let table = "[dependencies.tokio]\nfeatures = [\"rt\"]\nversion = \"1\"\n";
        assert_eq!(add(table, "tokio", "fs").unwrap(), "[dependencies.tokio]\nfeatures = [\"rt\", \"fs\"]\nversion = \"1\"\n");
        // Dotted keys.
        assert_eq!(
            add(&format!("{deps}tokio.workspace = true\nlog = \"0.4\"\n"), "tokio", "fs").unwrap(),
            format!("{deps}tokio.workspace = true\ntokio.features = [\"fs\"]\nlog = \"0.4\"\n")
        );
        assert_eq!(
            add(&format!("{deps}tokio.version = \"1\"\ntokio.features = [\"rt\"]\n"), "tokio", "fs").unwrap(),
            format!("{deps}tokio.version = \"1\"\ntokio.features = [\"rt\", \"fs\"]\n")
        );
        // Renamed: the key is what the manifest says.
        assert_eq!(
            add(&format!("{deps}rt = {{ package = \"tokio\", version = \"1\" }}\n"), "rt", "fs").unwrap(),
            format!("{deps}rt = {{ package = \"tokio\", version = \"1\", features = [\"fs\"] }}\n")
        );
        // Already there: nothing to do.
        assert_eq!(add(&format!("{deps}tokio = {{ version = \"1\", features = [\"fs\"] }}\n"), "tokio", "fs"), None);
    }

    #[test]
    fn bad_entries_are_left_alone() {
        let text = "[dependencies]\ntokio = { version = \"1\"\n";
        assert!(crate::dependency_lines(text).is_empty());
        let dep =
            ManifestDep { line: 1, key: "tokio".into(), name: "tokio".into(), req: Some("1".into()), source: crate::DepSource::CratesIo, in_workspace: false };
        assert_eq!(add_dependency_feature(text, &dep, "fs"), None);
        assert_eq!(add_dependency_feature("[dependencies]\ntokio = \"1\"\n", &dep, "not valid"), None);
        assert_eq!(add_dependency_feature("", &dep, "fs"), None);
        // Never panics on broken text.
        for text in [
            "[dependencies]\ntokio = \"",
            "[dependencies]\ntokio = {",
            "[dependencies]\ntokio = { features = [\"a\"",
            "[dependencies]\ntokio.features = [",
            "[dependencies.tokio]\nfeatures = [\n",
        ] {
            let _ = add_dependency_feature(text, &dep, "fs");
        }
    }

    #[test]
    fn a_feature_is_added_to_default() {
        let apply_default = |text: &str| add_default_feature(text, "extra").map(|e| apply(text, &e));
        assert_eq!(
            apply_default("[package]\nname = \"a\"\n\n[features]\ndefault = [\"std\"]\nstd = []\nextra = []\n").unwrap(),
            "[package]\nname = \"a\"\n\n[features]\ndefault = [\"std\", \"extra\"]\nstd = []\nextra = []\n"
        );
        assert_eq!(apply_default("[features]\nextra = []\n").unwrap(), "[features]\ndefault = [\"extra\"]\nextra = []\n");
        assert_eq!(apply_default("[features]\ndefault = [\"extra\"]\nextra = []\n"), None);
        assert_eq!(apply_default("[package]\nname = \"a\"\n"), None);
        assert_eq!(add_default_feature("[features]\ndefault = []\n", "default"), None);
    }
}
