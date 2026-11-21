//! Providers, permission modes, MCP configs, argument building, attachments, history.
use otherwise_agent_panel::args::{self, Session, Turn};
use otherwise_agent_panel::attach::{self, Kind};
use otherwise_agent_panel::history::{self, Conversation, DockMode, Message, PendingChange, Prefs};
use otherwise_agent_panel::mcp::{self, McpServer};
use otherwise_agent_panel::permission::{McpWrites, PermissionMode};
use otherwise_agent_panel::provider::{self, Cli, Model, ProviderId};
use std::path::{Path, PathBuf};

fn fake_bin(dir: &Path, name: &str, exec: bool) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(if cfg!(windows) { format!("{name}.exe") } else { name.to_string() });
    std::fs::write(&p, "#!/bin/sh\necho fake\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(if exec { 0o755 } else { 0o644 })).unwrap();
    }
    let _ = exec;
    p
}

#[test]
fn detection_with_fake_binaries() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let bin = tmp.path().join("bin");
    let claude = fake_bin(&bin, "claude", true);
    // opencode only in a usual place under the home, not on PATH
    let opencode = fake_bin(&home.join(".opencode/bin"), "opencode", true);
    let path = std::env::join_paths([bin.clone()]).unwrap().into_string().unwrap();
    let found = provider::detect(Some(&path), &home);
    let clis: Vec<Cli> = found.iter().map(|(c, _)| *c).collect();
    assert!(clis.contains(&Cli::ClaudeCode));
    assert!(clis.contains(&Cli::Opencode));
    assert_eq!(found.iter().find(|(c, _)| *c == Cli::ClaudeCode).unwrap().1, claude);
    assert_eq!(found.iter().find(|(c, _)| *c == Cli::Opencode).unwrap().1, opencode);
    // the order of the picker is fixed
    let order: Vec<usize> = clis.iter().map(|c| provider::CLIS.iter().position(|x| x == c).unwrap()).collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]));
    #[cfg(unix)]
    {
        // a file that isn't executable is not an agent
        fake_bin(&bin, "codex", false);
        assert!(provider::find(Cli::Codex, Some(&path), &home).is_none());
    }
}

#[test]
fn model_lists() {
    let claude = provider::claude_models();
    assert_eq!(claude[0].id, None, "Default first");
    let ids: Vec<_> = claude.iter().filter_map(|m| m.id.clone()).collect();
    assert_eq!(ids, ["opus", "sonnet", "haiku", "fable"]);
    assert!(claude.iter().any(|m| m.label == "Opus 5.5"));
    let oc = provider::parse_opencode_models("anthropic/claude-sonnet-5-5\nopenai/gpt-5.5\n\nsome warning line\nanthropic/claude-sonnet-5-5\n");
    assert_eq!(oc.iter().filter_map(|m| m.id.clone()).collect::<Vec<_>>(), ["anthropic/claude-sonnet-5-5", "openai/gpt-5.5"]);
    let cache = r#"{"models":[{"slug":"gpt-5.5","display_name":"GPT-5.5"},{"slug":"internal","visibility":"hide"},{"slug":"gpt-5.4-mini"}]}"#;
    let codex = provider::codex_models_from_cache(cache).unwrap();
    assert_eq!(codex.iter().map(|m| m.label.as_str()).collect::<Vec<_>>()[1..], ["GPT-5.5", "gpt-5.4-mini"]);
    assert!(provider::codex_models_from_cache("{}").is_none());
    // a CLI without a cache falls back to the maintained list
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(provider::models(Cli::Codex, Path::new("codex"), tmp.path()), provider::codex_models_fallback());
}

#[test]
fn provider_ids_round_trip() {
    for (id, key) in [(ProviderId::Cli(Cli::ClaudeCode), "claude"), (ProviderId::Cli(Cli::Codex), "codex"), (ProviderId::Remote("vcity".into()), "remote:vcity")] {
        assert_eq!(id.key(), key);
        assert_eq!(ProviderId::parse(key), Some(id.clone()));
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<ProviderId>(&json).unwrap(), id);
    }
    assert_eq!(serde_json::from_str::<ProviderId>("\"claude_code\"").unwrap(), ProviderId::Cli(Cli::ClaudeCode));
    assert_eq!(ProviderId::parse("remote:"), None);
    assert_eq!(ProviderId::parse("gemini"), None);
}

