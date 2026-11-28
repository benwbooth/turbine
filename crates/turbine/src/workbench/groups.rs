//! Editor groups, as VS Code's: the editors' place split side by side and
//! one above another, nested (a grid), each group its own tabs in its top
//! edge. A file opens in the group in use; Ctrl+\ (Split Right) and Split
//! Down show the file beside itself; a tab dragged onto another group moves
//! there, onto a group's edge splits it. Groups share their borders (one
//! line, as the side bars do), which drag to resize them; a group whose last
//! editor closes goes. One group is Turbine as it always was: its windows
//! can be tiled and cascaded.

use super::*;

/// The layout: a group, or groups side by side (`across`) or stacked,
/// each with its weight (its share, in cells once resized).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum GNode {
    Leaf(u32),
    Split { across: bool, kids: Vec<(GNode, u32)> },
}

/// A shared edge between two neighbours: where the split is (a path of
/// child indexes from the root), which pair (k, k + 1), and the line.
#[derive(Clone, Debug)]
pub(super) struct Sash {
    pub path: Vec<usize>,
    pub k: usize,
    pub across: bool,
    pub line: Rect,
}

#[derive(Debug)]
pub(super) struct Groups {
    pub tree: GNode,
    /// Each editor window's group.
    pub of: HashMap<u32, u32>,
    /// The group in use (where files open).
    pub active: u32,
    next: u32,
    /// Groups that had editors (one that empties then goes).
    had: HashSet<u32>,
    /// The editor in front last time (to follow it to its group).
    last_front: Option<u32>,
    /// Each group's place, as laid out.
    pub rects: Vec<(u32, Rect)>,
    /// An edge being dragged: it, and the place it was laid out in.
    pub drag: Option<Sash>,
    /// A tab being dragged: (window, where it was taken).
    pub tab_drag: Option<(u32, Point)>,
    /// Where a dragged tab would land: (group, side to split; None = into it).
    pub drop: Option<(u32, Option<Side>)>,
}

impl Default for Groups {
    fn default() -> Self {
        Groups {
            tree: GNode::Leaf(0),
            of: HashMap::new(),
            active: 0,
            next: 1,
            had: HashSet::new(),
            last_front: None,
            rects: Vec::new(),
            drag: None,
            tab_drag: None,
            drop: None,
        }
    }
}

impl GNode {
    pub fn leaves(&self, out: &mut Vec<u32>) {
        match self {
            GNode::Leaf(g) => out.push(*g),
            GNode::Split { kids, .. } => kids.iter().for_each(|(k, _)| k.leaves(out)),
        }
    }

    /// The kids' places in `r`: neighbours share their edge (one line).
    fn kid_rects(across: bool, kids: &[(GNode, u32)], r: Rect) -> Vec<Rect> {
        let n = kids.len() as i32;
        let len = if across { r.w } else { r.h } + (n - 1);
        let sum: u64 = kids.iter().map(|k| k.1.max(1) as u64).sum::<u64>().max(1);
        let mut out = Vec::new();
        let mut at = if across { r.x } else { r.y };
        let mut used = 0;
        for (i, (_, w)) in kids.iter().enumerate() {
            let size = if i as i32 == n - 1 { len - used } else { ((len as u64 * (*w).max(1) as u64) / sum) as i32 };
            let size = size.max(3);
            out.push(if across { Rect::new(at, r.y, size, r.h) } else { Rect::new(r.x, at, r.w, size) });
            at += size - 1;
            used += size;
        }
        out
    }

    pub fn rects(&self, r: Rect, out: &mut Vec<(u32, Rect)>) {
        match self {
            GNode::Leaf(g) => out.push((*g, r)),
            GNode::Split { across, kids } => {
                for ((k, _), kr) in kids.iter().zip(Self::kid_rects(*across, kids, r)) {
                    k.rects(kr, out);
                }
            }
        }
    }

    pub fn sashes(&self, r: Rect, path: &mut Vec<usize>, out: &mut Vec<Sash>) {
        if let GNode::Split { across, kids } = self {
            let rs = Self::kid_rects(*across, kids, r);
            for k in 0..kids.len() {
                if k + 1 < kids.len() {
                    let a = rs[k];
                    let line = if *across { Rect::new(a.right() - 1, r.y, 1, r.h) } else { Rect::new(r.x, a.bottom() - 1, r.w, 1) };
                    out.push(Sash { path: path.clone(), k, across: *across, line });
                }
                path.push(k);
                kids[k].0.sashes(rs[k], path, out);
                path.pop();
            }
        }
    }

