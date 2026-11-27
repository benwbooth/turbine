//! The workbench's docked parts, as in Cursor (VS Code): the primary side
//! bar at the left (Explorer, Outline, Search, Source Control, Run and
//! Debug), the panel under the editors (Problems, Output, Terminal) and the
//! secondary side bar at the right (AI Chat). The editors (Turbine's
//! windows) take the room in the middle.
//!
//! Each part is a frame in the control style with its tabs in the top edge;
//! the edge facing the editors drags to resize it, and parts show and hide
//! (Ctrl+B, Ctrl+J, Ctrl+Alt+B).

use turbine_text::unicode::str_width;
use turbine_ui::frame::draw_frame;
use turbine_ui::theme::{Style, Theme};
use turbine_ui::{Canvas, Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Bottom,
    Right,
}

/// A view a part can show: built in, or a window docked there (a terminal,
/// the chat log, the output log), by window id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Explorer,
    Outline,
    Search,
    Scm,
    Debug,
    Extensions,
    Problems,
    /// The current file's history (git), a section under the Explorer.
    Timeline,
    /// The panel's own tabs, as VS Code's: each opens its window (which
    /// then takes the tab's place) when it's picked.
    Output,
    DebugConsole,
    Terminal,
    Pane(u32),
}

impl Tab {
    pub fn name(self) -> &'static str {
        match self {
            Tab::Explorer => "Explorer",
            Tab::Outline => "Outline",
            Tab::Search => "Search",
            Tab::Scm => "Source Control",
            Tab::Debug => "Run and Debug",
            Tab::Extensions => "Extensions",
            Tab::Problems => "Problems",
            Tab::Timeline => "Timeline",
            Tab::Output => "Output",
            Tab::DebugConsole => "Debug Console",
            Tab::Terminal => "Terminal",
            Tab::Pane(_) => "",
        }
    }
    /// A one-letter label for a narrow part.
    pub fn short(self) -> Option<&'static str> {
        Some(match self {
            Tab::Explorer => "E",
            Tab::Outline => "O",
            Tab::Search => "S",
            Tab::Scm => "G",
            Tab::Debug => "D",
            Tab::Extensions => "X",
            Tab::Problems => "P",
            Tab::Timeline => "M",
            Tab::Output => "L",
            Tab::DebugConsole => "C",
            Tab::Terminal => "T",
            Tab::Pane(_) => return None,
        })
    }
    /// The automation id of a built-in view.
    pub fn key(self) -> String {
        match self {
            Tab::Pane(id) => format!("pane{id}"),
            t => t.name().to_lowercase().replace(' ', "_"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Part {
    pub visible: bool,
    /// Width (side bars) or height (the panel) in cells.
    pub size: i32,
    pub tabs: Vec<Tab>,
    pub active: usize,
}

impl Part {
    pub fn current(&self) -> Option<Tab> {
        self.tabs.get(self.active).copied()
    }
    pub fn show(&mut self, t: Tab) {
        if let Some(i) = self.tabs.iter().position(|x| *x == t) {
            self.active = i;
            self.visible = true;
        }
    }
}

/// Where everything goes this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub left: Option<Rect>,
    pub bottom: Option<Rect>,
    pub right: Option<Rect>,
    /// The editors' area (the windows), and the tab row above it.
    pub editors: Rect,
    pub editor_tabs: Option<Rect>,
    /// Where the cursor is: folders » file » symbols (under the tabs).
    pub breadcrumbs: Option<Rect>,
}

impl Layout {
    pub fn part(&self, side: Side) -> Option<Rect> {
        match side {
            Side::Left => self.left,
            Side::Bottom => self.bottom,
            Side::Right => self.right,
        }
    }
}

/// Rounded ends for tab pills (Nerd Font / Powerline round caps).
pub const CAP_LEFT: &str = "\u{e0b6}";
pub const CAP_RIGHT: &str = "\u{e0b4}";

#[derive(Clone, Copy, Debug)]
struct Drag {
    side: Side,
    start: Point,
    size: i32,
}

#[derive(Clone, Debug)]
pub struct Dock {
    pub left: Part,
    pub bottom: Part,
    pub right: Part,
    /// A row of tabs for the open editors above them.
    pub editor_tabs: bool,
    /// A breadcrumbs row above the editors.
    pub breadcrumbs: bool,
    /// The front editor has no file: no breadcrumbs row (set each layout).
    pub crumbs_hidden: bool,
    /// Tabs with emoji icons and rounded ends (from the control style).
    pub icons: bool,
    /// The panel covers the editors (VS Code's Maximize Panel Size).
    pub maximized: bool,
    /// Sections under the Explorer, as VS Code stacks them: (view, open,
    /// rows when open).
    pub sections: Vec<(Tab, bool, i32)>,
    drag: Option<Drag>,
}

impl Default for Dock {
    fn default() -> Self {
        Dock {
            left: Part { visible: true, size: 30, tabs: vec![Tab::Explorer, Tab::Search, Tab::Scm, Tab::Debug, Tab::Extensions], active: 0 },
            bottom: Part { visible: false, size: 12, tabs: vec![Tab::Problems, Tab::Output, Tab::DebugConsole, Tab::Terminal], active: 0 },
            right: Part { visible: false, size: 46, tabs: Vec::new(), active: 0 },
            editor_tabs: true,
            breadcrumbs: true,
            crumbs_hidden: false,
            icons: false,
            maximized: false,
            sections: vec![(Tab::Outline, true, 10), (Tab::Timeline, false, 8)],
            drag: None,
        }
    }
}

impl Dock {
    pub fn part(&self, s: Side) -> &Part {
        match s {
            Side::Left => &self.left,
            Side::Bottom => &self.bottom,
            Side::Right => &self.right,
        }
    }
    pub fn part_mut(&mut self, s: Side) -> &mut Part {
        match s {
            Side::Left => &mut self.left,
            Side::Bottom => &mut self.bottom,
            Side::Right => &mut self.right,
        }
    }

    /// Which part has this tab.
    pub fn side_of(&self, t: Tab) -> Option<Side> {
        [Side::Left, Side::Bottom, Side::Right].into_iter().find(|s| self.part(*s).tabs.contains(&t)).or_else(|| {
            // (A section goes with the Explorer.)
            self.sections.iter().any(|(x, ..)| *x == t).then(|| self.side_of(Tab::Explorer)).flatten()
        })
    }

    /// The Explorer's part split into the Explorer and its sections below:
    /// (the Explorer's place, each section's header row and its body when
    /// open). None when the part doesn't show the Explorer.
    pub fn sections_in(&self, side: Side, r: Rect) -> Option<(Rect, Vec<(Tab, Rect, Option<Rect>)>)> {
        let part = self.part(side);
        if part.current() != Some(Tab::Explorer) || self.sections.is_empty() {
            return None;
        }
        let inner = Self::content(r);
        // (The Explorer keeps 4 rows at least; sections shrink first.)
        let headers = self.sections.len() as i32;
        let room = (inner.h - 4 - headers).max(0);
        let mut want: Vec<i32> = self.sections.iter().map(|(_, open, size)| if *open { (*size).max(2) } else { 0 }).collect();
        while want.iter().sum::<i32>() > room {
            let Some(m) = want.iter_mut().max_by_key(|w| **w) else { break };
            if *m == 0 {
                break;
            }
            *m -= 1;
        }
        let mut y = inner.bottom();
        let mut out = Vec::new();
        for (i, (t, open, _)) in self.sections.iter().enumerate().rev() {
            let body = (*open && want[i] > 0).then(|| {
                y -= want[i];
                Rect::new(inner.x, y, inner.w, want[i])
            });
            y -= 1;
            out.push((*t, Rect::new(r.x, y, r.w, 1), body));
        }
        out.reverse();
        Some((Rect::new(inner.x, inner.y, inner.w, (y - inner.y).max(0)), out))
    }

    /// Show a tab (its part too).
    pub fn show(&mut self, t: Tab) {
        if let Some(s) = self.side_of(t) {
            self.part_mut(s).show(t);
        }
    }

    pub fn add(&mut self, s: Side, t: Tab) {
        let p = self.part_mut(s);
        if !p.tabs.contains(&t) {
            p.tabs.push(t);
        }
        p.show(t);
    }

    pub fn remove(&mut self, t: Tab) {
        for s in [Side::Left, Side::Bottom, Side::Right] {
            let p = self.part_mut(s);
            if let Some(i) = p.tabs.iter().position(|x| *x == t) {
                p.tabs.remove(i);
                if p.active >= i && p.active > 0 {
                    p.active -= 1;
                }
                if p.tabs.is_empty() {
                    p.visible = false;
                }
            }
        }
    }

    /// Lay the parts out in `area` (between the menu bar and status line).
    /// Each keeps at least a little room for the editors.
    pub fn layout(&self, area: Rect) -> Layout {
        let shown = |p: &Part| p.visible && !p.tabs.is_empty();
        let min_ed_w = 24.min(area.w);
        let mut lw = if shown(&self.left) { self.left.size.clamp(12, (area.w - min_ed_w).max(12)) } else { 0 };
        let mut rw = if shown(&self.right) { self.right.size.clamp(16, (area.w - min_ed_w - lw).max(16)) } else { 0 };
        if lw + rw > area.w - min_ed_w {
            // (Too narrow for both at their sizes: share what there is.)
            let room = (area.w - min_ed_w).max(0);
            if lw > 0 && rw > 0 {
                lw = room / 2;
                rw = room - lw;
            } else {
                lw = lw.min(room);
                rw = rw.min(room);
            }
        }
        let left = (lw > 0).then(|| Rect::new(area.x, area.y, lw, area.h));
        let right = (rw > 0).then(|| Rect::new(area.right() - rw, area.y, rw, area.h));
        // (The editors start on the left part's last column: one border
        // between them, not two side by side.)
        let ol = i32::from(lw > 0);
        let mid = Rect::new(area.x + lw - ol, area.y, (area.w - lw - rw + ol).max(0), area.h);
        let bh = if shown(&self.bottom) { self.bottom.size.clamp(4, (mid.h - 6).max(4)) } else { 0 };
        let bottom = (bh > 0 && mid.h > bh).then(|| Rect::new(mid.x, mid.bottom() - bh, mid.w, bh));
        // (And down onto the panel's top row: one border between them.)
        let ed = Rect::new(mid.x, mid.y, mid.w, mid.h - bottom.map_or(0, |b| b.h - 1));
        // Breadcrumbs above the editors (none for a file with no path:
        // they'd only repeat its name); the tabs in the editor window's
        // top border, where its title would be (Turbo Vision's way), so a
        // file's name shows once.
        let (breadcrumbs, editors) = if self.breadcrumbs && !self.crumbs_hidden && ed.h > 6 {
            (Some(Rect::new(ed.x, ed.y, ed.w, 1)), Rect::new(ed.x, ed.y + 1, ed.w, ed.h - 1))
        } else {
            (None, ed)
        };
        let editor_tabs = (self.editor_tabs && editors.h > 4 && editors.w > 16).then(|| Rect::new(editors.x + 2, editors.y, editors.w - 9, 1));
        // Maximized: the panel over the whole of the editors' place (they
        // keep theirs underneath, drawn over).
        if self.maximized && bottom.is_some() {
            return Layout { left, bottom: Some(mid), right, editors, editor_tabs: None, breadcrumbs: None };
        }
        Layout { left, bottom, right, editors, editor_tabs, breadcrumbs }
    }

    /// Where a part's view draws: inside its frame.
    pub fn content(r: Rect) -> Rect {
        r.inset(1)
    }

    /// The tabs in a part's top edge, centred like a DOS window's title:
    /// their places and labels. The current one is a pill (rounded ends)
    /// when the style has icons. When the names don't all fit, the current
    /// tab keeps its name and the others shrink to their icon (as
    /// an activity bar), or a letter without icons.
    pub fn tab_rects(&self, side: Side, r: Rect, title: &dyn Fn(Tab) -> String) -> Vec<(Tab, Rect, String)> {
        let part = self.part(side);
        // (The panel keeps the right of its edge for its buttons.)
        let r = if side == Side::Bottom && r.w > 40 { Rect::new(r.x, r.y, r.w - 14, r.h) } else { r };
        let caps = |_: Tab| 2;
        let room = r.w - 4;
        let width = |labels: &[(Tab, String)]| labels.iter().map(|(t, l)| str_width(l) as i32 + caps(*t)).sum::<i32>() + labels.len().saturating_sub(1) as i32;
        let mut labels: Vec<(Tab, String)> = part.tabs.iter().map(|t| (*t, title(*t))).collect();
        if width(&labels) > room {
            for (t, l) in &mut labels {
                if part.current() != Some(*t) {
                    *l = if self.icons {
                        l.split(' ').next().unwrap_or("").to_string()
                    } else {
                        t.short().map(str::to_string).unwrap_or_else(|| l.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default())
                    };
                }
            }
        }
        // (Still too wide: drop tabs after the current one, then before.)
        while width(&labels) > room && labels.len() > 1 {
            let cur = labels.iter().position(|(t, _)| part.current() == Some(*t)).unwrap_or(0);
            let drop = if cur + 1 < labels.len() { labels.len() - 1 } else { 0 };
            labels.remove(drop);
        }
        let mut x = r.x + ((r.w - width(&labels)) / 2).max(2);
        let mut out = Vec::new();
        for (t, l) in labels {
            let w = str_width(&l) as i32 + caps(t);
            out.push((t, Rect::new(x, r.y, w, 1), l));
            x += w + 1;
        }
        out
    }

    /// The edge of a part that drags to resize it (facing the editors).
    fn sash(side: Side, r: Rect, p: Point) -> bool {
        match side {
            Side::Left => p.x == r.right() - 1 && p.y > r.y && p.y < r.bottom(),
            Side::Right => p.x == r.x && p.y > r.y && p.y < r.bottom(),
            // (The panel's whole top edge; its tabs and buttons are
            // picked out before this is asked.)
            Side::Bottom => p.y == r.y,
        }
    }

    /// A press: start resizing when it's on a part's inner edge.
    pub fn begin_drag(&mut self, l: &Layout, p: Point) -> bool {
        for side in [Side::Left, Side::Bottom, Side::Right] {
            if let Some(r) = l.part(side) {
                if Self::sash(side, r, p) {
                    self.drag = Some(Drag { side, start: p, size: self.part(side).size });
                    return true;
                }
            }
        }
        false
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn drag_to(&mut self, p: Point) {
        let Some(d) = self.drag else { return };
        let size = match d.side {
            Side::Left => d.size + (p.x - d.start.x),
            Side::Right => d.size - (p.x - d.start.x),
            Side::Bottom => d.size - (p.y - d.start.y),
        };
        self.part_mut(d.side).size = size.max(4);
    }

    pub fn end_drag(&mut self) {
        self.drag = None;
    }

    /// The part's frame and its tabs (the current one lit), drawn like
    /// the style's own tool windows: Borland's cyan Help and Watches
    /// windows, QBasic's Immediate window, VB-DOS's grey Project window.
    pub fn draw_frame(&self, c: &mut Canvas, theme: &Theme, side: Side, r: Rect, focused: bool, title: &dyn Fn(Tab) -> String) {
        let s = &theme.scheme;
        let st = &theme.style;
        c.fill(r, " ", s.panel);
        // (VB-DOS's tool windows have a single black frame, not a title bar.)
        let fc = if st.title_bar { turbine_ui::theme::SINGLE } else if focused { st.active_frame } else { st.inactive_frame };
        let fs = if focused && !st.title_bar { Style { fg: s.frame_active.fg, ..s.panel_frame } } else { s.panel_frame };
        draw_frame(c, r, fc, fs);
        let part = self.part(side);
        for (t, tr, label) in self.tab_rects(side, r, title) {
            let on = part.current() == Some(t);
            if !on {
                c.print(tr.x, tr.y, &format!(" {label} "), fs);
                continue;
            }
            // The current tab: the style's title colours (VB-DOS's magenta
            // bar, QBasic's reverse title, Borland's green), a pill with
            // rounded ends when the style has icons.
            let pill = if st.title_bar {
                if focused { s.title_active } else { s.title_inactive }
            } else if st.title_reverse {
                s.frame_icons
            } else {
                s.menu_selected
            };
            // (As the originals title a window: a box in reverse, no
            // rounded ends.)
            c.print(tr.x, tr.y, &format!(" {label} "), pill);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_share_the_room_and_leave_some_for_the_editors() {
        let mut d = Dock::default();
        d.right.tabs.push(Tab::Pane(7));
        d.right.visible = true;
        d.bottom.visible = true;
        let area = Rect::new(0, 1, 120, 40);
        let l = d.layout(area);
        let (left, right, bottom) = (l.left.unwrap(), l.right.unwrap(), l.bottom.unwrap());
        assert_eq!((left.x, left.w, right.right(), right.w), (0, 30, 120, 46));
        assert_eq!((bottom.x, bottom.w, bottom.bottom()), (29, 120 - 30 - 46 + 1, 41));
        // (The tabs are in the editors' top border, under the breadcrumbs.)
        assert_eq!((l.breadcrumbs.unwrap().y, l.editor_tabs.unwrap().y), (1, 2));
        assert_eq!((l.editors.y, l.editors.bottom()), (2, bottom.y + 1));
        // A narrow screen: the side bars give way.
        let l = d.layout(Rect::new(0, 1, 60, 20));
        assert!(l.editors.w >= 24, "{l:?}");
    }

    #[test]
    fn dragging_an_edge_resizes_its_part() {
        let mut d = Dock::default();
        let area = Rect::new(0, 1, 120, 40);
        let l = d.layout(area);
        let edge = Point::new(l.left.unwrap().right() - 1, 10);
        assert!(d.begin_drag(&l, edge));
        d.drag_to(Point::new(edge.x + 8, 12));
        d.end_drag();
        assert_eq!(d.left.size, 38);
    }
}
