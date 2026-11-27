//! A debug adapter client (Debug Adapter Protocol) that never blocks:
//! commands go out at once; stops, frames, variables and output come back
//! as [`DapEvent`]s from [`DapClient::poll`].
//!
//! Session flow, as the protocol has it: initialize → launch → (the
//! adapter says `initialized`) → setBreakpoints per file →
//! configurationDone → running. On `stopped`, the client fetches the
//! thread's stack, the top frame's scopes and their variables by itself.

use crate::rpc::{Incoming, RpcProcess, on_path};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// The adapter for a language: (argv, the `type` its launch request wants).
pub fn adapter_for(lang: &str) -> Option<Vec<String>> {
    let candidates: &[&[&str]] = match lang {
        "rust" | "c" | "cpp" | "zig" | "swift" => &[&["lldb-dap"], &["lldb-vscode"], &["codelldb", "--port", "0"], &["gdb", "-i", "dap"]],
        "python" => &[&["python3", "-m", "debugpy.adapter"], &["python", "-m", "debugpy.adapter"]],
        "go" => &[&["dlv", "dap"]],
        "bash" => &[&["bash-debug-adapter"]],
        "c-sharp" | "fsharp" => &[&["netcoredbg", "--interpreter=vscode"]],
        "dart" => &[&["dart", "debug_adapter"]],
        "haskell" => &[&["haskell-debug-adapter"]],
        "ocaml" => &[&["ocamlearlybird", "debug"]],
        "kotlin" => &[&["kotlin-debug-adapter"]],
        "elixir" => &[&["elixir-debug-adapter"], &["elixir-ls-debugger"]],
        "php" => &[&["php-debug-adapter"]],
        "perl" => &[&["perl-debug-adapter"]],
        "d" | "odin" | "nim" | "crystal" | "fortran" | "pascal" | "ada" | "v" | "hare" => &[&["lldb-dap"], &["gdb", "-i", "dap"]],
        _ => &[],
    };
    let ok = |c: &&[&str]| {
        if !on_path(c[0]) {
            return false;
        }
        // debugpy: only when the module is there.
        if c.len() > 2 && c[1] == "-m" {
            return std::process::Command::new(c[0])
                .args(["-c", &format!("import {}", c[2].split('.').next().unwrap_or(""))])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
        }
        true
    };
    candidates.iter().find(|c| ok(c)).map(|c| c.iter().map(|s| s.to_string()).collect())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    /// 1-based, as DAP counts (the client says linesStartAt1).
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub kind: String,
    /// Non-zero: it has children (fetch with `variables`).
    pub reference: i64,
}

#[derive(Debug)]
pub enum DapEvent {
    /// The program stopped: why, and its stack (top first).
    Stopped { reason: String, thread: i64, frames: Vec<Frame> },
    /// The top frame's variables, by scope name.
    Variables { scopes: Vec<(String, Vec<Variable>)> },
    Continued,
    /// Program (or adapter) output: category ("stdout", "stderr", "console").
    Output { category: String, text: String },
    /// A breakpoint the adapter moved or rejected: (path, requested line, actual line, verified).
    Breakpoint { path: PathBuf, line: u32, verified: bool },
    Terminated,
    /// An `evaluate` (or setExpression) answer: the value, its type and
    /// its children's reference; or why it failed.
    Evaluated { id: i64, result: Result<(String, String, i64), String> },
    /// A variable's children (from `children`).
    Children { id: i64, vars: Vec<Variable> },
    Failed(String),
    Log(String),
    Exited(String),
}

/// A breakpoint's extras: stop only when `condition` holds, or log a
/// message instead of stopping (`{expr}` in it is evaluated).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BreakpointOpts {
    pub condition: String,
    pub hit_condition: String,
    pub log_message: String,
}

#[derive(Clone, Debug)]
enum Pending {
    Initialize,
    Launch,
    SetBreakpoints(PathBuf),
    ConfigurationDone,
    Threads,
    StackTrace(i64, String),
    Scopes,
    Variables(String),
    Evaluate(i64),
    Children(i64),
    Other(String),
}

/// What to debug.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub stop_on_entry: bool,
    /// "launch" (the default) or "attach".
    pub request: String,
    /// The configuration's name (from launch.json).
    pub name: String,
    /// Extra fields for the adapter's launch request (adapter specific).
    pub extra: Value,
}

