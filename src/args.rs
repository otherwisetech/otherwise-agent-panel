//! The exact command line for a chat turn (headless, streaming JSON) or a terminal session
//! (the interactive CLI in a PTY), per provider, permission mode and model. Nothing goes
//! through a shell: arguments are passed as they are.
use crate::attach::{Attachment, Kind, MAX_INLINE};
use crate::mcp::{self, McpServer};
use crate::permission::PermissionMode;
use crate::provider::Cli;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The parts of a session every invocation needs.
#[derive(Debug, Clone)]
pub struct Session<'a> {
    pub cli: Cli,
    pub program: &'a Path,
    /// `None`: Default (the CLI picks).
    pub model: Option<&'a str>,
    pub mode: PermissionMode,
    pub servers: &'a [McpServer],
    /// The session folder: the agent's working directory.
    pub dir: &'a Path,
}

/// One question in chat mode.
#[derive(Debug, Clone, Default)]
pub struct Turn<'a> {
    pub system: &'a str,
    /// "What I'm looking at" from the host, if any.
    pub context: Option<&'a str>,
    /// Earlier `(from the user, text)` pairs (the CLI starts fresh every turn).
    pub history: &'a [(bool, String)],
    pub text: &'a str,
    pub attachments: &'a [Attachment],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Written to stdin, then stdin is closed (chat mode only).
    pub stdin: String,
    pub cwd: PathBuf,
    /// Removed after the run.
    pub temp_files: Vec<PathBuf>,
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn unique(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    dir.join(format!("{stem}-{}-{}.{ext}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)))
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn model(s: &Session) -> Option<String> {
    s.model.map(str::trim).filter(|m| !m.is_empty()).map(str::to_string)
}

/// What the agent is told before the question (English: it is for the model, not the user).
pub fn system_prompt(app_name: &str, extra: Option<&str>, language: &str, today: &str) -> String {
    let mut p = format!(
        "You are the assistant inside {app_name}, a desktop app by Otherwise Technologies. Answer questions about the user's own data using the tools of the MCP servers you were given \
         (tool names start with the app they belong to). Do not run commands or browse the web; read files only inside your working folder (attachments the user added). \
         If a tool you need is not available, say so. Only change data when the user asks and a write tool is offered; when a write tool reports a change as pending, tell the user it is waiting for their approval in the panel. \
         Be brief and concrete. Today is {today}. Answer in the user's language (the app is set to \"{language}\")."
    );
    if let Some(e) = extra.map(str::trim).filter(|e| !e.is_empty()) {
        p.push_str("\n\n");
        p.push_str(e);
    }
    p
}

/// The text part of a chat turn: system, context, the conversation so far, the question, and
/// the attachments that aren't sent inline.
pub fn prompt_text(turn: &Turn, not_inline: &[&Attachment]) -> String {
    let mut p = format!("{}\n", turn.system.trim());
    if let Some(c) = turn.context.map(str::trim).filter(|c| !c.is_empty()) {
        p.push_str("\nWhat the user is looking at in the app right now:\n");
        p.push_str(c);
        p.push('\n');
    }
    if !turn.history.is_empty() {
        p.push_str("\nConversation so far:\n");
        for (user, text) in turn.history.iter().rev().take(12).rev() {
            p.push_str(if *user { "User: " } else { "Assistant: " });
            p.push_str(text.trim());
            p.push('\n');
        }
    }
    if !not_inline.is_empty() {
        p.push_str("\nFiles the user attached (in your working folder):\n");
        for a in not_inline {
            p.push_str(&format!("- {} ({})\n", a.path.display(), a.name));
        }
    }
    p.push_str("\nUser: ");
    p.push_str(turn.text.trim());
    p.push('\n');
    p
}

/// Claude Code's stream-json user message: the text plus images / PDFs as content blocks.
/// Returns the message and the attachments left for the text.
pub fn claude_message<'a>(turn: &'a Turn<'a>) -> (Value, Vec<&'a Attachment>) {
    let mut blocks = Vec::new();
    let mut rest = Vec::new();
    for a in turn.attachments {
        let inline = matches!(a.kind, Kind::Image | Kind::Pdf) && a.bytes <= MAX_INLINE;
        let data = if inline { std::fs::read(&a.path).ok() } else { None };
        match (a.kind, data) {
            (Kind::Image, Some(d)) => blocks.push(json!({"type": "image", "source": {"type": "base64", "media_type": a.mime, "data": crate::attach::base64(&d)}})),
            (Kind::Pdf, Some(d)) => blocks.push(json!({"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": crate::attach::base64(&d)}, "title": a.name})),
            _ => rest.push(a),
        }
    }
    blocks.insert(0, json!({"type": "text", "text": prompt_text(turn, &rest)}));
    (json!({"type": "user", "message": {"role": "user", "content": blocks}}), rest)
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    std::io::Write::write_all(&mut f, bytes)
}

fn claude_common(s: &Session, inv: &mut Invocation, interactive: bool, cfg: PathBuf) -> std::io::Result<()> {
    let body = serde_json::to_string_pretty(&mcp::claude_config(s.servers)).unwrap_or_default();
    if interactive {
        otherwise_link::fsutil::write_atomic(&cfg, body.as_bytes())?;
    } else {
        write_new(&cfg, body.as_bytes())?;
        inv.temp_files.push(cfg.clone());
    }
    inv.args.extend(strs(&["--strict-mcp-config", "--mcp-config"]));
    inv.args.push(cfg.display().to_string());
    // built-in tools: only Read (attachments in the session folder)
    inv.args.extend(strs(&["--tools", "Read", "--allowedTools"]));
    inv.args.extend(mcp::claude_allowed(s.servers));
    inv.args.push("Read".into());
    if let Some(p) = s.mode.claude_flag(interactive) {
        inv.args.extend(["--permission-mode".to_string(), p.to_string()]);
    }
    if let Some(m) = model(s) {
        inv.args.extend(["--model".to_string(), m]);
    }
    Ok(())
}

fn codex_common(s: &Session, inv: &mut Invocation, interactive: bool) {
    inv.args.extend(strs(&["--sandbox", "read-only"]));
    inv.args.extend(["-c".to_string(), format!("approval_policy={}", mcp::toml_string(s.mode.codex_approval(interactive)))]);
    for o in mcp::codex_overrides(s.servers) {
        inv.args.extend(["-c".to_string(), o]);
    }
    if let Some(m) = model(s) {
        inv.args.extend(["--model".to_string(), m]);
    }
}

fn opencode_env(s: &Session, inv: &mut Invocation, interactive: bool) {
    inv.env.push(("OPENCODE_CONFIG_CONTENT".into(), mcp::opencode_config(s.servers, s.mode.opencode_permission(interactive)).to_string()));
}

/// The command for one chat turn.
pub fn chat(s: &Session, turn: &Turn) -> std::io::Result<Invocation> {
    let mut inv = Invocation { program: s.program.to_path_buf(), args: Vec::new(), env: Vec::new(), stdin: String::new(), cwd: s.dir.to_path_buf(), temp_files: Vec::new() };
    match s.cli {
        Cli::ClaudeCode => {
            inv.args = strs(&["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--no-session-persistence"]);
            claude_common(s, &mut inv, false, unique(s.dir, "mcp-chat", "json"))?;
            let (msg, _) = claude_message(turn);
            inv.stdin = format!("{msg}\n");
        }
        Cli::Codex => {
            inv.args = strs(&["exec", "--json", "--skip-git-repo-check", "--color", "never"]);
            codex_common(s, &mut inv, false);
            let (images, rest): (Vec<&Attachment>, Vec<&Attachment>) = turn.attachments.iter().partition(|a| a.kind == Kind::Image);
            for a in images {
                inv.args.push(format!("--image={}", a.path.display()));
            }
            inv.stdin = prompt_text(turn, &rest);
            inv.args.push("-".into());
        }
        Cli::Opencode => {
            opencode_env(s, &mut inv, false);
            inv.args = strs(&["run", "--format", "json"]);
            if let Some(m) = model(s) {
                inv.args.extend(["--model".to_string(), m]);
            }
            for a in turn.attachments {
                inv.args.push(format!("--file={}", a.path.display()));
            }
            inv.stdin = prompt_text(turn, &[]);
        }
    }
    Ok(inv)
}

/// The file the terminal session's instructions go in for Codex and opencode (both read
/// `AGENTS.md` from their working directory; Claude Code gets `--append-system-prompt`).
pub const AGENTS_FILE: &str = "AGENTS.md";

/// The interactive CLI for terminal mode. `instructions` (system prompt + context) are fixed at
/// launch; relaunch to refresh them.
pub fn terminal(s: &Session, instructions: &str) -> std::io::Result<Invocation> {
    let mut inv = Invocation { program: s.program.to_path_buf(), args: Vec::new(), env: Vec::new(), stdin: String::new(), cwd: s.dir.to_path_buf(), temp_files: Vec::new() };
    std::fs::create_dir_all(s.dir)?;
    match s.cli {
        Cli::ClaudeCode => {
            claude_common(s, &mut inv, true, s.dir.join("mcp-terminal.json"))?;
            if !instructions.trim().is_empty() {
                inv.args.extend(["--append-system-prompt".to_string(), instructions.to_string()]);
            }
        }
        Cli::Codex => {
            codex_common(s, &mut inv, true);
            otherwise_link::fsutil::write_atomic(&s.dir.join(AGENTS_FILE), instructions.as_bytes())?;
        }
        Cli::Opencode => {
            opencode_env(s, &mut inv, true);
            if let Some(m) = model(s) {
                inv.args.extend(["--model".to_string(), m]);
            }
            otherwise_link::fsutil::write_atomic(&s.dir.join(AGENTS_FILE), instructions.as_bytes())?;
        }
    }
    Ok(inv)
}

/// Instructions for terminal mode: the system prompt plus what the user is looking at.
pub fn terminal_instructions(system: &str, context: Option<&str>) -> String {
    let mut p = system.trim().to_string();
    if let Some(c) = context.map(str::trim).filter(|c| !c.is_empty()) {
        p.push_str("\n\nWhat the user was looking at in the app when this session started:\n");
        p.push_str(c);
    }
    p.push('\n');
    p
}
