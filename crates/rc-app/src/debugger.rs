//! The debugger in the IDE: starting a session for the selected run configuration (with
//! netcoredbg, downloaded on first use), breakpoints kept in sync with the editors, the
//! execution point, the debug console, watches, hover values and expression evaluation.

use crate::docs::Docs;
use crate::runner::{ConfigKind, RunConfig};
use rc_core::{Inbox, Waker};
use rc_dap::{BreakpointSpec, Breakpoints, DebugEvent, DebugOptions, DebugSession, DebugTarget, DownloadEvent, EvalContext, EvalValue, OutputCategory, SessionState};
use rc_kit::i18n::{tr, trf};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What the debugger asks the rest of the UI to do.
pub enum DebugOutput {
    Notify(String, bool),
    /// Show the execution point: open the file at this 0-based line.
    ShowLine(PathBuf, usize),
    ShowDebugWindow,
    /// A breakpoint removed itself: the session must save.
    BreakpointsChanged,
}

/// A breakpoint as the session should have it: line, condition, log message, enabled.
type WantedBreakpoint = (u32, Option<String>, Option<String>, bool);

/// An Evaluate request: id, expression and its result once it arrives.
pub type Evaluation = (u64, String, Option<Result<EvalValue, String>>);

/// What Set Value changes: a variable (a child of `parent`, its scope or parent variable) or
/// a watch (by its expression).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetTarget {
    Variable { parent: i64, name: String, expression: Option<String> },
    Watch { expression: String },
}

impl SetTarget {
    /// Whether this adapter can change the value (otherwise Set Value is shown unavailable).
    pub fn supported(&self, capabilities: &rc_dap::protocol::Capabilities) -> bool {
        match self {
            SetTarget::Variable { parent, expression, .. } => rc_dap::set_method(capabilities, *parent, expression.as_deref()).is_some(),
            SetTarget::Watch { expression } => rc_dap::set_method(capabilities, 0, Some(expression)).is_some(),
        }
    }
}

/// The inline field of Set Value in the Variables view.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueEdit {
    /// The row being edited.
    pub key: String,
    pub target: SetTarget,
    pub text: String,
    /// The field takes the focus on its next frame.
    pub focus: bool,
    /// The request waiting for its answer.
    pub pending: Option<u64>,
    /// The debugger's error for the last value sent.
    pub error: Option<String>,
}

pub struct Debugger {
    pub session: Option<DebugSession>,
    pub console: Option<crate::runner::Console>,
    pub name: String,
    downloads: Inbox<DownloadEvent>,
    download: Option<rc_dap::netcoredbg::DownloadHandle>,
    pub downloading: Option<(u64, Option<u64>)>,
    pending: Option<(RunConfig, rc_dotnet::RunRequest, PathBuf)>,
    waker: Waker,
    /// Expanded rows of the Variables tree, by path of names (kept across steps).
    pub expanded: BTreeSet<String>,
    /// The selected row of the Variables view (F2 edits its value).
    pub selected_var: Option<String>,
    /// The value being edited (Set Value).
    pub edit: Option<ValueEdit>,
    pub new_watch: String,
    /// Breakpoints stay in the editors but do not stop the program.
    pub muted: bool,
    /// Integers shown in hexadecimal (variables, watches, inline values, evaluation).
    pub hex: bool,
    /// The stop and frame whose values the editor shows.
    inline_frame: Option<(u64, i64)>,
    /// The command being typed in the debugger's console.
    pub console_input: String,
    /// Console commands waiting for their answer.
    console_commands: Vec<u64>,
    /// Evaluate dialog: (request id, expression, result).
    pub eval: Option<Evaluation>,
    /// Hover evaluation: (request id, doc, pos, screen, expression).
    hover: Option<(u64, u64, usize, egui::Pos2, String)>,
    pub hover_result: Option<(u64, usize, egui::Pos2, String)>,
    /// Breakpoint conditions by (file, 0-based line).
    pub conditions: HashMap<(PathBuf, usize), String>,
    /// Logpoint messages by (file, 0-based line).
    pub logs: HashMap<(PathBuf, usize), String>,
    /// Breakpoints that remove themselves at their first stop, by (file, 0-based line).
    pub once: HashSet<(PathBuf, usize)>,
    pub show_console: bool,
    /// The Memory tab is shown (instead of Threads & Variables or the Console).
    pub show_memory: bool,
    pub memory: crate::memory_view::MemoryView,
    /// The Disassembly tab is shown (when neither the Console nor the Memory tab is).
    pub show_disassembly: bool,
    pub disassembly: crate::disassembly_view::DisassemblyView,
    /// Breakpoint set by Run to Cursor, removed at the next stop (file, 1-based line).
    temp: Option<(PathBuf, u32)>,
    /// URLs the debugged web app listens on.
    pub urls: Vec<String>,
    /// Extra environment for the debugged program (tests pick free ports with it).
    pub env: Vec<(String, String)>,
}

