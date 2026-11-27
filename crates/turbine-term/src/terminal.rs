//! A terminal: Ghostty's engine (libghostty-vt) fed by a shell on a PTY.
//!
//! The engine's types are not `Send`, so a `Terminal` lives on the UI
//! thread. A reader thread moves the PTY's output over a channel; `pump`
//! feeds it to the engine, as often as the host ticks.

use libghostty_vt::{
    fmt::{Format, Formatter, FormatterOptions},
    key, mouse,
    paste,
    render::{CellIterator, CursorVisualStyle, RenderState, RowIterator},
    screen::{CellWide, Screen},
    selection::{
        gesture::{Behavior, Behaviors, DragEvent, Geometry, Gesture, PressEvent, ReleaseEvent},
        Selection,
    },
    style::{RgbColor, Underline},
    terminal::{Mode, Options, Point, PointCoordinate, ScrollViewport},
    Terminal as VtTerminal,
};
use std::time::{Duration, Instant};
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
    /// Inside the mouse selection.
    pub selected: bool,
}

/// A mouse action on the terminal, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAct {
    /// Button 1 left, 2 right, 3 middle.
    Press(u8),
    Release(u8),
    /// Pointer moved (with or without buttons held).
    Motion,
    Wheel { up: bool },
}

/// What a mouse action did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseOutcome {
    /// Sent to the program (it asked for mouse reports).
    Reported,
    /// Started, extended or finished a selection.
    Selection,
    /// Scrolled the scrollback (or sent arrows to a full-screen program).
    Scrolled,
    /// Middle click: the host should paste its clipboard.
    PasteRequest,
    Ignored,
}

/// Cells are given to the engine as 8×16 "pixels", so pixel-based mouse
/// reports and click-distance thresholds still make sense on a cell grid.
const CW: u32 = 8;
const CH: u32 = 16;

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
    mouse_enc: mouse::Encoder<'static>,
    mouse_ev: mouse::Event<'static>,
    gesture: Gesture<'static>,
    press: PressEvent<'static>,
    release: ReleaseEvent<'static>,
    drag: DragEvent<'static>,
    t0: Instant,
    /// Buttons held (bit 1 left, 2 right, 3 middle).
    buttons: u8,
    selecting: bool,
    /// Where the selection started (col, row), to know the drag direction.
    press_at: (u16, u32),
    last_motion: Vec<u8>,
    selected: String,
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
    // The account's own shell, as a login shell: $SHELL may be a build
    // shell's (devloop runs Turbine in the project's dev shell, whose bash
    // has no readline), and a login shell sets PATH and the rest afresh.
    if let Some(sh) = login_shell() {
        return vec![sh, "-l".into()];
    }
    vec![std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into())]
}

#[cfg(unix)]
fn login_shell() -> Option<String> {
    let pw = unsafe { libc::getpwuid(libc::getuid()) };
    if pw.is_null() {
        return None;
    }
    let sh = unsafe { std::ffi::CStr::from_ptr((*pw).pw_shell) }.to_string_lossy().into_owned();
    (!sh.is_empty() && std::path::Path::new(&sh).exists()).then_some(sh)
}

