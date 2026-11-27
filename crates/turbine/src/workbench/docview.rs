//! A toolbar under an editor's tabs for files that read two ways, as
//! VS Code and grove offer: Markdown as Source or Preview (rendered in
//! the window itself), JSON, YAML, TOML and XML as Source or a Tree
//! (folding, with a search), and Word Wrap, as DOS radio buttons and a
//! check box: (•) Source  ( ) Preview  [X] Word Wrap.

use super::*;
use crate::structview::{self, StructView};
use turbine_ui::popup::{ListPopup, PopupResult};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Mode {
    Source,
    Preview,
    Tree,
}

pub(super) struct DocView {
    pub mode: Mode,
    /// The rendered page (Preview).
    pub alt: EditorView,
    pub tree: Option<StructView>,
    /// The document version the tree was read from.
    tree_version: u64,
    /// The preview wraps at the window's width (else lines run on, and it
    /// scrolls sideways).
    pub preview_wrap: bool,
    /// The preview on white (else the guide's black).
    pub light: bool,
    /// The tree's right-click menu: its list, where, and each item's action.
    pub menu: Option<(ListPopup, Point, Vec<&'static str>)>,
}

/// In-place previews go by an id of their own (not a window's).
pub(super) const PREVIEW_ID: u32 = 0x4000_0000;

impl Workbench {
    /// What a window's toolbar offers: "markdown", a tree format, or none.
    pub(super) fn doc_kind(w: &EdWin) -> Option<&'static str> {
        if w.term.is_some() || w.special.is_some() || w.dock.is_some() {
            return None;
        }
        match w.lang.as_deref()? {
            "markdown" => Some("markdown"),
            l => structview::format_of(l),
        }
    }

    pub(super) fn doc_mode(&self, id: u32) -> Mode {
        self.docviews.get(&id).map_or(Mode::Source, |d| d.mode)
    }

    /// The toolbar's controls in a window: (place, action, label, on, tip).
    pub(super) fn doc_buttons(&self, w: &EdWin) -> Vec<(Rect, &'static str, String, &'static str)> {
        let dv = self.docviews.get(&w.win.id);
        let all_open = dv.and_then(|d| d.tree.as_ref()).is_some_and(|t| t.all_open);
        buttons(w, self.doc_mode(w.win.id), dv.is_none_or(|d| d.preview_wrap), dv.is_some_and(|d| d.light), all_open)
    }

    /// Where a window's text goes: under the toolbar when it has one.
    pub(super) fn doc_body(w: &EdWin) -> Rect {
        let inner = w.win.inner();
        if Self::doc_kind(w).is_some() && inner.h > 3 { Rect::new(inner.x, inner.y + 1, inner.w, inner.h - 1) } else { inner }
    }

    pub(super) fn doc_action(&mut self, id: u32, action: &str) {
        let Some(i) = self.windows.iter().position(|w| w.win.id == id) else { return };
        match action {
            "wrap" => {
                // (The preview's own wrap in Preview; the text's in Source.)
                if self.doc_mode(id) == Mode::Preview {
                    if let Some(dv) = self.docviews.get_mut(&id) {
                        dv.preview_wrap = !dv.preview_wrap;
                        dv.alt.wrap = false;
                        dv.alt.scroll_x = 0;
                    }
                    if let Some(p) = self.preview.previews.get_mut(&(PREVIEW_ID | id)) {
                        p.rerender();
                    }
                    return;
                }
                let w = &mut self.windows[i];
                w.ed.wrap = !w.ed.wrap;
                return;
            }
            "light" => {
                if let Some(dv) = self.docviews.get_mut(&id) {
                    dv.light = !dv.light;
                }
            }
            "expand" | "collapse" => {
                if let Some(t) = self.docviews.get_mut(&id).and_then(|d| d.tree.as_mut()) {
                    t.expand_all(action == "expand");
                }
            }
            "source" => self.set_doc_mode(id, Mode::Source),
            "preview" => self.set_doc_mode(id, Mode::Preview),
            "tree" => self.set_doc_mode(id, Mode::Tree),
            _ => {}
        }
    }

