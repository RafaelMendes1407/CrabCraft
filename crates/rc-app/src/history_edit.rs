//! Guarded native plans for a bounded, unpublished linear suffix of the current branch.
use rc_core::{Inbox, Waker};
use rc_kit::i18n::tr;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Action {
    Pick,
    Reword,
    Drop,
    Fixup,
}
impl Action {
    fn title(self) -> &'static str {
        match self {
            Self::Pick => tr("Pick"),
            Self::Reword => tr("Reword"),
            Self::Drop => tr("Drop commit"),
            Self::Fixup => tr("Fixup into previous"),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Row {
    pub hash: String,
    pub subject: String,
    pub message: String,
    pub action: Action,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub head: String,
    pub branch: String,
    pub base: String,
    pub rows: Vec<Row>,
    root: PathBuf,
    git_dir: PathBuf,
    git_executable: PathBuf,
}
#[derive(Debug)]
pub enum Error {
    Git(rc_vcs::GitError),
    Io(std::io::Error),
    Invalid,
    Unsupported,
    Dirty,
    Published,
    Stale,
    Cancelled,
    Failed(String),
}
impl From<rc_vcs::GitError> for Error {
    fn from(e: rc_vcs::GitError) -> Self {
        Self::Git(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl Error {
    fn message(&self) -> String {
        match self {
            Self::Git(e) => {
                let (key, args) = e.user_message();
                let args: Vec<(&str, &dyn std::fmt::Display)> = args.iter().map(|(key, value)| (*key, value as &dyn std::fmt::Display)).collect();
                rc_kit::i18n::trf(key, &args)
            }
            Self::Io(e) => e.to_string(),
            Self::Failed(text) => text.clone(),
            Self::Invalid => tr("The history plan or private editor data is invalid").into(),
            Self::Unsupported => tr(
                "History editing supports 1–20 non-root linear commits on a local branch; merge commits and custom Git environment overrides are unsupported",
            )
            .into(),
            Self::Dirty => tr("Save files, clean the working tree and index, and finish existing Git operations before editing history").into(),
            Self::Published => tr("The selected history is reachable from a known remote-tracking branch; rewriting it is refused").into(),
            Self::Stale => tr("The branch or history changed; load a fresh history plan").into(),
            Self::Cancelled => tr("History editing cancelled; inspect the rebase state and recovery ref before continuing").into(),
        }
    }
}
#[derive(Clone, Default)]
pub struct Cancel {
    git: rc_vcs::CancelToken,
    process: rc_dotnet::CancelToken,
}
impl Cancel {
    pub fn cancel(&self) {
        self.git.cancel();
        self.process.cancel();
    }
    fn check(&self) -> Result<(), Error> {
        if self.process.is_cancelled() { Err(Error::Cancelled) } else { Ok(()) }
    }
}
fn hex(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn environment() -> Result<(), Error> {
    if ["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_CONFIG_COUNT"]
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty() && v != "0"))
    {
        return Err(Error::Unsupported);
    }
    Ok(())
}
async fn read(repo: &rc_vcs::Repository, args: &[&str], cancel: &Cancel) -> Result<String, Error> {
    cancel.check()?;
    let safe = repo.clone().with_trust(false);
    Ok(safe.cmd().args(args).cancel(Some(&cancel.git)).text().await?)
}
pub async fn preview(repo: &rc_vcs::Repository, count: usize, cancel: &Cancel) -> Result<Plan, Error> {
    environment()?;
    cancel.check()?;
    if !(1..=20).contains(&count) {
        return Err(Error::Unsupported);
    }
    if !read(repo, &["status", "--porcelain=v1", "-z", "--untracked-files=normal"], cancel).await?.is_empty() || repo.operation().await.state.is_some() {
        return Err(Error::Dirty);
    }
    let branch = read(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"], cancel).await?.trim().to_string();
    if branch.is_empty() || branch.contains(['\n', '\r']) {
        return Err(Error::Unsupported);
    }
    let safe = repo.clone().with_trust(false);
    let options = rc_vcs::LogOptions { max_count: count, ..Default::default() };
    let commits = tokio::select! { result = safe.log(&options) => result?, () = cancel.process.cancelled() => return Err(Error::Cancelled) };
    if commits.len() != count
        || commits.iter().any(|c| c.parents.len() != 1 || !hex(&c.hash) || !hex(&c.parents[0]) || c.subject.len() + c.body.len() > 4096)
        || commits.windows(2).any(|pair| pair[0].parents[0] != pair[1].hash)
    {
        return Err(Error::Unsupported);
    }
    let oldest = commits.last().ok_or(Error::Invalid)?;
    let published = read(repo, &["for-each-ref", "--format=%(refname)", &format!("--contains={}", oldest.hash), "refs/remotes"], cancel).await?;
    if !published.trim().is_empty() {
        return Err(Error::Published);
    }
    Ok(Plan {
        head: commits[0].hash.clone(),
        branch,
        base: oldest.parents[0].clone(),
        root: repo.root().to_path_buf(),
        git_dir: repo.git_dir().to_path_buf(),
        git_executable: repo.git().path().to_path_buf(),
        rows: commits
            .into_iter()
            .rev()
            .map(|c| Row {
                hash: c.hash,
                subject: c.subject.clone(),
                message: if c.body.is_empty() { c.subject } else { format!("{}\n\n{}", c.subject, c.body) },
                action: Action::Pick,
            })
            .collect(),
    })
}
fn validate(plan: &Plan) -> Result<(), Error> {
    if plan.rows.is_empty() || plan.rows.len() > 20 || !hex(&plan.head) || !hex(&plan.base) {
        return Err(Error::Invalid);
    }
    let mut hashes = std::collections::HashSet::new();
    let mut previous = false;
    for row in &plan.rows {
        if !hex(&row.hash)
            || !hashes.insert(&row.hash)
            || row.message.len() > 4096
            || row.message.contains('\0')
            || (row.action == Action::Reword && row.message.trim().is_empty())
        {
            return Err(Error::Invalid);
        }
        match row.action {
            Action::Drop => {}
            Action::Fixup if !previous => return Err(Error::Invalid),
            _ => previous = true,
        }
    }
    if !previous {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn quote(value: &Path) -> Result<String, Error> {
    let value = value.to_str().ok_or(Error::Unsupported)?;
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}
fn todo(plan: &Plan, executable: &Path, path: &Path) -> Result<String, Error> {
    validate(plan)?;
    let mut text = String::new();
    for row in &plan.rows {
        let command = match row.action {
            Action::Pick | Action::Reword => "pick",
            Action::Drop => "drop",
            Action::Fixup => "fixup",
        };
        text.push_str(&format!("{command} {}\n", row.hash));
        if row.action == Action::Reword {
            text.push_str(&format!("exec {} --internal-history-helper amend {} {}\n", quote(executable)?, quote(path)?, row.hash));
        }
    }
    Ok(text)
}
fn read_private(path: &Path) -> Result<Vec<u8>, Error> {
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(Error::Invalid);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > 128 * 1024 {
        return Err(Error::Invalid);
    }
    let mut bytes = Vec::new();
    file.take(128 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 128 * 1024 {
        return Err(Error::Invalid);
    }
    Ok(bytes)
}
fn plan_file(path: &Path) -> Result<Plan, Error> {
    let plan: Plan = serde_json::from_slice(&read_private(path)?).map_err(|_| Error::Invalid)?;
    validate(&plan)?;
    Ok(plan)
}
fn write_existing(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(Error::Invalid);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::Invalid);
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn internal_run(args: &[std::ffi::OsString]) -> Result<(), Error> {
    if args.len() != 3 {
        return Err(Error::Invalid);
    }
    let mode = args[0].to_str().ok_or(Error::Invalid)?;
    let path = Path::new(&args[1]);
    let plan = plan_file(path)?;
    if !rc_core::Settings::load().trusted_projects.contains(&plan.root) {
        return Err(Error::Git(rc_vcs::GitError::TrustRequired));
    }
    if plan.root.canonicalize()? != std::env::current_dir()?.canonicalize()?
        || path.parent().and_then(Path::parent).and_then(Path::parent) != Some(plan.git_dir.as_path())
        || !plan.git_dir.join("rebase-merge").is_dir()
    {
        return Err(Error::Invalid);
    }
    if mode == "sequence" {
        let target = Path::new(&args[2]);
        if target.canonicalize()? != plan.git_dir.join("rebase-merge/git-rebase-todo").canonicalize()? {
            return Err(Error::Invalid);
        }
        write_existing(target, todo(&plan, &std::env::current_exe()?, path)?.as_bytes())
    } else if mode == "amend" {
        let hash = args[2].to_str().ok_or(Error::Invalid)?;
        let row = plan.rows.iter().find(|row| row.hash == hash && row.action == Action::Reword).ok_or(Error::Invalid)?;
        let done = String::from_utf8(read_private(&plan.git_dir.join("rebase-merge/done"))?).map_err(|_| Error::Invalid)?;
        if !done.lines().last().is_some_and(|line| line.starts_with("exec ") && line.ends_with(hash)) {
            return Err(Error::Invalid);
        }
        let mut message = tempfile::NamedTempFile::new_in(path.parent().ok_or(Error::Invalid)?)?;
        message.write_all(row.message.as_bytes())?;
        message.flush()?;
        let output = std::process::Command::new(&plan.git_executable)
            .current_dir(&plan.root)
            .args(["-c", "commit.gpgSign=false", "commit", "--amend", "--no-edit", "--no-gpg-sign", "-F"])
            .arg(message.path())
            .env("GIT_EDITOR", ":")
            .stdin(std::process::Stdio::null())
            .output()?;
        if output.status.success() { Ok(()) } else { Err(Error::Failed(String::from_utf8_lossy(&output.stderr).chars().take(8192).collect())) }
    } else {
        Err(Error::Invalid)
    }
}
/// Private child-process mode, handled before GUI setup. No project-open path invokes it.
pub fn internal() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--internal-history-helper")) {
        return None;
    }
    Some(match internal_run(&args.collect::<Vec<_>>()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{}", error.message());
            1
        }
    })
}
async fn process(
    repo: &rc_vcs::Repository,
    args: &[String],
    editor: Option<String>,
    cancel: &Cancel,
    output: &mut (dyn FnMut(String) + Send),
) -> Result<(), Error> {
    cancel.check()?;
    let mut command = tokio::process::Command::new(repo.git().path());
    command.current_dir(repo.root()).args(args).env("GIT_EDITOR", ":").env("GIT_TERMINAL_PROMPT", "0").env("LC_ALL", "C").env("GIT_CONFIG_COUNT", "0");
    if let Some(editor) = editor {
        command.env("GIT_SEQUENCE_EDITOR", editor);
    }
    let mut text = String::new();
    let limit_cancel = cancel.process.clone();
    let result = rc_dotnet::process::run_streaming(
        command,
        &cancel.process,
        rc_dotnet::process::StreamOpts { partial_lines: true, ..Default::default() },
        &mut |_| {},
        &mut |line| {
            if text.len() < 64 * 1024 {
                let chunk: String = line.text.chars().take(2048).collect();
                text.push_str(&chunk);
                text.push('\n');
                output(chunk);
            } else {
                limit_cancel.cancel();
            }
        },
    )
    .await?;
    if result.cancelled {
        return Err(Error::Cancelled);
    }
    if result.code != Some(0) {
        return Err(Error::Failed(text));
    }
    Ok(())
}
pub async fn rewrite(
    repo: &rc_vcs::Repository,
    plan: &Plan,
    executable: &Path,
    cancel: &Cancel,
    backup: &mut (dyn FnMut(String) + Send),
    output: &mut (dyn FnMut(String) + Send),
) -> Result<(), Error> {
    if !repo.is_trusted() {
        return Err(Error::Git(rc_vcs::GitError::TrustRequired));
    }
    validate(plan)?;
    if read(repo, &["rev-parse", "--verify", "HEAD"], cancel).await?.trim() != plan.head {
        return Err(Error::Stale);
    }
    let current = preview(repo, plan.rows.len(), cancel).await?;
    let expected: std::collections::HashSet<_> = plan.rows.iter().map(|row| &row.hash).collect();
    let actual: std::collections::HashSet<_> = current.rows.iter().map(|row| &row.hash).collect();
    if current.head != plan.head
        || current.branch != plan.branch
        || current.base != plan.base
        || expected != actual
        || current.root != plan.root
        || current.git_dir != plan.git_dir
    {
        return Err(Error::Stale);
    }
    // This trusted mutation also proves the caller passed an explicitly trusted repository.
    let parent = repo.git_dir().join("crabcraft-history");
    std::fs::create_dir_all(&parent)?;
    if !std::fs::symlink_metadata(&parent)?.file_type().is_dir() {
        return Err(Error::Invalid);
    }
    let temp = tempfile::Builder::new().prefix("edit-").tempdir_in(&parent)?;
    let path = temp.path().join("plan.json");
    let bytes = serde_json::to_vec(plan).map_err(|_| Error::Invalid)?;
    if bytes.len() > 128 * 1024 {
        return Err(Error::Invalid);
    }
    std::fs::write(&path, bytes)?;
    let recovery = format!("refs/crabcraft/history/{}", temp.path().file_name().and_then(|p| p.to_str()).ok_or(Error::Invalid)?);
    let _kept = temp.keep();
    backup(recovery.clone());
    process(repo, &["update-ref".into(), recovery, plan.head.clone(), "0".repeat(plan.head.len())], None, cancel, output).await?;
    cancel.check()?;
    let editor = format!("{} --internal-history-helper sequence {}", quote(executable)?, quote(&path)?);
    let args = [
        "-c",
        "rebase.updateRefs=false",
        "-c",
        "rebase.autoStash=false",
        "-c",
        "rebase.autoSquash=false",
        "-c",
        "commit.gpgSign=false",
        "rebase",
        "--interactive",
        "--force-rebase",
        "--no-autostash",
        "--no-autosquash",
        "--no-rebase-merges",
        "--reschedule-failed-exec",
        &plan.base,
    ]
    .map(str::to_string);
    process(repo, &args, Some(editor), cancel, output).await
}

enum Event {
    Loaded(Result<Plan, Error>),
    Backup(String),
    Output(String),
    Done(Result<(), Error>),
}
pub struct HistoryEdit {
    #[cfg(test)]
    helper_executable: Option<PathBuf>,
    inbox: Inbox<Event>,
    cancel: Option<Cancel>,
    pub count: usize,
    pub plan: Option<Plan>,
    pub recovery: Option<String>,
    pub confirmed: bool,
    selected: usize,
    error: Option<String>,
    output: String,
}
impl HistoryEdit {
    pub fn new(waker: Waker) -> Self {
        Self {
            #[cfg(test)]
            helper_executable: None,
            inbox: Inbox::new(waker),
            cancel: None,
            count: 3,
            plan: None,
            recovery: None,
            confirmed: false,
            selected: 0,
            error: None,
            output: String::new(),
        }
    }
    pub fn cancel(&self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
    }
    pub fn poll(&mut self) -> bool {
        let mut refresh = false;
        for event in self.inbox.drain() {
            match event {
                Event::Loaded(result) => {
                    self.cancel = None;
                    match result {
                        Ok(plan) => {
                            self.plan = Some(plan);
                            self.selected = 0;
                            self.confirmed = false;
                        }
                        Err(e) => self.error = Some(e.message()),
                    }
                }
                Event::Backup(reference) => self.recovery = Some(reference),
                Event::Output(line) => {
                    if self.output.len() < 64 * 1024 {
                        self.output.push_str(&line);
                        self.output.push('\n');
                    }
                }
                Event::Done(result) => {
                    self.cancel = None;
                    self.confirmed = false;
                    self.plan = None;
                    refresh = true;
                    if let Err(e) = result {
                        self.error = Some(e.message());
                    }
                }
            }
        }
        refresh
    }
    fn load(&mut self, repo: rc_vcs::Repository) {
        if self.cancel.is_some() {
            return;
        }
        let cancel = Cancel::default();
        self.cancel = Some(cancel.clone());
        self.error = None;
        self.plan = None;
        let out = self.inbox.sender();
        let count = self.count;
        rc_core::Runtime::spawn(async move {
            out.send(Event::Loaded(preview(&repo, count, &cancel).await));
        });
    }
    fn apply(&mut self, repo: rc_vcs::Repository) {
        if self.cancel.is_some() || !self.confirmed {
            return;
        }
        let Some(plan) = self.plan.clone() else {
            return;
        };
        let selected_executable = {
            #[cfg(test)]
            {
                self.helper_executable.clone().map(Ok).unwrap_or_else(std::env::current_exe)
            }
            #[cfg(not(test))]
            {
                std::env::current_exe()
            }
        };
        let executable = match selected_executable {
            Ok(path) => path,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let cancel = Cancel::default();
        self.cancel = Some(cancel.clone());
        self.confirmed = false;
        self.output.clear();
        self.error = None;
        let out = self.inbox.sender();
        rc_core::Runtime::spawn(async move {
            let backup_out = out.clone();
            let line_out = out.clone();
            let result = rewrite(
                &repo,
                &plan,
                &executable,
                &cancel,
                &mut |reference| {
                    backup_out.send(Event::Backup(reference));
                },
                &mut |line| {
                    line_out.send(Event::Output(line));
                },
            )
            .await;
            out.send(Event::Done(result));
        });
    }
    pub fn show(&mut self, ui: &mut egui::Ui, repo: rc_vcs::Repository, busy: bool, unsaved: bool) {
        let running = self.cancel.is_some();
        let free = !running && !busy && !unsaved;
        egui::ScrollArea::vertical().id_salt("history_edit").show(ui, |ui| {
            ui.label(tr(
                "Edit only a clean, unpublished linear suffix of the current branch. Known remote-tracking history, merges and root commits are refused.",
            ));
            ui.label(tr(
                "Oldest first. Reordering, dropping or folding commits may conflict. Rewritten commits lose their original signatures; a recovery ref is kept.",
            ));
            if unsaved {
                ui.colored_label(rc_kit::pal(ui).warn, tr("Save project files and wait for formatting before editing history"));
            }
            ui.horizontal(|ui| {
                ui.label(tr("Recent commits"));
                if ui.add_enabled(!running, egui::DragValue::new(&mut self.count).range(1..=20)).changed() {
                    self.plan = None;
                    self.confirmed = false;
                }
                if ui.add_enabled(free, egui::Button::new(tr("Load History Plan"))).clicked() {
                    self.load(repo.clone());
                }
                if running && ui.button(tr("Cancel")).clicked() {
                    self.cancel();
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(rc_kit::pal(ui).error, error);
            }
            if let Some(reference) = &self.recovery {
                ui.label(format!("{}: {reference}", tr("Recovery ref")));
            }
            if let Some(plan) = &mut self.plan {
                ui.label(format!("{} · {} → {}", plan.branch, &plan.head[..8], &plan.base[..8]));
                let mut movement = None;
                let row_count = plan.rows.len();
                for (i, row) in plan.rows.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(self.selected == i, format!("{} {}", &row.hash[..8], row.subject)).clicked() {
                            self.selected = i;
                        }
                        egui::ComboBox::from_id_salt(("history_action", i)).selected_text(row.action.title()).show_ui(ui, |ui| {
                            for action in [Action::Pick, Action::Reword, Action::Drop, Action::Fixup] {
                                if ui.selectable_value(&mut row.action, action, action.title()).changed() {
                                    self.confirmed = false;
                                }
                            }
                        });
                        if ui.add_enabled(!running && i > 0, egui::Button::new(tr("Move commit earlier"))).clicked() {
                            movement = Some((i, i - 1));
                        }
                        if ui.add_enabled(!running && i + 1 < row_count, egui::Button::new(tr("Move commit later"))).clicked() {
                            movement = Some((i, i + 1));
                        }
                    });
                }
                if let Some((from, to)) = movement {
                    plan.rows.swap(from, to);
                    self.selected = to;
                    self.confirmed = false;
                }
                if let Some(row) = plan.rows.get_mut(self.selected) {
                    if row.action == Action::Reword {
                        ui.label(tr("New commit message"));
                        if ui.add_enabled(!running, egui::TextEdit::multiline(&mut row.message).char_limit(4096).desired_rows(4)).changed() {
                            self.confirmed = false;
                        }
                    }
                }
                ui.add_enabled_ui(!running, |ui| {
                    ui.checkbox(&mut self.confirmed, tr("I reviewed this history plan and accept rewritten commit IDs"));
                });
                if ui.add_enabled(free && self.confirmed, egui::Button::new(tr("Rewrite History"))).clicked() {
                    self.apply(repo.clone());
                }
            }
            if running {
                ui.spinner();
            }
            if !self.output.is_empty() {
                ui.label(tr("Git output"));
                ui.add(egui::TextEdit::multiline(&mut self.output).font(egui::TextStyle::Monospace).interactive(false).desired_rows(8));
            }
        });
    }
}
impl Drop for HistoryEdit {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git").current_dir(root).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().into()
    }
    fn fixture(root: &Path) {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname='history-proof'\nversion='0.1.0'\nedition='2021'\n[workspace]\n").unwrap();
        std::fs::write(root.join(".gitignore"), "/target/\n/Cargo.lock\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn base()->u8{1}\n").unwrap();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.name", "History"]);
        git(root, &["config", "user.email", "history@example.test"]);
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "Base"]);
        for (name, body) in [("a.rs", "pub fn a()->u8{2}\n"), ("b.rs", "pub fn b()->u8{3}\n"), ("c.rs", "pub fn c()->u8{4}\n")] {
            std::fs::write(root.join("src").join(name), body).unwrap();
            git(root, &["add", "."]);
            git(root, &["commit", "-qm", name]);
        }
    }
    fn executable() -> PathBuf {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        let output =
            std::process::Command::new("cargo").args(["build", "--offline", "-p", "rc-app", "--bin", "crabcraft"]).current_dir(&workspace).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        workspace.join("target/debug").join(if cfg!(windows) { "crabcraft.exe" } else { "crabcraft" })
    }
    #[tokio::test]
    async fn previews_refuse_published_dirty_root_and_stale_or_invalid_plans() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fixture(&root);
        let repo = rc_vcs::Repository::discover(&root).await.unwrap().with_trust(true);
        let cancel = Cancel::default();
        let mut plan = preview(&repo, 3, &cancel).await.unwrap();
        assert_eq!(plan.rows.iter().map(|r| r.subject.as_str()).collect::<Vec<_>>(), ["a.rs", "b.rs", "c.rs"]);
        plan.rows[0].action = Action::Fixup;
        assert!(matches!(validate(&plan), Err(Error::Invalid)));
        plan.rows[0].action = Action::Pick;
        plan.rows.swap(0, 1);
        assert!(validate(&plan).is_ok());
        assert!(matches!(preview(&repo, 4, &cancel).await, Err(Error::Unsupported)));
        std::fs::write(root.join("untracked"), "must survive").unwrap();
        assert!(matches!(preview(&repo, 3, &cancel).await, Err(Error::Dirty)));
        std::fs::remove_file(root.join("untracked")).unwrap();
        git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        assert!(matches!(preview(&repo, 3, &cancel).await, Err(Error::Published)));
        git(&root, &["update-ref", "-d", "refs/remotes/origin/main"]);
        let cancelled = Cancel::default();
        cancelled.cancel();
        assert!(matches!(preview(&repo, 3, &cancelled).await, Err(Error::Cancelled)));
        let mut copied = plan.clone();
        copied.rows[0].hash = "injection\nexec danger".into();
        assert!(matches!(todo(&copied, Path::new("/exe"), Path::new("/plan")), Err(Error::Invalid)));
        assert!(rewrite(&repo.clone().with_trust(false), &plan, Path::new("/unused"), &cancel, &mut |_| {}, &mut |_| {}).await.is_err());
    }
    #[tokio::test]
    #[ignore = "Builds the real native editor helper and rewrites a real Rust project's history"]
    async fn real_native_helper_reorders_rewords_fixes_up_and_drops_with_a_recovery_ref() {
        let _home_lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("CRABCRAFT_HOME", home.path());
        }
        let exe = executable();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fixture(&root);
        let mut settings = rc_core::Settings::default();
        settings.trusted_projects.push(root.clone());
        settings.save().unwrap();
        let repo = rc_vcs::Repository::discover(&root).await.unwrap().with_trust(true);
        let cancel = Cancel::default();
        let mut plan = preview(&repo, 3, &cancel).await.unwrap();
        let old = plan.head.clone();
        plan.rows.swap(0, 1);
        plan.rows[0].action = Action::Reword;
        plan.rows[0].message = "Native message with 'quotes' and $(touch never)\n\nPreserved body".into();
        plan.rows[1].action = Action::Fixup;
        plan.rows[2].action = Action::Drop;
        let mut recovery = None;
        let mut lines = Vec::new();
        rewrite(&repo, &plan, &exe, &cancel, &mut |reference| recovery = Some(reference), &mut |line| lines.push(line))
            .await
            .unwrap_or_else(|error| panic!("{}\n{lines:?}", error.message()));
        let new = git(&root, &["rev-parse", "HEAD"]);
        assert_ne!(new, old);
        assert_eq!(git(&root, &["rev-parse", recovery.as_ref().unwrap()]), old);
        assert_eq!(git(&root, &["rev-list", "--count", "HEAD"]), "2");
        assert!(git(&root, &["log", "-1", "--format=%B"]).contains("$(touch never)"));
        assert!(!root.join("never").exists());
        assert!(root.join("src/a.rs").is_file() && root.join("src/b.rs").is_file());
        assert!(!root.join("src/c.rs").exists());
        assert!(git(&root, &["status", "--porcelain"]).is_empty());
        let output = rc_cargo::Cargo::locate().unwrap().std_command().args(["test", "--offline"]).current_dir(&root).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(matches!(rewrite(&repo, &plan, &exe, &cancel, &mut |_| {}, &mut |_| {}).await, Err(Error::Stale)));
    }
    #[test]
    #[ignore = "Builds the real private helper and exercises native history controls on a Rust crate"]
    fn native_history_plan_requires_confirmation_and_rewrites_only_after_explicit_apply() {
        use egui_kittest::kittest::Queryable;
        use std::time::{Duration, Instant};
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fixture(&root);
        let exe = executable();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(&root);
        app.trust_project_tools_for_test();
        app.settings.save().unwrap();
        app.history_edit.helper_executable = Some(exe);
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::Git, 900.0);
        app.dock.show(crate::toolwindow::ToolWindow::Git);
        let mut h = crate::screens::harness(app, egui::vec2(1700.0, 1200.0));
        let start = Instant::now();
        while h.state().vcs.status.is_none() && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        let old = git(&root, &["rev-parse", "HEAD"]);
        h.get_by_label(tr("Edit History")).click();
        h.run_steps(2);
        h.get_by_label(tr("Load History Plan")).click();
        h.run_steps(2);
        while h.state().history_edit.cancel.is_some() && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(h.state().history_edit.plan.as_ref().unwrap().rows.len(), 3);
        assert_eq!(git(&root, &["rev-parse", "HEAD"]), old);
        {
            let plan = h.state_mut().history_edit.plan.as_mut().unwrap();
            plan.rows[2].action = Action::Reword;
            plan.rows[2].message = "Native reviewed message".into();
        }
        h.run_steps(2);
        h.get_by_label(tr("Rewrite History")).click();
        h.run_steps(2);
        assert_eq!(git(&root, &["rev-parse", "HEAD"]), old);
        assert!(h.state().history_edit.cancel.is_none());
        h.get_by_label(tr("I reviewed this history plan and accept rewritten commit IDs")).click();
        h.run_steps(2);
        h.get_by_label(tr("Rewrite History")).click();
        h.run_steps(2);
        while h.state().history_edit.cancel.is_some() && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(h.state().history_edit.error.is_none(), "{:?}\n{}", h.state().history_edit.error, h.state().history_edit.output);
        assert_eq!(git(&root, &["log", "-1", "--format=%s"]), "Native reviewed message");
        assert_ne!(git(&root, &["rev-parse", "HEAD"]), old);
        assert_eq!(git(&root, &["rev-parse", h.state().history_edit.recovery.as_ref().unwrap()]), old);
    }
}
