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
    /// A submenu's items (it opens at the side, ►), as VS Code's
    /// Preferences and Editor Layout.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub submenu: Vec<MenuItem>,
}

impl MenuItem {
    pub fn new(label: &str, shortcut: &str, command: &str) -> Self {
        MenuItem { label: label.into(), shortcut: shortcut.into(), command: command.into(), enabled: true, checked: None, submenu: Vec::new() }
    }
    /// An item that opens a submenu.
    pub fn sub(label: &str, items: Vec<MenuItem>) -> Self {
        MenuItem { label: label.into(), shortcut: String::new(), command: String::new(), enabled: true, checked: None, submenu: items }
    }
    pub fn separator() -> Self {
        MenuItem { label: String::new(), shortcut: String::new(), command: String::new(), enabled: false, checked: None, submenu: Vec::new() }
    }
    pub fn has_submenu(&self) -> bool {
        !self.submenu.is_empty()
    }
    /// It, and every item in its submenus (to find one by its command).
    pub fn walk<'a>(&'a self, out: &mut Vec<&'a MenuItem>) {
        out.push(self);
        for s in &self.submenu {
            s.walk(out);
        }
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
    /// Every item, those in submenus too.
    pub fn all_items(&self) -> Vec<&MenuItem> {
        let mut v = Vec::new();
        for it in &self.items {
            it.walk(&mut v);
        }
        v
    }
    /// Every item, mutable, those in submenus too.
    pub fn for_each_item_mut(&mut self, f: &mut impl FnMut(&mut MenuItem)) {
        fn go(items: &mut [MenuItem], f: &mut impl FnMut(&mut MenuItem)) {
            for it in items {
                f(it);
                go(&mut it.submenu, f);
            }
        }
        go(&mut self.items, f);
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
    /// The highlighted item's submenu is open, and its highlighted item.
    pub sub_open: bool,
    pub sub_sel: usize,
    /// Mouse button held since opening (release on an item picks it).
    tracking: bool,
    /// Screen width, for laying the titles out (0 = unlimited).
    pub width: i32,
    /// The last menu (Help) at the right end of the bar (QBasic, Edit).
    pub help_right: bool,
    /// Hotkey letters shown only while the bar is active.
    pub hotkeys_when_active: bool,
    /// No ≡ system menu (QBasic and Edit had none; Alt+Space still opens it).
    pub hide_system: bool,
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
        self.sub_open = false;
    }

    /// Rectangle of each title in the bar.
    /// Rectangle of each title in the bar. When they don't fit, the bar
    /// tightens (one space between titles); titles past the right edge get
    /// an empty rect and stay reachable from the keyboard.
    pub fn title_rects(&self) -> Vec<Rect> {
        let widths: Vec<i32> = self.menus.iter().map(|m| str_width(&m.plain_title()) as i32).collect();
        let roomy: i32 = 2 + widths.iter().map(|w| w + 2).sum::<i32>();
        let tight = self.width > 0 && roomy > self.width;
        let (mut x, pad) = if tight { (0, 1) } else { (2, 2) };
        let n = widths.len();
        // (Still too wide: VS Code's own menus give way first, Selection
        // then Go, keeping Borland's and QBasic's, and Help at the right;
        // their Alt keys still open them.)
        let mut gone: Vec<usize> = Vec::new();
        if tight {
            let shown = |gone: &[usize]| widths.iter().enumerate().filter(|(i, _)| !gone.contains(i) && !(self.hide_system && hotkey(&self.menus[*i].title).is_none())).map(|(_, w)| w + pad).sum::<i32>();
            for name in ["Selection", "Go"] {
                if shown(&gone) <= self.width {
                    break;
                }
                if let Some(i) = self.menus.iter().position(|m| m.plain_title() == name) {
                    gone.push(i);
                }
            }
        }
        let mut rects: Vec<Rect> = widths
            .iter()
            .enumerate()
            .map(|(i, &tw)| {
                if gone.contains(&i) {
                    return Rect::new(x, 0, 0, 1);
                }
                let w = tw + pad;
                let fits = self.width <= 0 || x + w <= self.width;
                let r = if fits { Rect::new(x, 0, w, 1) } else { Rect::new(self.width, 0, 0, 1) };
                x += w;
                r
            })
            .collect();
        if self.hide_system {
            // Shift everything left over the ≡ menu's place.
            if let Some(i) = self.menus.iter().position(|m| hotkey(&m.title).is_none()) {
                let gone = rects[i].w;
                rects[i] = Rect::new(rects[i].x, 0, 0, 1);
                for r in rects.iter_mut().skip(i + 1) {
                    if r.w > 0 {
                        r.x -= gone;
                    }
                }
            }
        }
        // Help alone at the right end, when there's room for it there.
        if self.help_right && self.width > 0 && n > 1 {
            let w = widths[n - 1] + pad;
            let right = Rect::new(self.width - w - 1, 0, w, 1);
            if rects[n - 2].right() <= right.x && rects[n - 2].w > 0 {
                rects[n - 1] = right;
            }
        }
        rects
    }

