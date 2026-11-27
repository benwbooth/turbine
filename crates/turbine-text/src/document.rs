//! A document: rope text plus where it came from and how to save it.

use crate::encoding::{self, LineEnding, TextEncoding};
use ropey::Rope;
use std::path::{Path, PathBuf};

/// One reversible change, in char offsets.
#[derive(Clone, Debug)]
struct Edit {
    at: usize,
    removed: String,
    inserted: String,
    /// Cursor before the edit, restored on undo.
    cursor_before: usize,
}

#[derive(Clone, Debug)]
pub struct Document {
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub encoding: TextEncoding,
    pub bom: bool,
    pub line_ending: LineEnding,
    pub lossy: bool,
    pub dirty: bool,
    /// Bumped on every change, so views and modes can tell when to resync.
    pub version: u64,
    undo: Vec<Vec<Edit>>,
    redo: Vec<Vec<Edit>>,
    /// Group consecutive typing into one undo step.
    open_group: bool,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub fn new() -> Self {
        Document {
            rope: Rope::new(),
            path: None,
            encoding: TextEncoding::Utf8,
            bom: false,
            line_ending: if cfg!(windows) { LineEnding::CrLf } else { LineEnding::Lf },
            lossy: false,
            dirty: false,
            version: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            open_group: false,
        }
    }

    pub fn from_str(text: &str) -> Self {
        let mut d = Self::new();
        d.rope = Rope::from_str(text);
        d
    }

