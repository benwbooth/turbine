//! Markdown shown rendered (crate::markdown), as Tessera's guide viewer
//! shows a guide: Help windows and guides always, and a live preview of a
//! Markdown file beside it (Ctrl+Shift+V). Rendering happens on a thread
//! of its own; a preview follows its window's width and its file's edits.
//! Enter (or a click) on a link follows it: a heading in the page, a file,
//! or a web page in the browser.

use super::*;
use crate::markdown::{self, Rendered};
use std::sync::mpsc::{Sender, channel};

/// What a page is: Markdown, a diff (side by side or not), or a commit list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PageKind {
    Markdown,
    /// Help and guides: Markdown with the Contents / Index / Back row.
    Help,
    Diff { side_by_side: bool },
    Log,
}

pub(super) enum Source {
    /// Waiting for its text (git running).
    Pending,
    /// Fixed Markdown (Help, a guide).
    Text(String),
    /// A Markdown file's window, followed as it changes.
    Window(u32),
}

pub(super) struct Preview {
    pub source: Source,
    /// Relative links are from here.
    pub base: Option<PathBuf>,
    /// What was asked to render last: (source version, width), and when.
    asked: Option<(u64, usize)>,
    changed_at: Option<(u64, Instant)>,
    generation: u64,
    pub rendered: Option<Rendered>,
    pub kind: PageKind,
    /// A diff's paths are from here (the repository).
    pub root: PathBuf,
    /// A heading to go to once it's rendered.
    pub jump: Option<String>,
    /// Lines left by following links on this page (Back, Alt+F1).
    pub back: Vec<usize>,
}

type Job = (u32, u64, String, usize, PageKind, PathBuf);

pub(super) struct PreviewState {
    pub previews: HashMap<u32, Preview>,
    tx: Sender<Job>,
    rx: Receiver<(u32, u64, Rendered)>,
    /// Text arriving for a Pending page (git's output): (window, text).
    pub text_tx: Sender<(u32, Result<String, String>)>,
    text_rx: Receiver<(u32, Result<String, String>)>,
}

impl Default for PreviewState {
    fn default() -> Self {
        let (tx, jobs) = channel::<Job>();
        let (done, rx) = channel();
        std::thread::Builder::new()
            .name("turbine-markdown".into())
            .spawn(move || {
                while let Ok(first) = jobs.recv() {
                    // Only the newest job per window.
                    let mut latest: HashMap<u32, Job> = HashMap::new();
                    latest.insert(first.0, first);
                    for j in jobs.try_iter() {
                        latest.insert(j.0, j);
                    }
                    for (_, (id, generation, md, width, kind, root)) in latest {
                        let r = match kind {
                            PageKind::Markdown => markdown::render(&md, width),
                            PageKind::Help => markdown::with_nav(markdown::render(&md, width), width),
                            PageKind::Diff { side_by_side } => crate::diffview::render(&md, width, side_by_side, &root),
                            PageKind::Log => log_page(&md),
                        };
                        if done.send((id, generation, r)).is_err() {
                            return;
                        }
                    }
                }
            })
            .expect("markdown thread");
        let (text_tx, text_rx) = channel();
        PreviewState { previews: HashMap::new(), tx, rx, text_tx, text_rx }
    }
}

impl Workbench {
    /// A read-only window titled `title` showing `md` rendered.
    pub(super) fn show_rendered(&mut self, title: &str, md: &str, base: Option<PathBuf>) {
        self.show_plain(title, "");
        let w = self.windows.last_mut().unwrap();
        w.ed.doc = Document::from_str("Rendering…\n");
        let id = w.win.id;
        self.preview.previews.insert(id, Preview { source: Source::Text(md.to_string()), base, asked: None, changed_at: None, generation: 0, rendered: None, jump: None, kind: PageKind::Help, root: PathBuf::new(), back: Vec::new() });
    }

    /// A page of `kind` whose text comes from running git (in the
    /// background) in `root`.
    pub(super) fn show_git_page(&mut self, title: &str, kind: PageKind, root: &Path, args: Vec<String>) {
        let r = root.to_path_buf();
        self.show_page_from(title, kind, root, move || {
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            crate::views::git(&r, &a)
        });
    }

