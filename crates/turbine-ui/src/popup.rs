//! A drop-down list next to the cursor (completion, quick picks).

use crate::event::{Button, Key, KeyEvent, MouseEvent, MouseKind};
use crate::frame::draw_frame;
use crate::geom::{Point, Rect};
use crate::surface::Canvas;
use crate::theme::Theme;
use turbine_text::unicode::str_width;

#[derive(Debug, PartialEq, Eq)]
pub enum PopupResult {
    /// Not the popup's key.
    Ignored,
    Consumed,
    Accept(usize),
    Close,
}

#[derive(Debug, Default)]
pub struct ListPopup {
    /// (label, detail)
    pub items: Vec<(String, String)>,
    pub sel: usize,
    scroll: usize,
    rect: Rect,
    pub max_rows: usize,
}

impl ListPopup {
    pub fn new(items: Vec<(String, String)>) -> Self {
        ListPopup { items, sel: 0, scroll: 0, rect: Rect::default(), max_rows: 8 }
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Item `i`'s row on screen (for agents), if shown.
    pub fn item_rect(&self, i: usize) -> Option<Rect> {
        let rows = self.rect.h - 2;
        (i >= self.scroll && i < self.scroll + rows.max(0) as usize)
            .then(|| Rect::new(self.rect.x + 1, self.rect.y + 1 + (i - self.scroll) as i32, self.rect.w - 2, 1))
    }

    pub fn handle_key(&mut self, ev: &KeyEvent) -> PopupResult {
        let n = self.items.len();
        if n == 0 {
            return PopupResult::Close;
        }
        let page = self.max_rows.max(1);
        match ev.key {
            Key::Up => self.sel = (self.sel + n - 1) % n,
            Key::Down => self.sel = (self.sel + 1) % n,
            Key::PageUp => self.sel = self.sel.saturating_sub(page),
            Key::PageDown => self.sel = (self.sel + page).min(n - 1),
            Key::Enter | Key::Tab => return PopupResult::Accept(self.sel),
            Key::Esc => return PopupResult::Close,
            _ => return PopupResult::Ignored,
        }
        PopupResult::Consumed
    }

    pub fn handle_mouse(&mut self, ev: &MouseEvent) -> PopupResult {
        if !self.rect.contains(ev.pos) {
            return if matches!(ev.kind, MouseKind::Down(_)) { PopupResult::Close } else { PopupResult::Ignored };
        }
        match ev.kind {
            MouseKind::Down(Button::Left) => {
                for i in 0..self.items.len() {
                    if self.item_rect(i).is_some_and(|r| r.contains(ev.pos)) {
                        return PopupResult::Accept(i);
                    }
                }
                PopupResult::Consumed
            }
            MouseKind::ScrollUp => {
                self.sel = self.sel.saturating_sub(1);
                PopupResult::Consumed
            }
            MouseKind::ScrollDown => {
                self.sel = (self.sel + 1).min(self.items.len().saturating_sub(1));
                PopupResult::Consumed
            }
            _ => PopupResult::Consumed,
        }
    }

    /// Draw below `at` (or above it when there's no room), inside `screen`.
    pub fn draw(&mut self, c: &mut Canvas, at: Point, screen: Rect, theme: &Theme) {
        let s = &theme.scheme;
        // The side with more room, and only as many rows as fit there.
        let room_below = screen.bottom() - (at.y + 1) - 2;
        let room_above = at.y - screen.y - 2;
        let below = room_below >= self.items.len().min(self.max_rows) as i32 || room_below >= room_above;
        let room = if below { room_below } else { room_above }.max(1);
        let rows = (self.items.len().min(self.max_rows) as i32).min(room);
        let label_w = self.items.iter().map(|(l, _)| str_width(l)).max().unwrap_or(0) as i32;
        let detail_w = self.items.iter().map(|(_, d)| str_width(d)).max().unwrap_or(0).min(30) as i32;
        let w = (label_w + if detail_w > 0 { detail_w + 2 } else { 0 } + 4).clamp(12, screen.w - 2);
        let h = rows + 2;
        let y = if below { at.y + 1 } else { (at.y - h).max(screen.y) };
        let x = (at.x - 1).min(screen.right() - w - 2).max(screen.x);
        self.rect = Rect::new(x, y, w, h);
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + rows as usize {
            self.scroll = self.sel + 1 - rows as usize;
        }
        c.fill(self.rect, " ", s.menu);
        draw_frame(c, self.rect, theme.style.menu_frame, s.menu);
        if theme.style.shadows {
            c.shadow(self.rect, s.shadow);
        }
        for (i, (label, detail)) in self.items.iter().enumerate().skip(self.scroll).take(rows as usize) {
            let r = Rect::new(x + 1, y + 1 + (i - self.scroll) as i32, w - 2, 1);
            let st = if i == self.sel { s.menu_selected } else { s.menu };
            let mut sub = c.sub(r);
            sub.fill(r, " ", st);
            sub.print(r.x + 1, r.y, label, st);
            if !detail.is_empty() {
                let dw = str_width(detail) as i32;
                let dx = (r.right() - dw - 1).max(r.x + label_w + 3);
                sub.print(dx, r.y, detail, if i == self.sel { st } else { s.menu_disabled });
            }
        }
        if self.items.len() > rows as usize {
            crate::frame::draw_vscroll(c, self.rect.right() - 1, y + 1, rows, self.sel, self.items.len(), s.menu);
        }
    }
}
