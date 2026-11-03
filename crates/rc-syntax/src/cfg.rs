//! Code turned off by `#[cfg(...)]`: the items, statements, fields, variants and match arms
//! whose `cfg` is false for the configuration in force, so the editor can fade them.
//!
//! The configuration comes from `rustc --print cfg` (the target) plus the enabled features. Like
//! rust-analyzer, `test` and `debug_assertions` count as set for the project's own code.

use rc_text::Rope;
use std::collections::BTreeSet;
use tree_sitter::{Node, Tree};

/// The names and `key = "value"` pairs that are set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CfgSet {
    pub names: BTreeSet<String>,
    pub pairs: BTreeSet<(String, String)>,
}

impl CfgSet {
    /// Reads the output of `rustc --print cfg` (`unix`, `target_os="linux"`, one per line), adds
    /// `test` and `debug_assertions`, and `feature = "…"` for each of `features`.
    pub fn from_rustc(text: &str, features: &[String]) -> CfgSet {
        let mut set = CfgSet::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            match line.split_once('=') {
                Some((k, v)) => {
                    set.pairs.insert((k.trim().to_string(), v.trim().trim_matches('"').to_string()));
                }
                None => {
                    set.names.insert(line.to_string());
                }
            }
        }
        set.names.insert("test".into());
        set.names.insert("debug_assertions".into());
        for f in features {
            set.pairs.insert(("feature".into(), f.clone()));
        }
        set
    }

    /// Whether the predicate holds (`unix`, `feature = "x"`, `all(…)`, `any(…)`, `not(…)`).
    /// `None` when it cannot be read: such code is never faded.
    pub fn eval(&self, predicate: &str) -> Option<bool> {
        let tokens = tokenize(predicate)?;
        let mut pos = 0;
        let value = self.eval_at(&tokens, &mut pos)?;
        (pos == tokens.len()).then_some(value)
    }

    fn eval_at(&self, t: &[Tok], pos: &mut usize) -> Option<bool> {
        let Tok::Ident(name) = t.get(*pos)? else { return None };
        *pos += 1;
        match t.get(*pos) {
            Some(Tok::Open) if matches!(name.as_str(), "all" | "any" | "not") => {
                *pos += 1;
                let mut values = Vec::new();
                while t.get(*pos) != Some(&Tok::Close) {
                    values.push(self.eval_at(t, pos)?);
                    match t.get(*pos)? {
                        Tok::Comma => *pos += 1,
                        Tok::Close => {}
                        _ => return None,
                    }
                }
                *pos += 1;
                match name.as_str() {
                    "all" => Some(values.iter().all(|v| *v)),
                    "any" => Some(values.iter().any(|v| *v)),
                    _ => (values.len() == 1).then(|| !values[0]),
                }
            }
            Some(Tok::Eq) => {
                *pos += 1;
                let Tok::Str(value) = t.get(*pos)? else { return None };
                *pos += 1;
                Some(self.pairs.contains(&(name.clone(), value.clone())))
            }
            _ => Some(self.names.contains(name)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Str(String),
    Open,
    Close,
    Comma,
    Eq,
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' | ')' | ',' | '=' => {
                chars.next();
                out.push(match c {
                    '(' => Tok::Open,
                    ')' => Tok::Close,
                    ',' => Tok::Comma,
                    _ => Tok::Eq,
                });
            }
            '"' => {
                chars.next();
                let mut text = String::new();
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => text.push(chars.next()?),
                        c => text.push(c),
                    }
                }
                out.push(Tok::Str(text));
            }
            c if c.is_alphanumeric() || c == '_' => {
                let mut text = String::new();
                while let Some(&c) = chars.peek().filter(|c| c.is_alphanumeric() || **c == '_') {
                    text.push(c);
                    chars.next();
                }
                out.push(Tok::Ident(text));
            }
            _ => return None,
        }
    }
    Some(out)
}

/// A stretch of code a `cfg` turns off, as char offsets, with the predicate as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inactive {
    pub start: usize,
    pub end: usize,
    pub predicate: String,
}

/// Every stretch of the file that a false `#[cfg(...)]` turns off, outermost only.
pub fn inactive_ranges(tree: &Tree, rope: &Rope, set: &CfgSet) -> Vec<Inactive> {
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "attribute_item"
            && let Some(predicate) = cfg_predicate(node, rope)
            && set.eval(&predicate) == Some(false)
            && let Some(end) = owner_end(node)
        {
            let start = rope.byte_to_char(node.start_byte());
            let end = rope.byte_to_char(end);
            if !out.iter().any(|r: &Inactive| r.start <= start && r.end >= end) {
                out.push(Inactive { start, end, predicate });
            }
            continue;
        }
        let children: Vec<Node> = node.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    out.sort_by_key(|r| r.start);
    // A disabled item can contain other `cfg`s: keep the outer one.
    let mut kept: Vec<Inactive> = Vec::new();
    for r in out {
        if !kept.last().is_some_and(|k| k.end >= r.end) {
            kept.push(r);
        }
    }
    kept
}