    fn at_mut(&mut self, path: &[usize]) -> Option<&mut GNode> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match self {
                GNode::Split { kids, .. } => kids.get_mut(*i)?.0.at_mut(rest),
                GNode::Leaf(_) => None,
            },
        }
    }

    /// Put `new` beside group `g`: after it (right or below) or before.
    pub fn split_leaf(&mut self, g: u32, new: u32, across: bool, before: bool) -> bool {
        match self {
            GNode::Leaf(x) if *x == g => {
                let pair = if before { vec![(GNode::Leaf(new), 1000), (GNode::Leaf(g), 1000)] } else { vec![(GNode::Leaf(g), 1000), (GNode::Leaf(new), 1000)] };
                *self = GNode::Split { across, kids: pair };
                true
            }
            GNode::Leaf(_) => false,
            GNode::Split { across: a, kids } => {
                // (The same way as its parent: a sibling, sharing the room.)
                if *a == across {
                    if let Some(i) = kids.iter().position(|(k, _)| *k == GNode::Leaf(g)) {
                        let w = kids[i].1 / 2;
                        kids[i].1 -= w;
                        kids.insert(if before { i } else { i + 1 }, (GNode::Leaf(new), w.max(1)));
                        return true;
                    }
                }
                kids.iter_mut().any(|(k, _)| k.split_leaf(g, new, across, before))
            }
        }
    }

    /// Take group `g` out (a split left with one kid becomes that kid).
    pub fn remove(&mut self, g: u32) -> bool {
        let GNode::Split { kids, .. } = self else { return false };
        let removed = if let Some(i) = kids.iter().position(|(k, _)| *k == GNode::Leaf(g)) {
            kids.remove(i);
            true
        } else {
            kids.iter_mut().any(|(k, _)| k.remove(g))
        };
        if kids.len() == 1 {
            let only = kids.remove(0).0;
            *self = only;
        }
        removed
    }
}

impl Workbench {
    pub(super) fn group_count(&self) -> usize {
        let mut v = Vec::new();
        self.groups.tree.leaves(&mut v);
        v.len()
    }

    /// The groups' places in the editors' area, the windows in theirs, and
    /// who's in which (called as the shell is laid out). One group: nothing
    /// to do (its windows fit the area as ever).
    pub(super) fn layout_groups(&mut self, editors: Rect) {
        let alive: HashSet<u32> = self.windows.iter().filter(|w| w.dock.is_none()).map(|w| w.win.id).collect();
        self.groups.of.retain(|id, _| alive.contains(id));
        let mut leaves = Vec::new();
        self.groups.tree.leaves(&mut leaves);
        if !leaves.contains(&self.groups.active) {
            self.groups.active = leaves[0];
        }
        // (Every new editor opens in the group in use.)
        let active = self.groups.active;
        for id in &alive {
            self.groups.of.entry(*id).or_insert(active);
        }
        // (The editor in front moved: its group is the one in use.)
        let front = self.editor_index().map(|i| self.windows[i].win.id);
        if front != self.groups.last_front {
            if let Some(g) = front.and_then(|f| self.groups.of.get(&f)) {
                self.groups.active = *g;
            }
            self.groups.last_front = front;
        }
        // (A group whose last editor went goes too, as VS Code's do.)
        let used: HashSet<u32> = self.groups.of.values().copied().collect();
        for g in leaves.clone() {
            if used.contains(&g) {
                self.groups.had.insert(g);
            } else if self.groups.had.remove(&g) && self.group_count() > 1 {
                self.groups.tree.remove(g);
            }
        }
        self.groups.rects.clear();
        let mut rects = Vec::new();
        self.groups.tree.rects(editors, &mut rects);
        self.groups.rects = rects;
        let mut leaves = Vec::new();
        self.groups.tree.leaves(&mut leaves);
        if !leaves.contains(&self.groups.active) {
            self.groups.active = leaves[0];
        }
        if leaves.len() < 2 {
            return;
        }
        let places: HashMap<u32, Rect> = self.groups.rects.iter().copied().collect();
        for w in self.windows.iter_mut().filter(|w| w.dock.is_none()) {
            let g = self.groups.of.get(&w.win.id).copied().unwrap_or(active);
            if let Some(r) = places.get(&g) {
                w.win.zoomed = false;
                w.win.rect = *r;
            }
        }
    }

