//! Diffs and patches shown as delta shows them: each file under a rule,
//! each hunk under its line number and the function it's in, removed and
//! added lines on red and green with the words that changed picked out,
//! code coloured by its language, line numbers in the margin; one after
//! another (unified) or side by side.

use crate::markdown::Rendered;
use std::path::{Path, PathBuf};
use turbine_text::unicode::str_width;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Context(String),
    Del(String),
    Add(String),
}

#[derive(Debug, Clone, Default)]
pub struct Hunk {
    pub old_start: usize,
    pub new_start: usize,
    /// What follows the second @@ (the function the hunk is in).
    pub context: String,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Default)]
pub struct FileDiff {
    pub old: String,
    pub new: String,
    /// "renamed", "new file", "deleted", "binary", ...
    pub note: String,
    pub hunks: Vec<Hunk>,
}

/// A commit's header in `git show` / `git log -p` output, kept as is.
#[derive(Debug, Clone)]
pub enum Part {
    Text(Vec<String>),
    File(FileDiff),
}

fn strip_ab(p: &str) -> String {
    let p = p.trim().trim_matches('"');
    let p = p.split('\t').next().unwrap_or(p);
    p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p).to_string()
}

fn range_start(s: &str) -> usize {
    s.trim_start_matches(['-', '+']).split(',').next().and_then(|n| n.parse().ok()).unwrap_or(1)
}

/// Read unified diff text (git diff, git show, diff -u, a .patch).
pub fn parse(text: &str) -> Vec<Part> {
    let mut parts: Vec<Part> = Vec::new();
    let mut file: Option<FileDiff> = None;
    let mut prose: Vec<String> = Vec::new();
    let flush_file = |file: &mut Option<FileDiff>, parts: &mut Vec<Part>| {
        if let Some(f) = file.take() {
            parts.push(Part::File(f));
        }
    };
    let mut lines = text.lines().peekable();
    while let Some(l) = lines.next() {
        if let Some(rest) = l.strip_prefix("diff --git ") {
            if !prose.is_empty() {
                parts.push(Part::Text(std::mem::take(&mut prose)));
            }
            flush_file(&mut file, &mut parts);
            let mut halves = rest.splitn(2, " b/");
            let old = strip_ab(halves.next().unwrap_or(""));
            let new = halves.next().map(|n| n.to_string()).unwrap_or_else(|| old.clone());
            file = Some(FileDiff { old, new, ..Default::default() });
            continue;
        }
        if l.starts_with("--- ") && lines.peek().is_some_and(|n| n.starts_with("+++ ")) {
            let old = strip_ab(&l[4..]);
            let new = strip_ab(&lines.next().unwrap()[4..]);
            match &mut file {
                Some(f) if f.hunks.is_empty() => {
                    if old != "/dev/null" {
                        f.old = old;
                    }
                    if new != "/dev/null" {
                        f.new = new;
                    }
                }
                _ => {
                    if !prose.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut prose)));
                    }
                    flush_file(&mut file, &mut parts);
                    let (o, n) = (if old == "/dev/null" { new.clone() } else { old }, if new == "/dev/null" { String::new() } else { new });
                    file = Some(FileDiff { new: if n.is_empty() { o.clone() } else { n }, old: o, ..Default::default() });
                }
            }
            continue;
        }
        if let (Some(f), Some(rest)) = (&mut file, l.strip_prefix("@@ ")) {
            let (ranges, ctx) = rest.split_once(" @@").unwrap_or((rest, ""));
            let mut r = ranges.split_whitespace();
            let old_start = r.next().map(range_start).unwrap_or(1);
            let new_start = r.next().map(range_start).unwrap_or(1);
            f.hunks.push(Hunk { old_start, new_start, context: ctx.trim().to_string(), lines: Vec::new() });
            continue;
        }
        if let Some(f) = &mut file {
            if let Some(h) = f.hunks.last_mut() {
                match l.chars().next() {
                    Some('+') => {
                        h.lines.push(Line::Add(l[1..].to_string()));
                        continue;
                    }
                    Some('-') => {
                        h.lines.push(Line::Del(l[1..].to_string()));
                        continue;
                    }
                    Some(' ') => {
                        h.lines.push(Line::Context(l[1..].to_string()));
                        continue;
                    }
                    None => {
                        h.lines.push(Line::Context(String::new()));
                        continue;
                    }
                    Some('\\') => continue, // "\ No newline at end of file"
                    _ => {}
                }
            } else {
                // Header lines before the first hunk.
                if l.starts_with("new file") {
                    f.note = "new file".into();
                } else if l.starts_with("deleted file") {
                    f.note = "deleted".into();
                } else if l.starts_with("rename from") {
                    f.note = "renamed".into();
                } else if l.starts_with("Binary files") {
                    f.note = "binary".into();
                }
                continue;
            }
            // (Anything else ends the file: a commit's header, say.)
            flush_file(&mut file, &mut parts);
        }
        prose.push(l.to_string());
    }
    flush_file(&mut file, &mut parts);
    if !prose.is_empty() {
        parts.push(Part::Text(prose));
    }
    parts
}

