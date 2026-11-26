//! Glue between an editor window and its vim session.

use crossterm::event::{KeyCode, KeyEvent as CtKey, KeyModifiers};
use turbine_ui::editor::EditorView;
use turbine_ui::event::{Key, KeyEvent};
use turbine_ui::CursorShape;
use turbine_vi::{Out, Session};

/// Which keys the editors use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keymap {
    /// DOS/CUA: Shift+arrows, Ctrl+X/C/V, F-keys.
    Cua,
    Vim,
}

impl Keymap {
    pub fn parse(s: &str) -> Option<Keymap> {
        match s.to_ascii_lowercase().as_str() {
            "cua" | "dos" | "default" => Some(Keymap::Cua),
            "vim" | "vi" => Some(Keymap::Vim),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Keymap::Cua => "CUA (DOS)",
            Keymap::Vim => "Vim",
        }
    }
}

pub fn to_crossterm(k: &KeyEvent) -> Option<CtKey> {
    let mut m = KeyModifiers::NONE;
    if k.mods.shift {
        m |= KeyModifiers::SHIFT;
    }
    if k.mods.ctrl {
        m |= KeyModifiers::CONTROL;
    }
    if k.mods.alt {
        m |= KeyModifiers::ALT;
    }
    let code = match k.key {
        Key::Char(c) => {
            // vim's keys are case-sensitive chars; Shift is already in them.
            if !k.mods.ctrl && !k.mods.alt {
                m.remove(KeyModifiers::SHIFT);
            }
            KeyCode::Char(if k.mods.ctrl { c.to_ascii_lowercase() } else { c })
        }
        Key::Enter => KeyCode::Enter,
        Key::Esc => KeyCode::Esc,
        Key::Backspace => KeyCode::Backspace,
        Key::Tab => KeyCode::Tab,
        Key::BackTab => KeyCode::BackTab,
        Key::Up => KeyCode::Up,
        Key::Down => KeyCode::Down,
        Key::Left => KeyCode::Left,
        Key::Right => KeyCode::Right,
        Key::Home => KeyCode::Home,
        Key::End => KeyCode::End,
        Key::PageUp => KeyCode::PageUp,
        Key::PageDown => KeyCode::PageDown,
        Key::Insert => KeyCode::Insert,
        Key::Delete => KeyCode::Delete,
        Key::F(n) => KeyCode::F(n),
    };
    Some(CtKey::new(code, m))
}

/// Send a key to the editor's vim session and bring the result back into
/// the document and view.
pub fn key(ed: &mut EditorView, vim: &mut Session, k: &KeyEvent) -> Out {
    let Some(ck) = to_crossterm(k) else { return Out::NotMine };
    let cursor_byte = ed.doc.rope.char_to_byte(ed.cursor.min(ed.doc.rope.len_chars()));
    let version = ed.doc.version;
    let doc = &ed.doc;
    vim.sync_in(|| doc.rope.to_string(), version, cursor_byte);
    let out = vim.key(&ck);
    sync_out(ed, vim);
    out
}

/// Copy vim's text, cursor and selection into the view.
pub fn sync_out(ed: &mut EditorView, vim: &mut Session) {
    let before = ed.cursor;
    if ed.doc.replace_all_minimal(&vim.t.text, before) {
        vim.synced(ed.doc.version);
    }
    let rope = &ed.doc.rope;
    let to_char = |b: usize| rope.byte_to_char(b.min(rope.len_bytes()));
    ed.cursor = to_char(vim.t.cursor);
    ed.anchor = vim.t.anchor.map(to_char);
    ed.block = if vim.vi.visual == Some(turbine_vi::vi::BLOCK) {
        vim.vi.block_ranges(&vim.t).into_iter().map(|(a, b)| (to_char(a), to_char(b.max(a + 1)))).collect()
    } else {
        Vec::new()
    };
    // A visual selection includes the char under the cursor, as vim draws it.
    if let (Some(a), Some(v)) = (ed.anchor, vim.vi.visual) {
        if v == 'v' && ed.cursor >= a {
            ed.block = vec![(ed.cursor, (ed.cursor + 1).min(ed.doc.rope.len_chars()))];
        }
    }
    ed.cursor_shape = Some(if vim.is_normal() { CursorShape::Block } else { CursorShape::Bar });
}

/// Start vim on an editor: load its text, show the block cursor.
pub fn attach(ed: &mut EditorView) -> Session {
    let mut s = Session::new();
    let cursor_byte = ed.doc.rope.char_to_byte(ed.cursor.min(ed.doc.rope.len_chars()));
    let doc = &ed.doc;
    s.sync_in(|| doc.rope.to_string(), doc.version, cursor_byte);
    sync_out(ed, &mut s);
    s
}

/// Leave vim: the view goes back to its own cursor style.
pub fn detach(ed: &mut EditorView) {
    ed.cursor_shape = None;
    ed.block.clear();
}
