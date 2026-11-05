//! Macro lookup uses the live analyzer; matching runs in a resource-limited IDE child.
use crate::{
    convert,
    docs::Docs,
    macro_step_engine::{Error, INPUT_BYTES, Input, OUTPUT_BYTES, Reply, SOURCE_BYTES},
};
use rc_core::{Inbox, Waker};
use rc_dotnet::process::{CancelToken, StreamOpts};
use rc_lsp::{Client, path_to_uri, uri_to_path};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
const FILE_BYTES: usize = 2 * 1024 * 1024;
#[derive(Clone)]
struct Source {
    id: u64,
    version: u64,
    path: PathBuf,
    text: rc_text::Rope,
}
pub struct Expansion {
    pub name: String,
    pub text: String,
}
pub struct Job {
    inbox: Inbox<Result<Expansion, Error>>,
    cancelled: Arc<AtomicBool>,
    process: CancelToken,
    sources: Vec<Source>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.process.cancel();
    }
}
impl Job {
    pub fn start(
        client: Client,
        doc: &crate::docs::Doc,
        pos: usize,
        docs: &Docs,
        waker: Waker,
    ) -> Result<Self, Error> {
        if !cfg!(unix) {
            return Err(Error::Unsupported);
        }
        if doc.scratch || doc.generated || doc.editor.lang() != rc_syntax::Lang::Rust {
            return Err(Error::Unsupported);
        }
        let sources: Vec<_> = docs
            .list
            .iter()
            .filter(|d| !d.scratch && !d.generated && d.editor.lang() == rc_syntax::Lang::Rust)
            .map(|d| Source {
                id: d.editor.id,
                version: d.editor.buffer.version(),
                path: d.path.clone(),
                text: d.editor.buffer.snapshot(),
            })
            .collect();
        if sources.len() > 64
            || sources.iter().any(|s| s.text.len_bytes() > FILE_BYTES)
            || sources.iter().map(|s| s.text.len_bytes()).sum::<usize>() > 8 * FILE_BYTES
        {
            return Err(Error::Limit);
        }
        let origin = doc.path.clone();
        let input = sources.clone();
        let inbox = Inbox::new(waker);
        let output = inbox.sender();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let process = CancelToken::new();
        let child = process.clone();
        std::thread::Builder::new()
            .name("rc-macro-step".into())
            .spawn(move || {
                let context = Context {
                    cancelled: flag,
                    process: child,
                    deadline: Instant::now() + Duration::from_secs(20),
                    observed: Mutex::new(BTreeMap::new()),
                };
                let result = helper_executable().and_then(|executable| {
                    expand(&client, &origin, pos, &input, &executable, &context)
                });
                if !context.cancelled.load(Ordering::Acquire) {
                    output.send(result);
                }
            })
            .map_err(|_| Error::Io)?;
        Ok(Self {
            inbox,
            cancelled,
            process,
            sources,
        })
    }
    pub fn current(&self, docs: &Docs) -> bool {
        self.sources.iter().all(|s| {
            docs.list.iter().any(|d| {
                d.editor.id == s.id && d.editor.buffer.version() == s.version && d.path == s.path
            })
        }) && docs
            .list
            .iter()
            .filter(|d| !d.scratch && !d.generated && d.editor.lang() == rc_syntax::Lang::Rust)
            .count()
            == self.sources.len()
    }
    pub fn poll(&self) -> Option<Result<Expansion, Error>> {
        self.inbox.drain().into_iter().next()
    }
}
fn helper_executable() -> Result<PathBuf, Error> {
    #[cfg(test)]
    {
        return std::env::var_os("CRABCRAFT_MACRO_WORKER")
            .map(PathBuf::from)
            .ok_or(Error::Unsupported);
    }
    #[cfg(not(test))]
    {
        std::env::current_exe().map_err(|_| Error::Io)
    }
}
struct Context {
    cancelled: Arc<AtomicBool>,
    process: CancelToken,
    deadline: Instant,
    observed: Mutex<BTreeMap<PathBuf, String>>,
}
impl Context {
    fn observe(&self, path: &Path, text: &str) -> Result<(), Error> {
        let mut observed = self.observed.lock().map_err(|_| Error::Io)?;
        if let Some(old) = observed.get(path) {
            if old != text {
                return Err(Error::Stale);
            }
            return Ok(());
        }
        if observed.len() >= 64
            || observed.values().map(String::len).sum::<usize>() + text.len() > 16 * 1024 * 1024
        {
            return Err(Error::Limit);
        }
        observed.insert(path.to_path_buf(), text.into());
        Ok(())
    }
    fn validate(&self) -> Result<(), Error> {
        let paths: Vec<_> = self
            .observed
            .lock()
            .map_err(|_| Error::Io)?
            .keys()
            .cloned()
            .collect();
        for path in paths {
            read(&path, FILE_BYTES, self)?;
        }
        Ok(())
    }
    fn check(&self) -> Result<(), Error> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(Error::Timeout)
        } else {
            Ok(())
        }
    }
    fn request(&self, client: &Client, params: Value) -> Result<Value, Error> {
        self.check()?;
        let mut pending = client.request_raw("textDocument/definition", params);
        loop {
            self.check()?;
            if let Some(reply) = pending.poll() {
                return reply.map_err(|_| Error::Analyzer);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
fn read(path: &Path, limit: usize, context: &Context) -> Result<String, Error> {
    context.check()?;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| Error::Io)?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
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
    if !opened.is_file() || opened.len() > limit as u64 {
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
        if count > limit.saturating_sub(bytes.len()) {
            return Err(Error::Limit);
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let text = String::from_utf8(bytes)
        .map(|text| {
            text.trim_start_matches('\u{feff}')
                .replace("\r\n", "\n")
                .replace('\r', "\n")
        })
        .map_err(|_| Error::Invalid)?;
    context.observe(path, &text)?;
    Ok(text)
}
fn source<'a>(path: &Path, sources: &'a [Source]) -> Option<&'a rc_text::Rope> {
    sources
        .iter()
        .find(|source| source.path == path)
        .map(|source| &source.text)
}
fn edition(path: &Path, context: &Context) -> Result<u16, Error> {
    let mut directory = path.parent().ok_or(Error::Unsupported)?;
    for _ in 0..32 {
        context.check()?;
        let manifest = directory.join("Cargo.toml");
        match std::fs::symlink_metadata(&manifest) {
            Ok(_) => {
                let text = read(&manifest, FILE_BYTES, context)?;
                let value: toml::Value = toml::from_str(&text).map_err(|_| Error::Invalid)?;
                if let Some(package) = value.get("package") {
                    let declared = package.get("edition");
                    if let Some(edition) = declared.and_then(toml::Value::as_str) {
                        return parse_edition(edition);
                    }
                    if declared.is_none() {
                        return Ok(2015);
                    }
                    if declared
                        .and_then(|v| v.get("workspace"))
                        .and_then(toml::Value::as_bool)
                        != Some(true)
                    {
                        return Err(Error::Unsupported);
                    }
                    if package.get("workspace").is_some() {
                        return Err(Error::Unsupported);
                    }
                    let mut workspace = directory;
                    for _ in 0..32 {
                        let text = read(&workspace.join("Cargo.toml"), FILE_BYTES, context)?;
                        let value: toml::Value =
                            toml::from_str(&text).map_err(|_| Error::Invalid)?;
                        if let Some(edition) = value
                            .get("workspace")
                            .and_then(|w| w.get("package"))
                            .and_then(|p| p.get("edition"))
                            .and_then(toml::Value::as_str)
                        {
                            return parse_edition(edition);
                        }
                        if value.get("workspace").is_some() {
                            return Err(Error::Unsupported);
                        }
                        workspace = workspace.parent().ok_or(Error::Unsupported)?;
                        while !workspace.join("Cargo.toml").is_file() {
                            context.check()?;
                            workspace = workspace.parent().ok_or(Error::Unsupported)?;
                        }
                    }
                    return Err(Error::Unsupported);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Io),
        }
        directory = directory.parent().ok_or(Error::Unsupported)?;
    }
    Err(Error::Limit)
}
fn parse_edition(text: &str) -> Result<u16, Error> {
    match text {
        "2015" => Ok(2015),
        "2018" => Ok(2018),
        "2021" => Ok(2021),
        "2024" => Ok(2024),
        _ => Err(Error::Unsupported),
    }
}
fn inner(text: &str) -> Result<String, Error> {
    if text.len() < 2 || text.len() > SOURCE_BYTES + 2 {
        return Err(Error::Limit);
    }
    let first = text.as_bytes()[0];
    let last = text.as_bytes()[text.len() - 1];
    if !matches!((first, last), (b'(', b')') | (b'{', b'}') | (b'[', b']')) {
        return Err(Error::Invalid);
    }
    text.get(1..text.len() - 1)
        .map(str::to_owned)
        .ok_or(Error::Invalid)
}
fn invocation(
    rope: &rc_text::Rope,
    pos: usize,
    context: &Context,
) -> Result<(String, usize, String), Error> {
    if pos > rope.len_chars() {
        return Err(Error::Invalid);
    }
    let text = rope.to_string();
    let byte = rope.char_to_byte(pos);
    let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, rope, None).ok_or(Error::Invalid)?;
    let mut current = Some(
        tree.root_node()
            .descendant_for_byte_range(byte, byte)
            .ok_or(Error::Invalid)?,
    );
    let mut best: Option<(usize, String, usize, String)> = None;
    let mut visited = 0;
    fn consider(
        path: rc_syntax::tree_sitter::Node<'_>,
        args: rc_syntax::tree_sitter::Node<'_>,
        start: usize,
        rope: &rc_text::Rope,
        text: &str,
        byte: usize,
        best: &mut Option<(usize, String, usize, String)>,
    ) -> Result<(), Error> {
        if byte < start || byte > args.end_byte() {
            return Ok(());
        }
        let size = args.end_byte() - start;
        if best.as_ref().is_some_and(|old| old.0 <= size) {
            return Ok(());
        }
        let mut name = path;
        while let Some(last) = name.child_by_field_name("name") {
            name = last;
        }
        let displayed = text
            .get(start..path.end_byte())
            .ok_or(Error::Invalid)?
            .to_owned();
        if displayed.len() > 256 {
            return Err(Error::Limit);
        }
        let arguments = inner(text.get(args.byte_range()).ok_or(Error::Invalid)?)?;
        *best = Some((
            size,
            displayed,
            rope.byte_to_char(name.start_byte()),
            arguments,
        ));
        Ok(())
    }
    for _ in 0..256 {
        context.check()?;
        let Some(node) = current else {
            break;
        };
        if node.kind() == "macro_invocation" {
            if let Some(path) = node.child_by_field_name("macro") {
                let mut cursor = node.walk();
                if let Some(args) = node
                    .named_children(&mut cursor)
                    .find(|child| child.kind() == "token_tree")
                {
                    consider(path, args, path.start_byte(), rope, &text, byte, &mut best)?;
                }
            }
        } else if node.kind() == "token_tree" {
            let mut tokens = Vec::new();
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                visited += 1;
                if visited > 500_000 {
                    return Err(Error::Limit);
                }
                context.check()?;
                if !matches!(child.kind(), "line_comment" | "block_comment") {
                    tokens.push(child);
                }
            }
            for index in 1..tokens.len().saturating_sub(1) {
                visited += 1;
                if visited > 500_000 {
                    return Err(Error::Limit);
                }
                context.check()?;
                let Some(bang) = tokens.get(index).copied() else {
                    continue;
                };
                if bang.kind() != "!" {
                    continue;
                }
                let Some(path) = tokens.get(index - 1).copied() else {
                    continue;
                };
                if !matches!(
                    path.kind(),
                    "identifier"
                        | "type_identifier"
                        | "scoped_identifier"
                        | "scoped_type_identifier"
                ) {
                    continue;
                }
                if index >= 2
                    && tokens
                        .get(index - 2)
                        .copied()
                        .is_some_and(|token| token.kind() == "$")
                {
                    continue;
                }
                let Some(args) = tokens
                    .get(index + 1)
                    .copied()
                    .filter(|token| token.kind() == "token_tree")
                else {
                    continue;
                };
                let mut first = index - 1;
                while first >= 2
                    && tokens
                        .get(first - 1)
                        .copied()
                        .is_some_and(|part| part.kind() == "::")
                    && tokens.get(first - 2).copied().is_some_and(|part| {
                        matches!(
                            part.kind(),
                            "identifier" | "type_identifier" | "crate" | "self" | "super"
                        )
                    })
                {
                    first -= 2;
                }
                let start = tokens
                    .get(first)
                    .copied()
                    .ok_or(Error::Invalid)?
                    .start_byte();
                consider(path, args, start, rope, &text, byte, &mut best)?;
            }
        }
        if best.is_some() {
            break;
        }
        current = node.parent();
    }
    if best.is_none() && current.is_some() {
        return Err(Error::Limit);
    }
    let (_, name, pos, arguments) = best.ok_or(Error::Unsupported)?;
    Ok((name, pos, arguments))
}

fn expand(
    client: &Client,
    origin: &Path,
    pos: usize,
    sources: &[Source],
    executable: &Path,
    context: &Context,
) -> Result<Expansion, Error> {
    context.check()?;
    let rope = source(origin, sources).ok_or(Error::Stale)?;
    if pos > rope.len_chars() {
        return Err(Error::Invalid);
    }
    let (name, macro_pos, arguments) = invocation(rope, pos, context)?;
    let reply=context.request(client,json!({"textDocument":{"uri":path_to_uri(origin).as_str()},"position":convert::lsp_position(rope,macro_pos)}))?;
    let entries = match &reply {
        Value::Array(entries) => entries.as_slice(),
        Value::Null => return Err(Error::Unsupported),
        other => std::slice::from_ref(other),
    };
    if entries.len() != 1 {
        return Err(Error::Unsupported);
    }
    let entry = &entries[0];
    let uri = entry
        .get("targetUri")
        .or_else(|| entry.get("uri"))
        .and_then(Value::as_str)
        .ok_or(Error::Unsupported)?
        .parse()
        .map_err(|_| Error::Analyzer)?;
    let definition_path = uri_to_path(&uri).ok_or(Error::Unsupported)?;
    let range = entry
        .get("targetSelectionRange")
        .or_else(|| entry.get("range"))
        .ok_or(Error::Analyzer)?;
    let start: rc_lsp::lsp_types::Position =
        serde_json::from_value(range.get("start").cloned().ok_or(Error::Analyzer)?)
            .map_err(|_| Error::Analyzer)?;
    let disk = if source(&definition_path, sources).is_none() {
        Some(read(&definition_path, FILE_BYTES, context)?)
    } else {
        None
    };
    let definition = source(&definition_path, sources)
        .cloned()
        .unwrap_or_else(|| rc_text::Rope::from_str(disk.as_deref().unwrap_or("")));
    let position = convert::char_of(&definition, start);
    if convert::lsp_position(&definition, position) != start {
        return Err(Error::Analyzer);
    }
    let tree =
        rc_syntax::parse_now(rc_syntax::Lang::Rust, &definition, None).ok_or(Error::Invalid)?;
    let byte = definition.char_to_byte(position);
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(byte, byte)
        .ok_or(Error::Unsupported)?;
    while node.kind() != "macro_definition" {
        node = node.parent().ok_or(Error::Unsupported)?;
    }
    if node.byte_range().len() > SOURCE_BYTES {
        return Err(Error::Limit);
    }
    let mut previous = node.prev_named_sibling();
    while let Some(attribute) = previous.filter(|node| {
        matches!(
            node.kind(),
            "attribute_item" | "line_comment" | "block_comment"
        )
    }) {
        if attribute.kind() == "attribute_item"
            && definition
                .byte_slice(attribute.byte_range())
                .to_string()
                .contains("rustc_builtin_macro")
        {
            return Err(Error::Unsupported);
        }
        previous = attribute.prev_named_sibling();
    }
    let mut cursor = node.walk();
    let rules: Vec<_> = node
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "macro_rule")
        .collect();
    let first = rules.first().ok_or(Error::Unsupported)?;
    let last = rules.last().ok_or(Error::Unsupported)?;
    let rules = definition
        .byte_slice(first.start_byte()..last.end_byte())
        .to_string();
    let input = Input {
        rules,
        arguments,
        definition_edition: edition(&definition_path, context)?,
        call_edition: edition(origin, context)?,
    };
    let text = run_helper(executable, input, context)?;
    context.validate()?;
    Ok(Expansion { name, text })
}
fn run_helper(executable: &Path, input: Input, context: &Context) -> Result<String, Error> {
    context.check()?;
    let payload = serde_json::to_vec(&input).map_err(|_| Error::Invalid)?;
    if payload.len() > INPUT_BYTES {
        return Err(Error::Limit);
    }
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("--internal-macro-step")
        .current_dir(std::env::temp_dir());
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender.send(payload).map_err(|_| Error::Io)?;
    drop(sender);
    let cancel = context.process.clone();
    let timer_token = cancel.clone();
    let remaining = context.deadline.saturating_duration_since(Instant::now());
    let timer = rc_core::Runtime::spawn(async move {
        tokio::time::sleep(remaining).await;
        timer_token.cancel();
    });
    let mut output = String::new();
    let mut exceeded = false;
    let mut diagnostics = 0usize;
    let output_cancel = cancel.clone();
    let result = rc_core::Runtime::handle().block_on(rc_dotnet::process::run_streaming(
        command,
        &cancel,
        StreamOpts {
            partial_lines: true,
            stdin: Some(receiver),
            ..Default::default()
        },
        &mut |_| {},
        &mut |line| {
            if line.stream == rc_dotnet::process::Stream::Stdout {
                if line.text.len() > (OUTPUT_BYTES * 6 + 1024).saturating_sub(output.len()) {
                    exceeded = true;
                    output_cancel.cancel();
                } else {
                    output.push_str(&line.text);
                    if line.newline {
                        output.push('\n');
                    }
                }
            } else if line.text.len() > (64 * 1024usize).saturating_sub(diagnostics) {
                exceeded = true;
                output_cancel.cancel();
            } else {
                diagnostics += line.text.len();
            }
        },
    ));
    timer.abort();
    context.check()?;
    if exceeded {
        return Err(Error::Limit);
    }
    let outcome = result.map_err(|_| Error::Io)?;
    if outcome.cancelled {
        return Err(Error::Cancelled);
    }
    if outcome.code != Some(0) {
        return Err(if outcome.code.is_none() {
            Error::Limit
        } else {
            Error::Invalid
        });
    }
    let reply: Reply = serde_json::from_str(&output).map_err(|_| Error::Invalid)?;
    let text = reply.result?;
    if text.len() > OUTPUT_BYTES {
        return Err(Error::Limit);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context(timeout: Duration) -> Context {
        Context {
            cancelled: Arc::new(AtomicBool::new(false)),
            process: CancelToken::new(),
            deadline: Instant::now() + timeout,
            observed: Mutex::new(BTreeMap::new()),
        }
    }
    #[test]
    fn selects_the_smallest_real_invocation_inside_opaque_token_trees() {
        let text = "/* café 🦀 */ fn main(){assert_eq!(crate::outer!(inner!(21)),42);}";
        let rope = rc_text::Rope::from_str(text);
        for (word, expected, args) in [
            ("outer!", "crate::outer", "inner!(21)"),
            ("inner!", "inner", "21"),
            ("assert_eq!", "assert_eq", "crate::outer!(inner!(21)),42"),
        ] {
            let byte = text.find(word).unwrap() + 2;
            let position = text[..byte].chars().count();
            let (name, _, arguments) =
                invocation(&rope, position, &context(Duration::from_secs(2))).unwrap();
            assert_eq!(name, expected);
            assert_eq!(arguments, args);
        }
        let text = "fn main(){assert_eq!(outer /* comment */ ! /* comment */ (21),42);}";
        let rope = rc_text::Rope::from_str(text);
        let (name, _, args) = invocation(
            &rope,
            text.find("outer").unwrap() + 2,
            &context(Duration::from_secs(2)),
        )
        .unwrap();
        assert_eq!(name, "outer");
        assert_eq!(args, "21");
        let text = "fn main(){outer!(\"not_a_macro!()\");}";
        let rope = rc_text::Rope::from_str(text);
        let (name, _, _) = invocation(
            &rope,
            text.find("not_a").unwrap(),
            &context(Duration::from_secs(2)),
        )
        .unwrap();
        assert_eq!(name, "outer");
    }
    #[test]
    fn editions_inherit_without_running_cargo_and_parser_keeps_token_delimiters() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("member/src")).unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[workspace]\nmembers=[\"member\"]\n[workspace.package]\nedition=\"2024\"\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("member/Cargo.toml"),
            "[package]\nname=\"member\"\nversion=\"0.1.0\"\nedition.workspace=true\n",
        )
        .unwrap();
        let file = root.path().join("member/src/main.rs");
        assert_eq!(
            edition(&file, &context(Duration::from_secs(2))).unwrap(),
            2024
        );
        assert_eq!(inner("[a, b]").unwrap(), "a, b");
        assert!(inner("abc").is_err());
        assert!(!root.path().join("target").exists());
    }
    #[cfg(unix)]
    #[test]
    fn cancelling_and_timing_out_a_stalled_helper_reaps_the_process() {
        use std::os::unix::fs::PermissionsExt;
        for cancel in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let executable = root.path().join("helper");
            let pidfile = root.path().join("pid");
            std::fs::write(
                &executable,
                format!(
                    "#!/bin/sh\nprintf '%s' $$ > '{}'\nexec sleep 30\n",
                    pidfile.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
            let context = context(if cancel {
                Duration::from_secs(3)
            } else {
                Duration::from_millis(250)
            });
            let token = context.process.clone();
            let worker = std::thread::spawn(move || {
                run_helper(
                    &executable,
                    Input {
                        rules: "()=>{42}".into(),
                        arguments: String::new(),
                        definition_edition: 2024,
                        call_edition: 2024,
                    },
                    &context,
                )
            });
            let deadline = Instant::now() + Duration::from_secs(2);
            while !pidfile.exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            let pid = std::fs::read_to_string(&pidfile)
                .unwrap()
                .parse::<i32>()
                .unwrap();
            if cancel {
                token.cancel();
            }
            let start = Instant::now();
            let result = worker.join().unwrap();
            assert!(
                matches!(result, Err(Error::Cancelled | Error::Timeout)),
                "{result:?}"
            );
            assert!(start.elapsed() < Duration::from_secs(2));
            assert_ne!(unsafe { libc::kill(pid, 0) }, 0);
        }
    }
    #[test]
    #[ignore = "requires the freshly built desktop binary in CRABCRAFT_MACRO_WORKER"]
    fn real_helper_expands_one_layer_and_generated_rust_compiles() {
        let executable = PathBuf::from(std::env::var_os("CRABCRAFT_MACRO_WORKER").unwrap());
        let output = run_helper(
            &executable,
            Input {
                rules: "($e:expr)=>{inner!($e,$e)}".into(),
                arguments: "21".into(),
                definition_edition: 2024,
                call_edition: 2024,
            },
            &context(Duration::from_secs(10)),
        )
        .unwrap();
        assert!(output.contains("inner"), "{output}");
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("main.rs");
        std::fs::write(&file,format!("macro_rules! inner {{ ($a:expr,$b:expr)=>{{$a+$b}}; }}\nfn main(){{assert_eq!({},42);}}",output)).unwrap();
        let binary = root.path().join("proof");
        let result = std::process::Command::new("rustc")
            .arg(&file)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            std::process::Command::new(binary)
                .status()
                .unwrap()
                .success()
        );
    }
}

