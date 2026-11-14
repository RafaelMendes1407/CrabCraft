//! Navigation between Cargo features and the `cfg` predicates that use them (N-13).
//!
//! Plain text scans, so they work on unsaved text and without rust-analyzer: the `feature = "x"`
//! predicates inside `cfg(…)`, `cfg!(…)` and `cfg_attr(…)` of Rust sources, and the feature names
//! of a `Cargo.toml` `[features]` table (the keys and the features their arrays turn on).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Larger sources are not scanned: they are generated, not written by hand.
pub const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;
/// At most this many `.rs` files are scanned in one package.
pub const MAX_FILES: usize = 20_000;

/// A `feature = "name"` inside a `cfg` predicate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CfgFeature {
    pub feature: String,
    /// Char offsets of the name, without the quotes.
    pub start: usize,
    pub end: usize,
    /// 0-based line and char column of the name.
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ident(String),
    /// Contents and the char offset, line and column where they start.
    Str(String, usize, usize, usize),
    Punct(char),
}

/// A rough Rust lexer: identifiers, string literals and punctuation; comments, char literals and
/// lifetimes are skipped. Good enough to read `cfg` predicates, never panics on any input.
fn lex(text: &str) -> Vec<Tok> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line, mut col) = (0usize, 0usize, 0usize);
    // Advances one char, keeping the line and column.
    let step = |i: &mut usize, line: &mut usize, col: &mut usize| {
        if chars.get(*i) == Some(&'\n') {
            *line += 1;
            *col = 0;
        } else {
            *col += 1;
        }
        *i += 1;
    };
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        if c.is_whitespace() {
            step(&mut i, &mut line, &mut col);
        } else if c == '/' && next == Some('/') {
            while chars.get(i).is_some_and(|c| *c != '\n') {
                step(&mut i, &mut line, &mut col);
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0usize;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    step(&mut i, &mut line, &mut col);
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth = depth.saturating_sub(1);
                    step(&mut i, &mut line, &mut col);
                    if depth == 0 {
                        step(&mut i, &mut line, &mut col);
                        break;
                    }
                }
                step(&mut i, &mut line, &mut col);
            }
        } else if c == '\'' {
            // `'x'`, `'\n'`, `'\u{..}'` are chars; `'a` is a lifetime.
            step(&mut i, &mut line, &mut col);
            if chars.get(i) == Some(&'\\') {
                while chars.get(i).is_some_and(|c| *c != '\'' && *c != '\n') {
                    step(&mut i, &mut line, &mut col);
                }
                step(&mut i, &mut line, &mut col);
            } else if chars.get(i + 1) == Some(&'\'') {
                step(&mut i, &mut line, &mut col);
                step(&mut i, &mut line, &mut col);
            } else {
                while chars.get(i).is_some_and(|c| c.is_alphanumeric() || *c == '_') {
                    step(&mut i, &mut line, &mut col);
                }
            }
        } else if c == '"' {
            step(&mut i, &mut line, &mut col);
            let (start, l, cl) = (i, line, col);
            while let Some(&c) = chars.get(i) {
                if c == '"' {
                    break;
                }
                if c == '\\' {
                    step(&mut i, &mut line, &mut col);
                }
                step(&mut i, &mut line, &mut col);
            }
            let end = i.min(chars.len());
            out.push(Tok::Str(chars[start.min(end)..end].iter().collect(), start, l, cl));
            step(&mut i, &mut line, &mut col);
        } else if c.is_alphanumeric() || c == '_' {
            let begin = i;
            while chars.get(i).is_some_and(|c| c.is_alphanumeric() || *c == '_') {
                step(&mut i, &mut line, &mut col);
            }
            let word: String = chars[begin..i].iter().collect();
            // Raw (and byte, C) strings: `r"…"`, `r#"…"#`, `br"…"`.
            let raw = matches!(word.as_str(), "r" | "br" | "cr");
            if raw && matches!(chars.get(i), Some('"' | '#')) {
                let mut hashes = 0usize;
                while chars.get(i) == Some(&'#') {
                    hashes += 1;
                    step(&mut i, &mut line, &mut col);
                }
                if chars.get(i) != Some(&'"') {
                    continue;
                }
                step(&mut i, &mut line, &mut col);
                let (start, l, cl) = (i, line, col);
                let closes = |at: usize| chars.get(at) == Some(&'"') && (1..=hashes).all(|k| chars.get(at + k) == Some(&'#'));
                while i < chars.len() && !closes(i) {
                    step(&mut i, &mut line, &mut col);
                }
                let end = i.min(chars.len());
                out.push(Tok::Str(chars[start.min(end)..end].iter().collect(), start, l, cl));
                for _ in 0..=hashes {
                    if i < chars.len() {
                        step(&mut i, &mut line, &mut col);
                    }
                }
            } else if matches!(word.as_str(), "b" | "c") && chars.get(i) == Some(&'"') {
                // The string itself is read on the next turn.
            } else {
                out.push(Tok::Ident(word));
            }
        } else {
            out.push(Tok::Punct(c));
            step(&mut i, &mut line, &mut col);
        }
    }
    out
}

