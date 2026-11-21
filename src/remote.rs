//! Providers the host app supplies itself — its own AI service over its own authenticated API
//! (for example "Vcity AI"). They appear in the agent picker next to the CLIs, answer in the
//! same chat (no terminal mode), get the same attachments and the same permission modes (what a
//! mode means is up to the host: typically which of its tools the service may call).
//!
//! The panel calls [`RemoteProvider::send`] on a background thread for every turn and renders
//! the [`StreamEvent`]s it emits; it checks [`Cancel`] when the user presses stop.
use crate::attach::Attachment;
use crate::permission::PermissionMode;
use crate::provider::Model;
use crate::stream::{AgentError, StreamEvent};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set when the user stops a run. Cheap to clone; check it between network reads.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Everything a remote turn needs. Owned, so it can move to the background thread.
#[derive(Debug, Clone)]
pub struct RemoteTurn {
    /// The panel's conversation id: the same for every turn of a conversation, so the service
    /// can keep its own thread (create it on the first turn it sees).
    pub conversation_id: String,
    /// The model id from [`RemoteProvider::models`] (`None`: Default).
    pub model: Option<String>,
    pub mode: PermissionMode,
    /// The panel's system prompt (English) and the host's "what I'm looking at" context.
    pub system: String,
    pub context: Option<String>,
    /// Earlier `(from the user, text)` pairs, for services that don't keep history.
    pub history: Vec<(bool, String)>,
    pub text: String,
    /// Copies inside the session folder; upload them as the service expects.
    pub attachments: Vec<Attachment>,
    /// The user's language (`es`, `en`, …).
    pub language: String,
}

pub trait RemoteProvider: Send + Sync + 'static {
    /// Stable id (`vcity`): saved in preferences and conversations as `remote:<id>`.
    fn id(&self) -> String;
    /// Shown in the picker ("Vcity AI").
    fn label(&self) -> String;
    /// Who receives the data, for the privacy note ("Vcity").
    fn vendor(&self) -> String;
    /// The model picker's entries; the first is the default.
    fn models(&self) -> Vec<Model> {
        vec![Model::default_model()]
    }
    /// `Err(message)` when the provider can't be used now (signed out, offline): shown instead
    /// of the composer's send button.
    fn available(&self) -> Result<(), String> {
        Ok(())
    }
    /// Run one turn to the end, emitting events as they arrive; returns the full answer text.
    /// Called on a background thread. Return `Err(AgentError::Cancelled)` once `cancel` is set.
    fn send(&self, turn: RemoteTurn, emit: &mut dyn FnMut(StreamEvent), cancel: &Cancel) -> Result<String, AgentError>;
}

/// A scripted provider for tests and the demo: answers with the question echoed, a tool call,
/// and a pending change when asked to "change" something in mode Ask.
#[derive(Debug, Default)]
pub struct FakeRemote {
    pub delay_ms: u64,
}

impl RemoteProvider for FakeRemote {
    fn id(&self) -> String {
        "fake".into()
    }
    fn label(&self) -> String {
        "Fake AI".into()
    }
    fn vendor(&self) -> String {
        "Example Inc.".into()
    }
    fn models(&self) -> Vec<Model> {
        vec![Model::default_model(), Model::new("fake-large", "Fake Large"), Model::new("fake-small", "Fake Small")]
    }
    fn send(&self, turn: RemoteTurn, emit: &mut dyn FnMut(StreamEvent), cancel: &Cancel) -> Result<String, AgentError> {
        let tool = crate::stream::ToolRef::new(Some("demo"), "items_list");
        emit(StreamEvent::ToolCall(tool.clone()));
        emit(StreamEvent::ToolResult { tool, is_error: false });
        let mut answer = String::new();
        let names: Vec<&str> = turn.attachments.iter().map(|a| a.name.as_str()).collect();
        let text = format!(
            "You asked: **{}** (model {}, {} attachment(s){}).",
            turn.text.trim(),
            turn.model.as_deref().unwrap_or("default"),
            names.len(),
            if names.is_empty() { String::new() } else { format!(": {}", names.join(", ")) }
        );
        for w in text.split_inclusive(' ') {
            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }
            if self.delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(self.delay_ms));
            }
            answer.push_str(w);
            emit(StreamEvent::Delta(w.to_string()));
        }
        Ok(answer)
    }
}
