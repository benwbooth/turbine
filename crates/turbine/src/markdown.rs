//! Markdown rendered for a text screen, as Tessera's guide viewer shows a
//! guide: headings set off, paragraphs wrapped, lists with bullets, code
//! blocks highlighted in a box, tables drawn with box characters, quotes
//! with a bar, links marked (and kept, to follow).
//!
//! The result is plain text plus highlight spans (Turbine's highlight
//! kinds), so a read-only editor shows it: scrolling, Find, selecting and
//! copying all work. It can take a while for a big guide: render it off
//! the UI thread.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use turbine_text::unicode::str_width;

/// A link in the rendered text: (line, start byte, end byte, target).
pub type Link = (usize, usize, usize, String);

#[derive(Default, Debug, Clone)]
pub struct Rendered {
    pub text: String,
    /// Spans per line: (start byte, end byte, kind index).
    pub spans: Vec<Vec<(u32, u32, u8)>>,
    pub links: Vec<Link>,
    /// Headings: (line, level, text), for the contents.
    pub headings: Vec<(usize, u8, String)>,
    /// Anchors ("#some-heading" targets): (slug, line).
    pub anchors: Vec<(String, usize)>,
    /// A diff's: each line's background (1 added, 2 removed, 3 hunk), the
    /// words changed in it, and the place in its file each line stands for.
    pub line_bg: Vec<u8>,
    pub words: Vec<Vec<(u32, u32)>>,
    pub places: Vec<Option<(std::path::PathBuf, usize)>>,
}

fn kind(name: &str) -> u8 {
    turbine_syntax::kind_index(name).unwrap_or(0)
}

/// A heading's anchor, GitHub's way.
pub fn slug(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { Some(c) } else if c == ' ' { Some('-') } else { None })
        .collect()
}

struct Out {
    r: Rendered,
    width: usize,
    /// The line being built: its text, spans and links.
    line: String,
    spans: Vec<(u32, u32, u8)>,
    links: Vec<(usize, usize, String)>,
    /// What every line of this block starts with (quote bars, list
    /// indents), and the first line's own (a bullet).
    prefix: Vec<(String, Option<u8>)>,
    first_prefix: Option<(String, Option<u8>)>,
    /// The next word sticks to what's before it (after a link's ◄).
    line_nospace: bool,
}

impl Out {
    fn new(width: usize) -> Out {
        Out { r: Rendered::default(), width: width.max(20), line: String::new(), spans: Vec::new(), links: Vec::new(), prefix: Vec::new(), first_prefix: None, line_nospace: false }
    }

    fn lines(&self) -> usize {
        self.r.spans.len()
    }

    /// Start the line with its prefixes.
    fn begin(&mut self) {
        if !self.line.is_empty() {
            return;
        }
        let parts: Vec<(String, Option<u8>)> = match self.first_prefix.take() {
            Some(first) => {
                let mut p: Vec<_> = self.prefix[..self.prefix.len().saturating_sub(1)].to_vec();
                p.push(first);
                p
            }
            None => self.prefix.clone(),
        };
        for (t, k) in parts {
            let a = self.line.len();
            self.line.push_str(&t);
            if let Some(k) = k {
                self.spans.push((a as u32, self.line.len() as u32, k));
            }
        }
    }

    fn prefix_width(&self) -> usize {
        self.prefix.iter().map(|(t, _)| str_width(t)).sum()
    }

    fn end_line(&mut self) {
        let n = self.lines();
        for (a, b, t) in self.links.drain(..) {
            self.r.links.push((n, a, b, t));
        }
        self.r.text.push_str(self.line.trim_end());
        self.r.text.push('\n');
        let len = self.line.trim_end().len() as u32;
        let spans = std::mem::take(&mut self.spans).into_iter().map(|(a, b, k)| (a.min(len), b.min(len), k)).filter(|(a, b, _)| b > a).collect();
        self.r.spans.push(spans);
        self.line.clear();
    }

    fn blank(&mut self) {
        if !self.line.is_empty() {
            self.end_line();
        }
        if !self.r.text.is_empty() && !self.r.text.ends_with("\n\n") {
            // (A quote's blank line keeps its bar.)
            let bars: String = self.prefix.iter().map(|(t, _)| if t.contains('▌') { t.clone() } else { " ".repeat(str_width(t)) }).collect();
            self.line = bars.trim_end().to_string();
            if !self.line.is_empty() {
                self.spans.push((0, self.line.len() as u32, kind("comment")));
            }
            self.end_line();
        }
    }

