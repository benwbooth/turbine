//! Indent guides, as VS Code draws them: a faint │ at each indent level
//! inside a line's leading whitespace; blank lines inside a block keep the
//! guides of the lines around them; the guide of the block the cursor is in
//! brighter.

use super::*;

/// How far a blank line looks for its neighbours' indent (lines).
const REACH: usize = 200;

impl EditorView {
    /// The indent step in columns (the file's: a tab, or its spaces), kept
    /// per version as it looks over the file's first lines.
    pub(super) fn indent_step(&mut self) -> usize {
        if self.guide_step.0 != self.doc.version || self.guide_step.1 == 0 {
            let u = self.indent_unit();
            let w = if u == "\t" { TAB_WIDTH } else { u.chars().count().max(1) };
            self.guide_step = (self.doc.version, w);
        }
        self.guide_step.1
    }

    /// A line's indent in columns, None for a blank line.
    pub(super) fn line_indent(&self, li: usize) -> Option<usize> {
        let text = self.doc.line(li);
        let mut col = 0;
        for c in text.chars() {
            match c {
                ' ' => col += 1,
                '\t' => col += TAB_WIDTH - col % TAB_WIDTH,
                '\n' | '\r' => return None,
                _ => return Some(col),
            }
        }
        None
    }

    /// The columns guides go at on line `li`: each step's start inside its
    /// indent (a blank line takes the smaller of its neighbours').
    pub(super) fn guide_cols(&mut self, li: usize) -> Vec<usize> {
        if !self.indent_guides {
            return Vec::new();
        }
        let step = self.indent_step();
        let indent = match self.line_indent(li) {
            Some(i) => i,
            None => {
                let n = self.doc.len_lines();
                let up = (1..=REACH.min(li)).find_map(|d| self.line_indent(li - d)).unwrap_or(0);
                let down = (1..REACH).take_while(|d| li + d < n).find_map(|d| self.line_indent(li + d)).unwrap_or(0);
                up.min(down)
            }
        };
        (0..indent).step_by(step).collect()
    }

    /// The cursor's block's guide: (column, first line, last line) — the
    /// level the cursor's line is inside, over the lines that keep it.
    pub(super) fn active_guide(&mut self) -> Option<(usize, usize, usize)> {
        if !self.indent_guides {
            return None;
        }
        let step = self.indent_step();
        let (line, _) = self.cursor_line_col();
        let ind = self.line_indent(line).unwrap_or(0);
        // (A line that opens a block: its inside's guide, as VS Code shows.)
        let next = (line + 1 < self.doc.len_lines()).then(|| self.line_indent(line + 1)).flatten().unwrap_or(0);
        let level_ind = if next > ind { next } else { ind };
        if level_ind < step {
            return None;
        }
        let col = (level_ind - 1) / step * step;
        let inside = |ed: &Self, l: usize| ed.line_indent(l).is_none_or(|i| i > col);
        let start = if next > ind { line + 1 } else { line };
        let mut first = start;
        while first > 0 && inside(self, first - 1) && line.abs_diff(first) < 2000 {
            first -= 1;
        }
        let mut last = start;
        while last + 1 < self.doc.len_lines() && inside(self, last + 1) && last.abs_diff(line) < 2000 {
            last += 1;
        }
        // (Blank lines at the block's end aren't in it.)
        while last > first && self.line_indent(last).is_none() {
            last -= 1;
        }
        Some((col, first, last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use turbine_text::document::Document;

    #[test]
    fn guides_at_each_level_and_through_blank_lines() {
        let mut ed = EditorView::new(Document::from_str("fn a() {\n    if x {\n        y();\n\n        z();\n    }\n}\n"));
        assert_eq!(ed.guide_cols(0), Vec::<usize>::new());
        assert_eq!(ed.guide_cols(1), vec![0]);
        assert_eq!(ed.guide_cols(2), vec![0, 4]);
        assert_eq!(ed.guide_cols(3), vec![0, 4], "a blank line keeps its block's guides");
        // The cursor in y(): the `if` block's guide (column 4), lines 2 to 4.
        ed.cursor = ed.doc.rope.line_to_char(2) + 9;
        assert_eq!(ed.active_guide(), Some((4, 2, 4)));
        // On `if x {`: its inside's guide.
        ed.cursor = ed.doc.rope.line_to_char(1) + 4;
        assert_eq!(ed.active_guide(), Some((4, 2, 4)));
    }
}
