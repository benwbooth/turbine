//! More than one cursor (Cursor's and VS Code's): Ctrl+Alt+Up and Down add
//! one on the line above or below, Ctrl+D selects the next match of the
//! selection (or word) with a cursor of its own, and column selection
//! mode makes Shift+Up and Down select a column. Typing, deleting, moving
//! and selecting happen at every cursor; Esc goes back to one.

use super::*;

impl EditorView {
    /// Every cursor, the main one first: (cursor, anchor).
    pub fn all_cursors(&self) -> Vec<(usize, Option<usize>)> {
        let mut v = vec![(self.cursor, self.anchor)];
        v.extend(self.cursors.iter().copied());
        v
    }

    /// Keys, at every cursor when there are several.
    pub fn handle_key(&mut self, ev: &KeyEvent, clipboard: &mut String) -> bool {
        if ev.kind == KeyKind::Release {
            return false;
        }
        let m = ev.mods;
        // Adding cursors.
        if m.ctrl && m.alt && !m.shift && matches!(ev.key, Key::Up | Key::Down) {
            self.add_cursor_line(ev.key == Key::Down);
            return true;
        }
        if ev.is_ctrl('d') && !m.shift && !m.alt {
            self.add_next_match();
            return true;
        }
        // Column selection: Shift+Up/Down add a cursor whose selection
        // starts in the same column.
        if self.column_mode && m.shift && !m.ctrl && !m.alt && matches!(ev.key, Key::Up | Key::Down) {
            self.add_cursor_line(ev.key == Key::Down);
            // (Each new cursor's selection runs from the column the first
            // one's started at.)
            let col = self.anchor.map(|a| self.doc.rope.char_to_line(a)).map(|l| self.anchor.unwrap() - self.doc.rope.line_to_char(l)).unwrap_or(0);
            for c in &mut self.cursors {
                if c.1.is_none() {
                    let l = self.doc.rope.char_to_line(c.0);
                    let start = self.doc.rope.line_to_char(l);
                    let len = self.doc.line(l).chars().count();
                    c.1 = Some(start + col.min(len));
                }
            }
            return true;
        }
        if self.cursors.is_empty() {
            return self.handle_key_one(ev, clipboard);
        }
        if ev.key == Key::Esc {
            self.cursors.clear();
            return true;
        }
        // Copy: every selection, a line each; paste: a line to each cursor
        // when the counts match.
        if ev.is_ctrl('c') || ev.is_ctrl('x') {
            let mut all = self.all_cursors();
            all.sort_by_key(|c| c.0);
            let texts: Vec<String> = all.iter().map(|&(c, a)| a.map(|a| self.doc.rope.slice(a.min(c)..a.max(c)).to_string()).unwrap_or_default()).collect();
            if texts.iter().any(|t| !t.is_empty()) {
                *clipboard = texts.join("\n");
            }
            if ev.is_ctrl('c') {
                return true;
            }
        }
        if ev.is_ctrl('a') || ev.is_ctrl('z') || ev.is_ctrl('y') {
            self.cursors.clear();
            return self.handle_key_one(ev, clipboard);
        }
        let paste_lines: Option<Vec<String>> = (ev.is_ctrl('v') && clipboard.lines().count() == self.cursors.len() + 1).then(|| clipboard.lines().map(str::to_string).collect());
        // Each cursor in turn, last in the text first: an edit then moves
        // the cursors after it by what it added or took.
        let mut all = self.all_cursors();
        let main = all[0];
        all.sort_by_key(|c| std::cmp::Reverse(c.0));
        let mut done: Vec<(usize, Option<usize>, bool)> = Vec::new();
        let mut used = false;
        self.doc.break_undo_group();
        for (k, (c, a)) in all.iter().copied().enumerate() {
            self.cursor = c;
            self.anchor = a;
            let before = self.doc.rope.len_chars();
            let edit_at = a.map_or(c, |a| a.min(c));
            let mut clip = match &paste_lines {
                Some(lines) => lines[all.len() - 1 - k].clone(),
                None => clipboard.clone(),
            };
            used |= self.handle_key_one(ev, &mut clip);
            let delta = self.doc.rope.len_chars() as isize - before as isize;
            if delta != 0 {
                for d in done.iter_mut() {
                    let shift = |x: usize| if x >= edit_at { (x as isize + delta).max(0) as usize } else { x };
                    d.0 = shift(d.0);
                    d.1 = d.1.map(shift);
                }
            }
            done.push((self.cursor, self.anchor, (c, a) == main));
        }
        self.doc.break_undo_group();
        // Back to the main one; the rest kept (ones that met, merged).
        let i = done.iter().position(|d| d.2).unwrap_or(0);
        let (c, a, _) = done.remove(i);
        self.cursor = c;
        self.anchor = a;
        let mut rest: Vec<(usize, Option<usize>)> = done.into_iter().map(|(c, a, _)| (c, a)).filter(|&(x, _)| x != c).collect();
        rest.sort_by_key(|r| r.0);
        rest.dedup_by_key(|r| r.0);
        self.cursors = rest;
        used
    }

