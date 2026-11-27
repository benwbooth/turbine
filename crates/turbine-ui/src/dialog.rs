//! Dialog boxes: movable and resizable, laid out by their contents.
//!
//! A dialog is a tree of controls in rows, columns and group boxes (see
//! `row`, `col`, `group`), plus its buttons. Nothing has a fixed size: each
//! control measures itself in the control style in use, so the same dialog
//! looks like QBasic's (fields in boxes with their labels beside them,
//! buttons spread along the bottom under a line), Turbo Vision's or
//! VB-DOS's. Rows and columns share out the space the dialog has, so it
//! can be dragged bigger or smaller and lists and fields stretch.

use crate::automation::Node;
use crate::event::{Button, Key, KeyEvent, KeyKind, MouseEvent, MouseKind};
use crate::frame::{draw_frame, draw_title, draw_vscroll};
use crate::geom::{Point, Rect};
use crate::surface::{Canvas, CursorShape};
use crate::theme::{ButtonLook, ButtonPlace, ControlStyle, FieldLook, Style, Theme, vga};
use std::time::{Duration, Instant};
use turbine_text::unicode::{grapheme_boundaries, str_width};

#[derive(Debug, Clone)]
pub enum Control {
    /// Text; `id` (may be empty) lets `set_label` change it.
    Label { id: String, text: String },
    /// A text field. `fresh`: its text is as it was given, shown
    /// highlighted, and typing replaces it (as DOS dialogs did).
    Input { id: String, label: String, value: String, cursor: usize, fresh: bool },
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
            Control::Label { id, .. } if !id.is_empty() => Some(id),
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
            Control::Label { text, .. } => text.clone(),
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

/// A control's place in the dialog (on screen, from the last layout): the
/// row a field or button sits on; boxes, labels and shadows go around it.
#[derive(Debug, Clone)]
pub struct Placed {
    pub control: Control,
    pub rect: Rect,
}

/// Size wishes for one item of a dialog's layout.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hints {
    /// Columns of text it would like (a field's, a list's).
    pub chars: Option<i32>,
    /// Rows a list would like.
    pub lines: Option<i32>,
    /// Takes spare width / height (lists and fields grow on their own).
    pub grow_x: Option<bool>,
    pub grow_y: Option<bool>,
}

/// A dialog's contents: controls in rows and columns, group boxes, spacers.
/// Nothing has a fixed size or place: each control measures itself in the
/// current control style (a QBasic field is a 3-row box with its label
/// beside it, a Turbo Vision one a label row and a field row), and rows and
/// columns share out what space there is.
#[derive(Debug, Clone)]
pub struct Item {
    kind: ItemKind,
    hints: Hints,
}

#[derive(Debug, Clone)]
enum ItemKind {
    /// Before the dialog takes it.
    Control(Box<Control>),
    /// Afterwards: the control's index.
    Leaf(usize),
    /// Side by side / one under another; the gap (None: the style's).
    Row(Vec<Item>, Option<i32>),
    Col(Vec<Item>, Option<i32>),
    /// A group box (its control) around an item.
    Group(Box<Item>, Box<Item>),
    /// Space that takes what's spare.
    Stretch,
    /// QBasic's line across the dialog above its buttons.
    Rule,
}

impl Item {
    fn new(kind: ItemKind) -> Item {
        Item { kind, hints: Hints::default() }
    }
    fn control(c: Control) -> Item {
        Item::new(ItemKind::Control(Box::new(c)))
    }
    /// Columns of text wanted (a field's, list's or label's width).
    pub fn chars(mut self, n: i32) -> Item {
        self.hints.chars = Some(n);
        self
    }
    /// Rows wanted (a list's).
    pub fn lines(mut self, n: i32) -> Item {
        self.hints.lines = Some(n);
        self
    }
    /// Take (or don't take) spare width.
    pub fn grow_x(mut self, g: bool) -> Item {
        self.hints.grow_x = Some(g);
        self
    }
    /// Take (or don't take) spare height.
    pub fn grow_y(mut self, g: bool) -> Item {
        self.hints.grow_y = Some(g);
        self
    }
    /// A list's column header row.
    pub fn header(mut self, h: &str) -> Item {
        if let ItemKind::Control(c) = &mut self.kind {
            if let Control::List { header, .. } = c.as_mut() {
                *header = Some(h.to_string());
            }
        }
        self
    }
    /// A list whose Enter and double-click report Activated.
    pub fn activates(mut self) -> Item {
        if let ItemKind::Control(c) = &mut self.kind {
            if let Control::List { activates, .. } = c.as_mut() {
                *activates = true;
            }
        }
        self
    }
    /// A label that can be changed later by `set_label`.
    pub fn id(mut self, new_id: &str) -> Item {
        if let ItemKind::Control(c) = &mut self.kind {
            if let Control::Label { id, .. } = c.as_mut() {
                *id = new_id.to_string();
            }
        }
        self
    }
}

