//! Build, run and test through Cargo, reported with the events of the Build, Run and Tests windows.

use crate::messages::{Message, parse_message};
use crate::test_output::TestParser;
use crate::toolchain::Cargo;
use rc_core::Runtime;
use rc_core::inbox::Outbox;
use rc_dotnet::process::{StreamOpts, run_streaming};
use rc_dotnet::run::STOP_GRACE;
use rc_dotnet::test::TestProgress;
use rc_dotnet::{BuildAction, BuildEvent, BuildResult, CancelToken, Diagnostic, LaunchPlan, OpHandle, OutputLine, RunEvent, RunHandle, Severity, Stream, TestEvent, TestFilter, TestOutcome, TestRequest, TestRunResult};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const MESSAGE_FORMAT: &str = "--message-format=json-diagnostic-rendered-ansi";

/// Starts a Cargo process with coloured output and no progress bar (it rewrites lines).
fn cargo_command(cargo: &Cargo, dir: Option<&Path>, args: &[String]) -> tokio::process::Command {
    let mut cmd = cargo.command();
    cmd.args(args).env("CARGO_TERM_COLOR", "always").env("CARGO_TERM_PROGRESS_WHEN", "never");
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    cmd
}

fn folder(manifest: &Path) -> Option<&Path> {
    manifest.parent().filter(|p| !p.as_os_str().is_empty())
}

fn manifest_arg(manifest: &Path) -> Vec<String> {
    vec!["--manifest-path".into(), manifest.to_string_lossy().into_owned()]
}

/// Arguments after `cargo`.
pub fn build_args(manifest: &Path, action: BuildAction) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    match action {
        BuildAction::Clean => a.push("clean".into()),
        BuildAction::Restore => a.push("fetch".into()),
        BuildAction::Build | BuildAction::Rebuild => a.extend(["build".into(), "--workspace".into(), "--all-targets".into(), MESSAGE_FORMAT.into()]),
    }
    a.extend(manifest_arg(manifest));
    a
}

/// What one Cargo invocation left behind.
#[derive(Default)]
struct Collected {
    diagnostics: Vec<Diagnostic>,
    executables: Vec<PathBuf>,
}

impl Collected {
    /// Handles one line of Cargo's output, reporting what the user should see.
    fn line(&mut self, line: OutputLine, root: &Path, on_event: &mut (dyn FnMut(BuildEvent) + Send)) {
        let message = (line.stream == Stream::Stdout).then(|| parse_message(&line.text, root)).flatten();
        match message {
            None => on_event(BuildEvent::Output(line)),
            Some(Message::Compiler { diagnostic, rendered }) => {
                if !rendered.is_empty() {
                    on_event(BuildEvent::Output(OutputLine { stream: Stream::Stdout, text: rendered, newline: true }));
                }
                if let Some(d) = diagnostic.map(|d| *d)
                    && !self.diagnostics.contains(&d)
                {
                    self.diagnostics.push(d.clone());
                    on_event(BuildEvent::Diagnostic(d));
                }
            }
            Some(Message::Artifact { executable }) => self.executables.extend(executable),
            Some(Message::BuildFinished { .. } | Message::Other) => {}
        }
    }

    fn result(self, code: Option<i32>, cancelled: bool, duration: Duration) -> (BuildResult, Vec<PathBuf>) {
        let errors = self.diagnostics.iter().filter(|d| d.severity == Severity::Error).count();
        let warnings = self.diagnostics.iter().filter(|d| d.severity == Severity::Warning).count();
        (BuildResult { success: !cancelled && code == Some(0), cancelled, exit_code: code, errors, warnings, diagnostics: self.diagnostics, duration }, self.executables)
    }
}

