//! The editor's side of the extension host: answering extensions' calls
//! (documents, edits, menu items, key bindings) on the UI thread, running
//! their commands ("ext:<extension>:<command>"), and their key bindings.

use super::*;
use turbine_ext::{Answer, ExtHost, Request};

#[derive(Default)]
pub(super) struct ExtState {
    pub host: Option<ExtHost>,
    /// Extensions' key bindings: (chord, command to run).
    pub keys: Vec<(KeyEvent, String)>,
}

/// Where Turbine looks for extensions: `TURBINE_EXTENSIONS` (folders
/// separated by ':'), else $XDG_DATA_HOME/turbine/extensions.
pub fn extension_dirs() -> Vec<PathBuf> {
    if let Some(v) = std::env::var_os("TURBINE_EXTENSIONS") {
        return std::env::split_paths(&v).collect();
    }
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    data.map(|d| vec![d.join("turbine/extensions")]).unwrap_or_default()
}

impl Workbench {
    /// Load the extensions in these folders (on their own thread).
    pub fn start_extensions(&mut self, dirs: Vec<PathBuf>) {
        self.exts.host = Some(ExtHost::start(dirs));
    }

    /// The installed extensions (for the Extensions view).
    pub(super) fn extension_list(&self) -> Vec<turbine_ext::ExtInfo> {
        self.exts.host.as_ref().map(|h| h.list()).unwrap_or_default()
    }

    /// Answer what the extensions asked since the last look.
    pub(super) fn ext_poll(&mut self) {
        let calls = match &self.exts.host {
            Some(h) => h.poll(),
            None => return,
        };
        for call in calls {
            let ext = call.ext.clone();
            let answer = match &call.request {
                Request::Log(m) => {
                    if self.windows.iter().any(|w| w.special == Some("output")) {
                        self.append_output(&format!("[{ext}] {m}\n"));
                    }
                    Answer::None
                }
                Request::Status(m) => {
                    self.say(m.clone());
                    Answer::None
                }
                Request::ActiveDocument => Answer::Doc(self.editor_index().filter(|&i| self.windows[i].special.is_none()).map(|i| self.windows[i].win.id)),
                Request::DocumentText(id) => Answer::Text(self.doc_of(*id).map(|w| w.ed.doc.rope.to_string()).unwrap_or_default()),
                Request::DocumentPath(id) => Answer::MaybeText(self.doc_of(*id).and_then(|w| w.ed.doc.path.as_ref().map(|p| p.display().to_string()))),
                Request::DocumentLanguage(id) => Answer::MaybeText(self.doc_of(*id).and_then(|w| w.lang.clone())),
                Request::Cursor(id) => {
                    let (l, c) = self.doc_of(*id).map(|w| w.ed.cursor_line_col()).unwrap_or((0, 0));
                    Answer::Pos(l as u32, c as u32)
                }
                Request::Edit { doc, start, end, text } => {
                    if let Some(w) = self.windows.iter_mut().find(|w| w.win.id == *doc) {
                        let ed = &mut w.ed;
                        let at = |ed: &EditorView, (l, c): (u32, u32)| {
                            let l = (l as usize).min(ed.doc.len_lines().saturating_sub(1));
                            let start = ed.doc.rope.line_to_char(l);
                            let len = ed.doc.rope.line(l).len_chars();
                            start + (c as usize).min(len)
                        };
                        let (a, b) = (at(ed, *start), at(ed, *end));
                        let (a, b) = (a.min(b), a.max(b));
                        ed.doc.break_undo_group();
                        ed.doc.replace(a, b, text, ed.cursor);
                        ed.doc.break_undo_group();
                        ed.cursor = ed.cursor.min(ed.doc.rope.len_chars());
                    }
                    Answer::None
                }
                Request::AddMenuItem { menu, label, shortcut, command } => {
                    self.ext_menu_item(menu, label, shortcut.as_deref().unwrap_or(""), &format!("ext:{ext}:{command}"));
                    Answer::None
                }
                Request::BindKey { chord, command, .. } => {
                    match KeyEvent::parse(chord) {
                        Some(k) => self.exts.keys.push((k, format!("ext:{ext}:{command}"))),
                        None => self.say(format!("{ext}: can't read the key {chord}")),
                    }
                    Answer::None
                }
                Request::RunCommand { command, .. } => {
                    self.run_command(command);
                    Answer::Result(Ok(String::new()))
                }
                Request::Setting(key) => {
                    let v = self.setting_value(key);
                    Answer::MaybeText((!v.is_empty()).then_some(v))
                }
                Request::Finished { command, result } => {
                    match result {
                        Ok(m) if !m.is_empty() => self.say(m.clone()),
                        Ok(_) => {}
                        Err(e) if command.is_empty() => self.say(format!("Extension {ext} didn't load: {e}")),
                        Err(e) => self.say(format!("{ext}: {e}")),
                    }
                    Answer::None
                }
            };
            call.reply(answer);
        }
    }

    fn doc_of(&self, id: u32) -> Option<&EdWin> {
        self.windows.iter().find(|w| w.win.id == id && w.term.is_none())
    }

    /// An extension's menu item: into the menu of that name (made before
    /// Window when there's none).
    fn ext_menu_item(&mut self, menu: &str, label: &str, shortcut: &str, command: &str) {
        let menus = &mut self.menubar.menus;
        let i = match menus.iter().position(|m| m.plain_title().eq_ignore_ascii_case(menu)) {
            Some(i) => i,
            None => {
                let at = menus.iter().position(|m| m.plain_title() == "Window").unwrap_or(menus.len());
                menus.insert(at, Menu::new(menu, Vec::new()));
                at
            }
        };
        if !menus[i].items.iter().any(|it| it.command == command) {
            menus[i].items.push(MenuItem::new(label, shortcut, command));
        }
    }

    /// Run "ext:<extension>:<command>". False when it isn't one.
    pub(super) fn ext_command(&mut self, cmd: &str) -> bool {
        let Some(rest) = cmd.strip_prefix("ext:") else { return false };
        let Some((ext, command)) = rest.split_once(':') else { return false };
        match &self.exts.host {
            Some(h) => h.run(ext, command, ""),
            None => self.say("Extensions aren't running"),
        }
        true
    }

    /// An extension's key binding for this key, if any.
    pub(super) fn ext_key(&self, k: &KeyEvent) -> Option<String> {
        self.exts.keys.iter().find(|(c, _)| c.key == k.key && c.mods == k.mods).map(|(_, cmd)| cmd.clone())
    }
}
