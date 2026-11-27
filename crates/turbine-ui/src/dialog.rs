//! Modal dialog boxes built from simple controls.

use crate::event::{Button, Key, KeyEvent, KeyKind, MouseEvent, MouseKind};
use crate::frame::{draw_frame, draw_title};
use crate::geom::{Point, Rect};
use crate::surface::{Canvas, CursorShape};
use crate::theme::Theme;
use turbine_text::unicode::{grapheme_boundaries, str_width};

#[derive(Debug, Clone)]
pub enum Control {
    Label { text: String },
    Input { id: String, label: String, value: String, cursor: usize },
    Button { id: String, label: String, default: bool },
    List { id: String, items: Vec<String>, selected: usize, scroll: usize },
}

impl Control {
    pub fn id(&self) -> Option<&str> {
        match self {
            Control::Input { id, .. } | Control::Button { id, .. } | Control::List { id, .. } => Some(id),
            Control::Label { .. } => None,
        }
    }
    fn focusable(&self) -> bool {
        !matches!(self, Control::Label { .. })
    }
    pub fn role(&self) -> &'static str {
        match self {
            Control::Label { .. } => "label",
            Control::Input { .. } => "input",
            Control::Button { .. } => "button",
            Control::List { .. } => "list",
        }
    }
}

/// A control placed at a position relative to the dialog's inside.
#[derive(Debug, Clone)]
pub struct Placed {
    pub control: Control,
    pub rect: Rect,
}

#[derive(Debug)]
pub enum DialogResult {
    None,
    /// A button was pressed (or Enter on the default button / a list).
    Button(String),
    Cancel,
}

#[derive(Debug)]
pub struct Dialog {
    /// What the dialog is for (the workbench uses it to act on the result).
    pub kind: String,
    pub title: String,
    pub w: i32,
    pub h: i32,
    pub controls: Vec<Placed>,
    pub focus: usize,
    /// Absolute rect from the last layout.
    pub rect: Rect,
}

impl Dialog {
    pub fn new(kind: &str, title: &str, w: i32, h: i32) -> Self {
        Dialog { kind: kind.into(), title: title.into(), w, h, controls: Vec::new(), focus: 0, rect: Rect::default() }
    }

    pub fn label(mut self, x: i32, y: i32, text: &str) -> Self {
        let w = str_width(text) as i32;
        self.controls.push(Placed { control: Control::Label { text: text.into() }, rect: Rect::new(x, y, w, 1) });
        self
    }

    pub fn input(mut self, id: &str, x: i32, y: i32, w: i32, label: &str, value: &str) -> Self {
        let c = Control::Input { id: id.into(), label: label.into(), value: value.into(), cursor: value.len() };
        self.controls.push(Placed { control: c, rect: Rect::new(x, y, w, 1) });
        self.fix_focus();
        self
    }

    pub fn button(mut self, id: &str, x: i32, y: i32, label: &str, default: bool) -> Self {
        let w = str_width(&label.replace('~', "")) as i32 + 4;
        let c = Control::Button { id: id.into(), label: label.into(), default };
        self.controls.push(Placed { control: c, rect: Rect::new(x, y, w, 1) });
        self.fix_focus();
        self
    }

    pub fn list(mut self, id: &str, r: Rect, items: Vec<String>, selected: usize) -> Self {
        let c = Control::List { id: id.into(), items, selected, scroll: 0 };
        self.controls.push(Placed { control: c, rect: r });
        self.fix_focus();
        self
    }

    fn fix_focus(&mut self) {
        if !self.controls[self.focus].control.focusable() {
            self.focus = self.controls.iter().position(|c| c.control.focusable()).unwrap_or(0);
        }
    }

    pub fn value(&self, id: &str) -> Option<String> {
        self.controls.iter().find_map(|p| match &p.control {
            Control::Input { id: i, value, .. } if i == id => Some(value.clone()),
            Control::List { id: i, items, selected, .. } if i == id => items.get(*selected).cloned(),
            _ => None,
        })
    }

    pub fn selected_index(&self, id: &str) -> Option<usize> {
        self.controls.iter().find_map(|p| match &p.control {
            Control::List { id: i, selected, .. } if i == id => Some(*selected),
            _ => None,
        })
    }

    /// Absolute rect of a control.
    pub fn control_rect(&self, i: usize) -> Rect {
        let r = self.controls[i].rect;
        Rect::new(self.rect.x + 2 + r.x, self.rect.y + 1 + r.y, r.w, r.h)
    }

    pub fn layout(&mut self, screen: Rect) {
        self.rect = screen.centered(self.w, self.h);
    }

