//! A bounded, cancellable snapshot of the stopped threads. Identical whole stacks are
//! grouped by function/source/line (never by adapter frame IDs, which differ by thread).

use rc_dap::{DebugEvent, DebugSession, ParallelStack};
use rc_kit::i18n::{tr, trf};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

const MAX_THREADS: usize = 64;
const MAX_FRAMES: usize = 64;
const CONCURRENT: usize = 4;
const TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug)]
enum StackState {
    Waiting,
    Loading,
    Ready(ParallelStack),
    Error(String),
}

#[derive(Clone, Debug)]
struct ThreadStack {
    id: i64,
    name: String,
    state: StackState,
}

#[derive(Default)]
pub struct ParallelStacks {
    generation: Option<u64>,
    started: bool,
    rows: Vec<ThreadStack>,
    groups: Vec<Vec<usize>>,
    queue: VecDeque<usize>,
    pending: BTreeMap<u64, (usize, Instant)>,
    pub cancelled: bool,
    truncated_threads: bool,
    selected_thread: Option<i64>,
    waiting_since: Option<Instant>,
    list_error: Option<String>,
}

impl ParallelStacks {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn loading(&self) -> bool {
        !self.cancelled && self.list_error.is_none() && (!self.started || !self.pending.is_empty() || !self.queue.is_empty())
    }

    pub fn cancel(&mut self, session: &mut DebugSession) {
        for id in self.pending.keys() {
            session.cancel_parallel_stack(*id);
        }
        self.pending.clear();
        self.queue.clear();
        for row in &mut self.rows {
            if matches!(row.state, StackState::Waiting | StackState::Loading) {
                row.state = StackState::Error(tr("Cancelled").into());
            }
        }
        self.cancelled = true;
    }

    pub fn refresh(&mut self, session: &mut DebugSession) {
        self.cancel(session);
        self.reset();
        session.refresh_threads();
    }

    /// Called every debugger frame, including when another tab is selected, so closing the
    /// tab or resuming cancels requests and never leaves a stale snapshot visible.
    pub fn sync(&mut self, session: &mut DebugSession, visible: bool) {
        if !visible || !session.state().is_stopped() {
            self.cancel(session);
            self.reset();
            return;
        }
        if self.generation != Some(session.generation()) {
            self.cancel(session);
            self.reset();
            self.generation = Some(session.generation());
            self.waiting_since = Some(Instant::now());
        }
        if self.cancelled {
            return;
        }
        if !session.threads_current() {
            if self.waiting_since.is_some_and(|start| start.elapsed() > TIMEOUT) {
                self.list_error = Some(tr("The stopped thread list is unavailable. Refresh to retry.").into());
            }
            return;
        }
        if !self.started {
            let stopped = session.stop_info().map(|stop| stop.thread_id);
            let threads: Vec<_> =
                session.threads().iter().filter(|thread| session.all_threads_stopped() || Some(thread.id) == stopped).take(MAX_THREADS + 1).collect();
            self.truncated_threads = threads.len() > MAX_THREADS;
            self.rows = threads
                .into_iter()
                .take(MAX_THREADS)
                .map(|thread| ThreadStack { id: thread.id, name: thread.name.chars().take(256).collect(), state: StackState::Waiting })
                .collect();
            self.queue.extend(0..self.rows.len());
            self.started = true;
            self.waiting_since = None;
        }
        let expired: Vec<_> = self.pending.iter().filter_map(|(id, (_, start))| (start.elapsed() > TIMEOUT).then_some(*id)).collect();
        for id in expired {
            session.cancel_parallel_stack(id);
            if let Some((index, _)) = self.pending.remove(&id) {
                self.rows[index].state = StackState::Error(tr("Stack request timed out").into());
            }
        }
        while self.pending.len() < CONCURRENT {
            let Some(index) = self.queue.pop_front() else { break };
            match session.request_parallel_stack(self.rows[index].id, MAX_FRAMES) {
                Some(id) => {
                    self.rows[index].state = StackState::Loading;
                    self.pending.insert(id, (index, Instant::now()));
                }
                None => {
                    self.rows[index].state = StackState::Error(tr("This thread is no longer stopped or available").into());
                }
            }
        }
    }

