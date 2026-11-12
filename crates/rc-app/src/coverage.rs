//! Line coverage from cargo llvm-cov and bounded, project-scoped LCOV imports.
use rc_core::{Inbox, Runtime, Waker};
use rc_dotnet::{CancelToken, OpHandle, OutputLine};
use rc_kit::i18n::{tr, trf};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_FILES: usize = 10_000;
const MAX_LINES: usize = 500_000;
const MAX_SUITES: usize = 20;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Limit,
    Format(usize),
    Empty,
    NotRegular,
    Cancelled,
    Failed(Option<i32>),
    Worker(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::NotRegular => f.write_str(tr("Coverage report must be a regular file")),
            Self::Limit => f.write_str(tr("Coverage report exceeds the size, file or line limit")),
            Self::Format(line) => f.write_str(&trf("Invalid LCOV record at line {line}", &[("line", line)])),
            Self::Empty => f.write_str(tr("No covered source files were found inside this project")),
            Self::Cancelled => f.write_str(tr("Coverage cancelled")),
            Self::Failed(code) => f.write_str(&trf("Coverage command failed ({code}); see the console", &[("code", &code.map_or_else(|| "?".into(), |code| code.to_string()))])),
            Self::Worker(error) => f.write_str(error),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error { fn from(error: std::io::Error) -> Self { Self::Io(error) } }

#[derive(Debug)]
pub struct FileCoverage {
    pub path: PathBuf,
    /// One-based source line and execution count. Summary counts are derived from these.
    pub lines: Vec<(usize, u64)>,
}
impl FileCoverage {
    pub fn covered(&self) -> usize { self.lines.iter().filter(|(_, hits)| *hits > 0).count() }
}

#[derive(Debug)]
pub struct Suite {
    pub name: String,
    pub root: PathBuf,
    pub files: Vec<FileCoverage>,
    pub skipped: usize,
}
impl Suite {
    pub fn totals(&self) -> (usize, usize) {
        self.files.iter().fold((0, 0), |(covered, total), file| (covered + file.covered(), total + file.lines.len()))
    }
}

fn read_report(path: &Path, root: &Path, name: String, token: &CancelToken) -> Result<Suite, Error> {
    if token.is_cancelled() { return Err(Error::Cancelled); }
    if !std::fs::metadata(path)?.is_file() { return Err(Error::NotRegular); }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; options.custom_flags(libc::O_NONBLOCK); }
    // Nonblocking open also protects against a regular file being replaced with a FIFO
    // between metadata and open. Always validate the descriptor we actually obtained.
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() { return Err(Error::NotRegular); }
    if metadata.len() > MAX_BYTES as u64 { return Err(Error::Limit); }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        if token.is_cancelled() { return Err(Error::Cancelled); }
        let count = match file.read(&mut chunk) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 { break; }
        if bytes.len() + count > MAX_BYTES { return Err(Error::Limit); }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| Error::Format(1))?;
    parse(text, root, name, token)
}

