//! Conservative item relocation. All parsing, disk snapshots and RA requests stay off the UI.
use rc_lsp::{Client, path_to_uri, uri_to_path};
use rc_syntax::tree_sitter::{Node, Tree};
use rc_text::Rope;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
const FILE: usize = 1024 * 1024;
const TOTAL: usize = 8 * 1024 * 1024;
const FILES: usize = 128;
#[derive(Clone)]
pub struct Frozen {
    pub id: u64,
    pub version: u64,
    pub path: PathBuf,
    pub text: Rope,
    pub readonly: bool,
}
#[derive(Clone)]
pub struct Disk {
    pub path: PathBuf,
    pub bytes: Arc<[u8]>,
}
#[derive(Clone)]
pub struct Edit {
    pub id: u64,
    pub path: PathBuf,
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub before: String,
    pub after: String,
}
pub struct Plan {
    pub name: String,
    pub edits: Vec<Edit>,
    pub frozen: Vec<Frozen>,
    pub disk: Vec<Disk>,
    pub layout: Option<(PathBuf, PathBuf)>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    Context,
    Visibility,
    Collision,
    Graph,
    Changed,
    Server,
    Cancelled,
    Timeout,
    Limit,
    Io,
}
pub struct Budget {
    pub cancel: Arc<AtomicBool>,
    pub deadline: Instant,
}
impl Budget {
    pub fn check(&self) -> Result<(), Error> {
        if self.cancel.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(Error::Timeout)
        } else {
            Ok(())
        }
    }
}
fn read(path: &Path) -> Result<Vec<u8>, Error> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| Error::Io)?;
    let meta = file.metadata().map_err(|_| Error::Io)?;
    if !meta.is_file() {
        return Err(Error::Io);
    }
    if meta.len() > FILE as u64 {
        return Err(Error::Limit);
    }
    let mut bytes = Vec::new();
    file.take(FILE as u64 + 1).read_to_end(&mut bytes).map_err(|_| Error::Io)?;
    if bytes.len() > FILE {
        return Err(Error::Limit);
    }
    Ok(bytes)
}
fn layout(package: &Path, root: &Path) -> Result<(), Error> {
    for path in [
        package.join("build.rs"),
        package.join("tests"),
        package.join("examples"),
        package.join("benches"),
        package.join("src/bin"),
        root.with_file_name(if root.file_name().is_some_and(|name| name == "main.rs") { "lib.rs" } else { "main.rs" }),
    ] {
        match std::fs::symlink_metadata(path) {
            Ok(_) => return Err(Error::Graph),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Io),
        }
    }
    Ok(())
}
pub fn verify(plan: &Plan, budget: &Budget) -> Result<(), Error> {
    if let Some((package, root)) = &plan.layout {
        layout(package, root).map_err(|_| Error::Changed)?;
        let dir = root.parent().ok_or(Error::Graph)?;
        let now = scan(dir, budget)?;
        let before: Vec<_> =
            plan.disk.iter().filter(|disk| disk.path.starts_with(dir) && disk.path.extension().is_some_and(|extension| extension == "rs")).collect();
        if now.len() != before.len() || now.iter().any(|(path, bytes)| !before.iter().any(|old| old.path == *path && old.bytes.as_ref() == bytes.as_slice())) {
            return Err(Error::Changed);
        }
    }
    for disk in &plan.disk {
        budget.check()?;
        if read(&disk.path)?.as_slice() != disk.bytes.as_ref() {
            return Err(Error::Changed);
        }
    }
    Ok(())
}
fn source(bytes: &[u8]) -> Result<Rope, Error> {
    let bytes = bytes.strip_prefix(&[239, 187, 191]).unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Io)?;
    if text.contains('\0') {
        return Err(Error::Io);
    }
    Ok(Rope::from_str(&text.replace("\r\n", "\n")))
}
fn text<'a>(node: Node<'_>, source: &'a str) -> Result<&'a str, Error> {
    source.get(node.byte_range()).ok_or(Error::Unsupported)
}
fn ident(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && name.bytes().enumerate().all(|(i, ch)| ch == b'_' || ch.is_ascii_alphabetic() || i > 0 && ch.is_ascii_digit())
}
fn visibility(node: Node<'_>, src: &str) -> Result<String, Error> {
    let mut cursor = node.walk();
    let vis = node.named_children(&mut cursor).find(|node| node.kind() == "visibility_modifier").ok_or(Error::Visibility)?;
    let value = text(vis, src)?.split_whitespace().collect::<String>();
    if !matches!(value.as_str(), "pub" | "pub(crate)") {
        return Err(Error::Visibility);
    }
    Ok(value)
}
fn primitive(name: &str) -> bool {
    matches!(name, "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "f32" | "f64" | "bool" | "char" | "str")
}
fn parse(rope: &Rope) -> Result<Tree, Error> {
    let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, rope, None).ok_or(Error::Unsupported)?;
    if tree.root_node().has_error() {
        return Err(Error::Unsupported);
    }
    Ok(tree)
}
fn nodes<'a>(root: Node<'a>, budget: &Budget) -> Result<Vec<Node<'a>>, Error> {
    let mut pending = vec![root];
    let mut out = Vec::new();
    while let Some(node) = pending.pop() {
        if out.len() > 100_000 {
            return Err(Error::Limit);
        }
        if out.len() % 1000 == 0 {
            budget.check()?;
        }
        out.push(node);
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    Ok(out)
}
fn global_guard(tree: &Tree, src: &str, budget: &Budget) -> Result<(), Error> {
    for node in nodes(tree.root_node(), budget)? {
        if matches!(node.kind(), "attribute_item" | "inner_attribute_item" | "macro_definition" | "macro_invocation" | "use_wildcard") {
            return Err(Error::Context);
        }
        if matches!(node.kind(), "line_comment" | "block_comment") {
            let value = text(node, src)?;
            if value.starts_with("///") || value.starts_with("//!") || value.starts_with("/**") || value.starts_with("/*!") {
                return Err(Error::Context);
            }
        }
        if node.kind() == "use_declaration" {
            let value = text(node, src)?;
            if value.contains('*') {
                return Err(Error::Context);
            }
            for primitive in ["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool", "char", "str"] {
                let words: Vec<_> = value.split(|ch: char| !ch.is_alphanumeric() && ch != '_').filter(|word| !word.is_empty()).collect();
                if words.contains(&primitive) {
                    return Err(Error::Context);
                }
            }
        }
        if matches!(node.kind(), "struct_item" | "enum_item" | "union_item" | "trait_item" | "type_item" | "mod_item")
            && node.child_by_field_name("name").and_then(|name| text(name, src).ok()).is_some_and(primitive)
        {
            return Err(Error::Context);
        }
    }
    Ok(())
}
fn selected<'a>(tree: &'a Tree, rope: &Rope, caret: usize) -> Result<Node<'a>, Error> {
    if caret > rope.len_chars() {
        return Err(Error::Unsupported);
    }
    let byte = rope.char_to_byte(caret);
    let node = tree.root_node().descendant_for_byte_range(byte, byte).ok_or(Error::Unsupported)?;
    let mut current = Some(node);
    while let Some(node) = current {
        if matches!(node.kind(), "function_item" | "struct_item" | "enum_item" | "const_item" | "type_item") {
            if node.parent().is_some_and(|parent| parent.kind() == "source_file") {
                return Ok(node);
            }
            return Err(Error::Unsupported);
        }
        current = node.parent();
    }
    Err(Error::Unsupported)
}
fn shape(item: Node<'_>, src: &str, budget: &Budget) -> Result<Vec<usize>, Error> {
    let _ = visibility(item, src)?;
    if item.end_byte() - item.start_byte() > 64 * 1024 || item.child_by_field_name("type_parameters").is_some() {
        return Err(Error::Unsupported);
    }
    let name = item.child_by_field_name("name").ok_or(Error::Unsupported)?;
    if !ident(text(name, src)?) {
        return Err(Error::Unsupported);
    }
    let mut references = vec![name.start_byte()];
    let allowed = [
        "function_item",
        "struct_item",
        "enum_item",
        "const_item",
        "type_item",
        "visibility_modifier",
        "crate",
        "identifier",
        "type_identifier",
        "parameters",
        "parameter",
        "primitive_type",
        "block",
        "let_declaration",
        "mutable_specifier",
        "binary_expression",
        "unary_expression",
        "return_expression",
        "expression_statement",
        "integer_literal",
        "float_literal",
        "boolean_literal",
        "char_literal",
        "string_literal",
        "raw_string_literal",
        "escape_sequence",
        "unit_expression",
        "unit_type",
        "parenthesized_expression",
        "if_expression",
        "else_clause",
        "assignment_expression",
        "compound_assignment_expr",
        "while_expression",
        "loop_expression",
        "break_expression",
        "continue_expression",
        "empty_statement",
        "type_cast_expression",
        "wildcard_pattern",
        "line_comment",
        "block_comment",
        "field_declaration_list",
        "field_declaration",
        "field_identifier",
        "ordered_field_declaration_list",
        "enum_variant_list",
        "enum_variant",
    ];
    for node in nodes(item, budget)? {
        if !allowed.contains(&node.kind()) {
            return Err(Error::Context);
        }
        if node.kind() == "function_item" && node.id() != item.id() {
            return Err(Error::Context);
        }
        if node.kind() == "primitive_type" && !primitive(text(node, src)?) {
            return Err(Error::Context);
        }
        if matches!(node.kind(), "identifier" | "type_identifier") && node.id() != name.id() {
            if item.kind() == "const_item" && text(node, src)? == text(name, src)? {
                return Err(Error::Context);
            }
            references.push(node.start_byte());
        }
        if item.kind() == "struct_item" && node.kind() == "field_declaration" {
            visibility(node, src)?;
        }
        if item.kind() == "struct_item" && node.kind() == "ordered_field_declaration_list" {
            let mut cursor = node.walk();
            for ty in node.children_by_field_name("type", &mut cursor) {
                let vis = ty.prev_named_sibling().filter(|node| node.kind() == "visibility_modifier").ok_or(Error::Visibility)?;
                let value = text(vis, src)?.split_whitespace().collect::<String>();
                if !matches!(value.as_str(), "pub" | "pub(crate)") {
                    return Err(Error::Visibility);
                }
            }
        }
    }
    if references.len() > 128 {
        return Err(Error::Limit);
    }
    Ok(references)
}
fn module(root: &Tree, src: &str, name: &str) -> Result<(), Error> {
    let mut cursor = root.root_node().walk();
    let modules: Vec<_> = root
        .root_node()
        .named_children(&mut cursor)
        .filter(|node| node.kind() == "mod_item" && node.child_by_field_name("name").and_then(|name| text(name, src).ok()) == Some(name))
        .collect();
    if modules.len() != 1 || modules[0].child_by_field_name("body").is_some() {
        return Err(Error::Graph);
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Definition {
    path: PathBuf,
    start: rc_lsp::lsp_types::Position,
    end: rc_lsp::lsp_types::Position,
}
fn definitions(value: Value) -> Result<Vec<Definition>, Error> {
    let values = match value {
        Value::Array(values) => values,
        Value::Null => Vec::new(),
        value => vec![value],
    };
    if values.len() > 8 {
        return Err(Error::Limit);
    }
    let mut result = Vec::new();
    for value in values {
        let uri = value.get("targetUri").or_else(|| value.get("uri")).and_then(Value::as_str).ok_or(Error::Server)?;
        let path = uri.parse().ok().and_then(|uri| uri_to_path(&uri)).ok_or(Error::Server)?;
        let range = value.get("targetSelectionRange").or_else(|| value.get("range")).ok_or(Error::Server)?;
        let range: rc_lsp::lsp_types::Range = serde_json::from_value(range.clone()).map_err(|_| Error::Server)?;
        result.push(Definition { path, start: range.start, end: range.end });
    }
    result.sort();
    result.dedup();
    if result.len() != 1 {
        return Err(Error::Context);
    }
    Ok(result)
}
fn resolve(client: &Client, requests: &[(PathBuf, Rope, usize)], budget: &Budget) -> Result<Vec<Definition>, Error> {
    if requests.len() > 256 {
        return Err(Error::Limit);
    }
    let mut pending: Vec<_> = requests
        .iter()
        .map(|(path, rope, byte)| {
            let position = crate::convert::lsp_position(rope, rope.byte_to_char(*byte));
            client.request_raw::<Value>("textDocument/definition", json!({"textDocument":{"uri":path_to_uri(path).as_str()},"position":position}))
        })
        .collect();
    let mut result = vec![None; pending.len()];
    while result.iter().any(Option::is_none) {
        budget.check()?;
        for (index, request) in pending.iter_mut().enumerate() {
            if result[index].is_none()
                && let Some(value) = request.poll()
            {
                result[index] = Some(definitions(value.map_err(|_| Error::Server)?)?.into_iter().next().ok_or(Error::Server)?);
            }
        }
        if result.iter().any(Option::is_none) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    result.into_iter().collect::<Option<Vec<_>>>().ok_or(Error::Server)
}
fn scan(dir: &Path, budget: &Budget) -> Result<Vec<(PathBuf, Vec<u8>)>, Error> {
    let mut dirs = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    let mut total = 0;
    let mut seen = 0;
    while let Some(dir) = dirs.pop() {
        budget.check()?;
        for entry in std::fs::read_dir(&dir).map_err(|_| Error::Io)? {
            seen += 1;
            if seen > 2048 {
                return Err(Error::Limit);
            }
            let entry = entry.map_err(|_| Error::Io)?;
            let kind = entry.file_type().map_err(|_| Error::Io)?;
            if kind.is_symlink() {
                return Err(Error::Graph);
            }
            if kind.is_dir() {
                dirs.push(entry.path());
            } else if kind.is_file() && entry.path().extension().is_some_and(|ext| ext == "rs") {
                let bytes = read(&entry.path())?;
                total += bytes.len();
                if total > TOTAL || files.len() >= FILES {
                    return Err(Error::Limit);
                }
                files.push((entry.path(), bytes));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}
pub fn plan(
    origin: u64,
    caret: usize,
    target: PathBuf,
    frozen: Vec<Frozen>,
    workspace: rc_cargo::CargoWorkspace,
    client: &Client,
    budget: &Budget,
) -> Result<Plan, Error> {
    budget.check()?;
    let from = frozen.iter().find(|item| item.id == origin && !item.readonly).ok_or(Error::Changed)?;
    let to = frozen.iter().find(|item| item.path == target && !item.readonly).ok_or(Error::Changed)?;
    if from.path == to.path || from.path.parent() != to.path.parent() {
        return Err(Error::Graph);
    }
    let package = workspace
        .packages
        .iter()
        .filter(|package| package.manifest.parent().is_some_and(|dir| from.path.starts_with(dir)))
        .max_by_key(|package| package.manifest.components().count())
        .ok_or(Error::Graph)?;
    let roots: Vec<_> = package.targets.iter().filter(|target| matches!(target.kind, rc_cargo::TargetKind::Bin | rc_cargo::TargetKind::Lib)).collect();
    if roots.len() != 1 || package.targets.len() != 1 {
        return Err(Error::Graph);
    }
    let root = roots[0].src.clone();
    if root.parent() != from.path.parent() || !matches!(root.file_name().and_then(|name| name.to_str()), Some("main.rs" | "lib.rs")) {
        return Err(Error::Graph);
    }
    let parent = frozen.iter().find(|item| item.path == root).ok_or(Error::Graph)?;
    let from_name = from.path.file_stem().and_then(|name| name.to_str()).filter(|name| ident(name)).ok_or(Error::Graph)?;
    let to_name = to.path.file_stem().and_then(|name| name.to_str()).filter(|name| ident(name)).ok_or(Error::Graph)?;
    let manifest_bytes = read(&package.manifest)?;
    let manifest: toml::Value = toml::from_str(std::str::from_utf8(&manifest_bytes).map_err(|_| Error::Graph)?).map_err(|_| Error::Graph)?;
    if ["lib", "bin", "test", "bench", "example"].iter().any(|key| manifest.get(*key).is_some())
        || manifest.get("package").is_none_or(|package| {
            ["autobins", "autolib"].iter().any(|key| package.get(*key).and_then(toml::Value::as_bool) == Some(false))
                || package.get("build").is_some_and(|build| build.as_bool() != Some(false))
        })
    {
        return Err(Error::Graph);
    }
    let package_root = package.manifest.parent().ok_or(Error::Graph)?.to_path_buf();
    layout(&package_root, &root)?;
    let mut disk = vec![Disk { path: package.manifest.clone(), bytes: Arc::from(manifest_bytes) }];
    if workspace.manifest != package.manifest {
        disk.push(Disk { path: workspace.manifest.clone(), bytes: Arc::from(read(&workspace.manifest)?) });
    }
    let mut sources = BTreeMap::new();
    for (path, bytes) in scan(root.parent().ok_or(Error::Graph)?, budget)? {
        disk.push(Disk { path: path.clone(), bytes: Arc::from(bytes.clone()) });
        let rope = match frozen.iter().find(|item| item.path == path) {
            Some(item) => item.text.clone(),
            None => source(&bytes)?,
        };
        sources.insert(path, rope);
    }
    for item in [&from, &to, &parent] {
        if !sources.contains_key(&item.path) {
            return Err(Error::Graph);
        }
    }
    if disk.iter().find(|disk| disk.path == root).and_then(|disk| source(&disk.bytes).ok()).is_none_or(|disk| disk != parent.text) {
        return Err(Error::Changed);
    }
    let original = from.text.to_string();
    let target_text = to.text.to_string();
    let parent_text = parent.text.to_string();
    let source_tree = parse(&from.text)?;
    parse(&to.text)?;
    let parent_tree = parse(&parent.text)?;
    module(&parent_tree, &parent_text, from_name)?;
    module(&parent_tree, &parent_text, to_name)?;
    let item = selected(&source_tree, &from.text, caret)?;
    let refs = shape(item, &original, budget)?;
    let name_node = item.child_by_field_name("name").ok_or(Error::Unsupported)?;
    let name = text(name_node, &original)?.to_string();
    let visibility = visibility(item, &original)?;
    let mut requests: Vec<_> = refs.iter().map(|byte| (from.path.clone(), from.text.clone(), *byte)).collect();
    let local_count = requests.len();
    for (path, rope) in &sources {
        budget.check()?;
        let content = rope.to_string();
        let tree = parse(rope)?;
        global_guard(&tree, &content, budget)?;
        for node in nodes(tree.root_node(), budget)? {
            if matches!(node.kind(), "identifier" | "type_identifier") && text(node, &content)? == name {
                if *path == to.path {
                    return Err(Error::Collision);
                }
                if *path == from.path && node.start_byte() >= item.start_byte() && node.end_byte() <= item.end_byte() {
                    continue;
                }
                requests.push((path.clone(), rope.clone(), node.start_byte()));
            }
        }
    }
    let resolved = resolve(client, &requests, budget)?;
    let expected = Definition {
        path: from.path.clone(),
        start: crate::convert::lsp_position(&from.text, from.text.byte_to_char(name_node.start_byte())),
        end: crate::convert::lsp_position(&from.text, from.text.byte_to_char(name_node.end_byte())),
    };
    if resolved.first() != Some(&expected) {
        return Err(Error::Context);
    }
    let start = crate::convert::lsp_position(&from.text, from.text.byte_to_char(item.start_byte()));
    let end = crate::convert::lsp_position(&from.text, from.text.byte_to_char(item.end_byte()));
    for definition in resolved.iter().take(local_count) {
        if definition.path != from.path || definition.start < start || definition.end > end {
            return Err(Error::Context);
        }
    }
    let body = text(item, &original)?.to_string();
    let replacement = format!("{visibility} use super::{to_name}::{name};");
    let source_start = from.text.byte_to_char(item.start_byte());
    let source_end = from.text.byte_to_char(item.end_byte());
    let mut after_source = original.clone();
    after_source.replace_range(item.byte_range(), &replacement);
    let append = format!("\n\n{body}\n");
    let after_target = format!("{target_text}{append}");
    parse(&Rope::from_str(&after_source))?;
    parse(&Rope::from_str(&after_target))?;
    budget.check()?;
    let preview = |text: &str| text.chars().take(16384).collect::<String>();
    let edits = vec![
        Edit {
            id: from.id,
            path: from.path.clone(),
            start: source_start,
            end: source_end,
            text: replacement,
            before: preview(&original),
            after: preview(&after_source),
        },
        Edit {
            id: to.id,
            path: to.path.clone(),
            start: to.text.len_chars(),
            end: to.text.len_chars(),
            text: append,
            before: preview(&target_text),
            after: preview(&after_target),
        },
    ];
    Ok(Plan { name, edits, frozen, disk, layout: Some((package_root, root)) })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn budget() -> Budget {
        Budget { cancel: Arc::new(AtomicBool::new(false)), deadline: Instant::now() + Duration::from_secs(5) }
    }
    #[test]
    fn supports_public_primitive_items_and_refuses_private_or_contextual_shapes() {
        for code in [
            "pub fn score(n:i32)->i32{let x=n*2;x+1}",
            "pub(crate) fn score(n:i32)->i32{n+1}",
            "pub struct Point {pub x:i32,pub(crate) y:i32}",
            "pub struct Marker;",
            "pub enum Mode {Fast,Slow}",
            "pub const OFFSET:i32=3;",
            "pub type Count=i32;",
        ] {
            let rope = Rope::from_str(code);
            let tree = parse(&rope).unwrap();
            let item = selected(&tree, &rope, 4).unwrap();
            assert!(shape(item, code, &budget()).is_ok(), "{code}: {:?}", shape(item, code, &budget()));
        }
        for (code, error) in [
            ("fn score(n:i32)->i32{n}", Error::Visibility),
            ("pub struct Point {x:i32}", Error::Visibility),
            ("pub fn score(n:i32)->i32{helper(n)}", Error::Context),
            ("pub fn score<T>(n:T)->T{n}", Error::Unsupported),
            ("pub fn score()->i32{some::CONSTANT}", Error::Context),
        ] {
            let rope = Rope::from_str(code);
            let tree = parse(&rope).unwrap();
            let item = selected(&tree, &rope, code.find("score").unwrap_or(4)).unwrap();
            assert_eq!(shape(item, code, &budget()).unwrap_err(), error, "{code}");
        }
        for code in ["use some::*;", "macro_rules! m {()=>{}}", "fn main(){println!(\"x\");}", "#[cfg(test)] fn f(){}", "/// doc\npub fn f(){}", "type i32=u8;"]
        {
            let rope = Rope::from_str(code);
            assert_eq!(global_guard(&parse(&rope).unwrap(), code, &budget()), Err(Error::Context), "{code}");
        }
    }
    #[test]
    fn disk_verification_refuses_external_changes_and_cancellation_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.rs");
        std::fs::write(&path, "pub const VALUE:i32=1;").unwrap();
        let plan = Plan {
            name: "VALUE".into(),
            edits: Vec::new(),
            frozen: Vec::new(),
            layout: None,
            disk: vec![Disk { path: path.clone(), bytes: Arc::from(read(&path).unwrap()) }],
        };
        verify(&plan, &budget()).unwrap();
        std::fs::write(&path, "pub const VALUE:i32=2;").unwrap();
        assert_eq!(verify(&plan, &budget()), Err(Error::Changed));
        let budget = budget();
        budget.cancel.store(true, Ordering::Release);
        assert_eq!(verify(&plan, &budget), Err(Error::Cancelled));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "pub const VALUE:i32=2;");
    }
}
