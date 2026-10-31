//! The Rust pipeline end to end inside the app (no window), against the real toolchain:
//! opening a Cargo project runs none of its code, trusting it starts rust-analyzer, and the
//! insight, build, run and test paths work on it.

use crate::app::CrabCraftApp;
use crate::insight::InsightStatus;
use crate::runner::ConfigKind;
use rc_core::{Settings, Waker};
use std::path::Path;
use std::time::{Duration, Instant};

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

const LIB: &str = "pub struct Greeter;\n\nimpl Greeter {\n    pub fn hello(&self) -> &'static str {\n        \"hello\"\n    }\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn works() {\n        assert_eq!(super::Greeter.hello(), \"hello\");\n    }\n}\n";
const MAIN: &str = "use demo::Greeter;\n\nfn main() {\n    println!(\"{}\", Greeter.hello());\n}\n";

fn demo(root: &Path) {
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/lib.rs"), LIB);
    write(&root.join("src/main.rs"), MAIN);
}

fn pump(app: &mut CrabCraftApp, what: &str, mut done: impl FnMut(&mut CrabCraftApp) -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(180) {
        app.poll_cargo_workspace();
        let out = app.insight.update(&mut app.docs, true);
        app.handle_insight(out);
        let outs = app.runner.update();
        crate::ui::run_windows::handle_outputs(app, outs);
        app.docs.poll_parses();
        if let InsightStatus::Failed(why) = &app.insight.status {
            panic!("server failed: {why}");
        }
        if done(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}; status {:?}", app.insight.status);
}

fn new_app(root: &Path) -> CrabCraftApp {
    // SAFETY: set before the app reads it, under HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", root.join("home")) };
    CrabCraftApp::with_settings(Settings::default(), Waker::noop(), None)
}

#[test]
fn an_untrusted_project_runs_nothing_until_the_user_trusts_it() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let root = root.canonicalize().unwrap();
    write(&root.join("build.rs"), "fn main() { std::fs::write(\"BUILD_RAN\", \"\").unwrap(); }\n");
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    assert_eq!(app.insight.status, InsightStatus::Untrusted);
    assert!(app.insight.session.is_none());
    assert!(!root.join("BUILD_RAN").exists());
    assert!(!root.join("target").exists());

    app.trust_project();
    assert!(app.settings.trusted_projects.contains(&root));
    assert!(app.insight.session.is_some());
    pump(&mut app, "the build script", |a| { let _ = a; root.join("BUILD_RAN").exists() });
    app.insight.stop();
}

#[test]
fn rust_insight_build_run_and_tests_end_to_end() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    let names: Vec<_> = app.runner.configs.iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, ["demo", "All Tests"]);
    assert!(matches!(&app.runner.configs[0].kind, ConfigKind::Cargo { bin, .. } if bin == "demo"));

    // Run.
    app.runner.selected = 0;
    app.runner.run_selected();
    pump(&mut app, "the program", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    assert_eq!(app.runner.runs[0].exit, Some(Some(0)));
    assert!(app.runner.runs[0].console.term.screen_lines().join("\n").contains("hello"));

    // Tests.
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::RunTests);
    pump(&mut app, "the tests", |a| a.runner.tests.as_ref().is_some_and(|t| !t.running));
    let t = app.runner.tests.as_ref().unwrap();
    assert_eq!((t.passed, t.failed), (1, 0), "{:?}", t.failure);
    assert!(t.results.contains_key("tests::works"), "{:?}", t.results.keys());

    // Insight.
    app.trust_project();
    let main = root.join("src/main.rs");
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);

    // Colours from the server.
    pump(&mut app, "semantic tokens", |a| a.docs.active_doc().is_some_and(|d| d.editor.semantic.iter().flatten().any(|s| s.hl == rc_syntax::Hl::Struct)));

    // Completion lists the method after a dot.
    let doc = app.docs.active_doc_mut().unwrap();
    let pos = doc.editor.buffer.rope().to_string().find("Greeter.hello").unwrap() + "Greeter.".len();
    doc.editor.selection = rc_text::Selection::point(pos);
    let i = app.docs.active.unwrap();
    app.insight.on_editor_events(&mut app.docs.list[i], &[rc_editor::EditorEvent::Typed('.')]);
    pump(&mut app, "completion", |a| a.docs.active_doc().and_then(|d| d.editor.popups.completion.as_ref()).is_some_and(|c| !c.filtered.is_empty()));
    let c = app.docs.active_doc().unwrap().editor.popups.completion.clone().unwrap();
    assert!(c.items.iter().any(|it| it.label.starts_with("hello")), "{:?}", c.items.iter().map(|i| &i.label).take(10).collect::<Vec<_>>());
    app.docs.active_doc_mut().unwrap().editor.popups.close_all();

    // Go to definition opens the library.
    let pos = MAIN.find("hello").unwrap() + 1;
    let i = app.docs.active.unwrap();
    app.insight.navigate(&app.docs.list[i], pos, crate::insight::NavKind::Declaration);
    pump(&mut app, "the definition", |a| a.docs.active_doc().is_some_and(|d| d.path.ends_with("src/lib.rs")));

    // A type error is reported by the analysis as you type, and with its position by the build.
    app.open_file(&main);
    let bad = "fn main() {\n    let x: u32 = \"text\";\n}\n";
    let doc = app.docs.active_doc_mut().unwrap();
    let end = doc.editor.buffer.len_chars();
    doc.editor.replace(0, end, bad);
    write(&main, bad);
    let target = app.workspace.as_ref().unwrap().ws.key_path().to_path_buf();
    app.runner.build(&target, rc_dotnet::BuildAction::Build);
    pump(&mut app, "the build", |a| a.runner.build.as_ref().is_some_and(|b| b.result.is_some() || b.failed.is_some()));
    let b = app.runner.build.as_ref().unwrap();
    assert!(!b.result.as_ref().unwrap().success);
    assert!(b.diagnostics.iter().any(|d| d.code == "E0308" && d.line == Some(2)), "{:?}", b.diagnostics);
    assert!(app.insight.problems.get(&main).is_some_and(|p| p.iter().any(|d| d.severity == rc_editor::Severity::Error)), "no problems for main.rs");
    app.insight.stop();
}
