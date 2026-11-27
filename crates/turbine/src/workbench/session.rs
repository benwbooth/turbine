//! The session, kept between runs per project as VS Code keeps it: the
//! files open (in their order, the front one last) with their cursors and
//! scroll, text not yet saved (hot exit: Exit doesn't ask, the text comes
//! back next time), folders open in the Explorer, breakpoints and whether
//! the panel had a terminal. Written next to look.json, off the UI thread,
//! a moment after anything changes; read when the front end sets the look.

use super::*;
use std::hash::{Hash, Hasher};

#[derive(Default)]
pub(super) struct SessionState {
    /// The last JSON written (only changes are written).
    last: String,
    /// When it was last looked at.
    checked: Option<Instant>,
    /// Unsaved text written: window id → the document version.
    backed_up: HashMap<u32, u64>,
    /// Restored already (the look can be set again in tests).
    pub restored: bool,
}

impl Workbench {
    /// Where this project's session goes (None without a look path: tests
    /// and runs that keep nothing).
    fn session_dir(&self) -> Option<PathBuf> {
        let base = self.look_path.as_ref()?.parent()?.join("sessions");
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.shell.explorer.root.hash(&mut h);
        let name = self.shell.explorer.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "root".into());
        Some(base.join(format!("{}-{:016x}", crate::markdown::slug(&name), h.finish())))
    }

    /// Whether Exit can keep unsaved text instead of asking.
    pub(super) fn hot_exit(&self) -> bool {
        self.look_path.is_some()
    }

    fn session_windows(&self) -> impl Iterator<Item = &EdWin> {
        self.windows.iter().filter(|w| w.term.is_none() && w.special.is_none() && w.dock.is_none() && w.mirror.is_none())
    }

    fn session_json(&self) -> Value {
        let files: Vec<Value> = self
            .session_windows()
            .map(|w| {
                let e = &w.ed;
                json!({
                    "id": w.win.id,
                    "path": e.doc.path.as_ref().map(|p| p.display().to_string()),
                    "cursor": e.cursor,
                    "anchor": e.anchor,
                    "scroll_y": e.scroll_y,
                    "scroll_x": e.scroll_x,
                    "dirty": e.doc.dirty,
                    "lang": w.lang_fixed,
                    "zoomed": w.win.zoomed,
                })
            })
            .collect();
        let mut expanded: Vec<String> = self.shell.explorer.expanded.iter().map(|p| p.display().to_string()).collect();
        expanded.sort();
        let bps: serde_json::Map<String, Value> = self.breakpoints.iter().filter(|(_, l)| !l.is_empty()).map(|(p, l)| (p.display().to_string(), json!(l))).collect();
        json!({
            "project": self.shell.explorer.root.display().to_string(),
            "files": files,
            "expanded": expanded,
            "breakpoints": bps,
            "terminal": self.windows.iter().any(|w| w.term.is_some() && w.dock.is_some()),
        })
    }

    /// Each tick: a second after the last look, write what changed.
    pub(super) fn session_tick(&mut self, now: Instant) {
        if !self.session.restored || self.session.checked.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
            return;
        }
        self.session.checked = Some(now);
        self.save_session(false);
    }

    /// Write the session (and unsaved text) if it changed; `wait` writes
    /// it before returning (the app's last moment).
    pub fn save_session(&mut self, wait: bool) {
        let Some(dir) = self.session_dir() else { return };
        if !self.session.restored {
            return;
        }
        let v = self.session_json();
        // Unsaved text: each document whose version moved on.
        let mut writes: Vec<(PathBuf, String)> = Vec::new();
        let mut keep: HashSet<u32> = HashSet::new();
        for w in self.session_windows() {
            if !w.ed.doc.dirty {
                continue;
            }
            keep.insert(w.win.id);
            if self.session.backed_up.get(&w.win.id) != Some(&w.ed.doc.version) {
                writes.push((dir.join(format!("unsaved-{}.txt", w.win.id)), w.ed.doc.rope.to_string()));
            }
        }
        let versions: Vec<(u32, u64)> = self.session_windows().filter(|w| keep.contains(&w.win.id)).map(|w| (w.win.id, w.ed.doc.version)).collect();
        self.session.backed_up.extend(versions);
        self.session.backed_up.retain(|id, _| keep.contains(id));
        let text = serde_json::to_string_pretty(&v).unwrap_or_default();
        if text == self.session.last && writes.is_empty() {
            return;
        }
        self.session.last = text.clone();
        let job = move || {
            let _ = std::fs::create_dir_all(&dir);
            for (p, t) in writes {
                let _ = std::fs::write(&p, t);
            }
            // (Unsaved copies no window holds any more go.)
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let id = name.strip_prefix("unsaved-").and_then(|n| n.strip_suffix(".txt")).and_then(|n| n.parse::<u32>().ok());
                    if id.is_some_and(|id| !keep.contains(&id)) {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            let tmp = dir.join("session.json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, dir.join("session.json"));
            }
        };
        if wait {
            job();
        } else {
            std::thread::spawn(job);
        }
    }

    /// Open last time's files again (once, when the look is set).
    pub(super) fn restore_session(&mut self) {
        if self.session.restored {
            return;
        }
        self.session.restored = true;
        let Some(dir) = self.session_dir() else { return };
        let Ok(text) = std::fs::read_to_string(dir.join("session.json")) else { return };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { return };
        if let Some(a) = v["expanded"].as_array() {
            self.shell.explorer.restore_expanded(a.iter().filter_map(|x| x.as_str().map(PathBuf::from)));
        }
        if let Some(m) = v["breakpoints"].as_object() {
            for (p, lines) in m {
                let set: BTreeSet<u32> = lines.as_array().into_iter().flatten().filter_map(|x| x.as_u64().map(|n| n as u32)).collect();
                if !set.is_empty() && Path::new(p).exists() {
                    self.breakpoints.entry(PathBuf::from(p)).or_default().extend(set);
                }
            }
        }
        // (An empty Untitled the start made gives way to the session's files.)
        let blank = self.windows.len() == 1 && self.windows[0].term.is_none() && self.windows[0].ed.doc.path.is_none() && !self.windows[0].ed.doc.dirty && self.windows[0].ed.doc.rope.len_chars() == 0;
        let open_before: HashSet<PathBuf> = self.windows.iter().filter_map(|w| w.ed.doc.path.clone()).collect();
        let mut opened = 0;
        for f in v["files"].as_array().into_iter().flatten() {
            let path = f["path"].as_str().map(PathBuf::from);
            let unsaved = f["dirty"].as_bool() == Some(true);
            let backup = f["id"].as_u64().map(|id| dir.join(format!("unsaved-{id}.txt"))).filter(|_| unsaved).and_then(|p| std::fs::read_to_string(p).ok());
            let doc = match (&path, backup) {
                (Some(p), _) if open_before.contains(p) => continue,
                // Unsaved changes to a file: the file, then the changes as
                // one edit (Undo goes back to what's on disk).
                (Some(p), Some(text)) => {
                    let mut d = Document::open(p).unwrap_or_else(|_| {
                        let mut d = Document::new();
                        d.path = Some(p.clone());
                        d
                    });
                    d.replace_all_minimal(&text, 0);
                    d.dirty = true;
                    d
                }
                (Some(p), None) if p.is_file() => match Document::open(p) {
                    Ok(d) => d,
                    Err(_) => continue,
                },
                (Some(_), None) => continue,
                (None, Some(text)) => {
                    let mut d = Document::new();
                    d.replace_all_minimal(&text, 0);
                    d.dirty = true;
                    d
                }
                (None, None) => continue,
            };
            self.new_window(doc);
            let w = self.windows.last_mut().unwrap();
            let len = w.ed.doc.rope.len_chars();
            w.ed.cursor = f["cursor"].as_u64().map_or(0, |n| (n as usize).min(len));
            w.ed.anchor = f["anchor"].as_u64().map(|n| (n as usize).min(len));
            w.ed.scroll_y = f["scroll_y"].as_u64().map_or(0, |n| (n as usize).min(w.ed.doc.len_lines().saturating_sub(1)));
            w.ed.scroll_x = f["scroll_x"].as_u64().unwrap_or(0) as usize;
            w.lang_fixed = f["lang"].as_str().map(str::to_string);
            if f["zoomed"].as_bool() == Some(false) {
                let d = self.desktop();
                let w = self.windows.last_mut().unwrap();
                if w.win.zoomed {
                    w.win.toggle_zoom(d);
                }
            }
            opened += 1;
        }
        if blank && opened > 0 {
            self.windows.remove(0);
            self.renumber();
        }
        if v["terminal"].as_bool() == Some(true) && !self.windows.iter().any(|w| w.term.is_some()) {
            self.new_terminal(Vec::new());
            self.layout_dock();
            if let Some(id) = self.windows.last().map(|w| w.win.id) {
                self.shell.dock.show(crate::dock::Tab::Pane(id));
            }
            // (The editor in front, as before.)
            if let Some(i) = self.windows.iter().rposition(|w| w.term.is_none() && w.dock.is_none()) {
                let w = self.windows.remove(i);
                self.windows.push(w);
                self.renumber();
            }
        }
    }
}
