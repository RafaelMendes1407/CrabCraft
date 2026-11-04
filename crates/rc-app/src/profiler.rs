//! Profiler (A-01): builds a Rust run configuration, records it with `perf record`, reads the
//! samples back from `perf script` and turns them into a call tree for the flame graph and the
//! call tree view.
//!
//! Everything runs off the UI thread. `perf` and the program run in their own process group:
//! Cancel kills the group, Stop sends it SIGINT so `perf` writes what it has. Output of `perf`
//! is untrusted: lines are parsed defensively, and the number of samples, stack depth and tree
//! size are capped. Functions of the profiled binary are mapped to source lines with
//! `nm --line-numbers`, read from the binary's debug info.
//!
//! When the kernel does not let unprivileged users record (`kernel.perf_event_paranoid` above
//! 2, the default on some distributions), the error says so and how to allow it.

use rc_core::inbox::Outbox;
use rc_core::{Inbox, Waker};
use rc_dotnet::LaunchPlan;
use rc_kit::i18n::{tr, trf};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

/// Samples per second.
const FREQUENCY: &str = "499";
/// Bytes of stack copied per sample for DWARF unwinding (Rust code rarely keeps frame pointers).
const STACK_BYTES: &str = "16384";
/// The most samples read from `perf script`.
const MAX_SAMPLES: u64 = 2_000_000;
/// The most frames of one stack kept (the outermost ones).
const MAX_DEPTH: usize = 256;
/// The most nodes of the call tree.
const MAX_NODES: usize = 500_000;
/// The most program and `perf` output kept.
const MAX_OUTPUT: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfError {
    /// `perf` exists only on Linux.
    NotLinux,
    NoPerf,
    /// The kernel refuses (`perf_event_paranoid`, when it could be read).
    Restricted(Option<i32>),
    /// The program could not be built.
    Build(String),
    /// `perf record` failed: its output.
    Record(String),
    Script(String),
    NoSamples,
    Cancelled,
}

impl ProfError {
    pub fn text(&self) -> String {
        match self {
            ProfError::NotLinux => tr("The profiler uses perf, which is only available on Linux.").to_string(),
            ProfError::NoPerf => tr("perf was not found. Install it (the linux-tools or perf package of your distribution) and try again.").to_string(),
            ProfError::Restricted(level) => {
                let level = level.map_or_else(|| "?".to_string(), |l| l.to_string());
                trf(
                    "The kernel does not allow this user to record performance events (kernel.perf_event_paranoid = {level}). To allow it until the next reboot, run: sudo sysctl kernel.perf_event_paranoid=2",
                    &[("level", &level)],
                )
            }
            ProfError::Build(e) => trf("The build failed: {error}", &[("error", e)]),
            ProfError::Record(out) => format!("{}\n{out}", tr("perf record failed:")),
            ProfError::Script(out) => format!("{}\n{out}", tr("perf script failed:")),
            ProfError::NoSamples => tr("No samples were recorded. The program may have ended too quickly.").to_string(),
            ProfError::Cancelled => tr("Profiling was cancelled.").to_string(),
        }
    }
}

/// `kernel.perf_event_paranoid`: up to 2 lets users record their own programs.
pub fn paranoid_level() -> Option<i32> {
    std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid").ok()?.trim().parse().ok()
}

/// Whether `perf` failed because of the kernel's restrictions.
pub fn is_restricted(stderr: &str) -> bool {
    stderr.contains("perf_event_paranoid") || stderr.contains("Access to performance monitoring and observability operations is limited")
}

