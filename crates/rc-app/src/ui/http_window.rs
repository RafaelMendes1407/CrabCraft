//! The HTTP Client window (F-03): the requests sent from `.http` files, newest first, and the
//! selected one's response: status, timing, size, headers and body (JSON re-indented).

use crate::app::CrabCraftApp;
use crate::http_client::{BodyView, Exchange, Outcome, size_text};
use crate::toolwindow::ToolWindow;
use crate::ui::run_windows::header;
use crate::ui::toolwindows::empty_state;
use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Ui, pos2, vec2};
use rc_kit::i18n::{tr, trf};
use rc_kit::{ico, theme};

const LIST_W: f32 = 280.0;
const ROW_H: f32 = 24.0;

pub fn show(app: &mut CrabCraftApp, ui: &mut Ui) {
    let selected_running = app.http.history.get(app.http.selected).is_some_and(|e| matches!(e.outcome, Outcome::Running(_)));
    let actions = [(ico::STOP, tr("Cancel Request"), selected_running), (ico::DELETE, tr("Clear Finished Requests"), app.http.history.iter().any(|e| !matches!(e.outcome, Outcome::Running(_))))];
    match header(ui, ToolWindow::Http.title(), &actions) {
        Some(0) => app.http.cancel(app.http.selected),
        Some(1) => {
            app.http.history.retain(|e| matches!(e.outcome, Outcome::Running(_)));
            app.http.selected = 0;
        }
        Some(usize::MAX) => app.toggle_tool(ToolWindow::Http),
        _ => {}
    }
    egui::ScrollArea::vertical().id_salt("http_connection_settings").max_height((ui.available_height() - 80.0).clamp(60.0, 320.0)).show(ui, |ui| {
        app.http.environment.show(ui);
        app.http.oauth.show(ui);
    });
    if app.http.history.is_empty() {
        empty_state(ui, ToolWindow::Http.icon(), tr("No requests sent yet"), tr("Open an .http file and click the run icon next to a request."));
        return;
    }
    if app.http.running() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
    let p = rc_kit::pal(ui);
    let avail = ui.available_rect_before_wrap();
    let left = Rect::from_min_size(avail.min, vec2(LIST_W.min(avail.width() * 0.4), avail.height()));
    let right = Rect::from_min_max(pos2(left.right() + 1.0, avail.top()), avail.max);
    ui.painter().vline(left.right() + 0.5, avail.y_range(), egui::Stroke::new(1.0, p.island_border));
    let mut select = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(left));
    egui::ScrollArea::vertical().id_salt("http_history").auto_shrink([false, false]).show(&mut child, |ui| {
        for (i, e) in app.http.history.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            if i == app.http.selected {
                ui.painter().rect_filled(rect, 0.0, p.selection);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 0.0, p.row_hover);
            }
            let (badge, color) = badge(e, &p);
            ui.painter().text(pos2(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, &badge, theme::mono_bold(11.5), color);
            let title = clip(&e.title, 48);
            ui.painter().text(pos2(rect.left() + 52.0, rect.center().y), Align2::LEFT_CENTER, title, FontId::proportional(13.0), p.text);
            if resp.clicked() {
                select = Some(i);
            }
        }
    });
    if let Some(i) = select {
        app.http.selected = i;
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(right.shrink2(vec2(10.0, 4.0))));
    let mut open = None;
    if let Some(e) = app.http.history.get(app.http.selected) {
        open = details(&mut child, e, &mut app.http.show_headers);
    }
    ui.allocate_rect(avail, Sense::hover());
    if let Some((path, line)) = open {
        app.open_at(&path, line, 0);
    }
}

/// The status column of a history row.
fn badge(e: &Exchange, p: &rc_kit::theme::Palette) -> (String, Color32) {
    match &e.outcome {
        Outcome::Running(_) => ("…".to_string(), p.muted),
        Outcome::Done { response, .. } => (response.status.to_string(), status_color(response.status, p)),
        Outcome::Failed(_) => ("ERR".to_string(), p.error),
    }
}

