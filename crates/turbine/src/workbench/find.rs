//! Find and Replace in the editor (Cursor's find widget): a small box at
//! the top right of the editor with the query, toggles for case, whole
//! word and regex, the match count, and a replace row (Ctrl+H).

use super::*;
use super::shell::Focus;

#[derive(Default)]
pub(super) struct FindBar {
    pub open: bool,
    pub query: LineInput,
    pub replace: LineInput,
    /// The replace row is shown; typing goes to it.
    pub replacing: bool,
    pub in_replace: bool,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    pub error: Option<String>,
    /// What the matches were found for: (window, version, query, flags).
    found_for: Option<(u32, u64, String, (bool, bool, bool))>,
}

/// The pattern for the query and flags (None for an empty query).
fn pattern(q: &str, case: bool, word: bool, regex: bool) -> Option<Result<regex::Regex, String>> {
    if q.is_empty() {
        return None;
    }
    let body = if regex { q.to_string() } else { regex::escape(q) };
    let body = if word { format!(r"\b(?:{body})\b") } else { body };
    let body = if case { body } else { format!("(?i){body}") };
    Some(regex::RegexBuilder::new(&body).multi_line(true).build().map_err(|e| e.to_string().lines().last().unwrap_or("bad pattern").to_string()))
}

const MAX_MATCHES: usize = 20_000;

impl Workbench {
    /// The box's place: the top right of the editor in front.
    pub(super) fn find_rect(&self) -> Option<Rect> {
        let i = self.editor_index()?;
        let inner = self.windows[i].win.inner();
        let w = 56.min(inner.w);
        let h = if self.find.replacing { 4 } else { 3 };
        Some(Rect::new(inner.right() - w, inner.y, w, h))
    }

    pub(super) fn open_find(&mut self, replace: bool) {
        // (The selection, when it's one line, is what to find.)
        if let Some(i) = self.editor_index() {
            if let Some(t) = self.windows[i].ed.selected_text().filter(|t| !t.contains('\n') && !t.is_empty()) {
                self.find.query.set(&t);
            }
        }
        self.find.open = true;
        self.find.replacing = replace;
        self.find.in_replace = replace && !self.find.query.text.is_empty();
        self.shell.focus = Focus::Find;
        self.refind(true);
    }

    pub(super) fn close_find(&mut self) {
        self.find.open = false;
        for w in &mut self.windows {
            w.ed.found.clear();
            w.ed.found_current = None;
        }
        if self.shell.focus == Focus::Find {
            self.shell.focus = Focus::Editors;
        }
    }

    /// Find again when the query, flags or text changed; `jump`: select
    /// the first match from the cursor.
    pub(super) fn refind(&mut self, jump: bool) {
        if !self.find.open {
            return;
        }
        let Some(i) = self.editor_index() else { return };
        let f = &self.find;
        let key = (self.windows[i].win.id, self.windows[i].ed.doc.version, f.query.text.clone(), (f.case, f.word, f.regex));
        if self.find.found_for.as_ref() == Some(&key) && !jump {
            return;
        }
        self.find.found_for = Some(key);
        let ed = &mut self.windows[i].ed;
        ed.found.clear();
        ed.found_current = None;
        self.find.error = None;
        let re = match pattern(&self.find.query.text, self.find.case, self.find.word, self.find.regex) {
            None => return,
            Some(Err(e)) => {
                self.find.error = Some(e);
                return;
            }
            Some(Ok(re)) => re,
        };
        let text = ed.doc.rope.to_string();
        for m in re.find_iter(&text).take(MAX_MATCHES) {
            if m.start() == m.end() {
                continue;
            }
            ed.found.push((ed.doc.rope.byte_to_char(m.start()), ed.doc.rope.byte_to_char(m.end())));
        }
        if jump {
            let from = ed.selection().map_or(ed.cursor, |(a, _)| a);
            let k = ed.found.partition_point(|&(a, _)| a < from);
            self.select_match(if k < self.windows[i].ed.found.len() { k } else { 0 });
        } else if let Some(k) = {
            let c = ed.cursor;
            ed.found.iter().position(|&(_, e)| e == c)
        } {
            ed.found_current = Some(k);
        }
    }

    fn select_match(&mut self, k: usize) {
        let Some(i) = self.editor_index() else { return };
        let ed = &mut self.windows[i].ed;
        let Some(&(a, b)) = ed.found.get(k) else { return };
        ed.found_current = Some(k);
        ed.anchor = Some(a);
        ed.cursor = b;
        ed.ensure_cursor_visible();
    }

    /// Next (or previous) match, wrapping around.
    pub(super) fn find_step(&mut self, forward: bool) {
        self.refind(false);
        let Some(i) = self.editor_index() else { return };
        let ed = &self.windows[i].ed;
        let n = ed.found.len();
        if n == 0 {
            return;
        }
        let k = match ed.found_current {
            Some(k) if forward => (k + 1) % n,
            Some(k) => (k + n - 1) % n,
            None => {
                // (The first one after the cursor, or the last one before it.)
                let c = ed.cursor;
                if forward { ed.found.partition_point(|&(a, _)| a < c) % n } else { (ed.found.partition_point(|&(a, _)| a < c) + n - 1) % n }
            }
        };
        self.select_match(k);
    }