    /// Words, wrapped at the width.
    fn words(&mut self, text: &str, k: Option<u8>, link: Option<&str>) {
        let glue = std::mem::take(&mut self.line_nospace);
        for (i, word) in text.split(' ').enumerate() {
            if i > 0 {
                self.space();
            }
            let glued = glue && i == 0;
            if word.is_empty() {
                continue;
            }
            self.begin();
            let w = str_width(word);
            if !glued && str_width(&self.line) + w > self.width && str_width(&self.line) > self.prefix_width() {
                self.end_line();
                self.begin();
            }
            let a = self.line.len();
            self.line.push_str(word);
            if let Some(k) = k {
                self.spans.push((a as u32, self.line.len() as u32, k));
            }
            // Key combinations (Ctrl+S, F5, Alt+F1) in their own colour, in
            // text, bold and code alike (not links or headings).
            if link.is_none() && k.is_none_or(|k| k == kind("string") || k == kind("constant")) {
                if let Some((ka, kb)) = key_combo(word) {
                    self.spans.push(((a + ka) as u32, (a + kb) as u32, kind("escape")));
                }
            }
            if let Some(t) = link {
                self.links.push((a, self.line.len(), t.to_string()));
            }
        }
    }

    /// Text stuck to what's before it (a link's ◄ and ►): no space, no wrap.
    fn attach(&mut self, text: &str, k: u8) {
        self.begin();
        let a = self.line.len();
        self.line.push_str(text);
        self.spans.push((a as u32, self.line.len() as u32, k));
    }

    fn space(&mut self) {
        if !self.line.is_empty() && !self.line.ends_with(' ') && str_width(&self.line) < self.width {
            self.line.push(' ');
        }
    }

    fn raw_line(&mut self, text: &str, spans: Vec<(u32, u32, u8)>) {
        self.begin();
        let base = self.line.len() as u32;
        self.line.push_str(text);
        self.spans.extend(spans.into_iter().map(|(a, b, k)| (a + base, b + base, k)));
        self.end_line();
    }
}

/// Render Markdown for a screen `width` columns wide.
/// Where in a word a key combination is (punctuation around it left out):
/// modifiers joined by + to a key (Ctrl+Shift+P, Alt+F1, Ctrl+`), or a
/// key alone that's never a word (F1-F12, Esc, PgUp, PgDn).
fn key_combo(word: &str) -> Option<(usize, usize)> {
    let start = word.find(|c: char| !matches!(c, '(' | '"' | '\'' | '[' | '“'))?;
    let mut end = word.len();
    while end > start {
        let c = word[..end].chars().next_back()?;
        if matches!(c, ',' | '.' | ';' | ':' | ')' | '"' | '\'' | ']' | '”' | '!' | '?') && end - c.len_utf8() > start {
            end -= c.len_utf8();
        } else {
            break;
        }
    }
    let w = &word[start..end];
    let lone = |k: &str| {
        let lower = k.to_ascii_lowercase();
        matches!(lower.as_str(), "esc" | "pgup" | "pgdn") || (lower.len() >= 2 && lower.starts_with('f') && lower[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)))
    };
    let key = |k: &str| {
        lone(k)
            || k.chars().count() == 1
            || matches!(
                k.to_ascii_lowercase().as_str(),
                "enter" | "return" | "tab" | "space" | "backspace" | "delete" | "del" | "ins" | "insert" | "home" | "end" | "up" | "down" | "left" | "right" | "plus" | "minus"
            )
    };
    let modifier = |m: &str| matches!(m.to_ascii_lowercase().as_str(), "ctrl" | "alt" | "shift" | "cmd" | "super" | "meta" | "win");
    let ok = match w.rsplit_once('+') {
        // ("Ctrl++": the + key.)
        Some((mods, "")) if mods.ends_with('+') => mods[..mods.len() - 1].split('+').all(modifier),
        Some((mods, k)) => !mods.is_empty() && mods.split('+').all(modifier) && key(k),
        None => lone(w),
    };
    ok.then_some((start, end))
}

