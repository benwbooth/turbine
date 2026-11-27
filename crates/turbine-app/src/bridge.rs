//! The Rust side of the desktop app: the same workbench the terminal front
//! end runs, driven by the C++ cell view (cpp/cell_view.cpp), which paints
//! the cell grid with QPainter in native fonts.
//!
//! All of it lives on Qt's GUI thread (a thread-local), so the C++ side calls
//! plain functions.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;
use turbine::workbench::{Request, Workbench};
use turbine_ui::event::{Button, Event, Key, KeyEvent, KeyKind, Mods, MouseEvent, MouseKind};
use turbine_ui::{App, Point, Surface};

#[cxx_qt::bridge]
pub mod ffi {
    #[namespace = "turbine"]
    /// One cell to paint.
    struct FrameCell {
        text: String,
        /// 0xRRGGBB
        fg: u32,
        bg: u32,
        /// 1 bold, 2 italic, 4 underline, 8 reverse, 16 wide (two cells),
        /// 32 continuation (the right half of a wide cell: skip it).
        flags: u8,
    }

    #[namespace = "turbine"]
    struct Frame {
        cols: i32,
        rows: i32,
        cells: Vec<FrameCell>,
        cursor_x: i32,
        cursor_y: i32,
        /// -1 hidden, 0 underline, 1 block, 2 bar.
        cursor_shape: i32,
    }

    extern "Rust" {
        #[namespace = "turbine"]
        fn app_resize(cols: i32, rows: i32);
        /// A key from Qt (key code, modifiers, text). True when used.
        #[namespace = "turbine"]
        fn app_key(key: i32, mods: u32, text: &str) -> bool;
        /// kind: 0 press, 1 release, 2 drag, 3 move, 4 wheel up, 5 wheel down;
        /// button: 1 left, 2 right, 3 middle.
        #[namespace = "turbine"]
        fn app_mouse(kind: i32, button: i32, col: i32, row: i32, mods: u32);
        #[namespace = "turbine"]
        fn app_paste(text: &str);
        /// The desktop clipboard's text, before keys that may paste.
        #[namespace = "turbine"]
        fn app_set_clipboard(text: &str);
        /// Advance time. Returns ms until the next tick, or -1 to quit.
        #[namespace = "turbine"]
        fn app_tick() -> i32;
        /// The screen changed since the last frame.
        #[namespace = "turbine"]
        fn app_dirty() -> bool;
        #[namespace = "turbine"]
        fn app_frame() -> Frame;
        /// Text Turbine copied, for the desktop clipboard ("" if none).
        #[namespace = "turbine"]
        fn app_take_clipboard() -> String;
        #[namespace = "turbine"]
        fn app_title() -> String;
        /// The menus as JSON [{title, items: [{label, shortcut, command, enabled, separator}]}].
        #[namespace = "turbine"]
        fn app_menus_json() -> String;
        #[namespace = "turbine"]
        fn app_command(cmd: &str);
    }

    unsafe extern "C++" {
        include!("bootstrap.h");
        /// QApplication (native menus), native text rendering, CellView registered.
        #[namespace = "turbine"]
        fn create_app();
        #[namespace = "turbine"]
        fn exec_app() -> i32;
    }
}

struct State {
    wb: Workbench,
    surf: Surface,
    shown: Option<Surface>,
    dirty: bool,
    socket: Option<PathBuf>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Set up the workbench (files to open, automation socket, AI).
pub fn init(files: &[PathBuf]) {
    let mut wb = Workbench::new(files);
    let (tx, rx) = mpsc::channel::<Request>();
    wb.set_requests(rx);
    let socket = turbine::server::listen(None, tx).ok();
    if let Some(p) = &socket {
        wb.set_socket_path(p.clone());
    }
    wb.start_ai();
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))) {
        wb.set_look_path(dir.join("turbine/look.json"));
    }
    STATE.with(|s| {
        *s.borrow_mut() = Some(State { wb, surf: Surface::new(80, 25), shown: None, dirty: true, socket });
    });
}

pub fn shutdown() {
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().take() {
            if let Some(p) = st.socket {
                let _ = std::fs::remove_file(p);
            }
        }
    });
}

fn app_resize(cols: i32, rows: i32) {
    with(|st| {
        st.wb.event(Event::Resize(cols.max(20), rows.max(8)));
        st.dirty = true;
    });
}

const QT_SHIFT: u32 = 0x0200_0000;
const QT_CTRL: u32 = 0x0400_0000;
const QT_ALT: u32 = 0x0800_0000;
const QT_META: u32 = 0x1000_0000;

