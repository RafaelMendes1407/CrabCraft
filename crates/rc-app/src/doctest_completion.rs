//! Bounded semantic projection of Rust documentation examples.
use rc_core::{Inbox, Waker};
use rc_dotnet::CancelToken;
use rc_lsp::{LanguageSession, ServerState, lsp_types as lsp};
use rc_kit::i18n::{tr, trf};
use rc_text::Rope;
use serde_json::{Value, json};
use std::{collections::HashMap, path::{Path, PathBuf}, process::Stdio, time::{Duration, Instant}};
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_DOCUMENT: usize = 1024 * 1024;
const MAX_CODE: usize = 64 * 1024;
const MAX_ROWS: usize = 512;
const MAX_METADATA: usize = 8 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(25);

#[derive(Debug)]
pub enum Error { NoContext, Limit, Cancelled, Timeout, Tool(String), Io(std::io::Error), Metadata, RustSource }
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoContext => f.write_str(tr("No supported Rust documentation code fence at the caret")),
            Self::Limit => f.write_str(tr("Doctest completion exceeds its document, code or metadata limit")),
            Self::Cancelled => f.write_str(tr("Doctest completion cancelled")),
            Self::Timeout => f.write_str(tr("Doctest completion timed out")),
            Self::Tool(message) => f.write_str(&trf("Doctest completion tool error: {error}", &[("error", message)])),
            Self::Io(error) => f.write_str(&trf("Doctest completion tool error: {error}", &[("error", &error.to_string())])),
            Self::Metadata => f.write_str(tr("Doctest completion requires resolved offline Cargo metadata for this package")),
            Self::RustSource => f.write_str(tr("Install rust-src to complete Rust standard library APIs in doctests")),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error { fn from(error: std::io::Error) -> Self { Self::Io(error) } }

