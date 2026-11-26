//! The bottom status line: key hints on the left, info on the right.

use crate::geom::{Point, Rect};
use crate::surface::Canvas;
use crate::theme::Theme;
use turbine_text::unicode::str_width;

#[derive(Clone, Debug)]
pub struct StatusItem {
    pub key: String,
    pub label: String,
    pub command: String,
}

impl StatusItem {
    pub fn new(key: &str, label: &str, command: &str) -> Self {
        StatusItem { key: key.into(), label: label.into(), command: command.into() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct StatusBar {
    pub items: Vec<StatusItem>,
    /// Shown on the right (cursor position, encoding, ...).
    pub info: String,
    /// When set, replaces the key hints (messages, agent activity).
    pub message: Option<String>,
}

impl StatusBar {
    pub fn item_rects(&self, y: i32) -> Vec<Rect> {
        let mut x = 1;
        self.items
            .iter()
            .map(|it| {
                let w = str_width(&it.key) as i32 + 1 + str_width(&it.label) as i32;
                let r = Rect::new(x, y, w, 1);
                x += w + 2;
                r
            })
            .collect()
    }

    pub fn hit(&self, p: Point, y: i32) -> Option<String> {
        if self.message.is_some() {
            return None;
        }
        self.item_rects(y).iter().zip(&self.items).find(|(r, _)| r.contains(p)).map(|(_, it)| it.command.clone())
    }

    pub fn draw(&self, c: &mut Canvas, theme: &Theme, r: Rect) {
        let s = &theme.scheme;
        c.fill(r, " ", s.status);
        let w = str_width(&self.info) as i32;
        let left = Rect::new(r.x, r.y, (r.w - w - 2).max(0), 1);
        let mut lc = c.sub(left);
        if let Some(m) = &self.message {
            lc.print(r.x + 1, r.y, m, s.status);
        } else {
            for (it, ir) in self.items.iter().zip(self.item_rects(r.y)) {
                if ir.right() > left.right() {
                    break;
                }
                let x = lc.print(ir.x, ir.y, &it.key, s.status_key);
                lc.print(x + 1, ir.y, &it.label, s.status);
            }
        }
        if w > 0 {
            c.print(r.right() - w - 1, r.y, &self.info, s.status);
        }
    }
}