    /// Replace the current match and go to the next.
    pub(super) fn replace_one(&mut self) {
        self.refind(false);
        let Some(i) = self.editor_index() else { return };
        let Some(Ok(re)) = pattern(&self.find.query.text, self.find.case, self.find.word, self.find.regex) else { return };
        let ed = &mut self.windows[i].ed;
        let Some(k) = ed.found_current else {
            self.find_step(true);
            return;
        };
        let (a, b) = ed.found[k];
        let old = ed.doc.rope.slice(a..b).to_string();
        let new = if self.find.regex { re.replace(&old, self.find.replace.text.as_str()).into_owned() } else { self.find.replace.text.clone() };
        ed.doc.break_undo_group();
        ed.doc.remove(a, b, ed.cursor);
        ed.doc.insert(a, &new, a);
        ed.cursor = a + new.chars().count();
        ed.anchor = None;
        ed.doc.break_undo_group();
        self.find.found_for = None;
        self.refind(false);
        // The next one after what was put in.
        let ed = &self.windows[i].ed;
        let c = ed.cursor;
        let n = ed.found.len();
        if n > 0 {
            let k = ed.found.partition_point(|&(s, _)| s < c) % n;
            self.select_match(k);
        }
    }

    /// Replace every match (one undo step). Returns how many.
    pub(super) fn replace_all(&mut self) -> usize {
        self.refind(false);
        let Some(i) = self.editor_index() else { return 0 };
        let Some(Ok(re)) = pattern(&self.find.query.text, self.find.case, self.find.word, self.find.regex) else { return 0 };
        let ed = &mut self.windows[i].ed;
        let text = ed.doc.rope.to_string();
        let rep = self.find.replace.text.as_str();
        let new = if self.find.regex { re.replace_all(&text, rep).into_owned() } else { re.replace_all(&text, regex::NoExpand(rep)).into_owned() };
        let n = ed.found.len();
        if n > 0 {
            ed.doc.break_undo_group();
            ed.doc.replace_all_minimal(&new, ed.cursor);
            ed.doc.break_undo_group();
            ed.cursor = ed.cursor.min(ed.doc.rope.len_chars());
            ed.anchor = None;
        }
        self.find.found_for = None;
        self.refind(false);
        n
    }

    pub(super) fn find_key(&mut self, k: &KeyEvent) {
        let alt = |c: char| k.mods.alt && !k.mods.ctrl && matches!(k.key, Key::Char(x) if x.eq_ignore_ascii_case(&c));
        match k.key {
            Key::Esc => self.close_find(),
            _ if alt('c') => self.find.case = !self.find.case,
            _ if alt('w') => self.find.word = !self.find.word,
            _ if alt('r') => self.find.regex = !self.find.regex,
            Key::Enter if k.mods.ctrl && k.mods.alt => {
                let n = self.replace_all();
                self.say(format!("Replaced {n} occurrence{}", if n == 1 { "" } else { "s" }));
            }
            Key::Enter if self.find.in_replace => self.replace_one(),
            Key::Enter => self.find_step(!k.mods.shift),
            Key::Tab | Key::BackTab if self.find.replacing => self.find.in_replace = !self.find.in_replace,
            Key::Up => self.find_step(false),
            Key::Down => self.find_step(true),
            _ => {
                let field = if self.find.in_replace { &mut self.find.replace } else { &mut self.find.query };
                if field.key(k) && !self.find.in_replace {
                    self.refind(true);
                }
                return;
            }
        }
        self.refind(false);
    }

    /// Clicks on the box: its fields and toggles. True when it was hit.
    pub(super) fn find_mouse(&mut self, m: &MouseEvent) -> bool {
        if !self.find.open {
            return false;
        }
        let Some(r) = self.find_rect() else { return false };
        if !r.contains(m.pos) {
            return false;
        }
        if !matches!(m.kind, MouseKind::Down(Button::Left)) {
            return true;
        }
        self.shell.focus = Focus::Find;
        let (rx, y) = (m.pos.x - r.x, m.pos.y);
        let tx = r.w - 22;
        if y == r.y + 1 {
            match rx - tx {
                0..=1 => self.find.case = !self.find.case,
                3..=4 => self.find.word = !self.find.word,
                6..=7 => self.find.regex = !self.find.regex,
                15 => self.find_step(false),
                17 => self.find_step(true),
                19 => self.close_find(),
                _ => self.find.in_replace = false,
            }
        } else if self.find.replacing && y == r.y + 2 {
            match rx - tx {
                0..=6 => self.replace_one(),
                8..=12 => {
                    self.replace_all();
                }
                _ => self.find.in_replace = true,
            }
        }
        self.refind(false);
        true
    }

