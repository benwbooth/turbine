//! The text being written: the cursor and selection (byte offsets), undo,
//! the kill ring, and the moves ComposeBox makes: words as VS Code has them,
//! indentation in levels (spaces, tabs or mixed), and the brackets and quotes
//! typed in pairs (only those it typed are typed over or removed together).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IndentMode {
    Spaces,
    Tabs,
    Mixed,
}

#[derive(Clone)]
pub struct Indent {
    pub mode: IndentMode,
    pub size: usize,
}

impl Indent {
    pub fn tab_width(&self) -> usize {
        if self.mode == IndentMode::Mixed {
            8
        } else {
            self.size
        }
    }
    pub fn unit(&self) -> String {
        if self.mode == IndentMode::Tabs {
            "\t".into()
        } else {
            " ".repeat(self.size)
        }
    }
    /// Leading whitespace → its width in columns.
    pub fn cols_of(&self, ws: &str) -> usize {
        let tw = self.tab_width().max(1);
        let mut c = 0;
        for ch in ws.chars() {
            match ch {
                '\t' => c += tw - c % tw,
                ' ' => c += 1,
                _ => break,
            }
        }
        c
    }
    pub fn indent_for(&self, cols: usize) -> String {
        match self.mode {
            IndentMode::Spaces => " ".repeat(cols),
            m => {
                let w = if m == IndentMode::Mixed { 8 } else { self.size.max(1) };
                "\t".repeat(cols / w) + &" ".repeat(cols % w)
            }
        }
    }
}

#[derive(Clone)]
struct Snap {
    text: String,
    cursor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub open: usize,
    pub close: usize,
}

pub struct Text {
    pub text: String,
    pub cursor: usize,
    /// The selection's other end (None: no selection).
    pub anchor: Option<usize>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    /// (Typing coalesces into one undo step until something else happens.)
    typing: bool,
    pub kill_ring: String,
    pub pairs: Vec<Pair>,
}

pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn is_space(c: char) -> bool {
    c == ' ' || c == '\t'
}

