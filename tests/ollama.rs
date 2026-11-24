//! The built-in Ollama provider against a mock Ollama HTTP server (the real one is never
//! called): model list, the tool loop, models without tools, Ollama down, the step cap,
//! proposals, the MCP → Ollama schema mapping and the panel's own MCP client.
use otherwise_agent_panel::args::Turn;
use otherwise_agent_panel::mcp_client::McpTool;
use otherwise_agent_panel::ollama::{self, Client, OllamaModel, Question, Status, Tools};
use otherwise_agent_panel::remote::Cancel;
use otherwise_agent_panel::stream::{AgentError, StreamEvent, ToolRef};
use otherwise_agent_panel::{McpWrites, PermissionMode, ProviderId};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

type Handler = dyn Fn(&str, &Value) -> (u16, Vec<Value>) + Send + Sync;

/// A mock Ollama: every request is recorded; `handler` answers with a status and NDJSON lines
/// (sent chunked, like Ollama streams).
struct Mock {
    url: String,
    seen: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Mock {
    fn start(handler: impl Fn(&str, &Value) -> (u16, Vec<Value>) + Send + Sync + 'static) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (seen2, handler): (_, Arc<Handler>) = (seen.clone(), Arc::new(handler));
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { break };
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut len = 0;
                loop {
                    line.clear();
                    r.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                seen2.lock().unwrap().push((path.clone(), body.clone()));
                let (status, lines) = handler(&path, &body);
                let mut out = format!("HTTP/1.1 {status} X\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\n\r\n");
                for l in lines {
                    let chunk = format!("{l}\n");
                    out.push_str(&format!("{:x}\r\n{chunk}\r\n", chunk.len()));
                }
                out.push_str("0\r\n\r\n");
                let _ = conn.write_all(out.as_bytes());
            }
        });
        Mock { url, seen }
    }

    fn chats(&self) -> Vec<Value> {
        self.seen.lock().unwrap().iter().filter(|(p, _)| p == "/api/chat").map(|(_, b)| b.clone()).collect()
    }
}

fn text(t: &str) -> Value {
    json!({"message": {"role": "assistant", "content": t}, "done": false})
}

fn done() -> Value {
    json!({"message": {"role": "assistant", "content": ""}, "done": true})
}

fn call(name: &str, args: Value) -> Value {
    json!({"message": {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": name, "arguments": args}}]}, "done": false})
}

#[derive(Default)]
struct FakeTools {
    listed: bool,
    calls: Vec<(String, String, Value)>,
}

impl Tools for FakeTools {
    fn list(&mut self) -> Vec<McpTool> {
        self.listed = true;
        vec![
            McpTool { server: "demo".into(), name: "items_list".into(), description: "List items".into(), input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}}) },
            McpTool { server: "demo".into(), name: "item_update".into(), description: "Change an item".into(), input_schema: json!({"type": "object", "properties": {"id": {"type": "string"}}}) },
        ]
    }
    fn call(&mut self, server: &str, tool: &str, args: Value) -> Result<(String, bool), String> {
        self.calls.push((server.into(), tool.into(), args));
        if tool == "item_update" { Ok(("Proposed change p1; pending approval in the panel.".into(), false)) } else { Ok(("[{\"id\":\"a\",\"name\":\"Apple\"}]".into(), false)) }
    }
}

fn model(name: &str, caps: Option<&[&str]>) -> OllamaModel {
    OllamaModel { name: name.into(), size: 5_000_000_000, parameter_size: None, capabilities: caps.map(|c| c.iter().map(|s| s.to_string()).collect()) }
}

fn ask(url: &str, m: &OllamaModel, tools: &mut FakeTools, max_steps: usize) -> (Result<String, AgentError>, Vec<StreamEvent>) {
    let client = Client::new(url).unwrap();
    let turn = Turn { system: "S", context: Some("ctx"), history: &[(true, "earlier".into()), (false, "ok".into())], text: "how many items?", attachments: &[] };
    let servers = vec!["demo".to_string()];
    let q = Question { client: &client, model: m, turn: &turn, servers: &servers, max_steps };
    let mut events = Vec::new();
    let res = ollama::run_turn(&q, tools, &Cancel::new(), &mut |e| events.push(e));
    (res, events)
}

