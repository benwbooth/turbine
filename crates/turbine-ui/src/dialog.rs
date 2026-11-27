//! Dialog boxes: movable, resizable, with controls that stretch and move
//! with the box (anchors, as in WinForms).
//!
//! A dialog is built at its natural size with controls placed inside it;
//! when it grows or shrinks (dragged by an edge or corner, or the screen
//! changes), each control keeps its distance to the edges it's anchored to.
//! Controls get sensible anchors on their own (lists and group boxes
//! stretch both ways, fields stretch across, buttons on the right or bottom
//! follow those edges); `.anchor()` sets them explicitly.

use crate::automation::Node;
use crate::event::{Button, Key, KeyEvent, KeyKind, MouseEvent, MouseKind};
use crate::frame::{draw_frame, draw_title, draw_vscroll};
use crate::geom::{Point, Rect};
use crate::surface::{Canvas, CursorShape};
use crate::theme::{Style, Theme, vga};
use std::time::{Duration, Instant};
use turbine_text::unicode::{grapheme_boundaries, str_width};

#[derive(Debug, Clone)]
pub enum Control {
    Label { text: String },
    Input { id: String, label: String, value: String, cursor: usize },
    Button { id: String, label: String, default: bool },
    /// A list; `header` is a column title row; `activates`: Enter and
    /// double-click report Activated (else Enter presses the default button).
    List { id: String, items: Vec<String>, selected: usize, scroll: usize, header: Option<String>, activates: bool },
    Check { id: String, label: String, checked: bool },
    /// Radio buttons with the same `group` are exclusive.
    Radio { id: String, label: String, group: String, checked: bool },
    /// A drop-down choice.
    Combo { id: String, label: String, items: Vec<String>, selected: usize, open: bool },
    /// A frame around other controls, its title in the top edge.
    Group { title: String },
}

impl Control {
    pub fn id(&self) -> Option<&str> {
        match self {
            Control::Input { id, .. }
            | Control::Button { id, .. }
            | Control::List { id, .. }
            | Control::Check { id, .. }
            | Control::Radio { id, .. }
            | Control::Combo { id, .. } => Some(id),
            Control::Label { .. } | Control::Group { .. } => None,
        }
    }
    fn focusable(&self) -> bool {
        !matches!(self, Control::Label { .. } | Control::Group { .. })
    }
    pub fn role(&self) -> &'static str {
        match self {
            Control::Label { .. } => "label",
            Control::Input { .. } => "input",
            Control::Button { .. } => "button",
            Control::List { .. } => "list",
            Control::Check { .. } => "checkbox",
            Control::Radio { .. } => "radio",
            Control::Combo { .. } => "combobox",
            Control::Group { .. } => "group",
        }
    }
    fn label_text(&self) -> String {
        match self {
            Control::Label { text } => text.clone(),
            Control::Input { label, .. }
            | Control::Check { label, .. }
            | Control::Radio { label, .. }
            | Control::Combo { label, .. }
            | Control::Button { label, .. } => label.replace('~', ""),
            Control::List { items, .. } => format!("{} items", items.len()),
            Control::Group { title } => title.replace('~', ""),
        }
    }
}

/// Which edges a control keeps its distance to as the dialog resizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchors {
    pub left: bool,
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
}

impl Anchors {
    pub const TOP_LEFT: Anchors = Anchors { left: true, top: true, right: false, bottom: false };
    pub const ALL: Anchors = Anchors { left: true, top: true, right: true, bottom: true };
    pub const TOP_WIDE: Anchors = Anchors { left: true, top: true, right: true, bottom: false };
    pub const TOP_RIGHT: Anchors = Anchors { left: false, top: true, right: true, bottom: false };
    pub const BOTTOM_LEFT: Anchors = Anchors { left: true, top: false, right: false, bottom: true };
    pub const BOTTOM_RIGHT: Anchors = Anchors { left: false, top: false, right: true, bottom: true };
    pub const BOTTOM_WIDE: Anchors = Anchors { left: true, top: false, right: true, bottom: true };
    /// Stretches down, keeps its width at the left.
    pub const LEFT_TALL: Anchors = Anchors { left: true, top: true, right: false, bottom: true };
}

/// A control placed at a position relative to the dialog's inside (at the
/// dialog's natural size).
#[derive(Debug, Clone)]
pub struct Placed {
    pub control: Control,
    pub rect: Rect,
    pub anchors: Option<Anchors>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DialogResult {
    None,
    /// A button was pressed (or Enter on the default button).
    Button(String),
    Cancel,
    /// A control's value changed (typing, a selection, a check box).
    /// "<id>:backspace" when Backspace is pressed in an empty field.
    Changed(String),
    /// Enter or a double-click on an activating list's item.
    Activated(String),
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    Move { grab: Point },
    Resize { start: Point, size: (i32, i32), right: bool, bottom: bool },
    /// Dragging list `i`'s scroll bar thumb.
    Thumb { i: usize },
}

#[derive(Debug)]
pub struct Dialog {
    /// What the dialog is for (the workbench uses it to act on the result).
    pub kind: String,
    pub title: String,
    /// The natural size the controls were placed for.
    pub w: i32,
    pub h: i32,
    pub controls: Vec<Placed>,
    pub focus: usize,
    /// Where it is on screen (from the last layout).
    pub rect: Rect,
    /// Its size now (it starts at the natural size), and the smallest.
    size: (i32, i32),
    pub min: (i32, i32),
    /// Where the person moved it (else centred).
    pos: Option<Point>,
    drag: Option<Drag>,
    last_click: Option<(Point, Instant)>,
    /// The screen from the last layout (drop-downs stay inside it).
    screen: Rect,
    /// Controls drawn in boxes (QBasic, VB-DOS) at the last draw: where
    /// their insides are. (Clicks and list rows use them.)
    boxed: Vec<Option<Rect>>,
}

impl Dialog {
    pub fn new(kind: &str, title: &str, w: i32, h: i32) -> Self {
        Dialog {
            kind: kind.into(),
            title: title.into(),
            w,
            h,
            controls: Vec::new(),
            focus: 0,
            rect: Rect::default(),
            size: (w, h),
            min: ((w * 2 / 3).max(24).min(w), (h * 2 / 3).max(6).min(h)),
            pos: None,
            drag: None,
            last_click: None,
            screen: Rect::default(),
            boxed: Vec::new(),
        }
    }

    fn push(mut self, control: Control, rect: Rect) -> Self {
        self.controls.push(Placed { control, rect, anchors: None });
        self.fix_focus();
        self
    }

    pub fn label(self, x: i32, y: i32, text: &str) -> Self {
        let w = str_width(text) as i32;
        self.push(Control::Label { text: text.into() }, Rect::new(x, y, w, 1))
    }