/// Runs one Cargo command to its end.
async fn invoke(cargo: &Cargo, manifest: &Path, args: &[String], cancel: &CancelToken, on_event: &mut (dyn FnMut(BuildEvent) + Send)) -> anyhow::Result<(BuildResult, Vec<PathBuf>)> {
    let started = Instant::now();
    let root = folder(manifest).unwrap_or(Path::new(".")).to_path_buf();
    on_event(BuildEvent::Started { command: format!("cargo {}", args.join(" ")) });
    let mut collected = Collected::default();
    let cmd = cargo_command(cargo, folder(manifest), args);
    let outcome = {
        let collected = &mut collected;
        let events = &mut *on_event;
        let mut on_line = |line: OutputLine| collected.line(line, &root, events);
        run_streaming(cmd, cancel, StreamOpts::default(), &mut |_| {}, &mut on_line).await?
    };
    Ok(collected.result(outcome.code, outcome.cancelled, started.elapsed()))
}

/// Starts `action` on the workspace of `manifest`.
pub fn start_build(cargo: &Cargo, manifest: &Path, action: BuildAction, outbox: Outbox<BuildEvent>) -> OpHandle {
    let cancel = CancelToken::new();
    let token = cancel.clone();
    let (cargo, manifest) = (cargo.clone(), manifest.to_path_buf());
    let task = Runtime::spawn(async move {
        let out = outbox.clone();
        let mut report = |e| {
            out.send(e);
        };
        if action == BuildAction::Rebuild {
            match invoke(&cargo, &manifest, &build_args(&manifest, BuildAction::Clean), &token, &mut report).await {
                Ok((r, _)) if r.success => {}
                Ok((r, _)) => {
                    outbox.send(BuildEvent::Finished(r));
                    return;
                }
                Err(e) => {
                    outbox.send(BuildEvent::Failed(format!("{e:#}")));
                    return;
                }
            }
        }
        match invoke(&cargo, &manifest, &build_args(&manifest, action), &token, &mut report).await {
            Ok((r, _)) => outbox.send(BuildEvent::Finished(r)),
            Err(e) => outbox.send(BuildEvent::Failed(format!("{e:#}"))),
        };
    });
    OpHandle::new(cancel, task)
}

/// A binary to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunTarget {
    /// The workspace `Cargo.toml`.
    pub manifest: PathBuf,
    pub package: String,
    pub bin: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The workspace root when `None`.
    pub working_dir: Option<PathBuf>,
    /// Cargo arguments of a test executable to build, instead of the binary `bin`.
    pub test: Option<Vec<String>>,
}

/// Arguments that build just this binary.
pub fn run_build_args(t: &RunTarget) -> Vec<String> {
    if let Some(selection) = &t.test {
        let mut a: Vec<String> = vec!["test".into(), "--no-run".into()];
        a.extend(selection.iter().cloned());
        a.push(MESSAGE_FORMAT.into());
        a.extend(manifest_arg(&t.manifest));
        return a;
    }
    let mut a: Vec<String> = vec!["build".into(), "--package".into(), t.package.clone(), "--bin".into(), t.bin.clone(), MESSAGE_FORMAT.into()];
    a.extend(manifest_arg(&t.manifest));
    a
}

/// Builds the binary and starts it directly (not through `cargo run`), so its process id, program
/// path and arguments are known, as a debugger needs them.
pub fn start_run(cargo: &Cargo, target: RunTarget, outbox: Outbox<RunEvent>) -> RunHandle {
    let cancel = CancelToken::new();
    let token = cancel.clone();
    let cargo = cargo.clone();
    let (input, input_rx) = mpsc::unbounded_channel();
    let pid = Arc::new(AtomicU32::new(0));
    let pid2 = pid.clone();
    let task = Runtime::spawn(async move {
        let out = outbox.clone();
        let end = run(&cargo, &target, &token, input_rx, &pid2, &mut |e| {
            out.send(e);
        })
        .await;
        outbox.send(end.unwrap_or_else(|e| RunEvent::Failed(format!("{e:#}"))));
    });
    RunHandle::new(OpHandle::new(cancel, task), input, pid)
}

