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

/// Title centred in the top edge: "═ title ═".
pub fn draw_title(c: &mut Canvas, r: Rect, title: &str, style: Style) {
    let room = r.w - 12;
    if room <= 2 {
        return;
    }
    let mut t: String = title.to_string();
    while str_width(&t) as i32 > room {
        t.remove(0);
    }
    let text = format!(" {t} ");
    let w = str_width(&text) as i32;
    c.print(r.x + (r.w - w) / 2, r.y, &text, style);
}

/// Vertical scroll bar: ↑ track ■ ↓.
pub fn draw_vscroll(c: &mut Canvas, x: i32, y: i32, h: i32, pos: usize, total: usize, style: Style) {
    if h < 3 {
        return;
    }
    c.print(x, y, "▲", style);
    c.print(x, y + h - 1, "▼", style);
    c.vline(x, y + 1, h - 2, "▒", style);
    let track = (h - 3).max(0) as usize;
    let thumb = if total <= 1 { 0 } else { pos.min(total - 1) * track / (total - 1) };
    c.print(x, y + 1 + thumb as i32, "■", style);
}

/// Horizontal scroll bar: ◄ track ■ ►.
pub fn draw_hscroll(c: &mut Canvas, x: i32, y: i32, w: i32, pos: usize, total: usize, style: Style) {
    if w < 3 {
        return;
    }
    c.print(x, y, "◄", style);
    c.print(x + w - 1, y, "►", style);
    c.hline(x + 1, y, w - 2, "▒", style);
    let track = (w - 3).max(0) as usize;
    let thumb = if total <= 1 { 0 } else { pos.min(total - 1) * track / (total - 1) };
    c.print(x + 1 + thumb as i32, y, "■", style);
}