/// Byte ranges that changed between two lines (word by word).
fn tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<u8> = None;
    for (i, c) in s.char_indices() {
        // (Runs of letters/digits/_ and runs of spaces are one token; any
        // other character is its own.)
        let class = if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
        if i > start && (prev != Some(class) || class == 2) {
            out.push(&s[start..i]);
            start = i;
        }
        prev = Some(class);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

fn changed_words(old: &str, new: &str) -> (Vec<(u32, u32)>, Vec<(u32, u32)>) {
    let (ta, tb) = (tokens(old), tokens(new));
    let offs = |t: &[&str]| {
        let mut v = Vec::with_capacity(t.len() + 1);
        let mut n = 0u32;
        for x in t {
            v.push(n);
            n += x.len() as u32;
        }
        v.push(n);
        v
    };
    let (oa, ob) = (offs(&ta), offs(&tb));
    let (mut a, mut b): (Vec<(u32, u32)>, Vec<(u32, u32)>) = (Vec::new(), Vec::new());
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &ta, &tb) {
        match op {
            similar::DiffOp::Equal { .. } => {}
            similar::DiffOp::Delete { old_index, old_len, .. } => a.push((oa[old_index], oa[old_index + old_len])),
            similar::DiffOp::Insert { new_index, new_len, .. } => b.push((ob[new_index], ob[new_index + new_len])),
            similar::DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                a.push((oa[old_index], oa[old_index + old_len]));
                b.push((ob[new_index], ob[new_index + new_len]));
            }
        }
    }
    let blank = |s: &str, (x, y): &(u32, u32)| s[*x as usize..*y as usize].trim().is_empty();
    a.retain(|r| !blank(old, r));
    b.retain(|r| !blank(new, r));
    // (A line changed nearly all through: no words picked out.)
    let most = |v: &[(u32, u32)], len: usize| v.iter().map(|(x, y)| (y - x) as usize).sum::<usize>() * 10 > len.max(1) * 7;
    if most(&a, old.len()) && most(&b, new.len()) {
        return (Vec::new(), Vec::new());
    }
    (a, b)
}

/// Highlight a hunk's two sides (each as the code it reads as).
fn hunk_spans(lang: Option<&str>, lines: &[&str]) -> Vec<Vec<(u32, u32, u8)>> {
    let Some(lang) = lang else { return vec![Vec::new(); lines.len()] };
    turbine_syntax::highlight(lang, &lines.join("\n"), std::time::Duration::from_millis(200)).unwrap_or_else(|| vec![Vec::new(); lines.len()])
}

fn kind(n: &str) -> u8 {
    turbine_syntax::kind_index(n).unwrap_or(0)
}

struct Page {
    r: Rendered,
}

impl Page {
    fn push(&mut self, text: &str, spans: Vec<(u32, u32, u8)>, bg: u8, words: Vec<(u32, u32)>, place: Option<(PathBuf, usize)>) {
        self.r.text.push_str(text);
        self.r.text.push('\n');
        self.r.spans.push(spans);
        self.r.line_bg.push(bg);
        self.r.words.push(words);
        self.r.places.push(place);
    }
}

/// Fit text to `w` columns (cut, or padded).
fn fit(s: &str, w: usize) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = if c == '\t' { ' ' } else { c };
        if str_width(&out) + str_width(&c.to_string()) > w {
            break;
        }
        out.push(c);
    }
    let pad = w.saturating_sub(str_width(&out));
    out.push_str(&" ".repeat(pad));
    out
}

