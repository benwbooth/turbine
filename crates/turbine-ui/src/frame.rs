//! Frames, title bars and scroll bars.

use crate::geom::Rect;
use crate::surface::Canvas;
use crate::theme::{FrameChars, Style};
use turbine_text::unicode::str_width;

pub fn draw_frame(c: &mut Canvas, r: Rect, fc: FrameChars, style: Style) {
    if r.w < 2 || r.h < 2 {
        return;
    }
    c.print(r.x, r.y, fc.tl, style);
    c.print(r.right() - 1, r.y, fc.tr, style);
    c.print(r.x, r.bottom() - 1, fc.bl, style);
    c.print(r.right() - 1, r.bottom() - 1, fc.br, style);
    c.hline(r.x + 1, r.y, r.w - 2, fc.h, style);
    c.hline(r.x + 1, r.bottom() - 1, r.w - 2, fc.h, style);
    c.vline(r.x, r.y + 1, r.h - 2, fc.v, style);
    c.vline(r.right() - 1, r.y + 1, r.h - 2, fc.v, style);
}

/// Title centred in the top edge: "═ title ═", kept clear of the close
/// icon on the left and the window number and zoom icon on the right; a
/// long one loses its start ("…ocument.rs").
pub fn draw_title(c: &mut Canvas, r: Rect, title: &str, style: Style) {
    draw_title_in(c, r, title, style, 10);
}

/// The same, leaving `right_room` cells free at the right (zoom box, number).
pub fn draw_title_in(c: &mut Canvas, r: Rect, title: &str, style: Style, right_room: i32) {
    let (left, right) = (r.x + 6, r.right() - right_room);
    let room = right - left;
    if room <= 3 {
        return;
    }
    let mut t: String = title.to_string();
    if str_width(&t) as i32 + 2 > room {
        while str_width(&t) as i32 + 3 > room {
            t.remove(0);
        }
        t.insert(0, '…');
    }
    let text = format!(" {t} ");
    let w = str_width(&text) as i32;
    let x = (r.x + (r.w - w) / 2).clamp(left, (right - w).max(left));
    c.print(x, r.y, &text, style);
}

/// The default scroll bar characters: up, down, left, right, track, thumb.
pub const SCROLL: [&str; 6] = ["▲", "▼", "◄", "►", "▒", "■"];

/// Vertical scroll bar: ↑ track ■ ↓ (characters from the control style).
pub fn draw_vscroll(c: &mut Canvas, x: i32, y: i32, h: i32, pos: usize, total: usize, style: Style, ch: [&str; 6]) {
    if h < 3 {
        return;
    }
    c.print(x, y, ch[0], style);
    c.print(x, y + h - 1, ch[1], style);
    c.vline(x, y + 1, h - 2, ch[4], style);
    let track = (h - 3).max(0) as usize;
    let thumb = if total <= 1 { 0 } else { pos.min(total - 1) * track / (total - 1) };
    c.print(x, y + 1 + thumb as i32, ch[5], style);
}

/// Horizontal scroll bar: ← track ■ →.
pub fn draw_hscroll(c: &mut Canvas, x: i32, y: i32, w: i32, pos: usize, total: usize, style: Style, ch: [&str; 6]) {
    if w < 3 {
        return;
    }
    c.print(x, y, ch[2], style);
    c.print(x + w - 1, y, ch[3], style);
    c.hline(x + 1, y, w - 2, ch[4], style);
    let track = (w - 3).max(0) as usize;
    let thumb = if total <= 1 { 0 } else { pos.min(total - 1) * track / (total - 1) };
    c.print(x + 1 + thumb as i32, y, ch[5], style);
}