#[test]
fn permission_mode_mapping() {
    use PermissionMode::*;
    assert_eq!(ReadOnly.writes(), McpWrites::Off);
    assert_eq!(Ask.writes(), McpWrites::Propose);
    assert_eq!(Auto.writes(), McpWrites::On);
    assert_eq!(ReadOnly.claude_flag(true), Some("dontAsk"));
    assert_eq!(Ask.claude_flag(true), None, "the interactive CLI asks before each tool");
    assert_eq!(Ask.claude_flag(false), Some("dontAsk"), "a headless run can't ask");
    assert_eq!(Auto.claude_flag(false), Some("acceptEdits"));
    assert_eq!(Ask.codex_approval(true), "on-request");
    assert_eq!(Ask.codex_approval(false), "never");
    assert_eq!(Auto.codex_approval(true), "never");
    assert_eq!(Ask.opencode_permission(true), "ask");
    assert_eq!(ReadOnly.opencode_permission(true), "deny");
    assert_eq!(PermissionMode::default(), Ask);
}

fn servers() -> Vec<McpServer> {
    let mut s = McpServer::new("expense", "/Applications/expense.app/Contents/MacOS/expense-cli", &["mcp", "--propose-writes"]);
    s.env.push(("EXPENSE_SOCKET".into(), "/tmp/x \"q\".sock".into()));
    vec![s, McpServer::hub(Path::new("/Applications/Otherwise Apps.app/Contents/MacOS/otherwise-apps"))]
}

fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

#[test]
fn claude_chat_arguments() {
    let tmp = tempfile::tempdir().unwrap();
    let servers = servers();
    let s = Session { cli: Cli::ClaudeCode, program: Path::new("/bin/claude"), model: Some("opus"), mode: PermissionMode::ReadOnly, servers: &servers, dir: tmp.path() };
    let turn = Turn { system: "SYS", context: Some("{\"screen\":\"receipts\"}"), history: &[(true, "hi".into()), (false, "hello".into())], text: "How many?", attachments: &[] };
    let inv = args::chat(&s, &turn).unwrap();
    for f in ["-p", "--strict-mcp-config", "--no-session-persistence", "--include-partial-messages", "--verbose"] {
        assert!(inv.args.iter().any(|a| a == f), "missing {f}");
    }
    assert_eq!(after(&inv.args, "--input-format"), Some("stream-json"));
    assert_eq!(after(&inv.args, "--output-format"), Some("stream-json"));
    assert_eq!(after(&inv.args, "--model"), Some("opus"));
    assert_eq!(after(&inv.args, "--permission-mode"), Some("dontAsk"));
    assert_eq!(after(&inv.args, "--tools"), Some("Read"));
    assert!(inv.args.iter().any(|a| a == "mcp__expense__*") && inv.args.iter().any(|a| a == "mcp__otherwise__*"));
    // the MCP config is a new file in the session folder, removed after the run
    let cfg = PathBuf::from(after(&inv.args, "--mcp-config").unwrap());
    assert!(cfg.starts_with(tmp.path()));
    assert_eq!(inv.temp_files, vec![cfg.clone()]);
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(v["mcpServers"]["expense"]["args"], serde_json::json!(["mcp", "--propose-writes"]));
    assert_eq!(v["mcpServers"]["expense"]["env"]["EXPENSE_SOCKET"], "/tmp/x \"q\".sock");
    assert_eq!(v["mcpServers"]["otherwise"]["args"], serde_json::json!(["mcp"]));
    // stdin: one stream-json user message with everything in its text block
    let msg: serde_json::Value = serde_json::from_str(inv.stdin.trim()).unwrap();
    assert_eq!(msg["type"], "user");
    let text = msg["message"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("SYS") && text.contains("receipts") && text.contains("User: hi") && text.contains("Assistant: hello") && text.ends_with("User: How many?\n"));
    assert_eq!(inv.cwd, tmp.path());
    // Default model: no --model
    let s2 = Session { model: None, mode: PermissionMode::Auto, ..s.clone() };
    let inv2 = args::chat(&s2, &turn).unwrap();
    assert!(!inv2.args.iter().any(|a| a == "--model"));
    assert_eq!(after(&inv2.args, "--permission-mode"), Some("acceptEdits"));
}

