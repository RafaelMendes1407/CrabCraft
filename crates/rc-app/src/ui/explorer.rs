//! Solution Explorer: solution → solution folders → projects → Dependencies and the project's
//! folders and files (read lazily from disk). Keyboard navigation like Rider: ↑/↓ move,
//! →/← expand/collapse, Enter or double-click opens, F2 renames, ⌫ deletes.

use rc_core::workspace::{FileNode, list_dir};
use rc_dotnet::{ProjectInfo, Solution, SolutionItem};
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
    Solution,
    SolutionFolder { name: String, children: Vec<SolutionItem> },
    Project(PathBuf),
    Dependencies(PathBuf),
    Frameworks(PathBuf),
    Packages(PathBuf),
    ProjectRefs(PathBuf),
    Framework(String),
    Package { name: String, version: String },
    ProjectRef(PathBuf),
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
    pub expanded: HashSet<String>,
    pub selected: Option<String>,
    pub dir_cache: HashMap<PathBuf, Vec<FileNode>>,
    /// The dependencies shown under External Libraries; their files are not the user's to change.
    pub libraries: Vec<rc_cargo::Library>,
    pub projects: HashMap<PathBuf, ProjectInfo>,
    /// Scroll the selected row into view on the next frame.
    pub reveal: bool,
    pub has_focus: bool,
    /// Open files with a single click (Settings › Editor).
    pub single_click: bool,
    /// Folders and files the user excluded from the index, Find in Files and the analysis
    /// (root-relative, `/`-separated, sorted). Saved in the session.
    pub excluded: Vec<String>,
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
    CopyPath(PathBuf),
    OpenInTerminal(PathBuf),
    ManagePackages(PathBuf),
    /// Add New Project to this solution.
    AddProject(PathBuf),
    /// Exclude the folder or file from the index, Find in Files and the analysis (`true`), or
    /// include it again (`false`).
    SetExcluded(PathBuf, bool),
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
            self.dir_cache.remove(parent);
        }
        self.dir_cache.remove(path);
        if path.extension().is_some_and(|e| e == "csproj") {
            self.projects.remove(path);
        }
    }

    fn children_of_dir(&mut self, dir: &Path) -> Vec<FileNode> {
        self.dir_cache.entry(dir.to_path_buf()).or_insert_with(|| list_dir(dir)).clone()
    }

    fn project(&mut self, path: &Path) -> Option<ProjectInfo> {
        if !self.projects.contains_key(path) {
            let info = ProjectInfo::load(path).ok()?;
            self.projects.insert(path.to_path_buf(), info);
        }
        self.projects.get(path).cloned()
    }

    /// Expands the parents of `path` and selects it (Locate in explorer / "Always select
    /// opened file").
    pub fn locate(&mut self, path: &Path, solution: Option<&Solution>, root: &Path) {
        let mut keys = vec!["sln".to_string()];
        if let Some(sln) = solution {
            // The project that contains the file, and the solution folders around it.
            let project = sln.projects().into_iter().filter(|p| p.path.parent().is_some_and(|d| path.starts_with(d))).max_by_key(|p| p.path.as_os_str().len()).cloned();
            if let Some(p) = project {
                fn find(items: &[SolutionItem], target: &Path, trail: &mut Vec<String>) -> bool {
                    for i in items {
                        match i {
                            SolutionItem::Project(p) if p.path == target => return true,
                            SolutionItem::Folder { name, children } => {
                                trail.push(format!("{}/{name}", trail.last().cloned().unwrap_or_default()));
                                if find(children, target, trail) {
                                    return true;
                                }
                                trail.pop();
                            }
                            _ => {}
                        }
                    }
                    false
                }
                let mut trail = Vec::new();
                find(&sln.items, &p.path, &mut trail);
                keys.extend(trail.into_iter().map(|t| format!("sf:{t}")));
                keys.push(format!("proj:{}", p.path.display()));
                let pdir = p.path.parent().unwrap_or(root).to_path_buf();
                let mut d = path.parent().map(Path::to_path_buf);
                let mut dirs = Vec::new();
                while let Some(cur) = d {
                    if cur == pdir || !cur.starts_with(&pdir) {
                        break;
                    }
                    dirs.push(format!("dir:{}", cur.display()));
                    d = cur.parent().map(Path::to_path_buf);
                }
                keys.extend(dirs.into_iter().rev());
            }
        } else {
            let mut d = path.parent().map(Path::to_path_buf);
            let mut dirs = Vec::new();
            while let Some(cur) = d {
                if !cur.starts_with(root) || cur == root {
                    break;
                }
                dirs.push(format!("dir:{}", cur.display()));
                d = cur.parent().map(Path::to_path_buf);
            }
            keys.extend(dirs.into_iter().rev());
        }
        self.expanded.extend(keys);
        self.selected = Some(format!("file:{}", path.display()));
        self.reveal = true;
    }

    /// The visible rows: a depth-first walk of the expanded nodes.
    pub fn rows(&mut self, solution: Option<&Solution>, root: &Path, root_name: &str) -> Vec<Row> {
        let mut rows = Vec::new();
        let root_row = Row {
            key: "sln".into(),
            kind: NodeKind::Solution,
            depth: 0,
            label: root_name.to_string(),
            secondary: solution.map_or_else(String::new, |s| {
                let n = s.projects().len();
                if n == 1 { tr("1 project").to_string() } else { rc_kit::i18n::trf("{n} projects", &[("n", &n)]) }
            }),
            icon: if solution.is_some() { ico::SOLUTION } else { ico::FOLDER },
            expandable: true,
        };
        rows.push(root_row);
        if !self.expanded.contains("sln") {
            return rows;
        }
        match solution {
            Some(sln) => {
                let items = sln.items.clone();
                self.push_items(&mut rows, &items, 1, "");
            }
            None => self.push_dir(&mut rows, root, 1),
        }
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

    fn push_items(&mut self, rows: &mut Vec<Row>, items: &[SolutionItem], depth: usize, trail: &str) {
        for item in items {
            match item {
                SolutionItem::Folder { name, children } => {
                    let path = format!("{trail}/{name}");
                    let key = format!("sf:{path}");
                    rows.push(Row {
                        key: key.clone(),
                        kind: NodeKind::SolutionFolder { name: name.clone(), children: children.clone() },
                        depth,
                        label: name.clone(),
                        secondary: String::new(),
                        icon: ico::FOLDER,
                        expandable: !children.is_empty(),
                    });
                    if self.expanded.contains(&key) {
                        self.push_items(rows, children, depth + 1, &path);
                    }
                }
                SolutionItem::Project(p) => self.push_project(rows, &p.path, &p.name, depth),
                SolutionItem::File(f) => rows.push(file_row(f, depth)),
            }
        }
    }

    fn push_project(&mut self, rows: &mut Vec<Row>, path: &Path, name: &str, depth: usize) {
        let key = format!("proj:{}", path.display());
        let info = self.project(path);
        let exists = path.exists();
        rows.push(Row {
            key: key.clone(),
            kind: NodeKind::Project(path.to_path_buf()),
            depth,
            label: name.to_string(),
            secondary: if !exists { tr("(not found)").to_string() } else { info.as_ref().map(|i| i.target_frameworks.join(", ")).unwrap_or_default() },
            icon: if info.as_ref().is_some_and(|i| i.is_test) { ico::TEST_ROOT } else { ico::PROJECT },
            expandable: exists,
        });
        if !self.expanded.contains(&key) || !exists {
            return;
        }
        // Dependencies first, then the project folder's content (minus the project file).
        let dkey = format!("deps:{}", path.display());
        rows.push(Row {
            key: dkey.clone(),
            kind: NodeKind::Dependencies(path.to_path_buf()),
            depth: depth + 1,
            label: tr("Dependencies").to_string(),
            secondary: String::new(),
            icon: ico::LIBRARY,
            expandable: true,
        });
        if self.expanded.contains(&dkey)
            && let Some(info) = &info
        {
            let groups: [(String, &str, Ico, bool); 3] = [
                (format!("fw:{}", path.display()), tr("Frameworks"), ico::LIBRARY, !info.target_frameworks.is_empty()),
                (format!("pkgs:{}", path.display()), tr("Packages"), ico::PACKAGE, !info.packages.is_empty()),
                (format!("prefs:{}", path.display()), tr("Projects"), ico::PROJECT_GROUP, !info.project_refs.is_empty()),
            ];
            for (i, (gkey, label, icon, nonempty)) in groups.into_iter().enumerate() {
                if !nonempty {
                    continue;
                }
                let kind = match i {
                    0 => NodeKind::Frameworks(path.to_path_buf()),
                    1 => NodeKind::Packages(path.to_path_buf()),
                    _ => NodeKind::ProjectRefs(path.to_path_buf()),
                };
                rows.push(Row { key: gkey.clone(), kind, depth: depth + 2, label: label.to_string(), secondary: String::new(), icon, expandable: true });
                if !self.expanded.contains(&gkey) {
                    continue;
                }
                match i {
                    0 => {
                        for fw in &info.target_frameworks {
                            rows.push(Row { key: format!("{gkey}/{fw}"), kind: NodeKind::Framework(fw.clone()), depth: depth + 3, label: framework_label(fw), secondary: fw.clone(), icon: ico::LIBRARY, expandable: false });
                        }
                    }
                    1 => {
                        for p in &info.packages {
                            rows.push(Row {
                                key: format!("{gkey}/{}", p.name),
                                kind: NodeKind::Package { name: p.name.clone(), version: p.version.clone() },
                                depth: depth + 3,
                                label: p.name.clone(),
                                secondary: p.version.clone(),
                                icon: ico::PACKAGE,
                                expandable: false,
                            });
                        }
                    }
                    _ => {
                        for r in &info.project_refs {
                            let name = r.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                            rows.push(Row { key: format!("{gkey}/{}", r.display()), kind: NodeKind::ProjectRef(r.clone()), depth: depth + 3, label: name, secondary: String::new(), icon: ico::PROJECT, expandable: false });
                        }
                    }
                }
            }
        }
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let children = self.children_of_dir(&dir);
        let cargo = has_cargo_toml(&children);
        for c in children {
            if c.path == path {
                continue;
            }
            self.push_node(rows, &c, depth + 1, cargo);
        }
    }

    fn push_dir(&mut self, rows: &mut Vec<Row>, dir: &Path, depth: usize) {
        let children = self.children_of_dir(dir);
        // From the cached listing: no file system access while drawing.
        let cargo = has_cargo_toml(&children);
        for c in children {
            self.push_node(rows, &c, depth, cargo);
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

fn has_cargo_toml(children: &[FileNode]) -> bool {
    children.iter().any(|c| !c.is_dir && c.name == "Cargo.toml")
}

fn file_row(path: &Path, depth: usize) -> Row {
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    Row { key: format!("file:{}", path.display()), kind: NodeKind::File(path.to_path_buf()), depth, icon: icons::for_file(&name), label: name, secondary: String::new(), expandable: false }
}

/// "net8.0" → ".NET 8.0".
fn framework_label(tfm: &str) -> String {
    if let Some(v) = tfm.strip_prefix("net").filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()) && v.contains('.')) {
        format!(".NET {v}")
    } else {
        tfm.to_string()
    }
}

/// Draws the explorer inside its island; returns the user's requests.
pub fn show(ui: &mut Ui, state: &mut ExplorerState, solution: Option<&Solution>, root: &Path, root_name: &str, active_file: Option<&Path>) -> Vec<ExplorerAction> {
    let p = rc_kit::pal(ui);
    let mut actions = Vec::new();
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

    let rows = state.rows(solution, root, root_name);
    let selected_index = state.selected.as_ref().and_then(|k| rows.iter().position(|r| &r.key == k));
    let tree_id = Id::new("explorer_tree");
    let mut open_row: Option<usize> = None;
    let mut toggle_row: Option<usize> = None;
    let mut select_row: Option<usize> = None;
    let mut context: Option<(usize, egui::Response)> = None;
    // The tree takes the keyboard focus, so it is a widget of its own (accessibility needs a
    // node for the focused id).
    ui.interact(ui.max_rect(), tree_id, Sense::focusable_noninteractive()).widget_info(|| egui::WidgetInfo::new(egui::WidgetType::Other));

    let scroll = egui::ScrollArea::both().id_salt("explorer_scroll").auto_shrink([false, false]);
    scroll.show_rows(ui, ROW_H, rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let row = &rows[i];
            let width = ui.available_width().max(200.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
            let selected = selected_index == Some(i);
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
                if resp.clicked() && ui.input(|i| i.pointer.interact_pos()).is_some_and(|pt| pt.x < x0 + 18.0) {
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
            if resp.clicked() || resp.secondary_clicked() {
                select_row = Some(i);
                ui.memory_mut(|m| m.request_focus(tree_id));
            }
            if resp.double_clicked() {
                open_row = Some(i);
            } else if resp.clicked()
                && state.single_click
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
                    let rust = rc_cargo::find_manifest(root).is_some();
                    context_menu(ui, &rows[i], solution.map(|s| s.path.as_path()), root, rust, exclusion, &mut actions);
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
        NodeKind::File(p) | NodeKind::Dir(p) | NodeKind::Project(p) => Some(p.clone()),
        _ => None,
    }
}

fn context_menu(ui: &mut Ui, row: &Row, solution: Option<&Path>, root: &Path, rust: bool, exclusion: Exclusion, actions: &mut Vec<ExplorerAction>) {
    ui.set_min_width(220.0);
    ui.set_max_width(280.0);
    if let (NodeKind::Solution, Some(sln)) = (&row.kind, solution) {
        if theme::menu_item(ui, Some(ico::ADD), tr("Add New Project…"), false).clicked() {
            actions.push(ExplorerAction::AddProject(sln.to_path_buf()));
            ui.close();
        }
        if theme::menu_item(ui, Some(ico::EDIT), tr("Edit Solution File"), false).clicked() {
            actions.push(ExplorerAction::Open(sln.to_path_buf()));
            ui.close();
        }
        if theme::menu_item(ui, Some(ico::OPEN), if cfg!(target_os = "macos") { tr("Reveal in Finder") } else { tr("Open in Explorer") }, false).clicked() {
            actions.push(ExplorerAction::Reveal(sln.to_path_buf()));
            ui.close();
        }
        return;
    }
    if let NodeKind::Project(p) = &row.kind
        && theme::menu_item(ui, Some(ico::EDIT), tr("Edit Project File"), false).clicked()
    {
        actions.push(ExplorerAction::Open(p.clone()));
        ui.close();
    }
    let dir_of = |p: &Path, is_dir: bool| if is_dir { p.to_path_buf() } else { p.parent().unwrap_or(p).to_path_buf() };
    if let NodeKind::Project(p) | NodeKind::Packages(p) | NodeKind::Dependencies(p) = &row.kind
        && theme::menu_item(ui, Some(ico::PACKAGE), tr("Manage NuGet Packages…"), false).clicked()
    {
        actions.push(ExplorerAction::ManagePackages(p.clone()));
        ui.close();
    }
    let (path, is_dir) = match &row.kind {
        NodeKind::File(p) => (p.clone(), false),
        NodeKind::Dir(p) => (p.clone(), true),
        NodeKind::Project(p) => (p.clone(), false),
        // The folder of the open project; it cannot be renamed or deleted from inside.
        NodeKind::Solution if solution.is_none() => (root.to_path_buf(), true),
        NodeKind::Packages(_) | NodeKind::Dependencies(_) => return,
        _ => {
            ui.label(egui::RichText::new(tr("No actions for this item")).color(rc_kit::pal(ui).muted));
            return;
        }
    };
    let target_dir = match &row.kind {
        NodeKind::Project(p) => p.parent().unwrap_or(p).to_path_buf(),
        _ => dir_of(&path, is_dir),
    };
    if let NodeKind::File(p) = &row.kind
        && theme::menu_item(ui, Some(ico::OPEN), tr("Open"), false).clicked()
    {
        actions.push(ExplorerAction::Open(p.clone()));
        ui.close();
    }
    // Add › (Rider): types, controllers, Razor, tests, files and folders.
    ui.menu_button(tr("Add"), |ui| {
        ui.set_min_width(220.0);
        ui.set_max_width(280.0);
        use crate::item_templates::ItemKind;
        for entry in if rust { ItemKind::RUST_MENU } else { ItemKind::MENU } {
            match entry {
                None => {
                    ui.separator();
                }
                Some(kind) => {
                    let label = format!("{}…", tr(kind.title()));
                    if theme::menu_item(ui, Some(kind.icon()), &label, false).clicked() {
                        actions.push(ExplorerAction::NewItem(target_dir.clone(), *kind));
                        ui.close();
                    }
                }
            }
        }
        if rust {
            ui.separator();
            if theme::menu_item(ui, Some(ico::LIBRARY), &format!("{}…", tr("Cargo Crate")), false).clicked() {
                actions.push(ExplorerAction::NewCrate(target_dir.clone()));
                ui.close();
            }
        }
    });
    if matches!(row.kind, NodeKind::Solution) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    /// The context menu of a folder, as it would open for `exclusion`; returns what it asked for.
    fn click_in_menu(exclusion: Exclusion, label: &str, absent: &str) -> Vec<ExplorerAction> {
        let root = Path::new("/ws");
        let row = Row { key: "dir:/ws/gen".into(), kind: NodeKind::Dir(root.join("gen")), depth: 1, label: "gen".into(), secondary: String::new(), icon: ico::FOLDER, expandable: true };
        let mut h = Harness::new_ui_state(move |ui, actions: &mut Vec<ExplorerAction>| context_menu(ui, &row, None, root, true, exclusion, actions), Vec::new());
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
        let rows = state.rows(None, root, "app");
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
