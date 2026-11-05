//! Explicit bounded :vars snapshots for managed evcxr tabs. Never evaluates values.
use rc_kit::i18n::tr;
use rc_term::TermSession;
use std::{collections::BTreeMap, time::{Duration, Instant}};

const MAX_VARIABLES: usize = 512;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable { pub name: String, pub ty: String }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error { Busy, Gap, InvalidReply, TooLarge, Timeout, Closed, Changed }
impl Error {
    fn message(self) -> &'static str {
        match self {
            Self::Busy => tr("Refresh variables at an empty Rust REPL prompt"),
            Self::Gap => tr("REPL output was discarded; refresh the variables again"),
            Self::InvalidReply => tr("The REPL variable reply was not recognized"),
            Self::TooLarge => tr("The REPL variable reply exceeds the display limit"),
            Self::Timeout => tr("The REPL variable request timed out"),
            Self::Closed => tr("The Rust REPL has exited"),
            Self::Changed => tr("REPL input changed; refresh the variables again"),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.message()) }
}
impl std::error::Error for Error {}

struct Pending { cursor: u64, epoch: u64, started: Instant }
#[derive(Default)]
pub struct Variables {
    pub rows: Vec<Variable>, pub error: Option<Error>,
    pending: Option<Pending>, epoch: u64, filter: String, observed: Option<(u64, Instant)>,
}

