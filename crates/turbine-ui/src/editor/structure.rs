//! The editor's structural side, from the syntax tree when there is one:
//! folding (hidden lines), smart indentation, Expand/Shrink Selection,
//! going between definitions and to matching brackets, and the line
//! commands (comment, move, copy, indent).

use super::*;

pub use turbine_syntax_shape::Structure;

/// (The structure's type lives here so turbine-ui needn't depend on
/// tree-sitter: the workbench copies turbine-syntax's into it.)
mod turbine_syntax_shape {
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct Structure {
        /// Foldable regions: (first line, last line hidden), by first line.
        pub folds: Vec<(usize, usize)>,
        /// Named nodes' byte ranges, sorted by start, outer first.
        pub nodes: Vec<(u32, u32)>,
        /// Lines where definitions start.
        pub defs: Vec<usize>,
    }
}

const OPEN: &[char] = &['(', '[', '{'];
const CLOSE: &[char] = &[')', ']', '}'];

impl EditorView {
    // ---- folding --------------------------------------------------------------------------

    /// Whether a line is inside a folded region (not its first line).
    pub fn hidden(&self, line: usize) -> bool {
        self.folds.range(..line).any(|(&s, &e)| s < line && line <= e)
    }

    /// The foldable region starting at a line (current structure only).
    pub fn foldable(&self, line: usize) -> Option<usize> {
        if self.structure_version != self.doc.version && self.fold_sync.0 != self.doc.version {
            return None;
        }
        let f = &self.structure.folds;
        let i = f.partition_point(|&(s, _)| s < line);
        f.get(i).filter(|(s, _)| *s == line).map(|&(_, e)| e)
    }

    /// The line `n` visible lines from `line` (back when negative), staying
    /// in the document. 0 moves a hidden line to its fold's first line.
    pub fn step_visible(&self, line: usize, n: isize) -> usize {
        let last = self.doc.len_lines().saturating_sub(1);
        let mut l = line.min(last);
        while l > 0 && self.hidden(l) {
            l -= 1;
        }
        if self.folds.is_empty() {
            return (l as isize + n).clamp(0, last as isize) as usize;
        }
        for _ in 0..n.unsigned_abs() {
            if n > 0 {
                let mut next = l + 1;
                while next <= last && self.hidden(next) {
                    next += 1;
                }
                if next > last {
                    break;
                }
                l = next;
            } else {
                if l == 0 {
                    break;
                }
                let mut prev = l - 1;
                while prev > 0 && self.hidden(prev) {
                    prev -= 1;
                }
                l = prev;
            }
        }
        l
    }

    /// Rows on screen from line `a` down to line `b` (visible lines in a..b).
    pub(super) fn rows_between(&self, a: usize, b: usize) -> i32 {
        if self.folds.is_empty() {
            return b as i32 - a as i32;
        }
        if b < a {
            return -self.rows_between(b, a);
        }
        (a..b).filter(|&l| !self.hidden(l)).count() as i32
    }

    /// Up to `n` visible lines from `from`.
    pub(super) fn visible_lines(&self, from: usize, n: usize) -> Vec<usize> {
        let total = self.doc.len_lines();
        let mut out = Vec::with_capacity(n);
        let mut l = from;
        while out.len() < n && l < total {
            if !self.hidden(l) {
                out.push(l);
                if let Some(&e) = self.folds.get(&l) {
                    l = e + 1;
                    continue;
                }
            }
            l += 1;
        }
        out
    }

    /// Keep the folds on their lines after an edit: ones below it move with
    /// the lines added or taken; one the edit was inside opens.
    pub(super) fn sync_folds(&mut self) {
        let (v, lines, at, at_line_start) = self.fold_sync;
        let now = self.doc.len_lines();
        if v != self.doc.version && v != u64::MAX && !self.folds.is_empty() {
            let delta = now as isize - lines as isize;
            let old = std::mem::take(&mut self.folds);
            for (s, e) in old {
                let shift = |x: usize| (x as isize + delta).max(0) as usize;
                if s > at || (s == at && at_line_start && delta > 0) {
                    self.folds.insert(shift(s), shift(e));
                } else if at > e {
                    self.folds.insert(s, e);
                } else if at == s && delta == 0 {
                    self.folds.insert(s, e);
                }
            }
            self.folds.retain(|&s, e| {
                *e = (*e).min(now.saturating_sub(1));
                *e > s
            });
        }
        let c = self.cursor.min(self.doc.rope.len_chars());
        let line = self.doc.rope.char_to_line(c);
        self.fold_sync = (self.doc.version, now, line, self.doc.rope.line_to_char(line) == c);
    }

