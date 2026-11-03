//! The editor islands: groups of tabs split in any direction, each with its strip and editor,
//! in the main window and in floating windows. Tabs drag between groups, onto a group's edge
//! to split it, or out of the window to float. With no file open, the most useful shortcuts.

use crate::app::CrabCraftApp;
use crate::layout::{Drop, Edge, Group, GroupId, Node};
use egui::{Align2, FontId, Pos2, Rect, Sense, Ui, pos2, vec2};
use rc_editor::{Action, EditorEvent, EditorView};
use rc_kit::i18n::tr;
use rc_kit::{ico, icons, theme};

const TABS_H: f32 = 38.0;

/// A tab being dragged. The window it started in keeps the pointer until the button is
/// released, so the position is kept in screen points for the other windows to test.
#[derive(Clone, Debug)]
pub struct TabDrag {
    pub doc: u64,
    pub viewport: egui::ViewportId,
    pub screen: Option<Pos2>,
    /// Where the tab goes on release, set by the group under the pointer this frame.
    pub target: Option<Drop>,
    /// Whether the pointer is over one of CrabCraft's windows this frame.
    pub over_window: bool,
    pub released: bool,
}

#[derive(Clone, PartialEq)]
enum TabCommand {
    Close(usize),
    CloseOthers(usize),
    CloseAll(usize),
    CloseRight(usize),
    Pin(usize),
    CopyPath(usize),
    Reveal(usize),
    Locate(usize),
    Blame(usize),
    Diff(usize),
    LocalHistory(usize),
    Split(usize, bool),
    Float(usize),
    Rename(usize),
    ToMain(usize),
    Unsplit(GroupId),
    UnsplitAll(GroupId),
    /// A divider moved: the split at `path` (steps into `a`/`b`) of a window's tree.
    Ratio { window: Option<u64>, path: Vec<bool>, ratio: f32 },
}

pub fn show(app: &mut CrabCraftApp, ui: &mut Ui, editor_actions: Vec<Action>) {
    if app.vcs.diff.is_some() {
        crate::ui::git::diff_view(app, ui);
        return;
    }
    app.sync_layout();
    let focus = std::mem::take(&mut app.focus_editor);
    app.editor_frame = Some((focus, editor_actions));
    if let Some(d) = &mut app.tab_drag {
        d.target = None;
        d.over_window = false;
    }
    track_drag(app, ui.ctx());
    if app.docs.list.is_empty() {
        let rect = ui.max_rect();
        empty_state(app, ui, rect);
        return;
    }
    let root = app.layout.main.clone();
    let mut commands = Vec::new();
    show_node(app, ui, &root, ui.max_rect(), None, &mut Vec::new(), &mut commands);
    mark_over_window(app, ui);
    run_tab_commands(app, ui.ctx(), commands);
}

/// The window the drag started in: where the pointer is, and whether the button is still down.
fn track_drag(app: &mut CrabCraftApp, ctx: &egui::Context) {
    let Some(d) = &mut app.tab_drag else { return };
    if d.viewport != ctx.viewport_id() {
        return;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.tab_drag = None;
        return;
    }
    if let Some(p) = ctx.input(|i| i.pointer.latest_pos()) {
        d.screen = Some(p + origin(ctx).to_vec2());
    }
    d.released = ctx.input(|i| !i.pointer.any_down());
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
}

/// Top left of this window's content on the screen (zero when the system does not say, as in
/// tests or when the window is drawn inside the main one).
fn origin(ctx: &egui::Context) -> Pos2 {
    ctx.input(|i| i.viewport().inner_rect.map(|r| r.min)).unwrap_or(Pos2::ZERO)
}

/// The dragged tab's pointer in this window's points.
fn drag_pointer(app: &CrabCraftApp, ctx: &egui::Context) -> Option<Pos2> {
    let screen = app.tab_drag.as_ref()?.screen?;
    Some(screen - origin(ctx).to_vec2())
}

fn mark_over_window(app: &mut CrabCraftApp, ui: &Ui) {
    let inside = drag_pointer(app, ui.ctx()).is_some_and(|p| ui.ctx().viewport_rect().contains(p));
    if let Some(d) = &mut app.tab_drag {
        d.over_window |= inside;
    }
}

/// After every window was drawn: a released tab goes where it was dropped, or into a new
/// window when it left CrabCraft's windows.
fn finish_drag(app: &mut CrabCraftApp) {
    let Some(d) = app.tab_drag.as_ref().filter(|d| d.released) else { return };
    let d = d.clone();
    app.tab_drag = None;
    let to = match (d.target, d.screen) {
        (Some(t), _) => t,
        (None, Some(p)) if !d.over_window => Drop::Window(p - vec2(40.0, 20.0)),
        _ => return,
    };
    move_doc(app, d.doc, to);
}

fn move_doc(app: &mut CrabCraftApp, doc: u64, to: Drop) {
    if app.layout.move_doc(doc, to) {
        if let Some(i) = app.docs.list.iter().position(|d| d.editor.id == doc) {
            app.docs.activate(i);
        }
        app.focus_editor = true;
        app.session_dirty = true;
    }
}

/// A node of a window's tree in `rect`: a group, or two parts and the divider between them.
fn show_node(app: &mut CrabCraftApp, ui: &mut Ui, node: &Node, rect: Rect, window: Option<u64>, path: &mut Vec<bool>, commands: &mut Vec<TabCommand>) {
    let (vertical, ratio, a, b) = match node {
        Node::Group(g) => return show_group(app, ui, g, rect, window, commands),
        Node::Split { vertical, ratio, a, b } => (*vertical, *ratio, a, b),
    };
    let p = rc_kit::pal(ui);
    let divider = if vertical {
        let x = (rect.left() + rect.width() * ratio).round();
        Rect::from_min_max(pos2(x - 3.0, rect.top()), pos2(x + 3.0, rect.bottom()))
    } else {
        let y = (rect.top() + rect.height() * ratio).round();
        Rect::from_min_max(pos2(rect.left(), y - 3.0), pos2(rect.right(), y + 3.0))
    };
    let resp = ui.interact(divider, egui::Id::new(("editor_split", window, path.clone())), Sense::drag());
    let mut ratio = ratio;
    if resp.dragged()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        ratio = if vertical { (pos.x - rect.left()) / rect.width() } else { (pos.y - rect.top()) / rect.height() }.clamp(0.1, 0.9);
        commands.push(TabCommand::Ratio { window, path: path.clone(), ratio });
    }
    let (ra, rb) = if vertical {
        let x = (rect.left() + rect.width() * ratio).round();
        (Rect::from_min_max(rect.min, pos2(x - 1.0, rect.bottom())), Rect::from_min_max(pos2(x + 1.0, rect.top()), rect.max))
    } else {
        let y = (rect.top() + rect.height() * ratio).round();
        (Rect::from_min_max(rect.min, pos2(rect.right(), y - 1.0)), Rect::from_min_max(pos2(rect.left(), y + 1.0), rect.max))
    };
    for (part, r, step) in [(a, ra, false), (b, rb, true)] {
        path.push(step);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt(("editor_part", step)));
        child.set_clip_rect(r.intersect(ui.clip_rect()));
        show_node(app, &mut child, part, r, window, path, commands);
        path.pop();
    }
    let line = if resp.hovered() || resp.dragged() { p.accent } else { p.island_border };
    if vertical {
        ui.painter().vline(ra.right() + 1.0, rect.y_range(), egui::Stroke::new(1.0, line));
    } else {
        ui.painter().hline(rect.x_range(), ra.bottom() + 1.0, egui::Stroke::new(1.0, line));
    }
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(if vertical { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
    }
}

