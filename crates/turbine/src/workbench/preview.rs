//! Markdown shown rendered (crate::markdown), as Tessera's guide viewer
//! shows a guide: Help windows and guides always, and a live preview of a
//! Markdown file beside it (Ctrl+Shift+V). Rendering happens on a thread
//! of its own; a preview follows its window's width and its file's edits.
//! Enter (or a click) on a link follows it: a heading in the page, a file,
//! or a web page in the browser.

use super::*;
use crate::markdown::{self, Rendered};
use std::sync::mpsc::{Sender, channel};

pub(super) enum Source {
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
    /// A heading to go to once it's rendered.
    pub jump: Option<String>,
}

type Job = (u32, u64, String, usize);

pub(super) struct PreviewState {
    pub previews: HashMap<u32, Preview>,
    tx: Sender<Job>,
    rx: Receiver<(u32, u64, Rendered)>,
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
                    for (_, (id, generation, md, width)) in latest {
                        if done.send((id, generation, markdown::render(&md, width))).is_err() {
                            return;
                        }
                    }
                }
            })
            .expect("markdown thread");
        PreviewState { previews: HashMap::new(), tx, rx }
    }
}

impl Workbench {
    /// A read-only window titled `title` showing `md` rendered.
    pub(super) fn show_rendered(&mut self, title: &str, md: &str, base: Option<PathBuf>) {
        self.show_plain(title, "");
        let w = self.windows.last_mut().unwrap();
        w.ed.doc = Document::from_str("Rendering…\n");
        let id = w.win.id;
        self.preview.previews.insert(id, Preview { source: Source::Text(md.to_string()), base, asked: None, changed_at: None, generation: 0, rendered: None, jump: None });
    }

    /// Ctrl+Shift+V: the front Markdown file rendered beside it, following
    /// its edits.
    pub(super) fn open_preview(&mut self) {
        let Some(i) = self.windows.iter().rposition(|w| w.term.is_none() && w.special.is_none() && w.dock.is_none()) else { return };
        let (src, title, base) = {
            let w = &self.windows[i];
            if w.lang.as_deref() != Some("markdown") {
                self.say("Preview is for Markdown files");
                return;
            }
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
        self.preview.previews.insert(id, Preview { source: Source::Window(src), base, asked: None, changed_at: None, generation: 0, rendered: None, jump: None });
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
            let _ = self.preview.tx.send((id, p.generation, md, width));
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
        self.preview.previews.values().any(|p| p.rendered.is_none() || p.asked.is_none())
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
        if let Some(anchor) = target.strip_prefix('#') {
            let line = self.preview.previews.get(&id).and_then(|p| p.rendered.as_ref()).and_then(|r| r.anchors.iter().find(|(s, _)| s == anchor).map(|(_, l)| *l));
            match line {
                Some(l) => {
                    let w = self.windows.last_mut().unwrap();
                    w.ed.cursor = w.ed.doc.rope.line_to_char(l);
                    w.ed.scroll_y = l;
                    w.ed.anchor = None;
                }
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