    fn step_focus(&mut self, dir: i32) {
        let n = self.controls.len() as i32;
        let mut i = self.focus as i32;
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if self.controls[i as usize].control.focusable() {
                self.focus = i as usize;
                return;
            }
        }
    }

    fn default_button(&self) -> Option<String> {
        self.controls.iter().find_map(|p| match &p.control {
            Control::Button { id, default: true, .. } => Some(id.clone()),
            _ => None,
        })
    }

    pub fn handle_key(&mut self, ev: &KeyEvent) -> DialogResult {
        if ev.kind == KeyKind::Release {
            return DialogResult::None;
        }
        match ev.key {
            Key::Esc => return DialogResult::Cancel,
            Key::Tab => {
                self.step_focus(1);
                return DialogResult::None;
            }
            Key::BackTab => {
                self.step_focus(-1);
                return DialogResult::None;
            }
            _ => {}
        }
        // Alt+hotkey presses a button.
        if ev.mods.alt {
            if let Key::Char(c) = ev.key {
                for p in &self.controls {
                    if let Control::Button { id, label, .. } = &p.control {
                        if crate::menu::hotkey(label) == Some(c.to_ascii_lowercase()) {
                            return DialogResult::Button(id.clone());
                        }
                    }
                }
            }
        }
        let default = self.default_button();
        let ctl = &mut self.controls[self.focus].control;
        match ctl {
            Control::Button { id, .. } => match ev.key {
                Key::Enter | Key::Char(' ') => return DialogResult::Button(id.clone()),
                Key::Right | Key::Down => self.step_focus(1),
                Key::Left | Key::Up => self.step_focus(-1),
                _ => {}
            },
            Control::Input { value, cursor, .. } => {
                let b = grapheme_boundaries(value);
                match ev.key {
                    Key::Enter => return default.map(DialogResult::Button).unwrap_or(DialogResult::None),
                    Key::Char(c) if !ev.mods.ctrl && !ev.mods.alt => {
                        value.insert(*cursor, c);
                        *cursor += c.len_utf8();
                    }
                    Key::Backspace => {
                        if let Some(&p) = b.iter().rev().find(|&&x| x < *cursor) {
                            value.replace_range(p..*cursor, "");
                            *cursor = p;
                        }
                    }
                    Key::Delete => {
                        if let Some(&n) = b.iter().find(|&&x| x > *cursor) {
                            value.replace_range(*cursor..n, "");
                        }
                    }
                    Key::Left => *cursor = b.iter().rev().find(|&&x| x < *cursor).copied().unwrap_or(0),
                    Key::Right => *cursor = b.iter().find(|&&x| x > *cursor).copied().unwrap_or(value.len()),
                    Key::Home => *cursor = 0,
                    Key::End => *cursor = value.len(),
                    _ => {}
                }
            }
            Control::List { id, items, selected, .. } => match ev.key {
                Key::Up => *selected = selected.saturating_sub(1),
                Key::Down => *selected = (*selected + 1).min(items.len().saturating_sub(1)),
                Key::Home => *selected = 0,
                Key::End => *selected = items.len().saturating_sub(1),
                Key::Enter => return default.map(DialogResult::Button).unwrap_or(DialogResult::Button(id.clone())),
                _ => {}
            },
            Control::Label { .. } => {}
        }
        DialogResult::None
    }

    pub fn handle_paste(&mut self, text: &str) {
        if let Control::Input { value, cursor, .. } = &mut self.controls[self.focus].control {
            let t: String = text.chars().filter(|c| !c.is_control()).collect();
            value.insert_str(*cursor, &t);
            *cursor += t.len();
        }
    }

    pub fn handle_mouse(&mut self, ev: &MouseEvent) -> DialogResult {
        let MouseKind::Down(Button::Left) = ev.kind else {
            if let MouseKind::ScrollUp | MouseKind::ScrollDown = ev.kind {
                for i in 0..self.controls.len() {
                    if self.control_rect(i).contains(ev.pos) {
                        if let Control::List { items, selected, .. } = &mut self.controls[i].control {
                            *selected = if ev.kind == MouseKind::ScrollUp {
                                selected.saturating_sub(1)
                            } else {
                                (*selected + 1).min(items.len().saturating_sub(1))
                            };
                        }
                    }
                }
            }
            return DialogResult::None;
        };
        for i in 0..self.controls.len() {
            let r = self.control_rect(i);
            if !r.contains(ev.pos) {
                continue;
            }
            if self.controls[i].control.focusable() {
                self.focus = i;
            }
            match &mut self.controls[i].control {
                Control::Button { id, .. } => return DialogResult::Button(id.clone()),
                Control::List { items, selected, scroll, .. } => {
                    let row = (ev.pos.y - r.y) as usize + *scroll;
                    if row < items.len() {
                        *selected = row;
                    }
                }
                Control::Input { value, cursor, .. } => {
                    let col = (ev.pos.x - r.x) as usize;
                    let mut w = 0;
                    *cursor = value.len();
                    for (bi, g) in unicode_segmentation::UnicodeSegmentation::grapheme_indices(value.as_str(), true) {
                        if w >= col {
                            *cursor = bi;
                            break;
                        }
                        w += turbine_text::unicode::grapheme_width(g);
                    }
                }
                Control::Label { .. } => {}
            }
        }
        DialogResult::None
    }

    pub fn draw(&mut self, c: &mut Canvas, theme: &Theme) {
        let s = &theme.scheme;
        let r = self.rect;
        c.fill(r, " ", s.dialog);
        draw_frame(c, r, theme.style.dialog_frame, s.dialog_frame);
        // (QBasic: a line across the box above the buttons.)
        if theme.style.dialog_rule {
            if let Some(by) = self.controls.iter().filter(|p| matches!(p.control, Control::Button { .. })).map(|p| p.rect.y).max() {
                let y = r.y + 1 + by - 1;
                if y > r.y + 1 && !self.controls.iter().any(|p| !matches!(p.control, Control::Button { .. }) && p.rect.y + p.rect.h > by - 1 && p.rect.y <= by - 1) {
                    let fc = theme.style.dialog_frame;
                    c.print(r.x, y, if fc.v == "║" { "╟" } else { "├" }, s.dialog_frame);
                    c.hline(r.x + 1, y, r.w - 2, "─", s.dialog_frame);
                    c.print(r.right() - 1, y, if fc.v == "║" { "╢" } else { "┤" }, s.dialog_frame);
                }
            }
        }
        draw_title(c, r, &self.title, s.dialog_frame);
        if theme.style.shadows {
            c.shadow(r, s.shadow);
        }
        for i in 0..self.controls.len() {
            let cr = self.control_rect(i);
            let focused = i == self.focus;
            match &mut self.controls[i].control {
                Control::Label { text } => {
                    c.print(cr.x, cr.y, text, s.dialog);
                }
                Control::Input { label, value, cursor, .. } => {
                    if !label.is_empty() {
                        c.print_hot(cr.x, cr.y - 1, label, s.dialog, s.menu_hotkey);
                    }
                    c.fill(cr, " ", s.input);
                    let before = str_width(&value[..*cursor]) as i32;
                    let skip = (before - cr.w + 2).max(0);
                    let mut sub = c.sub(cr);
                    sub.print(cr.x - skip + 1, cr.y, value, s.input);
                    if focused {
                        sub.set_cursor(Point::new(cr.x + 1 + before - skip, cr.y), CursorShape::Underline);
                    }
                }
                Control::Button { label, default, .. } => {
                    let st = if focused { s.button_focused } else { s.button };
                    let (l, rr) = theme.style.button;
                    let (l, rr) = if *default && l.trim().is_empty() { ("◄ ", " ►") } else { (l, rr) };
                    let text = format!("{l}{label}{rr}");
                    let w = str_width(&text.replace('~', "")) as i32;
                    if theme.style.dos_status {
                        // QBasic: "< OK >", the focused button's brackets bright.
                        let br = if focused { s.button_focused } else { s.button };
                        let x = c.print(cr.x, cr.y, l, br);
                        let x = c.print_hot(x, cr.y, label, s.button, s.button_hotkey);
                        c.print(x, cr.y, rr, br);
                    } else {
                        c.print_hot(cr.x, cr.y, &text, st, s.button_hotkey);
                    }
                    if theme.style.shadows && theme.style.button.0.trim().is_empty() {
                        c.print(cr.x + w, cr.y, "▄", s.dialog.fg(crate::theme::vga::BLACK));
                        c.hline(cr.x + 1, cr.y + 1, w, "▀", s.dialog.fg(crate::theme::vga::BLACK));
                    }
                }
                Control::List { items, selected, scroll, .. } => {
                    let h = cr.h.max(1) as usize;
                    if *selected < *scroll {
                        *scroll = *selected;
                    } else if *selected >= *scroll + h {
                        *scroll = *selected + 1 - h;
                    }
                    c.fill(cr, " ", s.input);
                    let mut sub = c.sub(cr);
                    for (row, it) in items.iter().enumerate().skip(*scroll).take(h) {
                        let y = cr.y + (row - *scroll) as i32;
                        let st = if row == *selected {
                            if focused { s.menu_selected } else { s.editor_selection }
                        } else {
                            s.input
                        };
                        sub.fill(Rect::new(cr.x, y, cr.w, 1), " ", st);
                        sub.print(cr.x + 1, y, it, st);
                    }
                }
            }
        }
    }
}
