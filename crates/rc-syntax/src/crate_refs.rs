//! Conservative Rust crate-root references. Comments, strings and macro token trees are not
//! searched as text. Bindings are collected globally to prefer omission over guessing.
use crate::{Lang, parse_now};
use rc_text::Rope;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use tree_sitter::Node;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub root: String,
    pub start: usize,
    pub end: usize,
    pub test_only: bool,
}
#[derive(Clone, Debug, Default)]
pub struct References {
    pub references: Vec<Reference>,
    pub local_names: BTreeSet<String>,
    pub imported: BTreeMap<String, String>,
    pub glob: bool,
    pub limited: bool,
}
fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or("")
}
fn ident(value: &str) -> Option<String> {
    let value = value.strip_prefix("r#").unwrap_or(value);
    (!value.is_empty() && value.len() <= 128 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then(|| value.to_string())
}
fn root(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" | "type_identifier" | "self" | "super" | "crate" => ident(text(node, source)),
        "scoped_identifier" | "scoped_type_identifier" => {
            node.child_by_field_name("path").or_else(|| node.child_by_field_name("name")).and_then(|node| root(node, source))
        }
        "generic_type" | "generic_function" => {
            node.child_by_field_name("type").or_else(|| node.child_by_field_name("function")).and_then(|node| root(node, source))
        }
        _ => None,
    }
}
fn last(node: Node<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("name").and_then(|node| ident(text(node, source))).or_else(|| ident(text(node, source)))
}
fn test_attribute(node: Node<'_>, source: &str) -> bool {
    let mut previous = node.prev_named_sibling();
    for _ in 0..32 {
        let Some(item) = previous else { break };
        match item.kind() {
            "attribute_item" => {
                let compact: String = text(item, source).chars().filter(|ch| !ch.is_whitespace()).take(128).collect();
                if compact == "#[cfg(test)]" || compact == "#[test]" {
                    return true;
                }
            }
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        previous = item.prev_named_sibling();
    }
    false
}
impl References {
    fn binding(&mut self, alias: String, base: String) {
        if self.imported.get(&alias).is_some_and(|old| old != &base) {
            self.local_names.insert(alias.clone());
        }
        self.imported.insert(alias, base);
    }
    fn reference(&mut self, root: String, node: Node<'_>, test_only: bool) {
        if self.references.len() >= 256 {
            self.limited = true;
            return;
        }
        if !matches!(root.as_str(), "self" | "super" | "crate" | "Self" | "std" | "core" | "alloc") {
            self.references.push(Reference { root, start: node.start_byte(), end: node.end_byte(), test_only });
        }
    }
    fn import(&mut self, node: Node<'_>, source: &str, prefix: Option<(String, String)>, test_only: bool, budget: &mut usize, depth: usize) {
        if *budget == 0 || depth > 64 {
            self.limited = true;
            return;
        }
        *budget -= 1;
        match node.kind() {
            "use_list" => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    self.import(child, source, prefix.clone(), test_only, budget, depth + 1);
                }
            }
            "scoped_use_list" => {
                let path = node.child_by_field_name("path");
                let context =
                    path.and_then(|path| Some((prefix.as_ref().map(|p| p.0.clone()).or_else(|| root(path, source))?, last(path, source)?))).or(prefix);
                if let Some(list) = node.child_by_field_name("list") {
                    self.import(list, source, context, test_only, budget, depth + 1);
                }
            }
            "use_wildcard" => {
                self.glob = true;
                let mut cursor = node.walk();
                let path = node.named_children(&mut cursor).next();
                if let Some(base) = prefix.map(|p| p.0).or_else(|| path.and_then(|path| root(path, source))) {
                    self.reference(base, node, test_only);
                }
            }
            "use_as_clause" => {
                if let Some(path) = node.child_by_field_name("path")
                    && let Some(base) = prefix.map(|p| p.0).or_else(|| root(path, source))
                {
                    self.reference(base.clone(), path, test_only);
                    if let Some(alias) = node.child_by_field_name("alias").and_then(|alias| ident(text(alias, source))) {
                        self.binding(alias, base);
                    }
                }
            }
            _ => {
                let base = prefix.as_ref().map(|p| p.0.clone()).or_else(|| root(node, source));
                if let Some(base) = base {
                    self.reference(base.clone(), node, test_only);
                    let binding = if node.kind() == "self" { prefix.map(|p| p.1) } else { last(node, source) };
                    if let Some(binding) = binding {
                        self.binding(binding, base);
                    }
                }
            }
        }
    }
}

