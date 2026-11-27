//! The IDE itself: menu bar, desktop of editor windows, dialogs, status
//! bar, and the automation endpoint agents drive it through.

use crate::agent::{Driver, Output, Step};
use crate::vim::{self, Keymap};
use turbine_term::{MouseOutcome, Spawn, TerminalView};
use turbine_services::dap::{self, DapClient, DapEvent, Frame, Launch, Variable};
use turbine_services::lsp::{self, CompletionItem, LspClient, LspEvent, Pos};
use std::collections::{BTreeMap, BTreeSet};
use turbine_ui::editor::DiagMark;
use turbine_ui::popup::{ListPopup, PopupResult};
use turbine_vi::{ExAction, Out as VimOut, Session};
use std::collections::{HashMap, HashSet};

/// A language server is keyed by language and project folder.
type LspKey = (String, PathBuf);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LspReq {
    Hover,
    Definition,
}

/// An open (or about to open) completion list.
struct Completion {
    win_id: u32,
    /// Char offset where the word being completed starts.
    anchor: usize,
    line: usize,
    /// Waiting for this request's answer.
    waiting: Option<(LspKey, i64)>,
    items: Vec<CompletionItem>,
    /// Indexes into `items` that match what's typed.
    shown: Vec<usize>,
    popup: ListPopup,
}

/// Starts a debug adapter for a language: None when there is none.
pub type DapSpawner = Box<dyn Fn(&str, Launch, BTreeMap<PathBuf, BTreeSet<u32>>) -> Option<std::io::Result<DapClient>>>;

/// `TURBINE_DAP_<LANG>` ("fake" = the built-in test adapter), else the
/// first installed adapter for the language.
fn default_dap_spawner() -> DapSpawner {
    Box::new(|lang, launch, bps| {
        let var = format!("TURBINE_DAP_{}", lang.to_uppercase().replace('-', "_"));
        let argv: Vec<String> = match std::env::var(&var) {
            Ok(v) if v.trim() == "fake" => {
                return Some(turbine_services::fake::fake_debug_adapter().map(|rpc| DapClient::with_rpc(rpc, launch, bps)));
            }
            Ok(v) if !v.trim().is_empty() => v.split_whitespace().map(str::to_string).collect(),
            _ => dap::adapter_for(lang)?,
        };
        Some(DapClient::start(&argv, launch, bps))
    })
}

/// A debugging session.
struct Debug {
    client: DapClient,
    frames: Vec<Frame>,
    scopes: Vec<(String, Vec<Variable>)>,
    /// Breakpoint set by Go to Cursor, removed at the next stop.
    temp: Option<(PathBuf, u32)>,
}

/// Starts a language server for (language, project root): None when there
/// is none for the language.
pub type LspSpawner = Box<dyn Fn(&str, &Path) -> Option<std::io::Result<LspClient>>>;

/// The default: `TURBINE_LSP_<LANG>` ("fake" = the built-in test server),
/// else the first installed server for the language.
fn default_lsp_spawner() -> LspSpawner {
    Box::new(|lang, root| {
        let var = format!("TURBINE_LSP_{}", lang.to_uppercase().replace('-', "_"));
        let argv: Vec<String> = match std::env::var(&var) {
            Ok(v) if v.trim() == "fake" => {
                return Some(turbine_services::fake::fake_server().map(|rpc| LspClient::with_rpc(rpc, root, lang)));
            }
            Ok(v) if !v.trim().is_empty() => v.split_whitespace().map(str::to_string).collect(),
            _ => lsp::server_for(lang)?,
        };
        Some(LspClient::start(&argv, root, lang))
    })
}
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};
use turbine_text::Document;
use turbine_ui::automation::Node;
use turbine_ui::dialog::{Control, Dialog, DialogResult};
use turbine_ui::editor::EditorView;
use turbine_ui::event::{Button, Event, Key, KeyEvent, KeyKind, Mods, MouseEvent, MouseKind};
use turbine_ui::menu::{Menu, MenuBar, MenuItem, MenuResult};
use turbine_ui::statusbar::{StatusBar, StatusItem};
use turbine_ui::theme::{ControlStyle, Scheme, Style, Theme};
use turbine_ui::window::{Hit, Window};
use turbine_ui::{App, Point, Rect, Surface};

/// A request from the automation socket (or a script).
pub struct Request {
    pub cmd: Value,
    pub reply: Sender<Value>,
}

struct EdWin {
    win: Window,
    ed: EditorView,
    /// The window's vim state while the Vim keymap is on.
    vim: Option<Session>,
    /// Set for terminal windows (then `ed` is an unused empty editor).
    term: Option<TerminalView>,
    /// The terminal's screen as text, refreshed when it changes (for agents).
    term_text: String,
    /// Syntax: the language, what it was detected for, the version sent.
    lang: Option<String>,
    lang_for: Option<Option<PathBuf>>,
    hl_sent: Option<u64>,
    /// Language server: which one, the version it has, dirty last tick.
    lsp_key: Option<LspKey>,
    lsp_sent: Option<u64>,
    was_dirty: bool,
    /// "output" or "debug" for those windows.
    special: Option<&'static str>,
}

impl EdWin {
    fn is_terminal(&self) -> bool {
        self.term.is_some()
    }
}

pub struct Workbench {
    theme: Theme,
    menubar: MenuBar,
    status: StatusBar,
    /// Windows bottom to top; the last one is active.
    windows: Vec<EdWin>,
    dialog: Option<Dialog>,
    clipboard: String,
    size: (i32, i32),
    quit: bool,
    next_id: u32,
    message: Option<(String, Instant)>,
    driver: Driver,
    requests: Option<Receiver<Request>>,
    /// Window index being moved/resized with the mouse.
    dragging: Option<usize>,
    /// Editor (window index) selecting with the mouse.
    selecting: Option<usize>,
    last_click: Option<(Point, Instant)>,
    keymap: Keymap,
    /// True while applying an agent's input (it types literal text).
    from_agent: bool,
    /// Copied text for the system clipboard (the front end takes it).
    clipboard_out: Option<String>,
    highlighter: turbine_syntax::Highlighter,
    /// Highlight requests not answered yet.
    hl_pending: usize,
    lsp: HashMap<LspKey, LspClient>,
    /// Languages with no server (or whose server failed): not tried again.
    lsp_missing: HashSet<String>,
    lsp_spawn: LspSpawner,
    lsp_requests: HashMap<(LspKey, i64), (LspReq, u32)>,
    completion: Option<Completion>,
    /// Breakpoints: file (canonical) → lines, 1-based.
    breakpoints: BTreeMap<PathBuf, BTreeSet<u32>>,
    debug: Option<Debug>,
    dap_spawn: DapSpawner,
    /// What Run starts, per project: (program, arguments).
    run_config: HashMap<PathBuf, (PathBuf, Vec<String>)>,
}

/// The menu bar: Cursor's (VS Code's) menus and layout, merged with
/// Borland's (the ≡ system menu, Search, Run's compile/debug items, Options
/// and Window). Items whose feature isn't built yet are shown disabled so
/// the structure is complete.
fn menus() -> Vec<Menu> {
    let it = MenuItem::new;
    let sep = MenuItem::separator;
    let soon = |l: &str, k: &str, c: &str| MenuItem::new(l, k, c).disabled();
    vec![
        Menu::new(
            "≡",
            vec![
                it("~A~bout Turbine...", "", "help.about"),
                sep(),
                it("~R~epaint Desktop", "", "app.repaint"),
                soon("Show ~C~lipboard", "", "edit.show_clipboard"),
                soon("~C~alculator", "", "tools.calculator"),
            ],
        ),
        Menu::new(
            "~F~ile",
            vec![
                it("~N~ew Text File", "Ctrl+N", "file.new"),
                soon("New ~W~indow", "Ctrl+Shift+N", "file.new_window"),
                sep(),
                it("~O~pen File...", "F3", "file.open"),
                soon("Open ~F~older...", "", "file.open_folder"),
                soon("Open ~R~ecent", "", "file.recent"),
                sep(),
                it("~S~ave", "F2", "file.save"),
                it("Save ~A~s...", "", "file.save_as"),
                it("Save A~l~l", "", "file.save_all"),
                sep(),
                it("Re~v~ert File", "", "file.revert"),
                it("~C~lose Editor", "Alt+F3", "window.close"),
                sep(),
                it("~D~OS Shell", "", "view.terminal"),
                it("E~x~it", "Alt+X", "app.quit"),
            ],
        ),
        Menu::new(
            "~E~dit",
            vec![
                it("~U~ndo", "Ctrl+Z", "edit.undo"),
                it("~R~edo", "Ctrl+Y", "edit.redo"),
                sep(),
                it("Cu~t~", "Ctrl+X", "edit.cut"),
                it("~C~opy", "Ctrl+C", "edit.copy"),
                it("~P~aste", "Ctrl+V", "edit.paste"),
                sep(),
                soon("Toggle Line ~C~omment", "Ctrl+/", "edit.comment"),
                soon("~F~ormat Document", "Ctrl+Shift+I", "edit.format"),
                sep(),
                it("Trigger ~S~uggest", "Ctrl+Space", "lsp.complete"),
            ],
        ),
        Menu::new(
            "Se~l~ection",
            vec![
                it("Select ~A~ll", "Ctrl+A", "edit.select_all"),
                soon("~E~xpand Selection", "Shift+Alt+Right", "selection.expand"),
                soon("~S~hrink Selection", "Shift+Alt+Left", "selection.shrink"),
                sep(),
                soon("Copy Line ~U~p", "Shift+Alt+Up", "selection.copy_up"),
                soon("Copy Line ~D~own", "Shift+Alt+Down", "selection.copy_down"),
                soon("Mo~v~e Line Up", "Alt+Up", "selection.move_up"),
                soon("Move Line Do~w~n", "Alt+Down", "selection.move_down"),
                sep(),
                soon("Add Cursor A~b~ove", "Ctrl+Alt+Up", "selection.cursor_above"),
                soon("Add Cursor Be~l~ow", "Ctrl+Alt+Down", "selection.cursor_below"),
                soon("~C~olumn Selection Mode", "", "selection.column"),
            ],
        ),
        Menu::new(
            "~S~earch",
            vec![
                soon("~F~ind...", "Ctrl+F", "search.find"),
                soon("~R~eplace...", "Ctrl+H", "search.replace"),
                soon("~S~earch Again", "", "search.again"),
                soon("Find in F~i~les...", "Ctrl+Shift+F", "search.files"),
                sep(),
                it("~G~o to Line Number...", "Ctrl+G", "search.goto"),
                soon("Find ~P~rocedure...", "Ctrl+Shift+O", "go.symbol"),
                sep(),
                it("Pre~v~ious Error", "Alt+F7", "go.prev_problem"),
                it("~N~ext Error", "Alt+F8", "go.next_problem"),
            ],
        ),
        Menu::new(
            "~V~iew",
            vec![
                soon("~C~ommand Palette...", "Ctrl+Shift+P", "view.palette"),
                sep(),
                soon("~E~xplorer", "Ctrl+Shift+E", "view.files"),
                soon("~S~ource Control", "Ctrl+Shift+G", "view.scm"),
                soon("~R~un and Debug", "Ctrl+Shift+D", "view.debug"),
                soon("E~x~tensions", "Ctrl+Shift+X", "view.extensions"),
                sep(),
                soon("~P~roblems", "Ctrl+Shift+M", "view.problems"),
                soon("~O~utput", "", "view.output"),
                it("~T~erminal", "Ctrl+`", "view.terminal"),
                sep(),
                it("~L~ine Numbers", "", "view.line_numbers"),
                it("Right to Le~f~t", "", "view.rtl"),
                soon("~W~ord Wrap", "Alt+Z", "view.wrap"),
            ],
        ),
        Menu::new(
            "~G~o",
            vec![
                soon("~B~ack", "Alt+Left", "go.back"),
                soon("~F~orward", "Alt+Right", "go.forward"),
                sep(),
                soon("Go to ~F~ile...", "Ctrl+P", "go.file"),
                soon("Go to ~S~ymbol...", "Ctrl+Shift+O", "go.symbol"),
                it("Go to ~D~efinition", "F12", "go.definition"),
                soon("Go to ~R~eferences", "Shift+F12", "go.references"),
                it("Go to ~L~ine...", "Ctrl+G", "search.goto"),
                soon("Go to ~B~racket", "Ctrl+Shift+\\", "go.bracket"),
            ],
        ),
        Menu::new(
            "~R~un",
            vec![
                it("~R~un", "Ctrl+F9", "run.run"),
                it("Start ~D~ebugging", "", "run.debug"),
                it("Program R~e~set", "Ctrl+F2", "run.reset"),
                it("~G~o to Cursor", "F4", "run.to_cursor"),
                it("~T~race Into", "F7", "run.step_into"),
                it("~S~tep Over", "F8", "run.step_over"),
                it("Step O~u~t", "", "run.step_out"),
                it("~P~ause", "", "run.pause"),
                it("~A~rguments...", "", "run.arguments"),
                sep(),
                it("Toggle ~B~reakpoint", "Ctrl+F8", "run.breakpoint"),
                soon("Add ~W~atch...", "Ctrl+F7", "run.watch"),
                soon("E~v~aluate/Modify...", "Ctrl+F4", "run.evaluate"),
                it("~C~all Stack", "Ctrl+F3", "run.stack"),
                sep(),
                soon("C~o~mpile", "Alt+F9", "build.compile"),
                soon("~M~ake", "F9", "build.make"),
                soon("Bui~l~d All", "", "build.all"),
            ],
        ),
        Menu::new(
            "~T~erminal",
            vec![
                it("~N~ew Terminal", "Ctrl+Shift+`", "terminal.new"),
                soon("~S~plit Terminal", "", "terminal.split"),
                sep(),
                soon("~R~un Task...", "", "terminal.task"),
                soon("Run ~B~uild Task...", "Ctrl+Shift+B", "terminal.build"),
            ],
        ),
        Menu::new(
            "~A~I",
            vec![
                soon("~C~hat", "Ctrl+L", "ai.chat"),
                soon("~I~nline Edit", "Ctrl+K", "ai.inline"),
                soon("~A~gent", "Ctrl+I", "ai.agent"),
                soon("~T~ab Completion", "", "ai.tab"),
                soon("~M~odels...", "", "ai.models"),
                sep(),
                it("Agent ~D~emo", "", "agent.demo"),
                it("~S~top Agent", "Esc", "agent.stop"),
            ],
        ),
        Menu::new(
            "~O~ptions",
            vec![
                it("~K~eymap...", "", "options.keymap"),
                it("~C~olors...", "", "options.colors"),
                it("Control ~S~tyle...", "", "options.style"),
                sep(),
                soon("~E~ditor...", "", "options.editor"),
                soon("~D~irectories...", "", "options.dirs"),
                soon("Se~t~tings File", "Ctrl+,", "options.settings"),
            ],
        ),
        Menu::new(
            "~W~indow",
            vec![
                it("~T~ile", "", "window.tile"),
                it("C~a~scade", "", "window.cascade"),
                sep(),
                it("~Z~oom", "F5", "window.zoom"),
                it("~N~ext", "F6", "window.next"),
                it("~P~revious", "Shift+F6", "window.prev"),
                it("~C~lose", "Alt+F3", "window.close"),
                it("C~l~ose All", "", "window.close_all"),
                sep(),
                it("~L~ist...", "Alt+0", "window.list"),
            ],
        ),
        Menu::new(
            "~H~elp",
            vec![
                soon("~C~ontents", "", "help.contents"),
                soon("~I~ndex", "Shift+F1", "help.index"),
                it("~T~opic Search", "Ctrl+F1", "lsp.hover"),
                sep(),
                it("~K~eyboard...", "F1", "help.keys"),
                it("~V~im Keys...", "", "help.vim"),
                sep(),
                it("~A~bout...", "", "help.about"),
            ],
        ),
    ]
}