    /// A group's windows, oldest first (its tabs' order).
    pub(super) fn group_windows(&self, g: u32) -> Vec<u32> {
        let mut v: Vec<u32> = self.windows.iter().filter(|w| w.dock.is_none() && self.groups.of.get(&w.win.id).is_none_or(|x| *x == g)).map(|w| w.win.id).collect();
        if self.group_count() > 1 {
            v.retain(|id| self.groups.of.get(id) == Some(&g));
        }
        v.sort();
        v
    }

    /// A group's editor in front.
    pub(super) fn group_front(&self, g: u32) -> Option<u32> {
        self.windows.iter().rev().find(|w| w.dock.is_none() && self.groups.of.get(&w.win.id).is_none_or(|x| *x == g)).map(|w| w.win.id)
    }

    /// Split Right (`across`) or Down (after; else Left / Up): the file in
    /// front beside itself in a new group (empty when there's none).
    pub(super) fn split_editor(&mut self, across: bool, before: bool) {
        let l = self.layout();
        self.layout_groups(l.editors);
        let g = self.groups.active;
        let new = self.groups.next;
        self.groups.next += 1;
        self.groups.tree.split_leaf(g, new, across, before);
        self.groups.active = new;
        let front = self.group_front(g).and_then(|id| self.windows.iter().position(|w| w.win.id == id));
        if let Some(i) = front {
            let id = self.mirror_window(i);
            self.groups.of.insert(id, new);
        }
        self.layout_dock();
        self.focus_editor();
    }

    /// Window `id` in a new group at the right of `src`'s.
    pub(super) fn group_beside(&mut self, src: u32, id: u32) {
        let g = self.groups.of.get(&src).copied().unwrap_or(self.groups.active);
        let new = self.groups.next;
        self.groups.next += 1;
        self.groups.tree.split_leaf(g, new, true, false);
        self.groups.of.insert(src, g);
        self.groups.of.insert(id, new);
        self.groups.active = new;
        self.groups.last_front = Some(id);
    }

    /// The editor in front into the next group (or the one before).
    pub(super) fn move_editor_to_group(&mut self, step: i32) {
        let Some(i) = self.editor_index() else { return };
        let id = self.windows[i].win.id;
        let mut leaves = Vec::new();
        self.groups.tree.leaves(&mut leaves);
        let cur = self.groups.of.get(&id).copied().unwrap_or(self.groups.active);
        let at = leaves.iter().position(|g| *g == cur).unwrap_or(0) as i32;
        let to = at + step;
        let target = if to < 0 || to >= leaves.len() as i32 {
            // (Past the last: a new group that way, as VS Code does.)
            if self.group_windows(cur).len() < 2 {
                return;
            }
            let new = self.groups.next;
            self.groups.next += 1;
            self.groups.tree.split_leaf(cur, new, true, to < 0);
            new
        } else {
            leaves[to as usize]
        };
        self.move_window_to_group(id, target);
    }

    pub(super) fn move_window_to_group(&mut self, id: u32, g: u32) {
        self.groups.of.insert(id, g);
        self.groups.active = g;
        self.activate_pane(id);
        self.groups.last_front = Some(id);
        self.layout_dock();
    }

    /// Ctrl+1…8: that group's editor in front (or the group, when empty).
    pub(super) fn focus_group(&mut self, n: usize) {
        let mut leaves = Vec::new();
        self.groups.tree.leaves(&mut leaves);
        let Some(&g) = leaves.get(n) else { return };
        self.groups.active = g;
        if let Some(id) = self.group_front(g) {
            self.activate_pane(id);
            self.groups.last_front = Some(id);
        }
        self.focus_editor_in(g);
    }

    fn focus_editor_in(&mut self, g: u32) {
        self.shell.focus = Focus::Editors;
        self.groups.active = g;
    }

    /// Back to one group: every editor in it.
    pub(super) fn single_group(&mut self) {
        let g = self.groups.active;
        self.groups.tree = GNode::Leaf(g);
        for v in self.groups.of.values_mut() {
            *v = g;
        }
        let d = self.layout().editors;
        for w in self.windows.iter_mut().filter(|w| w.dock.is_none()) {
            w.win.zoomed = false;
            w.win.toggle_zoom(d);
        }
        self.layout_dock();
    }

