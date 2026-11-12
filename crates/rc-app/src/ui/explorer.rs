//! Project explorer: folders and files read lazily from disk. Keyboard navigation: ↑/↓ move,
//! →/← expand/collapse, Enter or double-click opens, F2 renames, ⌫ deletes.

#[path="explorer_io.rs"]
mod explorer_io;
use rc_core::workspace::FileNode;
use std::sync::Arc;
use rc_kit::i18n::tr;
use rc_kit::{Ico, ico, icons, theme};
use egui::{Align2, FontId, Id, Key, Rect, Sense, Ui, pos2, vec2};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const ROW_H: f32 = 24.0;
const INDENT: f32 = 18.0;

/// What a node is; its key (`Node::key`) identifies it across frames and in the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Root,
    Dir(PathBuf),
    File(PathBuf),
    /// The node of the dependencies of a Cargo project.
    Libraries,
    Library(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Row {
    pub key: String,
    pub kind: NodeKind,
    pub depth: usize,
    pub label: String,
    pub secondary: String,
    pub icon: Ico,
    pub expandable: bool,
}

#[derive(Default)]
pub struct ExplorerState {
    /// The project is in a Git repository (its files have a history to show).
    io:explorer_io::Io,
    pub git: bool,
    pub new_item_filter: String,
    pub expanded: HashSet<String>,
    pub selected: Option<String>,
    pub dir_cache: HashMap<PathBuf, Arc<[FileNode]>>,
    /// The dependencies shown under External Libraries; their files are not the user's to change.
    pub libraries: Vec<rc_cargo::Library>,
    /// Scroll the selected row into view on the next frame.
    pub reveal: bool,
    pub has_focus: bool,
    /// Open files with a single click (Settings › Editor).
    pub single_click: bool,
    /// Folders and files the user excluded from the index, Find in Files and the analysis
    /// (root-relative, `/`-separated, sorted). Saved in the session.
    pub excluded: Vec<String>,
    /// `Cargo.toml` files of other Cargo projects analysed with the workspace (absolute).
    pub attached: Vec<PathBuf>,
    /// The project's scopes and file colours (saved in the session).
    pub scopes: crate::scopes::Scopes,
}

/// What the user asked for in the explorer this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum ExplorerAction {
    Open(PathBuf),
    /// Single-click open: show the file but keep the focus in the explorer.
    Preview(PathBuf),
    /// Add › item in this folder.
    NewItem(PathBuf, crate::item_templates::ItemKind),
    NewCrate(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Reveal(PathBuf),
    /// Git › Show History of a file or folder (V-06).
    ShowHistory(PathBuf),
    CopyPath(PathBuf),
    OpenInTerminal(PathBuf),
    /// Exclude the folder or file from the index, Find in Files and the analysis (`true`), or
    /// include it again (`false`).
    SetExcluded(PathBuf, bool),
    /// Attaches (or detaches) the Cargo project of a `Cargo.toml` outside the workspace.
    SetAttached(PathBuf, bool),
    Collapse,
    LocateActive,
}

/// How a folder or file stands with the user's exclusions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exclusion {
    /// Not excluded (or outside the workspace root).
    No,
    /// Excluded by the user.
    Itself,
    /// Inside an excluded folder.
    Inherited,
}

impl ExplorerState {
    /// Whether `path` was excluded, itself or through a folder above it.
    pub fn exclusion(&self, root: &Path, path: &Path) -> Exclusion {
        if self.excluded.is_empty() {
            return Exclusion::No;
        }
        let Some(rel) = rc_core::workspace::relative_path(root, path) else { return Exclusion::No };
        if self.excluded.contains(&rel) {
            Exclusion::Itself
        } else if rc_core::workspace::is_excluded(&rel, &self.excluded) {
            Exclusion::Inherited
        } else {
            Exclusion::No
        }
    }

    pub fn invalidate(&mut self, path: &Path) {
        if let Some(parent) = path.parent() {
            self.dir_cache.remove(parent);self.io.invalidate(parent);
        }
        self.dir_cache.remove(path);self.io.invalidate(path);
    }

    fn poll_io(&mut self, root: &Path) {
        if self.io.root(root) { self.dir_cache.clear(); }
        for (key, value) in self.io.drain() {
            let entries = self.dir_cache.values().map(|rows| rows.len()).sum::<usize>();
            let bytes = self.dir_cache.values().flat_map(|rows| rows.iter()).map(|node| node.path.as_os_str().len() + node.name.len() + std::mem::size_of::<FileNode>()).sum::<usize>();
            let explorer_io::Value::Directory(nodes) = value;
            let added = nodes.iter().map(|node| node.path.as_os_str().len() + node.name.len() + std::mem::size_of::<FileNode>()).sum::<usize>();
            if entries.saturating_add(nodes.len()) > 50_000 || self.dir_cache.len() >= 256 || bytes.saturating_add(added) > 64 * 1024 * 1024 {
                self.io.record_error(key, explorer_io::Error::Limit);
                continue;
            }
            let explorer_io::Key::Directory(path) = key;
            self.dir_cache.insert(path, Arc::from(nodes));
        }
    }
    fn children_of_dir(&mut self,dir:&Path)->Arc<[FileNode]>{if let Some(nodes)=self.dir_cache.get(dir){return nodes.clone();}self.io.request(explorer_io::Key::Directory(dir.into()));Arc::from([])}
    /// Expands the parents of `path` and selects it (Locate in explorer / "Always select
    /// opened file").
    pub fn locate(&mut self, path: &Path, root: &Path) {
        let mut keys = vec!["sln".to_string()];
        let mut d = path.parent().map(Path::to_path_buf);
        let mut dirs = Vec::new();
        while let Some(cur) = d {
            if !cur.starts_with(root) || cur == root { break; }
            dirs.push(format!("dir:{}", cur.display()));
            d = cur.parent().map(Path::to_path_buf);
        }
        keys.extend(dirs.into_iter().rev());
        self.expanded.extend(keys);
        self.selected = Some(format!("file:{}", path.display()));
        self.reveal = true;
    }