/// One group: its tab strip and the editor of its active tab.
fn show_group(app: &mut CrabCraftApp, ui: &mut Ui, g: &Group, rect: Rect, window: Option<u64>, commands: &mut Vec<TabCommand>) {
    let p = rc_kit::pal(ui);
    if g.tabs.is_empty() {
        // Every file is in another window: the main one waits for tabs.
        empty_state(app, ui, rect);
        drop_zones(app, ui, g, rect, None);
        return;
    }
    let (bottom, tabs) = tab_strip(app, ui, g, rect, window, commands);
    ui.painter().hline(rect.x_range(), bottom - 0.5, egui::Stroke::new(1.0, p.island_border));
    let strip = Rect::from_min_max(rect.min, pos2(rect.right(), bottom));
    let body = Rect::from_min_max(pos2(rect.left(), bottom), rect.max);
    if let Some(idx) = g.active.and_then(|a| app.docs.list.iter().position(|d| d.editor.id == a)) {
        let active = app.docs.active == Some(idx);
        let (focus, actions) = if active { app.editor_frame.take().unwrap_or_default() } else { (false, Vec::new()) };
        let mut pane = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt(("editor_pane", g.id)));
        if editor_pane(app, &mut pane, idx, body, actions, focus, active) && !active {
            // A click in another group makes it the active one.
            app.docs.activate(idx);
        }
    }
    drop_zones(app, ui, g, body, Some((strip, tabs)));
}

/// While a tab is dragged over this group: where it would go, highlighted, and remembered as
/// the drop target. Over the strip it goes between tabs; over the editor, into the group, or
/// beside it near an edge.
fn drop_zones(app: &mut CrabCraftApp, ui: &Ui, g: &Group, body: Rect, strip: Option<(Rect, Vec<Rect>)>) {
    let Some(doc) = app.tab_drag.as_ref().map(|d| d.doc) else { return };
    let Some(pointer) = drag_pointer(app, ui.ctx()) else { return };
    let over_strip = strip.as_ref().is_some_and(|(s, _)| s.contains(pointer));
    if !over_strip && !body.contains(pointer) {
        return;
    }
    let p = rc_kit::pal(ui);
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("tab_drop", g.id))));
    let from = app.layout.group_of(doc);
    let alone = from.and_then(|f| app.layout.group(f)).is_some_and(|f| f.tabs.len() == 1);
    let target = match strip {
        Some((strip, tabs)) if over_strip => {
            let index = tabs.iter().position(|r| pointer.x < r.center().x && pointer.y <= r.bottom() + 2.0).unwrap_or(tabs.len());
            match tabs.get(index.min(tabs.len().saturating_sub(1))) {
                Some(r) => {
                    let x = if index < tabs.len() { r.left() - 1.0 } else { r.right() + 1.0 };
                    painter.vline(x, r.y_range(), egui::Stroke::new(2.0, p.accent));
                }
                None => {
                    painter.rect_stroke(strip.shrink(2.0), 4.0, egui::Stroke::new(1.5, p.accent), egui::StrokeKind::Inside);
                }
            }
            Some(Drop::Group { group: g.id, index: Some(index) })
        }
        _ => {
            let fx = (pointer.x - body.left()) / body.width().max(1.0);
            let fy = (pointer.y - body.top()) / body.height().max(1.0);
            let edges = [(fx, Edge::Left), (1.0 - fx, Edge::Right), (fy, Edge::Top), (1.0 - fy, Edge::Bottom)];
            let near = edges.into_iter().filter(|(d, _)| *d < 0.25).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, e)| e);
            let (zone, drop) = match near {
                // An empty group (the main window's, every tab elsewhere) only takes tabs.
                Some(edge) if !g.tabs.is_empty() && !(from == Some(g.id) && alone) => {
                    let c = body.center();
                    let half = match edge {
                        Edge::Left => Rect::from_min_max(body.min, pos2(c.x, body.bottom())),
                        Edge::Right => Rect::from_min_max(pos2(c.x, body.top()), body.max),
                        Edge::Top => Rect::from_min_max(body.min, pos2(body.right(), c.y)),
                        Edge::Bottom => Rect::from_min_max(pos2(body.left(), c.y), body.max),
                    };
                    (Some(half), Some(Drop::Split { group: g.id, edge }))
                }
                _ if from != Some(g.id) => (Some(body), Some(Drop::Group { group: g.id, index: None })),
                _ => (None, None),
            };
            if let Some(zone) = zone {
                painter.rect(zone.shrink(4.0), 6.0, p.accent.gamma_multiply(0.16), egui::Stroke::new(1.5, p.accent), egui::StrokeKind::Inside);
            }
            drop
        }
    };
    if let Some(d) = &mut app.tab_drag {
        d.target = target;
    }
}

