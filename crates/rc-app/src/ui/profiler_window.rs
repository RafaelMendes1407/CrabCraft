//! The Profiler window (A-01): a flame graph and a call tree (total and self time) of the last
//! perf recording. Clicking a frame of the flame graph zooms into it; double-clicking a frame or
//! a row opens its source when the binary's debug info names it.

use crate::app::CrabCraftApp;
use crate::profiler::{Profile, Stage};
use crate::toolwindow::ToolWindow;
use crate::ui::run_windows::header;
use crate::ui::toolwindows::empty_state;
use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use rc_kit::i18n::{tr, trf};
use rc_kit::{ico, theme};

const BAR_H: f32 = 18.0;
const ROW_H: f32 = 22.0;

pub fn show(app: &mut CrabCraftApp, ui: &mut Ui) {
    let busy = app.profiler.busy();
    let recording = app.profiler.stage() == Some(Stage::Recording);
    let can_start = !busy && app.runner.selected_config().is_some_and(|c| matches!(c.kind, crate::runner::ConfigKind::Cargo { .. }));
    let actions = [(ico::RUN, tr("Profile"), can_start), (ico::STOP, tr("Stop and Analyze"), recording), (ico::KILL, tr("Cancel"), busy)];
    match header(ui, ToolWindow::Profiler.title(), &actions) {
        Some(0) => app.start_profiling(),
        Some(1) => app.profiler.stop(),
        Some(2) => app.profiler.cancel(),
        Some(usize::MAX) => app.toggle_tool(ToolWindow::Profiler),
        _ => {}
    }
    let p = rc_kit::pal(ui);
    if let Some(stage) = app.profiler.stage() {
        let text = match stage {
            Stage::Building => trf("Building {name}…", &[("name", &app.profiler.name)]),
            Stage::Recording => trf("Recording {name} with perf… Stop and Analyze ends it early.", &[("name", &app.profiler.name)]),
            Stage::Analyzing => tr("Reading the samples…").to_string(),
        };
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(RichText::new(text).color(p.text_muted));
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        return;
    }
    let Some(result) = &app.profiler.result else {
        let (title, hint) = (tr("No profile yet"), tr("Choose the run configuration of a Rust binary and click Profile. Uses perf on Linux."));
        empty_state(ui, ToolWindow::Profiler.icon(), title, hint);
        return;
    };
    let profile = match result {
        Ok(_) => match app.profiler.result.take() {
            Some(Ok(profile)) => profile,
            _ => return,
        },
        Err(e) => {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.add(egui::Label::new(RichText::new(e.text()).color(p.error)).selectable(true).wrap());
            });
            if !app.profiler.output.trim().is_empty() {
                ui.add_space(6.0);
                egui::ScrollArea::vertical().id_salt("profiler_output").auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(app.profiler.output.trim()).monospace().size(12.0).color(p.text_muted)).selectable(true));
                });
            }
            return;
        }
    };
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if theme::tab(ui, !app.profiler.show_tree, None, tr("Flame Graph"), false).0.clicked() {
            app.profiler.show_tree = false;
        }
        if theme::tab(ui, app.profiler.show_tree, None, tr("Call Tree"), false).0.clicked() {
            app.profiler.show_tree = true;
        }
        let mut info = trf("{name} · {n} samples", &[("name", &app.profiler.name), ("n", &profile.samples)]);
        if profile.truncated {
            info.push_str(" · ");
            info.push_str(tr("only part of the samples was read"));
        }
        ui.label(RichText::new(info).size(12.5).color(p.muted));
        if !app.profiler.show_tree && app.profiler.zoom != 0 && ui.link(tr("Reset Zoom")).clicked() {
            app.profiler.zoom = 0;
        }
    });
    let open = if app.profiler.show_tree { call_tree(app, ui, &profile) } else { flame_graph(app, ui, &profile) };
    // Taken out to draw it without a copy; a new run started meanwhile keeps its own result.
    if app.profiler.result.is_none() && !app.profiler.busy() {
        app.profiler.result = Some(Ok(profile));
    }
    if let Some((path, line)) = open {
        app.open_at(&path, line.saturating_sub(1) as usize, 0);
    }
}

