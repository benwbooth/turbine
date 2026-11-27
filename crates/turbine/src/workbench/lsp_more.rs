//! More of what language servers do, as Cursor shows it: Format Document,
//! quick fixes and refactorings (Ctrl+.), signature help while typing a
//! call, inlay hints, semantic highlighting, symbols across the project
//! (`#` in quick open), and offering to install a missing server.

use super::*;
use crate::palette;
use turbine_services::lsp::{CodeAction, Signature, WorkspaceEdit, WorkspaceSymbol};
use turbine_ui::popup::{ListPopup, PopupResult};

pub(super) struct ActionMenu {
    pub popup: ListPopup,
    pub actions: Vec<CodeAction>,
    pub key: LspKey,
    pub at: Point,
}

#[derive(Default)]
pub(super) struct LspExtra {
    /// The server's diagnostics per file, as sent (code actions' context).
    pub diags: HashMap<PathBuf, Vec<Value>>,
    pub format: Option<(LspKey, i64)>,
    pub actions_req: Option<(LspKey, i64, Point)>,
    pub resolve_req: Option<(LspKey, i64)>,
    pub menu: Option<ActionMenu>,
    pub sig_req: Option<(LspKey, i64, u32, usize)>,
    /// The signature shown: (window, its line).
    pub signature: Option<(u32, usize, Signature)>,
    /// Requests out for a window: (window, version) -> id.
    pub inlay_req: HashMap<u32, (LspKey, i64, u64)>,
    pub sem_req: HashMap<u32, (LspKey, i64, u64)>,
    /// When each window's text last changed: (version, when).
    pub changed: HashMap<u32, (u64, Instant)>,
    pub ws_req: Option<(LspKey, i64)>,
    pub ws_query: String,
    pub ws_symbols: Vec<WorkspaceSymbol>,
    /// Languages already offered a server install.
    pub offered: HashSet<String>,
    pub inlays_off: bool,
    /// When a window's hints came back empty (a server still indexing):
    /// asked again a few seconds on.
    pub inlay_empty_at: HashMap<u32, Instant>,
    pub semantic_off: bool,
}

/// Semantic token types as Turbine's highlight kinds.
fn semantic_kind(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "namespace" => "namespace",
        "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" => "type",
        "parameter" | "variable" => "variable",
        "property" => "property",
        "enumMember" => "constant",
        "function" | "method" | "macro" | "decorator" => "function",
        "keyword" | "modifier" => "keyword",
        "comment" => "comment",
        "string" | "regexp" => "string",
        "number" => "number",
        "operator" => "operator",
        _ => return None,
    })
}

/// LSP positions as a char offset in a document.
fn char_at(doc: &Document, p: Pos) -> usize {
    let line = (p.line as usize).min(doc.len_lines().saturating_sub(1));
    let text = doc.line(line);
    doc.line_byte_to_char(line, lsp::byte_of_utf16(&text, p.character))
}

/// A text with edits applied (positions in the original).
fn edited(doc: &Document, edits: &[(Pos, Pos, String)]) -> String {
    let mut spans: Vec<(usize, usize, &str)> = edits.iter().map(|(a, b, t)| (char_at(doc, *a), char_at(doc, *b), t.as_str())).collect();
    spans.sort_by_key(|s| std::cmp::Reverse((s.0, s.1)));
    let mut rope = doc.rope.clone();
    for (a, b, t) in spans {
        let (a, b) = (a.min(b), a.max(b));
        rope.remove(a..b.min(rope.len_chars()));
        rope.insert(a, t);
    }
    rope.to_string()
}

