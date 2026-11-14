//! The declarations around a position (Show Context Info): the headers of the items, blocks and
//! closures that enclose it, outermost first, each collapsed to one line.

use crate::lang::Lang;
use crate::structure::{FoldKind, FoldRange};
use rc_text::Rope;
use tree_sitter::{Node, Tree};

/// Rust nodes whose header is context.
const RUST_SCOPES: &[&str] =
    &["mod_item", "impl_item", "trait_item", "function_item", "struct_item", "enum_item", "union_item", "macro_definition", "closure_expression"];

/// A header longer than this many source lines keeps only its first line.
const MAX_LINES: usize = 6;
/// Headers are cut to this many chars.
const MAX_CHARS: usize = 160;

/// A piece of source text shown in a header: `line`, char columns `start_col..end_col`, shown
/// at char `at` of [`ContextHeader::text`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderPiece {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub at: usize,
}

/// One enclosing declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextHeader {
    /// Line the header starts on (where a click goes).
    pub line: usize,
    /// The header on one line; ends with `…` when cut.
    pub text: String,
    /// Where each part of `text` comes from (separators and `…` are not source text).
    pub pieces: Vec<HeaderPiece>,
}

/// The headers of the declarations enclosing char `pos` that start on an earlier line,
/// outermost first. Rust uses the tree; other languages (or Rust before its first parse) use
/// the block fold ranges.
pub fn context_headers(lang: Lang, tree: Option<&Tree>, folds: &[FoldRange], rope: &Rope, pos: usize) -> Vec<ContextHeader> {
    let pos = pos.min(rope.len_chars());
    match tree {
        Some(tree) if lang == Lang::Rust => rust_headers(tree, rope, pos),
        _ => fold_headers(folds, rope, pos),
    }
}

fn rust_headers(tree: &Tree, rope: &Rope, pos: usize) -> Vec<ContextHeader> {
    let caret_line = rope.char_to_line(pos);
    let Ok(byte) = rope.try_char_to_byte(pos) else { return Vec::new() };
    let Some(mut node) = tree.root_node().descendant_for_byte_range(byte, byte) else { return Vec::new() };
    let mut out: Vec<ContextHeader> = Vec::new();
    loop {
        if RUST_SCOPES.contains(&node.kind()) && node.start_position().row < caret_line && node.end_byte() <= rope.len_bytes() {
            let (start, end) = header_bytes(node, rope);
            if let Some(h) = header_from(rope, start, end) {
                // A closure on the line of an outer header (or another closure) adds nothing.
                match out.last_mut() {
                    Some(last) if last.line == h.line => *last = h,
                    _ => out.push(h),
                }
            }
        }
        match node.parent() {
            Some(p) => node = p,
            None => break,
        }
    }
    out.reverse();
    out
}

/// Byte range of a node's header: from its start (a closure: its line's first non-blank char)
/// to its body.
fn header_bytes(node: Node, rope: &Rope) -> (usize, usize) {
    let mut start = node.start_byte();
    if node.kind() == "closure_expression" {
        let line = node.start_position().row;
        let line_start = rope.line_to_byte(line);
        let indent = rope.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').count();
        start = line_start + indent;
    }
    let body = node.child_by_field_name("body").or_else(|| {
        // `macro_rules! m { … }`: the rules start at the first bracket.
        let mut cursor = node.walk();
        node.children(&mut cursor).find(|c| matches!(c.kind(), "{" | "(" | "["))
    });
    let end = body.map_or(node.end_byte(), |b| b.start_byte());
    (start, end.max(start))
}

/// The header for bytes `start..end`, collapsed to one line.
fn header_from(rope: &Rope, start: usize, end: usize) -> Option<ContextHeader> {
    let end = end.min(rope.len_bytes());
    let start = start.min(end);
    let (cs, ce) = (rope.byte_to_char(start), rope.byte_to_char(end));
    let (first, last) = (rope.char_to_line(cs), rope.char_to_line(ce));
    let mut raw: Vec<(usize, usize, usize)> = Vec::new();
    for line in first..=last {
        let line_start = rope.line_to_char(line);
        let line_len = line_len(rope, line);
        let a = if line == first { cs - line_start } else { 0 };
        let b = if line == last { (ce - line_start).min(line_len) } else { line_len };
        if let Some(r) = trimmed(rope, line, a, b) {
            raw.push(r);
        }
    }
    if raw.len() > MAX_LINES {
        raw.truncate(1);
        return Some(build(rope, first, &raw, true));
    }
    (!raw.is_empty()).then(|| build(rope, first, &raw, false))
}

