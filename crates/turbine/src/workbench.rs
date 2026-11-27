//! The IDE itself: menu bar, desktop of editor windows, dialogs, status
//! bar, and the automation endpoint agents drive it through.

use crate::agent::{Driver, Output, Step};
use crate::vim::{self, Keymap};
use turbine_term::{MouseOutcome, Spawn, TerminalView};
use turbine_services::ai::{self, Ai, AiEvent, ChatMessage};
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
            Ok(v) if matches!(v.trim(), "none" | "off" | "0") => return None,
            Ok(v) if !v.trim().is_empty() => v.split_whitespace().map(str::to_string).collect(),
            _ => dap::adapter_for(lang)?,
        };
        Some(DapClient::start(&argv, launch, bps))
    })
}

/// Turbine's assistant working the UI with tools.
struct AgentRun {
    model: ai::Model,
    messages: Vec<Value>,
    /// The model turn being waited for.
    waiting: Option<u64>,
    /// Tool calls still to run: (call id, tool, arguments).
    calls: std::collections::VecDeque<(String, String, Value)>,
    /// The tool call running now, and where its result arrives.
    current: Option<(String, String, Receiver<Value>)>,
    /// A terminal command waiting for the person's yes.
    approving: Option<(String, String, Value)>,
    /// The person said yes to all terminal commands this run.
    terminal_ok: bool,
    turns: usize,
}

