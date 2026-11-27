//! The workbench's Cursor-style shell: the docked parts (side bars and the
//! panel) and their views, the editor tabs, and the keys, clicks and
//! commands for them. See `crate::dock` for the geometry.

use super::*;
use crate::dock::{Dock, Layout, Tab};
use crate::palette::{self, Palette, Pick};
use std::collections::HashMap;
use turbine_ui::list::{ListOut, Row};

/// Where typing goes, besides dialogs and menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Focus {
    /// The active window (an editor, or a docked terminal or log).
    Editors,
    /// A side bar or panel view.
    View(Tab),
    /// The AI chat's question field.
    Chat,
    /// The editor's find box.
    Find,
    /// Ctrl+K's prompt box.
    Inline,
}

/// The views' state.
pub(super) struct Shell {
    pub dock: Dock,
    pub focus: Focus,
    pub explorer: Explorer,
    pub outline: Outline,
    pub search: Search,
    pub scm: Scm,
    pub problems: ListView,
    /// Where each Problems row goes (None for a file's heading).
    pub problem_at: Vec<Option<(PathBuf, usize, usize)>>,
    pub problem_count: (usize, usize),
    pub debug_view: ListView,
    pub extensions: Extensions,
    pub chat_input: LineInput,
    /// The editor whose file the Explorer last showed.
    pub revealed: Option<(u32, Option<PathBuf>)>,
    /// Places of the editor tabs from the last layout: (window id, tab, ×).
    pub editor_tab_rects: Vec<(u32, Rect, Rect)>,
    /// The command palette / quick open, while it's up.
    pub palette: Option<Palette>,
    /// The project came from the command line (else the last one returns).
    pub explicit_project: bool,
    /// The chat sends to the agent (which works the UI) instead of asking.
    pub chat_agent: bool,
    /// Split editors: the windows in the left and right halves, and which
    /// half was used last (a file chosen next goes there).
    pub split: Option<(u32, u32, usize)>,
    /// Ctrl+\ with one editor: the file picked next opens beside it.
    pub split_pending: Option<u32>,
    /// Long lines wrap (new editors too).
    pub word_wrap: bool,
    /// The preview tab (VS Code's): a file clicked once in the Explorer
    /// opens here and the next one replaces it, until it's kept (edited,
    /// double-clicked, or its tab double-clicked).
    pub preview_tab: Option<u32>,
    /// The last click on an editor tab (a double click keeps a preview).
    pub tab_click: Option<(u32, Instant)>,
    /// The cursor: "Block", "Underline" or "Bar", and whether it blinks.
    pub cursor_style: String,
    pub cursor_blink: bool,
    /// Each mirror group's document as last synced: (version, dirty, path).
    pub mirrored: HashMap<u32, (u64, bool, Option<PathBuf>)>,
    /// The breadcrumbs from the last layout: their places, labels and where
    /// each goes.
    pub crumbs: Vec<(Rect, String, Crumb)>,
    /// Editors show a minimap (None: as the control style says).
    pub minimap: Option<bool>,
    /// Editors number their lines (None: as the control style says).
    pub line_numbers: Option<bool>,
}

/// Where a breadcrumb goes.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Crumb {
    Folder(PathBuf),
    File,
    Symbol(usize),
}

impl Shell {
    pub fn new(root: PathBuf, explicit_project: bool) -> Shell {
        Shell {
            dock: Dock::default(),
            focus: Focus::Editors,
            explorer: Explorer::new(root),
            outline: Outline::default(),
            search: Search::default(),
            scm: Scm::default(),
            problems: ListView::new(),
            problem_at: Vec::new(),
            problem_count: (0, 0),
            debug_view: ListView::new(),
            extensions: Extensions::default(),
            chat_input: LineInput::default(),
            revealed: None,
            editor_tab_rects: Vec::new(),
            palette: None,
            explicit_project,
            chat_agent: false,
            split: None,
            split_pending: None,
            word_wrap: false,
            preview_tab: None,
            tab_click: None,
            cursor_style: "Block".into(),
            cursor_blink: true,
            mirrored: HashMap::new(),
            crumbs: Vec::new(),
            minimap: None,
            line_numbers: None,
        }
    }
}

/// A file's icon by its type (editor tabs, breadcrumbs): a Nerd Font
/// glyph, one cell, in the text's own colour.
pub(super) fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" => "\u{e7a8}",
        "py" | "pyi" => "\u{e73c}",
        "md" | "markdown" => "\u{e73e}",
        "js" | "mjs" | "ts" | "tsx" | "jsx" => "\u{e74e}",
        "json" => "\u{eb0f}",
        "toml" => "\u{e6b2}",
        "nix" => "\u{f313}",
        "yaml" | "yml" | "ini" | "cfg" | "conf" | "lock" => "\u{e615}",
        "c" | "h" => "\u{e61e}",
        "cc" | "cpp" | "hpp" | "cxx" => "\u{e61d}",
        "go" => "\u{e626}",
        "java" => "\u{e738}",
        "lua" => "\u{e620}",
        "sh" | "bash" | "zsh" | "fish" => "\u{f489}",
        "html" | "htm" => "\u{e736}",
        "css" => "\u{e749}",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "bmp" => "\u{eaea}",
        _ => "\u{ea7b}",
    }
}

/// An editor tab's text: the file's icon and name, * when changed.
fn editor_tab_name(w: &EdWin, icons: bool) -> String {
    let name = w.ed.doc.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| w.win.title.clone());
    let icon = if icons { format!("{} ", file_icon(&name)) } else { String::new() };
    format!(" {icon}{name}{} ", if w.ed.doc.dirty { "*" } else { "" })
}