/// Text; several lines with `\n`.
pub fn label(text: &str) -> Item {
    Item::control(Control::Label { id: String::new(), text: text.into() })
}
/// A text field; its label (with a `~h~otkey`) beside or above it.
pub fn input(id: &str, label: &str, value: &str) -> Item {
    Item::control(Control::Input { id: id.into(), label: label.into(), value: value.into(), cursor: value.len(), fresh: !value.is_empty() })
}
pub fn button(id: &str, label: &str, default: bool) -> Item {
    Item::control(Control::Button { id: id.into(), label: label.into(), default })
}
pub fn list(id: &str, items: Vec<String>, selected: usize) -> Item {
    Item::control(Control::List { id: id.into(), items, selected, scroll: 0, header: None, activates: false })
}
pub fn check(id: &str, label: &str, checked: bool) -> Item {
    Item::control(Control::Check { id: id.into(), label: label.into(), checked })
}
/// Radio buttons with the same `group` are exclusive.
pub fn radio(id: &str, group: &str, label: &str, checked: bool) -> Item {
    Item::control(Control::Radio { id: id.into(), label: label.into(), group: group.into(), checked })
}
/// A drop-down; its label beside or above it, like a field's.
pub fn combo(id: &str, label: &str, items: Vec<String>, selected: usize) -> Item {
    Item::control(Control::Combo { id: id.into(), label: label.into(), items, selected, open: false })
}
/// A titled box around `inner`.
pub fn group(title: &str, inner: Item) -> Item {
    Item::new(ItemKind::Group(Box::new(Item::control(Control::Group { title: title.into() })), Box::new(inner)))
}
/// Items side by side, the style's gap between them.
pub fn row(items: Vec<Item>) -> Item {
    Item::new(ItemKind::Row(items, None))
}
/// Items one under another, the style's gap between them.
pub fn col(items: Vec<Item>) -> Item {
    Item::new(ItemKind::Col(items, None))
}
/// Items one under another with no gap (radio buttons, check boxes).
pub fn stack(items: Vec<Item>) -> Item {
    Item::new(ItemKind::Col(items, Some(0)))
}
/// Space that takes what's spare.
pub fn stretch() -> Item {
    Item::new(ItemKind::Stretch)
}

/// What an item needs and would like, in cells.
#[derive(Debug, Clone, Copy, Default)]
struct Measure {
    min_w: i32,
    min_h: i32,
    w: i32,
    h: i32,
    grow_x: bool,
    grow_y: bool,
}

/// Share `avail` among items wanting `want` (at least `min`): spare space
/// goes to those that grow, a shortfall comes out of what's above their minimum.
fn share(avail: i32, items: &[(i32, i32, bool)]) -> Vec<i32> {
    let want: i32 = items.iter().map(|i| i.0).sum();
    let mut out: Vec<i32> = items.iter().map(|i| i.0).collect();
    if avail >= want {
        let growers: Vec<usize> = (0..items.len()).filter(|&k| items[k].2).collect();
        if !growers.is_empty() {
            let extra = avail - want;
            for (n, &k) in growers.iter().enumerate() {
                out[k] += extra / growers.len() as i32 + i32::from((n as i32) < extra % growers.len() as i32);
            }
        }
    } else {
        let slack: i32 = items.iter().map(|i| (i.0 - i.1).max(0)).sum();
        let mut cut = (want - avail).min(slack);
        // Proportionally to each one's slack; the rounding off the last ones.
        for k in 0..items.len() {
            let s = (items[k].0 - items[k].1).max(0);
            if slack > 0 && cut > 0 {
                let c = ((want - avail).min(slack) * s / slack).min(cut);
                out[k] -= c;
                cut -= c;
            }
        }
        for k in (0..items.len()).rev() {
            let room = out[k] - items[k].1;
            let c = room.min(cut).max(0);
            out[k] -= c;
            cut -= c;
        }
    }
    out
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
    pub controls: Vec<Placed>,
    pub focus: usize,
    /// Where it is on screen (from the last layout).
    pub rect: Rect,
    /// The size the person dragged it to (else the size its contents want).
    size: Option<(i32, i32)>,
    /// The smallest it can be (from the last layout).
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
    /// The contents, and the buttons (placed by the control style: a row
    /// along the bottom, or a column at the right beside lists).
    body: Item,
    buttons: Vec<Item>,
    /// Rows of QBasic's line above the buttons (from the last layout).
    rules: Vec<i32>,
}

/// Where an item's main row is (fields and drop-downs sit under or beside
/// their labels): rows side by side line up on it.
fn base_of(c: &Control, st: &ControlStyle) -> i32 {
    match c {
        Control::Input { label, .. } | Control::Combo { label, .. } => i32::from(st.controls.boxed() || !label.is_empty()),
        Control::Button { .. } => i32::from(st.controls.button == ButtonLook::Box3d),
        _ => 0,
    }
}

impl Dialog {
    /// A dialog; its contents come from `body` and `buttons`.
    pub fn new(kind: &str, title: &str) -> Self {
        Dialog {
            kind: kind.into(),
            title: title.into(),
            controls: Vec::new(),
            focus: 0,
            rect: Rect::default(),
            size: None,
            min: (0, 0),
            pos: None,
            drag: None,
            last_click: None,
            screen: Rect::default(),
            boxed: Vec::new(),
            body: Item::new(ItemKind::Stretch),
            buttons: Vec::new(),
            rules: Vec::new(),
        }
    }

