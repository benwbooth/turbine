//! The side bar and panel views: Explorer, Outline, Search, Source Control.
//! Anything that reads the disk or runs git does it on a worker thread and
//! hands the result back (`poll`), so the UI never waits on it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use turbine_syntax::Symbol;
use turbine_text::unicode::str_width;
use turbine_ui::event::{Key, KeyEvent};
use turbine_ui::list::{ListView, Row};
use turbine_ui::theme::{Theme, vga};
use turbine_ui::{Canvas, CursorShape, Point, Rect};

/// Folders nobody wants to see or search.
fn skipped(name: &str) -> bool {
    matches!(name, ".git" | ".hg" | ".svn")
}

/// Folders a search skips too (build output, dependencies).
fn bulky(name: &str) -> bool {
    skipped(name) || matches!(name, "target" | "node_modules" | ".direnv" | "result" | "__pycache__" | ".venv" | "dist" | "build")
}

// ---- a one-line text field ---------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct LineInput {
    pub text: String,
    /// Byte offset.
    pub cursor: usize,
}

impl LineInput {
    pub fn set(&mut self, t: &str) {
        self.text = t.to_string();
        self.cursor = self.text.len();
    }

    /// Edit keys; true when the text or cursor changed.
    pub fn key(&mut self, k: &KeyEvent) -> bool {
        let prev = |s: &str, i: usize| s[..i].char_indices().next_back().map_or(0, |(j, _)| j);
        let next = |s: &str, i: usize| s[i..].chars().next().map_or(i, |c| i + c.len_utf8());
        match k.key {
            Key::Char(c) if !k.mods.ctrl && !k.mods.alt => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            Key::Backspace if self.cursor > 0 => {
                let p = prev(&self.text, self.cursor);
                self.text.replace_range(p..self.cursor, "");
                self.cursor = p;
            }
            Key::Delete if self.cursor < self.text.len() => {
                let n = next(&self.text, self.cursor);
                self.text.replace_range(self.cursor..n, "");
            }
            Key::Left => self.cursor = prev(&self.text, self.cursor),
            Key::Right => self.cursor = next(&self.text, self.cursor),
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = self.text.len(),
            _ => return false,
        }
        true
    }

    /// The field on one row, a hint shown while it's empty.
    pub fn draw(&self, c: &mut Canvas, r: Rect, theme: &Theme, focused: bool, hint: &str) {
        let s = &theme.scheme;
        c.fill(r, " ", s.input);
        let mut fc = c.sub(r);
        if self.text.is_empty() && !hint.is_empty() {
            fc.print(r.x + 1, r.y, hint, turbine_ui::theme::Style::new(s.editor_gutter.fg, s.input.bg));
        }
        let before = str_width(&self.text[..self.cursor]) as i32;
        let skip = (before - r.w + 3).max(0);
        fc.print(r.x + 1 - skip, r.y, &self.text, s.input);
        if focused {
            c.set_cursor(Point::new(r.x + 1 + before - skip, r.y), CursorShape::Bar);
        }
    }
}

// ---- Explorer -------------------------------------------------------------------------

/// The project's files as a tree: folders open and close in place, and the
/// folder listings are read in the background.
pub struct Explorer {
    pub root: PathBuf,
    /// More folders in the workspace, each its own tree under the first
    /// (File > Add Folder to Workspace).
    pub extra: Vec<PathBuf>,
    pub expanded: HashSet<PathBuf>,
    /// Workspace folders folded to their heading (▸).
    pub closed_roots: HashSet<PathBuf>,
    children: HashMap<PathBuf, Vec<(String, bool)>>,
    pending: HashSet<PathBuf>,
    tx: Sender<(PathBuf, Vec<(String, bool)>, Vec<(PathBuf, Meta)>)>,
    rx: Receiver<(PathBuf, Vec<(String, bool)>, Vec<(PathBuf, Meta)>)>,
    /// Each listed entry's details (read with its folder's listing), for
    /// tooltips.
    meta: HashMap<PathBuf, Meta>,
    /// Folders' whole sizes, asked for (Calculate Size): (bytes, files), or
    /// None while counting.
    pub sizes: HashMap<PathBuf, Option<(u64, u64)>>,
    size_tx: Sender<(PathBuf, (u64, u64))>,
    size_rx: Receiver<(PathBuf, (u64, u64))>,
    pub list: ListView,
    /// The path of each row (row 0: the root).
    paths: Vec<(PathBuf, bool)>,
    refreshed: Instant,
    /// Select this path once its folder is listed.
    want: Option<PathBuf>,
    /// List names starting with a dot.
    pub show_hidden: bool,
    /// Git's status of changed paths (absolute), and of the folders holding
    /// them; read in the background with the listings.
    git: HashMap<PathBuf, char>,
    git_rx: Option<Receiver<HashMap<PathBuf, char>>>,
}

/// A listed file's details.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
    pub mode: u32,
    /// Where a symbolic link points.
    pub link: Option<PathBuf>,
}

fn meta_of(p: &Path) -> Option<Meta> {
    use std::os::unix::fs::PermissionsExt;
    let lm = std::fs::symlink_metadata(p).ok()?;
    let link = lm.file_type().is_symlink().then(|| std::fs::read_link(p).ok()).flatten();
    let m = if link.is_some() { std::fs::metadata(p).unwrap_or(lm) } else { lm };
    Some(Meta { size: m.len(), modified: m.modified().ok(), mode: m.permissions().mode(), link })
}

/// 1.2 KB, 3.4 MB (powers of 1024).
pub fn human_size(n: u64) -> String {
    let units = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < units.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} bytes") } else if v < 10.0 { format!("{v:.1} {}", units[u]) } else { format!("{v:.0} {}", units[u]) }
}

/// A time as the local date and time (2026-11-27 10:41).
pub fn local_time(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as libc::time_t).unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r writes only into tm.
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return String::new();
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
}

