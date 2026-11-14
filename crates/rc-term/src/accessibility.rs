//! Visible terminal text and cursor. Read-only semantics never send bytes to the PTY.
use super::{Metrics, TermSession, TermState};
use alacritty_terminal::{
    index::{Column, Line},
    term::cell::Flags,
};
use egui::{
    Response, Ui,
    accesskit::{self, Action, Role, TextPosition, TextSelection},
};
use rc_kit::i18n::{tr, trf};
const MAX_CHARS: usize = 16_384;
pub fn focus(ui: &Ui, response: &Response, _input_enabled: bool) {
    if ui.input(|input|input.events.iter().any(|event|matches!(event,egui::Event::AccessKitActionRequest(request) if request.target_tree==accesskit::TreeId::ROOT&&request.target_node==response.id.accesskit_id()&&request.action==Action::Focus))){response.request_focus();}
}
pub fn update(ui: &mut Ui, response: &Response, session: &TermSession, state: &TermState, m: &Metrics, _input_enabled: bool) {
    if ui.ctx().accesskit_node_builder(response.id, |_| ()).is_none() {
        return;
    }
    let text_ui =
        ui.new_child(egui::UiBuilder::new().id_salt(("terminal_accessibility", response.id)).accessibility_parent(response.id).max_rect(response.rect));
    let grid = state.term.grid();
    let offset = grid.display_offset() as i32;
    let cursor = grid.cursor.point;
    let cursor_row = cursor.line.0 + offset;
    let first = if (0..m.rows as i32).contains(&cursor_row) { (cursor_row as usize).saturating_sub(128) } else { 0 };
    let end = (first + 256).min(m.rows);
    let mut remaining = MAX_CHARS;
    let mut value = String::new();
    let mut caret = None;
    for row in first..end {
        if remaining == 0 {
            break;
        }
        let mut characters = Vec::new();
        for col in 0..m.cols.min(4096) {
            let cell = &grid[Line(row as i32 - offset)][Column(col)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            characters.push((cell.c, col, if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 }));
            if let Some(zero) = cell.zerowidth() {
                for ch in zero {
                    characters.push((*ch, col, 0));
                }
            }
        }
        let keep = characters.iter().rposition(|(ch, col, _)| *ch != ' ' || (row as i32 == cursor_row && *col <= cursor.column.0)).map_or(0, |index| index + 1);
        characters.truncate(keep.min(remaining));
        remaining = remaining.saturating_sub(characters.len());
        if row + 1 < end && remaining > 0 {
            characters.push(('\n', m.cols, 0));
            remaining -= 1;
        }
        let count = characters.len();
        let chunks = count.div_ceil(255).max(1);
        for chunk in 0..chunks {
            let chars = &characters[(chunk * 255).min(count)..((chunk + 1) * 255).min(count)];
            let id = response.id.with("terminal_text").with(row).with(chunk);
            let run_rect =
                egui::Rect::from_min_max(egui::pos2(m.x(chars.first().map_or(0, |(_, col, _)| *col)), m.y(row)), egui::pos2(m.x(m.cols), m.y(row) + m.row_h));
            text_ui.interact(run_rect, id, egui::Sense::hover());
            let text: String = chars.iter().map(|(ch, _, _)| ch).collect();
            value.push_str(&text);
            let start = chars.first().map_or(0, |(_, col, _)| *col);
            let last = chars.last().map_or(start, |(_, col, width)| col + width);
            let x = m.x(start);
            let y = m.y(row);
            if row as i32 == cursor_row && caret.is_none() {
                if let Some(index) = chars.iter().position(|(_, col, _)| *col >= cursor.column.0) {
                    caret = Some(TextPosition { node: id.accesskit_id(), character_index: index });
                } else if chunk + 1 == chunks {
                    caret = Some(TextPosition { node: id.accesskit_id(), character_index: chars.len() });
                }
            }
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::TextRun);
                node.set_text_direction(accesskit::TextDirection::LeftToRight);
                node.set_value(text);
                node.set_bounds(accesskit::Rect { x0: x.into(), y0: y.into(), x1: m.x(last).into(), y1: (y + m.row_h).into() });
                node.set_character_lengths(chars.iter().map(|(ch, _, _)| ch.len_utf8() as u8).collect::<Vec<_>>());
                node.set_character_positions(chars.iter().map(|(_, col, _)| m.x(*col) - x).collect::<Vec<_>>());
                node.set_character_widths(chars.iter().map(|(_, _, width)| *width as f32 * m.cell_w).collect::<Vec<_>>());
            });
        }
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::GenericContainer, true, tr("Terminal")));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Terminal);
        let label = if session.name().is_empty() { tr("Terminal").to_owned() } else { trf("Terminal: {name}", &[("name", &session.name())]) };
        node.set_label(label);
        node.set_value(value);
        node.set_read_only();
        node.add_action(Action::Focus);
        node.set_live(accesskit::Live::Polite);
        node.set_description(tr("Accessible terminal text is limited to the visible context; text actions never send commands"));
        if let Some(caret) = caret {
            node.set_text_selection(TextSelection { anchor: caret.clone(), focus: caret });
        }
    });
}
