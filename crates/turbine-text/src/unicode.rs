//! Graphemes, display width and bidirectional layout.

use unicode_bidi::{BidiInfo, Level};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Columns a grapheme cluster takes on a cell grid (0, 1 or 2).
pub fn grapheme_width(g: &str) -> usize {
    if g == "\t" {
        return 1;
    }
    match g.width() {
        0 if g.chars().next().is_some_and(|c| c.is_control()) => 1,
        w => w.min(2),
    }
}

/// Display width of a string.
pub fn str_width(s: &str) -> usize {
    s.graphemes(true).map(grapheme_width).sum()
}

/// Byte offsets of each grapheme boundary in `s`, including 0 and `s.len()`.
pub fn grapheme_boundaries(s: &str) -> Vec<usize> {
    let mut v: Vec<usize> = s.grapheme_indices(true).map(|(i, _)| i).collect();
    v.push(s.len());
    v
}

/// True when the text has characters of a right-to-left script.
pub fn has_rtl(s: &str) -> bool {
    s.chars().any(|c| {
        let u = c as u32;
        (0x0590..=0x08FF).contains(&u)
            || (0xFB1D..=0xFDFF).contains(&u)
            || (0xFE70..=0xFEFF).contains(&u)
            || (0x10800..=0x10FFF).contains(&u)
            || (0x1E800..=0x1EFFF).contains(&u)
            || matches!(u, 0x200F | 0x202B | 0x202E | 0x2067)
    })
}

/// One grapheme placed on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualGrapheme {
    /// Byte offset of the grapheme in the logical line.
    pub offset: usize,
    pub len: usize,
    /// True if it sits in a right-to-left run.
    pub rtl: bool,
}

/// Lay out one line in visual (left to right on screen) order using the
/// Unicode Bidirectional Algorithm. `base_rtl` forces an RTL paragraph;
/// `None` picks the direction from the first strong character.
pub fn visual_line(line: &str, base_rtl: Option<bool>) -> Vec<VisualGrapheme> {
    let logical = |rtl: bool| -> Vec<VisualGrapheme> {
        line.grapheme_indices(true)
            .map(|(i, g)| VisualGrapheme { offset: i, len: g.len(), rtl })
            .collect()
    };
    if line.is_empty() {
        return Vec::new();
    }
    if base_rtl != Some(true) && !has_rtl(line) {
        return logical(false);
    }
    let level = base_rtl.map(|r| if r { Level::rtl() } else { Level::ltr() });
    let info = BidiInfo::new(line, level);
    let Some(para) = info.paragraphs.first() else { return logical(false) };
    let (levels, runs) = info.visual_runs(para, para.range.clone());
    let mut out = Vec::with_capacity(line.len());
    for run in runs {
        let rtl = levels[run.start].is_rtl();
        let mut gs: Vec<VisualGrapheme> = line[run.clone()]
            .grapheme_indices(true)
            .map(|(i, g)| VisualGrapheme { offset: run.start + i, len: g.len(), rtl })
            .collect();
        if rtl {
            gs.reverse();
        }
        out.extend(gs);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths() {
        assert_eq!(str_width("abc"), 3);
        assert_eq!(str_width("日本"), 4);
        assert_eq!(str_width("e\u{301}"), 1);
        assert_eq!(str_width("👨‍👩‍👧"), 2);
    }

    #[test]
    fn bidi_reorders_hebrew() {
        let line = "ab שלום cd";
        let v = visual_line(line, None);
        let s: String = v.iter().map(|g| &line[g.offset..g.offset + g.len]).collect();
        assert_eq!(s, "ab םולש cd");
    }
}