impl Text {
    pub fn new() -> Text {
        Text { text: String::new(), cursor: 0, anchor: None, undo: vec![], redo: vec![], typing: false, kill_ring: String::new(), pairs: vec![] }
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    // ---- positions ----
    pub fn prev(&self, p: usize) -> usize {
        self.text[..p].chars().next_back().map(|c| p - c.len_utf8()).unwrap_or(0)
    }
    pub fn next(&self, p: usize) -> usize {
        self.text[p..].chars().next().map(|c| p + c.len_utf8()).unwrap_or(p)
    }
    pub fn char_at(&self, p: usize) -> Option<char> {
        self.text.get(p..).and_then(|s| s.chars().next())
    }
    pub fn char_before(&self, p: usize) -> Option<char> {
        self.text[..p].chars().next_back()
    }
    pub fn line_start(&self, p: usize) -> usize {
        self.text[..p].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }
    pub fn line_end(&self, p: usize) -> usize {
        self.text[p..].find('\n').map(|i| p + i).unwrap_or(self.text.len())
    }
    pub fn line_of(&self, p: usize) -> usize {
        self.text[..p].matches('\n').count()
    }
    pub fn indent_at(&self, p: usize) -> &str {
        let a = self.line_start(p);
        let s = &self.text[a..];
        &s[..s.len() - s.trim_start_matches([' ', '\t']).len()]
    }
    /// As VS Code moves by word: past spaces, then one run (word characters
    /// or punctuation), never over a line's end.
    pub fn word_right(&self, mut p: usize) -> usize {
        let s = p;
        while self.char_at(p).is_some_and(is_space) {
            p = self.next(p);
        }
        if self.char_at(p) == Some('\n') {
            return if p > s { p } else { p + 1 };
        }
        if self.char_at(p).is_some_and(is_word) {
            while self.char_at(p).is_some_and(is_word) {
                p = self.next(p);
            }
        } else {
            while self.char_at(p).is_some_and(|c| !is_word(c) && !is_space(c) && c != '\n') {
                p = self.next(p);
            }
        }
        p
    }
    pub fn word_left(&self, mut p: usize) -> usize {
        let s = p;
        while self.char_before(p).is_some_and(is_space) {
            p = self.prev(p);
        }
        if self.char_before(p) == Some('\n') {
            return if p < s { p } else { p - 1 };
        }
        if self.char_before(p).is_some_and(is_word) {
            while self.char_before(p).is_some_and(is_word) {
                p = self.prev(p);
            }
        } else {
            while self.char_before(p).is_some_and(|c| !is_word(c) && !is_space(c) && c != '\n') {
                p = self.prev(p);
            }
        }
        p
    }
    /// The word around p (for a double click): [a, b).
    pub fn word_around(&self, p: usize) -> (usize, usize) {
        let (mut a, mut b) = (p, p);
        let w = self.char_at(p).or(self.char_before(p)).map(is_word).unwrap_or(false);
        let same = |c: char| if w { is_word(c) } else { !is_word(c) && !c.is_whitespace() };
        while self.char_before(a).is_some_and(same) {
            a = self.prev(a);
        }
        while self.char_at(b).is_some_and(same) {
            b = self.next(b);
        }
        (a, b)
    }

    // ---- the selection ----
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.cursor).then(|| (a.min(self.cursor), a.max(self.cursor)))
    }
    pub fn selected(&self) -> &str {
        self.selection().map(|(a, b)| &self.text[a..b]).unwrap_or("")
    }
    /// Move the cursor; with `select`, the selection grows from where it was.
    pub fn move_to(&mut self, p: usize, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = p.min(self.text.len());
        self.typing = false;
        self.prune_pairs();
    }
    pub fn select(&mut self, a: usize, b: usize) {
        self.anchor = Some(a);
        self.cursor = b;
        self.typing = false;
    }

    // ---- changes ----
    fn snapshot(&mut self) {
        self.undo.push(Snap { text: self.text.clone(), cursor: self.cursor });
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    /// Replace [a, b) with s (the cursor after it). Pairs typed for you move
    /// with the text around them.
    pub fn replace(&mut self, a: usize, b: usize, s: &str) {
        let coalesce = self.typing && a == b && a == self.cursor && !s.contains('\n') && s.chars().count() == 1;
        if !coalesce {
            self.snapshot();
        }
        self.text.replace_range(a..b, s);
        let delta = s.len() as isize - (b - a) as isize;
        self.pairs = self
            .pairs
            .iter()
            .filter_map(|q| {
                let m = |x: usize| if x < a { Some(x) } else if x >= b { Some((x as isize + delta) as usize) } else { None };
                match (m(q.open), m(q.close)) {
                    (Some(o), Some(c)) if c > o => Some(Pair { open: o, close: c }),
                    _ => None,
                }
            })
            .collect();
        self.cursor = a + s.len();
        self.anchor = None;
        self.typing = coalesce || (a == b && s.chars().count() == 1 && s != "\n");
    }
    pub fn insert(&mut self, s: &str) {
        if let Some((a, b)) = self.selection() {
            self.replace(a, b, s);
        } else {
            let c = self.cursor;
            self.replace(c, c, s);
        }
    }
    pub fn set(&mut self, s: &str) {
        self.snapshot();
        self.text = s.to_string();
        self.cursor = s.len();
        self.anchor = None;
        self.pairs.clear();
        self.typing = false;
    }
    /// Cleared after a run: no undo back into the last command.
    pub fn reset(&mut self) {
        *self = Text { kill_ring: std::mem::take(&mut self.kill_ring), ..Text::new() };
    }
    pub fn kill(&mut self, a: usize, b: usize, append: bool) {
        let (a, b) = (a.min(b), a.max(b));
        let cut = self.text[a..b].to_string();
        if append {
            self.kill_ring.push_str(&cut);
        } else {
            self.kill_ring = cut;
        }
        self.replace(a, b, "");
    }
    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(Snap { text: self.text.clone(), cursor: self.cursor });
            self.text = s.text;
            self.cursor = s.cursor.min(self.text.len());
            self.anchor = None;
            self.pairs.clear();
        }
        self.typing = false;
    }
    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(Snap { text: self.text.clone(), cursor: self.cursor });
            self.text = s.text;
            self.cursor = s.cursor.min(self.text.len());
            self.anchor = None;
        }
        self.typing = false;
    }

    // ---- pairs typed for you ----
    pub fn prune_pairs(&mut self) {
        let c = self.cursor;
        self.pairs.retain(|q| c > q.open && c <= q.close);
    }
    pub fn pair_closing_at(&self, close: usize) -> Option<usize> {
        self.pairs.iter().position(|q| q.close == close)
    }

    // ---- indentation ----
    /// Replace the leading whitespace of the line at `a` with `cols` columns'.
    pub fn set_indent(&mut self, a: usize, cols: usize, ind: &Indent) -> isize {
        let end = self.line_end(a);
        let line = &self.text[a..end];
        let m = line.len() - line.trim_start_matches([' ', '\t']).len();
        let ws = ind.indent_for(cols);
        if ws == self.text[a..a + m] {
            return 0;
        }
        let keep = self.cursor;
        self.replace(a, a + m, &ws);
        let d = ws.len() as isize - m as isize;
        self.cursor = if keep >= a + m { (keep as isize + d) as usize } else if keep > a { a + ws.len().min(keep - a) } else { keep };
        d
    }
    /// Indent (1) or outdent (-1) the line or every selected one, onto the
    /// levels (3 columns at size 2 go to 4, or 2).
    pub fn shift_lines(&mut self, dir: i32, ind: &Indent) {
        let (s, e) = self.selection().unwrap_or((self.cursor, self.cursor));
        let a = self.line_start(s);
        let b = self.line_end(e);
        let lines: Vec<String> = self.text[a..b].split('\n').map(String::from).collect();
        let size = ind.size.max(1);
        let out: Vec<String> = lines
            .iter()
            .map(|l| {
                let m = l.len() - l.trim_start_matches([' ', '\t']).len();
                if l.trim().is_empty() && dir > 0 {
                    return l.clone();
                }
                let c = ind.cols_of(&l[..m]);
                let to = if dir > 0 { c / size * size + size } else { (c.div_ceil(size) * size).saturating_sub(size) };
                ind.indent_for(to) + &l[m..]
            })
            .collect();
        let block = out.join("\n");
        let pos = self.cursor;
        let delta = out[0].len() as isize - lines[0].len() as isize;
        let had_sel = self.selection().is_some();
        self.replace(a, b, &block);
        if had_sel {
            self.select(a, a + block.len());
        } else {
            self.cursor = ((pos as isize + delta).max(a as isize)) as usize;
        }
    }
    // ---- lines and columns, for vi and Ex (a column counts chars) ----
    pub fn line_count(&self) -> usize {
        self.text.matches('\n').count() + 1
    }
    /// Line n's start (past the last line: the last line's).
    pub fn pos_of_line(&self, n: usize) -> usize {
        let mut p = 0;
        for _ in 0..n {
            match self.text[p..].find('\n') {
                Some(i) => p += i + 1,
                None => return p,
            }
        }
        p
    }
    pub fn first_non_blank(&self, p: usize) -> usize {
        let mut i = self.line_start(p);
        let e = self.line_end(p);
        let b = self.text.as_bytes();
        while i < e && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        i
    }
    pub fn col_of(&self, p: usize) -> usize {
        self.text[self.line_start(p)..p].chars().count()
    }
    /// The position `col` chars into the line starting at `ls` (at most its end).
    pub fn pos_at_col(&self, ls: usize, col: usize) -> usize {
        let e = self.line_end(ls);
        self.fwd(ls, col, e)
    }
    /// n chars on from p, not past `limit`.
    pub fn fwd(&self, mut p: usize, n: usize, limit: usize) -> usize {
        for _ in 0..n {
            if p >= limit {
                break;
            }
            p = self.next(p);
        }
        p
    }
    /// n chars back from p, not before `limit`.
    pub fn back(&self, mut p: usize, n: usize, limit: usize) -> usize {
        for _ in 0..n {
            if p <= limit {
                break;
            }
            p = self.prev(p);
        }
        p
    }
    /// A position kept from before an edit, made safe: in the text, on a char.
    pub fn clamp_pos(&self, p: usize) -> usize {
        let mut p = p.min(self.text.len());
        while !self.text.is_char_boundary(p) {
            p -= 1;
        }
        p
    }

    /// Inside a string on this line, before p (a quote opened, not closed).
    pub fn in_string(&self, p: usize) -> bool {
        let a = self.line_start(p);
        let mut q: Option<char> = None;
        let mut it = self.text[a..p].chars();
        while let Some(c) = it.next() {
            match q {
                Some(open) => {
                    if c == '\\' {
                        it.next();
                    } else if c == open {
                        q = None;
                    }
                }
                None => {
                    if c == '"' || c == '\'' || c == '`' {
                        q = Some(c);
                    } else if c == '#' {
                        break;
                    }
                }
            }
        }
        q.is_some()
    }
}

