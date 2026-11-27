//! AI models for the editor: chat and fill-in-the-middle completion.
//!
//! On a hearth desktop the models come from hearth's AI hub (`hearth-ai`,
//! JSON lines on stdin/stdout): it lists every local server and what's
//! installed and loaded, and Turbine asks it to load a model before using
//! it, so hearth's graphics-memory arbiter stays in charge. Requests then go
//! straight to that server's OpenAI-compatible API (or Ollama's own for
//! fill-in-the-middle). Without the hub: a local Ollama, or any
//! OpenAI-compatible server named by `TURBINE_AI_URL` (+ `TURBINE_AI_KEY`,
//! `TURBINE_AI_MODEL`).
//!
//! Everything slow runs on threads; results come back as [`AiEvent`]s.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// A model somewhere Turbine can reach.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// The hub's provider id ("ollama", "llamacpp", ...) or "env"/"ollama-direct".
    pub provider: String,
    pub name: String,
    /// API base ending in /v1 (OpenAI-compatible).
    pub base: String,
    pub key: Option<String>,
    pub loaded: bool,
    /// Can fill in the middle (Ollama's "insert" capability, or a *-base coder).
    pub fim: bool,
    pub chat: bool,
}

#[derive(Debug)]
pub enum AiEvent {
    /// The hub (or a fallback) listed its models.
    Models(Vec<Model>),
    /// Part of a chat answer.
    Chunk { id: u64, text: String },
    Done { id: u64 },
    /// A whole assistant message from a tool-using chat (OpenAI format:
    /// `content`, and `tool_calls` [{id, function: {name, arguments}}]).
    Message { id: u64, message: Value },
    /// A fill-in-the-middle completion.
    Completion { id: u64, text: String },
    Error { id: u64, message: String },
    /// The hub went away (or was never there).
    HubGone(String),
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

fn api_base(url: &str) -> String {
    let u = url.trim_end_matches('/');
    if u.ends_with("/v1") || u.ends_with("/api/v1") { u.to_string() } else { format!("{u}/v1") }
}

/// Models from one hub state message.
pub fn models_from_state(st: &Value) -> Vec<Model> {
    let mut out = Vec::new();
    for p in st["providers"].as_array().cloned().unwrap_or_default() {
        if p["kind"] != "local" || p["running"] != true {
            continue;
        }
        let id = p["id"].as_str().unwrap_or("").to_string();
        let base = api_base(p["url"].as_str().unwrap_or(""));
        let loaded: Vec<String> = p["loaded"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect()).unwrap_or_default();
        for m in p["installed"].as_array().cloned().unwrap_or_default() {
            let name = m["name"].as_str().unwrap_or("").to_string();
            if name.is_empty() || m["fits"] == "DoesNotFit" {
                continue;
            }
            let caps: Vec<&str> = m["capabilities"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let lower = name.to_lowercase();
            let fim = caps.contains(&"insert") || (lower.contains("coder") && lower.contains("base"));
            let chat = !lower.contains("base") && !lower.contains("ocr") && !lower.contains("embed") && !lower.contains("translate");
            out.push(Model { provider: id.clone(), name: name.clone(), base: base.clone(), key: None, loaded: loaded.contains(&name), fim, chat });
        }
    }
    out
}

/// Pick the chat model: one already loaded, then the smallest-looking one.
pub fn pick_chat(models: &[Model]) -> Option<&Model> {
    models.iter().filter(|m| m.chat).max_by_key(|m| (m.loaded, std::cmp::Reverse(m.name.len())))
}

pub fn pick_fim(models: &[Model]) -> Option<&Model> {
    models.iter().filter(|m| m.fim).max_by_key(|m| m.loaded)
}

/// A streamed chunk with reasoning (`<think>…</think>`, possibly split
/// across chunks) removed; `in_think` carries the state between chunks.
pub fn strip_thinking(chunk: &str, in_think: &mut bool) -> String {
    let mut out = String::new();
    let mut rest = chunk;
    loop {
        if *in_think {
            match rest.find("</think>") {
                Some(i) => {
                    rest = &rest[i + 8..];
                    *in_think = false;
                }
                None => return out,
            }
        } else {
            match rest.find("<think>") {
                Some(i) => {
                    out.push_str(&rest[..i]);
                    rest = &rest[i + 7..];
                    *in_think = true;
                }
                None => {
                    out.push_str(rest);
                    return out;
                }
            }
        }
    }
}

/// The connection to hearth's AI hub, if there is one.
struct Hub {
    child: Child,
    stdin: ChildStdin,
}

pub struct Ai {
    hub: Option<Hub>,
    tx: Sender<AiEvent>,
    rx: Receiver<AiEvent>,
    next_id: u64,
    pub models: Vec<Model>,
    /// Where models came from, for the status line.
    pub source: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_connect(Some(Duration::from_secs(5))).timeout_global(Some(Duration::from_secs(600))).build().into()
}

impl Ai {
    /// Connect: the hub when `hearth-ai` is installed (and `TURBINE_AI_HUB`
    /// isn't "0"), else the fallbacks. Models arrive as an event.
    pub fn start() -> Ai {
        let (tx, rx) = mpsc::channel();
        let mut ai = Ai { hub: None, tx, rx, next_id: 1, models: Vec::new(), source: String::new() };
        let use_hub = std::env::var("TURBINE_AI_HUB").map(|v| v != "0").unwrap_or(true) && std::env::var("TURBINE_AI_URL").is_err();
        if use_hub && crate::rpc::on_path("hearth-ai") {
            if let Ok(h) = ai.spawn_hub() {
                ai.hub = Some(h);
                ai.source = "hearth AI hub".into();
                ai.refresh();
                return ai;
            }
        }
        ai.source = "local".into();
        ai.fallback_models();
        ai
    }

    /// A client that knows only these models (tests).
    pub fn with_models(models: Vec<Model>) -> Ai {
        let (tx, rx) = mpsc::channel();
        Ai { hub: None, tx, rx, next_id: 1, models, source: "test".into() }
    }

    pub fn has_hub(&self) -> bool {
        self.hub.is_some()
    }

    fn spawn_hub(&self) -> std::io::Result<Hub> {
        let mut child = Command::new("hearth-ai").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let tx = self.tx.clone();
        std::thread::Builder::new().name("hearth-ai".into()).spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if v["type"] == "state" && tx.send(AiEvent::Models(models_from_state(&v))).is_err() {
                    return;
                }
            }
            let _ = tx.send(AiEvent::HubGone("hearth-ai exited".into()));
        })?;
        Ok(Hub { child, stdin })
    }

    fn hub_send(&mut self, v: Value) {
        if let Some(h) = &mut self.hub {
            if writeln!(h.stdin, "{v}").and_then(|_| h.stdin.flush()).is_err() {
                self.hub = None;
            }
        }
    }

    /// Ask for the model list again.
    pub fn refresh(&mut self) {
        if self.hub.is_some() {
            self.hub_send(json!({"cmd": "refresh"}));
        } else {
            self.fallback_models();
        }
    }

    /// Before using a model: through the hub, so it can make room for it.
    pub fn prepare(&mut self, m: &Model) {
        if !m.loaded && self.hub.is_some() {
            let (p, n) = (m.provider.clone(), m.name.clone());
            self.hub_send(json!({"cmd": "load", "provider": p, "model": n}));
        }
    }

    /// Without the hub: TURBINE_AI_URL, else a local Ollama.
    fn fallback_models(&mut self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            if let Ok(url) = std::env::var("TURBINE_AI_URL") {
                let model = std::env::var("TURBINE_AI_MODEL").unwrap_or_else(|_| "default".into());
                let m = Model {
                    provider: "env".into(),
                    name: model,
                    base: api_base(&url),
                    key: std::env::var("TURBINE_AI_KEY").ok(),
                    loaded: true,
                    fim: false,
                    chat: true,
                };
                let _ = tx.send(AiEvent::Models(vec![m]));
                return;
            }
            let url = std::env::var("OLLAMA_HOST").map(|h| if h.starts_with("http") { h } else { format!("http://{h}") }).unwrap_or_else(|_| "http://127.0.0.1:11434".into());
            let tags: Option<Value> = agent().get(&format!("{url}/api/tags")).call().ok().and_then(|mut r| r.body_mut().read_json().ok());
            let state = json!({"providers": [{
                "id": "ollama-direct", "kind": "local", "running": tags.is_some(), "url": url,
                "installed": tags.as_ref().map(|t| t["models"].clone()).unwrap_or(json!([])), "loaded": []
            }]});
            let _ = tx.send(AiEvent::Models(models_from_state(&state)));
        });
    }

    pub fn poll(&mut self) -> Vec<AiEvent> {
        let evs: Vec<AiEvent> = self.rx.try_iter().collect();
        for e in &evs {
            match e {
                AiEvent::Models(m) => self.models = m.clone(),
                AiEvent::HubGone(_) => self.hub = None,
                _ => {}
            }
        }
        evs
    }

    /// Stream a chat answer. Returns the request id.
    pub fn chat(&mut self, model: &Model, messages: Vec<ChatMessage>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.prepare(model);
        let tx = self.tx.clone();
        let m = model.clone();
        std::thread::spawn(move || {
            let msgs: Vec<Value> = messages.iter().map(|x| json!({"role": x.role, "content": x.content})).collect();
            let mut body = json!({"model": m.name, "messages": msgs, "stream": true});
            // (Qwen 3.5 on Ollama thinks for a long time unless told not to.)
            if m.provider.starts_with("ollama") {
                body["reasoning_effort"] = json!("none");
            }
            let mut req = agent().post(&format!("{}/chat/completions", m.base)).header("Content-Type", "application/json");
            if let Some(k) = &m.key {
                req = req.header("Authorization", &format!("Bearer {k}"));
            }
            let resp = req.send_json(&body);
            let mut resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(AiEvent::Error { id, message: e.to_string() });
                    return;
                }
            };
            let reader = BufReader::new(resp.body_mut().as_reader());
            let mut in_think = false;
            for line in reader.lines().map_while(Result::ok) {
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    break;
                }
                let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
                let raw = v["choices"][0]["delta"]["content"].as_str().unwrap_or("");
                // Drop <think>…</think> reasoning some models still emit.
                let text = strip_thinking(raw, &mut in_think);
                if !text.is_empty() && tx.send(AiEvent::Chunk { id, text }).is_err() {
                    return;
                }
            }
            let _ = tx.send(AiEvent::Done { id });
        });
        id
    }

    /// One turn of a chat where the model may call tools (not streamed).
    /// `messages` are OpenAI-format message objects. Returns the request id.
    pub fn chat_with_tools(&mut self, model: &Model, messages: Vec<Value>, tools: Vec<Value>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.prepare(model);
        let tx = self.tx.clone();
        let m = model.clone();
        std::thread::spawn(move || {
            let mut body = json!({"model": m.name, "messages": messages, "tools": tools, "stream": false});
            if m.provider.starts_with("ollama") {
                body["reasoning_effort"] = json!("none");
            }
            let mut req = agent().post(&format!("{}/chat/completions", m.base)).header("Content-Type", "application/json");
            if let Some(k) = &m.key {
                req = req.header("Authorization", &format!("Bearer {k}"));
            }
            let started = std::time::Instant::now();
            let r = req
                .send_json(&body)
                .map_err(|e| e.to_string())
                .and_then(|mut r| r.body_mut().read_json::<Value>().map_err(|e| e.to_string()));
            // TURBINE_AI_LOG=file: every tool-using turn, for debugging agents.
            if let Ok(path) = std::env::var("TURBINE_AI_LOG") {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    let reply = match &r { Ok(v) => v.clone(), Err(e) => json!({"error": e}) };
                    let _ = writeln!(f, "{}", json!({"secs": started.elapsed().as_secs_f32(), "request": body, "reply": reply}));
                }
            }
            let _ = tx.send(match r {
                Ok(v) => {
                    let mut msg = v["choices"][0]["message"].clone();
                    if msg.is_null() {
                        AiEvent::Error { id, message: format!("no message in reply: {v}") }
                    } else {
                        let mut t = false;
                        let content = strip_thinking(msg["content"].as_str().unwrap_or(""), &mut t);
                        msg["content"] = json!(content.trim());
                        AiEvent::Message { id, message: msg }
                    }
                }
                Err(message) => AiEvent::Error { id, message },
            });
        });
        id
    }

    /// Fill in between `prefix` and `suffix`. Returns the request id.
    pub fn complete(&mut self, model: &Model, prefix: String, suffix: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.prepare(model);
        let tx = self.tx.clone();
        let m = model.clone();
        std::thread::spawn(move || {
            let result: Result<String, String> = if m.provider.starts_with("ollama") {
                // Ollama's own API does FIM with the model's template.
                let root = m.base.trim_end_matches("/v1").to_string();
                let body = json!({"model": m.name, "prompt": prefix, "suffix": suffix, "stream": false, "raw": false,
                                  "options": {"num_predict": 64, "temperature": 0.1, "stop": ["\n\n\n"]}});
                agent()
                    .post(&format!("{root}/api/generate"))
                    .send_json(&body)
                    .map_err(|e| e.to_string())
                    .and_then(|mut r| r.body_mut().read_json::<Value>().map_err(|e| e.to_string()))
                    .map(|v| v["response"].as_str().unwrap_or("").to_string())
            } else {
                let body = json!({"model": m.name, "prompt": prefix, "suffix": suffix, "max_tokens": 64, "temperature": 0.1});
                let mut req = agent().post(&format!("{}/completions", m.base));
                if let Some(k) = &m.key {
                    req = req.header("Authorization", &format!("Bearer {k}"));
                }
                req.send_json(&body)
                    .map_err(|e| e.to_string())
                    .and_then(|mut r| r.body_mut().read_json::<Value>().map_err(|e| e.to_string()))
                    .map(|v| v["choices"][0]["text"].as_str().unwrap_or("").to_string())
            };
            let _ = tx.send(match result {
                Ok(text) => AiEvent::Completion { id, text },
                Err(message) => AiEvent::Error { id, message },
            });
        });
        id
    }
}

