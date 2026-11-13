//! Shared application header, with custom window controls on Windows and Linux and native
//! macOS chrome.
use crate::i18n::tr;
use crate::theme::pal;
use egui::viewport::ResizeDirection;
use egui::{Color32, CursorIcon, Id, Rect, Sense, Stroke, Ui, Vec2, ViewportCommand, pos2, vec2};

/// Windows and Linux draw their own caption buttons in the header instead of a system title
/// bar; macOS keeps its traffic lights over the header.
pub const CUSTOM_CAPTION: bool = !cfg!(target_os = "macos");

pub fn viewport() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title("CrabCraft")
        .with_inner_size([1400.0, 900.0])
        .with_min_inner_size([900.0, 560.0])
        .with_resizable(true)
        .with_decorations(!CUSTOM_CAPTION)
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        .with_icon(crate::branding::icon_data())
}

pub fn leading_space(macos: bool, fullscreen: bool, zoom: f32) -> f32 {
    if macos && !fullscreen {
        80.0 / zoom.max(0.1)
    } else {
        10.0
    }
}

pub fn controls(ui: &mut Ui) -> Option<ViewportCommand> {
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    let p = pal(ui);
    let mut command = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // The header's right group is laid out right to left.
        for (index, label) in [
            (0, tr("Close window")),
            (
                1,
                if maximized {
                    tr("Restore window")
                } else {
                    tr("Maximize")
                },
            ),
            (2, tr("Minimize")),
        ] {
            let (rect, response) = ui.allocate_exact_size(
                vec2(44.0, ui.available_height().clamp(28.0, 40.0)),
                Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
            });
            let fill = if index == 0 && response.hovered() {
                Color32::from_rgb(196, 43, 28)
            } else if response.hovered() {
                p.hover
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 0.0, fill);
            let color = if index == 0 && response.hovered() {
                Color32::WHITE
            } else {
                p.text
            };
            let stroke = Stroke::new(1.0, color);
            let center = rect.center();
            match index {
                0 => {
                    ui.painter()
                        .line_segment([center + vec2(-5.0, -5.0), center + vec2(5.0, 5.0)], stroke);
                    ui.painter()
                        .line_segment([center + vec2(-5.0, 5.0), center + vec2(5.0, -5.0)], stroke);
                }
                1 => {
                    let square = Rect::from_center_size(center, Vec2::splat(10.0));
                    if maximized {
                        ui.painter().rect_stroke(
                            square.translate(vec2(2.0, -2.0)),
                            0.0,
                            stroke,
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().rect_filled(
                            square,
                            0.0,
                            if response.hovered() {
                                p.hover
                            } else {
                                p.main_bg
                            },
                        );
                    }
                    ui.painter()
                        .rect_stroke(square, 0.0, stroke, egui::StrokeKind::Inside);
                }
                _ => {
                    ui.painter()
                        .line_segment([center + vec2(-5.0, 1.0), center + vec2(5.0, 1.0)], stroke);
                }
            }
            if response.on_hover_text(label).clicked() {
                command = Some(match index {
                    0 => ViewportCommand::Close,
                    1 => ViewportCommand::Maximized(!maximized),
                    _ => ViewportCommand::Minimized(true),
                });
            }
        }
    });
    command
}