/// Columns `a..b` of `line` without surrounding blanks or a trailing line comment.
fn trimmed(rope: &Rope, line: usize, a: usize, b: usize) -> Option<(usize, usize, usize)> {
    let chars: Vec<char> = rope.line(line).chars().collect();
    let b = b.min(chars.len());
    let mut a = a.min(b);
    let mut b = b;
    // `// …` to the end of the piece is not part of the signature.
    if let Some(k) = (a..b.saturating_sub(1)).find(|&k| chars[k] == '/' && chars[k + 1] == '/') {
        b = k;
    }
    while a < b && chars[a].is_whitespace() {
        a += 1;
    }
    while b > a && chars[b - 1].is_whitespace() {
        b -= 1;
    }
    (a < b).then_some((line, a, b))
}

/// Joins the pieces: no space inside brackets, and a trailing comma before a closing bracket
/// goes (`fn f(\n    a: u8,\n)` → `fn f(a: u8)`).
fn build(rope: &Rope, first: usize, raw: &[(usize, usize, usize)], cut: bool) -> ContextHeader {
    let piece_text = |(l, a, b): (usize, usize, usize)| -> String { rope.line(l).chars().skip(a).take(b - a).collect() };
    let mut parts: Vec<((usize, usize, usize), String)> = raw.iter().map(|&r| (r, piece_text(r))).collect();
    for i in 1..parts.len() {
        if parts[i].1.starts_with([')', ']', '>', '}']) && parts[i - 1].1.ends_with(',') {
            let prev = &mut parts[i - 1];
            prev.1.pop();
            prev.0.2 -= 1;
        }
    }
    // `where T: Debug,` before the body.
    if let Some(last) = parts.last_mut().filter(|p| p.1.ends_with(',') && p.1.len() > 1) {
        last.1.pop();
        last.0.2 -= 1;
    }
    let mut text = String::new();
    let mut len = 0usize;
    let mut pieces = Vec::new();
    let mut cut = cut;
    for (i, ((line, a, b), t)) in parts.iter().enumerate() {
        if i > 0 {
            let prev = &parts[i - 1].1;
            let glue = !(prev.ends_with(['(', '[', '<']) || t.starts_with([')', ']', '>', ',', '.', '?', ';']));
            if glue && !prev.is_empty() {
                text.push(' ');
                len += 1;
            }
        }
        let room = MAX_CHARS.saturating_sub(len);
        let n = b - a;
        if n > room {
            text.extend(t.chars().take(room));
            if room > 0 {
                pieces.push(HeaderPiece { line: *line, start_col: *a, end_col: a + room, at: len });
            }
            cut = true;
            break;
        }
        pieces.push(HeaderPiece { line: *line, start_col: *a, end_col: *b, at: len });
        text.push_str(t);
        len += n;
    }
    if cut {
        text.push_str(" …");
    }
    ContextHeader { line: first, text, pieces }
}

/// Other languages: the line each enclosing block starts on (or the one before, when the block
/// opens on a line of its own), outermost first.
fn fold_headers(folds: &[FoldRange], rope: &Rope, pos: usize) -> Vec<ContextHeader> {
    let caret_line = rope.char_to_line(pos);
    let mut blocks: Vec<&FoldRange> = folds
        .iter()
        .filter(|f| f.kind == FoldKind::Block && f.start_line < caret_line && caret_line <= f.end_line && f.start_line < rope.len_lines())
        .collect();
    blocks.sort_by_key(|f| (f.start_line, std::cmp::Reverse(f.end_line)));
    let mut out: Vec<ContextHeader> = Vec::new();
    for f in blocks {
        let before = trimmed(rope, f.start_line, 0, f.start_col.max(1).min(line_len(rope, f.start_line)));
        let text_of = |r: (usize, usize, usize)| -> String { rope.line(r.0).chars().skip(r.1).take(r.2 - r.1).collect() };
        // A brace alone on its line: the declaration is on the line above.
        let piece = match before.filter(|r| !matches!(text_of(*r).as_str(), "{" | "(" | "[")) {
            Some(r) => Some(r),
            None => (0..f.start_line).rev().find_map(|l| trimmed(rope, l, 0, line_len(rope, l))),
        };
        let Some(piece) = piece else { continue };
        if out.iter().any(|h| h.line == piece.0) {
            continue;
        }
        out.push(build(rope, piece.0, &[piece], false));
    }
    out
}

