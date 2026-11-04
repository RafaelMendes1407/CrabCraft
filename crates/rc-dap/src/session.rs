//! A debug session owned by the UI thread: builds and launches (or attaches), runs the DAP
//! handshake in the right order, keeps breakpoints in sync, fetches what the UI shows when
//! the program stops and turns everything into [`DebugEvent`]s.
//!
//! [`DebugSession::pump`] is called once per frame and never blocks. The UI reads the state
//! through the accessors (`state`, `frames`, `variables`, `watches`…) and asks for more
//! with the `request_*` methods; answers arrive as events on a later frame.
//!
//! Handshake: `initialize` → `launch`/`attach` → (wait for `initialized`) → `setBreakpoints`
//! for every file → `setExceptionBreakpoints` → (their responses) → `configurationDone`.

use crate::breakpoints::{BreakpointSpec, Breakpoints, Emulation, LogPart, hit_condition_met, parse_log_message};
use crate::client::{DapClient, DapError, Incoming, Pending, args};
use crate::launch::{DebugOptions, DebugTarget, LaunchConfig, attach_arguments, gdb_attach_arguments, gdb_launch_arguments, launch_arguments, prepare};
use crate::netcoredbg::AdapterCommand;
use crate::protocol::*;
use rc_core::{Inbox, Runtime, Waker};
use rc_dotnet::{BuildEvent, CancelToken, LaunchPlan};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Why the program stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    Breakpoint,
    Step,
    Exception,
    /// A Rust panic: the stop shows the code that panicked, not the runtime.
    Panic,
    Pause,
    Entry,
    FunctionBreakpoint,
    Other(String),
}

/// The GDB skip of the standard library ([`crate::gdb`]) is the first one GDB creates.
const GDB_SKIP_DISABLE: &str = "skip disable 1";
const GDB_SKIP_ENABLE: &str = "skip enable 1";

/// The function of the Rust runtime every panic passes through.
const PANIC_FUNCTION: &str = "std::panicking::rust_panic";

/// A frame in the user's source: on disk, and outside the standard library and dependencies.
fn is_user_code(frame: &StackFrame) -> bool {
    frame.path().is_some_and(|p| {
        let text = p.to_string_lossy();
        p.is_file() && !text.contains("/rustlib/") && !text.contains("/.cargo/registry/") && !text.starts_with("/rustc/")
    })
}

impl StopReason {
    pub fn parse(s: &str) -> Self {
        match s {
            "breakpoint" => StopReason::Breakpoint,
            "step" => StopReason::Step,
            "exception" => StopReason::Exception,
            "pause" => StopReason::Pause,
            "entry" => StopReason::Entry,
            "function breakpoint" => StopReason::FunctionBreakpoint,
            other => StopReason::Other(other.to_string()),
        }
    }

    /// English text for `rc_kit::i18n::tr` (translations in `scripts/i18n/rc-dap.tsv`).
    pub fn label(&self) -> &'static str {
        match self {
            StopReason::Breakpoint | StopReason::FunctionBreakpoint => "Paused on breakpoint",
            StopReason::Step => "Paused after step",
            StopReason::Exception => "Paused on exception",
            StopReason::Panic => "Paused on panic",
            StopReason::Entry => "Paused on entry",
            StopReason::Pause | StopReason::Other(_) => "Paused",
        }
    }
}

/// Where and why the program is stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopInfo {
    pub thread_id: i64,
    pub reason: StopReason,
    pub description: Option<String>,
    pub text: Option<String>,
    /// Top frame of the stopped thread (`None` when it has no frames).
    pub frame_id: Option<i64>,
    /// Breakpoints (file, line) that caused the stop.
    pub breakpoints: Vec<(PathBuf, u32)>,
}

/// Why a session failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    AdapterNotStarted(String),
    BuildFailed(String),
    PrepareFailed(String),
    InitializeFailed(String),
    LaunchFailed(String),
    AttachFailed { pid: u32, error: String },
    AdapterExited(Option<i32>),
}