/// The `perf` on `PATH`.
pub fn perf_path() -> Option<PathBuf> {
    which("perf")
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Arguments of `perf record` for `plan`, writing to `data`.
pub fn record_args(data: &Path, plan: &LaunchPlan) -> Vec<String> {
    let mut a: Vec<String> = ["record", "-e", "cpu-clock:u", "-F", FREQUENCY, "--call-graph", &format!("dwarf,{STACK_BYTES}"), "--max-size=1G", "-o"].iter().map(|s| s.to_string()).collect();
    a.push(data.to_string_lossy().into_owned());
    a.push("--".into());
    a.push(plan.program.to_string_lossy().into_owned());
    a.extend(plan.args.iter().cloned());
    a
}

/// Arguments of `perf script` for `data`: one header line per sample, then its stack.
pub fn script_args(data: &Path) -> Vec<String> {
    let mut a: Vec<String> = ["script", "--no-inline", "-F", "comm,tid,time,period,event,ip,sym,dso", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(data.to_string_lossy().into_owned());
    a
}

/// A function seen in the samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    pub name: String,
    /// The binary or library it is in.
    pub dso: String,
    /// In the profiled program (not a library).
    pub own: bool,
    /// Its source, when the debug info names one that exists.
    pub source: Option<(PathBuf, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// Index in [`Profile::frames`]; the root has none.
    pub frame: Option<usize>,
    /// Weight of the samples whose stack goes through this node.
    pub total: u64,
    /// Weight of the samples that stopped here.
    pub self_: u64,
    pub children: Vec<usize>,
    pub depth: usize,
}

/// Samples merged into a top-down call tree. `nodes[0]` is the root (all samples).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub frames: Vec<FrameInfo>,
    pub nodes: Vec<Node>,
    pub samples: u64,
    /// More samples or nodes than the caps: the rest was left out.
    pub truncated: bool,
}

impl Profile {
    pub fn total(&self) -> u64 {
        self.nodes.first().map_or(0, |n| n.total)
    }

    pub fn name(&self, node: usize) -> &str {
        match self.nodes.get(node).and_then(|n| n.frame) {
            Some(f) => &self.frames[f].name,
            None => tr("All samples"),
        }
    }

    /// The deepest level of the tree.
    pub fn depth(&self) -> usize {
        self.nodes.iter().map(|n| n.depth).max().unwrap_or(0)
    }

    /// Children of `node`, heaviest first.
    pub fn sorted_children(&self, node: usize) -> Vec<usize> {
        let mut c = self.nodes.get(node).map(|n| n.children.clone()).unwrap_or_default();
        c.sort_by(|a, b| self.nodes[*b].total.cmp(&self.nodes[*a].total));
        c
    }

    /// Where `node`'s function is, if known.
    pub fn source(&self, node: usize) -> Option<&(PathBuf, u32)> {
        self.nodes.get(node)?.frame.and_then(|f| self.frames[f].source.as_ref())
    }
}

/// Builds a [`Profile`] from `perf script` output, line by line.
pub struct ScriptParser {
    profile: Profile,
    frame_ids: HashMap<(String, String), usize>,
    child_of: HashMap<(usize, usize), usize>,
    /// The program's binary: frames in it are the program's own.
    program: Option<String>,
    /// The sample being read: its weight and stack (innermost first).
    weight: u64,
    stack: Vec<usize>,
    in_sample: bool,
}

impl ScriptParser {
    pub fn new(program: Option<&Path>) -> Self {
        let root = Node { frame: None, total: 0, self_: 0, children: Vec::new(), depth: 0 };
        ScriptParser {
            profile: Profile { frames: Vec::new(), nodes: vec![root], samples: 0, truncated: false },
            frame_ids: HashMap::new(),
            child_of: HashMap::new(),
            program: program.map(|p| p.to_string_lossy().into_owned()),
            weight: 0,
            stack: Vec::new(),
            in_sample: false,
        }
    }

    /// Whether the caps were reached (the rest can be skipped).
    pub fn full(&self) -> bool {
        self.profile.samples >= MAX_SAMPLES
    }

    pub fn line(&mut self, line: &str) {
        let line = line.trim_end_matches(['\n', '\r']);
        if line.trim().is_empty() {
            self.end_sample();
            return;
        }
        if line.starts_with([' ', '\t']) {
            if self.in_sample
                && self.stack.len() < MAX_DEPTH * 4
                && let Some((name, dso)) = parse_frame(line)
            {
                let id = self.frame(name, dso);
                self.stack.push(id);
            }
            return;
        }
        // A new header without a blank line before it.
        self.end_sample();
        self.in_sample = true;
        self.weight = header_weight(line);
    }

    fn frame(&mut self, name: &str, dso: &str) -> usize {
        if let Some(&id) = self.frame_ids.get(&(name.to_string(), dso.to_string())) {
            return id;
        }
        let own = self.program.as_deref().is_some_and(|p| p == dso || Path::new(p).file_name().is_some_and(|f| Path::new(dso).file_name() == Some(f)));
        let id = self.profile.frames.len();
        self.profile.frames.push(FrameInfo { name: name.to_string(), dso: dso.to_string(), own, source: None });
        self.frame_ids.insert((name.to_string(), dso.to_string()), id);
        id
    }

    fn end_sample(&mut self) {
        if !std::mem::take(&mut self.in_sample) {
            return;
        }
        let stack = std::mem::take(&mut self.stack);
        if stack.is_empty() || self.full() {
            self.profile.truncated |= self.full();
            return;
        }
        let w = self.weight.max(1);
        self.profile.samples += 1;
        let p = &mut self.profile;
        p.nodes[0].total += w;
        let mut at = 0;
        // Outermost first; very deep stacks keep their outer part.
        for &frame in stack.iter().rev().take(MAX_DEPTH) {
            let next = match self.child_of.get(&(at, frame)) {
                Some(&n) => n,
                None if p.nodes.len() < MAX_NODES => {
                    let n = p.nodes.len();
                    let depth = p.nodes[at].depth + 1;
                    p.nodes.push(Node { frame: Some(frame), total: 0, self_: 0, children: Vec::new(), depth });
                    p.nodes[at].children.push(n);
                    self.child_of.insert((at, frame), n);
                    n
                }
                None => {
                    p.truncated = true;
                    break;
                }
            };
            p.nodes[next].total += w;
            at = next;
        }
        p.nodes[at].self_ += w;
    }

    pub fn finish(mut self) -> Profile {
        self.end_sample();
        self.profile
    }
}

/// The period of a sample header (`comm tid time: period event:`); 1 when it has none.
pub fn header_weight(line: &str) -> u64 {
    let words: Vec<&str> = line.split_whitespace().collect();
    // The time ends with `:`; the period follows it.
    let time = words.iter().position(|w| w.ends_with(':') && w[..w.len() - 1].contains('.') && w[..w.len() - 1].chars().all(|c| c.is_ascii_digit() || c == '.'));
    time.and_then(|i| words.get(i + 1)).and_then(|w| w.parse::<u64>().ok()).filter(|&w| w > 0).unwrap_or(1)
}

/// `    55d0c3a01234 fib::f+0x14 (/path/fib)` → (`fib::f`, `/path/fib`).
pub fn parse_frame(line: &str) -> Option<(&str, &str)> {
    let t = line.trim();
    let (addr, rest) = t.split_once(char::is_whitespace)?;
    if !addr.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let rest = rest.trim();
    let (sym, dso) = match rest.rfind(" (") {
        Some(i) if rest.ends_with(')') => (rest[..i].trim(), &rest[i + 2..rest.len() - 1]),
        _ => (rest, ""),
    };
    // `+0x1f` offsets.
    let sym = match sym.rfind("+0x") {
        Some(i) if sym[i + 3..].chars().all(|c| c.is_ascii_hexdigit()) && i > 0 => &sym[..i],
        _ => sym,
    };
    let sym = if sym.is_empty() { "[unknown]" } else { sym };
    Some((sym, dso))
}

/// A Rust symbol without its hash (`fib::f::h0123456789abcdef` → `fib::f`).
pub fn normalize(sym: &str) -> &str {
    match sym.rfind("::h") {
        Some(i) if sym.len() - i == 19 && sym[i + 3..].chars().all(|c| c.is_ascii_hexdigit()) => &sym[..i],
        _ => sym,
    }
}

/// Function → source line from `nm -C -l` output (`addr t name\tfile:line`). Files that do not
/// exist (the standard library's `/rustc/…`) are left out.
pub fn parse_nm(text: &str) -> HashMap<String, (PathBuf, u32)> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let Some((head, place)) = line.rsplit_once('\t') else { continue };
        let mut parts = head.splitn(3, ' ');
        let (Some(_addr), Some(kind), Some(name)) = (parts.next(), parts.next(), parts.next()) else { continue };
        if !matches!(kind, "t" | "T" | "w" | "W") {
            continue;
        }
        let Some((file, line)) = place.rsplit_once(':') else { continue };
        let Ok(line) = line.trim().parse::<u32>() else { continue };
        out.entry(normalize(name.trim()).to_string()).or_insert((PathBuf::from(file), line));
    }
    out.retain(|_, (file, _)| file.is_file());
    out
}