pub fn error_text(error: &Error) -> &'static str {
    use rc_kit::i18n::tr;
    match error {
        Error::Unsupported => tr(
            "One-step expansion supports macro_rules declarations on Unix with resource limits. Built-in, procedural and newer macro syntax are unsupported.",
        ),
        Error::Invalid | Error::NoMatch => tr(
            "The declarative macro or its arguments could not be matched for one-step expansion.",
        ),
        Error::Limit => {
            tr("One-step macro expansion exceeds its source, token, memory or output limit.")
        }
        Error::Stale => tr("Rust source changed during one-step expansion. Request it again."),
        Error::Timeout => tr("One-step macro expansion timed out."),
        Error::Cancelled => tr("One-step macro expansion cancelled."),
        Error::Io | Error::Analyzer => {
            tr("The one-step macro worker or rust-analyzer could not resolve this declaration.")
        }
    }
}
pub fn show_progress(app: &mut crate::app::CrabCraftApp, ctx: &egui::Context) {
    if !app.insight.macro_step_pending() {
        return;
    }
    let mut open = true;
    egui::Window::new(rc_kit::i18n::tr("Expanding declarative macro one step…"))
        .id(egui::Id::new("macro_step_progress"))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                if ui.button(rc_kit::i18n::tr("Cancel")).clicked() {
                    app.insight.cancel_macro_step();
                }
            });
        });
    if !open {
        app.insight.cancel_macro_step();
    }
}

