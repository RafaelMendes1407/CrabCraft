//! Copyright header profiles: the header a project's files start with, written in the file's
//! comment style. New files get it when the profile says so; Update Copyright adds it to the
//! current file, or brings the year range of the header it already has up to date.

use rc_core::inbox::Inbox;
use rc_core::session::CopyrightProfile;
use rc_syntax::Lang;
use std::path::{Path, PathBuf};

/// Lines of a template kept; the rest is dropped (the session file is untrusted).
const MAX_LINES: usize = 40;
/// Longer lines are not compared with the profile (the matcher backtracks).
const MAX_MATCH_LINE: usize = 400;

/// What the variables of a template stand for in one file.
#[derive(Clone, Copy, Debug)]
pub struct Vars<'a> {
    pub year: i32,
    pub file: &'a str,
    pub project: &'a str,
    pub author: &'a str,
}

/// One line, without control characters: values come from file names, git and settings.
fn clean_value(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
}

/// Whether `id` reads as an SPDX license expression (`MIT`, `MIT OR Apache-2.0`,
/// `GPL-2.0-or-later WITH Classpath-exception-2.0`).
pub fn valid_spdx(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty() && id.len() <= 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '+' | '(' | ')' | ':' | ' '))
}

/// The template's lines (trimmed at the end, at most [`MAX_LINES`]), without blank lines
/// around them.
fn template_lines(template: &str) -> Vec<&str> {
    let lines: Vec<&str> = template.lines().take(MAX_LINES).map(str::trim_end).collect();
    let start = lines.iter().position(|l| !l.is_empty()).unwrap_or(lines.len());
    let end = lines.iter().rposition(|l| !l.is_empty()).map_or(start, |e| e + 1);
    lines[start..end].to_vec()
}

/// A variable of a template starting at `s` (after its `$`): its name and length.
fn variable(s: &str) -> Option<&'static str> {
    ["year", "file", "project", "author"].into_iter().find(|v| s.starts_with(v) && !s[v.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
}

/// One template line with its variables replaced (an unknown `$name` stays as written).
fn expand_line(line: &str, vars: &Vars) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        match variable(after) {
            Some(name) => {
                out.push_str(&match name {
                    "year" => vars.year.to_string(),
                    "file" => clean_value(vars.file),
                    "project" => clean_value(vars.project),
                    _ => clean_value(vars.author),
                });
                rest = &after[name.len()..];
            }
            None => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    // An empty variable leaves a space before the end of the line.
    out.trim_end().to_string()
}

/// The header's text, without comment markers: the template with its variables replaced,
/// then the SPDX line. Empty when the template is.
pub fn expand(profile: &CopyrightProfile, vars: &Vars) -> String {
    let mut lines: Vec<String> = template_lines(&profile.template).into_iter().map(|l| expand_line(l, vars)).collect();
    let spdx = profile.spdx.trim();
    if valid_spdx(spdx) {
        lines.push(format!("SPDX-License-Identifier: {spdx}"));
    }
    lines.join("\n")
}

/// `text` as a comment of `lang`, ending with a newline: line comments where the language
/// has them, else one block comment. `None` for languages without comments (and JSON,
/// whose comments most tools reject).
pub fn comment(text: &str, lang: Lang) -> Option<String> {
    if text.is_empty() || lang == Lang::Json {
        return None;
    }
    if let Some(prefix) = lang.line_comment() {
        return Some(text.lines().map(|l| if l.is_empty() { format!("{prefix}\n") } else { format!("{prefix} {l}\n") }).collect());
    }
    let (open, close) = lang.block_comment()?;
    // The text cannot end the comment early.
    let safe = |l: &str| l.replace(close, &close.chars().map(String::from).collect::<Vec<_>>().join(" "));
    let mut out = format!("{open}\n");
    for l in text.lines() {
        if l.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("  {}\n", safe(l)));
        }
    }
    out.push_str(close);
    out.push('\n');
    Some(out)
}