    /// A preset: n columns, n rows, or a 2×2 grid (the editors stay where
    /// they are; the new groups start empty).
    pub(super) fn group_layout(&mut self, which: &str) {
        let g = self.groups.active;
        let mut fresh = || {
            let n = self.groups.next;
            self.groups.next += 1;
            n
        };
        self.groups.tree = match which {
            "two_columns" => GNode::Split { across: true, kids: vec![(GNode::Leaf(g), 1000), (GNode::Leaf(fresh()), 1000)] },
            "three_columns" => GNode::Split { across: true, kids: vec![(GNode::Leaf(g), 1000), (GNode::Leaf(fresh()), 1000), (GNode::Leaf(fresh()), 1000)] },
            "two_rows" => GNode::Split { across: false, kids: vec![(GNode::Leaf(g), 1000), (GNode::Leaf(fresh()), 1000)] },
            _ => {
                let (b, c, d) = (fresh(), fresh(), fresh());
                GNode::Split {
                    across: false,
                    kids: vec![
                        (GNode::Split { across: true, kids: vec![(GNode::Leaf(g), 1000), (GNode::Leaf(b), 1000)] }, 1000),
                        (GNode::Split { across: true, kids: vec![(GNode::Leaf(c), 1000), (GNode::Leaf(d), 1000)] }, 1000),
                    ],
                }
            }
        };
        for v in self.groups.of.values_mut() {
            *v = g;
        }
        self.layout_dock();
    }

    /// Close Group: its editors (unsaved ones ask), then the group.
    pub(super) fn close_group(&mut self) {
        let g = self.groups.active;
        let ids = self.group_windows(g);
        if ids.iter().any(|id| self.windows.iter().any(|w| w.win.id == *id && w.ed.doc.dirty)) {
            self.say("This group has unsaved files: save or close them first");
            return;
        }
        self.windows.retain(|w| !ids.contains(&w.win.id));
        self.renumber();
        if self.group_count() > 1 {
            self.groups.tree.remove(g);
        }
        self.layout_dock();
    }

    // ---- the mouse ----------------------------------------------------------------------

    /// A press on an edge between groups starts resizing them.
    pub(super) fn group_sash_press(&mut self, p: Point, editors: Rect) -> bool {
        if self.group_count() < 2 {
            return false;
        }
        let mut out = Vec::new();
        self.groups.tree.sashes(editors, &mut Vec::new(), &mut out);
        // (A stacked pair's edge is the lower one's tab row: its free part
        // drags, not a tab, ▼ or the window's buttons.)
        let on_tab = self.shell.editor_tab_rects.iter().any(|t| t.1.contains(p)) || self.shell.tab_more.iter().any(|t| t.0.contains(p));
        let on_buttons = self.groups.rects.iter().any(|(_, r)| p.y == r.y && p.x >= r.right() - 8);
        match out.into_iter().find(|s| s.line.contains(p) && (s.across || !(on_tab || on_buttons))) {
            Some(s) => {
                self.groups.drag = Some(s);
                true
            }
            None => false,
        }
    }

    /// The edge follows the mouse: the pair's sizes, the rest kept.
    pub(super) fn group_sash_drag(&mut self, p: Point, editors: Rect) {
        let Some(s) = self.groups.drag.clone() else { return };
        // (The split's own place: walk down to it.)
        let mut r = editors;
        let mut node = &self.groups.tree;
        for &i in &s.path {
            let GNode::Split { across, kids } = node else { return };
            r = GNode::kid_rects(*across, kids, r)[i];
            node = &kids[i].0;
        }
        let GNode::Split { across, kids } = node else { return };
        let rs = GNode::kid_rects(*across, kids, r);
        let sizes: Vec<i32> = rs.iter().map(|x| if *across { x.w } else { x.h }).collect();
        let (a, b) = (s.k, s.k + 1);
        let start = if *across { rs[a].x } else { rs[a].y };
        let min = if *across { 12 } else { 5 };
        let pair = sizes[a] + sizes[b];
        let want = (if *across { p.x } else { p.y }) - start + 1;
        let na = want.clamp(min, (pair - min).max(min));
        let mut new = sizes.clone();
        new[a] = na;
        new[b] = pair - na;
        if let Some(GNode::Split { kids, .. }) = self.groups.tree.at_mut(&s.path) {
            for (k, size) in kids.iter_mut().zip(new) {
                k.1 = size.max(1) as u32;
            }
        }
        self.layout_dock();
    }