#[derive(Clone)]
struct Row { original_line: u32, prefix_utf16: u32, prefix: String, code: String }
pub struct Projection { rows: Vec<Row>, source: String, position: lsp::Position, edition: Option<String>, block_doc: bool }
fn line_content(line: &str) -> Option<(&str, &str)> {
    let indentation = line.len() - line.trim_start_matches([' ', '\t']).len();
    let tail = &line[indentation..];
    if !(tail.starts_with("///") && !tail.starts_with("////") || tail.starts_with("//!")) { return None; }
    let mut prefix = indentation + 3;
    if line.as_bytes().get(prefix) == Some(&b' ') { prefix += 1; }
    Some((&line[..prefix], line[prefix..].trim_end_matches(['\r', '\n'])))
}
fn block_range(doc: &crate::docs::Doc) -> Option<std::ops::Range<usize>> {
    let rope = doc.editor.buffer.rope(); let byte = rope.char_to_byte(doc.editor.caret());
    let mut node = doc.editor.syntax.tree()?.root_node().descendant_for_byte_range(byte, byte)?;
    while node.kind() != "block_comment" { node = node.parent()?; }
    if node.end_byte() > rope.len_bytes() || node.start_byte() + 3 > node.end_byte() { return None; }
    let prefix = rope.byte_slice(node.start_byte()..node.start_byte() + 3).to_string();
    matches!(prefix.as_str(), "/**" | "/*!").then_some(node.byte_range())
}
pub fn candidate(doc: &crate::docs::Doc) -> bool {
    let rope = doc.editor.buffer.rope(); let line = rope.char_to_line(doc.editor.caret());
    rope.line(line).len_bytes() <= MAX_CODE && line_content(&rope.line(line).to_string()).is_some() || block_range(doc).is_some()
}
fn project(rope: &Rope, caret: usize) -> Result<Projection, Error> {
    if rope.len_bytes() > MAX_DOCUMENT || caret > rope.len_chars() { return Err(Error::Limit); }
    let line = rope.char_to_line(caret);
    let mut input = Vec::new(); let block_doc;
    if line_content(&rope.line(line).to_string()).is_some() {
        block_doc = false;
        let mut start = line; let mut end = line + 1;
        while start > 0 && line - start < MAX_ROWS {
            if rope.line(start - 1).len_bytes() > MAX_CODE { return Err(Error::Limit); }
            if line_content(&rope.line(start - 1).to_string()).is_none() { break; } start -= 1;
        }
        while end < rope.len_lines() && end - start < MAX_ROWS {
            if rope.line(end).len_bytes() > MAX_CODE { return Err(Error::Limit); }
            if line_content(&rope.line(end).to_string()).is_none() { break; } end += 1;
        }
        for index in start..end {
            let text = rope.line(index).to_string();
            let (prefix, content) = line_content(&text).ok_or(Error::NoContext)?;
            input.push((index, prefix.to_string(), content.to_string()));
        }
    } else {
        block_doc = true;
        let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, rope, None).ok_or(Error::NoContext)?;
        let byte = rope.char_to_byte(caret);
        let mut node = tree.root_node().descendant_for_byte_range(byte, byte).ok_or(Error::NoContext)?;
        while node.kind() != "block_comment" { node = node.parent().ok_or(Error::NoContext)?; }
        if node.byte_range().len() > MAX_CODE { return Err(Error::Limit); }
        let text = rope.byte_slice(node.byte_range()).to_string();
        if !(text.starts_with("/**") || text.starts_with("/*!")) || !text.ends_with("*/") { return Err(Error::NoContext); }
        let start_line = node.start_position().row;
        let before = rope.line(start_line).byte_slice(..node.start_position().column).to_string();
        for (offset, raw) in text.split_inclusive('\n').enumerate() {
            if offset >= MAX_ROWS { return Err(Error::Limit); }
            let mut content = raw.trim_end_matches(['\r', '\n']);
            if content.ends_with("*/") { content = &content[..content.len() - 2]; }
            let prefix = if offset == 0 {
                content = content.get(3..).ok_or(Error::NoContext)?;
                format!("{before}{}", &raw[..3])
            } else {
                let indent = content.len() - content.trim_start_matches([' ', '\t']).len();
                let count = if content[indent..].starts_with('*') { indent + 1 + usize::from(content.as_bytes().get(indent + 1) == Some(&b' ')) } else { 0 };
                let prefix = content[..count].to_string(); content = &content[count..]; prefix
            };
            input.push((start_line + offset, prefix, content.to_string()));
        }
    }
    let mut opened: Option<(char, usize, Option<String>, bool)> = None; let mut rows = Vec::new(); let mut bytes = 0;
    let fence_char = char::from(96);
    for (index, prefix, content) in input {
        let trimmed = content.trim_start();
        let marker = trimmed.chars().next().unwrap_or(' ');
        let count = trimmed.chars().take_while(|c| *c == marker).count();
        if (marker == fence_char || marker == '~') && count >= 3 {
            if let Some((kind, minimum, _, _)) = &opened {
                if marker == *kind && count >= *minimum && trimmed[count..].trim().is_empty() {
                    if rows.iter().any(|row: &Row| row.original_line as usize == line) { break; }
                    opened = None; rows.clear(); bytes = 0; continue;
                }
            } else {
                let info = trimmed[count..].trim();
                let flags: Vec<_> = info.split([',', ' ', '\t']).filter(|x| !x.is_empty()).collect();
                let rust = flags.iter().all(|flag| matches!(*flag, "rust" | "no_run" | "ignore" | "should_panic" | "compile_fail" | "test_harness" | "standalone_crate" | "edition2015" | "edition2018" | "edition2021" | "edition2024") || flag.starts_with("ignore-") || flag.len() == 5 && flag.starts_with('E') && flag.as_bytes()[1..].iter().all(u8::is_ascii_digit));
                let edition = flags.iter().find_map(|flag| flag.strip_prefix("edition")).map(str::to_string);
                opened = Some((marker, count, edition, rust)); rows.clear(); bytes = 0;
                continue;
            }
        }
        if !opened.as_ref().is_some_and(|(_, _, _, rust)| *rust) { continue; }
        let indent = content.len() - content.trim_start_matches([' ', '\t']).len();
        let visible = &content[indent..];
        let removed = if visible.starts_with("# ") { indent + 2 } else if visible == "#" { indent + 1 }
            else if visible.starts_with("##") { indent + 1 } else { 0 };
        let code = &content[removed..];
        let prefix = format!("{prefix}{}", &content[..removed]);
        bytes += code.len(); if bytes > MAX_CODE || rows.len() >= MAX_ROWS { return Err(Error::Limit); }
        rows.push(Row { original_line: index as u32, prefix_utf16: prefix.encode_utf16().count() as u32, prefix, code: code.into() });
    }
    let edition = opened.as_ref().and_then(|(_, _, edition, _)| edition.clone());
    let row = rows.iter().position(|row| row.original_line as usize == line).ok_or(Error::NoContext)?;
    let original_position = crate::convert::lsp_position(rope, caret);
    let character = original_position.character.checked_sub(rows[row].prefix_utf16).ok_or(Error::NoContext)?;
    let source = format!("fn __crabcraft_doctest() {{\n{}\n}}\n", rows.iter().map(|row| row.code.as_str()).collect::<Vec<_>>().join("\n"));
    Ok(Projection { rows, source, position: lsp::Position::new(row as u32 + 1, character), edition, block_doc })
}
impl Projection {
    fn position(&self, value: lsp::Position) -> Option<lsp::Position> {
        let row = self.rows.get(value.line.checked_sub(1)? as usize)?;
        let mut units = 0u32;
        for character in row.code.chars() {
            if units == value.character { break; }
            units += character.len_utf16() as u32;
            if units > value.character { return None; }
        }
        if units != value.character { return None; }
        Some(lsp::Position::new(row.original_line, row.prefix_utf16.checked_add(value.character)?))
    }
    fn text(&self, text: &str, row: usize) -> Option<String> {
        if text.len() > 16 * 1024 || self.block_doc && text.contains("*/") { return None; }
        let prefix = &self.rows.get(row)?.prefix;
        let text = text.replace("\r\n", "\n").replace('\n', &format!("\n{prefix}"));
        (text.len() <= MAX_CODE).then_some(text)
    }
    fn item(&self, mut item: lsp::CompletionItem, default: Option<lsp::Range>) -> Option<lsp::CompletionItem> {
        if item.label.len() > 512 || item.filter_text.as_ref().is_some_and(|text| text.len() > 1024) || item.sort_text.as_ref().is_some_and(|text| text.len() > 1024) { return None; }
        if item.detail.as_ref().is_some_and(|text| text.len() > 8192) { item.detail = None; }
        if item.documentation.as_ref().is_some_and(|documentation| match documentation { lsp::Documentation::String(text) => text.len() > 16 * 1024, lsp::Documentation::MarkupContent(content) => content.value.len() > 16 * 1024 }) { item.documentation = None; }
        let (range, text) = match item.text_edit.take() {
            Some(lsp::CompletionTextEdit::Edit(edit)) => (Some(edit.range), edit.new_text),
            Some(lsp::CompletionTextEdit::InsertAndReplace(edit)) => (Some(edit.replace), edit.new_text),
            None => (default, item.insert_text.clone().unwrap_or_else(|| item.label.clone())),
        };
        let range = range.unwrap_or(lsp::Range::new(self.position, self.position));
        if (range.start.line, range.start.character) > (range.end.line, range.end.character) { return None; }
        let start = self.position(range.start)?; let end = self.position(range.end)?;
        let new_text = self.text(&text, range.start.line.checked_sub(1)? as usize)?;
        item.text_edit = Some(lsp::CompletionTextEdit::Edit(lsp::TextEdit { range: lsp::Range::new(start, end), new_text }));
        // The isolated server is gone after the snapshot. Never resolve its opaque data or
        // execute its commands through the main project's different server.
        if item.additional_text_edits.as_ref().is_some_and(|edits| !edits.is_empty()) { return None; }
        item.additional_text_edits = None; item.command = None; item.data = None;
        Some(item)
    }
}