    /// Open any fold hiding a line.
    pub fn unfold_around(&mut self, line: usize) {
        if self.hidden(line) {
            self.folds.retain(|&s, &mut e| !(s < line && line <= e));
        }
    }

    /// The fold the cursor's line is in or starts (innermost).
    fn fold_for(&self, line: usize) -> Option<(usize, usize)> {
        self.structure.folds.iter().filter(|&&(s, e)| s <= line && line <= e).max_by_key(|&&(s, _)| s).copied()
    }

    /// Fold (true) or unfold the region the cursor is in.
    pub fn fold(&mut self, fold: bool) -> bool {
        self.sync_folds();
        let line = self.cursor_line_col().0;
        if !fold {
            if self.folds.remove(&line).is_some() {
                return true;
            }
            let inner = self.folds.iter().filter(|&(&s, &e)| s <= line && line <= e).map(|(&s, _)| s).max();
            return inner.is_some_and(|s| self.folds.remove(&s).is_some());
        }
        // (Already folded here: the one around it.)
        let mut target = self.fold_for(line);
        while let Some((s, _)) = target.filter(|(s, _)| self.folds.contains_key(s)) {
            target = s.checked_sub(1).and_then(|l| self.fold_for(l)).filter(|&(_, e)| e >= line);
        }
        let Some((s, e)) = target else { return false };
        self.folds.insert(s, e);
        self.cursor = self.doc.rope.line_to_char(s);
        self.anchor = None;
        true
    }

    /// Fold or open the region starting at a line (the gutter's marker).
    pub fn toggle_fold_at(&mut self, line: usize) -> bool {
        self.sync_folds();
        if self.folds.remove(&line).is_some() {
            return true;
        }
        match self.foldable(line) {
            Some(e) => {
                self.folds.insert(line, e);
                let cl = self.cursor_line_col().0;
                if line < cl && cl <= e {
                    self.cursor = self.doc.rope.line_to_char(line);
                    self.anchor = None;
                }
                true
            }
            None => false,
        }
    }

    /// Fold every region (`level` deep at most; 0 = all), or open them all.
    pub fn fold_all(&mut self, fold: bool) {
        self.sync_folds();
        self.folds.clear();
        if fold {
            for &(s, e) in &self.structure.folds {
                if !self.folds.iter().any(|(&a, &b)| a < s && e <= b) {
                    self.folds.insert(s, e);
                }
            }
            let l = self.cursor_line_col().0;
            if self.hidden(l) {
                let s = self.folds.iter().find(|(s, e)| **s < l && l <= **e).map(|(&s, _)| s).unwrap_or(0);
                self.cursor = self.doc.rope.line_to_char(s);
                self.anchor = None;
            }
        }
    }

    // ---- selection by structure -----------------------------------------------------------

    fn char_to_byte(&self, c: usize) -> u32 {
        self.doc.rope.char_to_byte(c.min(self.doc.rope.len_chars())) as u32
    }
    fn byte_to_char(&self, b: u32) -> usize {
        self.doc.rope.byte_to_char((b as usize).min(self.doc.rope.len_bytes()))
    }

    /// Grow the selection to the next enclosing syntax node (the word
    /// first, with no tree).
    pub fn expand_selection(&mut self) -> bool {
        let (a, b) = self.selection().unwrap_or((self.cursor, self.cursor));
        let (ba, bb) = (self.char_to_byte(a), self.char_to_byte(b));
        let current = self.structure_version == self.doc.version;
        let found = current
            .then(|| self.structure.nodes.iter().filter(|&&(s, e)| s <= ba && e >= bb && (s, e) != (ba, bb)).min_by_key(|&&(s, e)| e - s).copied())
            .flatten();
        let (na, nb) = match found {
            Some((s, e)) => (self.byte_to_char(s), self.byte_to_char(e)),
            None => {
                // The word, then the line, then everything.
                let is_w = |c: char| c.is_alphanumeric() || c == '_';
                let r = &self.doc.rope;
                let (mut s, mut e) = (a, b);
                while s > 0 && is_w(r.char(s - 1)) {
                    s -= 1;
                }
                while e < r.len_chars() && is_w(r.char(e)) {
                    e += 1;
                }
                if (s, e) == (a, b) || s == e {
                    let line = r.char_to_line(a);
                    let (ls, le) = (r.line_to_char(line), r.line_to_char(line) + self.doc.line(line).chars().count());
                    if (ls, le) != (a, b) && ls <= a && le >= b { (ls, le) } else { (0, r.len_chars()) }
                } else {
                    (s, e)
                }
            }
        };
        if (na, nb) == (a, b) {
            return false;
        }
        self.expand_stack.push((a, b));
        self.anchor = Some(na);
        self.cursor = nb;
        true
    }

