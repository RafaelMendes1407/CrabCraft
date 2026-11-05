//! Bounded trait graph collection off the UI thread; names are joined by RA definition identity.
use crate::{convert, docs::Docs, insight::Target};
use rc_core::{Inbox, Waker};
use rc_lsp::{Client, path_to_uri, uri_to_path};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const FILE_LIMIT: usize = 2 * 1024 * 1024;
const TOTAL_LIMIT: usize = 32 * 1024 * 1024;
const FILE_COUNT: usize = 1024;
const TRAIT_COUNT: usize = 2048;
const EDGE_COUNT: usize = 4096;
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    path: PathBuf,
    line: u32,
    column: u32,
}
#[derive(Clone, Debug)]
pub struct Declaration {
    key: Key,
    pub target: Target,
    pub name: String,
    bounds: Vec<usize>,
}
#[derive(Debug)]
pub enum Error {
    Cancelled,
    Timeout,
    Limit,
    Io,
    Syntax,
    Server,
    Changed,
}
#[derive(Clone)]
struct Overlay {
    id: u64,
    version: u64,
    path: PathBuf,
    text: rc_text::Rope,
}
pub struct ResultTree {
    pub root: Declaration,
    pub supertraits: Vec<Relation>,
    pub subtraits: Vec<Relation>,
    pub implementations: Vec<Target>,
}
pub struct Relation {
    pub declaration: Declaration,
    pub children: Vec<Relation>,
}
pub struct Job {
    inbox: Inbox<Result<ResultTree, Error>>,
    cancel: Arc<AtomicBool>,
    overlays: Vec<Overlay>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Job {
    pub fn start(
        root: PathBuf,
        origin: Target,
        client: Client,
        docs: &Docs,
        waker: Waker,
    ) -> Result<Self, Error> {
        let overlays: Vec<_> = docs
            .list
            .iter()
            .filter(|d| !d.generated && !d.scratch && d.editor.lang() == rc_syntax::Lang::Rust)
            .map(|d| Overlay {
                id: d.editor.id,
                version: d.editor.buffer.version(),
                path: d.path.clone(),
                text: d.editor.buffer.snapshot(),
            })
            .collect();
        if overlays.len() > FILE_COUNT
            || overlays.iter().map(|d| d.text.len_bytes()).sum::<usize>() > TOTAL_LIMIT
        {
            return Err(Error::Limit);
        }
        let inbox = Inbox::new(waker);
        let sender = inbox.sender();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let source = overlays.clone();
        std::thread::Builder::new()
            .name("rc-trait-hierarchy".into())
            .spawn(move || {
                let budget = Budget {
                    cancelled: token,
                    deadline: Instant::now() + Duration::from_secs(30),
                };
                let result = collect(&root, origin, &client, source, &budget);
                if budget.check().is_ok() {
                    sender.send(result);
                } else if !budget.cancelled.load(Ordering::Acquire) {
                    sender.send(Err(Error::Timeout));
                }
            })
            .map_err(|_| Error::Io)?;
        Ok(Self {
            inbox,
            cancel,
            overlays,
        })
    }
    pub fn current(&self, docs: &Docs) -> bool {
        self.overlays.iter().all(|old| {
            docs.list.iter().any(|d| {
                d.editor.id == old.id
                    && d.editor.buffer.version() == old.version
                    && d.path == old.path
            })
        }) && docs
            .list
            .iter()
            .filter(|d| !d.generated && !d.scratch && d.editor.lang() == rc_syntax::Lang::Rust)
            .count()
            == self.overlays.len()
    }
    pub fn poll(&self) -> Option<Result<ResultTree, Error>> {
        self.inbox.drain().into_iter().next()
    }
}
struct Budget {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}
impl Budget {
    fn check(&self) -> Result<(), Error> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(Error::Timeout)
        } else {
            Ok(())
        }
    }
    fn request(&self, client: &Client, method: &str, params: Value) -> Result<Value, Error> {
        self.check()?;
        let mut pending = client.request_raw(method, params);
        loop {
            self.check()?;
            if let Some(reply) = pending.poll() {
                return reply.map_err(|_| Error::Server);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
fn read(path: &Path, budget: &Budget) -> Result<String, Error> {
    budget.check()?;
    let info = std::fs::symlink_metadata(path).map_err(|_| Error::Io)?;
    if !info.is_file() || info.len() > FILE_LIMIT as u64 {
        return Err(Error::Limit);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|_| Error::Io)?;
    let opened = file.metadata().map_err(|_| Error::Io)?;
    if !opened.is_file() || opened.len() > FILE_LIMIT as u64 {
        return Err(Error::Limit);
    }
    let mut bytes = Vec::new();
    let mut chunk = [0; 16384];
    loop {
        budget.check()?;
        let count = file.read(&mut chunk).map_err(|_| Error::Io)?;
        if count == 0 {
            break;
        }
        if count > FILE_LIMIT.saturating_sub(bytes.len()) {
            return Err(Error::Limit);
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(bytes)
        .map(|text| {
            text.trim_start_matches('\u{feff}')
                .replace("\r\n", "\n")
                .replace('\r', "\n")
        })
        .map_err(|_| Error::Syntax)
}
fn files(root: &Path, budget: &Budget) -> Result<Vec<PathBuf>, Error> {
    let mut directories = vec![root.to_path_buf()];
    let mut files = Vec::new();
    let mut visited = 0;
    while let Some(directory) = directories.pop() {
        budget.check()?;
        for entry in std::fs::read_dir(directory).map_err(|_| Error::Io)? {
            budget.check()?;
            visited += 1;
            if visited > 100_000 {
                return Err(Error::Limit);
            }
            let entry = entry.map_err(|_| Error::Io)?;
            let kind = entry.file_type().map_err(|_| Error::Io)?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !matches!(
                    entry.file_name().to_str(),
                    Some("target" | ".git" | ".hg" | "node_modules" | ".idea")
                ) {
                    directories.push(entry.path());
                }
            } else if kind.is_file() && entry.path().extension().is_some_and(|x| x == "rs") {
                files.push(entry.path());
                if files.len() > FILE_COUNT {
                    return Err(Error::Limit);
                }
            }
        }
    }
    files.sort();
    Ok(files)
}
fn parse(path: &Path, text: &str, budget: &Budget) -> Result<Vec<Declaration>, Error> {
    budget.check()?;
    let rope = rc_text::Rope::from_str(text);
    let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, &rope, None).ok_or(Error::Syntax)?;
    let mut stack = vec![tree.root_node()];
    let mut result = Vec::new();
    let mut visited = 0;
    while let Some(node) = stack.pop() {
        budget.check()?;
        visited += 1;
        if visited > 500_000 {
            return Err(Error::Limit);
        }
        if node.kind() == "trait_item" {
            let Some(name) = node.child_by_field_name("name") else {
                continue;
            };
            let position = convert::lsp_position(&rope, rope.byte_to_char(name.start_byte()));
            let mut bounds = Vec::new();
            if let Some(bound) = node.child_by_field_name("bounds") {
                bound_positions(bound, &rope, &mut bounds);
            }
            let mut cursor = node.walk();
            for child in node
                .named_children(&mut cursor)
                .filter(|n| n.kind() == "where_clause")
            {
                let mut cursor = child.walk();
                for predicate in child.named_children(&mut cursor) {
                    if predicate
                        .child_by_field_name("left")
                        .and_then(|n| text.get(n.byte_range()))
                        == Some("Self")
                    {
                        if let Some(bound) = predicate.child_by_field_name("bounds") {
                            bound_positions(bound, &rope, &mut bounds);
                        }
                    }
                }
            }
            result.push(Declaration {
                key: Key {
                    path: path.to_path_buf(),
                    line: position.line,
                    column: position.character,
                },
                target: Target {
                    path: path.to_path_buf(),
                    line: position.line as usize,
                    col: rope.byte_to_char(name.start_byte())
                        - rope.line_to_char(position.line as usize),
                    preview: rope
                        .line(position.line as usize)
                        .to_string()
                        .trim_end()
                        .to_owned(),
                },
                name: text.get(name.byte_range()).ok_or(Error::Syntax)?.into(),
                bounds,
            });
            if result.len() > TRAIT_COUNT {
                return Err(Error::Limit);
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    Ok(result)
}
fn bound_positions(
    node: rc_syntax::tree_sitter::Node<'_>,
    rope: &rc_text::Rope,
    positions: &mut Vec<usize>,
) {
    // Resolve the path's final identifier, never a generic argument or a lifetime.
    let mut cursor = node.walk();
    for bound in node.named_children(&mut cursor) {
        let mut ty = bound;
        if ty.kind() == "higher_ranked_trait_bound" {
            if let Some(inner) = ty.child_by_field_name("type") {
                ty = inner;
            }
        }
        if ty.kind() == "generic_type" {
            if let Some(inner) = ty.child_by_field_name("type") {
                ty = inner;
            }
        }
        if ty.kind() == "scoped_type_identifier" {
            if let Some(inner) = ty.child_by_field_name("name") {
                ty = inner;
            }
        }
        if matches!(ty.kind(), "type_identifier" | "identifier") {
            positions.push(rope.byte_to_char(ty.start_byte()));
        }
    }
}
fn positions(value: &Value) -> Result<Vec<Key>, Error> {
    let entries: Vec<&Value> = match value {
        Value::Null => Vec::new(),
        Value::Array(items) => {
            if items.len() > EDGE_COUNT {
                return Err(Error::Limit);
            }
            items.iter().collect()
        }
        other => vec![other],
    };
    if entries.len() > EDGE_COUNT {
        return Err(Error::Limit);
    }
    let mut keys = Vec::new();
    for entry in entries {
        let uri = entry
            .get("targetUri")
            .or_else(|| entry.get("uri"))
            .and_then(Value::as_str)
            .ok_or(Error::Server)?;
        let range = entry
            .get("targetSelectionRange")
            .or_else(|| entry.get("range"))
            .ok_or(Error::Server)?;
        let uri = uri.parse().map_err(|_| Error::Server)?;
        let path = uri_to_path(&uri).ok_or(Error::Server)?;
        let position: rc_lsp::lsp_types::Position =
            serde_json::from_value(range.get("start").cloned().ok_or(Error::Server)?)
                .map_err(|_| Error::Server)?;
        keys.push(Key {
            path,
            line: position.line,
            column: position.character,
        });
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}
fn params(path: &Path, text: &rc_text::Rope, position: usize) -> Value {
    json!({"textDocument":{"uri":path_to_uri(path).as_str()},"position":convert::lsp_position(text, position)})
}
fn ensure_source(
    path: &Path,
    sources: &mut BTreeMap<PathBuf, rc_text::Rope>,
    budget: &Budget,
) -> Result<(), Error> {
    if sources.contains_key(path) {
        return Ok(());
    }
    if sources.len() >= FILE_COUNT {
        return Err(Error::Limit);
    }
    let text = read(path, budget)?;
    let used: usize = sources.values().map(|source| source.len_bytes()).sum();
    if text.len() > TOTAL_LIMIT.saturating_sub(used) {
        return Err(Error::Limit);
    }
    sources.insert(path.to_path_buf(), rc_text::Rope::from_str(&text));
    Ok(())
}
fn target(
    key: &Key,
    sources: &mut BTreeMap<PathBuf, rc_text::Rope>,
    budget: &Budget,
) -> Result<Target, Error> {
    ensure_source(&key.path, sources, budget)?;
    let rope = sources.get(&key.path).ok_or(Error::Io)?;
    if key.line as usize >= rope.len_lines() {
        return Err(Error::Server);
    }
    let position = rc_lsp::lsp_types::Position {
        line: key.line,
        character: key.column,
    };
    let character = convert::char_of(rope, position);
    Ok(Target {
        path: key.path.clone(),
        line: key.line as usize,
        col: character - rope.line_to_char(key.line as usize),
        preview: rope
            .line(key.line as usize)
            .to_string()
            .trim_end()
            .to_string(),
    })
}
fn collect(
    root: &Path,
    origin: Target,
    client: &Client,
    overlays: Vec<Overlay>,
    budget: &Budget,
) -> Result<ResultTree, Error> {
    let mut sources: BTreeMap<PathBuf, rc_text::Rope> = overlays
        .iter()
        .map(|d| (d.path.clone(), d.text.clone()))
        .collect();
    let origin_text = sources.get(&origin.path).ok_or(Error::Io)?;
    if origin.line >= origin_text.len_lines() {
        return Err(Error::Syntax);
    }
    let origin_pos = origin_text.line_to_char(origin.line) + origin.col;
    if origin_pos > origin_text.len_chars() {
        return Err(Error::Syntax);
    }
    let resolved = positions(&budget.request(
        client,
        "textDocument/definition",
        params(&origin.path, origin_text, origin_pos),
    )?)?;
    let implementations = positions(&budget.request(
        client,
        "textDocument/implementation",
        params(&origin.path, origin_text, origin_pos),
    )?)?;
    let mut declarations = BTreeMap::new();
    let mut total = 0;
    let mut paths: BTreeSet<PathBuf> = files(root, budget)?.into_iter().collect();
    paths.extend(
        overlays
            .iter()
            .filter(|d| d.path.starts_with(root))
            .map(|d| d.path.clone()),
    );
    for path in paths {
        budget.check()?;
        ensure_source(&path, &mut sources, budget)?;
        let source = sources.get(&path).ok_or(Error::Io)?;
        if source.len_bytes() > FILE_LIMIT {
            return Err(Error::Limit);
        }
        total += source.len_bytes();
        if total > TOTAL_LIMIT {
            return Err(Error::Limit);
        }
        for declaration in parse(&path, &source.to_string(), budget)? {
            declarations.insert(declaration.key.clone(), declaration);
        }
        if declarations.len() > TRAIT_COUNT {
            return Err(Error::Limit);
        }
    }
    let root_key = resolved
        .into_iter()
        .find(|key| declarations.contains_key(key))
        .or_else(|| {
            declarations
                .keys()
                .find(|key| {
                    key.path == origin.path
                        && key.line as usize == origin.line
                        && sources.get(&key.path).is_some_and(|rope| {
                            let char_start = convert::char_of(
                                rope,
                                rc_lsp::lsp_types::Position {
                                    line: key.line,
                                    character: key.column,
                                },
                            );
                            let len = declarations.get(*key).map_or(0, |d| d.name.chars().count());
                            origin_pos >= char_start && origin_pos <= char_start + len
                        })
                })
                .cloned()
        });
    let Some(root_key) = root_key else {
        // Structs/enums retain the implementation list, but resolve preview I/O here as well.
        let implementations = implementations
            .iter()
            .map(|key| target(key, &mut sources, budget))
            .collect::<Result<Vec<_>, _>>()?;
        let name = origin.preview.clone();
        let key = Key {
            path: origin.path.clone(),
            line: origin.line as u32,
            column: origin.col as u32,
        };
        validate_closed(&sources, &overlays, budget)?;
        return Ok(ResultTree {
            root: Declaration {
                key,
                target: origin,
                name,
                bounds: Vec::new(),
            },
            supertraits: Vec::new(),
            subtraits: Vec::new(),
            implementations,
        });
    };
    let mut edges: BTreeMap<Key, Vec<Key>> = BTreeMap::new();
    let mut external = BTreeMap::new();
    let mut count = 0;
    for (key, declaration) in &declarations {
        let source = sources.get(&key.path).ok_or(Error::Io)?.clone();
        for position in &declaration.bounds {
            budget.check()?;
            count += 1;
            if count > EDGE_COUNT {
                return Err(Error::Limit);
            }
            let definitions = positions(&budget.request(
                client,
                "textDocument/definition",
                params(&key.path, &source, *position),
            )?)?;
            for destination in definitions {
                if !declarations.contains_key(&destination) && !external.contains_key(&destination)
                {
                    let location = target(&destination, &mut sources, budget)?;
                    let source = sources.get(&destination.path).ok_or(Error::Io)?;
                    let found = parse(&destination.path, &source.to_string(), budget)?
                        .into_iter()
                        .find(|d| d.key == destination);
                    if let Some(found) = found {
                        external.insert(destination.clone(), found);
                    } else {
                        continue;
                    }
                    let _ = location;
                }
                edges.entry(key.clone()).or_default().push(destination);
            }
        }
    }
    declarations.extend(external);
    let mut reverse: BTreeMap<Key, Vec<Key>> = BTreeMap::new();
    for (from, destinations) in &mut edges {
        destinations.sort();
        destinations.dedup();
        for destination in destinations {
            reverse
                .entry(destination.clone())
                .or_default()
                .push(from.clone());
        }
    }
    let mut nodes = 0;
    fn relations(
        key: &Key,
        graph: &BTreeMap<Key, Vec<Key>>,
        declarations: &BTreeMap<Key, Declaration>,
        path: &mut BTreeSet<Key>,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<Vec<Relation>, Error> {
        if depth > 16 {
            return Err(Error::Limit);
        }
        let mut result = Vec::new();
        for destination in graph.get(key).into_iter().flatten() {
            if path.contains(destination) {
                continue;
            }
            *nodes += 1;
            if *nodes > TRAIT_COUNT {
                return Err(Error::Limit);
            }
            let declaration = declarations.get(destination).ok_or(Error::Server)?.clone();
            path.insert(destination.clone());
            let children = relations(destination, graph, declarations, path, depth + 1, nodes)?;
            path.remove(destination);
            result.push(Relation {
                declaration,
                children,
            });
        }
        result.sort_by(|a, b| a.declaration.name.cmp(&b.declaration.name));
        Ok(result)
    }
    let mut path = BTreeSet::from([root_key.clone()]);
    let supertraits = relations(&root_key, &edges, &declarations, &mut path, 0, &mut nodes)?;
    let subtraits = relations(&root_key, &reverse, &declarations, &mut path, 0, &mut nodes)?;
    let implementations = implementations
        .iter()
        .map(|key| target(key, &mut sources, budget))
        .collect::<Result<Vec<_>, _>>()?;
    validate_closed(&sources, &overlays, budget)?;
    Ok(ResultTree {
        root: declarations.remove(&root_key).ok_or(Error::Server)?,
        supertraits,
        subtraits,
        implementations,
    })
}

fn validate_closed(
    sources: &BTreeMap<PathBuf, rc_text::Rope>,
    overlays: &[Overlay],
    budget: &Budget,
) -> Result<(), Error> {
    // Re-read closed sources before publishing: disk changes cannot silently join two graphs.
    let open_paths: BTreeSet<_> = overlays.iter().map(|overlay| &overlay.path).collect();
    for (path, source) in sources {
        budget.check()?;
        if !open_paths.contains(path) && read(path, budget)? != source.to_string() {
            return Err(Error::Changed);
        }
    }
    Ok(())
}

pub fn into_call_tree(result: ResultTree) -> crate::call_tree::CallTree {
    use crate::call_tree::{CallNode, CallTree};
    use rc_kit::i18n::tr;
    fn group(name: &str, children: Vec<CallNode>) -> CallNode {
        let mut node = CallNode::new(json!({"name":name}), None, 1);
        node.children = Some(children);
        node.open = node
            .children
            .as_ref()
            .is_some_and(|children| !children.is_empty());
        node
    }
    let mut children = Vec::new();
    let related = !result.supertraits.is_empty() || !result.subtraits.is_empty();
    if !result.supertraits.is_empty() {
        children.push(group(
            tr("Supertraits"),
            result.supertraits.into_iter().map(relation_node).collect(),
        ));
    }
    if !result.subtraits.is_empty() {
        children.push(group(
            tr("Subtraits"),
            result.subtraits.into_iter().map(relation_node).collect(),
        ));
    }
    let implementations: Vec<_> = result.implementations.into_iter().map(|target| {
        let mut node = CallNode::new(json!({"name":target.preview.split('{').next().unwrap_or("").trim(),"detail":format!("{}:{}",target.path.display(),target.line+1)}), Some(target), 1); node.children = Some(Vec::new()); node
    }).collect();
    if related {
        if !implementations.is_empty() {
            children.push(group(tr("Implementations"), implementations));
        }
    } else {
        children.extend(implementations);
    }
    let mut root = CallNode::new(
        json!({"name":result.root.name,"trait_hierarchy":related}),
        Some(result.root.target),
        1,
    );
    root.children = Some(children);
    root.open = true;
    CallTree {
        callers: false,
        types: true,
        root,
    }
}
fn relation_node(relation: Relation) -> crate::call_tree::CallNode {
    use crate::call_tree::CallNode;
    let mut node = CallNode::new(
        json!({"name":relation.declaration.name,"detail":format!("{}:{}",relation.declaration.target.path.display(),relation.declaration.target.line+1)}),
        Some(relation.declaration.target),
        1,
    );
    node.children = Some(relation.children.into_iter().map(relation_node).collect());
    node.open = node
        .children
        .as_ref()
        .is_some_and(|children| !children.is_empty());
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    fn budget() -> Budget {
        Budget {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + Duration::from_secs(10),
        }
    }
    #[test]
    fn parses_only_supertrait_paths_not_generic_arguments_or_method_bounds() {
        let source = "trait Child<T>: module::Base<T> + for<'a> Other<'a> + 'static where Self: Third, T: NotSuper { fn f<U: AlsoNotSuper>(); }";
        let result = parse(Path::new("/project/lib.rs"), source, &budget()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Child");
        let words: Vec<_> = result[0]
            .bounds
            .iter()
            .map(|p| {
                source
                    .chars()
                    .skip(*p)
                    .take_while(|c| c.is_alphanumeric())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(words, ["Base", "Other", "Third"]);
        let token = budget();
        token.cancelled.store(true, Ordering::Release);
        assert!(matches!(
            parse(Path::new("/p.rs"), source, &token),
            Err(Error::Cancelled)
        ));
        assert!(matches!(
            read(Path::new("/nonexistent"), &token),
            Err(Error::Cancelled)
        ));
    }
}

#[cfg(all(test, unix))]
mod special_file_tests {
    use super::*;
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
    #[test]
    fn fifo_and_symlink_sources_are_rejected_without_waiting_for_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("source.rs");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let path = fifo.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let budget = Budget {
                cancelled: Arc::new(AtomicBool::new(false)),
                deadline: Instant::now() + Duration::from_secs(2),
            };
            sender.send(read(&path, &budget)).unwrap();
        });
        let result = receiver.recv_timeout(Duration::from_millis(500));
        if result.is_err() {
            // Rescue a regressed blocking FIFO open before failing the assertion.
            let _rescue = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo);
            let _ = receiver.recv_timeout(Duration::from_secs(2));
        }
        worker.join().unwrap();
        assert!(matches!(result, Ok(Err(Error::Limit))));
        let regular = dir.path().join("regular.rs");
        std::fs::write(&regular, "trait Real {}\n").unwrap();
        let link = dir.path().join("link.rs");
        std::os::unix::fs::symlink(&regular, &link).unwrap();
        let budget = Budget {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + Duration::from_secs(2),
        };
        assert!(matches!(read(&link, &budget), Err(Error::Limit)));
    }
}
