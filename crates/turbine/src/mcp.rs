//! `turbine mcp`: a Model Context Protocol server on stdin/stdout that lets
//! outside agents (Claude Code, Codex, hearth's assistant) drive a running
//! Turbine with the same tools Turbine's own agent has. Each tool call is
//! sent to Turbine's automation socket and acted out visibly on screen.

use crate::tools;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// The socket of a running Turbine: `--socket`, `TURBINE_SOCKET`, else the
/// newest `turbine-*.sock` in the runtime folder.
pub fn find_socket(arg: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = arg.or_else(|| std::env::var_os("TURBINE_SOCKET").map(PathBuf::from)) {
        return Some(p);
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let mut socks: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("turbine-") && e.file_name().to_string_lossy().ends_with(".sock"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    socks.sort();
    socks.into_iter().rev().map(|(_, p)| p).find(|p| connects(p))
}

#[cfg(unix)]
fn connects(p: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(p).is_ok()
}
#[cfg(not(unix))]
fn connects(_p: &std::path::Path) -> bool {
    false
}

/// The protocol, independent of where commands go (`send` returns
/// Turbine's reply to one automation command).
pub struct Server<F: FnMut(Value) -> Value> {
    send: F,
    client: String,
}

impl<F: FnMut(Value) -> Value> Server<F> {
    pub fn new(send: F) -> Self {
        Server { send, client: "MCP agent".into() }
    }

    /// One JSON-RPC message in, maybe one out.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id")?.clone();
        let method = msg["method"].as_str().unwrap_or("");
        let result = match method {
            "initialize" => {
                if let Some(n) = msg["params"]["clientInfo"]["name"].as_str() {
                    self.client = n.to_string();
                }
                let version = msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18");
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "turbine", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "Drive the Turbine editor the way a person does; the person watches every action. Call look first and after each change."
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": tools::mcp_tools()}),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
                match tools::to_command(name, &args) {
                    Ok(mut cmd) => {
                        cmd["agent"] = json!(self.client);
                        let reply = (self.send)(cmd);
                        let text = tools::result_text(name, &reply);
                        json!({"content": [{"type": "text", "text": text}], "isError": reply["ok"] != true})
                    }
                    Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
                }
            }
            _ => return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("no method {method}")}})),
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }
}

/// Run on stdin/stdout until stdin closes.
#[cfg(unix)]
pub fn run(socket: Option<PathBuf>) -> i32 {
    use std::os::unix::net::UnixStream;
    let Some(path) = find_socket(socket) else {
        eprintln!("turbine mcp: no running Turbine found (start turbine, or give --socket / TURBINE_SOCKET)");
        return 1;
    };
    let stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("turbine mcp: {}: {e}", path.display());
            return 1;
        }
    };
    let mut writer = stream.try_clone().expect("socket clone");
    let mut reader = BufReader::new(stream);
    let mut server = Server::new(move |cmd: Value| {
        if writeln!(writer, "{cmd}").is_err() {
            return json!({"ok": false, "error": "Turbine closed"});
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(n) if n > 0 => serde_json::from_str(&line).unwrap_or(json!({"ok": false, "error": "bad reply"})),
            _ => json!({"ok": false, "error": "Turbine closed"}),
        }
    });
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(reply) = server.handle(&msg) {
            if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
    0
}

#[cfg(not(unix))]
pub fn run(_socket: Option<PathBuf>) -> i32 {
    eprintln!("turbine mcp: needs Unix sockets for now");
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_list_and_call() {
        let mut sent = Vec::new();
        let mut s = Server::new(|cmd: Value| {
            sent.push(cmd.clone());
            json!({"ok": true})
        });
        let r = s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": "claude-code"}}})).unwrap();
        assert_eq!(r["result"]["serverInfo"]["name"], "turbine");
        assert!(s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let r = s.handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        assert!(r["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "menu"));
        let r = s.handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "menu", "arguments": {"path": "File/Save"}}})).unwrap();
        assert_eq!(r["result"]["isError"], false);
        drop(s);
        assert_eq!(sent[0]["cmd"], "menu");
        assert_eq!(sent[0]["agent"], "claude-code");
    }
}
