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

#[test]
fn a_failed_test_leads_to_its_code_and_runs_again_alone() {
    const LIB: &str = "pub fn one() -> u32 {\n    1\n}\n\n#[cfg(test)]\nmod checks {\n    #[test]\n    fn good() {\n        assert_eq!(super::one(), 1);\n    }\n\n    #[test]\n    fn bad() {\n        assert_eq!(super::one(), 2);\n    }\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/lib.rs"), LIB);
    write(&root.join("src/main.rs"), "fn main() {}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::RunTests);
    pump(&mut app, "the tests", |a| a.runner.tests.as_ref().is_some_and(|t| !t.running));
    assert_eq!(app.runner.tests.as_ref().map(|t| (t.passed, t.failed)), Some((1, 1)));

    // The panic's place opens from the result, on the assertion's line.
    let t = app.runner.tests.as_ref().unwrap();
    let frame = t.results["checks::bad"][0].stack_trace.first().cloned().expect("a frame");
    assert_eq!((frame.file.as_deref(), frame.line), (Some(root.join("src/lib.rs").as_path()), Some(14)));
    app.goto_test("checks::bad");
    let doc = app.docs.active_doc().unwrap();
    assert!(doc.path.ends_with("src/lib.rs"));
    assert_eq!(doc.editor.caret_line_col().0, 13);

    // A test that passed has no failure: it opens at its declaration.
    app.goto_test("checks::good");
    assert_eq!(app.docs.active_doc().unwrap().editor.caret_line_col().0, 7);

    // Only the failed test runs again.
    let outs = app.runner.rerun_failed();
    crate::ui::run_windows::handle_outputs(&mut app, outs);
    pump(&mut app, "the second run", |a| a.runner.tests.as_ref().is_some_and(|t| !t.running));
    let t = app.runner.tests.as_ref().unwrap();
    assert_eq!((t.passed, t.failed), (0, 1));
    assert!(t.results.keys().eq(["checks::bad"].iter().copied()), "{:?}", t.results.keys());
}

fn pump_debugger(app: &mut CrabCraftApp, what: &str) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(120) {
        let outs = app.debugger.update(&mut app.docs);
        app.handle_debug(outs);
        if let Some(rc_dap::SessionState::Failed(f)) = app.debugger.session.as_ref().map(|s| s.state()) {
            panic!("debugging failed waiting for {what}: {f}");
        }
        if app.debugger.stopped() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}: {}", app.debugger.state_label());
}

#[test]
fn a_test_stops_on_a_breakpoint_from_its_gutter_icon() {
    const LIB: &str = "pub fn one() -> u32 {\n    1\n}\n\n#[cfg(test)]\nmod checks {\n    #[test]\n    fn good() {\n        let value = super::one();\n        assert_eq!(value, 1);\n    }\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/lib.rs"), LIB);
    write(&root.join("src/main.rs"), "fn main() {}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/lib.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    pump(&mut app, "the runnables", |a| a.docs.active_doc().is_some_and(|d| a.insight.runnables.get(&d.editor.id).is_some_and(|(_, r)| r.iter().any(|x| x.line == 7))));

    // On `assert_eq!`, after `value` got its value.
    let line = 9;
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(line, rc_editor::BreakpointKind::Normal);
    app.debug_runnable_at(&egui::Context::default(), 7);
    pump_debugger(&mut app, "the breakpoint in the test");
    let doc = app.docs.active_doc().unwrap();
    assert!(doc.path.ends_with("src/lib.rs"));
    assert_eq!(doc.editor.execution_line, Some(line));
    app.debugger.session.as_mut().unwrap().stop();
    app.insight.stop();
}

