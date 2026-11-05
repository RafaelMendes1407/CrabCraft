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
        app.poll_rustfmt();
        app.poll_disk_watch();
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
    for _ in 0..10 { app.poll_cargo_workspace(); }
    assert!(app.runner.cargo_workspace.is_none());
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
    app.trust_project_tools_for_test();
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
    let mut symbols = app.insight.workspace_symbols("Greeter", false, false).expect("a request");
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
    app.insight.rename(&app.docs.list[i], pos, "greet", None);
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

    // Its hover offers rustc's explanation of E0308 (I-24), which opens in a read-only tab.
    let pos = saved.find("\"text\"").unwrap() + 1;
    let i = app.docs.active.unwrap();
    app.insight.on_editor_events(&mut app.docs.list[i], &[rc_editor::EditorEvent::HoverAt { pos, screen: egui::pos2(300.0, 200.0) }]);
    pump(&mut app, "the hover of the error", |a| a.docs.active_doc().and_then(|d| d.editor.popups.hover.as_ref()).is_some_and(|h| h.explain.as_deref() == Some("E0308")));
    app.explain_error("E0308");
    pump(&mut app, "the explanation", |a| {
        a.poll_explanation();
        a.docs.active_doc().is_some_and(|d| d.path.ends_with("E0308.md") && d.editor.buffer.rope().to_string().contains("Expected type did not match"))
    });
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

#[test]
fn a_gutter_icon_runs_a_benchmark() {
    use rc_editor::GutterIcon;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bench]]\nname = \"plain\"\nharness = false\n\n[workspace]\n");
    write(&root.join("benches/plain.rs"), "fn main() {\n    println!(\"measured here\");\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("benches/plain.rs"));
    pump(&mut app, "the benchmark's icon", |a| {
        a.update_test_markers();
        a.docs.active_doc().is_some_and(|d| d.editor.gutter_icons.get(&0) == Some(&GutterIcon::RunApp))
    });
    app.run_binary_at(&egui::Context::default(), 0);
    pump(&mut app, "the benchmark's output", |a| a.runner.build.as_ref().is_some_and(|b| b.console.term.screen_lines().join("\n").contains("measured here")));
    app.insight.stop();
}

#[test]
fn a_compiler_suggestion_is_applied_only_to_the_text_it_was_made_for() {
    const MAIN: &str = "fn main() {\n    let mut total = 1;\n    println!(\"{total}\");\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/main.rs"), MAIN);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    let build = |app: &mut CrabCraftApp| {
        app.run_ide_action(&egui::Context::default(), rc_editor::Action::Build);
        pump(app, "the build", |a| a.runner.build.as_ref().is_some_and(|b| b.result.is_some()));
        let list = &app.runner.build.as_ref().unwrap().diagnostics;
        list.iter().find_map(|d| Some((d.file.clone()?, d.fix.clone()?))).expect("a fix")
    };
    let (file, fix) = build(&mut app);
    assert_eq!(file, root.join("src/main.rs"));

    // A file that changed keeps its text.
    let changed = format!("// a comment\n{MAIN}");
    write(&file, &changed);
    app.apply_build_fix(&file, &fix);
    assert!(app.docs.index_of(&file).is_none());
    write(&file, MAIN);

    // Unsaved text in the editor is not what the compiler read either.
    app.open_file(&file);
    app.docs.active_doc_mut().unwrap().editor.replace(0, 0, "\n");
    app.apply_build_fix(&file, &fix);
    assert_eq!(app.docs.active_doc().unwrap().editor.buffer.rope().to_string(), format!("\n{MAIN}"));
    app.docs.active_doc_mut().unwrap().editor.replace(0, 1, "");
    app.save_all_docs();

    let (file, fix) = build(&mut app);
    app.apply_build_fix(&file, &fix);
    let doc = app.docs.active_doc().unwrap();
    assert_eq!(doc.editor.buffer.rope().to_string(), MAIN.replace("let mut total", "let total"));
    assert!(doc.is_modified(), "the user decides when to save");
    assert!(app.runner.build.as_ref().unwrap().diagnostics.iter().all(|d| d.fix.is_none()));
}

#[test]
fn a_file_outside_the_module_tree_can_be_declared() {
    let unlinked = |a: &CrabCraftApp, file: &str| a.docs.list.iter().any(|d| d.path.ends_with(file) && a.insight.unlinked.contains(&d.editor.id));
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/lone.rs"), "pub fn lone() {}\n");
    write(&root.join("src/open.rs"), "pub fn open() {}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/lone.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    pump(&mut app, "the unlinked file", |a| unlinked(a, "src/lone.rs"));

    // A declared file is not reported.
    app.open_file(&root.join("src/main.rs"));
    app.open_file(&root.join("src/lib.rs"));
    let settled = Instant::now();
    pump(&mut app, "the other files", |_| settled.elapsed() > Duration::from_secs(3));
    assert!(!unlinked(&app, "src/main.rs") && !unlinked(&app, "src/lib.rs"));
    let i = app.docs.index_of(&root.join("src/lib.rs")).unwrap();
    app.close_tab(i);

    // Closed parent: written to disk.
    app.declare_module(&root.join("src/lib.rs"), "lone", false);
    assert!(std::fs::read_to_string(root.join("src/lib.rs")).unwrap().starts_with("mod lone;\n"));

    // Open parent: its unsaved text stays and nothing is written yet.
    app.open_file(&root.join("src/lib.rs"));
    app.open_file(&root.join("src/open.rs"));
    let saved = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    app.declare_module(&root.join("src/lib.rs"), "open", true);
    assert_eq!(std::fs::read_to_string(root.join("src/lib.rs")).unwrap(), saved);
    let i = app.docs.index_of(&root.join("src/lib.rs")).unwrap();
    assert!(app.docs.list[i].editor.buffer.rope().to_string().starts_with("mod lone;\npub mod open;\n"));

    // Declaring twice changes nothing.
    app.declare_module(&root.join("src/lib.rs"), "open", true);
    assert_eq!(app.docs.list[i].editor.buffer.rope().to_string().matches("mod open;").count(), 1);
    app.insight.stop();
}

#[test]
fn run_anything_runs_commands_and_configurations() {
    use crate::run_anything::Item;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    let ctx = egui::Context::default();

    app.run_anything(&ctx, Item::Command("cargo tree --depth 0".to_string()));
    pump(&mut app, "the command", |a| a.runner.build.as_ref().is_some_and(|b| b.console.term.screen_lines().join("\n").contains("demo v0.1.0")));
    assert_eq!(app.settings.run_anything_history, ["cargo tree --depth 0"]);

    pump(&mut app, "the end of the command", |a| a.runner.build.as_ref().is_some_and(|b| b.handle.is_none()));
    app.run_anything(&ctx, Item::Config(0));
    pump(&mut app, "the program", |a| a.runner.runs.first().is_some_and(|r| r.console.term.screen_lines().join("\n").contains("hello")));
    assert_eq!(app.settings.run_anything_history.len(), 1, "configurations are not commands");
}

#[test]
fn optimize_imports_removes_the_unused_and_merges_the_rest() {
    const MAIN: &str = "use std::collections::HashMap;\nuse std::collections::HashSet;\nuse std::fmt;\n\nfn main() {\n    let _a: HashMap<u32, u32> = HashMap::new();\n    let _b: HashSet<u32> = HashSet::new();\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/main.rs"), MAIN);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/main.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    pump(&mut app, "the symbols", |a| a.docs.active_doc().is_some_and(|d| a.insight.runnables.contains_key(&d.editor.id)));
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::OptimizeImports);
    pump(&mut app, "the imports", |a| {
        let text = a.docs.active_doc().unwrap().editor.buffer.rope().to_string();
        !text.contains("fmt") && text.contains("use std::collections::{HashMap, HashSet};")
    });
    app.insight.stop();
}

#[test]
fn bookmarks_follow_their_lines_and_outlive_the_tab() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.rs");
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.open_file(&main);
    let line = MAIN.lines().position(|l| l.contains("println")).unwrap();
    let doc = app.docs.active_doc_mut().unwrap();
    doc.editor.toggle_bookmark(line);

    // A line added above moves it.
    doc.editor.replace(0, 0, "// new\n");
    let moved = line + 1;
    assert_eq!(app.bookmark_list().iter().map(|m| (m.line, m.preview.contains("println"))).collect::<Vec<_>>(), [(moved, true)]);
    app.docs.active_doc_mut().unwrap().editor.replace(0, 7, "");

    // Closing the tab keeps it, and it comes back with the file.
    let i = app.docs.index_of(&main).unwrap();
    app.close_tab(i);
    assert!(app.docs.index_of(&main).is_none());
    assert_eq!(app.bookmark_list().len(), 1);
    app.open_file(&main);
    assert!(app.docs.active_doc().unwrap().editor.bookmarks.contains(&line));

    // The list removes it, open or closed.
    app.docs.remove_bookmark(&main, line);
    assert!(app.bookmark_list().is_empty());
}

#[test]
fn external_documentation_finds_the_page_of_an_item() {
    use crate::insight::InsightOutput;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let text = "use std::collections::HashMap;\nfn main() {\n    let _a: HashMap<u32, u32> = HashMap::new();\n}\n";
    write(&root.join("src/main.rs"), text);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&root.join("src/main.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    pump(&mut app, "the indexing", |a| a.docs.active_doc().is_some_and(|d| a.insight.runnables.contains_key(&d.editor.id)));
    let i = app.docs.active.unwrap();
    app.insight.external_docs(&app.docs.list[i], text.find("HashMap<").unwrap() + 1);
    let url = output(&mut app, "the page", |o| match o {
        InsightOutput::OpenUrl(url) => Some(url.clone()),
        InsightOutput::Message(m) => panic!("{m}"),
        _ => None,
    });
    assert!(url.starts_with("https://") && url.contains("HashMap"), "{url}");
    app.insight.stop();
}

#[test]
fn a_macro_call_expands_recursively_into_a_read_only_tab() {
    use crate::insight::InsightOutput;
    const SOURCE: &str = "macro_rules! twice {\n    ($e:expr) => { double!($e) + double!($e) };\n}\nmacro_rules! double {\n    ($e:expr) => { $e * 2 };\n}\nfn main() {\n    let _n = twice!(21);\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/main.rs"), SOURCE);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&root.join("src/main.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    let i = app.docs.active.unwrap();
    // On `twice!`: both levels expanded. Asked again until the server has indexed the file.
    let start = Instant::now();
    let (name, text) = loop {
        app.insight.expand_macro(&app.docs.list[i], SOURCE.find("twice!(21)").unwrap() + 2);
        let reply = output(&mut app, "the expansion", |o| match o {
            InsightOutput::MacroExpansion { name, text } => Some(Some((name.clone(), text.clone()))),
            InsightOutput::Message(_) => Some(None),
            _ => None,
        });
        match reply {
            Some(found) => break found,
            None if start.elapsed() < Duration::from_secs(120) => std::thread::sleep(Duration::from_millis(500)),
            None => panic!("no expansion"),
        }
    };
    // Not on a macro call: said so.
    app.insight.expand_macro(&app.docs.list[i], SOURCE.find("fn main").unwrap() + 1);
    output(&mut app, "the empty answer", |o| matches!(o, InsightOutput::Message(_)).then_some(()));
    assert_eq!(name, "twice");
    assert!(text.contains("21*2") || text.contains("21 * 2"), "{text}");
    assert!(!text.contains("double!"), "{text}");
    app.show_macro_expansion(&name, &text);
    let d = app.docs.active_doc().unwrap();
    assert_eq!(d.name(), "twice.rs");
    assert!(d.editor.read_only && d.generated && !crate::insight::has_insight(d));
    assert!(d.editor.buffer.rope().to_string().contains(&text), "{:?} / {text:?}", d.editor.buffer.rope().to_string());
    app.insight.stop();
}

#[test]
fn the_type_hierarchy_lists_implementations() {
    const SOURCE: &str = "trait Speak {\n    fn speak(&self);\n}\nstruct A;\nstruct B;\nimpl Speak for A {\n    fn speak(&self) {}\n}\nimpl Speak for B {\n    fn speak(&self) {}\n}\nimpl A {\n    fn own(&self) {}\n}\ntrait Lonely {}\nfn main() {}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/main.rs"), SOURCE);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&root.join("src/main.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    pump(&mut app, "the indexing", |a| a.docs.active_doc().is_some_and(|d| a.insight.runnables.contains_key(&d.editor.id)));
    let i = app.docs.active.unwrap();
    let ask = |app: &mut CrabCraftApp, word: &str| {
        app.insight.call_tree = None;
        app.insight.type_hierarchy(&app.docs.list[i], SOURCE.find(word).unwrap() + word.len() - 1);
        pump(app, "the hierarchy", |a| a.insight.call_tree.is_some());
        let tree = app.insight.call_tree.as_ref().unwrap();
        let mut names: Vec<String> = tree.rows().iter().map(|(_, n)| n.name.clone()).collect();
        names[1..].sort();
        names
    };
    assert_eq!(ask(&mut app, "trait Speak"), ["Speak", "impl Speak for A", "impl Speak for B"]);
    assert_eq!(ask(&mut app, "struct A"), ["A", "impl A", "impl Speak for A"]);
    assert_eq!(ask(&mut app, "trait Lonely"), ["Lonely"]);
    app.insight.stop();
}

#[test]
fn the_action_popup_keeps_its_keys_from_the_editor() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), "fn main() {\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.open_file(&root.join("src/main.rs"));
    app.focus_editor = true;
    let mut h = crate::screens::harness(app, egui::vec2(1200.0, 800.0));
    h.run_steps(3);
    let command = |title: &str| {
        rc_lsp::lsp_types::CodeActionOrCommand::Command(rc_lsp::lsp_types::Command {
            title: title.to_string(),
            command: crate::insight::RENAME_COMMAND.to_string(),
            arguments: None,
        })
    };
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::CodeActions { list: vec![command("One"), command("Two")], selected: 0 });
    h.run_steps(2);
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(2);
    assert!(matches!(h.state().dialog, Some(crate::ui::dialogs::Dialog::CodeActions { selected: 1, .. })));
    assert_eq!(h.state().docs.active_doc().unwrap().editor.caret_line_col().0, 0, "the caret moved");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string(), "fn main() {\n}\n");

    // Select In: Alt+F1 offers the targets, and Enter picks the Explorer.
    let ctx = h.ctx.clone();
    h.state_mut().run_ide_action(&ctx, rc_editor::Action::SelectIn);
    h.run_steps(2);
    assert!(matches!(&h.state().dialog, Some(crate::ui::dialogs::Dialog::CodeActions { list, .. }) if list.len() == 2));
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert!(h.state().explorer.expanded.iter().any(|e| e.contains("src")), "{:?}", h.state().explorer.expanded);
}

#[test]
fn logpoints_print_and_a_breakpoint_can_remove_itself() {
    use rc_editor::{Action, BreakpointKind};
    if rc_dap::gdb::locate().is_none() {
        return;
    }
    const MAIN: &str = "fn main() {\n    let mut total = 0;\n    for i in 0..3 {\n        total += i;\n        std::hint::black_box(total);\n    }\n    println!(\"{total}\");\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), MAIN);
    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.rs");
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&main);
    let ctx = egui::Context::default();

    let (log, once) = (3, 4);
    let editor = &mut app.docs.active_doc_mut().unwrap().editor;
    editor.breakpoints.insert(log, BreakpointKind::Log);
    editor.breakpoints.insert(once, BreakpointKind::Normal);
    app.debugger.logs.insert((main.clone(), log), "seen i={i}".to_string());
    app.debugger.once.insert((main.clone(), once));

    // The session keeps both.
    let saved = app.session();
    assert_eq!(saved.logpoints[&main], [(log, "seen i={i}".to_string())]);
    assert_eq!(saved.remove_once_hit[&main], [once]);

    // The logpoint does not stop; the other breakpoint stops once and goes away.
    app.run_ide_action(&ctx, Action::Debug);
    pump_debugger(&mut app, "the breakpoint");
    assert_eq!(app.docs.active_doc().and_then(|d| d.editor.execution_line), Some(once));
    let editor = &app.docs.active_doc().unwrap().editor;
    assert!(!editor.breakpoints.contains_key(&once), "the breakpoint stayed");
    assert_eq!(editor.breakpoints.get(&log), Some(&BreakpointKind::Log));
    assert!(!app.debugger.once.contains(&(main.clone(), once)));

    app.run_ide_action(&ctx, Action::Resume);
    let start = Instant::now();
    while app.debugger.active() {
        let outs = app.debugger.update(&mut app.docs);
        app.handle_debug(outs);
        assert!(!app.debugger.stopped(), "stopped again");
        assert!(start.elapsed() < Duration::from_secs(30), "the program did not end");
        std::thread::sleep(Duration::from_millis(20));
    }
    let console = app.debugger.console.as_ref().unwrap().term.screen_lines().join("\n");
    for i in 0..3 {
        assert!(console.contains(&format!("seen i={i}")), "{console}");
    }
}

#[test]
fn integers_switch_to_hexadecimal_while_paused() {
    use rc_editor::{Action, BreakpointKind};
    if rc_dap::gdb::locate().is_none() {
        return;
    }
    const MAIN: &str = "fn main() {\n    let n: u32 = 255;\n    let m: i32 = -1;\n    let f: f64 = 2.5;\n    let s = \"hi\";\n    std::hint::black_box((n, m, f, s));\n    println!(\"{n} {m} {f} {s}\");\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), MAIN);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/main.rs"));
    let ctx = egui::Context::default();
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(6, BreakpointKind::Normal);
    app.run_ide_action(&ctx, Action::Debug);
    pump_debugger(&mut app, "the breakpoint");
    assert_eq!(local(&mut app, "n"), "255");
    let pid = app.debugger.session.as_ref().and_then(|s| s.debuggee_pid());
    let w = app.debugger.session.as_mut().unwrap().add_watch("n + 1");

    let settle = |app: &mut CrabCraftApp, what: &str, done: &dyn Fn(&CrabCraftApp) -> bool| {
        let start = Instant::now();
        while !done(app) {
            let outs = app.debugger.update(&mut app.docs);
            app.handle_debug(outs);
            // The Variables view asks for them while it is shown.
            locals(app);
            assert!(start.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let inline = |a: &CrabCraftApp, line: usize| a.docs.active_doc().and_then(|d| d.editor.debug_values.get(&line).cloned()).unwrap_or_default();
    let watch = |a: &CrabCraftApp| match a.debugger.session.as_ref().and_then(|s| s.watches().iter().find(|x| x.id == w)).map(|x| x.value.clone()) {
        Some(rc_dap::WatchValue::Value(v)) => v.result,
        _ => String::new(),
    };
    settle(&mut app, "decimal values", &|a| inline(a, 1) == "n = 255" && watch(a) == "256");

    // GDB prints in hexadecimal itself (`ValueFormat`, its `output-radix 16`).
    assert!(app.debugger.session.as_ref().unwrap().capabilities().supports_value_formatting_options);
    app.run_ide_action(&ctx, Action::ToggleHexValues);
    assert!(app.settings.debugger_hex);
    settle(&mut app, "hexadecimal values", &|a| inline(a, 1) == "n = 0xff" && watch(a) == "0x100");
    assert_eq!(local(&mut app, "n"), "0xff");
    assert_eq!(local(&mut app, "m"), "0xffffffff");
    assert_eq!(local(&mut app, "f"), "2.5");
    assert!(local(&mut app, "s").contains("\"hi\""), "{}", local(&mut app, "s"));
    assert_eq!(inline(&app, 2), "m = 0xffffffff");
    app.debugger.evaluate("n".into());
    settle(&mut app, "the evaluation", &|a| matches!(a.debugger.eval, Some((_, _, Some(_)))));
    match &app.debugger.eval {
        Some((_, _, Some(Ok(v)))) => assert_eq!(v.result, "0xff"),
        other => panic!("{other:?}"),
    }

    // And back, in the same session.
    app.run_ide_action(&ctx, Action::ToggleHexValues);
    assert!(!app.settings.debugger_hex);
    settle(&mut app, "decimal values again", &|a| inline(a, 1) == "n = 255" && watch(a) == "256");
    assert_eq!(local(&mut app, "m"), "-1");
    assert_eq!(app.debugger.session.as_ref().and_then(|s| s.debuggee_pid()), pid);
    assert!(app.debugger.stopped());
    app.debugger.stop();
}

const SET_VALUE_MAIN: &str = "fn main() {\n    let mut n: u32 = 255;\n    std::hint::black_box(&mut n);\n    let doubled = n * 2;\n    std::hint::black_box(doubled);\n    println!(\"n={n} doubled={doubled}\");\n}\n";

/// An app paused under GDB at `let doubled = n * 2;` of [`SET_VALUE_MAIN`].
fn paused_before_doubling(dir: &Path) -> CrabCraftApp {
    use rc_editor::{Action, BreakpointKind};
    let root = dir.join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), SET_VALUE_MAIN);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir);
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/main.rs"));
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(3, BreakpointKind::Normal);
    app.run_ide_action(&egui::Context::default(), Action::Debug);
    pump_debugger(&mut app, "the breakpoint");
    app
}

/// Opens Set Value on the local `name`, as its context menu does, with `text` typed in it.
fn edit_local(app: &mut CrabCraftApp, name: &str, text: &str) {
    let vars = locals(app);
    let s = app.debugger.session.as_ref().unwrap();
    let scope = s.selected_frame().and_then(|f| s.scopes(f.id)).and_then(|sc| sc.first()).cloned().unwrap();
    let v = vars.iter().find(|v| v.name == name).unwrap();
    let target = crate::debugger::SetTarget::Variable { parent: scope.variables_reference, name: v.name.clone(), expression: Some(v.evaluate_name.clone().unwrap_or_else(|| v.name.clone())) };
    assert!(target.supported(s.capabilities()));
    app.debugger.start_edit(format!("s:{}/{}", scope.name, v.name), target, &v.value);
    assert_eq!(app.debugger.edit.as_ref().map(|e| e.text.as_str()), Some(v.value.as_str()), "prefilled with the value");
    app.debugger.edit.as_mut().unwrap().text = text.into();
}