impl Drop for Ai {
    fn drop(&mut self) {
        if let Some(h) = &mut self.hub {
            let _ = h.child.kill();
            let _ = h.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_is_dropped_across_chunks() {
        let mut t = false;
        let parts = ["Hel", "lo <think>a", "b</think> wo", "rld <think>x</think>!"];
        let out: String = parts.iter().map(|p| strip_thinking(p, &mut t)).collect();
        assert_eq!(out, "Hello  world !");
    }

    #[test]
    fn hub_state_to_models() {
        let st = json!({"type": "state", "providers": [
            {"id": "ollama", "kind": "local", "url": "http://127.0.0.1:11434", "running": true,
             "installed": [
                {"name": "qwen2.5-coder:1.5b-base", "capabilities": ["completion", "insert"]},
                {"name": "qwen3.5:4b", "capabilities": ["completion", "tools"]},
                {"name": "glm-ocr:latest", "capabilities": ["completion"]}],
             "loaded": [{"name": "qwen3.5:4b"}]},
            {"id": "llamacpp", "kind": "local", "url": "http://127.0.0.1:8181", "running": false, "installed": [], "loaded": []},
            {"id": "magnitude", "kind": "local", "url": "http://127.0.0.1:10100/api/v1", "running": true,
             "installed": [{"name": "big", "fits": "DoesNotFit"}], "loaded": []}
        ]});
        let ms = models_from_state(&st);
        assert_eq!(ms.len(), 3);
        assert_eq!(ms[0].base, "http://127.0.0.1:11434/v1");
        assert_eq!(pick_fim(&ms).unwrap().name, "qwen2.5-coder:1.5b-base");
        assert_eq!(pick_chat(&ms).unwrap().name, "qwen3.5:4b");
        assert!(!ms.iter().any(|m| m.name == "big"));
    }
}
