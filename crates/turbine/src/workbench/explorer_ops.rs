//! The Explorer's file operations, as in Cursor: New File, New Folder,
//! Rename, Delete (to the desktop's trash, never for good), Copy Path; from
//! a right-click menu, the buttons on the Explorer's top row, and keys (F2
//! renames, Delete deletes, Shift+F10 opens the menu). The
//! disk work runs on a worker thread.

use super::shell::Focus;
use super::*;
use crate::dock::Tab;
use std::sync::mpsc::channel;
use turbine_ui::popup::{ListPopup, PopupResult};

/// What a finished file operation did.
pub(super) enum Done {
    Created(PathBuf, bool),
    Renamed(PathBuf, PathBuf),
    Trashed(PathBuf),
    Failed(String),
}

/// The Explorer's right-click menu: the row it's for, and its items.
pub(super) struct ExMenu {
    pub popup: ListPopup,
    pub at: Point,
    pub row: usize,
    pub actions: Vec<&'static str>,
}

#[derive(Default)]
pub(super) struct ExOps {
    pub menu: Option<ExMenu>,
    pub rx: Option<std::sync::mpsc::Receiver<Done>>,
    pub tx: Option<Sender<Done>>,
}

/// The desktop trash (the freedesktop.org spec): `TURBINE_TRASH` for tests,
/// else $XDG_DATA_HOME/Trash.
fn trash_dir() -> Option<PathBuf> {
    if let Some(t) = std::env::var_os("TURBINE_TRASH") {
        return Some(PathBuf::from(t));
    }
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("Trash"))
}

/// Move a file or folder into the trash (with its .trashinfo), so it can
/// be restored. Never deletes: a move across drives fails instead.
fn trash(p: &Path) -> Result<(), String> {
    let dir = trash_dir().ok_or("no trash folder")?;
    let (files, info) = (dir.join("files"), dir.join("info"));
    std::fs::create_dir_all(&files).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&info).map_err(|e| e.to_string())?;
    let name = p.file_name().ok_or("no name")?.to_string_lossy().into_owned();
    let mut n = 0;
    let base = loop {
        let candidate = if n == 0 { name.clone() } else { format!("{name}.{n}") };
        if !files.join(&candidate).exists() && !info.join(format!("{candidate}.trashinfo")).exists() {
            break candidate;
        }
        n += 1;
    };
    let enc: String = p
        .display()
        .to_string()
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    let now = std::process::Command::new("date").arg("+%Y-%m-%dT%H:%M:%S").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    std::fs::write(info.join(format!("{base}.trashinfo")), format!("[Trash Info]\nPath={enc}\nDeletionDate={now}\n")).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(p, files.join(&base)) {
        let _ = std::fs::remove_file(info.join(format!("{base}.trashinfo")));
        return Err(if e.raw_os_error() == Some(18) { "it's on another drive than the trash".to_string() } else { e.to_string() });
    }
    Ok(())
}

impl Workbench {
    fn ex_send(&mut self, job: impl FnOnce() -> Done + Send + 'static) {
        if self.ex.tx.is_none() {
            let (tx, rx) = channel();
            self.ex.tx = Some(tx);
            self.ex.rx = Some(rx);
        }
        let tx = self.ex.tx.clone().unwrap();
        std::thread::spawn(move || {
            let _ = tx.send(job());
        });
    }

    /// The folder for new things: the selected folder, or the selected
    /// file's folder, or the project.
    fn ex_target_dir(&self, row: usize) -> PathBuf {
        match self.shell.explorer.path_at(row) {
            Some((p, true)) => p.clone(),
            Some((p, false)) => p.parent().map(Path::to_path_buf).unwrap_or_else(|| self.shell.explorer.root.clone()),
            None => self.shell.explorer.root.clone(),
        }
    }

