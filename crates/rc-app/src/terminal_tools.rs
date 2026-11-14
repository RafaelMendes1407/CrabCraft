//! Native terminal pane and search controls; sessions remain owned by `rc_term::Terminals`.
use rc_core::Waker;
use rc_kit::i18n::{tr, trf};

#[derive(Clone, Copy)]
pub struct Split {
    pub first: u64,
    pub second: u64,
    pub stacked: bool,
    pub ratio: f32,
}
impl Split {
    pub fn contains(self, id: u64) -> bool {
        id == self.first || id == self.second
    }
}

#[derive(Default)]
pub struct Tools {
    pub split: Option<Split>,
    pub split_after_start: Option<(u64, bool)>,
    pub search: SearchUi,
}

#[derive(Default)]
pub struct SearchUi {
    pub open: bool,
    pub query: String,
    target: Option<u64>,
    job: Option<rc_term::search::Search>,
    result: Option<rc_term::search::Results>,
    current: usize,
    error: Option<String>,
}
impl SearchUi {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.target = None;
        }
    }
    fn start(&mut self, session: &rc_term::TermSession, waker: Waker) {
        self.job = None;
        self.result = None;
        self.current = 0;
        self.error = None;
        self.target = Some(session.id());
        if self.query.is_empty() {
            return;
        }
        match session.search(self.query.clone(), waker) {
            Ok(job) => self.job = Some(job),
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    fn reveal(&mut self, session: &rc_term::TermSession) {
        if let Some(result) = &mut self.result {
            if let Some(hit) = result.hits.get(self.current) {
                if !session.reveal_search_match(*hit, result.generation) {
                    result.stale = true;
                }
            }
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui, session: &rc_term::TermSession, waker: Waker) {
        if !self.open {
            self.job = None;
            return;
        }
        if self.target != Some(session.id()) {
            self.start(session, waker.clone());
        }
        if let Some(result) = self.job.as_ref().and_then(rc_term::search::Search::poll) {
            self.job = None;
            self.result = Some(result);
            self.reveal(session);
        }
        if let Some(result) = &mut self.result {
            if result.generation != session.lock().generation {
                result.stale = true;
            }
        }
        ui.horizontal(|ui| {
            let label = ui.label(tr("Terminal search"));
            let input = ui
                .add(egui::TextEdit::singleline(&mut self.query).char_limit(256).hint_text(tr("Find in Terminal")).desired_width(220.0))
                .labelled_by(label.id);
            if input.changed() {
                self.start(session, waker.clone());
            }
            if self.job.is_some() {
                ui.spinner();
            }
            let available = self.result.as_ref().is_some_and(|result| !result.stale && !result.hits.is_empty());
            if ui.add_enabled(available, egui::Button::new(tr("Previous"))).clicked() {
                if let Some(result) = &self.result {
                    self.current = if self.current == 0 { result.hits.len().saturating_sub(1) } else { self.current - 1 };
                }
                self.reveal(session);
            }
            if ui.add_enabled(available, egui::Button::new(tr("Next"))).clicked() {
                if let Some(result) = &self.result {
                    self.current = (self.current + 1) % result.hits.len().max(1);
                }
                self.reveal(session);
            }
            if ui.button(tr("Refresh")).clicked() {
                self.start(session, waker);
            }
            if ui.button(tr("Close")).clicked() {
                self.open = false;
                self.job = None;
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(rc_kit::pal(ui).error, error);
        }
        if let Some(result) = &self.result {
            if result.stale {
                ui.label(tr("Terminal output changed; refresh the search"));
            } else {
                let current = if result.hits.is_empty() { 0 } else { self.current + 1 };
                ui.label(trf("{current} of {total} terminal matches", &[("current", &current), ("total", &result.hits.len())]));
            }
            if result.truncated {
                ui.label(tr("Search limited to recent output and 5,000 matches"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};

    #[cfg(unix)]
    #[test]
    fn native_split_panes_and_search_work_in_a_real_cargo_workspace() {
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"terminal_split_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n")
            .unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main() { println!(\"SPLIT_SEARCH_OK\"); }\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace_and_wait(root.path());
        app.trust_project_tools_for_test();
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::Terminal, 420.0);
        app.open_terminal(None);
        let mut harness = crate::screens::harness(app, egui::vec2(1400.0, 900.0));
        let pump = |harness: &mut egui_kittest::Harness<'_, crate::app::CrabCraftApp>, count: usize| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while harness.state().terminals.len() != count {
                harness.run_steps(1);
                assert!(Instant::now() < deadline, "terminal startup timed out");
                std::thread::sleep(Duration::from_millis(5));
            }
            harness.run_steps(3);
        };
        pump(&mut harness, 1);
        let first = harness.state().terminals.active().unwrap().id();
        harness.state_mut().terminals.rename(first, Some("Cargo pane".into()));
        let before = harness.state().terminals.get(first).unwrap().session.size();
        harness.state().terminals.get(first).unwrap().session.write(b"cargo run --offline --quiet\r".to_vec());
        assert!(harness.state().terminals.get(first).unwrap().session.wait_for_text("SPLIT_SEARCH_OK", Duration::from_secs(30)));
        harness.get_by_label(tr("Split Terminal Right")).click();
        harness.run_steps(2);
        pump(&mut harness, 2);
        let split = harness.state().terminal_tools.split.unwrap();
        assert_eq!(split.first, first);
        assert!(!split.stacked);
        assert!(harness.state().terminals.get(first).unwrap().session.size().0 < before.0);
        assert!(harness.state().terminals.get(split.second).unwrap().session.is_running());
        harness.get_by_label("Cargo pane").click();
        harness.run_steps(2);
        harness.get_by_label(tr("Find in Terminal")).click();
        harness.run_steps(3);
        harness.get_by_label(tr("Terminal search")).focus();
        harness.get_by_label(tr("Terminal search")).type_text("SPLIT_SEARCH_OK");
        harness.run_steps(2);
        let deadline = Instant::now() + Duration::from_secs(3);
        while harness.state().terminal_tools.search.result.as_ref().is_none_or(|result| result.hits.is_empty()) {
            harness.run_steps(1);
            assert!(Instant::now() < deadline, "search did not find Cargo output");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(harness.state().terminals.get(first).unwrap().session.selection_text().unwrap(), "SPLIT_SEARCH_OK");
        let right = harness.state().terminals.get(split.second).unwrap().session.size();
        harness.get_by_label(tr("Close Terminal Split")).click();
        harness.run_steps(2);
        assert!(harness.state().terminal_tools.split.is_none());
        assert_eq!(harness.state().terminals.len(), 2, "closing a split preserves both shells");
        harness.state_mut().terminal_tools.search.open = false;
        harness.run_steps(2);
        harness.get_by_label(tr("Split Terminal Down")).click();
        harness.run_steps(2);
        pump(&mut harness, 3);
        let split = harness.state().terminal_tools.split.unwrap();
        assert!(split.stacked);
        assert!(harness.state().terminals.get(split.second).unwrap().session.size().1 < before.1);
        assert!(harness.state().terminals.get(split.second).unwrap().session.size().0 > right.0);
    }
}
