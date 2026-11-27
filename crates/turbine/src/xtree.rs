//! A file manager as XTree Gold shows one (Tools > File Manager, Ctrl+Alt+F): the path
//! and the time across the top, the directory tree with the files of the
//! directory under it, the FILE / DISK / DIRECTORY statistics at the right,
//! and the commands of the window in use across the bottom, a letter each
//! (Copy, Delete, Rename, Tag, eXecute ...), asked for on the bottom lines.
//! In XTree's own colours, whatever Turbine's look.
//!
//! Folders are read and files copied, moved and deleted on a thread of
//! their own; the screen asks it and shows what comes back.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use turbine_text::unicode::str_width;
use turbine_ui::event::{Button, Key, KeyEvent, MouseEvent, MouseKind};
use turbine_ui::theme::{Style, vga};
use turbine_ui::{Canvas, Point, Rect};

/// XTree Gold's colours.
mod colour {
    use super::*;
    pub fn base() -> Style {
        Style::new(vga::LIGHT_CYAN, vga::BLUE)
    }
    pub fn text() -> Style {
        Style::new(vga::WHITE, vga::BLUE)
    }
    pub fn frame() -> Style {
        Style::new(vga::LIGHT_CYAN, vga::BLUE)
    }
    pub fn bar() -> Style {
        Style::new(vga::BLACK, vga::LIGHT_GRAY)
    }
    /// The selection when its window isn't the one in use.
    pub fn bar_off() -> Style {
        Style::new(vga::WHITE, vga::BLUE)
    }
    pub fn tagged() -> Style {
        Style::new(vga::YELLOW, vga::BLUE)
    }
    pub fn key() -> Style {
        Style::new(vga::WHITE, vga::BLUE)
    }
    pub fn prompt() -> Style {
        Style::new(vga::WHITE, vga::BLUE)
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

/// What the worker thread is asked.
enum Job {
    List(PathBuf),
    Copy(Vec<PathBuf>, PathBuf),
    Move(Vec<PathBuf>, PathBuf),
    Delete(Vec<PathBuf>),
}

/// What it answers.
enum Done {
    Listed(PathBuf, Result<Vec<Entry>, String>),
    Finished(String, Vec<PathBuf>),
}

/// A command being asked for on the bottom lines: what, its question,
/// and the text typed.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    Copy,
    Move,
    Rename,
    Delete,
    MakeDir,
    Filespec,
}

/// What the file manager wants the editor to do.
#[derive(Debug, PartialEq)]
pub enum Out {
    None,
    Edit(PathBuf),
    View(PathBuf),
    Open(PathBuf),
    Execute(PathBuf),
    Close,
    Say(String),
}

pub struct XTree {
    pub root: PathBuf,
    /// Folders read: path → its entries (sorted, folders first).
    dirs: BTreeMap<PathBuf, Vec<Entry>>,
    /// Folders whose branches show.
    expanded: HashSet<PathBuf>,
    /// The tree as shown: (path, depth, last child?).
    tree: Vec<(PathBuf, usize, bool)>,
    tree_sel: usize,
    tree_top: usize,
    file_sel: usize,
    file_top: usize,
    /// The file window has the keys (else the tree).
    in_files: bool,
    /// The file window filling the screen (Enter in the files, as XTree).
    files_full: bool,
    pub tagged: BTreeSet<PathBuf>,
    pub filespec: String,
    ask: Option<(Ask, String)>,
    pub message: String,
    jobs: Sender<Job>,
    done: Receiver<Done>,
    pending: HashSet<PathBuf>,
    /// The last layout, for the mouse.
    tree_rect: Rect,
    files_rect: Rect,
    file_cols: usize,
}

fn read_dir(p: &Path) -> Result<Vec<Entry>, String> {
    let rd = std::fs::read_dir(p).map_err(|e| e.to_string())?;
    let mut v: Vec<Entry> = rd
        .flatten()
        .map(|e| {
            let md = e.metadata().ok();
            Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                dir: e.file_type().is_ok_and(|t| t.is_dir()),
                size: md.as_ref().map_or(0, |m| m.len()),
                modified: md.and_then(|m| m.modified().ok()),
            }
        })
        .collect();
    v.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(v)
}

