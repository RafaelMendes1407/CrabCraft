//! State machine tests against `tests/fake_adapter.py`, a scripted adapter that logs the
//! requests it receives.

use super::*;
use crate::netcoredbg::Origin;
use std::time::Instant;

fn fake(dir: &Path, mode: &str) -> (AdapterCommand, PathBuf) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_adapter.py");
    let log = dir.join("requests.jsonl");
    let mut cmd = AdapterCommand::new("python3", Origin::Settings);
    cmd.prefix = vec!["-u".into(), script.display().to_string(), log.display().to_string(), mode.into()];
    (cmd, log)
}

fn requests(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn commands(log: &Path) -> Vec<String> {
    requests(log).iter().map(|r| r["command"].as_str().unwrap().to_string()).collect()
}

fn launch() -> DebugTarget {
    DebugTarget::Launch(LaunchConfig { program: "/usr/bin/dotnet".into(), args: vec!["app.dll".into()], cwd: "/w".into(), env: vec![("A".into(), "1".into())] })
}

fn calc() -> PathBuf {
    PathBuf::from("/w/Calc.cs")
}

/// Pumps until `done` holds for an event or for the session.
fn pump_until(s: &mut DebugSession, all: &mut Vec<DebugEvent>, mut done: impl FnMut(&DebugEvent, &DebugSession) -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        let mut batch = s.pump().into_iter();
        while let Some(e) = batch.next() {
            let stop = done(&e, s);
            all.push(e);
            if stop {
                // One pump may coalesce the stop, scopes and other replies. Keep the
                // unread tail for the next waiter instead of losing those notifications.
                s.events.splice(0..0, batch);
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out in state {:?}; events {all:#?}", s.state());
}

#[test]
fn event_waiters_keep_coalesced_notifications_for_the_next_waiter() {
    let dir = tempfile::tempdir().unwrap();
    let (adapter, _) = fake(dir.path(), "late");
    let mut session = DebugSession::start(adapter, launch(), DebugOptions::default(), Breakpoints::new(), Waker::noop());
    session.events.extend([DebugEvent::Log("first".into()), DebugEvent::Log("second".into())]);
    let mut events = Vec::new();
    pump_until(&mut session, &mut events, |event,_|matches!(event,DebugEvent::Log(text) if text=="first"));
    pump_until(&mut session, &mut events, |event,_|matches!(event,DebugEvent::Log(text) if text=="second"));
    assert_eq!(events.iter().filter(|event|matches!(event,DebugEvent::Log(text) if text=="first"||text=="second")).count(),2);
    session.stop();
}

/// Pumps until the session satisfies `done`.
fn pump_until_state(s: &mut DebugSession, all: &mut Vec<DebugEvent>, mut done: impl FnMut(&DebugSession) -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        all.extend(s.pump());
        if done(s) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out in state {:?}; events {all:#?}", s.state());
}

fn stopped(e: &DebugEvent, _: &DebugSession) -> bool {
    matches!(e, DebugEvent::Stopped(_))
}

fn watch_value(s: &DebugSession, id: u64) -> WatchValue {
    s.watches().iter().find(|w| w.id == id).unwrap().value.clone()
}

fn settled(s: &DebugSession) -> bool {
    s.watches().iter().all(|w| !matches!(w.value, WatchValue::Evaluating))
}

fn result_of(v: WatchValue) -> String {
    match v {
        WatchValue::Value(v) => v.result,
        other => panic!("no value: {other:?}"),
    }
}

#[test]
fn handshake_order_stops_caches_watches_and_exit() {
    let dir = tempfile::tempdir().unwrap();
    let (adapter, log) = fake(dir.path(), "late");
    let mut bps = Breakpoints::new();
    bps.set(BreakpointSpec::new(calc(), 9));
    bps.set(BreakpointSpec::new("/w/Other.cs", 3).with_condition("y > 1"));
    let mut s = DebugSession::start(adapter, launch(), DebugOptions::default(), bps, Waker::noop());
    let mut ev = Vec::new();
    pump_until(&mut s, &mut ev, stopped);

    // initialize → launch → (initialized) → setBreakpoints per file → exception filters → configurationDone.
    assert_eq!(commands(&log)[..6], ["initialize", "launch", "setBreakpoints", "setBreakpoints", "setExceptionBreakpoints", "configurationDone"]);
    let reqs = requests(&log);
    assert_eq!(reqs[1]["arguments"]["program"], "/usr/bin/dotnet");
    assert_eq!(reqs[1]["arguments"]["env"]["A"], "1");
    assert_eq!(reqs[1]["arguments"]["justMyCode"], true);
    assert_eq!(reqs[3]["arguments"]["breakpoints"][0]["condition"], "y > 1");
    assert_eq!(reqs[4]["arguments"]["filters"], json!(["user-unhandled"]));
    assert_eq!(s.exception_filters_available().len(), 2);

    let states: Vec<SessionState> = ev.iter().filter_map(|e| if let DebugEvent::StateChanged(s) = e { Some(s.clone()) } else { None }).collect();
    assert_eq!(states[0], SessionState::Running);
    assert!(matches!(states[1], SessionState::Stopped(_)));
    assert!(ev.contains(&DebugEvent::ProcessStarted(4242)));
    assert_eq!(s.debuggee_pid(), Some(4242));
    assert!(ev.contains(&DebugEvent::Output { category: OutputCategory::Stdout, text: "hello\n".into() }));
    assert!(!ev.iter().any(|e| matches!(e, DebugEvent::Output { text, .. } if text == "ignored")));

    // Verification and the line the debugger bound the breakpoint to.
    let b = s.breakpoints().get(&calc(), 9).unwrap();
    assert!(b.verified);
    assert_eq!(b.display_line(), 10);
    assert!(s.breakpoints().get(Path::new("/w/Other.cs"), 3).unwrap().verified);

    let info = s.stop_info().unwrap().clone();
    assert_eq!((info.thread_id, &info.reason, info.frame_id), (1, &StopReason::Breakpoint, Some(10)));
    assert_eq!(info.breakpoints, vec![(calc(), 9)]);
    let frames = s.frames(1).unwrap();
    assert_eq!(frames.frames.len(), 2);
    assert!(!frames.has_more());
    assert_eq!(s.execution_point(), Some((calc(), 10)));

    // Scopes come with the stop, variables on demand (cached).
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::ScopesLoaded { frame_id: 10 }));
    let locals = s.scopes(10).unwrap()[0].variables_reference;
    assert!(s.variables(locals).is_none());
    s.request_variables(locals);
    assert!(s.is_loading_variables(locals));
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::VariablesLoaded { .. }));
    let vars = s.variables(locals).unwrap().to_vec();
    assert_eq!((vars[0].name.as_str(), vars[0].value.as_str()), ("x", "1"));
    s.request_variables(vars[1].variables_reference);
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::VariablesLoaded { .. }));
    assert_eq!(s.variables(vars[1].variables_reference).unwrap()[0].value, "\"Ada\"");

    // Watches and evaluation.
    let w = s.add_watch("x * 10");
    let bad = s.add_watch("nope");
    pump_until_state(&mut s, &mut ev, settled);
    assert_eq!(result_of(watch_value(&s, w)), "10");
    assert!(matches!(watch_value(&s, bad), WatchValue::Error(m) if m.contains("CS0103")));
    let id = s.evaluate("order", EvalContext::Repl);
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::Evaluated { .. }));
    let Some(DebugEvent::Evaluated { id: got, result: Ok(v), .. }) = ev.last().cloned() else { panic!() };
    assert_eq!((got, v.variables_reference), (id, 2001));

    // setVariable updates the cache and re-evaluates the watches.
    let set = s.set_variable(locals, "x", Some("x"), "2");
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::VariableSet { id, result: Ok(v), .. } if *id == set && v == "2"));
    pump_until_state(&mut s, &mut ev, settled);
    assert_eq!(s.variables(locals).unwrap()[0].value, "2");
    assert_eq!(requests(&log).iter().filter(|r| r["command"] == "setVariable").count(), 1);
    assert_eq!(result_of(watch_value(&s, w)), "20");

    // Continue invalidates everything fetched in the stop; the next stop re-evaluates watches.
    s.continue_();
    assert_eq!(s.state(), &SessionState::Running);
    assert!(s.variables(locals).is_none() && s.frames(1).is_none() && s.scopes(10).is_none());
    assert_eq!(watch_value(&s, w), WatchValue::NotEvaluated);
    pump_until(&mut s, &mut ev, stopped);
    pump_until_state(&mut s, &mut ev, settled);
    assert_eq!(result_of(watch_value(&s, w)), "30");
    assert_eq!(s.breakpoints().get(&calc(), 9).unwrap().hits, 2);

    // Steps.
    s.step_over();
    pump_until(&mut s, &mut ev, stopped);
    assert_eq!(s.stop_info().unwrap().reason, StopReason::Step);
    assert_eq!(s.execution_point().unwrap().1, 11);
    s.step_in();
    pump_until(&mut s, &mut ev, stopped);
    assert_eq!(s.execution_point().unwrap().1, 20);
    s.step_out();
    pump_until(&mut s, &mut ev, stopped);
    assert_eq!(s.execution_point().unwrap().1, 12);

    // Selecting another frame changes the context.
    let caller = s.frames(1).unwrap().frames[1].id;
    s.select_frame(caller);
    assert_eq!(s.execution_point(), Some((PathBuf::from("/w/Program.cs"), 5)));

    // Breakpoint edits while stopped re-send that file only.
    let before = commands(&log).len();
    s.remove_breakpoint(&calc(), 9);
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::BreakpointsChanged(p) if p == &calc()));
    pump_until_state(&mut s, &mut ev, |_| requests(&log).len() > before);
    let last = requests(&log).pop().unwrap();
    assert_eq!(last["command"], "setBreakpoints");
    assert_eq!(last["arguments"]["source"]["path"], "/w/Calc.cs");
    assert_eq!(last["arguments"]["breakpoints"], json!([]));

    // Run to the end.
    s.continue_();
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::Terminated));
    assert!(ev.contains(&DebugEvent::Exited(3)));
    assert!(ev.contains(&DebugEvent::Output { category: OutputCategory::Stdout, text: "bye\n".into() }));
    assert_eq!(s.state(), &SessionState::Exited(3));
    let start = Instant::now();
    while !commands(&log).contains(&"disconnect".to_string()) && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(commands(&log).last().unwrap(), "disconnect");
    assert_eq!(requests(&log).last().unwrap()["arguments"]["terminateDebuggee"], true);
}

