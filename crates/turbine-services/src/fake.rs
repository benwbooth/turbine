//! A tiny language server in a thread, for tests (and to try the editor's
//! LSP features with no server installed).
//!
//! - Diagnostics: an error on every line containing "BAD".
//! - Completion: `println!`, `print!`, `panic!` and the words in the file.
//! - Hover: "fake hover: <word>".
//! - Definition: the first line that contains the word under the cursor.
//! - Formatting: trailing spaces go. Code actions: "Replace BAD with GOOD"
//!   on a BAD line, one that needs resolving ("Add a header"), and one
//!   that's a command (the server then asks to append "// applied").
//! - Signature help: `add(a: i32, b: i32)`, the parameter by commas.
//! - Inlay hints: `: i32` after each `let name`. Semantic tokens: words
//!   starting with a capital letter are types. Workspace symbols: every
//!   `fn name` in the open files containing the query.

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
                "initialize" => reply(
                    &mut w,
                    json!({"capabilities": {
                        "textDocumentSync": 1, "completionProvider": {}, "hoverProvider": true, "definitionProvider": true,
                        "documentFormattingProvider": true, "codeActionProvider": {"resolveProvider": true},
                        "signatureHelpProvider": {"triggerCharacters": ["(", ","]}, "inlayHintProvider": true,
                        "semanticTokensProvider": {"legend": {"tokenTypes": ["type", "function"], "tokenModifiers": []}, "full": true},
                        "workspaceSymbolProvider": true, "executeCommandProvider": {"commands": ["fake.apply"]}
                    }}),
                ),
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
                    // (A shell option being typed, `--co`: options replacing
                    // it from its dashes, as a shell's server says.)
                    let (line, ch) = (p["position"]["line"].as_u64().unwrap_or(0), p["position"]["character"].as_u64().unwrap_or(0));
                    let before: String = text.lines().nth(line as usize).unwrap_or("").chars().take(ch as usize).collect();
                    if let Some(at) = before.rfind(' ').map(|i| i + 1).filter(|&i| before[i..].starts_with("--")) {
                        let range = json!({"start": {"line": line, "character": at}, "end": {"line": line, "character": ch}});
                        let items: Vec<Value> = ["--color", "--columns"].iter().map(|o| json!({"label": o, "detail": "option", "filterText": o, "textEdit": {"range": range, "newText": o}})).collect();
                        reply(&mut w, json!({"isIncomplete": false, "items": items}));
                        continue;
                    }
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
                "textDocument/references" | "textDocument/rename" => {
                    // Every whole-word use of the name in the file.
                    let text = docs.get(&uri).cloned().unwrap_or_default();
                    let word = word_at(&text, p["position"]["line"].as_u64().unwrap_or(0) as u32, p["position"]["character"].as_u64().unwrap_or(0) as u32);
                    let is_word = |c: char| c.is_alphanumeric() || c == '_';
                    let mut ranges = Vec::new();
                    for (i, l) in text.lines().enumerate() {
                        for (b, _) in l.match_indices(&word) {
                            let before = l[..b].chars().next_back();
                            let after = l[b + word.len()..].chars().next();
                            if !word.is_empty() && !before.is_some_and(is_word) && !after.is_some_and(is_word) {
                                let c = crate::lsp::utf16_col(l, b);
                                ranges.push(json!({"start": {"line": i, "character": c}, "end": {"line": i, "character": c + word.encode_utf16().count() as u32}}));
                            }
                        }
                    }
                    if method == "textDocument/references" {
                        reply(&mut w, Value::Array(ranges.into_iter().map(|r| json!({"uri": uri, "range": r})).collect()));
                    } else {
                        let new = p["newName"].as_str().unwrap_or("").to_string();
                        let edits: Vec<Value> = ranges.into_iter().map(|r| json!({"range": r, "newText": new})).collect();
                        reply(&mut w, json!({"changes": {uri.clone(): edits}}));
                    }
                }
                "textDocument/formatting" => {
                    let text = docs.get(&uri).cloned().unwrap_or_default();
                    let edits: Vec<Value> = text
                        .lines()
                        .enumerate()
                        .filter(|(_, l)| l.ends_with([' ', '\t']))
                        .map(|(i, l)| {
                            let (a, b) = (crate::lsp::utf16_col(l, l.trim_end().len()), crate::lsp::utf16_col(l, l.len()));
                            json!({"range": {"start": {"line": i, "character": a}, "end": {"line": i, "character": b}}, "newText": ""})
                        })
                        .collect();
                    reply(&mut w, Value::Array(edits));
                }
                "textDocument/codeAction" => {
                    let mut actions = Vec::new();
                    for d in p["context"]["diagnostics"].as_array().into_iter().flatten() {
                        actions.push(json!({"title": "Replace BAD with GOOD", "kind": "quickfix", "isPreferred": true, "diagnostics": [d],
                            "edit": {"changes": {uri.clone(): [{"range": d["range"], "newText": "GOOD"}]}}}));
                    }
                    actions.push(json!({"title": "Add a header", "kind": "refactor", "data": {"uri": uri}}));
                    actions.push(json!({"title": "Apply from the server", "command": "fake.apply", "arguments": [uri]}));
                    reply(&mut w, Value::Array(actions));
                }
                "codeAction/resolve" => {
                    let mut a = p.clone();
                    let u = p["data"]["uri"].as_str().unwrap_or("").to_string();
                    a["edit"] = json!({"changes": {u: [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "// header\n"}]}});
                    reply(&mut w, a);
                }
                "workspace/executeCommand" => {
                    reply(&mut w, Value::Null);
                    let u = p["arguments"][0].as_str().unwrap_or("").to_string();
                    let lines = docs.get(&u).map(|t| t.lines().count()).unwrap_or(0);
                    let edit = json!({"changes": {u: [{"range": {"start": {"line": lines, "character": 0}, "end": {"line": lines, "character": 0}}, "newText": "// applied\n"}]}});
                    let _ = write_message(&mut w, &json!({"jsonrpc": "2.0", "id": 9001, "method": "workspace/applyEdit", "params": {"edit": edit}}));
                }
                "textDocument/signatureHelp" => {
                    let text = docs.get(&uri).cloned().unwrap_or_default();
                    let l = text.lines().nth(p["position"]["line"].as_u64().unwrap_or(0) as usize).unwrap_or("");
                    let b = crate::lsp::byte_of_utf16(l, p["position"]["character"].as_u64().unwrap_or(0) as u32);
                    match l[..b].rfind('(') {
                        Some(open) if !l[open..b].contains(')') => {
                            let commas = l[open..b].matches(',').count();
                            reply(&mut w, json!({"signatures": [{"label": "add(a: i32, b: i32) -> i32", "documentation": "Adds two numbers.",
                                "parameters": [{"label": [4, 10]}, {"label": "b: i32"}]}], "activeSignature": 0, "activeParameter": commas}));
                        }
                        _ => reply(&mut w, Value::Null),
                    }
                }
                "textDocument/inlayHint" => {
                    let text = docs.get(&uri).cloned().unwrap_or_default();
                    let hints: Vec<Value> = text
                        .lines()
                        .enumerate()
                        .filter_map(|(i, l)| {
                            let at = l.find("let ")? + 4;
                            let end = l[at..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map(|e| at + e).unwrap_or(l.len());
                            (end > at && !l[end..].trim_start().starts_with(':')).then(|| json!({"position": {"line": i, "character": crate::lsp::utf16_col(l, end)}, "label": ": i32", "kind": 1}))
                        })
                        .collect();
                    reply(&mut w, Value::Array(hints));
                }
                "textDocument/semanticTokens/full" => {
                    let text = docs.get(&uri).cloned().unwrap_or_default();
                    let mut data = Vec::new();
                    let (mut pl, mut pc) = (0u32, 0u32);
                    for (i, l) in text.lines().enumerate() {
                        let mut b = 0;
                        for word in l.split(|c: char| !c.is_alphanumeric()) {
                            let at = b;
                            b += word.len() + 1;
                            if word.chars().next().is_some_and(char::is_uppercase) {
                                let c = crate::lsp::utf16_col(l, at);
                                let i = i as u32;
                                let dc = if i == pl { c - pc } else { c };
                                data.extend([i - pl, dc, word.encode_utf16().count() as u32, 0, 0]);
                                (pl, pc) = (i, c);
                            }
                        }
                    }
                    reply(&mut w, json!({"data": data}));
                }
                "workspace/symbol" => {
                    let q = p["query"].as_str().unwrap_or("").to_lowercase();
                    let mut out = Vec::new();
                    for (u, text) in &docs {
                        for (i, l) in text.lines().enumerate() {
                            if let Some(at) = l.find("fn ") {
                                let name: String = l[at + 3..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                                if !name.is_empty() && name.to_lowercase().contains(&q) {
                                    let c = crate::lsp::utf16_col(l, at + 3);
                                    out.push(json!({"name": name, "kind": 12, "location": {"uri": u, "range": {"start": {"line": i, "character": c}, "end": {"line": i, "character": c}}}}));
                                }
                            }
                        }
                    }
                    reply(&mut w, Value::Array(out));
                }
                "" => {} // (an answer to our applyEdit)
                "shutdown" => reply(&mut w, Value::Null),
                "exit" => return,
                _ if id.is_some() => reply(&mut w, Value::Null),
                _ => {}
            }
        }
    })?;
    RpcProcess::from_streams("fake-lsp", Box::new(client_r), Box::new(client_w), None)
}

