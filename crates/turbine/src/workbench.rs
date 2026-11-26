//! The IDE itself: menu bar, desktop of editor windows, dialogs, status
//! bar, and the automation endpoint agents drive it through.

use crate::agent::{Driver, Output, Step};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};
use turbine_text::Document;
use turbine_ui::automation::Node;
use turbine_ui::dialog::{Control, Dialog, DialogResult};
use turbine_ui::editor::EditorView;
use turbine_ui::event::{Button, Event, Key, KeyEvent, KeyKind, Mods, MouseEvent, MouseKind};
use turbine_ui::menu::{Menu, MenuBar, MenuItem, MenuResult};
use turbine_ui::statusbar::{StatusBar, StatusItem};
use turbine_ui::theme::{ControlStyle, Scheme, Style, Theme};
use turbine_ui::window::{Hit, Window};
use turbine_ui::{App, Point, Rect, Surface};

/// A request from the automation socket (or a script).
pub struct Request {
    pub cmd: Value,
    pub reply: Sender<Value>,
}

struct EdWin {
    win: Window,
    ed: EditorView,
}

pub struct Workbench {
    theme: Theme,
    menubar: MenuBar,
    status: StatusBar,
    /// Windows bottom to top; the last one is active.
    windows: Vec<EdWin>,
    dialog: Option<Dialog>,
    clipboard: String,
    size: (i32, i32),
    quit: bool,
    next_id: u32,
    message: Option<(String, Instant)>,
    driver: Driver,
    requests: Option<Receiver<Request>>,
    /// Window index being moved/resized with the mouse.
    dragging: Option<usize>,
    /// Editor (window index) selecting with the mouse.
    selecting: Option<usize>,
    last_click: Option<(Point, Instant)>,
}

fn menus() -> Vec<Menu> {
    let it = MenuItem::new;
    let sep = MenuItem::separator;
    vec![
        Menu::new(
            "~F~ile",
            vec![
                it("~N~ew", "Ctrl+N", "file.new"),
                it("~O~pen...", "F3", "file.open"),
                it("~S~ave", "F2", "file.save"),
                it("Save ~A~s...", "", "file.save_as"),
                sep(),
                it("E~x~it", "Alt+X", "app.quit"),
            ],
        ),
        Menu::new(
            "~E~dit",
            vec![
                it("~U~ndo", "Ctrl+Z", "edit.undo"),
                it("~R~edo", "Ctrl+Y", "edit.redo"),
                sep(),
                it("Cu~t~", "Ctrl+X", "edit.cut"),
                it("~C~opy", "Ctrl+C", "edit.copy"),
                it("~P~aste", "Ctrl+V", "edit.paste"),
                sep(),
                it("Select ~A~ll", "Ctrl+A", "edit.select_all"),
            ],
        ),
        Menu::new(
            "~S~earch",
            vec![it("~F~ind...", "Ctrl+F", "search.find").disabled(), it("~G~o to Line...", "Ctrl+G", "search.goto")],
        ),
        Menu::new(
            "~V~iew",
            vec![
                it("~L~ine Numbers", "", "view.line_numbers"),
                it("~R~ight to Left", "", "view.rtl"),
                sep(),
                it("~T~erminal", "Ctrl+`", "view.terminal").disabled(),
                it("~F~iles", "", "view.files").disabled(),
            ],
        ),
        Menu::new(
            "~R~un",
            vec![
                it("~R~un", "Ctrl+F9", "run.run").disabled(),
                it("~D~ebug", "F9", "run.debug").disabled(),
                it("Toggle ~B~reakpoint", "Ctrl+F8", "run.breakpoint").disabled(),
            ],
        ),
        Menu::new(
            "~A~I",
            vec![
                it("~A~ssistant", "Ctrl+L", "ai.assistant").disabled(),
                it("~C~omplete", "Tab", "ai.complete").disabled(),
                sep(),
                it("Agent ~D~emo", "", "agent.demo"),
                it("~S~top Agent", "Esc", "agent.stop"),
            ],
        ),
        Menu::new(
            "~O~ptions",
            vec![
                it("~C~olors...", "", "options.colors"),
                it("Control ~S~tyle...", "", "options.style"),
                it("~K~eymap...", "", "options.keymap").disabled(),
            ],
        ),
        Menu::new(
            "~W~indow",
            vec![
                it("~T~ile", "", "window.tile"),
                it("C~a~scade", "", "window.cascade"),
                it("~N~ext", "F6", "window.next"),
                it("~Z~oom", "F5", "window.zoom"),
                it("~C~lose", "Alt+F3", "window.close"),
            ],
        ),
        Menu::new("~H~elp", vec![it("~K~eyboard...", "F1", "help.keys"), it("~A~bout...", "", "help.about")]),
    ]
}

const DEMO_CODE: &str = "fn main() {\n    let name = \"Turbine\";\n    for i in 1..=3 {\n        println!(\"{i}: hello from {name}!\");\n    }\n}\n";

impl Workbench {
    pub fn new(files: &[PathBuf]) -> Self {
        let mut wb = Workbench {
            theme: Theme::default(),
            menubar: MenuBar::new(menus()),
            status: StatusBar {
                items: vec![
                    StatusItem::new("F1", "Help", "help.keys"),
                    StatusItem::new("F2", "Save", "file.save"),
                    StatusItem::new("F3", "Open", "file.open"),
                    StatusItem::new("Alt+F3", "Close", "window.close"),
                    StatusItem::new("F5", "Zoom", "window.zoom"),
                    StatusItem::new("F6", "Next", "window.next"),
                    StatusItem::new("F10", "Menu", "menu"),
                ],
                ..Default::default()
            },
            windows: Vec::new(),
            dialog: None,
            clipboard: String::new(),
            size: (80, 25),
            quit: false,
            next_id: 1,
            message: None,
            driver: Driver::new(),
            requests: None,
            dragging: None,
            selecting: None,
            last_click: None,
        };
        for f in files {
            wb.open_path(f);
        }
        if wb.windows.is_empty() {
            wb.new_window(Document::new());
        }
        wb.sync_menu_checks();
        wb
    }

