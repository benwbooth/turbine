//! The tools an AI gets to work Turbine, for Turbine's own agent and for
//! outside agents over MCP. Each is one automation command, so whatever an
//! agent does goes through the same paced, visible input a person's does.

use serde_json::{Value, json};

/// Tool definitions (name, description, JSON Schema for arguments).
pub fn specs() -> Vec<Value> {
    let t = |name: &str, desc: &str, props: Value, required: &[&str]| {
        json!({"name": name, "description": desc, "parameters": {"type": "object", "properties": props, "required": required}})
    };
    vec![
        t(
            "look",
            "See the screen: an outline of every widget (id, role, name, value; * marks focus) and the screen as text. Call it first, and after anything that changes the screen.",
            json!({}),
            &[],
        ),
        t(
            "menu",
            "Open a menu item by its path, e.g. \"File/Open File...\" or \"Window/Tile\". Menus: File Edit Selection Search View Go Run Terminal AI Options Window Help.",
            json!({"path": {"type": "string"}}),
            &["path"],
        ),
        t(
            "click",
            "Click a widget: by id from look (\"button:OK\" style role:name works too), or at x,y.",
            json!({"target": {"type": "string"}, "x": {"type": "integer"}, "y": {"type": "integer"}}),
            &[],
        ),
        t(
            "type",
            "Type text into whatever has focus (the editor, a dialog's field, a terminal). Newlines are typed as Enter.",
            json!({"text": {"type": "string"}}),
            &["text"],
        ),
        t(
            "key",
            "Press a key or chord: \"Enter\", \"Esc\", \"Tab\", \"Ctrl+S\", \"Alt+F\", \"F2\", \"Shift+Down\", \"Ctrl+Home\".",
            json!({"key": {"type": "string"}}),
            &["key"],
        ),
        t(
            "read",
            "Read the whole text of the active window (a file being edited, a terminal's screen, a help page).",
            json!({}),
            &[],
        ),
        t(
            "wait",
            "Wait until the active window shows some text (a build finishing, a prompt), up to timeout_ms.",
            json!({"text": {"type": "string"}, "timeout_ms": {"type": "integer"}}),
            &["text"],
        ),
        t("open", "Open a file in a new editor window.", json!({"path": {"type": "string"}}), &["path"]),
    ]
}

/// Tools in the OpenAI chat format.
pub fn openai_tools() -> Vec<Value> {
    specs().into_iter().map(|s| json!({"type": "function", "function": s})).collect()
}

/// Tools in MCP's format.
pub fn mcp_tools() -> Vec<Value> {
    specs()
        .into_iter()
        .map(|s| json!({"name": s["name"], "description": s["description"], "inputSchema": s["parameters"]}))
        .collect()
}

/// The automation command for a tool call.
pub fn to_command(name: &str, args: &Value) -> Result<Value, String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    Ok(match name {
        "look" => json!({"cmd": "snapshot", "screen": true}),
        "menu" => json!({"cmd": "menu", "path": s("path").ok_or("menu needs a path")?}),
        "click" => {
            let mut c = json!({"cmd": "click"});
            if let Some(t) = s("target") {
                c["target"] = json!(t);
            } else if let (Some(x), Some(y)) = (args.get("x"), args.get("y")) {
                c["x"] = x.clone();
                c["y"] = y.clone();
            } else {
                return Err("click needs a target or x and y".into());
            }
            c
        }
        "type" => json!({"cmd": "type", "text": s("text").ok_or("type needs text")?}),
        "key" => json!({"cmd": "key", "key": s("key").ok_or("key needs a key")?}),
        "read" => json!({"cmd": "text"}),
        "wait" => json!({"cmd": "wait", "text": s("text").ok_or("wait needs text")?, "timeout_ms": args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(10_000)}),
        "open" => json!({"cmd": "open", "path": s("path").ok_or("open needs a path")?}),
        other => return Err(format!("no tool named {other}")),
    })
}

/// A widget tree as a compact outline, one widget per line.
pub fn outline(node: &Value, depth: usize, out: &mut String) {
    let role = node["role"].as_str().unwrap_or("");
    // (The status bar's key hints and window buttons are noise for a model.)
    let skip_children = role == "statusbar";
    let mut line = format!("{}{} [{}] {}", "  ".repeat(depth), node["id"].as_str().unwrap_or(""), role, node["name"].as_str().unwrap_or(""));
    if let Some(v) = node.get("value").and_then(Value::as_str) {
        let v: String = v.chars().take(80).collect();
        line.push_str(&format!(" = {v:?}"));
    }
    if node.get("focused").and_then(Value::as_bool) == Some(true) {
        line.push_str(" *");
    }
    if node.get("enabled").and_then(Value::as_bool) == Some(false) {
        line.push_str(" (disabled)");
    }
    out.push_str(&line);
    out.push('\n');
    if !skip_children {
        for c in node["children"].as_array().into_iter().flatten() {
            if role == "window" && c["role"] == "button" {
                continue;
            }
            outline(c, depth + 1, out);
        }
    }
}

/// What a tool's automation reply says, as text for the model.
pub fn result_text(name: &str, reply: &Value) -> String {
    if reply["ok"] != true {
        return format!("error: {}", reply["error"].as_str().unwrap_or("failed"));
    }
    match name {
        "look" => {
            let mut o = String::from("Widgets:\n");
            outline(&reply["tree"], 0, &mut o);
            let focus = reply["focus"].as_str().map(|f| format!("\nNow: {f}\n")).unwrap_or_default();
            format!("{o}{focus}\nScreen:\n{}", reply["screen"].as_str().unwrap_or(""))
        }
        "read" => {
            let t = reply["text"].as_str().unwrap_or("");
            if t.len() > 12_000 { format!("{}\n... (cut: {} bytes in all)", &t[..t.floor_char_boundary(12_000)], t.len()) } else { t.to_string() }
        }
        _ => match reply["focus"].as_str() {
            Some(f) => format!("done. Now: {f}"),
            None => "done".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_map_to_commands() {
        assert_eq!(to_command("menu", &json!({"path": "File/Save"})).unwrap()["path"], "File/Save");
        assert_eq!(to_command("click", &json!({"x": 3, "y": 4})).unwrap()["x"], 3);
        assert!(to_command("click", &json!({})).is_err());
        assert!(to_command("rm_rf", &json!({})).is_err());
        assert_eq!(openai_tools().len(), specs().len());
        assert_eq!(mcp_tools()[0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn outline_is_compact() {
        let tree = json!({"id": "workbench", "role": "application", "name": "Turbine", "children": [
            {"id": "window.1", "role": "window", "name": "a.rs", "focused": true, "children": [
                {"id": "window.1.close", "role": "button", "name": "Close"},
                {"id": "editor.1", "role": "editor", "name": "a.rs", "value": "line 1", "focused": true}]},
            {"id": "statusbar", "role": "statusbar", "name": "Status", "children": [{"id": "status.x", "role": "button", "name": "F1 Help"}]}]});
        let mut o = String::new();
        outline(&tree, 0, &mut o);
        assert!(o.contains("  editor.1 [editor] a.rs = \"line 1\" *"));
        assert!(!o.contains("Close") && !o.contains("F1 Help"));
    }
}