#[test]
fn a_variable_is_set_while_paused_and_the_program_sees_it() {
    use rc_editor::Action;
    if rc_dap::gdb::locate().is_none() {
        return;
    }
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let mut app = paused_before_doubling(dir.path());
    let ctx = egui::Context::default();
    // GDB 15 has both requests.
    let caps = app.debugger.session.as_ref().unwrap().capabilities().clone();
    eprintln!("GDB supportsSetVariable={} supportsSetExpression={}", caps.supports_set_variable, caps.supports_set_expression);
    assert!(caps.supports_set_variable && caps.supports_set_expression);
    assert_eq!(local(&mut app, "n"), "255");
    let w = app.debugger.session.as_mut().unwrap().add_watch("n + 1");

    let settle = |app: &mut CrabCraftApp, what: &str, done: &dyn Fn(&CrabCraftApp) -> bool| {
        let start = Instant::now();
        while !done(app) {
            let outs = app.debugger.update(&mut app.docs);
            app.handle_debug(outs);
            locals(app);
            assert!(start.elapsed() < Duration::from_secs(30), "timed out waiting for {what}: {:?}", app.debugger.edit);
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let inline = |a: &CrabCraftApp, line: usize| a.docs.active_doc().and_then(|d| d.editor.debug_values.get(&line).cloned()).unwrap_or_default();
    let watch = |a: &CrabCraftApp| match a.debugger.session.as_ref().and_then(|s| s.watches().iter().find(|x| x.id == w)).map(|x| x.value.clone()) {
        Some(rc_dap::WatchValue::Value(v)) => v.result,
        _ => String::new(),
    };
    settle(&mut app, "the values", &|a| inline(a, 1) == "n = 255" && watch(a) == "256");

    // A value the debugger refuses: its message under the field, which stays open.
    edit_local(&mut app, "n", "no_such_name");
    app.debugger.apply_edit();
    settle(&mut app, "the error", &|a| a.debugger.edit.as_ref().is_some_and(|e| e.error.is_some()));
    let error = app.debugger.edit.as_ref().and_then(|e| e.error.clone()).unwrap();
    assert!(error.contains("no_such_name"), "{error}");
    app.debugger.cancel_edit();
    assert_eq!(local(&mut app, "n"), "255");

    // In hex, a hex value typed (setVariable, GDB prints it back in hex).
    app.run_ide_action(&ctx, Action::ToggleHexValues);
    settle(&mut app, "hexadecimal values", &|a| inline(a, 1) == "n = 0xff");
    edit_local(&mut app, "n", "0x1f");
    app.debugger.apply_edit();
    settle(&mut app, "the hex value", &|a| a.debugger.edit.is_none() && inline(a, 1) == "n = 0x1f" && watch(a) == "0x20");
    assert_eq!(local(&mut app, "n"), "0x1f");
    app.run_ide_action(&ctx, Action::ToggleHexValues);
    settle(&mut app, "decimal values", &|a| inline(a, 1) == "n = 31");

    // 255 → 7: the Variables view, the inline value and the watch follow.
    edit_local(&mut app, "n", "7");
    app.debugger.apply_edit();
    settle(&mut app, "the new value", &|a| a.debugger.edit.is_none() && inline(a, 1) == "n = 7" && watch(a) == "8");
    assert_eq!(local(&mut app, "n"), "7");

    // The program uses 7: `doubled` is 14 after the step, and so is its output.
    app.run_ide_action(&ctx, Action::StepOver);
    settle(&mut app, "the step", &|a| a.debugger.stopped() && a.docs.active_doc().and_then(|d| d.editor.execution_line) == Some(4));
    assert_eq!(local(&mut app, "doubled"), "14");
    app.run_ide_action(&ctx, Action::Resume);
    let start = Instant::now();
    while app.debugger.active() {
        let outs = app.debugger.update(&mut app.docs);
        app.handle_debug(outs);
        assert!(start.elapsed() < Duration::from_secs(30), "the program did not end");
        std::thread::sleep(Duration::from_millis(20));
    }
    let console = app.debugger.console.as_ref().unwrap().term.screen_lines().join("\n");
    assert!(console.contains("n=7 doubled=14"), "{console}");
}

#[test]
#[ignore = "Requires a GPU and GDB; writes target/snapshots/set-value-*.png"]
fn set_value_capture() {
    if rc_dap::gdb::locate().is_none() {
        return;
    }
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = paused_before_doubling(dir.path());
        app.settings.theme = if dark { rc_core::settings::ThemeChoice::Dark } else { rc_core::settings::ThemeChoice::Light };
        // A refused value shows its error; the field stays open with the text.
        edit_local(&mut app, "n", "0x1f + nope");
        app.debugger.apply_edit();
        settle_debugger(&mut app, "the error", &|a| a.debugger.edit.as_ref().is_some_and(|e| e.error.is_some()));
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
        h.run_steps(6);
        h.render().unwrap().save(out.join(format!("set-value-{}.png", if dark { "dark" } else { "light" }))).unwrap();
        h.state_mut().debugger.stop();
    }
}

const MEMORY_MAIN: &str = "fn main() {\n    let mut bytes: [u8; 4] = [0xde, 0xad, 0xbe, 0xef];\n    std::hint::black_box(&mut bytes);\n    bytes[1] = 0x42;\n    std::hint::black_box(&bytes);\n    println!(\"{bytes:?}\");\n}\n";

/// An app paused under GDB at `bytes[1] = 0x42` of [`MEMORY_MAIN`].
fn paused_on_bytes(dir: &Path) -> CrabCraftApp {
    paused_at(dir, MEMORY_MAIN, 3)
}

/// An app paused under GDB at the 0-based `line` of `main_rs`.
fn paused_at(dir: &Path, main_rs: &str, line: usize) -> CrabCraftApp {
    use rc_editor::{Action, BreakpointKind};
    let root = dir.join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), main_rs);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir);
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/main.rs"));
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(line, BreakpointKind::Normal);
    app.run_ide_action(&egui::Context::default(), Action::Debug);
    pump_debugger(&mut app, "the breakpoint");
    app
}

/// Pumps the debugger until `done`.
fn settle_debugger(app: &mut CrabCraftApp, what: &str, done: &dyn Fn(&CrabCraftApp) -> bool) {
    let start = Instant::now();
    while !done(app) {
        let outs = app.debugger.update(&mut app.docs);
        app.handle_debug(outs);
        assert!(start.elapsed() < Duration::from_secs(30), "timed out waiting for {what}: {:?}", app.debugger.memory.error);
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Opens the Memory tab on the local `bytes`, as its context menu does, and waits for the page.
fn show_bytes_in_memory(app: &mut CrabCraftApp) {
    let bytes = locals(app).into_iter().find(|v| v.name == "bytes").unwrap();
    app.debugger.show_memory_of(bytes.memory_reference.as_deref(), &bytes.evaluate_name.clone().unwrap_or_else(|| bytes.name.clone()));
    assert!(app.debugger.show_memory);
    settle_debugger(app, "the memory", &|a| a.debugger.memory.block.is_some() && !a.debugger.memory.loading());
}

/// Steps over the line that changes a byte and waits for the page to be read again.
fn step_over_the_byte_change(app: &mut CrabCraftApp) {
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::StepOver);
    settle_debugger(app, "the step", &|a| a.debugger.stopped() && a.debugger.memory.previous.is_some() && a.debugger.memory.block.is_some() && !a.debugger.memory.loading());
}

#[test]
fn the_memory_view_shows_bytes_and_highlights_what_a_step_changed() {
    if rc_dap::gdb::locate().is_none() {
        return;
    }
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let mut app = paused_on_bytes(dir.path());
    let caps = app.debugger.session.as_ref().unwrap().capabilities().clone();
    assert!(caps.supports_read_memory_request, "GDB should answer readMemory");

    // An array has no memory reference of its own: the view reads at `&bytes`.
    show_bytes_in_memory(&mut app);
    assert_eq!(app.debugger.memory.error, None);
    assert_eq!(app.debugger.memory.input, "&bytes");
    let block = app.debugger.memory.block.clone().unwrap();
    assert_eq!(block.bytes.len(), crate::memory_view::PAGE);
    let first = rc_dap::memory::row_text(&block.rows()[0]);
    assert!(first.contains("  de ad be ef "), "{first}");
    assert!(block.changed_since(app.debugger.memory.previous.as_ref()).iter().all(|c| !c));
    // GDB gives a memory reference for pointers (`&bytes`), the address the view shows.
    app.debugger.evaluate("&bytes".into());
    settle_debugger(&mut app, "the evaluation", &|a| matches!(a.debugger.eval, Some((_, _, Some(_)))));
    match &app.debugger.eval {
        Some((_, _, Some(Ok(v)))) => assert_eq!(v.memory_reference.as_deref().and_then(rc_dap::memory::parse_address), Some(block.address)),
        other => panic!("{other:?}"),
    }

    // Step over `bytes[1] = 0x42`: the page is read again and only that byte changed.
    let address = block.address;
    step_over_the_byte_change(&mut app);
    let now = app.debugger.memory.block.clone().unwrap();
    assert_eq!(now.address, address);
    assert!(rc_dap::memory::row_text(&now.rows()[0]).contains("  de 42 be ef "));
    let changed = now.changed_since(app.debugger.memory.previous.as_ref());
    assert!(changed[1], "the changed byte is highlighted");
    assert!(!changed[0] && !changed[2] && !changed[3]);

    // Paging and a typed address read on demand.
    app.debugger.memory_page(1);
    settle_debugger(&mut app, "the next page", &|a| !a.debugger.memory.loading());
    assert_eq!(app.debugger.memory.block.as_ref().map(|b| b.address), Some(address + crate::memory_view::PAGE as u64));
    app.debugger.memory.input = format!("{address:#x}");
    app.debugger.go_to_memory();
    settle_debugger(&mut app, "the typed address", &|a| !a.debugger.memory.loading());
    assert_eq!(app.debugger.memory.block.as_ref().and_then(|b| b.get(address + 1)), Some(Some(0x42)));
    // Address 0 cannot be read: shown as unreadable, with the reason.
    app.debugger.memory.input = "0x0".into();
    app.debugger.go_to_memory();
    settle_debugger(&mut app, "the unreadable page", &|a| !a.debugger.memory.loading());
    assert!(app.debugger.memory.error.is_some());
    assert!(app.debugger.memory.block.as_ref().is_some_and(|b| b.bytes.iter().all(|x| x.is_none())));
    app.debugger.stop();
}

#[test]
#[ignore = "Requires a GPU and GDB; writes target/snapshots/memory-view-*.png"]
fn memory_view_capture() {
    if rc_dap::gdb::locate().is_none() {
        return;
    }
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = paused_on_bytes(dir.path());
        app.settings.theme = if dark { rc_core::settings::ThemeChoice::Dark } else { rc_core::settings::ThemeChoice::Light };
        show_bytes_in_memory(&mut app);
        step_over_the_byte_change(&mut app);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
        h.run_steps(6);
        h.render().unwrap().save(out.join(format!("memory-view-{}.png", if dark { "dark" } else { "light" }))).unwrap();
        h.state_mut().debugger.stop();
    }
}

/// A function called from main: GDB shows no frame past `main`, so the caller is `demo::main`.
const DISASSEMBLY_MAIN: &str = "#[inline(never)]\nfn twice(n: u32) -> u32 {\n    let m = std::hint::black_box(n).wrapping_mul(2);\n    m.wrapping_add(1)\n}\n\nfn main() {\n    println!(\"{}\", twice(std::hint::black_box(20)));\n}\n";

/// An app paused under GDB at `let m = …` (line 3) of [`DISASSEMBLY_MAIN`].
fn paused_in_twice(dir: &Path) -> CrabCraftApp {
    paused_at(dir, DISASSEMBLY_MAIN, 2)
}

/// Opens the Disassembly tab and waits for the instructions around the current one.
fn show_disassembly(app: &mut CrabCraftApp) {
    app.debugger.show_disassembly = true;
    settle_debugger(app, "the disassembly", &|a| {
        let d = &a.debugger.disassembly;
        !d.loading() && d.instructions.iter().any(|i| Some(i.address) == d.current) || d.error.is_some()
    });
    assert_eq!(app.debugger.disassembly.error, None);
}

/// The selected frame's instruction pointer.
fn frame_ip(app: &CrabCraftApp) -> Option<u64> {
    app.debugger.session.as_ref()?.selected_frame()?.instruction_pointer_reference.as_deref().and_then(rc_dap::memory::parse_address)
}

/// The symbol of the last header at or before the current instruction's row.
fn current_function(app: &CrabCraftApp) -> Option<String> {
    use rc_dap::disassembly::{DisasmRow, rows};
    let d = &app.debugger.disassembly;
    let layout = rows(&d.instructions, d.current);
    let at = layout.iter().position(|r| matches!(r, DisasmRow::Instruction { current: true, .. }))?;
    layout[..at].iter().rev().find_map(|r| match r {
        DisasmRow::Header { symbol: Some(s), .. } => Some(s.clone()),
        _ => None,
    })
}

#[test]
fn the_disassembly_view_marks_the_current_instruction_of_the_function() {
    use rc_dap::disassembly::{DisasmRow, rows};
    if rc_dap::gdb::locate().is_none() {
        return;
    }
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let mut app = paused_in_twice(dir.path());
    let caps = app.debugger.session.as_ref().unwrap().capabilities().clone();
    assert!(caps.supports_disassemble_request, "GDB should answer disassemble");
    assert!(caps.supports_stepping_granularity, "GDB should step by instruction");

    show_disassembly(&mut app);
    let ip = frame_ip(&app).unwrap();
    let d = &app.debugger.disassembly;
    assert_eq!(d.current, Some(ip));
    // The current instruction is marked once, belongs to line 3 of main.rs and has its bytes.
    let layout = rows(&d.instructions, d.current);
    assert_eq!(layout.iter().filter(|r| matches!(r, DisasmRow::Instruction { current: true, .. })).count(), 1);
    let i = d.instructions.iter().find(|i| i.address == ip).unwrap();
    assert_eq!(i.line, Some(3), "{i:?}");
    assert!(i.file.as_deref().is_some_and(|f| f.ends_with("main.rs")), "{i:?}");
    assert!(i.bytes.is_some() && !i.text.is_empty(), "{i:?}");
    // Instructions were found above the current one (GDB cannot disassemble backwards).
    assert!(d.instructions.first().is_some_and(|f| f.address < ip), "nothing above the current instruction");
    // GDB names the function at its first instruction: load earlier until it shows.
    for _ in 0..5 {
        if current_function(&app).is_some() {
            break;
        }
        app.debugger.disassembly_more(true);
        settle_debugger(&mut app, "earlier instructions", &|a| !a.debugger.disassembly.loading());
    }
    assert_eq!(current_function(&app).as_deref(), Some("demo::twice"));

    // Load later adds instructions after the last one.
    let (count, last) = (app.debugger.disassembly.instructions.len(), app.debugger.disassembly.instructions.last().map(|i| i.address));
    app.debugger.disassembly_more(false);
    settle_debugger(&mut app, "later instructions", &|a| !a.debugger.disassembly.loading());
    assert!(app.debugger.disassembly.instructions.len() > count);
    assert!(app.debugger.disassembly.instructions.last().map(|i| i.address) > last);

    // Step Over Instruction: one instruction further, still in twice; the view follows.
    app.debugger.step_instruction(true);
    settle_debugger(&mut app, "the instruction step", &|a| a.debugger.stopped() && frame_ip(a).is_some_and(|n| n != ip));
    show_disassembly(&mut app);
    let next = frame_ip(&app).unwrap();
    assert!(next > ip && next - ip <= 15, "{ip:#x} → {next:#x}");
    assert_eq!(app.debugger.disassembly.current, Some(next));
    assert_eq!(current_function(&app).as_deref(), Some("demo::twice"));

    // Selecting the caller frame shows the code at its return address.
    let tid = app.debugger.session.as_ref().and_then(|s| s.stop_info()).unwrap().thread_id;
    let caller = |a: &CrabCraftApp| a.debugger.session.as_ref().and_then(|s| s.frames(tid)).and_then(|f| f.frames.get(1).cloned());
    if caller(&app).is_none() {
        app.debugger.session.as_mut().unwrap().request_frames(tid);
        settle_debugger(&mut app, "the caller frame", &|a| caller(a).is_some());
    }
    let caller = caller(&app).unwrap();
    app.debugger.session.as_mut().unwrap().select_frame(caller.id);
    let ret = caller.instruction_pointer_reference.as_deref().and_then(rc_dap::memory::parse_address).unwrap();
    assert_eq!(caller.name, "demo::main");
    settle_debugger(&mut app, "the caller's code", &|a| a.debugger.disassembly.current == Some(ret) && !a.debugger.disassembly.loading());
    let at = app.debugger.disassembly.instructions.iter().find(|i| i.address == ret).unwrap();
    assert_eq!(at.line, Some(8), "{at:?}");
    // The instruction before the return address is the call to twice.
    let call = app.debugger.disassembly.instructions.iter().rev().find(|i| i.address < ret).unwrap();
    assert!(call.text.contains("call") && call.text.contains("twice"), "{call:?}");
    app.debugger.stop();
}

#[test]
#[ignore = "Requires a GPU and GDB; writes target/snapshots/disassembly-view-*.png"]
fn disassembly_view_capture() {
    if rc_dap::gdb::locate().is_none() {
        return;
    }
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = paused_in_twice(dir.path());
        app.settings.theme = if dark { rc_core::settings::ThemeChoice::Dark } else { rc_core::settings::ThemeChoice::Light };
        show_disassembly(&mut app);
        app.debugger.disassembly.scroll_to_current = true;
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
        h.run_steps(6);
        h.render().unwrap().save(out.join(format!("disassembly-view-{}.png", if dark { "dark" } else { "light" }))).unwrap();
        h.state_mut().debugger.stop();
    }
}

#[test]
fn step_into_walks_past_the_standard_library() {
    use rc_editor::{Action, BreakpointKind};
    const MAIN: &str = "fn own(text: &str) -> usize {\n    text.len()\n}\n\nfn main() {\n    let text = String::from(\"abc\");\n    let upper = text.to_uppercase();\n    let n = own(&upper);\n    println!(\"{n}\");\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), MAIN);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/main.rs"));
    let ctx = egui::Context::default();
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(6, BreakpointKind::Normal);
    app.run_ide_action(&ctx, Action::Debug);
    pump_debugger(&mut app, "the breakpoint");
    let line = |a: &CrabCraftApp| a.docs.active_doc().and_then(|d| d.path.ends_with("src/main.rs").then_some(d.editor.execution_line).flatten());
    assert_eq!(line(&app), Some(6));

    // `to_uppercase` is library code; `own` is the project's own, and Step Into enters it.
    // A line with several calls can stop again on itself after each one is skipped.
    let step_to = |app: &mut CrabCraftApp, target: usize| {
        for _ in 0..5 {
            app.run_ide_action(&ctx, Action::StepInto);
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(600) {
                let outs = app.debugger.update(&mut app.docs);
                app.handle_debug(outs);
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(app.debugger.stopped(), "the step did not end");
            let now = line(app);
            assert!(now.is_some(), "stepped out of the project into a library");
            if now == Some(target) {
                return;
            }
        }
        panic!("never reached line {target}");
    };
    step_to(&mut app, 7);
    step_to(&mut app, 1);
    app.run_ide_action(&ctx, Action::StopRun);
}

#[test]
fn forced_steps_enter_the_library_and_pass_breakpoints_by() {
    use rc_editor::{Action, BreakpointKind};
    const MAIN: &str = "#[inline(never)]\nfn own(n: u32) -> u32 {\n    std::hint::black_box(n) + 1\n}\n\nfn main() {\n    let text = String::from(\"abc\");\n    let a = own(std::hint::black_box(1));\n    let b = own(a);\n    let c = own(b);\n    let d = own(c);\n    println!(\"{text} {d}\");\n}\n";
    if rc_dap::gdb::locate().is_none() {
        return;
    }
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    // Paused at `String::from`, with a breakpoint in `own` too.
    let mut app = paused_at(dir.path(), MAIN, 6);
    let main_rs = app.docs.active_doc().unwrap().path.clone();
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(2, BreakpointKind::Normal);
    let ctx = egui::Context::default();
    let line = |a: &CrabCraftApp| a.docs.list.iter().find(|d| d.path == main_rs).and_then(|d| d.editor.execution_line);
    let top = |a: &CrabCraftApp| a.debugger.session.as_ref().and_then(|s| s.selected_frame()).map(|f| f.name.clone()).unwrap_or_default();
    let run = |app: &mut CrabCraftApp, action: Action, what: &str| {
        app.run_ide_action(&ctx, action);
        settle_debugger(app, what, &|a| a.debugger.stopped() && a.debugger.session.as_ref().is_some_and(|s| s.selected_frame().is_some()));
    };
    assert_eq!(line(&app), Some(6));
    assert!(app.debugger.session.as_ref().unwrap().can_force_step_in());

    // Force Step Into enters `String::from`, which Step Into walks past.
    run(&mut app, Action::ForceStepInto, "the forced step into");
    let entered = top(&app);
    assert!(!entered.starts_with("demo::"), "still in the project: {entered}");
    assert!(entered.contains("from") || entered.contains("alloc") || entered.contains("core"), "{entered}");
    // The skip of the standard library is back once stopped.
    let id = app.debugger.session.as_mut().unwrap().evaluate("info skip", rc_dap::EvalContext::Repl);
    let mut skips = String::new();
    let start = Instant::now();
    while skips.is_empty() && start.elapsed() < Duration::from_secs(10) {
        for e in app.debugger.session.as_mut().unwrap().pump() {
            if let rc_dap::DebugEvent::Evaluated { id: i, result, .. } = e
                && i == id
            {
                skips = result.map(|v| v.result).unwrap_or_else(|e| e);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(skips.contains("core|alloc|std") && skips.contains(" y "), "{skips}");

    // Force Run to Cursor passes the breakpoint in `own` twice.
    app.open_file(&main_rs);
    app.docs.active_doc_mut().unwrap().editor.goto(9, 0);
    run(&mut app, Action::ForceRunToCursor, "the forced run to cursor");
    assert_eq!(line(&app), Some(9), "{}", top(&app));
    let s = app.debugger.session.as_ref().unwrap();
    assert!(!s.ignoring_breakpoints());
    assert!(s.breakpoints().get(&main_rs, 3).is_some_and(|b| b.spec.enabled));
    assert!(s.breakpoints().get(&main_rs, 10).is_none(), "the one-shot breakpoint is gone");

    // Force Step Over passes it once more; a plain Step Over then stops there again.
    run(&mut app, Action::ForceStepOver, "the forced step over");
    assert_eq!(line(&app), Some(10), "{}", top(&app));
    run(&mut app, Action::StepOver, "the step over");
    assert_eq!(line(&app), Some(2), "{}", top(&app));
    assert_eq!(app.debugger.session.as_ref().unwrap().stop_info().map(|i| i.reason.clone()), Some(rc_dap::StopReason::Breakpoint));
    app.run_ide_action(&ctx, Action::StopRun);
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
    assert!(app.runner.cargo_workspace.is_none());
    app.trust_project_tools_for_test();
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
    app.trust_project_tools_for_test();
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

    // The editor shows why on that line, until the text changes.
    app.update_test_markers();
    let note = |a: &CrabCraftApp| a.docs.active_doc().unwrap().editor.failure_notes.get(&13).cloned();
    let text = note(&app).expect("a note on the failed line");
    assert!(text.contains("assertion") && text.contains("left: 1") && text.contains("right: 2"), "{text}");
    let doc = app.docs.active_doc_mut().unwrap();
    doc.editor.replace(0, 0, "\n");
    app.update_test_markers();
    assert_eq!(note(&app), None);

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

/// A real Ctrl+click (⌘ on macOS) in the rendered editor, as a mouse sends it.
fn command_click(h: &mut egui_kittest::Harness<'static, CrabCraftApp>, at: egui::Pos2) {
    let mods = egui::Modifiers::COMMAND;
    h.input_mut().events.push(egui::Event::ModifiersChanged(mods));
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::ModifiersChanged(mods));
    h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: mods });
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::ModifiersChanged(mods));
    h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: mods });
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
}