/// rwxr-xr-x.
pub fn perms(mode: u32) -> String {
    let mut s = String::new();
    for shift in [6, 3, 0] {
        let b = (mode >> shift) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    s
}

impl Explorer {
    pub fn new(root: PathBuf) -> Explorer {
        let (tx, rx) = channel();
        let (size_tx, size_rx) = channel();
        let mut e = Explorer {
            root: root.clone(),
            extra: Vec::new(),
            expanded: HashSet::from([root.clone()]),
            closed_roots: HashSet::new(),
            children: HashMap::new(),
            pending: HashSet::new(),
            tx,
            rx,
            meta: HashMap::new(),
            sizes: HashMap::new(),
            size_tx,
            size_rx,
            list: {
                let mut l = ListView::new();
                l.bar_when_focused = true;
                l
            },
            paths: Vec::new(),
            refreshed: Instant::now(),
            want: None,
            show_hidden: false,
            git: HashMap::new(),
            git_rx: None,
        };
        e.request(&root);
        e.read_git();
        e.rebuild();
        e
    }

    pub fn set_root(&mut self, root: PathBuf) {
        if root != self.root {
            let hidden = self.show_hidden;
            *self = Explorer::new(root);
            self.show_hidden = hidden;
        }
    }

    /// Count a folder's whole size in the background (links not followed;
    /// other file systems not entered).
    pub fn count_size(&mut self, dir: &Path) {
        self.sizes.insert(dir.to_path_buf(), None);
        self.rebuild();
        let tx = self.size_tx.clone();
        let dir = dir.to_path_buf();
        std::thread::spawn(move || {
            use std::os::unix::fs::MetadataExt;
            let dev = std::fs::symlink_metadata(&dir).map(|m| m.dev()).unwrap_or(0);
            let (mut bytes, mut files) = (0u64, 0u64);
            let mut stack = vec![dir.clone()];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let Ok(m) = e.metadata() else { continue };
                    if m.is_dir() {
                        if m.dev() == dev {
                            stack.push(e.path());
                        }
                    } else {
                        // (Blocks used, as du counts: sparse files aren't their length.)
                        bytes += m.blocks() * 512;
                        files += 1;
                    }
                }
            }
            let _ = tx.send((dir, (bytes, files)));
        });
    }

    /// A row's tooltip: where it is, its size, when it changed, its
    /// permissions and git's status.
    pub fn describe(&self, i: usize) -> Option<String> {
        if self.is_up(i) {
            return Some(format!("Up to {}", self.root.parent()?.display()));
        }
        let (p, is_dir) = self.paths.get(i)?;
        let mut lines = vec![p.display().to_string()];
        let mut facts: Vec<String> = Vec::new();
        if let Some(m) = self.meta.get(p) {
            if *is_dir {
                let n = self.children.get(p).map(|k| k.len());
                facts.push(match n {
                    Some(n) => format!("Folder, {n} item{}", if n == 1 { "" } else { "s" }),
                    None => "Folder".into(),
                });
            } else {
                // (What it is: its language, else its kind of file.)
                let kind = turbine_syntax::detect(Some(p), "").map(|l| format!("{} file", turbine_syntax::label(&l))).unwrap_or_else(|| match crate::filekind::kind_by_name(p) {
                    Some(k) => k.to_string(),
                    None => "File".into(),
                });
                facts.push(kind);
                facts.push(human_size(m.size));
            }
            if let Some(t) = m.modified {
                facts.push(format!("modified {}", local_time(t)));
            }
            facts.push(perms(m.mode));
            if let Some(l) = &m.link {
                lines.push(format!("Link to {}", l.display()));
            }
        } else if *is_dir {
            facts.push("Folder".into());
        }
        if !facts.is_empty() {
            lines.push(facts.join("  ·  "));
        }
        if *is_dir {
            match self.sizes.get(p) {
                Some(Some((b, f))) => lines.push(format!("Size: {} in {f} file{}", human_size(*b), if *f == 1 { "" } else { "s" })),
                Some(None) => lines.push("Size: counting…".into()),
                None => lines.push("Right-click > Calculate Size for its whole size".into()),
            }
        }
        if let Some(code) = self.git.get(p) {
            lines.push(format!(
                "Git: {}",
                match code {
                    'M' => "changed",
                    'A' => "added",
                    'U' => "untracked (new)",
                    'I' => "ignored (.gitignore)",
                    'D' => "deleted",
                    'R' => "renamed",
                    'C' => "in conflict",
                    _ => "has changes inside",
                }
            ));
        }
        Some(lines.join("\n"))
    }

    /// Folders open last time (the session), read again.
    pub fn restore_expanded(&mut self, dirs: impl IntoIterator<Item = PathBuf>) {
        for d in dirs {
            if self.roots().iter().any(|r| d.starts_with(r)) && d.is_dir() && self.expanded.insert(d.clone()) {
                self.request(&d);
            }
        }
    }

    fn request(&mut self, dir: &Path) {
        if !self.pending.insert(dir.to_path_buf()) {
            return;
        }
        let tx = self.tx.clone();
        let dir = dir.to_path_buf();
        std::thread::spawn(move || {
            let mut v: Vec<(String, bool)> = std::fs::read_dir(&dir)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path().is_dir()))
                        .filter(|(n, _)| !skipped(n))
                        .collect()
                })
                .unwrap_or_default();
            v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
            // (Each one's details too, for its tooltip.)
            let metas: Vec<(PathBuf, Meta)> = v.iter().filter_map(|(n, _)| meta_of(&dir.join(n)).map(|m| (dir.join(n), m))).collect();
            let _ = tx.send((dir, v, metas));
        });
    }

    /// Git's status for the project, off the UI thread.
    fn read_git(&mut self) {
        if self.git_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.git_rx = Some(rx);
        let roots = self.roots();
        std::thread::spawn(move || {
            let mut out = HashMap::new();
            for root in roots {
            let top = std::process::Command::new("git").arg("-C").arg(&root).args(["rev-parse", "--show-toplevel"]).output().ok().filter(|o| o.status.success()).map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
            if let Some(top) = top {
                if let Ok(o) = std::process::Command::new("git").arg("-C").arg(&top).args(["status", "--porcelain=v1", "-z", "--untracked-files=normal", "--ignored=matching"]).output() {
                    for (path, code) in crate::filetypes::parse_git_status(&String::from_utf8_lossy(&o.stdout)) {
                        let p = top.join(&path);
                        if code == 'I' {
                            out.insert(p, code);
                            continue;
                        }
                        // (Its folders show that something in them changed.)
                        let mut d = p.parent();
                        while let Some(x) = d {
                            if !x.starts_with(&top) {
                                break;
                            }
                            out.entry(x.to_path_buf()).or_insert('•');
                            d = x.parent();
                        }
                        out.insert(p, code);
                    }
                }
            }
            }
            let _ = tx.send(out);
        });
    }

    /// Take finished listings; re-list open folders every few seconds while
    /// shown (files come and go). True when the tree changed.
    pub fn poll(&mut self, shown: bool) -> bool {
        let mut changed = false;
        while let Ok((dir, size)) = self.size_rx.try_recv() {
            self.sizes.insert(dir, Some(size));
            changed = true;
        }
        while let Ok((dir, v, metas)) = self.rx.try_recv() {
            self.pending.remove(&dir);
            self.meta.extend(metas);
            if self.children.get(&dir) != Some(&v) {
                self.children.insert(dir, v);
                changed = true;
            }
        }
        if let Some(rx) = &self.git_rx {
            if let Ok(g) = rx.try_recv() {
                self.git_rx = None;
                if g != self.git {
                    self.git = g;
                    changed = true;
                }
            }
        }
        if shown && self.refreshed.elapsed() > Duration::from_secs(3) {
            self.refreshed = Instant::now();
            for d in self.expanded.clone() {
                self.request(&d);
            }
            self.read_git();
        }
        if changed {
            self.rebuild();
        }
        changed
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let mut paths = Vec::new();
        // "..": up a folder (the project becomes its parent); not in a
        // workspace of several folders.
        if let (Some(parent), true) = (self.root.parent(), self.extra.is_empty()) {
            rows.push(Row::new("..").glyph(crate::filetypes::icon("..", true, false)).fg(turbine_ui::theme::vga::LIGHT_CYAN).bold());
            paths.push((parent.to_path_buf(), true));
        }
        // Each folder of the workspace: its name, then its tree.
        for root in self.roots() {
            let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
            // (Each folds to its heading, as VS Code's workspace folders do.)
            let open = !self.closed_roots.contains(&root);
            rows.push(Row::new(name).heading().icon(if open { "▾" } else { "▸" }));
            paths.push((root.clone(), true));
            if open {
                self.walk(&root, 0, &mut rows, &mut paths);
            }
        }
        self.paths = paths;
        self.list.set_rows(rows);
        if let Some(w) = self.want.clone() {
            if let Some(i) = self.paths.iter().position(|(p, _)| *p == w) {
                self.list.select(i);
                self.want = None;
            }
        }
    }

    fn walk(&self, dir: &Path, depth: u16, rows: &mut Vec<Row>, paths: &mut Vec<(PathBuf, bool)>) {
        let Some(kids) = self.children.get(dir) else {
            if self.pending.contains(dir) {
                rows.push(Row::new("…").indent(depth + 1));
                paths.push((dir.to_path_buf(), true));
            }
            return;
        };
        for (name, is_dir) in kids {
            if !self.show_hidden && name.starts_with('.') {
                continue;
            }
            let p = dir.join(name);
            let open = self.expanded.contains(&p);
            let icon = if *is_dir { if open { "▾" } else { "▸" } } else { " " };
            // Its type's icon and colour, and git's mark.
            let mut row = Row::new(name.clone()).indent(depth).icon(icon).glyph(crate::filetypes::icon(name, *is_dir, open)).fg(crate::filetypes::colour(name, *is_dir));
            if *is_dir {
                row = row.bold();
            }
            // (Ignored by git, or in an ignored folder: dim, as VS Code shows them.)
            let ignored = p.ancestors().take_while(|a| a.starts_with(&self.root) || self.extra.iter().any(|e| a.starts_with(e))).any(|a| self.git.get(a) == Some(&'I'));
            if ignored {
                row = row.fg(vga::DARK_GRAY);
            } else if let Some(code) = self.git.get(&p) {
                let (b, c) = crate::filetypes::git_badge(*code);
                row = row.badge(b, c);
                // (A changed file's name takes git's colour, as in VS Code.)
                if !*is_dir {
                    row = row.fg(c);
                }
            }
            // (A folder's counted size, once asked for.)
            match self.sizes.get(&p) {
                Some(Some((bytes, _))) => row = row.detail(human_size(*bytes)),
                Some(None) => row = row.detail("…".to_string()),
                None => {}
            }
            rows.push(row);
            paths.push((p.clone(), *is_dir));
            if *is_dir && open {
                self.walk(&p, depth + 1, rows, paths);
            }
        }
    }

    /// The person chose a row: forget a selection still waiting for its
    /// folder's listing.
    pub fn user_selected(&mut self) {
        self.want = None;
    }

    pub fn path_at(&self, i: usize) -> Option<&(PathBuf, bool)> {
        self.paths.get(i)
    }

    pub fn set_show_hidden(&mut self, on: bool) {
        self.show_hidden = on;
        self.rebuild();
    }

    /// Read every open folder again (after files changed).
    pub fn refresh_all(&mut self) {
        self.refreshed = Instant::now();
        for d in self.expanded.clone() {
            self.request(&d);
        }
        self.read_git();
    }

    /// Close every folder.
    pub fn collapse_all(&mut self) {
        self.expanded = HashSet::from([self.root.clone()]);
        self.rebuild();
        self.list.select(0);
    }

    /// Open or close a folder.
    pub fn toggle(&mut self, dir: &Path) {
        if self.roots().iter().any(|r| r == dir) {
            if !self.closed_roots.remove(dir) {
                self.closed_roots.insert(dir.to_path_buf());
            }
            self.rebuild();
            return;
        }
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_path_buf());
            self.request(dir);
        }
        self.rebuild();
    }

    /// Enter / double-click on row `i`: a folder opens or closes; a file is
    /// returned to be opened.
    /// The file on a row (None for a folder).
    pub fn file_at(&self, i: usize) -> Option<PathBuf> {
        self.paths.get(i).filter(|(_, dir)| !dir).map(|(p, _)| p.clone())
    }

    /// The ".." row (up a folder), when there is one.
    pub fn is_up(&self, i: usize) -> bool {
        i == 0 && self.root.parent().is_some() && self.extra.is_empty()
    }

    /// The row of the project folder itself.
    pub fn root_row(&self) -> usize {
        usize::from(self.root.parent().is_some() && self.extra.is_empty())
    }

    /// The workspace's folders: the project, then those added.
    pub fn roots(&self) -> Vec<PathBuf> {
        std::iter::once(self.root.clone()).chain(self.extra.iter().cloned()).collect()
    }

    /// Whether a row is one of the workspace's folders (its heading).
    pub fn is_root_row(&self, i: usize) -> bool {
        self.paths.get(i).is_some_and(|(p, _)| self.roots().contains(p))
    }

    /// Another folder beside the project (its tree read in the background).
    pub fn add_root(&mut self, dir: PathBuf) {
        if dir == self.root || self.extra.contains(&dir) {
            return;
        }
        self.extra.push(dir.clone());
        self.expanded.insert(dir.clone());
        self.request(&dir);
        self.git_rx = None;
        self.read_git();
        self.rebuild();
    }

    /// A folder out of the workspace (not the project itself).
    pub fn remove_root(&mut self, dir: &Path) {
        self.extra.retain(|d| d != dir);
        self.rebuild();
    }

    pub fn activate(&mut self, i: usize) -> Option<PathBuf> {
        if self.is_up(i) {
            return None;
        }
        let (p, dir) = self.paths.get(i)?.clone();
        if dir {
            self.toggle(&p);
            None
        } else {
            Some(p)
        }
    }

    /// Left: close the folder (or go to the parent); Right: open it.
    pub fn collapse(&mut self, i: usize) {
        let Some((p, dir)) = self.paths.get(i).cloned() else { return };
        if self.is_root_row(i) {
            if !self.closed_roots.contains(&p) {
                self.toggle(&p);
            }
            return;
        }
        if dir && self.expanded.contains(&p) && p != self.root {
            self.toggle(&p);
        } else if let Some(parent) = p.parent() {
            if let Some(j) = self.paths.iter().position(|(q, _)| q == parent) {
                self.list.select(j);
            }
        }
    }

    pub fn expand(&mut self, i: usize) {
        if self.is_root_row(i) {
            let p = self.paths[i].0.clone();
            if self.closed_roots.contains(&p) {
                self.toggle(&p);
            }
            return;
        }
        if let Some((p, true)) = self.paths.get(i).cloned() {
            if !self.expanded.contains(&p) {
                self.toggle(&p);
            }
        }
    }

    /// Open a folder and the folders down to it, and select it.
    pub fn expand_path(&mut self, dir: &Path) {
        let Ok(rel) = dir.strip_prefix(&self.root) else { return };
        let mut d = self.root.clone();
        for c in rel.components() {
            d.push(c);
            if self.expanded.insert(d.clone()) {
                self.request(&d);
            }
        }
        self.want = Some(dir.to_path_buf());
        self.rebuild();
    }

    /// Show a file: open the folders down to it and select it.
    pub fn reveal(&mut self, file: &Path) {
        // (Its workspace folder unfolded first.)
        if let Some(r) = self.roots().into_iter().find(|r| file.starts_with(r)) {
            if self.closed_roots.remove(&r) {
                self.rebuild();
            }
        }
        let Ok(rel) = file.strip_prefix(&self.root) else { return };
        let mut d = self.root.clone();
        let parts: Vec<_> = rel.components().collect();
        for c in &parts[..parts.len().saturating_sub(1)] {
            d.push(c);
            if self.expanded.insert(d.clone()) {
                self.request(&d);
            }
        }
        self.want = Some(file.to_path_buf());
        self.rebuild();
    }
}