    pub fn on_event(&mut self, event: &DebugEvent) {
        let DebugEvent::ParallelStackLoaded { id, generation, thread_id, result } = event else { return };
        if self.generation != Some(*generation) {
            return;
        }
        let Some((index, _)) = self.pending.remove(id) else { return };
        let Some(row) = self.rows.get_mut(index).filter(|row| row.id == *thread_id) else { return };
        row.state = match result {
            Ok(stack) => StackState::Ready(stack.clone()),
            Err(error) => StackState::Error(error.clone()),
        };
        // Group a stack only once when its response arrives, not on every painted frame.
        if let StackState::Ready(stack) = &self.rows[index].state {
            let same = self.groups.iter().position(|group| match &self.rows[group[0]].state {
                StackState::Ready(other) => same_stack(stack, other),
                _ => false,
            });
            match same {
                Some(group) => self.groups[group].push(index),
                None => self.groups.push(vec![index]),
            }
        }
    }

    fn groups(&self) -> Vec<Vec<usize>> {
        self.groups.clone()
    }
}

fn same_stack(left: &ParallelStack, right: &ParallelStack) -> bool {
    left.truncated == right.truncated
        && left.frames.len() == right.frames.len()
        && left.frames.iter().zip(&right.frames).all(|(a, b)| a.name == b.name && a.path() == b.path() && a.line == b.line)
}