/// A colour per function: warm for the program's own code, grey-blue for libraries.
fn colour(profile: &Profile, node: usize, dark: bool) -> Color32 {
    let frame = profile.nodes[node].frame.map(|f| &profile.frames[f]);
    let hash = frame.map_or(0u32, |f| f.name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619)));
    let jitter = (hash % 40) as u8;
    let (r, g, b) = match frame {
        Some(f) if f.own => (215u8.saturating_add(jitter), 110u8.saturating_add(jitter * 2), 50u8),
        Some(_) => (120u8 + jitter, 135u8 + jitter, 160u8 + jitter),
        None => (150, 150, 150),
    };
    let c = Color32::from_rgb(r, g, b);
    if dark { c.gamma_multiply(0.85) } else { c }
}

fn percent(part: u64, whole: u64) -> String {
    if whole == 0 { "0%".into() } else { format!("{:.1}%", part as f64 * 100.0 / whole as f64) }
}

/// Root at the top, callees below; widths are total time. Returns a source to open.
fn flame_graph(app: &mut CrabCraftApp, ui: &mut Ui, profile: &Profile) -> Option<(std::path::PathBuf, u32)> {
    let p = rc_kit::pal(ui);
    let zoom = if app.profiler.zoom < profile.nodes.len() { app.profiler.zoom } else { 0 };
    let base_depth = profile.nodes[zoom].depth;
    let levels = profile.depth() - base_depth + 1;
    let mut open = None;
    let mut clicked = None;
    egui::ScrollArea::both().id_salt("flame_graph").auto_shrink([false, false]).show(ui, |ui| {
        let width = ui.available_width().max(200.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, levels as f32 * BAR_H + 4.0), Sense::hover());
        let clip = ui.clip_rect();
        let total = profile.nodes[zoom].total.max(1) as f32;
        // (node, x, depth below the zoomed node)
        let mut todo = vec![(zoom, rect.left(), 0usize)];
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let (primary, double) = ui.input(|i| (i.pointer.primary_clicked(), i.pointer.button_double_clicked(egui::PointerButton::Primary)));
        let mut hovered = None;
        while let Some((node, x, level)) = todo.pop() {
            let n = &profile.nodes[node];
            let w = n.total as f32 / total * width;
            if w < 0.75 {
                continue;
            }
            let r = Rect::from_min_size(pos2(x, rect.top() + level as f32 * BAR_H), vec2(w, BAR_H - 1.0));
            if r.intersects(clip) {
                ui.painter().rect_filled(r, 2.0, colour(profile, node, p.dark));
                if w > 30.0 {
                    let name = profile.name(node);
                    let chars = ((w - 6.0) / 6.6) as usize;
                    let label = if name.chars().count() > chars {
                        format!("{}…", name.chars().take(chars.saturating_sub(1)).collect::<String>())
                    } else {
                        name.to_string()
                    };
                    ui.painter().with_clip_rect(r).text(
                        pos2(r.left() + 4.0, r.center().y),
                        Align2::LEFT_CENTER,
                        label,
                        FontId::proportional(11.5),
                        Color32::from_rgb(20, 20, 20),
                    );
                }
                if pointer.is_some_and(|pt| r.contains(pt)) {
                    hovered = Some(node);
                    ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, p.text), egui::StrokeKind::Inside);
                }
            }
            let mut cx = x;
            for &c in &n.children {
                todo.push((c, cx, level + 1));
                cx += profile.nodes[c].total as f32 / total * width;
            }
        }
        if let Some(node) = hovered {
            let n = &profile.nodes[node];
            let all = profile.total();
            let mut text = format!(
                "{}\n{}",
                profile.name(node),
                trf("Total {total} · Self {self}", &[("total", &percent(n.total, all)), ("self", &percent(n.self_, all))])
            );
            if let Some(f) = n.frame {
                text.push_str(&format!("\n{}", profile.frames[f].dso));
            }
            if profile.source(node).is_some() {
                text.push_str(&format!("\n{}", tr("Double-click to open the source")));
            }
            egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::unique("flame_tip"), egui::PopupAnchor::Pointer).show(|ui| {
                ui.label(text);
            });
            if double {
                open = profile.source(node).cloned();
            } else if primary {
                clicked = Some(node);
            }
        }
    });
    if let Some(node) = clicked {
        app.profiler.zoom = node;
        app.profiler.selected = Some(node);
    }
    open
}

