//! Scrollbar lens: while the pointer is over the vertical scrollbar, a floating preview shows
//! the code at that height of the document, with the diagnostic of its error stripe mark.
//! Only the few lines shown are laid out, and only while hovering.

use crate::layout::{mono, severity_color};
use crate::state::{EditorState, Severity};
use crate::view::{EditorConfig, Metrics, TEXT_PAD, layout_buffer_line, wavy};
use egui::{Id, LayerId, Order, Pos2, Rect, Stroke, Ui, pos2, vec2};
use rc_kit::Scheme;

/// Rows of code the lens shows around the hovered line.
pub const LENS_ROWS: usize = 9;
/// How close (in points) the pointer must be to a stripe mark to pick it.
const MARK_SLOP: f32 = 3.0;

/// What the lens shows this frame (read by tests and the host).
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollLens {
    /// Buffer line under the pointer, centred in the lens.
    pub line: usize,
    /// Buffer lines shown, top to bottom (one per display row: folds stay folded).
    pub lines: Vec<usize>,
    /// The error or warning of the centred line.
    pub message: Option<(Severity, String)>,
    /// Screen rect of the lens.
    pub rect: Rect,
}

/// Display row whose stripe mark is drawn at height `y` of a scrollbar track: the inverse of
/// the mark placement (`top + row / max(rows, track_h / row_h) * track_h`). Short documents
/// only use the top of the track, so the space below their last row maps to no row.
pub fn row_at_track_y(y: f32, track_top: f32, track_h: f32, rows: usize, row_h: f32) -> Option<usize> {
    if rows == 0 || track_h.is_nan() || track_h <= 0.0 || row_h.is_nan() || row_h <= 0.0 || !y.is_finite() {
        return None;
    }
    let t = (y - track_top) / track_h;
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    let scale = (rows as f32).max(track_h / row_h);
    let r = t * scale;
    if r > rows as f32 {
        return None;
    }
    Some((r.floor() as usize).min(rows - 1))
}

/// Buffer line at height `y` of the track, through the display rows of the folded regions
/// (`None`: nothing folded, one row per line).
pub fn line_at_track_y(y: f32, track_top: f32, track_h: f32, row_h: f32, display_rows: Option<&[usize]>, n_lines: usize) -> Option<usize> {
    let rows = display_rows.map_or(n_lines, <[usize]>::len);
    let row = row_at_track_y(y, track_top, track_h, rows, row_h)?;
    match display_rows {
        None => Some(row),
        Some(r) => r.get(row).copied(),
    }
}

/// The `count` display rows centred on `center`, kept inside `0..rows`.
pub fn lens_rows(center: usize, rows: usize, count: usize) -> std::ops::Range<usize> {
    let count = count.min(rows);
    let start = center.saturating_sub(count / 2).min(rows - count);
    start..start + count
}

/// Index of the mark nearest to `y` within the slop, if any. Marks are `(centre y, item)`.
pub fn mark_at<T: Copy>(y: f32, marks: &[(f32, T)]) -> Option<T> {
    marks
        .iter()
        .map(|&(my, item)| ((my - y).abs(), item))
        .filter(|(d, _)| *d <= MARK_SLOP)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, item)| item)
}