    pub fn open(path: &Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        let dec = encoding::decode(&bytes);
        let mut d = Self::new();
        d.rope = Rope::from_str(&dec.text);
        d.path = Some(path.to_path_buf());
        d.encoding = dec.encoding;
        d.bom = dec.bom;
        d.line_ending = dec.line_ending;
        d.lossy = dec.lossy;
        Ok(d)
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(std::io::Error::other("no file name"));
        };
        let bytes = encoding::encode(&self.rope.to_string(), self.encoding, self.bom, self.line_ending);
        std::fs::write(&path, bytes)?;
        self.dirty = false;
        Ok(())
    }

    pub fn save_as(&mut self, path: &Path) -> std::io::Result<()> {
        self.path = Some(path.to_path_buf());
        self.save()
    }

    pub fn title(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "UNTITLED".into())
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// A line's text without its trailing newline.
    pub fn line(&self, i: usize) -> String {
        if i >= self.rope.len_lines() {
            return String::new();
        }
        let mut s = self.rope.line(i).to_string();
        if s.ends_with('\n') {
            s.pop();
        }
        s
    }

    /// (line, byte offset in line) for a char offset.
    pub fn char_to_line_byte(&self, ch: usize) -> (usize, usize) {
        let ch = ch.min(self.rope.len_chars());
        let line = self.rope.char_to_line(ch);
        let start = self.rope.line_to_char(line);
        let byte = self.rope.slice(start..ch).len_bytes();
        (line, byte)
    }

    /// Char offset for (line, byte offset in line), clamped.
    pub fn line_byte_to_char(&self, line: usize, byte: usize) -> usize {
        let line = line.min(self.rope.len_lines().saturating_sub(1));
        let start = self.rope.line_to_char(line);
        let text = self.line(line);
        let byte = byte.min(text.len());
        start + text[..byte].chars().count()
    }

    /// Make the text equal to `new` with one edit over the part that differs.
    /// Returns true if anything changed.
    pub fn replace_all_minimal(&mut self, new: &str, cursor_before: usize) -> bool {
        let old = self.rope.to_string();
        if old == new {
            return false;
        }
        let mut pre = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
        while !old.is_char_boundary(pre) || !new.is_char_boundary(pre) {
            pre -= 1;
        }
        let max_suf = old.len().min(new.len()) - pre;
        let mut suf = old.bytes().rev().zip(new.bytes().rev()).take_while(|(a, b)| a == b).count().min(max_suf);
        while !old.is_char_boundary(old.len() - suf) || !new.is_char_boundary(new.len() - suf) {
            suf -= 1;
        }
        let from = old[..pre].chars().count();
        let to = from + old[pre..old.len() - suf].chars().count();
        let ins = &new[pre..new.len() - suf];
        self.break_undo_group();
        if to > from {
            self.remove(from, to, cursor_before);
        }
        if !ins.is_empty() {
            self.insert(from, ins, cursor_before);
        }
        self.break_undo_group();
        true
    }

    pub fn insert(&mut self, at: usize, text: &str, cursor_before: usize) {
        self.apply(Edit { at, removed: String::new(), inserted: text.into(), cursor_before });
    }

    pub fn remove(&mut self, from: usize, to: usize, cursor_before: usize) {
        if from >= to {
            return;
        }
        let removed = self.rope.slice(from..to).to_string();
        self.apply(Edit { at: from, removed, inserted: String::new(), cursor_before });
    }

    /// Replace a range with text: one undo step.
    pub fn replace(&mut self, from: usize, to: usize, text: &str, cursor_before: usize) {
        let to = to.max(from).min(self.rope.len_chars());
        let removed = self.rope.slice(from..to).to_string();
        if removed == text {
            return;
        }
        self.open_group = false;
        self.apply(Edit { at: from, removed, inserted: text.into(), cursor_before });
        self.open_group = false;
    }

    fn apply(&mut self, e: Edit) {
        let n = e.removed.chars().count();
        self.rope.remove(e.at..e.at + n);
        self.rope.insert(e.at, &e.inserted);
        self.dirty = true;
        self.version += 1;
        self.redo.clear();
        let typing = e.removed.is_empty() && e.inserted.chars().count() == 1 && !e.inserted.contains('\n');
        if self.open_group && typing {
            if let Some(g) = self.undo.last_mut() {
                g.push(e);
                return;
            }
        }
        self.open_group = typing;
        self.undo.push(vec![e]);
    }

    /// End the current typing group so the next edit starts a new undo step.
    pub fn break_undo_group(&mut self) {
        self.open_group = false;
    }

    /// Undo; returns where to put the cursor.
    pub fn undo(&mut self) -> Option<usize> {
        self.open_group = false;
        let group = self.undo.pop()?;
        let mut cursor = 0;
        for e in group.iter().rev() {
            let n = e.inserted.chars().count();
            self.rope.remove(e.at..e.at + n);
            self.rope.insert(e.at, &e.removed);
            cursor = e.cursor_before;
        }
        self.redo.push(group);
        self.dirty = true;
        self.version += 1;
        Some(cursor)
    }

    pub fn redo(&mut self) -> Option<usize> {
        self.open_group = false;
        let group = self.redo.pop()?;
        let mut cursor = 0;
        for e in &group {
            let n = e.removed.chars().count();
            self.rope.remove(e.at..e.at + n);
            self.rope.insert(e.at, &e.inserted);
            cursor = e.at + e.inserted.chars().count();
        }
        self.undo.push(group);
        self.dirty = true;
        self.version += 1;
        Some(cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_replace() {
        let mut d = Document::from_str("héllo wörld");
        assert!(d.replace_all_minimal("héllo big wörld", 0));
        assert_eq!(d.rope.to_string(), "héllo big wörld");
        assert!(d.replace_all_minimal("hé", 0));
        assert_eq!(d.rope.to_string(), "hé");
        assert!(!d.replace_all_minimal("hé", 0));
    }

    #[test]
    fn typing_groups_and_undo() {
        let mut d = Document::new();
        for (i, c) in "hello".chars().enumerate() {
            d.insert(i, &c.to_string(), i);
        }
        d.insert(5, "\n", 5);
        d.insert(6, "x", 6);
        assert_eq!(d.rope.to_string(), "hello\nx");
        assert_eq!(d.undo(), Some(6));
        assert_eq!(d.undo(), Some(5));
        assert_eq!(d.undo(), Some(0));
        assert_eq!(d.rope.to_string(), "");
        d.redo();
        assert_eq!(d.rope.to_string(), "hello");
    }
}