#[test]
fn hex_is_converted_here_when_the_adapter_cannot_format() {
    let dir = tempfile::tempdir().unwrap();
    let (adapter, log) = fake(dir.path(), "late");
    let mut bps = Breakpoints::new();
    bps.set(BreakpointSpec::new(calc(), 9));
    let mut s = DebugSession::start(adapter, launch(), DebugOptions::default(), bps, Waker::noop());
    let mut ev = Vec::new();
    pump_until(&mut s, &mut ev, stopped);
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::ScopesLoaded { frame_id: 10 }));
    let locals = s.scopes(10).unwrap()[0].variables_reference;
    s.request_variables(locals);
    let w = s.add_watch("x * 10");
    pump_until_state(&mut s, &mut ev, |s| s.variables(locals).is_some() && settled(s));
    assert_eq!(s.variables(locals).unwrap()[0].value, "1");
    assert_eq!(result_of(watch_value(&s, w)), "10");

    // Turning hex on asks again without restarting; objects stay as they are.
    s.set_hex(true);
    assert!(s.hex());
    assert!(s.variables(locals).is_none());
    s.request_variables(locals);
    pump_until_state(&mut s, &mut ev, |s| s.variables(locals).is_some() && settled(s));
    let vars = s.variables(locals).unwrap();
    assert_eq!((vars[0].value.as_str(), vars[1].value.as_str()), ("0x1", "{Order}"));
    assert_eq!(result_of(watch_value(&s, w)), "0xa");
    let id = s.evaluate("x", EvalContext::Hover);
    let mut hover = None;
    pump_until(&mut s, &mut ev, |e, _| match e {
        DebugEvent::Evaluated { id: got, result, .. } if *got == id => {
            hover = result.clone().ok().map(|v| v.result);
            true
        }
        _ => false,
    });
    assert_eq!(hover.as_deref(), Some("0x1"));
    // The fake adapter does not advertise value formatting: no `format` on the wire.
    assert!(requests(&log).iter().all(|r| r["arguments"].get("format").is_none()));

    s.set_hex(false);
    s.request_variables(locals);
    pump_until_state(&mut s, &mut ev, |s| s.variables(locals).is_some() && settled(s));
    assert_eq!(s.variables(locals).unwrap()[0].value, "1");
    assert_eq!(result_of(watch_value(&s, w)), "10");
}

