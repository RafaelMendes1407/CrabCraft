//! Bounded, explicit Axum graph resolution through the existing trusted RA connection.
//! No Cargo command, macro expansion request, registry lookup or project evaluation.
use rc_lsp::{Client, lsp_types::Position, path_to_uri, uri_to_path};
use rc_syntax::tree_sitter::Node;
use rc_text::Rope;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Cancelled,
    Timeout,
    Limit,
    Changed,
    Io,
    Server,
}
#[derive(Clone)]
pub struct Overlay {
    pub id: u64,
    pub version: u64,
    pub path: PathBuf,
    pub rope: Rope,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Key {
    file: PathBuf,
    position: Position,
}
#[derive(Clone)]
struct Source {
    path: PathBuf,
    text: Arc<str>,
    rope: Rope,
}
#[derive(Clone)]
enum Call {
    Function {
        key: Key,
        name: String,
    },
    Method {
        receiver: usize,
        key: Key,
        name: String,
    },
}
#[derive(Clone)]
enum ExprKind {
    Call(Call, Vec<usize>),
    Reference(Key),
    String(String),
    Unknown,
}
#[derive(Clone)]
struct Expr {
    key: Key,
    kind: ExprKind,
    display: String,
}
#[derive(Clone)]
struct Function {
    body: usize,
    parameters: usize,
}
#[derive(Default)]
struct Program {
    sources: BTreeMap<PathBuf, Source>,
    expressions: Vec<Expr>,
    memo: HashMap<(PathBuf, usize, usize), usize>,
    functions: BTreeMap<Key, Function>,
    bindings: BTreeMap<Key, usize>,
    roots: Vec<usize>,
}
#[derive(Clone)]
struct Symbol {
    key: Key,
    name: String,
    package: Option<String>,
    owner: Option<String>,
}
pub struct Route {
    pub file: PathBuf,
    pub route: rc_syntax::axum::Route,
    pub handler_file: Option<PathBuf>,
}
pub struct Report {
    pub routes: Vec<Route>,
    pub hashes: BTreeMap<PathBuf, u64>,
    pub unresolved: usize,
    pub limited: bool,
}
pub struct Budget {
    pub cancel: Arc<AtomicBool>,
    pub deadline: Instant,
}
impl Budget {
    fn check(&self) -> Result<(), Error> {
        if self.cancel.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(Error::Timeout)
        } else {
            Ok(())
        }
    }
}
pub fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    })
}
fn read(path: &Path, max: usize) -> Result<String, Error> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| Error::Io)?;
    if !file.metadata().map_err(|_| Error::Io)?.is_file() {
        return Err(Error::Io);
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Io)?;
    if bytes.len() > max {
        return Err(Error::Limit);
    }
    let text = String::from_utf8(bytes).map_err(|_| Error::Io)?;
    Ok(text.trim_start_matches('\u{feff}').replace("\r\n", "\n"))
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or("")
}
fn name_node(mut node: Node<'_>) -> Node<'_> {
    for _ in 0..8 {
        if let Some(name) = node.child_by_field_name("name") {
            return name;
        }
        if let Some(function) = node.child_by_field_name("function") {
            node = function;
        } else {
            break;
        }
    }
    node
}
fn key(source: &Source, node: Node<'_>) -> Key {
    Key {
        file: source.path.clone(),
        position: crate::convert::lsp_position(
            &source.rope,
            source.rope.byte_to_char(node.start_byte()),
        ),
    }
}
impl Program {
    fn expression(
        &mut self,
        source: &Source,
        node: Node<'_>,
        depth: usize,
    ) -> Result<usize, Error> {
        if depth > 64 || self.expressions.len() >= 50_000 {
            return Err(Error::Limit);
        }
        let address = (source.path.clone(), node.start_byte(), node.end_byte());
        if let Some(id) = self.memo.get(&address) {
            return Ok(*id);
        }
        let kind = match node.kind() {
            "block" | "expression_statement" | "return_expression" | "parenthesized_expression" => {
                if let Some(tail) = children(node)
                    .into_iter()
                    .filter(|node| !matches!(node.kind(), "line_comment" | "block_comment"))
                    .last()
                {
                    let id = self.expression(source, tail, depth + 1)?;
                    self.memo.insert(address, id);
                    return Ok(id);
                }
                ExprKind::Unknown
            }
            "identifier" | "scoped_identifier" => ExprKind::Reference(key(source, name_node(node))),
            "string_literal" | "raw_string_literal" => rc_syntax::axum::string_value(
                text(node, &source.text),
                node.kind() == "raw_string_literal",
            )
            .map(ExprKind::String)
            .unwrap_or(ExprKind::Unknown),
            "call_expression" => {
                let Some(function) = node.child_by_field_name("function") else {
                    return self.unknown(source, node, address);
                };
                let arguments = node
                    .child_by_field_name("arguments")
                    .map(children)
                    .unwrap_or_default();
                if arguments.len() > 32 {
                    return Err(Error::Limit);
                }
                let mut args = Vec::new();
                for argument in arguments {
                    args.push(self.expression(source, argument, depth + 1)?);
                }
                let function = if function.kind() == "generic_function" {
                    function.child_by_field_name("function").unwrap_or(function)
                } else {
                    function
                };
                let call = if function.kind() == "field_expression" {
                    let Some(field) = function.child_by_field_name("field") else {
                        return self.unknown(source, node, address);
                    };
                    let Some(receiver) = function.child_by_field_name("value") else {
                        return self.unknown(source, node, address);
                    };
                    Call::Method {
                        receiver: self.expression(source, receiver, depth + 1)?,
                        key: key(source, field),
                        name: text(field, &source.text).into(),
                    }
                } else {
                    let name = name_node(function);
                    Call::Function {
                        key: key(source, name),
                        name: text(name, &source.text).into(),
                    }
                };
                ExprKind::Call(call, args)
            }
            _ => ExprKind::Unknown,
        };
        let id = self.expressions.len();
        self.expressions.push(Expr {
            key: key(source, node),
            kind,
            display: text(node, &source.text).chars().take(160).collect(),
        });
        self.memo.insert(address, id);
        Ok(id)
    }
    fn unknown(
        &mut self,
        source: &Source,
        node: Node<'_>,
        address: (PathBuf, usize, usize),
    ) -> Result<usize, Error> {
        let id = self.expressions.len();
        self.expressions.push(Expr {
            key: key(source, node),
            kind: ExprKind::Unknown,
            display: String::new(),
        });
        self.memo.insert(address, id);
        Ok(id)
    }
    fn collect(&mut self, source: Source, budget: &Budget) -> Result<(), Error> {
        let tree =
            rc_syntax::parse_now(rc_syntax::Lang::Rust, &source.rope, None).ok_or(Error::Io)?;
        let mut stack = vec![tree.root_node()];
        let mut nodes = 0;
        while let Some(node) = stack.pop() {
            nodes += 1;
            if nodes > 100_000 {
                return Err(Error::Limit);
            }
            if nodes % 500 == 0 {
                budget.check()?;
            }
            if node.kind() == "function_item" {
                if let (Some(name), Some(body)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("body"),
                ) {
                    let parameters =
                        node.child_by_field_name("parameters")
                            .map(children)
                            .map_or(0, |nodes| {
                                nodes
                                    .into_iter()
                                    .filter(|node| {
                                        !matches!(node.kind(), "line_comment" | "block_comment")
                                    })
                                    .count()
                            });
                    let body = self.expression(&source, body, 0)?;
                    self.functions
                        .insert(key(&source, name), Function { body, parameters });
                }
            }
            if matches!(node.kind(), "let_declaration" | "const_item") {
                if let (Some(pattern), Some(value)) = (
                    node.child_by_field_name("pattern")
                        .or_else(|| node.child_by_field_name("name")),
                    node.child_by_field_name("value"),
                ) {
                    if pattern.kind() == "identifier"
                        && !children(node)
                            .iter()
                            .any(|node| node.kind() == "mutable_specifier")
                    {
                        let value = self.expression(&source, value, 0)?;
                        self.bindings.insert(key(&source, pattern), value);
                    }
                }
            }
            if node.kind() == "call_expression"
                && !node.parent().is_some_and(|parent| {
                    parent.kind() == "field_expression"
                        && parent
                            .child_by_field_name("value")
                            .is_some_and(|value| value.id() == node.id())
                })
            {
                let id = self.expression(&source, node, 0)?;
                self.roots.push(id);
            }
            stack.extend(children(node));
        }
        self.sources.insert(source.path.clone(), source);
        Ok(())
    }
}
struct Resolver<'a> {
    client: &'a Client,
    budget: &'a Budget,
    sources: BTreeMap<PathBuf, Source>,
    cache: HashMap<Key, Option<Symbol>>,
    packages: HashMap<PathBuf, Option<String>>,
    requests: usize,
    loaded: usize,
}
impl Resolver<'_> {
    fn source(&mut self, path: &Path) -> Result<Source, Error> {
        if let Some(source) = self.sources.get(path) {
            return Ok(source.clone());
        }
        if self.sources.len() > 256 {
            return Err(Error::Limit);
        }
        let value = read(path, 2 * 1024 * 1024)?;
        self.loaded = self.loaded.saturating_add(value.len());
        if self.loaded > 32 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        let source = Source {
            path: path.into(),
            rope: Rope::from_str(&value),
            text: Arc::from(value),
        };
        self.sources.insert(path.into(), source.clone());
        Ok(source)
    }
    fn package(&mut self, path: &Path) -> Result<Option<String>, Error> {
        for directory in path.ancestors().skip(1).take(10) {
            if let Some(value) = self.packages.get(directory) {
                return Ok(value.clone());
            }
            let manifest = directory.join("Cargo.toml");
            if let Ok(value) = read(&manifest, 256 * 1024) {
                let value = toml::from_str::<toml::Value>(&value).map_err(|_| Error::Io)?;
                let name = value
                    .get("package")
                    .and_then(|package| package.get("name"))
                    .and_then(toml::Value::as_str)
                    .map(str::to_string);
                self.packages.insert(directory.into(), name.clone());
                return Ok(name);
            }
        }
        Ok(None)
    }
    fn definition(&mut self, origin: &Key) -> Result<Option<Symbol>, Error> {
        if let Some(symbol) = self.cache.get(origin) {
            return Ok(symbol.clone());
        }
        let retry_until = Instant::now() + Duration::from_secs(10);
        let value = loop {
            self.budget.check()?;
            self.requests += 1;
            if self.requests > 1024 {
                return Err(Error::Limit);
            }
            let mut pending=self.client.request_raw::<Value>("textDocument/definition",json!({"textDocument":{"uri":path_to_uri(&origin.file).as_str()},"position":origin.position}));
            let response = loop {
                self.budget.check()?;
                if let Some(response) = pending.poll() {
                    break response;
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            match response {
                Ok(value)
                    if Instant::now() < retry_until
                        && (value.is_null() || value.as_array().is_some_and(Vec::is_empty)) =>
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(value) => break value,
                Err(error)
                    if Instant::now() < retry_until
                        && (matches!(error.code, -32801 | -32800)
                            || (error.code == -32603
                                && error.message.starts_with("file not found:"))) =>
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return Err(Error::Server),
            }
        };
        let values = match value {
            Value::Array(values) => values,
            Value::Null => Vec::new(),
            value => vec![value],
        };
        if values.len() != 1 {
            self.cache.insert(origin.clone(), None);
            return Ok(None);
        }
        let value = &values[0];
        let uri = value
            .get("targetUri")
            .or_else(|| value.get("uri"))
            .and_then(Value::as_str);
        let range = value
            .get("targetSelectionRange")
            .or_else(|| value.get("range"));
        let Some((path, range)) =
            uri.and_then(|uri| uri.parse().ok())
                .and_then(|uri| uri_to_path(&uri))
                .zip(range.cloned().and_then(|range| {
                    serde_json::from_value::<rc_lsp::lsp_types::Range>(range).ok()
                }))
        else {
            self.cache.insert(origin.clone(), None);
            return Ok(None);
        };
        let source = self.source(&path)?;
        let start = crate::convert::char_of(&source.rope, range.start);
        let end = crate::convert::char_of(&source.rope, range.end);
        if start > end
            || end - start > 256
            || crate::convert::lsp_position(&source.rope, start) != range.start
            || crate::convert::lsp_position(&source.rope, end) != range.end
        {
            return Err(Error::Server);
        }
        let name = source.rope.slice(start..end).to_string();
        let tree =
            rc_syntax::parse_now(rc_syntax::Lang::Rust, &source.rope, None).ok_or(Error::Io)?;
        let byte = source.rope.char_to_byte(start);
        let mut node = tree.root_node().descendant_for_byte_range(byte, byte);
        let mut owner = None;
        while let Some(current) = node {
            if current.kind() == "impl_item" {
                if let Some(ty) = current.child_by_field_name("type") {
                    let ty = if ty.kind() == "generic_type" {
                        ty.child_by_field_name("type").unwrap_or(ty)
                    } else {
                        ty
                    };
                    owner = Some(text(ty, &source.text).into());
                }
                break;
            }
            node = current.parent();
        }
        let symbol = Symbol {
            key: Key {
                file: path.clone(),
                position: range.start,
            },
            name,
            package: self.package(&path)?,
            owner,
        };
        self.cache.insert(origin.clone(), Some(symbol.clone()));
        Ok(Some(symbol))
    }
}
fn method(name: &str) -> Option<&'static str> {
    match name {
        "get" => Some("GET"),
        "post" => Some("POST"),
        "put" => Some("PUT"),
        "patch" => Some("PATCH"),
        "delete" => Some("DELETE"),
        "head" => Some("HEAD"),
        "options" => Some("OPTIONS"),
        "trace" => Some("TRACE"),
        "connect" => Some("CONNECT"),
        "any" => Some("ANY"),
        _ => None,
    }
}
fn join(prefix: &str, path: &str) -> Option<String> {
    let value = if prefix.is_empty() {
        path.into()
    } else {
        format!("{}{path}", prefix.trim_end_matches('/'))
    };
    (value.len() <= 4096).then_some(value)
}
struct Graph<'a> {
    program: &'a Program,
    resolver: Resolver<'a>,
    used: HashSet<usize>,
    unresolved: usize,
    steps: usize,
    active: HashSet<usize>,
}
impl Graph<'_> {
    fn literal(&mut self, id: usize, depth: usize) -> Result<Option<String>, Error> {
        if depth > 32 {
            return Ok(None);
        }
        let Some(expression) = self.program.expressions.get(id).cloned() else {
            return Ok(None);
        };
        match expression.kind {
            ExprKind::String(value) => Ok(Some(value)),
            ExprKind::Reference(origin) => {
                if let Some(symbol) = self.resolver.definition(&origin)? {
                    if let Some(value) = self.program.bindings.get(&symbol.key) {
                        return self.literal(*value, depth + 1);
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
    fn methods(&mut self, id: usize, depth: usize) -> Result<Vec<(String, usize)>, Error> {
        if depth > 32 {
            return Ok(Vec::new());
        }
        let Some(expression) = self.program.expressions.get(id).cloned() else {
            return Ok(Vec::new());
        };
        let ExprKind::Call(call, args) = expression.kind else {
            return Ok(Vec::new());
        };
        let (origin, receiver) = match call {
            Call::Function { key, .. } => (key, None),
            Call::Method { key, receiver, .. } => (key, Some(receiver)),
        };
        let Some(symbol) = self.resolver.definition(&origin)? else {
            return Ok(Vec::new());
        };
        if symbol.package.as_deref() != Some("axum") {
            return Ok(Vec::new());
        }
        let mut methods = if let Some(receiver) = receiver {
            self.methods(receiver, depth + 1)?
        } else {
            Vec::new()
        };
        if let (Some(method), Some(handler)) = (method(&symbol.name), args.first()) {
            methods.push((method.into(), *handler));
        }
        Ok(methods)
    }
    fn evaluate(
        &mut self,
        id: usize,
        prefix: &str,
        depth: usize,
    ) -> Result<Option<Vec<Route>>, Error> {
        self.resolver.budget.check()?;
        self.steps += 1;
        if self.steps > 20_000 || depth > 64 {
            return Err(Error::Limit);
        }
        if !self.active.insert(id) {
            self.unresolved += 1;
            return Ok(None);
        }
        let result = self.inner(id, prefix, depth);
        self.active.remove(&id);
        result
    }
    fn inner(
        &mut self,
        id: usize,
        prefix: &str,
        depth: usize,
    ) -> Result<Option<Vec<Route>>, Error> {
        let Some(expression) = self.program.expressions.get(id).cloned() else {
            return Ok(None);
        };
        match expression.kind {
            ExprKind::Reference(origin) => {
                if let Some(symbol) = self.resolver.definition(&origin)? {
                    if let Some(value) = self.program.bindings.get(&symbol.key) {
                        let value = *value;
                        self.used.insert(value);
                        return self.evaluate(value, prefix, depth + 1);
                    }
                }
                Ok(None)
            }
            ExprKind::Call(Call::Function { key: origin, .. }, args) => {
                let Some(symbol) = self.resolver.definition(&origin)? else {
                    return Ok(None);
                };
                if symbol.package.as_deref() == Some("axum")
                    && symbol.owner.as_deref() == Some("Router")
                    && symbol.name == "new"
                    && args.is_empty()
                {
                    return Ok(Some(Vec::new()));
                }
                if let Some(function) = self
                    .program
                    .functions
                    .get(&symbol.key)
                    .filter(|function| function.parameters == 0 && args.is_empty())
                {
                    let body = function.body;
                    self.used.insert(body);
                    return self.evaluate(body, prefix, depth + 1);
                }
                Ok(None)
            }
            ExprKind::Call(
                Call::Method {
                    receiver,
                    key: origin,
                    name,
                },
                args,
            ) => {
                let Some(symbol) = self.resolver.definition(&origin)? else {
                    return Ok(None);
                };
                if symbol.package.as_deref() != Some("axum")
                    || symbol.owner.as_deref() != Some("Router")
                    || symbol.name != name
                {
                    return Ok(None);
                }
                let mut routes = match self.evaluate(receiver, prefix, depth + 1)? {
                    Some(routes) => routes,
                    None => {
                        self.unresolved += 1;
                        Vec::new()
                    }
                };
                match name.as_str() {
                    "route" if args.len() == 2 => {
                        let path = self.literal(args[0], 0)?.filter(|path| {
                            path.starts_with('/') && !path.chars().any(char::is_control)
                        });
                        let Some(path) = path.and_then(|path| join(prefix, &path)) else {
                            self.unresolved += 1;
                            return Ok(Some(routes));
                        };
                        let methods = self.methods(args[1], 0)?;
                        if methods.is_empty() {
                            self.unresolved += 1;
                        }
                        for (method, handler) in methods {
                            let handler_expr = &self.program.expressions[handler];
                            let handler_key = match &handler_expr.kind {
                                ExprKind::Reference(key) => Some(key.clone()),
                                _ => None,
                            };
                            let symbol = if let Some(key) = handler_key {
                                self.resolver.definition(&key)?
                            } else {
                                None
                            };
                            let target = symbol
                                .filter(|symbol| self.program.functions.contains_key(&symbol.key))
                                .map(|symbol| symbol.key);
                            let at = self.location(&origin);
                            let handler_at = target
                                .as_ref()
                                .map(|key| self.location(key))
                                .unwrap_or_else(|| self.location(&handler_expr.key));
                            routes.push(Route {
                                file: origin.file.clone(),
                                route: rc_syntax::axum::Route {
                                    method,
                                    path: path.clone(),
                                    handler: handler_expr.display.clone(),
                                    line: at.0,
                                    column: at.1,
                                    handler_line: handler_at.0,
                                    handler_column: handler_at.1,
                                },
                                handler_file: target.map(|target| target.file),
                            });
                        }
                    }
                    "nest" if args.len() == 2 => {
                        if let Some(path) = self
                            .literal(args[0], 0)?
                            .filter(|path| {
                                path.starts_with('/') && !path.chars().any(char::is_control)
                            })
                            .and_then(|path| join(prefix, &path))
                        {
                            self.used.insert(args[1]);
                            if let Some(nested) = self.evaluate(args[1], &path, depth + 1)? {
                                routes.extend(nested);
                            } else {
                                self.unresolved += 1;
                            }
                        } else {
                            self.unresolved += 1;
                        }
                    }
                    "merge" if args.len() == 1 => {
                        self.used.insert(args[0]);
                        if let Some(nested) = self.evaluate(args[0], prefix, depth + 1)? {
                            routes.extend(nested);
                        } else {
                            self.unresolved += 1;
                        }
                    }
                    "with_state" | "layer" | "route_layer" | "fallback" | "fallback_service"
                    | "without_v07_checks" => {}
                    _ => self.unresolved += 1,
                }
                if routes.len() > 5000 {
                    return Err(Error::Limit);
                }
                Ok(Some(routes))
            }
            _ => Ok(None),
        }
    }
    fn location(&self, key: &Key) -> (usize, usize) {
        let Some(source) = self.program.sources.get(&key.file) else {
            return (key.position.line as usize, key.position.character as usize);
        };
        let at = crate::convert::char_of(&source.rope, key.position);
        let line = source.rope.char_to_line(at);
        (line, at - source.rope.line_to_char(line))
    }
}
pub fn scan(
    roots: Vec<PathBuf>,
    workspace: &Path,
    excluded: &[String],
    overlays: &[Overlay],
    client: &Client,
    budget: &Budget,
) -> Result<Report, Error> {
    let mut dirs = roots;
    let mut files = BTreeMap::new();
    let mut seen = HashSet::new();
    let mut entries = 0;
    let mut total = 0;
    let mut disk = BTreeMap::new();
    while let Some(dir) = dirs.pop() {
        budget.check()?;
        if !seen.insert(dir.clone()) {
            continue;
        }
        if seen.len() > 512 {
            return Err(Error::Limit);
        }
        for entry in std::fs::read_dir(&dir).map_err(|_| Error::Io)? {
            entries += 1;
            if entries > 10_000 {
                return Err(Error::Limit);
            }
            let entry = entry.map_err(|_| Error::Io)?;
            let path = entry.path();
            let ty = entry.file_type().map_err(|_| Error::Io)?;
            if ty.is_symlink() {
                continue;
            }
            if rc_core::workspace::relative_path(workspace, &path)
                .is_some_and(|path| rc_core::workspace::is_excluded(&path, excluded))
            {
                continue;
            }
            if ty.is_dir() {
                if !matches!(
                    entry.file_name().to_str(),
                    Some("target" | ".git" | "node_modules" | ".crabcraft")
                ) {
                    dirs.push(path);
                }
                continue;
            }
            if !ty.is_file() || path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            if files.len() >= 128 {
                return Err(Error::Limit);
            }
            let saved = read(&path, 2 * 1024 * 1024)?;
            disk.insert(path.clone(), hash(&saved));
            let value = overlays
                .iter()
                .find(|overlay| overlay.path == path)
                .map(|overlay| overlay.rope.to_string())
                .unwrap_or(saved);
            total += value.len();
            if total > 16 * 1024 * 1024 {
                return Err(Error::Limit);
            }
            files.insert(
                path.clone(),
                Source {
                    path,
                    rope: Rope::from_str(&value),
                    text: Arc::from(value),
                },
            );
        }
    }
    let mut program = Program::default();
    for source in files.into_values() {
        budget.check()?;
        program.collect(source, budget)?;
    }
    let resolver = Resolver {
        client,
        budget,
        sources: program.sources.clone(),
        cache: HashMap::new(),
        packages: HashMap::new(),
        requests: 0,
        loaded: program
            .sources
            .values()
            .map(|source| source.text.len())
            .sum(),
    };
    let mut graph = Graph {
        program: &program,
        resolver,
        used: HashSet::new(),
        unresolved: 0,
        steps: 0,
        active: HashSet::new(),
    };
    let mut groups = Vec::new();
    for id in &program.roots {
        let candidate = match &program.expressions[*id].kind {
            ExprKind::Call(Call::Method { name, .. }, _) => matches!(
                name.as_str(),
                "route"
                    | "nest"
                    | "merge"
                    | "with_state"
                    | "layer"
                    | "route_layer"
                    | "fallback"
                    | "fallback_service"
                    | "without_v07_checks"
            ),
            ExprKind::Call(Call::Function { name, .. }, _) => {
                name == "new"
                    || program.functions.keys().any(|key| {
                        graph.program.sources.get(&key.file).is_some_and(|source| {
                            let at = crate::convert::char_of(&source.rope, key.position);
                            source
                                .rope
                                .slice(at..source.rope.len_chars().min(at + name.chars().count()))
                                .to_string()
                                == *name
                        })
                    })
            }
            _ => false,
        };
        if candidate {
            if let Some(routes) = graph.evaluate(*id, "", 0)? {
                groups.push((*id, routes));
            }
        }
    }
    let mut routes = Vec::new();
    let mut unique = HashSet::new();
    for (id, group) in groups {
        if graph.used.contains(&id) {
            continue;
        }
        for route in group {
            let key = (route.file.clone(), route.route.clone());
            if unique.insert(key) {
                routes.push(route);
            }
        }
    }
    routes.sort_by(|a, b| {
        (&a.route.path, &a.route.method, &a.file, a.route.line).cmp(&(
            &b.route.path,
            &b.route.method,
            &b.file,
            b.route.line,
        ))
    });
    for (path, old) in disk {
        budget.check()?;
        if hash(&read(&path, 2 * 1024 * 1024)?) != old {
            return Err(Error::Changed);
        }
    }
    Ok(Report {
        hashes: program
            .sources
            .iter()
            .map(|(path, source)| (path.clone(), hash(&source.text)))
            .collect(),
        routes,
        unresolved: graph.unresolved,
        limited: false,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn parser_preserves_cross_file_identity_sites_and_budget_cancels() {
        let source = "pub fn routes()->Router { Router::new().route(\"/test\", get(handler)) }";
        let cancel = Arc::new(AtomicBool::new(false));
        let budget = Budget {
            cancel: cancel.clone(),
            deadline: Instant::now() + Duration::from_secs(1),
        };
        let mut program = Program::default();
        program
            .collect(
                Source {
                    path: PathBuf::from("/workspace/src/api.rs"),
                    text: Arc::from(source),
                    rope: Rope::from_str(source),
                },
                &budget,
            )
            .unwrap();
        assert_eq!(program.functions.len(), 1);
        assert!(program.expressions.iter().any(|expression| matches!(&expression.kind,ExprKind::Call(Call::Method{name,..},_) if name=="route")));
        assert!(program.expressions.iter().any(|expression| matches!(&expression.kind,ExprKind::Reference(origin) if origin.file.ends_with("api.rs"))));
        cancel.store(true, Ordering::Release);
        assert_eq!(budget.check(), Err(Error::Cancelled));
    }
    pub(crate) fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"),"[package]\nname=\"axum_semantic_evidence\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[dependencies]\naxum=\"0.8.9\"\nserde={version=\"1\",features=[\"derive\"]}\ntokio={version=\"1\",features=[\"macros\",\"rt-multi-thread\",\"net\"]}\nreqwest={version=\"0.13\",default-features=false,features=[\"rustls\"]}\n").unwrap();
        std::fs::write(root.path().join("src/main.rs"),"mod api;mod handlers;use axum::Router as HttpRouter;pub fn app()->HttpRouter{HttpRouter::new().nest(\"/v1\",api::routes()).merge(api::admin())}fn main(){let _=app();}\n").unwrap();
        std::fs::write(root.path().join("src/api.rs"),"use axum::{Router,routing::{get as fetch,post}};use crate::handlers;pub fn routes()->Router{Router::new().route(\"/users/{id}\",fetch(handlers::show).post(handlers::create))}pub fn admin()->Router{Router::new().route(\"/health\",fetch(handlers::health))}\n").unwrap();
        std::fs::write(root.path().join("src/handlers.rs"),"use axum::extract::{Path,Query};#[derive(serde::Deserialize)]pub struct Search{pub q:Option<String>,#[serde(rename=\"page_size\")]pub page:Option<usize>}pub async fn show(Path(id):Path<String>,Query(search):Query<Search>)->String{let _=search;id}pub async fn create()->&'static str{\"created\"}pub async fn health()->&'static str{\"ok\"}\n").unwrap();
        let output = std::process::Command::new("cargo")
            .args(["generate-lockfile", "--offline"])
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        root
    }
    #[test]
    #[ignore = "Requires cached Axum dependencies and a real rust-analyzer"]
    fn real_axum_semantic_routes_compose_factories_aliases_and_handlers_between_files() {
        let _home = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = fixture();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace(root.path());
        app.trust_project_tools_for_test();
        app.insight
            .set_env(vec![("CARGO_NET_OFFLINE".into(), "true".into())]);
        app.insight.start_rust(root.path(), None, false);
        for name in ["main.rs"] {
            app.open_file_and_wait(&root.path().join("src").join(name));
        }
        let mut h = crate::screens::harness(app, egui::vec2(1500.0, 1000.0));
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(60) {
            h.run_steps(2);
            if h.state().insight.status == crate::insight::InsightStatus::Ready
                && h.state().runner.cargo_workspace.is_some()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            h.state().insight.status,
            crate::insight::InsightStatus::Ready
        );
        let client = h.state().insight.session.as_ref().unwrap().client.clone();
        let root_path = root.path().to_path_buf();
        let (send, recv) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = scan(
                vec![root_path.clone()],
                &root_path,
                &[],
                &[],
                &client,
                &Budget {
                    cancel: Arc::new(AtomicBool::new(false)),
                    deadline: Instant::now() + Duration::from_secs(45),
                },
            );
            let _ = send.send(result);
        });
        let start = Instant::now();
        let report = loop {
            h.run_steps(2);
            if let Ok(result) = recv.try_recv() {
                break result.unwrap();
            }
            assert!(start.elapsed() < Duration::from_secs(50));
            std::thread::sleep(Duration::from_millis(10));
        };
        for route in &report.routes {
            eprintln!(
                "{} {} {} {:?}",
                route.route.method, route.route.path, route.route.handler, route.handler_file
            );
        }
        assert!(report.routes.iter().any(|route| {
            route.route.path == "/v1/users/{id}"
                && route.route.method == "GET"
                && route
                    .handler_file
                    .as_ref()
                    .is_some_and(|path| path.ends_with("src/handlers.rs"))
        }));
        assert!(
            report
                .routes
                .iter()
                .any(|route| route.route.path == "/v1/users/{id}" && route.route.method == "POST")
        );
        assert!(
            report
                .routes
                .iter()
                .any(|route| route.route.path == "/health")
        );
    }
}