    pub fn shrink_selection(&mut self) -> bool {
        let Some((a, b)) = self.expand_stack.pop() else { return false };
        if a == b {
            self.anchor = None;
            self.cursor = a;
        } else {
            self.anchor = Some(a);
            self.cursor = b;
        }
        true
    }

    /// Go to the next (or previous) definition's line.
    pub fn goto_definition_line(&mut self, forward: bool) -> bool {
        let line = self.cursor_line_col().0;
        let defs = &self.structure.defs;
        let to = if forward { defs.iter().find(|&&d| d > line) } else { defs.iter().rev().find(|&&d| d < line) };
        let Some(&to) = to else { return false };
        let text = self.doc.line(to);
        let indent = text.len() - text.trim_start().len();
        self.move_to(self.doc.line_byte_to_char(to, indent), false);
        true
    }

    /// Jump to the bracket matching the one at (or just before) the cursor.
    pub fn goto_bracket(&mut self) -> bool {
        let r = &self.doc.rope;
        let n = r.len_chars();
        let at = [self.cursor, self.cursor.wrapping_sub(1)].into_iter().find(|&i| i < n && (OPEN.contains(&r.char(i)) || CLOSE.contains(&r.char(i))));
        let Some(i) = at else { return false };
        let c = r.char(i);
        let (open, close, fwd) = match OPEN.iter().position(|&o| o == c) {
            Some(k) => (c, CLOSE[k], true),
            None => {
                let k = CLOSE.iter().position(|&x| x == c).unwrap();
                (OPEN[k], c, false)
            }
        };
        let mut depth = 0i32;
        let mut j = i as isize;
        while j >= 0 && (j as usize) < n {
            let ch = r.char(j as usize);
            if ch == open {
                depth += if fwd { 1 } else { -1 };
            } else if ch == close {
                depth += if fwd { -1 } else { 1 };
            }
            if depth == 0 {
                self.move_to(j as usize, false);
                return true;
            }
            j += if fwd { 1 } else { -1 };
        }
        false
    }

    // ---- indentation -----------------------------------------------------------------------

    /// One level of indentation, as the file has it.
    pub fn indent_unit(&self) -> String {
        for i in 0..self.doc.len_lines().min(400) {
            let l = self.doc.line(i);
            if l.starts_with('\t') {
                return "\t".into();
            }
            let n = l.len() - l.trim_start_matches(' ').len();
            if n > 0 && n <= 8 && !l.trim().is_empty() && !l.trim_start().starts_with('*') {
                return " ".repeat(n);
            }
        }
        " ".repeat(TAB_WIDTH)
    }

    /// Enter: the line's indent, one more after an opener (`{`, `(`, `[`,
    /// or `:` in Python), and the closer on a line of its own when the
    /// cursor sits between a pair.
    pub(super) fn smart_newline(&mut self) {
        self.delete_selection();
        let (line, byte) = self.doc.char_to_line_byte(self.cursor);
        let text = self.doc.line(line);
        let indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let before = text[..byte.min(text.len())].trim_end();
        let after = text[byte.min(text.len())..].trim_start();
        let unit = self.indent_unit();
        let opens = before.ends_with(OPEN) || (self.colon_indents && before.ends_with(':') && !before.trim_start().starts_with('#'));
        let pair = opens && before.chars().last().and_then(|o| OPEN.iter().position(|&x| x == o)).is_some_and(|k| after.starts_with(CLOSE[k]));
        // (Python: after return/pass/break, the next line goes back a level.)
        let ends_block = self.colon_indents && matches!(before.trim_start().split_whitespace().next(), Some("return" | "pass" | "break" | "continue" | "raise"));
        if pair {
            // Spaces before the cursor's closer go.
            let skip = text[byte.min(text.len())..].len() - after.len();
            if skip > 0 {
                let c = self.cursor;
                self.doc.remove(c, c + skip, c);
            }
            self.insert_text(&format!("\n{indent}{unit}\n{indent}"));
            self.cursor -= 1 + indent.chars().count();
        } else if opens {
            self.insert_text(&format!("\n{indent}{unit}"));
        } else if ends_block && indent.len() >= unit.len() {
            self.insert_text(&format!("\n{}", &indent[..indent.len() - unit.len()]));
        } else {
            self.insert_text(&format!("\n{indent}"));
        }
    }

