//! Turbine's extension host. Extensions are WebAssembly components made to
//! `wit/turbine.wit` (the `extension` world), run with wasmtime on a worker
//! thread of their own.
//!
//! An extension calls the host (to read or edit a document, add a menu
//! item, show a message) through [`HostCall`]s the editor answers on its UI
//! thread when it next looks (`ExtHost::poll`): the extension's thread waits
//! for the answer, the UI never waits for an extension.
//!
//! Extensions live in folders: `<dir>/<name>/extension.wasm`, with an
//! optional `README.md` whose first line describes it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

wasmtime::component::bindgen!({
    path: "../../wit/turbine.wit",
    world: "extension",
});

use turbine::ext::types::{MenuItem, Position, Range};

/// What an extension asks of the editor.
#[derive(Debug)]
pub enum Request {
    Log(String),
    Status(String),
    ActiveDocument,
    DocumentText(u32),
    DocumentPath(u32),
    DocumentLanguage(u32),
    /// Replace a range (lines and columns in chars) with text.
    Edit { doc: u32, start: (u32, u32), end: (u32, u32), text: String },
    Cursor(u32),
    AddMenuItem { menu: String, label: String, shortcut: Option<String>, command: String },
    BindKey { chord: String, command: String, when: Option<String> },
    RunCommand { command: String, args: String },
    Setting(String),
    /// Not a request: a command the editor asked the extension to run has
    /// finished (or the extension failed to load: `command` empty).
    Finished { command: String, result: Result<String, String> },
}

/// The editor's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    None,
    Doc(Option<u32>),
    Text(String),
    MaybeText(Option<String>),
    Pos(u32, u32),
    Result(Result<String, String>),
}

/// One call from an extension, to answer with `reply`.
pub struct HostCall {
    /// The extension's name.
    pub ext: String,
    pub request: Request,
    reply: Option<Sender<Answer>>,
}

impl HostCall {
    /// Answer it (calls that need no answer ignore this).
    pub fn reply(self, a: Answer) {
        if let Some(r) = self.reply {
            let _ = r.send(a);
        }
    }
}

/// An installed extension, as found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtInfo {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    /// Why it didn't load.
    pub error: Option<String>,
    pub loaded: bool,
}

/// The per-instance state the generated bindings call into.
struct HostState {
    ext: String,
    tx: Sender<HostCall>,
}

impl HostState {
    /// Ask the editor and wait for its answer.
    fn ask(&self, request: Request) -> Answer {
        let (rtx, rrx) = channel();
        if self.tx.send(HostCall { ext: self.ext.clone(), request, reply: Some(rtx) }).is_err() {
            return Answer::None;
        }
        rrx.recv().unwrap_or(Answer::None)
    }
    fn tell(&self, request: Request) {
        let _ = self.tx.send(HostCall { ext: self.ext.clone(), request, reply: None });
    }
}

impl turbine::ext::types::Host for HostState {}

impl turbine::ext::host::Host for HostState {
    fn log(&mut self, message: String) {
        self.tell(Request::Log(message));
    }
    fn status(&mut self, message: String) {
        self.tell(Request::Status(message));
    }
    fn active_document(&mut self) -> Option<u32> {
        match self.ask(Request::ActiveDocument) {
            Answer::Doc(d) => d,
            _ => None,
        }
    }
    fn document_text(&mut self, doc: u32) -> String {
        match self.ask(Request::DocumentText(doc)) {
            Answer::Text(t) => t,
            _ => String::new(),
        }
    }
    fn document_path(&mut self, doc: u32) -> Option<String> {
        match self.ask(Request::DocumentPath(doc)) {
            Answer::MaybeText(t) => t,
            _ => None,
        }
    }
    fn document_language(&mut self, doc: u32) -> Option<String> {
        match self.ask(Request::DocumentLanguage(doc)) {
            Answer::MaybeText(t) => t,
            _ => None,
        }
    }
    fn edit(&mut self, doc: u32, at: Range, text: String) {
        self.ask(Request::Edit { doc, start: (at.start.line, at.start.column), end: (at.end.line, at.end.column), text });
    }
    fn cursor(&mut self, doc: u32) -> Position {
        match self.ask(Request::Cursor(doc)) {
            Answer::Pos(line, column) => Position { line, column },
            _ => Position { line: 0, column: 0 },
        }
    }
    fn add_menu_item(&mut self, menu_path: String, item: MenuItem) {
        self.tell(Request::AddMenuItem { menu: menu_path, label: item.label, shortcut: item.shortcut, command: item.command });
    }
    fn bind_key(&mut self, chord: String, command: String, when: Option<String>) {
        self.tell(Request::BindKey { chord, command, when });
    }
    fn run_command(&mut self, command: String, args: String) -> Result<String, String> {
        match self.ask(Request::RunCommand { command, args }) {
            Answer::Result(r) => r,
            _ => Err("no answer".into()),
        }
    }
    fn setting(&mut self, key: String) -> Option<String> {
        match self.ask(Request::Setting(key)) {
            Answer::MaybeText(t) => t,
            _ => None,
        }
    }
}

