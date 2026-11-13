//! The Cargo window: the members of the workspace with their targets, and a field to run any
//! Cargo command. Output goes to the Build window, binaries to the Run window. Each member also
//! shows its dependency tree from `cargo tree`, read when its node is first unfolded, and its
//! `[features]`, which can be turned on and off for the analysis.

use crate::app::{CrabCraftApp, FeatureState};
use crate::toolwindow::ToolWindow;
use crate::ui::run_windows::header;
use crate::ui::toolwindows::empty_state;
use egui::{Align2, FontId, Sense, Ui, pos2, vec2};
use rc_cargo::TargetKind;
use rc_kit::i18n::tr;
use rc_kit::{Ico, ico, icons};
use std::path::PathBuf;

/// What a background `cargo tree` sends back.
pub type TreeResult = (Result<Vec<rc_cargo::DepNode>, String>, Option<rc_cargo::LoadLog>);

/// The dependency tree of one package.
pub enum DepTree {
    Loading(rc_core::Inbox<TreeResult>),
    Ready(Vec<rc_cargo::DepNode>),
    Failed(String),
}

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
    if ui.button(tr("Manage Cargo Dependencies…")).clicked() {
        app.dialog = Some(crate::ui::dialogs::Dialog::CargoDependencies);
    }
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
    let mut load: Option<(PathBuf, String)> = None;
    let workspace_manifest = app.runner.cargo_workspace.as_ref().map(|w| w.manifest.clone()).unwrap_or_default();
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
            let key = (manifest.clone().unwrap_or_else(|| workspace_manifest.clone()), package.name.clone());
            features(app, ui, package, &key);
            if dependencies(app, ui, &key) {
                load = Some(key);
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
    if let Some((manifest, package)) = load {
        app.load_dep_tree(manifest, package);
    }
    if let Some((package, target, manifest)) = chosen {
        let ctx = ui.ctx().clone();
        match manifest {
            None => app.run_cargo_target(&ctx, &package, &target),
            Some(m) => app.run_attached_target(&ctx, &m, &package, &target),
        }
    }
}

/// The Features node of a member and, unfolded, a switch per feature.
fn features(app: &mut CrabCraftApp, ui: &mut Ui, package: &rc_cargo::Package, key: &(PathBuf, String)) {
    if package.features.is_empty() {
        return;
    }
    let id = format!("{}#{}#features", key.0.display(), key.1);
    let open = app.cargo_open.contains(&id);
    if row(ui, 1, Some(open), ico::SETTINGS, tr("Features"), "", true).clicked() {
        toggle(&mut app.cargo_open, &id);
    }
    if !open {
        return;
    }
    let mut change = None;
    for name in package.features.keys() {
        let Some(state) = app.feature_state(package, name) else { continue };
        let (on, note) = match state {
            FeatureState::Chosen => (true, ""),
            FeatureState::Implied => (true, tr("implied")),
            FeatureState::Off => (false, ""),
        };
        let response = feature_row(ui, name, note, on, state != FeatureState::Implied);
        if state == FeatureState::Implied {
            response.on_hover_text(tr("Turned on by default or by another enabled feature"));
        } else if response.on_hover_text(tr("Click to turn on or off for the analysis")).clicked() {
            change = Some((name.clone(), state == FeatureState::Off));
        }
    }
    if let Some((name, on)) = change {
        app.set_cargo_feature(&package.name, &name, on);
    }
}

/// A feature with its switch, two levels in.
fn feature_row(ui: &mut Ui, name: &str, note: &str, on: bool, enabled: bool) -> egui::Response {
    let p = rc_kit::pal(ui);
    let (r, response) = ui.allocate_exact_size(vec2(ui.available_width().max(200.0), 24.0), Sense::click());
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::Role::CheckBox, enabled, name);
        info.selected = Some(on);
        info
    });
    if response.hovered() && enabled {
        ui.painter().rect_filled(r.shrink2(vec2(6.0, 0.0)), 4.0, p.row_hover);
    }
    let x = r.left() + 14.0 + 2.0 * 18.0 + 20.0;
    icons::paint_switch(ui.painter(), egui::Rect::from_center_size(pos2(x + 8.0, r.center().y), vec2(16.0, 16.0)), on, &p);
    let text = ui.painter().text(pos2(x + 24.0, r.center().y), Align2::LEFT_CENTER, name, FontId::proportional(13.0), if enabled { p.text } else { p.muted });
    ui.painter().text(pos2(text.right() + 8.0, r.center().y), Align2::LEFT_CENTER, note, FontId::proportional(12.0), p.muted);
    response
}