/// LCOV line data only. Paths are canonicalized before retaining them, so imported reports
/// cannot make the results panel navigate through `..` or a symlink outside the project.
pub fn parse(text: &str, root: &Path, name: String, token: &CancelToken) -> Result<Suite, Error> {
    if text.len() > MAX_BYTES { return Err(Error::Limit); }
    let root = root.canonicalize()?;
    let mut files: BTreeMap<PathBuf, BTreeMap<usize, u64>> = BTreeMap::new();
    let mut source: Option<Option<PathBuf>> = None;
    let (mut entries, mut skipped) = (0usize, 0usize);
    for (index, record) in text.lines().enumerate() {
        if token.is_cancelled() { return Err(Error::Cancelled); }
        let invalid = || Error::Format(index + 1);
        let record = record.trim_end_matches('\r');
        if record.is_empty() || record.starts_with('#') { continue; }
        if let Some(path) = record.strip_prefix("SF:").or_else(|| record.strip_prefix("KF:")) {
            if source.is_some() || path.is_empty() { return Err(invalid()); }
            let path = Path::new(path);
            let resolved = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
            let resolved = resolved.canonicalize().ok().filter(|path| path.starts_with(&root) && path.is_file());
            if resolved.is_none() { skipped = skipped.saturating_add(1); }
            source = Some(resolved);
        } else if record == "end_of_record" {
            if source.take().is_none() { return Err(invalid()); }
        } else if let Some(data) = record.strip_prefix("DA:") {
            let current = source.as_ref().ok_or_else(invalid)?;
            let mut fields = data.split(',');
            let line = fields.next().and_then(|line| line.parse::<usize>().ok()).filter(|line| *line > 0 && *line <= 10_000_000).ok_or_else(invalid)?;
            let hits = fields.next().and_then(|hits| hits.parse::<u64>().ok()).ok_or_else(invalid)?;
            entries += 1;
            if entries > MAX_LINES { return Err(Error::Limit); }
            if let Some(path) = current {
                let lines = files.entry(path.clone()).or_default();
                let count = lines.entry(line).or_default();
                *count = count.saturating_add(hits);
                if files.len() > MAX_FILES { return Err(Error::Limit); }
            }
        } else if record.starts_with("TN:") {
            if source.is_some() { return Err(invalid()); }
        } else if source.is_none() {
            return Err(invalid());
        }
    }
    if source.is_some() { return Err(Error::Format(text.lines().count().max(1))); }
    if files.is_empty() { return Err(Error::Empty); }
    Ok(Suite { name, root, files: files.into_iter().map(|(path, lines)| FileCoverage { path, lines: lines.into_iter().collect() }).collect(), skipped })
}

enum Event { Line(OutputLine), Done(Result<Suite, Error>) }

