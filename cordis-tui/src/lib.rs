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
    GATEWAY, GATEWAY_PAIRING, SESSION, SESSION_PORT, THEME, TUI, TUI_PAIRING, TUI_PROMPT,
    TUI_SCROLLBACK, TUI_SHORTCUTS, TUI_STATUS, TUI_TABS, TUI_WELCOME,
};
pub use plugin::{pairing, prompt, scrollback, shortcuts, status_bar, theme, tui, welcome};
pub use seam::gateway::{
    CompanionStatus, GatewayPort, GatewayRef, PairingBinding, PairingError, PairingPrompt,
};
pub use seam::session::{QueuedItem, SessionPort, SessionRef};
pub use seam::tabs::{tabs, TabInfo, TabKind, TabMount, Tabs, MAX_TABS, PER_TAB_SERVICES};
pub use slash::{resolve_slash, slash_catalog, SlashCatalogEntry};
pub use views::prompt::PromptWidget;
