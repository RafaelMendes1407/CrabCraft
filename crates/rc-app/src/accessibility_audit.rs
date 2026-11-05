//! Native AccessKit contract against a real Rust source document.
use egui::accesskit::{Action, ActionData, ActionRequest, Role, TextPosition, TextSelection};
use egui_kittest::kittest::{NodeT, Queryable};
use rc_kit::i18n::trf;

fn runs(root: egui_kittest::Node<'_>) -> Vec<egui_kittest::Node<'_>> {
    let mut pending = vec![root];
    let mut out = Vec::new();
    while let Some(node) = pending.pop() {
        if node.accesskit_node().role() == Role::TextRun {
            out.push(node);
        } else {
            pending.extend(node.children());
        }
    }
    out.reverse();
    out
}

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
    app.open_file_and_wait(&path);
    let mut h = crate::screens::harness(app, egui::vec2(1280.0, 900.0));
    h.run_steps(2);
    let label = trf("Source code: {file}", &[("file", &"main.rs")]);
    let node = h.get_by_role_and_label(Role::MultilineTextInput, &label);
    assert_eq!(node.value().as_deref(), Some(source));
    let (target_node, target_tree) = node.accesskit_node().locate();
    let run = runs(node)
        .into_iter()
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
    h.state_mut().open_file_and_wait(&big);
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
    let editor = h.get_by_role_and_label(Role::MultilineTextInput, &label);
    let (target_node, target_tree) = editor.accesskit_node().locate();
    let old_run = runs(editor)
        .into_iter()
        .find(|child| child.value().is_some_and(|value| !value.is_empty()))
        .unwrap()
        .accesskit_node()
        .locate()
        .0;
    h.state_mut()
        .docs
        .active_doc_mut()
        .unwrap()
        .editor
        .goto(19_999, 0);
    h.run_steps(3);
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
                node: old_run,
                character_index: 0,
            },
            focus: TextPosition {
                node: old_run,
                character_index: 1,
            },
        })),
    }));
    h.run_steps(2);
    assert_eq!(
        h.state().docs.active_doc().unwrap().editor.selection,
        before,
        "a request from a previous large-file text window must not select different source"
    );
}