#[test]
fn ctrl_click_goes_to_the_definition_of_a_method_a_type_and_a_std_item() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/main.rs"), "use demo::Greeter;\n\nfn main() {\n    let text: String = Greeter.hello().to_string();\n    println!(\"{text}\");\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    app.trust_project();
    let main = root.join("src/main.rs");
    app.open_file(&main);
    let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
    let ready = |a: &CrabCraftApp| a.insight.status == InsightStatus::Ready && a.docs.active_doc().is_some_and(|d| !d.editor.semantic.is_empty());
    let start = Instant::now();
    while !ready(h.state()) {
        assert!(start.elapsed() < Duration::from_secs(180), "timed out waiting for rust-analyzer: {:?}", h.state().insight.status);
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(30));
    }
    // Where a word of main.rs is drawn: put the caret in it and read the caret's place.
    let place = |h: &mut egui_kittest::Harness<'static, CrabCraftApp>, line: usize, col: usize| {
        let i = h.state().docs.index_of(&main).unwrap();
        h.state_mut().docs.activate(i);
        h.state_mut().docs.list[i].editor.goto(line, col);
        h.run_steps(3);
        h.state().caret_screen.unwrap() + egui::vec2(1.0, -6.0)
    };
    let wait = |h: &mut egui_kittest::Harness<'static, CrabCraftApp>, what: &str, done: &dyn Fn(&CrabCraftApp) -> bool| {
        let start = Instant::now();
        while !done(h.state()) {
            assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    let at = |a: &CrabCraftApp| a.docs.active_doc().map(|d| (d.path.clone(), d.editor.caret_line_col().0));

    // The method: to `pub fn hello` in lib.rs.
    let p = place(&mut h, 3, "    let text: String = Greeter.he".len());
    command_click(&mut h, p);
    wait(&mut h, "hello in lib.rs", &|a| at(a) == Some((root.join("src/lib.rs"), 3)));

    // The type, from its `use`: to `pub struct Greeter`.
    let p = place(&mut h, 0, "use demo::Gre".len());
    command_click(&mut h, p);
    wait(&mut h, "Greeter in lib.rs", &|a| at(a) == Some((root.join("src/lib.rs"), 0)));

    // A standard library type opens read-only.
    let p = place(&mut h, 3, "    let text: Str".len());
    command_click(&mut h, p);
    wait(&mut h, "String in the standard library", &|a| a.docs.active_doc().is_some_and(|d| d.path.ends_with("string.rs") && d.editor.read_only));

    // Back returns to the click.
    h.state_mut().run_ide_action(&egui::Context::default(), rc_editor::Action::NavigateBack);
    assert_eq!(at(h.state()).map(|(p, l)| (p, l)), Some((main.clone(), 3)));
    h.state_mut().insight.stop();
}

/// A real right click in the rendered editor, as a mouse sends it.
fn right_click(h: &mut egui_kittest::Harness<'static, CrabCraftApp>, at: egui::Pos2) {
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
}

#[test]
fn go_to_declaration_from_the_context_menu_goes_to_the_clicked_name() {
    use egui_kittest::kittest::Queryable;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/main.rs"), "use demo::Greeter;\n\nfn main() {\n    let text: String = Greeter.hello().to_string();\n    println!(\"{text}\");\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    app.trust_project();
    let main = root.join("src/main.rs");
    app.open_file(&main);
    let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
    let wait = |h: &mut egui_kittest::Harness<'static, CrabCraftApp>, what: &str, secs: u64, done: &dyn Fn(&CrabCraftApp) -> bool| {
        let start = Instant::now();
        while !done(h.state()) {
            assert!(start.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}: {:?}", h.state().insight.status);
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    wait(&mut h, "rust-analyzer", 180, &|a| a.insight.status == InsightStatus::Ready && a.docs.active_doc().is_some_and(|d| !d.editor.semantic.is_empty()));
    let at = |a: &CrabCraftApp| a.docs.active_doc().map(|d| (d.path.clone(), d.editor.caret_line_col().0));
    // Where `hello` is drawn; the caret is elsewhere when the user right-clicks it.
    let i = h.state().docs.index_of(&main).unwrap();
    h.state_mut().docs.list[i].editor.goto(3, "    let text: String = Greeter.he".len());
    h.run_steps(3);
    let p = h.state().caret_screen.unwrap() + egui::vec2(1.0, -6.0);
    h.state_mut().docs.list[i].editor.goto(5, 0);
    h.run_steps(3);
    right_click(&mut h, p);
    h.get_by_label("Go to Declaration").click();
    h.run_steps(1);
    wait(&mut h, "hello in lib.rs", 60, &|a| at(a) == Some((root.join("src/lib.rs"), 3)));

    // Find Action has it, also under the name other editors give it.
    for name in ["Go to Declaration", "Go to Definition"] {
        let i = h.state().docs.index_of(&main).unwrap();
        h.state_mut().docs.activate(i);
        h.state_mut().docs.list[i].editor.goto(0, "use demo::Gre".len());
        h.run_steps(3);
        let chord = h.state().keymap.chord(rc_editor::Action::FindAction).unwrap();
        h.key_press_modifiers(chord.mods, chord.key);
        h.run_steps(3);
        h.input_mut().events.push(egui::Event::Text(name.into()));
        h.run_steps(3);
        h.key_press(egui::Key::Enter);
        h.run_steps(1);
        wait(&mut h, &format!("{name}: Greeter in lib.rs"), 60, &|a| at(a) == Some((root.join("src/lib.rs"), 0)));
    }
    h.state_mut().insight.stop();
}

#[test]
fn tests_run_with_nextest_when_the_setting_is_on() {
    if std::process::Command::new("cargo").args(["nextest", "--version"]).output().map_or(true, |o| !o.status.success()) {
        eprintln!("skipped: cargo-nextest is not installed");
        return;
    }
    const LIB: &str = "pub fn one() -> u32 {\n    1\n}\n\n#[cfg(test)]\nmod checks {\n    #[test]\n    fn good() {\n        assert_eq!(super::one(), 1);\n    }\n\n    #[test]\n    fn bad() {\n        assert_eq!(super::one(), 2);\n    }\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/lib.rs"), LIB);
    write(&root.join("src/main.rs"), "fn main() {}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    let mut settings = app.settings.clone();
    settings.use_nextest = true;
    app.apply_settings(&egui::Context::default(), settings);
    assert!(app.runner.nextest);
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::RunTests);
    pump(&mut app, "the tests", |a| a.runner.tests.as_ref().is_some_and(|t| !t.running));
    let t = app.runner.tests.as_ref().unwrap();
    assert_eq!((t.passed, t.failed), (1, 1));
    // nextest names the test binary where cargo test gives its path.
    assert_eq!(t.results["checks::bad"][0].source.as_deref(), Some(Path::new("demo")), "not run by nextest");
    // The failure leads to its line, as with cargo test.
    app.goto_test("checks::bad");
    assert_eq!(app.docs.active_doc().unwrap().editor.caret_line_col().0, 13);
    // Only the failed test runs again.
    let outs = app.runner.rerun_failed();
    crate::ui::run_windows::handle_outputs(&mut app, outs);
    pump(&mut app, "the second run", |a| a.runner.tests.as_ref().is_some_and(|t| !t.running));
    let t = app.runner.tests.as_ref().unwrap();
    assert_eq!((t.passed, t.failed, t.skipped), (0, 1, 0));
}

#[test]
fn invalidate_caches_rebuilds_what_the_ide_derived_and_keeps_the_tabs() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    assert!(app.runner.cargo_workspace.is_none());
    app.open_file(&root.join("src/lib.rs"));
    app.open_file(&root.join("src/main.rs"));

    // Untrusted: neither Cargo metadata, cargo clean nor the language server may run.
    app.invalidate_caches(true);
    assert_eq!(app.insight.status, InsightStatus::Untrusted);
    assert!(app.insight.session.is_none());
    assert!(app.runner.cargo_workspace.is_none());
    assert!(app.runner.build.is_none());

    app.trust_project();
    pump(&mut app, "the server", |a| a.insight.status == InsightStatus::Ready);
    app.runner.selected = 0;
    app.runner.run_selected();
    pump(&mut app, "the build", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    assert!(root.join("target").exists());

    // A change the watcher missed, and a Cargo model that is out of date.
    write(&root.join("src/lib.rs"), "pub fn changed() {}\n");
    app.runner.cargo_workspace = None;
    app.invalidate_caches(true);
    assert!(matches!(app.insight.status, InsightStatus::Starting | InsightStatus::Loading(_)), "{:?}", app.insight.status);
    assert_eq!(app.docs.list.len(), 2);
    assert_eq!(app.docs.active_doc().map(|d| d.name()), Some("main.rs".to_string()));
    let lib = app.docs.index_of(&root.join("src/lib.rs")).unwrap();
    pump(&mut app, "the asynchronous external reload", |app| app.docs.list[lib].editor.buffer.rope().to_string() == "pub fn changed() {}\n");
    assert_eq!(app.docs.list[lib].editor.buffer.rope().to_string(), "pub fn changed() {}\n");
    let cleaned = |a: &CrabCraftApp| a.runner.build.as_ref().is_some_and(|b| b.title == "cargo clean" && b.result.as_ref().is_some_and(|r| r.success));
    pump(&mut app, "everything again", |a| a.insight.status == InsightStatus::Ready && a.runner.cargo_workspace.is_some() && cleaned(a));
    app.insight.stop();
}

/// Asks rust-analyzer for the workspace symbols named `query` until `want` holds for the names.
fn wait_symbols(app: &mut CrabCraftApp, query: &str, what: &str, want: impl Fn(&[String]) -> bool) {
    let start = Instant::now();
    let mut last = Vec::new();
    while start.elapsed() < Duration::from_secs(120) {
        let mut pending = app.insight.workspace_symbols(query, true, false);
        let asked = Instant::now();
        while let Some(p) = &mut pending
            && asked.elapsed() < Duration::from_secs(30)
        {
            let out = app.insight.update(&mut app.docs, true);
            app.handle_insight(out);
            if let Some(r) = p.poll() {
                last = r.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().filter_map(|s| s.get("name")?.as_str().map(str::to_string)).collect();
                pending = None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if want(&last) {
            return;
        }
        let out = app.insight.update(&mut app.docs, true);
        app.handle_insight(out);
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("timed out waiting for {what}; last symbols {last:?}");
}

#[test]
fn an_excluded_folder_leaves_the_index_find_in_files_and_rust_analyzer() {
    use crate::ui::explorer::Exclusion;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/lib.rs"), &format!("pub mod generated;\n{LIB}"));
    write(&root.join("src/generated/mod.rs"), "pub fn only_in_generated_code() -> u32 {\n    7\n}\n");
    let root = root.canonicalize().unwrap();
    let generated = root.join("src/generated");
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/main.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    let indexed = |a: &CrabCraftApp| a.files.index.as_ref().filter(|i| i.is_ready()).map(|i| i.snapshot().get("src/generated/mod.rs").is_some());
    pump(&mut app, "the file index", |a| indexed(a).is_some());
    assert_eq!(indexed(&app), Some(true));
    wait_symbols(&mut app, "only_in_generated", "the symbol of the folder", |names| names.iter().any(|n| n == "only_in_generated_code"));

    // Excluded: out of the index, Find in Files and the server; dimmed in the explorer; saved.
    app.set_excluded(&generated, true);
    assert_eq!(app.explorer.excluded, ["src/generated"]);
    assert_eq!(app.explorer.exclusion(&root, &generated), Exclusion::Itself);
    assert_eq!(app.explorer.exclusion(&root, &generated.join("mod.rs")), Exclusion::Inherited);
    assert_eq!(app.explorer.exclusion(&root, &root.join("src/lib.rs")), Exclusion::No);
    pump(&mut app, "the new file index", |a| indexed(a) == Some(false));
    let mut find = crate::search::FindInFiles::new(&root, Waker::noop(), "generated".into());
    find.excluded = app.explorer.excluded.clone();
    find.search();
    pump(&mut app, "Find in Files", |_| {
        find.poll();
        find.summary.is_some()
    });
    let found: Vec<&str> = find.results.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(found, ["src/lib.rs"]);
    wait_symbols(&mut app, "only_in_generated", "rust-analyzer to drop the folder", |names| names.is_empty());
    app.save_session();
    assert_eq!(rc_core::Session::load(&root).excluded, ["src/generated"]);

    // Included again: everything comes back.
    app.set_excluded(&generated, false);
    assert!(app.explorer.excluded.is_empty());
    pump(&mut app, "the file index again", |a| indexed(a) == Some(true));
    wait_symbols(&mut app, "only_in_generated", "the symbol again", |names| names.iter().any(|n| n == "only_in_generated_code"));
    app.insight.stop();

    // Reopened: the exclusion list comes from the session.
    let mut app = new_app(dir.path());
    std::fs::write(rc_core::Session::file_for(&root), r#"{ "excluded": ["src\\generated", "../outside"] }"#).unwrap();
    app.open_workspace(&root);
    assert_eq!(app.explorer.excluded, ["src/generated"]);
    pump(&mut app, "the reopened file index", |a| indexed(a) == Some(false));
    app.insight.stop();
}

/// Looks for what renaming `from` to `to` changes until `want` holds for the found changes.
fn rename_changes(app: &mut CrabCraftApp, from: &Path, to: &Path, want: impl Fn(&[crate::file_rename::Change]) -> bool) -> crate::file_rename::FileRename {
    let start = Instant::now();
    loop {
        let mut r = crate::file_rename::FileRename::start(app, from.to_path_buf(), to.to_path_buf());
        assert!(r.asked_server, "rust-analyzer offers willRenameFiles");
        pump(app, "the changes of the rename", |a| {
            r.update(&a.docs);
            !r.searching()
        });
        if want(&r.changes) {
            return r;
        }
        // The server may still be loading the last rename.
        assert!(start.elapsed() < Duration::from_secs(60), "{:?} {:?}", r.server_error, r.changes);
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
fn renaming_a_module_file_updates_its_declaration_and_paths() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/lib.rs"), "pub mod util;\npub use util::helper;\n");
    write(&root.join("src/util.rs"), "pub fn helper() {}\n");
    write(&root.join("src/main.rs"), "fn main() {\n    demo::util::helper();\n}\n");
    write(&root.join("README.md"), "See src/util.rs.\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    let src = root.join("src");
    app.open_file(&src.join("util.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    let read = |name: &str| std::fs::read_to_string(src.join(name)).unwrap();

    // The declaration and the path in lib.rs, the path in main.rs, and the README as text.
    let mentions = |name: &'static str| move |c: &[crate::file_rename::Change]| c.iter().filter(|c| c.from_server && c.after.contains(name)).count() >= 3;
    let mut r = rename_changes(&mut app, &src.join("util.rs"), &src.join("tools.rs"), mentions("tools"));
    let lib: Vec<_> = r.changes.iter().filter(|c| c.path == src.join("lib.rs")).map(|c| (c.line, c.before.clone(), c.after.clone(), c.checked)).collect();
    assert!(lib.contains(&(0, "pub mod util;".into(), "pub mod tools;".into(), true)), "{lib:?}");
    let readme = r.changes.iter().find(|c| c.path == root.join("README.md")).expect("the README's mention");
    assert!(!readme.from_server && !readme.checked && readme.after == "See src/tools.rs.", "{readme:?}");
    let changes = std::mem::take(&mut r.changes);
    assert_eq!(crate::file_rename::apply(&mut app, &r.from, &r.to, &changes), Ok(2));
    assert!(!src.join("util.rs").exists() && src.join("tools.rs").exists());
    assert_eq!(read("lib.rs"), "pub mod tools;\npub use tools::helper;\n");
    assert!(read("main.rs").contains("demo::tools::helper();"));
    assert_eq!(std::fs::read_to_string(root.join("README.md")).unwrap(), "See src/util.rs.\n");
    // The open tab follows the file.
    assert!(app.docs.list.iter().any(|d| d.path == src.join("tools.rs")));
    let check = std::process::Command::new("cargo").args(["check", "--quiet"]).current_dir(&root).output().unwrap();
    assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));

    // A file left unchecked keeps its text.
    let mut r = rename_changes(&mut app, &src.join("tools.rs"), &src.join("gear.rs"), mentions("gear"));
    for c in r.changes.iter_mut().filter(|c| c.path == src.join("main.rs")) {
        c.checked = false;
    }
    let changes = std::mem::take(&mut r.changes);
    assert_eq!(crate::file_rename::apply(&mut app, &r.from, &r.to, &changes), Ok(1));
    assert_eq!(read("lib.rs"), "pub mod gear;\npub use gear::helper;\n");
    assert!(read("main.rs").contains("demo::tools::helper();"));

    // Rename only: nothing else changes.
    let r = rename_changes(&mut app, &src.join("gear.rs"), &src.join("wheel.rs"), |c| !c.is_empty());
    assert_eq!(crate::file_rename::apply(&mut app, &r.from, &r.to, &[]), Ok(0));
    assert!(src.join("wheel.rs").exists());
    assert_eq!(read("lib.rs"), "pub mod gear;\npub use gear::helper;\n");
    app.insight.stop();
}

#[test]
fn f2_in_the_explorer_renames_a_file_and_offers_its_text_mentions() {
    use egui_kittest::kittest::Queryable;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("notes");
    write(&root.join("notes.txt"), "plain\n");
    write(&root.join("README.md"), "Read notes.txt first.\n");
    let root = root.canonicalize().unwrap();
    let mut app = crate::screens::test_app(&dir.path().join("home"), true);
    app.open_workspace(&root);
    let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
    h.get_by_label("notes.txt").click();
    h.run_steps(2);
    h.key_press(egui::Key::F2);
    h.run_steps(2);
    match &mut h.state_mut().dialog {
        Some(crate::ui::dialogs::Dialog::Rename { name, .. }) => *name = "todo.txt".into(),
        _ => panic!("F2 opens the Rename dialog; focus {} docs {}", h.state().explorer.has_focus, h.state().docs.list.len()),
    }
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    let start = Instant::now();
    while h.query_by_label("Rename Only").is_none() || h.state().dialog.as_ref().is_some_and(|d| matches!(d, crate::ui::dialogs::Dialog::RenameFile(r) if r.searching())) {
        assert!(start.elapsed() < Duration::from_secs(30), "the list of changes");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(20));
    }
    // A text match is not picked until the user picks it.
    assert!(h.query_by_label("No language server checked the code: only text matches are listed.").is_some());
    assert!(!root.join("todo.txt").exists());
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "README.md").click();
    h.run_steps(2);
    h.get_by_label("Rename and Update References").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert!(root.join("todo.txt").exists() && !root.join("notes.txt").exists());
    assert_eq!(std::fs::read_to_string(root.join("README.md")).unwrap(), "Read todo.txt first.\n");
}

/// Search Everywhere's Classes and Symbols tabs ask rust-analyzer; the libraries scope adds
/// the standard library.
#[test]
fn search_everywhere_finds_classes_symbols_and_library_types() {
    use crate::search::{SeScope, SeTab};
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    app.trust_project();
    app.open_file(&root.join("src/main.rs"));
    let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
    let wait = |h: &mut egui_kittest::Harness<'static, CrabCraftApp>, what: &str, done: &dyn Fn(&CrabCraftApp) -> bool| {
        let start = Instant::now();
        while !done(h.state()) {
            assert!(start.elapsed() < Duration::from_secs(180), "timed out waiting for {what}: {:?}", h.state().insight.status);
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    wait(&mut h, "rust-analyzer", &|a| a.insight.status == InsightStatus::Ready);
    let chord = h.state().keymap.chord(rc_editor::Action::SearchEverywhere).unwrap();
    h.key_press_modifiers(chord.mods, chord.key);
    h.run_steps(3);
    let set = |h: &mut egui_kittest::Harness<'static, CrabCraftApp>, tab: SeTab, text: &str, scope: SeScope| {
        let s = h.state_mut().search_everywhere.as_mut().expect("Search Everywhere is open");
        s.tab = tab;
        s.text = text.into();
        s.scope = scope;
    };
    let found = |a: &CrabCraftApp, name: &str| a.search_everywhere.as_ref().is_some_and(|s| s.symbols.iter().any(|x| x.name == name));
    let symbol = |a: &CrabCraftApp, name: &str| a.search_everywhere.as_ref().and_then(|s| s.symbols.iter().find(|x| x.name == name).cloned());

    // Classes: the struct of lib.rs.
    set(&mut h, SeTab::Classes, "Greeter", SeScope::Project);
    wait(&mut h, "the Greeter struct", &|a| found(a, "Greeter"));
    let g = symbol(h.state(), "Greeter").unwrap();
    assert!(g.is_type() && g.path == root.join("src/lib.rs"), "{g:?}");

    // Symbols: a method, which rust-analyzer only returns when asked for every symbol.
    set(&mut h, SeTab::Symbols, "hello", SeScope::Project);
    wait(&mut h, "the hello method", &|a| found(a, "hello"));
    assert_eq!(symbol(h.state(), "hello").unwrap().path, root.join("src/lib.rs"));

    // The standard library only with the libraries scope.
    set(&mut h, SeTab::Classes, "HashMap", SeScope::Project);
    wait(&mut h, "the project's answer", &|a| a.search_everywhere.as_ref().is_some_and(|s| !s.symbols.iter().any(|x| x.name == "Greeter" || x.name == "hello")));
    h.run_steps(30);
    assert!(!found(h.state(), "HashMap"), "HashMap is not a project type");
    set(&mut h, SeTab::Classes, "HashMap", SeScope::ProjectAndLibraries);
    wait(&mut h, "HashMap from the standard library", &|a| found(a, "HashMap"));
    assert!(!symbol(h.state(), "HashMap").unwrap().path.starts_with(&root));
    h.state_mut().insight.stop();
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
    app.trust_project_tools_for_test();
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
    app.trust_project_tools_for_test();
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
    app.trust_project_tools_for_test();
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
    app.trust_project_tools_for_test();
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
    app.trust_project_tools_for_test();
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
fn the_project_environment_reaches_cargo_the_program_and_its_tests() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    // `env!` needs the variable in the compiler's environment, so Cargo must have it.
    write(&root.join("src/main.rs"), "fn main() {\n    println!(\"built={} run={} shared={}\", env!(\"CRAB_PROJECT\"), std::env::var(\"CRAB_PROJECT\").unwrap_or_default(), std::env::var(\"CRAB_SHARED\").unwrap_or_default());\n}\n");
    write(&root.join("src/lib.rs"), "#[test]\nfn sees_the_project_environment() {\n    assert_eq!(option_env!(\"CRAB_PROJECT\"), Some(\"from-project\"));\n    assert_eq!(std::env::var(\"CRAB_SHARED\").as_deref(), Ok(\"project\"));\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());

    // The action opens the dialog with the current variables.
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::ProjectEnvironment);
    assert!(matches!(&app.dialog, Some(crate::ui::dialogs::Dialog::ProjectEnv { text }) if text.is_empty()));
    app.dialog = None;
    app.set_project_env("CRAB_PROJECT=from-project\nCRAB_SHARED=project".to_string());
    assert!(app.session_dirty);
    pump(&mut app, "the reload", |a| a.runner.cargo_workspace.is_some());

    // The run configuration's variables win over the project's.
    let name = app.runner.selected_config().unwrap().name.clone();
    app.runner.options.insert(name, rc_core::session::RunOptions { env: "CRAB_SHARED=config".to_string(), ..Default::default() });
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Run);
    pump(&mut app, "the output", |a| a.runner.runs.iter().any(|r| r.console.term.screen_lines().join("\n").contains("built=from-project run=from-project shared=config")));

    app.run_ide_action(&egui::Context::default(), rc_editor::Action::RunTests);
    pump(&mut app, "the tests", |a| a.runner.tests.as_ref().is_some_and(|t| !t.running));
    let t = app.runner.tests.as_ref().unwrap();
    assert_eq!((t.passed, t.failed), (1, 0), "{:?}", t.failure);

    // A command typed in the Cargo window has it too.
    app.run_cargo_command("run --bin demo");
    let console = |a: &CrabCraftApp| a.runner.build.as_ref().map(|b| b.console.term.screen_lines().join("\n")).unwrap_or_default();
    pump(&mut app, "the typed command", |a| console(a).contains("built=from-project run=from-project shared=project"));

    // It is kept in the session of the workspace.
    app.save_session();
    assert_eq!(rc_core::Session::load(&root).project_env, "CRAB_PROJECT=from-project\nCRAB_SHARED=project");
}

#[test]
fn rust_analyzer_runs_build_scripts_with_the_project_environment() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("build.rs"), "fn main() {\n    println!(\"cargo:rerun-if-env-changed=CRAB_PROJECT\");\n    std::fs::write(\"BUILD_ENV\", std::env::var(\"CRAB_PROJECT\").unwrap_or_default()).unwrap();\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    assert!(app.runner.cargo_workspace.is_none());
    // Setting it runs nothing while the project is not trusted.
    app.set_project_env("CRAB_PROJECT=first".to_string());
    assert!(app.insight.session.is_none());
    app.trust_project();
    pump(&mut app, "the build script", |_| std::fs::read_to_string(root.join("BUILD_ENV")).is_ok_and(|t| t == "first"));

    // A change restarts the server, whose build scripts see the new value.
    std::fs::remove_file(root.join("BUILD_ENV")).unwrap();
    app.set_project_env("CRAB_PROJECT=second".to_string());
    assert!(app.insight.session.is_some());
    pump(&mut app, "the build script again", |_| std::fs::read_to_string(root.join("BUILD_ENV")).is_ok_and(|t| t == "second"));
    app.insight.stop();
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
fn problems_of_a_closed_file_show_their_real_line() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("examples/bad.rs"), "fn main() {\n    let _a = 1;\n    let _n: u32 = \"text\";\n}\n");
    let root = root.canonicalize().unwrap();
    let bad = root.join("examples/bad.rs");
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Build);
    pump(&mut app, "the build", |a| a.runner.build.as_ref().is_some_and(|b| b.result.is_some()));
    let start = app.insight.problems[&bad][0].start;
    assert_eq!(app.insight.problem_line(&app.docs, &bad, start), 2);
    app.open_file(&bad);
    assert_eq!(app.insight.problem_line(&app.docs, &bad, start), 2);
}

#[test]
fn external_libraries_come_from_the_lock_file_and_leave_it_alone() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nnear = { path = \"../near\" }\n\n[workspace]\n");
    let near = dir.path().join("near");
    write(&near.join("Cargo.toml"), "[package]\nname = \"near\"\nversion = \"0.2.0\"\nedition = \"2021\"\n");
    write(&near.join("src/lib.rs"), "pub fn near() {}\n");
    let root = root.canonicalize().unwrap();
    let near = near.canonicalize().unwrap();
    let mut app = new_app(dir.path());

    // Without a lock file nothing is resolved, and none is written.
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    let stdlib = |a: &mut CrabCraftApp| a.explorer.libraries.iter().any(|l| l.name == "stdlib");
    pump(&mut app, "the standard library", stdlib);
    assert_eq!(app.explorer.libraries.len(), 1, "{:?}", app.explorer.libraries);
    assert!(!root.join("Cargo.lock").exists());

    // Once locked, the dependencies appear and the file is untouched.
    let cargo = rc_cargo::Cargo::locate().expect("cargo is installed");
    assert!(cargo.std_command().args(["generate-lockfile", "--offline"]).current_dir(&root).status().unwrap().success());
    let locked = std::fs::read(root.join("Cargo.lock")).unwrap();
    app.reload_cargo();
    pump(&mut app, "the path dependency", |a| a.explorer.libraries.iter().any(|l| l.name == "near"));
    assert_eq!(std::fs::read(root.join("Cargo.lock")).unwrap(), locked);

    // The tree shows them when expanded.
    let name = "project".to_string();
    let rows = app.explorer.rows(None, &root, &name);
    assert!(rows.iter().any(|r| r.key == "libraries"));
    assert!(!rows.iter().any(|r| r.label == "near"));
    app.explorer.expanded.insert("libraries".into());
    app.explorer.expanded.insert(format!("dir:{}", near.display()));
    let rows = app.explorer.rows(None, &root, &name);
    assert!(rows.iter().any(|r| r.label == "near" && r.secondary == "0.2.0"));
    assert!(rows.iter().any(|r| r.label == "src"));

    // Downloaded sources are protected, a path dependency is the user's.
    let library = app.explorer.libraries.iter().find(|l| l.name == "stdlib").unwrap().root.clone();
    assert!(app.explorer.in_library(&library.join("core/src/lib.rs")));
    assert!(!app.explorer.in_library(&near.join("src/lib.rs")));
    app.open_file(&library.join("core/src/lib.rs"));
    assert!(app.docs.active_doc().unwrap().editor.read_only);
    app.open_file(&near.join("src/lib.rs"));
    assert!(!app.docs.active_doc().unwrap().editor.read_only);
}