    pub(super) fn ex_command(&mut self, action: &str, row: usize) {
        let root = self.shell.explorer.root.clone();
        let path = self.shell.explorer.path_at(row).map(|(p, _)| p.clone());
        let name_dialog = |kind: String, title: &str, label: &str, value: &str| {
            Dialog::new(&kind, title).body(ui::input("name", label, value).chars(40)).buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)])
        };
        match action {
            "new_file" => {
                let dir = self.ex_target_dir(row);
                self.dialog = Some(name_dialog(format!("ex_new_file:{}", dir.display()), "New File", "File ~n~ame:", ""));
            }
            "new_folder" => {
                let dir = self.ex_target_dir(row);
                self.dialog = Some(name_dialog(format!("ex_new_folder:{}", dir.display()), "New Folder", "Folder ~n~ame:", ""));
            }
            "rename" => {
                let Some(p) = path.filter(|p| *p != root) else { return };
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.dialog = Some(name_dialog(format!("ex_rename:{}", p.display()), "Rename", "New ~n~ame:", &name));
            }
            "delete" => {
                let Some(p) = path.filter(|p| *p != root) else { return };
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let d = Dialog::new(&format!("ex_delete:{}", p.display()), "Delete")
                    .body(ui::label(&format!("Move \"{name}\" to the trash?\nYou can restore it from there.")))
                    .buttons(vec![ui::button("ok", "Move to ~T~rash", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "copy_path" => {
                if let Some(p) = path {
                    self.copy(p.display().to_string());
                    self.say("Copied the path");
                }
            }
            "copy_relative" => {
                if let Some(p) = path {
                    self.copy(p.strip_prefix(&root).unwrap_or(&p).display().to_string());
                    self.say("Copied the relative path");
                }
            }
            "refresh" => self.shell.explorer.refresh_all(),
            "collapse" => self.shell.explorer.collapse_all(),
            "terminal" => {
                let dir = self.ex_target_dir(row);
                self.new_terminal(vec![]);
                if let Some(t) = self.windows.last_mut().and_then(|w| w.term.as_mut()) {
                    t.paste(&format!("cd {}\n", shell_quote(&dir.display().to_string())));
                }
                self.layout_dock();
                if let Some(id) = self.windows.last().map(|w| w.win.id) {
                    self.shell.dock.show(Tab::Pane(id));
                }
            }
            _ => {}
        }
    }

    /// OK in one of the Explorer's dialogs. True when it was one.
    pub(super) fn ex_dialog_done(&mut self, kind: &str, d: &Dialog) -> bool {
        let name = d.value("name").unwrap_or_default().trim().to_string();
        let bad = name.is_empty() || name.contains('/') || name == "." || name == "..";
        if let Some(dir) = kind.strip_prefix("ex_new_file:") {
            if bad {
                return true;
            }
            let p = Path::new(dir).join(&name);
            self.ex_send(move || match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
                Ok(_) => Done::Created(p, false),
                Err(e) => Done::Failed(format!("Can't create {}: {e}", p.display())),
            });
        } else if let Some(dir) = kind.strip_prefix("ex_new_folder:") {
            if bad {
                return true;
            }
            let p = Path::new(dir).join(&name);
            self.ex_send(move || match std::fs::create_dir(&p) {
                Ok(()) => Done::Created(p, true),
                Err(e) => Done::Failed(format!("Can't create {}: {e}", p.display())),
            });
        } else if let Some(from) = kind.strip_prefix("ex_rename:") {
            let from = PathBuf::from(from);
            if bad || from.file_name().is_some_and(|n| n == name.as_str()) {
                return true;
            }
            let to = from.with_file_name(&name);
            self.ex_send(move || {
                if to.exists() {
                    return Done::Failed(format!("{} already exists", to.display()));
                }
                match std::fs::rename(&from, &to) {
                    Ok(()) => Done::Renamed(from, to),
                    Err(e) => Done::Failed(format!("Can't rename: {e}")),
                }
            });
        } else if let Some(p) = kind.strip_prefix("ex_delete:") {
            let p = PathBuf::from(p);
            self.ex_send(move || match trash(&p) {
                Ok(()) => Done::Trashed(p),
                Err(e) => Done::Failed(format!("Can't move {} to the trash: {e}", p.display())),
            });
        } else {
            return false;
        }
        true
    }

    /// Finished file operations: show them in the tree and the editors.
    pub(super) fn ex_poll(&mut self) {
        let done: Vec<Done> = self.ex.rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for d in done {
            match d {
                Done::Created(p, dir) => {
                    self.shell.explorer.refresh_all();
                    self.shell.explorer.reveal(&p);
                    if !dir {
                        self.goto_file(&p, None, 0);
                    }
                }
                Done::Renamed(from, to) => {
                    // Open editors follow their files.
                    for w in &mut self.windows {
                        if let Some(dp) = &w.ed.doc.path {
                            if let Ok(rest) = dp.strip_prefix(&from) {
                                let np = if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) };
                                w.win.title = np.display().to_string();
                                w.ed.doc.path = Some(np);
                                w.lang_for = None;
                            }
                        }
                    }
                    self.shell.explorer.refresh_all();
                    self.shell.explorer.reveal(&to);
                    self.say(format!("Renamed to {}", to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                }
                Done::Trashed(p) => {
                    self.shell.explorer.refresh_all();
                    self.say(format!("Moved {} to the trash", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                }
                Done::Failed(e) => self.say(e),
            }
        }
    }

    pub(super) fn open_ex_menu(&mut self, row: usize, at: Point) {
        let is_root = self.shell.explorer.path_at(row).is_some_and(|(p, _)| *p == self.shell.explorer.root);
        let mut items: Vec<(&'static str, &'static str, &'static str)> = vec![
            ("New File...", "", "new_file"),
            ("New Folder...", "", "new_folder"),
            ("Open in Terminal", "", "terminal"),
        ];
        if !is_root {
            items.push(("Rename...", "F2", "rename"));
            items.push(("Delete", "Del", "delete"));
        }
        items.push(("Copy Path", "", "copy_path"));
        items.push(("Copy Relative Path", "", "copy_relative"));
        items.push(("Refresh", "", "refresh"));
        items.push(("Collapse All", "", "collapse"));
        let mut popup = ListPopup::new(items.iter().map(|(l, k, _)| (l.to_string(), k.to_string())).collect());
        popup.max_rows = 12;
        popup.place(at, self.screen());
        self.ex.menu = Some(ExMenu { popup, at, row, actions: items.iter().map(|(_, _, a)| *a).collect() });
    }

    /// Keys for the Explorer menu, and the Explorer's own keys. True when used.
    pub(super) fn ex_key(&mut self, k: &KeyEvent) -> bool {
        if let Some(m) = &mut self.ex.menu {
            match m.popup.handle_key(k) {
                PopupResult::Accept(i) => {
                    let (a, row) = (m.actions[i], m.row);
                    self.ex.menu = None;
                    self.ex_command(a, row);
                }
                PopupResult::Close => self.ex.menu = None,
                _ => {}
            }
            return true;
        }
        if self.shell.focus != Focus::View(Tab::Explorer) {
            return false;
        }
        let row = self.shell.explorer.list.selected;
        let r = self.shell.explorer.list.area;
        let at = Point::new(r.x + 4, r.y + (row as i32 - self.shell.explorer.list.scroll as i32).max(0));
        match k.key {
            Key::F(2) if k.mods == Mods::NONE => self.ex_command("rename", row),
            Key::Delete if k.mods == Mods::NONE => self.ex_command("delete", row),
            Key::F(10) if k.mods.shift => self.open_ex_menu(row, at),
            _ => return false,
        }
        true
    }

    /// Clicks on the Explorer's menu, its header buttons and right-clicks
    /// on its rows. True when used.
    pub(super) fn ex_mouse(&mut self, m: &MouseEvent) -> bool {
        if let Some(menu) = &mut self.ex.menu {
            match menu.popup.handle_mouse(m) {
                PopupResult::Accept(i) => {
                    let (a, row) = (menu.actions[i], menu.row);
                    self.ex.menu = None;
                    self.ex_command(a, row);
                }
                PopupResult::Close => self.ex.menu = None,
                _ => {}
            }
            return true;
        }
        let l = self.layout();
        let Some(side) = self.shell.dock.side_of(Tab::Explorer) else { return false };
        if self.shell.dock.part(side).current() != Some(Tab::Explorer) {
            return false;
        }
        let Some(pr) = l.part(side) else { return false };
        let area = self.shell.explorer.list.area;
        if !area.contains(m.pos) {
            return false;
        }
        // The buttons at the right of the top row (the project's name).
        if m.pos.y == area.y && self.shell.explorer.list.scroll == 0 {
            if let Some(a) = self.ex_buttons(pr).into_iter().find(|(r, _)| r.contains(m.pos)).map(|(_, a)| a) {
                if matches!(m.kind, MouseKind::Down(Button::Left)) {
                    self.shell.focus = Focus::View(Tab::Explorer);
                    self.ex_command(a, 0);
                }
                return true;
            }
        }
        if let MouseKind::Down(Button::Right) = m.kind {
            self.shell.focus = Focus::View(Tab::Explorer);
            let row = self.shell.explorer.list.scroll + (m.pos.y - area.y) as usize;
            if row < self.shell.explorer.list.rows.len() {
                self.shell.explorer.list.select(row);
                self.open_ex_menu(row, m.pos);
            }
            return true;
        }
        false
    }

    /// The header buttons' places: New File, New Folder, Refresh, Collapse All.
    pub(super) fn ex_buttons(&self, part: Rect) -> Vec<(Rect, &'static str)> {
        let area = self.shell.explorer.list.area;
        let icons = self.theme.style.icons;
        let labels: [(&str, &str, &'static str); 4] = [("\u{ea7f}", "+F", "new_file"), ("\u{ea80}", "+D", "new_folder"), ("\u{eb37}", "↻", "refresh"), ("\u{eac5}", "-", "collapse")];
        let mut x = area.right() - 1 - i32::from(self.shell.explorer.list.rows.len() as i32 > area.h);
        let mut out = Vec::new();
        for (e, t, a) in labels.iter().rev() {
            let w = str_width(if icons { e } else { t }) as i32;
            x -= w + 1;
            if x <= part.x + 8 {
                break;
            }
            out.push((Rect::new(x, area.y, w, 1), *a));
        }
        out
    }

    pub(super) fn draw_ex(&mut self, c: &mut Canvas, theme: &Theme) {
        let l = self.layout();
        if let (Some(side), true) = (self.shell.dock.side_of(Tab::Explorer), self.shell.explorer.list.scroll == 0) {
            if self.shell.dock.part(side).current() == Some(Tab::Explorer) {
                if let Some(pr) = l.part(side) {
                    let icons = theme.style.icons;
                    for (r, a) in self.ex_buttons(pr) {
                        let label = match (a, icons) {
                            ("new_file", true) => "\u{ea7f}",
                            ("new_folder", true) => "\u{ea80}",
                            ("refresh", true) => "\u{eb37}",
                            ("collapse", true) => "\u{eac5}",
                            ("new_file", false) => "+F",
                            ("new_folder", false) => "+D",
                            ("refresh", false) => "↻",
                            _ => "-",
                        };
                        let row_st = if self.shell.explorer.list.selected == 0 && self.shell.focus == Focus::View(Tab::Explorer) {
                            theme.scheme.menu_selected
                        } else {
                            theme.scheme.panel
                        };
                        c.print(r.x, r.y, label, row_st);
                    }
                }
            }
        }
        if let Some(m) = &mut self.ex.menu {
            let screen = Rect::new(0, 0, self.size.0, self.size.1);
            m.popup.draw(c, m.at, screen, theme);
        }
    }

    pub(super) fn ex_nodes(&self, mut root: Node) -> Node {
        let l = self.layout();
        if let Some(side) = self.shell.dock.side_of(Tab::Explorer) {
            if let (Some(pr), true) = (l.part(side), self.shell.dock.part(side).current() == Some(Tab::Explorer) && self.shell.explorer.list.scroll == 0) {
                let names = [("new_file", "New File"), ("new_folder", "New Folder"), ("refresh", "Refresh Explorer"), ("collapse", "Collapse Folders")];
                for (r, a) in self.ex_buttons(pr) {
                    let name = names.iter().find(|(k, _)| *k == a).map(|(_, n)| *n).unwrap_or(a);
                    root = root.child(Node::new(format!("explorer.{a}"), "button", name, r));
                }
            }
        }
        if let Some(m) = &self.ex.menu {
            let mut n = Node::new("explorer.menu", "menu", "Explorer", m.popup.rect()).focused(true);
            for (i, (label, _)) in m.popup.items.iter().enumerate() {
                if let Some(r) = m.popup.item_rect(i) {
                    n = n.child(Node::new(format!("explorer.menu.{i}"), "menuitem", label.clone(), r));
                }
            }
            root = root.child(n);
        }
        root
    }
}

/// Quote a path for a shell's `cd`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
