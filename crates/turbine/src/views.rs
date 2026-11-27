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
    pub expanded: HashSet<PathBuf>,
    children: HashMap<PathBuf, Vec<(String, bool)>>,
    pending: HashSet<PathBuf>,
    tx: Sender<(PathBuf, Vec<(String, bool)>)>,
    rx: Receiver<(PathBuf, Vec<(String, bool)>)>,
    pub list: ListView,
    /// The path of each row (row 0: the root).
    paths: Vec<(PathBuf, bool)>,
    refreshed: Instant,
    /// Select this path once its folder is listed.
    want: Option<PathBuf>,
}

impl Explorer {
    pub fn new(root: PathBuf) -> Explorer {
        let (tx, rx) = channel();
        let mut e = Explorer {
            root: root.clone(),
            expanded: HashSet::from([root.clone()]),
            children: HashMap::new(),
            pending: HashSet::new(),
            tx,
            rx,
            list: ListView::new(),
            paths: Vec::new(),
            refreshed: Instant::now(),
            want: None,
        };
        e.request(&root);
        e.rebuild();
        e
    }

    pub fn set_root(&mut self, root: PathBuf) {
        if root != self.root {
            *self = Explorer::new(root);
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
            let _ = tx.send((dir, v));
        });
    }

    /// Take finished listings; re-list open folders every few seconds while
    /// shown (files come and go). True when the tree changed.
    pub fn poll(&mut self, shown: bool) -> bool {
        let mut changed = false;
        while let Ok((dir, v)) = self.rx.try_recv() {
            self.pending.remove(&dir);
            if self.children.get(&dir) != Some(&v) {
                self.children.insert(dir, v);
                changed = true;
            }
        }
        if shown && self.refreshed.elapsed() > Duration::from_secs(3) {
            self.refreshed = Instant::now();
            for d in self.expanded.clone() {
                self.request(&d);
            }
        }
        if changed {
            self.rebuild();
        }
        changed
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let mut paths = Vec::new();
        let name = self.root.file_name().map(|n| n.to_string_lossy().to_uppercase()).unwrap_or_else(|| self.root.display().to_string());
        rows.push(Row::new(name).heading());
        paths.push((self.root.clone(), true));
        self.walk(&self.root.clone(), 0, &mut rows, &mut paths);
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
            let p = dir.join(name);
            let open = self.expanded.contains(&p);
            let icon = if *is_dir { if open { "▾" } else { "▸" } } else { " " };
            rows.push(Row::new(name.clone()).indent(depth).icon(icon));
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

    /// Read every open folder again (after files changed).
    pub fn refresh_all(&mut self) {
        self.refreshed = Instant::now();
        for d in self.expanded.clone() {
            self.request(&d);
        }
    }

    /// Close every folder.
    pub fn collapse_all(&mut self) {
        self.expanded = HashSet::from([self.root.clone()]);
        self.rebuild();
        self.list.select(0);
    }

    /// Open or close a folder.
    pub fn toggle(&mut self, dir: &Path) {
        if dir == self.root {
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
    pub fn activate(&mut self, i: usize) -> Option<PathBuf> {
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
        if dir && self.expanded.contains(&p) && p != self.root {
            self.toggle(&p);
        } else if let Some(parent) = p.parent() {
            if let Some(j) = self.paths.iter().position(|(q, _)| q == parent) {
                self.list.select(j);
            }
        }
    }

    pub fn expand(&mut self, i: usize) {
        if let Some((p, true)) = self.paths.get(i).cloned() {
            if !self.expanded.contains(&p) {
                self.toggle(&p);
            }
        }
    }

    /// Show a file: open the folders down to it and select it.
    pub fn reveal(&mut self, file: &Path) {
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
    tx: Sender<(u32, u64, Option<Vec<Symbol>>)>,
    rx: Receiver<(u32, u64, Option<Vec<Symbol>>)>,
}

impl Default for Outline {
    fn default() -> Self {
        let (tx, rx) = channel();
        Outline { list: ListView::new(), lines: Vec::new(), shown: None, asked: None, tx, rx }
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

impl Outline {
    /// Ask for the outline of this editor's text (when it changed).
    pub fn update(&mut self, win: u32, version: u64, lang: Option<&str>, text: impl FnOnce() -> String) {
        let key = Some((win, version));
        if self.asked == key {
            return;
        }
        self.asked = key;
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
                syms.iter().map(|s| Row::new(s.name.clone()).indent(s.depth as u16).icon(symbol_icon(s.kind)).detail(format!("{}", s.line + 1))).collect()
            };
            self.list.set_rows(rows);
            changed = true;
        }
        changed
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
    rx: Option<Receiver<Vec<(PathBuf, usize, String)>>>,
    started: Option<Instant>,
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
                                let _ = tx.send(hits);
                                return;
                            }
                        }
                    }
                }
            }
            let _ = tx.send(hits);
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
        for (p, line, text) in &found {
            if last != Some(p) {
                let rel = p.strip_prefix(root).unwrap_or(p).display().to_string();
                rows.push(Row::new(rel).heading());
                hits.push(None);
                last = Some(p);
            }
            rows.push(Row::new(text.clone()).indent(1).detail(format!("{}", line + 1)));
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
            let _ = tx.send(hits);
        });
    }
}

// ---- Source Control -------------------------------------------------------------------

/// Git: the changed files (staged and not), staging, and a commit message.
pub struct Scm {
    pub message: LineInput,
    pub list: ListView,
    /// Each row's (staged, path); None for headings.
    pub entries: Vec<Option<(bool, PathBuf)>>,
    pub in_field: bool,
    pub status: String,
    pub branch: String,
    tx: Sender<ScmMsg>,
    rx: Receiver<ScmMsg>,
    busy: bool,
    refreshed: Option<Instant>,
}

