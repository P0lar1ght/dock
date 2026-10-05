//! DSH ctx keys for the TUI plugins.
//!
//! 宿主之间共用的名字（`session` `session.port` `tabs` `gateway`）在
//! `cordis_spine::names`。

pub const THEME: &str = "theme";
pub const TUI_SCROLLBACK: &str = "tui.scrollback";
pub const TUI_PROMPT: &str = "tui.prompt";
pub const TUI_STATUS: &str = "tui.statusBar";
pub const TUI_WELCOME: &str = "tui.welcome";
pub const TUI_SHORTCUTS: &str = "tui.shortcuts";
pub const TUI_PAIRING: &str = "tui.pairing";
pub const TUI: &str = "tui";

pub(crate) use cordis_spine::{GATEWAY, GATEWAY_PAIRING, SESSION, SESSION_PORT, TABS};