/// What the background work reports.
#[derive(Debug)]
pub enum ProfEvent {
    Stage(Stage),
    Done(Result<Profile, ProfError>, String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Building,
    Recording,
    Analyzing,
}

/// Stops the work of one profiling session.
#[derive(Default, Debug)]
pub struct Control {
    cancel: AtomicBool,
    /// The process group running now (perf), 0 when none.
    pid: AtomicU32,
}

impl Control {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        rc_dotnet::kill_tree(self.pid.load(Ordering::Relaxed));
    }

    /// Ends the recording early, keeping what was sampled.
    pub fn stop(&self) {
        let pid = self.pid.load(Ordering::Relaxed);
        #[cfg(unix)]
        if pid != 0 {
            // SAFETY: a plain syscall on the process group started for perf.
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGINT);
            }
        }
        let _ = pid;
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A child process with its output read on other threads, at most [`MAX_OUTPUT`] bytes kept.
fn spawn(mut cmd: Command, control: &Control, stdout_to: Option<Box<dyn FnMut(&str) + Send>>) -> std::io::Result<(std::process::Child, Vec<std::thread::JoinHandle<String>>)> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    control.pid.store(child.id(), Ordering::Relaxed);
    let mut readers = Vec::new();
    if let Some(stderr) = child.stderr.take() {
        readers.push(std::thread::spawn(move || read_capped(stderr)));
    }
    if let Some(stdout) = child.stdout.take() {
        readers.push(match stdout_to {
            Some(mut f) => std::thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                // Lines are read as bytes: perf output need not be UTF-8.
                let mut buf = Vec::new();
                while reader.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                    line.clear();
                    line.push_str(&String::from_utf8_lossy(&buf));
                    f(&line);
                    buf.clear();
                }
                String::new()
            }),
            None => std::thread::spawn(move || read_capped(stdout)),
        });
    }
    Ok((child, readers))
}

