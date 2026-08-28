//! Grok-shaped pager as Cordis plugins.
//!
//! Swap any of `theme` / `tui.scrollback` / `tui.prompt` / `tui.statusBar` /
//! `tui.welcome` / `tui` without touching the session actor. The event loop
//! live-looks-up `session.port` at the send site.

mod actions;
mod dispatch;
mod error;
mod event_loop;
mod grok;
mod input;
mod names;
mod overlay;
mod plugin;
mod prompt;
mod scrollback;
mod session;
mod slash;
mod status;
mod status_bar;
mod theme;
mod welcome;

pub use actions::{Action, Effect};
pub use dispatch::dispatch;
pub use names::{
    SESSION, SESSION_PORT, THEME, TUI, TUI_PROMPT, TUI_SCROLLBACK, TUI_STATUS, TUI_WELCOME,
};
pub use plugin::{prompt, scrollback, status_bar, theme, tui, welcome};
pub use prompt::PromptWidget;
pub use session::{SessionPort, SessionRef};