// ---- Outline --------------------------------------------------------------------------

/// The active editor's definitions, worked out in the background.
pub struct Outline {
    pub list: ListView,
    /// The line of each row.
    pub lines: Vec<usize>,
    /// (window id, document version) shown, and asked for.
    shown: Option<(u32, u64)>,
    asked: Option<(u32, u64)>,
    /// When it last asked (typing waits a moment before asking again).
    asked_at: Option<Instant>,
    tx: Sender<(u32, u64, Option<Vec<Symbol>>)>,
    rx: Receiver<(u32, u64, Option<Vec<Symbol>>)>,
}

impl Default for Outline {
    fn default() -> Self {
        let (tx, rx) = channel();
        Outline { list: ListView::new(), lines: Vec::new(), shown: None, asked: None, asked_at: None, tx, rx }
    }
}

fn symbol_icon(kind: &str) -> &'static str {
    match kind {
        "function" | "method" => "ƒ",
        "class" | "struct" => "■",
        "enum" => "≡",
        "trait" | "interface" => "♦",
        "impl" => "◘",
        "module" => "§",
        "type" => "τ",
        "constant" => "π",
        "heading" => "#",
        _ => "∙",
    }
}

/// A symbol's colour by kind, as the code's colours have them (functions
/// light cyan, types light red, constants magenta, modules yellow).
fn symbol_colour(kind: &str) -> turbine_ui::theme::Color {
    use turbine_ui::theme::vga::*;
    match kind {
        "function" | "method" => LIGHT_CYAN,
        "class" | "struct" | "enum" | "trait" | "interface" | "type" => LIGHT_RED,
        "impl" | "module" => YELLOW,
        "constant" => LIGHT_MAGENTA,
        "heading" => YELLOW,
        _ => LIGHT_GREEN,
    }
}

