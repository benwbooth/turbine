//! AI models for the editor: chat and fill-in-the-middle completion.
//!
//! Two ways to reach them:
//!
//! - **The hearth AI hub** (on a hearth desktop): its OpenAI-compatible app
//!   API, address and token in `$XDG_RUNTIME_DIR/hearth/ai-api.json`. It
//!   lists every model the desktop has (local servers, hearth's model
//!   services such as Inkling, and Claude Code / Codex / Gemini when cloud AI
//!   is allowed), "default" and "fast" first (Settings > AI > Apps), and makes
//!   room in graphics memory before each request, so it stays in charge.
//! - **Standalone**: Turbine finds them itself: a local Ollama, a llama.cpp
//!   server, any OpenAI-compatible server named by `TURBINE_AI_URL`
//!   (+ `TURBINE_AI_KEY`, `TURBINE_AI_MODEL`), and the cloud APIs whose keys
//!   are set (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`,
//!   `ZAI_API_KEY` for GLM). `TURBINE_AI_HUB=0` forces this.
//!
//! How hard a reasoning model thinks is [`Ai::thinking`].
//! Everything slow runs on threads; results come back as [`AiEvent`]s.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// A model somewhere Turbine can reach.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Model {
    /// The hub's provider id ("ollama", "service:…", "claude", ...), or
    /// standalone's ("ollama-direct", "llamacpp", "env", "openai", ...).
    pub provider: String,
    /// The name requests use (through the hub: its id, "<provider>/<model>").
    pub name: String,
    /// The name shown ("Inkling 2 Mini", "qwen3.5:4b (Ollama)").
    pub label: String,
    /// It runs in the cloud (what's typed leaves the computer).
    pub cloud: bool,
    /// Reached through the hub's app API.
    pub via_hub: bool,
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
    /// Part of a reasoning model's thinking (shown as it thinks).
    Thinking { id: u64, text: String },
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
        // (A service the hub knows starts when asked, socket-activated:
        // its models count while it's stopped.)
        let service = p["service"] == true;
        if p["kind"] != "local" || (p["running"] != true && !service) {
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
            let label = m["label"].as_str().map(str::to_string).unwrap_or_else(|| format!("{name} ({})", p["label"].as_str().unwrap_or(&id)));
            out.push(Model { provider: id.clone(), name: name.clone(), label, base: base.clone(), key: None, loaded: loaded.contains(&name), fim, chat, ..Default::default() });
        }
    }
    out
}

/// Chat models best first: the hub's own (Inkling), then other dedicated
/// services; anything Ollama has after.
const PREFERRED: [&str; 2] = ["inkling", "qwen3.8"];

/// Pick the chat model: a preferred one (Inkling first), then a dedicated
/// service, then one already loaded, then the smallest-looking one.
pub fn pick_chat(models: &[Model]) -> Option<&Model> {
    let rank = |m: &Model| {
        let lower = m.name.to_lowercase();
        let pref = PREFERRED.iter().position(|p| lower.contains(p)).map_or(0, |i| PREFERRED.len() - i);
        (pref, m.provider.starts_with("service:"), m.loaded, std::cmp::Reverse(m.name.len()))
    };
    models.iter().filter(|m| m.chat).max_by_key(|m| rank(m))
}

/// Whether a suggestion is in the text's own scripts: one with CJK,
/// Cyrillic or other letters the file doesn't use (a base model's ramble)
/// is dropped, as is one that's mostly symbols no code has.
pub fn fits_text(suggestion: &str, doc: &str) -> bool {
    fn script(c: char) -> u8 {
        match c as u32 {
            0..=0x24F => 0,
            0x370..=0x3FF => 1,
            0x400..=0x52F => 2,
            0x590..=0x6FF => 3,
            0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF => 4,
            _ => 5,
        }
    }
    let used: std::collections::HashSet<u8> = doc.chars().filter(|c| c.is_alphabetic()).map(script).collect();
    let mut foreign = 0;
    let mut letters = 0;
    for c in suggestion.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        let sc = script(c);
        if sc != 0 && !used.contains(&sc) {
            foreign += 1;
        }
    }
    foreign == 0 || (letters > 0 && foreign * 10 < letters && used.len() > 1)
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

/// The hub's app API: (base ending in /v1, token).
#[derive(Clone, Debug)]
struct HubApi {
    base: String,
    token: String,
}

