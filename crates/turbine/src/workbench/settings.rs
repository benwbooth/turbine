//! Settings (Cursor's settings editor, as a DOS dialog): every setting by
//! category, a search field that filters them, the selected one's
//! description, Enter to change it. Changes apply at once and are kept with
//! the look (look.json).

use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Bool,
    /// One of a list (the list comes from `choices`).
    Choice,
    /// A number or text, typed.
    Text,
}

struct Def {
    key: &'static str,
    category: &'static str,
    label: &'static str,
    description: &'static str,
    kind: Kind,
}

const DEFS: &[Def] = &[
    Def { key: "style", category: "Appearance", label: "Control style", description: "How windows, dialogs and controls are drawn: Turbo Vision, QBasic / MS-DOS Edit, VB-DOS or plain ASCII. Each brings its own colours.", kind: Kind::Choice },
    Def { key: "scheme", category: "Appearance", label: "Colour scheme", description: "The colours, independent of the control style.", kind: Kind::Choice },
    Def { key: "editor_tabs", category: "Appearance", label: "Editor tabs", description: "A row of tabs for the open files above the editors.", kind: Kind::Bool },
    Def { key: "breadcrumbs", category: "Appearance", label: "Breadcrumbs", description: "A row under the tabs showing the file's folders and the definitions the cursor is in.", kind: Kind::Bool },
    Def { key: "minimap", category: "Appearance", label: "Minimap", description: "A miniature of the file at the editor's right; click or drag it to scroll.", kind: Kind::Bool },
    Def { key: "line_numbers", category: "Editor", label: "Line numbers", description: "Number the lines in editors (QBasic and Edit never did).", kind: Kind::Bool },
    Def { key: "keymap", category: "Editor", label: "Keymap", description: "CUA keys (DOS and Windows style) or Vim.", kind: Kind::Choice },
    Def { key: "tab_complete", category: "Editor", label: "AI tab completion", description: "Grey suggestions from the AI model as you type; Tab takes one.", kind: Kind::Bool },
    Def { key: "sidebar", category: "Layout", label: "Primary side bar", description: "The Explorer, Outline, Search, Source Control, Run and Debug and Extensions at the left (Ctrl+B).", kind: Kind::Bool },
    Def { key: "panel", category: "Layout", label: "Panel", description: "Problems, Output and terminals under the editors (Ctrl+J).", kind: Kind::Bool },
    Def { key: "secondary", category: "Layout", label: "Secondary side bar", description: "The AI chat at the right (Ctrl+Alt+B).", kind: Kind::Bool },
    Def { key: "hidden_files", category: "Files", label: "Show hidden files", description: "List files and folders whose names start with a dot in the Explorer.", kind: Kind::Bool },
    Def { key: "chat_model", category: "AI", label: "Chat model", description: "The model for the chat, Ctrl+K and the agent (from hearth's AI hub, or Ollama / llama.cpp).", kind: Kind::Choice },
    Def { key: "chat_agent", category: "AI", label: "Chat sends to the agent", description: "The chat field starts in Agent mode (Ctrl+. switches it).", kind: Kind::Bool },
    Def { key: "agent_speed", category: "Agent", label: "Typing speed", description: "Characters a second the agent types at, so you can watch it work (0: as fast as it can).", kind: Kind::Text },
];

fn def(key: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.key == key)
}

impl Workbench {
    /// A setting's value as shown.
    pub(super) fn setting_value(&self, key: &str) -> String {
        let b = |v: bool| if v { "on" } else { "off" }.to_string();
        let mm = self.shell.minimap.unwrap_or(self.theme.style.icons);
        match key {
            "style" => self.theme.style.name.to_string(),
            "scheme" => self.theme.scheme.name.to_string(),
            "editor_tabs" => b(self.shell.dock.editor_tabs),
            "breadcrumbs" => b(self.shell.dock.breadcrumbs),
            "minimap" => b(mm),
            "line_numbers" => b(self.editor_index().map(|i| self.windows[i].ed.line_numbers).unwrap_or(self.theme.style.line_numbers)),
            "keymap" => self.keymap.name().to_string(),
            "tab_complete" => b(self.tab_complete),
            "sidebar" => b(self.shell.dock.left.visible),
            "panel" => b(self.shell.dock.bottom.visible),
            "secondary" => b(self.shell.dock.right.visible),
            "hidden_files" => b(self.shell.explorer.show_hidden),
            "chat_model" => self.chat_model().map(|m| m.name).unwrap_or_else(|| "(none yet)".into()),
            "chat_agent" => b(self.shell.chat_agent),
            "agent_speed" => format!("{}", self.driver.cps),
            _ => String::new(),
        }
    }

