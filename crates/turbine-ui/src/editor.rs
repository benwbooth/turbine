//! The text editor view.

use crate::event::{Button, Key, KeyEvent, KeyKind, MouseEvent, MouseKind};
use crate::frame::{draw_hscroll, draw_vscroll};
use crate::geom::{Point, Rect};
use crate::surface::{Canvas, CursorShape};
use crate::theme::{Style, Theme};
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

/// A problem a language server found, placed in the document.
#[derive(Clone, Debug, PartialEq)]
pub struct DiagMark {
    pub line: usize,
    /// Byte range in the line (end may equal start: then one cell is marked).
    pub start: usize,
    pub end: usize,
    /// 1 error, 2 warning, 3 info, 4 hint.
    pub severity: u8,
    pub message: String,
}

/// The minimap's width in cells (eight chars of text each, as braille
/// dots), and the lines in each of its rows.
mod multi;
mod wrap;
mod structure;
pub use structure::Structure;

pub const MINIMAP_W: i32 = 10;
const MINIMAP_LINES: usize = 4;

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
    /// The cursor when inserting (a setting: block by default); typing
    /// over (Insert) shows the other one.
    pub insert_shape: CursorShape,
    /// Syntax highlighting per line: (start byte, end byte, kind index),
    /// later spans over earlier. May lag an edit by a moment.
    pub highlights: Vec<Vec<(u32, u32, u8)>>,
    /// Problems from the language server.
    pub diagnostics: Vec<DiagMark>,
    /// Lines (0-based) with breakpoints, and where the debugger stopped.
    pub breakpoints: Vec<usize>,
    pub exec_line: Option<usize>,
    /// Lines with a background of their own (a diff's): 1 added, 2
    /// removed, 3 a hunk's header; and per line the byte ranges changed.
    pub line_bg: Vec<u8>,
    pub words: Vec<Vec<(u32, u32)>>,
    /// The other cursors (and their selections' anchors), when there are
    /// several; column selection mode (Shift+Up/Down add cursors).
    pub cursors: Vec<(usize, Option<usize>)>,
    pub column_mode: bool,
    /// Long lines wrap at the window's edge (View > Word Wrap).
    pub wrap: bool,
    /// A help page: the scheme's help colours (QBasic's black, Borland's
    /// cyan), headings, links and their marks by highlight kind.
    pub help_look: bool,
    /// Extra selected char ranges (vim's visual block).
    pub block: Vec<(usize, usize)>,
    /// Inlay hints from the language server, per line: (byte, text), shown
    /// before that byte; for the document version they were made for.
    pub inlays: std::collections::BTreeMap<usize, Vec<(usize, String)>>,
    pub inlays_version: u64,
    /// Semantic highlighting (like `highlights`, from the language server),
    /// over the tree-sitter colours; for the version it was made for.
    pub semantic: Vec<Vec<(u32, u32, u8)>>,
    pub semantic_version: u64,
    /// The text's shape from its syntax tree (folds, nodes, definitions),
    /// for the version it was made for.
    pub structure: Structure,
    pub structure_version: u64,
    /// Folded regions: first line → last hidden line.
    pub folds: std::collections::BTreeMap<usize, usize>,
    /// (version, line count, cursor line) when the folds last followed an
    /// edit.
    fold_sync: (u64, usize, usize, bool),
    /// Selections Expand Selection grew from (Shrink goes back).
    expand_stack: Vec<(usize, usize)>,
    /// The language's line comment ("//", "#"), and whether a line ending
    /// in ':' opens a block (Python, YAML).
    pub line_comment: Option<String>,
    pub colon_indents: bool,
    /// Find's matches (char ranges, in order) and the current one.
    pub found: Vec<(usize, usize)>,
    pub found_current: Option<usize>,
    /// Text area from the last draw, for mouse hit testing.
    area: Rect,
    dragging: bool,
    /// A minimap at the right (Cursor's), when there's room; where it was
    /// drawn, and a drag on it.
    pub minimap: bool,
    minimap_rect: Option<Rect>,
    minimap_drag: bool,
    /// The cursor and text the view last followed: it scrolls to the
    /// cursor only when one of them changes (so the wheel can scroll away).
    followed: (usize, u64),
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
            insert_shape: CursorShape::Block,
            highlights: Vec::new(),
            diagnostics: Vec::new(),
            breakpoints: Vec::new(),
            exec_line: None,
            line_bg: Vec::new(),
            words: Vec::new(),
            block: Vec::new(),
            cursors: Vec::new(),
            column_mode: false,
            wrap: false,
            help_look: false,
            found: Vec::new(),
            found_current: None,
            structure: Structure::default(),
            structure_version: u64::MAX,
            folds: Default::default(),
            fold_sync: (u64::MAX, 0, 0, false),
            expand_stack: Vec::new(),
            line_comment: None,
            colon_indents: false,
            inlays: std::collections::BTreeMap::new(),
            inlays_version: u64::MAX,
            semantic: Vec::new(),
            semantic_version: u64::MAX,
            area: Rect::default(),
            dragging: false,
            minimap: false,
            minimap_rect: None,
            minimap_drag: false,
            followed: (usize::MAX, u64::MAX),
        }
    }

    /// (line, column) of the cursor, both from 0.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let (line, byte) = self.doc.char_to_line_byte(self.cursor);
        (line, self.col_of(line, byte))
    }

    /// The inlay hints on a line, when they're for this text.
    fn line_inlays(&self, line: usize) -> &[(usize, String)] {
        if self.inlays_version != self.doc.version || self.base_rtl == Some(true) {
            return &[];
        }
        self.inlays.get(&line).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Columns the line's inlay hints take before `byte` (`inclusive`: the
    /// ones at it too: a char sits after the hints at its place, the cursor
    /// before them).
    fn inlay_shift(&self, line: usize, byte: usize, inclusive: bool) -> usize {
        self.line_inlays(line).iter().filter(|(b, _)| if inclusive { *b <= byte } else { *b < byte }).map(|(_, t)| turbine_text::unicode::str_width(t)).sum()
    }

    /// The line's layout with inlay hints making room.
    fn layout_line(&self, line: usize, text: &str) -> (Vec<Placed>, usize) {
        let (mut placed, total) = layout(text, self.base_rtl);
        if self.line_inlays(line).is_empty() {
            return (placed, total);
        }
        for p in &mut placed {
            p.col += self.inlay_shift(line, p.offset, true);
        }
        (placed, total + self.inlay_shift(line, usize::MAX, true))
    }

    fn col_of(&self, line: usize, byte: usize) -> usize {
        let text = self.doc.line(line);
        let (placed, total) = layout(&text, self.base_rtl);
        let base = placed.iter().find(|p| p.offset == byte).map(|p| p.col).unwrap_or(total);
        base + self.inlay_shift(line, byte, false)
    }

    fn byte_at_col(&self, line: usize, col: usize) -> usize {
        let text = self.doc.line(line);
        let (placed, _) = self.layout_line(line, &text);
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

    /// Where the cursor is on screen (from the last draw), if visible.
    pub fn cursor_screen_pos(&self) -> Option<Point> {
        let (line, col) = self.cursor_line_col();
        if self.hidden(line) {
            return None;
        }
        let (row, x) = self.screen_place(line, col);
        let p = Point::new(self.area.x + self.gutter_width() + x as i32, self.area.y + row);
        self.area.contains(p).then_some(p)
    }

    /// The most severe problem at the cursor's line, if any.
    pub fn diagnostic_at_cursor(&self) -> Option<&DiagMark> {
        let (line, byte) = self.doc.char_to_line_byte(self.cursor);
        let on_line = self.diagnostics.iter().filter(|d| d.line == line);
        let mut best: Option<&DiagMark> = None;
        for d in on_line {
            let here = byte >= d.start && byte <= d.end.max(d.start + 1);
            let better = match best {
                None => true,
                Some(b) => (here, std::cmp::Reverse(d.severity)) > (byte >= b.start && byte <= b.end.max(b.start + 1), std::cmp::Reverse(b.severity)),
            };
            if better {
                best = Some(d);
            }
        }
        best
    }

    /// Move to the next (or previous) problem after the cursor, wrapping.
    pub fn goto_diagnostic(&mut self, forward: bool) -> Option<String> {
        if self.diagnostics.is_empty() {
            return None;
        }
        let mut marks: Vec<(usize, usize)> = self.diagnostics.iter().map(|d| (d.line, d.start)).collect();
        marks.sort();
        marks.dedup();
        let here = self.doc.char_to_line_byte(self.cursor);
        let target = if forward {
            marks.iter().find(|m| **m > here).or(marks.first())
        } else {
            marks.iter().rev().find(|m| **m < here).or(marks.last())
        }
        .copied()?;
        self.cursor = self.doc.line_byte_to_char(target.0, target.1);
        self.anchor = None;
        self.diagnostic_at_cursor().map(|d| d.message.clone())
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
        if let Some(ch) = self.wrapped_vertical(delta, col) {
            let pref = self.pref_col;
            self.move_to(ch, select);
            self.pref_col = pref;
            return;
        }
        // (Folded lines are stepped over.)
        let target = self.step_visible(line, delta);
        let byte = self.byte_at_col(target, col);
        let ch = self.doc.line_byte_to_char(target, byte);
        let pref = self.pref_col;
        self.move_to(ch, select);
        self.pref_col = pref;
    }

    /// Handle a key. Returns true if the editor used it.
    fn handle_key_one(&mut self, ev: &KeyEvent, clipboard: &mut String) -> bool {
        if ev.kind == KeyKind::Release {
            return false;
        }
        let m = ev.mods;
        let sel = m.shift;
        let page = self.area.h.max(1) as isize;
        self.sync_folds();
        if !matches!(ev.key, Key::Right | Key::Left) || !(m.shift && m.alt) {
            self.expand_stack.clear();
        }
        if self.structure_key(ev) {
            self.pref_col = None;
            return true;
        }
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
                    self.scroll_y = self.step_visible(self.scroll_y, -page);
                    self.vertical(-page, sel)
                }
                Key::PageDown => {
                    self.scroll_y = self.step_visible(self.scroll_y, page);
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
                Key::Enter if !m.ctrl && !m.alt => self.smart_newline(),
                Key::Tab if !m.ctrl && !m.alt && m.shift => self.indent_lines(false),
                Key::Tab if !m.ctrl && !m.alt && self.selection().is_some_and(|(a, b)| self.doc.rope.char_to_line(a) != self.doc.rope.char_to_line(b)) => self.indent_lines(true),
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
        if let Some(ch) = self.wrapped_pos(p.y - self.area.y, p.x - self.area.x - self.gutter_width()) {
            return ch;
        }
        let line = self.step_visible(self.scroll_y, (p.y - self.area.y).max(0) as isize);
        let col = (self.scroll_x as i32 + p.x - self.area.x - self.gutter_width()).max(0) as usize;
        let byte = self.byte_at_col(line, col);
        self.doc.line_byte_to_char(line, byte)
    }

    /// The first line the minimap shows (it scrolls with the text when the
    /// file is longer than it can show at two lines a row).
    fn minimap_top(&self, h: i32) -> usize {
        let total = self.doc.len_lines();
        let cap = (h.max(1) as usize) * MINIMAP_LINES;
        if total <= cap {
            return 0;
        }
        let room = total.saturating_sub(self.area.h.max(1) as usize).max(1);
        (self.scroll_y.min(room) * (total - cap)) / room
    }

    /// Scroll so the line under the minimap's row `y` is in the middle.
    fn minimap_scroll_to(&mut self, y: i32) {
        let Some(mr) = self.minimap_rect else { return };
        let line = self.minimap_top(mr.h) + (y - mr.y).clamp(0, mr.h - 1) as usize * MINIMAP_LINES;
        let h = self.area.h.max(1) as usize;
        let max = self.doc.len_lines().saturating_sub(h);
        self.scroll_y = line.saturating_sub(h / 2).min(max);
    }

    fn draw_minimap(&self, c: &mut Canvas, theme: &Theme) {
        let Some(mr) = self.minimap_rect else { return };
        let s = &theme.scheme;
        let top = self.minimap_top(mr.h);
        let (vis_a, vis_b) = (self.scroll_y, self.scroll_y + self.area.h.max(1) as usize);
        // Braille: each cell is 2 dots across by 4 down; a dot is 4 chars of
        // a line, lit when any of them isn't blank, in the code's colour.
        const BITS: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
        let total = self.doc.len_lines();
        for row in 0..mr.h {
            let first = top + row as usize * MINIMAP_LINES;
            // The lines in this row: their text, and a colour per dot column.
            let lines: Vec<Option<(Vec<char>, usize)>> = (0..MINIMAP_LINES)
                .map(|d| {
                    let li = first + d;
                    (li < total).then(|| (self.doc.line(li).chars().collect(), li))
                })
                .collect();
            let in_view = (first..first + MINIMAP_LINES).any(|l| l >= vis_a && l < vis_b && l < total.max(1));
            // (The view's rows in their colours, the rest dimmed: no band
            // over the text.)
            let bg = s.editor.bg;
            for k in 0..mr.w as usize {
                let mut bits = 0u32;
                let mut colour = None;
                for (dy, line) in lines.iter().enumerate() {
                    let Some((chars, li)) = line else { continue };
                    for dx in 0..2 {
                        let from = (k * 2 + dx) * 4;
                        let Some(off) = (from..from + 4).find(|&i| chars.get(i).is_some_and(|c| !c.is_whitespace())) else { continue };
                        bits |= BITS[dx][dy];
                        if colour.is_none() {
                            let byte: usize = chars[..off].iter().map(|c| c.len_utf8()).sum();
                            let kind = self
                                .highlights
                                .get(*li)
                                .and_then(|spans| spans.iter().rev().find(|(a, b, _)| (*a as usize) <= byte && byte < *b as usize))
                                .map(|&(_, _, k)| k as usize);
                            colour = Some(kind.and_then(|k| s.syntax.get(k).copied().flatten()).unwrap_or(s.editor.fg));
                        }
                    }
                }
                let glyph = char::from_u32(0x2800 + bits).unwrap_or(' ').to_string();
                let fg = if in_view { colour.unwrap_or(s.editor.fg) } else { s.editor_ghost.fg };
                c.print(mr.x + k as i32, mr.y + row, &glyph, crate::theme::Style::new(fg, bg));
            }
        }
    }

    pub fn handle_mouse(&mut self, ev: &MouseEvent) -> bool {
        // The minimap: a click or drag scrolls there.
        if let Some(mr) = self.minimap_rect {
            match ev.kind {
                MouseKind::Down(Button::Left) if mr.contains(ev.pos) => {
                    self.minimap_drag = true;
                    self.minimap_scroll_to(ev.pos.y);
                    return true;
                }
                MouseKind::Drag(Button::Left) if self.minimap_drag => {
                    self.minimap_scroll_to(ev.pos.y);
                    return true;
                }
                MouseKind::Up(_) if self.minimap_drag => {
                    self.minimap_drag = false;
                    return true;
                }
                MouseKind::ScrollUp | MouseKind::ScrollDown if mr.contains(ev.pos) => {
                    let up = ev.kind == MouseKind::ScrollUp;
                    let max = self.doc.len_lines().saturating_sub(1);
                    self.scroll_y = if up { self.scroll_y.saturating_sub(6) } else { (self.scroll_y + 6).min(max) };
                    return true;
                }
                _ => {}
            }
        }
        match ev.kind {
            // The gutter's fold marker.
            MouseKind::Down(Button::Left) if self.area.contains(ev.pos) && self.line_numbers && ev.pos.x == self.area.x + self.gutter_width() - 1 => {
                let line = self.step_visible(self.scroll_y, (ev.pos.y - self.area.y) as isize);
                if !self.toggle_fold_at(line) {
                    let ch = self.doc.rope.line_to_char(line);
                    self.move_to(ch, false);
                }
                true
            }
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
                self.scroll_y = self.step_visible(self.scroll_y, -3);
                true
            }
            MouseKind::ScrollDown if self.area.contains(ev.pos) => {
                self.scroll_y = self.step_visible(self.scroll_y, 3);
                true
            }
            _ => false,
        }
    }

    /// The minimap's place (for clicks routed here).
    pub fn minimap_rect(&self) -> Option<Rect> {
        self.minimap_rect
    }

    fn gutter_width(&self) -> i32 {
        if self.line_numbers { self.doc.len_lines().to_string().len().max(3) as i32 + 1 } else { 0 }
    }

    pub fn ensure_cursor_visible(&mut self) {
        self.sync_folds();
        let (line, col) = self.cursor_line_col();
        // (A cursor put inside a fold opens it.)
        self.unfold_around(line);
        let h = self.area.h.max(1);
        let w = (self.area.w - self.gutter_width()).max(1) as usize;
        let ww = self.wrap_width();
        if ww > 0 {
            // (Wrapped: the cursor's row in view; no sideways scrolling.)
            self.scroll_x = 0;
            if line < self.scroll_y {
                self.scroll_y = line;
            }
            while self.scroll_y < line && self.screen_place(line, col).0 >= h {
                let next = self.step_visible(self.scroll_y, 1);
                if next == self.scroll_y {
                    break;
                }
                self.scroll_y = next;
            }
            return;
        }
        if line < self.scroll_y {
            self.scroll_y = line;
        } else if self.rows_between(self.scroll_y, line) >= h {
            self.scroll_y = self.step_visible(line, -(h as isize - 1));
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
        let base = if self.help_look { s.help } else { s.editor };
        // The minimap takes the right columns when the editor is wide enough.
        let full = area;
        self.minimap_rect = (self.minimap && area.w >= 50).then(|| Rect::new(area.right() - MINIMAP_W, area.y, MINIMAP_W, area.h));
        let area = if self.minimap_rect.is_some() { Rect::new(area.x, area.y, area.w - MINIMAP_W - 1, area.h) } else { area };
        self.area = area;
        let now = (self.cursor, self.doc.version);
        if follow_cursor && now != self.followed {
            self.ensure_cursor_visible();
        }
        self.followed = now;
        if let Some(mr) = self.minimap_rect {
            c.fill(Rect::new(mr.x - 1, full.y, 1, full.h), " ", base);
            self.draw_minimap(c, theme);
        }
        let gw = self.gutter_width();
        let text_x = area.x + gw;
        c.fill(area, " ", base);
        let sel = self.selection();
        let (cur_line, cur_col) = self.cursor_line_col();
        let mut c = c.sub(area);
        self.sync_folds();
        if self.hidden(self.scroll_y) {
            self.scroll_y = self.step_visible(self.scroll_y, 0);
        }
        let shown = self.visible_lines(self.scroll_y, area.h.max(0) as usize);
        let wrap_w = self.wrap_width();
        let mut row: i32 = 0;
        for &li in shown.iter() {
            let nrows = if wrap_w > 0 { self.line_rows(li, wrap_w) } else { 1 };
            for sub in 0..nrows {
                if row >= area.h {
                    break;
                }
                let y = area.y + row;
                row += 1;
                // (Wrapped: each row shows its own stretch of the line.)
                let sx = if wrap_w > 0 { sub * wrap_w } else { self.scroll_x };
                if gw > 0 && sub == 0 {
                    let n = format!("{:>w$} ", li + 1, w = (gw - 1) as usize);
                    c.print(area.x, y, &n, s.editor_gutter);
                    if let Some(sev) = self.diagnostics.iter().filter(|d| d.line == li).map(|d| d.severity).min() {
                        let color = if sev == 1 { s.diag_error } else if sev == 2 { s.diag_warning } else { s.editor_gutter.fg };
                        c.print(area.x + gw - 1, y, "■", s.editor_gutter.fg(color));
                    } else if self.folds.contains_key(&li) {
                        c.print(area.x + gw - 1, y, "▸", s.editor_gutter);
                    } else if self.foldable(li).is_some() {
                        c.print(area.x + gw - 1, y, "▾", s.editor_gutter);
                    }
                }
                let text = self.doc.line(li);
                let line_start = self.doc.rope.line_to_char(li);
                let (placed, total) = self.layout_line(li, &text);
                // Borland's bars: the execution line, else a breakpoint line.
                let bar = if self.exec_line == Some(li) {
                    Some(s.exec_bar)
                } else if self.breakpoints.contains(&li) {
                    Some(s.breakpoint)
                } else {
                    match self.line_bg.get(li) {
                        Some(1) => Some(s.diff_add),
                        Some(2) => Some(s.diff_del),
                        Some(3) => Some(s.diff_hunk),
                        _ => None,
                    }
                };
                let diff_line = bar.is_some() && self.exec_line != Some(li) && !self.breakpoints.contains(&li);
                if let Some(b) = bar {
                    c.fill(Rect::new(text_x, y, area.w - gw, 1), " ", b);
                }
                let mut tc = c.sub(Rect::new(text_x, y, area.w - gw, 1));
                for p in &placed {
                    if wrap_w > 0 && (p.col < sx || p.col >= sx + wrap_w) {
                        continue;
                    }
                    let x = text_x + p.col as i32 - sx as i32;
                    if x + p.width as i32 <= text_x || x >= area.right() {
                        continue;
                    }
                    let ch = line_start + p.char_off;
                    let selected = sel.is_some_and(|(a, b)| ch >= a && ch < b)
                        || self.block.iter().any(|&(a, b)| ch >= a && ch < b)
                        || (!self.cursors.is_empty() && self.in_extra_selection(ch));
                    // (Find's matches: the current one, then the others.)
                    let found = if self.found.is_empty() {
                        None
                    } else {
                        let k = self.found.partition_point(|&(_, e)| e <= ch);
                        self.found.get(k).filter(|&&(a, _)| a <= ch).map(|_| self.found_current == Some(k))
                    };
                    let st = if selected {
                        s.editor_selection
                    } else if let Some(cur) = found {
                        if cur { s.find_current } else { s.find_match }
                    } else if let Some(b) = bar.filter(|_| !diff_line) {
                        b
                    } else if diff_line && self.words.get(li).is_some_and(|w| w.iter().any(|(a, b)| (*a as usize) <= p.offset && p.offset < *b as usize)) {
                        // (A diff's changed words stand out.)
                        if self.line_bg.get(li) == Some(&2) { s.diff_del_word } else { s.diff_add_word }
                    } else {
                        // (The language server's meaning first, then tree-sitter's.)
                        let find = |spans: Option<&Vec<(u32, u32, u8)>>| {
                            spans.and_then(|spans| spans.iter().rev().find(|(a, b, _)| (*a as usize) <= p.offset && p.offset < *b as usize)).map(|&(_, _, k)| k as usize)
                        };
                        let semantic = if self.semantic_version == self.doc.version { find(self.semantic.get(li)) } else { None };
                        let kind = semantic.or_else(|| find(self.highlights.get(li)));
                        match kind {
                            Some(k) if self.help_look && matches!(crate::theme::SYNTAX_KINDS.get(k).copied(), Some("keyword" | "type" | "function" | "label" | "punctuation" | "comment" | "attribute" | "constant" | "string" | "escape")) => {
                                match crate::theme::SYNTAX_KINDS[k] {
                                    "keyword" | "type" | "constant" => s.help_heading,
                                    "function" => s.help_link,
                                    "label" => s.help_mark,
                                    "string" => s.help_code,
                                    "escape" => s.help_key,
                                    "attribute" => Style { attrs: crate::theme::Attrs { bold: true, ..s.help.attrs }, ..s.help_heading },
                                    _ => s.help,
                                }
                            }
                            Some(k) => {
                                let mut st = base;
                                if let Some(Some(c)) = s.syntax.get(k) {
                                    st.fg = *c;
                                }
                                if k == 0 && s.bold_keywords {
                                    st.attrs.bold = true;
                                }
                                st
                            }
                            None => base,
                        }
                    };
                    // (A diff's line: its colour behind the syntax colours.)
                    let st = match bar {
                        Some(b) if diff_line && !selected && found.is_none() && st.bg == base.bg => Style { bg: b.bg, fg: if st.fg == base.fg { b.fg } else { st.fg }, ..st },
                        _ => st,
                    };
                    let st = match self.diagnostics.iter().filter(|d| d.line == li).find(|d| {
                        p.offset >= d.start && (p.offset < d.end || (d.end <= d.start && p.offset == d.start))
                    }) {
                        Some(d) if !selected => {
                            let mut st = st;
                            st.attrs.underline = true;
                            match d.severity {
                                1 => st.fg = s.diag_error,
                                2 => st.fg = s.diag_warning,
                                _ => {}
                            }
                            st
                        }
                        _ => st,
                    };
                    let g = &text[p.offset..p.offset + p.len];
                    if g == "\t" {
                        tc.fill(Rect::new(x, y, p.width as i32, 1), " ", st);
                    } else {
                        tc.print(x, y, g, st);
                    }
                }
                // Inlay hints (types, parameter names) in their places, dimmed.
                for (b, t) in self.line_inlays(li) {
                    let x = text_x + self.col_of(li, *b) as i32 - sx as i32;
                    tc.print(x, y, t, s.editor_ghost);
                }
                // A folded line ends in ⋯ (what's hidden).
                if self.folds.contains_key(&li) {
                    let x = text_x + total as i32 - sx as i32;
                    tc.print(x + 1, y, "⋯", s.editor_ghost);
                }
                // Selected newline shows as one highlighted cell.
                let eol = line_start + text.chars().count();
                if sel.is_some_and(|(a, b)| eol >= a && eol < b) {
                    let x = text_x + total as i32 - sx as i32;
                    tc.print(x, y, " ", s.editor_selection);
                }
                if li == cur_line {
                    if let Some(g) = &self.ghost {
                        let first = g.lines().next().unwrap_or("");
                        let x = text_x + cur_col as i32 - sx as i32;
                        tc.print(x, y, first, s.editor_ghost);
                    }
                }
            }
        }
        // The other cursors: a cell each, reversed.
        for &(cur, _) in &self.cursors {
            let (l, b) = self.doc.char_to_line_byte(cur.min(self.doc.rope.len_chars()));
            if self.hidden(l) || l < self.scroll_y {
                continue;
            }
            let (row, cx) = self.screen_place(l, self.col_of(l, b));
            if row >= area.h {
                continue;
            }
            let x = text_x + cx as i32;
            if x >= text_x && x < area.right() {
                let st = Style { fg: base.bg, bg: base.fg, ..base };
                let t = self.doc.line(l);
                let g = t[b.min(t.len())..].chars().next().filter(|c| *c != '\n').map(|c| c.to_string()).unwrap_or_else(|| " ".into());
                c.print(x, area.y + row, &g, st);
            }
        }
        if focused {
            let (crow, cx) = self.screen_place(cur_line, cur_col);
            let x = text_x + cx as i32;
            let y = area.y + if self.hidden(cur_line) { -1 } else { crow };
            let other = if self.insert_shape == CursorShape::Block { CursorShape::Underline } else { CursorShape::Block };
            let shape = self.cursor_shape.unwrap_or(if self.overwrite { other } else { self.insert_shape });
            c.set_cursor(Point::new(x, y), shape);
        }
    }

    pub fn draw_scrollbars(&self, c: &mut Canvas, frame: Rect, theme: &Theme) {
        let (line, col) = self.cursor_line_col();
        let ch = theme.style.scroll;
        draw_vscroll(c, frame.right() - 1, frame.y + 1, frame.h - 2, line, self.doc.len_lines(), theme.scheme.scrollbar, ch);
        let longest = 256.max(col + 1);
        // (Borland's windows show line:column in the bottom edge's first
        // cells: the bar starts after them.)
        let skip = if theme.style.window_numbers && frame.w > 20 { 13 } else { 2 };
        draw_hscroll(c, frame.x + skip, frame.bottom() - 1, frame.w - skip - 2, self.scroll_x, longest, theme.scheme.scrollbar, ch);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn inlay_hints_make_room_and_the_cursor_stays_true() {
        let mut ed = EditorView::new(Document::from_str("let x = 1;"));
        ed.inlays.insert(0, vec![(5, ": i32".into())]);
        ed.inlays_version = ed.doc.version;
        // "let x: i32 = 1;": '=' at byte 6 moves right by the hint's width.
        let (placed, _) = ed.layout_line(0, "let x = 1;");
        assert_eq!(placed.iter().find(|p| p.offset == 6).unwrap().col, 6 + 5);
        // The cursor at the hint's place sits before it; after it, past it.
        assert_eq!(ed.col_of(0, 5), 5);
        assert_eq!(ed.col_of(0, 6), 11);
        assert_eq!(ed.byte_at_col(0, 11), 6);
        // A change: the hint's stale, and goes.
        ed.doc.insert(0, "x", 0);
        assert_eq!(ed.col_of(0, 6), 6);
    }

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
