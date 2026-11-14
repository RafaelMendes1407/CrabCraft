//! Regression tests for the application-owned trust boundary.
#![cfg(test)]

#[cfg(unix)]
#[test]
fn untrusted_open_and_execution_controls_never_invoke_project_toolchains_or_wrappers() {
    // Cargo's own test environment selects its toolchain and prepends its real bin folder.
    // Reproduce a desktop launch using rustup's proxies in an isolated process instead.
    let cargo_bin = dirs::home_dir().unwrap().join(".cargo/bin");
    let paths = std::iter::once(cargo_bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())).collect::<Vec<_>>();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "project_trust::untrusted_project_toolchain_child", "--ignored", "--nocapture"])
        .env_remove("RUSTUP_TOOLCHAIN")
        .env("PATH", std::env::join_paths(paths).unwrap())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[cfg(unix)]
#[test]
#[ignore = "Isolated desktop-like toolchain environment; invoked by the parent regression"]
fn untrusted_project_toolchain_child() {
    use crate::runner::{ConfigKind, ProjectExecutionError, RunConfig, RunnerOutput};
    use egui_kittest::kittest::Queryable;
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("fake-toolchain/bin")).unwrap();
    std::fs::create_dir_all(root.join("fake-toolchain/lib/rustlib")).unwrap();
    std::fs::create_dir(root.join(".cargo")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname='trust-proof'\nversion='0.1.0'\nedition='2021'\n[workspace]\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("build.rs"), "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}\n").unwrap();
    std::fs::write(
        root.join("rust-toolchain.toml"),
        format!("[toolchain]\npath={}\n", serde_json::to_string(&root.join("fake-toolchain").to_string_lossy()).unwrap()),
    )
    .unwrap();
    for tool in ["cargo", "rustc", "rust-analyzer"] {
        let path = root.join("fake-toolchain/bin").join(tool);
        std::fs::write(&path, format!("#!/bin/sh\nprintf invoked > toolchain-ran\nprintf invoked > {tool}-ran\nexit 1\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let wrapper = root.join("wrapper");
    std::fs::write(&wrapper, "#!/bin/sh\nprintf invoked > wrapper-ran\nexit 1\n").unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(root.join(".cargo/config.toml"), format!("[build]\nrustc-wrapper={}\n", serde_json::to_string(&wrapper.to_string_lossy()).unwrap()))
        .unwrap();
    let mut app = crate::screens::test_app(home.path(), true);
    app.open_workspace_and_wait(&root);
    assert_eq!(app.runner.require_trust(), Err(ProjectExecutionError::Untrusted));
    assert!(app.runner.cargo_tool().is_err());
    app.reload_cargo();
    app.invalidate_caches(true);
    app.load_toolchain();
    app.insight.restart(); // Restart must not turn an untrusted/closed session into execution.
    let denied = |outputs: Vec<RunnerOutput>| assert!(matches!(outputs.as_slice(), [RunnerOutput::Notify(_, true)]));
    denied(app.runner.build(&root.join("Cargo.toml"), rc_dotnet::BuildAction::Build));
    denied(app.runner.cargo_command_in(&root.join("Cargo.toml"), vec!["metadata".into()]));
    denied(app.runner.run_tests(&root.join("Cargo.toml"), None));
    denied(app.runner.run_scratch(&root.join("src/main.rs")));
    // A project-owned saved command cannot bypass the missing Cargo model.
    app.runner.configs.push(RunConfig { name: "malicious".into(), kind: ConfigKind::Command { program: wrapper.clone(), args: Vec::new() } });
    app.runner.selected = 0;
    denied(app.runner.run_selected());
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Debug);
    app.open_terminal(None);
    assert!(app.terminal_starting.is_none());
    let mut h = crate::screens::harness(app, egui::vec2(1400.0, 1000.0));
    for _ in 0..20 {
        h.run_steps(2);
        std::thread::sleep(Duration::from_millis(10));
    }
    h.get_by_label(rc_kit::i18n::tr("Restricted mode"));
    assert!(h.state().runner.cargo_workspace.is_none());
    assert!(!root.join("toolchain-ran").exists());
    assert!(!root.join("wrapper-ran").exists());
    assert!(!root.join("build-ran").exists());
    assert!(!root.join("target").exists());
    // The same malicious fixture really is executable: explicit Trust reloads metadata,
    // allowing rustup's project-selected cargo to run. No shell or network is necessary.
    h.get_by_label(rc_kit::i18n::tr("Trust this project")).click();
    let start = Instant::now();
    while !root.join("cargo-ran").exists() && start.elapsed() < Duration::from_secs(15) {
        h.run_steps(2);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        root.join("cargo-ran").exists(),
        "Trust must reload the deferred Cargo model: {:?}; trusted {:?}",
        h.state().sync_log.entries,
        h.state().runner.trusted_project
    );
    assert!(h.state().settings.trusted_projects.contains(&root));
    assert!(h.state().runner.require_trust().is_ok());
    h.state_mut().insight.stop();
    // Once the custom toolchain is removed, the project's compiler wrapper is another
    // execution path of dependency metadata. It too was absent before explicit trust.
    std::fs::remove_file(root.join("rust-toolchain.toml")).unwrap();
    h.state_mut().trust_project_tools_for_test();
    let start = Instant::now();
    while !root.join("wrapper-ran").exists() && start.elapsed() < Duration::from_secs(15) {
        h.run_steps(2);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(root.join("wrapper-ran").exists(), "The configured wrapper fixture must be effective after trust");
    assert!(!root.join("build-ran").exists());
}

#[cfg(unix)]
#[test]
fn a_plain_git_folder_stays_restricted_until_the_native_trust_control_is_used() {
    use egui_kittest::kittest::Queryable;
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git").args(args).current_dir(&root).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "Trust test"]);
    git(&["config", "user.email", "trust@example.test"]);
    std::fs::write(root.join("example.rs"), "fn main() {}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "Rust source"]);
    let monitor = root.join("monitor");
    std::fs::write(&monitor, "#!/bin/sh\nprintf invoked > git-ran\nexit 0\n").unwrap();
    std::fs::set_permissions(&monitor, std::fs::Permissions::from_mode(0o700)).unwrap();
    git(&["config", "core.fsmonitor", monitor.to_str().unwrap()]);
    let mut app = crate::screens::test_app(home.path(), true);
    app.open_workspace_and_wait(&root);
    assert!(!app.vcs.trusted);
    assert!(app.vcs.repo.is_none());
    app.vcs.changelists.create("Review").unwrap();
    app.vcs.message = "retained draft".into();
    app.vcs.refresh();
    app.vcs.fetch();
    app.vcs.push();
    let mut h = crate::screens::harness(app, egui::vec2(1400.0, 1000.0));
    for _ in 0..10 {
        h.run_steps(2);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!root.join("git-ran").exists());
    assert!(h.state().vcs.repo.is_none());
    h.get_by_label(rc_kit::i18n::tr("Restricted mode"));
    h.get_by_label(rc_kit::i18n::tr("Trust this project")).click();
    let start = Instant::now();
    while h.state().vcs.status.is_none() && start.elapsed() < Duration::from_secs(10) {
        h.run_steps(2);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(h.state().vcs.status.is_some(), "{:?}", h.state().vcs.error);
    assert!(h.state().vcs.trusted);
    assert!(root.join("git-ran").exists());
    assert!(h.state().vcs.changelists.lists.contains_key("Review"));
    assert_eq!(h.state().vcs.message, "retained draft");
    // A non-Cargo folder does not accidentally start rust-analyzer after trust.
    assert!(h.state().insight.session.is_none());
}
