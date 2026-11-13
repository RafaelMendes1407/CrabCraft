//! New Project: a template on the left, name, location and Git on the right. `cargo new` runs in
//! the background through `rc_cargo::new_project`.

use crate::app::CrabCraftApp;
use egui::{Align2, Context, FontId, Id, Rect, Sense, pos2, vec2};
use rc_cargo::new_project::{NewProject, Template, WASM_TARGET, WasmTools};
use rc_core::{Inbox, Runtime, Waker};
use rc_kit::i18n::{tr, trf};
use rc_kit::{Ico, ico, icons, theme};
use std::path::PathBuf;

/// Name, description and icon of each template.
const TEMPLATES: [(Template, &str, &str, Ico); 4] = [
    (Template::Binary, "Binary", "An executable program with src/main.rs", ico::CONFIG_APP),
    (Template::Library, "Library", "A reusable library with src/lib.rs", ico::LIBRARY),
    (Template::ProcMacro, "Procedural Macro", "A derive macro built with syn and quote, and an integration test in tests/", ico::ANNOTATION),
    (Template::WebAssembly, "WebAssembly", "A cdylib library for the web with wasm-bindgen", ico::CONFIG_WEB),
];

pub struct NewProjectDialog {
    template: usize,
    repository: String,
    template_branch: String,
    trust_template: bool,
    cancel: Option<rc_dotnet::CancelToken>,
    name: String,
    location: String,
    git: bool,
    /// Adding a crate to the open project instead of opening a new one.
    member: bool,
    busy: bool,
    error: Option<String>,
    inbox: Inbox<Result<PathBuf, String>>,
    focus: bool,
    /// The WebAssembly tools, looked for in the background the first time that template is shown.
    wasm_tools: Option<WasmTools>,
    wasm_asked: bool,
    wasm_inbox: Inbox<WasmTools>,
}

/// The folder new projects go to: the last one used, next to the last workspace, or the home.
fn default_location(app: &CrabCraftApp) -> PathBuf {
    if let Some(p) = app.settings.new_project_location.clone().filter(|p| p.is_dir()) {
        return p;
    }
    if let Some(parent) = app.settings.recent_workspaces.first().and_then(|w| w.parent()).filter(|p| p.is_dir()) {
        return parent.to_path_buf();
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    ["Projects", "RustroverProjects"].iter().map(|d| home.join(d)).find(|p| p.is_dir()).unwrap_or(home)
}

impl NewProjectDialog {
    pub fn new(app: &CrabCraftApp, waker: Waker) -> Self {
        let location = default_location(app);
        let name = (1..).map(|i| format!("project{i}")).find(|n| !location.join(n).exists()).unwrap_or_default();
        NewProjectDialog::with(0, name, location.display().to_string(), true, false, waker)
    }

    fn with(template: usize, name: String, location: String, git: bool, member: bool, waker: Waker) -> Self {
        NewProjectDialog {
            template,
            repository: String::new(),
            template_branch: String::new(),
            trust_template: false,
            cancel: None,
            name,
            location,
            git,
            member,
            busy: false,
            error: None,
            inbox: Inbox::new(waker.clone()),
            focus: true,
            wasm_tools: None,
            wasm_asked: false,
            wasm_inbox: Inbox::new(waker),
        }
    }

    /// A new crate in `location`, inside the open project.
    pub fn member(location: &std::path::Path, waker: Waker) -> Self {
        let name = (1..).map(|i| format!("crate{i}")).find(|n| !location.join(n).exists()).unwrap_or_default();
        NewProjectDialog::with(1, name, location.display().to_string(), false, true, waker)
    }

    fn generated(&self) -> bool { self.template == TEMPLATES.len() }

    fn generate_request(&self) -> rc_cargo::generate::GenerateProject {
        rc_cargo::generate::GenerateProject {
            repository: self.repository.trim().to_string(), branch: self.template_branch.trim().to_string(),
            name: self.name.trim().to_string(), location: PathBuf::from(self.location.trim()), git: self.git, trusted: self.trust_template,
        }
    }

    fn request(&self) -> NewProject {
        NewProject { template: TEMPLATES[self.template.min(TEMPLATES.len() - 1)].0, name: self.name.trim().to_string(), location: PathBuf::from(self.location.trim()), git: self.git }
    }
}

impl Drop for NewProjectDialog {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() { cancel.cancel(); }
    }
}

fn generate_error(error: rc_cargo::generate::GenerateError) -> String {
    use rc_cargo::generate::GenerateError;
    match error {
        GenerateError::Invalid(message) => tr(message).to_string(),
        GenerateError::Io(error) => trf("Could not start cargo-generate: {error}", &[("error", &error)]),
        GenerateError::Failed { code, output } => trf("Template generation failed ({code}): {output}", &[("code", &code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())), ("output", &output)]),
        GenerateError::Cancelled => tr("Template generation cancelled").to_string(),
        GenerateError::MissingManifest(path) => trf("The template did not create a Cargo.toml in {path}", &[("path", &path.display())]),
    }
}

