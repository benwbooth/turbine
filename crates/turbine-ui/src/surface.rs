//! The cell grid every front end draws.

use crate::geom::{Point, Rect};
use crate::theme::{Color, Style, vga};
use turbine_text::unicode::grapheme_width;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// One grapheme cluster; empty for the right half of a wide character.
    pub text: String,
    pub style: Style,
}

impl Cell {
    pub fn is_continuation(&self) -> bool {
        self.text.is_empty()
    }
}

impl Default for Cell {
    fn default() -> Self {
        Cell { text: " ".into(), style: Style::new(vga::LIGHT_GRAY, vga::BLACK) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Underline,
    Block,
    Bar,
}

#[derive(Clone, Debug)]
pub struct Surface {
    pub w: i32,
    pub h: i32,
    pub cells: Vec<Cell>,
    pub cursor: Option<(Point, CursorShape)>,
    /// Whether the cursor blinks (a setting).
    pub cursor_blink: bool,
    /// Pictures over the cells (an image viewer): where, and the file.
    /// The app draws them; a terminal with kitty's graphics protocol too.
    pub images: Vec<(Rect, std::path::PathBuf)>,
}

impl Surface {
    pub fn new(w: i32, h: i32) -> Self {
        let w = w.max(1);
        let h = h.max(1);
        Surface { w, h, cells: vec![Cell::default(); (w * h) as usize], cursor: None, cursor_blink: true, images: Vec::new() }
    }

    pub fn rect(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }

    pub fn get(&self, x: i32, y: i32) -> Option<&Cell> {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return None;
        }
        self.cells.get((y * self.w + x) as usize)
    }

    pub fn get_mut(&mut self, x: i32, y: i32) -> Option<&mut Cell> {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return None;
        }
        self.cells.get_mut((y * self.w + x) as usize)
    }

    pub fn canvas(&mut self) -> Canvas<'_> {
        let clip = self.rect();
        Canvas { surf: self, clip }
    }

    /// The screen as plain text, one line per row (for tests and agents).
    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity((self.w * self.h + self.h) as usize);
        for y in 0..self.h {
            let mut line = String::new();
            for x in 0..self.w {
                line.push_str(&self.cells[(y * self.w + x) as usize].text);
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

/// A clipped view onto a surface. Coordinates are absolute.
pub struct Canvas<'a> {
    surf: &'a mut Surface,
    pub clip: Rect,
}

impl<'a> Canvas<'a> {
    /// A canvas clipped further to `r`.
    pub fn sub(&mut self, r: Rect) -> Canvas<'_> {
        let clip = self.clip.intersect(&r);
        Canvas { surf: self.surf, clip }
    }

    /// The style already drawn at a cell (to draw over it in its colours).
    pub fn style_at(&self, x: i32, y: i32) -> Option<Style> {
        self.surf.get(x, y).map(|c| c.style)
    }

    fn put(&mut self, x: i32, y: i32, g: &str, w: i32, style: Style) {
        if !self.clip.contains(Point::new(x, y)) {
            return;
        }
        // A wide grapheme that doesn't fit in the clip becomes a space.
        if w == 2 && !self.clip.contains(Point::new(x + 1, y)) {
            if let Some(c) = self.surf.get_mut(x, y) {
                *c = Cell { text: " ".into(), style };
            }
            return;
        }
        // Writing over half of a wide char clears the other half.
        if let Some(c) = self.surf.get(x, y) {
            if c.is_continuation() {
                if let Some(l) = self.surf.get_mut(x - 1, y) {
                    l.text = " ".into();
                }
            }
        }
        if let Some(c) = self.surf.get_mut(x, y) {
            c.text.clear();
            c.text.push_str(g);
            c.style = style;
        }
        if w == 2 {
            if let Some(c) = self.surf.get_mut(x + 1, y) {
                c.text.clear();
                c.style = style;
            }
        } else if let Some(n) = self.surf.get(x + 1, y) {
            if n.is_continuation() {
                let n = self.surf.get_mut(x + 1, y).unwrap();
                n.text = " ".into();
            }
        }
    }

    /// Print text; returns the column after it. Tabs and control
    /// characters show as visible symbols, never move the cursor.
    pub fn print(&mut self, x: i32, y: i32, text: &str, style: Style) -> i32 {
        let mut cx = x;
        for g in text.graphemes(true) {
            let w = grapheme_width(g) as i32;
            if w == 0 {
                continue;
            }
            let shown = if g.chars().next().is_some_and(|c| c.is_control()) { "·" } else { g };
            self.put(cx, y, shown, w, style);
            cx += w;
            if cx >= self.clip.right() {
                break;
            }
        }
        cx
    }

    /// Print text where `~x~` marks a hotkey letter drawn in `hot`.
    pub fn print_hot(&mut self, x: i32, y: i32, text: &str, style: Style, hot: Style) -> i32 {
        let mut cx = x;
        let mut in_hot = false;
        for part in text.split('~') {
            cx = self.print(cx, y, part, if in_hot { hot } else { style });
            in_hot = !in_hot;
        }
        cx
    }

    pub fn fill(&mut self, r: Rect, g: &str, style: Style) {
        let r = r.intersect(&self.clip);
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                self.put(x, y, g, 1, style);
            }
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, g: &str, style: Style) {
        self.fill(Rect::new(x, y, w, 1), g, style);
    }

    pub fn vline(&mut self, x: i32, y: i32, h: i32, g: &str, style: Style) {
        self.fill(Rect::new(x, y, 1, h), g, style);
    }

    /// Recolour cells without changing their text (shadows, selection).
    pub fn restyle(&mut self, r: Rect, f: impl Fn(Style) -> Style) {
        let r = r.intersect(&self.clip);
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                if let Some(c) = self.surf.get_mut(x, y) {
                    c.style = f(c.style);
                }
            }
        }
    }

    /// The classic drop shadow: two columns right, one row below.
    pub fn shadow(&mut self, r: Rect, style: Style) {
        let f = |s: Style| Style { fg: style.fg, bg: style.bg, ..s };
        self.restyle(Rect::new(r.right(), r.y + 1, 2, r.h), f);
        self.restyle(Rect::new(r.x + 2, r.bottom(), r.w, 1), f);
    }

    pub fn set_cursor(&mut self, p: Point, shape: CursorShape) {
        if self.clip.contains(p) {
            self.surf.cursor = Some((p, shape));
        }
    }

    pub fn set_style_at(&mut self, p: Point, style: Style) {
        if self.clip.contains(p) {
            if let Some(c) = self.surf.get_mut(p.x, p.y) {
                c.style = style;
            }
        }
    }

    pub fn color_at(&self, p: Point) -> Option<(Color, Color)> {
        self.surf.get(p.x, p.y).map(|c| (c.style.fg, c.style.bg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_chars_take_two_cells() {
        let mut s = Surface::new(10, 1);
        let st = Style::new(vga::WHITE, vga::BLACK);
        let end = s.canvas().print(0, 0, "日本x", st);
        assert_eq!(end, 5);
        assert_eq!(s.to_text(), "日本x\n");
        s.canvas().print(1, 0, "a", st);
        assert_eq!(s.to_text(), " a本x\n");
    }
}