#[test]
fn lists_models_with_sizes_and_capabilities() {
    let mock = Mock::start(|path, body| match path {
        "/api/tags" => (
            200,
            vec![json!({"models": [
                {"name": "qwen3:8b", "size": 5_200_000_000u64, "details": {"parameter_size": "8.2B"}, "capabilities": ["completion", "tools"]},
                {"name": "gemma:2b", "size": 1_700_000_000u64, "details": {"parameter_size": "2B"}},
                {"name": "nomic-embed-text", "size": 270_000_000u64, "capabilities": ["embedding"]}
            ]})],
        ),
        "/api/show" if body["model"] == "gemma:2b" => (200, vec![json!({"capabilities": ["completion"]})]),
        _ => (404, vec![json!({"error": "not found"})]),
    });
    let c = Client::new(&mock.url).unwrap();
    let Status::Ready(list) = c.status(std::path::Path::new("/nonexistent")) else { panic!("ready") };
    assert_eq!(list.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["gemma:2b", "qwen3:8b"], "sorted, embedding-only left out");
    assert!(!list[0].tools(), "no tools capability: chat only");
    assert!(list[1].tools());
    assert_eq!(list[1].size, 5_200_000_000);
    assert_eq!(list[1].parameter_size.as_deref(), Some("8.2B"));
    let seen = mock.seen.lock().unwrap().clone();
    assert!(seen.iter().all(|(p, _)| p != "/api/pull" && p != "/api/chat" && p != "/api/generate"), "looking never pulls or loads: {seen:?}");
    assert_eq!(seen.iter().filter(|(p, _)| p == "/api/show").count(), 1, "show only when tags lacks capabilities");

    let empty = Mock::start(|_, _| (200, vec![json!({"models": []})]));
    assert_eq!(Client::new(&empty.url).unwrap().status(std::path::Path::new("/nonexistent")), Status::NoModels);
}

#[test]
fn chat_with_tool_calls() {
    let mock = Mock::start(|path, body| {
        assert_eq!(path, "/api/chat");
        let msgs = body["messages"].as_array().unwrap();
        if msgs.last().unwrap()["role"] == "tool" {
            (200, vec![text("You have "), text("**1** item."), done()])
        } else {
            (200, vec![text("Let me look."), call("demo__items_list", json!({"q": "a"})), done()])
        }
    });
    let mut tools = FakeTools::default();
    let (res, events) = ask(&mock.url, &model("qwen3:8b", Some(&["completion", "tools"])), &mut tools, ollama::MAX_STEPS);
    assert_eq!(res.unwrap(), "Let me look.\n\nYou have **1** item.");
    assert_eq!(tools.calls, [("demo".to_string(), "items_list".to_string(), json!({"q": "a"}))]);
    let chip = ToolRef::new(Some("demo"), "items_list");
    assert!(events.contains(&StreamEvent::ToolCall(chip.clone())));
    assert!(events.contains(&StreamEvent::ToolResult { tool: chip, is_error: false }));
    let streamed: String = events.iter().filter_map(|e| if let StreamEvent::Delta(d) = e { Some(d.as_str()) } else { None }).collect();
    assert_eq!(streamed, "Let me look.\n\nYou have **1** item.", "deltas match the answer");

    let chats = mock.chats();
    assert_eq!(chats.len(), 2);
    for c in &chats {
        assert_eq!(c["keep_alive"], ollama::KEEP_ALIVE);
        assert_eq!(c["stream"], true);
        assert_eq!(c["model"], "qwen3:8b");
    }
    let names: Vec<&str> = chats[0]["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["demo__items_list", "demo__item_update"]);
    let m = chats[0]["messages"].as_array().unwrap();
    assert_eq!(m[0]["role"], "system");
    assert!(m[0]["content"].as_str().unwrap().contains("ctx"), "the host's context goes in the system message");
    assert_eq!((m[1]["role"].as_str(), m[2]["role"].as_str(), m[3]["content"].as_str()), (Some("user"), Some("assistant"), Some("how many items?")));
    let last = chats[1]["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["role"], "tool");
    assert_eq!(last["tool_name"], "demo__items_list");
    assert!(last["content"].as_str().unwrap().contains("Apple"));
}

#[test]
fn model_without_tool_support() {
    // known from its capabilities: no tools offered, the app's tools aren't even listed
    let mock = Mock::start(|_, body| {
        assert!(body.get("tools").is_none());
        (200, vec![text("Hi."), done()])
    });
    let mut tools = FakeTools::default();
    let (res, _) = ask(&mock.url, &model("gemma:2b", Some(&["completion"])), &mut tools, ollama::MAX_STEPS);
    assert_eq!(res.unwrap(), "Hi.");
    assert!(!tools.listed && tools.calls.is_empty());

    // unknown capabilities and Ollama refuses `tools`: the same question again as plain chat
    let mock =
        Mock::start(
            |_, body| if body.get("tools").is_some() { (400, vec![json!({"error": "registry.ollama.ai/library/old:1b does not support tools"})]) } else { (200, vec![text("Plain answer."), done()]) },
        );
    let mut tools = FakeTools::default();
    let (res, _) = ask(&mock.url, &model("old:1b", None), &mut tools, ollama::MAX_STEPS);
    assert_eq!(res.unwrap(), "Plain answer.");
    let chats = mock.chats();
    assert_eq!(chats.len(), 2);
    assert!(chats[0].get("tools").is_some() && chats[1].get("tools").is_none());
}

#[test]
fn ollama_down() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}");
    let c = Client::new(&url).unwrap();
    assert!(matches!(c.status(std::path::Path::new("/nonexistent")), Status::Down { .. }), "nothing answers: down (installed or not)");
    let mut tools = FakeTools::default();
    let (res, _) = ask(&url, &model("qwen3:8b", None), &mut tools, ollama::MAX_STEPS);
    assert!(matches!(res, Err(AgentError::Failed(m)) if m.starts_with("Ollama")));
    assert!(Client::new("https://example.com").is_err(), "plain http only");
}