    /// The visible rows: a depth-first walk of the expanded nodes.
    pub fn rows(&mut self, root: &Path, root_name: &str) -> Vec<Row> {
        self.poll_io(root);
        let mut rows = Vec::new();
        let root_row = Row {
            key: "sln".into(),
            kind: NodeKind::Root,
            depth: 0,
            label: root_name.to_string(),
            secondary: String::new(),
            icon: ico::FOLDER,
            expandable: true,
        };
        rows.push(root_row);
        if !self.expanded.contains("sln") {
            return rows;
        }
        self.push_dir(&mut rows, root, 1);
        self.push_libraries(&mut rows);
        rows
    }

    fn push_libraries(&mut self, rows: &mut Vec<Row>) {
        if self.libraries.is_empty() {
            return;
        }
        let key = "libraries";
        rows.push(Row { key: key.into(), kind: NodeKind::Libraries, depth: 1, label: tr("External Libraries").into(), secondary: String::new(), icon: ico::PACKAGE, expandable: true });
        if !self.expanded.contains(key) {
            return;
        }
        for library in self.libraries.clone() {
            if rows.len()>=20_000{break;}
            let key = format!("dir:{}", library.root.display());
            rows.push(Row { key: key.clone(), kind: NodeKind::Library(library.root.clone()), depth: 2, label: library.name, secondary: library.version, icon: ico::PACKAGE, expandable: true });
            if self.expanded.contains(&key) {
                self.push_dir(rows, &library.root, 3);
            }
        }
    }

    /// Whether `path` belongs to a dependency that Cargo downloaded.
    pub fn in_library(&self, path: &Path) -> bool {
        self.libraries.iter().any(|l| l.read_only && path.starts_with(&l.root))
    }

    fn push_dir(&mut self, rows: &mut Vec<Row>, dir: &Path, depth: usize) {
        if depth>64||rows.len()>=20_000{self.io.record_error(explorer_io::Key::Directory(dir.into()),explorer_io::Error::Limit);return;}
        let children = self.children_of_dir(dir);
        // From the cached listing: no file system access while drawing.
        let cargo = has_cargo_toml(&children);
        for c in children.iter() {
            if rows.len()>=20_000{break;}
            self.push_node(rows, c, depth, cargo);
        }
    }

    /// `in_cargo_package`: the node's folder has a `Cargo.toml` (picks the source/test root icons).
    fn push_node(&mut self, rows: &mut Vec<Row>, node: &FileNode, depth: usize, in_cargo_package: bool) {
        if node.is_dir {
            let key = format!("dir:{}", node.path.display());
            rows.push(Row {
                key: key.clone(),
                kind: NodeKind::Dir(node.path.clone()),
                depth,
                label: node.name.clone(),
                secondary: String::new(),
                icon: icons::for_dir(&node.name, in_cargo_package),
                expandable: true,
            });
            if self.expanded.contains(&key) {
                self.push_dir(rows, &node.path, depth + 1);
            }
        } else {
            rows.push(file_row(&node.path, depth));
        }
    }
}

fn explorer_error(error:explorer_io::Error)->String{tr(match error{explorer_io::Error::Io(_)=>"Could not read project files",explorer_io::Error::Limit=>"Project tree input or cache limit exceeded",explorer_io::Error::Cancelled=>"Project tree read cancelled",explorer_io::Error::Changed=>"Project tree changed while reading; refresh it",explorer_io::Error::Busy=>"Project tree worker queue is full; refresh to retry"}).into()}

fn has_cargo_toml(children: &[FileNode]) -> bool {
    children.iter().any(|c| !c.is_dir && c.name == "Cargo.toml")
}

fn file_row(path: &Path, depth: usize) -> Row {
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    Row { key: format!("file:{}", path.display()), kind: NodeKind::File(path.to_path_buf()), depth, icon: icons::for_file(&name), label: name, secondary: String::new(), expandable: false }
}

/// "net8.0" → ".NET 8.0".
fn accessibility_ancestor(
    ui: &mut Ui,
    rows: &[Row],
    parents: &[Option<usize>],
    index: usize,
    tree: Id,
    visible: &std::ops::Range<usize>,
    created: &mut HashSet<usize>,
    depth: usize,
) -> Id {
    if depth >= 64 {
        return tree;
    }
    let id = tree.with(&rows[index].key);
    if visible.contains(&index) || created.contains(&index) {
        return id;
    }
    let parent = parents[index]
        .map(|parent| {
            accessibility_ancestor(ui, rows, parents, parent, tree, visible, created, depth + 1)
        })
        .unwrap_or(tree);
    let rect = Rect::from_min_size(ui.max_rect().min, egui::Vec2::ZERO);
    let ancestor = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("explorer_accessible_ancestor", &rows[index].key))
            .accessibility_parent(parent)
            .max_rect(rect),
    );
    ancestor.interact(rect, id, Sense::hover());
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::TreeItem);
        node.set_label(rows[index].label.as_str());
        node.set_level(rows[index].depth.saturating_add(1));
        if rows[index].expandable {
            node.set_expanded(true);
        }
    });
    created.insert(index);
    id
}

