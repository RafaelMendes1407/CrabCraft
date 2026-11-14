//! Settings backup file dialogs and disk access run away from the UI thread.
use rc_core::{
    Inbox, Settings, Waker,
    settings_transfer::{Backup, TransferError},
};
use rc_kit::i18n::tr;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub struct State {
    pub migration: crate::settings_migration::State,
    sync: crate::settings_sync::Sync,
    job: Option<Job>,
    pub preview: Option<Backup>,
    message: Option<String>,
    exported: bool,
}
enum Completed {
    Cancelled,
    Exported,
    Imported(Backup),
}
struct Job {
    inbox: Inbox<Result<Completed, TransferError>>,
    cancelled: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
impl State {
    pub fn cancel(&mut self) {
        self.job = None;
        self.preview = None;
        self.sync.clear();
        self.migration.cancel();
    }

    fn start(&mut self, export: Option<Backup>, waker: Waker) {
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = cancelled.clone();
        self.preview = None;
        self.message = None;
        self.exported = false;
        let title = if export.is_some() { tr("Export preferences") } else { tr("Import preferences") }.to_string();
        let result = std::thread::Builder::new().name("rc-settings-backup".into()).spawn(move || {
            let dialog = rfd::FileDialog::new().set_title(title).add_filter("JSON", &["json"]);
            let path = if export.is_some() { dialog.set_file_name("crabcraft-preferences.json").save_file() } else { dialog.pick_file() };
            if token.load(Ordering::Acquire) {
                return;
            }
            let Some(path) = path else {
                out.send(Ok(Completed::Cancelled));
                return;
            };
            let result = match export {
                Some(backup) => backup.write(&path).map(|()| Completed::Exported),
                None => Backup::read(&path).map(Completed::Imported),
            };
            if !token.load(Ordering::Acquire) {
                out.send(result);
            }
        });
        match result {
            Ok(_) => self.job = Some(Job { inbox, cancelled }),
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, draft: &mut Settings, waker: Waker) {
        if ui.collapsing(tr("Import from another editor"), |ui| self.migration.ui(ui, draft, waker.clone())).body_returned.is_some() {
            return;
        }
        if let Some(result) = self.job.as_ref().and_then(|job| job.inbox.drain().into_iter().next()) {
            self.job = None;
            match result {
                Ok(Completed::Imported(preview)) => {
                    self.preview = Some(preview);
                    self.message = None;
                }
                Ok(Completed::Exported) => self.exported = true,
                Ok(Completed::Cancelled) => {}
                Err(error) => self.message = Some(error_text(&error)),
            }
        }
        ui.label(tr("Export appearance, editor preferences, templates, shortcuts and inspection levels to a JSON file. Import loads a preview; use Apply imported preferences, then OK to save."));
        ui.add_space(8.0);
        ui.label(tr("Trust decisions, project paths, history, tool programs and environment variables stay on this computer."));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(self.job.is_none() && !self.sync.busy(), egui::Button::new(tr("Export preferences"))).clicked() {
                match Backup::from_settings(draft) {
                    Ok(backup) => self.start(Some(backup), waker.clone()),
                    Err(error) => self.message = Some(error_text(&error)),
                }
            }
            if ui.add_enabled(self.job.is_none() && !self.sync.busy(), egui::Button::new(tr("Import preferences"))).clicked() {
                self.start(None, waker.clone());
            }
            if self.job.is_some() && ui.button(tr("Cancel")).clicked() {
                self.cancel();
            }
        });
        if self.job.is_some() {
            ui.spinner();
        }
        if self.exported {
            ui.label(tr("Preferences exported."));
        }
        if let Some(message) = &self.message {
            ui.colored_label(rc_kit::pal(ui).error, message);
        }
        if let Some(backup) = self.sync.ui(ui, draft, waker, self.job.is_none()) {
            self.preview = Some(backup);
        }
        if let Some(preview) = &self.preview {
            ui.add_space(12.0);
            ui.label(tr("Preferences to replace:"));
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                let fields: Vec<_> = preview.fields().collect();
                for (keys, label) in [
                    (
                        &[
                            "theme",
                            "language",
                            "editor_font_size",
                            "editor_line_spacing",
                            "ui_zoom",
                            "show_inlay_hints",
                            "show_indent_guides",
                            "sticky_lines",
                            "scrollbar_lens",
                            "right_margin",
                            "autosave",
                            "autosave_idle",
                            "autosave_delay_secs",
                            "single_click_open",
                            "tabs_multirow",
                            "tab_colors",
                        ][..],
                        tr("Appearance and editor preferences"),
                    ),
                    (&["live_templates"][..], tr("Live Templates")),
                    (&["keymap", "shortcuts"][..], tr("Keymap and shortcuts")),
                    (&["autoimport_exclude", "imports_on_paste"][..], tr("Auto-import preferences")),
                    (
                        &["inspection_profile", "use_clippy", "use_nextest", "format_on_save", "format_on_commit", "debugger_hex", "check_crates_io"][..],
                        tr("Inspection levels"),
                    ),
                ] {
                    if fields.iter().any(|field| keys.contains(field)) {
                        ui.label(label);
                    }
                }
            });
            if ui.button(tr("Apply imported preferences")).clicked() {
                match preview.apply(draft) {
                    Ok(settings) => {
                        *draft = settings;
                        self.preview = None;
                        self.message = None;
                    }
                    Err(error) => self.message = Some(error_text(&error)),
                }
            }
        }
    }
}
fn error_text(error: &TransferError) -> String {
    match error {
        TransferError::TooLarge => tr("The preferences backup exceeds 1 MB.").to_string(),
        TransferError::Version => tr("This preferences backup format or version is not supported.").to_string(),
        TransferError::Invalid => tr("The preferences backup contains invalid or unsupported settings.").to_string(),
        TransferError::Io(_) | TransferError::Json(_) => format!("{}: {error}", tr("Could not transfer preferences")),
    }
}
