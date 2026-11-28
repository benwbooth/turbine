//! The IDE itself: menu bar, desktop of editor windows, dialogs, status
//! bar, and the automation endpoint agents drive it through.

use crate::agent::{Driver, Output, Step};
use crate::dock::Side;
use crate::views::{Explorer, Extensions, LineInput, Outline, Scm, Search};
use crate::vim::{self, Keymap};
use turbine_text::unicode::str_width;
use turbine_ui::list::ListView;
use turbine_ui::theme::vga;
use turbine_ui::Canvas;

mod build;
mod chat_pickers;
mod debug_more;
mod docview;
mod explorer_ops;
mod extras;
mod extensions;
pub use extensions::extension_dirs;
mod find;
mod git;
mod gitlines;
mod groups;
mod images;
mod inline_ai;
mod lsp_more;
mod preview;
mod top_row;
mod scm;
mod sections;
mod session;
mod tooltip;
mod settings;
mod shell;
use find::FindBar;
use shell::{Focus, Shell};
use turbine_term::{MouseOutcome, Spawn, TerminalView};
use turbine_services::ai::{self, Ai, AiEvent, ChatMessage};
use turbine_services::dap::{self, DapClient, DapEvent, Frame, Launch, Variable};
use turbine_services::lsp::{self, CompletionItem, LspClient, LspEvent, Pos};
use std::collections::{BTreeMap, BTreeSet};
use turbine_ui::editor::DiagMark;
use turbine_ui::popup::{ListPopup, PopupResult};
use turbine_vi::{ExAction, Out as VimOut, Session};
use std::collections::{HashMap, HashSet};

/// Turbine's manual (Help > Contents).
const MANUAL: &str = include_str!("../../../docs/manual.md");