/// One editor: conflict banner, find bar (when focused), the editor and its events. Returns
/// whether it was clicked or got the keyboard (so a split pane becomes the active one).
pub fn editor_pane(app: &mut CrabCraftApp, ui: &mut Ui, active: usize, rect: Rect, editor_actions: Vec<Action>, focus: bool, focused: bool) -> bool {
    let p = rc_kit::pal(ui);
    let editor_rect = rect;
    // Conflict banner: the file changed on disk while it had unsaved edits.
    let mut editor_rect = editor_rect;
    if app.docs.list[active].conflict {
        let banner = Rect::from_min_size(editor_rect.min, vec2(editor_rect.width(), 34.0));
        ui.painter().rect_filled(banner, 0.0, rc_kit::theme::blend(p.island_bg, p.warn.gamma_multiply(0.18)));
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(banner.shrink2(vec2(12.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        icons::image(&mut b, ico::WARNING, 16.0);
        b.label(tr("The file was changed on disk. Keep your version or load the one on disk?"));
        if b.button(tr("Load from Disk")).clicked() {
            let path = app.docs.list[active].path.clone();
            if let Ok((text, bom, mtime)) = crate::docs::read_text(&path) {
                // The unsaved edits it discards stay in the local history.
                app.docs.remember(active);
                app.docs.reloaded(active, &text, bom, mtime);
            }
        }
        if b.button(tr("Keep Mine")).clicked() {
            app.docs.list[active].conflict = false;
            if let Err(e) = app.docs.save(active) {
                app.notify(e, true);
            }
        }
        editor_rect.min.y = banner.bottom();
    }
    // LightEdit: a file opened without its project; offer to open the whole project.
    if app.workspace.is_none()
        && let Some(project) = app.docs.list[active].light_project.clone()
    {
        let banner = Rect::from_min_size(editor_rect.min, vec2(editor_rect.width(), 34.0));
        ui.painter().rect_filled(banner, 0.0, rc_kit::theme::blend(p.island_bg, p.accent.gamma_multiply(0.12)));
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(banner.shrink2(vec2(12.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        icons::image(&mut b, ico::INFO, 16.0);
        let name = project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        b.label(rc_kit::i18n::trf("LightEdit: this file belongs to the Cargo project {name}. Code insight, run and build need the project.", &[("name", &name)]));
        if b.button(tr("Open Project")).clicked() {
            app.open_workspace(&project);
            return false;
        }
        editor_rect.min.y = banner.bottom();
    }
    // A Rust scratch file is outside every Cargo package: say that code insight is missing.
    if app.docs.list[active].scratch && app.docs.list[active].editor.lang() == rc_syntax::Lang::Rust {
        let banner = Rect::from_min_size(editor_rect.min, vec2(editor_rect.width(), 34.0));
        ui.painter().rect_filled(banner, 0.0, rc_kit::theme::blend(p.island_bg, p.warn.gamma_multiply(0.18)));
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(banner.shrink2(vec2(12.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        // With a project open, rust-analyzer analyses it on its own; without one nothing does.
        if app.workspace.is_some() {
            icons::image(&mut b, ico::INFO, 16.0);
            b.label(tr("Scratch file: analysed on its own, outside the project's crates"));
        } else {
            icons::image(&mut b, ico::WARNING, 16.0);
            b.label(tr("Scratch file: open a project to have it analysed"));
        }
        if b.button(tr("Run")).on_hover_text(tr("Build it with rustc and run it")).clicked() {
            let path = app.docs.list[active].path.clone();
            app.save_all_docs();
            let outs = app.runner.run_scratch(&path);
            crate::ui::run_windows::handle_outputs(app, outs);
        }
        editor_rect.min.y = banner.bottom();
    }
    // A Rust file no module declares: rust-analyzer knows nothing about its code.
    if app.insight.unlinked.contains(&app.docs.list[active].editor.id) {
        let path = app.docs.list[active].path.clone();
        let banner = Rect::from_min_size(editor_rect.min, vec2(editor_rect.width(), 34.0));
        ui.painter().rect_filled(banner, 0.0, rc_kit::theme::blend(p.island_bg, p.warn.gamma_multiply(0.18)));
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(banner.shrink2(vec2(12.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        icons::image(&mut b, ico::WARNING, 16.0);
        b.label(tr("No module declares this file"));
        let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parent = path.parent().and_then(rc_cargo::modules::parent_module).filter(|p| *p != path && rc_cargo::modules::is_module_name(&name));
        if let Some(parent) = parent {
            let file = parent.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if b.button(rc_kit::i18n::trf("Declare in {file}", &[("file", &file)])).clicked() {
                app.declare_module(&parent, &name, false);
                app.insight.unlinked.remove(&app.docs.list[active].editor.id);
            }
        }
        editor_rect.min.y = banner.bottom();
    }
    // Markdown: the rendered page beside or instead of the editor.
    let pane = editor_rect;
    let mut preview_clicked = false;
    if let Some(view) = app.docs.list[active].md_view {
        let (ed, preview) = crate::ui::markdown::layout(view, editor_rect);
        if let Some(r) = preview {
            preview_clicked = crate::ui::markdown::preview(app, ui, active, r);
        }
        crate::ui::markdown::switcher(app, ui, active, pane);
        match ed {
            Some(r) => editor_rect = r,
            None => return preview_clicked,
        }
    }
    if focused {
        let bar_h = crate::ui::search_popup::find_bar(app, ui, editor_rect);
        editor_rect.min.y += bar_h;
    }
    let mut ed_ui = ui.new_child(egui::UiBuilder::new().max_rect(editor_rect));
    ed_ui.set_clip_rect(editor_rect.intersect(ui.clip_rect()));
    let doc = &mut app.docs.list[active];
    let mut from_menu = Vec::new();
    for a in &editor_actions {
        from_menu.extend(rc_editor::run_action(&ed_ui, &mut doc.editor, &app.editor_config, *a));
    }
    // The context actions popup reads the keys itself, so the editor must not act on them too.
    if matches!(app.dialog, Some(crate::ui::dialogs::Dialog::CodeActions { .. })) {
        ui.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
    let out = EditorView::new(&mut doc.editor, &app.editor_config, &app.keymap).focus(focus).show(&mut ed_ui);
    let touched = preview_clicked || out.response.clicked() || out.response.secondary_clicked() || out.events.contains(&EditorEvent::Focused);
    // Shown first, so the frame of the click that opens it does not count as a click outside.
    gutter_menu(app, ui);
    let opened = out.events.iter().find_map(|e| match e {
        EditorEvent::GutterIconSecondaryClicked { line, icon, screen } if is_run_icon(*icon) => Some((*line, *screen)),
        _ => None,
    });
    if opened.is_some() {
        app.gutter_menu = opened;
    }
    if app.gutter_menu.is_none() {
        editor_context_menu(app, active, &out.response);
    }
    let doc = &mut app.docs.list[active];
    // While paused, hovering shows the debugger's value instead of the symbol's documentation.
    // Before the insertion reaches the server, which resolves the item's import on the old text.
    if let Some(data) = out.accepted_completion {
        if data >= crate::manifest_completion::LOCAL_ITEM_THEN_VALUE {
            crate::manifest_completion::accepted(doc, data);
        } else {
            app.insight.completion_accepted(doc, data);
        }
    }
    if crate::manifest_completion::is_manifest(doc) {
        crate::manifest_completion::on_editor_events(doc, &out.events);
    }
    if app.debugger.stopped() {
        let insight_events: Vec<_> = out.events.iter().filter(|e| !matches!(e, EditorEvent::HoverAt { .. })).cloned().collect();
        app.insight.on_editor_events(doc, &insight_events);
        for e in &out.events {
            match e {
                EditorEvent::HoverAt { pos, screen } => {
                    let expr = crate::debugger::expression_at(doc.editor.buffer.rope(), *pos);
                    app.debugger.hover(doc.editor.id, *pos, *screen, expr);
                }
                EditorEvent::HoverEnd | EditorEvent::Changed | EditorEvent::CaretMoved => app.debugger.hover_result = None,
                _ => {}
            }
        }
    } else {
        app.insight.on_editor_events(doc, &out.events);
    }
    let mut run_test_line = None;
    let mut run_binary_line = None;
    let mut explain = None;
    let mut feature_line = None;
    let mut docs_rs_line = None;
    // ⌘+click: the clicked spot goes into Back/Forward history.
    let navigated_from = out.events.iter().find_map(|e| matches!(e, EditorEvent::NavigateAt(_)).then(|| (doc.path.clone(), doc.editor.caret())));
    let caret_screen = out.caret_rect.map_or(editor_rect.center_top(), |r| r.left_bottom());
    app.caret_screen = Some(caret_screen);
    for ev in out.events {
        match ev {
            EditorEvent::GutterClicked { line } => {
                doc.editor.toggle_breakpoint(line);
                if !doc.editor.breakpoints.contains_key(&line) {
                    app.debugger.forget_breakpoint(&doc.path, line);
                }
                app.session_dirty = true;
            }
            EditorEvent::GutterSecondaryClicked { line, .. } => {
                // The editor's context menu opened on the same click: the dialog replaces it.
                egui::Popup::close_all(ui.ctx());
                let path = doc.path.clone();
                let present = doc.editor.breakpoints.get(&line).copied();
                if present.is_none() {
                    doc.editor.breakpoints.insert(line, rc_editor::BreakpointKind::Normal);
                }
                let condition = app.debugger.conditions.get(&(path.clone(), line)).cloned().unwrap_or_default();
                let enabled = present != Some(rc_editor::BreakpointKind::Disabled);
                let log = app.debugger.logs.get(&(path.clone(), line)).cloned().unwrap_or_default();
                let once = app.debugger.once.contains(&(path.clone(), line));
                app.dialog = Some(crate::ui::dialogs::Dialog::BreakpointProperties { path, line, condition, log, once, enabled, focus: true });
                app.session_dirty = true;
            }
            EditorEvent::CaretMoved => app.session_dirty = true,
            // Editing a preview tab keeps it.
            EditorEvent::Changed => doc.preview = false,
            EditorEvent::ExplainError(code) => explain = Some(code),
            EditorEvent::GutterIconClicked { line, icon: rc_editor::GutterIcon::RunApp } => run_binary_line = Some(line),
            EditorEvent::GutterIconClicked { line, icon: rc_editor::GutterIcon::FeatureOn | rc_editor::GutterIcon::FeatureOff } => feature_line = Some(line),
            EditorEvent::GutterIconClicked { line, icon: rc_editor::GutterIcon::DocsRs } => docs_rs_line = Some(line),
            EditorEvent::GutterIconClicked { line, icon: rc_editor::GutterIcon::RunTest | rc_editor::GutterIcon::TestPassed | rc_editor::GutterIcon::TestFailed } => run_test_line = Some(line),
            _ => {}
        }
    }
    if let Some((d, _, screen, text)) = &app.debugger.hover_result
        && *d == doc.editor.id
    {
        let p = rc_kit::pal(ui);
        egui::Area::new(egui::Id::new("debug_hover")).order(egui::Order::Tooltip).fixed_pos(*screen + vec2(0.0, 14.0)).show(ui.ctx(), |ui| {
            egui::Frame::new().fill(p.popup_bg).stroke(egui::Stroke::new(1.0, p.popup_border)).corner_radius(6.0).inner_margin(egui::Margin::symmetric(8, 5)).show(ui, |ui| {
                ui.set_max_width(520.0);
                ui.label(egui::RichText::new(text).monospace().size(12.5).color(p.text));
            });
        });
    }
    let mut insight_actions = Vec::new();
    for a in out.actions {
        match a {
            Action::ToggleBookmark => {
                let line = doc.editor.caret_line_col().0;
                doc.editor.toggle_bookmark(line);
                app.session_dirty = true;
            }
            Action::ToggleBreakpoint => {
                let line = doc.editor.caret_line_col().0;
                doc.editor.toggle_breakpoint(line);
                if !doc.editor.breakpoints.contains_key(&line) {
                    app.debugger.forget_breakpoint(&doc.path, line);
                }
                app.session_dirty = true;
            }
            Action::NextError | Action::PreviousError => {
                doc.editor.next_diagnostic(a == Action::NextError);
            }
            Action::Escape => {
                if app.find_bar.is_some() {
                    app.find_bar = None;
                    doc.editor.search_matches.clear();
                    doc.editor.current_match = None;
                }
            }
            Action::Find | Action::Replace => {
                let sel = doc.editor.selected_text();
                let bar = app.find_bar.get_or_insert_with(Default::default);
                if !sel.is_empty() && !sel.contains('\n') {
                    bar.query = sel;
                }
                if a == Action::Replace && bar.replace.is_none() {
                    bar.replace = Some(String::new());
                }
                bar.focus = true;
            }
            Action::FindNext | Action::FindPrevious => {
                let n = doc.editor.search_matches.len();
                if n > 0 {
                    let cur = doc.editor.current_match.unwrap_or(0);
                    let next = if a == Action::FindNext { (cur + 1) % n } else { (cur + n - 1) % n };
                    let (s, e) = doc.editor.search_matches[next];
                    let keep = doc.editor.search_matches.clone();
                    doc.editor.select_range(s, e);
                    doc.editor.search_matches = keep;
                    doc.editor.current_match = Some(next);
                }
            }
            // Global shortcuts typed in a floating window or a split pane.
            other if other.is_ide() => app.pending.push(other),
            other => insight_actions.push(other),
        }
    }
    insight_actions.extend(from_menu);
    if let Some(line) = run_binary_line {
        let ctx = ui.ctx().clone();
        app.run_binary_at(&ctx, line);
    }
    if let Some(line) = run_test_line {
        app.run_test_at(line);
    }
    if let Some(code) = explain {
        app.explain_error(&code);
    }
    if let Some(line) = feature_line {
        app.toggle_feature_at(line);
    }
    if let Some(line) = docs_rs_line {
        app.open_docs_rs_at(line);
    }
    if let Some((path, pos)) = navigated_from {
        app.push_nav_at(path, pos);
    }
    for a in insight_actions {
        app.run_insight_action(a, caret_screen);
    }
    touched
}

/// Right-click in the editor (Rider): code actions and navigation for C#, editing, formatting.
fn is_run_icon(icon: rc_editor::GutterIcon) -> bool {
    use rc_editor::GutterIcon::*;
    matches!(icon, RunApp | RunTest | TestPassed | TestFailed)
}

/// Run and Debug for a gutter icon. Debug is there for what rust-analyzer found in a Rust file.
fn gutter_menu(app: &mut CrabCraftApp, ui: &Ui) {
    let Some((line, pos)) = app.gutter_menu else { return };
    let can_debug = app.docs.active_doc().and_then(|d| app.insight.runnables.get(&d.editor.id)).is_some_and(|(_, list)| list.iter().any(|r| r.line == line));
    let ctx = ui.ctx().clone();
    let p = rc_kit::pal(ui);
    let mut chosen = None;
    let area = egui::Area::new(egui::Id::new("gutter_run_menu")).order(egui::Order::Foreground).fixed_pos(pos).show(&ctx, |ui| {
        egui::Frame::new().fill(p.popup_bg).stroke(egui::Stroke::new(1.0, p.popup_border)).corner_radius(6.0).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
            ui.set_width(150.0);
            if theme::menu_entry(ui, Some(ico::RUN), tr("Run"), None, false, true).clicked() {
                chosen = Some(false);
            }
            if can_debug && theme::menu_entry(ui, Some(ico::DEBUG), tr("Debug"), None, false, true).clicked() {
                chosen = Some(true);
            }
        });
    });
    match chosen {
        Some(debug) => {
            app.gutter_menu = None;
            if debug {
                app.debug_runnable_at(&ctx, line);
            } else if app.run_markers.contains_key(&line) {
                app.run_binary_at(&ctx, line);
            } else {
                app.run_test_at(line);
            }
        }
        None if area.response.clicked_elsewhere() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) => app.gutter_menu = None,
        None => {}
    }
}

fn editor_context_menu(app: &mut CrabCraftApp, active: usize, resp: &egui::Response) {
    use crate::native_menu::{MenuCommand, forward_edit};
    let insight = app.docs.active_doc().is_some_and(crate::insight::has_insight);
    let is_csharp = app.docs.active_doc().is_some_and(|d| d.editor.lang() == rc_syntax::Lang::CSharp);
    let shortcut = |app: &CrabCraftApp, a: Action| app.keymap.shortcut_text(a);
    let mut actions = Vec::new();
    let mut edit = None;
    let mut usings = false;
    resp.context_menu(|ui| {
        ui.set_min_width(260.0);
        ui.set_max_width(320.0);
        let mut entry = |ui: &mut Ui, icon: Option<rc_kit::Ico>, text: &str, action: Action| {
            if theme::menu_entry(ui, icon, text, shortcut(app, action).as_deref(), false, true).clicked() {
                actions.push(action);
                ui.close();
            }
        };
        if insight {
            entry(ui, Some(ico::BULB), tr("Show Context Actions"), Action::ShowIntentions);
            entry(ui, None, tr("Go to Declaration"), Action::GotoDeclaration);
            entry(ui, None, tr("Go to Implementation"), Action::GotoImplementation);
            entry(ui, Some(ico::TW_FIND), tr("Find Usages"), Action::FindUsages);
            entry(ui, Some(ico::EDIT), tr("Rename…"), Action::Rename);
            entry(ui, Some(ico::BULB), tr("Refactor This…"), Action::RefactorThis);
            ui.separator();
        }
        for (icon, text, cmd) in [(ico::CUT, tr("Cut"), MenuCommand::Cut), (ico::COPY, tr("Copy"), MenuCommand::Copy), (ico::PASTE, tr("Paste"), MenuCommand::Paste)] {
            if theme::menu_item(ui, Some(icon), text, false).clicked() {
                edit = Some(cmd);
                ui.close();
            }
        }
        ui.separator();
        entry(ui, Some(ico::REFORMAT), tr("Reformat Code"), Action::Reformat);
        entry(ui, None, tr("Comment with Line Comment"), Action::CommentLine);
        if is_csharp && theme::menu_item(ui, Some(ico::NAMESPACE), tr("Add Missing Usings"), false).clicked() {
            usings = true;
            ui.close();
        }
        ui.separator();
        entry(ui, Some(ico::BREAKPOINT), tr("Toggle Line Breakpoint"), Action::ToggleBreakpoint);
    });
    // The editor has already read this frame's input, so it copies and cuts here.
    match edit {
        Some(MenuCommand::Copy) => rc_editor::copy_selection(&resp.ctx, &mut app.docs.list[active].editor, false),
        Some(MenuCommand::Cut) => rc_editor::copy_selection(&resp.ctx, &mut app.docs.list[active].editor, true),
        // The clipboard's text arrives as an event next frame: the editor must have the keyboard.
        Some(cmd) => {
            resp.request_focus();
            forward_edit(&resp.ctx, cmd);
        }
        None => {}
    }
    if usings && let Some(i) = app.docs.active {
        app.insight.add_missing_usings(&app.docs.list[i]);
    }
    app.pending.extend(actions);
}

/// Whether `path` belongs to a test project (its nearest `.csproj`), cached per project.
fn in_test_project(path: &std::path::Path) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<std::path::PathBuf, bool>>> = Mutex::new(None);
    let mut dir = path.parent();
    while let Some(d) = dir {
        let project = std::fs::read_dir(d).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "csproj")));
        if let Some(p) = project {
            let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            return *cache.get_or_insert_with(HashMap::new).entry(p.clone()).or_insert_with(|| rc_dotnet::ProjectInfo::load(&p).is_ok_and(|i| i.is_test));
        }
        dir = d.parent();
    }
    false
}

/// Rider's file colours, by kind of file: tests green, Razor/HTML/CSS orange, configuration
/// and project files blue; C# keeps the plain tab.
fn file_color(path: &std::path::Path) -> Option<egui::Color32> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let in_tests = in_test_project(path);
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext == "cs" && (in_tests || name.ends_with("tests.cs")) {
        Some(egui::Color32::from_rgb(0x5F, 0xB8, 0x65))
    } else if matches!(ext.as_str(), "razor" | "cshtml" | "html" | "htm" | "css" | "scss" | "js" | "ts") {
        Some(egui::Color32::from_rgb(0xE0, 0x8E, 0x3C))
    } else if matches!(ext.as_str(), "json" | "xml" | "csproj" | "fsproj" | "sln" | "slnx" | "props" | "targets" | "config" | "yaml" | "yml" | "editorconfig") {
        Some(egui::Color32::from_rgb(0x4A, 0x8F, 0xE0))
    } else {
        None
    }
}

/// A group's tab strip: one scrolling row with a list of every tab (Rider's hidden tabs), or
/// several rows (Settings › Editor). Returns the strip's bottom and the rectangles of its tabs.
fn tab_strip(app: &mut CrabCraftApp, ui: &mut Ui, g: &Group, full: Rect, window: Option<u64>, commands: &mut Vec<TabCommand>) -> (f32, Vec<Rect>) {
    let multirow = app.settings.tabs_multirow;
    let colors = app.settings.tab_colors;
    let list_w = if multirow { 0.0 } else { 34.0 };
    // One row: the strip's height, tabs centred. Several rows: from the top, as tall as needed.
    let area = if multirow {
        Rect::from_min_max(full.min + vec2(6.0, 5.0), pos2(full.right() - 6.0, full.bottom()))
    } else {
        Rect::from_min_max(full.min + vec2(6.0, 0.0), pos2(full.right() - 6.0 - list_w, full.top() + TABS_H))
    };
    let several = app.layout.trees().map(|t| t.groups().len()).sum::<usize>() > 1;
    let focused_group = app.layout.focused == g.id;
    let mut rects: Vec<Rect> = Vec::new();
    let layout = if multirow { egui::Layout::top_down(egui::Align::Min) } else { egui::Layout::left_to_right(egui::Align::Center) };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(layout).id_salt(("editor_tabs", g.id)));
    child.set_clip_rect(area.intersect(ui.clip_rect()));
    let mut draw_tabs = |ui: &mut Ui, rects: &mut Vec<Rect>| {
        ui.spacing_mut().item_spacing = vec2(2.0, 4.0);
        for &id in &g.tabs {
            let Some(i) = app.docs.list.iter().position(|d| d.editor.id == id) else { continue };
            let doc = &app.docs.list[i];
            let selected = g.active == Some(id);
            let mut title = doc.name();
            if doc.is_modified() {
                title.push_str(" *");
            }
            let style = theme::TabStyle { italic: doc.preview, pinned: doc.pinned, color: if colors { file_color(&doc.path) } else { None }, draggable: true };
            let (resp, close) = theme::tab_ex(ui, selected, Some(icons::for_file(&doc.name())), &title, true, style);
            rects.push(resp.rect);
            // With several groups, the one that has the keyboard is underlined.
            if selected && several && focused_group {
                let r = resp.rect;
                ui.painter().hline(r.x_range().shrink(6.0), r.bottom() + 1.0, egui::Stroke::new(2.0, rc_kit::pal(ui).accent));
            }
            if selected && app.focus_editor {
                resp.scroll_to_me(None);
            }
            if resp.drag_started() {
                app.tab_drag = Some(TabDrag { doc: id, viewport: ui.ctx().viewport_id(), screen: None, target: None, over_window: true, released: false });
            }
            let resp = resp.on_hover_text(doc.path.display().to_string());
            if close || resp.middle_clicked() {
                commands.push(TabCommand::Close(i));
            } else if resp.double_clicked() {
                // Double click keeps a preview tab open (Rider, VS Code).
                app.docs.list[i].preview = false;
            } else if resp.clicked() {
                app.docs.activate(i);
                app.focus_editor = true;
                app.session_dirty = true;
            }
            tab_menu(app, &resp, i, g, window, commands);
        }
    };
    if multirow {
        child.horizontal_wrapped(|ui| draw_tabs(ui, &mut rects));
    } else {
        // The mouse wheel scrolls the row sideways.
        if ui.rect_contains_pointer(area) {
            ui.input_mut(|i| {
                if i.smooth_scroll_delta.x == 0.0 && i.smooth_scroll_delta.y != 0.0 {
                    i.smooth_scroll_delta = vec2(i.smooth_scroll_delta.y, 0.0);
                }
            });
        }
        egui::ScrollArea::horizontal().id_salt(("editor_tabs_scroll", g.id)).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(&mut child, |ui| {
            ui.horizontal_centered(|ui| draw_tabs(ui, &mut rects));
        });
    }
    let bottom = if multirow { rects.iter().map(|r| r.bottom()).fold(full.top() + TABS_H - 5.0, f32::max) + 5.0 } else { full.top() + TABS_H };
    // Every tab of the group, for the ones scrolled out of sight.
    if !multirow {
        let br = Rect::from_center_size(pos2(full.right() - 6.0 - 15.0, full.top() + TABS_H / 2.0), vec2(26.0, 26.0));
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(br).id_salt(("editor_tabs_all", g.id)));
        let resp = theme::icon_button(&mut b, ico::CHEVRON_DOWN, true, tr("Show All Tabs"));
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(260.0);
            let mut pick = None;
            for &id in &g.tabs {
                let Some(i) = app.docs.list.iter().position(|d| d.editor.id == id) else { continue };
                let d = &app.docs.list[i];
                if theme::menu_item(ui, Some(icons::for_file(&d.name())), &d.name(), g.active == Some(id)).on_hover_text(d.path.display().to_string()).clicked() {
                    pick = Some(i);
                    ui.close();
                }
            }
            if let Some(i) = pick {
                app.docs.activate(i);
                app.focus_editor = true;
            }
        });
    }
    (bottom, rects)
}

fn tab_menu(app: &mut CrabCraftApp, resp: &egui::Response, i: usize, g: &Group, window: Option<u64>, commands: &mut Vec<TabCommand>) {
    resp.context_menu(|ui| {
        ui.set_min_width(220.0);
        let shortcut = app.keymap.shortcut_text(Action::CloseTab).unwrap_or_default();
        if theme::menu_entry(ui, Some(ico::CLOSE), tr("Close"), Some(&shortcut), false, true).clicked() {
            commands.push(TabCommand::Close(i));
            ui.close();
        }
        if theme::menu_item(ui, Some(ico::CLOSE), tr("Close Other Tabs"), false).clicked() {
            commands.push(TabCommand::CloseOthers(i));
            ui.close();
        }
        if theme::menu_item(ui, Some(ico::CLOSE), tr("Close Tabs to the Right"), false).clicked() {
            commands.push(TabCommand::CloseRight(i));
            ui.close();
        }
        if theme::menu_item(ui, Some(ico::CLOSE), tr("Close All Tabs"), false).clicked() {
            commands.push(TabCommand::CloseAll(i));
            ui.close();
        }
        ui.separator();
        let pin = if app.docs.list[i].pinned { tr("Unpin Tab") } else { tr("Pin Tab") };
        if theme::menu_item(ui, Some(ico::PIN), pin, false).clicked() {
            commands.push(TabCommand::Pin(i));
            ui.close();
        }
        // The tab moves to a new group beside this one, so the group needs another tab.
        let others = g.tabs.len() > 1;
        if theme::menu_entry(ui, Some(ico::SPLIT_RIGHT), tr("Split and Move Right"), None, false, others).clicked() {
            commands.push(TabCommand::Split(i, true));
            ui.close();
        }
        if theme::menu_entry(ui, Some(ico::SPLIT_DOWN), tr("Split and Move Down"), None, false, others).clicked() {
            commands.push(TabCommand::Split(i, false));
            ui.close();
        }
        let beside = app.layout.groups_beside(g.id) > 1;
        if theme::menu_entry(ui, None, tr("Unsplit"), None, false, beside).clicked() {
            commands.push(TabCommand::Unsplit(g.id));
            ui.close();
        }
        if theme::menu_entry(ui, None, tr("Unsplit All"), None, false, beside).clicked() {
            commands.push(TabCommand::UnsplitAll(g.id));
            ui.close();
        }
        if window.is_some() {
            if theme::menu_item(ui, Some(ico::EXTERNAL_LINK), tr("Move to Main Window"), false).clicked() {
                commands.push(TabCommand::ToMain(i));
                ui.close();
            }
        } else if theme::menu_entry(ui, Some(ico::EXTERNAL_LINK), tr("Open in New Window"), None, false, true).clicked() {
            commands.push(TabCommand::Float(i));
            ui.close();
        }
        ui.separator();
        if theme::menu_entry(ui, None, tr("Show Tabs in Multiple Rows"), None, app.settings.tabs_multirow, true).clicked() {
            app.settings.tabs_multirow = !app.settings.tabs_multirow;
            let _ = app.settings.save();
            ui.close();
        }
        ui.separator();
        if theme::menu_item(ui, Some(ico::COPY), tr("Copy Path"), false).clicked() {
            commands.push(TabCommand::CopyPath(i));
            ui.close();
        }
        let renamable = !app.docs.list[i].editor.read_only;
        if theme::menu_entry(ui, Some(ico::EDIT), tr("Rename File…"), None, false, renamable).clicked() {
            commands.push(TabCommand::Rename(i));
            ui.close();
        }
        if app.vcs.repo.is_some() {
            let on = app.vcs.blame_for.as_deref() == Some(app.docs.list[i].path.as_path());
            if theme::menu_entry(ui, Some(ico::HISTORY), tr("Annotate with Git Blame"), None, on, true).clicked() {
                commands.push(TabCommand::Blame(i));
                ui.close();
            }
            if theme::menu_item(ui, Some(ico::DIFF), tr("Show Diff"), false).clicked() {
                commands.push(TabCommand::Diff(i));
                ui.close();
            }
        }
        if theme::menu_item(ui, Some(ico::HISTORY), tr("Show Local History"), false).clicked() {
            commands.push(TabCommand::LocalHistory(i));
            ui.close();
        }
        if theme::menu_item(ui, Some(ico::LOCATE), tr("Select in Explorer"), false).clicked() {
            commands.push(TabCommand::Locate(i));
            ui.close();
        }
        let reveal = if cfg!(target_os = "macos") { tr("Reveal in Finder") } else { tr("Open in Explorer") };
        if theme::menu_item(ui, Some(ico::OPEN), reveal, false).clicked() {
            commands.push(TabCommand::Reveal(i));
            ui.close();
        }
    });
}

/// Closes the documents `ids` (editor ids), skipping pinned ones when `keep_pinned`.
fn close_docs(app: &mut CrabCraftApp, ids: &[u64], keep_pinned: bool) {
    for id in ids {
        if let Some(j) = app.docs.list.iter().position(|d| d.editor.id == *id && !(keep_pinned && d.pinned)) {
            app.close_tab(j);
        }
    }
}

fn run_tab_commands(app: &mut CrabCraftApp, ctx: &egui::Context, commands: Vec<TabCommand>) {
    let id_at = |app: &CrabCraftApp, i: usize| app.docs.list.get(i).map(|d| d.editor.id);
    // The tabs of the group `i` is in, in their order.
    let group_tabs = |app: &CrabCraftApp, id: u64| app.layout.group_of(id).and_then(|g| app.layout.group(g)).map(|g| g.tabs.clone()).unwrap_or_default();
    for c in commands {
        if let TabCommand::Ratio { window, path, ratio } = &c {
            app.layout.set_ratio(*window, path, *ratio);
            app.session_dirty = true;
            continue;
        }
        if let TabCommand::Unsplit(g) | TabCommand::UnsplitAll(g) = c {
            if matches!(c, TabCommand::Unsplit(_)) {
                app.layout.unsplit(g);
            } else {
                app.layout.unsplit_all(g);
            }
            app.session_dirty = true;
            continue;
        }
        let i = match c {
            TabCommand::Close(i)
            | TabCommand::CloseOthers(i)
            | TabCommand::CloseAll(i)
            | TabCommand::CloseRight(i)
            | TabCommand::Pin(i)
            | TabCommand::CopyPath(i)
            | TabCommand::Reveal(i)
            | TabCommand::Locate(i)
            | TabCommand::Blame(i)
            | TabCommand::Diff(i)
            | TabCommand::LocalHistory(i)
            | TabCommand::Rename(i)
            | TabCommand::Split(i, _)
            | TabCommand::Float(i)
            | TabCommand::ToMain(i) => i,
            TabCommand::Unsplit(_) | TabCommand::UnsplitAll(_) | TabCommand::Ratio { .. } => continue,
        };
        let Some(id) = id_at(app, i) else { continue };
        match c {
            TabCommand::Close(i) => app.close_tab(i),
            TabCommand::CloseOthers(_) => {
                let others: Vec<u64> = group_tabs(app, id).into_iter().filter(|&t| t != id).collect();
                close_docs(app, &others, true);
            }
            TabCommand::CloseRight(_) => {
                let tabs = group_tabs(app, id);
                let right: Vec<u64> = tabs.iter().skip_while(|&&t| t != id).skip(1).copied().collect();
                close_docs(app, &right, false);
            }
            TabCommand::CloseAll(_) => {
                let tabs = group_tabs(app, id);
                close_docs(app, &tabs, true);
            }
            TabCommand::Pin(i) => {
                // Pinned tabs go first in their group.
                let d = &mut app.docs.list[i];
                d.pinned = !d.pinned;
                app.sync_layout();
                app.session_dirty = true;
            }
            TabCommand::Split(_, right) => {
                if let Some(group) = app.layout.group_of(id) {
                    move_doc(app, id, Drop::Split { group, edge: if right { Edge::Right } else { Edge::Bottom } });
                }
            }
            TabCommand::Float(_) => {
                let at = origin(ctx) + vec2(80.0, 80.0);
                move_doc(app, id, Drop::Window(at));
            }
            TabCommand::ToMain(_) => {
                let group = app.layout.main_target();
                move_doc(app, id, Drop::Group { group, index: None });
            }
            TabCommand::CopyPath(i) => ctx.copy_text(app.docs.list[i].path.display().to_string()),
            TabCommand::Rename(i) => {
                let path = app.docs.list[i].path.clone();
                let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                app.dialog = Some(crate::ui::dialogs::Dialog::Rename { path, name, error: None });
            }
            TabCommand::Reveal(i) => crate::app::reveal_in_file_manager(&app.docs.list[i].path),
            TabCommand::Blame(i) => {
                let (path, text) = (app.docs.list[i].path.clone(), app.docs.list[i].editor.buffer.rope().to_string());
                app.vcs.toggle_blame(&path, text, &mut app.docs);
            }
            TabCommand::Diff(i) => {
                let path = app.docs.list[i].path.clone();
                if let Some(rel) = app.vcs.repo.as_ref().and_then(|r| r.rel_path(&path)) {
                    app.save_all_docs();
                    app.vcs.open_diff(&rel);
                }
            }
            TabCommand::LocalHistory(i) => app.show_local_history(i),
            TabCommand::Locate(i) => {
                app.docs.activate(i);
                app.locate_active();
            }
            TabCommand::Unsplit(_) | TabCommand::UnsplitAll(_) | TabCommand::Ratio { .. } => {}
        }
    }
}

/// No open file: the most useful shortcuts, like IntelliJ's empty editor.
fn empty_state(app: &mut CrabCraftApp, ui: &mut Ui, rect: Rect) {
    let p = rc_kit::pal(ui);
    let lines: Vec<(&str, Option<String>)> = vec![
        (tr("Search Everywhere"), Some("⇧⇧".into())),
        (tr("Go to File"), app.keymap.shortcut_text(Action::GotoFile)),
        (tr("Recent Files"), app.keymap.shortcut_text(Action::RecentFiles)),
        (tr("Find in Files"), app.keymap.shortcut_text(Action::FindInFiles)),
        (tr("Explorer"), app.keymap.shortcut_text(Action::ToggleProjectWindow)),
    ];
    let row_h = 26.0;
    let top = rect.center().y - lines.len() as f32 * row_h / 2.0;
    for (i, (label, shortcut)) in lines.into_iter().enumerate() {
        let y = top + i as f32 * row_h;
        let label_g = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(13.0), p.text_muted);
        let sc = shortcut.unwrap_or_default();
        let sc_g = ui.painter().layout_no_wrap(sc, FontId::proportional(13.0), p.accent);
        let total = label_g.size().x + 12.0 + sc_g.size().x;
        let x = rect.center().x - total / 2.0;
        ui.painter().galley(pos2(x, y), label_g.clone(), p.text_muted);
        ui.painter().galley(pos2(x + label_g.size().x + 12.0, y), sc_g, p.accent);
    }
    ui.painter().text(pos2(rect.center().x, top + 5.0 * row_h + 16.0), Align2::CENTER_TOP, tr("Drop files here to open them"), FontId::proportional(13.0), p.disabled);
    let _ = ui.allocate_rect(rect, Sense::hover());
    // Files dropped on the window open in tabs.
    let dropped: Vec<std::path::PathBuf> = ui.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
    for path in dropped {
        if path.is_file() {
            app.open_file(&path);
        }
    }
}

