//! The MCP servers an agent gets for one session, and how each CLI is told about them without
//! touching its configuration files: Claude Code a temporary `--mcp-config` file in the session
//! folder plus `--strict-mcp-config` (so the user's own servers stay out), Codex
//! `-c mcp_servers.<name>.*` overrides, opencode an inline `OPENCODE_CONFIG_CONTENT`.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The name the Otherwise hub gets inside the agent.
pub const HUB: &str = "otherwise";

/// One stdio MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    /// Name inside the agent (`expense`, `otherwise`): tool names are prefixed with it. ASCII
    /// letters, digits, `-` and `_` only.
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl McpServer {
    pub fn new(name: &str, program: impl Into<PathBuf>, args: &[&str]) -> McpServer {
        McpServer { name: sanitize(name), program: program.into(), args: args.iter().map(|s| s.to_string()).collect(), env: Vec::new() }
    }

    /// The Otherwise hub (one server for every installed Otherwise app) as `<exe> mcp`.
    pub fn hub(exe: &Path) -> McpServer {
        McpServer::new(HUB, exe, &["mcp"])
    }

    /// The hub, if one is installed: `otherwise-hub` on the PATH / usual places, else the
    /// installed Otherwise Apps executable with `mcp`.
    pub fn find_hub() -> Option<McpServer> {
        let home = otherwise_link::paths::home();
        let path = crate::provider::login_shell_path();
        let dirs: Vec<PathBuf> = path.as_deref().map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
        let exe = if cfg!(windows) { "otherwise-hub.exe" } else { "otherwise-hub" };
        if let Some(p) = dirs.into_iter().chain(crate::provider::usual_dirs(&home)).map(|d| d.join(exe)).find(|p| p.is_file()) {
            return Some(McpServer { name: HUB.into(), program: p, args: vec![], env: vec![] });
        }
        let loc = otherwise_link::apps::Locations::system();
        let spec = otherwise_link::apps::known(otherwise_link::apps::HUB_ID)?.spec();
        let found = otherwise_link::apps::detect(&spec, &loc)?;
        found.executable.map(|e| McpServer::hub(&e))
    }
}

/// MCP server names become parts of tool names (`mcp__<name>__tool`).
pub fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if s.is_empty() { "app".into() } else { s }
}

/// Claude Code's `--mcp-config` content.
pub fn claude_config(servers: &[McpServer]) -> Value {
    let mut m = serde_json::Map::new();
    for s in servers {
        let env: serde_json::Map<String, Value> = s.env.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        m.insert(s.name.clone(), json!({"type": "stdio", "command": s.program.display().to_string(), "args": s.args, "env": env}));
    }
    json!({ "mcpServers": m })
}

/// Claude Code's `--allowedTools` patterns: every tool of our servers (whether a tool may
/// write is decided by the server, from the permission mode).
pub fn claude_allowed(servers: &[McpServer]) -> Vec<String> {
    servers.iter().map(|s| format!("mcp__{}__*", s.name)).collect()
}

/// A TOML basic string (Codex parses `-c key=value` values as TOML).
pub fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Codex's `-c` overrides (each becomes `-c <item>`).
pub fn codex_overrides(servers: &[McpServer]) -> Vec<String> {
    let mut out = Vec::new();
    for s in servers {
        out.push(format!("mcp_servers.{}.command={}", s.name, toml_string(&s.program.display().to_string())));
        out.push(format!("mcp_servers.{}.args=[{}]", s.name, s.args.iter().map(|a| toml_string(a)).collect::<Vec<_>>().join(", ")));
        if !s.env.is_empty() {
            let env: Vec<String> = s.env.iter().map(|(k, v)| format!("{} = {}", toml_string(k), toml_string(v))).collect();
            out.push(format!("mcp_servers.{}.env={{ {} }}", s.name, env.join(", ")));
        }
    }
    out
}

/// opencode's inline config: our servers, its file / shell / web tools off (reading kept so it
/// can open attachments), and the permission for anything left.
pub fn opencode_config(servers: &[McpServer], permission: &str) -> Value {
    let mut mcp = serde_json::Map::new();
    for s in servers {
        let mut cmd = vec![s.program.display().to_string()];
        cmd.extend(s.args.iter().cloned());
        let env: serde_json::Map<String, Value> = s.env.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        mcp.insert(s.name.clone(), json!({"type": "local", "command": cmd, "enabled": true, "environment": env}));
    }
    json!({
        "mcp": mcp,
        "tools": {"bash": false, "edit": false, "write": false, "patch": false, "webfetch": false},
        "permission": {"edit": permission, "bash": permission, "webfetch": permission},
    })
}