/// Pumps until the set `id` answers.
fn set_result(s: &mut DebugSession, ev: &mut Vec<DebugEvent>, id: u64) -> Result<String, String> {
    let mut got = None;
    pump_until(s, ev, |e, _| match e {
        DebugEvent::VariableSet { id: i, result, .. } if *i == id => {
            got = Some(result.clone());
            true
        }
        _ => false,
    });
    got.unwrap()
}

#[test]
fn the_set_method_follows_the_capabilities() {
    let caps = |var: bool, expr: bool| Capabilities { supports_set_variable: var, supports_set_expression: expr, ..Default::default() };
    assert_eq!(set_method(&caps(true, true), 1000, Some("x")), Some(SetMethod::SetVariable));
    assert_eq!(set_method(&caps(true, false), 1000, None), Some(SetMethod::SetVariable));
    // A watch has no parent reference: only setExpression can change it.
    assert_eq!(set_method(&caps(true, true), 0, Some("x + 0")), Some(SetMethod::SetExpression));
    assert_eq!(set_method(&caps(true, false), 0, Some("x")), None);
    assert_eq!(set_method(&caps(false, true), 1000, Some("order.Customer")), Some(SetMethod::SetExpression));
    assert_eq!(set_method(&caps(false, true), 1000, None), None);
    assert_eq!(set_method(&caps(false, true), 1000, Some("  ")), None);
    assert_eq!(set_method(&caps(false, false), 1000, Some("x")), None);
}

