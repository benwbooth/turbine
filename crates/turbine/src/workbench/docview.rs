//! A toolbar under an editor's tabs for files that read two ways, as
//! VS Code and grove offer: Markdown as Source or Preview (rendered in
//! the window itself), JSON, YAML, TOML and XML as Source or a Tree
//! (folding, with a search), and Word Wrap, as DOS radio buttons and a
//! check box: (•) Source  ( ) Preview  [X] Word Wrap.

use super::*;
use crate::structview::{self, StructView};

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
        buttons(w, self.doc_mode(w.win.id), self.docviews.get(&w.win.id).is_none_or(|d| d.preview_wrap))
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
            DocView { mode: Mode::Source, alt, tree: None, tree_version: u64::MAX, preview_wrap: true }
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
fn buttons(w: &EdWin, mode: Mode, preview_wrap: bool) -> Vec<(Rect, &'static str, String, &'static str)> {
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
            Mode::Preview => items.push(("wrap", format!("[{}] Word Wrap", if preview_wrap { "X" } else { " " }), "Wrap the page at the window's edge, or let lines run on (Alt+Z)")),
            Mode::Tree => {}
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
    let bs = Style { fg: s.editor_gutter.fg, ..s.editor };
    c.fill(bar, " ", bs);
    let mode = dv.as_ref().map_or(Mode::Source, |d| d.mode);
    let pw = dv.as_ref().is_none_or(|d| d.preview_wrap);
    for (r, _, label, _) in buttons(w, mode, pw) {
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
            dv.alt.draw(c, body, theme, focused, false);
            true
        }
    }
}