impl Outline {
    /// Ask for the outline of this editor's text (when it changed).
    pub fn update(&mut self, win: u32, version: u64, lang: Option<&str>, text: impl FnOnce() -> String) {
        // (The language is part of what's asked for: a file's language is
        // known a moment after it opens, and then it's worth a look.)
        let version = version.wrapping_mul(2) + u64::from(lang.is_some());
        let key = Some((win, version));
        if self.asked == key {
            return;
        }
        // (The same file being typed in: wait a moment between parses.)
        let same_file = self.asked.is_some_and(|(w, _)| w == win);
        if same_file && self.asked_at.is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
            return;
        }
        self.asked = key;
        self.asked_at = Some(Instant::now());
        let Some(lang) = lang.map(str::to_string) else {
            self.lines.clear();
            self.list.set_rows(vec![Row::new("No outline for this file")]);
            self.shown = key;
            return;
        };
        let text = text();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let syms = turbine_syntax::symbols(&lang, &text, Duration::from_millis(500));
            let _ = tx.send((win, version, syms));
        });
    }

    pub fn clear(&mut self) {
        self.asked = None;
        self.shown = None;
        self.lines.clear();
        self.list.set_rows(Vec::new());
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok((win, version, syms)) = self.rx.try_recv() {
            if self.asked != Some((win, version)) {
                continue;
            }
            self.shown = Some((win, version));
            let syms = syms.unwrap_or_default();
            self.lines = syms.iter().map(|s| s.line).collect();
            let rows = if syms.is_empty() {
                vec![Row::new("No symbols found")]
            } else {
                syms.iter().map(|s| Row::new(s.name.clone()).indent(s.depth as u16).icon(symbol_icon(s.kind)).fg(symbol_colour(s.kind)).detail(format!("{}", s.line + 1))).collect()
            };
            self.list.set_rows(rows);
            changed = true;
        }
        changed
    }

    /// The outline is of this window.
    pub fn shows(&self, win: u32) -> bool {
        self.shown.is_some_and(|(w, _)| w == win)
    }

    /// The definitions the line is in, outermost first: (icon, name, line).
    pub fn chain(&self, line: usize) -> Vec<(String, String, usize)> {
        let mut stack: Vec<(u16, String, String, usize)> = Vec::new();
        for (row, &l) in self.list.rows.iter().zip(&self.lines) {
            if l > line {
                break;
            }
            stack.retain(|(d, ..)| *d < row.indent);
            stack.push((row.indent, row.icon.clone(), row.text.clone(), l));
        }
        stack.into_iter().map(|(_, i, n, l)| (i, n, l)).collect()
    }

    /// The row for the cursor's line (the definition it's in).
    pub fn follow(&mut self, line: usize) {
        if let Some(i) = self.lines.iter().rposition(|&l| l <= line) {
            if self.list.selected != i {
                self.list.select(i);
            }
        }
    }
}

// ---- Search ---------------------------------------------------------------------------

/// Find in Files: a query field over the results, grouped by file.
pub struct Search {
    pub query: LineInput,
    pub list: ListView,
    /// The place of each row (None for a file's heading).
    pub hits: Vec<Option<(PathBuf, usize)>>,
    /// Typing goes to the field (else to the results).
    pub in_field: bool,
    pub status: String,
    rx: Option<Receiver<Vec<Hit>>>,
    started: Option<Instant>,
}

/// A search result: the file, its line, the line's text, its syntax
/// colours and where the query is in it (byte ranges).
type Hit = (PathBuf, usize, String, Vec<(u32, u32, u8)>, Vec<(u32, u32)>);

/// Results coloured as the editor colours code, and the query marked in
/// them (case-insensitively). Runs on the search's thread.
fn colour_hits(found: Vec<(PathBuf, usize, String)>, query: Option<&str>) -> Vec<Hit> {
    let mut langs: HashMap<PathBuf, Option<String>> = HashMap::new();
    let q = query.map(str::to_lowercase).filter(|q| !q.is_empty());
    found
        .into_iter()
        .enumerate()
        .map(|(n, (p, line, text))| {
            let lang = langs.entry(p.clone()).or_insert_with(|| turbine_syntax::detect(Some(&p), "")).clone();
            // (A few hundred: past that, plain, so a big search stays quick.)
            let syntax = match lang {
                Some(l) if n < 600 => turbine_syntax::highlight(&l, &text, Duration::from_millis(10)).and_then(|v| v.into_iter().next()).unwrap_or_default(),
                _ => Vec::new(),
            };
            let mut marks = Vec::new();
            if let Some(q) = &q {
                let lower = text.to_lowercase();
                // (Lowercasing kept the bytes where they were: mark them.)
                if lower.len() == text.len() {
                    let mut from = 0;
                    while let Some(i) = lower[from..].find(q.as_str()) {
                        let a = from + i;
                        marks.push((a as u32, (a + q.len()) as u32));
                        from = a + q.len().max(1);
                    }
                }
            }
            // (A match far along the line: the start cut, so it shows in a
            // narrow side bar, as VS Code's results do.)
            const KEEP: usize = 10;
            if let Some(&(first, _)) = marks.first() {
                let first = first as usize;
                if text[..first].chars().count() > KEEP + 4 {
                    let cut = text[..first].char_indices().rev().nth(KEEP - 1).map_or(0, |(i, _)| i);
                    let shift = cut as u32;
                    let ell = '…'.len_utf8() as u32;
                    let text = format!("…{}", &text[cut..]);
                    let syntax = syntax.into_iter().filter(|x| x.1 > shift).map(|(a, b, k)| (a.max(shift) - shift + ell, b - shift + ell, k)).collect();
                    let marks = marks.into_iter().map(|(a, b)| (a - shift + ell, b - shift + ell)).collect();
                    return (p, line, text, syntax, marks);
                }
            }
            (p, line, text, syntax, marks)
        })
        .collect()
}

impl Default for Search {
    fn default() -> Self {
        Search { query: LineInput::default(), list: ListView::new(), hits: Vec::new(), in_field: true, status: String::new(), rx: None, started: None }
    }
}

const MAX_HITS: usize = 2000;

