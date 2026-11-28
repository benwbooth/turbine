//! The top row, as VS Code's title bar: the menus at the left; ← → and the
//! command center in the middle (the file and project; a click opens Go to
//! File, as VS Code's does); the layout toggles; with Turbine's own title
//! bar, the window's buttons at the right end, and its free space moves the
//! window (a double-click maximizes it). One place lays it all out, so the
//! parts never overlap however narrow the window gets.

use super::*;

/// What's on the row besides the menus: (place, action, name, label).
/// Actions: a command, "win:1" … "win:3" (minimize, maximize, close),
/// "center" (the command center).
#[derive(Clone, Debug)]
pub(super) struct TopItem {
    pub rect: Rect,
    pub action: &'static str,
    pub name: &'static str,
    pub label: String,
    /// Lit (a toggle whose part shows).
    pub on: bool,
}

impl Workbench {
    /// Where the menus may go: up to the toggles (and the window buttons).
    /// (QBasic's style keeps Help at the right end: the toggles go before it.)
    pub(super) fn menus_width(&self) -> i32 {
        let w = self.size.0;
        let buttons = if self.shell.own_title_bar { 9 } else { 0 };
        let toggles = if self.menubar.help_right { 0 } else { 10 };
        (w - buttons - toggles).max(10)
    }

    /// The row's items, laid out for the window's width.
    pub(super) fn top_items(&self) -> Vec<TopItem> {
        let w = self.size.0;
        let mut out = Vec::new();
        let mut right = w;
        // The window's buttons, in the style's own marks (QBasic's arrows,
        // Borland's triangles and close box).
        if self.shell.own_title_bar {
            right -= 9;
            let qbasic = self.theme.style.name.contains("QBasic") || self.theme.style.name.contains("QuickBASIC");
            let (min, max, close) = if qbasic { ("[↓]", "[↑]", "[×]") } else { ("[▼]", "[▲]", "[■]") };
            for (k, (a, n, l)) in [("win:1", "Minimize", min), ("win:2", "Maximize or Restore", max), ("win:3", "Close", close)].into_iter().enumerate() {
                out.push(TopItem { rect: Rect::new(right + k as i32 * 3, 0, 3, 1), action: a, name: n, label: l.into(), on: false });
            }
        }
        // The layout toggles: [▌] lit when its part shows (before Help when
        // it's at the right, QBasic's way).
        let titles = self.menubar.title_rects();
        let n = titles.len();
        let help_right = n > 1 && titles[n - 1].w > 0 && titles[n - 2].w > 0 && titles[n - 1].x > titles[n - 2].right() + 1;
        if help_right {
            right = titles[n - 1].x;
        }
        right -= 10;
        let toggles = [
            ("view.sidebar", "Toggle Primary Side Bar (Ctrl+B)", "▌", self.shell.dock.left.visible),
            ("view.panel", "Toggle Panel (Ctrl+J)", "▄", self.shell.dock.bottom.visible),
            ("view.secondary", "Toggle Secondary Side Bar (Ctrl+Alt+B)", "▐", self.shell.dock.right.visible),
        ];
        if right > 20 {
            for (k, (a, n, g, on)) in toggles.into_iter().enumerate() {
                out.push(TopItem { rect: Rect::new(right + 1 + k as i32 * 3, 0, 3, 1), action: a, name: n, label: format!("[{g}]"), on });
            }
        }
        // The command center: between the menus and the toggles, centred
        // on the window where it fits.
        let left_end = titles.iter().take(if help_right { n - 1 } else { n }).filter(|r| r.w > 0).map(|r| r.right()).max().unwrap_or(0);
        let region_end = right;
        let room = region_end - left_end - 2;
        let bw = (w * 2 / 5).clamp(24, 64).min(room - 6);
        if bw >= 18 {
            let x = ((w - bw) / 2).clamp(left_end + 7, region_end - 1 - bw);
            out.push(TopItem { rect: Rect::new(x - 6, 0, 3, 1), action: "go.back", name: "Go Back (Alt+Left)", label: " ← ".into(), on: false });
            out.push(TopItem { rect: Rect::new(x - 3, 0, 3, 1), action: "go.forward", name: "Go Forward (Alt+Right)", label: " → ".into(), on: false });
            out.push(TopItem { rect: Rect::new(x, 0, bw, 1), action: "center", name: "Search files by name (Ctrl+P); > for commands (Ctrl+Shift+P)", label: self.center_text(bw as usize), on: false });
        }
        out
    }

    /// The command center's words: the file and the project, cut to fit.
    fn center_text(&self, width: usize) -> String {
        let project = self.shell.explorer.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let file = self.editor_index().map(|i| self.windows[i].ed.doc.title()).unwrap_or_default();
        let t = match (file.is_empty(), project.is_empty()) {
            (false, false) => format!("{file} - {project}"),
            (false, true) => file,
            (true, false) => project,
            (true, true) => "Turbine".into(),
        };
        let inner = width.saturating_sub(4);
        let t = if str_width(&t) > inner { t.chars().take(inner.saturating_sub(1)).chain(std::iter::once('…')).collect() } else { t };
        // (Centred in its box, as VS Code's.)
        let pad = inner.saturating_sub(str_width(&t));
        format!("  {}{t}{}  ", " ".repeat(pad / 2), " ".repeat(pad - pad / 2))
    }

    pub(super) fn draw_top_items(&self, c: &mut Canvas, theme: &Theme) {
        let s = &theme.scheme;
        let base = s.menu;
        for it in self.top_items() {
            let st = match it.action {
                // (A box set in the bar, as VS Code's: the field's colours.)
                "center" => s.editor,
                a if a.starts_with("win:") => s.frame_icons,
                "go.back" | "go.forward" => base,
                _ if it.on => Style { fg: s.menu_hotkey.fg, ..base },
                _ => Style { fg: s.shadow.fg, ..base },
            };
            c.print(it.rect.x, it.rect.y, &it.label, st);
        }
    }

    /// A click on the row (not on a menu's title): its item, else (with
    /// Turbine's own title bar) a move, or a maximize when double.
    /// True when it took the click.
    pub(super) fn top_mouse(&mut self, m: &MouseEvent) -> bool {
        if m.pos.y != 0 || self.menubar.is_open() {
            return false;
        }
        let item = self.top_items().into_iter().find(|t| t.rect.contains(m.pos));
        let press = matches!(m.kind, MouseKind::Down(Button::Left));
        match item {
            Some(t) => {
                if press {
                    match t.action {
                        "win:1" => self.window_op = 1,
                        "win:2" => self.window_op = 2,
                        "win:3" => self.window_op = 3,
                        "center" => self.run_command("go.file"),
                        a => self.run_command(a),
                    }
                }
                true
            }
            None if self.shell.own_title_bar && !self.menubar.title_rects().iter().any(|r| r.contains(m.pos)) => {
                if press {
                    let double = self.last_click.is_some_and(|(p, t)| p.y == -1 && (p.x - m.pos.x).abs() < 2 && t.elapsed() < Duration::from_millis(400));
                    self.last_click = Some((Point::new(m.pos.x, -1), Instant::now()));
                    self.window_op = if double { 2 } else { 4 };
                }
                true
            }
            None => false,
        }
    }
}