#[test]
fn claude_inline_images_and_pdfs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("attachments");
    let src = tmp.path().join("shot.png");
    std::fs::write(&src, b"\x89PNG fake").unwrap();
    let pdf = tmp.path().join("Factura café.pdf");
    std::fs::write(&pdf, b"%PDF-1.4").unwrap();
    let csv = tmp.path().join("data.csv");
    std::fs::write(&csv, b"a,b").unwrap();
    let atts: Vec<_> = [&src, &pdf, &csv].iter().map(|p| attach::import(p, &dir).unwrap()).collect();
    let turn = Turn { system: "S", text: "look", attachments: &atts, ..Default::default() };
    let (msg, rest) = args::claude_message(&turn);
    let blocks = msg["message"]["content"].as_array().unwrap();
    assert_eq!(blocks[1]["type"], "image");
    assert_eq!(blocks[1]["source"]["media_type"], "image/png");
    assert_eq!(blocks[1]["source"]["data"], attach::base64(b"\x89PNG fake"));
    assert_eq!(blocks[2]["type"], "document");
    assert_eq!(blocks[2]["title"], "Factura café.pdf");
    assert_eq!(rest.len(), 1, "the CSV goes by path");
    assert!(blocks[0]["text"].as_str().unwrap().contains(&atts[2].path.display().to_string()));
}

#[test]
fn codex_and_opencode_arguments() {
    let tmp = tempfile::tempdir().unwrap();
    let servers = servers();
    let img = tmp.path().join("a.png");
    std::fs::write(&img, b"x").unwrap();
    let doc = tmp.path().join("b.pdf");
    std::fs::write(&doc, b"y").unwrap();
    let atts = vec![attach::import(&img, &tmp.path().join("att")).unwrap(), attach::import(&doc, &tmp.path().join("att")).unwrap()];
    let turn = Turn { system: "S", text: "q", attachments: &atts, ..Default::default() };
    let s = Session { cli: Cli::Codex, program: Path::new("codex"), model: Some("gpt-5.5"), mode: PermissionMode::Ask, servers: &servers, dir: tmp.path() };
    let inv = args::chat(&s, &turn).unwrap();
    assert_eq!(&inv.args[..2], ["exec", "--json"]);
    assert_eq!(after(&inv.args, "--sandbox"), Some("read-only"));
    assert!(inv.args.contains(&"approval_policy=\"never\"".to_string()));
    assert!(inv.args.contains(&"mcp_servers.expense.command=\"/Applications/expense.app/Contents/MacOS/expense-cli\"".to_string()));
    assert!(inv.args.contains(&"mcp_servers.expense.args=[\"mcp\", \"--propose-writes\"]".to_string()));
    assert!(inv.args.contains(&"mcp_servers.expense.env={ \"EXPENSE_SOCKET\" = \"/tmp/x \\\"q\\\".sock\" }".to_string()));
    assert!(inv.args.contains(&format!("--image={}", atts[0].path.display())));
    assert!(!inv.args.iter().any(|a| a.contains("b.pdf")), "PDFs go by path in the prompt");
    assert!(inv.stdin.contains("b.pdf"));
    assert_eq!(inv.args.last().unwrap(), "-", "prompt on stdin");
    assert!(inv.temp_files.is_empty());

    let s = Session { cli: Cli::Opencode, program: Path::new("opencode"), model: Some("anthropic/claude-sonnet-5-5"), mode: PermissionMode::ReadOnly, servers: &servers, dir: tmp.path() };
    let inv = args::chat(&s, &turn).unwrap();
    assert_eq!(&inv.args[..3], ["run", "--format", "json"]);
    assert_eq!(after(&inv.args, "--model"), Some("anthropic/claude-sonnet-5-5"));
    assert_eq!(inv.args.iter().filter(|a| a.starts_with("--file=")).count(), 2);
    let (k, v) = &inv.env[0];
    assert_eq!(k, "OPENCODE_CONFIG_CONTENT");
    let cfg: serde_json::Value = serde_json::from_str(v).unwrap();
    assert_eq!(cfg["mcp"]["expense"]["command"][1], "mcp");
    assert_eq!(cfg["mcp"]["expense"]["environment"]["EXPENSE_SOCKET"], "/tmp/x \"q\".sock");
    assert_eq!(cfg["tools"]["bash"], false);
    assert_eq!(cfg["permission"]["edit"], "deny");
}