pub fn render(md: &str, width: usize) -> Rendered {
    let mut o = Out::new(width);
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_HEADING_ATTRIBUTES;
    // Inline style: a stack of kinds; the innermost wins.
    let mut style: Vec<u8> = Vec::new();
    let mut link: Vec<String> = Vec::new();
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut heading: Option<(u8, String, usize)> = None;
    let mut code: Option<(String, String)> = None; // (language, text)
    let mut table: Option<(Vec<Alignment>, Vec<Vec<String>>, Vec<String>, String)> = None;
    let mut image: Option<String> = None;
    let mut link_open = false;
    for ev in Parser::new_ext(md, opts) {
        // Tables are gathered, then drawn whole.
        if let Some((_, rows, row, cell)) = &mut table {
            match &ev {
                Event::Text(t) | Event::Code(t) => {
                    cell.push_str(t);
                    continue;
                }
                Event::SoftBreak | Event::HardBreak => {
                    cell.push(' ');
                    continue;
                }
                Event::End(TagEnd::TableCell) => {
                    row.push(std::mem::take(cell).trim().to_string());
                    continue;
                }
                Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                    rows.push(std::mem::take(row));
                    continue;
                }
                Event::End(TagEnd::Table) => {}
                _ => continue,
            }
        }
        if let Some((_, text)) = &mut code {
            match &ev {
                Event::Text(t) => {
                    text.push_str(t);
                    continue;
                }
                Event::End(TagEnd::CodeBlock) => {}
                _ => continue,
            }
        }
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                o.blank();
                let n = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                heading = Some((n, String::new(), o.lines()));
                style.push(kind(if n <= 2 { "keyword" } else { "type" }));
            }
            Event::End(TagEnd::Heading(_)) => {
                style.pop();
                if let Some((n, text, line)) = heading.take() {
                    let w = str_width(text.trim()).min(o.width);
                    if !o.line.is_empty() {
                        o.end_line();
                    }
                    let _ = w;
                    o.r.headings.push((line, n, text.trim().to_string()));
                    o.r.anchors.push((slug(&text), line));
                    o.blank();
                }
            }
            Event::Start(Tag::Paragraph) => {
                if lists.is_empty() || o.first_prefix.is_none() {
                    if o.line.trim().is_empty() {
                        o.line.clear();
                    }
                    if lists.is_empty() {
                        o.blank();
                    }
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if !o.line.is_empty() {
                    o.end_line();
                }
            }
            Event::Start(Tag::BlockQuote(_)) => {
                o.blank();
                o.prefix.push(("▌ ".into(), Some(kind("comment"))));
                style.push(kind("comment"));
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                if !o.line.is_empty() {
                    o.end_line();
                }
                o.prefix.pop();
                style.pop();
            }
            Event::Start(Tag::List(start)) => {
                if lists.is_empty() {
                    o.blank();
                } else if !o.line.is_empty() {
                    o.end_line();
                }
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
                if !o.line.is_empty() {
                    o.end_line();
                }
            }
            Event::Start(Tag::Item) => {
                if !o.line.is_empty() {
                    o.end_line();
                }
                let depth = lists.len();
                let bullet = match lists.last_mut() {
                    Some(Some(n)) => {
                        let b = format!("{n}. ");
                        *n += 1;
                        b
                    }
                    // (QBasic's help bullets.)
                    _ => format!("{} ", ["■", "·", "■"][(depth - 1) % 3]),
                };
                let w = str_width(&bullet);
                o.prefix.push((" ".repeat(w), None));
                o.first_prefix = Some((bullet, Some(kind("punctuation"))));
            }
            Event::End(TagEnd::Item) => {
                if !o.line.is_empty() {
                    o.end_line();
                }
                o.prefix.pop();
                o.first_prefix = None;
            }
            Event::TaskListMarker(done) => {
                if let Some((b, _)) = &mut o.first_prefix {
                    *b = if done { "☑ ".into() } else { "☐ ".into() };
                }
            }
            Event::Start(Tag::CodeBlock(k)) => {
                let lang = match k {
                    CodeBlockKind::Fenced(info) => info.split([',', ' ']).next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                let (lang, text) = code.take().unwrap_or_default();
                o.blank();
                code_block(&mut o, &lang, &text);
                o.blank();
            }
            Event::Start(Tag::Table(al)) => {
                o.blank();
                table = Some((al, Vec::new(), Vec::new(), String::new()));
            }
            Event::End(TagEnd::Table) => {
                if let Some((al, rows, _, _)) = table.take() {
                    draw_table(&mut o, &al, &rows);
                }
            }
            Event::Start(Tag::Emphasis) => style.push(kind("attribute")),
            Event::Start(Tag::Strong) => style.push(kind("constant")),
            Event::Start(Tag::Strikethrough) => style.push(kind("comment")),
            Event::End(TagEnd::Emphasis) | Event::End(TagEnd::Strong) | Event::End(TagEnd::Strikethrough) => {
                style.pop();
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                link.push(dest_url.to_string());
                style.push(kind("function"));
                // (QBasic's help: ◄link►.)
                o.space();
                link_open = true;
            }
            Event::End(TagEnd::Link) => {
                link.pop();
                style.pop();
                o.attach("►", kind("label"));
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                image = Some(dest_url.to_string());
                style.push(kind("special"));
                o.words("[picture:", style.last().copied(), Some(&dest_url));
                o.space();
            }
            Event::End(TagEnd::Image) => {
                let t = image.take().unwrap_or_default();
                o.words("]", style.last().copied(), Some(&t));
                style.pop();
            }
            Event::Text(t) => {
                if let Some((_, h, _)) = &mut heading {
                    h.push_str(&t);
                }
                let l = link.last().cloned();
                if std::mem::take(&mut link_open) {
                    // (The ◄ goes with the link's first word, on its line.)
                    let first = t.split(' ').next().unwrap_or("");
                    if !o.line.is_empty() && str_width(&o.line) + 1 + str_width(first) > o.width {
                        o.end_line();
                    }
                    o.attach("◄", kind("label"));
                    o.line_nospace = true;
                }
                // (Text keeps its own line breaks inside, rare: words.)
                for (i, part) in t.split('\n').enumerate() {
                    if i > 0 {
                        o.space();
                    }
                    o.words(part, style.last().copied(), l.as_deref());
                }
            }
            Event::Code(t) => {
                if let Some((_, h, _)) = &mut heading {
                    h.push_str(&t);
                }
                let l = link.last().cloned();
                if std::mem::take(&mut link_open) {
                    o.attach("◄", kind("label"));
                    o.line_nospace = true;
                }
                o.words(&t, Some(kind("string")), l.as_deref());
            }
            Event::SoftBreak => o.space(),
            Event::HardBreak => {
                o.end_line();
            }
            Event::Rule => {
                o.blank();
                let w = o.width.saturating_sub(o.prefix_width()).min(72);
                let rule = "─".repeat(w);
                o.raw_line(&rule.clone(), vec![(0, rule.len() as u32, kind("punctuation"))]);
                o.blank();
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                // (Comments and tags left out; <br> breaks.)
                let t = h.trim();
                if t.eq_ignore_ascii_case("<br>") || t.eq_ignore_ascii_case("<br/>") || t.eq_ignore_ascii_case("<br />") {
                    o.end_line();
                }
            }
            Event::FootnoteReference(n) => o.words(&format!("[{n}]"), Some(kind("label")), None),
            _ => {}
        }
    }
    if !o.line.is_empty() {
        o.end_line();
    }
    // (No blank lines at the end.)
    while o.r.text.ends_with("\n\n") {
        o.r.text.pop();
        o.r.spans.pop();
    }
    o.r
}

