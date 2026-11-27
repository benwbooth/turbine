//! Sections under the Explorer, as VS Code stacks them in its side bar:
//! Outline (the file's symbols) and Timeline (the file's history in git;
//! Enter or a click shows a commit's change to it). Each has a header row:
//! a click folds or opens it, a drag resizes it.

use super::*;
use crate::dock::Tab;
use std::sync::mpsc::channel;
use turbine_ui::list::{ListOut, Row};

#[derive(Default)]
pub(super) struct Timeline {
    pub list: ListView,
    /// The file shown, and git's answer coming.
    path: Option<PathBuf>,
    rx: Option<Receiver<(PathBuf, PathBuf, Vec<(String, String, String, String)>)>>,
    /// Each row's commit, and the repository.
    hashes: Vec<String>,
    root: PathBuf,
    asked: Option<Instant>,
}

impl Workbench {
    /// Whether a section's view shows (open, its part showing the Explorer).
    pub(super) fn section_shown(&self, t: Tab) -> bool {
        let Some(side) = self.shell.dock.side_of(Tab::Explorer) else { return false };
        let p = self.shell.dock.part(side);
        p.visible && p.current() == Some(Tab::Explorer) && self.shell.dock.sections.iter().any(|(x, open, _)| *x == t && *open)
    }

    /// Open a section and give it the keys (View > Outline, Timeline).
    pub(super) fn show_section(&mut self, t: Tab) {
        self.show_tab(Tab::Explorer);
        if let Some(s) = self.shell.dock.sections.iter_mut().find(|(x, ..)| *x == t) {
            s.1 = true;
        }
        self.shell.focus = shell::Focus::View(t);
    }

    /// Keep the Timeline on the front file (git log, off the UI thread).
    pub(super) fn timeline_tick(&mut self) {
        if let Some(rx) = &self.shell.timeline.rx {
            if let Ok((path, root, entries)) = rx.try_recv() {
                let tl = &mut self.shell.timeline;
                tl.rx = None;
                if tl.path.as_ref() == Some(&path) {
                    tl.root = root;
                    tl.hashes = entries.iter().map(|e| e.0.clone()).collect();
                    let rows: Vec<Row> = if entries.is_empty() {
                        vec![Row::new("No history in git").fg(turbine_ui::theme::vga::DARK_GRAY)]
                    } else {
                        entries.into_iter().map(|(_, when, who, subject)| Row::new(subject).icon("•").detail(format!("{who}, {when}"))).collect()
                    };
                    tl.list.set_rows(rows);
                }
            }
        }
        if !self.section_shown(Tab::Timeline) {
            return;
        }
        let path = self.editor_index().and_then(|i| self.windows[i].ed.doc.path.clone());
        let tl = &mut self.shell.timeline;
        // (Asked again now and then: commits come.)
        let stale = tl.asked.is_none_or(|t| t.elapsed() > Duration::from_secs(10));
        if path == tl.path && !stale {
            return;
        }
        if tl.rx.is_some() {
            return;
        }
        if path != tl.path {
            tl.hashes.clear();
            tl.list.set_rows(Vec::new());
        }
        tl.path = path.clone();
        tl.asked = Some(Instant::now());
        let Some(path) = path else {
            tl.list.set_rows(vec![Row::new("No file").fg(turbine_ui::theme::vga::DARK_GRAY)]);
            return;
        };
        let (tx, rx) = channel();
        tl.rx = Some(rx);
        std::thread::spawn(move || {
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let top = std::process::Command::new("git").arg("-C").arg(&dir).args(["rev-parse", "--show-toplevel"]).output().ok().filter(|o| o.status.success()).map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
            let mut out = Vec::new();
            if let Some(top) = &top {
                if let Ok(o) = std::process::Command::new("git").arg("-C").arg(top).args(["log", "--follow", "-n", "200", "--format=%h%x1f%ar%x1f%an%x1f%s", "--"]).arg(&path).output() {
                    for l in String::from_utf8_lossy(&o.stdout).lines() {
                        let f: Vec<&str> = l.split('\x1f').collect();
                        if f.len() == 4 {
                            out.push((f[0].to_string(), f[1].to_string(), f[2].to_string(), f[3].to_string()));
                        }
                    }
                }
            }
            let _ = tx.send((path, top.unwrap_or_default(), out));
        });
    }