#[test]
fn step_cap() {
    // a model that never stops calling tools
    let mock = Mock::start(|_, body| if body.get("tools").is_some() { (200, vec![call("demo__items_list", json!({})), done()]) } else { (200, vec![text("Here is what I found."), done()]) });
    let mut tools = FakeTools::default();
    let (res, events) = ask(&mock.url, &model("qwen3:8b", Some(&["tools"])), &mut tools, 3);
    assert_eq!(res.unwrap(), "Here is what I found.");
    assert_eq!(tools.calls.len(), 3, "at most max_steps rounds of tool calls");
    assert_eq!(events.iter().filter(|e| matches!(e, StreamEvent::ToolCall(_))).count(), 3);
    let chats = mock.chats();
    assert_eq!(chats.len(), 4);
    assert!(chats[3].get("tools").is_none(), "the last round can only answer");
    assert_eq!(ollama::MAX_STEPS, 8);
}

#[test]
fn proposals_are_not_auto_applied() {
    // Automático still asks the host for proposal tools
    assert_eq!(ollama::effective_mode(PermissionMode::Auto), PermissionMode::Ask);
    assert_eq!(ollama::effective_mode(PermissionMode::Auto).writes(), McpWrites::Propose);
    assert_eq!(ollama::effective_mode(PermissionMode::ReadOnly), PermissionMode::ReadOnly);
    // a write tool call is just a call: its "pending approval" goes back to the model, nothing applies it
    let mock = Mock::start(|_, body| {
        let last = body["messages"].as_array().unwrap().last().unwrap().clone();
        if last["role"] == "tool" { (200, vec![text("The change is waiting for your approval."), done()]) } else { (200, vec![call("demo__item_update", json!({"id": "a"})), done()]) }
    });
    let mut tools = FakeTools::default();
    let (res, _) = ask(&mock.url, &model("qwen3:8b", Some(&["tools"])), &mut tools, ollama::MAX_STEPS);
    assert!(res.unwrap().contains("waiting for your approval"));
    assert_eq!(tools.calls.len(), 1, "one proposal, nothing else called");
    assert_eq!(tools.calls[0].1, "item_update");
    assert!(mock.chats()[1]["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().contains("pending approval"));
}

#[test]
fn unknown_tool_and_string_arguments() {
    let mock = Mock::start(|_, body| {
        let n = body["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "tool").count();
        match n {
            0 => (
                200,
                vec![
                    json!({"message": {"role": "assistant", "content": "", "tool_calls": [
                {"function": {"name": "nope", "arguments": {}}},
                {"function": {"name": "demo__items_list", "arguments": "{\"q\":\"x\"}"}}
            ]}, "done": false}),
                    done(),
                ],
            ),
            _ => (200, vec![text("Done."), done()]),
        }
    });
    let mut tools = FakeTools::default();
    let (res, events) = ask(&mock.url, &model("qwen3:8b", Some(&["tools"])), &mut tools, ollama::MAX_STEPS);
    assert_eq!(res.unwrap(), "Done.");
    assert_eq!(tools.calls, [("demo".to_string(), "items_list".to_string(), json!({"q": "x"}))]);
    assert!(events.contains(&StreamEvent::ToolResult { tool: ToolRef::new(None, "nope"), is_error: true }));
}

#[test]
fn unload_and_keep_alive() {
    let mock = Mock::start(|_, _| (200, vec![json!({"done": true})]));
    Client::new(&mock.url).unwrap().unload("qwen3:8b");
    let seen = mock.seen.lock().unwrap().clone();
    assert_eq!(seen, [("/api/generate".to_string(), json!({"model": "qwen3:8b", "keep_alive": 0}))]);
}

#[test]
fn schema_mapping() {
    let t = McpTool {
        server: "otherwise".into(),
        name: "vigia.due".into(),
        description: "  Due items  ".into(),
        input_schema: json!({"$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object", "properties": {"days": {"type": "integer", "description": "ahead"}}, "required": ["days"]}),
    };
    let f = ollama::function_name(&t.server, &t.name);
    assert_eq!(f, "otherwise__vigia_due");
    let s = ollama::tool_schema(&f, &t);
    assert_eq!(
        s,
        json!({"type": "function", "function": {"name": "otherwise__vigia_due", "description": "Due items", "parameters": {"type": "object", "properties": {"days": {"type": "integer", "description": "ahead"}}, "required": ["days"]}}})
    );
    // no schema / no description
    let bare = McpTool { server: "demo".into(), name: "ping".into(), description: String::new(), input_schema: Value::Null };
    let s = ollama::tool_schema("demo__ping", &bare);
    assert_eq!(s["function"]["parameters"], json!({"type": "object", "properties": {}}));
    assert_eq!(s["function"]["description"], "ping tool of demo");
    assert!(ollama::function_name("a", &"x".repeat(100)).len() <= 64);
}

#[test]
fn settings_helpers() {
    assert_eq!(ollama::url_or_default(None), "http://localhost:11434");
    assert_eq!(ollama::url_or_default(Some("  ")), "http://localhost:11434");
    assert!(ollama::is_local("http://localhost:11434") && ollama::is_local("127.0.0.1:11434") && ollama::is_local("http://[::1]:11434"));
    assert!(!ollama::is_local("http://100.109.149.41:11434"));
    assert_eq!(ollama::host_of("http://studio.local:11434/"), "studio.local");
    let gib16 = 16u64 << 30;
    assert!(ollama::heavy(6_550_813_676, Some(gib16)), "a 6.6 GB model on 16 GB warns");
    assert!(!ollama::heavy(2_000_000_000, Some(gib16)));
    assert_eq!(ollama::ram(gib16), "16 GB");
    assert_eq!(ollama::gb(6_550_813_676), "6.6 GB");
    assert_eq!(ProviderId::parse("ollama"), Some(ProviderId::Ollama));
    assert_eq!(ProviderId::Ollama.key(), "ollama");
    assert!(ProviderId::Ollama.cli().is_none(), "no terminal mode");
    let tmp = tempfile::tempdir().unwrap();
    let mut p = otherwise_agent_panel::Prefs::default();
    p.ollama.url = Some("http://studio:11434".into());
    p.ollama.hidden = true;
    p.provider = Some(ProviderId::Ollama);
    p.save(tmp.path()).unwrap();
    assert_eq!(otherwise_agent_panel::Prefs::load(tmp.path()), p);
    std::fs::write(tmp.path().join(".panel.json"), r#"{"dock":"right","provider":"claude"}"#).unwrap();
    let old = otherwise_agent_panel::Prefs::load(tmp.path());
    assert!(old.ollama.url.is_none() && !old.ollama.hidden, "0.1 preferences load");
}

#[cfg(unix)]
#[test]
fn mcp_client_against_a_fake_server() {
    use otherwise_agent_panel::McpServer;
    use otherwise_agent_panel::mcp_client::McpClient;
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("calls.jsonl");
    let script = tmp.path().join("fake-mcp");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
read l; echo "$l" >> '{log}'; echo '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":"2025-06-18","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"fake","version":"1"}}}}}}'
read l; echo "$l" >> '{log}'
read l; echo "$l" >> '{log}'; echo '{{"jsonrpc":"2.0","method":"notifications/message","params":{{}}}}'; echo '{{"jsonrpc":"2.0","id":2,"result":{{"tools":[{{"name":"items_list","description":"List","inputSchema":{{"type":"object"}}}}]}}}}'
read l; echo "$l" >> '{log}'; echo '{{"jsonrpc":"2.0","id":3,"result":{{"content":[{{"type":"text","text":"two items"}}],"isError":false}}}}'
read l
"#,
            log = log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut c = McpClient::start(&McpServer::new("demo", &script, &[]), tmp.path()).unwrap();
    let tools = c.list_tools().unwrap();
    assert_eq!(tools, [McpTool { server: "demo".into(), name: "items_list".into(), description: "List".into(), input_schema: json!({"type": "object"}) }]);
    assert_eq!(c.call("items_list", json!({"q": 1})).unwrap(), ("two items".to_string(), false));
    drop(c);
    let lines: Vec<Value> = std::fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.iter().map(|l| l["method"].as_str().unwrap()).collect::<Vec<_>>(), ["initialize", "notifications/initialized", "tools/list", "tools/call"]);
    assert_eq!(lines[3]["params"], json!({"name": "items_list", "arguments": {"q": 1}}));
}
