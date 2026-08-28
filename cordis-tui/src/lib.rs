//! Grok-shaped pager as Cordis plugins.
//!
//! Swap any of `theme` / `tui.scrollback` / `tui.prompt` / `tui.statusBar` /
//! `tui.welcome` / `tui.shortcuts` / `tui` without touching the session actor.

mod ask_view;
mod actions;
mod clipboard;
mod image_meta;
mod dispatch;
mod error;
mod event_loop;
mod file_search;
mod grok;
mod input;
mod mermaid_png;
mod names;
mod overlay;
mod permission_view;
mod plugin;
mod prompt;
mod scrollback;
mod session;
mod settings_modal;
mod shortcuts;
mod slash;
mod status;
mod status_bar;
mod theme;
mod welcome;

pub use actions::{Action, Effect};
pub use dispatch::dispatch;
pub use names::{
    SESSION, SESSION_PORT, THEME, TUI, TUI_PROMPT, TUI_SCROLLBACK, TUI_SHORTCUTS, TUI_STATUS,
    TUI_WELCOME,
};
pub use plugin::{prompt, scrollback, shortcuts, status_bar, theme, tui, welcome};
pub use prompt::PromptWidget;
pub use session::{SessionPort, SessionRef};
