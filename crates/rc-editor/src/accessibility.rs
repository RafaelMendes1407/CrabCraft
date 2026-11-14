//! AccessKit semantics for the painted editor. Text layout is bounded and cached.
use crate::{EditorConfig, EditorState};
use egui::{Id, Response, Ui};
use rc_kit::i18n::{tr, trf};
use std::sync::Arc;
const MAX_CHARS: usize = 16_384;
#[derive(Clone)]
struct Snapshot {
    version: u64,
    start: usize,
    end: usize,
    size: u32,
    row_height: u32,
    galley: Arc<egui::Galley>,
}
fn key(id: Id) -> Id {
    id.with("source_accessibility")
}

fn run_id(parent: Id, snapshot: &Snapshot, row: usize, chunk: usize) -> Id {
    parent.with(("source_text", snapshot.version, snapshot.start, snapshot.end, row, chunk))
}
fn offset(parent: Id, snapshot: &Snapshot, position: &egui::accesskit::TextPosition) -> Option<usize> {
    let mut at = snapshot.start;
    for (row_index, row) in snapshot.galley.rows.iter().enumerate() {
        let count = row.glyphs.len() + usize::from(row.ends_with_newline);
        for chunk in 0..count.div_ceil(255).max(1) {
            if run_id(parent, snapshot, row_index, chunk).accesskit_id() == position.node
                && position.character_index <= count.saturating_sub(chunk * 255).min(255)
            {
                return Some(at + chunk * 255 + position.character_index);
            }
        }
        at = at.saturating_add(count);
    }
    None
}
fn text_position(parent: Id, snapshot: &Snapshot, source: usize) -> Option<egui::accesskit::TextPosition> {
    let mut at = snapshot.start;
    for (row_index, row) in snapshot.galley.rows.iter().enumerate() {
        let count = row.glyphs.len() + usize::from(row.ends_with_newline);
        if source >= at && source <= at + count {
            let column = source - at;
            let chunk = column.saturating_sub(1) / 255;
            return Some(egui::accesskit::TextPosition {
                node: run_id(parent, snapshot, row_index, chunk).accesskit_id(),
                character_index: column - chunk * 255,
            });
        }
        at = at.saturating_add(count);
    }
    None
}

pub(crate) fn handle(ui: &Ui, response: &Response, state: &mut EditorState) {
    let events = ui.input(|input| input.events.clone());
    let cached = ui.ctx().data(|data| data.get_temp::<Snapshot>(key(response.id)));
    for event in events {
        let egui::Event::AccessKitActionRequest(request) = &event else {
            continue;
        };
        if request.target_node != response.id.accesskit_id() || request.target_tree != egui::accesskit::TreeId::ROOT {
            continue;
        }
        if request.action == egui::accesskit::Action::Focus {
            response.request_focus();
        }
        if request.action != egui::accesskit::Action::SetTextSelection {
            continue;
        }
        let Some(snapshot) = &cached else { continue };
        if snapshot.version != state.buffer.version() {
            continue;
        }
        let Some(egui::accesskit::ActionData::SetTextSelection(selection)) = &request.data else {
            continue;
        };
        let Some(anchor) = offset(response.id, snapshot, &selection.anchor) else {
            continue;
        };
        let Some(head) = offset(response.id, snapshot, &selection.focus) else {
            continue;
        };
        if anchor <= snapshot.end && head <= snapshot.end {
            state.selection = rc_text::Selection::single(rc_text::Range::new(anchor, head));
            let anchor_line = state.buffer.rope().char_to_line(anchor);
            let head_line = state.buffer.rope().char_to_line(head);
            state.unfold_line(anchor_line);
            state.unfold_line(head_line);
            state.request_scroll_to_caret();
            response.request_focus();
        }
    }
}