fn copy_any(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)?.flatten() {
            copy_any(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

fn worker(jobs: Receiver<Job>, done: Sender<Done>) {
    while let Ok(j) = jobs.recv() {
        let d = match j {
            Job::List(p) => {
                let r = read_dir(&p);
                Done::Listed(p, r)
            }
            Job::Copy(files, dest) | Job::Move(files, dest) if !dest.is_dir() && files.len() > 1 => Done::Finished(format!("{} isn't a directory", dest.display()), vec![]),
            Job::Copy(files, dest) => {
                let mut n = 0;
                let mut err = None;
                for f in &files {
                    let to = if dest.is_dir() { dest.join(f.file_name().unwrap_or_default()) } else { dest.clone() };
                    match copy_any(f, &to) {
                        Ok(()) => n += 1,
                        Err(e) => err = Some(format!("{}: {e}", f.display())),
                    }
                }
                let changed = vec![dest.clone(), dest.parent().map(Path::to_path_buf).unwrap_or_default()];
                Done::Finished(err.unwrap_or_else(|| format!("{n} copied")), changed)
            }
            Job::Move(files, dest) => {
                let mut n = 0;
                let mut err = None;
                let mut changed = vec![dest.clone(), dest.parent().map(Path::to_path_buf).unwrap_or_default()];
                for f in &files {
                    let to = if dest.is_dir() { dest.join(f.file_name().unwrap_or_default()) } else { dest.clone() };
                    let r = std::fs::rename(f, &to).or_else(|_| copy_any(f, &to).and_then(|_| if f.is_dir() { std::fs::remove_dir_all(f) } else { std::fs::remove_file(f) }));
                    match r {
                        Ok(()) => n += 1,
                        Err(e) => err = Some(format!("{}: {e}", f.display())),
                    }
                    changed.extend(f.parent().map(Path::to_path_buf));
                }
                Done::Finished(err.unwrap_or_else(|| format!("{n} moved")), changed)
            }
            Job::Delete(files) => {
                let mut n = 0;
                let mut err = None;
                let mut changed = Vec::new();
                for f in &files {
                    // (To the trash, where the desktop keeps one, as Turbine's
                    // Explorer does; else gone.)
                    let r = std::process::Command::new("gio").arg("trash").arg(f).status().ok().filter(|s| s.success()).map(|_| ()).ok_or(()).or_else(|_| {
                        if f.is_dir() { std::fs::remove_dir_all(f) } else { std::fs::remove_file(f) }
                    });
                    match r {
                        Ok(()) => n += 1,
                        Err(e) => err = Some(format!("{}: {e}", f.display())),
                    }
                    changed.extend(f.parent().map(Path::to_path_buf));
                }
                Done::Finished(err.unwrap_or_else(|| format!("{n} deleted")), changed)
            }
        };
        if done.send(d).is_err() {
            return;
        }
    }
}

/// A size as XTree shows it: 262,111,744.
fn commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A filespec (*.rs, *.*) matches a name.
fn matches(spec: &str, name: &str) -> bool {
    spec.split([' ', ';']).filter(|s| !s.is_empty()).any(|p| {
        let p = if p == "*.*" { "*" } else { p };
        glob(p.as_bytes(), name.as_bytes())
    })
}

fn glob(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => glob(&p[1..], s) || (!s.is_empty() && glob(p, &s[1..])),
        (Some(b'?'), Some(_)) => glob(&p[1..], &s[1..]),
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => glob(&p[1..], &s[1..]),
        _ => false,
    }
}

impl XTree {
    pub fn new(root: &Path) -> XTree {
        let (jtx, jrx) = channel();
        let (dtx, drx) = channel();
        std::thread::Builder::new().name("turbine-xtree".into()).spawn(move || worker(jrx, dtx)).expect("xtree thread");
        let mut x = XTree {
            root: root.to_path_buf(),
            dirs: BTreeMap::new(),
            expanded: HashSet::new(),
            tree: Vec::new(),
            tree_sel: 0,
            tree_top: 0,
            file_sel: 0,
            file_top: 0,
            in_files: false,
            files_full: false,
            tagged: BTreeSet::new(),
            filespec: "*.*".into(),
            ask: None,
            message: String::new(),
            jobs: jtx,
            done: drx,
            pending: HashSet::new(),
            tree_rect: Rect::default(),
            files_rect: Rect::default(),
            file_cols: 1,
        };
        x.expanded.insert(root.to_path_buf());
        x.list(root);
        x.rebuild();
        x
    }

    fn list(&mut self, p: &Path) {
        if self.pending.insert(p.to_path_buf()) {
            let _ = self.jobs.send(Job::List(p.to_path_buf()));
        }
    }

    /// Take what the worker has done. True when the screen changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for d in self.done.try_iter().collect::<Vec<_>>() {
            changed = true;
            match d {
                Done::Listed(p, r) => {
                    self.pending.remove(&p);
                    match r {
                        Ok(v) => {
                            // (Its folders' own listings, for the tree's +.)
                            self.dirs.insert(p, v);
                        }
                        Err(e) => self.message = format!("{}: {e}", p.display()),
                    }
                }
                Done::Finished(msg, dirs) => {
                    self.message = msg;
                    self.tagged.retain(|t| t.exists());
                    for d in dirs {
                        if self.dirs.contains_key(&d) {
                            self.list(&d);
                        }
                    }
                }
            }
        }
        if changed {
            self.rebuild();
        }
        changed
    }

    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The tree's rows from the expanded folders.
    fn rebuild(&mut self) {
        let keep = self.tree.get(self.tree_sel).map(|t| t.0.clone());
        let mut out = Vec::new();
        fn walk(x: &XTree, p: &Path, depth: usize, last: bool, out: &mut Vec<(PathBuf, usize, bool)>) {
            out.push((p.to_path_buf(), depth, last));
            if !x.expanded.contains(p) {
                return;
            }
            let Some(entries) = x.dirs.get(p) else { return };
            let kids: Vec<&Entry> = entries.iter().filter(|e| e.dir).collect();
            for (i, e) in kids.iter().enumerate() {
                walk(x, &p.join(&e.name), depth + 1, i + 1 == kids.len(), out);
            }
        }
        walk(self, &self.root.clone(), 0, true, &mut out);
        self.tree = out;
        if let Some(k) = keep {
            if let Some(i) = self.tree.iter().position(|t| t.0 == k) {
                self.tree_sel = i;
            }
        }
        self.tree_sel = self.tree_sel.min(self.tree.len().saturating_sub(1));
        let n = self.files().len();
        self.file_sel = self.file_sel.min(n.saturating_sub(1));
    }

    pub fn current_dir(&self) -> PathBuf {
        self.tree.get(self.tree_sel).map(|t| t.0.clone()).unwrap_or_else(|| self.root.clone())
    }

    /// The current directory's files matching the filespec.
    fn files(&self) -> Vec<Entry> {
        self.dirs.get(&self.current_dir()).map(|v| v.iter().filter(|e| !e.dir && matches(&self.filespec, &e.name)).cloned().collect()).unwrap_or_default()
    }

    fn current_file(&self) -> Option<PathBuf> {
        self.files().get(self.file_sel).map(|e| self.current_dir().join(&e.name))
    }

    /// What a command works on: the tagged files, else the current one.
    fn targets(&self) -> Vec<PathBuf> {
        let dir = self.current_dir();
        let tagged: Vec<PathBuf> = self.tagged.iter().filter(|t| t.parent() == Some(dir.as_path())).cloned().collect();
        if !tagged.is_empty() && self.in_files {
            tagged
        } else if self.in_files {
            self.current_file().into_iter().collect()
        } else {
            vec![dir]
        }
    }

    fn select_dir(&mut self, i: usize) {
        self.tree_sel = i.min(self.tree.len().saturating_sub(1));
        let d = self.current_dir();
        if !self.dirs.contains_key(&d) {
            self.list(&d);
        }
        self.file_sel = 0;
        self.file_top = 0;
    }

    // ---- keys ---------------------------------------------------------------------------

    pub fn key(&mut self, k: &KeyEvent) -> Out {
        if self.ask.is_some() {
            return self.ask_key(k);
        }
        self.message.clear();
        let m = k.mods;
        let plain = !m.ctrl && !m.alt;
        match k.key {
            Key::Esc if self.files_full => self.files_full = false,
            Key::Esc if self.in_files => self.in_files = false,
            Key::Esc => return Out::Close,
            Key::Char('q' | 'Q') if plain => return Out::Close,
            Key::Enter if !self.in_files => {
                self.in_files = true;
                self.file_sel = 0;
            }
            Key::Enter if !self.files_full => self.files_full = true,
            Key::Enter => {
                self.files_full = false;
                self.in_files = false;
            }
            Key::Tab => self.in_files = !self.in_files,
            Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Home | Key::End | Key::Left | Key::Right => self.mv(k.key),
            // The tree: + opens a branch, - closes it, * opens all under it.
            Key::Char('+') if !self.in_files => {
                let d = self.current_dir();
                self.expanded.insert(d.clone());
                self.list(&d);
                self.rebuild();
            }
            Key::Char('-') if !self.in_files => {
                let d = self.current_dir();
                self.expanded.remove(&d);
                self.rebuild();
            }
            Key::Char('*') if !self.in_files => {
                let d = self.current_dir();
                self.expand_all(&d, 3);
            }
            Key::Char(c) if plain => {
                let c = c.to_ascii_lowercase();
                return self.command(c);
            }
            Key::F(1) => self.message = "Enter files/tree  Tab switch  + - * branches  T tag  U untag  Ctrl+T/U all  C copy  M move  D delete  R rename  F filespec  E edit  V view  O open  X execute  Q quit".into(),
            _ if m.ctrl => match k.key {
                Key::Char('t' | 'T') => {
                    for e in self.files() {
                        self.tagged.insert(self.current_dir().join(e.name));
                    }
                }
                Key::Char('u' | 'U') => {
                    let d = self.current_dir();
                    self.tagged.retain(|t| t.parent() != Some(d.as_path()));
                }
                _ => {}
            },
            _ => {}
        }
        Out::None
    }

    fn expand_all(&mut self, d: &Path, depth: usize) {
        self.expanded.insert(d.to_path_buf());
        if let Some(v) = self.dirs.get(d).cloned() {
            if depth > 0 {
                for e in v.iter().filter(|e| e.dir) {
                    let p = d.join(&e.name);
                    if !self.dirs.contains_key(&p) {
                        self.list(&p);
                    }
                    self.expand_all(&p, depth - 1);
                }
            }
        } else {
            self.list(d);
        }
        self.rebuild();
    }

    fn mv(&mut self, k: Key) {
        if self.in_files {
            let n = self.files().len();
            if n == 0 {
                return;
            }
            let rows = (self.files_rect.h.max(1)) as usize;
            let cols = self.file_cols.max(1);
            let i = self.file_sel as isize;
            let to = match k {
                Key::Up => i - 1,
                Key::Down => i + 1,
                Key::Left => i - rows as isize,
                Key::Right => i + rows as isize,
                Key::PageUp => i - (rows * cols) as isize,
                Key::PageDown => i + (rows * cols) as isize,
                Key::Home => 0,
                _ => n as isize - 1,
            };
            self.file_sel = to.clamp(0, n as isize - 1) as usize;
        } else {
            let n = self.tree.len();
            let page = self.tree_rect.h.max(1) as isize;
            let i = self.tree_sel as isize;
            let to = match k {
                Key::Up => i - 1,
                Key::Down => i + 1,
                Key::PageUp => i - page,
                Key::PageDown => i + page,
                Key::Home => 0,
                Key::End => n as isize - 1,
                Key::Left => {
                    // (To the parent.)
                    let d = self.current_dir();
                    d.parent().and_then(|p| self.tree.iter().position(|t| t.0 == p)).map(|x| x as isize).unwrap_or(i)
                }
                _ => {
                    let d = self.current_dir();
                    self.expanded.insert(d.clone());
                    self.list(&d);
                    self.rebuild();
                    i + 1
                }
            };
            self.select_dir(to.clamp(0, n.saturating_sub(1) as isize) as usize);
        }
    }

    /// A letter command (XTree's), for the window in use.
    fn command(&mut self, c: char) -> Out {
        let dir = self.current_dir();
        match (self.in_files, c) {
            (true, 't') => {
                if let Some(f) = self.current_file() {
                    self.tagged.insert(f);
                    self.mv(Key::Down);
                }
            }
            (true, 'u') => {
                if let Some(f) = self.current_file() {
                    self.tagged.remove(&f);
                    self.mv(Key::Down);
                }
            }
            (false, 't') => {
                for e in self.files() {
                    self.tagged.insert(dir.join(e.name));
                }
            }
            (false, 'u') => self.tagged.retain(|t| t.parent() != Some(dir.as_path())),
            (true, 'i') | (false, 'i') => {
                for e in self.files() {
                    let p = dir.join(e.name);
                    if !self.tagged.remove(&p) {
                        self.tagged.insert(p);
                    }
                }
            }
            (true, 'e') => return self.current_file().map(Out::Edit).unwrap_or(Out::None),
            (true, 'v') => return self.current_file().map(Out::View).unwrap_or(Out::None),
            (true, 'o') => return self.current_file().map(Out::Open).unwrap_or(Out::None),
            (true, 'x') => return self.current_file().map(Out::Execute).unwrap_or(Out::None),
            (false, 'x') => return Out::Execute(dir),
            (_, 'c') => self.ask = Some((Ask::Copy, dir.display().to_string() + "/")),
            (_, 'm') if self.in_files => self.ask = Some((Ask::Move, dir.display().to_string() + "/")),
            (false, 'm') => self.ask = Some((Ask::MakeDir, String::new())),
            (_, 'r') => {
                let name = if self.in_files { self.current_file() } else { Some(dir.clone()) }.and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
                self.ask = Some((Ask::Rename, name));
            }
            (_, 'd') => self.ask = Some((Ask::Delete, String::new())),
            (_, 'f') => self.ask = Some((Ask::Filespec, self.filespec.clone())),
            (_, 'l') => {
                // Log: read the directory again.
                self.list(&dir);
            }
            (false, 's') => {
                // Showall: every branch.
                let root = self.root.clone();
                self.expand_all(&root, 6);
            }
            _ => {}
        }
        Out::None
    }

    fn ask_key(&mut self, k: &KeyEvent) -> Out {
        let Some((what, text)) = &mut self.ask else { return Out::None };
        match k.key {
            Key::Esc => self.ask = None,
            Key::Backspace => {
                text.pop();
            }
            Key::Char(c) if !k.mods.ctrl && !k.mods.alt => {
                if *what == Ask::Delete {
                    let yes = c.eq_ignore_ascii_case(&'y');
                    self.ask = None;
                    if yes {
                        let t = self.targets();
                        self.message = format!("Deleting {}…", t.len());
                        let _ = self.jobs.send(Job::Delete(t));
                    }
                    return Out::None;
                }
                text.push(c);
            }
            Key::Enter => {
                let (what, text) = self.ask.take().unwrap();
                let t = self.targets();
                let dir = self.current_dir();
                match what {
                    Ask::Copy | Ask::Move => {
                        let dest = PathBuf::from(text.trim());
                        let dest = if dest.is_absolute() { dest } else { dir.join(dest) };
                        self.message = format!("{} {}…", if what == Ask::Copy { "Copying" } else { "Moving" }, t.len());
                        let _ = self.jobs.send(if what == Ask::Copy { Job::Copy(t, dest) } else { Job::Move(t, dest) });
                    }
                    Ask::Rename => {
                        let Some(from) = (if self.in_files { self.current_file() } else { Some(dir.clone()) }) else { return Out::None };
                        let to = from.with_file_name(text.trim());
                        match std::fs::rename(&from, &to) {
                            Ok(()) => {
                                self.message = format!("Renamed to {}", text.trim());
                                if let Some(p) = from.parent() {
                                    self.list(p);
                                }
                            }
                            Err(e) => self.message = e.to_string(),
                        }
                    }
                    Ask::MakeDir => match std::fs::create_dir(dir.join(text.trim())) {
                        Ok(()) => {
                            self.expanded.insert(dir.clone());
                            self.list(&dir);
                        }
                        Err(e) => self.message = e.to_string(),
                    },
                    Ask::Filespec => {
                        self.filespec = if text.trim().is_empty() { "*.*".into() } else { text.trim().to_string() };
                        self.file_sel = 0;
                    }
                    Ask::Delete => {}
                }
            }
            _ => {}
        }
        Out::None
    }

    pub fn mouse(&mut self, m: &MouseEvent) -> Out {
        if !matches!(m.kind, MouseKind::Down(Button::Left)) {
            if matches!(m.kind, MouseKind::ScrollUp | MouseKind::ScrollDown) {
                let k = if m.kind == MouseKind::ScrollUp { Key::Up } else { Key::Down };
                for _ in 0..3 {
                    self.mv(k);
                }
            }
            return Out::None;
        }
        if self.tree_rect.contains(m.pos) && !self.files_full {
            self.in_files = false;
            let i = self.tree_top + (m.pos.y - self.tree_rect.y) as usize;
            if i < self.tree.len() {
                self.select_dir(i);
            }
        } else if self.files_rect.contains(m.pos) {
            self.in_files = true;
            let rows = self.files_rect.h.max(1) as usize;
            let colw = (self.files_rect.w as usize / self.file_cols.max(1)).max(1);
            let col = (m.pos.x - self.files_rect.x) as usize / colw;
            let i = self.file_top + col * rows + (m.pos.y - self.files_rect.y) as usize;
            if i < self.files().len() {
                self.file_sel = i;
            }
        }
        Out::None
    }

    // ---- drawing ------------------------------------------------------------------------

    pub fn draw(&mut self, c: &mut Canvas, r: Rect, focused: bool) {
        let b = colour::base();
        let f = colour::frame();
        c.fill(r, " ", b);
        if r.w < 40 || r.h < 12 {
            c.print(r.x + 1, r.y, "Too small for the file manager", b);
            return;
        }
        let stats_w = 24;
        let cmd_h = 3;
        let main = Rect::new(r.x, r.y + 1, r.w - stats_w, r.h - 1 - cmd_h);
        let side = Rect::new(main.right() - 1, r.y + 1, stats_w + 1, r.h - 1 - cmd_h);
        // Top line: the path, and the date and time.
        let dir = self.current_dir();
        c.print(r.x + 2, r.y, &format!("Path: {}", dir.display()), b);
        let now = chrono_like();
        c.print(r.right() - str_width(&now) as i32 - 1, r.y, &now, b);
        // The frames: tree above, files below, stats at the right.
        let split = if self.files_full { main.y } else { main.y + (main.h * 2 / 3).max(3) };
        let tl = |c: &mut Canvas, x, y, s: &str| {
            c.print(x, y, s, f);
        };
        // Outer box.
        turbine_ui::frame::draw_frame(c, Rect::new(main.x, main.y, main.w + stats_w, main.h), turbine_ui::theme::SINGLE, f);
        for y in main.y + 1..main.bottom() - 1 {
            tl(c, side.x, y, "│");
        }
        tl(c, side.x, main.y, "┬");
        tl(c, side.x, main.bottom() - 1, "┴");
        if !self.files_full {
            c.hline(main.x + 1, split, side.x - main.x - 1, "─", f);
            tl(c, main.x, split, "├");
            tl(c, side.x, split, "┤");
        }
        // The tree.
        self.tree_rect = if self.files_full { Rect::default() } else { Rect::new(main.x + 1, main.y + 1, side.x - main.x - 1, split - main.y - 1) };
        if !self.files_full {
            let h = self.tree_rect.h.max(1) as usize;
            if self.tree_sel < self.tree_top {
                self.tree_top = self.tree_sel;
            } else if self.tree_sel >= self.tree_top + h {
                self.tree_top = self.tree_sel + 1 - h;
            }
            for (row, i) in (self.tree_top..self.tree.len()).take(h).enumerate() {
                let (p, depth, last) = &self.tree[i];
                let y = self.tree_rect.y + row as i32;
                let mut prefix = String::new();
                if *depth > 0 {
                    // (Lines down from the parents still to come.)
                    for d in 1..*depth {
                        let ancestor_last = self.tree[..i].iter().rev().find(|t| t.1 == d).map(|t| t.2).unwrap_or(true);
                        prefix.push_str(if ancestor_last { "   " } else { "│  " });
                    }
                    prefix.push_str(if *last { "└──" } else { "├──" });
                }
                let name = if *depth == 0 { p.display().to_string() } else { p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() };
                let x = c.print(self.tree_rect.x + 1, y, &prefix, f);
                let x = c.print(x, y, &format!("{} ", crate::filetypes::icon(&name, true, self.expanded.contains(p))), colour::tagged());
                let st = if i == self.tree_sel { if focused && !self.in_files { colour::bar() } else { colour::bar_off() } } else { colour::text() };
                c.print(x, y, &name, st);
            }
        }
        // The files, in columns.
        self.files_rect = if self.files_full { Rect::new(main.x + 1, main.y + 1, side.x - main.x - 1, main.h - 2) } else { Rect::new(main.x + 1, split + 1, side.x - main.x - 1, main.bottom() - split - 2) };
        let files = self.files();
        let colw = 26usize;
        self.file_cols = ((self.files_rect.w as usize) / colw).max(1);
        let rows = self.files_rect.h.max(1) as usize;
        let page = rows * self.file_cols;
        if self.file_sel < self.file_top {
            self.file_top = self.file_sel / rows * rows;
        } else if self.file_sel >= self.file_top + page {
            self.file_top = (self.file_sel + 1).saturating_sub(page).div_ceil(rows) * rows;
        }
        if files.is_empty() {
            let note = if self.dirs.contains_key(&dir) { " No files match" } else { " Dir Not Logged" };
            c.print(self.files_rect.x + 1, self.files_rect.y, note, colour::text());
        }
        for (k, e) in files.iter().enumerate().skip(self.file_top).take(page) {
            let k0 = k - self.file_top;
            let (col, row) = (k0 / rows, k0 % rows);
            let x = self.files_rect.x + 1 + (col * colw) as i32;
            let y = self.files_rect.y + row as i32;
            let p = dir.join(&e.name);
            let tagged = self.tagged.contains(&p);
            let mark = if tagged { "♦" } else { " " };
            // (Its type's icon and colour, as the Explorer shows them.)
            let shown = format!("{} {}", crate::filetypes::icon(&e.name, false, false), e.name);
            let mut name: String = shown.chars().take(colw - 3).collect();
            if str_width(&name) < colw - 3 {
                name.push_str(&" ".repeat(colw - 3 - str_width(&name)));
            }
            let st = if k == self.file_sel && (self.in_files || self.files_full) {
                if focused { colour::bar() } else { colour::bar_off() }
            } else if tagged {
                colour::tagged()
            } else {
                Style::new(crate::filetypes::colour(&e.name, false), vga::BLUE)
            };
            c.print(x, y, &name, st);
            c.print(x + colw as i32 - 3, y, mark, colour::tagged());
        }
        // The statistics.
        let sx = side.x + 2;
        let mut y = side.y + 1;
        let line = |c: &mut Canvas, y: &mut i32, label: &str, value: String, indent: i32| {
            c.print(sx + indent, *y, label, b);
            if !value.is_empty() {
                c.print(side.right() - 2 - str_width(&value) as i32, *y, &value, colour::text());
            }
            *y += 1;
        };
        let all = self.dirs.get(&dir).cloned().unwrap_or_default();
        let total_files = all.iter().filter(|e| !e.dir).count() as u64;
        let total_bytes: u64 = all.iter().filter(|e| !e.dir).map(|e| e.size).sum();
        let match_bytes: u64 = files.iter().map(|e| e.size).sum();
        let tagged_here: Vec<&Entry> = files.iter().filter(|e| self.tagged.contains(&dir.join(&e.name))).collect();
        line(c, &mut y, &format!("FILE  {}", self.filespec), String::new(), 0);
        c.hline(side.x + 1, y, stats_w - 1, "─", f);
        y += 1;
        let avail = disk_free(&dir);
        line(c, &mut y, "DISK", String::new(), 0);
        line(c, &mut y, "Available", String::new(), 1);
        line(c, &mut y, "Bytes", avail.map(commas).unwrap_or_else(|| "?".into()), 2);
        c.hline(side.x + 1, y, stats_w - 1, "─", f);
        y += 1;
        line(c, &mut y, "DIRECTORY Stats", String::new(), 0);
        line(c, &mut y, "Total", String::new(), 1);
        line(c, &mut y, "Files", total_files.to_string(), 2);
        line(c, &mut y, "Bytes", commas(total_bytes), 2);
        line(c, &mut y, "Matching", String::new(), 1);
        line(c, &mut y, "Files", files.len().to_string(), 2);
        line(c, &mut y, "Bytes", commas(match_bytes), 2);
        line(c, &mut y, "Tagged", String::new(), 1);
        line(c, &mut y, "Files", tagged_here.len().to_string(), 2);
        line(c, &mut y, "Bytes", commas(tagged_here.iter().map(|e| e.size).sum()), 2);
        line(c, &mut y, if self.in_files { "Current File" } else { "Current Directory" }, String::new(), 1);
        let cur = if self.in_files { self.current_file().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())) } else { dir.file_name().map(|n| n.to_string_lossy().into_owned()).or(Some("/".into())) };
        line(c, &mut y, &cur.unwrap_or_default().chars().take(stats_w as usize - 4).collect::<String>(), String::new(), 2);
        let cur_bytes = if self.in_files { self.files().get(self.file_sel).map(|e| e.size).unwrap_or(0) } else { total_bytes };
        line(c, &mut y, "Bytes", commas(cur_bytes), 2);
        // The commands of the window in use, or the question being asked.
        let cy = r.bottom() - cmd_h;
        let k = colour::key();
        if let Some((what, text)) = &self.ask {
            let t = self.targets();
            let what_s = match what {
                Ask::Copy => format!("COPY {} to:", describe(&t)),
                Ask::Move => format!("MOVE {} to:", describe(&t)),
                Ask::Rename => "RENAME to:".to_string(),
                Ask::Delete => format!("DELETE {} (Y/N)?", describe(&t)),
                Ask::MakeDir => "MAKE DIRECTORY:".to_string(),
                Ask::Filespec => "FILESPEC:".to_string(),
            };
            c.print(r.x, cy, &what_s, colour::prompt());
            let x = c.print(r.x, cy + 1, " ", b);
            let x = c.print(x, cy + 1, text, colour::text());
            c.set_cursor(Point::new(x, cy + 1), turbine_ui::CursorShape::Underline);
            c.print(r.x, cy + 2, "◄┘ ok  ESC cancel", b);
            return;
        }
        let cmds: &[(&str, &str)] = if self.in_files {
            &[("C", "opy"), ("D", "elete"), ("E", "dit"), ("F", "ilespec"), ("I", "nvert"), ("M", "ove"), ("O", "pen"), ("R", "ename"), ("T", "ag"), ("U", "ntag"), ("V", "iew"), ("e", "X"), ("Q", "uit")]
        } else {
            &[("C", "opy"), ("D", "elete"), ("F", "ilespec"), ("I", "nvert"), ("L", "og"), ("M", "akedir"), ("R", "ename"), ("S", "howall"), ("T", "ag"), ("U", "ntag"), ("e", "X"), ("Q", "uit")]
        };
        let mut x = c.print(r.x, cy, if self.in_files { "FILE" } else { "DIR " }, k);
        x += 3;
        for (hot, rest) in cmds {
            // (eXecute: the X is the hotkey.)
            if *rest == "X" {
                let x1 = c.print(x, cy, "e", b);
                let x1 = c.print(x1, cy, "X", k);
                x = c.print(x1, cy, "ecute", b) + 2;
                continue;
            }
            let x1 = c.print(x, cy, hot, k);
            x = c.print(x1, cy, rest, b) + 2;
        }
        c.print(r.x, cy + 1, "COMMANDS", k);
        c.print(r.x + 10, cy + 1, "+ - * branches   Tab tree/files   Ctrl+T tag all   Ctrl+U untag all", b);
        let msg = if self.message.is_empty() { if self.in_files { "◄┘ more   F1 help   ESC cancel".to_string() } else { "◄┘ file   F1 help   ESC quit".to_string() } } else { self.message.clone() };
        c.print(r.x, cy + 2, &msg, if self.message.is_empty() { b } else { colour::text() });
    }
}

