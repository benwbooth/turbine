//! The terminal drawn as a Turbine view.

use crate::terminal::{CellStyle, CursorShape as TermCursor, KeyInput, MouseAct, MouseOutcome, Spawn, Terminal};
use libghostty_vt::key::{Key as GKey, Mods as GMods};
use turbine_ui::event::{Button, Key, KeyEvent, KeyKind, Mods, MouseEvent, MouseKind};
use turbine_ui::theme::{Attrs, Color, Style, Theme};
use turbine_ui::{Canvas, CursorShape, Point, Rect};

pub struct TerminalView {
    term: Result<Terminal, String>,
    spawn: Spawn,
    area: Rect,
}

impl TerminalView {
    /// Start a terminal; it sizes itself to the view on first draw.
    pub fn new(spawn: Spawn) -> Self {
        let term = Terminal::spawn(&spawn, 80, 24);
        TerminalView { term, spawn, area: Rect::default() }
    }

    pub fn terminal(&mut self) -> Option<&mut Terminal> {
        self.term.as_mut().ok()
    }

    pub fn error(&self) -> Option<&str> {
        self.term.as_ref().err().map(String::as_str)
    }

    /// Start the program again (after it exited).
    pub fn restart(&mut self) {
        let (c, r) = (self.area.w.max(2) as u16, self.area.h.max(1) as u16);
        self.term = Terminal::spawn(&self.spawn, c, r);
    }

    pub fn title(&self) -> Option<String> {
        self.term.as_ref().ok().and_then(|t| t.title())
    }

    /// Read the program's output. True if the screen changed.
    pub fn pump(&mut self) -> bool {
        self.term.as_mut().map(|t| t.pump()).unwrap_or(false)
    }

    pub fn screen_text(&mut self) -> String {
        match &mut self.term {
            Ok(t) => t.screen_text(),
            Err(e) => e.clone(),
        }
    }

    pub fn paste(&mut self, text: &str) {
        if let Ok(t) = &mut self.term {
            t.paste(text);
        }
    }

    pub fn handle_key(&mut self, ev: &KeyEvent) -> bool {
        if ev.kind == KeyKind::Release {
            return false;
        }
        let Ok(t) = &mut self.term else { return false };
        if t.exited().is_some() {
            // Enter starts it again, like a closed DOS box reopening.
            if ev.key == Key::Enter {
                self.restart();
            }
            return true;
        }
        t.key(&key_input(ev));
        true
    }

    /// A mouse event in screen coordinates (the view knows where it is).
    pub fn handle_mouse(&mut self, ev: &MouseEvent) -> MouseOutcome {
        let Ok(t) = &mut self.term else { return MouseOutcome::Ignored };
        let b = |b: Button| match b {
            Button::Left => 1,
            Button::Right => 2,
            Button::Middle => 3,
        };
        let act = match ev.kind {
            MouseKind::Down(x) => MouseAct::Press(b(x)),
            MouseKind::Up(x) => MouseAct::Release(b(x)),
            MouseKind::Drag(_) | MouseKind::Moved => MouseAct::Motion,
            MouseKind::ScrollUp => MouseAct::Wheel { up: true },
            MouseKind::ScrollDown => MouseAct::Wheel { up: false },
            // (Terminals have no sideways wheel to pass on.)
            MouseKind::ScrollLeft | MouseKind::ScrollRight => return MouseOutcome::Ignored,
        };
        t.mouse(act, ev.pos.x - self.area.x, ev.pos.y - self.area.y, gmods(ev.mods))
    }

    pub fn selected_text(&self) -> Option<String> {
        self.term.as_ref().ok().filter(|t| t.has_selection()).map(|t| t.selected_text().to_string())
    }

    pub fn select_all(&mut self) {
        if let Ok(t) = &mut self.term {
            t.select_all();
        }
    }

    pub fn clear_selection(&mut self) {
        if let Ok(t) = &mut self.term {
            t.clear_selection();
        }
    }

    /// Place the view (and size the terminal) without drawing.
    pub fn layout(&mut self, area: Rect) {
        self.area = area;
        if let Ok(t) = &mut self.term {
            t.resize(area.w.max(2) as u16, area.h.max(1) as u16);
        }
    }

    /// Where the view is on screen.
    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn draw(&mut self, c: &mut Canvas, area: Rect, theme: &Theme, focused: bool) {
        self.area = area;
        let base = theme.scheme.terminal;
        c.fill(area, " ", base);
        let t = match &mut self.term {
            Ok(t) => t,
            Err(e) => {
                c.print(area.x + 1, area.y, &format!("Can't start the terminal: {e}"), base);
                return;
            }
        };
        t.resize(area.w.max(2) as u16, area.h.max(1) as u16);
        let mut c = c.sub(area);
        let sel = theme.scheme.editor_selection;
        let cursor = t.render(|x, y, g, _w, st| {
            let s = if st.selected { sel } else { style(st, base) };
            c.print(area.x + x as i32, area.y + y as i32, g, s);
        });
        if focused {
            if let Some((x, y, shape)) = cursor {
                let shape = match shape {
                    TermCursor::Bar => CursorShape::Bar,
                    TermCursor::Underline => CursorShape::Underline,
                    TermCursor::Block | TermCursor::Hollow => CursorShape::Block,
                };
                c.set_cursor(Point::new(area.x + x as i32, area.y + y as i32), shape);
            }
        }
    }
}

