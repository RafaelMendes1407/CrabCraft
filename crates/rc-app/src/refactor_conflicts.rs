//! Refactoring conflicts: what may go wrong if a rename is applied, listed for the user to
//! review before anything changes (the new name is not a valid identifier, is already used in
//! a file the rename touches, or the edits reach files outside the project, in the build output,
//! in a dependency or read-only, or no longer match the file's text).
//!
//! The checks read closed files and their metadata, so they run on a thread.

use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use rc_lsp::lsp_types as lsp;
use rc_text::Rope;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One conflict: where it is (a file and 0-based line and column, when it has a place) and
/// what it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    pub path: Option<PathBuf>,
    pub line: usize,
    pub col: usize,
    pub message: String,
}

/// A rename with conflicts, waiting for the user to continue or cancel.
#[derive(Clone, Debug)]
pub struct Review {
    pub name: String,
    pub conflicts: Vec<Conflict>,
    /// The edits to apply on "Continue anyway"; `None` when the server gave none (an error).
    pub edit: Option<lsp::WorkspaceEdit>,
    /// The preview's title when the edits are to be reviewed instead of applied.
    pub preview: Option<String>,
}

/// The conflict checks of a rename, running on a thread.
pub struct Check {
    pub name: String,
    pub edit: lsp::WorkspaceEdit,
    pub preview: Option<String>,
    inbox: Inbox<Vec<Conflict>>,
}

impl Check {
    /// Starts checking `edit`, the rename of a symbol to `name`. `open` are the open
    /// documents' texts; `root` is the project's folder.
    pub fn start(name: String, edit: lsp::WorkspaceEdit, preview: Option<String>, root: Option<PathBuf>, open: Vec<(PathBuf, Rope)>, waker: Waker) -> Check {
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let files = crate::insight::workspace_edit_files(&edit);
        let (n, r) = (name.clone(), root.clone());
        let spawned = std::thread::Builder::new().name("rc-refactor-conflicts".into()).spawn(move || out.send(check(&n, &files, r.as_deref(), &open)));
        if let Err(e) = spawned {
            let failed = Conflict { path: None, line: 0, col: 0, message: trf("Could not check for conflicts: {error}", &[("error", &e)]) };
            inbox.sender().send(vec![failed]);
        }
        Check { name, edit, preview, inbox }
    }

    /// The conflicts once the checks are done.
    pub fn poll(&self) -> Option<Vec<Conflict>> {
        self.inbox.drain().pop()
    }
}

/// The server could not rename: its error, with what is wrong with the name.
pub fn server_error(name: &str, error: &str) -> Vec<Conflict> {
    let mut out: Vec<Conflict> = name_problem(name).into_iter().map(|message| Conflict { path: None, line: 0, col: 0, message }).collect();
    out.push(Conflict { path: None, line: 0, col: 0, message: trf("rust-analyzer could not rename: {error}", &[("error", &error.trim())]) });
    out
}

/// Every conflict of renaming to `name` with the edits `files`.
pub fn check(name: &str, files: &BTreeMap<PathBuf, Vec<lsp::TextEdit>>, root: Option<&Path>, open: &[(PathBuf, Rope)]) -> Vec<Conflict> {
    let mut out: Vec<Conflict> = name_problem(name).into_iter().map(|message| Conflict { path: None, line: 0, col: 0, message }).collect();
    let word = name.strip_prefix("r#").unwrap_or(name);
    for (path, edits) in files {
        let first = edits.iter().map(|e| e.range.start).min_by_key(|p| (p.line, p.character)).unwrap_or(lsp::Position::new(0, 0));
        let at = |message: String| Conflict { path: Some(path.clone()), line: first.line as usize, col: first.character as usize, message };
        if let Some(why) = place_problem(path, root) {
            out.push(at(why));
        }
        if std::fs::metadata(path).is_ok_and(|m| m.permissions().readonly()) {
            out.push(at(tr("This file is read-only").into()));
        }
        let rope = match open.iter().find(|(p, _)| p == path) {
            Some((_, rope)) => rope.clone(),
            None => match crate::docs::read_text(path) {
                Ok((text, _, _)) => Rope::from_str(&text),
                Err(e) => {
                    out.push(at(trf("This file cannot be read: {error}", &[("error", &e)])));
                    continue;
                }
            },
        };
        if edits.iter().any(|e| !fits(&rope, &e.range)) {
            out.push(at(tr("The edits do not match the file's current text").into()));
            continue;
        }
        if !word.is_empty() {
            for (line, col) in uses_of(&rope.to_string(), word).into_iter().take(20) {
                out.push(Conflict { path: Some(path.clone()), line, col, message: trf("'{name}' is already used here", &[("name", &word)]) });
            }
        }
    }
    out
}