/// Every `feature = "name"` inside `cfg(…)`, `cfg!(…)` or `cfg_attr(…)` of a Rust source, at any
/// depth of `all`, `any` and `not`, in order.
pub fn cfg_features(text: &str) -> Vec<CfgFeature> {
    let toks = lex(text);
    let mut out = Vec::new();
    // What opened each bracket: the identifier before a `(` (or before `!(`), if any.
    let mut stack: Vec<Option<&str>> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Punct('(' | '[' | '{') => {
                let opener = match (i.checked_sub(1).map(|k| &toks[k]), i.checked_sub(2).map(|k| &toks[k])) {
                    (Some(Tok::Ident(name)), _) if *t == Tok::Punct('(') => Some(name.as_str()),
                    (Some(Tok::Punct('!')), Some(Tok::Ident(name))) if *t == Tok::Punct('(') => Some(name.as_str()),
                    _ => None,
                };
                stack.push(opener);
            }
            Tok::Punct(')' | ']' | '}') => {
                stack.pop();
            }
            Tok::Ident(word) if word == "feature" => {
                let in_cfg = stack.iter().any(|o| matches!(o, Some("cfg" | "cfg_attr")));
                if let (true, Some(Tok::Punct('=')), Some(Tok::Str(name, start, line, col))) = (in_cfg, toks.get(i + 1), toks.get(i + 2)) {
                    let len = name.chars().count();
                    out.push(CfgFeature { feature: name.clone(), start: *start, end: start + len, line: *line, col: *col });
                }
            }
            _ => {}
        }
    }
    out
}

/// The feature named by the `cfg` predicate string at char offset `pos` (the quotes count).
pub fn cfg_feature_at(text: &str, pos: usize) -> Option<String> {
    cfg_features(text).into_iter().find(|f| f.start.saturating_sub(1) <= pos && pos <= f.end + 1).map(|f| f.feature)
}

/// A feature name under the caret in a `Cargo.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestFeature {
    /// The key of a `[features]` entry: `name = [...]`.
    Declared(String),
    /// A feature that an entry turns on: `"name"` in another entry's array.
    Enabled(String),
}

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'))
}

/// The quoted strings of a TOML line, as char ranges of their contents, and where a comment
/// outside them starts.
fn toml_strings(line: &[char]) -> (Vec<(usize, usize)>, usize) {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        match line[i] {
            '#' => return (out, i),
            q @ ('"' | '\'') => {
                let start = i + 1;
                i += 1;
                while i < line.len() && line[i] != q {
                    if q == '"' && line[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                out.push((start, i.min(line.len())));
                i += 1;
            }
            _ => i += 1,
        }
    }
    (out, line.len())
}

/// The feature under char offset `pos` of a `Cargo.toml`: an entry's key in `[features]`, or a
/// plain feature name in an entry's array (`"dep:x"` and `"x/y"` name dependencies: `None`).
pub fn manifest_feature_at(text: &str, pos: usize) -> Option<ManifestFeature> {
    let before: String = text.chars().take(pos).collect();
    let target = before.matches('\n').count();
    let col = before.chars().rev().take_while(|c| *c != '\n').count();
    let mut inside = false;
    let mut in_array = false;
    for (n, raw) in text.lines().enumerate() {
        let line: Vec<char> = raw.chars().collect();
        let (strings, comment) = toml_strings(&line);
        let code: String = line[..comment].iter().collect();
        let t = code.trim();
        let continuation = in_array;
        if in_array {
            in_array = !t.contains(']');
        } else if t.starts_with('[') {
            inside = !t.starts_with("[[") && t.trim_start_matches('[').trim_end_matches(']').trim() == "features";
        } else if let Some((_, value)) = t.split_once('=') {
            let value = value.trim();
            in_array = value.starts_with('[') && !value.contains(']');
        }
        if n < target {
            continue;
        }
        if !inside || t.starts_with('[') && !continuation {
            return None;
        }
        // The `=` outside strings separates the key from the value.
        let eq = (0..comment).find(|&k| line[k] == '=' && !strings.iter().any(|(s, e)| *s <= k && k < *e));
        if !continuation
            && let Some(eq) = eq
            && col <= eq
        {
            let key: String = line[..eq].iter().collect();
            let key = key.trim().trim_matches('"');
            return valid_name(key).then(|| ManifestFeature::Declared(key.to_string()));
        }
        let from = if continuation { 0 } else { eq? + 1 };
        let (s, e) = strings.into_iter().find(|(s, e)| *s >= from && s.saturating_sub(1) <= col && col <= *e + 1)?;
        let name: String = line[s..e].iter().collect();
        return valid_name(&name).then_some(ManifestFeature::Enabled(name));
    }
    None
}

/// The line and char column of the key of feature `name` in a `Cargo.toml`.
pub fn feature_declaration(text: &str, name: &str) -> Option<(usize, usize)> {
    let (line, _) = crate::feature_lines(text).into_iter().find(|(_, n)| n == name)?;
    let raw = text.lines().nth(line)?;
    let col = raw.chars().take_while(|c| c.is_whitespace() || *c == '"').count();
    Some((line, col))
}

/// One use of a feature found in the sources of a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureUse {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    /// The line's text, for result lists.
    pub preview: String,
}