/// A help page's first line, as QBasic's and VB-DOS's help have it:
/// ◄Contents► ◄Index► ◄Back► at the right (links to `help:contents`,
/// `help:index` and `help:back`), then a blank line.
pub fn with_nav(mut r: Rendered, width: usize) -> Rendered {
    let items = [("Contents", "help:contents"), ("Index", "help:index"), ("Back", "help:back")];
    let row_w: usize = items.iter().map(|(t, _)| str_width(t) + 2).sum::<usize>() + 2 * (items.len() - 1);
    let mut line = " ".repeat(width.saturating_sub(row_w + 1));
    let mut spans = Vec::new();
    let mut links = Vec::new();
    for (i, (t, target)) in items.iter().enumerate() {
        if i > 0 {
            line.push_str("  ");
        }
        let a = line.len();
        line.push('◄');
        let b = line.len();
        line.push_str(t);
        let c = line.len();
        line.push('►');
        spans.push((a as u32, b as u32, kind("label")));
        spans.push((b as u32, c as u32, kind("function")));
        spans.push((c as u32, line.len() as u32, kind("label")));
        links.push((0, a, line.len(), target.to_string()));
    }
    r.text = format!("{line}\n\n{}", r.text);
    r.spans.insert(0, Vec::new());
    r.spans.insert(0, spans);
    for l in &mut r.links {
        l.0 += 2;
    }
    r.links.splice(0..0, links);
    for h in &mut r.headings {
        h.0 += 2;
    }
    for a in &mut r.anchors {
        a.1 += 2;
    }
    r
}

