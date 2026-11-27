//! Tooltips: the mouse resting on a control for a moment shows what it is
//! in a hint box under it, as VS Code shows them: an icon's meaning, a cut
//! name in full, an editor tab's path, the error under the mouse. They
//! come from the controls' tree (the one agents read): a node's own tip,
//! else its name when what's drawn is shorter than that.

use super::*;

const DELAY: Duration = Duration::from_millis(600);

#[derive(Default)]
pub(super) struct TipState {
    /// Where the mouse rests, and since when.
    at: Option<(Point, Instant)>,
    /// The tip shown: the control's place (moving off it hides the tip)
    /// and the text.
    pub shown: Option<(Rect, String)>,
    /// Looked for at this resting place already (found nothing).
    looked: bool,
    /// The language server asked what's under the mouse: its request, and
    /// the place the answer belongs to.
    pub hover: Option<((String, PathBuf), i64, Rect)>,
    /// The tip's lines already in colours (code: an Outline item's lines).
    pub rich: Option<Vec<Vec<(String, Style)>>>,
}

impl Workbench {
    /// Every event: moving rests the mouse anew; keys and clicks hide it.
    pub(super) fn tip_event(&mut self, ev: &Event) {
        match ev {
            Event::Mouse(m) if matches!(m.kind, MouseKind::Moved) => {
                if m.pos.x < 0 || m.pos.y < 0 {
                    self.tip = TipState::default();
                    return;
                }
                if self.tip.at.is_some_and(|(p, _)| p == m.pos) {
                    return;
                }
                if self.tip.shown.as_ref().is_some_and(|(r, _)| !r.contains(m.pos)) {
                    self.tip.shown = None;
                }
                self.tip.at = Some((m.pos, Instant::now()));
                self.tip.looked = false;
            }
            Event::Mouse(_) | Event::Key(_) | Event::Paste(_) | Event::Resize(..) => {
                self.tip = TipState::default();
            }
            _ => {}
        }
    }

    /// When the mouse has rested long enough: the tip. Returns when to
    /// look again.
    pub(super) fn tip_tick(&mut self, now: Instant) -> Option<Instant> {
        let (p, since) = self.tip.at?;
        if self.tip.shown.is_some() || self.tip.looked {
            return None;
        }
        if now < since + DELAY {
            return Some(since + DELAY);
        }
        self.tip.looked = true;
        if self.menubar.is_open() || self.driver.busy() {
            return None;
        }
        self.tip.shown = self.tip_at(p);
        self.tip.rich = self.tip.shown.as_ref().and_then(|_| self.outline_rich_at(p));
        // (Code under the mouse, nothing else to say: ask its server, as
        // VS Code's hover does.)
        if self.tip.shown.is_none() {
            self.hover_ask(p);
        }
        None
    }

    fn hover_ask(&mut self, p: Point) {
        if self.dialog.is_some() {
            return;
        }
        let Some(w) = self.windows.last_mut().filter(|w| w.term.is_none() && w.dock.is_none() && w.special.is_none()) else { return };
        let Some(ch) = w.ed.char_at(p) else { return };
        let (Some(key), Some(path)) = (w.lsp_key.clone(), w.ed.doc.path.clone()) else { return };
        let (line, byte) = w.ed.doc.char_to_line_byte(ch);
        let text = w.ed.doc.line(line);
        // (Nothing under the mouse but space: nothing to ask.)
        if text.get(byte..).and_then(|t| t.chars().next()).is_none_or(char::is_whitespace) {
            return;
        }
        let v = w.ed.doc.version;
        let full = (w.lsp_sent != Some(v)).then(|| w.ed.doc.rope.to_string());
        let Some(c) = self.lsp.get_mut(&key).filter(|c| c.is_open(&path)) else { return };
        if let Some(t) = full {
            c.did_change(&path, v as i64, &t);
            if let Some(w) = self.windows.last_mut() {
                w.lsp_sent = Some(v);
            }
        }
        let id = c.hover(&path, Pos { line: line as u32, character: lsp::utf16_col(&text, byte) });
        self.tip.hover = Some((key, id, Rect::new(p.x, p.y, 1, 1)));
    }

