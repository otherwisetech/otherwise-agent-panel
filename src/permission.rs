//! The three permission modes of the picker and what each means for the MCP servers and for
//! each CLI's own flags.
//!
//! | mode | host's MCP server | Claude Code | Codex | opencode |
//! |---|---|---|---|---|
//! | Solo lectura | read tools only ([`McpWrites::Off`]) | `--permission-mode dontAsk` | `--sandbox read-only`, `approval_policy="never"` | edit / bash / webfetch `deny` |
//! | Preguntar antes de cambiar | writes become pending changes ([`McpWrites::Propose`]) the user applies or rejects in the panel | terminal: CLI's default (asks before each tool); chat: `dontAsk` | `read-only`, terminal `on-request` / chat `never` | terminal `ask`, chat `deny` |
//! | Automático | write tools on ([`McpWrites::On`]) | `--permission-mode acceptEdits` | `read-only`, `never` | `deny` |
//!
//! The agent's own file / shell / web tools stay off in every mode (only `Read`, inside the
//! session folder, for attachments): the user's data reaches it only through the host's tools.
//! A headless run can't ask, so in chat mode "ask" is the host's pending-change cards.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    ReadOnly,
    #[default]
    Ask,
    Auto,
}

pub const MODES: [PermissionMode; 3] = [PermissionMode::ReadOnly, PermissionMode::Ask, PermissionMode::Auto];

/// What the host's MCP server should offer for a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpWrites {
    /// Read tools only.
    Off,
    /// Write tools exist but only *propose* a change; the host reports it to the panel
    /// (`AgentPanel::report_pending_change`) and applies it when the user says so.
    Propose,
    /// Write tools apply directly (each app still records what changed).
    On,
}

impl PermissionMode {
    pub fn writes(self) -> McpWrites {
        match self {
            PermissionMode::ReadOnly => McpWrites::Off,
            PermissionMode::Ask => McpWrites::Propose,
            PermissionMode::Auto => McpWrites::On,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            PermissionMode::ReadOnly => "read_only",
            PermissionMode::Ask => "ask",
            PermissionMode::Auto => "auto",
        }
    }

    pub fn label(self) -> String {
        crate::i18n::tr(&format!("panel.perm.{}", self.key()), &[])
    }

    pub fn hint(self) -> String {
        crate::i18n::tr(&format!("panel.perm.{}_hint", self.key()), &[])
    }

    /// Claude Code's `--permission-mode` (`None`: leave its default, which asks).
    pub fn claude_flag(self, interactive: bool) -> Option<&'static str> {
        match (self, interactive) {
            (PermissionMode::ReadOnly, _) => Some("dontAsk"),
            (PermissionMode::Ask, true) => None,
            (PermissionMode::Ask, false) => Some("dontAsk"),
            (PermissionMode::Auto, _) => Some("acceptEdits"),
        }
    }

    /// Codex's `approval_policy` override.
    pub fn codex_approval(self, interactive: bool) -> &'static str {
        match (self, interactive) {
            (PermissionMode::Ask, true) => "on-request",
            _ => "never",
        }
    }

    /// opencode's permission value for edit / bash / webfetch.
    pub fn opencode_permission(self, interactive: bool) -> &'static str {
        match (self, interactive) {
            (PermissionMode::Ask, true) => "ask",
            _ => "deny",
        }
    }
}
