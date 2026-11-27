//! Brackets as VS Code shows them: the pair at the cursor marked, and each
//! bracket coloured by how deep it is (rainbow: yellow, magenta, cyan, as
//! VS Code's gold, orchid and blue), a closer with no opener in red.
//! Brackets in strings and comments (by the highlighting) don't count.
//! The depths at each line's start are worked out once per version.

use super::*;
use crate::theme::vga;

const OPEN: [char; 3] = ['(', '[', '{'];
const CLOSE: [char; 3] = [')', ']', '}'];

/// The depth colours, in turn.
pub(super) const RAINBOW: [crate::theme::Color; 3] = [vga::YELLOW, vga::LIGHT_MAGENTA, vga::LIGHT_CYAN];

/// How far the pair is looked for (characters), so a huge file stays quick.
const PAIR_REACH: usize = 100_000;

#[derive(Default, Debug)]
pub struct BracketCache {
    /// (document version, highlighted lines) the depths are for.
    key: Option<(u64, usize)>,
    /// The depth at each line's start.
    depths: Vec<u32>,
    /// (cursor, version) → the pair at the cursor.
    pair_key: Option<(usize, u64, usize)>,
    pair: Option<(usize, usize)>,
}

impl EditorView {
    /// Whether the byte at `li`:`byte` is code (not in a string or comment).
    fn is_code(&self, li: usize, byte: usize) -> bool {
        let Some(spans) = self.highlights.get(li) else { return true };
        !spans.iter().any(|&(a, b, k)| (a as usize) <= byte && byte < b as usize && matches!(k, 1 | 2))
    }

    fn refresh_depths(&mut self) {
        let key = (self.doc.version, self.highlights.len());
        if self.brackets.key == Some(key) {
            return;
        }
        let n = self.doc.len_lines();
        let mut depths = Vec::with_capacity(n);
        let mut d: u32 = 0;
        for li in 0..n {
            depths.push(d);
            let text = self.doc.line(li);
            for (b, ch) in text.char_indices() {
                if OPEN.contains(&ch) && self.is_code(li, b) {
                    d += 1;
                } else if CLOSE.contains(&ch) && self.is_code(li, b) {
                    d = d.saturating_sub(1);
                }
            }
        }
        self.brackets.depths = depths;
        self.brackets.key = Some(key);
    }

    /// A line's brackets for drawing: (byte, colour); None as the colour
    /// for a closer with nothing open.
    pub(super) fn line_brackets(&mut self, li: usize, text: &str) -> Vec<(usize, Option<crate::theme::Color>)> {
        if !self.rainbow_brackets {
            return Vec::new();
        }
        self.refresh_depths();
        let mut d = self.brackets.depths.get(li).copied().unwrap_or(0) as usize;
        let mut out = Vec::new();
        for (b, ch) in text.char_indices() {
            if OPEN.contains(&ch) && self.is_code(li, b) {
                out.push((b, Some(RAINBOW[d % RAINBOW.len()])));
                d += 1;
            } else if CLOSE.contains(&ch) && self.is_code(li, b) {
                if d == 0 {
                    out.push((b, None));
                } else {
                    d -= 1;
                    out.push((b, Some(RAINBOW[d % RAINBOW.len()])));
                }
            }
        }
        out
    }

    /// Whether the character at `i` is a bracket in code: (its kind, opener).
    fn code_bracket(&self, i: usize) -> Option<(usize, bool)> {
        let r = &self.doc.rope;
        let c = r.char(i);
        let (k, open) = match OPEN.iter().position(|&o| o == c) {
            Some(k) => (k, true),
            None => (CLOSE.iter().position(|&x| x == c)?, false),
        };
        let li = r.char_to_line(i);
        let byte = r.char_to_byte(i) - r.line_to_byte(li);
        self.is_code(li, byte).then_some((k, open))
    }

    /// The bracket at (or just before) the cursor and its partner, as
    /// character positions.
    pub fn bracket_pair(&mut self) -> Option<(usize, usize)> {
        if !self.match_brackets {
            return None;
        }
        let key = (self.cursor, self.doc.version, self.highlights.len());
        if self.brackets.pair_key == Some(key) {
            return self.brackets.pair;
        }
        let pair = self.find_pair();
        self.brackets.pair_key = Some(key);
        self.brackets.pair = pair;
        pair
    }

    fn find_pair(&self) -> Option<(usize, usize)> {
        let n = self.doc.rope.len_chars();
        let (i, (k, open)) = [self.cursor, self.cursor.wrapping_sub(1)].into_iter().filter(|&i| i < n).find_map(|i| self.code_bracket(i).map(|b| (i, b)))?;
        let mut depth = 0i32;
        let mut j = i;
        for _ in 0..PAIR_REACH {
            if let Some((kk, o)) = self.code_bracket(j).filter(|(kk, _)| *kk == k) {
                let _ = kk;
                depth += if o == open { 1 } else { -1 };
                if depth == 0 {
                    return Some((i, j));
                }
            }
            if open {
                j += 1;
                if j >= n {
                    return None;
                }
            } else {
                j = j.checked_sub(1)?;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use turbine_text::document::Document;

    #[test]
    fn rainbow_depths_and_the_cursors_pair() {
        let mut ed = EditorView::new(Document::from_str("f(a[b]{c})\n  x)\n"));
        let line = ed.doc.line(0);
        let cols: Vec<_> = ed.line_brackets(0, &line).into_iter().map(|(_, c)| c.unwrap()).collect();
        let (y, m) = (RAINBOW[0], RAINBOW[1]);
        assert_eq!(cols, vec![y, m, m, m, m, y]);
        // (A closer with nothing open: red, drawn as None.)
        let line = ed.doc.line(1);
        assert_eq!(ed.line_brackets(1, &line), vec![(3, None)]);
        // The pair at the cursor: on the opener, or just after the closer.
        ed.cursor = 1;
        assert_eq!(ed.bracket_pair(), Some((1, 9)));
        ed.cursor = 6;
        assert_eq!(ed.bracket_pair(), Some((6, 8)));
        ed.cursor = 10;
        assert_eq!(ed.bracket_pair(), Some((9, 1)));
        ed.cursor = 2;
        assert_eq!(ed.bracket_pair(), Some((1, 9)));
    }

    #[test]
    fn brackets_in_strings_do_not_count() {
        let mut ed = EditorView::new(Document::from_str("f(\"(\", x)\n"));
        // (The string "(" at bytes 2..5.)
        ed.highlights = vec![vec![(2, 5, 1)]];
        let line = ed.doc.line(0);
        assert_eq!(ed.line_brackets(0, &line).iter().map(|b| b.0).collect::<Vec<_>>(), vec![1, 8]);
        ed.cursor = 1;
        assert_eq!(ed.bracket_pair(), Some((1, 8)));
    }
}