/// Where a header goes: the very top, except after a byte order mark, a shebang (`#!` not
/// followed by `[`, which starts an inner attribute such as `#![allow(..)]`) and an XML
/// declaration, which must come first.
pub fn insertion_point(text: &str) -> usize {
    let mut at = if text.starts_with('\u{feff}') { '\u{feff}'.len_utf8() } else { 0 };
    let rest = &text[at..];
    let first_line_end = |s: &str| s.find('\n').map_or(s.len(), |n| n + 1);
    if let Some(after) = rest.strip_prefix("#!")
        && !after.trim_start().starts_with('[')
    {
        at += first_line_end(rest);
    } else if rest.starts_with("<?xml") {
        at += first_line_end(rest);
    }
    at
}

/// A token of the line of a template with `$year`: text, the year (`2024` or `2020-2024`) or
/// another variable (any text).
#[derive(Debug, PartialEq)]
enum Token {
    Text(String),
    Year,
    Any,
}

fn tokens(line: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = line.trim();
    while let Some(at) = rest.find('$') {
        text.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        match variable(after) {
            Some(name) => {
                if !text.is_empty() {
                    out.push(Token::Text(std::mem::take(&mut text)));
                }
                out.push(if name == "year" { Token::Year } else { Token::Any });
                rest = &after[name.len()..];
            }
            None => {
                text.push('$');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    if !text.is_empty() {
        out.push(Token::Text(text));
    }
    out
}

/// Four ASCII digits at `s[at..]`.
fn year_at(s: &str, at: usize) -> Option<i32> {
    let digits = s.get(at..at + 4)?;
    digits.bytes().all(|b| b.is_ascii_digit()).then(|| digits.parse().ok()).flatten()
}

/// The ends of a year or year range starting at `at` (the range first, then the single year).
fn year_ends(s: &str, at: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    if year_at(s, at).is_none() {
        return ends;
    }
    let i = at + 4;
    let skip_spaces = |mut i: usize| {
        while s[i..].starts_with(' ') {
            i += 1;
        }
        i
    };
    let j = skip_spaces(i);
    let sep = ["-", "–"].into_iter().find(|d| s[j..].starts_with(d));
    if let Some(d) = sep {
        let k = skip_spaces(j + d.len());
        if year_at(s, k).is_some() {
            ends.push(k + 4);
        }
    }
    ends.push(i);
    ends
}

/// Matches `s[at..]` against `tokens`; the byte ranges of the years go to `years`. Positions
/// that failed are remembered in `failed`, so a template with many variables cannot make the
/// search take exponential time.
fn matches(tokens: &[Token], s: &str, at: usize, years: &mut Vec<(usize, usize)>, failed: &mut std::collections::HashSet<(usize, usize)>) -> bool {
    let Some((first, rest)) = tokens.split_first() else { return at == s.len() };
    let key = (tokens.len(), at);
    if failed.contains(&key) {
        return false;
    }
    let ok = match first {
        Token::Text(t) => s[at..].starts_with(t.as_str()) && matches(rest, s, at + t.len(), years, failed),
        Token::Year => year_ends(s, at).into_iter().any(|end| {
            years.push((at, end));
            matches(rest, s, end, years, failed) || {
                years.pop();
                false
            }
        }),
        Token::Any => s[at..]
            .char_indices()
            .skip(1)
            .map(|(i, _)| at + i)
            .chain(std::iter::once(s.len()))
            .filter(|e| *e > at)
            .any(|end| matches(rest, s, end, years, failed)),
    };
    if !ok {
        failed.insert(key);
    }
    ok
}

/// The comments at `text[at..]`, blank lines between them included: each line's byte range
/// and its text without the comment markers.
fn leading_comments(text: &str, at: usize, lang: Lang) -> Vec<(usize, usize)> {
    let line = lang.line_comment();
    let block = lang.block_comment();
    let mut out = Vec::new();
    let mut pos = at;
    let mut in_block = false;
    for raw in text[at..].split_inclusive('\n') {
        let start = pos;
        pos += raw.len();
        let l = raw.trim();
        if in_block {
            out.push((start, start + raw.trim_end().len()));
            in_block = block.is_some_and(|(_, close)| !l.contains(close));
            continue;
        }
        if l.is_empty() {
            continue;
        }
        // Doc comments (`///`, `//!`) document code: they are not a header.
        let is_line = line.is_some_and(|p| l.starts_with(p) && !(p == "//" && (l.starts_with("///") || l.starts_with("//!"))));
        let is_block = block.is_some_and(|(open, _)| l.starts_with(open) && !(open == "/*" && (l.starts_with("/**") || l.starts_with("/*!"))));
        if is_line {
            out.push((start, start + raw.trim_end().len()));
        } else if is_block {
            out.push((start, start + raw.trim_end().len()));
            in_block = block.is_some_and(|(open, close)| !l[open.len()..].contains(close));
        } else {
            break;
        }
    }
    out
}

/// A comment line without its markers, and where that text starts in the line.
fn comment_text(line: &str, lang: Lang) -> (usize, &str) {
    let lead = line.len() - line.trim_start().len();
    let mut s = line.trim_start();
    let mut at = lead;
    let strip = |s: &mut &str, at: &mut usize, p: &str| {
        if let Some(r) = s.strip_prefix(p) {
            *at += p.len();
            *s = r;
        }
    };
    if let Some(p) = lang.line_comment() {
        strip(&mut s, &mut at, p);
    }
    if let Some((open, close)) = lang.block_comment() {
        strip(&mut s, &mut at, open);
        if let Some(r) = s.strip_suffix(close) {
            s = r;
        }
        // ` * text` inside a block comment.
        if open == "/*" && s.trim_start().starts_with('*') {
            let skip = s.len() - s.trim_start().len() + 1;
            at += skip;
            s = &s[skip..];
        }
    }
    let skip = s.len() - s.trim_start().len();
    (at + skip, s.trim())
}

/// What Update Copyright does to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// The file had no header: one is inserted.
    Added,
    /// The header's year range now ends this year.
    Updated,
    /// The header is there and up to date.
    Current,
}

/// An edit of a file's text (byte offsets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderEdit {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub change: Change,
}

/// `2020` → `2020-2026`, `2020-2024` → `2020-2026`; `None` when it already ends this year
/// (or later).
pub fn updated_years(found: &str, year: i32) -> Option<String> {
    let first = year_at(found, 0)?;
    let last = found.len().checked_sub(4).and_then(|at| year_at(found, at)).unwrap_or(first);
    if last >= year || first > year {
        return None;
    }
    Some(if first == year { year.to_string() } else { format!("{first}-{year}") })
}

/// The edit Update Copyright makes to `text`: the header inserted at
/// [`insertion_point`], or the year range of the header already there updated. `None` when
/// the profile has no template or `lang` has no comments.
pub fn header_edit(text: &str, lang: Lang, profile: &CopyrightProfile, vars: &Vars) -> Option<HeaderEdit> {
    let header = comment(&expand(profile, vars), lang)?;
    let at = insertion_point(text);
    let lines = template_lines(&profile.template);
    let comments = leading_comments(text, at, lang);
    // The header is found by the first template line with `$year` (the others may have been
    // edited), else by its first line as it expands today.
    let year_line = lines.iter().find(|l| tokens(l).contains(&Token::Year)).map(|l| tokens(l));
    for &(start, end) in &comments {
        let line = &text[start..end];
        if line.len() > MAX_MATCH_LINE {
            continue;
        }
        let (offset, body) = comment_text(line, lang);
        match &year_line {
            Some(toks) => {
                let mut years = Vec::new();
                if matches(toks, body, 0, &mut years, &mut Default::default()) {
                    let Some(&(ys, ye)) = years.first() else { continue };
                    let (ys, ye) = (start + offset + ys, start + offset + ye);
                    return Some(match updated_years(&text[ys..ye], vars.year) {
                        Some(new) => HeaderEdit { start: ys, end: ye, text: new, change: Change::Updated },
                        None => HeaderEdit { start: ys, end: ys, text: String::new(), change: Change::Current },
                    });
                }
            }
            None => {
                let first = lines.first().map(|l| expand_line(l, vars)).unwrap_or_default();
                if !first.is_empty() && body == first.trim() {
                    return Some(HeaderEdit { start, end: start, text: String::new(), change: Change::Current });
                }
            }
        }
    }
    // A blank line between the header and the code.
    let rest = &text[at..];
    let gap = if rest.is_empty() || rest.starts_with('\n') { "" } else { "\n" };
    Some(HeaderEdit { start: at, end: at, text: format!("{header}{gap}"), change: Change::Added })
}

/// `edit` applied to `text`.
pub fn apply(text: &str, edit: &HeaderEdit) -> String {
    format!("{}{}{}", &text[..edit.start], edit.text, &text[edit.end..])
}

/// This year (UTC).
pub fn current_year() -> i32 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    crate::vcs::format_date(secs).get(..4).and_then(|y| y.parse().ok()).unwrap_or(1970)
}

