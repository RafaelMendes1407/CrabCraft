//! Completion inside `Cargo.toml`, without the language server (rust-analyzer does not complete
//! TOML) and without the network: table headers after `[`, the keys valid in the table the caret
//! is in, and values where Cargo accepts a fixed set.
//!
//! [`context`] reads the text up to the caret (which table, key or value position, inside a
//! string or not) and [`complete`] turns it into suggestions. Both work on any text: an invalid
//! or half-typed manifest gives fewer suggestions, never an error.
//!
//! Descriptions are English and short; the caller translates them.

/// What a suggestion completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Table,
    Key,
    Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub label: String,
    /// Inserted on accept, in snippet syntax: `$0` marks where the caret goes.
    pub insert: String,
    pub kind: Kind,
    /// English description of the table or key.
    pub doc: Option<&'static str>,
    /// Accepting it leaves the caret where a value from a fixed set goes.
    pub then_value: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestCompletion {
    /// Char offset where the typed prefix starts (it is replaced on accept).
    pub anchor: usize,
    pub items: Vec<Suggestion>,
}

/// Where the caret is, for completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Context {
    /// Inside `[…` or `[[…` of a table header.
    Header { array: bool },
    /// Where a key starts, in `table` (an inline table's path includes its key).
    Key { table: Vec<String>, inline: bool },
    /// After `key =`, in `table`.
    Value { table: Vec<String>, key: String, in_string: bool },
}

/// The caret's context and the char offset where the word being typed starts. `None` where
/// nothing is completed: comments, multi-line strings, arrays, after a value.
pub fn context(text: &str, caret: usize) -> Option<(usize, Context)> {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let mut table: Vec<String> = Vec::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut st = Stmt::new(0);
    let mut i = 0;
    while i < caret {
        let c = chars[i];
        match c {
            '#' => {
                while i < caret && chars[i] != '\n' {
                    i += 1;
                }
                if i >= caret {
                    return None;
                }
                continue;
            }
            '"' | '\'' => {
                let triple = chars.get(i + 1) == Some(&c) && chars.get(i + 2) == Some(&c);
                let start = i + if triple { 3 } else { 1 };
                match string_end(&chars, start, caret, c, triple) {
                    Some(end) => {
                        if st.key.is_some() || matches!(frames.last(), Some(Frame { array: true, .. })) {
                            st.after_value = true;
                        }
                        i = end;
                        continue;
                    }
                    None => {
                        // The caret is inside this string: only a single-line value string.
                        let in_array = matches!(frames.last(), Some(Frame { array: true, .. }));
                        let key = st.key.as_ref()?;
                        if triple || in_array || st.after_value || !blank(&chars[st.value_start..i]) {
                            return None;
                        }
                        let (table, key) = value_key(&current_path(&table, &frames), key)?;
                        return Some((start, Context::Value { table, key, in_string: true }));
                    }
                }
            }
            '[' if frames.is_empty() && st.key.is_none() && blank(&chars[st.start..i]) => {
                let array = chars.get(i + 1) == Some(&'[');
                let open = i + if array { 2 } else { 1 };
                let mut j = open;
                let mut quote = None;
                while j < caret && chars[j] != '\n' && (quote.is_some() || chars[j] != ']') {
                    match (quote, chars[j]) {
                        (None, q @ ('"' | '\'')) => quote = Some(q),
                        (Some(q), ch) if ch == q => quote = None,
                        _ => {}
                    }
                    j += 1;
                }
                if j >= caret {
                    let anchor = (open..caret).find(|&k| !chars[k].is_whitespace()).unwrap_or(caret);
                    return Some((anchor, Context::Header { array }));
                }
                if chars[j] == ']' {
                    table = split_key(&chars[open..j].iter().collect::<String>());
                }
                // The rest of the header line holds nothing to complete.
                while i < caret && chars[i] != '\n' {
                    i += 1;
                }
                if i >= caret {
                    return None;
                }
                st = Stmt::new(i + 1);
                i += 1;
                continue;
            }
            '=' if st.key.is_none() && !st.after_value => {
                st.key = Some(split_key(&chars[st.start..i].iter().collect::<String>()));
                st.value_start = i + 1;
            }
            '{' | '[' if st.key.is_some() && !st.after_value && blank(&chars[st.value_start..i]) => {
                let key = st.key.clone().unwrap_or_default();
                let mut path = current_path(&table, &frames);
                path.extend(key.iter().cloned());
                let array = c == '[';
                let saved = std::mem::replace(&mut st, Stmt::new(i + 1));
                if array {
                    // Elements are values of the array's key.
                    st.key = Some(key);
                    st.value_start = i + 1;
                }
                frames.push(Frame { path, array, saved });
            }
            '[' if matches!(frames.last(), Some(Frame { array: true, .. })) && blank(&chars[st.value_start..i]) => {
                // A nested array.
                let path = current_path(&table, &frames);
                let saved = std::mem::replace(&mut st, Stmt::new(i + 1));
                st.key = saved.key.clone();
                st.value_start = i + 1;
                frames.push(Frame { path, array: true, saved });
            }
            ',' => match frames.last() {
                Some(Frame { array: false, .. }) => st = Stmt::new(i + 1),
                Some(Frame { array: true, saved, .. }) => {
                    let key = saved.key.clone();
                    st = Stmt::new(i + 1);
                    st.key = key;
                }
                None => {}
            },
            '}' | ']' if !frames.is_empty() => {
                if let Some(f) = frames.pop() {
                    st = f.saved;
                    st.after_value = true;
                }
            }
            '\n' if frames.is_empty() => st = Stmt::new(i + 1),
            _ => {}
        }
        i += 1;
    }
    if matches!(frames.last(), Some(Frame { array: true, .. })) || st.after_value {
        return None;
    }
    let word = word_start(&chars, caret);
    match &st.key {
        None => {
            if !blank(&chars[st.start..word]) {
                return None;
            }
            Some((word, Context::Key { table: current_path(&table, &frames), inline: !frames.is_empty() }))
        }
        Some(key) => {
            if !blank(&chars[st.value_start..word]) {
                return None;
            }
            let (table, key) = value_key(&current_path(&table, &frames), key)?;
            Some((word, Context::Value { table, key, in_string: false }))
        }
    }
}

