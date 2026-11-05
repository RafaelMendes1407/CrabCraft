//! Evidence for inherited editor behavior in real Rust projects; no input to real windows.
use egui_kittest::Harness;
use rc_editor::{EditorConfig, EditorState, EditorView, Keymap, KeymapKind};
use rc_text::{Range, Selection};
use std::path::Path;

fn project(source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"editor-audit\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
    let path = dir.path().join("src/main.rs");
    std::fs::write(&path, source).unwrap();
    (dir, path)
}

fn compile(path: &Path) {
    let program = path.parent().unwrap().join("audit-program");
    let output = std::process::Command::new("rustc").arg("--edition=2024").arg(path).arg("-o").arg(program).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

fn editor(path: &Path, source: &str) -> Harness<'static, EditorState> {
    let config = EditorConfig::default();
    let keymap = Keymap::new(KeymapKind::VsCode);
    let state = EditorState::new(1, Some(path.into()), source);
    let mut first = true;
    let mut harness = Harness::builder().with_size(egui::vec2(1100.0, 650.0)).build_ui_state(move |ui, state: &mut EditorState| {
        if std::mem::take(&mut first) { rc_kit::theme::install_fonts(ui.ctx()); return; }
        EditorView::new(state, &config, &keymap).focus(true).show(ui);
    }, state);
    harness.run_steps(3);
    harness
}

fn typed(harness: &mut Harness<'static, EditorState>, text: &str) {
    for ch in text.chars() { harness.event(egui::Event::Text(ch.to_string())); harness.run_steps(1); }
}

fn caret(harness: &mut Harness<'static, EditorState>, needle: &str, after: usize) {
    let text = harness.state().buffer.rope().to_string();
    let byte = text.find(needle).unwrap();
    let pos = text[..byte].chars().count() + after;
    harness.state_mut().selection = Selection::point(pos);
}

#[test]
fn rust_lifetimes_labels_characters_and_closure_pipes_survive_native_typing() {
    let source = "fn borrow<>(value: ) ->  { value }\nfn main() {\n    let increment = ;\n    let letter = ;\n    let empty = ;\n    let label_value = loop { break 7; };\n    assert_eq!(increment(1), 2);\n    assert_eq!(borrow(\"crab\"), \"crab\");\n    assert_eq!(letter, 'é');\n    assert_eq!(empty(), 3);\n    assert_eq!(label_value, 7);\n}\n";
    let (_dir, path) = project(source);
    let mut h = editor(&path, source);
    caret(&mut h, "borrow<", "borrow<".len());
    typed(&mut h, "'a");
    assert!(h.state().buffer.rope().to_string().contains("borrow<'a>"));
    let text = h.state().buffer.rope().to_string();
    let positions = [text.find("value: ").unwrap() + "value: ".len(), text.find("-> ").unwrap() + "-> ".len()].map(|byte| Range::point(text[..byte].chars().count()));
    h.state_mut().selection = Selection::new(positions, 0);
    typed(&mut h, "&'a str");
    caret(&mut h, "increment = ", "increment = ".len());
    typed(&mut h, "|value: i32| value + 1");
    caret(&mut h, "letter = ", "letter = ".len());
    typed(&mut h, "'é'");
    caret(&mut h, "empty = ", "empty = ".len());
    typed(&mut h, "|| 3");
    caret(&mut h, "loop {", 0);
    typed(&mut h, "'again: ");
    caret(&mut h, "break ", "break ".len());
    typed(&mut h, "'again ");
    let text = h.state().buffer.rope().to_string();
    assert!(text.contains("&'a str") && !text.contains("'a'"), "{text}");
    assert!(text.contains("|value: i32| value + 1") && text.contains("|| 3"));
    std::fs::write(&path, text).unwrap();
    compile(&path);
}

#[test]
fn native_multicursor_occurrences_type_undo_redo_and_collapse_on_rust() {
    let source = "fn main() {\n    // café before the carets\n    let value = 2;\n    assert_eq!(value + value, 4);\n}\n";
    let (_dir, path) = project(source);
    let mut h = editor(&path, source);
    caret(&mut h, "let value", "let ".len());
    for _ in 0..3 { h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::D); h.run_steps(1); }
    assert_eq!(h.state().selection.len(), 3);
    typed(&mut h, "renamed");
    let changed = h.state().buffer.rope().to_string();
    assert_eq!(changed.matches("renamed").count(), 3, "{changed}");
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Z);
    h.run_steps(1);
    assert_eq!(h.state().buffer.rope().to_string(), source);
    h.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Z);
    h.run_steps(1);
    assert_eq!(h.state().buffer.rope().to_string(), changed);
    h.key_press(egui::Key::Escape);
    h.run_steps(1);
    assert_eq!(h.state().selection.len(), 1);
    caret(&mut h, "let renamed", "let renamed".len());
    h.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::ALT, egui::Key::ArrowDown);
    h.run_steps(1);
    assert_eq!(h.state().selection.len(), 2);
    assert!(h.state().selection.ranges().iter().all(|range| range.is_empty()));
    let lines: Vec<_> = h.state().selection.ranges().iter().map(|range| h.state().buffer.rope().char_to_line(range.head)).collect();
    assert_eq!(lines, vec![2, 3]);
    h.key_press(egui::Key::Escape);
    h.run_steps(1);
    caret(&mut h, "let renamed", "let ".len());
    h.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::L);
    h.run_steps(1);
    assert_eq!(h.state().selection.len(), 3);
    std::fs::write(&path, changed).unwrap();
    compile(&path);
}