/// Chars of `line` without its line break.
fn line_len(rope: &Rope, line: usize) -> usize {
    rope.line(line).chars().filter(|c| *c != '\n' && *c != '\r').count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{folding_ranges, parse_now};

    fn headers(lang: Lang, src: &str, at: &str) -> Vec<(usize, String)> {
        let rope = Rope::from_str(src);
        let tree = parse_now(lang, &rope, None);
        let folds = tree.as_ref().map(|t| folding_ranges(lang, t, &rope)).unwrap_or_default();
        let pos = rope.byte_to_char(src.find(at).unwrap());
        context_headers(lang, tree.as_ref(), &folds, &rope, pos).into_iter().map(|h| (h.line, h.text)).collect()
    }

    #[test]
    fn a_method_in_an_impl_shows_both_headers_outermost_first() {
        let src = "mod m {\n    impl<T> Foo<T> for Bar {\n        /// Docs.\n        #[inline]\n        pub fn compute(&self, x: u32) -> u32 {\n            let y = x;\n            y\n        }\n    }\n}\n";
        assert_eq!(
            headers(Lang::Rust, src, "let y"),
            vec![(0, "mod m".to_string()), (1, "impl<T> Foo<T> for Bar".to_string()), (4, "pub fn compute(&self, x: u32) -> u32".to_string())]
        );
        // On a header's own line, that header is visible: only the outer ones count.
        assert_eq!(headers(Lang::Rust, src, "compute").len(), 2);
        assert!(headers(Lang::Rust, src, "mod m").is_empty());
    }

    #[test]
    fn multi_line_signatures_collapse_to_one_line() {
        let src = "fn long(\n    a: u32, // the first\n    b: &str,\n) -> Result<(), E>\nwhere\n    E: Error,\n{\n    todo!()\n}\n";
        assert_eq!(headers(Lang::Rust, src, "todo"), vec![(0, "fn long(a: u32, b: &str) -> Result<(), E> where E: Error".to_string())]);
        let many = format!("fn many(\n{}) {{\n    body();\n}}\n", (0..10).map(|i| format!("    a{i}: u8,\n")).collect::<String>());
        assert_eq!(headers(Lang::Rust, &many, "body"), vec![(0, "fn many( …".to_string())]);
    }

    #[test]
    fn types_traits_macros_and_closures_are_context() {
        let src = "trait T {\n    fn f() {\n        let c = items.iter().map(|x| {\n            x + 1\n        });\n    }\n}\nstruct S {\n    a: u8,\n}\nenum E {\n    A,\n}\nmacro_rules! m {\n    () => {};\n}\n";
        assert_eq!(
            headers(Lang::Rust, src, "x + 1"),
            vec![(0, "trait T".to_string()), (1, "fn f()".to_string()), (2, "let c = items.iter().map(|x|".to_string())]
        );
        assert_eq!(headers(Lang::Rust, src, "a: u8"), vec![(7, "struct S".to_string())]);
        assert_eq!(headers(Lang::Rust, src, "A,"), vec![(10, "enum E".to_string())]);
        assert_eq!(headers(Lang::Rust, src, "() =>"), vec![(13, "macro_rules! m".to_string())]);
    }

    #[test]
    fn pieces_point_at_the_source_text() {
        let src = "impl A {\n    fn f(\n        x: u8,\n    ) {\n        g();\n    }\n}\n";
        let rope = Rope::from_str(src);
        let tree = parse_now(Lang::Rust, &rope, None).unwrap();
        let hs = context_headers(Lang::Rust, Some(&tree), &[], &rope, src.find("g()").unwrap());
        let f = &hs[1];
        assert_eq!(f.text, "fn f(x: u8)");
        for p in &f.pieces {
            let src_text: String = rope.line(p.line).chars().skip(p.start_col).take(p.end_col - p.start_col).collect();
            let shown: String = f.text.chars().skip(p.at).take(p.end_col - p.start_col).collect();
            assert_eq!(src_text, shown);
        }
    }

    #[test]
    fn other_languages_use_the_block_folds() {
        // Without a tree, Rust falls back to the folds too (none here: nothing to show).
        let rope = Rope::from_str("fn f() {\n    x();\n}\n");
        assert!(context_headers(Lang::Rust, None, &[], &rope, 12).is_empty());
        // Out-of-range positions are clamped.
        assert!(context_headers(Lang::Rust, None, &[], &rope, 10_000).is_empty());
    }
}