/// Work for the extensions' thread.
enum Job {
    Run { ext: String, command: String, args: String },
}

/// The running extensions.
pub struct ExtHost {
    calls: Receiver<HostCall>,
    jobs: Sender<Job>,
    infos: Arc<Mutex<Vec<ExtInfo>>>,
}

/// Turn a core WebAssembly module built with wit-bindgen (for
/// wasm32-unknown-unknown) into a component Turbine can load.
pub fn pack(core: &[u8]) -> anyhow::Result<Vec<u8>> {
    wit_component::ComponentEncoder::default().module(core)?.validate(true).encode()
}

/// The extensions in these folders (each `<name>/extension.wasm`).
pub fn find(dirs: &[PathBuf]) -> Vec<ExtInfo> {
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let wasm = e.path().join("extension.wasm");
            if !wasm.is_file() {
                continue;
            }
            let description = std::fs::read_to_string(e.path().join("README.md"))
                .ok()
                .and_then(|t| t.lines().map(|l| l.trim_start_matches('#').trim().to_string()).find(|l| !l.is_empty()))
                .unwrap_or_default();
            out.push(ExtInfo { name: e.file_name().to_string_lossy().into_owned(), description, path: wasm, error: None, loaded: false });
        }
    }
    out
}

impl ExtHost {
    /// Load the extensions in `dirs` on a thread of their own and call
    /// each one's `activate`.
    pub fn start(dirs: Vec<PathBuf>) -> ExtHost {
        let (ctx, calls) = channel();
        let (jobs, jrx) = channel::<Job>();
        let infos = Arc::new(Mutex::new(Vec::new()));
        let shared = infos.clone();
        std::thread::Builder::new()
            .name("turbine-ext".into())
            .spawn(move || {
                let mut config = Config::new();
                config.wasm_component_model(true);
                let engine = match Engine::new(&config) {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = ctx.send(HostCall { ext: String::new(), request: Request::Finished { command: String::new(), result: Err(e.to_string()) }, reply: None });
                        return;
                    }
                };
                let mut linker: Linker<HostState> = Linker::new(&engine);
                if let Err(e) = Extension::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |s| s) {
                    let _ = ctx.send(HostCall { ext: String::new(), request: Request::Finished { command: String::new(), result: Err(e.to_string()) }, reply: None });
                    return;
                }
                let mut running: Vec<(String, Store<HostState>, Extension)> = Vec::new();
                let found = find(&dirs);
                *shared.lock().unwrap() = found.clone();
                for info in found {
                    let loaded = load(&engine, &linker, &info.path, &info.name, ctx.clone());
                    let mut list = shared.lock().unwrap();
                    let entry = list.iter_mut().find(|i| i.path == info.path).unwrap();
                    match loaded {
                        Ok(r) => {
                            entry.loaded = true;
                            running.push(r);
                        }
                        Err(e) => {
                            entry.error = Some(format!("{e:#}"));
                            let _ = ctx.send(HostCall {
                                ext: info.name.clone(),
                                request: Request::Finished { command: String::new(), result: Err(format!("{e:#}")) },
                                reply: None,
                            });
                        }
                    }
                }
                while let Ok(job) = jrx.recv() {
                    match job {
                        Job::Run { ext, command, args } => {
                            let result = match running.iter_mut().find(|(n, ..)| *n == ext) {
                                Some((_, store, inst)) => inst.call_run(&mut *store, &command, &args).unwrap_or_else(|e| Err(format!("the extension failed: {e:#}"))),
                                None => Err(format!("no extension {ext}")),
                            };
                            let _ = ctx.send(HostCall { ext, request: Request::Finished { command, result }, reply: None });
                        }
                    }
                }
            })
            .expect("extension thread");
        ExtHost { calls, jobs, infos }
    }

    /// The extensions' calls waiting for the editor.
    pub fn poll(&self) -> Vec<HostCall> {
        self.calls.try_iter().collect()
    }

    /// Run an extension's command (its answer comes as a Finished call).
    pub fn run(&self, ext: &str, command: &str, args: &str) {
        let _ = self.jobs.send(Job::Run { ext: ext.into(), command: command.into(), args: args.into() });
    }

    /// The extensions found, and whether each loaded.
    pub fn list(&self) -> Vec<ExtInfo> {
        self.infos.lock().unwrap().clone()
    }
}

fn load(engine: &Engine, linker: &Linker<HostState>, path: &Path, name: &str, tx: Sender<HostCall>) -> wasmtime::Result<(String, Store<HostState>, Extension)> {
    let component = Component::from_file(engine, path)?;
    let mut store = Store::new(engine, HostState { ext: name.to_string(), tx });
    let inst = Extension::instantiate(&mut store, &component, linker)?;
    inst.call_activate(&mut store)?;
    Ok((name.to_string(), store, inst))
}