/// A code block: in a box, highlighted when its language is known.
fn code_block(o: &mut Out, lang: &str, text: &str) {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let lang_name = turbine_syntax::detect(Some(std::path::Path::new(&format!("x.{lang}"))), "").or_else(|| (!lang.is_empty()).then(|| lang.to_string()));
    let spans = lang_name.as_deref().and_then(|l| turbine_syntax::highlight(l, text, std::time::Duration::from_millis(300)));
    let frame = kind("punctuation");
    let inner = o.width.saturating_sub(o.prefix_width() + 4).max(10);
    let longest = text.lines().map(str_width).max().unwrap_or(0).min(inner).max(lang.chars().count() + 2);
    let label = if lang.is_empty() { String::new() } else { format!(" {lang} ") };
    let top = format!("┌{label}{}┐", "─".repeat((longest + 2).saturating_sub(str_width(&label))));
    o.raw_line(&top.clone(), vec![(0, top.len() as u32, frame)]);
    for (i, line) in text.split('\n').enumerate() {
        // (Too wide lines are cut: the box stays a box.)
        let mut shown = String::new();
        for ch in line.chars() {
            if str_width(&shown) + str_width(&ch.to_string()) > longest {
                break;
            }
            shown.push(ch);
        }
        let pad = longest - str_width(&shown);
        let row = format!("│ {shown}{} │", " ".repeat(pad));
        let mut sp = vec![(0, "│ ".len() as u32, frame), ((row.len() - "│".len()) as u32, row.len() as u32, frame)];
        let base = "│ ".len() as u32;
        match spans.as_ref().and_then(|s| s.get(i)) {
            Some(line_spans) => sp.extend(line_spans.iter().filter(|(a, _, _)| (*a as usize) < shown.len()).map(|(a, b, k)| (a + base, (*b).min(shown.len() as u32) + base, *k))),
            None => sp.push((base, base + shown.len() as u32, kind("string"))),
        }
        o.raw_line(&row, sp);
    }
    let bottom = format!("└{}┘", "─".repeat(longest + 2));
    o.raw_line(&bottom.clone(), vec![(0, bottom.len() as u32, frame)]);
}