    pub fn set_requests(&mut self, rx: Receiver<Request>) {
        self.requests = Some(rx);
    }

    pub fn driver_mut(&mut self) -> &mut Driver {
        &mut self.driver
    }

    fn desktop(&self) -> Rect {
        Rect::new(0, 1, self.size.0, (self.size.1 - 2).max(1))
    }

    fn screen(&self) -> Rect {
        Rect::new(0, 0, self.size.0, self.size.1)
    }

    fn say(&mut self, m: impl Into<String>) {
        self.message = Some((m.into(), Instant::now()));
    }

    fn active(&mut self) -> Option<&mut EdWin> {
        self.windows.last_mut()
    }

    fn renumber(&mut self) {
        for (i, w) in self.windows.iter_mut().enumerate() {
            w.win.number = Some(i as u32 + 1);
        }
    }

    fn new_window(&mut self, doc: Document) {
        let d = self.desktop();
        let n = self.windows.len() as i32;
        let off = (n * 2) % (d.h / 2).max(1);
        let rect = Rect::new(d.x + off, d.y + off / 2, (d.w - off).max(20), (d.h - off / 2).max(6));
        let title = doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| doc.title());
        let mut win = Window::new(self.next_id, &title, rect);
        if self.windows.is_empty() {
            win.zoomed = false;
        }
        self.next_id += 1;
        self.windows.push(EdWin { win, ed: EditorView::new(doc) });
        self.renumber();
    }

    fn open_path(&mut self, p: &Path) {
        if !p.exists() {
            let mut d = Document::new();
            d.path = Some(p.to_path_buf());
            self.new_window(d);
            self.say(format!("New file {}", p.display()));
            return;
        }
        match Document::open(p) {
            Ok(d) => {
                let note = if d.lossy { " (some bytes could not be decoded)" } else { "" };
                let msg = format!("Opened {} as {}{note}", p.display(), d.encoding.name());
                // Replace an untouched empty window.
                if self.windows.len() == 1 && self.windows[0].ed.doc.path.is_none() && !self.windows[0].ed.doc.dirty {
                    self.windows.clear();
                }
                self.new_window(d);
                self.say(msg);
            }
            Err(e) => self.say(format!("Can't open {}: {e}", p.display())),
        }
    }

    fn sync_menu_checks(&mut self) {
        let (ln, rtl) = self
            .windows
            .last()
            .map(|w| (w.ed.line_numbers, w.ed.base_rtl == Some(true)))
            .unwrap_or((true, false));
        for m in &mut self.menubar.menus {
            for it in &mut m.items {
                match it.command.as_str() {
                    "view.line_numbers" => it.checked = Some(ln),
                    "view.rtl" => it.checked = Some(rtl),
                    _ => {}
                }
            }
        }
    }

    // ----- commands ---------------------------------------------------------

    pub fn run_command(&mut self, cmd: &str) {
        let desk = self.desktop();
        match cmd {
            "file.new" => self.new_window(Document::new()),
            "file.open" => self.open_dialog(std::env::current_dir().unwrap_or_default()),
            "file.save" => self.save(false),
            "file.save_as" => self.save(true),
            "app.quit" => {
                if self.windows.iter().any(|w| w.ed.doc.dirty) {
                    self.unsaved_dialog("quit");
                } else {
                    self.quit = true;
                }
            }
            "edit.undo" | "edit.redo" | "edit.cut" | "edit.copy" | "edit.paste" | "edit.select_all" => {
                let k = match cmd {
                    "edit.undo" => 'z',
                    "edit.redo" => 'y',
                    "edit.cut" => 'x',
                    "edit.copy" => 'c',
                    "edit.paste" => 'v',
                    _ => 'a',
                };
                let mut clip = std::mem::take(&mut self.clipboard);
                if let Some(w) = self.active() {
                    w.ed.handle_key(&KeyEvent::new(Key::Char(k), Mods::CTRL), &mut clip);
                }
                self.clipboard = clip;
            }
            "search.goto" => {
                let d = Dialog::new("goto", "Go to Line", 36, 8)
                    .input("line", 1, 2, 30, "~L~ine number:", "")
                    .button("ok", 6, 4, "O~K~", true)
                    .button("cancel", 18, 4, "Cancel", false);
                self.dialog = Some(d);
            }
            "view.line_numbers" => {
                if let Some(w) = self.active() {
                    w.ed.line_numbers = !w.ed.line_numbers;
                }
            }
            "view.rtl" => {
                if let Some(w) = self.active() {
                    w.ed.base_rtl = if w.ed.base_rtl == Some(true) { None } else { Some(true) };
                }
            }
            "options.colors" => {
                let names: Vec<String> = Scheme::all().iter().map(|s| s.name.to_string()).collect();
                let cur = names.iter().position(|n| n == self.theme.scheme.name).unwrap_or(0);
                let d = Dialog::new("colors", "Colors", 40, 12)
                    .label(1, 0, "Color scheme:")
                    .list("scheme", Rect::new(1, 1, 22, 7), names, cur)
                    .button("ok", 25, 2, "O~K~", true)
                    .button("cancel", 25, 5, "Cancel", false);
                self.dialog = Some(d);
            }
            "options.style" => {
                let names: Vec<String> = ControlStyle::all().iter().map(|s| s.name.to_string()).collect();
                let cur = names.iter().position(|n| n == self.theme.style.name).unwrap_or(0);
                let d = Dialog::new("style", "Control Style", 40, 12)
                    .label(1, 0, "Draw controls like:")
                    .list("style", Rect::new(1, 1, 22, 7), names, cur)
                    .button("ok", 25, 2, "O~K~", true)
                    .button("cancel", 25, 5, "Cancel", false);
                self.dialog = Some(d);
            }
            "window.tile" => self.tile(),
            "window.cascade" => {
                for (i, w) in self.windows.iter_mut().enumerate() {
                    let o = i as i32;
                    w.win.zoomed = false;
                    w.win.rect = Rect::new(desk.x + o, desk.y + o, (desk.w - o).max(20), (desk.h - o).max(6));
                }
            }
            "window.next" => {
                if self.windows.len() > 1 {
                    let w = self.windows.remove(0);
                    self.windows.push(w);
                }
            }
            "window.zoom" => {
                if let Some(w) = self.active() {
                    w.win.toggle_zoom(desk);
                }
            }
            "window.close" => {
                if self.windows.last().is_some_and(|w| w.ed.doc.dirty) {
                    self.unsaved_dialog("close");
                } else {
                    self.windows.pop();
                    self.renumber();
                }
            }
            "help.about" => {
                let d = Dialog::new("about", "About", 46, 11)
                    .label(15, 1, "T U R B I N E")
                    .label(2, 3, "A retro, AI-first code editor and IDE")
                    .label(2, 4, &format!("Version {}  -  MIT License", env!("CARGO_PKG_VERSION")))
                    .label(2, 5, "github.com/benwbooth/turbine")
                    .button("ok", 17, 7, "O~K~", true);
                self.dialog = Some(d);
            }
            "help.keys" => {
                let lines = [
                    "F10 / Alt+letter   Menus",
                    "F2  Save    F3  Open    Ctrl+N  New",
                    "F5  Zoom    F6  Next    Alt+F3  Close",
                    "Alt+1..9    Go to window",
                    "Shift+arrows select; Ctrl+X/C/V clipboard",
                    "Ctrl+Z/Y undo/redo; Ins overwrite",
                    "Drag title to move, edges to resize",
                    "Esc stops an agent; any key pauses it",
                ];
                let mut d = Dialog::new("keys", "Keyboard", 50, lines.len() as i32 + 6);
                for (i, l) in lines.iter().enumerate() {
                    d = d.label(1, i as i32 + 1, l);
                }
                d = d.button("ok", 19, lines.len() as i32 + 2, "O~K~", true);
                self.dialog = Some(d);
            }
            "agent.demo" => self.queue_demo(),
            "agent.stop" => {
                self.driver.cancel();
                self.say("Agent stopped");
            }
            "menu" => {
                self.menubar.active = true;
                self.menubar.title_sel = 0;
            }
            other => self.say(format!("{other}: not available yet")),
        }
        self.sync_menu_checks();
    }

    fn tile(&mut self) {
        let d = self.desktop();
        let n = self.windows.len() as i32;
        if n == 0 {
            return;
        }
        let cols = (n as f64).sqrt().ceil() as i32;
        let rows = (n + cols - 1) / cols;
        for (i, w) in self.windows.iter_mut().enumerate() {
            let i = i as i32;
            let (c, r) = (i % cols, i / cols);
            let x0 = d.x + d.w * c / cols;
            let x1 = d.x + d.w * (c + 1) / cols;
            let y0 = d.y + d.h * r / rows;
            let y1 = d.y + d.h * (r + 1) / rows;
            w.win.zoomed = false;
            w.win.rect = Rect::new(x0, y0, x1 - x0, y1 - y0);
        }
    }

    fn save(&mut self, ask: bool) {
        let Some(w) = self.windows.last_mut() else { return };
        if ask || w.ed.doc.path.is_none() {
            let cur = w.ed.doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            let d = Dialog::new("save_as", "Save As", 52, 8)
                .input("path", 1, 2, 46, "File ~n~ame:", &cur)
                .button("ok", 12, 4, "O~K~", true)
                .button("cancel", 28, 4, "Cancel", false);
            self.dialog = Some(d);
            return;
        }
        let r = w.ed.doc.save();
        let msg = match r {
            Ok(()) => format!("Saved {} ({}, {})", w.ed.doc.title(), w.ed.doc.encoding.name(), w.ed.doc.line_ending.name()),
            Err(e) => format!("Save failed: {e}"),
        };
        self.say(msg);
    }

    fn open_dialog(&mut self, dir: PathBuf) {
        let mut items: Vec<String> = Vec::new();
        if dir.parent().is_some() {
            items.push("../".into());
        }
        let mut entries: Vec<(bool, String)> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| {
                        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        (!is_dir, e.file_name().to_string_lossy().into_owned())
                    })
                    .filter(|(_, n)| !n.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        entries.sort();
        items.extend(entries.into_iter().map(|(f, n)| if f { n } else { format!("{n}/") }));
        let title = format!("Open - {}", dir.display());
        let d = Dialog::new(&format!("open:{}", dir.display()), &title, 60, 18)
            .input("path", 1, 2, 40, "File ~n~ame:", "")
            .label(1, 4, "Files:")
            .list("files", Rect::new(1, 5, 40, 10), items, 0)
            .button("ok", 44, 2, "~O~pen", true)
            .button("cancel", 44, 5, "Cancel", false);
        self.dialog = Some(d);
    }

    fn unsaved_dialog(&mut self, then: &str) {
        let name = self.windows.iter().rev().find(|w| w.ed.doc.dirty).map(|w| w.ed.doc.title()).unwrap_or_default();
        let d = Dialog::new(&format!("unsaved:{then}"), "Unsaved Changes", 50, 8)
            .label(1, 1, &format!("{name} has unsaved changes."))
            .button("save", 2, 3, "~S~ave", true)
            .button("discard", 15, 3, "~D~on't Save", false)
            .button("cancel", 33, 3, "Cancel", false);
        self.dialog = Some(d);
    }

    fn finish_dialog(&mut self, d: Dialog, button: Option<String>) {
        let Some(b) = button else { return };
        if b == "cancel" {
            return;
        }
        let kind = d.kind.clone();
        if let Some(dir) = kind.strip_prefix("open:") {
            let dir = PathBuf::from(dir);
            let typed = d.value("path").unwrap_or_default();
            let pick = if typed.trim().is_empty() { d.value("files").unwrap_or_default() } else { typed };
            let p = if Path::new(&pick).is_absolute() { PathBuf::from(&pick) } else { dir.join(&pick) };
            if pick.ends_with('/') || p.is_dir() {
                let p = if pick == "../" { dir.parent().map(Path::to_path_buf).unwrap_or(dir) } else { p };
                self.open_dialog(p);
            } else if !pick.is_empty() {
                self.open_path(&p);
            }
            return;
        }
        match kind.as_str() {
            "save_as" => {
                let p = d.value("path").unwrap_or_default();
                if p.trim().is_empty() {
                    return;
                }
                let p = PathBuf::from(p.trim());
                if let Some(w) = self.windows.last_mut() {
                    let r = w.ed.doc.save_as(&p);
                    w.win.title = p.display().to_string();
                    let msg = match r {
                        Ok(()) => format!("Saved {}", p.display()),
                        Err(e) => format!("Save failed: {e}"),
                    };
                    self.say(msg);
                }
            }
            "goto" => {
                if let Ok(n) = d.value("line").unwrap_or_default().trim().parse::<usize>() {
                    if let Some(w) = self.active() {
                        let line = n.saturating_sub(1).min(w.ed.doc.len_lines().saturating_sub(1));
                        w.ed.cursor = w.ed.doc.rope.line_to_char(line);
                        w.ed.anchor = None;
                    }
                }
            }
            "colors" => {
                if let Some(i) = d.selected_index("scheme") {
                    if let Some(s) = Scheme::all().into_iter().nth(i) {
                        self.theme.scheme = s;
                    }
                }
            }
            "style" => {
                if let Some(i) = d.selected_index("style") {
                    if let Some(s) = ControlStyle::all().into_iter().nth(i) {
                        self.theme.style = s;
                    }
                }
            }
            k if k.starts_with("unsaved:") => {
                let then = &k["unsaved:".len()..];
                let idx = self.windows.iter().rposition(|w| w.ed.doc.dirty);
                match b.as_str() {
                    "save" => {
                        if let Some(i) = idx {
                            let w = self.windows.remove(i);
                            self.windows.push(w);
                            if self.windows.last().unwrap().ed.doc.path.is_none() {
                                self.save(true);
                                return;
                            }
                            self.save(false);
                        }
                    }
                    "discard" => {
                        if let Some(i) = idx {
                            self.windows[i].ed.doc.dirty = false;
                        }
                    }
                    _ => return,
                }
                if then == "close" {
                    self.windows.pop();
                    self.renumber();
                } else if then == "quit" {
                    self.run_command("app.quit");
                }
            }
            _ => {}
        }
    }

    // ----- input ------------------------------------------------------------

    /// Input from the person at the keyboard: pauses a running agent.
    fn human_input(&mut self, ev: &Event) {
        if !self.driver.busy() {
            return;
        }
        match ev {
            Event::Key(k) if k.key == Key::Esc => {
                self.driver.cancel();
                self.say("Agent stopped");
            }
            Event::Key(k) if k.key == Key::F(12) => {
                self.driver.paused = !self.driver.paused;
            }
            Event::Key(_) | Event::Mouse(MouseEvent { kind: MouseKind::Down(_), .. }) => {
                self.driver.paused = true;
            }
            _ => {}
        }
    }

    fn dispatch(&mut self, ev: Event) {
        match ev {
            Event::Resize(w, h) => {
                self.size = (w, h);
                let d = self.desktop();
                for w in &mut self.windows {
                    w.win.fit(d);
                }
            }
            Event::Key(k) => self.key(k),
            Event::Mouse(m) => self.mouse(m),
            Event::Paste(s) => {
                if let Some(d) = &mut self.dialog {
                    d.handle_paste(&s);
                } else if let Some(w) = self.active() {
                    w.ed.insert_text(&s);
                }
            }
            Event::Focus(_) => {}
        }
    }

    fn key(&mut self, k: KeyEvent) {
        if k.kind == KeyKind::Release {
            return;
        }
        if let Some(d) = &mut self.dialog {
            match d.handle_key(&k) {
                DialogResult::None => {}
                DialogResult::Cancel => self.dialog = None,
                DialogResult::Button(b) => {
                    let d = self.dialog.take().unwrap();
                    self.finish_dialog(d, Some(b));
                }
            }
            return;
        }
        match self.menubar.handle_key(&k) {
            MenuResult::Command(c) => return self.run_command(&c),
            MenuResult::Consumed => return,
            MenuResult::Ignored => {}
        }
        let shortcut = if k.is_ctrl('n') {
            Some("file.new")
        } else if k.is(Key::F(3), Mods::NONE) || k.is_ctrl('o') {
            Some("file.open")
        } else if k.is(Key::F(2), Mods::NONE) || k.is_ctrl('s') {
            Some("file.save")
        } else if k.is_alt('x') || k.is_ctrl('q') {
            Some("app.quit")
        } else if k.is(Key::F(6), Mods::NONE) {
            Some("window.next")
        } else if k.is(Key::F(5), Mods::NONE) {
            Some("window.zoom")
        } else if k.is(Key::F(3), Mods::ALT) || k.is_ctrl('w') {
            Some("window.close")
        } else if k.is(Key::F(1), Mods::NONE) {
            Some("help.keys")
        } else if k.is_ctrl('g') {
            Some("search.goto")
        } else {
            None
        };
        if let Some(c) = shortcut {
            return self.run_command(c);
        }
        if k.mods.alt && !k.mods.ctrl {
            if let Key::Char(c @ '1'..='9') = k.key {
                let n = c as usize - '1' as usize;
                if n < self.windows.len() {
                    let w = self.windows.remove(n);
                    self.windows.push(w);
                    self.renumber();
                }
                return;
            }
        }
        let mut clip = std::mem::take(&mut self.clipboard);
        if let Some(w) = self.windows.last_mut() {
            w.ed.handle_key(&k, &mut clip);
        }
        self.clipboard = clip;
    }

    fn mouse(&mut self, m: MouseEvent) {
        let screen = self.screen();
        let desk = self.desktop();
        if let Some(d) = &mut self.dialog {
            match d.handle_mouse(&m) {
                DialogResult::Button(b) => {
                    let d = self.dialog.take().unwrap();
                    self.finish_dialog(d, Some(b));
                }
                DialogResult::Cancel => self.dialog = None,
                DialogResult::None => {}
            }
            return;
        }
        match self.menubar.handle_mouse(&m, screen) {
            MenuResult::Command(c) => return self.run_command(&c),
            MenuResult::Consumed => return,
            MenuResult::Ignored => {}
        }
        if let Some(i) = self.dragging {
            match m.kind {
                MouseKind::Drag(Button::Left) => self.windows[i].win.drag_to(m.pos, desk),
                _ => {
                    self.windows[i].win.end_drag();
                    self.dragging = None;
                }
            }
            return;
        }
        if let Some(i) = self.selecting {
            if i < self.windows.len() {
                self.windows[i].ed.handle_mouse(&m);
            }
            if !matches!(m.kind, MouseKind::Drag(_)) {
                self.selecting = None;
            }
            return;
        }
        if m.pos.y == self.size.1 - 1 {
            if let MouseKind::Down(Button::Left) = m.kind {
                if let Some(c) = self.status.hit(m.pos, self.size.1 - 1) {
                    self.run_command(&c);
                }
            }
            return;
        }
        let Some(i) = self.windows.iter().rposition(|w| w.win.rect.contains(m.pos)) else { return };
        match m.kind {
            MouseKind::Down(Button::Left) => {
                if i != self.windows.len() - 1 {
                    let w = self.windows.remove(i);
                    self.windows.push(w);
                    self.renumber();
                }
                let i = self.windows.len() - 1;
                let double = self.last_click.is_some_and(|(p, t)| p == m.pos && t.elapsed() < Duration::from_millis(400));
                self.last_click = Some((m.pos, Instant::now()));
                let w = &mut self.windows[i];
                match w.win.hit(m.pos) {
                    Hit::Close => self.run_command("window.close"),
                    Hit::Zoom => w.win.toggle_zoom(desk),
                    Hit::Title if double => w.win.toggle_zoom(desk),
                    h @ (Hit::Title | Hit::Left | Hit::Right | Hit::Bottom | Hit::BottomLeft | Hit::BottomRight) => {
                        if w.win.begin_drag(h, m.pos) {
                            self.dragging = Some(i);
                        }
                    }
                    Hit::VScroll => {
                        let r = w.win.rect;
                        if m.pos.y == r.y + 1 {
                            w.ed.scroll_y = w.ed.scroll_y.saturating_sub(1);
                        } else if m.pos.y == r.bottom() - 2 {
                            w.ed.scroll_y += 1;
                        } else {
                            let frac = (m.pos.y - r.y - 2) as f64 / (r.h - 4).max(1) as f64;
                            let line = (frac * w.ed.doc.len_lines() as f64) as usize;
                            w.ed.cursor = w.ed.doc.rope.line_to_char(line.min(w.ed.doc.len_lines().saturating_sub(1)));
                        }
                    }
                    Hit::Inside => {
                        if w.ed.handle_mouse(&m) {
                            self.selecting = Some(i);
                        }
                    }
                    _ => {}
                }
            }
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                let w = &mut self.windows[i];
                let mut m2 = m;
                m2.pos = w.win.inner().center();
                w.ed.handle_mouse(&m2);
            }
            _ => {}
        }
    }

    // ----- automation -------------------------------------------------------

    pub fn tree(&self) -> Node {
        let screen = self.screen();
        let mut root = Node::new("workbench", "application", "Turbine", screen);
        let mut bar = Node::new("menubar", "menubar", "Menu", Rect::new(0, 0, screen.w, 1));
        let titles = self.menubar.title_rects();
        for (i, m) in self.menubar.menus.iter().enumerate() {
            let key = m.plain_title().to_lowercase();
            let mut mn = Node::new(format!("menu.{key}"), "menu", m.plain_title(), titles[i]);
            if self.menubar.open == Some(i) {
                mn.focused = true;
                for (j, it) in m.items.iter().enumerate() {
                    if it.is_separator() {
                        continue;
                    }
                    let mut n = Node::new(
                        format!("menuitem.{}", it.command),
                        "menuitem",
                        it.plain_label(),
                        self.menubar.item_rect(i, j, screen),
                    )
                    .enabled(it.enabled);
                    if let Some(c) = it.checked {
                        n = n.value(if c { "checked" } else { "unchecked" });
                    }
                    mn = mn.child(n);
                }
            }
            bar = bar.child(mn);
        }
        root = root.child(bar);
        let top = self.windows.len().saturating_sub(1);
        for (i, w) in self.windows.iter().enumerate() {
            let id = w.win.id;
            let (line, col) = w.ed.cursor_line_col();
            let mut wn = Node::new(format!("window.{id}"), "window", w.win.title.clone(), w.win.rect)
                .focused(i == top && self.dialog.is_none());
            wn = wn.child(Node::new(format!("window.{id}.close"), "button", "Close", w.win.close_rect()));
            wn = wn.child(Node::new(format!("window.{id}.zoom"), "button", "Zoom", w.win.zoom_rect()));
            let mut ed = Node::new(format!("editor.{id}"), "editor", w.ed.doc.title(), w.win.inner())
                .focused(i == top && self.dialog.is_none())
                .value(format!(
                    "line {} col {} of {} lines{}",
                    line + 1,
                    col + 1,
                    w.ed.doc.len_lines(),
                    if w.ed.doc.dirty { ", modified" } else { "" }
                ));
            ed.enabled = true;
            wn = wn.child(ed);
            root = root.child(wn);
        }
        if let Some(d) = &self.dialog {
            let mut dn = Node::new(format!("dialog.{}", d.kind), "dialog", d.title.clone(), d.rect).focused(true);
            for (i, p) in d.controls.iter().enumerate() {
                let r = d.control_rect(i);
                let (name, value) = match &p.control {
                    Control::Label { text } => (text.clone(), None),
                    Control::Input { label, value, .. } => (label.replace('~', ""), Some(value.clone())),
                    Control::Button { label, .. } => (label.replace('~', ""), None),
                    Control::List { items, selected, .. } => {
                        (format!("{} items", items.len()), items.get(*selected).cloned())
                    }
                };
                let id = p.control.id().map(|s| format!("dialog.{s}")).unwrap_or_else(|| format!("dialog.label{i}"));
                let mut n = Node::new(id, p.control.role(), name, r).focused(i == d.focus);
                if let Some(v) = value {
                    n = n.value(v);
                }
                if let Control::List { items, scroll, .. } = &p.control {
                    for (row, it) in items.iter().enumerate().skip(*scroll).take(r.h as usize) {
                        let ir = Rect::new(r.x, r.y + (row - scroll) as i32, r.w, 1);
                        let cid = format!("{}.{row}", n.id);
                        n = n.child(Node::new(cid, "listitem", it.clone(), ir));
                    }
                }
                dn = dn.child(n);
            }
            root = root.child(dn);
        }
        let srow = self.size.1 - 1;
        let mut sb = Node::new("statusbar", "statusbar", "Status", Rect::new(0, srow, screen.w, 1));
        for (it, r) in self.status.items.iter().zip(self.status.item_rects(srow)) {
            sb = sb.child(Node::new(format!("status.{}", it.command), "button", format!("{} {}", it.key, it.label), r));
        }
        root.child(sb)
    }

    /// Queue the steps that click a widget like a person would.
    fn queue_click(&mut self, target: &str) {
        self.driver.queue.push_back(Step::MoveToTarget(target.into()));
        self.driver.queue.push_back(Step::Press(Button::Left));
        self.driver.queue.push_back(Step::Release(Button::Left));
    }

    fn queue_menu(&mut self, path: &str) {
        let parts: Vec<&str> = path.split('/').map(str::trim).filter(|s| !s.is_empty()).collect();
        if let Some((first, rest)) = parts.split_first() {
            self.queue_click(&format!("menu:{first}"));
            for p in rest {
                self.queue_click(&format!("menuitem:{p}"));
            }
        }
    }

    fn queue_type(&mut self, text: &str) {
        for c in text.chars() {
            self.driver.queue.push_back(match c {
                // Literal text: no auto-indent on top of the text's own.
                '\n' => Step::Key(KeyEvent::new(Key::Enter, Mods::SHIFT)),
                '\t' => Step::Key(KeyEvent::plain(Key::Tab)),
                c => Step::Char(c),
            });
        }
    }

    fn queue_demo(&mut self) {
        self.driver.agent_name = "Demo agent".into();
        self.queue_menu("File/New");
        self.driver.queue.push_back(Step::Pause(Duration::from_millis(300)));
        self.queue_type(DEMO_CODE);
        self.driver.queue.push_back(Step::Pause(Duration::from_millis(400)));
        self.queue_menu("Window/Tile");
        self.queue_menu("Options/Colors");
        self.queue_click("listitem:Borland Cyan");
        self.queue_click("button:OK");
    }

    /// Handle one automation request. Actions reply when they finish.
    pub fn request(&mut self, cmd: Value, reply: Sender<Value>) {
        let name = cmd.get("cmd").and_then(Value::as_str).unwrap_or("").to_string();
        let s = |k: &str| cmd.get(k).and_then(Value::as_str).map(str::to_string);
        let ok = json!({"ok": true});
        match name.as_str() {
            "snapshot" => {
                let mut v = json!({"ok": true, "tree": self.tree()});
                if cmd.get("screen").and_then(Value::as_bool).unwrap_or(false) {
                    let mut surf = Surface::new(self.size.0, self.size.1);
                    self.render(&mut surf);
                    v["screen"] = Value::String(surf.to_text());
                }
                let _ = reply.send(v);
                return;
            }
            "text" => {
                let t = self.windows.last().map(|w| w.ed.doc.rope.to_string()).unwrap_or_default();
                let _ = reply.send(json!({"ok": true, "text": t}));
                return;
            }
            "speed" => {
                if let Some(c) = cmd.get("cps").and_then(Value::as_f64) {
                    self.driver.cps = c;
                }
                let _ = reply.send(json!({"ok": true, "cps": self.driver.cps}));
                return;
            }
            "click" => {
                if let (Some(x), Some(y)) = (cmd.get("x").and_then(Value::as_i64), cmd.get("y").and_then(Value::as_i64)) {
                    self.driver.queue.push_back(Step::MoveTo(Point::new(x as i32, y as i32)));
                    self.driver.queue.push_back(Step::Press(Button::Left));
                    self.driver.queue.push_back(Step::Release(Button::Left));
                } else if let Some(t) = s("target") {
                    self.queue_click(&t);
                } else {
                    let _ = reply.send(json!({"ok": false, "error": "click needs target or x,y"}));
                    return;
                }
            }
            "drag" => {
                let p = |k: &str| {
                    cmd.get(k).and_then(|v| Some(Point::new(v.get(0)?.as_i64()? as i32, v.get(1)?.as_i64()? as i32)))
                };
                let (Some(a), Some(b)) = (p("from"), p("to")) else {
                    let _ = reply.send(json!({"ok": false, "error": "drag needs from:[x,y] and to:[x,y]"}));
                    return;
                };
                self.driver.queue.push_back(Step::MoveTo(a));
                self.driver.queue.push_back(Step::Press(Button::Left));
                self.driver.queue.push_back(Step::MoveTo(b));
                self.driver.queue.push_back(Step::Release(Button::Left));
            }
            "key" => {
                let chord = s("key").unwrap_or_default();
                match KeyEvent::parse(&chord) {
                    Some(k) => self.driver.queue.push_back(Step::Key(k)),
                    None => {
                        let _ = reply.send(json!({"ok": false, "error": format!("bad key {chord:?}")}));
                        return;
                    }
                }
            }
            "type" => self.queue_type(&s("text").unwrap_or_default()),
            "paste" => self.driver.queue.push_back(Step::Paste(s("text").unwrap_or_default())),
            "menu" => self.queue_menu(&s("path").unwrap_or_default()),
            "command" => self.driver.queue.push_back(Step::Command(s("name").unwrap_or_default())),
            "open" => {
                let p = PathBuf::from(s("path").unwrap_or_default());
                self.open_path(&p);
            }
            "wait" => {}
            "stop" => self.driver.cancel(),
            _ => {
                let _ = reply.send(json!({"ok": false, "error": format!("unknown cmd {name:?}")}));
                return;
            }
        }
        if let Some(a) = s("agent") {
            self.driver.agent_name = a;
        }
        self.driver.queue.push_back(Step::Reply(reply, ok));
    }

    fn apply_driver(&mut self, outs: Vec<Output>) {
        for o in outs {
            match o {
                Output::Pointer(p, held) => {
                    let kind = held.map(MouseKind::Drag).unwrap_or(MouseKind::Moved);
                    self.dispatch(Event::Mouse(MouseEvent { kind, pos: p, mods: Mods::NONE }));
                }
                Output::Press(p, b) => {
                    self.dispatch(Event::Mouse(MouseEvent { kind: MouseKind::Down(b), pos: p, mods: Mods::NONE }));
                }
                Output::Release(p, b) => {
                    self.dispatch(Event::Mouse(MouseEvent { kind: MouseKind::Up(b), pos: p, mods: Mods::NONE }));
                }
                Output::Key(k) => self.dispatch(Event::Key(k)),
                Output::Paste(s) => self.dispatch(Event::Paste(s)),
                Output::Command(c) => self.run_command(&c),
                Output::Resolve(t) => match self.tree().find_target(&t) {
                    Some(n) if n.enabled => self.driver.push_front(Step::MoveTo(n.click_point())),
                    Some(_) => {
                        self.say(format!("Agent: {t} is disabled"));
                        self.fail_queue(format!("{t} is disabled"));
                    }
                    None => {
                        self.say(format!("Agent: can't find {t}"));
                        self.fail_queue(format!("can't find {t}"));
                    }
                },
                Output::Reply(tx, v) => {
                    let _ = tx.send(v);
                }
            }
        }
    }

    /// Drop the rest of the current action and report the error.
    fn fail_queue(&mut self, err: String) {
        while let Some(s) = self.driver.queue.pop_front() {
            if let Step::Reply(tx, _) = s {
                let _ = tx.send(json!({"ok": false, "error": err}));
                break;
            }
        }
    }
}