pub struct Ai {
    hub: Option<HubApi>,
    /// How hard reasoning models think: "off", "low", "medium", "high";
    /// None leaves it to the model (Ollama's: off, as thinking aloud is slow).
    pub thinking: Option<String>,
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

/// The hub's app API, when hearth's hub runs (its file says where).
fn hub_api() -> Option<HubApi> {
    let file = std::env::var("HEARTH_AI_API_FILE").ok().map(std::path::PathBuf::from).or_else(|| {
        let run = std::env::var("XDG_RUNTIME_DIR").ok()?;
        Some(std::path::Path::new(&run).join("hearth").join("ai-api.json"))
    })?;
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    Some(HubApi { base: api_base(v["url"].as_str()?), token: v["token"].as_str()?.to_string() })
}

/// Standalone: what this computer has, and the clouds whose keys are set.
fn standalone_models() -> Vec<Model> {
    let mut out = Vec::new();
    if let Ok(url) = std::env::var("TURBINE_AI_URL") {
        let base = api_base(&url);
        let key = std::env::var("TURBINE_AI_KEY").ok();
        match std::env::var("TURBINE_AI_MODEL") {
            Ok(name) => out.push(Model { provider: "env".into(), label: name.clone(), name, base, key, loaded: true, chat: true, ..Default::default() }),
            Err(_) => out.extend(list_models("env", "TURBINE_AI_URL", &base, key.as_deref(), false)),
        }
    }
    let ollama = std::env::var("OLLAMA_HOST").map(|h| if h.starts_with("http") { h } else { format!("http://{h}") }).unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let tags: Option<Value> = agent().get(&format!("{ollama}/api/tags")).call().ok().and_then(|mut r| r.body_mut().read_json().ok());
    if let Some(t) = tags {
        let state = json!({"providers": [{"id": "ollama-direct", "label": "Ollama", "kind": "local", "running": true, "url": ollama, "installed": t["models"].clone(), "loaded": []}]});
        out.extend(models_from_state(&state));
    }
    let llama = std::env::var("TURBINE_LLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:8181".into());
    out.extend(list_models("llamacpp", "llama.cpp", &api_base(&llama), None, false));
    for (id, label, base, vars) in CLOUD {
        if let Some(key) = vars.iter().find_map(|v| std::env::var(v).ok().filter(|k| !k.is_empty())) {
            out.extend(list_models(id, label, base, Some(&key), true));
        }
    }
    out
}

/// The hub's model list (GET /v1/models) as models.
pub fn models_from_hub(v: &Value, base: &str, token: &str) -> Vec<Model> {
    let mut out = Vec::new();
    for m in v["data"].as_array().cloned().unwrap_or_default() {
        let id = m["id"].as_str().unwrap_or("").to_string();
        let h = &m["hearth"];
        if id.is_empty() || h["available"] == false {
            continue;
        }
        let inner = h["model"].as_str().unwrap_or(&id).to_lowercase();
        let mut label = h["label"].as_str().unwrap_or(&id).to_string();
        // ("default" and "fast": Settings > AI > Apps's picks.)
        if id == "default" || id == "fast" {
            label = format!("hearth {id}: {label}");
        }
        let fim = inner.contains("coder") && inner.contains("base");
        let chat = !inner.contains("base") && !inner.contains("ocr") && !inner.contains("embed") && !inner.contains("translate");
        out.push(Model {
            provider: h["provider"].as_str().unwrap_or("hub").to_string(),
            name: id,
            label,
            cloud: h["cloud"] == true,
            via_hub: true,
            base: base.to_string(),
            key: Some(token.to_string()),
            loaded: false,
            fim,
            chat,
        });
    }
    out
}

/// The cloud APIs Turbine can use standalone: (provider, label, base, key's variables).
const CLOUD: [(&str, &str, &str, &[&str]); 4] = [
    ("openai", "OpenAI", "https://api.openai.com/v1", &["OPENAI_API_KEY"]),
    ("anthropic", "Claude", "https://api.anthropic.com/v1", &["ANTHROPIC_API_KEY"]),
    ("gemini", "Gemini", "https://generativelanguage.googleapis.com/v1beta/openai", &["GEMINI_API_KEY", "GOOGLE_API_KEY"]),
    ("glm", "GLM", "https://api.z.ai/api/paas/v4", &["ZAI_API_KEY", "GLM_API_KEY", "ZHIPUAI_API_KEY"]),
];

/// An OpenAI-compatible server's models (GET /models), as chat models.
fn list_models(provider: &str, label: &str, base: &str, key: Option<&str>, cloud: bool) -> Vec<Model> {
    let mut req = agent().get(&format!("{base}/models"));
    if let Some(k) = key {
        req = req.header("Authorization", &format!("Bearer {k}"));
        if provider == "anthropic" {
            req = req.header("x-api-key", k).header("anthropic-version", "2023-06-01");
        }
    }
    let Some(v) = req.call().ok().and_then(|mut r| r.body_mut().read_json::<Value>().ok()) else { return Vec::new() };
    v["data"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| m["id"].as_str().map(|s| s.trim_start_matches("models/").to_string()))
        // (Chat models only: no embeddings, speech, pictures.)
        .filter(|id| !["embed", "tts", "whisper", "dall-e", "image", "audio", "moderation", "transcribe", "realtime", "aqa"].iter().any(|w| id.contains(w)))
        .map(|id| Model {
            provider: provider.into(),
            label: format!("{id} ({label})"),
            name: id,
            cloud,
            base: base.to_string(),
            key: key.map(str::to_string),
            loaded: !cloud,
            chat: true,
            ..Default::default()
        })
        .collect()
}

/// The request fields that set how hard a model thinks: local servers'
/// templates read `chat_template_kwargs` (llama.cpp: Inkling, Qwen,
/// gpt-oss), Ollama and the cloud APIs `reasoning_effort`.
pub fn thinking_fields(m: &Model, thinking: Option<&str>) -> Vec<(&'static str, Value)> {
    let local = !m.cloud;
    match thinking {
        None if m.provider.starts_with("ollama") && !m.via_hub => vec![("reasoning_effort", json!("none"))],
        None => Vec::new(),
        Some("off") if local => vec![("reasoning_effort", json!("none")), ("chat_template_kwargs", json!({"enable_thinking": false, "reasoning_effort": "low"}))],
        Some("off") => vec![("reasoning_effort", json!("minimal"))],
        Some(l) if local => vec![("reasoning_effort", json!(l)), ("chat_template_kwargs", json!({"enable_thinking": true, "reasoning_effort": l}))],
        Some(l) => vec![("reasoning_effort", json!(l))],
    }
}

impl Ai {
    /// Connect: the hub's app API when hearth's hub runs (and
    /// `TURBINE_AI_HUB` isn't "0"), else standalone. Models arrive as an event.
    pub fn start() -> Ai {
        let (tx, rx) = mpsc::channel();
        let mut ai = Ai { hub: None, thinking: None, tx, rx, next_id: 1, models: Vec::new(), source: String::new() };
        let use_hub = std::env::var("TURBINE_AI_HUB").map(|v| v != "0").unwrap_or(true) && std::env::var("TURBINE_AI_URL").is_err();
        ai.hub = use_hub.then(hub_api).flatten();
        ai.source = if ai.hub.is_some() { "hearth AI hub".into() } else { "standalone".into() };
        ai.refresh();
        ai
    }

