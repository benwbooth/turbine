//! A language server client that never blocks: requests return an id at
//! once, and answers, diagnostics and the server's death come back as
//! [`LspEvent`]s from [`LspClient::poll`] on the UI's ticks.

use crate::rpc::{Incoming, RpcProcess, on_path};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Which server to run for a language: the first one installed.
pub fn server_for(lang: &str) -> Option<Vec<String>> {
    candidates(lang).iter().find(|c| on_path(c[0])).map(|c| c.iter().map(|s| s.to_string()).collect())
}

/// The language servers Turbine knows for a language (their programs).
pub fn server_names(lang: &str) -> Vec<&'static str> {
    candidates(lang).iter().map(|c| c[0]).collect()
}

fn candidates(lang: &str) -> &'static [&'static [&'static str]] {
    let candidates: &[&[&str]] = match lang {
        "bash" => &[&["bash-language-server", "start"]],
        "python" => &[&["basedpyright-langserver", "--stdio"], &["pyright-langserver", "--stdio"], &["pylsp"]],
        "javascript" | "typescript" | "tsx" => &[&["typescript-language-server", "--stdio"]],
        "rust" => &[&["rust-analyzer"]],
        "go" => &[&["gopls"]],
        "c" | "cpp" => &[&["clangd"]],
        "java" => &[&["jdtls"]],
        "ruby" => &[&["ruby-lsp"], &["solargraph", "stdio"]],
        "lua" => &[&["lua-language-server"]],
        "json" => &[&["vscode-json-language-server", "--stdio"], &["vscode-json-languageserver", "--stdio"]],
        "yaml" => &[&["yaml-language-server", "--stdio"]],
        "toml" => &[&["taplo", "lsp", "stdio"]],
        "html" => &[&["vscode-html-language-server", "--stdio"]],
        "css" => &[&["vscode-css-language-server", "--stdio"]],
        "markdown" => &[&["marksman", "server"]],
        "nix" => &[&["nil"], &["nixd"]],
        "zig" => &[&["zls"]],
        "haskell" => &[&["haskell-language-server-wrapper", "--lsp"]],
        "ocaml" => &[&["ocamllsp"]],
        "elixir" => &[&["elixir-ls"]],
        "kotlin" => &[&["kotlin-language-server"]],
        "c-sharp" => &[&["csharp-ls"], &["OmniSharp", "-lsp"]],
        "php" => &[&["intelephense", "--stdio"]],
        "swift" => &[&["sourcekit-lsp"]],
        "dart" => &[&["dart", "language-server"]],
        "qml" => &[&["qmlls"]],
        _ => &[],
    };
    candidates
}

