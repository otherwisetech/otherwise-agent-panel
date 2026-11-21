//! Stream parsers on recorded output, a fake CLI run end to end, the fake remote provider.
use otherwise_agent_panel::provider::Cli;
use otherwise_agent_panel::remote::{Cancel, FakeRemote, RemoteProvider, RemoteTurn};
use otherwise_agent_panel::stream::{AgentError, Parser, StreamEvent, ToolRef, classify, tool_ref};
use otherwise_agent_panel::{PermissionMode, attach};

fn parse(cli: Cli, fixture: &str) -> (Vec<StreamEvent>, String) {
    let mut p = Parser::new(cli, vec!["expense".into(), "otherwise".into()]);
    let events: Vec<StreamEvent> = fixture.lines().flat_map(|l| p.line(l)).collect();
    (events, p.final_text)
}

fn tools(events: &[StreamEvent]) -> (Vec<ToolRef>, Vec<(ToolRef, bool)>) {
    let calls = events.iter().filter_map(|e| if let StreamEvent::ToolCall(t) = e { Some(t.clone()) } else { None }).collect();
    let results = events.iter().filter_map(|e| if let StreamEvent::ToolResult { tool, is_error } = e { Some((tool.clone(), *is_error)) } else { None }).collect();
    (calls, results)
}

#[test]
fn claude_stream_json() {
    let (events, text) = parse(Cli::ClaudeCode, include_str!("fixtures/claude.jsonl"));
    assert_eq!(text, "Let me check your receipts.\n\nYou have **3** receipts.", "partial text once, not repeated by the whole message");
    let (calls, results) = tools(&events);
    assert_eq!(calls, [ToolRef::new(Some("expense"), "receipts_list"), ToolRef::new(Some("vigia"), "due")]);
    assert_eq!(calls[0].label(), "expense · receipts_list");
    assert_eq!(results, [(calls[0].clone(), false), (calls[1].clone(), true)]);
    let (events, _) = parse(Cli::ClaudeCode, include_str!("fixtures/claude_login.jsonl"));
    let err = events.iter().find_map(|e| if let StreamEvent::Error(m) = e { Some(m.clone()) } else { None }).unwrap();
    assert_eq!(classify(Cli::ClaudeCode, &err), AgentError::NotLoggedIn(Cli::ClaudeCode));
}

#[test]
fn codex_json() {
    let (events, text) = parse(Cli::Codex, include_str!("fixtures/codex.jsonl"));
    assert_eq!(text, "You have 3 receipts.");
    let (calls, results) = tools(&events);
    assert_eq!(calls, [ToolRef::new(Some("expense"), "receipts_list"), ToolRef::new(Some("nodo"), "devices")]);
    assert_eq!(results.iter().map(|r| r.1).collect::<Vec<_>>(), [false, true]);
}

#[test]
fn opencode_json() {
    let (events, text) = parse(Cli::Opencode, include_str!("fixtures/opencode.jsonl"));
    assert_eq!(text, "You have 3 receipts.");
    let (calls, results) = tools(&events);
    assert_eq!(calls, [ToolRef::new(Some("expense"), "receipts_list"), ToolRef::new(Some("vigia"), "due")]);
    assert_eq!(results.iter().map(|r| r.1).collect::<Vec<_>>(), [false, true]);
}

#[test]
fn tool_names() {
    let s = vec!["expense".to_string()];
    assert_eq!(tool_ref(&s, "mcp__expense__receipts_list", None), ToolRef::new(Some("expense"), "receipts_list"));
    assert_eq!(tool_ref(&s, "expense_receipts_list", None), ToolRef::new(Some("expense"), "receipts_list"));
    assert_eq!(tool_ref(&s, "Read", None), ToolRef::new(None, "Read"));
    let hub = vec!["otherwise".to_string()];
    assert_eq!(tool_ref(&hub, "mcp__otherwise__today", None), ToolRef::new(None, "today"));
    assert_eq!(tool_ref(&hub, "vigia.due", None), ToolRef::new(Some("vigia"), "due"));
    assert_eq!(classify(Cli::Codex, "boom").clone(), AgentError::Failed("boom".into()));
}