impl App for Workbench {
    fn event(&mut self, ev: Event) {
        self.human_input(&ev);
        if self.driver.busy() && matches!(ev, Event::Key(_)) {
            return; // keys go to the agent controls while it drives
        }
        self.dispatch(ev);
    }

    fn tick(&mut self, now: Instant) -> Option<Instant> {
        if let Some(rx) = &self.requests {
            let reqs: Vec<Request> = rx.try_iter().collect();
            for r in reqs {
                self.request(r.cmd, r.reply);
            }
        }
        let mut next = None;
        for _ in 0..10_000 {
            let (outs, n) = self.driver.tick(now);
            let progressed = !outs.is_empty();
            self.apply_driver(outs);
            next = n;
            // Instant mode, or a resolve that pushed a step: keep going.
            if !(progressed && (self.driver.instant() || n == Some(now))) {
                break;
            }
        }
        if let Some((_, t)) = &self.message {
            if t.elapsed() > Duration::from_secs(4) {
                self.message = None;
            }
        }
        let msg_tick = self.message.as_ref().map(|(_, t)| *t + Duration::from_secs(4));
        let poll = self.requests.as_ref().map(|_| now + Duration::from_millis(50));
        [next, msg_tick, poll].into_iter().flatten().min()
    }

    fn render(&mut self, surf: &mut Surface) {
        if surf.w != self.size.0 || surf.h != self.size.1 {
            *surf = Surface::new(self.size.0, self.size.1);
        }
        surf.cursor = None;
        let theme = self.theme.clone();
        let screen = self.screen();
        let desk = self.desktop();
        let mut c = surf.canvas();
        c.fill(desk, theme.scheme.desktop_char, theme.scheme.desktop);
        let n = self.windows.len();
        let has_modal = self.dialog.is_some() || self.menubar.is_open();
        for (i, w) in self.windows.iter_mut().enumerate() {
            let active = i + 1 == n;
            let mut wc = c.sub(desk);
            if theme.style.shadows {
                wc.shadow(w.win.rect, theme.scheme.shadow);
            }
            w.win.draw_frame(&mut wc, &theme, active);
            let inner = w.win.inner();
            w.ed.draw(&mut wc, inner, &theme, active && !has_modal, active);
            if active {
                w.ed.draw_scrollbars(&mut wc, w.win.rect, &theme);
                if w.ed.doc.dirty {
                    wc.print(w.win.rect.x + 1, w.win.rect.bottom() - 1, "*", theme.scheme.frame_icons);
                }
            }
        }
        // Status bar.
        let info = self
            .windows
            .last()
            .map(|w| {
                let (l, col) = w.ed.cursor_line_col();
                format!(
                    "{}:{}  {} {}  {}",
                    l + 1,
                    col + 1,
                    w.ed.doc.encoding.name(),
                    w.ed.doc.line_ending.name(),
                    if w.ed.overwrite { "OVR" } else { "INS" }
                )
            })
            .unwrap_or_default();
        self.status.info = info;
        self.status.message = if self.driver.busy() {
            Some(format!(
                "{} {} - Esc stops, F12 {}",
                if self.driver.paused { "⏸" } else { "►" },
                self.driver.agent_name,
                if self.driver.paused { "resumes" } else { "pauses" }
            ))
        } else {
            self.message.as_ref().map(|(m, _)| m.clone())
        };
        self.status.draw(&mut c, &theme, Rect::new(0, self.size.1 - 1, self.size.0, 1));
        self.menubar.draw(&mut c, &theme, screen);
        if let Some(d) = &mut self.dialog {
            d.layout(screen);
            d.draw(&mut c, &theme);
        }
        self.menubar.draw_dropdown(&mut c, &theme, screen);
        // The agent's pointer: a highlighted cell, like a DOS mouse cursor.
        if let Some(p) = self.driver.pointer {
            if self.driver.busy() {
                let st: Style = theme.scheme.agent_pointer;
                c.set_style_at(p, st);
            }
        }
    }

