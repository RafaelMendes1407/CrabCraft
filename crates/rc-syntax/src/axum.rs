//! Static Axum route declarations. No expansion, type checking or user code execution.
use crate::{Lang, parse_now};
use rc_text::Rope;
use std::collections::{BTreeSet, HashMap, HashSet};
use tree_sitter::Node;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Route {
    pub method: String, pub path: String, pub handler: String,
    /// Zero-based source location of `.route` and the handler expression.
    pub line: usize, pub column: usize, pub handler_line: usize, pub handler_column: usize,
}
#[derive(Clone, Debug, Default)]
pub struct Routes { pub routes: Vec<Route>, pub unresolved: usize, pub limited: bool }
struct Binding<'a> { name: String, value: Node<'a>, start: usize, end: usize, declared: usize }
struct Model<'a> { source: &'a str, routers: HashSet<String>, methods: HashMap<String, String>, bindings: Vec<Binding<'a>>, functions: HashMap<String, Node<'a>>, ambiguous: HashSet<String>, budget: usize, unresolved: usize, used: HashSet<usize> }
fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str { source.get(node.byte_range()).unwrap_or("") }
fn location(node: Node<'_>, source: &str) -> (usize, usize) {
    let start = node.start_byte();
    let line_start = source.get(..start).and_then(|s| s.rfind('\n')).map_or(0, |p| p + 1);
    (node.start_position().row, source.get(line_start..start).map_or(0, |s| s.chars().count()))
}
fn children(node: Node<'_>) -> Vec<Node<'_>> { let mut cursor = node.walk(); node.named_children(&mut cursor).collect() }
fn method(name: &str) -> Option<&'static str> {
    match name { "get" => Some("GET"), "post" => Some("POST"), "put" => Some("PUT"), "patch" => Some("PATCH"), "delete" => Some("DELETE"), "head" => Some("HEAD"), "options" => Some("OPTIONS"), "trace" => Some("TRACE"), "connect" => Some("CONNECT"), "any" => Some("ANY"), _ => None }
}
fn literal(node: Node<'_>, source: &str) -> Option<String> {
    string_value(text(node,source),node.kind()=="raw_string_literal")
}
pub fn string_value(value:&str,raw:bool)->Option<String>{
    if raw { let first = value.find('"')?; let last = value.rfind('"')?; return value.get(first + 1..last).map(str::to_string); }
    let mut chars = value.strip_prefix('"')?.strip_suffix('"')?.chars(); let mut out = String::new();
    while let Some(c) = chars.next() {
        if c != '\\' { out.push(c); continue; }
        out.push(match chars.next()? {
            '\\' => '\\', '"' => '"', '\'' => '\'', 'n' => '\n', 'r' => '\r', 't' => '\t', '0' => '\0',
            'x' => { let a = chars.next()?.to_digit(16)?; let b = chars.next()?.to_digit(16)?; char::from_u32(a * 16 + b)? },
            'u' => {
                if chars.next()? != '{' { return None; }
                let mut digits = String::new(); let mut closed = false;
                for c in chars.by_ref().take(8) { if c == '}' { closed = true; break; } if c != '_' { digits.push(c); } }
                if !closed { return None; } char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?
            }
            _ => return None,
        });
    }
    Some(out)
}
fn join(prefix: &str, path: &str) -> String { if prefix.is_empty() { path.to_string() } else { format!("{}{path}", prefix.trim_end_matches('/')) } }
impl Model<'_> {
    fn routes(&mut self, node: Node<'_>, prefix: &str, depth: usize) -> Option<Vec<Route>> {
        if depth > 64 || self.budget == 0 { return None; } self.budget -= 1;
        match node.kind() {
            "block" | "expression_statement" | "parenthesized_expression" | "return_expression" => return children(node).last().copied().and_then(|n| self.routes(n, prefix, depth + 1)),
            "identifier" => {
                let name = text(node, self.source);
                let value = self.bindings.iter().filter(|b| b.name == name && b.declared < node.start_byte() && b.start <= node.start_byte() && b.end >= node.end_byte()).max_by_key(|b| b.declared)?.value;
                self.used.insert(value.id()); return self.routes(value, prefix, depth + 1);
            }
            "call_expression" => {}
            _ => return None,
        }
        let function = node.child_by_field_name("function")?;
        let args = children(node.child_by_field_name("arguments")?);
        if function.kind() != "field_expression" {
            let name = text(function, self.source).split_whitespace().collect::<String>();
            let mut simple = String::new(); let mut generic = 0usize;
            for c in name.chars() { match c { '<' => generic += 1, '>' => generic = generic.saturating_sub(1), _ if generic == 0 => simple.push(c), _ => {} } }
            let simple = simple.replace("::::", "::");
            if simple == "axum::Router::new" || self.routers.iter().any(|r| simple == format!("{r}::new")) { return Some(Vec::new()); }
            if let Some(body) = self.functions.get(&simple).copied() { let root = children(body).last().copied()?; self.used.insert(root.id()); return self.routes(root, prefix, depth + 1); }
            return None;
        }
        let name = text(function.child_by_field_name("field")?, self.source);
        let receiver = function.child_by_field_name("value")?;
        let mut routes = self.routes(receiver, prefix, depth + 1)?;
        match name {
            "route" if args.len() == 2 => {
                let Some(path) = literal(args[0], self.source).filter(|p| p.starts_with('/') && p.len() <= 2048 && !p.chars().any(char::is_control)) else { self.unresolved += 1; return Some(routes); };
                let methods = self.method_routes(args[1], 0);
                if methods.is_empty() { self.unresolved += 1; }
                let path = join(prefix, &path);
                if path.len() > 4096 { self.unresolved += 1; return Some(routes); }
                for (method, handler) in methods {
                    let definition = (handler.kind() == "identifier").then(|| self.functions.get(text(handler, self.source)).copied()).flatten()
                        .and_then(|body| body.parent()).and_then(|item| item.child_by_field_name("name")).unwrap_or(handler);
                    let point = location(definition, self.source); let at = location(function.child_by_field_name("field")?, self.source);
                    routes.push(Route { method, path: path.clone(), handler: text(handler, self.source).split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect(), line: at.0, column: at.1, handler_line: point.0, handler_column: point.1 });
                }
            }
            "nest" if args.len() == 2 => {
                if let Some(path) = literal(args[0], self.source).filter(|p| p.starts_with('/') && p.len() <= 2048) {
                    let nested_prefix = join(prefix, &path);
                    if nested_prefix.len() > 4096 { self.unresolved += 1; return Some(routes); }
                    if let Some(nested) = self.routes(args[1], &nested_prefix, depth + 1) { self.used.insert(args[1].id()); routes.extend(nested); }
                    else { self.unresolved += 1; }
                } else { self.unresolved += 1; }
            }
            "merge" if args.len() == 1 => {
                if let Some(nested) = self.routes(args[0], prefix, depth + 1) { self.used.insert(args[0].id()); routes.extend(nested); } else { self.unresolved += 1; }
            }
            // These leave previously declared paths unchanged.
            "with_state" | "layer" | "route_layer" | "fallback" | "fallback_service" | "without_v07_checks" => {}
            _ => self.unresolved += 1,
        }
        if routes.len() > 5000 { self.budget = 0; routes.truncate(5000); }
        Some(routes)
    }
    fn method_routes<'a>(&self, node: Node<'a>, depth: usize) -> Vec<(String, Node<'a>)> {
        if depth > 32 || node.kind() != "call_expression" { return Vec::new(); }
        let Some(function) = node.child_by_field_name("function") else { return Vec::new() };
        let Some(arguments) = node.child_by_field_name("arguments") else { return Vec::new() };
        let args = children(arguments); let mut out = Vec::new();
        let name = if function.kind() == "field_expression" {
            if let Some(value) = function.child_by_field_name("value") { out.extend(self.method_routes(value, depth + 1)); }
            function.child_by_field_name("field").map(|n| text(n, self.source)).unwrap_or("")
        } else { text(function, self.source) };
        let http = if name.starts_with("axum::routing::") || name.starts_with("routing::") { method(name.rsplit("::").next().unwrap_or("")).map(str::to_string) }
            else { self.methods.get(name).cloned().or_else(|| (function.kind() == "field_expression").then(|| method(name).map(str::to_string)).flatten()) };
        if let (Some(http), Some(handler)) = (http, args.first()) { out.push((http, *handler)); }
        out
    }
}

