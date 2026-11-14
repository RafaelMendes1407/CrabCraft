//! Real Rust file and headless native editor evidence for the large-file policy.
use egui_kittest::Harness;
use rc_editor::{EditorConfig, EditorState, EditorView, Keymap, KeymapKind};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
fn native_editor_navigates_edits_and_protects_large_real_rust_files() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join("src")).unwrap();
    std::fs::write(project.path().join("Cargo.toml"), "[package]\nname='large-file-audit'\nversion='0.1.0'\nedition='2024'\n[workspace]\n").unwrap();
    let path = project.path().join("src/main.rs");
    for (bytes, read_only) in [(2 * 1024 * 1024, false), (9 * 1024 * 1024, true)] {
        let mut source = String::with_capacity(bytes + 100);
        let line = "// Large Rust source: navigation, UTF-8 café, highlighting and safe read-only fallback.\n";
        while source.len() < bytes {
            source.push_str(line);
        }
        source.push_str("fn main() { assert_eq!(6 * 7, 42); }\n");
        std::fs::write(&path, &source).unwrap();
        let mut docs = crate::docs::Docs::new(rc_syntax::ParserPool::new(Arc::new(|| {})));
        let index = docs.open(&path).unwrap();
        assert_eq!(docs.list[index].editor.read_only, read_only);
        if !read_only {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !docs.list[index].editor.syntax.is_current() {
                docs.poll_parses();
                assert!(Instant::now() < deadline, "large Rust file did not finish background parsing");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let state = docs.list.remove(index).editor;
        let config = EditorConfig::default();
        let keymap = Keymap::new(KeymapKind::VsCode);
        let mut first = true;
        let mut harness = Harness::builder().with_size(egui::vec2(1100.0, 650.0)).build_ui_state(
            move |ui, state: &mut EditorState| {
                if std::mem::take(&mut first) {
                    rc_kit::theme::install_fonts(ui.ctx());
                    return;
                }
                EditorView::new(state, &config, &keymap).focus(true).show(ui);
            },
            state,
        );
        harness.run_steps(3);
        let started = Instant::now();
        let lines = harness.state().buffer.len_lines();
        for line in [lines / 2, lines - 2, 0, lines / 4] {
            harness.state_mut().goto(line, 0);
            harness.run_steps(2);
            assert_eq!(harness.state().caret_line_col().0, line);
        }
        eprintln!("large Rust editor: {} bytes, {} lines, readonly={}, eight navigation frames={:?}", source.len(), lines, read_only, started.elapsed());
        harness.state_mut().goto(0, 0);
        harness.event(egui::Event::Text("// edited\n".into()));
        harness.run_steps(1);
        let changed = harness.state().buffer.rope().to_string();
        if read_only {
            assert_eq!(changed, source);
        } else {
            assert!(changed.starts_with("// edited\n"));
            harness.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Z);
            harness.run_steps(1);
            assert_eq!(harness.state().buffer.rope().to_string(), source);
        }
        std::fs::write(&path, changed).unwrap();
        let built =
            std::process::Command::new("cargo").args(["check", "--offline", "--manifest-path"]).arg(project.path().join("Cargo.toml")).output().unwrap();
        assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    }
    let huge = project.path().join("oversized.rs");
    let file = std::fs::File::create(&huge).unwrap();
    file.set_len(129 * 1024 * 1024).unwrap();
    let mut docs = crate::docs::Docs::new(rc_syntax::ParserPool::new(Arc::new(|| {})));
    assert!(docs.open(&huge).is_err());
    assert!(docs.list.is_empty());
}
