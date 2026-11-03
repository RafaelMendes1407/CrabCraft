//! The Cargo window: the members of the workspace with their targets, and a field to run any
//! Cargo command. Output goes to the Build window, binaries to the Run window.

use crate::app::CrabCraftApp;
use crate::toolwindow::ToolWindow;
use crate::ui::run_windows::header;
use crate::ui::toolwindows::empty_state;
use egui::{Align2, FontId, Sense, Ui, pos2, vec2};
use rc_cargo::TargetKind;
use rc_kit::i18n::tr;
use rc_kit::{Ico, ico, icons};

pub fn show(app: &mut CrabCraftApp, ui: &mut Ui) {
    let running = app.runner.build.as_ref().is_some_and(|b| b.handle.is_some());
    let loaded = app.runner.cargo_workspace.is_some();
    let actions = [(ico::REFRESH, tr("Reload Cargo Project"), loaded), (ico::BUILD, tr("Build Project"), loaded && !running), (ico::STOP, tr("Cancel Build"), running)];
    match header(ui, "Cargo", &actions) {
        Some(0) => app.reload_cargo(),
        Some(1) => app.pending.push(rc_editor::Action::Build),
        Some(2) => app.runner.cancel_build(),
        Some(usize::MAX) => app.toggle_tool(ToolWindow::Cargo),
        _ => {}
    }
    let Some(own) = app.runner.cargo_workspace.as_ref().map(|w| w.packages.clone()) else {
        empty_state(ui, ToolWindow::Cargo.icon(), tr("No Cargo project"), tr("Open a folder with a Cargo.toml to see its members and targets."));
        return;
    };
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let width = ui.available_width() - 12.0;
        let field = ui.add(egui::TextEdit::singleline(&mut app.cargo_input).desired_width(width).hint_text(tr("Cargo command, then Enter")));
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let text = app.cargo_input.clone();
            app.run_cargo_command(&text);
        }
    });
    ui.add_space(4.0);
    // The workspace's packages, then those of the attached projects (with their manifest).
    let mut packages: Vec<(rc_cargo::Package, Option<std::path::PathBuf>)> = own.into_iter().map(|p| (p, None)).collect();
    for w in &app.runner.attached {
        packages.extend(w.packages.iter().cloned().map(|p| (p, Some(w.manifest.clone()))));
    }
    let mut chosen = None;
    egui::ScrollArea::both().id_salt("cargo_targets").auto_shrink([false, false]).show(ui, |ui| {
        for (package, manifest) in &packages {
            let folded = app.cargo_folded.contains(&package.name);
            let note = if manifest.is_some() { tr("attached") } else { "" };
            if row(ui, 0, Some(!folded), ico::PACKAGE, &package.name, note, true).clicked() {
                if folded {
                    app.cargo_folded.remove(&package.name);
                } else {
                    app.cargo_folded.insert(package.name.clone());
                }
            }
            if folded {
                continue;
            }
            for target in &package.targets {
                let (icon, kind) = look(target.kind);
                let response = row(ui, 1, None, icon, &target.name, kind, target.kind != TargetKind::Other);
                if target.kind == TargetKind::Other {
                    response.on_hover_text(tr("Cargo cannot run this target"));
                    continue;
                }
                let response = response.on_hover_text(tr("Double-click to run"));
                if response.double_clicked() {
                    chosen = Some((package.name.clone(), target.clone(), manifest.clone()));
                }
                response.context_menu(|ui| {
                    if ui.button(tr("Run")).clicked() {
                        chosen = Some((package.name.clone(), target.clone(), manifest.clone()));
                        ui.close();
                    }
                });
            }
        }
    });
    if let Some((package, target, manifest)) = chosen {
        let ctx = ui.ctx().clone();
        match manifest {
            None => app.run_cargo_target(&ctx, &package, &target),
            Some(m) => app.run_attached_target(&ctx, &m, &package, &target),
        }
    }
}

/// The icon and the short name of a kind of target.
fn look(kind: TargetKind) -> (Ico, &'static str) {
    match kind {
        TargetKind::Bin => (ico::RUN, "bin"),
        TargetKind::Lib => (ico::RUN_ALL, "lib"),
        TargetKind::Example => (ico::RUN, "example"),
        TargetKind::Test => (ico::RUN_ALL, "test"),
        TargetKind::Bench => (ico::RUN_ALL, "bench"),
        TargetKind::Other => (ico::BUILD, "build"),
    }
}

fn row(ui: &mut Ui, depth: usize, open: Option<bool>, icon: Ico, text: &str, note: &str, enabled: bool) -> egui::Response {
    let p = rc_kit::pal(ui);
    let (r, response) = ui.allocate_exact_size(vec2(ui.available_width().max(200.0), 24.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(r.shrink2(vec2(6.0, 0.0)), 4.0, p.row_hover);
    }
    let mut x = r.left() + 14.0 + depth as f32 * 18.0;
    if let Some(open) = open {
        let rect = egui::Rect::from_center_size(pos2(x + 8.0, r.center().y), vec2(16.0, 16.0));
        icons::paint(ui.painter(), rect, if open { ico::CHEVRON_DOWN } else { ico::CHEVRON_RIGHT }, &p, true);
    }
    x += 20.0;
    icons::paint(ui.painter(), egui::Rect::from_center_size(pos2(x + 8.0, r.center().y), vec2(16.0, 16.0)), icon, &p, enabled);
    x += 24.0;
    let color = if enabled { p.text } else { p.muted };
    let name = ui.painter().text(pos2(x, r.center().y), Align2::LEFT_CENTER, text, FontId::proportional(13.0), color);
    ui.painter().text(pos2(name.right() + 8.0, r.center().y), Align2::LEFT_CENTER, note, FontId::proportional(12.0), p.muted);
    response
}