/// Suggestions at `caret` (a char offset) in the manifest `text`.
pub fn complete(text: &str, caret: usize) -> Option<ManifestCompletion> {
    let (anchor, ctx) = context(text, caret)?;
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let line_end = (caret..chars.len()).find(|&k| chars[k] == '\n').unwrap_or(chars.len());
    // What follows the word under the caret on its line.
    let after_word = (caret..line_end).find(|&k| !is_word(chars[k])).unwrap_or(line_end);
    let rest: String = chars[after_word..line_end].iter().collect();
    let items = match ctx {
        Context::Header { array } => {
            let existing = existing_headers(text, chars[..caret].iter().filter(|&&c| c == '\n').count());
            let closes = rest.contains(']');
            HEADERS
                .iter()
                .filter(|h| if array { h.array } else { !existing.iter().any(|e| e == h.name) || h.array })
                .map(|h| {
                    let close = if closes { "" } else if h.array { "]]" } else { "]" };
                    // From a single `[`, an array table gets its second bracket.
                    let open = if h.array && !array { "[" } else { "" };
                    let close = if h.array && !array && closes { "]" } else { close };
                    Suggestion { label: h.label.to_string(), insert: format!("{open}{}{close}", h.insert), kind: Kind::Table, doc: Some(h.doc), then_value: false }
                })
                .collect()
        }
        Context::Key { table, inline } => {
            let defs = keys_for(&table)?;
            let existing = if inline { Vec::new() } else { existing_keys(&chars, caret) };
            let rest = rest.trim_start();
            let bare = !(rest.is_empty() || inline && (rest.starts_with(',') || rest.starts_with('}')));
            defs.iter()
                .filter(|d| !existing.iter().any(|e| e == d.name))
                .map(|d| {
                    let insert = if bare {
                        d.name.to_string()
                    } else {
                        match d.shape {
                            Shape::Str => format!("{} = \"$0\"", d.name),
                            Shape::Array => format!("{} = [$0]", d.name),
                            Shape::Table => format!("{} = {{ $0 }}", d.name),
                            Shape::Bool | Shape::Other => format!("{} = $0", d.name),
                        }
                    };
                    let then_value = !bare && !values_for(&table, d.name).is_empty();
                    Suggestion { label: d.name.to_string(), insert, kind: Kind::Key, doc: Some(d.doc), then_value }
                })
                .collect()
        }
        Context::Value { table, key, in_string } => {
            let quote = if in_string { chars[..caret].iter().rev().find(|&&c| c == '"' || c == '\'').copied().unwrap_or('"') } else { '"' };
            let closed = chars.get(after_word) == Some(&quote);
            values_for(&table, &key)
                .into_iter()
                .filter(|v| !in_string || v.quoted)
                .map(|v| {
                    let insert = match (in_string, v.quoted) {
                        (true, _) if closed => v.text.to_string(),
                        (true, _) => format!("{}{quote}", v.text),
                        (false, true) => format!("\"{}\"", v.text),
                        (false, false) => v.text.to_string(),
                    };
                    let label = if in_string || !v.quoted { v.text.to_string() } else { format!("\"{}\"", v.text) };
                    Suggestion { label, insert, kind: Kind::Value, doc: None, then_value: false }
                })
                .collect()
        }
    };
    Some(ManifestCompletion { anchor, items })
}

