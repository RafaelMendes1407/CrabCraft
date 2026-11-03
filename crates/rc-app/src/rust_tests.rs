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

#[test]
fn tests_run_with_nextest_when_the_setting_is_on() {
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
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
    app.open_file(&root.join("src/lib.rs"));
    app.open_file(&root.join("src/main.rs"));

    // Untrusted: the server stays off (cargo clean runs no project code).
    app.invalidate_caches(true);
    assert_eq!(app.insight.status, InsightStatus::Untrusted);
    assert!(app.insight.session.is_none());

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
    pump(&mut app, "the workspace", |a| a.runner.cargo_workspace.is_some());
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