#[test]
fn set_value_uses_set_expression_without_set_variable_and_reports_errors() {
    let dir = tempfile::tempdir().unwrap();
    let (adapter, log) = fake(dir.path(), "late-noset");
    let mut bps = Breakpoints::new();
    bps.set(BreakpointSpec::new(calc(), 9));
    let mut s = DebugSession::start(adapter, launch(), DebugOptions::default(), bps, Waker::noop());
    let mut ev = Vec::new();
    // Not paused yet: refused without asking the adapter.
    let early = s.set_variable(1001, "x", Some("x"), "5");
    assert!(set_result(&mut s, &mut ev, early).is_err());
    pump_until(&mut s, &mut ev, stopped);
    assert!(!s.capabilities().supports_set_variable && s.capabilities().supports_set_expression);
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::ScopesLoaded { frame_id: 10 }));
    let locals = s.scopes(10).unwrap()[0].variables_reference;
    s.request_variables(locals);
    let w = s.add_watch("x * 10");
    pump_until_state(&mut s, &mut ev, |s| s.variables(locals).is_some() && settled(s));

    // The adapter has no setVariable: setExpression on the expression, in the selected frame.
    let id = s.set_variable(locals, "x", Some("x"), "5");
    assert_eq!(set_result(&mut s, &mut ev, id), Ok("5".into()));
    pump_until_state(&mut s, &mut ev, settled);
    assert_eq!(s.variables(locals).unwrap()[0].value, "5");
    assert_eq!(result_of(watch_value(&s, w)), "50");
    let req = requests(&log).into_iter().find(|r| r["command"] == "setExpression").unwrap();
    assert_eq!(req["arguments"], json!({"expression": "x", "value": "5", "frameId": 10}));
    assert!(!commands(&log).contains(&"setVariable".to_string()));

    // A hex value typed by the user; shown back in hex when hex is on (converted here).
    s.set_hex(true);
    s.request_variables(locals);
    pump_until_state(&mut s, &mut ev, |s| s.variables(locals).is_some() && settled(s));
    let id = s.set_expression("x", "0x1f");
    assert_eq!(set_result(&mut s, &mut ev, id), Ok("0x1f".into()));
    pump_until_state(&mut s, &mut ev, settled);
    assert_eq!(s.variables(locals).unwrap()[0].value, "0x1f");
    assert_eq!(result_of(watch_value(&s, w)), "0x136");
    s.set_hex(false);

    // The adapter's error comes back as it is; the cached value does not change.
    let id = s.set_variable(locals, "x", Some("x"), "abc");
    assert_eq!(set_result(&mut s, &mut ev, id), Err("cannot assign 'abc'".into()));
    s.request_variables(locals);
    pump_until_state(&mut s, &mut ev, |s| s.variables(locals).is_some() && settled(s));
    assert_eq!(s.variables(locals).unwrap()[0].value, "31");
    // A child with no expression cannot be changed by this adapter.
    let id = s.set_variable(2001, "Customer", None, "\"Bob\"");
    assert!(set_result(&mut s, &mut ev, id).is_err());

    // An answer that arrives after the program continued is not put in the new stop.
    let id = s.set_variable(locals, "x", Some("x"), "9");
    s.continue_();
    assert_eq!(set_result(&mut s, &mut ev, id), Ok("9".into()));
    assert!(s.variables(locals).is_none());
    s.stop();
}