// ---------------------------------------------------------------------------- scanning

struct Frame {
    path: Vec<String>,
    array: bool,
    /// The statement the inline table or array belongs to.
    saved: Stmt,
}

/// One `key = value` in progress.
#[derive(Clone)]
struct Stmt {
    start: usize,
    key: Option<Vec<String>>,
    value_start: usize,
    after_value: bool,
}

impl Stmt {
    fn new(start: usize) -> Self {
        Stmt { start, key: None, value_start: start, after_value: false }
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn word_start(chars: &[char], caret: usize) -> usize {
    let mut k = caret;
    while k > 0 && is_word(chars[k - 1]) {
        k -= 1;
    }
    k
}

fn blank(chars: &[char]) -> bool {
    chars.iter().all(|c| c.is_whitespace())
}

/// Where the string that starts at `start` ends (after its closing quote), or `None` when it
/// is still open at `limit`. A one-line string also ends at the end of its line.
fn string_end(chars: &[char], start: usize, limit: usize, quote: char, triple: bool) -> Option<usize> {
    let mut j = start;
    while j < limit {
        let c = chars[j];
        if quote == '"' && c == '\\' {
            j += 2;
            continue;
        }
        if triple {
            if c == quote && chars.get(j + 1) == Some(&quote) && chars.get(j + 2) == Some(&quote) {
                return Some(j + 3);
            }
        } else if c == quote {
            return Some(j + 1);
        } else if c == '\n' {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// The parts of a dotted key, unquoted: `target.'cfg(unix)'.dependencies` → three parts.
pub(crate) fn split_key(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote = None;
    for c in s.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '.') => parts.push(std::mem::take(&mut cur).trim().to_string()),
            (None, c) if c.is_whitespace() => {}
            _ => cur.push(c),
        }
    }
    parts.push(cur.trim().to_string());
    parts
}

fn current_path(table: &[String], frames: &[Frame]) -> Vec<String> {
    frames.last().map_or_else(|| table.to_vec(), |f| f.path.clone())
}

/// A dotted key's last part, with the parts before it added to the table.
fn value_key(table: &[String], key: &[String]) -> Option<(Vec<String>, String)> {
    let (last, parents) = key.split_last()?;
    let mut t = table.to_vec();
    t.extend(parents.iter().cloned());
    Some((t, last.clone()))
}

/// Names of the tables declared with `[…]` (not `[[…]]`), except on line `skip`.
fn existing_headers(text: &str, skip: usize) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter(|(n, _)| *n != skip)
        .filter_map(|(_, l)| {
            let l = l.trim();
            let inner = l.strip_prefix('[').filter(|r| !r.starts_with('['))?;
            let inner = &inner[..inner.find(']')?];
            Some(split_key(inner).join("."))
        })
        .collect()
}

/// Keys written at the start of a line in the caret's table section, except on its line.
fn existing_keys(chars: &[char], caret: usize) -> Vec<String> {
    let text: String = chars.iter().collect();
    let caret_line = chars[..caret].iter().filter(|&&c| c == '\n').count();
    let lines: Vec<&str> = text.lines().collect();
    let is_header = |l: &str| l.trim_start().starts_with('[');
    let first = (0..caret_line.min(lines.len())).rev().find(|&n| is_header(lines[n])).map_or(0, |n| n + 1);
    let last = (caret_line + 1..lines.len()).find(|&n| is_header(lines[n])).unwrap_or(lines.len());
    (first..last)
        .filter(|&n| n != caret_line)
        .filter_map(|n| {
            let (key, _) = lines.get(n)?.split_once('=')?;
            let key = key.trim();
            (!key.is_empty() && key.chars().all(is_word)).then(|| key.to_string())
        })
        .collect()
}

// ---------------------------------------------------------------------------- the schema

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Str,
    Bool,
    Array,
    Table,
    /// A number, or a value of several types.
    Other,
}

pub struct KeyDef {
    pub name: &'static str,
    shape: Shape,
    pub doc: &'static str,
}

const fn k(name: &'static str, shape: Shape, doc: &'static str) -> KeyDef {
    KeyDef { name, shape, doc }
}

pub struct HeaderDef {
    pub label: &'static str,
    /// The name as written, in snippet syntax.
    insert: &'static str,
    /// The table's name, to hide headers the file already has.
    name: &'static str,
    array: bool,
    pub doc: &'static str,
}

const fn h(name: &'static str, doc: &'static str) -> HeaderDef {
    HeaderDef { label: name, insert: name, name, array: false, doc }
}

const fn ha(name: &'static str, doc: &'static str) -> HeaderDef {
    HeaderDef { label: name, insert: name, name, array: true, doc }
}

