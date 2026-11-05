//! Standalone native diff/merge windows. They never open a workspace, restore a session,
//! start a language server, invoke Git, or run project code.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, AtomicI32, Ordering}};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_LINES: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompareRequest {
    Diff { left: PathBuf, right: PathBuf },
    Merge { left: PathBuf, right: PathBuf, base: PathBuf, result: PathBuf },
}

pub fn parse(merge: bool, args: impl Iterator<Item = OsString>, cwd: &Path) -> Result<crate::cli::Command, crate::cli::CliError> {
    let mut paths = Vec::new();
    let mut literal = false;
    for arg in args {
        if !literal {
            match arg.to_str() {
                Some("--") => { literal = true; continue; },
                Some("--help" | "-h") => return Ok(crate::cli::Command::Help),
                Some(text) if text.starts_with('-') => return Err(crate::cli::CliError::UnknownOption(text.to_string())),
                _ => {},
            }
        }
        let path = PathBuf::from(arg);
        paths.push(if path.is_absolute() { path } else { cwd.join(path) });
    }
    match paths.as_slice() {
        [left, right] if !merge => Ok(crate::cli::Command::Compare(CompareRequest::Diff { left: left.clone(), right: right.clone() })),
        [left, right, base, result] if merge => Ok(crate::cli::Command::Compare(CompareRequest::Merge { left: left.clone(), right: right.clone(), base: base.clone(), result: result.clone() })),
        _ => Err(crate::cli::CliError::ComparisonPaths { merge }),
    }
}

#[derive(Debug)]
enum CompareError {
    Io { path: PathBuf, error: std::io::Error },
    InvalidText(PathBuf),
    TooLarge(PathBuf),
    OutputChanged,
    Cancelled,
}

impl std::fmt::Display for CompareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Io { path, error } => trf("Cannot access comparison file {path}: {error}", &[("path", &path.display()), ("error", error)]),
            Self::InvalidText(path) => trf("Comparison requires a regular UTF-8 text file: {path}", &[("path", &path.display())]),
            Self::TooLarge(path) => trf("Comparison file exceeds 4 MB or 100000 lines: {path}", &[("path", &path.display())]),
            Self::OutputChanged => tr("The output file changed since the merge was opened. Reopen the merge before saving.").to_string(),
            Self::Cancelled => tr("Cancelled").to_string(),
        };
        f.write_str(&text)
    }
}

fn io(path: &Path, error: std::io::Error) -> CompareError { CompareError::Io { path: path.to_path_buf(), error } }

fn read(path: &Path, cancel: &AtomicBool) -> Result<Vec<u8>, CompareError> {
    if cancel.load(Ordering::Relaxed) { return Err(CompareError::Cancelled); }
    let metadata = std::fs::metadata(path).map_err(|e| io(path, e))?;
    if !metadata.is_file() { return Err(CompareError::InvalidText(path.to_path_buf())); }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; options.custom_flags(libc::O_NONBLOCK); }
    let mut file = options.open(path).map_err(|e| io(path, e))?;
    let metadata = file.metadata().map_err(|e| io(path, e))?;
    if !metadata.is_file() { return Err(CompareError::InvalidText(path.to_path_buf())); }
    if metadata.len() > MAX_BYTES as u64 { return Err(CompareError::TooLarge(path.to_path_buf())); }
    let mut bytes = Vec::new();
    let mut chunk = [0; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) { return Err(CompareError::Cancelled); }
        let n = file.read(&mut chunk).map_err(|e| io(path, e))?;
        if n == 0 { break; }
        if bytes.len() + n > MAX_BYTES { return Err(CompareError::TooLarge(path.to_path_buf())); }
        bytes.extend_from_slice(&chunk[..n]);
    }
    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() { return Err(CompareError::InvalidText(path.to_path_buf())); }
    if bytes.iter().filter(|b| **b == b'\n').count() > MAX_LINES { return Err(CompareError::TooLarge(path.to_path_buf())); }
    Ok(bytes)
}

fn output_snapshot(path: &Path, cancel: &AtomicBool) -> Result<Option<Vec<u8>>, CompareError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => Err(CompareError::InvalidText(path.to_path_buf())),
        Ok(_) => read(path, cancel).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io(path, error)),
    }
}