/// Pumps until the memory read `id` answers.
fn memory_read(s: &mut DebugSession, ev: &mut Vec<DebugEvent>, id: u64) -> Result<crate::memory::MemoryBlock, String> {
    let mut got = None;
    pump_until(s, ev, |e, _| match e {
        DebugEvent::MemoryRead { id: i, result } if *i == id => {
            got = Some(result.clone());
            true
        }
        _ => false,
    });
    got.unwrap()
}

#[test]
fn memory_is_read_decoded_and_partly_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let (adapter, log) = fake(dir.path(), "late");
    let mut bps = Breakpoints::new();
    bps.set(BreakpointSpec::new(calc(), 9));
    let mut s = DebugSession::start(adapter, launch(), DebugOptions::default(), bps, Waker::noop());
    let mut ev = Vec::new();
    // Not stopped yet: refused without asking the adapter.
    let early = s.read_memory("0x1000", 0, 8);
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::MemoryRead { id, result: Err(_) } if *id == early));
    pump_until(&mut s, &mut ev, stopped);
    assert!(s.capabilities().supports_read_memory_request);
    assert_eq!(requests(&log)[0]["arguments"]["supportsMemoryReferences"], true);

    let id = s.read_memory("0x1000", 0, 12);
    let block = memory_read(&mut s, &mut ev, id).unwrap();
    assert_eq!(block.address, 0x1000);
    assert_eq!(block.bytes[..4], [Some(0xde), Some(0xad), Some(0xbe), Some(0xef)]);
    assert_eq!(block.bytes[7], Some(1));
    // Past the readable bytes: `unreadableBytes`.
    assert_eq!(block.bytes[8..], [None, None, None, None]);
    let req = requests(&log).into_iter().find(|r| r["command"] == "readMemory").unwrap();
    assert_eq!((req["arguments"]["memoryReference"].as_str(), req["arguments"]["offset"].as_i64(), req["arguments"]["count"].as_i64()), (Some("0x1000"), Some(0), Some(12)));

    // Before the readable bytes: the answer starts later; a huge count is cut.
    let id = s.read_memory("0x1000", -2, 1 << 30);
    let block = memory_read(&mut s, &mut ev, id).unwrap();
    assert_eq!(block.address, 0xffe);
    assert_eq!(block.bytes.len(), crate::memory::MAX_READ);
    assert_eq!(block.bytes[..3], [None, None, Some(0xde)]);

    // The next stop changes the last byte (x = 2).
    s.continue_();
    pump_until(&mut s, &mut ev, stopped);
    let id = s.read_memory("0x1000", 0, 8);
    let again = memory_read(&mut s, &mut ev, id).unwrap();
    let first = crate::memory::MemoryBlock { address: 0x1000, bytes: vec![Some(0xde), Some(0xad), Some(0xbe), Some(0xef), Some(0), Some(0), Some(0), Some(1)] };
    assert_eq!(again.changed_since(Some(&first)), [false, false, false, false, false, false, false, true]);
    s.stop();
}

/// Pumps until the disassembly `id` answers.
fn disassembled(s: &mut DebugSession, ev: &mut Vec<DebugEvent>, id: u64) -> Result<Vec<crate::disassembly::Instruction>, String> {
    let mut got = None;
    pump_until(s, ev, |e, _| match e {
        DebugEvent::Disassembled { id: i, result } if *i == id => {
            got = Some(result.clone());
            true
        }
        _ => false,
    });
    got.unwrap()
}