    /// Indent (or outdent) the selected lines, the selection kept.
    pub fn indent_lines(&mut self, indent: bool) {
        let (a, b) = self.selection().unwrap_or((self.cursor, self.cursor));
        let r = &self.doc.rope;
        let (la, mut lb) = (r.char_to_line(a), r.char_to_line(b));
        if lb > la && b == r.line_to_char(lb) {
            lb -= 1;
        }
        let unit = self.indent_unit();
        let mut new = String::new();
        for l in la..=lb {
            let t = self.doc.line(l);
            let t = if indent {
                if t.trim().is_empty() { t } else { format!("{unit}{t}") }
            } else {
                let n = t.chars().take(unit.chars().count()).take_while(|c| *c == ' ' || *c == '\t').count();
                t.chars().skip(if t.starts_with('\t') { 1 } else { n }).collect()
            };
            new.push_str(&t);
            if l < lb {
                new.push('\n');
            }
        }
        let start = self.doc.rope.line_to_char(la);
        let end = self.doc.rope.line_to_char(lb) + self.doc.line(lb).chars().count();
        self.doc.break_undo_group();
        self.doc.replace(start, end, &new, self.cursor);
        self.doc.break_undo_group();
        let end = start + new.chars().count();
        self.anchor = Some(start);
        self.cursor = end;
    }

    /// The lines the cursor or selection covers.
    fn line_span(&self) -> (usize, usize) {
        let (a, b) = self.selection().unwrap_or((self.cursor, self.cursor));
        let r = &self.doc.rope;
        let (la, mut lb) = (r.char_to_line(a), r.char_to_line(b));
        if lb > la && b == r.line_to_char(lb) {
            lb -= 1;
        }
        (la, lb)
    }

    /// Ctrl+/: comment the lines out, or back in when they all are.
    pub fn toggle_comment(&mut self) -> bool {
        let Some(tok) = self.line_comment.clone() else { return false };
        let (la, lb) = self.line_span();
        let lines: Vec<String> = (la..=lb).map(|l| self.doc.line(l)).collect();
        let all = lines.iter().filter(|l| !l.trim().is_empty()).all(|l| l.trim_start().starts_with(tok.as_str()));
        let min_indent = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
        let new: Vec<String> = lines
            .iter()
            .map(|l| {
                if l.trim().is_empty() {
                    l.clone()
                } else if all {
                    let i = l.find(tok.as_str()).unwrap();
                    let rest = &l[i + tok.len()..];
                    format!("{}{}", &l[..i], rest.strip_prefix(' ').unwrap_or(rest))
                } else {
                    format!("{}{tok} {}", &l[..min_indent], &l[min_indent..])
                }
            })
            .collect();
        let (cl, cc) = self.cursor_line_col();
        let start = self.doc.rope.line_to_char(la);
        let end = self.doc.rope.line_to_char(lb) + self.doc.line(lb).chars().count();
        let had_sel = self.selection().is_some();
        self.doc.break_undo_group();
        self.doc.replace(start, end, &new.join("\n"), self.cursor);
        self.doc.break_undo_group();
        if had_sel {
            self.anchor = Some(start);
            self.cursor = start + new.join("\n").chars().count();
        } else {
            let shift = tok.chars().count() as isize + 1;
            let col = (cc as isize + if all { -shift } else { shift }).max(0) as usize;
            let len = self.doc.line(cl).chars().count();
            self.cursor = self.doc.rope.line_to_char(cl) + col.min(len);
        }
        true
    }