impl Search {
    /// Search `root` for the query (case-insensitively), in the background.
    pub fn run(&mut self, root: &Path) {
        let q = self.query.text.trim().to_lowercase();
        if q.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.started = Some(Instant::now());
        self.status = "Searching…".into();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let mut hits = Vec::new();
            let mut stack = vec![root];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                let mut entries: Vec<_> = rd.flatten().collect();
                entries.sort_by_key(|e| e.file_name());
                for e in entries.into_iter().rev() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let p = e.path();
                    let Ok(ft) = e.file_type() else { continue };
                    if ft.is_dir() {
                        if !bulky(&name) {
                            stack.push(p);
                        }
                        continue;
                    }
                    if e.metadata().map(|m| m.len() > 4 << 20).unwrap_or(true) {
                        continue;
                    }
                    let Ok(bytes) = std::fs::read(&p) else { continue };
                    if bytes.iter().take(8000).any(|&b| b == 0) {
                        continue;
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    for (n, line) in text.lines().enumerate() {
                        if line.to_lowercase().contains(&q) {
                            hits.push((p.clone(), n, line.trim().chars().take(200).collect()));
                            if hits.len() >= MAX_HITS {
                                let _ = tx.send(colour_hits(hits, Some(&q)));
                                return;
                            }
                        }
                    }
                }
            }
            let _ = tx.send(colour_hits(hits, Some(&q)));
        });
    }

    pub fn poll(&mut self, root: &Path) -> bool {
        let Some(rx) = &self.rx else { return false };
        let Ok(found) = rx.try_recv() else { return false };
        self.rx = None;
        let files: HashSet<&PathBuf> = found.iter().map(|h| &h.0).collect();
        self.status = format!(
            "{}{} results in {} files",
            if found.len() >= MAX_HITS { "over " } else { "" },
            found.len(),
            files.len()
        );
        let mut rows = Vec::new();
        let mut hits = Vec::new();
        let mut last: Option<&PathBuf> = None;
        let mut counts: HashMap<&PathBuf, usize> = HashMap::new();
        for h in &found {
            *counts.entry(&h.0).or_default() += 1;
        }
        for (p, line, text, syntax, marks) in &found {
            if last != Some(p) {
                // (The file as the Explorer shows it: its icon and colour,
                // its folder dim after it, how many lines matched.)
                let rel = p.strip_prefix(root).unwrap_or(p);
                let name = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| rel.display().to_string());
                let dir = rel.parent().map(|d| d.display().to_string()).unwrap_or_default();
                let row = Row::new(name.clone())
                    .heading()
                    .icon("▾")
                    .glyph(crate::filetypes::icon(&name, false, false))
                    .fg(crate::filetypes::colour(&name, false))
                    .detail(dir)
                    .badge(counts[p].to_string(), vga::LIGHT_GRAY);
                rows.push(row);
                hits.push(None);
                last = Some(p);
            }
            rows.push(Row::new(text.clone()).indent(1).detail(format!("{}", line + 1)).syntax(syntax.clone()).marks(marks.clone()));
            hits.push(Some((p.clone(), *line)));
        }
        self.hits = hits;
        self.list.set_rows(rows);
        self.list.select(1);
        true
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Show places found elsewhere (Go to References) as results: each
    /// line's text is read in the background.
    pub fn show_places(&mut self, title: &str, places: Vec<(PathBuf, usize)>) {
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.query.set(title);
        self.in_field = false;
        std::thread::spawn(move || {
            let mut cache: HashMap<PathBuf, Vec<String>> = HashMap::new();
            let mut hits = Vec::new();
            for (p, line) in places {
                let lines = cache.entry(p.clone()).or_insert_with(|| std::fs::read_to_string(&p).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default());
                let text = lines.get(line).map(|l| l.trim().chars().take(200).collect()).unwrap_or_default();
                hits.push((p, line, text));
            }
            let _ = tx.send(colour_hits(hits, None));
        });
    }
}

// ---- Source Control -------------------------------------------------------------------

/// One workspace folder's repository, as last read.
#[derive(Clone, Debug, Default)]
pub struct Repo {
    /// The workspace folder.
    pub folder: PathBuf,
    /// The repository's top folder (git's paths are from it), once known.
    pub top: Option<PathBuf>,
    pub branch: String,
    /// The upstream branch (origin/main), if it has one.
    pub upstream: Option<String>,
    /// Commits ahead of and behind the upstream.
    pub ahead: u32,
    pub behind: u32,
    /// (git letter, path from the top): conflicts, staged, and the rest.
    pub conflicts: Vec<(char, String)>,
    pub staged: Vec<(char, String)>,
    pub changes: Vec<(char, String)>,
    /// Why there's nothing to show (not a git repository…).
    pub error: Option<String>,
    busy: bool,
    refreshed: Option<Instant>,
}

impl Repo {
    fn new(folder: PathBuf) -> Self {
        Repo { folder, ..Default::default() }
    }

    /// Where git runs for it.
    pub fn root(&self) -> PathBuf {
        self.top.clone().unwrap_or_else(|| self.folder.clone())
    }

    pub fn has_changes(&self) -> bool {
        !(self.conflicts.is_empty() && self.staged.is_empty() && self.changes.is_empty())
    }

    pub fn changed_count(&self) -> usize {
        self.conflicts.len() + self.staged.len() + self.changes.len()
    }

    /// git push's arguments: a branch with no upstream is published to origin.
    pub fn push_args(&self) -> Vec<String> {
        if self.upstream.is_none() && !self.branch.is_empty() {
            vec!["push".into(), "-u".into(), "origin".into(), self.branch.clone()]
        } else {
            vec!["push".into()]
        }
    }

    /// Its name in the view (the folder's).
    pub fn name(&self) -> String {
        self.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.folder.display().to_string())
    }

    /// `git status --porcelain=v1 -b`'s answer.
    fn read_status(&mut self, out: &str) {
        self.conflicts.clear();
        self.staged.clear();
        self.changes.clear();
        self.error = None;
        for line in out.lines() {
            if let Some(b) = line.strip_prefix("## ") {
                // "main...origin/main [ahead 1, behind 2]"
                let (head, rest) = b.split_once(' ').unwrap_or((b, ""));
                let (branch, up) = head.split_once("...").map_or((head, None), |(a, u)| (a, Some(u.to_string())));
                self.branch = branch.to_string();
                self.upstream = up;
                let num = |key: &str| rest.split(key).nth(1).and_then(|t| t.trim_start().split(|c: char| !c.is_ascii_digit()).next()).and_then(|n| n.parse().ok()).unwrap_or(0);
                self.ahead = num("ahead");
                self.behind = num("behind");
                continue;
            }
            if line.len() < 4 {
                continue;
            }
            let (x, y) = (line.as_bytes()[0] as char, line.as_bytes()[1] as char);
            let path = line[3..].rsplit(" -> ").next().unwrap_or(&line[3..]).trim_matches('"').to_string();
            // (Both sides changed it: a merge conflict, its own list.)
            if matches!((x, y), ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D')) {
                self.conflicts.push(('C', path));
                continue;
            }
            if x != ' ' && x != '?' {
                self.staged.push((x, path.clone()));
            }
            if y != ' ' {
                self.changes.push((if x == '?' { 'U' } else { y }, path));
            }
        }
    }
}

/// Git: every workspace folder's repository stacked (one alone shows
/// without its heading), the changed files (staged and not), staging, and a
/// commit message. The toolbar and the message are the selected row's
/// repository's, as in VS Code's.
pub struct Scm {
    pub message: LineInput,
    pub list: ListView,
    pub repos: Vec<Repo>,
    /// Each row's (staged, path from its repository's top); None for headings.
    pub entries: Vec<Option<(bool, PathBuf)>>,
    /// Each row's git letter (U untracked, M, A, D, R, C conflict; ' ' for headings).
    pub codes: Vec<char>,
    /// Each row's repository (an index into `repos`).
    pub row_repo: Vec<usize>,
    pub in_field: bool,
    pub status: String,
    /// git's answer to the last command run, for the status bar (taken once).
    pub done: Option<String>,
    tx: Sender<ScmMsg>,
    rx: Receiver<ScmMsg>,
}