pub struct DapClient {
    rpc: RpcProcess,
    seq: i64,
    pending: HashMap<i64, Pending>,
    launch: Launch,
    /// Breakpoints to send once the adapter is ready: path → lines (1-based).
    pub breakpoints: BTreeMap<PathBuf, BTreeSet<u32>>,
    configured: bool,
    /// Conditions and log messages: path → line → options.
    pub options: BTreeMap<PathBuf, BTreeMap<u32, BreakpointOpts>>,
    pub thread: Option<i64>,
    /// The frame being looked at (the top one at a stop): evaluations
    /// happen in it.
    pub frame: Option<i64>,
    /// What the adapter said it can do (initialize's answer).
    pub capabilities: Value,
    pub running: bool,
    /// Scopes being gathered for one stop: (name, variables) in order.
    gathering: Vec<(String, Option<Vec<Variable>>)>,
    dead: Option<String>,
}

impl DapClient {
    pub fn start(argv: &[String], launch: Launch, breakpoints: BTreeMap<PathBuf, BTreeSet<u32>>) -> std::io::Result<DapClient> {
        let rpc = RpcProcess::spawn(argv, Some(&launch.cwd))?;
        Ok(Self::with_rpc(rpc, launch, breakpoints))
    }

    pub fn with_rpc(rpc: RpcProcess, launch: Launch, breakpoints: BTreeMap<PathBuf, BTreeSet<u32>>) -> DapClient {
        let mut c = DapClient {
            rpc,
            seq: 1,
            pending: HashMap::new(),
            launch,
            breakpoints,
            options: BTreeMap::new(),
            configured: false,
            thread: None,
            frame: None,
            capabilities: Value::Null,
            running: false,
            gathering: Vec::new(),
            dead: None,
        };
        c.request(
            Pending::Initialize,
            "initialize",
            json!({
                "clientID": "turbine", "clientName": "Turbine", "adapterID": "turbine",
                "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path",
                "supportsVariableType": true, "supportsRunInTerminalRequest": false
            }),
        );
        c
    }

    pub fn name(&self) -> &str {
        &self.rpc.name
    }

    pub fn dead(&self) -> Option<&str> {
        self.dead.as_deref()
    }

    fn request(&mut self, kind: Pending, command: &str, args: Value) -> i64 {
        let seq = self.seq;
        self.seq += 1;
        self.pending.insert(seq, kind);
        self.rpc.send(json!({"seq": seq, "type": "request", "command": command, "arguments": args}));
        seq
    }

    fn send_breakpoints(&mut self, path: &Path) {
        let lines: Vec<u32> = self.breakpoints.get(path).map(|s| s.iter().copied().collect()).unwrap_or_default();
        let opts = self.options.get(path);
        let bps: Vec<Value> = lines
            .iter()
            .map(|l| {
                let mut b = json!({"line": l});
                if let Some(o) = opts.and_then(|o| o.get(l)) {
                    for (k, v) in [("condition", &o.condition), ("hitCondition", &o.hit_condition), ("logMessage", &o.log_message)] {
                        if !v.trim().is_empty() {
                            b[k] = json!(v.trim());
                        }
                    }
                }
                b
            })
            .collect();
        self.request(
            Pending::SetBreakpoints(path.to_path_buf()),
            "setBreakpoints",
            json!({"source": {"path": path, "name": path.file_name().map(|n| n.to_string_lossy().into_owned())}, "breakpoints": bps, "lines": lines}),
        );
    }

    /// Change one file's breakpoints (sent now when the session is set up).
    pub fn set_breakpoints(&mut self, path: &Path, lines: BTreeSet<u32>) {
        if lines.is_empty() {
            self.breakpoints.remove(path);
        } else {
            self.breakpoints.insert(path.to_path_buf(), lines);
        }
        if self.configured {
            self.send_breakpoints(path);
        }
    }

    fn thread_args(&self) -> Value {
        json!({"threadId": self.thread.unwrap_or(1)})
    }

    pub fn continue_(&mut self) {
        let a = self.thread_args();
        self.request(Pending::Other("continue".into()), "continue", a);
        self.running = true;
    }

    pub fn step_over(&mut self) {
        let a = self.thread_args();
        self.request(Pending::Other("next".into()), "next", a);
        self.running = true;
    }

    pub fn step_in(&mut self) {
        let a = self.thread_args();
        self.request(Pending::Other("stepIn".into()), "stepIn", a);
        self.running = true;
    }

    pub fn step_out(&mut self) {
        let a = self.thread_args();
        self.request(Pending::Other("stepOut".into()), "stepOut", a);
        self.running = true;
    }