impl Debugger {
    pub fn new(waker: Waker) -> Self {
        Debugger {
            session: None,
            console: None,
            name: String::new(),
            downloads: Inbox::new(waker.clone()),
            download: None,
            downloading: None,
            pending: None,
            waker,
            expanded: BTreeSet::new(),
            selected_var: None,
            edit: None,
            new_watch: String::new(),
            eval: None,
            hover: None,
            hover_result: None,
            conditions: HashMap::new(),
            logs: HashMap::new(),
            once: HashSet::new(),
            muted: false,
            hex: false,
            inline_frame: None,
            console_input: String::new(),
            console_commands: Vec::new(),
            show_console: false,
            show_memory: false,
            memory: Default::default(),
            show_disassembly: false,
            disassembly: Default::default(),
            temp: None,
            urls: Vec::new(),
            env: Vec::new(),
        }
    }

    pub fn active(&self) -> bool {
        self.session.as_ref().is_some_and(|s| !s.state().is_ended())
    }

    pub fn stopped(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.state().is_stopped())
    }

    /// Drops what a removed breakpoint had set.
    pub fn forget_breakpoint(&mut self, path: &Path, line: usize) {
        let key = (path.to_path_buf(), line);
        self.conditions.remove(&key);
        self.logs.remove(&key);
        self.once.remove(&key);
    }

    /// Breakpoints of every open editor (1-based lines for the adapter).
    fn breakpoints(&self, docs: &Docs) -> Breakpoints {
        let mut bps = Breakpoints::new();
        for d in &docs.list {
            for (&line, kind) in &d.editor.breakpoints {
                let mut spec = BreakpointSpec::new(d.path.clone(), line as u32 + 1);
                if let Some(c) = self.conditions.get(&(d.path.clone(), line)) {
                    spec = spec.with_condition(c.clone());
                }
                if let Some(m) = self.logs.get(&(d.path.clone(), line)) {
                    spec = spec.with_log_message(m.clone());
                }
                spec.enabled = !self.muted && *kind != rc_editor::BreakpointKind::Disabled;
                bps.set(spec);
            }
        }
        bps
    }

    /// Attaches GDB to a running process.
    pub fn attach_gdb(&mut self, pid: u32, name: &str, docs: &Docs) -> Vec<DebugOutput> {
        if let Some(s) = &mut self.session {
            s.stop();
        }
        let Some(adapter) = rc_dap::gdb::locate() else {
            return vec![DebugOutput::Notify(tr("GDB was not found. Install it to debug Rust programs.").into(), true)];
        };
        self.urls.clear();
        let console = crate::runner::Console::new(&self.waker);
        console.note(&trf("Attaching to {name}…", &[("name", &name)]));
        self.console = Some(console);
        self.name = name.to_string();
        self.expanded.clear();
        self.memory.reset();
        self.disassembly.reset();
        let mut session = DebugSession::start(adapter, DebugTarget::Attach { pid }, DebugOptions::default(), self.breakpoints(docs), self.waker.clone());
        session.set_hex(self.hex);
        self.session = Some(session);
        vec![DebugOutput::ShowDebugWindow]
    }

    /// Starts debugging a Rust binary under GDB.
    pub fn start_cargo(&mut self, config: RunConfig, cargo: rc_cargo::Cargo, mut target: rc_cargo::RunTarget, docs: &Docs) -> Vec<DebugOutput> {
        if let Some(s) = &mut self.session {
            s.stop();
        }
        let Some(adapter) = rc_dap::gdb::locate() else {
            return vec![DebugOutput::Notify(tr("GDB was not found. Install it to debug Rust programs.").into(), true)];
        };
        target.env.extend(self.env.iter().cloned());
        self.urls.clear();
        let console = crate::runner::Console::new(&self.waker);
        console.note(&trf("Building {name}…", &[("name", &config.name)]));
        self.console = Some(console);
        self.name = config.name.clone();
        self.expanded.clear();
        self.memory.reset();
        self.disassembly.reset();
        let watches: Vec<String> = self.session.as_ref().map(|s| s.watches().iter().map(|w| w.expression.clone()).collect()).unwrap_or_default();
        let mut session = DebugSession::start(adapter, DebugTarget::Cargo { cargo, target }, DebugOptions::default(), self.breakpoints(docs), self.waker.clone());
        session.set_hex(self.hex);
        for w in watches {
            session.add_watch(w);
        }
        self.session = Some(session);
        vec![DebugOutput::ShowDebugWindow]
    }

    /// Starts debugging `config` (apps only); downloads netcoredbg first when needed.
    pub fn start(&mut self, config: RunConfig, request: Option<rc_dotnet::RunRequest>, dotnet: &Path, configured: Option<&Path>, docs: &Docs) -> Vec<DebugOutput> {
        let (ConfigKind::App { .. }, Some(mut run)) = (&config.kind, request) else {
            return vec![DebugOutput::Notify(tr("Choose an application run configuration to debug").into(), false)];
        };
        if let Some(s) = &mut self.session {
            s.stop();
        }
        let Some(adapter) = rc_dap::netcoredbg::locate(configured) else {
            if self.download.is_none() {
                self.downloading = Some((0, None));
                self.download = Some(rc_dap::netcoredbg::start_download(self.downloads.sender()));
            }
            self.pending = Some((config.clone(), run, dotnet.to_path_buf()));
            return vec![DebugOutput::Notify(tr("Downloading the netcoredbg debugger (first use)…").into(), false)];
        };
        let Some(dotnet) = rc_dotnet::Dotnet::locate(Some(dotnet)) else {
            return vec![DebugOutput::Notify(tr("The .NET SDK was not found. Install it or set its path in Settings › Tools.").into(), true)];
        };
        run.env.extend(self.env.iter().cloned());
        self.urls.clear();
        let target = DebugTarget::Project { dotnet, run };
        let console = crate::runner::Console::new(&self.waker);
        console.note(&trf("Building {name}…", &[("name", &config.name)]));
        self.console = Some(console);
        self.name = config.name.clone();
        self.expanded.clear();
        self.memory.reset();
        self.disassembly.reset();
        let watches: Vec<String> = self.session.as_ref().map(|s| s.watches().iter().map(|w| w.expression.clone()).collect()).unwrap_or_default();
        let mut session = DebugSession::start(adapter, target, DebugOptions::default(), self.breakpoints(docs), self.waker.clone());
        session.set_hex(self.hex);
        for w in watches {
            session.add_watch(w);
        }
        self.session = Some(session);
        vec![DebugOutput::ShowDebugWindow]
    }

    pub fn stop(&mut self) {
        if let Some(s) = &mut self.session {
            s.stop();
        }
    }

    /// Shows integers in hexadecimal or decimal; a paused session shows them again in the new
    /// format without restarting (variables, watches, inline values, the Evaluate result).
    pub fn set_hex(&mut self, hex: bool) {
        if self.hex == hex {
            return;
        }
        self.hex = hex;
        self.inline_frame = None;
        self.hover_result = None;
        if let Some(s) = &mut self.session {
            s.set_hex(hex);
        }
        if let Some((_, expression, _)) = &self.eval
            && self.stopped()
        {
            let expression = expression.clone();
            self.evaluate(expression);
        }
    }

    /// Opens the inline field of Set Value on a row, with `value` (its value as shown, in
    /// hexadecimal when hex is on) to edit. Nothing happens when not paused.
    pub fn start_edit(&mut self, key: String, target: SetTarget, value: &str) {
        if !self.stopped() {
            return;
        }
        self.selected_var = Some(key.clone());
        self.edit = Some(ValueEdit { key, target, text: value.to_string(), focus: true, pending: None, error: None });
    }

    /// Sends the value typed in the Set Value field; the answer closes it, or shows the
    /// debugger's error under it.
    pub fn apply_edit(&mut self) {
        let (Some(edit), Some(s)) = (&mut self.edit, &mut self.session) else { return };
        let value = edit.text.trim();
        if value.is_empty() || edit.pending.is_some() {
            return;
        }
        edit.error = None;
        edit.pending = Some(match &edit.target {
            SetTarget::Variable { parent, name, expression } => s.set_variable(*parent, name, expression.as_deref(), value),
            SetTarget::Watch { expression } => s.set_expression(expression, value),
        });
    }

    pub fn cancel_edit(&mut self) {
        self.edit = None;
    }

    /// Shows the memory of a variable or watch in the Memory tab: its memory reference, or
    /// the address of `expression`.
    pub fn show_memory_of(&mut self, memory_reference: Option<&str>, expression: &str) {
        let Some(s) = &mut self.session else { return };
        self.memory.open_variable(s, memory_reference, expression);
        self.show_memory = true;
        self.show_console = false;
    }

    /// Shows the memory at the address or expression typed in the Memory tab.
    pub fn go_to_memory(&mut self) {
        let Some(s) = &mut self.session else { return };
        let text = self.memory.input.clone();
        self.memory.go(s, &text);
    }

    /// The previous (`-1`) or next (`1`) page of the Memory tab.
    pub fn memory_page(&mut self, pages: i64) {
        if let Some(s) = &mut self.session {
            self.memory.page(s, pages);
        }
    }

    /// Loads the instructions before (`earlier`) or after the ones in the Disassembly tab.
    pub fn disassembly_more(&mut self, earlier: bool) {
        if let Some(s) = &mut self.session {
            if earlier {
                self.disassembly.load_earlier(s);
            } else {
                self.disassembly.load_later(s);
            }
        }
    }

    /// Runs one machine instruction, over calls (`over`) or into them.
    pub fn step_instruction(&mut self, over: bool) {
        if let Some(s) = self.session.as_mut().filter(|s| s.state().is_stopped()) {
            s.step_instruction(over);
        }
    }

    /// Keeps the session's breakpoints equal to the editors' (toggled in the gutter, edited
    /// conditions, lines moved by edits).
    fn sync_breakpoints(&mut self, docs: &Docs) {
        let Some(s) = &mut self.session else { return };
        if s.state().is_ended() {
            return;
        }
        for d in &docs.list {
            let current: Vec<WantedBreakpoint> = s.breakpoints().in_file(&d.path).iter().map(|b| (b.spec.line, b.spec.condition.clone(), b.spec.log_message.clone(), b.spec.enabled)).collect();
            let wanted: Vec<WantedBreakpoint> = d
                .editor
                .breakpoints
                .iter()
                .map(|(&l, k)| {
                    let key = (d.path.clone(), l);
                    (l as u32 + 1, self.conditions.get(&key).cloned(), self.logs.get(&key).cloned(), !self.muted && *k != rc_editor::BreakpointKind::Disabled)
                })
                .chain(self.temp.iter().filter(|(p, _)| *p == d.path).map(|(_, l)| (*l, None, None, true)))
                .collect();
            let same = current.len() == wanted.len() && wanted.iter().all(|w| current.contains(w));
            if !same {
                let specs = wanted
                    .into_iter()
                    .map(|(line, cond, log, enabled)| {
                        let mut spec = BreakpointSpec::new(d.path.clone(), line);
                        spec.condition = cond;
                        spec.log_message = log;
                        spec.enabled = enabled;
                        spec
                    })
                    .collect();
                s.set_file_breakpoints(&d.path, specs);
            }
        }
    }

    /// Continues to `line` (0-based) of `path` with a one-shot breakpoint; `force` ignores the
    /// other breakpoints on the way (Force Run to Cursor).
    pub fn run_to_cursor(&mut self, path: &Path, line: usize, force: bool) {
        if !self.stopped() {
            return;
        }
        self.temp = Some((path.to_path_buf(), line as u32 + 1));
        if let Some(s) = &mut self.session {
            let spec = BreakpointSpec::new(path.to_path_buf(), line as u32 + 1);
            if force {
                s.force_continue_to(spec);
            } else {
                s.set_breakpoint(spec);
                s.continue_();
            }
        }
    }

    /// Evaluates the expression under the mouse while stopped (hover values).
    pub fn hover(&mut self, doc: u64, pos: usize, screen: egui::Pos2, expression: String) {
        if expression.is_empty() || !self.stopped() {
            return;
        }
        if let Some(s) = &mut self.session {
            let id = s.evaluate(&expression, EvalContext::Hover);
            self.hover = Some((id, doc, pos, screen, expression));
        }
    }

    pub fn evaluate(&mut self, expression: String) {
        if let Some(s) = &mut self.session {
            let context = s.expression_context();
            let id = s.evaluate(&expression, context);
            self.eval = Some((id, expression, None));
        }
    }

    /// Sends the console's line to the debugger as one of its commands.
    pub fn send_console_command(&mut self) {
        let text = std::mem::take(&mut self.console_input);
        if text.trim().is_empty() {
            return;
        }
        if let Some(c) = &self.console {
            c.note(&format!("> {text}"));
        }
        if let Some(s) = &mut self.session {
            self.console_commands.push(s.evaluate(&text, EvalContext::Repl));
        }
    }

    pub fn update(&mut self, docs: &mut Docs) -> Vec<DebugOutput> {
        let mut out = Vec::new();
        for ev in self.downloads.drain() {
            match ev {
                DownloadEvent::Progress { done, total } => self.downloading = Some((done, total)),
                DownloadEvent::Unpacking => {}
                DownloadEvent::Done(_) => {
                    self.downloading = None;
                    self.download = None;
                    out.push(DebugOutput::Notify(tr("netcoredbg is ready").into(), false));
                    if let Some((cfg, run, dotnet)) = self.pending.take() {
                        out.extend(self.start(cfg, Some(run), &dotnet, None, docs));
                    }
                }
                DownloadEvent::Failed(e) => {
                    self.downloading = None;
                    self.download = None;
                    self.pending = None;
                    out.push(DebugOutput::Notify(trf("Could not download netcoredbg: {error}", &[("error", &e)]), true));
                }
            }
        }
        self.sync_breakpoints(docs);
        let temp = self.temp.clone();
        let Some(session) = &mut self.session else { return out };
        let events = session.pump();
        for ev in events {
            self.memory.on_event(&ev, session);
            self.disassembly.on_event(&ev, session);
            match ev {
                DebugEvent::VariableSet { id, name, result, .. } => {
                    let mine = self.edit.as_ref().is_some_and(|e| e.pending == Some(id));
                    match result {
                        Ok(_) => {
                            if mine {
                                self.edit = None;
                            }
                            // What shows the value is drawn again: inline values now, the
                            // memory page read again (watches are evaluated by the session).
                            self.inline_frame = None;
                            self.memory.on_stopped(session);
                        }
                        Err(e) => {
                            let msg = trf("Cannot set {name}: {error}", &[("name", &name), ("error", &e)]);
                            match &mut self.edit {
                                Some(edit) if mine => {
                                    edit.pending = None;
                                    edit.error = Some(msg);
                                    edit.focus = true;
                                }
                                _ => out.push(DebugOutput::Notify(msg, true)),
                            }
                        }
                    }
                }
                DebugEvent::Build(rc_dotnet::BuildEvent::Output(line) | rc_dotnet::BuildEvent::Task(line)) => {
                    if let Some(c) = &self.console {
                        c.line(&line);
                    }
                }
                DebugEvent::Build(_) => {}
                DebugEvent::Prepared(plan) => {
                    self.urls = plan.urls.clone();
                    if let Some(c) = &self.console {
                        c.note(&plan.command_line());
                        if !plan.urls.is_empty() {
                            c.note(&trf("Listening on {urls}", &[("urls", &plan.urls.join(", "))]));
                        }
                    }
                }
                DebugEvent::Output { category, text } => {
                    if let Some(c) = &self.console {
                        let stream = if category == OutputCategory::Stderr { rc_dotnet::Stream::Stderr } else { rc_dotnet::Stream::Stdout };
                        let console_msg = matches!(category, OutputCategory::Console | OutputCategory::Important);
                        if console_msg {
                            c.note(text.trim_end());
                        } else {
                            c.raw(&text, stream);
                        }
                    }
                }
                DebugEvent::Stopped(info) => {
                    if let Some((p, l)) = &temp {
                        let user = docs.list.iter().any(|d| d.path == *p && d.editor.breakpoints.contains_key(&(*l as usize - 1)));
                        if !user {
                            session.remove_breakpoint(p, *l);
                        }
                        self.temp = None;
                    }
                    for (path, line) in &info.breakpoints {
                        let key = (path.clone(), (*line as usize).saturating_sub(1));
                        if self.once.remove(&key) {
                            self.conditions.remove(&key);
                            session.remove_breakpoint(path, *line);
                            if let Some(i) = docs.index_of(path) {
                                docs.list[i].editor.breakpoints.remove(&key.1);
                            }
                            out.push(DebugOutput::BreakpointsChanged);
                        }
                    }
                    if let Some((path, line)) = session.execution_point() {
                        out.push(DebugOutput::ShowLine(path, line.saturating_sub(1) as usize));
                    }
                    self.memory.on_stopped(session);
                    if info.reason == rc_dap::StopReason::Panic {
                        out.push(DebugOutput::Notify(tr("The program panicked").into(), true));
                    }
                    if info.reason == rc_dap::StopReason::Exception {
                        out.push(DebugOutput::Notify(info.description.clone().or(info.text.clone()).unwrap_or_else(|| tr("Exception").into()), true));
                    }
                    out.push(DebugOutput::ShowDebugWindow);
                }
                DebugEvent::FrameSelected { .. } => {
                    if let Some((path, line)) = session.execution_point() {
                        out.push(DebugOutput::ShowLine(path, line.saturating_sub(1) as usize));
                    }
                }
                DebugEvent::Continued => {
                    self.hover_result = None;
                    self.edit = None;
                }
                DebugEvent::Exited(code) => {
                    if let Some(c) = &self.console {
                        c.note(&trf("Process finished with exit code {code}", &[("code", &code)]));
                    }
                }
                DebugEvent::Terminated => {
                    if let Some(c) = &self.console {
                        c.note(tr("Debugger disconnected"));
                    }
                }
                DebugEvent::Failed(f) => {
                    let (tpl, args) = f.user_message();
                    let args: Vec<(&str, &dyn std::fmt::Display)> = args.iter().map(|(k, v)| (*k, v as &dyn std::fmt::Display)).collect();
                    let mut msg = trf(tpl, &args);
                    if matches!(&f, rc_dap::session::Failure::AttachFailed { error, .. } if error.contains("not permitted")) {
                        msg = format!("{msg}\n{}", tr("Linux blocked the attach. Allow it with: sudo sysctl kernel.yama.ptrace_scope=0"));
                    }
                    if let Some(c) = &self.console {
                        c.note(&msg);
                    }
                    out.push(DebugOutput::Notify(msg, true));
                }
                DebugEvent::Evaluated { id, result, .. } => {
                    if let Some(i) = self.console_commands.iter().position(|c| *c == id) {
                        self.console_commands.swap_remove(i);
                        if let Some(c) = &self.console {
                            match &result {
                                Ok(v) => c.term.feed(format!("{}\r\n", v.result.replace('\n', "\r\n")).as_bytes()),
                                Err(e) => c.note(&e.to_string()),
                            }
                        }
                    }
                    if let Some((eid, _, slot)) = &mut self.eval
                        && *eid == id
                    {
                        *slot = Some(result.clone().map_err(|e| e.to_string()));
                    }
                    if let Some((hid, doc, pos, screen, expr)) = &self.hover
                        && *hid == id
                    {
                        if let Ok(v) = &result {
                            let text = match &v.type_name {
                                Some(t) if !t.is_empty() => format!("{expr} = {}  ({t})", v.result),
                                _ => format!("{expr} = {}", v.result),
                            };
                            self.hover_result = Some((*doc, *pos, *screen, text));
                        }
                        self.hover = None;
                    }
                }
                _ => {}
            }
        }
        // The Disassembly tab follows the selected frame and reads again at each stop.
        if self.show_disassembly && !self.show_console && !self.show_memory {
            self.disassembly.sync(session);
        }
        // Execution point and breakpoint state in the editors.
        let exec = if session.state().is_stopped() { session.execution_point() } else { None };
        if let Some((path, line)) = &exec
            && let Some(frame) = session.selected_frame().map(|f| f.id)
            && self.inline_frame != Some((session.generation(), frame))
            && let Some(vars) = frame_variables(session, frame)
            && let Some(d) = docs.list.iter_mut().find(|d| d.path == *path)
        {
            d.editor.debug_values = inline_values(&d.editor.buffer, line.saturating_sub(1) as usize, &vars);
            self.inline_frame = Some((session.generation(), frame));
        }
        let muted = self.muted;
        for d in &mut docs.list {
            d.editor.execution_line = exec.as_ref().filter(|(p, _)| *p == d.path).map(|(_, l)| l.saturating_sub(1) as usize);
            if exec.is_none() {
                d.editor.debug_values.clear();
            }
            for st in session.breakpoints().in_file(&d.path) {
                let line = st.spec.line.saturating_sub(1) as usize;
                if temp.as_ref().is_some_and(|(p, l)| *p == d.path && *l == st.spec.line) && !d.editor.breakpoints.contains_key(&line) {
                    continue;
                }
                // Muting is not the user disabling these breakpoints: the editors keep them.
                if muted {
                    continue;
                }
                if let Some(kind) = d.editor.breakpoints.get_mut(&line) {
                    *kind = if !st.spec.enabled {
                        rc_editor::BreakpointKind::Disabled
                    } else if st.spec.log_message.is_some() {
                        rc_editor::BreakpointKind::Log
                    } else if st.spec.condition.is_some() {
                        rc_editor::BreakpointKind::Conditional
                    } else if st.verified {
                        rc_editor::BreakpointKind::Verified
                    } else if session.state().is_ended() {
                        rc_editor::BreakpointKind::Normal
                    } else {
                        rc_editor::BreakpointKind::Invalid
                    };
                }
            }
        }
        if session.state().is_ended() {
            for d in &mut docs.list {
                d.editor.execution_line = None;
                for k in d.editor.breakpoints.values_mut() {
                    if matches!(k, rc_editor::BreakpointKind::Verified | rc_editor::BreakpointKind::Invalid) {
                        *k = rc_editor::BreakpointKind::Normal;
                    }
                }
            }
        }
        out
    }

    pub fn state_label(&self) -> String {
        match self.session.as_ref().map(|s| s.state()) {
            None => String::new(),
            Some(SessionState::Preparing) => tr("Building…").into(),
            Some(SessionState::Starting) => tr("Starting…").into(),
            Some(SessionState::Running) => tr("Running").into(),
            Some(SessionState::Stopped(info)) => tr(info.reason.label()).into(),
            Some(SessionState::Exited(c)) => trf("Exited with code {code}", &[("code", c)]),
            Some(SessionState::Terminated) => tr("Disconnected").into(),
            Some(SessionState::Failed(f)) => {
                let (tpl, args) = f.user_message();
                let args: Vec<(&str, &dyn std::fmt::Display)> = args.iter().map(|(k, v)| (*k, v as &dyn std::fmt::Display)).collect();
                trf(tpl, &args)
            }
        }
    }
}

