//! Bounded rustc labeled-range visualization. These are compiler locations, never inferred
//! lifetime intervals. Source reads and validation run in a cancellable worker.
use rc_core::{Inbox, Waker};
use rc_dotnet::{
    Diagnostic,
    diagnostics::{BorrowDiagnostic, BorrowSpan},
};
use rc_kit::i18n::{tr, trf};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

struct Snapshot {
    id: u64,
    version: u64,
    epoch: u64,
    rope: rc_text::Rope,
}
struct Source {
    path: PathBuf,
    text: String,
    bom: bool,
    mtime: Option<SystemTime>,
    style: rc_core::code_style::CodeStyle,
    start: usize,
    end: usize,
    snapshot: Option<(u64, u64, u64)>,
}
#[derive(Debug, PartialEq, Eq)]
enum SourceError {
    Cancelled,
    Stale,
    Outside,
    Large,
    Invalid,
    Io,
}
fn error_text(error: &SourceError) -> &'static str {
    match error {
        SourceError::Cancelled => tr("Cancelled"),
        SourceError::Stale => {
            tr("Compiler source ranges are stale; save and rebuild before navigating")
        }
        SourceError::Outside => tr("Compiler source is outside the loaded workspace"),
        SourceError::Large => {
            tr("Compiler source navigation supports regular UTF-8 files up to 1 MB")
        }
        SourceError::Invalid => tr("The compiler source range could not be validated"),
        SourceError::Io => tr("Could not read the compiler source file"),
    }
}
pub struct BorrowErrors {
    selected: Option<Diagnostic>,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    inbox: Inbox<(u64, Result<Source, SourceError>)>,
    pub error: Option<String>,
    highlight: Option<(u64, u64, usize, usize)>,
}
impl BorrowErrors {
    pub fn new(waker: Waker) -> Self {
        Self {
            selected: None,
            generation: 0,
            cancel: None,
            inbox: Inbox::new(waker),
            error: None,
            highlight: None,
        }
    }
    fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn select(&mut self, diagnostic: Diagnostic) {
        self.cancel();
        self.error = None;
        self.selected = Some(diagnostic);
    }
    fn request(&mut self, index: usize, docs: &crate::docs::Docs) {
        self.cancel();
        self.error = None;
        let Some(bundle) = self
            .selected
            .as_ref()
            .and_then(|diagnostic| diagnostic.borrow.clone())
        else {
            return;
        };
        let Some(span) = bundle.spans.get(index).cloned() else {
            return;
        };
        let snapshot = docs.index_of(&span.file).map(|index| {
            let doc = &docs.list[index];
            Snapshot {
                id: doc.editor.id,
                version: doc.editor.buffer.version(),
                epoch: doc.disk_epoch,
                rope: doc.editor.buffer.snapshot(),
            }
        });
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let generation = self.generation;
        let out = self.inbox.sender();
        let style_root = docs.style_root.clone();
        std::thread::spawn(move || {
            let result = source(&bundle, &span, snapshot, style_root, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                out.send((generation, result));
            }
        });
    }
    pub fn poll(&mut self, docs: &mut crate::docs::Docs) -> bool {
        if let Some((id, version, start, end)) = self.highlight
            && let Some(doc) = docs.list.iter_mut().find(|doc| doc.editor.id == id)
            && doc.editor.buffer.version() != version
        {
            if doc.editor.highlights == vec![(start, end, true)] {
                doc.editor.highlights.clear();
            }
            self.highlight = None;
        }
        let mut opened = false;
        for (generation, result) in self.inbox.drain() {
            if generation != self.generation {
                continue;
            }
            self.cancel = None;
            match result {
                Err(error) => self.error = Some(error_text(&error).into()),
                Ok(source) => {
                    let existing = docs.index_of(&source.path);
                    let stable = match (source.snapshot, existing) {
                        (Some((id, version, epoch)), Some(index)) => {
                            let doc = &docs.list[index];
                            doc.editor.id == id
                                && doc.editor.buffer.version() == version
                                && doc.disk_epoch == epoch
                        }
                        (None, None) => true,
                        _ => false,
                    };
                    if !stable {
                        self.error = Some(error_text(&SourceError::Stale).into());
                        continue;
                    }
                    let index = docs.insert_compiler_source(
                        source.path,
                        &source.text,
                        source.bom,
                        source.mtime,
                        source.style,
                    );
                    if let Some((id, _, start, end)) = self.highlight.take()
                        && let Some(doc) = docs.list.iter_mut().find(|doc| doc.editor.id == id)
                    {
                        if doc.editor.highlights == vec![(start, end, true)] {
                            doc.editor.highlights.clear();
                        }
                    }
                    let doc = &mut docs.list[index];
                    doc.editor
                        .set_selection(rc_text::Selection::single(rc_text::Range::new(
                            source.start,
                            source.end,
                        )));
                    doc.editor.highlights = vec![(source.start, source.end, true)];
                    self.highlight = Some((
                        doc.editor.id,
                        doc.editor.buffer.version(),
                        source.start,
                        source.end,
                    ));
                    opened = true;
                }
            }
        }
        opened
    }
    pub fn show(&mut self, ui: &mut egui::Ui, docs: &mut crate::docs::Docs) {
        let Some(diagnostic) = self.selected.as_ref() else {
            return;
        };
        let Some(bundle) = diagnostic.borrow.as_ref() else {
            return;
        };
        let mut close = false;
        let mut navigate = None;
        ui.horizontal(|ui| {
            ui.strong(tr("Borrow checker source ranges"));
            ui.label(&diagnostic.code);
            if ui.small_button(tr("Close")).clicked() {
                close = true;
            }
        });
        ui.label(diagnostic.message.chars().take(2048).collect::<String>());
        ui.small(tr(
            "Compiler labels show reported source ranges, not complete inferred lifetimes.",
        ));
        if bundle.limited {
            ui.colored_label(
                rc_kit::pal(ui).warn,
                tr("Compiler range details were truncated at the display limit"),
            );
        }
        egui::ScrollArea::vertical()
            .id_salt("borrow_source_ranges")
            .max_height(240.0)
            .show(ui, |ui| {
                for (index, span) in bundle.spans.iter().enumerate() {
                    let color = if span.primary {
                        rc_kit::pal(ui).error
                    } else {
                        rc_kit::pal(ui).warn
                    };
                    egui::Frame::group(ui.style())
                        .stroke(egui::Stroke::new(1.0, color))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(
                                    color,
                                    if span.primary {
                                        tr("Primary range")
                                    } else {
                                        tr("Related range")
                                    },
                                );
                                ui.monospace(format!(
                                    "{}:{}:{}–{}:{}",
                                    span.file.file_name().unwrap_or_default().to_string_lossy(),
                                    span.line,
                                    span.column,
                                    span.end_line,
                                    span.end_column
                                ));
                                if ui
                                    .button(trf("Open source range {n}", &[("n", &(index + 1))]))
                                    .clicked()
                                {
                                    navigate = Some(index);
                                }
                            });
                            if !span.label.is_empty() {
                                ui.label(&span.label);
                            }
                            for line in &span.text {
                                let mut job = egui::text::LayoutJob::default();
                                let normal = egui::TextFormat {
                                    font_id: egui::FontId::monospace(12.0),
                                    color: rc_kit::pal(ui).text,
                                    ..Default::default()
                                };
                                job.append(&format!("{:>4} │ ", line.line), 0.0, normal.clone());
                                let chars: Vec<_> = line.text.chars().collect();
                                let start =
                                    (line.start.saturating_sub(1) as usize).min(chars.len());
                                let end = (line.end.saturating_sub(1) as usize)
                                    .max(start)
                                    .min(chars.len());
                                job.append(
                                    &chars[..start].iter().collect::<String>(),
                                    0.0,
                                    normal.clone(),
                                );
                                let mut selected = normal.clone();
                                selected.background = color.gamma_multiply(0.25);
                                selected.underline = egui::Stroke::new(1.0, color);
                                job.append(
                                    &chars[start..end].iter().collect::<String>(),
                                    0.0,
                                    selected,
                                );
                                job.append(&chars[end..].iter().collect::<String>(), 0.0, normal);
                                ui.label(job);
                            }
                        });
                }
                for note in &bundle.notes {
                    ui.small(note);
                }
            });
        if self.cancel.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Checking compiler source…"));
                if ui.button(tr("Cancel")).clicked() {
                    close = true;
                }
            });
        }
        if let Some(error) = &self.error {
            ui.colored_label(rc_kit::pal(ui).warn, error);
        }
        if close {
            self.cancel();
            self.selected = None;
            if let Some((id, _, start, end)) = self.highlight.take()
                && let Some(doc) = docs.list.iter_mut().find(|doc| doc.editor.id == id)
            {
                if doc.editor.highlights == vec![(start, end, true)] {
                    doc.editor.highlights.clear();
                }
            }
        } else if let Some(index) = navigate {
            self.request(index, docs);
        }
    }
}
impl Drop for BorrowErrors {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn source(
    bundle: &BorrowDiagnostic,
    span: &BorrowSpan,
    snapshot: Option<Snapshot>,
    style_root: Option<PathBuf>,
    cancel: &AtomicBool,
) -> Result<Source, SourceError> {
    use std::io::Read;
    if cancel.load(Ordering::Relaxed) {
        return Err(SourceError::Cancelled);
    }
    let root = bundle
        .root
        .canonicalize()
        .map_err(|_| SourceError::Outside)?;
    let path = span.file.canonicalize().map_err(|_| SourceError::Io)?;
    if !path.starts_with(root) || path.extension().is_none_or(|ext| ext != "rs") {
        return Err(SourceError::Outside);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let mut file = options.open(&path).map_err(|_| SourceError::Io)?;
    let metadata = file.metadata().map_err(|_| SourceError::Io)?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(SourceError::Large);
    }
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(SourceError::Cancelled);
        }
        let count = file.read(&mut chunk).map_err(|_| SourceError::Io)?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > 1024 * 1024 {
            return Err(SourceError::Large);
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let after = file.metadata().map_err(|_| SourceError::Io)?;
    let named = std::fs::metadata(&path).map_err(|_| SourceError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if named.dev() != metadata.dev() || named.ino() != metadata.ino() {
            return Err(SourceError::Stale);
        }
    }
    if named.len() != metadata.len()
        || named.modified().ok() != metadata.modified().ok()
        || after.len() != metadata.len()
        || after.modified().ok() != metadata.modified().ok()
    {
        return Err(SourceError::Stale);
    }
    let raw = String::from_utf8(bytes).map_err(|_| SourceError::Invalid)?;
    if raw.contains('\0') {
        return Err(SourceError::Invalid);
    }
    let bom = raw.starts_with('\u{feff}');
    let text = raw
        .strip_prefix('\u{feff}')
        .unwrap_or(&raw)
        .replace("\r\n", "\n");
    let positions = validate(&raw, &text, span)?;
    let stamp = if let Some(snapshot) = snapshot {
        if snapshot.rope.to_string() != text {
            return Err(SourceError::Stale);
        }
        Some((snapshot.id, snapshot.version, snapshot.epoch))
    } else {
        None
    };
    let style = match style_root.as_deref().filter(|root| path.starts_with(root)) {
        Some(root) => rc_core::code_style::for_file(&path, Some(root)),
        None => Default::default(),
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(SourceError::Cancelled);
    }
    Ok(Source {
        path,
        text: raw.strip_prefix('\u{feff}').unwrap_or(&raw).to_string(),
        bom,
        mtime: metadata.modified().ok(),
        style,
        start: positions.0,
        end: positions.1,
        snapshot: stamp,
    })
}
fn validate(raw: &str, text: &str, span: &BorrowSpan) -> Result<(usize, usize), SourceError> {
    if span.start > span.end
        || span.end > raw.len()
        || !raw.is_char_boundary(span.start)
        || !raw.is_char_boundary(span.end)
        || span.text.is_empty()
    {
        return Err(SourceError::Invalid);
    }
    let lines: Vec<_> = text.lines().collect();
    for expected in &span.text {
        if lines.get(expected.line.saturating_sub(1) as usize).copied()
            != Some(expected.text.as_str())
        {
            return Err(SourceError::Stale);
        }
    }
    if span.end_line.saturating_sub(span.line) as usize >= span.text.len() {
        return Err(SourceError::Invalid);
    }
    let rope = rc_text::Rope::from_str(text);
    let at = |line: u32, column: u32| -> Option<usize> {
        let line = line.checked_sub(1)? as usize;
        let column = column.checked_sub(1)? as usize;
        if line >= rope.len_lines() {
            return None;
        }
        let start = rope.line_to_char(line);
        let len = rope.line(line).chars().filter(|ch| *ch != '\n').count();
        (column <= len).then_some(start + column)
    };
    let start = at(span.line, span.column).ok_or(SourceError::Invalid)?;
    let end = at(span.end_line, span.end_column)
        .filter(|end| *end >= start)
        .ok_or(SourceError::Invalid)?;
    let bytes_start = raw[..span.start]
        .strip_prefix('\u{feff}')
        .unwrap_or(&raw[..span.start])
        .replace("\r\n", "\n")
        .chars()
        .count();
    let bytes_end = raw[..span.end]
        .strip_prefix('\u{feff}')
        .unwrap_or(&raw[..span.end])
        .replace("\r\n", "\n")
        .chars()
        .count();
    if start != bytes_start || end != bytes_end {
        return Err(SourceError::Stale);
    }
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{HOME_LOCK, harness, test_app};
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};
    fn pump(
        h: &mut egui_kittest::Harness<'static, crate::app::CrabCraftApp>,
        done: impl Fn(&crate::app::CrabCraftApp) -> bool,
    ) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            h.run_steps(2);
            if done(h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("borrow UI timed out: {:?}", h.state().borrow_errors.error);
    }
    #[test]
    fn real_rust_borrow_errors_show_compiler_labels_and_navigate_checked_source() {
        let _lock = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"borrow_evidence\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
        )
        .unwrap();
        let code = "// Unicode 🦀 before compiler byte offsets\npub fn mixed() {\n    let mut items = vec![1];\n    let first = &items[0];\n    items.push(2);\n    println!(\"{}\", first);\n}\npub fn mutable() {\n    let mut value = 1;\n    let first = &mut value;\n    let second = &mut value;\n    println!(\"{} {}\", first, second);\n}\npub fn lifetime() {\n    let reference;\n    { let value = 1; reference = &value; }\n    println!(\"{}\", reference);\n}\n";
        let raw = format!("\u{feff}{}", code.replace('\n', "\r\n"));
        std::fs::write(root.join("src/lib.rs"), &raw).unwrap();
        std::fs::write(
            root.join("build.rs"),
            "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}",
        )
        .unwrap();
        let mut app = test_app(home.path(), true);
        app.open_workspace(&root);
        app.trust_project_tools_for_test();
        let mut h = harness(app, egui::vec2(1440.0, 1200.0));
        pump(&mut h, |app| app.runner.cargo_workspace.is_some());
        assert!(!root.join("build-ran").exists());
        h.state_mut().settings.trusted_projects.push(root.clone());
        h.state_mut()
            .runner
            .build(&root.join("Cargo.toml"), rc_dotnet::BuildAction::Build);
        h.state_mut()
            .dock
            .show(crate::toolwindow::ToolWindow::Build);
        h.state_mut()
            .dock
            .sizes
            .insert(crate::toolwindow::ToolWindow::Build, 650.0);
        pump(&mut h, |app| {
            app.runner
                .build
                .as_ref()
                .is_some_and(|build| build.result.is_some())
        });
        let diagnostics = h.state().runner.build.as_ref().unwrap().diagnostics.clone();
        for code in ["E0502", "E0499", "E0597"] {
            let diagnostic = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.code == code)
                .unwrap();
            let bundle = diagnostic.borrow.as_ref().unwrap();
            assert!(bundle.spans.iter().any(|span| span.primary));
            assert!(
                bundle
                    .spans
                    .iter()
                    .any(|span| !span.primary && !span.label.is_empty())
            );
            for span in &bundle.spans {
                if !span.text.is_empty() {
                    let checked = source(
                        bundle,
                        span,
                        None,
                        Some(root.clone()),
                        &AtomicBool::new(false),
                    )
                    .unwrap();
                    assert!(checked.end >= checked.start);
                }
            }
        }
        h.run_steps(3);
        h.get_by_label(&trf(
            "Visualize {code} source ranges…",
            &[("code", &"E0502")],
        ))
        .click();
        h.run_steps(2);
        let diagnostic = h.state().borrow_errors.selected.as_ref().unwrap();
        assert_eq!(diagnostic.code, "E0502");
        let bundle = diagnostic.borrow.as_ref().unwrap().clone();
        let selected = bundle.spans[0].clone();
        h.get_by_label(&trf("Open source range {n}", &[("n", &1)]))
            .click();
        h.run_steps(2);
        pump(&mut h, |app| {
            app.docs
                .active_doc()
                .is_some_and(|doc| !doc.editor.selection.primary().is_empty())
        });
        let doc = h.state().docs.active_doc().unwrap();
        assert_eq!(doc.path, root.join("src/lib.rs"));
        let range = doc.editor.selection.primary();
        let expected = source(
            &bundle,
            &selected,
            None,
            Some(root.clone()),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!((range.start(), range.end()), (expected.start, expected.end));
        assert_eq!(
            doc.editor.highlights,
            vec![(expected.start, expected.end, true)]
        );
        assert!(doc.bom);
        assert_eq!(doc.editor.buffer.line_ending(), rc_text::LineEnding::CrLf);
        // A result prepared for the previous document version must not move a changed buffer.
        {
            let app = h.state_mut();
            app.borrow_errors.request(0, &app.docs);
        }
        // An unsaved edit invalidates navigation and annotations. No old source is applied.
        {
            let doc = h.state_mut().docs.active_doc_mut().unwrap();
            let selection = doc.editor.selection.clone();
            doc.editor.buffer.apply(
                &rc_text::ChangeSet::single(rc_text::Change::new(0, 0, "// changed\n")),
                &selection,
                &selection,
                rc_text::EditKind::Insert,
            );
        }
        h.run_steps(2);
        pump(&mut h, |app| app.borrow_errors.error.is_some());
        assert!(
            h.state()
                .docs
                .active_doc()
                .unwrap()
                .editor
                .highlights
                .is_empty()
        );
        h.get_by_label(&trf("Open source range {n}", &[("n", &1)]))
            .click();
        h.run_steps(2);
        pump(&mut h, |app| app.borrow_errors.error.is_some());
        assert!(
            h.state()
                .docs
                .active_doc()
                .unwrap()
                .editor
                .buffer
                .rope()
                .to_string()
                .starts_with("// changed")
        );
        {
            let app = h.state_mut();
            app.borrow_errors.request(0, &app.docs);
            app.borrow_errors.cancel();
        }
        let docs = &h.state().docs;
        let current = docs.active_doc().unwrap();
        let snapshot = Snapshot {
            id: current.editor.id,
            version: current.editor.buffer.version(),
            epoch: current.disk_epoch,
            rope: current.editor.buffer.snapshot(),
        };
        assert!(matches!(
            source(
                &bundle,
                &selected,
                Some(snapshot),
                Some(root.clone()),
                &AtomicBool::new(false)
            ),
            Err(SourceError::Stale)
        ));
        std::fs::write(
            root.join("src/lib.rs"),
            raw.replace("items.push(2)", "items.push(3)"),
        )
        .unwrap();
        assert!(matches!(
            source(
                &bundle,
                &selected,
                None,
                Some(root.clone()),
                &AtomicBool::new(false)
            ),
            Err(SourceError::Stale)
        ));
        assert!(matches!(
            source(&bundle, &selected, None, Some(root), &AtomicBool::new(true)),
            Err(SourceError::Cancelled)
        ));
    }
}
