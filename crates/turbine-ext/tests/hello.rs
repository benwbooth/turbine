//! The example extension (extensions/hello), built for WebAssembly, packed
//! and run in the host: it adds its menu items, and Uppercase Line reads
//! and edits the document through the host's calls.

use std::path::PathBuf;
use std::time::{Duration, Instant};
use turbine_ext::{Answer, ExtHost, HostCall, Request};

fn build_hello() -> Vec<u8> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extensions/hello");
    let st = std::process::Command::new(env!("CARGO"))
        .args(["build", "--quiet", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .status()
        .expect("cargo");
    assert!(st.success(), "building extensions/hello");
    std::fs::read(root.join("target/wasm32-unknown-unknown/release/turbine_hello.wasm")).unwrap()
}

/// Answer the extension's calls (as the editor would) until `done` says so.
fn serve(host: &ExtHost, mut answer: impl FnMut(HostCall) -> bool) {
    let t = Instant::now();
    loop {
        for c in host.poll() {
            if answer(c) {
                return;
            }
        }
        assert!(t.elapsed() < Duration::from_secs(60), "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn hello_adds_menu_items_and_edits_a_line() {
    let component = turbine_ext::pack(&build_hello()).expect("pack");
    let dir = std::env::temp_dir().join(format!("turbine-ext-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("hello")).unwrap();
    std::fs::write(dir.join("hello/extension.wasm"), component).unwrap();
    std::fs::write(dir.join("hello/README.md"), "# Says hello\n").unwrap();

    let host = ExtHost::start(vec![dir.clone()]);
    let mut items = Vec::new();
    let mut keys = Vec::new();
    serve(&host, |c| {
        let done = matches!(&c.request, Request::Log(m) if m == "hello: ready");
        match &c.request {
            Request::AddMenuItem { menu, label, command, .. } => items.push((menu.clone(), label.clone(), command.clone())),
            Request::BindKey { chord, command, .. } => keys.push((chord.clone(), command.clone())),
            Request::Finished { result: Err(e), .. } => panic!("{e}"),
            _ => {}
        }
        c.reply(Answer::None);
        done
    });
    assert_eq!(items[0], ("Tools".into(), "~H~ello from WASI".into(), "hello.greet".into()));
    assert_eq!(keys, vec![("Ctrl+Alt+U".to_string(), "hello.upper".to_string())]);
    let list = host.list();
    assert!(list[0].loaded && list[0].description == "Says hello", "{list:?}");

    // Uppercase Line: it asks for the document, the cursor and the text,
    // then sends the edit.
    host.run("hello", "hello.upper", "");
    let mut edit = None;
    serve(&host, |c| match &c.request {
        Request::ActiveDocument => {
            c.reply(Answer::Doc(Some(7)));
            false
        }
        Request::Cursor(7) => {
            c.reply(Answer::Pos(1, 2));
            false
        }
        Request::DocumentText(7) => {
            c.reply(Answer::Text("first\nsecond line\n".into()));
            false
        }
        Request::Edit { doc: 7, start, end, text } => {
            edit = Some((*start, *end, text.clone()));
            c.reply(Answer::None);
            false
        }
        Request::Finished { command, result } => {
            assert_eq!((command.as_str(), result), ("hello.upper", &Ok(String::new())));
            true
        }
        other => panic!("{other:?}"),
    });
    assert_eq!(edit, Some(((1, 0), (1, 11), "SECOND LINE".to_string())));

    // A greeting comes back as the result.
    host.run("hello", "hello.greet", "");
    serve(&host, |c| matches!(&c.request, Request::Finished { result: Ok(m), .. } if m.contains("Hello")));
    std::fs::remove_dir_all(&dir).unwrap();
}
