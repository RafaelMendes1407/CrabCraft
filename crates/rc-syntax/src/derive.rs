//! `#[derive(...)]` of the Rust type at the caret: the traits it already derives, and the edit
//! that makes it derive a new list (merged into the attribute, or a new attribute above the item).

use rc_text::Rope;
use tree_sitter::{Node, Tree};

/// The traits most types derive, in the order they are usually written.
pub const COMMON_DERIVES: &[&str] = &["Debug", "Clone", "Copy", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash", "Default"];

/// A struct, enum or union and its derive attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeriveTarget {
    /// The type's name.
    pub name: String,
    /// What it derives now (paths as written, like `serde::Serialize`).
    pub traits: Vec<String>,
    /// Char range of the derive list between its parentheses, when there is an attribute.
    list: Option<(usize, usize)>,
    /// Char offset of the start of the item's line, where a new attribute goes.
    line_start: usize,
    /// The item's indentation.
    indent: String,
}

/// The type the caret at char `pos` is in (on its name, attributes, fields or variants).
pub fn derive_target(tree: &Tree, rope: &Rope, pos: usize) -> Option<DeriveTarget> {
    let byte = rope.try_char_to_byte(pos.min(rope.len_chars())).ok()?;
    let mut node = tree.root_node().descendant_for_byte_range(byte, byte)?;
    // On one of the type's own attributes: the item follows them.
    let item = loop {
        match node.kind() {
            "struct_item" | "enum_item" | "union_item" => break node,
            "attribute_item" => break following_item(node)?,
            // Not inside another item's body (a struct defined in a function's block is fine).
            "function_item" | "impl_item" | "trait_item" | "mod_item" | "source_file" => return None,
            _ => node = node.parent()?,
        }
    };
    let text = |n: Node| rope.byte_slice(n.start_byte()..n.end_byte()).to_string();
    let name = text(item.child_by_field_name("name")?);
    let mut traits = Vec::new();
    let mut list = None;
    let mut prev = item.prev_named_sibling();
    while let Some(p) = prev.filter(|p| matches!(p.kind(), "attribute_item" | "line_comment" | "block_comment")) {
        if let Some(args) = derive_arguments(p, rope) {
            let inner = (rope.byte_to_char(args.start_byte()) + 1, rope.byte_to_char(args.end_byte()) - 1);
            traits = text(args).trim_start_matches('(').trim_end_matches(')').split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
            list = Some(inner);
            break;
        }
        prev = p.prev_named_sibling();
    }
    let line = item.start_position().row;
    let line_start = rope.line_to_char(line);
    let indent: String = rope.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    Some(DeriveTarget { name, traits, list, line_start, indent })
}

/// The item an attribute belongs to: the next sibling that is not another attribute.
fn following_item(attr: Node) -> Option<Node> {
    let mut next = attr.next_named_sibling();
    while let Some(n) = next {
        match n.kind() {
            "attribute_item" | "line_comment" | "block_comment" => next = n.next_named_sibling(),
            "struct_item" | "enum_item" | "union_item" => return Some(n),
            _ => return None,
        }
    }
    None
}

/// The `(...)` of `#[derive(...)]`.
fn derive_arguments<'t>(attr_item: Node<'t>, rope: &Rope) -> Option<Node<'t>> {
    let attr = attr_item.named_child(0).filter(|a| a.kind() == "attribute")?;
    let path = attr.named_child(0).filter(|p| p.kind() == "identifier")?;
    if rope.byte_slice(path.start_byte()..path.end_byte()) != "derive" {
        return None;
    }
    attr.child_by_field_name("arguments").filter(|a| a.kind() == "token_tree")
}

impl DeriveTarget {
    /// The edit (char range and text) that makes the type derive `traits`, or `None` when it
    /// already derives exactly those. An empty list removes the derive list's contents.
    pub fn edit(&self, traits: &[String]) -> Option<(usize, usize, String)> {
        if traits == self.traits.as_slice() {
            return None;
        }
        let joined = traits.join(", ");
        match self.list {
            Some((a, b)) => Some((a, b, joined)),
            None if traits.is_empty() => None,
            None => Some((self.line_start, self.line_start, format!("{}#[derive({joined})]\n", self.indent))),
        }
    }
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
        assert_eq!((t.name.as_str(), t.traits.len()), ("Point", 0));
        assert_eq!(apply(src, &t, &["Debug", "Clone"]), "mod m {\n    /// Docs.\n    #[derive(Debug, Clone)]\n    pub struct Point {\n        x: i32,\n    }\n}\n");
    }

    #[test]
    fn an_existing_derive_list_is_rewritten_in_place() {
        let src = "#[derive(Debug, serde::Serialize)]\n#[allow(dead_code)]\nenum Kind {\n    A,\n}\n";
        for at in ["enum Kind", "A,", "allow"] {
            let t = target(src, at).unwrap();
            assert_eq!(t.traits, vec!["Debug", "serde::Serialize"], "{at}");
        }
        let t = target(src, "Kind").unwrap();
        assert_eq!(apply(src, &t, &["Debug", "serde::Serialize", "Clone"]), src.replace("Debug, serde::Serialize", "Debug, serde::Serialize, Clone"));
        assert_eq!(t.edit(&["Debug".into(), "serde::Serialize".into()]), None);
    }

    #[test]
    fn only_types_derive() {
        assert_eq!(target("fn main() {\n    let x = 1;\n}\n", "x = 1"), None);
        assert_eq!(target("struct A;\nimpl A {\n    fn f() {}\n}\n", "fn f"), None);
        // A type declared inside a function body still can.
        assert_eq!(target("fn main() {\n    struct Local(u8);\n}\n", "Local").map(|t| t.name), Some("Local".into()));
    }
}
