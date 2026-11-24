//! Ollama, built in: the models the user already pulled, answering on this computer (or on the
//! Ollama server they configured) with the same MCP tools the app gives the CLIs.
//!
//! - Opt-in and light: the panel only *asks* a running Ollama what it has (`/api/tags`, with
//!   `/api/show` for the capabilities when the list lacks them). It never starts Ollama and
//!   never pulls a model. Requests carry a short `keep_alive`; closing the conversation unloads the
//!   model (`keep_alive: 0`).
//! - The agent loop runs here: `/api/chat` with `tools`, the calls executed on the app's MCP
//!   servers ([`crate::mcp_client`]), at most [`MAX_STEPS`] rounds of tool calls.
//! - Writes stay proposals: in Automático the servers are still asked for
//!   [`McpWrites::Propose`](crate::McpWrites) ([`effective_mode`]), so every change waits for
//!   Aplicar.
//! - Models without tool support (no `tools` capability, or Ollama refuses `tools`) answer as
//!   plain chat.
use crate::args::Turn;
use crate::attach::Kind;
use crate::http::{self, Base};
use crate::mcp::McpServer;
use crate::mcp_client::{McpClient, McpTool};
use crate::permission::PermissionMode;
use crate::remote::Cancel;
use crate::stream::{AgentError, StreamEvent, ToolRef, tool_ref};
use serde_json::{Value, json};
use std::io::BufRead;
use std::path::Path;
use std::time::Duration;

pub const DEFAULT_URL: &str = "http://localhost:11434";
/// How long Ollama keeps the model loaded after a request.
pub const KEEP_ALIVE: &str = "5m";
/// Rounds of tool calls per question; then the model must answer with what it has.
pub const MAX_STEPS: usize = 8;
/// A tool result longer than this is cut before it goes back to the model (small context).
pub const MAX_TOOL_TEXT: usize = 16_000;
/// Text attachments inlined into the question, at most.
const MAX_INLINE_TEXT: u64 = 48 * 1024;
const PROBE: Duration = Duration::from_millis(600);
const CHAT_READ: Duration = Duration::from_secs(600);

/// A model `/api/tags` lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaModel {
    pub name: String,
    /// Bytes on disk, about what it takes in memory once loaded.
    pub size: u64,
    pub parameter_size: Option<String>,
    /// `/api/tags` (recent Ollama) or `/api/show` capabilities; `None`: unknown.
    pub capabilities: Option<Vec<String>>,
}

impl OllamaModel {
    /// Can use the app's tools. Unknown counts as yes; a refusal at run time falls back to chat.
    pub fn tools(&self) -> bool {
        self.capabilities.as_ref().is_none_or(|c| c.iter().any(|x| x == "tools"))
    }

    pub fn vision(&self) -> bool {
        self.capabilities.as_ref().is_some_and(|c| c.iter().any(|x| x == "vision"))
    }
}

/// What the panel knows about Ollama after looking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Nothing answers at the URL (`installed`: the app is on this computer).
    Down {
        installed: bool,
    },
    /// It runs but has no models yet.
    NoModels,
    Ready(Vec<OllamaModel>),
}

/// Automático is never automatic here: writes stay proposals the user applies.
pub fn effective_mode(mode: PermissionMode) -> PermissionMode {
    match mode {
        PermissionMode::Auto => PermissionMode::Ask,
        m => m,
    }
}

/// The URL to use: the configured one, else [`DEFAULT_URL`].
pub fn url_or_default(url: Option<&str>) -> String {
    url.map(str::trim).filter(|u| !u.is_empty()).unwrap_or(DEFAULT_URL).to_string()
}

/// `localhost` and friends: "everything stays on this computer".
pub fn is_local(url: &str) -> bool {
    Base::parse(url).is_ok_and(|b| b.is_local())
}

/// The host part of a URL, for the privacy note.
pub fn host_of(url: &str) -> String {
    Base::parse(url).map(|b| b.host).unwrap_or_else(|_| url.to_string())
}

