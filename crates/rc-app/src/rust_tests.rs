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

/// What the language server offers once a file is open, each against the real rust-analyzer.
#[test]
fn rust_language_features() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    let main = root.join("src/main.rs");
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);

    // Structure of the file and inlay hints arrive with the refresh.
    let id = app.docs.active_doc().unwrap().editor.id;
    pump(&mut app, "the structure", |a| a.insight.structure.get(&id).is_some_and(|s| s.iter().any(|n| n.name == "main")));

    // Workspace symbols.
    let mut symbols = app.insight.workspace_symbols("Greeter").expect("a request");
    let found = {
        let start = Instant::now();
        loop {
            if let Some(r) = symbols.poll() {
                break r.expect("symbols");
            }
            assert!(start.elapsed() < Duration::from_secs(30), "no symbols");
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    assert!(found.to_string().contains("Greeter"), "{found}");

    // Rename across files: the use in main.rs and the definition in lib.rs.
    let pos = MAIN.find("Greeter.hello").unwrap() + "Greeter.".len() + 1;
    let i = app.docs.active.unwrap();
    app.insight.rename(&app.docs.list[i], pos, "greet");
    pump(&mut app, "the rename", |a| a.docs.active_doc().is_some_and(|d| d.editor.buffer.rope().to_string().contains("Greeter.greet()")));
    pump(&mut app, "the definition to change", |_| std::fs::read_to_string(root.join("src/lib.rs")).is_ok_and(|t| t.contains("pub fn greet(")));

    // A new error shows while typing, with no build, and marks its range.
    let doc = app.docs.active_doc_mut().unwrap();
    let end = doc.editor.buffer.len_chars();
    doc.editor.replace(end, end, "\nfn broken( {\n");
    let i = app.docs.active.unwrap();
    app.insight.on_editor_events(&mut app.docs.list[i], &[rc_editor::EditorEvent::Changed]);
    pump(&mut app, "a live diagnostic", |a| a.docs.active_doc().is_some_and(|d| d.editor.diagnostics.iter().any(|x| x.severity == rc_editor::Severity::Error)));

    // Saving runs `cargo check`. The wording is rustc's: rust-analyzer's own check says "expected u32".
    let saved = "use demo::Greeter;\n\nfn main() {\n    let x: u32 = \"text\";\n    println!(\"{} {x}\", Greeter.greet());\n}\n";
    let doc = app.docs.active_doc_mut().unwrap();
    let end = doc.editor.buffer.len_chars();
    doc.editor.replace(0, end, saved);
    let i = app.docs.active.unwrap();
    app.insight.on_editor_events(&mut app.docs.list[i], &[rc_editor::EditorEvent::Changed]);
    write(&main, saved);
    app.insight.did_save(&main);
    pump(&mut app, "the error of cargo check", |a| a.docs.active_doc().is_some_and(|d| d.editor.diagnostics.iter().any(|x| x.code.as_deref() == Some("E0308") && x.message.contains("mismatched types"))));
    app.insight.stop();
}

/// Runs the insight loop until `want` takes one of its outputs.
fn output<T>(app: &mut CrabCraftApp, what: &str, mut want: impl FnMut(&crate::insight::InsightOutput) -> Option<T>) -> T {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        let out = app.insight.update(&mut app.docs, true);
        if let Some(found) = out.iter().find_map(&mut want) {
            return found;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}; status {:?}", app.insight.status);
}

#[test]
fn rust_navigation_and_hints() {
    use crate::insight::{InsightOutput, NavKind};
    const SOURCE: &str = "use demo::Greeter;\n\ntrait Speak {\n    fn speak(&self) -> u32;\n}\nstruct A;\nstruct B;\nimpl Speak for A {\n    fn speak(&self) -> u32 { 1 }\n}\nimpl Speak for B {\n    fn speak(&self) -> u32 { 2 }\n}\n\nfn twice(amount: u32) -> u32 {\n    amount * 2\n}\n\nfn main() {\n    let total = twice(A.speak());\n    println!(\"{} {} {}\", total, twice(B.speak()), Greeter.hello());\n    let map = HashMap::<u32, u32>::new();\n    drop(map);\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let main = root.join("src/main.rs");
    write(&main, SOURCE);
    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.rs");
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);

    // Inlay hints: the inferred type and the parameter name.
    pump(&mut app, "the inlay hints", |a| {
        a.docs.active_doc().is_some_and(|d| {
            d.editor.inlays.iter().any(|h| h.label.contains("u32")) && d.editor.inlays.iter().any(|h| h.label.contains("amount"))
        })
    });

    // Hover.
    let i = app.docs.active.unwrap();
    app.docs.list[i].editor.selection = rc_text::Selection::point(SOURCE.find("twice(A").unwrap() + 1);
    app.insight.quick_doc(&app.docs.list[i], egui::Pos2::ZERO);
    pump(&mut app, "the hover", |a| {
        a.docs.active_doc().and_then(|d| d.editor.popups.hover.as_ref()).is_some_and(|h| h.text.contains("fn twice(amount: u32) -> u32"))
    });

    // Parameter info.
    let inside = SOURCE.find("twice(A").unwrap() + "twice(".len();
    app.docs.list[i].editor.selection = rc_text::Selection::point(inside);
    app.insight.request_signature(&app.docs.list[i], inside);
    pump(&mut app, "the signature", |a| a.docs.active_doc().is_some_and(|d| d.editor.popups.signature.is_some()));

    // Usages: both calls, whatever else the server adds.
    let at = SOURCE.find("twice(A").unwrap() + 1;
    app.insight.navigate(&app.docs.list[i], at, NavKind::Usages);
    let lines = output(&mut app, "the usages", |o| match o {
        InsightOutput::Usages { targets, .. } => Some(targets.iter().map(|t| t.line).collect::<Vec<_>>()),
        _ => None,
    });
    assert!(lines.contains(&19) && lines.contains(&20), "{lines:?}");

    // Implementations of the trait.
    let at = SOURCE.find("Speak").unwrap() + 1;
    app.insight.navigate(&app.docs.list[i], at, NavKind::Implementation);
    let lines = output(&mut app, "the implementations", |o| match o {
        InsightOutput::Usages { targets, .. } => Some(targets.iter().map(|t| t.line).collect::<Vec<_>>()),
        _ => None,
    });
    assert_eq!(lines.len(), 2, "{lines:?}");

    // Alt+Enter: importing the missing name changes the buffer.
    app.docs.list[i].editor.selection = rc_text::Selection::point(SOURCE.find("HashMap").unwrap() + 1);
    app.insight.code_actions(&app.docs.list[i]);
    let actions = output(&mut app, "the code actions", |o| match o {
        InsightOutput::Actions(a) => Some(a.clone()),
        _ => None,
    });
    let title = |a: &rc_lsp::lsp_types::CodeActionOrCommand| match a {
        rc_lsp::lsp_types::CodeActionOrCommand::Command(c) => c.title.clone(),
        rc_lsp::lsp_types::CodeActionOrCommand::CodeAction(c) => c.title.clone(),
    };
    let import = actions.iter().find(|a| title(a).contains("Import")).unwrap_or_else(|| panic!("{:?}", actions.iter().map(title).collect::<Vec<_>>())).clone();
    let mut out = Vec::new();
    app.insight.run_code_action(&mut app.docs, import, &mut out);
    pump(&mut app, "the import", |a| a.docs.active_doc().is_some_and(|d| d.editor.buffer.rope().to_string().contains("use std::collections::HashMap;")));
    app.insight.stop();
}

