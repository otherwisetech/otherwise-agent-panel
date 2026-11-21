//! What a host app tells the panel. Implement [`AgentHost`] on a small struct that holds a
//! `WeakEntity` of your main view (the panel calls it outside your view's updates: entry points
//! like `AgentPanel::ask` defer their work, so reading your entity inside these methods is safe).
use crate::mcp::McpServer;
use crate::permission::PermissionMode;
use crate::remote::RemoteProvider;
use gpui::{App, Window};
use std::path::PathBuf;
use std::sync::Arc;

/// An app-specific thing to do with dropped files, offered before "attach to the conversation"
/// (expense: "Importar como recibo", "Preguntar sobre esto").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropAction {
    pub id: String,
    pub label: String,
}

impl DropAction {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> DropAction {
        DropAction { id: id.into(), label: label.into() }
    }
}

/// What happened after a drop action ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropOutcome {
    /// Done by the app; the optional text is shown as a short notice in the panel.
    Done(Option<String>),
    /// Attach the files to the conversation (nothing else).
    Attach,
    /// Attach the files and put this question in the composer (`send: true` sends it now).
    Ask { prompt: String, send: bool },
    /// It failed; shown as an error notice.
    Failed(String),
}

pub trait AgentHost: 'static {
    /// The app id under the data root (`expense`, `otherwise-apps`): history goes to
    /// `<data root>/<app id>/assistant/`.
    fn app_id(&self) -> String;

    /// The app's name for the panel title and the agent's instructions.
    fn app_name(&self) -> String;

    /// The MCP servers the agent gets for a session in `mode` (see [`PermissionMode`] for what
    /// each mode should expose: read tools only, proposals, or writes). Usually one: the app's
    /// own `<cli> mcp`, or a small bridge to the running app's socket (see INTEGRATING.md).
    fn mcp_servers(&self, mode: PermissionMode, cx: &App) -> Vec<McpServer>;

    /// Also attach the Otherwise hub (every installed Otherwise app's tools) when it is found.
    fn include_hub(&self) -> bool {
        false
    }

    /// "Lo que estoy viendo": a short text or JSON about the current screen / selection, added
    /// to every chat turn (and to a terminal session when it starts). Keep it small.
    fn context(&self, _cx: &App) -> Option<String> {
        None
    }

    /// Extra instructions for the agent (English), appended to the panel's system prompt.
    fn instructions(&self) -> Option<String> {
        None
    }

    /// Example questions shown on an empty conversation and in the `/` menu.
    fn suggestions(&self) -> Vec<String> {
        Vec::new()
    }

    /// Providers the app runs itself (its own AI service), listed after the CLIs.
    fn remote_providers(&self) -> Vec<Arc<dyn RemoteProvider>> {
        Vec::new()
    }

    /// App-specific actions for files dropped or pasted on the panel; empty: just attach.
    fn drop_actions(&self, _files: &[PathBuf], _cx: &App) -> Vec<DropAction> {
        Vec::new()
    }

    fn run_drop_action(&self, _action: &str, _files: &[PathBuf], _window: &mut Window, _cx: &mut App) -> DropOutcome {
        DropOutcome::Attach
    }

    /// The user pressed Aplicar (`approve`) or Rechazar on a change the app reported with
    /// `AgentPanel::report_pending_change`. `Err` is shown on the card.
    fn resolve_change(&self, _id: &str, _approve: bool, _window: &mut Window, _cx: &mut App) -> Result<(), String> {
        Err(crate::t!("panel.change.unsupported"))
    }
}