#[test]
fn code_is_disassembled_around_the_frame_and_stepped_by_instruction() {
    use crate::disassembly::{instructions_before, rows, BACK_BYTES, DisasmRow};
    let dir = tempfile::tempdir().unwrap();
    let (adapter, log) = fake(dir.path(), "late");
    let mut bps = Breakpoints::new();
    bps.set(BreakpointSpec::new(calc(), 9));
    let mut s = DebugSession::start(adapter, launch(), DebugOptions::default(), bps, Waker::noop());
    let mut ev = Vec::new();
    let early = s.disassemble("0x2000", 0, 0, 4);
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::Disassembled { id, result: Err(_) } if *id == early));
    pump_until(&mut s, &mut ev, stopped);
    pump_until_state(&mut s, &mut ev, |s| s.selected_frame().is_some());
    assert!(s.capabilities().supports_disassemble_request && s.capabilities().supports_stepping_granularity);
    let ip = s.selected_frame().and_then(|f| f.instruction_pointer_reference.clone()).unwrap();
    assert_eq!(ip, "0x2010");

    // Below: from the instruction pointer on; addresses past the code are dropped.
    let id = s.disassemble(&ip, 0, 0, 1000);
    let below = disassembled(&mut s, &mut ev, id).unwrap();
    assert_eq!(below.len(), (0x2100 - 0x2010) / 4);
    assert_eq!((below[0].address, below[0].text.as_str(), below[0].bytes.as_deref()), (0x2010, "op 4", Some("00 00 20 10")));
    let req = requests(&log).into_iter().find(|r| r["command"] == "disassemble" && r["arguments"]["instructionCount"] == 1000).unwrap();
    assert_eq!(req["arguments"]["resolveSymbols"], true);
    assert_eq!(req["arguments"]["memoryReference"], "0x2010");

    // Above: read from bytes before the anchor and kept when the decoding lands on it.
    let id = s.disassemble(&ip, -BACK_BYTES, 0, 64);
    let back = disassembled(&mut s, &mut ev, id).unwrap();
    let above = instructions_before(&back, 0x2010).unwrap();
    // The first three decoded (0x2000..0x2008) may be out of step and are dropped.
    assert_eq!(above.iter().map(|i| i.address).collect::<Vec<_>>(), [0x200c]);
    // One byte off: nothing lands on an instruction.
    let id = s.disassemble(&ip, -BACK_BYTES + 1, 0, 64);
    assert!(disassembled(&mut s, &mut ev, id).is_err());

    // The whole function from its symbol: headers for the symbol and each line.
    let id = s.disassemble("0x2000", 0, 0, 8);
    let all = disassembled(&mut s, &mut ev, id).unwrap();
    let r = rows(&all, Some(0x2010));
    assert_eq!(r[0], DisasmRow::Header { symbol: Some("Calc".into()), location: Some("/w/Calc.cs:9".into()) });
    assert!(r.contains(&DisasmRow::Instruction { index: 4, current: true }));
    assert_eq!(r.iter().filter(|r| matches!(r, DisasmRow::Header { .. })).count(), 2);

    // Step Over Instruction: `next` by instruction; the frame moves to the next one.
    s.step_instruction(true);
    pump_until(&mut s, &mut ev, stopped);
    pump_until_state(&mut s, &mut ev, |s| s.selected_frame().is_some());
    assert_eq!(s.selected_frame().and_then(|f| f.instruction_pointer_reference.as_deref()), Some("0x2014"));
    s.step_instruction(false);
    pump_until(&mut s, &mut ev, stopped);
    let steps: Vec<Value> = requests(&log).into_iter().filter(|r| r["arguments"]["granularity"] == "instruction").collect();
    assert_eq!(steps.iter().map(|r| r["command"].as_str().unwrap()).collect::<Vec<_>>(), ["next", "stepIn"]);
    s.stop();
}

/// The `setBreakpoints` requests sent after the `index`-th request, as (file, lines).
fn breakpoint_sets(log: &Path, from: usize) -> Vec<(String, Vec<u64>)> {
    requests(log)[from..]
        .iter()
        .filter(|r| r["command"] == "setBreakpoints")
        .map(|r| (r["arguments"]["source"]["path"].as_str().unwrap().to_string(), r["arguments"]["breakpoints"].as_array().unwrap().iter().map(|b| b["line"].as_u64().unwrap()).collect()))
        .collect()
}