/// Everything `r` writes, keeping the last [`MAX_OUTPUT`] bytes.
fn read_capped(mut r: impl Read) -> String {
    let mut kept: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    while let Ok(n) = r.read(&mut buf) {
        if n == 0 {
            break;
        }
        kept.extend_from_slice(&buf[..n]);
        if kept.len() > 2 * MAX_OUTPUT {
            kept.drain(..kept.len() - MAX_OUTPUT);
        }
    }
    if kept.len() > MAX_OUTPUT {
        kept.drain(..kept.len() - MAX_OUTPUT);
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// Waits for `child`, killing its group when the session is cancelled.
fn wait(child: &mut std::process::Child, control: &Control) -> Result<std::process::ExitStatus, ProfError> {
    loop {
        if control.cancelled() {
            rc_dotnet::kill_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProfError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(e) => return Err(ProfError::Record(e.to_string())),
        }
    }
}

/// Records `plan` with `perf` and reads the samples. Blocks; run it on its own thread.
/// Returns the profile and the program's output (with perf's messages).
pub fn record_and_analyze(perf: &Path, plan: &LaunchPlan, dir: &Path, control: &Control, stage: &dyn Fn(Stage)) -> (Result<Profile, ProfError>, String) {
    let data = dir.join("perf.data");
    let mut cmd = Command::new(perf);
    cmd.args(record_args(&data, plan)).current_dir(&plan.working_dir);
    for (k, v) in &plan.env {
        cmd.env(k, v);
    }
    stage(Stage::Recording);
    let (mut child, readers) = match spawn(cmd, control, None) {
        Ok(c) => c,
        Err(e) => return (Err(ProfError::Record(e.to_string())), String::new()),
    };
    let status = wait(&mut child, control);
    control.pid.store(0, Ordering::Relaxed);
    let output: String = readers.into_iter().filter_map(|r| r.join().ok()).collect::<Vec<_>>().join("\n");
    if let Err(e) = status {
        return (Err(e), output);
    }
    let recorded = std::fs::metadata(&data).is_ok_and(|m| m.len() > 0);
    if !recorded {
        let err = if is_restricted(&output) { ProfError::Restricted(paranoid_level()) } else { ProfError::Record(output.trim().to_string()) };
        return (Err(err), output);
    }

    stage(Stage::Analyzing);
    let parser = Arc::new(std::sync::Mutex::new(ScriptParser::new(Some(&plan.program))));
    let sink = parser.clone();
    let mut cmd = Command::new(perf);
    cmd.args(script_args(&data));
    let feed: Box<dyn FnMut(&str) + Send> = Box::new(move |line| {
        if let Ok(mut p) = sink.lock()
            && !p.full()
        {
            p.line(line);
        }
    });
    let (mut child, readers) = match spawn(cmd, control, Some(feed)) {
        Ok(c) => c,
        Err(e) => return (Err(ProfError::Script(e.to_string())), output),
    };
    let status = wait(&mut child, control);
    control.pid.store(0, Ordering::Relaxed);
    let errors: String = readers.into_iter().filter_map(|r| r.join().ok()).collect::<Vec<_>>().join("");
    let _ = std::fs::remove_file(&data);
    let status = match status {
        Ok(s) => s,
        Err(e) => return (Err(e), output),
    };
    let Ok(parser) = Arc::try_unwrap(parser).map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner())) else {
        return (Err(ProfError::Script(tr("The samples could not be read.").to_string())), output);
    };
    let mut profile = parser.finish();
    if profile.samples == 0 {
        let err = if status.success() { ProfError::NoSamples } else { ProfError::Script(errors.trim().to_string()) };
        return (Err(err), output);
    }
    // Source lines of the program's own functions.
    if let Some(nm) = which("nm")
        && !control.cancelled()
        && let Ok(out) = Command::new(nm).args(["-C", "-l", "--defined-only"]).arg(&plan.program).stdin(Stdio::null()).stderr(Stdio::null()).output()
    {
        let map = parse_nm(&String::from_utf8_lossy(&out.stdout));
        for f in profile.frames.iter_mut().filter(|f| f.own) {
            f.source = map.get(normalize(&f.name)).cloned();
        }
    }
    (Ok(profile), output)
}