/// Draws the explorer inside its island; returns the user's requests.
pub fn show(ui: &mut Ui, state: &mut ExplorerState, root: &Path, root_name: &str, active_file: Option<&Path>) -> Vec<ExplorerAction> {
    let p = rc_kit::pal(ui);
    let mut actions = Vec::new();
    state.io.attach(ui.ctx());
    let rows = state.rows(root, root_name);
    // Header: title and actions (New UI tool window header).
    let header = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 36.0));
    ui.allocate_rect(header, Sense::hover());
    ui.painter().text(pos2(header.left() + 12.0, header.center().y), Align2::LEFT_CENTER, tr("Explorer"), rc_kit::theme::bold(13.0), p.text);
    let mut x = header.right() - 8.0;
    for (icon, tip, action) in [
        (ico::HIDE, tr("Hide"), None),
        (ico::COLLAPSE_ALL, tr("Collapse All"), Some(ExplorerAction::Collapse)),
        (ico::LOCATE, tr("Select Opened File"), Some(ExplorerAction::LocateActive)),
    ] {
        x -= 26.0;
        let r = Rect::from_min_size(pos2(x, header.center().y - 13.0), vec2(26.0, 26.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        let enabled = !(action == Some(ExplorerAction::LocateActive) && active_file.is_none());
        if rc_kit::theme::icon_button(&mut child, icon, enabled, tip).clicked() {
            match action {
                Some(a) => actions.push(a),
                None => ui.ctx().data_mut(|d| {
                    d.insert_temp(Id::new("explorer_hide"), true);
                }),
            }
        }
    }

    x -= 26.0;
    let add_rect = Rect::from_min_size(pos2(x, header.center().y - 13.0), vec2(26.0, 26.0));
    let mut add_ui = ui.new_child(egui::UiBuilder::new().max_rect(add_rect));
    let add = theme::icon_button(&mut add_ui, ico::ADD, true, tr("New File…"));
    let selected = state.selected.as_ref().and_then(|key| rows.iter().find(|row| &row.key == key));
    let target = match selected.map(|row| &row.kind) {
        Some(NodeKind::Dir(path)) => path.clone(),
        Some(NodeKind::File(path)) => path.parent().unwrap_or(root).to_path_buf(),
        _ => root.to_path_buf(),
    };
    let popup = egui::Popup::menu(&add);
    if !popup.is_open() { state.new_item_filter.clear(); }
    popup.show(|ui| new_item_menu(ui, &target, &mut state.new_item_filter, &mut actions));

    let refresh=ui.small_button(tr("Refresh")).clicked();
    if state.io.busy(){ui.horizontal(|ui|{ui.spinner();ui.label(tr("Loading project files…"));if ui.button(tr("Cancel")).clicked(){state.io.cancel();}});}
    for(key,error)in state.io.errors.iter().take(3){ui.small(rc_kit::i18n::trf("Explorer read failed for {path}: {error}",&[("path",&key.path().display()),("error",&explorer_error(*error))]));}
    if refresh{state.io.reset();state.dir_cache.clear();ui.ctx().request_repaint();}
    let attached = state.attached.clone();
    let selected_index = state.selected.as_ref().and_then(|k| rows.iter().position(|r| &r.key == k));
    let tree_id = Id::new("explorer_tree");
    let mut open_row: Option<usize> = None;
    let mut toggle_row: Option<usize> = None;
    let mut select_row: Option<usize> = None;
    let mut context: Option<(usize, egui::Response)> = None;
    // The tree takes the keyboard focus, so it is a widget of its own (accessibility needs a
    // node for the focused id).
    ui.interact(ui.max_rect(), tree_id, Sense::focusable_noninteractive()).widget_info(|| egui::WidgetInfo::new(egui::WidgetType::Other));

    let accessibility=ui.ctx().accesskit_node_builder(tree_id,|node|{node.set_role(egui::accesskit::Role::Tree);node.set_label(tr("Project files"));node.set_description(tr("Accessible project hierarchy is limited to visible items and 64 ancestor levels"));node.add_action(egui::accesskit::Action::Focus);}).is_some();
    ui.scope_builder(egui::UiBuilder::new().accessibility_parent(tree_id),|ui| {
    let mut scroll = egui::ScrollArea::both().id_salt("explorer_scroll").auto_shrink([false, false]);
    if state.reveal {if let Some(index)=selected_index {let offset=egui::scroll_area::State::load(ui.ctx(),ui.make_persistent_id(egui::IdSalt::new("explorer_scroll"))).map_or(0.0,|state|state.offset.y);let height=ui.available_height().max(ROW_H);let top=index as f32*ROW_H;if top<offset||top+ROW_H>offset+height{scroll=scroll.vertical_scroll_offset((top-(height-ROW_H)/2.0).max(0.0));}}}
    scroll.show_rows(ui, ROW_H, rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut accessible_ancestors=HashSet::new();
        let mut parents=Vec::new();if accessibility {let mut ancestors:Vec<usize>=Vec::new();for(index,row)in rows.iter().take(range.end).enumerate(){while ancestors.last().is_some_and(|ancestor|rows[*ancestor].depth>=row.depth){ancestors.pop();}parents.push(ancestors.last().copied());ancestors.push(index);}}
        let visible_rows=range.clone();
        for i in range {
            let row = &rows[i];
            let width = ui.available_width().max(200.0);
            let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
            let resp=if accessibility {let parent=parents[i].map(|parent|accessibility_ancestor(ui,&rows,&parents,parent,tree_id,&visible_rows,&mut accessible_ancestors,0)).unwrap_or(tree_id);let row_ui=ui.new_child(egui::UiBuilder::new().id_salt(("explorer_accessible_row",&row.key)).accessibility_parent(parent).max_rect(rect));row_ui.interact(rect,tree_id.with(&row.key),Sense::click())}else{ui.interact(rect,tree_id.with(&row.key),Sense::click())};
            let accessibility_actions:Vec<_>=ui.input(|input|input.events.iter().filter_map(|event|match event{egui::Event::AccessKitActionRequest(request) if request.target_tree==egui::accesskit::TreeId::ROOT&&request.target_node==resp.id.accesskit_id()=>Some(request.action),_=>None}).collect());
            let accessibility_click=accessibility_actions.contains(&egui::accesskit::Action::Click);
            if accessibility_actions.iter().any(|action|matches!(action,egui::accesskit::Action::Focus|egui::accesskit::Action::Click|egui::accesskit::Action::Expand|egui::accesskit::Action::Collapse)){select_row=Some(i);state.reveal=true;ui.memory_mut(|memory|memory.request_focus(tree_id));}
            if accessibility_click{open_row=Some(i);}
            if row.expandable {let expanded=state.expanded.contains(&row.key);if accessibility_actions.iter().any(|action|matches!(action,egui::accesskit::Action::Expand) && !expanded ||matches!(action,egui::accesskit::Action::Collapse)&&expanded){toggle_row=Some(i);}}

            let selected = selected_index == Some(i);
            // File colours: a light tint behind the files of a coloured scope.
            if let NodeKind::File(path) = &row.kind
                && let Some(c) = state.scopes.color_of(path)
            {
                ui.painter().rect_filled(rect.shrink2(vec2(6.0, 0.0)), 4.0, c.gamma_multiply(if p.dark { 0.3 } else { 0.18 }));
            }
            if selected {
                let color = if state.has_focus { p.selection } else { p.selection_inactive };
                ui.painter().rect_filled(rect.shrink2(vec2(6.0, 0.0)), 4.0, color);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect.shrink2(vec2(6.0, 0.0)), 4.0, p.row_hover);
            }
            let x0 = rect.left() + 10.0 + row.depth as f32 * INDENT;
            let cy = rect.center().y;
            if row.expandable {
                let chevron = if state.expanded.contains(&row.key) { ico::CHEVRON_DOWN } else { ico::CHEVRON_RIGHT };
                let cr = Rect::from_center_size(pos2(x0 + 8.0, cy), vec2(16.0, 16.0));
                icons::paint(ui.painter(), cr, chevron, &p, true);
                if resp.clicked() && !accessibility_click && ui.input(|i| i.pointer.interact_pos()).is_some_and(|pt| pt.x < x0 + 18.0) {
                    toggle_row = Some(i);
                }
            }
            // Excluded folders and files are dimmed; the excluded one itself says so.
            let exclusion = match &row.kind {
                NodeKind::Dir(path) | NodeKind::File(path) => state.exclusion(root, path),
                _ => Exclusion::No,
            };
            let ir = Rect::from_center_size(pos2(x0 + 26.0, cy), vec2(16.0, 16.0));
            icons::paint(ui.painter(), ir, row.icon, &p, exclusion == Exclusion::No);
            let text_color = if exclusion == Exclusion::No { p.text } else { p.muted };
            let label = ui.painter().layout_no_wrap(row.label.clone(), FontId::proportional(13.0), text_color);
            let lx = x0 + 40.0;
            ui.painter().galley(pos2(lx, (cy - label.size().y / 2.0).round()), label.clone(), text_color);
            let secondary = if exclusion == Exclusion::Itself { tr("excluded").to_string() } else { row.secondary.clone() };
            if !secondary.is_empty() {
                let sec = ui.painter().layout_no_wrap(secondary, FontId::proportional(12.0), p.muted);
                ui.painter().galley(pos2(lx + label.size().x + 8.0, (cy - sec.size().y / 2.0).round()), sec, p.muted);
            }
            if selected && state.reveal {
                ui.scroll_to_rect(rect, Some(egui::Align::Center));
                state.reveal = false;
            }
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &row.label));
            ui.ctx().accesskit_node_builder(resp.id,|node|{node.set_role(egui::accesskit::Role::TreeItem);node.set_label(row.label.as_str());node.set_level(row.depth.saturating_add(1));node.set_selected(selected);node.add_action(egui::accesskit::Action::Focus);node.add_action(egui::accesskit::Action::Click);if row.expandable{let expanded=state.expanded.contains(&row.key);node.set_expanded(expanded);node.add_action(if expanded{egui::accesskit::Action::Collapse}else{egui::accesskit::Action::Expand});}});
            if selected {ui.ctx().accesskit_node_builder(tree_id,|node|node.set_active_descendant(resp.id.accesskit_id()));}

            if resp.clicked() || resp.secondary_clicked() {
                select_row = Some(i);
                ui.memory_mut(|m| m.request_focus(tree_id));
            }
            if resp.double_clicked() {
                open_row = Some(i);
            } else if resp.clicked()
                && state.single_click && !accessibility_click
                && let NodeKind::File(p) = &row.kind
            {
                actions.push(ExplorerAction::Preview(p.clone()));
            }
            if resp.secondary_clicked() {
                context = Some((i, resp.clone()));
            }
            let in_library = row_path(&rows[i]).is_some_and(|p| state.in_library(&p)) || matches!(rows[i].kind, NodeKind::Libraries | NodeKind::Library(_));
            if !in_library {
                resp.context_menu(|ui| {
                    let rust=state.dir_cache.get(root).is_some_and(|nodes|has_cargo_toml(nodes));
                    let git = state.git;
                    context_menu(ui, &rows[i], root, rust, git, exclusion, &attached, &mut actions);
                });
            }
        }
        // Empty space below the rows clears focus to the tree too.
        let rest = ui.available_rect_before_wrap();
        if rest.height() > 0.0 {
            let r = ui.allocate_rect(rest, Sense::click());
            if r.clicked() {
                ui.memory_mut(|m| m.request_focus(tree_id));
            }
        }
    });
    });
    let _ = context;

    // Keyboard.
    let has_focus = ui.memory(|m| m.has_focus(tree_id));
    state.has_focus = has_focus;
    if has_focus {
        ui.memory_mut(|m| m.set_focus_lock_filter(tree_id, egui::EventFilter { tab: false, horizontal_arrows: true, vertical_arrows: true, escape: false }));
        let cur = selected_index.unwrap_or(0);
        let pressed = |k: Key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
        if pressed(Key::ArrowDown) {
            select_row = Some((cur + 1).min(rows.len().saturating_sub(1)));
            state.reveal = true;
        }
        if pressed(Key::ArrowUp) {
            select_row = Some(cur.saturating_sub(1));
            state.reveal = true;
        }
        if pressed(Key::ArrowRight)
            && let Some(r) = rows.get(cur) {
                if r.expandable && !state.expanded.contains(&r.key) {
                    toggle_row = Some(cur);
                } else if r.expandable {
                    select_row = Some((cur + 1).min(rows.len() - 1));
                }
            }
        if pressed(Key::ArrowLeft)
            && let Some(r) = rows.get(cur) {
                if r.expandable && state.expanded.contains(&r.key) {
                    toggle_row = Some(cur);
                } else if let Some(parent) = (0..cur).rev().find(|&j| rows[j].depth < r.depth) {
                    select_row = Some(parent);
                    state.reveal = true;
                }
            }
        if pressed(Key::Enter) {
            open_row = Some(cur);
        }
        if pressed(Key::F2)
            && let Some(path) = rows.get(cur).and_then(row_path)
        {
            actions.push(ExplorerAction::Rename(path));
        }
        if (pressed(Key::Delete) || ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Backspace)))
            && let Some(path) = rows.get(cur).and_then(row_path)
        {
            actions.push(ExplorerAction::Delete(path));
        }
    }

    if let Some(i) = select_row {
        state.selected = rows.get(i).map(|r| r.key.clone());
    }
    if let Some(i) = toggle_row {
        let key = rows[i].key.clone();
        if !state.expanded.remove(&key) {
            state.expanded.insert(key);
        }
    }
    actions.retain(|a| !matches!(a, ExplorerAction::Rename(p) | ExplorerAction::Delete(p) if state.in_library(p)));
    if let Some(i) = open_row {
        match &rows[i].kind {
            NodeKind::File(p) => actions.push(ExplorerAction::Open(p.clone())),
            _ if rows[i].expandable => {
                let key = rows[i].key.clone();
                if !state.expanded.remove(&key) {
                    state.expanded.insert(key);
                }
            }
            _ => {}
        }
    }
    actions
}