fn status_color(status: u16, p: &rc_kit::theme::Palette) -> Color32 {
    match status {
        200..=299 => p.ok,
        300..=399 => p.muted,
        400..=499 => p.warn,
        _ => p.error,
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() }
}

/// The selected request and its response; the request's place when its title is clicked.
fn details(ui: &mut Ui, e: &Exchange, show_headers: &mut bool) -> Option<(std::path::PathBuf, usize)> {
    let p = rc_kit::pal(ui);
    let mut open = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(&e.method).font(theme::mono_bold(12.5)).color(p.text));
        ui.label(RichText::new(clip(&e.url, 160)).monospace().color(p.text_muted));
        if !e.file.as_os_str().is_empty() {
            let name = e.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if ui.link(format!("{name}:{}", e.line + 1)).on_hover_text(tr("Go to the request")).clicked() {
                open = Some((e.file.clone(), e.line));
            }
        }
    });
    for note in &e.notes {
        ui.label(RichText::new(format!("⚠ {note}")).size(12.5).color(p.warn));
    }
    match &e.outcome {
        Outcome::Running(_) => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                let secs = e.started.elapsed().as_secs_f32();
                ui.label(RichText::new(trf("Waiting for the response… {seconds} s", &[("seconds", &format!("{secs:.1}"))])).color(p.muted));
            });
        }
        Outcome::Failed(err) => {
            ui.add_space(4.0);
            ui.label(RichText::new(err.text()).color(p.error));
        }
        Outcome::Done { response, body } => {
            ui.horizontal(|ui| {
                let status = format!("{} {}", response.status, response.reason);
                ui.label(RichText::new(status).font(theme::bold(13.0)).color(status_color(response.status, &p)));
                let ms = |d: std::time::Duration| if d.as_millis() < 10 { format!("{:.1} ms", d.as_secs_f64() * 1000.0) } else { format!("{} ms", d.as_millis()) };
                let info = trf("{time} (headers after {headers}) · {size} · {version}", &[("time", &ms(response.elapsed)), ("headers", &ms(response.to_headers)), ("size", &size_text(response.body.len())), ("version", &response.version)]);
                ui.label(RichText::new(info).size(12.5).color(p.muted));
            });
            if response.truncated {
                ui.label(RichText::new(trf("The body is larger than {size}; only the start was read.", &[("size", &size_text(response.body.len()))])).size(12.5).color(p.warn));
            }
            ui.horizontal(|ui| {
                if theme::tab(ui, !*show_headers, None, tr("Body"), false).0.clicked() {
                    *show_headers = false;
                }
                let headers = trf("Headers ({n})", &[("n", &response.headers.len())]);
                if theme::tab(ui, *show_headers, None, &headers, false).0.clicked() {
                    *show_headers = true;
                }
            });
            egui::ScrollArea::both().id_salt("http_body").auto_shrink([false, false]).show(ui, |ui| {
                if *show_headers {
                    egui::Grid::new("http_headers").num_columns(2).spacing(vec2(16.0, 4.0)).show(ui, |ui| {
                        for (k, v) in &response.headers {
                            ui.label(RichText::new(k).monospace().color(p.text_muted));
                            ui.add(egui::Label::new(RichText::new(v).monospace().color(p.text)).selectable(true));
                            ui.end_row();
                        }
                    });
                    return;
                }
                match body {
                    BodyView::Empty => {
                        ui.label(RichText::new(tr("The response has no body.")).color(p.muted));
                    }
                    BodyView::Binary(n) => {
                        ui.label(RichText::new(trf("Binary body, {size}", &[("size", &size_text(*n))])).color(p.muted));
                    }
                    BodyView::Text { text, cut, .. } => {
                        ui.add(egui::Label::new(RichText::new(text.as_str()).monospace().size(12.5).color(p.text)).selectable(true).extend());
                        if *cut {
                            ui.label(RichText::new(tr("Only the start of the body is shown.")).color(p.muted));
                        }
                    }
                }
            });
        }
    }
    open
}
