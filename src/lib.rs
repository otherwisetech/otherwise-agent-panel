//! # otherwise-agent-panel
//!
//! The AI panel every Otherwise desktop app shares: the user's own Claude Code, Codex,
//! opencode or local [`ollama`] models (or the host app's own AI service, [`remote`]) docked
//! next to the app, with the
//! app's data attached through MCP **for that session only** — the user's agent configuration
//! files are never touched.
//!
//! - Non-UI core (always built): [`provider`] detection and model lists, [`permission`] modes,
//!   per-session [`mcp`] configs, [`args`] for chat and terminal runs, [`stream`] parsers,
//!   [`run`], [`attach`]ments, [`history`] and preferences, [`i18n`], the built-in [`ollama`]
//!   provider with its own [`mcp_client`].
//! - `ui` feature (default): [`ui::AgentDock`] (the host's main view plus the panel in Barra /
//!   Abajo / Derecha / Foco), [`ui::AgentPanel`] (pickers, Chat and Terminal modes), and the
//!   [`ui::AgentHost`] trait the app implements. See docs/INTEGRATING.md.
pub mod args;
pub mod attach;
pub mod history;
pub mod http;
pub mod i18n;
pub mod mcp;
pub mod mcp_client;
pub mod ollama;
pub mod permission;
pub mod provider;
pub mod remote;
pub mod run;
pub mod stream;
#[cfg(feature = "ui")]
pub mod ui;

pub use attach::Attachment;
pub use history::{Conversation, DockMode, Message, PendingChange, Prefs, ViewMode};
pub use mcp::McpServer;
pub use permission::{McpWrites, PermissionMode};
pub use provider::{Cli, Model, ProviderId};
pub use remote::{Cancel, RemoteProvider, RemoteTurn};
pub use stream::{AgentError, StreamEvent, ToolRef};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
