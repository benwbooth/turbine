//! The automation endpoint: JSON lines over a Unix socket, and scripts.
//!
//! Each line is a command such as `{"cmd":"menu","path":"File/Open..."}`;
//! each gets one JSON reply line when its action has finished on screen.

use crate::workbench::Request;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock};

fn send(tx: &Sender<Request>, line: &str) -> Value {
    let cmd: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({"ok": false, "error": format!("bad JSON: {e}")}),
    };
    let (rtx, rrx) = mpsc::channel();
    if tx.send(Request { cmd, reply: rtx }).is_err() {
        return json!({"ok": false, "error": "turbine is closing"});
    }
    rrx.recv().unwrap_or_else(|_| json!({"ok": false, "error": "no reply"}))
}

#[cfg(unix)]
pub fn listen(path: Option<PathBuf>, tx: Sender<Request>) -> std::io::Result<PathBuf> {
    use std::os::unix::net::UnixListener;
    let path = path.unwrap_or_else(|| {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        dir.join(format!("turbine-{}.sock", std::process::id()))
    });
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let Ok(mut w) = conn.try_clone() else { return };
                for line in BufReader::new(conn).lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let reply = send(&tx, &line);
                    if writeln!(w, "{reply}").is_err() {
                        break;
                    }
                }
            });
        }
    });
    Ok(path)
}

#[cfg(not(unix))]
pub fn listen(_path: Option<PathBuf>, _tx: Sender<Request>) -> std::io::Result<PathBuf> {
    Err(std::io::Error::other("the automation socket needs Unix sockets (TCP coming)"))
}

static DONE: OnceLock<Arc<AtomicBool>> = OnceLock::new();

pub fn script_done() -> Arc<AtomicBool> {
    DONE.get_or_init(|| Arc::new(AtomicBool::new(false))).clone()
}

/// Run each line of a script in order, waiting for each to finish.
pub fn run_script(path: PathBuf, tx: Sender<Request>) {
    let done = script_done();
    std::thread::spawn(move || {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("//")) {
                    let r = send(&tx, line);
                    if r["ok"] != true {
                        eprintln!("turbine: script: {line}: {r}");
                    }
                }
            }
            Err(e) => eprintln!("turbine: script {}: {e}", path.display()),
        }
        done.store(true, Ordering::SeqCst);
    });
}