#[test]
fn rustfmt_at_commit_covers_the_included_files_or_stops_the_commit() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/left.rs"), "fn  left( ){}\n");
    write(&root.join(".gitignore"), "target\n");
    let root = root.canonicalize().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").args(args).current_dir(&root).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "First"]);
    let (lib, left) = (root.join("src/lib.rs"), root.join("src/left.rs"));
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());

    let wait = |app: &mut CrabCraftApp, what: &str, done: &dyn Fn(&CrabCraftApp) -> bool| {
        let start = Instant::now();
        while !done(app) {
            assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for {what}; error {:?}", app.vcs.error);
            app.vcs.update(&mut app.docs, true);
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let changed = |app: &mut CrabCraftApp, paths: &'static [&'static str]| {
        app.vcs.refresh();
        wait(app, "the changes", &|a| paths.iter().all(|p| a.vcs.changes().iter().any(|c| c.path == *p)));
    };
    wait(&mut app, "the status", &|a| a.vcs.status.is_some());

    // Formatting is asked for: the included file is committed formatted, the excluded one stays.
    app.settings.format_on_commit = true;
    std::fs::write(&lib, "pub  fn  a( ){}\n").unwrap();
    std::fs::write(&left, "fn  left( ) {}\n").unwrap();
    changed(&mut app, &["src/lib.rs", "src/left.rs"]);
    app.vcs.excluded.insert("src/left.rs".into());
    app.vcs.message = "Formatted".into();
    app.commit_changes(false);
    wait(&mut app, "the commit", &|a| a.vcs.busy.is_none());
    assert_eq!(app.vcs.error, None);
    assert_eq!(git(&["show", "HEAD:src/lib.rs"]), "pub fn a() {}\n");
    assert_eq!(std::fs::read_to_string(&left).unwrap(), "fn  left( ) {}\n");
    app.vcs.excluded.clear();

    // A file rustfmt cannot read stops the commit and changes nothing.
    let head = git(&["rev-parse", "HEAD"]);
    std::fs::write(&lib, "pub fn broken( {\n").unwrap();
    changed(&mut app, &["src/lib.rs", "src/left.rs"]);
    app.vcs.message = "Must not exist".into();
    app.commit_changes(false);
    wait(&mut app, "the failure", &|a| a.vcs.busy.is_none());
    assert!(app.vcs.error.as_deref().is_some_and(|e| e.contains("lib.rs")), "{:?}", app.vcs.error);
    assert_eq!(git(&["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read_to_string(&left).unwrap(), "fn  left( ) {}\n");
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), "pub fn broken( {\n");

    // Not asked for: the text goes in as it is.
    app.settings.format_on_commit = false;
    app.vcs.message = "As it is".into();
    app.commit_changes(false);
    wait(&mut app, "the plain commit", &|a| a.vcs.busy.is_none());
    assert_eq!(app.vcs.error, None);
    assert_eq!(git(&["show", "HEAD:src/lib.rs"]), "pub fn broken( {\n");
}

#[test]
fn rustfmt_settings_apply_to_reformat_save_and_commit() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join(".gitignore"), "target\n");
    let root = root.canonicalize().unwrap();
    let (main, lib) = (root.join("src/main.rs"), root.join("src/lib.rs"));
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").args(args).current_dir(&root).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "First"]);
    write(&main, "fn main(){\nprintln!(\"x\");\n}\n");
    // Not trusted: no language server runs, and rustfmt still does.
    let mut app = new_app(dir.path());
    app.settings.rustfmt = rc_core::RustfmtSettings { args: "--config tab_spaces=3".into(), channel: "+stable".into(), env: "CRABCRAFT_RUSTFMT=1\n".into() };
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&main);
    let text = |a: &CrabCraftApp| a.docs.active_doc().map(|d| d.editor.buffer.rope().to_string()).unwrap_or_default();

    // Reformat Code: the document changes, the file waits for a save.
    app.run_insight_action(rc_editor::Action::Reformat, egui::Pos2::ZERO);
    pump(&mut app, "Reformat Code", |a| text(a) == "fn main() {\n   println!(\"x\");\n}\n");
    assert!(app.docs.active_doc().unwrap().is_modified());

    // On save: the formatted text is written.
    app.settings.format_on_save = true;
    let doc = app.docs.active_doc_mut().unwrap();
    let len = doc.editor.buffer.len_chars();
    doc.editor.replace(0, len, "fn main(){\nlet a=1;\n}\n");
    app.save_all_docs();
    pump(&mut app, "rustfmt on save", |_| std::fs::read_to_string(&main).is_ok_and(|t| t == "fn main() {\n   let a = 1;\n}\n"));
    assert!(!app.docs.active_doc().unwrap().is_modified());
    app.settings.format_on_save = false;

    // The environment reaches rustfmt: rustup reads this toolchain from it and fails.
    app.settings.rustfmt = rc_core::RustfmtSettings { env: "RUSTUP_TOOLCHAIN=crabcraft-missing".into(), ..Default::default() };
    let doc = app.docs.active_doc_mut().unwrap();
    doc.editor.replace(0, 0, "\n");
    let before = text(&app);
    app.run_insight_action(rc_editor::Action::Reformat, egui::Pos2::ZERO);
    pump(&mut app, "the rustfmt error", |a| a.notices.iter().any(|n| n.error && n.text.contains("main.rs") && n.text.contains("crabcraft-missing")));
    assert_eq!(text(&app), before);
    // Settings that cannot work are reported before rustfmt runs.
    app.settings.rustfmt = rc_core::RustfmtSettings { args: "--config 'tab_spaces=2".into(), ..Default::default() };
    app.run_insight_action(rc_editor::Action::Reformat, egui::Pos2::ZERO);
    assert_eq!(app.notices.last().map(|n| n.text.clone()), Some(crate::vcs::rustfmt_failure(rc_cargo::FormatError::Arguments)));
    assert!(app.rustfmt_jobs.is_empty());

    // At commit: the included file is committed formatted with the same settings.
    app.settings.rustfmt = rc_core::RustfmtSettings { args: "--config tab_spaces=3".into(), ..Default::default() };
    app.settings.format_on_commit = true;
    app.docs.active_doc_mut().unwrap().editor.replace(0, 1, "");
    std::fs::write(&lib, "pub fn a(){\nlet b=1;\n}\n").unwrap();
    let wait = |app: &mut CrabCraftApp, what: &str, done: &dyn Fn(&CrabCraftApp) -> bool| {
        let start = Instant::now();
        while !done(app) {
            assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for {what}; error {:?}", app.vcs.error);
            app.vcs.update(&mut app.docs, true);
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait(&mut app, "the status", &|a| a.vcs.status.is_some());
    app.vcs.refresh();
    wait(&mut app, "the changes", &|a| ["src/lib.rs", "src/main.rs"].iter().all(|p| a.vcs.changes().iter().any(|c| c.path == *p)));
    app.vcs.message = "Formatted".into();
    app.commit_changes(false);
    wait(&mut app, "the commit", &|a| a.vcs.busy.is_none());
    assert_eq!(app.vcs.error, None);
    assert_eq!(git(&["show", "HEAD:src/lib.rs"]), "pub fn a() {\n   let b = 1;\n}\n");

    // A bad setting stops the commit.
    app.settings.rustfmt.channel = "not a channel".into();
    std::fs::write(&lib, "pub fn c(){}\n").unwrap();
    app.vcs.refresh();
    wait(&mut app, "the change", &|a| a.vcs.changes().iter().any(|c| c.path == "src/lib.rs"));
    let head = git(&["rev-parse", "HEAD"]);
    app.vcs.message = "Must not exist".into();
    app.commit_changes(false);
    wait(&mut app, "the failure", &|a| a.vcs.busy.is_none());
    assert!(app.vcs.error.as_deref().is_some_and(|e| e.contains("not a channel")), "{:?}", app.vcs.error);
    assert_eq!(git(&["rev-parse", "HEAD"]), head);
}

#[test]
fn run_options_and_required_features_reach_the_target() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nextra = []\nmore = []\n\n[[bin]]\nname = \"demo\"\npath = \"src/main.rs\"\nrequired-features = [\"extra\"]\n\n[workspace]\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    let config = app.runner.configs.iter().find(|c| c.name == "demo").unwrap().clone();

    // Nothing was chosen: what the binary needs is still there.
    let (_, target) = app.runner.cargo_debug_target(&config).unwrap().unwrap();
    assert_eq!(target.options.features, ["extra"]);

    let options = rc_core::session::RunOptions { cargo_profile: " release ".into(), channel: "stable".into(), features: "more, extra".into(), backtrace: "full".into(), stdin_file: "in.txt".into(), ..Default::default() };
    app.runner.options.insert("demo".into(), options);
    let (_, target) = app.runner.cargo_debug_target(&config).unwrap().unwrap();
    assert_eq!(target.options, rc_cargo::CargoOptions { channel: "stable".into(), profile: "release".into(), features: vec!["more".into(), "extra".into()], backtrace: "full".into(), stdin_file: Some("in.txt".into()) });

    // It runs for real through the configuration.
    app.runner.options.clear();
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Run);
    pump(&mut app, "the program", |a| a.runner.runs.first().is_some_and(|r| !r.running()));
    pump(&mut app, "its output", |a| a.runner.runs[0].console.term.screen_lines().join("\n").contains("hello"));
}