/// What the text is indented with, as VS Code detects it (None: no indented
/// lines yet).
pub fn detect_indent(text: &str, default_size: usize) -> Option<Indent> {
    let (mut tabs, mut spaces) = (0, 0);
    let mut steps = std::collections::BTreeMap::new();
    let mut prev = 0usize;
    for l in text.split('\n').filter(|l| !l.trim().is_empty()) {
        let ws = &l[..l.len() - l.trim_start_matches([' ', '\t']).len()];
        if ws.contains('\t') {
            tabs += 1;
        } else if !ws.is_empty() {
            spaces += 1;
        }
        if !ws.contains('\t') {
            let c = ws.len();
            if c > prev && c - prev <= 8 {
                *steps.entry(c - prev).or_insert(0) += 1;
            }
            prev = c;
        }
    }
    if tabs == 0 && spaces == 0 {
        return None;
    }
    let (mut size, mut best) = (0, 0);
    for (k, n) in steps {
        if n > best {
            best = n;
            size = k;
        }
    }
    if tabs > 0 && spaces == 0 {
        return Some(Indent { mode: IndentMode::Tabs, size: default_size });
    }
    if size == 0 {
        return None;
    }
    Some(Indent { mode: if tabs > 0 { IndentMode::Mixed } else { IndentMode::Spaces }, size })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn words_and_undo() {
        let mut t = Text::new();
        for c in "print \"${x}\"".chars() {
            t.insert(&c.to_string());
        }
        assert_eq!(t.word_left(t.len()), t.len() - 2);
        assert_eq!(t.word_right(0), 5);
        t.undo();
        assert_eq!(t.text, "");
        t.redo();
        assert_eq!(t.text, "print \"${x}\"");
    }
    #[test]
    fn levels() {
        let ind = Indent { mode: IndentMode::Spaces, size: 2 };
        let mut t = Text::new();
        t.set("   x");
        t.cursor = 3;
        t.shift_lines(1, &ind);
        assert_eq!(t.text, "    x");
        t.shift_lines(-1, &ind);
        assert_eq!(t.text, "  x");
        assert_eq!(detect_indent("if a\n    b\n    c", 2).unwrap().size, 4);
    }
}