/// The predicate of `#[cfg(...)]`.
fn cfg_predicate(attr_item: Node, rope: &Rope) -> Option<String> {
    let attr = attr_item.named_child(0).filter(|a| a.kind() == "attribute")?;
    let path = attr.named_child(0).filter(|p| p.kind() == "identifier")?;
    if rope.byte_slice(path.start_byte()..path.end_byte()) != "cfg" {
        return None;
    }
    let args = attr.child_by_field_name("arguments").filter(|a| a.kind() == "token_tree")?;
    let text = rope.byte_slice(args.start_byte()..args.end_byte()).to_string();
    Some(text.strip_prefix('(')?.strip_suffix(')')?.trim().to_string())
}

/// The end byte of what an attribute applies to: a match arm it is inside, or the next sibling
/// that is not an attribute or a comment.
fn owner_end(attr: Node) -> Option<usize> {
    if let Some(parent) = attr.parent().filter(|p| p.kind() == "match_arm") {
        return Some(parent.end_byte());
    }
    let mut next = attr.next_named_sibling();
    while let Some(n) = next {
        match n.kind() {
            "attribute_item" | "line_comment" | "block_comment" => next = n.next_named_sibling(),
            _ => {
                // A statement ends at its `;`, which tree-sitter leaves outside some of them.
                let end = n.next_sibling().filter(|s| s.kind() == ";").map_or(n.end_byte(), |s| s.end_byte());
                return Some(end);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Lang;

    fn set() -> CfgSet {
        CfgSet::from_rustc("unix\ntarget_os=\"linux\"\ntarget_pointer_width=\"64\"\n", &["std".to_string()])
    }

    #[test]
    fn predicates() {
        let s = set();
        assert_eq!(s.eval("unix"), Some(true));
        assert_eq!(s.eval("windows"), Some(false));
        assert_eq!(s.eval("target_os = \"linux\""), Some(true));
        assert_eq!(s.eval("feature = \"extra\""), Some(false));
        assert_eq!(s.eval("feature = \"std\""), Some(true));
        assert_eq!(s.eval("all(unix, not(windows))"), Some(true));
        assert_eq!(s.eval("any(windows, target_os = \"macos\")"), Some(false));
        assert_eq!(s.eval("any()"), Some(false));
        assert_eq!(s.eval("all()"), Some(true));
        assert_eq!(s.eval("test"), Some(true));
        assert_eq!(s.eval("not(a, b)"), None);
        assert_eq!(s.eval("unix windows"), None);
        assert_eq!(s.eval("feature ="), None);
        assert_eq!(s.eval("\"open"), None);
        assert_eq!(s.eval("$"), None);
    }

    #[test]
    fn disabled_code_is_found() {
        let src = "#[cfg(feature = \"extra\")]\npub fn extra() {}\n\n#[cfg(unix)]\nfn on() {}\n\nstruct S {\n    #[cfg(windows)]\n    handle: u32,\n    keep: u8,\n}\n\nfn f(x: u8) {\n    #[cfg(windows)]\n    let y = 1;\n    match x {\n        #[cfg(windows)]\n        0 => {}\n        _ => {}\n    }\n}\n\n#[cfg(windows)]\nmod win {\n    #[cfg(unix)]\n    fn inner() {}\n}\n";
        let rope = Rope::from_str(src);
        let tree = crate::parse::parse_now(Lang::Rust, &rope, None).unwrap();
        let got: Vec<String> = inactive_ranges(&tree, &rope, &set()).iter().map(|r| src.chars().skip(r.start).take(r.end - r.start).collect()).collect();
        assert_eq!(
            got,
            vec![
                "#[cfg(feature = \"extra\")]\npub fn extra() {}".to_string(),
                "#[cfg(windows)]\n    handle: u32".to_string(),
                "#[cfg(windows)]\n    let y = 1;".to_string(),
                "#[cfg(windows)]\n        0 => {}".to_string(),
                "#[cfg(windows)]\nmod win {\n    #[cfg(unix)]\n    fn inner() {}\n}".to_string(),
            ]
        );
    }
}
