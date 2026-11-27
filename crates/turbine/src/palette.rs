//! The command palette and quick open (Cursor's Ctrl+Shift+P and Ctrl+P):
//! one field over a list, filtered fuzzily as you type. What's typed first
//! picks the mode, as in VS Code: `>` commands, `:` a line, `@` a symbol in
//! the file, `#` a symbol in the project, anything else a file in the project.

use crate::views::LineInput;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use turbine_ui::list::{ListView, Row};

/// What choosing an item does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Command(String),
    Open(PathBuf),
    Line(usize),
    /// A place in a file: (path, line, UTF-16 column).
    Place(PathBuf, usize, usize),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub detail: String,
    pub pick: Pick,
}

pub struct Palette {
    pub input: LineInput,
    pub list: ListView,
    commands: Vec<Item>,
    symbols: Vec<Item>,
    /// The project's symbols matching what's typed after `#` (from the
    /// language server), and what to say when there are none.
    workspace: Vec<Item>,
    workspace_empty: &'static str,
    files: Vec<Item>,
    files_rx: Option<Receiver<Vec<PathBuf>>>,
    root: PathBuf,
    /// The items behind the list's rows.
    shown: Vec<Item>,
}

/// How well `query` matches `text` as a subsequence: None when it doesn't;
/// higher for runs of letters and letters at the start of words.
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().collect();
    let mut score = 0;
    let mut ti = 0;
    let mut prev: Option<usize> = None;
    for qc in query.chars().flat_map(char::to_lowercase) {
        let mut found = None;
        while ti < t.len() {
            if t[ti].to_lowercase().eq(std::iter::once(qc)) {
                found = Some(ti);
                break;
            }
            ti += 1;
        }
        let i = found?;
        let word_start = i == 0 || matches!(t[i - 1], ' ' | '/' | '_' | '-' | '.' | ':');
        score += 1 + if prev == Some(i.wrapping_sub(1)) { 5 } else { 0 } + if word_start { 3 } else { 0 };
        prev = Some(i);
        ti = i + 1;
    }
    // (Shorter texts first among equals.)
    Some(score * 100 - t.len() as i32)
}