#[test]
fn rust_save_and_disk_reload_preserve_new_bom_and_line_endings() {
    let source = "fn main() { println!(\"one\"); }\n";
    let (_dir, path) = project(source);
    let mut docs = crate::docs::Docs::new(rc_syntax::ParserPool::new(std::sync::Arc::new(|| {})));
    let index = docs.open(&path).unwrap();
    docs.list[index].editor.replace(0, 0, "// local edit\n");
    docs.save(index).unwrap();
    assert!(!docs.list[index].is_modified());
    // Same normalized text, but externally changed encoding: a following save must retain it.
    let disk = format!("\u{feff}{}", docs.list[index].editor.buffer.rope().to_string().replace('\n', "\r\n"));
    std::fs::write(&path, disk.as_bytes()).unwrap();
    docs.list[index].mtime = None;
    docs.on_disk_change(&path);
    docs.save(index).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), disk.as_bytes());
    // A content conflict is kept until the user chooses Load from Disk; reloaded is its path.
    docs.list[index].editor.replace(0, 0, "// unsaved\n");
    let external = "\u{feff}fn main() { println!(\"external\"); }\r\n";
    std::fs::write(&path, external).unwrap();
    docs.list[index].mtime = None;
    docs.on_disk_change(&path);
    assert!(docs.list[index].conflict);
    assert!(docs.list[index].editor.buffer.rope().to_string().contains("unsaved"));
    let (text, bom, mtime) = crate::docs::read_text(&path).unwrap();
    docs.reloaded(index, &text, bom, mtime);
    assert!(!docs.list[index].is_modified() && !docs.list[index].conflict);
    docs.save(index).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), external.as_bytes());
    compile(&path);
}

#[test]
fn native_rust_brackets_quotes_type_over_and_match_the_parser() {
    let source = "fn main() {\n    let text = ;\n    let values = ;\n    let total = ;\n    let letter = 'é';\n    assert_eq!(text, \"crab\");\n    assert_eq!(values, [1, 2]);\n    assert_eq!(total, 3);\n    assert_eq!(letter, 'é');\n}\n";
    let (_dir, path) = project(source);
    let mut h = editor(&path, source);
    caret(&mut h, "text = ", "text = ".len());
    typed(&mut h, "(");
    assert!(h.state().buffer.rope().to_string().contains("text = ();"));
    typed(&mut h, "\"");
    assert!(h.state().buffer.rope().to_string().contains("text = (\"\");"));
    typed(&mut h, "crab\")");
    caret(&mut h, "values = ", "values = ".len());
    typed(&mut h, "[");
    assert!(h.state().buffer.rope().to_string().contains("values = [];"));
    typed(&mut h, "1, 2]");
    caret(&mut h, "total = ", "total = ".len());
    typed(&mut h, "{");
    assert!(h.state().buffer.rope().to_string().contains("total = {};"));
    typed(&mut h, "1 + 2}");
    caret(&mut h, "'é'", 2);
    let before = h.state().buffer.rope().to_string();
    typed(&mut h, "'");
    assert_eq!(h.state().buffer.rope().to_string(), before, "existing closing character quotes are typed over");
    h.state_mut().parse_blocking();
    let state = h.state();
    for needle in ["(\"crab\")", "[1, 2]", "{1 + 2}"] {
        let byte = before.find(needle).unwrap();
        let start = before[..byte].chars().count();
        let end = start + needle.chars().count() - 1;
        assert_eq!(rc_syntax::matching_bracket(state.syntax.tree().unwrap(), state.buffer.rope(), start), Some((start, end)));
    }
    std::fs::write(&path, before).unwrap();
    compile(&path);
}

