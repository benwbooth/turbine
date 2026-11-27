//! A program spoken to with JSON-RPC over its stdin/stdout, framed with
//! `Content-Length` headers (language servers and debug adapters both).
//!
//! A writer thread and a reader thread do the I/O; the UI thread only
//! touches channels, so nothing here ever blocks it.

use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

#[derive(Debug)]
pub enum Incoming {
    Message(Value),
    /// A line the program wrote to stderr (its log).
    Log(String),
    /// The program is gone (and why).
    Exited(String),
}

pub struct RpcProcess {
    /// None when talking over in-process pipes (tests, built-in servers).
    child: Option<Child>,
    out: Sender<Value>,
    inc: Receiver<Incoming>,
    pub name: String,
}

/// Is a program on PATH?
pub fn on_path(name: &str) -> bool {
    if Path::new(name).is_absolute() {
        return Path::new(name).is_file();
    }
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| d.join(name).is_file() || d.join(format!("{name}.exe")).is_file())
    })
}

/// Read one Content-Length framed message.
pub fn read_message(r: &mut impl BufRead) -> std::io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if len.is_some() {
                break;
            }
            continue;
        }
        if let Some(v) = line.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length")) {
            len = v.1.trim().parse().ok();
        }
    }
    let mut buf = vec![0u8; len.unwrap_or(0)];
    r.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf).unwrap_or(Value::Null)))
}

pub fn write_message(w: &mut impl Write, v: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(v).unwrap_or_default();
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

impl RpcProcess {
    pub fn spawn(argv: &[String], cwd: Option<&Path>) -> std::io::Result<RpcProcess> {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(d) = cwd {
            cmd.current_dir(d);
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let name = argv[0].rsplit(['/', '\\']).next().unwrap_or(&argv[0]).to_string();
        let mut p = Self::from_streams(&name, Box::new(stdout), Box::new(stdin), Some(Box::new(stderr)))?;
        p.child = Some(child);
        Ok(p)
    }

    /// Talk over any pair of streams (an in-process server in tests).
    pub fn from_streams(
        name: &str,
        stdout: Box<dyn Read + Send>,
        mut stdin: Box<dyn Write + Send>,
        stderr: Option<Box<dyn Read + Send>>,
    ) -> std::io::Result<RpcProcess> {
        let name = name.to_string();
        let (out, out_rx) = mpsc::channel::<Value>();
        let (inc_tx, inc) = mpsc::channel();
        std::thread::Builder::new().name(format!("rpc-w-{name}")).spawn(move || {
            for v in out_rx {
                if write_message(&mut stdin, &v).is_err() {
                    break;
                }
            }
        })?;
        let tx = inc_tx.clone();
        std::thread::Builder::new().name(format!("rpc-r-{name}")).spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                match read_message(&mut r) {
                    Ok(Some(v)) => {
                        if tx.send(Incoming::Message(v)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {
                        let _ = tx.send(Incoming::Exited("closed its output".into()));
                        return;
                    }
                    Err(e) => {
                        let _ = tx.send(Incoming::Exited(e.to_string()));
                        return;
                    }
                }
            }
        })?;
        if let Some(stderr) = stderr {
            std::thread::Builder::new().name(format!("rpc-e-{name}")).spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if inc_tx.send(Incoming::Log(line)).is_err() {
                        return;
                    }
                }
            })?;
        }
        Ok(RpcProcess { child: None, out, inc, name })
    }

    pub fn send(&self, v: Value) {
        let _ = self.out.send(v);
    }

    pub fn incoming(&self) -> Vec<Incoming> {
        self.inc.try_iter().collect()
    }
}

impl Drop for RpcProcess {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn framing_round_trips() {
        let mut buf = Vec::new();
        write_message(&mut buf, &json!({"a": "é"})).unwrap();
        write_message(&mut buf, &json!([1, 2])).unwrap();
        let mut r = std::io::Cursor::new(buf);
        assert_eq!(read_message(&mut r).unwrap(), Some(json!({"a": "é"})));
        assert_eq!(read_message(&mut r).unwrap(), Some(json!([1, 2])));
        assert_eq!(read_message(&mut r).unwrap(), None);
    }
}