    pub(super) fn set_doc_mode(&mut self, id: u32, mode: Mode) {
        let dv = self.docviews.entry(id).or_insert_with(|| {
            let mut alt = EditorView::new(Document::new());
            alt.line_numbers = false;
            // (The page is laid out at the window's width already.)
            alt.wrap = false;
            // (Drawn as the guide is: black, its headings, links, code and
            // keys in their colours.)
            alt.help_look = true;
            DocView { mode: Mode::Source, alt, tree: None, tree_version: u64::MAX, preview_wrap: true, light: false, menu: None }
        });
        dv.mode = mode;
        let pid = PREVIEW_ID | id;
        if mode == Mode::Preview {
            let base = self.windows.iter().find(|w| w.win.id == id).and_then(|w| w.ed.doc.path.as_ref()).and_then(|p| p.parent().map(Path::to_path_buf));
            self.preview.previews.entry(pid).or_insert_with(|| preview::Preview::in_place(id, base));
        } else {
            self.preview.previews.remove(&pid);
        }
    }

    /// Each tick: trees follow their file's edits.
    pub(super) fn docview_tick(&mut self) {
        let ids: HashSet<u32> = self.windows.iter().map(|w| w.win.id).collect();
        self.docviews.retain(|id, _| ids.contains(id));
        for w in &self.windows {
            let Some(dv) = self.docviews.get_mut(&w.win.id) else { continue };
            if dv.mode != Mode::Tree || dv.tree_version == w.ed.doc.version {
                continue;
            }
            let Some(fmt) = w.lang.as_deref().and_then(structview::format_of) else { continue };
            dv.tree_version = w.ed.doc.version;
            let root = structview::parse(fmt, &w.ed.doc.rope.to_string());
            match &mut dv.tree {
                Some(t) => t.replace(root),
                None => dv.tree = Some(StructView::new(root)),
            }
        }
    }

    /// Keys for the front window's toolbar views. True when used.
    pub(super) fn docview_key(&mut self, k: &KeyEvent) -> bool {
        let Some(id) = self.windows.last().filter(|w| Self::doc_kind(w).is_some()).map(|w| w.win.id) else { return false };
        // Alt+Z: word wrap, as VS Code's.
        if k.is(Key::Char('z'), Mods::ALT) && self.doc_mode(id) != Mode::Tree {
            self.doc_action(id, "wrap");
            return true;
        }
        // The tree's menu has the keys while it's open.
        if let Some((popup, _, actions)) = self.docviews.get_mut(&id).and_then(|d| d.menu.as_mut()) {
            let actions = actions.clone();
            match popup.handle_key(k) {
                PopupResult::Accept(i) => {
                    self.docviews.get_mut(&id).unwrap().menu = None;
                    self.tree_action(id, actions[i]);
                }
                PopupResult::Close => self.docviews.get_mut(&id).unwrap().menu = None,
                _ => {}
            }
            return true;
        }
        if self.doc_mode(id) == Mode::Tree {
            if k.is(Key::F(2), Mods::NONE) {
                self.tree_action(id, "edit");
                return true;
            }
            if k.is(Key::Delete, Mods::NONE) {
                self.tree_action(id, "delete");
                return true;
            }
            if k.is(Key::Insert, Mods::NONE) {
                let branch = self.docviews.get(&id).and_then(|d| d.tree.as_ref()).and_then(|t| t.selected_node()).is_some_and(|n| matches!(n.kind, structview::Kind::Object | structview::Kind::Array));
                self.tree_action(id, if branch { "add_child" } else { "add_after" });
                return true;
            }
            if k.is(Key::F(10), Mods::SHIFT) {
                let at = self.docviews.get(&id).and_then(|d| d.tree.as_ref()).map(|t| Point::new(t.area.x + 4, t.area.y + 1 + (t.selected.saturating_sub(t.scroll)) as i32)).unwrap_or_default();
                self.open_tree_menu(id, at);
                return true;
            }
        }
        let Some(dv) = self.docviews.get_mut(&id) else { return false };
        match dv.mode {
            Mode::Source => false,
            Mode::Tree => {
                let Some(t) = &mut dv.tree else { return true };
                match t.key(k) {
                    structview::Out::Ignored => {
                        // (Esc back to the source.)
                        if k.key == Key::Esc {
                            self.set_doc_mode(id, Mode::Source);
                            return true;
                        }
                        false
                    }
                    structview::Out::Used => true,
                    structview::Out::Line(l) => {
                        self.set_doc_mode(id, Mode::Source);
                        self.goto_line_in(id, l);
                        true
                    }
                }
            }
            Mode::Preview => {
                // Reading keys only: the page isn't for typing in.
                let nav = matches!(k.key, Key::Up | Key::Down | Key::Left | Key::Right | Key::PageUp | Key::PageDown | Key::Home | Key::End) || k.is_ctrl('c') || k.is_ctrl('a');
                if k.key == Key::Esc {
                    self.set_doc_mode(id, Mode::Source);
                    return true;
                }
                if nav {
                    let mut clip = String::new();
                    dv.alt.handle_key(k, &mut clip);
                    if !clip.is_empty() {
                        self.copy(clip);
                    }
                }
                // (Typing goes nowhere: Source is for that.)
                nav || (!k.mods.alt && !k.mods.ctrl && matches!(k.key, Key::Char(_) | Key::Enter | Key::Backspace | Key::Delete | Key::Tab))
            }
        }
    }