    /// A cursor on the line above the topmost (or below the bottommost),
    /// in the same column.
    pub fn add_cursor_line(&mut self, down: bool) {
        let all = self.all_cursors();
        let edge = if down { all.iter().map(|c| c.0).max() } else { all.iter().map(|c| c.0).min() }.unwrap_or(self.cursor);
        let line = self.doc.rope.char_to_line(edge);
        let col = self.cursor_line_col().1;
        let to = if down { line + 1 } else { line.wrapping_sub(1) };
        if to >= self.doc.len_lines() {
            return;
        }
        let byte = self.byte_at_col(to, col);
        let ch = self.doc.line_byte_to_char(to, byte);
        if !all.iter().any(|c| c.0 == ch) {
            self.cursors.push((ch, None));
        }
    }

    /// Ctrl+D: the word at the cursor selected; again, its next match
    /// gets a cursor (and selection) of its own.
    pub fn add_next_match(&mut self) {
        let Some((a, b)) = self.selection() else {
            // The word first.
            let r = &self.doc.rope;
            let is_w = |c: char| c.is_alphanumeric() || c == '_';
            let (mut s, mut e) = (self.cursor, self.cursor);
            while s > 0 && is_w(r.char(s - 1)) {
                s -= 1;
            }
            while e < r.len_chars() && is_w(r.char(e)) {
                e += 1;
            }
            if e > s {
                self.anchor = Some(s);
                self.cursor = e;
            }
            return;
        };
        let needle: String = self.doc.rope.slice(a..b).to_string();
        let text = self.doc.rope.to_string();
        // After the last cursor, wrapping around.
        let last = self.all_cursors().iter().map(|c| c.0).max().unwrap_or(b);
        let from = self.doc.rope.char_to_byte(last);
        let taken: Vec<usize> = self.all_cursors().iter().map(|c| c.0).collect();
        let hit = text[from..].find(&needle).map(|i| i + from).or_else(|| text[..from].find(&needle));
        if let Some(byte) = hit {
            let s = self.doc.rope.byte_to_char(byte);
            let e = s + needle.chars().count();
            if !taken.contains(&e) {
                self.cursors.push((e, Some(s)));
            }
        }
    }

    /// Whether a char is in one of the other cursors' selections.
    pub(super) fn in_extra_selection(&self, ch: usize) -> bool {
        self.cursors.iter().any(|&(c, a)| a.is_some_and(|a| ch >= a.min(c) && ch < a.max(c)))
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

    #[test]
    fn typing_at_every_cursor() {
        let mut ed = EditorView::new(Document::from_str("one\ntwo\nthree\n"));
        let ca = Mods { ctrl: true, alt: true, ..Mods::NONE };
        key(&mut ed, Key::Down, ca);
        key(&mut ed, Key::Down, ca);
        assert_eq!(ed.cursors.len(), 2);
        for c in "> ".chars() {
            key(&mut ed, Key::Char(c), Mods::NONE);
        }
        assert_eq!(ed.doc.rope.to_string(), "> one\n> two\n> three\n");
        key(&mut ed, Key::End, Mods::NONE);
        key(&mut ed, Key::Char(';'), Mods::NONE);
        key(&mut ed, Key::Backspace, Mods::NONE);
        key(&mut ed, Key::Char('!'), Mods::NONE);
        assert_eq!(ed.doc.rope.to_string(), "> one!\n> two!\n> three!\n");
        // One undo takes the lot back... a step at a time per key, all lines at once.
        ed.doc.undo();
        assert_eq!(ed.doc.rope.to_string(), "> one;\n> two;\n> three;\n".replace(';', ""));
        key(&mut ed, Key::Esc, Mods::NONE);
        assert!(ed.cursors.is_empty());
    }

    #[test]
    fn ctrl_d_selects_the_next_match() {
        let mut ed = EditorView::new(Document::from_str("let x = x + x;\n"));
        ed.cursor = 4;
        key(&mut ed, Key::Char('d'), Mods::CTRL);
        assert_eq!(ed.selected_text().as_deref(), Some("x"));
        key(&mut ed, Key::Char('d'), Mods::CTRL);
        key(&mut ed, Key::Char('d'), Mods::CTRL);
        assert_eq!(ed.cursors.len(), 2);
        key(&mut ed, Key::Char('y'), Mods::NONE);
        assert_eq!(ed.doc.rope.to_string(), "let y = y + y;\n");
    }

    #[test]
    fn column_selection() {
        let mut ed = EditorView::new(Document::from_str("abcd\nefgh\nijkl\n"));
        ed.column_mode = true;
        ed.cursor = 1;
        key(&mut ed, Key::Right, Mods::SHIFT);
        key(&mut ed, Key::Down, Mods::SHIFT);
        key(&mut ed, Key::Down, Mods::SHIFT);
        // (New cursors start where the first's selection did: column 1.)
        let mut clip = String::new();
        ed.handle_key(&KeyEvent::new(Key::Right, Mods::SHIFT), &mut clip);
        ed.handle_key(&KeyEvent::new(Key::Char('c'), Mods::CTRL), &mut clip);
        assert_eq!(clip, "bc\nfg\njk");
    }
}