const DEMO_CODE: &str = "fn main() {\n    let name = \"Turbine\";\n    for i in 1..=3 {\n        println!(\"{i}: hello from {name}!\");\n    }\n}\n";

impl Workbench {
    pub fn new(files: &[PathBuf]) -> Self {
        let mut wb = Workbench {
            theme: Theme::default(),
            menubar: MenuBar::new(menus()),
            status: StatusBar {
                items: vec![
                    StatusItem::new("F1", "Help", "help.keys"),
                    StatusItem::new("F2", "Save", "file.save"),
                    StatusItem::new("F3", "Open", "file.open"),
                    StatusItem::new("Alt+F3", "Close", "window.close"),
                    StatusItem::new("F5", "Zoom", "window.zoom"),
                    StatusItem::new("F6", "Next", "window.next"),
                    StatusItem::new("F10", "Menu", "menu"),
                ],
                ..Default::default()
            },
            windows: Vec::new(),
            dialog: None,
            clipboard: String::new(),
            size: (80, 25),
            quit: false,
            next_id: 1,
            message: None,
            driver: Driver::new(),
            requests: None,
            dragging: None,
            selecting: None,
            last_click: None,
            keymap: Keymap::Cua,
            from_agent: false,
            clipboard_out: None,
            highlighter: turbine_syntax::Highlighter::new(),
            hl_pending: 0,
            lsp: HashMap::new(),
            lsp_missing: HashSet::new(),
            lsp_spawn: default_lsp_spawner(),
            lsp_requests: HashMap::new(),
            completion: None,
            breakpoints: BTreeMap::new(),
            debug: None,
            dap_spawn: default_dap_spawner(),
            run_config: HashMap::new(),
        };
        for f in files {
            wb.open_path(f);
        }
        if wb.windows.is_empty() {
            wb.new_window(Document::new());
        }
        wb.sync_menu_checks();
        wb
    }

    pub fn set_keymap(&mut self, k: Keymap) {
        self.keymap = k;
        for w in &mut self.windows {
            match k {
                Keymap::Vim => {
                    if w.vim.is_none() {
                        w.vim = Some(vim::attach(&mut w.ed));
                    }
                }
                Keymap::Cua => {
                    w.vim = None;
                    vim::detach(&mut w.ed);
                }
            }
        }
    }

    pub fn set_requests(&mut self, rx: Receiver<Request>) {
        self.requests = Some(rx);
    }

    pub fn driver_mut(&mut self) -> &mut Driver {
        &mut self.driver
    }

    fn desktop(&self) -> Rect {
        Rect::new(0, 1, self.size.0, (self.size.1 - 2).max(1))
    }

    fn screen(&self) -> Rect {
        Rect::new(0, 0, self.size.0, self.size.1)
    }

    fn say(&mut self, m: impl Into<String>) {
        self.message = Some((m.into(), Instant::now()));
    }

    fn active(&mut self) -> Option<&mut EdWin> {
        self.windows.last_mut()
    }

    fn renumber(&mut self) {
        for (i, w) in self.windows.iter_mut().enumerate() {
            w.win.number = Some(i as u32 + 1);
        }
    }

