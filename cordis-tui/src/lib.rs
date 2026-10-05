//! Grok-shaped pager as Cordis plugins.
//!
//! Swap any of `theme` / `tui.scrollback` / `tui.prompt` / `tui.statusBar` /
//! `tui.welcome` / `tui.shortcuts` / `tui` without touching the session actor.

mod app;
mod error;
mod file_search;
mod grok;
mod media;
mod names;
mod plugin;
mod scrollback;
mod seam;
mod slash;
mod theme;
mod views;

pub use app::actions::{Action, Effect, PromptSend};
pub use app::dispatch::dispatch;
pub use names::{
    THEME, TUI, TUI_PAIRING, TUI_PROMPT, TUI_SCROLLBACK, TUI_SHORTCUTS, TUI_STATUS, TUI_WELCOME,
};
pub use plugin::{pairing, prompt, scrollback, shortcuts, status_bar, theme, tui, views, welcome};
pub use seam::tabs::{carry_back, PER_TAB_VIEWS};
pub use views::prompt::PromptWidget;