impl Workbench {
    /// Apply edits to files: open ones get them as one undo step; others
    /// open in the background with them, unsaved, to look over.
    pub(super) fn apply_workspace_edit(&mut self, edits: WorkspaceEdit) -> (usize, usize) {
        let front = self.windows.last().map(|w| w.win.id);
        let (mut files, mut n) = (0, 0);
        for (path, list) in edits {
            if list.is_empty() {
                continue;
            }
            let want = Self::canon(&path);
            let i = match self.windows.iter().position(|w| w.dock.is_none() && w.ed.doc.path.as_ref().is_some_and(|q| Self::canon(q) == want)) {
                Some(i) => i,
                None => {
                    self.open_path(&path);
                    self.windows.len() - 1
                }
            };
            let ed = &mut self.windows[i].ed;
            let new = edited(&ed.doc, &list);
            ed.doc.break_undo_group();
            ed.doc.replace_all_minimal(&new, ed.cursor);
            ed.doc.break_undo_group();
            ed.cursor = ed.cursor.min(ed.doc.rope.len_chars());
            ed.anchor = None;
            n += list.len();
            files += 1;
        }
        if let Some(id) = front {
            if let Some(i) = self.windows.iter().position(|w| w.win.id == id) {
                let w = self.windows.remove(i);
                self.windows.push(w);
            }
        }
        self.renumber();
        (n, files)
    }

    // ---- requests -----------------------------------------------------------------------

    pub(super) fn lsp_format(&mut self) {
        let Some((key, path, _)) = self.lsp_here() else {
            self.no_server();
            return;
        };
        let indent = self.windows.last().map(|w| detect_indent(&w.ed.doc)).unwrap_or(4);
        let Some(c) = self.lsp.get_mut(&key) else { return };
        let id = c.formatting(&path, indent, true);
        self.lspx.format = Some((key, id));
    }