    /// Rectangle of the drop-down for menu `i` (frame included).
    pub fn dropdown_rect(&self, i: usize, screen: Rect) -> Rect {
        let m = &self.menus[i];
        let w = items_width(&m.items);
        let h = m.items.len() as i32 + 2;
        let tr = self.title_rects()[i];
        let x = (tr.x - 1).min(screen.right() - w - 2).max(0);
        let w = w.min(screen.w);
        Rect::new(x, 1, w, h)
    }

    /// Rectangle of item `j` in menu `i`'s drop-down.
    pub fn item_rect(&self, i: usize, j: usize, screen: Rect) -> Rect {
        let d = self.dropdown_rect(i, screen);
        Rect::new(d.x + 1, d.y + 1 + j as i32, d.w - 2, 1)
    }

    /// The open submenu's box: beside its item (at the left when there's
    /// no room at the right).
    pub fn sub_rect(&self, screen: Rect) -> Option<Rect> {
        let i = self.open?;
        let item = self.menus[i].items.get(self.item_sel).filter(|it| it.has_submenu() && self.sub_open)?;
        let d = self.dropdown_rect(i, screen);
        let w = items_width(&item.submenu);
        let h = item.submenu.len() as i32 + 2;
        let y = (d.y + self.item_sel as i32).min(screen.bottom() - h).max(1);
        let x = if d.right() - 1 + w <= screen.right() { d.right() - 1 } else { (d.x - w + 1).max(0) };
        Some(Rect::new(x, y, w.min(screen.w), h))
    }

    pub fn sub_item_rect(&self, j: usize, screen: Rect) -> Option<Rect> {
        let r = self.sub_rect(screen)?;
        Some(Rect::new(r.x + 1, r.y + 1 + j as i32, r.w - 2, 1))
    }

    fn open_sub(&mut self) -> bool {
        let Some(i) = self.open else { return false };
        let Some(it) = self.menus[i].items.get(self.item_sel).filter(|it| it.has_submenu() && it.enabled) else { return false };
        self.sub_sel = it.submenu.iter().position(|s| s.enabled).unwrap_or(0);
        self.sub_open = true;
        true
    }

    fn step_sub(&mut self, dir: i32) {
        let Some(i) = self.open else { return };
        let items = &self.menus[i].items[self.item_sel].submenu;
        let n = items.len() as i32;
        let mut j = self.sub_sel as i32;
        for _ in 0..n {
            j = (j + dir).rem_euclid(n);
            if items[j as usize].enabled {
                self.sub_sel = j as usize;
                return;
            }
        }
    }

    fn pick_sub(&mut self) -> MenuResult {
        let Some(i) = self.open else { return MenuResult::Consumed };
        let Some(it) = self.menus[i].items[self.item_sel].submenu.get(self.sub_sel) else { return MenuResult::Consumed };
        if !it.enabled || it.is_separator() {
            return MenuResult::Consumed;
        }
        let cmd = it.command.clone();
        self.close();
        MenuResult::Command(cmd)
    }

    fn open_menu(&mut self, i: usize) {
        self.sub_open = false;
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
                self.sub_open = false;
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
        // (A submenu's item: it opens.)
        if it.has_submenu() {
            self.open_sub();
            return MenuResult::Consumed;
        }
        let cmd = it.command.clone();
        self.close();
        MenuResult::Command(cmd)
    }

