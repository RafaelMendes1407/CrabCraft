//! Explicit attachment to processes belonging to one native terminal session.
use rc_core::processes::{TerminalProcess, TerminalProcessError};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::tr;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct Job {
    cancel: Arc<AtomicBool>,
    inbox: Inbox<Result<Vec<TerminalProcess>, TerminalProcessError>>,
    validating: bool,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
#[derive(Default)]
pub struct Picker {
    session: Option<(u64, u32)>,
    rows: Vec<TerminalProcess>,
    job: Option<Job>,
    filter: String,
    error: Option<String>,
}
impl Picker {
    pub fn open(&mut self, id: u64, pid: u32, waker: Waker) {
        *self = Self::default();
        self.session = Some((id, pid));
        self.start(None, waker);
    }
    fn start(&mut self, selected: Option<TerminalProcess>, waker: Waker) {
        self.job = None;
        self.error = None;
        let Some((_, pid)) = self.session else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let validating = selected.is_some();
        std::thread::spawn(move || {
            let answer = match selected {
                Some(selected) => rc_core::processes::validate_terminal_process(pid, &selected, &token).map(|()| vec![selected]),
                None => rc_core::processes::terminal_processes(pid, &token),
            };
            out.send(answer);
        });
        self.job = Some(Job { cancel, inbox, validating });
    }
    pub fn show(&mut self, ui: &mut egui::Ui, terminals: &rc_term::Terminals, waker: Waker) -> Option<TerminalProcess> {
        let (id, pid) = self.session?;
        if !terminals.active().is_some_and(|tab| tab.id() == id && tab.session.pid() == Some(pid) && tab.session.exit_status().is_none()) {
            *self = Self::default();
            return None;
        }
        if let Some(answer) = self.job.as_ref().and_then(|job| job.inbox.drain().into_iter().next()) {
            let validating = self.job.take().is_some_and(|job| job.validating);
            match answer {
                Ok(rows) if validating => {
                    *self = Self::default();
                    return rows.into_iter().next();
                }
                Ok(rows) => self.rows = rows,
                Err(TerminalProcessError::Cancelled) => (),
                Err(_) => self.error = Some(tr("Terminal processes changed or could not be read; refresh the list").to_string()),
            }
        }
        let mut close = false;
        let mut selected = None;
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(tr("Debug Terminal Process"));
                if ui.button(tr("Refresh")).clicked() {
                    self.start(None, waker.clone());
                }
                if ui.button(tr("Cancel")).clicked() {
                    close = true;
                }
                if self.job.is_some() {
                    ui.spinner();
                }
            });
            ui.label(tr("Choose a process from this terminal; attaching pauses it under the debugger"));
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(tr("Filter by name or PID")));
            if let Some(error) = &self.error {
                ui.colored_label(rc_kit::pal(ui).error, error);
            }
            let needle = self.filter.to_lowercase();
            egui::ScrollArea::vertical().id_salt("terminal_attach_processes").max_height(180.0).show(ui, |ui| {
                for process in self.rows.iter().filter(|p| p.process.name.to_lowercase().contains(&needle) || p.process.pid.to_string().contains(&needle)) {
                    let label = format!("{}  {}  {}", process.process.pid, process.process.name, process.process.command);
                    if ui.add_enabled(self.job.is_none(), egui::Button::new(label).truncate()).clicked() {
                        selected = Some(process.clone());
                    }
                }
            });
            if self.rows.is_empty() && self.job.is_none() {
                ui.label(tr("No attachable processes in this terminal"));
            }
        });
        if close {
            *self = Self::default();
        } else if let Some(selected) = selected {
            self.start(Some(selected), waker);
        }
        None
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    #[test]
    #[ignore = "requires a real PTY and GDB ptrace permissions"]
    fn native_terminal_picker_attaches_to_a_real_rust_program_and_detaches_without_killing_it() {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"terminal_attach_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n")
            .unwrap();
        let source = "unsafe extern \"C\" { fn prctl(option:i32, arg:u64, ...)->i32; }\nfn main() {\n    unsafe { prctl(0x59616d61, u64::MAX); }\n    let mut rounds=0u64;\n    while !std::path::Path::new(\"finish\").exists() {\n        rounds+=1;\n        std::thread::sleep(std::time::Duration::from_millis(10));\n    }\n    std::fs::write(\"survived\", rounds.to_string()).unwrap();\n}\n";
        let path = root.path().join("src/main.rs");
        std::fs::write(&path, source).unwrap();
        let built = std::process::Command::new("cargo").args(["build", "--offline", "--quiet"]).current_dir(root.path()).output().unwrap();
        assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace(root.path());
        app.trust_project_tools_for_test();
        app.open_file_and_wait(&path);
        app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(5, rc_editor::BreakpointKind::Normal);
        let spec = rc_term::SpawnSpec::shell(Some(root.path().to_path_buf()));
        let id = app.terminals.open(&spec, Waker::noop()).unwrap();
        let session = &app.terminals.active().unwrap().session;
        session.write(b"./target/debug/terminal_attach_fixture\n");
        app.dock.show(crate::toolwindow::ToolWindow::Terminal);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 900.0));
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while h.state().runner.cargo_workspace.is_none() {
            h.run_steps(1);
            assert!(std::time::Instant::now() < end);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        h.run_steps(2);
        h.get_by_label(tr("Debug Terminal Process")).click();
        h.run_steps(2);
        loop {
            h.run_steps(1);
            if h.state().terminal_attach.rows.iter().any(|p| p.process.exe == "terminal_attach_fixture") {
                break;
            }
            assert!(std::time::Instant::now() < end, "terminal process missing");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let selected = h.state().terminal_attach.rows.iter().find(|p| p.process.exe == "terminal_attach_fixture").unwrap().clone();
        let label = format!("{}  {}  {}", selected.process.pid, selected.process.name, selected.process.command);
        h.get_by_label(&label).click();
        h.run_steps(2);
        while !h.state().debugger.stopped() {
            h.run_steps(1);
            assert!(std::time::Instant::now() < end, "debugger did not stop");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(h.state().debugger.session.as_ref().unwrap().stop_info().is_some());
        assert!(h.state().terminals.tabs().iter().any(|t| t.id() == id));
        h.state_mut().debugger.stop();
        std::fs::write(root.path().join("finish"), "").unwrap();
        while !root.path().join("survived").exists() {
            h.run_steps(1);
            assert!(std::time::Instant::now() < end, "attached process was not detached alive");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(std::fs::read_to_string(root.path().join("survived")).unwrap().parse::<u64>().unwrap() > 0);
        assert!(h.state().terminals.active().unwrap().session.exit_status().is_none(), "shell must remain alive");
    }
}