enum View {
    Diff(crate::vcs::DiffView),
    Merge { tool: crate::merge_tool::MergeTool, previous: Option<Vec<u8>> },
}

fn load(request: &CompareRequest, dark: bool, cancel: &AtomicBool) -> Result<View, CompareError> {
    let text = |path: &Path| read(path, cancel).map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    match request {
        CompareRequest::Diff { left, right } => {
            let (a, b) = (text(left)?, text(right)?);
            let mut vcs = crate::vcs::Vcs::new(Waker::noop());
            vcs.show_diff(format!("{} ↔ {}", left.display(), right.display()), right.clone(), &a, &b, dark);
            vcs.diff.take().map(View::Diff).ok_or(CompareError::Cancelled)
        }
        CompareRequest::Merge { left, right, base, result } => {
            let (a, b, common) = (text(left)?, text(right)?, text(base)?);
            let previous = output_snapshot(result, cancel)?;
            let tool = crate::merge_tool::MergeTool::new(&mut 100, result.display().to_string(), result.clone(), &common, &a, &b, (left.display().to_string(), right.display().to_string()), dark);
            Ok(View::Merge { tool, previous })
        }
    }
}

/// Atomically replaces only the explicit output; source files are read-only. A changed
/// output is refused. A new output uses no-clobber persistence so a racing writer is safe.
fn save(path: &Path, previous: &Option<Vec<u8>>, text: &str, cancel: &AtomicBool) -> Result<(), CompareError> {
    if &output_snapshot(path, cancel)? != previous { return Err(CompareError::OutputChanged); }
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| io(path, e))?;
    if previous.is_some() {
        let permissions = std::fs::metadata(path).map_err(|e| io(path, e))?.permissions();
        tmp.as_file().set_permissions(permissions).map_err(|e| io(path, e))?;
    }
    tmp.write_all(text.as_bytes()).map_err(|e| io(path, e))?;
    tmp.as_file().sync_all().map_err(|e| io(path, e))?;
    if cancel.load(Ordering::Relaxed) { return Err(CompareError::Cancelled); }
    if &output_snapshot(path, cancel)? != previous { return Err(CompareError::OutputChanged); }
    if previous.is_some() { tmp.persist(path).map_err(|e| io(path, e.error))?; }
    else { tmp.persist_noclobber(path).map_err(|e| io(path, e.error))?; }
    Ok(())
}

struct CompareApp {
    view: Option<View>,
    loaded: Inbox<Result<View, CompareError>>,
    saved: Inbox<Result<(), CompareError>>,
    cancel: Arc<AtomicBool>,
    exit: Arc<AtomicI32>,
    error: Option<String>,
    saving: bool,
    config: rc_editor::EditorConfig,
    keymap: rc_editor::Keymap,
}

impl CompareApp {
    fn new(request: CompareRequest, dark: bool, waker: Waker, exit: Arc<AtomicI32>) -> Self {
        let app = CompareApp { view: None, loaded: Inbox::new(waker.clone()), saved: Inbox::new(waker), cancel: Arc::new(AtomicBool::new(false)), exit, error: None, saving: false, config: rc_editor::EditorConfig::default(), keymap: rc_editor::Keymap::new(if cfg!(target_os = "macos") { rc_editor::KeymapKind::RiderMac } else { rc_editor::KeymapKind::Rider }) };
        let (out, cancel) = (app.loaded.sender(), app.cancel.clone());
        std::thread::spawn(move || {
            let result = load(&request, dark, &cancel);
            if !cancel.load(Ordering::Relaxed) { out.send(result); }
        });
        app
    }