/// The project a file belongs to (its repository or build root), else the
/// current folder.
pub(super) fn project_of(files: &[PathBuf]) -> PathBuf {
    files
        .first()
        .and_then(|f| std::fs::canonicalize(f).ok())
        .map(|f| if f.is_dir() { f } else { turbine_services::lsp::project_root(&f) })
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

impl Workbench {
    /// The parts and the editors' area on this screen.
    pub(super) fn layout(&self) -> Layout {
        let mut dock = self.shell.dock.clone();
        // (A front editor with no file: no breadcrumbs, its name is the tab.)
        dock.crumbs_hidden = self.editor_index().is_some_and(|i| self.windows[i].ed.doc.path.is_none() && self.windows[i].special.is_none());
        dock.layout(Rect::new(0, 1, self.size.0, (self.size.1 - 2).max(1)))
    }

    /// The window being edited: the front window that isn't docked.
    pub(super) fn editor_index(&self) -> Option<usize> {
        self.windows.iter().rposition(|w| w.dock.is_none())
    }

    /// Match the parts' tabs to the docked windows, put those windows in
    /// their parts, and keep the editors inside their area.
    pub(super) fn layout_dock(&mut self) {
        // Tabs for new docked windows; none for closed ones.
        let ids: HashSet<u32> = self.windows.iter().map(|w| w.win.id).collect();
        for side in [Side::Left, Side::Bottom, Side::Right] {
            let gone: Vec<Tab> = self.shell.dock.part(side).tabs.iter().copied().filter(|t| matches!(t, Tab::Pane(id) if !ids.contains(id))).collect();
            for t in gone {
                self.shell.dock.remove(t);
            }
        }
        for w in &self.windows {
            if let Some(side) = w.dock {
                let t = Tab::Pane(w.win.id);
                if self.shell.dock.side_of(t).is_none() {
                    self.shell.dock.add(side, t);
                }
            }
        }
        if let Focus::View(t) = self.shell.focus {
            if self.shell.dock.side_of(t).is_none_or(|s| !self.shell.dock.part(s).visible) {
                self.shell.focus = Focus::Editors;
            }
        }
        self.shell.dock.icons = self.theme.style.icons;
        // (Minimaps in every editor, as set or as the style has it.)
        let mm = self.shell.minimap.unwrap_or(self.theme.style.icons);
        for w in self.windows.iter_mut().filter(|w| w.dock.is_none() && w.term.is_none()) {
            w.ed.minimap = mm && w.special.is_none();
        }
        let l = self.layout();
        self.layout_editor_tabs(&l);
        self.layout_crumbs(&l);
        // (The views' lists learn their places here too, so keys and agents
        // know them before anything is drawn.)
        for side in [Side::Left, Side::Bottom, Side::Right] {
            let (Some(r), Some(t)) = (l.part(side), self.shell.dock.part(side).current()) else { continue };
            let inner = Dock::content(r);
            let below_field = Rect::new(inner.x, inner.y + 2, inner.w, (inner.h - 2).max(0));
            let area = if matches!(t, Tab::Search | Tab::Scm) { below_field } else { inner };
            if let Some(list) = self.view_list(t) {
                list.area = area;
            }
        }
        // Split editors: the pair side by side; a file brought to the front
        // takes the half used last.
        let front = self.editor_index().map(|i| self.windows[i].win.id);
        if let Some(pending) = self.shell.split_pending {
            if front.is_some_and(|f| f != pending) && self.shell.palette.is_none() {
                self.shell.split = Some((pending, front.unwrap(), 1));
                self.shell.split_pending = None;
            } else if self.shell.palette.is_none() {
                self.shell.split_pending = None;
            }
        }
        if let Some((a, b, active)) = self.shell.split {
            let alive = |id: u32| ids.contains(&id) && self.windows.iter().any(|w| w.win.id == id && w.dock.is_none());
            if !alive(a) || !alive(b) {
                self.shell.split = None;
            } else if let Some(f) = front {
                self.shell.split = Some(if f == a {
                    (a, b, 0)
                } else if f == b {
                    (a, b, 1)
                } else if active == 0 {
                    (f, b, 0)
                } else {
                    (a, f, 1)
                });
            }
        }
        let halves = self.shell.split.map(|(a, b, _)| {
            let e = l.editors;
            let lw = e.w / 2;
            ((a, Rect::new(e.x, e.y, lw, e.h)), (b, Rect::new(e.x + lw, e.y, e.w - lw, e.h)))
        });
        for w in &mut self.windows {
            if let (None, Some(((a, ra), (b, rb)))) = (w.dock, halves) {
                if w.win.id == a || w.win.id == b {
                    w.win.zoomed = false;
                    w.win.rect = if w.win.id == a { ra } else { rb };
                    continue;
                }
            }
            match w.dock {
                Some(side) => {
                    let shown = self.shell.dock.part(side).visible && self.shell.dock.part(side).current() == Some(Tab::Pane(w.win.id));
                    w.win.zoomed = false;
                    w.win.rect = match (shown, l.part(side)) {
                        // (The chat keeps two rows at the bottom for its question.)
                        (true, Some(r)) if w.special == Some("chat") => Rect::new(r.x, r.y, r.w, (r.h - 2).max(3)),
                        (true, Some(r)) => r,
                        _ => Rect::default(),
                    };
                }
                None => w.win.fit(l.editors),
            }
        }
    }

    /// Where the editor tabs go: one per window on the desktop, in the
    /// order they were opened, as many as fit.
    fn layout_editor_tabs(&mut self, l: &Layout) {
        self.shell.editor_tab_rects.clear();
        let Some(tr) = l.editor_tabs else { return };
        let mut eds: Vec<&EdWin> = self.windows.iter().filter(|w| w.dock.is_none()).collect();
        eds.sort_by_key(|w| w.win.id);
        let icons = self.theme.style.icons;
        let mut x = tr.x;
        for w in eds {
            // (Text, ×, and a rounded end or a │.)
            let wdt = str_width(&editor_tab_name(w, icons)) as i32 + 2 + i32::from(icons);
            if x + wdt > tr.right() {
                break;
            }
            let x0 = x + i32::from(icons);
            self.shell.editor_tab_rects.push((w.win.id, Rect::new(x0, tr.y, wdt - 1 - i32::from(icons), 1), Rect::new(x + wdt - 2, tr.y, 1, 1)));
            x += wdt;
        }
    }

    /// The breadcrumbs: the file's folders from the project down, the file,
    /// and the definitions the cursor is in. A long path loses its start.
    fn layout_crumbs(&mut self, l: &Layout) {
        self.shell.crumbs.clear();
        let Some(r) = l.breadcrumbs else { return };
        let Some(i) = self.editor_index() else { return };
        let w = &self.windows[i];
        let mut items: Vec<(String, Crumb)> = Vec::new();
        let icons = self.theme.style.icons;
        match &w.ed.doc.path {
            Some(p) => {
                let root = &self.shell.explorer.root;
                let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
                let rel = p.strip_prefix(root).ok();
                let mut dir = if rel.is_some() { root.clone() } else { PathBuf::from("/") };
                if rel.is_some() {
                    items.push((root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()), Crumb::Folder(root.clone())));
                }
                let parts: Vec<String> = rel.unwrap_or(&p).iter().map(|c| c.to_string_lossy().into_owned()).filter(|c| c != "/").collect();
                for (k, part) in parts.iter().enumerate() {
                    dir.push(part);
                    if k + 1 == parts.len() {
                        let icon = if icons { format!("{} ", file_icon(part)) } else { String::new() };
                        items.push((format!("{icon}{part}"), Crumb::File));
                    } else {
                        items.push((part.clone(), Crumb::Folder(dir.clone())));
                    }
                }
            }
            // (A page with no file: its window's title, HELP: Turbine.)
            None if w.special.is_some() => items.push((w.win.title.clone(), Crumb::File)),
            None => items.push((w.ed.doc.title(), Crumb::File)),
        }
        let line = w.ed.cursor_line_col().0;
        if self.shell.outline.shows(w.win.id) {
            for (icon, name, l) in self.shell.outline.chain(line) {
                items.push((format!("{icon} {name}"), Crumb::Symbol(l)));
            }
        }
        // Fit: drop from the start (keeping the file and symbols).
        let sep = 3;
        let total = |it: &[(String, Crumb)]| it.iter().map(|(t, _)| str_width(t) as i32 + sep).sum::<i32>();
        let mut dropped = false;
        while total(&items) > r.w - 3 && items.len() > 1 && matches!(items[0].1, Crumb::Folder(_)) {
            items.remove(0);
            dropped = true;
        }
        let mut x = r.x + 1;
        if dropped {
            x += 2;
        }
        for (t, c) in items {
            let wdt = str_width(&t) as i32;
            if x + wdt > r.right() {
                break;
            }
            self.shell.crumbs.push((Rect::new(x, r.y, wdt, 1), t, c));
            x += wdt + sep;
        }
    }

    /// The part that has focus, for its frame.
    fn focused_side(&self) -> Option<Side> {
        if self.dialog.is_some() || self.menubar.is_open() {
            return None;
        }
        match self.shell.focus {
            Focus::View(t) => self.shell.dock.side_of(t),
            Focus::Chat => Some(Side::Right),
            Focus::Find | Focus::Inline => None,
            Focus::Editors => self.windows.last().and_then(|w| w.dock),
        }
    }

    /// A tab's title (the counts on Problems, a docked window's title).
    fn tab_title(&self, t: Tab) -> String {
        let name = match t {
            Tab::Pane(id) => self.windows.iter().find(|w| w.win.id == id).map(|w| w.win.title.clone()).unwrap_or_default(),
            Tab::Problems if self.shell.problem_count != (0, 0) => {
                format!("Problems {}", self.shell.problem_count.0 + self.shell.problem_count.1)
            }
            t => t.name().to_string(),
        };
        if !self.theme.style.icons {
            return name;
        }
        let icon = match t {
            Tab::Explorer => "\u{eaf0}",
            Tab::Outline => "\u{eb86}",
            Tab::Search => "\u{ea6d}",
            Tab::Scm => "\u{ea68}",
            Tab::Debug => "\u{eb91}",
            Tab::Extensions => "\u{eae6}",
            Tab::Problems => "\u{ea6c}",
            Tab::Pane(id) => match self.windows.iter().find(|w| w.win.id == id) {
                Some(w) if w.term.is_some() => "\u{ea85}",
                Some(w) if w.special == Some("chat") => "\u{eac7}",
                Some(w) if w.special == Some("debug") => "\u{eaaf}",
                _ => "\u{eb9d}",
            },
        };
        format!("{icon} {name}")
    }

    // ---- keeping the views current ----------------------------------------------------

    pub(super) fn update_views(&mut self) {
        let shown = |wb: &Workbench, t: Tab| wb.shell.dock.side_of(t).is_some_and(|s| wb.shell.dock.part(s).visible && wb.shell.dock.part(s).current() == Some(t));
        let explorer_shown = shown(self, Tab::Explorer);
        self.shell.explorer.poll(explorer_shown);
        let root = self.shell.explorer.root.clone();
        self.shell.search.poll(&root);
        let ext_shown = shown(self, Tab::Extensions);
        let installed = self.extension_list();
        self.shell.extensions.set_installed(installed.iter().map(|i| (i.name.clone(), i.description.clone(), i.error.clone(), i.loaded)).collect());
        self.shell.extensions.poll(ext_shown);
        let scm_shown = shown(self, Tab::Scm);
        self.shell.scm.poll(&root, scm_shown);
        // The Explorer follows the editor in front.
        if let Some(i) = self.editor_index() {
            let w = &self.windows[i];
            let key = (w.win.id, w.ed.doc.path.clone());
            if self.shell.revealed.as_ref() != Some(&key) {
                if let Some(p) = &key.1 {
                    let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
                    self.shell.explorer.reveal(&p);
                }
                self.shell.revealed = Some(key);
            }
        }
        // The Outline (and Go to Symbol): the editor in front, when one shows.
        if self.shell.outline.poll() {
            self.palette_symbols();
        }
        let symbols_wanted = self.shell.palette.as_ref().is_some_and(|p| p.input.text.starts_with('@')) || self.shell.dock.breadcrumbs;
        if shown(self, Tab::Outline) || symbols_wanted {
            match self.editor_index() {
                Some(i) if self.windows[i].special.is_none() => {
                    let w = &self.windows[i];
                    let (id, v, lang) = (w.win.id, w.ed.doc.version, w.lang.clone());
                    let line = w.ed.cursor_line_col().0;
                    let rope = w.ed.doc.rope.clone();
                    self.shell.outline.update(id, v, lang.as_deref(), || rope.to_string());
                    if self.shell.focus != Focus::View(Tab::Outline) {
                        self.shell.outline.follow(line);
                    }
                }
                _ => self.shell.outline.clear(),
            }
        }
        self.update_problems();
        self.update_debug_view();
        if let Some(p) = &mut self.shell.palette {
            p.poll();
        }
    }

    // ---- the command palette and quick open -------------------------------------------

    /// Every menu command, as "Menu: Item".
    fn palette_commands(&self) -> Vec<palette::Item> {
        let mut out = Vec::new();
        for m in &self.menubar.menus {
            let menu = m.plain_title();
            let menu = if menu == "≡" { "Turbine".to_string() } else { menu };
            for it in &m.items {
                if it.is_separator() || !it.enabled {
                    continue;
                }
                // (On/off items read as toggles, as in VS Code.)
                let toggle = if it.checked.is_some() { "Toggle " } else { "" };
                out.push(palette::Item {
                    label: format!("{menu}: {toggle}{}", it.plain_label().trim_end_matches("...")),
                    detail: it.shortcut.clone(),
                    pick: Pick::Command(it.command.clone()),
                });
            }
        }
        out
    }

    /// The Outline's symbols into an open palette.
    fn palette_symbols(&mut self) {
        let syms: Vec<palette::Item> = self
            .shell
            .outline
            .list
            .rows
            .iter()
            .zip(&self.shell.outline.lines)
            .map(|(r, l)| palette::Item { label: format!("{} {}", r.icon, r.text), detail: format!("{}", l + 1), pick: Pick::Line(*l) })
            .collect();
        if let Some(p) = &mut self.shell.palette {
            p.set_symbols(syms);
        }
    }

    pub(super) fn open_palette(&mut self, start: &str) {
        let root = self.shell.explorer.root.clone();
        let p = Palette::new(start, self.palette_commands(), Vec::new(), &root);
        self.shell.palette = Some(p);
        self.palette_symbols();
    }

    fn palette_pick(&mut self, i: usize) {
        let Some(p) = self.shell.palette.take() else { return };
        match p.pick(i) {
            Some(Pick::Command(c)) => self.run_command(&c),
            Some(Pick::Open(f)) => self.goto_file(&f, None, 0),
            Some(Pick::Place(f, line, col16)) => {
                self.goto_file(&f, Some(line), 0);
                if let Some(w) = self.windows.last_mut() {
                    let line = line.min(w.ed.doc.len_lines().saturating_sub(1));
                    let b = lsp::byte_of_utf16(&w.ed.doc.line(line), col16 as u32);
                    w.ed.cursor = w.ed.doc.line_byte_to_char(line, b);
                }
            }
            Some(Pick::Line(l)) => {
                if let Some(ei) = self.editor_index() {
                    let id = self.windows[ei].win.id;
                    self.activate_pane(id);
                    let w = self.windows.last_mut().unwrap();
                    let l = l.min(w.ed.doc.len_lines().saturating_sub(1));
                    w.ed.cursor = w.ed.doc.rope.line_to_char(l);
                    w.ed.anchor = None;
                }
            }
            None => self.shell.palette = Some(p),
        }
    }

    /// Where the palette goes: a box at the top of the editors.
    fn palette_rect(&self) -> Rect {
        let l = self.layout();
        let area = Rect::new(0, 1, self.size.0, (self.size.1 - 2).max(1));
        let w = 72.min(area.w - 4).max(20);
        let rows = self.shell.palette.as_ref().map_or(1, |p| p.list.rows.len() as i32);
        let h = (rows + 3).clamp(5, 18).min(area.h);
        let cx = l.editors.x + l.editors.w / 2;
        let x = (cx - w / 2).clamp(area.x, (area.right() - w).max(area.x));
        Rect::new(x, area.y, w, h)
    }

    fn palette_key(&mut self, k: &KeyEvent) {
        let Some(p) = &mut self.shell.palette else { return };
        match k.key {
            Key::Esc => self.shell.palette = None,
            Key::Enter => {
                let i = p.list.selected;
                self.palette_pick(i);
            }
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                p.list.key(k);
            }
            _ => {
                if p.input.key(k) {
                    p.refilter();
                }
            }
        }
    }

    fn update_problems(&mut self) {
        let mut rows = Vec::new();
        let mut at = Vec::new();
        let (mut errors, mut warnings) = (0, 0);
        for w in &self.windows {
            if w.ed.diagnostics.is_empty() || w.term.is_some() {
                continue;
            }
            let path = w.ed.doc.path.clone().unwrap_or_default();
            let mut diags: Vec<_> = w.ed.diagnostics.iter().collect();
            diags.sort_by_key(|d| (d.severity, d.line));
            rows.push(Row::new(w.ed.doc.title()).heading().detail(format!("{}", diags.len())));
            at.push(None);
            for d in diags {
                let (icon, fg) = match d.severity {
                    1 => {
                        errors += 1;
                        ("■", vga::LIGHT_RED)
                    }
                    2 => {
                        warnings += 1;
                        ("▲", vga::YELLOW)
                    }
                    _ => ("i", vga::LIGHT_CYAN),
                };
                rows.push(Row::new(d.message.lines().next().unwrap_or("").to_string()).indent(1).icon(icon).fg(fg).detail(format!("{}:{}", d.line + 1, d.start + 1)));
                at.push(Some((path.clone(), d.line, d.start)));
            }
        }
        // (Files a build reported on that aren't open.)
        for (path, probs) in self.closed_file_problems() {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            rows.push(Row::new(name).heading().detail(format!("{}", probs.len())));
            at.push(None);
            for p in probs {
                let (icon, fg) = match p.severity {
                    1 => {
                        errors += 1;
                        ("■", vga::LIGHT_RED)
                    }
                    2 => {
                        warnings += 1;
                        ("▲", vga::YELLOW)
                    }
                    _ => ("i", vga::LIGHT_CYAN),
                };
                rows.push(Row::new(p.message.lines().next().unwrap_or("").to_string()).indent(1).icon(icon).fg(fg).detail(format!("{}:{}", p.line + 1, p.col + 1)));
                at.push(Some((path.clone(), p.line, p.col)));
            }
        }
        if rows.is_empty() {
            rows.push(Row::new("No problems have been detected."));
            at.push(None);
        }
        if self.shell.problem_at != at || self.shell.problems.rows.len() != rows.len() {
            self.shell.problems.set_rows(rows);
            self.shell.problem_at = at;
        }
        self.shell.problem_count = (errors, warnings);
    }

    fn update_debug_view(&mut self) {
        let mut rows = Vec::new();
        // Watches first (Delete removes one), then the stack and variables.
        if !self.dbx.watches.is_empty() {
            rows.push(Row::new("Watch").heading());
            for l in self.watch_lines() {
                rows.push(Row::new(l.trim().to_string()).indent(1));
            }
        }
        let bps: Vec<(PathBuf, u32)> = self.breakpoints.iter().flat_map(|(p, ls)| ls.iter().map(move |l| (p.clone(), *l))).collect();
        if !bps.is_empty() {
            rows.push(Row::new("Breakpoints").heading());
            for (p, l) in bps {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let o = self.dbx.bp_opts.get(&p).and_then(|m| m.get(&l));
                let how = match o {
                    Some(o) if !o.log_message.is_empty() => format!("log: {}", o.log_message),
                    Some(o) if !o.condition.is_empty() => format!("when {}", o.condition),
                    Some(o) if !o.hit_condition.is_empty() => format!("hit {}", o.hit_condition),
                    _ => String::new(),
                };
                rows.push(Row::new(format!("{name}:{l}")).indent(1).detail(how));
            }
        }
        match &self.debug {
            None => {
                rows.push(Row::new("Not debugging."));
                rows.push(Row::new("Run > Start Debugging (or F7)").fg(vga::LIGHT_GRAY));
            }
            Some(d) => {
                rows.push(Row::new("Call Stack").heading());
                for f in &d.frames {
                    let place = f.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    rows.push(Row::new(f.name.clone()).indent(1).detail(format!("{place}:{}", f.line)));
                }
                for (scope, vars) in &d.scopes {
                    rows.push(Row::new(scope.clone()).heading());
                    for v in vars {
                        rows.push(Row::new(format!("{} = {}", v.name, v.value)).indent(1).detail(v.kind.clone()));
                    }
                }
            }
        }
        if self.shell.debug_view.rows != rows {
            self.shell.debug_view.set_rows(rows);
        }
    }

    // ---- going places ---------------------------------------------------------------------

    /// Open a file (or bring its window to the front) and put the cursor at
    /// a line and column; the editor gets focus.
    pub(super) fn goto_file(&mut self, p: &Path, line: Option<usize>, col: usize) {
        self.nav_push();
        let want = Self::canon(p);
        match self.windows.iter().position(|w| w.dock.is_none() && w.ed.doc.path.as_ref().is_some_and(|q| Self::canon(q) == want)) {
            Some(i) => {
                let w = self.windows.remove(i);
                self.windows.push(w);
                self.renumber();
            }
            None => self.open_path(p),
        }
        if let (Some(line), Some(w)) = (line, self.windows.last_mut()) {
            let line = line.min(w.ed.doc.len_lines().saturating_sub(1));
            let start = w.ed.doc.rope.line_to_char(line);
            let len = w.ed.doc.rope.line(line).len_chars();
            w.ed.cursor = start + col.min(len.saturating_sub(1));
            w.ed.anchor = None;
        }
        self.shell.focus = Focus::Editors;
    }

    /// Make a folder the project: the Explorer shows it, the Search and
    /// Source Control views work in it, and it's remembered.
    pub(super) fn open_folder(&mut self, p: &Path) {
        let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        self.shell.explorer.set_root(p.clone());
        self.shell.revealed = None;
        self.shell.scm = Scm::default();
        self.shell.search = Search::default();
        self.show_tab(Tab::Explorer);
        self.say(format!("Opened folder {}", p.display()));
        self.save_look();
    }

    /// Bring a docked window to the front (it gets the keys).
    fn activate_pane(&mut self, id: u32) {
        if let Some(i) = self.windows.iter().position(|w| w.win.id == id) {
            let w = self.windows.remove(i);
            self.windows.push(w);
            self.renumber();
        }
        self.shell.focus = Focus::Editors;
    }

    /// Back to the editor in front (from a view or a docked window).
    fn focus_editor(&mut self) {
        self.shell.focus = Focus::Editors;
        if let Some(i) = self.editor_index() {
            if i + 1 != self.windows.len() {
                let w = self.windows.remove(i);
                self.windows.push(w);
                self.renumber();
            }
        }
    }

    /// Show a tab and give it focus.
    pub(super) fn show_tab(&mut self, t: Tab) {
        self.shell.dock.show(t);
        match t {
            Tab::Pane(id) => self.activate_pane(id),
            Tab::Search => {
                self.shell.search.in_field = true;
                self.shell.focus = Focus::View(t);
            }
            _ => self.shell.focus = Focus::View(t),
        }
    }

    /// The AI chat in the right side bar, its question field focused.
    fn show_chat(&mut self) {
        if !self.windows.iter().any(|w| w.special == Some("chat")) {
            self.append_chat("");
        }
        self.layout_dock();
        if let Some(id) = self.windows.iter().find(|w| w.special == Some("chat")).map(|w| w.win.id) {
            self.shell.dock.show(Tab::Pane(id));
        }
        self.shell.dock.right.visible = true;
        self.shell.focus = Focus::Chat;
    }

    /// Ctrl+\: split the editors (or join them back).
    fn toggle_split(&mut self) {
        if self.shell.split.take().is_some() {
            let d = self.layout().editors;
            for w in self.windows.iter_mut().filter(|w| w.dock.is_none()) {
                w.win.zoomed = false;
                w.win.toggle_zoom(d);
            }
            return;
        }
        let eds: Vec<u32> = self.windows.iter().rev().filter(|w| w.dock.is_none()).map(|w| w.win.id).collect();
        match eds.as_slice() {
            [front, other, ..] => self.shell.split = Some((*other, *front, 1)),
            [only] => {
                // One editor: the same file beside it (as VS Code does);
                // an edit in either shows in both.
                let only = *only;
                let Some(i) = self.windows.iter().position(|w| w.win.id == only) else { return };
                let src = &self.windows[i];
                let group = src.mirror.unwrap_or(only);
                let mut ed = EditorView::new(src.ed.doc.clone());
                ed.line_numbers = src.ed.line_numbers;
                ed.cursor = src.ed.cursor;
                ed.scroll_y = src.ed.scroll_y;
                ed.highlights = src.ed.highlights.clone();
                let lang = src.lang.clone();
                let mut win = Window::new(self.next_id, &src.win.title, src.win.rect);
                win.toggle_zoom(self.layout().editors);
                self.next_id += 1;
                let id = win.id;
                self.windows[i].mirror = Some(group);
                let vim = (self.keymap == Keymap::Vim).then(Session::new);
                self.windows.push(EdWin {
                    win,
                    ed,
                    vim,
                    term: None,
                    term_text: String::new(),
                    lang,
                    lang_for: None,
                    lang_fixed: None,
                    hl_sent: None,
                    lsp_key: None,
                    lsp_sent: None,
                    was_dirty: false,
                    special: None,
                    dock: None,
                    mirror: Some(group),
                });
                self.renumber();
                self.shell.split = Some((only, id, 1));
            }
            [] => {}
        }
        self.focus_editor();
    }

    /// Windows showing one document: copy the one that changed (an edit,
    /// an undo, a save, Save As) to the others, undo history and all.
    pub(super) fn sync_mirrors(&mut self) {
        let groups: HashSet<u32> = self.windows.iter().filter_map(|w| w.mirror).collect();
        for g in groups {
            let members: Vec<usize> = (0..self.windows.len()).filter(|&i| self.windows[i].mirror == Some(g)).collect();
            if members.len() < 2 {
                // (Alone again: no longer a mirror.)
                for i in members {
                    self.windows[i].mirror = None;
                }
                self.shell.mirrored.remove(&g);
                continue;
            }
            let state = |w: &EdWin| (w.ed.doc.version, w.ed.doc.dirty, w.ed.doc.path.clone());
            let last = self.shell.mirrored.get(&g).cloned();
            let changed = members.iter().copied().find(|&i| Some(state(&self.windows[i])) != last);
            if let Some(src) = changed {
                let doc = self.windows[src].ed.doc.clone();
                let title = self.windows[src].win.title.clone();
                for &i in &members {
                    if i != src {
                        let w = &mut self.windows[i];
                        w.ed.doc = doc.clone();
                        w.win.title = title.clone();
                        let n = w.ed.doc.rope.len_chars();
                        w.ed.cursor = w.ed.cursor.min(n);
                        w.ed.anchor = w.ed.anchor.map(|a| a.min(n));
                    }
                }
                self.shell.mirrored.insert(g, state(&self.windows[src]));
            }
        }
    }

    /// Toggle a part; showing one focuses its view.
    fn toggle_part(&mut self, side: Side) {
        let p = self.shell.dock.part_mut(side);
        p.visible = !p.visible && !p.tabs.is_empty();
        if p.visible {
            if let Some(t) = p.current() {
                self.show_tab(t);
            }
        } else if self.focused_side() == Some(side) {
            self.focus_editor();
        }
    }

    /// The chat's mode pill and its field's hint.
    fn chat_mode_label(&self, icons: bool) -> (String, &'static str) {
        match (self.shell.chat_agent, icons) {
            (true, true) => (" \u{f06a9} Agent ".into(), "What should the agent do? (Enter)"),
            (true, false) => (" Agent ".into(), "What should the agent do? (Enter)"),
            (false, true) => (" \u{eac7} Ask ".into(), "Ask about the code… (Enter)"),
            (false, false) => (" Ask ".into(), "Ask about the code… (Enter)"),
        }
    }

    // ---- remembering the layout --------------------------------------------------------

    pub(super) fn layout_json(&self) -> Value {
        let d = &self.shell.dock;
        let part = |p: &crate::dock::Part| json!({"visible": p.visible, "size": p.size, "active": p.current().filter(|t| !matches!(t, Tab::Pane(_))).map(|t| t.key())});
        json!({"left": part(&d.left), "bottom": part(&d.bottom), "right": part(&d.right), "editor_tabs": d.editor_tabs, "breadcrumbs": d.breadcrumbs, "minimap": self.shell.minimap})
    }

    pub(super) fn apply_layout_json(&mut self, v: &Value) {
        if !v.is_object() {
            return;
        }
        for (side, key) in [(Side::Left, "left"), (Side::Bottom, "bottom"), (Side::Right, "right")] {
            let pv = &v[key];
            let p = self.shell.dock.part_mut(side);
            if let Some(b) = pv["visible"].as_bool() {
                p.visible = b;
            }
            if let Some(n) = pv["size"].as_i64() {
                p.size = (n as i32).clamp(4, 400);
            }
            if let Some(a) = pv["active"].as_str() {
                if let Some(i) = p.tabs.iter().position(|t| t.key() == a) {
                    p.active = i;
                }
            }
        }
        if let Some(b) = v["editor_tabs"].as_bool() {
            self.shell.dock.editor_tabs = b;
        }
        if let Some(b) = v["breadcrumbs"].as_bool() {
            self.shell.dock.breadcrumbs = b;
        }
        self.shell.minimap = v["minimap"].as_bool();
    }

    // ---- commands -------------------------------------------------------------------------

    /// The shell's commands; false when it's not one of them.
    pub(super) fn shell_command(&mut self, cmd: &str) -> bool {
        match cmd {
            "view.files" => self.show_tab(Tab::Explorer),
            "view.outline" => self.show_tab(Tab::Outline),
            "view.search" | "search.files" => self.show_tab(Tab::Search),
            "view.scm" => self.show_tab(Tab::Scm),
            "view.debug" => self.show_tab(Tab::Debug),
            "view.extensions" => {
                self.shell.extensions.load();
                self.show_tab(Tab::Extensions);
            }
            "view.problems" => {
                self.shell.dock.add(Side::Bottom, Tab::Problems);
                self.show_tab(Tab::Problems);
            }
            "view.output" => {
                let i = self.special_window("output");
                let id = self.windows[i].win.id;
                self.layout_dock();
                self.show_tab(Tab::Pane(id));
            }
            "view.terminal" => {
                // The terminal in the panel: show it, or start one; again
                // from the terminal goes back to the editor.
                let term = self.windows.iter().rev().find(|w| w.term.is_some() && w.dock.is_some()).map(|w| w.win.id);
                match term {
                    Some(id) if self.windows.last().is_some_and(|w| w.win.id == id) && self.shell.dock.bottom.visible => self.focus_editor(),
                    Some(id) => {
                        self.shell.dock.show(Tab::Pane(id));
                        self.activate_pane(id);
                    }
                    None => {
                        self.new_terminal(Vec::new());
                        self.layout_dock();
                        if let Some(id) = self.windows.last().map(|w| w.win.id) {
                            self.shell.dock.show(Tab::Pane(id));
                        }
                    }
                }
            }
            "view.sidebar" => self.toggle_part(Side::Left),
            "view.panel" => self.toggle_part(Side::Bottom),
            "view.secondary" => self.toggle_part(Side::Right),
            "view.editor_tabs" => self.shell.dock.editor_tabs = !self.shell.dock.editor_tabs,
            "view.breadcrumbs" => self.shell.dock.breadcrumbs = !self.shell.dock.breadcrumbs,
            "view.minimap" => self.shell.minimap = Some(!self.shell.minimap.unwrap_or(self.theme.style.icons)),
            "ai.chat" => self.show_chat(),
            "view.focus_editor" => self.focus_editor(),
            "view.split" => self.toggle_split(),
            "view.reset_layout" => {
                let tabs = (self.shell.dock.bottom.tabs.clone(), self.shell.dock.right.tabs.clone());
                self.shell.dock = Dock::default();
                self.shell.dock.bottom.tabs = tabs.0;
                self.shell.dock.right.tabs = tabs.1;
            }
            "view.palette" => self.open_palette(">"),
            "ai.inline" => {
                self.focus_editor();
                self.inline_start();
            }
            "ai.chat_mode" => self.shell.chat_agent = !self.shell.chat_agent,
            "search.find" => {
                self.focus_editor();
                self.open_find(false);
            }
            "search.replace" => {
                self.focus_editor();
                self.open_find(true);
            }
            "search.again" => self.find_step(true),
            "go.file" => self.open_palette(""),
            "go.symbol" => self.open_palette("@"),
            _ => return false,
        }
        self.layout_dock();
        if cmd.starts_with("view.") && !matches!(cmd, "view.palette" | "view.focus_editor") {
            self.save_look();
        }
        true
    }

    // ---- keys -----------------------------------------------------------------------------

    /// Layout keys (wherever focus is) and keys for a focused view. True
    /// when used.
    pub(super) fn shell_key(&mut self, k: &KeyEvent) -> bool {
        let ch = |c: char| matches!(k.key, Key::Char(x) if x.eq_ignore_ascii_case(&c));
        let cs = k.mods.ctrl && k.mods.shift && !k.mods.alt;
        // (Vim keeps Ctrl+B and Ctrl+J in its editors: they're its keys.)
        let vim_editor = self.shell.focus == Focus::Editors && self.keymap == Keymap::Vim && self.windows.last().is_some_and(|w| w.dock.is_none() && !w.is_terminal());
        if self.shell.palette.is_some() {
            self.palette_key(k);
            return true;
        }
        if self.ex_key(k) {
            return true;
        }
        // Extensions' key bindings.
        if let Some(cmd) = self.ext_key(k) {
            self.run_command(&cmd);
            return true;
        }
        let cmd = if k.mods.ctrl && !k.mods.alt && matches!(k.key, Key::Char(',')) {
            Some("options.settings")
        } else if k.is_ctrl('k') && !k.mods.shift && !vim_editor && self.shell.focus == Focus::Editors && self.windows.last().is_some_and(|w| w.dock.is_none()) {
            Some("ai.inline")
        } else if k.is_ctrl('f') && !k.mods.shift && !vim_editor && !self.windows.last().is_some_and(|w| w.is_terminal() && self.shell.focus == Focus::Editors) {
            Some("search.find")
        } else if k.is_ctrl('h') && !k.mods.shift && !vim_editor && self.shell.focus == Focus::Editors && self.windows.last().is_some_and(|w| w.dock.is_none()) {
            Some("search.replace")
        } else if cs && ch('p') {
            Some("view.palette")
        } else if k.is_ctrl('p') && !vim_editor {
            Some("go.file")
        } else if cs && ch('o') {
            Some("go.symbol")
        } else if k.is_ctrl('t') && !vim_editor && !self.windows.last().is_some_and(|w| w.is_terminal() && self.shell.focus == Focus::Editors) {
            Some("go.workspace_symbol")
        } else if cs && ch('e') {
            Some("view.files")
        } else if cs && ch('f') {
            Some("view.search")
        } else if cs && ch('g') {
            Some("view.scm")
        } else if cs && ch('d') {
            Some("view.debug")
        } else if cs && ch('x') {
            Some("view.extensions")
        } else if cs && ch('m') {
            Some("view.problems")
        } else if cs && ch('u') {
            Some("view.output")
        } else if k.mods.ctrl && k.mods.alt && ch('b') {
            Some("view.secondary")
        } else if cs && ch('b') {
            Some("terminal.build")
        } else if k.is_ctrl('b') && !vim_editor {
            Some("view.sidebar")
        } else if k.is_ctrl('j') && !vim_editor {
            Some("view.panel")
        } else if k.mods.ctrl && !k.mods.alt && matches!(k.key, Key::Char('\\')) {
            Some("view.split")
        } else if k.is_ctrl('l') && !self.windows.last().is_some_and(|w| w.is_terminal() && self.shell.focus == Focus::Editors) {
            Some("ai.chat")
        } else {
            None
        };
        if let Some(c) = cmd {
            self.run_command(c);
            return true;
        }
        match self.shell.focus {
            Focus::Editors => false,
            Focus::Find => {
                self.find_key(k);
                true
            }
            Focus::Inline => {
                self.inline_key(k);
                true
            }
            Focus::Chat => {
                self.chat_key(k);
                true
            }
            Focus::View(t) => {
                if k.key == Key::Esc {
                    self.focus_editor();
                } else {
                    self.view_key(t, k);
                }
                true
            }
        }
    }

    fn chat_key(&mut self, k: &KeyEvent) {
        if k.is_ctrl('.') {
            self.shell.chat_agent = !self.shell.chat_agent;
            return;
        }
        match k.key {
            Key::Esc => self.focus_editor(),
            Key::Enter => {
                let q = std::mem::take(&mut self.shell.chat_input.text);
                self.shell.chat_input.cursor = 0;
                if !q.trim().is_empty() {
                    if self.shell.chat_agent {
                        self.append_chat(&format!("\n## You (to the agent)\n\n{}\n\n", q.trim()));
                        self.start_agent(q.trim());
                    } else {
                        self.ask(q.trim());
                    }
                }
            }
            Key::PageUp | Key::PageDown => {
                if let Some(w) = self.windows.iter_mut().find(|w| w.special == Some("chat")) {
                    let mut clip = String::new();
                    w.ed.handle_key(k, &mut clip);
                }
            }
            _ => {
                self.shell.chat_input.key(k);
            }
        }
    }

    fn view_key(&mut self, t: Tab, k: &KeyEvent) {
        let root = self.shell.explorer.root.clone();
        match t {
            Tab::Explorer => match {
                let out = self.shell.explorer.list.key(k);
                if out != ListOut::Ignored {
                    self.shell.explorer.user_selected();
                }
                out
            } {
                ListOut::Activated(i) => {
                    if let Some(p) = self.shell.explorer.activate(i) {
                        self.goto_file(&p, None, 0);
                    }
                }
                ListOut::Collapse(i) => self.shell.explorer.collapse(i),
                ListOut::Expand(i) => self.shell.explorer.expand(i),
                _ => {}
            },
            Tab::Outline => match self.shell.outline.list.key(k) {
                ListOut::Selected(i) => self.outline_goto(i, false),
                ListOut::Activated(i) => self.outline_goto(i, true),
                _ => {}
            },
            Tab::Search => {
                let s = &mut self.shell.search;
                if s.in_field {
                    match k.key {
                        Key::Enter => s.run(&root),
                        Key::Down | Key::Tab if !s.list.rows.is_empty() => s.in_field = false,
                        _ => {
                            s.query.key(k);
                        }
                    }
                } else {
                    match k.key {
                        Key::Up if s.list.selected == 0 => s.in_field = true,
                        Key::Tab => s.in_field = true,
                        _ => {
                            if let ListOut::Activated(i) = s.list.key(k) {
                                if let Some(Some((p, line))) = s.hits.get(i).cloned() {
                                    self.goto_file(&p, Some(line), 0);
                                }
                            }
                        }
                    }
                }
            }
            Tab::Scm => {
                let s = &mut self.shell.scm;
                if k.key == Key::Enter && k.mods.ctrl {
                    if let Err(e) = s.commit(&root) {
                        self.say(e);
                    }
                    return;
                }
                if s.in_field {
                    match k.key {
                        Key::Enter => {
                            if let Err(e) = s.commit(&root) {
                                self.say(e);
                            }
                        }
                        Key::Down | Key::Tab => s.in_field = false,
                        _ => {
                            s.message.key(k);
                        }
                    }
                } else {
                    match k.key {
                        Key::Up if s.list.selected == 0 => s.in_field = true,
                        Key::Tab => s.in_field = true,
                        Key::Char(' ') => {
                            let i = s.list.selected;
                            s.toggle_stage(&root, i);
                        }
                        // o: the file itself (Enter shows its changes).
                        Key::Char('o') => {
                            if let Some(Some((_, p))) = s.entries.get(s.list.selected).cloned() {
                                self.goto_file(&root.join(p), None, 0);
                            }
                        }
                        _ => {
                            if let ListOut::Activated(i) = s.list.key(k) {
                                if let Some(Some((staged, p))) = s.entries.get(i).cloned() {
                                    self.scm_diff(staged, &p);
                                }
                            }
                        }
                    }
                }
            }
            Tab::Problems => {
                if let ListOut::Activated(i) = self.shell.problems.key(k) {
                    self.problem_goto(i);
                }
            }
            Tab::Debug => {
                // Delete on a watch removes it.
                let sel = self.shell.debug_view.selected;
                if k.key == Key::Delete && !self.dbx.watches.is_empty() && (1..=self.dbx.watches.len()).contains(&sel) {
                    self.remove_watch(sel - 1);
                    return;
                }
                self.shell.debug_view.key(k);
            }
            Tab::Extensions => match self.shell.extensions.list.key(k) {
                ListOut::Selected(i) | ListOut::Activated(i) => {
                    if let Some(d) = self.shell.extensions.describe(i) {
                        self.say(d);
                    }
                }
                _ => {}
            },
            Tab::Pane(_) => {}
        }
    }

    fn outline_goto(&mut self, i: usize, focus: bool) {
        let Some(&line) = self.shell.outline.lines.get(i) else { return };
        if let Some(ei) = self.editor_index() {
            let w = &mut self.windows[ei];
            let line = line.min(w.ed.doc.len_lines().saturating_sub(1));
            w.ed.cursor = w.ed.doc.rope.line_to_char(line);
            w.ed.anchor = None;
            if focus {
                self.focus_editor();
            }
        }
    }

    pub(super) fn problem_goto(&mut self, i: usize) {
        if let Some(Some((p, line, col))) = self.shell.problem_at.get(i).cloned() {
            if p.as_os_str().is_empty() {
                return;
            }
            self.goto_file(&p, Some(line), col);
        }
    }

    // ---- the mouse ------------------------------------------------------------------------

    /// Clicks on the parts' edges and tabs, the views and the editor tabs.
    /// True when used. (Clicks inside a docked window go on to the window.)
    pub(super) fn shell_mouse(&mut self, m: &MouseEvent) -> bool {
        // The palette: a click picks; one outside closes it.
        if self.shell.palette.is_some() {
            let pr = self.palette_rect();
            if let MouseKind::Down(_) = m.kind {
                if !pr.contains(m.pos) {
                    self.shell.palette = None;
                    return true;
                }
            }
            let p = self.shell.palette.as_mut().unwrap();
            match p.list.mouse(m) {
                ListOut::Activated(i) | ListOut::Selected(i) if matches!(m.kind, MouseKind::Down(_)) => self.palette_pick(i),
                _ => {}
            }
            return true;
        }
        if self.ex_mouse(m) {
            return true;
        }
        let l = self.layout();
        if self.shell.dock.dragging() {
            match m.kind {
                MouseKind::Drag(_) => self.shell.dock.drag_to(m.pos),
                _ => {
                    self.shell.dock.end_drag();
                    self.save_look();
                }
            }
            self.layout_dock();
            return true;
        }
        // A view's list holding its scroll bar thumb gets the drag.
        if matches!(m.kind, MouseKind::Drag(_) | MouseKind::Up(_)) {
            if let Focus::View(t) = self.shell.focus {
                if let Some(list) = self.view_list(t) {
                    if list.mouse(m) == ListOut::Consumed {
                        return true;
                    }
                }
            }
        }
        let press = matches!(m.kind, MouseKind::Down(Button::Left));
        if press && self.shell.dock.begin_drag(&l, m.pos) {
            return true;
        }
        // Breadcrumbs: a folder shows in the Explorer, the file or a symbol
        // opens Go to Symbol.
        if let Some(br) = l.breadcrumbs {
            if br.contains(m.pos) {
                if press {
                    let hit = self.shell.crumbs.iter().find(|(r, ..)| r.contains(m.pos)).map(|(_, _, c)| c.clone());
                    match hit {
                        Some(Crumb::Folder(d)) => {
                            self.shell.explorer.expand_path(&d);
                            self.show_tab(Tab::Explorer);
                        }
                        Some(Crumb::File) | Some(Crumb::Symbol(_)) => self.open_palette("@"),
                        None => {}
                    }
                }
                return true;
            }
        }
        // Editor tabs.
        if let Some(tr) = l.editor_tabs {
            if tr.contains(m.pos) {
                if press {
                    let hit = self.shell.editor_tab_rects.iter().find(|(_, r, _)| r.contains(m.pos)).copied();
                    if let Some((id, _, x)) = hit {
                        // (A second click on the preview tab keeps it.)
                        let again = self.shell.tab_click.is_some_and(|(i, t)| i == id && t.elapsed() < Duration::from_millis(500));
                        self.shell.tab_click = Some((id, Instant::now()));
                        self.activate_pane(id);
                        if x.contains(m.pos) {
                            self.run_command("window.close");
                        } else if again {
                            self.keep_preview();
                        }
                    }
                }
                return true;
            }
        }
        for side in [Side::Left, Side::Bottom, Side::Right] {
            let Some(r) = l.part(side) else { continue };
            if !r.contains(m.pos) {
                continue;
            }
            // The top edge: tabs.
            if m.pos.y == r.y {
                if press {
                    let titles: Vec<(Tab, String)> = self.shell.dock.part(side).tabs.iter().map(|t| (*t, self.tab_title(*t))).collect();
                    let title = |t: Tab| titles.iter().find(|(x, _)| *x == t).map(|(_, s)| s.clone()).unwrap_or_default();
                    if let Some((t, _, _)) = self.shell.dock.tab_rects(side, r, &title).into_iter().find(|(_, tr, _)| tr.contains(m.pos)) {
                        self.show_tab(t);
                        self.layout_dock();
                    }
                }
                return true;
            }
            let cur = self.shell.dock.part(side).current();
            match cur {
                // A docked window: the chat's question field, else the window.
                Some(Tab::Pane(id)) => {
                    let is_chat = self.windows.iter().any(|w| w.win.id == id && w.special == Some("chat"));
                    if is_chat && m.pos.y >= r.bottom() - 3 {
                        if press {
                            self.shell.focus = Focus::Chat;
                            // (The mode pill at the row's start switches the mode.)
                            let (mode, _) = self.chat_mode_label(self.theme.style.icons);
                            let pill_w = str_width(&mode) as i32 + if self.theme.style.icons { 2 } else { 0 };
                            if m.pos.y == r.bottom() - 2 && m.pos.x <= r.x + pill_w {
                                self.shell.chat_agent = !self.shell.chat_agent;
                            }
                        }
                        return true;
                    }
                    if press {
                        self.shell.focus = Focus::Editors;
                    }
                    return false;
                }
                Some(t) => {
                    if press || matches!(m.kind, MouseKind::ScrollUp | MouseKind::ScrollDown) {
                        if press {
                            self.shell.focus = Focus::View(t);
                        }
                        self.view_mouse(t, m, Dock::content(r));
                    }
                    return true;
                }
                None => return true,
            }
        }
        // A click on the editors takes focus back from the views.
        if press && l.editors.contains(m.pos) {
            self.shell.focus = Focus::Editors;
        }
        false
    }

    fn view_list(&mut self, t: Tab) -> Option<&mut ListView> {
        Some(match t {
            Tab::Explorer => &mut self.shell.explorer.list,
            Tab::Outline => &mut self.shell.outline.list,
            Tab::Search => &mut self.shell.search.list,
            Tab::Scm => &mut self.shell.scm.list,
            Tab::Problems => &mut self.shell.problems,
            Tab::Debug => &mut self.shell.debug_view,
            Tab::Extensions => &mut self.shell.extensions.list,
            Tab::Pane(_) => return None,
        })
    }

    fn view_mouse(&mut self, t: Tab, m: &MouseEvent, content: Rect) {
        // The Search and Source Control fields sit on the first row.
        if matches!(t, Tab::Search | Tab::Scm) && m.pos.y == content.y {
            if matches!(m.kind, MouseKind::Down(_)) {
                match t {
                    Tab::Search => self.shell.search.in_field = true,
                    _ => self.shell.scm.in_field = true,
                }
            }
            return;
        }
        if matches!(m.kind, MouseKind::Down(_)) {
            match t {
                Tab::Search => self.shell.search.in_field = false,
                Tab::Scm => self.shell.scm.in_field = false,
                _ => {}
            }
        }
        let Some(list) = self.view_list(t) else { return };
        let out = list.mouse(m);
        if t == Tab::Explorer && matches!(out, ListOut::Selected(_) | ListOut::Activated(_)) {
            self.shell.explorer.user_selected();
        }
        match (t, out) {
            (Tab::Explorer, ListOut::Activated(i)) => {
                if let Some(p) = self.shell.explorer.activate(i) {
                    self.goto_file(&p, None, 0);
                    self.keep_preview();
                }
            }
            // One click on a file: a look at it, in the preview tab.
            (Tab::Explorer, ListOut::Selected(i)) if matches!(m.kind, MouseKind::Down(Button::Left)) => {
                if let Some(p) = self.shell.explorer.file_at(i) {
                    self.preview_file(&p);
                }
            }
            (Tab::Extensions, ListOut::Selected(i) | ListOut::Activated(i)) => {
                if let Some(d) = self.shell.extensions.describe(i) {
                    self.say(d);
                }
            }
            (Tab::Outline, ListOut::Selected(i)) => self.outline_goto(i, false),
            (Tab::Outline, ListOut::Activated(i)) => self.outline_goto(i, true),
            (Tab::Search, ListOut::Activated(i)) => {
                if let Some(Some((p, line))) = self.shell.search.hits.get(i).cloned() {
                    self.goto_file(&p, Some(line), 0);
                }
            }
            (Tab::Scm, ListOut::Activated(i)) => {
                let root = self.shell.explorer.root.clone();
                if let Some(Some((_, p))) = self.shell.scm.entries.get(i).cloned() {
                    self.goto_file(&root.join(p), None, 0);
                }
            }
            (Tab::Problems, ListOut::Activated(i) | ListOut::Selected(i)) => {
                if out == ListOut::Activated(i) {
                    self.problem_goto(i);
                }
            }
            _ => {}
        }
    }

    // ---- drawing --------------------------------------------------------------------------

    /// The palette, over everything but menus and dialogs.
    pub(super) fn draw_palette(&mut self, c: &mut Canvas, theme: &Theme) {
        if self.shell.palette.is_none() {
            return;
        }
        let r = self.palette_rect();
        let s = &theme.scheme;
        let p = self.shell.palette.as_mut().unwrap();
        c.fill(r, " ", s.menu);
        turbine_ui::frame::draw_frame(c, r, theme.style.menu_frame, s.menu);
        turbine_ui::frame::draw_title_in(c, r, p.title(), s.menu, 2);
        if theme.style.shadows {
            c.shadow(r, s.shadow);
        }
        p.input.draw(c, Rect::new(r.x + 1, r.y + 1, r.w - 2, 1), theme, true, "");
        // (The list shows its pick lit; the field keeps the cursor.)
        p.list.draw(c, Rect::new(r.x + 1, r.y + 2, r.w - 2, r.h - 3), theme, true);
        let before = str_width(&p.input.text[..p.input.cursor]) as i32;
        c.set_cursor(Point::new(r.x + 2 + before, r.y + 1), turbine_ui::CursorShape::Bar);
    }

    /// The editor tabs, the parts and their views (after the windows).
    pub(super) fn draw_shell(&mut self, c: &mut Canvas, theme: &Theme) {
        let l = self.layout();
        let s = &theme.scheme;
        // Editor tabs: one per window on the desktop, the front one lit.
        if let Some(tr) = l.editor_tabs {
            // (On the front window's top border: its line under the tabs.)
            let fw = self.editor_index().map(|i| &self.windows[i]);
            let line_style = if fw.is_some_and(|w| w.ed.help_look) { Style::new(s.help.fg, s.help.bg) } else { s.frame_active };
            let h = if theme.style.title_bar { " " } else { theme.style.active_frame.h };
            c.fill(tr, h, line_style);
            let front = self.editor_index().map(|i| self.windows[i].win.id);
            let mut tc = c.sub(tr);
            let icons = theme.style.icons;
            // The front tab as the originals title a window: a grey box
            // (QBasic's reverse title, the grey of the others' menus); the
            // rest plain on the border line.
            let active = if theme.style.title_reverse { s.frame_icons } else { s.menu };
            for (id, r, x) in &self.shell.editor_tab_rects {
                let Some(w) = self.windows.iter().find(|w| w.win.id == *id) else { continue };
                let on = Some(*id) == front;
                let stl = if on { active } else { line_style };
                // (The preview tab in italics, as VS Code shows it.)
                let stl = if self.shell.preview_tab == Some(*id) { Style { attrs: turbine_ui::theme::Attrs { italic: true, ..stl.attrs }, ..stl } } else { stl };
                tc.print(r.x, r.y, &editor_tab_name(w, icons), stl);
                tc.print(x.x, x.y, "×", stl);
                tc.print(x.x + 1, x.y, " ", stl);
            }
        }
        // Breadcrumbs.
        if let Some(br) = l.breadcrumbs {
            let row = s.crumbs;
            c.fill(br, " ", row);
            let mut bc = c.sub(br);
            if self.shell.crumbs.first().is_some_and(|(r, ..)| r.x > br.x + 1) {
                bc.print(br.x + 1, br.y, "…", row);
            }
            let n = self.shell.crumbs.len();
            for (k, (r, t, _)) in self.shell.crumbs.iter().enumerate() {
                bc.print(r.x, r.y, t, if k + 1 == n { s.crumbs_last } else { row });
                if k + 1 < n {
                    bc.print(r.right() + 1, r.y, "»", s.crumbs_sep);
                }
            }
        }
        let focused_side = self.focused_side();
        let modal = self.dialog.is_some() || self.menubar.is_open();
        for side in [Side::Left, Side::Bottom, Side::Right] {
            let Some(r) = l.part(side) else { continue };
            let focused = focused_side == Some(side);
            let titles: Vec<(Tab, String)> = self.shell.dock.part(side).tabs.iter().map(|t| (*t, self.tab_title(*t))).collect();
            let title = |t: Tab| titles.iter().find(|(x, _)| *x == t).map(|(_, s)| s.clone()).unwrap_or_default();
            self.shell.dock.draw_frame(c, theme, side, r, focused, &title);
            let inner = Dock::content(r);
            let view_focus = |t: Tab| focused && !modal && self.shell.focus == Focus::View(t);
            match self.shell.dock.part(side).current() {
                Some(Tab::Pane(id)) => {
                    let front = self.windows.last().is_some_and(|w| w.win.id == id) && self.shell.focus == Focus::Editors && !modal;
                    let chat_focus = self.shell.focus == Focus::Chat && !modal;
                    let Some(w) = self.windows.iter_mut().find(|w| w.win.id == id) else { continue };
                    let wi = w.win.inner();
                    if let Some(t) = &mut w.term {
                        t.draw(c, wi, theme, front);
                        if let Some((total, offset, len)) = t.terminal().and_then(|t| t.scrollbar()) {
                            if total > len {
                                let max = total - len;
                                turbine_ui::frame::draw_vscroll(c, r.right() - 1, r.y + 1, r.h - 2, offset.min(max), max + 1, s.scrollbar, theme.style.scroll);
                            }
                        }
                    } else {
                        w.ed.draw(c, wi, theme, front, front);
                        if front {
                            w.ed.draw_scrollbars(c, w.win.rect, theme);
                        }
                    }
                    if w.special == Some("chat") {
                        // The question field under the log.
                        let y = r.bottom() - 3;
                        let fc = if focused { theme.style.active_frame } else { theme.style.inactive_frame };
                        let fs = if focused { s.frame_active } else { s.frame };
                        c.print(r.x, y, if fc.v == "║" { "╟" } else { "├" }, fs);
                        c.hline(r.x + 1, y, r.w - 2, "─", fs);
                        c.print(r.right() - 1, y, if fc.v == "║" { "╢" } else { "┤" }, fs);
                        // The mode (Ctrl+. or a click switches it), then the field.
                        let (mode, hint) = self.chat_mode_label(theme.style.icons);
                        let pill = s.menu_selected;
                        let mx = r.x + 1;
                        let mut px = mx;
                        if theme.style.icons {
                            c.print(px, r.bottom() - 2, crate::dock::CAP_LEFT, Style::new(pill.bg, s.panel.bg));
                            px += 1;
                        }
                        let px = c.print(px, r.bottom() - 2, &mode, pill);
                        let px = if theme.style.icons { c.print(px, r.bottom() - 2, crate::dock::CAP_RIGHT, Style::new(pill.bg, s.panel.bg)) } else { px };
                        let field = Rect::new(px + 1, r.bottom() - 2, (r.right() - 1 - (px + 1)).max(4), 1);
                        self.shell.chat_input.draw(c, field, theme, chat_focus, hint);
                    }
                }
                Some(Tab::Search) => {
                    let f = view_focus(Tab::Search);
                    let sr = &mut self.shell.search;
                    sr.query.draw(c, Rect::new(inner.x, inner.y, inner.w, 1), theme, f && sr.in_field, "Search (Enter)");
                    let status = if sr.busy() { "Searching…".to_string() } else { sr.status.clone() };
                    c.sub(inner).print(inner.x + 1, inner.y + 1, &status, Style::new(s.panel_frame.fg, s.panel.bg));
                    sr.list.draw(c, Rect::new(inner.x, inner.y + 2, inner.w, (inner.h - 2).max(0)), theme, f && !sr.in_field);
                }
                Some(Tab::Scm) => {
                    let f = view_focus(Tab::Scm);
                    let sc = &mut self.shell.scm;
                    sc.message.draw(c, Rect::new(inner.x, inner.y, inner.w, 1), theme, f && sc.in_field, "Message (Enter commits)");
                    let info = if sc.status.is_empty() { format!("⎇ {}   Space stages", sc.branch) } else { sc.status.clone() };
                    c.sub(inner).print(inner.x + 1, inner.y + 1, &info, Style::new(s.panel_frame.fg, s.panel.bg));
                    sc.list.draw(c, Rect::new(inner.x, inner.y + 2, inner.w, (inner.h - 2).max(0)), theme, f && !sc.in_field);
                }
                Some(t) => {
                    let f = view_focus(t);
                    if let Some(list) = self.view_list(t) {
                        list.draw(c, inner, theme, f);
                    }
                }
                None => {}
            }
        }
    }

    // ---- automation -----------------------------------------------------------------------

    /// The parts, tabs and views as automation nodes.
    pub(super) fn shell_nodes(&self, mut root: Node) -> Node {
        let l = self.layout();
        if let Some(p) = &self.shell.palette {
            let r = self.palette_rect();
            let mut pn = Node::new("palette", "dialog", p.title(), r).focused(true);
            pn = pn.child(Node::new("palette.input", "input", p.title(), Rect::new(r.x + 1, r.y + 1, r.w - 2, 1)).focused(true).value(p.input.text.clone()));
            pn = pn.child(p.list.node("palette.list", "Results", false));
            root = root.child(pn);
        }
        if let Some(br) = l.breadcrumbs {
            let mut n = Node::new("breadcrumbs", "toolbar", "Breadcrumbs", br);
            for (k, (r, t, _)) in self.shell.crumbs.iter().enumerate() {
                n = n.child(Node::new(format!("breadcrumbs.{k}"), "button", t.clone(), *r));
            }
            root = root.child(n);
        }
        if l.editor_tabs.is_some() {
            let mut tabs = Node::new("editor_tabs", "tablist", "Open Editors", l.editor_tabs.unwrap());
            for (id, r, x) in &self.shell.editor_tab_rects {
                let w = self.windows.iter().find(|w| w.win.id == *id);
                let name = w.map(|w| w.ed.doc.title()).unwrap_or_default();
                tabs = tabs.child(Node::new(format!("editor_tab.{id}"), "tab", name.clone(), *r).focused(self.editor_index().map(|i| self.windows[i].win.id) == Some(*id)));
                tabs = tabs.child(Node::new(format!("editor_tab.{id}.close"), "button", format!("Close {name}"), *x));
            }
            root = root.child(tabs);
        }
        for (side, name) in [(Side::Left, "Primary Side Bar"), (Side::Bottom, "Panel"), (Side::Right, "Secondary Side Bar")] {
            let Some(r) = l.part(side) else { continue };
            let key = format!("{side:?}").to_lowercase();
            let mut pn = Node::new(format!("part.{key}"), "pane", name, r).focused(self.focused_side() == Some(side));
            let titles: Vec<(Tab, String)> = self.shell.dock.part(side).tabs.iter().map(|t| (*t, self.tab_title(*t))).collect();
            let title = |t: Tab| titles.iter().find(|(x, _)| *x == t).map(|(_, s)| s.clone()).unwrap_or_default();
            for (t, tr, _) in self.shell.dock.tab_rects(side, r, &title) {
                pn = pn.child(Node::new(format!("tab.{}", t.key()), "tab", title(t), tr).focused(self.shell.dock.part(side).current() == Some(t)));
            }
            let inner = Dock::content(r);
            match self.shell.dock.part(side).current() {
                Some(Tab::Pane(id)) => {
                    if self.windows.iter().any(|w| w.win.id == id && w.special == Some("chat")) {
                        let field = Rect::new(r.x + 1, r.bottom() - 2, r.w - 2, 1);
                        let (mode, _) = self.chat_mode_label(self.theme.style.icons);
                        let pill_w = str_width(&mode) as i32 + if self.theme.style.icons { 2 } else { 0 };
                        let field = Rect::new(r.x + 2 + pill_w, field.y, (r.right() - 1 - (r.x + 2 + pill_w)).max(4), 1);
                        pn = pn.child(
                            Node::new("chat.mode", "button", if self.shell.chat_agent { "Mode: Agent" } else { "Mode: Ask" }, Rect::new(r.x + 1, r.bottom() - 2, pill_w, 1)),
                        );
                        pn = pn.child(
                            Node::new("chat.input", "input", "Ask AI", field).focused(self.shell.focus == Focus::Chat).value(self.shell.chat_input.text.clone()),
                        );
                    }
                }
                Some(t @ (Tab::Search | Tab::Scm)) => {
                    let (id, label, text, in_field) = match t {
                        Tab::Search => ("search.query", "Search", &self.shell.search.query.text, self.shell.search.in_field),
                        _ => ("scm.message", "Commit message", &self.shell.scm.message.text, self.shell.scm.in_field),
                    };
                    let focus = self.shell.focus == Focus::View(t);
                    pn = pn.child(Node::new(id, "input", label, Rect::new(inner.x, inner.y, inner.w, 1)).focused(focus && in_field).value(text.clone()));
                    let list = if t == Tab::Search { &self.shell.search.list } else { &self.shell.scm.list };
                    pn = pn.child(list.node(&format!("view.{}", t.key()), t.name(), focus && !in_field));
                }
                Some(t) => {
                    let list = match t {
                        Tab::Explorer => &self.shell.explorer.list,
                        Tab::Outline => &self.shell.outline.list,
                        Tab::Problems => &self.shell.problems,
                        Tab::Extensions => &self.shell.extensions.list,
                        _ => &self.shell.debug_view,
                    };
                    pn = pn.child(list.node(&format!("view.{}", t.key()), t.name(), self.shell.focus == Focus::View(t)));
                }
                None => {}
            }
            root = root.child(pn);
        }
        root
    }
}