    fn open_tree_menu(&mut self, id: u32, at: Point) {
        let Some(t) = self.docviews.get(&id).and_then(|d| d.tree.as_ref()) else { return };
        let Some(n) = t.selected_node() else { return };
        let mut items: Vec<(&str, &str, &'static str)> = Vec::new();
        let json = self.windows.iter().find(|w| w.win.id == id).and_then(|w| w.lang.as_deref()).and_then(structview::format_of) == Some("json");
        if n.span.is_some() && n.value.is_some() {
            items.push(("Edit Value...", "F2", "edit"));
        }
        if json {
            if n.key_span.is_some() {
                items.push(("Rename Key...", "", "rename_key"));
            }
            if matches!(n.kind, structview::Kind::Object | structview::Kind::Array) {
                items.push(("Add Inside...", "Ins", "add_child"));
            }
            items.push(("Add After...", "", "add_after"));
            items.push(("Delete", "Del", "delete"));
        }
        if n.value.is_some() {
            items.push(("Copy Value", "", "copy_value"));
        }
        if n.key.is_some() {
            items.push(("Copy Key", "", "copy_key"));
        }
        items.push(("Copy Path", "", "copy_path"));
        if n.line.is_some() {
            items.push(("Go to Source", "Ctrl+Enter", "source"));
        }
        if n.is_branch() {
            items.push(("Expand Branch", "*", "expand_branch"));
        }
        items.push(("Expand All", "", "expand"));
        items.push(("Collapse All", "-", "collapse"));
        let mut popup = ListPopup::new(items.iter().map(|(l, k, _)| (l.to_string(), k.to_string())).collect());
        popup.max_rows = 12;
        popup.place(at, self.screen());
        if let Some(dv) = self.docviews.get_mut(&id) {
            dv.menu = Some((popup, at, items.iter().map(|i| i.2).collect()));
        }
    }

    /// A tree menu's action on the selected row.
    pub(super) fn tree_action(&mut self, id: u32, action: &str) {
        let Some(t) = self.docviews.get(&id).and_then(|d| d.tree.as_ref()) else { return };
        let Some(n) = t.selected_node().cloned() else { return };
        let path = t.selected_path();
        match action {
            "copy_value" => self.copy(n.value.unwrap_or_default()),
            "copy_key" => self.copy(n.key.unwrap_or_default()),
            "copy_path" => self.copy(path),
            "source" => {
                if let Some(l) = n.line {
                    self.set_doc_mode(id, Mode::Source);
                    self.goto_line_in(id, l);
                }
            }
            "expand_branch" => {
                if let Some(t) = self.docviews.get_mut(&id).and_then(|d| d.tree.as_mut()) {
                    t.key(&KeyEvent::plain(Key::Char('*')));
                }
            }
            "expand" | "collapse" => self.doc_action(id, action),
            "delete" | "rename_key" | "add_child" | "add_after" => self.tree_struct_action(id, action),
            "edit" => {
                let Some((a, b)) = n.span else {
                    self.say("This value can be changed in Source (the format doesn't say where it is)");
                    return;
                };
                let Some(w) = self.windows.iter().find(|w| w.win.id == id) else { return };
                let raw = w.ed.doc.rope.to_string().get(a..b).unwrap_or("").to_string();
                let d = Dialog::new(&format!("tree_edit:{id}:{a}:{b}"), "Edit Value")
                    .body(ui::input("value", &format!("{} ~=~ ", if path.len() > 30 { &path[path.len() - 30..] } else { &path }), &raw).chars(50))
                    .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            _ => {}
        }
    }

    /// JSON's structural edits: Delete (the entry and its comma), Rename Key,
    /// Add Inside (a member or an item at the end) and Add After.
    fn tree_struct_action(&mut self, id: u32, action: &str) {
        let Some(t) = self.docviews.get(&id).and_then(|d| d.tree.as_ref()) else { return };
        let (Some(n), Some(path)) = (t.selected_node().cloned(), t.selected_index_path()) else { return };
        // The container the selected entry is in (for Add After: what kind).
        let parent = t.node(&path[..path.len().saturating_sub(1)]).cloned();
        let Some(w) = self.windows.iter().find(|w| w.win.id == id) else { return };
        let text = w.ed.doc.rope.to_string();
        let Some((vs, ve)) = n.span else {
            self.say("Not something the tree can change (its place in the text isn't known)");
            return;
        };
        let start = n.key_span.map_or(vs, |k| k.0);
        match action {
            "delete" => {
                if path.is_empty() {
                    return;
                }
                let (a, b) = json_entry_range(&text, start, ve);
                self.tree_replace(id, a, b, "");
            }
            "rename_key" => {
                let Some((ka, kb)) = n.key_span else { return };
                let key = n.key.clone().unwrap_or_default();
                let d = Dialog::new(&format!("tree_key:{id}:{ka}:{kb}"), "Rename Key").body(ui::input("key", "~K~ey:", &key).chars(40)).buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "add_child" | "add_after" => {
                let into_object = if action == "add_child" { n.kind == structview::Kind::Object } else { parent.as_ref().is_some_and(|p| p.kind == structview::Kind::Object) };
                if action == "add_after" && path.is_empty() {
                    return;
                }
                let kind = if action == "add_child" { "in" } else { "after" };
                let mut items = Vec::new();
                if into_object {
                    items.push(ui::input("key", "~K~ey:", "").chars(40));
                }
                items.push(ui::input("value", "~V~alue (JSON):", "null").chars(40));
                let d = Dialog::new(&format!("tree_add:{id}:{kind}:{}:{}:{}", start, ve, if into_object { 1 } else { 0 }), if action == "add_child" { "Add Inside" } else { "Add After" })
                    .body(ui::col(items))
                    .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            _ => {}
        }
    }

    /// Replace bytes a..b of a window's text (one undoable edit).
    fn tree_replace(&mut self, id: u32, a: usize, b: usize, new: &str) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.win.id == id) {
            let r = &w.ed.doc.rope;
            if b <= r.len_bytes() && a <= b {
                let (ca, cb) = (r.byte_to_char(a), r.byte_to_char(b));
                let cur = w.ed.cursor;
                w.ed.doc.replace(ca, cb, new, cur);
            }
        }
    }

    /// The Rename Key and Add dialogs' answers. True when it was one of them.
    pub(super) fn tree_struct_done(&mut self, kind: &str, d: &Dialog) -> bool {
        if let Some(rest) = kind.strip_prefix("tree_key:") {
            let mut it = rest.split(':').filter_map(|x| x.parse::<usize>().ok());
            if let (Some(id), Some(a), Some(b)) = (it.next(), it.next(), it.next()) {
                let key = d.value("key").unwrap_or_default();
                self.tree_replace(id as u32, a, b, &serde_json::to_string(&key).unwrap_or_default());
            }
            return true;
        }
        let Some(rest) = kind.strip_prefix("tree_add:") else { return false };
        let parts: Vec<&str> = rest.split(':').collect();
        let (Some(id), Some(place), Some(start), Some(end), Some(obj)) = (parts.first().and_then(|x| x.parse::<u32>().ok()), parts.get(1), parts.get(2).and_then(|x| x.parse::<usize>().ok()), parts.get(3).and_then(|x| x.parse::<usize>().ok()), parts.get(4)) else { return true };
        let value = d.value("value").unwrap_or_default();
        let value = if value.trim().is_empty() { "null".to_string() } else { value.trim().to_string() };
        // (A value that isn't JSON goes in as a string.)
        let value = if serde_json::from_str::<serde_json::Value>(&value).is_ok() { value } else { serde_json::to_string(&value).unwrap_or_default() };
        let entry = if *obj == "1" { format!("{}: {value}", serde_json::to_string(&d.value("key").unwrap_or_default()).unwrap_or_default()) } else { value };
        let Some(w) = self.windows.iter().find(|w| w.win.id == id) else { return true };
        let text = w.ed.doc.rope.to_string();
        let (at, insert) = if *place == "after" { json_insert_after(&text, start, end, &entry) } else { json_insert_inside(&text, start, end, &entry) };
        self.tree_replace(id, at, at, &insert);
        true
    }

    /// The Edit Value dialog's answer: the value replaced in the text (one
    /// undoable edit). True when it was this dialog.
    pub(super) fn tree_edit_done(&mut self, kind: &str, d: &Dialog) -> bool {
        let Some(rest) = kind.strip_prefix("tree_edit:") else { return false };
        let mut it = rest.split(':').filter_map(|x| x.parse::<usize>().ok());
        let (Some(id), Some(a), Some(b)) = (it.next(), it.next(), it.next()) else { return true };
        let new = d.value("value").unwrap_or_default();
        if let Some(w) = self.windows.iter_mut().find(|w| w.win.id == id as u32) {
            let r = &w.ed.doc.rope;
            if b <= r.len_bytes() {
                let (ca, cb) = (r.byte_to_char(a), r.byte_to_char(b));
                let cur = w.ed.cursor;
                w.ed.doc.replace(ca, cb, &new, cur);
            }
        }
        true
    }

    /// Draw the tree's menu (over everything else in the window).
    pub(super) fn draw_docview_menu(&mut self, c: &mut Canvas, theme: &Theme) {
        let screen = self.screen();
        let Some(id) = self.windows.last().map(|w| w.win.id) else { return };
        if let Some((popup, at, _)) = self.docviews.get_mut(&id).and_then(|d| d.menu.as_mut()) {
            let at = *at;
            popup.draw(c, at, screen, theme);
        }
    }

    /// Scroll a window's Preview to a heading (from the Outline).
    pub(super) fn preview_to_heading(&mut self, id: u32, heading: &str) {
        let Some(line) = self.preview.previews.get(&(PREVIEW_ID | id)).and_then(|p| p.rendered.as_ref()).and_then(|r| r.headings.iter().find(|(_, _, t)| t.trim() == heading).map(|h| h.0)) else { return };
        if let Some(dv) = self.docviews.get_mut(&id) {
            let l = line.min(dv.alt.doc.len_lines().saturating_sub(1));
            dv.alt.cursor = dv.alt.doc.rope.line_to_char(l);
            dv.alt.scroll_y = l;
        }
    }

    fn goto_line_in(&mut self, id: u32, line: usize) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.win.id == id) {
            let l = line.min(w.ed.doc.len_lines().saturating_sub(1));
            w.ed.cursor = w.ed.doc.rope.line_to_char(l);
            w.ed.anchor = None;
            w.ed.scroll_y = l.saturating_sub(3);
        }
    }