    /// A page whose text `make` makes, off the UI thread.
    pub(super) fn show_page_from(&mut self, title: &str, kind: PageKind, root: &Path, make: impl FnOnce() -> Result<String, String> + Send + 'static) {
        self.show_plain(title, "Running git…\n");
        let w = self.windows.last_mut().unwrap();
        w.special = Some("preview");
        let id = w.win.id;
        self.preview.previews.insert(id, Preview { source: Source::Pending, base: Some(root.to_path_buf()), asked: None, changed_at: None, generation: 0, rendered: None, jump: None, kind, root: root.to_path_buf(), back: Vec::new() });
        let tx = self.preview.text_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((id, make()));
        });
    }

    /// The front page's kind, if it's a rendered page.
    pub(super) fn page_kind(&self) -> Option<PageKind> {
        self.windows.last().and_then(|w| self.preview.previews.get(&w.win.id)).map(|p| p.kind)
    }

    /// Keys on a diff or log page: s side by side or not, n / p the next
    /// and previous change, Enter the file at that line (or the commit).
    pub(super) fn page_key(&mut self, k: &KeyEvent) -> bool {
        let Some(kind) = self.page_kind() else { return false };
        let id = self.windows.last().unwrap().win.id;
        match (kind, k.key) {
            (PageKind::Diff { side_by_side }, Key::Char('s')) if k.mods == Mods::NONE => {
                let p = self.preview.previews.get_mut(&id).unwrap();
                p.kind = PageKind::Diff { side_by_side: !side_by_side };
                p.asked = None;
                self.say(if side_by_side { "One after another" } else { "Side by side" });
                true
            }
            (PageKind::Diff { .. }, Key::Char(c @ ('n' | 'p' | 'N'))) if !k.mods.ctrl && !k.mods.alt => {
                let w = self.windows.last_mut().unwrap();
                let line = w.ed.cursor_line_col().0;
                let bg = &w.ed.line_bg;
                let starts: Vec<usize> = (0..bg.len()).filter(|&i| bg[i] != 0 && (i == 0 || bg[i - 1] == 0)).collect();
                // (Side by side: changed rows have words but no colour.)
                let starts: Vec<usize> = if starts.is_empty() { (0..w.ed.words.len()).filter(|&i| !w.ed.words[i].is_empty() && (i == 0 || w.ed.words[i - 1].is_empty())).collect() } else { starts };
                let to = if c == 'n' { starts.iter().find(|&&s| s > line) } else { starts.iter().rev().find(|&&s| s < line) };
                if let Some(&l) = to {
                    w.ed.cursor = w.ed.doc.rope.line_to_char(l);
                    w.ed.anchor = None;
                }
                true
            }
            (PageKind::Diff { .. }, Key::Enter) => {
                let place = self.preview.previews.get(&id).and_then(|p| p.rendered.as_ref()).and_then(|r| {
                    let line = self.windows.last().unwrap().ed.cursor_line_col().0;
                    r.places.get(line).cloned().flatten()
                });
                match place {
                    Some((p, l)) if p.exists() => self.goto_file(&p, Some(l), 0),
                    Some((p, _)) => self.say(format!("{} isn't here", p.display())),
                    None => {}
                }
                true
            }
            (PageKind::Log, Key::Enter) => {
                let w = self.windows.last().unwrap();
                let line = w.ed.doc.line(w.ed.cursor_line_col().0);
                let hash: String = line.chars().skip_while(|c| !c.is_ascii_hexdigit()).take_while(|c| c.is_ascii_hexdigit()).collect();
                if hash.len() >= 7 {
                    let root = self.preview.previews.get(&id).map(|p| p.root.clone()).unwrap_or_default();
                    self.show_git_page(&format!("Commit {hash}"), PageKind::Diff { side_by_side: false }, &root, vec!["show".into(), "--stat".into(), "-p".into(), "--format=commit %H%nAuthor: %an <%ae>%nDate:   %ad%n%n    %s%n%n%b".into(), hash]);
                }
                true
            }
            _ => false,
        }
    }

    /// Ctrl+Shift+V: the front Markdown file rendered beside it, following
    /// its edits (a .patch or .diff as a diff).
    pub(super) fn open_preview(&mut self) {
        let Some(i) = self.windows.iter().rposition(|w| w.term.is_none() && w.special.is_none() && w.dock.is_none()) else { return };
        let kind = match self.windows[i].lang.as_deref() {
            Some("markdown") => PageKind::Markdown,
            Some("diff") => PageKind::Diff { side_by_side: false },
            _ => {
                self.say("Preview is for Markdown files and patches");
                return;
            }
        };
        let (src, title, base) = {
            let w = &self.windows[i];
            (w.win.id, w.ed.doc.title(), w.ed.doc.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)))
        };
        // (Already open: to the front.)
        if let Some((&id, _)) = self.preview.previews.iter().find(|(_, p)| matches!(p.source, Source::Window(s) if s == src)) {
            if let Some(j) = self.windows.iter().position(|w| w.win.id == id) {
                let w = self.windows.remove(j);
                self.windows.push(w);
                self.renumber();
                return;
            }
        }
        self.show_plain(&format!("Preview {title}"), "Rendering…\n");
        let w = self.windows.last_mut().unwrap();
        w.special = Some("preview");
        let id = w.win.id;
        let root = base.clone().map(|b| lsp::project_root(&b.join("x"))).unwrap_or_default();
        self.preview.previews.insert(id, Preview { source: Source::Window(src), base, asked: None, changed_at: None, generation: 0, rendered: None, jump: None, kind, root, back: Vec::new() });
        // Side by side with its file.
        self.shell.split = Some((src, id, 1));
    }

    /// Each tick: render what changed (a file's preview a moment after
    /// typing stops), and show what's rendered.
    pub(super) fn preview_tick(&mut self, now: Instant) {
        let ids: Vec<u32> = self.preview.previews.keys().copied().collect();
        for id in ids {
            let Some(w) = self.windows.iter().find(|w| w.win.id == id) else {
                self.preview.previews.remove(&id);
                continue;
            };
            let width = (w.win.inner().w.max(20) as usize).saturating_sub(2);
            let p = self.preview.previews.get(&id).unwrap();
            let (version, md) = match &p.source {
                Source::Pending => continue,
                Source::Text(t) => (0, Some(t.clone())),
                Source::Window(src) => match self.windows.iter().find(|w| w.win.id == *src) {
                    Some(s) => (s.ed.doc.version, None),
                    None => {
                        // (Its file closed: the preview goes too.)
                        self.preview.previews.remove(&id);
                        if let Some(i) = self.windows.iter().position(|w| w.win.id == id) {
                            self.windows.remove(i);
                            self.renumber();
                        }
                        continue;
                    }
                },
            };
            if p.asked == Some((version, width)) {
                continue;
            }
            // (Typing in the file: wait until it pauses, but not for a resize.)
            let p = self.preview.previews.get_mut(&id).unwrap();
            let typing = matches!(p.source, Source::Window(_)) && p.asked.is_some_and(|(v, _)| v != version);
            if typing {
                match p.changed_at {
                    Some((v, t)) if v == version => {
                        if now.duration_since(t) < Duration::from_millis(150) {
                            continue;
                        }
                    }
                    _ => {
                        p.changed_at = Some((version, now));
                        continue;
                    }
                }
            }
            let md = match (&p.source, md) {
                (_, Some(m)) => m,
                (Source::Window(src), None) => self.windows.iter().find(|w| w.win.id == *src).map(|w| w.ed.doc.rope.to_string()).unwrap_or_default(),
                _ => String::new(),
            };
            p.asked = Some((version, width));
            p.generation += 1;
            let _ = self.preview.tx.send((id, p.generation, md, width, p.kind, p.root.clone()));
        }
        // Git's output for pages waiting on it.
        let texts: Vec<(u32, Result<String, String>)> = self.preview.text_rx.try_iter().collect();
        for (id, t) in texts {
            match t {
                Ok(t) => {
                    if let Some(p) = self.preview.previews.get_mut(&id) {
                        p.source = Source::Text(t);
                    }
                }
                Err(e) => {
                    self.preview.previews.remove(&id);
                    if let Some(w) = self.windows.iter_mut().find(|w| w.win.id == id) {
                        w.ed.doc = Document::from_str(&format!("git: {e}\n"));
                    }
                }
            }
        }
        let done: Vec<(u32, u64, Rendered)> = self.preview.rx.try_iter().collect();
        for (id, generation, r) in done {
            let Some(p) = self.preview.previews.get_mut(&id) else { continue };
            if p.generation != generation {
                continue;
            }
            let Some(w) = self.windows.iter_mut().find(|w| w.win.id == id) else { continue };
            // (A re-render keeps the place: the same share of the way down.)
            let first = p.rendered.is_none();
            let old_lines = w.ed.doc.len_lines().max(1);
            let (scroll, cursor_line) = (w.ed.scroll_y, w.ed.cursor_line_col().0);
            w.ed.doc = Document::from_str(&r.text);
            w.ed.highlights = r.spans.clone();
            w.ed.line_bg = r.line_bg.clone();
            w.ed.words = r.words.clone();
            let n = w.ed.doc.len_lines().max(1);
            if first {
                w.ed.cursor = 0;
                w.ed.scroll_y = 0;
            } else {
                w.ed.scroll_y = scroll * n / old_lines;
                let l = (cursor_line * n / old_lines).min(n - 1);
                w.ed.cursor = w.ed.doc.rope.line_to_char(l);
            }
            if let Some(h) = p.jump.take() {
                if let Some((l, _, _)) = r.headings.iter().find(|(_, _, t)| *t == h) {
                    w.ed.cursor = w.ed.doc.rope.line_to_char(*l);
                    w.ed.scroll_y = *l;
                }
            }
            p.rendered = Some(r);
        }
    }

    pub(super) fn preview_busy(&self) -> bool {
        self.preview.previews.values().any(|p| p.rendered.is_none() || p.asked.is_none() || matches!(p.source, Source::Pending))
            || self.preview.previews.iter().any(|(_, p)| matches!(p.source, Source::Window(_)) && p.changed_at.is_some())
    }

    /// The link under the front window's cursor, if it's a rendered page.
    fn link_here(&self) -> Option<(u32, String)> {
        let w = self.windows.last()?;
        let p = self.preview.previews.get(&w.win.id)?;
        let r = p.rendered.as_ref()?;
        let (line, byte) = w.ed.doc.char_to_line_byte(w.ed.cursor);
        r.links.iter().find(|(l, a, b, _)| *l == line && *a <= byte && byte < *b).map(|(_, _, _, t)| (w.win.id, t.clone()))
    }

    /// Follow the link at the cursor. True when there was one.
    pub(super) fn follow_link(&mut self) -> bool {
        let Some((id, target)) = self.link_here() else { return false };
        match target.as_str() {
            "help:index" => {
                self.run_command("help.index");
                return true;
            }
            "help:back" => {
                self.page_back();
                return true;
            }
            "help:contents" => {
                // (The page's own contents, else its top.)
                let line = self.preview.previews.get(&id).and_then(|p| p.rendered.as_ref()).and_then(|r| r.anchors.iter().find(|(s, _)| s == "contents").map(|(_, l)| *l)).unwrap_or(0);
                self.page_go(id, line);
                return true;
            }
            _ => {}
        }
        if let Some(anchor) = target.strip_prefix('#') {
            let line = self.preview.previews.get(&id).and_then(|p| p.rendered.as_ref()).and_then(|r| r.anchors.iter().find(|(s, _)| s == anchor).map(|(_, l)| *l));
            match line {
                Some(l) => self.page_go(id, l),
                None => self.say(format!("No heading {target} here")),
            }
            return true;
        }
        if target.contains("://") || target.starts_with("mailto:") {
            let r = std::process::Command::new("xdg-open").arg(&target).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
            self.say(match r {
                Ok(_) => format!("Opening {target}"),
                Err(e) => format!("Can't open {target}: {e}"),
            });
            return true;
        }
        let base = self.preview.previews.get(&id).and_then(|p| p.base.clone()).unwrap_or_else(|| self.shell.explorer.root.clone());
        let (file, _) = target.split_once('#').unwrap_or((&target, ""));
        let p = base.join(file);
        if p.exists() {
            self.open_path(&p);
        } else {
            self.say(format!("No file {}", p.display()));
        }
        true
    }
}