    pub(super) fn lsp_quick_fix(&mut self) {
        let Some((key, path, pos)) = self.lsp_here() else {
            self.no_server();
            return;
        };
        let Some(w) = self.windows.last() else { return };
        let ed = &w.ed;
        let to_pos = |ch: usize| {
            let (line, byte) = ed.doc.char_to_line_byte(ch);
            Pos { line: line as u32, character: lsp::utf16_col(&ed.doc.line(line), byte) }
        };
        let (start, end) = ed.selection().map(|(a, b)| (to_pos(a), to_pos(b))).unwrap_or((pos, pos));
        // The diagnostics on those lines, as the server sent them.
        let diags: Vec<Value> = self
            .lspx
            .diags
            .get(&Self::canon(&path))
            .map(|v| {
                v.iter()
                    .filter(|d| {
                        let (a, b) = (d["range"]["start"]["line"].as_u64().unwrap_or(0) as u32, d["range"]["end"]["line"].as_u64().unwrap_or(0) as u32);
                        a <= end.line && b >= start.line
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let at = ed.cursor_screen_pos().unwrap_or(Point::new(2, 2));
        let Some(c) = self.lsp.get_mut(&key) else { return };
        let id = c.code_actions(&path, start, end, diags);
        self.lspx.actions_req = Some((key, id, at));
    }

    pub(super) fn lsp_signature(&mut self) {
        let Some((key, path, pos)) = self.lsp_here() else { return };
        let win = self.windows.last().map(|w| w.win.id).unwrap_or(0);
        let Some(c) = self.lsp.get_mut(&key) else { return };
        let id = c.signature_help(&path, pos);
        self.lspx.sig_req = Some((key, id, win, pos.line as usize));
    }

    /// After a key typed in an editor: signature help opens on ( and ,
    /// and closes on ) or leaving the line.
    pub(super) fn lsp_after_key(&mut self, k: &KeyEvent) {
        let Some(w) = self.windows.last() else { return };
        if w.dock.is_some() || w.lsp_key.is_none() {
            return;
        }
        let line = w.ed.cursor_line_col().0;
        if let Some((win, l, _)) = &self.lspx.signature {
            if *win != w.win.id || *l != line || k.key == Key::Esc || k.key == Key::Char(')') {
                self.lspx.signature = None;
            }
        }
        if matches!(k.key, Key::Char('(' | ',')) && !k.mods.ctrl && !k.mods.alt {
            self.lsp_signature();
        }
    }

    /// Inlay hints and semantic tokens for the editor in front, a moment
    /// after typing pauses; symbols for the palette's `#` search.
    pub(super) fn lsp_more_tick(&mut self, now: Instant) {
        if let Some(i) = self.editor_index() {
            let w = &self.windows[i];
            if let (Some(key), Some(path)) = (w.lsp_key.clone(), w.ed.doc.path.clone()) {
                let (id, v) = (w.win.id, w.ed.doc.version);
                let since = match self.lspx.changed.get(&id) {
                    Some((cv, t)) if *cv == v => *t,
                    _ => {
                        self.lspx.changed.insert(id, (v, now));
                        now
                    }
                };
                let idle = now.duration_since(since) > Duration::from_millis(400);
                let ready = self.lsp.get(&key).is_some_and(|c| c.is_ready() && c.is_open(&path)) && w.lsp_sent == Some(v);
                if idle && ready {
                    // (Inside the file: a range past its end gets refused.)
                    let last = w.ed.doc.len_lines().saturating_sub(1);
                    let (from, to) = (w.ed.scroll_y.min(last) as u32, (w.ed.scroll_y + 200).min(last) as u32);
                    let retry_ok = self.lspx.inlay_empty_at.get(&id).is_none_or(|t| now.duration_since(*t) > Duration::from_secs(3));
                    let want_inlay = w.ed.inlays_version != v && retry_ok && self.lspx.inlay_req.get(&id).is_none_or(|r| r.2 != v || self.lspx.inlay_empty_at.contains_key(&id));
                    let want_sem = w.ed.semantic_version != v && self.lspx.sem_req.get(&id).is_none_or(|r| r.2 != v);
                    let has_inlay = self.lsp.get(&key).is_some_and(|c| !c.capabilities["inlayHintProvider"].is_null());
                    if let Some(c) = self.lsp.get_mut(&key) {
                        if want_inlay && has_inlay && !self.lspx.inlays_off {
                            let rid = c.inlay_hints(&path, from, to);
                            self.lspx.inlay_req.insert(id, (key.clone(), rid, v));
                        }
                        if want_sem && !self.lspx.semantic_off {
                            if let Some(rid) = c.semantic_tokens(&path) {
                                self.lspx.sem_req.insert(id, (key.clone(), rid, v));
                            }
                        }
                    }
                }
            }
        }
        // `#` in quick open: the project's symbols.
        let q = self.shell.palette.as_ref().filter(|p| p.input.text.starts_with('#')).map(|p| p.input.text[1..].trim().to_string());
        if let Some(q) = q {
            if q != self.lspx.ws_query && !q.is_empty() {
                self.lspx.ws_query = q.clone();
                let key = self.lsp.keys().find(|(_, root)| *root == self.shell.explorer.root).or_else(|| self.lsp.keys().next()).cloned();
                if let Some(key) = key {
                    if let Some(c) = self.lsp.get_mut(&key) {
                        let id = c.workspace_symbols(&q);
                        self.lspx.ws_req = Some((key, id));
                    }
                } else if let Some(p) = &mut self.shell.palette {
                    p.set_workspace_symbols(Vec::new(), "No language server running for this project");
                }
            }
        }
    }

    // ---- answers ------------------------------------------------------------------------

    /// The new kinds of answer. True when it was one of them.
    pub(super) fn lsp_more_event(&mut self, key: &LspKey, e: &LspEvent) -> bool {
        match e {
            LspEvent::Formatting { id, edits } => {
                if self.lspx.format.as_ref() != Some(&(key.clone(), *id)) {
                    return true;
                }
                self.lspx.format = None;
                let Some(path) = self.windows.last().and_then(|w| w.ed.doc.path.clone()) else { return true };
                if edits.is_empty() {
                    self.say("Already formatted");
                } else {
                    let (n, _) = self.apply_workspace_edit(vec![(path, edits.clone())]);
                    self.say(format!("Formatted ({n} changes)"));
                }
            }
            LspEvent::CodeActions { id, actions } => {
                let Some((k, rid, at)) = self.lspx.actions_req.clone() else { return true };
                if (&k, rid) != (key, *id) {
                    return true;
                }
                self.lspx.actions_req = None;
                if actions.is_empty() {
                    self.say("No quick fixes here");
                    return true;
                }
                let mut actions = actions.clone();
                actions.sort_by_key(|a| (!a.preferred, !a.kind.starts_with("quickfix")));
                let items = actions.iter().map(|a| (format!("{}{}", if a.preferred { "★ " } else { "" }, a.title), a.kind.clone())).collect();
                let mut popup = ListPopup::new(items);
                popup.max_rows = 12;
                popup.place(at, self.screen());
                self.lspx.menu = Some(ActionMenu { popup, actions, key: k, at });
            }
            LspEvent::CodeActionResolved { id, action } => {
                if self.lspx.resolve_req.as_ref() != Some(&(key.clone(), *id)) {
                    return true;
                }
                self.lspx.resolve_req = None;
                self.apply_code_action(key.clone(), action.clone(), false);
            }
            LspEvent::Signature { id, sig } => {
                let Some((k, rid, win, line)) = self.lspx.sig_req.clone() else { return true };
                if (&k, rid) != (key, *id) {
                    return true;
                }
                self.lspx.sig_req = None;
                self.lspx.signature = sig.clone().map(|s| (win, line, s));
            }
            LspEvent::Refresh => {
                // (Everything this server shows: asked again on the next tick.)
                for w in self.windows.iter_mut().filter(|w| w.lsp_key.as_ref() == Some(key)) {
                    w.ed.inlays_version = u64::MAX;
                    w.ed.semantic_version = u64::MAX;
                }
                self.lspx.inlay_req.retain(|_, r| &r.0 != key);
                self.lspx.sem_req.retain(|_, r| &r.0 != key);
            }
            LspEvent::InlayHints { id, hints } => {
                let Some((win, v)) = self.lspx.inlay_req.iter().find(|(_, r)| (&r.0, r.1) == (key, *id)).map(|(w, r)| (*w, r.2)) else { return true };
                if let Some(w) = self.windows.iter_mut().find(|w| w.win.id == win) {
                    if w.ed.doc.version == v {
                        let mut map: std::collections::BTreeMap<usize, Vec<(usize, String)>> = Default::default();
                        for (p, label, pad_l, pad_r) in hints {
                            let line = p.line as usize;
                            if line >= w.ed.doc.len_lines() {
                                continue;
                            }
                            let text = w.ed.doc.line(line);
                            let byte = lsp::byte_of_utf16(&text, p.character);
                            let label = format!("{}{label}{}", if *pad_l { " " } else { "" }, if *pad_r { " " } else { "" });
                            map.entry(line).or_default().push((byte, label));
                        }
                        // (None yet: the server may still be indexing; ask
                        // again in a moment rather than settle for nothing.)
                        if hints.is_empty() {
                            self.lspx.inlay_empty_at.insert(win, Instant::now());
                            self.lspx.inlay_req.remove(&win);
                        } else {
                            self.lspx.inlay_empty_at.remove(&win);
                            w.ed.inlays = map;
                            w.ed.inlays_version = v;
                        }
                    }
                }
            }
            LspEvent::SemanticTokens { id, tokens } => {
                let Some((win, v)) = self.lspx.sem_req.iter().find(|(_, r)| (&r.0, r.1) == (key, *id)).map(|(w, r)| (*w, r.2)) else { return true };
                if let Some(w) = self.windows.iter_mut().find(|w| w.win.id == win) {
                    if w.ed.doc.version == v {
                        let mut lines: Vec<Vec<(u32, u32, u8)>> = vec![Vec::new(); w.ed.doc.len_lines()];
                        for (line, start, len, ty) in tokens {
                            let Some(kind) = semantic_kind(ty).and_then(turbine_syntax::kind_index) else { continue };
                            let Some(spans) = lines.get_mut(*line as usize) else { continue };
                            let text = w.ed.doc.line(*line as usize);
                            let (a, b) = (lsp::byte_of_utf16(&text, *start), lsp::byte_of_utf16(&text, start + len));
                            spans.push((a as u32, b as u32, kind));
                        }
                        w.ed.semantic = lines;
                        w.ed.semantic_version = v;
                    }
                }
            }
            LspEvent::WorkspaceSymbols { id, symbols } => {
                if self.lspx.ws_req.as_ref() != Some(&(key.clone(), *id)) {
                    return true;
                }
                self.lspx.ws_symbols = symbols.clone();
                let root = self.shell.explorer.root.clone();
                let items: Vec<palette::Item> = symbols
                    .iter()
                    .map(|s| palette::Item {
                        label: s.name.clone(),
                        detail: format!("{}  {}", s.container, s.path.strip_prefix(&root).unwrap_or(&s.path).display()).trim().to_string(),
                        pick: palette::Pick::Place(s.path.clone(), s.start.line as usize, s.start.character as usize),
                    })
                    .collect();
                if let Some(p) = &mut self.shell.palette {
                    p.set_workspace_symbols(items, "No matching symbols");
                }
            }
            LspEvent::ApplyEdit { edits } => {
                let (n, files) = self.apply_workspace_edit(edits.clone());
                if n > 0 {
                    self.say(format!("Applied {n} changes in {files} files"));
                }
            }
            _ => return false,
        }
        true
    }

    /// Do a code action: its edit, then its command; resolve it first when
    /// it came without either.
    fn apply_code_action(&mut self, key: LspKey, a: CodeAction, may_resolve: bool) {
        if a.edit.is_none() && a.command.is_none() && may_resolve {
            if let Some(c) = self.lsp.get_mut(&key) {
                let id = c.resolve_code_action(&a.raw);
                self.lspx.resolve_req = Some((key, id));
            }
            return;
        }
        if let Some(e) = a.edit {
            self.apply_workspace_edit(e);
        }
        if let Some(cmd) = &a.command {
            if let Some(c) = self.lsp.get_mut(&key) {
                c.execute_command(cmd);
            }
        }
        self.say(a.title);
    }

    // ---- the quick fix menu and the signature box ---------------------------------------

    pub(super) fn lsp_menu_key(&mut self, k: &KeyEvent) -> bool {
        let Some(m) = &mut self.lspx.menu else { return false };
        match m.popup.handle_key(k) {
            PopupResult::Accept(i) => {
                let m = self.lspx.menu.take().unwrap();
                let a = m.actions[i].clone();
                self.apply_code_action(m.key, a, true);
            }
            PopupResult::Close => self.lspx.menu = None,
            _ => {}
        }
        true
    }

    pub(super) fn lsp_menu_mouse(&mut self, ev: &MouseEvent) -> bool {
        let Some(m) = &mut self.lspx.menu else { return false };
        match m.popup.handle_mouse(ev) {
            PopupResult::Accept(i) => {
                let m = self.lspx.menu.take().unwrap();
                let a = m.actions[i].clone();
                self.apply_code_action(m.key, a, true);
            }
            PopupResult::Close => self.lspx.menu = None,
            _ => {}
        }
        true
    }

    /// The signature box's place: above the cursor (below near the top).
    fn signature_rect(&self) -> Option<(Rect, Point)> {
        let (win, _, sig) = self.lspx.signature.as_ref()?;
        let w = self.windows.last().filter(|w| w.win.id == *win)?;
        let at = w.ed.cursor_screen_pos()?;
        let screen = self.screen();
        let doc_line = sig.doc.lines().find(|l| !l.trim().is_empty());
        let width = (str_width(&sig.label).max(doc_line.map_or(0, str_width)) as i32 + 4).min(screen.w - 2);
        let h = 3 + i32::from(doc_line.is_some());
        let y = if at.y - h >= 1 { at.y - h } else { at.y + 1 };
        let x = (at.x - 2).clamp(0, (screen.w - width).max(0));
        Some((Rect::new(x, y, width, h), at))
    }

    pub(super) fn draw_lsp_more(&mut self, c: &mut Canvas, theme: &Theme) {
        let s = &theme.scheme;
        if let Some((r, _)) = self.signature_rect() {
            let (_, _, sig) = self.lspx.signature.as_ref().unwrap();
            c.fill(r, " ", s.menu);
            turbine_ui::frame::draw_frame(c, r, theme.style.menu_frame, s.menu);
            if theme.style.shadows {
                c.shadow(r, s.shadow);
            }
            let mut tc = c.sub(r.inset(1));
            let (y, mut x) = (r.y + 1, r.x + 2);
            // The parameter being typed, underlined and bright.
            let (a, b) = sig.active.unwrap_or((0, 0));
            let hot = Style { attrs: turbine_ui::theme::Attrs { underline: true, bold: true, ..s.menu_selected_hotkey.attrs }, ..s.menu_hotkey };
            for (part, st) in [(&sig.label[..a.min(sig.label.len())], s.menu), (&sig.label[a.min(sig.label.len())..b.min(sig.label.len())], hot), (&sig.label[b.min(sig.label.len())..], s.menu)] {
                x = tc.print(x, y, part, st);
            }
            if let Some(d) = sig.doc.lines().find(|l| !l.trim().is_empty()) {
                tc.print(r.x + 2, y + 1, d.trim(), s.menu_disabled);
            }
        }
        if let Some(m) = &mut self.lspx.menu {
            let screen = Rect::new(0, 0, self.size.0, self.size.1);
            m.popup.draw(c, m.at, screen, theme);
        }
    }

    pub(super) fn lsp_more_nodes(&self, mut root: Node) -> Node {
        if let Some((r, _)) = self.signature_rect() {
            let (_, _, sig) = self.lspx.signature.as_ref().unwrap();
            root = root.child(Node::new("signature", "tooltip", sig.label.clone(), r));
        }
        if let Some(m) = &self.lspx.menu {
            let mut n = Node::new("quick_fix", "menu", "Quick Fix", m.popup.rect()).focused(true);
            for (i, (label, _)) in m.popup.items.iter().enumerate() {
                if let Some(r) = m.popup.item_rect(i) {
                    n = n.child(Node::new(format!("quick_fix.{i}"), "menuitem", label.clone(), r));
                }
            }
            root = root.child(n);
        }
        root
    }

    // ---- installing a missing server ----------------------------------------------------

    /// A language feature was asked for with no server: offer to install
    /// one (once a session per language), else say so.
    pub(super) fn no_server(&mut self) {
        let lang = self.windows.last().and_then(|w| w.lang.clone());
        match lang {
            Some(l) if self.lsp_missing.contains(&l) && !self.lspx.offered.contains(&l) && lsp::install_command(&l).is_some() => {
                self.lspx.offered.insert(l.clone());
                self.offer_install(&l);
            }
            Some(l) if self.lsp_missing.contains(&l) => self.say(format!("No language server for {l} (Edit > Install Language Server)")),
            _ => self.say("No language server for this file"),
        }
    }

    /// Offer to install the language server for `lang` (in a terminal, so
    /// you see what runs).
    pub(super) fn offer_install(&mut self, lang: &str) {
        match lsp::install_command(lang) {
            Some((server, cmd)) => {
                let d = Dialog::new(&format!("install:{lang}\u{1f}{cmd}"), "Install Language Server")
                    .body(ui::label(&format!("No language server for {lang} was found.\nInstall {server} with:\n\n  {cmd}\n\nIt runs in a terminal in the panel.")))
                    .buttons(vec![ui::button("ok", "~I~nstall", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            None => self.say(format!("Turbine doesn't know a language server for {lang} yet")),
        }
    }

    /// Run an install command in a panel terminal; look for servers again.
    pub(super) fn run_install(&mut self, lang: &str, cmd: &str) {
        self.new_terminal(Vec::new());
        if let Some(t) = self.windows.last_mut().and_then(|w| w.term.as_mut()) {
            t.paste(&format!("{cmd}\n"));
        }
        self.layout_dock();
        if let Some(id) = self.windows.last().map(|w| w.win.id) {
            self.shell.dock.show(crate::dock::Tab::Pane(id));
        }
        // (Try starting it again from now on.)
        self.lsp_missing.remove(lang);
        self.lspx.offered.insert(lang.to_string());
    }
}

/// The document's indent width (its first indented line), else 4.
fn detect_indent(doc: &Document) -> u32 {
    (0..doc.len_lines().min(200))
        .map(|i| doc.line(i))
        .find_map(|l| {
            let n = l.len() - l.trim_start_matches(' ').len();
            (n > 0 && !l.starts_with('\t') && n <= 8).then_some(n as u32)
        })
        .unwrap_or(4)
}
