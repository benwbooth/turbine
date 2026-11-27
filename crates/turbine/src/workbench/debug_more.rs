//! More of the debugger, Turbo C and Cursor style: watches (Ctrl+F7),
//! Evaluate/Modify (Ctrl+F4), breakpoint conditions and log points, the
//! Immediate window (QBasic's, here the debug console: what's typed there
//! runs in the stopped program), and launch configurations from
//! `.turbine/launch.json` or `.vscode/launch.json`.

use super::*;
use turbine_services::dap::{BreakpointOpts, LaunchConfig};

/// Who an evaluation's answer is for.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum EvalFor {
    Watch(usize),
    /// The Evaluate/Modify dialog's Result field.
    Dialog,
    /// The Immediate window (its answer goes at the end).
    Immediate,
}

#[derive(Default)]
pub(super) struct DebugExtra {
    pub watches: Vec<String>,
    /// Each watch's value at the last stop (None: not evaluated yet).
    pub watch_vals: Vec<Option<Result<String, String>>>,
    pub evals: HashMap<i64, EvalFor>,
    /// Breakpoint conditions and log messages: path → line → options.
    pub bp_opts: BTreeMap<PathBuf, BTreeMap<u32, BreakpointOpts>>,
    /// The launch configuration chosen per project (its name).
    pub config: HashMap<PathBuf, String>,
}

/// A starting launch.json for a language.
fn launch_template(lang: &str) -> &'static str {
    match lang {
        "python" => {
            r#"{
    // Debug configurations: F5 / Run > Start Debugging asks which one.
    "version": "0.2.0",
    "configurations": [
        {
            "name": "Python: this file",
            "type": "debugpy",
            "request": "launch",
            "program": "${file}",
            "args": [],
            "cwd": "${workspaceFolder}"
        },
        {
            "name": "Python: attach",
            "type": "debugpy",
            "request": "attach",
            "connect": { "host": "localhost", "port": 5678 }
        }
    ]
}
"#
        }
        "go" => {
            r#"{
    "version": "0.2.0",
    "configurations": [
        {
            "name": "Go: this package",
            "type": "go",
            "request": "launch",
            "mode": "debug",
            "program": "${fileDirname}"
        }
    ]
}
"#
        }
        _ => {
            r#"{
    // Debug configurations: F5 / Run > Start Debugging asks which one.
    "version": "0.2.0",
    "configurations": [
        {
            "name": "Debug the program",
            "type": "lldb",
            "request": "launch",
            "program": "${workspaceFolder}/target/debug/${workspaceFolderBasename}",
            "args": [],
            "cwd": "${workspaceFolder}"
        },
        {
            "name": "Attach to a process",
            "type": "lldb",
            "request": "attach",
            "pid": 0
        }
    ]
}
"#
        }
    }
}

impl Workbench {
    // ---- launch configurations ------------------------------------------------------------