enum ScmMsg {
    /// (folder, its top, git status)
    Status(PathBuf, Option<PathBuf>, Result<String, String>),
    Done(Result<String, String>),
}

/// A folder's repository top and its status (with the branch line).
fn scm_status(folder: &Path) -> ScmMsg {
    let top = git(folder, &["rev-parse", "--show-toplevel"]).ok().map(|t| PathBuf::from(t.trim()));
    let st = git(folder, &["status", "--porcelain=v1", "-b", "-uall"]);
    ScmMsg::Status(folder.to_path_buf(), top, st)
}

pub fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git").arg("-C").arg(root).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

impl Default for Scm {
    fn default() -> Self {
        let (tx, rx) = channel();
        Scm {
            message: LineInput::default(),
            list: ListView::new(),
            repos: Vec::new(),
            entries: Vec::new(),
            codes: Vec::new(),
            row_repo: Vec::new(),
            in_field: false,
            status: String::new(),
            done: None,
            tx,
            rx,
        }
    }
}

impl Scm {
    /// The workspace's folders (the project's first): their repositories,
    /// keeping what's known of those already shown.
    pub fn set_folders(&mut self, folders: &[PathBuf]) {
        if self.repos.len() == folders.len() && self.repos.iter().zip(folders).all(|(r, f)| &r.folder == f) {
            return;
        }
        let active = self.active().map(|r| r.folder.clone());
        let mut old: Vec<Repo> = std::mem::take(&mut self.repos);
        self.repos = folders.iter().map(|f| old.iter().position(|r| &r.folder == f).map(|i| old.remove(i)).unwrap_or_else(|| Repo::new(f.clone()))).collect();
        self.rebuild();
        if let Some(i) = active.and_then(|a| self.repos.iter().position(|r| r.folder == a)) {
            self.select_repo(i);
        }
    }

    /// The selected row's repository's index.
    pub fn active_index(&self) -> usize {
        self.row_repo.get(self.list.selected).copied().unwrap_or(0).min(self.repos.len().saturating_sub(1))
    }

    /// The selected row's repository (the toolbar's and the message's).
    pub fn active(&self) -> Option<&Repo> {
        self.repos.get(self.active_index())
    }

    /// Where git runs for the selected row (the project's folder before any is read).
    pub fn root(&self) -> Option<PathBuf> {
        self.active().map(Repo::root)
    }

    /// The branch of the repository a file is in (else the selected one's).
    pub fn branch_of(&self, file: Option<&Path>) -> String {
        let r = file.and_then(|f| self.repos.iter().filter(|r| f.starts_with(r.root()) || f.starts_with(&r.folder)).max_by_key(|r| r.root().as_os_str().len()));
        r.or(self.active()).map(|r| r.branch.clone()).unwrap_or_default()
    }

    /// Select a repository's first row (its heading when stacked).
    pub fn select_repo(&mut self, k: usize) {
        if let Some(i) = self.row_repo.iter().position(|&r| r == k) {
            self.list.select(i);
        }
    }

    /// Look again at every repository.
    pub fn refresh_all(&mut self) {
        for k in 0..self.repos.len() {
            self.refresh(k);
        }
    }

    fn refresh(&mut self, k: usize) {
        let Some(r) = self.repos.get_mut(k) else { return };
        if r.busy {
            return;
        }
        r.busy = true;
        r.refreshed = Some(Instant::now());
        let tx = self.tx.clone();
        let folder = r.folder.clone();
        std::thread::spawn(move || {
            let _ = tx.send(scm_status(&folder));
        });
    }

    /// Run a git command in a repository, then look again (commit, push, pull and the rest).
    pub fn run(&mut self, root: &Path, args: Vec<String>) {
        self.run_all(root, vec![args]);
    }

    /// Run git commands one after another (stopping at a failure), then look again.
    pub fn run_all(&mut self, root: &Path, cmds: Vec<Vec<String>>) {
        self.run_with(root, move |r| {
            let mut last = Ok(String::new());
            for args in &cmds {
                let a: Vec<&str> = args.iter().map(String::as_str).collect();
                last = git(r, &a);
                if last.is_err() {
                    break;
                }
            }
            last
        });
    }

    /// Do some work in a repository off the UI thread (its answer the
    /// status line), then look at that repository again.
    pub fn run_with(&mut self, root: &Path, f: impl FnOnce(&Path) -> Result<String, String> + Send + 'static) {
        let tx = self.tx.clone();
        let root = root.to_path_buf();
        let k = self.repos.iter().position(|r| r.root() == root || r.folder == root);
        let folder = k.map_or_else(|| root.clone(), |k| self.repos[k].folder.clone());
        if let Some(k) = k {
            self.repos[k].busy = true;
        }
        self.status = "Working…".into();
        std::thread::spawn(move || {
            let r = f(&root);
            let _ = tx.send(ScmMsg::Done(r));
            let _ = tx.send(scm_status(&folder));
        });
    }

    /// Stage or unstage the file on row `i`.
    pub fn toggle_stage(&mut self, i: usize) {
        let Some((staged, p)) = self.entry(i) else { return };
        let Some(root) = self.row_root(i) else { return };
        let path = p.display().to_string();
        let args = if staged { vec!["restore".into(), "--staged".into(), "--".into(), path] } else { vec!["add".into(), "--".into(), path] };
        self.run(&root, args);
    }

    /// Where git runs for row `i`'s repository.
    pub fn row_root(&self, i: usize) -> Option<PathBuf> {
        self.row_repo.get(i).and_then(|&k| self.repos.get(k)).map(Repo::root)
    }

