//! A scrolling list for side bars and panels (Explorer, Outline, Search
//! results, Problems, Source Control): rows with an indent, an icon, text
//! and a right-aligned detail; keyboard and mouse selection; a draggable
//! scroll bar; type-ahead.

use crate::automation::Node;
use crate::event::{Button, Key, KeyEvent, MouseEvent, MouseKind};
use crate::frame::draw_vscroll;
use crate::geom::{Point, Rect};
use crate::surface::{Canvas, CursorShape};
use crate::theme::{Style, Theme};
use std::time::{Duration, Instant};
use turbine_text::unicode::str_width;

/// One row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    pub text: String,
    /// Levels of nesting (two cells each).
    pub indent: u16,
    /// A one-cell mark before the text (▸ ▾ for folders, a status letter).
    pub icon: String,
    /// Shown dimmed at the right (a size, a line number, a path).
    pub detail: String,
    /// Text colour instead of the list's (errors red, untracked green, ...).
    pub fg: Option<crate::theme::Color>,
    /// A heading row (bold, not selectable by type-ahead).
    pub heading: bool,
}

impl Row {
    pub fn new(text: impl Into<String>) -> Row {
        Row { text: text.into(), ..Default::default() }
    }
    pub fn indent(mut self, n: u16) -> Row {
        self.indent = n;
        self
    }
    pub fn icon(mut self, s: impl Into<String>) -> Row {
        self.icon = s.into();
        self
    }
    pub fn detail(mut self, s: impl Into<String>) -> Row {
        self.detail = s.into();
        self
    }
    pub fn fg(mut self, c: crate::theme::Color) -> Row {
        self.fg = Some(c);
        self
    }
    pub fn heading(mut self) -> Row {
        self.heading = true;
        self
    }
}

/// What a key or click did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListOut {
    /// Not for the list.
    Ignored,
    /// Used (scrolled, nothing else).
    Consumed,
    /// The selection moved to this row.
    Selected(usize),
    /// Enter or a double-click on this row.
    Activated(usize),
    /// Left / Right on this row (collapse / expand a tree).
    Collapse(usize),
    Expand(usize),
}

#[derive(Debug, Default)]
pub struct ListView {
    pub rows: Vec<Row>,
    pub selected: usize,
    pub scroll: usize,
    /// Where it was drawn last (clicks use it).
    pub area: Rect,
    thumb: bool,
    last_click: Option<(Point, Instant)>,
    typed: String,
    typed_at: Option<Instant>,
}

impl ListView {
    pub fn new() -> ListView {
        ListView::default()
    }

