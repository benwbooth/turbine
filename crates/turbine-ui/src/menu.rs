//! Menu model and the DOS menu bar.
//!
//! The model is plain data so the same menus can be exported to a global
//! menu bar (dbusmenu) or a native macOS menu by the app front end.

use crate::event::{Button, Key, KeyEvent, MouseEvent, MouseKind};
use crate::geom::Rect;
use crate::surface::Canvas;
use crate::theme::Theme;
use serde::Serialize;
use turbine_text::unicode::str_width;

#[derive(Clone, Debug, Serialize)]
pub struct MenuItem {
    /// Label with `~` around the hotkey letter, e.g. "~S~ave". Empty for a separator.
    pub label: String,
    pub shortcut: String,
    pub command: String,
    pub enabled: bool,
    pub checked: Option<bool>,
}

impl MenuItem {
    pub fn new(label: &str, shortcut: &str, command: &str) -> Self {
        MenuItem { label: label.into(), shortcut: shortcut.into(), command: command.into(), enabled: true, checked: None }
    }
    pub fn separator() -> Self {
        MenuItem { label: String::new(), shortcut: String::new(), command: String::new(), enabled: false, checked: None }
    }
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
    pub fn is_separator(&self) -> bool {
        self.label.is_empty()
    }
    /// Label without the `~` markers.
    pub fn plain_label(&self) -> String {
        self.label.replace('~', "")
    }
    pub fn hotkey(&self) -> Option<char> {
        hotkey(&self.label)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
}

impl Menu {
    pub fn new(title: &str, items: Vec<MenuItem>) -> Self {
        Menu { title: title.into(), items }
    }
    pub fn plain_title(&self) -> String {
        self.title.replace('~', "")
    }
}

pub fn hotkey(label: &str) -> Option<char> {
    let i = label.find('~')?;
    label[i + 1..].chars().next().map(|c| c.to_ascii_lowercase())
}

#[derive(Debug)]
pub enum MenuResult {
    Ignored,
    Consumed,
    Command(String),
}

#[derive(Debug, Default)]
pub struct MenuBar {
    pub menus: Vec<Menu>,
    /// Menu bar has keyboard focus (F10 or Alt pressed).
    pub active: bool,
    /// Index of the open drop-down, if any.
    pub open: Option<usize>,
    /// Highlighted title (when active) .
    pub title_sel: usize,
    /// Highlighted item in the open drop-down.
    pub item_sel: usize,
    /// Mouse button held since opening (release on an item picks it).
    tracking: bool,
}

impl MenuBar {
    pub fn new(menus: Vec<Menu>) -> Self {
        MenuBar { menus, ..Default::default() }
    }

    pub fn is_open(&self) -> bool {
        self.active || self.open.is_some()
    }

    pub fn close(&mut self) {
        self.active = false;
        self.open = None;
        self.tracking = false;
    }

    /// Rectangle of each title in the bar.
    pub fn title_rects(&self) -> Vec<Rect> {
        let mut x = 2;
        self.menus
            .iter()
            .map(|m| {
                let w = str_width(&m.plain_title()) as i32 + 2;
                let r = Rect::new(x, 0, w, 1);
                x += w;
                r
            })
            .collect()
    }

    /// Rectangle of the drop-down for menu `i` (frame included).
    pub fn dropdown_rect(&self, i: usize, screen: Rect) -> Rect {
        let m = &self.menus[i];
        let label_w = m.items.iter().map(|it| str_width(&it.plain_label())).max().unwrap_or(0) as i32;
        let key_w = m.items.iter().map(|it| str_width(&it.shortcut)).max().unwrap_or(0) as i32;
        let w = label_w + if key_w > 0 { key_w + 3 } else { 0 } + 6;
        let h = m.items.len() as i32 + 2;
        let tr = self.title_rects()[i];
        let x = (tr.x - 1).min(screen.right() - w - 2).max(0);
        Rect::new(x, 1, w, h)
    }

    /// Rectangle of item `j` in menu `i`'s drop-down.
    pub fn item_rect(&self, i: usize, j: usize, screen: Rect) -> Rect {
        let d = self.dropdown_rect(i, screen);
        Rect::new(d.x + 1, d.y + 1 + j as i32, d.w - 2, 1)
    }

    fn open_menu(&mut self, i: usize) {
        self.active = true;
        self.title_sel = i;
        self.open = Some(i);
        self.item_sel = self.menus[i].items.iter().position(|it| it.enabled).unwrap_or(0);
    }

    fn step_item(&mut self, dir: i32) {
        let Some(i) = self.open else { return };
        let n = self.menus[i].items.len() as i32;
        let mut j = self.item_sel as i32;
        for _ in 0..n {
            j = (j + dir).rem_euclid(n);
            if self.menus[i].items[j as usize].enabled {
                self.item_sel = j as usize;
                return;
            }
        }
    }

    fn pick(&mut self) -> MenuResult {
        let Some(i) = self.open else {
            self.open_menu(self.title_sel);
            return MenuResult::Consumed;
        };
        let it = &self.menus[i].items[self.item_sel];
        if !it.enabled {
            return MenuResult::Consumed;
        }
        let cmd = it.command.clone();
        self.close();
        MenuResult::Command(cmd)
    }