/// Shows the dialog; returns false when it closed.
pub fn show(app: &mut CrabCraftApp, ctx: &Context, d: &mut NewProjectDialog) -> bool {
    let mut open = true;
    for result in d.inbox.drain() {
        d.busy = false;
        d.cancel = None;
        match result {
            Ok(dir) => {
                if d.member {
                    app.reload_workspace();
                } else {
                    app.settings.new_project_location = Some(PathBuf::from(d.location.trim()));
                    app.open_workspace(&dir);
                }
                let main = ["src/main.rs", "src/lib.rs"].iter().map(|f| dir.join(f)).find(|p| p.exists());
                if let Some(f) = main {
                    app.open_file(&f);
                }
                app.notify(trf("{name} created", &[("name", &d.name.trim())]), false);
                return false;
            }
            Err(e) => d.error = Some(e),
        }
    }
    if let Some(tools) = d.wasm_inbox.drain().pop() {
        d.wasm_tools = Some(tools);
    }
    if !d.generated() && TEMPLATES[d.template].0 == Template::WebAssembly && !d.wasm_asked {
        d.wasm_asked = true;
        if let Some(cargo) = rc_cargo::Cargo::locate() {
            let out = d.wasm_inbox.sender();
            std::thread::spawn(move || out.send(WasmTools::detect(&cargo)));
        }
    }
    if d.busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
    }
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let req = d.request();
    let generated_request = d.generate_request();
    let problem = if d.generated() { generated_request.problem() } else { req.problem() }.map(tr);
    let mut create = false;
    let resp = egui::Modal::new(Id::unique("new_project")).show(ctx, |ui| {
        ui.set_width(700.0);
        ui.set_height(if d.generated() { 490.0 } else { 330.0 });
        ui.label(egui::RichText::new(tr(if d.member { "New Crate" } else { "New Project" })).font(theme::bold(15.0)));
        ui.add_space(10.0);
        let avail = ui.available_rect_before_wrap();
        let left = Rect::from_min_size(avail.min, vec2(200.0, avail.height() - 44.0));
        let right = Rect::from_min_max(pos2(left.right() + 18.0, avail.top()), pos2(avail.right(), avail.bottom() - 44.0));
        ui.painter().rect_filled(left, 8.0, p.main_bg);
        let mut lu = ui.new_child(egui::UiBuilder::new().max_rect(left.shrink(4.0)));
        lu.spacing_mut().item_spacing.y = 2.0;
        let mut choices = TEMPLATES.to_vec();
        if !d.member { choices.push((Template::Binary, "Git Template", "Generate a Rust project from a Git repository with cargo-generate", ico::CONFIG_APP)); }
        for (i, (_, name, _, icon)) in choices.iter().enumerate() {
            let (r, resp) = lu.allocate_exact_size(vec2(lu.available_width(), 30.0), Sense::click());
            if i == d.template {
                lu.painter().rect_filled(r, 6.0, p.selection);
            } else if resp.hovered() {
                lu.painter().rect_filled(r, 6.0, p.row_hover);
            }
            icons::paint(lu.painter(), Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(16.0, 16.0)), *icon, &p, true);
            lu.painter().text(pos2(r.left() + 32.0, r.center().y), Align2::LEFT_CENTER, tr(name), FontId::proportional(13.0), p.text);
            if resp.clicked() && !d.busy {
                d.template = i;
            }
            resp.widget_info(|| {
                let mut info = egui::WidgetInfo::labeled(egui::Role::Button, true, tr(name));
                info.selected = Some(i == d.template);
                info
            });

        }
        let mut ru = ui.new_child(egui::UiBuilder::new().max_rect(right));
        let (_, name, description, _) = &choices[d.template];
        ru.label(egui::RichText::new(tr(name)).font(theme::bold(14.0)));
        ru.label(egui::RichText::new(tr(description)).size(12.5).color(p.muted));
        ru.add_space(12.0);
        egui::Grid::new("new_project_grid").num_columns(2).spacing([12.0, 10.0]).show(&mut ru, |ui| {
            let w = 330.0;
            ui.label(tr(if d.member { "Crate name" } else { "Project name" }));
            let te = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(w));
            if std::mem::take(&mut d.focus) {
                te.request_focus();
            }
            ui.end_row();
            ui.label(tr("Location"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.location).desired_width(w - 34.0));
                if theme::icon_button(ui, ico::OPEN, true, tr("Choose a folder")).clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_directory(d.location.trim()).pick_folder()
                {
                    d.location = dir.display().to_string();
                }
            });
            ui.end_row();
            if d.generated() {
                ui.label(tr("Template repository"));
                if ui.add_enabled(!d.busy, egui::TextEdit::singleline(&mut d.repository).desired_width(w)).changed() { d.trust_template = false; }
                ui.end_row();
                ui.label(tr("Template branch (optional)"));
                if ui.add_enabled(!d.busy, egui::TextEdit::singleline(&mut d.template_branch).desired_width(w)).changed() { d.trust_template = false; }
                ui.end_row();
            }
            if !d.member {
                ui.label("");
                ui.checkbox(&mut d.git, tr("Create Git repository"));
                ui.end_row();
            }
        });
        if d.generated() {
            ru.add_space(6.0);
            ru.label(egui::RichText::new(tr("Requires cargo-generate 0.26 or newer. Install with: cargo install --locked cargo-generate")).size(12.0).color(p.muted));
            ru.label(egui::RichText::new(tr("Templates may run hook scripts. Review the repository before trusting it. Shell commands and Git submodules are disabled; custom placeholders must have defaults.")).size(12.0).color(p.warn));
            ru.add_enabled_ui(!d.busy, |ui| { ui.checkbox(&mut d.trust_template, tr("I trust this template repository")); });
        }
        ru.add_space(14.0);
        if d.member && !rc_cargo::modules::in_workspace(std::path::Path::new(d.location.trim())) {
            ru.label(egui::RichText::new(tr("No [workspace] here: the crate will not join the project")).size(12.0).color(p.warn));
        }
        if !d.generated() && TEMPLATES[d.template].0 == Template::WebAssembly {
            match d.wasm_tools {
                None => {
                    ru.label(egui::RichText::new(tr("Looking for the WebAssembly tools…")).size(12.0).color(p.muted));
                }
                Some(tools) => {
                    if !tools.target {
                        let text = trf("The {target} target is not installed. Add it with: rustup target add {target}", &[("target", &WASM_TARGET)]);
                        ru.label(egui::RichText::new(text).size(12.0).color(p.warn));
                    }
                    if !tools.wasm_pack {
                        ru.label(egui::RichText::new(tr("wasm-pack was not found: install it to package the library for JavaScript")).size(12.0).color(p.warn));
                    }
                }
            }
        }
        ru.label(egui::RichText::new(trf("The project will be created in {path}", &[("path", &req.project_dir().display())])).size(12.0).color(p.muted));
        if let Some(e) = d.error.as_deref().or(problem) {
            ru.add_space(6.0);
            ru.label(egui::RichText::new(e).size(12.5).color(p.error));
        }
        if d.busy {
            ru.add_space(6.0);
            ru.add(egui::Spinner::new().size(14.0));
        }
        let bar = Rect::from_min_max(pos2(avail.left(), avail.bottom() - 32.0), avail.max);
        let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)));
        if bu.add_enabled_ui(problem.is_none() && !d.busy, |ui| theme::primary_button(ui, tr("Create"))).inner.clicked() {
            create = true;
        }
        if theme::secondary_button(&mut bu, tr("Cancel")).clicked() {
            open = false;
        }
        ui.advance_cursor_after_rect(avail);
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) && problem.is_none() && !d.busy {
            create = true;
        }
    });
    if resp.should_close() && !d.busy {
        open = false;
    }
    if create && d.generated() {
        let generated_request = d.generate_request();
        d.error = None;
        d.busy = true;
        let out = d.inbox.sender();
        let cancel = rc_dotnet::CancelToken::new();
        d.cancel = Some(cancel.clone());
        Runtime::spawn(async move {
            // Tool detection and all Git/template work happen off the UI thread.
            let result = match rc_cargo::toolchain::find_tool("cargo-generate") {
                Some(program) => generated_request.create(&program, &cancel).await.map_err(generate_error),
                None => Err(tr("cargo-generate was not found. Install it with: cargo install --locked cargo-generate").to_string()),
            };
            out.send(result);
        });
    } else if create {
        let Some(cargo) = rc_cargo::Cargo::locate() else {
            d.error = Some(tr("Cargo was not found. Install Rust with rustup.").into());
            return open;
        };
        d.error = None;
        d.busy = true;
        let out = d.inbox.sender();
        Runtime::spawn(async move {
            out.send(req.create(&cargo).await.map_err(|e| format!("{e:#}")));
        });
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn git_template_dialog_requires_explicit_trust() {
        let _guard = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.new_project_dialog();
        let mut harness = crate::screens::harness(app, vec2(1100.0, 850.0));
        harness.get_by_label("Git Template").click();
        harness.run_steps(3);
        harness.get_by_label("I trust this template repository");
        harness.get_by_label("Template repository");
        let dialog = harness.state_mut().new_project.as_mut().unwrap();
        assert!(dialog.generated());
        dialog.repository = "https://example.invalid/template".into();
        dialog.location = home.path().display().to_string();
        assert_eq!(dialog.generate_request().problem(), Some("Trust the template before generating a project"));
        harness.get_by_label("I trust this template repository").click();
        harness.run_steps(3);
        assert!(harness.state().new_project.as_ref().unwrap().trust_template);
        assert!(harness.state().new_project.as_ref().unwrap().generate_request().problem().is_none());
        harness.get_by_label("Cancel").click();
        harness.run_steps(3);
        assert!(harness.state().new_project.is_none());
        assert!(harness.state().workspace.is_none());
    }
}