const DOC_EDITION: &str = "The Rust edition the code is compiled with.";
const DOC_RESOLVER: &str = "Which version of Cargo's dependency feature resolver to use.";
const DOC_METADATA: &str = "Free-form settings that external tools read; Cargo ignores them.";
const DOC_WS_PACKAGE: &str = "Package values that members can inherit with key.workspace = true.";
const DOC_WS_DEPS: &str = "Dependencies that members can inherit with workspace = true.";
const DOC_WS_LINTS: &str = "Lint levels that members can inherit.";
const DOC_LINTS_RUST: &str = "Levels for the compiler's own lints.";
const DOC_LINTS_CLIPPY: &str = "Levels for Clippy lints.";
const DOC_LINTS_RUSTDOC: &str = "Levels for rustdoc lints.";
const DOC_CFG_DEPS: &str = "Dependencies used only on targets that match the condition.";

pub const HEADERS: &[HeaderDef] = &[
    h("package", "Information about the package and how it is built."),
    h("dependencies", "Libraries the package uses."),
    h("dev-dependencies", "Libraries used only by tests, examples and benchmarks."),
    h("build-dependencies", "Libraries used only by the build script."),
    h("features", "Optional features and what each one turns on."),
    h("lib", "Settings of the library target."),
    ha("bin", "A binary target; repeat the table for each binary."),
    ha("test", "An integration test target."),
    ha("example", "An example target."),
    ha("bench", "A benchmark target."),
    h("workspace", "A group of packages that share a lock file and an output directory."),
    h("workspace.package", DOC_WS_PACKAGE),
    h("workspace.dependencies", DOC_WS_DEPS),
    h("workspace.lints.rust", DOC_WS_LINTS),
    h("workspace.lints.clippy", DOC_WS_LINTS),
    h("workspace.metadata", DOC_METADATA),
    h("profile.dev", "Compiler settings for development builds."),
    h("profile.release", "Compiler settings for release builds."),
    h("profile.test", "Compiler settings for tests."),
    h("profile.bench", "Compiler settings for benchmarks."),
    HeaderDef { label: "target.'cfg(…)'.dependencies", insert: "target.'${1:cfg(unix)}'.dependencies", name: "", array: false, doc: DOC_CFG_DEPS },
    HeaderDef { label: "target.'cfg(…)'.dev-dependencies", insert: "target.'${1:cfg(unix)}'.dev-dependencies", name: "", array: false, doc: DOC_CFG_DEPS },
    HeaderDef { label: "target.'cfg(…)'.build-dependencies", insert: "target.'${1:cfg(unix)}'.build-dependencies", name: "", array: false, doc: DOC_CFG_DEPS },
    h("lints", "Lint levels for this package."),
    h("lints.rust", DOC_LINTS_RUST),
    h("lints.clippy", DOC_LINTS_CLIPPY),
    h("lints.rustdoc", DOC_LINTS_RUSTDOC),
    h("badges", "Badges for registries; crates.io no longer shows them."),
    h("patch.crates-io", "Replaces crates.io dependencies with another source, such as a local copy."),
    h("replace", "Replaces exact dependency versions; patch is the newer way."),
    h("package.metadata", DOC_METADATA),
];

const ROOT_KEYS: &[KeyDef] = &[k("cargo-features", Shape::Array, "Unstable Cargo features this manifest uses (nightly only).")];

const PACKAGE_KEYS: &[KeyDef] = &[
    k("name", Shape::Str, "The package name, also used for its crates and on crates.io."),
    k("version", Shape::Str, "The package version, in semantic versioning."),
    k("edition", Shape::Str, DOC_EDITION),
    k("rust-version", Shape::Str, "The oldest Rust version that can build the package."),
    k("authors", Shape::Array, "The people credited as the package's authors."),
    k("description", Shape::Str, "A one-line summary of the package, shown on crates.io."),
    k("documentation", Shape::Str, "The address of the package's documentation."),
    k("readme", Shape::Other, "The README file shown on crates.io, or false for none."),
    k("homepage", Shape::Str, "The address of the package's home page."),
    k("repository", Shape::Str, "The address of the package's source repository."),
    k("license", Shape::Str, "The licence as an SPDX expression, such as MIT OR Apache-2.0."),
    k("license-file", Shape::Str, "A licence file, for licences without an SPDX identifier."),
    k("keywords", Shape::Array, "Up to five words that help people find the package."),
    k("categories", Shape::Array, "The crates.io categories the package belongs to."),
    k("workspace", Shape::Str, "The workspace root, when searching the parent directories would not find it."),
    k("build", Shape::Other, "The build script run before compiling, or false to turn it off."),
    k("links", Shape::Str, "The native library the package links, so only one package links it."),
    k("exclude", Shape::Array, "Files left out of the published package."),
    k("include", Shape::Array, "The only files put in the published package."),
    k("publish", Shape::Other, "Whether the package may be published, or the registries it may go to."),
    k("default-run", Shape::Str, "The binary cargo run starts when the package has several."),
    k("autobins", Shape::Bool, "Whether files in src/bin become binaries without a [[bin]] table."),
    k("autoexamples", Shape::Bool, "Whether files in examples become examples without a table."),
    k("autotests", Shape::Bool, "Whether files in tests become tests without a table."),
    k("autobenches", Shape::Bool, "Whether files in benches become benchmarks without a table."),
    k("resolver", Shape::Str, DOC_RESOLVER),
];

