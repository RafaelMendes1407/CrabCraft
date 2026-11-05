//! AccessKit semantics for the painted editor. Text layout is bounded and cached.
use crate::{EditorConfig, EditorState};
use egui::{Id, Response, Ui, text::CCursor, text_selection::CCursorRange};
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

pub(crate) fn handle(ui: &Ui, response: &Response, state: &mut EditorState) {
    let events = ui.input(|input| input.events.clone());
    let cached = ui
        .ctx()
        .data(|data| data.get_temp::<Snapshot>(key(response.id)));
    for event in events {
        let egui::Event::AccessKitActionRequest(request) = &event else {
            continue;
        };
        if request.target_node != response.id.accesskit_id()
            || request.target_tree != egui::accesskit::TreeId::ROOT
        {
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
        let valid = |position: &egui::accesskit::TextPosition| {
            snapshot
                .galley
                .rows
                .iter()
                .enumerate()
                .any(|(row_index, row)| {
                    let count = row.glyphs.len() + usize::from(row.ends_with_newline);
                    (0..count.div_ceil(255).max(1)).any(|chunk| {
                        response.id.with(row_index).with(chunk).accesskit_id() == position.node
                            && position.character_index
                                <= count.saturating_sub(chunk * 255).min(255)
                    })
                })
        };
        if !valid(&selection.anchor) || !valid(&selection.focus) {
            continue;
        }
        let mut range = CCursorRange::one(CCursor::new(0));
        if range.on_event(ui.ctx().os(), &event, &snapshot.galley, response.id) {
            let anchor = snapshot.start.saturating_add(range.secondary.index.0);
            let head = snapshot.start.saturating_add(range.primary.index.0);
            if anchor <= snapshot.end && head <= snapshot.end {
                state.selection = rc_text::Selection::single(rc_text::Range::new(anchor, head));
                state.request_scroll_to_caret();
                response.request_focus();
            }
        }
    }
}

pub(crate) fn update(
    ui: &Ui,
    response: &Response,
    state: &EditorState,
    config: &EditorConfig,
    text_rect: egui::Rect,
    row_height: f32,
) {
    if ui
        .ctx()
        .accesskit_node_builder(response.id, |_| ())
        .is_none()
    {
        ui.ctx()
            .data_mut(|data| data.remove::<Snapshot>(key(response.id)));
        return;
    }
    let rope = state.buffer.rope();
    let len = rope.len_chars();
    let line = rope.char_to_line(state.caret().min(len));
    let first_line = line.saturating_sub(128);
    let start = if len <= MAX_CHARS && rope.len_lines() <= 256 {
        0
    } else {
        (state.caret().saturating_sub(MAX_CHARS / 2) / 4096 * 4096)
            .min(len)
            .max(rope.line_to_char(first_line))
    };
    let after_line = (first_line + 256).min(rope.len_lines());
    let line_end = if after_line < rope.len_lines() {
        rope.line_to_char(after_line)
    } else {
        len
    };
    let end = start.saturating_add(MAX_CHARS).min(len).min(line_end);
    let previous = ui
        .ctx()
        .data(|data| data.get_temp::<Snapshot>(key(response.id)));
    let cached = previous.as_ref().filter(|s| {
        s.version == state.buffer.version()
            && s.start == start
            && s.end == end
            && s.size == config.font.size.to_bits()
            && s.row_height == row_height.to_bits()
    });
    let snapshot = if let Some(cached) = cached {
        cached.clone()
    } else {
        let text = rope.slice(start..end).to_string();
        let mut job = egui::text::LayoutJob::simple(
            text,
            egui::FontId::monospace(config.font.size),
            ui.visuals().text_color(),
            f32::INFINITY,
        );
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
    response.widget_info(|| {
        egui::WidgetInfo::text_edit(
            true,
            previous.as_ref().map(|s| s.galley.text()).unwrap_or(text),
            text,
            &label,
        )
    });
    let selected = state.selection.primary();
    let cursor = (selected.anchor >= start
        && selected.anchor <= end
        && selected.head >= start
        && selected.head <= end)
        .then(|| {
            CCursorRange::two(
                CCursor::new(selected.anchor - start),
                CCursor::new(selected.head - start),
            )
        });
    let origin = text_rect.min - egui::vec2(state.scroll.x, state.scroll.y)
        + egui::vec2(4.0, rope.char_to_line(start) as f32 * row_height);
    egui::text_selection::accesskit_text::update_accesskit_for_text_widget(
        ui.ctx(),
        response.id,
        cursor,
        egui::accesskit::Role::MultilineTextInput,
        egui::emath::TSTransform::from_translation(origin.to_vec2()),
        &snapshot.galley,
    );
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label(label);
        node.add_action(egui::accesskit::Action::SetTextSelection);
        if state.read_only {
            node.set_read_only();
        }
        if start != 0 || end != len {
            node.set_description(tr(
                "Accessible text is limited to the context around the caret",
            ));
        }
    });
    ui.ctx()
        .data_mut(|data| data.insert_temp(key(response.id), snapshot));
}