/// A box character's arms (up, right, down, left) and weight (1 single,
/// 2 double); None for anything else.
fn arms(c: &str) -> Option<([bool; 4], u8)> {
    Some(match c {
        "─" => ([false, true, false, true], 1),
        "│" => ([true, false, true, false], 1),
        "┌" => ([false, true, true, false], 1),
        "┐" => ([false, false, true, true], 1),
        "└" => ([true, true, false, false], 1),
        "┘" => ([true, false, false, true], 1),
        "├" => ([true, true, true, false], 1),
        "┤" => ([true, false, true, true], 1),
        "┬" => ([false, true, true, true], 1),
        "┴" => ([true, true, false, true], 1),
        "┼" => ([true, true, true, true], 1),
        "═" => ([false, true, false, true], 2),
        "║" => ([true, false, true, false], 2),
        "╔" => ([false, true, true, false], 2),
        "╗" => ([false, false, true, true], 2),
        "╚" => ([true, true, false, false], 2),
        "╝" => ([true, false, false, true], 2),
        "╠" => ([true, true, true, false], 2),
        "╣" => ([true, false, true, true], 2),
        "╦" => ([false, true, true, true], 2),
        "╩" => ([true, true, false, true], 2),
        "╬" => ([true, true, true, true], 2),
        _ => return None,
    })
}

