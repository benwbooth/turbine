//! Word wrap (View > Word Wrap): a long line takes as many rows as it
//! needs at the window's width. Rows, the cursor's place on screen,
//! Up and Down (a row at a time) and clicks follow the wrapped rows.

use super::*;

impl EditorView {
    /// The columns a wrapped row holds (0: not wrapping).
    pub(super) fn wrap_width(&self) -> usize {
        if self.wrap { (self.area.w - self.gutter_width() - 1).max(10) as usize } else { 0 }
    }

    /// Rows a line takes when wrapped at `w`.
    pub(super) fn line_rows(&self, line: usize, w: usize) -> usize {
        let text = self.doc.line(line);
        let (_, total) = self.layout_line(line, &text);
        total / w.max(1) + 1
    }

    /// Where (line, column) is on screen: (row from the top line, column).
    pub(super) fn screen_place(&self, line: usize, col: usize) -> (i32, usize) {
        let w = self.wrap_width();
        if w == 0 {
            return (self.rows_between(self.scroll_y, line), col.saturating_sub(self.scroll_x));
        }
        (self.rows_between(self.scroll_y, line) + (col / w) as i32, col % w)
    }

    /// Up or Down by a row when wrapping: Some(target char), or None to
    /// move by lines.
    pub(super) fn wrapped_vertical(&self, delta: isize, col: usize) -> Option<usize> {
        let w = self.wrap_width();
        if w == 0 || delta.unsigned_abs() != 1 {
            return None;
        }
        let (line, cur) = self.cursor_line_col();
        let rows = self.line_rows(line, w);
        let sub = cur / w;
        let pref = col % w;
        let (to_line, to_col) = if delta > 0 {
            if sub + 1 < rows {
                (line, (sub + 1) * w + pref)
            } else {
                let next = self.step_visible(line, 1);
                if next == line {
                    return None;
                }
                (next, pref)
            }
        } else if sub > 0 {
            (line, (sub - 1) * w + pref)
        } else {
            let prev = self.step_visible(line, -1);
            if prev == line {
                return None;
            }
            let last = self.line_rows(prev, w) - 1;
            (prev, last * w + pref)
        };
        let byte = self.byte_at_col(to_line, to_col);
        Some(self.doc.line_byte_to_char(to_line, byte))
    }

    /// The char at a screen row and column when wrapping.
    pub(super) fn wrapped_pos(&self, row: i32, x: i32) -> Option<usize> {
        let w = self.wrap_width();
        if w == 0 {
            return None;
        }
        let mut left = row.max(0) as usize;
        let mut line = self.scroll_y;
        let last = self.doc.len_lines().saturating_sub(1);
        loop {
            let rows = self.line_rows(line, w);
            if left < rows || line >= last {
                let col = left.min(rows - 1) * w + x.max(0) as usize;
                let byte = self.byte_at_col(line, col);
                return Some(self.doc.line_byte_to_char(line, byte));
            }
            left -= rows;
            let next = self.step_visible(line, 1);
            if next == line {
                return Some(self.doc.rope.len_chars());
            }
            line = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Key, KeyEvent, Mods};

    #[test]
    fn long_lines_wrap() {
        let mut ed = EditorView::new(Document::from_str(&format!("{}\nshort\n", "abcdefghij".repeat(3))));
        ed.wrap = true;
        ed.line_numbers = false;
        let mut surf = crate::Surface::new(16, 6);
        let theme = Theme::default();
        ed.draw(&mut surf.canvas(), Rect::new(0, 0, 16, 6), &theme, true, true);
        let t = surf.to_text();
        let rows: Vec<&str> = t.lines().collect();
        // 15 columns a row (one kept at the end: ↩ where a line goes on).
        assert_eq!(rows[0].trim_end(), "abcdefghijabcde↩");
        assert_eq!(rows[1].trim_end().trim_end_matches('↩'), "fghijabcdefghij");
        assert_eq!(rows[3].trim_end(), "short");
        // Down goes a row at a time; a click finds its char.
        let mut clip = String::new();
        ed.handle_key(&KeyEvent::new(Key::Down, Mods::NONE), &mut clip);
        assert_eq!(ed.cursor_line_col(), (0, 15));
        assert_eq!(ed.cursor_screen_pos(), Some(crate::geom::Point::new(0, 1)));
        assert_eq!(ed.wrapped_pos(3, 2), Some(31 + 2));
    }
}