/// What is wrong with `name` as a Rust identifier, if anything.
pub fn name_problem(name: &str) -> Option<String> {
    let (raw, word) = match name.strip_prefix("r#") {
        Some(w) => (true, w),
        None => (false, name),
    };
    let mut chars = word.chars();
    let valid = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_') && chars.all(|c| c.is_alphanumeric() || c == '_');
    if !valid || word == "_" {
        return Some(trf("'{name}' is not a valid identifier", &[("name", &name)]));
    }
    // Raw identifiers cannot be these.
    if matches!(word, "crate" | "self" | "Self" | "super") {
        return Some(trf("'{name}' is a keyword", &[("name", &name)]));
    }
    if !raw && is_keyword(word) {
        return Some(trf("'{name}' is a keyword", &[("name", &name)]));
    }
    None
}

/// Strict and reserved keywords of Rust 2021 and 2024.
fn is_keyword(w: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "gen", "macro", "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
    ];
    KEYWORDS.contains(&w)
}

/// Why editing `path` is risky: outside the project, in the build output or in a dependency.
fn place_problem(path: &Path, root: Option<&Path>) -> Option<String> {
    let parts: Vec<String> = path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let dependency = parts.windows(2).any(|w| (w[0] == ".cargo" && (w[1] == "registry" || w[1] == "git")) || (w[0] == ".rustup" && w[1] == "toolchains"));
    if dependency {
        return Some(tr("This file belongs to a dependency or the toolchain").into());
    }
    let root = root?;
    match path.strip_prefix(root) {
        Ok(rel) if rel.components().next().is_some_and(|c| c.as_os_str() == "target") => Some(tr("This file is generated (in the build output folder)").into()),
        Ok(_) => None,
        Err(_) => Some(tr("This file is outside the project").into()),
    }
}

/// Whether `range` lies within the text (columns in UTF-16 units, as LSP sends them).
fn fits(rope: &Rope, range: &lsp::Range) -> bool {
    let ok = |p: &lsp::Position| {
        let line = p.line as usize;
        if line >= rope.len_lines() {
            // The end of a text without a final line break.
            return line == rope.len_lines() && p.character == 0;
        }
        let text = rope.line(line).to_string();
        let text = text.trim_end_matches(['\n', '\r']);
        p.character as usize <= text.encode_utf16().count()
    };
    ok(&range.start) && ok(&range.end) && (range.start.line, range.start.character) <= (range.end.line, range.end.character)
}