    /// The mouse on a window with a toolbar: its buttons, and the views.
    /// True when used.
    pub(super) fn docview_mouse(&mut self, m: &MouseEvent) -> bool {
        // The tree's menu first.
        if let Some(id) = self.windows.last().map(|w| w.win.id) {
            if let Some((popup, _, actions)) = self.docviews.get_mut(&id).and_then(|d| d.menu.as_mut()) {
                let actions = actions.clone();
                match popup.handle_mouse(m) {
                    PopupResult::Accept(i) => {
                        self.docviews.get_mut(&id).unwrap().menu = None;
                        self.tree_action(id, actions[i]);
                    }
                    PopupResult::Close => self.docviews.get_mut(&id).unwrap().menu = None,
                    PopupResult::Ignored if matches!(m.kind, MouseKind::Down(_)) => self.docviews.get_mut(&id).unwrap().menu = None,
                    _ => {}
                }
                return true;
            }
        }
        let Some(w) = self.windows.last().filter(|w| Self::doc_kind(w).is_some() && w.win.inner().contains(m.pos)) else { return false };
        let id = w.win.id;
        if m.pos.y == w.win.inner().y {
            if let MouseKind::Down(Button::Left) = m.kind {
                if let Some(a) = self.doc_buttons(w).into_iter().find(|b| b.0.contains(m.pos)).map(|b| b.1) {
                    self.doc_action(id, a);
                }
            }
            return true;
        }
        let Some(dv) = self.docviews.get_mut(&id) else { return false };
        match dv.mode {
            Mode::Source => false,
            Mode::Tree => {
                if matches!(m.kind, MouseKind::Down(Button::Right)) {
                    if dv.tree.as_mut().is_some_and(|t| t.select_at(m.pos)) {
                        self.open_tree_menu(id, m.pos);
                    }
                    return true;
                }
                if let Some(t) = &mut dv.tree {
                    if let structview::Out::Line(l) = t.mouse(m) {
                        self.set_doc_mode(id, Mode::Source);
                        self.goto_line_in(id, l);
                    }
                }
                true
            }
            Mode::Preview => {
                dv.alt.handle_mouse(m);
                true
            }
        }
    }