    pub(super) fn draw_find(&mut self, c: &mut Canvas, theme: &Theme) {
        if !self.find.open {
            return;
        }
        self.refind(false);
        let Some(r) = self.find_rect() else { return };
        let s = &theme.scheme;
        let focused = self.shell.focus == Focus::Find && self.dialog.is_none() && !self.menubar.is_open();
        c.fill(r, " ", s.dialog);
        turbine_ui::frame::draw_frame(c, r, theme.style.menu_frame, s.dialog_frame);
        turbine_ui::frame::draw_title_in(c, r, if self.find.replacing { "Replace" } else { "Find" }, s.dialog_frame, 2);
        if theme.style.shadows {
            c.shadow(r, s.shadow);
        }
        let tx = r.x + r.w - 22;
        let field_w = tx - r.x - 2;
        let (count, n) = self.editor_index().map_or((None, 0), |i| (self.windows[i].ed.found_current, self.windows[i].ed.found.len()));
        let f = &self.find;
        f.query.draw(c, Rect::new(r.x + 1, r.y + 1, field_w, 1), theme, focused && !f.in_replace, "Find");
        // Toggles lit when on, then the count, ↑ ↓ and ×.
        let tog = |on: bool| if on { s.button_focused } else { s.dialog };
        let mut cc = c.sub(r.inset(1));
        cc.print(tx, r.y + 1, "Aa", tog(f.case));
        cc.print(tx + 3, r.y + 1, "ab", tog(f.word));
        cc.print(tx + 6, r.y + 1, ".*", tog(f.regex));
        let label = match (&f.error, n) {
            (Some(_), _) => "error".to_string(),
            (None, 0) if !f.query.text.is_empty() => "none".to_string(),
            (None, 0) => String::new(),
            (None, n) => format!("{}/{}", count.map_or(0, |k| k + 1), n),
        };
        cc.print(tx + 9, r.y + 1, &format!("{label:>5}"), s.dialog);
        cc.print(tx + 15, r.y + 1, "↑", s.dialog);
        cc.print(tx + 17, r.y + 1, "↓", s.dialog);
        cc.print(tx + 19, r.y + 1, "×", s.dialog);
        if f.replacing {
            f.replace.draw(c, Rect::new(r.x + 1, r.y + 2, field_w, 1), theme, focused && f.in_replace, "Replace");
            let mut cc = c.sub(r.inset(1));
            cc.print(tx, r.y + 2, "Replace", s.dialog);
            cc.print(tx + 8, r.y + 2, "All", s.dialog);
        }
        if let Some(e) = &f.error {
            let mut cc = c.sub(r.inset(1));
            cc.print(r.x + 1, r.bottom() - 1, &format!(" {e} "), s.dialog_frame.fg(vga::LIGHT_RED));
        }
    }

    pub(super) fn find_nodes(&self, root: Node) -> Node {
        let Some(r) = self.find_rect().filter(|_| self.find.open) else { return root };
        let tx = r.x + r.w - 22;
        let field_w = tx - r.x - 2;
        let mut n = Node::new("find", "dialog", if self.find.replacing { "Replace" } else { "Find" }, r).focused(self.shell.focus == Focus::Find);
        n = n.child(Node::new("find.query", "input", "Find", Rect::new(r.x + 1, r.y + 1, field_w, 1)).value(self.find.query.text.clone()).focused(!self.find.in_replace));
        n = n.child(Node::new("find.case", "checkbox", "Match Case", Rect::new(tx, r.y + 1, 2, 1)).value(if self.find.case { "checked" } else { "unchecked" }));
        n = n.child(Node::new("find.word", "checkbox", "Match Whole Word", Rect::new(tx + 3, r.y + 1, 2, 1)).value(if self.find.word { "checked" } else { "unchecked" }));
        n = n.child(Node::new("find.regex", "checkbox", "Use Regular Expression", Rect::new(tx + 6, r.y + 1, 2, 1)).value(if self.find.regex { "checked" } else { "unchecked" }));
        n = n.child(Node::new("find.prev", "button", "Previous Match", Rect::new(tx + 15, r.y + 1, 1, 1)));
        n = n.child(Node::new("find.next", "button", "Next Match", Rect::new(tx + 17, r.y + 1, 1, 1)));
        n = n.child(Node::new("find.close", "button", "Close", Rect::new(tx + 19, r.y + 1, 1, 1)));
        if self.find.replacing {
            n = n.child(Node::new("find.replace", "input", "Replace", Rect::new(r.x + 1, r.y + 2, field_w, 1)).value(self.find.replace.text.clone()).focused(self.find.in_replace));
            n = n.child(Node::new("find.replace_one", "button", "Replace", Rect::new(tx, r.y + 2, 7, 1)));
            n = n.child(Node::new("find.replace_all", "button", "Replace All", Rect::new(tx + 8, r.y + 2, 3, 1)));
        }
        root.child(n)
    }
}