    pub fn input(self, id: &str, x: i32, y: i32, w: i32, label: &str, value: &str) -> Self {
        let c = Control::Input { id: id.into(), label: label.into(), value: value.into(), cursor: value.len() };
        self.push(c, Rect::new(x, y, w, 1))
    }

    pub fn button(self, id: &str, x: i32, y: i32, label: &str, default: bool) -> Self {
        let w = str_width(&label.replace('~', "")) as i32 + 4;
        self.push(Control::Button { id: id.into(), label: label.into(), default }, Rect::new(x, y, w, 1))
    }

    pub fn list(self, id: &str, r: Rect, items: Vec<String>, selected: usize) -> Self {
        let c = Control::List { id: id.into(), items, selected, scroll: 0, header: None, activates: false };
        self.push(c, r)
    }

    /// A list whose Enter / double-click reports Activated, with an optional
    /// column header row.
    pub fn activating_list(self, id: &str, r: Rect, header: Option<&str>, items: Vec<String>, selected: usize) -> Self {
        let c = Control::List { id: id.into(), items, selected, scroll: 0, header: header.map(str::to_string), activates: true };
        self.push(c, r)
    }

    pub fn check(self, id: &str, x: i32, y: i32, label: &str, checked: bool) -> Self {
        let w = str_width(&label.replace('~', "")) as i32 + 4;
        self.push(Control::Check { id: id.into(), label: label.into(), checked }, Rect::new(x, y, w, 1))
    }

    pub fn radio(self, id: &str, group: &str, x: i32, y: i32, label: &str, checked: bool) -> Self {
        let w = str_width(&label.replace('~', "")) as i32 + 4;
        let c = Control::Radio { id: id.into(), label: label.into(), group: group.into(), checked };
        self.push(c, Rect::new(x, y, w, 1))
    }

    /// A drop-down: `label` goes on the row above, like an input's.
    pub fn combo(self, id: &str, x: i32, y: i32, w: i32, label: &str, items: Vec<String>, selected: usize) -> Self {
        let c = Control::Combo { id: id.into(), label: label.into(), items, selected, open: false };
        self.push(c, Rect::new(x, y, w, 1))
    }

    /// A group box: the frame is `r` (its title in the top edge).
    pub fn group(self, title: &str, r: Rect) -> Self {
        self.push(Control::Group { title: title.into() }, r)
    }

    /// Anchors for the control added last.
    pub fn anchor(mut self, a: Anchors) -> Self {
        if let Some(p) = self.controls.last_mut() {
            p.anchors = Some(a);
        }
        self
    }

    /// Start bigger (or smaller) than the natural size.
    pub fn sized(mut self, w: i32, h: i32) -> Self {
        self.size = (w.max(self.min.0), h.max(self.min.1));
        self
    }

    pub fn min_size(mut self, w: i32, h: i32) -> Self {
        self.min = (w, h);
        self
    }

    fn fix_focus(&mut self) {
        if !self.controls[self.focus].control.focusable() {
            self.focus = self.controls.iter().position(|c| c.control.focusable()).unwrap_or(0);
        }
    }

    /// Focus the control with this id.
    pub fn focus_id(&mut self, id: &str) {
        if let Some(i) = self.controls.iter().position(|p| p.control.id() == Some(id)) {
            self.focus = i;
        }
    }

    pub fn focused_id(&self) -> Option<&str> {
        self.controls.get(self.focus).and_then(|p| p.control.id())
    }

    pub fn value(&self, id: &str) -> Option<String> {
        self.controls.iter().find_map(|p| match &p.control {
            Control::Input { id: i, value, .. } if i == id => Some(value.clone()),
            Control::List { id: i, items, selected, .. } if i == id => items.get(*selected).cloned(),
            Control::Combo { id: i, items, selected, .. } if i == id => items.get(*selected).cloned(),
            _ => None,
        })
    }

    pub fn selected_index(&self, id: &str) -> Option<usize> {
        self.controls.iter().find_map(|p| match &p.control {
            Control::List { id: i, selected, .. } | Control::Combo { id: i, selected, .. } if i == id => Some(*selected),
            _ => None,
        })
    }

    pub fn checked(&self, id: &str) -> bool {
        self.controls.iter().any(|p| match &p.control {
            Control::Check { id: i, checked, .. } | Control::Radio { id: i, checked, .. } => i == id && *checked,
            _ => false,
        })
    }

    /// Replace a list's items (the selection kept in range).
    pub fn set_items(&mut self, id: &str, new: Vec<String>, sel: usize) {
        for p in &mut self.controls {
            if let Control::List { id: i, items, selected, scroll, .. } = &mut p.control {
                if i == id {
                    *items = new;
                    *selected = sel.min(items.len().saturating_sub(1));
                    *scroll = (*scroll).min(*selected);
                    return;
                }
            }
        }
    }

    pub fn set_value(&mut self, id: &str, v: &str) {
        for p in &mut self.controls {
            if let Control::Input { id: i, value, cursor, .. } = &mut p.control {
                if i == id {
                    *value = v.to_string();
                    *cursor = value.len();
                }
            }
        }
    }

    /// Change the text of the label at index `i` (labels have no ids).
    pub fn set_label(&mut self, i: usize, v: &str) {
        if let Some(Placed { control: Control::Label { text }, rect, .. }) = self.controls.get_mut(i) {
            *text = v.to_string();
            rect.w = rect.w.max(str_width(v) as i32);
        }
    }

    // ---- layout ------------------------------------------------------------------

    fn default_anchors(&self, p: &Placed) -> Anchors {
        let (w0, h0) = (self.w - 4, self.h - 2);
        let low = p.rect.y >= h0 * 3 / 5;
        let right = p.rect.x >= w0 / 2;
        match p.control {
            Control::List { .. } | Control::Group { .. } => Anchors::ALL,
            Control::Input { .. } | Control::Combo { .. } => Anchors { left: true, right: true, top: !low, bottom: low },
            Control::Button { .. } => Anchors { left: !right, right, top: !low, bottom: low },
            _ => Anchors { left: true, right: false, top: !low, bottom: low },
        }
    }

    /// Place the dialog on the screen: its current size (never larger than
    /// the screen, never smaller than its minimum), where it was moved to or
    /// centred.
    pub fn layout(&mut self, screen: Rect) {
        self.screen = screen;
        let w = self.size.0.min(screen.w).max(self.min.0.min(screen.w));
        let h = self.size.1.min(screen.h).max(self.min.1.min(screen.h));
        self.rect = match self.pos {
            Some(p) => Rect::new(
                p.x.clamp(screen.x, (screen.right() - w).max(screen.x)),
                p.y.clamp(screen.y, (screen.bottom() - h).max(screen.y)),
                w,
                h,
            ),
            None => screen.centered(w, h),
        };
    }

