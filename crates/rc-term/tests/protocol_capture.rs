//! Protocol inspection never consumes ordinary terminal input while capture is locked.
#![cfg(unix)]
use rc_core::Waker;
use rc_term::{SpawnSpec, TermConfig, TermSession, TerminalView};

#[test]
fn a_locked_native_terminal_view_never_sends_interleaved_keyboard_input() {
    let mut spec = SpawnSpec::command("/bin/cat", std::iter::empty::<&str>(), None);
    spec.capture_output = true;
    let session = TermSession::spawn(&spec, Waker::noop()).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(700.0, 300.0)).build_ui_state(
        |ui, state: &mut (TermSession, bool)| {
            TerminalView::new(&state.0, &TermConfig::default()).focus(true).input_enabled(state.1).show(ui);
        },
        (session, false),
    );
    h.run_steps(3);
    h.event(egui::Event::Text("blocked".into()));
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().0.input_epoch(), 0);
    assert_eq!(h.state().0.output_cursor(), Some(0));
    h.state_mut().1 = true;
    h.run_steps(2);
    h.event(egui::Event::Text("allowed".into()));
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(h.state().0.wait_for_text("allowed", std::time::Duration::from_secs(5)));
    let bytes = h.state().0.output_since(0).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("allowed"));
    assert!(!String::from_utf8_lossy(&bytes).contains("blocked"));
    assert!(h.state().0.input_epoch() > 0);
}

#[test]
fn ordinary_shell_output_is_not_captured_by_default() {
    let spec = SpawnSpec::command("/bin/sh", ["-c", "printf normal"], None);
    let session = TermSession::spawn(&spec, Waker::noop()).unwrap();
    assert!(session.wait_for_text("normal", std::time::Duration::from_secs(5)));
    assert!(session.output_cursor().is_none());
    assert!(session.output_tail().is_none());
    assert!(session.output_since(0).is_err());
}