    fn should_quit(&self) -> bool {
        self.quit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn run(wb: &mut Workbench, cmd: Value) -> Value {
        let (tx, rx) = mpsc::channel();
        wb.request(cmd, tx);
        for _ in 0..1000 {
            wb.tick(Instant::now());
            if let Ok(v) = rx.try_recv() {
                return v;
            }
        }
        panic!("no reply");
    }

    fn bench() -> Workbench {
        let mut wb = Workbench::new(&[]);
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        wb
    }

    #[test]
    fn agent_types_through_the_ui() {
        let mut wb = bench();
        assert_eq!(run(&mut wb, json!({"cmd": "menu", "path": "File/New"}))["ok"], true);
        assert_eq!(wb.windows.len(), 2);
        run(&mut wb, json!({"cmd": "type", "text": "hello\nworld"}));
        assert_eq!(run(&mut wb, json!({"cmd": "text"}))["text"], "hello\nworld");
        run(&mut wb, json!({"cmd": "key", "key": "Ctrl+A"}));
        run(&mut wb, json!({"cmd": "key", "key": "Ctrl+X"}));
        assert_eq!(wb.clipboard, "hello\nworld");
    }

    #[test]
    fn agent_picks_a_color_scheme_in_a_dialog() {
        let mut wb = bench();
        run(&mut wb, json!({"cmd": "menu", "path": "Options/Colors..."}));
        assert!(wb.dialog.is_some());
        run(&mut wb, json!({"cmd": "click", "target": "listitem:QBasic"}));
        let r = run(&mut wb, json!({"cmd": "click", "target": "button:OK"}));
        assert_eq!(r["ok"], true);
        assert_eq!(wb.theme.scheme.name, "QBasic");
    }

    #[test]
    fn missing_target_fails_cleanly() {
        let mut wb = bench();
        let r = run(&mut wb, json!({"cmd": "click", "target": "button:Nope"}));
        assert_eq!(r["ok"], false);
    }

    #[test]
    fn window_drag_resizes() {
        let mut wb = bench();
        let r0 = wb.windows[0].win.rect;
        let corner = [r0.right() - 1, r0.bottom() - 1];
        run(&mut wb, json!({"cmd": "drag", "from": corner, "to": [corner[0] - 20, corner[1] - 5]}));
        let r1 = wb.windows[0].win.rect;
        assert_eq!((r1.w, r1.h), (r0.w - 20, r0.h - 5));
    }
}