#[test]
fn terminal_arguments_and_instructions() {
    let tmp = tempfile::tempdir().unwrap();
    let servers = servers();
    let instr = args::terminal_instructions("SYS", Some("screen: today"));
    assert!(instr.contains("SYS") && instr.contains("screen: today"));
    let s = Session { cli: Cli::ClaudeCode, program: Path::new("claude"), model: Some("sonnet"), mode: PermissionMode::Ask, servers: &servers, dir: tmp.path() };
    let inv = args::terminal(&s, &instr).unwrap();
    assert!(!inv.args.iter().any(|a| a == "-p"), "interactive");
    assert!(!inv.args.iter().any(|a| a == "--permission-mode"), "Ask: the CLI's own prompts");
    assert_eq!(after(&inv.args, "--append-system-prompt"), Some(instr.as_str()));
    assert_eq!(PathBuf::from(after(&inv.args, "--mcp-config").unwrap()), tmp.path().join("mcp-terminal.json"));
    assert!(tmp.path().join("mcp-terminal.json").is_file());

    let s = Session { cli: Cli::Codex, ..s.clone() };
    let inv = args::terminal(&s, &instr).unwrap();
    assert!(inv.args.contains(&"approval_policy=\"on-request\"".to_string()));
    assert!(!inv.args.iter().any(|a| a == "exec"));
    assert_eq!(std::fs::read_to_string(tmp.path().join(args::AGENTS_FILE)).unwrap(), instr);

    let s = Session { cli: Cli::Opencode, ..s.clone() };
    let inv = args::terminal(&s, &instr).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&inv.env[0].1).unwrap();
    assert_eq!(cfg["permission"]["bash"], "ask");
}

