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
    /// kept on the screen: one line, or a few when it's long.
    pub(super) fn draw_tip(&self, c: &mut Canvas, theme: &Theme) {
        let (Some((_, text)), Some((p, _))) = (&self.tip.shown, self.tip.at) else { return };
        let screen = self.screen();
        let max_w = (screen.w - 4).clamp(10, 70) as usize;
        let mut lines: Vec<String> = Vec::new();
        for para in text.lines() {
            let mut line = String::new();
            for word in para.split(' ') {
                if !line.is_empty() && str_width(&line) + 1 + str_width(word) > max_w {
                    lines.push(std::mem::take(&mut line));
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
            }
            lines.push(line);
            if lines.len() >= 8 {
                break;
            }
        }
        lines.truncate(8);
        let w = lines.iter().map(|l| str_width(l)).max().unwrap_or(0).min(max_w) as i32 + 2;
        let h = lines.len() as i32;
        let x = p.x.min(screen.right() - w).max(0);
        let y = if p.y + 1 + h <= screen.bottom() { p.y + 1 } else { (p.y - h).max(0) };
        let st = theme.scheme.tooltip;
        let r = Rect::new(x, y, w, h);
        c.fill(r, " ", st);
        for (i, l) in lines.iter().enumerate() {
            c.print(x + 1, y + i as i32, l, st);
        }
        // (A shadow, as DOS windows have one.)
        if theme.style.shadows {
            c.shadow(r, theme.scheme.shadow);
        }
    }
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