#[test]
fn navigation_reaches_the_standard_library_and_compiler_fixes_apply() {
    use crate::insight::{InsightOutput, NavKind};
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let text = "trait Speak {\n    fn speak(&self);\n}\nstruct A;\nimpl Speak for A {\n    fn speak(&self) {}\n}\nfn main() {\n    let mut x = Vec::<u32>::new();\n    drop(x);\n    A.speak();\n}\n";
    write(&root.join("src/main.rs"), text);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&root.join("src/main.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    pump(&mut app, "the libraries", |a| a.explorer.libraries.iter().any(|l| l.name == "stdlib"));
    let i = app.docs.active.unwrap();

    // From the implementation of a method to the trait that declares it.
    let at = text.rfind("fn speak").unwrap() + 4;
    app.insight.navigate(&app.docs.list[i], at, NavKind::Super);
    let found = output(&mut app, "the trait", |o| match o {
        InsightOutput::Navigate(t) => Some(t.line),
        _ => None,
    });
    assert_eq!(found, 1);

    // Into the sources of the standard library, which open read-only.
    let at = text.find("Vec").unwrap() + 1;
    app.insight.navigate(&app.docs.list[i], at, NavKind::Declaration);
    let target = output(&mut app, "the declaration", |o| match o {
        InsightOutput::Navigate(t) => Some(t.clone()),
        _ => None,
    });
    assert!(target.path.ends_with("alloc/src/vec/mod.rs"), "{:?}", target.path);
    assert!(target.preview.contains("pub struct Vec"), "{:?}", target.preview);
    app.open_file(&target.path);
    assert!(app.docs.active_doc().unwrap().editor.read_only);
    app.docs.active = Some(i);

    // The suggestion of the compiler is offered and applied.
    pump(&mut app, "the warning", |a| a.docs.list[i].editor.diagnostics.iter().any(|d| d.message.contains("mutable")));
    app.docs.list[i].editor.selection = rc_text::Selection::point(text.find("mut x").unwrap() + 1);
    app.insight.code_actions(&app.docs.list[i]);
    let actions = output(&mut app, "the fixes", |o| match o {
        InsightOutput::Actions(a) => Some(a.clone()),
        _ => None,
    });
    let fix = actions.into_iter().find(|a| matches!(a, rc_lsp::lsp_types::CodeActionOrCommand::CodeAction(c) if c.title == "remove this `mut`")).expect("the compiler's fix");
    let mut out = Vec::new();
    app.insight.run_code_action(&mut app.docs, fix, &mut out);
    pump(&mut app, "the fix", |a| a.docs.list[i].editor.buffer.rope().to_string().contains("let x = Vec"));
    app.insight.stop();
}

#[test]
fn generate_offers_only_what_writes_code() {
    use crate::insight::InsightOutput;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let text = "struct A {\n    a: u32,\n}\nfn main() {\n    let n = 1 + 2;\n    drop(n);\n}\n";
    write(&root.join("src/main.rs"), text);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&root.join("src/main.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    let i = app.docs.active.unwrap();
    let title = |a: &rc_lsp::lsp_types::CodeActionOrCommand| match a {
        rc_lsp::lsp_types::CodeActionOrCommand::Command(c) => c.title.clone(),
        rc_lsp::lsp_types::CodeActionOrCommand::CodeAction(c) => c.title.clone(),
    };
    app.docs.list[i].editor.selection = rc_text::Selection::point(text.find("a: u32").unwrap());
    app.insight.generate_actions(&app.docs.list[i]);
    let found = output(&mut app, "the generators", |o| match o {
        InsightOutput::Actions(a) => Some(a.iter().map(title).collect::<Vec<_>>()),
        _ => None,
    });
    assert!(found.iter().any(|t| t == "Generate `new`"), "{found:?}");
    // The IDE's own Derive row comes first on a type.
    assert_eq!(found[0], "Derive…", "{found:?}");
    assert!(found[1..].iter().all(|t| t.starts_with("Generate") || t.starts_with("Implement") || t.starts_with("Add missing")), "{found:?}");
    // Deriving writes the attribute above the type, and a second time merges into it.
    for traits in [vec!["Debug", "Clone"], vec!["Debug", "Clone", "PartialEq"]] {
        let d = &mut app.docs.list[i];
        let t = crate::insight::derive_target_of(d).unwrap();
        let (a, b, new) = t.edit(&traits.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        d.editor.replace(a, b, &new);
        d.editor.parse_blocking();
    }
    let edited = app.docs.list[i].editor.buffer.rope().to_string();
    assert!(edited.starts_with("#[derive(Debug, Clone, PartialEq)]\nstruct A {"), "{edited}");
    app.docs.list[i].editor.reload(text);
    app.docs.list[i].editor.parse_blocking();

    // Refactor This offers Rename and the refactorings, and no generators.
    let at = text.find("1 + 2").unwrap();
    app.docs.list[i].editor.selection = rc_text::Selection::single(rc_text::Range::new(at, at + 5));
    app.insight.refactor_actions(&app.docs.list[i]);
    let found = output(&mut app, "the refactorings", |o| match o {
        InsightOutput::Actions(a) => Some(a.iter().map(title).collect::<Vec<_>>()),
        _ => None,
    });
    assert_eq!(found[0], "Rename…", "{found:?}");
    assert!(found.iter().any(|t| t == "Extract into variable"), "{found:?}");
    assert!(!found.iter().any(|t| t.starts_with("Generate")), "{found:?}");

    // Inside a body nothing is generated, and that is said.
    app.docs.list[i].editor.selection = rc_text::Selection::point(text.find("drop").unwrap());
    app.insight.generate_actions(&app.docs.list[i]);
    let said = output(&mut app, "the answer", |o| match o {
        InsightOutput::Message(m) => Some(m.clone()),
        InsightOutput::Actions(a) => Some(format!("{:?}", a.iter().map(title).collect::<Vec<_>>())),
        _ => None,
    });
    assert_eq!(said, "Nothing to generate here");

    // Alt+Enter is not filtered.
    app.insight.code_actions(&app.docs.list[i]);
    let all = output(&mut app, "the context actions", |o| match o {
        InsightOutput::Actions(a) => Some(a.len()),
        _ => None,
    });
    assert!(all > 0);
    app.insight.stop();
}

#[test]
fn the_call_hierarchy_is_a_tree_of_callers_and_callees() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let text = "fn leaf() {}\nfn middle() {\n    leaf();\n    leaf();\n}\nfn lonely() {}\nfn main() {\n    middle();\n    lonely();\n}\n";
    write(&root.join("src/main.rs"), text);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&root.join("src/main.rs"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    let i = app.docs.active.unwrap();
    // The labels of the open tree, as (depth, name), once `want` lines are loaded.
    let shown = |app: &CrabCraftApp| app.insight.call_tree.as_ref().map_or(Vec::new(), |t| t.rows().iter().map(|(p, n)| (p.len(), n.name.clone(), n.calls)).collect::<Vec<_>>());
    let ask = |app: &mut CrabCraftApp, name: &str, callers: bool| {
        app.insight.call_tree = None;
        app.insight.call_hierarchy(&app.docs.list[i], text.find(name).unwrap() + 3, callers);
        pump(app, "the first level", |a| a.insight.call_tree.as_ref().is_some_and(|t| t.root.children.is_some()));
    };
    let row = |depth: usize, name: &str, calls: usize| (depth, name.to_string(), calls);

    ask(&mut app, "fn leaf", true);
    assert!(app.insight.call_tree.as_ref().unwrap().callers);
    assert_eq!(shown(&app), [row(0, "leaf", 1), row(1, "middle", 2)]);
    assert_eq!(app.insight.call_tree.as_ref().unwrap().root.children.as_ref().unwrap()[0].target.as_ref().unwrap().line, 2);

    // Opening a caller loads who calls it, and a closed node hides them again.
    app.insight.toggle_call(&[0]);
    pump(&mut app, "the second level", |a| shown(a).len() == 3);
    assert_eq!(shown(&app)[2], row(2, "main", 1));
    app.insight.toggle_call(&[0]);
    assert_eq!(shown(&app).len(), 2);

    ask(&mut app, "fn main", false);
    assert_eq!(shown(&app), [row(0, "main", 1), row(1, "middle", 1), row(1, "lonely", 1)]);

    // The same function, in the other direction.
    app.insight.switch_calls();
    pump(&mut app, "the callers of main", |a| a.insight.call_tree.as_ref().is_some_and(|t| t.callers && t.root.children.is_some()));
    assert_eq!(shown(&app), [row(0, "main", 1)]);

    ask(&mut app, "fn leaf", false);
    assert_eq!(shown(&app), [row(0, "leaf", 1)]);
    app.insight.stop();
}

#[test]
fn a_new_project_opens_with_its_run_configuration() {
    use rc_cargo::new_project::{NewProject, Template};
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let request = NewProject { template: Template::Binary, name: "fresh".into(), location: dir.path().to_path_buf(), git: false };
    let cargo = rc_cargo::Cargo::locate().expect("cargo is installed");
    let root = rc_core::Runtime::block_on(request.create(&cargo)).unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    let config = app.runner.selected_config().unwrap();
    assert_eq!(config.name, "fresh");
    assert!(matches!(&config.kind, ConfigKind::Cargo { package, bin } if package == "fresh" && bin == "fresh"));
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Run);
    pump(&mut app, "the program", |a| a.runner.runs.first().is_some_and(|r| !r.running()));
    pump(&mut app, "its output", |a| a.runner.runs[0].console.term.screen_lines().join("\n").contains("Hello, world!"));
}

#[test]
fn a_new_crate_joins_the_open_workspace() {
    use rc_cargo::new_project::{NewProject, Template};
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    let packages = |a: &CrabCraftApp| a.runner.cargo_workspace.as_ref().map_or(0, |w| w.packages.len());
    assert_eq!(packages(&app), 1);

    assert!(rc_cargo::modules::in_workspace(&root));
    let request = NewProject { template: Template::Library, name: "member".into(), location: root.clone(), git: false };
    let cargo = rc_cargo::Cargo::locate().expect("cargo is installed");
    rc_core::Runtime::block_on(request.create(&cargo)).unwrap();
    assert!(std::fs::read_to_string(root.join("Cargo.toml")).unwrap().contains("member"));
    app.reload_workspace();
    pump(&mut app, "the new crate", |a| packages(a) == 2);
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
    app.trust_project_tools_for_test();
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

#[test]
fn a_warning_is_suppressed_for_its_statement_or_the_file() {
    use crate::insight::{InsightOutput, SUPPRESS_COMMAND};
    const SOURCE: &str = "fn main() {\n    let unused = 1;\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/main.rs"), SOURCE);
    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.rs").canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    // rustc's warning arrives with `cargo check` on save, named by its lint.
    app.insight.did_save(&main);
    pump(&mut app, "the warning of cargo check", |a| a.docs.active_doc().is_some_and(|d| d.editor.diagnostics.iter().any(|x| x.code.as_deref() == Some("unused_variables"))));
    let i = app.docs.active.unwrap();
    app.docs.list[i].editor.selection = rc_text::Selection::point(SOURCE.find("unused =").unwrap());
    app.insight.code_actions(&app.docs.list[i]);
    let rows = output(&mut app, "the context actions", |o| match o {
        InsightOutput::Actions(list) => Some(
            list.iter()
                .filter_map(|a| match a {
                    rc_lsp::lsp_types::CodeActionOrCommand::Command(c) if c.command == SUPPRESS_COMMAND => Some(c.title.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        ),
        _ => None,
    });
    assert_eq!(rows, vec!["Suppress `unused_variables` for the statement", "Suppress `unused_variables` for the file"]);
    let (a, b, text) = crate::insight::suppress_edit(&app.docs.list[i], "unused_variables", "item").unwrap();
    app.docs.list[i].editor.replace(a, b, &text);
    assert_eq!(app.docs.list[i].editor.buffer.rope().to_string(), "fn main() {\n    #[allow(unused_variables)]\n    let unused = 1;\n}\n");
    assert_eq!(crate::insight::suppress_edit(&app.docs.list[i], "unused_variables", "file").map(|e| e.2), Some("#![allow(unused_variables)]\n".into()));
    assert_eq!(crate::insight::suppress_edit(&app.docs.list[i], "unresolved-import", "item"), None);
    app.insight.stop();
}

/// Alt+F7 on one function, a click on another and Alt+F7 again: the Find window shows the
/// usages of the second, also when it has a single one.
#[test]
fn find_usages_again_on_another_function_replaces_the_results() {
    const SOURCE: &str = "fn first() {}
fn second() {}
fn third() {}

fn a() {
    first();
    first();
}
fn b() {
    second();
    second();
    second();
}
fn c() {
    third();
}

fn main() {
    a();
    b();
    c();
}
";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/main.rs"), SOURCE);
    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.rs");
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
    let lines = |a: &CrabCraftApp| a.find_results.as_ref().map(|(_, t)| t.iter().map(|t| t.line).collect::<Vec<_>>());
    // A click on the function's name in the rendered editor, then Alt+F7.
    let find = |h: &mut egui_kittest::Harness<'static, CrabCraftApp>, line: usize, want: Vec<usize>| {
        let i = h.state().docs.index_of(&main).unwrap();
        h.state_mut().docs.activate(i);
        h.state_mut().docs.list[i].editor.goto(line, 5);
        h.run_steps(3);
        let at = h.state().caret_screen.unwrap() + egui::vec2(1.0, -6.0);
        h.state_mut().docs.list[i].editor.goto(0, 0);
        h.input_mut().events.push(egui::Event::PointerMoved(at));
        h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
        h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(2);
        assert_eq!(h.state().docs.active_doc().map(|d| d.editor.caret_line_col().0), Some(line));
        h.key_press_modifiers(egui::Modifiers::ALT, egui::Key::F7);
        let start = Instant::now();
        while lines(h.state()).as_ref() != Some(&want) {
            assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for {want:?}, the window shows {:?}", lines(h.state()));
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    find(&mut h, 0, vec![5, 6]);
    // Another function: its usages replace those of the first.
    find(&mut h, 1, vec![9, 10, 11]);
    // A single usage lands in the window too instead of leaving the previous list there.
    find(&mut h, 2, vec![14]);
    assert!(h.state().dock.is_open(crate::toolwindow::ToolWindow::Find));
    h.state_mut().insight.stop();
}

/// An external tool runs a real program on the active file, its macros expanded, and its
/// output and exit code show in the Run window; Rerun starts it again.
#[cfg(unix)]
#[test]
fn an_external_tool_runs_on_the_active_file() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("my proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"tooling\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
    let mut app = new_app(dir.path());
    app.settings.external_tools = vec![rc_core::external_tools::ExternalTool { name: "Count lines".into(), program: "wc".into(), args: "-l $FileRelativePath$".into(), working_dir: String::new() }];
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    app.open_file(&root.join("src/main.rs"));
    app.run_external_tool("Count lines");
    pump(&mut app, "the tool", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    let run = &app.runner.runs[0];
    assert_eq!(run.name, "Count lines");
    assert_eq!(run.exit, Some(Some(0)));
    let text = run.console.term.screen_lines().join("\n");
    assert!(text.contains("3 src/main.rs"), "{text}");

    // A macro with no value stops it before anything starts.
    app.docs.list.clear();
    app.docs.active = None;
    app.run_external_tool("Count lines");
    assert_eq!(app.runner.runs.len(), 1);
    assert!(app.notices.last().is_some_and(|n| n.text.contains("$FileRelativePath$")), "{:?}", app.notices.last().map(|n| &n.text));
}

/// Settings › Tools asks rustup for updates and runs rustup commands in the Run window; the
/// toolchain versions are read again when a command ends.
#[test]
fn rustup_checks_updates_and_runs_commands() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let mut app = new_app(dir.path());
    if app.rustup().is_none() {
        eprintln!("rustup is not installed: skipped");
        return;
    }
    app.check_toolchain_updates();
    pump(&mut app, "rustup check", |a| {
        a.poll_toolchain_updates();
        !a.checking_toolchain_updates()
    });
    match app.toolchain_updates.as_ref().unwrap() {
        Ok(list) => assert!(list.iter().any(|u| u.name == "rustup"), "{list:?}"),
        // Offline machines get rustup's own error.
        Err(e) => assert!(!e.is_empty()),
    }

    app.toolchain = Some(Default::default());
    app.run_rustup(&["component", "list", "--installed"]);
    pump(&mut app, "the rustup command", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    let run = &app.runner.runs[0];
    assert_eq!(run.name, "rustup component list --installed");
    assert_eq!(run.exit, Some(Some(0)));
    assert!(run.console.term.screen_lines().join("\n").contains("cargo"));
    app.poll_toolchain_updates();
    assert!(app.toolchain.is_none(), "read again after a rustup command");
}

/// A run configuration's "before launch" tools run first, in order, with their output in the
/// Run window; one that fails stops the launch with its exit code.
#[cfg(unix)]
#[test]
fn before_launch_tools_run_first_and_a_failure_stops_the_run() {
    use rc_core::external_tools::ExternalTool;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {\n    println!(\"hello from main\");\n}\n").unwrap();
    let mut app = new_app(dir.path());
    app.settings.external_tools = vec![
        ExternalTool { name: "Greet".into(), program: "sh".into(), args: "-c 'echo prepared $FileName$'".into(), working_dir: String::new() },
        ExternalTool { name: "Fail".into(), program: "false".into(), args: String::new(), working_dir: String::new() },
    ];
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/main.rs"));
    let name = app.runner.configs[0].name.clone();
    app.runner.options.insert(name.clone(), rc_core::session::RunOptions { before_launch: vec!["Greet".into()], ..Default::default() });
    app.runner.selected = 0;
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Run);
    pump(&mut app, "the program", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    let text = app.runner.runs[0].console.term.screen_lines().join("\n");
    assert_eq!(app.runner.runs[0].exit, Some(Some(0)), "{text}");
    let (greet, main) = (text.find("prepared main.rs"), text.find("hello from main"));
    assert!(greet.is_some() && greet < main, "the tool runs before the program:\n{text}");

    app.runner.options.insert(name, rc_core::session::RunOptions { before_launch: vec!["Fail".into(), "Greet".into()], ..Default::default() });
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Run);
    pump(&mut app, "the stopped launch", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    let text = app.runner.runs[0].console.term.screen_lines().join("\n");
    assert!(text.contains("Fail failed with exit code 1"), "{text}");
    assert!(!text.contains("hello from main") && !text.contains("prepared"), "{text}");
}

/// "Allow multiple instances" keeps the running program and starts a numbered second one;
/// "Store as project file" writes the configuration into the project, and opening the project
/// again reads it from there.
#[cfg(unix)]
#[test]
fn run_configurations_allow_instances_and_live_in_the_project() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("server");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"server\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    // Runs until stopped.
    std::fs::write(root.join("src/main.rs"), "fn main() {\n    println!(\"up\");\n    std::thread::sleep(std::time::Duration::from_secs(60));\n}\n").unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    let name = app.runner.configs[0].name.clone();
    let options = rc_core::session::RunOptions { allow_parallel: true, in_project: true, args: "--port 1".into(), ..Default::default() };
    app.runner.options.insert(name.clone(), options.clone());
    rc_core::run_files::save(&root, &name, &options).unwrap();
    app.runner.selected = 0;
    for _ in 0..2 {
        app.run_ide_action(&egui::Context::default(), rc_editor::Action::Run);
        pump(&mut app, "the program", |a| a.runner.runs.last().is_some_and(|r| r.console.term.screen_lines().join("\n").contains("up")));
    }
    let names: Vec<_> = app.runner.runs.iter().map(|r| r.name.clone()).collect();
    assert_eq!(names, [name.clone(), format!("{name} (2)")]);
    assert!(app.runner.runs.iter().all(|r| r.running()));
    app.runner.stop_all();

    // Without the session's copy, the project's file still brings the configuration back.
    app.close_workspace();
    std::fs::remove_dir_all(rc_core::paths::sessions_dir()).unwrap();
    app.open_workspace(&root);
    let loaded = app.runner.options.get(&name).cloned().unwrap();
    assert!(loaded.in_project && loaded.allow_parallel);
    assert_eq!(loaded.args, "--port 1");
}

/// A gutter test run leaves a temporary configuration that Run starts again; at most five are
/// kept, Save makes one permanent (kept in the session), and reloading the Cargo model keeps
/// them.
#[test]
fn test_runs_become_temporary_configurations() {
    use crate::runner::ConfigKind;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "#[cfg(test)]\nmod tests {\n    #[test]\n    fn one() {}\n    #[test]\n    fn two() {}\n}\n").unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    let manifest = app.runner.cargo_workspace.as_ref().unwrap().manifest.clone();
    let filter = |n: &str| rc_dotnet::TestFilter::Tests(vec![n.to_string()]);

    app.runner.select_tests("one".into(), manifest.clone(), filter("tests::one"));
    assert_eq!(app.runner.selected_config().map(|c| c.name.as_str()), Some("one"));
    assert!(app.runner.is_temporary("one"));
    // The same tests again select the same configuration.
    app.runner.select_tests("one".into(), manifest.clone(), filter("tests::one"));
    assert_eq!(app.runner.temporary.len(), 1);

    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Run);
    pump(&mut app, "the test", |a| a.runner.tests.as_ref().is_some_and(|t| !t.running));
    let t = app.runner.tests.as_ref().unwrap();
    assert_eq!((t.passed, t.failed), (1, 0), "only tests::one runs: {:?}", t.results.keys());

    for i in 0..6 {
        app.runner.select_tests(format!("t{i}"), manifest.clone(), filter(&format!("tests::t{i}")));
    }
    assert_eq!(app.runner.temporary.len(), 5, "the oldest go");
    assert!(!app.runner.configs.iter().any(|c| c.name == "one"));

    app.runner.save_temporary("t5");
    assert!(!app.runner.is_temporary("t5"));
    app.reload_cargo();
    pump(&mut app, "the reload", |a| a.runner.cargo_workspace.is_some() && a.runner.configs.iter().any(|c| c.name == "t5"));
    assert!(app.runner.configs.iter().any(|c| c.name == "t4" && matches!(c.kind, ConfigKind::TestSelection { .. })), "temporary ones survive a reload");
    app.save_session();
    let session = rc_core::session::Session::load(&root);
    assert_eq!(session.test_configs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["t5"]);
}

/// "Emulate terminal" starts the program in a pseudo-terminal: it sees a terminal, reads what
/// is typed in the console and its exit code reaches the tab.
#[cfg(unix)]
#[test]
fn emulated_terminal_runs_see_a_tty_and_take_input() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tty");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"tty\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "use std::io::{BufRead, IsTerminal};\nfn main() {\n    println!(\"tty={}\", std::io::stdout().is_terminal());\n    let mut line = String::new();\n    std::io::stdin().lock().read_line(&mut line).unwrap();\n    println!(\"got {}\", line.trim());\n    std::process::exit(3);\n}\n",
    )
    .unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    let name = app.runner.configs[0].name.clone();
    app.runner.options.insert(name, rc_core::session::RunOptions { emulate_terminal: true, ..Default::default() });
    app.runner.selected = 0;
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Run);
    pump(&mut app, "the program in the terminal", |a| a.runner.runs.first().is_some_and(|r| r.is_terminal() && r.console.term.screen_lines().join("\n").contains("tty=")));
    assert!(app.runner.runs[0].console.term.screen_lines().join("\n").contains("tty=true"));
    app.runner.runs[0].console.term.write(b"crab\r".to_vec());
    pump(&mut app, "the end", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    let text = app.runner.runs[0].console.term.screen_lines().join("\n");
    assert!(text.contains("got crab"), "{text}");
    assert_eq!(app.runner.runs[0].exit, Some(Some(3)));
}

/// Rename › Preview lists rust-analyzer's edits with their lines; unchecking one leaves it
/// out of the refactoring.
#[test]
fn a_rename_preview_applies_only_the_checked_changes() {
    use crate::ui::dialogs::Dialog;
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

    let pos = MAIN.find("Greeter.hello").unwrap() + "Greeter.".len() + 1;
    let i = app.docs.active.unwrap();
    app.insight.rename(&app.docs.list[i], pos, "greet", Some("Rename".into()));
    pump(&mut app, "the preview", |a| match &mut a.dialog {
        Some(Dialog::RefactorPreview(p)) => {
            p.update();
            !p.loading()
        }
        _ => false,
    });
    let Some(Dialog::RefactorPreview(preview)) = &mut app.dialog else { unreachable!() };
    let lines: Vec<(String, String)> = preview.changes.iter().map(|c| (c.path.file_name().unwrap().to_string_lossy().into_owned(), c.after.trim().to_string())).collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines.contains(&("main.rs".into(), "println!(\"{}\", Greeter.greet());".into())), "{lines:?}");
    // Nothing changed yet.
    assert!(std::fs::read_to_string(root.join("src/lib.rs")).unwrap().contains("pub fn hello("));
    for c in &mut preview.changes {
        if c.after.contains("assert_eq!") {
            c.checked = false;
        }
    }
    let checked = preview.checked();
    crate::insight::apply_file_edits(&mut app.docs, checked).unwrap();
    let main_text = app.docs.active_doc().unwrap().editor.buffer.rope().to_string();
    assert!(main_text.contains("Greeter.greet()"), "{main_text}");
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains("pub fn greet(") && lib.contains("Greeter.hello(), \"hello\""), "the unchecked test line stays:\n{lib}");
}

/// Inspect Code runs Clippy over every member and target, also files never opened, and its
/// findings land in the Problems window's Inspection tab with their lint names.
#[test]
fn inspect_code_runs_clippy_over_the_whole_workspace() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    write(&root.join("Cargo.toml"), "[workspace]\nmembers = [\"app\", \"util\"]\nresolver = \"2\"\n");
    write(&root.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    write(&root.join("app/src/main.rs"), "fn main() {\n    let v = vec![1, 2];\n    println!(\"{}\", v.len() == 0);\n}\n");
    write(&root.join("util/Cargo.toml"), "[package]\nname = \"util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    write(&root.join("util/src/lib.rs"), "pub fn twice(x: &Vec<i32>) -> usize {\n    x.len() * 2\n}\n");
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::InspectCode);
    assert!(app.inspection.as_ref().is_some_and(|i| i.running));
    pump(&mut app, "the inspection", |a| {
        a.poll_inspection();
        a.inspection.as_ref().is_some_and(|i| !i.running)
    });
    let insp = app.inspection.as_ref().unwrap();
    assert_eq!(insp.failed, None);
    let codes: Vec<(&str, String)> = insp.diagnostics.iter().map(|d| (d.code.as_str(), d.file.as_ref().map(|f| f.display().to_string()).unwrap_or_default())).collect();
    assert!(codes.iter().any(|(c, f)| c.contains("len_zero") && f.ends_with("main.rs")), "{codes:?}");
    assert!(codes.iter().any(|(c, f)| c.contains("ptr_arg") && f.ends_with("lib.rs")), "{codes:?}");
    assert!(app.problems_tab_inspection && app.dock.is_open(crate::toolwindow::ToolWindow::Problems));
}

/// A compound configuration starts several Cargo binaries of the workspace together, and it
/// stays after the Cargo model is read again.
#[test]
fn a_compound_configuration_starts_several_cargo_binaries() {
    use crate::runner::ConfigKind;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("pair");
    write(&root.join("Cargo.toml"), "[package]\nname = \"pair\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    write(&root.join("src/bin/server.rs"), "fn main() {\n    println!(\"server up\");\n}\n");
    write(&root.join("src/bin/client.rs"), "fn main() {\n    println!(\"client up\");\n}\n");
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.app_names().len() == 2);
    app.runner.save_compound(None, "Both".into(), app.runner.app_names());
    assert!(matches!(app.runner.selected_config().map(|c| &c.kind), Some(ConfigKind::Compound { .. })));
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Run);
    pump(&mut app, "both programs", |a| a.runner.runs.len() == 2 && a.runner.runs.iter().all(|r| r.exit.is_some()));
    for (name, text) in [("server", "server up"), ("client", "client up")] {
        let r = app.runner.runs.iter().find(|r| r.name == name).unwrap();
        assert_eq!(r.exit, Some(Some(0)));
        assert!(r.console.term.screen_lines().join("\n").contains(text));
    }
    app.reload_cargo();
    pump(&mut app, "the reload", |a| a.runner.configs.iter().any(|c| c.name == "Both"));
}