// Only SGR color sequences are accepted; other terminal controls cannot forge table rows.
fn plain(bytes: &[u8]) -> Result<String, Error> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x1b => {
                i += 1;
                if bytes.get(i) != Some(&b'[') { return Err(Error::InvalidReply); }
                i += 1;
                let start = i;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) { i += 1; }
                if i == bytes.len() || i - start > 32 || bytes[i] != b'm' { return Err(Error::InvalidReply); }
            }
            b'\r' => {},
            b'\n' | b'\t' => out.push(bytes[i]),
            0..=31 | 127 => return Err(Error::InvalidReply),
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8(out).map_err(|_| Error::InvalidReply)
}
fn parse_reply(bytes: &[u8]) -> Result<Option<Vec<Variable>>, Error> {
    if !bytes.ends_with(b">> ") && !bytes.ends_with(b">> \x1b[0m") { return Ok(None); }
    let text = plain(bytes)?;
    let Some(body) = text.strip_suffix(">> ") else { return Ok(None); };
    // Direct-readline PTY mode echoes the command. Never parse previous screen rows.
    let Some(body) = body.strip_prefix(":vars\n") else { return Err(Error::InvalidReply); };
    let mut rows = Vec::new();
    for line in body.lines().filter(|line| !line.is_empty()) {
        if rows.len() >= MAX_VARIABLES { return Err(Error::TooLarge); }
        let Some((name, ty)) = line.split_once(": ") else { return Err(Error::InvalidReply); };
        if name.is_empty() || ty.is_empty() || name.len() > 256 || ty.len() > 4096 { return Err(Error::TooLarge); }
        rows.push(Variable { name: name.into(), ty: ty.into() });
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    if rows.windows(2).any(|pair| pair[0].name == pair[1].name) { return Err(Error::InvalidReply); }
    Ok(Some(rows))
}
impl Variables {
    pub fn pending(&self) -> bool { self.pending.is_some() }
    pub fn cancel(&mut self) { self.pending = None; self.rows.clear(); self.error = None; }
    pub fn poll(&mut self, session: &TermSession) {
        if session.exit_status().is_some() { self.rows.clear(); self.pending = None; self.error = Some(Error::Closed); return; }
        let cursor = session.output_cursor().unwrap_or(0);
        if self.observed.is_none_or(|(previous, _)| previous != cursor) { self.observed = Some((cursor, Instant::now())); }
        if self.epoch != session.input_epoch() && self.pending.is_none() {
            self.rows.clear(); self.error = None; self.epoch = session.input_epoch();
        }
        let Some(pending) = &self.pending else { return; };
        let result = if session.exit_status().is_some() { Err(Error::Closed) }
            else if session.input_epoch() != pending.epoch { Err(Error::Changed) }
            else if pending.started.elapsed() > Duration::from_secs(5) { Err(Error::Timeout) }
            else { session.output_since(pending.cursor).map_err(|_| Error::Gap).and_then(|bytes| parse_reply(&bytes)) };
        match result {
            Ok(Some(rows)) => { self.rows = rows; self.pending = None; self.error = None; },
            Ok(None) => {},
            Err(error) => { self.pending = None; self.rows.clear(); self.error = Some(error); },
        }
    }
    pub fn ready(&self, session: &TermSession) -> bool {
        self.pending.is_none() && session.exit_status().is_none()
            && self.observed.is_some_and(|(cursor, since)| Some(cursor) == session.output_cursor() && since.elapsed() >= Duration::from_millis(100)
                && (session.input_epoch() == 0 || cursor > session.input_output_cursor()))
            && session.output_tail().and_then(|bytes| plain(bytes.rsplit(|byte| *byte == b'\n').next().unwrap_or_default()).ok()).is_some_and(|text| text.ends_with(">> "))
    }
    pub fn refresh(&mut self, session: &TermSession) -> Result<(), Error> {
        if !self.ready(session) { return Err(Error::Busy); }
        let cursor = session.output_cursor().ok_or(Error::Gap)?;
        self.rows.clear(); self.error = None;
        session.write(b":vars\r".to_vec()); self.epoch = session.input_epoch();
        self.pending = Some(Pending { cursor, epoch: self.epoch, started: Instant::now() });
        Ok(())
    }
    fn show(&mut self, session: &TermSession, ui: &mut egui::Ui) -> bool {
        self.poll(session);
        ui.horizontal(|ui| {
            ui.strong(tr("REPL Variables"));
            if self.pending() {
                ui.spinner();
                if ui.button(tr("Cancel variable refresh")).clicked() { self.cancel(); }
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            } else if ui.add_enabled(self.ready(session), egui::Button::new(tr("Refresh variables"))).clicked()
                && let Err(error) = self.refresh(session) { self.error = Some(error); }
        });
        ui.small(tr("Names and types from :vars; values are not evaluated"));
        if let Some(error) = self.error { ui.colored_label(ui.visuals().error_fg_color, error.message()); }
        if !self.pending() && self.rows.is_empty() { ui.small(tr("No variable snapshot. Refresh at an empty REPL prompt.")); }
        if !self.rows.is_empty() {
            let filter_response = ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(tr("Filter REPL variables")));
            filter_response.widget_info(|| {
                let mut info = egui::WidgetInfo::text_edit(ui.is_enabled(), &self.filter, &self.filter, tr("Filter REPL variables"));
                info.label = Some(tr("Filter REPL variables").into());
                info
            });
            let filter = self.filter.to_lowercase();
            egui::ScrollArea::vertical().id_salt(("repl_variables", session.id())).max_height(160.0).show(ui, |ui| {
                egui::Grid::new(("repl_variable_rows", session.id())).striped(true).show(ui, |ui| {
                    ui.strong(tr("Name")); ui.strong(tr("Type")); ui.end_row();
                    for row in self.rows.iter().filter(|row| row.name.to_lowercase().contains(&filter) || row.ty.to_lowercase().contains(&filter)) {
                        ui.monospace(&row.name); ui.monospace(&row.ty); ui.end_row();
                    }
                });
            });
        }
        ui.separator(); ui.ctx().request_repaint_after(Duration::from_millis(100));
        self.pending()
    }
}
#[derive(Default)]
pub struct State { sessions: BTreeMap<u64, Variables> }
impl State {
    pub fn register(&mut self, id: u64) { self.sessions.insert(id, Default::default()); }
    pub fn retain(&mut self, terminals: &rc_term::Terminals) { self.sessions.retain(|id, _| terminals.get(*id).is_some()); }
    pub fn show(&mut self, session: &TermSession, ui: &mut egui::Ui) -> bool {
        self.sessions.get_mut(&session.id()).is_some_and(|variables| variables.show(session, ui))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reply_is_bounded_exact_and_never_evaluates_values() {
        assert_eq!(parse_reply(b":vars\r\nx: i32\r\ny: String\r\n\x1b[33m>> \x1b[0m").unwrap().unwrap().len(), 2);
        assert_eq!(parse_reply(b":vars\n>> ").unwrap(), Some(Vec::new()));
        assert_eq!(parse_reply(b":vars\nx: i32\n").unwrap(), None);
        assert_eq!(parse_reply(b"old: i32\n>> "), Err(Error::InvalidReply));
        assert_eq!(parse_reply(b":vars\nx: i32\nx: i64\n>> "), Err(Error::InvalidReply));
        assert_eq!(parse_reply(format!(":vars\n{}>> ", "x: i32\n".repeat(MAX_VARIABLES + 1)).as_bytes()), Err(Error::TooLarge));
    }
    #[test]
    fn stale_input_gaps_timeout_and_cancel_never_keep_variable_rows() {
        let session = TermSession::output_only(80, 24, rc_core::Waker::noop());
        let mut vars = Variables::default();
        assert_eq!(vars.refresh(&session), Err(Error::Busy));
        let row = || Variable { name: "old".into(), ty: "i32".into() };
        vars.rows.push(row());
        vars.pending = Some(Pending { cursor: 0, epoch: 0, started: Instant::now() - Duration::from_secs(6) });
        vars.poll(&session);
        assert_eq!(vars.error, Some(Error::Timeout)); assert!(vars.rows.is_empty());
        vars.rows.push(row());
        vars.pending = Some(Pending { cursor: 0, epoch: 0, started: Instant::now() });
        vars.poll(&session);
        assert_eq!(vars.error, Some(Error::Gap)); assert!(vars.rows.is_empty());
        vars.rows.push(row());
        vars.pending = Some(Pending { cursor: 0, epoch: 0, started: Instant::now() });
        session.write(b"another command\r".to_vec()); vars.poll(&session);
        assert_eq!(vars.error, Some(Error::Changed)); assert!(vars.rows.is_empty());
        vars.rows.push(row()); vars.cancel();
        assert!(vars.rows.is_empty()); assert!(vars.error.is_none()); assert!(!vars.pending());
        assert_eq!(parse_reply(b":vars\nx: i32\n\x1b[2J>> "), Err(Error::InvalidReply));
        assert_eq!(parse_reply(b":vars\nx: i32\n\x1b[3").unwrap(), None);
    }

    #[test]
    #[ignore = "Requires evcxr and rust-src; native panel driven by a real Cargo dependency"]
    fn native_variables_follow_shadow_moves_clear_and_cancel_in_real_evcxr() {
        use egui_kittest::kittest::Queryable;
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"repl_vars_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(root.path().join("src/lib.rs"), "pub fn answer() -> i32 { 42 }\n").unwrap();
        let mut spec = rc_term::SpawnSpec::rust_repl(Some(root.path().into())).size(140, 40)
            .env("CARGO_NET_OFFLINE", "true").env("XDG_CONFIG_HOME", root.path().join("config").to_string_lossy());
        if let rc_term::Program::Command { program, .. } = &mut spec.program {
            *program = std::env::var("CRABCRAFT_TEST_EVCXR").unwrap_or_else(|_| "evcxr".into());
        }
        let session = TermSession::spawn(&spec, rc_core::Waker::noop()).unwrap();
        assert!(session.wait_for_text(">>", Duration::from_secs(30)), "{:?}", session.screen_lines());
        let path = serde_json::to_string(&root.path().to_string_lossy()).unwrap();
        session.write(format!(":dep repl_vars_fixture = {{ path = {path} }}\rlet answer = repl_vars_fixture::answer();\rlet greeting = String::from(\"hello\");\rprintln!(\"VARS_READY\");\r"));
        assert!(session.wait_until(Duration::from_secs(40), |state| (0..40).any(|row| state.row_text(row).ends_with("VARS_READY"))), "{:?}", session.screen_lines());
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace(root.path());
        app.trust_project_tools_for_test();
        let id = app.terminals.push(session);
        app.repl_variables.register(id);
        app.dock.show(crate::toolwindow::ToolWindow::Terminal);
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::Terminal, 500.0);
        let mut h = crate::screens::harness(app, egui::vec2(1400.0, 950.0));
        fn until(h: &mut egui_kittest::Harness<'_, crate::app::CrabCraftApp>, id: u64, done: impl Fn(&Variables, &TermSession) -> bool) {
            let end = Instant::now() + Duration::from_secs(40);
            loop {
                h.run_steps(1);
                let app = h.state();
                let vars = app.repl_variables.sessions.get(&id).unwrap();
                let session = &app.terminals.get(id).unwrap().session;
                if done(vars, session) { break; }
                assert!(Instant::now() < end, "variables {:?}, error {:?}, output {:?}", vars.rows, vars.error, session.output_tail().map(|bytes| String::from_utf8_lossy(&bytes).into_owned()));
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        until(&mut h, id, |vars, session| vars.ready(session));
        h.get_by_label(tr("Refresh variables")).click();
        until(&mut h, id, |vars, _| !vars.rows.is_empty());
        let rows = &h.state().repl_variables.sessions[&id].rows;
        assert!(rows.iter().any(|row| row.name == "answer" && row.ty == "i32"), "{rows:?}");
        assert!(rows.iter().any(|row| row.name == "greeting" && row.ty.contains("String")), "{rows:?}");
        h.get_by_label("answer"); h.get_by_label("greeting");
        h.get_by_label(tr("Filter REPL variables")).click(); h.run_steps(2);
        h.get_by_label(tr("Filter REPL variables")).type_text("greeting");
        h.run_steps(2);
        assert!(h.query_by_label("answer").is_none(), "filter={:?}", h.state().repl_variables.sessions[&id].filter);
        h.get_by_label("greeting");
        h.state_mut().repl_variables.sessions.get_mut(&id).unwrap().filter.clear();
        h.run_steps(2);
        // Partial user input must not be completed or overwritten by an inspector command.
        h.state().terminals.get(id).unwrap().session.write(b"let unfinished".to_vec());
        h.run_steps(4); std::thread::sleep(Duration::from_millis(150)); h.run_steps(2);
        assert!(!h.state().repl_variables.sessions[&id].ready(&h.state().terminals.get(id).unwrap().session));
        assert!(h.state().repl_variables.sessions[&id].rows.is_empty());
        h.state().terminals.get(id).unwrap().session.write(b" = 7;\rlet answer = \"forty two\";\rlet moved = greeting;\rprintln!(\"MOVED_READY\");\r".to_vec());
        until(&mut h, id, |vars, session| vars.ready(session) && session.screen_lines().iter().any(|line| line.trim_end().ends_with("MOVED_READY")));
        h.get_by_label(tr("Refresh variables")).click();
        until(&mut h, id, |vars, _| vars.rows.iter().any(|row| row.name == "moved"));
        let rows = &h.state().repl_variables.sessions[&id].rows;
        assert!(rows.iter().any(|row| row.name == "answer" && row.ty.contains("str")), "{rows:?}");
        assert!(!rows.iter().any(|row| row.name == "greeting"), "{rows:?}");
        until(&mut h, id, |vars, session| vars.ready(session));
        {
            let app = h.state_mut();
            let session = &app.terminals.get(id).unwrap().session;
            let vars = app.repl_variables.sessions.get_mut(&id).unwrap();
            vars.refresh(session).unwrap(); vars.cancel();
        }
        std::thread::sleep(Duration::from_millis(150)); h.run_steps(3);
        assert!(h.state().repl_variables.sessions[&id].rows.is_empty(), "cancelled snapshots never repopulate");
        h.state().terminals.get(id).unwrap().session.write(b":clear\r".to_vec());
        until(&mut h, id, |vars, session| vars.ready(session));
        h.get_by_label(tr("Refresh variables")).click();
        until(&mut h, id, |vars, _| !vars.pending());
        assert!(h.state().repl_variables.sessions[&id].rows.is_empty());
        assert!(h.state().repl_variables.sessions[&id].error.is_none());
        h.state().terminals.get(id).unwrap().session.write(b":quit\r".to_vec());
        until(&mut h, id, |_, session| session.exit_status().is_some());
        h.state_mut().terminals.close(id);
        h.run_steps(2);
        assert!(!h.state().repl_variables.sessions.contains_key(&id));
    }

}