pub fn references(source: &str, cancel: &AtomicBool) -> References {
    let mut result = References::default();
    if source.len() > 1024 * 1024 || cancel.load(Ordering::Relaxed) {
        result.limited = true;
        return result;
    }
    let Some(tree) = parse_now(Lang::Rust, &Rope::from_str(source), None) else {
        return result;
    };
    let mut cursor = tree.root_node().walk();
    let test_crate = tree.root_node().named_children(&mut cursor).any(|node| {
        node.kind() == "inner_attribute_item" && text(node, source).chars().filter(|ch| !ch.is_whitespace()).take(128).collect::<String>() == "#![cfg(test)]"
    });
    let mut todo = vec![(tree.root_node(), test_crate)];
    let mut budget = 50_000_usize;
    while let Some((node, inherited_test)) = todo.pop() {
        if budget == 0 || cancel.load(Ordering::Relaxed) {
            result.limited = true;
            break;
        }
        budget -= 1;
        let test_only = inherited_test || test_attribute(node, source);
        if matches!(node.kind(), "mod_item" | "type_item" | "struct_item" | "enum_item" | "union_item" | "trait_item" | "type_parameter")
            && let Some(name) = node.child_by_field_name("name").and_then(|name| ident(text(name, source)))
        {
            result.local_names.insert(name);
        }
        match node.kind() {
            "use_declaration" => {
                if let Some(argument) = node.child_by_field_name("argument") {
                    result.import(argument, source, None, test_only, &mut budget, 0);
                }
                continue;
            }
            "extern_crate_declaration" => {
                if let Some(name) = node.child_by_field_name("name")
                    && let Some(base) = ident(text(name, source))
                {
                    result.reference(base.clone(), name, test_only);
                    if let Some(alias) = node.child_by_field_name("alias").and_then(|alias| ident(text(alias, source))) {
                        result.binding(alias, base);
                    }
                }
                continue;
            }
            "scoped_identifier" | "scoped_type_identifier" => {
                if let Some(base) = root(node, source) {
                    result.reference(base, node, test_only);
                }
            }
            "token_tree" | "token_tree_pattern" | "line_comment" | "block_comment" | "string_literal" | "raw_string_literal" => continue,
            _ => {}
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        todo.extend(children.into_iter().rev().map(|child| (child, test_only)));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rust_paths_use_trees_and_nested_imports_do_not_become_external_roots() {
        let source = r#"use serde::{Serialize, de::DeserializeOwned}; use std::{fmt::Debug, io}; use crate::serde as local; use foo::{serde as renamed}; fn f<T>() { let _ = serde_json::json!({}); } // tokio::spawn
const TEXT: &str = "reqwest::Client";"#;
        let result = references(source, &AtomicBool::new(false));
        assert!(result.references.iter().any(|item| item.root == "serde"));
        assert!(result.references.iter().any(|item| item.root == "serde_json"));
        assert!(!result.references.iter().any(|item| { ["de", "std", "fmt", "io", "reqwest", "tokio"].contains(&item.root.as_str()) }));
        assert_eq!(result.imported.get("renamed").map(String::as_str), Some("foo"));
    }
    #[test]
    fn local_names_aliases_globs_and_cfg_test_are_recorded_conservatively() {
        let source = "mod serde {} type tokio = (); use crate as regex; use other::*; #[cfg(test)] mod tests { use tempfile::tempdir; }";
        let result = references(source, &AtomicBool::new(false));
        assert!(result.local_names.contains("serde") && result.local_names.contains("tokio"));
        assert_eq!(result.imported.get("regex").map(String::as_str), Some("crate"));
        assert!(result.glob);
        assert!(result.references.iter().any(|item| item.root == "tempfile" && item.test_only));
    }
}