fn locals(app: &mut CrabCraftApp) -> Vec<rc_dap::Variable> {
    let start = Instant::now();
    loop {
        let outs = app.debugger.update(&mut app.docs);
        app.handle_debug(outs);
        let s = app.debugger.session.as_mut().unwrap();
        if let Some(frame) = s.selected_frame().map(|f| f.id) {
            match s.scopes(frame).and_then(|sc| sc.first()).map(|sc| sc.variables_reference) {
                Some(r) => match s.variables(r) {
                    Some(v) => return v.to_vec(),
                    None => s.request_variables(r),
                },
                None => s.request_scopes(frame),
            }
        }
        assert!(start.elapsed() < Duration::from_secs(30), "no locals");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn local(app: &mut CrabCraftApp, name: &str) -> String {
    let vars = locals(app);
    vars.iter().find(|v| v.name == name).unwrap_or_else(|| panic!("no {name} in {vars:?}")).value.clone()
}

#[test]
fn the_debugger_steps_and_evaluates_a_rust_program() {
    use rc_editor::{Action, BreakpointKind};
    const MAIN: &str = "fn add(a: u32, b: u32) -> u32 {\n    let sum = a + b;\n    sum\n}\n\nfn main() {\n    let mut total = 0;\n    for i in 0..5 {\n        total = add(total, i);\n    }\n    println!(\"{total}\");\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), MAIN);
    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.rs");
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&main);
    let ctx = egui::Context::default();
    let line = |a: &CrabCraftApp| a.docs.active_doc().and_then(|d| d.editor.execution_line);

    // A conditional breakpoint stops only on the fourth round.
    let round = 8;
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(round, BreakpointKind::Conditional);
    app.debugger.conditions.insert((main.clone(), round), "i == 3".to_string());
    app.run_ide_action(&ctx, Action::Debug);
    pump_debugger(&mut app, "the conditional breakpoint");
    assert_eq!(line(&app), Some(round));
    assert_eq!(local(&mut app, "i"), "3");
    assert_eq!(local(&mut app, "total"), "3");

    // The values show on the lines that use them.
    let start = Instant::now();
    while !app.docs.active_doc().is_some_and(|d| d.editor.debug_values.contains_key(&round)) {
        let outs = app.debugger.update(&mut app.docs);
        app.handle_debug(outs);
        assert!(start.elapsed() < Duration::from_secs(30), "no inline values");
        std::thread::sleep(Duration::from_millis(20));
    }
    let shown = app.docs.active_doc().unwrap().editor.debug_values[&round].clone();
    assert!(shown.contains("total = 3") && shown.contains("i = 3"), "{shown}");

    // Watches and Evaluate.
    app.debugger.session.as_mut().unwrap().add_watch("total + 1");
    app.debugger.evaluate("i * 2".into());
    let start = Instant::now();
    loop {
        let outs = app.debugger.update(&mut app.docs);
        app.handle_debug(outs);
        let w = &app.debugger.session.as_ref().unwrap().watches()[0];
        if matches!(app.debugger.eval, Some((_, _, Some(_)))) && !matches!(w.value, rc_dap::WatchValue::Evaluating | rc_dap::WatchValue::NotEvaluated) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "no evaluation");
        std::thread::sleep(Duration::from_millis(20));
    }
    match &app.debugger.eval {
        Some((_, _, Some(Ok(v)))) => assert_eq!(v.result, "6"),
        other => panic!("{other:?}"),
    }
    match &app.debugger.session.as_ref().unwrap().watches()[0].value {
        rc_dap::WatchValue::Value(v) => assert_eq!(v.result, "4"),
        other => panic!("{other:?}"),
    }

    // Into the function, a step inside it, and back out.
    app.run_ide_action(&ctx, Action::StepInto);
    pump_debugger(&mut app, "the step into add");
    assert_eq!(line(&app), Some(1));
    assert_eq!(local(&mut app, "a"), "3");
    app.run_ide_action(&ctx, Action::StepOver);
    pump_debugger(&mut app, "the step over");
    // The compiler may give the last expression no line of its own.
    assert!(matches!(line(&app), Some(2 | 3)), "{:?}", line(&app));
    app.run_ide_action(&ctx, Action::StepOut);
    pump_debugger(&mut app, "the step out");
    assert_eq!(line(&app), Some(round));

    // Run to Cursor leaves the loop, past the conditional breakpoint.
    app.docs.active_doc_mut().unwrap().editor.goto(10, 0);
    app.run_ide_action(&ctx, Action::RunToCursor);
    pump_debugger(&mut app, "the cursor");
    assert_eq!(line(&app), Some(10));
    assert_eq!(local(&mut app, "total"), "10");
    app.debugger.session.as_mut().unwrap().stop();
}

