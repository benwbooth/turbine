//! A terminal: Ghostty's engine (libghostty-vt) fed by a shell on a PTY.
//!
//! The engine's types are not `Send`, so a `Terminal` lives on the UI
//! thread. A reader thread moves the PTY's output over a channel; `pump`
//! feeds it to the engine, as often as the host ticks.

use libghostty_vt::{
    key,
    paste,
    render::{CellIterator, CursorVisualStyle, RenderState, RowIterator},
    screen::CellWide,
    style::{RgbColor, Underline},
    terminal::{Mode, Options, ScrollViewport},
    Terminal as VtTerminal,
};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub type Rgb = (u8, u8, u8);

/// How one cell is drawn. `None` colours mean "the terminal's default",
/// which the view takes from the colour scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellStyle {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub faint: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
    Hollow,
}

/// A key for the terminal, already split into what the encoder needs.
#[derive(Clone, Debug)]
pub struct KeyInput {
    pub key: key::Key,
    pub mods: key::Mods,
    /// The character the key types (None for Ctrl chords and special keys).
    pub text: Option<String>,
    /// The key's character without Shift (US layout), for the kitty protocol.
    pub unshifted: Option<char>,
}

/// What to run in the terminal.
#[derive(Clone, Debug, Default)]
pub struct Spawn {
    /// Program and arguments; empty = the user's shell.
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

/// The 16 colours as a DOS text screen shows them (CGA/VGA), so programs
/// in the terminal look like they're running on the same machine as the IDE.
pub const VGA_PALETTE: [Rgb; 16] = [
    (0x00, 0x00, 0x00),
    (0xAA, 0x00, 0x00),
    (0x00, 0xAA, 0x00),
    (0xAA, 0x55, 0x00),
    (0x00, 0x00, 0xAA),
    (0xAA, 0x00, 0xAA),
    (0x00, 0xAA, 0xAA),
    (0xAA, 0xAA, 0xAA),
    (0x55, 0x55, 0x55),
    (0xFF, 0x55, 0x55),
    (0x55, 0xFF, 0x55),
    (0xFF, 0xFF, 0x55),
    (0x55, 0x55, 0xFF),
    (0xFF, 0x55, 0xFF),
    (0x55, 0xFF, 0xFF),
    (0xFF, 0xFF, 0xFF),
];

pub struct Terminal {
    term: VtTerminal<'static, 'static>,
    rs: RenderState<'static>,
    rows_it: RowIterator<'static>,
    cells_it: CellIterator<'static>,
    key_enc: key::Encoder<'static>,
    key_ev: key::Event<'static>,
    /// What the engine wants written back to the program (query replies).
    outbox: Rc<RefCell<Vec<u8>>>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: Receiver<Vec<u8>>,
    cols: u16,
    rows: u16,
    exit: Option<String>,
    /// Engine replies are flushed after each pump; this is set when the
    /// screen changed since the last render.
    changed: bool,
}

fn rgb(c: RgbColor) -> Rgb {
    (c.r, c.g, c.b)
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The user's shell.
pub fn default_shell() -> Vec<String> {
    if cfg!(windows) {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
        return vec![comspec];
    }
    vec![std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into())]
}

impl Terminal {
    pub fn spawn(spawn: &Spawn, cols: u16, rows: u16) -> Result<Terminal, String> {
        let cols = cols.max(2);
        let rows = rows.max(1);
        // Scrollback is counted in bytes (~10 per cell): about 10,000 lines.
        let mut term = VtTerminal::new(Options { cols, rows, max_scrollback: 10_000 * 160 * 10 }).map_err(err)?;
        term.resize(cols, rows, 8, 16).map_err(err)?;
        let outbox: Rc<RefCell<Vec<u8>>> = Rc::default();
        {
            let outbox = outbox.clone();
            term.on_pty_write(move |_t, data| outbox.borrow_mut().extend_from_slice(data)).map_err(err)?;
        }
        term.on_xtversion(|_t| Some(concat!("Turbine ", env!("CARGO_PKG_VERSION")))).map_err(err)?;
        // DOS colours for the 16-colour palette (OSC 4).
        let mut osc = String::new();
        for (i, (r, g, b)) in VGA_PALETTE.iter().enumerate() {
            osc.push_str(&format!("\x1b]4;{i};rgb:{r:02x}/{g:02x}/{b:02x}\x1b\\"));
        }
        term.vt_write(osc.as_bytes());

        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows, cols, pixel_width: cols * 8, pixel_height: rows * 16 })
            .map_err(err)?;
        let argv = if spawn.argv.is_empty() { default_shell() } else { spawn.argv.clone() };
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "turbine");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        for (k, v) in &spawn.env {
            cmd.env(k, v);
        }
        if let Some(d) = spawn.cwd.clone().or_else(|| std::env::current_dir().ok()) {
            cmd.cwd(d);
        }
        let child = pair.slave.spawn_command(cmd).map_err(err)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(err)?;
        let writer = pair.master.take_writer().map_err(err)?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("turbine-pty".into())
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .map_err(err)?;

        Ok(Terminal {
            term,
            rs: RenderState::new().map_err(err)?,
            rows_it: RowIterator::new().map_err(err)?,
            cells_it: CellIterator::new().map_err(err)?,
            key_enc: key::Encoder::new().map_err(err)?,
            key_ev: key::Event::new().map_err(err)?,
            outbox,
            master: pair.master,
            writer,
            child,
            rx,
            cols,
            rows,
            exit: None,
            changed: true,
        })
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Feed everything the program wrote. Returns true if the screen changed.
    pub fn pump(&mut self) -> bool {
        let mut got = false;
        loop {
            match self.rx.try_recv() {
                Ok(bytes) => {
                    self.term.vt_write(&bytes);
                    got = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.exit.is_none() {
                        let status = self.child.try_wait().ok().flatten();
                        let msg = match status {
                            Some(s) if s.success() => "exited".to_string(),
                            Some(s) => format!("exited with code {}", s.exit_code()),
                            None => "closed".to_string(),
                        };
                        self.term.vt_write(format!("\r\n\x1b[0;7m[process {msg}]\x1b[0m").as_bytes());
                        self.exit = Some(msg);
                        got = true;
                    }
                    break;
                }
            }
        }
        self.flush_outbox();
        if got {
            self.changed = true;
        }
        got
    }

    fn flush_outbox(&mut self) {
        let out = std::mem::take(&mut *self.outbox.borrow_mut());
        if !out.is_empty() {
            self.write(&out);
        }
    }

    /// Bytes to the program.
    pub fn write(&mut self, bytes: &[u8]) {
        if self.exit.is_some() {
            return;
        }
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn exited(&self) -> Option<&str> {
        self.exit.as_deref()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let _ = self.term.resize(cols, rows, 8, 16);
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: cols * 8, pixel_height: rows * 16 });
        self.changed = true;
    }

    /// Encode a key the way the program asked (legacy, xterm modifyOtherKeys
    /// or the kitty keyboard protocol) and send it.
    pub fn key(&mut self, k: &KeyInput) {
        let mut consumed = key::Mods::empty();
        if k.text.is_some() && k.mods.contains(key::Mods::SHIFT) {
            consumed |= key::Mods::SHIFT;
        }
        self.key_ev
            .set_action(key::Action::Press)
            .set_key(k.key)
            .set_mods(k.mods)
            .set_consumed_mods(consumed)
            .set_utf8(k.text.clone());
        if let Some(u) = k.unshifted {
            self.key_ev.set_unshifted_codepoint(u);
        }
        let mut out = Vec::new();
        let _ = self.key_enc.set_options_from_terminal(&self.term).set_alt_esc_prefix(true).encode_to_vec(&self.key_ev, &mut out);
        if out.is_empty() {
            if let Some(t) = &k.text {
                out.extend_from_slice(t.as_bytes());
            }
        }
        if !out.is_empty() {
            self.term.scroll_viewport(ScrollViewport::Bottom);
            self.write(&out);
        }
    }

    /// Paste text, bracketed when the program asked for it.
    pub fn paste(&mut self, text: &str) {
        let bracketed = self.term.mode(Mode::BRACKETED_PASTE).unwrap_or(false);
        let mut data = text.replace("\r\n", "\r").replace('\n', "\r").into_bytes();
        let mut out = vec![0u8; data.len() + 16];
        let n = paste::encode(&mut data, bracketed, &mut out).unwrap_or(0);
        out.truncate(n);
        self.term.scroll_viewport(ScrollViewport::Bottom);
        self.write(&out);
    }

    pub fn scroll(&mut self, delta: isize) {
        self.term.scroll_viewport(ScrollViewport::Delta(delta));
        self.changed = true;
    }

    pub fn title(&self) -> Option<String> {
        self.term.title().ok().filter(|t| !t.is_empty()).map(str::to_string)
    }

    /// True once after the screen changes.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Visit every visible cell: (x, y, grapheme, width, style). Returns the
    /// cursor (x, y, shape) when it's shown.
    pub fn render(&mut self, mut f: impl FnMut(u16, u16, &str, u16, CellStyle)) -> Option<(u16, u16, CursorShape)> {
        let Ok(snap) = self.rs.update(&self.term) else { return None };
        let cursor = snap.cursor_viewport().ok().flatten();
        let visible = snap.cursor_visible().unwrap_or(false);
        let shape = match snap.cursor_visual_style() {
            Ok(CursorVisualStyle::Bar) => CursorShape::Bar,
            Ok(CursorVisualStyle::Underline) => CursorShape::Underline,
            Ok(CursorVisualStyle::BlockHollow) => CursorShape::Hollow,
            _ => CursorShape::Block,
        };
        let Ok(mut rows) = self.rows_it.update(&snap) else { return None };
        let mut y: u16 = 0;
        let mut text = String::new();
        while let Some(row) = rows.next() {
            if let Ok(mut cells) = self.cells_it.update(row) {
                let mut x: u16 = 0;
                while let Some(cell) = cells.next() {
                    let wide = cell.raw_cell().ok().and_then(|c| c.wide().ok()).unwrap_or(CellWide::Narrow);
                    if matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead) {
                        x += 1;
                        continue;
                    }
                    let width = if wide == CellWide::Wide { 2 } else { 1 };
                    text.clear();
                    if cell.graphemes_len().unwrap_or(0) > 0 {
                        let _ = cell.graphemes_utf8(&mut text);
                    }
                    if text.is_empty() {
                        text.push(' ');
                    }
                    let mut st = CellStyle {
                        fg: cell.fg_color().ok().flatten().map(rgb),
                        bg: cell.bg_color().ok().flatten().map(rgb),
                        ..Default::default()
                    };
                    if cell.has_styling().unwrap_or(false) {
                        if let Ok(s) = cell.style() {
                            st.bold = s.bold;
                            st.italic = s.italic;
                            st.faint = s.faint;
                            st.inverse = s.inverse;
                            st.underline = !matches!(s.underline, Underline::None);
                            if s.invisible {
                                text.clear();
                                text.push(' ');
                            }
                        }
                    }
                    f(x, y, &text, width, st);
                    // (A wide character's second column is its own spacer cell.)
                    x += 1;
                }
            }
            y += 1;
        }
        cursor.filter(|_| visible).map(|c| (c.x, c.y, shape))
    }

    /// The visible screen as text, one line per row (for agents and tests).
    pub fn screen_text(&mut self) -> String {
        let mut lines: Vec<String> = vec![String::new(); self.rows as usize];
        self.render(|_, y, g, _, _| {
            if let Some(l) = lines.get_mut(y as usize) {
                l.push_str(g);
            }
        });
        let mut s: String = lines.iter().map(|l| format!("{}\n", l.trim_end())).collect();
        while s.ends_with("\n\n") {
            s.pop();
        }
        s
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.exit.is_none() {
            let _ = self.child.kill();
        }
    }
}