/// File system path of a row (for file actions).
fn row_path(r: &Row) -> Option<PathBuf> {
    match &r.kind {
        NodeKind::File(p) | NodeKind::Dir(p) => Some(p.clone()),
        _ => None,
    }
}

fn context_menu(ui: &mut Ui, row: &Row, root: &Path, rust: bool, git: bool, exclusion: Exclusion, state_attached: &[PathBuf], actions: &mut Vec<ExplorerAction>) {
    ui.set_min_width(220.0);
    ui.set_max_width(280.0);
    let dir_of = |p: &Path, is_dir: bool| if is_dir { p.to_path_buf() } else { p.parent().unwrap_or(p).to_path_buf() };
    let (path, is_dir) = match &row.kind {
        NodeKind::File(p) => (p.clone(), false),
        NodeKind::Dir(p) => (p.clone(), true),
        // The folder of the open project; it cannot be renamed or deleted from inside.
        NodeKind::Root => (root.to_path_buf(), true),
        _ => {
            ui.label(egui::RichText::new(tr("No actions for this item")).color(rc_kit::pal(ui).muted));
            return;
        }
    };
    let target_dir = match &row.kind {
        _ => dir_of(&path, is_dir),
    };
    if let NodeKind::File(p) = &row.kind
        && theme::menu_item(ui, Some(ico::OPEN), tr("Open"), false).clicked()
    {
        actions.push(ExplorerAction::Open(p.clone()));
        ui.close();
    }
    ui.menu_button(tr("Add"), |ui| {
        let id = ui.id().with("new_item_filter");
        let mut filter = ui.ctx().data(|data| data.get_temp::<String>(id).unwrap_or_default());
        new_item_menu(ui, &target_dir, &mut filter, actions);
        ui.ctx().data_mut(|data| data.insert_temp(id, filter));
    });
    if matches!(row.kind, NodeKind::Root) {
        return;
    }
    ui.separator();
    if theme::menu_item(ui, Some(ico::EDIT), tr("Rename…"), false).clicked() {
        actions.push(ExplorerAction::Rename(path.clone()));
        ui.close();
    }
    if theme::menu_item(ui, Some(ico::DELETE), tr("Delete…"), false).clicked() {
        actions.push(ExplorerAction::Delete(path.clone()));
        ui.close();
    }
    ui.separator();
    if theme::menu_item(ui, Some(ico::COPY), tr("Copy Path"), false).clicked() {
        actions.push(ExplorerAction::CopyPath(path.clone()));
        ui.close();
    }
    let reveal = if cfg!(target_os = "macos") { tr("Reveal in Finder") } else { tr("Open in Explorer") };
    if theme::menu_item(ui, Some(ico::OPEN), reveal, false).clicked() {
        actions.push(ExplorerAction::Reveal(path.clone()));
        ui.close();
    }
    if theme::menu_item(ui, Some(ico::FILE_SHELL), tr("Open in Terminal"), false).clicked() {
        actions.push(ExplorerAction::OpenInTerminal(target_dir));
        ui.close();
    }
    if git {
        if theme::menu_item(ui, Some(ico::HISTORY), tr("Show Git History"), false).clicked() {
            actions.push(ExplorerAction::ShowHistory(path.clone()));
            ui.close();
        }
    }
    // A Cargo.toml other than the workspace's: its project can be analysed with it.
    if matches!(row.kind, NodeKind::File(_)) && path.file_name().is_some_and(|n| n == "Cargo.toml") && path.parent() != Some(root) {
        ui.separator();
        let attached = state_attached.contains(&path);
        let (label, on) = if attached { (tr("Detach Cargo Project"), false) } else { (tr("Attach Cargo Project"), true) };
        if theme::menu_item(ui, Some(ico::CARGO_TOML), label, false).on_hover_text(tr("Analyse this Cargo project with the workspace, though it is not a member")).clicked() {
            actions.push(ExplorerAction::SetAttached(path.clone(), on));
            ui.close();
        }
    }
    // Exclusion: folders and files inside the workspace (an excluded folder's contents follow it).
    if matches!(row.kind, NodeKind::Dir(_) | NodeKind::File(_)) && rc_core::workspace::relative_path(root, &path).is_some() {
        ui.separator();
        match exclusion {
            Exclusion::No => {
                if theme::menu_item(ui, Some(ico::HIDE), tr("Exclude from Analysis"), false).clicked() {
                    actions.push(ExplorerAction::SetExcluded(path, true));
                    ui.close();
                }
            }
            Exclusion::Itself => {
                if theme::menu_item(ui, Some(ico::HIDE), tr("Include in Analysis Again"), false).clicked() {
                    actions.push(ExplorerAction::SetExcluded(path, false));
                    ui.close();
                }
            }
            Exclusion::Inherited => {
                ui.label(egui::RichText::new(tr("Excluded with its folder")).color(rc_kit::pal(ui).muted));
            }
        }
    }
}