fn resize_regions(r: Rect) -> [(ResizeDirection, Rect, CursorIcon); 8] {
    use CursorIcon::{
        ResizeHorizontal as H, ResizeNeSw as NE, ResizeNwSe as NW, ResizeVertical as V,
    };
    use ResizeDirection::*;
    let corner = 16.0;
    let edge = 8.0;
    [
        (
            NorthWest,
            Rect::from_min_size(r.left_top(), Vec2::splat(corner)),
            NW,
        ),
        (
            NorthEast,
            Rect::from_min_size(r.right_top() - vec2(corner, 0.0), Vec2::splat(corner)),
            NE,
        ),
        (
            SouthWest,
            Rect::from_min_size(r.left_bottom() - vec2(0.0, corner), Vec2::splat(corner)),
            NE,
        ),
        (
            SouthEast,
            Rect::from_min_size(r.right_bottom() - Vec2::splat(corner), Vec2::splat(corner)),
            NW,
        ),
        (
            North,
            Rect::from_min_size(
                r.left_top() + vec2(corner, 0.0),
                vec2(r.width() - 2.0 * corner, edge),
            ),
            V,
        ),
        (
            South,
            Rect::from_min_size(
                pos2(r.left() + corner, r.bottom() - edge),
                vec2(r.width() - 2.0 * corner, edge),
            ),
            V,
        ),
        (
            West,
            Rect::from_min_size(
                r.left_top() + vec2(0.0, corner),
                vec2(edge, r.height() - 2.0 * corner),
            ),
            H,
        ),
        (
            East,
            Rect::from_min_size(
                pos2(r.right() - edge, r.top() + corner),
                vec2(edge, r.height() - 2.0 * corner),
            ),
            H,
        ),
    ]
}

