//! Conversations and the panel's own preferences, kept with the rest of the user's Otherwise
//! data (the data root from otherwise-link, so they move with it):
//!
//! ```text
//! <data root>/<app>/assistant/<conversation>.json          one file per conversation
//! <data root>/<app>/assistant/sessions/<conversation>/     the session folder: the agent's
//!     attachments/…                                         working directory, attachments,
//!     mcp-<n>.json                                          per-session MCP config (removed after)
//! <data root>/<app>/assistant/.panel.json                  dock mode, sizes, provider, model…
//! ```
//!
//! Files are written atomically; unreadable or newer-schema files are skipped. Conversations
//! written by Otherwise Apps 0.3 (same folder, schema 1) load as they are.
use crate::attach::Attachment;
use crate::permission::PermissionMode;
use crate::provider::ProviderId;
use crate::stream::ToolRef;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SCHEMA: u32 = 1;

/// `<root>/<app>/assistant`.
pub fn dir_in(root: &Path, app: &str) -> PathBuf {
    root.join(app).join("assistant")
}

/// The assistant folder in the current data root.
pub fn dir(app: &str) -> PathBuf {
    dir_in(&otherwise_link::paths::data_root(), app)
}

/// Ids become file and folder names: plain ASCII only.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// `<assistant dir>/sessions/<id>` (not created here).
pub fn session_dir(assistant_dir: &Path, id: &str) -> PathBuf {
    assistant_dir.join("sessions").join(if safe_id(id) { id } else { "invalid" })
}