    /// A hover answer for the tooltip: true when it was the tooltip's.
    pub(super) fn hover_answer(&mut self, key: &(String, PathBuf), id: i64, text: &str) -> bool {
        let Some((k, i, anchor)) = &self.tip.hover else { return false };
        if k != key || *i != id {
            return false;
        }
        let anchor = *anchor;
        self.tip.hover = None;
        // (Still resting there: the server's words, without its code fences.)
        if self.tip.at.is_some_and(|(p, _)| anchor.contains(p)) && self.tip.shown.is_none() {
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("```")).collect();
            let t = lines.join("\n").trim().to_string();
            if !t.is_empty() {
                self.tip.shown = Some((anchor, t));
            }
        }
        true
    }

    /// What hovering here shows, and the place it belongs to.
    pub(super) fn tip_at(&self, p: Point) -> Option<(Rect, String)> {
        // The error or warning under the mouse in the front editor.
        if self.dialog.is_none() {
            if let Some(w) = self.windows.last().filter(|w| w.term.is_none() && w.win.inner().contains(p)) {
                if let Some(d) = w.ed.diagnostic_at(p) {
                    let what = match d.severity {
                        1 => "Error",
                        2 => "Warning",
                        3 => "Info",
                        _ => "Hint",
                    };
                    return Some((Rect::new(p.x, p.y, 1, 1), format!("{what}: {}", d.message)));
                }
            }
        }
        // The file manager's commands, folders, files and figures.
        if self.dialog.is_none() {
            if let Some(x) = self.windows.last().filter(|w| w.win.rect.contains(p)).and_then(|w| self.xtrees.get(&w.win.id)) {
                return x.tip_at(p);
            }
        }
        // A tree row cut short: whole.
        if self.dialog.is_none() {
            if let Some(t) = self.docview_tip(p) {
                return Some(t);
            }
        }
        let tree = self.tree();
        let n = deepest(&tree, p)?;
        let text = match &n.tip {
            Some(t) => t.clone(),
            None if matches!(n.role.as_str(), "button" | "tab") && str_width(&n.name) as i32 > n.rect.w => n.name.replace('~', ""),
            None => return None,
        };
        (!text.trim().is_empty()).then(|| (n.rect, text))
    }

    /// Draw the tip under the mouse (above it at the bottom of the screen),
    /// kept on the screen, its parts in colours: a path's folders, its
    /// slashes and its name (in the tree's colour); sizes, dates,
    /// permissions and git's status each their own.
    pub(super) fn draw_tip(&self, c: &mut Canvas, theme: &Theme) {
        let (Some((_, text)), Some((p, _))) = (&self.tip.shown, self.tip.at) else { return };
        let screen = self.screen();
        let max_w = (screen.w - 4).clamp(10, 70) as usize;
        let base = theme.scheme.tooltip;
        let is_dir = text.lines().any(|l| l.starts_with("Folder"));
        // Each line as coloured pieces, then cut to the box's width.
        let mut lines: Vec<Vec<(String, Style)>> = Vec::new();
        let rich = self.tip.rich.clone();
        let source: Vec<Vec<(String, Style)>> = match rich {
            Some(r) => r,
            None => text.lines().map(|l| colour_line(l, is_dir, base)).collect(),
        };
        for pieces in source {
            let mut cur: Vec<(String, Style)> = Vec::new();
            let mut w = 0;
            for (t, st) in pieces {
                for ch in t.chars() {
                    let cw = str_width(&ch.to_string());
                    if w + cw > max_w {
                        lines.push(std::mem::take(&mut cur));
                        w = 0;
                    }
                    match cur.last_mut() {
                        Some((s, x)) if *x == st => s.push(ch),
                        _ => cur.push((ch.to_string(), st)),
                    }
                    w += cw;
                }
            }
            lines.push(cur);
            if lines.len() >= 10 {
                break;
            }
        }
        lines.truncate(10);
        let w = lines.iter().map(|l| l.iter().map(|(t, _)| str_width(t)).sum::<usize>()).max().unwrap_or(0) as i32 + 2;
        let h = lines.len() as i32;
        let x = p.x.min(screen.right() - w).max(0);
        let y = if p.y + 1 + h <= screen.bottom() { p.y + 1 } else { (p.y - h).max(0) };
        let r = Rect::new(x, y, w, h);
        c.fill(r, " ", base);
        for (i, l) in lines.iter().enumerate() {
            let mut cx = x + 1;
            for (t, st) in l {
                cx = c.print(cx, y + i as i32, t, *st);
            }
        }
        // (A shadow, as DOS windows have one.)
        if theme.style.shadows {
            c.shadow(r, theme.scheme.shadow);
        }
    }
}

