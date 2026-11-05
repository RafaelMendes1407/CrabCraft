//! Welcome-screen clone: Git runs on the async runtime with cancellable process-group
//! handling. Checkout cannot use project hooks, global filters or recursive submodules.
use rc_core::{Inbox, Waker};
use rc_dotnet::CancelToken;
use rc_kit::i18n::{tr, trf};
use std::path::{Path, PathBuf};

#[derive(Debug)]
enum CloneError { Source, Destination, Git(rc_vcs::GitError), Io(std::io::Error), Failed(String), Cancelled }
enum Event { Progress(String), Done(Result<PathBuf, CloneError>) }
pub struct CloneProject {
    pub source: String,
    pub destination: String,
    cancel: Option<CancelToken>,
    inbox: Inbox<Event>,
    pub output: String,
    error: Option<String>,
}
impl CloneProject {
    pub fn new(waker: Waker) -> Self { Self { source: String::new(), destination: String::new(), cancel: None, inbox: Inbox::new(waker), output: String::new(), error: None } }
    fn start(&mut self) {
        if self.cancel.is_some() { return; }
        let (source, destination) = (self.source.trim().to_string(), PathBuf::from(self.destination.trim()));
        let cancel = CancelToken::new(); self.cancel = Some(cancel.clone());
        self.output.clear(); self.error = None;
        let out = self.inbox.sender();
        rc_core::Runtime::spawn(async move {
            let lines = out.clone();
            let result = clone_repository(&source, &destination, &cancel, &mut |text| { lines.send(Event::Progress(text)); }).await;
            out.send(Event::Done(result));
        });
    }
    fn stop(&self) { if let Some(cancel) = &self.cancel { cancel.cancel(); } }
}
impl Drop for CloneProject { fn drop(&mut self) { self.stop(); } }

async fn clone_repository(source: &str, destination: &Path, cancel: &CancelToken, progress: &mut (dyn FnMut(String) + Send)) -> Result<PathBuf, CloneError> {
    if cancel.is_cancelled() { return Err(CloneError::Cancelled); }
    if source.is_empty() || source.len() > 4096 || source.starts_with('-') || source.chars().any(char::is_control) { return Err(CloneError::Source); }
    if destination.as_os_str().is_empty() { return Err(CloneError::Destination); }
    let destination = if destination.is_absolute() { destination.to_path_buf() } else { std::env::current_dir().map_err(CloneError::Io)?.join(destination) };
    let parent = destination.parent().ok_or(CloneError::Destination)?;
    if !parent.is_dir() { return Err(CloneError::Destination); }
    let git = rc_vcs::Git::detect().await.map_err(CloneError::Git)?;
    if cancel.is_cancelled() { return Err(CloneError::Cancelled); }
    // Atomic reservation also rejects existing empty folders and dangling symlinks.
    std::fs::create_dir(&destination).map_err(|error| if error.kind() == std::io::ErrorKind::AlreadyExists { CloneError::Destination } else { CloneError::Io(error) })?;
    let templates = tempfile::Builder::new().prefix("crabcraft-clone-hooks-").tempdir().map_err(CloneError::Io)?;
    let mut hooks = std::ffi::OsString::from("core.hooksPath="); hooks.push(templates.path());
    let mut command = tokio::process::Command::new(git.path());
    command.current_dir(parent).args(["-c", "protocol.allow=never", "-c", "protocol.file.allow=always", "-c", "protocol.https.allow=always", "-c", "protocol.http.allow=always", "-c", "protocol.ssh.allow=always", "-c", "protocol.git.allow=always", "-c"])
        .arg(hooks).args(["-c", "submodule.recurse=false", "-c", "color.ui=false", "clone", "--no-recurse-submodules", "--no-local", "--progress", "--"])
        .arg(source).arg(&destination)
        .env("LC_ALL", "C").env("LANGUAGE", "C").env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_SYSTEM", if cfg!(windows) { "NUL" } else { "/dev/null" })
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" }).env("GIT_CONFIG_COUNT", "0")
        .env("GIT_TEMPLATE_DIR", templates.path()).env("GIT_ALLOW_PROTOCOL", "file:ssh:https:http:git")
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_COMMON_DIR").env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY").env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES").env_remove("GIT_CONFIG")
        .env_remove("GIT_ASKPASS").env_remove("SSH_ASKPASS").env("SSH_ASKPASS_REQUIRE", "never")
        .env_remove("GIT_SSH").env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    let mut output = String::new();
    let mut line = |line: rc_dotnet::OutputLine| {
        let text: String = line.text.chars().take(2048).collect();
        if output.len() < 64 * 1024 { output.push_str(&text); output.push('\n'); }
        progress(text);
    };
    let outcome = rc_dotnet::process::run_streaming(command, cancel, rc_dotnet::process::StreamOpts { partial_lines: true, ..Default::default() }, &mut |_| {}, &mut line).await.map_err(CloneError::Io)?;
    if outcome.cancelled || cancel.is_cancelled() { return Err(CloneError::Cancelled); }
    if outcome.code != Some(0) { return Err(CloneError::Failed(output)); }
    destination.canonicalize().map_err(CloneError::Io)
}
fn error_text(error: CloneError) -> String {
    match error {
        CloneError::Source => tr("Enter a Git repository URL or local repository path").to_string(),
        CloneError::Destination => tr("Choose a destination folder whose parent exists and which does not already exist").to_string(),
        CloneError::Git(error) => {
            let (text, args) = error.user_message();
            let args: Vec<(&str, &dyn std::fmt::Display)> = args.iter().map(|(name, value)| (*name, value as &dyn std::fmt::Display)).collect(); trf(text, &args)
        }
        CloneError::Io(error) => trf("Could not access the clone destination: {error}", &[("error", &error)]),
        CloneError::Failed(output) => trf("Git clone failed: {output}", &[("output", &output)]),
        CloneError::Cancelled => tr("Clone cancelled; its incomplete destination folder may remain.").to_string(),
    }
}