    /// Tooltips and agents: the toolbar's controls.
    pub(super) fn docview_nodes(&self, mut root: Node) -> Node {
        if let Some(w) = self.windows.last() {
            for (r, a, label, tip) in self.doc_buttons(w) {
                root = root.child(Node::new(format!("docview.{a}"), "button", label, r).tip(tip));
            }
            if let (Some(dv), true) = (self.docviews.get(&w.win.id), self.doc_mode(w.win.id) == Mode::Tree) {
                if let Some(t) = &dv.tree {
                    let a = t.area;
                    root = root.child(Node::new("docview.treeview", "tree", "Tree", a).value(format!("{} rows, {} found", t.rows(), t.matches)));
                }
            }
        }
        root
    }

    /// A cut row's whole text in the tree, for its tooltip.
    pub(super) fn docview_tip(&self, p: Point) -> Option<(Rect, String)> {
        let w = self.windows.last()?;
        let t = self.docviews.get(&w.win.id)?.tree.as_ref().filter(|_| self.doc_mode(w.win.id) == Mode::Tree)?;
        t.row_text_at(p).map(|s| (Rect::new(t.area.x, p.y, t.area.w, 1), s))
    }
}

/// The toolbar's controls: (place, action, label, tooltip).
fn buttons(w: &EdWin, mode: Mode, preview_wrap: bool, light: bool, all_open: bool) -> Vec<(Rect, &'static str, String, &'static str)> {
    let Some(kind) = Workbench::doc_kind(w) else { return Vec::new() };
    let inner = w.win.inner();
        let other = if kind == "markdown" { ("preview", "Preview", Mode::Preview, "Show it rendered (Ctrl+Shift+V beside it)") } else { ("tree", "Tree", Mode::Tree, "Show it as a tree to fold and search") };
        let radio = |on: bool| if on { "(•)" } else { "( )" };
        let mut items: Vec<(&'static str, String, &'static str)> = vec![
            ("source", format!("{} Source", radio(mode == Mode::Source)), "Show the text to edit"),
            (other.0, format!("{} {}", radio(mode == other.2), other.1), other.3),
        ];
        match mode {
            Mode::Source => items.push(("wrap", format!("[{}] Word Wrap", if w.ed.wrap { "X" } else { " " }), "Wrap long lines at the window's edge (Alt+Z)")),
            Mode::Preview => {
                items.push(("wrap", format!("[{}] Word Wrap", if preview_wrap { "X" } else { " " }), "Wrap the page at the window's edge, or let lines run on (Alt+Z)"));
                items.push(("light", format!("[{}] White Page", if light { "X" } else { " " }), "Black text on white, or the guide's black page"));
            }
            // (One toggle: Expand All, then Collapse All.)
            Mode::Tree if all_open => items.push(("collapse", "[-] Collapse All".into(), "Close every branch (-)")),
            Mode::Tree => items.push(("expand", "[+] Expand All".into(), "Open every branch (*)")),
        }
        let mut x = inner.x + 1;
        let mut out = Vec::new();
        for (i, (a, label, tip)) in items.into_iter().enumerate() {
            if i == 2 {
                x += 2;
            }
            let wdt = str_width(&label) as i32;
            if x + wdt > inner.right() {
                break;
            }
            out.push((Rect::new(x, inner.y, wdt, 1), a, label, tip));
            x += wdt + 2;
        }
        out
    }