/// How to install a language's server: (server, shell command). With Nix
/// on the PATH it's a `nix profile install`, else the server's own way.
pub fn install_command(lang: &str) -> Option<(&'static str, String)> {
    // (server, nixpkgs attribute, the other way)
    let (server, nixpkg, other): (&str, &str, &str) = match lang {
        "bash" => ("bash-language-server", "bash-language-server", "npm install -g bash-language-server"),
        "python" => ("basedpyright", "basedpyright", "pip install --user basedpyright"),
        "javascript" | "typescript" | "tsx" => ("typescript-language-server", "typescript-language-server", "npm install -g typescript-language-server typescript"),
        "rust" => ("rust-analyzer", "rust-analyzer", "rustup component add rust-analyzer"),
        "go" => ("gopls", "gopls", "go install golang.org/x/tools/gopls@latest"),
        "c" | "cpp" => ("clangd", "clang-tools", "sudo apt install clangd"),
        "java" => ("jdtls", "jdt-language-server", ""),
        "ruby" => ("ruby-lsp", "rubyPackages.ruby-lsp", "gem install ruby-lsp"),
        "lua" => ("lua-language-server", "lua-language-server", ""),
        "json" | "html" | "css" => ("vscode-langservers-extracted", "vscode-langservers-extracted", "npm install -g vscode-langservers-extracted"),
        "yaml" => ("yaml-language-server", "yaml-language-server", "npm install -g yaml-language-server"),
        "toml" => ("taplo", "taplo", "cargo install taplo-cli --locked --features lsp"),
        "markdown" => ("marksman", "marksman", ""),
        "nix" => ("nil", "nil", ""),
        "zig" => ("zls", "zls", ""),
        "haskell" => ("haskell-language-server", "haskell-language-server", "ghcup install hls"),
        "ocaml" => ("ocaml-lsp", "ocamlPackages.ocaml-lsp", "opam install ocaml-lsp-server"),
        "elixir" => ("elixir-ls", "elixir-ls", ""),
        "kotlin" => ("kotlin-language-server", "kotlin-language-server", ""),
        "c-sharp" => ("csharp-ls", "csharp-ls", "dotnet tool install --global csharp-ls"),
        "php" => ("intelephense", "intelephense", "npm install -g intelephense"),
        "dart" => ("dart", "dart", ""),
        "qml" => ("qmlls", "kdePackages.qtdeclarative", ""),
        _ => return None,
    };
    let has_nix = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("nix").is_file()));
    if has_nix {
        Some((server, format!("nix profile install nixpkgs#{nixpkg}")))
    } else if !other.is_empty() {
        Some((server, other.to_string()))
    } else {
        None
    }
}

/// The LSP language id for one of Turbine's language names.
pub fn language_id(lang: &str) -> &str {
    match lang {
        "bash" => "shellscript",
        "tsx" => "typescriptreact",
        "c-sharp" => "csharp",
        l => l,
    }
}

/// The project folder a file belongs to: the nearest folder up from it with
/// a version-control or build marker, else the file's own folder.
pub fn project_root(file: &Path) -> PathBuf {
    const MARKERS: &[&str] = &[
        ".git", "Cargo.toml", "package.json", "go.mod", "pyproject.toml", "setup.py", "flake.nix", "CMakeLists.txt",
        "Makefile", "build.gradle", "pom.xml", "Gemfile", "mix.exs", "deno.json", ".hg",
    ];
    let start = file.parent().unwrap_or(Path::new("."));
    let mut dir = Some(start);
    // The outermost Cargo workspace wins over a member crate.
    let mut found: Option<PathBuf> = None;
    while let Some(d) = dir {
        if MARKERS.iter().any(|m| d.join(m).exists()) {
            let is_git = d.join(".git").exists();
            found = Some(d.to_path_buf());
            if is_git {
                break;
            }
        }
        dir = d.parent();
    }
    found.unwrap_or_else(|| start.to_path_buf())
}

pub fn path_to_uri(p: &Path) -> String {
    let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8_lossy(&out).into_owned();
    // file:///C:/x on Windows.
    let s = if s.len() > 2 && s.as_bytes()[2] == b':' { s[1..].to_string() } else { s };
    Some(PathBuf::from(s))
}

/// UTF-16 column of a byte offset in a line (what LSP counts).
pub fn utf16_col(line: &str, byte: usize) -> u32 {
    line[..byte.min(line.len())].encode_utf16().count() as u32
}

/// Byte offset in a line of a UTF-16 column.
pub fn byte_of_utf16(line: &str, col: u32) -> usize {
    let mut n = 0u32;
    for (i, c) in line.char_indices() {
        if n >= col {
            return i;
        }
        n += c.len_utf16() as u32;
    }
    line.len()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    /// UTF-16 units.
    pub character: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub start: Pos,
    pub end: Pos,
    /// 1 error, 2 warning, 3 information, 4 hint.
    pub severity: u8,
    pub message: String,
    pub source: String,
    /// As the server sent it (code actions get it back as context).
    pub raw: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    /// Text to insert (replacing the word before the cursor).
    pub insert: String,
    pub detail: String,
    pub kind: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub path: PathBuf,
    pub start: Pos,
}