/// Ollama is on this computer (its app or its command line), running or not.
pub fn installed(home: &Path) -> bool {
    if cfg!(target_os = "macos") && (Path::new("/Applications/Ollama.app").exists() || home.join("Applications/Ollama.app").exists()) {
        return true;
    }
    let exe = if cfg!(windows) { "ollama.exe" } else { "ollama" };
    let path = crate::provider::login_shell_path();
    let dirs: Vec<_> = path.as_deref().map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = dirs.into_iter().chain(crate::provider::usual_dirs(home)).collect();
    if cfg!(windows)
        && let Ok(l) = std::env::var("LOCALAPPDATA")
    {
        dirs.push(Path::new(&l).join("Programs").join("Ollama"));
    }
    dirs.iter().any(|d| d.join(exe).is_file())
}

/// Physical memory in bytes, when the OS tells.
pub fn total_memory() -> Option<u64> {
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).stderr(std::process::Stdio::null()).output().ok()?;
        return String::from_utf8_lossy(&out.stdout).trim().parse().ok();
    }
    if cfg!(target_os = "linux") {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = s.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse().ok()?;
        return Some(kb * 1024);
    }
    None
}

/// A model that takes a large share of the memory once loaded (≥ 30 %: on 16 GB, ~4.8 GB).
pub fn heavy(size: u64, memory: Option<u64>) -> bool {
    match memory {
        Some(m) if m > 0 => size.saturating_mul(10) >= m.saturating_mul(3),
        _ => size >= 8 * 1024 * 1024 * 1024,
    }
}

/// "6.6 GB"
pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Physical memory as the user knows it: "16 GB".
pub fn ram(bytes: u64) -> String {
    format!("{:.0} GB", bytes as f64 / (1u64 << 30) as f64)
}

/// `/api/tags` → models.
pub fn parse_tags(text: &str) -> Result<Vec<OllamaModel>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let list = v.get("models").and_then(Value::as_array).ok_or("no models field")?;
    let mut out: Vec<OllamaModel> = list
        .iter()
        .filter_map(|m| {
            let name = m.get("name").or_else(|| m.get("model")).and_then(Value::as_str).filter(|s| !s.is_empty())?;
            let caps = m.get("capabilities").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect());
            Some(OllamaModel {
                name: name.to_string(),
                size: m.get("size").and_then(Value::as_u64).unwrap_or(0),
                parameter_size: m["details"]["parameter_size"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
                capabilities: caps,
            })
        })
        // embedding-only models can't chat
        .filter(|m: &OllamaModel| !m.capabilities.as_ref().is_some_and(|c| !c.is_empty() && c.iter().all(|x| x == "embedding")))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// MCP tool → Ollama's `tools` entry (OpenAI-style function). `function` is the name the
/// model sees (see [`function_name`]).
pub fn tool_schema(function: &str, tool: &McpTool) -> Value {
    let mut params = match &tool.input_schema {
        Value::Object(o) => Value::Object(o.clone()),
        _ => json!({}),
    };
    if let Value::Object(o) = &mut params {
        o.remove("$schema");
        o.insert("type".into(), json!("object"));
        o.entry("properties").or_insert_with(|| json!({}));
    }
    let description = if tool.description.trim().is_empty() { format!("{} tool of {}", tool.name, tool.server) } else { tool.description.trim().to_string() };
    json!({"type": "function", "function": {"name": function, "description": description, "parameters": params}})
}

/// `<server>__<tool>`, only `[A-Za-z0-9_-]`, at most 64 characters (what models accept).
pub fn function_name(server: &str, tool: &str) -> String {
    let s: String = format!("{server}__{tool}").chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    s.chars().take(64).collect()
}

/// The tools of a session, however they are reached (the app's MCP servers; a fake in tests).
pub trait Tools {
    fn list(&mut self) -> Vec<McpTool>;
    /// Run `tool` of `server`: `(text, is_error)`; `Err` when it couldn't run at all.
    fn call(&mut self, server: &str, tool: &str, args: Value) -> Result<(String, bool), String>;
}