    /// A client that knows only these models (tests).
    pub fn with_models(models: Vec<Model>) -> Ai {
        let (tx, rx) = mpsc::channel();
        Ai { hub: None, thinking: None, tx, rx, next_id: 1, models, source: "test".into() }
    }

    pub fn has_hub(&self) -> bool {
        self.hub.is_some()
    }

    /// Ask for the model list again.
    pub fn refresh(&mut self) {
        let tx = self.tx.clone();
        match self.hub.clone() {
            Some(h) => {
                std::thread::spawn(move || {
                    let r = agent().get(&format!("{}/models", h.base)).header("Authorization", &format!("Bearer {}", h.token)).call();
                    match r.map_err(|e| e.to_string()).and_then(|mut r| r.body_mut().read_json::<Value>().map_err(|e| e.to_string())) {
                        Ok(v) => {
                            let _ = tx.send(AiEvent::Models(models_from_hub(&v, &h.base, &h.token)));
                        }
                        // (The hub stopped: standalone, as it was never there.)
                        Err(e) => {
                            let _ = tx.send(AiEvent::HubGone(e));
                            let _ = tx.send(AiEvent::Models(standalone_models()));
                        }
                    }
                });
            }
            None => {
                std::thread::spawn(move || {
                    let _ = tx.send(AiEvent::Models(standalone_models()));
                });
            }
        }
    }

    /// Before using a model (the hub makes room itself, on each request).
    pub fn prepare(&mut self, _m: &Model) {}

