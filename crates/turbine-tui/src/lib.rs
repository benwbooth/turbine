//! Terminal front end: runs a Turbine [`App`] in any terminal.
//!
//! Uses the kitty keyboard protocol when the terminal has it, so
//! Ctrl+Shift+letter, Shift+Enter and friends arrive intact; SGR mouse with
//! drag and motion; bracketed paste; truecolor with a 16-colour fallback.

use crossterm::event::{
    self as ct, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::style::{Attribute, Color as CtColor, Print, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::{cursor, execute, queue, terminal};
use std::io::{self, Write};
use std::time::{Duration, Instant};
use turbine_ui::event::{Button, Event, Key, KeyEvent, KeyKind, Mods, MouseEvent, MouseKind};
use turbine_ui::theme::{Color, Style, vga};
use turbine_ui::{App, CursorShape, Point, Surface};

/// How many colours the terminal can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorDepth {
    TrueColor,
    Ansi16,
}

impl ColorDepth {
    pub fn detect() -> Self {
        match std::env::var("COLORTERM").as_deref() {
            Ok("truecolor") | Ok("24bit") => ColorDepth::TrueColor,
            _ if std::env::var("TERM").is_ok_and(|t| t.contains("direct") || t.contains("kitty") || t.contains("ghostty")) => {
                ColorDepth::TrueColor
            }
            _ => ColorDepth::Ansi16,
        }
    }
}

fn to_ct(c: Color, depth: ColorDepth) -> CtColor {
    if depth == ColorDepth::TrueColor {
        return CtColor::Rgb { r: c.0, g: c.1, b: c.2 };
    }
    // Nearest VGA colour.
    let i = vga::ALL
        .iter()
        .enumerate()
        .min_by_key(|(_, v)| {
            let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
            d(v.0, c.0) + d(v.1, c.1) + d(v.2, c.2)
        })
        .map(|(i, _)| i)
        .unwrap_or(0);
    [
        CtColor::Black,
        CtColor::DarkBlue,
        CtColor::DarkGreen,
        CtColor::DarkCyan,
        CtColor::DarkRed,
        CtColor::DarkMagenta,
        CtColor::DarkYellow,
        CtColor::Grey,
        CtColor::DarkGrey,
        CtColor::Blue,
        CtColor::Green,
        CtColor::Cyan,
        CtColor::Red,
        CtColor::Magenta,
        CtColor::Yellow,
        CtColor::White,
    ][i]
}

/// Puts the terminal into full-screen raw mode and restores it on drop.
pub struct Terminal {
    out: io::Stdout,
    kitty: bool,
    depth: ColorDepth,
    prev: Option<Surface>,
}

impl Terminal {
    pub fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, terminal::EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste, EnableFocusChange, cursor::Hide)?;
        let kitty = std::env::var("TURBINE_KITTY_KEYS").map(|v| v != "0").unwrap_or(true)
            && terminal::supports_keyboard_enhancement().unwrap_or(false);
        if kitty {
            execute!(
                out,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                )
            )?;
        }
        // Restore the terminal even if we panic.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore(kitty);
            hook(info);
        }));
        Ok(Terminal { out, kitty, depth: ColorDepth::detect(), prev: None })
    }

    pub fn kitty(&self) -> bool {
        self.kitty
    }

    pub fn size(&self) -> (i32, i32) {
        terminal::size().map(|(w, h)| (w as i32, h as i32)).unwrap_or((80, 25))
    }

    /// Draw the cells that changed since the last frame.
    pub fn draw(&mut self, s: &Surface) -> io::Result<()> {
        let full = self.prev.as_ref().is_none_or(|p| p.w != s.w || p.h != s.h);
        let out = &mut self.out;
        queue!(out, terminal::BeginSynchronizedUpdate, cursor::Hide)?;
        if full {
            queue!(out, terminal::Clear(terminal::ClearType::All))?;
        }
        let mut last: Option<Style> = None;
        let mut at: Option<(i32, i32)> = None;
        for y in 0..s.h {
            for x in 0..s.w {
                let c = s.get(x, y).unwrap();
                if c.is_continuation() {
                    continue;
                }
                if !full && self.prev.as_ref().and_then(|p| p.get(x, y)) == Some(c) {
                    // Also require the right half of a wide char unchanged.
                    let same_next = self.prev.as_ref().and_then(|p| p.get(x + 1, y)) == s.get(x + 1, y);
                    if same_next {
                        continue;
                    }
                }
                if at != Some((x, y)) {
                    queue!(out, cursor::MoveTo(x as u16, y as u16))?;
                }
                if last != Some(c.style) {
                    let st = c.style;
                    let (fg, bg) = if st.attrs.reverse { (st.bg, st.fg) } else { (st.fg, st.bg) };
                    queue!(
                        out,
                        SetAttribute(Attribute::Reset),
                        SetForegroundColor(to_ct(fg, self.depth)),
                        SetBackgroundColor(to_ct(bg, self.depth))
                    )?;
                    if st.attrs.bold {
                        queue!(out, SetAttribute(Attribute::Bold))?;
                    }
                    if st.attrs.underline {
                        queue!(out, SetAttribute(Attribute::Underlined))?;
                    }
                    if st.attrs.italic {
                        queue!(out, SetAttribute(Attribute::Italic))?;
                    }
                    last = Some(st);
                }
                queue!(out, Print(&c.text))?;
                let w = if s.get(x + 1, y).is_some_and(|n| n.is_continuation()) { 2 } else { 1 };
                at = Some((x + w, y));
            }
        }
        queue!(out, SetAttribute(Attribute::Reset))?;
        if let Some((p, shape)) = s.cursor {
            let style = match shape {
                CursorShape::Underline => cursor::SetCursorStyle::BlinkingUnderScore,
                CursorShape::Block => cursor::SetCursorStyle::BlinkingBlock,
                CursorShape::Bar => cursor::SetCursorStyle::BlinkingBar,
            };
            queue!(out, cursor::MoveTo(p.x as u16, p.y as u16), style, cursor::Show)?;
        }
        queue!(out, terminal::EndSynchronizedUpdate)?;
        out.flush()?;
        self.prev = Some(s.clone());
        Ok(())
    }

    /// Wait up to `timeout` for input; returns every event that arrived.
    pub fn poll(&mut self, timeout: Duration) -> io::Result<Vec<Event>> {
        let mut evs = Vec::new();
        if ct::poll(timeout)? {
            loop {
                if let Some(e) = convert(ct::read()?) {
                    evs.push(e);
                }
                if !ct::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        Ok(evs)
    }
}

fn restore(kitty: bool) {
    let mut out = io::stdout();
    if kitty {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        cursor::SetCursorStyle::DefaultUserShape,
        cursor::Show,
        DisableFocusChange,
        DisableBracketedPaste,
        DisableMouseCapture,
        terminal::LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}

impl Drop for Terminal {
    fn drop(&mut self) {
        restore(self.kitty);
    }
}

fn mods(m: ct::KeyModifiers) -> Mods {
    Mods {
        shift: m.contains(ct::KeyModifiers::SHIFT),
        ctrl: m.contains(ct::KeyModifiers::CONTROL),
        alt: m.contains(ct::KeyModifiers::ALT),
        meta: m.contains(ct::KeyModifiers::SUPER) || m.contains(ct::KeyModifiers::META),
    }
}

/// Translate a crossterm event.
pub fn convert(e: ct::Event) -> Option<Event> {
    Some(match e {
        ct::Event::Key(k) => {
            let kind = match k.kind {
                ct::KeyEventKind::Press => KeyKind::Press,
                ct::KeyEventKind::Repeat => KeyKind::Repeat,
                ct::KeyEventKind::Release => return None,
            };
            let mut m = mods(k.modifiers);
            let key = match k.code {
                ct::KeyCode::Char(c) => {
                    // Kitty reports Shift+a as 'a' + SHIFT in some modes.
                    let c = if m.shift && c.is_lowercase() { c.to_uppercase().next().unwrap_or(c) } else { c };
                    if !m.ctrl && !m.alt && !c.is_alphabetic() {
                        m.shift = false;
                    }
                    Key::Char(c)
                }
                ct::KeyCode::Enter => Key::Enter,
                ct::KeyCode::Esc => Key::Esc,
                ct::KeyCode::Backspace => Key::Backspace,
                ct::KeyCode::Tab if m.shift => {
                    m.shift = false;
                    Key::BackTab
                }
                ct::KeyCode::Tab => Key::Tab,
                ct::KeyCode::BackTab => {
                    m.shift = false;
                    Key::BackTab
                }
                ct::KeyCode::Up => Key::Up,
                ct::KeyCode::Down => Key::Down,
                ct::KeyCode::Left => Key::Left,
                ct::KeyCode::Right => Key::Right,
                ct::KeyCode::Home => Key::Home,
                ct::KeyCode::End => Key::End,
                ct::KeyCode::PageUp => Key::PageUp,
                ct::KeyCode::PageDown => Key::PageDown,
                ct::KeyCode::Insert => Key::Insert,
                ct::KeyCode::Delete => Key::Delete,
                ct::KeyCode::F(n) => Key::F(n),
                _ => return None,
            };
            Event::Key(KeyEvent { key, mods: m, kind })
        }
        ct::Event::Mouse(me) => {
            let b = |b: ct::MouseButton| match b {
                ct::MouseButton::Left => Button::Left,
                ct::MouseButton::Right => Button::Right,
                ct::MouseButton::Middle => Button::Middle,
            };
            let kind = match me.kind {
                ct::MouseEventKind::Down(x) => MouseKind::Down(b(x)),
                ct::MouseEventKind::Up(x) => MouseKind::Up(b(x)),
                ct::MouseEventKind::Drag(x) => MouseKind::Drag(b(x)),
                ct::MouseEventKind::Moved => MouseKind::Moved,
                ct::MouseEventKind::ScrollUp => MouseKind::ScrollUp,
                ct::MouseEventKind::ScrollDown => MouseKind::ScrollDown,
                _ => return None,
            };
            Event::Mouse(MouseEvent { kind, pos: Point::new(me.column as i32, me.row as i32), mods: mods(me.modifiers) })
        }
        ct::Event::Paste(s) => Event::Paste(s),
        ct::Event::Resize(w, h) => Event::Resize(w as i32, h as i32),
        ct::Event::FocusGained => Event::Focus(true),
        ct::Event::FocusLost => Event::Focus(false),
    })
}

/// Run an app until it quits.
pub fn run(app: &mut dyn App) -> io::Result<()> {
    let mut term = Terminal::enter()?;
    let (w, h) = term.size();
    app.event(Event::Resize(w, h));
    let mut surface = Surface::new(w, h);
    let mut next_tick: Option<Instant> = None;
    loop {
        let now = Instant::now();
        let wait = next_tick.map(|t| t.saturating_duration_since(now)).unwrap_or(Duration::from_millis(250));
        for ev in term.poll(wait)? {
            if let Event::Resize(w, h) = ev {
                surface = Surface::new(w, h);
            }
            app.event(ev);
        }
        next_tick = app.tick(Instant::now());
        if app.should_quit() {
            return Ok(());
        }
        surface.cursor = None;
        app.render(&mut surface);
        term.draw(&surface)?;
    }
}