impl Workbench {
    /// An Outline row's tooltip in colours: its name, then its first lines
    /// as the editor colours them.
    fn outline_rich_at(&self, p: Point) -> Option<Vec<Vec<(String, Style)>>> {
        let list = &self.shell.outline.list;
        if !list.area.contains(p) || !self.section_shown(crate::dock::Tab::Outline) && self.shell.dock.side_of(crate::dock::Tab::Outline).is_none() {
            return None;
        }
        let i = list.scroll + (p.y - list.area.y) as usize;
        let name = list.rows.get(i)?.text.clone();
        let &line = self.shell.outline.lines.get(i)?;
        let ei = self.editor_index()?;
        let ed = &self.windows[ei].ed;
        let base = self.theme.scheme.tooltip;
        let s = &self.theme.scheme;
        let mut out = vec![vec![(name, Style { fg: vga::WHITE, attrs: turbine_ui::theme::Attrs { bold: true, ..base.attrs }, ..base }), (format!("  line {}", line + 1), Style { fg: vga::DARK_GRAY, ..base })]];
        for l in line..(line + 8).min(ed.doc.len_lines()) {
            let text = ed.doc.line(l);
            let spans = ed.highlights.get(l);
            let mut pieces: Vec<(String, Style)> = vec![("  ".into(), base)];
            let mut cur = String::new();
            let mut cur_st = base;
            for (b, ch) in text.char_indices() {
                let kind = spans.and_then(|sp| sp.iter().rev().find(|(a, e, _)| (*a as usize) <= b && b < *e as usize)).map(|x| x.2 as usize);
                let fg = kind.and_then(|k| s.syntax.get(k).copied().flatten()).map(|c| turbine_ui::list::readable(c, base.bg)).unwrap_or(base.fg);
                let st = Style { fg, ..base };
                if st != cur_st && !cur.is_empty() {
                    pieces.push((std::mem::take(&mut cur), cur_st));
                }
                cur_st = st;
                if ch == '\t' {
                    cur.push_str("    ");
                } else {
                    cur.push(ch);
                }
            }
            if !cur.is_empty() {
                pieces.push((cur, cur_st));
            }
            out.push(pieces);
        }
        Some(out)
    }
}

/// A tooltip line's colours (on the tip's black).
fn colour_line(line: &str, is_dir: bool, base: Style) -> Vec<(String, Style)> {
    let fg = |c| Style { fg: c, ..base };
    // A path: folders, slashes, and the name in the tree's colour.
    if line.starts_with('/') || line.starts_with("~/") {
        let mut out = Vec::new();
        let parts: Vec<&str> = line.split('/').collect();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                out.push(("/".to_string(), fg(vga::YELLOW)));
            }
            let last = i + 1 == parts.len();
            let st = if last { fg(crate::filetypes::colour(part, is_dir)) } else { fg(vga::LIGHT_CYAN) };
            out.push((part.to_string(), st));
        }
        return out;
    }
    if let Some(v) = line.strip_prefix("Git: ") {
        let c = match v {
            "changed" => vga::YELLOW,
            "added" | "untracked (new)" => vga::LIGHT_GREEN,
            "deleted" | "in conflict" => vga::LIGHT_RED,
            "renamed" => vga::LIGHT_CYAN,
            _ => vga::YELLOW,
        };
        return vec![("Git: ".into(), fg(vga::WHITE)), (v.to_string(), fg(c))];
    }
    if let Some(v) = line.strip_prefix("Size: ") {
        return vec![("Size: ".into(), fg(vga::WHITE)), (v.to_string(), fg(vga::LIGHT_GREEN))];
    }
    for pre in ["Error: ", "Warning: ", "Info: ", "Hint: "] {
        if let Some(v) = line.strip_prefix(pre) {
            let c = match pre {
                "Error: " => vga::LIGHT_RED,
                "Warning: " => vga::YELLOW,
                _ => vga::LIGHT_CYAN,
            };
            return vec![(pre.to_string(), fg(c)), (v.to_string(), base)];
        }
    }
    // Facts apart by " · ": sizes, dates, permissions, a folder's count.
    if line.contains("  ·  ") {
        let mut out = Vec::new();
        for (i, f) in line.split("  ·  ").enumerate() {
            if i > 0 {
                out.push(("  ·  ".into(), fg(vga::DARK_GRAY)));
            }
            if let Some(d) = f.strip_prefix("modified ") {
                out.push(("modified ".into(), fg(vga::WHITE)));
                out.push((d.to_string(), fg(vga::YELLOW)));
            } else if f.len() == 9 && f.chars().all(|c| matches!(c, 'r' | 'w' | 'x' | '-')) {
                out.push((f.to_string(), fg(vga::LIGHT_MAGENTA)));
            } else if f.ends_with(" file") || matches!(f, "Picture" | "PDF document" | "Sound" | "Video" | "Archive" | "Document" | "Spreadsheet" | "Presentation" | "Font" | "Program" | "Text" | "File") {
                out.push((f.to_string(), fg(vga::WHITE)));
            } else if f.starts_with("Folder") {
                out.push((f.to_string(), fg(vga::LIGHT_BLUE)));
            } else {
                out.push((f.to_string(), fg(vga::LIGHT_GREEN)));
            }
        }
        return out;
    }
    vec![(line.to_string(), base)]
}

/// The innermost node under the point, the one drawn last winning.
fn deepest(n: &Node, p: Point) -> Option<&Node> {
    for ch in n.children.iter().rev() {
        if let Some(d) = deepest(ch, p) {
            return Some(d);
        }
    }
    (n.rect.contains(p) && n.role != "application").then_some(n)
}