/// The app's MCP servers, started for one question (stopped when dropped).
pub struct McpTools {
    clients: Vec<McpClient>,
    tools: Vec<McpTool>,
    /// Servers that didn't start (named in the answer's errors).
    pub failed: Vec<String>,
}

impl McpTools {
    pub fn start(servers: &[McpServer], cwd: &Path) -> McpTools {
        let mut t = McpTools { clients: Vec::new(), tools: Vec::new(), failed: Vec::new() };
        for s in servers {
            match McpClient::start(s, cwd).and_then(|mut c| c.list_tools().map(|l| (c, l))) {
                Ok((c, list)) => {
                    t.tools.extend(list);
                    t.clients.push(c);
                }
                Err(e) => t.failed.push(e),
            }
        }
        t
    }
}

impl Tools for McpTools {
    fn list(&mut self) -> Vec<McpTool> {
        self.tools.clone()
    }
    fn call(&mut self, server: &str, tool: &str, args: Value) -> Result<(String, bool), String> {
        let c = self.clients.iter_mut().find(|c| c.name == server).ok_or_else(|| format!("no server {server}"))?;
        c.call(tool, args)
    }
}

/// An Ollama server.
#[derive(Debug, Clone)]
pub struct Client {
    pub url: String,
    base: Base,
}

#[derive(Debug, Default)]
struct Reply {
    content: String,
    tool_calls: Vec<Value>,
}

enum ChatError {
    NoTools,
    Cancelled,
    Failed(String),
}

impl Client {
    pub fn new(url: &str) -> Result<Client, String> {
        Ok(Client { url: url.to_string(), base: Base::parse(url)? })
    }

    fn get(&self, path: &str) -> Result<(u16, String), String> {
        let r = http::request(&self.base, "GET", path, None, PROBE, Duration::from_secs(5)).map_err(|e| e.to_string())?;
        let status = r.status;
        Ok((status, r.text().map_err(|e| e.to_string())?))
    }

    fn post(&self, path: &str, body: &Value, read: Duration) -> Result<(u16, String), String> {
        let r = http::request(&self.base, "POST", path, Some(body.to_string().as_bytes()), PROBE, read).map_err(|e| e.to_string())?;
        let status = r.status;
        Ok((status, r.text().map_err(|e| e.to_string())?))
    }

    pub fn tags(&self) -> Result<Vec<OllamaModel>, String> {
        match self.get("/api/tags")? {
            (200, t) => parse_tags(&t),
            (s, t) => Err(format!("HTTP {s}: {}", t.trim())),
        }
    }

    /// `/api/show` capabilities (doesn't load the model).
    pub fn capabilities(&self, model: &str) -> Option<Vec<String>> {
        let (s, t) = self.post("/api/show", &json!({"model": model}), Duration::from_secs(5)).ok()?;
        if s != 200 {
            return None;
        }
        let v: Value = serde_json::from_str(&t).ok()?;
        v.get("capabilities").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
    }

    /// Ask what's there, quickly: never starts Ollama, never pulls.
    pub fn status(&self, home: &Path) -> Status {
        match self.tags() {
            Err(_) => Status::Down { installed: installed(home) },
            Ok(l) if l.is_empty() => Status::NoModels,
            Ok(mut l) => {
                for m in l.iter_mut().filter(|m| m.capabilities.is_none()).take(24) {
                    m.capabilities = self.capabilities(&m.name);
                }
                Status::Ready(l)
            }
        }
    }

    /// Free the model's memory now (`keep_alive: 0`).
    pub fn unload(&self, model: &str) {
        let _ = self.post("/api/generate", &json!({"model": model, "keep_alive": 0}), Duration::from_secs(3));
    }

