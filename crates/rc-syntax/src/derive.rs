//! List attributes of the Rust item at the caret: `#[derive(...)]` of a type and `#[allow(...)]`
//! of a statement or item. What they list now, and the edit that writes a new list (in the
//! attribute, or in a new attribute above the item).

use rc_text::Rope;
use tree_sitter::{Node, Tree};

/// The traits most types derive, in the order they are usually written.
pub const COMMON_DERIVES: &[&str] = &["Debug", "Clone", "Copy", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash", "Default"];

const TYPES: &[&str] = &["struct_item", "enum_item", "union_item"];
/// What `#[allow]` can go on: statements and items, innermost first.
const ALLOWABLE: &[&str] = &[
    "let_declaration",
    "function_item",
    "struct_item",
    "enum_item",
    "union_item",
    "impl_item",
    "trait_item",
    "mod_item",
    "const_item",
    "static_item",
    "type_item",
    "macro_definition",
];

/// An item and one of its list attributes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttrList {
    /// The item's name (empty for a statement).
    pub name: String,
    /// What the attribute lists now (paths as written, like `serde::Serialize`).
    pub items: Vec<String>,
    attr: &'static str,
    /// Char range of the list between its parentheses, when there is an attribute.
    list: Option<(usize, usize)>,
    /// Char offset of the start of the item's line, where a new attribute goes.
    line_start: usize,
    /// The item's indentation.
    indent: String,
}

/// The `#[derive]` of a struct, enum or union.
pub type DeriveTarget = AttrList;

/// The type the caret at char `pos` is in (on its name, attributes, fields or variants).
pub fn derive_target(tree: &Tree, rope: &Rope, pos: usize) -> Option<DeriveTarget> {
    // Not inside another item's body (a struct defined in a function's block is fine).
    target(tree, rope, pos, "derive", TYPES, &["function_item", "impl_item", "trait_item", "mod_item"])
}

/// The `#[allow]` of the innermost statement or item around char `pos`.
pub fn allow_target(tree: &Tree, rope: &Rope, pos: usize) -> Option<AttrList> {
    target(tree, rope, pos, "allow", ALLOWABLE, &[])
}

fn target(tree: &Tree, rope: &Rope, pos: usize, attr: &'static str, kinds: &[&str], stop: &[&str]) -> Option<AttrList> {
    let byte = rope.try_char_to_byte(pos.min(rope.len_chars())).ok()?;
    let mut node = tree.root_node().descendant_for_byte_range(byte, byte)?;
    // On one of the item's own attributes: the item follows them.
    let item = loop {
        match node.kind() {
            k if kinds.contains(&k) => break node,
            "attribute_item" => break following_item(node, kinds)?,
            "source_file" => return None,
            k if stop.contains(&k) => return None,
            _ => node = node.parent()?,
        }
    };
    let text = |n: Node| rope.byte_slice(n.start_byte()..n.end_byte()).to_string();
    let name = item.child_by_field_name("name").map(text).unwrap_or_default();
    let mut items = Vec::new();
    let mut list = None;
    let mut prev = item.prev_named_sibling();
    while let Some(p) = prev.filter(|p| matches!(p.kind(), "attribute_item" | "line_comment" | "block_comment")) {
        if let Some(args) = arguments(p, rope, attr) {
            let inner = (rope.byte_to_char(args.start_byte()) + 1, rope.byte_to_char(args.end_byte()) - 1);
            items = text(args).trim_start_matches('(').trim_end_matches(')').split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
            list = Some(inner);
            break;
        }
        prev = p.prev_named_sibling();
    }
    let line = item.start_position().row;
    let line_start = rope.line_to_char(line);
    let indent: String = rope.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    Some(AttrList { name, items, attr, list, line_start, indent })
}

/// The item an attribute belongs to: the next sibling that is not another attribute.
fn following_item<'t>(attr: Node<'t>, kinds: &[&str]) -> Option<Node<'t>> {
    let mut next = attr.next_named_sibling();
    while let Some(n) = next {
        match n.kind() {
            "attribute_item" | "line_comment" | "block_comment" => next = n.next_named_sibling(),
            k if kinds.contains(&k) => return Some(n),
            _ => return None,
        }
    }
    None
}

/// The `(...)` of `#[name(...)]`.
fn arguments<'t>(attr_item: Node<'t>, rope: &Rope, name: &str) -> Option<Node<'t>> {
    let attr = attr_item.named_child(0).filter(|a| a.kind() == "attribute")?;
    let path = attr.named_child(0).filter(|p| p.kind() == "identifier")?;
    if rope.byte_slice(path.start_byte()..path.end_byte()) != name {
        return None;
    }
    attr.child_by_field_name("arguments").filter(|a| a.kind() == "token_tree")
}

impl AttrList {
    /// The edit (char range and text) that makes the attribute list `items`, or `None` when it
    /// already lists exactly those. An empty list removes the list's contents.
    pub fn edit(&self, items: &[String]) -> Option<(usize, usize, String)> {
        if items == self.items.as_slice() {
            return None;
        }
        let joined = items.join(", ");
        match self.list {
            Some((a, b)) => Some((a, b, joined)),
            None if items.is_empty() => None,
            None => Some((self.line_start, self.line_start, format!("{}#[{}({joined})]\n", self.indent, self.attr))),
        }
    }