    /// A control's place on screen now.
    pub fn control_rect(&self, i: usize) -> Rect {
        let p = &self.controls[i];
        let a = p.anchors.unwrap_or_else(|| self.default_anchors(p));
        // (Before the first layout the dialog is at its natural size.)
        let (dx, dy) = if self.rect.w == 0 { (0, 0) } else { (self.rect.w - self.w, self.rect.h - self.h) };
        let r = p.rect;
        let (x, w) = match (a.left, a.right) {
            (true, true) => (r.x, (r.w + dx).max(1)),
            (false, true) => (r.x + dx, r.w),
            (true, false) => (r.x, r.w),
            (false, false) => (r.x + dx / 2, r.w),
        };
        let (y, h) = match (a.top, a.bottom) {
            (true, true) => (r.y, (r.h + dy).max(1)),
            (false, true) => (r.y + dy, r.h),
            (true, false) => (r.y, r.h),
            (false, false) => (r.y + dy / 2, r.h),
        };
        Rect::new(self.rect.x + 2 + x, self.rect.y + 1 + y, w, h)
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

    /// Where a control's contents go: inside its box when it has one.
    fn inner(&self, i: usize) -> Rect {
        self.boxed.get(i).copied().flatten().unwrap_or_else(|| self.control_rect(i))
    }

    /// The box around a field, drop-down or list (QBasic, VB-DOS), or None
    /// where there's no room for one. Inputs and drop-downs: the label goes
    /// beside the field and the box takes the label's row and the row below.
    fn box_of(&self, i: usize) -> Option<(Rect, Rect)> {
        let cr = self.control_rect(i);
        match &self.controls[i].control {
            Control::Input { label, .. } | Control::Combo { label, .. } => {
                let lw = str_width(&label.replace('~', "")) as i32;
                let x0 = if lw > 0 { cr.x + lw + 1 } else { cr.x };
                let outer = Rect::new(x0, cr.y - 1, cr.right() - x0, 3);
                let room = outer.w >= 8
                    && self.row_free(i, cr.y + 1, outer.x, outer.right())
                    && (lw > 0 || self.row_free(i, cr.y - 1, outer.x, outer.right()));
                room.then(|| (outer, Rect::new(x0 + 1, cr.y, outer.w - 2, 1)))
            }
            Control::List { .. } => {
                // (A list filling a group box already has its box.)
                let in_group = self.controls.iter().enumerate().any(|(j, p)| {
                    matches!(p.control, Control::Group { .. }) && {
                        let g = self.control_rect(j);
                        g.inset(1) == cr
                    }
                });
                (!in_group && cr.w >= 4 && cr.h >= 3).then(|| (cr, cr.inset(1)))
            }
            _ => None,
        }
    }

    fn list_rows(&self, i: usize) -> usize {
        let r = self.inner(i);
        let header = matches!(&self.controls[i].control, Control::List { header: Some(_), .. });
        (r.h - header as i32).max(1) as usize
    }

    fn select_radio(&mut self, i: usize) {
        let Control::Radio { group, .. } = &self.controls[i].control else { return };
        let g = group.clone();
        for (j, p) in self.controls.iter_mut().enumerate() {
            if let Control::Radio { group, checked, .. } = &mut p.control {
                if *group == g {
                    *checked = j == i;
                }
            }
        }
    }

    // ---- input -------------------------------------------------------------------

    pub fn handle_key(&mut self, ev: &KeyEvent) -> DialogResult {
        if ev.kind == KeyKind::Release {
            return DialogResult::None;
        }
        // An open drop-down takes the keys.
        if let Some(i) = self.controls.iter().position(|p| matches!(p.control, Control::Combo { open: true, .. })) {
            if let Control::Combo { id, items, selected, open, .. } = &mut self.controls[i].control {
                match ev.key {
                    Key::Up => *selected = selected.saturating_sub(1),
                    Key::Down => *selected = (*selected + 1).min(items.len().saturating_sub(1)),
                    Key::Enter | Key::Char(' ') => {
                        *open = false;
                        return DialogResult::Changed(id.clone());
                    }
                    Key::Esc | Key::Tab => *open = false,
                    _ => {}
                }
            }
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
        // Alt+hotkey: a button presses; other controls take focus (a check
        // box or radio button also flips).
        if ev.mods.alt {
            if let Key::Char(c) = ev.key {
                let c = c.to_ascii_lowercase();
                let hit = self.controls.iter().position(|p| match &p.control {
                    Control::Button { label, .. }
                    | Control::Check { label, .. }
                    | Control::Radio { label, .. }
                    | Control::Input { label, .. }
                    | Control::Combo { label, .. } => crate::menu::hotkey(label) == Some(c),
                    Control::Group { title } => crate::menu::hotkey(title) == Some(c),
                    _ => false,
                });
                if let Some(i) = hit {
                    // (A group box's hotkey goes to the first control inside it.)
                    let i = if matches!(self.controls[i].control, Control::Group { .. }) {
                        let g = self.controls[i].rect;
                        (0..self.controls.len()).find(|&j| j != i && self.controls[j].control.focusable() && g.contains(Point::new(self.controls[j].rect.x, self.controls[j].rect.y))).unwrap_or(i)
                    } else {
                        i
                    };
                    if self.controls[i].control.focusable() {
                        self.focus = i;
                    }
                    match &mut self.controls[i].control {
                        Control::Button { id, .. } => return DialogResult::Button(id.clone()),
                        Control::Check { id, checked, .. } => {
                            *checked = !*checked;
                            return DialogResult::Changed(id.clone());
                        }
                        Control::Radio { id, .. } => {
                            let id = id.clone();
                            self.select_radio(i);
                            return DialogResult::Changed(id);
                        }
                        _ => return DialogResult::None,
                    }
                }
            }
        }
        let default = self.default_button();
        let rows = if matches!(self.controls[self.focus].control, Control::List { .. }) { self.list_rows(self.focus) } else { 1 };
        let focus = self.focus;
        let ctl = &mut self.controls[self.focus].control;
        match ctl {
            Control::Button { id, .. } => match ev.key {
                Key::Enter | Key::Char(' ') => return DialogResult::Button(id.clone()),
                Key::Right | Key::Down => self.step_focus(1),
                Key::Left | Key::Up => self.step_focus(-1),
                _ => {}
            },
            Control::Input { id, value, cursor, .. } => {
                let b = grapheme_boundaries(value);
                let before = value.clone();
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
                        } else {
                            // Backspace in an empty field: the dialog may act (go up a folder).
                            return DialogResult::Changed(format!("{id}:backspace"));
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
                    // Down from a field: on to the next control (a list below it).
                    Key::Down => {
                        self.step_focus(1);
                        return DialogResult::None;
                    }
                    _ => {}
                }
                if *value != before {
                    return DialogResult::Changed(id.clone());
                }
            }
            Control::List { id, items, selected, activates, .. } => {
                let n = items.len();
                let old = *selected;
                match ev.key {
                    Key::Up => *selected = selected.saturating_sub(1),
                    Key::Down => *selected = (*selected + 1).min(n.saturating_sub(1)),
                    Key::PageUp => *selected = selected.saturating_sub(rows),
                    Key::PageDown => *selected = (*selected + rows).min(n.saturating_sub(1)),
                    Key::Home => *selected = 0,
                    Key::End => *selected = n.saturating_sub(1),
                    Key::Enter if *activates => return DialogResult::Activated(id.clone()),
                    Key::Enter => return default.map(DialogResult::Button).unwrap_or(DialogResult::Button(id.clone())),
                    Key::Backspace if *activates => return DialogResult::Changed(format!("{id}:backspace")),
                    // Type-ahead: the next item starting with that letter.
                    Key::Char(c) if !ev.mods.ctrl && !ev.mods.alt && n > 0 => {
                        let c = c.to_lowercase().next().unwrap_or(c);
                        let starts = |s: &String| {
                            let t = s.trim_start_matches(|ch: char| !ch.is_alphanumeric());
                            t.chars().next().and_then(|f| f.to_lowercase().next()) == Some(c)
                        };
                        if let Some(j) = (1..=n).map(|k| (*selected + k) % n).find(|&j| starts(&items[j])) {
                            *selected = j;
                        }
                    }
                    _ => {}
                }
                if *selected != old {
                    return DialogResult::Changed(id.clone());
                }
            }
            Control::Check { id, checked, .. } => match ev.key {
                Key::Char(' ') => {
                    *checked = !*checked;
                    return DialogResult::Changed(id.clone());
                }
                Key::Enter => return default.map(DialogResult::Button).unwrap_or(DialogResult::None),
                Key::Down | Key::Right => self.step_focus(1),
                Key::Up | Key::Left => self.step_focus(-1),
                _ => {}
            },
            Control::Radio { id, .. } => {
                let id = id.clone();
                match ev.key {
                    Key::Char(' ') => {
                        self.select_radio(focus);
                        return DialogResult::Changed(id);
                    }
                    Key::Enter => return default.map(DialogResult::Button).unwrap_or(DialogResult::None),
                    Key::Down | Key::Right | Key::Up | Key::Left => {
                        self.step_focus(if matches!(ev.key, Key::Down | Key::Right) { 1 } else { -1 });
                        if matches!(self.controls[self.focus].control, Control::Radio { .. }) {
                            self.select_radio(self.focus);
                            return DialogResult::Changed(self.controls[self.focus].control.id().unwrap_or("").to_string());
                        }
                    }
                    _ => {}
                }
            }
            Control::Combo { id, items, selected, open, .. } => {
                let old = *selected;
                match ev.key {
                    Key::Up => *selected = selected.saturating_sub(1),
                    Key::Down if ev.mods.alt => *open = true,
                    Key::Down => *selected = (*selected + 1).min(items.len().saturating_sub(1)),
                    Key::Char(' ') | Key::F(4) => *open = true,
                    Key::Enter => return default.map(DialogResult::Button).unwrap_or(DialogResult::None),
                    _ => {}
                }
                if *selected != old {
                    return DialogResult::Changed(id.clone());
                }
            }
            Control::Label { .. } | Control::Group { .. } => {}
        }
        DialogResult::None
    }

    pub fn handle_paste(&mut self, text: &str) -> DialogResult {
        if let Control::Input { id, value, cursor, .. } = &mut self.controls[self.focus].control {
            let t: String = text.chars().filter(|c| !c.is_control()).collect();
            value.insert_str(*cursor, &t);
            *cursor += t.len();
            return DialogResult::Changed(id.clone());
        }
        DialogResult::None
    }

    /// The drop-down of an open combo box, and where it is.
    fn combo_popup(&self) -> Option<(usize, Rect)> {
        let i = self.controls.iter().position(|p| matches!(p.control, Control::Combo { open: true, .. }))?;
        let Control::Combo { items, .. } = &self.controls[i].control else { return None };
        let r = self.inner(i);
        // (Below the box, when it has one.)
        let r = if self.boxed.get(i).copied().flatten().is_some() { Rect::new(r.x - 1, r.y + 1, r.w + 2, 1) } else { r };
        // As wide as the longest item, kept on screen.
        let w = items.iter().map(|s| str_width(s) as i32 + 4).max().unwrap_or(0).max(r.w);
        let x = if self.screen.w > 0 { r.x.min(self.screen.right() - w).max(self.screen.x) } else { r.x };
        Some((i, Rect::new(x, r.y + 1, w, items.len().min(10) as i32 + 2)))
    }

    /// The first item an open drop-down shows (keeping the selected one in view).
    fn combo_top(&self, i: usize, pr: Rect) -> usize {
        let Control::Combo { selected, .. } = &self.controls[i].control else { return 0 };
        selected.saturating_sub((pr.h - 3).max(0) as usize)
    }

    /// Scroll list `i` so its thumb is at screen row `y` (on the track
    /// between the arrows); the selection stays in view.
    fn thumb_to(&mut self, i: usize, y: i32) {
        let cr = self.inner(i);
        let rows = self.list_rows(i);
        if let Control::List { items, selected, scroll, header, .. } = &mut self.controls[i].control {
            let y0 = cr.y + header.is_some() as i32;
            let track = (rows as i32 - 3).max(0);
            let max = items.len().saturating_sub(rows);
            let at = (y - y0 - 1).clamp(0, track);
            *scroll = if track == 0 { 0 } else { ((at as usize * max) + track as usize / 2) / track as usize }.min(max);
            *selected = (*selected).clamp(*scroll, (*scroll + rows).saturating_sub(1));
        }
    }

    pub fn handle_mouse(&mut self, ev: &MouseEvent) -> DialogResult {
        // Dragging the dialog (moving or resizing).
        if let Some(d) = self.drag {
            match ev.kind {
                MouseKind::Drag(_) => match d {
                    Drag::Move { grab } => self.pos = Some(Point::new(ev.pos.x - grab.x, ev.pos.y - grab.y)),
                    Drag::Resize { start, size, right, bottom } => {
                        let w = if right { size.0 + ev.pos.x - start.x } else { size.0 };
                        let h = if bottom { size.1 + ev.pos.y - start.y } else { size.1 };
                        self.size = (w.max(self.min.0), h.max(self.min.1));
                        self.pos = Some(Point::new(self.rect.x, self.rect.y));
                    }
                    Drag::Thumb { i } => self.thumb_to(i, ev.pos.y),
                },
                _ => self.drag = None,
            }
            return DialogResult::None;
        }
        // An open drop-down.
        if let Some((i, pr)) = self.combo_popup() {
            if let MouseKind::Down(Button::Left) = ev.kind {
                let top = self.combo_top(i, pr);
                if let Control::Combo { id, items, selected, open, .. } = &mut self.controls[i].control {
                    *open = false;
                    if pr.contains(ev.pos) && ev.pos.y > pr.y && ev.pos.y < pr.bottom() - 1 {
                        let row = (ev.pos.y - pr.y - 1) as usize + top;
                        if row < items.len() {
                            *selected = row;
                            return DialogResult::Changed(id.clone());
                        }
                    }
                }
            }
            return DialogResult::None;
        }
        if let MouseKind::ScrollUp | MouseKind::ScrollDown = ev.kind {
            for i in 0..self.controls.len() {
                if self.control_rect(i).contains(ev.pos) {
                    let rows = if matches!(self.controls[i].control, Control::List { .. }) { self.list_rows(i) } else { 1 };
                    if let Control::List { scroll, items, selected, .. } = &mut self.controls[i].control {
                        if ev.kind == MouseKind::ScrollUp {
                            *scroll = scroll.saturating_sub(3);
                        } else {
                            *scroll = (*scroll + 3).min(items.len().saturating_sub(rows));
                        }
                        // (The selection stays in view.)
                        *selected = (*selected).clamp(*scroll, (*scroll + rows).saturating_sub(1).max(*scroll));
                    }
                }
            }
            return DialogResult::None;
        }
        let MouseKind::Down(Button::Left) = ev.kind else { return DialogResult::None };
        let r = self.rect;
        // Edges: the right and bottom ones (and the corner) resize; the title row moves.
        let on_right = ev.pos.x == r.right() - 1 && ev.pos.y > r.y && ev.pos.y < r.bottom();
        let on_bottom = ev.pos.y == r.bottom() - 1 && ev.pos.x >= r.x && ev.pos.x < r.right();
        if on_right || on_bottom {
            self.drag = Some(Drag::Resize { start: ev.pos, size: (r.w, r.h), right: on_right || ev.pos.x >= r.right() - 2, bottom: on_bottom });
            return DialogResult::None;
        }
        if ev.pos.y == r.y && r.contains(ev.pos) {
            self.drag = Some(Drag::Move { grab: Point::new(ev.pos.x - r.x, ev.pos.y - r.y) });
            return DialogResult::None;
        }
        let double = self.last_click.is_some_and(|(p, t)| p == ev.pos && t.elapsed() < Duration::from_millis(450));
        self.last_click = Some((ev.pos, Instant::now()));
        for i in (0..self.controls.len()).rev() {
            let cr = self.control_rect(i);
            let hit = cr.contains(ev.pos) || self.boxed.get(i).copied().flatten().is_some_and(|b| b.y == ev.pos.y && b.x - 1 <= ev.pos.x && ev.pos.x <= b.right());
            if !hit || matches!(self.controls[i].control, Control::Group { .. } | Control::Label { .. }) {
                continue;
            }
            if self.controls[i].control.focusable() {
                self.focus = i;
            }
            let rows = if matches!(self.controls[i].control, Control::List { .. }) { self.list_rows(i) } else { 1 };
            let ir = self.inner(i);
            match &mut self.controls[i].control {
                Control::Button { id, .. } => return DialogResult::Button(id.clone()),
                Control::List { id, items, selected, scroll, header, activates } => {
                    let row = ev.pos.y - ir.y - header.is_some() as i32;
                    // (A click on the scroll bar column scrolls.)
                    if ev.pos.x == cr.right() - 1 && items.len() > rows && row >= 0 {
                        let max = items.len().saturating_sub(rows);
                        if row == 0 {
                            *scroll = scroll.saturating_sub(1);
                        } else if row as usize >= rows - 1 {
                            *scroll = (*scroll + 1).min(max);
                        } else {
                            // (On the track: the thumb jumps there and drags.)
                            self.drag = Some(Drag::Thumb { i });
                            self.thumb_to(i, ev.pos.y);
                            return DialogResult::None;
                        }
                        *selected = (*selected).clamp(*scroll, *scroll + rows - 1);
                        return DialogResult::None;
                    }
                    if row < 0 {
                        return DialogResult::None;
                    }
                    let idx = row as usize + *scroll;
                    if idx < items.len() {
                        let changed = *selected != idx;
                        *selected = idx;
                        if double && *activates {
                            return DialogResult::Activated(id.clone());
                        }
                        if changed {
                            return DialogResult::Changed(id.clone());
                        }
                    }
                    return DialogResult::None;
                }
                Control::Input { value, cursor, .. } => {
                    let pad = if self.boxed.get(i).copied().flatten().is_some() { 0 } else { 1 };
                    let col = (ev.pos.x - ir.x - pad).max(0) as usize;
                    let mut w = 0;
                    *cursor = value.len();
                    for (bi, g) in unicode_segmentation::UnicodeSegmentation::grapheme_indices(value.as_str(), true) {
                        if w >= col {
                            *cursor = bi;
                            break;
                        }
                        w += turbine_text::unicode::grapheme_width(g);
                    }
                    return DialogResult::None;
                }
                Control::Check { id, checked, .. } => {
                    *checked = !*checked;
                    return DialogResult::Changed(id.clone());
                }
                Control::Radio { id, .. } => {
                    let id = id.clone();
                    self.select_radio(i);
                    return DialogResult::Changed(id);
                }
                Control::Combo { open, .. } => {
                    *open = true;
                    return DialogResult::None;
                }
                Control::Label { .. } | Control::Group { .. } => {}
            }
        }
        DialogResult::None
    }

    // ---- drawing -----------------------------------------------------------------

    /// Is this row free of other controls between x0 and x1 (for a VB-DOS
    /// button's box)?
    fn row_free(&self, me: usize, y: i32, x0: i32, x1: i32) -> bool {
        (0..self.controls.len()).all(|j| {
            if j == me || matches!(self.controls[j].control, Control::Group { .. }) {
                return true;
            }
            let o = self.control_rect(j);
            // (A field's or combo's label sits on the row above it.)
            let top = if matches!(self.controls[j].control, Control::Input { .. } | Control::Combo { .. }) { o.y - 1 } else { o.y };
            !(y >= top && y < o.bottom() && x1 > o.x && x0 < o.right())
        })
    }

    pub fn draw(&mut self, c: &mut Canvas, theme: &Theme) {
        let s = &theme.scheme;
        let st = &theme.style;
        let r = self.rect;
        c.fill(r, " ", s.dialog);
        draw_frame(c, r, st.dialog_frame, s.dialog_frame);
        if st.title_bar {
            c.fill(Rect::new(r.x, r.y, r.w, 1), " ", s.dialog_title);
            let w = str_width(&self.title) as i32;
            c.print(r.x + (r.w - w) / 2, r.y, &self.title, s.dialog_title);
        } else {
            draw_title(c, r, &self.title, s.dialog_frame);
        }
        if st.shadows {
            c.shadow(r, s.shadow);
        }
        // (QBasic: a line across the box above the buttons, when they share a row.)
        if st.dialog_rule {
            let buttons: Vec<i32> = (0..self.controls.len())
                .filter(|&i| matches!(self.controls[i].control, Control::Button { .. }))
                .map(|i| self.control_rect(i).y)
                .collect();
            if let Some(&by) = buttons.iter().max() {
                let y = by - 1;
                let clear = (0..self.controls.len()).all(|j| {
                    let o = self.control_rect(j);
                    matches!(self.controls[j].control, Control::Button { .. }) || !(y >= o.y && y < o.bottom())
                });
                if y > r.y + 1 && clear && buttons.iter().all(|&b| b == by) {
                    let fc = st.dialog_frame;
                    c.print(r.x, y, if fc.v == "║" { "╟" } else { "├" }, s.dialog_frame);
                    c.hline(r.x + 1, y, r.w - 2, "─", s.dialog_frame);
                    c.print(r.right() - 1, y, if fc.v == "║" { "╢" } else { "┤" }, s.dialog_frame);
                }
            }
        }
        // (No resize grip: DOS dialogs never had one; the edges still drag.)
        self.boxed = (0..self.controls.len()).map(|i| if st.boxed_fields { self.box_of(i).map(|(_, inner)| inner) } else { None }).collect();
        let boxes: Vec<Option<Rect>> = (0..self.controls.len()).map(|i| if st.boxed_fields { self.box_of(i).map(|(outer, _)| outer) } else { None }).collect();
        let mut c = c.sub(r.inset(1));
        // The boxes: QBasic's in the frame's colour, VB-DOS's sunken (dark
        // top and left, white bottom and right).
        for b in boxes.iter().flatten() {
            let (lo, hi) = if st.title_bar { (s.dialog.fg(vga::BLACK), s.dialog.fg(vga::WHITE)) } else { (s.dialog_frame, s.dialog_frame) };
            let (x0, x1, y0, y1) = (b.x, b.right() - 1, b.y, b.bottom() - 1);
            c.print(x0, y0, "┌", lo);
            c.hline(x0 + 1, y0, x1 - x0 - 1, "─", lo);
            c.print(x1, y0, "┐", hi);
            c.vline(x0, y0 + 1, y1 - y0 - 1, "│", lo);
            c.vline(x1, y0 + 1, y1 - y0 - 1, "│", hi);
            c.print(x0, y1, "└", lo);
            c.hline(x0 + 1, y1, x1 - x0 - 1, "─", hi);
            c.print(x1, y1, "┘", hi);
        }
        // Group boxes first: the controls draw over their insides.
        let order: Vec<usize> = (0..self.controls.len())
            .filter(|&i| matches!(self.controls[i].control, Control::Group { .. }))
            .chain((0..self.controls.len()).filter(|&i| !matches!(self.controls[i].control, Control::Group { .. })))
            .collect();
        for i in order {
            let cr = self.control_rect(i);
            let focused = i == self.focus;
            let boxed_button = st.title_bar
                && matches!(self.controls[i].control, Control::Button { .. })
                && self.row_free(i, cr.y - 1, cr.x - 1, cr.right() + 1)
                && self.row_free(i, cr.y + 1, cr.x - 1, cr.right() + 1);
            let rows = if matches!(self.controls[i].control, Control::List { .. }) { self.list_rows(i) } else { 1 };
            match &mut self.controls[i].control {
                Control::Label { text } => {
                    c.print(cr.x, cr.y, text, s.dialog);
                }
                Control::Group { title } => {
                    draw_frame(&mut c, cr, crate::theme::SINGLE, s.dialog_frame);
                    if !title.is_empty() {
                        c.print_hot(cr.x + 1, cr.y, title, s.dialog, s.label_hotkey);
                    }
                }
                Control::Input { label, value, cursor, .. } => {
                    let boxed = self.boxed[i].is_some();
                    if !label.is_empty() {
                        // (Boxed: the label beside the field, as in QBasic.)
                        c.print_hot(cr.x, if boxed { cr.y } else { cr.y - 1 }, label, if focused { s.label_focused } else { s.dialog }, s.label_hotkey);
                    }
                    // (Inside a box the field's text starts at its left edge.)
                    let cr = match self.boxed[i] {
                        Some(ir) => Rect::new(ir.x - 1, ir.y, ir.w + 1, 1),
                        None => cr,
                    };
                    let field = self.boxed[i].unwrap_or(cr);
                    c.fill(field, " ", s.input);
                    let before = str_width(&value[..*cursor]) as i32;
                    let skip = (before - cr.w + 2).max(0);
                    let mut sub = c.sub(field);
                    sub.print(cr.x - skip + 1, cr.y, value, s.input);
                    if focused {
                        sub.set_cursor(Point::new(cr.x + 1 + before - skip, cr.y), CursorShape::Underline);
                    }
                }
                Control::Button { label, default, .. } => {
                    let (l, rr) = st.button;
                    let text = format!("{l}{label}{rr}");
                    let w = str_width(&text.replace('~', "")) as i32;
                    if boxed_button {
                        // VB-DOS: a 3-D box lit from the top left; the default
                        // or focused button gets a heavier lower-right edge.
                        let hi = s.dialog.fg(vga::WHITE);
                        let lo = s.dialog.fg(vga::BLACK);
                        let (x0, x1) = (cr.x - 1, cr.x + w);
                        let strong = *default || focused;
                        c.print(x0, cr.y - 1, "┌", hi);
                        c.hline(x0 + 1, cr.y - 1, x1 - x0 - 1, "─", hi);
                        c.print(x1, cr.y - 1, if strong { "╖" } else { "┐" }, lo);
                        c.print(x0, cr.y, "│", hi);
                        c.print(x1, cr.y, if strong { "║" } else { "│" }, lo);
                        c.print(x0, cr.y + 1, "└", hi);
                        c.hline(x0 + 1, cr.y + 1, x1 - x0 - 1, if strong { "═" } else { "─" }, lo);
                        c.print(x1, cr.y + 1, if strong { "╝" } else { "┘" }, lo);
                        c.fill(Rect::new(cr.x, cr.y, w, 1), " ", s.button);
                        let lw = str_width(&label.replace('~', "")) as i32;
                        let label_st = if focused { s.button_focused } else { s.button };
                        c.print_hot(cr.x + (w - lw) / 2, cr.y, label, label_st, s.button_hotkey);
                    } else if st.dos_status {
                        // QBasic: "< OK >", the focused button's brackets bright.
                        let br = if focused { s.button_focused } else { s.button };
                        let x = c.print(cr.x, cr.y, l, br);
                        let x = c.print_hot(x, cr.y, label, s.button, s.button_hotkey);
                        c.print(x, cr.y, rr, br);
                    } else {
                        let stl = if focused { s.button_focused } else if *default { s.button_default } else { s.button };
                        c.print_hot(cr.x, cr.y, &text, stl, s.button_hotkey);
                        if st.shadows && st.button.0.trim().is_empty() {
                            c.print(cr.x + w, cr.y, "▄", s.dialog.fg(vga::BLACK));
                            c.hline(cr.x + 1, cr.y + 1, w, "▀", s.dialog.fg(vga::BLACK));
                        }
                    }
                    if focused {
                        c.set_cursor(Point::new(cr.x + str_width(l) as i32, cr.y), CursorShape::Underline);
                    }
                }
                Control::Check { label, checked, .. } => {
                    let x = c.print(cr.x, cr.y, if *checked { "[X] " } else { "[ ] " }, s.dialog);
                    c.print_hot(x, cr.y, label, s.dialog, s.label_hotkey);
                    if focused {
                        c.set_cursor(Point::new(cr.x + 1, cr.y), CursorShape::Underline);
                    }
                }
                Control::Radio { label, checked, .. } => {
                    let x = c.print(cr.x, cr.y, if *checked { "(•) " } else { "( ) " }, s.dialog);
                    c.print_hot(x, cr.y, label, s.dialog, s.label_hotkey);
                    if focused {
                        c.set_cursor(Point::new(cr.x + 1, cr.y), CursorShape::Underline);
                    }
                }
                Control::Combo { label, items, selected, .. } => {
                    let boxed = self.boxed[i].is_some();
                    if !label.is_empty() {
                        c.print_hot(cr.x, if boxed { cr.y } else { cr.y - 1 }, label, if focused { s.label_focused } else { s.dialog }, s.label_hotkey);
                    }
                    let cr = match self.boxed[i] {
                        Some(ir) => Rect::new(ir.x - 1, ir.y, ir.w + 1, 1),
                        None => cr,
                    };
                    // Turbo Vision: a green ▐↓▌ beside the field (its history
                    // button); elsewhere a ↓ at the field's end.
                    let tv = !boxed && st.shadows && st.button.0.trim().is_empty();
                    let cr = if tv { Rect::new(cr.x, cr.y, cr.w - 3, 1) } else { cr };
                    c.fill(self.boxed[i].unwrap_or(cr), " ", s.input);
                    let mut sub = c.sub(Rect::new(cr.x, cr.y, cr.w - if tv { 0 } else { 2 }, 1));
                    sub.print(cr.x + 1, cr.y, items.get(*selected).map(String::as_str).unwrap_or(""), s.input);
                    if tv {
                        let edge = Style::new(s.button.bg, s.dialog.bg);
                        c.print(cr.right(), cr.y, "▐", edge);
                        c.print(cr.right() + 1, cr.y, "↓", s.button);
                        c.print(cr.right() + 2, cr.y, "▌", edge);
                    } else {
                        c.print(cr.right() - 1, cr.y, st.scroll[1], s.scrollbar);
                    }
                    if focused {
                        c.set_cursor(Point::new(cr.x + 1, cr.y), CursorShape::Underline);
                    }
                }
                Control::List { items, selected, scroll, header, .. } => {
                    // (Boxed: the items inside, the scroll bar on the box's right side.)
                    let outer = cr;
                    let cr = self.boxed[i].unwrap_or(cr);
                    if *selected < *scroll {
                        *scroll = *selected;
                    } else if *selected >= *scroll + rows {
                        *scroll = *selected + 1 - rows;
                    }
                    c.fill(cr, " ", s.list);
                    let bar = items.len() > rows;
                    let text_w = if outer != cr { cr.w } else { cr.w - bar as i32 };
                    let mut y0 = cr.y;
                    if let Some(h) = header {
                        // (Column titles: the dialog's colours, underlined.)
                        let mut hs = s.dialog;
                        hs.attrs.underline = true;
                        c.fill(Rect::new(cr.x, cr.y, cr.w, 1), " ", hs);
                        c.sub(Rect::new(cr.x, cr.y, cr.w, 1)).print(cr.x + 1, cr.y, h, hs);
                        y0 += 1;
                    }
                    let mut sub = c.sub(Rect::new(cr.x, y0, text_w, rows as i32));
                    for (row, it) in items.iter().enumerate().skip(*scroll).take(rows) {
                        let y = y0 + (row - *scroll) as i32;
                        let stl = if row == *selected {
                            if focused { s.list_selected } else { s.list_selected_unfocused }
                        } else {
                            s.list
                        };
                        sub.fill(Rect::new(cr.x, y, text_w, 1), " ", stl);
                        sub.print(cr.x + 1, y, it, stl);
                    }
                    if bar {
                        // (The thumb shows where the view is, so it can be dragged.)
                        let max = items.len().saturating_sub(rows);
                        draw_vscroll(&mut c, outer.right() - 1, y0, rows as i32, *scroll, max + 1, s.scrollbar, st.scroll);
                    }
                    if focused {
                        let y = y0 + (*selected as i32 - *scroll as i32).max(0);
                        c.set_cursor(Point::new(cr.x + 1, y), CursorShape::Underline);
                    }
                }
            }
        }
    }

    /// Draw an open combo box's list (after `draw`, so it's on top).
    pub fn draw_overlays(&self, c: &mut Canvas, theme: &Theme) {
        let Some((i, pr)) = self.combo_popup() else { return };
        let Control::Combo { items, selected, .. } = &self.controls[i].control else { return };
        let s = &theme.scheme;
        c.fill(pr, " ", s.menu);
        draw_frame(c, pr, theme.style.menu_frame, s.menu);
        if theme.style.shadows {
            c.shadow(pr, s.shadow);
        }
        let (top, rows) = (self.combo_top(i, pr), (pr.h - 2).max(0) as usize);
        if items.len() > rows {
            draw_vscroll(c, pr.right() - 1, pr.y + 1, rows as i32, *selected, items.len(), s.scrollbar, theme.style.scroll);
        }
        for (row, it) in items.iter().enumerate().skip(top).take(rows) {
            let y = pr.y + 1 + (row - top) as i32;
            let stl = if row == *selected { s.menu_selected } else { s.menu };
            c.fill(Rect::new(pr.x + 1, y, pr.w - 2, 1), " ", stl);
            c.sub(Rect::new(pr.x + 1, y, pr.w - 2, 1)).print(pr.x + 2, y, it, stl);
        }
    }

    /// The dialog as automation nodes (for agents).
    pub fn automation_node(&self) -> Node {
        let mut dn = Node::new(format!("dialog.{}", self.kind), "dialog", self.title.clone(), self.rect).focused(true);
        for (i, p) in self.controls.iter().enumerate() {
            let r = self.control_rect(i);
            let id = p.control.id().map(|s| format!("dialog.{s}")).unwrap_or_else(|| format!("dialog.label{i}"));
            let mut n = Node::new(id, p.control.role(), p.control.label_text(), r).focused(i == self.focus);
            match &p.control {
                Control::Input { value, .. } => n = n.value(value.clone()),
                Control::List { items, selected, scroll, header, .. } => {
                    if let Some(v) = items.get(*selected) {
                        n = n.value(v.clone());
                    }
                    let r = self.inner(i);
                    let y0 = r.y + header.is_some() as i32;
                    let rows = (r.h - header.is_some() as i32).max(1) as usize;
                    for (row, it) in items.iter().enumerate().skip(*scroll).take(rows) {
                        let ir = Rect::new(r.x, y0 + (row - scroll) as i32, r.w, 1);
                        let cid = format!("{}.{row}", n.id);
                        n = n.child(Node::new(cid, "listitem", it.clone(), ir).focused(row == *selected));
                    }
                }
                Control::Check { checked, .. } | Control::Radio { checked, .. } => {
                    n = n.value(if *checked { "checked" } else { "unchecked" })
                }
                Control::Combo { items, selected, open, .. } => {
                    if let Some(v) = items.get(*selected) {
                        n = n.value(v.clone());
                    }
                    if *open {
                        if let Some((ci, pr)) = self.combo_popup() {
                            let top = self.combo_top(ci, pr);
                            for (row, it) in items.iter().enumerate().skip(top).take((pr.h - 2).max(0) as usize) {
                                let ir = Rect::new(pr.x + 1, pr.y + 1 + (row - top) as i32, pr.w - 2, 1);
                                let cid = format!("{}.{row}", n.id);
                                n = n.child(Node::new(cid, "listitem", it.clone(), ir));
                            }
                        }
                    }
                }
                _ => {}
            }
            dn = dn.child(n);
        }
        dn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Mods;

    fn dlg() -> Dialog {
        Dialog::new("t", "Test", 40, 14)
            .input("name", 1, 2, 24, "~N~ame:", "")
            .button("ok", 28, 2, "O~K~", true)
            .button("cancel", 28, 5, "Cancel", false)
            .activating_list("files", Rect::new(1, 4, 24, 5), Some("Name"), (0..20).map(|i| format!("file{i}")).collect(), 0)
            .check("hidden", 1, 10, "~H~idden", false)
    }

    #[test]
    fn anchors_stretch_and_follow() {
        let screen = Rect::new(0, 0, 100, 40);
        let mut d = dlg();
        d.layout(screen);
        let (list0, ok0, hid0, r0) = (d.control_rect(3), d.control_rect(1), d.control_rect(4), d.rect);
        d.size = (60, 24);
        d.layout(screen);
        let (list1, ok1, hid1, r1) = (d.control_rect(3), d.control_rect(1), d.control_rect(4), d.rect);
        assert_eq!((list1.w - list0.w, list1.h - list0.h), (20, 10), "the list stretches");
        assert_eq!(r1.right() - ok1.right(), r0.right() - ok0.right(), "OK keeps to the right edge");
        assert_eq!(r1.bottom() - hid1.y, r0.bottom() - hid0.y, "the check box keeps to the bottom");
    }

    #[test]
    fn resize_by_dragging_the_corner() {
        let mut d = dlg();
        d.layout(Rect::new(0, 0, 100, 40));
        let r = d.rect;
        let corner = Point::new(r.right() - 1, r.bottom() - 1);
        let m = |kind, pos| MouseEvent { kind, pos, mods: Mods::NONE };
        d.handle_mouse(&m(MouseKind::Down(Button::Left), corner));
        d.handle_mouse(&m(MouseKind::Drag(Button::Left), Point::new(corner.x + 10, corner.y + 5)));
        d.handle_mouse(&m(MouseKind::Up(Button::Left), corner));
        d.layout(Rect::new(0, 0, 100, 40));
        assert_eq!((d.rect.w, d.rect.h), (50, 19));
    }

    #[test]
    fn lists_type_ahead_activate_and_report_changes() {
        let mut d = dlg();
        d.focus_id("files");
        assert_eq!(d.handle_key(&KeyEvent::plain(Key::Down)), DialogResult::Changed("files".into()));
        assert_eq!(d.handle_key(&KeyEvent::plain(Key::Enter)), DialogResult::Activated("files".into()));
        d.focus_id("name");
        assert_eq!(d.handle_key(&KeyEvent::plain(Key::Char('a'))), DialogResult::Changed("name".into()));
        assert_eq!(d.handle_key(&KeyEvent::new(Key::Char('h'), Mods::ALT)), DialogResult::Changed("hidden".into()));
        assert!(d.checked("hidden"));
    }
    #[test]
    fn list_scrolls_to_the_bottom_by_wheel_and_thumb_drag() {
        let mut d = dlg();
        d.layout(Rect::new(0, 0, 100, 40));
        let i = d.controls.iter().position(|p| p.control.id() == Some("files")).unwrap();
        let cr = d.control_rect(i);
        let rows = d.list_rows(i);
        let m = |kind, pos| MouseEvent { kind, pos, mods: Mods::NONE };
        let scroll = |d: &Dialog| match &d.controls[i].control { Control::List { scroll, .. } => *scroll, _ => 0 };
        for _ in 0..20 {
            d.handle_mouse(&m(MouseKind::ScrollDown, Point::new(cr.x + 2, cr.y + 2)));
        }
        assert_eq!(scroll(&d), 20 - rows);
        for _ in 0..20 {
            d.handle_mouse(&m(MouseKind::ScrollUp, Point::new(cr.x + 2, cr.y + 2)));
        }
        assert_eq!(scroll(&d), 0);
        // Grab the track, drag to its end: the last items show.
        let x = cr.right() - 1;
        let y0 = cr.y + 1; // (below the header)
        d.handle_mouse(&m(MouseKind::Down(Button::Left), Point::new(x, y0 + 1)));
        d.handle_mouse(&m(MouseKind::Drag(Button::Left), Point::new(x, y0 + rows as i32 + 5)));
        d.handle_mouse(&m(MouseKind::Up(Button::Left), Point::new(x, y0 + rows as i32 + 5)));
        assert_eq!(scroll(&d), 20 - rows);
        let n = d.automation_node();
        let names: Vec<_> = n.find_target("dialog.files").unwrap().children.iter().map(|c| c.name.clone()).collect();
        assert_eq!(names.last().map(String::as_str), Some("file19"));
    }
}
