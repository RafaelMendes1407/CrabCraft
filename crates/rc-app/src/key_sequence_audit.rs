//! Real Cargo project, persisted native shortcut recording and ordinary editor event routing.
#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use rc_kit::i18n::tr;
    #[test]
    fn native_mac_reference_events_route_in_a_real_rust_project() {
        use rc_core::settings::KeymapChoice;
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(crate::app::keymap_kind_for(KeymapChoice::RustRover,true),rc_editor::KeymapKind::RustRoverMac);
        assert_eq!(crate::app::keymap_kind_for(KeymapChoice::RustRover,false),rc_editor::KeymapKind::RustRover);
        let home = tempfile::tempdir().unwrap(); let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("Cargo.toml"),"[package]\nname='mac-keymap-audit'\nversion='0.1.0'\nedition='2024'\n[workspace]\n").unwrap();
        let path = project.path().join("src/main.rs");
        std::fs::write(&path,"fn main() { assert_eq!(6 * 7, 42); }\n").unwrap();
        let built = std::process::Command::new("cargo").args(["check","--offline","--manifest-path"]).arg(project.path().join("Cargo.toml")).output().unwrap();
        assert!(built.status.success());
        std::fs::write(project.path().join("build.rs"),"fn main(){ std::fs::write(\"unexpected-build\",\"bad\").unwrap(); }").unwrap();
        let mut app = crate::screens::test_app(home.path(),true);
        app.settings.keymap = KeymapChoice::RustRover;
        app.settings.save().unwrap();
        // Emulate macOS key events in the headless UI on the test host. Native NSApp/window
        // integration is not exercised by this test and is not claimed as product parity.
        app.keymap = rc_editor::Keymap::new(crate::app::keymap_kind_for(KeymapChoice::RustRover,true));
        app.open_workspace_and_wait(project.path()); app.open_file_and_wait(&path);
        let mut h = crate::screens::harness(app,egui::vec2(1280.0,800.0));
        let command_shift = egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        h.key_press_modifiers(command_shift,egui::Key::O); h.run_steps(3);
        assert_eq!(h.state().search_everywhere.as_ref().unwrap().tab,crate::search::SeTab::Files);
        h.key_press(egui::Key::Escape); h.run_steps(2);
        h.key_press_modifiers(command_shift,egui::Key::A); h.run_steps(3);
        assert_eq!(h.state().search_everywhere.as_ref().unwrap().tab,crate::search::SeTab::Actions);
        assert_eq!(rc_core::Settings::load().keymap,KeymapChoice::RustRover);
        assert!(h.state().insight.session.is_none() && h.state().settings.trusted_projects.is_empty());
        assert!(!project.path().join("unexpected-build").exists());
    }
    #[test]
    fn standalone_editor_shortcuts_do_not_insert_the_second_key_or_edit_readonly_sources() {
        use rc_editor::{EditorState, EditorView, EditorConfig, Keymap, KeymapKind};
        let (first,second) = rc_editor::keymap::parse_sequence("ctrl+K C").unwrap();
        let map = Keymap::new(KeymapKind::Rider).with_sequences(&[(rc_editor::Action::CommentLine,first,second)]);
        let config = EditorConfig::default();
        let state = EditorState::new(1,Some("main.rs".into()),"fn main() {}\n");
        let mut h = egui_kittest::Harness::builder().with_step_dt(1.0 / 60.0).build_ui_state(move |ui,state: &mut EditorState| {
            EditorView::new(state,&config,&map).focus(true).show(ui);
        },state);
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::CTRL,egui::Key::K); h.run_steps(1);
        h.key_press_modifiers(egui::Modifiers::NONE,egui::Key::C); h.event(egui::Event::Text("c".into())); h.run_steps(2);
        let expected = "// fn main() {}\n";
        assert_eq!(h.state().buffer.rope().to_string(),expected);
        h.state_mut().read_only = true;
        h.key_press_modifiers(egui::Modifiers::CTRL,egui::Key::K); h.run_steps(1);
        h.key_press_modifiers(egui::Modifiers::NONE,egui::Key::C); h.event(egui::Event::Text("c".into())); h.run_steps(2);
        assert_eq!(h.state().buffer.rope().to_string(),expected);
    }
    #[test]
    fn native_two_step_shortcuts_record_persist_cancel_timeout_and_fold_rust() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap(); let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[package]\nname='key-sequence-audit'\nversion='0.1.0'\nedition='2024'\n[workspace]\n").unwrap();
        let source = project.path().join("src/main.rs");
        let text = "fn main() {\n    let value = 42;\n    assert_eq!(value, 42);\n}\n";
        std::fs::write(&source, text).unwrap();
        let compiled = std::process::Command::new("cargo").args(["check", "--offline", "--manifest-path"]).arg(project.path().join("Cargo.toml")).output().unwrap();
        assert!(compiled.status.success());
        std::fs::write(project.path().join("build.rs"), "fn main(){ std::fs::write(\"build-ran\", \"bad\").unwrap(); }\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.settings.shortcuts.insert("FoldAll".into(), "ctrl+K ctrl+0".into());
        app.settings.shortcuts.insert("UnfoldAll".into(), "ctrl+K J".into());
        app.keymap = crate::app::build_keymap(&app.settings);
        app.open_workspace_and_wait(project.path()); app.open_file_and_wait(&source);
        app.open_settings(crate::ui::dialogs::SettingsPage::Keymap);
        let mut first = true;
        let mut frame = eframe::Frame::_new_kittest();
        let mut h = egui_kittest::Harness::builder().with_step_dt(1.0 / 60.0).with_size(egui::vec2(1400.0, 1000.0)).build_ui_state(move |ui, app: &mut crate::app::CrabCraftApp| {
            if std::mem::take(&mut first) { rc_kit::theme::install_fonts(ui.ctx()); return; }
            let ctx = ui.ctx().clone();
            eframe::App::logic(app, &ctx, &mut frame);
            let mut root = ui.new_child(egui::UiBuilder::new().max_rect(ctx.content_rect()));
            eframe::App::ui(app, &mut root, &mut frame);
        }, app);
        h.run_steps(8);
        h.ctx.data_mut(|data| data.insert_temp(egui::Id::new("shortcut_filter"), "Settings".to_string())); h.run_steps(3);
        h.get_by_label(tr("Record two-step shortcuts")).click(); h.run_steps(2);
        h.get_by_label(tr("Record Shortcut")).click(); h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::K); h.run_steps(2);
        assert!(h.query_by_label(tr("Press the second shortcut…")).is_some());
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::S); h.run_steps(2);
        h.get_by_label(tr("OK")).click(); h.run_steps(3);
        assert_eq!(rc_core::Settings::load().shortcuts.get("ShowSettings").map(String::as_str), Some("ctrl+K ctrl+S"));
        assert!(h.state().keymap.lookup(egui::Key::K, egui::Modifiers::CTRL).is_none());
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::K); h.run_steps(2);
        assert!(h.state().dialog.is_none());
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::S); h.run_steps(2);
        assert!(matches!(h.state().dialog, Some(crate::ui::dialogs::Dialog::Settings { .. })));
        h.get_by_label(tr("Cancel")).click(); h.run_steps(2);
        h.state_mut().docs.active_doc_mut().unwrap().editor.goto(4,0);
        h.state_mut().focus_editor = true; h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::K); h.run_steps(1);
        h.key_press(egui::Key::Escape); h.run_steps(1);
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Num0); h.run_steps(1);
        assert!(!h.state().docs.active_doc().unwrap().editor.is_folded(0));
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::K); h.run_steps(140);
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Num0); h.run_steps(1);
        assert!(!h.state().docs.active_doc().unwrap().editor.is_folded(0));
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::K); h.run_steps(1);
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Num0); h.run_steps(2);
        assert!(h.state().docs.active_doc().unwrap().editor.is_folded(0));
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::K); h.run_steps(1);
        h.key_press_modifiers(egui::Modifiers::NONE, egui::Key::J); h.event(egui::Event::Text("j".into())); h.run_steps(2);
        assert!(!h.state().docs.active_doc().unwrap().editor.is_folded(0));
        assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string(), text);
        h.state_mut().open_settings(crate::ui::dialogs::SettingsPage::Keymap); h.run_steps(4);
        h.get_by_label(tr("Record Shortcut")).click(); h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::CTRL,egui::Key::K); h.run_steps(2);
        h.get_by_label(tr("Cancel")).click(); h.run_steps(2);
        assert!(h.ctx.data(|data| data.get_temp::<String>(egui::Id::new("recording_shortcut"))).is_none());
        h.key_press_modifiers(egui::Modifiers::CTRL,egui::Key::K); h.run_steps(1);
        h.key_press_modifiers(egui::Modifiers::CTRL,egui::Key::S); h.run_steps(2);
        assert!(matches!(h.state().dialog,Some(crate::ui::dialogs::Dialog::Settings { .. })));
        assert!(!project.path().join("build-ran").exists());
    }
}