    /// The controls, as rows, columns and groups (see `row`, `col`, `group`).
    pub fn body(mut self, item: Item) -> Self {
        self.body = self.attach(item);
        self.fix_focus();
        self
    }

    /// The buttons (made with `button`), in order.
    pub fn buttons(mut self, items: Vec<Item>) -> Self {
        self.buttons = items.into_iter().map(|i| self.attach(i)).collect();
        self.fix_focus();
        self
    }

    /// Move an item's controls into the dialog (in reading order: that's
    /// the Tab order), leaving their indexes in the tree.
    fn attach(&mut self, item: Item) -> Item {
        let hints = item.hints;
        let kind = match item.kind {
            ItemKind::Control(c) => {
                self.controls.push(Placed { control: *c, rect: Rect::default() });
                ItemKind::Leaf(self.controls.len() - 1)
            }
            ItemKind::Row(v, g) => ItemKind::Row(v.into_iter().map(|i| self.attach(i)).collect(), g),
            ItemKind::Col(v, g) => ItemKind::Col(v.into_iter().map(|i| self.attach(i)).collect(), g),
            ItemKind::Group(g, inner) => {
                let g = self.attach(*g);
                ItemKind::Group(Box::new(g), Box::new(self.attach(*inner)))
            }
            k => k,
        };
        Item { kind, hints }
    }

    // ---- measuring and arranging ----------------------------------------------------

    fn measure_control(&self, i: usize, h: &Hints, st: &ControlStyle) -> Measure {
        let text_w = |s: &str| str_width(&s.replace('~', "")) as i32;
        let mut m = match &self.controls[i].control {
            Control::Label { text, .. } => {
                let w = h.chars.unwrap_or_else(|| text.split('\n').map(|l| str_width(l) as i32).max().unwrap_or(0));
                let n = text.split('\n').count() as i32;
                Measure { min_w: w.min(12), min_h: n, w, h: n, ..Default::default() }
            }
            Control::Input { label, .. } | Control::Combo { label, .. } => {
                let combo = matches!(self.controls[i].control, Control::Combo { .. });
                let lw = text_w(label);
                let items_w = match &self.controls[i].control {
                    Control::Combo { items, .. } => items.iter().map(|s| str_width(s) as i32).max().unwrap_or(4) + 2,
                    _ => 20,
                };
                let c = h.chars.unwrap_or(items_w) + if combo && !st.controls.boxed() && st.controls.combo_arrow_beside { 3 } else { 0 };
                if st.controls.boxed() {
                    let lead = if lw > 0 { lw + 1 } else { 0 };
                    Measure { min_w: lead + 10, min_h: 3, w: lead + c + 2, h: 3, grow_x: !combo, ..Default::default() }
                } else {
                    let rows = if lw > 0 { 2 } else { 1 };
                    Measure { min_w: lw.max(10), min_h: rows, w: lw.max(c + 2), h: rows, grow_x: !combo, ..Default::default() }
                }
            }
            Control::Button { label, .. } => {
                let lw = text_w(label);
                let (w, hh) = match st.controls.button {
                    // A 3-D box, the label with room either side.
                    ButtonLook::Box3d => (lw + 6, 3),
                    // A bar and its shadow (a column right, a row below).
                    ButtonLook::Bar { min_w } => ((lw + 4).max(min_w) + 1, 2),
                    ButtonLook::Brackets(l, r) => (text_w(l) + lw + text_w(r), 1),
                };
                Measure { min_w: w, min_h: hh, w, h: hh, ..Default::default() }
            }
            Control::Check { label, .. } | Control::Radio { label, .. } => {
                let w = text_w(label) + 4;
                Measure { min_w: w, min_h: 1, w, h: 1, ..Default::default() }
            }
            Control::List { items, header, .. } => {
                let pad = if st.controls.boxed() { 2 } else { 0 };
                let widest = items.iter().map(|s| str_width(s) as i32).max().unwrap_or(0).max(header.as_deref().map_or(0, |s| str_width(s) as i32));
                let w = h.chars.unwrap_or((widest + 3).clamp(12, 60)) + pad;
                let hd = i32::from(header.is_some());
                let rows = h.lines.unwrap_or((items.len() as i32).clamp(3, 12));
                Measure { min_w: 10 + pad, min_h: 3 + hd + pad, w, h: rows + hd + pad, grow_x: true, grow_y: true, ..Default::default() }
            }
            Control::Group { title } => {
                let w = text_w(title) + 4;
                Measure { min_w: w, min_h: 2, w, h: 2, ..Default::default() }
            }
        };
        if let Some(g) = h.grow_x {
            m.grow_x = g;
        }
        if let Some(g) = h.grow_y {
            m.grow_y = g;
        }
        m.min_w = m.min_w.min(m.w);
        m.min_h = m.min_h.min(m.h);
        m
    }