/// A table, in box characters, its columns fitted to the width.
fn draw_table(o: &mut Out, al: &[Alignment], rows: &[Vec<String>]) {
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    let mut w: Vec<usize> = (0..cols).map(|c| rows.iter().map(|r| r.get(c).map_or(0, |s| str_width(s))).max().unwrap_or(0).max(1)).collect();
    let room = o.width.saturating_sub(o.prefix_width() + 3 * cols + 1).max(cols * 3);
    while w.iter().sum::<usize>() > room {
        let i = (0..cols).max_by_key(|&i| w[i]).unwrap();
        w[i] -= 1;
    }
    let frame = kind("punctuation");
    let rule = |l: &str, m: &str, r: &str| format!("{l}{}{r}", w.iter().map(|n| "─".repeat(n + 2)).collect::<Vec<_>>().join(m));
    let fit = |s: &str, n: usize, a: Alignment| {
        let mut t = String::new();
        for ch in s.chars() {
            if str_width(&t) + str_width(&ch.to_string()) > n {
                break;
            }
            t.push(ch);
        }
        let pad = n - str_width(&t);
        match a {
            Alignment::Right => format!("{}{t}", " ".repeat(pad)),
            Alignment::Center => format!("{}{t}{}", " ".repeat(pad / 2), " ".repeat(pad - pad / 2)),
            _ => format!("{t}{}", " ".repeat(pad)),
        }
    };
    let top = rule("┌", "┬", "┐");
    o.raw_line(&top.clone(), vec![(0, top.len() as u32, frame)]);
    for (ri, r) in rows.iter().enumerate() {
        let mut line = String::from("│");
        let mut sp = vec![(0u32, line.len() as u32, frame)];
        for c in 0..cols {
            line.push(' ');
            let a = line.len() as u32;
            line.push_str(&fit(r.get(c).map(String::as_str).unwrap_or(""), w[c], al.get(c).copied().unwrap_or(Alignment::None)));
            if ri == 0 {
                sp.push((a, line.len() as u32, kind("keyword")));
            }
            line.push(' ');
            let b = line.len() as u32;
            line.push('│');
            sp.push((b, line.len() as u32, frame));
        }
        o.raw_line(&line, sp);
        if ri == 0 && rows.len() > 1 {
            let mid = rule("├", "┼", "┤");
            o.raw_line(&mid.clone(), vec![(0, mid.len() as u32, frame)]);
        }
    }
    let bottom = rule("└", "┴", "┘");
    o.raw_line(&bottom.clone(), vec![(0, bottom.len() as u32, frame)]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_blocks() {
        let md = "# Title\n\nSome *em* and `code` with a [link](https://x.org).\n\n## Lists\n\n- one\n- two\n  1. a\n  2. b\n- [x] done\n\n> quoted\n> text\n\n```rust\nfn main() {}\n```\n\n| A | Bee |\n|---|----:|\n| 1 | 22 |\n";
        let r = render(md, 40);
        let lines: Vec<&str> = r.text.lines().collect();
        assert_eq!(lines[0], "Title");
        assert_eq!(lines[1], "");
        assert!(lines.contains(&"Some em and code with a ◄link►."), "{}", r.text);
        assert!(lines.contains(&"■ one") && lines.contains(&"  1. a") && lines.contains(&"☑ done"), "{}", r.text);
        assert!(lines.contains(&"▌ quoted text"), "{}", r.text);
        assert!(lines.iter().any(|l| l.starts_with("┌ rust ")) && lines.contains(&"│ fn main() {} │"), "{}", r.text);
        assert!(lines.contains(&"│ A │ Bee │") && lines.contains(&"│ 1 │  22 │"), "{}", r.text);
        let (line, a, b, t) = &r.links[0];
        assert_eq!((&lines[*line][*a..*b], t.as_str()), ("link", "https://x.org"));
        assert_eq!(r.headings[1].2, "Lists");
        assert_eq!(r.anchors[1].0, "lists");
        // `fn` coloured as a keyword in the code box.
        let li = lines.iter().position(|l| l.starts_with("│ fn")).unwrap();
        assert!(r.spans[li].iter().any(|(a, _, k)| *a == "│ ".len() as u32 && *k == kind("keyword")), "{:?}", r.spans[li]);
    }

    #[test]
    fn key_combinations() {
        assert_eq!(key_combo("Ctrl+Shift+P"), Some((0, 12)));
        assert_eq!(key_combo("(F5),"), Some((1, 3)));
        assert_eq!(key_combo("Ctrl+`."), Some((0, 6)));
        assert_eq!(key_combo("Ctrl++"), Some((0, 6)));
        assert_eq!(key_combo("Enter"), None);
        assert_eq!(key_combo("C++"), None);
        assert_eq!(key_combo("a+b"), None);
        let r = render("Press **Ctrl+S** or F2 to save, `x+1` stays code.", 80);
        let line = r.text.lines().next().unwrap();
        let at = |w: &str| line.find(w).unwrap() as u32;
        let esc = kind("escape");
        assert!(r.spans[0].iter().any(|s| s.0 == at("Ctrl+S") && s.2 == esc), "{:?}", r.spans[0]);
        assert!(r.spans[0].iter().any(|s| s.0 == at("F2") && s.2 == esc));
        assert!(!r.spans[0].iter().any(|s| s.0 == at("x+1") && s.2 == esc));
    }

    #[test]
    fn wraps_at_the_width() {
        let r = render(&"word ".repeat(30), 30);
        assert!(r.text.lines().all(|l| str_width(l) <= 30));
        assert!(r.text.lines().count() >= 5);
    }
}
