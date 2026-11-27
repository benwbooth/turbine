//! The text editor view.

use crate::event::{Button, Key, KeyEvent, KeyKind, MouseEvent, MouseKind};
use crate::frame::{draw_hscroll, draw_vscroll};
use crate::geom::{Point, Rect};
use crate::surface::{Canvas, CursorShape};
use crate::theme::Theme;
use turbine_text::Document;
use turbine_text::unicode::{grapheme_boundaries, grapheme_width, visual_line};

pub const TAB_WIDTH: usize = 4;

/// One grapheme of a laid-out line, in visual order.
#[derive(Clone, Debug)]
struct Placed {
    offset: usize,
    len: usize,
    col: usize,
    width: usize,
    /// Char offset of the grapheme in the line.
    char_off: usize,
}

fn layout(line: &str, base_rtl: Option<bool>) -> (Vec<Placed>, usize) {
    // Code reads left to right unless the window is set to RTL, so the
    // paragraph direction is never guessed from the first strong letter.
    let vis = visual_line(line, Some(base_rtl.unwrap_or(false)));
    // char offset for each byte boundary we need
    let mut char_at = std::collections::HashMap::with_capacity(vis.len());
    let mut n = 0;
    for (i, _) in line.char_indices() {
        char_at.insert(i, n);
        n += 1;
    }
    let mut col = 0;
    let mut out = Vec::with_capacity(vis.len());
    for g in vis {
        let s = &line[g.offset..g.offset + g.len];
        let width = if s == "\t" { TAB_WIDTH - col % TAB_WIDTH } else { grapheme_width(s) };
        out.push(Placed { offset: g.offset, len: g.len, col, width, char_off: char_at[&g.offset] });
        col += width;
    }
    (out, col)
}

#[derive(Debug)]
pub struct EditorView {
    pub doc: Document,
    /// Cursor as a char offset into the document.
    pub cursor: usize,
    /// Selection anchor (char offset); selection is anchor..cursor.
    pub anchor: Option<usize>,
    pub scroll_y: usize,
    pub scroll_x: usize,
    pref_col: Option<usize>,
    pub line_numbers: bool,
    pub overwrite: bool,
    /// Right-to-left paragraphs (None/false = left to right).
    pub base_rtl: Option<bool>,
    /// AI completion shown as ghost text after the cursor; Tab accepts.
    pub ghost: Option<String>,
    /// Overrides the cursor shape (vim's normal mode: a block).
    pub cursor_shape: Option<CursorShape>,
    /// Syntax highlighting per line: (start byte, end byte, kind index),
    /// later spans over earlier. May lag an edit by a moment.
    pub highlights: Vec<Vec<(u32, u32, u8)>>,
    /// Extra selected char ranges (vim's visual block).
    pub block: Vec<(usize, usize)>,
    /// Text area from the last draw, for mouse hit testing.
    area: Rect,
    dragging: bool,
}

impl EditorView {
    pub fn new(doc: Document) -> Self {
        EditorView {
            doc,
            cursor: 0,
            anchor: None,
            scroll_y: 0,
            scroll_x: 0,
            pref_col: None,
            line_numbers: true,
            overwrite: false,
            base_rtl: None,
            ghost: None,
            cursor_shape: None,
            highlights: Vec::new(),
            block: Vec::new(),
            area: Rect::default(),
            dragging: false,
        }
    }