/// Package keys members can inherit from `[workspace.package]`.
const INHERITABLE: &[&str] =
    &["version", "authors", "description", "documentation", "readme", "homepage", "repository", "license", "license-file", "keywords", "categories", "edition", "rust-version", "publish", "exclude", "include"];

const WORKSPACE_KEYS: &[KeyDef] = &[
    k("members", Shape::Array, "Paths or glob patterns of the packages in the workspace."),
    k("exclude", Shape::Array, "Paths kept out of the workspace even when a member pattern matches."),
    k("default-members", Shape::Array, "The members commands act on when no package is chosen."),
    k("resolver", Shape::Str, DOC_RESOLVER),
    k("package", Shape::Table, DOC_WS_PACKAGE),
    k("dependencies", Shape::Table, DOC_WS_DEPS),
    k("lints", Shape::Table, DOC_WS_LINTS),
    k("metadata", Shape::Table, DOC_METADATA),
];

const TARGET_KEYS: &[KeyDef] = &[
    k("name", Shape::Str, "The target's name."),
    k("path", Shape::Str, "The target's source file, relative to the manifest."),
    k("test", Shape::Bool, "Whether cargo test builds and runs the target's tests."),
    k("doctest", Shape::Bool, "Whether the examples in the documentation are tested."),
    k("bench", Shape::Bool, "Whether cargo bench runs the target's benchmarks."),
    k("doc", Shape::Bool, "Whether cargo doc documents the target."),
    k("harness", Shape::Bool, "Whether the built-in test harness provides main."),
    k("edition", Shape::Str, DOC_EDITION),
    k("crate-type", Shape::Array, "The kinds of crate to build, such as lib, cdylib or staticlib."),
    k("required-features", Shape::Array, "Features that must be on for the target to be built."),
    k("proc-macro", Shape::Bool, "Whether the library is a procedural macro crate."),
];

const PROFILE_KEYS: &[KeyDef] = &[
    k("opt-level", Shape::Other, "How much the compiler optimises: 0 to 3, or s and z for size."),
    k("debug", Shape::Other, "How much debug information goes into the output."),
    k("split-debuginfo", Shape::Str, "Whether debug information is kept in separate files."),
    k("strip", Shape::Other, "Whether symbols or debug information are removed from the output."),
    k("debug-assertions", Shape::Bool, "Whether debug_assert! and cfg(debug_assertions) are on."),
    k("overflow-checks", Shape::Bool, "Whether integer overflow panics at run time."),
    k("lto", Shape::Other, "Link-time optimisation across crates: off, thin or fat."),
    k("panic", Shape::Str, "What a panic does: unwind the stack or abort the process."),
    k("incremental", Shape::Bool, "Whether the compiler reuses work from earlier builds."),
    k("codegen-units", Shape::Other, "How many parts a crate is split into for parallel code generation."),
    k("rpath", Shape::Bool, "Whether the output records a run-time library search path."),
    k("inherits", Shape::Str, "The profile a custom profile starts from."),
    k("package", Shape::Table, "Settings that differ for some dependencies."),
    k("build-override", Shape::Table, "Settings for build scripts, procedural macros and their dependencies."),
];

/// Profile keys that cannot be set per package.
const PROFILE_ONLY: &[&str] = &["lto", "panic", "rpath", "inherits", "package", "build-override"];

const DEPENDENCY_KEYS: &[KeyDef] = &[
    k("version", Shape::Str, "The versions of the dependency that are accepted."),
    k("path", Shape::Str, "A local directory that holds the dependency."),
    k("git", Shape::Str, "A Git repository the dependency comes from."),
    k("branch", Shape::Str, "The Git branch to use."),
    k("tag", Shape::Str, "The Git tag to use."),
    k("rev", Shape::Str, "The Git commit to use."),
    k("features", Shape::Array, "Features of the dependency to turn on."),
    k("optional", Shape::Bool, "Whether the dependency is built only when a feature asks for it."),
    k("default-features", Shape::Bool, "Whether the dependency's default features are on."),
    k("package", Shape::Str, "The crate's real name, when the key renames it."),
    k("workspace", Shape::Bool, "Whether the dependency comes from the workspace's dependencies."),
    k("registry", Shape::Str, "The registry the dependency comes from, instead of crates.io."),
];