#[test]
fn forced_steps_take_the_breakpoints_away_until_the_program_stops() {
    let dir = tempfile::tempdir().unwrap();
    let (adapter, log) = fake(dir.path(), "late");
    let mut bps = Breakpoints::new();
    bps.set(BreakpointSpec::new(calc(), 9));
    bps.set(BreakpointSpec::new("/w/Other.cs", 3));
    let mut s = DebugSession::start(adapter, launch(), DebugOptions::default(), bps, Waker::noop());
    let mut ev = Vec::new();
    pump_until(&mut s, &mut ev, stopped);
    let other = |sets: &[(String, Vec<u64>)], file: &str| sets.iter().filter(|(f, _)| f == file).map(|(_, l)| l.clone()).collect::<Vec<_>>();

    // Force Step Over: every file is sent empty before `next`, then sent again at the stop.
    let before = commands(&log).len();
    s.force_step_over();
    assert!(s.ignoring_breakpoints());
    pump_until(&mut s, &mut ev, stopped);
    assert!(!s.ignoring_breakpoints());
    pump_until_state(&mut s, &mut ev, |_| breakpoint_sets(&log, before).len() >= 4);
    let after: Vec<String> = commands(&log)[before..].to_vec();
    let next = after.iter().position(|c| c == "next").unwrap();
    assert_eq!(after[..next].iter().filter(|c| *c == "setBreakpoints").count(), 2);
    let sets = breakpoint_sets(&log, before);
    assert_eq!(other(&sets, "/w/Calc.cs"), [vec![], vec![9]]);
    assert_eq!(other(&sets, "/w/Other.cs"), [vec![], vec![3]]);
    // The user's breakpoints never changed.
    assert!(s.breakpoints().get(&calc(), 9).is_some_and(|b| b.spec.enabled));

    // Force Run to Cursor: only the target line stays, in its file; an edit on the way stays
    // out of the adapter too.
    let before = commands(&log).len();
    s.force_continue_to(BreakpointSpec::new(calc(), 15));
    s.set_breakpoint(BreakpointSpec::new("/w/Other.cs", 7));
    pump_until(&mut s, &mut ev, stopped);
    pump_until_state(&mut s, &mut ev, |_| breakpoint_sets(&log, before).len() >= 6);
    let sets = breakpoint_sets(&log, before);
    assert_eq!(other(&sets, "/w/Calc.cs"), [vec![], vec![15], vec![9, 15]]);
    assert_eq!(other(&sets, "/w/Other.cs"), [vec![], vec![], vec![3, 7]]);
    let continued = commands(&log)[before..].iter().position(|c| c == "continue").unwrap();
    assert_eq!(commands(&log)[before..][..continued].iter().filter(|c| *c == "setBreakpoints").count(), 3);

    // Force Step Into needs GDB's skip: another adapter does a plain nothing.
    assert!(!s.can_force_step_in());
    let before = commands(&log).len();
    s.force_step_in();
    assert!(s.state().is_stopped());
    assert_eq!(commands(&log).len(), before);

    // Ending in the middle of a forced step leaves nothing pending.
    s.force_step_over();
    s.stop();
    assert!(!s.ignoring_breakpoints());
}

#[test]
fn logpoints_and_hit_conditions_are_emulated() {
    let dir = tempfile::tempdir().unwrap();
    let (adapter, log) = fake(dir.path(), "early");
    let mut bps = Breakpoints::new();
    bps.set(BreakpointSpec::new(calc(), 10).with_hit_condition(">= 2").with_log_message("x={x}!"));
    let mut s = DebugSession::start(adapter, launch(), DebugOptions::default(), bps, Waker::noop());
    let mut ev = Vec::new();
    pump_until(&mut s, &mut ev, |e, _| matches!(e, DebugEvent::Terminated));
    assert_eq!(commands(&log)[..6], ["initialize", "launch", "setBreakpoints", "setExceptionBreakpoints", "configurationDone", "threads"]);
    // The adapter lacks both capabilities: neither goes on the wire.
    assert_eq!(requests(&log)[2]["arguments"]["breakpoints"], json!([{"line": 10}]));
    assert!(!ev.iter().any(|e| matches!(e, DebugEvent::Stopped(_))));
    let console: Vec<&str> = ev.iter().filter_map(|e| if let DebugEvent::Output { category: OutputCategory::Console, text } = e { Some(text.as_str()) } else { None }).collect();
    assert_eq!(console, ["x=2!\n", "x=3!\n"]);
    assert_eq!(s.state(), &SessionState::Exited(3));
}

#[test]
fn hit_condition_stops_on_the_nth_hit_and_stop_disconnects() {
    let dir = tempfile::tempdir().unwrap();
    let (adapter, log) = fake(dir.path(), "early");
    let mut bps = Breakpoints::new();
    bps.set(BreakpointSpec::new(calc(), 10).with_hit_condition("3"));
    let mut s = DebugSession::start(adapter, launch(), DebugOptions::default(), bps, Waker::noop());
    let w = s.add_watch("x");
    assert_eq!(watch_value(&s, w), WatchValue::NotEvaluated);
    let mut ev = Vec::new();
    pump_until(&mut s, &mut ev, stopped);
    pump_until_state(&mut s, &mut ev, settled);
    assert_eq!(result_of(watch_value(&s, w)), "3");
    assert_eq!(ev.iter().filter(|e| matches!(e, DebugEvent::Stopped(_))).count(), 1);

    s.stop();
    assert_eq!(s.state(), &SessionState::Terminated);
    let start = Instant::now();
    while !commands(&log).contains(&"disconnect".to_string()) && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(commands(&log).contains(&"disconnect".to_string()));
    assert!(s.pump().contains(&DebugEvent::Terminated));
    // Later messages do not revive an ended session.
    std::thread::sleep(Duration::from_millis(100));
    assert!(s.pump().iter().all(|e| !matches!(e, DebugEvent::StateChanged(_) | DebugEvent::Stopped(_))));
}