#[test]
fn gutter_icons_run_tests_and_binaries() {
    use rc_editor::GutterIcon;
    const LIB: &str = "pub fn one() -> u32 {\n    1\n}\n\n#[cfg(test)]\nmod checks {\n    #[test]\n    fn good() {\n        assert_eq!(super::one(), 1);\n    }\n\n    #[test]\n    fn bad() {\n        assert_eq!(super::one(), 2);\n    }\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/lib.rs"), LIB);
    write(&root.join("src/main.rs"), "fn main() {\n    println!(\"from the icon\");\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/lib.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);

    let icon = |a: &mut CrabCraftApp, line: usize| {
        a.update_test_markers();
        a.docs.active_doc().and_then(|d| d.editor.gutter_icons.get(&line).copied())
    };
    // 0-based lines of `mod checks`, `fn good` and `fn bad`.
    let (module, good, bad) = (5, 7, 12);
    pump(&mut app, "the icons", |a| icon(a, good) == Some(GutterIcon::RunTest) && icon(a, bad) == Some(GutterIcon::RunTest) && icon(a, module) == Some(GutterIcon::RunTest));

    // One test: only that test runs, and only its icon changes.
    app.run_test_at(good);
    pump(&mut app, "the passed icon", |a| icon(a, good) == Some(GutterIcon::TestPassed));
    pump(&mut app, "the end of the run", |a| a.runner.tests.as_ref().is_some_and(|t| !t.running));
    assert_eq!(icon(&mut app, bad), Some(GutterIcon::RunTest));
    let t = app.runner.tests.as_ref().unwrap();
    assert_eq!((t.passed, t.failed), (1, 0));

    // The whole module: the failure reaches the test and the module.
    app.run_test_at(module);
    pump(&mut app, "the failed icons", |a| icon(a, bad) == Some(GutterIcon::TestFailed) && icon(a, module) == Some(GutterIcon::TestFailed));
    pump(&mut app, "the end of the run", |a| a.runner.tests.as_ref().is_some_and(|t| !t.running));
    let t = app.runner.tests.as_ref().unwrap();
    assert_eq!((t.passed, t.failed), (1, 1));

    // The icon of `fn main` runs its binary.
    app.open_file(&root.join("src/main.rs"));
    pump(&mut app, "the icon of main", |a| icon(a, 0) == Some(GutterIcon::RunApp));
    app.run_binary_at(&egui::Context::default(), 0);
    pump(&mut app, "the program's output", |a| a.runner.runs.iter().any(|r| r.console.term.screen_lines().join("\n").contains("from the icon")));
    app.insight.stop();
}