const LINTS_KEYS: &[KeyDef] = &[
    k("rust", Shape::Table, DOC_LINTS_RUST),
    k("clippy", Shape::Table, DOC_LINTS_CLIPPY),
    k("rustdoc", Shape::Table, DOC_LINTS_RUSTDOC),
    k("workspace", Shape::Bool, "Whether the package uses the workspace's lint levels."),
];

const LINT_KEYS: &[KeyDef] = &[
    k("level", Shape::Str, "How the lint is reported: allow, warn, deny or forbid."),
    k("priority", Shape::Other, "The order lint settings apply in; a higher number wins."),
];

const FEATURES_KEYS: &[KeyDef] = &[k("default", Shape::Array, "The features that are on unless the user turns them off.")];

const LINT_TOOLS: &[&str] = &["rust", "clippy", "rustdoc"];

fn is_deps(s: &str) -> bool {
    matches!(s, "dependencies" | "dev-dependencies" | "build-dependencies")
}

/// The keys valid in `table`; `None` where keys are names the user chooses.
fn keys_for(table: &[String]) -> Option<Vec<&'static KeyDef>> {
    let t: Vec<&str> = table.iter().map(String::as_str).collect();
    let all = |defs: &'static [KeyDef]| Some(defs.iter().collect());
    let only = |defs: &'static [KeyDef], names: &[&str]| Some(defs.iter().filter(|d| names.contains(&d.name)).collect());
    let without = |defs: &'static [KeyDef], names: &[&str]| Some(defs.iter().filter(|d| !names.contains(&d.name)).collect());
    match t.as_slice() {
        [] => all(ROOT_KEYS),
        ["package"] => all(PACKAGE_KEYS),
        ["workspace"] => all(WORKSPACE_KEYS),
        ["workspace", "package"] => only(PACKAGE_KEYS, INHERITABLE),
        ["lib"] => without(TARGET_KEYS, &["required-features"]),
        ["bin" | "test" | "bench"] => without(TARGET_KEYS, &["crate-type", "proc-macro", "doctest"]),
        ["example"] => without(TARGET_KEYS, &["proc-macro", "doctest"]),
        ["profile", _] => all(PROFILE_KEYS),
        ["profile", _, "build-override"] | ["profile", _, "package", _] => without(PROFILE_KEYS, PROFILE_ONLY),
        [d, _] if is_deps(d) => all(DEPENDENCY_KEYS),
        ["target", _, d, _] if is_deps(d) => all(DEPENDENCY_KEYS),
        ["workspace", "dependencies", _] | ["patch", _, _] | ["replace", _] => without(DEPENDENCY_KEYS, &["workspace"]),
        ["lints"] => all(LINTS_KEYS),
        ["workspace", "lints"] => without(LINTS_KEYS, &["workspace"]),
        ["lints", tool, _] | ["workspace", "lints", tool, _] if LINT_TOOLS.contains(tool) => all(LINT_KEYS),
        ["features"] => all(FEATURES_KEYS),
        _ => None,
    }
}

pub struct Value {
    pub text: &'static str,
    /// Written as a TOML string.
    pub quoted: bool,
}

const fn q(text: &'static str) -> Value {
    Value { text, quoted: true }
}

const fn b(text: &'static str) -> Value {
    Value { text, quoted: false }
}

const LEVELS: [Value; 4] = [q("allow"), q("warn"), q("deny"), q("forbid")];

/// The values `key` accepts in `table` when they form a fixed set.
fn values_for(table: &[String], key: &str) -> Vec<Value> {
    let t: Vec<&str> = table.iter().map(String::as_str).collect();
    let lint_table = matches!(t.as_slice(), ["lints", tool] | ["workspace", "lints", tool] if LINT_TOOLS.contains(tool));
    if lint_table {
        return LEVELS.into();
    }
    let in_lint = matches!(t.as_slice(), ["lints", tool, _] | ["workspace", "lints", tool, _] if LINT_TOOLS.contains(tool));
    if in_lint && key == "level" {
        return LEVELS.into();
    }
    let profile = matches!(t.first(), Some(&"profile"));
    match key {
        "edition" => vec![q("2024"), q("2021"), q("2018"), q("2015")],
        "resolver" if matches!(t.as_slice(), ["package"] | ["workspace"]) => vec![q("3"), q("2"), q("1")],
        "panic" if profile => vec![q("unwind"), q("abort")],
        "lto" if profile => vec![b("false"), b("true"), q("off"), q("thin"), q("fat")],
        "opt-level" if profile => vec![b("0"), b("1"), b("2"), b("3"), q("s"), q("z")],
        "strip" if profile => vec![b("false"), b("true"), q("none"), q("debuginfo"), q("symbols")],
        "debug" if profile => vec![b("false"), b("true"), b("0"), b("1"), b("2"), q("none"), q("line-directives-only"), q("line-tables-only"), q("limited"), q("full")],
        "split-debuginfo" if profile => vec![q("off"), q("packed"), q("unpacked")],
        "publish" if matches!(t.as_slice(), ["package"] | ["workspace", "package"]) => vec![b("true"), b("false")],
        "readme" | "build" if t.as_slice() == ["package"] => vec![b("false")],
        _ => match keys_for(table).and_then(|defs| defs.into_iter().find(|d| d.name == key)) {
            Some(d) if d.shape == Shape::Bool => vec![b("true"), b("false")],
            _ => Vec::new(),
        },
    }
}

