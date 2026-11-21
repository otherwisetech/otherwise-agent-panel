//! The GPUI side: [`AgentDock`] (the host's main view with the panel docked beside, below, as a
//! bar or full window), [`AgentPanel`] (header, pickers, Chat and Terminal), [`AgentHost`].
//!
//! Call [`init`] once after `gpui_component::init`, then make an `AgentDock` the window's root
//! view (inside gpui-component's `Root`) with your main view in it.
mod chat;
pub mod dock;
pub mod host;
pub mod panel;
pub mod style;
pub mod terminal;
mod widgets;

pub use dock::AgentDock;
pub use host::{AgentHost, DropAction, DropOutcome};
pub use panel::{AgentPanel, PanelEvent};
pub use style::PanelStyle;

gpui::actions!(otherwise_agent_panel, [TogglePanel, NewConversation, ClosePanelMenu]);

/// ⌘J (Ctrl-J elsewhere) shows / hides the panel from anywhere in the window.
pub fn init(cx: &mut gpui::App) {
    cx.bind_keys([gpui::KeyBinding::new("secondary-j", TogglePanel, None), gpui::KeyBinding::new("escape", ClosePanelMenu, Some(panel::KEY_CONTEXT))]);
}