/// A Cargo project nested too deep for rust-analyzer to find is analysed once attached (its
/// type error shows up), not before; the attachment is kept in the session.
#[test]
fn an_attached_cargo_project_is_analysed() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("app");
    write(&root.join("Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    write(&root.join("src/main.rs"), "fn main() {}\n");
    let gen_manifest = root.join("tools/gen/Cargo.toml");
    write(&gen_manifest, "[package]\nname = \"gen\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    write(&root.join("tools/gen/src/lib.rs"), "pub fn wrong() -> u32 {\n    \"text\"\n}\n");
    write(&root.join("tools/gen/src/main.rs"), "fn main() {\n    println!(\"generated\");\n}\n");
    let root = root.canonicalize().unwrap();
    let gen_manifest = gen_manifest.canonicalize().unwrap();
    let lib = root.join("tools/gen/src/lib.rs");
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    app.open_file(&lib);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    let mismatch = |a: &CrabCraftApp| a.docs.active_doc().is_some_and(|d| d.editor.diagnostics.iter().any(|x| x.message.contains("mismatched types") || x.code.as_deref() == Some("E0308")));
    // Give the server time to say what it knows about the file.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        pump(&mut app, "a frame", |_| true);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!mismatch(&app), "not analysed before it is attached");

    app.set_attached(&gen_manifest, true);
    pump(&mut app, "the analysis of the attached project", |a| mismatch(a));
    // Its binary is a run configuration that builds with its own manifest.
    pump(&mut app, "the attached project's targets", |a| a.runner.configs.iter().any(|c| c.name == "gen (gen)"));
    assert!(app.sync_log.entries.iter().any(|e| e.kind == crate::sync_log::SyncKind::Attached && e.log.success));
    std::fs::write(&lib, "pub fn right() -> u32 {\n    1\n}\n").unwrap();
    app.runner.selected = app.runner.configs.iter().position(|c| c.name == "gen (gen)").unwrap();
    app.run_ide_action(&egui::Context::default(), rc_editor::Action::Run);
    pump(&mut app, "the attached binary", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    let text = app.runner.runs[0].console.term.screen_lines().join("\n");
    assert!(text.contains("generated"), "{text}");
    app.save_session();
    assert_eq!(rc_core::session::Session::load(&root).attached_projects, ["tools/gen/Cargo.toml"]);
    app.set_attached(&gen_manifest, false);
    assert!(app.explorer.attached.is_empty());
    assert!(!app.runner.configs.iter().any(|c| c.name == "gen (gen)"));
}

/// Paths in Settings › Editor › Never import drop out of completion's import suggestions,
/// on a running rust-analyzer.
#[test]
fn excluded_paths_are_not_offered_for_import() {
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
    let paths = |c: &rc_editor::Completion| c.items.iter().filter(|i| i.label.starts_with("Ordering")).map(|i| format!("{} {}", i.label, i.detail)).collect::<Vec<_>>();

    let c = complete_at_end(&mut app, "fn main() {\n    let _o: Orderin|;\n}\n");
    let before = paths(&c);
    assert!(before.iter().any(|p| p.contains("atomic")) && before.iter().any(|p| p.contains("cmp")), "{before:?}");

    let mut settings = app.settings.clone();
    settings.autoimport_exclude = vec!["std::sync::atomic::Ordering".into(), "  ".into()];
    app.apply_settings(&egui::Context::default(), settings);
    let c = complete_at_end(&mut app, "fn main() {\n    let _o: Orderin|;\n}\n");
    let after = paths(&c);
    assert!(!after.iter().any(|p| p.contains("atomic")) && after.iter().any(|p| p.contains("cmp")), "{after:?}");
    app.insight.stop();
}

/// Type-like names of pasted code: not after `::` or `.`, not macros, strings or comments.
#[test]
fn pasted_names_are_the_type_like_ones() {
    let names: Vec<String> = crate::insight::pasted_names("let m: HashMap<String, Vec<u8>> = HashMap::new(); // Foo\nlet s = \"Bar\"; x.Baz; std::fmt::Display; Self; 'Life; vec![]; Some(1); println!(\"{}\", Ordering::Less);").into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["HashMap", "String", "Vec", "Some", "Ordering"]);
}

/// After a paste, the names with exactly one import get their `use` line; the ambiguous ones
/// are named instead.
#[test]
fn pasted_code_gets_its_unambiguous_imports() {
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

    let before = "use demo::Greeter;\n\nfn main() {\n    \n}\n";
    let pasted = "let m: HashMap<String, Ordering> = HashMap::new();";
    let i = app.docs.active.unwrap();
    let doc = &mut app.docs.list[i];
    let end = doc.editor.buffer.len_chars();
    doc.editor.replace(0, end, before);
    let at = before.find("    \n}").unwrap() + 4;
    doc.editor.replace(at, at, pasted);
    doc.editor.last_paste = Some((at, at + pasted.chars().count()));
    app.insight.on_editor_events(&mut app.docs.list[i], &[rc_editor::EditorEvent::Changed, rc_editor::EditorEvent::Pasted]);
    pump(&mut app, "the import", |a| a.docs.active_doc().is_some_and(|d| d.editor.buffer.rope().to_string().contains("use std::collections::HashMap;")));
    let text = app.docs.list[i].editor.buffer.rope().to_string();
    assert!(text.starts_with("use demo::Greeter;\nuse std::collections::HashMap;\n"), "{text}");
    assert!(!text.contains("Ordering;"), "two imports fit Ordering: none is chosen\n{text}");
    pump(&mut app, "the ambiguity message", |a| a.notices.iter().any(|n| n.text.contains("Ordering")));
    app.insight.stop();
}

/// Without imports, the `use` lines go after the file's leading comments and attributes.
#[test]
fn imports_go_after_the_leading_comments() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("lib.rs");
    write(&file, "//! Crate docs.\n#![allow(dead_code)]\n\nfn f() {}\n");
    let mut app = new_app(dir.path());
    app.open_file(&file);
    let i = app.docs.active.unwrap();
    let added = crate::insight::insert_rust_uses(&mut app.docs.list[i], &["std::fmt::Write".into(), "std::fmt::Write".into()]);
    assert_eq!(added, ["std::fmt::Write"]);
    assert_eq!(app.docs.list[i].editor.buffer.rope().to_string(), "//! Crate docs.\n#![allow(dead_code)]\n\nuse std::fmt::Write;\n\nfn f() {}\n");
    assert!(crate::insight::insert_rust_uses(&mut app.docs.list[i], &["std::fmt::Write".into()]).is_empty(), "already there");
}

/// A Rust scratch file is analysed on its own while a project is open (type hints arrive),
/// and Run builds it with rustc and runs it.
#[test]
fn rust_scratches_are_analysed_and_run() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    let scratch = rc_core::scratch::create(&rc_core::paths::scratches_dir(), rc_core::scratch::Kind::Rust).unwrap();
    std::fs::write(&scratch, "fn main() {\n    let x = 6u8;\n    let y = x * 7;\n    println!(\"{y}\");\n}\n").unwrap();
    app.open_file(&scratch);
    let i = app.docs.active.unwrap();
    assert!(app.docs.list[i].scratch);
    pump(&mut app, "the scratch's type hints", |a| a.docs.list.get(i).is_some_and(|d| !d.editor.inlays.is_empty()));

    let outs = app.runner.run_scratch(&scratch);
    crate::ui::run_windows::handle_outputs(&mut app, outs);
    pump(&mut app, "the scratch program", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    let text = app.runner.runs[0].console.term.screen_lines().join("\n");
    assert_eq!(app.runner.runs[0].exit, Some(Some(0)), "{text}");
    assert!(text.contains("42"), "{text}");

    // A compile error stops before running, with rustc's message.
    std::fs::write(&scratch, "fn main() { let x: u8 = \"no\"; }\n").unwrap();
    let outs = app.runner.run_scratch(&scratch);
    crate::ui::run_windows::handle_outputs(&mut app, outs);
    pump(&mut app, "the failed build", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    let text = app.runner.runs[0].console.term.screen_lines().join("\n");
    assert!(text.contains("mismatched types") && text.contains("rustc failed"), "{text}");
    app.insight.stop();
}

/// Reformat Code with a selection: rustfmt formats the file and only the selected function
/// changes.
#[test]
fn reformat_with_a_selection_changes_only_those_lines() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    let main = root.join("src/main.rs");
    let text = "fn main(){println!(\"a\");}\n\nfn other(){let x=1;let _y=x;}\n";
    write(&main, text);
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&main);
    let i = app.docs.active.unwrap();
    let start = text.find("fn other").unwrap();
    app.docs.list[i].editor.selection = rc_text::Selection::single(rc_text::Range::new(start, text.len() - 1));
    app.run_insight_action(rc_editor::Action::Reformat, egui::Pos2::ZERO);
    pump(&mut app, "rustfmt", |a| a.docs.list[i].editor.buffer.rope().to_string() != text);
    assert_eq!(app.docs.list[i].editor.buffer.rope().to_string(), "fn main(){println!(\"a\");}\n\nfn other() {\n    let x = 1;\n    let _y = x;\n}\n");
}

/// Code that a `cfg` turns off is faded in the editor: the target comes from `rustc --print
/// cfg` and the features from the manifest's `default`. It needs no language server.
#[test]
fn code_disabled_by_cfg_is_faded() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ndefault = [\"std\"]\nstd = []\nextra = []\n\n[workspace]\n");
    let lib = "#[cfg(feature = \"extra\")]\npub fn extra() -> u32 {\n    1\n}\n\n#[cfg(feature = \"std\")]\npub fn with_std() {}\n\n#[cfg(not(any(unix, windows)))]\npub fn elsewhere() {}\n\npub fn always() -> u32 {\n    2\n}\n";
    write(&root.join("src/lib.rs"), lib);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    app.open_file(&root.join("src/lib.rs"));
    let faded = |a: &mut CrabCraftApp| -> Vec<String> {
        a.update_cfg_sets();
        let doc = a.docs.active_doc_mut().unwrap();
        doc.editor.syntax.parse_blocking(&doc.editor.buffer.rope().clone(), doc.editor.buffer.version());
        let text = doc.editor.buffer.rope().to_string();
        doc.editor.inactive().iter().map(|r| text.chars().skip(r.start).take(r.end - r.start).collect()).collect()
    };
    pump(&mut app, "the cfg of the crate", |a| {
        a.update_cfg_sets();
        a.docs.active_doc().is_some_and(|d| d.editor.cfg.is_some())
    });
    assert_eq!(faded(&mut app), vec!["#[cfg(feature = \"extra\")]\npub fn extra() -> u32 {\n    1\n}".to_string(), "#[cfg(not(any(unix, windows)))]\npub fn elsewhere() {}".to_string()]);
    // Typing keeps the faded range on the item.
    let doc = app.docs.active_doc_mut().unwrap();
    doc.editor.replace(0, 0, "// note\n");
    assert!(faded(&mut app)[0].starts_with("#[cfg(feature = \"extra\")]"));
    // Trusted project tools; this fixture deliberately did not start a language server.
    assert!(app.insight.session.is_none());
}

/// A feature turned on from the gutter of `Cargo.toml` stops the fading of its code, is saved
/// with the session and reaches rust-analyzer, whose build script run sees it.
#[test]
fn a_feature_turned_on_in_the_gutter_reaches_the_fading_and_rust_analyzer() {
    use rc_editor::GutterIcon;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ndefault = [\"std\"]\nstd = []\nextra = []\n\n[workspace]\n");
    write(&root.join("src/lib.rs"), "#[cfg(feature = \"extra\")]\npub fn extra() -> u32 {\n    1\n}\n\npub fn always() -> u32 {\n    2\n}\n");
    // Build scripts see the enabled features as `CARGO_FEATURE_*`.
    write(&root.join("build.rs"), "fn main() {\n    std::fs::write(\"BUILD_FEATURES\", format!(\"extra={}\", std::env::var(\"CARGO_FEATURE_EXTRA\").is_ok())).unwrap();\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    app.open_file(&root.join("src/lib.rs"));
    pump(&mut app, "the cfg of the crate", |a| {
        a.update_cfg_sets();
        a.docs.active_doc().is_some_and(|d| d.editor.cfg.is_some())
    });
    let faded = |a: &mut CrabCraftApp| -> usize {
        a.update_cfg_sets();
        let doc = a.docs.active_doc_mut().unwrap();
        doc.editor.syntax.parse_blocking(&doc.editor.buffer.rope().clone(), doc.editor.buffer.version());
        doc.editor.inactive().len()
    };
    assert_eq!(faded(&mut app), 1, "extra is off");

    app.open_file(&root.join("Cargo.toml"));
    app.update_feature_icons();
    let icons = |a: &CrabCraftApp| a.docs.active_doc().unwrap().editor.gutter_icons.clone();
    // default, std (on through default) and extra (off), on lines 6 to 8.
    assert_eq!(icons(&app).into_iter().collect::<Vec<_>>(), vec![(6, GutterIcon::FeatureOn), (7, GutterIcon::FeatureOn), (8, GutterIcon::FeatureOff)]);
    // A feature that default turns on stays on.
    app.toggle_feature_at(7);
    assert!(app.cargo_features.is_empty());
    app.toggle_feature_at(8);
    assert_eq!(app.cargo_features, vec!["demo/extra".to_string()]);
    app.update_feature_icons();
    assert_eq!(icons(&app).get(&8), Some(&GutterIcon::FeatureOn));

    app.open_file(&root.join("src/lib.rs"));
    assert_eq!(faded(&mut app), 0, "extra is on");
    app.save_session();
    assert_eq!(rc_core::Session::load(&root).cargo_features, vec!["demo/extra".to_string()]);

    // Trusted, rust-analyzer builds with the feature.
    app.trust_project();
    pump(&mut app, "the build script with extra", |_| std::fs::read_to_string(root.join("BUILD_FEATURES")).is_ok_and(|t| t == "extra=true"));
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    // Turned off, the running server reloads the workspace without restarting.
    let started = app.insight.session.as_ref().map(|s| s.started);
    std::fs::remove_file(root.join("BUILD_FEATURES")).unwrap();
    app.open_file(&root.join("Cargo.toml"));
    app.toggle_feature_at(8);
    assert!(app.cargo_features.is_empty());
    assert_eq!(app.insight.session.as_ref().map(|s| s.started), started);
    pump(&mut app, "the build script without extra", |_| std::fs::read_to_string(root.join("BUILD_FEATURES")).is_ok_and(|t| t == "extra=false"));
    app.insight.stop();

    // A feature removed from the manifest is dropped once the project is read again.
    app.set_cargo_feature("demo", "extra", true);
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ndefault = [\"std\"]\nstd = []\n\n[workspace]\n");
    app.reload_cargo();
    pump(&mut app, "the feature dropped", |a| a.cargo_features.is_empty());
}

/// The target picked in the status bar list flips the cfg fading (`windows` on, `unix` off), is
/// saved with the session and reaches rust-analyzer as `cargo.target`, whose build script run
/// sees it as `TARGET`, live.
#[test]
fn a_target_picked_in_the_status_bar_reaches_the_fading_and_rust_analyzer() {
    const WINDOWS: &str = "x86_64-pc-windows-msvc";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/lib.rs"), "#[cfg(windows)]\npub fn on_windows() {}\n\n#[cfg(unix)]\npub fn on_unix() {}\n\npub fn always() -> u32 {\n    2\n}\n");
    write(&root.join(".cargo/config.toml"), "[build]\n# a board\ntarget = \"thumbv7em-none-eabihf\"\n");
    write(&root.join("build.rs"), "fn main() {\n    std::fs::write(\"BUILD_TARGET\", std::env::var(\"TARGET\").unwrap()).unwrap();\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    app.open_file(&root.join("src/lib.rs"));
    let faded = |a: &mut CrabCraftApp| -> Vec<String> {
        a.update_cfg_sets();
        let doc = a.docs.active_doc_mut().unwrap();
        doc.editor.syntax.parse_blocking(&doc.editor.buffer.rope().clone(), doc.editor.buffer.version());
        let text = doc.editor.buffer.rope().to_string();
        doc.editor.inactive().iter().map(|r| text.chars().skip(r.start).take(r.end - r.start).collect()).collect()
    };
    pump(&mut app, "the cfg and the targets", |a| {
        a.update_cfg_sets();
        a.docs.active_doc().is_some_and(|d| d.editor.cfg.is_some()) && !a.triple_choices.is_empty()
    });
    // The project builds for a board by default: neither unix nor windows.
    pump(&mut app, "the cfg of the board", |a| faded(a).len() == 2);
    assert_eq!(app.analysis_triple(), Some("thumbv7em-none-eabihf"));
    // The list: the host first, then the project's target, then rustup's.
    let host = app.host_triple.clone().expect("rustc -vV gives the host");
    assert!(app.triple_choices[0].host && app.triple_choices[0].installed && app.triple_choices[0].triple == host);
    assert!(app.triple_choices[1].from_config && app.triple_choices[1].triple == "thumbv7em-none-eabihf");
    let windows = app.triple_choices.iter().find(|c| c.triple == WINDOWS).expect("rustup lists the targets it can install");
    assert!(!windows.installed);

    // An invalid triple never reaches a process.
    app.set_cargo_target(Some("--print=sysroot".into()));
    assert_eq!(app.cargo_target, None);
    app.set_cargo_target(Some(WINDOWS.into()));
    assert_eq!(app.analysis_triple(), Some(WINDOWS));
    pump(&mut app, "the cfg of Windows", |a| faded(a) == vec!["#[cfg(unix)]\npub fn on_unix() {}".to_string()]);
    app.save_session();
    assert_eq!(rc_core::Session::load(&root).cargo_target, WINDOWS);

    // Opened again, the session brings the target back.
    let mut again = new_app(dir.path());
    again.open_workspace(&root);
    assert_eq!(again.cargo_target.as_deref(), Some(WINDOWS));
    drop(again);

    // Trusted, rust-analyzer analyses for the target: its build script run sees it.
    app.trust_project();
    pump(&mut app, "the build script for Windows", |_| std::fs::read_to_string(root.join("BUILD_TARGET")).is_ok_and(|t| t == WINDOWS));
    let settings = app.insight.session.as_ref().map(|s| s.ra_settings().clone()).unwrap();
    assert_eq!(rc_lsp::rust_analyzer::configuration_reply(&serde_json::json!({ "items": [{ "section": "rust-analyzer" }] }), &settings)[0]["cargo"]["target"], serde_json::json!(WINDOWS));
    // The host, the running server reloads the workspace without restarting.
    let started = app.insight.session.as_ref().map(|s| s.started);
    std::fs::remove_file(root.join("BUILD_TARGET")).unwrap();
    app.set_cargo_target(Some(host.clone()));
    assert_eq!(app.cargo_target.as_deref(), Some(host.as_str()));
    assert_eq!(app.insight.session.as_ref().map(|s| s.started), started);
    assert_eq!(app.insight.session.as_ref().and_then(|s| s.ra_settings().target.clone()), Some(host.clone()));
    pump(&mut app, "the build script for the host", |_| std::fs::read_to_string(root.join("BUILD_TARGET")).is_ok_and(|t| t == host));
    pump(&mut app, "the cfg of the host", |a| faded(a) == vec!["#[cfg(windows)]\npub fn on_windows() {}".to_string()]);
    // Back to the default (the board): nothing is sent, rust-analyzer reads the config.
    app.set_cargo_target(None);
    assert_eq!(app.insight.session.as_ref().and_then(|s| s.ra_settings().target.clone()), None);
    pump(&mut app, "the cfg of the board again", |a| faded(a).len() == 2);
    app.insight.stop();
}

/// A crate whose features are used in several files, for N-13.
pub(crate) fn feature_crate(root: &Path) {
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ndefault = [\"std\"]\nstd = []\nextra = []\nfull = [\"std\", \"extra\"]\n\n[workspace]\n");
    write(&root.join("src/lib.rs"), "#[cfg(feature = \"extra\")]\npub mod extra;\n\n#[cfg(all(feature = \"std\", not(feature = \"extra\")))]\npub fn plain() {}\n\npub fn always() -> bool {\n    cfg!(feature = \"extra\")\n}\n");
    write(&root.join("src/extra.rs"), "#[cfg_attr(feature = \"extra\", derive(Debug))]\npub struct Extra;\n\n// #[cfg(feature = \"extra\")] in a comment does not count\n");
    write(&root.join("tests/it.rs"), "#[test]\n#[cfg(any(windows, feature = \"extra\"))]\nfn it() {}\n");
    // Not part of the package's sources: the build output.
    write(&root.join("target/gen.rs"), "#[cfg(feature = \"extra\")]\nfn generated() {}\n");
}

/// Go to Declaration and Find Usages between Cargo features and their `cfg` uses (N-13): from
/// a `[features]` key to every `cfg` that names it, from `"extra"` of a `cfg` to its line in
/// `Cargo.toml`, and from a feature another one turns on to its own line. No server needed.
#[test]
fn cargo_features_and_their_cfg_uses_lead_to_each_other() {
    use rc_editor::Action;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    feature_crate(&root);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    let manifest = root.join("Cargo.toml");
    let caret_on = |app: &mut CrabCraftApp, line: usize, col: usize| app.docs.active_doc_mut().unwrap().editor.goto(line, col);
    let here = |app: &CrabCraftApp| {
        let d = app.docs.active_doc().unwrap();
        (d.path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"), d.editor.caret_line_col().0)
    };
    let uses = |app: &mut CrabCraftApp, title: &str| {
        let title = title.to_string();
        pump(app, &title.clone(), move |a| {
            a.poll_feature_search();
            a.find_results.as_ref().is_some_and(|(t, _)| *t == title)
        });
        let (_, targets) = app.find_results.clone().unwrap();
        targets
    };

    // From the `extra = []` key: its uses, from a scan in the background.
    app.open_file(&manifest);
    caret_on(&mut app, 8, 2);
    app.run_insight_action(Action::GotoDeclaration, egui::Pos2::ZERO);
    assert!(app.feature_search.busy());
    let targets = uses(&mut app, "Uses of feature extra");
    let got: Vec<(String, usize)> = targets.iter().map(|t| (t.path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"), t.line)).collect();
    assert_eq!(got, vec![("src/extra.rs".into(), 0), ("src/lib.rs".into(), 0), ("src/lib.rs".into(), 3), ("src/lib.rs".into(), 7), ("tests/it.rs".into(), 1)]);
    assert_eq!(targets[2].preview, "#[cfg(all(feature = \"std\", not(feature = \"extra\")))]");
    assert!(app.dock.is_open(crate::toolwindow::ToolWindow::Find));
    // A row opens its place, as a click in the Find window does.
    let t = targets[4].clone();
    app.open_at(&t.path, t.line, t.col);
    assert_eq!(here(&app), ("tests/it.rs".into(), 1));

    // Find Usages on `"std"` in the array of `full`; Go to Declaration there jumps to `std = []`.
    app.open_file(&manifest);
    caret_on(&mut app, 9, 10);
    app.run_insight_action(Action::FindUsages, egui::Pos2::ZERO);
    assert_eq!(uses(&mut app, "Uses of feature std").len(), 1);
    caret_on(&mut app, 9, 10);
    app.run_insight_action(Action::GotoDeclaration, egui::Pos2::ZERO);
    assert_eq!(here(&app), ("Cargo.toml".into(), 7));
    // Back returns to where the jump started.
    app.navigate(true);
    assert_eq!(here(&app), ("Cargo.toml".into(), 9));

    // From `"extra"` in a `cfg` of a Rust file to `extra = []`, with no language server.
    app.open_file(&root.join("src/lib.rs"));
    caret_on(&mut app, 3, 44);
    let pos = app.docs.active_doc().unwrap().editor.caret();
    assert!(crate::feature_nav::handles_click(app.docs.active_doc().unwrap(), pos), "⌃+click is answered locally");
    app.run_insight_action(Action::GotoDeclaration, egui::Pos2::ZERO);
    assert_eq!(here(&app), ("Cargo.toml".into(), 8));
    assert!(app.insight.session.is_none());
    // Elsewhere in a Rust file the server keeps the click.
    app.open_file(&root.join("src/lib.rs"));
    caret_on(&mut app, 6, 8);
    let pos = app.docs.active_doc().unwrap().editor.caret();
    assert!(!crate::feature_nav::handles_click(app.docs.active_doc().unwrap(), pos));
    // Find Usages on a `cfg!` string lists the feature's uses too.
    caret_on(&mut app, 7, 21);
    app.find_results = None;
    app.run_insight_action(Action::FindUsages, egui::Pos2::ZERO);
    assert_eq!(uses(&mut app, "Uses of feature extra").len(), 5);

    // A feature that is not declared stays where it is.
    app.docs.active_doc_mut().unwrap().editor.replace(0, 0, "#[cfg(feature = \"missing\")]\nfn m() {}\n");
    caret_on(&mut app, 0, 18);
    app.run_insight_action(Action::GotoDeclaration, egui::Pos2::ZERO);
    assert_eq!(here(&app), ("src/lib.rs".into(), 0));
    assert!(!root.join("target/debug").exists(), "nothing was built");
}

#[test]
fn the_new_project_dialog_creates_procedural_macro_and_webassembly_projects() {
    use egui_kittest::kittest::Queryable;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let location = dir.path().canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.new_project_location = Some(location.clone());
    app.new_project_dialog(None);
    let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
    h.run_steps(2);
    let wait = |h: &mut egui_kittest::Harness<'static, CrabCraftApp>, what: &str, done: &dyn Fn(&mut egui_kittest::Harness<'static, CrabCraftApp>) -> bool| {
        let start = Instant::now();
        while !done(h) {
            assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(30));
        }
    };

    // WebAssembly: the missing tools are named, nothing is installed.
    h.get_by_label("WebAssembly").click();
    h.run_steps(2);
    wait(&mut h, "the WebAssembly tools", &|h| h.query_by_label("Looking for the WebAssembly tools…").is_none());
    let cargo = rc_cargo::Cargo::locate().expect("cargo is installed");
    let tools = rc_cargo::new_project::WasmTools::detect(&cargo);
    let target_note = "The wasm32-unknown-unknown target is not installed. Add it with: rustup target add wasm32-unknown-unknown";
    assert_eq!(h.query_by_label(target_note).is_some(), !tools.target);
    assert_eq!(h.query_by_label("wasm-pack was not found: install it to package the library for JavaScript").is_some(), !tools.wasm_pack);

    // Procedural Macro: created in the background, then opened.
    h.get_by_label("Procedural Macro").click();
    h.run_steps(2);
    h.get_by_label("Create").click();
    let project = location.join("project1");
    wait(&mut h, "the new project", &|h| h.state().new_project.is_none() && h.state().workspace.is_some());
    assert!(h.state().runner.cargo_workspace.is_none());
    h.state_mut().trust_project_tools_for_test();
    wait(&mut h, "the explicitly trusted project model", &|h| h.state().runner.cargo_workspace.is_some());
    let manifest = std::fs::read_to_string(project.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("proc-macro = true") && manifest.contains("syn = "), "{manifest}");
    assert!(project.join("tests/describe.rs").is_file());
    assert_eq!(h.state().docs.active_doc().map(|d| d.path.clone()), Some(project.join("src/lib.rs")));
}

/// Before a rename is made, its conflicts are listed (R-04): a name already used in a file
/// the rename touches, a keyword. "Continue anyway" makes it (or previews it); a rename
/// without conflicts is made at once.
#[test]
fn a_rename_lists_its_conflicts_before_it_is_made() {
    use crate::ui::dialogs::Dialog;
    const LIB: &str = "pub struct Greeter;\n\nimpl Greeter {\n    pub fn hello(&self) -> &'static str {\n        \"hello\"\n    }\n\n    pub fn bye(&self) -> &'static str {\n        \"bye\"\n    }\n}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/lib.rs"), LIB);
    let root = root.canonicalize().unwrap();
    let lib = root.join("src/lib.rs");
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    let main = root.join("src/main.rs");
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready);
    let pos = MAIN.find("Greeter.hello").unwrap() + "Greeter.".len() + 1;
    let i = app.docs.active.unwrap();
    let conflicts = |app: &mut CrabCraftApp, name: &str, preview: Option<String>| {
        app.dialog = None;
        app.insight.rename(&app.docs.list[i], pos, name, preview);
        pump(app, "the conflicts", |a| matches!(a.dialog, Some(Dialog::RefactorConflicts(_))));
        let Some(Dialog::RefactorConflicts(review)) = app.dialog.take() else { unreachable!() };
        review
    };

    // A name another method of the type already has.
    let review = conflicts(&mut app, "bye", None);
    let found: Vec<(Option<&Path>, usize, &str)> = review.conflicts.iter().map(|c| (c.path.as_deref(), c.line, c.message.as_str())).collect();
    assert_eq!(found, [(Some(lib.as_path()), 7, "'bye' is already used here")], "{review:#?}");
    assert!(std::fs::read_to_string(&lib).unwrap().contains("pub fn hello("), "nothing is renamed before the user continues");

    // A keyword: listed, whether or not rust-analyzer accepts it.
    let review = conflicts(&mut app, "fn", None);
    assert!(review.conflicts.iter().any(|c| c.path.is_none() && c.message == "'fn' is a keyword"), "{review:#?}");
    assert!(std::fs::read_to_string(&lib).unwrap().contains("pub fn hello("));

    // Continue anyway with a preview: the preview opens with the edits.
    let review = conflicts(&mut app, "bye", Some("Rename".into()));
    app.continue_refactoring(*review);
    assert!(matches!(app.dialog, Some(Dialog::RefactorPreview(_))));

    // Without conflicts nothing is asked: the preview of a rename to a new name opens at once.
    app.dialog = None;
    app.insight.rename(&app.docs.list[i], pos, "farewell", Some("Rename".into()));
    pump(&mut app, "the preview", |a| a.dialog.is_some());
    assert!(matches!(app.dialog, Some(Dialog::RefactorPreview(_))));

    // Continue anyway: the rename is made.
    let review = conflicts(&mut app, "bye", None);
    app.continue_refactoring(*review);
    let main_text = app.docs.active_doc().unwrap().editor.buffer.rope().to_string();
    assert!(main_text.contains("Greeter.bye()"), "{main_text}");
    assert_eq!(std::fs::read_to_string(&lib).unwrap().matches("pub fn bye(").count(), 2);
    assert!(app.dialog.is_none());
}