/// Qt's key codes and text to Turbine's keys.
fn qt_key(key: i32, mods: u32, text: &str) -> Option<KeyEvent> {
    let mut m = Mods { shift: mods & QT_SHIFT != 0, ctrl: mods & QT_CTRL != 0, alt: mods & QT_ALT != 0, meta: mods & QT_META != 0 };
    let k = match key as u32 {
        0x0100_0000 => Key::Esc,
        0x0100_0001 => Key::Tab,
        0x0100_0002 => {
            m.shift = false;
            Key::BackTab
        }
        0x0100_0003 => Key::Backspace,
        0x0100_0004 | 0x0100_0005 => Key::Enter,
        0x0100_0006 => Key::Insert,
        0x0100_0007 => Key::Delete,
        0x0100_0010 => Key::Home,
        0x0100_0011 => Key::End,
        0x0100_0012 => Key::Left,
        0x0100_0013 => Key::Up,
        0x0100_0014 => Key::Right,
        0x0100_0015 => Key::Down,
        0x0100_0016 => Key::PageUp,
        0x0100_0017 => Key::PageDown,
        f @ 0x0100_0030..=0x0100_003b => Key::F((f - 0x0100_0030 + 1) as u8),
        // Modifier keys alone, Caps Lock and the like.
        0x0100_0020..=0x0100_0026 => return None,
        _ => {
            let typed = text.chars().next().filter(|c| !c.is_control());
            let c = match typed {
                Some(c) if !m.ctrl && !m.alt => c,
                // With Ctrl/Alt the text is a control character: use the key.
                _ => {
                    let code = char::from_u32(key as u32).filter(|c| c.is_ascii_graphic() || *c == ' ')?;
                    if m.shift && code.is_ascii_alphabetic() { code } else { code.to_ascii_lowercase() }
                }
            };
            if !c.is_alphabetic() && !m.ctrl && !m.alt {
                m.shift = false;
            }
            Key::Char(c)
        }
    };
    Some(KeyEvent { key: k, mods: m, kind: KeyKind::Press })
}

fn app_key(key: i32, mods: u32, text: &str) -> bool {
    let Some(ev) = qt_key(key, mods, text) else { return false };
    with(|st| {
        st.wb.event(Event::Key(ev));
        true
    })
    .unwrap_or(false)
}

fn app_mouse(kind: i32, button: i32, col: i32, row: i32, mods: u32) {
    let b = match button {
        2 => Button::Right,
        3 => Button::Middle,
        _ => Button::Left,
    };
    let kind = match kind {
        0 => MouseKind::Down(b),
        1 => MouseKind::Up(b),
        2 => MouseKind::Drag(b),
        3 => MouseKind::Moved,
        4 => MouseKind::ScrollUp,
        _ => MouseKind::ScrollDown,
    };
    let m = Mods { shift: mods & QT_SHIFT != 0, ctrl: mods & QT_CTRL != 0, alt: mods & QT_ALT != 0, meta: mods & QT_META != 0 };
    with(|st| st.wb.event(Event::Mouse(MouseEvent { kind, pos: Point::new(col, row), mods: m })));
}

fn app_paste(text: &str) {
    with(|st| st.wb.event(Event::Paste(text.to_string())));
}

fn app_set_clipboard(text: &str) {
    with(|st| st.wb.set_clipboard(text.to_string()));
}

fn app_tick() -> i32 {
    with(|st| {
        let now = Instant::now();
        let next = st.wb.tick(now);
        if st.wb.should_quit() {
            return -1;
        }
        st.surf.cursor = None;
        st.wb.render(&mut st.surf);
        let changed = match &st.shown {
            Some(prev) => prev.cells != st.surf.cells || prev.cursor != st.surf.cursor || prev.w != st.surf.w,
            None => true,
        };
        if changed {
            st.dirty = true;
        }
        next.map(|t| t.saturating_duration_since(now).as_millis().clamp(1, 250) as i32).unwrap_or(250)
    })
    .unwrap_or(-1)
}

fn app_dirty() -> bool {
    with(|st| st.dirty).unwrap_or(false)
}

fn rgb(c: turbine_ui::theme::Color) -> u32 {
    (c.0 as u32) << 16 | (c.1 as u32) << 8 | c.2 as u32
}