#[cfg(test)]
mod native_views {
    use egui_kittest::kittest::Queryable;
    #[test]
    fn generated_macro_tabs_are_read_only_memory_only_and_do_not_corrupt_session_indices() {
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"),"[package]\nname=\"memory_macro_views\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        let main = root.path().join("src/main.rs");
        let lib = root.path().join("src/lib.rs");
        std::fs::write(&main, "fn main() {}\n").unwrap();
        std::fs::write(&lib, "pub struct Item;\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace(root.path());
        app.open_file_and_wait(&main);
        app.show_macro_expansion("view", "inner!(21)");
        let path = app.docs.active_doc().unwrap().path.clone();
        assert!(!path.exists());
        app.docs.save(app.docs.active.unwrap()).unwrap();
        assert!(!path.exists());
        app.open_file_and_wait(&lib);
        app.show_macro_expansion("view", "inner!(22)");
        assert_eq!(app.docs.list.len(), 3);
        let mut h = crate::screens::harness(app, egui::vec2(1280., 900.));
        h.run_steps(2);
        h.get_by_label("view.rs").click();
        h.run_steps(2);
        let doc = h.state().docs.active_doc().unwrap();
        assert!(doc.generated && doc.editor.read_only);
        assert!(doc.editor.buffer.rope().to_string().contains("inner!(22)"));
        let session = h.state().session();
        assert_eq!(session.files.len(), 2);
        assert!(
            session
                .files
                .iter()
                .all(|f| f.path == main || f.path == lib)
        );
        assert!(session.active.is_none());
        fn valid(node: &rc_core::session::EditorNode, len: usize) {
            match node {
                rc_core::session::EditorNode::Group { tabs, active } => {
                    assert!(tabs.iter().all(|i| *i < len));
                    assert!(active.is_none_or(|i| i < len));
                }
                rc_core::session::EditorNode::Split { a, b, .. } => {
                    valid(a, len);
                    valid(b, len);
                }
            }
        }
        valid(session.editor_layout.as_ref().unwrap(), session.files.len());
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(main).unwrap(), "fn main() {}\n");
        assert!(h.state().docs.recent.iter().all(|p| p != &path));
        h.get_by_label("lib.rs").click();
        h.run_steps(2);
        assert_eq!(h.state().docs.active, Some(2));
        assert_eq!(h.state().session().active, Some(1));
    }
}