#[cfg(unix)]
#[test]
fn runs_a_fake_cli_and_cleans_up() {
    use otherwise_agent_panel::args::{self, Session, Turn};
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let fixture = tmp.path().join("out.jsonl");
    std::fs::write(&fixture, include_str!("fixtures/claude.jsonl")).unwrap();
    let stdin_copy = tmp.path().join("stdin.txt");
    let script = tmp.path().join("claude");
    std::fs::write(&script, format!("#!/bin/sh\ncat > '{}'\ncat '{}'\n", stdin_copy.display(), fixture.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let session = tmp.path().join("session");
    std::fs::create_dir_all(&session).unwrap();
    let s = Session { cli: Cli::ClaudeCode, program: &script, model: None, mode: PermissionMode::ReadOnly, servers: &[], dir: &session };
    let inv = args::chat(&s, &Turn { system: "S", text: "hola", ..Default::default() }).unwrap();
    assert_eq!(inv.temp_files.len(), 1);
    let mut seen = Vec::new();
    let text = otherwise_agent_panel::run::run(Cli::ClaudeCode, &inv, vec!["expense".into()], &Cancel::new(), &mut |e| seen.push(e)).unwrap();
    assert!(text.ends_with("You have **3** receipts."));
    assert!(seen.iter().any(|e| matches!(e, StreamEvent::ToolCall(_))));
    assert!(!inv.temp_files[0].exists(), "the per-session MCP config is removed");
    assert!(std::fs::read_to_string(&stdin_copy).unwrap().contains("\"type\":\"user\""));

    // stop: the child is killed and the run says so
    let slow = tmp.path().join("slow");
    std::fs::write(&slow, "#!/bin/sh\nsleep 30\n").unwrap();
    std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut inv2 = inv.clone();
    inv2.program = slow;
    inv2.temp_files.clear();
    let cancel = Cancel::new();
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        c2.cancel();
    });
    let start = std::time::Instant::now();
    assert_eq!(otherwise_agent_panel::run::run(Cli::ClaudeCode, &inv2, vec![], &cancel, &mut |_| {}), Err(AgentError::Cancelled));
    assert!(start.elapsed() < std::time::Duration::from_secs(10));

    // a missing program is "not installed"
    let mut inv3 = inv.clone();
    inv3.program = tmp.path().join("nope");
    assert_eq!(otherwise_agent_panel::run::run(Cli::Codex, &inv3, vec![], &Cancel::new(), &mut |_| {}), Err(AgentError::NotInstalled(Cli::Codex)));
}

#[test]
fn fake_remote_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("r.png");
    std::fs::write(&f, b"x").unwrap();
    let a = attach::import(&f, &tmp.path().join("att")).unwrap();
    let r = FakeRemote::default();
    assert_eq!(r.models().len(), 3);
    let turn = RemoteTurn {
        conversation_id: "c1".into(),
        model: Some("fake-large".into()),
        mode: PermissionMode::Ask,
        system: "S".into(),
        context: None,
        history: vec![],
        text: "hola".into(),
        attachments: vec![a],
        language: "es".into(),
    };
    let mut events = Vec::new();
    let text = r.send(turn.clone(), &mut |e| events.push(e), &Cancel::new()).unwrap();
    assert!(text.contains("**hola**") && text.contains("fake-large") && text.contains("r.png"));
    assert!(matches!(events[0], StreamEvent::ToolCall(_)));
    let streamed: String = events.iter().filter_map(|e| if let StreamEvent::Delta(d) = e { Some(d.as_str()) } else { None }).collect();
    assert_eq!(streamed, text);
    let c = Cancel::new();
    c.cancel();
    assert_eq!(r.send(turn, &mut |_| {}, &c), Err(AgentError::Cancelled));
}
