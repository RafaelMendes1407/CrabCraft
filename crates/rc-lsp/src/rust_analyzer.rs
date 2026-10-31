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

/// Starts the server; must run inside the tokio runtime.
pub fn start(program: &Path, workspace: &Path, incoming: Outbox<Incoming>) -> std::io::Result<Client> {
    Client::spawn(program, &[], workspace, &[], incoming)
}

/// The settings sent at start and when the server asks for them.
pub fn config() -> Value {
    json!({
        "checkOnSave": true,
        "check": { "command": "check" },
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
    // Without this capability the server pushes its diagnostics, which include `cargo check`'s.
    if let Some(text) = caps.get_mut("textDocument").and_then(Value::as_object_mut) {
        text.remove("diagnostic");
    }
    caps
}

pub fn initialize(client: &Client, root: &Path) -> Pending<lsp_types::InitializeResult> {
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
            "initializationOptions": config()
        }),
    )
}

/// `workspace/configuration` of rust-analyzer: the settings once per requested section.
pub fn configuration_reply(params: &Value) -> Value {
    let count = params.get("items").and_then(Value::as_array).map_or(0, Vec::len);
    Value::Array(vec![config(); count])
}

/// Whether an `experimental/serverStatus` notification means analysis finished.
pub fn is_quiescent(params: &Value) -> bool {
    params.get("quiescent").and_then(Value::as_bool).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configuration_answers_every_section() {
        assert_eq!(configuration_reply(&json!({ "items": [{ "section": "a" }, { "section": "b" }] })).as_array().map(Vec::len), Some(2));
        assert_eq!(configuration_reply(&json!({})).as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn asks_for_the_status_notification() {
        assert_eq!(capabilities()["experimental"]["serverStatusNotification"], json!(true));
        assert!(capabilities()["textDocument"].get("diagnostic").is_none(), "pull diagnostics would stop the published ones");
    }

    #[test]
    fn quiescent_needs_the_flag() {
        assert!(is_quiescent(&json!({ "quiescent": true, "health": "ok" })));
        assert!(!is_quiescent(&json!({ "quiescent": false })));
        assert!(!is_quiescent(&json!({})));
    }
}