/// The project's profile, the author git knows for it and the draft Settings edits.
#[derive(Default)]
pub struct Copyright {
    pub profile: CopyrightProfile,
    pub root: Option<PathBuf>,
    /// `user.name` of the project's git configuration, read once in the background.
    pub author: Option<String>,
    author_load: Option<Inbox<Option<String>>>,
    /// The profile being edited in Settings.
    pub draft: Option<CopyrightProfile>,
}

impl Copyright {
    /// The project opened: its profile, and its author asked of git in the background (git
    /// reads its configuration; nothing of the project runs).
    pub fn open(&mut self, root: &Path, profile: CopyrightProfile, waker: rc_core::Waker) {
        self.profile = profile;
        self.root = Some(root.to_path_buf());
        self.author = None;
        self.draft = None;
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let dir = root.to_path_buf();
        std::thread::spawn(move || out.send(git_user_name(&dir)));
        self.author_load = Some(inbox);
    }

    pub fn close(&mut self) {
        *self = Copyright::default();
    }

    pub fn poll(&mut self) {
        if let Some(name) = self.author_load.as_ref().and_then(|i| i.drain().into_iter().last()) {
            self.author_load = None;
            self.author = name;
        }
    }

    /// Whether the author is still being read.
    pub fn loading(&self) -> bool {
        self.author_load.is_some()
    }