impl Workbench {
    /// Go to a line of a page, noting where it was for Back.
    fn page_go(&mut self, id: u32, line: usize) {
        let Some(w) = self.windows.iter_mut().find(|w| w.win.id == id) else { return };
        let here = w.ed.cursor_line_col().0;
        w.ed.cursor = w.ed.doc.rope.line_to_char(line.min(w.ed.doc.len_lines().saturating_sub(1)));
        w.ed.scroll_y = line;
        w.ed.anchor = None;
        if let Some(p) = self.preview.previews.get_mut(&id) {
            p.back.push(here);
        }
    }

    /// Back (Alt+F1): where the front page was before its last link.
    pub(super) fn page_back(&mut self) {
        let Some(id) = self.windows.last().map(|w| w.win.id) else { return };
        let Some(line) = self.preview.previews.get_mut(&id).and_then(|p| p.back.pop()) else {
            self.say("Nowhere to go back to");
            return;
        };
        let w = self.windows.last_mut().unwrap();
        w.ed.cursor = w.ed.doc.rope.line_to_char(line.min(w.ed.doc.len_lines().saturating_sub(1)));
        w.ed.scroll_y = line.saturating_sub(2);
        w.ed.anchor = None;
    }
}

/// A commit list (git log --format="%h\x1f%s\x1f%an\x1f%ar"): one a line.
fn log_page(text: &str) -> Rendered {
    let mut r = Rendered::default();
    let k = |n: &str| turbine_syntax::kind_index(n).unwrap_or(0);
    for l in text.lines() {
        let f: Vec<&str> = l.split('\x1f').collect();
        let (hash, subject, who, when) = (f.first().copied().unwrap_or(""), f.get(1).copied().unwrap_or(""), f.get(2).copied().unwrap_or(""), f.get(3).copied().unwrap_or(""));
        let line = format!("{hash}  {subject}  ({who}, {when})");
        let a = hash.len() as u32;
        let b = a + 2 + subject.len() as u32;
        r.spans.push(vec![(0, a, k("number")), (b, line.len() as u32, k("comment"))]);
        r.text.push_str(&line);
        r.text.push('\n');
    }
    r
}
