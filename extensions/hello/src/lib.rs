//! An example Turbine extension: Tools > Hello from WASI, and Tools >
//! Uppercase Line (Ctrl+Alt+U), which reads and edits the document through
//! the host.

wit_bindgen::generate!({ path: "../../wit/turbine.wit", world: "extension" });

use turbine::ext::host;
use turbine::ext::types::{MenuItem, Position, Range};

struct Hello;

impl Guest for Hello {
    fn activate() {
        host::add_menu_item("Tools", &MenuItem { label: "~H~ello from WASI".into(), shortcut: None, command: "hello.greet".into() });
        host::add_menu_item(
            "Tools",
            &MenuItem { label: "~U~ppercase Line".into(), shortcut: Some("Ctrl+Alt+U".into()), command: "hello.upper".into() },
        );
        host::bind_key("Ctrl+Alt+U", "hello.upper", None);
        host::log("hello: ready");
    }

    fn run(command: String, _args: String) -> Result<String, String> {
        match command.as_str() {
            "hello.greet" => Ok("Hello from a WebAssembly extension!".into()),
            "hello.upper" => {
                let doc = host::active_document().ok_or("No document open")?;
                let at = host::cursor(doc);
                let text = host::document_text(doc);
                let line = text.lines().nth(at.line as usize).unwrap_or("");
                let end = line.chars().count() as u32;
                host::edit(doc, Range { start: Position { line: at.line, column: 0 }, end: Position { line: at.line, column: end } }, &line.to_uppercase());
                Ok(String::new())
            }
            _ => Err(format!("hello: no command {command}")),
        }
    }
}

export!(Hello);