pub fn attachments_dir(session: &Path) -> PathBuf {
    session.join("attachments")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolUse {
    #[serde(flatten)]
    pub tool: ToolRef,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeState {
    Pending,
    Applied,
    Rejected,
    Failed,
}

/// A change the host's tools proposed (permission mode "ask"), shown as an Aplicar / Rechazar
/// card. `id` is the host's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingChange {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub state: ChangeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PendingChange {
    pub fn new(id: impl Into<String>, title: impl Into<String>, detail: impl Into<String>) -> PendingChange {
        PendingChange { id: id.into(), title: title.into(), detail: detail.into(), state: ChangeState::Pending, error: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub user: bool,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolUse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<PendingChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Message {
    pub fn user(text: impl Into<String>, attachments: Vec<Attachment>) -> Message {
        Message { user: true, text: text.into(), tools: vec![], attachments, changes: vec![], error: None }
    }

    pub fn assistant(text: impl Into<String>) -> Message {
        Message { user: false, text: text.into(), tools: vec![], attachments: vec![], changes: vec![], error: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub schema: u32,
    pub id: String,
    pub title: String,
    /// The provider of the last answer.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "backend")]
    pub provider: Option<ProviderId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Unix seconds.
    pub created: i64,
    pub updated: i64,
    pub messages: Vec<Message>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Conversation {
    pub fn new() -> Conversation {
        let t = now();
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        Conversation { schema: SCHEMA, id: format!("{t}-{nanos:08x}"), title: String::new(), provider: None, model: None, created: t, updated: t, messages: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn push(&mut self, m: Message) {
        if self.title.is_empty() && m.user {
            self.title = title_of(if m.text.trim().is_empty() { m.attachments.first().map(|a| a.name.as_str()).unwrap_or("") } else { &m.text });
        }
        self.messages.push(m);
        self.updated = now();
    }

    /// `(from the user, text)` pairs for the prompt; failed turns are left out.
    pub fn pairs(&self) -> Vec<(bool, String)> {
        self.messages.iter().filter(|m| m.error.is_none() && !m.text.trim().is_empty()).map(|m| (m.user, m.text.clone())).collect()
    }

    /// The card of a reported change, wherever it is.
    pub fn change_mut(&mut self, id: &str) -> Option<&mut PendingChange> {
        self.messages.iter_mut().rev().flat_map(|m| m.changes.iter_mut()).find(|c| c.id == id)
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        if !safe_id(&self.id) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad conversation id"));
        }
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        otherwise_link::fsutil::write_atomic(&dir.join(format!("{}.json", self.id)), (json + "\n").as_bytes())
    }

    /// Removes the conversation file and its session folder.
    pub fn delete(dir: &Path, id: &str) -> std::io::Result<()> {
        if !safe_id(id) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad conversation id"));
        }
        let f = dir.join(format!("{id}.json"));
        if f.exists() {
            std::fs::remove_file(f)?;
        }
        let s = session_dir(dir, id);
        if s.exists() {
            std::fs::remove_dir_all(s)?;
        }
        Ok(())
    }
}

impl Default for Conversation {
    fn default() -> Self {
        Conversation::new()
    }
}

/// The first line of the question, at most 60 characters.
pub fn title_of(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    let mut t: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        t.push('…');
    }
    t
}

/// Every saved conversation, the most recent first.
pub fn list(dir: &Path) -> Vec<Conversation> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Conversation> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json") && !p.file_name().and_then(|n| n.to_str()).unwrap_or(".").starts_with('.'))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|s| serde_json::from_str::<Conversation>(&s).ok())
        .filter(|c| c.schema <= SCHEMA && safe_id(&c.id))
        .collect();
    v.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
    v
}

/// Where the panel sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockMode {
    Hidden,
    /// A one-line bar along the bottom of the window.
    Bar,
    Bottom,
    #[default]
    Right,
    /// The panel takes the whole window.
    Focus,
}

pub const DOCK_MODES: [DockMode; 4] = [DockMode::Bar, DockMode::Bottom, DockMode::Right, DockMode::Focus];

impl DockMode {
    pub fn key(self) -> &'static str {
        match self {
            DockMode::Hidden => "hidden",
            DockMode::Bar => "bar",
            DockMode::Bottom => "bottom",
            DockMode::Right => "right",
            DockMode::Focus => "focus",
        }
    }
}

/// Chat (structured, headless runs) or Terminal (the CLI itself in a PTY).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    Chat,
    Terminal,
}

pub const MIN_WIDTH: f32 = 300.;
pub const MIN_HEIGHT: f32 = 160.;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub dock: DockMode,
    /// The last visible mode, for ⌘J.
    pub last_visible: DockMode,
    pub width: f32,
    pub height: f32,
    pub view: ViewMode,
    pub provider: Option<ProviderId>,
    /// Model id per provider key (absent: Default).
    pub models: std::collections::BTreeMap<String, String>,
    pub permission: PermissionMode,
    pub ollama: OllamaPrefs,
}

/// The built-in Ollama provider's settings. Its default model is `models["ollama"]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OllamaPrefs {
    /// `None`: `http://localhost:11434`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Leave Ollama out of the picker (and don't look for it).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            dock: DockMode::Hidden,
            last_visible: DockMode::Right,
            width: 420.,
            height: 320.,
            view: ViewMode::Chat,
            provider: None,
            models: Default::default(),
            permission: PermissionMode::default(),
            ollama: OllamaPrefs::default(),
        }
    }
}

impl Prefs {
    pub const FILE: &'static str = ".panel.json";

    pub fn load(dir: &Path) -> Prefs {
        let mut p: Prefs = std::fs::read_to_string(dir.join(Self::FILE)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        p.width = clamp_or(p.width, MIN_WIDTH, 4000., 420.);
        p.height = clamp_or(p.height, MIN_HEIGHT, 4000., 320.);
        if matches!(p.last_visible, DockMode::Hidden) {
            p.last_visible = DockMode::Right;
        }
        p
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        otherwise_link::fsutil::write_atomic(&dir.join(Self::FILE), (json + "\n").as_bytes())
    }

    pub fn model_for(&self, p: &ProviderId) -> Option<String> {
        self.models.get(&p.key()).cloned()
    }

    /// ⌘J: hidden ↔ the last visible mode.
    pub fn toggle(&mut self) {
        if self.dock == DockMode::Hidden {
            self.dock = self.last_visible;
        } else {
            self.last_visible = self.dock;
            self.dock = DockMode::Hidden;
        }
    }

    pub fn set_dock(&mut self, m: DockMode) {
        if m != DockMode::Hidden {
            self.last_visible = m;
        }
        self.dock = m;
    }
}

fn clamp_or(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}