    /// Replace the rows, keeping the selection where it was (by text when
    /// it's still there, else by position).
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        let was = self.rows.get(self.selected).map(|r| (r.text.clone(), r.indent));
        self.rows = rows;
        if let Some((t, ind)) = was {
            if let Some(i) = self.rows.iter().position(|r| r.text == t && r.indent == ind) {
                self.selected = i;
            }
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(1));
    }

    fn visible(&self) -> usize {
        self.area.h.max(1) as usize
    }

    /// Keep the selected row in view.
    pub fn reveal(&mut self) {
        let n = self.visible();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + n {
            self.scroll = self.selected + 1 - n;
        }
    }

    pub fn select(&mut self, i: usize) {
        if !self.rows.is_empty() {
            self.selected = i.min(self.rows.len() - 1);
            self.reveal();
        }
    }

    pub fn key(&mut self, k: &KeyEvent) -> ListOut {
        if self.rows.is_empty() {
            return ListOut::Ignored;
        }
        let n = self.visible();
        let last = self.rows.len() - 1;
        let to = match k.key {
            Key::Up => self.selected.saturating_sub(1),
            Key::Down => (self.selected + 1).min(last),
            Key::PageUp => self.selected.saturating_sub(n.saturating_sub(1).max(1)),
            Key::PageDown => (self.selected + n.saturating_sub(1).max(1)).min(last),
            Key::Home => 0,
            Key::End => last,
            Key::Enter => return ListOut::Activated(self.selected),
            Key::Left => return ListOut::Collapse(self.selected),
            Key::Right => return ListOut::Expand(self.selected),
            Key::Char(c) if !k.mods.ctrl && !k.mods.alt && !c.is_control() => {
                // Type-ahead: the next row starting with what's been typed.
                if self.typed_at.is_none_or(|t| t.elapsed() > Duration::from_millis(900)) {
                    self.typed.clear();
                }
                self.typed.push(c.to_ascii_lowercase());
                self.typed_at = Some(Instant::now());
                let start = if self.typed.len() == 1 { self.selected + 1 } else { self.selected };
                let hit = (0..self.rows.len())
                    .map(|o| (start + o) % self.rows.len())
                    .find(|&i| !self.rows[i].heading && self.rows[i].text.to_lowercase().starts_with(&self.typed));
                match hit {
                    Some(i) => i,
                    None => return ListOut::Consumed,
                }
            }
            _ => return ListOut::Ignored,
        };
        self.selected = to;
        self.reveal();
        ListOut::Selected(to)
    }

    /// Scroll so the thumb sits at screen row `y`.
    fn thumb_to(&mut self, y: i32) {
        let n = self.visible();
        let max = self.rows.len().saturating_sub(n);
        let track = (self.area.h - 3).max(0);
        let at = (y - self.area.y - 1).clamp(0, track);
        self.scroll = if track == 0 { 0 } else { (at as usize * max + track as usize / 2) / track as usize }.min(max);
    }

    pub fn mouse(&mut self, m: &MouseEvent) -> ListOut {
        let r = self.area;
        if self.thumb {
            match m.kind {
                MouseKind::Drag(_) => {
                    self.thumb_to(m.pos.y);
                    return ListOut::Consumed;
                }
                _ => self.thumb = false,
            }
        }
        if !r.contains(m.pos) {
            return ListOut::Ignored;
        }
        let n = self.visible();
        let max = self.rows.len().saturating_sub(n);
        match m.kind {
            MouseKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(3);
                ListOut::Consumed
            }
            MouseKind::ScrollDown => {
                self.scroll = (self.scroll + 3).min(max);
                ListOut::Consumed
            }
            MouseKind::Down(Button::Left) => {
                // The scroll bar: arrows step, the track jumps and drags.
                if self.rows.len() > n && m.pos.x == r.right() - 1 {
                    if m.pos.y == r.y {
                        self.scroll = self.scroll.saturating_sub(1);
                    } else if m.pos.y == r.bottom() - 1 {
                        self.scroll = (self.scroll + 1).min(max);
                    } else {
                        self.thumb = true;
                        self.thumb_to(m.pos.y);
                    }
                    return ListOut::Consumed;
                }
                let i = self.scroll + (m.pos.y - r.y) as usize;
                if i >= self.rows.len() {
                    return ListOut::Consumed;
                }
                let double = self.last_click.is_some_and(|(p, t)| p.y == m.pos.y && t.elapsed() < Duration::from_millis(450));
                self.last_click = Some((m.pos, Instant::now()));
                self.selected = i;
                if double {
                    self.last_click = None;
                    return ListOut::Activated(i);
                }
                // (A click on a folder's ▸ toggles it.)
                let row = &self.rows[i];
                let icon_x = r.x + 1 + row.indent as i32 * 2;
                if !row.icon.is_empty() && m.pos.x == icon_x {
                    return ListOut::Activated(i);
                }
                ListOut::Selected(i)
            }
            _ => ListOut::Consumed,
        }
    }

    pub fn draw(&mut self, c: &mut Canvas, r: Rect, theme: &Theme, focused: bool) {
        self.area = r;
        let s = &theme.scheme;
        let base = s.panel;
        c.fill(r, " ", base);
        let n = r.h.max(0) as usize;
        let max = self.rows.len().saturating_sub(n);
        self.scroll = self.scroll.min(max);
        let bar = self.rows.len() > n && n >= 3;
        let text_w = r.w - i32::from(bar);
        let dim = Style::new(s.panel_frame.fg, base.bg);
        for (k, row) in self.rows.iter().enumerate().skip(self.scroll).take(n) {
            let y = r.y + (k - self.scroll) as i32;
            let sel = k == self.selected;
            let stl = if sel && focused {
                s.menu_selected
            } else if sel {
                s.panel_selected
            } else {
                Style { fg: row.fg.unwrap_or(base.fg), ..base }
            };
            let stl = if row.heading { Style { attrs: crate::theme::Attrs { bold: true, ..stl.attrs }, ..stl } } else { stl };
            let line = Rect::new(r.x, y, text_w, 1);
            c.fill(line, " ", stl);
            let mut lc = c.sub(line);
            let mut x = r.x + 1 + row.indent as i32 * 2;
            if !row.icon.is_empty() {
                lc.print(x, y, &row.icon, stl);
                x += str_width(&row.icon) as i32 + 1;
            }
            // The name comes first: the detail gets what's left (cut at its
            // start, or dropped), never the name.
            let avail = (r.x + text_w - 1 - x).max(0);
            let name_w = str_width(&row.text) as i32;
            let mut detail = row.detail.clone();
            let room_d = avail - name_w - 2;
            if str_width(&detail) as i32 > room_d {
                if room_d >= 6 {
                    while !detail.is_empty() && str_width(&detail) as i32 > room_d - 1 {
                        detail.remove(0);
                    }
                    detail.insert(0, '…');
                } else {
                    detail.clear();
                }
            }
            let dw = str_width(&detail) as i32;
            let room = (avail - if dw > 0 { dw + 1 } else { 0 }).max(0);
            let mut t = row.text.clone();
            if str_width(&t) as i32 > room {
                while !t.is_empty() && str_width(&t) as i32 > (room - 1).max(0) {
                    t.pop();
                }
                t.push('…');
            }
            lc.print(x, y, &t, stl);
            if dw > 0 {
                let ds = if sel { stl } else { dim };
                lc.print(r.x + text_w - 1 - dw, y, &detail, ds);
            }
        }
        if bar {
            draw_vscroll(c, r.right() - 1, r.y, r.h, self.scroll, max + 1, s.scrollbar, theme.style.scroll);
        }
        if focused && !self.rows.is_empty() && self.selected >= self.scroll && self.selected < self.scroll + n {
            c.set_cursor(Point::new(r.x + 1, r.y + (self.selected - self.scroll) as i32), CursorShape::Underline);
        }
    }

    /// The rows as automation nodes (the ones in view, with their places).
    pub fn node(&self, id: &str, name: &str, focused: bool) -> Node {
        let r = self.area;
        let mut n = Node::new(id, "list", name, r).focused(focused);
        if let Some(row) = self.rows.get(self.selected) {
            n = n.value(row.text.clone());
        }
        for (k, row) in self.rows.iter().enumerate().skip(self.scroll).take(r.h.max(0) as usize) {
            let ir = Rect::new(r.x, r.y + (k - self.scroll) as i32, r.w, 1);
            let mut it = Node::new(format!("{id}.{k}"), "listitem", row.text.clone(), ir).focused(k == self.selected);
            if !row.detail.is_empty() {
                it = it.value(row.detail.clone());
            }
            n = n.child(it);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Mods;

    fn list(n: usize) -> ListView {
        let mut l = ListView::new();
        l.set_rows((0..n).map(|i| Row::new(format!("row{i}"))).collect());
        l.area = Rect::new(0, 0, 20, 5);
        l
    }

    #[test]
    fn keys_move_and_keep_the_selection_in_view() {
        let mut l = list(30);
        assert_eq!(l.key(&KeyEvent::plain(Key::End)), ListOut::Selected(29));
        assert_eq!(l.scroll, 25);
        assert_eq!(l.key(&KeyEvent::plain(Key::PageUp)), ListOut::Selected(25));
        l.key(&KeyEvent::plain(Key::Home));
        assert_eq!(l.key(&KeyEvent::plain(Key::Char('r'))), ListOut::Selected(1));
        assert_eq!(l.key(&KeyEvent::plain(Key::Enter)), ListOut::Activated(1));
    }

    #[test]
    fn clicks_select_double_clicks_activate_and_the_thumb_drags() {
        let mut l = list(30);
        let m = |kind, x, y| MouseEvent { kind, pos: Point::new(x, y), mods: Mods::NONE };
        assert_eq!(l.mouse(&m(MouseKind::Down(Button::Left), 3, 2)), ListOut::Selected(2));
        assert_eq!(l.mouse(&m(MouseKind::Down(Button::Left), 3, 2)), ListOut::Activated(2));
        l.mouse(&m(MouseKind::Down(Button::Left), 19, 1));
        l.mouse(&m(MouseKind::Drag(Button::Left), 19, 9));
        l.mouse(&m(MouseKind::Up(Button::Left), 19, 9));
        assert_eq!(l.scroll, 25);
    }

    #[test]
    fn new_rows_keep_the_selected_one() {
        let mut l = list(5);
        l.select(3);
        l.set_rows(vec![Row::new("new"), Row::new("row3"), Row::new("row4")]);
        assert_eq!(l.selected, 1);
    }
}