impl Failure {
    /// English template and arguments for `rc_kit::i18n::trf`.
    pub fn user_message(&self) -> (&'static str, Vec<(&'static str, String)>) {
        match self {
            Failure::AdapterNotStarted(e) | Failure::InitializeFailed(e) => ("Cannot start the debugger: {error}", vec![("error", e.clone())]),
            Failure::BuildFailed(_) => ("The build failed", vec![]),
            Failure::PrepareFailed(e) => ("Cannot launch the project: {error}", vec![("error", e.clone())]),
            Failure::LaunchFailed(e) => ("The debugger could not launch the program: {error}", vec![("error", e.clone())]),
            Failure::AttachFailed { pid, error } => ("The debugger could not attach to process {pid}: {error}", vec![("pid", pid.to_string()), ("error", error.clone())]),
            Failure::AdapterExited(_) => ("The debugger exited unexpectedly", vec![]),
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (text, args) = self.user_message();
        let mut s = text.to_string();
        for (k, v) in args {
            s = s.replace(&format!("{{{k}}}"), &v);
        }
        match self {
            Failure::BuildFailed(d) => write!(f, "{s}: {d}"),
            Failure::AdapterExited(Some(code)) => write!(f, "{s} ({code})"),
            _ => f.write_str(&s),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// Building the project and resolving what to launch.
    Preparing,
    /// The adapter is starting; handshake in progress.
    Starting,
    Running,
    Stopped(StopInfo),
    /// The program ended with this exit code.
    Exited(i32),
    /// The session ended without an exit code (stopped by the user, detached).
    Terminated,
    Failed(Failure),
}

impl SessionState {
    pub fn is_ended(&self) -> bool {
        matches!(self, SessionState::Exited(_) | SessionState::Terminated | SessionState::Failed(_))
    }

    pub fn is_stopped(&self) -> bool {
        matches!(self, SessionState::Stopped(_))
    }
}

/// Where an `output` event belongs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputCategory {
    /// The program's stdout.
    Stdout,
    /// The program's stderr.
    Stderr,
    /// Debugger messages (and logpoints).
    Console,
    Important,
    Other(String),
}

impl OutputCategory {
    fn parse(s: Option<&str>) -> Self {
        match s.unwrap_or("console") {
            "stdout" => OutputCategory::Stdout,
            "stderr" => OutputCategory::Stderr,
            "console" => OutputCategory::Console,
            "important" => OutputCategory::Important,
            other => OutputCategory::Other(other.to_string()),
        }
    }
}

/// The value of an expression: expandable through `variables(variables_reference)`.
pub type EvalValue = EvaluateResponse;

#[derive(Clone, Debug, PartialEq)]
pub enum WatchValue {
    /// Not stopped: nothing to evaluate in.
    NotEvaluated,
    Evaluating,
    Value(EvalValue),
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Watch {
    pub id: u64,
    pub expression: String,
    pub value: WatchValue,
}

/// Frames of a thread fetched in this stop.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadFrames {
    pub frames: Vec<StackFrame>,
    pub total: Option<i64>,
    pub loading: bool,
    /// The adapter returned fewer frames than asked: there are no more.
    pub complete: bool,
}

impl ThreadFrames {
    pub fn has_more(&self) -> bool {
        !self.complete && self.total.is_none_or(|t| (self.frames.len() as i64) < t)
    }
}

/// Something the UI should react to.
#[derive(Clone, Debug, PartialEq)]
pub enum DebugEvent {
    Build(BuildEvent),
    /// The project was built and its launch resolved (URLs of web apps are in the plan).
    Prepared(LaunchPlan),
    StateChanged(SessionState),
    /// Program output (stdout/stderr) and debugger messages.
    Output { category: OutputCategory, text: String },
    /// Adapter logs (its stderr) and protocol notes.
    Log(String),
    /// Verification or bound line of breakpoints in this file changed.
    BreakpointsChanged(PathBuf),
    Stopped(StopInfo),
    /// The program runs again; everything fetched in the stop is gone.
    Continued,
    ThreadsChanged,
    FramesLoaded { thread_id: i64 },
    FrameSelected { frame_id: i64 },
    ScopesLoaded { frame_id: i64 },
    VariablesLoaded { reference: i64 },
    /// Loading children failed (the reference is cached as empty).
    VariablesFailed { reference: i64, message: String },
    WatchesUpdated,
    Evaluated { id: u64, expression: String, context: EvalContext, result: Result<EvalValue, String> },
    /// The answer of [`DebugSession::set_variable`] or [`DebugSession::set_expression`] with
    /// its id: the new value as the debugger prints it, or its error message.
    VariableSet { id: u64, reference: i64, name: String, result: Result<String, String> },
    /// The answer of [`DebugSession::read_memory`] with its id.
    MemoryRead { id: u64, result: Result<crate::memory::MemoryBlock, String> },
    /// The answer of [`DebugSession::disassemble`] with its id: instructions in address order.
    Disassembled { id: u64, result: Result<Vec<crate::disassembly::Instruction>, String> },
    /// The exception the program stopped on.
    Exception(ExceptionInfo),
    Module(ModuleEvent),
    /// The debuggee process started (its pid).
    ProcessStarted(u32),
    /// The program exited with this code (the session ends right after).
    Exited(i32),
    /// The session is over (any reason).
    Terminated,
    Failed(Failure),
    /// A command (step, continue, pause…) the adapter refused.
    CommandFailed { command: String, message: String },
    /// Set Execution Point did not move the program.
    ExecutionPointFailed(GotoError),
}

/// Why Set Execution Point did not move the program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GotoError {
    /// GDB moves the execution point only within the function the program is in.
    OtherFunction,
    /// No statement starts at that line.
    NoCode,
    /// The adapter refused (its message).
    Adapter(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct VarKey {
    reference: i64,
    start: Option<i64>,
    count: Option<i64>,
}

enum Purpose {
    Initialize,
    /// `$_gdb_major`, asked before a GDB `launch`.
    GdbVersion,
    /// `gotoTargets`, then `goto` for the first target in this thread.
    GotoTargets { thread_id: i64 },
    /// `goto`, or GDB's jump.
    Goto,
    Launch,
    Config,
    ConfigurationDone,
    SetBreakpoints { path: PathBuf, lines: Vec<u32> },
    Threads { generation: u64 },
    StackTrace { generation: u64, thread_id: i64, for_stop: bool },
    Scopes { generation: u64, frame_id: i64 },
    Variables { generation: u64, key: VarKey, hex: bool },
    Watch { generation: u64, id: u64, hex: bool },
    Evaluate { id: u64, expression: String, context: EvalContext, hex: bool },
    SetVariable { id: u64, generation: u64, reference: i64, name: String, expression: Option<String>, hex: bool },
    ReadMemory { id: u64, requested: Option<u64>, count: usize },
    Disassemble { id: u64 },
    ExceptionInfo { generation: u64 },
    LogPoint { generation: u64, index: usize, part: usize },
    Command(&'static str),
    Ignore,
}

enum Prep {
    Build(BuildEvent),
    Ready(LaunchPlan),
    Failed(Failure),
    Cancelled,
}

#[derive(Default)]
struct Handshake {
    initialized_event: bool,
    initialize_done: bool,
    /// Configuration requests sent (breakpoints, exception filters).
    configuring: bool,
    config_outstanding: usize,
    configuration_done: bool,
    launch_done: bool,
    /// The start request waits for `configurationDone`: GDB before 16 runs the program as soon
    /// as `launch` arrives, and then refuses the breakpoints of every file but the first
    /// (`notStopped`).
    launch_after_config: bool,
}

struct PendingStop {
    event: StoppedEvent,
    generation: u64,
    thread_id: Option<i64>,
}

struct LogPointEval {
    parts: Vec<LogPart>,
    values: Vec<Option<String>>,
    outstanding: usize,
}

/// See the module documentation.
pub struct DebugSession {
    adapter: AdapterCommand,
    options: DebugOptions,
    attach_pid: Option<u32>,
    waker: Waker,
    dap: Inbox<Incoming>,
    prep_inbox: Inbox<Prep>,
    prep_cancel: Option<CancelToken>,
    client: Option<DapClient>,
    /// `launch` or `attach` with its arguments, sent once `initialize` answers.
    start_request: Option<(&'static str, Value)>,
    state: SessionState,
    capabilities: Capabilities,
    hs: Handshake,
    breakpoints: Breakpoints,
    exception_filters: Option<Vec<String>>,
    in_flight: Vec<(Pending<Value>, Purpose)>,
    /// Bumped on every stop and resume: answers of an older generation are stale.
    generation: u64,
    pending_stop: Option<PendingStop>,
    /// The stop that attaching causes was passed.
    attach_resumed: bool,
    threads: Vec<Thread>,
    frames: HashMap<i64, ThreadFrames>,
    scopes: HashMap<i64, Vec<Scope>>,
    variables: HashMap<VarKey, Vec<Variable>>,
    loading: HashSet<(i64, VarKey)>,
    scopes_loading: HashSet<i64>,
    selected_frame: Option<i64>,
    /// Integers shown in hexadecimal (variables, watches, hover and evaluation).
    hex: bool,
    watches: Vec<Watch>,
    next_id: u64,
    exception: Option<ExceptionInfo>,
    modules: Vec<Module>,
    debuggee_pid: Option<u32>,
    exit_code: Option<i32>,
    log_evals: Vec<LogPointEval>,
    /// Thread to resume once the logpoints of an emulated stop are printed.
    continue_after_logs: Option<i64>,
    orphan_breakpoint_events: Vec<Breakpoint>,
    /// Force Step Over and Force Run to Cursor: until the program stops or ends, the adapter
    /// has no breakpoint but this one (file, 1-based line). The user's breakpoints are kept.
    ignoring_breakpoints: Option<Option<(PathBuf, u32)>>,
    /// Force Step Into: GDB's skip of the standard library is off until the program stops.
    skip_disabled: bool,
    shutdown_started: bool,
    events: Vec<DebugEvent>,
}

/// Time the adapter gets to end the debuggee and exit after `disconnect`.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

impl DebugSession {
    /// Starts a session (never blocks): builds first for [`DebugTarget::Project`], then starts
    /// the adapter. `breakpoints` are the ones set before the session (their debugger state
    /// is reset).
    pub fn start(adapter: AdapterCommand, target: DebugTarget, options: DebugOptions, mut breakpoints: Breakpoints, waker: Waker) -> DebugSession {
        breakpoints.reset_state();
        breakpoints.reset_hits();
        let mut s = DebugSession {
            adapter,
            exception_filters: options.exception_filters.clone(),
            options,
            attach_pid: None,
            dap: Inbox::new(waker.clone()),
            prep_inbox: Inbox::new(waker.clone()),
            waker,
            prep_cancel: None,
            client: None,
            start_request: None,
            state: SessionState::Starting,
            capabilities: Capabilities::default(),
            hs: Handshake::default(),
            breakpoints,
            in_flight: Vec::new(),
            generation: 0,
            pending_stop: None,
            attach_resumed: false,
            threads: Vec::new(),
            frames: HashMap::new(),
            scopes: HashMap::new(),
            variables: HashMap::new(),
            loading: HashSet::new(),
            scopes_loading: HashSet::new(),
            selected_frame: None,
            hex: false,
            watches: Vec::new(),
            next_id: 1,
            exception: None,
            modules: Vec::new(),
            debuggee_pid: None,
            exit_code: None,
            log_evals: Vec::new(),
            continue_after_logs: None,
            orphan_breakpoint_events: Vec::new(),
            ignoring_breakpoints: None,
            skip_disabled: false,
            shutdown_started: false,
            events: Vec::new(),
        };
        match target {
            DebugTarget::Project { dotnet, run } => {
                s.set_state(SessionState::Preparing);
                let cancel = CancelToken::new();
                s.prep_cancel = Some(cancel.clone());
                let out = s.prep_inbox.sender();
                Runtime::spawn(async move {
                    let build_out = out.clone();
                    let r = prepare(&dotnet, &run, &cancel, &mut |e| {
                        build_out.send(Prep::Build(e));
                    })
                    .await;
                    out.send(match r {
                        Ok(Some(plan)) => Prep::Ready(plan),
                        Ok(None) => Prep::Cancelled,
                        Err(e) => {
                            let msg = format!("{e:#}");
                            if msg.starts_with("build failed") { Prep::Failed(Failure::BuildFailed(msg)) } else { Prep::Failed(Failure::PrepareFailed(msg)) }
                        }
                    });
                });
            }
            DebugTarget::Cargo { cargo, target } => {
                s.set_state(SessionState::Preparing);
                let cancel = CancelToken::new();
                s.prep_cancel = Some(cancel.clone());
                let out = s.prep_inbox.sender();
                Runtime::spawn(async move {
                    let build_out = out.clone();
                    let r = rc_cargo::prepare(&cargo, &target, &cancel, &mut |e| {
                        build_out.send(Prep::Build(e));
                    })
                    .await;
                    out.send(match r {
                        Ok(Some(plan)) => Prep::Ready(plan),
                        Ok(None) => Prep::Cancelled,
                        Err(e) => {
                            let msg = format!("{e:#}");
                            if msg.starts_with("build failed") { Prep::Failed(Failure::BuildFailed(msg)) } else { Prep::Failed(Failure::PrepareFailed(msg)) }
                        }
                    });
                });
            }
            DebugTarget::Launch(config) => {
                let a = s.launch_arguments(&config);
                s.spawn_adapter("launch", a);
            }
            DebugTarget::Attach { pid } => {
                s.attach_pid = Some(pid);
                s.debuggee_pid = Some(pid);
                let a = if s.is_gdb() { gdb_attach_arguments(pid) } else { attach_arguments(pid, &s.options) };
                s.spawn_adapter("attach", a);
            }
        }
        s
    }

    fn launch_arguments(&self, config: &LaunchConfig) -> Value {
        if self.adapter.origin == crate::netcoredbg::Origin::Gdb { gdb_launch_arguments(config, &self.options) } else { launch_arguments(config, &self.options) }
    }

    fn spawn_adapter(&mut self, command: &'static str, arguments: Value) {
        self.set_state(SessionState::Starting);
        let _guard = Runtime::handle().enter();
        match DapClient::spawn(&self.adapter.program, &self.adapter.args(), None, &[], self.dap.sender(), self.waker.clone()) {
            Ok(client) => {
                let init = json!({
                    "clientID": "crabcraft",
                    "clientName": "CrabCraft",
                    "adapterID": "coreclr",
                    "locale": "en-US",
                    "pathFormat": "path",
                    "linesStartAt1": true,
                    "columnsStartAt1": true,
                    "supportsVariableType": true,
                    "supportsVariablePaging": true,
                    "supportsRunInTerminalRequest": false,
                    "supportsMemoryReferences": true,
                    "supportsProgressReporting": false,
                    "supportsInvalidatedEvent": false,
                });
                self.in_flight.push((client.request("initialize", init), Purpose::Initialize));
                self.client = Some(client);
                self.start_request = Some((command, arguments));
            }
            Err(e) => self.fail(Failure::AdapterNotStarted(format!("{}: {e}", self.adapter.program.display()))),
        }
    }

    // ---- Per frame ----------------------------------------------------------------------------

    /// Processes everything that arrived; returns the events for the UI. Never blocks.
    pub fn pump(&mut self) -> Vec<DebugEvent> {
        for msg in self.prep_inbox.drain() {
            self.on_prep(msg);
        }
        // Responses first: an event that follows a response on the wire (a breakpoint
        // verified after `setBreakpoints` answered) must see its effect.
        self.poll_responses();
        for msg in self.dap.drain() {
            self.on_incoming(msg);
            self.poll_responses();
        }
        std::mem::take(&mut self.events)
    }

    fn poll_responses(&mut self) {
        loop {
            let mut done = Vec::new();
            let mut keep = Vec::new();
            for (mut p, purpose) in std::mem::take(&mut self.in_flight) {
                match p.poll() {
                    Some(r) => done.push((r, purpose)),
                    None => keep.push((p, purpose)),
                }
            }
            self.in_flight = keep;
            if done.is_empty() {
                return;
            }
            for (r, purpose) in done {
                self.on_response(r, purpose);
            }
        }
    }

    fn on_prep(&mut self, msg: Prep) {
        if self.state.is_ended() {
            return;
        }
        match msg {
            Prep::Build(e) => self.events.push(DebugEvent::Build(e)),
            Prep::Ready(plan) => {
                self.prep_cancel = None;
                let config = LaunchConfig::from(&plan);
                self.events.push(DebugEvent::Prepared(plan));
                let a = self.launch_arguments(&config);
                self.spawn_adapter("launch", a);
            }
            Prep::Failed(f) => self.fail(f),
            Prep::Cancelled => self.end(None),
        }
    }

    fn on_incoming(&mut self, msg: Incoming) {
        match msg {
            Incoming::Event { event, body } => self.on_event(&event, body),
            Incoming::Request { seq, command, .. } => {
                let answer = match command.as_str() {
                    "runInTerminal" => Err("CrabCraft runs the program inside the debugger (internalConsole)".to_string()),
                    "startDebugging" => Err("child debug sessions are not supported".to_string()),
                    _ => Err(format!("unsupported request '{command}'")),
                };
                if let Some(c) = &self.client {
                    c.reply(seq, &command, answer);
                }
            }
            Incoming::Stderr(line) => self.events.push(DebugEvent::Log(line)),
            Incoming::Exited(code) => {
                if !self.state.is_ended() {
                    if self.hs.configuration_done && self.exit_code.is_some() {
                        self.end(self.exit_code);
                    } else {
                        self.fail(Failure::AdapterExited(code));
                    }
                }
            }
        }
    }

    fn on_event(&mut self, event: &str, body: Value) {
        fn parse<T: serde::de::DeserializeOwned + Default>(v: Value) -> T {
            serde_json::from_value(v).unwrap_or_default()
        }
        match event {
            "initialized" => {
                self.hs.initialized_event = true;
                self.maybe_configure();
            }
            "capabilities" => {
                if let Some(c) = body.get("capabilities").and_then(|c| serde_json::from_value::<Capabilities>(c.clone()).ok())
                    && !self.hs.initialize_done
                {
                    self.capabilities = c;
                }
            }
            "stopped" => self.on_stopped(parse(body)),
            "continued" => {
                if self.state.is_stopped() {
                    self.invalidate();
                    self.set_state(SessionState::Running);
                    self.events.push(DebugEvent::Continued);
                }
            }
            "exited" => {
                let e: ExitedEvent = parse(body);
                self.exit_code = Some(e.exit_code);
                self.events.push(DebugEvent::Exited(e.exit_code));
            }
            "terminated" => self.end(self.exit_code),
            "output" => {
                let o: OutputEvent = parse(body);
                let category = OutputCategory::parse(o.category.as_deref());
                if category != OutputCategory::Other("telemetry".into()) {
                    self.events.push(DebugEvent::Output { category, text: o.output });
                }
            }
            "breakpoint" => {
                let e: BreakpointEvent = parse(body);
                match self.breakpoints.apply_event(&e.breakpoint) {
                    Some(path) => self.events.push(DebugEvent::BreakpointsChanged(path)),
                    None if e.reason != "removed" => self.orphan_breakpoint_events.push(e.breakpoint),
                    // A one-shot breakpoint of the session's own (Set Execution Point).
                    None => self.orphan_breakpoint_events.retain(|b| b.id.is_none() || b.id != e.breakpoint.id),
                }
            }
            "thread" => {
                let e: ThreadEvent = parse(body);
                if e.reason == "exited" {
                    self.threads.retain(|t| t.id != e.thread_id);
                } else if !self.threads.iter().any(|t| t.id == e.thread_id) {
                    self.threads.push(Thread { id: e.thread_id, name: format!("Thread #{}", e.thread_id) });
                }
                self.events.push(DebugEvent::ThreadsChanged);
            }
            "module" => {
                let e: ModuleEvent = parse(body);
                if e.reason == "removed" {
                    self.modules.retain(|m| m.id != e.module.id);
                } else {
                    self.modules.retain(|m| m.id != e.module.id);
                    self.modules.push(e.module.clone());
                }
                self.events.push(DebugEvent::Module(e));
            }
            "process" => {
                let e: ProcessEvent = parse(body);
                if let Some(pid) = e.system_process_id {
                    self.debuggee_pid = Some(pid);
                    self.events.push(DebugEvent::ProcessStarted(pid));
                }
            }
            _ => {}
        }
    }

    fn on_response(&mut self, r: Result<Value, DapError>, purpose: Purpose) {
        match purpose {
            Purpose::Initialize => match r {
                Ok(v) => {
                    self.capabilities = serde_json::from_value(v).unwrap_or_default();
                    self.hs.initialize_done = true;
                    match &self.client {
                        Some(c) if self.is_gdb() && self.attach_pid.is_none() => {
                            let p = c.request("evaluate", json!({ "expression": "$_gdb_major", "context": "repl" }));
                            self.in_flight.push((p, Purpose::GdbVersion));
                        }
                        _ => self.send_start_request(),
                    }
                    self.maybe_configure();
                }
                Err(e) => self.fail(Failure::InitializeFailed(e.message)),
            },
            Purpose::GdbVersion => {
                let major = r.ok().and_then(|v| gdb_major(&v));
                if major.is_some_and(|m| m < 16) && !self.hs.configuration_done {
                    self.hs.launch_after_config = true;
                } else {
                    self.send_start_request();
                }
            }
            Purpose::Launch => match r {
                Ok(_) => {
                    self.hs.launch_done = true;
                    self.maybe_running();
                }
                Err(e) => match self.attach_pid {
                    Some(pid) => self.fail(Failure::AttachFailed { pid, error: e.message }),
                    None => self.fail(Failure::LaunchFailed(e.message)),
                },
            },
            Purpose::SetBreakpoints { path, lines } => {
                match r.and_then(|v| serde_json::from_value::<SetBreakpointsResponse>(v).map_err(|e| DapError::new(e.to_string()))) {
                    Ok(resp) => {
                        self.breakpoints.apply_response(&path, &lines, &resp.breakpoints);
                        for bp in std::mem::take(&mut self.orphan_breakpoint_events) {
                            if self.breakpoints.apply_event(&bp).is_none() {
                                self.orphan_breakpoint_events.push(bp);
                            }
                        }
                    }
                    Err(e) => {
                        self.events.push(DebugEvent::Log(format!("setBreakpoints {}: {}", path.display(), e.message)));
                        for line in lines {
                            if let Some(b) = self.breakpoints.get_mut(&path, line) {
                                b.verified = false;
                                b.message = Some(e.message.clone());
                            }
                        }
                    }
                }
                self.events.push(DebugEvent::BreakpointsChanged(path));
                self.config_answered();
            }
            Purpose::Config => {
                if let Err(e) = r {
                    self.events.push(DebugEvent::Log(format!("setExceptionBreakpoints: {}", e.message)));
                }
                self.config_answered();
            }
            Purpose::ConfigurationDone => {
                if let Err(e) = &r {
                    self.events.push(DebugEvent::Log(format!("configurationDone: {}", e.message)));
                }
                self.hs.configuration_done = true;
                if self.hs.launch_after_config {
                    self.send_start_request();
                }
                self.maybe_running();
            }
            Purpose::Threads { generation } => {
                if let Ok(t) = r.and_then(|v| serde_json::from_value::<ThreadsResponse>(v).map_err(|e| DapError::new(e.to_string()))) {
                    self.threads = t.threads;
                    self.events.push(DebugEvent::ThreadsChanged);
                }
                // A stop without thread id: use the first thread.
                if let Some(ps) = &mut self.pending_stop
                    && ps.generation == generation
                    && generation == self.generation
                    && ps.thread_id.is_none()
                {
                    match self.threads.first().map(|t| t.id) {
                        Some(tid) => {
                            ps.thread_id = Some(tid);
                            self.request_stack(tid, 0, true);
                        }
                        None => self.commit_stop(Vec::new()),
                    }
                }
            }
            Purpose::StackTrace { generation, thread_id, for_stop } => {
                if generation != self.generation {
                    return;
                }
                let resp = r.and_then(|v| serde_json::from_value::<StackTraceResponse>(v).map_err(|e| DapError::new(e.to_string())));
                let entry = self.frames.entry(thread_id).or_default();
                entry.loading = false;
                let mut matched = Vec::new();
                match resp {
                    Ok(st) => {
                        let asked = self.options.top_frames.max(1) as usize;
                        entry.complete = st.stack_frames.len() < asked;
                        entry.total = st.total_frames;
                        entry.frames.extend(st.stack_frames);
                        if for_stop {
                            matched = entry.frames.clone();
                        }
                    }
                    Err(e) => {
                        entry.complete = true;
                        self.events.push(DebugEvent::Log(format!("stackTrace: {}", e.message)));
                    }
                }
                self.events.push(DebugEvent::FramesLoaded { thread_id });
                if for_stop {
                    self.on_stop_frames(matched);
                }
            }
            Purpose::Scopes { generation, frame_id } => {
                if generation != self.generation {
                    return;
                }
                self.scopes_loading.remove(&frame_id);
                let scopes = r.ok().and_then(|v| serde_json::from_value::<ScopesResponse>(v).ok()).map(|s| s.scopes).unwrap_or_default();
                // GDB lists every CPU register as a scope; the variables tree is not the place.
                let scopes = scopes.into_iter().filter(|s| s.presentation_hint.as_deref() != Some("registers")).collect();
                self.scopes.insert(frame_id, scopes);
                self.events.push(DebugEvent::ScopesLoaded { frame_id });
            }
            Purpose::Variables { generation, key, hex } => {
                // Asked in the other format: the cache was cleared and asked again.
                if generation != self.generation || hex != self.hex {
                    return;
                }
                self.loading.remove(&(key.reference, key));
                match r.and_then(|v| serde_json::from_value::<VariablesResponse>(v).map_err(|e| DapError::new(e.to_string()))) {
                    Ok(mut v) => {
                        if self.client_hex(hex) {
                            for var in v.variables.iter_mut().filter(|var| var.variables_reference == 0) {
                                if let Some(h) = crate::format::hex_integer(&var.value, var.type_name.as_deref()) {
                                    var.value = h;
                                }
                            }
                        }
                        self.variables.insert(key, v.variables);
                        self.events.push(DebugEvent::VariablesLoaded { reference: key.reference });
                    }
                    Err(e) => {
                        self.variables.insert(key, Vec::new());
                        self.events.push(DebugEvent::VariablesFailed { reference: key.reference, message: e.message });
                    }
                }
            }
            Purpose::Watch { generation, id, hex } => {
                if generation != self.generation || hex != self.hex {
                    return;
                }
                let client_hex = self.client_hex(hex);
                if let Some(w) = self.watches.iter_mut().find(|w| w.id == id) {
                    w.value = match r.and_then(|v| serde_json::from_value::<EvaluateResponse>(v).map_err(|e| DapError::new(e.to_string()))) {
                        Ok(v) => WatchValue::Value(hex_result(v, client_hex)),
                        Err(e) => WatchValue::Error(e.message),
                    };
                    self.events.push(DebugEvent::WatchesUpdated);
                }
            }
            Purpose::Evaluate { id, expression, context, hex } => {
                let client_hex = self.client_hex(hex);
                let result = r.and_then(|v| serde_json::from_value::<EvaluateResponse>(v).map_err(|e| DapError::new(e.to_string()))).map(|v| hex_result(v, client_hex)).map_err(|e| e.message);
                self.events.push(DebugEvent::Evaluated { id, expression, context, result });
            }
            Purpose::SetVariable { id, generation, reference, name, expression, hex } => {
                let result = r.and_then(|v| serde_json::from_value::<SetVariableResponse>(v).map_err(|e| DapError::new(e.to_string())));
                match result {
                    Ok(mut v) => {
                        if self.client_hex(hex)
                            && v.variables_reference.unwrap_or(0) == 0
                            && let Some(h) = crate::format::hex_integer(&v.value, v.type_name.as_deref())
                        {
                            v.value = h;
                        }
                        // Answers of an older stop or of the other format are not shown.
                        if generation == self.generation && hex == self.hex {
                            self.apply_set(reference, &name, expression.as_deref(), &v);
                            self.evaluate_watches();
                        }
                        self.events.push(DebugEvent::VariableSet { id, reference, name, result: Ok(v.value) });
                    }
                    Err(e) => self.events.push(DebugEvent::VariableSet { id, reference, name, result: Err(e.message) }),
                }
            }
            Purpose::ReadMemory { id, requested, count } => {
                let result = r
                    .and_then(|v| serde_json::from_value::<ReadMemoryResponse>(v).map_err(|e| DapError::new(e.to_string())))
                    .map_err(|e| e.message)
                    .and_then(|reply| crate::memory::decode_read(requested, count, &reply).map_err(|e| e.to_string()));
                self.events.push(DebugEvent::MemoryRead { id, result });
            }
            Purpose::Disassemble { id } => {
                let result = r
                    .and_then(|v| serde_json::from_value::<DisassembleResponse>(v).map_err(|e| DapError::new(e.to_string())))
                    .map_err(|e| e.message)
                    .and_then(|reply| crate::disassembly::decode(&reply).map_err(|e| e.to_string()));
                self.events.push(DebugEvent::Disassembled { id, result });
            }
            Purpose::ExceptionInfo { generation } => {
                if generation != self.generation {
                    return;
                }
                if let Ok(info) = r.and_then(|v| serde_json::from_value::<ExceptionInfo>(v).map_err(|e| DapError::new(e.to_string()))) {
                    self.exception = Some(info.clone());
                    self.events.push(DebugEvent::Exception(info));
                }
            }
            Purpose::LogPoint { generation, index, part } => {
                if generation != self.generation {
                    return;
                }
                let value = match r.and_then(|v| serde_json::from_value::<EvaluateResponse>(v).map_err(|e| DapError::new(e.to_string()))) {
                    Ok(v) => v.result,
                    Err(e) => format!("<{}>", e.message),
                };
                if let Some(eval) = self.log_evals.get_mut(index) {
                    eval.values[part] = Some(value);
                    eval.outstanding -= 1;
                }
                self.flush_logpoints();
            }
            Purpose::GotoTargets { thread_id } => {
                let target = r.as_ref().ok().and_then(|v| v.get("targets")?.as_array()?.first()?.get("id")?.as_i64());
                match (r, target, &self.client) {
                    (Err(e), _, _) => self.events.push(DebugEvent::ExecutionPointFailed(GotoError::Adapter(e.message))),
                    (Ok(_), None, _) => self.events.push(DebugEvent::ExecutionPointFailed(GotoError::NoCode)),
                    (Ok(_), Some(id), Some(c)) => {
                        let p = c.request("goto", json!({ "threadId": thread_id, "targetId": id }));
                        self.in_flight.push((p, Purpose::Goto));
                    }
                    (Ok(_), Some(_), None) => {}
                }
            }
            Purpose::Goto => {
                if let Err(e) = r {
                    self.events.push(DebugEvent::ExecutionPointFailed(goto_error(e.message)));
                }
            }
            Purpose::Command(command) => {
                if let Err(e) = r {
                    self.events.push(DebugEvent::CommandFailed { command: command.to_string(), message: e.message });
                }
            }
            Purpose::Ignore => {}
        }
    }

    // ---- Handshake ----------------------------------------------------------------------------

    /// Sends `launch` or `attach` (once).
    fn send_start_request(&mut self) {
        if let (Some(c), Some((command, a))) = (&self.client, self.start_request.take()) {
            self.in_flight.push((c.request(command, a), Purpose::Launch));
        }
    }

    fn maybe_configure(&mut self) {
        if !self.hs.initialize_done || !self.hs.initialized_event || self.hs.configuring || self.client.is_none() {
            return;
        }
        self.hs.configuring = true;
        let files: Vec<PathBuf> = self.breakpoints.files().map(Path::to_path_buf).collect();
        for path in files {
            self.send_breakpoints(&path, true);
        }
        self.send_exception_filters(true);
        self.send_panic_breakpoint();
        self.config_answered_check();
    }

    fn config_answered(&mut self) {
        if self.hs.config_outstanding > 0 {
            self.hs.config_outstanding -= 1;
            self.config_answered_check();
        }
    }

    fn config_answered_check(&mut self) {
        if self.hs.configuring && self.hs.config_outstanding == 0 && !self.hs.configuration_done && !self.in_flight.iter().any(|(_, p)| matches!(p, Purpose::ConfigurationDone)) {
            if self.capabilities.supports_configuration_done_request {
                if let Some(c) = &self.client {
                    self.in_flight.push((c.request("configurationDone", json!({})), Purpose::ConfigurationDone));
                }
            } else {
                self.hs.configuration_done = true;
                self.maybe_running();
            }
        }
    }

    fn maybe_running(&mut self) {
        if self.hs.launch_done && self.hs.configuration_done && self.state == SessionState::Starting {
            self.set_state(SessionState::Running);
        }
    }

    fn emulation(&self) -> Emulation {
        Emulation { hit_conditions: !self.capabilities.supports_hit_conditional_breakpoints, log_points: !self.capabilities.supports_log_points }
    }

    fn send_breakpoints(&mut self, path: &Path, during_config: bool) {
        let Some(c) = &self.client else { return };
        let (mut lines, mut bps) = self.breakpoints.request_for(path, self.emulation());
        if let Some(keep) = &self.ignoring_breakpoints {
            let kept = |line: u32| keep.as_ref().is_some_and(|(p, l)| p == path && *l == line);
            (lines, bps) = lines.into_iter().zip(bps).filter(|(l, _)| kept(*l)).unzip();
        }
        let p = c.request("setBreakpoints", args::set_breakpoints(path, &bps));
        if during_config {
            self.hs.config_outstanding += 1;
        }
        self.in_flight.push((p, Purpose::SetBreakpoints { path: path.to_path_buf(), lines }));
    }

    fn send_exception_filters(&mut self, during_config: bool) {
        let Some(c) = &self.client else { return };
        let filters = self.exception_filters.clone().unwrap_or_else(|| self.capabilities.exception_breakpoint_filters.iter().filter(|f| f.default_on).map(|f| f.filter.clone()).collect());
        let p = c.request("setExceptionBreakpoints", json!({"filters": filters}));
        if during_config {
            self.hs.config_outstanding += 1;
            self.in_flight.push((p, Purpose::Config));
        } else {
            self.in_flight.push((p, Purpose::Ignore));
        }
    }

    /// GDB has no exception filter for Rust: panics all pass through this function.
    fn send_panic_breakpoint(&mut self) {
        if !self.is_gdb() {
            return;
        }
        let Some(c) = &self.client else { return };
        let p = c.request("setFunctionBreakpoints", json!({ "breakpoints": [{ "name": PANIC_FUNCTION }] }));
        self.hs.config_outstanding += 1;
        self.in_flight.push((p, Purpose::Config));
    }

    fn is_gdb(&self) -> bool {
        self.adapter.origin == crate::netcoredbg::Origin::Gdb
    }

    /// Whether breakpoint changes go to the adapter right away.
    fn live(&self) -> bool {
        self.client.is_some() && self.hs.configuring && !self.state.is_ended()
    }

    // ---- Stops --------------------------------------------------------------------------------

    fn on_stopped(&mut self, ev: StoppedEvent) {
        if self.state.is_ended() {
            return;
        }
        // GDB stops a process to attach to it, wherever it happened to be: nothing to show.
        if self.is_gdb() && self.attach_pid.is_some() && !self.attach_resumed {
            self.attach_resumed = true;
            if let Some(c) = &self.client {
                let p = c.request("continue", json!({ "threadId": ev.thread_id.unwrap_or(1) }));
                self.in_flight.push((p, Purpose::Ignore));
            }
            return;
        }
        self.end_forced_step();
        self.invalidate();
        let generation = self.generation;
        let thread_id = ev.thread_id;
        self.pending_stop = Some(PendingStop { event: ev, generation, thread_id });
        if let Some(c) = &self.client {
            self.in_flight.push((c.request("threads", json!({})), Purpose::Threads { generation }));
        }
        if let Some(tid) = thread_id {
            self.request_stack(tid, 0, true);
        }
    }

    fn request_stack(&mut self, thread_id: i64, start: i64, for_stop: bool) {
        let Some(c) = &self.client else { return };
        let levels = self.options.top_frames.max(1);
        let p = c.request("stackTrace", args::stack_trace(thread_id, start, levels));
        self.frames.entry(thread_id).or_default().loading = true;
        self.in_flight.push((p, Purpose::StackTrace { generation: self.generation, thread_id, for_stop }));
    }

    /// The top frames of the stopped thread arrived: decide whether to really stop (hit
    /// conditions and logpoints the adapter does not support are evaluated here).
    fn on_stop_frames(&mut self, frames: Vec<StackFrame>) {
        let Some(ps) = &self.pending_stop else { return };
        if ps.event.reason != "breakpoint" {
            self.commit_stop(frames);
            return;
        }
        let top = frames.first();
        let at = self.breakpoints.at_stop(&ps.event.hit_breakpoint_ids, top.and_then(|f| f.path()), top.map_or(0, |f| f.line));
        if at.is_empty() {
            self.commit_stop(frames);
            return;
        }
        let emulation = self.emulation();
        let mut stop = false;
        let mut logs = Vec::new();
        for (path, line) in &at {
            let Some(b) = self.breakpoints.get_mut(path, *line) else { continue };
            b.hits += 1;
            let hit_ok = !emulation.hit_conditions || b.spec.hit_condition.as_deref().is_none_or(|c| hit_condition_met(c, b.hits));
            if !hit_ok {
                continue;
            }
            match (&b.spec.log_message, emulation.log_points) {
                (Some(msg), true) => logs.push(msg.clone()),
                _ => stop = true,
            }
        }
        let frame_id = top.map(|f| f.id);
        let thread_id = ps.thread_id;
        for msg in logs {
            self.start_logpoint(&msg, frame_id);
        }
        if stop {
            self.commit_stop(frames);
        } else {
            // Not a real stop for the user: resume once the logpoints are printed.
            self.continue_after_logs = thread_id;
            self.flush_logpoints();
        }
    }

    fn start_logpoint(&mut self, message: &str, frame_id: Option<i64>) {
        let parts = parse_log_message(message);
        let index = self.log_evals.len();
        let mut eval = LogPointEval { values: vec![None; parts.len()], outstanding: 0, parts };
        let context = if self.is_gdb() { EvalContext::Watch } else { EvalContext::Repl };
        for (i, part) in eval.parts.iter().enumerate() {
            match part {
                LogPart::Text(t) => eval.values[i] = Some(t.clone()),
                LogPart::Expr(e) => {
                    if let Some(c) = &self.client {
                        eval.outstanding += 1;
                        let p = c.request("evaluate", args::evaluate(e, frame_id, context, false));
                        self.in_flight.push((p, Purpose::LogPoint { generation: self.generation, index, part: i }));
                    }
                }
            }
        }
        self.log_evals.push(eval);
    }

    fn flush_logpoints(&mut self) {
        if self.log_evals.iter().any(|e| e.outstanding > 0) {
            return;
        }
        for eval in std::mem::take(&mut self.log_evals) {
            let text: String = eval.values.into_iter().map(|v| v.unwrap_or_default()).collect();
            self.events.push(DebugEvent::Output { category: OutputCategory::Console, text: format!("{text}\n") });
        }
        if let Some(tid) = self.continue_after_logs.take() {
            self.pending_stop = None;
            if let Some(c) = &self.client {
                self.in_flight.push((c.request("continue", json!({"threadId": tid})), Purpose::Command("continue")));
            }
            self.invalidate();
        }
    }

    fn commit_stop(&mut self, frames: Vec<StackFrame>) {
        let Some(ps) = self.pending_stop.take() else { return };
        let thread_id = ps.thread_id.unwrap_or(0);
        let panicked = self.is_gdb() && frames.first().is_some_and(|f| f.name.contains(PANIC_FUNCTION));
        let top = if panicked { frames.iter().find(|f| is_user_code(f)).or(frames.first()) } else { frames.first() };
        let breakpoints = if !panicked && ps.event.reason == "breakpoint" { self.breakpoints.at_stop(&ps.event.hit_breakpoint_ids, top.and_then(|f| f.path()), top.map_or(0, |f| f.line)) } else { Vec::new() };
        let info = StopInfo {
            thread_id,
            reason: if panicked { StopReason::Panic } else { StopReason::parse(&ps.event.reason) },
            description: ps.event.description.clone(),
            text: ps.event.text.clone(),
            frame_id: top.map(|f| f.id),
            breakpoints,
        };
        self.selected_frame = info.frame_id;
        self.set_state(SessionState::Stopped(info.clone()));
        self.events.push(DebugEvent::Stopped(info.clone()));
        if let Some(f) = info.frame_id {
            self.request_scopes(f);
        }
        self.evaluate_watches();
        if info.reason == StopReason::Exception
            && self.capabilities.supports_exception_info_request
            && let Some(c) = &self.client
        {
            self.in_flight.push((c.request("exceptionInfo", json!({"threadId": thread_id})), Purpose::ExceptionInfo { generation: self.generation }));
        }
    }

    /// Forgets everything fetched in the current stop.
    fn invalidate(&mut self) {
        self.generation += 1;
        self.pending_stop = None;
        self.frames.clear();
        self.scopes.clear();
        self.variables.clear();
        self.loading.clear();
        self.scopes_loading.clear();
        self.selected_frame = None;
        self.exception = None;
        self.log_evals.clear();
        for w in &mut self.watches {
            w.value = WatchValue::NotEvaluated;
        }
    }

    fn resume(&mut self, command: &'static str) {
        self.resume_with(command, None);
    }

    /// Resumes with `command`, stepping by `granularity` (`"instruction"`) when given.
    fn resume_with(&mut self, command: &'static str, granularity: Option<&str>) {
        let SessionState::Stopped(info) = &self.state else { return };
        let tid = info.thread_id;
        let Some(c) = &self.client else { return };
        let arguments = match granularity {
            Some(g) => json!({"threadId": tid, "granularity": g}),
            None => json!({"threadId": tid}),
        };
        self.in_flight.push((c.request(command, arguments), Purpose::Command(command)));
        self.invalidate();
        self.set_state(SessionState::Running);
        self.events.push(DebugEvent::Continued);
    }

    // ---- State changes ------------------------------------------------------------------------

    fn set_state(&mut self, state: SessionState) {
        if self.state != state {
            self.state = state.clone();
            self.events.push(DebugEvent::StateChanged(state));
        }
    }

    fn fail(&mut self, failure: Failure) {
        if self.state.is_ended() {
            return;
        }
        self.set_state(SessionState::Failed(failure.clone()));
        self.events.push(DebugEvent::Failed(failure));
        self.events.push(DebugEvent::Terminated);
        self.shutdown();
    }

    fn end(&mut self, code: Option<i32>) {
        if self.state.is_ended() {
            return;
        }
        self.invalidate();
        self.set_state(code.map_or(SessionState::Terminated, SessionState::Exited));
        self.events.push(DebugEvent::Terminated);
        self.shutdown();
    }

    /// Disconnects and makes sure the adapter (and a launched debuggee) are gone after a grace.
    fn shutdown(&mut self) {
        // Nothing to give back to an adapter that is going away.
        self.ignoring_breakpoints = None;
        self.skip_disabled = false;
        if let Some(c) = &self.prep_cancel {
            c.cancel();
        }
        if self.shutdown_started {
            return;
        }
        self.shutdown_started = true;
        let Some(client) = self.client.clone() else { return };
        let launched = self.attach_pid.is_none();
        let debuggee = if launched { self.debuggee_pid } else { None };
        let _guard = Runtime::handle().enter();
        let disconnect = if client.has_exited() { None } else { Some(client.disconnect(launched)) };
        Runtime::spawn(async move {
            if let Some(d) = disconnect {
                let _ = tokio::time::timeout(SHUTDOWN_GRACE, d.wait()).await;
                let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
                while !client.has_exited() && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
            client.kill();
            if let Some(pid) = debuggee {
                rc_dotnet::kill_tree(pid);
            }
        });
    }

    // ---- Public API: control ------------------------------------------------------------------

    /// Ends the session: a launched program is terminated, an attached one detached. The
    /// adapter and the debuggee are killed if they do not exit within [`SHUTDOWN_GRACE`].
    pub fn stop(&mut self) {
        if self.state.is_ended() {
            return;
        }
        self.invalidate();
        self.set_state(SessionState::Terminated);
        self.events.push(DebugEvent::Terminated);
        self.shutdown();
    }

    pub fn continue_(&mut self) {
        self.resume("continue");
    }

    pub fn step_over(&mut self) {
        self.resume("next");
    }

    pub fn step_in(&mut self) {
        self.resume("stepIn");
    }

    pub fn step_out(&mut self) {
        self.resume("stepOut");
    }

    /// Runs one machine instruction, stepping over calls (`over`) or into them. Does nothing
    /// when the adapter cannot step by instruction.
    pub fn step_instruction(&mut self, over: bool) {
        if !self.capabilities.supports_stepping_granularity {
            return;
        }
        self.resume_with(if over { "next" } else { "stepIn" }, Some("instruction"));
    }

    /// Whether Force Step Into can step into the code Step Into skips (GDB's skip of the
    /// standard library); other adapters filter their steps in ways the session cannot lift.
    pub fn can_force_step_in(&self) -> bool {
        self.is_gdb()
    }

    /// Step Into that also enters the standard library: GDB's skip is turned off until the
    /// program stops. Does nothing when [`Self::can_force_step_in`] is false.
    pub fn force_step_in(&mut self) {
        if !self.can_force_step_in() || !self.state.is_stopped() {
            return;
        }
        self.gdb_command(GDB_SKIP_DISABLE);
        self.skip_disabled = true;
        self.step_in();
    }

    /// Step Over that does not stop at the breakpoints of the code it runs.
    pub fn force_step_over(&mut self) {
        if !self.state.is_stopped() {
            return;
        }
        self.ignore_breakpoints(None);
        self.step_over();
    }

    /// Continues to `spec` (a one-shot breakpoint the caller removes) without stopping at any
    /// other breakpoint on the way.
    pub fn force_continue_to(&mut self, spec: BreakpointSpec) {
        if !self.state.is_stopped() {
            return;
        }
        self.ignore_breakpoints(Some((spec.path.clone(), spec.line)));
        self.set_breakpoint(spec);
        self.continue_();
    }

    /// Whether the adapter has the breakpoints taken away by a forced step right now.
    pub fn ignoring_breakpoints(&self) -> bool {
        self.ignoring_breakpoints.is_some()
    }

    /// Takes every breakpoint but `keep` from the adapter until the program stops.
    fn ignore_breakpoints(&mut self, keep: Option<(PathBuf, u32)>) {
        self.ignoring_breakpoints = Some(keep);
        self.send_all_breakpoints();
    }

    fn send_all_breakpoints(&mut self) {
        if !self.live() {
            return;
        }
        let files: Vec<PathBuf> = self.breakpoints.files().map(Path::to_path_buf).collect();
        for f in files {
            self.send_breakpoints(&f, false);
        }
    }

    /// The program stopped after a forced step: the adapter gets back the user's breakpoints
    /// and the skip of the standard library.
    fn end_forced_step(&mut self) {
        if self.ignoring_breakpoints.take().is_some() {
            self.send_all_breakpoints();
        }
        if std::mem::take(&mut self.skip_disabled) {
            self.gdb_command(GDB_SKIP_ENABLE);
        }
    }

    /// Runs a GDB command whose answer nobody reads.
    fn gdb_command(&mut self, command: &str) {
        if let Some(c) = &self.client {
            let p = c.request("evaluate", args::evaluate(command, None, EvalContext::Repl, false));
            self.in_flight.push((p, Purpose::Ignore));
        }
    }

    /// Whether Set Execution Point works with this adapter: GDB jumps, others answer
    /// `gotoTargets`.
    pub fn can_set_execution_point(&self) -> bool {
        self.is_gdb() || self.capabilities.supports_goto_targets_request
    }

    /// Moves the execution point of the stopped thread to `line` (1-based) of `path` without
    /// running the code in between. With GDB, only within the function the program is in, and
    /// only to a line where a statement starts. A failure comes as
    /// [`DebugEvent::ExecutionPointFailed`]; the move itself as a stop.
    pub fn set_execution_point(&mut self, path: &Path, line: u32) {
        let SessionState::Stopped(info) = &self.state else { return };
        let thread_id = info.thread_id;
        let Some(c) = &self.client else { return };
        if self.is_gdb() {
            match gdb_jump(path, line) {
                Some(command) => {
                    let p = c.request("evaluate", args::evaluate(&command, None, EvalContext::Repl, false));
                    self.in_flight.push((p, Purpose::Goto));
                }
                None => self.events.push(DebugEvent::ExecutionPointFailed(GotoError::NoCode)),
            }
        } else if self.capabilities.supports_goto_targets_request {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let p = c.request("gotoTargets", json!({ "source": { "name": name, "path": path }, "line": line }));
            self.in_flight.push((p, Purpose::GotoTargets { thread_id }));
        }
    }

    /// Breaks into the running program.
    pub fn pause(&mut self) {
        if self.state != SessionState::Running {
            return;
        }
        let tid = self.threads.first().map_or(0, |t| t.id);
        if let Some(c) = &self.client {
            self.in_flight.push((c.request("pause", json!({"threadId": tid})), Purpose::Command("pause")));
        }
    }

    // ---- Public API: breakpoints ------------------------------------------------------------

    pub fn breakpoints(&self) -> &Breakpoints {
        &self.breakpoints
    }

    /// Adds or replaces a breakpoint (re-sent to the adapter for its file).
    pub fn set_breakpoint(&mut self, spec: BreakpointSpec) {
        let path = spec.path.clone();
        self.breakpoints.set(spec);
        self.sync_file(&path);
    }

    pub fn remove_breakpoint(&mut self, path: &Path, line: u32) {
        if self.breakpoints.remove(path, line) {
            self.sync_file(path);
        }
    }

    /// Adds a plain breakpoint or removes the one there; true when there is one now.
    pub fn toggle_breakpoint(&mut self, path: &Path, line: u32) -> bool {
        if self.breakpoints.get(path, line).is_some() {
            self.remove_breakpoint(path, line);
            false
        } else {
            self.set_breakpoint(BreakpointSpec::new(path, line));
            true
        }
    }

    pub fn set_breakpoint_enabled(&mut self, path: &Path, line: u32, enabled: bool) {
        if let Some(b) = self.breakpoints.get(path, line) {
            let spec = BreakpointSpec { enabled, ..b.spec.clone() };
            self.set_breakpoint(spec);
        }
    }

    /// Replaces every breakpoint of a file (after the editor moved lines, for instance).
    pub fn set_file_breakpoints(&mut self, path: &Path, specs: Vec<BreakpointSpec>) {
        let lines: Vec<u32> = self.breakpoints.in_file(path).iter().map(|b| b.spec.line).collect();
        for l in lines {
            self.breakpoints.remove(path, l);
        }
        for s in specs {
            self.breakpoints.set(BreakpointSpec { path: path.to_path_buf(), ..s });
        }
        self.sync_file(path);
    }

    fn sync_file(&mut self, path: &Path) {
        if self.live() {
            self.send_breakpoints(path, false);
        }
        self.events.push(DebugEvent::BreakpointsChanged(path.to_path_buf()));
    }

    /// Filters the adapter offers (`all`, `user-unhandled` for netcoredbg).
    pub fn exception_filters_available(&self) -> &[ExceptionBreakpointsFilter] {
        &self.capabilities.exception_breakpoint_filters
    }

    /// The enabled exception filters (`None`: the adapter's defaults).
    pub fn exception_filters(&self) -> Option<&[String]> {
        self.exception_filters.as_deref()
    }

    pub fn set_exception_filters(&mut self, filters: Vec<String>) {
        self.exception_filters = Some(filters);
        if self.live() {
            self.send_exception_filters(false);
        }
    }

    // ---- Public API: inspection ---------------------------------------------------------------

    pub fn state(&self) -> &SessionState {
        &self.state
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    pub fn stop_info(&self) -> Option<&StopInfo> {
        match &self.state {
            SessionState::Stopped(i) => Some(i),
            _ => None,
        }
    }

    pub fn threads(&self) -> &[Thread] {
        &self.threads
    }

    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    /// Frames fetched for a thread in this stop (the stopped thread's top frames come
    /// automatically; [`DebugSession::request_frames`] loads others and more).
    pub fn frames(&self, thread_id: i64) -> Option<&ThreadFrames> {
        self.frames.get(&thread_id)
    }

    /// Loads the frames of a thread, or the next page when some are loaded.
    pub fn request_frames(&mut self, thread_id: i64) {
        if !self.state.is_stopped() {
            return;
        }
        let (start, more) = match self.frames.get(&thread_id) {
            Some(f) if f.loading => return,
            Some(f) => (f.frames.len() as i64, f.has_more()),
            None => (0, true),
        };
        if more {
            self.request_stack(thread_id, start, false);
        }
    }

    pub fn selected_frame(&self) -> Option<&StackFrame> {
        let id = self.selected_frame?;
        self.frames.values().flat_map(|f| &f.frames).find(|f| f.id == id)
    }

    /// The source location to highlight (the selected frame's file and 1-based line).
    pub fn execution_point(&self) -> Option<(PathBuf, u32)> {
        let f = self.selected_frame()?;
        Some((f.path()?.to_path_buf(), f.line))
    }

    /// Makes `frame_id` the context of variables, watches and evaluation.
    pub fn select_frame(&mut self, frame_id: i64) {
        if !self.state.is_stopped() || self.selected_frame == Some(frame_id) {
            return;
        }
        self.selected_frame = Some(frame_id);
        self.events.push(DebugEvent::FrameSelected { frame_id });
        self.request_scopes(frame_id);
        self.evaluate_watches();
    }

    pub fn scopes(&self, frame_id: i64) -> Option<&[Scope]> {
        self.scopes.get(&frame_id).map(Vec::as_slice)
    }

    pub fn request_scopes(&mut self, frame_id: i64) {
        if !self.state.is_stopped() || self.scopes.contains_key(&frame_id) || self.scopes_loading.contains(&frame_id) {
            return;
        }
        let Some(c) = &self.client else { return };
        self.scopes_loading.insert(frame_id);
        self.in_flight.push((c.request("scopes", json!({"frameId": frame_id})), Purpose::Scopes { generation: self.generation, frame_id }));
    }

    /// Children of a scope, variable or evaluation result (all of them), when loaded.
    pub fn variables(&self, reference: i64) -> Option<&[Variable]> {
        self.variables.get(&VarKey { reference, start: None, count: None }).map(Vec::as_slice)
    }

    /// Loads all children of `reference` (cached until the program continues).
    pub fn request_variables(&mut self, reference: i64) {
        self.request_var_key(VarKey { reference, start: None, count: None });
    }

    /// A page of indexed children (`start`, `count`) for big collections.
    pub fn variables_page(&self, reference: i64, start: i64, count: i64) -> Option<&[Variable]> {
        self.variables.get(&VarKey { reference, start: Some(start), count: Some(count) }).map(Vec::as_slice)
    }

    pub fn request_variables_page(&mut self, reference: i64, start: i64, count: i64) {
        self.request_var_key(VarKey { reference, start: Some(start), count: Some(count) });
    }

    fn request_var_key(&mut self, key: VarKey) {
        if key.reference <= 0 || !self.state.is_stopped() || self.variables.contains_key(&key) || self.loading.contains(&(key.reference, key)) {
            return;
        }
        let Some(c) = &self.client else { return };
        self.loading.insert((key.reference, key));
        let p = c.request("variables", args::variables(key.reference, key.start, key.count, self.adapter_hex(self.hex)));
        self.in_flight.push((p, Purpose::Variables { generation: self.generation, key, hex: self.hex }));
    }

    pub fn is_loading_variables(&self, reference: i64) -> bool {
        self.loading.iter().any(|(r, _)| *r == reference)
    }

    /// Changes the value of the variable `name`, a child of `reference` (its scope or parent
    /// variable), to `value` (an expression in the program's language, `0x1f` included).
    ///
    /// Uses `setVariable`, or `setExpression` on `evaluate_name` when the adapter only has
    /// that (see [`set_method`]). The answer comes as [`DebugEvent::VariableSet`] with the
    /// returned id; the cached variable and the watches are updated on success.
    pub fn set_variable(&mut self, reference: i64, name: &str, evaluate_name: Option<&str>, value: &str) -> u64 {
        let id = self.next_id();
        let refuse = match set_method(&self.capabilities, reference, evaluate_name) {
            _ if !self.state.is_stopped() => Err("the program is not paused"),
            None => Err("the debugger cannot change this value"),
            Some(m) => Ok(m),
        };
        let hex = self.hex;
        let format = self.adapter_hex(hex).then(|| json!({"hex": true}));
        let request = refuse.map(|method| match method {
            SetMethod::SetVariable => ("setVariable", json!({"variablesReference": reference, "name": name, "value": value}), None),
            SetMethod::SetExpression => {
                let expression = evaluate_name.unwrap_or(name).to_string();
                let mut a = json!({"expression": expression, "value": value});
                if let Some(frame) = self.selected_frame {
                    a["frameId"] = json!(frame);
                }
                ("setExpression", a, Some(expression))
            }
        });
        match (&self.client, request) {
            (Some(c), Ok((command, mut a, expression))) => {
                if let Some(f) = format {
                    a["format"] = f;
                }
                let p = c.request(command, a);
                self.in_flight.push((p, Purpose::SetVariable { id, generation: self.generation, reference, name: name.to_string(), expression, hex }));
            }
            (_, Err(why)) => self.events.push(DebugEvent::VariableSet { id, reference, name: name.to_string(), result: Err(why.into()) }),
            (None, Ok(_)) => self.events.push(DebugEvent::VariableSet { id, reference, name: name.to_string(), result: Err("the debugger is not running".into()) }),
        }
        id
    }

    /// Changes the value of `expression` (a watch) with `setExpression` in the selected frame;
    /// the answer comes as [`DebugEvent::VariableSet`] with reference 0 and the expression as
    /// its name.
    pub fn set_expression(&mut self, expression: &str, value: &str) -> u64 {
        self.set_variable(0, expression, Some(expression), value)
    }

    /// Puts a value the debugger set into the cached variables: the variable `name` under
    /// `reference`, and any variable known by `expression` (its evaluate name, or its name for
    /// a variable of the selected frame's scopes).
    fn apply_set(&mut self, reference: i64, name: &str, expression: Option<&str>, v: &SetVariableResponse) {
        let frame_scopes: Vec<i64> = self.selected_frame.and_then(|f| self.scopes.get(&f)).map(|sc| sc.iter().map(|s| s.variables_reference).collect()).unwrap_or_default();
        for (key, vars) in &mut self.variables {
            for var in vars.iter_mut() {
                let by_expression = expression.is_some_and(|e| match var.evaluate_name.as_deref().filter(|n| !n.is_empty()) {
                    Some(n) => n == e,
                    None => frame_scopes.contains(&key.reference) && var.name == e,
                });
                let same = (reference > 0 && key.reference == reference && var.name == name) || by_expression;
                if !same {
                    continue;
                }
                var.value = v.value.clone();
                if let Some(t) = &v.type_name {
                    var.type_name = Some(t.clone());
                }
                if let Some(r) = v.variables_reference {
                    var.variables_reference = r;
                }
            }
        }
    }

    pub fn exception(&self) -> Option<&ExceptionInfo> {
        self.exception.as_ref()
    }

    /// Process id of the debuggee (launched or attached), once known.
    pub fn debuggee_pid(&self) -> Option<u32> {
        self.debuggee_pid
    }

    /// Process id of the adapter.
    pub fn adapter_pid(&self) -> Option<u32> {
        self.client.as_ref().map(DapClient::pid)
    }

    /// The raw client, for requests this session does not wrap.
    pub fn client(&self) -> Option<&DapClient> {
        self.client.as_ref()
    }

    // ---- Public API: watches and evaluation -------------------------------------------------

    pub fn watches(&self) -> &[Watch] {
        &self.watches
    }

    /// Adds a watch (evaluated now if stopped, and on every stop).
    pub fn add_watch(&mut self, expression: impl Into<String>) -> u64 {
        let id = self.next_id();
        self.watches.push(Watch { id, expression: expression.into(), value: WatchValue::NotEvaluated });
        self.evaluate_watch(id);
        id
    }

    pub fn edit_watch(&mut self, id: u64, expression: impl Into<String>) {
        if let Some(w) = self.watches.iter_mut().find(|w| w.id == id) {
            w.expression = expression.into();
            w.value = WatchValue::NotEvaluated;
            self.evaluate_watch(id);
        }
    }

    pub fn remove_watch(&mut self, id: u64) {
        self.watches.retain(|w| w.id != id);
    }

    fn evaluate_watches(&mut self) {
        let ids: Vec<u64> = self.watches.iter().map(|w| w.id).collect();
        for id in ids {
            self.evaluate_watch(id);
        }
        if !self.watches.is_empty() {
            self.events.push(DebugEvent::WatchesUpdated);
        }
    }

    fn evaluate_watch(&mut self, id: u64) {
        let frame = self.selected_frame;
        let (hex, adapter_hex) = (self.hex, self.adapter_hex(self.hex));
        let stopped = self.state.is_stopped();
        let generation = self.generation;
        let Some(w) = self.watches.iter_mut().find(|w| w.id == id) else { return };
        let Some(c) = self.client.as_ref().filter(|_| stopped) else {
            w.value = WatchValue::NotEvaluated;
            return;
        };
        w.value = WatchValue::Evaluating;
        let p = c.request("evaluate", args::evaluate(&w.expression, frame, EvalContext::Watch, adapter_hex));
        self.in_flight.push((p, Purpose::Watch { generation, id, hex }));
    }

    /// Evaluates in the selected frame (console, hover); the answer comes as
    /// [`DebugEvent::Evaluated`] with the returned id.
    /// Changes on every stop.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// GDB runs `repl` text as one of its commands: expressions go as `watch` there.
    pub fn expression_context(&self) -> EvalContext {
        if self.is_gdb() { EvalContext::Watch } else { EvalContext::Repl }
    }

    pub fn evaluate(&mut self, expression: &str, context: EvalContext) -> u64 {
        let id = self.next_id();
        match &self.client {
            Some(c) if !self.state.is_ended() => {
                // Debugger commands typed in the console print as they are.
                let hex = self.hex && context != EvalContext::Repl;
                let p = c.request("evaluate", args::evaluate(expression, self.selected_frame, context, self.adapter_hex(hex)));
                self.in_flight.push((p, Purpose::Evaluate { id, expression: expression.to_string(), context, hex }));
            }
            _ => self.events.push(DebugEvent::Evaluated { id, expression: expression.to_string(), context, result: Err("the debugger is not running".into()) }),
        }
        id
    }

    /// Reads `count` bytes (at most [`crate::memory::MAX_READ`]) at `offset` from
    /// `memory_reference` while stopped; the answer comes as [`DebugEvent::MemoryRead`] with
    /// the returned id.
    pub fn read_memory(&mut self, memory_reference: &str, offset: i64, count: usize) -> u64 {
        let id = self.next_id();
        let count = count.min(crate::memory::MAX_READ);
        let refuse = if !self.state.is_stopped() {
            Some("the program is not paused")
        } else if !self.capabilities.supports_read_memory_request {
            Some("the debugger cannot read memory")
        } else {
            None
        };
        match (&self.client, refuse) {
            (Some(c), None) => {
                let requested = crate::memory::parse_address(memory_reference).and_then(|a| a.checked_add_signed(offset));
                let p = c.request("readMemory", json!({"memoryReference": memory_reference, "offset": offset, "count": count}));
                self.in_flight.push((p, Purpose::ReadMemory { id, requested, count }));
            }
            (_, refuse) => self.events.push(DebugEvent::MemoryRead { id, result: Err(refuse.unwrap_or("the debugger is not running").into()) }),
        }
        id
    }

    /// Disassembles `count` instructions (at most [`crate::disassembly::MAX_COUNT`]) from
    /// `memory_reference` plus `offset` bytes, skipping `instruction_offset` instructions,
    /// while stopped; the answer comes as [`DebugEvent::Disassembled`] with the returned id.
    pub fn disassemble(&mut self, memory_reference: &str, offset: i64, instruction_offset: i64, count: usize) -> u64 {
        let id = self.next_id();
        let count = count.clamp(1, crate::disassembly::MAX_COUNT);
        let refuse = if !self.state.is_stopped() {
            Some("the program is not paused")
        } else if !self.capabilities.supports_disassemble_request {
            Some("the debugger cannot disassemble")
        } else {
            None
        };
        match (&self.client, refuse) {
            (Some(c), None) => {
                let p = c.request(
                    "disassemble",
                    json!({"memoryReference": memory_reference, "offset": offset, "instructionOffset": instruction_offset, "instructionCount": count, "resolveSymbols": true}),
                );
                self.in_flight.push((p, Purpose::Disassemble { id }));
            }
            (_, refuse) => self.events.push(DebugEvent::Disassembled { id, result: Err(refuse.unwrap_or("the debugger is not running").into()) }),
        }
        id
    }

    /// Whether integers are shown in hexadecimal.
    pub fn hex(&self) -> bool {
        self.hex
    }

    /// Shows integers in hexadecimal or decimal: what was fetched in this stop is asked again
    /// in the new format (variables when shown, watches now).
    pub fn set_hex(&mut self, hex: bool) {
        if self.hex == hex {
            return;
        }
        self.hex = hex;
        self.variables.clear();
        self.loading.clear();
        if self.state.is_stopped() {
            self.evaluate_watches();
        }
    }

    /// The adapter prints `hex` values itself (`format` in the request).
    fn adapter_hex(&self, hex: bool) -> bool {
        hex && self.capabilities.supports_value_formatting_options
    }

    /// `hex` values are converted here, the adapter cannot.
    fn client_hex(&self, hex: bool) -> bool {
        hex && !self.capabilities.supports_value_formatting_options
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

/// How a value is changed (DAP `setVariable` or `setExpression`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetMethod {
    SetVariable,
    SetExpression,
}

/// How to change a value with this adapter: `setVariable` for a child of `reference` when it
/// has it, otherwise `setExpression` on the value's expression; `None` when it can do
/// neither (the UI shows Set Value as unavailable).
pub fn set_method(capabilities: &Capabilities, reference: i64, evaluate_name: Option<&str>) -> Option<SetMethod> {
    let has_expression = evaluate_name.is_some_and(|e| !e.trim().is_empty());
    if capabilities.supports_set_variable && reference > 0 {
        Some(SetMethod::SetVariable)
    } else if capabilities.supports_set_expression && has_expression {
        Some(SetMethod::SetExpression)
    } else {
        None
    }
}

/// An evaluation result with its integer value in hexadecimal when `hex`.
fn hex_result(mut v: EvaluateResponse, hex: bool) -> EvaluateResponse {
    if hex
        && v.variables_reference == 0
        && let Some(h) = crate::format::hex_integer(&v.result, v.type_name.as_deref())
    {
        v.result = h;
    }
    v
}

impl Drop for DebugSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Marks the errors of [`gdb_jump`].
const GOTO_NO_CODE: &str = "crabcraft-goto: no code";
const GOTO_OTHER_FUNCTION: &str = "crabcraft-goto: other function";

/// The GDB command for Set Execution Point: a Python check that `line` of `path` starts a
/// statement of the function the program is in (GDB would jump anywhere, asking nobody under
/// DAP), then a one-shot breakpoint there and `jump`. `None` for a path GDB cannot be given.
fn gdb_jump(path: &Path, line: u32) -> Option<String> {
    let path = path.to_str()?;
    if path.contains(['"', '\n', '\r']) {
        return None;
    }
    let location = serde_json::to_string(&format!("\"{path}\":{line}")).ok()?;
    let script = format!(
        "import gdb
try:
    sals = gdb.decode_line({location})[1]
except gdb.error:
    sals = None
if not sals or sals[0].line != {line}:
    raise gdb.GdbError(\"{GOTO_NO_CODE}\")
pc = sals[0].pc
def function(block):
    while block is not None and block.function is None:
        block = block.superblock
    return block
here = function(gdb.newest_frame().block())
there = function(gdb.block_for_pc(pc))
if here is None or there is None or (here.start, here.end) != (there.start, there.end):
    raise gdb.GdbError(\"{GOTO_OTHER_FUNCTION}\")
if pc == there.start:
    raise gdb.GdbError(\"{GOTO_NO_CODE}\")
gdb.execute(\"tbreak *%d\" % pc)
gdb.execute(\"jump *%d\" % pc)
"
    );
    Some(format!("python exec({})", serde_json::to_string(&script).ok()?))
}

fn goto_error(message: String) -> GotoError {
    if message.contains(GOTO_OTHER_FUNCTION) {
        GotoError::OtherFunction
    } else if message.contains(GOTO_NO_CODE) {
        GotoError::NoCode
    } else {
        GotoError::Adapter(message)
    }
}

/// The major version from the answer to `evaluate $_gdb_major`.
fn gdb_major(v: &Value) -> Option<u32> {
    v.get("result")?.as_str()?.trim().parse().ok()
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