/// Edits to one file: (start, end, new text).
pub type TextEdits = Vec<(Pos, Pos, String)>;
/// Edits to several files.
pub type WorkspaceEdit = Vec<(PathBuf, TextEdits)>;

/// A quick fix or refactoring the server offers.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeAction {
    pub title: String,
    pub kind: String,
    pub preferred: bool,
    pub edit: Option<WorkspaceEdit>,
    /// A command to run on the server (after the edit, if any).
    pub command: Option<Value>,
    /// The action as sent (to resolve it when it came without its edit).
    pub raw: Value,
}

/// Signature help: the call's signature, and the active parameter's byte
/// range in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    pub label: String,
    pub active: Option<(usize, usize)>,
    pub doc: String,
}

/// A symbol anywhere in the project.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: u8,
    pub container: String,
    pub path: PathBuf,
    pub start: Pos,
}

#[derive(Debug)]
pub enum LspEvent {
    Ready,
    Diagnostics { path: PathBuf, items: Vec<Diagnostic> },
    Completion { id: i64, items: Vec<CompletionItem> },
    Hover { id: i64, text: String },
    Definition { id: i64, locations: Vec<Location> },
    References { id: i64, locations: Vec<Location> },
    /// A rename's edits: per file, (start, end, new text), in the order given.
    Rename { id: i64, edits: Vec<(PathBuf, Vec<(Pos, Pos, String)>)> },
    Formatting { id: i64, edits: TextEdits },
    CodeActions { id: i64, actions: Vec<CodeAction> },
    CodeActionResolved { id: i64, action: CodeAction },
    Signature { id: i64, sig: Option<Signature> },
    /// Inlay hints: (where, text, padding before, padding after).
    InlayHints { id: i64, hints: Vec<(Pos, String, bool, bool)> },
    /// Semantic tokens: (line, start, length in UTF-16 units, token type).
    SemanticTokens { id: i64, tokens: Vec<(u32, u32, u32, String)> },
    WorkspaceSymbols { id: i64, symbols: Vec<WorkspaceSymbol> },
    /// The server asks to apply edits (after a command it ran).
    ApplyEdit { edits: WorkspaceEdit },
    /// A request failed (or got an error back).
    Failed { id: i64, message: String },
    Log(String),
    Exited(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Initialize,
    Completion,
    Hover,
    Definition,
    References,
    Rename,
    Formatting,
    CodeAction,
    CodeActionResolve,
    SignatureHelp,
    InlayHint,
    SemanticTokens,
    WorkspaceSymbol,
    Other,
}

pub struct LspClient {
    rpc: RpcProcess,
    pub root: PathBuf,
    pub lang: String,
    next_id: i64,
    pending: HashMap<i64, Kind>,
    ready: bool,
    /// Notifications waiting for `initialized`.
    queued: Vec<Value>,
    open: HashMap<String, i64>,
    dead: Option<String>,
    /// The semantic token types the server uses (from its capabilities).
    legend: Vec<String>,
    /// What the server said it can do.
    pub capabilities: Value,
}

impl LspClient {
    pub fn start(argv: &[String], root: &Path, lang: &str) -> std::io::Result<LspClient> {
        Ok(Self::with_rpc(RpcProcess::spawn(argv, Some(root))?, root, lang))
    }