    pub fn handle_key(&mut self, ev: &KeyEvent) -> MenuResult {
        // Alt+letter opens a menu from anywhere.
        if ev.mods.alt && !ev.mods.ctrl {
            if let Key::Char(c) = ev.key {
                if let Some(i) = self.menus.iter().position(|m| hotkey(&m.title) == Some(c.to_ascii_lowercase())) {
                    self.open_menu(i);
                    return MenuResult::Consumed;
                }
            }
        }
        if ev.is(Key::F(10), Default::default()) {
            if self.is_open() {
                self.close();
            } else {
                self.active = true;
                self.title_sel = 0;
            }
            return MenuResult::Consumed;
        }
        if !self.is_open() {
            return MenuResult::Ignored;
        }
        let n = self.menus.len();
        match ev.key {
            Key::Esc => {
                if self.open.is_some() {
                    self.open = None;
                } else {
                    self.close();
                }
            }
            Key::Left => {
                let i = (self.title_sel + n - 1) % n;
                if self.open.is_some() { self.open_menu(i) } else { self.title_sel = i }
            }
            Key::Right => {
                let i = (self.title_sel + 1) % n;
                if self.open.is_some() { self.open_menu(i) } else { self.title_sel = i }
            }
            Key::Down if self.open.is_none() => self.open_menu(self.title_sel),
            Key::Down => self.step_item(1),
            Key::Up => self.step_item(-1),
            Key::Enter => return self.pick(),
            Key::Char(c) => {
                let c = c.to_ascii_lowercase();
                if let Some(i) = self.open {
                    if let Some(j) = self.menus[i].items.iter().position(|it| it.enabled && it.hotkey() == Some(c)) {
                        self.item_sel = j;
                        return self.pick();
                    }
                } else if let Some(i) = self.menus.iter().position(|m| hotkey(&m.title) == Some(c)) {
                    self.open_menu(i);
                }
            }
            _ => {}
        }
        MenuResult::Consumed
    }

    pub fn handle_mouse(&mut self, ev: &MouseEvent, screen: Rect) -> MenuResult {
        let titles = self.title_rects();
        let on_title = titles.iter().position(|r| r.contains(ev.pos));
        let on_item = self.open.and_then(|i| {
            (0..self.menus[i].items.len()).find(|&j| self.item_rect(i, j, screen).contains(ev.pos))
        });
        match ev.kind {
            MouseKind::Down(Button::Left) => {
                if let Some(t) = on_title {
                    if self.open == Some(t) {
                        self.close();
                    } else {
                        self.open_menu(t);
                        self.tracking = true;
                    }
                    return MenuResult::Consumed;
                }
                if let Some(j) = on_item {
                    self.item_sel = j;
                    self.tracking = true;
                    return MenuResult::Consumed;
                }
                if self.is_open() {
                    let inside = self.open.is_some_and(|i| self.dropdown_rect(i, screen).contains(ev.pos));
                    if !inside {
                        self.close();
                    }
                    return MenuResult::Consumed;
                }
                if ev.pos.y == 0 {
                    return MenuResult::Consumed;
                }
                MenuResult::Ignored
            }
            MouseKind::Drag(Button::Left) | MouseKind::Moved if self.open.is_some() => {
                if let Some(t) = on_title {
                    if self.open != Some(t) && (self.tracking || ev.kind == MouseKind::Moved) {
                        self.open_menu(t);
                    }
                }
                if let Some(j) = on_item {
                    if self.menus[self.open.unwrap()].items[j].enabled {
                        self.item_sel = j;
                    }
                }
                MenuResult::Consumed
            }
            MouseKind::Up(Button::Left) if self.open.is_some() => {
                let was_tracking = std::mem::take(&mut self.tracking);
                if let Some(j) = on_item {
                    self.item_sel = j;
                    return self.pick();
                }
                let _ = was_tracking;
                MenuResult::Consumed
            }
            _ if self.is_open() => MenuResult::Consumed,
            _ => MenuResult::Ignored,
        }
    }

    pub fn draw(&self, c: &mut Canvas, theme: &Theme, screen: Rect) {
        let s = &theme.scheme;
        c.hline(0, 0, screen.w, " ", s.menu);
        let titles = self.title_rects();
        for (i, (m, r)) in self.menus.iter().zip(&titles).enumerate() {
            let sel = self.active && self.title_sel == i;
            let (st, hot) = if sel { (s.menu_selected, s.menu_selected_hotkey) } else { (s.menu, s.menu_hotkey) };
            c.fill(*r, " ", st);
            c.print_hot(r.x + 1, 0, &m.title, st, hot);
        }
    }

    /// Draw the open drop-down (call after everything else so it's on top).
    pub fn draw_dropdown(&self, c: &mut Canvas, theme: &Theme, screen: Rect) {
        let Some(i) = self.open else { return };
        let s = &theme.scheme;
        let fc = theme.style.menu_frame;
        let d = self.dropdown_rect(i, screen);
        c.fill(d, " ", s.menu);
        crate::frame::draw_frame(c, d, fc, s.menu);
        if theme.style.shadows {
            c.shadow(d, s.shadow);
        }
        for (j, it) in self.menus[i].items.iter().enumerate() {
            let r = self.item_rect(i, j, screen);
            if it.is_separator() {
                c.print(d.x, r.y, "├", s.menu);
                c.hline(d.x + 1, r.y, d.w - 2, fc.h, s.menu);
                c.print(d.right() - 1, r.y, "┤", s.menu);
                continue;
            }
            let sel = j == self.item_sel;
            let (st, hot) = match (sel, it.enabled) {
                (true, _) => (s.menu_selected, s.menu_selected_hotkey),
                (false, true) => (s.menu, s.menu_hotkey),
                (false, false) => (s.menu_disabled, s.menu_disabled),
            };
            c.fill(r, " ", st);
            if it.checked == Some(true) {
                c.print(r.x, r.y, "√", st);
            }
            c.print_hot(r.x + 2, r.y, &it.label, st, if it.enabled { hot } else { st });
            if !it.shortcut.is_empty() {
                let kw = str_width(&it.shortcut) as i32;
                c.print(r.right() - kw - 1, r.y, &it.shortcut, st);
            }
        }
    }
}