    /// (line, column) of the cursor, both from 0.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let (line, byte) = self.doc.char_to_line_byte(self.cursor);
        (line, self.col_of(line, byte))
    }

    fn col_of(&self, line: usize, byte: usize) -> usize {
        let text = self.doc.line(line);
        let (placed, total) = layout(&text, self.base_rtl);
        placed.iter().find(|p| p.offset == byte).map(|p| p.col).unwrap_or(total)
    }

    fn byte_at_col(&self, line: usize, col: usize) -> usize {
        let text = self.doc.line(line);
        let (placed, _) = layout(&text, self.base_rtl);
        let mut best: Option<&Placed> = None;
        for p in &placed {
            if p.col <= col && col < p.col + p.width {
                return if col - p.col > p.width / 2 && p.width > 1 { p.offset + p.len } else { p.offset };
            }
            if p.col <= col {
                best = Some(p);
            }
        }
        match best {
            Some(_) if col >= placed.iter().map(|p| p.col + p.width).max().unwrap_or(0) => text.len(),
            Some(p) => p.offset,
            None => 0,
        }
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        if a == self.cursor {
            return None;
        }
        Some((a.min(self.cursor), a.max(self.cursor)))
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|(a, b)| self.doc.rope.slice(a..b).to_string())
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((a, b)) = self.selection() {
            self.doc.remove(a, b, self.cursor);
            self.cursor = a;
            self.anchor = None;
            true
        } else {
            self.anchor = None;
            false
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        self.delete_selection();
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if self.overwrite && !text.contains('\n') {
            let (line, byte) = self.doc.char_to_line_byte(self.cursor);
            let lt = self.doc.line(line);
            if byte < lt.len() {
                let b = grapheme_boundaries(&lt);
                let next = b.iter().find(|&&x| x > byte).copied().unwrap_or(lt.len());
                let end = self.doc.line_byte_to_char(line, next);
                self.doc.remove(self.cursor, end, self.cursor);
            }
        }
        self.doc.insert(self.cursor, &text, self.cursor);
        self.cursor += text.chars().count();
        self.pref_col = None;
        self.ghost = None;
    }

    fn move_to(&mut self, ch: usize, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = ch.min(self.doc.rope.len_chars());
        self.doc.break_undo_group();
        self.ghost = None;
    }

    fn grapheme_step(&self, forward: bool) -> usize {
        let (line, byte) = self.doc.char_to_line_byte(self.cursor);
        let text = self.doc.line(line);
        let b = grapheme_boundaries(&text);
        if forward {
            match b.iter().find(|&&x| x > byte) {
                Some(&x) => self.doc.line_byte_to_char(line, x),
                None if line + 1 < self.doc.len_lines() => self.doc.rope.line_to_char(line + 1),
                None => self.cursor,
            }
        } else {
            match b.iter().rev().find(|&&x| x < byte) {
                Some(&x) => self.doc.line_byte_to_char(line, x),
                None if line > 0 => self.doc.line_byte_to_char(line - 1, usize::MAX),
                None => 0,
            }
        }
    }

    fn word_step(&self, forward: bool) -> usize {
        let rope = &self.doc.rope;
        let n = rope.len_chars();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut i = self.cursor;
        if forward {
            while i < n && is_word(rope.char(i)) {
                i += 1;
            }
            while i < n && !is_word(rope.char(i)) {
                i += 1;
            }
        } else {
            while i > 0 && !is_word(rope.char(i - 1)) {
                i -= 1;
            }
            while i > 0 && is_word(rope.char(i - 1)) {
                i -= 1;
            }
        }
        i
    }

    fn vertical(&mut self, delta: isize, select: bool) {
        let (line, col) = self.cursor_line_col();
        let col = *self.pref_col.get_or_insert(col);
        let last = self.doc.len_lines().saturating_sub(1) as isize;
        let target = (line as isize + delta).clamp(0, last) as usize;
        let byte = self.byte_at_col(target, col);
        let ch = self.doc.line_byte_to_char(target, byte);
        let pref = self.pref_col;
        self.move_to(ch, select);
        self.pref_col = pref;
    }

    /// Handle a key. Returns true if the editor used it.
    pub fn handle_key(&mut self, ev: &KeyEvent, clipboard: &mut String) -> bool {
        if ev.kind == KeyKind::Release {
            return false;
        }
        let m = ev.mods;
        let sel = m.shift;
        let page = self.area.h.max(1) as isize;
        match ev.key {
            Key::Char(c) if !m.ctrl && !m.alt && !m.meta => {
                self.insert_text(&c.to_string());
                return true;
            }
            _ => {}
        }
        if ev.is_ctrl('z') {
            if let Some(c) = self.doc.undo() {
                self.cursor = c.min(self.doc.rope.len_chars());
                self.anchor = None;
            }
        } else if ev.is_ctrl('y') {
            if let Some(c) = self.doc.redo() {
                self.cursor = c.min(self.doc.rope.len_chars());
                self.anchor = None;
            }
        } else if ev.is_ctrl('a') {
            self.anchor = Some(0);
            self.cursor = self.doc.rope.len_chars();
        } else if ev.is_ctrl('c') || (ev.key == Key::Insert && m.ctrl) {
            if let Some(t) = self.selected_text() {
                *clipboard = t;
            }
        } else if ev.is_ctrl('x') || (ev.key == Key::Delete && m.shift) {
            if let Some(t) = self.selected_text() {
                *clipboard = t;
                self.delete_selection();
            }
        } else if ev.is_ctrl('v') || (ev.key == Key::Insert && m.shift) {
            let t = clipboard.clone();
            self.insert_text(&t);
        } else {
            match ev.key {
                Key::Left if m.ctrl => self.move_to(self.word_step(false), sel),
                Key::Right if m.ctrl => self.move_to(self.word_step(true), sel),
                Key::Left => {
                    let t = if !sel && let Some((a, _)) = self.selection() { a } else { self.grapheme_step(false) };
                    self.move_to(t, sel)
                }
                Key::Right => {
                    let t = if !sel && let Some((_, b)) = self.selection() { b } else { self.grapheme_step(true) };
                    self.move_to(t, sel)
                }
                Key::Up => self.vertical(-1, sel),
                Key::Down => self.vertical(1, sel),
                Key::PageUp => {
                    self.scroll_y = self.scroll_y.saturating_sub(page as usize);
                    self.vertical(-page, sel)
                }
                Key::PageDown => {
                    self.scroll_y += page as usize;
                    self.vertical(page, sel)
                }
                Key::Home if m.ctrl => self.move_to(0, sel),
                Key::End if m.ctrl => self.move_to(self.doc.rope.len_chars(), sel),
                Key::Home => {
                    let (line, _) = self.doc.char_to_line_byte(self.cursor);
                    let text = self.doc.line(line);
                    let indent = text.len() - text.trim_start().len();
                    let (_, byte) = self.doc.char_to_line_byte(self.cursor);
                    let target = if byte == indent { 0 } else { indent };
                    self.move_to(self.doc.line_byte_to_char(line, target), sel)
                }
                Key::End => {
                    let (line, _) = self.doc.char_to_line_byte(self.cursor);
                    self.move_to(self.doc.line_byte_to_char(line, usize::MAX), sel)
                }
                // Shift+Enter: a bare newline (agents type literal text this way).
                Key::Enter if m.shift && !m.ctrl && !m.alt => self.insert_text("\n"),
                Key::Enter if !m.ctrl && !m.alt => {
                    let (line, _) = self.doc.char_to_line_byte(self.cursor);
                    let text = self.doc.line(line);
                    let indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                    self.insert_text(&format!("\n{indent}"));
                }
                Key::Tab if !m.ctrl && !m.alt => {
                    if let Some(g) = self.ghost.take() {
                        self.insert_text(&g);
                    } else {
                        let (_, col) = self.cursor_line_col();
                        self.insert_text(&" ".repeat(TAB_WIDTH - col % TAB_WIDTH));
                    }
                }
                Key::Backspace => {
                    if !self.delete_selection() {
                        let from = self.grapheme_step(false);
                        if from < self.cursor {
                            self.doc.remove(from, self.cursor, self.cursor);
                            self.cursor = from;
                        }
                    }
                    self.ghost = None;
                }
                Key::Delete if !m.shift => {
                    if !self.delete_selection() {
                        let to = self.grapheme_step(true);
                        if to > self.cursor {
                            self.doc.remove(self.cursor, to, self.cursor);
                        }
                    }
                    self.ghost = None;
                }
                Key::Insert if m == Default::default() => self.overwrite = !self.overwrite,
                Key::Esc if self.ghost.is_some() => self.ghost = None,
                _ => return false,
            }
        }
        if !matches!(ev.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown) {
            self.pref_col = None;
        }
        true
    }

    fn pos_to_char(&self, p: Point) -> usize {
        let line = (self.scroll_y as i32 + p.y - self.area.y).max(0) as usize;
        let line = line.min(self.doc.len_lines().saturating_sub(1));
        let col = (self.scroll_x as i32 + p.x - self.area.x - self.gutter_width()).max(0) as usize;
        let byte = self.byte_at_col(line, col);
        self.doc.line_byte_to_char(line, byte)
    }

    pub fn handle_mouse(&mut self, ev: &MouseEvent) -> bool {
        match ev.kind {
            MouseKind::Down(Button::Left) if self.area.contains(ev.pos) => {
                let ch = self.pos_to_char(ev.pos);
                self.move_to(ch, ev.mods.shift);
                if !ev.mods.shift {
                    self.anchor = Some(ch);
                }
                self.dragging = true;
                true
            }
            MouseKind::Drag(Button::Left) if self.dragging => {
                let ch = self.pos_to_char(ev.pos);
                self.cursor = ch;
                true
            }
            MouseKind::Up(Button::Left) if self.dragging => {
                self.dragging = false;
                true
            }
            MouseKind::ScrollUp if self.area.contains(ev.pos) => {
                self.scroll_y = self.scroll_y.saturating_sub(3);
                true
            }
            MouseKind::ScrollDown if self.area.contains(ev.pos) => {
                self.scroll_y = (self.scroll_y + 3).min(self.doc.len_lines().saturating_sub(1));
                true
            }
            _ => false,
        }
    }

    fn gutter_width(&self) -> i32 {
        if self.line_numbers { self.doc.len_lines().to_string().len().max(3) as i32 + 1 } else { 0 }
    }

    pub fn ensure_cursor_visible(&mut self) {
        let (line, col) = self.cursor_line_col();
        let h = self.area.h.max(1) as usize;
        let w = (self.area.w - self.gutter_width()).max(1) as usize;
        if line < self.scroll_y {
            self.scroll_y = line;
        } else if line >= self.scroll_y + h {
            self.scroll_y = line + 1 - h;
        }
        if col < self.scroll_x {
            self.scroll_x = col;
        } else if col >= self.scroll_x + w {
            self.scroll_x = col + 1 - w;
        }
    }

    /// Draw into `area` (the inside of the window frame). Scroll bars go on
    /// the frame: right edge and bottom edge, given by `vbar`/`hbar`.
    pub fn draw(&mut self, c: &mut Canvas, area: Rect, theme: &Theme, focused: bool, follow_cursor: bool) {
        let s = &theme.scheme;
        self.area = area;
        if follow_cursor {
            self.ensure_cursor_visible();
        }
        let gw = self.gutter_width();
        let text_x = area.x + gw;
        c.fill(area, " ", s.editor);
        let sel = self.selection();
        let (cur_line, cur_col) = self.cursor_line_col();
        let mut c = c.sub(area);
        for row in 0..area.h {
            let li = self.scroll_y + row as usize;
            if li >= self.doc.len_lines() {
                break;
            }
            let y = area.y + row;
            if gw > 0 {
                let n = format!("{:>w$} ", li + 1, w = (gw - 1) as usize);
                c.print(area.x, y, &n, s.editor_gutter);
            }
            let text = self.doc.line(li);
            let line_start = self.doc.rope.line_to_char(li);
            let (placed, total) = layout(&text, self.base_rtl);
            let mut tc = c.sub(Rect::new(text_x, y, area.w - gw, 1));
            for p in &placed {
                let x = text_x + p.col as i32 - self.scroll_x as i32;
                if x + p.width as i32 <= text_x || x >= area.right() {
                    continue;
                }
                let ch = line_start + p.char_off;
                let selected = sel.is_some_and(|(a, b)| ch >= a && ch < b)
                    || self.block.iter().any(|&(a, b)| ch >= a && ch < b);
                let st = if selected {
                    s.editor_selection
                } else {
                    let kind = self
                        .highlights
                        .get(li)
                        .and_then(|spans| spans.iter().rev().find(|(a, b, _)| (*a as usize) <= p.offset && p.offset < *b as usize))
                        .map(|&(_, _, k)| k as usize);
                    match kind {
                        Some(k) => {
                            let mut st = s.editor;
                            if let Some(Some(c)) = s.syntax.get(k) {
                                st.fg = *c;
                            }
                            if k == 0 && s.bold_keywords {
                                st.attrs.bold = true;
                            }
                            st
                        }
                        None => s.editor,
                    }
                };
                let g = &text[p.offset..p.offset + p.len];
                if g == "\t" {
                    tc.fill(Rect::new(x, y, p.width as i32, 1), " ", st);
                } else {
                    tc.print(x, y, g, st);
                }
            }
            // Selected newline shows as one highlighted cell.
            let eol = line_start + text.chars().count();
            if sel.is_some_and(|(a, b)| eol >= a && eol < b) {
                let x = text_x + total as i32 - self.scroll_x as i32;
                tc.print(x, y, " ", s.editor_selection);
            }
            if li == cur_line {
                if let Some(g) = &self.ghost {
                    let first = g.lines().next().unwrap_or("");
                    let x = text_x + cur_col as i32 - self.scroll_x as i32;
                    tc.print(x, y, first, s.editor_ghost);
                }
            }
        }
        if focused {
            let x = text_x + cur_col as i32 - self.scroll_x as i32;
            let y = area.y + cur_line as i32 - self.scroll_y as i32;
            let shape = self.cursor_shape.unwrap_or(if self.overwrite { CursorShape::Block } else { CursorShape::Underline });
            c.set_cursor(Point::new(x, y), shape);
        }
    }

    pub fn draw_scrollbars(&self, c: &mut Canvas, frame: Rect, theme: &Theme) {
        let (line, col) = self.cursor_line_col();
        draw_vscroll(c, frame.right() - 1, frame.y + 1, frame.h - 2, line, self.doc.len_lines(), theme.scheme.scrollbar);
        let longest = 256.max(col + 1);
        draw_hscroll(c, frame.x + 2, frame.bottom() - 1, frame.w - 4, self.scroll_x, longest, theme.scheme.scrollbar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Mods;

    fn key(ed: &mut EditorView, k: Key, m: Mods) {
        let mut clip = String::new();
        ed.handle_key(&KeyEvent::new(k, m), &mut clip);
    }

    #[test]
    fn typing_moving_and_graphemes() {
        let mut ed = EditorView::new(Document::new());
        for c in "ae\u{301}x".chars() {
            ed.insert_text(&c.to_string());
        }
        key(&mut ed, Key::Left, Mods::NONE);
        key(&mut ed, Key::Left, Mods::NONE);
        assert_eq!(ed.cursor, 1, "left skips the whole é");
        key(&mut ed, Key::Delete, Mods::NONE);
        assert_eq!(ed.doc.rope.to_string(), "ax");
        key(&mut ed, Key::Enter, Mods::NONE);
        assert_eq!(ed.doc.rope.to_string(), "a\nx");
        key(&mut ed, Key::Up, Mods::NONE);
        key(&mut ed, Key::End, Mods::NONE);
        assert_eq!(ed.cursor_line_col(), (0, 1));
    }

    #[test]
    fn select_cut_paste() {
        let mut ed = EditorView::new(Document::from_str("hello world"));
        let mut clip = String::new();
        ed.handle_key(&KeyEvent::new(Key::Right, Mods::CTRL.with_shift()), &mut clip);
        ed.handle_key(&KeyEvent::new(Key::Char('x'), Mods::CTRL), &mut clip);
        assert_eq!(clip, "hello ");
        assert_eq!(ed.doc.rope.to_string(), "world");
        ed.handle_key(&KeyEvent::new(Key::End, Mods::NONE), &mut clip);
        ed.handle_key(&KeyEvent::new(Key::Char('v'), Mods::CTRL), &mut clip);
        assert_eq!(ed.doc.rope.to_string(), "worldhello ");
    }
}