/// Returns syntactic declarations, including inactive cfg branches. Dynamic strings, macro
/// generated routers, cross-file factories and runtime conditionals are not inferred.
pub fn routes(source: &str) -> Routes {
    if source.len() > 2 * 1024 * 1024 { return Routes { limited: true, ..Default::default() }; }
    let Some(tree) = parse_now(Lang::Rust, &Rope::from_str(source), None) else { return Routes::default() };
    let mut model = Model { source, routers: HashSet::new(), methods: HashMap::new(), bindings: Vec::new(), functions: HashMap::new(), ambiguous: HashSet::new(), budget: 20_000, unresolved: 0, used: HashSet::new() };
    let mut stack = vec![tree.root_node()]; let mut roots = Vec::new(); let mut visited = 0;
    while let Some(node) = stack.pop() {
        visited += 1; if visited > 100_000 { break; }
        if node.kind() == "use_declaration" && text(node, source).contains("axum") {
            let mut imports = vec![node];
            while let Some(part) = imports.pop() {
                if part.kind() == "use_as_clause" {
                    if let (Some(path), Some(alias)) = (part.child_by_field_name("path"), part.child_by_field_name("alias")) {
                        let original = text(path, source).rsplit("::").next().unwrap_or(""); let alias = text(alias, source).to_string();
                        if original == "Router" { model.routers.insert(alias); } else if let Some(http) = method(original) { model.methods.insert(alias, http.to_string()); }
                    }
                    continue;
                } else if matches!(part.kind(), "identifier" | "type_identifier") {
                    let name = text(part, source);
                    if name == "Router" { model.routers.insert(name.to_string()); } else if let Some(http) = method(name) { model.methods.insert(name.to_string(), http.to_string()); }
                }
                imports.extend(children(part));
            }
        }
        if node.kind() == "let_declaration" && let (Some(name), Some(value)) = (node.child_by_field_name("pattern"), node.child_by_field_name("value")) {
            if name.kind() == "identifier" && let Some(scope) = node.parent() { model.bindings.push(Binding { name: text(name, source).to_string(), value, start: scope.start_byte(), end: scope.end_byte(), declared: node.start_byte() }); }
        }
        if node.kind() == "function_item" && let (Some(name), Some(body)) = (node.child_by_field_name("name"), node.child_by_field_name("body")) {
            let name = text(name, source).to_string();
            if model.functions.remove(&name).is_some() { model.ambiguous.insert(name); }
            else if !model.ambiguous.contains(&name) { model.functions.insert(name, body); }
        }
        if node.kind() == "call_expression" && !node.parent().is_some_and(|p| p.kind() == "field_expression" && p.child_by_field_name("value").is_some_and(|v| v.id() == node.id())) { roots.push(node); }
        stack.extend(children(node));
    }
    let mut groups = Vec::new();
    for root in roots { if let Some(routes) = model.routes(root, "", 0) { groups.push((root.id(), routes)); } }
    let mut unique = BTreeSet::new();
    for (id, routes) in groups { if !model.used.contains(&id) { unique.extend(routes); } }
    let limited = visited > 100_000 || model.budget == 0 || unique.len() > 5000;
    Routes { routes: unique.into_iter().take(5000).collect(), unresolved: model.unresolved, limited }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_from_axum_syntax_exclude_comments_and_resolve_local_nesting() {
        let source = r##"use axum::{Router, routing::{get, post as write}};
async fn users() {}
async fn save() {}
fn nested() -> Router { Router::new().route(r#"/health"#, get(users)) }
fn app() -> Router {
    let routes = Router::new().route("/users/{id}", get(users).post(save));
    // Router::new().route("/not-code", get(users));
    Router::new().nest("/api", routes).nest("/admin", nested()).route("/write", write(save))
}"##;
        let report = routes(source);
        let paths: BTreeSet<_> = report.routes.iter().map(|r| (r.method.as_str(), r.path.as_str())).collect();
        assert_eq!(paths, BTreeSet::from([("GET", "/admin/health"), ("GET", "/api/users/{id}"), ("POST", "/api/users/{id}"), ("POST", "/write")]));
        assert!(!report.limited);
    }
    #[test]
    fn aliases_generics_and_unicode_columns_keep_navigation_in_rust_character_units() {
        let source = "use axum::{Router as ApiRouter, routing::get as read}; async fn handler() {} fn app() -> ApiRouter { let café = 1; ApiRouter::<()>::new().route(\"/café\", read(handler)) }";
        let report = routes(source);
        assert_eq!(report.routes.len(), 1);
        let route = &report.routes[0];
        let at = source.find("route(").unwrap();
        assert_eq!(route.column, source[..at].chars().count());
        assert_eq!(route.handler_column, source[..source.find("handler()").unwrap()].chars().count());
        assert_eq!(route.path, "/café");
        assert!(routes("use axum::Router as ApiRouter; fn x() { Router::new().route(\"/unknown\", get(handler)); }").routes.is_empty());
    }
    #[test]
    fn unknown_router_and_dynamic_paths_are_not_invented() {
        assert!(routes("fn f() { Other::new().route(\"/no\", get(handler)); }").routes.is_empty());
        let report = routes("fn f() { axum::Router::new().route(path(), axum::routing::get(handler)); }");
        assert!(report.routes.is_empty()); assert!(report.unresolved > 0);
    }
}