impl Palette {
    /// `start`: what's in the field to begin with (">" for commands).
    pub fn new(start: &str, commands: Vec<Item>, symbols: Vec<Item>, root: &Path) -> Palette {
        let mut p = Palette {
            input: LineInput::default(),
            list: ListView::new(),
            commands,
            symbols,
            workspace: Vec::new(),
            workspace_empty: "Searching…",
            files: Vec::new(),
            files_rx: None,
            root: root.to_path_buf(),
            shown: Vec::new(),
        };
        p.input.set(start);
        // The project's files, listed in the background.
        let (tx, rx) = channel();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut stack = vec![root];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let Ok(ft) = e.file_type() else { continue };
                    if ft.is_dir() {
                        if !matches!(name.as_str(), ".git" | "target" | "node_modules" | ".direnv" | "result" | "__pycache__" | ".venv") {
                            stack.push(e.path());
                        }
                    } else if out.len() < 50_000 {
                        out.push(e.path());
                    }
                }
            }
            let _ = tx.send(out);
        });
        p.files_rx = Some(rx);
        p.refilter();
        p
    }

    /// Take the file list when it's ready.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.files_rx else { return false };
        let Ok(files) = rx.try_recv() else { return false };
        self.files_rx = None;
        self.files = files
            .into_iter()
            .map(|p| {
                let rel = p.strip_prefix(&self.root).unwrap_or(&p).to_path_buf();
                let name = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let dir = rel.parent().map(|d| d.display().to_string()).unwrap_or_default();
                Item { label: name, detail: dir, pick: Pick::Open(p) }
            })
            .collect();
        self.refilter();
        true
    }

    pub fn title(&self) -> &'static str {
        match self.input.text.chars().next() {
            Some('>') => "Command Palette",
            Some(':') => "Go to Line",
            Some('@') => "Go to Symbol in Editor",
            Some('#') => "Go to Symbol in Workspace",
            _ => "Go to File",
        }
    }

    /// Rebuild the list for what's typed.
    pub fn refilter(&mut self) {
        let text = self.input.text.clone();
        let (pool, q): (&[Item], &str) = match text.chars().next() {
            Some('>') => (&self.commands, text[1..].trim()),
            Some('@') => (&self.symbols, text[1..].trim()),
            Some('#') => {
                // (The server already matched them.)
                self.shown = self.workspace.clone();
                let rows = if self.shown.is_empty() {
                    vec![Row::new(if text[1..].trim().is_empty() { "Type a symbol's name" } else { self.workspace_empty })]
                } else {
                    self.shown.iter().map(|i| Row::new(i.label.clone()).detail(i.detail.clone())).collect()
                };
                self.list.set_rows(rows);
                self.list.select(0);
                return;
            }
            Some(':') => {
                let n = text[1..].trim().parse::<usize>().ok();
                self.shown = n.map(|n| Item { label: format!("Go to line {n}"), detail: String::new(), pick: Pick::Line(n.saturating_sub(1)) }).into_iter().collect();
                if self.shown.is_empty() {
                    self.list.set_rows(vec![Row::new("Type a line number")]);
                } else {
                    self.list.set_rows(self.shown.iter().map(|i| Row::new(i.label.clone())).collect());
                }
                return;
            }
            _ => (&self.files, text.trim()),
        };
        let mut scored: Vec<(i32, &Item)> = pool
            .iter()
            .filter_map(|it| {
                // (Files match on their path too, the name counting most.)
                let s = fuzzy(q, &it.label).map(|s| s + 1000).or_else(|| fuzzy(q, &format!("{}/{}", it.detail, it.label)));
                s.map(|s| (s, it))
            })
            .collect();
        if !q.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0));
        }
        self.shown = scored.into_iter().take(500).map(|(_, it)| it.clone()).collect();
        let rows: Vec<Row> = if self.shown.is_empty() {
            vec![Row::new(if self.files_rx.is_some() && !text.starts_with(['>', '@']) { "Listing files…" } else { "No matching results" })]
        } else {
            self.shown.iter().map(|i| Row::new(i.label.clone()).detail(i.detail.clone())).collect()
        };
        self.list.set_rows(rows);
        self.list.select(0);
    }

    /// The current file's symbols (they arrive from the Outline).
    pub fn set_symbols(&mut self, syms: Vec<Item>) {
        self.symbols = syms;
        if self.input.text.starts_with('@') {
            self.refilter();
        }
    }

    /// The project's symbols for what's typed after `#`.
    pub fn set_workspace_symbols(&mut self, items: Vec<Item>, empty: &'static str) {
        self.workspace = items;
        self.workspace_empty = empty;
        if self.input.text.starts_with('#') {
            self.refilter();
        }
    }

    /// The item on a row.
    pub fn pick(&self, i: usize) -> Option<Pick> {
        self.shown.get(i).map(|it| it.pick.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_runs_and_word_starts() {
        assert!(fuzzy("wb", "workbench.rs").is_some());
        assert!(fuzzy("xyz", "workbench.rs").is_none());
        assert!(fuzzy("work", "workbench.rs") > fuzzy("work", "w_o_r_k.rs"));
        assert!(fuzzy("tog", "Toggle Panel") > fuzzy("tog", "Go to Bracket"));
    }

    #[test]
    fn modes_follow_the_first_character() {
        let cmds = vec![
            Item { label: "View: Toggle Panel".into(), detail: "Ctrl+J".into(), pick: Pick::Command("view.panel".into()) },
            Item { label: "File: Save".into(), detail: "F2".into(), pick: Pick::Command("file.save".into()) },
        ];
        let syms = vec![Item { label: "main".into(), detail: "3".into(), pick: Pick::Line(2) }];
        let dir = std::env::temp_dir().join(format!("turbine-pal-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "").unwrap();
        let mut p = Palette::new(">", cmds, syms, &dir);
        p.input.set(">panel");
        p.refilter();
        assert_eq!(p.pick(0), Some(Pick::Command("view.panel".into())));
        p.input.set(":12");
        p.refilter();
        assert_eq!(p.pick(0), Some(Pick::Line(11)));
        p.input.set("@ma");
        p.refilter();
        assert_eq!(p.pick(0), Some(Pick::Line(2)));
        let t = std::time::Instant::now();
        while !p.poll() {
            assert!(t.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        p.input.set("lib");
        p.refilter();
        assert_eq!(p.pick(0), Some(Pick::Open(dir.join("src/lib.rs"))));
        assert_eq!(p.title(), "Go to File");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