fn app_frame() -> ffi::Frame {
    with(|st| {
        st.dirty = false;
        let s = &st.surf;
        let mut cells = Vec::with_capacity(s.cells.len());
        for y in 0..s.h {
            for x in 0..s.w {
                let c = s.get(x, y).unwrap();
                let a = c.style.attrs;
                let wide = s.get(x + 1, y).is_some_and(|n| n.is_continuation());
                let flags = (a.bold as u8) | (a.italic as u8) << 1 | (a.underline as u8) << 2 | (a.reverse as u8) << 3 | (wide as u8) << 4 | (c.is_continuation() as u8) << 5;
                cells.push(ffi::FrameCell { text: c.text.clone(), fg: rgb(c.style.fg), bg: rgb(c.style.bg), flags });
            }
        }
        let (cx, cy, shape) = match s.cursor {
            Some((p, shape)) => (p.x, p.y, match shape {
                turbine_ui::CursorShape::Underline => 0,
                turbine_ui::CursorShape::Block => 1,
                turbine_ui::CursorShape::Bar => 2,
            }),
            None => (0, 0, -1),
        };
        st.shown = Some(s.clone());
        ffi::Frame { cols: s.w, rows: s.h, cells, cursor_x: cx, cursor_y: cy, cursor_shape: shape }
    })
    .unwrap_or(ffi::Frame { cols: 0, rows: 0, cells: Vec::new(), cursor_x: 0, cursor_y: 0, cursor_shape: -1 })
}

fn app_take_clipboard() -> String {
    with(|st| st.wb.take_clipboard()).flatten().unwrap_or_default()
}

fn app_title() -> String {
    with(|st| {
        let t = st.wb.title();
        if t.is_empty() { "Turbine".to_string() } else { format!("{t} - Turbine") }
    })
    .unwrap_or_default()
}

fn app_menus_json() -> String {
    with(|st| {
        // GlobalMenuBar's model: ids unique across the whole bar.
        let menus: Vec<serde_json::Value> = st
            .wb
            .menus()
            .iter()
            .enumerate()
            .map(|(mi, m)| {
                serde_json::json!({
                    // Qt marks the mnemonic with &, Turbine with ~...~.
                    "title": qt_mnemonic(&m.title),
                    "items": m.items.iter().enumerate().map(|(ii, it)| serde_json::json!({
                        "id": format!("{mi}.{ii}:{}", it.command),
                        "text": qt_mnemonic(&it.label),
                        "shortcut": it.shortcut,
                        "enabled": it.enabled,
                        "separator": it.is_separator(),
                        "checkable": it.checked.is_some(),
                        "checked": it.checked == Some(true),
                    })).collect::<Vec<_>>()
                })
            })
            .collect();
        serde_json::Value::Array(menus).to_string()
    })
    .unwrap_or_else(|| "[]".into())
}

/// "~F~ile" → "&File"; a literal & doubled.
fn qt_mnemonic(label: &str) -> String {
    let mut out = String::new();
    let mut first = true;
    for (i, part) in label.replace('&', "&&").split('~').enumerate() {
        if i % 2 == 1 && first && !part.is_empty() {
            out.push('&');
            first = false;
        }
        out.push_str(part);
    }
    out
}

/// A command, or a menu item id ("menu.item:command") from the global menu.
fn app_command(cmd: &str) {
    let cmd = cmd.split_once(':').map(|(_, c)| c).unwrap_or(cmd);
    with(|st| st.wb.run_command(cmd));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qt_keys_map() {
        assert_eq!(qt_key(0x41, QT_CTRL, "\u{1}").unwrap(), KeyEvent::new(Key::Char('a'), Mods::CTRL));
        assert_eq!(qt_key(0x41, QT_SHIFT, "A").unwrap(), KeyEvent::new(Key::Char('A'), Mods::SHIFT));
        assert_eq!(qt_key(0x21, QT_SHIFT, "!").unwrap(), KeyEvent::plain(Key::Char('!')));
        assert_eq!(qt_key(0x0100_0039, 0, "").unwrap(), KeyEvent::plain(Key::F(10)));
        assert_eq!(qt_key(0x0100_0002, QT_SHIFT, "").unwrap(), KeyEvent::plain(Key::BackTab));
        assert!(qt_key(0x0100_0020, QT_SHIFT, "").is_none());
        assert_eq!(qt_key(0x46, QT_ALT, "f").unwrap(), KeyEvent::new(Key::Char('f'), Mods::ALT));
    }

    #[test]
    fn mnemonics() {
        assert_eq!(qt_mnemonic("~F~ile"), "&File");
        assert_eq!(qt_mnemonic("Save ~A~s..."), "Save &As...");
        assert_eq!(qt_mnemonic("R&D"), "R&&D");
        assert_eq!(qt_mnemonic("≡"), "≡");
    }
}