/// The `.rs` files of the package in `dir`: `target`, hidden folders and nested packages (folders
/// with their own `Cargo.toml`) are left out, symbolic links are not followed. `None` when
/// cancelled.
pub fn rust_sources(dir: &Path, cancel: &AtomicBool) -> Option<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if kind.is_dir() {
                if name.starts_with('.') || name == "target" || path.join("Cargo.toml").exists() {
                    continue;
                }
                pending.push(path);
            } else if kind.is_file() && name.ends_with(".rs") {
                if out.len() >= MAX_FILES {
                    return Some(out);
                }
                out.push(path);
            }
        }
    }
    out.sort();
    Some(out)
}

/// The uses of `feature` in the `.rs` files of the package in `dir`, sorted by file and line.
/// `open` holds the text of files open in the editor, which wins over the disk. `None` when
/// cancelled.
pub fn find_feature_uses(dir: &Path, feature: &str, open: &HashMap<PathBuf, String>, cancel: &AtomicBool) -> Option<Vec<FeatureUse>> {
    let mut out = Vec::new();
    for path in rust_sources(dir, cancel)? {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let text = match open.get(&path) {
            Some(t) => t.clone(),
            None => {
                if std::fs::metadata(&path).map_or(true, |m| m.len() > MAX_SOURCE_BYTES) {
                    continue;
                }
                let Ok(t) = std::fs::read_to_string(&path) else { continue };
                t.replace("\r\n", "\n")
            }
        };
        // A cheap test first: most files never name the feature.
        if !text.contains(feature) {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for f in cfg_features(&text).into_iter().filter(|f| f.feature == feature) {
            let preview = lines.get(f.line).map(|l| l.to_string()).unwrap_or_default();
            out.push(FeatureUse { path: path.clone(), line: f.line, col: f.col, preview });
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<(String, usize, usize)> {
        cfg_features(text).into_iter().map(|f| (f.feature, f.line, f.col)).collect()
    }

    #[test]
    fn every_kind_of_cfg_is_found() {
        let src = r#"#[cfg(feature = "a")]
fn a() {}
#[cfg(all(unix, any(feature = "b", not(feature = "c"))))]
fn b() {}
#[cfg_attr(feature = "d", derive(Debug))]
struct S;
fn f() -> bool { cfg!(feature = "e") }
cfg_if::cfg_if! { if #[cfg(feature="f")] {} }
#[cfg_attr(docsrs, doc(cfg(feature = "g")))]
fn g() {}
"#;
        assert_eq!(
            names(src),
            vec![
                ("a".into(), 0, 17),
                ("b".into(), 2, 31),
                ("c".into(), 2, 50),
                ("d".into(), 4, 22),
                ("e".into(), 6, 33),
                ("f".into(), 7, 36),
                ("g".into(), 8, 38),
            ]
        );
    }

    #[test]
    fn comments_strings_and_other_macros_do_not_count() {
        let src = "// #[cfg(feature = \"a\")]\n/* #[cfg(feature = \"b\") /* nested */ ] */\nconst S: &str = \"#[cfg(feature = \\\"c\\\")]\";\nconst R: &str = r#\"cfg(feature = \"d\")\"#;\nfn f<'a>(x: &'a str) -> char { let feature = \"e\"; println!(\"{}\", feature == \"e\"); 'x' }\nmacro_rules! m { () => { other!(feature = \"f\") } }\n#[cfg(feature = \"ok\")]\nfn ok() {}\n";
        assert_eq!(names(src), vec![("ok".into(), 6, 17)]);
    }

    #[test]
    fn the_predicate_under_the_caret() {
        let src = "#[cfg(any(feature = \"std\", feature = \"alloc\"))]\nfn f() {}\n";
        let at = |needle: &str| src.find(needle).map(|b| src[..b].chars().count()).unwrap();
        assert_eq!(cfg_feature_at(src, at("std") + 1), Some("std".into()));
        assert_eq!(cfg_feature_at(src, at("\"std")), Some("std".into()));
        assert_eq!(cfg_feature_at(src, at("alloc") + 5), Some("alloc".into()));
        assert_eq!(cfg_feature_at(src, at("any")), None);
        assert_eq!(cfg_feature_at(src, at("fn f")), None);
    }

    #[test]
    fn broken_input_does_not_panic() {
        for src in ["#[cfg(feature = \"", "cfg(feature =", "r#\"", "'", "'\\", "/* /*", ")))]]]", "cfg!(feature = r#\"x", "ü'é\"ß"] {
            let _ = cfg_features(src);
            let _ = cfg_feature_at(src, 3);
            let _ = manifest_feature_at(src, 2);
        }
    }

    const MANIFEST: &str = "[package]\nname = \"demo\" # features = 1\n\n[features]\ndefault = [\"std\"]\nstd = [\"alloc\", \"dep:serde\", \"log/std\"]\n\"alloc\" = []\nfull = [\n    \"std\", # all of it\n    \"extra\",\n]\nextra = []\n\n[dependencies]\nstd = \"1\"\n";

    fn at(line: usize, col: usize) -> usize {
        MANIFEST.lines().take(line).map(|l| l.chars().count() + 1).sum::<usize>() + col
    }

    #[test]
    fn features_under_the_caret_in_the_manifest() {
        use ManifestFeature::*;
        assert_eq!(manifest_feature_at(MANIFEST, at(4, 2)), Some(Declared("default".into())));
        assert_eq!(manifest_feature_at(MANIFEST, at(4, 13)), Some(Enabled("std".into())));
        assert_eq!(manifest_feature_at(MANIFEST, at(5, 9)), Some(Enabled("alloc".into())));
        assert_eq!(manifest_feature_at(MANIFEST, at(5, 20)), None, "dep:serde is a dependency");
        assert_eq!(manifest_feature_at(MANIFEST, at(5, 33)), None, "log/std is a dependency's feature");
        assert_eq!(manifest_feature_at(MANIFEST, at(6, 2)), Some(Declared("alloc".into())));
        assert_eq!(manifest_feature_at(MANIFEST, at(8, 6)), Some(Enabled("std".into())));
        assert_eq!(manifest_feature_at(MANIFEST, at(8, 16)), None, "a comment");
        assert_eq!(manifest_feature_at(MANIFEST, at(9, 7)), Some(Enabled("extra".into())));
        assert_eq!(manifest_feature_at(MANIFEST, at(11, 1)), Some(Declared("extra".into())));
        assert_eq!(manifest_feature_at(MANIFEST, at(1, 2)), None, "another table");
        assert_eq!(manifest_feature_at(MANIFEST, at(14, 1)), None, "a dependency");
        assert_eq!(manifest_feature_at(MANIFEST, at(3, 3)), None, "the header");
    }

    #[test]
    fn the_line_of_a_feature() {
        assert_eq!(feature_declaration(MANIFEST, "extra"), Some((11, 0)));
        assert_eq!(feature_declaration(MANIFEST, "alloc"), Some((6, 1)));
        assert_eq!(feature_declaration(MANIFEST, "serde"), None);
    }

    #[test]
    fn the_uses_in_a_package() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let write = |p: &str, t: &str| {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        };
        write("Cargo.toml", "[package]\nname = \"a\"\n");
        write("src/lib.rs", "#[cfg(feature = \"x\")]\nmod m;\n");
        write("src/m.rs", "fn f() { if cfg!(feature = \"x\") {} }\n");
        write("tests/t.rs", "#[cfg(not(feature = \"x\"))]\nfn t() {}\n#[cfg(feature = \"y\")]\nfn u() {}\n");
        write("target/debug/gen.rs", "#[cfg(feature = \"x\")]\nfn g() {}\n");
        write(".hidden/h.rs", "#[cfg(feature = \"x\")]\nfn h() {}\n");
        write("member/Cargo.toml", "[package]\nname = \"b\"\n");
        write("member/src/lib.rs", "#[cfg(feature = \"x\")]\nfn b() {}\n");
        let cancel = AtomicBool::new(false);
        let mut open = HashMap::new();
        open.insert(root.join("src/m.rs"), "// unsaved\nfn f() { if cfg!(feature = \"x\") {} }\n".to_string());
        let found = find_feature_uses(root, "x", &open, &cancel).unwrap();
        let got: Vec<(String, usize, usize)> =
            found.iter().map(|u| (u.path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), u.line, u.col)).collect();
        assert_eq!(got, vec![("src/lib.rs".into(), 0, 17), ("src/m.rs".into(), 1, 28), ("tests/t.rs".into(), 0, 21)]);
        assert_eq!(found[1].preview, "fn f() { if cfg!(feature = \"x\") {} }");
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(find_feature_uses(root, "x", &open, &cancel), None);
    }

    /// A published crate from the local Cargo registry (egui), when one was downloaded.
    fn registry_crate(prefix: &str) -> Option<PathBuf> {
        let home = std::env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))?;
        let mut found: Vec<PathBuf> = std::fs::read_dir(home.join("registry/src"))
            .ok()?
            .flatten()
            .flat_map(|index| std::fs::read_dir(index.path()).into_iter().flatten().flatten())
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().strip_prefix(prefix).is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()))))
            .filter(|p| p.join("Cargo.toml").is_file() && p.join("src/lib.rs").is_file())
            .collect();
        found.sort();
        found.pop()
    }

    /// On a real crate: every `cfg` use of a declared feature leads back to its `[features]` line,
    /// and the features named in the manifest list their uses in the sources.
    #[test]
    fn a_real_crate_from_the_registry() {
        let Some(dir) = registry_crate("egui-") else {
            eprintln!("skipped: no egui in the local Cargo registry");
            return;
        };
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        let declared: Vec<String> = crate::feature_lines(&manifest).into_iter().map(|(_, n)| n).collect();
        assert!(declared.iter().any(|n| n == "serde") && declared.iter().any(|n| n == "persistence"), "{declared:?}");
        // From each key, and from a feature that `persistence` turns on.
        for name in ["serde", "persistence"] {
            let (line, col) = feature_declaration(&manifest, name).unwrap();
            let pos = manifest.lines().take(line).map(|l| l.chars().count() + 1).sum::<usize>() + col + 1;
            assert_eq!(manifest_feature_at(&manifest, pos), Some(ManifestFeature::Declared(name.into())));
        }
        let (p, _) = feature_declaration(&manifest, "persistence").unwrap();
        let line = manifest.lines().enumerate().skip(p).find(|(_, l)| l.trim() == "\"serde\",").map(|(n, _)| n).unwrap();
        let pos = manifest.lines().take(line).map(|l| l.chars().count() + 1).sum::<usize>() + 6;
        assert_eq!(manifest_feature_at(&manifest, pos), Some(ManifestFeature::Enabled("serde".into())));

        let cancel = AtomicBool::new(false);
        for name in ["serde", "persistence", "callstack"] {
            let uses = find_feature_uses(&dir, name, &HashMap::new(), &cancel).unwrap();
            assert!(!uses.is_empty(), "no uses of {name}");
            for u in &uses {
                assert!(u.path.starts_with(&dir) && u.preview.contains(name), "{u:?}");
                // The way back: the string under the caret names the feature, which is declared.
                let text = std::fs::read_to_string(&u.path).unwrap().replace("\r\n", "\n");
                let at = text.lines().take(u.line).map(|l| l.chars().count() + 1).sum::<usize>() + u.col;
                assert_eq!(cfg_feature_at(&text, at).as_deref(), Some(name), "{u:?}");
                assert!(feature_declaration(&manifest, name).is_some());
            }
        }
    }
}