#[test]
fn mcp_helpers() {
    assert_eq!(mcp::toml_string("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    assert_eq!(mcp::sanitize("Vigía app"), "Vig_a_app");
    assert_eq!(McpServer::new("", "x", &[]).name, "app");
    assert_eq!(mcp::claude_allowed(&servers()), ["mcp__expense__*", "mcp__otherwise__*"]);
}

#[test]
fn attachments() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session/attachments");
    let src = tmp.path().join("Recibo café (1).JPG");
    std::fs::write(&src, b"jpg").unwrap();
    let a = attach::import(&src, &dir).unwrap();
    assert_eq!(a.kind, Kind::Image);
    assert_eq!(a.mime, "image/jpeg");
    assert_eq!(a.name, "Recibo café (1).JPG");
    assert_eq!(a.path, dir.join("Recibo_caf_1.jpg"));
    assert!(src.exists(), "the original stays where it was");
    let b = attach::import(&src, &dir).unwrap();
    assert_eq!(b.path, dir.join("Recibo_caf_1-2.jpg"), "never overwrites");
    let again = attach::import(&a.path, &dir).unwrap();
    assert_eq!(again.path, a.path, "a file already in the session isn't copied again");
    assert!(attach::import(tmp.path(), &dir).is_err(), "folders are refused");
    let p = attach::save_bytes(b"\x89PNG", "png", &dir).unwrap();
    assert!(p.path.starts_with(&dir) && p.path.extension().unwrap() == "png" && p.kind == Kind::Image);
    assert_eq!(attach::safe_name("../../etc/passwd"), "passwd");
    assert_eq!(attach::base64(b"Ma"), "TWE=");
    assert_eq!(attach::base64(b"Man"), "TWFu");
    assert_eq!(attach::base64(b"M"), "TQ==");
    assert_eq!(attach::human_size(2048), "2 KB");
    #[cfg(unix)]
    {
        assert_eq!(attach::quote_path(Path::new("/tmp/plain.png")), "/tmp/plain.png");
        assert_eq!(attach::quote_path(Path::new("/tmp/a b/it's.png")), "'/tmp/a b/it'\\''s.png'");
        assert_eq!(attach::paths_to_text(&[PathBuf::from("/a b"), PathBuf::from("/c")]), " '/a b' /c ");
    }
    assert_eq!(attach::paths_to_text(&[]), "");
}

#[test]
fn history_store_and_compat() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = history::dir_in(tmp.path(), "expense");
    assert_eq!(dir, tmp.path().join("expense/assistant"));
    let mut c = Conversation::new();
    c.push(Message::user("  ¿Cuánto gasté en octubre en restaurantes y cafés, separado por semana?  ", vec![]));
    let mut a = Message::assistant("Unos 300.");
    a.changes.push(PendingChange::new("c1", "Recategorizar", ""));
    c.push(a);
    c.provider = Some(ProviderId::Remote("vcity".into()));
    c.save(&dir).unwrap();
    std::fs::write(dir.join("broken.json"), "{").unwrap();
    Prefs::default().save(&dir).unwrap();
    let old = include_str!("fixtures/otherwise_apps_0_3.json");
    std::fs::write(dir.join("1763700000-0000abcd.json"), old).unwrap();
    let mut list = history::list(&dir);
    assert_eq!(list.len(), 2, "broken files and the prefs file are skipped");
    assert_eq!(list[0].id, c.id);
    assert_eq!(list[0].title.chars().count(), 61);
    assert!(list[0].title.ends_with('…'));
    assert_eq!(list[0].provider, Some(ProviderId::Remote("vcity".into())));
    assert!(list[0].change_mut("c1").is_some());
    let o = &list[1];
    assert_eq!(o.provider, Some(ProviderId::Cli(Cli::ClaudeCode)), "Otherwise Apps 0.3 conversations load");
    assert_eq!(o.messages[1].tools[0].tool.label(), "Vigía · due");
    assert!(o.messages[1].tools[1].failed);
    assert_eq!(o.pairs().len(), 2);
    let session = history::session_dir(&dir, &c.id);
    std::fs::create_dir_all(history::attachments_dir(&session)).unwrap();
    Conversation::delete(&dir, &c.id).unwrap();
    assert!(!session.exists());
    assert_eq!(history::list(&dir).len(), 1);
    assert!(Conversation::delete(&dir, "../x").is_err());
    assert_eq!(history::session_dir(&dir, "../../x"), dir.join("sessions/invalid"));
}

#[test]
fn prefs_dock_toggle_and_clamp() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = Prefs::default();
    assert_eq!(p.dock, DockMode::Hidden);
    p.toggle();
    assert_eq!(p.dock, DockMode::Right);
    p.set_dock(DockMode::Bottom);
    p.toggle();
    assert_eq!(p.dock, DockMode::Hidden);
    p.toggle();
    assert_eq!(p.dock, DockMode::Bottom, "⌘J brings back the last visible mode");
    p.width = 10.;
    p.models.insert("claude".into(), "opus".into());
    p.save(tmp.path()).unwrap();
    let q = Prefs::load(tmp.path());
    assert_eq!(q.width, history::MIN_WIDTH);
    assert_eq!(q.model_for(&ProviderId::Cli(Cli::ClaudeCode)).as_deref(), Some("opus"));
    assert_eq!(Prefs::load(&tmp.path().join("missing")), Prefs::default());
    let _ = Model::default_model();
}