/// The expression under `pos` for hover values: the identifier with its member-access chain
/// (`order.Customer`), without calls.
pub fn expression_at(rope: &rc_text::Rope, pos: usize) -> String {
    let (s, e) = rc_text::commands::word_at(rope, pos);
    if s == e {
        return String::new();
    }
    let mut start = s;
    while start >= 2 && rope.char(start - 1) == '.' && {
        let c = rope.char(start - 2);
        c.is_alphanumeric() || c == '_'
    } {
        let (ps, _) = rc_text::commands::word_at(rope, start - 2);
        start = ps;
    }
    rope.slice(start..e).to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn hover_expression_follows_member_chains() {
        let rope = rc_text::Rope::from_str("var t = order.Customer.Name;");
        assert_eq!(super::expression_at(&rope, 25), "order.Customer.Name");
        assert_eq!(super::expression_at(&rope, 9), "order");
    }
}

/// The variables of `frame`, asking for what has not arrived yet (`None` until it all has).
fn frame_variables(session: &mut rc_dap::DebugSession, frame: i64) -> Option<Vec<(String, String)>> {
    let Some(scopes) = session.scopes(frame) else {
        session.request_scopes(frame);
        return None;
    };
    let references: Vec<i64> = scopes.iter().map(|s| s.variables_reference).filter(|r| *r != 0).collect();
    let mut out = Vec::new();
    let mut complete = true;
    for reference in references {
        match session.variables(reference) {
            Some(vars) => out.extend(vars.iter().map(|v| (v.name.clone(), v.value.clone()))),
            None => {
                complete = false;
                if !session.is_loading_variables(reference) {
                    session.request_variables(reference);
                }
            }
        }
    }
    complete.then_some(out)
}