    pub fn pause(&mut self) {
        let a = self.thread_args();
        self.request(Pending::Other("pause".into()), "pause", a);
    }

    pub fn stop(&mut self) {
        let attached = self.launch.request == "attach";
        self.request(Pending::Other("disconnect".into()), "disconnect", json!({"terminateDebuggee": !attached}));
    }

    /// A line's condition / log message (sent now when set up).
    pub fn set_options(&mut self, path: &Path, line: u32, o: BreakpointOpts) {
        let m = self.options.entry(path.to_path_buf()).or_default();
        if o == BreakpointOpts::default() {
            m.remove(&line);
        } else {
            m.insert(line, o);
        }
        if self.configured {
            self.send_breakpoints(path);
        }
    }

    /// Evaluate an expression in the current frame. `context`: "watch",
    /// "repl" (the debug console: statements too) or "hover".
    pub fn evaluate(&mut self, expr: &str, context: &str) -> i64 {
        let mut a = json!({"expression": expr, "context": context});
        if let Some(f) = self.frame {
            a["frameId"] = json!(f);
        }
        let seq = self.seq;
        self.request(Pending::Evaluate(seq), "evaluate", a)
    }

    /// Give an expression (a variable, a field) a new value: with
    /// setExpression when the adapter has it, else as an assignment.
    pub fn set_expression(&mut self, expr: &str, value: &str) -> i64 {
        if self.capabilities["supportsSetExpression"].as_bool() == Some(true) {
            let mut a = json!({"expression": expr, "value": value});
            if let Some(f) = self.frame {
                a["frameId"] = json!(f);
            }
            let seq = self.seq;
            self.request(Pending::Evaluate(seq), "setExpression", a)
        } else {
            self.evaluate(&format!("{expr} = {value}"), "repl")
        }
    }

    /// A structured value's children (its `reference`).
    pub fn children(&mut self, reference: i64) -> i64 {
        let seq = self.seq;
        self.request(Pending::Children(seq), "variables", json!({"variablesReference": reference}))
    }

    pub fn poll(&mut self) -> Vec<DapEvent> {
        let mut evs = Vec::new();
        for inc in self.rpc.incoming() {
            match inc {
                Incoming::Log(l) => evs.push(DapEvent::Log(l)),
                Incoming::Exited(why) => {
                    if self.dead.is_none() {
                        self.dead = Some(why.clone());
                        evs.push(DapEvent::Exited(why));
                    }
                }
                Incoming::Message(m) => self.message(m, &mut evs),
            }
        }
        evs
    }

    fn message(&mut self, m: Value, evs: &mut Vec<DapEvent>) {
        match m["type"].as_str() {
            Some("event") => self.event(&m, evs),
            Some("response") => {
                let seq = m["request_seq"].as_i64().unwrap_or(0);
                let Some(kind) = self.pending.remove(&seq) else { return };
                if m["success"].as_bool() == Some(false) {
                    let msg = m["message"].as_str().unwrap_or("failed").to_string();
                    let detail = m["body"]["error"]["format"].as_str().map(|f| format!(": {f}")).unwrap_or_default();
                    if let Pending::Evaluate(id) = kind {
                        let why = m["body"]["error"]["format"].as_str().map(str::to_string).unwrap_or(msg);
                        evs.push(DapEvent::Evaluated { id, result: Err(why) });
                        return;
                    }
                    evs.push(DapEvent::Failed(format!("{}{msg}{detail}", match &kind {
                        Pending::Launch => "launch: ",
                        Pending::Initialize => "initialize: ",
                        _ => "",
                    })));
                    return;
                }
                self.response(kind, &m["body"], evs);
            }
            Some("request") => {
                // Reverse requests (runInTerminal, startDebugging): not supported.
                let seq = m["seq"].as_i64().unwrap_or(0);
                let cmd = m["command"].as_str().unwrap_or("");
                self.rpc.send(json!({"seq": self.seq, "type": "response", "request_seq": seq, "success": false, "command": cmd, "message": "not supported"}));
                self.seq += 1;
            }
            _ => {}
        }
    }

