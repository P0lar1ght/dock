//! DSH ctx keys for the TUI plugins.

pub const SESSION: &str = "session";
pub const SESSION_PORT: &str = "session.port";
pub const THEME: &str = "theme";
pub const TUI_SCROLLBACK: &str = "tui.scrollback";
pub const TUI_PROMPT: &str = "tui.prompt";
pub const TUI_STATUS: &str = "tui.statusBar";
pub const TUI_WELCOME: &str = "tui.welcome";
pub const TUI_SHORTCUTS: &str = "tui.shortcuts";
pub const TUI_PAIRING: &str = "tui.pairing";
pub const TUI: &str = "tui";
/// Named service provided by the `gateway` plugin.
pub const GATEWAY: &str = "gateway";
/// Pairing queue / binding changes. Payload is `()`.
pub const GATEWAY_PAIRING: &str = "gateway/pairing";
