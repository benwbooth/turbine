//! A tiny language server in a thread, for tests (and to try the editor's
//! LSP features with no server installed).
//!
//! - Diagnostics: an error on every line containing "BAD".
//! - Completion: `println!`, `print!`, `panic!` and the words in the file.
//! - Hover: "fake hover: <word>".
//! - Definition: the first line that contains the word under the cursor.

use crate::rpc::{RpcProcess, read_message, write_message};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::BufReader;

fn word_at(text: &str, line: u32, ch: u32) -> String {
    let l = text.lines().nth(line as usize).unwrap_or("");
    let b = crate::lsp::byte_of_utf16(l, ch);
    let is_w = |c: char| c.is_alphanumeric() || c == '_';
    let start = l[..b].rfind(|c| !is_w(c)).map(|i| i + 1).unwrap_or(0);
    let end = l[b..].find(|c| !is_w(c)).map(|i| b + i).unwrap_or(l.len());
    l[start..end].to_string()
}

pub fn fake_server() -> std::io::Result<RpcProcess> {
    let (client_r, server_w) = std::io::pipe()?;
    let (server_r, client_w) = std::io::pipe()?;
    std::thread::Builder::new().name("fake-lsp".into()).spawn(move || {
        let mut r = BufReader::new(server_r);
        let mut w = server_w;
        let mut docs: HashMap<String, String> = HashMap::new();
        while let Ok(Some(m)) = read_message(&mut r) {
            let method = m["method"].as_str().unwrap_or("");
            let id = m.get("id").cloned();
            let p = &m["params"];
            let uri = p["textDocument"]["uri"].as_str().unwrap_or("").to_string();
            let reply = |w: &mut std::io::PipeWriter, result: Value| {
                let _ = write_message(w, &json!({"jsonrpc": "2.0", "id": id, "result": result}));
            };
            match method {
                "initialize" => reply(&mut w, json!({"capabilities": {"textDocumentSync": 1, "completionProvider": {}, "hoverProvider": true, "definitionProvider": true}})),
                "textDocument/didOpen" | "textDocument/didChange" => {
                    let text = if method == "textDocument/didOpen" {
                        p["textDocument"]["text"].as_str().unwrap_or("").to_string()
                    } else {
                        p["contentChanges"][0]["text"].as_str().unwrap_or("").to_string()
                    };
                    let diags: Vec<Value> = text
                        .lines()
                        .enumerate()
                        .filter_map(|(i, l)| {
                            let b = l.find("BAD")?;
                            let c = crate::lsp::utf16_col(l, b);
                            Some(json!({"range": {"start": {"line": i, "character": c}, "end": {"line": i, "character": c + 3}}, "severity": 1, "message": "BAD is not allowed", "source": "fake"}))
                        })
                        .collect();
                    docs.insert(uri.clone(), text);
                    let _ = write_message(&mut w, &json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": diags}}));
                }
                "textDocument/completion" => {
                    let text = docs.get(&uri).cloned().unwrap_or_default();
                    let mut words: Vec<String> = vec!["println!".into(), "print!".into(), "panic!".into()];
                    for w in text.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| w.len() > 2) {
                        if !words.iter().any(|x| x == w) {
                            words.push(w.to_string());
                        }
                    }
                    let items: Vec<Value> = words.iter().map(|w| json!({"label": w, "detail": "fake"})).collect();
                    reply(&mut w, json!({"isIncomplete": false, "items": items}));
                }
                "textDocument/hover" => {
                    let text = docs.get(&uri).cloned().unwrap_or_default();
                    let word = word_at(&text, p["position"]["line"].as_u64().unwrap_or(0) as u32, p["position"]["character"].as_u64().unwrap_or(0) as u32);
                    reply(&mut w, json!({"contents": {"kind": "plaintext", "value": format!("fake hover: {word}")}}));
                }
                "textDocument/definition" => {
                    let text = docs.get(&uri).cloned().unwrap_or_default();
                    let word = word_at(&text, p["position"]["line"].as_u64().unwrap_or(0) as u32, p["position"]["character"].as_u64().unwrap_or(0) as u32);
                    let hit = text.lines().enumerate().find_map(|(i, l)| l.find(&word).map(|b| (i, crate::lsp::utf16_col(l, b))));
                    match hit {
                        Some((line, c)) if !word.is_empty() => reply(&mut w, json!({"uri": uri, "range": {"start": {"line": line, "character": c}, "end": {"line": line, "character": c}}})),
                        _ => reply(&mut w, Value::Null),
                    }
                }
                "shutdown" => reply(&mut w, Value::Null),
                "exit" => return,
                _ if id.is_some() => reply(&mut w, Value::Null),
                _ => {}
            }
        }
    })?;
    RpcProcess::from_streams("fake-lsp", Box::new(client_r), Box::new(client_w), None)
}
