//! IDE-local shelves. Patches describe working-tree changes against the index, so neither
//! shelving nor restoring writes the Git index or creates Git stash refs.
use rc_core::{Inbox, Waker};
use rc_kit::i18n::tr;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const MAX_PATCH: u64 = 32 * 1024 * 1024;
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Git(rc_vcs::GitError),
    Unsupported,
    Invalid,
    Empty,
    Cancelled,
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rc_vcs::GitError> for Error {
    fn from(e: rc_vcs::GitError) -> Self {
        Self::Git(e)
    }
}
impl Error {
    fn message(&self) -> String {
        match self {
            Self::Io(e) => e.to_string(),
            Self::Git(e) => {
                let (key, args) = e.user_message();
                let args: Vec<(&str, &dyn std::fmt::Display)> = args.iter().map(|(key, value)| (*key, value as &dyn std::fmt::Display)).collect();
                rc_kit::i18n::trf(key, &args)
            }
            Self::Unsupported => tr("Shelves support tracked regular files only; untracked files, symlinks and submodules are unsupported").into(),
            Self::Invalid => tr("The shelf is invalid, changed or exceeds its safety limits").into(),
            Self::Empty => tr("No unstaged changes in the selected tracked files").into(),
            Self::Cancelled => tr("Shelf operation cancelled; saved shelves are retained").into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Shelf {
    pub id: String,
    pub name: String,
    pub created: u64,
    pub files: Vec<String>,
    root: String,
    checksum: String,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn root_name(root: &Path) -> Result<String, Error> {
    root.to_str().map(str::to_string).ok_or(Error::Unsupported)
}
fn store(root: &Path, base: &Path) -> PathBuf {
    base.join(digest(root.as_os_str().as_encoded_bytes()))
}
fn cancelled(cancel: &rc_vcs::CancelToken) -> Result<(), Error> {
    if cancel.is_cancelled() { Err(Error::Cancelled) } else { Ok(()) }
}
fn regular_read(path: &Path, limit: u64) -> Result<Vec<u8>, Error> {
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(Error::Invalid);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > limit {
        return Err(Error::Invalid);
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::Invalid);
    }
    Ok(bytes)
}
fn directory(path: &Path) -> Result<(), Error> {
    if std::fs::symlink_metadata(path)?.file_type().is_dir() { Ok(()) } else { Err(Error::Invalid) }
}
fn shelf_dir(root: &Path, base: &Path, id: &str) -> Result<PathBuf, Error> {
    if id.len() > 80 || !id.starts_with("shelf-") || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(Error::Invalid);
    }
    directory(base)?;
    let store = store(root, base);
    directory(&store)?;
    let dir = store.join(id);
    directory(&dir)?;
    Ok(dir)
}
fn load(root: &Path, base: &Path, shelf: &Shelf) -> Result<Vec<u8>, Error> {
    let dir = shelf_dir(root, base, &shelf.id)?;
    let current: Shelf = serde_json::from_slice(&regular_read(&dir.join("shelf.json"), 128 * 1024)?).map_err(|_| Error::Invalid)?;
    if current.id != shelf.id || current.checksum != shelf.checksum || current.root != root_name(root)? || current.files != shelf.files {
        return Err(Error::Invalid);
    }
    let patch = regular_read(&dir.join("changes.patch"), MAX_PATCH)?;
    if digest(&patch) != shelf.checksum {
        return Err(Error::Invalid);
    }
    Ok(patch)
}
pub fn list(root: &Path, base: &Path, cancel: &rc_vcs::CancelToken) -> Result<Vec<Shelf>, Error> {
    let store = store(root, base);
    if !store.exists() {
        return Ok(Vec::new());
    }
    directory(base)?;
    directory(&store)?;
    let mut shelves = Vec::new();
    for (n, entry) in std::fs::read_dir(&store)?.enumerate() {
        cancelled(cancel)?;
        if n >= 1000 {
            return Err(Error::Invalid);
        }
        let entry = entry?;
        if !entry.file_type()?.is_dir() || !entry.file_name().to_string_lossy().starts_with("shelf-") {
            continue;
        }
        // Incomplete temporary writes and corrupt records never become executable patches.
        let Ok(bytes) = regular_read(&entry.path().join("shelf.json"), 128 * 1024) else { continue };
        let Ok(shelf) = serde_json::from_slice::<Shelf>(&bytes) else { continue };
        if shelf.root == root_name(root)? && shelf.id == entry.file_name().to_string_lossy() && shelf.files.len() <= 1000 && shelf.name.chars().count() <= 120 {
            shelves.push(shelf);
        }
    }
    shelves.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
    Ok(shelves)
}
fn paths(root: &Path, selected: &[String]) -> Result<Vec<String>, Error> {
    if selected.is_empty() || selected.len() > 1000 {
        return Err(Error::Unsupported);
    }
    let mut result = Vec::new();
    for name in selected {
        let path = Path::new(name);
        if name.len() > 4096 || path.components().any(|c| !matches!(c, Component::Normal(_))) || path.components().any(|c| c.as_os_str() == ".git") {
            return Err(Error::Unsupported);
        }
        let mut full = root.to_path_buf();
        for component in path.components() {
            full.push(component);
            match std::fs::symlink_metadata(&full) {
                Ok(meta) if meta.file_type().is_symlink() => return Err(Error::Unsupported),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        result.push(format!(":(literal){name}"));
    }
    Ok(result)
}
async fn apply(repo: &rc_vcs::Repository, patch: Vec<u8>, reverse: bool, cancel: &rc_vcs::CancelToken) -> Result<(), Error> {
    cancelled(cancel)?;
    let command = repo.cmd().args(["apply", "--whitespace=nowarn"]).args(reverse.then_some("--reverse")).cancel(Some(cancel));
    command.clone().arg("--check").arg("-").stdin(patch.clone()).output().await?;
    cancelled(cancel)?;
    command.arg("-").stdin(patch).output().await?;
    Ok(())
}
pub async fn create(repo: &rc_vcs::Repository, base: &Path, name: &str, selected: &[String], keep: bool, cancel: &rc_vcs::CancelToken) -> Result<Shelf, Error> {
    cancelled(cancel)?;
    if name.trim().is_empty() || name.chars().count() > 120 {
        return Err(Error::Invalid);
    }
    if list(repo.root(), base, cancel)?.len() >= 500 {
        return Err(Error::Invalid);
    }
    let selected_paths = paths(repo.root(), selected)?;
    // ls-files mode prevents shelving symlink/submodule entries even when deleted on disk.
    for selected in &selected_paths {
        let out = repo.cmd().args(["ls-files", "--stage", "-z", "--"]).arg(selected).cancel(Some(cancel)).output().await?;
        let mut entries = out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty());
        let Some(entry) = entries.next() else { return Err(Error::Unsupported) };
        if entries.next().is_some()
            || !(entry.starts_with(b"100644 ") || entry.starts_with(b"100755 "))
            || !entry.split(|b| *b == b'\t').next().is_some_and(|header| header.ends_with(b" 0"))
        {
            return Err(Error::Unsupported);
        }
    }
    let patch = repo
        .cmd()
        .args(["diff", "--binary", "--full-index", "--no-renames", "--no-ext-diff", "--no-textconv", "--src-prefix=a/", "--dst-prefix=b/", "--"])
        .args(&selected_paths)
        .cancel(Some(cancel))
        .output()
        .await?
        .stdout;
    if patch.is_empty() {
        return Err(Error::Empty);
    }
    if patch.len() as u64 > MAX_PATCH {
        return Err(Error::Invalid);
    }
    cancelled(cancel)?;
    std::fs::create_dir_all(base)?;
    directory(base)?;
    let store = store(repo.root(), base);
    std::fs::create_dir_all(&store)?;
    directory(&store)?;
    let temp = tempfile::Builder::new().prefix("pending-").tempdir_in(&store)?;
    let unique = temp.path().file_name().and_then(|n| n.to_str()).ok_or(Error::Invalid)?.trim_start_matches("pending-").to_string();
    let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| Error::Invalid)?.as_secs();
    let shelf = Shelf {
        id: format!("shelf-{created}-{unique}"),
        name: name.trim().into(),
        created,
        files: selected.to_vec(),
        root: root_name(repo.root())?,
        checksum: digest(&patch),
    };
    let manifest = serde_json::to_vec(&shelf).map_err(|_| Error::Invalid)?;
    if manifest.len() > 128 * 1024 {
        return Err(Error::Invalid);
    }
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(temp.path().join("changes.patch"))?;
    file.write_all(&patch)?;
    file.sync_all()?;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(temp.path().join("shelf.json"))?;
    file.write_all(&manifest)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        std::fs::File::open(temp.path())?.sync_all()?;
    }
    cancelled(cancel)?;
    std::fs::rename(temp.path(), store.join(&shelf.id))?;
    #[cfg(unix)]
    {
        std::fs::File::open(&store)?.sync_all()?;
    }
    // The durable backup precedes cleanup. Any failed/cancelled cleanup leaves it available.
    if !keep {
        apply(repo, patch, true, cancel).await?;
    }
    Ok(shelf)
}
pub async fn restore(repo: &rc_vcs::Repository, base: &Path, shelf: &Shelf, cancel: &rc_vcs::CancelToken) -> Result<(), Error> {
    cancelled(cancel)?;
    let patch = load(repo.root(), base, shelf)?;
    paths(repo.root(), &shelf.files)?;
    let numstat = repo.cmd().args(["apply", "--numstat", "-z", "-"]).stdin(patch.clone()).cancel(Some(cancel)).output().await?;
    let mut count = 0usize;
    for record in numstat.stdout.split(|b| *b == 0).filter(|record| !record.is_empty()) {
        let path = record.splitn(3, |b| *b == b'\t').nth(2).and_then(|p| std::str::from_utf8(p).ok()).ok_or(Error::Invalid)?;
        if !shelf.files.iter().any(|file| file == path) {
            return Err(Error::Invalid);
        }
        count += 1;
    }
    if count == 0 {
        return Err(Error::Invalid);
    }
    for line in patch.split(|b| *b == b'\n') {
        if line.starts_with(b"index ")
            && let Some(mode) = line.split(|b| *b == b' ').nth(2)
            && mode != b"100644"
            && mode != b"100755"
        {
            return Err(Error::Unsupported);
        }
        for prefix in [b"new file mode ".as_slice(), b"deleted file mode ", b"old mode ", b"new mode "] {
            if let Some(mode) = line.strip_prefix(prefix)
                && mode != b"100644"
                && mode != b"100755"
            {
                return Err(Error::Unsupported);
            }
        }
    }
    apply(repo, patch, false, cancel).await
}
pub fn remove(root: &Path, base: &Path, shelf: &Shelf, cancel: &rc_vcs::CancelToken) -> Result<(), Error> {
    cancelled(cancel)?;
    let _ = load(root, base, shelf)?;
    let dir = shelf_dir(root, base, &shelf.id)?;
    std::fs::remove_file(dir.join("shelf.json"))?;
    std::fs::remove_file(dir.join("changes.patch"))?;
    std::fs::remove_dir(dir)?;
    Ok(())
}

#[derive(Clone)]
enum Work {
    List,
    Create { name: String, files: Vec<String>, keep: bool },
    Restore(Shelf),
    Remove(Shelf),
    Preview(Shelf),
}
struct Event {
    result: Result<Option<Vec<u8>>, Error>,
    shelves: Option<Vec<Shelf>>,
    changed: bool,
}
pub struct Shelves {
    root: Option<PathBuf>,
    inbox: Inbox<Event>,
    cancel: Option<rc_vcs::CancelToken>,
    pub files: Vec<String>,
    pub name: String,
    pub keep: bool,
    pub entries: Vec<Shelf>,
    selected: Option<String>,
    preview: String,
    error: Option<String>,
    delete_confirm: bool,
}
impl Shelves {
    pub fn new(waker: Waker) -> Self {
        Self {
            root: None,
            inbox: Inbox::new(waker),
            cancel: None,
            files: Vec::new(),
            name: tr("Local changes").into(),
            keep: false,
            entries: Vec::new(),
            selected: None,
            preview: String::new(),
            error: None,
            delete_confirm: false,
        }
    }
    pub fn cancel(&self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
    }
    fn start(&mut self, repo: rc_vcs::Repository, work: Work) {
        if self.cancel.is_some() {
            return;
        }
        self.error = None;
        let base = rc_core::paths::data_dir().join("shelves");
        let out = self.inbox.sender();
        let cancel = rc_vcs::CancelToken::new();
        self.cancel = Some(cancel.clone());
        rc_core::Runtime::spawn(async move {
            let changed = matches!(work, Work::Create { keep: false, .. } | Work::Restore(_));
            let result = match work {
                Work::List => Ok(None),
                Work::Create { name, files, keep } => create(&repo, &base, &name, &files, keep, &cancel).await.map(|_| None),
                Work::Restore(shelf) => restore(&repo, &base, &shelf, &cancel).await.map(|_| None),
                Work::Remove(shelf) => remove(repo.root(), &base, &shelf, &cancel).map(|_| None),
                Work::Preview(shelf) => load(repo.root(), &base, &shelf).map(Some),
            };
            // Even cancelled cleanup can have saved a durable shelf. Always reload the list
            // with a fresh token so that backup remains visible after cancellation/errors.
            let (result, shelves) = match list(repo.root(), &base, &rc_vcs::CancelToken::new()) {
                Ok(shelves) => (result, Some(shelves)),
                Err(error) => (result.and(Err(error)), None),
            };
            out.send(Event { result, shelves, changed });
        });
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for event in self.inbox.drain() {
            self.cancel = None;
            if let Some(shelves) = event.shelves {
                self.entries = shelves;
            }
            changed |= event.changed;
            match event.result {
                Ok(Some(bytes)) => {
                    let n = bytes.len().min(64 * 1024);
                    self.preview = String::from_utf8_lossy(&bytes[..n]).into_owned();
                }
                Ok(None) => {}
                Err(error) => self.error = Some(error.message()),
            }
        }
        changed
    }
    pub fn show(&mut self, ui: &mut egui::Ui, repo: rc_vcs::Repository, checked: Vec<String>, git_busy: bool, unsaved: bool) {
        egui::ScrollArea::vertical().id_salt("shelves_body").show(ui, |ui| self.contents(ui, repo, checked, git_busy, unsaved));
    }
    fn contents(&mut self, ui: &mut egui::Ui, repo: rc_vcs::Repository, checked: Vec<String>, git_busy: bool, unsaved: bool) {
        if self.root.as_deref() != Some(repo.root()) {
            self.root = Some(repo.root().to_path_buf());
            self.start(repo.clone(), Work::List);
        }
        let busy = self.cancel.is_some() || git_busy;
        ui.label(tr("IDE-local shelves keep unstaged tracked-file changes separately from Git stashes. The index is never changed."));
        ui.label(tr("Untracked files, symlinks and submodules are unsupported. Save files before shelving; restore keeps the shelf."));
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new(tr("Use Checked Files"))).clicked() {
                self.files = checked;
            }
            ui.label(format!("{}: {}", tr("Selected files"), self.files.len()));
            if ui.add_enabled(!busy, egui::Button::new(tr("Refresh Shelves"))).clicked() {
                self.start(repo.clone(), Work::List);
            }
            if self.cancel.is_some() && ui.button(tr("Cancel")).clicked() {
                self.cancel();
            }
        });
        if unsaved {
            ui.colored_label(rc_kit::pal(ui).warn, tr("Save project files and wait for formatting before shelving or restoring"));
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.name).char_limit(120).hint_text(tr("Shelf name")));
            ui.checkbox(&mut self.keep, tr("Keep changes in the working tree"));
            if ui
                .add_enabled(!busy && !unsaved && !self.files.is_empty() && !self.name.trim().is_empty(), egui::Button::new(tr("Shelve Checked Files")))
                .clicked()
            {
                self.start(repo.clone(), Work::Create { name: self.name.clone(), files: self.files.clone(), keep: self.keep });
            }
        });
        if !self.files.is_empty() {
            egui::CollapsingHeader::new(tr("Selected files")).show(ui, |ui| {
                for file in &self.files {
                    ui.label(file);
                }
            });
        }
        if let Some(error) = &self.error {
            ui.colored_label(rc_kit::pal(ui).error, error);
        }
        if self.cancel.is_some() {
            ui.spinner();
        }
        egui::ScrollArea::vertical().id_salt("shelf_list").max_height(150.0).show(ui, |ui| {
            for shelf in &self.entries {
                if ui
                    .selectable_label(self.selected.as_deref() == Some(&shelf.id), format!("{} · {} · {}", shelf.name, shelf.created, shelf.files.len()))
                    .clicked()
                {
                    self.selected = Some(shelf.id.clone());
                    self.preview.clear();
                    self.delete_confirm = false;
                }
            }
        });
        if let Some(shelf) = self.entries.iter().find(|shelf| self.selected.as_deref() == Some(&shelf.id)).cloned() {
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new(tr("Preview Shelf"))).clicked() {
                    self.start(repo.clone(), Work::Preview(shelf.clone()));
                }
                if ui.add_enabled(!busy && !unsaved, egui::Button::new(tr("Restore Shelf"))).clicked() {
                    self.start(repo.clone(), Work::Restore(shelf.clone()));
                }
                if ui.add_enabled(!busy, egui::Button::new(tr("Delete Shelf…"))).clicked() {
                    self.delete_confirm = true;
                }
            });
            if self.delete_confirm {
                ui.label(tr("Permanently delete this local shelf? Working files are unchanged."));
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new(tr("Delete Shelf"))).clicked() {
                        self.delete_confirm = false;
                        self.start(repo.clone(), Work::Remove(shelf.clone()));
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        self.delete_confirm = false;
                    }
                });
            }
            for file in shelf.files.iter().take(50) {
                ui.label(file);
            }
        }
        if !self.preview.is_empty() {
            ui.label(tr("Patch preview (first 64 KiB)"));
            egui::ScrollArea::both().id_salt("shelf_preview").max_height(220.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.preview).font(egui::TextStyle::Monospace).interactive(false).desired_width(f32::INFINITY));
            });
        }
    }
}
impl Drop for Shelves {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) -> Vec<u8> {
        let output = std::process::Command::new("git").args(args).current_dir(root).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output.stdout
    }
    fn fixture(root: &Path) {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname='shelf-proof'\nversion='0.1.0'\nedition='2021'\n[workspace]\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n").unwrap();
        std::fs::write(root.join("src/[literal].rs"), "original\n").unwrap();
        std::fs::write(root.join("data.bin"), [0u8, 1, 2, 255]).unwrap();
        git(root, &["init", "-q"]);
        git(root, &["config", "user.name", "Shelves"]);
        git(root, &["config", "user.email", "shelf@example.test"]);
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "Rust baseline"]);
    }
    #[tokio::test]
    async fn real_rust_shelves_preserve_the_index_unselected_files_binary_data_and_persistent_backups() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fixture(&root);
        let base = tempfile::tempdir().unwrap();
        let base = base.path().join("shelves");
        let repo = rc_vcs::Repository::discover(&root).await.unwrap().with_trust(true);
        let token = rc_vcs::CancelToken::new();
        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 2 }\n").unwrap();
        git(&root, &["add", "src/lib.rs"]);
        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 3 }\n").unwrap();
        std::fs::write(root.join("src/[literal].rs"), "changed\n").unwrap();
        std::fs::write(root.join("untracked.rs"), "keep me\n").unwrap();
        std::fs::write(root.join("data.bin"), [255u8, 0, 4, 128]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.join("data.bin"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let files = vec!["src/lib.rs".into(), "data.bin".into()];
        let shelf = create(&repo, &base, "Work in progress", &files, false, &token).await.unwrap();
        assert_eq!(std::fs::read_to_string(root.join("src/lib.rs")).unwrap(), "pub fn value() -> u8 { 2 }\n");
        assert_eq!(std::fs::read(root.join("data.bin")).unwrap(), [0u8, 1, 2, 255]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(root.join("data.bin")).unwrap().permissions().mode() & 0o111, 0);
        }
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(std::fs::read_to_string(root.join("src/[literal].rs")).unwrap(), "changed\n");
        assert!(root.join("untracked.rs").is_file());
        assert!(git(&root, &["stash", "list"]).is_empty());
        assert_eq!(list(&root, &base, &token).unwrap().len(), 1);
        restore(&repo, &base, &shelf, &token).await.unwrap();
        assert_eq!(std::fs::read_to_string(root.join("src/lib.rs")).unwrap(), "pub fn value() -> u8 { 3 }\n");
        assert_eq!(std::fs::read(root.join("data.bin")).unwrap(), [255u8, 0, 4, 128]);
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(list(&root, &base, &token).unwrap().len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(root.join("data.bin")).unwrap().permissions().mode() & 0o111, 0o111);
        }
        let output = rc_cargo::Cargo::locate().unwrap().std_command().args(["test", "--offline"]).current_dir(&root).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 7 }\n").unwrap();
        assert!(restore(&repo, &base, &shelf, &token).await.is_err());
        assert!(std::fs::read_to_string(root.join("src/lib.rs")).unwrap().contains("7"));
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        remove(&root, &base, &shelf, &token).unwrap();
        assert!(list(&root, &base, &token).unwrap().is_empty());
    }
    #[tokio::test]
    async fn literal_paths_deletions_keep_copy_cancellation_and_untrusted_records_are_safe() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fixture(&root);
        let base = tempfile::tempdir().unwrap();
        let base = base.path().join("shelves");
        let repo = rc_vcs::Repository::discover(&root).await.unwrap().with_trust(true);
        let token = rc_vcs::CancelToken::new();
        std::fs::remove_file(root.join("src/[literal].rs")).unwrap();
        let file = vec!["src/[literal].rs".into()];
        let shelf = create(&repo, &base, "Deleted file", &file, false, &token).await.unwrap();
        assert!(root.join("src/[literal].rs").is_file());
        restore(&repo, &base, &shelf, &token).await.unwrap();
        assert!(!root.join("src/[literal].rs").exists());
        let copy = create(&repo, &base, "Keep copy", &file, true, &token).await.unwrap();
        assert!(!root.join("src/[literal].rs").exists());
        std::fs::write(root.join("untracked.rs"), "untracked").unwrap();
        assert!(matches!(create(&repo, &base, "Unsupported", &["untracked.rs".into()], false, &token).await, Err(Error::Unsupported)));
        let cancelled = rc_vcs::CancelToken::new();
        cancelled.cancel();
        assert!(matches!(restore(&repo, &base, &copy, &cancelled).await, Err(Error::Cancelled)));
        let dir = shelf_dir(&root, &base, &shelf.id).unwrap();
        std::fs::write(dir.join("changes.patch"), "malicious patch").unwrap();
        assert!(matches!(restore(&repo, &base, &shelf, &token).await, Err(Error::Invalid)));
        assert!(!root.join("src/[literal].rs").exists());
        let malicious = b"diff --git a/src/[literal].rs b/src/[literal].rs\nnew file mode 120000\nindex 0000000..1111111\n--- /dev/null\n+++ b/src/[literal].rs\n@@ -0,0 +1 @@\n+../../outside\n";
        let mut forged = copy.clone();
        forged.checksum = digest(malicious);
        let dir = shelf_dir(&root, &base, &forged.id).unwrap();
        std::fs::write(dir.join("changes.patch"), malicious).unwrap();
        std::fs::write(dir.join("shelf.json"), serde_json::to_vec(&forged).unwrap()).unwrap();
        assert!(matches!(restore(&repo, &base, &forged, &token).await, Err(Error::Unsupported)));
        assert!(!root.join("src/[literal].rs").exists());
        assert!(matches!(paths(&root, &["../outside".into()]), Err(Error::Unsupported)));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("src"), root.join("linked")).unwrap();
            assert!(matches!(paths(&root, &["linked/lib.rs".into()]), Err(Error::Unsupported)));
        }
    }
    #[test]
    fn native_shelves_save_preview_restore_and_delete_changes_in_a_real_rust_project() {
        use egui_kittest::kittest::Queryable;
        use std::time::{Duration, Instant};
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fixture(&root);
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(&root);
        app.trust_project_tools_for_test();
        let mut h = crate::screens::harness(app, egui::vec2(1650.0, 1200.0));
        let start = Instant::now();
        while (h.state().runner.cargo_workspace.is_none() || h.state().vcs.status.is_none()) && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(h.state().runner.cargo_workspace.is_some());
        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 42 }\n").unwrap();
        h.state_mut().vcs.refresh();
        while !h.state().vcs.changes().iter().any(|change| change.path == "src/lib.rs") && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        // Only this tracked file is checked; metadata may have created an untracked lockfile.
        let excluded = h.state().vcs.changes().iter().filter(|change| change.path != "src/lib.rs").map(|change| change.path.clone()).collect();
        h.state_mut().vcs.excluded = excluded;
        h.state_mut().dock.sizes.insert(crate::toolwindow::ToolWindow::Git, 900.0);
        h.state_mut().dock.show(crate::toolwindow::ToolWindow::Commit);
        h.run_steps(3);
        h.get_by_label(tr("Shelve Checked…")).click();
        h.run_steps(3);
        assert_eq!(h.state().shelves.files, ["src/lib.rs"]);
        h.state_mut().shelves.name = "Native Rust shelf".into();
        while h.state().shelves.cancel.is_some() && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        let index = std::fs::read(root.join(".git/index")).unwrap();
        h.get_by_label(tr("Shelve Checked Files")).click();
        h.run_steps(2);
        while h.state().shelves.cancel.is_some() && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(h.state().shelves.entries.len(), 1, "{:?}", h.state().shelves.error);
        assert!(std::fs::read_to_string(root.join("src/lib.rs")).unwrap().contains("{ 1 }"));
        let shelf = h.state().shelves.entries[0].clone();
        h.get_by_label(&format!("{} · {} · {}", shelf.name, shelf.created, shelf.files.len())).click();
        h.run_steps(2);
        h.get_by_label(tr("Preview Shelf")).click();
        h.run_steps(2);
        while h.state().shelves.cancel.is_some() && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(h.state().shelves.preview.contains("+pub fn value() -> u8 { 42 }"));
        h.get_by_label(tr("Restore Shelf")).click();
        h.run_steps(2);
        while h.state().shelves.cancel.is_some() && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(std::fs::read_to_string(root.join("src/lib.rs")).unwrap().contains("{ 42 }"));
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(h.state().shelves.entries.len(), 1);
        h.get_by_label(tr("Delete Shelf…")).click();
        h.run_steps(2);
        h.get_by_label(tr("Delete Shelf")).click();
        h.run_steps(2);
        while h.state().shelves.cancel.is_some() && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            h.state().shelves.entries.is_empty(),
            "{:?}; busy{}; confirm{}",
            h.state().shelves.error,
            h.state().shelves.cancel.is_some(),
            h.state().shelves.delete_confirm
        );
        assert!(std::fs::read_to_string(root.join("src/lib.rs")).unwrap().contains("{ 42 }"));
    }
}