/// Longest value shown at the end of a line.
const INLINE_VALUE_CHARS: usize = 40;
/// How far back to look for the variables of a function with no `fn` line in sight.
const INLINE_LINES: usize = 60;

/// `name = value` for each variable, on the lines from the function's start to `line`
/// that mention it.
fn inline_values(buffer: &rc_text::Buffer, line: usize, vars: &[(String, String)]) -> std::collections::BTreeMap<usize, String> {
    let mut out = std::collections::BTreeMap::new();
    let last = line.min(buffer.len_lines().saturating_sub(1));
    let first = last.saturating_sub(INLINE_LINES);
    let start = (first..=last).rev().find(|l| buffer.line_text(*l).contains("fn ")).unwrap_or(first);
    for l in start..=last {
        let text = buffer.line_text(l);
        let shown: Vec<String> = vars
            .iter()
            .filter(|(name, _)| !name.is_empty() && mentions(&text, name))
            .map(|(name, value)| {
                let short: String = value.chars().take(INLINE_VALUE_CHARS).collect();
                if short.len() < value.len() { format!("{name} = {short}…") } else { format!("{name} = {short}") }
            })
            .collect();
        if !shown.is_empty() {
            out.insert(l, shown.join(", "));
        }
    }
    out
}

/// Whether `word` appears in `text` as a whole identifier.
fn mentions(text: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(i, _)| {
        !text[..i].chars().next_back().is_some_and(is_word) && !text[i + word.len()..].chars().next().is_some_and(is_word)
    })
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn finds_whole_words_only() {
        assert!(mentions("total = add(total, i);", "total"));
        assert!(mentions("x", "x"));
        assert!(!mentions("subtotal += 1", "total"));
        assert!(!mentions("total_price", "total"));
        assert!(!mentions("", "x"));
    }

    #[test]
    fn shows_values_from_the_function_start_to_the_line() {
        let b = rc_text::Buffer::new("let before = 1;\nfn f() {\n    let a = 1;\n    let b = a;\n    let c = 2;\n}\n");
        let vars = vec![("a".to_string(), "1".to_string()), ("before".to_string(), "9".to_string()), ("c".to_string(), "2".to_string())];
        let shown = inline_values(&b, 3, &vars);
        assert_eq!(shown.len(), 2, "{shown:?}");
        assert_eq!(shown[&2], "a = 1");
        assert_eq!(shown[&3], "a = 1");
    }

    #[test]
    fn cuts_long_values_and_survives_a_line_past_the_end() {
        let b = rc_text::Buffer::new("fn f() {\n    x\n}\n");
        let shown = inline_values(&b, 99, &[("x".to_string(), "é".repeat(200))]);
        assert_eq!(shown[&1].chars().count(), "x = ".len() + INLINE_VALUE_CHARS + 1);
    }
}