    fn measure(&self, item: &Item, st: &ControlStyle) -> (Measure, i32) {
        let (mut m, base) = match &item.kind {
            ItemKind::Leaf(i) => (self.measure_control(*i, &item.hints, st), base_of(&self.controls[*i].control, st)),
            ItemKind::Control(_) => (Measure::default(), 0),
            ItemKind::Stretch => (Measure { grow_x: true, grow_y: true, ..Default::default() }, 0),
            ItemKind::Rule => (Measure { min_h: 2, h: 2, grow_x: true, ..Default::default() }, 0),
            ItemKind::Row(v, gap) => {
                let gap = gap.unwrap_or(st.controls.col_gap);
                let ms: Vec<(Measure, i32)> = v.iter().map(|i| self.measure(i, st)).collect();
                let base = ms.iter().map(|m| m.1).max().unwrap_or(0);
                let gaps = gap * (v.len() as i32 - 1).max(0);
                let mut m = Measure { w: gaps, min_w: gaps, ..Default::default() };
                for (c, b) in &ms {
                    m.w += c.w;
                    m.min_w += c.min_w;
                    m.h = m.h.max(base - b + c.h);
                    m.min_h = m.min_h.max(base - b + c.min_h);
                    m.grow_x |= c.grow_x;
                    m.grow_y |= c.grow_y;
                }
                (m, base)
            }
            ItemKind::Col(v, gap) => {
                let gap = gap.unwrap_or(st.controls.row_gap);
                let ms: Vec<(Measure, i32)> = v.iter().map(|i| self.measure(i, st)).collect();
                let gaps = gap * (v.len() as i32 - 1).max(0);
                let mut m = Measure { h: gaps, min_h: gaps, ..Default::default() };
                for (c, _) in &ms {
                    m.h += c.h;
                    m.min_h += c.min_h;
                    m.w = m.w.max(c.w);
                    m.min_w = m.min_w.max(c.min_w);
                    m.grow_x |= c.grow_x;
                    m.grow_y |= c.grow_y;
                }
                (m, ms.first().map_or(0, |m| m.1))
            }
            ItemKind::Group(g, inner) if self.heading_list(inner) => {
                let (gm, _) = self.measure(g, st);
                let (im, _) = self.measure(inner, st);
                let m = Measure {
                    w: im.w.max(gm.w - 2),
                    min_w: im.min_w.max(gm.w - 2),
                    h: im.h + 1,
                    min_h: im.min_h + 1,
                    grow_x: im.grow_x,
                    grow_y: im.grow_y,
                };
                (m, 0)
            }
            ItemKind::Group(g, inner) => {
                let (gm, _) = self.measure(g, st);
                let (im, _) = self.measure(inner, st);
                let m = Measure {
                    w: im.w.max(gm.w - 4) + 4,
                    min_w: im.min_w.max(gm.w - 4) + 4,
                    h: im.h + 2,
                    min_h: im.min_h + 2,
                    grow_x: im.grow_x,
                    grow_y: im.grow_y,
                };
                (m, 0)
            }
        };
        if let Some(g) = item.hints.grow_x {
            m.grow_x = g;
        }
        if let Some(g) = item.hints.grow_y {
            m.grow_y = g;
        }
        (m, base)
    }

    /// A group box around a single list is drawn as a heading above the
    /// list instead (as the originals label their lists: "Files" over the
    /// box), so a list never sits in two frames.
    fn heading_list(&self, inner: &Item) -> bool {
        matches!(inner.kind, ItemKind::Leaf(i) if matches!(self.controls[i].control, Control::List { .. }))
    }

    /// Where a control's main row goes, given the space laid out for it.
    fn core(&self, i: usize, f: Rect, st: &ControlStyle) -> Rect {
        match &self.controls[i].control {
            Control::Input { label, .. } | Control::Combo { label, .. } => {
                let dy = i32::from(st.controls.boxed() || !label.is_empty());
                Rect::new(f.x, f.y + dy, f.w, 1)
            }
            Control::Button { .. } if st.controls.button == ButtonLook::Box3d => Rect::new(f.x + 1, f.y + 1, (f.w - 2).max(1), 1),
            Control::Button { .. } if matches!(st.controls.button, ButtonLook::Bar { .. }) => Rect::new(f.x, f.y, (f.w - 1).max(1), 1),
            Control::Button { .. } | Control::Check { .. } | Control::Radio { .. } => Rect::new(f.x, f.y, f.w, 1),
            _ => f,
        }
    }