/// Every description shown to the user, to check that each one has a translation.
pub fn all_docs() -> Vec<&'static str> {
    let keys = [ROOT_KEYS, PACKAGE_KEYS, WORKSPACE_KEYS, TARGET_KEYS, PROFILE_KEYS, DEPENDENCY_KEYS, LINTS_KEYS, LINT_KEYS, FEATURES_KEYS];
    let mut docs: Vec<&'static str> = keys.iter().flat_map(|defs| defs.iter().map(|d| d.doc)).chain(HEADERS.iter().map(|h| h.doc)).collect();
    docs.sort_unstable();
    docs.dedup();
    docs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text with `|` marking the caret: (text, caret char offset).
    fn at(s: &str) -> (String, usize) {
        let caret = s.chars().position(|c| c == '|').unwrap_or(s.chars().count());
        (s.replacen('|', "", 1), caret)
    }

    fn ctx(s: &str) -> Option<Context> {
        let (text, caret) = at(s);
        context(&text, caret).map(|(_, c)| c)
    }

    fn labels(s: &str) -> Vec<String> {
        let (text, caret) = at(s);
        complete(&text, caret).map(|c| c.items.into_iter().map(|i| i.label).collect()).unwrap_or_default()
    }

    fn item(s: &str, label: &str) -> Suggestion {
        let (text, caret) = at(s);
        let c = complete(&text, caret).unwrap_or_else(|| panic!("no completion in {s:?}"));
        c.items.into_iter().find(|i| i.label == label).unwrap_or_else(|| panic!("no {label} in {s:?}"))
    }

    fn path(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn headers_after_a_bracket() {
        assert_eq!(ctx("[package]\nname = \"a\"\n\n[|"), Some(Context::Header { array: false }));
        assert_eq!(ctx("[[|"), Some(Context::Header { array: true }));
        let (text, caret) = at("[a]\n[  dev|");
        assert_eq!(context(&text, caret).map(|(a, _)| a), Some(7));
        let l = labels("[package]\nname = \"a\"\n[|");
        assert!(l.contains(&"dependencies".to_string()) && l.contains(&"profile.release".to_string()) && l.contains(&"bin".to_string()));
        // Tables the file already has are not offered again; array tables are.
        assert!(!l.contains(&"package".to_string()));
        assert_eq!(labels("[[|"), vec!["bin", "test", "example", "bench"]);
    }

    #[test]
    fn header_inserts_close_the_brackets() {
        assert_eq!(item("[|", "dependencies").insert, "dependencies]");
        assert_eq!(item("[|]", "dependencies").insert, "dependencies");
        assert_eq!(item("[|]", "bin").insert, "[bin]");
        assert_eq!(item("[|", "bin").insert, "[bin]]");
        assert_eq!(item("[[|", "bin").insert, "bin]]");
        assert_eq!(item("[[|]]", "bin").insert, "bin");
        assert_eq!(item("[|", "target.'cfg(…)'.dependencies").insert, "target.'${1:cfg(unix)}'.dependencies]");
    }

    #[test]
    fn keys_of_the_current_table() {
        assert_eq!(ctx("[package]\nna|"), Some(Context::Key { table: path(&["package"]), inline: false }));
        let l = labels("[package]\nname = \"demo\"\n|\n[dependencies]\n");
        assert!(l.contains(&"edition".to_string()) && l.contains(&"rust-version".to_string()));
        assert!(!l.contains(&"name".to_string()), "already written");
        assert!(labels("[[bin]]\n|").contains(&"required-features".to_string()));
        assert!(!labels("[lib]\n|").contains(&"required-features".to_string()));
        assert!(labels("[profile.release]\n|").contains(&"codegen-units".to_string()));
        assert!(!labels("[profile.release.package.foo]\n|").contains(&"lto".to_string()));
        assert!(labels("[workspace]\n|").contains(&"resolver".to_string()));
        assert!(labels("[dependencies.serde]\n|").contains(&"default-features".to_string()));
        assert!(labels("[target.'cfg(unix)'.dependencies.libc]\n|").contains(&"version".to_string()));
        // Dependency names are the user's.
        assert!(labels("[dependencies]\n|").is_empty());
    }

    #[test]
    fn keys_of_an_inline_dependency_table() {
        assert_eq!(ctx("[dependencies]\nserde = { ver|"), Some(Context::Key { table: path(&["dependencies", "serde"]), inline: true }));
        assert_eq!(ctx("[dependencies]\nserde = { version = \"1\", fe| }"), Some(Context::Key { table: path(&["dependencies", "serde"]), inline: true }));
        assert_eq!(item("[dependencies]\nserde = { |}", "features").insert, "features = [$0]");
        assert_eq!(item("[dependencies]\nserde = { |", "version").insert, "version = \"$0\"");
    }

    #[test]
    fn key_inserts_skip_the_equals_sign_when_one_follows() {
        assert_eq!(item("[package]\n|", "edition").insert, "edition = \"$0\"");
        assert!(item("[package]\n|", "edition").then_value);
        assert_eq!(item("[package]\ned| = \"2021\"", "edition").insert, "edition");
        assert_eq!(item("[profile.dev]\n|", "incremental").insert, "incremental = $0");
    }

    #[test]
    fn values_from_a_fixed_set() {
        assert_eq!(ctx("[package]\nedition = \"|\""), Some(Context::Value { table: path(&["package"]), key: "edition".into(), in_string: true }));
        assert_eq!(labels("[package]\nedition = \"|\""), vec!["2024", "2021", "2018", "2015"]);
        assert_eq!(item("[package]\nedition = \"|\"", "2021").insert, "2021");
        assert_eq!(item("[package]\nedition = \"|", "2021").insert, "2021\"");
        assert_eq!(item("[package]\nedition = |", "\"2021\"").insert, "\"2021\"");
        assert_eq!(labels("[workspace]\nresolver = \"|\""), vec!["3", "2", "1"]);
        assert_eq!(labels("[profile.release]\npanic = \"|\""), vec!["unwind", "abort"]);
        assert!(labels("[profile.release]\nlto = |").contains(&"\"thin\"".to_string()));
        assert!(labels("[profile.release]\nopt-level = |").contains(&"\"z\"".to_string()));
        assert!(labels("[profile.release]\nstrip = |").contains(&"\"symbols\"".to_string()));
        assert_eq!(labels("[profile.dev]\nincremental = t|"), vec!["true", "false"]);
        assert_eq!(labels("[dependencies]\nserde = { version = \"1\", optional = |"), vec!["true", "false"]);
        assert_eq!(labels("[lints.clippy]\npedantic = \"|\""), vec!["allow", "warn", "deny", "forbid"]);
        assert_eq!(labels("[lints.rust]\nunsafe_code = { level = \"|\" }"), vec!["allow", "warn", "deny", "forbid"]);
        // Free text has no fixed set.
        assert!(labels("[package]\nname = \"|\"").is_empty());
    }

    #[test]
    fn nothing_where_there_is_nothing_to_complete() {
        assert_eq!(ctx("[package]\n# a comm|"), None);
        assert_eq!(ctx("[package]\nname = \"a\" |"), None);
        assert_eq!(ctx("[workspace]\nmembers = [\"cr|"), None);
        assert_eq!(ctx("[workspace]\nmembers = [\n  \"a\",\n  |"), None);
        assert_eq!(ctx("[package]\ndescription = \"\"\"\nmulti|"), None);
        assert_eq!(ctx("[package] |"), None);
        // After a multi-line array closes, keys again.
        assert_eq!(ctx("[workspace]\nmembers = [\n  \"a\",\n]\nres|"), Some(Context::Key { table: path(&["workspace"]), inline: false }));
        // Brackets inside strings do not open anything.
        assert_eq!(ctx("[package]\ndescription = \"a [b {\"\nna|"), Some(Context::Key { table: path(&["package"]), inline: false }));
    }

    #[test]
    fn any_input_is_safe() {
        let samples = ["", "[", "[[", "\"", "'''", "a = {", "a = [", "]]]", "}}}", "= = =", "[a.'b", "x = \"\\", "[p]\na = { b = [ { c = \"", "ñ = \"é", "\u{0}"];
        for s in samples {
            for caret in 0..=s.chars().count() + 2 {
                let _ = complete(s, caret);
            }
        }
    }

    #[test]
    fn every_key_has_a_description() {
        assert!(all_docs().iter().all(|d| !d.is_empty() && d.ends_with('.')));
    }
}