    /// Where a tab dragged to `p` would land: a group, and the edge to
    /// split it at when it's near one (its outer quarter), else into it.
    pub(super) fn tab_drop_at(&self, p: Point) -> Option<(u32, Option<Side>)> {
        let (g, r) = *self.groups.rects.iter().find(|(_, r)| r.contains(p))?;
        // (Over the tabs: into it.)
        if p.y == r.y {
            return Some((g, None));
        }
        let (dx, dy) = (p.x - r.x, p.y - r.y);
        let edge = if dx < r.w / 4 {
            Some(Side::Left)
        } else if dx >= r.w - r.w / 4 {
            Some(Side::Right)
        } else if dy >= r.h - r.h / 4 {
            Some(Side::Bottom)
        } else {
            None
        };
        Some((g, edge))
    }

    /// A dragged tab let go: into that group, or a new one at its edge.
    pub(super) fn tab_drop(&mut self, id: u32, p: Point) {
        let Some((g, edge)) = self.tab_drop_at(p) else { return };
        let from = self.groups.of.get(&id).copied().unwrap_or(self.groups.active);
        match edge {
            None if g == from => {}
            None => self.move_window_to_group(id, g),
            Some(side) => {
                // (Its own group's edge with only it there: nothing to split.)
                if g == from && self.group_windows(from).len() < 2 {
                    return;
                }
                let new = self.groups.next;
                self.groups.next += 1;
                let (across, before) = match side {
                    Side::Left => (true, true),
                    Side::Right => (true, false),
                    Side::Bottom => (false, false),
                };
                self.groups.tree.split_leaf(g, new, across, before);
                self.move_window_to_group(id, new);
            }
        }
    }

    /// Empty groups: their frame and how to fill them; and where a dragged
    /// tab would land, lit.
    pub(super) fn draw_groups(&self, c: &mut Canvas, theme: &Theme) {
        if self.group_count() < 2 {
            return;
        }
        let s = &theme.scheme;
        for (g, r) in &self.groups.rects {
            if !self.group_windows(*g).is_empty() {
                continue;
            }
            let on = *g == self.groups.active && self.shell.focus == Focus::Editors;
            c.fill(Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2), " ", s.editor);
            let fc = if on { theme.style.active_frame } else { theme.style.inactive_frame };
            turbine_ui::frame::draw_frame(c, *r, fc, if on { s.frame_active } else { s.frame });
            for (k, line) in ["An empty group", "Drop a tab here, or open a file", "(Ctrl+P) while it's in use"].iter().enumerate() {
                let x = r.x + ((r.w - str_width(line) as i32) / 2).max(1);
                c.sub(*r).print(x, r.y + r.h / 2 - 1 + k as i32, line, s.editor_ghost);
            }
        }
        if let (Some(_), Some((g, edge))) = (self.groups.tab_drag, self.groups.drop) {
            let Some(r) = self.groups.rects.iter().find(|(x, _)| *x == g).map(|(_, r)| *r) else { return };
            let zone = match edge {
                None => Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2),
                Some(Side::Left) => Rect::new(r.x + 1, r.y + 1, (r.w / 2).max(1), r.h - 2),
                Some(Side::Right) => Rect::new(r.x + r.w / 2, r.y + 1, r.w - r.w / 2 - 1, r.h - 2),
                Some(Side::Bottom) => Rect::new(r.x + 1, r.y + r.h / 2, r.w - 2, r.h - r.h / 2 - 1),
            };
            c.restyle(zone, |st| Style { bg: turbine_ui::theme::vga::CYAN, fg: turbine_ui::theme::vga::BLACK, ..st });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_split_share_edges_and_collapse() {
        let mut t = GNode::Leaf(0);
        assert!(t.split_leaf(0, 1, true, false));
        assert!(t.split_leaf(1, 2, true, false), "the same way: a sibling");
        assert!(t.split_leaf(2, 3, false, false), "the other way: nested");
        let mut rs = Vec::new();
        t.rects(Rect::new(0, 0, 100, 40), &mut rs);
        let r: HashMap<u32, Rect> = rs.into_iter().collect();
        // (Neighbours share their edge: one column, one row.)
        assert_eq!(r[&0].right() - 1, r[&1].x);
        assert_eq!(r[&1].right() - 1, r[&2].x);
        assert_eq!(r[&2].right(), 100);
        assert_eq!(r[&2].bottom() - 1, r[&3].y);
        assert_eq!((r[&3].x, r[&3].w), (r[&2].x, r[&2].w));
        let mut sashes = Vec::new();
        t.sashes(Rect::new(0, 0, 100, 40), &mut Vec::new(), &mut sashes);
        assert_eq!(sashes.len(), 3);
        // Taking groups out folds the splits back.
        assert!(t.remove(3));
        assert!(t.remove(1));
        assert!(t.remove(2));
        assert_eq!(t, GNode::Leaf(0));
    }
}