pub struct Coverage {
    pub show: bool,
    pub suites: Vec<Suite>,
    pub selected: usize,
    pub selected_file: usize,
    pub error: Option<String>,
    pub console: crate::runner::Console,
    inbox: Inbox<Event>,
    job: Option<OpHandle>,
    waker: Waker,
}
impl Coverage {
    pub fn new(waker: Waker) -> Self {
        Self { show: false, suites: Vec::new(), selected: 0, selected_file: 0, error: None, console: crate::runner::Console::new(&waker), inbox: Inbox::new(waker.clone()), job: None, waker }
    }
    pub fn busy(&self) -> bool { self.job.is_some() }
    pub fn cancel(&self) { if let Some(job) = &self.job { job.cancel(); } }
    fn begin(&mut self) -> (rc_core::inbox::Outbox<Event>, CancelToken) {
        self.error = None;
        self.console = crate::runner::Console::new(&self.waker);
        self.inbox = Inbox::new(self.waker.clone());
        (self.inbox.sender(), CancelToken::new())
    }
    pub fn start(&mut self, root: PathBuf, manifest: PathBuf, cargo: PathBuf, env: Vec<(String, String)>) {
        if self.busy() { return; }
        let (out, cancel) = self.begin();
        let token = cancel.clone();
        let task = Runtime::spawn(async move {
            let result = run(root, manifest, cargo, env, token, out.clone()).await;
            out.send(Event::Done(result));
        });
        self.job = Some(OpHandle::new(cancel, task));
    }
    #[cfg(test)]
    pub fn import(&mut self, path: PathBuf, root: PathBuf) {
        if self.busy() { return; }
        let (out, cancel) = self.begin();
        let token = cancel.clone();
        let task = Runtime::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                read_report(&path, &root, name, &token)
            }).await.unwrap_or_else(|error| Err(Error::Worker(error.to_string())));
            out.send(Event::Done(result));
        });
        self.job = Some(OpHandle::new(cancel, task));
    }
    pub fn pick_import(&mut self, root: PathBuf) {
        if self.busy() { return; }
        let (out, cancel) = self.begin();
        let token = cancel.clone();
        let task = Runtime::spawn(async move {
            let picked = tokio::select! {
                file = rfd::AsyncFileDialog::new().add_filter("LCOV", &["info", "lcov"]).pick_file() => file,
                _ = token.cancelled() => None,
            };
            let result = match picked {
                Some(file) if !token.is_cancelled() => tokio::task::spawn_blocking(move || {
                    let path = file.path();
                    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                    read_report(path, &root, name, &token)
                }).await.unwrap_or_else(|error| Err(Error::Worker(error.to_string()))),
                _ => Err(Error::Cancelled),
            };
            out.send(Event::Done(result));
        });
        self.job = Some(OpHandle::new(cancel, task));
    }
    pub fn poll(&mut self) {
        for event in self.inbox.drain() {
            match event {
                Event::Line(line) => self.console.line(&line),
                Event::Done(result) => {
                    self.job = None;
                    match result {
                        Ok(mut suite) => {
                            let base = suite.name.clone();
                            if self.suites.iter().any(|existing| existing.name == suite.name) {
                                for number in 2..=MAX_SUITES + 2 {
                                    let candidate = format!("{base} ({number})");
                                    if self.suites.iter().all(|existing| existing.name != candidate) { suite.name = candidate; break; }
                                }
                            }
                            if self.suites.len() == MAX_SUITES { self.suites.remove(0); }
                            self.suites.push(suite);
                            self.selected = self.suites.len() - 1;
                            self.selected_file = 0;
                        }
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
            }
        }
    }
}

async fn run(root: PathBuf, manifest: PathBuf, cargo: PathBuf, env: Vec<(String, String)>, token: CancelToken, out: rc_core::inbox::Outbox<Event>) -> Result<Suite, Error> {
    let temp = tokio::task::spawn_blocking(|| tempfile::Builder::new().prefix("crabcraft-coverage-").tempdir()).await.map_err(|error| Error::Worker(error.to_string()))??;
    let report = temp.path().join("coverage.info");
    let mut command = tokio::process::Command::new(cargo);
    command.args(["llvm-cov", "--workspace", "--all-targets", "--lcov", "--manifest-path"]).arg(manifest).arg("--output-path").arg(&report);
    command.current_dir(&root).envs(env).env("CARGO_LLVM_COV_SETUP", "no").env("RUSTUP_AUTO_INSTALL", "0");
    let options = rc_dotnet::process::StreamOpts { partial_lines: true, grace: Duration::from_millis(500), stdin: None };
    let status = rc_dotnet::process::run_streaming(command, &token, options, &mut |_| {}, &mut |line| { out.send(Event::Line(line)); }).await?;
    if status.cancelled || token.is_cancelled() { return Err(Error::Cancelled); }
    if status.code != Some(0) { return Err(Error::Failed(status.code)); }
    tokio::task::spawn_blocking(move || {
        let result = read_report(&report, &root, tr("Coverage run").to_string(), &token);
        drop(temp);
        result
    }).await.map_err(|error| Error::Worker(error.to_string()))?
}

pub fn show(app: &mut crate::app::CrabCraftApp, ui: &mut egui::Ui) {
    let busy = app.coverage.busy();
    let cargo = app.runner.cargo_workspace.is_some();
    let actions = [
        (rc_kit::ico::RUN_ALL, tr("Run with Coverage"), cargo && !busy && !app.criticalup_provider.selected),
        (rc_kit::ico::STOP, tr("Stop"), busy),
        (rc_kit::ico::OPEN, tr("Import LCOV…"), cargo && !busy),
    ];
    match crate::ui::run_windows::header(ui, tr("Coverage"), &actions) {
        Some(0) => app.start_coverage(),
        Some(1) => app.coverage.cancel(),
        Some(2) => if let Some(root) = app.workspace.as_ref().map(|w| w.ws.root.clone()) { app.coverage.pick_import(root); },
        Some(usize::MAX) => app.toggle_tool(crate::toolwindow::ToolWindow::Tests),
        _ => {}
    }
    ui.label(tr("Line coverage only. Install cargo-llvm-cov and llvm-tools-preview before running; tools are never installed automatically."));
    if app.criticalup_provider.selected{ui.label(crate::criticalup_provider::unsupported());}
    if busy { ui.horizontal(|ui| { ui.spinner(); ui.label(tr("Collecting coverage…")); }); }
    if let Some(error) = &app.coverage.error { ui.colored_label(rc_kit::pal(ui).error, error); }
    let mut remove = false;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("coverage_suite").selected_text(app.coverage.suites.get(app.coverage.selected).map_or(tr("No coverage suites"), |suite| suite.name.as_str())).show_ui(ui, |ui| {
            for (index, suite) in app.coverage.suites.iter().enumerate() {
                if ui.selectable_value(&mut app.coverage.selected, index, &suite.name).changed() { app.coverage.selected_file = 0; }
            }
        });
        remove = ui.add_enabled(!app.coverage.suites.is_empty(), egui::Button::new(tr("Remove Suite"))).clicked();
    });
    if remove {
        let index = app.coverage.selected;
        app.coverage.suites.remove(index);
        app.coverage.selected = index.saturating_sub(1);
        app.coverage.selected_file = 0;
    }
    let Some(suite) = app.coverage.suites.get(app.coverage.selected) else {
        crate::ui::run_windows::console(ui, &app.coverage.console.term);
        return;
    };
    let (covered, total) = suite.totals();
    ui.label(summary(covered, total));
    if suite.skipped > 0 { ui.label(trf("Skipped {count} missing or external source records", &[("count", &suite.skipped)])); }
    let mut open = None;
    ui.columns(2, |columns| {
        egui::ScrollArea::vertical().id_salt("coverage_files").max_height(220.0).show_rows(&mut columns[0], 24.0, suite.files.len(), |ui, range| {
            for index in range {
                let file = &suite.files[index];
                let relative = file.path.strip_prefix(&suite.root).unwrap_or(&file.path);
                let label = format!("{}  {}", relative.display(), summary(file.covered(), file.lines.len()));
                let response = ui.selectable_label(app.coverage.selected_file == index, label);
                if response.clicked() { app.coverage.selected_file = index; }
                if response.double_clicked() { open = Some((file.path.clone(), file.lines.first().map_or(1, |(line, _)| *line))); }
            }
        });
        if let Some(file) = suite.files.get(app.coverage.selected_file) {
            egui::ScrollArea::vertical().id_salt("coverage_lines").max_height(220.0).show_rows(&mut columns[1], 24.0, file.lines.len(), |ui, range| {
                for index in range {
                    let (line, hits) = file.lines[index];
                    let color = if hits == 0 { rc_kit::pal(ui).error } else { rc_kit::pal(ui).text };
                    let label = trf("Line {line}: {hits} hits", &[("line", &line), ("hits", &hits)]);
                    if ui.link(egui::RichText::new(label).color(color)).clicked() { open = Some((file.path.clone(), line)); }
                }
            });
        }
    });
    if let Some((path, line)) = open { app.open_at(&path, line.saturating_sub(1), 0); }
    egui::CollapsingHeader::new(tr("Console")).default_open(busy || app.coverage.error.is_some()).show(ui, |ui| { crate::ui::run_windows::console(ui, &app.coverage.console.term); });
}