/// C-10: `cargo check` (run by rust-analyzer) says what a feature hides; Alt+Enter turns it on.
/// A dependency's feature goes into its `features` in `Cargo.toml` (undoable, in the editor), the
/// package's own into the analysis or `default`; with the fixes the errors go away. Uses tokio
/// from the local Cargo cache, offline.
#[test]
fn a_name_behind_a_feature_gets_a_fix_that_turns_the_feature_on() {
    use crate::feature_fix::FeatureFix;
    use rc_editor::Action;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    const MANIFEST: &str = "[package]\nname = \"gated\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\nextra = []\n\n[dependencies]\ntokio = { version = \"1\", default-features = false }\n\n[workspace]\n";
    write(&root.join("Cargo.toml"), MANIFEST);
    write(&root.join("src/main.rs"), "use tokio::fs::File;\n\n#[cfg(feature = \"extra\")]\nfn helper() -> u32 {\n    1\n}\n\nfn main() {\n    let _ = helper();\n    let _f: Option<File> = None;\n}\n");
    let locked = std::process::Command::new("cargo").args(["generate-lockfile", "--offline"]).current_dir(&root).output().is_ok_and(|o| o.status.success());
    if !locked {
        eprintln!("skipped: tokio is not in the local Cargo cache");
        return;
    }
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    let main = root.join("src/main.rs");
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready && a.runner.cargo_workspace.is_some());
    let has = |a: &CrabCraftApp, code: &str| a.docs.list.iter().find(|d| d.path == main).is_some_and(|d| d.editor.diagnostics.iter().any(|x| x.code.as_deref() == Some(code)));
    // `cargo check` runs on save.
    app.insight.did_save(&main);
    pump(&mut app, "cargo check's errors", |a| has(a, "E0432") && has(a, "E0425") && a.insight.feature_gates.get(&main).is_some_and(|g| g.len() == 2));
    let i = app.docs.index_of(&main).unwrap();
    let manifest = root.join("Cargo.toml");

    // On the import: tokio's `fs` feature, in the dependency.
    app.docs.list[i].editor.goto(0, 12);
    let tokio = FeatureFix::Dependency { manifest: manifest.clone(), key: "tokio".into(), name: "tokio".into(), feature: "fs".into() };
    assert_eq!(app.feature_fixes(i), vec![tokio.clone()]);
    // Alt+Enter lists it first, before the server's actions.
    app.run_insight_action(Action::ShowIntentions, egui::Pos2::ZERO);
    pump(&mut app, "the actions", |a| matches!(a.dialog, Some(crate::ui::dialogs::Dialog::CodeActions { .. })));
    let Some(crate::ui::dialogs::Dialog::CodeActions { list, .. }) = &app.dialog else { unreachable!() };
    let first = match &list[0] {
        rc_lsp::lsp_types::CodeActionOrCommand::CodeAction(a) => a.title.clone(),
        rc_lsp::lsp_types::CodeActionOrCommand::Command(c) => c.title.clone(),
    };
    assert_eq!(first, "Add feature `fs` to the tokio dependency");
    app.dialog = None;
    app.apply_feature_fix(tokio);
    let edited = MANIFEST.replace("default-features = false }", "default-features = false, features = [\"fs\"] }");
    let text = |a: &CrabCraftApp| a.docs.active_doc().unwrap().editor.buffer.rope().to_string();
    assert_eq!(app.docs.active_doc().unwrap().path, manifest);
    assert_eq!(text(&app), edited);
    // Undoable, and not saved for the user.
    app.docs.active_doc_mut().unwrap().editor.undo();
    assert_eq!(text(&app), MANIFEST);
    app.docs.active_doc_mut().unwrap().editor.redo();
    assert_eq!(text(&app), edited);
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), MANIFEST);

    // On the call: the package's own `extra`, for the analysis or in `default`.
    app.open_file(&main);
    app.docs.list[i].editor.goto(8, 12);
    let fixes = app.feature_fixes(i);
    assert_eq!(fixes, vec![FeatureFix::Analysis { package: "gated".into(), feature: "extra".into() }, FeatureFix::Default { manifest: manifest.clone(), package: "gated".into(), feature: "extra".into() }]);
    app.apply_feature_fix(fixes[0].clone());
    assert_eq!(app.cargo_features, vec!["gated/extra".to_string()]);
    // Once on, it is not offered again.
    assert!(app.feature_fixes(i).is_empty());

    // Saved, `cargo check` with both features finds neither error.
    app.save_all_docs();
    app.insight.did_save(&main);
    pump(&mut app, "the errors to go", |a| !has(a, "E0432") && !has(a, "E0425") && a.insight.feature_gates.get(&main).is_none());
    app.insight.stop();
}

/// D-27: a breakpoint in a file that is closed stays set and stops the program; groups turn
/// their breakpoints off together; a change made in the list reaches a running session; the
/// list, with groups and closed files, comes back with the session.
#[test]
fn breakpoints_of_closed_files_stop_and_groups_turn_on_and_off_together() {
    use rc_editor::{Action, BreakpointKind};
    if rc_dap::gdb::locate().is_none() {
        eprintln!("skipped: GDB is not installed");
        return;
    }
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), "mod price;\n\nfn main() {\n    let a = price::double(2);\n    let b = price::double(a);\n    println!(\"{b}\");\n}\n");
    write(&root.join("src/price.rs"), "pub fn double(n: u32) -> u32 {\n    let twice = n * 2;\n    twice\n}\n");
    let root = root.canonicalize().unwrap();
    let (main_rs, price_rs) = (root.join("src/main.rs"), root.join("src/price.rs"));
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());

    // A breakpoint in price.rs, which is then closed: it stays.
    app.open_file(&price_rs);
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(1, BreakpointKind::Normal);
    let i = app.docs.index_of(&price_rs).unwrap();
    app.docs.close(i);
    assert_eq!(app.docs.closed_breakpoints.get(&price_rs).and_then(|b| b.get(&1)), Some(&BreakpointKind::Normal));
    app.open_file(&main_rs);
    for line in [3, 4] {
        app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(line, BreakpointKind::Normal);
    }
    app.set_breakpoint_group(&price_rs, 1, Some("Pricing"));
    app.set_breakpoint_group(&main_rs, 3, Some("  Calls "));
    app.set_breakpoint_group(&main_rs, 4, Some("Calls"));
    app.set_breakpoint_group(&main_rs, 4, Some("   "));
    app.set_breakpoint_group(&main_rs, 4, Some("Calls"));
    assert_eq!(app.breakpoint_groups(), ["Calls", "Pricing"]);
    let listed = |a: &CrabCraftApp| a.breakpoint_rows().into_iter().map(|r| (r.path.file_name().unwrap().to_string_lossy().into_owned(), r.line, r.enabled, r.group)).collect::<Vec<_>>();
    // The whole Calls group off.
    app.set_group_enabled(Some("Calls"), false);
    let expected = vec![("main.rs".to_string(), 3, false, Some("Calls".to_string())), ("main.rs".into(), 4, false, Some("Calls".into())), ("price.rs".into(), 1, true, Some("Pricing".into()))];
    assert_eq!(listed(&app), expected);
    assert_eq!(app.docs.active_doc().unwrap().editor.breakpoints.get(&3), Some(&BreakpointKind::Disabled));

    // Debugging stops in the closed file, not at the disabled calls.
    app.run_ide_action(&egui::Context::default(), Action::Debug);
    pump_debugger(&mut app, "the breakpoint in price.rs");
    let at = app.debugger.session.as_ref().and_then(|s| s.execution_point());
    assert_eq!(at, Some((price_rs.clone(), 2)));
    // Turned off from the list while stopped, it no longer stops the second call.
    app.set_breakpoint_enabled(&price_rs, 1, false);
    app.run_ide_action(&egui::Context::default(), Action::Resume);
    assert!(app.debugger.session.as_ref().and_then(|s| s.breakpoints().get(&price_rs, 2)).is_some_and(|bp| !bp.spec.enabled), "the disabled breakpoint must be synchronized before resume is sent");
    let start = Instant::now();
    while !app.debugger.session.as_ref().is_none_or(|s| s.state().is_ended()) {
        let outs = app.debugger.update(&mut app.docs);
        app.handle_debug(outs);
        assert!(!app.debugger.stopped(), "stopped again at {:?}", app.debugger.session.as_ref().and_then(|s| s.execution_point()));
        assert!(start.elapsed() < Duration::from_secs(60), "the program did not end");
        std::thread::sleep(Duration::from_millis(20));
    }
    app.set_breakpoint_enabled(&price_rs, 1, true);

    // Removing one from the list, in a closed file too; then the session keeps the rest.
    app.docs.closed_breakpoints.entry(price_rs.clone()).or_default().insert(2, BreakpointKind::Normal);
    app.remove_breakpoint(&price_rs, 2);
    let before = listed(&app);
    assert_eq!(before.len(), 3);
    app.save_session();
    let mut again = new_app(dir.path());
    again.open_workspace(&root);
    assert_eq!(listed(&again), before);
}

/// D-13: Set Execution Point to Cursor moves the program forward and back within the function
/// without running the lines in between, and refuses lines in other functions and lines without
/// code.
#[test]
fn set_execution_point_skips_and_repeats_lines_with_gdb() {
    use rc_editor::{Action, BreakpointKind};
    if rc_dap::gdb::locate().is_none() {
        eprintln!("skipped: GDB is not installed");
        return;
    }
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
    write(&root.join("src/main.rs"), "mod other;\n\nfn main() {\n    let mut n = 1;\n    n += 10;\n    n += 100;\n    n += 1000;\n    other::show(n);\n}\n");
    write(&root.join("src/other.rs"), "pub fn show(n: u32) {\n    println!(\"{n}\");\n}\n");
    let root = root.canonicalize().unwrap();
    let main_rs = root.join("src/main.rs");
    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&main_rs);
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(5, BreakpointKind::Normal);
    app.run_ide_action(&egui::Context::default(), Action::Debug);
    pump_debugger(&mut app, "the breakpoint");
    assert!(app.debugger.session.as_ref().unwrap().can_set_execution_point());
    assert_eq!(local(&mut app, "n"), "11");
    let at = |a: &CrabCraftApp| a.debugger.session.as_ref().and_then(|s| s.execution_point());
    let jump_to = |app: &mut CrabCraftApp, line: usize| {
        app.docs.active_doc_mut().unwrap().editor.goto(line, 0);
        app.run_ide_action(&egui::Context::default(), Action::SetExecutionPoint);
    };

    // Forward: n += 100 and n += 1000 do not run.
    jump_to(&mut app, 7);
    settle_debugger(&mut app, "the jump forward", &|a| a.debugger.stopped() && at(a) == Some((main_rs.clone(), 8)));
    assert_eq!(local(&mut app, "n"), "11");

    // Refused: a line in another function, and a line without code. The program stays.
    let notices = app.notices.len();
    app.open_file(&root.join("src/other.rs"));
    jump_to(&mut app, 1);
    settle_debugger(&mut app, "the refusal", &|a| a.notices.len() > notices);
    assert!(app.notices.last().unwrap().text.contains("current function"), "{}", app.notices.last().unwrap().text);
    app.open_file(&main_rs);
    jump_to(&mut app, 1);
    settle_debugger(&mut app, "the second refusal", &|a| a.notices.len() > notices + 1);
    assert!(app.notices.last().unwrap().text.contains("No statement"), "{}", app.notices.last().unwrap().text);
    assert_eq!(at(&app), Some((main_rs.clone(), 8)));

    // Back: n += 100 runs again from 11.
    jump_to(&mut app, 5);
    settle_debugger(&mut app, "the jump back", &|a| a.debugger.stopped() && at(a) == Some((main_rs.clone(), 6)));
    // Overflow checks split `n += 100` in two ranges: GDB stops on it twice, as without a jump.
    for _ in 0..3 {
        if at(&app) == Some((main_rs.clone(), 7)) {
            break;
        }
        let generation = app.debugger.session.as_ref().unwrap().generation();
        app.run_ide_action(&egui::Context::default(), Action::StepOver);
        settle_debugger(&mut app, "the step", &|a| a.debugger.stopped() && a.debugger.session.as_ref().is_some_and(|s| s.generation() > generation + 1) && at(a).is_some());
    }
    assert_eq!(at(&app), Some((main_rs.clone(), 7)));
    assert_eq!(local(&mut app, "n"), "111");
    app.run_ide_action(&egui::Context::default(), Action::Resume);
    settle_debugger(&mut app, "the end", &|a| a.debugger.session.as_ref().is_none_or(|s| s.state().is_ended()));
}