    /// One `/api/chat` round, streamed: text deltas go to `emit` as they arrive.
    fn chat(&self, body: &Value, cancel: &Cancel, emit: &mut dyn FnMut(StreamEvent)) -> Result<Reply, ChatError> {
        let resp = http::request(&self.base, "POST", "/api/chat", Some(body.to_string().as_bytes()), PROBE, CHAT_READ).map_err(|e| ChatError::Failed(e.to_string()))?;
        if resp.status != 200 {
            let status = resp.status;
            let text = resp.text().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or(text);
            return Err(if msg.contains("does not support tools") { ChatError::NoTools } else { ChatError::Failed(format!("HTTP {status}: {}", msg.trim())) });
        }
        // closing the socket unblocks the read when the user stops
        let done = Cancel::new();
        let watchdog = {
            let (stream, cancel, done) = (resp.stream.try_clone().ok(), cancel.clone(), done.clone());
            std::thread::spawn(move || {
                while !done.is_cancelled() {
                    if cancel.is_cancelled() {
                        if let Some(s) = &stream {
                            let _ = s.shutdown(std::net::Shutdown::Both);
                        }
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(60));
                }
            })
        };
        let mut reply = Reply::default();
        let mut err = None;
        for line in resp.body.lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
            if let Some(e) = v.get("error").and_then(Value::as_str) {
                err = Some(if e.contains("does not support tools") { ChatError::NoTools } else { ChatError::Failed(e.to_string()) });
                break;
            }
            let m = &v["message"];
            if let Some(t) = m["content"].as_str().filter(|t| !t.is_empty()) {
                reply.content.push_str(t);
                emit(StreamEvent::Delta(t.to_string()));
            }
            reply.tool_calls.extend(m["tool_calls"].as_array().into_iter().flatten().cloned());
            if v["done"].as_bool() == Some(true) {
                break;
            }
        }
        done.cancel();
        let _ = watchdog.join();
        if cancel.is_cancelled() {
            return Err(ChatError::Cancelled);
        }
        match err {
            Some(e) => Err(e),
            None => Ok(reply),
        }
    }
}

fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (cut, {} more bytes)", &s[..end], s.len() - end)
}

/// The user's message: the question, small text attachments inline, images for vision models.
fn user_message(turn: &Turn, vision: bool) -> Value {
    let mut text = turn.text.trim().to_string();
    let mut images = Vec::new();
    for a in turn.attachments {
        match a.kind {
            Kind::Image if vision && a.bytes <= crate::attach::MAX_INLINE => {
                if let Ok(b) = std::fs::read(&a.path) {
                    images.push(Value::String(crate::attach::base64(&b)));
                }
            }
            Kind::Other if a.bytes <= MAX_INLINE_TEXT && (a.mime.starts_with("text/") || a.mime == "application/json") => match std::fs::read_to_string(&a.path) {
                Ok(t) => text.push_str(&format!("\n\nAttached file {}:\n```\n{}\n```", a.name, t.trim_end())),
                Err(_) => text.push_str(&format!("\n\n(Attached file {}, unreadable.)", a.name)),
            },
            _ => text.push_str(&format!("\n\n(The user attached {}, which you can't open.)", a.name)),
        }
    }
    let mut m = json!({"role": "user", "content": text});
    if !images.is_empty() {
        m["images"] = Value::Array(images);
    }
    m
}

/// Everything one question needs besides the tools.
pub struct Question<'a> {
    pub client: &'a Client,
    pub model: &'a OllamaModel,
    pub turn: &'a Turn<'a>,
    /// The session's MCP server names (to label tool chips like the CLIs do).
    pub servers: &'a [String],
    pub max_steps: usize,
}

