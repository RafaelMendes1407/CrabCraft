//! How the editors are arranged: groups of tabs split in any direction, in the main window and
//! in floating windows. Documents are referred to by editor id; `Docs` still owns them, and
//! `sync` keeps the layout in step with what is open and active.

use egui::{Pos2, Rect};
use rc_core::session::{EditorNode, EditorWindow};

pub type DocId = u64;
pub type GroupId = u64;

/// Tabs shown together, one of them in the editor below the strip.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: GroupId,
    pub tabs: Vec<DocId>,
    pub active: Option<DocId>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Group(Group),
    /// `vertical`: side by side (else one above the other); `ratio`: the size of `a` (0..1).
    Split {
        vertical: bool,
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

/// Side of a group a dragged tab splits it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

/// Where a moved tab goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drop {
    /// Into a group, at a position of its strip (none: at the end).
    Group { group: GroupId, index: Option<usize> },
    /// Into a new group beside `group`.
    Split { group: GroupId, edge: Edge },
    /// Into a new floating window at this screen position.
    Window(Pos2),
}

/// A floating editor window.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub id: u64,
    pub root: Node,
    /// Where it opens (screen points).
    pub initial: Option<Rect>,
    /// Where it is now, as last reported by the system (saved with the session).
    pub rect: Option<Rect>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub main: Node,
    pub windows: Vec<Window>,
    /// The group new tabs open in: the one of the active document.
    pub focused: GroupId,
    next: u64,
}

impl Default for Layout {
    fn default() -> Self {
        Layout { main: Node::Group(Group { id: 1, tabs: Vec::new(), active: None }), windows: Vec::new(), focused: 1, next: 2 }
    }
}

impl Node {
    pub fn groups(&self) -> Vec<&Group> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a Group>) {
        match self {
            Node::Group(g) => out.push(g),
            Node::Split { a, b, .. } => {
                a.collect(out);
                b.collect(out);
            }
        }
    }

    fn for_each_group(&mut self, f: &mut impl FnMut(&mut Group)) {
        match self {
            Node::Group(g) => f(g),
            Node::Split { a, b, .. } => {
                a.for_each_group(f);
                b.for_each_group(f);
            }
        }
    }

    pub fn group(&self, id: GroupId) -> Option<&Group> {
        match self {
            Node::Group(g) => (g.id == id).then_some(g),
            Node::Split { a, b, .. } => a.group(id).or_else(|| b.group(id)),
        }
    }

    fn group_mut(&mut self, id: GroupId) -> Option<&mut Group> {
        match self {
            Node::Group(g) => (g.id == id).then_some(g),
            Node::Split { a, b, .. } => match a.group_mut(id) {
                Some(g) => Some(g),
                None => b.group_mut(id),
            },
        }
    }

    fn first_group(&self) -> &Group {
        match self {
            Node::Group(g) => g,
            Node::Split { a, .. } => a.first_group(),
        }
    }

    /// Without its empty groups; `None` when nothing is left.
    fn pruned(self) -> Option<Node> {
        match self {
            Node::Group(g) if g.tabs.is_empty() => None,
            Node::Group(g) => Some(Node::Group(g)),
            Node::Split { vertical, ratio, a, b } => match (a.pruned(), b.pruned()) {
                (Some(a), Some(b)) => Some(Node::Split { vertical, ratio, a: Box::new(a), b: Box::new(b) }),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            },
        }
    }

    /// Puts `new` beside group `target`; returns whether the group was found.
    fn split(&mut self, target: GroupId, edge: Edge, new: Group) -> bool {
        match self {
            Node::Group(g) if g.id == target => {
                let old = Node::Group(std::mem::replace(g, Group { id: 0, tabs: Vec::new(), active: None }));
                let new = Node::Group(new);
                let vertical = matches!(edge, Edge::Left | Edge::Right);
                let (a, b) = if matches!(edge, Edge::Left | Edge::Top) { (new, old) } else { (old, new) };
                *self = Node::Split { vertical, ratio: 0.5, a: Box::new(a), b: Box::new(b) };
                true
            }
            Node::Group(_) => false,
            Node::Split { a, b, .. } => a.split(target, edge, new.clone()) || b.split(target, edge, new),
        }
    }

    /// The group that takes the tabs of `id` when it is unsplit: the first of its sibling.
    fn sibling_of(&self, id: GroupId) -> Option<GroupId> {
        let Node::Split { a, b, .. } = self else { return None };
        match (&**a, &**b) {
            (Node::Group(g), other) if g.id == id => Some(other.first_group().id),
            (other, Node::Group(g)) if g.id == id => Some(other.first_group().id),
            _ => a.sibling_of(id).or_else(|| b.sibling_of(id)),
        }
    }
}

