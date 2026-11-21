//! Reading each CLI's headless output line by line — Claude Code `stream-json`, Codex `exec
//! --json`, opencode `run --format json` — into one event type the chat renders. Remote
//! providers emit the same [`StreamEvent`]s directly.
use crate::provider::Cli;
use serde_json::Value;

/// Which tool ran: the app it belongs to (`expense`, or for the hub's proxied tools the app
/// behind it) and its name there. Shown as a chip: "expense · receipts_list".
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    pub tool: String,
}

impl ToolRef {
    pub fn new(app: Option<&str>, tool: &str) -> ToolRef {
        ToolRef { app: app.map(str::to_string), tool: tool.to_string() }
    }

    /// "expense · receipts_list" (the app's display name when Otherwise knows it).
    pub fn label(&self) -> String {
        match &self.app {
            Some(a) => format!("{} · {}", otherwise_link::apps::display_name(a), self.tool),
            None => self.tool.clone(),
        }
    }
}

/// A tool name as the agent reports it → which app and tool. `servers` are the MCP server
/// names of the session. Claude Code says `mcp__expense__receipts_list`, opencode
/// `expense_receipts_list`, Codex `receipts_list` with the server apart (`server` field). The
/// hub's tools are `<app>.<tool>` (`mcp__otherwise__vigia_due` once Claude replaced the dot).
pub fn tool_ref(servers: &[String], raw: &str, server: Option<&str>) -> ToolRef {
    let mut srv = server.map(str::to_string);
    let mut rest = raw.to_string();
    if let Some(r) = raw.strip_prefix("mcp__")
        && let Some((s, t)) = r.split_once("__")
    {
        srv = Some(s.to_string());
        rest = t.to_string();
    } else if srv.is_none() {
        for s in servers {
            if let Some(t) = raw.strip_prefix(&format!("{s}_")).or_else(|| raw.strip_prefix(&format!("{s}."))).filter(|t| !t.is_empty()) {
                srv = Some(s.clone());
                rest = t.to_string();
                break;
            }
        }
    }
    match srv.as_deref() {
        Some(crate::mcp::HUB) => hub_tool(&rest),
        Some(s) => ToolRef::new(Some(s), &rest),
        None if servers.iter().any(|s| s == crate::mcp::HUB) => hub_tool(&rest),
        None => ToolRef::new(None, &rest),
    }
}

/// The hub's own tools (`today`, `entities_search`) have no app; proxied ones are `<app>.<tool>`
/// or, with the dot replaced, `<app>_<tool>` for a known app id.
fn hub_tool(s: &str) -> ToolRef {
    if let Some((a, t)) = s.split_once('.').filter(|(a, t)| !a.is_empty() && !t.is_empty()) {
        return ToolRef::new(Some(a), t);
    }
    for app in otherwise_link::apps::KNOWN.iter().map(|k| k.id).filter(|id| *id != otherwise_link::apps::HUB_ID) {
        if let Some(t) = s.strip_prefix(app).and_then(|r| r.strip_prefix(['_', '-'])).filter(|t| !t.is_empty()) {
            return ToolRef::new(Some(app), t);
        }
    }
    ToolRef::new(None, s)
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// More text of the answer.
    Delta(String),
    ToolCall(ToolRef),
    ToolResult {
        tool: ToolRef,
        is_error: bool,
    },
    /// Something went wrong (the run still ends with its result).
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentError {
    /// The program wasn't found.
    NotInstalled(Cli),
    /// The CLI needs `login_command()`.
    NotLoggedIn(Cli),
    /// The user stopped it.
    Cancelled,
    Failed(String),
}

/// An error message (from the stream or stderr) that means "log in first".
pub fn is_login_error(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "not logged in",
        "please run /login",
        "run /login",
        "invalid api key",
        "login required",
        "authentication_error",
        "unauthorized",
        "codex login",
        "auth login",
        "no credentials",
        "not authenticated",
        "oauth token has expired",
        "please log in",
    ]
    .iter()
    .any(|w| t.contains(w))
}

pub fn classify(cli: Cli, text: &str) -> AgentError {
    if is_login_error(text) { AgentError::NotLoggedIn(cli) } else { AgentError::Failed(text.trim().chars().take(600).collect()) }
}

fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(|x| x.get("text").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| content_text(x))).collect::<Vec<_>>().join(""),
        Value::Object(o) => o
            .get("content")
            .map(content_text)
            .or_else(|| o.get("text").and_then(Value::as_str).map(str::to_string))
            .or_else(|| o.get("message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| v.to_string()),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Turns one CLI's output lines into events.
#[derive(Debug)]
pub struct Parser {
    cli: Cli,
    servers: Vec<String>,
    /// Claude Code: text already streamed as partial messages (the whole message repeats it).
    streamed: bool,
    /// tool_use id → tool, for results that only carry the id.
    tools: std::collections::HashMap<String, ToolRef>,
    pub final_text: String,
}

impl Parser {
    pub fn new(cli: Cli, servers: Vec<String>) -> Parser {
        Parser { cli, servers, streamed: false, tools: Default::default(), final_text: String::new() }
    }

    pub fn line(&mut self, line: &str) -> Vec<StreamEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return Vec::new() };
        let mut out = Vec::new();
        match self.cli {
            Cli::ClaudeCode => self.claude(&v, &mut out),
            Cli::Codex => self.codex(&v, &mut out),
            Cli::Opencode => self.opencode(&v, &mut out),
        }
        for e in &out {
            if let StreamEvent::Delta(t) = e {
                self.final_text.push_str(t);
            }
        }
        out
    }

    fn sep(&self) -> &'static str {
        if self.final_text.is_empty() || self.final_text.ends_with("\n\n") { "" } else { "\n\n" }
    }

    fn tool(&self, raw: &str, server: Option<&str>) -> ToolRef {
        tool_ref(&self.servers, raw, server)
    }

    fn claude(&mut self, v: &Value, out: &mut Vec<StreamEvent>) {
        match v["type"].as_str().unwrap_or("") {
            "stream_event" => {
                let ev = &v["event"];
                if ev["type"] == "content_block_delta"
                    && ev["delta"]["type"] == "text_delta"
                    && let Some(t) = ev["delta"]["text"].as_str()
                {
                    if !self.streamed && !self.final_text.is_empty() && !self.final_text.ends_with(char::is_whitespace) {
                        out.push(StreamEvent::Delta("\n\n".into()));
                    }
                    self.streamed = true;
                    out.push(StreamEvent::Delta(t.to_string()));
                }
            }
            "assistant" => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str().unwrap_or("") {
                        "text" if !self.streamed => {
                            if let Some(t) = b["text"].as_str() {
                                out.push(StreamEvent::Delta(format!("{}{t}", self.sep())));
                            }
                        }
                        "tool_use" => {
                            let tool = self.tool(b["name"].as_str().unwrap_or(""), None);
                            if let Some(id) = b["id"].as_str() {
                                self.tools.insert(id.to_string(), tool.clone());
                            }
                            out.push(StreamEvent::ToolCall(tool));
                        }
                        _ => {}
                    }
                }
                self.streamed = false;
            }
            "user" => {
                for b in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let tool = b["tool_use_id"].as_str().and_then(|id| self.tools.get(id)).cloned().unwrap_or(ToolRef { app: None, tool: String::new() });
                    out.push(StreamEvent::ToolResult { tool, is_error: b["is_error"].as_bool().unwrap_or(false) });
                }
            }
            "result" => {
                if v["is_error"].as_bool() == Some(true) || v["subtype"].as_str().is_some_and(|s| s.starts_with("error")) {
                    let msg = v["result"]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| v["errors"].as_array().map(|e| e.iter().map(content_text).collect::<Vec<_>>().join("; ")))
                        .unwrap_or_else(|| v["subtype"].as_str().unwrap_or("error").to_string());
                    out.push(StreamEvent::Error(msg));
                } else if self.final_text.is_empty()
                    && let Some(t) = v["result"].as_str()
                {
                    out.push(StreamEvent::Delta(t.to_string()));
                }
            }
            _ => {}
        }
    }

    fn codex(&mut self, v: &Value, out: &mut Vec<StreamEvent>) {
        let item = &v["item"];
        match v["type"].as_str().unwrap_or("") {
            "item.started" if item["type"] == "mcp_tool_call" => out.push(StreamEvent::ToolCall(self.tool(item["tool"].as_str().unwrap_or(""), item["server"].as_str()))),
            "item.completed" => match item["type"].as_str().unwrap_or("") {
                "agent_message" => {
                    if let Some(t) = item["text"].as_str() {
                        out.push(StreamEvent::Delta(format!("{}{t}", self.sep())));
                    }
                }
                "mcp_tool_call" => {
                    let failed = item["status"] == "failed" || !item["error"].is_null();
                    out.push(StreamEvent::ToolResult { tool: self.tool(item["tool"].as_str().unwrap_or(""), item["server"].as_str()), is_error: failed });
                }
                "error" => out.push(StreamEvent::Error(item["message"].as_str().unwrap_or("").to_string())),
                _ => {}
            },
            "turn.failed" => out.push(StreamEvent::Error(v["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| v["error"].to_string()))),
            "error" => out.push(StreamEvent::Error(v["message"].as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))),
            _ => {}
        }
    }

    fn opencode(&mut self, v: &Value, out: &mut Vec<StreamEvent>) {
        let part = &v["part"];
        match v["type"].as_str().unwrap_or("") {
            "text" => {
                if let Some(t) = part["text"].as_str() {
                    out.push(StreamEvent::Delta(format!("{}{t}", self.sep())));
                }
            }
            "tool_use" => {
                let tool = self.tool(part["tool"].as_str().unwrap_or(""), None);
                let failed = part["state"]["status"] == "error";
                out.push(StreamEvent::ToolCall(tool.clone()));
                out.push(StreamEvent::ToolResult { tool, is_error: failed });
            }
            "error" => {
                let e = &v["error"];
                out.push(StreamEvent::Error(e["data"]["message"].as_str().or(e["message"].as_str()).or(e["name"].as_str()).map(str::to_string).unwrap_or_else(|| e.to_string())));
            }
            _ => {}
        }
    }
}