/// A profiling session's state, for the Profiler window.
pub enum Session {
    Building { handle: rc_dotnet::OpHandle, inbox: Inbox<rc_cargo::PrepareEvent> },
    Running { control: Arc<Control>, inbox: Inbox<ProfEvent>, stage: Stage },
}

/// The Profiler: the session in progress and the last result.
pub struct Profiler {
    waker: Waker,
    pub session: Option<Session>,
    /// What is or was profiled.
    pub name: String,
    pub result: Option<Result<Profile, ProfError>>,
    /// The program's and perf's output of the last run.
    pub output: String,
    /// The flame graph's zoomed node.
    pub zoom: usize,
    /// Call tree instead of the flame graph.
    pub show_tree: bool,
    /// Open nodes of the call tree.
    pub open: std::collections::HashSet<usize>,
    pub selected: Option<usize>,
}

impl Profiler {
    pub fn new(waker: Waker) -> Self {
        Profiler { waker, session: None, name: String::new(), result: None, output: String::new(), zoom: 0, show_tree: false, open: Default::default(), selected: None }
    }

    pub fn busy(&self) -> bool {
        self.session.is_some()
    }

    pub fn stage(&self) -> Option<Stage> {
        match &self.session {
            Some(Session::Building { .. }) => Some(Stage::Building),
            Some(Session::Running { stage, .. }) => Some(*stage),
            None => None,
        }
    }

    fn reset(&mut self, name: String) {
        self.name = name;
        self.result = None;
        self.output.clear();
        self.zoom = 0;
        self.open.clear();
        self.selected = None;
    }

    /// Builds `target` and profiles it. Fails at once when perf cannot be used here.
    pub fn start(&mut self, name: String, cargo: &rc_cargo::Cargo, target: rc_cargo::RunTarget) {
        self.cancel();
        self.reset(name);
        if !cfg!(target_os = "linux") {
            self.result = Some(Err(ProfError::NotLinux));
            return;
        }
        if perf_path().is_none() {
            self.result = Some(Err(ProfError::NoPerf));
            return;
        }
        let inbox = Inbox::new(self.waker.clone());
        let handle = rc_cargo::start_prepare(cargo, target, inbox.sender());
        self.session = Some(Session::Building { handle, inbox });
    }

    /// Profiles a program that is already built (tests, and after the build).
    pub fn start_plan(&mut self, plan: LaunchPlan) {
        let Some(perf) = perf_path() else {
            self.session = None;
            self.result = Some(Err(ProfError::NoPerf));
            return;
        };
        let control = Arc::new(Control::default());
        let inbox = Inbox::new(self.waker.clone());
        let tx: Outbox<ProfEvent> = inbox.sender();
        let c = control.clone();
        std::thread::spawn(move || {
            let dir = std::env::temp_dir().join(format!("crabcraft-perf-{}-{}", std::process::id(), unique()));
            let result = match std::fs::create_dir_all(&dir) {
                Ok(()) => record_and_analyze(&perf, &plan, &dir, &c, &|s| {
                    tx.send(ProfEvent::Stage(s));
                }),
                Err(e) => (Err(ProfError::Record(e.to_string())), String::new()),
            };
            let _ = std::fs::remove_dir_all(&dir);
            tx.send(ProfEvent::Done(result.0, result.1));
        });
        self.session = Some(Session::Running { control, inbox, stage: Stage::Recording });
    }

    /// Stops the recording and keeps what was sampled.
    pub fn stop(&mut self) {
        if let Some(Session::Running { control, .. }) = &self.session {
            control.stop();
        }
    }