    /// Alt+Up/Down: move the lines; with `copy`, duplicate them that way.
    pub fn move_lines(&mut self, down: bool, copy: bool) -> bool {
        let (la, lb) = self.line_span();
        let last = self.doc.len_lines().saturating_sub(1);
        if !copy && ((down && lb >= last) || (!down && la == 0)) {
            return false;
        }
        let block: Vec<String> = (la..=lb).map(|l| self.doc.line(l)).collect();
        let n = block.len();
        let sel = self.selection();
        let (cl, cc) = self.cursor_line_col();
        let (from, to, text, shift) = if copy {
            // The copy goes below (down) or above, the cursor on it.
            let t = block.join("\n");
            let at = self.doc.rope.line_to_char(lb) + self.doc.line(lb).chars().count();
            (at, at, format!("\n{t}"), if down { n as isize } else { 0 })
        } else if down {
            let below = self.doc.line(lb + 1);
            let start = self.doc.rope.line_to_char(la);
            let end = self.doc.rope.line_to_char(lb + 1) + below.chars().count();
            (start, end, format!("{below}\n{}", block.join("\n")), 1)
        } else {
            let above = self.doc.line(la - 1);
            let start = self.doc.rope.line_to_char(la - 1);
            let end = self.doc.rope.line_to_char(lb) + self.doc.line(lb).chars().count();
            (start, end, format!("{}\n{above}", block.join("\n")), -1)
        };
        self.doc.break_undo_group();
        self.doc.replace(from, to, &text, self.cursor);
        self.doc.break_undo_group();
        let place = |ed: &Self, line: usize, col: usize| ed.doc.rope.line_to_char(line) + col.min(ed.doc.line(line).chars().count());
        let nl = (cl as isize + shift) as usize;
        self.cursor = place(self, nl, cc);
        // (The selection's other end moves with it.)
        let anchor_lc = sel.and(self.anchor).map(|a| (self.doc.rope.char_to_line(a.min(self.doc.rope.len_chars())), a));
        let _ = anchor_lc;
        self.anchor = None;
        if let Some((a, b)) = sel {
            let first = place(self, (la as isize + shift).max(0) as usize, 0);
            let last_line = (lb as isize + shift).max(0) as usize;
            let last = place(self, last_line, usize::MAX);
            let _ = (a, b);
            self.anchor = Some(first);
            self.cursor = last;
        }
        true
    }

