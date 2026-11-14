//! Native Tab must reach the shell's completion engine and must never execute candidates.
#![cfg(unix)]
use rc_term::{SpawnSpec, TermConfig, TermSession, TerminalView};
use std::time::Duration;

#[test]
fn native_tab_completes_commands_and_rust_paths_before_explicit_enter() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join("src")).unwrap();
    std::fs::write(project.path().join("Cargo.toml"), "[package]\nname='crabcraft_completion_probe'\nversion='0.1.0'\nedition='2024'\n[workspace]\n").unwrap();
    std::fs::write(
        project.path().join("src/main.rs"),
        "fn main() { std::fs::write(\"completed-command\", \"yes\").unwrap(); println!(\"COMPLETED_RUST_COMMAND\"); }\n",
    )
    .unwrap();
    let built = std::process::Command::new("cargo").args(["build", "--offline", "--manifest-path"]).arg(project.path().join("Cargo.toml")).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let path = std::env::join_paths([project.path().join("target/debug"), "/usr/bin".into(), "/bin".into()]).unwrap();
    let spec = SpawnSpec::command("/bin/bash", ["--noprofile", "--norc", "-i"], Some(project.path().into()))
        .env("PATH", path.to_str().unwrap())
        .env("PS1", "CC_COMPLETE> ")
        .env("INPUTRC", "/dev/null");
    let session = TermSession::spawn(&spec, rc_core::Waker::noop()).unwrap();
    assert!(session.wait_for_text("CC_COMPLETE>", Duration::from_secs(5)));
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1100.0, 400.0)).build_ui_state(
        |ui, state: &mut TermSession| {
            TerminalView::new(state, &TermConfig::default()).focus(true).show(ui);
        },
        session,
    );
    harness.run_steps(3);
    harness.event(egui::Event::Text("crabcraft_completion_pr".into()));
    harness.run_steps(1);
    harness.key_press(egui::Key::Tab);
    harness.run_steps(2);
    assert!(harness.state().wait_for_text("crabcraft_completion_probe", Duration::from_secs(5)), "{:?}", harness.state().screen_lines());
    assert!(!project.path().join("completed-command").exists());
    harness.key_press(egui::Key::Enter);
    harness.run_steps(2);
    assert!(harness.state().wait_for_text("COMPLETED_RUST_COMMAND", Duration::from_secs(5)));
    assert_eq!(std::fs::read_to_string(project.path().join("completed-command")).unwrap(), "yes");
    harness.event(egui::Event::Text("cat src/ma".into()));
    harness.run_steps(1);
    harness.key_press(egui::Key::Tab);
    harness.run_steps(2);
    assert!(harness.state().wait_for_text("cat src/main.rs", Duration::from_secs(5)), "{:?}", harness.state().screen_lines());
    harness.key_press(egui::Key::Enter);
    harness.run_steps(2);
    assert!(harness.state().wait_for_text("fn main()", Duration::from_secs(5)));
}
