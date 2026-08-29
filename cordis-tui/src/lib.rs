//! Grok-shaped pager as Cordis plugins.
//!
//! Swap any of `theme` / `tui.scrollback` / `tui.prompt` / `tui.statusBar` /
//! `tui.welcome` / `tui.shortcuts` / `tui` without touching the session actor.

mod actions;
mod ask_view;
mod clipboard;
mod dispatch;
mod error;
mod event_loop;
mod file_search;
mod goal_overlay;
mod goal_pane;
mod grok;
mod image_meta;
mod input;
mod inspect_overlay;
mod mermaid_png;
mod mode_cycle;
mod names;
mod overlay;
mod permission_view;
mod plan_approval_view;
mod plugin;
mod preset_overlay;
mod prompt;
mod queue_pane;
mod scrollback;
mod session;
mod settings_modal;
mod shortcuts;
mod slash;
mod status;
mod status_bar;
mod subagent_dock;
mod text_overlay;
mod theme;
mod usage_overlay;
mod welcome;

pub use actions::{Action, Effect};
pub use dispatch::dispatch;
pub use names::{
    SESSION, SESSION_PORT, THEME, TUI, TUI_PROMPT, TUI_SCROLLBACK, TUI_SHORTCUTS, TUI_STATUS,
    TUI_WELCOME,
};
pub use plugin::{prompt, scrollback, shortcuts, status_bar, theme, tui, welcome};
pub use prompt::PromptWidget;
pub use session::{QueuedItem, SessionPort, SessionRef};