#[cfg(not(unix))]
fn login_shell() -> Option<String> {
    None
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
            mouse_enc: mouse::Encoder::new().map_err(err)?,
            mouse_ev: mouse::Event::new().map_err(err)?,
            gesture: Gesture::new().map_err(err)?,
            press: {
                let mut p = PressEvent::new().map_err(err)?;
                p.set_repeat_interval(Duration::from_millis(500)).map_err(err)?;
                let b = Behaviors::new()
                    .with_single_click_behavior(Behavior::Cell)
                    .with_double_click_behavior(Behavior::Word)
                    .with_triple_click_behavior(Behavior::Line);
                p.set_behaviors(&b).map_err(err)?;
                p
            },
            release: ReleaseEvent::new().map_err(err)?,
            drag: DragEvent::new().map_err(err)?,
            t0: Instant::now(),
            buttons: 0,
            selecting: false,
            press_at: (0, 0),
            last_motion: Vec::new(),
            selected: String::new(),
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

    /// The selected text (empty when nothing is selected).
    pub fn selected_text(&self) -> &str {
        &self.selected
    }

    pub fn has_selection(&self) -> bool {
        !self.selected.is_empty()
    }

    pub fn clear_selection(&mut self) {
        if !self.selected.is_empty() {
            let _ = self.term.set_selection(None);
            self.selected.clear();
            self.changed = true;
        }
    }

    /// Select everything (screen and scrollback).
    pub fn select_all(&mut self) {
        if let Ok(Some(sel)) = self.term.select_all() {
            let _ = self.term.set_selection(Some(&sel));
            self.selected = format_selection(&self.term, &sel);
            self.changed = true;
        }
    }

    /// True when the program asked for mouse reports.
    pub fn wants_mouse(&self) -> bool {
        self.term.is_mouse_tracking().unwrap_or(false)
    }

    /// A mouse action at cell (col, row) of the screen. Shift (or a program
    /// that doesn't track the mouse) means the action selects text.
    pub fn mouse(&mut self, act: MouseAct, col: i32, row: i32, mods: key::Mods) -> MouseOutcome {
        let (cols, rows) = (self.cols as i32, self.rows as i32);
        let cx = col.clamp(0, cols - 1) as u16;
        let cy = row.clamp(0, rows - 1) as u32;
        let (x, y) = (cx as f32 * CW as f32 + CW as f32 / 2.0, cy as f32 * CH as f32 + CH as f32 / 2.0);
        match act {
            MouseAct::Press(b) if (1..4).contains(&b) => self.buttons |= 1 << b,
            MouseAct::Release(b) if (1..4).contains(&b) => self.buttons &= !(1 << b),
            _ => {}
        }
        let report = self.wants_mouse() && !mods.contains(key::Mods::SHIFT) && !self.selecting;

        if let MouseAct::Wheel { up } = act {
            if !report {
                if matches!(self.term.active_screen(), Ok(Screen::Alternate)) {
                    // Full-screen programs scroll with arrow keys (alternate scroll).
                    let app = self.term.mode(Mode::DECCKM).unwrap_or(false);
                    let seq: &[u8] = match (up, app) {
                        (true, false) => b"\x1b[A",
                        (false, false) => b"\x1b[B",
                        (true, true) => b"\x1bOA",
                        (false, true) => b"\x1bOB",
                    };
                    self.write(&seq.repeat(3));
                } else {
                    self.scroll(if up { -3 } else { 3 });
                }
                return MouseOutcome::Scrolled;
            }
        }

        if report {
            self.mouse_enc
                .set_options_from_terminal(&self.term)
                .set_size(mouse::EncoderSize {
                    screen_width: self.cols as u32 * CW,
                    screen_height: self.rows as u32 * CH,
                    cell_width: CW,
                    cell_height: CH,
                    padding_top: 0,
                    padding_bottom: 0,
                    padding_left: 0,
                    padding_right: 0,
                })
                .set_any_button_pressed(self.buttons != 0)
                .set_track_last_cell(true);
            let btn = |b: u8| match b {
                1 => Some(mouse::Button::Left),
                2 => Some(mouse::Button::Right),
                3 => Some(mouse::Button::Middle),
                _ => None,
            };
            let held = [1u8, 2, 3].into_iter().find(|b| self.buttons & (1 << b) != 0).and_then(btn);
            let (actions, button): (&[mouse::Action], Option<mouse::Button>) = match act {
                MouseAct::Press(b) => (&[mouse::Action::Press], btn(b)),
                MouseAct::Release(b) => (&[mouse::Action::Release], btn(b)),
                MouseAct::Motion => (&[mouse::Action::Motion], held),
                MouseAct::Wheel { up } => (
                    &[mouse::Action::Press, mouse::Action::Release],
                    Some(if up { mouse::Button::Four } else { mouse::Button::Five }),
                ),
            };
            self.mouse_ev.set_mods(mods).set_position(mouse::Position { x, y });
            let mut out = Vec::new();
            for &a in actions {
                self.mouse_ev.set_action(a).set_button(button);
                let _ = self.mouse_enc.encode_to_vec(&self.mouse_ev, &mut out);
            }
            if act == MouseAct::Motion {
                if out == self.last_motion {
                    return MouseOutcome::Reported;
                }
                self.last_motion = out.clone();
            } else {
                self.last_motion.clear();
            }
            self.write(&out);
            return MouseOutcome::Reported;
        }

        if act == MouseAct::Press(3) {
            return MouseOutcome::PasteRequest;
        }
        let point = Point::Viewport(PointCoordinate { x: cx, y: cy });
        match act {
            MouseAct::Press(1) => {
                let Ok(grid) = self.term.grid_ref(point) else { return MouseOutcome::Ignored };
                let elapsed = self.t0.elapsed();
                let sel = self
                    .press
                    .set_repeat_distance(CW as f64)
                    .and_then(|p| p.set_time(elapsed))
                    .and_then(|p| p.set_position(x as f64, y as f64))
                    .and_then(|p| p.apply(&mut self.gesture, &self.term, grid));
                self.selecting = true;
                self.press_at = (cx, cy);
                if let Ok(sel) = sel {
                    let _ = self.term.set_selection(sel.as_ref());
                    self.selected = sel.map(|s| format_selection(&self.term, &s)).unwrap_or_default();
                }
                self.changed = true;
                MouseOutcome::Selection
            }
            MouseAct::Motion if self.selecting => {
                // Dragging past the top or bottom scrolls.
                if row < 0 {
                    self.term.scroll_viewport(ScrollViewport::Delta(-1));
                } else if row >= rows {
                    self.term.scroll_viewport(ScrollViewport::Delta(1));
                }
                let Ok(grid) = self.term.grid_ref(point) else { return MouseOutcome::Ignored };
                // The cell under the pointer counts as selected: put the point
                // at its far edge in the drag's direction.
                let forward = (cy, cx) >= (self.press_at.1, self.press_at.0);
                let x = if forward { (cx as u32 * CW + CW - 1) as f32 } else { (cx as u32 * CW) as f32 };
                let rect = mods.contains(key::Mods::ALT);
                let geometry = Geometry { columns: cols as u32, cell_width: CW, padding_left: 0, screen_height: rows as u32 * CH };
                let sel = self
                    .drag
                    .set_rectangle(rect)
                    .and_then(|d| d.set_position(x as f64, y as f64))
                    .and_then(|d| d.apply(&mut self.gesture, &self.term, grid, geometry));
                if let Ok(sel) = sel {
                    let _ = self.term.set_selection(sel.as_ref());
                    self.selected = sel.as_ref().map(|s| format_selection(&self.term, s)).unwrap_or_default();
                }
                self.changed = true;
                MouseOutcome::Selection
            }
            MouseAct::Release(1) if self.selecting => {
                let grid = self.term.grid_ref(point).ok();
                let _ = self.release.apply(&mut self.gesture, &self.term, grid);
                self.selecting = false;
                MouseOutcome::Selection
            }
            _ => MouseOutcome::Ignored,
        }
    }

    /// Encode a key the way the program asked (legacy, xterm modifyOtherKeys
    /// or the kitty keyboard protocol) and send it.
    pub fn key(&mut self, k: &KeyInput) {
        self.key_as(k, key::Action::Press);
    }

    /// A key pressed, repeated or let go (the kitty protocol reports all
    /// three when the program asks; other encodings only presses).
    pub fn key_as(&mut self, k: &KeyInput, action: key::Action) {
        let mut consumed = key::Mods::empty();
        if k.text.is_some() && k.mods.contains(key::Mods::SHIFT) {
            consumed |= key::Mods::SHIFT;
        }
        self.key_ev
            .set_action(action)
            .set_key(k.key)
            .set_mods(k.mods)
            .set_consumed_mods(consumed)
            .set_utf8(k.text.clone());
        if let Some(u) = k.unshifted {
            self.key_ev.set_unshifted_codepoint(u);
        }
        let mut out = Vec::new();
        let _ = self.key_enc.set_options_from_terminal(&self.term).set_alt_esc_prefix(true).encode_to_vec(&self.key_ev, &mut out);
        if out.is_empty() && action != key::Action::Release {
            if let Some(t) = &k.text {
                out.extend_from_slice(t.as_bytes());
            }
        }
        if !out.is_empty() && action == key::Action::Release {
            self.write(&out);
            return;
        }
        if !out.is_empty() {
            self.term.scroll_viewport(ScrollViewport::Bottom);
            self.clear_selection();
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

    /// The scrollback: (total rows, first row shown, rows shown).
    pub fn scrollbar(&self) -> Option<(usize, usize, usize)> {
        let b = self.term.scrollbar().ok()?;
        Some((b.total as usize, b.offset as usize, b.len as usize))
    }

    /// Show the scrollback from `row` (0 = the oldest line).
    pub fn scroll_to_row(&mut self, row: usize) {
        self.term.scroll_viewport(ScrollViewport::Row(row));
        self.changed = true;
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
                        selected: cell.is_selected().unwrap_or(false),
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

fn format_selection(term: &VtTerminal<'static, 'static>, sel: &Selection<'_>) -> String {
    let opts = FormatterOptions::new().with_format(Format::Plain).with_selection(sel).with_trim(true).with_unwrap(true);
    let Ok(mut f) = Formatter::new(term, opts) else { return String::new() };
    f.format_alloc(None).map(|b| String::from_utf8_lossy(&b).trim_end_matches(['\n', '\r']).to_string()).unwrap_or_default()
}