/// Builds the binary and resolves what to start. `Ok(None)` when cancelled.
pub async fn prepare(cargo: &Cargo, t: &RunTarget, cancel: &CancelToken, on_build: &mut (dyn FnMut(BuildEvent) + Send)) -> anyhow::Result<Option<LaunchPlan>> {
    let (result, executables) = invoke(cargo, &t.manifest, &run_build_args(t), cancel, &mut *on_build).await?;
    on_build(BuildEvent::Finished(result.clone()));
    if result.cancelled {
        return Ok(None);
    }
    anyhow::ensure!(result.success, "build failed with {} error(s)", result.errors);
    let Some(program) = executables.into_iter().last() else {
        anyhow::bail!("cargo did not report an executable for {}", t.bin);
    };
    let working_dir = t.working_dir.clone().or_else(|| folder(&t.manifest).map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
    let mut env = vec![("RUST_BACKTRACE".to_string(), "short".to_string())];
    env.extend(t.env.iter().cloned());
    Ok(Some(LaunchPlan { program, args: t.args.clone(), env, working_dir, urls: Vec::new(), profile: None, browser_url: None }))
}

async fn run(cargo: &Cargo, t: &RunTarget, cancel: &CancelToken, input: mpsc::UnboundedReceiver<Vec<u8>>, pid: &AtomicU32, on_event: &mut (dyn FnMut(RunEvent) + Send)) -> anyhow::Result<RunEvent> {
    let prepared = prepare(cargo, t, cancel, &mut |e| on_event(RunEvent::Build(e))).await;
    let plan = match prepared {
        Ok(Some(plan)) => plan,
        Ok(None) => return Ok(RunEvent::Exited { code: None, cancelled: true, duration: Duration::ZERO }),
        Err(e) => return Ok(RunEvent::Failed(format!("{e:#}"))),
    };

    let mut cmd = tokio::process::Command::new(&plan.program);
    cmd.args(&plan.args).current_dir(&plan.working_dir);
    for (k, v) in &plan.env {
        cmd.env(k, v);
    }
    let started = Instant::now();
    let events = std::sync::Mutex::new(on_event);
    let report = |e: RunEvent| {
        if let Ok(mut f) = events.lock() {
            f(e);
        }
    };
    let opts = StreamOpts { partial_lines: true, grace: STOP_GRACE, stdin: Some(input) };
    let outcome = run_streaming(
        cmd,
        cancel,
        opts,
        &mut |p| {
            pid.store(p, Ordering::SeqCst);
            report(RunEvent::Started { pid: p, plan: plan.clone() });
        },
        &mut |line| report(RunEvent::Output(line)),
    )
    .await?;
    Ok(RunEvent::Exited { code: outcome.code, cancelled: outcome.cancelled, duration: started.elapsed() })
}

/// Arguments after `cargo` for a test run. `--no-fail-fast` runs every suite even when one fails.
pub fn test_args(req: &TestRequest) -> Vec<String> {
    let mut a: Vec<String> = vec!["test".into(), "--workspace".into(), "--no-fail-fast".into(), MESSAGE_FORMAT.into()];
    a.extend(manifest_arg(&req.target));
    a.push("--".into());
    a.extend(["--color".into(), "never".into()]);
    match &req.filter {
        Some(TestFilter::Tests(names)) => {
            a.push("--exact".into());
            a.extend(names.iter().cloned());
        }
        // Tests of a module, which is Rust's nearest thing to a test class.
        Some(TestFilter::Class(module)) => a.push(format!("{module}::")),
        _ => {}
    }
    a
}

/// Runs the tests of the workspace `req.target`.
pub fn start_tests(cargo: &Cargo, req: TestRequest, outbox: Outbox<TestEvent>) -> OpHandle {
    let cancel = CancelToken::new();
    let token = cancel.clone();
    let cargo = cargo.clone();
    let task = Runtime::spawn(async move {
        let out = outbox.clone();
        match tests(&cargo, &req, &token, &mut |e| {
            out.send(e);
        })
        .await
        {
            Ok(r) => outbox.send(TestEvent::Finished(r)),
            Err(e) => outbox.send(TestEvent::Failed(format!("{e:#}"))),
        };
    });
    OpHandle::new(cancel, task)
}

async fn tests(cargo: &Cargo, req: &TestRequest, cancel: &CancelToken, on_event: &mut (dyn FnMut(TestEvent) + Send)) -> anyhow::Result<TestRunResult> {
    let started = Instant::now();
    let args = test_args(req);
    let root = folder(&req.target).unwrap_or(Path::new(".")).to_path_buf();
    on_event(TestEvent::ProjectStarted { project: req.target.clone() });
    let mut collected = Collected::default();
    let mut parser = TestParser::new();
    let cmd = cargo_command(cargo, folder(&req.target), &args);
    let outcome = {
        let (collected, parser) = (&mut collected, &mut parser);
        let events = &mut *on_event;
        let project = req.target.clone();
        let mut on_line = |line: OutputLine| {
            let is_message = line.stream == Stream::Stdout && parse_message(&line.text, &root).is_some();
            if is_message {
                collected.line(line, &root, &mut |e| match e {
                    BuildEvent::Output(l) => events(TestEvent::Output { project: project.clone(), line: l }),
                    other => events(TestEvent::Build(other)),
                });
                return;
            }
            let clean = rc_dotnet::strip_ansi(&line.text);
            if let Some(f) = parser.line(&clean) {
                events(TestEvent::Progress(TestProgress { project: project.clone(), outcome: f.outcome, name: f.name, duration: None }));
            }
            events(TestEvent::Output { project: project.clone(), line });
        };
        run_streaming(cmd, cancel, StreamOpts::default(), &mut |_| {}, &mut on_line).await?
    };
    let results = parser.finish(&root);
    let count = |o: TestOutcome| results.iter().filter(|r| r.outcome == o).count();
    let (passed, failed, skipped) = (count(TestOutcome::Passed), count(TestOutcome::Failed), count(TestOutcome::Skipped));
    let (build, _) = collected.result(outcome.code, outcome.cancelled, started.elapsed());
    // A failure with no failed test means the code did not compile.
    if outcome.code != Some(0) && failed == 0 && !outcome.cancelled {
        let mut failed_build = build.clone();
        failed_build.success = false;
        on_event(TestEvent::Build(BuildEvent::Finished(failed_build)));
    }
    Ok(TestRunResult {
        passed,
        failed,
        skipped,
        success: !outcome.cancelled && outcome.code == Some(0) && failed == 0,
        cancelled: outcome.cancelled,
        build: Some(build),
        projects: Vec::new(),
        results,
        results_dir: root,
        duration: started.elapsed(),
    })
}

#[cfg(test)]
mod tests_args {
    use super::*;

    #[test]
    fn build_actions_map_to_cargo_commands() {
        let m = Path::new("/w/Cargo.toml");
        assert_eq!(&build_args(m, BuildAction::Build)[..3], ["build", "--workspace", "--all-targets"]);
        assert_eq!(build_args(m, BuildAction::Clean), ["clean", "--manifest-path", "/w/Cargo.toml"]);
        assert_eq!(build_args(m, BuildAction::Restore)[0], "fetch");
    }

    #[test]
    fn test_filters_go_to_libtest() {
        let mut r = TestRequest::new("/w/Cargo.toml");
        assert!(test_args(&r).ends_with(&["--".into(), "--color".into(), "never".into()]));
        r.filter = Some(TestFilter::Tests(vec!["a::b".into(), "c".into()]));
        assert!(test_args(&r).ends_with(&["--exact".into(), "a::b".into(), "c".into()]));
        r.filter = Some(TestFilter::Class("a::tests".into()));
        assert_eq!(test_args(&r).last().unwrap(), "a::tests::");
    }
}