#[test]
fn project_tree_supports_expansion_focus_and_open_without_project_execution() {
    let _lock = crate::screens::HOME_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname=\"accessible_tree\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("src/main.rs"),
        "fn main(){println!(\"accessible Rust\");}\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("build.rs"),
        "fn main(){std::fs::write(\"build-marker\",\"ran\").unwrap();}",
    )
    .unwrap();
    for index in 0..80 {
        std::fs::write(
            root.path().join(format!("src/p{index:03}.rs")),
            "pub const VALUE:usize=42;\n",
        )
        .unwrap();
    }
    let mut app = crate::screens::test_app(home.path(), true);
    app.open_workspace(root.path());
    let mut h = crate::screens::harness(app, egui::vec2(1400.0, 1000.0));
    h.run_steps(3);
    let _tree = h.get_by_role_and_label(Role::Tree, rc_kit::i18n::tr("Project files"));
    let src = h.get_by_role_and_label(Role::TreeItem, "src");
    assert_eq!(src.accesskit_node().data().is_expanded(), Some(false));
    let mut ancestor = src.parent();
    let mut parent_item = None;
    while let Some(node) = ancestor {
        if node.accesskit_node().role() == Role::TreeItem {
            parent_item = Some(node);
            break;
        }
        ancestor = node.parent();
    }
    assert!(
        parent_item.is_some(),
        "the platform must get actual nested tree parents, not only an unmapped level attribute"
    );
    let src_level = src.accesskit_node().level().unwrap_or(0);
    let (target_node, target_tree) = src.accesskit_node().locate();
    let request = |action| {
        egui::Event::AccessKitActionRequest(ActionRequest {
            target_node,
            target_tree,
            action,
            data: None,
        })
    };
    h.event(request(Action::Expand));
    h.run_steps(3);
    assert_eq!(
        h.get_by_role_and_label(Role::TreeItem, "src")
            .accesskit_node()
            .data()
            .is_expanded(),
        Some(true)
    );
    h.event(request(Action::Expand));
    h.run_steps(2);
    assert_eq!(
        h.get_by_role_and_label(Role::TreeItem, "src")
            .accesskit_node()
            .data()
            .is_expanded(),
        Some(true),
        "Expand is idempotent"
    );
    let file = h.get_by_role_and_label(Role::TreeItem, "main.rs");
    assert!(file.accesskit_node().level().unwrap_or(0) > src_level);
    let (file_node, file_tree) = file.accesskit_node().locate();
    h.event(egui::Event::AccessKitActionRequest(ActionRequest {
        target_node: file_node,
        target_tree: file_tree,
        action: Action::Focus,
        data: None,
    }));
    h.run_steps(3);
    assert_eq!(
        h.get_by_role_and_label(Role::TreeItem, "main.rs")
            .accesskit_node()
            .is_selected(),
        Some(true)
    );
    h.event(egui::Event::AccessKitActionRequest(ActionRequest {
        target_node: file_node,
        target_tree: file_tree,
        action: Action::Click,
        data: None,
    }));
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        h.run_steps(2);
        if h.state()
            .docs
            .active_doc()
            .is_some_and(|doc| doc.path.ends_with("src/main.rs"))
        {
            break;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.event(egui::Event::AccessKitActionRequest(ActionRequest {
        target_node: file_node,
        target_tree: file_tree,
        action: Action::Focus,
        data: None,
    }));
    h.run_steps(3);
    for _ in 0..65 {
        h.key_press(egui::Key::ArrowDown);
        h.run_steps(2);
    }
    let late = h.get_by_role_and_label(Role::TreeItem, "p064.rs");
    let mut ancestor = late.parent();
    let mut tree_parents = 0;
    while let Some(node) = ancestor {
        tree_parents += usize::from(node.accesskit_node().role() == Role::TreeItem);
        ancestor = node.parent();
    }
    assert!(
        tree_parents >= 2,
        "virtualized ancestors must remain in the real tree"
    );
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(3);
    h.event(request(Action::Collapse));
    h.run_steps(3);
    assert_eq!(
        h.get_by_role_and_label(Role::TreeItem, "src")
            .accesskit_node()
            .data()
            .is_expanded(),
        Some(false)
    );
    assert!(!root.path().join("build-marker").exists());
    assert!(h.state().runner.cargo_workspace.is_none());
}