pub(crate) fn update(ui: &mut Ui, response: &Response, state: &mut EditorState, config: &EditorConfig, text_rect: egui::Rect, row_height: f32) {
    if ui.ctx().accesskit_node_builder(response.id, |_| ()).is_none() {
        ui.ctx().data_mut(|data| data.remove::<Snapshot>(key(response.id)));
        return;
    }
    let rope = state.buffer.snapshot();
    let len = rope.len_chars();
    let line = rope.char_to_line(state.caret().min(len));
    let first_line = line.saturating_sub(128);
    let start = if len <= MAX_CHARS && rope.len_lines() <= 256 {
        0
    } else {
        (state.caret().saturating_sub(MAX_CHARS / 2) / 4096 * 4096).min(len).max(rope.line_to_char(first_line))
    };
    let after_line = (first_line + 256).min(rope.len_lines());
    let line_end = if after_line < rope.len_lines() { rope.line_to_char(after_line) } else { len };
    let end = start.saturating_add(MAX_CHARS).min(len).min(line_end);
    let previous = ui.ctx().data(|data| data.get_temp::<Snapshot>(key(response.id)));
    let cached = previous.as_ref().filter(|s| {
        s.version == state.buffer.version() && s.start == start && s.end == end && s.size == config.font.size.to_bits() && s.row_height == row_height.to_bits()
    });
    let snapshot = if let Some(cached) = cached {
        cached.clone()
    } else {
        let text = rope.slice(start..end).to_string();
        let mut job = egui::text::LayoutJob::simple(text, egui::FontId::monospace(config.font.size), ui.visuals().text_color(), f32::INFINITY);
        for section in &mut job.sections {
            section.format.line_height = Some(row_height);
        }
        Snapshot {
            version: state.buffer.version(),
            start,
            end,
            size: config.font.size.to_bits(),
            row_height: row_height.to_bits(),
            galley: ui.painter().layout_job(job),
        }
    };
    let label = state
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|p| trf("Source code: {file}", &[("file", &p.to_string_lossy())]))
        .unwrap_or_else(|| tr("Source code editor").into());
    let text = snapshot.galley.text();
    response.widget_info(|| egui::WidgetInfo::text_edit(true, previous.as_ref().map(|s| s.galley.text()).unwrap_or(text), text, &label));
    let selected = state.selection.primary();
    let selection = text_position(response.id, &snapshot, selected.anchor)
        .zip(text_position(response.id, &snapshot, selected.head))
        .map(|(anchor, focus)| egui::accesskit::TextSelection { anchor, focus });
    let text_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("source_accessible_context", response.id, start, end, snapshot.version))
            .accessibility_parent(response.id)
            .max_rect(text_rect),
    );
    // Logical text remains complete across folds. Only visible source has painted bounds;
    // selecting a hidden range above unfolds it before scrolling to the caret.
    let scheme = rc_kit::theme::scheme(ui.visuals().dark_mode);
    let mut position = start;
    for (row_index, row) in snapshot.galley.rows.iter().enumerate() {
        let source_line = rope.char_to_line(position.min(len));
        let line_start = rope.line_to_char(source_line);
        let column = position.saturating_sub(line_start);
        let count = row.glyphs.len() + usize::from(row.ends_with_newline);
        let display_row = crate::view::row_of_line(state, source_line);
        let visible = crate::view::line_of_row(state, display_row) == source_line;
        let full_line = column == 0 && (row.ends_with_newline || position + count == len);
        let layout = if visible && full_line { Some(crate::view::layout_buffer_line(ui, state, config, &scheme, source_line, &[])) } else { None };
        for chunk in 0..count.div_ceil(255).max(1) {
            let from = (chunk * 255).min(count);
            let to = ((chunk + 1) * 255).min(count);
            let glyph_end = row.glyphs.len();
            let x_of = |index: usize| layout.as_ref().map_or(0.0, |layout| layout.x_of_col(index.min(glyph_end)));
            let x = text_rect.left() + 4.0 - state.scroll.x + x_of(from);
            let y = text_rect.top() + display_row as f32 * row_height - state.scroll.y;
            let width = if layout.is_some() { (x_of(to) - x_of(from)).max(0.0) } else { 0.0 };
            let positions = (from..to).map(|index| x_of(index) - x_of(from)).collect::<Vec<_>>();
            let widths = (from..to).map(|index| if index < glyph_end { (x_of(index + 1) - x_of(index)).max(0.0) } else { 0.0 }).collect::<Vec<_>>();
            let id = run_id(response.id, &snapshot, row_index, chunk);
            let bounds = egui::Rect::from_min_max(egui::pos2(x, y), egui::pos2(x + width, y + if layout.is_some() { row_height } else { 0.0 }));
            text_ui.interact(bounds, id, egui::Sense::hover());
            let characters = (from..to)
                .filter_map(|index| {
                    row.glyphs.get(index).map(|glyph| glyph.chr).or_else(|| (index == row.glyphs.len() && row.ends_with_newline).then_some('\n'))
                })
                .collect::<Vec<_>>();
            let value = characters.iter().collect::<String>();
            let lengths = characters.iter().map(|ch| ch.len_utf8() as u8).collect::<Vec<_>>();
            let mut word_starts = Vec::new();
            let mut previous_word = false;
            for (index, ch) in characters.iter().enumerate() {
                let word = ch.is_alphanumeric() || *ch == '_';
                if word && !previous_word {
                    word_starts.push(index as u8);
                }
                previous_word = word;
            }
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(egui::accesskit::Role::TextRun);
                node.set_text_direction(egui::accesskit::TextDirection::LeftToRight);
                node.set_value(value);
                node.set_character_lengths(lengths);
                node.set_word_starts(word_starts);
                node.set_bounds(egui::accesskit::Rect {
                    x0: x.into(),
                    y0: y.into(),
                    x1: (x + width).into(),
                    y1: (y + if layout.is_some() { row_height } else { 0.0 }).into(),
                });
                node.set_character_positions(positions);
                node.set_character_widths(widths);
            });
        }
        position = position.saturating_add(count);
    }
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(egui::accesskit::Role::MultilineTextInput);
        node.set_label(label);
        node.add_action(egui::accesskit::Action::Focus);
        if let Some(selection) = selection {
            node.set_text_selection(selection);
        }
        node.add_action(egui::accesskit::Action::SetTextSelection);
        if state.read_only {
            node.set_read_only();
        }
        if start != 0 || end != len {
            node.set_description(tr("Accessible text is limited to the context around the caret"));
        }
    });
    ui.ctx().data_mut(|data| data.insert_temp(key(response.id), snapshot));
}