    /// The project's launch configurations for the active file.
    pub(super) fn launch_configs(&self) -> (PathBuf, Vec<LaunchConfig>) {
        let file = self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none() && w.ed.doc.path.is_some()).and_then(|w| w.ed.doc.path.clone());
        let root = file.as_ref().map(|f| lsp::project_root(f)).unwrap_or_else(|| self.shell.explorer.root.clone());
        let configs = dap::launch_configs(&root, file.as_deref());
        (root, configs)
    }

    /// Start debugging: with a launch configuration when the project has
    /// them (asking which when there are several), else the guessed program.
    /// True when it was handled here.
    pub(super) fn start_with_config(&mut self, stop_on_entry: bool) -> bool {
        let (root, configs) = self.launch_configs();
        if configs.is_empty() {
            return false;
        }
        let chosen = self.dbx.config.get(&root).and_then(|n| configs.iter().find(|c| &c.name == n)).cloned();
        match chosen.or_else(|| (configs.len() == 1).then(|| configs[0].clone())) {
            Some(c) => self.start_config(c, stop_on_entry),
            None => {
                let names: Vec<String> = configs.iter().map(|c| c.name.clone()).collect();
                let d = Dialog::new(&format!("launch_cfg:{}\u{1f}{}", u8::from(stop_on_entry), root.display()), "Select Debug Configuration")
                    .body(ui::list("config", names, 0).lines(8).chars(40))
                    .buttons(vec![ui::button("ok", "~S~tart", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
        }
        true
    }

    pub(super) fn start_config(&mut self, c: LaunchConfig, stop_on_entry: bool) {
        let file_lang = self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none()).and_then(|w| w.lang.clone()).unwrap_or_default();
        let lang = match dap::lang_for_type(&c.kind) {
            "" => file_lang,
            l => l.to_string(),
        };
        let mut launch = c.launch.clone();
        launch.stop_on_entry |= stop_on_entry;
        match (self.dap_spawn)(&lang, launch, self.breakpoints.clone()) {
            None => self.say(format!("No debugger for \"{}\" ({lang}): install lldb-dap, gdb, debugpy or dlv", c.kind)),
            Some(Err(e)) => self.say(format!("Debugger didn't start: {e}")),
            Some(Ok(mut client)) => {
                client.options = self.dbx.bp_opts.clone();
                self.say(format!("Debugging \"{}\" with {}", c.name, client.name()));
                self.debug_started(client);
            }
        }
    }

    /// Run > Debug Configurations: open the project's launch.json (made
    /// from a template when there's none).
    pub(super) fn edit_launch_json(&mut self) {
        let file = self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none() && w.ed.doc.path.is_some()).and_then(|w| w.ed.doc.path.clone());
        let root = file.as_ref().map(|f| lsp::project_root(f)).unwrap_or_else(|| self.shell.explorer.root.clone());
        let existing = [".turbine/launch.json", ".vscode/launch.json"].iter().map(|f| root.join(f)).find(|p| p.is_file());
        let path = match existing {
            Some(p) => p,
            None => {
                let p = root.join(".turbine/launch.json");
                let lang = self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none()).and_then(|w| w.lang.clone()).unwrap_or_default();
                if let Err(e) = std::fs::create_dir_all(root.join(".turbine")).and_then(|_| std::fs::write(&p, launch_template(&lang))) {
                    self.say(format!("Can't write {}: {e}", p.display()));
                    return;
                }
                p
            }
        };
        self.goto_file(&path, None, 0);
    }

    // ---- watches and evaluating -------------------------------------------------------------

    /// The word at the cursor, or the selection (to start a watch with).
    fn expression_here(&self) -> String {
        let Some(w) = self.windows.last().filter(|w| w.term.is_none()) else { return String::new() };
        if let Some((a, b)) = w.ed.selection() {
            let t = w.ed.doc.rope.slice(a..b).to_string();
            if !t.contains('\n') {
                return t;
            }
        }
        self.word_at_cursor()
    }

    pub(super) fn add_watch_dialog(&mut self) {
        let expr = self.expression_here();
        let d = Dialog::new("watch", "Add Watch")
            .body(ui::input("expr", "~W~atch expression:", &expr).chars(40))
            .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
        self.dialog = Some(d);
    }

    pub(super) fn add_watch(&mut self, expr: &str) {
        let expr = expr.trim();
        if expr.is_empty() || self.dbx.watches.iter().any(|w| w == expr) {
            return;
        }
        self.dbx.watches.push(expr.to_string());
        self.dbx.watch_vals.push(None);
        let i = self.dbx.watches.len() - 1;
        self.eval_watch(i);
        self.show_debug_info();
        // (Shown, but the keys stay where they were.)
        self.layout_dock();
        self.shell.dock.show(crate::dock::Tab::Debug);
    }

    pub(super) fn remove_watch(&mut self, i: usize) {
        if i < self.dbx.watches.len() {
            self.dbx.watches.remove(i);
            self.dbx.watch_vals.remove(i);
            // (Answers on their way for later watches move down.)
            let evals = std::mem::take(&mut self.dbx.evals);
            self.dbx.evals = evals
                .into_iter()
                .filter_map(|(id, f)| match f {
                    EvalFor::Watch(j) if j == i => None,
                    EvalFor::Watch(j) if j > i => Some((id, EvalFor::Watch(j - 1))),
                    f => Some((id, f)),
                })
                .collect();
            self.show_debug_info();
        }
    }

    fn eval_watch(&mut self, i: usize) {
        let Some(d) = self.debug.as_mut().filter(|d| !d.client.running) else { return };
        let id = d.client.evaluate(&self.dbx.watches[i], "watch");
        self.dbx.evals.insert(id, EvalFor::Watch(i));
    }

    /// At each stop: every watch again.
    pub(super) fn eval_watches(&mut self) {
        for i in 0..self.dbx.watches.len() {
            self.eval_watch(i);
        }
    }

    pub(super) fn evaluate_dialog(&mut self) {
        let expr = self.expression_here();
        let d = Dialog::new("evaluate", "Evaluate and Modify")
            .body(ui::col(vec![
                ui::input("expr", "~E~xpression:", &expr).chars(44),
                ui::input("result", "~R~esult:", "").chars(44),
                ui::input("value", "~N~ew value:", "").chars(44),
            ]))
            .buttons(vec![ui::button("eval", "~E~valuate", true), ui::button("modify", "~M~odify", false), ui::button("cancel", "Cancel", false)]);
        self.dialog = Some(d);
        if !expr.is_empty() {
            self.evaluate_now(&expr);
        }
    }

    fn evaluate_now(&mut self, expr: &str) {
        match self.debug.as_mut().filter(|d| !d.client.running) {
            Some(d) => {
                let id = d.client.evaluate(expr, "watch");
                self.dbx.evals.insert(id, EvalFor::Dialog);
            }
            None => {
                if let Some(d) = &mut self.dialog {
                    d.set_value("result", "(the program isn't stopped)");
                }
            }
        }
    }

    /// The Evaluate/Modify dialog's buttons (it stays up).
    pub(super) fn evaluate_button(&mut self, mut d: Dialog, b: &str) {
        let expr = d.value("expr").unwrap_or_default();
        let value = d.value("value").unwrap_or_default();
        d.set_value("result", "");
        self.dialog = Some(d);
        if expr.trim().is_empty() {
            return;
        }
        if b == "modify" && !value.trim().is_empty() {
            match self.debug.as_mut().filter(|d| !d.client.running) {
                Some(dbg) => {
                    let id = dbg.client.set_expression(expr.trim(), value.trim());
                    self.dbx.evals.insert(id, EvalFor::Dialog);
                }
                None => {
                    if let Some(d) = &mut self.dialog {
                        d.set_value("result", "(the program isn't stopped)");
                    }
                }
            }
        } else {
            self.evaluate_now(expr.trim());
        }
    }

    /// An evaluation's answer.
    pub(super) fn debug_evaluated(&mut self, id: i64, result: Result<(String, String, i64), String>) {
        let Some(to) = self.dbx.evals.remove(&id) else { return };
        let text = match &result {
            Ok((v, _, _)) => Ok(v.clone()),
            Err(e) => Err(e.clone()),
        };
        match to {
            EvalFor::Watch(i) => {
                if let Some(slot) = self.dbx.watch_vals.get_mut(i) {
                    *slot = Some(text);
                }
                self.show_debug_info();
            }
            EvalFor::Dialog => {
                if let Some(d) = self.dialog.as_mut().filter(|d| d.kind == "evaluate") {
                    d.set_value("result", &text.unwrap_or_else(|e| format!("Error: {e}")));
                }
                // (A Modify changes what the variables show.)
                self.eval_watches();
            }
            EvalFor::Immediate => {
                let out = match text {
                    Ok(v) if v.is_empty() => String::new(),
                    Ok(v) => format!("{v}\n"),
                    Err(e) => format!("Error: {e}\n"),
                };
                self.immediate_append(&out);
                self.eval_watches();
            }
        }
    }

    // ---- breakpoint conditions ----------------------------------------------------------------

    pub(super) fn breakpoint_dialog(&mut self) {
        let Some(w) = self.windows.last() else { return };
        let Some(path) = w.ed.doc.path.clone().map(|p| Self::canon(&p)) else {
            self.say("Save the file first to set breakpoints in it");
            return;
        };
        let line = w.ed.cursor_line_col().0 as u32 + 1;
        let o = self.dbx.bp_opts.get(&path).and_then(|m| m.get(&line)).cloned().unwrap_or_default();
        let d = Dialog::new(&format!("bp_opts:{line}\u{1f}{}", path.display()), &format!("Breakpoint at Line {line}"))
            .body(ui::col(vec![
                ui::input("condition", "~C~ondition (stop when true):", &o.condition).chars(44),
                ui::input("hits", "~H~it count (stop on hit N, or >N, %N):", &o.hit_condition).chars(44),
                ui::input("log", "~L~og message (log instead of stopping; {expr} is evaluated):", &o.log_message).chars(44),
            ]))
            .buttons(vec![ui::button("ok", "O~K~", true), ui::button("remove", "~R~emove", false), ui::button("cancel", "Cancel", false)]);
        self.dialog = Some(d);
    }

    pub(super) fn breakpoint_done(&mut self, d: &Dialog, b: &str, rest: &str) {
        let Some((line, path)) = rest.split_once('\u{1f}') else { return };
        let (Ok(line), path) = (line.parse::<u32>(), PathBuf::from(path)) else { return };
        let remove = b == "remove";
        let o = if remove {
            BreakpointOpts::default()
        } else {
            BreakpointOpts { condition: d.value("condition").unwrap_or_default(), hit_condition: d.value("hits").unwrap_or_default(), log_message: d.value("log").unwrap_or_default() }
        };
        let m = self.dbx.bp_opts.entry(path.clone()).or_default();
        if o == BreakpointOpts::default() {
            m.remove(&line);
        } else {
            m.insert(line, o.clone());
        }
        let set = self.breakpoints.entry(path.clone()).or_default();
        if remove {
            set.remove(&line);
        } else {
            set.insert(line);
        }
        let lines = set.clone();
        if lines.is_empty() {
            self.breakpoints.remove(&path);
        }
        if let Some(dbg) = &mut self.debug {
            dbg.client.set_options(&path, line, o);
            dbg.client.set_breakpoints(&path, lines);
        }
    }

    // ---- the Immediate window -----------------------------------------------------------------

    pub(super) fn open_immediate(&mut self) {
        let i = self.special_window("immediate");
        if self.windows[i].ed.doc.rope.len_chars() == 0 {
            // (Empty: just a place to type.)
            self.windows[i].ed.doc.dirty = false;
        }
        let id = self.windows[i].win.id;
        self.layout_dock();
        self.shell.dock.show(crate::dock::Tab::Pane(id));
        self.activate_window(id);
        self.shell.focus = shell::Focus::Editors;
    }

    fn activate_window(&mut self, id: u32) {
        if let Some(i) = self.windows.iter().position(|w| w.win.id == id) {
            let w = self.windows.remove(i);
            self.windows.push(w);
            self.renumber();
        }
    }

    fn immediate_append(&mut self, text: &str) {
        let i = self.special_window("immediate");
        let ed = &mut self.windows[i].ed;
        let end = ed.doc.rope.len_chars();
        ed.doc.insert(end, text, ed.cursor);
        ed.doc.dirty = false;
        ed.cursor = ed.doc.rope.len_chars();
        ed.anchor = None;
    }

    /// Enter in the Immediate window: run the cursor's line in the stopped
    /// program and put the answer under it. True when it took the key.
    pub(super) fn immediate_key(&mut self, k: &KeyEvent) -> bool {
        let Some(w) = self.windows.last_mut().filter(|w| w.special == Some("immediate")) else { return false };
        if k.key != Key::Enter || k.mods.ctrl || k.mods.alt {
            return false;
        }
        let (line, _) = w.ed.cursor_line_col();
        let text = w.ed.doc.line(line).trim().to_string();
        // To the end, on a new line, as QBasic does.
        let end = w.ed.doc.rope.len_chars();
        w.ed.cursor = end;
        let ends_nl = end > 0 && w.ed.doc.rope.char(end - 1) == '\n';
        if line + 1 < w.ed.doc.len_lines() && !(line + 2 == w.ed.doc.len_lines() && ends_nl) {
            // (An old line run again: copied to the end first.)
            let t = format!("{}{text}\n", if ends_nl || end == 0 { "" } else { "\n" });
            w.ed.doc.insert(end, &t, end);
        } else if !ends_nl {
            w.ed.doc.insert(end, "\n", end);
        }
        w.ed.doc.dirty = false;
        w.ed.cursor = w.ed.doc.rope.len_chars();
        if text.is_empty() {
            return true;
        }
        match self.debug.as_mut().filter(|d| !d.client.running) {
            Some(d) => {
                let id = d.client.evaluate(&text, "repl");
                self.dbx.evals.insert(id, EvalFor::Immediate);
            }
            None => self.immediate_append("(Start debugging and stop the program to run code here)\n"),
        }
        true
    }

    // ---- what the views show -------------------------------------------------------------------

    /// The watches as text lines (the Debug window).
    pub(super) fn watch_lines(&self) -> Vec<String> {
        self.dbx
            .watches
            .iter()
            .zip(&self.dbx.watch_vals)
            .map(|(w, v)| match v {
                _ if self.debug.is_none() => format!("  {w}: <not available>"),
                Some(Ok(v)) => format!("  {w}: {v}"),
                Some(Err(e)) => format!("  {w}: <{e}>"),
                None => format!("  {w}: …"),
            })
            .collect()
    }
}