/// The places (0-based line, column in characters) where `word` appears as a whole identifier
/// in Rust code, leaving out comments, strings and character literals.
pub fn uses_of(text: &str, word: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let target: Vec<char> = word.chars().collect();
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let (mut i, mut line, mut line_start) = (0usize, 0usize, 0usize);
    let newline = |i: usize, line: &mut usize, line_start: &mut usize| {
        *line += 1;
        *line_start = i + 1;
    };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '\n' {
            newline(i, &mut line, &mut line_start);
            i += 1;
        } else if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 1;
            i += 2;
            while i < chars.len() && depth > 0 {
                match (chars[i], chars.get(i + 1).copied()) {
                    ('/', Some('*')) => {
                        depth += 1;
                        i += 2;
                    }
                    ('*', Some('/')) => {
                        depth -= 1;
                        i += 2;
                    }
                    ('\n', _) => {
                        newline(i, &mut line, &mut line_start);
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
        } else if c == '"' || (c == 'r' && matches!(next, Some('"') | Some('#')) && raw_string_start(&chars, i).is_some()) {
            // Ordinary or raw string: skipped to its end.
            let (hashes, mut j) = if c == '"' { (None, i + 1) } else { raw_string_start(&chars, i).map_or((None, i + 1), |(h, j)| (Some(h), j)) };
            while j < chars.len() {
                let d = chars[j];
                if d == '\n' {
                    newline(j, &mut line, &mut line_start);
                }
                match hashes {
                    None if d == '\\' => j += 1,
                    None if d == '"' => break,
                    Some(h) if d == '"' && (1..=h).all(|k| chars.get(j + k) == Some(&'#')) => {
                        j += h;
                        break;
                    }
                    _ => {}
                }
                j += 1;
            }
            i = j + 1;
        } else if c == '\'' {
            // A character literal ('x', '\n'); a lifetime ('a) has no closing quote nearby.
            let close = if next == Some('\\') { (i + 2..chars.len().min(i + 12)).find(|&k| chars[k] == '\'') } else { (chars.get(i + 2) == Some(&'\'')).then_some(i + 2) };
            i = close.map_or(i + 1, |k| k + 1);
        } else if is_ident(c) {
            let start = i;
            while i < chars.len() && is_ident(chars[i]) {
                i += 1;
            }
            if chars[start..i] == target[..] {
                out.push((line, start - line_start));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// At `i`, `r"` or `r#…#"`: the number of hashes and where the content starts.
fn raw_string_start(chars: &[char], i: usize) -> Option<(usize, usize)> {
    if i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_') {
        return None;
    }
    let mut j = i + 1;
    while chars.get(j) == Some(&'#') {
        j += 1;
    }
    (chars.get(j) == Some(&'"')).then_some((j - i - 1, j + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(line: u32, from: u32, to: u32, text: &str) -> lsp::TextEdit {
        lsp::TextEdit { range: lsp::Range::new(lsp::Position::new(line, from), lsp::Position::new(line, to)), new_text: text.into() }
    }

    #[test]
    fn keywords_and_invalid_names_are_conflicts() {
        assert!(name_problem("total").is_none());
        assert!(name_problem("_tmp").is_none());
        assert!(name_problem("r#fn").is_none());
        assert!(name_problem("número").is_none());
        for bad in ["fn", "Self", "r#self", "async", "", "_", "1st", "a-b", "a b"] {
            assert!(name_problem(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn uses_leave_out_comments_strings_and_other_words() {
        let text = "fn bye() {}\n// bye\nlet s = \"bye\"; let r = r#\"bye\"#;\n/* bye /* bye */ */ let c = 'b'; fn f<'a>(x: &'a u8) { bye_now(); bye(); }\n";
        assert_eq!(uses_of(text, "bye"), [(0, 3), (3, 66)]);
    }

    #[test]
    fn edits_outside_the_project_in_target_read_only_or_stale_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug/build")).unwrap();
        let lib = root.join("src/lib.rs");
        std::fs::write(&lib, "pub fn hello() {}\npub fn bye() {}\n").unwrap();
        let generated = root.join("target/debug/build/gen.rs");
        std::fs::write(&generated, "fn hello() {}\n").unwrap();
        let outside = dir.path().join("other.rs");
        std::fs::write(&outside, "fn hello() {}\n").unwrap();
        let locked = root.join("src/locked.rs");
        std::fs::write(&locked, "fn hello() {}\n").unwrap();
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&locked, perms).unwrap();
        let stale = root.join("src/stale.rs");
        let mut files = BTreeMap::new();
        files.insert(lib.clone(), vec![edit(0, 7, 12, "bye")]);
        files.insert(generated.clone(), vec![edit(0, 3, 8, "bye")]);
        files.insert(outside.clone(), vec![edit(0, 3, 8, "bye")]);
        files.insert(locked.clone(), vec![edit(0, 3, 8, "bye")]);
        files.insert(stale.clone(), vec![edit(5, 0, 5, "bye")]);
        let open = vec![(stale.clone(), Rope::from_str("fn hello() {}\n"))];
        let found = check("bye", &files, Some(&root), &open);
        let has = |p: &Path, line: usize, what: &str| found.iter().any(|c| c.path.as_deref() == Some(p) && c.line == line && c.message == what);
        assert!(has(&lib, 1, "'bye' is already used here"), "{found:#?}");
        assert!(has(&generated, 0, "This file is generated (in the build output folder)"), "{found:#?}");
        assert!(has(&outside, 0, "This file is outside the project"), "{found:#?}");
        assert!(has(&stale, 5, "The edits do not match the file's current text"), "{found:#?}");
        // Root ignores permissions: only check read-only when the file really is.
        if std::fs::OpenOptions::new().write(true).open(&locked).is_err() {
            assert!(has(&locked, 0, "This file is read-only"), "{found:#?}");
        }
        assert_eq!(found.len(), 4 + usize::from(std::fs::OpenOptions::new().write(true).open(&locked).is_err()), "{found:#?}");
        assert!(check("greet", &files.into_iter().filter(|(p, _)| *p == lib).collect(), Some(&root), &[]).is_empty());
    }
}
