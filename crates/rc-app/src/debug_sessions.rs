//! Independent debugger state, consoles and watches; only the selected session paints editors.
use crate::{app::CrabCraftApp, debugger::{Debugger, DebugOutput}, runner::RunConfig};
use rc_kit::i18n::{tr, trf};

struct Parked {
    id: u64, debugger: Debugger,
    test: Option<(RunConfig, rc_cargo::RunTarget)>,
}
pub struct Sessions { parked: Vec<Parked>, current: u64, next: u64 }
impl Default for Sessions { fn default() -> Self { Self { parked: Vec::new(), current: 1, next: 2 } } }

impl CrabCraftApp {
    /// Explicitly start another session; ordinary Debug/Restart keeps its existing meaning.
    pub fn debug_in_new_session(&mut self) {
        if let Err(error) = self.runner.require_trust() { self.notify(error.to_string(), true); return; }
        if self.debug_sessions.parked.len() >= 7 {
            self.notify(tr("Close a debug session before starting another (limit: 8)"), true); return;
        }
        if self.debugger.session.is_some() || self.debugger.downloading.is_some() {
            let mut fresh = Debugger::new(self.debugger.waker());
            fresh.rust_backend = self.settings.rust_debugger;
            fresh.gdb_path = self.settings.gdb_path.clone();
            fresh.lldb_dap_path = self.settings.lldb_dap_path.clone();
            fresh.hex = self.debugger.hex;
            fresh.env = self.debugger.env.clone();
            fresh.conditions = self.debugger.conditions.clone();
            fresh.logs = self.debugger.logs.clone();
            fresh.once = self.debugger.once.clone();
            fresh.groups = self.debugger.groups.clone();
            fresh.muted = self.debugger.muted;
            let previous = std::mem::replace(&mut self.debugger, fresh);
            self.debug_sessions.parked.push(Parked { id: self.debug_sessions.current, debugger: previous, test: self.debug_test.take() });
            self.debug_sessions.current = self.debug_sessions.next;
            self.debug_sessions.next = self.debug_sessions.next.saturating_add(1);
        }
        self.start_debugging();
    }

    pub fn select_debug_session(&mut self, id: u64) {
        let Some(parked) = self.debug_sessions.parked.iter_mut().find(|s| s.id == id) else { return; };
        std::mem::swap(&mut parked.id, &mut self.debug_sessions.current);
        std::mem::swap(&mut parked.debugger, &mut self.debugger);
        std::mem::swap(&mut parked.test, &mut self.debug_test);
        self.debugger.became_selected();
        if let Some(index) = self.runner.configs.iter().position(|c| c.name == self.debugger.name) { self.runner.selected = index; }
        let outputs = self.debugger.update(&mut self.docs);
        self.handle_debug(outputs);
        if let Some((path, line)) = self.debugger.session.as_ref().filter(|s| s.state().is_stopped()).and_then(|s| s.execution_point()) {
            self.handle_debug(vec![DebugOutput::ShowLine(path, line.saturating_sub(1) as usize)]);
        }
    }

    pub fn stop_all_debug_sessions(&mut self) {
        self.debugger.stop();
        for parked in &mut self.debug_sessions.parked { parked.debugger.stop(); }
    }

    pub fn poll_background_debug_sessions(&mut self) {
        let mut notifications = Vec::new();
        for parked in &mut self.debug_sessions.parked {
            for output in parked.debugger.update_background(&mut self.docs) {
                match output {
                    DebugOutput::Notify(message, error) => notifications.push(DebugOutput::Notify(trf("Session {number}: {message}", &[("number", &parked.id), ("message", &message)]), error)),
                    DebugOutput::BreakpointsChanged => notifications.push(DebugOutput::BreakpointsChanged),
                    DebugOutput::ShowLine(..) | DebugOutput::ShowDebugWindow => {},
                }
            }
        }
        self.handle_debug(notifications);
    }
}