#[test]
fn a_missing_adapter_fails_the_session() {
    let adapter = AdapterCommand::new("/nonexistent/netcoredbg", Origin::Settings);
    let mut s = DebugSession::start(adapter, launch(), DebugOptions::default(), Breakpoints::new(), Waker::noop());
    let ev = s.pump();
    assert!(matches!(s.state(), SessionState::Failed(Failure::AdapterNotStarted(_))));
    assert!(ev.iter().any(|e| matches!(e, DebugEvent::Failed(_))));
    let id = s.evaluate("x", EvalContext::Repl);
    assert!(matches!(&s.pump()[..], [DebugEvent::Evaluated { id: got, result: Err(_), .. }] if *got == id));
}

#[test]
fn failure_messages_have_templates() {
    let f = Failure::AttachFailed { pid: 42, error: "denied".into() };
    assert_eq!(f.to_string(), "The debugger could not attach to process 42: denied");
    assert_eq!(StopReason::parse("exception").label(), "Paused on exception");
}

#[test]
fn reads_the_gdb_version() {
    assert_eq!(gdb_major(&json!({ "result": "15", "variablesReference": 0 })), Some(15));
    assert_eq!(gdb_major(&json!({ "result": " 16\n" })), Some(16));
    assert_eq!(gdb_major(&json!({ "result": "void" })), None);
    assert_eq!(gdb_major(&json!({})), None);
}

#[test]
fn gdb_jumps_only_where_it_was_checked() {
    let c = gdb_jump(Path::new("/w/src/main.rs"), 6).unwrap();
    assert!(c.starts_with("python exec(\""));
    assert!(c.contains(r#"gdb.decode_line(\"\\\"/w/src/main.rs\\\":6\")"#), "{c}");
    assert!(c.contains("sals[0].line != 6"));
    assert!(gdb_jump(Path::new("/w/a\"b.rs"), 1).is_none());
    assert_eq!(goto_error("Python Exception: crabcraft-goto: other function".into()), GotoError::OtherFunction);
    assert_eq!(goto_error("crabcraft-goto: no code".into()), GotoError::NoCode);
    assert_eq!(goto_error("boom".into()), GotoError::Adapter("boom".into()));
}

#[test]
fn parallel_stack_responses_are_bounded_and_stale_generations_are_discarded() {
    let dir = tempfile::tempdir().unwrap();
    let (adapter, _) = fake(dir.path(), "normal");
    let mut session = DebugSession::start(adapter, launch(), DebugOptions::default(), Breakpoints::new(), Waker::noop());
    session.state = SessionState::Stopped(StopInfo { thread_id: 1, reason: StopReason::Pause, description: None, text: None, frame_id: None, breakpoints: Vec::new() });
    let frames: Vec<_> = (0..100).map(|id| json!({"id": id, "name": "é".repeat(4000), "line": 10, "moduleId": "x".repeat(3000), "source": {"path": "x".repeat(10000)}})).collect();
    let response = json!({"stackFrames": frames, "totalFrames": 100});
    let generation = session.generation;
    session.on_response(Ok(response.clone()), Purpose::ParallelStack { id: 1, generation: generation + 1, thread_id: 1, limit: 64 });
    assert!(!session.events.iter().any(|event| matches!(event, DebugEvent::ParallelStackLoaded { .. })));
    session.on_response(Ok(response), Purpose::ParallelStack { id: 2, generation, thread_id: 1, limit: 64 });
    let stack = session.events.iter().find_map(|event| if let DebugEvent::ParallelStackLoaded { result: Ok(stack), .. } = event { Some(stack) } else { None }).unwrap();
    assert_eq!(stack.frames.len(), 64);
    assert!(stack.truncated);
    assert!(stack.frames.iter().all(|frame| frame.name.len() <= 2048 && frame.module_id.is_none() && frame.source.is_none()));
    session.stop();
}