    /// The project's name: its folder's.
    pub fn project_name(&self) -> String {
        self.root.as_ref().and_then(|r| r.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// The edit Update Copyright makes to the file at `path` holding `text`.
    pub fn edit_for(&self, path: &Path, text: &str) -> Option<HeaderEdit> {
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let project = self.project_name();
        let vars = Vars { year: current_year(), file: &file, project: &project, author: self.author.as_deref().unwrap_or("") };
        header_edit(text, Lang::from_path(path), &self.profile, &vars)
    }

    /// The content of a new file, with the header when the profile adds it to new files.
    pub fn new_file_content(&self, path: &Path, content: &str) -> String {
        if !self.profile.on_new_files || self.root.is_none() {
            return content.to_string();
        }
        match self.edit_for(path, content) {
            Some(edit) if edit.change == Change::Added => apply(content, &edit),
            _ => content.to_string(),
        }
    }
}

/// `git config user.name` in `dir`; `None` without git or a name.
fn git_user_name(dir: &Path) -> Option<String> {
    let git = rc_vcs::Git::find_executable()?;
    let out = std::process::Command::new(git).args(["config", "user.name"]).current_dir(dir).stdin(std::process::Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let name = clean_value(&String::from_utf8_lossy(&out.stdout));
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(year: i32) -> Vars<'static> {
        Vars { year, file: "net.rs", project: "demo", author: "Ada Lovelace" }
    }

    fn profile(template: &str, spdx: &str) -> CopyrightProfile {
        CopyrightProfile { template: template.into(), spdx: spdx.into(), on_new_files: true }
    }

    #[test]
    fn templates_expand_their_variables() {
        let p = profile("\n\nCopyright $year $author\n$file in $project, $price $years\n\n", "MIT OR Apache-2.0");
        assert_eq!(expand(&p, &vars(2026)), "Copyright 2026 Ada Lovelace\nnet.rs in demo, $price $years\nSPDX-License-Identifier: MIT OR Apache-2.0");
        // No author: no space left at the end; a bad license id is left out.
        let v = Vars { author: "\u{1b}\n\t ", ..vars(2026) };
        assert_eq!(expand(&profile("Copyright $year $author", "MIT; rm -rf"), &v), "Copyright 2026");
        assert_eq!(expand(&profile("", ""), &v), "");
        assert!(valid_spdx("GPL-2.0-or-later WITH Classpath-exception-2.0") && !valid_spdx("") && !valid_spdx("a\nb"));
    }

    #[test]
    fn headers_take_the_comment_style_of_the_language() {
        assert_eq!(comment("Copyright 2026\n\nMIT", Lang::Rust).as_deref(), Some("// Copyright 2026\n//\n// MIT\n"));
        assert_eq!(comment("Copyright 2026", Lang::Toml).as_deref(), Some("# Copyright 2026\n"));
        assert_eq!(comment("a --> b", Lang::Xml).as_deref(), Some("<!--\n  a - - > b\n-->\n"));
        assert_eq!(comment("Copyright", Lang::Json), None);
        assert_eq!(comment("Copyright", Lang::Plain), None);
        assert_eq!(comment("", Lang::Rust), None);
    }

    #[test]
    fn the_header_goes_first_except_after_a_shebang() {
        assert_eq!(insertion_point("#![allow(dead_code)]\nfn main() {}\n"), 0);
        assert_eq!(insertion_point("#! [allow(dead_code)]\n"), 0);
        assert_eq!(insertion_point("#!/usr/bin/env run-cargo-script\nfn main() {}\n"), 32);
        assert_eq!(insertion_point("\u{feff}fn main() {}"), 3);
        assert_eq!(insertion_point("<?xml version=\"1.0\"?>\n<a/>\n"), 22);
        assert_eq!(insertion_point(""), 0);
        let p = profile("Copyright $year $author", "");
        let text = "#!/usr/bin/env rust-script\n#![allow(unused)]\nfn main() {}\n";
        let e = header_edit(text, Lang::Rust, &p, &vars(2026)).unwrap();
        assert_eq!(apply(text, &e), "#!/usr/bin/env rust-script\n// Copyright 2026 Ada Lovelace\n\n#![allow(unused)]\nfn main() {}\n");
        let text = "#![allow(unused)]\n";
        assert_eq!(apply(text, &header_edit(text, Lang::Rust, &p, &vars(2026)).unwrap()), "// Copyright 2026 Ada Lovelace\n\n#![allow(unused)]\n");
    }

    #[test]
    fn year_ranges_end_this_year() {
        assert_eq!(updated_years("2024", 2026).as_deref(), Some("2024-2026"));
        assert_eq!(updated_years("2020-2024", 2026).as_deref(), Some("2020-2026"));
        assert_eq!(updated_years("2020 – 2024", 2026).as_deref(), Some("2020-2026"));
        assert_eq!(updated_years("2026", 2026), None);
        assert_eq!(updated_years("2020-2026", 2026), None);
        assert_eq!(updated_years("2030", 2026), None);
        assert_eq!(updated_years("20x0", 2026), None);
    }

    #[test]
    fn update_copyright_adds_or_updates_the_header() {
        let p = profile("Copyright (c) $year $author\nPart of $project.", "MIT");
        // No header: added, with a blank line before the code; doc comments are not a header.
        let text = "//! The net module.\nfn f() {}\n";
        let e = header_edit(text, Lang::Rust, &p, &vars(2026)).unwrap();
        assert_eq!(e.change, Change::Added);
        assert_eq!(
            apply(text, &e),
            "// Copyright (c) 2026 Ada Lovelace\n// Part of demo.\n// SPDX-License-Identifier: MIT\n\n//! The net module.\nfn f() {}\n"
        );
        // An old header, by another author: only its year changes.
        let text = "// Copyright (c) 2020 Grace Hopper\n// Part of demo.\n\nfn f() {}\n";
        let e = header_edit(text, Lang::Rust, &p, &vars(2026)).unwrap();
        assert_eq!(e.change, Change::Updated);
        assert_eq!(apply(text, &e), "// Copyright (c) 2020-2026 Grace Hopper\n// Part of demo.\n\nfn f() {}\n");
        // Run again: nothing to do.
        let text = apply(text, &e);
        assert_eq!(header_edit(&text, Lang::Rust, &p, &vars(2026)).unwrap().change, Change::Current);
        // A block comment header, after a shebang.
        let text = "#!/bin/run\n/*\n * Copyright (c) 2019-2024 Ada Lovelace\n */\nfn f() {}\n";
        let e = header_edit(text, Lang::Rust, &p, &vars(2026)).unwrap();
        assert_eq!(apply(text, &e), "#!/bin/run\n/*\n * Copyright (c) 2019-2026 Ada Lovelace\n */\nfn f() {}\n");
        // A header that is not the profile's: a new one goes above it.
        let text = "// Some other notice 2020\nfn f() {}\n";
        assert_eq!(header_edit(text, Lang::Rust, &p, &vars(2026)).unwrap().change, Change::Added);
        // A template without `$year` is found by its first line.
        let p = profile("Part of $project.", "");
        assert_eq!(header_edit("// Part of demo.\n", Lang::Rust, &p, &vars(2026)).unwrap().change, Change::Current);
        // Nothing for empty templates and JSON.
        assert_eq!(header_edit("fn f() {}", Lang::Rust, &profile("", ""), &vars(2026)), None);
        assert_eq!(header_edit("{}", Lang::Json, &p, &vars(2026)), None);
    }

    #[test]
    fn new_files_start_with_the_header() {
        let mut c =
            Copyright { profile: profile("Copyright $year $author", ""), root: Some("/w/demo".into()), author: Some("Ada".into()), ..Default::default() };
        let year = current_year();
        assert_eq!(c.new_file_content(Path::new("/w/demo/src/net.rs"), "$0"), format!("// Copyright {year} Ada\n\n$0"));
        assert_eq!(c.new_file_content(Path::new("/w/demo/a.json"), "{\n  $0\n}\n"), "{\n  $0\n}\n");
        c.profile.on_new_files = false;
        assert_eq!(c.new_file_content(Path::new("/w/demo/src/net.rs"), "$0"), "$0");
    }

    #[test]
    fn many_variables_do_not_make_matching_slow() {
        let p = profile(&format!("{}$year", "$file x ".repeat(30)), "");
        let text = format!("// {}\nfn f() {{}}\n", "x ".repeat(190));
        let started = std::time::Instant::now();
        assert_eq!(header_edit(&text, Lang::Rust, &p, &vars(2026)).unwrap().change, Change::Added);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn a_real_source_file_gets_one_header_then_keeps_it() {
        let p = profile(CopyrightProfile::DEFAULT_TEMPLATE, "MIT");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.rs");
        std::fs::write(&path, include_str!("main.rs")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let e = header_edit(&text, Lang::from_path(&path), &p, &vars(2024)).unwrap();
        assert_eq!(e.change, Change::Added);
        let once = apply(&text, &e);
        assert!(once.starts_with("// Copyright (c) 2024 Ada Lovelace\n// Part of demo.\n// SPDX-License-Identifier: MIT\n\n"));
        assert!(once.ends_with(&text));
        // A year later only the year changes.
        let twice = apply(&once, &header_edit(&once, Lang::Rust, &p, &vars(2026)).unwrap());
        assert_eq!(twice, once.replacen("2024", "2024-2026", 1));
        assert_eq!(header_edit(&twice, Lang::Rust, &p, &vars(2026)).unwrap().change, Change::Current);
    }

    #[test]
    fn current_year_is_plausible() {
        assert!(current_year() >= 2025);
    }
}