    fn poll(&mut self, ctx: &egui::Context) {
        for result in self.loaded.drain() {
            match result {
                Ok(view) => { if matches!(view, View::Diff(_)) { self.exit.store(0, Ordering::Relaxed); } self.view = Some(view); },
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        for result in self.saved.drain() {
            self.saving = false;
            match result {
                Ok(()) => { self.exit.store(0, Ordering::Relaxed); ctx.send_viewport_cmd(egui::ViewportCommand::Close); },
                Err(error) => { self.error = Some(error.to_string()); if let Some(View::Merge { tool, .. }) = &mut self.view { tool.saving = false; } },
            }
        }
    }

    fn save_result(&mut self) {
        let Some(View::Merge { tool, previous }) = &mut self.view else { return };
        tool.sync();
        tool.saving = true;
        self.saving = true;
        self.error = None;
        let (path, previous, text) = (tool.abs.clone(), previous.clone(), tool.output());
        let (out, cancel) = (self.saved.sender(), self.cancel.clone());
        std::thread::spawn(move || { out.send(save(&path, &previous, &text, &cancel)); });
    }

    fn show(&mut self, ui: &mut egui::Ui) {
        self.poll(ui.ctx());
        if let Some(error) = &self.error { ui.colored_label(rc_kit::pal(ui).error, error); }
        match &mut self.view {
            None => { if self.error.is_none() { ui.spinner(); ui.label(tr("Loading comparison…")); } if ui.button(tr("Close")).clicked() { ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close); } },
            Some(View::Diff(diff)) => {
                ui.horizontal(|ui| { ui.label(&diff.title); if ui.button(tr("Close")).clicked() { ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close); } });
                ui.separator();
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(ui.available_rect_before_wrap()));
                crate::ui::git::diff_editors(diff, &mut child, &self.config, &self.keymap);
            }
            Some(View::Merge { tool, .. }) => {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(ui.available_rect_before_wrap()));
                match crate::ui::git::merge_editor(tool, &mut child, &self.config, &self.keymap, tr("Save Result")) {
                    crate::ui::git::MergeAction::None => {},
                    crate::ui::git::MergeAction::Close => { if !self.saving { ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close); } },
                    crate::ui::git::MergeAction::Save => self.save_result(),
                }
            }
        }
    }
}

impl Drop for CompareApp { fn drop(&mut self) { self.cancel.store(true, Ordering::Relaxed); } }
impl eframe::App for CompareApp {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll(ctx);
        if self.saving && ctx.input(|i| i.viewport().close_requested()) { ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose); }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) { self.show(ui); }
}