fn describe(t: &[PathBuf]) -> String {
    match t {
        [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        many => format!("{} tagged files", many.len()),
    }
}

/// The free space where a folder is (statvfs).
fn disk_free(p: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

/// The date and time as XTree shows them: 11-27-26  9:17:15 am.
fn chrono_like() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = secs as libc::time_t;
    unsafe { libc::localtime_r(&t, &mut tm) };
    let h = tm.tm_hour % 12;
    format!(
        "{:02}-{:02}-{:02}  {}:{:02}:{:02} {}",
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_year % 100,
        if h == 0 { 12 } else { h },
        tm.tm_min,
        tm.tm_sec,
        if tm.tm_hour < 12 { "am" } else { "pm" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use turbine_ui::event::Mods;

    fn wait(x: &mut XTree) {
        let t = std::time::Instant::now();
        loop {
            x.poll();
            if !x.busy() {
                return;
            }
            assert!(t.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn key(x: &mut XTree, k: Key) -> Out {
        x.key(&KeyEvent::new(k, Mods::NONE))
    }

    #[test]
    fn tree_files_tags_and_commands() {
        let dir = std::env::temp_dir().join(format!("turbine-xtree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("a.rs"), "a").unwrap();
        std::fs::write(dir.join("b.rs"), "bb").unwrap();
        std::fs::write(dir.join("c.txt"), "ccc").unwrap();
        let mut x = XTree::new(&dir);
        wait(&mut x);
        // The root, then its folders as branches.
        let names: Vec<String> = x.tree.iter().map(|t| t.0.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names[1..], ["docs".to_string(), "src".to_string()]);
        assert_eq!(x.files().len(), 3);
        // Filespec *.rs.
        key(&mut x, Key::Char('f'));
        for _ in 0..3 {
            key(&mut x, Key::Backspace);
        }
        for c in "*.rs".chars() {
            key(&mut x, Key::Char(c));
        }
        key(&mut x, Key::Enter);
        assert_eq!(x.files().len(), 2);
        // Enter: the files; tag both; copy them into docs.
        key(&mut x, Key::Enter);
        key(&mut x, Key::Char('t'));
        key(&mut x, Key::Char('t'));
        assert_eq!(x.tagged.len(), 2);
        key(&mut x, Key::Char('c'));
        if let Some((_, t)) = &mut x.ask {
            *t = dir.join("docs").display().to_string();
        }
        key(&mut x, Key::Enter);
        let t = std::time::Instant::now();
        while !dir.join("docs/b.rs").exists() {
            x.poll();
            assert!(t.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(dir.join("docs/a.rs").exists());
        // Rename the current file.
        key(&mut x, Key::Home);
        key(&mut x, Key::Char('r'));
        if let Some((_, t)) = &mut x.ask {
            *t = "z.rs".into();
        }
        key(&mut x, Key::Enter);
        assert!(dir.join("z.rs").exists() && !dir.join("a.rs").exists());
        // E edits, X executes.
        wait(&mut x);
        key(&mut x, Key::Home);
        assert!(matches!(key(&mut x, Key::Char('e')), Out::Edit(p) if p.ends_with("b.rs")));
        // Back to the tree; * opens every branch.
        key(&mut x, Key::Esc);
        key(&mut x, Key::Char('*'));
        wait(&mut x);
        assert!(x.tree.iter().any(|t| t.0.ends_with("src/deep")));
        // Make a directory.
        key(&mut x, Key::Home);
        key(&mut x, Key::Char('m'));
        for c in "new".chars() {
            key(&mut x, Key::Char(c));
        }
        key(&mut x, Key::Enter);
        wait(&mut x);
        assert!(x.tree.iter().any(|t| t.0.ends_with("new")));
        assert_eq!(key(&mut x, Key::Esc), Out::Close);
        // Drawing at 80x25 fits.
        let mut surf = turbine_ui::Surface::new(80, 25);
        x.draw(&mut surf.canvas(), Rect::new(0, 0, 80, 25), true);
        let t = surf.to_text();
        assert!(t.contains("Path: ") && t.contains("DIRECTORY Stats") && t.contains("COMMANDS"), "{t}");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches("*.rs", "a.RS") && !matches("*.rs", "a.txt") && matches("*.*", "x"));
        assert_eq!(commas(262111744), "262,111,744");
    }
}
