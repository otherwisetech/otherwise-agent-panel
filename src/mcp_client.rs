//! The panel's own MCP client, for providers that don't speak MCP themselves (Ollama): starts a
//! host's stdio server for the session, lists its tools and calls them. Newline-delimited
//! JSON-RPC; the server runs in its own process group and is stopped when the client drops.
use crate::mcp::McpServer;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

pub const PROTOCOL: &str = "2025-06-18";
const START_TIMEOUT: Duration = Duration::from_secs(20);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// One tool as the server lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    /// The server's name in this session (`expense`).
    pub server: String,
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments (`inputSchema`).
    pub input_schema: Value,
}

pub struct McpClient {
    pub name: String,
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    next_id: u64,
}

impl McpClient {
    /// Start `server` in `cwd` and run the MCP handshake.
    pub fn start(server: &McpServer, cwd: &Path) -> Result<McpClient, String> {
        let mut cmd = Command::new(&server.program);
        cmd.args(&server.args).current_dir(cwd).env("PATH", crate::provider::child_path()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        for (k, v) in &server.env {
            cmd.env(k, v);
        }
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", server.program.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                    && tx.send(v).is_err()
                {
                    break;
                }
            }
        });
        let mut c = McpClient { name: server.name.clone(), child, stdin, rx, next_id: 1 };
        c.request("initialize", json!({"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "otherwise-agent-panel", "version": crate::VERSION}}), START_TIMEOUT)?;
        c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(c)
    }

    fn send(&mut self, v: &Value) -> Result<(), String> {
        let mut line = v.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).and_then(|_| self.stdin.flush()).map_err(|e| format!("{}: {e}", self.name))
    }

    pub fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let start = Instant::now();
        loop {
            let left = timeout.checked_sub(start.elapsed()).ok_or_else(|| format!("{}: {method} timed out", self.name))?;
            let msg = match self.rx.recv_timeout(left) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => return Err(format!("{}: {method} timed out", self.name)),
                Err(RecvTimeoutError::Disconnected) => return Err(format!("{}: the server stopped", self.name)),
            };
            // a request from the server (ping, roots/list…): answer so it doesn't wait
            if let (Some(mid), Some(m)) = (msg.get("id"), msg.get("method").and_then(Value::as_str)) {
                let reply = if m == "ping" { json!({"jsonrpc": "2.0", "id": mid, "result": {}}) } else { json!({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "not supported"}}) };
                self.send(&reply)?;
                continue;
            }
            if msg.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(e) = msg.get("error") {
                return Err(format!("{}: {}", self.name, e.get("message").and_then(Value::as_str).unwrap_or("error")));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    pub fn list_tools(&mut self) -> Result<Vec<McpTool>, String> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let r = self.request("tools/list", params, START_TIMEOUT)?;
            for t in r.get("tools").and_then(Value::as_array).into_iter().flatten() {
                let Some(name) = t.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) else { continue };
                out.push(McpTool {
                    server: self.name.clone(),
                    name: name.to_string(),
                    description: t.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
                    input_schema: t.get("inputSchema").cloned().unwrap_or(Value::Null),
                });
            }
            cursor = r.get("nextCursor").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    /// `tools/call`: the text of the result and whether the tool reported an error.
    pub fn call(&mut self, tool: &str, args: Value) -> Result<(String, bool), String> {
        let r = self.request("tools/call", json!({"name": tool, "arguments": args}), CALL_TIMEOUT)?;
        Ok((result_text(&r), r.get("isError").and_then(Value::as_bool).unwrap_or(false)))
    }
}

/// The text parts of a `tools/call` result (structured content when there is no text).
pub fn result_text(r: &Value) -> String {
    let parts: Vec<String> = r
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| match c.get("type").and_then(Value::as_str) {
            Some("text") => c.get("text").and_then(Value::as_str).map(str::to_string),
            Some("resource") => c.get("resource").and_then(|r| r.get("text")).and_then(Value::as_str).map(str::to_string),
            Some(other) => Some(format!("[{other} content]")),
            None => None,
        })
        .collect();
    if parts.is_empty() { r.get("structuredContent").map(Value::to_string).unwrap_or_default() } else { parts.join("\n") }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: signalling the server's own process group (started as its leader); a gone
        // group yields ESRCH, ignored
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGTERM);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