/// Existing templates only: the same creation flow handles the header and context menu.
fn new_item_menu(ui: &mut Ui, target: &Path, filter: &mut String, actions: &mut Vec<ExplorerAction>) {
    use crate::item_templates::ItemKind;
    ui.set_min_width(260.0);
    ui.set_max_width(320.0);
    let pass = ui.ctx().cumulative_pass_nr();
    let focus_id = ui.id().with("new_item_search_opened");
    let opened = ui.ctx().data_mut(|data| {
        let (last, opened) = data.get_temp::<(u64, u64)>(focus_id).unwrap_or((pass, pass));
        let opened = if last.saturating_add(1) < pass { pass } else { opened };
        data.insert_temp(focus_id, (pass, opened));
        opened
    });
    let field = ui.add(egui::TextEdit::singleline(filter).hint_text(tr("Search file type")).desired_width(260.0));
    if pass <= opened.saturating_add(1) { field.request_focus(); }
    let enter = (field.has_focus() || field.lost_focus()) && ui.input(|input| input.key_pressed(Key::Enter));
    ui.separator();
    let query = filter.trim().to_lowercase();
    let mut first = None;
    let mut count = 0;
    let height = (ui.ctx().content_rect().bottom() - ui.cursor().top() - 16.0).clamp(80.0, 360.0);
    egui::ScrollArea::vertical().max_height(height).show(ui, |ui| {
        for entry in ItemKind::RUST_MENU {
            if let Some(kind) = entry {
                let title = tr(kind.title());
                if !title.to_lowercase().contains(&query) { continue; }
                let action = ExplorerAction::NewItem(target.to_path_buf(), *kind);
                if first.is_none() { first = Some(action.clone()); }
                count += 1;
                if theme::menu_item(ui, Some(kind.icon()), title, false).clicked() {
                    actions.push(action);
                    ui.close();
                }
            } else if query.is_empty() { ui.separator(); }
        }
        if tr("Cargo Crate").to_lowercase().contains(&query) {
            ui.separator();
            let action = ExplorerAction::NewCrate(target.to_path_buf());
            if first.is_none() { first = Some(action.clone()); }
            count += 1;
            if theme::menu_item(ui, Some(ico::LIBRARY), tr("Cargo Crate"), false).clicked() {
                actions.push(action);
                ui.close();
            }
        }
        if count == 0 { ui.label(tr("No file type matches the filter")); }
    });
    if enter && let Some(action) = first {
        actions.push(action);
        ui.close();
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    /// The context menu of a folder, as it would open for `exclusion`; returns what it asked for.
    fn click_in_menu(exclusion: Exclusion, label: &str, absent: &str) -> Vec<ExplorerAction> {
        let root = Path::new("/ws");
        let row = Row { key: "dir:/ws/gen".into(), kind: NodeKind::Dir(root.join("gen")), depth: 1, label: "gen".into(), secondary: String::new(), icon: ico::FOLDER, expandable: true };
        let mut h = Harness::new_ui_state(move |ui, actions: &mut Vec<ExplorerAction>| context_menu(ui, &row, root, true, false, exclusion, &[], actions), Vec::new());
        h.run();
        assert!(h.query_by_label(absent).is_none(), "{absent} is offered");
        h.get_by_label(label).click();
        h.run();
        std::mem::take(h.state_mut())
    }

    #[test]
    fn the_context_menu_excludes_a_folder_and_includes_it_again() {
        let gen_dir = Path::new("/ws").join("gen");
        assert_eq!(click_in_menu(Exclusion::No, "Exclude from Analysis", "Include in Analysis Again"), [ExplorerAction::SetExcluded(gen_dir.clone(), true)]);
        assert_eq!(click_in_menu(Exclusion::Itself, "Include in Analysis Again", "Exclude from Analysis"), [ExplorerAction::SetExcluded(gen_dir, false)]);
    }

    #[test]
    fn rows_know_whether_they_are_excluded() {
        let root = Path::new("/ws");
        let state = ExplorerState { excluded: vec!["gen".into(), "src/old.rs".into()], ..Default::default() };
        assert_eq!(state.exclusion(root, &root.join("gen")), Exclusion::Itself);
        assert_eq!(state.exclusion(root, &root.join("gen/a/b.rs")), Exclusion::Inherited);
        assert_eq!(state.exclusion(root, &root.join("src/old.rs")), Exclusion::Itself);
        assert_eq!(state.exclusion(root, &root.join("src/new.rs")), Exclusion::No);
        assert_eq!(state.exclusion(root, root), Exclusion::No);
        assert_eq!(state.exclusion(root, Path::new("/elsewhere/gen")), Exclusion::No);
    }
}

#[cfg(test)]
mod icon_tests {
    use super::*;

    #[test]
    fn cargo_packages_show_source_and_test_root_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for d in ["src", "tests", "docs/src"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["Cargo.toml", "Cargo.lock", "src/main.rs", "tests/it.rs", "docs/src/a.md"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        let mut state = ExplorerState::default();
        state.expanded.extend(["sln".to_string(), format!("dir:{}", root.join("docs").display())]);
        let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
        let rows=loop{let rows=state.rows(root,"app");if !state.io.busy()&&state.dir_cache.contains_key(root)&&state.dir_cache.contains_key(&root.join("docs")){break rows;}assert!(std::time::Instant::now()<until);std::thread::sleep(std::time::Duration::from_millis(5));};
        let icon = |label: &str, depth: usize| rows.iter().find(|r| r.label == label && r.depth == depth).map(|r| r.icon);
        assert_eq!(icon("src", 1), Some(ico::SOURCE_ROOT));
        assert_eq!(icon("tests", 1), Some(ico::TEST_ROOT));
        assert_eq!(icon("docs", 1), Some(ico::FOLDER));
        // No Cargo.toml next to it: an ordinary folder.
        assert_eq!(icon("src", 2), Some(ico::FOLDER));
        assert_eq!(icon("Cargo.toml", 1), Some(ico::CARGO_TOML));
        assert_eq!(icon("Cargo.lock", 1), Some(ico::CARGO_LOCK));
    }
}

#[cfg(test)]mod asynchronous_tests {
use egui_kittest::Harness;
 use super::*;
 fn fixture()->tempfile::TempDir{let root=tempfile::tempdir().unwrap();std::fs::create_dir(root.path().join("src")).unwrap();std::fs::write(root.path().join("Cargo.toml"),"[package]\nname=\"explorer_async_evidence\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();std::fs::write(root.path().join("src/main.rs"),"fn main(){println!(\"Rust tree\");}\n").unwrap();std::fs::write(root.path().join("build.rs"),"fn main(){std::fs::write(\"executed-marker\",\"bad\").unwrap();}").unwrap();root}
 fn wait(state:&mut ExplorerState,root:&Path){let until=std::time::Instant::now()+std::time::Duration::from_secs(5);loop{state.rows(root,"Rust");if !state.io.busy(){return;}assert!(std::time::Instant::now()<until);std::thread::sleep(std::time::Duration::from_millis(5));}}
 #[test]fn real_directory_cancel_invalidation_and_workspace_switch_reject_old_replies(){let first=fixture();let second=fixture();let mut state=ExplorerState::default();state.expanded.insert("sln".into());assert_eq!(state.rows(first.path(),"Rust").len(),1);assert!(state.dir_cache.is_empty());assert!(state.io.busy());state.io.cancel();wait(&mut state,first.path());assert!(state.dir_cache.is_empty());assert_eq!(state.io.error(&explorer_io::Key::Directory(first.path().into())),Some(explorer_io::Error::Cancelled));state.io.reset();state.rows(first.path(),"Rust");std::fs::write(first.path().join("fresh.rs"),"pub const N:usize=42;").unwrap();state.invalidate(&first.path().join("fresh.rs"));wait(&mut state,first.path());assert!(state.dir_cache[first.path()].iter().any(|node|node.name=="fresh.rs"));assert_eq!(state.rows(second.path(),"Other").len(),1);assert!(state.dir_cache.is_empty());wait(&mut state,second.path());assert!(!state.dir_cache.contains_key(first.path()));assert!(!first.path().join("executed-marker").exists());assert!(!second.path().join("executed-marker").exists());}
 #[test]fn native_tree_waits_for_real_worker_and_refreshes_without_running_cargo(){use egui_kittest::kittest::Queryable;let root=fixture();let path=root.path().to_path_buf();let mut state=ExplorerState::default();state.expanded.insert("sln".into());let mut first=true;let mut h=Harness::new_ui_state(move|ui,state:&mut ExplorerState|{if std::mem::take(&mut first){rc_kit::theme::install_fonts(ui.ctx());return;}let _=show(ui,state,&path,"Rust",None);},state);let until=std::time::Instant::now()+std::time::Duration::from_secs(5);while !h.state().dir_cache.contains_key(root.path()){h.run_steps(2);assert!(std::time::Instant::now()<until);std::thread::sleep(std::time::Duration::from_millis(5));}h.get_by_role_and_label(egui::accesskit::Role::TreeItem,"src");std::fs::write(root.path().join("after_refresh.rs"),"pub const N:usize=42;").unwrap();h.get_by_label(tr("Refresh")).click();loop{h.run_steps(2);if h.state().dir_cache.get(root.path()).is_some_and(|nodes|nodes.iter().any(|node|node.name=="after_refresh.rs")){break;}assert!(std::time::Instant::now()<until);std::thread::sleep(std::time::Duration::from_millis(5));}h.get_by_role_and_label(egui::accesskit::Role::TreeItem,"after_refresh.rs");assert!(!root.path().join("executed-marker").exists());}
 #[test]fn oversized_real_directory_reports_limit_and_refresh_recovers(){let root=fixture();for index in 0..5001{std::fs::write(root.path().join(format!("file-{index}.rs")),"// Rust\n").unwrap();}let mut state=ExplorerState::default();state.expanded.insert("sln".into());wait(&mut state,root.path());assert!(state.dir_cache.is_empty());assert_eq!(state.io.error(&explorer_io::Key::Directory(root.path().into())),Some(explorer_io::Error::Limit));for index in 0..5001{std::fs::remove_file(root.path().join(format!("file-{index}.rs"))).unwrap();}state.io.reset();wait(&mut state,root.path());assert!(state.dir_cache[root.path()].iter().any(|node|node.name=="Cargo.toml"));assert!(!root.path().join("executed-marker").exists());}
}

#[cfg(test)]
mod template_picker_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    fn picker() -> Harness<'static, (String, Vec<ExplorerAction>)> {
        Harness::new_ui_state(|ui, state: &mut (String, Vec<ExplorerAction>)| {
            let button = ui.button("Add template");
            egui::Popup::menu(&button).show(|ui| new_item_menu(ui, Path::new("/project/src"), &mut state.0, &mut state.1));
        }, (String::new(), Vec::new()))
    }

    #[test]
    fn template_search_receives_typing_and_enter_selects_the_filtered_item() {
        let mut h = picker();
        h.get_by_label("Add template").click();
        h.run_steps(3);
        h.input_mut().events.push(egui::Event::Text(tr("Rust Module").to_string()));
        h.run_steps(2);
        assert_eq!(h.state().0, tr("Rust Module"));
        assert!(h.query_by_label(tr("Rust File")).is_none());
        h.input_mut().events.push(egui::Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
        h.run_steps(2);
        assert_eq!(h.state().1, [ExplorerAction::NewItem(PathBuf::from("/project/src"), crate::item_templates::ItemKind::RustModule)]);
    }

    #[test]
    fn template_search_explains_empty_results_and_enter_does_not_create_a_file() {
        let mut h = picker();
        h.get_by_label("Add template").click();
        h.run_steps(3);
        h.input_mut().events.push(egui::Event::Text("  no-such-template  ".into()));
        h.run_steps(2);
        assert!(h.query_by_label(tr("No file type matches the filter")).is_some());
        h.input_mut().events.push(egui::Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
        h.run_steps(2);
        assert!(h.state().1.is_empty());
    }
}
