//! Explicit native connection dialog; constructing it performs no network or process work.
use crate::app::CrabCraftApp;
use rc_kit::i18n::tr;

pub struct Dialog {
    pub host: String,
    pub port: String,
    pub program: String,
}
impl Default for Dialog {
    fn default() -> Self {
        Self { host: "127.0.0.1".into(), port: "2345".into(), program: String::new() }
    }
}

pub fn show(app: &mut CrabCraftApp, ctx: &egui::Context, draft: &mut Dialog) -> bool {
    let mut connect = false;
    let mut cancel = false;
    let valid = draft.port.parse::<u16>().ok().and_then(|port| rc_dap::remote::RemoteTarget::new(&draft.host, port, draft.program.clone().into()).ok());
    egui::Modal::new(egui::Id::unique("remote-gdb-connection")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading(tr("Connect to GDB Server"));
        ui.label(tr(
            "Connect to an existing gdbserver using a local binary with matching debug symbols. Stop detaches; it does not terminate the remote process.",
        ));
        egui::Grid::new("remote-gdb-fields").show(ui, |ui| {
            ui.label(tr("Host"));
            ui.add(egui::TextEdit::singleline(&mut draft.host).char_limit(253));
            ui.end_row();
            ui.label(tr("Port"));
            ui.add(egui::TextEdit::singleline(&mut draft.port).char_limit(5));
            ui.end_row();
            ui.label(tr("Local symbol binary"));
            ui.add(egui::TextEdit::singleline(&mut draft.program).desired_width(350.0).char_limit(8192));
            ui.end_row();
        });
        if valid.is_none() {
            ui.label(tr("Enter a valid host, TCP port and absolute local binary path."));
        }
        ui.horizontal(|ui| {
            connect = ui.add_enabled(valid.is_some(), egui::Button::new(tr("Connect"))).clicked();
            cancel = ui.button(tr("Cancel")).clicked();
        });
    });
    if connect && let Some(target) = valid {
        if let Err(error) = app.runner.require_trust() {
            app.notify(error.to_string(), true);
        } else {
            let outputs = app.debugger.start_remote(target, &app.docs);
            app.handle_debug(outputs);
        }
        return true;
    }
    cancel || ctx.input(|input| input.key_pressed(egui::Key::Escape))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    #[test]
    fn cancelling_or_connecting_without_trust_starts_no_remote_session() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname='remote-ui'\nversion='0.1.0'\nedition='2024'\n[workspace]\n").unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(root.path());
        app.dialog = Some(crate::ui::dialogs::Dialog::RemoteDebug(Dialog {
            host: "127.0.0.1".into(),
            port: "2345".into(),
            program: root.path().join("symbols").display().to_string(),
        }));
        let mut harness = crate::screens::harness(app, egui::vec2(1200.0, 800.0));
        harness.get_by_label(tr("Cancel")).click();
        harness.run_steps(2);
        assert!(harness.state().dialog.is_none());
        assert!(harness.state().debugger.session.is_none());
        harness.state_mut().dialog = Some(crate::ui::dialogs::Dialog::RemoteDebug(Dialog {
            host: "127.0.0.1".into(),
            port: "2345".into(),
            program: root.path().join("symbols").display().to_string(),
        }));
        harness.run_steps(2);
        harness.get_by_label(tr("Connect")).click();
        harness.run_steps(2);
        assert!(harness.state().dialog.is_none());
        assert!(harness.state().debugger.session.is_none());
        assert!(harness.state().runner.cargo_workspace.is_none());
        assert!(!root.path().join("target").exists());
    }
}
