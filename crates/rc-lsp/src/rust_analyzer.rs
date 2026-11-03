//! rust-analyzer: starting it over stdio and its `initialize` handshake.
//!
//! The server runs the project's `build.rs` files and procedural macros, so the application only
//! starts it for projects the user trusts.

use crate::roslyn::client_capabilities;
use crate::rpc::{Client, Incoming, Pending};
use crate::sync::path_to_uri;
use rc_core::inbox::Outbox;
use serde_json::{Value, json};
use std::path::Path;

/// Starts the server with the project's environment `env`, which the Cargo commands, build
/// scripts and procedural macros it runs inherit; must run inside the tokio runtime.
pub fn start(program: &Path, workspace: &Path, env: &[(String, String)], incoming: Outbox<Incoming>) -> std::io::Result<Client> {
    Client::spawn(program, &[], workspace, env, incoming)
}

/// What CrabCraft sets in rust-analyzer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RaSettings {
    /// Clippy instead of `cargo check` on save.
    pub clippy: bool,
    /// Folders and files relative to the workspace root that the server leaves out.
    pub exclude: Vec<String>,
    /// The `Cargo.toml` files to analyse (`linkedProjects`, which replaces the server's own
    /// discovery); empty lets the server find them.
    pub linked: Vec<std::path::PathBuf>,
    /// Paths completion never offers to import (`std::sync::atomic::Ordering`, a module…).
    pub autoimport_exclude: Vec<String>,
}

/// The settings sent at start and when the server asks for them.
pub fn config(s: &RaSettings) -> Value {
    let mut value = config_base(s.clippy, &s.exclude);
    if let Some(map) = value.as_object_mut() {
        if !s.linked.is_empty() {
            map.insert("linkedProjects".into(), json!(s.linked.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()));
        }
        let exclude: Vec<Value> = s.autoimport_exclude.iter().map(|p| json!({ "path": p, "type": "always" })).collect();
        map.insert("completion".into(), json!({ "autoimport": { "enable": true, "exclude": exclude }, "postfix": { "enable": true } }));
    }
    value
}

fn config_base(clippy: bool, exclude: &[String]) -> Value {
    json!({
        "checkOnSave": true,
        "files": { "exclude": exclude },
        "check": { "command": if clippy { "clippy" } else { "check" } },
        "cargo": { "buildScripts": { "enable": true } },
        "procMacro": { "enable": true },
        "completion": { "autoimport": { "enable": true }, "postfix": { "enable": true } },
        "inlayHints": {
            "typeHints": { "enable": true },
            "parameterHints": { "enable": true },
            "chainingHints": { "enable": true }
        }
    })
}

/// The shared client capabilities, plus the status notification that tells when analysis ended.
fn capabilities() -> Value {
    let mut caps = client_capabilities();
    if let Some(map) = caps.as_object_mut() {
        map.insert("experimental".into(), json!({ "serverStatusNotification": true }));
    }
    // Renaming a file asks the server first, which renames the module and its paths.
    if let Some(workspace) = caps.get_mut("workspace").and_then(Value::as_object_mut) {
        workspace.insert("fileOperations".into(), json!({ "willRename": true, "didRename": true }));
    }
    // Without this capability the server pushes its diagnostics, which include `cargo check`'s.
    if let Some(text) = caps.get_mut("textDocument").and_then(Value::as_object_mut) {
        text.remove("diagnostic");
    }
    // The text of an item must arrive with it. Its imports stay lazy: the server only offers
    // items from other modules to clients that resolve `additionalTextEdits`.
    if let Some(properties) = caps.pointer_mut("/textDocument/completion/completionItem/resolveSupport/properties").and_then(Value::as_array_mut) {
        properties.retain(|p| p != "textEdit");
    }
    caps
}