/// Run one question to the end: the tool-calling loop, streamed. Returns the answer.
pub fn run_turn(q: &Question, tools: &mut dyn Tools, cancel: &Cancel, emit: &mut dyn FnMut(StreamEvent)) -> Result<String, AgentError> {
    let listed = if q.model.tools() { tools.list() } else { Vec::new() };
    let mut by_function = std::collections::HashMap::new();
    let mut schemas = Vec::new();
    for t in &listed {
        let mut f = function_name(&t.server, &t.name);
        while by_function.contains_key(&f) {
            f.push('_');
        }
        schemas.push(tool_schema(&f, t));
        by_function.insert(f, t.clone());
    }
    let mut system = q.turn.system.trim().to_string();
    system.push_str(if schemas.is_empty() {
        "\n\nYou have no tools in this session: answer from the conversation and the context below; say so when you would need the app's data."
    } else {
        "\n\nCall the app's tools (functions) to read the user's data; never invent data. Call a tool only with the arguments its schema describes."
    });
    if let Some(c) = q.turn.context.map(str::trim).filter(|c| !c.is_empty()) {
        system.push_str("\n\nWhat the user is looking at in the app right now:\n");
        system.push_str(c);
    }
    let mut messages = vec![json!({"role": "system", "content": system})];
    for (user, text) in q.turn.history.iter().rev().take(12).rev() {
        messages.push(json!({"role": if *user { "user" } else { "assistant" }, "content": text}));
    }
    messages.push(user_message(q.turn, q.model.vision()));

    let mut answer = String::new();
    let mut use_tools = !schemas.is_empty();
    let mut steps = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        // the last round has no tools: the model has to answer
        let offer = use_tools && steps < q.max_steps;
        let mut body = json!({"model": q.model.name, "messages": messages, "stream": true, "keep_alive": KEEP_ALIVE});
        if offer {
            body["tools"] = Value::Array(schemas.clone());
        } else if use_tools {
            messages.push(json!({"role": "system", "content": "The tool-call limit for this question is reached. Answer now with what you found."}));
            body["messages"] = Value::Array(messages.clone());
        }
        let sep = !answer.is_empty() && !answer.ends_with("\n\n");
        let mut first = true;
        let mut sink = |e: StreamEvent| {
            if matches!(e, StreamEvent::Delta(_)) && std::mem::take(&mut first) && sep {
                emit(StreamEvent::Delta("\n\n".into()));
            }
            emit(e)
        };
        let reply = match q.client.chat(&body, cancel, &mut sink) {
            Ok(r) => r,
            Err(ChatError::Cancelled) => return Err(AgentError::Cancelled),
            Err(ChatError::NoTools) if offer => {
                // the model can't take tools after all: answer as plain chat
                use_tools = false;
                schemas.clear();
                continue;
            }
            Err(ChatError::NoTools) => return Err(AgentError::Failed("this model does not support tools".into())),
            Err(ChatError::Failed(e)) => return Err(AgentError::Failed(format!("Ollama: {e}"))),
        };
        if !reply.content.is_empty() {
            if sep {
                answer.push_str("\n\n");
            }
            answer.push_str(&reply.content);
        }
        if reply.tool_calls.is_empty() || !offer {
            return Ok(answer);
        }
        steps += 1;
        messages.push(json!({"role": "assistant", "content": reply.content, "tool_calls": reply.tool_calls}));
        for call in &reply.tool_calls {
            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }
            let f = call["function"]["name"].as_str().unwrap_or("");
            let mut args = call["function"]["arguments"].clone();
            if let Value::String(s) = &args {
                args = serde_json::from_str(s).unwrap_or(json!({}));
            }
            if !args.is_object() {
                args = json!({});
            }
            let (chip, out, failed) = match by_function.get(f) {
                Some(t) => {
                    let chip = tool_ref(q.servers, &t.name, Some(&t.server));
                    emit(StreamEvent::ToolCall(chip.clone()));
                    match tools.call(&t.server, &t.name, args) {
                        Ok((text, is_error)) => (chip, text, is_error),
                        Err(e) => (chip, format!("error: {e}"), true),
                    }
                }
                None => {
                    let chip = ToolRef::new(None, f);
                    emit(StreamEvent::ToolCall(chip.clone()));
                    (chip, format!("error: there is no tool named {f}"), true)
                }
            };
            emit(StreamEvent::ToolResult { tool: chip, is_error: failed });
            messages.push(json!({"role": "tool", "tool_name": f, "content": cut(&out, MAX_TOOL_TEXT)}));
        }
    }
}