/// Floating editor windows: each an OS window with its own groups of tabs. Closing one brings
/// its tabs back to the main window. Runs after the main window, and ends a tab drag.
pub fn floating_windows(app: &mut CrabCraftApp, ctx: &egui::Context) {
    let windows: Vec<(u64, Node, Option<Rect>)> = app.layout.windows.iter().map(|w| (w.id, w.root.clone(), w.initial)).collect();
    for (id, root, initial) in windows {
        let title = root
            .groups()
            .iter()
            .find(|g| g.id == app.layout.focused)
            .or(root.groups().first())
            .and_then(|g| g.active)
            .and_then(|a| app.docs.list.iter().find(|d| d.editor.id == a))
            .map(|d| format!("{}{} – CrabCraft", d.name(), if d.is_modified() { " *" } else { "" }))
            .unwrap_or_else(|| "CrabCraft".into());
        let mut builder = egui::ViewportBuilder::default().with_title(title).with_inner_size(initial.map_or(vec2(960.0, 720.0), |r| r.size()));
        if let Some(r) = initial {
            builder = builder.with_position(r.min);
        }
        let mut commands = Vec::new();
        let (closed, rect) = ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(("crabcraft_editor_window", id)), builder, |ui, _| {
            track_drag(app, ui.ctx());
            let p = rc_kit::pal(ui);
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, p.island_bg);
            show_node(app, ui, &root, rect.shrink(4.0), Some(id), &mut Vec::new(), &mut commands);
            mark_over_window(app, ui);
            let info = ui.input(|i| (i.viewport().close_requested(), i.viewport().outer_rect.zip(i.viewport().inner_rect)));
            (info.0, info.1.map(|(outer, inner)| Rect::from_min_size(outer.min, inner.size())))
        });
        run_tab_commands(app, ctx, commands);
        let Some(i) = app.layout.windows.iter().position(|w| w.id == id) else { continue };
        if closed {
            app.layout.dock_window(i);
            app.session_dirty = true;
        } else if rect.is_some() && app.layout.windows[i].rect != rect {
            app.layout.windows[i].rect = rect;
            app.session_dirty = true;
        }
    }
    finish_drag(app);
    app.editor_frame = None;
}