/// A pretend debug adapter: it "runs" the program file a line at a time.
/// A line containing `print(...)` writes what's inside to stdout; it stops
/// at breakpoints (and the first line with stopOnEntry); `next`/`stepIn`
/// go one line; variables are `line` and `text` (the current line).
pub fn fake_debug_adapter() -> std::io::Result<RpcProcess> {
    let (client_r, server_w) = std::io::pipe()?;
    let (server_r, client_w) = std::io::pipe()?;
    std::thread::Builder::new().name("fake-dap".into()).spawn(move || {
        let mut r = BufReader::new(server_r);
        let mut w = server_w;
        let mut seq = 1000;
        let mut send = |w: &mut std::io::PipeWriter, mut v: Value| {
            v["seq"] = json!(seq);
            seq += 1;
            let _ = write_message(w, &v);
        };
        let mut program = String::new();
        let mut lines: Vec<String> = Vec::new();
        let mut bps: Vec<u32> = Vec::new();
        // Conditions and log messages by line; variables set with `x = v`.
        let mut opts: HashMap<u32, (String, String)> = HashMap::new();
        let mut vars: HashMap<String, String> = HashMap::new();
        let mut at: u32 = 0; // current line, 1-based; 0 = not started
        let mut stop_on_entry = false;
        // Run from `from` (inclusive) until a breakpoint or the end; true = stopped.
        // Does a breakpoint at `l` stop (its condition), or log instead?
        fn hits(w: &mut std::io::PipeWriter, send: &mut dyn FnMut(&mut std::io::PipeWriter, Value), bps: &[u32], opts: &HashMap<u32, (String, String)>, l: u32) -> bool {
            if !bps.contains(&l) {
                return false;
            }
            let Some((cond, log)) = opts.get(&l) else { return true };
            if !cond.is_empty() {
                // "line == N", "line > N", "line < N"
                let parts: Vec<&str> = cond.split_whitespace().collect();
                let n: u32 = parts.get(2).and_then(|n| n.parse().ok()).unwrap_or(0);
                let ok = match parts.get(1).copied() {
                    Some("==") => l == n,
                    Some(">") => l > n,
                    Some("<") => l < n,
                    _ => true,
                };
                if !ok {
                    return false;
                }
            }
            if !log.is_empty() {
                send(w, json!({"type": "event", "event": "output", "body": {"category": "console", "output": format!("{}\n", log.replace("{line}", &l.to_string()))}}));
                return false;
            }
            true
        }
        #[allow(clippy::too_many_arguments)]
        fn run(w: &mut std::io::PipeWriter, send: &mut dyn FnMut(&mut std::io::PipeWriter, Value), lines: &[String], bps: &[u32], opts: &HashMap<u32, (String, String)>, at: &mut u32, from: u32, step: bool) -> bool {
            let mut l = from;
            loop {
                if l as usize > lines.len() {
                    send(w, json!({"type": "event", "event": "exited", "body": {"exitCode": 0}}));
                    send(w, json!({"type": "event", "event": "terminated"}));
                    return false;
                }
                if (step && l == from) || (!step && l != *at && hits(w, send, bps, opts, l)) || (step && l > from) {
                    if !(step && l == from) {
                        *at = l;
                        let reason = if step { "step" } else { "breakpoint" };
                        send(w, json!({"type": "event", "event": "stopped", "body": {"reason": reason, "threadId": 1}}));
                        return true;
                    }
                }
                // Execute line l.
                let text = &lines[l as usize - 1];
                if let Some(i) = text.find("print(") {
                    let inner = text[i + 6..].split(')').next().unwrap_or("").trim_matches('"');
                    send(w, json!({"type": "event", "event": "output", "body": {"category": "stdout", "output": format!("{inner}\n")}}));
                }
                l += 1;
                if !step && hits(w, send, bps, opts, l) {
                    *at = l;
                    send(w, json!({"type": "event", "event": "stopped", "body": {"reason": "breakpoint", "threadId": 1}}));
                    return true;
                }
            }
        }
        while let Ok(Some(m)) = read_message(&mut r) {
            let cmd = m["command"].as_str().unwrap_or("").to_string();
            let a = &m["arguments"];
            let ok = |body: Value| json!({"type": "response", "request_seq": m["seq"], "success": true, "command": cmd, "body": body});
            match cmd.as_str() {
                "initialize" => send(&mut w, ok(json!({"supportsConfigurationDoneRequest": true, "supportsConditionalBreakpoints": true, "supportsLogPoints": true, "supportsSetExpression": true}))),
                "attach" => {
                    send(&mut w, ok(json!({})));
                    send(&mut w, json!({"type": "event", "event": "initialized"}));
                }
                "evaluate" | "setExpression" => {
                    let e = a["expression"].as_str().unwrap_or("").trim().to_string();
                    let assign = if cmd == "setExpression" { Some((e.clone(), a["value"].as_str().unwrap_or("").to_string())) } else { e.split_once(" = ").map(|(n, v)| (n.trim().to_string(), v.trim().to_string())) };
                    if let Some((name, value)) = assign {
                        vars.insert(name, value.clone());
                        send(&mut w, ok(json!({"result": value, "value": value, "type": "int", "variablesReference": 0})));
                    } else if e == "line" {
                        send(&mut w, ok(json!({"result": at.to_string(), "type": "int", "variablesReference": 0})));
                    } else if e == "text" {
                        let t = lines.get((at as usize).wrapping_sub(1)).cloned().unwrap_or_default();
                        send(&mut w, ok(json!({"result": format!("{t:?}"), "type": "str", "variablesReference": 0})));
                    } else if e == "point" {
                        send(&mut w, ok(json!({"result": "Point(x=1, y=2)", "type": "Point", "variablesReference": 8})));
                    } else if let Some(v) = vars.get(&e) {
                        send(&mut w, ok(json!({"result": v, "type": "int", "variablesReference": 0})));
                    } else {
                        send(&mut w, json!({"type": "response", "request_seq": m["seq"], "success": false, "command": cmd, "message": format!("name '{e}' is not defined")}));
                    }
                }
                "launch" => {
                    program = a["program"].as_str().unwrap_or("").to_string();
                    stop_on_entry = a["stopOnEntry"].as_bool().unwrap_or(false);
                    lines = std::fs::read_to_string(&program).unwrap_or_default().lines().map(str::to_string).collect();
                    send(&mut w, ok(json!({})));
                    send(&mut w, json!({"type": "event", "event": "initialized"}));
                }
                "setBreakpoints" => {
                    bps = a["breakpoints"].as_array().map(|b| b.iter().filter_map(|x| x["line"].as_u64().map(|l| l as u32)).collect()).unwrap_or_default();
                    opts = a["breakpoints"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|x| {
                            let (c, m) = (x["condition"].as_str().unwrap_or(""), x["logMessage"].as_str().unwrap_or(""));
                            (!c.is_empty() || !m.is_empty()).then(|| (x["line"].as_u64().unwrap_or(0) as u32, (c.to_string(), m.to_string())))
                        })
                        .collect();
                    let out: Vec<Value> = bps.iter().map(|l| json!({"verified": true, "line": l})).collect();
                    send(&mut w, ok(json!({"breakpoints": out})));
                }
                "configurationDone" => {
                    send(&mut w, ok(json!({})));
                    if stop_on_entry {
                        at = 1;
                        send(&mut w, json!({"type": "event", "event": "stopped", "body": {"reason": "entry", "threadId": 1}}));
                    } else if bps.contains(&1) {
                        at = 1;
                        send(&mut w, json!({"type": "event", "event": "stopped", "body": {"reason": "breakpoint", "threadId": 1}}));
                    } else {
                        let mut a2 = at;
                        run(&mut w, &mut send, &lines, &bps, &opts, &mut a2, 1, false);
                        at = a2;
                    }
                }
                "threads" => send(&mut w, ok(json!({"threads": [{"id": 1, "name": "main"}]}))),
                "stackTrace" => send(&mut w, ok(json!({"stackFrames": [{"id": 1, "name": "main", "line": at, "column": 1, "source": {"path": program}}], "totalFrames": 1}))),
                "scopes" => send(&mut w, ok(json!({"scopes": [{"name": "Locals", "variablesReference": 7, "expensive": false}]}))),
                "variables" if a["variablesReference"] == 8 => {
                    send(&mut w, ok(json!({"variables": [
                        {"name": "x", "value": "1", "type": "int", "variablesReference": 0},
                        {"name": "y", "value": "2", "type": "int", "variablesReference": 0}
                    ]})));
                }
                "variables" => {
                    let text = lines.get(at as usize - 1).cloned().unwrap_or_default();
                    send(&mut w, ok(json!({"variables": [
                        {"name": "line", "value": at.to_string(), "type": "int", "variablesReference": 0},
                        {"name": "text", "value": format!("{text:?}"), "type": "str", "variablesReference": 0}
                    ]})));
                }
                "next" | "stepIn" | "stepOut" => {
                    send(&mut w, ok(json!({})));
                    let mut a2 = at;
                    run(&mut w, &mut send, &lines, &bps, &opts, &mut a2, at, true);
                    at = a2;
                }
                "continue" => {
                    send(&mut w, ok(json!({"allThreadsContinued": true})));
                    let mut a2 = at;
                    run(&mut w, &mut send, &lines, &bps, &opts, &mut a2, at, false);
                    at = a2;
                }
                "disconnect" => {
                    send(&mut w, ok(json!({})));
                    send(&mut w, json!({"type": "event", "event": "terminated"}));
                    return;
                }
                _ => send(&mut w, ok(json!({}))),
            }
        }
    })?;
    RpcProcess::from_streams("fake-dap", Box::new(client_r), Box::new(client_w), None)
}