    /// A Timeline entry: the commit's change to the file, as a diff page.
    pub(super) fn timeline_open(&mut self, i: usize) {
        let tl = &self.shell.timeline;
        let (Some(hash), Some(path)) = (tl.hashes.get(i).cloned(), tl.path.clone()) else { return };
        let root = tl.root.clone();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.show_git_page(&format!("{name} @ {hash}"), preview::PageKind::Diff { side_by_side: false }, &root, vec!["show".into(), "--format=commit %H%nAuthor: %an%nDate:   %ad%n%n    %s%n".into(), hash, "--".into(), path.display().to_string()]);
    }

    pub(super) fn timeline_key(&mut self, k: &KeyEvent) {
        if let ListOut::Activated(i) = self.shell.timeline.list.key(k) {
            self.timeline_open(i);
        }
    }

    /// The mouse on a section's header (fold, or drag to resize) or body.
    /// True when used.
    pub(super) fn section_mouse(&mut self, m: &MouseEvent) -> bool {
        // A header being dragged.
        if let Some((i, y0, size0)) = self.shell.section_drag {
            match m.kind {
                MouseKind::Drag(_) => {
                    if let Some(s) = self.shell.dock.sections.get_mut(i) {
                        s.2 = (size0 + (y0 - m.pos.y)).clamp(2, 200);
                        s.1 = true;
                    }
                    self.layout_dock();
                }
                _ => self.shell.section_drag = None,
            }
            return true;
        }
        let l = self.layout();
        let Some(side) = self.shell.dock.side_of(Tab::Explorer) else { return false };
        let Some(r) = l.part(side) else { return false };
        let Some((_, secs)) = self.shell.dock.sections_in(side, r) else { return false };
        for (i, (t, hdr, body)) in secs.into_iter().enumerate() {
            if hdr.contains(m.pos) {
                if let MouseKind::Down(Button::Left) = m.kind {
                    // (On its title: fold; elsewhere on the line: resize.)
                    let title_end = hdr.x + 3 + str_width(t.name()) as i32 + 2;
                    if m.pos.x <= title_end {
                        let s = &mut self.shell.dock.sections[i];
                        s.1 = !s.1;
                        self.shell.focus = if s.1 { shell::Focus::View(t) } else { shell::Focus::View(Tab::Explorer) };
                    } else {
                        let size = body.map_or(0, |b| b.h);
                        self.shell.section_drag = Some((i, m.pos.y, size));
                    }
                    self.layout_dock();
                }
                return true;
            }
            if body.is_some_and(|b| b.contains(m.pos)) {
                let press = matches!(m.kind, MouseKind::Down(_));
                if press || matches!(m.kind, MouseKind::ScrollUp | MouseKind::ScrollDown) {
                    if press {
                        self.shell.focus = shell::Focus::View(t);
                    }
                    let list = if t == Tab::Timeline { &mut self.shell.timeline.list } else { &mut self.shell.outline.list };
                    let out = list.mouse(m);
                    match (t, out) {
                        (Tab::Outline, ListOut::Selected(i)) => self.outline_goto_pub(i, false),
                        (Tab::Outline, ListOut::Activated(i)) => self.outline_goto_pub(i, true),
                        (Tab::Timeline, ListOut::Activated(i)) => self.timeline_open(i),
                        _ => {}
                    }
                }
                return true;
            }
        }
        false
    }

    /// Draw the sections under the Explorer: a header line each (├─▾ Outline ─┤)
    /// and the open ones' lists.
    pub(super) fn draw_sections(&mut self, c: &mut Canvas, theme: &Theme, side: Side, r: Rect, focused_part: bool) {
        let Some((_, secs)) = self.shell.dock.sections_in(side, r) else { return };
        let s = &theme.scheme;
        let modal = self.dialog.is_some() || self.menubar.is_open();
        for (t, hdr, body) in secs {
            let has_keys = focused_part && !modal && self.shell.focus == shell::Focus::View(t);
            let line = s.panel_frame;
            c.print(hdr.x, hdr.y, "├", line);
            for x in hdr.x + 1..hdr.right() - 1 {
                c.print(x, hdr.y, "─", line);
            }
            c.print(hdr.right() - 1, hdr.y, "┤", line);
            let open = body.is_some();
            let title = format!(" {} {} ", if open { "▾" } else { "▸" }, t.name());
            let st = if has_keys { s.menu_selected } else { Style { fg: s.frame_active.fg, ..line } };
            c.print(hdr.x + 1, hdr.y, &title, st);
            if let Some(b) = body {
                let list = if t == Tab::Timeline { &mut self.shell.timeline.list } else { &mut self.shell.outline.list };
                list.draw(c, b, theme, has_keys);
            }
        }
    }
}