#[test]
fn source_geometry_uses_tabs_and_visual_fold_rows_and_selection_unfolds_source() {
    let _lock = crate::screens::HOME_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("Cargo.toml"),"[package]\nname=\"accessible_geometry\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
    let source =
        "fn helper() {\n\tlet café = 42;\n\tassert_eq!(café, 42);\n}\nfn main() { helper(); }\n";
    let path = root.path().join("src/main.rs");
    std::fs::write(&path, source).unwrap();
    let mut app = crate::screens::test_app(home.path(), true);
    app.open_workspace(root.path());
    app.open_file_and_wait(&path);
    app.docs.active_doc_mut().unwrap().editor.indent.tab_width = 8;
    let mut h = crate::screens::harness(app, egui::vec2(1400.0, 1000.0));
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !h
        .state()
        .docs
        .active_doc()
        .unwrap()
        .editor
        .syntax
        .is_current()
    {
        h.run_steps(2);
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let label = trf("Source code: {file}", &[("file", &"main.rs")]);
    let editor = h.get_by_role_and_label(Role::MultilineTextInput, &label);
    let (target_node, target_tree) = editor.accesskit_node().locate();
    let tabbed = runs(editor)
        .into_iter()
        .find(|node| node.value().is_some_and(|text| text.contains("café")))
        .unwrap();
    let positions = tabbed
        .accesskit_node()
        .data()
        .character_positions()
        .unwrap();
    assert!(
        positions[1] > positions[2] - positions[1],
        "a tab occupies multiple painted character cells"
    );
    let main = runs(editor)
        .into_iter()
        .find(|node| node.value().is_some_and(|text| text.starts_with("fn main")))
        .unwrap();
    let before = main.accesskit_node().data().bounds().unwrap().y0;
    h.state_mut()
        .docs
        .active_doc_mut()
        .unwrap()
        .editor
        .fold_at(0);
    h.run_steps(3);
    assert!(h.state().docs.active_doc().unwrap().editor.is_folded(0));
    let editor = h.get_by_role_and_label(Role::MultilineTextInput, &label);
    let main = runs(editor)
        .into_iter()
        .find(|node| node.value().is_some_and(|text| text.starts_with("fn main")))
        .unwrap();
    assert!(main.accesskit_node().data().bounds().unwrap().y0 < before);
    let hidden = runs(editor)
        .into_iter()
        .find(|node| node.value().is_some_and(|text| text.contains("café =")))
        .unwrap();
    let bounds = hidden.accesskit_node().data().bounds().unwrap();
    assert_eq!(
        bounds.y0, bounds.y1,
        "folded logical text has no fake painted area"
    );
    let run_id = hidden.accesskit_node().locate().0;
    let at = "\tlet café = ".chars().count();
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
    h.run_steps(3);
    assert!(!h.state().docs.active_doc().unwrap().editor.is_folded(0));
    let doc = h.state().docs.active_doc().unwrap();
    let range = doc.editor.selection.primary();
    assert_eq!(
        doc.editor
            .buffer
            .rope()
            .slice(range.start()..range.end())
            .to_string(),
        "42"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
}

#[test]
fn terminal_exposes_bounded_unicode_rust_output_and_notices_are_live_regions() {
    let _lock = crate::screens::HOME_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("Cargo.toml"),"[package]\nname=\"accessible_terminal\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
    std::fs::write(
        root.path().join("src/main.rs"),
        "fn main(){println!(\"Rust café 界 🦀\");}\n",
    )
    .unwrap();
    let mut app = crate::screens::test_app(home.path(), true);
    app.open_workspace(root.path());
    app.trust_project_tools_for_test();
    let output = std::process::Command::new("cargo")
        .args(["run", "--offline", "--quiet"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let terminal = rc_term::TermSession::output_only(80, 20, rc_core::Waker::noop());
    terminal.feed(&output.stdout);
    let id = terminal.id();
    app.terminals.push(terminal);
    app.dock.show(crate::toolwindow::ToolWindow::Terminal);
    app.notify(rc_kit::i18n::tr("Build"), false);
    let mut h = crate::screens::harness(app, egui::vec2(1400.0, 1000.0));
    h.run_steps(3);
    let terminal = h.get_by_role(Role::Terminal);
    let text = terminal.value().unwrap();
    assert!(text.contains("Rust café 界 🦀"));
    assert!(text.chars().count() <= 16_384);
    assert!(terminal.accesskit_node().is_read_only());
    assert_eq!(
        terminal.accesskit_node().data().live(),
        Some(egui::accesskit::Live::Polite)
    );
    assert!(terminal.accesskit_node().supports_text_ranges());
    let (target_node, target_tree) = terminal.accesskit_node().locate();
    let epoch = h.state().terminals.get(id).unwrap().session.input_epoch();
    h.event(egui::Event::AccessKitActionRequest(ActionRequest {
        target_node,
        target_tree,
        action: Action::SetValue,
        data: Some(ActionData::Value("do-not-execute\n".into())),
    }));
    h.run_steps(2);
    assert_eq!(
        h.state().terminals.get(id).unwrap().session.input_epoch(),
        epoch
    );
    let notice = h.get_by_role(Role::Status);
    assert_eq!(
        notice.accesskit_node().data().live(),
        Some(egui::accesskit::Live::Polite)
    );
    h.state_mut().notify(rc_kit::i18n::tr("Error"), true);
    h.run_steps(3);
    assert_eq!(
        h.get_by_role(Role::Alert).accesskit_node().data().live(),
        Some(egui::accesskit::Live::Assertive)
    );
}