/// Asks for completion at the end of `text` in the active document and returns the server's list.
fn complete_at_end(app: &mut CrabCraftApp, text: &str) -> rc_editor::Completion {
    let doc = app.docs.active_doc_mut().unwrap();
    let end = doc.editor.buffer.len_chars();
    doc.editor.replace(0, end, text);
    let caret = text.find('|').unwrap();
    doc.editor.replace(caret, caret + 1, "");
    doc.editor.selection = rc_text::Selection::point(caret);
    let i = app.docs.active.unwrap();
    app.insight.on_editor_events(&mut app.docs.list[i], &[rc_editor::EditorEvent::Changed]);
    let _ = app.insight.update(&mut app.docs, true);
    app.insight.complete(&mut app.docs.list[i], None);
    pump(app, "completion", |a| a.docs.active_doc().and_then(|d| d.editor.popups.completion.as_ref()).is_some_and(|c| !c.loading && !c.items.is_empty()));
    app.docs.active_doc().unwrap().editor.popups.completion.clone().unwrap()
}

#[test]
fn completion_imports_and_works_inside_derives_and_macros() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&root.join("src/main.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);

    let c = complete_at_end(&mut app, "#[derive(De|)]\nstruct A;\nfn main() {}\n");
    assert!(c.items.iter().any(|it| it.label.contains("Debug")), "{:?}", c.items.iter().map(|i| &i.label).take(20).collect::<Vec<_>>());

    let c = complete_at_end(&mut app, "use demo::Greeter;\nfn main() {\n    println!(\"{}\", Greeter.he|);\n}\n");
    assert!(c.items.iter().any(|it| it.label.starts_with("hello")), "{:?}", c.items.iter().map(|i| &i.label).take(20).collect::<Vec<_>>());

    let c = complete_at_end(&mut app, "fn main() {\n    let _m = HashMa|;\n}\n");
    let shown = c.filtered.iter().position(|&i| c.items[i].label.starts_with("HashMap")).expect("HashMap");
    let data = c.items[c.filtered[shown]].data;
    let i = app.docs.active.unwrap();
    rc_editor::accept_completion(&mut app.docs.list[i].editor, shown, false);
    app.insight.completion_accepted(&mut app.docs.list[i], data);
    pump(&mut app, "the import", |a| a.docs.active_doc().is_some_and(|d| d.editor.buffer.rope().to_string().contains("use std::collections::HashMap;")));
    let text = app.docs.list[i].editor.buffer.rope().to_string();
    assert!(text.contains("let _m = HashMap"), "{text}");
    assert_eq!(text.matches("use std::collections::HashMap;").count(), 1, "{text}");
    app.insight.stop();
}

#[test]
fn a_changed_manifest_reloads_the_targets_and_opening_it_opens_the_project() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    let manifest = root.join("Cargo.toml");
    app.open_path(&manifest);
    assert_eq!(app.workspace.as_ref().map(|w| w.ws.root.clone()), Some(root.clone()));
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    assert!(!app.runner.configs.iter().any(|c| c.name == "extra"));

    // Inside the open project it is a file to edit.
    app.open_path(&manifest);
    assert_eq!(app.docs.active_doc().map(|d| d.path.clone()), Some(manifest.clone()));

    write(&root.join("src/extra.rs"), "fn main() {}\n");
    let text = std::fs::read_to_string(&manifest).unwrap() + "\n[[bin]]\nname = \"extra\"\npath = \"src/extra.rs\"\n";
    write(&manifest, &text);
    app.report_fs_to_server(&[rc_core::watch::FsEvent::Changed(manifest.clone())]);
    pump(&mut app, "the new target", |a| a.runner.configs.iter().any(|c| c.name == "extra"));
}