fn style(st: CellStyle, base: Style) -> Style {
    let c = |rgb: Option<(u8, u8, u8)>, d: Color| rgb.map(|(r, g, b)| Color(r, g, b)).unwrap_or(d);
    let mut fg = c(st.fg, base.fg);
    let mut bg = c(st.bg, base.bg);
    if st.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if st.faint {
        fg = Color(((fg.0 as u16 + bg.0 as u16) / 2) as u8, ((fg.1 as u16 + bg.1 as u16) / 2) as u8, ((fg.2 as u16 + bg.2 as u16) / 2) as u8);
    }
    Style { fg, bg, attrs: Attrs { bold: st.bold, italic: st.italic, underline: st.underline, reverse: false } }
}

/// US-layout key for a character: (key, unshifted character).
fn char_key(c: char) -> (GKey, Option<char>) {
    let lower = c.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        let k = [
            GKey::A, GKey::B, GKey::C, GKey::D, GKey::E, GKey::F, GKey::G, GKey::H, GKey::I, GKey::J, GKey::K,
            GKey::L, GKey::M, GKey::N, GKey::O, GKey::P, GKey::Q, GKey::R, GKey::S, GKey::T, GKey::U, GKey::V,
            GKey::W, GKey::X, GKey::Y, GKey::Z,
        ][(lower as u8 - b'a') as usize];
        return (k, Some(lower));
    }
    let digits = [
        GKey::Digit0, GKey::Digit1, GKey::Digit2, GKey::Digit3, GKey::Digit4, GKey::Digit5, GKey::Digit6,
        GKey::Digit7, GKey::Digit8, GKey::Digit9,
    ];
    let (k, u) = match c {
        '0'..='9' => (digits[(c as u8 - b'0') as usize], c),
        ')' => (GKey::Digit0, '0'),
        '!' => (GKey::Digit1, '1'),
        '@' => (GKey::Digit2, '2'),
        '#' => (GKey::Digit3, '3'),
        '$' => (GKey::Digit4, '4'),
        '%' => (GKey::Digit5, '5'),
        '^' => (GKey::Digit6, '6'),
        '&' => (GKey::Digit7, '7'),
        '*' => (GKey::Digit8, '8'),
        '(' => (GKey::Digit9, '9'),
        ' ' => (GKey::Space, ' '),
        '`' | '~' => (GKey::Backquote, '`'),
        '-' | '_' => (GKey::Minus, '-'),
        '=' | '+' => (GKey::Equal, '='),
        '[' | '{' => (GKey::BracketLeft, '['),
        ']' | '}' => (GKey::BracketRight, ']'),
        '\\' | '|' => (GKey::Backslash, '\\'),
        ';' | ':' => (GKey::Semicolon, ';'),
        '\'' | '"' => (GKey::Quote, '\''),
        ',' | '<' => (GKey::Comma, ','),
        '.' | '>' => (GKey::Period, '.'),
        '/' | '?' => (GKey::Slash, '/'),
        _ => return (GKey::Unidentified, None),
    };
    (k, Some(u))
}

fn gmods(m: Mods) -> GMods {
    let mut mods = GMods::empty();
    if m.shift {
        mods |= GMods::SHIFT;
    }
    if m.ctrl {
        mods |= GMods::CTRL;
    }
    if m.alt {
        mods |= GMods::ALT;
    }
    if m.meta {
        mods |= GMods::SUPER;
    }
    mods
}

pub fn key_input(ev: &KeyEvent) -> KeyInput {
    let mut mods = gmods(ev.mods);
    let special = |k: GKey| KeyInput { key: k, mods, text: None, unshifted: None };
    match ev.key {
        Key::Char(c) => {
            let (k, unshifted) = char_key(c);
            // Shifted characters carry Shift; the encoder treats it as used up.
            if c.is_uppercase() || "~!@#$%^&*()_+{}|:\"<>?".contains(c) {
                mods |= GMods::SHIFT;
            }
            let text = if ev.mods.ctrl { None } else { Some(c.to_string()) };
            KeyInput { key: k, mods, text, unshifted }
        }
        Key::Enter => special(GKey::Enter),
        Key::Esc => special(GKey::Escape),
        Key::Backspace => special(GKey::Backspace),
        Key::Tab => special(GKey::Tab),
        Key::BackTab => {
            mods |= GMods::SHIFT;
            KeyInput { key: GKey::Tab, mods, text: None, unshifted: None }
        }
        Key::Up => special(GKey::ArrowUp),
        Key::Down => special(GKey::ArrowDown),
        Key::Left => special(GKey::ArrowLeft),
        Key::Right => special(GKey::ArrowRight),
        Key::Home => special(GKey::Home),
        Key::End => special(GKey::End),
        Key::PageUp => special(GKey::PageUp),
        Key::PageDown => special(GKey::PageDown),
        Key::Insert => special(GKey::Insert),
        Key::Delete => special(GKey::Delete),
        Key::F(n) => special(match n {
            1 => GKey::F1,
            2 => GKey::F2,
            3 => GKey::F3,
            4 => GKey::F4,
            5 => GKey::F5,
            6 => GKey::F6,
            7 => GKey::F7,
            8 => GKey::F8,
            9 => GKey::F9,
            10 => GKey::F10,
            11 => GKey::F11,
            12 => GKey::F12,
            _ => GKey::Unidentified,
        }),
    }
}
