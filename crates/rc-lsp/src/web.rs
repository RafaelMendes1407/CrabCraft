//! Explicitly configured external web servers. This adapter never installs packages or
//! executes npm/project scripts. Callers must enforce the workspace trust boundary.
use crate::{Client, Pending, rpc::Incoming};
use rc_core::inbox::Outbox;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Html,
    Css,
    TypeScript,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub kind: Kind,
    pub program: PathBuf,
    pub tsserver: Option<PathBuf>,
}
impl Settings {
    pub fn validate(&self, root: &Path) -> std::io::Result<()> {
        if self.program.as_os_str().len() > 8192
            || self
                .tsserver
                .as_ref()
                .is_some_and(|path| path.as_os_str().len() > 8192)
            || !self.program.is_absolute()
            || self.program.starts_with(root)
            || self
                .tsserver
                .as_ref()
                .is_some_and(|path| !path.is_absolute() || path.starts_with(root))
            || self.kind == Kind::TypeScript && self.tsserver.is_none()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Web language servers must be explicitly configured outside the workspace",
            ));
        }
        Ok(())
    }
    pub fn initialization(&self) -> Value {
        match self.kind {
            Kind::Html => {
                json!({"embeddedLanguages":{"css":true,"javascript":true},"provideFormatter":true})
            }
            Kind::Css => json!({"provideFormatter":true}),
            Kind::TypeScript => {
                json!({"tsserver":{"path":self.tsserver},"disableAutomaticTypingAcquisition":true,"preferences":{"includeCompletionsForModuleExports":false}})
            }
        }
    }
}
pub fn start(
    settings: &Settings,
    root: &Path,
    incoming: Outbox<Incoming>,
) -> std::io::Result<Client> {
    settings.validate(root)?;
    Client::spawn(
        &settings.program,
        &["--stdio".into()],
        root,
        &[
            ("NODE_OPTIONS".into(), String::new()),
            ("NODE_PATH".into(), String::new()),
        ],
        incoming,
    )
}
pub fn initialize(
    client: &Client,
    root: &Path,
    settings: &Settings,
) -> Pending<lsp_types::InitializeResult> {
    let uri = crate::path_to_uri(root);
    let mut capabilities = crate::capabilities::client_capabilities();
    if let Some(text) = capabilities
        .get_mut("textDocument")
        .and_then(Value::as_object_mut)
    {
        text.remove("diagnostic");
    }
    client.request_raw("initialize",json!({"processId":std::process::id(),"clientInfo":{"name":"CrabCraft","version":env!("CARGO_PKG_VERSION")},"rootUri":uri.as_str(),"workspaceFolders":[{"uri":uri.as_str(),"name":"web"}],"capabilities":capabilities,"initializationOptions":settings.initialization()}))
}
pub fn configuration(params: &Value) -> Value {
    Value::Array(
        params
            .get("items")
            .and_then(Value::as_array)
            .map(|items| items.iter().map(|_| json!({})).collect())
            .unwrap_or_default(),
    )
}