enum ScmMsg {
    Status(Result<String, String>),
    Done(Result<String, String>),
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
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
            entries: Vec::new(),
            in_field: false,
            status: String::new(),
            branch: String::new(),
            tx,
            rx,
            busy: false,
            refreshed: None,
        }
    }
}

impl Scm {
    pub fn refresh(&mut self, root: &Path) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.refreshed = Some(Instant::now());
        let tx = self.tx.clone();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(ScmMsg::Status(git(&root, &["status", "--porcelain=v1", "-b", "-uall"])));
        });
    }

    /// Run a git command, then look again.
    fn run(&mut self, root: &Path, args: Vec<String>) {
        let tx = self.tx.clone();
        let root = root.to_path_buf();
        self.busy = true;
        std::thread::spawn(move || {
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            let r = git(&root, &a);
            let _ = tx.send(ScmMsg::Done(r));
            let _ = tx.send(ScmMsg::Status(git(&root, &["status", "--porcelain=v1", "-b", "-uall"])));
        });
    }

    /// Stage or unstage the file on row `i`.
    pub fn toggle_stage(&mut self, root: &Path, i: usize) {
        let Some(Some((staged, p))) = self.entries.get(i).cloned() else { return };
        let path = p.display().to_string();
        let args = if staged { vec!["restore".into(), "--staged".into(), "--".into(), path] } else { vec!["add".into(), "--".into(), path] };
        self.run(root, args);
    }

    pub fn commit(&mut self, root: &Path) -> Result<(), &'static str> {
        let msg = self.message.text.trim().to_string();
        if msg.is_empty() {
            return Err("Type a commit message first");
        }
        if !self.entries.iter().flatten().any(|(s, _)| *s) {
            return Err("Nothing staged: Space stages the selected file");
        }
        self.message.set("");
        self.run(root, vec!["commit".into(), "-m".into(), msg]);
        Ok(())
    }

    pub fn poll(&mut self, root: &Path, shown: bool) -> bool {
        let mut changed = false;
        while let Ok(m) = self.rx.try_recv() {
            match m {
                ScmMsg::Done(r) => {
                    self.status = match r {
                        Ok(out) => out.lines().next().unwrap_or("Done").to_string(),
                        Err(e) => e.lines().last().unwrap_or("git failed").to_string(),
                    };
                }
                ScmMsg::Status(r) => {
                    self.busy = false;
                    match r {
                        Ok(out) => self.read_status(&out),
                        Err(e) => {
                            self.entries.clear();
                            self.list.set_rows(vec![Row::new(if e.contains("not a git repository") { "Not a git repository".to_string() } else { e })]);
                        }
                    }
                }
            }
            changed = true;
        }
        // (Shown: every few seconds; else now and then, for the status bar's branch.)
        let every = Duration::from_secs(if shown { 4 } else { 20 });
        if !self.busy && self.refreshed.is_none_or(|t| t.elapsed() > every) {
            self.refresh(root);
        }
        changed
    }

    fn read_status(&mut self, out: &str) {
        let mut staged = Vec::new();
        let mut changes = Vec::new();
        for line in out.lines() {
            if let Some(b) = line.strip_prefix("## ") {
                self.branch = b.split("...").next().unwrap_or(b).to_string();
                continue;
            }
            if line.len() < 4 {
                continue;
            }
            let (x, y) = (line.as_bytes()[0] as char, line.as_bytes()[1] as char);
            let path = line[3..].rsplit(" -> ").next().unwrap_or(&line[3..]).trim_matches('"').to_string();
            if x != ' ' && x != '?' {
                staged.push((x, path.clone()));
            }
            if y != ' ' {
                changes.push((if x == '?' { 'U' } else { y }, path));
            }
        }
        let colour = |c: char| match c {
            'M' => vga::YELLOW,
            'A' | 'U' => vga::LIGHT_GREEN,
            'D' => vga::LIGHT_RED,
            _ => vga::LIGHT_CYAN,
        };
        let mut rows = Vec::new();
        let mut entries = Vec::new();
        for (title, list, is_staged) in [("Staged Changes", &staged, true), ("Changes", &changes, false)] {
            if list.is_empty() {
                continue;
            }
            rows.push(Row::new(format!("{title} ({})", list.len())).heading());
            entries.push(None);
            for (c, p) in list {
                let name = Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
                let dir = Path::new(p).parent().map(|d| d.display().to_string()).unwrap_or_default();
                rows.push(Row::new(name).indent(1).icon(c.to_string()).detail(dir).fg(colour(*c)));
                entries.push(Some((is_staged, PathBuf::from(p))));
            }
        }
        if rows.is_empty() {
            rows.push(Row::new("No changes"));
            entries.push(None);
        }
        self.entries = entries;
        self.list.set_rows(rows);
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
        assert_eq!(names(&e)[1..], ["src".to_string(), "README.md".to_string()]);
        assert_eq!(e.activate(1), None);
        wait(|| e.poll(true));
        assert_eq!(names(&e)[1..], ["src".to_string(), "deep".into(), "README.md".into()]);
        assert_eq!(e.activate(3), Some(dir.join("README.md")));
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
    fn scm_reads_git_status() {
        let mut s = Scm::default();
        s.read_status("## main...origin/main\nM  staged.rs\n M src/changed.rs\n?? new.txt\n");
        assert_eq!(s.branch, "main");
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
    }
}