/// A language server is keyed by language and project folder.
type LspKey = (String, PathBuf);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LspReq {
    Hover,
    Definition,
    References,
    Rename,
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
            // In the project's environment (direnv's), from its PATH, else
            // from nixpkgs with `nix shell` (nothing to install).
            _ => {
                let env = match turbine_services::env::project_env(root) {
                    turbine_services::env::Env::Ready(e) => e,
                    turbine_services::env::Env::Loading => Default::default(),
                };
                let (argv, _from_nix) = lsp::server_in(lang, &env)?;
                return Some(LspClient::start_env(&argv, root, lang, &env));
            }
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
use turbine_ui::dialog::{self as ui, Dialog, DialogResult};
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
    /// A language chosen by hand (Change Language Mode): it wins.
    lang_fixed: Option<String>,
    hl_sent: Option<u64>,
    /// Language server: which one, the version it has, dirty last tick.
    lsp_key: Option<LspKey>,
    lsp_sent: Option<u64>,
    was_dirty: bool,
    /// "output" or "debug" for those windows.
    special: Option<&'static str>,
    /// Docked in a part (the panel, a side bar) instead of on the desktop.
    dock: Option<Side>,
    /// Windows showing the same document share a group: an edit in one
    /// shows in the others (a split of one file).
    mirror: Option<u32>,
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
    /// A window whose scroll bar thumb is being dragged.
    scroll_drag: Option<(usize, bool, usize)>,
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
    lspx: lsp_more::LspExtra,
    dbx: debug_more::DebugExtra,
    build: build::BuildState,
    preview: preview::PreviewState,
    git: git::GitState,
    extras: extras::Extras,
    /// File managers (XTree's look), by window.
    xtrees: HashMap<u32, crate::xtree::XTree>,
    /// Picture viewers, by window; where this frame's pictures go; and
    /// whether the front end can show them.
    viewers: HashMap<u32, images::Viewer>,
    image_places: Vec<(Rect, PathBuf)>,
    images_inline: bool,
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
    /// Editor groups (VS Code's split editors).
    groups: groups::Groups,
    /// How hard the chat's model thinks ("off" … "high"; None: its own way).
    chat_thinking: Option<String>,
    /// Where the chat's "thinking…" stands until the answer's first words.
    chat_wait: Option<usize>,
    /// The answer's thinking so far, and when it started (shown as it comes).
    chat_thought: (String, Option<Instant>),
    /// A question asked before the model list arrived.
    pending_question: Option<String>,
    agent: Option<AgentRun>,
    /// A task given before the model list arrived.
    pending_task: Option<String>,
    /// The automation socket, told to programs in terminals (TURBINE_SOCKET).
    socket_path: Option<PathBuf>,
    /// Where the chosen colours and style are kept.
    look_path: Option<PathBuf>,
    /// The docked parts and their views (Explorer, Search, ...).
    shell: Shell,
    /// Find and Replace in the editor.
    find: FindBar,
    /// The Explorer's file operations and right-click menu.
    ex: explorer_ops::ExOps,
    /// Ctrl+K: an inline AI edit in progress.
    inline: Option<inline_ai::InlineEdit>,
    /// WebAssembly extensions (started by the front end).
    exts: extensions::ExtState,
    session: session::SessionState,
    tip: tooltip::TipState,
    docviews: HashMap<u32, docview::DocView>,
    /// The editor's right-click menu: its list, where, each item's command.
    ed_menu: Option<(ListPopup, Point, Vec<&'static str>)>,
    /// What the front end's window should do (its own title bar's buttons):
    /// 1 minimize, 2 maximize or restore, 3 close, 4 start a move.
    window_op: i32,
    /// What git knows of each open file's lines (marks, blame).
    gitlines: gitlines::GitLines,
    /// Projects whose environment note was shown (loading, blocked).
    env_note: HashSet<PathBuf>,
    /// A status item's drop-up: its list, where, and each choice's action
    /// ("lang:rust", "enc:3", "eol:CRLF").
    status_menu: Option<(ListPopup, Point, Vec<String>)>,
    /// The whole window in cells (one row more than Turbine's own with its
    /// own title bar).
    outer: (i32, i32),
    /// The last key, click or move: for a moment after it, ticks come at
    /// screen rate, so what it started (a listing, colours, a preview)
    /// shows the moment it's ready.
    last_input: Option<Instant>,
}

/// The menu bar: VS Code's menus (File, Edit, Selection, View, Go, Run,
/// Terminal, Help), with Turbine's AI and Tools (Turbo Pascal's: the
/// programs that come with it, Git, extensions'), Borland's ≡ system menu,
/// and the DOS IDEs' items folded in: Search's in Edit, Options' under File
/// > Preferences, Window's under View > Editor Layout and Windows. Items
/// whose feature isn't built yet are shown disabled.
fn menus() -> Vec<Menu> {
    let it = MenuItem::new;
    let sep = MenuItem::separator;
    let sub = MenuItem::sub;
    vec![
        Menu::new(
            "≡",
            vec![
                it("~A~bout Turbine...", "", "help.about"),
                sep(),
                it("~R~epaint Desktop", "", "app.repaint"),
            ],
        ),
        Menu::new(
            "~F~ile",
            vec![
                it("~N~ew Text File", "Ctrl+N", "file.new"),
                it("New ~W~indow", "Ctrl+Shift+N", "file.new_window"),
                sep(),
                it("~O~pen File...", "F3", "file.open"),
                it("Open ~F~older...", "", "file.open_folder"),
                it("Add Folder to Wor~k~space...", "", "file.add_folder"),
                it("Open ~R~ecent", "", "file.recent"),
                sep(),
                it("~S~ave", "F2", "file.save"),
                it("Save ~A~s...", "", "file.save_as"),
                it("Save A~l~l", "", "file.save_all"),
                sep(),
                it("Re~v~ert File", "", "file.revert"),
                it("Reopen with ~E~ncoding...", "", "file.reopen_encoding"),
                it("Save with Enco~d~ing...", "", "file.save_encoding"),
                it("~C~lose Editor", "Alt+F3", "window.close"),
                sep(),
                sub(
                    "~P~references",
                    vec![
                        it("~S~ettings...", "Ctrl+,", "options.settings"),
                        it("~K~eymap...", "", "options.keymap"),
                        it("~C~olors...", "", "options.colors"),
                        it("Control S~t~yle...", "", "options.style"),
                        sep(),
                        it("~E~ditor...", "", "options.editor"),
                        it("~D~irectories...", "", "options.dirs"),
                        it("Keyboard S~h~ortcuts", "", "help.keys"),
                    ],
                ),
                sep(),
                it("DOS S~h~ell", "", "view.terminal"),
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
                it("~F~ind...", "Ctrl+F", "search.find"),
                it("R~e~place...", "Ctrl+H", "search.replace"),
                it("Search A~g~ain", "Enter", "search.again"),
                it("Find in F~i~les...", "Ctrl+Shift+F", "search.files"),
                sep(),
                it("Toggle Line C~o~mment", "Ctrl+/", "edit.comment"),
                it("Format ~D~ocument", "Ctrl+Shift+I", "edit.format"),
                sep(),
                it("Trigger ~S~uggest", "Ctrl+Space", "lsp.complete"),
                it("Parameter ~H~ints", "Ctrl+Shift+Space", "lsp.signature"),
                it("~Q~uick Fix...", "Ctrl+.", "edit.quick_fix"),
                it("Re~n~ame Symbol...", "Shift+F2", "edit.rename"),
                it("Change ~L~anguage Mode...", "", "edit.language"),
                it("Install Language Ser~v~er...", "", "lsp.install"),
            ],
        ),
        Menu::new(
            "Se~l~ection",
            vec![
                it("Select ~A~ll", "Ctrl+A", "edit.select_all"),
                it("~E~xpand Selection", "Shift+Alt+Right", "selection.expand"),
                it("~S~hrink Selection", "Shift+Alt+Left", "selection.shrink"),
                sep(),
                it("Copy Line ~U~p", "Shift+Alt+Up", "selection.copy_up"),
                it("Copy Line ~D~own", "Shift+Alt+Down", "selection.copy_down"),
                it("Mo~v~e Line Up", "Alt+Up", "selection.move_up"),
                it("Move Line Do~w~n", "Alt+Down", "selection.move_down"),
                sep(),
                it("Add Cursor A~b~ove", "Ctrl+Alt+Up", "selection.cursor_above"),
                it("Add Cursor Be~l~ow", "Ctrl+Alt+Down", "selection.cursor_below"),
                it("Add ~N~ext Match", "Ctrl+D", "selection.add_next"),
                it("~C~olumn Selection Mode", "", "selection.column"),
            ],
        ),
        Menu::new(
            "~V~iew",
            vec![
                it("~C~ommand Palette...", "Ctrl+Shift+P", "view.palette"),
                sep(),
                it("~E~xplorer", "Ctrl+Shift+E", "view.files"),
                it("O~u~tline", "", "view.outline"),
                it("Searc~h~", "Ctrl+Shift+F", "view.search"),
                it("~S~ource Control", "Ctrl+Shift+G", "view.scm"),
                it("~R~un and Debug", "Ctrl+Shift+D", "view.debug"),
                it("E~x~tensions", "Ctrl+Shift+X", "view.extensions"),
                sep(),
                it("~P~roblems", "Ctrl+Shift+M", "view.problems"),
                it("~O~utput", "Ctrl+Shift+U", "view.output"),
                it("~T~erminal", "Ctrl+`", "view.terminal"),
                it("AI Ch~a~t", "Ctrl+L", "ai.chat"),
                sep(),
                sub(
                    "Appeara~n~ce",
                    vec![
                        it("Primary Side ~B~ar", "Ctrl+B", "view.sidebar"),
                        it("~P~anel", "Ctrl+J", "view.panel"),
                        it("Secondar~y~ Side Bar", "Ctrl+Alt+B", "view.secondary"),
                        sep(),
                        it("Editor ~T~abs", "", "view.editor_tabs"),
                        it("Brea~d~crumbs", "", "view.breadcrumbs"),
                        it("~M~inimap", "", "view.minimap"),
                        it("~L~ine Numbers", "", "view.line_numbers"),
                        it("Render ~W~hitespace", "", "view.whitespace"),
                        it("~R~ight to Left", "", "view.rtl"),
                        sep(),
                        it("Reset La~y~out", "", "view.reset_layout"),
                    ],
                ),
                sub(
                    "Editor ~L~ayout",
                    vec![
                        it("Split ~R~ight", "Ctrl+\\", "view.split_right"),
                        it("Split ~D~own", "", "view.split_down"),
                        it("Split ~L~eft", "", "view.split_left"),
                        it("Split ~U~p", "", "view.split_up"),
                        sep(),
                        it("~S~ingle", "", "view.single_group"),
                        it("Two C~o~lumns", "", "view.two_columns"),
                        it("T~h~ree Columns", "", "view.three_columns"),
                        it("Two Ro~w~s", "", "view.two_rows"),
                        it("~G~rid (2×2)", "", "view.grid"),
                        sep(),
                        it("Move to Ne~x~t Group", "Ctrl+Alt+Right", "view.next_group_editor"),
                        it("Move to ~P~revious Group", "Ctrl+Alt+Left", "view.prev_group_editor"),
                        it("~C~lose Group", "", "view.close_group"),
                        sep(),
                        it("~T~ile", "", "window.tile"),
                        it("C~a~scade", "", "window.cascade"),
                        it("~M~aximize All", "", "window.maximize_all"),
                    ],
                ),
                sub(
                    "~W~indows",
                    vec![
                        it("~Z~oom", "F5", "window.zoom"),
                        it("~N~ext", "F6", "window.next"),
                        it("~P~revious", "Shift+F6", "window.prev"),
                        it("~C~lose", "Alt+F3", "window.close"),
                        it("C~l~ose All", "", "window.close_all"),
                        sep(),
                        it("L~i~st...", "Alt+0", "window.list"),
                    ],
                ),
                sep(),
                it("Word Wr~a~p", "Alt+Z", "view.wrap"),
                it("Markdown Pre~v~iew", "Ctrl+Shift+V", "view.preview"),
                sep(),
                it("~F~old", "Ctrl+Shift+[", "view.fold"),
                it("Unfol~d~", "Ctrl+Shift+]", "view.unfold"),
                it("Fold A~l~l", "", "view.fold_all"),
                it("Unfold All", "", "view.unfold_all"),
            ],
        ),
        Menu::new(
            "~G~o",
            vec![
                it("~B~ack", "Alt+Left", "go.back"),
                it("~F~orward", "Alt+Right", "go.forward"),
                sep(),
                it("Go to F~i~le...", "Ctrl+P", "go.file"),
                it("Go to ~S~ymbol...", "Ctrl+Shift+O", "go.symbol"),
                it("Go to Symbol in ~W~orkspace...", "Ctrl+T", "go.workspace_symbol"),
                it("Go to ~D~efinition", "F12", "go.definition"),
                it("Go to ~R~eferences", "Shift+F12", "go.references"),
                it("Go to ~L~ine...", "Ctrl+G", "search.goto"),
                it("Go to Brac~k~et", "Ctrl+Shift+\\", "go.bracket"),
                it("~N~ext Definition", "Alt+PgDn", "go.next_def"),
                it("~P~revious Definition", "Alt+PgUp", "go.prev_def"),
                sep(),
                it("Ne~x~t Problem", "Alt+F8", "go.next_problem"),
                it("Previous Pr~o~blem", "Alt+F7", "go.prev_problem"),
                sep(),
                sub(
                    "Switch ~G~roup",
                    vec![
                        it("Group ~1~", "Ctrl+1", "view.focus_group_1"),
                        it("Group ~2~", "Ctrl+2", "view.focus_group_2"),
                        it("Group ~3~", "Ctrl+3", "view.focus_group_3"),
                        it("Group ~4~", "Ctrl+4", "view.focus_group_4"),
                    ],
                ),
            ],
        ),
        Menu::new(
            "~R~un",
            vec![
                it("~R~un", "Ctrl+F9", "run.run"),
                it("Start ~D~ebugging", "", "run.debug"),
                it("Run ~W~ithout Debugging", "Ctrl+F5", "run.without_debug"),
                it("Program R~e~set", "Ctrl+F2", "run.reset"),
                it("~G~o to Cursor", "F4", "run.to_cursor"),
                it("~T~race Into", "F7", "run.step_into"),
                it("~S~tep Over", "F8", "run.step_over"),
                it("Step O~u~t", "", "run.step_out"),
                it("~P~ause", "", "run.pause"),
                it("~A~rguments...", "", "run.arguments"),
                sep(),
                it("Toggle ~B~reakpoint", "Ctrl+F8", "run.breakpoint"),
                it("Breakpoint Con~d~ition...", "Ctrl+Shift+F8", "run.bp_condition"),
                it("Add ~W~atch...", "Ctrl+F7", "run.watch"),
                it("E~v~aluate/Modify...", "Ctrl+F4", "run.evaluate"),
                it("~I~mmediate Window", "Ctrl+Shift+Y", "run.immediate"),
                it("Debug Co~n~figurations...", "", "run.configurations"),
                it("~C~all Stack", "Ctrl+F3", "run.stack"),
                sep(),
                it("C~o~mpile", "Alt+F9", "build.compile"),
                it("~M~ake", "F9", "build.make"),
                it("Bui~l~d All", "", "build.all"),
                it("Stop B~u~ild", "", "build.stop"),
            ],
        ),
        Menu::new(
            "Ter~m~inal",
            vec![
                it("~N~ew Terminal", "Ctrl+Shift+`", "terminal.new"),
                it("~S~plit Terminal", "", "terminal.split"),
                sep(),
                it("~R~un Task...", "", "terminal.task"),
                it("Run ~B~uild Task...", "Ctrl+Shift+B", "terminal.build"),
            ],
        ),
        Menu::new(
            "~A~I",
            vec![
                it("~C~hat...", "Ctrl+L", "ai.chat"),
                it("~I~nline Edit", "Ctrl+K", "ai.inline"),
                it("~A~gent...", "Ctrl+I", "ai.agent"),
                it("~T~ab Completion", "", "ai.tab"),
                it("~M~odels...", "", "ai.models"),
                sep(),
                it("Agent ~D~emo", "", "agent.demo"),
                it("~S~top Agent", "Esc", "agent.stop"),
            ],
        ),
        // Turbo Pascal 7's Tools menu: the programs that come with Turbine,
        // Git (VS Code keeps it in Source Control's view), and extensions'
        // items at the end.
        Menu::new(
            "~T~ools",
            vec![
                it("~F~ile Manager", "Ctrl+Alt+F", "tools.file_manager"),
                it("~C~alculator", "", "tools.calculator"),
                it("Show C~l~ipboard", "", "edit.show_clipboard"),
                sep(),
                sub(
                    "G~i~t",
                    vec![
                        it("Open ~C~hanges", "", "git.diff_file"),
                        it("~A~ll Changes", "", "git.diff_all"),
                        it("~S~taged Changes", "", "git.diff_staged"),
                        sep(),
                        it("~H~istory", "", "git.log"),
                        it("~F~ile History", "", "git.log_file"),
                        it("~W~ho Changed This Line", "", "git.blame"),
                        sep(),
                        it("Switch ~B~ranch...", "", "git.checkout"),
                        it("~P~ull", "", "git.pull"),
                        it("P~u~sh", "", "git.push"),
                        it("F~e~tch", "", "git.fetch"),
                        it("S~t~ash", "", "git.stash"),
                        it("P~o~p Stash", "", "git.stash_pop"),
                    ],
                ),
                sep(),
                it("~G~rep (Find in Files)...", "Ctrl+Shift+F", "search.files"),
                it("New ~T~erminal", "Ctrl+Shift+`", "terminal.new"),
                it("Command ~P~alette...", "Ctrl+Shift+P", "view.palette"),
            ],
        ),
        Menu::new(
            "~H~elp",
            vec![
                it("~C~ontents", "F1", "help.contents"),
                it("~I~ndex", "Shift+F1", "help.index"),
                it("~T~opic Search", "Ctrl+F1", "help.topic"),
                sep(),
                it("~L~anguage Guide", "", "help.language"),
                it("Language ~G~uides...", "", "help.guides"),
                it("Download ~A~ll Guides", "", "help.download_all"),
                sep(),
                it("~K~eyboard...", "", "help.keys"),
                it("~V~im Keys...", "", "help.vim"),
                sep(),
                it("A~b~out...", "", "help.about"),
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
            scroll_drag: None,
            selecting: None,
            last_click: None,
            keymap: Keymap::Cua,
            from_agent: false,
            clipboard_out: None,
            highlighter: turbine_syntax::Highlighter::new(),
            hl_pending: 0,
            lsp: HashMap::new(),
            lsp_missing: HashSet::new(),
            lspx: Default::default(),
            dbx: Default::default(),
            build: Default::default(),
            preview: Default::default(),
            git: Default::default(),
            extras: Default::default(),
            xtrees: HashMap::new(),
            viewers: HashMap::new(),
            image_places: Vec::new(),
            images_inline: false,
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
            groups: groups::Groups::default(),
            chat_thinking: None,
            chat_wait: None,
            chat_thought: (String::new(), None),
            pending_question: None,
            agent: None,
            pending_task: None,
            socket_path: None,
            look_path: None,
            shell: Shell::new(shell::project_of(files), !files.is_empty()),
            find: FindBar::default(),
            ex: explorer_ops::ExOps::default(),
            inline: None,
            exts: extensions::ExtState::default(),
            session: session::SessionState::default(),
            tip: tooltip::TipState::default(),
            docviews: HashMap::new(),
            ed_menu: None,
            window_op: 0,
            gitlines: gitlines::GitLines::default(),
            env_note: HashSet::new(),
            status_menu: None,
            outer: (80, 25),
            last_input: None,
        };
        for f in files {
            // (A folder is the project: the Explorer shows it.)
            if !f.is_dir() {
                wb.open_path(f);
            }
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

    /// Run a scroll bar action on window `i`'s view: its text, or the
    /// rendered page when it shows its Preview.
    fn bar_at<T>(&mut self, i: usize, f: impl FnOnce(&mut EdWin) -> T) -> T {
        let id = self.windows[i].win.id;
        let page = self.doc_mode(id) == docview::Mode::Preview && self.docviews.contains_key(&id);
        if page {
            std::mem::swap(&mut self.windows[i].ed, &mut self.docviews.get_mut(&id).unwrap().alt);
        }
        let out = f(&mut self.windows[i]);
        if page {
            std::mem::swap(&mut self.windows[i].ed, &mut self.docviews.get_mut(&id).unwrap().alt);
        }
        out
    }

    /// A right-click in the front editor's text: the cursor there (unless
    /// it's in the selection), and the editor's menu, as VS Code's.
    fn ed_menu_open(&mut self, p: Point) -> bool {
        let Some(w) = self.windows.last_mut().filter(|w| w.term.is_none() && w.dock.is_none() && w.special.is_none() && !self.xtrees.contains_key(&w.win.id)) else { return false };
        if self.docviews.get(&w.win.id).is_some_and(|d| d.mode != docview::Mode::Source) {
            return false;
        }
        let Some(ch) = w.ed.char_at(p) else { return false };
        if !w.ed.selection().is_some_and(|(a, b)| ch >= a && ch < b) {
            w.ed.cursor = ch;
            w.ed.anchor = None;
        }
        let has_sel = w.ed.selection().is_some();
        let lsp = w.lsp_key.is_some();
        let mut items: Vec<(&str, &str, &'static str)> = Vec::new();
        if lsp {
            items.push(("Go to Definition", "F12", "go.definition"));
            items.push(("Go to References", "Shift+F12", "go.references"));
            items.push(("Rename Symbol...", "Shift+F2", "edit.rename"));
            items.push(("Quick Fix...", "Ctrl+.", "edit.quick_fix"));
        }
        if has_sel {
            items.push(("Cut", "Ctrl+X", "edit.cut"));
            items.push(("Copy", "Ctrl+C", "edit.copy"));
        }
        items.push(("Paste", "Ctrl+V", "edit.paste"));
        items.push(("Select All", "Ctrl+A", "edit.select_all"));
        items.push(("Toggle Line Comment", "Ctrl+/", "edit.comment"));
        if lsp {
            items.push(("Format Document", "Ctrl+Shift+I", "edit.format"));
        }
        items.push(("Command Palette...", "Ctrl+Shift+P", "view.palette"));
        let mut popup = ListPopup::new(items.iter().map(|(l, k, _)| (l.to_string(), k.to_string())).collect());
        popup.max_rows = 14;
        popup.place(p, self.screen());
        self.ed_menu = Some((popup, p, items.iter().map(|i| i.2).collect()));
        true
    }

    /// A right-click in the front terminal, when its program isn't using
    /// the mouse itself: copy, paste, and the like.
    fn term_menu_open(&mut self, p: Point) -> bool {
        let Some(w) = self.windows.last_mut().filter(|w| w.win.inner().contains(p)) else { return false };
        let Some(t) = w.term.as_mut() else { return false };
        if t.terminal().is_some_and(|t| t.wants_mouse()) {
            return false;
        }
        let has_sel = t.selected_text().is_some();
        let mut items: Vec<(&str, &str, &'static str)> = Vec::new();
        if has_sel {
            items.push(("Copy", "Ctrl+Shift+C", "terminal.copy"));
        }
        items.push(("Paste", "Ctrl+Shift+V", "terminal.paste"));
        items.push(("Select All", "", "terminal.select_all"));
        items.push(("Clear", "Ctrl+L", "terminal.clear"));
        items.push(("New Terminal", "Ctrl+Shift+`", "terminal.new"));
        items.push(("Kill Terminal", "", "terminal.kill"));
        let mut popup = ListPopup::new(items.iter().map(|(l, k, _)| (l.to_string(), k.to_string())).collect());
        popup.place(p, self.screen());
        self.ed_menu = Some((popup, p, items.iter().map(|i| i.2).collect()));
        true
    }

    /// A status item's drop-up (file type, encoding, line ends): true when
    /// the command is one of those.
    fn status_menu_open(&mut self, cmd: &str, at: Point) -> bool {
        let Some(w) = self.windows.last().filter(|w| w.term.is_none()) else { return false };
        let (items, acts, sel): (Vec<(String, String)>, Vec<String>, usize) = match cmd {
            "edit.language" => {
                let mut names: Vec<String> = turbine_syntax::BUILTIN.iter().map(|b| b.0.to_string()).collect();
                for n in turbine_syntax::grammars::known() {
                    if !names.contains(&n) {
                        names.push(n);
                    }
                }
                names.sort_by_key(|n| turbine_syntax::label(n).to_lowercase());
                let cur = w.lang.clone().unwrap_or_default();
                let mut items = vec![("Auto Detect".to_string(), String::new())];
                let mut acts = vec!["lang:".to_string()];
                for n in &names {
                    items.push((turbine_syntax::label(n), n.clone()));
                    acts.push(format!("lang:{n}"));
                }
                let sel = names.iter().position(|n| *n == cur).map_or(0, |i| i + 1);
                (items, acts, sel)
            }
            "file.reopen_encoding" => {
                let list = crate::file_dialog::encodings();
                let cur = w.ed.doc.encoding.name();
                let mut items = vec![("Detect it".to_string(), "reopen".to_string())];
                let mut acts = vec!["enc:detect".to_string()];
                let mut sel = 0;
                for (i, (name, _, _)) in list.iter().enumerate() {
                    if name.starts_with(cur) && sel == 0 {
                        sel = i + 1;
                    }
                    items.push((name.clone(), "reopen".into()));
                    acts.push(format!("enc:{i}"));
                }
                (items, acts, sel)
            }
            "file.line_ending" => {
                let crlf = w.ed.doc.line_ending == turbine_text::encoding::LineEnding::CrLf;
                (vec![("LF".into(), "Unix, macOS".into()), ("CRLF".into(), "Windows, DOS".into())], vec!["eol:LF".into(), "eol:CRLF".into()], usize::from(crlf))
            }
            _ => return false,
        };
        let mut popup = ListPopup::new(items);
        popup.max_rows = 16;
        popup.sel = sel;
        popup.place(at, self.screen());
        self.status_menu = Some((popup, at, acts));
        true
    }

    fn status_choice(&mut self, a: &str) {
        if let Some(x) = a.strip_prefix("scm:") {
            self.scm_action(x);
            return;
        }
        if self.chat_picker_choice(a) {
            return;
        }
        let Some(w) = self.windows.iter_mut().rev().find(|w| w.term.is_none() && w.special.is_none()) else { return };
        if let Some(lang) = a.strip_prefix("lang:") {
            w.lang_fixed = (!lang.is_empty()).then(|| lang.to_string());
            w.lang_for = None;
        } else if let Some(e) = a.strip_prefix("enc:") {
            let list = crate::file_dialog::encodings();
            let enc = e.parse::<usize>().ok().and_then(|i| list.get(i)).and_then(|(_, l, _)| turbine_text::encoding::TextEncoding::for_label(l));
            let r = w.ed.doc.reopen_as(enc);
            w.ed.cursor = w.ed.cursor.min(w.ed.doc.rope.len_chars());
            w.hl_sent = None;
            let name = w.ed.doc.encoding.name();
            self.say(match r {
                Ok(()) => format!("Reopened as {name}"),
                Err(e) => format!("Can't reopen: {e}"),
            });
        } else if let Some(e) = a.strip_prefix("eol:") {
            use turbine_text::encoding::LineEnding;
            let want = if e == "CRLF" { LineEnding::CrLf } else { LineEnding::Lf };
            if w.ed.doc.line_ending != want {
                w.ed.doc.line_ending = want;
                w.ed.doc.dirty = true;
                self.say(format!("Line ends: {e} (saved as such)"));
            }
        }
    }

    /// The Borland line:column cells before the sideways bar in a window's bottom edge.
    fn hbar_skip(&self) -> i32 {
        if self.theme.style.window_numbers { 13 } else { 2 }
    }

    /// Whether window `i`'s sideways bar is over its last row (its bottom
    /// edge being the panel's top).
    fn hbar_inside(&self, i: usize) -> bool {
        let w = &self.windows[i];
        let l = self.layout();
        let shared = l.bottom.is_some_and(|b| b.y == w.win.rect.bottom() - 1) || (self.group_count() > 1 && w.dock.is_none() && w.win.rect.bottom() != l.editors.bottom());
        shared && w.ed.hbar_inside_shown()
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

    /// Lay out open dialogs for the screen and control style (their
    /// controls' places come from this).
    fn layout_dialogs(&mut self) {
        let screen = self.screen();
        let theme = self.theme.clone();
        for d in [&mut self.dialog, &mut self.overwrite_q].into_iter().flatten() {
            d.layout(screen, &theme);
        }
        if let (Some(f), Some(d)) = (&mut self.file_dlg, &mut self.dialog) {
            f.fit(d);
        }
    }

    /// The colours and control style in use.
    pub fn theme(&self) -> &Theme {
        &self.theme
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
                self.apply_layout_json(&v["layout"]);
                self.apply_settings_json(&v["settings"]);
                // Started without a file or folder: the last project again.
                if !self.shell.explicit_project {
                    if let Some(p) = v["project"].as_str().map(PathBuf::from).filter(|p| p.is_dir()) {
                        self.shell.explorer.set_root(p);
                    }
                }
            }
        }
        self.look_path = Some(p);
        self.restore_session();
    }

    /// Keep the look and layout for next time (written off the UI thread).
    fn save_look(&self) {
        if let Some(p) = self.look_path.clone() {
            let v = json!({
                "style": self.theme.style.name,
                "scheme": self.theme.scheme.name,
                "layout": self.layout_json(),
                "settings": self.settings_json(),
                "project": self.shell.explorer.root.display().to_string(),
            });
            std::thread::spawn(move || {
                if let Some(d) = p.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap_or_default());
            });
        }
    }

    /// An agent (or a question for it) is still at work.
    pub fn agent_running(&self) -> bool {
        self.agent.is_some() || self.pending_task.is_some() || self.chat_streaming.is_some() || self.pending_question.is_some()
    }

    /// Its own title bar unless the settings say otherwise (the app's default).
    pub fn default_own_title_bar(&mut self, on: bool) {
        self.shell.own_title_bar = on;
    }

    /// The app's window drawn with Turbine's own title bar (no desktop frame).
    pub fn own_title_bar(&self) -> bool {
        self.shell.own_title_bar
    }

    /// The window action asked for by the title bar's buttons (0: none).
    pub fn take_window_op(&mut self) -> i32 {
        std::mem::take(&mut self.window_op)
    }

    /// The front end can draw pictures (the app; a kitty-graphics terminal).
    pub fn set_images_inline(&mut self, on: bool) {
        self.images_inline = on;
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

    /// Where the editor windows go (between the docked parts).
    fn desktop(&self) -> Rect {
        self.layout().editors
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
        let mut n = 0;
        for w in self.windows.iter_mut() {
            if w.dock.is_some() {
                w.win.number = None;
            } else {
                n += 1;
                w.win.number = Some(n);
            }
        }
    }

    fn new_window(&mut self, doc: Document) {
        let d = self.desktop();
        let n = self.windows.len() as i32;
        let off = (n * 2) % (d.h / 2).max(1);
        let rect = Rect::new(d.x + off, d.y + off / 2, (d.w - off).max(20), (d.h - off / 2).max(6));
        let title = doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| doc.title());
        let mut win = Window::new(self.next_id, &title, rect);
        // (Editors fill their area, one per tab, as in Cursor; Window >
        // Tile and Cascade still lay them out DOS style.)
        win.toggle_zoom(d);
        self.next_id += 1;
        let mut ed = EditorView::new(doc);
        ed.line_numbers = self.shell.line_numbers.unwrap_or(self.theme.style.line_numbers);
        ed.wrap = self.shell.word_wrap;
        ed.show_whitespace = self.shell.show_whitespace;
        ed.rainbow_brackets = self.shell.rainbow_brackets;
        ed.indent_guides = self.shell.indent_guides;
        ed.match_brackets = self.shell.match_brackets;
        let mut ew = EdWin { win, ed, vim: None, term: None, term_text: String::new(), lang: None, lang_for: None, lang_fixed: None, hl_sent: None, lsp_key: None, lsp_sent: None, was_dirty: false, special: None, dock: None, mirror: None };
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
            "immediate" => ("Immediate", Rect::new(d.x, d.bottom() - (d.h / 4).max(5), d.w, (d.h / 4).max(5))),
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
            lang_fixed: None,
            hl_sent: None,
            lsp_key: None,
            lsp_sent: None,
            was_dirty: false,
            special: Some(kind),
            dock: Some(Side::Bottom),
            mirror: None,
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
        let mut t = String::new();
        if !self.dbx.watches.is_empty() {
            t.push_str("Watch\n");
            for l in self.watch_lines() {
                t.push_str(&l);
                t.push('\n');
            }
            t.push('\n');
        }
        t.push_str("Call stack\n");
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
        let d = Dialog::new(&format!("arguments:{}", root.display()), "Program and Arguments")
            .body(ui::col(vec![
                ui::input("program", "~P~rogram to run:", &program.display().to_string()).chars(50),
                ui::input("args", "~A~rguments:", &args).chars(50),
            ]))
            .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
        self.dialog = Some(d);
    }

    fn start_debugging(&mut self, stop_on_entry: bool) {
        if self.start_with_config(stop_on_entry) {
            return;
        }
        let Some((root, program, lang)) = self.guess_program() else {
            self.say("Open a file of the program to debug first");
            return;
        };
        let args = self.run_config.get(&root).map(|(_, a)| a.clone()).unwrap_or_default();
        let launch = Launch { program: program.clone(), args, cwd: root.clone(), stop_on_entry, ..Default::default() };
        match (self.dap_spawn)(&lang, launch, self.breakpoints.clone()) {
            None => self.say(format!("No debugger for {lang}: install lldb-dap, gdb, debugpy or dlv")),
            Some(Err(e)) => self.say(format!("Debugger didn't start: {e}")),
            Some(Ok(mut c)) => {
                c.options = self.dbx.bp_opts.clone();
                self.say(format!("Debugging {} with {}", program.display(), c.name()));
                self.debug_started(c);
            }
        }
    }

    fn debug_started(&mut self, c: DapClient) {
        self.debug = Some(Debug { client: c, frames: Vec::new(), scopes: Vec::new(), temp: None });
        for v in &mut self.dbx.watch_vals {
            *v = None;
        }
        self.debug_layout();
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
        if let Some(i) = self.windows.iter().position(|w| w.special == Some("help") && w.win.title == format!("HELP: {title}")) {
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
    /// A read-only window of Markdown, rendered.
    fn show_help(&mut self, title: &str, md: &str) {
        self.show_rendered(title, md, None);
    }

    /// A read-only window of plain text.
    fn show_plain(&mut self, title: &str, md: &str) {
        let name = format!("HELP: {title}");
        let i = match self.windows.iter().position(|w| w.special == Some("help") && w.win.title == name) {
            Some(i) => i,
            None => {
                let d = self.desktop();
                let mut win = Window::new(self.next_id, &name, Rect::new(d.x + 2, d.y + 1, (d.w - 4).max(20), (d.h - 2).max(6)));
                win.min_h = 4;
                // (Filling the editors' area, as every editor window does.)
                win.toggle_zoom(d);
                self.next_id += 1;
                let mut ed = EditorView::new(Document::new());
                ed.line_numbers = false;
                self.windows.push(EdWin {
                    win,
                    ed,
                    vim: None,
                    term: None,
                    term_text: String::new(),
                    lang: None,
                    lang_for: Some(None),
                    lang_fixed: None,
                    hl_sent: None,
                    lsp_key: None,
                    lsp_sent: None,
                    was_dirty: false,
                    special: Some("help"),
                    dock: None,
                    mirror: None,
                });
                self.renumber();
                self.windows.len() - 1
            }
        };
        // (In front: the window asked for.)
        let w = self.windows.remove(i);
        self.windows.push(w);
        self.renumber();
        let w = self.windows.last_mut().unwrap();
        w.ed.doc = Document::from_str(md);
        w.ed.help_look = true;
        w.ed.highlights.clear();
        w.ed.line_bg.clear();
        w.ed.words.clear();
        w.ed.cursor = 0;
        w.ed.scroll_y = 0;
        w.hl_sent = None;
        let id = w.win.id;
        self.preview.previews.remove(&id);
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
        let d = Dialog::new(&format!("guides:{}", keys.join("\u{1f}")), "Language Guides (√ = kept offline)")
            .body(ui::list("guides", items, 0).activates().lines(18))
            .buttons(vec![ui::button("ok", "~O~pen", true), ui::button("cancel", "Cancel", false)]);
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
            let mut ai = Ai::start();
            ai.thinking = self.chat_thinking.clone();
            self.ai = Some(ai);
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
        let d = Dialog::new("ask", title)
            .body(ui::input("q", "~Q~uestion:", "").chars(60))
            .buttons(vec![ui::button("ok", "~A~sk", true), ui::button("cancel", "Cancel", false)]);
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
        let shown = if model.label.is_empty() { model.name.clone() } else { model.label.clone() };
        self.append_chat(&format!("\n## You\n\n{q}\n\n## {shown}\n\n"));
        // (A model starting, or thinking first, takes a while: say so.)
        self.chat_wait = self.windows.iter().find(|w| w.special == Some("chat")).map(|w| w.ed.doc.rope.len_chars());
        self.append_chat(CHAT_WAIT);
        let msgs = self.chat_history.clone();
        let id = self.ai.as_mut().unwrap().chat(&model, msgs);
        self.chat_streaming = Some((id, String::new()));
    }

    /// Take the chat's "thinking…" away (its answer is coming, or failed).
    fn clear_chat_wait(&mut self) {
        let Some(at) = self.chat_wait.take() else { return };
        let (thought, since) = std::mem::take(&mut self.chat_thought);
        let Some(w) = self.windows.iter_mut().find(|w| w.special == Some("chat")) else { return };
        let end = w.ed.doc.rope.len_chars();
        if at <= end {
            w.ed.doc.remove(at, end, end);
            w.ed.doc.dirty = false;
            w.ed.cursor = w.ed.doc.rope.len_chars();
        }
        // (It thought: for how long stays, as Cursor's "Thought for 4s".)
        if let (false, Some(t)) = (thought.trim().is_empty(), since) {
            self.append_chat(&format!("*Thought for {} s*\n\n", t.elapsed().as_secs().max(1)));
        }
    }

    /// The model's thinking so far, live where the answer will go: its last
    /// few lines in a quote (a stream to watch, not to read back).
    fn show_chat_thinking(&mut self, text: &str) {
        let Some(at) = self.chat_wait else { return };
        let (thought, since) = &mut self.chat_thought;
        since.get_or_insert_with(Instant::now);
        thought.push_str(text);
        let lines: Vec<&str> = thought.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = lines[lines.len().saturating_sub(4)..].iter().map(|l| format!("> {}", l.trim())).collect::<Vec<_>>().join("\n");
        let shown = format!("*thinking…*\n\n{tail}\n");
        let Some(w) = self.windows.iter_mut().find(|w| w.special == Some("chat")) else { return };
        let end = w.ed.doc.rope.len_chars();
        if at <= end {
            w.ed.doc.replace(at, end, &shown, end);
            w.ed.doc.dirty = false;
            w.ed.cursor = w.ed.doc.rope.len_chars();
            w.ed.line_bg = chat_line_colours(&w.ed.doc);
        }
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
                // (Answers are prose: wrapped to the side bar.)
                ed.wrap = true;
                self.windows.push(EdWin {
                    win,
                    ed,
                    vim: None,
                    term: None,
                    term_text: String::new(),
                    lang: Some("markdown".into()),
                    lang_for: Some(None),
                    lang_fixed: None,
                    hl_sent: None,
                    lsp_key: None,
                    lsp_sent: None,
                    was_dirty: false,
                    special: Some("chat"),
                    dock: Some(Side::Right),
                    mirror: None,
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
        ed.line_bg = chat_line_colours(&ed.doc);
    }

    fn agent_dialog(&mut self) {
        self.start_ai();
        let d = Dialog::new("agent", "Agent: what should I do?")
            .body(ui::input("task", "~T~ask:", "").chars(60))
            .buttons(vec![ui::button("ok", "~S~tart", true), ui::button("cancel", "Cancel", false)]);
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
                let d = Dialog::new("agent_approve", "Agent wants to run a command")
                    .body(ui::col(vec![ui::label("In the terminal:"), ui::label(&format!("  {what}"))]))
                    .buttons(vec![
                        ui::button("allow", "~A~llow", true),
                        ui::button("always", "Allow A~l~ways", false),
                        ui::button("deny", "~D~eny", false),
                    ]);
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
        let d = Dialog::new("models", &format!("AI Models ({})", a.source))
            .body(ui::col(vec![ui::label("Chat model:"), ui::list("models", items, sel).activates()]))
            .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
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
                AiEvent::Thinking { id, text } => {
                    if self.chat_streaming.as_ref().is_some_and(|(i, _)| *i == id) {
                        self.show_chat_thinking(&text);
                    }
                }
                AiEvent::Chunk { id, text } if self.inline_chunk(id, &text) => {}
                AiEvent::Done { id } if self.inline_done(id) => {}
                AiEvent::Error { id, ref message } if self.inline_error(id, message) => {}
                AiEvent::Chunk { id, text } => {
                    if self.chat_streaming.as_ref().is_some_and(|(i, _)| *i == id) {
                        self.clear_chat_wait();
                        self.chat_streaming.as_mut().unwrap().1.push_str(&text);
                        self.append_chat(&text);
                    }
                }
                AiEvent::Done { id } => {
                    if let Some((i, answer)) = self.chat_streaming.take() {
                        if i == id {
                            self.clear_chat_wait();
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
                        self.clear_chat_wait();
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
                            // (Up to a blank line, three lines at most.)
                            let ghost: String = text.trim_end().split("\n\n").next().unwrap_or("").lines().take(3).collect::<Vec<_>>().join("\n");
                            let doc = w.ed.doc.rope.slice(..w.ed.doc.rope.len_chars().min(20_000)).to_string();
                            if !ghost.trim().is_empty() && ai::fits_text(&ghost, &doc) {
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
        // (With little to go on a base model rambles, often in Chinese: ask
        // only once there's some code before the cursor.)
        if before.split_whitespace().map(str::len).sum::<usize>() < 24 {
            return;
        }
        let id = a.complete(&model, before, after);
        self.fim_pending = Some((id, win, version, cur));
    }

    /// Output and Debug along the bottom, code windows above them.
    fn debug_layout(&mut self) {
        // The debugger's output and state in the panel, the Run and Debug
        // view in the side bar.
        let o = self.special_window("output");
        let id = self.windows[o].win.id;
        self.special_window("debug");
        self.layout_dock();
        self.shell.dock.show(crate::dock::Tab::Pane(id));
        self.shell.dock.show(crate::dock::Tab::Debug);
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
                    self.eval_watches();
                    self.show_debug_info();
                }
                DapEvent::Evaluated { id, result } => self.debug_evaluated(id, result),
                DapEvent::Children { .. } => {}
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
                // (The project's direnv environment first: still loading,
                // the server waits for it.)
                if matches!(turbine_services::env::project_env(&key.1), turbine_services::env::Env::Loading) {
                    if self.env_note.insert(key.1.clone()) {
                        started.push(format!("Loading {}'s environment (direnv)…", key.1.display()));
                    }
                    continue;
                }
                if let Some(e) = turbine_services::env::project_env_error(&key.1) {
                    if self.env_note.insert(key.1.join(".blocked")) {
                        started.push(e);
                    }
                }
                match (self.lsp_spawn)(&lang, &key.1) {
                    None => {
                        self.lsp_missing.insert(lang.clone());
                    }
                    Some(Err(e)) => {
                        self.lsp_missing.insert(lang.clone());
                        started.push(format!("Language server for {lang} didn't start: {e}"));
                    }
                    Some(Ok(c)) => {
                        let from_nix = if c.name() == "nix" { " from nixpkgs (the first time downloads it)" } else { "" };
                        started.push(format!("Started {}{from_nix} for {}", c.name(), key.1.display()));
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
        if self.lsp_more_event(&key, &e) {
            return;
        }
        match e {
            LspEvent::Ready | LspEvent::Log(_) => {}
            LspEvent::Diagnostics { path, items } => {
                let want = std::fs::canonicalize(&path).unwrap_or(path);
                self.lspx.diags.insert(want.clone(), items.iter().map(|d| d.raw.clone()).collect());
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
                if self.hover_answer(&key, id, &text) {
                    return;
                }
                if self.lsp_requests.remove(&(key, id)).is_none() {
                    return;
                }
                if text.trim().is_empty() {
                    self.say("Nothing to show here");
                    return;
                }
                let lines: Vec<String> = text.lines().filter(|l| !l.starts_with("```")).map(str::to_string).collect();
                let d = Dialog::new("hover", "Topic")
                    .body(ui::list("text", lines, 0).lines(20))
                    .buttons(vec![ui::button("ok", "O~K~", true)]);
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
                self.nav_push();
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
            LspEvent::References { id, locations } => {
                if self.lsp_requests.remove(&(key, id)).is_none() {
                    return;
                }
                if locations.is_empty() {
                    self.say("No references found");
                    return;
                }
                let word = self.word_at_cursor();
                let places = locations.into_iter().map(|l| (l.path, l.start.line as usize)).collect();
                self.shell.search.show_places(&format!("References: {word}"), places);
                self.run_command("view.search");
                self.shell.search.in_field = false;
            }
            LspEvent::Rename { id, edits } => {
                if self.lsp_requests.remove(&(key, id)).is_none() {
                    return;
                }
                self.apply_rename(edits);
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
            _ => {}
        }
    }

    /// The active editor's server and the cursor's position for it.
    fn lsp_here(&mut self) -> Option<(LspKey, PathBuf, Pos)> {
        let w = self.windows.last_mut()?;
        let key = w.lsp_key.clone()?;
        let path = w.ed.doc.path.clone()?;
        // (The server has to have what was just typed before it's asked.)
        let v = w.ed.doc.version;
        if w.lsp_sent != Some(v) {
            if let Some(c) = self.lsp.get_mut(&key).filter(|c| c.is_open(&path)) {
                c.did_change(&path, v as i64, &w.ed.doc.rope.to_string());
                w.lsp_sent = Some(v);
            }
        }
        let w = self.windows.last()?;
        let (line, byte) = w.ed.doc.char_to_line_byte(w.ed.cursor);
        let text = w.ed.doc.line(line);
        Some((key, path, Pos { line: line as u32, character: lsp::utf16_col(&text, byte) }))
    }

    fn lsp_request(&mut self, what: LspReq) {
        let Some((key, path, pos)) = self.lsp_here() else {
            self.no_server();
            return;
        };
        let Some(c) = self.lsp.get_mut(&key) else { return };
        let id = match what {
            LspReq::Hover => c.hover(&path, pos),
            LspReq::Definition => c.definition(&path, pos),
            LspReq::References => c.references(&path, pos),
            // (Rename goes through lsp_rename, with the new name.)
            LspReq::Rename => return,
        };
        let win = self.windows.last().map(|w| w.win.id).unwrap_or(0);
        self.lsp_requests.insert((key, id), (what, win));
    }

    /// Rename the symbol at the cursor everywhere (the language server's edits).
    fn lsp_rename(&mut self, new_name: &str) {
        let Some((key, path, pos)) = self.lsp_here() else {
            self.no_server();
            return;
        };
        let Some(c) = self.lsp.get_mut(&key) else { return };
        let id = c.rename(&path, pos, new_name);
        let win = self.windows.last().map(|w| w.win.id).unwrap_or(0);
        self.lsp_requests.insert((key, id), (LspReq::Rename, win));
    }

    /// Help > Topic Search (Ctrl+F1, Borland's): the word at the cursor (or
    /// the selection; else it asks) looked up in the language's guide and
    /// Turbine's manual, every section it's in on one page; the language
    /// server's hover too, when there's one.
    pub(super) fn topic_search(&mut self, topic: Option<String>) {
        let sel = self.windows.last().and_then(|w| w.ed.selected_text()).filter(|t| !t.contains('\n') && t.len() < 80);
        let topic = topic.or(sel).unwrap_or_else(|| self.word_at_cursor()).trim().to_string();
        if topic.is_empty() {
            let d = Dialog::new("topic", "Topic Search")
                .body(ui::input("q", "~T~opic:", "").chars(40))
                .buttons(vec![ui::button("ok", "~S~earch", true), ui::button("cancel", "Cancel", false)]);
            self.dialog = Some(d);
            return;
        }
        let lang = self.windows.iter().rev().find(|w| w.special.is_none() && w.term.is_none()).and_then(|w| w.lang.clone());
        let in_editor = self.windows.last().is_some_and(|w| w.special.is_none() && w.term.is_none());
        if in_editor && self.lsp_here().is_some() {
            self.lsp_request(LspReq::Hover);
        }
        let title = format!("Topic: {topic}");
        self.show_help(&title, &format!("# {title}\n\nSearching the guides…\n"));
        let tx = self.guide_tx.clone();
        self.guides_fetching += 1;
        std::thread::spawn(move || {
            let mut sources: Vec<(String, String)> = Vec::new();
            if let Some(g) = lang.as_deref().and_then(turbine_services::guides::find) {
                if let Ok(md) = turbine_services::guides::guide_markdown(g.key(), false) {
                    sources.push((g.title.to_string(), md));
                }
            }
            sources.push(("Turbine".into(), MANUAL.to_string()));
            let _ = tx.send(("topic".into(), title.clone(), Ok(topic_page(&title, &topic, &sources))));
        });
    }

    /// The word at the cursor (for Rename's field).
    fn word_at_cursor(&self) -> String {
        let Some(w) = self.windows.last() else { return String::new() };
        let rope = &w.ed.doc.rope;
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let (mut a, mut b) = (w.ed.cursor, w.ed.cursor);
        while a > 0 && is_word(rope.char(a - 1)) {
            a -= 1;
        }
        while b < rope.len_chars() && is_word(rope.char(b)) {
            b += 1;
        }
        rope.slice(a..b).to_string()
    }

    /// Apply a rename's edits: open files get them, others open (in the
    /// background) with them, all left unsaved to look over.
    fn apply_rename(&mut self, edits: Vec<(PathBuf, Vec<(Pos, Pos, String)>)>) {
        let front = self.windows.last().map(|w| w.win.id);
        let mut files = 0;
        let mut n = 0;
        for (path, mut list) in edits {
            let want = Self::canon(&path);
            let i = match self.windows.iter().position(|w| w.dock.is_none() && w.ed.doc.path.as_ref().is_some_and(|q| Self::canon(q) == want)) {
                Some(i) => i,
                None => {
                    self.open_path(&path);
                    self.windows.len() - 1
                }
            };
            let ed = &mut self.windows[i].ed;
            // Last first, so earlier places stay put.
            list.sort_by(|a, b| (b.0.line, b.0.character).cmp(&(a.0.line, a.0.character)));
            let to_char = |ed: &EditorView, p: Pos| {
                let line = (p.line as usize).min(ed.doc.len_lines().saturating_sub(1));
                let text = ed.doc.line(line);
                ed.doc.line_byte_to_char(line, lsp::byte_of_utf16(&text, p.character))
            };
            ed.doc.break_undo_group();
            for (s, e, t) in list {
                let (a, b) = (to_char(ed, s), to_char(ed, e));
                ed.doc.replace(a, b, &t, ed.cursor);
                n += 1;
            }
            ed.doc.break_undo_group();
            ed.cursor = ed.cursor.min(ed.doc.rope.len_chars());
            files += 1;
        }
        if let Some(id) = front {
            if let Some(i) = self.windows.iter().position(|w| w.win.id == id) {
                let w = self.windows.remove(i);
                self.windows.push(w);
            }
        }
        self.renumber();
        self.say(if n == 0 { "Nothing to rename here".to_string() } else { format!("Renamed {n} places in {files} files (save to keep them)") });
    }

    fn start_completion(&mut self) {
        let Some((key, path, pos)) = self.lsp_here() else {
            self.no_server();
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
        w.ed.doc.replace(comp.anchor, cur, &insert, cur);
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
            let fixed = matches!(w.special, Some("help" | "chat" | "preview"));
            let stale = !fixed && (w.lang_for.as_ref() != Some(&path) || (path.is_none() && w.hl_sent != Some(w.ed.doc.version)));
            if stale {
                // (Untitled: a guess from what's typed, as Tessera's input does.)
                let lang = turbine_syntax::detect(path.as_deref(), &w.ed.doc.line(0)).or_else(|| {
                    path.is_none().then(|| {
                        let end = w.ed.doc.rope.len_chars().min(4096);
                        turbine_syntax::guess(&w.ed.doc.rope.slice(..end).to_string()).map(str::to_string)
                    })?
                });
                let lang = w.lang_fixed.clone().or(lang);
                if lang != w.lang {
                    w.ed.line_comment = lang.as_deref().and_then(turbine_syntax::line_comment).map(str::to_string);
                    w.ed.colon_indents = matches!(lang.as_deref(), Some("python" | "yaml" | "nim" | "tessh"));
                    w.lang = lang;
                    w.ed.highlights.clear();
                    w.hl_sent = None;
                }
                w.lang_for = Some(path);
            }
            // (Text no grammar covers is coloured too: "text" has none.)
            if w.hl_sent != Some(w.ed.doc.version) && !fixed && (w.lang.is_some() || w.lang_for.is_some() || w.special.is_some()) {
                let lang = w.lang.as_deref().unwrap_or("text");
                self.highlighter.request(w.win.id as u64, w.ed.doc.version, lang, w.ed.doc.rope.to_string());
                w.hl_sent = Some(w.ed.doc.version);
                self.hl_pending += 1;
            }
        }
        for r in self.highlighter.results() {
            self.hl_pending = self.hl_pending.saturating_sub(1);
            if let Some(w) = self.windows.iter_mut().find(|w| w.win.id as u64 == r.id) {
                w.ed.highlights = r.lines.unwrap_or_default();
                if let Some(st) = r.structure {
                    w.ed.structure = turbine_ui::editor::Structure { folds: st.folds, nodes: st.nodes, defs: st.defs };
                    w.ed.structure_version = r.version;
                }
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
        // (In the project's folder and its direnv environment, when ready.)
        let root = self.shell.explorer.root.clone();
        let mut env: Vec<(String, String)> = env;
        if let turbine_services::env::Env::Ready(e) = turbine_services::env::project_env(&root) {
            env.extend(e);
        }
        let view = TerminalView::new(Spawn { argv, env, cwd: Some(root) });
        if let Some(e) = view.error() {
            self.say(format!("Terminal: {e}"));
        }
        self.windows.push(EdWin { win, ed: EditorView::new(Document::new()), vim: None, term: Some(view), term_text: String::new(), lang: None, lang_for: None, lang_fixed: None, hl_sent: None, lsp_key: None, lsp_sent: None, was_dirty: false, special: None, dock: Some(Side::Bottom), mirror: None });
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
        // Pictures, PDFs, media, archives: the desktop's app for them.
        match crate::filekind::classify(p) {
            crate::filekind::Kind::Text => {}
            crate::filekind::Kind::Binary => {
                let d = Dialog::new(&format!("binary:{}", p.display()), "Not a Text File")
                    .body(ui::label(&format!("{} isn't text.", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())))
                    .buttons(vec![ui::button("app", "Open in Its ~A~pp", true), ui::button("hex", "~H~ex View", false), ui::button("text", "As ~T~ext", false), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
                return;
            }
            // (Pictures show in a viewer where they can be drawn.)
            crate::filekind::Kind::Image if self.images_inline => {
                self.open_image(p);
                return;
            }
            _ => {
                match crate::filekind::open_outside(p) {
                    Ok(()) => self.say(format!("Opened {} in its app", p.display())),
                    Err(e) => self.say(format!("Can't open {}: {e}", p.display())),
                }
                return;
            }
        }
        self.open_text(p);
    }

    /// Open a file as text, whatever it is.
    fn open_text(&mut self, p: &Path) {
        match Document::open(p) {
            Ok(d) => {
                let note = if d.lossy { " (some bytes could not be decoded)" } else { "" };
                let msg = format!("Opened {} as {}{note}", p.display(), d.encoding.name());
                self.note_recent(p);
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
        let d = &self.shell.dock;
        let on = [
            ("view.line_numbers", ln),
            ("view.rtl", rtl),
            ("ai.tab", tab),
            ("view.sidebar", d.left.visible),
            ("view.panel", d.bottom.visible),
            ("view.secondary", d.right.visible),
            ("view.editor_tabs", d.editor_tabs),
            ("view.breadcrumbs", d.breadcrumbs),
            ("view.minimap", self.shell.minimap.unwrap_or(self.theme.style.icons)),
            ("selection.column", self.windows.last().is_some_and(|w| w.ed.column_mode)),
            ("view.wrap", self.shell.word_wrap),
            ("view.whitespace", self.shell.show_whitespace),
        ];
        // (Submenus' items too: Appearance's toggles.)
        for m in &mut self.menubar.menus {
            m.for_each_item_mut(&mut |it| {
                if let Some((_, v)) = on.iter().find(|(c, _)| *c == it.command) {
                    it.checked = Some(*v);
                }
            });
        }
    }

    // ----- commands ---------------------------------------------------------

    pub fn run_command(&mut self, cmd: &str) {
        if self.shell_command(cmd) {
            self.sync_menu_checks();
            return;
        }
        if self.ext_command(cmd) {
            return;
        }
        let desk = self.desktop();
        match cmd {
            "file.new" => self.new_window(Document::new()),
            "terminal.copy" | "terminal.paste" | "terminal.select_all" | "terminal.clear" | "terminal.kill" => {
                let clip = self.clipboard.clone();
                let Some(i) = self.windows.iter().rposition(|w| w.term.is_some()) else { return };
                let t = self.windows[i].term.as_mut().unwrap();
                match cmd {
                    "terminal.copy" => {
                        if let Some(s) = t.selected_text() {
                            self.copy(s);
                        }
                    }
                    "terminal.paste" => t.paste(&clip),
                    "terminal.select_all" => t.select_all(),
                    // (Ctrl+L: the shell clears its screen.)
                    "terminal.clear" => {
                        if let Some(t) = t.terminal() {
                            t.write(b"\x0c");
                        }
                    }
                    _ => {
                        self.windows.remove(i);
                        self.renumber();
                        self.layout_dock();
                    }
                }
            }
            "terminal.new" => {
                self.new_terminal(Vec::new());
                self.layout_dock();
                if let Some(id) = self.windows.last().map(|w| w.win.id) {
                    self.shell.dock.show(crate::dock::Tab::Pane(id));
                }
            }
            "file.open" => self.open_dialog(std::env::current_dir().unwrap_or_default()),
            "options.settings" => self.open_settings(),
            "file.add_folder" => {
                let root = self.shell.explorer.root.clone();
                let start = root.parent().map(Path::to_path_buf).unwrap_or(root.clone());
                let (mut f, mut d) = FileDialog::folder(start, Some(root), &self.recent_dirs);
                f.add_to_workspace = true;
                d.title = "Add Folder to Workspace".into();
                self.file_dlg = Some(f);
                self.dialog = Some(d);
            }
            "file.open_folder" => {
                let root = self.shell.explorer.root.clone();
                let start = root.parent().map(Path::to_path_buf).unwrap_or(root.clone());
                let (f, d) = FileDialog::folder(start, Some(root), &self.recent_dirs);
                self.file_dlg = Some(f);
                self.dialog = Some(d);
            }
            "file.save" => self.save(false),
            "file.save_as" => self.save(true),
            "app.quit" => {
                // (Hot exit: unsaved text is kept in the session, as VS
                // Code keeps it, when there's a session to keep it in.)
                if self.windows.iter().any(|w| w.ed.doc.dirty) && !self.hot_exit() {
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
            "help.contents" => self.show_help("Turbine", MANUAL),
            "help.back" => self.page_back(),
            "file.line_ending" => {
                // LF and CRLF, swapped (written so the next save).
                if let Some(w) = self.windows.last_mut().filter(|w| w.term.is_none()) {
                    use turbine_text::encoding::LineEnding;
                    w.ed.doc.line_ending = if w.ed.doc.line_ending == LineEnding::Lf { LineEnding::CrLf } else { LineEnding::Lf };
                    w.ed.doc.dirty = true;
                    let n = w.ed.doc.line_ending.name();
                    self.say(format!("Line ends: {n} (saved as such)"));
                }
            }
            "edit.language" => {
                let mut names: Vec<String> = turbine_syntax::BUILTIN.iter().map(|b| b.0.to_string()).collect();
                for n in turbine_syntax::grammars::known() {
                    if !names.contains(&n) {
                        names.push(n);
                    }
                }
                names.sort();
                names.insert(0, "Detect it".into());
                let cur = self.windows.last().and_then(|w| w.lang.clone()).unwrap_or_default();
                let sel = names.iter().position(|n| *n == cur).unwrap_or(0);
                let d = Dialog::new("language", "Change Language Mode")
                    .body(ui::list("lang", names, sel).activates().lines(16).chars(32))
                    .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "file.reopen_encoding" | "file.save_encoding" => {
                let save = cmd == "file.save_encoding";
                let Some(w) = self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none()) else { return };
                if w.ed.doc.path.is_none() {
                    self.say("Save the file first");
                    return;
                }
                if !save && w.ed.doc.dirty {
                    self.say("Save or undo the changes first: reopening reads the file again");
                    return;
                }
                let cur = w.ed.doc.encoding;
                let mut items: Vec<String> = crate::file_dialog::encodings().into_iter().map(|(n, _, _)| n).collect();
                let mut sel = crate::file_dialog::encodings().iter().position(|(_, l, b)| TextEncoding::for_label(l) == Some(cur) && *b == w.ed.doc.bom).unwrap_or(0);
                if !save {
                    items.insert(0, "Detect it".into());
                    sel += 1;
                }
                let title = if save { "Save with Encoding" } else { "Reopen with Encoding" };
                let d = Dialog::new(if save { "encoding:save" } else { "encoding:reopen" }, title)
                    .body(ui::list("enc", items, sel).activates().lines(16).chars(40))
                    .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "help.index" => {
                // The manual's headings, to go to one.
                let items: Vec<String> = MANUAL
                    .lines()
                    .filter_map(|l| l.strip_prefix("## ").map(|h| h.to_string()).or_else(|| l.strip_prefix("### ").map(|h| format!("  {h}"))))
                    .collect();
                let d = Dialog::new("help_index", "Index")
                    .body(ui::list("topic", items, 0).activates().lines(18).chars(36))
                    .buttons(vec![ui::button("ok", "~G~o", true), ui::button("guides", "~L~anguage Guides", false), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "help.language" => {
                let lang = self.windows.iter().rev().find(|w| w.special.is_none() && w.term.is_none()).and_then(|w| w.lang.clone());
                match lang.filter(|l| turbine_services::guides::find(l).is_some()) {
                    Some(l) => self.open_guide(&l),
                    None => self.guides_index(),
                }
            }
            "help.guides" => self.guides_index(),
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
            "help.topic" => self.topic_search(None),
            "go.definition" => self.lsp_request(LspReq::Definition),
            "build.make" | "build.all" | "build.compile" | "build.stop" | "terminal.task" | "terminal.build" | "run.without_debug" => self.build_command(cmd),
            "view.preview" => self.open_preview(),
            "view.whitespace" => {
                let on = !self.shell.show_whitespace;
                self.setting_set("show_whitespace", if on { "on" } else { "off" });
            }
            "view.wrap" => {
                let on = !self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none()).is_some_and(|w| w.ed.wrap);
                self.setting_set("word_wrap", if on { "on" } else { "off" });
            }
            c if c.starts_with("git.") => self.git_command(c),
            c if self.extras_command(c) => {}
            "tools.file_manager" => self.open_file_manager(),
            "run.watch" => self.add_watch_dialog(),
            "edit.comment" | "selection.expand" | "selection.shrink" | "selection.copy_up" | "selection.copy_down" | "selection.move_up" | "selection.move_down" | "go.bracket"
            | "go.next_def" | "go.prev_def" | "view.fold" | "view.unfold" | "view.fold_all" | "view.unfold_all" | "selection.cursor_above" | "selection.cursor_below"
            | "selection.add_next" | "selection.column" => {
                let Some(w) = self.windows.last_mut().filter(|w| w.term.is_none()) else { return };
                let ed = &mut w.ed;
                let editable = w.special.is_none() || w.special == Some("immediate");
                let done = match cmd {
                    "edit.comment" if editable => ed.toggle_comment(),
                    "selection.expand" => ed.expand_selection(),
                    "selection.shrink" => ed.shrink_selection(),
                    "selection.copy_up" if editable => ed.move_lines(false, true),
                    "selection.copy_down" if editable => ed.move_lines(true, true),
                    "selection.move_up" if editable => ed.move_lines(false, false),
                    "selection.move_down" if editable => ed.move_lines(true, false),
                    "go.bracket" => ed.goto_bracket(),
                    "go.next_def" => ed.goto_definition_line(true),
                    "go.prev_def" => ed.goto_definition_line(false),
                    "selection.cursor_above" => {
                        ed.add_cursor_line(false);
                        true
                    }
                    "selection.cursor_below" => {
                        ed.add_cursor_line(true);
                        true
                    }
                    "selection.add_next" => {
                        ed.add_next_match();
                        true
                    }
                    "selection.column" => {
                        ed.column_mode = !ed.column_mode;
                        true
                    }
                    "view.fold" => ed.fold(true),
                    "view.unfold" => ed.fold(false),
                    "view.fold_all" => {
                        ed.fold_all(true);
                        true
                    }
                    "view.unfold_all" => {
                        ed.fold_all(false);
                        true
                    }
                    _ => false,
                };
                if !done && cmd == "edit.comment" {
                    self.say("This language has no line comments");
                }
            }
            "run.evaluate" => self.evaluate_dialog(),
            "run.bp_condition" => self.breakpoint_dialog(),
            "run.immediate" => self.open_immediate(),
            "run.configurations" => self.edit_launch_json(),
            "edit.format" => self.lsp_format(),
            "edit.quick_fix" => self.lsp_quick_fix(),
            "lsp.signature" => self.lsp_signature(),
            "lsp.install" => {
                match self.windows.last().and_then(|w| w.lang.clone()) {
                    Some(l) => self.offer_install(&l),
                    None => self.say("This file has no language"),
                }
            }
            "go.workspace_symbol" => self.open_palette("#"),
            "go.references" => self.lsp_request(LspReq::References),
            "edit.rename" => {
                let word = self.word_at_cursor();
                if word.is_empty() {
                    self.say("Put the cursor on a name to rename it");
                } else {
                    let d = Dialog::new("rename_symbol", "Rename Symbol")
                        .body(ui::input("name", "New ~n~ame:", &word).chars(30))
                        .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
                    self.dialog = Some(d);
                }
            }
            "go.next_problem" | "go.prev_problem" => {
                let fwd = cmd == "go.next_problem";
                let msg = self.windows.last_mut().and_then(|w| w.ed.goto_diagnostic(fwd));
                self.say(msg.unwrap_or_else(|| "No problems".into()));
            }
            "search.goto" => {
                let d = Dialog::new("goto", "Go to Line")
                    .body(ui::input("line", "~L~ine number:", "").chars(10))
                    .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
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
                let d = Dialog::new("colors", "Colors")
                    .body(ui::col(vec![ui::label("Color scheme:"), ui::list("scheme", names, cur).activates()]))
                    .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "options.style" => {
                let names: Vec<String> = ControlStyle::all().iter().map(|s| s.name.to_string()).collect();
                let cur = names.iter().position(|n| n == self.theme.style.name).unwrap_or(0);
                let d = Dialog::new("style", "Control Style")
                    .body(ui::col(vec![ui::label("Draw controls like:"), ui::list("style", names, cur).activates()]))
                    .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "window.tile" => self.tile(),
            // (Back to every editor filling the editors' place, after Tile or Cascade.)
            "window.maximize_all" => {
                for w in self.windows.iter_mut().filter(|w| w.dock.is_none()) {
                    if !w.win.zoomed {
                        w.win.toggle_zoom(desk);
                    }
                }
            }
            "window.cascade" => {
                for (i, w) in self.windows.iter_mut().enumerate() {
                    let o = i as i32;
                    w.win.zoomed = false;
                    w.win.rect = Rect::new(desk.x + o, desk.y + o, (desk.w - o).max(20), (desk.h - o).max(6));
                }
            }
            "window.prev" => {
                if let Some(i) = self.editor_index() {
                    let w = self.windows.remove(i);
                    self.windows.insert(0, w);
                    if let Some(j) = self.editor_index() {
                        let w = self.windows.remove(j);
                        self.windows.push(w);
                    }
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
                let d = Dialog::new("winlist", "Window List")
                    .body(ui::list("windows", items, 0).activates())
                    .buttons(vec![ui::button("ok", "~G~o to", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "file.save_all" => self.vim_actions(vec![ExAction::WriteAll(false)]),
            "file.revert" => self.vim_actions(vec![ExAction::EditFile(String::new(), true)]),
            "app.repaint" => {}
            "options.keymap" => {
                let names = vec![Keymap::Cua.name().to_string(), Keymap::Vim.name().to_string()];
                let cur = if self.keymap == Keymap::Vim { 1 } else { 0 };
                let d = Dialog::new("keymap", "Keymap")
                    .body(ui::col(vec![ui::label("Editor keys:"), ui::list("keymap", names, cur).activates()]))
                    .buttons(vec![ui::button("ok", "O~K~", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "help.vim" => {
                let md = vim_help_md();
                self.show_help("Vim Keys", &md);
            }
            "window.next" => {
                // (Editors only: docked windows have their tabs.)
                if let Some(i) = self.windows.iter().position(|w| w.dock.is_none()) {
                    if self.windows.iter().filter(|w| w.dock.is_none()).count() > 1 || self.windows.last().is_some_and(|w| w.dock.is_some()) {
                        let w = self.windows.remove(i);
                        self.windows.push(w);
                        self.renumber();
                    }
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
                let d = Dialog::new("about", "About")
                    .body(ui::label(&format!(
                        "             T U R B I N E\n\nA retro, AI-first code editor and IDE\nVersion {}  -  MIT License\ngithub.com/benwbooth/turbine",
                        env!("CARGO_PKG_VERSION")
                    )))
                    .buttons(vec![ui::button("ok", "O~K~", true)]);
                self.dialog = Some(d);
            }
            "help.keys" => {
                let md = keys_help_md(&self.menubar.menus, self.keymap == Keymap::Vim);
                self.show_help("Keyboard", &md);
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
        let (f, d) = FileDialog::new(save, dir, name, project, &self.recent_dirs, fmt.0, fmt.1);
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
                // A folder typed or selected is entered, not returned
                // (Open Folder returns it).
                if let Some(t) = f.target(d) {
                    if t.is_dir() && !f.folder {
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
        if f.folder {
            let add = f.add_to_workspace;
            self.dialog = None;
            self.file_dlg = None;
            if p.is_dir() && add {
                // (Another folder beside the project; its files' servers go
                // by their own projects.)
                self.shell.explorer.add_root(p.clone());
                self.say(format!("Added {} to the workspace", p.display()));
            } else if p.is_dir() {
                self.open_folder(&p);
            } else {
                self.say(format!("{} isn't a folder", p.display()));
            }
            return;
        }
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
            let q = Dialog::new("overwrite", "Replace File")
                .body(ui::label(&format!("{name} already exists. Replace it?")))
                .buttons(vec![ui::button("yes", "~Y~es", false), ui::button("cancel", "~N~o", true)]);
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
        // (The overwrite question sits over the file dialog; a setting's
        // own little dialog over Settings.)
        if self.overwrite_q.as_ref().is_some_and(|q| q.kind.starts_with("setting:")) {
            match r {
                DialogResult::Button(b) if b == "yes" => {
                    let q = self.overwrite_q.take().unwrap();
                    self.setting_answer(&q, true);
                }
                DialogResult::Activated(_) => {
                    let q = self.overwrite_q.take().unwrap();
                    self.setting_answer(&q, true);
                }
                DialogResult::Button(_) | DialogResult::Cancel => {
                    let q = self.overwrite_q.take().unwrap();
                    self.setting_answer(&q, false);
                }
                _ => {}
            }
            return;
        }
        if self.settings_event(&r) {
            return;
        }
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
        let d = Dialog::new(&format!("unsaved:{then}"), "Unsaved Changes")
            .body(ui::label(&format!("{name} has unsaved changes.")))
            .buttons(vec![
                ui::button("save", "~S~ave", true),
                ui::button("discard", "~D~on't Save", false),
                ui::button("cancel", "Cancel", false),
            ]);
        self.dialog = Some(d);
    }

    fn finish_dialog(&mut self, d: Dialog, button: Option<String>) {
        let Some(b) = button else { return };
        if b == "cancel" {
            return;
        }
        let kind = d.kind.clone();
        if self.ex_dialog_done(&kind, &d) || self.scm_dialog_done(&kind, &d) || self.tree_edit_done(&kind, &d) || self.tree_struct_done(&kind, &d) {
            return;
        }
        if let Some(rest) = kind.strip_prefix("install:") {
            if let Some((lang, cmd)) = rest.split_once('\u{1f}') {
                let (lang, cmd) = (lang.to_string(), cmd.to_string());
                self.run_install(&lang, &cmd);
            }
            return;
        }
        if let Some(root) = kind.strip_prefix("task:") {
            let root = root.to_string();
            self.task_chosen(&d, &root);
            return;
        }
        if let Some(path) = kind.strip_prefix("binary:") {
            let p = PathBuf::from(path);
            match b.as_str() {
                "app" => {
                    if let Err(e) = crate::filekind::open_outside(&p) {
                        self.say(format!("Can't open {}: {e}", p.display()));
                    }
                }
                "hex" => {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.show_plain(&format!("{name} (hex)"), &crate::filekind::hex_dump(&p));
                }
                "text" => self.open_text(&p),
                _ => {}
            }
            return;
        }
        if let Some(rest) = kind.strip_prefix("encoding:") {
            let save = rest == "save";
            let i = d.selected_index("enc").unwrap_or(0);
            let list = crate::file_dialog::encodings();
            let Some(w) = self.windows.iter_mut().rev().find(|w| w.term.is_none() && w.special.is_none()) else { return };
            if save {
                if let Some((_, label, bom)) = list.get(i) {
                    w.ed.doc.encoding = TextEncoding::for_label(label).unwrap_or(TextEncoding::Utf8);
                    w.ed.doc.bom = *bom;
                    let r = w.ed.doc.save();
                    let name = w.ed.doc.encoding.name();
                    self.say(match r {
                        Ok(()) => format!("Saved as {name}"),
                        Err(e) => format!("Can't save: {e}"),
                    });
                }
            } else {
                // (The first choice is to detect it.)
                let enc = if i == 0 { None } else { list.get(i - 1).and_then(|(_, l, _)| TextEncoding::for_label(l)) };
                let r = w.ed.doc.reopen_as(enc);
                w.ed.cursor = w.ed.cursor.min(w.ed.doc.rope.len_chars());
                w.hl_sent = None;
                let name = w.ed.doc.encoding.name();
                self.say(match r {
                    Ok(()) => format!("Reopened as {name}"),
                    Err(e) => format!("Can't reopen: {e}"),
                });
            }
            return;
        }
        if kind == "language" {
            let pick = d.value("lang").unwrap_or_default();
            if let Some(w) = self.windows.iter_mut().rev().find(|w| w.term.is_none() && w.special.is_none()) {
                w.lang_fixed = (pick != "Detect it").then_some(pick);
                w.lang_for = None;
            }
            return;
        }
        if kind == "help_index" {
            if b == "guides" {
                self.guides_index();
                return;
            }
            let topic = d.value("topic").unwrap_or_default().trim().to_string();
            self.show_help("Turbine", MANUAL);
            if let Some(id) = self.windows.last().map(|w| w.win.id) {
                if let Some(p) = self.preview.previews.get_mut(&id) {
                    p.jump = Some(topic);
                }
            }
            return;
        }
        if matches!(kind.as_str(), "recent" | "calculator") {
            self.extras_dialog(d, &b);
            return;
        }
        if let Some(root) = kind.strip_prefix("checkout:") {
            let root = root.to_string();
            self.checkout_chosen(&d, &root);
            return;
        }
        if kind == "watch" {
            let e = d.value("expr").unwrap_or_default();
            self.add_watch(&e);
            return;
        }
        if kind == "evaluate" {
            self.evaluate_button(d, &b);
            return;
        }
        if let Some(rest) = kind.strip_prefix("bp_opts:") {
            let rest = rest.to_string();
            self.breakpoint_done(&d, &b, &rest);
            return;
        }
        if let Some(rest) = kind.strip_prefix("launch_cfg:") {
            let (stop, root) = rest.split_once('\u{1f}').unwrap_or(("0", ""));
            let (stop, root) = (stop == "1", PathBuf::from(root));
            let name = d.value("config").unwrap_or_default();
            let (_, configs) = self.launch_configs();
            if let Some(c) = configs.into_iter().find(|c| c.name == name) {
                self.dbx.config.insert(root, name);
                self.start_config(c, stop);
            }
            return;
        }
        if kind == "rename_symbol" {
            let name = d.value("name").unwrap_or_default().trim().to_string();
            if !name.is_empty() {
                self.lsp_rename(&name);
            }
            return;
        }
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
            "topic" => {
                let q = d.value("q").unwrap_or_default();
                if !q.trim().is_empty() {
                    self.topic_search(Some(q));
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
        self.tip_event(&ev);
        match ev {
            Event::Resize(w, h) => {
                let old = self.desktop();
                self.size = (w, h);
                self.menubar.width = self.menus_width();
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
            // (Let go: only a terminal in front wants to know, for programs
            // that asked kitty's keyboard protocol for it.)
            if self.dialog.is_none() && !self.menubar.is_open() {
                if let Some(t) = self.windows.last_mut().and_then(|w| w.term.as_mut()) {
                    t.handle_key(&k);
                }
            }
            return;
        }
        // The quick fix menu, then the completion list take their keys
        // first; the rest type on.
        if self.dialog.is_none() && self.lsp_menu_key(&k) {
            return;
        }
        if let Some((popup, _, acts)) = &mut self.status_menu {
            match popup.handle_key(&k) {
                PopupResult::Accept(i) => {
                    let a = acts[i].clone();
                    self.status_menu = None;
                    self.status_choice(&a);
                }
                PopupResult::Close => self.status_menu = None,
                _ => {}
            }
            return;
        }
        if let Some((popup, _, cmds)) = &mut self.ed_menu {
            match popup.handle_key(&k) {
                PopupResult::Accept(i) => {
                    let c = cmds[i];
                    self.ed_menu = None;
                    self.run_command(c);
                }
                PopupResult::Close => self.ed_menu = None,
                _ => {}
            }
            return;
        }
        if let Some((popup, ids, _)) = &mut self.shell.tab_menu {
            match popup.handle_key(&k) {
                PopupResult::Accept(i) => {
                    let id = ids[i];
                    self.shell.tab_menu = None;
                    if let Some(i) = self.windows.iter().position(|w| w.win.id == id) {
                        let w = self.windows.remove(i);
                        self.windows.push(w);
                        self.renumber();
                    }
                }
                PopupResult::Close => self.shell.tab_menu = None,
                _ => {}
            }
            return;
        }
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
        if self.dialog.is_none() && before.is_some_and(|b| self.windows.last().is_some_and(|w| w.win.id == b.0)) {
            self.lsp_after_key(&k);
        }
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
        // Layout keys, and keys for a side bar or panel view.
        if !self.menubar.is_open() && self.shell_key(&k) {
            return;
        }
        // A terminal gets every key except the few Turbine keeps: F10 (menus),
        // Alt+F3 (close) and Alt+0..9 (windows). Shells need Alt+letters.
        let reserved = k.is(Key::F(10), Mods::NONE)
            || k.is(Key::F(3), Mods::ALT)
            || (k.mods.alt && !k.mods.ctrl && matches!(k.key, Key::Char('0'..='9')));
        // A file manager takes every key but the menus' and windows'.
        if !self.menubar.is_open() && !reserved && !k.mods.alt {
            if let Some(id) = self.windows.last().map(|w| w.win.id).filter(|id| self.xtrees.contains_key(id)) {
                let out = self.xtrees.get_mut(&id).unwrap().key(&k);
                self.xtree_out(id, out);
                return;
            }
        }
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
        // A Markdown or data file's Preview or Tree has the keys (and Alt+Z).
        if self.shell.focus == Focus::Editors && self.docview_key(&k) {
            return;
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
        let shortcut = if k.mods.ctrl && k.mods.shift && !k.mods.alt && matches!(k.key, Key::Char('v' | 'V')) {
            Some("view.preview")
        } else if (k.mods.ctrl && k.mods.shift && !k.mods.alt && matches!(k.key, Key::Char('i' | 'I'))) || (k.mods.shift && k.mods.alt && !k.mods.ctrl && matches!(k.key, Key::Char('f' | 'F'))) {
            Some("edit.format")
        } else if k.is_ctrl('i') && k.key != Key::Tab {
            Some("ai.agent")
        } else if k.is_ctrl('l') {
            Some("ai.chat")
        } else if k.is(Key::Char(' '), Mods::CTRL) {
            Some("lsp.complete")
        } else if k.is(Key::Char(' '), Mods { ctrl: true, shift: true, ..Mods::NONE }) {
            Some("lsp.signature")
        } else if k.is_ctrl('.') {
            Some("edit.quick_fix")
        } else if k.is_alt('z') {
            Some("view.wrap")
        } else if k.mods.ctrl && k.mods.alt && !k.mods.shift && matches!(k.key, Key::Char('f' | 'F')) {
            Some("tools.file_manager")
        } else if k.is(Key::Left, Mods::ALT) && self.windows.last().is_some_and(|w| w.term.is_none()) {
            Some("go.back")
        } else if k.is(Key::Right, Mods::ALT) && self.windows.last().is_some_and(|w| w.term.is_none()) {
            Some("go.forward")
        } else if k.is(Key::PageDown, Mods::ALT) {
            Some("go.next_def")
        } else if k.is(Key::PageUp, Mods::ALT) {
            Some("go.prev_def")
        } else if k.is(Key::F(12), Mods::NONE) {
            Some("go.definition")
        } else if k.is(Key::F(12), Mods::SHIFT) {
            Some("go.references")
        } else if k.is(Key::F(2), Mods::SHIFT) {
            Some("edit.rename")
        } else if k.is(Key::F(1), Mods::CTRL) {
            Some("help.topic")
        } else if k.is(Key::F(9), Mods::CTRL) {
            Some("run.run")
        } else if k.is(Key::F(9), Mods::NONE) {
            Some("build.make")
        } else if k.is(Key::F(9), Mods::ALT) {
            Some("build.compile")
        } else if k.is(Key::F(5), Mods::CTRL) {
            Some("run.without_debug")
        } else if k.mods.ctrl && k.mods.shift && !k.mods.alt && matches!(k.key, Key::Char('b' | 'B')) {
            Some("terminal.build")
        } else if k.is(Key::F(7), Mods::CTRL) {
            Some("run.watch")
        } else if k.is(Key::F(4), Mods::CTRL) {
            Some("run.evaluate")
        } else if k.is(Key::F(8), Mods { ctrl: true, shift: true, ..Mods::NONE }) {
            Some("run.bp_condition")
        } else if k.mods.ctrl && k.mods.shift && !k.mods.alt && matches!(k.key, Key::Char('y' | 'Y')) {
            Some("run.immediate")
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
        } else if k.is(Key::F(1), Mods::ALT) && self.windows.last().is_some_and(|w| matches!(w.special, Some("help" | "preview"))) {
            Some("help.back")
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
        if self.image_key(&k) {
            return;
        }
        // The Immediate window runs its lines.
        if self.immediate_key(&k) {
            return;
        }
        // Enter on a rendered page's link follows it.
        if k.key == Key::Enter && k.mods == Mods::NONE && self.windows.last().is_some_and(|w| matches!(w.special, Some("help" | "preview"))) && self.follow_link() {
            return;
        }
        // A diff's or a history's keys (s, n, p, Enter).
        if self.windows.last().is_some_and(|w| matches!(w.special, Some("help" | "preview"))) && self.page_key(&k) {
            return;
        }
        // Help, Debug and Output windows are read-only: only moving around.
        if self.windows.last().is_some_and(|w| w.special.is_some() && w.special != Some("immediate")) {
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
        // (:sp / :vs with no file: this one again, in a group below / beside.)
        if let Some(vertical) = split {
            self.split_editor(vertical, false);
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
            "split" => self.split_editor(false, false),
            "vsplit" => self.split_editor(true, false),
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
        let click = matches!(m.kind, MouseKind::Up(Button::Left)) && self.dialog.is_none();
        self.mouse_inner(m);
        // A click on a rendered page's link follows it.
        if click && self.windows.last().is_some_and(|w| matches!(w.special, Some("help" | "preview")) && w.ed.selection().is_none()) {
            self.follow_link();
        }
    }

    fn mouse_inner(&mut self, m: MouseEvent) {
        let screen = self.screen();
        let desk = self.desktop();
        if self.dialog.is_none() && self.lspx.menu.is_some() && self.lsp_menu_mouse(&m) {
            return;
        }
        if let Some((popup, _, acts)) = &mut self.status_menu {
            match popup.handle_mouse(&m) {
                PopupResult::Accept(i) => {
                    let a = acts[i].clone();
                    self.status_menu = None;
                    self.status_choice(&a);
                }
                PopupResult::Close => self.status_menu = None,
                PopupResult::Ignored if matches!(m.kind, MouseKind::Down(_)) => self.status_menu = None,
                _ => {}
            }
            return;
        }
        // The editor's right-click menu.
        if let Some((popup, _, cmds)) = &mut self.ed_menu {
            match popup.handle_mouse(&m) {
                PopupResult::Accept(i) => {
                    let c = cmds[i];
                    self.ed_menu = None;
                    self.run_command(c);
                }
                PopupResult::Close => self.ed_menu = None,
                PopupResult::Ignored if matches!(m.kind, MouseKind::Down(_)) => self.ed_menu = None,
                _ => {}
            }
            return;
        }
        if self.dialog.is_none() && matches!(m.kind, MouseKind::Down(Button::Right)) && (self.ed_menu_open(m.pos) || self.term_menu_open(m.pos)) {
            return;
        }
        if let Some((popup, ids, _)) = &mut self.shell.tab_menu {
            match popup.handle_mouse(&m) {
                PopupResult::Accept(i) => {
                    let id = ids[i];
                    self.shell.tab_menu = None;
                    if let Some(i) = self.windows.iter().position(|w| w.win.id == id) {
                        let w = self.windows.remove(i);
                        self.windows.push(w);
                        self.renumber();
                    }
                }
                PopupResult::Close => self.shell.tab_menu = None,
                PopupResult::Consumed => {}
                PopupResult::Ignored => {
                    if matches!(m.kind, MouseKind::Down(_)) {
                        self.shell.tab_menu = None;
                    }
                }
            }
            return;
        }
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
        // The top row: ← →, the command center, the toggles, the window's
        // buttons and its free space (the menus' titles are the menu bar's).
        if self.top_mouse(&m) {
            return;
        }
        match self.menubar.handle_mouse(&m, screen) {
            MenuResult::Command(c) => return self.run_command(&c),
            MenuResult::Consumed => return,
            MenuResult::Ignored => {}
        }
        // A file manager under the pointer.
        if let Some(id) = self.windows.iter().rev().find(|w| self.xtrees.contains_key(&w.win.id) && w.win.rect.contains(m.pos)).map(|w| w.win.id) {
            if matches!(m.kind, MouseKind::Down(_)) && self.windows.last().is_some_and(|w| w.win.id != id) {
                if let Some(i) = self.windows.iter().position(|w| w.win.id == id) {
                    let w = self.windows.remove(i);
                    self.windows.push(w);
                    self.renumber();
                }
            }
            let out = self.xtrees.get_mut(&id).unwrap().mouse(&m);
            self.xtree_out(id, out);
            return;
        }
        if let Some((i, horizontal, grab)) = self.scroll_drag {
            match m.kind {
                MouseKind::Drag(Button::Left) if i < self.windows.len() => {
                    let skip = self.hbar_skip();
                    let inside = self.hbar_inside(i);
                    self.bar_at(i, |w| bar_drag(w, horizontal, inside, skip, m.pos, grab));
                }
                MouseKind::Drag(_) => {}
                _ => self.scroll_drag = None,
            }
            return;
        }
        if self.shell_mouse(&m) || self.find_mouse(&m) || (self.dialog.is_none() && self.docview_mouse(&m)) {
            return;
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
                    // (The file's items open a list right above them.)
                    if self.status_menu_open(&c, m.pos) {
                        return;
                    }
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
                let on_inside_bar = self.windows[i].term.is_none() && self.hbar_inside(i) && m.pos.y == self.windows[i].ed.area().bottom() - 1;
                let on_inside_vbar = {
                    let w = &self.windows[i];
                    let r = w.win.rect;
                    w.ed.vbar_inside && m.pos.x == r.right() - 2 && m.pos.y > r.y && m.pos.y < r.bottom() - 1
                };
                let skip = if self.theme.style.window_numbers && self.windows[i].win.rect.w > 20 { 13 } else { 2 };
                let w = &mut self.windows[i];
                // (A docked window's frame belongs to its part: no moving,
                // resizing, closing or zooming it from there.)
                let hit = match w.win.hit(m.pos) {
                    h @ (Hit::Inside | Hit::VScroll) => h,
                    _ if w.dock.is_some() => Hit::None,
                    h => h,
                };
                match hit {
                    Hit::Close => self.run_command("window.close"),
                    Hit::Zoom => w.win.toggle_zoom(desk),
                    Hit::Title if double => w.win.toggle_zoom(desk),
                    h @ (Hit::Title | Hit::Left | Hit::Right | Hit::Bottom | Hit::BottomLeft | Hit::BottomRight) => {
                        if w.win.begin_drag(h, m.pos) {
                            self.dragging = Some(i);
                        }
                    }
                    Hit::VScroll | Hit::HScroll if w.term.is_none() || hit == Hit::VScroll => {
                        let horizontal = hit == Hit::HScroll;
                        if let Some(grab) = self.bar_at(i, |w| bar_press(w, horizontal, false, skip, m.pos)) {
                            self.scroll_drag = Some((i, horizontal, grab));
                        }
                    }
                    // The up-down bar a column in (the right side bar beside it).
                    Hit::Inside if on_inside_vbar => {
                        if let Some(grab) = self.bar_at(i, |w| bar_press(w, false, true, skip, m.pos)) {
                            self.scroll_drag = Some((i, false, grab));
                        }
                    }
                    // The sideways bar over the last row (the panel under it).
                    Hit::Inside if on_inside_bar => {
                        if let Some(grab) = self.bar_at(i, |w| bar_press(w, true, true, skip, m.pos)) {
                            self.scroll_drag = Some((i, true, grab));
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
            MouseKind::ScrollUp | MouseKind::ScrollDown | MouseKind::ScrollLeft | MouseKind::ScrollRight => {
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
                // (An open submenu's items, beside it.)
                if let Some(parent) = m.items.get(self.menubar.item_sel).filter(|_| self.menubar.sub_open) {
                    for (j, it) in parent.submenu.iter().enumerate() {
                        if it.is_separator() {
                            continue;
                        }
                        let Some(r) = self.menubar.sub_item_rect(j, screen) else { continue };
                        let mut n = Node::new(format!("menuitem.{}", it.command), "menuitem", it.plain_label(), r).enabled(it.enabled);
                        if let Some(c) = it.checked {
                            n = n.value(if c { "checked" } else { "unchecked" });
                        }
                        mn = mn.child(n);
                    }
                }
            }
            bar = bar.child(mn);
        }
        for it in self.top_items() {
            let id = if it.action.starts_with("view.") { format!("layout.{}", it.action) } else { format!("top.{}", it.action) };
            let mut n = Node::new(id, "button", it.name, it.rect).tip(it.name);
            if it.action.starts_with("view.") {
                n = n.value(if it.on { "on" } else { "off" });
            }
            bar = bar.child(n);
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
        root = self.shell_nodes(root);
        root = self.find_nodes(root);
        root = self.ex_nodes(root);
        root = self.inline_nodes(root);
        root = self.lsp_more_nodes(root);
        root = self.docview_nodes(root);
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
        if self.status.left.is_empty() && self.status.right.is_empty() {
            for (it, r) in self.status.items.iter().zip(self.status.item_rects(srow)) {
                sb = sb.child(Node::new(format!("status.{}", it.command), "button", format!("{} {}", it.key, it.label), r));
            }
        }
        for (r, cmd, tip, text) in self.status.segment_rects(Rect::new(0, srow, screen.w, 1)) {
            let id = if cmd.is_empty() { format!("status.{text}") } else { format!("status.{cmd}") };
            sb = sb.child(Node::new(id, "button", text, r).tip(tip));
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
        self.queue_menu("View/Editor Layout/Tile");
        self.queue_menu("File/Preferences/Colors");
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
                self.layout_dialogs();
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
                Output::Resolve(t) => match {
                    self.layout_dialogs();
                    self.tree().find_target(&t).cloned()
                } {
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

/// The window's layout: the client area is all of it. (With Turbine's own
/// title bar, the menu bar's row is the title bar too, as VS Code's: the
/// window's buttons at its right end, its free space moves the window.)
impl Workbench {
    /// The client area's place in the window (where Turbine's row 0 is).
    pub fn client_area(&self) -> Rect {
        let (w, h) = self.outer;
        Rect::new(0, 0, w, h.max(4))
    }

    /// An event in the window's terms.
    fn frame_event(&mut self, ev: Event) {
        if let Event::Resize(w, h) = ev {
            self.outer = (w, h);
        }
        self.event_inner(ev);
    }
}

impl Workbench {
    fn event_inner(&mut self, ev: Event) {
        self.last_input = Some(Instant::now());
        // (A tooltip the pointer is on takes its wheel and clicks.)
        if let Event::Mouse(m) = &ev {
            if self.tip_mouse(m) {
                return;
            }
        }
        // (The mouse leaving the window: only the tooltip cares.)
        if matches!(&ev, Event::Mouse(m) if m.pos.x < 0 || m.pos.y < 0) {
            self.tip_event(&ev);
            return;
        }
        self.human_input(&ev);
        if self.driver.busy() && matches!(ev, Event::Key(_)) {
            return; // keys go to the agent controls while it drives
        }
        self.dispatch(ev);
    }

}

impl App for Workbench {
    fn event(&mut self, ev: Event) {
        self.frame_event(ev);
    }

    fn tick(&mut self, now: Instant) -> Option<Instant> {
        // (Agents find controls by where they are: lay dialogs and parts out
        // now, not only when the screen is drawn.)
        self.sync_mirrors();
        self.layout_dock();
        self.update_views();
        self.ex_poll();
        self.ext_poll();
        self.layout_dialogs();
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
        self.lsp_more_tick(now);
        self.build_tick();
        self.preview_tick(now);
        self.git_tick();
        self.preview_tab_tick();
        self.images_tick();
        self.session_tick(now);
        self.docview_tick();
        self.gitlines_tick();
        // (Closed file managers' state goes; the rest take their answers.)
        let ids: HashSet<u32> = self.windows.iter().map(|w| w.win.id).collect();
        self.xtrees.retain(|id, _| ids.contains(id));
        for x in self.xtrees.values_mut() {
            x.poll();
        }
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
            || self.last_edit.is_some()
            || self.build_busy()
            || self.preview_busy();
        let term_tick = (any_term || self.hl_pending > 0 || lsp_waiting).then(|| now + Duration::from_millis(16));
        // Servers send diagnostics unasked: look a few times a second.
        let lsp_tick = (!self.lsp.is_empty()).then(|| now + Duration::from_millis(100));
        let tip_tick = self.tip_tick(now);
        let lively = self.last_input.filter(|t| now.saturating_duration_since(*t) < Duration::from_millis(1500)).map(|_| now + Duration::from_millis(16));
        [next, msg_tick, poll, term_tick, lsp_tick, tip_tick, lively].into_iter().flatten().min()
    }

    fn render(&mut self, surf: &mut Surface) {
        self.render_client(surf)
    }

    fn should_quit(&self) -> bool {
        self.quit
    }

    fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard_out.take()
    }
}

impl Workbench {
    fn render_client(&mut self, surf: &mut Surface) {
        // The cursor setting: editors insert with it; fields and dialogs use
        // it too (vim's normal mode keeps its block).
        let shape = match self.shell.cursor_style.as_str() {
            "Underline" => turbine_ui::CursorShape::Underline,
            "Bar" => turbine_ui::CursorShape::Bar,
            _ => turbine_ui::CursorShape::Block,
        };
        for w in &mut self.windows {
            w.ed.insert_shape = shape;
        }
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
        let panel_top = self.layout().bottom.map(|b| b.y);
        let right_edge = self.layout().right.map(|r| r.x);
        // (Editor groups: an edge shared with another group is like one
        // shared with a part: the bars a row or column in.)
        let ed_area = self.layout().editors;
        let grouped = self.group_count() > 1;
        let bottom_shared = |r: Rect| panel_top == Some(r.bottom() - 1) || (grouped && r.bottom() != ed_area.bottom());
        self.image_places.clear();
        let mut to_view: Vec<(u32, Rect)> = Vec::new();
        for (i, w) in self.windows.iter_mut().enumerate() {
            if w.dock.is_some() {
                continue;
            }
            let active = i + 1 == n && self.shell.focus == Focus::Editors;
            let mut wc = c.sub(desk);
            if theme.style.shadows {
                wc.shadow(w.win.rect, theme.scheme.shadow);
            }
            // (A help page's frame in the help colours: QBasic's white lines
            // on black, VB-DOS's none at the sides.)
            if w.ed.help_look {
                let mut ht = theme.clone();
                let h = theme.scheme.help;
                let line = if theme.style.title_bar { Style::new(h.bg, h.bg) } else { Style::new(h.fg, h.bg) };
                ht.scheme.frame = line;
                ht.scheme.frame_active = line;
                w.win.draw_frame(&mut wc, &ht, active);
            } else {
                w.win.draw_frame(&mut wc, &theme, active);
            }
            let inner = w.win.inner();
            if w.special == Some("image") {
                to_view.push((w.win.id, inner));
                continue;
            }
            // A file manager: XTree's screen over the whole window.
            if let Some(x) = self.xtrees.get_mut(&w.win.id) {
                x.draw(&mut wc, w.win.rect, active && !has_modal);
                continue;
            }
            if let Some(t) = &mut w.term {
                t.draw(&mut wc, inner, &theme, active && !has_modal);
                // (The scrollback's bar, once there is any.)
                if let Some((total, offset, len)) = t.terminal().and_then(|t| t.scrollbar()) {
                    if total > len {
                        let r = w.win.rect;
                        let max = total - len;
                        turbine_ui::frame::draw_vscroll(&mut wc, r.right() - 1, r.y + 1, r.h - 2, offset.min(max), len, total, theme.scheme.scrollbar, theme.style.scroll);
                    }
                }
                continue;
            }
            // (Markdown and data files: a toolbar, and Preview or Tree.)
            let viewed = docview::draw(w, self.docviews.get_mut(&w.win.id), &mut wc, &theme, active && !has_modal);
            // (Against the right side bar: the bar a column in, the edge its.)
            w.ed.vbar_inside = w.term.is_none() && (right_edge == Some(w.win.rect.right() - 1) || (grouped && w.win.rect.right() != ed_area.right()));
            if !viewed {
                let mut body = Self::doc_body(w);
                if w.ed.vbar_inside {
                    body.w -= 1;
                }
                w.ed.draw(&mut wc, body, &theme, active && !has_modal, active);
            }
            if active && !viewed {
                w.ed.hbar_over_last_row = bottom_shared(w.win.rect);
                w.ed.draw_scrollbars(&mut wc, w.win.rect, &theme);
                // (Its bottom edge is the panel's top: the bar over the last row.)
                if bottom_shared(w.win.rect) {
                    w.ed.draw_hscroll_inside(&mut wc, w.win.rect, &theme);
                }
                if w.ed.doc.dirty {
                    wc.print(w.win.rect.x + 1, w.win.rect.bottom() - 1, "*", theme.scheme.frame_icons);
                }
                // Borland: the line and column in the bottom edge (═ 72:4 ═).
                if theme.style.window_numbers && w.win.rect.w > 20 {
                    let (l, col) = w.ed.cursor_line_col();
                    let r = w.win.rect;
                    let at = format!(" {}:{} ", l + 1, col + 1);
                    wc.fill(Rect::new(r.x + 2, r.bottom() - 1, 11, 1), theme.style.active_frame.h, theme.scheme.frame_active);
                    wc.print(r.x + 12 - turbine_text::unicode::str_width(&at) as i32, r.bottom() - 1, &at, theme.scheme.frame_active);
                }
            }
        }
        // Picture viewers (only the front one's picture, as windows overlap).
        for (id, inner) in to_view {
            let mut vc = c.sub(desk);
            self.draw_viewer(&mut vc, id, inner);
        }
        let front = self.windows.iter().rev().find(|w| w.dock.is_none()).map(|w| w.win.id);
        let front_rect = self.windows.iter().find(|w| Some(w.win.id) == front).map(|w| w.win.rect);
        if let Some(fr) = front_rect {
            self.image_places.retain(|(r, _)| fr.contains(Point::new(r.x, r.y)));
        }
        self.sync_mirrors();
        self.layout_dock();
        self.draw_groups(&mut c, &theme);
        self.draw_shell(&mut c, &theme);
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
                // As Cursor's: the branch and problem counts, then the file's.
                let file = self.windows.last().and_then(|w| w.ed.doc.path.as_deref());
                let branch = &self.shell.scm.branch_of(file);
                let (e, wn) = self.shell.problem_count;
                format!(
                    "{}■ {e} ▲ {wn}  {lang}  {}:{}  {} {}  {}",
                    if branch.is_empty() { String::new() } else { format!("{} {branch}  ", if self.theme.style.icons { "\u{e725}" } else { "⎇" }) },
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
        // VS Code's status items, each a click away (the key hints make
        // room for them).
        self.status.width_hint = self.size.0;
        self.status.left.clear();
        self.status.right.clear();
        if let Some(w) = self.windows.last().filter(|w| w.term.is_none()) {
            let icons = self.theme.style.icons;
            let branch = self.shell.scm.branch_of(w.ed.doc.path.as_deref());
            if !branch.is_empty() {
                self.status.left.push((format!("{} {branch}", if icons { "\u{e725}" } else { "⎇" }), "view.scm".into(), format!("Branch {branch}: Source Control")));
            }
            let (e, wn) = self.shell.problem_count;
            self.status.left.push((format!("■ {e} ▲ {wn}"), "view.problems".into(), "Errors and warnings: the Problems panel".into()));
            if let Some((mode, _)) = &vim_line {
                self.status.right.push((mode.clone(), String::new(), "Vim's mode".into()));
            }
            let (l, col) = w.ed.cursor_line_col();
            self.status.right.push((format!("Ln {}, Col {}", l + 1, col + 1), "search.goto".into(), "Go to Line (Ctrl+G)".into()));
            if let Some((a, b)) = w.ed.selection() {
                self.status.right.push((format!("({} selected)", b - a), String::new(), "Characters selected".into()));
            }
            self.status.right.push((w.ed.doc.encoding.name().to_string(), "file.reopen_encoding".into(), "Reopen with Encoding".into()));
            self.status.right.push((w.ed.doc.line_ending.name().to_string(), "file.line_ending".into(), "Change the line ends (LF or CRLF)".into()));
            let lang = w.lang.as_deref().map(turbine_syntax::label).unwrap_or_else(|| "Plain Text".into());
            self.status.right.push((lang, "edit.language".into(), "Change Language Mode".into()));
            if w.vim.is_none() {
                self.status.right.push((if w.ed.overwrite { "OVR" } else { "INS" }.into(), String::new(), "Typing inserts (Insert switches to typing over)".into()));
            }
        }
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
        // Vim's ':' line: the commands it could be, with what each does,
        // in a box over the status line (Tab picks the lit one).
        if let Some(v) = self.windows.last().and_then(|w| w.vim.as_ref()).filter(|_| self.dialog.is_none()) {
            let (choices, pick) = v.command_choices();
            if !choices.is_empty() {
                let n = choices.len().min(10);
                let first = pick.saturating_sub(n - 1).min(choices.len() - n);
                let name_w = choices.iter().map(|c| str_width(&c.0)).max().unwrap_or(0).min(28);
                let w = (choices.iter().map(|c| name_w + 2 + str_width(&c.1)).max().unwrap_or(0) + 4).min(self.size.0 as usize - 2) as i32;
                let r = Rect::new(0, self.size.1 - 1 - n as i32 - 2, w, n as i32 + 2);
                let s = &theme.scheme;
                c.fill(r, " ", s.menu);
                turbine_ui::frame::draw_frame(&mut c, r, theme.style.menu_frame, s.menu);
                for (k, (name, help)) in choices.iter().enumerate().skip(first).take(n) {
                    let y = r.y + 1 + (k - first) as i32;
                    let st = if k == pick { s.menu_selected } else { s.menu };
                    let line = Rect::new(r.x + 1, y, r.w - 2, 1);
                    c.fill(line, " ", st);
                    let mut lc = c.sub(line);
                    lc.print(r.x + 2, y, name, Style { fg: if k == pick { st.fg } else { s.menu_hotkey.fg }, ..st });
                    lc.print(r.x + 2 + name_w as i32 + 2, y, help, st);
                }
                if theme.style.shadows {
                    c.shadow(r, s.shadow);
                }
            }
        }
        if let Some((_, Some(col), _)) = &cmdline {
            if self.dialog.is_none() && !self.driver.busy() {
                c.set_cursor(Point::new(1 + *col as i32, self.size.1 - 1), turbine_ui::CursorShape::Bar);
            }
        }
        self.menubar.width = self.menus_width();
        self.menubar.draw(&mut c, &theme, screen);
        // (The rest of the top row: ← → and the command center, the layout
        // toggles lit when their part shows, the window's buttons.)
        self.draw_top_items(&mut c, &theme);
        if let Some(comp) = &mut self.completion {
            let at = self.windows.last().filter(|w| w.win.id == comp.win_id).and_then(|w| w.ed.cursor_screen_pos());
            if let (Some(at), false) = (at, comp.popup.items.is_empty()) {
                comp.popup.draw(&mut c, at, desk, &theme);
            }
        }
        self.draw_lsp_more(&mut c, &theme);
        if let Some((popup, at, _)) = &mut self.ed_menu {
            let at = *at;
            popup.draw(&mut c, at, screen, &theme);
        }
        if let Some((popup, at, _)) = &mut self.status_menu {
            let at = *at;
            popup.draw(&mut c, at, screen, &theme);
        }
        if let Some((popup, _, at)) = &mut self.shell.tab_menu {
            let at = *at;
            popup.draw(&mut c, at, screen, &theme);
        }
        self.draw_ex(&mut c, &theme);
        self.draw_docview_menu(&mut c, &theme);
        self.draw_find(&mut c, &theme);
        self.draw_inline(&mut c, &theme);
        self.draw_palette(&mut c, &theme);
        self.layout_dialogs();
        for d in [&mut self.dialog, &mut self.overwrite_q].into_iter().flatten() {
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
        // (Side bars and the panel share their border with the editors.)
        let l = self.layout();
        let mut parts: Vec<Rect> = [l.left, l.bottom, l.right].into_iter().flatten().collect();
        if self.group_count() > 1 {
            parts.extend(self.groups.rects.iter().map(|(_, r)| *r));
        }
        if self.dialog.is_none() && !self.menubar.is_open() {
            shell::join_borders(surf, &parts);
        }
        self.draw_tip(&mut surf.canvas(), &theme);
        surf.cursor_blink = self.shell.cursor_blink;
        // (No pictures under a dialog or a menu.)
        surf.images = if self.dialog.is_none() && !self.menubar.is_open() && self.shell.palette.is_none() { std::mem::take(&mut self.image_places) } else { Vec::new() };
        let in_editor = self.dialog.is_none() && self.overwrite_q.is_none() && self.shell.focus == Focus::Editors && self.shell.palette.is_none();
        if let Some((p, s)) = surf.cursor {
            if !in_editor && s != shape {
                surf.cursor = Some((p, shape));
            }
        }
    }
}

/// A window's view along a bar: (first shown, how many shown, of how
/// many). Editors: lines, or columns sideways; terminals: the scrollback.
fn bar_view(w: &mut EdWin, horizontal: bool) -> (usize, usize, usize) {
    if let Some(t) = &mut w.term {
        return match t.terminal().and_then(|t| t.scrollbar()) {
            Some((total, offset, len)) => (offset, len, total),
            None => (0, 1, 1),
        };
    }
    let e = &w.ed;
    if horizontal {
        let shown = e.text_width();
        (e.scroll_x, shown, e.longest_in_view().max(e.scroll_x + shown))
    } else {
        let shown = e.area().h.max(1) as usize;
        (e.scroll_y, shown, e.doc.len_lines().max(e.scroll_y + 1))
    }
}

/// Scroll the view (never the cursor, as VS Code's bars do).
fn set_bar_first(w: &mut EdWin, horizontal: bool, first: usize) {
    if let Some(t) = &mut w.term {
        if let Some(t) = t.terminal() {
            t.scroll_to_row(first);
        }
        return;
    }
    if horizontal {
        w.ed.scroll_x = first;
    } else {
        w.ed.scroll_y = first.min(w.ed.doc.len_lines().saturating_sub(1));
    }
}

/// A bar's track: (its first cell along the bar, its length), past the
/// arrows. `inside`: the sideways bar drawn over the last row.
fn bar_track(w: &EdWin, horizontal: bool, inside: bool, skip: i32) -> (i32, i32) {
    let r = w.win.rect;
    if !horizontal {
        // (A row shorter when the sideways bar is over the last row: the corner.)
        let corner = i32::from(w.ed.hbar_over_last_row && w.ed.hbar_inside_shown());
        return (r.y + 2, (r.h - 4 - corner).max(0));
    }
    if inside {
        let x = w.ed.area().x + (w.ed.area().w - w.ed.text_width() as i32);
        return (x + 1, (r.right() - 1 - x - 2).max(0));
    }
    (r.x + skip + 1, (r.w - skip - 4).max(0))
}

/// A press on a bar: an arrow scrolls a step; the track scrolls there at
/// once (the thumb centred on the press) and drags on; the thumb drags
/// from where it was taken. The grab point (within the thumb) to drag
/// with, unless it was an arrow.
fn bar_press(w: &mut EdWin, horizontal: bool, inside: bool, skip: i32, p: Point) -> Option<usize> {
    let (start, track) = bar_track(w, horizontal, inside, skip);
    let at = if horizontal { p.x } else { p.y };
    let (first, shown, total) = bar_view(w, horizontal);
    if at < start || at >= start + track {
        // The arrows: a step.
        let step = if horizontal { 4 } else { 1 };
        let to = if at < start { first.saturating_sub(step) } else { (first + step).min(total.saturating_sub(shown)) };
        set_bar_first(w, horizontal, to);
        return None;
    }
    let (t_at, t_len) = turbine_ui::frame::thumb(track as usize, first, shown, total);
    let off = (at - start) as usize;
    let grab = if t_len > 0 && off >= t_at && off < t_at + t_len { off - t_at } else { t_len / 2 };
    bar_drag(w, horizontal, inside, skip, p, grab);
    Some(grab)
}

/// Dragging a thumb: its start follows the pointer, less where it was held.
fn bar_drag(w: &mut EdWin, horizontal: bool, inside: bool, skip: i32, p: Point, grab: usize) {
    let (start, track) = bar_track(w, horizontal, inside, skip);
    let (_, shown, total) = bar_view(w, horizontal);
    let (_, t_len) = turbine_ui::frame::thumb(track as usize, 0, shown, total);
    let room = (track as usize).saturating_sub(t_len);
    let span = total.saturating_sub(shown);
    if room == 0 || span == 0 {
        return;
    }
    let at = (if horizontal { p.x } else { p.y }) - start - grab as i32;
    let at = at.clamp(0, room as i32) as usize;
    set_bar_first(w, horizontal, (at * span + room / 2) / room);
}

/// Topic Search's page: each section (under its heading) of each source
/// that has the topic as a word, the paragraph or code with it quoted.
fn topic_page(title: &str, topic: &str, sources: &[(String, String)]) -> String {
    let needle = topic.to_lowercase();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let has = |line: &str| {
        let l = line.to_lowercase();
        l.match_indices(&needle).any(|(i, _)| {
            let before = l[..i].chars().next_back().is_none_or(|c| !is_word(c));
            let after = l[i + needle.len()..].chars().next().is_none_or(|c| !is_word(c));
            before && after
        })
    };
    let mut md = format!("# {title}\n");
    let mut found = 0;
    for (name, text) in sources {
        let mut hits = String::new();
        let mut heading = String::new();
        let mut shown_heading = String::new();
        // (A paragraph or code block at a time: the one with the topic, quoted.)
        let mut block: Vec<&str> = Vec::new();
        let mut in_code = false;
        let mut n = 0;
        let flush = |block: &mut Vec<&str>, heading: &str, shown: &mut String, hits: &mut String, n: &mut usize| {
            if *n < 12 && block.iter().any(|l| has(l)) {
                if shown != heading {
                    hits.push_str(&format!("\n### {}\n\n", if heading.is_empty() { name.as_str() } else { heading }));
                    *shown = heading.to_string();
                }
                let lines: Vec<&str> = block.iter().take(14).copied().collect();
                hits.push_str(&lines.join("\n"));
                hits.push_str("\n\n");
                *n += 1;
            }
            block.clear();
        };
        for line in text.lines() {
            if line.trim_start().starts_with("```") {
                block.push(line);
                if in_code {
                    flush(&mut block, &heading, &mut shown_heading, &mut hits, &mut n);
                }
                in_code = !in_code;
                continue;
            }
            if in_code {
                block.push(line);
                continue;
            }
            if let Some(h) = line.strip_prefix('#') {
                flush(&mut block, &heading, &mut shown_heading, &mut hits, &mut n);
                heading = h.trim_start_matches('#').trim().to_string();
                continue;
            }
            if line.trim().is_empty() {
                flush(&mut block, &heading, &mut shown_heading, &mut hits, &mut n);
            } else {
                block.push(line);
            }
        }
        flush(&mut block, &heading, &mut shown_heading, &mut hits, &mut n);
        if n > 0 {
            md.push_str(&format!("\n## In {name}\n{hits}"));
            found += n;
        }
    }
    if found == 0 {
        md.push_str(&format!("\nNothing about `{topic}` in {}. Help > Language Guides lists every guide; Ctrl+Shift+F searches the project.\n", sources.iter().map(|s| s.0.as_str()).collect::<Vec<_>>().join(" or ")));
    }
    md
}

/// A table cell's text as code: its pipes escaped (a table's columns).
fn md_key(k: &str) -> String {
    format!("`{}`", k.replace('|', "\\|"))
}

/// Help > Vim Keys: each of vi's groups a table (keys, what they do), then
/// Turbine's own (macros, windows, files).
fn vim_help_md() -> String {
    let mut md = String::from("# Vim Keys\n\nOptions > Keymap > Vim turns these on. A count first repeats most of them (`3w`, `5j`); `:` opens the command line (Tab completes, with what each does).\n");
    let table = |md: &mut String, title: &str, rows: &[(&str, &str)]| {
        md.push_str(&format!("\n## {title}\n\n| Keys | What they do |\n|:--|:--|\n"));
        for (k, what) in rows {
            md.push_str(&format!("| {} | {} |\n", md_key(k), what.replace('|', "\\|")));
        }
    };
    for (section, keys) in turbine_vi::vi::Vi::help() {
        let title = section.strip_prefix("vi: ").unwrap_or(section);
        let mut t: String = title.chars().take(1).flat_map(char::to_uppercase).collect();
        t.push_str(&title.chars().skip(1).collect::<String>());
        table(&mut md, &t, &keys);
    }
    table(
        &mut md,
        "Turbine's own",
        &[
            ("q{reg} … q", "record a macro"),
            ("[n]@{reg}  @@", "play a macro (@: repeats the last :)"),
            ("Ctrl-W w  Ctrl-W W", "next / previous window"),
            ("Ctrl-W h j k l", "the window to the left, below, above, right"),
            ("Ctrl-W q  o  n", "close / only this one / new window"),
            (":e :w :q :wq :x", "files and windows, as vim"),
            (":sp :vs {file}", "open a file beside this one"),
            (":bn :bp :new :only", "windows as buffers"),
        ],
    );
    md
}

/// Help > Keyboard: every menu's shortcuts (from the menus themselves, so
/// it's never out of date), then the keys no menu shows.
fn keys_help_md(menus: &[Menu], vim: bool) -> String {
    let mut md = String::from("# Keyboard\n\nF10 or Alt and a menu's letter opens the menus; each item's keys are beside it. Ctrl+Shift+P finds any command by name.\n");
    if vim {
        md.push_str("\nThe Vim keymap is on: in Normal mode the letters are vim's (Help > Vim Keys); the Ctrl and F-keys below still work.\n");
    }
    for m in menus {
        let rows: Vec<(String, String)> = m.all_items().into_iter().filter(|i| !i.shortcut.is_empty() && !i.label.is_empty()).map(|i| (i.shortcut.clone(), i.label.replace('~', "").trim_end_matches("...").to_string())).collect();
        if rows.is_empty() {
            continue;
        }
        md.push_str(&format!("\n## {}\n\n| Keys | Does |\n|:--|:--|\n", m.plain_title()));
        for (k, what) in rows {
            md.push_str(&format!("| {} | {} |\n", md_key(&k), what));
        }
    }
    md.push_str("\n## Editing\n\n| Keys | Does |\n|:--|:--|\n");
    for (k, what) in [
        ("Shift+arrows", "select"),
        ("Ctrl+arrows", "a word at a time"),
        ("Alt+click", "another cursor"),
        ("Double / triple / quadruple click", "select a word / line / paragraph (drag for more)"),
        ("Ctrl+Shift+\\", "the matching bracket"),
        ("Ins", "overwrite"),
        ("Tab", "accept the grey suggestion"),
        ("Ctrl+Space", "completions"),
        ("Ctrl+.", "quick fix"),
        ("Alt+Z", "word wrap"),
        ("Ctrl+\\", "split the editors"),
        ("Ctrl+= Ctrl+- Ctrl+0", "zoom the text (the app)"),
    ] {
        md.push_str(&format!("| {} | {} |\n", md_key(k), what));
    }
    md.push_str("\n## Windows and parts\n\n| Keys | Does |\n|:--|:--|\n");
    for (k, what) in [
        ("Alt+1 … Alt+9", "go to a window by its number"),
        ("Ctrl+B", "the side bar"),
        ("Ctrl+J", "the panel"),
        ("Ctrl+`", "a terminal"),
        ("Ctrl+L", "the AI chat"),
        ("Ctrl+.  (in the chat)", "Ask or Agent"),
        ("Esc", "back to the editor"),
        ("Drag a title / an edge", "move / resize a window"),
    ] {
        md.push_str(&format!("| {} | {} |\n", md_key(k), what));
    }
    md.push_str("\n## The agent\n\n| Keys | Does |\n|:--|:--|\n| `Space` | pause or go on |\n| `Esc` | stop it |\n");
    md
}

/// The chat's placeholder while the model starts or thinks.
const CHAT_WAIT: &str = "*thinking…*";

/// A chat's line colours: the user's turns (from a "## You" heading to the
/// next heading) as a bubble (4), each answer's heading a bar (3).
fn chat_line_colours(doc: &Document) -> Vec<u8> {
    let mut out = Vec::with_capacity(doc.len_lines());
    let mut mine = false;
    for i in 0..doc.len_lines() {
        let l = doc.line(i);
        if let Some(h) = l.strip_prefix("## ") {
            mine = h.starts_with("You");
            out.push(if mine { 4 } else { 3 });
        } else {
            out.push(if mine { 4 } else { 0 });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn run(wb: &mut Workbench, cmd: Value) -> Value {
        let (tx, rx) = mpsc::channel();
        wb.request(cmd, tx);
        // (By time, not ticks: optimised builds tick through a count before
        // a background job is done.)
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            wb.tick(Instant::now());
            if let Ok(v) = rx.try_recv() {
                return v;
            }
            std::thread::sleep(Duration::from_micros(200));
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
        run(&mut wb, json!({"cmd": "menu", "path": "File/Preferences/Colors..."}));
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
    fn window_scroll_bar_drags_to_the_end() {
        let mut wb = bench();
        let text: String = (0..300).map(|i| format!("line {i}\n")).collect();
        run(&mut wb, json!({"cmd": "type", "text": text}));
        keys(&mut wb, &["Ctrl+Home"]);
        let r = wb.windows.last().unwrap().win.rect;
        let m = |kind, y| Event::Mouse(MouseEvent { kind, pos: Point::new(r.right() - 1, y), mods: Mods::NONE });
        // (The bar scrolls the view; the cursor stays.)
        let line = |wb: &Workbench| wb.windows.last().unwrap().ed.scroll_y;
        // Grab the thumb at the top and drag it past the bottom.
        wb.dispatch(m(MouseKind::Down(Button::Left), r.y + 2));
        assert_eq!(line(&wb), 0);
        wb.dispatch(m(MouseKind::Drag(Button::Left), r.y + r.h / 2));
        let mid = line(&wb);
        assert!(mid > 100 && mid < 200, "{mid}");
        wb.dispatch(m(MouseKind::Drag(Button::Left), r.bottom() + 3));
        wb.dispatch(m(MouseKind::Up(Button::Left), r.bottom() + 3));
        let shown = (wb.windows.last().unwrap().ed.area().h.max(1)) as usize;
        let end = line(&wb);
        assert_eq!(end, 301 - shown);
        assert_eq!(wb.windows.last().unwrap().ed.cursor, 0);
        // The up arrow steps back a line.
        wb.dispatch(m(MouseKind::Down(Button::Left), r.y + 1));
        wb.dispatch(m(MouseKind::Up(Button::Left), r.y + 1));
        assert_eq!(line(&wb), end - 1);
    }

    #[test]
    fn vims_command_line_lists_what_it_could_be() {
        let mut wb = bench();
        wb.set_keymap(Keymap::Vim);
        run(&mut wb, json!({"cmd": "type", "text": ":wri"}));
        let mut surf = Surface::new(100, 30);
        wb.render(&mut surf);
        let t = surf.to_text();
        assert!(t.contains("write"), "{t}");
        keys(&mut wb, &["Esc"]);
    }

    #[test]
    fn vim_undo_stops_at_the_file_as_opened() {
        let (dir, mut wb) = project("vimundo");
        wb.set_keymap(Keymap::Vim);
        wb.goto_file(&dir.join("notes.md"), Some(0), 0);
        run(&mut wb, json!({"cmd": "type", "text": "ix"}));
        keys(&mut wb, &["Esc"]);
        run(&mut wb, json!({"cmd": "type", "text": "uuuuu"}));
        assert_eq!(wb.windows.last().unwrap().ed.doc.rope.to_string(), "# Notes\n");
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
        run(&mut wb, json!({"cmd": "menu", "path": "View/Editor Layout/Tile"}));
        let snap = run(&mut wb, json!({"cmd": "snapshot"}));
        assert!(snap.to_string().contains("vim NORMAL"));
    }

    #[test]
    fn narrow_menu_bar_fits_or_stays_reachable() {
        let mut wb = bench();
        // (VS Code's menus and Borland's together: 84 columns for all.)
        wb.dispatch(Event::Resize(84, 25));
        let rects = wb.menubar.title_rects();
        assert!(rects.iter().all(|r| r.right() <= 84));
        assert!(rects.iter().all(|r| r.w > 0), "all menus fit at 84 columns");
        wb.dispatch(Event::Resize(40, 25));
        keys(&mut wb, &["Alt+H"]);
        assert!(wb.menubar.open.is_some(), "Help opens from the keyboard even when it doesn't fit");
    }

    #[test]
    fn terminal_programs_get_the_mouse_and_kitty_keys() {
        let (dir, mut wb) = project("termio");
        keys(&mut wb, &["Ctrl+`"]);
        let tid = wb.windows.last().unwrap().win.id;
        let screen = |wb: &Workbench| wb.windows.iter().find(|w| w.win.id == tid).map(|w| w.term_text.clone()).unwrap_or_default();
        let pump = |wb: &mut Workbench, what: &str, f: &dyn Fn(&str) -> bool| {
            let t = Instant::now();
            loop {
                wb.tick(Instant::now());
                let s = screen(wb);
                if f(&s) {
                    break;
                }
                assert!(t.elapsed() < Duration::from_secs(8), "{what}: {s}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        pump(&mut wb, "a prompt", &|s| !s.trim().is_empty());
        // A program asking for SGR mouse reports and kitty's keys.
        for c in "printf '\\033[?1000h\\033[?1006h\\033[>1u'; echo READY; cat -v\n".chars() {
            wb.event(Event::Key(KeyEvent::new(if c == '\n' { Key::Enter } else { Key::Char(c) }, Mods::NONE)));
        }
        pump(&mut wb, "ready", &|s| s.lines().any(|l| l.trim() == "READY"));
        let inner = wb.windows.iter().find(|w| w.win.id == tid).unwrap().win.inner();
        let at = Point::new(inner.x + 3, inner.y + 2);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: at, mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: at, mods: Mods::NONE }));
        pump(&mut wb, "the click reported", &|s| s.contains("^[[<0;4;3M"));
        // Kitty: Esc is CSI 27 u (disambiguated).
        wb.event(Event::Key(KeyEvent::new(Key::Esc, Mods::NONE)));
        wb.event(Event::Key(KeyEvent::new(Key::Enter, Mods::NONE)));
        pump(&mut wb, "kitty's Esc", &|s| s.contains("^[[27u"));
        std::fs::remove_dir_all(&dir).unwrap();
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
        assert!(wb.status.info.contains("  Rust  1:1"), "{}", wb.status.info);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn wait_until(wb: &mut Workbench, what: &str, mut f: impl FnMut(&mut Workbench) -> bool) {
        let start = Instant::now();
        while !f(wb) {
            wb.tick(Instant::now());
            assert!(start.elapsed() < Duration::from_secs(15), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A project folder with a few files, and a workbench on it.
    fn project(tag: &str) -> (PathBuf, Workbench) {
        let dir = std::env::temp_dir().join(format!("turbine-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    let needle = 1;\n}\n").unwrap();
        std::fs::write(dir.join("notes.md"), "# Notes\n").unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let mut wb = Workbench::new(&[dir.clone()]);
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(120, 36));
        (dir, wb)
    }

    fn title(wb: &Workbench) -> String {
        wb.windows.iter().rev().find(|w| w.dock.is_none()).map(|w| w.ed.doc.title()).unwrap_or_default()
    }

    #[test]
    fn explorer_opens_files_from_the_keyboard_and_the_agent() {
        let (dir, mut wb) = project("explorer");
        wait_until(&mut wb, "the explorer listing", |wb| wb.shell.explorer.list.rows.len() >= 4);
        keys(&mut wb, &["Ctrl+Shift+E"]);
        assert_eq!(wb.shell.focus, Focus::View(crate::dock::Tab::Explorer));
        // src/ opens, then main.rs (type-ahead, Enter).
        keys(&mut wb, &["s", "Enter"]);
        wait_until(&mut wb, "src listed", |wb| wb.shell.explorer.list.rows.iter().any(|r| r.text == "main.rs"));
        keys(&mut wb, &["Down", "Enter"]);
        assert_eq!(title(&wb), "main.rs");
        assert_eq!(wb.shell.focus, Focus::Editors);
        // The agent clicks notes.md in the tree (a double-click opens).
        let r = run(&mut wb, json!({"cmd": "click", "target": "listitem:notes.md"}));
        assert_eq!(r["ok"], true, "{r}");
        run(&mut wb, json!({"cmd": "click", "target": "listitem:notes.md"}));
        assert_eq!(title(&wb), "notes.md");
        // Two editors: two tabs; clicking the first brings it back.
        run(&mut wb, json!({"cmd": "click", "target": "tab:main.rs"}));
        assert_eq!(title(&wb), "main.rs");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_l_opens_the_chat_in_vim_mode_too() {
        let (dir, mut wb) = project("vimchat");
        wb.set_keymap(Keymap::Vim);
        wb.goto_file(&dir.join("src/main.rs"), Some(0), 0);
        keys(&mut wb, &["Ctrl+L"]);
        assert!(wb.shell.dock.right.visible, "the chat side bar shows");
        assert_eq!(wb.shell.focus, Focus::Chat);
        run(&mut wb, json!({"cmd": "type", "text": "hi"}));
        assert_eq!(wb.shell.chat_input.text, "hi");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_chat_side_bar_shares_the_editors_border() {
        let (dir, mut wb) = project("oneborder");
        std::fs::write(dir.join("long.txt"), "x\n".repeat(300)).unwrap();
        wb.goto_file(&dir.join("long.txt"), Some(0), 0);
        wb.run_command("view.secondary");
        wb.focus_editor();
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let right = wb.layout().right.unwrap();
        let ed = wb.windows.iter().rev().find(|w| w.dock.is_none()).unwrap().win.rect;
        assert_eq!(ed.right() - 1, right.x, "one column between them");
        let y = ed.y + ed.h / 2;
        let col = |x: i32| surf.get(x, y).unwrap().text.clone();
        // (The shared edge is the side bar's; the editor's bar a column in;
        // no second line anywhere beside them.)
        assert!(["│", "║"].contains(&col(right.x).as_str()), "{:?}", col(right.x));
        assert!(["▒", "░", "█", "▓"].contains(&col(right.x - 1).as_str()), "{:?}", col(right.x - 1));
        assert!(!["│", "║"].contains(&col(right.x + 1).as_str()) && !["│", "║"].contains(&col(right.x - 2).as_str()));
        // The edge drags to resize the side bar, beside the editor's rows too.
        let w0 = right.w;
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(right.x, y), mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Drag(Button::Left), pos: Point::new(right.x - 6, y), mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: Point::new(right.x - 6, y), mods: Mods::NONE }));
        assert_eq!(wb.layout().right.unwrap().w, w0 + 6);
        // And the editor's bar still scrolls it: a click low on its track.
        wb.render(&mut surf);
        let ed = wb.windows.iter().rev().find(|w| w.dock.is_none()).unwrap().win.rect;
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(ed.right() - 2, ed.bottom() - 4), mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: Point::new(ed.right() - 2, ed.bottom() - 4), mods: Mods::NONE }));
        assert!(wb.windows.iter().rev().find(|w| w.dock.is_none()).unwrap().ed.scroll_y > 100);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_chat_tells_you_from_the_ai() {
        let doc = Document::from_str("# AI Chat\n\n## You\n\nwhy?\n\n## inkling-2-mini\n\nbecause\n");
        assert_eq!(chat_line_colours(&doc), vec![0, 0, 4, 4, 4, 4, 3, 0, 0, 0]);
    }

    #[test]
    fn the_chat_picks_its_model_and_how_hard_it_thinks() {
        let (dir, mut wb) = project("pickers");
        let m = |name: &str, label: &str, cloud: bool| ai::Model { provider: "p".into(), name: name.into(), label: label.into(), cloud, chat: true, base: "http://127.0.0.1:9/v1".into(), ..Default::default() };
        wb.set_ai(Ai::with_models(vec![m("service:x/inkling-2-mini", "Inkling 2 Mini", false), m("claude/sonnet", "Claude Code sonnet", true)]));
        wb.run_command("view.secondary");
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let r = wb.layout().right.unwrap();
        let pickers = wb.chat_pickers(r);
        assert!(pickers[0].2.contains("Inkling 2 Mini") && pickers[1].2.contains("Default"), "{pickers:?}");
        // The model's list: a click, the cloud one marked, picked.
        let at = Point::new(pickers[0].0.x + 1, pickers[0].0.y);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: at, mods: Mods::NONE }));
        let (popup, _, acts) = wb.status_menu.take().expect("the model list");
        assert_eq!(popup.items[1].0, "☁ Claude Code sonnet");
        wb.status_choice(&acts[1]);
        assert_eq!(wb.chat_model().unwrap().name, "claude/sonnet");
        // Thinking: High goes with every request.
        let at = Point::new(wb.chat_pickers(r)[1].0.x + 1, r.bottom() - 3);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: at, mods: Mods::NONE }));
        let (_, _, acts) = wb.status_menu.take().expect("the thinking list");
        wb.status_choice(acts.iter().find(|a| a.ends_with(":high")).unwrap());
        assert_eq!(wb.ai.as_ref().unwrap().thinking.as_deref(), Some("high"));
        assert!(wb.chat_pickers(r)[1].2.contains("High"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn maximize_all_undoes_tile() {
        let (dir, mut wb) = project("maxall");
        wb.goto_file(&dir.join("src/main.rs"), Some(0), 0);
        wb.goto_file(&dir.join("notes.md"), Some(0), 0);
        wb.run_command("window.tile");
        assert!(wb.windows.iter().filter(|w| w.dock.is_none()).all(|w| !w.win.zoomed));
        wb.run_command("window.maximize_all");
        let ed: Vec<_> = wb.windows.iter().filter(|w| w.dock.is_none()).collect();
        assert!(ed.len() >= 2 && ed.iter().all(|w| w.win.zoomed && w.win.rect == ed[0].win.rect));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn topic_search_finds_the_word_in_the_guides() {
        let guide = "# Rust\n\n## Loops\n\nUse `loop` for ever.\n\nA for loop: for x in v.\n\n## Matching\n\nmatch on enums.\n\n```rust\nmatch x { _ => {} }\n```\n";
        let md = topic_page("Topic: match", "match", &[("Rust".into(), guide.into()), ("Turbine".into(), "# Turbine\n\nNothing here.\n".into())]);
        assert!(md.contains("## In Rust") && md.contains("### Matching") && md.contains("match x { _ => {} }"), "{md}");
        assert!(!md.contains("Loops") && !md.contains("In Turbine"), "whole words, only where it is: {md}");
        let none = topic_page("Topic: zzz", "zzz", &[("Rust".into(), guide.into())]);
        assert!(none.contains("Nothing about `zzz` in Rust"));
        // With no word at the cursor: it asks.
        let mut wb = bench();
        wb.run_command("help.topic");
        assert_eq!(wb.dialog.as_ref().map(|d| d.kind.as_str()), Some("topic"));
    }

    #[test]
    fn the_secondary_side_bar_button_opens_the_chat() {
        let (dir, mut wb) = project("rightbar");
        assert!(wb.shell.dock.right.tabs.is_empty());
        wb.run_command("view.secondary");
        assert!(wb.shell.dock.right.visible && !wb.shell.dock.right.tabs.is_empty());
        assert_eq!(wb.shell.focus, Focus::Chat);
        wb.run_command("view.secondary");
        assert!(!wb.shell.dock.right.visible);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn panel_terminal_and_chat_dock_and_toggle() {
        let (dir, mut wb) = project("panel");
        assert!(!wb.shell.dock.bottom.visible);
        keys(&mut wb, &["Ctrl+`"]);
        assert!(wb.shell.dock.bottom.visible);
        let t = wb.windows.last().unwrap();
        assert!(t.term.is_some() && t.dock == Some(Side::Bottom), "a docked terminal in front");
        let panel = wb.layout().bottom.unwrap();
        assert!(panel.contains_rect(wb.windows.last().unwrap().win.rect), "the terminal fills the panel");
        // Ctrl+J hides the panel; the editor gets the keys back.
        keys(&mut wb, &["Ctrl+J"]);
        assert!(!wb.shell.dock.bottom.visible);
        // Ctrl+L: the chat at the right, typing into its field.
        keys(&mut wb, &["Ctrl+L"]);
        assert!(wb.shell.dock.right.visible);
        assert_eq!(wb.shell.focus, Focus::Chat);
        run(&mut wb, json!({"cmd": "type", "text": "hi"}));
        assert_eq!(wb.shell.chat_input.text, "hi");
        let tree = wb.tree();
        assert_eq!(tree.find_target("chat.input").and_then(|n| n.value.clone()).as_deref(), Some("hi"));
        keys(&mut wb, &["Esc"]);
        assert_eq!(wb.shell.focus, Focus::Editors);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn problems_list_diagnostics_and_jump_to_them() {
        let (dir, mut wb) = project("problems");
        wb.open_path(&dir.join("src/main.rs"));
        wb.windows.last_mut().unwrap().ed.diagnostics =
            vec![DiagMark { line: 1, start: 8, end: 14, severity: 1, message: "unused variable".into() }];
        keys(&mut wb, &["Ctrl+Shift+M"]);
        assert!(wb.shell.dock.bottom.visible);
        assert_eq!(wb.shell.problem_count, (1, 0));
        let rows: Vec<String> = wb.shell.problems.rows.iter().map(|r| r.text.clone()).collect();
        assert_eq!(rows, ["main.rs", "unused variable"]);
        keys(&mut wb, &["Down", "Enter"]);
        assert_eq!(wb.shell.focus, Focus::Editors);
        assert_eq!(wb.windows.last().unwrap().ed.cursor_line_col(), (1, 8));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_finds_and_opens_a_line() {
        let (dir, mut wb) = project("search");
        keys(&mut wb, &["Ctrl+Shift+F"]);
        run(&mut wb, json!({"cmd": "type", "text": "needle"}));
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "search results", |wb| !wb.shell.search.busy() && wb.shell.search.hits.len() == 2);
        keys(&mut wb, &["Down", "Down", "Enter"]);
        assert_eq!(title(&wb), "main.rs");
        assert_eq!(wb.windows.last().unwrap().ed.cursor_line_col().0, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn palette_runs_commands_opens_files_and_goes_to_lines() {
        let (dir, mut wb) = project("palette");
        // Ctrl+Shift+P, "panel", Enter: the panel shows.
        keys(&mut wb, &["Ctrl+Shift+P"]);
        assert!(wb.shell.palette.is_some());
        run(&mut wb, json!({"cmd": "type", "text": "toggle pa"}));
        let first = wb.shell.palette.as_ref().unwrap().list.rows[0].text.clone();
        assert_eq!(first, "View: Toggle Panel", "{first}");
        keys(&mut wb, &["Enter"]);
        assert!(wb.shell.palette.is_none() && wb.shell.dock.bottom.visible);
        // Ctrl+P, a file's name: it opens; ":2" goes to its second line.
        keys(&mut wb, &["Ctrl+P"]);
        wait_until(&mut wb, "the file list", |wb| wb.shell.palette.as_ref().is_some_and(|p| p.list.rows.iter().any(|r| r.text == "main.rs")));
        run(&mut wb, json!({"cmd": "type", "text": "mainrs"}));
        keys(&mut wb, &["Enter"]);
        assert_eq!(title(&wb), "main.rs");
        keys(&mut wb, &["Ctrl+P"]);
        run(&mut wb, json!({"cmd": "type", "text": ":2"}));
        keys(&mut wb, &["Enter"]);
        assert_eq!(wb.windows.last().unwrap().ed.cursor_line_col().0, 1);
        // Ctrl+Shift+O: the file's symbols.
        keys(&mut wb, &["Ctrl+Shift+O"]);
        wait_until(&mut wb, "symbols", |wb| wb.shell.palette.as_ref().is_some_and(|p| p.list.rows.iter().any(|r| r.text.ends_with("main"))));
        keys(&mut wb, &["Esc"]);
        assert!(wb.shell.palette.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_and_replace_in_the_editor() {
        let mut wb = bench();
        run(&mut wb, json!({"cmd": "type", "text": "cat dog Cat\ncatalog cat"}));
        keys(&mut wb, &["Ctrl+Home", "Ctrl+F"]);
        assert_eq!(wb.shell.focus, Focus::Find);
        run(&mut wb, json!({"cmd": "type", "text": "cat"}));
        let ed = |wb: &Workbench| wb.windows.iter().rev().find(|w| w.dock.is_none()).unwrap().ed.found.clone();
        assert_eq!(ed(&wb).len(), 4, "case-insensitive by default");
        // Whole words only (Alt+W): "catalog" drops out.
        keys(&mut wb, &["Alt+W"]);
        assert_eq!(ed(&wb).len(), 3);
        // Match case (Alt+C): "Cat" drops out; Enter steps through the rest.
        keys(&mut wb, &["Alt+C"]);
        assert_eq!(ed(&wb), vec![(0, 3), (20, 23)]);
        keys(&mut wb, &["Enter"]);
        let w = wb.windows.iter().rev().find(|w| w.dock.is_none()).unwrap();
        assert_eq!((w.ed.found_current, w.ed.selected_text().as_deref()), (Some(1), Some("cat")));
        // Replace: one, then all.
        // (Ctrl+H with something to find: the cursor goes to Replace.)
        keys(&mut wb, &["Esc", "Ctrl+H"]);
        assert!(wb.find.in_replace);
        run(&mut wb, json!({"cmd": "type", "text": "fox"}));
        keys(&mut wb, &["Enter"]);
        let text = |wb: &mut Workbench| run(wb, json!({"cmd": "text"}))["text"].as_str().unwrap().to_string();
        assert_eq!(text(&mut wb).matches("fox").count(), 1);
        keys(&mut wb, &["Ctrl+Alt+Enter"]);
        assert_eq!(text(&mut wb), "fox dog Cat\ncatalog fox");
        // Undo puts back what Replace All changed in one step.
        keys(&mut wb, &["Esc", "Ctrl+Z"]);
        assert_eq!(text(&mut wb).matches("fox").count(), 1);
        // A bad regex says so instead of matching.
        keys(&mut wb, &["Ctrl+F", "Alt+R"]);
        keys(&mut wb, &["End"]);
        run(&mut wb, json!({"cmd": "type", "text": "("}));
        assert!(wb.find.error.is_some());
    }

    #[test]
    fn the_panel_has_vs_codes_tabs_and_resizes() {
        let (dir, mut wb) = project("paneltabs");
        keys(&mut wb, &["Ctrl+J"]);
        assert!(wb.shell.dock.bottom.visible);
        let names = |wb: &Workbench| wb.tree().find_target("part.bottom").map(|p| p.children.iter().filter(|c| c.role == "tab").map(|c| c.name.split_once(' ').map_or(c.name.clone(), |(_, n)| n.to_string())).collect::<Vec<_>>()).unwrap_or_default();
        let plain = |v: Vec<String>| v.into_iter().map(|n| n.trim_start_matches(|c: char| !c.is_ascii_alphabetic()).to_string()).collect::<Vec<_>>();
        assert_eq!(plain(names(&wb)), ["Problems", "Output", "Debug Console", "Terminal"]);
        // Terminal: one starts in its place; + starts a second beside it.
        run(&mut wb, json!({"cmd": "click", "target": "tab.terminal"}));
        assert!(wb.windows.last().unwrap().term.is_some());
        let r = run(&mut wb, json!({"cmd": "click", "target": "panel.new_terminal"}));
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(plain(names(&wb)), ["Problems", "Output", "Debug Console", "Terminal", "Terminal 2"]);
        // Maximize, then restore.
        let h = wb.layout().bottom.unwrap().h;
        run(&mut wb, json!({"cmd": "click", "target": "panel.maximize"}));
        // (Over the whole of the editors' place.)
        let l = wb.layout();
        assert!(l.bottom.unwrap().y <= l.editors.y && l.bottom.unwrap().h > h, "{l:?}");
        run(&mut wb, json!({"cmd": "click", "target": "panel.maximize"}));
        assert_eq!(wb.layout().bottom.unwrap().h, h);
        // The top edge (not on a tab) drags it taller.
        let b = wb.layout().bottom.unwrap();
        let x = b.x + 1;
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(x, b.y), mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Drag(Button::Left), pos: Point::new(x, b.y - 4), mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: Point::new(x, b.y - 4), mods: Mods::NONE }));
        assert_eq!(wb.layout().bottom.unwrap().h, b.h + 4);
        // Close.
        run(&mut wb, json!({"cmd": "click", "target": "panel.close"}));
        assert!(!wb.shell.dock.bottom.visible);
        // The menu bar's layout toggles show and hide the parts.
        let left = wb.shell.dock.left.visible;
        let r = run(&mut wb, json!({"cmd": "click", "target": "layout.view.sidebar"}));
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(wb.shell.dock.left.visible, !left);
        run(&mut wb, json!({"cmd": "click", "target": "layout.view.panel"}));
        assert!(wb.shell.dock.bottom.visible);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn markdown_and_data_files_have_a_toolbar() {
        let (dir, mut wb) = project("docview");
        std::fs::write(dir.join("data.json"), "{\n  \"name\": \"turbine\",\n  \"deps\": {\n    \"needle\": 3\n  }\n}\n").unwrap();
        wb.goto_file(&dir.join("data.json"), Some(0), 0);
        wait_until(&mut wb, "the language", |wb| wb.windows.last().unwrap().lang.as_deref() == Some("json"));
        // Tree: folding and a search; Enter on a value goes to its line.
        let r = run(&mut wb, json!({"cmd": "click", "target": "docview.tree"}));
        assert_eq!(r["ok"], true, "{r}");
        wait_until(&mut wb, "the tree", |wb| wb.tree().find_target("docview.treeview").is_some());
        keys(&mut wb, &["/"]);
        run(&mut wb, json!({"cmd": "type", "text": "needle"}));
        let v = wb.tree().find_target("docview.treeview").and_then(|n| n.value.clone()).unwrap_or_default();
        assert!(v.contains("1 found"), "{v}");
        keys(&mut wb, &["Enter", "Enter"]);
        assert_eq!(wb.doc_mode(wb.windows.last().unwrap().win.id), docview::Mode::Source);
        assert_eq!(wb.windows.last().unwrap().ed.cursor_line_col().0, 3);
        // Word wrap from the toolbar (and Alt+Z).
        let before = wb.windows.last().unwrap().ed.wrap;
        run(&mut wb, json!({"cmd": "click", "target": "docview.wrap"}));
        assert_eq!(wb.windows.last().unwrap().ed.wrap, !before);
        keys(&mut wb, &["Alt+Z"]);
        assert_eq!(wb.windows.last().unwrap().ed.wrap, before);
        // Markdown: Preview in the window itself.
        wb.goto_file(&dir.join("notes.md"), Some(0), 0);
        wait_until(&mut wb, "markdown", |wb| wb.windows.last().unwrap().lang.as_deref() == Some("markdown"));
        run(&mut wb, json!({"cmd": "click", "target": "docview.preview"}));
        let id = wb.windows.last().unwrap().win.id;
        wait_until(&mut wb, "the rendered page", |wb| wb.docviews.get(&id).is_some_and(|d| d.alt.doc.rope.to_string().contains("Notes") && !d.alt.doc.rope.to_string().contains('#')));
        // Word Wrap off: the page's lines run on (rendered wide).
        let wrapped_lines = wb.docviews[&id].alt.doc.len_lines();
        let r = run(&mut wb, json!({"cmd": "click", "target": "docview.wrap"}));
        assert!(!wb.docviews[&id].preview_wrap, "{r}");
        let _ = wrapped_lines;
        // Typing doesn't change the file; Esc is back to Source.
        keys(&mut wb, &["x", "Esc"]);
        assert_eq!(wb.windows.last().unwrap().ed.doc.rope.to_string(), "# Notes\n");
        assert_eq!(wb.doc_mode(id), docview::Mode::Source);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn outline_and_timeline_sit_under_the_explorer() {
        let (dir, mut wb) = project("sections");
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git").arg("-C").arg(&dir).args(args).env("GIT_AUTHOR_NAME", "T").env("GIT_AUTHOR_EMAIL", "t@x").env("GIT_COMMITTER_NAME", "T").env("GIT_COMMITTER_EMAIL", "t@x").output().unwrap().status.success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "First commit"]);
        wb.goto_file(&dir.join("src/main.rs"), Some(0), 0);
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        // Outline open below the Explorer: main's symbol listed.
        wait_until(&mut wb, "the outline", |wb| wb.shell.outline.list.rows.iter().any(|r| r.text.contains("main")));
        let l = wb.layout();
        let (main, secs) = wb.shell.dock.sections_in(Side::Left, l.left.unwrap()).unwrap();
        assert_eq!(secs.len(), 2);
        assert!(secs[0].2.is_some() && secs[1].2.is_none(), "Outline open, Timeline folded");
        assert!(main.h >= 4 && wb.shell.outline.list.area == secs[0].2.unwrap());
        // A click on an Outline row: the cursor (and view) at it.
        wb.goto_file(&dir.join("src/main.rs"), Some(2), 0);
        let ob = secs[0].2.unwrap();
        let row = wb.shell.outline.list.rows.iter().position(|r| r.text.contains("main")).unwrap();
        let at = Point::new(ob.x + 3, ob.y + (row - wb.shell.outline.list.scroll) as i32);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: at, mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: at, mods: Mods::NONE }));
        let ed = &wb.windows.iter().rev().find(|w| w.ed.doc.path.as_deref() == Some(dir.join("src/main.rs").as_path())).unwrap().ed;
        assert_eq!(ed.cursor_line_col().0, wb.shell.outline.lines[row], "the cursor at the item");
        // Timeline: a click on its title opens it; it lists the commit.
        let th = secs[1].1;
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(th.x + 3, th.y), mods: Mods::NONE }));
        wait_until(&mut wb, "the timeline", |wb| wb.shell.timeline.list.rows.iter().any(|r| r.text == "First commit"));
        // Dragging the Outline's header resizes it.
        let (_, secs) = wb.shell.dock.sections_in(Side::Left, wb.layout().left.unwrap()).unwrap();
        let (h, oh) = (secs[0].1, secs[0].2.unwrap().h);
        let at = Point::new(h.right() - 3, h.y);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: at, mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Drag(Button::Left), pos: Point::new(at.x, at.y - 2), mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: Point::new(at.x, at.y - 2), mods: Mods::NONE }));
        let (_, secs) = wb.shell.dock.sections_in(Side::Left, wb.layout().left.unwrap()).unwrap();
        assert_eq!(secs[0].2.unwrap().h, oh + 2);
        // And down, with the Outline's own list under the pointer (it has the keys).
        wb.shell.focus = shell::Focus::View(crate::dock::Tab::Outline);
        let h = secs[0].1;
        let at = Point::new(h.right() - 3, h.y);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: at, mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Drag(Button::Left), pos: Point::new(at.x, at.y + 3), mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: Point::new(at.x, at.y + 3), mods: Mods::NONE }));
        let (_, secs) = wb.shell.dock.sections_in(Side::Left, wb.layout().left.unwrap()).unwrap();
        assert_eq!(secs[0].2.unwrap().h, oh - 1);
        // Enter on the commit: its change as a diff page.
        let (_, secs) = wb.shell.dock.sections_in(Side::Left, wb.layout().left.unwrap()).unwrap();
        let b = secs[1].2.unwrap();
        for _ in 0..2 {
            wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(b.x + 3, b.y), mods: Mods::NONE }));
            wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: Point::new(b.x + 3, b.y), mods: Mods::NONE }));
        }
        wait_until(&mut wb, "the diff", |wb| wb.windows.last().is_some_and(|w| w.win.title.contains("main.rs @")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scroll_bars_scroll_the_view_not_the_cursor() {
        let (dir, mut wb) = project("bars");
        let text: String = (0..300).map(|i| format!("line {i} {}\n", "x".repeat(i % 7 * 40))).collect();
        std::fs::write(dir.join("long.txt"), text).unwrap();
        wb.goto_file(&dir.join("long.txt"), Some(0), 0);
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let r = wb.windows.last().unwrap().win.rect;
        let press = |wb: &mut Workbench, kind: MouseKind, p: Point| wb.event(Event::Mouse(MouseEvent { kind, pos: p, mods: Mods::NONE }));
        // A press halfway down the track: there at once; the cursor stays.
        let mid = Point::new(r.right() - 1, r.y + r.h / 2);
        press(&mut wb, MouseKind::Down(Button::Left), mid);
        let y1 = wb.windows.last().unwrap().ed.scroll_y;
        assert!(y1 > 100 && y1 < 200, "{y1}");
        assert_eq!(wb.windows.last().unwrap().ed.cursor, 0);
        // Dragging on from there.
        press(&mut wb, MouseKind::Drag(Button::Left), Point::new(mid.x, mid.y + 5));
        press(&mut wb, MouseKind::Up(Button::Left), Point::new(mid.x, mid.y + 5));
        let y2 = wb.windows.last().unwrap().ed.scroll_y;
        assert!(y2 > y1, "{y1} {y2}");
        // Drawing keeps the view where the bar put it.
        wb.render(&mut surf);
        assert_eq!(wb.windows.last().unwrap().ed.scroll_y, y2);
        // Sideways: the bottom edge's track.
        let skip = wb.hbar_skip();
        let at = Point::new(r.x + skip + 1 + (r.w - skip - 4) / 2, r.bottom() - 1);
        press(&mut wb, MouseKind::Down(Button::Left), at);
        press(&mut wb, MouseKind::Up(Button::Left), at);
        assert!(wb.windows.last().unwrap().ed.scroll_x > 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn markdown_outline_jumps() {
        let (dir, mut wb) = project("mdoutline");
        std::fs::write(dir.join("doc.md"), "# One\n\ntext\n\n## Two\n\nmore\n\n## Three\n\nend\n").unwrap();
        wb.goto_file(&dir.join("doc.md"), Some(0), 0);
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        wait_until(&mut wb, "the outline", |wb| wb.shell.outline.list.rows.iter().any(|r| r.text.contains("Three")));
        let rows: Vec<String> = wb.shell.outline.list.rows.iter().map(|r| r.text.clone()).collect();
        let i = rows.iter().position(|r| r.contains("Three")).unwrap();
        wb.outline_goto_pub(i, true);
        assert_eq!(wb.windows.last().unwrap().ed.cursor_line_col().0, 8, "{rows:?} {:?}", wb.shell.outline.lines);
        // In Preview: the page scrolls to the heading.
        let id = wb.windows.last().unwrap().win.id;
        wb.set_doc_mode(id, docview::Mode::Preview);
        wait_until(&mut wb, "the page", |wb| wb.docviews[&id].alt.doc.rope.to_string().contains("Three"));
        wb.outline_goto_pub(i, true);
        let alt = &wb.docviews[&id].alt;
        assert!(alt.doc.line(alt.scroll_y).contains("Three"), "{}", alt.doc.line(alt.scroll_y));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_data_tree_edits_values_and_has_a_menu() {
        let (dir, mut wb) = project("treeedit");
        std::fs::write(dir.join("d.json"), "{\n  \"name\": \"old\",\n  \"n\": [1, 2]\n}\n").unwrap();
        wb.goto_file(&dir.join("d.json"), Some(0), 0);
        wait_until(&mut wb, "json", |wb| wb.windows.last().unwrap().lang.as_deref() == Some("json"));
        let id = wb.windows.last().unwrap().win.id;
        wb.set_doc_mode(id, docview::Mode::Tree);
        wait_until(&mut wb, "the tree", |wb| wb.docviews[&id].tree.as_ref().is_some_and(|t| t.rows() >= 2));
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        // Expand All and Collapse All: one toggle.
        assert!(wb.tree().find_target("docview.expand").is_some() && wb.tree().find_target("docview.collapse").is_none());
        run(&mut wb, json!({"cmd": "click", "target": "docview.expand"}));
        assert!(wb.tree().find_target("docview.collapse").is_some() && wb.tree().find_target("docview.expand").is_none());
        run(&mut wb, json!({"cmd": "click", "target": "docview.collapse"}));
        assert!(wb.tree().find_target("docview.expand").is_some());
        wb.render(&mut surf);
        // Right-click "name": the menu; Copy Path.
        let a = wb.docviews[&id].tree.as_ref().unwrap().area;
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Right), pos: Point::new(a.x + 4, a.y + 1), mods: Mods::NONE }));
        assert!(wb.docviews[&id].menu.is_some());
        let r = run(&mut wb, json!({"cmd": "click", "target": "menuitem:Copy Path"}));
        if r["ok"] != true {
            // (The menu's items aren't agent nodes: the action itself.)
            wb.tree_action(id, "copy_path");
        }
        assert_eq!(wb.clipboard, ".name");
        // F2: edit its value; the file's text changes (undoably).
        wb.docviews.get_mut(&id).unwrap().menu = None;
        keys(&mut wb, &["F2"]);
        assert_eq!(wb.dialog.as_ref().map(|d| d.title.as_str()), Some("Edit Value"));
        assert_eq!(wb.dialog.as_ref().and_then(|d| d.value("value")).as_deref(), Some("\"old\""));
        // (The old value is selected: typing replaces it.)
        run(&mut wb, json!({"cmd": "type", "text": "\"new\""}));
        keys(&mut wb, &["Enter"]);
        let text = wb.windows.iter().find(|w| w.win.id == id).unwrap().ed.doc.rope.to_string();
        assert!(text.contains("\"name\": \"new\""), "{text}");
        wait_until(&mut wb, "the tree again", |wb| wb.docviews[&id].tree.as_ref().and_then(|t| t.node(&[0]).and_then(|n| n.value.clone())).as_deref() == Some("new"));
        let text_of = |wb: &Workbench| wb.windows.iter().find(|w| w.win.id == id).unwrap().ed.doc.rope.to_string();
        // Ins on "name": an entry after it (key, then a value; null by default).
        keys(&mut wb, &["Insert"]);
        assert_eq!(wb.dialog.as_ref().map(|d| d.title.as_str()), Some("Add After"));
        run(&mut wb, json!({"cmd": "type", "text": "k"}));
        keys(&mut wb, &["Enter"]);
        assert!(text_of(&wb).contains("\"name\": \"new\",\n  \"k\": null,\n  \"n\""), "{}", text_of(&wb));
        // Del on it: gone again, commas and all.
        wait_until(&mut wb, "k listed", |wb| wb.docviews[&id].tree.as_ref().is_some_and(|t| t.node(&[1]).and_then(|n| n.key.clone()).as_deref() == Some("k")));
        keys(&mut wb, &["Down", "Delete"]);
        assert_eq!(text_of(&wb), "{\n  \"name\": \"new\",\n  \"n\": [1, 2]\n}\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_editor_has_a_right_click_menu() {
        let (dir, mut wb) = project("edmenu");
        wb.goto_file(&dir.join("notes.md"), Some(0), 0);
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let a = wb.windows.last().unwrap().ed.area();
        let at = Point::new(a.x + a.w / 2, a.y);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Right), pos: at, mods: Mods::NONE }));
        assert!(wb.ed_menu.is_some());
        wb.render(&mut surf);
        let i = wb.ed_menu.as_ref().unwrap().2.iter().position(|c| *c == "edit.select_all").unwrap();
        let r = wb.ed_menu.as_ref().unwrap().0.item_rect(i).unwrap();
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(r.x + 1, r.y), mods: Mods::NONE }));
        assert!(wb.ed_menu.is_none());
        assert_eq!(wb.windows.last().unwrap().ed.selected_text().as_deref(), Some("# Notes\n"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn its_own_title_bar_is_the_menu_row_as_vs_codes() {
        let mut wb = bench();
        wb.event(Event::Resize(120, 30));
        wb.setting_set("title_bar", "on");
        // (One row: the menus, ← → and the command center, the toggles, the
        // window's buttons; the client area is the whole window.)
        assert_eq!(wb.client_area(), Rect::new(0, 0, 120, 30));
        assert_eq!(wb.size, (120, 30));
        assert!(wb.own_title_bar());
        let items = wb.top_items();
        let find = |a: &str| items.iter().find(|t| t.action == a).map(|t| t.rect).unwrap_or_else(|| panic!("{a}: {items:?}"));
        let (center, back, sidebar, close) = (find("center"), find("go.back"), find("view.sidebar"), find("win:3"));
        let titles = wb.menubar.title_rects();
        let last_menu = titles.iter().rev().find(|r| r.w > 0 && r.right() < center.x).unwrap();
        assert!(last_menu.right() < back.x && back.right() <= center.x && center.right() < sidebar.x && sidebar.right() <= close.x, "{items:?}");
        assert_eq!(close.right(), 120);
        let at = |r: Rect| Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(r.x + 1, 0), mods: Mods::NONE });
        wb.event(at(find("win:1")));
        assert_eq!(wb.take_window_op(), 1);
        wb.event(at(close));
        assert_eq!(wb.take_window_op(), 3);
        // The free part of the row: a move.
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(back.x - 1, 0), mods: Mods::NONE }));
        assert_eq!(wb.take_window_op(), 4);
        // The command center: Go to File.
        wb.event(at(center));
        assert!(wb.shell.palette.is_some());
        keys(&mut wb, &["Esc"]);
        let mut surf = Surface::new(120, 30);
        wb.render(&mut surf);
        let t = surf.to_text();
        let row = t.lines().next().unwrap();
        assert!(row.contains("File  Edit") && row.contains(" ← ") && row.contains("[▌]") && row.trim_end().ends_with("[▼][▲][■]"), "{row}");
        // A click on a menu's title still opens it.
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(3, 0), mods: Mods::NONE }));
        assert!(wb.menubar.is_open());
    }

    #[test]
    fn tooltips_render_markdown_in_colour() {
        let wb = bench();
        let rich = wb.markdown_rich("```rust\nfn area(w: u32) -> u32\n```\n---\nThe **area** of `w`.", 70);
        let text: Vec<String> = rich.iter().map(|l| l.iter().map(|(t, _)| t.as_str()).collect()).collect();
        assert!(text.iter().any(|l| l.contains("fn area")) && !text.iter().any(|l| l.contains("```")), "{text:?}");
        // The code's type in a colour of its own.
        let code = rich.iter().find(|l| l.iter().any(|(t, _)| t.contains("fn"))).unwrap();
        let ty = code.iter().find(|(t, _)| t == "u32").unwrap().1;
        assert_ne!(ty.fg, wb.theme.scheme.tooltip.fg, "{code:?}");
        // Inline code in the file's language: its type and its punctuation apart.
        let rich = wb.markdown_rich_in("Type: `const char *const[45]`", 70, Some("c"));
        let line = rich.iter().find(|l| l.iter().any(|(t, _)| t.contains("char"))).unwrap();
        let styles: std::collections::HashSet<_> = line.iter().filter(|(t, _)| !t.trim().is_empty() && !t.contains("Type")).map(|(_, s)| (s.fg.0, s.fg.1, s.fg.2)).collect();
        assert!(styles.len() >= 2, "{line:?}");
    }

    #[test]
    fn the_status_line_has_vs_codes_items() {
        let (dir, mut wb) = project("statusline");
        wb.goto_file(&dir.join("src/main.rs"), Some(1), 4);
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let t = surf.to_text();
        let last = t.lines().last().unwrap().to_string();
        assert!(last.contains("Ln 2, Col 5") && last.contains("UTF-8"), "{last}");
        wait_until(&mut wb, "the language", |wb| wb.windows.last().unwrap().lang.as_deref() == Some("rust"));
        wb.render(&mut surf);
        assert!(surf.to_text().lines().last().unwrap().contains("Rust"));
        // The line ends: a click opens a list above it; CRLF picked.
        let r = run(&mut wb, json!({"cmd": "click", "target": "status.file.line_ending"}));
        assert_eq!(r["ok"], true, "{r}");
        let (p, _, acts) = wb.status_menu.as_ref().expect("the drop-up");
        assert!(p.rect().bottom() <= 35 && acts.len() == 2);
        keys(&mut wb, &["Down", "Enter"]);
        assert_eq!(wb.windows.last().unwrap().ed.doc.line_ending, turbine_text::encoding::LineEnding::CrLf);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn terminals_have_a_right_click_menu() {
        let (dir, mut wb) = project("termmenu");
        keys(&mut wb, &["Ctrl+`"]);
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let inner = wb.windows.last().unwrap().win.inner();
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Right), pos: Point::new(inner.x + 5, inner.y + 2), mods: Mods::NONE }));
        let cmds = wb.ed_menu.as_ref().map(|m| m.2.clone()).unwrap_or_default();
        assert!(cmds.contains(&"terminal.paste") && cmds.contains(&"terminal.kill"), "{cmds:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_workspace_holds_several_folders() {
        let (dir, mut wb) = project("workspace");
        let other = std::env::temp_dir().join(format!("turbine-workspace2-{}", std::process::id()));
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("second.txt"), "hi").unwrap();
        wait_until(&mut wb, "the listing", |wb| wb.shell.explorer.list.rows.len() >= 4);
        // File > Add Folder to Workspace: the folder, typed, Select.
        wb.run_command("file.add_folder");
        assert_eq!(wb.dialog.as_ref().map(|d| d.title.as_str()), Some("Add Folder to Workspace"));
        run(&mut wb, json!({"cmd": "type", "text": other.display().to_string()}));
        keys(&mut wb, &["Enter"]);
        assert_eq!(wb.shell.explorer.extra, vec![other.clone()]);
        // Both trees in the Explorer, each under its name.
        let name = other.file_name().unwrap().to_string_lossy().into_owned();
        wait_until(&mut wb, "both trees", |wb| wb.shell.explorer.list.rows.iter().any(|r| r.text == "second.txt") && wb.shell.explorer.list.rows.iter().any(|r| r.text == "notes.md"));
        assert!(wb.shell.explorer.list.rows.iter().any(|r| r.heading && r.text == name));
        // Each folder folds to its heading (▾ ▸), as VS Code's do.
        let i = wb.shell.explorer.list.rows.iter().position(|r| r.heading && r.text == name).unwrap();
        assert_eq!(wb.shell.explorer.list.rows[i].icon, "▾");
        assert_eq!(wb.shell.explorer.activate(i), None);
        assert_eq!(wb.shell.explorer.list.rows[i].icon, "▸");
        assert!(!wb.shell.explorer.list.rows.iter().any(|r| r.text == "second.txt"));
        assert!(wb.shell.explorer.list.rows.iter().any(|r| r.text == "notes.md"), "the other stays open");
        wb.shell.explorer.expand(i);
        assert!(wb.shell.explorer.list.rows.iter().any(|r| r.text == "second.txt"));
        // Right-click its heading: Remove Folder from Workspace.
        let i = wb.shell.explorer.list.rows.iter().position(|r| r.heading && r.text == name).unwrap();
        wb.ex_command("remove_root", i);
        assert!(wb.shell.explorer.extra.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
    }

    #[test]
    fn the_wheel_scrolls_tabs_and_breadcrumbs_sideways() {
        let dir = std::env::temp_dir().join(format!("turbine-tabscroll-{}/a/very/deeply/nested/folder/structure/here", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut wb = bench();
        wb.dispatch(Event::Resize(90, 20));
        if wb.shell.dock.left.visible {
            wb.run_command("view.sidebar");
        }
        for i in 0..8 {
            let p = dir.join(format!("a_long_file_name_{i:02}.rs"));
            std::fs::write(&p, "fn main() {}\n").unwrap();
            wb.goto_file(&p, Some(0), 0);
        }
        let mut surf = Surface::new(90, 20);
        wb.render(&mut surf);
        let l = wb.layout();
        let tr = l.editor_tabs.unwrap();
        assert!(!wb.shell.tab_more.is_empty());
        let first = |wb: &Workbench| wb.shell.editor_tab_rects.first().map(|t| t.0);
        let before = first(&wb);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::ScrollUp, pos: Point::new(tr.x + 2, tr.y), mods: Mods::NONE }));
        wb.render(&mut surf);
        assert_ne!(first(&wb), before, "the wheel scrolls the tabs");
        // The breadcrumbs: cut at the start; the wheel shows it.
        let br = wb.layout().breadcrumbs.unwrap();
        let x0 = wb.shell.crumbs.first().map(|c| c.0.x).unwrap();
        for _ in 0..30 {
            wb.event(Event::Mouse(MouseEvent { kind: MouseKind::ScrollUp, pos: Point::new(br.x + 3, br.y), mods: Mods::NONE }));
        }
        wb.render(&mut surf);
        assert!(wb.shell.crumbs.first().map(|c| c.0.x).unwrap() > x0, "scrolled to the start");
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("turbine-tabscroll-{}", std::process::id())));
    }

    #[test]
    fn a_long_tooltip_scrolls_without_shrinking() {
        let mut wb = bench();
        wb.dispatch(Event::Resize(100, 40));
        let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
        wb.tip.at = Some((Point::new(10, 5), Instant::now()));
        wb.tip.shown = Some((Rect::new(10, 5, 1, 1), text));
        let mut surf = Surface::new(100, 40);
        wb.render(&mut surf);
        let r = wb.tip.drawn.get().unwrap();
        assert_eq!(r.h, 20, "half the screen");
        for _ in 0..40 {
            wb.event(Event::Mouse(MouseEvent { kind: MouseKind::ScrollDown, pos: Point::new(r.x + 1, r.y + 1), mods: Mods::NONE }));
        }
        wb.render(&mut surf);
        assert_eq!(wb.tip.drawn.get().unwrap().h, 20, "the box keeps its size");
        assert!(surf.to_text().contains("line 29") && !surf.to_text().contains("line 9 "), "scrolled to its last line");
    }

    #[test]
    fn git_marks_changes_and_blames_the_cursor_line() {
        let (dir, mut wb) = project("gitlines");
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git").arg("-C").arg(&dir).args(args).env("GIT_AUTHOR_NAME", "Ada").env("GIT_AUTHOR_EMAIL", "a@x").env("GIT_COMMITTER_NAME", "Ada").env("GIT_COMMITTER_EMAIL", "a@x").output().unwrap().status.success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "The first version"]);
        wb.goto_file(&dir.join("src/main.rs"), Some(0), 0);
        wait_until(&mut wb, "the blame", |wb| wb.windows.last().unwrap().ed.blame.as_ref().is_some_and(|(_, t)| t.contains("Ada") && t.contains("The first version")));
        // An edit: its line marked changed, and not committed.
        keys(&mut wb, &["End"]);
        run(&mut wb, json!({"cmd": "type", "text": " // changed"}));
        wait_until(&mut wb, "the mark", |wb| wb.windows.last().unwrap().ed.git_marks.first() == Some(&2));
        assert!(wb.windows.last().unwrap().ed.blame.as_ref().is_some_and(|(_, t)| t.contains("not committed")));
        // Hovering the mark: what it was.
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let a = wb.windows.last().unwrap().ed.area();
        let tip = wb.git_mark_tip(Point::new(a.x, a.y)).map(|t| t.1).unwrap_or_default();
        assert!(tip.contains("- fn main() {") && tip.contains("+ fn main() { // changed"), "{tip}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn source_control_commits_ignores_and_switches_repositories() {
        let (dir, mut wb) = project("scmbar");
        let git = |d: &Path, args: &[&str]| {
            let ok = std::process::Command::new("git").arg("-C").arg(d).args(args).output().unwrap().status.success();
            assert!(ok, "git {args:?}");
        };
        let init = |d: &Path| {
            git(d, &["init", "-q", "-b", "main"]);
            git(d, &["config", "user.name", "Ada"]);
            git(d, &["config", "user.email", "a@x"]);
            git(d, &["add", "."]);
            git(d, &["commit", "-q", "-m", "first"]);
        };
        init(&dir);
        std::fs::write(dir.join("notes.md"), "# Notes\nmore\n").unwrap();
        wb.run_command("view.scm");
        wait_until(&mut wb, "the change", |wb| wb.shell.scm.has_changes() && wb.shell.scm.branch_of(None) == "main");
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        // Nothing staged: Commit commits every change (VS Code's smart commit).
        wb.shell.scm.message.set("second");
        assert_eq!(run(&mut wb, json!({"cmd": "click", "target": "button:✓ Commit"}))["ok"], true);
        wait_until(&mut wb, "the commit", |wb| !wb.shell.scm.has_changes() && wb.shell.scm.status != "Working…");
        let log = crate::views::git(&dir, &["log", "--format=%s"]).unwrap();
        assert_eq!(log.lines().collect::<Vec<_>>(), vec!["second", "first"]);
        // No upstream and nothing to commit: the button publishes the branch.
        wb.render(&mut surf);
        let inner = Rect::new(0, 0, 40, 20);
        assert_eq!(wb.scm_buttons(inner)[0].action, "publish");
        // A new file, added to .gitignore from its menu: gone from the list, dim in the Explorer.
        std::fs::write(dir.join("junk.log"), "x").unwrap();
        wb.shell.scm.refresh_all();
        wait_until(&mut wb, "the new file", |wb| wb.shell.scm.has_changes());
        let row = wb.shell.scm.entries.iter().position(|e| e.as_ref().is_some_and(|(_, p)| p == Path::new("junk.log"))).unwrap();
        wb.shell.scm.list.select(row);
        wb.scm_menu_open("file", Point::new(5, 5));
        let acts = wb.status_menu.as_ref().unwrap().2.clone();
        assert!(acts.contains(&"scm:f_ignore".to_string()) && acts.contains(&"scm:f_discard".to_string()), "{acts:?}");
        wb.status_menu = None;
        wb.scm_action("f_ignore");
        wait_until(&mut wb, "ignored", |wb| wb.shell.scm.entries.iter().flatten().all(|(_, p)| p != Path::new("junk.log")));
        assert_eq!(std::fs::read_to_string(dir.join(".gitignore")).unwrap(), "/junk.log\n");
        // A second project with its own repository: the branch menu lists both and switches.
        let other = dir.join("..").join(format!("turbine-scmbar2-{}", std::process::id()));
        std::fs::create_dir_all(&other).unwrap();
        let other = std::fs::canonicalize(&other).unwrap();
        std::fs::write(other.join("a.txt"), "a\n").unwrap();
        init(&other);
        git(&other, &["switch", "-q", "-c", "feature"]);
        std::fs::write(other.join("a.txt"), "b\n").unwrap();
        wb.shell.explorer.add_root(other.clone());
        wait_until(&mut wb, "both repositories", |wb| wb.shell.scm.repos.len() == 2 && wb.shell.scm.repos[1].branch == "feature");
        // (Stacked: each under its folder's name.)
        let texts: Vec<String> = wb.shell.scm.list.rows.iter().map(|r| r.text.clone()).collect();
        assert!(texts.contains(&format!("turbine-scmbar2-{}", std::process::id())) && texts.contains(&"a.txt".to_string()), "{texts:?}");
        wb.scm_menu_open("branch", Point::new(5, 5));
        let (popup, _, acts) = wb.status_menu.take().unwrap();
        assert_eq!(acts[1], "scm:repo:1");
        assert!(popup.items[1].1.contains("feature") && popup.items[1].1.contains("• 1"), "{:?}", popup.items);
        wb.scm_action("repo:1");
        assert!(wb.shell.scm.has_changes() && wb.shell.scm.active().unwrap().branch == "feature");
        assert_eq!(wb.scm_root(), other);
        assert!(wb.scm_buttons(inner).iter().any(|b| b.label.contains("turbine-scmbar2")));
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
    }

    #[test]
    fn scrolling_sideways() {
        let (dir, mut wb) = project("hscroll");
        std::fs::write(dir.join("wide.txt"), format!("{}\n", "x".repeat(400))).unwrap();
        wb.goto_file(&dir.join("wide.txt"), Some(0), 0);
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let inner = wb.windows.last().unwrap().win.inner();
        let at = Point::new(inner.x + 10, inner.y + 1);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::ScrollRight, pos: at, mods: Mods::NONE }));
        assert_eq!(wb.windows.last().unwrap().ed.scroll_x, 3);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::ScrollDown, pos: at, mods: Mods::SHIFT }));
        assert_eq!(wb.windows.last().unwrap().ed.scroll_x, 9);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::ScrollLeft, pos: at, mods: Mods::NONE }));
        assert_eq!(wb.windows.last().unwrap().ed.scroll_x, 6);
        // (Drawing keeps it: the cursor didn't move.)
        wb.render(&mut surf);
        assert_eq!(wb.windows.last().unwrap().ed.scroll_x, 6);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hovering_a_right_click_menu_moves_its_highlight() {
        let (dir, mut wb) = project("menuhover");
        wait_until(&mut wb, "the listing", |wb| wb.shell.explorer.list.rows.len() >= 4);
        keys(&mut wb, &["Ctrl+Shift+E", "Shift+F10"]);
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let r = wb.ex.menu.as_ref().and_then(|m| m.popup.item_rect(2)).expect("the menu");
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Moved, pos: Point::new(r.x + 2, r.y), mods: Mods::NONE }));
        assert_eq!(wb.ex.menu.as_ref().map(|m| m.popup.sel), Some(2));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tooltips_name_icons_and_show_cut_names_whole() {
        let (dir, mut wb) = project("tips");
        std::fs::write(dir.join("a_very_long_file_name_that_will_not_fit_in_the_side_bar.txt"), "").unwrap();
        wait_until(&mut wb, "the listing", |wb| wb.shell.explorer.list.rows.iter().any(|r| r.text.starts_with("a_very")));
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let rest = |wb: &mut Workbench, p: Point| {
            wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Moved, pos: p, mods: Mods::NONE }));
            wb.tick(Instant::now() + Duration::from_millis(700));
            wb.tip.shown.as_ref().map(|(_, t)| t.clone())
        };
        // The New File icon in the Explorer's header.
        let b = wb.tree().find_target("explorer.new_file").map(|n| n.rect).expect("the button");
        assert_eq!(rest(&mut wb, Point::new(b.x, b.y)).as_deref(), Some("New File"));
        // A name cut short: whole.
        wb.render(&mut surf);
        let i = wb.shell.explorer.list.rows.iter().position(|r| r.text.starts_with("a_very")).unwrap();
        let a = wb.shell.explorer.list.area;
        let y = a.y + (i - wb.shell.explorer.list.scroll) as i32;
        let tip = rest(&mut wb, Point::new(a.x + 3, y)).unwrap_or_default();
        assert!(tip.contains("a_very_long_file_name_that_will_not_fit_in_the_side_bar.txt"), "{tip}");
        wb.render(&mut surf);
        assert!(surf.to_text().contains("rw-r--r--"), "drawn");
        // The pointer can cross onto it (it stays); a click there copies it.
        let tr = wb.tip.drawn.get().expect("drawn");
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Moved, pos: Point::new(tr.x + 1, tr.y), mods: Mods::NONE }));
        assert!(wb.tip.shown.is_some(), "still shown with the pointer on it");
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(tr.x + 1, tr.y), mods: Mods::NONE }));
        assert!(wb.clipboard.contains("a_very_long_file_name"));
        // A key hides it.
        keys(&mut wb, &["Down"]);
        assert!(wb.tip.shown.is_none());
        // A file's tooltip: its path, size, date, permissions.
        let i = wb.shell.explorer.list.rows.iter().position(|r| r.text == "notes.md").unwrap();
        let at = Point::new(a.x + 3, a.y + (i - wb.shell.explorer.list.scroll) as i32);
        let tip = rest(&mut wb, at).unwrap_or_default();
        assert!(tip.contains(&dir.join("notes.md").display().to_string()) && tip.contains("8 bytes") && tip.contains("Markdown file") && tip.contains("modified ") && tip.contains("rw"), "{tip}");
        // A folder's whole size, counted when asked.
        let i = wb.shell.explorer.list.rows.iter().position(|r| r.text == "src").unwrap();
        wb.ex_command("size", i);
        wait_until(&mut wb, "the size", |wb| wb.shell.explorer.sizes.get(&dir.join("src")).is_some_and(|s| s.is_some()));
        assert!(wb.shell.explorer.describe(i).unwrap().contains("in 1 file"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_session_comes_back_with_unsaved_text() {
        let (dir, mut wb) = project("session");
        let look = dir.join("cfg/look.json");
        wb.set_look_path(look.clone());
        wb.goto_file(&dir.join("src/main.rs"), Some(1), 4);
        run(&mut wb, json!({"cmd": "type", "text": "X"}));
        wb.run_command("file.new");
        run(&mut wb, json!({"cmd": "type", "text": "scratch"}));
        wb.goto_file(&dir.join("notes.md"), Some(0), 0);
        // Exit keeps it all without asking.
        wb.run_command("app.quit");
        assert!(wb.dialog.is_none() && wb.should_quit());
        wb.save_session(true);
        let mut wb2 = Workbench::new(&[dir.clone()]);
        wb2.set_look_path(look);
        let open: Vec<(Option<PathBuf>, String, bool)> = wb2.windows.iter().filter(|w| w.term.is_none()).map(|w| (w.ed.doc.path.clone(), w.ed.doc.rope.to_string(), w.ed.doc.dirty)).collect();
        assert_eq!(open.len(), 3, "{open:?}");
        assert_eq!(open[0].0.as_deref(), Some(dir.join("src/main.rs").as_path()));
        assert!(open[0].1.contains("    Xlet needle") && open[0].2, "{open:?}");
        assert_eq!((open[1].0.clone(), open[1].1.as_str(), open[1].2), (None, "scratch", true));
        assert_eq!(title(&wb2), "notes.md");
        // The file's unsaved change undoes back to what's on disk.
        let w = wb2.windows.iter_mut().find(|w| w.ed.doc.path.as_deref() == Some(dir.join("src/main.rs").as_path())).unwrap();
        assert_eq!(w.ed.cursor, w.ed.doc.line_byte_to_char(1, 5));
        w.ed.doc.undo();
        assert!(!w.ed.doc.rope.to_string().contains('X'));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_folder_switches_the_project_and_it_comes_back_next_time() {
        let (dir, mut wb) = project("folder");
        let other = dir.join("src");
        let look = dir.join("look.json");
        wb.set_look_path(look.clone());
        wb.run_command("file.open_folder");
        assert_eq!(wb.dialog.as_ref().map(|d| d.title.as_str()), Some("Open Folder"));
        // Type the folder, Select: the Explorer shows it.
        run(&mut wb, json!({"cmd": "type", "text": other.display().to_string()}));
        keys(&mut wb, &["Enter"]);
        assert!(wb.dialog.is_none());
        assert_eq!(wb.shell.explorer.root, other);
        // Saved off the UI thread; a new start without a folder reopens it.
        wait_until(&mut wb, "look.json", |_| std::fs::read_to_string(&look).is_ok_and(|t| t.contains("\"project\"") && t.contains("/src\"")));
        let mut wb2 = Workbench::new(&[]);
        wb2.set_look_path(look.clone());
        assert_eq!(wb2.shell.explorer.root, other);
        // Started on a folder: that one wins.
        let mut wb3 = Workbench::new(&[dir.clone()]);
        wb3.set_look_path(look);
        assert_eq!(wb3.shell.explorer.root, dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn explorer_creates_renames_and_trashes_files() {
        let (dir, mut wb) = project("exops");
        // (A trash of the test's own: never the real one.)
        let trash = dir.join(".test-trash");
        // SAFETY: tests that read TURBINE_TRASH all set it to their own folder first.
        unsafe { std::env::set_var("TURBINE_TRASH", &trash) };
        wait_until(&mut wb, "the listing", |wb| wb.shell.explorer.list.rows.len() >= 4);
        keys(&mut wb, &["Ctrl+Shift+E"]);
        // The header's New File button (the project row), a name, OK: it
        // exists and opens.
        let r = run(&mut wb, json!({"cmd": "click", "target": "explorer.new_file"}));
        assert_eq!(r["ok"], true, "{r}");
        run(&mut wb, json!({"cmd": "type", "text": "new.txt"}));
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "the new file", |wb| title(wb) == "new.txt");
        assert!(dir.join("new.txt").is_file());
        // A folder from the right-click menu.
        keys(&mut wb, &["Ctrl+Shift+E", "Home", "Shift+F10"]);
        assert!(wb.ex.menu.is_some());
        run(&mut wb, json!({"cmd": "click", "target": "menuitem:New Folder"}));
        run(&mut wb, json!({"cmd": "type", "text": "docs"}));
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "the new folder", |_| dir.join("docs").is_dir());
        // Rename new.txt (F2): the open editor follows the file.
        wait_until(&mut wb, "new.txt and docs listed", |wb| ["new.txt", "docs"].iter().all(|n| wb.shell.explorer.list.rows.iter().any(|r| r.text == *n)));
        let i = wb.shell.explorer.list.rows.iter().position(|r| r.text == "new.txt").unwrap();
        wb.shell.explorer.list.select(i);
        // (The old name is highlighted: typing replaces it.)
        keys(&mut wb, &["Ctrl+Shift+E", "F2"]);
        run(&mut wb, json!({"cmd": "type", "text": "renamed.txt"}));
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "the rename", |_| dir.join("renamed.txt").is_file());
        wait_until(&mut wb, "the editor's path", |wb| wb.windows.iter().any(|w| w.ed.doc.path.as_deref() == Some(&dir.join("renamed.txt"))));
        // Delete: asks, then moves it to the trash (with its .trashinfo).
        wait_until(&mut wb, "renamed.txt listed", |wb| wb.shell.explorer.list.rows.iter().any(|r| r.text == "renamed.txt"));
        let i = wb.shell.explorer.list.rows.iter().position(|r| r.text == "renamed.txt").unwrap();
        wb.shell.explorer.list.select(i);
        keys(&mut wb, &["Ctrl+Shift+E", "Delete"]);
        assert_eq!(wb.dialog.as_ref().map(|d| d.title.as_str()), Some("Delete"));
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "the trash", |_| trash.join("files/renamed.txt").is_file());
        assert!(!dir.join("renamed.txt").exists());
        let info = std::fs::read_to_string(trash.join("info/renamed.txt.trashinfo")).unwrap();
        assert!(info.contains("Path=") && info.contains("renamed.txt"), "{info}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn split_editor_shows_one_file_twice_or_two_side_by_side() {
        let (dir, mut wb) = project("split");
        wb.goto_file(&dir.join("src/main.rs"), None, 0);
        wb.windows.retain(|w| w.dock.is_some() || w.ed.doc.path.is_some());
        // One editor: Ctrl+\ shows the same file beside it.
        wb.key(KeyEvent::new(Key::Char('\\'), Mods::CTRL));
        wb.tick(Instant::now());
        let views: Vec<usize> = (0..wb.windows.len()).filter(|&i| wb.windows[i].ed.doc.title() == "main.rs").collect();
        assert_eq!(views.len(), 2);
        let e = wb.layout().editors;
        let (l, r) = (wb.windows[views[0]].win.rect, wb.windows[views[1]].win.rect);
        // (Two groups sharing their edge: one column between them.)
        assert_eq!((l.x, r.x, l.w + r.w - 1, l.h), (e.x, l.right() - 1, e.w, e.h), "{l:?} {r:?} {e:?}");
        // Typing in one shows in the other; undo from the other takes it back.
        run(&mut wb, json!({"cmd": "type", "text": "hi "}));
        wb.tick(Instant::now());
        let line0 = |wb: &Workbench, i: usize| wb.windows[i].ed.doc.line(0);
        assert_eq!((line0(&wb, views[0]), line0(&wb, views[1])), ("hi fn main() {".to_string(), "hi fn main() {".to_string()));
        let other = wb.windows.remove(views[0]);
        wb.windows.push(other);
        keys(&mut wb, &["Ctrl+Z"]);
        wb.tick(Instant::now());
        assert!(wb.windows.iter().filter(|w| w.ed.doc.title() == "main.rs").all(|w| w.ed.doc.line(0) == "fn main() {"));
        // Saving from one: neither is marked changed.
        run(&mut wb, json!({"cmd": "type", "text": "x"}));
        keys(&mut wb, &["F2"]);
        wb.tick(Instant::now());
        assert!(wb.windows.iter().filter(|w| w.ed.doc.title() == "main.rs").all(|w| !w.ed.doc.dirty));
        // Back to one group; then two files side by side (Ctrl+Alt+Right
        // moves the one in front into a new group at the right).
        wb.run_command("view.single_group");
        assert_eq!(wb.group_count(), 1);
        wb.goto_file(&dir.join("notes.md"), None, 0);
        keys(&mut wb, &["Ctrl+Alt+Right"]);
        wb.tick(Instant::now());
        let rect = |wb: &Workbench, name: &str| wb.windows.iter().rev().find(|w| w.ed.doc.title() == name).unwrap().win.rect;
        let half = |wb: &Workbench| wb.layout().editors.x + (wb.layout().editors.w + 1) / 2 - 1;
        assert_eq!(rect(&wb, "notes.md").x, half(&wb));
        assert_eq!(rect(&wb, "main.rs").x, wb.layout().editors.x);
        // The halves follow the side bar.
        keys(&mut wb, &["Ctrl+B"]);
        wb.tick(Instant::now());
        assert_eq!(rect(&wb, "notes.md").x, half(&wb));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn editor_groups_split_resize_take_tabs_and_close() {
        let (dir, mut wb) = project("groups");
        wb.goto_file(&dir.join("src/main.rs"), None, 0);
        wb.goto_file(&dir.join("notes.md"), None, 0);
        wb.windows.retain(|w| w.dock.is_some() || w.ed.doc.path.is_some());
        let mut surf = Surface::new(120, 36);
        // Split Down: notes.md above and below; each group its own tab row.
        wb.run_command("view.split_down");
        wb.render(&mut surf);
        assert_eq!(wb.group_count(), 2);
        let rs: Vec<Rect> = wb.groups.rects.iter().map(|g| g.1).collect();
        assert_eq!(rs[0].bottom() - 1, rs[1].y, "one row between them");
        let rows = wb.tab_rows(&wb.layout());
        assert_eq!(rows.len(), 2);
        let tabs_in = |wb: &Workbench, row: Rect| wb.shell.editor_tab_rects.iter().filter(|t| t.1.y == row.y).count();
        assert_eq!((tabs_in(&wb, rows[0].1), tabs_in(&wb, rows[1].1)), (2, 1), "the top has both files, the new group the one split off");
        // The shared edge drags: the top group taller.
        let edge = Point::new(rs[0].x + 20, rs[0].bottom() - 1);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: edge, mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Drag(Button::Left), pos: Point::new(edge.x, edge.y + 4), mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: Point::new(edge.x, edge.y + 4), mods: Mods::NONE }));
        assert_eq!(wb.groups.rects[0].1.h, rs[0].h + 4);
        // main.rs's tab dragged into the lower group: it moves there.
        wb.render(&mut surf);
        let main_id = wb.windows.iter().find(|w| w.ed.doc.title() == "main.rs").unwrap().win.id;
        let tab = wb.shell.editor_tab_rects.iter().find(|t| t.0 == main_id).unwrap().1;
        let lower = wb.groups.rects[1];
        let into = Point::new(lower.1.x + lower.1.w / 2, lower.1.y + 2);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(tab.x + 1, tab.y), mods: Mods::NONE }));
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Drag(Button::Left), pos: into, mods: Mods::NONE }));
        wb.render(&mut surf);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: into, mods: Mods::NONE }));
        assert_eq!(wb.groups.of.get(&main_id), Some(&lower.0));
        // Ctrl+1: the top group's editor in front.
        keys(&mut wb, &["Ctrl+1"]);
        assert_eq!(wb.groups.active, wb.groups.rects[0].0);
        assert_eq!(wb.windows[wb.editor_index().unwrap()].ed.doc.title(), "notes.md");
        // Its last editor closed: the group goes.
        wb.run_command("window.close");
        wb.layout_dock();
        assert_eq!(wb.group_count(), 1);
        // A preset makes empty groups; Close Group takes one away.
        wb.run_command("view.three_columns");
        wb.render(&mut surf);
        assert_eq!(wb.group_count(), 3);
        keys(&mut wb, &["Ctrl+3"]);
        wb.run_command("view.close_group");
        assert_eq!(wb.group_count(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn breadcrumbs_show_folders_file_and_symbol_and_click_through() {
        let (dir, mut wb) = project("crumbs");
        wb.goto_file(&dir.join("src/main.rs"), Some(1), 4);
        let names = |wb: &Workbench| wb.shell.crumbs.iter().map(|(_, t, _)| t.clone()).collect::<Vec<_>>();
        wait_until(&mut wb, "the symbol", |wb| names(wb).iter().any(|t| t.ends_with(" main")));
        let n = names(&wb);
        let root = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(n[0], root);
        assert_eq!(n[1], "src");
        assert!(n[2].ends_with("main.rs"), "{n:?}");
        // The "src" crumb: the Explorer opens there.
        let r = run(&mut wb, json!({"cmd": "click", "target": "button:src"}));
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(wb.shell.focus, Focus::View(crate::dock::Tab::Explorer));
        assert!(wb.shell.explorer.expanded.contains(&dir.join("src")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_wheel_and_the_minimap_scroll_without_jumping_back() {
        let mut wb = bench();
        wb.dispatch(Event::Resize(140, 40));
        let text: String = (0..400).map(|i| format!("fn line_{i}() {{}}\n")).collect();
        run(&mut wb, json!({"cmd": "type", "text": text}));
        keys(&mut wb, &["Ctrl+Home"]);
        let mut surf = Surface::new(140, 40);
        wb.render(&mut surf);
        let i = wb.editor_index().unwrap();
        let mr = wb.windows[i].ed.minimap_rect().expect("a minimap in a wide editor");
        // The wheel scrolls, and the next draw doesn't jump back to the cursor.
        let inner = wb.windows[i].win.inner();
        let wheel = MouseEvent { kind: MouseKind::ScrollDown, pos: Point::new(inner.x + 5, inner.y + 5), mods: Mods::NONE };
        wb.dispatch(Event::Mouse(wheel));
        wb.render(&mut surf);
        let i = wb.editor_index().unwrap();
        assert_eq!(wb.windows[i].ed.scroll_y, 3);
        // A click near the minimap's bottom: down to the lines shown there
        // (the minimap shows the stretch around the view), cursor unmoved.
        let click = |kind| Event::Mouse(MouseEvent { kind, pos: Point::new(mr.x + 2, mr.bottom() - 1), mods: Mods::NONE });
        wb.dispatch(click(MouseKind::Down(Button::Left)));
        wb.dispatch(click(MouseKind::Up(Button::Left)));
        wb.render(&mut surf);
        let ed = &wb.windows[wb.editor_index().unwrap()].ed;
        assert!(ed.scroll_y > 40, "{}", ed.scroll_y);
        assert_eq!(ed.cursor_line_col().0, 0);
        // View > Minimap turns it off.
        wb.run_command("view.minimap");
        wb.render(&mut surf);
        assert!(wb.windows[wb.editor_index().unwrap()].ed.minimap_rect().is_none());
    }

    #[test]
    fn extensions_list_each_languages_support() {
        let mut wb = bench();
        keys(&mut wb, &["Ctrl+Shift+X"]);
        assert_eq!(wb.shell.focus, Focus::View(crate::dock::Tab::Extensions));
        wait_until(&mut wb, "the list", |wb| wb.shell.extensions.list.rows.iter().any(|r| r.text == "Rust"));
        let rows = &wb.shell.extensions.list.rows;
        assert!(rows[0].heading && rows[0].text.starts_with("Installed"), "{:?}", rows[0]);
        let i = rows.iter().position(|r| r.text == "Rust").unwrap();
        wb.shell.extensions.list.select(i);
        keys(&mut wb, &["Enter"]);
        assert!(wb.message.as_ref().is_some_and(|(m, _)| m.starts_with("Rust:")), "{:?}", wb.message);
    }

    #[test]
    fn settings_search_change_and_come_back_next_time() {
        let (dir, mut wb) = project("settings");
        let look = dir.join("look.json");
        wb.set_look_path(look.clone());
        keys(&mut wb, &["Ctrl+,"]);
        assert_eq!(wb.dialog.as_ref().map(|d| d.kind.as_str()), Some("settings"));
        // Search narrows the list; Enter on an on/off setting flips it.
        run(&mut wb, json!({"cmd": "type", "text": "minimap"}));
        let rows = |wb: &Workbench| wb.dialog.as_ref().unwrap().automation_node().find_target("dialog.items").unwrap().children.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(rows(&wb).len(), 1, "{:?}", rows(&wb));
        assert!(rows(&wb)[0].ends_with("on"));
        keys(&mut wb, &["Tab", "Enter"]);
        assert!(rows(&wb)[0].ends_with("off"), "{:?}", rows(&wb));
        assert_eq!(wb.shell.minimap, Some(false));
        // A choice: its list comes up over Settings; picking applies it.
        keys(&mut wb, &["Shift+Tab"]);
        for _ in 0..7 {
            keys(&mut wb, &["Backspace"]);
        }
        run(&mut wb, json!({"cmd": "type", "text": "control style"}));
        keys(&mut wb, &["Tab", "Enter"]);
        assert!(wb.overwrite_q.as_ref().is_some_and(|q| q.kind == "setting:style"));
        run(&mut wb, json!({"cmd": "click", "target": "listitem:VB-DOS"}));
        keys(&mut wb, &["Enter"]);
        assert_eq!(wb.theme.style.name, "VB-DOS");
        assert!(wb.dialog.as_ref().is_some_and(|d| d.kind == "settings"), "Settings stays open");
        // A typed value: the agent's typing speed.
        keys(&mut wb, &["Shift+Tab"]);
        for _ in 0..13 {
            keys(&mut wb, &["Backspace"]);
        }
        run(&mut wb, json!({"cmd": "type", "text": "typing speed"}));
        keys(&mut wb, &["Tab", "Enter"]);
        run(&mut wb, json!({"cmd": "type", "text": "120"}));
        keys(&mut wb, &["Enter"]);
        assert_eq!(wb.driver.cps, 120.0);
        // Kept: a new start reads them back.
        wait_until(&mut wb, "look.json", |_| std::fs::read_to_string(&look).is_ok_and(|t| t.contains("\"agent_speed\": 120")));
        let mut wb2 = Workbench::new(&[dir.clone()]);
        wb2.set_look_path(look);
        assert_eq!((wb2.theme.style.name, wb2.shell.minimap, wb2.driver.cps), ("VB-DOS", Some(false), 120.0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_wasm_extension_adds_a_menu_item_and_edits_through_it() {
        // (Built and packed as extensions/build.sh does; turbine-ext's own
        // test builds the same extension.)
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extensions/hello");
        let st = std::process::Command::new(env!("CARGO"))
            .args(["build", "--quiet", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path"])
            .arg(root.join("Cargo.toml"))
            .status()
            .unwrap();
        assert!(st.success());
        let core = std::fs::read(root.join("target/wasm32-unknown-unknown/release/turbine_hello.wasm")).unwrap();
        let (dir, mut wb) = project("wasm");
        std::fs::create_dir_all(dir.join("exts/hello")).unwrap();
        std::fs::write(dir.join("exts/hello/extension.wasm"), turbine_ext::pack(&core).unwrap()).unwrap();
        wb.start_extensions(vec![dir.join("exts")]);
        wait_until(&mut wb, "the Tools menu", |wb| wb.menubar.menus.iter().any(|m| m.plain_title() == "Tools" && m.items.iter().filter(|i| i.command.starts_with("ext:")).count() == 2));
        // The menu item, through the palette as a person would find it.
        wb.goto_file(&dir.join("notes.md"), Some(0), 0);
        keys(&mut wb, &["Ctrl+Shift+P"]);
        run(&mut wb, json!({"cmd": "type", "text": "uppercase line"}));
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "the edit", |wb| wb.windows.last().unwrap().ed.doc.line(0) == "# NOTES");
        // Its key binding, and its greeting on the status line.
        keys(&mut wb, &["Ctrl+Z"]);
        keys(&mut wb, &["Ctrl+Alt+U"]);
        wait_until(&mut wb, "the key", |wb| wb.windows.last().unwrap().ed.doc.line(0) == "# NOTES");
        wb.run_command("ext:hello:hello.greet");
        wait_until(&mut wb, "the greeting", |wb| wb.message.as_ref().is_some_and(|(m, _)| m.contains("Hello from a WebAssembly")));
        // The Extensions view lists it.
        wait_until(&mut wb, "the list", |wb| wb.shell.extensions.list.rows.iter().any(|r| r.text == "hello" && r.icon == "✓"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn language_server_diagnostics_completion_definition_hover() {
        let dir = std::env::temp_dir().join(format!("turbine-lsp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("main.rs");
        std::fs::write(&f, "fn main() {\n    BAD;\n    pr\n}\n").unwrap();
        let mut wb = Workbench::new(&[f.clone()]);
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
        keys(&mut wb, &["Esc"]);
        // (And the guides' page for it, in front: closed again.)
        assert!(wb.windows.last().is_some_and(|w| w.win.title.starts_with("HELP: Topic: println")));
        wb.run_command("window.close");

        // The mouse resting on code: the server's hover in a tooltip.
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let inner = wb.windows.last().unwrap().win.inner();
        let gutter = wb.windows.last().unwrap().ed.area().x;
        let _ = gutter;
        let line2 = wb.windows.last().unwrap().ed.doc.line(2);
        let col = line2.find("println").unwrap() as i32;
        let area = wb.windows.last().unwrap().ed.area();
        let _ = inner;
        let ed = &wb.windows.last().unwrap().ed;
        let x = (area.x..area.right()).find(|&x| ed.char_at(Point::new(x, area.y + 2)).map(|c| ed.doc.char_to_line_byte(c)) == Some((2, col as usize + 1))).unwrap();
        let at = Point::new(x, area.y + 2);
        wb.event(Event::Mouse(MouseEvent { kind: MouseKind::Moved, pos: at, mods: Mods::NONE }));
        wb.tick(Instant::now() + Duration::from_millis(700));
        wait_until(&mut wb, "the hover tooltip", |wb| wb.tip.shown.as_ref().is_some_and(|(_, t)| t.contains("fake hover")));

        // Shift+F12 on "println!": its uses, in the Search view.
        keys(&mut wb, &["Shift+F12"]);
        wait_until(&mut wb, "references", |wb| !wb.shell.search.busy() && wb.shell.search.hits.iter().flatten().count() == 1);
        assert!(wb.shell.search.query.text.starts_with("References: println"));

        // Shift+F2 renames "main" (line 0) everywhere it's used.
        wb.goto_file(&f, Some(0), 3);
        keys(&mut wb, &["Shift+F2"]);
        assert_eq!(wb.dialog.as_ref().map(|d| d.kind.as_str()), Some("rename_symbol"));
        run(&mut wb, json!({"cmd": "type", "text": "start"}));
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "the rename", |wb| wb.windows.iter().any(|w| w.ed.doc.line(0).starts_with("fn start")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn language_server_format_fixes_signatures_hints_symbols() {
        let dir = std::env::temp_dir().join(format!("turbine-lsp2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("main.rs");
        std::fs::write(&f, "fn main() {   \n    let x = Foo;\n    BAD;\n    add(\n}\n").unwrap();
        let mut wb = Workbench::new(&[f.clone()]);
        wb.set_lsp_spawner(Box::new(|lang, root| {
            Some(turbine_services::fake::fake_server().map(|rpc| LspClient::with_rpc(rpc, root, lang)))
        }));
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        let ed = |wb: &Workbench| wb.windows.iter().find(|w| w.ed.doc.path.as_ref() == Some(&f)).unwrap().ed.doc.rope.to_string();
        wait_until(&mut wb, "diagnostics", |wb| !wb.windows[0].ed.diagnostics.is_empty());

        // Inlay hints after `let x`; semantic tokens make Foo a type.
        wait_until(&mut wb, "inlay hints", |wb| wb.windows[0].ed.inlays.get(&1).is_some_and(|h| h[0] == (9, ": i32".to_string())));
        wait_until(&mut wb, "semantic tokens", |wb| wb.windows[0].ed.semantic.get(1).is_some_and(|s| s.contains(&(12, 15, turbine_syntax::kind_index("type").unwrap()))));
        let mut surf = Surface::new(100, 30);
        wb.render(&mut surf);
        assert!(surf.to_text().contains("let x: i32 = Foo;"), "the hint shows in the line");

        // Format Document: the trailing spaces go, as one undo step.
        keys(&mut wb, &["Ctrl+Shift+I"]);
        wait_until(&mut wb, "formatting", |wb| ed(wb).starts_with("fn main() {\n"));
        keys(&mut wb, &["Ctrl+Z"]);
        assert!(ed(&wb).starts_with("fn main() {   \n"));
        keys(&mut wb, &["Ctrl+Y"]);

        // Ctrl+. on the BAD line: the preferred fix first; Enter applies it.
        wb.goto_file(&f, Some(2), 5);
        keys(&mut wb, &["Ctrl+."]);
        wait_until(&mut wb, "quick fixes", |wb| wb.lspx.menu.is_some());
        let titles: Vec<String> = wb.lspx.menu.as_ref().unwrap().popup.items.iter().map(|i| i.0.clone()).collect();
        assert_eq!(titles, vec!["★ Replace BAD with GOOD", "Add a header", "Apply from the server"]);
        assert!(run(&mut wb, json!({"cmd": "snapshot"})).to_string().contains("\"id\":\"quick_fix\""));
        keys(&mut wb, &["Enter"]);
        assert!(ed(&wb).contains("    GOOD;"), "{}", ed(&wb));
        // One that's resolved first, and one the server applies itself.
        let choose = |wb: &mut Workbench, title: &str| {
            keys(wb, &["Ctrl+."]);
            wait_until(wb, "quick fixes", |wb| wb.lspx.menu.is_some());
            let i = wb.lspx.menu.as_ref().unwrap().popup.items.iter().position(|it| it.0 == title).unwrap();
            for _ in 0..i {
                keys(wb, &["Down"]);
            }
            keys(wb, &["Enter"]);
        };
        choose(&mut wb, "Add a header");
        wait_until(&mut wb, "the resolved action", |wb| ed(wb).starts_with("// header\n"));
        choose(&mut wb, "Apply from the server");
        wait_until(&mut wb, "the server's edit", |wb| ed(wb).ends_with("// applied\n"));

        // Typing ( shows the signature, the parameter by the commas.
        let line = ed(&wb).lines().position(|l| l.contains("add(")).unwrap();
        wb.goto_file(&f, Some(line), 8);
        run(&mut wb, json!({"cmd": "type", "text": "("}));
        wait_until(&mut wb, "signature", |wb| wb.lspx.signature.is_some());
        assert_eq!(wb.lspx.signature.as_ref().unwrap().2.active, Some((4, 10)));
        run(&mut wb, json!({"cmd": "type", "text": "1,"}));
        wait_until(&mut wb, "second parameter", |wb| wb.lspx.signature.as_ref().is_some_and(|s| s.2.active == Some((12, 18))));
        let mut surf = Surface::new(100, 30);
        wb.render(&mut surf);
        assert!(surf.to_text().contains("add(a: i32, b: i32) -> i32"));
        run(&mut wb, json!({"cmd": "type", "text": "2)"}));
        assert!(wb.lspx.signature.is_none(), ") closes it");

        // Ctrl+T: the project's symbols; Enter goes there.
        keys(&mut wb, &["Ctrl+T"]);
        run(&mut wb, json!({"cmd": "type", "text": "mai"}));
        wait_until(&mut wb, "workspace symbols", |wb| wb.shell.palette.as_ref().is_some_and(|p| p.pick(0).is_some_and(|p| matches!(p, crate::palette::Pick::Place(..)))));
        keys(&mut wb, &["Enter"]);
        let w = wb.windows.last().unwrap();
        assert_eq!(w.ed.doc.path.as_ref(), Some(&f));
        assert_eq!(w.ed.cursor_line_col(), (1, 3));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_language_server_offers_an_install() {
        let dir = std::env::temp_dir().join(format!("turbine-lsp3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("main.go");
        std::fs::write(&f, "package main\n").unwrap();
        let mut wb = Workbench::new(&[f]);
        wb.set_lsp_spawner(Box::new(|_, _| None));
        wb.dispatch(Event::Resize(100, 30));
        wait_until(&mut wb, "no server", |wb| wb.lsp_missing.contains("go"));
        keys(&mut wb, &["F12"]);
        let d = wb.dialog.as_ref().expect("the offer");
        assert!(d.kind.starts_with("install:go\u{1f}"), "{}", d.kind);
        keys(&mut wb, &["Esc"]);
        keys(&mut wb, &["F12"]);
        assert!(wb.dialog.is_none(), "offered once");
        assert!(wb.message.as_ref().unwrap().0.contains("Install Language Server"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn debugger_watches_evaluate_conditions_immediate() {
        let dir = std::env::temp_dir().join(format!("turbine-dbg2-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let f = dir.join("prog.py");
        std::fs::write(&f, "print(a)\nx = 1\nprint(b)\nprint(c)\nprint(d)\n").unwrap();
        let mut wb = Workbench::new(&[f.clone()]);
        wb.set_lsp_spawner(Box::new(|_, _| None));
        wb.set_dap_spawner(Box::new(|_, launch, bps| {
            Some(turbine_services::fake::fake_debug_adapter().map(|rpc| DapClient::with_rpc(rpc, launch, bps)))
        }));
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        let special = |wb: &Workbench, k: &str| wb.windows.iter().find(|w| w.special == Some(k)).map(|w| w.ed.doc.rope.to_string()).unwrap_or_default();
        let set_bp = |wb: &mut Workbench, line: usize, fields: &[&str]| {
            wb.goto_file(&f, Some(line), 0);
            keys(wb, &["Ctrl+Shift+F8"]);
            assert!(wb.dialog.as_ref().is_some_and(|d| d.kind.starts_with("bp_opts:")));
            for (i, t) in fields.iter().enumerate() {
                if i > 0 {
                    keys(wb, &["Tab"]);
                }
                run(wb, json!({"cmd": "type", "text": t}));
            }
            keys(wb, &["Enter"]);
            assert!(wb.dialog.is_none());
        };
        // Line 2 never stops (its condition); line 3 logs; line 4 stops.
        set_bp(&mut wb, 1, &["line > 9"]);
        set_bp(&mut wb, 2, &["", "", "at line {line}"]);
        set_bp(&mut wb, 3, &["line == 4"]);
        assert_eq!(wb.breakpoints.get(&Workbench::canon(&f)).map(|s| s.len()), Some(3));

        // Watches, before running: not available yet.
        keys(&mut wb, &["Ctrl+F7"]);
        run(&mut wb, json!({"cmd": "type", "text": "text"}));
        keys(&mut wb, &["Enter"]);
        wb.add_watch("nope");
        wb.tick(Instant::now());
        assert!(wb.shell.debug_view.rows.iter().any(|r| r.text == "text: <not available>"));

        keys(&mut wb, &["Ctrl+F9"]);
        wait_until(&mut wb, "the conditional stop", |wb| wb.windows.iter().any(|w| w.ed.exec_line == Some(3)));
        assert!(special(&wb, "output").contains("at line 3\n"), "{}", special(&wb, "output"));
        wait_until(&mut wb, "watches", |wb| special(wb, "debug").contains("text: \"print(c)\"") && special(wb, "debug").contains("nope: <name 'nope' is not defined>"));

        // Ctrl+F4: evaluate, then modify.
        keys(&mut wb, &["Ctrl+F4"]);
        run(&mut wb, json!({"cmd": "type", "text": "line"}));
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "the result", |wb| wb.dialog.as_ref().and_then(|d| d.value("result")).as_deref() == Some("4"));
        let d = wb.dialog.as_mut().unwrap();
        d.set_value("expr", "x");
        d.set_value("value", "42");
        let d = wb.dialog.take().unwrap();
        wb.finish_dialog(d, Some("modify".into()));
        wait_until(&mut wb, "the change", |wb| wb.dialog.as_ref().and_then(|d| d.value("result")).as_deref() == Some("42"));
        keys(&mut wb, &["Esc"]);

        // The Immediate window runs what's typed in the stopped program.
        keys(&mut wb, &["Ctrl+Shift+Y"]);
        assert_eq!(wb.windows.last().unwrap().special, Some("immediate"));
        run(&mut wb, json!({"cmd": "type", "text": "x"}));
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "the answer", |wb| special(wb, "immediate") == "x\n42\n");

        // Delete removes a watch in Run and Debug.
        wb.show_tab(crate::dock::Tab::Debug);
        wb.shell.focus = Focus::View(crate::dock::Tab::Debug);
        wb.shell.debug_view.select(2);
        keys(&mut wb, &["Delete"]);
        assert_eq!(wb.dbx.watches, vec!["text".to_string()]);

        wb.shell.focus = Focus::Editors;
        keys(&mut wb, &["Ctrl+F9"]);
        wait_until(&mut wb, "finish", |wb| wb.debug.is_none());
        // (A check of the file may still be writing __pycache__.)
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn debugger_launch_configurations() {
        let dir = std::env::temp_dir().join(format!("turbine-dbg3-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".vscode")).unwrap();
        std::fs::write(dir.join("one.py"), "print(one)\n").unwrap();
        std::fs::write(dir.join("two.py"), "print(two)\n").unwrap();
        std::fs::write(
            dir.join(".vscode/launch.json"),
            "{\n  // comments, as VS Code writes them\n  \"configurations\": [\n    {\"name\": \"One\", \"type\": \"debugpy\", \"request\": \"launch\", \"program\": \"${workspaceFolder}/one.py\"},\n    {\"name\": \"Two\", \"type\": \"debugpy\", \"request\": \"launch\", \"program\": \"${workspaceFolder}/two.py\",},\n  ],\n}\n",
        )
        .unwrap();
        let mut wb = Workbench::new(&[dir.join("one.py")]);
        wb.set_lsp_spawner(Box::new(|_, _| None));
        let langs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = langs.clone();
        wb.set_dap_spawner(Box::new(move |lang, launch, bps| {
            seen.borrow_mut().push(lang.to_string());
            Some(turbine_services::fake::fake_debug_adapter().map(|rpc| DapClient::with_rpc(rpc, launch, bps)))
        }));
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        wb.run_command("run.debug");
        let d = wb.dialog.as_ref().expect("which configuration");
        assert_eq!(d.title, "Select Debug Configuration");
        keys(&mut wb, &["Down", "Enter"]);
        let special = |wb: &Workbench, k: &str| wb.windows.iter().find(|w| w.special == Some(k)).map(|w| w.ed.doc.rope.to_string()).unwrap_or_default();
        wait_until(&mut wb, "finish", |wb| wb.debug.is_none() && special(wb, "output") == "two\n");
        assert_eq!(*langs.borrow(), vec!["python".to_string()]);
        // The choice is remembered.
        wb.run_command("run.debug");
        assert!(wb.dialog.is_none());
        wait_until(&mut wb, "again", |wb| wb.debug.is_none() && special(wb, "output") == "two\ntwo\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tree_sitter_folds_selects_and_moves_by_definition() {
        let dir = std::env::temp_dir().join(format!("turbine-ts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("m.rs");
        std::fs::write(&f, "fn a() {\n    let x = (1 + 2);\n}\n\nfn b() {\n    x();\n}\n").unwrap();
        let mut wb = Workbench::new(&[f]);
        wb.set_lsp_spawner(Box::new(|_, _| None));
        wb.dispatch(Event::Resize(80, 25));
        wait_until(&mut wb, "structure", |wb| wb.windows[0].ed.structure_version == wb.windows[0].ed.doc.version && !wb.windows[0].ed.structure.folds.is_empty());
        assert_eq!(wb.windows[0].ed.line_comment.as_deref(), Some("//"));
        wb.run_command("view.fold_all");
        let mut surf = Surface::new(80, 25);
        wb.render(&mut surf);
        let t = surf.to_text();
        assert!(t.contains("fn a() { ⋯") && t.contains("fn b() { ⋯") && !t.contains("let x"), "{t}");
        wb.run_command("view.unfold_all");
        keys(&mut wb, &["Alt+PgDn"]);
        assert_eq!(wb.windows[0].ed.cursor_line_col(), (4, 0));
        keys(&mut wb, &["Alt+PgUp"]);
        assert_eq!(wb.windows[0].ed.cursor_line_col(), (0, 0));
        // Shift+Alt+Right grows from `1` out to the parentheses.
        wb.goto_file(&wb.windows[0].ed.doc.path.clone().unwrap(), Some(1), 13);
        keys(&mut wb, &["Shift+Alt+Right", "Shift+Alt+Right"]);
        assert_eq!(wb.windows[0].ed.selected_text().as_deref(), Some("1 + 2"));
        keys(&mut wb, &["Shift+Alt+Right"]);
        assert_eq!(wb.windows[0].ed.selected_text().as_deref(), Some("(1 + 2)"));
        keys(&mut wb, &["Esc", "Ctrl+/"]);
        assert_eq!(wb.windows[0].ed.doc.line(1), "    // let x = (1 + 2);");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn make_compile_and_checks_fill_problems() {
        let dir = std::env::temp_dir().join(format!("turbine-make-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("a.c"), "int main() {\n  return x;\n}\n").unwrap();
        std::fs::write(dir.join("Makefile"), "all:\n\t@echo 'a.c:2:10: error: x undeclared'; echo 'a.c:1:5: warning: odd main'; false\n").unwrap();
        std::fs::write(dir.join("bad.sh"), "if true; then\n  echo hi\n").unwrap();
        let mut wb = Workbench::new(&[dir.join("bad.sh")]);
        wb.set_lsp_spawner(Box::new(|_, _| None));
        wb.dispatch(Event::Resize(100, 30));
        // A shell file with no language server is checked when opened.
        wait_until(&mut wb, "bash -n", |wb| !wb.windows[0].ed.diagnostics.is_empty());
        assert_eq!(wb.windows[0].ed.diagnostics[0].severity, 1);
        // F9: the project's build; its messages go to Problems.
        keys(&mut wb, &["F9"]);
        wait_until(&mut wb, "the build", |wb| !wb.build_busy() && wb.message.as_ref().is_some_and(|m| m.0.contains("failed")));
        assert!(wb.message.as_ref().unwrap().0.contains("1 errors, 1 warnings"), "{:?}", wb.message);
        let out = wb.windows.iter().find(|w| w.special == Some("output")).unwrap().ed.doc.rope.to_string();
        assert!(out.starts_with("> make") && out.contains("a.c:2:10: error"), "{out}");
        wb.tick(Instant::now());
        let i = wb.shell.problems.rows.iter().position(|r| r.text == "x undeclared").expect("in Problems");
        wb.problem_goto(i);
        let w = wb.windows.last().unwrap();
        assert!(w.ed.doc.path.as_ref().unwrap().ends_with("a.c"));
        assert_eq!(w.ed.cursor_line_col(), (1, 9));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn help_manual_and_its_index() {
        let mut wb = Workbench::new(&[]);
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        keys(&mut wb, &["F1"]);
        assert_eq!(wb.windows.last().unwrap().win.title, "HELP: Turbine");
        // (The Contents / Index / Back row, then the page.)
        wait_until(&mut wb, "rendered", |wb| wb.windows.last().unwrap().ed.doc.line(2) == "Turbine");
        assert!(wb.windows.last().unwrap().ed.doc.line(0).trim_start().starts_with("◄Contents►  ◄Index►  ◄Back►"));
        keys(&mut wb, &["Shift+F1"]);
        let d = wb.dialog.as_mut().expect("the index");
        let items = match d.controls.iter().find_map(|c| match &c.control {
            turbine_ui::dialog::Control::List { items, .. } => Some(items.clone()),
            _ => None,
        }) {
            Some(v) => v,
            None => panic!("no list"),
        };
        let at = items.iter().position(|t| t == "Debugging").unwrap();
        assert!(items.iter().any(|t| t.trim() == "tessh"));
        for _ in 0..at {
            keys(&mut wb, &["Down"]);
        }
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "the heading", |wb| {
            let w = wb.windows.last().unwrap();
            w.ed.doc.line(w.ed.cursor_line_col().0) == "Debugging"
        });
    }

    #[test]
    fn reopen_with_encoding_and_binary_files() {
        let dir = std::env::temp_dir().join(format!("turbine-enc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Latin-1 bytes chardetng may call something else: reopen as CP850.
        std::fs::write(dir.join("a.txt"), b"caf\x82 na\x8bve\n").unwrap();
        std::fs::write(dir.join("b.bin"), [0u8, 1, 2, 3, 0, 9]).unwrap();
        let mut wb = Workbench::new(&[dir.join("a.txt")]);
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        wb.run_command("file.reopen_encoding");
        let d = wb.dialog.as_ref().unwrap();
        assert_eq!(d.title, "Reopen with Encoding");
        let items = match d.controls.iter().find_map(|c| match &c.control {
            turbine_ui::dialog::Control::List { items, .. } => Some(items.clone()),
            _ => None,
        }) {
            Some(v) => v,
            None => panic!("no list"),
        };
        let i = items.iter().position(|t| t.starts_with("CP850")).unwrap();
        let mut d = wb.dialog.take().unwrap();
        for p in &mut d.controls {
            if let turbine_ui::dialog::Control::List { selected, .. } = &mut p.control {
                *selected = i;
            }
        }
        wb.finish_dialog(d, Some("ok".into()));
        assert_eq!(wb.windows[0].ed.doc.line(0), "café naïve");
        assert_eq!(wb.windows[0].ed.doc.encoding.name(), "CP850");
        // A binary file asks; Hex View shows it.
        wb.open_path(&dir.join("b.bin"));
        assert!(wb.dialog.as_ref().unwrap().kind.starts_with("binary:"));
        let d = wb.dialog.take().unwrap();
        wb.finish_dialog(d, Some("hex".into()));
        assert!(wb.windows.last().unwrap().ed.doc.line(0).starts_with("00000000  00 01 02 03"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn untitled_text_guesses_its_language() {
        let mut wb = bench();
        run(&mut wb, json!({"cmd": "type", "text": "def f(x):\n    return x\nimport os\n"}));
        wait_until(&mut wb, "python", |wb| wb.windows.last().unwrap().lang.as_deref() == Some("python"));
        // Chosen by hand, it stays.
        wb.run_command("edit.language");
        let mut d = wb.dialog.take().unwrap();
        let items = match d.controls.iter().find_map(|c| match &c.control {
            turbine_ui::dialog::Control::List { items, .. } => Some(items.clone()),
            _ => None,
        }) {
            Some(v) => v,
            None => panic!(),
        };
        let i = items.iter().position(|t| t == "tessh").unwrap();
        for p in &mut d.controls {
            if let turbine_ui::dialog::Control::List { selected, .. } = &mut p.control {
                *selected = i;
            }
        }
        wb.finish_dialog(d, Some("ok".into()));
        wait_until(&mut wb, "tessh", |wb| wb.windows.last().unwrap().lang.as_deref() == Some("tessh"));
        run(&mut wb, json!({"cmd": "type", "text": "print(1)\n"}));
        wb.tick(Instant::now());
        assert_eq!(wb.windows.last().unwrap().lang.as_deref(), Some("tessh"));
    }

    #[test]
    fn markdown_preview_follows_its_file() {
        let dir = std::env::temp_dir().join(format!("turbine-md-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("r.md"), "# Read me\n\n- one\n").unwrap();
        let mut wb = Workbench::new(&[dir.join("r.md")]);
        wb.set_lsp_spawner(Box::new(|_, _| None));
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(120, 30));
        wait_until(&mut wb, "markdown", |wb| wb.windows[0].lang.as_deref() == Some("markdown"));
        keys(&mut wb, &["Ctrl+Shift+V"]);
        let pv = wb.windows.last().unwrap().win.id;
        assert_eq!(wb.windows.last().unwrap().special, Some("preview"));
        let text = |wb: &Workbench| wb.windows.iter().find(|w| w.win.id == pv).unwrap().ed.doc.rope.to_string();
        wait_until(&mut wb, "rendered", |wb| text(wb).starts_with("Read me\n") && text(wb).contains("■ one"));
        // Typing in the file changes the preview a moment later.
        let src = wb.windows.iter().position(|w| w.special.is_none()).unwrap();
        let w = wb.windows.remove(src);
        wb.windows.push(w);
        keys(&mut wb, &["Ctrl+End"]);
        run(&mut wb, json!({"cmd": "type", "text": "- two\n"}));
        wait_until(&mut wb, "followed", |wb| text(wb).contains("■ two"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_changes_history_and_patches_like_delta() {
        let dir = std::env::temp_dir().join(format!("turbine-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sh = |c: &str| assert!(std::process::Command::new("sh").arg("-c").arg(c).current_dir(&dir).status().unwrap().success(), "{c}");
        sh("git init -q && git config user.email t@t && git config user.name T");
        std::fs::write(dir.join("m.py"), "def f():\n    return 1\n\nprint(f())\n").unwrap();
        sh("git add . && git commit -qm 'First commit'");
        std::fs::write(dir.join("m.py"), "def f():\n    return 2\n\nprint(f())\n").unwrap();
        let mut wb = Workbench::new(&[dir.join("m.py")]);
        wb.set_lsp_spawner(Box::new(|_, _| None));
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(120, 30));
        wb.run_command("git.diff_file");
        let page = |wb: &Workbench| wb.windows.last().unwrap().ed.doc.rope.to_string();
        wait_until(&mut wb, "the diff", |wb| page(wb).contains("-    return 1"));
        let w = wb.windows.last().unwrap();
        let del = (0..w.ed.doc.len_lines()).find(|&l| w.ed.doc.line(l).ends_with("-    return 1")).unwrap();
        assert_eq!((w.ed.line_bg[del], w.ed.line_bg[del + 1]), (2, 1));
        // n goes to the change; Enter to the file at that line.
        keys(&mut wb, &["Ctrl+Home", "n"]);
        assert_eq!(wb.windows.last().unwrap().ed.cursor_line_col().0, del);
        keys(&mut wb, &["Down", "Enter"]);
        assert!(wb.windows.last().unwrap().ed.doc.path.as_ref().unwrap().ends_with("m.py"));
        assert_eq!(wb.windows.last().unwrap().ed.cursor_line_col().0, 1);
        // s: side by side.
        wb.run_command("git.diff_all");
        wait_until(&mut wb, "all changes", |wb| page(wb).contains("return 2"));
        keys(&mut wb, &["s"]);
        wait_until(&mut wb, "side by side", |wb| page(wb).lines().any(|l| l.contains("return 1") && l.contains("return 2")));
        // History, then the commit.
        wb.run_command("git.log");
        wait_until(&mut wb, "history", |wb| page(wb).contains("First commit"));
        keys(&mut wb, &["Ctrl+Home", "Enter"]);
        wait_until(&mut wb, "the commit", |wb| page(wb).contains("+    return 1") && page(wb).starts_with("commit "));
        // A patch file, rendered with Ctrl+Shift+V.
        sh("git diff > change.patch");
        wb.open_path(&dir.join("change.patch"));
        wait_until(&mut wb, "diff language", |wb| wb.windows.last().unwrap().lang.as_deref() == Some("diff"));
        keys(&mut wb, &["Ctrl+Shift+V"]);
        wait_until(&mut wb, "the patch", |wb| wb.windows.last().unwrap().special == Some("preview") && wb.windows.last().unwrap().ed.line_bg.contains(&2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn debugging_a_tessh_script() {
        // (Needs a tessh with --dap.)
        let has = std::process::Command::new("tessh").arg("--help").output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("--dap"));
        if !has {
            eprintln!("skipped: no tessh --dap");
            return;
        }
        let dir = std::env::temp_dir().join(format!("turbine-tsh-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let f = dir.join("s.tsh");
        std::fs::write(&f, "total = 0\ndef add a b\n    c = $a + $b\n    return $c\nfor i in [1 2 3]\n    total = add $total $i\n    print \"total $total\"\necho done\n").unwrap();
        let mut wb = Workbench::new(&[f.clone()]);
        wb.set_lsp_spawner(Box::new(|_, _| None));
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        let special = |wb: &Workbench, k: &str| wb.windows.iter().find(|w| w.special == Some(k)).map(|w| w.ed.doc.rope.to_string()).unwrap_or_default();
        wait_until(&mut wb, "tessh", |wb| wb.windows[0].lang.as_deref() == Some("tessh"));
        // A breakpoint in add, only when a is 1.
        wb.goto_file(&f, Some(2), 0);
        keys(&mut wb, &["Ctrl+Shift+F8"]);
        run(&mut wb, json!({"cmd": "type", "text": "a == 1"}));
        keys(&mut wb, &["Enter"]);
        keys(&mut wb, &["Ctrl+F9"]);
        let start = Instant::now();
        while !wb.windows.iter().any(|w| w.ed.exec_line == Some(2)) {
            wb.tick(Instant::now());
            assert!(start.elapsed() < Duration::from_secs(20), "no stop: {}", special(&wb, "output"));
            std::thread::sleep(Duration::from_millis(10));
        }
        wait_until(&mut wb, "variables", |wb| special(wb, "debug").contains("a: int = 1"));
        // The Immediate window: tessh's own syntax works there.
        wb.add_watch("a + b");
        wait_until(&mut wb, "the watch", |wb| special(wb, "debug").contains("a + b: 3"));
        keys(&mut wb, &["F8"]);
        wait_until(&mut wb, "step", |wb| wb.windows.iter().any(|w| w.ed.exec_line == Some(3)));
        keys(&mut wb, &["Ctrl+F9"]);
        let start = Instant::now();
        while wb.debug.is_some() {
            wb.tick(Instant::now());
            assert!(start.elapsed() < Duration::from_secs(20), "didn't finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = special(&wb, "output");
        assert!(out.contains("total 1\n") && out.contains("total 6\n") && out.contains("done\n"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_files_and_back_and_forward() {
        let dir = std::env::temp_dir().join(format!("turbine-nav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("b.txt"), "x\ny\n").unwrap();
        let mut wb = Workbench::new(&[dir.join("a.txt")]);
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        keys(&mut wb, &["Down", "Down"]);
        wb.goto_file(&dir.join("b.txt"), Some(1), 0);
        keys(&mut wb, &["Alt+Left"]);
        let w = wb.windows.last().unwrap();
        assert!(w.ed.doc.path.as_ref().unwrap().ends_with("a.txt"));
        assert_eq!(w.ed.cursor_line_col().0, 2);
        keys(&mut wb, &["Alt+Right"]);
        assert!(wb.windows.last().unwrap().ed.doc.path.as_ref().unwrap().ends_with("b.txt"));
        wb.run_command("file.recent");
        let d = wb.dialog.as_ref().unwrap();
        assert!(d.value("file").unwrap().ends_with("a.txt") || d.value("file").unwrap().ends_with("b.txt"));
        keys(&mut wb, &["Esc"]);
        wb.run_command("tools.calculator");
        run(&mut wb, json!({"cmd": "type", "text": "6*7"}));
        keys(&mut wb, &["Enter"]);
        assert_eq!(wb.dialog.as_ref().unwrap().value("result").as_deref(), Some("42  0x2A  0b101010"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_clicks_preview_files_like_vscode() {
        let dir = std::env::temp_dir().join(format!("turbine-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.md"), "# Notes\n").unwrap();
        std::fs::write(dir.join("other.txt"), "other\n").unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let mut wb = Workbench::new(&[dir.clone()]);
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(120, 36));
        let rows = |wb: &Workbench, name: &str| wb.shell.explorer.list.rows.iter().position(|r| r.text.ends_with(name)).unwrap();
        wb.tick(Instant::now());
        let click = |wb: &mut Workbench, i: usize| {
            let a = wb.shell.explorer.list.area;
            let y = a.y + (i - wb.shell.explorer.list.scroll) as i32;
            let p = Point::new(a.x + 3, y);
            wb.dispatch(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: p, mods: Mods::NONE }));
            wb.dispatch(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: p, mods: Mods::NONE }));
        };
        wait_until(&mut wb, "files listed", |wb| wb.shell.explorer.list.rows.iter().any(|r| r.text.ends_with("notes.md")));
        let i = rows(&wb, "notes.md");
        click(&mut wb, i);
        let pv = wb.shell.preview_tab.expect("a preview tab");
        // (The untouched empty window gives way to it.)
        let n0 = wb.windows.len() - 1;
        // Another file replaces it.
        let j = rows(&wb, "other.txt");
        std::thread::sleep(Duration::from_millis(600));
        click(&mut wb, j);
        assert_eq!(wb.windows.len(), n0 + 1, "the preview was replaced");
        assert!(wb.windows.iter().all(|w| w.win.id != pv));
        assert!(wb.windows.iter().any(|w| w.ed.doc.path.as_ref().is_some_and(|p| p.ends_with("other.txt"))));
        // Editing keeps it; the next click opens a new tab.
        wb.goto_file(&dir.join("other.txt"), Some(0), 0);
        run(&mut wb, json!({"cmd": "type", "text": "x"}));
        wb.tick(Instant::now());
        assert!(wb.shell.preview_tab.is_none());
        std::thread::sleep(Duration::from_millis(600));
        let i = rows(&wb, "notes.md");
        click(&mut wb, i);
        assert_eq!(wb.windows.len(), n0 + 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resizing_keeps_the_explorer_still() {
        let dir = std::env::temp_dir().join(format!("turbine-bounce-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..80 {
            std::fs::write(dir.join(format!("f{i:02}.txt")), "x").unwrap();
        }
        let dir = std::fs::canonicalize(&dir).unwrap();
        let mut wb = Workbench::new(&[dir.clone()]);
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(120, 36));
        wait_until(&mut wb, "listed", |wb| wb.shell.explorer.list.rows.len() > 70);
        wb.shell.explorer.list.select(40);
        let mut surf = Surface::new(120, 36);
        wb.render(&mut surf);
        let first = |wb: &Workbench| (wb.shell.explorer.list.scroll, wb.shell.explorer.list.selected);
        let before = first(&wb);
        // Dragging the side bar's edge a column at a time.
        let l = wb.layout();
        let edge = l.left.unwrap().right() - 1;
        let y = 10;
        let mut seen = vec![];
        wb.dispatch(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(edge, y), mods: Mods::NONE }));
        for dx in 1..12 {
            wb.dispatch(Event::Mouse(MouseEvent { kind: MouseKind::Drag(Button::Left), pos: Point::new(edge + dx, y), mods: Mods::NONE }));
            wb.render(&mut surf);
            seen.push((wb.layout().left.unwrap().w, first(&wb), surf.to_text().lines().nth(3).unwrap_or("").chars().take(40).collect::<String>()));
        }
        wb.dispatch(Event::Mouse(MouseEvent { kind: MouseKind::Up(Button::Left), pos: Point::new(edge + 11, y), mods: Mods::NONE }));
        // Resizing the whole window.
        for w in (90..120).rev().step_by(3) {
            wb.dispatch(Event::Resize(w, 36));
            wb.render(&mut surf);
            seen.push((wb.layout().left.unwrap().w, first(&wb), surf.to_text().lines().nth(3).unwrap_or("").chars().take(40).collect::<String>()));
        }
        let widths: Vec<i32> = seen[..11].iter().map(|s| s.0).collect();
        assert!(widths.windows(2).all(|w| w[1] >= w[0]), "the edge followed the pointer: {widths:?}");
        assert!(seen.iter().all(|s| s.1 == before), "the list stayed put: {before:?} {seen:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn too_many_tabs_overflow_into_a_list() {
        let dir = std::env::temp_dir().join(format!("turbine-tabs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<PathBuf> = (0..14).map(|i| {
            let p = dir.join(format!("file_number_{i:02}.txt"));
            std::fs::write(&p, "x").unwrap();
            p
        }).collect();
        let mut wb = Workbench::new(&files);
        wb.driver.cps = 0.0;
        wb.dispatch(Event::Resize(100, 30));
        let mut surf = Surface::new(100, 30);
        wb.render(&mut surf);
        let (r, n, _) = *wb.shell.tab_more.first().expect("an overflow button");
        assert!(n > 0);
        // The front editor's tab stays in view.
        let front = wb.windows[wb.editor_index().unwrap()].win.id;
        assert!(wb.shell.editor_tab_rects.iter().any(|t| t.0 == front));
        assert!(surf.to_text().contains(&format!("▼ {n}")));
        wb.dispatch(Event::Mouse(MouseEvent { kind: MouseKind::Down(Button::Left), pos: Point::new(r.x, r.y), mods: Mods::NONE }));
        assert!(wb.shell.tab_menu.is_some());
        // (It lists only the tabs that didn't fit.)
        assert_eq!(wb.shell.tab_menu.as_ref().unwrap().1.len(), n);
        keys(&mut wb, &["Home", "Enter"]);
        assert!(wb.shell.tab_menu.is_none());
        let w = &wb.windows[wb.editor_index().unwrap()];
        assert!(w.ed.doc.path.as_ref().unwrap().ends_with("file_number_00.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pictures_open_in_a_viewer_fitted_to_it() {
        let dir = std::env::temp_dir().join(format!("turbine-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("wide.png");
        image::RgbImage::new(400, 100).save(&p).unwrap();
        let mut wb = Workbench::new(&[]);
        wb.driver.cps = 0.0;
        wb.set_images_inline(true);
        wb.dispatch(Event::Resize(100, 30));
        wb.open_path(&p);
        assert_eq!(wb.windows.last().unwrap().special, Some("image"));
        wait_until(&mut wb, "its size", |wb| wb.viewers.values().any(|v| v.dims == Some((400, 100))));
        let mut surf = Surface::new(100, 30);
        wb.render(&mut surf);
        let (r, path) = surf.images.first().cloned().expect("a picture placed");
        assert_eq!(path, p);
        // 4:1 with cells twice as tall as wide: twice as many columns as rows... times 4.
        assert!((r.w as f64 / r.h as f64 - 8.0).abs() < 1.0, "{r:?}");
        assert!(surf.to_text().contains("400 × 100"));
        let _ = std::fs::remove_dir_all(&dir);
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
        wb.show_help("Test Guide", "# Title\n\nSome `code` here, [a link](#title).\n");
        assert_eq!(wb.windows.last().unwrap().win.title, "HELP: Test Guide");
        wait_until(&mut wb, "rendered", |wb| wb.windows.last().unwrap().ed.doc.line(2) == "Title" && !wb.windows.last().unwrap().ed.highlights.is_empty());
        assert_eq!(wb.windows.last().unwrap().ed.doc.line(4), "Some code here, ◄a link►.");
        assert!(wb.windows.last().unwrap().ed.help_look);
        run(&mut wb, json!({"cmd": "type", "text": "xyz"}));
        assert!(wb.windows.last().unwrap().ed.doc.line(2) == "Title", "typing doesn't change help");
        // Enter on the link goes to its heading.
        let w = wb.windows.last_mut().unwrap();
        w.ed.cursor = w.ed.doc.rope.line_to_char(4) + "Some code here, ◄a l".len();
        keys(&mut wb, &["Enter"]);
        assert_eq!(wb.windows.last().unwrap().ed.cursor_line_col().0, 2);
        // Alt+F1 goes back.
        keys(&mut wb, &["Alt+F1"]);
        assert_eq!(wb.windows.last().unwrap().ed.cursor_line_col().0, 4);
        wb.run_command("help.guides");
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
            ..Default::default()
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
        // (Enough code before the cursor for the model to go on.)
        for c in "fn greet(name: &str) { let n = 1; ".chars() {
            wb.key(KeyEvent::plain(Key::Char(c)));
        }
        wait_until(&mut wb, "ghost text", |wb| wb.windows.last().unwrap().ed.ghost.is_some());
        assert_eq!(wb.windows.last().unwrap().ed.ghost.as_deref(), Some("println!(\"hi\");"));
        wb.key(KeyEvent::plain(Key::Tab));
        assert_eq!(wb.windows.last().unwrap().ed.doc.line(0), "fn greet(name: &str) { let n = 1; println!(\"hi\");");
    }

    #[test]
    fn ctrl_k_rewrites_the_line_and_esc_puts_it_back() {
        let addr = fake_model_server();
        let mut wb = bench();
        wb.set_ai(Ai::with_models(vec![ai::Model {
            provider: "ollama-test".into(),
            name: "test-model".into(),
            base: format!("http://{addr}/v1"),
            key: None,
            loaded: true,
            fim: false,
            chat: true,
            ..Default::default()
        }]));
        wb.tab_complete = false;
        run(&mut wb, json!({"cmd": "type", "text": "let x = 1;\nkeep"}));
        keys(&mut wb, &["Up", "Ctrl+K"]);
        assert_eq!(wb.shell.focus, Focus::Inline);
        run(&mut wb, json!({"cmd": "type", "text": "say hello"}));
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "the rewrite", |wb| matches!(wb.inline.as_ref().map(|i| &i.phase), Some(inline_ai::Phase::Review)));
        let text = |wb: &mut Workbench| run(wb, json!({"cmd": "text"}))["text"].as_str().unwrap().to_string();
        assert_eq!(text(&mut wb), "Hello world\nkeep");
        // Esc: the old line back, in one step.
        keys(&mut wb, &["Esc"]);
        assert_eq!(text(&mut wb), "let x = 1;\nkeep");
        assert!(wb.inline.is_none());
        // Again, and Enter keeps it.
        keys(&mut wb, &["Ctrl+Home", "Ctrl+K"]);
        run(&mut wb, json!({"cmd": "type", "text": "say hello"}));
        keys(&mut wb, &["Enter"]);
        wait_until(&mut wb, "the rewrite", |wb| matches!(wb.inline.as_ref().map(|i| &i.phase), Some(inline_ai::Phase::Review)));
        keys(&mut wb, &["Enter"]);
        assert_eq!(text(&mut wb), "Hello world\nkeep");
        assert!(wb.windows.last().unwrap().ed.found.is_empty(), "no highlight left");
        // The chat's mode: Ctrl+. switches Ask to Agent.
        keys(&mut wb, &["Ctrl+L", "Ctrl+."]);
        assert!(wb.shell.chat_agent);
        assert_eq!(wb.tree().find_target("chat.mode").map(|n| n.name.clone()).as_deref(), Some("Mode: Agent"));
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
            ..Default::default()
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
        // (The window alone, as the original showed it: no side bar or tabs.)
        wb.run_command("view.sidebar");
        wb.run_command("view.editor_tabs");
        wb.run_command("view.breadcrumbs");
        wb.dispatch(Event::Resize(80, 25));
        run(&mut wb, json!({"cmd": "menu", "path": "File/Preferences/Control Style..."}));
        run(&mut wb, json!({"cmd": "click", "target": "listitem:QBasic / MS-DOS Edit"}));
        run(&mut wb, json!({"cmd": "click", "target": "button:OK"}));
        assert_eq!(wb.theme.scheme.name, "QBasic");
        let mut surf = Surface::new(80, 25);
        wb.render(&mut surf);
        let text = surf.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].trim_end().ends_with("Help"), "Help at the right: {:?}", lines[0]);
        assert!(lines[1].starts_with("┌") && lines[1].contains(" Untitled ") && lines[1].contains("┤↑├"), "{:?}", lines[1]);
        assert!(lines[2].starts_with("│") && !lines[2].contains(" 1 "), "no line numbers: {:?}", lines[2]);
        // (VS Code's items in the status line, QBasic's │ between them.)
        assert!(lines[24].contains("│ Ln 1, Col 1") && lines[24].contains("│ UTF-8"), "{:?}", lines[24]);
        assert!(text.contains('↓') && text.contains('→'), "arrow scroll bars");
        // The title is in reverse video (blue on grey).
        let x = lines[1].find(" Untitled ").map(|b| lines[1][..b].chars().count()).unwrap() as i32 + 1;
        assert_eq!(surf.get(x, 1).unwrap().style.bg, turbine_ui::theme::vga::LIGHT_GRAY);
    }

    #[test]
    fn vbdos_style_has_title_bars() {
        let mut wb = bench();
        // (The window alone, as the original showed it: no side bar or tabs.)
        wb.run_command("view.sidebar");
        wb.run_command("view.editor_tabs");
        wb.run_command("view.breadcrumbs");
        wb.dispatch(Event::Resize(80, 25));
        wb.apply_style(turbine_ui::theme::ControlStyle::vbdos());
        assert_eq!(wb.theme.scheme.name, "VB-DOS");
        let mut surf = Surface::new(80, 25);
        wb.render(&mut surf);
        let text = surf.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[1].contains("[1] Untitled") && lines[1].contains('▼') && lines[1].contains('▲'), "{:?}", lines[1]);
        assert_eq!(surf.get(3, 1).unwrap().style.bg, turbine_ui::theme::vga::MAGENTA);
        // The status line: VS Code's items.
        assert!(lines[24].contains("Ln 1, Col 1") && lines[24].contains("Plain Text"), "{:?}", lines[24]);
        // The control box at the left of the title bar; no black edge column.
        assert_eq!(lines[1].chars().next(), Some('■'));
        assert_eq!(surf.get(0, 5).unwrap().style.bg, turbine_ui::theme::vga::BLUE);
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