pub fn initialize(client: &Client, root: &Path, settings: &RaSettings) -> Pending<lsp_types::InitializeResult> {
    let uri = path_to_uri(root);
    let name = root.file_name().map_or_else(|| "workspace".to_string(), |n| n.to_string_lossy().into_owned());
    client.request_raw(
        "initialize",
        json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "CrabCraft", "version": env!("CARGO_PKG_VERSION") },
            "locale": "en",
            "rootUri": uri.as_str(),
            "workspaceFolders": [{ "uri": uri.as_str(), "name": name }],
            "capabilities": capabilities(),
            "initializationOptions": config(settings)
        }),
    )
}

/// `workspace/configuration` of rust-analyzer: the settings once per requested section.
pub fn configuration_reply(params: &Value, settings: &RaSettings) -> Value {
    let count = params.get("items").and_then(Value::as_array).map_or(0, Vec::len);
    Value::Array(vec![config(settings); count])
}

/// Whether an `experimental/serverStatus` notification means analysis finished.
pub fn is_quiescent(params: &Value) -> bool {
    params.get("quiescent").and_then(Value::as_bool).unwrap_or(false)
}

/// What a gutter icon of a Rust file starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunnableKind {
    /// One test, by its name for libtest.
    Test(String),
    /// Every test of a module.
    Module(String),
    /// A binary target, by name.
    Binary(String),
    /// A benchmark: everything to pass to `cargo`.
    Bench(Vec<String>),
}

/// A place in a file that can be run, from `experimental/runnables`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runnable {
    /// 0-based line of the item's name.
    pub line: usize,
    pub kind: RunnableKind,
    /// Cargo's arguments that select the item's target, after `test` or `run`.
    pub cargo_args: Vec<String>,
}

/// The reply of `experimental/runnables`. Entries with no place in the file (the whole-package
/// `cargo check` and `cargo test`) and doctests, which libtest cannot select by these names, are left out.
pub fn parse_runnables(reply: &Value) -> Vec<Runnable> {
    reply.as_array().map(|list| list.iter().filter_map(parse_runnable).collect()).unwrap_or_default()
}

