//! Explicit local process extensions. The protocol limits host actions, not OS permissions.
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const TEXT: usize = 1024 * 1024;
const BINARY: usize = 32 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Trust,
    Manifest,
    UnsafePath,
    Hash,
    Exists,
    Io,
    Limit,
    Protocol,
    Cancelled,
    Stale,
}
impl Error {
    fn message(&self) -> String {
        match self {
            Self::Trust => tr("Trust the project and this local process plugin before running it"),
            Self::Manifest => tr("Invalid or unsupported CrabCraft plugin v1 manifest"),
            Self::UnsafePath => {
                tr("Plugin paths must be regular files without symlinks or traversal")
            }
            Self::Hash => tr("Plugin executable SHA-256 does not match its manifest"),
            Self::Exists => {
                tr("This plugin is already installed; remove it before installing another version")
            }
            Self::Io => tr("Could not read, install or run the local plugin"),
            Self::Limit => tr("Plugin input, output, executable or time limit exceeded"),
            Self::Protocol => tr("Plugin returned an invalid CrabCraft plugin v1 response"),
            Self::Cancelled => tr("Local plugin operation cancelled"),
            Self::Stale => tr("The active document changed; run the plugin again before applying"),
        }
        .into()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub protocol: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub executable: String,
    pub sha256: String,
    pub commands: Vec<Command>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub id: String,
    pub title: String,
}
fn ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn basename(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !matches!(s, "." | "..")
        && !s.contains(['/', '\\', ':', '\0'])
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
impl Manifest {
    fn validate(&self) -> Result<(), Error> {
        if self.protocol != 1
            || !ident(&self.id)
            || (!basename(&self.executable) || self.executable == "installed.json")
            || self.name.is_empty()
            || self.name.len() > 256
            || self.version.is_empty()
            || self.version.len() > 64
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || self.commands.is_empty()
            || self.commands.len() > 16
        {
            return Err(Error::Manifest);
        }
        let mut ids = std::collections::HashSet::new();
        for command in &self.commands {
            if !ident(&command.id)
                || !ids.insert(&command.id)
                || command.title.is_empty()
                || command.title.len() > 256
            {
                return Err(Error::Manifest);
            }
        }
        Ok(())
    }
}
// O_NONBLOCK prevents a user-selected FIFO/device from blocking a worker before type checking.
fn check(stop: Option<&rc_dotnet::CancelToken>) -> Result<(), Error> {
    if stop.is_some_and(|stop| stop.is_cancelled()) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
fn regular(path: &Path, max: usize) -> Result<Vec<u8>, Error> {
    regular_cancel(path, max, None)
}
fn regular_cancel(
    path: &Path,
    max: usize,
    stop: Option<&rc_dotnet::CancelToken>,
) -> Result<Vec<u8>, Error> {
    check(stop)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    if !std::fs::symlink_metadata(path)
        .map_err(|_| Error::Io)?
        .file_type()
        .is_file()
    {
        return Err(Error::UnsafePath);
    }
    let mut file = options.open(path).map_err(|_| Error::UnsafePath)?;
    let metadata = file.metadata().map_err(|_| Error::Io)?;
    if !metadata.is_file() {
        return Err(Error::UnsafePath);
    }
    if metadata.len() > max as u64 {
        return Err(Error::Limit);
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        check(stop)?;
        let n = file.read(&mut chunk).map_err(|_| Error::Io)?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > max {
            return Err(Error::Limit);
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    check(stop)?;
    Ok(bytes)
}
#[derive(Clone)]
struct Package {
    manifest: Manifest,
    bytes: std::sync::Arc<Vec<u8>>,
}
#[cfg(test)]
fn inspect(path: &Path) -> Result<Package, Error> {
    inspect_cancel(path, None)
}
fn inspect_cancel(path: &Path, stop: Option<&rc_dotnet::CancelToken>) -> Result<Package, Error> {
    check(stop)?;
    let parent = path.parent().ok_or(Error::UnsafePath)?;
    if !path.is_absolute() || parent.canonicalize().map_err(|_| Error::Io)? != parent {
        return Err(Error::UnsafePath);
    }
    let manifest: Manifest = serde_json::from_slice(&regular_cancel(path, 64 * 1024, stop)?)
        .map_err(|_| Error::Manifest)?;
    manifest.validate()?;
    let bytes = regular_cancel(&parent.join(&manifest.executable), BINARY, stop)?;
    if hash(&bytes) != manifest.sha256.to_ascii_lowercase() {
        return Err(Error::Hash);
    }
    Ok(Package {
        manifest,
        bytes: std::sync::Arc::new(bytes),
    })
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Installed {
    manifest: Manifest,
    enabled: bool,
}
fn private_dir(path: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(path).map_err(|_| Error::Io)?;
    if std::fs::symlink_metadata(path)
        .map_err(|_| Error::Io)?
        .file_type()
        .is_symlink()
        || path.canonicalize().map_err(|_| Error::Io)? != path
    {
        return Err(Error::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Io)?;
    }
    Ok(())
}
fn write_cancel(
    path: &Path,
    bytes: &[u8],
    executable: bool,
    stop: Option<&rc_dotnet::CancelToken>,
) -> Result<(), Error> {
    check(stop)?;
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(if executable { 0o500 } else { 0o600 })
            .custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|_| Error::Io)?;
    for chunk in bytes.chunks(65536) {
        check(stop)?;
        file.write_all(chunk).map_err(|_| Error::Io)?;
    }
    check(stop)?;
    file.sync_all().map_err(|_| Error::Io)
}
#[cfg(test)]
fn install(store: &Path, package: Package, trusted: bool) -> Result<(), Error> {
    install_cancel(store, package, trusted, None)
}
fn install_cancel(
    store: &Path,
    package: Package,
    trusted: bool,
    stop: Option<&rc_dotnet::CancelToken>,
) -> Result<(), Error> {
    check(stop)?;
    if !trusted {
        return Err(Error::Trust);
    }
    package.manifest.validate()?;
    if hash(&package.bytes) != package.manifest.sha256.to_ascii_lowercase() {
        return Err(Error::Hash);
    }
    private_dir(store)?;
    let target = store.join(&package.manifest.id);
    if target.try_exists().map_err(|_| Error::Io)? || std::fs::symlink_metadata(&target).is_ok() {
        return Err(Error::Exists);
    }
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(store)
        .map_err(|_| Error::Io)?;
    write_cancel(
        &staging.path().join(&package.manifest.executable),
        &package.bytes,
        true,
        stop,
    )?;
    let metadata = serde_json::to_vec(&Installed {
        manifest: package.manifest,
        enabled: false,
    })
    .map_err(|_| Error::Manifest)?;
    write_cancel(
        &staging.path().join("installed.json"),
        &metadata,
        false,
        stop,
    )?;
    check(stop)?;
    std::fs::rename(staging.path(), target).map_err(|_| Error::Io)?;
    Ok(())
}
fn load(store: &Path, id: &str) -> Result<Installed, Error> {
    if !ident(id) {
        return Err(Error::Manifest);
    }
    let dir = store.join(id);
    if dir.canonicalize().map_err(|_| Error::Io)? != dir {
        return Err(Error::UnsafePath);
    }
    let item: Installed = serde_json::from_slice(&regular(&dir.join("installed.json"), 64 * 1024)?)
        .map_err(|_| Error::Manifest)?;
    item.manifest.validate()?;
    if item.manifest.id != id {
        return Err(Error::Manifest);
    }
    Ok(item)
}
fn catalog(store: &Path, stop: Option<&rc_dotnet::CancelToken>) -> Result<Vec<Installed>, Error> {
    check(stop)?;
    if !store.exists() {
        return Ok(Vec::new());
    }
    if store.canonicalize().map_err(|_| Error::Io)? != store {
        return Err(Error::UnsafePath);
    }
    let mut result = Vec::new();
    for (count, entry) in std::fs::read_dir(store).map_err(|_| Error::Io)?.enumerate() {
        check(stop)?;
        if count >= 256 {
            return Err(Error::Limit);
        }
        let entry = entry.map_err(|_| Error::Io)?;
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if id.starts_with('.') {
            continue;
        }
        if result.len() >= 128 {
            return Err(Error::Limit);
        }
        result.push(load(store, &id)?);
    }
    result.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    Ok(result)
}
fn enable(store: &Path, id: &str, enabled: bool) -> Result<(), Error> {
    let mut item = load(store, id)?;
    item.enabled = enabled;
    let target = store.join(id).join("installed.json");
    let mut temp = tempfile::NamedTempFile::new_in(store.join(id)).map_err(|_| Error::Io)?;
    use std::io::Write;
    temp.write_all(&serde_json::to_vec(&item).map_err(|_| Error::Manifest)?)
        .map_err(|_| Error::Io)?;
    temp.persist(target).map_err(|_| Error::Io)?;
    Ok(())
}
fn remove(store: &Path, id: &str) -> Result<(), Error> {
    let item = load(store, id)?;
    let dir = store.join(id);
    regular(&dir.join(&item.manifest.executable), BINARY)?;
    std::fs::remove_file(dir.join(&item.manifest.executable)).map_err(|_| Error::Io)?;
    std::fs::remove_file(dir.join("installed.json")).map_err(|_| Error::Io)?;
    std::fs::remove_dir(dir).map_err(|_| Error::Io)
}
#[derive(Clone)]
struct Snapshot {
    path: PathBuf,
    text: String,
    digest: String,
}
impl Snapshot {
    fn new(path: PathBuf, text: String) -> Result<Self, Error> {
        if text.len() > TEXT {
            return Err(Error::Limit);
        }
        let digest = hash(text.as_bytes());
        Ok(Self { path, text, digest })
    }
    fn matches(&self, path: &Path, text: &str) -> bool {
        self.path == path && self.digest == hash(text.as_bytes())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    protocol: u32,
    base_sha256: String,
    replacement: String,
    message: Option<String>,
}
struct Preview {
    snapshot: Snapshot,
    reply: Reply,
}
struct Group(Option<u32>);
impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            rc_dotnet::kill_tree(pid);
        }
    }
}
async fn limited<R: tokio::io::AsyncRead + Unpin>(reader: R, max: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    reader
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| Error::Io)?;
    if bytes.len() > max {
        return Err(Error::Limit);
    }
    Ok(bytes)
}
async fn run(
    store: PathBuf,
    id: String,
    command: String,
    snapshot: Snapshot,
    trusted: bool,
    stop: rc_dotnet::CancelToken,
) -> Result<Preview, Error> {
    if !trusted {
        return Err(Error::Trust);
    }
    let item = load(&store, &id)?;
    if !item.enabled {
        return Err(Error::Trust);
    }
    if !item.manifest.commands.iter().any(|c| c.id == command) {
        return Err(Error::Manifest);
    }
    let bytes = regular_cancel(
        &store.join(&id).join(&item.manifest.executable),
        BINARY,
        Some(&stop),
    )?;
    if hash(&bytes) != item.manifest.sha256.to_ascii_lowercase() {
        return Err(Error::Hash);
    }
    if stop.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let dir = tempfile::Builder::new()
        .prefix("crabcraft-plugin-")
        .tempdir()
        .map_err(|_| Error::Io)?;
    let executable = dir.path().join(&item.manifest.executable);
    write_cancel(&executable, &bytes, true, Some(&stop))?;
    let input=serde_json::to_vec(&serde_json::json!({"protocol":1,"command":command,"document":{"path":snapshot.path,"text":snapshot.text,"sha256":snapshot.digest}})).map_err(|_|Error::Protocol)?;
    let mut cmd = tokio::process::Command::new(executable);
    cmd.current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|_| Error::Io)?;
    let group = Group(child.id());
    let mut stdin = child.stdin.take().ok_or(Error::Io)?;
    let stdout = child.stdout.take().ok_or(Error::Io)?;
    let stderr = child.stderr.take().ok_or(Error::Io)?;
    let work = async {
        let write = async move {
            stdin.write_all(&input).await.map_err(|_| Error::Io)?;
            stdin.shutdown().await.map_err(|_| Error::Io)?;
            drop(stdin);
            Ok(())
        };
        let (_, output, _) =
            tokio::try_join!(write, limited(stdout, 2 * TEXT), limited(stderr, 64 * 1024))?;
        let status = child.wait().await.map_err(|_| Error::Io)?;
        if !status.success() {
            return Err(Error::Protocol);
        }
        let reply: Reply = serde_json::from_slice(&output).map_err(|_| Error::Protocol)?;
        if reply.protocol != 1
            || reply.base_sha256 != snapshot.digest
            || reply.replacement.len() > TEXT
            || reply.message.as_ref().is_some_and(|s| s.len() > 4096)
        {
            return Err(Error::Protocol);
        }
        Ok(Preview { snapshot, reply })
    };
    let result = tokio::select! {result=tokio::time::timeout(Duration::from_secs(30),work)=>result.map_err(|_|Error::Limit)?,()=stop.cancelled()=>Err(Error::Cancelled)};
    drop(group);
    let _ = child.start_kill();
    let _ = child.wait().await;
    result
}

enum Event {
    Inspected(Result<Package, Error>),
    Changed(Result<Vec<Installed>, Error>),
    Finished(Result<Preview, Error>),
}
pub struct Plugins {
    store: PathBuf,
    inbox: Inbox<Event>,
    pub source: String,
    package: Option<Package>,
    items: Vec<Installed>,
    selected: String,
    command: String,
    busy: bool,
    running: bool,
    stop: Option<rc_dotnet::CancelToken>,
    preview: Option<Preview>,
    trust: bool,
    confirm: bool,
    error: Option<String>,
}
impl Plugins {
    pub fn new(waker: Waker) -> Self {
        Self {
            store: rc_core::paths::data_dir().join("plugins-v1"),
            inbox: Inbox::new(waker),
            source: String::new(),
            package: None,
            items: Vec::new(),
            selected: String::new(),
            command: String::new(),
            busy: false,
            running: false,
            stop: None,
            preview: None,
            trust: false,
            confirm: false,
            error: None,
        }
    }
    fn inspect(&mut self) {
        let path = PathBuf::from(&self.source);
        let stop = rc_dotnet::CancelToken::new();
        self.stop = Some(stop.clone());
        let out = self.inbox.sender();
        self.busy = true;
        self.package = None;
        self.trust = false;
        self.error = None;
        rc_core::Runtime::spawn(async move {
            let result = tokio::task::spawn_blocking(move || inspect_cancel(&path, Some(&stop)))
                .await
                .unwrap_or(Err(Error::Io));
            out.send(Event::Inspected(result));
        });
    }
    fn change(
        &mut self,
        operation: impl FnOnce(&Path, &rc_dotnet::CancelToken) -> Result<(), Error> + Send + 'static,
    ) {
        let store = self.store.clone();
        let stop = rc_dotnet::CancelToken::new();
        self.stop = Some(stop.clone());
        let out = self.inbox.sender();
        self.busy = true;
        self.error = None;
        rc_core::Runtime::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                check(Some(&stop))?;
                operation(&store, &stop).and_then(|()| catalog(&store, Some(&stop)))
            })
            .await
            .unwrap_or(Err(Error::Io));
            out.send(Event::Changed(result));
        });
    }
    fn start(&mut self, snapshot: Snapshot, trusted: bool) {
        let (store, id, command) = (
            self.store.clone(),
            self.selected.clone(),
            self.command.clone(),
        );
        let stop = rc_dotnet::CancelToken::new();
        self.stop = Some(stop.clone());
        self.busy = true;
        self.running = true;
        self.preview = None;
        self.confirm = false;
        self.error = None;
        let out = self.inbox.sender();
        rc_core::Runtime::spawn(async move {
            out.send(Event::Finished(
                run(store, id, command, snapshot, trusted, stop).await,
            ));
        });
    }
    pub fn poll(&mut self, trusted: bool) {
        if !trusted {
            if self.running
                && let Some(stop) = &self.stop
            {
                stop.cancel();
            }
            self.preview = None;
            self.confirm = false;
        }
        for event in self.inbox.drain() {
            self.busy = false;
            self.running = false;
            self.stop = None;
            let result = match event {
                Event::Inspected(result) => result.map(|package| {
                    self.package = Some(package);
                }),
                Event::Changed(result) => result.map(|items| {
                    self.items = items;
                }),
                Event::Finished(result) => {
                    self.stop = None;
                    result.and_then(|preview| {
                        if trusted {
                            self.preview = Some(preview);
                            Ok(())
                        } else {
                            Err(Error::Trust)
                        }
                    })
                }
            };
            if let Err(error) = result {
                self.error = Some(error.message());
            }
        }
    }
    fn apply(&mut self, path: &Path, text: &str, trusted: bool) -> Result<String, Error> {
        if !trusted {
            return Err(Error::Trust);
        }
        if !self.confirm {
            return Err(Error::Trust);
        }
        let preview = self.preview.as_ref().ok_or(Error::Protocol)?;
        if !preview.snapshot.matches(path, text) {
            return Err(Error::Stale);
        }
        let preview = self.preview.take().ok_or(Error::Protocol)?;
        self.confirm = false;
        Ok(preview.reply.replacement)
    }
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: Option<Snapshot>,
        trusted: bool,
    ) -> Option<String> {
        let mut apply = None;
        egui::ScrollArea::vertical().id_salt("plugins_body").show(ui,|ui|{
        ui.label(tr("Local process plugins run with your operating-system permissions. This is not an OS sandbox: only install binaries from sources you trust."));
        ui.label(tr("CrabCraft plugin v1 supports explicit active-document transformations. Marketplace, dynamic libraries, automatic activation and arbitrary host actions are unsupported."));
        ui.label(tr("Installing only copies verified files; no plugin runs on installation or project open. Installed plugins start disabled."));
        if !trusted{ui.colored_label(rc_kit::pal(ui).warn,tr("Trust this project before running project tools or code"));}
        ui.label(tr("Local plugin manifest path"));if ui.add_enabled(!self.busy,egui::TextEdit::singleline(&mut self.source).char_limit(4096)).changed(){self.package=None;self.trust=false;}
        ui.horizontal_wrapped(|ui|{if ui.add_enabled(!self.busy&&!self.source.is_empty(),egui::Button::new(tr("Inspect Plugin Package"))).clicked(){self.inspect();}if ui.add_enabled(!self.busy,egui::Button::new(tr("Refresh Installed Plugins"))).clicked(){self.change(|_,_|Ok(()));}if self.busy{ui.spinner();}if ui.add_enabled(self.stop.is_some(),egui::Button::new(tr("Cancel Plugin Operation"))).clicked(){if let Some(stop)=&self.stop{stop.cancel();}}});
        if let Some(package)=self.package.clone(){ui.separator();ui.label(format!("{} {} ({})",package.manifest.name,package.manifest.version,package.manifest.id));ui.monospace(format!("SHA-256: {}",package.manifest.sha256));for command in &package.manifest.commands{ui.label(format!("{}: {}",command.id,command.title));}ui.checkbox(&mut self.trust,tr("I trust this local executable to run with my user permissions"));if ui.add_enabled(!self.busy&&self.trust,egui::Button::new(tr("Install Verified Plugin"))).clicked(){self.package=None;self.trust=false;self.change(move|store,stop|install_cancel(store,package,true,Some(stop)));}}
        ui.separator();let selected_before=self.selected.clone();egui::ComboBox::from_id_salt("installed_plugin").selected_text(if self.selected.is_empty(){tr("Select a local plugin").to_string()}else{self.selected.clone()}).show_ui(ui,|ui|{for item in &self.items{ui.selectable_value(&mut self.selected,item.manifest.id.clone(),format!("{} {}",item.manifest.name,item.manifest.version));}});if self.selected!=selected_before{self.command.clear();self.preview=None;self.confirm=false;}
        if let Some(item)=self.items.iter().find(|item|item.manifest.id==self.selected).cloned(){ui.horizontal_wrapped(|ui|{let mut enabled=item.enabled;if ui.add_enabled(!self.busy,egui::Checkbox::new(&mut enabled,tr("Enable Trusted Plugin"))).changed(){let id=item.manifest.id.clone();self.change(move|store,_|enable(store,&id,enabled));}if ui.add_enabled(!self.busy,egui::Button::new(tr("Remove Local Plugin"))).clicked(){let id=item.manifest.id.clone();self.preview=None;self.change(move|store,_|remove(store,&id));}});egui::ComboBox::from_id_salt("plugin_command").selected_text(if self.command.is_empty(){tr("Select a plugin command").to_string()}else{self.command.clone()}).show_ui(ui,|ui|{for command in &item.manifest.commands{ui.selectable_value(&mut self.command,command.id.clone(),&command.title);}});if let Some(snapshot)=&snapshot{ui.label(trf("The plugin receives the complete unsaved text of {path}",&[("path",&snapshot.path.display())]));}if ui.add_enabled(!self.busy&&trusted&&item.enabled&&snapshot.is_some()&&!self.command.is_empty(),egui::Button::new(tr("Run Plugin on Active Document"))).clicked(){if let Some(snapshot)=snapshot.clone(){self.start(snapshot,trusted);}}}
        if let Some(preview)=&self.preview{ui.separator();ui.label(tr("Plugin edit preview: review the complete replacement before applying"));if let Some(message)=&preview.reply.message{ui.label(message);}ui.collapsing(tr("Original document text"),|ui|{let mut text=preview.snapshot.text.clone();ui.add(egui::TextEdit::multiline(&mut text).interactive(false).font(egui::TextStyle::Monospace).desired_rows(8));});ui.collapsing(tr("Proposed replacement text"),|ui|{let mut text=preview.reply.replacement.clone();ui.add(egui::TextEdit::multiline(&mut text).interactive(false).font(egui::TextStyle::Monospace).desired_rows(8));});let current=snapshot.as_ref().is_some_and(|snapshot|preview.snapshot.matches(&snapshot.path,&snapshot.text));if !current{ui.colored_label(rc_kit::pal(ui).warn,Error::Stale.message());}ui.checkbox(&mut self.confirm,tr("I reviewed this plugin replacement; apply it to the unsaved editor"));if ui.add_enabled(current&&trusted&&self.confirm,egui::Button::new(tr("Apply Plugin Replacement"))).clicked(){if let Some(snapshot)=&snapshot{match self.apply(&snapshot.path,&snapshot.text,trusted){Ok(text)=>apply=Some(text),Err(error)=>self.error=Some(error.message())}}}if ui.button(tr("Discard Plugin Preview")).clicked(){self.preview=None;self.confirm=false;}}
        if let Some(error)=&self.error{ui.colored_label(rc_kit::pal(ui).error,error);}
    });
        apply
    }
}
impl Drop for Plugins {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.cancel();
        }
    }
}
pub fn show(app: &mut crate::app::CrabCraftApp, ui: &mut egui::Ui) {
    if crate::ui::toolwindows::header(ui, tr("Local Plugins")) {
        app.toggle_tool(crate::toolwindow::ToolWindow::Plugins);
    }
    let trusted = app
        .workspace
        .as_ref()
        .is_some_and(|workspace| app.settings.trusted_projects.contains(&workspace.ws.root));
    let snapshot = app
        .docs
        .active_doc()
        .filter(|doc| !doc.generated && doc.editor.buffer.rope().len_bytes() <= TEXT)
        .and_then(|doc| Snapshot::new(doc.path.clone(), doc.editor.buffer.rope().to_string()).ok());
    if let Some(text) = app.plugins.show(ui, snapshot, trusted) {
        if let Some(doc) = app.docs.active_doc_mut() {
            let end = doc.editor.buffer.rope().len_chars();
            doc.editor.replace(0, end, &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const BEFORE: &str =
        "pub fn answer()->u32 {1}\n#[test] fn answer_is_two(){assert_eq!(answer(),2);}\n";
    const AFTER: &str =
        "pub fn answer()->u32 {2}\n#[test] fn answer_is_two(){assert_eq!(answer(),2);}\n";
    fn fixture(root: &Path) -> PathBuf {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"plugin_rust_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), BEFORE).unwrap();
        std::fs::write(
            root.join("build.rs"),
            "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}",
        )
        .unwrap();
        let package = root.join("package");
        std::fs::create_dir(&package).unwrap();
        let source = format!(
            r#"use std::io::Read;fn main(){{let mut input=String::new();std::io::stdin().read_to_string(&mut input).unwrap();std::fs::write({marker:?},"ran").unwrap();if input.contains("sleep"){{std::thread::sleep(std::time::Duration::from_secs(60));}}let digest=input.split("\"sha256\":\"").nth(1).unwrap().split('"').next().unwrap();println!("{{{{\"protocol\":1,\"base_sha256\":\"{{}}\",\"replacement\":{{}}}}}}",digest,{replacement:?});}}"#,
            marker = root.join("plugin-ran"),
            replacement = serde_json::to_string(AFTER).unwrap()
        );
        std::fs::write(package.join("plugin.rs"), source).unwrap();
        let executable = package.join(if cfg!(windows) {
            "transform.exe"
        } else {
            "transform"
        });
        let output = std::process::Command::new("rustc")
            .arg(package.join("plugin.rs"))
            .arg("-o")
            .arg(&executable)
            .current_dir(std::env::temp_dir())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let manifest = Manifest {
            protocol: 1,
            id: "rust-transform".into(),
            name: "Rust transformation fixture".into(),
            version: "1.0.0".into(),
            executable: executable.file_name().unwrap().to_string_lossy().into(),
            sha256: hash(&std::fs::read(executable).unwrap()),
            commands: vec![
                Command {
                    id: "replace".into(),
                    title: "Replace Rust answer".into(),
                },
                Command {
                    id: "sleep".into(),
                    title: "Cancellation fixture".into(),
                },
            ],
        };
        let path = package.join("plugin.json");
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        path
    }
    #[tokio::test]
    async fn verified_packages_require_explicit_enable_trust_and_return_only_reviewable_edits() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let path = fixture(&root);
        let store = root.join("plugins");
        let package = inspect(&path).unwrap();
        assert!(!root.join("plugin-ran").exists());
        assert_eq!(install(&store, package.clone(), false), Err(Error::Trust));
        install(&store, package, true).unwrap();
        assert!(!root.join("plugin-ran").exists());
        let snap = Snapshot::new(root.join("src/lib.rs"), BEFORE.into()).unwrap();
        assert!(matches!(
            run(
                store.clone(),
                "rust-transform".into(),
                "replace".into(),
                snap.clone(),
                true,
                rc_dotnet::CancelToken::new()
            )
            .await,
            Err(Error::Trust)
        ));
        enable(&store, "rust-transform", true).unwrap();
        assert!(matches!(
            run(
                store.clone(),
                "rust-transform".into(),
                "replace".into(),
                snap.clone(),
                false,
                rc_dotnet::CancelToken::new()
            )
            .await,
            Err(Error::Trust)
        ));
        assert!(!root.join("plugin-ran").exists());
        let preview = run(
            store.clone(),
            "rust-transform".into(),
            "replace".into(),
            snap,
            true,
            rc_dotnet::CancelToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(preview.reply.replacement, AFTER);
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            BEFORE
        );
        assert!(!root.join("build-ran").exists());
        let executable = store.join("rust-transform").join(if cfg!(windows) {
            "transform.exe"
        } else {
            "transform"
        });
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::write(executable, "tamper").unwrap();
        assert!(matches!(
            run(
                store,
                "rust-transform".into(),
                "replace".into(),
                preview.snapshot,
                true,
                rc_dotnet::CancelToken::new()
            )
            .await,
            Err(Error::Hash)
        ));
    }
    #[test]
    fn manifest_rejects_traversal_symlinks_reserved_names_and_modified_executables() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let path = fixture(&root);
        let package = inspect(&path).unwrap();
        let stop = rc_dotnet::CancelToken::new();
        stop.cancel();
        assert!(matches!(
            inspect_cancel(&path, Some(&stop)),
            Err(Error::Cancelled)
        ));
        assert_eq!(
            install_cancel(
                &root.join("cancelled-store"),
                package.clone(),
                true,
                Some(&stop)
            ),
            Err(Error::Cancelled)
        );
        assert!(!root.join("cancelled-store").exists());
        assert_eq!(
            write_cancel(&root.join("cancelled-copy"), b"data", false, Some(&stop)),
            Err(Error::Cancelled)
        );
        assert!(!root.join("cancelled-copy").exists());
        for name in [
            "../transform",
            "/tmp/transform",
            "a/b",
            "a\\b",
            "installed.json",
        ] {
            let mut bad = package.manifest.clone();
            bad.executable = name.into();
            assert_eq!(bad.validate(), Err(Error::Manifest));
        }
        let mut bad = package.manifest.clone();
        bad.commands.push(bad.commands[0].clone());
        assert_eq!(bad.validate(), Err(Error::Manifest));
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let executable = path.parent().unwrap().join(&package.manifest.executable);
            std::fs::rename(&executable, executable.with_extension("real")).unwrap();
            symlink(executable.with_extension("real"), &executable).unwrap();
            assert!(matches!(inspect(&path), Err(Error::UnsafePath)));
        }
    }
    #[tokio::test]
    async fn cancellation_stops_a_running_rust_plugin_without_an_edit() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let path = fixture(&root);
        let store = root.join("plugins");
        install(&store, inspect(&path).unwrap(), true).unwrap();
        enable(&store, "rust-transform", true).unwrap();
        let stop = rc_dotnet::CancelToken::new();
        let task = tokio::spawn(run(
            store,
            "rust-transform".into(),
            "sleep".into(),
            Snapshot::new(root.join("src/lib.rs"), BEFORE.into()).unwrap(),
            true,
            stop.clone(),
        ));
        for _ in 0..200 {
            if root.join("plugin-ran").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(root.join("plugin-ran").exists());
        stop.cancel();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .unwrap()
                .unwrap(),
            Err(Error::Cancelled)
        ));
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            BEFORE
        );
    }
    #[test]
    fn native_plugin_preview_applies_to_real_rust_with_stale_guard_and_undo() {
        use egui_kittest::kittest::Queryable;
        let _lock = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let path = fixture(&root);
        let file = root.join("src/lib.rs");
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace(&root);
        app.open_file(&file);
        app.plugins.source = path.to_string_lossy().into();
        app.dock
            .sizes
            .insert(crate::toolwindow::ToolWindow::Plugins, 850.0);
        let mut h = crate::screens::harness(app, egui::vec2(1900.0, 1500.0));
        h.get_by_label(tr("Local Plugins")).click();
        h.run_steps(2);
        h.get_by_label(tr("Inspect Plugin Package")).click();
        h.run_steps(2);
        let wait = |h: &mut egui_kittest::Harness<'_, crate::app::CrabCraftApp>| {
            let start = std::time::Instant::now();
            while h.state().plugins.busy && start.elapsed() < Duration::from_secs(15) {
                h.run_steps(2);
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(!h.state().plugins.busy, "{:?}", h.state().plugins.error);
            assert!(
                h.state().plugins.error.is_none(),
                "{:?}",
                h.state().plugins.error
            );
        };
        wait(&mut h);
        assert!(!root.join("plugin-ran").exists());
        h.get_by_label(tr(
            "I trust this local executable to run with my user permissions",
        ))
        .click();
        h.run_steps(2);
        h.get_by_label(tr("Install Verified Plugin")).click();
        h.run_steps(2);
        wait(&mut h);
        assert!(!root.join("plugin-ran").exists());
        h.state_mut().plugins.selected = "rust-transform".into();
        h.run_steps(2);
        h.get_by_label(tr("Enable Trusted Plugin")).click();
        h.run_steps(2);
        wait(&mut h);
        h.state_mut().plugins.command = "replace".into();
        h.run_steps(2);
        h.get_by_label(tr("Run Plugin on Active Document")).click();
        h.run_steps(2);
        assert!(!root.join("plugin-ran").exists());
        h.state_mut().trust_project_tools_for_test();
        h.run_steps(2);
        assert!(!root.join("build-ran").exists());
        h.get_by_label(tr("Run Plugin on Active Document")).click();
        h.run_steps(2);
        wait(&mut h);
        assert!(h.state().plugins.preview.is_some());
        assert_eq!(
            h.state()
                .docs
                .active_doc()
                .unwrap()
                .editor
                .buffer
                .rope()
                .to_string(),
            BEFORE
        );
        h.state_mut().plugins.confirm = true;
        assert_eq!(
            h.state_mut().plugins.apply(&file, "changed", true),
            Err(Error::Stale)
        );
        h.state_mut().plugins.confirm = false;
        h.get_by_label(tr(
            "I reviewed this plugin replacement; apply it to the unsaved editor",
        ))
        .click();
        h.run_steps(2);
        h.get_by_label(tr("Apply Plugin Replacement")).click();
        h.run_steps(2);
        assert_eq!(
            h.state()
                .docs
                .active_doc()
                .unwrap()
                .editor
                .buffer
                .rope()
                .to_string(),
            AFTER
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), BEFORE);
        h.state_mut().docs.active_doc_mut().unwrap().editor.undo();
        assert_eq!(
            h.state()
                .docs
                .active_doc()
                .unwrap()
                .editor
                .buffer
                .rope()
                .to_string(),
            BEFORE
        );
        std::fs::write(&file, AFTER).unwrap();
        let output = std::process::Command::new("cargo")
            .args(["test", "--offline"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(root.join("build-ran").exists());
    }
}