    pub fn poll(&mut self) -> Vec<AiEvent> {
        let evs: Vec<AiEvent> = self.rx.try_iter().collect();
        for e in &evs {
            match e {
                AiEvent::Models(m) => self.models = m.clone(),
                AiEvent::HubGone(_) => {
                    self.hub = None;
                    self.source = "standalone".into();
                }
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
        let think = thinking_fields(model, self.thinking.as_deref());
        std::thread::spawn(move || {
            let msgs: Vec<Value> = messages.iter().map(|x| json!({"role": x.role, "content": x.content})).collect();
            let mut body = json!({"model": m.name, "messages": msgs, "stream": true});
            for (k, v) in think {
                body[k] = v;
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
                // (Its thinking, where the server keeps it apart: shown as it comes.)
                let delta = &v["choices"][0]["delta"];
                if let Some(t) = delta["reasoning_content"].as_str().or(delta["reasoning"].as_str()).filter(|t| !t.is_empty()) {
                    if tx.send(AiEvent::Thinking { id, text: t.to_string() }).is_err() {
                        return;
                    }
                }
                let raw = delta["content"].as_str().unwrap_or("");
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
        let think = thinking_fields(model, self.thinking.as_deref());
        std::thread::spawn(move || {
            let mut body = json!({"model": m.name, "messages": messages, "tools": tools, "stream": false});
            for (k, v) in think {
                body[k] = v;
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
            let result: Result<String, String> = if m.provider.starts_with("ollama") && !m.via_hub {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rambles_in_other_scripts_are_dropped() {
        assert!(fits_text("0;\n    for i in 1..100 {", "fn main() {}"));
        assert!(!fits_text("这是一个函数", "fn main() {}"));
        assert!(!fits_text("let x = 1; // 计算", "fn main() {}"));
        assert!(fits_text("// 计算总和", "// 中文注释\nfn main() {}"));
    }

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
        // A stopped service (socket-activated) still counts, and Inkling wins.
        let mut st = st;
        st["providers"].as_array_mut().unwrap().push(json!({"id": "service:inkling-2-mini.service", "kind": "local", "url": "http://127.0.0.1:11438",
            "running": false, "service": true, "installed": [{"name": "inkling-2-mini", "label": "Inkling 2 Mini"}], "loaded": []}));
        let ms = models_from_state(&st);
        let m = pick_chat(&ms).unwrap();
        assert_eq!((m.name.as_str(), m.base.as_str()), ("inkling-2-mini", "http://127.0.0.1:11438/v1"));
    }

    #[test]
    fn hub_api_models_and_thinking() {
        let v = json!({"object": "list", "data": [
            {"id": "default", "hearth": {"provider": "claude", "model": "haiku", "label": "haiku (Claude Code)", "available": true, "cloud": true}},
            {"id": "service:inkling-2-mini.service/inkling-2-mini", "hearth": {"provider": "service:inkling-2-mini.service", "model": "inkling-2-mini", "label": "Inkling 2 Mini", "available": true}},
            {"id": "ollama/qwen2.5-coder:1.5b-base", "hearth": {"provider": "ollama", "model": "qwen2.5-coder:1.5b-base", "label": "qwen2.5-coder:1.5b-base (Ollama)", "available": true}},
            {"id": "gone/x", "hearth": {"provider": "gone", "model": "x", "available": false}}
        ]});
        let ms = models_from_hub(&v, "http://127.0.0.1:18380/v1", "tok");
        assert_eq!(ms.len(), 3);
        assert_eq!((ms[0].label.as_str(), ms[0].cloud), ("hearth default: haiku (Claude Code)", true));
        assert!(ms.iter().all(|m| m.via_hub && m.key.as_deref() == Some("tok")));
        assert_eq!(pick_chat(&ms).unwrap().name, "service:inkling-2-mini.service/inkling-2-mini");
        assert_eq!(pick_fim(&ms).unwrap().name, "ollama/qwen2.5-coder:1.5b-base");
        // Thinking: local templates and the cloud's field; nothing unasked.
        let (ink, claude) = (&ms[1], &ms[0]);
        assert!(thinking_fields(ink, None).is_empty());
        let f = thinking_fields(ink, Some("high"));
        assert_eq!(f[1].1["reasoning_effort"], "high");
        assert_eq!(thinking_fields(claude, Some("high")), vec![("reasoning_effort", json!("high"))]);
        assert_eq!(thinking_fields(ink, Some("off"))[1].1["enable_thinking"], false);
    }
}