/// Works out and paints the lens for a pointer at `p` over `track`; `marks` are the error
/// stripe marks (centre y, buffer line). Fills `state.scroll_lens`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(ui: &Ui, state: &mut EditorState, config: &EditorConfig, scheme: &Scheme, pal: &rc_kit::Palette, m: &Metrics, editor: Rect, track: Rect, p: Pos2, marks: &[(f32, usize)]) {
    let n_lines = state.buffer.len_lines();
    let line = match mark_at(p.y, marks) {
        Some(l) => Some(l),
        None => {
            let rows = state.display_rows().map(<[usize]>::to_vec);
            line_at_track_y(p.y, track.top(), track.height(), m.row_h, rows.as_deref(), n_lines)
        }
    };
    let Some(line) = line.filter(|&l| l < n_lines) else { return };
    let center = crate::view::row_of_line(state, line);
    let total = crate::view::display_row_count(state);
    if total == 0 {
        return;
    }
    let lines: Vec<usize> = lens_rows(center, total, LENS_ROWS).map(|r| crate::view::line_of_row(state, r)).collect();
    let rope = state.buffer.rope().clone();
    let (ls, le) = (rope.line_to_char(line), rope.line_to_char(line) + rope.line(line).len_chars());
    let message = state
        .diagnostics
        .iter()
        .filter(|d| d.severity <= Severity::Warning && d.start >= ls && d.start < le.max(ls + 1))
        .min_by_key(|d| d.severity)
        .map(|d| (d.severity, d.message.lines().next().unwrap_or_default().chars().take(300).collect::<String>()));

    // Geometry: the lens sits left of the scrollbar, centred on the pointer, inside the window.
    let digits = n_lines.max(1).to_string().len().max(2) as f32;
    let gutter_w = (digits * m.char_w + 16.0).round();
    let w = (editor.width() * 0.75).clamp(120.0, 760.0).min(editor.width() - track.width() - 8.0).max(60.0);
    let msg_galley = message.as_ref().map(|(_, text)| ui.painter().layout(text.clone(), egui::FontId::proportional(12.0), pal.text, w - 28.0));
    let msg_h = msg_galley.as_ref().map_or(0.0, |g| g.size().y + 10.0);
    let code_h = lines.len() as f32 * m.row_h;
    let h = code_h + msg_h + 8.0;
    let screen = ui.ctx().content_rect();
    let top = (p.y - h / 2.0).clamp(screen.top(), (screen.bottom() - h).max(screen.top()));
    let rect = Rect::from_min_size(pos2(track.left() - 6.0 - w, top), vec2(w, h));

    let painter = ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::unique(("scroll_lens", state.id)))).with_clip_rect(rect.expand(8.0));
    painter.add(ui.visuals().popup_shadow.as_shape(rect, 6.0));
    painter.rect(rect, 6.0, pal.editor_bg, Stroke::new(1.0, pal.popup_border), egui::StrokeKind::Inside);
    let code_top = rect.top() + 4.0;
    let code = Rect::from_min_max(pos2(rect.left() + 1.0, code_top), pos2(rect.right() - 1.0, code_top + code_h));
    let text_painter = painter.with_clip_rect(Rect::from_min_max(pos2(code.left() + gutter_w, code.top()), code.max));
    let x0 = code.left() + gutter_w + TEXT_PAD;
    for (i, &l) in lines.iter().enumerate() {
        let y = code_top + i as f32 * m.row_h;
        if l == line {
            painter.rect_filled(Rect::from_min_size(pos2(code.left(), y), vec2(code.width(), m.row_h)), 0.0, pal.caret_row);
        }
        let color = if l == line { pal.gutter_fg_caret } else { pal.gutter_fg };
        let g = painter.layout_no_wrap((l + 1).to_string(), mono(12.0), color);
        painter.galley(pos2(code.left() + gutter_w - 8.0 - g.size().x, (y + m.row_h / 2.0 - g.size().y / 2.0).round()), g, color);
        let spans = state.spans_of_line(l);
        let lay = layout_buffer_line(ui, state, config, scheme, l, &spans);
        // The problems marked on the line.
        let start = rope.line_to_char(l);
        for d in state.diagnostics.iter().filter(|d| !d.unnecessary && d.severity <= Severity::Warning && d.end >= start && d.start <= start + lay.len) {
            let (mut a, mut b) = (d.start.max(start) - start, d.end.min(start + lay.len) - start);
            if a == b {
                if b < lay.len { b += 1 } else { a = a.saturating_sub(1) }
            }
            let xa = x0 + lay.x_of_col(a);
            let xb = (x0 + lay.x_of_col(b)).max(xa + m.char_w);
            text_painter.add(wavy(xa, xb, y + m.row_h - 3.0, severity_color(d.severity, scheme)));
        }
        text_painter.galley(pos2(x0, y + m.text_dy), lay.galley, scheme.text);
    }
    if let (Some(g), Some((sev, _))) = (msg_galley, &message) {
        let y = code.bottom() + 4.0;
        painter.hline(rect.x_range().shrink(1.0), y, Stroke::new(1.0, pal.popup_border));
        let color = severity_color(*sev, scheme);
        painter.circle_filled(pos2(rect.left() + 12.0, y + 6.0 + 7.0), 4.0, color);
        painter.galley(pos2(rect.left() + 22.0, y + 6.0), g, pal.text);
    }
    state.scroll_lens = Some(ScrollLens { line, lines, message, rect });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_document_maps_the_whole_track() {
        // 1000 rows on a 500 pt track: two rows per point.
        assert_eq!(row_at_track_y(0.0, 0.0, 500.0, 1000, 20.0), Some(0));
        assert_eq!(row_at_track_y(250.0, 0.0, 500.0, 1000, 20.0), Some(500));
        assert_eq!(row_at_track_y(500.0, 0.0, 500.0, 1000, 20.0), Some(999));
        assert_eq!(row_at_track_y(-1.0, 0.0, 500.0, 1000, 20.0), None);
        assert_eq!(row_at_track_y(501.0, 0.0, 500.0, 1000, 20.0), None);
    }

    #[test]
    fn a_short_document_uses_only_the_top_of_the_track() {
        // 10 rows of 20 pt on 500 pt: marks take the top 200 pt.
        assert_eq!(row_at_track_y(110.0, 100.0, 500.0, 10, 20.0), Some(0));
        assert_eq!(row_at_track_y(100.0 + 199.0, 100.0, 500.0, 10, 20.0), Some(9));
        assert_eq!(row_at_track_y(100.0 + 250.0, 100.0, 500.0, 10, 20.0), None);
        assert_eq!(row_at_track_y(10.0, 0.0, 500.0, 0, 20.0), None);
        assert_eq!(row_at_track_y(10.0, 0.0, 0.0, 10, 20.0), None);
        assert_eq!(row_at_track_y(f32::NAN, 0.0, 500.0, 10, 20.0), None);
    }

    #[test]
    fn the_mapping_inverts_the_mark_placement() {
        let (top, h, row_h, rows) = (40.0, 600.0, 18.0, 1649usize);
        for row in [0, 1, 77, 800, 1648] {
            let y = top + (row as f32 / (rows as f32).max(h / row_h)) * h;
            assert_eq!(row_at_track_y(y + 0.01, top, h, rows, row_h), Some(row));
        }
    }

    #[test]
    fn folded_regions_take_one_row() {
        // Lines 3..=9 folded into line 3: rows 0,1,2,3,10,11...
        let display: Vec<usize> = (0..=3).chain(10..20).collect();
        let rows = display.len() as f32; // 14
        let at = |row: usize| (row as f32 + 0.5) / rows * 280.0;
        assert_eq!(line_at_track_y(at(3), 0.0, 280.0, 20.0, Some(&display), 20), Some(3));
        assert_eq!(line_at_track_y(at(4), 0.0, 280.0, 20.0, Some(&display), 20), Some(10));
        assert_eq!(line_at_track_y(at(13), 0.0, 280.0, 20.0, Some(&display), 20), Some(19));
        assert_eq!(line_at_track_y(at(4), 0.0, 280.0, 20.0, None, 14), Some(4));
    }

    #[test]
    fn the_lens_window_stays_inside_the_document() {
        assert_eq!(lens_rows(0, 100, 9), 0..9);
        assert_eq!(lens_rows(50, 100, 9), 46..55);
        assert_eq!(lens_rows(99, 100, 9), 91..100);
        assert_eq!(lens_rows(2, 4, 9), 0..4);
    }

    #[test]
    fn the_nearest_mark_within_the_slop_wins() {
        let marks = [(10.0, 1usize), (14.0, 2), (40.0, 3)];
        assert_eq!(mark_at(11.0, &marks), Some(1));
        assert_eq!(mark_at(13.0, &marks), Some(2));
        assert_eq!(mark_at(25.0, &marks), None);
        assert_eq!(mark_at(42.5, &marks), Some(3));
        assert_eq!(mark_at::<usize>(1.0, &[]), None);
    }
}