/// Draw a window's toolbar, and its view when it isn't the source. False
/// when the source editor draws (into `doc_body`).
pub(super) fn draw(w: &mut EdWin, dv: Option<&mut DocView>, c: &mut Canvas, theme: &Theme, focused: bool) -> bool {
    if Workbench::doc_kind(w).is_none() || w.win.inner().h <= 3 {
        return false;
    }
    let inner = w.win.inner();
    let s = &theme.scheme;
    let bar = Rect::new(inner.x, inner.y, inner.w, 1);
    let bs = Style { fg: turbine_ui::theme::vga::CYAN, ..s.editor };
    c.fill(bar, " ", bs);
    let mode = dv.as_ref().map_or(Mode::Source, |d| d.mode);
    let pw = dv.as_ref().is_none_or(|d| d.preview_wrap);
    let light = dv.as_ref().is_some_and(|d| d.light);
    let all_open = dv.as_ref().and_then(|d| d.tree.as_ref()).is_some_and(|t| t.all_open);
    for (r, _, label, _) in buttons(w, mode, pw, light, all_open) {
        let on = label.starts_with("(•)") || label.starts_with("[X]");
        let st = if on { Style { fg: s.frame_active.fg, ..bs } } else { bs };
        c.print(r.x, r.y, &label, st);
    }
    let body = Workbench::doc_body(w);
    let Some(dv) = dv else { return false };
    match dv.mode {
        Mode::Source => false,
        Mode::Tree => {
            match &mut dv.tree {
                Some(t) => t.draw(c, body, theme, focused),
                None => {
                    c.fill(body, " ", s.editor);
                    c.print(body.x + 1, body.y, "Reading…", s.editor);
                }
            }
            true
        }
        Mode::Preview => {
            if dv.light {
                // (Black on white, the page's marks in dark colours.)
                use turbine_ui::theme::vga::*;
                let mut t = theme.clone();
                let sc = &mut t.scheme;
                sc.help = Style::new(BLACK, WHITE);
                sc.help_heading = Style::new(BLUE, WHITE);
                sc.help_link = Style::new(MAGENTA, WHITE);
                sc.help_mark = Style::new(RED, WHITE);
                sc.help_code = Style::new(GREEN, WHITE);
                sc.help_key = Style::new(WHITE, BLUE);
                dv.alt.draw(c, body, &t, focused, false);
            } else {
                dv.alt.draw(c, body, theme, focused, false);
            }
            // (Its scroll bars on the window's edges.)
            dv.alt.draw_scrollbars(c, w.win.rect, theme);
            true
        }
    }
}