/// The Dependencies node of a member and, unfolded, its tree. True when the tree must be read.
fn dependencies(app: &mut CrabCraftApp, ui: &mut Ui, key: &(PathBuf, String)) -> bool {
    let id = format!("{}#{}", key.0.display(), key.1);
    let open = app.cargo_open.contains(&id);
    if row(ui, 1, Some(open), ico::FOLDER, tr("Dependencies"), "", true).clicked() {
        toggle(&mut app.cargo_open, &id);
        return !open && !matches!(app.cargo_trees.get(key), Some(DepTree::Loading(_) | DepTree::Ready(_)));
    }
    if !open {
        return false;
    }
    let p = rc_kit::pal(ui);
    match app.cargo_trees.get(key) {
        None => false,
        Some(DepTree::Loading(_)) => {
            message(ui, tr("Loading…"), p.muted);
            false
        }
        Some(DepTree::Failed(e)) => {
            let first = e.lines().find(|l| !l.trim().is_empty()).unwrap_or(e.as_str()).to_string();
            message(ui, &first, p.error).on_hover_text(e.as_str());
            let mut retry = false;
            ui.horizontal(|ui| {
                ui.add_space(14.0 + 2.0 * 18.0 + 20.0);
                retry = ui.small_button(tr("Retry")).clicked();
            });
            retry
        }
        Some(DepTree::Ready(nodes)) => {
            let kids = rc_cargo::has_children(nodes);
            let mut toggled = None;
            // Rows below a folded node are hidden until the depth climbs back to it.
            let mut hidden_below: Option<usize> = None;
            for (i, node) in nodes.iter().enumerate().skip(1) {
                if hidden_below.is_some_and(|d| node.depth > d) {
                    continue;
                }
                hidden_below = None;
                let node_id = format!("{id}#{i}");
                let unfolded = app.cargo_open.contains(&node_id);
                let mut note = format!("v{}", node.version);
                match node.kind {
                    rc_cargo::DepKind::Normal => {}
                    rc_cargo::DepKind::Build => note += " · build",
                    rc_cargo::DepKind::Dev => note += " · dev",
                }
                if !node.detail.is_empty() {
                    note += &format!(" {}", short_detail(&node.detail, &key.0));
                }
                if node.repeated {
                    note += &format!(" · {}", tr("listed above"));
                }
                let response = row(ui, node.depth + 1, kids[i].then_some(unfolded), ico::PACKAGE, &node.name, &note, !node.repeated);
                if kids[i] && response.clicked() {
                    toggled = Some(node_id);
                }
                if !(kids[i] && unfolded) {
                    hidden_below = Some(node.depth);
                }
            }
            if let Some(node_id) = toggled {
                toggle(&mut app.cargo_open, &node_id);
            }
            false
        }
    }
}

/// A local path dependency's folder relative to the workspace, other details as Cargo wrote them.
fn short_detail(detail: &str, manifest: &std::path::Path) -> String {
    let inner = detail.strip_prefix('(').and_then(|d| d.strip_suffix(')'));
    let root = manifest.parent().unwrap_or(manifest);
    match inner.map(std::path::Path::new).and_then(|p| p.strip_prefix(root).ok()) {
        Some(rel) if !rel.as_os_str().is_empty() => format!("({})", rel.display()),
        _ => detail.to_string(),
    }
}

fn toggle(set: &mut std::collections::BTreeSet<String>, id: &str) {
    if !set.remove(id) {
        set.insert(id.to_string());
    }
}

fn message(ui: &mut Ui, text: &str, color: egui::Color32) -> egui::Response {
    let (r, response) = ui.allocate_exact_size(vec2(ui.available_width().max(200.0), 22.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 14.0 + 2.0 * 18.0 + 20.0, r.center().y), Align2::LEFT_CENTER, text, FontId::proportional(12.0), color);
    response
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
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, text));
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

#[cfg(test)]
mod tests {
    #[test]
    fn local_paths_are_shown_from_the_workspace() {
        let m = std::path::Path::new("/w/Cargo.toml");
        assert_eq!(super::short_detail("(/w/crates/beta)", m), "(crates/beta)");
        assert_eq!(super::short_detail("(/elsewhere/x)", m), "(/elsewhere/x)");
        assert_eq!(super::short_detail("(proc-macro)", m), "(proc-macro)");
    }
}