impl Group {
    /// Removes `doc`; the tab to its left (or the new first) becomes the active one.
    fn remove(&mut self, doc: DocId) -> Option<usize> {
        let i = self.tabs.iter().position(|&t| t == doc)?;
        self.tabs.remove(i);
        if self.active == Some(doc) {
            self.active = self.tabs.get(i.saturating_sub(1)).or(self.tabs.first()).copied();
        }
        Some(i)
    }
}

impl Layout {
    fn new_group(&mut self, tabs: Vec<DocId>) -> Group {
        let id = self.next;
        self.next += 1;
        Group { id, active: tabs.first().copied(), tabs }
    }

    /// Every tree: the main window's, then the floating windows'.
    pub fn trees(&self) -> impl Iterator<Item = &Node> {
        std::iter::once(&self.main).chain(self.windows.iter().map(|w| &w.root))
    }

    fn for_each_tree(&mut self, mut f: impl FnMut(&mut Node)) {
        f(&mut self.main);
        for w in &mut self.windows {
            f(&mut w.root);
        }
    }

    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.trees().find_map(|t| t.group(id))
    }

    fn group_mut(&mut self, id: GroupId) -> Option<&mut Group> {
        if let Some(g) = self.main.group_mut(id) {
            return Some(g);
        }
        self.windows.iter_mut().find_map(|w| w.root.group_mut(id))
    }

    pub fn group_of(&self, doc: DocId) -> Option<GroupId> {
        self.trees().flat_map(|t| t.groups()).find(|g| g.tabs.contains(&doc)).map(|g| g.id)
    }

    /// The floating window holding `group` (none: the main window).
    pub fn window_of(&self, group: GroupId) -> Option<usize> {
        self.windows.iter().position(|w| w.root.group(group).is_some())
    }

    /// Whether `doc` is in a floating window.
    #[cfg(test)]
    pub fn is_floating(&self, doc: DocId) -> bool {
        self.group_of(doc).is_some_and(|g| self.window_of(g).is_some())
    }

    /// How many groups the tree holding `group` has.
    pub fn groups_beside(&self, group: GroupId) -> usize {
        self.trees().find(|t| t.group(group).is_some()).map_or(0, |t| t.groups().len())
    }

    /// Follows the open documents: closed ones leave their group, new ones join the focused
    /// group, the active one focuses its group, pinned tabs come first and empty groups go.
    pub fn sync(&mut self, docs: &[(DocId, bool)], active: Option<DocId>) {
        let open = |d: &DocId| docs.iter().any(|(id, _)| id == d);
        self.for_each_tree(|t| {
            t.for_each_group(&mut |g| {
                let closed: Vec<DocId> = g.tabs.iter().copied().filter(|d| !open(d)).collect();
                for d in closed {
                    g.remove(d);
                }
            })
        });
        if self.group(self.focused).is_none() {
            self.focused = self.main.first_group().id;
        }
        let placed: Vec<DocId> = self.trees().flat_map(|t| t.groups()).flat_map(|g| g.tabs.iter().copied()).collect();
        let focused = self.focused;
        for (id, _) in docs.iter().filter(|(id, _)| !placed.contains(id)) {
            if let Some(g) = self.group_mut(focused) {
                g.tabs.push(*id);
            }
        }
        if let Some(a) = active
            && let Some(g) = self.group_of(a)
        {
            self.focused = g;
            if let Some(g) = self.group_mut(g) {
                g.active = Some(a);
            }
        }
        let pinned = |d: &DocId| docs.iter().any(|(id, p)| id == d && *p);
        self.for_each_tree(|t| {
            t.for_each_group(&mut |g| {
                g.tabs.sort_by_key(|d| !pinned(d));
                if g.active.is_none_or(|a| !g.tabs.contains(&a)) {
                    g.active = g.tabs.first().copied();
                }
            })
        });
        self.prune();
    }

    /// Drops empty groups and windows; the main window always keeps one group.
    fn prune(&mut self) {
        let keep = self.main.first_group().id;
        let main = std::mem::replace(&mut self.main, Node::Group(Group { id: keep, tabs: Vec::new(), active: None }));
        if let Some(m) = main.pruned() {
            self.main = m;
        }
        self.windows = std::mem::take(&mut self.windows).into_iter().filter_map(|w| w.root.pruned().map(|root| Window { root, ..w })).collect();
        if self.group(self.focused).is_none() {
            self.focused = self.main.first_group().id;
        }
    }

    /// Moves `doc` to `to`; returns whether anything changed. A tab does not split off from a
    /// group it is alone in, since that would leave the group empty.
    pub fn move_doc(&mut self, doc: DocId, to: Drop) -> bool {
        let Some(from) = self.group_of(doc) else { return false };
        let alone = self.group(from).is_some_and(|g| g.tabs.len() == 1);
        match to {
            Drop::Group { group, index } => {
                if self.group(group).is_none() || (group == from && index.is_none()) {
                    return false;
                }
                let old = self.group_mut(from).and_then(|g| g.remove(doc));
                let Some(g) = self.group_mut(group) else { return false };
                let mut at = index.unwrap_or(g.tabs.len());
                if group == from && old.is_some_and(|o| o < at) {
                    at -= 1;
                }
                g.tabs.insert(at.min(g.tabs.len()), doc);
                g.active = Some(doc);
                self.focused = group;
            }
            Drop::Split { group, edge } => {
                if self.group(group).is_none() || (group == from && alone) {
                    return false;
                }
                if let Some(g) = self.group_mut(from) {
                    g.remove(doc);
                }
                let new = self.new_group(vec![doc]);
                let id = new.id;
                let mut placed = false;
                self.for_each_tree(|t| placed = placed || t.split(group, edge, new.clone()));
                self.focused = id;
            }
            Drop::Window(pos) => {
                if alone && self.window_of(from).is_some() {
                    return false;
                }
                if let Some(g) = self.group_mut(from) {
                    g.remove(doc);
                }
                let new = self.new_group(vec![doc]);
                self.focused = new.id;
                let id = self.next;
                self.next += 1;
                let initial = Some(Rect::from_min_size(pos, egui::vec2(960.0, 720.0)));
                self.windows.push(Window { id, root: Node::Group(new), initial, rect: None });
            }
        }
        self.prune();
        true
    }

    /// Puts the tabs of `group` into the group beside it.
    pub fn unsplit(&mut self, group: GroupId) {
        let Some(target) = self.trees().find_map(|t| t.sibling_of(group)) else { return };
        let (tabs, active) = match self.group_mut(group) {
            Some(g) => (std::mem::take(&mut g.tabs), g.active),
            None => return,
        };
        let was_focused = self.focused == group;
        if let Some(t) = self.group_mut(target) {
            t.tabs.extend(tabs);
            if was_focused {
                t.active = active;
            }
        }
        if self.focused == group {
            self.focused = target;
        }
        self.prune();
    }

    /// Every group of the tree holding `group` becomes one.
    pub fn unsplit_all(&mut self, group: GroupId) {
        let gather = |t: &mut Node| {
            let mut tabs = Vec::new();
            t.for_each_group(&mut |g| tabs.append(&mut g.tabs));
            let first = t.first_group().id;
            if let Some(g) = t.group_mut(first) {
                g.tabs = tabs;
            }
        };
        if self.main.group(group).is_some() {
            gather(&mut self.main);
        } else if let Some(w) = self.windows.iter_mut().find(|w| w.root.group(group).is_some()) {
            gather(&mut w.root);
        }
        self.prune();
    }

    /// Closes floating window `i`: its tabs go back to the main window.
    pub fn dock_window(&mut self, i: usize) {
        if i >= self.windows.len() {
            return;
        }
        let w = self.windows.remove(i);
        let target = self.main_target();
        let tabs: Vec<DocId> = w.root.groups().iter().flat_map(|g| g.tabs.iter().copied()).collect();
        if let Some(g) = self.main.group_mut(target) {
            g.tabs.extend(tabs);
        }
        self.focused = target;
        self.prune();
    }

    /// The main window's group that takes tabs coming back from floating windows.
    pub fn main_target(&self) -> GroupId {
        if self.main.group(self.focused).is_some() { self.focused } else { self.main.first_group().id }
    }

    /// Sets the size of the split at `path` (`false`: into `a`) of a window's tree.
    pub fn set_ratio(&mut self, window: Option<u64>, path: &[bool], ratio: f32) {
        let mut node = match window {
            None => &mut self.main,
            Some(id) => match self.windows.iter_mut().find(|w| w.id == id) {
                Some(w) => &mut w.root,
                None => return,
            },
        };
        for &step in path {
            match node {
                Node::Split { a, b, .. } => node = if step { &mut **b } else { &mut **a },
                Node::Group(_) => return,
            }
        }
        if let Node::Split { ratio: r, .. } = node {
            *r = ratio.clamp(0.1, 0.9);
        }
    }

    /// The tab after (or before) `doc` in its group, wrapping around.
    pub fn next_tab(&self, doc: DocId, forward: bool) -> Option<DocId> {
        let g = self.group(self.group_of(doc)?)?;
        let i = g.tabs.iter().position(|&t| t == doc)?;
        let n = g.tabs.len();
        Some(g.tabs[if forward { (i + 1) % n } else { (i + n - 1) % n }])
    }

    /// The document to show once `doc` closes: its left neighbour in the group, else the
    /// active tab of another group.
    pub fn after_close(&self, doc: DocId) -> Option<DocId> {
        let mut copy = self.clone();
        let g = copy.group_of(doc)?;
        copy.group_mut(g)?.remove(doc);
        copy.group(g).and_then(|g| g.active).or_else(|| copy.trees().flat_map(|t| t.groups()).find_map(|g| g.active))
    }

    // ------------------------------------------------------------------ session

    pub fn to_session(&self, index_of: impl Fn(DocId) -> Option<usize>) -> (EditorNode, Vec<EditorWindow>) {
        fn node(n: &Node, index_of: &dyn Fn(DocId) -> Option<usize>) -> EditorNode {
            match n {
                Node::Group(g) => EditorNode::Group { tabs: g.tabs.iter().filter_map(|&d| index_of(d)).collect(), active: g.active.and_then(index_of) },
                Node::Split { vertical, ratio, a, b } => {
                    EditorNode::Split { vertical: *vertical, ratio: *ratio, a: Box::new(node(a, index_of)), b: Box::new(node(b, index_of)) }
                }
            }
        }
        let windows = self
            .windows
            .iter()
            .map(|w| EditorWindow { root: node(&w.root, &index_of), rect: w.rect.or(w.initial).map(|r| [r.left(), r.top(), r.width(), r.height()]) })
            .collect();
        (node(&self.main, &index_of), windows)
    }

    /// The saved arrangement, for the documents that could be opened again (`id_of` maps the
    /// session's file indexes to them). Anything that does not fit is dropped; `sync` puts
    /// documents the session did not place into the focused group.
    pub fn from_session(main: Option<&EditorNode>, windows: &[EditorWindow], id_of: impl Fn(usize) -> Option<DocId>) -> Layout {
        let mut layout = Layout::default();
        let mut seen = Vec::new();
        fn node(n: &EditorNode, l: &mut Layout, seen: &mut Vec<DocId>, id_of: &dyn Fn(usize) -> Option<DocId>) -> Node {
            match n {
                EditorNode::Group { tabs, active } => {
                    let mut placed = Vec::new();
                    for d in tabs.iter().filter_map(|&i| id_of(i)) {
                        if !seen.contains(&d) {
                            seen.push(d);
                            placed.push(d);
                        }
                    }
                    let tabs = placed;
                    let mut g = l.new_group(tabs);
                    g.active = active.and_then(id_of).filter(|a| g.tabs.contains(a)).or(g.active);
                    Node::Group(g)
                }
                EditorNode::Split { vertical, ratio, a, b } => {
                    let ratio = if ratio.is_finite() { ratio.clamp(0.1, 0.9) } else { 0.5 };
                    Node::Split { vertical: *vertical, ratio, a: Box::new(node(a, l, seen, id_of)), b: Box::new(node(b, l, seen, id_of)) }
                }
            }
        }
        if let Some(m) = main {
            let root = node(m, &mut layout, &mut seen, &id_of);
            let first = root.first_group().id;
            layout.main = root.pruned().unwrap_or(Node::Group(Group { id: first, tabs: Vec::new(), active: None }));
        }
        for w in windows {
            let root = node(&w.root, &mut layout, &mut seen, &id_of);
            let rect = w
                .rect
                .filter(|r| r.iter().all(|v| v.is_finite()) && r[2] >= 200.0 && r[3] >= 150.0)
                .map(|[x, y, w, h]| Rect::from_min_size(egui::pos2(x, y), egui::vec2(w.min(8000.0), h.min(8000.0))));
            let id = layout.next;
            layout.next += 1;
            layout.windows.push(Window { id, root, initial: rect, rect });
        }
        layout.focused = layout.main.first_group().id;
        layout.prune();
        layout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(ids: &[DocId]) -> Vec<(DocId, bool)> {
        ids.iter().map(|&d| (d, false)).collect()
    }

    fn tabs(l: &Layout, g: GroupId) -> Vec<DocId> {
        l.group(g).unwrap().tabs.clone()
    }

    #[test]
    fn new_documents_join_the_focused_group_and_closed_ones_leave() {
        let mut l = Layout::default();
        l.sync(&docs(&[1, 2, 3]), Some(3));
        assert_eq!(tabs(&l, 1), vec![1, 2, 3]);
        assert_eq!(l.group(1).unwrap().active, Some(3));
        l.sync(&docs(&[1, 3]), Some(3));
        assert_eq!(tabs(&l, 1), vec![1, 3]);
    }

    #[test]
    fn tabs_reorder_inside_a_group_and_pinned_ones_stay_first() {
        let mut l = Layout::default();
        l.sync(&docs(&[1, 2, 3]), Some(1));
        assert!(l.move_doc(1, Drop::Group { group: 1, index: Some(3) }));
        assert_eq!(tabs(&l, 1), vec![2, 3, 1]);
        assert!(l.move_doc(1, Drop::Group { group: 1, index: Some(0) }));
        assert_eq!(tabs(&l, 1), vec![1, 2, 3]);
        l.sync(&[(1, false), (2, false), (3, true)], Some(1));
        assert_eq!(tabs(&l, 1), vec![3, 1, 2]);
    }

    #[test]
    fn splitting_moves_the_tab_to_a_new_group_and_empty_groups_disappear() {
        let mut l = Layout::default();
        l.sync(&docs(&[1, 2, 3]), Some(1));
        assert!(l.move_doc(2, Drop::Split { group: 1, edge: Edge::Right }));
        let g2 = l.group_of(2).unwrap();
        assert_ne!(g2, 1);
        assert_eq!(l.focused, g2);
        assert!(matches!(&l.main, Node::Split { vertical: true, .. }));
        // A tab alone in its group does not split off from itself.
        assert!(!l.move_doc(2, Drop::Split { group: g2, edge: Edge::Bottom }));
        // Three groups, nested: below the second one.
        assert!(l.move_doc(3, Drop::Split { group: g2, edge: Edge::Bottom }));
        assert_eq!(l.main.groups().len(), 3);
        // Moving the last tab of a group out removes the group.
        assert!(l.move_doc(2, Drop::Group { group: 1, index: None }));
        assert_eq!(l.main.groups().len(), 2);
        assert_eq!(tabs(&l, 1), vec![1, 2]);
        // Closing every document of a group removes it too.
        l.sync(&docs(&[1, 2]), Some(1));
        assert_eq!(l.main.groups().len(), 1);
        assert!(matches!(l.main, Node::Group(_)));
    }

    #[test]
    fn unsplit_gives_the_tabs_to_the_neighbour() {
        let mut l = Layout::default();
        l.sync(&docs(&[1, 2, 3]), Some(1));
        l.move_doc(2, Drop::Split { group: 1, edge: Edge::Left });
        l.move_doc(3, Drop::Split { group: 1, edge: Edge::Top });
        assert_eq!(l.main.groups().len(), 3);
        let g3 = l.group_of(3).unwrap();
        l.unsplit(g3);
        assert_eq!(l.main.groups().len(), 2);
        assert_eq!(tabs(&l, 1), vec![1, 3]);
        l.unsplit_all(1);
        assert_eq!(l.main.groups().len(), 1);
        let mut all = l.main.groups()[0].tabs.clone();
        all.sort();
        assert_eq!(all, vec![1, 2, 3]);
    }

    #[test]
    fn floating_windows_hold_groups_and_give_their_tabs_back_when_closed() {
        let mut l = Layout::default();
        l.sync(&docs(&[1, 2, 3]), Some(1));
        assert!(l.move_doc(2, Drop::Window(egui::pos2(100.0, 100.0))));
        assert_eq!(l.windows.len(), 1);
        assert!(l.is_floating(2));
        let wg = l.group_of(2).unwrap();
        // A second tab joins the window, and the window splits like the main one.
        assert!(l.move_doc(3, Drop::Group { group: wg, index: None }));
        assert!(l.move_doc(3, Drop::Split { group: wg, edge: Edge::Right }));
        assert_eq!(l.windows[0].root.groups().len(), 2);
        // The last tab of a window does not tear off into another window.
        let solo = l.group_of(3).unwrap();
        assert_eq!(tabs(&l, solo), vec![3]);
        assert!(!l.move_doc(3, Drop::Window(egui::pos2(0.0, 0.0))));
        // Dragging both back empties the window, which goes away.
        l.move_doc(3, Drop::Group { group: 1, index: Some(0) });
        l.move_doc(2, Drop::Group { group: 1, index: None });
        assert!(l.windows.is_empty());
        assert_eq!(tabs(&l, 1), vec![3, 1, 2]);
        // Closing a window brings its tabs back.
        l.move_doc(1, Drop::Window(egui::pos2(0.0, 0.0)));
        l.dock_window(0);
        assert!(l.windows.is_empty());
        assert_eq!(tabs(&l, 1), vec![3, 2, 1]);
    }

    #[test]
    fn closing_shows_the_left_neighbour_in_the_same_group() {
        let mut l = Layout::default();
        l.sync(&docs(&[1, 2, 3, 4]), Some(1));
        l.move_doc(3, Drop::Split { group: 1, edge: Edge::Right });
        l.move_doc(4, Drop::Group { group: l.group_of(3).unwrap(), index: None });
        assert_eq!(l.after_close(4), Some(3));
        assert_eq!(l.after_close(2), Some(1));
        assert_eq!(l.next_tab(1, true), Some(2));
        assert_eq!(l.next_tab(2, true), Some(1));
        assert_eq!(l.next_tab(3, false), Some(4));
    }

    #[test]
    fn the_layout_round_trips_through_the_session_and_bad_entries_are_dropped() {
        let mut l = Layout::default();
        l.sync(&docs(&[10, 20, 30]), Some(10));
        l.move_doc(20, Drop::Split { group: 1, edge: Edge::Bottom });
        l.move_doc(30, Drop::Window(egui::pos2(50.0, 60.0)));
        let ids = [10u64, 20, 30];
        let (main, windows) = l.to_session(|d| ids.iter().position(|&x| x == d));
        let back = Layout::from_session(Some(&main), &windows, |i| ids.get(i).copied());
        assert_eq!(back.main.groups().iter().map(|g| g.tabs.clone()).collect::<Vec<_>>(), vec![vec![10], vec![20]]);
        assert_eq!(back.windows.len(), 1);
        assert_eq!(back.windows[0].root.groups()[0].tabs, vec![30]);
        assert_eq!(back.windows[0].initial.map(|r| r.min), Some(egui::pos2(50.0, 60.0)));
        // Missing files, repeated indexes, odd ratios and tiny windows.
        let bad = EditorNode::Split {
            vertical: true,
            ratio: f32::NAN,
            a: Box::new(EditorNode::Group { tabs: vec![0, 0, 99], active: Some(99) }),
            b: Box::new(EditorNode::Group { tabs: vec![7], active: None }),
        };
        let w = EditorWindow { root: EditorNode::Group { tabs: vec![1], active: None }, rect: Some([0.0, 0.0, 1.0, f32::INFINITY]) };
        let l = Layout::from_session(Some(&bad), &[w], |i| ids.get(i).copied());
        assert_eq!(l.main, Node::Group(Group { id: l.main.groups()[0].id, tabs: vec![10], active: Some(10) }));
        assert_eq!(l.windows[0].initial, None);
    }
}
