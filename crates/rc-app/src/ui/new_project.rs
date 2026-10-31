//! New Project: a template on the left, name, location and Git on the right. `cargo new` runs in
//! the background through `rc_cargo::new_project`.

use crate::app::CrabCraftApp;
use egui::{Align2, Context, FontId, Id, Rect, Sense, pos2, vec2};
use rc_cargo::new_project::{NewProject, Template};
use rc_core::{Inbox, Runtime, Waker};
use rc_kit::i18n::{tr, trf};
use rc_kit::{Ico, ico, icons, theme};
use std::path::PathBuf;

/// Name, description and icon of each template.
const TEMPLATES: [(Template, &str, &str, Ico); 2] = [
    (Template::Binary, "Binary", "An executable program with src/main.rs", ico::CONFIG_APP),
    (Template::Library, "Library", "A reusable library with src/lib.rs", ico::LIBRARY),
];

pub struct NewProjectDialog {
    template: usize,
    name: String,
    location: String,
    git: bool,
    busy: bool,
    error: Option<String>,
    inbox: Inbox<Result<PathBuf, String>>,
    focus: bool,
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
        NewProjectDialog { template: 0, name, location: location.display().to_string(), git: true, busy: false, error: None, inbox: Inbox::new(waker), focus: true }
    }

    fn request(&self) -> NewProject {
        NewProject { template: TEMPLATES[self.template].0, name: self.name.trim().to_string(), location: PathBuf::from(self.location.trim()), git: self.git }
    }
}

/// Shows the dialog; returns false when it closed.
pub fn show(app: &mut CrabCraftApp, ctx: &Context, d: &mut NewProjectDialog) -> bool {
    let mut open = true;
    for result in d.inbox.drain() {
        d.busy = false;
        match result {
            Ok(dir) => {
                app.settings.new_project_location = Some(PathBuf::from(d.location.trim()));
                app.open_workspace(&dir);
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
    if d.busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
    }
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let req = d.request();
    let problem = req.problem().map(tr);
    let mut create = false;
    let resp = egui::Modal::new(Id::new("new_project")).show(ctx, |ui| {
        ui.set_width(700.0);
        ui.set_height(330.0);
        ui.label(egui::RichText::new(tr("New Project")).font(theme::bold(15.0)));
        ui.add_space(10.0);
        let avail = ui.available_rect_before_wrap();
        let left = Rect::from_min_size(avail.min, vec2(200.0, avail.height() - 44.0));
        let right = Rect::from_min_max(pos2(left.right() + 18.0, avail.top()), pos2(avail.right(), avail.bottom() - 44.0));
        ui.painter().rect_filled(left, 8.0, p.main_bg);
        let mut lu = ui.new_child(egui::UiBuilder::new().max_rect(left.shrink(4.0)));
        lu.spacing_mut().item_spacing.y = 2.0;
        for (i, (_, name, _, icon)) in TEMPLATES.iter().enumerate() {
            let (r, resp) = lu.allocate_exact_size(vec2(lu.available_width(), 30.0), Sense::click());
            if i == d.template {
                lu.painter().rect_filled(r, 6.0, p.selection);
            } else if resp.hovered() {
                lu.painter().rect_filled(r, 6.0, p.row_hover);
            }
            icons::paint(lu.painter(), Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(16.0, 16.0)), *icon, &p, true);
            lu.painter().text(pos2(r.left() + 32.0, r.center().y), Align2::LEFT_CENTER, tr(name), FontId::proportional(13.0), p.text);
            if resp.clicked() {
                d.template = i;
            }
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, i == d.template, tr(name)));
        }
        let mut ru = ui.new_child(egui::UiBuilder::new().max_rect(right));
        let (_, name, description, _) = &TEMPLATES[d.template];
        ru.label(egui::RichText::new(tr(name)).font(theme::bold(14.0)));
        ru.label(egui::RichText::new(tr(description)).size(12.5).color(p.muted));
        ru.add_space(12.0);
        egui::Grid::new("new_project_grid").num_columns(2).spacing([12.0, 10.0]).show(&mut ru, |ui| {
            let w = 330.0;
            ui.label(tr("Project name"));
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
            ui.label("");
            ui.checkbox(&mut d.git, tr("Create Git repository"));
            ui.end_row();
        });
        ru.add_space(14.0);
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
    if create {
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
