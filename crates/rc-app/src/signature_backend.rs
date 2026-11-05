//! Guarded workspace snapshots and semantic identity for Change Signature. Worker threads only.
use crate::{convert, disk_revert::Ticket, signature_model as model};
use rc_lsp::{Client, lsp_types as lsp, path_to_uri, uri_to_path};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
const FILE_BYTES: usize = 2 * 1024 * 1024;
const TOTAL_BYTES: usize = 16 * 1024 * 1024;
const FILES: usize = 256;
const REFERENCES: usize = 4096;
#[derive(Debug)]
pub enum Error {
    Io,
    Limit,
    Stale,
    Unsupported,
    FunctionValue,
    UsedParameter,
    NameConflict,
    Analysis,
    Cancelled,
    NoChanges,
    Model(model::Error),
}
impl From<model::Error> for Error {
    fn from(error: model::Error) -> Self {
        Self::Model(error)
    }
}
#[derive(Clone)]
pub struct Overlay {
    pub ticket: Ticket,
    pub rope: rc_text::Rope,
    pub saved: rc_text::Rope,
    pub bom: bool,
    pub ending: rc_text::LineEnding,
    pub modified: Option<SystemTime>,
    pub server_version: Option<i32>,
}
#[derive(Clone)]
pub struct Capture {
    pub root: PathBuf,
    pub members: Vec<PathBuf>,
    pub overlays: Vec<Overlay>,
    pub client: Client,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl Stamp {
    fn of(metadata: &std::fs::Metadata) -> Result<Self, Error> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified().map_err(|_| Error::Io)?,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}
#[derive(Clone)]
struct Disk {
    raw: Arc<str>,
    bom: bool,
    stamp: Stamp,
}
#[derive(Clone)]
pub struct File {
    pub path: PathBuf,
    pub raw: Arc<str>,
    pub normal: Arc<str>,
    pub rope: rc_text::Rope,
    pub bom: bool,
    pub modified: SystemTime,
    pub open: Option<Overlay>,
    disk: Disk,
}
impl File {
    fn byte(&self, mut position: lsp::Position) -> Result<usize, Error> {
        if self.bom && self.open.is_none() && position.line == 0 {
            position.character = position.character.checked_sub(1).ok_or(Error::Analysis)?;
        }
        if position.line as usize >= self.rope.len_lines() {
            return Err(Error::Analysis);
        }
        let character = convert::char_of(&self.rope, position);
        if convert::lsp_position(&self.rope, character) != position {
            return Err(Error::Analysis);
        }
        Ok(self.rope.char_to_byte(character))
    }
    fn range(&self, range: lsp::Range) -> Result<Range<usize>, Error> {
        let start = self.byte(range.start)?;
        let end = self.byte(range.end)?;
        if end < start {
            return Err(Error::Analysis);
        }
        Ok(start..end)
    }
    fn key(&self, byte: usize) -> Result<Key, Error> {
        if byte > self.normal.len() || !self.normal.is_char_boundary(byte) {
            return Err(Error::Analysis);
        }
        let mut position = convert::lsp_position(&self.rope, self.rope.byte_to_char(byte));
        if self.bom && self.open.is_none() && position.line == 0 {
            position.character = position.character.checked_add(1).ok_or(Error::Analysis)?;
        }
        Ok(Key {
            path: self.path.clone(),
            line: position.line,
            column: position.character,
        })
    }
}
#[derive(Clone)]
pub struct Project {
    pub capture: Capture,
    pub files: BTreeMap<PathBuf, File>,
    paths: BTreeSet<PathBuf>,
    configs: Vec<(PathBuf, Option<Disk>)>,
}
pub struct Context {
    pub cancel: Arc<AtomicBool>,
    pub client: Client,
    deadline: Instant,
}
impl Context {
    pub fn new(client: Client, cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            client,
            deadline: Instant::now() + Duration::from_secs(45),
        }
    }
    pub fn check(&self) -> Result<(), Error> {
        if self.cancel.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(Error::Limit)
        } else {
            Ok(())
        }
    }
    fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        self.check()?;
        let mut pending = self.client.request_raw(method, params);
        loop {
            self.check()?;
            if let Some(result) = pending.poll() {
                return result.map_err(|_| Error::Analysis);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn definitions(&self, key: &Key) -> Result<Vec<Key>, Error> {
        keys(&self.request("textDocument/definition", params(key))?)
    }
    fn references(&self, key: &Key) -> Result<Vec<Key>, Error> {
        let mut query = params(key);
        query["context"] = json!({"includeDeclaration":false});
        keys(&self.request("textDocument/references", query)?)
    }
}
fn params(key: &Key) -> Value {
    json!({"textDocument":{"uri":path_to_uri(&key.path)},"position":{"line":key.line,"character":key.column}})
}
fn keys(value: &Value) -> Result<Vec<Key>, Error> {
    let values: Vec<&Value> = match value {
        Value::Null => Vec::new(),
        Value::Array(items) => {
            if items.len() > REFERENCES {
                return Err(Error::Limit);
            }
            items.iter().collect()
        }
        other => vec![other],
    };
    let mut result = Vec::new();
    for value in values {
        let uri = value
            .get("targetUri")
            .or_else(|| value.get("uri"))
            .and_then(Value::as_str)
            .ok_or(Error::Analysis)?
            .parse()
            .map_err(|_| Error::Analysis)?;
        let path = uri_to_path(&uri)
            .ok_or(Error::Unsupported)?
            .canonicalize()
            .map_err(|_| Error::Io)?;
        let range = value
            .get("targetSelectionRange")
            .or_else(|| value.get("range"))
            .ok_or(Error::Analysis)?;
        let start: lsp::Position =
            serde_json::from_value(range.get("start").cloned().ok_or(Error::Analysis)?)
                .map_err(|_| Error::Analysis)?;
        result.push(Key {
            path,
            line: start.line,
            column: start.character,
        });
    }
    result.sort();
    result.dedup();
    Ok(result)
}
fn read(path: &Path, context: &Context) -> Result<Disk, Error> {
    context.check()?;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| Error::Io)?;
    if !metadata.is_file() || metadata.len() > FILE_BYTES as u64 {
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
    let metadata = file.metadata().map_err(|_| Error::Io)?;
    if !metadata.is_file() {
        return Err(Error::Unsupported);
    }
    let stamp = Stamp::of(&metadata)?;
    if stamp.length > FILE_BYTES as u64 {
        return Err(Error::Limit);
    }
    let mut bytes = Vec::new();
    let mut chunk = [0; 16384];
    loop {
        context.check()?;
        let count = file.read(&mut chunk).map_err(|_| Error::Io)?;
        if count == 0 {
            break;
        }
        if count > FILE_BYTES.saturating_sub(bytes.len()) {
            return Err(Error::Limit);
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let current = std::fs::symlink_metadata(path).map_err(|_| Error::Stale)?;
    if !current.is_file()
        || Stamp::of(&current)? != stamp
        || Stamp::of(&file.metadata().map_err(|_| Error::Io)?)? != stamp
    {
        return Err(Error::Stale);
    }
    if bytes.contains(&0) {
        return Err(Error::Unsupported);
    }
    let bom = bytes.starts_with(&[239, 187, 191]);
    if bom {
        bytes.drain(..3);
    }
    let raw = String::from_utf8(bytes).map_err(|_| Error::Unsupported)?;
    Ok(Disk {
        raw: raw.into(),
        bom,
        stamp,
    })
}
fn paths(root: &Path, members: &[PathBuf], context: &Context) -> Result<BTreeSet<PathBuf>, Error> {
    let members: BTreeSet<_> = members
        .iter()
        .map(|p| p.canonicalize().map_err(|_| Error::Io))
        .collect::<Result<_, _>>()?;
    let mut pending = vec![root.to_path_buf()];
    pending.extend(members.iter().cloned());
    let mut visited = BTreeSet::new();
    let mut result = BTreeSet::new();
    let mut entries = 0;
    while let Some(directory) = pending.pop() {
        context.check()?;
        let directory = directory.canonicalize().map_err(|_| Error::Io)?;
        if !visited.insert(directory.clone()) {
            continue;
        }
        if directory != root
            && !members.contains(&directory)
            && std::fs::symlink_metadata(directory.join("Cargo.toml")).is_ok()
        {
            continue;
        }
        for entry in std::fs::read_dir(&directory).map_err(|_| Error::Io)? {
            context.check()?;
            entries += 1;
            if entries > 100_000 {
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
                    pending.push(entry.path());
                }
            } else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "rs") {
                result.insert(entry.path());
                if result.len() > FILES {
                    return Err(Error::Limit);
                }
            }
        }
    }
    Ok(result)
}
fn config_paths(capture: &Capture) -> BTreeSet<PathBuf> {
    let mut result = BTreeSet::new();
    for root in std::iter::once(&capture.root).chain(capture.members.iter()) {
        for file in [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain",
            "rust-toolchain.toml",
            ".cargo/config",
            ".cargo/config.toml",
            "rust-project.json",
        ] {
            result.insert(root.join(file));
        }
    }
    result
}
impl Project {
    fn load(mut capture: Capture, context: &Context) -> Result<Self, Error> {
        capture.root = capture.root.canonicalize().map_err(|_| Error::Io)?;
        capture.members = capture
            .members
            .iter()
            .map(|p| p.canonicalize().map_err(|_| Error::Io))
            .collect::<Result<_, _>>()?;
        if capture.members.len() > 64 || capture.overlays.len() > 64 {
            return Err(Error::Limit);
        }
        let paths = paths(&capture.root, &capture.members, context)?;
        let mut files = BTreeMap::new();
        let mut total = 0;
        for path in &paths {
            let disk = read(path, context)?;
            let open = capture
                .overlays
                .iter()
                .find(|overlay| overlay.ticket.path == *path)
                .cloned();
            let raw: Arc<str> = if let Some(open) = &open {
                if open.modified != Some(disk.stamp.modified) || open.bom != disk.bom {
                    return Err(Error::Stale);
                }
                let disk_rope = rc_text::Buffer::new(&disk.raw).snapshot();
                if disk_rope != open.saved {
                    return Err(Error::Stale);
                }
                let text = open.rope.to_string();
                if open.ending == rc_text::LineEnding::CrLf {
                    text.replace('\n', "\r\n").into()
                } else {
                    text.into()
                }
            } else {
                disk.raw.clone()
            };
            let rope = rc_text::Buffer::new(&raw).snapshot();
            let normal: Arc<str> = rope.to_string().into();
            total += normal.len();
            if normal.len() > FILE_BYTES || total > TOTAL_BYTES {
                return Err(Error::Limit);
            }
            files.insert(
                path.clone(),
                File {
                    path: path.clone(),
                    raw,
                    normal,
                    rope,
                    bom: open.as_ref().map_or(disk.bom, |o| o.bom),
                    modified: disk.stamp.modified,
                    open,
                    disk,
                },
            );
        }
        let mut configs = Vec::new();
        let mut config_bytes = 0;
        for path in config_paths(&capture) {
            context.check()?;
            let disk = match std::fs::symlink_metadata(&path) {
                Ok(_) => Some(read(&path, context)?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err(Error::Io),
            };
            if let Some(disk) = &disk {
                config_bytes += disk.raw.len();
                if config_bytes > TOTAL_BYTES {
                    return Err(Error::Limit);
                }
            }
            configs.push((path, disk));
        }
        Ok(Self {
            capture,
            files,
            paths,
            configs,
        })
    }
    pub fn validate(&self, context: &Context) -> Result<(), Error> {
        if paths(&self.capture.root, &self.capture.members, context)? != self.paths {
            return Err(Error::Stale);
        }
        for file in self.files.values() {
            context.check()?;
            let now = read(&file.path, context)?;
            if now.stamp != file.disk.stamp || now.raw != file.disk.raw || now.bom != file.disk.bom
            {
                return Err(Error::Stale);
            }
        }
        for (path, old) in &self.configs {
            context.check()?;
            match old {
                Some(old) => {
                    let now = read(path, context)?;
                    if now.stamp != old.stamp || now.raw != old.raw || now.bom != old.bom {
                        return Err(Error::Stale);
                    }
                }
                None => {
                    if std::fs::symlink_metadata(path).is_ok() {
                        return Err(Error::Stale);
                    }
                }
            }
        }
        Ok(())
    }
    fn file(&self, key: &Key) -> Result<&File, Error> {
        self.files.get(&key.path).ok_or(Error::Unsupported)
    }
    fn byte(&self, key: &Key) -> Result<usize, Error> {
        self.file(key)?.byte(lsp::Position {
            line: key.line,
            character: key.column,
        })
    }
}
#[derive(Clone)]
pub struct Prepared {
    pub project: Arc<Project>,
    pub key: Key,
    pub function: model::Function,
    pub calls: Vec<(PathBuf, model::Call)>,
}
#[derive(Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub edits: Vec<model::Edit>,
    pub after: Arc<str>,
}
#[derive(Clone)]
pub struct Plan {
    pub prepared: Arc<Prepared>,
    pub input: model::Input,
    pub files: Vec<FileChange>,
}

fn parsed(file: &File) -> Result<rc_syntax::tree_sitter::Tree, Error> {
    let tree =
        rc_syntax::parse_now(rc_syntax::Lang::Rust, &file.rope, None).ok_or(Error::Unsupported)?;
    if tree.root_node().has_error() {
        return Err(Error::Unsupported);
    }
    Ok(tree)
}
fn leaf(mut node: rc_syntax::tree_sitter::Node<'_>) -> rc_syntax::tree_sitter::Node<'_> {
    while let Some(name) = node.child_by_field_name("name") {
        node = name;
    }
    node
}
fn name(text: &str) -> &str {
    text.strip_prefix("r#").unwrap_or(text)
}
fn canonical_definition(
    project: &Project,
    key: &Key,
    context: &Context,
) -> Result<Option<Key>, Error> {
    let mut current = key.clone();
    let mut seen = BTreeSet::new();
    for _ in 0..32 {
        context.check()?;
        if !seen.insert(current.clone()) {
            return Err(Error::Unsupported);
        }
        let targets = context.definitions(&current)?;
        if targets.is_empty() {
            return Ok(None);
        }
        if targets.len() != 1 {
            return Err(Error::Unsupported);
        }
        let target = targets.into_iter().next().ok_or(Error::Analysis)?;
        let Some(file) = project.files.get(&target.path) else {
            return Ok(Some(target));
        };
        let byte = file.byte(lsp::Position {
            line: target.line,
            character: target.column,
        })?;
        let tree = parsed(file)?;
        let mut node = tree
            .root_node()
            .descendant_for_byte_range(byte, byte)
            .ok_or(Error::Analysis)?;
        let mut alias = None;
        loop {
            if node.kind() == "use_as_clause" {
                if let Some(path) = node.child_by_field_name("path") {
                    alias = Some(file.key(leaf(path).start_byte())?);
                }
                break;
            }
            match node.parent() {
                Some(parent) => node = parent,
                None => break,
            }
        }
        if let Some(alias) = alias {
            if alias != target && alias != current {
                current = alias;
                continue;
            }
        }
        return Ok(Some(target));
    }
    Err(Error::Limit)
}
fn within_comment_or_import(file: &File, byte: usize) -> Result<bool, Error> {
    let tree = parsed(file)?;
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(byte, byte)
        .ok_or(Error::Analysis)?;
    loop {
        if matches!(
            node.kind(),
            "line_comment" | "block_comment" | "use_declaration"
        ) {
            return Ok(true);
        }
        match node.parent() {
            Some(parent) => node = parent,
            None => return Ok(false),
        }
    }
}
fn std_source(project: &Project, key: &Key) -> bool {
    if project.files.contains_key(&key.path) || key.path.starts_with(&project.capture.root) {
        return false;
    }
    let components: Vec<_> = key
        .path
        .components()
        .filter_map(|part| part.as_os_str().to_str())
        .collect();
    components.windows(3).any(|parts| {
        parts[0] == "library" && matches!(parts[1], "core" | "alloc" | "std") && parts[2] == "src"
    })
}
fn macro_contexts(project: &Project, context: &Context) -> Result<(), Error> {
    let allowed_macro = [
        "assert",
        "assert_eq",
        "assert_ne",
        "debug_assert",
        "debug_assert_eq",
        "debug_assert_ne",
        "format",
        "format_args",
        "format_args_nl",
        "print",
        "println",
        "eprint",
        "eprintln",
        "write",
        "writeln",
        "panic",
        "todo",
        "unimplemented",
        "unreachable",
        "matches",
        "vec",
        "dbg",
        "env",
        "option_env",
        "concat",
        "concat_bytes",
        "stringify",
        "line",
        "column",
        "file",
        "module_path",
        "include_str",
        "include_bytes",
    ];
    let allowed_attribute = [
        "allow",
        "warn",
        "deny",
        "forbid",
        "expect",
        "doc",
        "deprecated",
        "must_use",
        "non_exhaustive",
        "inline",
        "cold",
        "cfg",
        "test",
        "bench",
        "no_std",
        "no_main",
        "feature",
        "recursion_limit",
        "type_length_limit",
    ];
    let mut queries = 0;
    for file in project.files.values() {
        let tree = parsed(file)?;
        let mut stack = vec![tree.root_node()];
        let mut visited = 0;
        while let Some(node) = stack.pop() {
            context.check()?;
            visited += 1;
            if visited > 500_000 {
                return Err(Error::Limit);
            }
            if node.kind() == "macro_definition" {
                continue;
            }
            if node.kind() == "macro_invocation" {
                let path = leaf(
                    node.child_by_field_name("macro")
                        .ok_or(Error::Unsupported)?,
                );
                let label = file.normal.get(path.byte_range()).ok_or(Error::Analysis)?;
                if !allowed_macro.contains(&name(label)) {
                    return Err(Error::Unsupported);
                }
                queries += 1;
                if queries > 1024 {
                    return Err(Error::Limit);
                }
                let target = canonical_definition(project, &file.key(path.start_byte())?, context)?
                    .ok_or(Error::Unsupported)?;
                if !std_source(project, &target) {
                    return Err(Error::Unsupported);
                }
            }
            if node.kind() == "attribute" {
                let mut cursor = node.walk();
                let path = node
                    .named_children(&mut cursor)
                    .find(|child| {
                        matches!(
                            child.kind(),
                            "identifier" | "scoped_identifier" | "crate" | "self" | "super"
                        )
                    })
                    .ok_or(Error::Unsupported)?;
                let label = file.normal.get(path.byte_range()).ok_or(Error::Analysis)?;
                if label == "derive" {
                    let args = node
                        .child_by_field_name("arguments")
                        .ok_or(Error::Unsupported)?;
                    let mut cursor = args.walk();
                    for item in args.named_children(&mut cursor) {
                        if matches!(item.kind(), "line_comment" | "block_comment") {
                            continue;
                        }
                        if item.kind() != "identifier" {
                            return Err(Error::Unsupported);
                        }
                        let target =
                            canonical_definition(project, &file.key(item.start_byte())?, context)?
                                .ok_or(Error::Unsupported)?;
                        if !std_source(project, &target) {
                            return Err(Error::Unsupported);
                        }
                    }
                } else if !allowed_attribute.contains(&label) {
                    return Err(Error::Unsupported);
                }
            }
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
    }
    Ok(())
}
pub fn prepare(capture: Capture, origin: Key, context: &Context) -> Result<Prepared, Error> {
    let project = Arc::new(Project::load(capture, context)?);
    let key = canonical_definition(&project, &origin, context)?.ok_or(Error::Unsupported)?;
    let file = project.file(&key)?;
    let byte = project.byte(&key)?;
    let function = model::function(&file.normal, byte)?;
    if function.name.len() > 128
        || function.params.len() > 64 * 1024
        || function
            .parameters
            .iter()
            .any(|p| p.name.len() > 128 || p.ty.len() > 2048)
        || function.return_type.len() > 2048
    {
        return Err(Error::Limit);
    }
    macro_contexts(&project, context)?;
    let mut aliases = Vec::new();
    let mut known = BTreeSet::from([name(&function.name).to_owned()]);
    for file in project.files.values() {
        let tree = parsed(file)?;
        let mut stack = vec![tree.root_node()];
        let mut visited = 0;
        while let Some(node) = stack.pop() {
            context.check()?;
            visited += 1;
            if visited > 500_000 {
                return Err(Error::Limit);
            }
            if node.kind() == "use_as_clause" {
                let path = leaf(node.child_by_field_name("path").ok_or(Error::Unsupported)?);
                let alias = node
                    .child_by_field_name("alias")
                    .ok_or(Error::Unsupported)?;
                let path_key = file.key(path.start_byte())?;
                match canonical_definition(&project, &path_key, context)? {
                    Some(target) if target == key => {
                        let alias_name =
                            file.normal.get(alias.byte_range()).ok_or(Error::Analysis)?;
                        if alias_name != "_" {
                            known.insert(name(alias_name).into());
                            aliases.push(file.key(alias.start_byte())?);
                            if aliases.len() > 128 {
                                return Err(Error::Limit);
                            }
                        }
                    }
                    None => {
                        if known.contains(name(
                            file.normal.get(path.byte_range()).ok_or(Error::Analysis)?,
                        )) {
                            return Err(Error::Unsupported);
                        }
                    }
                    _ => {}
                }
            }
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
    }
    let mut candidates = BTreeSet::new();
    for symbol in std::iter::once(&key).chain(aliases.iter()) {
        for reference in context.references(symbol)? {
            candidates.insert(reference);
            if candidates.len() > REFERENCES {
                return Err(Error::Limit);
            }
        }
    }
    for file in project.files.values() {
        let tree = parsed(file)?;
        let mut stack = vec![tree.root_node()];
        let mut visited = 0;
        while let Some(node) = stack.pop() {
            context.check()?;
            visited += 1;
            if visited > 500_000 {
                return Err(Error::Limit);
            }
            if matches!(
                node.kind(),
                "identifier"
                    | "type_identifier"
                    | "field_identifier"
                    | "shorthand_field_identifier"
            ) && file
                .normal
                .get(node.byte_range())
                .is_some_and(|text| known.contains(name(text)))
            {
                candidates.insert(file.key(node.start_byte())?);
                if candidates.len() > REFERENCES {
                    return Err(Error::Limit);
                }
            }
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
    }
    let mut calls = BTreeMap::new();
    for candidate in candidates {
        context.check()?;
        if candidate == key {
            continue;
        }
        let file = project.file(&candidate)?;
        let byte = project.byte(&candidate)?;
        if within_comment_or_import(file, byte)? {
            continue;
        }
        match canonical_definition(&project, &candidate, context)? {
            Some(target) if target == key => {
                let call = model::call(&file.normal, byte).map_err(|_| Error::FunctionValue)?;
                calls.insert((candidate.path.clone(), call.arguments.start), call);
            }
            Some(_) => {}
            None => return Err(Error::Unsupported),
        }
    }
    project.validate(context)?;
    Ok(Prepared {
        project,
        key,
        function,
        calls: calls
            .into_iter()
            .map(|((path, _), call)| (path, call))
            .collect(),
    })
}

fn body_bindings(
    prepared: &Prepared,
    context: &Context,
) -> Result<(BTreeSet<String>, bool), Error> {
    let file = prepared.project.file(&prepared.key)?;
    let tree = parsed(file)?;
    let mut stack = vec![tree.root_node()];
    let mut names = BTreeSet::new();
    let mut macros = false;
    while let Some(node) = stack.pop() {
        context.check()?;
        if node.end_byte() < prepared.function.body.start
            || node.start_byte() > prepared.function.body.end
        {
            continue;
        }
        if node.start_byte() >= prepared.function.body.start
            && node.end_byte() <= prepared.function.body.end
        {
            if node.kind() == "macro_invocation" {
                macros = true;
            }
            if matches!(node.kind(), "identifier" | "shorthand_field_identifier") {
                names.insert(
                    name(file.normal.get(node.byte_range()).ok_or(Error::Analysis)?).into(),
                );
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    Ok((names, macros))
}
fn parameter_used(prepared: &Prepared, index: usize, context: &Context) -> Result<bool, Error> {
    let file = prepared.project.file(&prepared.key)?;
    let parameter = prepared
        .function
        .parameters
        .get(index)
        .ok_or(Error::Analysis)?;
    if parameter.name == "_" {
        return Ok(false);
    }
    let binding = file.key(parameter.binding.start)?;
    for reference in context.references(&binding)? {
        if reference == binding {
            continue;
        }
        if reference.path != file.path {
            return Err(Error::Unsupported);
        }
        let byte = prepared.project.byte(&reference)?;
        if prepared.function.body.contains(&byte) {
            return Ok(true);
        }
        return Err(Error::Unsupported);
    }
    let tree = parsed(file)?;
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        context.check()?;
        if node.start_byte() >= prepared.function.body.start
            && node.end_byte() <= prepared.function.body.end
            && matches!(node.kind(), "identifier" | "shorthand_field_identifier")
            && file
                .normal
                .get(node.byte_range())
                .is_some_and(|text| name(text) == name(&parameter.name))
        {
            let targets = context.definitions(&file.key(node.start_byte())?)?;
            if targets.contains(&binding) {
                return Ok(true);
            }
            if targets.is_empty() {
                return Err(Error::Unsupported);
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    Ok(false)
}
fn parameter_rename(
    prepared: &Prepared,
    index: usize,
    new_name: &str,
    context: &Context,
) -> Result<Vec<model::Edit>, Error> {
    let file = prepared.project.file(&prepared.key)?;
    let parameter = prepared
        .function
        .parameters
        .get(index)
        .ok_or(Error::Analysis)?;
    let mut query = params(&file.key(parameter.binding.start)?);
    query["newName"] = json!(new_name);
    let reply = context.request("textDocument/rename", query)?;
    let edit: lsp::WorkspaceEdit = serde_json::from_value(reply).map_err(|_| Error::Analysis)?;
    if edit.document_changes.as_ref().is_some_and(|changes|matches!(changes,lsp::DocumentChanges::Operations(operations)if operations.iter().any(|op|matches!(op,lsp::DocumentChangeOperation::Op(_))))){return Err(Error::Unsupported);}
    if let Some(changes) = &edit.document_changes {
        let changes: Vec<_> = match changes {
            lsp::DocumentChanges::Edits(edits) => edits.iter().collect(),
            lsp::DocumentChanges::Operations(operations) => operations
                .iter()
                .filter_map(|op| match op {
                    lsp::DocumentChangeOperation::Edit(edit) => Some(edit),
                    _ => None,
                })
                .collect(),
        };
        for change in changes {
            if let Some(version) = change.text_document.version {
                let uri_path = uri_to_path(&change.text_document.uri)
                    .ok_or(Error::Analysis)?
                    .canonicalize()
                    .map_err(|_| Error::Io)?;
                if uri_path != file.path
                    || file
                        .open
                        .as_ref()
                        .and_then(|overlay| overlay.server_version)
                        != Some(version)
                {
                    return Err(Error::Stale);
                }
            }
        }
    }
    let mut output = Vec::new();
    for (path, edits) in crate::insight::workspace_edit_files(&edit) {
        if path.canonicalize().map_err(|_| Error::Io)? != file.path {
            return Err(Error::Unsupported);
        }
        if edits.len() > REFERENCES {
            return Err(Error::Limit);
        }
        for edit in edits {
            let range = file.range(edit.range)?;
            if range == parameter.binding {
                continue;
            }
            if range.start < prepared.function.body.start || range.end > prepared.function.body.end
            {
                return Err(Error::Unsupported);
            }
            if edit.new_text.len() > 64 * 1024 {
                return Err(Error::Limit);
            }
            output.push(model::Edit {
                range,
                text: edit.new_text,
            });
        }
    }
    Ok(output)
}
fn insert(edits: &mut Vec<model::Edit>, edit: model::Edit) -> Result<(), Error> {
    for old in edits.iter() {
        let overlap = old.range.start < edit.range.end && edit.range.start < old.range.end;
        if overlap && !(old.range.start >= edit.range.start && old.range.end <= edit.range.end) {
            return Err(Error::Model(model::Error::Overlap));
        }
    }
    edits.retain(|old| !(old.range.start >= edit.range.start && old.range.end <= edit.range.end));
    edits.push(edit);
    if edits.len() > REFERENCES {
        return Err(Error::Limit);
    }
    Ok(())
}
pub fn preview(
    prepared: Arc<Prepared>,
    input: model::Input,
    context: &Context,
) -> Result<Plan, Error> {
    model::validate(&prepared.function, &input)?;
    if input == prepared.function.input() {
        return Err(Error::NoChanges);
    }
    prepared.project.validate(context)?;
    let (body_names, macros) = body_bindings(&prepared, context)?;
    let bindings_changed = input.parameters.len() != prepared.function.parameters.len()
        || input.parameters.iter().any(|row| {
            row.original.is_none_or(|index| {
                prepared
                    .function
                    .parameters
                    .get(index)
                    .is_none_or(|old| old.name != row.name)
            })
        });
    if bindings_changed && macros {
        return Err(Error::Unsupported);
    }
    for row in &input.parameters {
        if row.name != "_"
            && row
                .original
                .is_none_or(|index| prepared.function.parameters[index].name != row.name)
            && body_names.contains(name(&row.name))
        {
            return Err(Error::NameConflict);
        }
    }
    for index in 0..prepared.function.parameters.len() {
        if !input
            .parameters
            .iter()
            .any(|row| row.original == Some(index))
            && parameter_used(&prepared, index, context)?
        {
            return Err(Error::UsedParameter);
        }
    }
    let mut changes: BTreeMap<PathBuf, Vec<model::Edit>> = BTreeMap::new();
    for row in &input.parameters {
        if let Some(index) = row.original {
            if prepared.function.parameters[index].name != row.name
                && prepared.function.parameters[index].name != "_"
            {
                for edit in parameter_rename(&prepared, index, &row.name, context)? {
                    insert(changes.entry(prepared.key.path.clone()).or_default(), edit)?;
                }
            }
        }
    }
    let original = (0..prepared.function.parameters.len())
        .map(Some)
        .collect::<Vec<_>>();
    let structure_changed = input
        .parameters
        .iter()
        .map(|row| row.original)
        .collect::<Vec<_>>()
        != original;
    if structure_changed {
        let mut calls = prepared.calls.clone();
        calls.sort_by_key(|(_, call)| call.arguments.len());
        for (path, call) in calls {
            context.check()?;
            if path == prepared.key.path
                && call.arguments.start >= prepared.function.params.start
                && call.arguments.end <= prepared.function.params.end
            {
                return Err(Error::Unsupported);
            }
            let file = prepared.project.files.get(&path).ok_or(Error::Analysis)?;
            let edits = changes.entry(path.clone()).or_default();
            let rendered =
                model::arguments(&file.normal, &call, &prepared.function, &input, edits)?;
            insert(
                edits,
                model::Edit {
                    range: call.arguments,
                    text: rendered,
                },
            )?;
        }
    }
    let root = prepared.project.file(&prepared.key)?;
    for edit in model::signature(&root.normal, &prepared.function, &input)? {
        insert(changes.entry(root.path.clone()).or_default(), edit)?;
    }
    let mut files = Vec::new();
    let mut count = 0;
    let mut bytes = 0;
    for (path, mut edits) in changes {
        context.check()?;
        let file = prepared.project.files.get(&path).ok_or(Error::Analysis)?;
        edits.sort_by_key(|edit| edit.range.start);
        edits.retain(|edit| file.normal.get(edit.range.clone()) != Some(edit.text.as_str()));
        if edits.is_empty() {
            continue;
        }
        count += edits.len();
        bytes += edits.iter().map(|edit| edit.text.len()).sum::<usize>();
        if count > REFERENCES || bytes > FILE_BYTES {
            return Err(Error::Limit);
        }
        let after = model::render(&file.normal, 0..file.normal.len(), &edits)?;
        if after.len() > FILE_BYTES {
            return Err(Error::Limit);
        }
        files.push(FileChange {
            path,
            edits,
            after: after.into(),
        });
    }
    if files.is_empty() {
        return Err(Error::NoChanges);
    }
    prepared.project.validate(context)?;
    Ok(Plan {
        prepared,
        input,
        files,
    })
}