    /// A client over an already connected server.
    pub fn with_rpc(rpc: RpcProcess, root: &Path, lang: &str) -> LspClient {
        let mut c = LspClient {
            rpc,
            root: root.to_path_buf(),
            lang: lang.into(),
            next_id: 1,
            pending: HashMap::new(),
            ready: false,
            queued: Vec::new(),
            open: HashMap::new(),
            dead: None,
            legend: Vec::new(),
            capabilities: Value::Null,
        };
        let root_uri = path_to_uri(root);
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        c.request_kind(
            Kind::Initialize,
            "initialize",
            json!({
                "processId": std::process::id(),
                "clientInfo": {"name": "Turbine", "version": env!("CARGO_PKG_VERSION")},
                "rootUri": root_uri,
                "workspaceFolders": [{"uri": root_uri, "name": name}],
                "capabilities": {
                    "general": {"positionEncodings": ["utf-16"]},
                    "textDocument": {
                        "synchronization": {"didSave": true, "dynamicRegistration": false},
                        "publishDiagnostics": {"relatedInformation": false},
                        "completion": {"completionItem": {"snippetSupport": false, "documentationFormat": ["plaintext", "markdown"]}},
                        "hover": {"contentFormat": ["plaintext", "markdown"]},
                        "definition": {"linkSupport": false},
                        "references": {},
                        "rename": {"prepareSupport": false},
                        "formatting": {},
                        "codeAction": {
                            "codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"]}},
                            "resolveSupport": {"properties": ["edit"]},
                            "dataSupport": true,
                            "isPreferredSupport": true
                        },
                        "signatureHelp": {"signatureInformation": {"documentationFormat": ["plaintext", "markdown"], "parameterInformation": {"labelOffsetSupport": true}}},
                        "inlayHint": {},
                        "semanticTokens": {
                            "requests": {"full": true},
                            "tokenTypes": ["namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter", "variable", "property", "enumMember", "event", "function", "method", "macro", "keyword", "modifier", "comment", "string", "number", "regexp", "operator", "decorator"],
                            "tokenModifiers": [],
                            "formats": ["relative"]
                        }
                    },
                    "workspace": {"workspaceFolders": true, "configuration": false, "applyEdit": true, "symbol": {}, "workspaceEdit": {"documentChanges": true}}
                }
            }),
        );
        c
    }

    pub fn name(&self) -> &str {
        &self.rpc.name
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn dead(&self) -> Option<&str> {
        self.dead.as_deref()
    }

    pub fn is_open(&self, path: &Path) -> bool {
        self.open.contains_key(&path_to_uri(path))
    }

    fn request_kind(&mut self, kind: Kind, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, kind);
        self.rpc.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        if self.ready {
            self.rpc.send(msg);
        } else {
            self.queued.push(msg);
        }
    }

    pub fn did_open(&mut self, path: &Path, version: i64, text: &str) {
        let uri = path_to_uri(path);
        self.open.insert(uri.clone(), version);
        let lang = language_id(&self.lang).to_string();
        self.notify("textDocument/didOpen", json!({"textDocument": {"uri": uri, "languageId": lang, "version": version, "text": text}}));
    }

    pub fn did_change(&mut self, path: &Path, version: i64, text: &str) {
        let uri = path_to_uri(path);
        if self.open.get(&uri) == Some(&version) {
            return;
        }
        self.open.insert(uri.clone(), version);
        self.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
        );
    }

    pub fn did_save(&mut self, path: &Path) {
        self.notify("textDocument/didSave", json!({"textDocument": {"uri": path_to_uri(path)}}));
    }

    pub fn did_close(&mut self, path: &Path) {
        let uri = path_to_uri(path);
        if self.open.remove(&uri).is_some() {
            self.notify("textDocument/didClose", json!({"textDocument": {"uri": uri}}));
        }
    }

    fn at(path: &Path, pos: Pos) -> Value {
        json!({"textDocument": {"uri": path_to_uri(path)}, "position": {"line": pos.line, "character": pos.character}})
    }

    pub fn completion(&mut self, path: &Path, pos: Pos) -> i64 {
        self.request_kind(Kind::Completion, "textDocument/completion", Self::at(path, pos))
    }

    pub fn hover(&mut self, path: &Path, pos: Pos) -> i64 {
        self.request_kind(Kind::Hover, "textDocument/hover", Self::at(path, pos))
    }

    pub fn definition(&mut self, path: &Path, pos: Pos) -> i64 {
        self.request_kind(Kind::Definition, "textDocument/definition", Self::at(path, pos))
    }

    /// Requests sent and not answered yet.
    pub fn references(&mut self, path: &Path, pos: Pos) -> i64 {
        let mut p = Self::at(path, pos);
        p["context"] = json!({"includeDeclaration": true});
        self.request_kind(Kind::References, "textDocument/references", p)
    }

    pub fn rename(&mut self, path: &Path, pos: Pos, new_name: &str) -> i64 {
        let mut p = Self::at(path, pos);
        p["newName"] = json!(new_name);
        self.request_kind(Kind::Rename, "textDocument/rename", p)
    }

    pub fn formatting(&mut self, path: &Path, tab_size: u32, spaces: bool) -> i64 {
        let p = json!({"textDocument": {"uri": path_to_uri(path)}, "options": {"tabSize": tab_size, "insertSpaces": spaces}});
        self.request_kind(Kind::Formatting, "textDocument/formatting", p)
    }

    /// Actions for a range, with the diagnostics there as context.
    pub fn code_actions(&mut self, path: &Path, start: Pos, end: Pos, diagnostics: Vec<Value>) -> i64 {
        let p = json!({
            "textDocument": {"uri": path_to_uri(path)},
            "range": {"start": {"line": start.line, "character": start.character}, "end": {"line": end.line, "character": end.character}},
            "context": {"diagnostics": diagnostics}
        });
        self.request_kind(Kind::CodeAction, "textDocument/codeAction", p)
    }

    pub fn resolve_code_action(&mut self, raw: &Value) -> i64 {
        self.request_kind(Kind::CodeActionResolve, "codeAction/resolve", raw.clone())
    }

    /// Run a command on the server (it may answer with workspace/applyEdit).
    pub fn execute_command(&mut self, command: &Value) {
        let p = json!({"command": command["command"], "arguments": command.get("arguments").cloned().unwrap_or(json!([]))});
        self.request_kind(Kind::Other, "workspace/executeCommand", p);
    }

    pub fn signature_help(&mut self, path: &Path, pos: Pos) -> i64 {
        self.request_kind(Kind::SignatureHelp, "textDocument/signatureHelp", Self::at(path, pos))
    }

    pub fn inlay_hints(&mut self, path: &Path, from_line: u32, to_line: u32) -> i64 {
        let p = json!({"textDocument": {"uri": path_to_uri(path)}, "range": {"start": {"line": from_line, "character": 0}, "end": {"line": to_line, "character": 0}}});
        self.request_kind(Kind::InlayHint, "textDocument/inlayHint", p)
    }

    /// Semantic tokens, when the server has them.
    pub fn semantic_tokens(&mut self, path: &Path) -> Option<i64> {
        if self.legend.is_empty() {
            return None;
        }
        Some(self.request_kind(Kind::SemanticTokens, "textDocument/semanticTokens/full", json!({"textDocument": {"uri": path_to_uri(path)}})))
    }

    pub fn workspace_symbols(&mut self, query: &str) -> i64 {
        self.request_kind(Kind::WorkspaceSymbol, "workspace/symbol", json!({"query": query}))
    }

    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    pub fn shutdown(&mut self) {
        self.request_kind(Kind::Other, "shutdown", Value::Null);
        self.rpc.send(json!({"jsonrpc": "2.0", "method": "exit"}));
    }

    pub fn poll(&mut self) -> Vec<LspEvent> {
        let mut evs = Vec::new();
        for inc in self.rpc.incoming() {
            match inc {
                Incoming::Log(l) => evs.push(LspEvent::Log(l)),
                Incoming::Exited(why) => {
                    self.dead = Some(why.clone());
                    for (id, _) in self.pending.drain() {
                        evs.push(LspEvent::Failed { id, message: format!("{} exited", self.rpc.name) });
                    }
                    evs.push(LspEvent::Exited(why));
                }
                Incoming::Message(m) => self.message(m, &mut evs),
            }
        }
        evs
    }

    fn message(&mut self, m: Value, evs: &mut Vec<LspEvent>) {
        // A request from the server: answer the ones that matter, refuse the rest.
        if let (Some(method), Some(id)) = (m.get("method").and_then(Value::as_str), m.get("id")) {
            let result = match method {
                "workspace/configuration" => {
                    let n = m["params"]["items"].as_array().map(|a| a.len()).unwrap_or(0);
                    Value::Array(vec![Value::Null; n])
                }
                "window/workDoneProgress/create" | "client/registerCapability" | "client/unregisterCapability" => Value::Null,
                "workspace/applyEdit" => {
                    evs.push(LspEvent::ApplyEdit { edits: workspace_edit(&m["params"]["edit"]) });
                    json!({"applied": true})
                }
                "workspace/workspaceFolders" => {
                    json!([{"uri": path_to_uri(&self.root), "name": self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()}])
                }
                _ => {
                    self.rpc.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "not supported"}}));
                    return;
                }
            };
            self.rpc.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
            return;
        }
        if let Some(method) = m.get("method").and_then(Value::as_str) {
            match method {
                "textDocument/publishDiagnostics" => {
                    let p = &m["params"];
                    let Some(path) = p["uri"].as_str().and_then(uri_to_path) else { return };
                    let items = p["diagnostics"].as_array().map(|a| a.iter().map(diagnostic).collect()).unwrap_or_default();
                    evs.push(LspEvent::Diagnostics { path, items });
                }
                "window/logMessage" | "window/showMessage" => {
                    if let Some(t) = m["params"]["message"].as_str() {
                        evs.push(LspEvent::Log(t.to_string()));
                    }
                }
                _ => {}
            }
            return;
        }
        let Some(id) = m.get("id").and_then(Value::as_i64) else { return };
        let Some(kind) = self.pending.remove(&id) else { return };
        if let Some(e) = m.get("error") {
            if kind == Kind::Initialize {
                self.dead = Some(format!("initialize failed: {e}"));
            }
            evs.push(LspEvent::Failed { id, message: e["message"].as_str().unwrap_or("error").to_string() });
            return;
        }
        let r = &m["result"];
        match kind {
            Kind::Initialize => {
                self.ready = true;
                self.capabilities = r["capabilities"].clone();
                self.legend = r["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"]
                    .as_array()
                    .map(|a| a.iter().map(|t| t.as_str().unwrap_or("").to_string()).collect())
                    .unwrap_or_default();
                self.rpc.send(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
                for q in std::mem::take(&mut self.queued) {
                    self.rpc.send(q);
                }
                evs.push(LspEvent::Ready);
            }
            Kind::Completion => {
                let list = r.get("items").unwrap_or(r);
                let mut items: Vec<(String, CompletionItem)> = list
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|it| {
                                let label = it["label"].as_str().unwrap_or("").to_string();
                                let insert = it["textEdit"]["newText"]
                                    .as_str()
                                    .or_else(|| it["insertText"].as_str())
                                    .unwrap_or(&label)
                                    .to_string();
                                let sort = it["sortText"].as_str().unwrap_or(&label).to_string();
                                let item = CompletionItem {
                                    label: label.clone(),
                                    insert,
                                    detail: it["detail"].as_str().unwrap_or("").to_string(),
                                    kind: it["kind"].as_u64().unwrap_or(0) as u8,
                                };
                                (sort, item)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                items.sort_by(|a, b| a.0.cmp(&b.0));
                evs.push(LspEvent::Completion { id, items: items.into_iter().map(|(_, i)| i).collect() });
            }
            Kind::Hover => evs.push(LspEvent::Hover { id, text: hover_text(&r["contents"]) }),
            Kind::Definition => {
                let arr = if r.is_array() { r.as_array().cloned().unwrap_or_default() } else if r.is_null() { vec![] } else { vec![r.clone()] };
                let locations = arr
                    .iter()
                    .filter_map(|l| {
                        let uri = l["uri"].as_str().or_else(|| l["targetUri"].as_str())?;
                        let range = if l.get("range").is_some() { &l["range"] } else { &l["targetSelectionRange"] };
                        Some(Location { path: uri_to_path(uri)?, start: pos(&range["start"]) })
                    })
                    .collect();
                evs.push(LspEvent::Definition { id, locations });
            }
            Kind::References => {
                let locations = r
                    .as_array()
                    .map(|a| a.iter().filter_map(|l| Some(Location { path: uri_to_path(l["uri"].as_str()?)?, start: pos(&l["range"]["start"]) })).collect())
                    .unwrap_or_default();
                evs.push(LspEvent::References { id, locations });
            }
            Kind::Rename => evs.push(LspEvent::Rename { id, edits: workspace_edit(r) }),
            Kind::Formatting => evs.push(LspEvent::Formatting { id, edits: text_edits(r) }),
            Kind::CodeAction => {
                let actions = r.as_array().map(|a| a.iter().map(code_action).collect()).unwrap_or_default();
                evs.push(LspEvent::CodeActions { id, actions });
            }
            Kind::CodeActionResolve => evs.push(LspEvent::CodeActionResolved { id, action: code_action(r) }),
            Kind::SignatureHelp => evs.push(LspEvent::Signature { id, sig: signature(r) }),
            Kind::InlayHint => {
                let hints = r
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|h| {
                                let label = match &h["label"] {
                                    Value::String(s) => s.clone(),
                                    Value::Array(parts) => parts.iter().filter_map(|p| p["value"].as_str()).collect(),
                                    _ => String::new(),
                                };
                                (pos(&h["position"]), label, h["paddingLeft"].as_bool().unwrap_or(false), h["paddingRight"].as_bool().unwrap_or(false))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                evs.push(LspEvent::InlayHints { id, hints });
            }
            Kind::SemanticTokens => {
                // Relative: (delta line, delta start, length, type, modifiers) ...
                let data: Vec<u64> = r["data"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
                let (mut line, mut start) = (0u32, 0u32);
                let mut tokens = Vec::new();
                for t in data.chunks_exact(5) {
                    if t[0] > 0 {
                        line += t[0] as u32;
                        start = t[1] as u32;
                    } else {
                        start += t[1] as u32;
                    }
                    if let Some(ty) = self.legend.get(t[3] as usize) {
                        tokens.push((line, start, t[2] as u32, ty.clone()));
                    }
                }
                evs.push(LspEvent::SemanticTokens { id, tokens });
            }
            Kind::WorkspaceSymbol => {
                let symbols = r
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| {
                                let loc = &s["location"];
                                Some(WorkspaceSymbol {
                                    name: s["name"].as_str()?.to_string(),
                                    kind: s["kind"].as_u64().unwrap_or(0) as u8,
                                    container: s["containerName"].as_str().unwrap_or("").to_string(),
                                    path: uri_to_path(loc["uri"].as_str()?)?,
                                    start: pos(&loc["range"]["start"]),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                evs.push(LspEvent::WorkspaceSymbols { id, symbols });
            }
            Kind::Other => {}
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if self.dead.is_none() {
            self.shutdown();
        }
    }
}

fn text_edits(a: &Value) -> TextEdits {
    a.as_array()
        .map(|v| v.iter().map(|e| (pos(&e["range"]["start"]), pos(&e["range"]["end"]), e["newText"].as_str().unwrap_or("").to_string())).collect())
        .unwrap_or_default()
}

/// A WorkspaceEdit: `changes` {uri: [TextEdit]}, or `documentChanges`.
fn workspace_edit(r: &Value) -> WorkspaceEdit {
    let mut edits = Vec::new();
    if let Some(ch) = r["changes"].as_object() {
        for (uri, a) in ch {
            if let Some(p) = uri_to_path(uri) {
                edits.push((p, text_edits(a)));
            }
        }
    }
    if let Some(dc) = r["documentChanges"].as_array() {
        for d in dc {
            if let Some(p) = d["textDocument"]["uri"].as_str().and_then(uri_to_path) {
                edits.push((p, text_edits(&d["edits"])));
            }
        }
    }
    edits
}

fn code_action(a: &Value) -> CodeAction {
    // (A bare Command is an action too.)
    let is_command = a["command"].is_string();
    CodeAction {
        title: a["title"].as_str().unwrap_or("").to_string(),
        kind: a["kind"].as_str().unwrap_or("").to_string(),
        preferred: a["isPreferred"].as_bool().unwrap_or(false),
        edit: a.get("edit").filter(|e| !e.is_null()).map(workspace_edit),
        command: if is_command { Some(a.clone()) } else { a.get("command").filter(|c| !c.is_null()).cloned() },
        raw: a.clone(),
    }
}

fn signature(r: &Value) -> Option<Signature> {
    let sigs = r["signatures"].as_array()?;
    let i = r["activeSignature"].as_u64().unwrap_or(0) as usize;
    let s = sigs.get(i).or(sigs.first())?;
    let label = s["label"].as_str()?.to_string();
    let ap = s["activeParameter"].as_u64().or(r["activeParameter"].as_u64()).unwrap_or(0) as usize;
    let active = s["parameters"].as_array().and_then(|ps| ps.get(ap)).and_then(|p| match &p["label"] {
        // [start, end] in UTF-16 units of the label, or the parameter's text.
        Value::Array(o) if o.len() == 2 => {
            let (a, b) = (o[0].as_u64()? as u32, o[1].as_u64()? as u32);
            Some((byte_of_utf16(&label, a), byte_of_utf16(&label, b)))
        }
        Value::String(t) => label.find(t.as_str()).map(|i| (i, i + t.len())),
        _ => None,
    });
    let doc = match &s["documentation"] {
        Value::String(d) => d.clone(),
        Value::Object(o) => o.get("value").and_then(Value::as_str).unwrap_or("").to_string(),
        _ => String::new(),
    };
    Some(Signature { label, active, doc })
}

fn pos(v: &Value) -> Pos {
    Pos { line: v["line"].as_u64().unwrap_or(0) as u32, character: v["character"].as_u64().unwrap_or(0) as u32 }
}

fn diagnostic(d: &Value) -> Diagnostic {
    Diagnostic {
        start: pos(&d["range"]["start"]),
        end: pos(&d["range"]["end"]),
        severity: d["severity"].as_u64().unwrap_or(1) as u8,
        message: d["message"].as_str().unwrap_or("").to_string(),
        source: d["source"].as_str().unwrap_or("").to_string(),
        raw: d.clone(),
    }
}

fn hover_text(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(hover_text).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n"),
        Value::Object(o) => {
            let v = o.get("value").and_then(Value::as_str).unwrap_or("");
            match o.get("language").and_then(Value::as_str) {
                Some(lang) => format!("```{lang}\n{v}\n```"),
                None => v.to_string(),
            }
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip() {
        let p = std::env::temp_dir().join("a b%é.rs");
        let u = path_to_uri(&p);
        assert!(u.starts_with("file:///") && u.contains("a%20b%25%C3%A9.rs"), "{u}");
        assert_eq!(uri_to_path(&u).unwrap().file_name(), p.file_name());
    }

    #[test]
    fn utf16_columns() {
        let l = "a😀b";
        assert_eq!(utf16_col(l, 5), 3);
        assert_eq!(byte_of_utf16(l, 3), 5);
        assert_eq!(byte_of_utf16(l, 99), l.len());
    }

    #[test]
    fn hover_markup() {
        assert_eq!(hover_text(&json!({"kind": "markdown", "value": "**x**"})), "**x**");
        assert_eq!(hover_text(&json!([{"language": "rust", "value": "fn f()"}, "doc"])), "```rust\nfn f()\n```\n\ndoc");
    }
}