    /// Keys for the above: Shift+Alt+Right/Left grow and shrink the
    /// selection, Alt+Up/Down move lines, Shift+Alt+Up/Down copy them,
    /// Ctrl+/ comments, Ctrl+Shift+[ and ] fold and unfold, Ctrl+Shift+\
    /// goes to the matching bracket, and a closer typed on an empty line
    /// goes back a level.
    pub(super) fn structure_key(&mut self, ev: &KeyEvent) -> bool {
        let m = ev.mods;
        match ev.key {
            Key::Right if m.shift && m.alt && !m.ctrl => {
                self.expand_selection();
                true
            }
            Key::Left if m.shift && m.alt && !m.ctrl => {
                self.shrink_selection();
                true
            }
            Key::Up | Key::Down if m.alt && !m.ctrl => {
                self.move_lines(ev.key == Key::Down, m.shift);
                true
            }
            Key::Char('/') if m.ctrl && !m.alt => self.toggle_comment(),
            Key::Char('[' | '{') if m.ctrl && m.shift && !m.alt => self.fold(true) || true,
            Key::Char(']' | '}') if m.ctrl && m.shift && !m.alt => self.fold(false) || true,
            Key::Char('\\' | '|') if m.ctrl && m.shift && !m.alt => self.goto_bracket() || true,
            Key::Char(c @ ('}' | ')' | ']')) if !m.ctrl && !m.alt && self.selection().is_none() => {
                let (line, byte) = self.doc.char_to_line_byte(self.cursor);
                let text = self.doc.line(line);
                if text[..byte.min(text.len())].trim().is_empty() && byte > 0 && byte == text.len() {
                    let unit = self.indent_unit();
                    if text.ends_with(unit.as_str()) {
                        let n = unit.chars().count();
                        let c0 = self.cursor;
                        self.doc.remove(c0 - n, c0, c0);
                        self.cursor -= n;
                    }
                }
                let _ = c;
                false
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Key, KeyEvent, Mods};

    fn key(ed: &mut EditorView, k: Key, m: Mods) {
        let mut clip = String::new();
        ed.handle_key(&KeyEvent::new(k, m), &mut clip);
    }
    fn text(ed: &EditorView) -> String {
        ed.doc.rope.to_string()
    }

    #[test]
    fn folds_hide_lines_and_follow_edits() {
        let mut ed = EditorView::new(Document::from_str("a {\n  b\n  c\n}\nd\n"));
        ed.structure = Structure { folds: vec![(0, 2)], nodes: vec![], defs: vec![0] };
        ed.structure_version = ed.doc.version;
        ed.line_numbers = false;
        assert!(ed.toggle_fold_at(0));
        assert!(ed.hidden(1) && ed.hidden(2) && !ed.hidden(3));
        let mut surf = crate::Surface::new(20, 5);
        let theme = Theme::default();
        ed.draw(&mut surf.canvas(), Rect::new(0, 0, 20, 5), &theme, true, true);
        let shown = surf.to_text();
        assert!(shown.starts_with("a { ⋯"), "{shown}");
        assert_eq!(shown.lines().nth(1).unwrap().trim(), "}");
        // Down steps over the fold.
        key(&mut ed, Key::Down, Mods::NONE);
        assert_eq!(ed.cursor_line_col().0, 3);
        // A line added above moves the fold down.
        ed.cursor = 0;
        ed.sync_folds();
        ed.insert_text("x\n");
        ed.sync_folds();
        assert_eq!(ed.folds.get(&1), Some(&3));
        // Going into it opens it.
        ed.cursor = ed.doc.rope.line_to_char(2);
        ed.ensure_cursor_visible();
        assert!(ed.folds.is_empty());
    }

    #[test]
    fn smart_indent_comment_move_expand() {
        let mut ed = EditorView::new(Document::from_str("fn f() {}\n"));
        ed.cursor = 8;
        key(&mut ed, Key::Enter, Mods::NONE);
        assert_eq!(text(&ed), "fn f() {\n    \n}\n");
        assert_eq!(ed.cursor_line_col(), (1, 4));
        for c in "if x {".chars() {
            key(&mut ed, Key::Char(c), Mods::NONE);
        }
        key(&mut ed, Key::Enter, Mods::NONE);
        key(&mut ed, Key::Char('y'), Mods::NONE);
        key(&mut ed, Key::Enter, Mods::NONE);
        key(&mut ed, Key::Char('}'), Mods::NONE);
        assert_eq!(text(&ed), "fn f() {\n    if x {\n        y\n    }\n}\n");

        // Python: ':' opens; return ends the block.
        let mut py = EditorView::new(Document::from_str("def f():"));
        py.colon_indents = true;
        py.cursor = 8;
        key(&mut py, Key::Enter, Mods::NONE);
        for c in "return 1".chars() {
            key(&mut py, Key::Char(c), Mods::NONE);
        }
        key(&mut py, Key::Enter, Mods::NONE);
        assert_eq!(text(&py), "def f():\n    return 1\n");

        // Ctrl+/ both ways; Alt+Down moves; Shift+Alt+Down copies.
        let mut ed = EditorView::new(Document::from_str("one\n  two\nthree\n"));
        ed.line_comment = Some("//".into());
        key(&mut ed, Key::Char('/'), Mods::CTRL);
        assert_eq!(text(&ed), "// one\n  two\nthree\n");
        key(&mut ed, Key::Char('/'), Mods::CTRL);
        assert_eq!(text(&ed), "one\n  two\nthree\n");
        key(&mut ed, Key::Down, Mods::ALT);
        assert_eq!(text(&ed), "  two\none\nthree\n");
        assert_eq!(ed.cursor_line_col().0, 1);
        key(&mut ed, Key::Down, Mods { shift: true, alt: true, ..Mods::NONE });
        assert_eq!(text(&ed), "  two\none\none\nthree\n");
        assert_eq!(ed.cursor_line_col().0, 2);
        ed.doc.undo();

        // Expand: the word, then nodes outward; Shrink goes back.
        let mut ed = EditorView::new(Document::from_str("f(ab + c)"));
        ed.structure = Structure { folds: vec![], nodes: vec![(0, 9), (1, 9), (2, 8), (2, 4)], defs: vec![] };
        ed.structure_version = ed.doc.version;
        ed.cursor = 3;
        let sa = Mods { shift: true, alt: true, ..Mods::NONE };
        key(&mut ed, Key::Right, sa);
        assert_eq!(ed.selected_text().as_deref(), Some("ab"));
        key(&mut ed, Key::Right, sa);
        assert_eq!(ed.selected_text().as_deref(), Some("ab + c"));
        key(&mut ed, Key::Right, sa);
        assert_eq!(ed.selected_text().as_deref(), Some("(ab + c)"));
        key(&mut ed, Key::Left, sa);
        assert_eq!(ed.selected_text().as_deref(), Some("ab + c"));
        key(&mut ed, Key::Left, sa);
        key(&mut ed, Key::Left, sa);
        assert_eq!((ed.selection(), ed.cursor), (None, 3));
        // Ctrl+Shift+\: the matching bracket.
        ed.cursor = 1;
        key(&mut ed, Key::Char('\\'), Mods { ctrl: true, shift: true, ..Mods::NONE });
        assert_eq!(ed.cursor, 8);
    }
}
