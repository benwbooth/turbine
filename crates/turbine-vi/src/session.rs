//! One editor's vim state, for hosts that keep the text elsewhere (a rope).
//!
//! The host hands in the text when it changed outside vim (a paste from a
//! menu, an agent, a reload), sends keys, and reads the text, cursor and
//! selection back. On top of tessh's engine this adds what a multi-window
//! editor needs: insert-mode editing keys, macros (`q{reg}` … `q`,
//! `[count]@{reg}`, `@@`), `Ctrl-W` window commands, and ex commands that
//! act on files and windows ([`Out::Actions`]).

use crate::ex::{Ex, ExAction};
use crate::text::{Indent, Text};
use crate::vi::{CmdLine, Vi, ViOut};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::HashMap;

/// What the host should do after a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// Vim used the key; read the text and cursor back.
    Handled,
    /// Not vim's: the host's own keys may take it.
    NotMine,
    /// Ex commands for the host: write, quit, edit a file, windows, options.
    Actions(Vec<ExAction>),
    /// `Ctrl-W` commands: "next", "prev", "split", "vsplit", "close",
    /// "only", "new", "left", "right", "up", "down".
    Window(&'static str),
}

pub struct Session {
    pub t: Text,
    pub vi: Vi,
    pub ex: Ex,
    pub cmd: CmdLine,
    pub indent: Indent,
    /// Host text version this state was loaded from.
    version: Option<u64>,
    /// `q` or `@` or `w` (Ctrl-W) waiting for its next key.
    awaiting: Option<char>,
    recording: Option<(char, Vec<KeyEvent>)>,
    macros: HashMap<char, Vec<KeyEvent>>,
    last_macro: Option<char>,
    /// Count typed before `@`.
    macro_count: usize,
    replay_depth: usize,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        let mut vi = Vi::new();
        let t = Text::new();
        vi.set_normal(true, &t);
        Session {
            t,
            vi,
            ex: Ex::new(),
            cmd: CmdLine::new(),
            indent: Indent { mode: crate::text::IndentMode::Spaces, size: 4 },
            version: None,
            awaiting: None,
            recording: None,
            macros: HashMap::new(),
            last_macro: None,
            macro_count: 0,
            replay_depth: 0,
        }
    }

    /// Bring the text in from the host if it changed there (by `version`),
    /// and follow the host's cursor (mouse clicks move it).
    pub fn sync_in(&mut self, text: impl FnOnce() -> String, version: u64, cursor: usize) {
        if self.version != Some(version) {
            let s = text();
            if s != self.t.text {
                // (The first text a session sees isn't an edit: undo never
                // goes back past it to nothing.)
                if self.version.is_none() {
                    self.t.set_fresh(&s);
                    self.vi.start_from(&self.t);
                } else {
                    self.t.set(&s);
                }
            }
            self.version = Some(version);
        }
        if self.t.anchor.is_none() && self.vi.visual.is_none() {
            self.t.cursor = self.t.clamp_pos(cursor.min(self.t.text.len()));
        }
    }

    /// The host applied vim's text: remember which version that is.
    pub fn synced(&mut self, version: u64) {
        self.version = Some(version);
    }

    /// "NORMAL", "INSERT", "VISUAL", ... plus "recording @q" and pending keys.
    pub fn mode_line(&self) -> String {
        let mut s = format!("-- {} --", self.vi.mode_name());
        if !self.vi.pending.is_empty() {
            s.push(' ');
            s.push_str(&self.vi.pending);
        }
        if let Some((r, _)) = &self.recording {
            s.push_str(&format!(" recording @{r}"));
        }
        s
    }

    pub fn is_normal(&self) -> bool {
        self.vi.normal
    }

    /// The command line while open (":" + text, cursor column in chars),
    /// else the last message (text, is_error).
    pub fn command_line(&self) -> Option<(String, Option<usize>, bool)> {
        if self.cmd.is_open() {
            let line = format!("{}{}", self.cmd.kind, self.cmd.text);
            let col = 1 + self.cmd.text[..self.cmd.cursor].chars().count();
            return Some((line, Some(col), false));
        }
        self.cmd.message.as_ref().map(|(m, e)| (m.clone(), None, *e))
    }

    pub fn key(&mut self, k: &KeyEvent) -> Out {
        if k.kind == KeyEventKind::Release {
            return Out::NotMine;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        if let Some(a) = self.awaiting.take() {
            return self.finish_awaiting(a, k);
        }

        let idle = self.vi.normal && self.vi.visual.is_none() && !self.cmd.is_open();
        let count_only = self.vi.pending.chars().all(|c| c.is_ascii_digit());
        if idle && !alt && count_only {
            match k.code {
                KeyCode::Char('q') if !ctrl && self.vi.pending.is_empty() => {
                    if let Some((r, keys)) = self.recording.take() {
                        self.store_macro(r, keys);
                    } else {
                        self.awaiting = Some('q');
                    }
                    return Out::Handled;
                }
                KeyCode::Char('@') if !ctrl => {
                    self.macro_count = self.vi.pending.parse().unwrap_or(1).max(1);
                    self.vi.clear_pending();
                    self.awaiting = Some('@');
                    return Out::Handled;
                }
                KeyCode::Char('w') if ctrl && self.vi.pending.is_empty() => {
                    self.awaiting = Some('w');
                    return Out::Handled;
                }
                _ => {}
            }
        }

        if let Some((_, keys)) = &mut self.recording {
            keys.push(*k);
        }
        self.key_inner(k)
    }

    fn store_macro(&mut self, r: char, keys: Vec<KeyEvent>) {
        if r.is_ascii_uppercase() {
            self.macros.entry(r.to_ascii_lowercase()).or_default().extend(keys);
        } else {
            self.macros.insert(r, keys);
        }
    }

    fn finish_awaiting(&mut self, a: char, k: &KeyEvent) -> Out {
        let KeyCode::Char(c) = k.code else {
            return Out::Handled; // Esc or anything else cancels
        };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match a {
            'q' if c.is_ascii_alphanumeric() || c == '"' => {
                self.recording = Some((c, Vec::new()));
                Out::Handled
            }
            '@' => {
                let reg = if c == '@' { self.last_macro } else { Some(c.to_ascii_lowercase()) };
                let Some(reg) = reg else { return Out::Handled };
                // ":"-register replays the last ex command.
                let keys: Vec<KeyEvent> = if reg == ':' {
                    match self.ex.history.last() {
                        Some(h) => std::iter::once(':')
                            .chain(h.chars())
                            .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
                            .chain(std::iter::once(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)))
                            .collect(),
                        None => return Out::Handled,
                    }
                } else {
                    match self.macros.get(&reg) {
                        Some(m) => m.clone(),
                        None => return Out::Handled,
                    }
                };
                self.last_macro = Some(reg);
                if self.replay_depth > 20 {
                    return Out::Handled;
                }
                self.replay_depth += 1;
                let mut actions = Vec::new();
                for _ in 0..self.macro_count.max(1) {
                    for key in &keys {
                        match self.key(key) {
                            Out::Actions(a) => actions.extend(a),
                            Out::Window(_) | Out::Handled | Out::NotMine => {}
                        }
                    }
                }
                self.replay_depth -= 1;
                if actions.is_empty() { Out::Handled } else { Out::Actions(actions) }
            }
            'w' => Out::Window(match (c, ctrl) {
                ('w', _) => "next",
                ('W', _) | ('p', _) => "prev",
                ('s', _) | ('S', _) => "split",
                ('v', _) => "vsplit",
                ('q', _) | ('c', _) => "close",
                ('o', _) => "only",
                ('n', _) => "new",
                ('h', _) => "left",
                ('l', _) => "right",
                ('k', _) => "up",
                ('j', _) => "down",
                _ => return Out::Handled,
            }),
            _ => Out::Handled,
        }
    }

    fn key_inner(&mut self, k: &KeyEvent) -> Out {
        let ind = self.indent.clone();
        if self.cmd.is_open() {
            let out = self.cmd.key(k, &mut self.t, &ind, &mut self.vi, &mut self.ex);
            return self.after(out);
        }
        self.cmd.dismiss();
        let out = self.vi.key(k, &mut self.t, &ind, &mut self.ex);
        if out == ViOut::None {
            if self.vi.normal {
                return Out::NotMine;
            }
            return if self.insert_key(k) { Out::Handled } else { Out::NotMine };
        }
        self.after(out)
    }

    fn after(&mut self, out: ViOut) -> Out {
        match out {
            ViOut::None => Out::NotMine,
            ViOut::Handled | ViOut::HistoryStep(_) => Out::Handled,
            ViOut::Actions(a) => Out::Actions(a),
            ViOut::Submit => Out::Actions(vec![ExAction::Write(String::new(), false)]),
            ViOut::Clear => Out::Actions(vec![ExAction::Quit(false)]),
            o => {
                self.cmd.apply(&o, &self.ex);
                Out::Handled
            }
        }
    }

    /// Insert mode's editing keys (vim's, plus arrows).
    fn insert_key(&mut self, k: &KeyEvent) -> bool {
        let t = &mut self.t;
        let m = k.modifiers;
        let ctrl = m.contains(KeyModifiers::CONTROL);
        let alt = m.contains(KeyModifiers::ALT);
        let c = t.cursor;
        match k.code {
            KeyCode::Char(ch) if !ctrl && !alt => t.insert(&ch.to_string()),
            // Shift+Enter: a bare newline (literal text, as agents type it).
            KeyCode::Enter if m.contains(KeyModifiers::SHIFT) => t.insert("\n"),
            KeyCode::Enter => {
                let indent = t.indent_at(c).to_string();
                t.insert(&format!("\n{indent}"));
            }
            KeyCode::Tab => {
                let u = self.indent.unit();
                t.insert(&u);
            }
            KeyCode::Backspace => {
                if c > 0 {
                    t.replace(t.prev(c), c, "");
                }
            }
            KeyCode::Delete => {
                if c < t.len() {
                    t.replace(c, t.next(c), "");
                }
            }
            KeyCode::Char('w') if ctrl => {
                let a = t.word_left(c);
                t.replace(a, c, "");
            }
            KeyCode::Char('u') if ctrl => {
                let a = t.line_start(c);
                t.replace(a, c, "");
            }
            KeyCode::Char('h') if ctrl => {
                if c > 0 {
                    t.replace(t.prev(c), c, "");
                }
            }
            KeyCode::Left => {
                let p = if c > t.line_start(c) { t.prev(c) } else { c };
                t.move_to(p, false);
            }
            KeyCode::Right => {
                let p = if c < t.line_end(c) { t.next(c) } else { c };
                t.move_to(p, false);
            }
            KeyCode::Home => t.move_to(t.line_start(c), false),
            KeyCode::End => t.move_to(t.line_end(c), false),
            KeyCode::Up | KeyCode::Down => {
                let col = t.col_of(c);
                let ls = t.line_start(c);
                let target = if k.code == KeyCode::Up {
                    if ls == 0 {
                        return true;
                    }
                    t.line_start(ls - 1)
                } else {
                    let le = t.line_end(c);
                    if le >= t.len() {
                        return true;
                    }
                    le + 1
                };
                let p = t.pos_at_col(target, col);
                t.move_to(p, false);
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &mut Session, text: &str) -> Vec<Out> {
        let mut outs = Vec::new();
        let mut it = text.chars().peekable();
        while let Some(c) = it.next() {
            let k = match c {
                '\u{1b}' => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                '\r' => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                '\u{17}' => KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
                c => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            };
            outs.push(s.key(&k));
        }
        outs
    }

    fn session(text: &str) -> Session {
        let mut s = Session::new();
        s.sync_in(|| text.to_string(), 1, 0);
        s
    }

    #[test]
    fn insert_and_escape() {
        let mut s = session("");
        keys(&mut s, "ihello\rworld\u{1b}");
        assert_eq!(s.t.text, "hello\nworld");
        assert!(s.is_normal());
        keys(&mut s, "ggdd");
        assert_eq!(s.t.text, "world");
        keys(&mut s, "u");
        assert_eq!(s.t.text, "hello\nworld");
    }

    #[test]
    fn macros_record_and_replay() {
        let mut s = session("a\nb\nc\nd");
        keys(&mut s, "qqA!\u{1b}jq");
        assert_eq!(s.t.text, "a!\nb\nc\nd");
        keys(&mut s, "@q");
        assert_eq!(s.t.text, "a!\nb!\nc\nd");
        keys(&mut s, "2@@");
        assert_eq!(s.t.text, "a!\nb!\nc!\nd!");
    }

    #[test]
    fn ex_file_commands_go_to_host() {
        let mut s = session("x");
        let outs = keys(&mut s, ":wq\r");
        assert_eq!(
            outs.last(),
            Some(&Out::Actions(vec![ExAction::Write(String::new(), false), ExAction::Quit(false)]))
        );
        let outs = keys(&mut s, ":vsp\r");
        assert_eq!(outs.last(), Some(&Out::Actions(vec![ExAction::Window("vsplit".into())])));
        let outs = keys(&mut s, "\u{17}w");
        assert_eq!(outs.last(), Some(&Out::Window("next")));
    }

    #[test]
    fn substitute_and_text_objects() {
        let mut s = session("foo(bar, baz) foo");
        keys(&mut s, ":%s/foo/qux/g\r");
        assert_eq!(s.t.text, "qux(bar, baz) qux");
        keys(&mut s, "0f(ci(x\u{1b}");
        assert_eq!(s.t.text, "qux(x) qux");
    }

    #[test]
    fn host_changes_are_picked_up() {
        let mut s = session("one");
        s.sync_in(|| "two".to_string(), 2, 0);
        keys(&mut s, "x");
        assert_eq!(s.t.text, "wo");
    }
}
