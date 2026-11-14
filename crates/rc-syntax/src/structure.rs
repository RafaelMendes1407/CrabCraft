//! Structure from the tree: folding ranges and bracket matching.

use crate::lang::Lang;
use rc_text::Rope;
use tree_sitter::{Node, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    Block,
    Comment,
    Imports,
    Region,
}

/// A foldable range: lines `start_line + 1 ..= end_line` hide behind `start_line`
/// (for braces, the closing line stays visible: `end_line` is the line before it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRange {
    pub start_line: usize,
    pub end_line: usize,
    pub kind: FoldKind,
    /// Char column on `start_line` where the placeholder (`{...}`) starts.
    pub start_col: usize,
    /// Char column on `end_line` right after the folded text (what follows stays visible
    /// after the placeholder, e.g. the `);` of `});`).
    pub end_col: usize,
}

/// Rust nodes that fold on their own (the body of an item, a match, a macro call...).
const RUST_BLOCKS: &[&str] =
    &["block", "declaration_list", "field_declaration_list", "enum_variant_list", "match_block", "token_tree", "use_list", "ordered_field_declaration_list"];

pub fn folding_ranges(lang: Lang, tree: &Tree, rope: &Rope) -> Vec<FoldRange> {
    let mut out = Vec::new();
    match lang {
        Lang::Rust => {
            let mut comments: Option<(usize, usize, usize)> = None;
            let mut lines: Vec<(usize, usize, usize)> = Vec::new();
            walk(tree.root_node(), &mut |node| {
                let kind = node.kind();
                let (sr, er) = (node.start_position().row, node.end_position().row);
                if RUST_BLOCKS.contains(&kind) && er > sr {
                    out.push(FoldRange { start_line: sr, end_line: er, kind: FoldKind::Block, start_col: char_col(rope, node), end_col: end_col(rope, node) });
                } else if kind == "block_comment" && er > sr {
                    out.push(FoldRange {
                        start_line: sr,
                        end_line: er,
                        kind: FoldKind::Comment,
                        start_col: char_col(rope, node),
                        end_col: end_col(rope, node),
                    });
                } else if kind == "line_comment" {
                    lines.push((sr, er, char_col(rope, node)));
                }
            });
            // Adjacent line comments (doc comments above an item, for instance) fold as one.
            lines.sort_unstable();
            let flush = |run: Option<(usize, usize, usize)>, out: &mut Vec<FoldRange>| {
                if let Some((first, last, col)) = run.filter(|(first, last, _)| last > first) {
                    out.push(FoldRange { start_line: first, end_line: last, kind: FoldKind::Comment, start_col: col, end_col: line_end(rope, last) });
                }
            };
            for (sr, _, col) in lines {
                comments = match comments {
                    Some((first, last, c)) if sr == last + 1 => Some((first, sr, c)),
                    run => {
                        flush(run, &mut out);
                        Some((sr, sr, col))
                    }
                };
            }
            flush(comments, &mut out);
        }
        Lang::Toml => walk(tree.root_node(), &mut |node| {
            let (sr, er) = (node.start_position().row, node.end_position().row);
            if matches!(node.kind(), "table" | "table_array_element" | "array" | "inline_table") && er > sr {
                // A table ends at the next one, past its blank lines: keep the fold on its content.
                let end = if node.end_position().column == 0 { er - 1 } else { er };
                let last = (sr..=end.min(rope.len_lines().saturating_sub(1))).rev().find(|l| rope.line(*l).chars().any(|c| !c.is_whitespace())).unwrap_or(sr);
                if last > sr {
                    out.push(FoldRange {
                        start_line: sr,
                        end_line: last,
                        kind: FoldKind::Block,
                        start_col: char_col(rope, node),
                        end_col: line_end(rope, last),
                    });
                }
            }
        }),
        Lang::Xml => walk(tree.root_node(), &mut |node| {
            let (sr, er) = (node.start_position().row, node.end_position().row);
            if (node.kind() == "element" || node.kind() == "Comment") && er > sr {
                out.push(FoldRange { start_line: sr, end_line: er, kind: FoldKind::Block, start_col: char_col(rope, node), end_col: end_col(rope, node) });
            }
        }),
        Lang::Json => walk(tree.root_node(), &mut |node| {
            let (sr, er) = (node.start_position().row, node.end_position().row);
            if (node.kind() == "object" || node.kind() == "array") && er > sr {
                out.push(FoldRange { start_line: sr, end_line: er, kind: FoldKind::Block, start_col: char_col(rope, node), end_col: end_col(rope, node) });
            }
        }),
        Lang::Markdown => walk(tree.root_node(), &mut |node| {
            let (sr, er) = (node.start_position().row, node.end_position().row);
            if (node.kind() == "section" || node.kind() == "fenced_code_block") && er > sr + 1 {
                let end = if node.end_position().column == 0 { er - 1 } else { er };
                if end > sr {
                    out.push(FoldRange { start_line: sr, end_line: end, kind: FoldKind::Block, start_col: line_end(rope, sr), end_col: line_end(rope, end) });
                }
            }
        }),
        Lang::Html | Lang::Css | Lang::JavaScript | Lang::TypeScript | Lang::Tsx => walk(tree.root_node(), &mut |node| {
            let (sr, er) = (node.start_position().row, node.end_position().row);
            let kind = node.kind();
            if er > sr
                && matches!(
                    kind,
                    "element"
                        | "script_element"
                        | "style_element"
                        | "block"
                        | "statement_block"
                        | "class_body"
                        | "object"
                        | "array"
                        | "interface_body"
                        | "enum_body"
                        | "comment"
                )
            {
                let kind = if kind == "comment" { FoldKind::Comment } else { FoldKind::Block };
                out.push(FoldRange { start_line: sr, end_line: er, kind, start_col: char_col(rope, node), end_col: end_col(rope, node) });
            }
        }),
        Lang::Http | Lang::Plain => {}
    }
    out.sort_by_key(|f| (f.start_line, std::cmp::Reverse(f.end_line)));
    // One fold per start line (the outermost), like IntelliJ.
    out.dedup_by_key(|f| f.start_line);
    out
}