pub fn show(app: &mut crate::app::CrabCraftApp, ctx: &egui::Context) -> bool {
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) { app.clone_project.stop(); return true; }
    let state = &mut app.clone_project;
    let mut opened = None;
    for event in state.inbox.drain() {
        match event {
            Event::Progress(text) => if state.output.len() < 64 * 1024 { state.output.push_str(&text); state.output.push('\n'); },
            Event::Done(result) => {
                let cancelled = state.cancel.take().is_some_and(|token| token.is_cancelled());
                if cancelled { state.error = Some(error_text(CloneError::Cancelled)); }
                else { match result { Ok(path) => opened = Some(path), Err(error) => state.error = Some(error_text(error)) } }
            }
        }
    }
    let mut close = false;
    egui::Modal::new(egui::Id::new("clone_git_repository")).show(ctx, |ui| {
        ui.set_width(600.0);
        ui.heading(tr("Clone Git Repository"));
        let busy = state.cancel.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            ui.label(tr("Repository URL or local path"));
            ui.add(egui::TextEdit::singleline(&mut state.source).desired_width(f32::INFINITY).char_limit(4096));
            ui.label(tr("Destination folder (must not exist)"));
            ui.add(egui::TextEdit::singleline(&mut state.destination).desired_width(f32::INFINITY).char_limit(4096));
        });
        ui.label(tr("Clones without submodules, hooks or globally configured checkout filters. The project opens without trust; build scripts and procedural macros stay disabled."));
        ui.small(tr("Interactive authentication and custom Git remote helpers are unsupported. Use SSH agent or a repository accessible without an interactive prompt."));
        if busy { ui.spinner(); }
        if !state.output.is_empty() { egui::ScrollArea::vertical().id_salt("clone_output").max_height(180.0).show(ui, |ui| { ui.monospace(&state.output); }); }
        if let Some(error) = &state.error { ui.colored_label(rc_kit::pal(ui).error, error); }
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy && !state.source.trim().is_empty() && !state.destination.trim().is_empty(), egui::Button::new(tr("Clone"))).clicked() { state.start(); }
            if ui.button(if busy { tr("Cancel Clone") } else { tr("Close") }).clicked() { state.stop(); close = true; }
        });
    });
    if let Some(path) = opened {
        // A deleted project's saved trust must never be inherited by newly cloned code.
        app.settings.trusted_projects.retain(|root| root != &path);
        app.open_workspace(&path);
        close = true;
    }
    close
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{HOME_LOCK, harness, test_app};
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};
    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git").current_dir(root).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    fn repository(root: &Path) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.name", "Clone Test"]); git(root, &["config", "user.email", "clone@example.test"]);
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"clone_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u32 { 42 }\n#[test] fn works() { assert_eq!(value(), 42); }\n").unwrap();
        std::fs::write(root.join("build.rs"), "fn main() { std::fs::write(\"build-ran\", \"yes\").unwrap(); }\n").unwrap();
        std::fs::write(root.join(".gitattributes"), "*.rs filter=untrusted\n").unwrap();
        std::fs::write(root.join(".gitmodules"), "[submodule \"never\"]\npath = never\nurl = ext::false\n").unwrap();
        git(root, &["add", "."]); git(root, &["commit", "-qm", "Rust fixture"]);
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            let hook = root.join(".git/hooks/post-checkout");
            std::fs::write(&hook, "#!/bin/sh\ntouch hook-ran\n").unwrap();
            std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    #[test]
    fn native_welcome_clone_opens_a_real_rust_project_without_inheriting_trust_or_running_code() {
        let _lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap(); let source = tempfile::tempdir().unwrap(); let parent = tempfile::tempdir().unwrap();
        repository(source.path());
        let destination = parent.path().canonicalize().unwrap().join("cloned project");
        let mut app = test_app(home.path(), true);
        app.settings.trusted_projects.push(destination.clone()); // Stale trust for a deleted project.
        let mut h = harness(app, egui::vec2(1440.0, 1000.0));
        h.get_by_label(tr("Clone Repository…")).click(); h.run_steps(2);
        h.state_mut().clone_project.source = source.path().to_string_lossy().into_owned();
        h.state_mut().clone_project.destination = destination.to_string_lossy().into_owned();
        h.run_steps(2); h.get_by_label(tr("Clone")).click(); h.run_steps(2);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) && h.state().workspace.is_none() {
            h.run_steps(2); std::thread::sleep(Duration::from_millis(20));
        }
        assert!(h.state().workspace.is_some(), "{:?}\n{}", h.state().clone_project.error, h.state().clone_project.output);
        assert!(h.state().runner.cargo_workspace.is_none());
        assert!(h.state().workspace.as_ref().is_some_and(|w| w.ws.root == destination));
        assert!(!h.state().settings.trusted_projects.contains(&destination));
        assert!(h.state().runner.trusted_project.is_none());
        assert!(!destination.join("build-ran").exists()); assert!(!destination.join("hook-ran").exists());
        assert!(!destination.join("never").exists());
        assert!(h.state().dialog.is_none());
        // The cloned sources really build; only this explicit test invocation executes build.rs.
        let output = rc_cargo::Cargo::locate().unwrap().std_command().args(["test", "--offline"]).current_dir(&destination).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(destination.join("build-ran").exists());
    }
    #[tokio::test]
    async fn refuses_existing_destinations_and_external_helpers_and_honors_pre_cancel() {
        let source = tempfile::tempdir().unwrap(); repository(source.path()); let parent = tempfile::tempdir().unwrap();
        let existing = parent.path().join("existing"); std::fs::create_dir(&existing).unwrap(); std::fs::write(existing.join("keep"), "mine").unwrap();
        let result = clone_repository(source.path().to_str().unwrap(), &existing, &CancelToken::new(), &mut |_| {}).await;
        assert!(matches!(result, Err(CloneError::Destination)));
        assert_eq!(std::fs::read_to_string(existing.join("keep")).unwrap(), "mine");
        let cancel = CancelToken::new(); cancel.cancel(); let destination = parent.path().join("cancelled");
        assert!(matches!(clone_repository(source.path().to_str().unwrap(), &destination, &cancel, &mut |_| {}).await, Err(CloneError::Cancelled)));
        assert!(!destination.exists());
        let result = clone_repository("ext::false", &parent.path().join("external"), &CancelToken::new(), &mut |_| {}).await;
        assert!(matches!(result, Err(CloneError::Failed(_))));
    }
    #[test]
    #[cfg(unix)]
    fn configured_smudge_filters_and_template_hooks_do_not_run_during_clone() {
        use std::os::unix::fs::PermissionsExt;
        let source = tempfile::tempdir().unwrap(); repository(source.path());
        let parent = tempfile::tempdir().unwrap();
        let config = parent.path().join("global.gitconfig");
        let filter_marker = parent.path().join("filter-ran");
        let hook_marker = parent.path().join("template-hook-ran");
        let filter = "sh -c 'printf yes > \"$CRABCRAFT_FILTER_MARKER\"; cat'";
        let output = std::process::Command::new("git").args(["config", "--file"]).arg(&config).args(["filter.untrusted.smudge", filter]).output().unwrap();
        assert!(output.status.success());
        let template = parent.path().join("template"); std::fs::create_dir_all(template.join("hooks")).unwrap();
        let hook = template.join("hooks/post-checkout");
        std::fs::write(&hook, "#!/bin/sh\nprintf yes > \"$CRABCRAFT_HOOK_MARKER\"\n").unwrap();
        std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Prove the fixture's filters and template hooks would execute with ordinary Git.
        let output = std::process::Command::new("git").current_dir(parent.path()).args(["clone", "--no-local"])
            .arg(source.path()).arg(parent.path().join("control"))
            .env("GIT_CONFIG_GLOBAL", &config).env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_COUNT", "0")
            .env("GIT_TEMPLATE_DIR", &template).env("CRABCRAFT_FILTER_MARKER", &filter_marker).env("CRABCRAFT_HOOK_MARKER", &hook_marker).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(filter_marker.exists() && hook_marker.exists(), "the control checkout did not execute the test hooks/filters");
        std::fs::remove_file(&filter_marker).unwrap(); std::fs::remove_file(&hook_marker).unwrap();
        let destination = parent.path().join("isolated-clone");
        // A child test process safely supplies inherited Git settings without mutating the
        // process-wide environment shared by the rest of the test suite.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["clone_project::tests::clone_with_inherited_git_settings_child", "--ignored", "--exact", "--nocapture"])
            .env("CRABCRAFT_CLONE_TEST_SOURCE", source.path()).env("CRABCRAFT_CLONE_TEST_DESTINATION", &destination)
            .env("GIT_CONFIG_GLOBAL", config).env("GIT_TEMPLATE_DIR", template)
            .env("GIT_CONFIG_COUNT", "1").env("GIT_CONFIG_KEY_0", "filter.untrusted.smudge").env("GIT_CONFIG_VALUE_0", filter)
            .env("CRABCRAFT_FILTER_MARKER", &filter_marker).env("CRABCRAFT_HOOK_MARKER", &hook_marker).output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(destination.join("src/lib.rs").is_file());
        assert!(!filter_marker.exists()); assert!(!hook_marker.exists());
    }
    #[tokio::test]
    #[cfg(unix)]
    #[ignore = "Child fixture invoked by configured_smudge_filters_and_template_hooks_do_not_run_during_clone"]
    async fn clone_with_inherited_git_settings_child() {
        let source = std::env::var("CRABCRAFT_CLONE_TEST_SOURCE").unwrap();
        let destination = std::env::var_os("CRABCRAFT_CLONE_TEST_DESTINATION").map(PathBuf::from).unwrap();
        clone_repository(&source, &destination, &CancelToken::new(), &mut |_| {}).await.unwrap();
    }
    #[tokio::test]
    async fn cancelling_a_clone_stops_the_git_process_waiting_on_http() {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap();
        let (accepted, receiver) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || { let (socket, _) = server.accept().unwrap(); accepted.send(()).unwrap(); let _ = released.recv_timeout(Duration::from_secs(10)); drop(socket); });
        let parent = tempfile::tempdir().unwrap(); let destination = parent.path().join("cancelled");
        let cancel = CancelToken::new(); let token = cancel.clone();
        let work = tokio::spawn(async move { clone_repository(&format!("http://{address}/repo"), &destination, &token, &mut |_| {}).await });
        tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(10)).unwrap()).await.unwrap();
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), work).await.unwrap().unwrap();
        assert!(matches!(result, Err(CloneError::Cancelled)));
        let _ = release.send(()); worker.join().unwrap();
    }
}