pub fn show(ui: &mut egui::Ui, model: &mut ParallelStacks, session: &mut DebugSession) {
    if !session.state().is_stopped() {
        ui.label(tr("Pause the program to compare thread stacks"));
        return;
    }
    let loading = model.loading();
    ui.horizontal(|ui| {
        if ui.button(tr("Refresh")).clicked() {
            model.refresh(session);
        }
        if ui.add_enabled(loading, egui::Button::new(tr("Cancel"))).clicked() {
            model.cancel(session);
        }
        if loading {
            ui.spinner();
        }
        let loaded = model.rows.iter().filter(|row| matches!(row.state, StackState::Ready(_))).count();
        ui.label(trf("{loaded} of {total} thread stacks loaded", &[("loaded", &loaded), ("total", &model.rows.len())]));
    });
    if !session.all_threads_stopped() {
        ui.label(tr("Other threads may be running; only the stopped thread is shown"));
    }
    if model.truncated_threads {
        ui.label(tr("Only the first 64 stopped threads are shown"));
    }
    if model.cancelled {
        ui.label(tr("Stack loading cancelled. Refresh to load again."));
    }
    if let Some(error) = &model.list_error {
        ui.colored_label(rc_kit::pal(ui).error, error);
    }
    let selected_frame = session.selected_frame().map(|frame| frame.id);
    let mut select = None;
    egui::ScrollArea::both().id_salt("parallel_stacks").auto_shrink([false, false]).show(ui, |ui| {
        for group in model.groups() {
            let chosen = group.iter().copied().find(|index| Some(model.rows[*index].id) == model.selected_thread).unwrap_or(group[0]);
            let row = &model.rows[chosen];
            let StackState::Ready(stack) = &row.state else { continue };
            let head = stack.frames.first().map_or(tr("No stack frames"), |frame| frame.name.as_str());
            let title = trf("{count} threads: {function}", &[("count", &group.len()), ("function", &head)]);
            egui::CollapsingHeader::new(title).id_salt((model.generation, model.rows[group[0]].id)).default_open(true).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for index in &group {
                        let thread = &model.rows[*index];
                        if ui.selectable_label(thread.id == row.id, format!("{} ({})", thread.name, thread.id)).clicked() {
                            model.selected_thread = Some(thread.id);
                            if let StackState::Ready(stack) = &thread.state
                                && let Some(frame) = stack.frames.first()
                            {
                                select = Some(frame.id);
                            }
                        }
                    }
                });
                for frame in &stack.frames {
                    let response = ui.selectable_label(selected_frame == Some(frame.id), &frame.name);
                    let response = if let Some(path) = frame.path() { response.on_hover_text(format!("{}:{}", path.display(), frame.line)) } else { response };
                    if response.clicked() {
                        select = Some(frame.id);
                        model.selected_thread = Some(row.id);
                    }
                }
                if stack.truncated {
                    ui.label(tr("Only the first 64 frames are shown"));
                }
            });
        }
        for row in &model.rows {
            if let StackState::Error(error) = &row.state {
                ui.colored_label(rc_kit::pal(ui).error, format!("{} ({}): {error}", row.name, row.id));
            }
        }
    });
    if let Some(frame) = select {
        session.select_frame(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_dap::StackFrame;
    fn stack(id: i64, line: u32) -> ParallelStack {
        ParallelStack { frames: vec![StackFrame { id, name: "worker".into(), line, ..Default::default() }], truncated: false }
    }
    #[test]
    fn grouping_uses_locations_and_functions_instead_of_adapter_frame_ids() {
        let mut model = ParallelStacks::default();
        model.generation = Some(1);
        for (id, line) in [(1, 10), (2, 10), (3, 20)] {
            let index = model.rows.len();
            model.rows.push(ThreadStack { id, name: format!("worker{id}"), state: StackState::Loading });
            model.pending.insert(id as u64, (index, Instant::now()));
            model.on_event(&DebugEvent::ParallelStackLoaded { id: id as u64, generation: 1, thread_id: id, result: Ok(stack(id, line)) });
        }
        assert_eq!(model.groups(), vec![vec![0, 1], vec![2]]);
        assert!(same_stack(&stack(1, 10), &stack(22, 10)));
    }
    #[test]
    fn late_or_unknown_replies_do_not_replace_the_snapshot() {
        let mut model = ParallelStacks::default();
        model.generation = Some(5);
        model.rows.push(ThreadStack { id: 1, name: "worker".into(), state: StackState::Loading });
        model.pending.insert(10, (0, Instant::now()));
        model.on_event(&DebugEvent::ParallelStackLoaded { id: 10, generation: 4, thread_id: 1, result: Ok(stack(1, 10)) });
        assert!(matches!(model.rows[0].state, StackState::Loading));
        assert_eq!(model.pending.len(), 1);
        model.on_event(&DebugEvent::ParallelStackLoaded { id: 10, generation: 5, thread_id: 1, result: Ok(stack(1, 10)) });
        assert!(matches!(model.rows[0].state, StackState::Ready(_)));
        assert!(model.pending.is_empty());
    }
}

#[cfg(test)]
mod real_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    use rc_core::Waker;
    use rc_dap::{BreakpointSpec, Breakpoints, DebugOptions, DebugSession, DebugTarget};

    const SOURCE: &str = r#"use std::sync::{Arc, Barrier};
#[inline(never)]
fn worker_ready(barrier: Arc<Barrier>) {
    barrier.wait();
    std::thread::park();
}
#[inline(never)]
fn checkpoint() {
    std::hint::black_box(7);
}
fn main() {
    let barrier = Arc::new(Barrier::new(3));
    let a = barrier.clone();
    let b = barrier.clone();
    let alpha = std::thread::Builder::new().name("alpha".into()).spawn(move || worker_ready(a)).unwrap();
    let beta = std::thread::Builder::new().name("beta".into()).spawn(move || worker_ready(b)).unwrap();
    barrier.wait();
    checkpoint();
    alpha.thread().unpark();
    beta.thread().unpark();
    alpha.join().unwrap();
    beta.join().unwrap();
}
"#;

    struct State {
        session: DebugSession,
        model: ParallelStacks,
        events: Vec<DebugEvent>,
    }
    impl State {
        fn pump(&mut self) {
            for event in self.session.pump() {
                if let DebugEvent::Failed(failure) = &event {
                    panic!("{failure:?}");
                }
                self.model.on_event(&event);
                self.events.push(event);
            }
            self.model.sync(&mut self.session, true);
        }
        fn until(&mut self, mut condition: impl FnMut(&Self) -> bool) {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(60) {
                self.pump();
                if condition(self) {
                    return;
                }
                assert!(!self.session.state().is_ended(), "ended before expected state: {:?}", self.events);
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("timeout: {:?}, rows {:?}", self.session.state(), self.model.rows);
        }
    }

    #[test]
    #[ignore = "requires real GDB with ptrace permission"]
    fn real_rust_threads_load_cancel_refresh_and_select_in_the_native_view() {
        let _guard = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let adapter = rc_dap::gdb::locate().expect("GDB is required for this integration test");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"parallel_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        let main = root.join("src/main.rs");
        std::fs::write(&main, SOURCE).unwrap();
        let mut breakpoints = Breakpoints::default();
        let line = SOURCE.lines().position(|line| line.trim() == "checkpoint();").unwrap() as u32 + 1;
        breakpoints.set(BreakpointSpec::new(main.clone(), line));
        let target = rc_cargo::RunTarget {
            before: Vec::new(),
            test: None,
            manifest: root.join("Cargo.toml"),
            package: "parallel_fixture".into(),
            bin: "parallel_fixture".into(),
            args: Vec::new(),
            env: Vec::new(),
            working_dir: None,
            options: Default::default(),
        };
        let session = DebugSession::start(
            adapter,
            DebugTarget::Cargo { cargo: rc_cargo::Cargo::locate().unwrap(), target },
            DebugOptions::default(),
            breakpoints,
            Waker::noop(),
        );
        let mut state = State { session, model: ParallelStacks::default(), events: Vec::new() };
        state.until(|state| state.session.state().is_stopped() && state.model.started && !state.model.loading());
        assert!(state.session.all_threads_stopped());
        assert!(state.model.rows.len() >= 3, "{:?}", state.model.rows);
        for name in ["alpha", "beta"] {
            let row = state.model.rows.iter().find(|row| row.name.contains(name)).expect("named worker thread");
            let StackState::Ready(stack) = &row.state else { panic!("{row:?}") };
            assert!(stack.frames.iter().any(|frame| frame.name.contains("worker_ready")), "{stack:?}");
            assert!(stack.frames.len() <= MAX_FRAMES);
        }
        // The client bounds outstanding work and explicitly forgets cancelled replies.
        let thread = state.model.rows[0].id;
        let ids: Vec<_> = (0..4).map(|_| state.session.request_parallel_stack(thread, usize::MAX).unwrap()).collect();
        assert!(state.session.request_parallel_stack(thread, 64).is_none());
        for id in &ids {
            state.session.cancel_parallel_stack(*id);
        }
        for _ in 0..20 {
            for event in state.session.pump() {
                assert!(!matches!(event, DebugEvent::ParallelStackLoaded { id, .. } if ids.contains(&id)));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // Cancel/refresh use native controls; clicking a frame selects its source context.
        let mut harness = Harness::builder().with_size(egui::vec2(1300.0, 800.0)).build_ui_state(
            |ui, state: &mut State| {
                state.pump();
                show(ui, &mut state.model, &mut state.session);
            },
            state,
        );
        harness.run_steps(3);
        harness.get_by_label("Refresh").click();
        harness.run_steps(1);
        harness.get_by_label("Cancel").click();
        harness.run_steps(2);
        assert!(harness.state().model.cancelled);
        assert!(harness.state().model.pending.is_empty());
        harness.get_by_label("Refresh").click();
        for _ in 0..200 {
            harness.run_steps(1);
            if harness.state().model.started && !harness.state().model.loading() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let source_frame = harness
            .state()
            .model
            .rows
            .iter()
            .find_map(|row| match &row.state {
                StackState::Ready(stack) => stack.frames.iter().find(|frame| frame.name.contains("worker_ready")).cloned(),
                _ => None,
            })
            .unwrap();
        harness.get_all_by_label(&source_frame.name).next().unwrap().click();
        harness.run_steps(2);
        assert_eq!(harness.state().session.selected_frame().map(|frame| frame.id), Some(source_frame.id));
        assert_eq!(harness.state().session.execution_point().map(|(path, _)| path), Some(main));
        harness.state_mut().session.continue_();
        harness.run_steps(2);
        assert!(harness.state().model.rows.is_empty(), "resume discards the previous stop");
        harness.state_mut().session.stop();
    }
}