fn box_char(a: [bool; 4], w: u8) -> Option<&'static str> {
    let single = ["─", "│", "┌", "┐", "└", "┘", "├", "┤", "┬", "┴", "┼"];
    let double = ["═", "║", "╔", "╗", "╚", "╝", "╠", "╣", "╦", "╩", "╬"];
    let set = if w == 2 { double } else { single };
    set.into_iter().find(|c| arms(c).is_some_and(|(b, _)| b == a))
}

/// Where frames meet along the side bars' and panel's edges, their lines
/// join (┐ beside ─ becomes ┬; │ beside ─ becomes ├), so a part and the
/// editor window share one border.
pub(super) fn join_borders(surf: &mut turbine_ui::Surface, parts: &[Rect]) {
    let at = |surf: &turbine_ui::Surface, x: i32, y: i32| surf.get(x, y).and_then(|c| arms(&c.text));
    let mut changes = Vec::new();
    for r in parts {
        let mut cells = Vec::new();
        for x in r.x..r.right() {
            cells.push((x, r.y));
            cells.push((x, r.bottom() - 1));
        }
        for y in r.y..r.bottom() {
            cells.push((r.x, y));
            cells.push((r.right() - 1, y));
        }
        for (x, y) in cells {
            let Some((mut a, w)) = at(surf, x, y) else { continue };
            let before = a;
            // A neighbour of the same weight reaching toward this cell.
            let near = [(0, -1, 2usize, 0usize), (1, 0, 3, 1), (0, 1, 0, 2), (-1, 0, 1, 3)];
            for (dx, dy, back, mine) in near {
                if let Some((n, nw)) = at(surf, x + dx, y + dy) {
                    if nw == w && n[back] {
                        a[mine] = true;
                    }
                }
            }
            if a != before {
                if let Some(c) = box_char(a, w) {
                    changes.push((x, y, c));
                }
            }
        }
    }
    for (x, y, c) in changes {
        if let Some(cell) = surf.get_mut(x, y) {
            cell.text = c.into();
        }
    }
}