    /// The choices for a Choice setting.
    fn setting_choices(&self, key: &str) -> Vec<String> {
        match key {
            "style" => ControlStyle::all().iter().map(|s| s.name.to_string()).collect(),
            "scheme" => Scheme::all().iter().map(|s| s.name.to_string()).collect(),
            "keymap" => vec![Keymap::Cua.name().to_string(), Keymap::Vim.name().to_string()],
            "chat_model" => self.ai.as_ref().map(|a| a.models.iter().filter(|m| m.chat).map(|m| m.name.clone()).collect()).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Set a setting (from the editor, or from saved settings).
    pub(super) fn setting_set(&mut self, key: &str, value: &str) {
        let on = matches!(value, "on" | "true");
        let toggle = |wb: &mut Workbench, cmd: &str, now: bool| {
            if now != on {
                wb.run_command(cmd);
            }
        };
        match key {
            "style" => {
                if let Some(st) = ControlStyle::all().into_iter().find(|s| s.name == value) {
                    self.apply_style(st);
                }
            }
            "scheme" => {
                if let Some(sc) = Scheme::all().into_iter().find(|s| s.name == value) {
                    self.theme.scheme = sc;
                }
            }
            "editor_tabs" => toggle(self, "view.editor_tabs", self.shell.dock.editor_tabs),
            "breadcrumbs" => toggle(self, "view.breadcrumbs", self.shell.dock.breadcrumbs),
            "minimap" => self.shell.minimap = Some(on),
            "line_numbers" => {
                for w in self.windows.iter_mut().filter(|w| w.dock.is_none() && w.special.is_none()) {
                    w.ed.line_numbers = on;
                }
                self.shell.line_numbers = Some(on);
            }
            "keymap" => self.set_keymap(if value == Keymap::Vim.name() { Keymap::Vim } else { Keymap::Cua }),
            "tab_complete" => self.tab_complete = on,
            "sidebar" => toggle(self, "view.sidebar", self.shell.dock.left.visible),
            "panel" => toggle(self, "view.panel", self.shell.dock.bottom.visible),
            "secondary" => toggle(self, "view.secondary", self.shell.dock.right.visible),
            "hidden_files" => self.shell.explorer.set_show_hidden(on),
            "chat_model" => self.chat_model = Some(value.to_string()),
            "chat_agent" => self.shell.chat_agent = on,
            "agent_speed" => {
                if let Ok(n) = value.trim().parse::<f64>() {
                    self.driver.cps = n.clamp(0.0, 10_000.0);
                }
            }
            _ => {}
        }
        self.sync_menu_checks();
    }

    /// What's kept between runs (beyond the look and layout).
    pub(super) fn settings_json(&self) -> Value {
        json!({
            "keymap": self.keymap.name(),
            "tab_complete": self.tab_complete,
            "hidden_files": self.shell.explorer.show_hidden,
            "chat_model": self.chat_model,
            "chat_agent": self.shell.chat_agent,
            "agent_speed": self.driver.cps,
            "line_numbers": self.shell.line_numbers,
        })
    }

    pub(super) fn apply_settings_json(&mut self, v: &Value) {
        if !v.is_object() {
            return;
        }
        if let Some(k) = v["keymap"].as_str() {
            self.setting_set("keymap", k);
        }
        if let Some(b) = v["tab_complete"].as_bool() {
            self.tab_complete = b;
        }
        if let Some(b) = v["hidden_files"].as_bool() {
            self.shell.explorer.set_show_hidden(b);
        }
        if let Some(m) = v["chat_model"].as_str() {
            self.chat_model = Some(m.to_string());
        }
        if let Some(b) = v["chat_agent"].as_bool() {
            self.shell.chat_agent = b;
        }
        if let Some(n) = v["agent_speed"].as_f64() {
            self.driver.cps = n;
        }
        if let Some(b) = v["line_numbers"].as_bool() {
            self.setting_set("line_numbers", if b { "on" } else { "off" });
        }
    }

    /// The settings matching the search, as list rows (and their keys).
    fn settings_rows(&self, query: &str) -> (Vec<String>, Vec<&'static str>) {
        let q = query.trim().to_lowercase();
        let mut rows = Vec::new();
        let mut keys = Vec::new();
        for d in DEFS {
            let hay = format!("{} {} {} {}", d.category, d.label, d.description, d.key).to_lowercase();
            if !q.is_empty() && !q.split_whitespace().all(|w| hay.contains(w)) {
                continue;
            }
            rows.push(format!("{:<11}{:<26}{}", d.category, d.label, self.setting_value(d.key)));
            keys.push(d.key);
        }
        (rows, keys)
    }

    pub(super) fn open_settings(&mut self) {
        let (rows, _) = self.settings_rows("");
        let first = DEFS[0].description;
        let d = Dialog::new("settings", "Settings")
            .body(ui::col(vec![
                ui::input("q", "~S~earch settings:", "").chars(40),
                ui::list("items", rows, 0).header(&format!("{:<11}{:<26}{}", "Category", "Setting", "Value")).activates().chars(64).lines(15),
                ui::label(first).id("desc").chars(64),
            ]))
            .buttons(vec![ui::button("ok", "~C~lose", true)]);
        self.dialog = Some(d);
    }

    fn settings_refresh(&mut self) {
        let Some(d) = self.dialog.as_ref() else { return };
        let q = d.value("q").unwrap_or_default();
        let sel = d.selected_index("items").unwrap_or(0);
        let (rows, keys) = self.settings_rows(&q);
        let desc = keys.get(sel.min(keys.len().saturating_sub(1))).and_then(|k| def(k)).map(|d| d.description).unwrap_or("No setting matches.");
        let d = self.dialog.as_mut().unwrap();
        d.set_items("items", rows, sel);
        d.set_label("desc", desc);
    }

    /// The settings dialog's events. True when it was the settings dialog.
    pub(super) fn settings_event(&mut self, r: &DialogResult) -> bool {
        if !self.dialog.as_ref().is_some_and(|d| d.kind == "settings") {
            return false;
        }
        match r {
            DialogResult::Changed(id) if id == "q" || id == "items" => self.settings_refresh(),
            DialogResult::Activated(_) => {
                let d = self.dialog.as_ref().unwrap();
                let (_, keys) = self.settings_rows(&d.value("q").unwrap_or_default());
                let Some(key) = d.selected_index("items").and_then(|i| keys.get(i)).copied() else { return true };
                let Some(def) = def(key) else { return true };
                match def.kind {
                    Kind::Bool => {
                        let on = self.setting_value(key) == "on";
                        self.setting_set(key, if on { "off" } else { "on" });
                        self.save_look();
                    }
                    Kind::Choice | Kind::Text => {
                        // A small dialog over this one; its answer comes back here.
                        let cur = self.setting_value(key);
                        let q = if def.kind == Kind::Choice {
                            let choices = self.setting_choices(key);
                            if choices.is_empty() {
                                self.say("Nothing to choose from yet");
                                return true;
                            }
                            let sel = choices.iter().position(|c| *c == cur).unwrap_or(0);
                            Dialog::new(&format!("setting:{key}"), def.label)
                                .body(ui::list("value", choices, sel).activates())
                                .buttons(vec![ui::button("yes", "O~K~", true), ui::button("cancel", "Cancel", false)])
                        } else {
                            Dialog::new(&format!("setting:{key}"), def.label)
                                .body(ui::input("value", &format!("{}:", def.label), &cur).chars(20))
                                .buttons(vec![ui::button("yes", "O~K~", true), ui::button("cancel", "Cancel", false)])
                        };
                        self.overwrite_q = Some(q);
                    }
                }
                self.settings_refresh();
            }
            DialogResult::Button(_) | DialogResult::Cancel => self.dialog = None,
            _ => {}
        }
        true
    }

    /// The answer from a setting's own little dialog.
    pub(super) fn setting_answer(&mut self, q: &Dialog, ok: bool) {
        let Some(key) = q.kind.strip_prefix("setting:") else { return };
        if ok {
            if let Some(v) = q.value("value") {
                self.setting_set(key, &v);
                self.save_look();
            }
        }
        self.settings_refresh();
    }
}