    /// The edit that adds `item` to the list (none when it is there already).
    pub fn add(&self, item: &str) -> Option<(usize, usize, String)> {
        if self.items.iter().any(|i| i == item) {
            return None;
        }
        let mut items = self.items.clone();
        items.push(item.to_string());
        self.edit(&items)
    }
}

/// The edit that allows `lint` in the whole file: `#![allow(lint)]` on the first line.
pub fn allow_in_file(lint: &str) -> (usize, usize, String) {
    (0, 0, format!("#![allow({lint})]\n"))
}

/// Whether a diagnostic code names a lint `#[allow]` takes (`unused_variables`,
/// `clippy::len_zero`), not one of rust-analyzer's own checks (`unresolved-import`).
pub fn is_lint_name(code: &str) -> bool {
    !code.is_empty() && code.split("::").all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Lang, parse_now};

    fn target(src: &str, at: &str) -> Option<DeriveTarget> {
        let rope = Rope::from_str(src);
        let tree = parse_now(Lang::Rust, &rope, None).unwrap();
        derive_target(&tree, &rope, src.find(at).unwrap() + 1)
    }

    fn apply(src: &str, t: &DeriveTarget, traits: &[&str]) -> String {
        let traits: Vec<String> = traits.iter().map(|s| s.to_string()).collect();
        let (a, b, text) = t.edit(&traits).unwrap();
        let mut rope = Rope::from_str(src);
        rope.remove(a..b);
        rope.insert(a, &text);
        rope.to_string()
    }

    #[test]
    fn a_type_without_derive_gets_the_attribute_above_it_with_its_indent() {
        let src = "mod m {\n    /// Docs.\n    pub struct Point {\n        x: i32,\n    }\n}\n";
        let t = target(src, "x: i32").unwrap();
        assert_eq!((t.name.as_str(), t.items.len()), ("Point", 0));
        assert_eq!(apply(src, &t, &["Debug", "Clone"]), "mod m {\n    /// Docs.\n    #[derive(Debug, Clone)]\n    pub struct Point {\n        x: i32,\n    }\n}\n");
    }

    #[test]
    fn an_existing_derive_list_is_rewritten_in_place() {
        let src = "#[derive(Debug, serde::Serialize)]\n#[allow(dead_code)]\nenum Kind {\n    A,\n}\n";
        for at in ["enum Kind", "A,", "allow"] {
            let t = target(src, at).unwrap();
            assert_eq!(t.items, vec!["Debug", "serde::Serialize"], "{at}");
        }
        let t = target(src, "Kind").unwrap();
        assert_eq!(apply(src, &t, &["Debug", "serde::Serialize", "Clone"]), src.replace("Debug, serde::Serialize", "Debug, serde::Serialize, Clone"));
        assert_eq!(t.edit(&["Debug".into(), "serde::Serialize".into()]), None);
    }

    #[test]
    fn allow_goes_on_the_innermost_statement_or_item_and_merges() {
        let src = "fn main() {\n    let x = 1;\n}\n#[allow(dead_code)]\nstruct S;\n";
        let rope = Rope::from_str(src);
        let tree = parse_now(Lang::Rust, &rope, None).unwrap();
        let t = allow_target(&tree, &rope, src.find("x = 1").unwrap()).unwrap();
        assert_eq!(t.name, "");
        let at = src.find("    let").unwrap();
        assert_eq!(t.add("unused_variables"), Some((at, at, "    #[allow(unused_variables)]\n".to_string())));
        let t = allow_target(&tree, &rope, src.find("S;").unwrap()).unwrap();
        assert_eq!((t.name.as_str(), t.items.clone()), ("S", vec!["dead_code".to_string()]));
        assert_eq!(t.add("dead_code"), None);
        let (a, b, text) = t.add("clippy::all").unwrap();
        let mut edited = rope.clone();
        edited.remove(a..b);
        edited.insert(a, &text);
        assert!(edited.to_string().contains("#[allow(dead_code, clippy::all)]\nstruct S;"));
        assert_eq!(allow_in_file("unused"), (0, 0, "#![allow(unused)]\n".to_string()));
        assert!(is_lint_name("clippy::len_zero") && is_lint_name("unused_variables"));
        assert!(!is_lint_name("unresolved-import") && !is_lint_name("E0308") && !is_lint_name("clippy::"));
    }

    #[test]
    fn only_types_derive() {
        assert_eq!(target("fn main() {\n    let x = 1;\n}\n", "x = 1"), None);
        assert_eq!(target("struct A;\nimpl A {\n    fn f() {}\n}\n", "fn f"), None);
        // A type declared inside a function body still can.
        assert_eq!(target("fn main() {\n    struct Local(u8);\n}\n", "Local").map(|t| t.name), Some("Local".into()));
    }
}