    pub fn cancel(&mut self) {
        match self.session.take() {
            Some(Session::Building { handle, .. }) => handle.cancel(),
            Some(Session::Running { control, .. }) => control.cancel(),
            None => return,
        }
        self.result = Some(Err(ProfError::Cancelled));
    }

    /// Takes what the background work sent.
    pub fn poll(&mut self) {
        let mut plan = None;
        match &mut self.session {
            Some(Session::Building { inbox, .. }) => {
                for e in inbox.drain() {
                    match e {
                        rc_cargo::PrepareEvent::Ready(p) => plan = Some(p),
                        rc_cargo::PrepareEvent::Failed(m) => {
                            self.result = Some(Err(ProfError::Build(m)));
                            self.session = None;
                            return;
                        }
                        rc_cargo::PrepareEvent::Cancelled => {
                            self.result = Some(Err(ProfError::Cancelled));
                            self.session = None;
                            return;
                        }
                        rc_cargo::PrepareEvent::Build(_) => {}
                    }
                }
            }
            Some(Session::Running { inbox, stage, .. }) => {
                for e in inbox.drain() {
                    match e {
                        ProfEvent::Stage(s) => *stage = s,
                        ProfEvent::Done(result, output) => {
                            self.output = output;
                            // Show the hot path from the start.
                            if let Ok(p) = &result {
                                self.open = hot_path(p);
                            }
                            self.result = Some(result);
                            self.session = None;
                            return;
                        }
                    }
                }
            }
            None => {}
        }
        if let Some(plan) = plan {
            self.start_plan(plan);
        }
    }
}