/// Top-down rows: total, self, function, library. Returns a source to open.
fn call_tree(app: &mut CrabCraftApp, ui: &mut Ui, profile: &Profile) -> Option<(std::path::PathBuf, u32)> {
    let p = rc_kit::pal(ui);
    // The visible rows, in order.
    let mut rows = Vec::new();
    let mut stack = vec![0usize];
    while let Some(node) = stack.pop() {
        rows.push(node);
        if app.profiler.open.contains(&node) {
            let mut children = profile.sorted_children(node);
            children.reverse();
            stack.extend(children);
        }
    }
    let all = profile.total();
    let mut toggle = None;
    let mut open = None;
    let mut select = None;
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.add_sized(vec2(64.0, 18.0), egui::Label::new(RichText::new(tr("Total")).size(12.0).color(p.muted)));
        ui.add_sized(vec2(64.0, 18.0), egui::Label::new(RichText::new(tr("Self")).size(12.0).color(p.muted)));
        ui.label(RichText::new(tr("Function")).size(12.0).color(p.muted));
    });
    egui::ScrollArea::both().id_salt("profiler_tree").auto_shrink([false, false]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
        for &node in &rows[range] {
            let n = &profile.nodes[node];
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(600.0), ROW_H), Sense::click());
            if app.profiler.selected == Some(node) {
                ui.painter().rect_filled(rect, 0.0, p.selection);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 0.0, p.row_hover);
            }
            let y = rect.center().y;
            let font = FontId::proportional(12.5);
            ui.painter().text(pos2(rect.left() + 12.0, y), Align2::LEFT_CENTER, percent(n.total, all), font.clone(), p.text);
            ui.painter().text(pos2(rect.left() + 82.0, y), Align2::LEFT_CENTER, percent(n.self_, all), font.clone(), p.text_muted);
            let indent = rect.left() + 150.0 + n.depth as f32 * 14.0;
            if !n.children.is_empty() {
                let icon = if app.profiler.open.contains(&node) { ico::CHEVRON_DOWN } else { ico::CHEVRON_RIGHT };
                rc_kit::icons::paint(ui.painter(), Rect::from_center_size(pos2(indent + 7.0, y), vec2(14.0, 14.0)), icon, &p, true);
            }
            let own = n.frame.is_some_and(|f| profile.frames[f].own);
            let g = ui.painter().layout_no_wrap(profile.name(node).to_string(), font.clone(), if own { p.text } else { p.text_muted });
            let name_w = g.size().x;
            ui.painter().galley(pos2(indent + 18.0, y - g.size().y / 2.0), g, p.text);
            if let Some(f) = n.frame {
                let dso = std::path::Path::new(&profile.frames[f].dso).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                ui.painter().text(pos2(indent + 26.0 + name_w, y), Align2::LEFT_CENTER, dso, FontId::proportional(11.5), p.muted);
            }
            if resp.double_clicked() {
                open = profile.source(node).cloned();
            } else if resp.clicked() {
                select = Some(node);
                if !n.children.is_empty() {
                    toggle = Some(node);
                }
            }
            let resp = match profile.source(node) {
                Some((path, line)) => resp.on_hover_text(trf("{file}:{line} · double-click to open", &[("file", &path.display()), ("line", line)])),
                None => resp,
            };
            let _ = resp;
        }
    });
    if let Some(node) = toggle
        && !app.profiler.open.remove(&node)
    {
        app.profiler.open.insert(node);
    }
    if select.is_some() {
        app.profiler.selected = select;
    }
    open
}