pub fn resize_edges(ctx: &egui::Context) {
    if ctx.input(|i| {
        i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)
    }) {
        return;
    }
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    let pressed = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
    let mut resize = None;
    for (index, (direction, rect, cursor)) in
        resize_regions(ctx.content_rect()).into_iter().enumerate()
    {
        egui::Area::new(Id::new(("window_resize", index)))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .movable(false)
            .constrain(false)
            .show(ctx, |ui| {
                ui.allocate_exact_size(rect.size(), Sense::hover());
            });
        if pointer.is_some_and(|pos| rect.contains(pos)) {
            ctx.set_cursor_icon(cursor);
            if pressed {
                resize = Some(direction);
            }
        }
    }
    if let Some(direction) = resize {
        // The compositor owns the drag from this press onward. Wayland may consume
        // its release, so egui must not keep a pressed button or captured widget.
        // Begin on press while the backend still has the matching input serial.
        ctx.stop_dragging();
        ctx.input_mut(|input| input.pointer = Default::default());
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn platform_chrome_keeps_mac_controls_and_removes_the_system_title_bar() {
        let viewport = viewport();
        assert_eq!(CUSTOM_CAPTION, !cfg!(target_os = "macos"));
        assert_eq!(viewport.decorations, Some(cfg!(target_os = "macos")));
        assert_eq!(viewport.resizable, Some(true));
        assert_eq!(viewport.fullsize_content_view, Some(true));
        assert_eq!(leading_space(false, false, 1.0), 10.0);
        assert_eq!(leading_space(true, false, 2.0), 40.0);
        assert_eq!(leading_space(true, true, 1.0), 10.0);
    }

    #[test]
    fn caption_buttons_dispatch_native_window_commands() {
        for (label, maximized, expected) in [
            ("Close window", false, ViewportCommand::Close),
            ("Minimize", false, ViewportCommand::Minimized(true)),
            ("Maximize", false, ViewportCommand::Maximized(true)),
            ("Restore window", true, ViewportCommand::Maximized(false)),
        ] {
            let mut h = egui_kittest::Harness::builder()
                .with_size(vec2(300.0, 44.0))
                .build_ui_state(
                    |ui, commands: &mut Vec<ViewportCommand>| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(command) = controls(ui) {
                                commands.push(command);
                            }
                        });
                    },
                    Vec::new(),
                );
            h.input_mut()
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .maximized = Some(maximized);
            h.run_steps(2);
            h.get_by_label(label).click();
            h.run_steps(2);
            assert_eq!(h.state(), &vec![expected]);
        }
    }

    #[test]
    fn native_resize_releases_capture_and_accepts_another_press_without_release() {
        let ctx = egui::Context::default();
        let bounds = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 560.0));
        for (point, expected, cursor) in [
            (pos2(4.0, 200.0), ResizeDirection::West, CursorIcon::ResizeHorizontal),
            (pos2(450.0, 4.0), ResizeDirection::North, CursorIcon::ResizeVertical),
            (pos2(896.0, 556.0), ResizeDirection::SouthEast, CursorIcon::ResizeNwSe),
        ] {
            let mut input = egui::RawInput { screen_rect: Some(bounds), ..Default::default() };
            input.events.push(egui::Event::PointerMoved(point));
            ctx.begin_pass(input);
            resize_edges(&ctx);
            let output = ctx.end_pass();
            assert_eq!(output.platform_output.cursor_icon, cursor);
            output.drop_without_applying_deltas();

            let mut input = egui::RawInput { screen_rect: Some(bounds), ..Default::default() };
            input.events.push(egui::Event::PointerButton {
                pos: point, button: egui::PointerButton::Primary, pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            ctx.begin_pass(input);
            resize_edges(&ctx);
            assert!(!ctx.input(|i| i.pointer.any_down()));
            assert_eq!(ctx.dragged_id(), None);
            let output = ctx.end_pass();
            assert!(output.viewport_output[&egui::ViewportId::ROOT].commands
                .contains(&ViewportCommand::BeginResize(expected)));
            output.drop_without_applying_deltas();
            // Deliberately omit PointerButton { pressed: false }: KWin owns it.
        }
    }

    #[test]
    fn native_resize_ignores_content_presses_and_maximized_or_fullscreen_windows() {
        let ctx = egui::Context::default();
        for (point, maximized, fullscreen) in [
            (pos2(450.0, 200.0), false, false),
            (pos2(4.0, 200.0), true, false),
            (pos2(4.0, 200.0), false, true),
        ] {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 560.0))),
                ..Default::default()
            };
            let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
            viewport.maximized = Some(maximized);
            viewport.fullscreen = Some(fullscreen);
            input.events.push(egui::Event::PointerButton {
                pos: point, button: egui::PointerButton::Primary, pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            ctx.begin_pass(input);
            resize_edges(&ctx);
            let output = ctx.end_pass();
            assert!(!output.viewport_output[&egui::ViewportId::ROOT].commands.iter()
                .any(|command| matches!(command, ViewportCommand::BeginResize(_))));
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn dragging_from_editor_across_edge_does_not_start_native_resize() {
        let ctx = egui::Context::default();
        let bounds = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 560.0));
        let mut input = egui::RawInput { screen_rect: Some(bounds), ..Default::default() };
        input.events.push(egui::Event::PointerButton {
            pos: pos2(450.0, 200.0), button: egui::PointerButton::Primary,
            pressed: true, modifiers: egui::Modifiers::NONE,
        });
        ctx.begin_pass(input);
        resize_edges(&ctx);
        ctx.end_pass().drop_without_applying_deltas();
        let mut input = egui::RawInput { screen_rect: Some(bounds), ..Default::default() };
        input.events.push(egui::Event::PointerMoved(pos2(4.0, 200.0)));
        ctx.begin_pass(input);
        resize_edges(&ctx);
        let output = ctx.end_pass();
        assert!(ctx.input(|i| i.pointer.primary_down()));
        assert!(!output.viewport_output[&egui::ViewportId::ROOT].commands.iter()
            .any(|command| matches!(command, ViewportCommand::BeginResize(_))));
        output.drop_without_applying_deltas();
    }

    #[test]
    fn resize_regions_cover_edges_without_covering_header_buttons() {
        let bounds = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 560.0));
        let regions = resize_regions(bounds);
        for (point, expected) in [
            (pos2(1.0, 1.0), ResizeDirection::NorthWest),
            (pos2(899.0, 559.0), ResizeDirection::SouthEast),
            (pos2(450.0, 1.0), ResizeDirection::North),
            (pos2(1.0, 200.0), ResizeDirection::West),
        ] {
            assert_eq!(
                regions
                    .iter()
                    .filter(|(_, rect, _)| rect.contains(point))
                    .map(|(direction, _, _)| *direction)
                    .collect::<Vec<_>>(),
                vec![expected]
            );
        }
        assert!(
            regions
                .iter()
                .all(|(_, rect, _)| !rect.contains(pos2(878.0, 20.0)))
        );
    }
}