async fn read_bounded(mut stream: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new(); let mut chunk = [0; 8192];
    loop {
        let count = stream.read(&mut chunk).await?; if count == 0 { return Ok(data); }
        if data.len() + count > limit { return Err(Error::Limit); }
        data.extend_from_slice(&chunk[..count]);
    }
}
async fn command(mut command: tokio::process::Command, token: &CancelToken, started: Instant, limit: usize) -> Result<Vec<u8>, Error> {
    check(token, started)?;
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)] command.process_group(0);
    let mut child = command.spawn()?; let pid = child.id().unwrap_or(0);
    let stdout = child.stdout.take().ok_or_else(|| Error::Tool(tr("Completion tool pipes were unavailable").into()))?;
    let stderr = child.stderr.take().ok_or_else(|| Error::Tool(tr("Completion tool pipes were unavailable").into()))?;
    let result = tokio::select! {
        _ = token.cancelled() => Err(Error::Cancelled),
        _ = tokio::time::sleep(DEADLINE.saturating_sub(started.elapsed())) => Err(Error::Timeout),
        result = async { tokio::try_join!(read_bounded(stdout, limit), read_bounded(stderr, 64 * 1024)) } => result,
    };
    let (stdout, stderr) = match result {
        Ok(output) => output,
        Err(error) => { rc_dotnet::process::kill_tree(pid); let _ = child.kill().await; let _ = child.wait().await; return Err(error); }
    };
    let status = tokio::select! {
        _ = token.cancelled() => None,
        _ = tokio::time::sleep(DEADLINE.saturating_sub(started.elapsed())) => None,
        status = child.wait() => Some(status?),
    };
    let Some(status) = status else { rc_dotnet::process::kill_tree(pid); let _ = child.kill().await; let _ = child.wait().await; check(token, started)?; return Err(Error::Timeout); };
    if !status.success() { return Err(Error::Tool(String::from_utf8_lossy(&stderr).chars().take(4096).collect())); }
    Ok(stdout)
}
fn check(token: &CancelToken, started: Instant) -> Result<(), Error> {
    if token.is_cancelled() { Err(Error::Cancelled) } else if started.elapsed() > DEADLINE { Err(Error::Timeout) } else { Ok(()) }
}
#[derive(Clone)]
pub struct Overlay { pub id: u64, pub version: u64, pub path: PathBuf, pub text: Option<Rope> }
pub fn overlays(docs: &crate::docs::Docs) -> Vec<Overlay> {
    docs.list.iter().take(257).filter(|doc| doc.editor.lang() == rc_syntax::Lang::Rust && !doc.generated && !doc.scratch)
        .map(|doc| Overlay { id: doc.editor.id, version: doc.editor.buffer.version(), path: doc.path.clone(), text: doc.is_modified().then(|| doc.editor.buffer.rope().clone()) }).collect()
}
#[derive(Clone)]
pub struct Context {
    pub root: PathBuf, pub path: PathBuf, pub rope: Rope, pub caret: usize,
    pub overlays: Vec<Overlay>, pub analyzer: Option<PathBuf>, pub env: Vec<(String, String)>, pub features: Vec<String>, pub target: Option<String>,
}
fn graph(metadata: Value, path: &Path, snippet: &Path, sysroot: &Path, cfg: &[String], edition: Option<&str>, selected: &[String], target: Option<&str>) -> Result<Value, Error> {
    let packages = metadata.get("packages").and_then(Value::as_array).ok_or(Error::Metadata)?;
    if packages.len() > 512 { return Err(Error::Limit); }
    let mut crates = Vec::new(); let mut ids = HashMap::new(); let mut owner = None; let mut owner_depth = 0;
    let path = path.canonicalize().unwrap_or_else(|_| path.into());
    for package in packages {
        let id = package.get("id").and_then(Value::as_str).ok_or(Error::Metadata)?;
        let targets = package.get("targets").and_then(Value::as_array).ok_or(Error::Metadata)?;
        let target = targets.iter().find(|target| target.get("kind").and_then(Value::as_array).is_some_and(|kinds| kinds.iter().any(|kind| matches!(kind.as_str(), Some("lib" | "rlib" | "proc-macro")))));
        let Some(target) = target else { continue; };
        let root = PathBuf::from(package.get("manifest_path").and_then(Value::as_str).ok_or(Error::Metadata)?).parent().ok_or(Error::Metadata)?.to_path_buf();
        let root = root.canonicalize().unwrap_or(root); let depth = root.components().count();
        if path.starts_with(&root) && depth >= owner_depth { owner = Some((crates.len(), target.get("name").and_then(Value::as_str).ok_or(Error::Metadata)?.replace('-', "_"))); owner_depth = depth; }
        let is_proc_macro = target["kind"].as_array().is_some_and(|kinds| kinds.iter().any(|kind| kind == "proc-macro"));
        if ids.insert(id.to_string(), crates.len()).is_some() { return Err(Error::Metadata); }
        crates.push(json!({ "root_module": target["src_path"], "edition": package["edition"], "deps": [], "cfg": cfg, "env": {},
            "is_workspace_member": true, "is_proc_macro": is_proc_macro }));
    }
    let fallback;
    let nodes = match metadata.pointer("/resolve/nodes").and_then(Value::as_array) {
        Some(nodes) => nodes,
        None => {
            // Cargo --no-deps needs no lockfile. It is exact for a dependency-free owner;
            // never invent dependency edges or silently write Cargo.lock for completion.
            let owner_index = owner.as_ref().map(|(index, _)| *index).ok_or(Error::Metadata)?;
            let package = packages.iter().find(|package| package.get("id").and_then(Value::as_str).and_then(|id| ids.get(id)).copied() == Some(owner_index)).ok_or(Error::Metadata)?;
            if package.get("dependencies").and_then(Value::as_array).is_some_and(|deps| deps.iter().any(|dep| dep.get("kind").and_then(Value::as_str) != Some("build"))) { return Err(Error::Metadata); }
            let name = package.get("name").and_then(Value::as_str).ok_or(Error::Metadata)?;
            let features = package.get("features").and_then(Value::as_object).ok_or(Error::Metadata)?;
            let mut enabled = std::collections::BTreeSet::new();
            let mut todo = selected.iter().filter_map(|feature| match feature.split_once('/') { Some((package, feature)) if package == name => Some(feature.to_string()), None => Some(feature.clone()), _ => None }).collect::<Vec<_>>();
            if features.contains_key("default") { todo.push("default".into()); }
            while let Some(feature) = todo.pop() {
                if enabled.len() > 512 { return Err(Error::Limit); }
                if !enabled.insert(feature.clone()) { continue; }
                if let Some(children) = features.get(&feature).and_then(Value::as_array) {
                    todo.extend(children.iter().filter_map(Value::as_str).filter(|feature| !feature.contains('/') && !feature.starts_with("dep:")).map(str::to_string));
                }
            }
            fallback = vec![json!({ "id": package["id"], "deps": [], "features": enabled })];
            &fallback
        },
    };
    let mut dependency_count = 0;
    for node in nodes {
        let Some(index) = node.get("id").and_then(Value::as_str).and_then(|id| ids.get(id)).copied() else { continue; };
        let dependencies = node.get("deps").and_then(Value::as_array).ok_or(Error::Metadata)?.iter().filter_map(|dependency| {
            let id = dependency.get("pkg")?.as_str()?; let name = dependency.get("name")?.as_str()?;
            ids.get(id).map(|index| json!({ "crate": index, "name": name.replace('-', "_") }))
        }).collect::<Vec<_>>();
        dependency_count += dependencies.len(); if dependency_count > 8192 { return Err(Error::Limit); }
        crates[index]["deps"] = json!(dependencies);
        if let Some(features) = node.get("features").and_then(Value::as_array) {
            if features.len() > 512 { return Err(Error::Limit); }
            let mut cfgs = cfg.to_vec();
            for feature in features.iter().filter_map(Value::as_str) {
                if feature.len() > 256 { return Err(Error::Limit); }
                cfgs.push(format!("feature={}", serde_json::to_string(feature).map_err(|_| Error::Metadata)?));
            }
            crates[index]["cfg"] = json!(cfgs);
        }
    }
    let (owner, name) = owner.ok_or(Error::Metadata)?;
    let mut dependencies = crates[owner]["deps"].as_array().cloned().unwrap_or_default();
    dependencies.retain(|dependency| dependency.get("name").and_then(Value::as_str) != Some(&name));
    dependencies.push(json!({ "crate": owner, "name": name }));
    let edition = edition.or_else(|| crates[owner]["edition"].as_str()).unwrap_or("2021").to_string();
    crates.push(json!({ "root_module": snippet, "edition": edition, "deps": dependencies, "cfg": cfg, "env": {}, "is_workspace_member": true }));
    if let Some(target) = target { for item in &mut crates { item["target"] = json!(target); } }
    Ok(json!({ "crates": crates, "sysroot_src": sysroot }))
}
struct Session(LanguageSession);
impl Drop for Session { fn drop(&mut self) { self.0.stop(); } }