    /// Commit in the selected repository (see commit_with).
    pub fn commit(&mut self) -> Result<(), &'static str> {
        let Some(root) = self.root() else { return Err("No repository") };
        self.commit_with(&root, &[])
    }

    /// Commit (nothing staged: every change, as VS Code's smart commit),
    /// with more of git's arguments (--amend), the message from the box.
    pub fn commit_with(&mut self, root: &Path, extra: &[&str]) -> Result<(), &'static str> {
        let args = self.commit_args(extra)?;
        self.run(root, args);
        Ok(())
    }

    /// git's arguments for a commit in the selected repository (see
    /// commit_with); the box is emptied.
    pub fn commit_args(&mut self, extra: &[&str]) -> Result<Vec<String>, &'static str> {
        let msg = self.message.text.trim().to_string();
        let amend = extra.contains(&"--amend");
        if msg.is_empty() && !amend {
            return Err("Type a commit message first");
        }
        let (any, any_staged) = self.active().map_or((false, false), |r| (r.has_changes(), !r.staged.is_empty()));
        if !any && !amend {
            return Err("Nothing to commit");
        }
        self.message.set("");
        let mut args: Vec<String> = vec!["commit".into()];
        if !any_staged && !amend {
            args.push("--all".into());
        }
        args.extend(extra.iter().map(|s| s.to_string()));
        if msg.is_empty() {
            args.push("--no-edit".into());
        } else {
            args.push("-m".into());
            args.push(msg);
        }
        Ok(args)
    }

    /// The selected repository's push arguments.
    pub fn push_args(&self) -> Vec<String> {
        self.active().map(Repo::push_args).unwrap_or_else(|| vec!["push".into()])
    }

    /// Whether the selected repository has changes.
    pub fn has_changes(&self) -> bool {
        self.active().is_some_and(Repo::has_changes)
    }

    /// The (staged, path) on a row.
    pub fn entry(&self, i: usize) -> Option<(bool, PathBuf)> {
        self.entries.get(i).cloned().flatten()
    }

    pub fn poll(&mut self, shown: bool) -> bool {
        let mut changed = false;
        while let Ok(m) = self.rx.try_recv() {
            match m {
                ScmMsg::Done(r) => {
                    self.status = match r {
                        Ok(out) => out.lines().next().unwrap_or("Done").to_string(),
                        Err(e) => e.lines().last().unwrap_or("git failed").to_string(),
                    };
                    self.done = Some(self.status.clone());
                }
                ScmMsg::Status(folder, top, r) => {
                    let Some(repo) = self.repos.iter_mut().find(|r| r.folder == folder) else { continue };
                    repo.busy = false;
                    if top.is_some() {
                        repo.top = top;
                    }
                    match r {
                        Ok(out) => repo.read_status(&out),
                        Err(e) => {
                            *repo = Repo { folder: repo.folder.clone(), refreshed: repo.refreshed, ..Default::default() };
                            repo.error = Some(if e.contains("not a git repository") { "Not a git repository".to_string() } else { e });
                        }
                    }
                    self.rebuild();
                }
            }
            changed = true;
        }
        // (Shown: every few seconds; else now and then, for the status bar's branch.)
        let every = Duration::from_secs(if shown { 4 } else { 20 });
        for k in 0..self.repos.len() {
            let r = &self.repos[k];
            if !r.busy && r.refreshed.is_none_or(|t| t.elapsed() > every) {
                self.refresh(k);
            }
        }
        changed
    }

    /// The rows from every repository's lists (the selection kept on its file).
    fn rebuild(&mut self) {
        let keep = (self.row_repo.get(self.list.selected).copied(), self.entry(self.list.selected));
        let colour = |c: char| match c {
            'M' => vga::YELLOW,
            'A' | 'U' => vga::LIGHT_GREEN,
            'D' | 'C' => vga::LIGHT_RED,
            _ => vga::LIGHT_CYAN,
        };
        let stacked = self.repos.len() > 1;
        let mut rows = Vec::new();
        let (mut entries, mut codes, mut row_repo) = (Vec::new(), Vec::new(), Vec::new());
        let mut push = |rows: &mut Vec<Row>, row: Row, e: Option<(bool, PathBuf)>, c: char, k: usize| {
            rows.push(row);
            entries.push(e);
            codes.push(c);
            row_repo.push(k);
        };
        for (k, repo) in self.repos.iter().enumerate() {
            let ind = u16::from(stacked);
            if stacked {
                let mut detail = if repo.branch.is_empty() { String::new() } else { format!("⎇ {}", repo.branch) };
                if repo.ahead > 0 || repo.behind > 0 {
                    detail.push_str(&format!(" {}↓ {}↑", repo.behind, repo.ahead));
                }
                push(&mut rows, Row::new(repo.name()).heading().bold().icon("▣").detail(detail), None, ' ', k);
            }
            if let Some(e) = &repo.error {
                push(&mut rows, Row::new(e.clone()).indent(ind), None, ' ', k);
                continue;
            }
            let mut any = false;
            for (title, list, is_staged) in [("Merge Changes", &repo.conflicts, false), ("Staged Changes", &repo.staged, true), ("Changes", &repo.changes, false)] {
                if list.is_empty() {
                    continue;
                }
                any = true;
                let mut h = Row::new(format!("{title} ({})", list.len())).indent(ind);
                if !stacked {
                    h = h.heading();
                }
                push(&mut rows, h, None, ' ', k);
                for (c, p) in list {
                    let name = Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
                    let dir = Path::new(p).parent().map(|d| d.display().to_string()).unwrap_or_default();
                    push(&mut rows, Row::new(name).indent(ind + 1).icon(c.to_string()).detail(dir).fg(colour(*c)), Some((is_staged, PathBuf::from(p))), *c, k);
                }
            }
            if !any {
                push(&mut rows, Row::new(if repo.refreshed.is_some() && !repo.busy || !repo.branch.is_empty() { "No changes" } else { "…" }).indent(ind), None, ' ', k);
            }
        }
        if rows.is_empty() {
            push(&mut rows, Row::new("No folder open"), None, ' ', 0);
        }
        self.entries = entries;
        self.codes = codes;
        self.row_repo = row_repo;
        let sel = self.list.selected;
        self.list.set_rows(rows);
        // (The same file, else the same repository, else where it was.)
        let found = match keep {
            (Some(k), Some(e)) => (0..self.entries.len()).find(|&i| self.row_repo[i] == k && self.entries[i].as_ref() == Some(&e)),
            _ => None,
        }
        .or_else(|| keep.0.filter(|&k| self.row_repo.get(sel) != Some(&k)).and_then(|k| self.row_repo.iter().position(|&r| r == k)));
        self.list.select(found.unwrap_or(sel.min(self.entries.len().saturating_sub(1))));
    }
}

// ---- Extensions -----------------------------------------------------------------------

/// What each language has: its language server and debugger (found on
/// PATH), and its highlighting (built in, or a grammar fetched on demand).
/// Turbine's WASI extensions will list here too.
pub struct Extensions {
    pub list: ListView,
    /// Each row's language (None for headings and extensions).
    pub langs: Vec<Option<String>>,
    rx: Option<Receiver<(Vec<Row>, Vec<Option<String>>)>>,
    loaded: bool,
    /// Installed extensions: (name, description, why it failed, loaded).
    installed: Vec<(String, String, Option<String>, bool)>,
    /// The languages' rows, as found.
    lang_rows: (Vec<Row>, Vec<Option<String>>),
}

impl Default for Extensions {
    fn default() -> Self {
        Extensions { list: ListView::new(), langs: Vec::new(), rx: None, loaded: false, installed: Vec::new(), lang_rows: (Vec::new(), Vec::new()) }
    }
}

const LANGS: &[&str] = &[
    "bash", "c", "cpp", "c-sharp", "css", "dart", "elixir", "go", "haskell", "html", "java", "javascript", "json", "kotlin",
    "lua", "markdown", "nix", "ocaml", "php", "python", "qml", "ruby", "rust", "swift", "toml", "tsx", "typescript", "yaml",
    "zig",
];