    fn event(&mut self, m: &Value, evs: &mut Vec<DapEvent>) {
        let b = &m["body"];
        match m["event"].as_str().unwrap_or("") {
            "initialized" => {
                let paths: Vec<PathBuf> = self.breakpoints.keys().cloned().collect();
                self.configured = true;
                for p in paths {
                    self.send_breakpoints(&p);
                }
                self.request(Pending::ConfigurationDone, "configurationDone", json!({}));
                self.running = true;
            }
            "stopped" => {
                self.running = false;
                let reason = b["reason"].as_str().unwrap_or("pause").to_string();
                match b["threadId"].as_i64() {
                    Some(t) => {
                        self.thread = Some(t);
                        self.request(Pending::StackTrace(t, reason), "stackTrace", json!({"threadId": t, "startFrame": 0, "levels": 50}));
                    }
                    None => {
                        self.request(Pending::Threads, "threads", json!({}));
                        self.gathering.clear();
                        self.pending.insert(-1, Pending::Other(reason));
                    }
                }
            }
            "continued" => {
                self.running = true;
                evs.push(DapEvent::Continued);
            }
            "output" => {
                let category = b["category"].as_str().unwrap_or("console").to_string();
                if category != "telemetry" {
                    evs.push(DapEvent::Output { category, text: b["output"].as_str().unwrap_or("").to_string() });
                }
            }
            "breakpoint" => {
                let bp = &b["breakpoint"];
                if let Some(p) = bp["source"]["path"].as_str() {
                    evs.push(DapEvent::Breakpoint {
                        path: PathBuf::from(p),
                        line: bp["line"].as_u64().unwrap_or(0) as u32,
                        verified: bp["verified"].as_bool().unwrap_or(false),
                    });
                }
            }
            "terminated" | "exited" => {
                if self.running || m["event"] == "terminated" {
                    self.running = false;
                    evs.push(DapEvent::Terminated);
                }
            }
            _ => {}
        }
    }