fn summary(covered: usize, total: usize) -> String {
    let percent = if total == 0 { 0.0 } else { covered as f64 * 100.0 / total as f64 };
    trf("{covered}/{total} lines ({percent}%)", &[("covered", &covered), ("total", &total), ("percent", &format!("{percent:.1}"))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn source() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"coverage_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
        std::fs::write(root.path().join("src/lib.rs"), "pub fn answer() -> i32 { 42 }\n").unwrap();
        root
    }

    #[test]
    fn lcov_merges_lines_and_derives_totals_instead_of_trusting_summaries() {
        let root = source();
        let text = "TN:unit\nSF:src/lib.rs\nDA:1,0\nDA:2,1,checksum\nLF:999\nLH:999\nend_of_record\nTN:integration\nSF:src/lib.rs\nDA:1,2\nDA:3,0\nend_of_record\n";
        let suite = parse(text, root.path(), "demo".into(), &CancelToken::new()).unwrap();
        assert_eq!(suite.totals(), (2, 3));
        assert_eq!(suite.files[0].lines, [(1, 2), (2, 1), (3, 0)]);
        for malformed in ["DA:1,0\n", "SF:src/lib.rs\nDA:0,1\nend_of_record\n", "SF:src/lib.rs\nDA:1,-1\nend_of_record\n", "SF:src/lib.rs\nDA:1,2\n", "end_of_record\n", "SF:src/lib.rs\nSF:src/lib.rs\nend_of_record\n"] {
            assert!(matches!(parse(malformed, root.path(), "bad".into(), &CancelToken::new()), Err(Error::Format(_))));
        }
        let token = CancelToken::new(); token.cancel();
        assert!(matches!(parse(text, root.path(), "cancel".into(), &token), Err(Error::Cancelled)));
    }

    #[test]
    fn external_paths_and_symlink_escapes_are_not_navigable() {
        let root = source();
        let outside = source();
        let report = format!("SF:{}\nDA:1,1\nend_of_record\nSF:src/lib.rs\nDA:1,0\nend_of_record\n", outside.path().join("src/lib.rs").display());
        let suite = parse(&report, root.path(), "scope".into(), &CancelToken::new()).unwrap();
        assert_eq!(suite.skipped, 1);
        assert_eq!(suite.files.len(), 1);
        assert!(suite.files[0].path.starts_with(root.path()));
        #[cfg(unix)] {
            std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
            assert!(matches!(parse("SF:escape/src/lib.rs\nDA:1,1\nend_of_record\n", root.path(), "bad".into(), &CancelToken::new()), Err(Error::Empty)));
        }
        assert!(matches!(parse(&" ".repeat(MAX_BYTES + 1), root.path(), "huge".into(), &CancelToken::new()), Err(Error::Limit)));
    }

    fn finish(state: &mut Coverage) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while state.busy() {
            state.poll();
            assert!(Instant::now() < deadline, "coverage timed out: {:?}", state.console.term.screen_lines());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[test]
    fn fifo_import_and_cancellation_finish_without_a_writer() {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        use std::io::Write;
        let root = source();
        let path = root.path().join("report.info");
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: the temporary path is NUL-terminated and owned for this call.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let (send, receive) = std::sync::mpsc::channel();
        let fifo = path.clone();
        let folder = root.path().to_path_buf();
        let worker = std::thread::spawn(move || {
            let result = read_report(&fifo, &folder, "fifo".into(), &CancelToken::new());
            let _ = send.send(result);
        });
        let result = receive.recv_timeout(Duration::from_secs(1));
        if result.is_err() {
            // Unblock a regressed blocking open before failing, so the test never hangs.
            if let Ok(mut writer) = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&path) {
                let _ = writer.write_all(b"invalid\n");
            }
        }
        assert!(matches!(result, Ok(Err(Error::NotRegular))));
        worker.join().unwrap();
        let cancelled = CancelToken::new(); cancelled.cancel();
        assert!(matches!(read_report(&path, root.path(), "fifo".into(), &cancelled), Err(Error::Cancelled)));
        let mut coverage = Coverage::new(Waker::noop());
        coverage.import(path, root.path().into());
        coverage.cancel();
        let deadline = Instant::now() + Duration::from_secs(1);
        while coverage.busy() {
            coverage.poll();
            assert!(Instant::now() < deadline, "FIFO import did not finish after cancellation");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(coverage.error.is_some());
        assert!(coverage.suites.is_empty());
    }

    #[test]
    fn import_creates_distinct_suites_without_running_project_code() {
        let root = source();
        std::fs::write(root.path().join("build.rs"), "fn main() { panic!(\"must not execute\"); }\n").unwrap();
        let report = root.path().join("report.info");
        std::fs::write(&report, "SF:src/lib.rs\nDA:1,1\nend_of_record\n").unwrap();
        let mut coverage = Coverage::new(Waker::noop());
        coverage.import(report.clone(), root.path().into()); finish(&mut coverage);
        coverage.import(report, root.path().into()); finish(&mut coverage);
        assert_eq!(coverage.suites.len(), 2);
        assert_ne!(coverage.suites[0].name, coverage.suites[1].name);
        assert_eq!(coverage.suites[0].totals(), (1, 1));
        assert!(coverage.error.is_none());
    }

    #[test]
    fn native_coverage_panel_opens_the_missed_rust_line() {
        use egui_kittest::kittest::Queryable;
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = source();
        let source = root.path().join("src/lib.rs");
        std::fs::write(&source, "pub fn answer() -> i32 {\n    42\n}\n").unwrap();
        let report = root.path().join("report.info");
        std::fs::write(&report, "SF:src/lib.rs\nDA:1,1\nDA:2,0\nend_of_record\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        app.open_workspace_and_wait(root.path());
        app.coverage.import(report, root.path().into());
        finish(&mut app.coverage);
        app.coverage.show = true;
        app.dock.show(crate::toolwindow::ToolWindow::Tests);
        let mut harness = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
        let label = trf("Line {line}: {hits} hits", &[("line", &2), ("hits", &0)]);
        harness.get_by_label(&label).click();
        harness.run_steps(8);
        let doc = harness.state().docs.active_doc().unwrap();
        assert_eq!(doc.path, source.canonicalize().unwrap());
        assert_eq!(doc.editor.caret_line_col().0, 1);
    }

    #[test]
    fn app_requires_trust_before_starting_coverage() {
        let root = source();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"coverage_fixture\"\nversion = \"0.1.0\"\n[workspace]\n").unwrap();
        let mut app = crate::app::CrabCraftApp::with_settings(rc_core::Settings::default(), Waker::noop(), None);
        app.workspace = Some(crate::app::WorkspaceState { ws: rc_core::Workspace::open(root.path()).unwrap() });
        app.start_coverage();
        assert!(!app.coverage.busy());
        assert!(app.coverage.error.is_some());
    }

    #[test]
    #[ignore = "Requires cargo-llvm-cov and llvm-tools-preview; runs real instrumented Rust tests"]
    fn real_rust_coverage_includes_missed_lines_and_can_be_stopped() {
        let root = source();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"coverage_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
        std::fs::write(root.path().join("src/lib.rs"), "pub fn used() -> i32 {\n    42\n}\npub fn unused() -> i32 {\n    7\n}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn works() { assert_eq!(super::used(), 42); }\n}\n").unwrap();
        let cargo = rc_cargo::Cargo::locate().unwrap();
        let mut coverage = Coverage::new(Waker::noop());
        coverage.start(root.path().into(), root.path().join("Cargo.toml"), cargo.path().into(), vec![("CARGO_NET_OFFLINE".into(), "true".into())]);
        finish(&mut coverage);
        assert!(coverage.error.is_none(), "{:?}; {:?}", coverage.error, coverage.console.term.screen_lines());
        let suite = &coverage.suites[0];
        let (covered, total) = suite.totals();
        assert!(covered > 0 && covered < total, "{suite:?}");
        assert!(suite.files[0].lines.iter().any(|(line, hits)| *line == 5 && *hits == 0), "{suite:?}");
        std::fs::write(root.path().join("build.rs"), "fn main() { std::fs::write(\"build-started\", \"yes\").unwrap(); std::thread::sleep(std::time::Duration::from_secs(20)); std::fs::write(\"build-finished\", \"yes\").unwrap(); }\n").unwrap();
        coverage.start(root.path().into(), root.path().join("Cargo.toml"), cargo.path().into(), vec![("CARGO_NET_OFFLINE".into(), "true".into())]);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !root.path().join("build-started").exists() {
            coverage.poll();
            assert!(Instant::now() < deadline, "build did not start: {:?}", coverage.console.term.screen_lines());
            std::thread::sleep(Duration::from_millis(10));
        }
        coverage.cancel(); finish(&mut coverage);
        assert!(coverage.error.is_some());
        assert_eq!(coverage.suites.len(), 1);
        assert!(!root.path().join("build-finished").exists());
    }
}