pub fn selector(app: &mut CrabCraftApp, ui: &mut egui::Ui) {
    let current = app.debug_sessions.current;
    let mut selected = current;
    ui.horizontal_wrapped(|ui| {
        let text = trf("Session {number}: {name}", &[("number", &current), ("name", &app.debugger.name)]);
        egui::ComboBox::from_id_salt("debug-session-selector").selected_text(text).show_ui(ui, |ui| {
            ui.selectable_value(&mut selected, current, trf("Session {number}: {name}", &[("number", &current), ("name", &app.debugger.name)]));
            for parked in &app.debug_sessions.parked {
                ui.selectable_value(&mut selected, parked.id, format!("{} — {}", trf("Session {number}: {name}", &[("number", &parked.id), ("name", &parked.debugger.name)]), parked.debugger.state_label()));
            }
        });
        if ui.button(tr("Debug in New Session")).clicked() { app.debug_in_new_session(); }
        if ui.add_enabled(!app.debugger.active(), egui::Button::new(tr("Close Debug Session"))).clicked() {
            app.debugger.stop();
            app.debugger.session = None;
            app.debugger.console = None;
            if let Some(id) = app.debug_sessions.parked.last().map(|s| s.id) {
                app.select_debug_session(id);
                app.debug_sessions.parked.retain(|s| s.debugger.session.is_some() || s.debugger.downloading.is_some());
            }
        }
        if ui.button(tr("Stop All Debug Sessions")).clicked() { app.stop_all_debug_sessions(); }
    });
    if selected != current { app.select_debug_session(selected); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    use std::time::{Duration, Instant};
    fn wait(h: &mut Harness<'_, CrabCraftApp>, done: impl Fn(&CrabCraftApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done(h.state()) {
            h.run_steps(1);
            assert!(Instant::now() < deadline, "session did not settle: {}", h.state().debugger.state_label());
            if let Some(session) = &h.state().debugger.session { assert!(!matches!(session.state(), rc_dap::SessionState::Failed(_)), "{:?}", session.state()); }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn native_parallel_rust_sessions_switch_and_stop_independently() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join("src/bin")).unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[package]\nname='parallel-audit'\nversion='0.1.0'\nedition='2024'\n[workspace]\n").unwrap();
        let first = project.path().join("src/main.rs");
        let second = project.path().join("src/bin/second.rs");
        for (path, value, marker) in [(&first, 42, "first.done"), (&second, 84, "second.done")] {
            std::fs::write(path, format!("fn main() {{\n    let value: u32 = {value};\n    std::fs::write(\"{marker}\", value.to_string()).unwrap();\n}}\n")).unwrap();
        }
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace(project.path());
        app.trust_project_tools_for_test();
        let mut harness = crate::screens::harness(app, egui::vec2(1450.0, 900.0));
        wait(&mut harness, |a| a.runner.cargo_workspace.is_some());
        for path in [&first, &second] {
            harness.state_mut().open_file_and_wait(path);
            harness.state_mut().docs.active_doc_mut().unwrap().editor.breakpoints.insert(2, rc_editor::BreakpointKind::Normal);
        }
        harness.state_mut().runner.selected = harness.state().runner.configs.iter().position(|c| c.name == "parallel-audit").unwrap();
        harness.state_mut().debug_in_new_session();
        wait(&mut harness, |a| a.debugger.stopped());
        harness.state_mut().runner.selected = harness.state().runner.configs.iter().position(|c| c.name == "second").unwrap();
        harness.run_steps(2);
        harness.get_by_label(tr("Debug in New Session")).click(); harness.run_steps(2);
        wait(&mut harness, |a| a.debugger.stopped());
        assert_eq!(harness.state().debug_sessions.parked.len(), 1);
        assert!(harness.state().debug_sessions.parked[0].debugger.stopped());
        assert_eq!(harness.state().debugger.name, "second");
        assert!(!project.path().join("first.done").exists() && !project.path().join("second.done").exists());
        let watch = harness.state_mut().debugger.session.as_mut().unwrap().add_watch("value");
        wait(&mut harness, |a| a.debugger.session.as_ref().unwrap().watches().iter().any(|w| w.id == watch && matches!(&w.value, rc_dap::WatchValue::Value(v) if v.result == "84")));
        let parked = &harness.state().debug_sessions.parked[0];
        let option = format!("{} — {}", trf("Session {number}: {name}", &[("number", &parked.id), ("name", &parked.debugger.name)]), parked.debugger.state_label());
        harness.get_by_value(&trf("Session {number}: {name}", &[("number", &2), ("name", &"second")])).click(); harness.run_steps(2);
        harness.get_by_label(&option).click(); harness.run_steps(2);
        assert_eq!(harness.state().debugger.name, "parallel-audit");
        assert!(harness.state().debugger.session.as_ref().unwrap().watches().is_empty());
        harness.state_mut().debugger.session.as_mut().unwrap().continue_();
        wait(&mut harness, |a| a.debugger.session.as_ref().unwrap().state().is_ended());
        assert_eq!(std::fs::read_to_string(project.path().join("first.done")).unwrap(), "42");
        assert!(harness.state().debug_sessions.parked[0].debugger.stopped());
        let second_id = harness.state().debug_sessions.parked[0].id;
        harness.state_mut().select_debug_session(second_id);
        assert!(harness.state().debugger.session.as_ref().unwrap().watches().iter().any(|w| w.id == watch));
        harness.state_mut().debugger.session.as_mut().unwrap().continue_();
        wait(&mut harness, |a| a.debugger.session.as_ref().unwrap().state().is_ended());
        assert_eq!(std::fs::read_to_string(project.path().join("second.done")).unwrap(), "84");
        harness.state_mut().runner.selected = harness.state().runner.configs.iter().position(|c| c.name == "parallel-audit").unwrap();
        harness.state_mut().debug_in_new_session();
        wait(&mut harness, |a| a.debugger.stopped());
        harness.state_mut().runner.selected = harness.state().runner.configs.iter().position(|c| c.name == "second").unwrap();
        harness.state_mut().debug_in_new_session();
        wait(&mut harness, |a| a.debugger.stopped());
        harness.state_mut().debugger.stop();
        assert!(harness.state().debug_sessions.parked.iter().any(|s| s.debugger.name == "parallel-audit" && s.debugger.stopped()));
        harness.run_steps(2);
        harness.get_by_label(tr("Stop All Debug Sessions")).click(); harness.run_steps(2);
        assert!(harness.state().debug_sessions.parked.iter().all(|s| !s.debugger.active()));
        harness.get_by_label(tr("Close Debug Session")).click(); harness.run_steps(2);
        harness.state_mut().close_workspace();
        assert!(harness.state().debug_sessions.parked.is_empty());
    }
}