/// An entry's bytes to delete: the entry (key and value) and one comma
/// (after it, else before it), whole lines when it's alone on its line.
fn json_entry_range(text: &str, start: usize, end: usize) -> (usize, usize) {
    let b = text.as_bytes();
    let mut j = end;
    while j < b.len() && matches!(b[j], b' ' | b'\t') {
        j += 1;
    }
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let alone_before = text[line_start..start].trim().is_empty();
    if j < b.len() && b[j] == b',' {
        let mut k = j + 1;
        while k < b.len() && matches!(b[k], b' ' | b'\t') {
            k += 1;
        }
        if alone_before && k < b.len() && b[k] == b'\n' {
            return (line_start, k + 1);
        }
        return (start, k);
    }
    // The last one: the comma before it goes.
    let mut i = start;
    while i > 0 && b[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    if i > 0 && b[i - 1] == b',' {
        return (i - 1, end);
    }
    (start, end)
}

/// The indentation of the line a byte is on.
fn indent_at(text: &str, at: usize) -> String {
    let ls = text[..at].rfind('\n').map_or(0, |i| i + 1);
    text[ls..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// Where and what to insert for a new entry after the one at start..end.
fn json_insert_after(text: &str, start: usize, end: usize, entry: &str) -> (usize, String) {
    let one_line = !text[start..end].contains('\n') && text[..start].rfind('\n').is_some_and(|i| text[i + 1..start].trim().is_empty());
    if one_line {
        (end, format!(",\n{}{entry}", indent_at(text, start)))
    } else {
        (end, format!(", {entry}"))
    }
}

/// Where and what to insert for a new last entry inside the container at
/// start..end ({...} or [...]).
fn json_insert_inside(text: &str, start: usize, end: usize, entry: &str) -> (usize, String) {
    let close = end - 1;
    let inner = &text[start + 1..close];
    let outer_indent = indent_at(text, start);
    if inner.trim().is_empty() {
        // (Empty: on a line of its own, indented one step more.)
        return (start + 1, format!("\n{outer_indent}  {entry}\n{outer_indent}"));
    }
    // After the last thing in it (skipping trailing space and a trailing comma).
    let b = text.as_bytes();
    let mut i = close;
    while i > start + 1 && b[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    let had_comma = i > start + 1 && b[i - 1] == b',';
    let multi = inner.contains('\n');
    let child_indent = if multi {
        inner.lines().skip(1).find(|l| !l.trim().is_empty()).map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').collect::<String>()).unwrap_or(format!("{outer_indent}  "))
    } else {
        String::new()
    };
    let sep = if had_comma { "" } else { "," };
    if multi { (i, format!("{sep}\n{child_indent}{entry}")) } else { (i, format!("{sep} {entry}")) }
}

#[cfg(test)]
mod json_edit_tests {
    use super::*;

    #[test]
    fn json_edits_keep_commas_and_indentation() {
        let t = "{\n  \"a\": 1,\n  \"b\": 2\n}\n";
        // Delete "a": its line goes with its comma.
        let (a, b) = json_entry_range(t, 4, 10);
        assert_eq!(format!("{}{}", &t[..a], &t[b..]), "{\n  \"b\": 2\n}\n");
        // Delete "b" (the last): the comma before it goes.
        let s = t.find("\"b\"").unwrap();
        let (a, b) = json_entry_range(t, s, s + 6);
        assert_eq!(format!("{}{}", &t[..a], &t[b..]), "{\n  \"a\": 1\n}\n");
        // Add inside: a new last member, indented as the others.
        let (at, ins) = json_insert_inside(t, 0, t.len() - 1, "\"c\": 3");
        assert_eq!(format!("{}{ins}{}", &t[..at], &t[at..]), "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}\n");
        // Add after "a".
        let (at, ins) = json_insert_after(t, 4, 10, "\"x\": 0");
        assert_eq!(format!("{}{ins}{}", &t[..at], &t[at..]), "{\n  \"a\": 1,\n  \"x\": 0,\n  \"b\": 2\n}\n");
        // Into an empty array; and a one-line one.
        let e = "[]";
        let (at, ins) = json_insert_inside(e, 0, 2, "1");
        assert_eq!(format!("{}{ins}{}", &e[..at], &e[at..]), "[\n  1\n]");
        let o = "[1, 2]";
        let (at, ins) = json_insert_inside(o, 0, 6, "3");
        assert_eq!(format!("{}{ins}{}", &o[..at], &o[at..]), "[1, 2, 3]");
    }
}
