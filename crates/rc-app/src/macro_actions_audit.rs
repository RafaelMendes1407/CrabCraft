//! Real native Find Action routing for the macro commands, separate from menu registration.
use egui_kittest::kittest::Queryable;
use rc_editor::Action;
use rc_kit::i18n::tr;
#[test]
fn find_action_lists_macro_commands_and_opens_the_native_macro_tester() {
    let _lock = crate::screens::HOME_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("Cargo.toml"),"[package]\nname=\"macro_action_catalog\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
    let file = root.path().join("src/main.rs");
    std::fs::write(
        &file,
        "macro_rules! twice {($e:expr)=>{$e+$e};}\nfn main(){assert_eq!(twice!(21),42);}\n",
    )
    .unwrap();
    let mut app = crate::screens::test_app(home.path(), false);
    app.open_workspace(root.path());
    app.open_file_and_wait(&file);
    let items = crate::search::action_items();
    for action in [
        Action::ExpandMacro,
        Action::ExpandMacroStep,
        Action::TestDeclarativeMacro,
    ] {
        assert_eq!(items.iter().filter(|(_, item)| *item == action).count(), 1);
    }
    let mut h = crate::screens::harness(app, egui::vec2(1280., 900.));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while h.state().docs.active_doc().is_none() {
        assert!(std::time::Instant::now() < deadline);
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.state_mut()
        .run_ide_action(&egui::Context::default(), Action::FindAction);
    h.run_steps(2);
    h.state_mut().search_everywhere.as_mut().unwrap().text = tr("Test Declarative Macro…").into();
    h.run_steps(2);
    h.get_by_label(tr("Test Declarative Macro…")).click();
    h.run_steps(3);
    assert!(h.state().search_everywhere.is_none());
    assert!(h.state().macro_tester.open);
    assert!(!h.state().macro_tester.pending());
    assert!(h.state().insight.session.is_none());
    assert!(!root.path().join("target").exists());
}
