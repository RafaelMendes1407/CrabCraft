//! Reviewed import of portable preferences from another editor.
use rc_core::{
    Inbox, Settings, Waker,
    editor_import::{self, Error, Preview, Source},
};
use rc_kit::i18n::{tr, trf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct Job {
    inbox: Inbox<Result<Preview, Error>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
pub struct State {
    pub source: Source,
    pub path: String,
    job: Option<Job>,
    preview: Option<Preview>,
    error: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        Self { source: Source::VsCode, path: String::new(), job: None, preview: None, error: None }
    }
}
impl State {
    pub fn cancel(&mut self) {
        self.job = None;
        self.preview = None;
        self.error = None;
    }
    fn start(&mut self, waker: Waker) {
        self.cancel();
        if self.path.len() > 4096 || self.path.trim().is_empty() {
            self.error = Some(tr("Choose a settings.json or RustRover options XML file").to_string());
            return;
        }
        let path = std::path::PathBuf::from(self.path.trim());
        let source = self.source;
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let result = std::thread::Builder::new().name("rc-editor-preferences-import".into()).spawn(move || {
            out.send(editor_import::read(source, &path, &token));
        });
        match result {
            Ok(_) => self.job = Some(Job { inbox, cancel }),
            Err(error) => self.error = Some(trf("Could not read editor preferences: {error}", &[("error", &error)])),
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, draft: &mut Settings, waker: Waker) {
        if let Some(result) = self.job.as_ref().and_then(|job| job.inbox.drain().into_iter().next()) {
            self.job = None;
            match result {
                Ok(preview) => self.preview = Some(preview),
                Err(Error::Cancelled) => (),
                Err(error) => self.error = Some(error_text(error)),
            }
        }
        ui.label(tr("Import portable editor preferences with a preview. Commands, extensions, credentials and project trust are never imported."));
        let mut changed = false;
        ui.horizontal(|ui| {
            let label = ui.label(tr("Source editor"));
            egui::ComboBox::from_id_salt("migration_source")
                .selected_text(match self.source {
                    Source::VsCode => "VS Code",
                    Source::RustRover => "RustRover",
                })
                .show_ui(ui, |ui| {
                    changed |= ui.selectable_value(&mut self.source, Source::VsCode, "VS Code").changed();
                    changed |= ui.selectable_value(&mut self.source, Source::RustRover, "RustRover").changed();
                })
                .response
                .labelled_by(label.id);
        });
        let label = ui.label(tr("Source preferences file"));
        changed |= ui.add(egui::TextEdit::singleline(&mut self.path).char_limit(4096).desired_width(f32::INFINITY)).labelled_by(label.id).changed();
        if changed {
            self.cancel();
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(self.job.is_none(), egui::Button::new(tr("Read editor preferences"))).clicked() {
                self.start(waker);
            }
            if self.job.is_some() {
                ui.spinner();
                if ui.button(tr("Cancel")).clicked() {
                    self.cancel();
                }
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(rc_kit::pal(ui).error, error);
        }
        let mut apply = false;
        if let Some(preview) = &self.preview {
            ui.label(tr("Preferences to replace:"));
            egui::ScrollArea::vertical().id_salt("migration_preview").max_height(160.0).show(ui, |ui| {
                for (key, value) in &preview.changes {
                    ui.label(format!("{}: {value}", field(key)));
                }
                if !preview.ignored.is_empty() {
                    ui.label(tr("Unsupported preferences (left unchanged)"));
                    for key in &preview.ignored {
                        ui.monospace(key);
                    }
                }
            });
            apply = ui.button(tr("Apply imported editor preferences")).clicked();
        }
        if apply && let Some(preview) = self.preview.take() {
            match preview.apply(draft) {
                Ok(imported) => {
                    *draft = imported;
                }
                Err(error) => self.error = Some(error_text(error)),
            }
        }
    }
}
fn field(key: &str) -> &'static str {
    match key {
        "editor_font_size" => tr("Font size"),
        "editor_line_spacing" => tr("Line spacing"),
        "theme" => tr("Theme"),
        "keymap" => tr("Keymap"),
        "ui_zoom" => tr("Interface zoom"),
        "show_indent_guides" => tr("Indent guides"),
        "sticky_lines" => tr("Sticky lines"),
        "right_margin" => tr("Right margin"),
        "autosave" => tr("Autosave"),
        "format_on_save" => tr("Format on save"),
        "show_inlay_hints" => tr("Inlay hints"),
        _ => tr("Editor preferences"),
    }
}
fn error_text(error: Error) -> String {
    match error {
        Error::Io(error) => trf("Could not read editor preferences: {error}", &[("error", &error)]),
        Error::Cancelled => tr("Cancelled").into(),
        Error::Format => tr("Choose a settings.json or RustRover options XML file").into(),
        Error::Value => tr("The source preferences contain an unsupported value").into(),
        Error::Limit => tr("The editor preferences exceed the import limits").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    #[test]
    fn native_import_previews_applies_only_after_review_and_preserves_project_trust() {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for source in [Source::VsCode, Source::RustRover] {
            let home = tempfile::tempdir().unwrap();
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir(root.path().join("src")).unwrap();
            std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"migration_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n")
                .unwrap();
            let rust = "fn main() { assert_eq!(2+2,4); }\n";
            let rust_path = root.path().join("src/main.rs");
            std::fs::write(&rust_path, rust).unwrap();
            std::fs::write(root.path().join("build.rs"), "fn main() { std::fs::write(\"executed\",\"yes\").unwrap(); }\n").unwrap();
            let input = root.path().join(if source == Source::VsCode { "settings.json" } else { "editor-font.xml" });
            let vscode = r#"{"editor.fontSize":18,"workbench.colorTheme":"Default Light Modern","rust-analyzer.server.path":"./danger","security.workspace.trust.enabled":false}"#;
            let xml = r#"<application><component name="DefaultFont"><option name="FONT_SIZE" value="18"/></component><component name="EditorColorsManager"><global_color_scheme name="Default"/></component><component name="Unknown"><option name="TRUST" value="true"/></component></application>"#;
            std::fs::write(&input, if source == Source::VsCode { vscode } else { xml }).unwrap();
            let mut app = crate::screens::test_app(home.path(), false);
            app.open_workspace_and_wait(root.path());
            app.open_file_and_wait(&rust_path);
            app.settings_backup.migration.source = source;
            app.settings_backup.migration.path = input.display().to_string();
            app.open_settings(crate::ui::dialogs::SettingsPage::Backup);
            let mut h = crate::screens::harness(app, egui::vec2(1280.0, 900.0));
            h.run_steps(2);
            h.get_by_label(tr("Import from another editor")).click();
            h.run_steps(2);
            h.get_by_label(tr("Read editor preferences")).click();
            h.run_steps(2);
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while h.state().settings_backup.migration.preview.is_none() {
                h.run_steps(1);
                assert!(std::time::Instant::now() < end);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_ne!(h.state().settings.editor_font_size, 18.0);
            h.get_by_label(tr("Apply imported editor preferences")).click();
            h.run_steps(2);
            assert!(
                matches!(&h.state().dialog,Some(crate::ui::dialogs::Dialog::Settings{draft,..}) if draft.editor_font_size==18.0),
                "import must update the settings draft"
            );
            assert_ne!(h.state().settings.editor_font_size, 18.0);
            h.get_by_label(tr("OK")).click();
            h.run_steps(2);
            assert_eq!(h.state().settings.editor_font_size, 18.0);
            assert_eq!(
                h.state().settings.keymap,
                if source == Source::VsCode { rc_core::settings::KeymapChoice::VsCode } else { rc_core::settings::KeymapChoice::RustRover }
            );
            assert!(h.state().settings.trusted_projects.is_empty());
            assert!(h.state().settings.rust_analyzer_path.is_none());
            assert!(!root.path().join("executed").exists());
            assert_eq!(std::fs::read_to_string(&rust_path).unwrap(), rust);
            let saved = Settings::load();
            assert_eq!(saved.editor_font_size, 18.0);
            assert_eq!(saved.theme, rc_core::settings::ThemeChoice::Light);
            let output = std::process::Command::new("cargo").args(["run", "--offline", "--quiet"]).current_dir(root.path()).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            assert!(root.path().join("executed").exists());
        }
    }
}
