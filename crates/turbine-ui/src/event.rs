//! Input events, shared by every front end and by the automation driver.

use crate::geom::Point;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { shift: false, ctrl: false, alt: false, meta: false };
    pub const CTRL: Mods = Mods { shift: false, ctrl: true, alt: false, meta: false };
    pub const ALT: Mods = Mods { shift: false, ctrl: false, alt: true, meta: false };
    pub const SHIFT: Mods = Mods { shift: true, ctrl: false, alt: false, meta: false };
    pub const fn with_shift(mut self) -> Mods {
        self.shift = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum KeyKind {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Mods,
    pub kind: KeyKind,
}

impl KeyEvent {
    pub fn new(key: Key, mods: Mods) -> Self {
        KeyEvent { key, mods, kind: KeyKind::Press }
    }
    pub fn plain(key: Key) -> Self {
        Self::new(key, Mods::NONE)
    }
    /// True for a press of `key` with exactly `mods`.
    pub fn is(&self, key: Key, mods: Mods) -> bool {
        self.kind != KeyKind::Release && self.key == key && self.mods == mods
    }
    /// Ctrl+letter, case-insensitive.
    pub fn is_ctrl(&self, c: char) -> bool {
        self.kind != KeyKind::Release
            && self.mods.ctrl
            && !self.mods.alt
            && matches!(self.key, Key::Char(k) if k.eq_ignore_ascii_case(&c))
    }
    /// Alt+letter, case-insensitive.
    pub fn is_alt(&self, c: char) -> bool {
        self.kind != KeyKind::Release
            && self.mods.alt
            && !self.mods.ctrl
            && matches!(self.key, Key::Char(k) if k.eq_ignore_ascii_case(&c))
    }

    /// Parse a chord such as "Ctrl+S", "Alt+F", "F10", "Shift+Tab", "a".
    pub fn parse(chord: &str) -> Option<KeyEvent> {
        let mut mods = Mods::NONE;
        let parts: Vec<&str> = chord.split('+').collect();
        let (last, rest) = parts.split_last()?;
        for m in rest {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "c" => mods.ctrl = true,
                "alt" | "a" | "option" => mods.alt = true,
                "shift" | "s" => mods.shift = true,
                "meta" | "super" | "cmd" | "win" => mods.meta = true,
                _ => return None,
            }
        }
        let l = last.to_ascii_lowercase();
        let key = match l.as_str() {
            "enter" | "return" => Key::Enter,
            "esc" | "escape" => Key::Esc,
            "backspace" | "bs" => Key::Backspace,
            "tab" if mods.shift => {
                mods.shift = false;
                Key::BackTab
            }
            "tab" => Key::Tab,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" | "pgup" => Key::PageUp,
            "pagedown" | "pgdn" => Key::PageDown,
            "insert" | "ins" => Key::Insert,
            "delete" | "del" => Key::Delete,
            "space" => Key::Char(' '),
            f if f.starts_with('f') && f.len() > 1 && f[1..].parse::<u8>().is_ok() => Key::F(f[1..].parse().ok()?),
            _ => {
                let mut cs = last.chars();
                let c = cs.next()?;
                if cs.next().is_some() {
                    return None;
                }
                Key::Char(c)
            }
        };
        Some(KeyEvent::new(key, mods))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum MouseKind {
    Down(Button),
    Up(Button),
    Drag(Button),
    Moved,
    ScrollUp,
    ScrollDown,
    /// A trackpad's (or tilting wheel's) sideways scroll.
    ScrollLeft,
    ScrollRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub pos: Point,
    pub mods: Mods,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Resize(i32, i32),
    Focus(bool),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_chords() {
        assert_eq!(KeyEvent::parse("Ctrl+S"), Some(KeyEvent::new(Key::Char('S'), Mods::CTRL)));
        assert_eq!(KeyEvent::parse("F10"), Some(KeyEvent::plain(Key::F(10))));
        assert_eq!(KeyEvent::parse("Shift+Tab"), Some(KeyEvent::plain(Key::BackTab)));
        assert_eq!(KeyEvent::parse("+"), None);
    }
}