const AGENT_PROMPT: &str = "You are Turbine's agent. Turbine is a DOS-style code editor (menus, windows, \
dialogs, editors, terminals) and you work it like a person, with tools: look at the screen, open menus, \
click widgets, type, press keys, read a window's text, wait for text. The person watches everything you \
do. Start with look. After each action that changes the screen, look again before the next. To write \
code, focus an editor and type it (newlines become Enter; editors don't auto-indent your text). Save with \
F2. Terminals run a real shell: type a command and a newline to run it, then wait for its output. When the \
task is done, answer briefly in plain text without calling a tool.";

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
            Ok(v) if matches!(v.trim(), "none" | "off" | "0") => return None,
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
use turbine_text::encoding::{LineEnding, TextEncoding};
use turbine_text::Document;
use turbine_ui::automation::Node;
use crate::file_dialog::FileDialog;
use turbine_ui::dialog::{Dialog, DialogResult};
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
    /// The Open / Save As dialog's state (with `dialog` of kind "filedlg").
    file_dlg: Option<FileDialog>,
    /// "Replace it?" over the Save As dialog.
    overwrite_q: Option<Dialog>,
    /// Folders files were opened from or saved to, newest first.
    recent_dirs: Vec<PathBuf>,
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
    /// Guides fetched in the background: (key, title, Markdown or error).
    guide_tx: std::sync::mpsc::Sender<(String, String, Result<String, String>)>,
    guide_rx: Receiver<(String, String, Result<String, String>)>,
    guides_fetching: usize,
    /// AI models (hearth's hub, or local fallbacks); None until started.
    ai: Option<Ai>,
    /// Cursor-style ghost-text completion while typing.
    tab_complete: bool,
    /// The last edit by a person: (when, window, doc version).
    last_edit: Option<(Instant, u32, u64)>,
    /// A completion asked for: (request, window, version, cursor).
    fim_pending: Option<(u64, u32, u64, usize)>,
    /// The chat so far, and the answer being streamed (request id).
    chat_history: Vec<ChatMessage>,
    chat_streaming: Option<(u64, String)>,
    /// The chat model the person picked (else the hub's loaded one).
    chat_model: Option<String>,
    /// A question asked before the model list arrived.
    pending_question: Option<String>,
    agent: Option<AgentRun>,
    /// A task given before the model list arrived.
    pending_task: Option<String>,
    /// The automation socket, told to programs in terminals (TURBINE_SOCKET).
    socket_path: Option<PathBuf>,
    /// Where the chosen colours and style are kept.
    look_path: Option<PathBuf>,
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
                it("~C~hat...", "Ctrl+L", "ai.chat"),
                soon("~I~nline Edit", "Ctrl+K", "ai.inline"),
                it("~A~gent...", "Ctrl+I", "ai.agent"),
                it("~T~ab Completion", "", "ai.tab"),
                it("~M~odels...", "", "ai.models"),
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
                it("~C~ontents", "F1", "help.contents"),
                it("~I~ndex", "Shift+F1", "help.index"),
                it("~T~opic Search", "Ctrl+F1", "lsp.hover"),
                sep(),
                it("Download ~A~ll Guides", "", "help.download_all"),
                sep(),
                it("~K~eyboard...", "", "help.keys"),
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
        let (guide_tx, guide_rx) = std::sync::mpsc::channel();
        let mut wb = Workbench {
            theme: Theme::default(),
            menubar: MenuBar::new(menus()),
            status: StatusBar {
                items: vec![
                    StatusItem::new("F1", "Help", "help.contents"),
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
            file_dlg: None,
            overwrite_q: None,
            recent_dirs: Vec::new(),
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
            guide_tx,
            guide_rx,
            guides_fetching: 0,
            ai: None,
            tab_complete: true,
            last_edit: None,
            fim_pending: None,
            chat_history: Vec::new(),
            chat_streaming: None,
            chat_model: None,
            pending_question: None,
            agent: None,
            pending_task: None,
            socket_path: None,
            look_path: None,
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

    /// The system clipboard's text, from a front end that has one (the Qt
    /// app), so pasting in Turbine pastes what the desktop copied.
    pub fn set_clipboard(&mut self, text: String) {
        if !text.is_empty() {
            self.clipboard = text;
        }
    }

    /// The menu model, for native menu bars (the desktop's global menu).
    pub fn menus(&self) -> &[Menu] {
        &self.menubar.menus
    }

    /// The active window's title (for a desktop window's title).
    pub fn title(&self) -> String {
        self.windows.last().map(|w| w.win.title.clone()).unwrap_or_default()
    }

    /// A control style, and the rest of its look when it has one (QBasic:
    /// its colours, no line numbers, editors filling the screen).
    pub fn apply_style(&mut self, style: ControlStyle) {
        if let Some(name) = style.scheme {
            if let Some(sc) = Scheme::all().into_iter().find(|s| s.name == name) {
                self.theme.scheme = sc;
            }
        }
        let numbers = style.line_numbers;
        let full = style.dos_status;
        self.theme.style = style;
        let desk = self.desktop();
        for w in &mut self.windows {
            if w.special.is_none() && w.term.is_none() {
                w.ed.line_numbers = numbers;
            }
            // QBasic's editors fill the screen (Window > Tile still splits them).
            if full && w.special.is_none() && !w.win.zoomed {
                w.win.toggle_zoom(desk);
            }
        }
        self.sync_menu_checks();
    }

    /// Where the chosen look is kept (set by the front end; tests keep none).
    pub fn set_look_path(&mut self, p: PathBuf) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if let Some(st) = v["style"].as_str().and_then(|n| ControlStyle::all().into_iter().find(|s| s.name == n)) {
                    self.apply_style(st);
                }
                if let Some(sc) = v["scheme"].as_str().and_then(|n| Scheme::all().into_iter().find(|s| s.name == n)) {
                    self.theme.scheme = sc;
                }
            }
        }
        self.look_path = Some(p);
    }

    fn save_look(&self) {
        if let Some(p) = &self.look_path {
            let v = json!({"style": self.theme.style.name, "scheme": self.theme.scheme.name});
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(p, serde_json::to_string_pretty(&v).unwrap_or_default());
        }
    }

    /// An agent (or a question for it) is still at work.
    pub fn agent_running(&self) -> bool {
        self.agent.is_some() || self.pending_task.is_some() || self.chat_streaming.is_some() || self.pending_question.is_some()
    }

    pub fn set_socket_path(&mut self, p: PathBuf) {
        self.socket_path = Some(p);
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
        let mut ed = EditorView::new(doc);
        ed.line_numbers = self.theme.style.line_numbers;
        let mut ew = EdWin { win, ed, vim: None, term: None, term_text: String::new(), lang: None, lang_for: None, hl_sent: None, lsp_key: None, lsp_sent: None, was_dirty: false, special: None };
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

    /// Open a language's guide in a Help window (fetched in the background
    /// the first time, then kept for offline reading).
    fn open_guide(&mut self, key: &str) {
        use turbine_services::guides;
        let Some(g) = guides::find(key) else {
            self.say(format!("No guide for {key}"));
            return;
        };
        let (key, title) = (g.key().to_string(), g.title.to_string());
        if let Some(i) = self.windows.iter().position(|w| w.special == Some("help") && w.win.title == format!("Help: {title}")) {
            let w = self.windows.remove(i);
            self.windows.push(w);
            self.renumber();
            return;
        }
        self.show_help(&title, &format!("# {title}\n\nFetching this guide from its site... (it's kept for offline reading after this)\n"));
        if !guides::is_kept(&key) {
            self.say(format!("Fetching the {title} guide..."));
        }
        let tx = self.guide_tx.clone();
        self.guides_fetching += 1;
        std::thread::spawn(move || {
            let r = guides::guide_markdown(&key, false);
            let _ = tx.send((key, title, r));
        });
    }

    /// Show Markdown in a read-only Help window (one per title).
    fn show_help(&mut self, title: &str, md: &str) {
        let name = format!("Help: {title}");
        let i = match self.windows.iter().position(|w| w.special == Some("help") && w.win.title == name) {
            Some(i) => i,
            None => {
                let d = self.desktop();
                let mut win = Window::new(self.next_id, &name, Rect::new(d.x + 2, d.y + 1, (d.w - 4).max(20), (d.h - 2).max(6)));
                win.min_h = 4;
                self.next_id += 1;
                let mut ed = EditorView::new(Document::new());
                ed.line_numbers = false;
                self.windows.push(EdWin {
                    win,
                    ed,
                    vim: None,
                    term: None,
                    term_text: String::new(),
                    lang: Some("markdown".into()),
                    lang_for: Some(None),
                    hl_sent: None,
                    lsp_key: None,
                    lsp_sent: None,
                    was_dirty: false,
                    special: Some("help"),
                });
                self.renumber();
                self.windows.len() - 1
            }
        };
        let w = &mut self.windows[i];
        w.ed.doc = Document::from_str(md);
        w.ed.cursor = 0;
        w.ed.scroll_y = 0;
        w.hl_sent = None;
    }

    fn guides_index(&mut self) {
        use turbine_services::guides;
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        let mut keys = Vec::new();
        for g in guides::GUIDES {
            if seen.insert(g.key()) {
                items.push(format!("{}{}", if guides::is_kept(g.key()) { "√ " } else { "  " }, g.title));
                keys.push(g.key().to_string());
            }
        }
        let h = (self.size.1 - 4).clamp(10, 24);
        let w = (self.size.0 - 4).clamp(40, 76);
        let d = Dialog::new(&format!("guides:{}", keys.join("\u{1f}")), "Language Guides (√ = kept offline)", w, h)
            .list("guides", Rect::new(0, 0, w - 18, h - 3), items, 0)
            .button("ok", w - 16, 1, "~O~pen", true)
            .button("cancel", w - 16, 4, "Cancel", false);
        self.dialog = Some(d);
    }

    fn update_guides(&mut self) {
        let done: Vec<_> = self.guide_rx.try_iter().collect();
        for (_key, title, r) in done {
            self.guides_fetching = self.guides_fetching.saturating_sub(1);
            match r {
                Ok(md) => self.show_help(&title, &md),
                Err(e) => self.show_help(&title, &format!("# {title}\n\nThis guide couldn't be fetched:\n\n    {e}\n")),
            }
        }
    }

    /// Connect to the AI models (hearth's hub when it's there).
    pub fn start_ai(&mut self) {
        if self.ai.is_none() {
            self.ai = Some(Ai::start());
        }
    }

    #[allow(dead_code)] // tests
    pub fn set_ai(&mut self, ai: Ai) {
        self.ai = Some(ai);
    }

    fn chat_model(&self) -> Option<ai::Model> {
        let models = &self.ai.as_ref()?.models;
        if let Some(name) = &self.chat_model {
            if let Some(m) = models.iter().find(|m| &m.name == name) {
                return Some(m.clone());
            }
        }
        ai::pick_chat(models).cloned()
    }

    fn ask_dialog(&mut self) {
        if self.ai.is_none() {
            self.start_ai();
        }
        let has_sel = self.windows.last().is_some_and(|w| w.ed.selection().is_some());
        let title = if has_sel { "Ask AI about the selection" } else { "Ask AI" };
        let w = (self.size.0 - 6).clamp(40, 76);
        let d = Dialog::new("ask", title, w, 8)
            .input("q", 1, 2, w - 6, "~Q~uestion:", "")
            .button("ok", w / 2 - 12, 4, "~A~sk", true)
            .button("cancel", w / 2 + 2, 4, "Cancel", false);
        self.dialog = Some(d);
    }

    /// The code the question is about: the selection, else lines around the cursor.
    fn chat_context(&self) -> String {
        let Some(w) = self.windows.iter().rev().find(|w| w.special.is_none() && w.term.is_none()) else { return String::new() };
        let name = w.ed.doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| w.ed.doc.title());
        let lang = w.lang.clone().unwrap_or_default();
        let code = match w.ed.selected_text() {
            Some(t) => t,
            None => {
                let (line, _) = w.ed.cursor_line_col();
                let from = line.saturating_sub(60);
                let to = (line + 60).min(w.ed.doc.len_lines());
                (from..to).map(|i| w.ed.doc.line(i)).collect::<Vec<_>>().join("\n")
            }
        };
        format!("File: {name}\n```{lang}\n{code}\n```")
    }

    fn ask(&mut self, q: &str) {
        let Some(model) = self.chat_model() else {
            // The model list may not be here yet: ask when it is.
            self.pending_question = Some(q.to_string());
            self.say("Waiting for the AI model list...");
            return;
        };
        if self.chat_history.is_empty() {
            self.chat_history.push(ChatMessage {
                role: "system".into(),
                content: "You are the assistant inside Turbine, a retro DOS-style code editor. Answer briefly, in Markdown, with code in fenced blocks.".into(),
            });
        }
        let ctx = self.chat_context();
        self.chat_history.push(ChatMessage { role: "user".into(), content: format!("{q}\n\n{ctx}") });
        self.append_chat(&format!("\n## You\n\n{q}\n\n## {}\n\n", model.name));
        let msgs = self.chat_history.clone();
        let id = self.ai.as_mut().unwrap().chat(&model, msgs);
        self.chat_streaming = Some((id, String::new()));
    }

    fn append_chat(&mut self, text: &str) {
        let i = match self.windows.iter().position(|w| w.special == Some("chat")) {
            Some(i) => i,
            None => {
                let d = self.desktop();
                let mut win = Window::new(self.next_id, "AI Chat", Rect::new(d.x + d.w / 2, d.y, d.w - d.w / 2, d.h));
                win.min_h = 4;
                self.next_id += 1;
                let mut ed = EditorView::new(Document::from_str("# AI Chat\n"));
                ed.line_numbers = false;
                self.windows.push(EdWin {
                    win,
                    ed,
                    vim: None,
                    term: None,
                    term_text: String::new(),
                    lang: Some("markdown".into()),
                    lang_for: Some(None),
                    hl_sent: None,
                    lsp_key: None,
                    lsp_sent: None,
                    was_dirty: false,
                    special: Some("chat"),
                });
                // (Behind the active window: the log never takes focus.)
                let n = self.windows.len();
                if n >= 2 {
                    self.windows.swap(n - 1, n - 2);
                }
                self.renumber();
                self.windows.len().saturating_sub(2).min(n - 1)
            }
        };
        let ed = &mut self.windows[i].ed;
        let end = ed.doc.rope.len_chars();
        ed.doc.insert(end, text, end);
        ed.doc.dirty = false;
        ed.cursor = ed.doc.rope.len_chars();
    }

    fn agent_dialog(&mut self) {
        self.start_ai();
        let w = (self.size.0 - 6).clamp(40, 76);
        let d = Dialog::new("agent", "Agent: what should I do?", w, 8)
            .input("task", 1, 2, w - 6, "~T~ask:", "")
            .button("ok", w / 2 - 12, 4, "~S~tart", true)
            .button("cancel", w / 2 + 2, 4, "Cancel", false);
        self.dialog = Some(d);
    }

    fn start_agent(&mut self, task: &str) {
        let Some(model) = self.chat_model() else {
            self.pending_task = Some(task.to_string());
            self.say("Waiting for the AI model list...");
            return;
        };
        self.append_chat(&format!("\n## Agent task\n\n{task}\n\n*({} is working; Esc stops it)*\n\n", model.name));
        let messages = vec![json!({"role": "system", "content": AGENT_PROMPT}), json!({"role": "user", "content": task})];
        let id = self.ai.as_mut().unwrap().chat_with_tools(&model, messages.clone(), crate::tools::openai_tools());
        self.driver.agent_name = format!("Agent ({})", model.name);
        self.agent = Some(AgentRun {
            model,
            messages,
            waiting: Some(id),
            calls: Default::default(),
            current: None,
            approving: None,
            terminal_ok: false,
            turns: 0,
        });
    }

    fn stop_agent(&mut self, why: &str) {
        if self.agent.take().is_some() {
            self.driver.agent_name = "Agent".into();
            self.append_chat(&format!("\n*(Agent stopped: {why})*\n"));
        }
    }

    /// The model answered a turn: run its tool calls, or it's done.
    fn agent_message(&mut self, id: u64, message: Value) {
        let Some(a) = &mut self.agent else { return };
        if a.waiting != Some(id) {
            return;
        }
        a.waiting = None;
        a.turns += 1;
        a.messages.push(message.clone());
        let text = message["content"].as_str().unwrap_or("").trim().to_string();
        let calls: Vec<(String, String, Value)> = message["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, c)| {
                let f = &c["function"];
                let args = match &f["arguments"] {
                    Value::String(s) => serde_json::from_str(s).unwrap_or(json!({})),
                    v => v.clone(),
                };
                (c["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("call{i}")), f["name"].as_str().unwrap_or("").to_string(), args)
            })
            .collect();
        let done = calls.is_empty();
        let too_long = a.turns > 40;
        a.calls.extend(calls);
        if !text.is_empty() {
            self.append_chat(&format!("{text}\n\n"));
        }
        if done {
            self.agent = None;
            self.driver.agent_name = "Agent".into();
            self.append_chat("*(Agent finished)*\n");
        } else if too_long {
            self.stop_agent("too many steps");
        }
    }

    /// Run the next tool call, collect results, and ask the model again.
    fn agent_step(&mut self) {
        let Some(a) = &mut self.agent else { return };
        if a.approving.is_some() || a.waiting.is_some() {
            return;
        }
        if let Some((call_id, name, rx)) = &a.current {
            let Ok(reply) = rx.try_recv() else { return };
            let text = crate::tools::result_text(name, &reply);
            let short = if reply["ok"] == true { if name == "look" || name == "read" { "seen".to_string() } else { "ok".to_string() } } else { text.clone() };
            a.messages.push(json!({"role": "tool", "tool_call_id": call_id, "content": text}));
            a.current = None;
            self.append_chat(&format!("   → {short}\n"));
            return;
        }
        if let Some((call_id, name, args)) = a.calls.pop_front() {
            // A command typed into a terminal waits for the person's yes.
            let in_terminal = self.windows.last().is_some_and(|w| w.term.is_some());
            let runs = match name.as_str() {
                "type" => args["text"].as_str().is_some_and(|t| t.contains('\n')),
                "key" => args["key"].as_str().is_some_and(|k| k.eq_ignore_ascii_case("enter")),
                _ => false,
            };
            if in_terminal && runs && !a.terminal_ok {
                let what = args["text"].as_str().unwrap_or("Enter").trim().to_string();
                a.approving = Some((call_id, name, args));
                let w = (self.size.0 - 6).clamp(40, 76);
                let d = Dialog::new("agent_approve", "Agent wants to run a command", w, 9)
                    .label(1, 1, "In the terminal:")
                    .label(3, 2, &what.chars().take((w - 8) as usize).collect::<String>())
                    .button("allow", 2, 4, "~A~llow", true)
                    .button("always", 14, 4, "Allow A~l~ways", false)
                    .button("deny", 32, 4, "~D~eny", false);
                self.dialog = Some(d);
                return;
            }
            self.agent_run_call(call_id, name, args);
            return;
        }
        // Every call answered: the model's next turn.
        let (model, msgs) = (a.model.clone(), a.messages.clone());
        let id = self.ai.as_mut().map(|ai| ai.chat_with_tools(&model, msgs, crate::tools::openai_tools()));
        if let (Some(a), Some(id)) = (&mut self.agent, id) {
            a.waiting = Some(id);
        }
    }

    fn agent_run_call(&mut self, call_id: String, name: String, args: Value) {
        let shown = match name.as_str() {
            "look" | "read" => String::new(),
            _ => args.as_object().map(|o| o.values().map(|v| v.as_str().map(|s| format!("{s:?}")).unwrap_or_else(|| v.to_string())).collect::<Vec<_>>().join(" ")).unwrap_or_default(),
        };
        self.append_chat(&format!("▶ {name} {shown}\n"));
        let (tx, rx) = std::sync::mpsc::channel();
        match crate::tools::to_command(&name, &args) {
            Ok(cmd) => self.request(cmd, tx),
            Err(e) => {
                let _ = tx.send(json!({"ok": false, "error": e}));
            }
        }
        if let Some(a) = &mut self.agent {
            a.current = Some((call_id, name, rx));
        }
    }

    fn models_dialog(&mut self) {
        self.start_ai();
        let Some(a) = &self.ai else { return };
        if a.models.is_empty() {
            self.say("No AI models found yet (looking...)");
            return;
        }
        let cur = self.chat_model().map(|m| m.name);
        let items: Vec<String> = a
            .models
            .iter()
            .map(|m| {
                let tags = [m.loaded.then_some("loaded"), m.fim.then_some("tab completion"), (!m.chat).then_some("no chat")]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}  [{}{}]", m.name, m.provider, if tags.is_empty() { String::new() } else { format!("; {tags}") })
            })
            .collect();
        let sel = a.models.iter().position(|m| Some(&m.name) == cur.as_ref()).unwrap_or(0);
        let w = (self.size.0 - 4).clamp(40, 78);
        let h = (items.len() as i32 + 7).clamp(9, self.size.1 - 2);
        let d = Dialog::new("models", &format!("AI Models ({})", a.source), w, h)
            .label(1, 0, "Chat model:")
            .list("models", Rect::new(1, 1, w - 18, h - 5), items, sel)
            .button("ok", w - 15, 1, "O~K~", true)
            .button("cancel", w - 15, 4, "Cancel", false);
        self.dialog = Some(d);
    }

    /// Ghost-text completion after typing pauses, and AI answers.
    fn update_ai(&mut self, now: Instant) {
        let Some(a) = &mut self.ai else { return };
        let events = a.poll();
        for e in events {
            match e {
                AiEvent::Models(models) => {
                    if let Some(t) = self.pending_task.take() {
                        self.start_agent(&t);
                    }
                    if let Some(q) = self.pending_question.take() {
                        if models.iter().any(|m| m.chat) {
                            self.ask(&q);
                        } else {
                            self.say("No chat model: start Ollama or llama.cpp (hearth's AI hub lists them)");
                        }
                    }
                }
                AiEvent::HubGone(why) => self.say(format!("AI hub: {why}")),
                AiEvent::Chunk { id, text } => {
                    if self.chat_streaming.as_ref().is_some_and(|(i, _)| *i == id) {
                        self.chat_streaming.as_mut().unwrap().1.push_str(&text);
                        self.append_chat(&text);
                    }
                }
                AiEvent::Done { id } => {
                    if let Some((i, answer)) = self.chat_streaming.take() {
                        if i == id {
                            self.chat_history.push(ChatMessage { role: "assistant".into(), content: answer });
                            self.append_chat("\n");
                        } else {
                            self.chat_streaming = Some((i, answer));
                        }
                    }
                }
                AiEvent::Message { id, message } => self.agent_message(id, message),
                AiEvent::Error { id, message } => {
                    if self.agent.as_ref().is_some_and(|a| a.waiting == Some(id)) {
                        self.agent = None;
                        self.driver.agent_name = "Agent".into();
                        self.append_chat(&format!("\n*(Agent stopped: the model didn't answer: {message})*\n"));
                        continue;
                    }
                    if self.chat_streaming.as_ref().is_some_and(|(i, _)| *i == id) {
                        self.chat_streaming = None;
                        self.append_chat(&format!("\n*(The model didn't answer: {message})*\n"));
                    } else if self.fim_pending.as_ref().is_some_and(|p| p.0 == id) {
                        self.fim_pending = None;
                    }
                }
                AiEvent::Completion { id, text } => {
                    let Some((rid, win, version, cursor)) = self.fim_pending.take() else { continue };
                    if rid != id {
                        self.fim_pending = Some((rid, win, version, cursor));
                        continue;
                    }
                    if let Some(w) = self.windows.last_mut() {
                        if w.win.id == win && w.ed.doc.version == version && w.ed.cursor == cursor {
                            let ghost: String = text.trim_end().lines().take(3).collect::<Vec<_>>().join("\n");
                            if !ghost.trim().is_empty() {
                                w.ed.ghost = Some(ghost);
                            }
                        }
                    }
                }
            }
        }
        self.agent_step();
        // Ask for a completion once typing has paused.
        let Some((at, win, version)) = self.last_edit else { return };
        if now.duration_since(at) < Duration::from_millis(350) || self.fim_pending.is_some() {
            return;
        }
        self.last_edit = None;
        let Some(w) = self.windows.last() else { return };
        if w.win.id != win || w.ed.doc.version != version || w.ed.selection().is_some() {
            return;
        }
        let Some(a) = &mut self.ai else { return };
        let Some(model) = ai::pick_fim(&a.models).cloned() else { return };
        let rope = &w.ed.doc.rope;
        let cur = w.ed.cursor;
        let before = rope.slice(cur.saturating_sub(3000)..cur).to_string();
        let after = rope.slice(cur..(cur + 1500).min(rope.len_chars())).to_string();
        let id = a.complete(&model, before, after);
        self.fim_pending = Some((id, win, version, cur));
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
            // (Help windows say their language themselves.)
            let fixed = matches!(w.special, Some("help" | "chat"));
            let stale = !fixed && (w.lang_for.as_ref() != Some(&path) || (path.is_none() && w.hl_sent != Some(w.ed.doc.version)));
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
        let env = self.socket_path.iter().map(|p| ("TURBINE_SOCKET".to_string(), p.display().to_string())).collect();
        let view = TerminalView::new(Spawn { argv, env, ..Default::default() });
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
        let tab = self.tab_complete;
        for m in &mut self.menubar.menus {
            for it in &mut m.items {
                match it.command.as_str() {
                    "view.line_numbers" => it.checked = Some(ln),
                    "view.rtl" => it.checked = Some(rtl),
                    "ai.tab" => it.checked = Some(tab),
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
            "ai.chat" => self.ask_dialog(),
            "ai.agent" => self.agent_dialog(),
            "ai.models" => self.models_dialog(),
            "ai.tab" => {
                self.tab_complete = !self.tab_complete;
                if self.tab_complete {
                    self.start_ai();
                }
                self.say(if self.tab_complete { "AI tab completion on" } else { "AI tab completion off" });
            }
            "help.contents" => {
                let lang = self.windows.last().and_then(|w| w.lang.clone()).filter(|_| self.windows.last().is_some_and(|w| w.special.is_none()));
                match lang.filter(|l| turbine_services::guides::find(l).is_some()) {
                    Some(l) => self.open_guide(&l),
                    None => self.run_command("help.keys"),
                }
            }
            "help.index" => self.guides_index(),
            "help.download_all" => {
                let tx = self.guide_tx.clone();
                self.say("Downloading every language guide in the background...");
                self.guides_fetching += 1;
                std::thread::spawn(move || {
                    let (n, failed) = turbine_services::guides::download_all(|_, _, _| {});
                    let msg = format!("# Language guides\n\n{n} guides fetched and kept for offline reading.\n\n{}", if failed.is_empty() { String::new() } else { format!("Couldn't fetch:\n\n{}\n", failed.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n")) });
                    let _ = tx.send(("all".into(), "Download Report".into(), Ok(msg)));
                });
            }
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
                self.stop_agent("stopped from the menu");
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
        let Some(w) = self.windows.last() else { return };
        if ask || w.ed.doc.path.is_none() {
            let doc = &w.ed.doc;
            let dir = doc.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf));
            let name = doc.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let fmt = (Some((doc.encoding, doc.bom)), doc.line_ending);
            self.file_dialog(true, dir, &name, fmt);
            return;
        }
        let w = self.windows.last_mut().unwrap();
        let r = w.ed.doc.save();
        let msg = match r {
            Ok(()) => format!("Saved {} ({}, {})", w.ed.doc.title(), w.ed.doc.encoding.name(), w.ed.doc.line_ending.name()),
            Err(e) => format!("Save failed: {e}"),
        };
        self.say(msg);
    }

    fn open_dialog(&mut self, dir: PathBuf) {
        self.file_dialog(false, Some(dir), "", (None, LineEnding::Lf));
    }

    /// Open the Open (`save` false) or Save As dialog.
    fn file_dialog(&mut self, save: bool, dir: Option<PathBuf>, name: &str, fmt: (Option<(TextEncoding, bool)>, LineEnding)) {
        let cur = self.windows.last().and_then(|w| w.ed.doc.path.clone());
        let dir = dir
            .or_else(|| cur.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)))
            .or_else(|| self.recent_dirs.first().cloned())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        let project = cur.as_deref().map(turbine_services::lsp::project_root).or_else(|| std::env::current_dir().ok());
        let (f, d) = FileDialog::new(save, dir, name, project, &self.recent_dirs, fmt.0, fmt.1, self.size);
        self.file_dlg = Some(f);
        self.dialog = Some(d);
    }

    fn remember_dir(&mut self, p: &Path) {
        if let Some(d) = p.parent() {
            self.recent_dirs.retain(|x| x != d);
            self.recent_dirs.insert(0, d.to_path_buf());
            self.recent_dirs.truncate(8);
        }
    }

    /// The file dialog reacting to its controls changing or a row picked.
    /// Returns true when the dialog should close and `finish` with OK.
    fn file_dialog_event(&mut self, r: &DialogResult) -> bool {
        let (Some(f), Some(d)) = (&mut self.file_dlg, &mut self.dialog) else { return false };
        match r {
            DialogResult::Changed(id) => match id.as_str() {
                "places" => {
                    if let Some((_, p)) = d.selected_index("places").and_then(|i| f.places.get(i)) {
                        let p = p.clone();
                        f.go(p, d);
                    }
                }
                "fname" | "type" | "hidden" => f.refresh(d),
                "fname:backspace" | "files:backspace" => f.up(d),
                "files" if f.save => {
                    // (Picking a file to save over fills in its name.)
                    if let Some(Some((p, false))) = f.selected(d) {
                        let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        d.set_value("fname", &n);
                    }
                }
                _ => {}
            },
            DialogResult::Activated(id) if id == "places" => {
                if let Some((_, p)) = d.selected_index("places").and_then(|i| f.places.get(i)) {
                    let p = p.clone();
                    f.go(p, d);
                    d.focus_id("files");
                }
            }
            DialogResult::Activated(id) if id == "files" => match f.selected(d) {
                Some(None) => f.up(d),
                Some(Some((p, true))) => f.go(p, d),
                Some(Some((p, false))) => {
                    let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    d.set_value("fname", &n);
                    return true;
                }
                None => {}
            },
            DialogResult::Button(b) if b == "ok" => {
                // A folder typed or selected is entered, not returned.
                if let Some(t) = f.target(d) {
                    if t.is_dir() {
                        f.go(t, d);
                        return false;
                    }
                }
                return true;
            }
            _ => {}
        }
        false
    }

    /// OK in the file dialog: open, or save (asking before replacing).
    fn finish_file_dialog(&mut self, overwrite_ok: bool) {
        let (Some(f), Some(d)) = (self.file_dlg.as_ref(), self.dialog.as_ref()) else { return };
        let Some(p) = f.target(d) else { return };
        if !f.save {
            self.dialog = None;
            self.file_dlg = None;
            self.remember_dir(&p);
            self.open_path(&p);
            return;
        }
        if d.value("fname").unwrap_or_default().trim().is_empty() {
            return;
        }
        if p.exists() && !overwrite_ok {
            // Ask over the file dialog; Yes comes back here.
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let q = Dialog::new("overwrite", "Replace File", 50, 7)
                .label(1, 1, &format!("{name} already exists. Replace it?"))
                .button("yes", 10, 3, "~Y~es", false)
                .button("cancel", 26, 3, "~N~o", true);
            self.overwrite_q = Some(q);
            return;
        }
        let fmt = f.format(d);
        self.dialog = None;
        self.file_dlg = None;
        self.remember_dir(&p);
        if let Some(w) = self.windows.last_mut() {
            if let Some((enc, bom, le)) = fmt {
                w.ed.doc.encoding = enc;
                w.ed.doc.bom = bom;
                w.ed.doc.line_ending = le;
            }
            let r = w.ed.doc.save_as(&p);
            w.win.title = p.display().to_string();
            w.lang_for = None;
            let msg = match r {
                Ok(()) => format!("Saved {} ({}, {})", p.display(), w.ed.doc.encoding.name(), w.ed.doc.line_ending.name()),
                Err(e) => format!("Save failed: {e}"),
            };
            self.say(msg);
        }
    }

    /// What a dialog's key, click or paste did.
    fn dialog_result(&mut self, r: DialogResult) {
        // (The overwrite question sits over the file dialog.)
        if self.overwrite_q.is_some() {
            match r {
                DialogResult::Button(b) if b == "yes" => {
                    self.overwrite_q = None;
                    self.finish_file_dialog(true);
                }
                DialogResult::Button(_) | DialogResult::Cancel => self.overwrite_q = None,
                _ => {}
            }
            return;
        }
        if self.file_dlg.is_some() && self.dialog.as_ref().is_some_and(|d| d.kind == "filedlg") {
            match r {
                DialogResult::Cancel | DialogResult::Button(_) if !matches!(&r, DialogResult::Button(b) if b == "ok") => {
                    self.dialog = None;
                    self.file_dlg = None;
                }
                _ => {
                    if self.file_dialog_event(&r) {
                        self.finish_file_dialog(false);
                    }
                }
            }
            return;
        }
        match r {
            DialogResult::None | DialogResult::Changed(_) => {}
            DialogResult::Cancel => {
                let d = self.dialog.take().unwrap();
                // (Closing the agent's question is a no.)
                if d.kind == "agent_approve" {
                    self.finish_dialog(d, Some("deny".into()));
                }
            }
            DialogResult::Button(b) => {
                let d = self.dialog.take().unwrap();
                self.finish_dialog(d, Some(b));
            }
            // (A list's Enter or double-click is its dialog's OK.)
            DialogResult::Activated(_) => {
                let d = self.dialog.take().unwrap();
                let ok = d.controls.iter().find_map(|p| match &p.control {
                    turbine_ui::dialog::Control::Button { id, default: true, .. } => Some(id.clone()),
                    _ => None,
                });
                self.finish_dialog(d, Some(ok.unwrap_or_else(|| "ok".into())));
            }
        }
    }

    /// The dialog on top: the overwrite question, else the dialog.
    fn top_dialog(&mut self) -> Option<&mut Dialog> {
        if self.overwrite_q.is_some() { self.overwrite_q.as_mut() } else { self.dialog.as_mut() }
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
        if kind == "agent" {
            let t = d.value("task").unwrap_or_default();
            if !t.trim().is_empty() {
                self.start_agent(t.trim());
            }
            return;
        }
        if kind == "agent_approve" {
            if let Some(a) = &mut self.agent {
                if let Some((call_id, name, args)) = a.approving.take() {
                    match b.as_str() {
                        "allow" | "always" => {
                            a.terminal_ok |= b == "always";
                            self.agent_run_call(call_id, name, args);
                        }
                        _ => {
                            a.messages.push(json!({"role": "tool", "tool_call_id": call_id, "content": "The person said no: don't run that."}));
                            self.append_chat("   → denied\n");
                        }
                    }
                }
            }
            return;
        }
        if kind == "ask" {
            let q = d.value("q").unwrap_or_default();
            if !q.trim().is_empty() {
                self.ask(q.trim());
            }
            return;
        }
        if kind == "models" {
            if let (Some(i), Some(a)) = (d.selected_index("models"), &self.ai) {
                if let Some(m) = a.models.get(i) {
                    self.chat_model = Some(m.name.clone());
                    self.say(format!("Chat model: {}", m.name));
                }
            }
            return;
        }
        if let Some(keys) = kind.strip_prefix("guides:") {
            if let Some(i) = d.selected_index("guides") {
                if let Some(k) = keys.split('\u{1f}').nth(i) {
                    let k = k.to_string();
                    self.open_guide(&k);
                }
            }
            return;
        }
        if let Some(root) = kind.strip_prefix("arguments:") {
            let program = PathBuf::from(d.value("program").unwrap_or_default().trim());
            let args: Vec<String> = d.value("args").unwrap_or_default().split_whitespace().map(str::to_string).collect();
            self.run_config.insert(PathBuf::from(root), (program, args));
            return;
        }
        match kind.as_str() {
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
                        self.save_look();
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
                        self.apply_style(s);
                        self.save_look();
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
                self.stop_agent("you pressed Esc");
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
                let old = self.desktop();
                self.size = (w, h);
                self.menubar.width = w;
                let d = self.desktop();
                // Windows keep their place and share of the desktop as it
                // grows or shrinks (zoomed ones fill it).
                for win in &mut self.windows {
                    let r = win.win.rect;
                    if old.w > 0 && old.h > 0 && !win.win.zoomed {
                        let sx = |v: i32| d.x + ((v - old.x) as i64 * d.w as i64 / old.w as i64) as i32;
                        let sy = |v: i32| d.y + ((v - old.y) as i64 * d.h as i64 / old.h as i64) as i32;
                        let (x0, y0, x1, y1) = (sx(r.x), sy(r.y), sx(r.right()), sy(r.bottom()));
                        win.win.rect = Rect::new(x0, y0, (x1 - x0).max(win.win.min_w), (y1 - y0).max(win.win.min_h));
                    }
                    win.win.fit(d);
                }
            }
            Event::Key(k) => self.key(k),
            Event::Mouse(m) => self.mouse(m),
            Event::Paste(s) => {
                if let Some(d) = self.top_dialog() {
                    let r = d.handle_paste(&s);
                    self.dialog_result(r);
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
        let before = self.windows.last().map(|w| (w.win.id, w.ed.doc.version));
        self.key_inner(k);
        if self.completion.is_some() {
            self.refilter_completion();
        }
        // A person typed in an editor: maybe suggest a completion soon.
        let after = self.windows.last().filter(|w| w.special.is_none() && w.term.is_none() && w.vim.is_none()).map(|w| (w.win.id, w.ed.doc.version));
        if self.tab_complete && !self.from_agent && after.is_some() && after != before {
            let (win, v) = after.unwrap();
            self.last_edit = Some((Instant::now(), win, v));
        } else if after != before {
            self.last_edit = None;
        }
    }

    fn key_inner(&mut self, k: KeyEvent) {
        if let Some(d) = self.top_dialog() {
            let r = d.handle_key(&k);
            self.dialog_result(r);
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
        let shortcut = if k.is_ctrl('i') && k.key != Key::Tab {
            Some("ai.agent")
        } else if k.is_ctrl('l') {
            Some("ai.chat")
        } else if k.is(Key::Char(' '), Mods::CTRL) {
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
            Some("help.contents")
        } else if k.is(Key::F(1), Mods::SHIFT) {
            Some("help.index")
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
        // Help, Debug and Output windows are read-only: only moving around.
        if self.windows.last().is_some_and(|w| w.special.is_some()) {
            let editing = matches!(k.key, Key::Char(_) | Key::Enter | Key::Backspace | Key::Delete | Key::Tab)
                && !(k.mods.ctrl && matches!(k.key, Key::Char('c' | 'C' | 'a' | 'A')));
            if editing {
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
        if let Some(d) = self.top_dialog() {
            let r = d.handle_mouse(&m);
            self.dialog_result(r);
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
            root = root.child(d.automation_node());
        }
        if let Some(q) = &self.overwrite_q {
            root = root.child(q.automation_node());
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

    /// One line on what has focus: for agents after each action.
    fn focus_summary(&self) -> String {
        if let Some(d) = &self.dialog {
            let f = d.controls.get(d.focus).map(|c| format!("{} {}", c.control.role(), c.control.id().unwrap_or(""))).unwrap_or_default();
            return format!("dialog \"{}\" is open (focus: {f})", d.title);
        }
        if let Some(i) = self.menubar.open {
            return format!("menu {} is open", self.menubar.menus[i].plain_title());
        }
        match self.windows.last() {
            None => "no windows open".into(),
            Some(w) if w.term.is_some() => format!("terminal window \"{}\" has focus", w.win.title),
            Some(w) => {
                let (l, c) = w.ed.cursor_line_col();
                let kind = w.special.unwrap_or("editor");
                format!(
                    "{kind} \"{}\" has focus, cursor at line {} col {} of {} lines{}",
                    w.win.title,
                    l + 1,
                    c + 1,
                    w.ed.doc.len_lines(),
                    if w.ed.doc.dirty { ", unsaved changes" } else { "" }
                )
            }
        }
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
                let mut v = json!({"ok": true, "tree": self.tree(), "focus": self.focus_summary()});
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
                Output::Reply(tx, mut v) => {
                    // Every finished action says where focus is now.
                    if v["ok"] == true {
                        v["focus"] = json!(self.focus_summary());
                    }
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
        self.update_guides();
        self.update_ai(now);
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
        let lsp_waiting = self.lsp.values().any(|c| c.waiting() > 0)
            || self.debug.is_some()
            || self.guides_fetching > 0
            || self.fim_pending.is_some()
            || self.chat_streaming.is_some()
            || self.pending_question.is_some()
            || self.agent.is_some()
            || self.pending_task.is_some()
            || self.last_edit.is_some();
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
        // (The style decides how the bar and status line are laid out.)
        self.menubar.help_right = theme.style.help_right;
        self.menubar.hotkeys_when_active = theme.style.hotkeys_when_active;
        self.menubar.hide_system = theme.style.hide_system_menu;
        self.status.dos = theme.style.dos_status;
        self.status.rules = theme.style.status_rules;
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
                // QBasic/Edit: just the position, as 00001:001.
                if self.theme.style.dos_status {
                    return format!("{:05}:{:03}", l + 1, col + 1);
                }
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
        if let (Some(f), Some(d)) = (&mut self.file_dlg, &mut self.dialog) {
            d.layout(screen);
            f.fit(d);
        }
        for d in [&mut self.dialog, &mut self.overwrite_q].into_iter().flatten() {
            d.layout(screen);
            d.draw(&mut c, &theme);
            d.draw_overlays(&mut c, &theme);
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
    fn file_dialog_opens_and_saves_as() {
        let dir = std::env::temp_dir().join(format!("turbine-fdlg-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/hello.bas"), "PRINT \"HI\"\r\n").unwrap();
        let mut wb = bench();
        wb.open_dialog(dir.clone());
        // Into "sub/" by typing its name and Enter, then a file by filter.
        run(&mut wb, json!({"cmd": "type", "text": "sub"}));
        keys(&mut wb, &["Enter"]);
        assert!(wb.file_dlg.as_ref().unwrap().dir.ends_with("sub"));
        run(&mut wb, json!({"cmd": "type", "text": "hel"}));
        keys(&mut wb, &["Enter"]);
        assert!(wb.dialog.is_none());
        assert_eq!(run(&mut wb, json!({"cmd": "text"}))["text"], "PRINT \"HI\"\n");
        // Save As over an existing file asks first; CP437 + CRLF chosen.
        std::fs::write(dir.join("out.txt"), "old").unwrap();
        wb.save(true);
        let d = wb.dialog.as_mut().unwrap();
        assert_eq!(d.value("fname").as_deref(), Some("hello.bas"));
        d.set_value("fname", &dir.join("out.txt").display().to_string());
        for p in &mut d.controls {
            if let turbine_ui::dialog::Control::Combo { id, selected, .. } = &mut p.control {
                *selected = if id == "encoding" { 4 } else { 1 };
            }
        }
        keys(&mut wb, &["Enter"]);
        assert!(wb.overwrite_q.is_some());
        keys(&mut wb, &["Alt+Y"]);
        assert!(wb.dialog.is_none() && wb.overwrite_q.is_none());
        assert_eq!(std::fs::read(dir.join("out.txt")).unwrap(), b"PRINT \"HI\"\r\n");
        assert_eq!(wb.recent_dirs.first(), Some(&std::fs::canonicalize(&dir).unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
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
    fn help_windows_are_read_only_markdown_and_the_index_lists_guides() {
        let mut wb = bench();
        wb.show_help("Test Guide", "# Title\n\nSome `code` here.\n");
        assert_eq!(wb.windows.last().unwrap().win.title, "Help: Test Guide");
        wait_until(&mut wb, "markdown highlighting", |wb| !wb.windows.last().unwrap().ed.highlights.is_empty());
        assert_eq!(wb.windows.last().unwrap().lang.as_deref(), Some("markdown"));
        run(&mut wb, json!({"cmd": "type", "text": "xyz"}));
        assert!(wb.windows.last().unwrap().ed.doc.line(0) == "# Title", "typing doesn't change help");
        keys(&mut wb, &["Shift+F1"]);
        let d = wb.dialog.as_ref().expect("guide index");
        assert!(d.kind.starts_with("guides:"));
        assert!(d.kind.contains("rust") && d.kind.contains("python"));
    }

    /// A pretend model server: streamed chat, and Ollama-style FIM.
    fn fake_model_server() -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for conn in l.incoming().flatten() {
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let mut w = conn;
                if first.contains("/v1/chat/completions") {
                    let chunk = |t: &str| format!("data: {}\n\n", json!({"choices": [{"delta": {"content": t}}]}));
                    let out = format!("{}{}{}data: [DONE]\n\n", chunk("Hello"), chunk(" <think>hmm</think>"), chunk("world"));
                    let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len());
                } else {
                    let out = json!({"response": "println!(\"hi\");\n"}).to_string();
                    let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len());
                }
            }
        });
        addr
    }

    #[test]
    fn ai_chat_streams_and_tab_completion_ghosts() {
        let addr = fake_model_server();
        let mut wb = bench();
        wb.set_ai(Ai::with_models(vec![ai::Model {
            provider: "ollama-test".into(),
            name: "test-model".into(),
            base: format!("http://{addr}/v1"),
            key: None,
            loaded: true,
            fim: true,
            chat: true,
        }]));
        // Chat: the answer streams into the AI Chat window, thinking left out.
        wb.ask("what does this do?");
        wait_until(&mut wb, "chat answer", |wb| wb.chat_streaming.is_none());
        let chat = wb.windows.iter().find(|w| w.special == Some("chat")).unwrap().ed.doc.rope.to_string();
        assert!(chat.contains("## You\n\nwhat does this do?"), "{chat}");
        assert!(wb.windows.last().unwrap().special.is_none(), "the chat window doesn't take focus");
        assert!(chat.contains("Hello world"), "{chat}");
        assert_eq!(wb.chat_history.last().unwrap().content, "Hello world");

        // Tab completion: a person types, pauses, and a ghost appears; Tab takes it.
        let ed = wb.windows.iter().position(|w| w.special.is_none()).unwrap();
        let w = wb.windows.remove(ed);
        wb.windows.push(w);
        for c in "fn main() { ".chars() {
            wb.key(KeyEvent::plain(Key::Char(c)));
        }
        wait_until(&mut wb, "ghost text", |wb| wb.windows.last().unwrap().ed.ghost.is_some());
        assert_eq!(wb.windows.last().unwrap().ed.ghost.as_deref(), Some("println!(\"hi\");"));
        wb.key(KeyEvent::plain(Key::Tab));
        assert_eq!(wb.windows.last().unwrap().ed.doc.line(0), "fn main() { println!(\"hi\");");
    }

    /// A pretend tool-calling model: answers each chat turn from a script,
    /// and keeps the requests it got.
    fn fake_agent_server(script: Vec<Value>) -> (String, std::sync::Arc<std::sync::Mutex<Vec<Value>>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            let mut turn = 0;
            for conn in l.incoming().flatten() {
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                seen2.lock().unwrap().push(serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null));
                let msg = script.get(turn).cloned().unwrap_or(json!({"role": "assistant", "content": "Done."}));
                turn += 1;
                let out = json!({"choices": [{"message": msg}]}).to_string();
                let mut w = conn;
                let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len());
            }
        });
        (addr, seen)
    }

    fn call(id: &str, name: &str, args: Value) -> Value {
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}]})
    }

    fn agent_bench(addr: &str) -> Workbench {
        let mut wb = bench();
        wb.set_ai(Ai::with_models(vec![ai::Model {
            provider: "test".into(),
            name: "agent-model".into(),
            base: format!("http://{addr}/v1"),
            key: None,
            loaded: true,
            fim: false,
            chat: true,
        }]));
        wb
    }

    #[test]
    fn agent_works_the_ui_with_tools() {
        let (addr, seen) = fake_agent_server(vec![
            call("c1", "look", json!({})),
            call("c2", "menu", json!({"path": "File/New Text File"})),
            call("c3", "type", json!({"text": "hello from the agent"})),
            json!({"role": "assistant", "content": "I wrote the greeting."}),
        ]);
        let mut wb = agent_bench(&addr);
        wb.start_agent("write a greeting in a new file");
        assert!(wb.driver.agent_name.contains("agent-model"));
        wait_until(&mut wb, "agent to finish", |wb| wb.agent.is_none());
        let code = wb.windows.iter().rev().find(|w| w.special.is_none()).unwrap();
        assert_eq!(code.ed.doc.rope.to_string(), "hello from the agent");
        let reqs = seen.lock().unwrap();
        assert_eq!(reqs.len(), 4);
        assert!(reqs[0]["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "look"));
        // The second turn saw the screen from look.
        let tool_msg = reqs[1]["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").unwrap();
        assert!(tool_msg["content"].as_str().unwrap().contains("Widgets:"), "{tool_msg}");
        let chat = wb.windows.iter().find(|w| w.special == Some("chat")).unwrap().ed.doc.rope.to_string();
        assert!(chat.contains("▶ menu \"File/New Text File\""), "{chat}");
        assert!(chat.contains("I wrote the greeting.") && chat.contains("Agent finished"), "{chat}");
    }

    #[test]
    fn agent_asks_before_running_a_terminal_command() {
        let (addr, seen) = fake_agent_server(vec![
            call("c1", "type", json!({"text": "echo nope\n"})),
            json!({"role": "assistant", "content": "OK, I won't."}),
        ]);
        let mut wb = agent_bench(&addr);
        wb.new_terminal(vec!["sh".into()]);
        wb.start_agent("run something");
        wait_until(&mut wb, "approval question", |wb| wb.dialog.as_ref().is_some_and(|d| d.kind == "agent_approve"));
        // The person says no (Esc).
        wb.key(KeyEvent::plain(Key::Esc));
        wait_until(&mut wb, "agent to finish", |wb| wb.agent.is_none());
        let reqs = seen.lock().unwrap();
        let tool_msg = reqs[1]["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").unwrap().clone();
        assert!(tool_msg["content"].as_str().unwrap().contains("said no"), "{tool_msg}");
        assert!(!wb.windows.last().unwrap().term_text.contains("nope\nnope"), "nothing ran");
    }

    #[test]
    fn qbasic_style_looks_like_qbasic() {
        let mut wb = bench();
        wb.dispatch(Event::Resize(80, 25));
        run(&mut wb, json!({"cmd": "menu", "path": "Options/Control Style..."}));
        run(&mut wb, json!({"cmd": "click", "target": "listitem:QBasic / MS-DOS Edit"}));
        run(&mut wb, json!({"cmd": "click", "target": "button:OK"}));
        assert_eq!(wb.theme.scheme.name, "QBasic");
        let mut surf = Surface::new(80, 25);
        wb.render(&mut surf);
        let text = surf.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].trim_end().ends_with("Help"), "Help at the right: {:?}", lines[0]);
        assert!(lines[1].starts_with("┌") && lines[1].contains(" UNTITLED ") && lines[1].contains("┤↑├"), "{:?}", lines[1]);
        assert!(lines[2].starts_with("│") && !lines[2].contains(" 1 "), "no line numbers: {:?}", lines[2]);
        assert!(lines[24].contains("<F1=Help>") && lines[24].contains("│ 00001:001"), "{:?}", lines[24]);
        assert!(text.contains('↓') && text.contains('→'), "arrow scroll bars");
        // The title is in reverse video (blue on grey).
        let x = lines[1].find(" UNTITLED ").map(|b| lines[1][..b].chars().count()).unwrap() as i32 + 1;
        assert_eq!(surf.get(x, 1).unwrap().style.bg, turbine_ui::theme::vga::LIGHT_GRAY);
    }

    #[test]
    fn vbdos_style_has_title_bars() {
        let mut wb = bench();
        wb.dispatch(Event::Resize(80, 25));
        wb.apply_style(turbine_ui::theme::ControlStyle::vbdos());
        assert_eq!(wb.theme.scheme.name, "VB-DOS");
        let mut surf = Surface::new(80, 25);
        wb.render(&mut surf);
        let text = surf.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[1].contains("[1] UNTITLED") && lines[1].contains('▼') && lines[1].contains('▲'), "{:?}", lines[1]);
        assert_eq!(surf.get(3, 1).unwrap().style.bg, turbine_ui::theme::vga::MAGENTA);
        assert!(lines[24].contains("F1=Help │"), "{:?}", lines[24]);
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