    fn new_window(&mut self, doc: Document) {
        let d = self.desktop();
        let n = self.windows.len() as i32;
        let off = (n * 2) % (d.h / 2).max(1);
        let rect = Rect::new(d.x + off, d.y + off / 2, (d.w - off).max(20), (d.h - off / 2).max(6));
        let title = doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| doc.title());
        let mut win = Window::new(self.next_id, &title, rect);
        if self.windows.is_empty() {
            win.zoomed = false;
        }
        self.next_id += 1;
        let mut ew = EdWin { win, ed: EditorView::new(doc), vim: None, term: None, term_text: String::new(), lang: None, lang_for: None, hl_sent: None, lsp_key: None, lsp_sent: None, was_dirty: false, special: None };
        if self.keymap == Keymap::Vim {
            ew.vim = Some(vim::attach(&mut ew.ed));
        }
        self.windows.push(ew);
        self.renumber();
    }

    #[allow(dead_code)] // tests
    pub fn set_dap_spawner(&mut self, f: DapSpawner) {
        self.dap_spawn = f;
    }

    fn canon(p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    }

    /// The window that shows a special pane ("output", "debug"), made if needed.
    fn special_window(&mut self, kind: &'static str) -> usize {
        if let Some(i) = self.windows.iter().position(|w| w.special == Some(kind)) {
            return i;
        }
        let d = self.desktop();
        let (title, rect) = match kind {
            "output" => ("Output", Rect::new(d.x, d.bottom() - (d.h / 3).max(6), d.w / 2, (d.h / 3).max(6))),
            _ => ("Debug", Rect::new(d.x + d.w / 2, d.bottom() - (d.h / 3).max(6), d.w - d.w / 2, (d.h / 3).max(6))),
        };
        let mut win = Window::new(self.next_id, title, rect);
        win.min_h = 4;
        self.next_id += 1;
        let mut ed = EditorView::new(Document::new());
        ed.line_numbers = false;
        let mut w = EdWin {
            win,
            ed,
            vim: None,
            term: None,
            term_text: String::new(),
            lang: None,
            lang_for: Some(None),
            hl_sent: None,
            lsp_key: None,
            lsp_sent: None,
            was_dirty: false,
            special: Some(kind),
        };
        w.win.number = None;
        // Below the active window, so the code stays in front.
        let at = self.windows.len().saturating_sub(1);
        self.windows.insert(at, w);
        self.renumber();
        at
    }

    fn append_output(&mut self, text: &str) {
        let i = self.special_window("output");
        let ed = &mut self.windows[i].ed;
        let end = ed.doc.rope.len_chars();
        ed.doc.insert(end, text, end);
        ed.doc.dirty = false;
        ed.cursor = ed.doc.rope.len_chars();
    }

    fn show_debug_info(&mut self) {
        let Some(d) = &self.debug else { return };
        let mut t = String::from("Call stack\n");
        for f in &d.frames {
            let place = f.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            t.push_str(&format!("  {}  {}:{}\n", f.name, place, f.line));
        }
        for (scope, vars) in &d.scopes {
            t.push_str(&format!("\n{scope}\n"));
            for v in vars {
                let kind = if v.kind.is_empty() { String::new() } else { format!(": {}", v.kind) };
                t.push_str(&format!("  {}{kind} = {}\n", v.name, v.value));
            }
        }
        let i = self.special_window("debug");
        let ed = &mut self.windows[i].ed;
        ed.doc = Document::from_str(&t);
        ed.cursor = 0;
    }

    /// Guess what Run should start for the active file.
    fn guess_program(&self) -> Option<(PathBuf, PathBuf, String)> {
        let w = self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none() && w.ed.doc.path.is_some())?;
        let path = w.ed.doc.path.clone()?;
        let lang = w.lang.clone().unwrap_or_default();
        let root = lsp::project_root(&path);
        if let Some((p, _)) = self.run_config.get(&root) {
            return Some((root.clone(), p.clone(), lang));
        }
        let program = match lang.as_str() {
            "rust" => {
                let name = std::fs::read_to_string(root.join("Cargo.toml"))
                    .ok()
                    .and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("name").map(|r| r.trim_start_matches([' ', '=']).trim().trim_matches('"').to_string())))
                    .unwrap_or_else(|| "main".into());
                root.join("target/debug").join(name)
            }
            "c" | "cpp" => path.with_extension(""),
            _ => path.clone(),
        };
        Some((root, program, lang))
    }

    fn arguments_dialog(&mut self) {
        let Some((root, program, _)) = self.guess_program() else {
            self.say("Open a file of the program first");
            return;
        };
        let args = self.run_config.get(&root).map(|(_, a)| a.join(" ")).unwrap_or_default();
        let d = Dialog::new(&format!("arguments:{}", root.display()), "Program and Arguments", 60, 11)
            .input("program", 1, 2, 54, "~P~rogram to run:", &program.display().to_string())
            .input("args", 1, 5, 54, "~A~rguments:", &args)
            .button("ok", 16, 7, "O~K~", true)
            .button("cancel", 32, 7, "Cancel", false);
        self.dialog = Some(d);
    }

    fn start_debugging(&mut self, stop_on_entry: bool) {
        let Some((root, program, lang)) = self.guess_program() else {
            self.say("Open a file of the program to debug first");
            return;
        };
        let args = self.run_config.get(&root).map(|(_, a)| a.clone()).unwrap_or_default();
        let launch = Launch { program: program.clone(), args, cwd: root.clone(), stop_on_entry, extra: serde_json::Value::Null };
        match (self.dap_spawn)(&lang, launch, self.breakpoints.clone()) {
            None => self.say(format!("No debugger for {lang}: install lldb-dap, gdb, debugpy or dlv")),
            Some(Err(e)) => self.say(format!("Debugger didn't start: {e}")),
            Some(Ok(c)) => {
                self.say(format!("Debugging {} with {}", program.display(), c.name()));
                self.debug = Some(Debug { client: c, frames: Vec::new(), scopes: Vec::new(), temp: None });
                self.debug_layout();
            }
        }
    }

    fn debug_command(&mut self, cmd: &str) {
        let paused = self.debug.as_ref().is_some_and(|d| !d.client.running);
        match cmd {
            "run.run" if paused => self.debug.as_mut().unwrap().client.continue_(),
            "run.run" | "run.debug" if self.debug.is_none() => self.start_debugging(false),
            "run.step_into" if self.debug.is_none() => self.start_debugging(true),
            "run.step_into" if paused => self.debug.as_mut().unwrap().client.step_in(),
            "run.step_over" if self.debug.is_none() => self.start_debugging(true),
            "run.step_over" if paused => self.debug.as_mut().unwrap().client.step_over(),
            "run.step_out" if paused => self.debug.as_mut().unwrap().client.step_out(),
            "run.pause" => {
                if let Some(d) = &mut self.debug {
                    d.client.pause();
                }
            }
            "run.reset" => {
                if let Some(mut d) = self.debug.take() {
                    d.client.stop();
                    self.clear_exec_line();
                    self.say("Program reset");
                }
            }
            "run.to_cursor" => {
                let Some(w) = self.windows.last() else { return };
                let Some(path) = w.ed.doc.path.clone().map(|p| Self::canon(&p)) else { return };
                let line = w.ed.cursor_line_col().0 as u32 + 1;
                let added = self.breakpoints.entry(path.clone()).or_default().insert(line);
                let lines = self.breakpoints.get(&path).cloned().unwrap_or_default();
                match &mut self.debug {
                    Some(d) => {
                        d.client.set_breakpoints(&path, lines);
                        if added {
                            d.temp = Some((path, line));
                        }
                        if !d.client.running {
                            d.client.continue_();
                        }
                    }
                    None => {
                        self.start_debugging(false);
                        if let (Some(d), true) = (&mut self.debug, added) {
                            d.temp = Some((path, line));
                        }
                    }
                }
            }
            "run.breakpoint" => {
                let Some(w) = self.windows.last() else { return };
                let Some(path) = w.ed.doc.path.clone().map(|p| Self::canon(&p)) else {
                    self.say("Save the file first to set breakpoints in it");
                    return;
                };
                let line = w.ed.cursor_line_col().0 as u32 + 1;
                let set = self.breakpoints.entry(path.clone()).or_default();
                if !set.remove(&line) {
                    set.insert(line);
                }
                let lines = set.clone();
                if lines.is_empty() {
                    self.breakpoints.remove(&path);
                }
                if let Some(d) = &mut self.debug {
                    d.client.set_breakpoints(&path, lines);
                }
            }
            "run.stack" => {
                self.special_window("debug");
                self.show_debug_info();
            }
            _ if self.debug.as_ref().is_some_and(|d| d.client.running) => self.say("The program is running (Run > Pause stops it)"),
            _ => {}
        }
    }

    /// Output and Debug along the bottom, code windows above them.
    fn debug_layout(&mut self) {
        self.special_window("output");
        self.special_window("debug");
        let d = self.desktop();
        let panel_h = (d.h / 3).max(6).min(d.h - 6);
        let top = d.bottom() - panel_h;
        for w in &mut self.windows {
            match w.special {
                Some("output") => {
                    w.win.zoomed = false;
                    w.win.rect = Rect::new(d.x, top, d.w / 2, panel_h);
                }
                Some(_) => {
                    w.win.zoomed = false;
                    w.win.rect = Rect::new(d.x + d.w / 2, top, d.w - d.w / 2, panel_h);
                }
                None if w.win.rect.bottom() > top && w.term.is_none() => {
                    w.win.zoomed = false;
                    w.win.rect.y = w.win.rect.y.min(top - 6).max(d.y);
                    w.win.rect.h = (top - w.win.rect.y).max(6);
                }
                None => {}
            }
        }
    }

    fn clear_exec_line(&mut self) {
        for w in &mut self.windows {
            w.ed.exec_line = None;
        }
    }

    /// Breakpoint marks into editors, and the debugger's events.
    fn update_debug(&mut self) {
        for w in &mut self.windows {
            if w.term.is_some() || w.special.is_some() {
                continue;
            }
            let want = w.ed.doc.path.as_ref().map(|p| Self::canon(p)).and_then(|p| self.breakpoints.get(&p));
            w.ed.breakpoints = want.map(|s| s.iter().map(|l| *l as usize - 1).collect()).unwrap_or_default();
        }
        let Some(d) = &mut self.debug else { return };
        let events = d.client.poll();
        for e in events {
            match e {
                DapEvent::Stopped { reason, frames, .. } => {
                    let top = frames.first().cloned();
                    if let Some(d) = &mut self.debug {
                        d.frames = frames;
                        if let Some((p, l)) = d.temp.take() {
                            if let Some(set) = self.breakpoints.get_mut(&p) {
                                set.remove(&l);
                                let lines = set.clone();
                                d.client.set_breakpoints(&p, lines);
                            }
                        }
                    }
                    self.clear_exec_line();
                    if let Some(f) = top {
                        if let Some(p) = &f.path {
                            let target = Self::canon(p);
                            match self.windows.iter().position(|w| w.ed.doc.path.as_ref().is_some_and(|q| Self::canon(q) == target)) {
                                Some(i) => {
                                    let w = self.windows.remove(i);
                                    self.windows.push(w);
                                    self.renumber();
                                }
                                None => self.open_path(p),
                            }
                            if let Some(w) = self.windows.last_mut() {
                                let line = (f.line as usize).saturating_sub(1).min(w.ed.doc.len_lines().saturating_sub(1));
                                w.ed.exec_line = Some(line);
                                w.ed.cursor = w.ed.doc.rope.line_to_char(line);
                                w.ed.anchor = None;
                            }
                        }
                        self.say(format!("Stopped ({reason}) in {} at line {}", f.name, f.line));
                    }
                    self.show_debug_info();
                }
                DapEvent::Variables { scopes } => {
                    if let Some(d) = &mut self.debug {
                        d.scopes = scopes;
                    }
                    self.show_debug_info();
                }
                DapEvent::Continued => self.clear_exec_line(),
                DapEvent::Output { category, text } => {
                    if category != "telemetry" {
                        self.append_output(&text);
                    }
                }
                DapEvent::Breakpoint { line, verified: false, .. } => self.say(format!("Breakpoint at line {line} can't be set there")),
                DapEvent::Breakpoint { .. } | DapEvent::Log(_) => {}
                DapEvent::Failed(m) => self.say(format!("Debugger: {m}")),
                DapEvent::Terminated | DapEvent::Exited(_) => {
                    self.debug = None;
                    self.clear_exec_line();
                    self.say("Program finished");
                    break;
                }
            }
        }
    }

    #[allow(dead_code)] // used by tests; settings will choose servers later
    pub fn set_lsp_spawner(&mut self, f: LspSpawner) {
        self.lsp_spawn = f;
    }

    /// Start servers for open files, keep them in sync, and act on what
    /// they send.
    fn update_lsp(&mut self) {
        let mut started = Vec::new();
        for i in 0..self.windows.len() {
            let w = &self.windows[i];
            if w.term.is_some() {
                continue;
            }
            let (Some(path), Some(lang)) = (w.ed.doc.path.clone(), w.lang.clone()) else { continue };
            let key: LspKey = (lang.clone(), lsp::project_root(&path));
            if !self.lsp.contains_key(&key) && !self.lsp_missing.contains(&lang) {
                match (self.lsp_spawn)(&lang, &key.1) {
                    None => {
                        self.lsp_missing.insert(lang.clone());
                    }
                    Some(Err(e)) => {
                        self.lsp_missing.insert(lang.clone());
                        started.push(format!("Language server for {lang} didn't start: {e}"));
                    }
                    Some(Ok(c)) => {
                        started.push(format!("Started {} for {}", c.name(), key.1.display()));
                        self.lsp.insert(key.clone(), c);
                    }
                }
            }
            let w = &mut self.windows[i];
            let Some(c) = self.lsp.get_mut(&key) else { continue };
            let v = w.ed.doc.version;
            if !c.is_open(&path) {
                c.did_open(&path, v as i64, &w.ed.doc.rope.to_string());
            } else if w.lsp_sent != Some(v) {
                c.did_change(&path, v as i64, &w.ed.doc.rope.to_string());
            }
            if w.was_dirty && !w.ed.doc.dirty {
                c.did_save(&path);
            }
            w.was_dirty = w.ed.doc.dirty;
            w.lsp_sent = Some(v);
            w.lsp_key = Some(key);
        }
        for m in started {
            self.say(m);
        }
        let mut events = Vec::new();
        for (key, c) in self.lsp.iter_mut() {
            for e in c.poll() {
                events.push((key.clone(), e));
            }
        }
        for (key, e) in events {
            self.lsp_event(key, e);
        }
    }

    fn lsp_event(&mut self, key: LspKey, e: LspEvent) {
        match e {
            LspEvent::Ready | LspEvent::Log(_) => {}
            LspEvent::Diagnostics { path, items } => {
                let want = std::fs::canonicalize(&path).unwrap_or(path);
                for w in &mut self.windows {
                    let Some(p) = &w.ed.doc.path else { continue };
                    if std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) != want {
                        continue;
                    }
                    let doc = &w.ed.doc;
                    w.ed.diagnostics = items
                        .iter()
                        .map(|d| {
                            let line = d.start.line as usize;
                            let text = doc.line(line);
                            let start = lsp::byte_of_utf16(&text, d.start.character);
                            let end = if d.end.line as usize > line { text.len() } else { lsp::byte_of_utf16(&text, d.end.character) };
                            let msg = if d.source.is_empty() { d.message.clone() } else { format!("{}: {}", d.source, d.message) };
                            DiagMark { line, start, end, severity: d.severity, message: msg }
                        })
                        .collect();
                }
            }
            LspEvent::Completion { id, items } => {
                let Some(comp) = &mut self.completion else { return };
                if comp.waiting.as_ref() != Some(&(key, id)) {
                    return;
                }
                comp.waiting = None;
                comp.items = items;
                if comp.items.is_empty() {
                    self.completion = None;
                    self.say("No completions here");
                    return;
                }
                self.refilter_completion();
            }
            LspEvent::Hover { id, text } => {
                if self.lsp_requests.remove(&(key, id)).is_none() {
                    return;
                }
                if text.trim().is_empty() {
                    self.say("Nothing to show here");
                    return;
                }
                let lines: Vec<String> = text.lines().filter(|l| !l.starts_with("```")).map(str::to_string).collect();
                let w = (lines.iter().map(|l| turbine_text::unicode::str_width(l)).max().unwrap_or(10) as i32 + 6).clamp(30, self.size.0 - 4);
                let h = (lines.len() as i32 + 6).clamp(8, self.size.1 - 4);
                let d = Dialog::new("hover", "Topic", w, h)
                    .list("text", Rect::new(0, 0, w - 4, h - 5), lines, 0)
                    .button("ok", (w - 10) / 2, h - 4, "O~K~", true);
                self.dialog = Some(d);
            }
            LspEvent::Definition { id, locations } => {
                if self.lsp_requests.remove(&(key, id)).is_none() {
                    return;
                }
                let Some(loc) = locations.into_iter().next() else {
                    self.say("No definition found");
                    return;
                };
                let same = self.windows.last().and_then(|w| w.ed.doc.path.clone()).map(|p| std::fs::canonicalize(&p).unwrap_or(p))
                    == Some(std::fs::canonicalize(&loc.path).unwrap_or(loc.path.clone()));
                if !same {
                    if let Some(i) = self.windows.iter().position(|w| {
                        w.ed.doc.path.as_ref().is_some_and(|p| std::fs::canonicalize(p).ok() == std::fs::canonicalize(&loc.path).ok())
                    }) {
                        let w = self.windows.remove(i);
                        self.windows.push(w);
                        self.renumber();
                    } else {
                        self.open_path(&loc.path);
                    }
                }
                if let Some(w) = self.windows.last_mut() {
                    let line = loc.start.line as usize;
                    let text = w.ed.doc.line(line);
                    let b = lsp::byte_of_utf16(&text, loc.start.character);
                    w.ed.cursor = w.ed.doc.line_byte_to_char(line, b);
                    w.ed.anchor = None;
                }
            }
            LspEvent::Failed { id, message } => {
                let ours = self.lsp_requests.remove(&(key.clone(), id)).is_some()
                    || self.completion.as_ref().is_some_and(|c| c.waiting.as_ref() == Some(&(key.clone(), id)));
                if ours {
                    if self.completion.as_ref().is_some_and(|c| c.waiting.is_some()) {
                        self.completion = None;
                    }
                    self.say(format!("Language server: {message}"));
                }
            }
            LspEvent::Exited(why) => {
                if let Some(c) = self.lsp.remove(&key) {
                    self.say(format!("{} stopped: {why}", c.name()));
                }
                self.lsp_missing.insert(key.0);
            }
        }
    }

    /// The active editor's server and the cursor's position for it.
    fn lsp_here(&mut self) -> Option<(LspKey, PathBuf, Pos)> {
        let w = self.windows.last()?;
        let key = w.lsp_key.clone()?;
        let path = w.ed.doc.path.clone()?;
        let (line, byte) = w.ed.doc.char_to_line_byte(w.ed.cursor);
        let text = w.ed.doc.line(line);
        Some((key, path, Pos { line: line as u32, character: lsp::utf16_col(&text, byte) }))
    }

    fn lsp_request(&mut self, what: LspReq) {
        let Some((key, path, pos)) = self.lsp_here() else {
            self.say("No language server for this file");
            return;
        };
        let Some(c) = self.lsp.get_mut(&key) else { return };
        let id = match what {
            LspReq::Hover => c.hover(&path, pos),
            LspReq::Definition => c.definition(&path, pos),
        };
        let win = self.windows.last().map(|w| w.win.id).unwrap_or(0);
        self.lsp_requests.insert((key, id), (what, win));
    }

    fn start_completion(&mut self) {
        let Some((key, path, pos)) = self.lsp_here() else {
            self.say("No language server for this file");
            return;
        };
        let w = self.windows.last().unwrap();
        let rope = &w.ed.doc.rope;
        let mut anchor = w.ed.cursor;
        while anchor > 0 {
            let c = rope.char(anchor - 1);
            if c.is_alphanumeric() || c == '_' {
                anchor -= 1;
            } else {
                break;
            }
        }
        let (line, _) = w.ed.doc.char_to_line_byte(w.ed.cursor);
        let win_id = w.win.id;
        let Some(c) = self.lsp.get_mut(&key) else { return };
        let id = c.completion(&path, pos);
        self.completion = Some(Completion {
            win_id,
            anchor,
            line,
            waiting: Some((key, id)),
            items: Vec::new(),
            shown: Vec::new(),
            popup: ListPopup::new(Vec::new()),
        });
    }

    /// Match the list to what's typed since it opened; close it when the
    /// cursor leaves the word.
    fn refilter_completion(&mut self) {
        let Some(comp) = &mut self.completion else { return };
        let Some(w) = self.windows.last() else {
            self.completion = None;
            return;
        };
        let (line, _) = w.ed.doc.char_to_line_byte(w.ed.cursor);
        if w.win.id != comp.win_id || line != comp.line || w.ed.cursor < comp.anchor {
            self.completion = None;
            return;
        }
        let typed: String = w.ed.doc.rope.slice(comp.anchor..w.ed.cursor).to_string();
        if typed.chars().any(|c| !(c.is_alphanumeric() || c == '_')) {
            self.completion = None;
            return;
        }
        if comp.waiting.is_some() {
            return;
        }
        let t = typed.to_lowercase();
        let mut shown: Vec<usize> = (0..comp.items.len()).filter(|&i| comp.items[i].label.to_lowercase().starts_with(&t)).collect();
        if shown.is_empty() {
            shown = (0..comp.items.len()).filter(|&i| comp.items[i].label.to_lowercase().contains(&t)).collect();
        }
        if shown.is_empty() {
            self.completion = None;
            return;
        }
        let sel_label = comp.shown.get(comp.popup.sel).map(|&i| comp.items[i].label.clone());
        comp.popup.items = shown.iter().map(|&i| (comp.items[i].label.clone(), comp.items[i].detail.clone())).collect();
        comp.popup.sel = sel_label.and_then(|l| shown.iter().position(|&i| comp.items[i].label == l)).unwrap_or(0);
        comp.shown = shown;
    }

    fn accept_completion(&mut self, i: usize) {
        let Some(comp) = self.completion.take() else { return };
        let Some(&idx) = comp.shown.get(i) else { return };
        let insert = comp.items[idx].insert.clone();
        let Some(w) = self.windows.last_mut() else { return };
        let cur = w.ed.cursor;
        w.ed.doc.break_undo_group();
        w.ed.doc.remove(comp.anchor, cur, cur);
        w.ed.doc.insert(comp.anchor, &insert, comp.anchor);
        w.ed.doc.break_undo_group();
        w.ed.cursor = comp.anchor + insert.chars().count();
        w.ed.anchor = None;
    }

    /// Detect languages, send changed documents to the highlighter, and
    /// take its answers.
    fn update_highlights(&mut self) {
        for w in &mut self.windows {
            if w.term.is_some() {
                continue;
            }
            let path = w.ed.doc.path.clone();
            // Untitled documents are checked again as they're typed (a #! line).
            let stale = w.lang_for.as_ref() != Some(&path) || (path.is_none() && w.hl_sent != Some(w.ed.doc.version));
            if stale {
                let lang = turbine_syntax::detect(path.as_deref(), &w.ed.doc.line(0));
                if lang != w.lang {
                    w.lang = lang;
                    w.ed.highlights.clear();
                    w.hl_sent = None;
                }
                w.lang_for = Some(path);
            }
            if let Some(lang) = &w.lang {
                if w.hl_sent != Some(w.ed.doc.version) {
                    self.highlighter.request(w.win.id as u64, w.ed.doc.version, lang, w.ed.doc.rope.to_string());
                    w.hl_sent = Some(w.ed.doc.version);
                    self.hl_pending += 1;
                }
            } else if w.lang_for.is_some() {
                w.hl_sent = Some(w.ed.doc.version);
            }
        }
        for r in self.highlighter.results() {
            self.hl_pending = self.hl_pending.saturating_sub(1);
            if let Some(w) = self.windows.iter_mut().find(|w| w.win.id as u64 == r.id) {
                w.ed.highlights = r.lines.unwrap_or_default();
            }
        }
    }

    /// Put text on Turbine's clipboard and the system's.
    fn copy(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.clipboard_out = Some(text.clone());
        self.clipboard = text;
    }

    /// A terminal window running the user's shell (or `argv`).
    fn new_terminal(&mut self, argv: Vec<String>) {
        let d = self.desktop();
        // The lower part of the desktop, like VS Code's panel.
        let h = (d.h * 2 / 5).max(8).min(d.h);
        let rect = Rect::new(d.x, d.bottom() - h, d.w, h);
        let mut win = Window::new(self.next_id, "Terminal", rect);
        win.min_h = 4;
        self.next_id += 1;
        let view = TerminalView::new(Spawn { argv, ..Default::default() });
        if let Some(e) = view.error() {
            self.say(format!("Terminal: {e}"));
        }
        self.windows.push(EdWin { win, ed: EditorView::new(Document::new()), vim: None, term: Some(view), term_text: String::new(), lang: None, lang_for: None, hl_sent: None, lsp_key: None, lsp_sent: None, was_dirty: false, special: None });
        self.renumber();
    }

    fn open_path(&mut self, p: &Path) {
        if !p.exists() {
            let mut d = Document::new();
            d.path = Some(p.to_path_buf());
            self.new_window(d);
            self.say(format!("New file {}", p.display()));
            return;
        }
        match Document::open(p) {
            Ok(d) => {
                let note = if d.lossy { " (some bytes could not be decoded)" } else { "" };
                let msg = format!("Opened {} as {}{note}", p.display(), d.encoding.name());
                // Replace an untouched empty window.
                if self.windows.len() == 1 && self.windows[0].ed.doc.path.is_none() && !self.windows[0].ed.doc.dirty {
                    self.windows.clear();
                }
                self.new_window(d);
                self.say(msg);
            }
            Err(e) => self.say(format!("Can't open {}: {e}", p.display())),
        }
    }

    fn sync_menu_checks(&mut self) {
        let (ln, rtl) = self
            .windows
            .last()
            .map(|w| (w.ed.line_numbers, w.ed.base_rtl == Some(true)))
            .unwrap_or((true, false));
        for m in &mut self.menubar.menus {
            for it in &mut m.items {
                match it.command.as_str() {
                    "view.line_numbers" => it.checked = Some(ln),
                    "view.rtl" => it.checked = Some(rtl),
                    _ => {}
                }
            }
        }
    }

    // ----- commands ---------------------------------------------------------

    pub fn run_command(&mut self, cmd: &str) {
        let desk = self.desktop();
        match cmd {
            "file.new" => self.new_window(Document::new()),
            "view.terminal" | "terminal.new" => self.new_terminal(Vec::new()),
            "file.open" => self.open_dialog(std::env::current_dir().unwrap_or_default()),
            "file.save" => self.save(false),
            "file.save_as" => self.save(true),
            "app.quit" => {
                if self.windows.iter().any(|w| w.ed.doc.dirty) {
                    self.unsaved_dialog("quit");
                } else {
                    self.quit = true;
                }
            }
            "edit.undo" | "edit.redo" | "edit.cut" | "edit.copy" | "edit.paste" | "edit.select_all" => {
                let k = match cmd {
                    "edit.undo" => 'z',
                    "edit.redo" => 'y',
                    "edit.cut" => 'x',
                    "edit.copy" => 'c',
                    "edit.paste" => 'v',
                    _ => 'a',
                };
                if let Some(t) = self.windows.last_mut().and_then(|w| w.term.as_mut()) {
                    match k {
                        'c' | 'x' => {
                            if let Some(s) = t.selected_text() {
                                self.copy(s);
                            }
                        }
                        'v' => {
                            let c = self.clipboard.clone();
                            t.paste(&c);
                        }
                        'a' => t.select_all(),
                        _ => {}
                    }
                    return;
                }
                let before = self.clipboard.clone();
                let mut clip = std::mem::take(&mut self.clipboard);
                if let Some(w) = self.active() {
                    w.ed.handle_key(&KeyEvent::new(Key::Char(k), Mods::CTRL), &mut clip);
                }
                if clip != before {
                    self.copy(clip);
                } else {
                    self.clipboard = clip;
                }
            }
            "lsp.complete" => self.start_completion(),
            "run.run" | "run.debug" | "run.step_into" | "run.step_over" | "run.step_out" | "run.pause" | "run.reset"
            | "run.to_cursor" | "run.breakpoint" | "run.stack" => self.debug_command(cmd),
            "run.arguments" => self.arguments_dialog(),
            "lsp.hover" => self.lsp_request(LspReq::Hover),
            "go.definition" => self.lsp_request(LspReq::Definition),
            "go.next_problem" | "go.prev_problem" => {
                let fwd = cmd == "go.next_problem";
                let msg = self.windows.last_mut().and_then(|w| w.ed.goto_diagnostic(fwd));
                self.say(msg.unwrap_or_else(|| "No problems".into()));
            }
            "search.goto" => {
                let d = Dialog::new("goto", "Go to Line", 36, 8)
                    .input("line", 1, 2, 30, "~L~ine number:", "")
                    .button("ok", 6, 4, "O~K~", true)
                    .button("cancel", 18, 4, "Cancel", false);
                self.dialog = Some(d);
            }
            "view.line_numbers" => {
                if let Some(w) = self.active() {
                    w.ed.line_numbers = !w.ed.line_numbers;
                }
            }
            "view.rtl" => {
                if let Some(w) = self.active() {
                    w.ed.base_rtl = if w.ed.base_rtl == Some(true) { None } else { Some(true) };
                }
            }
            "options.colors" => {
                let names: Vec<String> = Scheme::all().iter().map(|s| s.name.to_string()).collect();
                let cur = names.iter().position(|n| n == self.theme.scheme.name).unwrap_or(0);
                let d = Dialog::new("colors", "Colors", 40, 12)
                    .label(1, 0, "Color scheme:")
                    .list("scheme", Rect::new(1, 1, 22, 7), names, cur)
                    .button("ok", 25, 2, "O~K~", true)
                    .button("cancel", 25, 5, "Cancel", false);
                self.dialog = Some(d);
            }
            "options.style" => {
                let names: Vec<String> = ControlStyle::all().iter().map(|s| s.name.to_string()).collect();
                let cur = names.iter().position(|n| n == self.theme.style.name).unwrap_or(0);
                let d = Dialog::new("style", "Control Style", 40, 12)
                    .label(1, 0, "Draw controls like:")
                    .list("style", Rect::new(1, 1, 22, 7), names, cur)
                    .button("ok", 25, 2, "O~K~", true)
                    .button("cancel", 25, 5, "Cancel", false);
                self.dialog = Some(d);
            }
            "window.tile" => self.tile(),
            "window.cascade" => {
                for (i, w) in self.windows.iter_mut().enumerate() {
                    let o = i as i32;
                    w.win.zoomed = false;
                    w.win.rect = Rect::new(desk.x + o, desk.y + o, (desk.w - o).max(20), (desk.h - o).max(6));
                }
            }
            "window.prev" => {
                if self.windows.len() > 1 {
                    let w = self.windows.pop().unwrap();
                    self.windows.insert(0, w);
                    self.renumber();
                }
            }
            "window.close_all" => {
                self.windows.retain(|w| w.ed.doc.dirty);
                self.renumber();
                if !self.windows.is_empty() {
                    self.say("Windows with unsaved changes were left open");
                }
            }
            "window.list" => {
                let items: Vec<String> = self
                    .windows
                    .iter()
                    .rev()
                    .map(|w| format!("{}{}", w.win.title, if w.ed.doc.dirty { " *" } else { "" }))
                    .collect();
                if items.is_empty() {
                    return;
                }
                let d = Dialog::new("winlist", "Window List", 56, 14)
                    .list("windows", Rect::new(1, 1, 38, 9), items, 0)
                    .button("ok", 41, 1, "~G~o to", true)
                    .button("cancel", 41, 4, "Cancel", false);
                self.dialog = Some(d);
            }
            "file.save_all" => self.vim_actions(vec![ExAction::WriteAll(false)]),
            "file.revert" => self.vim_actions(vec![ExAction::EditFile(String::new(), true)]),
            "app.repaint" => {}
            "options.keymap" => {
                let names = vec![Keymap::Cua.name().to_string(), Keymap::Vim.name().to_string()];
                let cur = if self.keymap == Keymap::Vim { 1 } else { 0 };
                let d = Dialog::new("keymap", "Keymap", 40, 10)
                    .label(1, 0, "Editor keys:")
                    .list("keymap", Rect::new(1, 1, 22, 5), names, cur)
                    .button("ok", 25, 1, "O~K~", true)
                    .button("cancel", 25, 4, "Cancel", false);
                self.dialog = Some(d);
            }
            "help.vim" => {
                let mut items = Vec::new();
                for (section, keys) in turbine_vi::vi::Vi::help() {
                    items.push(format!("── {section} ──"));
                    for (k, what) in keys {
                        items.push(format!("{k:<14} {what}"));
                    }
                }
                items.push("── Turbine ──".into());
                for (k, what) in [
                    ("q{reg} … q", "record a macro"),
                    ("[n]@{reg} @@", "play a macro (@: repeats the last :)"),
                    ("Ctrl-W w/W", "next / previous window"),
                    ("Ctrl-W h/j/k/l", "window to the left/below/above/right"),
                    ("Ctrl-W q / o / n", "close / only this / new window"),
                    (":e :w :q :wq :x", "files and windows, as vim"),
                    (":sp :vs {file}", "open a file beside this one"),
                    (":bn :bp :new :only", "windows as buffers"),
                ] {
                    items.push(format!("{k:<14} {what}"));
                }
                let h = (self.size.1 - 4).clamp(10, 30);
                let d = Dialog::new("vimhelp", "Vim Keys", 72.min(self.size.0 - 2), h)
                    .list("keys", Rect::new(0, 0, 66.min(self.size.0 - 8), h - 5), items, 0)
                    .button("ok", 28, h - 4, "O~K~", true);
                self.dialog = Some(d);
            }
            "window.next" => {
                if self.windows.len() > 1 {
                    let w = self.windows.remove(0);
                    self.windows.push(w);
                }
            }
            "window.zoom" => {
                if let Some(w) = self.active() {
                    w.win.toggle_zoom(desk);
                }
            }
            "window.close" => {
                if self.windows.last().is_some_and(|w| w.ed.doc.dirty) {
                    self.unsaved_dialog("close");
                } else {
                    self.windows.pop();
                    self.renumber();
                }
            }
            "help.about" => {
                let d = Dialog::new("about", "About", 46, 11)
                    .label(15, 1, "T U R B I N E")
                    .label(2, 3, "A retro, AI-first code editor and IDE")
                    .label(2, 4, &format!("Version {}  -  MIT License", env!("CARGO_PKG_VERSION")))
                    .label(2, 5, "github.com/benwbooth/turbine")
                    .button("ok", 17, 7, "O~K~", true);
                self.dialog = Some(d);
            }
            "help.keys" => {
                let lines = [
                    "F10 / Alt+letter   Menus",
                    "F2  Save    F3  Open    Ctrl+N  New",
                    "F5  Zoom    F6  Next    Alt+F3  Close",
                    "Alt+1..9    Go to window",
                    "Shift+arrows select; Ctrl+X/C/V clipboard",
                    "Ctrl+Z/Y undo/redo; Ins overwrite",
                    "Drag title to move, edges to resize",
                    "Agents: Esc stops, Space pauses/resumes",
                ];
                let mut d = Dialog::new("keys", "Keyboard", 50, lines.len() as i32 + 6);
                for (i, l) in lines.iter().enumerate() {
                    d = d.label(1, i as i32 + 1, l);
                }
                d = d.button("ok", 19, lines.len() as i32 + 2, "O~K~", true);
                self.dialog = Some(d);
            }
            "agent.demo" => self.queue_demo(),
            "agent.stop" => {
                self.driver.cancel();
                self.say("Agent stopped");
            }
            "menu" => {
                self.menubar.active = true;
                self.menubar.title_sel = 0;
            }
            other => self.say(format!("{other}: not available yet")),
        }
        self.sync_menu_checks();
    }

    fn tile(&mut self) {
        let d = self.desktop();
        let n = self.windows.len() as i32;
        if n == 0 {
            return;
        }
        let cols = (n as f64).sqrt().ceil() as i32;
        let rows = (n + cols - 1) / cols;
        for (i, w) in self.windows.iter_mut().enumerate() {
            let i = i as i32;
            let (c, r) = (i % cols, i / cols);
            let x0 = d.x + d.w * c / cols;
            let x1 = d.x + d.w * (c + 1) / cols;
            let y0 = d.y + d.h * r / rows;
            let y1 = d.y + d.h * (r + 1) / rows;
            w.win.zoomed = false;
            w.win.rect = Rect::new(x0, y0, x1 - x0, y1 - y0);
        }
    }

    fn save(&mut self, ask: bool) {
        let Some(w) = self.windows.last_mut() else { return };
        if ask || w.ed.doc.path.is_none() {
            let cur = w.ed.doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            let d = Dialog::new("save_as", "Save As", 52, 8)
                .input("path", 1, 2, 46, "File ~n~ame:", &cur)
                .button("ok", 12, 4, "O~K~", true)
                .button("cancel", 28, 4, "Cancel", false);
            self.dialog = Some(d);
            return;
        }
        let r = w.ed.doc.save();
        let msg = match r {
            Ok(()) => format!("Saved {} ({}, {})", w.ed.doc.title(), w.ed.doc.encoding.name(), w.ed.doc.line_ending.name()),
            Err(e) => format!("Save failed: {e}"),
        };
        self.say(msg);
    }

    fn open_dialog(&mut self, dir: PathBuf) {
        let mut items: Vec<String> = Vec::new();
        if dir.parent().is_some() {
            items.push("../".into());
        }
        let mut entries: Vec<(bool, String)> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| {
                        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        (!is_dir, e.file_name().to_string_lossy().into_owned())
                    })
                    .filter(|(_, n)| !n.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        entries.sort();
        items.extend(entries.into_iter().map(|(f, n)| if f { n } else { format!("{n}/") }));
        let title = format!("Open - {}", dir.display());
        let d = Dialog::new(&format!("open:{}", dir.display()), &title, 60, 18)
            .input("path", 1, 2, 40, "File ~n~ame:", "")
            .label(1, 4, "Files:")
            .list("files", Rect::new(1, 5, 40, 10), items, 0)
            .button("ok", 44, 2, "~O~pen", true)
            .button("cancel", 44, 5, "Cancel", false);
        self.dialog = Some(d);
    }

    fn unsaved_dialog(&mut self, then: &str) {
        let name = self.windows.iter().rev().find(|w| w.ed.doc.dirty).map(|w| w.ed.doc.title()).unwrap_or_default();
        let d = Dialog::new(&format!("unsaved:{then}"), "Unsaved Changes", 50, 8)
            .label(1, 1, &format!("{name} has unsaved changes."))
            .button("save", 2, 3, "~S~ave", true)
            .button("discard", 15, 3, "~D~on't Save", false)
            .button("cancel", 33, 3, "Cancel", false);
        self.dialog = Some(d);
    }

    fn finish_dialog(&mut self, d: Dialog, button: Option<String>) {
        let Some(b) = button else { return };
        if b == "cancel" {
            return;
        }
        let kind = d.kind.clone();
        if let Some(root) = kind.strip_prefix("arguments:") {
            let program = PathBuf::from(d.value("program").unwrap_or_default().trim());
            let args: Vec<String> = d.value("args").unwrap_or_default().split_whitespace().map(str::to_string).collect();
            self.run_config.insert(PathBuf::from(root), (program, args));
            return;
        }
        if let Some(dir) = kind.strip_prefix("open:") {
            let dir = PathBuf::from(dir);
            let typed = d.value("path").unwrap_or_default();
            let pick = if typed.trim().is_empty() { d.value("files").unwrap_or_default() } else { typed };
            let p = if Path::new(&pick).is_absolute() { PathBuf::from(&pick) } else { dir.join(&pick) };
            if pick.ends_with('/') || p.is_dir() {
                let p = if pick == "../" { dir.parent().map(Path::to_path_buf).unwrap_or(dir) } else { p };
                self.open_dialog(p);
            } else if !pick.is_empty() {
                self.open_path(&p);
            }
            return;
        }
        match kind.as_str() {
            "save_as" => {
                let p = d.value("path").unwrap_or_default();
                if p.trim().is_empty() {
                    return;
                }
                let p = PathBuf::from(p.trim());
                if let Some(w) = self.windows.last_mut() {
                    let r = w.ed.doc.save_as(&p);
                    w.win.title = p.display().to_string();
                    let msg = match r {
                        Ok(()) => format!("Saved {}", p.display()),
                        Err(e) => format!("Save failed: {e}"),
                    };
                    self.say(msg);
                }
            }
            "goto" => {
                if let Ok(n) = d.value("line").unwrap_or_default().trim().parse::<usize>() {
                    if let Some(w) = self.active() {
                        let line = n.saturating_sub(1).min(w.ed.doc.len_lines().saturating_sub(1));
                        w.ed.cursor = w.ed.doc.rope.line_to_char(line);
                        w.ed.anchor = None;
                    }
                }
            }
            "colors" => {
                if let Some(i) = d.selected_index("scheme") {
                    if let Some(s) = Scheme::all().into_iter().nth(i) {
                        self.theme.scheme = s;
                    }
                }
            }
            "keymap" => {
                let k = if d.selected_index("keymap") == Some(1) { Keymap::Vim } else { Keymap::Cua };
                self.set_keymap(k);
                self.say(format!("Keymap: {}", k.name()));
            }
            "winlist" => {
                if let Some(i) = d.selected_index("windows") {
                    let n = self.windows.len();
                    if i < n {
                        let w = self.windows.remove(n - 1 - i);
                        self.windows.push(w);
                        self.renumber();
                    }
                }
            }
            "style" => {
                if let Some(i) = d.selected_index("style") {
                    if let Some(s) = ControlStyle::all().into_iter().nth(i) {
                        self.theme.style = s;
                    }
                }
            }
            k if k.starts_with("unsaved:") => {
                let then = &k["unsaved:".len()..];
                let idx = self.windows.iter().rposition(|w| w.ed.doc.dirty);
                match b.as_str() {
                    "save" => {
                        if let Some(i) = idx {
                            let w = self.windows.remove(i);
                            self.windows.push(w);
                            if self.windows.last().unwrap().ed.doc.path.is_none() {
                                self.save(true);
                                return;
                            }
                            self.save(false);
                        }
                    }
                    "discard" => {
                        if let Some(i) = idx {
                            self.windows[i].ed.doc.dirty = false;
                        }
                    }
                    _ => return,
                }
                if then == "close" {
                    self.windows.pop();
                    self.renumber();
                } else if then == "quit" {
                    self.run_command("app.quit");
                }
            }
            _ => {}
        }
    }

    // ----- input ------------------------------------------------------------

    /// Input from the person at the keyboard: pauses a running agent.
    fn human_input(&mut self, ev: &Event) {
        if !self.driver.busy() {
            return;
        }
        match ev {
            Event::Key(k) if k.key == Key::Esc => {
                self.driver.cancel();
                self.say("Agent stopped");
            }
            Event::Key(k) if k.key == Key::Char(' ') && k.mods == Mods::NONE => {
                self.driver.paused = !self.driver.paused;
            }
            Event::Key(_) | Event::Mouse(MouseEvent { kind: MouseKind::Down(_), .. }) => {
                self.driver.paused = true;
            }
            _ => {}
        }
    }

    fn dispatch(&mut self, ev: Event) {
        match ev {
            Event::Resize(w, h) => {
                self.size = (w, h);
                self.menubar.width = w;
                let d = self.desktop();
                for w in &mut self.windows {
                    w.win.fit(d);
                }
            }
            Event::Key(k) => self.key(k),
            Event::Mouse(m) => self.mouse(m),
            Event::Paste(s) => {
                if let Some(d) = &mut self.dialog {
                    d.handle_paste(&s);
                } else if let Some(w) = self.active() {
                    match &mut w.term {
                        Some(t) => t.paste(&s),
                        None => w.ed.insert_text(&s),
                    }
                }
            }
            Event::Focus(_) => {}
        }
    }

    fn key(&mut self, k: KeyEvent) {
        if k.kind == KeyKind::Release {
            return;
        }
        // The completion list takes its keys first; the rest type on.
        if self.dialog.is_none() {
            if let Some(comp) = &mut self.completion {
                if comp.waiting.is_none() {
                    match comp.popup.handle_key(&k) {
                        PopupResult::Accept(i) => return self.accept_completion(i),
                        PopupResult::Close => {
                            self.completion = None;
                            return;
                        }
                        PopupResult::Consumed => return,
                        PopupResult::Ignored => {}
                    }
                } else if k.key == Key::Esc {
                    self.completion = None;
                    return;
                }
            }
        }
        self.key_inner(k);
        if self.completion.is_some() {
            self.refilter_completion();
        }
    }

    fn key_inner(&mut self, k: KeyEvent) {
        if let Some(d) = &mut self.dialog {
            match d.handle_key(&k) {
                DialogResult::None => {}
                DialogResult::Cancel => self.dialog = None,
                DialogResult::Button(b) => {
                    let d = self.dialog.take().unwrap();
                    self.finish_dialog(d, Some(b));
                }
            }
            return;
        }
        // A terminal gets every key except the few Turbine keeps: F10 (menus),
        // Alt+F3 (close) and Alt+0..9 (windows). Shells need Alt+letters.
        let reserved = k.is(Key::F(10), Mods::NONE)
            || k.is(Key::F(3), Mods::ALT)
            || (k.mods.alt && !k.mods.ctrl && matches!(k.key, Key::Char('0'..='9')));
        if !self.menubar.is_open() && !reserved && self.windows.last().is_some_and(EdWin::is_terminal) {
            let from_agent = self.from_agent;
            let w = self.windows.last_mut().unwrap();
            let mut k = k;
            // Agents type literal text: their newline is a plain Enter here.
            if from_agent && k.key == Key::Enter {
                k.mods = Mods::NONE;
            }
            let t = w.term.as_mut().unwrap();
            // Copy and paste keys (Ctrl+C itself belongs to the program).
            let ch = |c: char| matches!(k.key, Key::Char(x) if x.eq_ignore_ascii_case(&c));
            let copy = (k.mods.ctrl && k.mods.shift && ch('c')) || k.is(Key::Insert, Mods::CTRL);
            let paste = (k.mods.ctrl && k.mods.shift && ch('v')) || k.is(Key::Insert, Mods::SHIFT);
            if copy {
                if let Some(s) = t.selected_text() {
                    self.copy(s);
                }
            } else if paste {
                let c = self.clipboard.clone();
                t.paste(&c);
            } else {
                t.handle_key(&k);
            }
            return;
        }
        match self.menubar.handle_key(&k) {
            MenuResult::Command(c) => return self.run_command(&c),
            MenuResult::Consumed => return,
            MenuResult::Ignored => {}
        }
        // Vim gets every key but the F-keys and Alt chords (menus).
        let vim_on = self.keymap == Keymap::Vim && self.windows.last().is_some_and(|w| !w.is_terminal());
        if vim_on && !matches!(k.key, Key::F(_)) && !k.mods.alt {
            let w = self.windows.last_mut().unwrap();
            let session = w.vim.get_or_insert_with(Session::new);
            match vim::key(&mut w.ed, session, &k) {
                VimOut::Handled => return,
                VimOut::Actions(a) => return self.vim_actions(a),
                VimOut::Window(c) => return self.vim_window(c),
                VimOut::NotMine => {}
            }
        }
        let shortcut = if k.is(Key::Char(' '), Mods::CTRL) {
            Some("lsp.complete")
        } else if k.is(Key::F(12), Mods::NONE) {
            Some("go.definition")
        } else if k.is(Key::F(1), Mods::CTRL) {
            Some("lsp.hover")
        } else if k.is(Key::F(9), Mods::CTRL) {
            Some("run.run")
        } else if k.is(Key::F(7), Mods::NONE) {
            Some("run.step_into")
        } else if k.is(Key::F(8), Mods::NONE) && self.debug.is_some() {
            Some("run.step_over")
        } else if k.is(Key::F(4), Mods::NONE) {
            Some("run.to_cursor")
        } else if k.is(Key::F(2), Mods::CTRL) {
            Some("run.reset")
        } else if k.is(Key::F(8), Mods::CTRL) {
            Some("run.breakpoint")
        } else if k.is(Key::F(3), Mods::CTRL) {
            Some("run.stack")
        } else if k.is(Key::F(8), Mods::NONE) || k.is(Key::F(8), Mods::ALT) {
            Some("go.next_problem")
        } else if k.is(Key::F(8), Mods::SHIFT) || k.is(Key::F(7), Mods::ALT) {
            Some("go.prev_problem")
        } else if k.is(Key::Char('`'), Mods::CTRL) {
            Some("view.terminal")
        } else if k.is(Key::F(6), Mods::SHIFT) {
            Some("window.prev")
        } else if k.is_alt('0') {
            Some("window.list")
        } else if k.is_ctrl('n') {
            Some("file.new")
        } else if k.is(Key::F(3), Mods::NONE) || k.is_ctrl('o') {
            Some("file.open")
        } else if k.is(Key::F(2), Mods::NONE) || k.is_ctrl('s') {
            Some("file.save")
        } else if k.is_alt('x') || k.is_ctrl('q') {
            Some("app.quit")
        } else if k.is(Key::F(6), Mods::NONE) {
            Some("window.next")
        } else if k.is(Key::F(5), Mods::NONE) {
            Some("window.zoom")
        } else if k.is(Key::F(3), Mods::ALT) || k.is_ctrl('w') {
            Some("window.close")
        } else if k.is(Key::F(1), Mods::NONE) {
            Some("help.keys")
        } else if k.is_ctrl('g') {
            Some("search.goto")
        } else {
            None
        };
        if let Some(c) = shortcut {
            return self.run_command(c);
        }
        if k.mods.alt && !k.mods.ctrl {
            if let Key::Char(c @ '1'..='9') = k.key {
                let n = c as usize - '1' as usize;
                if n < self.windows.len() {
                    let w = self.windows.remove(n);
                    self.windows.push(w);
                    self.renumber();
                }
                return;
            }
        }
        // In vim, only paging keys vim doesn't take reach the editor.
        if vim_on && !matches!(k.key, Key::PageUp | Key::PageDown) {
            return;
        }
        let before = self.clipboard.clone();
        let mut clip = std::mem::take(&mut self.clipboard);
        if let Some(w) = self.windows.last_mut() {
            w.ed.handle_key(&k, &mut clip);
        }
        if clip != before {
            self.copy(clip);
        } else {
            self.clipboard = clip;
        }
    }

    /// What vim's ex commands ask of Turbine: files and windows.
    fn vim_actions(&mut self, actions: Vec<ExAction>) {
        let mut split: Option<bool> = None;
        for a in actions {
            match a {
                ExAction::Write(p, _) => {
                    if p.is_empty() {
                        self.save(false);
                    } else if let Some(w) = self.windows.last_mut() {
                        let path = PathBuf::from(&p);
                        let msg = match w.ed.doc.save_as(&path) {
                            Ok(()) => {
                                w.win.title = path.display().to_string();
                                format!("\"{}\" written", path.display())
                            }
                            Err(e) => format!("E212: Can't open file for writing: {e}"),
                        };
                        self.say(msg);
                    }
                }
                ExAction::WriteAll(_) => {
                    let mut n = 0;
                    for w in &mut self.windows {
                        if w.ed.doc.dirty && w.ed.doc.path.is_some() && w.ed.doc.save().is_ok() {
                            n += 1;
                        }
                    }
                    self.say(format!("{n} file(s) written"));
                }
                ExAction::Quit(bang) => {
                    let Some(w) = self.windows.last_mut() else { continue };
                    if w.ed.doc.dirty && !bang {
                        self.say("E37: No write since last change (add ! to override)");
                        return;
                    }
                    self.windows.pop();
                    self.renumber();
                    if self.windows.is_empty() {
                        self.quit = true;
                    }
                }
                ExAction::QuitAll(bang) => {
                    if !bang && self.windows.iter().any(|w| w.ed.doc.dirty) {
                        self.say("E37: No write since last change (add ! to override)");
                        return;
                    }
                    self.quit = true;
                }
                ExAction::EditFile(p, bang) => {
                    if let Some(vertical) = split.take() {
                        self.open_path(Path::new(&p));
                        self.arrange_split(vertical);
                        continue;
                    }
                    let Some(w) = self.windows.last_mut() else { continue };
                    if w.ed.doc.dirty && !bang {
                        self.say("E37: No write since last change (add ! to override)");
                        return;
                    }
                    let path = if p.is_empty() { w.ed.doc.path.clone() } else { Some(PathBuf::from(&p)) };
                    let Some(path) = path else {
                        self.say("E32: No file name");
                        return;
                    };
                    let doc = if path.exists() {
                        match Document::open(&path) {
                            Ok(d) => d,
                            Err(e) => {
                                self.say(format!("Can't open {}: {e}", path.display()));
                                return;
                            }
                        }
                    } else {
                        let mut d = Document::new();
                        d.path = Some(path.clone());
                        d
                    };
                    let msg = format!("\"{}\" {} lines, {}", path.display(), doc.len_lines(), doc.encoding.name());
                    w.win.title = path.display().to_string();
                    w.ed = EditorView::new(doc);
                    if let Some(old) = w.vim.take() {
                        drop(old);
                        w.vim = Some(vim::attach(&mut w.ed));
                    }
                    self.say(msg);
                }
                ExAction::Window(c) => match c.as_str() {
                    "split" | "vsplit" => split = Some(c == "vsplit"),
                    "new" | "vnew" => {
                        self.new_window(Document::new());
                        self.arrange_split(c == "vnew");
                    }
                    "enew" => {
                        if self.windows.last().is_some_and(|w| !w.ed.doc.dirty) {
                            self.windows.pop();
                        }
                        self.new_window(Document::new());
                    }
                    "only" => {
                        let n = self.windows.len();
                        let mut kept = 0;
                        let mut i = 0;
                        self.windows.retain(|w| {
                            i += 1;
                            let keep = i == n || w.ed.doc.dirty;
                            if keep && i != n {
                                kept += 1;
                            }
                            keep
                        });
                        self.renumber();
                        let desk = self.desktop();
                        if let Some(w) = self.windows.last_mut() {
                            if !w.win.zoomed {
                                w.win.toggle_zoom(desk);
                            }
                        }
                        if kept > 0 {
                            self.say("E445: Other window contains changes");
                        }
                    }
                    "bnext" => self.run_command("window.next"),
                    "bprev" => self.run_command("window.prev"),
                    _ => {}
                },
                ExAction::SetOption(name, value) => {
                    let on = value != "false" && value != "0";
                    match name.as_str() {
                        "number" | "nu" => {
                            if let Some(w) = self.windows.last_mut() {
                                w.ed.line_numbers = on;
                            }
                        }
                        "rightleft" | "rl" => {
                            if let Some(w) = self.windows.last_mut() {
                                w.ed.base_rtl = if on { Some(true) } else { None };
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if split.is_some() {
            self.say("Two views of one buffer aren't in yet: give :sp a file name");
        }
        self.sync_menu_checks();
    }

    /// Ctrl-W commands.
    fn vim_window(&mut self, c: &str) {
        match c {
            "next" => self.run_command("window.next"),
            "prev" => self.run_command("window.prev"),
            "close" => self.vim_actions(vec![ExAction::Quit(false)]),
            "only" => self.vim_actions(vec![ExAction::Window("only".into())]),
            "new" => self.vim_actions(vec![ExAction::Window("new".into())]),
            "split" | "vsplit" => self.say("Two views of one buffer aren't in yet: use :sp {file}"),
            dir => self.focus_direction(dir),
        }
    }

    /// Focus the nearest window to the left/right/above/below.
    fn focus_direction(&mut self, dir: &str) {
        let Some(cur) = self.windows.last().map(|w| w.win.rect.center()) else { return };
        let best = self
            .windows
            .iter()
            .enumerate()
            .filter_map(|(i, w)| {
                let c = w.win.rect.center();
                let (dx, dy) = (c.x - cur.x, c.y - cur.y);
                let ok = match dir {
                    "left" => dx < 0,
                    "right" => dx > 0,
                    "up" => dy < 0,
                    "down" => dy > 0,
                    _ => false,
                };
                ok.then_some((i, dx.abs() + dy.abs() * 2))
            })
            .min_by_key(|&(_, d)| d);
        if let Some((i, _)) = best {
            let w = self.windows.remove(i);
            self.windows.push(w);
            self.renumber();
        }
    }

    /// Put the two top windows side by side (vertical) or stacked.
    fn arrange_split(&mut self, vertical: bool) {
        let n = self.windows.len();
        if n < 2 {
            return;
        }
        let d = self.desktop();
        let (a, b) = if vertical {
            let half = d.w / 2;
            (Rect::new(d.x, d.y, half, d.h), Rect::new(d.x + half, d.y, d.w - half, d.h))
        } else {
            let half = d.h / 2;
            (Rect::new(d.x, d.y, d.w, half), Rect::new(d.x, d.y + half, d.w, d.h - half))
        };
        self.windows[n - 2].win.zoomed = false;
        self.windows[n - 2].win.rect = a;
        self.windows[n - 1].win.zoomed = false;
        self.windows[n - 1].win.rect = b;
    }

    fn mouse(&mut self, m: MouseEvent) {
        let screen = self.screen();
        let desk = self.desktop();
        if self.dialog.is_none() {
            if let Some(comp) = &mut self.completion {
                match comp.popup.handle_mouse(&m) {
                    PopupResult::Accept(i) => return self.accept_completion(i),
                    PopupResult::Close => self.completion = None,
                    PopupResult::Consumed => return,
                    PopupResult::Ignored => {}
                }
            }
        }
        if let Some(d) = &mut self.dialog {
            match d.handle_mouse(&m) {
                DialogResult::Button(b) => {
                    let d = self.dialog.take().unwrap();
                    self.finish_dialog(d, Some(b));
                }
                DialogResult::Cancel => self.dialog = None,
                DialogResult::None => {}
            }
            return;
        }
        match self.menubar.handle_mouse(&m, screen) {
            MenuResult::Command(c) => return self.run_command(&c),
            MenuResult::Consumed => return,
            MenuResult::Ignored => {}
        }
        if let Some(i) = self.dragging {
            match m.kind {
                MouseKind::Drag(Button::Left) => self.windows[i].win.drag_to(m.pos, desk),
                _ => {
                    self.windows[i].win.end_drag();
                    self.dragging = None;
                }
            }
            return;
        }
        if let Some(i) = self.selecting {
            let done = !matches!(m.kind, MouseKind::Drag(_) | MouseKind::Moved);
            if i < self.windows.len() {
                if let Some(t) = &mut self.windows[i].term {
                    t.handle_mouse(&m);
                    // Copy on select, as X terminals do.
                    if done {
                        if let Some(s) = t.selected_text() {
                            self.copy(s);
                        }
                    }
                } else {
                    self.windows[i].ed.handle_mouse(&m);
                }
            }
            if done {
                self.selecting = None;
            }
            return;
        }
        // Other buttons and motion over the active terminal go to it (programs
        // that track the mouse, and middle-click paste).
        if let Some(w) = self.windows.last_mut() {
            if let Some(t) = &mut w.term {
                let inside = w.win.inner().contains(m.pos);
                let other = matches!(m.kind, MouseKind::Down(Button::Right | Button::Middle) | MouseKind::Up(_) | MouseKind::Moved);
                if inside && other {
                    if t.handle_mouse(&m) == MouseOutcome::PasteRequest {
                        let c = self.clipboard.clone();
                        t.paste(&c);
                    }
                    return;
                }
            }
        }
        if m.pos.y == self.size.1 - 1 {
            if let MouseKind::Down(Button::Left) = m.kind {
                if let Some(c) = self.status.hit(m.pos, self.size.1 - 1) {
                    self.run_command(&c);
                }
            }
            return;
        }
        let Some(i) = self.windows.iter().rposition(|w| w.win.rect.contains(m.pos)) else { return };
        match m.kind {
            MouseKind::Down(Button::Left) => {
                if i != self.windows.len() - 1 {
                    let w = self.windows.remove(i);
                    self.windows.push(w);
                    self.renumber();
                }
                let i = self.windows.len() - 1;
                let double = self.last_click.is_some_and(|(p, t)| p == m.pos && t.elapsed() < Duration::from_millis(400));
                self.last_click = Some((m.pos, Instant::now()));
                let w = &mut self.windows[i];
                match w.win.hit(m.pos) {
                    Hit::Close => self.run_command("window.close"),
                    Hit::Zoom => w.win.toggle_zoom(desk),
                    Hit::Title if double => w.win.toggle_zoom(desk),
                    h @ (Hit::Title | Hit::Left | Hit::Right | Hit::Bottom | Hit::BottomLeft | Hit::BottomRight) => {
                        if w.win.begin_drag(h, m.pos) {
                            self.dragging = Some(i);
                        }
                    }
                    Hit::VScroll => {
                        let r = w.win.rect;
                        if m.pos.y == r.y + 1 {
                            w.ed.scroll_y = w.ed.scroll_y.saturating_sub(1);
                        } else if m.pos.y == r.bottom() - 2 {
                            w.ed.scroll_y += 1;
                        } else {
                            let frac = (m.pos.y - r.y - 2) as f64 / (r.h - 4).max(1) as f64;
                            let line = (frac * w.ed.doc.len_lines() as f64) as usize;
                            w.ed.cursor = w.ed.doc.rope.line_to_char(line.min(w.ed.doc.len_lines().saturating_sub(1)));
                        }
                    }
                    Hit::Inside if w.term.is_some() => {
                        let t = w.term.as_mut().unwrap();
                        let out = t.handle_mouse(&m);
                        if out == MouseOutcome::Selection || out == MouseOutcome::Reported {
                            self.selecting = Some(i);
                        }
                    }
                    Hit::Inside => {
                        if w.ed.handle_mouse(&m) {
                            self.selecting = Some(i);
                        }
                    }
                    _ => {}
                }
            }
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                let w = &mut self.windows[i];
                let mut m2 = m;
                m2.pos = w.win.inner().center();
                match &mut w.term {
                    Some(t) => {
                        // At the pointer: programs that track the mouse want where.
                        t.handle_mouse(&m);
                    }
                    None => {
                        w.ed.handle_mouse(&m2);
                    }
                }
            }
            _ => {}
        }
    }

    // ----- automation -------------------------------------------------------

    pub fn tree(&self) -> Node {
        let screen = self.screen();
        let mut root = Node::new("workbench", "application", "Turbine", screen);
        let mut bar = Node::new("menubar", "menubar", "Menu", Rect::new(0, 0, screen.w, 1));
        let titles = self.menubar.title_rects();
        for (i, m) in self.menubar.menus.iter().enumerate() {
            let key = m.plain_title().to_lowercase();
            let mut mn = Node::new(format!("menu.{key}"), "menu", m.plain_title(), titles[i]);
            if self.menubar.open == Some(i) {
                mn.focused = true;
                for (j, it) in m.items.iter().enumerate() {
                    if it.is_separator() {
                        continue;
                    }
                    let mut n = Node::new(
                        format!("menuitem.{}", it.command),
                        "menuitem",
                        it.plain_label(),
                        self.menubar.item_rect(i, j, screen),
                    )
                    .enabled(it.enabled);
                    if let Some(c) = it.checked {
                        n = n.value(if c { "checked" } else { "unchecked" });
                    }
                    mn = mn.child(n);
                }
            }
            bar = bar.child(mn);
        }
        root = root.child(bar);
        let top = self.windows.len().saturating_sub(1);
        for (i, w) in self.windows.iter().enumerate() {
            let id = w.win.id;
            let (line, col) = w.ed.cursor_line_col();
            let mut wn = Node::new(format!("window.{id}"), "window", w.win.title.clone(), w.win.rect)
                .focused(i == top && self.dialog.is_none());
            wn = wn.child(Node::new(format!("window.{id}.close"), "button", "Close", w.win.close_rect()));
            wn = wn.child(Node::new(format!("window.{id}.zoom"), "button", "Zoom", w.win.zoom_rect()));
            if w.term.is_some() {
                let tn = Node::new(format!("terminal.{id}"), "terminal", w.win.title.clone(), w.win.inner())
                    .focused(i == top && self.dialog.is_none())
                    .value(w.term_text.clone());
                wn = wn.child(tn);
                root = root.child(wn);
                continue;
            }
            let mut ed = Node::new(format!("editor.{id}"), "editor", w.ed.doc.title(), w.win.inner())
                .focused(i == top && self.dialog.is_none())
                .value(format!(
                    "line {} col {} of {} lines{}",
                    line + 1,
                    col + 1,
                    w.ed.doc.len_lines(),
                    if w.ed.doc.dirty { ", modified" } else { "" }
                ));
            if let Some(v) = &w.vim {
                ed.value = Some(format!("{}, vim {}", ed.value.unwrap_or_default(), v.vi.mode_name()));
            }
            ed.enabled = true;
            wn = wn.child(ed);
            root = root.child(wn);
        }
        if let Some(d) = &self.dialog {
            let mut dn = Node::new(format!("dialog.{}", d.kind), "dialog", d.title.clone(), d.rect).focused(true);
            for (i, p) in d.controls.iter().enumerate() {
                let r = d.control_rect(i);
                let (name, value) = match &p.control {
                    Control::Label { text } => (text.clone(), None),
                    Control::Input { label, value, .. } => (label.replace('~', ""), Some(value.clone())),
                    Control::Button { label, .. } => (label.replace('~', ""), None),
                    Control::List { items, selected, .. } => {
                        (format!("{} items", items.len()), items.get(*selected).cloned())
                    }
                };
                let id = p.control.id().map(|s| format!("dialog.{s}")).unwrap_or_else(|| format!("dialog.label{i}"));
                let mut n = Node::new(id, p.control.role(), name, r).focused(i == d.focus);
                if let Some(v) = value {
                    n = n.value(v);
                }
                if let Control::List { items, scroll, .. } = &p.control {
                    for (row, it) in items.iter().enumerate().skip(*scroll).take(r.h as usize) {
                        let ir = Rect::new(r.x, r.y + (row - scroll) as i32, r.w, 1);
                        let cid = format!("{}.{row}", n.id);
                        n = n.child(Node::new(cid, "listitem", it.clone(), ir));
                    }
                }
                dn = dn.child(n);
            }
            root = root.child(dn);
        }
        if let Some(comp) = self.completion.as_ref().filter(|c| !c.popup.items.is_empty()) {
            let mut list = Node::new("completion", "list", "Completions", comp.popup.rect()).focused(true);
            for (i, (label, detail)) in comp.popup.items.iter().enumerate() {
                if let Some(r) = comp.popup.item_rect(i) {
                    let mut n = Node::new(format!("completion.{i}"), "listitem", label.clone(), r).focused(i == comp.popup.sel);
                    if !detail.is_empty() {
                        n = n.value(detail.clone());
                    }
                    list = list.child(n);
                }
            }
            root = root.child(list);
        }
        let srow = self.size.1 - 1;
        let mut sb = Node::new("statusbar", "statusbar", "Status", Rect::new(0, srow, screen.w, 1));
        for (it, r) in self.status.items.iter().zip(self.status.item_rects(srow)) {
            sb = sb.child(Node::new(format!("status.{}", it.command), "button", format!("{} {}", it.key, it.label), r));
        }
        root.child(sb)
    }

    /// Queue the steps that click a widget like a person would.
    fn queue_click(&mut self, target: &str) {
        self.driver.queue.push_back(Step::MoveToTarget(target.into()));
        self.driver.queue.push_back(Step::Press(Button::Left));
        self.driver.queue.push_back(Step::Release(Button::Left));
    }

    fn queue_menu(&mut self, path: &str) {
        let parts: Vec<&str> = path.split('/').map(str::trim).filter(|s| !s.is_empty()).collect();
        if let Some((first, rest)) = parts.split_first() {
            self.queue_click(&format!("menu:{first}"));
            for p in rest {
                self.queue_click(&format!("menuitem:{p}"));
            }
        }
    }

    fn queue_type(&mut self, text: &str) {
        for c in text.chars() {
            self.driver.queue.push_back(match c {
                // Literal text: no auto-indent on top of the text's own.
                '\n' => Step::Key(KeyEvent::new(Key::Enter, Mods::SHIFT)),
                '\t' => Step::Key(KeyEvent::plain(Key::Tab)),
                c => Step::Char(c),
            });
        }
    }

    fn queue_demo(&mut self) {
        self.driver.agent_name = "Demo agent".into();
        self.queue_menu("File/New Text File");
        self.driver.queue.push_back(Step::Pause(Duration::from_millis(300)));
        self.queue_type(DEMO_CODE);
        self.driver.queue.push_back(Step::Pause(Duration::from_millis(400)));
        self.queue_menu("Window/Tile");
        self.queue_menu("Options/Colors");
        self.queue_click("listitem:Borland Cyan");
        self.queue_click("button:OK");
    }

    /// Handle one automation request. Actions reply when they finish.
    pub fn request(&mut self, cmd: Value, reply: Sender<Value>) {
        let name = cmd.get("cmd").and_then(Value::as_str).unwrap_or("").to_string();
        let s = |k: &str| cmd.get(k).and_then(Value::as_str).map(str::to_string);
        let ok = json!({"ok": true});
        match name.as_str() {
            "snapshot" => {
                let mut v = json!({"ok": true, "tree": self.tree()});
                if cmd.get("screen").and_then(Value::as_bool).unwrap_or(false) {
                    let mut surf = Surface::new(self.size.0, self.size.1);
                    self.render(&mut surf);
                    v["screen"] = Value::String(surf.to_text());
                }
                let _ = reply.send(v);
                return;
            }
            "text" => {
                let t = self
                    .windows
                    .last()
                    .map(|w| if w.term.is_some() { w.term_text.clone() } else { w.ed.doc.rope.to_string() })
                    .unwrap_or_default();
                let _ = reply.send(json!({"ok": true, "text": t}));
                return;
            }
            "speed" => {
                if let Some(c) = cmd.get("cps").and_then(Value::as_f64) {
                    self.driver.cps = c;
                }
                let _ = reply.send(json!({"ok": true, "cps": self.driver.cps}));
                return;
            }
            "click" => {
                if let (Some(x), Some(y)) = (cmd.get("x").and_then(Value::as_i64), cmd.get("y").and_then(Value::as_i64)) {
                    self.driver.queue.push_back(Step::MoveTo(Point::new(x as i32, y as i32)));
                    self.driver.queue.push_back(Step::Press(Button::Left));
                    self.driver.queue.push_back(Step::Release(Button::Left));
                } else if let Some(t) = s("target") {
                    self.queue_click(&t);
                } else {
                    let _ = reply.send(json!({"ok": false, "error": "click needs target or x,y"}));
                    return;
                }
            }
            "drag" => {
                let p = |k: &str| {
                    cmd.get(k).and_then(|v| Some(Point::new(v.get(0)?.as_i64()? as i32, v.get(1)?.as_i64()? as i32)))
                };
                let (Some(a), Some(b)) = (p("from"), p("to")) else {
                    let _ = reply.send(json!({"ok": false, "error": "drag needs from:[x,y] and to:[x,y]"}));
                    return;
                };
                self.driver.queue.push_back(Step::MoveTo(a));
                self.driver.queue.push_back(Step::Press(Button::Left));
                self.driver.queue.push_back(Step::MoveTo(b));
                self.driver.queue.push_back(Step::Release(Button::Left));
            }
            "key" => {
                let chord = s("key").unwrap_or_default();
                match KeyEvent::parse(&chord) {
                    Some(k) => self.driver.queue.push_back(Step::Key(k)),
                    None => {
                        let _ = reply.send(json!({"ok": false, "error": format!("bad key {chord:?}")}));
                        return;
                    }
                }
            }
            "type" => self.queue_type(&s("text").unwrap_or_default()),
            "paste" => self.driver.queue.push_back(Step::Paste(s("text").unwrap_or_default())),
            "menu" => self.queue_menu(&s("path").unwrap_or_default()),
            "command" => self.driver.queue.push_back(Step::Command(s("name").unwrap_or_default())),
            "open" => {
                let p = PathBuf::from(s("path").unwrap_or_default());
                self.open_path(&p);
            }
            "wait" => {
                if let Some(t) = s("text") {
                    let ms = cmd.get("timeout_ms").and_then(Value::as_u64).unwrap_or(10_000);
                    self.driver.queue.push_back(Step::WaitText(t, Duration::from_millis(ms)));
                }
            }
            "stop" => self.driver.cancel(),
            _ => {
                let _ = reply.send(json!({"ok": false, "error": format!("unknown cmd {name:?}")}));
                return;
            }
        }
        if let Some(a) = s("agent") {
            self.driver.agent_name = a;
        }
        self.driver.queue.push_back(Step::Reply(reply, ok));
    }

    fn apply_driver(&mut self, outs: Vec<Output>) {
        for o in outs {
            match o {
                Output::Pointer(p, held) => {
                    let kind = held.map(MouseKind::Drag).unwrap_or(MouseKind::Moved);
                    self.dispatch(Event::Mouse(MouseEvent { kind, pos: p, mods: Mods::NONE }));
                }
                Output::Press(p, b) => {
                    self.dispatch(Event::Mouse(MouseEvent { kind: MouseKind::Down(b), pos: p, mods: Mods::NONE }));
                }
                Output::Release(p, b) => {
                    self.dispatch(Event::Mouse(MouseEvent { kind: MouseKind::Up(b), pos: p, mods: Mods::NONE }));
                }
                Output::Key(k) => {
                    self.from_agent = true;
                    self.dispatch(Event::Key(k));
                    self.from_agent = false;
                }
                Output::Paste(s) => self.dispatch(Event::Paste(s)),
                Output::Command(c) => self.run_command(&c),
                Output::Resolve(t) => match self.tree().find_target(&t) {
                    Some(n) if n.enabled => self.driver.push_front(Step::MoveTo(n.click_point())),
                    Some(_) => {
                        self.say(format!("Agent: {t} is disabled"));
                        self.fail_queue(format!("{t} is disabled"));
                    }
                    None => {
                        self.say(format!("Agent: can't find {t}"));
                        self.fail_queue(format!("can't find {t}"));
                    }
                },
                Output::Reply(tx, v) => {
                    let _ = tx.send(v);
                }
            }
        }
    }

    /// Drop the rest of the current action and report the error.
    fn fail_queue(&mut self, err: String) {
        while let Some(s) = self.driver.queue.pop_front() {
            if let Step::Reply(tx, _) = s {
                let _ = tx.send(json!({"ok": false, "error": err}));
                break;
            }
        }
    }
}

impl App for Workbench {
    fn event(&mut self, ev: Event) {
        self.human_input(&ev);
        if self.driver.busy() && matches!(ev, Event::Key(_)) {
            return; // keys go to the agent controls while it drives
        }
        self.dispatch(ev);
    }

    fn tick(&mut self, now: Instant) -> Option<Instant> {
        let mut any_term = false;
        for w in &mut self.windows {
            if let Some(t) = &mut w.term {
                any_term = true;
                t.layout(w.win.inner());
                if t.pump() {
                    w.term_text = t.screen_text();
                    w.win.title = t.title().unwrap_or_else(|| "Terminal".into());
                }
            }
        }
        self.update_highlights();
        self.update_lsp();
        self.update_debug();
        if let Some(rx) = &self.requests {
            let reqs: Vec<Request> = rx.try_iter().collect();
            for r in reqs {
                self.request(r.cmd, r.reply);
            }
        }
        if let Some((text, deadline)) = self.driver.waiting.clone() {
            let shown = self
                .windows
                .last()
                .map(|w| if w.term.is_some() { w.term_text.contains(&text) } else { w.ed.doc.rope.to_string().contains(&text) })
                .unwrap_or(false);
            if shown {
                self.driver.waiting = None;
            } else if now >= deadline {
                self.driver.waiting = None;
                self.fail_queue(format!("timed out waiting for {text:?}"));
            }
        }
        let mut next = None;
        for _ in 0..10_000 {
            let (outs, n) = self.driver.tick(now);
            let progressed = !outs.is_empty();
            self.apply_driver(outs);
            next = n;
            // Instant mode, or a resolve that pushed a step: keep going.
            if !(progressed && (self.driver.instant() || n == Some(now))) {
                break;
            }
        }
        if let Some((_, t)) = &self.message {
            if t.elapsed() > Duration::from_secs(4) {
                self.message = None;
            }
        }
        let msg_tick = self.message.as_ref().map(|(_, t)| *t + Duration::from_secs(4));
        let poll = self.requests.as_ref().map(|_| now + Duration::from_millis(50));
        // Terminals are read on ticks: ~60 a second while one is open.
        let lsp_waiting = self.lsp.values().any(|c| c.waiting() > 0) || self.debug.is_some();
        let term_tick = (any_term || self.hl_pending > 0 || lsp_waiting).then(|| now + Duration::from_millis(16));
        // Servers send diagnostics unasked: look a few times a second.
        let lsp_tick = (!self.lsp.is_empty()).then(|| now + Duration::from_millis(100));
        [next, msg_tick, poll, term_tick, lsp_tick].into_iter().flatten().min()
    }

    fn render(&mut self, surf: &mut Surface) {
        if surf.w != self.size.0 || surf.h != self.size.1 {
            *surf = Surface::new(self.size.0, self.size.1);
        }
        surf.cursor = None;
        let theme = self.theme.clone();
        let screen = self.screen();
        let desk = self.desktop();
        let mut c = surf.canvas();
        c.fill(desk, theme.scheme.desktop_char, theme.scheme.desktop);
        let n = self.windows.len();
        let has_modal = self.dialog.is_some() || self.menubar.is_open();
        for (i, w) in self.windows.iter_mut().enumerate() {
            let active = i + 1 == n;
            let mut wc = c.sub(desk);
            if theme.style.shadows {
                wc.shadow(w.win.rect, theme.scheme.shadow);
            }
            w.win.draw_frame(&mut wc, &theme, active);
            let inner = w.win.inner();
            if let Some(t) = &mut w.term {
                t.draw(&mut wc, inner, &theme, active && !has_modal);
                continue;
            }
            w.ed.draw(&mut wc, inner, &theme, active && !has_modal, active);
            if active {
                w.ed.draw_scrollbars(&mut wc, w.win.rect, &theme);
                if w.ed.doc.dirty {
                    wc.print(w.win.rect.x + 1, w.win.rect.bottom() - 1, "*", theme.scheme.frame_icons);
                }
            }
        }
        // Status bar.
        let info = self
            .windows
            .last()
            .map(|w| {
                if let Some(t) = &w.term {
                    return match t.error() {
                        Some(_) => "Terminal (failed)".to_string(),
                        None => "Terminal  F10 menu".to_string(),
                    };
                }
                let (l, col) = w.ed.cursor_line_col();
                let lang = w
                    .lang
                    .as_deref()
                    .map(|l| turbine_syntax::BUILTIN.iter().find(|(n, _)| *n == l).map(|(_, label)| label.to_string()).unwrap_or_else(|| l.to_string()))
                    .unwrap_or_else(|| "Text".into());
                format!(
                    "{lang}  {}:{}  {} {}  {}",
                    l + 1,
                    col + 1,
                    w.ed.doc.encoding.name(),
                    w.ed.doc.line_ending.name(),
                    if w.vim.is_some() {
                        ""
                    } else if w.ed.overwrite {
                        "OVR"
                    } else {
                        "INS"
                    }
                )
            })
            .unwrap_or_default();
        let vim_line = self.windows.last().and_then(|w| w.vim.as_ref()).map(|v| (v.mode_line(), v.command_line()));
        self.status.info = match &vim_line {
            Some((mode, _)) => format!("{mode}  {info}"),
            None => info,
        };
        let cmdline = vim_line.and_then(|(_, c)| c);
        self.status.message = if self.driver.busy() {
            Some(format!(
                "{} {} - Esc stops, Space {}",
                if self.driver.paused { "⏸" } else { "►" },
                self.driver.agent_name,
                if self.driver.paused { "resumes" } else { "pauses" }
            ))
        } else if let Some((line, _, _)) = &cmdline {
            Some(line.clone())
        } else if let Some((m, _)) = &self.message {
            Some(m.clone())
        } else {
            self.windows
                .last()
                .filter(|w| w.term.is_none())
                .and_then(|w| w.ed.diagnostic_at_cursor())
                .map(|d| format!("{} {}", if d.severity == 1 { "■ Error:" } else if d.severity == 2 { "■ Warning:" } else { "■" }, d.message))
        };
        self.status.draw(&mut c, &theme, Rect::new(0, self.size.1 - 1, self.size.0, 1));
        if let Some((_, Some(col), _)) = &cmdline {
            if self.dialog.is_none() && !self.driver.busy() {
                c.set_cursor(Point::new(1 + *col as i32, self.size.1 - 1), turbine_ui::CursorShape::Bar);
            }
        }
        self.menubar.draw(&mut c, &theme, screen);
        if let Some(comp) = &mut self.completion {
            let at = self.windows.last().filter(|w| w.win.id == comp.win_id).and_then(|w| w.ed.cursor_screen_pos());
            if let (Some(at), false) = (at, comp.popup.items.is_empty()) {
                comp.popup.draw(&mut c, at, desk, &theme);
            }
        }
        if let Some(d) = &mut self.dialog {
            d.layout(screen);
            d.draw(&mut c, &theme);
        }
        self.menubar.draw_dropdown(&mut c, &theme, screen);
        // The agent's pointer: a highlighted cell, like a DOS mouse cursor.
        if let Some(p) = self.driver.pointer {
            if self.driver.busy() {
                let st: Style = theme.scheme.agent_pointer;
                c.set_style_at(p, st);
            }
        }
    }

    fn should_quit(&self) -> bool {
        self.quit
    }

    fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard_out.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn run(wb: &mut Workbench, cmd: Value) -> Value {
        let (tx, rx) = mpsc::channel();
        wb.request(cmd, tx);
        for _ in 0..1000 {
            wb.tick(Instant::now());
            if let Ok(v) = rx.try_recv() {
                return v;
            }
        }
        panic!("no reply");
    }

    fn bench() -> Workbench {
        let mut wb = Workbench::new(&[]);
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        wb
    }

    #[test]
    fn agent_types_through_the_ui() {
        let mut wb = bench();
        assert_eq!(run(&mut wb, json!({"cmd": "menu", "path": "File/New Text File"}))["ok"], true);
        assert_eq!(wb.windows.len(), 2);
        run(&mut wb, json!({"cmd": "type", "text": "hello\nworld"}));
        assert_eq!(run(&mut wb, json!({"cmd": "text"}))["text"], "hello\nworld");
        run(&mut wb, json!({"cmd": "key", "key": "Ctrl+A"}));
        run(&mut wb, json!({"cmd": "key", "key": "Ctrl+X"}));
        assert_eq!(wb.clipboard, "hello\nworld");
    }

    #[test]
    fn agent_picks_a_color_scheme_in_a_dialog() {
        let mut wb = bench();
        run(&mut wb, json!({"cmd": "menu", "path": "Options/Colors..."}));
        assert!(wb.dialog.is_some());
        run(&mut wb, json!({"cmd": "click", "target": "listitem:QBasic"}));
        let r = run(&mut wb, json!({"cmd": "click", "target": "button:OK"}));
        assert_eq!(r["ok"], true);
        assert_eq!(wb.theme.scheme.name, "QBasic");
    }

    #[test]
    fn missing_target_fails_cleanly() {
        let mut wb = bench();
        let r = run(&mut wb, json!({"cmd": "click", "target": "button:Nope"}));
        assert_eq!(r["ok"], false);
    }

    fn keys(wb: &mut Workbench, chords: &[&str]) {
        for c in chords {
            assert_eq!(run(wb, json!({"cmd": "key", "key": c}))["ok"], true, "{c}");
        }
    }

    #[test]
    fn vim_edits_saves_and_quits() {
        let mut wb = bench();
        wb.set_keymap(Keymap::Vim);
        run(&mut wb, json!({"cmd": "type", "text": "ihello world"}));
        keys(&mut wb, &["Esc"]);
        run(&mut wb, json!({"cmd": "type", "text": "0dwyyp"}));
        assert_eq!(run(&mut wb, json!({"cmd": "text"}))["text"], "world\nworld");
        // Macro: append ";" to each line.
        run(&mut wb, json!({"cmd": "type", "text": "ggqqA;"}));
        keys(&mut wb, &["Esc"]);
        run(&mut wb, json!({"cmd": "type", "text": "jq@q"}));
        assert_eq!(run(&mut wb, json!({"cmd": "text"}))["text"], "world;\nworld;");
        // :w to a file, then :q closes the window and, being the last, quits.
        let dir = std::env::temp_dir().join(format!("turbine-vim-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("out.txt");
        run(&mut wb, json!({"cmd": "type", "text": format!(":w {}", f.display())}));
        keys(&mut wb, &["Enter"]);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "world;\nworld;");
        run(&mut wb, json!({"cmd": "type", "text": ":q"}));
        keys(&mut wb, &["Enter"]);
        assert!(wb.should_quit());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn vim_refuses_to_quit_with_changes_and_menus_still_work() {
        let mut wb = bench();
        wb.set_keymap(Keymap::Vim);
        run(&mut wb, json!({"cmd": "type", "text": "ix"}));
        keys(&mut wb, &["Esc"]);
        run(&mut wb, json!({"cmd": "type", "text": ":q"}));
        keys(&mut wb, &["Enter"]);
        assert!(!wb.should_quit());
        assert!(wb.message.as_ref().unwrap().0.starts_with("E37"));
        // Ctrl-W n opens a window; F6 and menus work in vim mode.
        keys(&mut wb, &["Ctrl+W", "n"]);
        assert_eq!(wb.windows.len(), 2);
        run(&mut wb, json!({"cmd": "menu", "path": "Window/Tile"}));
        let snap = run(&mut wb, json!({"cmd": "snapshot"}));
        assert!(snap.to_string().contains("vim NORMAL"));
    }

    #[test]
    fn narrow_menu_bar_fits_or_stays_reachable() {
        let mut wb = bench();
        wb.dispatch(Event::Resize(80, 25));
        let rects = wb.menubar.title_rects();
        assert!(rects.iter().all(|r| r.right() <= 80));
        assert!(rects.iter().all(|r| r.w > 0), "all menus fit at 80 columns");
        wb.dispatch(Event::Resize(40, 25));
        keys(&mut wb, &["Alt+H"]);
        assert!(wb.menubar.open.is_some(), "Help opens from the keyboard even when it doesn't fit");
    }

    #[test]
    fn terminal_runs_a_shell_command_the_agent_types() {
        let mut wb = bench();
        wb.new_terminal(vec!["sh".into()]);
        let (tx, rx) = mpsc::channel();
        wb.request(json!({"cmd": "type", "text": "echo turbine-$((6*7))\n"}), tx.clone());
        wb.request(json!({"cmd": "wait", "text": "turbine-42", "timeout_ms": 5000}), tx);
        let start = Instant::now();
        let mut replies = Vec::new();
        while replies.len() < 2 && start.elapsed() < Duration::from_secs(8) {
            wb.tick(Instant::now());
            replies.extend(rx.try_iter());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert!(replies.iter().all(|r| r["ok"] == true), "{replies:?}");
        let snap = run(&mut wb, json!({"cmd": "snapshot"}));
        assert!(snap.to_string().contains("\"role\":\"terminal\""));
        // Its screen drew into the window.
        let mut surf = Surface::new(100, 30);
        wb.render(&mut surf);
        assert!(surf.to_text().contains("turbine-42"), "{}", surf.to_text());
    }

    #[test]
    fn terminal_drag_selects_and_copies() {
        let mut wb = bench();
        wb.new_terminal(vec!["sh".into(), "-c".into(), "printf 'alpha beta gamma\\n'; exec cat".into()]);
        let start = Instant::now();
        let mut surf = Surface::new(100, 30);
        let (x0, y) = loop {
            wb.tick(Instant::now());
            wb.render(&mut surf);
            let text = surf.to_text();
            if let Some((row, line)) = text.lines().enumerate().find(|(_, l)| l.contains("alpha beta")) {
                break (line.chars().take_while(|_| true).collect::<String>().find("alpha").map(|b| line[..b].chars().count()).unwrap() as i64, row as i64);
            }
            assert!(start.elapsed() < Duration::from_secs(5), "no output: {text}");
            std::thread::sleep(Duration::from_millis(10));
        };
        // Drag from "beta" to the end of "gamma".
        let r = run(&mut wb, json!({"cmd": "drag", "from": [x0 + 6, y], "to": [x0 + 15, y]}));
        assert_eq!(r["ok"], true);
        assert_eq!(wb.clipboard, "beta gamma");
        assert_eq!(wb.clipboard_out.as_deref(), Some("beta gamma"));
        // Shift+Insert pastes it back into the program (cat echoes it).
        run(&mut wb, json!({"cmd": "key", "key": "Shift+Insert"}));
        let start = Instant::now();
        loop {
            wb.tick(Instant::now());
            if wb.windows.last().unwrap().term_text.matches("beta gamma").count() >= 2 {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "{}", wb.windows.last().unwrap().term_text);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn terminal_reports_clicks_to_programs_that_ask() {
        let mut wb = bench();
        let script = "printf '\\033[?1000h\\033[?1006hready\\n'; stty -icanon -echo; exec cat -v";
        wb.new_terminal(vec!["sh".into(), "-c".into(), script.into()]);
        let start = Instant::now();
        while !wb.windows.last().unwrap().term_text.contains("ready") {
            wb.tick(Instant::now());
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100)); // let stty run
        let inner = wb.windows.last().unwrap().win.inner();
        run(&mut wb, json!({"cmd": "click", "x": inner.x + 4, "y": inner.y + 2}));
        let start = Instant::now();
        loop {
            wb.tick(Instant::now());
            let t = &wb.windows.last().unwrap().term_text;
            if t.contains("^[[<0;5;3M") && t.contains("^[[<0;5;3m") {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "{t}");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(wb.clipboard.is_empty(), "a reported click doesn't select");
    }

    #[test]
    fn rust_file_is_highlighted_in_borland_colours() {
        let dir = std::env::temp_dir().join(format!("turbine-hl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("main.rs");
        std::fs::write(&f, "fn main() { let s = \"hi\"; } // done\n").unwrap();
        let mut wb = Workbench::new(&[f]);
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        let start = Instant::now();
        while wb.windows[0].ed.highlights.is_empty() {
            wb.tick(Instant::now());
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut surf = Surface::new(100, 30);
        wb.render(&mut surf);
        let fg_of = |needle: &str| {
            for y in 0..surf.h {
                let row: String = (0..surf.w).map(|x| surf.get(x, y).unwrap().text.clone()).collect();
                if let Some(i) = row.find(needle) {
                    let x = row[..i].chars().count() as i32;
                    return surf.get(x, y).unwrap().style.fg;
                }
            }
            panic!("{needle} not on screen");
        };
        use turbine_ui::theme::vga;
        assert_eq!(fg_of("fn main"), vga::WHITE);
        assert_eq!(fg_of("\"hi\""), vga::LIGHT_CYAN);
        assert_eq!(fg_of("// done"), vga::LIGHT_GRAY);
        assert!(wb.status.info.starts_with("Rust"), "{}", wb.status.info);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn wait_until(wb: &mut Workbench, what: &str, mut f: impl FnMut(&mut Workbench) -> bool) {
        let start = Instant::now();
        while !f(wb) {
            wb.tick(Instant::now());
            assert!(start.elapsed() < Duration::from_secs(5), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn language_server_diagnostics_completion_definition_hover() {
        let dir = std::env::temp_dir().join(format!("turbine-lsp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("main.rs");
        std::fs::write(&f, "fn main() {\n    BAD;\n    pr\n}\n").unwrap();
        let mut wb = Workbench::new(&[f]);
        wb.set_lsp_spawner(Box::new(|lang, root| {
            Some(turbine_services::fake::fake_server().map(|rpc| LspClient::with_rpc(rpc, root, lang)))
        }));
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));

        // Diagnostics: the BAD line is marked; F8 goes there and says why.
        wait_until(&mut wb, "diagnostics", |wb| !wb.windows[0].ed.diagnostics.is_empty());
        assert_eq!(wb.windows[0].ed.diagnostics[0].line, 1);
        wb.message = None;
        keys(&mut wb, &["F8"]);
        assert_eq!(wb.windows[0].ed.cursor_line_col(), (1, 4));
        assert!(wb.message.as_ref().unwrap().0.contains("BAD is not allowed"));

        // Completion after "pr": a list of matches; Enter puts one in.
        keys(&mut wb, &["Down", "End", "Ctrl+Space"]);
        wait_until(&mut wb, "completion list", |wb| wb.completion.as_ref().is_some_and(|c| !c.popup.items.is_empty()));
        let labels: Vec<String> = wb.completion.as_ref().unwrap().popup.items.iter().map(|i| i.0.clone()).collect();
        assert!(labels.iter().all(|l| l.starts_with("pr")), "{labels:?}");
        let snap = run(&mut wb, json!({"cmd": "snapshot"}));
        assert!(snap.to_string().contains("\"id\":\"completion\""), "agents see the list");
        run(&mut wb, json!({"cmd": "type", "text": "intl"}));
        let labels: Vec<String> = wb.completion.as_ref().unwrap().popup.items.iter().map(|i| i.0.clone()).collect();
        assert_eq!(labels, vec!["println!"]);
        keys(&mut wb, &["Enter"]);
        assert!(wb.completion.is_none());
        assert_eq!(wb.windows[0].ed.doc.line(2), "    println!");

        // F12 on "main" (line 0) jumps to where it's defined.
        keys(&mut wb, &["Ctrl+Home", "Right", "Right", "Right", "Right"]);
        wb.windows[0].ed.cursor = wb.windows[0].ed.doc.line_byte_to_char(2, 6);
        keys(&mut wb, &["F12"]);
        wait_until(&mut wb, "definition", |wb| wb.windows[0].ed.cursor_line_col() == (2, 4));

        // Ctrl+F1 shows what the server says about the word.
        keys(&mut wb, &["Ctrl+F1"]);
        wait_until(&mut wb, "hover", |wb| wb.dialog.as_ref().is_some_and(|d| d.kind == "hover"));
        let text = wb.dialog.as_ref().unwrap().value("text").unwrap_or_default();
        assert!(text.contains("fake hover: println"), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn debugger_breakpoint_step_continue_output() {
        let dir = std::env::temp_dir().join(format!("turbine-dbg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("prog.py");
        std::fs::write(&f, "print(a)\nx = 1\nprint(b)\nprint(c)\n").unwrap();
        let mut wb = Workbench::new(&[f]);
        wb.set_lsp_spawner(Box::new(|_, _| None));
        wb.set_dap_spawner(Box::new(|_, launch, bps| {
            Some(turbine_services::fake::fake_debug_adapter().map(|rpc| DapClient::with_rpc(rpc, launch, bps)))
        }));
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        let code = |wb: &Workbench| wb.windows.iter().position(|w| w.special.is_none()).unwrap();
        let special = |wb: &Workbench, k: &str| wb.windows.iter().find(|w| w.special == Some(k)).map(|w| w.ed.doc.rope.to_string()).unwrap_or_default();

        keys(&mut wb, &["Down", "Down", "Ctrl+F8"]);
        wb.tick(Instant::now());
        assert_eq!(wb.windows[code(&wb)].ed.breakpoints, vec![2]);

        keys(&mut wb, &["Ctrl+F9"]);
        wait_until(&mut wb, "stop at the breakpoint", |wb| wb.windows.iter().any(|w| w.ed.exec_line == Some(2)));
        wait_until(&mut wb, "variables", |wb| special(wb, "debug").contains("line: int = 3"));
        assert_eq!(special(&wb, "output"), "a\n");
        // The bar colours: exec line wins over the breakpoint's red.
        let mut surf = Surface::new(100, 30);
        wb.render(&mut surf);

        keys(&mut wb, &["F8"]);
        wait_until(&mut wb, "step", |wb| wb.windows.iter().any(|w| w.ed.exec_line == Some(3)));
        assert_eq!(special(&wb, "output"), "a\nb\n");

        keys(&mut wb, &["Ctrl+F9"]);
        wait_until(&mut wb, "finish", |wb| wb.debug.is_none());
        assert_eq!(special(&wb, "output"), "a\nb\nc\n");
        assert!(wb.windows.iter().all(|w| w.ed.exec_line.is_none()));
        // F8 means "next problem" again when not debugging.
        keys(&mut wb, &["F8"]);
        assert!(wb.debug.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn window_drag_resizes() {
        let mut wb = bench();
        let r0 = wb.windows[0].win.rect;
        let corner = [r0.right() - 1, r0.bottom() - 1];
        run(&mut wb, json!({"cmd": "drag", "from": corner, "to": [corner[0] - 20, corner[1] - 5]}));
        let r1 = wb.windows[0].win.rect;
        assert_eq!((r1.w, r1.h), (r0.w - 20, r0.h - 5));
    }
}