    fn arrange(&mut self, item: &Item, r: Rect, st: &ControlStyle) {
        match &item.kind {
            ItemKind::Leaf(i) => {
                let c = self.core(*i, r, st);
                self.controls[*i].rect = c;
            }
            ItemKind::Rule => self.rules.push(r.bottom() - 1),
            ItemKind::Control(_) | ItemKind::Stretch => {}
            ItemKind::Row(v, gap) => {
                let gap = gap.unwrap_or(st.controls.col_gap);
                let ms: Vec<(Measure, i32)> = v.iter().map(|i| self.measure(i, st)).collect();
                let base = ms.iter().map(|m| m.1).max().unwrap_or(0);
                let gaps = gap * (v.len() as i32 - 1).max(0);
                let ws = share(r.w - gaps, &ms.iter().map(|(m, _)| (m.w, m.min_w, m.grow_x)).collect::<Vec<_>>());
                let mut x = r.x;
                for (k, child) in v.iter().enumerate() {
                    let (m, b) = ms[k];
                    let dy = base - b;
                    let h = if m.grow_y { r.h - dy } else { m.h.min(r.h - dy) };
                    self.arrange(child, Rect::new(x, r.y + dy, ws[k], h.max(0)), st);
                    x += ws[k] + gap;
                }
            }
            ItemKind::Col(v, gap) => {
                let gap = gap.unwrap_or(st.controls.row_gap);
                let ms: Vec<(Measure, i32)> = v.iter().map(|i| self.measure(i, st)).collect();
                let gaps = gap * (v.len() as i32 - 1).max(0);
                let hs = share(r.h - gaps, &ms.iter().map(|(m, _)| (m.h, m.min_h, m.grow_y)).collect::<Vec<_>>());
                let mut y = r.y;
                for (k, child) in v.iter().enumerate() {
                    let m = ms[k].0;
                    let w = if m.grow_x { r.w } else { m.w.min(r.w) };
                    self.arrange(child, Rect::new(r.x, y, w, hs[k]), st);
                    y += hs[k] + gap;
                }
            }
            ItemKind::Group(g, inner) if self.heading_list(inner) => {
                // (The group's rect: just its heading row.)
                self.arrange(g, Rect::new(r.x, r.y, r.w, 1), st);
                self.arrange(inner, Rect::new(r.x, r.y + 1, r.w, (r.h - 1).max(0)), st);
            }
            ItemKind::Group(g, inner) => {
                self.arrange(g, r, st);
                self.arrange(inner, Rect::new(r.x + 2, r.y + 1, (r.w - 4).max(0), (r.h - 2).max(0)), st);
            }
        }
    }

    /// The whole contents with the buttons where this control style puts
    /// them (see `ButtonPlace`). True when they're along the bottom edge.
    fn whole(&self, st: &ControlStyle) -> (Item, bool) {
        let body = self.body.clone();
        if self.buttons.is_empty() {
            return (body, false);
        }
        let look = &st.controls;
        let tall = self.measure(&body, st).0.grow_y;
        match look.buttons {
            ButtonPlace::BottomRule => {
                let mut bar = vec![stretch()];
                for b in &self.buttons {
                    bar.push(b.clone());
                    bar.push(stretch());
                }
                let bar = Item::new(ItemKind::Row(bar, Some(0)));
                (Item::new(ItemKind::Col(vec![body.grow_y(true), Item::new(ItemKind::Rule), bar], Some(0))), true)
            }
            ButtonPlace::Beside if tall => {
                let mut buttons: Vec<Item> = self.buttons.iter().map(|b| b.clone().grow_x(true)).collect();
                buttons.push(stretch().grow_x(false));
                let column = Item::new(ItemKind::Col(buttons, Some(look.button_row_gap))).grow_y(true).grow_x(false);
                (Item::new(ItemKind::Row(vec![body.grow_x(true), column], Some(look.col_gap + 1))), false)
            }
            ButtonPlace::Beside => {
                let mut bar = vec![stretch()];
                bar.extend(self.buttons.iter().cloned());
                bar.push(stretch());
                let bar = Item::new(ItemKind::Row(bar, Some(look.button_col_gap)));
                (Item::new(ItemKind::Col(vec![body.grow_y(true), bar], Some(look.row_gap.max(1)))), false)
            }
        }
    }

    /// Lay the dialog out on the screen in this control style: the size its
    /// contents want (or the size it was dragged to; never larger than the
    /// screen nor smaller than its contents need), moved or centred, and
    /// every control placed.
    pub fn layout(&mut self, screen: Rect, theme: &Theme) {
        let st = &theme.style;
        self.screen = screen;
        let (top, bottom_bar) = self.whole(st);
        let (m, _) = self.measure(&top, st);
        // The frame and the style's padding (buttons along the bottom edge
        // sit right on it).
        let look = &st.controls;
        let pad_h = 2 + look.pad_top + if bottom_bar { 0 } else { look.pad_bottom };
        let pad_w = 2 + 2 * look.pad_x;
        let title_w = str_width(&self.title) as i32 + 8;
        self.min = ((m.min_w + pad_w).max(title_w).min(screen.w), (m.min_h + pad_h).min(screen.h));
        let want = self.size.unwrap_or(((m.w + pad_w).max(title_w), m.h + pad_h));
        let w = want.0.clamp(self.min.0, screen.w.max(self.min.0)).min(screen.w);
        let h = want.1.clamp(self.min.1, screen.h.max(self.min.1)).min(screen.h);
        self.rect = match self.pos {
            Some(p) => Rect::new(
                p.x.clamp(screen.x, (screen.right() - w).max(screen.x)),
                p.y.clamp(screen.y, (screen.bottom() - h).max(screen.y)),
                w,
                h,
            ),
            None => screen.centered(w, h),
        };
        let r = self.rect;
        let inside = Rect::new(r.x + 1 + look.pad_x, r.y + 1 + look.pad_top, (r.w - pad_w).max(0), (r.h - pad_h).max(0));
        self.rules.clear();
        self.arrange(&top, inside, st);
        let boxed = st.controls.boxed();
        self.boxed = (0..self.controls.len()).map(|i| if boxed { self.box_of(i).map(|(_, inner)| inner) } else { None }).collect();
    }