fn line_end(rope: &Rope, line: usize) -> usize {
    rc_text::buffer::line_len(rope, line)
}

fn end_col(rope: &Rope, node: Node) -> usize {
    let p = node.end_position();
    let line_start = rope.line_to_byte(p.row.min(rope.len_lines().saturating_sub(1)));
    let byte = (line_start + p.column).min(rope.len_bytes());
    rope.byte_to_char(byte) - rope.line_to_char(rope.byte_to_line(byte))
}

fn char_col(rope: &Rope, node: Node) -> usize {
    let p = node.start_position();
    let line_start = rope.line_to_byte(p.row.min(rope.len_lines().saturating_sub(1)));
    let byte = (line_start + p.column).min(rope.len_bytes());
    rope.byte_to_char(byte) - rope.line_to_char(rope.byte_to_line(byte))
}

/// Pre-order walk of the nodes that may hold a fold: a node on a single line cannot
/// contain a multi-line one, so its children are skipped (the node itself is visited).
fn walk(root: Node, f: &mut impl FnMut(Node)) {
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        f(node);
        if node.end_position().row > node.start_position().row && cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

const BRACKETS: [(&str, &str); 4] = [("{", "}"), ("(", ")"), ("[", "]"), ("<", ">")];

/// The bracket matching the one at (or just before) char `pos`: returns both char offsets.
/// Uses the tree so brackets inside strings and comments are ignored.
pub fn matching_bracket(tree: &Tree, rope: &Rope, pos: usize) -> Option<(usize, usize)> {
    let try_at = |p: usize| -> Option<(usize, usize)> {
        if p >= rope.len_chars() {
            return None;
        }
        let byte = rope.char_to_byte(p);
        let node = tree.root_node().descendant_for_byte_range(byte, byte + 1)?;
        if node.is_named() || node.start_byte() != byte {
            return None;
        }
        let kind = node.kind();
        let (open, close) = BRACKETS.iter().find(|(o, c)| *o == kind || *c == kind)?;
        if *open == "<" && node.parent().is_none_or(|p| p.kind() != "type_argument_list" && p.kind() != "type_parameter_list") {
            return None;
        }
        let parent = node.parent()?;
        let mut cursor = parent.walk();
        let siblings: Vec<Node> = parent.children(&mut cursor).collect();
        let idx = siblings.iter().position(|n| n.id() == node.id())?;
        let other =
            if kind == *open { siblings[idx + 1..].iter().find(|n| n.kind() == *close)? } else { siblings[..idx].iter().rev().find(|n| n.kind() == *open)? };
        Some((p, rope.byte_to_char(other.start_byte())))
    };
    try_at(pos).or_else(|| pos.checked_sub(1).and_then(try_at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_now;

    #[test]
    fn rust_folds_items_matches_macros_and_comment_runs() {
        let src = "/// One\n/// Two\nstruct S {\n    a: u8,\n}\nfn f(x: u8) {\n    match x {\n        _ => {}\n    }\n    vec![\n        1,\n    ];\n}\n/* a\n b */\n// single\n";
        let rope = Rope::from_str(src);
        let tree = parse_now(Lang::Rust, &rope, None).unwrap();
        let folds = folding_ranges(Lang::Rust, &tree, &rope);
        let has = |s: usize, e: usize, k: FoldKind| folds.iter().any(|f| f.start_line == s && f.end_line == e && f.kind == k);
        assert!(has(0, 1, FoldKind::Comment), "{folds:?}");
        assert!(has(2, 4, FoldKind::Block), "{folds:?}");
        assert!(has(5, 12, FoldKind::Block), "{folds:?}");
        assert!(has(6, 8, FoldKind::Block), "{folds:?}");
        assert!(has(9, 11, FoldKind::Block), "{folds:?}");
        assert!(has(13, 14, FoldKind::Comment), "{folds:?}");
        assert!(!folds.iter().any(|f| f.start_line == 15), "{folds:?}");
    }

    #[test]
    fn toml_tables_fold_without_their_trailing_blank_lines() {
        let src = "[package]\nname = \"a\"\n\n[dependencies]\nx = \"1\"\ny = [\n  1,\n]\n";
        let rope = Rope::from_str(src);
        let tree = parse_now(Lang::Toml, &rope, None).unwrap();
        let folds = folding_ranges(Lang::Toml, &tree, &rope);
        assert!(folds.iter().any(|f| f.start_line == 0 && f.end_line == 1), "{folds:?}");
        assert!(folds.iter().any(|f| f.start_line == 3 && f.end_line == 7), "{folds:?}");
        assert!(folds.iter().any(|f| f.start_line == 5 && f.end_line == 7), "{folds:?}");
    }
}