fn unique() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// The nodes along the heaviest path from the root, while a child holds most of the time.
pub fn hot_path(p: &Profile) -> std::collections::HashSet<usize> {
    let mut open = std::collections::HashSet::new();
    let mut at = 0;
    open.insert(0);
    for _ in 0..64 {
        let Some(&next) = p.sorted_children(at).first() else { break };
        if p.nodes[next].total * 2 < p.nodes[at].total {
            break;
        }
        open.insert(next);
        at = next;
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `perf script -F comm,tid,time,period,event,ip,sym,dso` of a recursive Fibonacci, with a
    /// sample from the C library and one with an unknown frame.
    const SCRIPT: &str = "fib  4321 81234.000101:    2004008 cpu-clock:u: \n\
\t    5581f0a1cc4f fib::f (/home/u/fib/target/debug/fib)\n\
\t    5581f0a1cc70 fib::f (/home/u/fib/target/debug/fib)\n\
\t    5581f0a1cd30 fib::main (/home/u/fib/target/debug/fib)\n\
\t    5581f0a1c9a1 core::ops::function::FnOnce::call_once (/home/u/fib/target/debug/fib)\n\
\t    7f3c9d42a1c9 __libc_start_call_main (/usr/lib/x86_64-linux-gnu/libc.so.6)\n\
\n\
fib  4321 81234.002105:    2004008 cpu-clock:u: \n\
\t    5581f0a1cc4f fib::f+0x1f (/home/u/fib/target/debug/fib)\n\
\t    5581f0a1cd30 fib::main (/home/u/fib/target/debug/fib)\n\
\t    5581f0a1c9a1 core::ops::function::FnOnce::call_once (/home/u/fib/target/debug/fib)\n\
\t    7f3c9d42a1c9 __libc_start_call_main (/usr/lib/x86_64-linux-gnu/libc.so.6)\n\
\n\
fib  4321 81234.004109:    2004008 cpu-clock:u: \n\
\t    7f3c9d4b0a8e __GI___libc_write (/usr/lib/x86_64-linux-gnu/libc.so.6)\n\
\t    5581f0a1cd30 fib::main (/home/u/fib/target/debug/fib)\n\
\t    5581f0a1c9a1 core::ops::function::FnOnce::call_once (/home/u/fib/target/debug/fib)\n\
\t    7f3c9d42a1c9 __libc_start_call_main (/usr/lib/x86_64-linux-gnu/libc.so.6)\n\
\n\
tokio-runtime-w  4322 81234.004200:    1000 cpu-clock:u: \n\
\t                   0 [unknown] ([unknown])\n\
\n";

    fn parse(text: &str) -> Profile {
        let mut p = ScriptParser::new(Some(Path::new("/home/u/fib/target/debug/fib")));
        for l in text.split_inclusive('\n') {
            p.line(l);
        }
        p.finish()
    }

    fn find(p: &Profile, path: &[&str]) -> usize {
        let mut at = 0;
        for name in path {
            at = *p.nodes[at].children.iter().find(|&&c| p.name(c) == *name).unwrap_or_else(|| panic!("no {name} under {}", p.name(at)));
        }
        at
    }

    #[test]
    fn samples_become_a_top_down_tree_with_self_and_total() {
        let p = parse(SCRIPT);
        assert_eq!(p.samples, 4);
        assert_eq!(p.total(), 3 * 2004008 + 1000);
        let start = ["__libc_start_call_main", "core::ops::function::FnOnce::call_once", "fib::main"];
        let main = find(&p, &start);
        assert_eq!(p.nodes[main].total, 3 * 2004008);
        assert_eq!(p.nodes[main].self_, 0);
        let f = find(&p, &[&start[..], &["fib::f"]].concat());
        assert_eq!((p.nodes[f].total, p.nodes[f].self_), (2 * 2004008, 2004008));
        let inner = find(&p, &[&start[..], &["fib::f", "fib::f"]].concat());
        assert_eq!((p.nodes[inner].total, p.nodes[inner].self_, p.nodes[inner].depth), (2004008, 2004008, 5));
        let write = find(&p, &[&start[..], &["__GI___libc_write"]].concat());
        assert_eq!(p.nodes[write].self_, 2004008);
        // The same function keeps one frame; the program's own are marked.
        assert_eq!(p.frames.iter().filter(|f| f.name == "fib::f").count(), 1);
        assert!(p.frames.iter().find(|f| f.name == "fib::main").unwrap().own);
        assert!(!p.frames.iter().find(|f| f.name == "__GI___libc_write").unwrap().own);
        assert_eq!(p.name(0), tr("All samples"));
        assert_eq!(p.sorted_children(main)[0], f);
        let open = hot_path(&p);
        assert!(open.contains(&main) && open.contains(&f));
    }

    #[test]
    fn frames_and_headers_are_read_defensively() {
        assert_eq!(parse_frame("\t    5581f0a1cc4f fib::f+0x1f (/a b/fib)"), Some(("fib::f", "/a b/fib")));
        assert_eq!(parse_frame("  7f00 <T as core::fmt::Display>::fmt (/lib/x.so)"), Some(("<T as core::fmt::Display>::fmt", "/lib/x.so")));
        assert_eq!(parse_frame("  7f00 main::{{closure}} (deleted)"), Some(("main::{{closure}}", "deleted")));
        assert_eq!(parse_frame("  7f00 [unknown] ([unknown])"), Some(("[unknown]", "[unknown]")));
        assert_eq!(parse_frame("  7f00"), None);
        assert_eq!(parse_frame("  zz f (x)"), None);
        assert_eq!(header_weight("fib  4321 81234.000101:    2004008 cpu-clock:u: "), 2004008);
        assert_eq!(header_weight("my prog 12 3.5:  77 cpu-clock:u:"), 77);
        assert_eq!(header_weight("fib 4321"), 1);
        assert_eq!(normalize("fib::f::h0123456789abcdef"), "fib::f");
        assert_eq!(normalize("fib::h::g"), "fib::h::g");
        for junk in ["", "\n\n\n", "\t1 x (y)\n", "header\n\tnothex x\n", "h\n\t\u{0} ( )\n", "a 1.0: 99999999999999999999999 e:\n\tff f (d)\n"] {
            let _ = parse(junk);
        }
        // A header with no blank line before the next one, and a stack with no header.
        let p = parse("\tff lost (x)\na 1.0: 5 e:\n\tff one (x)\nb 2.0: 7 e:\n\tff two (x)\n");
        assert_eq!((p.samples, p.total()), (2, 12));
    }

    #[test]
    fn deep_stacks_and_many_samples_are_capped() {
        let mut text = String::from("p 1 1.0: 1 e:\n");
        for i in 0..2000 {
            text.push_str(&format!("\tff f{i} (bin)\n"));
        }
        let p = parse(&text);
        assert!(p.depth() <= MAX_DEPTH);
        assert_eq!(p.samples, 1);
    }

    #[test]
    fn nm_lines_map_functions_to_existing_sources() {
        let dir = std::env::temp_dir().join(format!("crabcraft-nm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("main.rs");
        std::fs::write(&src, "fn main() {}\n").unwrap();
        let text = format!("0000000000014c40 t fib::f\t{0}:1\n0000000000014d10 t fib::main::h0123456789abcdef\t{0}:2\n000000000004a700 T std::rt::x\t/rustc/abc/library/std/src/rt.rs:9\n                 U __libc_start_main\n0000000000054e30 b data\t{0}:3\nbad line\n", src.display());
        let map = parse_nm(&text);
        assert_eq!(map.get("fib::f"), Some(&(src.clone(), 1)));
        assert_eq!(map.get("fib::main"), Some(&(src.clone(), 2)));
        assert!(!map.contains_key("std::rt::x") && !map.contains_key("data"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn errors_explain_the_paranoid_level() {
        let out = "Error:\nAccess to performance monitoring and observability operations is limited.\nperf_event_paranoid setting is 4:\n";
        assert!(is_restricted(out));
        assert!(!is_restricted("No such file or directory"));
        let text = ProfError::Restricted(Some(4)).text();
        assert!(text.contains("= 4") && text.contains("sysctl"), "{text}");
        let plan = LaunchPlan { program: "/bin/x".into(), args: vec!["--n".into(), "3".into()], env: Vec::new(), working_dir: "/".into(), urls: Vec::new(), profile: None, browser_url: None };
        let args = record_args(Path::new("/tmp/d/perf.data"), &plan);
        let at = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(&args[at + 1..], ["/bin/x", "--n", "3"]);
        assert!(args.windows(2).any(|w| w[0] == "--call-graph" && w[1].starts_with("dwarf")));
    }

    /// The whole path with the real `perf`: either a profile of a real Rust binary, or, where the
    /// kernel forbids it, the paranoid error (never a silent failure).
    #[test]
    fn real_perf_profiles_a_rust_binary_or_explains_why_not() {
        let Some(perf) = perf_path() else {
            eprintln!("skipped: no perf");
            return;
        };
        let dir = std::env::temp_dir().join(format!("crabcraft-perf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("fib.rs");
        std::fs::write(&src, "fn f(n: u64) -> u64 { if n < 2 { n } else { f(n - 1) + f(n - 2) } }\nfn main() { println!(\"{}\", f(std::hint::black_box(34))); }\n").unwrap();
        let bin = dir.join("fib");
        let rustc = std::process::Command::new("rustc").args(["-g", "-o"]).arg(&bin).arg(&src).status();
        if !rustc.is_ok_and(|s| s.success()) {
            eprintln!("skipped: no rustc");
            return;
        }
        // The binary's debug info maps its functions to the source, with or without perf.
        if let Some(nm) = which("nm") {
            let out = std::process::Command::new(nm).args(["-C", "-l", "--defined-only"]).arg(&bin).output().unwrap();
            let map = parse_nm(&String::from_utf8_lossy(&out.stdout));
            assert_eq!(map.get("fib::f"), Some(&(src.canonicalize().unwrap(), 1)), "{:?}", map.keys().filter(|k| k.starts_with("fib")).collect::<Vec<_>>());
            assert_eq!(map.get("fib::main").map(|s| s.1), Some(2));
        }
        let plan = LaunchPlan { program: bin.clone(), args: Vec::new(), env: Vec::new(), working_dir: dir.clone(), urls: Vec::new(), profile: None, browser_url: None };
        let (result, output) = record_and_analyze(&perf, &plan, &dir, &Control::default(), &|_| {});
        let _ = std::fs::remove_dir_all(&dir);
        match result {
            Ok(p) => {
                assert!(p.samples > 10, "{p:?}");
                let f = p.frames.iter().find(|f| normalize(&f.name) == "fib::f").expect("fib::f sampled");
                assert!(f.own && f.source.is_some(), "{f:?}");
            }
            Err(ProfError::Restricted(level)) => {
                assert!(paranoid_level().is_some_and(|l| l > 2), "restricted at {level:?}: {output}");
                eprintln!("perf is restricted here (perf_event_paranoid = {level:?})");
            }
            Err(e) => panic!("{e:?}: {output}"),
        }
    }

    #[test]
    fn cancel_kills_perf_and_its_program() {
        let Some(perf) = perf_path() else { return };
        let dir = std::env::temp_dir().join(format!("crabcraft-perf-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = LaunchPlan { program: "/bin/sleep".into(), args: vec!["30".into()], env: Vec::new(), working_dir: dir.clone(), urls: Vec::new(), profile: None, browser_url: None };
        let control = Arc::new(Control::default());
        let c = control.clone();
        let started = std::time::Instant::now();
        let worker = std::thread::spawn(move || record_and_analyze(&perf, &plan, &dir, &c, &|_| {}));
        std::thread::sleep(Duration::from_millis(300));
        control.cancel();
        let (result, _) = worker.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        // Where perf is refused it ends by itself before the cancel.
        assert!(matches!(result, Err(ProfError::Cancelled | ProfError::Restricted(_))), "{result:?}");
    }
}