fn run(context: Context, token: &CancelToken) -> Result<Vec<lsp::CompletionItem>, Error> {
    let started = Instant::now(); check(token, started)?;
    if context.features.len() > 256 || context.features.iter().any(|feature| feature.len() > 256) || context.target.as_ref().is_some_and(|target| !rc_cargo::valid_triple(target)) { return Err(Error::Limit); }
    let overlay_bytes = context.overlays.iter().filter_map(|overlay| overlay.text.as_ref()).try_fold(0usize, |bytes, rope| bytes.checked_add(rope.len_bytes()));
    if context.overlays.len() > 256 || overlay_bytes.is_none_or(|bytes| bytes > 8 * MAX_DOCUMENT)
        || context.overlays.iter().filter_map(|overlay| overlay.text.as_ref()).any(|rope| rope.len_bytes() > MAX_DOCUMENT) { return Err(Error::Limit); }
    let projection = project(&context.rope, context.caret)?;
    let cargo = rc_cargo::Cargo::locate().ok_or(Error::Metadata)?;
    let mut metadata = cargo.command();
    metadata.args(["metadata", "--format-version", "1", "--offline"]);
    if context.root.join("Cargo.lock").is_file() { metadata.arg("--locked"); } else { metadata.arg("--no-deps"); }
    if !context.features.is_empty() { metadata.arg("--features").arg(context.features.join(",")); }
    if let Some(target) = &context.target { metadata.arg("--filter-platform").arg(target); }
    metadata.current_dir(&context.root).envs(context.env.clone()).env("RUSTUP_AUTO_INSTALL", "0");
    let metadata = rc_core::Runtime::block_on(command(metadata, token, started, MAX_METADATA))?;
    check(token, started)?;
    let metadata: Value = serde_json::from_slice(&metadata).map_err(|_| Error::Metadata)?;
    let rustc = context.env.iter().rev().find(|(key, _)| if cfg!(windows) { key.eq_ignore_ascii_case("RUSTC") } else { key == "RUSTC" }).map(|(_, value)| PathBuf::from(value))
        .or_else(|| std::env::var_os("RUSTC").map(PathBuf::from)).or_else(|| cargo.path().parent().map(|root| root.join(if cfg!(windows) { "rustc.exe" } else { "rustc" })).filter(|path| path.is_file())).unwrap_or_else(|| "rustc".into());
    let mut sysroot = tokio::process::Command::new(&rustc);
    sysroot.args(["--print", "sysroot"]).current_dir(&context.root).envs(context.env.clone()).env("RUSTUP_AUTO_INSTALL", "0");
    let sysroot = rc_core::Runtime::block_on(command(sysroot, token, started, 16 * 1024))?;
    let sysroot = PathBuf::from(String::from_utf8(sysroot).map_err(|_| Error::RustSource)?.trim_end_matches(['\r', '\n'])).join("lib/rustlib/src/rust/library");
    if !sysroot.is_dir() { return Err(Error::RustSource); }
    let mut cfg = tokio::process::Command::new(&rustc);
    cfg.args(["--print", "cfg"]).current_dir(&context.root).envs(context.env.clone()).env("RUSTUP_AUTO_INSTALL", "0");
    if let Some(target) = &context.target { cfg.arg("--target").arg(target); }
    let cfg = rc_core::Runtime::block_on(command(cfg, token, started, 64 * 1024))?;
    let cfg = String::from_utf8(cfg).map_err(|_| Error::Metadata)?.lines().map(str::to_string).collect::<Vec<_>>();
    if cfg.len() > 128 || cfg.iter().any(|cfg| cfg.len() > 1024) { return Err(Error::Limit); }
    let dir = tempfile::Builder::new().prefix("crabcraft-doctest-").tempdir()?;
    let file = dir.path().join("example.rs"); std::fs::write(&file, &projection.source)?;
    let project_file = dir.path().join("rust-project.json");
    let graph = graph(metadata, &context.path, &file, &sysroot, &cfg, projection.edition.as_deref(), &context.features, context.target.as_deref())?;
    std::fs::write(&project_file, serde_json::to_vec(&graph).map_err(|_| Error::Metadata)?)?;
    let executable = if cfg!(windows) { "rust-analyzer.exe" } else { "rust-analyzer" };
    let program = context.analyzer.or_else(|| std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).take(128).map(|dir| dir.join(executable)).find(|path| path.is_file()))).ok_or(Error::Metadata)?;
    let settings = rc_lsp::rust_analyzer::RaSettings { isolated_projection: true, linked: vec![project_file], ..Default::default() };
    check(token, started)?;
    let mut analyzer_env = context.env.clone(); analyzer_env.push(("RUSTUP_AUTO_INSTALL".into(), "0".into()));
    let mut session = Session(LanguageSession::start_rust(&program, dir.path(), settings, &analyzer_env, Waker::noop())?);
    let mut opened = false;
    loop {
        check(token, started)?; session.0.pump();
        if let ServerState::Failed(error) = &session.0.state { return Err(Error::Tool(error.chars().take(4096).collect())); }
        if session.0.is_initialized() && !opened {
            for overlay in &context.overlays {
                if overlay.path != context.path && let Some(text) = &overlay.text { session.0.did_open(&overlay.path, "rust", &text.to_string()); }
            }
            session.0.did_open(&context.path, "rust", &context.rope.to_string());
            session.0.did_open(&file, "rust", &projection.source); opened = true;
        }
        if opened && session.0.is_ready() { break; }
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut request = session.0.client.request_raw::<Value>("textDocument/completion", json!({
        "textDocument": { "uri": rc_lsp::path_to_uri(&file) }, "position": projection.position, "context": { "triggerKind": 1 }
    }));
    loop {
        check(token, started)?; session.0.pump();
        if let Some(reply) = request.poll() {
            let value = reply.map_err(|error| Error::Tool(error.message.chars().take(4096).collect()))?;
            let (items, default) = crate::insight::parse_completion(value);
            return Ok(items.into_iter().take(256).filter_map(|item| projection.item(item, default)).collect());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
pub struct Job {
    pub doc: u64, pub version: u64, pub caret: usize, pub anchor: usize, pub path: PathBuf, pub started: Instant,
    pub root: PathBuf, pub features: Vec<String>, pub target: Option<String>, pub env: Vec<(String, String)>, snapshots: Vec<Overlay>,
    inbox: Inbox<Result<Vec<lsp::CompletionItem>, Error>>, token: CancelToken,
}
impl Job {
    pub fn start(context: Context, doc: u64, version: u64, anchor: usize, waker: Waker) -> Result<Self, Error> {
        let root = context.root.clone(); let features = context.features.clone(); let target = context.target.clone(); let env = context.env.clone(); let snapshots = context.overlays.clone();
        if anchor > context.caret { return Err(Error::NoContext); }
        let caret = context.caret; let path = context.path.clone(); let started = Instant::now(); let inbox = Inbox::new(waker); let out = inbox.sender();
        let token = CancelToken::new(); let cancel = token.clone();
        std::thread::Builder::new().name("rc-doctest-completion".into()).spawn(move || {
            let result = run(context, &cancel);
            if !cancel.is_cancelled() { out.send(result); }
        })?;
        Ok(Self { doc, version, caret, anchor, path, started, root, features, target, env, snapshots, inbox, token })
    }
    pub fn buffers_match(&self, docs: &crate::docs::Docs) -> bool {
        if docs.list.len() > 256 { return false; }
        let current = docs.list.iter().filter(|doc| doc.editor.lang() == rc_syntax::Lang::Rust && !doc.generated && !doc.scratch);
        current.map(|doc| (doc.editor.id, doc.editor.buffer.version(), &doc.path)).eq(self.snapshots.iter().map(|overlay| (overlay.id, overlay.version, &overlay.path)))
    }
    pub fn poll(&self) -> Option<Result<Vec<lsp::CompletionItem>, Error>> { self.inbox.drain().into_iter().next() }
}
impl Drop for Job { fn drop(&mut self) { self.token.cancel(); } }

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};
    #[test]
    fn semantic_project_method_completion_is_accepted_and_passes_real_cargo_doctests() {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap(); let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"doc_completion_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[dependencies]\nhelper_alias={package=\"doc_helper_fixture\",path=\"helper\"}\n[workspace]\n").unwrap();
        let helper = root.path().join("helper/src/lib.rs");
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::write(root.path().join("helper/Cargo.toml"), "[package]\nname=\"doc_helper_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n").unwrap();
        std::fs::write(&helper, "pub struct Greeter; impl Greeter { pub fn hello(&self) -> i32 { 42 } }\n").unwrap();
        let fence = char::from(96).to_string().repeat(3);
        let text = format!("pub use helper_alias::Greeter;\n/// {fence}rust\n/// use doc_completion_fixture::Greeter;\n/// let g = Greeter;\n/// g.hello_a|;\n/// {fence}\npub fn example() {{}}\n");
        let caret = text.find('|').unwrap(); let text = text.replace('|', "");
        let path = root.path().join("src/lib.rs"); std::fs::write(&path, &text).unwrap();
        assert!(std::process::Command::new("cargo").args(["generate-lockfile", "--offline"]).current_dir(root.path()).status().unwrap().success());
        let mut app = crate::screens::test_app(home.path(), false);
        app.settings.trusted_projects.push(root.path().canonicalize().unwrap());
        app.open_workspace(root.path()); app.open_file_and_wait(&path);
        let end = Instant::now() + Duration::from_secs(30);
        while app.insight.status != crate::insight::InsightStatus::Ready {
            app.poll_cargo_workspace(); let output = app.insight.update(&mut app.docs, true); app.handle_insight(output);
            assert!(Instant::now() < end, "{:?}", app.insight.status); std::thread::sleep(Duration::from_millis(10));
        }
        app.open_file_and_wait(&helper);
        let doc = app.docs.active_doc_mut().unwrap();
        let source = doc.editor.buffer.rope().to_string();
        doc.editor.replace(0, doc.editor.buffer.len_chars(), &source.replace("fn hello(", "fn hello_after_edit("));
        app.open_file_and_wait(&path);
        app.docs.active_doc_mut().unwrap().editor.selection = rc_text::Selection::point(caret);
        let index = app.docs.active.unwrap();
        app.insight.complete(&mut app.docs.list[index], None);
        let end = Instant::now() + Duration::from_secs(35); let mut messages = Vec::new();
        loop {
            let output = app.insight.update(&mut app.docs, true);
            for message in &output { if let crate::insight::InsightOutput::Message(message) = message { messages.push(message.clone()); } }
            app.handle_insight(output);
            if app.docs.active_doc().unwrap().editor.popups.completion.as_ref().is_some_and(|completion| !completion.loading && !completion.items.is_empty()) { break; }
            assert!(Instant::now() < end, "completion unavailable: {messages:?}"); std::thread::sleep(Duration::from_millis(10));
        }
        let completion = app.docs.active_doc().unwrap().editor.popups.completion.as_ref().unwrap();
        let choice = completion.filtered.iter().position(|index| completion.items[*index].label.starts_with("hello_after_edit")).unwrap_or_else(|| panic!("{:?}", completion.items.iter().map(|item| &item.label).collect::<Vec<_>>()));
        let raw = completion.items[completion.filtered[choice]].data;
        rc_editor::accept_completion(&mut app.docs.list[index].editor, choice, false);
        app.insight.completion_accepted(&mut app.docs.list[index], raw);
        let completed = app.docs.active_doc().unwrap().editor.buffer.rope().to_string();
        assert!(completed.contains("/// g.hello_after_edit();"), "{completed}");
        assert!(!std::fs::read_to_string(&helper).unwrap().contains("hello_after_edit"), "completion used the unsaved dependency buffer");
        app.docs.save(index).unwrap();
        let helper_index = app.docs.index_of(&helper).unwrap(); app.docs.save(helper_index).unwrap();
        let output = std::process::Command::new("cargo").args(["test", "--doc", "--offline"]).current_dir(root.path()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        app.insight.stop();
    }
}

#[cfg(test)]
mod projection_tests {
    use super::*;
    fn projected(text: &str) -> Projection {
        let caret = text.find('|').unwrap(); let caret = text[..caret].chars().count();
        project(&Rope::from_str(&text.replace('|', "")), caret).unwrap()
    }
    #[test]
    fn hidden_lines_block_comments_and_utf16_edits_preserve_documentation_syntax() {
        let fence = char::from(96).to_string().repeat(3);
        let text = format!("/// {fence}rust,no_run,edition2021\n/// # let café = String::from(\"🦀\");\n/// café.le|;\n/// {fence}\n");
        let projection = projected(&text);
        assert_eq!(projection.edition.as_deref(), Some("2021"));
        assert!(projection.source.contains("let café = String::from(\"🦀\");"));
        let emoji = projection.rows[0].code.find('🦀').unwrap();
        let split_surrogate = projection.rows[0].code[..emoji].encode_utf16().count() as u32 + 1;
        assert!(projection.position(lsp::Position::new(1, split_surrogate)).is_none());
        let mut item = lsp::CompletionItem { label: "len".into(), text_edit: Some(lsp::CompletionTextEdit::Edit(lsp::TextEdit { range: lsp::Range::new(projection.position, projection.position), new_text: "len()".into() })), command: Some(lsp::Command { title: "foreign".into(), command: "foreign.execute".into(), arguments: None }), data: Some(json!({ "foreign": true })), ..Default::default() };
        let mapped = projection.item(item.clone(), None).unwrap();
        assert!(mapped.command.is_none() && mapped.data.is_none());
        item.text_edit = Some(lsp::CompletionTextEdit::Edit(lsp::TextEdit { range: lsp::Range::new(projection.position, lsp::Position::new(1, 0)), new_text: "invalid".into() }));
        assert!(projection.item(item, None).is_none());
        let original = projection.position(projection.position).unwrap();
        let caret = text.find('|').unwrap(); let rope = Rope::from_str(&text.replace('|', ""));
        assert_eq!(crate::convert::char_of(&rope, original), text[..caret].chars().count());
        let inserted = projection.text("len()\nnext()", 1).unwrap();
        assert_eq!(inserted, "len()\n/// next()");
        let hidden = projection.text("setup()\nmore()", 0).unwrap();
        assert_eq!(hidden, "setup()\n/// # more()");
        let block = projected(&format!("/**\n * {fence}rust\n * let name = String::new();\n * name.le|;\n * {fence}\n */\npub fn documented() {{}}\n"));
        assert!(block.block_doc);
        assert_eq!(block.text("len()\nmore()", 1).unwrap(), "len()\n * more()");
        assert!(block.text("*/ executable", 1).is_none());
        let non_rust = format!("/// {fence}javascript\n/// ignored\n/// {fence}\n/// regular prose|\n");
        let at = non_rust.find('|').unwrap();
        assert!(matches!(project(&Rope::from_str(&non_rust.replace('|', "")), at), Err(Error::NoContext)));
    }
    #[cfg(unix)]
    #[test]
    fn bounded_tool_output_timeout_and_cancellation_terminate_real_processes() {
        let token = CancelToken::new();
        let mut flood = tokio::process::Command::new("/bin/sh"); flood.args(["-c", "yes output"]);
        assert!(matches!(rc_core::Runtime::block_on(command(flood, &token, Instant::now(), 4096)), Err(Error::Limit)));
        let mut sleeper = tokio::process::Command::new("/bin/sh"); sleeper.args(["-c", "exec sleep 30"]);
        let started = Instant::now() - DEADLINE + Duration::from_millis(40);
        assert!(matches!(rc_core::Runtime::block_on(command(sleeper, &token, started, 4096)), Err(Error::Timeout)));
        let token = CancelToken::new(); let cancel = token.clone();
        std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(40)); cancel.cancel(); });
        let mut sleeper = tokio::process::Command::new("/bin/sh"); sleeper.args(["-c", "exec sleep 30"]);
        assert!(matches!(rc_core::Runtime::block_on(command(sleeper, &token, Instant::now(), 4096)), Err(Error::Cancelled)));
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    fn wait(h: &mut egui_kittest::Harness<'_, crate::app::CrabCraftApp>, label: &str, done: impl Fn(&crate::app::CrabCraftApp) -> bool) {
        let end = Instant::now() + Duration::from_secs(35);
        loop {
            h.run_steps(1); if done(h.state()) { return; }
            assert!(Instant::now() < end, "{label}; insight={:?}", h.state().insight.status);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn complete(h: &mut egui_kittest::Harness<'_, crate::app::CrabCraftApp>) {
        h.state_mut().focus_editor = true; h.run_steps(2);
        let chord = h.state().keymap.chord(rc_editor::Action::Complete).unwrap();
        h.key_press_modifiers(chord.mods, chord.key); h.run_steps(2);
    }
    #[test]
    fn native_ctrl_space_completes_std_api_in_block_doctests_after_unicode_and_hidden_lines() {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap(); let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"doc_std_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        let fence = char::from(96).to_string().repeat(3);
        let text = format!("pub struct Greeter;\n/**\n * {fence}rust\n * # use doc_std_fixture::Greeter;\n * let café = String::from(\"🦀\"); café.le|;\n * {fence}\n */\npub fn example() {{}}\n");
        let marker = text.find('|').unwrap(); let caret = text[..marker].chars().count(); let text = text.replace('|', "");
        let path = root.path().join("src/lib.rs"); std::fs::write(&path, &text).unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace(root.path()); app.open_file_and_wait(&path); app.docs.active_doc_mut().unwrap().editor.selection = rc_text::Selection::point(caret);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 900.0));
        complete(&mut h);
        assert_eq!(h.state().insight.status, crate::insight::InsightStatus::Untrusted);
        assert!(h.state().insight.session.is_none()); assert!(h.state().runner.cargo_workspace.is_none());
        assert!(!root.path().join("Cargo.lock").exists()); assert!(!root.path().join("target").exists());
        h.state_mut().trust_project();
        wait(&mut h, "project analysis", |app| app.insight.status == crate::insight::InsightStatus::Ready);
        complete(&mut h);
        wait(&mut h, "semantic String completion", |app| app.docs.active_doc().unwrap().editor.popups.completion.as_ref().is_some_and(|completion| !completion.loading && !completion.items.is_empty()));
        let completion = h.state().docs.active_doc().unwrap().editor.popups.completion.as_ref().unwrap();
        let choice = completion.filtered.iter().position(|index| completion.items[*index].label.starts_with("len")).unwrap_or_else(|| panic!("{:?}", completion.items.iter().map(|item| &item.label).collect::<Vec<_>>()));
        for _ in 0..choice { h.key_press(egui::Key::ArrowDown); h.run_steps(1); }
        h.key_press(egui::Key::Tab); h.run_steps(3);
        let text = h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();
        assert!(text.contains(" * let café = String::from(\"🦀\"); café.len();"), "{text}");
        assert!(text.contains(" * # use doc_std_fixture::Greeter;"));
        let index = h.state().docs.active.unwrap(); h.state_mut().docs.save(index).unwrap();
        let output = std::process::Command::new("cargo").args(["test", "--doc", "--offline"]).current_dir(root.path()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
    #[cfg(unix)]
    #[test]
    fn escaping_and_editing_a_loading_doctest_cancels_the_analyzer_and_discards_late_results() {
        use std::os::unix::fs::PermissionsExt;
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap(); let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"doc_cancel_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        let fence = char::from(96).to_string().repeat(3);
        let text = format!("pub struct Greeter;\nimpl Greeter {{ pub fn hello(&self) {{}} }}\n/// {fence}rust\n/// let name = String::new(); name.le|;\n/// {fence}\npub fn example() {{}}\n");
        let caret = text.find('|').unwrap(); let text = text.replace('|', "");
        let path = root.path().join("src/lib.rs"); std::fs::write(&path, &text).unwrap();
        let real = rc_cargo::locate_rust_analyzer(None).unwrap();
        let wrapper = home.path().join("analyzer"); let pid_file = home.path().join("pid");
        let quoted = |path: &Path| serde_json::to_string(&path.to_string_lossy()).unwrap();
        std::fs::write(&wrapper, format!("#!/usr/bin/python3\nimport os,sys\nactual={}\nroot={}\nif os.path.realpath(os.getcwd()) == root:\n os.execv(actual,[actual]+sys.argv[1:])\nelse:\n with open({},'w') as f: f.write(str(os.getpid()))\n os.execl('/bin/sleep','sleep','30')\n", quoted(&real), quoted(&root.path().canonicalize().unwrap()), quoted(&pid_file))).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        app.settings.rust_analyzer_path = Some(wrapper); app.settings.trusted_projects.push(root.path().canonicalize().unwrap());
        app.open_workspace(root.path()); app.open_file_and_wait(&path); app.docs.active_doc_mut().unwrap().editor.selection = rc_text::Selection::point(caret);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 900.0));
        wait(&mut h, "project analysis", |app| app.insight.status == crate::insight::InsightStatus::Ready);
        fn pid(file: &Path) -> Option<i32> { std::fs::read_to_string(file).ok()?.parse().ok() }
        fn stopped(pid: i32) -> bool {
            // SAFETY: signal zero only probes the test-owned analyzer process.
            unsafe { libc::kill(pid, 0) != 0 }
        }
        complete(&mut h);
        wait(&mut h, "isolated analyzer startup", |_| pid(&pid_file).is_some());
        let first = pid(&pid_file).unwrap();
        h.key_press(egui::Key::Escape); h.run_steps(2);
        wait(&mut h, "cancelled analyzer reaping", |_| stopped(first));
        assert!(h.state().docs.active_doc().unwrap().editor.popups.completion.is_none());
        complete(&mut h);
        wait(&mut h, "new isolated analyzer", |_| pid(&pid_file).is_some_and(|pid| pid != first));
        let second = pid(&pid_file).unwrap();
        {
            let doc = h.state_mut().docs.active_doc_mut().unwrap();
            doc.editor.replace(caret, caret, "x");
        }
        h.run_steps(2);
        wait(&mut h, "stale analyzer reaping", |_| stopped(second));
        h.run_steps(3);
        assert!(h.state().docs.active_doc().unwrap().editor.popups.completion.is_none());
        assert_eq!(std::fs::read_to_string(path).unwrap(), text);
    }
}