/// Render a diff. `root`: where its paths are from (to go to a line).
pub fn render(text: &str, width: usize, side_by_side: bool, root: &Path) -> Rendered {
    let width = width.max(40);
    let mut p = Page { r: Rendered::default() };
    let parts = parse(text);
    if parts.iter().all(|x| matches!(x, Part::Text(_))) {
        p.push("(No changes)", vec![(0, 12, kind("comment"))], 0, vec![], None);
        return p.r;
    }
    for part in parts {
        match part {
            Part::Text(lines) => {
                for l in lines {
                    // (A commit's header: its hash and its message.)
                    let k = if l.starts_with("commit ") { Some(kind("keyword")) } else if l.starts_with("Author:") || l.starts_with("Date:") { Some(kind("comment")) } else { None };
                    let spans = k.map(|k| vec![(0, l.len() as u32, k)]).unwrap_or_default();
                    p.push(&l, spans, 0, vec![], None);
                }
            }
            Part::File(f) => render_file(&mut p, &f, width, side_by_side, root),
        }
    }
    p.r
}

fn render_file(p: &mut Page, f: &FileDiff, width: usize, sbs: bool, root: &Path) {
    let lang = turbine_syntax::detect(Some(Path::new(&f.new)), "");
    let path = root.join(&f.new);
    let name = if f.old != f.new && !f.old.is_empty() { format!("{} ⟶ {}", f.old, f.new) } else { f.new.clone() };
    let title = if f.note.is_empty() { name } else { format!("{name}  ({})", f.note) };
    let rule = "─".repeat(width.min(str_width(&title) + 2).max(20));
    p.push("", vec![], 0, vec![], None);
    p.push(&title, vec![(0, title.len() as u32, kind("keyword"))], 0, vec![], Some((path.clone(), 0)));
    p.push(&rule, vec![(0, rule.len() as u32, kind("keyword"))], 0, vec![], None);
    for h in &f.hunks {
        // The hunk: its line and the function it's in, in a little box.
        let label = if h.context.is_empty() { format!("{}", h.new_start) } else { format!("{}: {}", h.new_start, h.context) };
        let label = fit(&label, str_width(&label).min(width - 4)).trim_end().to_string();
        let top = format!("┌{}┐", "─".repeat(str_width(&label) + 2));
        let mid = format!("│ {label} │");
        p.push(&top, vec![(0, top.len() as u32, kind("punctuation"))], 0, vec![], None);
        let lab_at = "│ ".len() as u32;
        p.push(&mid, vec![(0, lab_at, kind("punctuation")), (lab_at, lab_at + label.len() as u32, kind("function")), (lab_at + label.len() as u32, mid.len() as u32, kind("punctuation"))], 0, vec![], Some((path.clone(), h.new_start.saturating_sub(1))));
        p.push(&top.replace('┌', "└").replace('┐', "┘"), vec![(0, top.len() as u32, kind("punctuation"))], 0, vec![], None);
        // Both sides' code, highlighted as code.
        let old_side: Vec<&str> = h.lines.iter().filter_map(|l| match l {
            Line::Context(t) | Line::Del(t) => Some(t.as_str()),
            Line::Add(_) => None,
        }).collect();
        let new_side: Vec<&str> = h.lines.iter().filter_map(|l| match l {
            Line::Context(t) | Line::Add(t) => Some(t.as_str()),
            Line::Del(_) => None,
        }).collect();
        let old_hl = hunk_spans(lang.as_deref(), &old_side);
        let new_hl = hunk_spans(lang.as_deref(), &new_side);
        // Runs of removed then added lines pair up for their words.
        let mut i = 0;
        let (mut on, mut nn) = (h.old_start, h.new_start);
        let (mut oi, mut ni) = (0usize, 0usize);
        while i < h.lines.len() {
            if let Line::Context(t) = &h.lines[i] {
                let (os, ns) = (old_hl.get(oi).cloned().unwrap_or_default(), new_hl.get(ni).cloned().unwrap_or_default());
                emit(p, sbs, width, Some((on, t, os, vec![])), Some((nn, t, ns, vec![])), 0, &path);
                on += 1;
                nn += 1;
                oi += 1;
                ni += 1;
                i += 1;
                continue;
            }
            let dels: Vec<&String> = h.lines[i..].iter().take_while(|l| matches!(l, Line::Del(_))).map(|l| if let Line::Del(t) = l { t } else { unreachable!() }).collect();
            let j = i + dels.len();
            let adds: Vec<&String> = h.lines[j..].iter().take_while(|l| matches!(l, Line::Add(_))).map(|l| if let Line::Add(t) = l { t } else { unreachable!() }).collect();
            i = j + adds.len();
            let pairs = dels.len().max(adds.len());
            let words: Vec<(Vec<(u32, u32)>, Vec<(u32, u32)>)> = (0..pairs).map(|k| match (dels.get(k), adds.get(k)) {
                (Some(d), Some(a)) => changed_words(d, a),
                _ => (vec![], vec![]),
            }).collect();
            if sbs {
                for k in 0..pairs {
                    let left = dels.get(k).map(|t| (on + k, t.as_str(), old_hl.get(oi + k).cloned().unwrap_or_default(), words[k].0.clone()));
                    let right = adds.get(k).map(|t| (nn + k, t.as_str(), new_hl.get(ni + k).cloned().unwrap_or_default(), words[k].1.clone()));
                    emit(p, true, width, left, right, 3, &path);
                }
            } else {
                for (k, t) in dels.iter().enumerate() {
                    emit(p, false, width, Some((on + k, t.as_str(), old_hl.get(oi + k).cloned().unwrap_or_default(), words[k].0.clone())), None, 2, &path);
                }
                for (k, t) in adds.iter().enumerate() {
                    emit(p, false, width, None, Some((nn + k, t.as_str(), new_hl.get(ni + k).cloned().unwrap_or_default(), words[k].1.clone())), 1, &path);
                }
            }
            on += dels.len();
            oi += dels.len();
            nn += adds.len();
            ni += adds.len();
        }
    }
}