pub fn run(request: CompareRequest, mut options: eframe::NativeOptions) -> eframe::Result<i32> {
    options.persist_window = false;
    // Standalone tools use the operating system's movable/resizable title bar, including
    // Windows where the main IDE normally paints its own window controls.
    options.viewport = options.viewport.with_decorations(true).with_titlebar_shown(true).with_title_shown(true).with_fullsize_content_view(false);
    let exit = Arc::new(AtomicI32::new(1));
    let result = exit.clone();
    eframe::run_native("CrabCraft", options, Box::new(move |cc| {
        rc_kit::theme::install_fonts(&cc.egui_ctx);
        let settings = rc_core::Settings::load();
        let dark = match settings.theme { rc_core::settings::ThemeChoice::Light => false, rc_core::settings::ThemeChoice::Dark => true, _ => cc.egui_ctx.system_theme().is_none_or(|t| t == egui::Theme::Dark) };
        rc_kit::theme::apply(&cc.egui_ctx, dark);
        let ctx = cc.egui_ctx.clone();
        Ok(Box::new(CompareApp::new(request, dark, Waker::new(move || ctx.request_repaint()), exit)))
    }))?;
    Ok(result.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    fn fixture() -> (tempfile::TempDir, CompareRequest) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"compare-fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(dir.path().join("build.rs"), "fn main() { panic!(\"must not execute\"); }\n").unwrap();
        for (file, value) in [("base.rs", 0), ("left.rs", 1), ("right.rs", 2)] {
            std::fs::write(dir.path().join(file), format!("fn main() {{\n    println!(\"{value}\");\n}}\n")).unwrap();
        }
        let request = CompareRequest::Merge { left: dir.path().join("left.rs"), right: dir.path().join("right.rs"), base: dir.path().join("base.rs"), result: dir.path().join("src/main.rs") };
        (dir, request)
    }

    fn harness(request: CompareRequest) -> Harness<'static, CompareApp> {
        let exit = Arc::new(AtomicI32::new(1));
        let app = CompareApp::new(request, true, Waker::noop(), exit);
        let mut first = true;
        let mut h = Harness::builder().with_size(egui::vec2(1500.0, 800.0)).build_ui_state(move |ui, app: &mut CompareApp| {
            if std::mem::take(&mut first) { rc_kit::theme::install_fonts(ui.ctx()); rc_kit::theme::apply(ui.ctx(), true); return; }
            app.show(ui);
        }, app);
        for _ in 0..200 {
            h.run_steps(1);
            if h.state().view.is_some() || h.state().error.is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(h.state().error.is_none(), "{:?}", h.state().error);
        assert!(h.state().view.is_some());
        h
    }

    #[test]
    fn paths_are_literal_and_arity_is_checked() {
        let cwd = Path::new("/work");
        let parse = |args: &[&str]| crate::cli::parse(args.iter().map(OsString::from), cwd);
        assert_eq!(parse(&["diff", "left.rs:7", "right.rs"]), Ok(crate::cli::Command::Compare(CompareRequest::Diff { left: cwd.join("left.rs:7"), right: cwd.join("right.rs") })));
        assert!(parse(&["diff", "left.rs"]).is_err());
        assert!(parse(&["merge", "left.rs", "right.rs", "base.rs"]).is_err());
        assert!(parse(&["diff", "left.rs", "right.rs", "extra"]).is_err());
        assert_eq!(parse(&["merge", "--help"]), Ok(crate::cli::Command::Help));
        assert!(parse(&["diff", "--bogus", "right.rs"]).is_err());
        assert!(parse(&["diff", "--", "-left.rs", "right.rs"]).is_ok());
    }

    #[test]
    fn native_merge_accepts_a_side_and_saves_only_the_explicit_rust_output() {
        let _guard = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (dir, request) = fixture();
        let mut h = harness(request);
        assert_eq!(h.state().exit.load(Ordering::Relaxed), 1);
        assert!(!dir.path().join("src/main.rs").exists());
        h.get_by_label("Accept Right").click();
        h.run_steps(2);
        h.get_by_label("Save Result").click();
        for _ in 0..200 {
            h.run_steps(1);
            if !h.state().saving { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(h.state().error.is_none(), "{:?}", h.state().error);
        assert_eq!(h.state().exit.load(Ordering::Relaxed), 0);
        assert_eq!(std::fs::read_to_string(dir.path().join("src/main.rs")).unwrap(), std::fs::read_to_string(dir.path().join("right.rs")).unwrap());
        assert!(std::fs::read_to_string(dir.path().join("left.rs")).unwrap().contains("1"));
        // Explicitly compile the resulting Rust file without running the project's build.rs.
        assert!(std::process::Command::new("rustc").arg(dir.path().join("src/main.rs")).arg("-o").arg(dir.path().join("merged-program")).status().unwrap().success());
    }

    #[test]
    fn native_diff_is_read_only_and_does_not_open_a_workspace() {
        let _guard = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (dir, _) = fixture();
        let h = harness(CompareRequest::Diff { left: dir.path().join("left.rs"), right: dir.path().join("right.rs") });
        let Some(View::Diff(diff)) = &h.state().view else { panic!("diff expected") };
        assert!(diff.left.read_only && diff.right.read_only);
        assert!(!diff.left.line_backgrounds.is_empty());
        assert_eq!(h.state().exit.load(Ordering::Relaxed), 0);
        assert!(!dir.path().join("src/main.rs").exists());
    }

    #[test]
    fn cancellation_bad_files_and_changed_output_are_safe() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let file = dir.path().join("output.rs");
        std::fs::write(&file, "original").unwrap();
        let original = output_snapshot(&file, &cancel).unwrap();
        std::fs::write(&file, "changed elsewhere").unwrap();
        assert!(matches!(save(&file, &original, "replacement", &cancel), Err(CompareError::OutputChanged)));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "changed elsewhere");
        std::fs::write(&file, b"\xff\0").unwrap();
        assert!(matches!(read(&file, &cancel), Err(CompareError::InvalidText(_))));
        assert!(matches!(read(dir.path(), &cancel), Err(CompareError::InvalidText(_))));
        std::fs::write(&file, vec![b'a'; MAX_BYTES + 1]).unwrap();
        assert!(matches!(read(&file, &cancel), Err(CompareError::TooLarge(_))));
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(read(&file, &cancel), Err(CompareError::Cancelled)));
    }
}