#[test]
fn markdown_preview_updates_and_switches_views_inside_a_rust_project() {
    use egui_kittest::kittest::Queryable;
    use crate::docs::MdView;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, _) = project("fn main() {}\n");
    let home = tempfile::tempdir().unwrap();
    let readme = dir.path().join("README.md");
    std::fs::write(&readme, "# Rust guide\n\nFerris overview\n\n```rust\nlet answer = 42;\n```\n").unwrap();
    let mut app = crate::screens::test_app(home.path(), true);
    app.open_workspace(dir.path());
    app.open_file(&readme);
    let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
    h.get_by_label("Rust guide");
    h.get_by_label("Ferris overview");
    h.get_by_label(rc_kit::i18n::tr("Preview Only")).click();
    h.run_steps(3);
    assert_eq!(h.state().docs.active_doc().unwrap().md_view, Some(MdView::Preview));
    h.get_by_label("Rust guide");
    let doc = h.state_mut().docs.active_doc_mut().unwrap();
    let text = doc.editor.buffer.rope().to_string();
    let start = text.find("Ferris overview").unwrap();
    doc.editor.replace(start, start + "Ferris overview".len(), "Live Rust documentation");
    h.run_steps(3);
    h.get_by_label("Live Rust documentation");
    h.get_by_label(rc_kit::i18n::tr("Editor Only")).click();
    h.run_steps(3);
    assert_eq!(h.state().docs.active_doc().unwrap().md_view, Some(MdView::Editor));
    assert!(h.query_by_label("Rust guide").is_none());
    h.get_by_label(rc_kit::i18n::tr("Editor and Preview")).click();
    h.run_steps(3);
    h.get_by_label("Live Rust documentation");
    assert!(h.state().insight.session.is_none(), "preview must not start project code");
}

#[test]
fn json_and_xml_assets_format_parse_and_undo_in_a_real_rust_project() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let source = "fn main() { assert!(include_str!(\"config.json\").contains(\"answer\")); assert!(include_str!(\"config.xml\").contains(\"project\")); }\n";
    let (dir, path) = project(source);
    let home = tempfile::tempdir().unwrap();
    for (name, text) in [("config.json", "{\"answer\":42,\"nested\":{\"enabled\":true}}\n"), ("config.xml", "<project>\n<item enabled=\"true\">Rust</item>\n</project>\n")] {
        std::fs::write(dir.path().join("src").join(name), text).unwrap();
    }
    let mut app = crate::screens::test_app(home.path(), true);
    app.open_workspace(dir.path());
    let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
    for (name, language) in [("config.json", rc_syntax::Lang::Json), ("config.xml", rc_syntax::Lang::Xml)] {
        let file = dir.path().join("src").join(name);
        h.state_mut().open_file(&file);
        h.run_steps(3);
        let original = h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();
        h.state_mut().run_insight_action(rc_editor::Action::Reformat, egui::Pos2::ZERO);
        h.run_steps(3);
        let doc = h.state_mut().docs.active_doc_mut().unwrap();
        doc.editor.parse_blocking();
        assert_eq!(doc.editor.lang(), language);
        assert!(!doc.editor.syntax.tree().unwrap().root_node().has_error());
        assert!(doc.editor.buffer.rope().to_string().lines().count() > 1);
        assert!(!doc.editor.syntax.folds().is_empty());
        h.state_mut().save_all_docs();
        let formatted = std::fs::read_to_string(&file).unwrap();
        assert_ne!(formatted, original);
        // Formatting remains a single undoable editor operation.
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Z);
        h.run_steps(2);
        assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string(), original);
    }
    compile(&path);
    assert!(h.state().insight.session.is_none());
}


#[cfg(not(target_os = "macos"))]
#[test]
fn rust_reference_keymap_is_selected_and_routes_native_actions_in_a_cargo_project() {
    use egui_kittest::kittest::Queryable;
    use crate::ui::dialogs::SettingsPage;
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let source = "fn main() {\n    let answer = 42;\n    assert_eq!(answer, 42);\n}\n";
    let (dir, path) = project(source);
    let home = tempfile::tempdir().unwrap();
    let mut app = crate::screens::test_app(home.path(), true);
    app.open_workspace(dir.path());
    app.open_file(&path);
    app.open_settings(SettingsPage::Keymap);
    let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
    h.get_by(|node| node.value().as_deref() == Some("Rider")).click();
    h.run_steps(2);
    h.get_by_label(rc_kit::i18n::tr("RustRover (Windows/Linux)")).click();
    h.run_steps(2);
    h.get_by_label(rc_kit::i18n::tr("OK")).click();
    h.run_steps(3);
    assert_eq!(h.state().settings.keymap, rc_core::settings::KeymapChoice::RustRover);
    assert_eq!(h.state().keymap.kind, rc_editor::KeymapKind::RustRover);
    assert_eq!(rc_core::Settings::load().keymap, rc_core::settings::KeymapChoice::RustRover);
    h.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::A);
    h.run_steps(3);
    assert_eq!(h.state().search_everywhere.as_ref().unwrap().tab, crate::search::SeTab::Actions);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::N);
    h.run_steps(3);
    assert_eq!(h.state().search_everywhere.as_ref().unwrap().tab, crate::search::SeTab::Files);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string(), source);
    assert!(h.state().insight.session.is_none());
    compile(&path);
}