fn parse_runnable(item: &Value) -> Option<Runnable> {
    let location = item.get("location")?;
    let range = location.get("targetSelectionRange").or_else(|| location.get("targetRange"))?;
    let line = usize::try_from(range.pointer("/start/line")?.as_u64()?).ok()?;
    let args = item.get("args")?;
    let strings = |key: &str| -> Vec<&str> {
        args.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
    };
    let cargo = strings("cargoArgs");
    let executable = strings("executableArgs");
    let kind = match *cargo.first()? {
        "run" => RunnableKind::Binary((*cargo.iter().skip_while(|a| **a != "--bin").nth(1)?).to_string()),
        "bench" => {
            let mut all: Vec<String> = cargo.iter().map(|a| (*a).to_string()).collect();
            if !executable.is_empty() {
                all.push("--".to_string());
                all.extend(executable.iter().map(|a| (*a).to_string()));
            }
            RunnableKind::Bench(all)
        }
        "test" if cargo.contains(&"--doc") => return None,
        "test" => {
            let name = (*executable.first().filter(|n| !n.starts_with('-'))?).to_string();
            if executable.contains(&"--exact") { RunnableKind::Test(name) } else { RunnableKind::Module(name.trim_end_matches("::").to_string()) }
        }
        _ => return None,
    };
    Some(Runnable { line, kind, cargo_args: cargo.iter().skip(1).map(|a| (*a).to_string()).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmarks_keep_their_whole_command() {
        let list = parse_runnables(&json!([entry(1, &["bench", "--package", "p", "--bench", "b"], &[]), entry(2, &["bench", "--package", "p"], &["fast", "--exact"])]));
        assert_eq!(list[0].kind, RunnableKind::Bench(["bench", "--package", "p", "--bench", "b"].map(String::from).to_vec()));
        assert_eq!(list[1].kind, RunnableKind::Bench(["bench", "--package", "p", "--", "fast", "--exact"].map(String::from).to_vec()));
    }

    fn entry(line: u64, cargo: &[&str], executable: &[&str]) -> Value {
        json!({
            "label": "x",
            "kind": "cargo",
            "location": { "targetUri": "file:///a.rs", "targetSelectionRange": { "start": { "line": line, "character": 3 }, "end": { "line": line, "character": 6 } } },
            "args": { "cargoArgs": cargo, "executableArgs": executable }
        })
    }

    #[test]
    fn reads_tests_modules_and_binaries() {
        let reply = json!([
            entry(1, &["run", "--package", "demo", "--bin", "demo"], &[]),
            entry(2, &["test", "--package", "demo", "--lib"], &["tests::works", "--exact", "--show-output"]),
            entry(3, &["test", "--package", "demo", "--lib"], &["tests::"]),
        ]);
        let found = parse_runnables(&reply);
        let kinds: Vec<_> = found.iter().map(|r| (r.line, r.kind.clone())).collect();
        assert_eq!(kinds, vec![
            (1, RunnableKind::Binary("demo".into())),
            (2, RunnableKind::Test("tests::works".into())),
            (3, RunnableKind::Module("tests".into())),
        ]);
        assert_eq!(found[1].cargo_args, ["--package", "demo", "--lib"]);
    }

    #[test]
    fn leaves_out_what_has_no_place_or_cannot_be_selected() {
        let mut whole_package = entry(0, &["test", "--package", "demo"], &[]);
        whole_package.as_object_mut().unwrap().remove("location");
        let reply = json!([
            whole_package,
            entry(4, &["test", "--doc"], &["Greeter::hello", "--exact"]),
            entry(5, &["test", "--lib"], &[]),
            entry(6, &["check"], &[]),
            entry(7, &["run"], &[]),
            json!({ "location": 3 }),
            7,
        ]);
        assert!(parse_runnables(&reply).is_empty());
        assert!(parse_runnables(&json!(null)).is_empty());
    }

    #[test]
    fn the_configuration_answers_every_section() {
        assert_eq!(configuration_reply(&json!({ "items": [{ "section": "a" }, { "section": "b" }] }), &RaSettings::default()).as_array().map(Vec::len), Some(2));
        assert_eq!(configuration_reply(&json!({}), &RaSettings::default()).as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn the_linter_is_the_check_command() {
        assert_eq!(config(&RaSettings::default())["check"]["command"], json!("check"));
        assert_eq!(config(&RaSettings { clippy: true, ..Default::default() })["check"]["command"], json!("clippy"));
    }

    #[test]
    fn excluded_paths_are_the_files_exclude_setting() {
        assert_eq!(config(&RaSettings { exclude: vec!["gen".into(), "src/old.rs".into()], ..Default::default() })["files"]["exclude"], json!(["gen", "src/old.rs"]));
        assert_eq!(config(&RaSettings::default())["files"]["exclude"], json!([]));
        assert!(config(&RaSettings::default()).get("linkedProjects").is_none(), "the server finds the projects itself");
        let excluded = config(&RaSettings { autoimport_exclude: vec!["std::sync::atomic::Ordering".into()], ..Default::default() });
        assert_eq!(excluded["completion"]["autoimport"]["exclude"], json!([{ "path": "std::sync::atomic::Ordering", "type": "always" }]));
        assert_eq!(config(&RaSettings { linked: vec!["/w/Cargo.toml".into(), "/w/tools/x/Cargo.toml".into()], ..Default::default() })["linkedProjects"], json!(["/w/Cargo.toml", "/w/tools/x/Cargo.toml"]));
    }

    #[test]
    fn asks_for_the_status_notification() {
        assert_eq!(capabilities()["experimental"]["serverStatusNotification"], json!(true));
        assert!(capabilities()["textDocument"].get("diagnostic").is_none(), "pull diagnostics would stop the published ones");
        let lazy = &capabilities()["textDocument"]["completion"]["completionItem"]["resolveSupport"]["properties"];
        let lazy = lazy.as_array().unwrap();
        assert!(lazy.contains(&json!("additionalTextEdits")) && !lazy.contains(&json!("textEdit")), "{lazy:?}");
    }

    #[test]
    fn quiescent_needs_the_flag() {
        assert!(is_quiescent(&json!({ "quiescent": true, "health": "ok" })));
        assert!(!is_quiescent(&json!({ "quiescent": false })));
        assert!(!is_quiescent(&json!({})));
    }
}
