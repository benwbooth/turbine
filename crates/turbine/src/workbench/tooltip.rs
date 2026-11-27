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
        None
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
        for line in text.lines() {
            let pieces = colour_line(line, is_dir, base);
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