    pub fn handle_key(&mut self, ev: &KeyEvent) -> MenuResult {
        // Alt+Space opens the ≡ system menu (the first one without a hotkey).
        if ev.mods.alt && !ev.mods.ctrl && ev.key == Key::Char(' ') {
            if let Some(i) = self.menus.iter().position(|m| hotkey(&m.title).is_none()) {
                self.open_menu(i);
                return MenuResult::Consumed;
            }
        }
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
        // An open submenu has the keys: ↑ ↓, Enter or a letter picks,
        // ← or Esc back to its menu.
        if self.sub_open && self.open.is_some() {
            let i = self.open.unwrap();
            match ev.key {
                Key::Up => self.step_sub(-1),
                Key::Down => self.step_sub(1),
                Key::Left | Key::Esc => self.sub_open = false,
                Key::Enter => return self.pick_sub(),
                Key::Char(c) => {
                    let c = c.to_ascii_lowercase();
                    if let Some(j) = self.menus[i].items[self.item_sel].submenu.iter().position(|it| it.enabled && it.hotkey() == Some(c)) {
                        self.sub_sel = j;
                        return self.pick_sub();
                    }
                }
                _ => {}
            }
            return MenuResult::Consumed;
        }
        // (→ on a submenu's item opens it.)
        if ev.key == Key::Right && self.open_sub() {
            return MenuResult::Consumed;
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
        // The open submenu: hovering lights its items, a click or release picks.
        let sub_n = self.open.map_or(0, |i| self.menus[i].items.get(self.item_sel).map_or(0, |it| it.submenu.len()));
        let on_sub = (0..sub_n).find(|&j| self.sub_item_rect(j, screen).is_some_and(|r| r.contains(ev.pos)));
        let in_sub = self.sub_rect(screen).is_some_and(|r| r.contains(ev.pos));
        if in_sub {
            if let Some(j) = on_sub {
                let enabled = self.open.is_some_and(|i| self.menus[i].items[self.item_sel].submenu[j].enabled);
                if enabled {
                    self.sub_sel = j;
                }
                if matches!(ev.kind, MouseKind::Up(Button::Left)) || (matches!(ev.kind, MouseKind::Down(Button::Left)) && !self.tracking) {
                    self.tracking = false;
                    return self.pick_sub();
                }
            }
            return MenuResult::Consumed;
        }
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
                    self.sub_open = false;
                    self.open_sub();
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
                    if self.menus[self.open.unwrap()].items[j].enabled && (self.item_sel != j || !self.sub_open) {
                        self.item_sel = j;
                        // (Resting on a submenu's item opens it, as VS Code's do.)
                        self.sub_open = false;
                        self.open_sub();
                    }
                }
                MenuResult::Consumed
            }
            MouseKind::Up(Button::Left) if self.open.is_some() => {
                let was_tracking = std::mem::take(&mut self.tracking);
                if let Some(j) = on_item {
                    self.item_sel = j;
                    // (Let go on a submenu's item: it stays open to pick from.)
                    if self.menus[self.open.unwrap()].items[j].has_submenu() {
                        self.open_sub();
                        return MenuResult::Consumed;
                    }
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
            // (QBasic: the letters light up only once Alt or F10 is pressed.)
            let hot = if self.hotkeys_when_active && !self.is_open() { st } else { hot };
            if r.w == 0 {
                continue;
            }
            c.fill(*r, " ", st);
            c.print_hot(r.x + 1, 0, &m.title, st, hot);
        }
    }

    /// Draw the open drop-down (call after everything else so it's on top),
    /// and its open submenu beside it.
    pub fn draw_dropdown(&self, c: &mut Canvas, theme: &Theme, screen: Rect) {
        let Some(i) = self.open else { return };
        let d = self.dropdown_rect(i, screen);
        let sel = if self.sub_open { None } else { Some(self.item_sel) };
        draw_items(c, theme, d, &self.menus[i].items, sel, Some(self.item_sel));
        if let Some(r) = self.sub_rect(screen) {
            draw_items(c, theme, r, &self.menus[i].items[self.item_sel].submenu, Some(self.sub_sel), None);
        }
    }
}

/// A drop-down's width for its items (labels, keys, a submenu's ►).
fn items_width(items: &[MenuItem]) -> i32 {
    let label_w = items.iter().map(|it| str_width(&it.plain_label())).max().unwrap_or(0) as i32;
    let key_w = items.iter().map(|it| if it.has_submenu() { 1 } else { str_width(&it.shortcut) }).max().unwrap_or(0) as i32;
    label_w + if key_w > 0 { key_w + 3 } else { 0 } + 6
}

/// A box of items: `sel` lit; `parent` (the item whose submenu is open)
/// lit dimmer.
fn draw_items(c: &mut Canvas, theme: &Theme, d: Rect, items: &[MenuItem], sel: Option<usize>, parent: Option<usize>) {
    let s = &theme.scheme;
    let fc = theme.style.menu_frame;
    c.fill(d, " ", s.menu);
    crate::frame::draw_frame(c, d, fc, s.menu);
    if theme.style.shadows {
        c.shadow(d, s.shadow);
    }
    for (j, it) in items.iter().enumerate() {
        let r = Rect::new(d.x + 1, d.y + 1 + j as i32, d.w - 2, 1);
        if it.is_separator() {
            c.print(d.x, r.y, "├", s.menu);
            c.hline(d.x + 1, r.y, d.w - 2, fc.h, s.menu);
            c.print(d.right() - 1, r.y, "┤", s.menu);
            continue;
        }
        let lit = sel == Some(j) || (sel.is_none() && parent == Some(j));
        let (st, hot) = match (lit, it.enabled) {
            (true, _) => (s.menu_selected, s.menu_selected_hotkey),
            (false, true) => (s.menu, s.menu_hotkey),
            (false, false) => (s.menu_disabled, s.menu_disabled),
        };
        c.fill(r, " ", st);
        if it.checked == Some(true) {
            c.print(r.x, r.y, "√", st);
        }
        c.print_hot(r.x + 2, r.y, &it.label, st, if it.enabled { hot } else { st });
        if it.has_submenu() {
            c.print(r.right() - 2, r.y, "►", st);
        } else if !it.shortcut.is_empty() {
            let kw = str_width(&it.shortcut) as i32;
            c.print(r.right() - kw - 1, r.y, &it.shortcut, st);
        }
    }
}
