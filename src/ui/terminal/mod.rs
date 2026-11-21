//! The embedded terminal for Terminal mode: alacritty's grid and parser on a PTY (`model`), a
//! GPUI view that types, pastes, drops files and paints the grid (`view`).
mod colors;
pub mod keys;
pub mod model;
pub mod view;

pub use model::Spawn;
pub use view::{TermEvent, TermStyle, TerminalView};
