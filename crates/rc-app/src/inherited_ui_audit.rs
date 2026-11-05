//! Headless verification of inherited UI behavior against real Cargo workspaces.
use crate::app::CrabCraftApp;
use crate::toolwindow::{Side, ToolWindow};
use egui::{Shape, Color32, Rect, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use rc_core::settings::ThemeChoice;
use rc_editor::Action;
use rc_kit::i18n::tr;
use std::path::Path;
use std::time::{Duration, Instant};

#[test]
fn native_rust_debugger_choice_is_explicit_and_persists_without_running_project_code() {
    let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().unwrap();
    let (root, mut harness) = workspace(home.path(), true);
    harness.state_mut().open_settings(crate::ui::dialogs::SettingsPage::Tools);
    harness.run_steps(2);
    harness.get_by_value("GDB").click(); harness.run_steps(2);
    harness.get_by_label("LLDB").click(); harness.run_steps(2);
    harness.get_by_label(tr("OK")).click(); harness.run_steps(2);
    assert_eq!(harness.state().settings.rust_debugger, rc_core::settings::RustDebugger::Lldb);
    assert_eq!(harness.state().debugger.rust_backend, rc_core::settings::RustDebugger::Lldb);
    assert_eq!(rc_core::Settings::load().rust_debugger, rc_core::settings::RustDebugger::Lldb);
    assert!(!root.path().join("build-ran").exists());
    assert!(harness.state().debugger.session.is_none());
    harness.state_mut().open_settings(crate::ui::dialogs::SettingsPage::Tools);
    harness.run_steps(2);
    harness.get_by_value("LLDB").click(); harness.run_steps(2);
    harness.get_by_label("GDB").click(); harness.run_steps(2);
    harness.get_by_label(tr("OK")).click(); harness.run_steps(2);
    assert_eq!(rc_core::Settings::load().rust_debugger, rc_core::settings::RustDebugger::Gdb);
    assert!(!root.path().join("build-ran").exists());
}

fn workspace(home: &Path, dark: bool) -> (tempfile::TempDir, Harness<'static, CrabCraftApp>) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"ui_audit_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    std::fs::write(root.path().join("src/main.rs"), "mod helper;\nfn main() { println!(\"{}\", helper::value()); }\n").unwrap();
    std::fs::write(root.path().join("src/helper.rs"), "pub fn value() -> i32 {\n    // café before a saved caret\n    42\n}\n").unwrap();
    std::fs::write(root.path().join("build.rs"), "fn main() { std::fs::write(\"build-ran\", \"yes\").unwrap(); }\n").unwrap();
    let mut app = crate::screens::test_app(home, dark);
    app.open_workspace(root.path());
    app.open_file(&root.path().join("src/main.rs"));
    (root, crate::screens::harness(app, vec2(1280.0, 800.0)))
}

fn frames(harness: &mut Harness<'_, CrabCraftApp>, label: &str, done: impl Fn(&CrabCraftApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(harness.state()) {
        harness.run_steps(1);
        assert!(Instant::now() < deadline, "{label} did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn keyword_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<Color32> {
    fn visit(shape: &Shape, colors: &mut Vec<Color32>) {
        match shape {
            Shape::Vec(shapes) => for shape in shapes { visit(shape, colors); },
            Shape::Text(text) if text.galley.job.text.contains("fn main") => {
                colors.extend(text.galley.job.sections.iter().map(|section| section.format.color));
            }
            _ => {}
        }
    }
    let mut colors = Vec::new();
    for shape in shapes { visit(&shape.shape, &mut colors); }
    colors
}

#[test]
fn native_light_dark_choices_update_rust_token_colors_and_persist() {
    let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().unwrap();
    let (root, mut harness) = workspace(home.path(), true);
    assert!(harness.ctx.global_style().visuals.dark_mode);
    assert!(keyword_colors(&harness.output().shapes).contains(&rc_kit::theme::scheme(true).keyword));
    harness.state_mut().open_settings(crate::ui::dialogs::SettingsPage::Appearance);
    harness.run_steps(2);
    harness.get_by_value("Islands Dark").click(); harness.run_steps(2);
    harness.get_by_label("Islands Light").click(); harness.run_steps(2);
    harness.get_by_label(tr("OK")).click(); harness.run_steps(3);
    assert!(!harness.ctx.global_style().visuals.dark_mode);
    assert!(keyword_colors(&harness.output().shapes).contains(&rc_kit::theme::scheme(false).keyword));
    assert_eq!(rc_core::Settings::load().theme, ThemeChoice::Light);
    harness.state_mut().open_settings(crate::ui::dialogs::SettingsPage::Appearance);
    harness.run_steps(2);
    harness.get_by_value("Islands Light").click(); harness.run_steps(2);
    harness.get_by_label("Islands Dark").click(); harness.run_steps(2);
    harness.get_by_label(tr("OK")).click(); harness.run_steps(3);
    assert!(harness.ctx.global_style().visuals.dark_mode);
    assert!(keyword_colors(&harness.output().shapes).contains(&rc_kit::theme::scheme(true).keyword));
    assert_eq!(rc_core::Settings::load().theme, ThemeChoice::Dark);
    assert!(!root.path().join("build-ran").exists());
    assert!(!harness.state().docs.active_doc().unwrap().is_modified());
}

fn island_rectangles(shapes: &[egui::epaint::ClippedShape], color: Color32) -> Vec<Rect> {
    fn visit(shape: &Shape, color: Color32, out: &mut Vec<Rect>) {
        match shape {
            Shape::Vec(shapes) => for shape in shapes { visit(shape, color, out); },
            Shape::Rect(rect) if rect.fill == color && rect.rect.width() > 180.0 && rect.rect.height() > 100.0 => out.push(rect.rect),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for shape in shapes { visit(&shape.shape, color, &mut out); }
    out
}

#[test]
fn native_dock_stripes_move_show_hide_and_restore_the_cargo_layout() {
    let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().unwrap();
    let (root, mut harness) = workspace(home.path(), false);
    let islands = island_rectangles(&harness.output().shapes, rc_kit::theme::palette(false).island_bg);
    assert!(islands.iter().enumerate().any(|(index, a)| islands.iter().skip(index + 1).any(|b| !a.intersects(*b))), "native editor and tool window islands must be separate");
    harness.get_by_label(ToolWindow::Tests.title()).click(); harness.run_steps(2);
    assert_eq!(harness.state().dock.bottom, Some(ToolWindow::Tests));
    harness.get_by_label(ToolWindow::Tests.title()).click(); harness.run_steps(2);
    assert!(!harness.state().dock.is_open(ToolWindow::Tests));
    harness.get_by_label(ToolWindow::Tests.title()).click_secondary(); harness.run_steps(2);
    harness.get_by_label(Side::Right.title()).click(); harness.run_steps(2);
    assert_eq!(harness.state().dock.side_of(ToolWindow::Tests), Side::Right);
    harness.get_by_label(ToolWindow::Tests.title()).click(); harness.run_steps(2);
    assert_eq!(harness.state().dock.right, Some(ToolWindow::Tests));
    harness.get_by_label(ToolWindow::Cargo.title()).click(); harness.run_steps(2);
    assert_eq!(harness.state().dock.right, Some(ToolWindow::Cargo));
    harness.get_by_label(ToolWindow::Tests.title()).click(); harness.run_steps(2);
    assert_eq!(harness.state().dock.right, Some(ToolWindow::Tests));
    let size = harness.state().dock.size(ToolWindow::Tests);
    harness.state_mut().save_session();
    let mut app = crate::screens::test_app(home.path(), false);
    app.open_workspace(root.path());
    let restored = crate::screens::harness(app, vec2(1280.0, 800.0));
    assert_eq!(restored.state().dock.side_of(ToolWindow::Tests), Side::Right);
    assert_eq!(restored.state().dock.right, Some(ToolWindow::Tests));
    assert!((restored.state().dock.size(ToolWindow::Tests) - size).abs() <= 2.0);
    harness.set_size(vec2(900.0, 560.0)); harness.run_steps(4);
    let mut visible = Vec::new();
    for window in ToolWindow::ALL {
        if matches!(harness.state().dock.side_of(window), Side::Left | Side::Bottom) {
            if let Some(node) = harness.query_by_label(window.title()) {
                let rect = node.rect();
                if rect.intersects(harness.ctx.content_rect()) { visible.push((window, rect)); }
            }
        }
    }
    assert!(!visible.iter().enumerate().any(|(index, (_, a))| visible.iter().skip(index + 1).any(|(_, b)| a.intersects(*b))), "stripe buttons overlap at the advertised minimum window size: {visible:?}");

    harness.hover_at(egui::pos2(20.0, 280.0));
    harness.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -1000.0), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE });
    harness.run_steps(12);
    harness.get_by_label(ToolWindow::Endpoints.title()).click(); harness.run_steps(2);
    assert_eq!(harness.state().dock.bottom, Some(ToolWindow::Endpoints), "overflowed stripe controls remain reachable by scrolling");
    assert!(!root.path().join("build-ran").exists());
}

#[test]
fn native_recent_files_filter_and_closed_location_restore_the_rust_caret() {
    let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().unwrap();
    let (root, mut harness) = workspace(home.path(), false);
    let main = root.path().join("src/main.rs").canonicalize().unwrap();
    let helper = root.path().join("src/helper.rs").canonicalize().unwrap();
    let manifest = root.path().join("Cargo.toml").canonicalize().unwrap();
    harness.state_mut().open_at(&helper, 2, 5);
    let position = harness.state().docs.active_doc().unwrap().editor.caret();
    harness.state_mut().open_file(&main);
    let index = harness.state().docs.index_of(&helper).unwrap();
    harness.state_mut().close_tab(index);
    harness.state_mut().open_file(&manifest);
    harness.state_mut().open_file(&main);
    let ctx = harness.ctx.clone();
    harness.state_mut().run_ide_action(&ctx, Action::RecentFiles); harness.run_steps(2);
    harness.input_mut().events.push(egui::Event::Text("Cargo.toml".into())); harness.run_steps(2);
    harness.key_press(egui::Key::Enter); harness.run_steps(2);
    assert_eq!(harness.state().docs.active_doc().unwrap().path, manifest, "filtering a single result must reset the previous selection");
    harness.state_mut().run_ide_action(&ctx, Action::RecentFiles); harness.run_steps(2);
    let rect = harness.get_by_label(&helper.display().to_string()).rect();
    harness.get_by_label(&helper.display().to_string()).click(); harness.run_steps(2);
    assert_eq!(harness.state().docs.active_doc().unwrap().path, helper, "row={rect:?}, dialog={}, notices={:?}", harness.state().dialog.is_some(), harness.state().notices.iter().map(|notice| &notice.text).collect::<Vec<_>>());
    harness.state_mut().open_file(&main);
    let index = harness.state().docs.index_of(&helper).unwrap();
    harness.state_mut().close_tab(index);
    let selected = harness.state().nav_back.iter().rev().take(30).position(|(path, saved)| path == &helper && *saved == position).unwrap();
    harness.state_mut().run_ide_action(&ctx, Action::RecentLocations); harness.run_steps(2);
    for _ in 0..selected { harness.key_press(egui::Key::ArrowDown); harness.run_steps(1); }
    harness.key_press(egui::Key::Enter); harness.run_steps(3);
    let doc = harness.state().docs.active_doc().unwrap();
    assert_eq!(doc.path, helper);
    assert_eq!(doc.editor.caret_line_col(), (2, 5));
    assert!(!root.path().join("build-ran").exists());
}

#[test]
fn real_rust_error_marks_paint_hover_and_scroll_in_the_native_editor() {
    let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().unwrap();
    let (root, mut harness) = workspace(home.path(), false);
    let main = root.path().join("src/main.rs").canonicalize().unwrap();
    let mut source = String::from("fn main() {\n");
    for line in 0..220 { source.push_str(&format!("    // filler {line}\n")); }
    source.push_str("    let value: u32 = \"wrong\";\n    println!(\"{value}\");\n}\n");
    std::fs::write(&main, &source).unwrap();
    harness.state_mut().docs.list[0].mtime = None;
    harness.state_mut().docs.on_disk_change(&main);
    harness.state_mut().trust_project();
    frames(&mut harness, "real Rust E0308 diagnostics", |app| app.docs.active_doc().unwrap().editor.diagnostics.iter().any(|diagnostic| diagnostic.code.as_deref() == Some("E0308")));
    harness.state_mut().dock.hide_all();
    harness.state_mut().docs.active_doc_mut().unwrap().editor.goto(0, 0);
    harness.run_steps(3);
    let track = harness.state().docs.active_doc().unwrap().editor.scrollbar_track;
    let color = rc_kit::theme::scheme(false).error;
    fn marks(shape: &Shape, track: Rect, color: Color32, out: &mut Vec<Rect>) {
        match shape {
            Shape::Vec(shapes) => for shape in shapes { marks(shape, track, color, out); },
            Shape::Rect(rect) if rect.fill == color && rect.rect.height() <= 3.0 && track.contains(rect.rect.center()) => out.push(rect.rect),
            _ => {}
        }
    }
    let mut painted = Vec::new();
    for shape in &harness.output().shapes { marks(&shape.shape, track, color, &mut painted); }
    let at = painted.first().expect("real Rust error paints a scrollbar mark").center();
    assert_eq!(harness.state().docs.active_doc().unwrap().editor.scroll_offset().y, 0.0);
    harness.hover_at(at); harness.run_steps(3);
    let lens = harness.state().docs.active_doc().unwrap().editor.scroll_lens.as_ref().expect("error mark shows a preview");
    assert!(lens.message.as_ref().is_some_and(|(severity, message)| *severity == rc_editor::Severity::Error && message.contains("mismatched types")), "{lens:?}");
    harness.drag_at(at); harness.run_steps(1); harness.drop_at(at); harness.run_steps(3);
    assert!(harness.state().docs.active_doc().unwrap().editor.scroll_offset().y > 0.0);
    assert!(harness.state().docs.active_doc().unwrap().editor.scroll_lens.is_none());
    fn has_error_line(shape: &Shape) -> bool {
        match shape { Shape::Vec(shapes) => shapes.iter().any(has_error_line), Shape::Text(text) => text.galley.job.text.contains("let value: u32"), _ => false }
    }
    assert!(harness.output().shapes.iter().any(|shape| has_error_line(&shape.shape)), "clicking the real mark brings its source line into view; scroll={:?}, caret={:?}, mark={at:?}, track={track:?}", harness.state().docs.active_doc().unwrap().editor.scroll_offset(), harness.state().docs.active_doc().unwrap().editor.caret_line_col());
    assert_eq!(harness.state().docs.active_doc().unwrap().editor.caret_line_col().0, 221);
    {
        let editor = &mut harness.state_mut().docs.active_doc_mut().unwrap().editor;
        editor.goto(0, 0); editor.toggle_fold(0); editor.set_scroll_offset(vec2(0.0, 0.0));
    }
    harness.run_steps(3);
    assert!(harness.state().docs.active_doc().unwrap().editor.is_folded(0));
    let track = harness.state().docs.active_doc().unwrap().editor.scrollbar_track;
    let mut folded_marks = Vec::new();
    for shape in &harness.output().shapes { marks(&shape.shape, track, color, &mut folded_marks); }
    let at = folded_marks.first().expect("folded diagnostics retain their stripe mark").center();
    harness.hover_at(at); harness.drag_at(at); harness.run_steps(1); harness.drop_at(at); harness.run_steps(3);
    assert!(!harness.state().docs.active_doc().unwrap().editor.is_folded(0));
    assert_eq!(harness.state().docs.active_doc().unwrap().editor.caret_line_col().0, 221);
    assert!(harness.output().shapes.iter().any(|shape| has_error_line(&shape.shape)));
    assert_eq!(std::fs::read_to_string(main).unwrap(), source);
}