type Side<'a> = Option<(usize, &'a str, Vec<(u32, u32, u8)>, Vec<(u32, u32)>)>;

/// One line out: unified (`bg` 1 added, 2 removed, 0 context) or a
/// side-by-side row (bg 3: a changed pair; the halves' colours come from
/// their words and spans).
fn emit(p: &mut Page, sbs: bool, width: usize, old: Side, new: Side, bg: u8, path: &Path) {
    let gutter = kind("comment");
    let num = |n: Option<usize>| n.map(|n| format!("{n:>5}")).unwrap_or_else(|| "     ".into());
    if !sbs {
        // " 12   13 │ code"
        let (on, nn) = (old.as_ref().map(|o| o.0), new.as_ref().map(|n| n.0));
        let (text, spans, words) = match (&old, &new) {
            (_, Some((_, t, s, w))) if bg != 2 => (*t, s.clone(), w.clone()),
            (Some((_, t, s, w)), _) => (*t, s.clone(), w.clone()),
            (None, Some((_, t, s, w))) => (*t, s.clone(), w.clone()),
            _ => ("", vec![], vec![]),
        };
        let mark = match bg {
            1 => "+",
            2 => "-",
            _ => " ",
        };
        let head = format!("{} {} │{mark}", num(on), num(nn));
        let base = head.len() as u32;
        let line = format!("{head}{}", text.replace('\t', "    "));
        let mut sp = vec![(0, base - 1, gutter)];
        // (Tabs widened: spans past one move; good enough for code with few.)
        sp.extend(spans.into_iter().map(|(a, b, k)| (a + base, b + base, k)));
        let w = words.into_iter().map(|(a, b)| (a + base, b + base)).collect();
        let place = nn.or(on).map(|n| (path.to_path_buf(), n.saturating_sub(1)));
        p.push(&line, sp, bg, w, place);
        return;
    }
    // Side by side: "  12 old…    │   13 new…"
    let half = (width - 1) / 2;
    let text_w = half.saturating_sub(7);
    let mut line = String::new();
    let mut sp = Vec::new();
    let mut words = Vec::new();
    let side = |line: &mut String, sp: &mut Vec<(u32, u32, u8)>, words: &mut Vec<(u32, u32)>, s: &Side, del: bool| {
        let start = line.len() as u32;
        line.push_str(&num(s.as_ref().map(|x| x.0)));
        line.push_str(if s.is_none() { "  " } else if bg == 3 { if del { " -" } else { " +" } } else { "  " });
        sp.push((start, line.len() as u32, gutter));
        let base = line.len() as u32;
        let t = s.as_ref().map(|x| x.1).unwrap_or("");
        let shown = fit(t, text_w);
        let cut = shown.trim_end().len().max(t.len().min(shown.len())) as u32;
        line.push_str(&shown);
        if let Some((_, _, spans, w)) = s {
            sp.extend(spans.iter().filter(|(a, _, _)| *a < cut).map(|(a, b, k)| (a + base, (*b).min(cut) + base, *k)));
            if bg == 3 {
                // (A changed pair: its whole text marked when nothing finer.)
                if w.is_empty() {
                    words.push((base, base + shown.trim_end().len() as u32));
                } else {
                    words.extend(w.iter().filter(|(a, _)| *a < cut).map(|(a, b)| (a + base, (*b).min(cut) + base)));
                }
            }
        }
    };
    side(&mut line, &mut sp, &mut words, &old, true);
    let bar = line.len() as u32;
    line.push('│');
    sp.push((bar, line.len() as u32, kind("punctuation")));
    side(&mut line, &mut sp, &mut words, &new, false);
    let place = new.as_ref().or(old.as_ref()).map(|x| (path.to_path_buf(), x.0.saturating_sub(1)));
    // (Side by side: a changed row's colour is its words', the left red
    // where only removed, the right green where only added.)
    let row_bg = match (bg, old.is_some(), new.is_some()) {
        (3, true, false) => 2,
        (3, false, true) => 1,
        (3, true, true) => 1,
        _ => 0,
    };
    p.push(line.trim_end(), sp, if bg == 3 && old.is_some() && new.is_some() { 0 } else { row_bg }, words, place);
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "commit abc123\nAuthor: A <a@x>\n\n    Fix it\n\ndiff --git a/src/m.rs b/src/m.rs\nindex 1..2 100644\n--- a/src/m.rs\n+++ b/src/m.rs\n@@ -1,4 +1,4 @@ fn main() {\n fn main() {\n-    let x = 1;\n+    let x = 2;\n     go();\n }\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\n";

    #[test]
    fn parses_files_and_hunks() {
        let parts = parse(PATCH);
        let files: Vec<&FileDiff> = parts.iter().filter_map(|p| if let Part::File(f) = p { Some(f) } else { None }).collect();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].new, "src/m.rs");
        assert_eq!(files[0].hunks[0].context, "fn main() {");
        assert_eq!(files[0].hunks[0].lines[1], Line::Del("    let x = 1;".into()));
        assert_eq!(files[1].note, "new file");
        assert!(matches!(&parts[0], Part::Text(t) if t[0] == "commit abc123"));
    }

    #[test]
    fn unified_and_side_by_side() {
        let r = render(PATCH, 80, false, Path::new("/p"));
        let lines: Vec<&str> = r.text.lines().collect();
        let del = lines.iter().position(|l| l.ends_with("-    let x = 1;")).unwrap();
        assert_eq!(r.line_bg[del], 2);
        assert_eq!(r.line_bg[del + 1], 1);
        assert!(lines[del].starts_with("    2       │-"), "{:?}", lines[del]);
        // Only the 1 → 2 changed.
        let w = &r.words[del + 1];
        assert_eq!(w.len(), 1);
        assert_eq!(&lines[del + 1][w[0].0 as usize..w[0].1 as usize], "2");
        assert_eq!(r.places[del + 1], Some((PathBuf::from("/p/src/m.rs"), 1)));
        assert!(lines.contains(&"src/m.rs") && lines.contains(&"new.txt  (new file)"));
        assert!(lines.iter().any(|l| l.contains("│ 1: fn main() { │")));
        let s = render(PATCH, 80, true, Path::new("/p"));
        let row = s.text.lines().find(|l| l.contains("let x = 1;")).unwrap();
        assert!(row.contains("let x = 2;") && row.contains('│'), "{row}");
        assert!(s.text.lines().all(|l| str_width(l) <= 80));
    }
}