impl Extensions {
    /// Look (again) in the background.
    pub fn load(&mut self) {
        self.loaded = true;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let mut found = Vec::new();
            let mut missing = Vec::new();
            for &lang in LANGS {
                let label = turbine_syntax::BUILTIN.iter().find(|(n, _)| *n == lang).map(|(_, l)| l.to_string()).unwrap_or_else(|| {
                    let mut c = lang.chars();
                    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
                });
                let server = turbine_services::lsp::server_for(lang).map(|a| a[0].clone());
                let debugger = turbine_services::dap::adapter_for(lang).map(|a| a[0].clone());
                let grammar = if turbine_syntax::BUILTIN.iter().any(|(n, _)| *n == lang) { "built-in" } else { "on demand" };
                let detail = match (&server, &debugger) {
                    (Some(s), Some(d)) => format!("{s} · {d}"),
                    (Some(s), None) => s.clone(),
                    (None, _) => grammar.to_string(),
                };
                let row = Row::new(label).detail(detail);
                if server.is_some() {
                    found.push((row.icon("✓").fg(vga::LIGHT_GREEN), lang.to_string()));
                } else {
                    missing.push((row.icon("·"), lang.to_string()));
                }
            }
            let mut rows = vec![Row::new(format!("Language servers found ({})", found.len())).heading()];
            let mut langs = vec![None];
            for (r, l) in found {
                rows.push(r.indent(1));
                langs.push(Some(l));
            }
            rows.push(Row::new(format!("Not installed ({})", missing.len())).heading());
            langs.push(None);
            for (r, l) in missing {
                rows.push(r.indent(1));
                langs.push(Some(l));
            }
            let _ = tx.send((rows, langs));
        });
    }

    pub fn poll(&mut self, shown: bool) -> bool {
        if shown && !self.loaded {
            self.load();
        }
        let Some(rx) = &self.rx else { return false };
        let Ok(found) = rx.try_recv() else { return false };
        self.rx = None;
        self.lang_rows = found;
        self.rebuild();
        true
    }

    /// The installed extensions (from the host), when they change.
    pub fn set_installed(&mut self, list: Vec<(String, String, Option<String>, bool)>) {
        if list != self.installed {
            self.installed = list;
            self.rebuild();
        }
    }

    /// Installed extensions first, then the languages.
    fn rebuild(&mut self) {
        let mut rows = vec![Row::new(format!("Installed ({})", self.installed.len())).heading()];
        let mut langs = vec![None];
        if self.installed.is_empty() {
            rows.push(Row::new("None yet: see extensions/build.sh").indent(1));
            langs.push(None);
        }
        for (name, desc, err, loaded) in &self.installed {
            let row = Row::new(name.clone()).indent(1);
            rows.push(match (loaded, err) {
                (_, Some(e)) => row.icon("×").fg(vga::LIGHT_RED).detail(e.lines().next().unwrap_or("").to_string()),
                (true, None) => row.icon("✓").fg(vga::LIGHT_GREEN).detail(desc.clone()),
                (false, None) => row.icon("·").detail("loading…"),
            });
            langs.push(None);
        }
        rows.extend(self.lang_rows.0.iter().cloned());
        langs.extend(self.lang_rows.1.iter().cloned());
        self.list.set_rows(rows);
        self.langs = langs;
    }

    /// What a row says about its language, for the status line.
    pub fn describe(&self, i: usize) -> Option<String> {
        let lang = self.langs.get(i)?.as_ref()?;
        let row = self.list.rows.get(i)?;
        Some(if row.icon == "✓" {
            format!("{}: completions, diagnostics, go to definition via {}", row.text, row.detail)
        } else {
            let hint = turbine_services::lsp::server_names(lang).join(" or ");
            format!("{}: install {hint} (on PATH) for completions and diagnostics", row.text)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn explorer_lists_opens_folders_and_reveals_files() {
        let dir = std::env::temp_dir().join(format!("turbine-ex-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::write(dir.join("src/deep/x.rs"), "").unwrap();
        std::fs::write(dir.join("README.md"), "").unwrap();
        let mut e = Explorer::new(dir.clone());
        wait(|| e.poll(true));
        let names = |e: &Explorer| e.list.rows.iter().map(|r| r.text.clone()).collect::<Vec<_>>();
        assert_eq!(names(&e)[0], "..");
        assert_eq!(names(&e)[2..], ["src".to_string(), "README.md".to_string()]);
        assert_eq!(e.activate(2), None);
        wait(|| e.poll(true));
        assert_eq!(names(&e)[2..], ["src".to_string(), "deep".into(), "README.md".into()]);
        assert_eq!(e.activate(4), Some(dir.join("README.md")));
        e.reveal(&dir.join("src/deep/x.rs"));
        wait(|| {
            e.poll(true);
            e.list.rows.get(e.list.selected).is_some_and(|r| r.text == "x.rs")
        });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_finds_lines_grouped_by_file() {
        let dir = std::env::temp_dir().join(format!("turbine-se-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("a.txt"), "one\nNeedle here\nnone\n").unwrap();
        std::fs::write(dir.join("target/b.txt"), "needle in build output\n").unwrap();
        let mut s = Search::default();
        s.query.set("needle");
        s.run(&dir);
        wait(|| s.poll(&dir));
        assert_eq!(s.hits, vec![None, Some((dir.join("a.txt"), 1))]);
        assert_eq!(s.list.rows[0].text, "a.txt");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_results_are_coloured_and_marked() {
        let hits = colour_hits(vec![(PathBuf::from("a.rs"), 0, "let needle = Needle::new();".into())], Some("needle"));
        let (_, _, _, syntax, marks) = &hits[0];
        assert_eq!(marks, &vec![(4, 10), (13, 19)]);
        // (`let` is a keyword: coloured as the editor colours it.)
        assert!(syntax.iter().any(|(a, b, _)| *a == 0 && *b == 3), "{syntax:?}");
        // A match far along: the start cut to keep it in view.
        let hits = colour_hits(vec![(PathBuf::from("a.rs"), 0, "let a_very_long_name_here = 1 + needle;".into())], Some("needle"));
        let (_, _, text, _, marks) = &hits[0];
        assert_eq!(text, "…ere = 1 + needle;");
        let (a, b) = marks[0];
        assert_eq!(&text[a as usize..b as usize], "needle");
    }

    #[test]
    fn scm_reads_git_status() {
        let mut s = Scm::default();
        s.set_folders(&[PathBuf::from("/p")]);
        let feed = |s: &mut Scm, k: usize, out: &str| {
            s.repos[k].read_status(out);
            s.rebuild();
        };
        feed(&mut s, 0, "## main...origin/main\nM  staged.rs\n M src/changed.rs\n?? new.txt\n");
        assert_eq!(s.repos[0].branch, "main");
        let rows: Vec<_> = s.list.rows.iter().map(|r| (r.icon.clone(), r.text.clone())).collect();
        assert_eq!(
            rows,
            vec![
                (String::new(), "Staged Changes (1)".to_string()),
                ("M".into(), "staged.rs".into()),
                (String::new(), "Changes (2)".into()),
                ("M".into(), "changed.rs".into()),
                ("U".into(), "new.txt".into()),
            ]
        );
        assert_eq!(s.entries[1], Some((true, PathBuf::from("staged.rs"))));
        let r = &s.repos[0];
        assert_eq!((r.ahead, r.behind, r.upstream.as_deref()), (0, 0, Some("origin/main")));
        // Ahead and behind, a conflict in its own list, no upstream.
        feed(&mut s, 0, "## dev...origin/dev [ahead 2, behind 13]\nUU both.rs\n");
        let r = &s.repos[0];
        assert_eq!((r.branch.as_str(), r.ahead, r.behind), ("dev", 2, 13));
        assert_eq!(s.list.rows[0].text, "Merge Changes (1)");
        assert_eq!(s.codes[1], 'C');
        feed(&mut s, 0, "## topic\n");
        assert_eq!((s.repos[0].upstream.as_deref(), s.push_args()), (None, vec!["push".to_string(), "-u".into(), "origin".into(), "topic".into()]));
        // Two folders: stacked under their names; the toolbar's is the selected row's.
        s.set_folders(&[PathBuf::from("/p"), PathBuf::from("/q")]);
        feed(&mut s, 1, "## feature\n M a.txt\n");
        let rows: Vec<_> = s.list.rows.iter().map(|r| r.text.clone()).collect();
        assert_eq!(rows, vec!["p", "No changes", "q", "Changes (1)", "a.txt"]);
        assert_eq!(s.row_repo, vec![0, 0, 1, 1, 1]);
        assert_eq!(s.active().unwrap().branch, "topic");
        s.select_repo(1);
        assert_eq!(s.active().unwrap().branch, "feature");
        assert_eq!(s.branch_of(Some(Path::new("/q/a.txt"))), "feature");
        assert_eq!(s.branch_of(Some(Path::new("/p/x"))), "topic");
    }
}