/// D-15: watching for a name pattern attaches GDB to the next matching program that starts,
/// not to the one already running, and not while a debug session runs.
#[test]
fn the_debugger_attaches_automatically_to_a_program_that_starts() {
    use rc_editor::BreakpointKind;
    // Yama allows a debugger that is not the parent only when the program says so.
    const MAIN: &str = "extern \"C\" {\n    fn prctl(option: i32, arg: u64, ...) -> i32;\n}\n\nfn main() {\n    unsafe { prctl(0x59616d61, u64::MAX) };\n    let mut rounds = 0u64;\n    loop {\n        rounds += 1;\n        std::thread::sleep(std::time::Duration::from_millis(5));\n        if rounds == u64::MAX {\n            break;\n        }\n    }\n}\n";
    if rc_dap::gdb::locate().is_none() {
        eprintln!("skipped: GDB is not installed");
        return;
    }
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root.join("Cargo.toml"), "[package]\nname = \"autoserver\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
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
    let spawn = || Running(std::process::Command::new(root.join("target/debug/autoserver")).spawn().unwrap());
    let frames = |app: &mut CrabCraftApp, n: usize| {
        for _ in 0..n {
            let outs = app.debugger.update(&mut app.docs);
            app.handle_debug(outs);
            app.poll_auto_attach();
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    let mut app = new_app(dir.path());
    app.open_workspace(&root);
    app.trust_project_tools_for_test();
    pump(&mut app, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/main.rs"));
    app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(8, BreakpointKind::Normal);
    let before = spawn();
    std::thread::sleep(Duration::from_millis(300));
    app.auto_attach_text = " other, auto* ,".into();
    assert!(app.start_auto_attach());
    assert_eq!(app.auto_attach_text, "other, auto*");
    frames(&mut app, 60);
    assert!(app.debugger.session.is_none(), "attached to the program that was already running");

    let after = spawn();
    let start = Instant::now();
    while !app.debugger.stopped() {
        frames(&mut app, 1);
        if let Some(rc_dap::SessionState::Failed(f)) = app.debugger.session.as_ref().map(|s| s.state()) {
            panic!("attach failed: {f}");
        }
        assert!(start.elapsed() < Duration::from_secs(60), "did not attach: {}", app.debugger.state_label());
    }
    assert_eq!(app.debugger.session.as_ref().and_then(|s| s.debuggee_pid()), Some(after.0.id()));
    assert_eq!(app.docs.active_doc().and_then(|d| d.editor.execution_line), Some(8));
    assert!(app.notices.iter().any(|n| n.text.contains("autoserver")));

    // While debugging, another one is let go.
    let third = spawn();
    frames(&mut app, 60);
    assert_eq!(app.debugger.session.as_ref().and_then(|s| s.debuggee_pid()), Some(after.0.id()));
    app.debugger.stop();
    app.stop_auto_attach();
    app.save_session();
    let mut again = new_app(dir.path());
    again.open_workspace(&root);
    assert_eq!(again.auto_attach_text, "other, auto*");
    assert!(again.auto_attach.is_none(), "watching starts only when asked");
    drop((before, after, third));
}

/// I-33: the highlighted Alt+Enter action is previewed before it is applied. rust-analyzer's
/// import fix comes without its edit, so it is resolved in the background; moving to another
/// row drops the pending answer; the resolved row is applied without asking again.
#[test]
fn the_highlighted_intention_is_previewed_before_it_is_applied() {
    use crate::intention_preview::State;
    use rc_lsp::lsp_types::CodeActionOrCommand as A;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    const SOURCE: &str = "fn main() {\n    let mut m = HashMap::new();\n    m.insert(1, 2);\n    println!(\"{m:?}\");\n}\n";
    write(&root.join("src/main.rs"), SOURCE);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    let main = root.join("src/main.rs");
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready && a.runner.cargo_workspace.is_some());
    pump(&mut app, "the unresolved name", |a| a.docs.active_doc().is_some_and(|d| !d.editor.diagnostics.is_empty()));
    let i = app.docs.index_of(&main).unwrap();
    app.docs.list[i].editor.selection = rc_text::Selection::point(SOURCE.find("HashMap").unwrap() + 1);
    app.run_insight_action(rc_editor::Action::ShowIntentions, egui::Pos2::ZERO);
    pump(&mut app, "the actions", |a| matches!(a.dialog, Some(crate::ui::dialogs::Dialog::CodeActions { .. })));
    let Some(crate::ui::dialogs::Dialog::CodeActions { mut list, .. }) = app.dialog.take() else { unreachable!() };
    let title = |a: &A| match a {
        A::Command(c) => c.title.clone(),
        A::CodeAction(c) => c.title.clone(),
    };
    let import = list.iter().position(|a| title(a).contains("Import `std::collections::HashMap`")).unwrap_or_else(|| panic!("{:?}", list.iter().map(title).collect::<Vec<_>>()));
    assert!(matches!(&list[import], A::CodeAction(a) if a.edit.is_none()), "rust-analyzer resolves the edit lazily");
    let other = (import + 1) % list.len();
    assert_ne!(other, import, "{:?}", list.iter().map(title).collect::<Vec<_>>());

    // Highlighted, then left before the answer: its resolve is dropped.
    app.update_intention_preview(&mut list, import);
    assert_eq!(app.intention_preview.state, State::Resolving);
    let settle = |app: &mut CrabCraftApp, list: &mut Vec<A>, row: usize| {
        let start = Instant::now();
        loop {
            app.update_intention_preview(list, row);
            if !app.intention_preview.busy() || start.elapsed() > Duration::from_secs(60) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    settle(&mut app, &mut list, other);
    // The import's late answer went nowhere: its row is still unresolved, and the preview is the
    // other row's.
    assert!(matches!(&list[import], A::CodeAction(a) if a.edit.is_none()));
    assert!(!matches!(&app.intention_preview.state, State::Ready { files, .. } if files.iter().any(|f| f.lines.contains(&('+', "use std::collections::HashMap;".to_string())))));
    // Back on the import: resolved in the background, then diffed.
    settle(&mut app, &mut list, import);
    let State::Ready { files, .. } = app.intention_preview.state.clone() else { panic!("{:?}", app.intention_preview.state) };
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, main);
    assert!(files[0].lines.contains(&('+', "use std::collections::HashMap;".to_string())), "{:?}", files[0].lines);
    assert_eq!(files[0].removed, 0);
    // Nothing was applied, and the row now carries its edit.
    assert_eq!(app.docs.list[i].editor.buffer.rope().to_string(), SOURCE);
    assert!(matches!(&list[import], A::CodeAction(a) if a.edit.is_some()));

    // Applying it needs no second request: the text changes at once.
    let mut out = Vec::new();
    app.insight.run_code_action(&mut app.docs, list.swap_remove(import), &mut out);
    assert!(app.docs.list[i].editor.buffer.rope().to_string().starts_with("use std::collections::HashMap;\n"));
    app.intention_preview.clear();
    app.insight.stop();
}

/// I-33: the IDE's own quick fixes (here the update of a Cargo.toml requirement) are previewed
/// from the edit they make, without a language server.
#[test]
fn an_ide_quick_fix_is_previewed_without_a_server() {
    use crate::intention_preview::State;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    const MANIFEST: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1.0.100\"\n";
    write(&root.join("Cargo.toml"), MANIFEST);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.open_file(&root.join("Cargo.toml"));
    let command = rc_lsp::lsp_types::Command { title: "Update to 1.0.200".into(), command: crate::insight::UPDATE_DEPENDENCY_COMMAND.into(), arguments: Some(vec![serde_json::json!(6), serde_json::json!("1.0.200")]) };
    let mut list = vec![rc_lsp::lsp_types::CodeActionOrCommand::CodeAction(rc_lsp::lsp_types::CodeAction { title: command.title.clone(), command: Some(command), ..Default::default() })];
    let start = Instant::now();
    loop {
        app.update_intention_preview(&mut list, 0);
        if !app.intention_preview.busy() || start.elapsed() > Duration::from_secs(10) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let State::Ready { files, .. } = app.intention_preview.state.clone() else { panic!("{:?}", app.intention_preview.state) };
    assert_eq!(files[0].lines, vec![('@', "7".to_string()), ('-', "serde = \"1.0.100\"".to_string()), ('+', "serde = \"1.0.200\"".to_string())]);
    assert_eq!(app.docs.active_doc().unwrap().editor.buffer.rope().to_string(), MANIFEST);
}

/// I-18: an inspection profile changes what rustc's findings (through rust-analyzer's `cargo
/// check`) and CrabCraft's own `Cargo.toml` inspections become in the editor (underline and
/// scroll-bar marks follow `editor.diagnostics`) and in the Problems window; the project can use
/// a profile of its own, saved with it.
#[test]
fn inspection_profiles_change_the_level_of_findings() {
    use rc_core::inspections::{InspectionProfile, Level};
    use rc_editor::Severity;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    write(&root.join("src/main.rs"), "fn main() {\n    let unused = 1;\n}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    let main = root.join("src/main.rs");
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready && a.runner.cargo_workspace.is_some());
    app.insight.did_save(&main);
    let level = |a: &CrabCraftApp| a.docs.list.iter().find(|d| d.path == main).and_then(|d| d.editor.diagnostics.iter().find(|x| x.code.as_deref() == Some("unused_variables") && x.message.starts_with("unused variable")).map(|x| x.severity));
    let listed = |a: &CrabCraftApp| a.insight.problems.get(&main).is_some_and(|l| l.iter().any(|x| x.code.as_deref() == Some("unused_variables")));
    pump(&mut app, "cargo check's warning", |a| level(a) == Some(Severity::Warning) && listed(a));

    // Default profile: an error, then off, then a weak warning (underlined, not listed).
    app.settings.inspection_profile.set("unused_variables", Some(Level::Error));
    app.refresh_inspection_profile();
    assert_eq!(level(&app), Some(Severity::Error));
    assert!(listed(&app));
    app.settings.inspection_profile.set("unused_variables", Some(Level::Off));
    app.refresh_inspection_profile();
    assert_eq!(level(&app), None);
    assert!(!listed(&app));
    app.settings.inspection_profile.set("unused_variables", Some(Level::WeakWarning));
    app.refresh_inspection_profile();
    assert_eq!(level(&app), Some(Severity::Info));
    assert!(!listed(&app));
    // A new report from the server follows the profile too.
    app.insight.did_save(&main);
    std::thread::sleep(Duration::from_millis(300));
    pump(&mut app, "the next report", |a| level(a) == Some(Severity::Info));

    // The project's own profile wins once chosen, and is saved with the project.
    let mut project = InspectionProfile::default();
    project.set("unused_variables", Some(Level::Error));
    project.levels.insert("bad id".into(), "error".into());
    app.set_project_inspections(true, project);
    assert_eq!(level(&app), Some(Severity::Error));
    assert!(!app.inspections.project.levels.contains_key("bad id"), "cleaned");
    app.set_project_inspections(false, app.inspections.project.clone());
    assert_eq!(level(&app), Some(Severity::Info));
    app.set_project_inspections(true, app.inspections.project.clone());
    app.save_session();
    let saved = rc_core::Session::load(&root);
    assert!(saved.use_project_profile);
    assert_eq!(saved.inspection_profile.level("unused_variables"), Some(Level::Error));

    // CrabCraft's own inspection of Cargo.toml: an unknown key, turned off.
    let manifest = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest).unwrap().replace("edition = \"2021\"", "edition = \"2021\"\nautthors = []");
    std::fs::write(&manifest, text).unwrap();
    app.open_file(&manifest);
    let unknown = |a: &CrabCraftApp| a.docs.active_doc().unwrap().editor.diagnostics.iter().find(|d| d.message.contains("autthors")).map(|d| d.severity);
    pump(&mut app, "the manifest inspection", |a| {
        a.manifest_inspections.update(&mut a.docs, &mut a.insight.problems, a.cfg_epoch, &a.insight.profile.clone());
        unknown(a) == Some(Severity::Warning)
    });
    let mut project = app.inspections.project.clone();
    project.set(crate::inspection_profile::UNKNOWN_KEY, Some(Level::Off));
    app.set_project_inspections(true, project);
    app.manifest_inspections.update(&mut app.docs, &mut app.insight.problems, app.cfg_epoch, &app.insight.profile.clone());
    assert_eq!(unknown(&app), None);
    assert!(app.insight.problems.get(&manifest).is_none_or(|l| !l.iter().any(|d| d.message.contains("autthors"))));
    app.insight.stop();
}

/// rust-analyzer's fix for a module file named against the convention renames the file and
/// its `mod` line: the file operation of the edit is applied, and the open editor follows.
#[test]
fn a_fix_that_renames_a_module_file_renames_it() {
    use rc_lsp::lsp_types::CodeActionOrCommand as A;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    const MAIN: &str = "mod MyMod;\n\nfn main() {\n    MyMod::hello();\n}\n";
    write(&root.join("src/main.rs"), MAIN);
    write(&root.join("src/MyMod.rs"), "pub fn hello() {}\n");
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    let main = root.join("src/main.rs");
    let module = root.join("src/MyMod.rs");
    app.open_file(&module);
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready && a.runner.cargo_workspace.is_some());
    pump(&mut app, "the case warning", |a| a.docs.active_doc().is_some_and(|d| d.editor.diagnostics.iter().any(|x| x.message.contains("snake_case"))));
    let i = app.docs.index_of(&main).unwrap();
    app.docs.list[i].editor.selection = rc_text::Selection::point(MAIN.find("MyMod").unwrap() + 1);
    app.run_insight_action(rc_editor::Action::ShowIntentions, egui::Pos2::ZERO);
    pump(&mut app, "the actions", |a| matches!(a.dialog, Some(crate::ui::dialogs::Dialog::CodeActions { .. })));
    let Some(crate::ui::dialogs::Dialog::CodeActions { mut list, .. }) = app.dialog.take() else { unreachable!() };
    let title = |a: &A| match a {
        A::Command(c) => c.title.clone(),
        A::CodeAction(c) => c.title.clone(),
    };
    let fix = list.iter().position(|a| title(a).contains("Rename to") && title(a).contains("my_mod")).unwrap_or_else(|| panic!("{:?}", list.iter().map(title).collect::<Vec<_>>()));
    let mut out = Vec::new();
    app.insight.run_code_action(&mut app.docs, list.swap_remove(fix), &mut out);
    app.handle_insight(out);
    let renamed = root.join("src/my_mod.rs");
    pump(&mut app, "the renamed file", |a| renamed.is_file() && !module.exists());
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "pub fn hello() {}\n");
    // The editor of the module follows it, and the `mod` line names the new file.
    assert!(app.docs.index_of(&renamed).is_some());
    assert!(app.docs.index_of(&module).is_none());
    let text = app.docs.list[app.docs.index_of(&main).unwrap()].editor.buffer.rope().to_string();
    assert!(text.starts_with("mod my_mod;\n"), "{text}");
    app.insight.stop();
}

/// rust-analyzer's "Rename to `snake_case`" fix of a function renames it and its calls.
#[test]
fn the_snake_case_fix_renames_a_function() {
    use rc_lsp::lsp_types::CodeActionOrCommand as A;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    demo(&root);
    const MAIN: &str = "fn main() {\n    testeDeEscrita();\n}\n\nfn testeDeEscrita() {}\n";
    write(&root.join("src/main.rs"), MAIN);
    let root = root.canonicalize().unwrap();
    let mut app = new_app(dir.path());
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace(&root);
    let main = root.join("src/main.rs");
    app.open_file(&main);
    pump(&mut app, "the analysis", |a| a.insight.status == InsightStatus::Ready && a.runner.cargo_workspace.is_some());
    pump(&mut app, "the case warning", |a| a.docs.active_doc().is_some_and(|d| d.editor.diagnostics.iter().any(|x| x.message.contains("snake_case"))));
    let i = app.docs.index_of(&main).unwrap();
    app.docs.list[i].editor.selection = rc_text::Selection::point(MAIN.rfind("testeDeEscrita").unwrap() + 2);
    app.run_insight_action(rc_editor::Action::ShowIntentions, egui::Pos2::ZERO);
    pump(&mut app, "the actions", |a| matches!(a.dialog, Some(crate::ui::dialogs::Dialog::CodeActions { .. })));
    let Some(crate::ui::dialogs::Dialog::CodeActions { mut list, .. }) = app.dialog.take() else { unreachable!() };
    let title = |a: &A| match a {
        A::Command(c) => c.title.clone(),
        A::CodeAction(c) => c.title.clone(),
    };
    let fix = list.iter().position(|a| title(a) == "Rename to teste_de_escrita").unwrap_or_else(|| panic!("{:?}", list.iter().map(title).collect::<Vec<_>>()));
    eprintln!("fix: {:?}", list[fix]);
    let mut out = Vec::new();
    app.insight.run_code_action(&mut app.docs, list.swap_remove(fix), &mut out);
    app.handle_insight(out);
    pump(&mut app, "the renamed function", |a| a.docs.list[i].editor.buffer.rope().to_string().contains("fn teste_de_escrita()"));
    assert_eq!(app.docs.list[i].editor.buffer.rope().to_string(), "fn main() {\n    teste_de_escrita();\n}\n\nfn teste_de_escrita() {}\n");
    app.insight.stop();
}

#[test]
fn native_trait_hierarchy_resolves_aliases_homonyms_transitive_and_unsaved_relations() {
    use egui_kittest::kittest::Queryable;
    const SOURCE: &str = "pub mod left {\n    pub trait Base {}\n    pub trait Child: Base {}\n}\npub mod right {\n    pub trait Base {}\n    pub trait Wrong: Base {}\n}\nuse left::Base as Alias;\npub trait Leaf: Alias {}\npub trait Where where Self: Leaf {}\nstruct Value;\nimpl left::Base for Value {}\nfn main() {}\n";
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap(); let project = tempfile::tempdir().unwrap(); let root = project.path().canonicalize().unwrap();
    demo(&root); let file = root.join("src/main.rs"); write(&file,SOURCE);
    let mut app = new_app(home.path()); app.settings.trusted_projects.push(root.clone()); app.open_workspace(&root); app.open_file(&file);
    pump(&mut app,"trait fixture analysis",|a| a.insight.status==InsightStatus::Ready);
    pump(&mut app,"trait fixture indexing",|a| a.docs.active_doc().is_some_and(|doc| a.insight.runnables.contains_key(&doc.editor.id)));
    let ask = |app: &mut CrabCraftApp, position:usize| {
        let doc=app.docs.active_doc().unwrap(); app.insight.type_hierarchy(doc,position);
        pump(app,"semantic trait hierarchy",|a| !a.insight.type_hierarchy_pending() && a.insight.call_tree.is_some());
        app.insight.call_tree.as_ref().unwrap().rows().into_iter().map(|(_,node)|node.name.clone()).collect::<Vec<_>>()
    };
    let base = SOURCE.find("trait Base").unwrap()+"trait B".len();
    let names=ask(&mut app,base);
    assert!(names.contains(&"Child".into()),"{names:?}"); assert!(names.contains(&"Leaf".into()),"{names:?}"); assert!(names.contains(&"Where".into()),"{names:?}");
    assert!(!names.contains(&"Wrong".into()),"homonymous trait leaked: {names:?}");
    assert!(names.iter().any(|s|s.starts_with("impl left::Base for Value")),"{names:?}");
    let right = SOURCE.find("pub mod right").unwrap()+SOURCE[SOURCE.find("pub mod right").unwrap()..].find("trait Base").unwrap()+"trait B".len();
    let names=ask(&mut app,right); assert!(names.contains(&"Wrong".into())); assert!(!names.contains(&"Leaf".into()));
    let leaf=SOURCE.find("trait Leaf").unwrap()+"trait L".len();
    let names=ask(&mut app,leaf); assert!(names.contains(&"Base".into()),"alias did not resolve: {names:?}"); assert!(names.contains(&"Where".into()));
    let doc=app.docs.active_doc_mut().unwrap(); let end=doc.editor.buffer.len_chars(); doc.editor.replace(end,end,"pub trait Unsaved: Alias {}\n");
    let outputs=app.insight.update(&mut app.docs,true); app.handle_insight(outputs);
    let names=ask(&mut app,base); assert!(names.contains(&"Unsaved".into()),"{names:?}"); assert_eq!(std::fs::read_to_string(&file).unwrap(),SOURCE);
    app.dock.show(crate::toolwindow::ToolWindow::Hierarchy);
    let mut h=crate::screens::harness(app,egui::vec2(1280.,900.));
    h.get_by_label("Unsaved").click(); h.run_steps(2);
    let doc=h.state().docs.active_doc().unwrap(); assert_eq!(doc.editor.caret_line_col().0,SOURCE.lines().count());
    // A source edit invalidates the complete graph snapshot, including other open buffers.
    {
        let app=h.state_mut(); let doc=app.docs.active_doc().unwrap(); app.insight.type_hierarchy(doc,base);
        let outputs=app.insight.update(&mut app.docs,true); app.handle_insight(outputs);
        app.docs.active_doc_mut().unwrap().editor.replace(0,0,"// changed\n");
        let outputs=app.insight.update(&mut app.docs,true); app.handle_insight(outputs);
        assert!(!app.insight.type_hierarchy_pending()); assert!(app.insight.call_tree.is_none());
    }
    // Cancel is available immediately, even before the background scan is scheduled.
    {
        let app=h.state_mut(); let doc=app.docs.active_doc().unwrap(); app.insight.type_hierarchy(doc,base+"// changed\n".len());
        assert!(app.insight.type_hierarchy_pending()); app.insight.cancel_type_hierarchy(); assert!(!app.insight.type_hierarchy_pending());
        app.insight.stop();
    }
}

#[cfg(unix)]
#[test]
#[ignore="requires freshly built desktop binary in CRABCRAFT_MACRO_WORKER"]
fn native_one_step_macro_expansion_uses_unsaved_definition_and_keeps_nested_calls() {
    use rc_editor::Action;
    const SOURCE:&str="/* café 🦀 */ macro_rules! inner { ($a:expr,$b:expr)=>{($a)+($b)}; }\n/* café 🦀 */ macro_rules! outer { ($e:expr)=>{inner!($e,$e)}; }\nfn main(){assert_eq!(outer!(21),42);}\n";
    let _lock=crate::screens::HOME_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    assert!(std::env::var_os("CRABCRAFT_MACRO_WORKER").is_some());
    let home=tempfile::tempdir().unwrap();let project=tempfile::tempdir().unwrap();let root=project.path().canonicalize().unwrap();demo(&root);
    let file=root.join("src/main.rs");write(&file,SOURCE);
    let mut app=new_app(home.path());app.open_workspace(&root);app.open_file(&file);
    app.run_insight_action(Action::ExpandMacroStep,egui::Pos2::ZERO);
    assert!(!app.insight.macro_step_pending());assert!(app.insight.session.is_none());
    app.trust_project();pump(&mut app,"one-step macro analysis",|a|a.insight.status==InsightStatus::Ready);
    pump(&mut app,"one-step macro indexing",|a|a.docs.active_doc().is_some_and(|doc|a.insight.runnables.contains_key(&doc.editor.id)));
    let unsaved=SOURCE.replace("inner!($e,$e)","inner!($e,$e) + 1");
    let doc=app.docs.active_doc_mut().unwrap();doc.editor.replace(0,doc.editor.buffer.len_chars(),&unsaved);
    let call=unsaved.find("outer!(21)").unwrap()+2;let line=unsaved[..call].chars().filter(|c|*c=='\n').count();let start=unsaved[..call].rfind('\n').map_or(0,|byte|byte+1);doc.editor.goto(line,unsaved[start..call].chars().count());
    app.run_insight_action(Action::ExpandMacroStep,egui::Pos2::ZERO);
    pump(&mut app,"one-step expansion tab",|a|a.docs.active_doc().is_some_and(|doc|doc.generated));
    let generated=app.docs.active_doc().unwrap();assert!(generated.editor.read_only);let expansion=generated.editor.buffer.rope().to_string();
    assert!(expansion.contains("inner"),"nested macro was expanded: {expansion}");assert!(expansion.contains('+')&&expansion.contains('1'),"unsaved definition ignored: {expansion}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(),SOURCE);
    assert!(app.insight.session.as_ref().unwrap().docs.paths().iter().all(|p|p!=&generated.path));
    let proof=root.join("one_step_proof.rs");write(&proof,&unsaved.replace("outer!(21)",&format!("({expansion})")).replace(",42)",",43)"));
    let binary=root.join("one_step_proof");let compiler=std::process::Command::new("rustc").args(["--edition=2024"]).arg(&proof).arg("-o").arg(&binary).output().unwrap();assert!(compiler.status.success(),"{}",String::from_utf8_lossy(&compiler.stderr));assert!(std::process::Command::new(binary).status().unwrap().success());
    let original=app.docs.index_of(&file).unwrap();app.docs.activate(original);
    app.run_insight_action(Action::ExpandMacroStep,egui::Pos2::ZERO);
    let out=app.insight.update(&mut app.docs,true);app.handle_insight(out);app.docs.active_doc_mut().unwrap().editor.replace(0,0,"// changed\n");
    let out=app.insight.update(&mut app.docs,true);app.handle_insight(out);assert!(!app.insight.macro_step_pending());assert!(!app.docs.active_doc().unwrap().generated);
    app.insight.stop();
}