    fn response(&mut self, kind: Pending, b: &Value, evs: &mut Vec<DapEvent>) {
        match kind {
            Pending::Initialize => {
                self.capabilities = b.clone();
                let request = if self.launch.request.is_empty() { "launch" } else { self.launch.request.as_str() };
                let mut args = json!({
                    "program": self.launch.program,
                    "args": self.launch.args,
                    "cwd": self.launch.cwd,
                    "stopOnEntry": self.launch.stop_on_entry,
                    "name": if self.launch.name.is_empty() { "Turbine" } else { self.launch.name.as_str() },
                    "request": request,
                });
                if request == "attach" {
                    if let Some(a) = args.as_object_mut() {
                        a.remove("program");
                        a.remove("args");
                        a.remove("stopOnEntry");
                    }
                }
                if let (Some(a), Some(extra)) = (args.as_object_mut(), self.launch.extra.as_object()) {
                    for (k, v) in extra {
                        a.insert(k.clone(), v.clone());
                    }
                }
                let command = if request == "attach" { "attach" } else { "launch" };
                self.request(Pending::Launch, command, args);
            }
            Pending::Launch | Pending::ConfigurationDone | Pending::Other(_) => {}
            Pending::SetBreakpoints(path) => {
                let wanted: Vec<u32> = self.breakpoints.get(&path).map(|s| s.iter().copied().collect()).unwrap_or_default();
                for (i, bp) in b["breakpoints"].as_array().cloned().unwrap_or_default().iter().enumerate() {
                    let line = bp["line"].as_u64().map(|l| l as u32).or(wanted.get(i).copied()).unwrap_or(0);
                    evs.push(DapEvent::Breakpoint { path: path.clone(), line, verified: bp["verified"].as_bool().unwrap_or(false) });
                }
            }
            Pending::Threads => {
                let t = b["threads"][0]["id"].as_i64().unwrap_or(1);
                self.thread = Some(t);
                let reason = match self.pending.remove(&-1) {
                    Some(Pending::Other(r)) => r,
                    _ => "pause".into(),
                };
                self.request(Pending::StackTrace(t, reason), "stackTrace", json!({"threadId": t, "startFrame": 0, "levels": 50}));
            }
            Pending::StackTrace(thread, reason) => {
                let frames: Vec<Frame> = b["stackFrames"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|f| Frame {
                                id: f["id"].as_i64().unwrap_or(0),
                                name: f["name"].as_str().unwrap_or("?").to_string(),
                                path: f["source"]["path"].as_str().map(PathBuf::from),
                                line: f["line"].as_u64().unwrap_or(0) as u32,
                                column: f["column"].as_u64().unwrap_or(0) as u32,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(top) = frames.first() {
                    self.frame = Some(top.id);
                    self.request(Pending::Scopes, "scopes", json!({"frameId": top.id}));
                }
                evs.push(DapEvent::Stopped { reason, thread, frames });
            }
            Pending::Scopes => {
                self.gathering.clear();
                for s in b["scopes"].as_array().cloned().unwrap_or_default() {
                    let name = s["name"].as_str().unwrap_or("Scope").to_string();
                    let r = s["variablesReference"].as_i64().unwrap_or(0);
                    // Big scopes (registers, globals) stay folded.
                    if r != 0 && !s["expensive"].as_bool().unwrap_or(false) {
                        self.gathering.push((name.clone(), None));
                        self.request(Pending::Variables(name), "variables", json!({"variablesReference": r}));
                    }
                }
                if self.gathering.is_empty() {
                    evs.push(DapEvent::Variables { scopes: vec![] });
                }
            }
            Pending::Evaluate(id) => {
                let value = b["result"].as_str().or(b["value"].as_str()).unwrap_or("").to_string();
                let kind = b["type"].as_str().unwrap_or("").to_string();
                evs.push(DapEvent::Evaluated { id, result: Ok((value, kind, b["variablesReference"].as_i64().unwrap_or(0))) });
            }
            Pending::Children(id) => evs.push(DapEvent::Children { id, vars: variables(b) }),
            Pending::Variables(scope) => {
                let vars = variables(b);
                if let Some(slot) = self.gathering.iter_mut().find(|(n, v)| *n == scope && v.is_none()) {
                    slot.1 = Some(vars);
                }
                if self.gathering.iter().all(|(_, v)| v.is_some()) {
                    let scopes = self.gathering.drain(..).map(|(n, v)| (n, v.unwrap_or_default())).collect();
                    evs.push(DapEvent::Variables { scopes });
                }
            }
        }
    }
}

fn variables(b: &Value) -> Vec<Variable> {
    b["variables"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| Variable {
                    name: v["name"].as_str().unwrap_or("").to_string(),
                    value: v["value"].as_str().unwrap_or("").to_string(),
                    kind: v["type"].as_str().unwrap_or("").to_string(),
                    reference: v["variablesReference"].as_i64().unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A debug configuration from `.turbine/launch.json` or
/// `.vscode/launch.json`, its `${...}` variables filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct LaunchConfig {
    pub name: String,
    /// The adapter type ("lldb", "debugpy", "go", ...).
    pub kind: String,
    pub launch: Launch,
}

/// The language whose adapter a launch.json `type` means.
pub fn lang_for_type(kind: &str) -> &'static str {
    match kind {
        "lldb" | "lldb-dap" | "codelldb" | "cppdbg" | "cppvsdbg" | "gdb" => "cpp",
        "debugpy" | "python" => "python",
        "go" | "delve" => "go",
        "bashdb" => "bash",
        _ => "",
    }
}

/// JSON with comments and trailing commas (as VS Code writes it) → JSON.
pub fn strip_jsonc(t: &str) -> String {
    let (mut out, mut chars) = (String::with_capacity(t.len()), t.chars().peekable());
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas: a comma whose next non-space is } or ].
    let b: Vec<char> = out.chars().collect();
    let mut res = String::with_capacity(b.len());
    let mut in_str = false;
    for i in 0..b.len() {
        let c = b[i];
        if in_str {
            in_str = !(c == '"' && b[..i].iter().rev().take_while(|&&x| x == '\\').count() % 2 == 0);
        } else if c == '"' {
            in_str = true;
        } else if c == ',' && b[i + 1..].iter().find(|x| !x.is_whitespace()).is_some_and(|&x| x == '}' || x == ']') {
            continue;
        }
        res.push(c);
    }
    res
}

/// The project's debug configurations. `file`: the active file (for
/// `${file}` and friends).
pub fn launch_configs(root: &Path, file: Option<&Path>) -> Vec<LaunchConfig> {
    let text = [".turbine/launch.json", ".vscode/launch.json"].iter().find_map(|f| std::fs::read_to_string(root.join(f)).ok());
    let Some(text) = text else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&strip_jsonc(&text)) else { return Vec::new() };
    let file_s = file.map(|f| f.display().to_string()).unwrap_or_default();
    let vars: Vec<(&str, String)> = vec![
        ("${workspaceFolder}", root.display().to_string()),
        ("${workspaceRoot}", root.display().to_string()),
        ("${workspaceFolderBasename}", root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        ("${file}", file_s.clone()),
        ("${relativeFile}", file.and_then(|f| f.strip_prefix(root).ok()).map(|f| f.display().to_string()).unwrap_or_default()),
        ("${fileBasename}", file.and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        ("${fileBasenameNoExtension}", file.and_then(|f| f.file_stem()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        ("${fileDirname}", file.and_then(|f| f.parent()).map(|f| f.display().to_string()).unwrap_or_default()),
        ("${fileExtname}", file.and_then(|f| f.extension()).map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default()),
        ("${userHome}", std::env::var("HOME").unwrap_or_default()),
    ];
    fn fill(v: &mut Value, vars: &[(&str, String)]) {
        match v {
            Value::String(s) => {
                for (k, val) in vars {
                    if s.contains(k) {
                        *s = s.replace(k, val);
                    }
                }
                while let Some(i) = s.find("${env:") {
                    let Some(end) = s[i..].find('}') else { break };
                    let name = s[i + 6..i + end].to_string();
                    *s = format!("{}{}{}", &s[..i], std::env::var(&name).unwrap_or_default(), &s[i + end + 1..]);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| fill(x, vars)),
            Value::Object(o) => o.values_mut().for_each(|x| fill(x, vars)),
            _ => {}
        }
    }
    v["configurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut c| {
            fill(&mut c, &vars);
            let s = |k: &str| c[k].as_str().unwrap_or("").to_string();
            let cwd = c["cwd"].as_str().map(PathBuf::from).unwrap_or_else(|| root.to_path_buf());
            let mut extra = c.clone();
            if let Some(o) = extra.as_object_mut() {
                for k in ["name", "type", "request", "program", "args", "cwd", "stopOnEntry"] {
                    o.remove(k);
                }
            }
            let args = match &c["args"] {
                Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                Value::String(s) => s.split_whitespace().map(str::to_string).collect(),
                _ => Vec::new(),
            };
            LaunchConfig {
                name: s("name"),
                kind: s("type"),
                launch: Launch {
                    program: PathBuf::from(s("program")),
                    args,
                    cwd,
                    stop_on_entry: c["stopOnEntry"].as_bool().unwrap_or(false),
                    request: if s("request").is_empty() { "launch".into() } else { s("request") },
                    name: s("name"),
                    extra,
                },
            }
        })
        .collect()
}

impl Drop for DapClient {
    fn drop(&mut self) {
        if self.dead.is_none() {
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(c: &mut DapClient, mut f: impl FnMut(&DapEvent) -> bool) -> Vec<DapEvent> {
        let start = Instant::now();
        let mut all = Vec::new();
        loop {
            let evs = c.poll();
            let hit = evs.iter().any(&mut f);
            all.extend(evs);
            if hit {
                return all;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "{all:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn breakpoint_stack_variables_step_continue() {
        let dir = std::env::temp_dir().join(format!("turbine-dap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let prog = dir.join("prog.txt");
        std::fs::write(&prog, "print(one)\nx = 1\nprint(two)\nprint(three)\n").unwrap();
        let mut bps = BTreeMap::new();
        bps.insert(prog.clone(), BTreeSet::from([3]));
        let launch = Launch { program: prog.clone(), cwd: dir.clone(), ..Default::default() };
        let mut c = DapClient::with_rpc(crate::fake::fake_debug_adapter().unwrap(), launch, bps);
        let evs = wait(&mut c, |e| matches!(e, DapEvent::Variables { .. }));
        assert!(evs.iter().any(|e| matches!(e, DapEvent::Output { text, .. } if text == "one\n")));
        let frames = evs.iter().find_map(|e| if let DapEvent::Stopped { frames, .. } = e { Some(frames.clone()) } else { None }).unwrap();
        assert_eq!((frames[0].line, frames[0].path.as_deref()), (3, Some(prog.as_path())));
        let vars = evs.iter().find_map(|e| if let DapEvent::Variables { scopes } = e { Some(scopes.clone()) } else { None }).unwrap();
        assert_eq!(vars[0].0, "Locals");
        assert_eq!(vars[0].1[0].value, "3");
        c.step_over();
        let evs = wait(&mut c, |e| matches!(e, DapEvent::Stopped { .. }));
        assert!(evs.iter().any(|e| matches!(e, DapEvent::Output { text, .. } if text == "two\n")));
        c.continue_();
        let evs = wait(&mut c, |e| matches!(e, DapEvent::Terminated));
        assert!(evs.iter().any(|e| matches!(e, DapEvent::Output { text, .. } if text == "three\n")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