#[test]
fn the_debugger_stops_a_panic_at_the_code_that_panicked() {
    const MAIN: &str = "fn main() {\n    let values: Vec<u32> = vec![1];\n    let index = values.len() + 4;\n    let far = values[index];\n    println!(\"{far}\");\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), MAIN);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/main.rs"));
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Debug);
    pump_debugger(&mut app, "the panic");
    let s = app.debugger.session.as_ref().unwrap();
    let info = s.stop_info().expect("stopped");
    assert_eq!(info.reason, rc_dap::StopReason::Panic, "{info:?}");
    assert_eq!(app.docs.active_doc().and_then(|d| d.editor.execution_line), Some(3));
    assert_eq!(local(&mut app, "index"), "5");
    app.debugger.session.as_mut().unwrap().stop();
}

#[test]
fn the_debugger_pauses_mutes_and_takes_console_commands() {
    use rc_editor::{Action, BreakpointKind};
    const MAIN: &str = "use std::time::Duration;\n\nfn main() {\n    let mut rounds = 0u64;\n    loop {\n        rounds += 1;\n        std::thread::sleep(Duration::from_millis(5));\n        if std::path::Path::new(\"stop\").exists() {\n            break;\n        }\n    }\n    println!(\"{rounds}\");\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), MAIN);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/main.rs"));
    let ctx = egui::Context::default();
    let wait = |app: &mut CrabCraftApp, what: &str, done: &dyn Fn(&CrabCraftApp) -> bool| {
        let start = Instant::now();
        while !done(app) {
            let outs = app.debugger.update(&mut app.docs);
            app.handle_debug(outs);
            assert!(start.elapsed() < Duration::from_secs(90), "timed out waiting for {what}: {}", app.debugger.state_label());
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    // A breakpoint in the loop.
    let round = 5;
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(round, BreakpointKind::Normal);
    app.run_ide_action(&ctx, Action::Debug);
    pump_debugger(&mut app, "the breakpoint");

    // The console reaches GDB itself, and answers.
    app.debugger.show_console = true;
    app.debugger.console_input = "print 40 + 2".to_string();
    app.debugger.send_console_command();
    let answered = |a: &CrabCraftApp| a.debugger.console.as_ref().is_some_and(|c| c.term.screen_lines().join("\n").contains("= 42"));
    wait(&mut app, "the console's answer", &answered);

    // Muted, the breakpoint no longer stops it; Pause does.
    app.debugger.muted = true;
    let _ = app.debugger.update(&mut app.docs);
    app.run_ide_action(&ctx, Action::Resume);
    wait(&mut app, "the program to run", &|a| !a.debugger.stopped());
    std::thread::sleep(Duration::from_millis(400));
    let _ = app.debugger.update(&mut app.docs);
    assert!(!app.debugger.stopped(), "a muted breakpoint stopped the program");
    app.debugger.session.as_mut().unwrap().pause();
    pump_debugger(&mut app, "the pause");
    assert_eq!(app.debugger.session.as_ref().unwrap().stop_info().unwrap().reason, rc_dap::StopReason::Pause);

    // Unmuted, it stops again.
    app.debugger.muted = false;
    let _ = app.debugger.update(&mut app.docs);
    app.run_ide_action(&ctx, Action::Resume);
    wait(&mut app, "the program to run", &|a| !a.debugger.stopped());
    pump_debugger(&mut app, "the breakpoint again");
    assert_eq!(app.docs.active_doc().and_then(|d| d.editor.execution_line), Some(round));

    // Restart: a new session stops at the same place, with `rounds` from the start.
    app.restart_debugging();
    wait(&mut app, "the old session to go", &|a| !a.debugger.stopped());
    pump_debugger(&mut app, "the breakpoint after the restart");
    assert_eq!(local(&mut app, "rounds"), "0");
    app.debugger.stop();
}

#[test]
fn the_debugger_attaches_to_a_running_program() {
    use rc_editor::BreakpointKind;
    // Yama allows a debugger that is not the parent only when the program says so.
    const MAIN: &str = "extern \"C\" {\n    fn prctl(option: i32, arg: u64, ...) -> i32;\n}\n\nfn main() {\n    unsafe { prctl(0x59616d61, u64::MAX) };\n    let mut rounds = 0u64;\n    loop {\n        rounds += 1;\n        std::thread::sleep(std::time::Duration::from_millis(5));\n        if rounds == u64::MAX {\n            break;\n        }\n    }\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), MAIN);
    let root = root.canonicalize().unwrap();
    let built = std::process::Command::new("cargo").arg("build").arg("-q").current_dir(&root).status().unwrap();
    assert!(built.success());
    struct Running(std::process::Child);
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let program = Running(std::process::Command::new(root.join("target/debug/demo")).spawn().unwrap());
    std::thread::sleep(Duration::from_millis(300));
    let found = rc_core::processes::list().into_iter().find(|p| p.pid == program.0.id());
    let pid = found.expect("the program in the list").pid;

    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/main.rs"));
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(8, BreakpointKind::Normal);
    let outs = app.debugger.attach_gdb(pid, "demo", &app.docs);
    app.handle_debug(outs);
    pump_debugger(&mut app, "the breakpoint after attaching");
    assert_eq!(app.docs.active_doc().and_then(|d| d.editor.execution_line), Some(8));
    let rounds: u64 = local(&mut app, "rounds").parse().expect("a number");
    assert!(rounds > 0, "it was already running");
    app.debugger.stop();
    drop(program);
}

#[test]
fn a_rust_run_configuration_takes_arguments_and_environment() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/main.rs"), "fn main() {\n    let args: Vec<String> = std::env::args().skip(1).collect();\n    println!(\"[{}] {}\", args.join(\",\"), std::env::var(\"CRAB\").unwrap_or_default());\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    let name = app.runner.selected_config().unwrap().name.clone();
    app.edit_run_config(Some(name.clone()));
    assert!(matches!(&app.dialog, Some(crate::ui::dialogs::Dialog::RunOptions { project, .. }) if project.ends_with("Cargo.toml")));
    let mut options = rc_core::session::RunOptions { args: "one \"two words\"".to_string(), ..Default::default() };
    options.env = "CRAB=rust".to_string();
    app.runner.options.insert(name, options);
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Run);
    pump(&mut app, "the output", |a| a.runner.runs.iter().any(|r| r.console.term.screen_lines().join("\n").contains("[one,two words] rust")));
}

#[test]
fn reformat_code_runs_rustfmt() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/main.rs"), "fn main(){\nprintln!(\"x\");\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&root.join("src/main.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    let i = app.docs.active.unwrap();
    app.insight.format(&app.docs.list[i]);
    pump(&mut app, "the formatted text", |a| a.docs.active_doc().is_some_and(|d| d.editor.buffer.rope().to_string().contains("fn main() {\n    println!")));
    app.insight.stop();
}

/// Puts `text` in the active document with the range between the `|` marks selected (one mark:
/// the caret), asks for its actions and applies the one whose title has `title`.
fn apply_assist(app: &mut CrabCraftApp, text: &str, title: &str, expected: &str) {
    let first = text.find('|').unwrap();
    let plain = text.replace('|', "");
    let second = text[first + 1..].find('|').map(|i| first + i);
    let doc = app.docs.active_doc_mut().unwrap();
    let end = doc.editor.buffer.len_chars();
    doc.editor.replace(0, end, &plain);
    doc.editor.selection = match second {
        Some(last) => rc_text::Selection::single(rc_text::Range::new(first, last)),
        None => rc_text::Selection::point(first),
    };
    let i = app.docs.active.unwrap();
    app.insight.on_editor_events(&mut app.docs.list[i], &[rc_editor::EditorEvent::Changed]);
    pump(app, "the analysis of the new text", |a| a.insight.status == InsightStatus::Ready);
    app.insight.code_actions(&app.docs.list[i]);
    let actions = output(app, "the code actions", |o| match o {
        crate::insight::InsightOutput::Actions(a) => Some(a.clone()),
        _ => None,
    });
    let name = |a: &rc_lsp::lsp_types::CodeActionOrCommand| match a {
        rc_lsp::lsp_types::CodeActionOrCommand::Command(c) => c.title.clone(),
        rc_lsp::lsp_types::CodeActionOrCommand::CodeAction(c) => c.title.clone(),
    };
    let action = actions.iter().find(|a| name(a).contains(title)).unwrap_or_else(|| panic!("no {title} in {:?}", actions.iter().map(name).collect::<Vec<_>>())).clone();
    let mut out = Vec::new();
    app.insight.run_code_action(&mut app.docs, action, &mut out);
    let start = Instant::now();
    while !app.docs.active_doc().is_some_and(|d| d.editor.buffer.rope().to_string().contains(expected)) {
        let _ = app.insight.update(&mut app.docs, true);
        assert!(start.elapsed() < Duration::from_secs(30), "{title}: {}", app.docs.active_doc().unwrap().editor.buffer.rope());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn refactorings_of_the_language_server() {
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

    apply_assist(&mut app, "fn main() {\n    let x = (|1 + 2|) * 3;\n    drop(x);\n}\n", "Extract into variable", "= 1 + 2;");
    apply_assist(&mut app, "fn main() {\n    |let y = 1 + 2;\n    drop(y);|\n}\n", "Extract into function", "fn fun_name()");
    apply_assist(&mut app, "fn main() {\n    let y = 1;\n    let z = |y;\n    drop(z);\n}\n", "Inline", "let z = 1;");
    apply_assist(&mut app, "struct A;\nimpl std::fmt::Display for |A {}\nfn main() {}\n", "Implement missing members", "fn fmt(");
    apply_assist(&mut app, "struct A {\n    a: u32,|\n}\nfn main() {}\n", "Generate `new`", "fn new(");
    apply_assist(&mut app, "use std::collections::|HashMap;\nfn main() {}\n", "Remove", "fn main");

    let c = complete_at_end(&mut app, "fn main() {\n    let x = true;\n    x.i|;\n}\n");
    assert!(c.items.iter().any(|it| it.label.starts_with("if")), "{:?}", c.items.iter().map(|i| &i.label).take(30).collect::<Vec<_>>());
    app.insight.stop();
}

#[test]
fn the_cargo_window_runs_targets_and_typed_commands() {
    use rc_cargo::{Target, TargetKind};
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("examples/sample.rs"), "fn main() {\n    println!(\"from the example\");\n}\n");
    write(&root.join("tests/fails.rs"), "#[test]\nfn no() {\n    panic!();\n}\n");
    write(&root.join("build.rs"), "fn main() {}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    let ctx = egui::Context::default();
    let target = |a: &CrabCraftApp, kind| -> Target { a.runner.cargo_workspace.as_ref().unwrap().packages[0].targets.iter().find(|t| t.kind == kind).unwrap().clone() };
    let finished = |a: &mut CrabCraftApp| a.runner.build.as_ref().is_some_and(|b| b.result.is_some() || b.failed.is_some());
    let console = |a: &CrabCraftApp| a.runner.build.as_ref().unwrap().console.term.screen_lines().join("\n");

    // Nothing is typed: nothing starts.
    app.run_cargo_command("   ");
    assert!(app.runner.build.is_none());

    // What Cargo cannot run directly is refused.
    let script = target(&app, TargetKind::Other);
    app.run_cargo_target(&ctx, "demo", &script);
    assert!(app.runner.build.is_none());
    assert!(app.notices.iter().any(|n| n.text == rc_kit::i18n::tr("Cargo cannot run this target")));

    // An example goes through `cargo run`.
    let example = target(&app, TargetKind::Example);
    app.run_cargo_target(&ctx, "demo", &example);
    pump(&mut app, "the example", finished);
    let build = app.runner.build.as_ref().unwrap();
    assert!(build.result.as_ref().unwrap().success, "{}", console(&app));
    pump(&mut app, "the output", |a| console(a).contains("from the example"));

    // A failing command reports its exit code, not a compile error.
    let failing = target(&app, TargetKind::Test);
    app.run_cargo_target(&ctx, "demo", &failing);
    pump(&mut app, "the failing test", finished);
    let build = app.runner.build.as_ref().unwrap();
    let result = build.result.as_ref().unwrap();
    assert!(!result.success);
    assert_eq!(crate::runner::command_summary(&build.title, result), "cargo test failed with exit code 101");
    pump(&mut app, "the test result", |a| console(a).contains("test result: FAILED"));

    // A typed command runs in the workspace, with or without `cargo`.
    app.run_cargo_command("cargo tree --depth 0");
    pump(&mut app, "the typed command", finished);
    assert!(app.runner.build.as_ref().unwrap().result.as_ref().unwrap().success, "{}", console(&app));
    pump(&mut app, "the output", |a| console(a).contains("demo v0.1.0"));
    app.run_cargo_command("no-such-command");
    pump(&mut app, "the unknown command", finished);
    assert!(!app.runner.build.as_ref().unwrap().result.as_ref().unwrap().success);

    // A second command waits for the first.
    app.run_cargo_command("tree");
    app.run_cargo_command("metadata");
    assert_eq!(app.runner.build.as_ref().unwrap().title, "cargo tree");
    pump(&mut app, "the command", finished);

    // A binary uses its run configuration, in the Run window.
    let bin = target(&app, TargetKind::Bin);
    app.run_cargo_target(&ctx, "demo", &bin);
    pump(&mut app, "the binary", |a| a.runner.runs.first().is_some_and(|r| !r.running()));

    // New targets appear after a reload.
    write(&root.join("examples/other.rs"), "fn main() {}\n");
    app.reload_cargo();
    pump(&mut app, "the reload", |a| a.runner.cargo_workspace.as_ref().unwrap().packages[0].targets.iter().any(|t| t.name == "other"));
}

#[test]
fn clippy_replaces_cargo_check_and_rustfmt_runs_on_save() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let text = "fn main(){\n    let v = vec![1];\n    if v.len() == 0 {\n        println!(\"empty\");\n    }\n}\n";
    write(&root.join("src/main.rs"), text);
    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.rs");
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.settings.format_on_save = true;
    app.open_workspace(&root);
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    let lint = |a: &mut CrabCraftApp| a.docs.active_doc().is_some_and(|d| d.editor.diagnostics.iter().any(|x| x.code.as_deref() == Some("len_zero")));
    assert!(!lint(&mut app), "cargo check has no such lint");

    app.toggle_linter();
    assert!(app.settings.use_clippy);
    let doc = app.docs.active_doc_mut().unwrap();
    doc.editor.replace(0, 0, "\n");
    let i = app.docs.active.unwrap();
    app.insight.on_editor_events(&mut app.docs.list[i], &[rc_editor::EditorEvent::Changed]);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        let _ = app.insight.update(&mut app.docs, true);
        std::thread::sleep(Duration::from_millis(20));
    }
    app.save_all_docs();
    pump(&mut app, "rustfmt to change the file on disk", |_| std::fs::read_to_string(&main).is_ok_and(|t| t.starts_with("fn main() {\n")));
    pump(&mut app, "the lint of Clippy", |a| lint(a));
    app.insight.stop();
}
