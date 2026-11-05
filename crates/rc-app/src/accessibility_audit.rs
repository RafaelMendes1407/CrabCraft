//! Native AccessKit contract against a real Rust source document.
use egui::accesskit::{Action, ActionData, ActionRequest, Role, TextPosition, TextSelection};
use egui_kittest::kittest::{NodeT, Queryable};
use rc_kit::i18n::trf;

#[test]
fn source_editor_exposes_text_selection_unicode_readonly_and_bounded_large_context() {
    let _lock = crate::screens::HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("Cargo.toml"),"[package]\nname=\"accessibility_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
    let source = "fn main() {\n    let café = 41;\n    assert_eq!(café, 42);\n}\n";
    let path = root.path().join("src/main.rs");
    std::fs::write(&path, source).unwrap();
    let mut app = crate::screens::test_app(home.path(), false);
    app.open_workspace(root.path());
    app.open_file(&path);
    let mut h = crate::screens::harness(app, egui::vec2(1280.0, 900.0));
    h.run_steps(2);
    let label = trf("Source code: {file}", &[("file", &"main.rs")]);
    let node = h.get_by_role_and_label(Role::MultilineTextInput, &label);
    assert_eq!(node.value().as_deref(), Some(source));
    let (target_node, target_tree) = node.accesskit_node().locate();
    let run = node
        .children()
        .find(|child| child.value().is_some_and(|text| text.contains("café = 41")))
        .expect("accessible Unicode line");
    let run_id = run.accesskit_node().locate().0;
    let line = "    let café = 41;\n";
    let at = line[..line.find("41").unwrap()].chars().count();
    h.event(egui::Event::AccessKitActionRequest(ActionRequest {
        target_node,
        target_tree,
        action: Action::SetTextSelection,
        data: Some(ActionData::SetTextSelection(TextSelection {
            anchor: TextPosition {
                node: run_id,
                character_index: at,
            },
            focus: TextPosition {
                node: run_id,
                character_index: at + 2,
            },
        })),
    }));
    h.run_steps(2);
    let editor = &h.state().docs.active_doc().unwrap().editor;
    let selected = editor.selection.primary();
    assert_eq!(
        editor
            .buffer
            .rope()
            .slice(selected.start()..selected.end())
            .to_string(),
        "41"
    );
    h.get_by_role_and_label(Role::MultilineTextInput, &label)
        .type_text("42");
    h.run_steps(2);
    let corrected = h
        .state()
        .docs
        .active_doc()
        .unwrap()
        .editor
        .buffer
        .rope()
        .to_string();
    assert!(corrected.contains("café = 42;"));
    h.state_mut()
        .docs
        .active_doc_mut()
        .unwrap()
        .editor
        .read_only = true;
    h.run_steps(2);
    assert!(
        h.get_by_role_and_label(Role::MultilineTextInput, &label)
            .accesskit_node()
            .is_read_only()
    );
    h.get_by_role_and_label(Role::MultilineTextInput, &label)
        .type_text("99");
    h.run_steps(2);
    assert_eq!(
        h.state()
            .docs
            .active_doc()
            .unwrap()
            .editor
            .buffer
            .rope()
            .to_string(),
        corrected
    );
    std::fs::write(&path, &corrected).unwrap();
    let output = std::process::Command::new("cargo")
        .args(["run", "--offline", "--quiet"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let before = h
        .state()
        .docs
        .active_doc()
        .unwrap()
        .editor
        .selection
        .clone();
    h.event(egui::Event::AccessKitActionRequest(ActionRequest {
        target_node,
        target_tree,
        action: Action::SetTextSelection,
        data: Some(ActionData::SetTextSelection(TextSelection {
            anchor: TextPosition {
                node: run_id,
                character_index: usize::MAX,
            },
            focus: TextPosition {
                node: run_id,
                character_index: usize::MAX,
            },
        })),
    }));
    h.run_steps(2);
    assert_eq!(
        h.state().docs.active_doc().unwrap().editor.selection,
        before,
        "invalid character offsets must be refused"
    );
    let large = format!(
        "{}{}",
        "// accessible 🦀 context\n".repeat(20_000),
        corrected
    );
    let big = root.path().join("src/large.rs");
    std::fs::write(&big, large).unwrap();
    h.state_mut().open_file(&big);
    h.run_steps(2);
    let label = trf("Source code: {file}", &[("file", &"large.rs")]);
    assert!(
        h.get_by_role_and_label(Role::MultilineTextInput, &label)
            .value()
            .unwrap()
            .chars()
            .count()
            <= 16_384
    );
}