    /// A control's place on screen (from the last layout).
    pub fn control_rect(&self, i: usize) -> Rect {
        self.controls[i].rect
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
            if let Control::Input { id: i, value, cursor, fresh, .. } = &mut p.control {
                if i == id {
                    *value = v.to_string();
                    *cursor = value.len();
                    *fresh = !value.is_empty();
                }
            }
        }
    }

    /// Change the label of a button, field, check box or drop-down.
    pub fn set_caption(&mut self, id: &str, v: &str) {
        for p in &mut self.controls {
            match &mut p.control {
                Control::Button { id: i, label, .. }
                | Control::Input { id: i, label, .. }
                | Control::Check { id: i, label, .. }
                | Control::Combo { id: i, label, .. }
                    if i == id =>
                {
                    *label = v.to_string()
                }
                _ => {}
            }
        }
    }

    /// Change a label's text (one made with `.id()`).
    pub fn set_label(&mut self, id: &str, v: &str) {
        for p in &mut self.controls {
            if let Control::Label { id: i, text } = &mut p.control {
                if i == id {
                    *text = v.to_string();
                }
            }
        }
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
    pub fn content_rect(&self, i: usize) -> Rect {
        self.inner(i)
    }

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
                (cr.w >= 4 && cr.h >= 3).then(|| (cr, cr.inset(1)))
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
            Control::Input { id, value, cursor, fresh, .. } => {
                // Highlighted text: typing (or Backspace, Delete) replaces
                // it; moving the cursor keeps it.
                if std::mem::take(fresh) {
                    match ev.key {
                        Key::Char(_) if !ev.mods.ctrl && !ev.mods.alt => {
                            value.clear();
                            *cursor = 0;
                        }
                        Key::Backspace | Key::Delete => {
                            value.clear();
                            *cursor = 0;
                            return DialogResult::Changed(id.clone());
                        }
                        Key::Enter | Key::Tab | Key::BackTab | Key::Esc | Key::Up | Key::Down => *fresh = true,
                        _ => {}
                    }
                }
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
        if let Control::Input { id, value, cursor, fresh, .. } = &mut self.controls[self.focus].control {
            if std::mem::take(fresh) {
                value.clear();
                *cursor = 0;
            }
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
                        self.size = Some((w.max(self.min.0), h.max(self.min.1)));
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
                            // (A third click starts over: the list may have
                            // changed under the pointer.)
                            self.last_click = None;
                            return DialogResult::Activated(id.clone());
                        }
                        if changed {
                            return DialogResult::Changed(id.clone());
                        }
                    }
                    return DialogResult::None;
                }
                Control::Input { value, cursor, fresh, .. } => {
                    *fresh = false;
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
        // (QBasic: a line across the box above the buttons.)
        for &y in &self.rules {
            let fc = st.dialog_frame;
            c.print(r.x, y, if fc.v == "║" { "╟" } else { "├" }, s.dialog_frame);
            c.hline(r.x + 1, y, r.w - 2, "─", s.dialog_frame);
            c.print(r.right() - 1, y, if fc.v == "║" { "╢" } else { "┤" }, s.dialog_frame);
        }
        // (No resize grip: DOS dialogs never had one; the edges still drag.)
        let boxed = st.controls.boxed();
        let boxes: Vec<Option<Rect>> = (0..self.controls.len()).map(|i| if boxed { self.box_of(i).map(|(outer, _)| outer) } else { None }).collect();
        let mut c = c.sub(r.inset(1));
        // The boxes: QBasic's in the frame's colour, VB-DOS's sunken (dark
        // top and left, white bottom and right).
        for b in boxes.iter().flatten() {
            let (lo, hi) = if st.controls.field == FieldLook::Sunken { (s.dialog.fg(vga::BLACK), s.dialog.fg(vga::WHITE)) } else { (s.dialog_frame, s.dialog_frame) };
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
            // (The layout leaves VB-DOS buttons room for their boxes.)
            let boxed_button = st.controls.button == ButtonLook::Box3d && matches!(self.controls[i].control, Control::Button { .. });
            let rows = if matches!(self.controls[i].control, Control::List { .. }) { self.list_rows(i) } else { 1 };
            match &mut self.controls[i].control {
                Control::Label { text, .. } => {
                    let mut lc = c.sub(cr);
                    for (k, line) in text.split('\n').enumerate() {
                        lc.print(cr.x, cr.y + k as i32, line, s.dialog);
                    }
                }
                Control::Group { title } if cr.h == 1 => {
                    // A list's heading: centred over a boxed list (QBasic,
                    // VB-DOS), at the left over a plain one (Turbo Vision).
                    let tw = str_width(&title.replace('~', "")) as i32;
                    let x = if st.controls.boxed() { cr.x + (cr.w - tw) / 2 } else { cr.x };
                    c.print_hot(x, cr.y, title, s.dialog, s.label_hotkey);
                }
                Control::Group { title } => {
                    draw_frame(&mut c, cr, crate::theme::SINGLE, s.dialog_frame);
                    if !title.is_empty() {
                        c.print_hot(cr.x + 1, cr.y, title, s.dialog, s.label_hotkey);
                    }
                }
                Control::Input { label, value, cursor, fresh, .. } => {
                    let fresh = *fresh && focused && !value.is_empty();
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
                    sub.print(cr.x - skip + 1, cr.y, value, if fresh { s.menu_selected } else { s.input });
                    if focused {
                        sub.set_cursor(Point::new(cr.x + 1 + before - skip, cr.y), CursorShape::Underline);
                    }
                }
                Control::Button { label, default, .. } => {
                    let (l, rr) = match st.controls.button {
                        ButtonLook::Brackets(l, r) => (l, r),
                        _ => ("", ""),
                    };
                    let lw = str_width(&label.replace('~', "")) as i32;
                    let lx = cr.x + (cr.w - lw) / 2;
                    if boxed_button {
                        // VB-DOS: a 3-D box lit from the top left (white top
                        // and left, black bottom and right); the default or
                        // focused one in double lines.
                        let hi = s.dialog.fg(vga::WHITE);
                        let lo = s.dialog.fg(vga::BLACK);
                        let (x0, x1) = (cr.x - 1, cr.right());
                        let d = *default || focused;
                        let (tl, h, tr, v, bl, br) = if d { ("╔", "═", "╗", "║", "╚", "╝") } else { ("┌", "─", "┐", "│", "└", "┘") };
                        c.print(x0, cr.y - 1, tl, hi);
                        c.hline(x0 + 1, cr.y - 1, x1 - x0 - 1, h, hi);
                        c.print(x1, cr.y - 1, tr, lo);
                        c.print(x0, cr.y, v, hi);
                        c.print(x1, cr.y, v, lo);
                        c.print(x0, cr.y + 1, bl, hi);
                        c.hline(x0 + 1, cr.y + 1, x1 - x0 - 1, h, lo);
                        c.print(x1, cr.y + 1, br, lo);
                        c.fill(cr, " ", s.button);
                        // (The focused button's label in reverse, as VB-DOS.)
                        let label_st = if focused { s.list_selected } else { s.button };
                        c.print_hot(lx, cr.y, label, label_st, s.button_hotkey);
                        if focused {
                            c.set_cursor(Point::new(lx, cr.y), CursorShape::Underline);
                        }
                    } else if !l.is_empty() {
                        // "< OK >": the focused button's brackets bright (QBasic).
                        let br = if focused { s.button_focused } else { s.button };
                        let x0 = cr.x + (cr.w - lw - str_width(l) as i32 - str_width(rr) as i32) / 2;
                        let x = c.print(x0, cr.y, l, br);
                        let x = c.print_hot(x, cr.y, label, s.button, s.button_hotkey);
                        c.print(x, cr.y, rr, br);
                        if focused {
                            c.set_cursor(Point::new(x0 + str_width(l) as i32, cr.y), CursorShape::Underline);
                        }
                    } else {
                        let stl = if focused { s.button_focused } else if *default { s.button_default } else { s.button };
                        if matches!(st.controls.button, ButtonLook::Bar { .. }) {
                            // Turbo Vision: a bar the button's width, its
                            // label centred, a half-block shadow right and below.
                            c.fill(cr, " ", stl);
                            c.print_hot(lx, cr.y, label, stl, s.button_hotkey);
                            c.print(cr.right(), cr.y, "▄", s.dialog.fg(vga::BLACK));
                            c.hline(cr.x + 1, cr.y + 1, cr.w, "▀", s.dialog.fg(vga::BLACK));
                        } else {
                            let text = format!("{l}{label}{rr}");
                            let tw = str_width(&text.replace('~', "")) as i32;
                            c.print_hot(cr.x + (cr.w - tw) / 2, cr.y, &text, stl, s.button_hotkey);
                        }
                        if focused {
                            c.set_cursor(Point::new(lx, cr.y), CursorShape::Underline);
                        }
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
                    let tv = !boxed && st.controls.combo_arrow_beside;
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
                        draw_vscroll(&mut c, outer.right() - 1, y0, rows as i32, (*scroll).min(max), rows, items.len(), s.scrollbar, st.scroll);
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
            draw_vscroll(c, pr.right() - 1, pr.y + 1, rows as i32, top, rows, items.len(), s.scrollbar, theme.style.scroll);
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
        Dialog::new("t", "Test")
            .body(col(vec![
                input("name", "~N~ame:", ""),
                list("files", (0..20).map(|i| format!("file{i}")).collect(), 0).header("Name").activates().lines(5),
                check("hidden", "~H~idden", false),
            ]))
            .buttons(vec![button("ok", "O~K~", true), button("cancel", "Cancel", false)])
    }

    fn themes() -> Vec<Theme> {
        use crate::theme::Scheme;
        vec![
            Theme::default(),
            Theme { scheme: Scheme::qbasic(), style: ControlStyle::qbasic() },
            Theme { scheme: Scheme::vbdos(), style: ControlStyle::vbdos() },
            Theme { scheme: Scheme::turbo(), style: ControlStyle::ascii() },
        ]
    }

    /// The cells a control takes, its label, box and shadow included.
    fn footprint(d: &Dialog, i: usize, st: &ControlStyle) -> Rect {
        let r = d.control_rect(i);
        match &d.controls[i].control {
            Control::Input { label, .. } | Control::Combo { label, .. } if st.controls.boxed() || !label.is_empty() => {
                Rect::new(r.x, r.y - 1, r.w, if st.controls.boxed() { 3 } else { 2 })
            }
            Control::Button { .. } if st.controls.button == ButtonLook::Box3d => Rect::new(r.x - 1, r.y - 1, r.w + 2, 3),
            Control::Button { .. } if matches!(st.controls.button, ButtonLook::Bar { .. }) => Rect::new(r.x, r.y, r.w + 1, 2),
            _ => r,
        }
    }

    #[test]
    fn every_style_fits_its_controls_without_overlap() {
        for theme in themes() {
            for size in [None, Some((90, 36)), Some((10, 5))] {
                let mut d = dlg();
                d.size = size;
                d.layout(Rect::new(0, 0, 100, 40), &theme);
                let st = &theme.style;
                let fp: Vec<Rect> = (0..d.controls.len()).map(|i| footprint(&d, i, st)).collect();
                for (i, a) in fp.iter().enumerate() {
                    assert!(d.rect.inset(1).contains_rect(*a), "{}: {i} {a:?} outside {:?}", st.name, d.rect);
                    for (j, b) in fp.iter().enumerate().skip(i + 1) {
                        assert!(a.intersect(b).is_empty(), "{}: {i} {a:?} overlaps {j} {b:?}", st.name);
                    }
                }
            }
        }
    }

    #[test]
    fn the_list_takes_the_room_and_buttons_go_where_the_style_puts_them() {
        let screen = Rect::new(0, 0, 100, 40);
        let [tv, qb, vb, _] = <[Theme; 4]>::try_from(themes()).ok().unwrap();
        // Turbo Vision: a column at the right; the list grows with the dialog.
        let mut d = dlg();
        d.layout(screen, &tv);
        let (list0, ok0) = (d.control_rect(1), d.control_rect(3));
        assert!(ok0.x > list0.right() && ok0.y <= list0.y + 1, "OK beside the list: {ok0:?} {list0:?}");
        d.size = Some((d.rect.w + 20, d.rect.h + 10));
        d.layout(screen, &tv);
        let list1 = d.control_rect(1);
        assert_eq!((list1.w - list0.w, list1.h - list0.h), (20, 10), "the list takes the new room");
        // QBasic: buttons in a row along the bottom, under a line.
        let mut d = dlg();
        d.layout(screen, &qb);
        let (ok, cancel) = (d.control_rect(3), d.control_rect(4));
        assert_eq!(ok.y, cancel.y);
        assert_eq!(ok.y, d.rect.bottom() - 2, "the buttons sit on the bottom edge");
        assert_eq!(d.rules, vec![ok.y - 1]);
        // VB-DOS: boxed buttons need three rows each.
        let mut d = dlg();
        d.layout(screen, &vb);
        assert!(d.control_rect(4).y - d.control_rect(3).y >= 3);
    }

    #[test]
    fn resize_by_dragging_the_corner() {
        let theme = Theme::default();
        let mut d = dlg();
        d.layout(Rect::new(0, 0, 100, 40), &theme);
        let r = d.rect;
        let corner = Point::new(r.right() - 1, r.bottom() - 1);
        let m = |kind, pos| MouseEvent { kind, pos, mods: Mods::NONE };
        d.handle_mouse(&m(MouseKind::Down(Button::Left), corner));
        d.handle_mouse(&m(MouseKind::Drag(Button::Left), Point::new(corner.x + 10, corner.y + 5)));
        d.handle_mouse(&m(MouseKind::Up(Button::Left), corner));
        d.layout(Rect::new(0, 0, 100, 40), &theme);
        assert_eq!((d.rect.w, d.rect.h), (r.w + 10, r.h + 5));
    }

    #[test]
    fn a_given_value_is_replaced_by_typing_or_kept_by_moving() {
        let mk = || Dialog::new("t", "T").body(input("name", "Name:", "old.txt")).buttons(vec![button("ok", "OK", true)]);
        let mut d = mk();
        d.handle_key(&KeyEvent::plain(Key::Char('n')));
        assert_eq!(d.value("name").as_deref(), Some("n"));
        let mut d = mk();
        d.handle_key(&KeyEvent::plain(Key::End));
        d.handle_key(&KeyEvent::plain(Key::Char('x')));
        assert_eq!(d.value("name").as_deref(), Some("old.txtx"));
        let mut d = mk();
        d.handle_key(&KeyEvent::plain(Key::Backspace));
        assert_eq!(d.value("name").as_deref(), Some(""));
    }

    #[test]
    fn a_third_click_does_not_activate_again() {
        let mut d = dlg();
        d.layout(Rect::new(0, 0, 100, 40), &Theme::default());
        let i = d.controls.iter().position(|p| p.control.id() == Some("files")).unwrap();
        let ir = d.content_rect(i);
        let at = Point::new(ir.x + 2, ir.y + 2);
        let press = MouseEvent { kind: MouseKind::Down(Button::Left), pos: at, mods: Mods::NONE };
        assert_eq!(d.handle_mouse(&press), DialogResult::Changed("files".into()));
        assert_eq!(d.handle_mouse(&press), DialogResult::Activated("files".into()));
        assert_ne!(d.handle_mouse(&press), DialogResult::Activated("files".into()));
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
        d.layout(Rect::new(0, 0, 100, 40), &Theme::default());
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
