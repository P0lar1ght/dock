//! Dock 浏览器（BUA）的 MCP 服务：`dock mcp browser`。
//!
//! 以前是 spine 里的进程内工具 `tool-browser`；现在拆成独立的 stdio MCP 服务，
//! Dock 按内置 `[mcp_servers.browser]` 自动接上（见 `cordis_base::config`）。
//!
//! - [`session`]：chromiumoxide CDP。一个共享的 [`Chromium`]（共用 profile，
//!   登录态共享），每个会话一组标签页（[`ConnectedSession`]）。
//! - [`hub`]：按会话 id 管标签页组，把 `browser_*` 调用分派到对应的组。
//! - [`tools`]：工具清单（MCP `tools/list` 的形状）与参数解析。
//! - [`server`]：NDJSON JSON-RPC over stdio，`server/discover` 与 `initialize` 两代握手都认。
//!
//! 会话身份由 Dock 在 `tools/call` 的 `_meta["dock/sessionId"]` 里带；没带的
//! （别的 MCP 客户端）归到同一个默认组。

pub mod hub;
pub mod server;
pub mod session;
mod snapshot;
pub mod tools;
mod wait;

pub use hub::{BrowserHub, CallOutput};
pub use server::{serve, serve_stdio, SERVER_NAME};
pub use session::{
    browser_screenshot_dir, browser_user_data_dir, devtools_ws_url, Chromium, ConnectedSession,
};

/// 一个标签页的摘要（`browser_tabs` 与状态行用）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabInfo {
    pub index: usize,
    pub url: String,
    pub active: bool,
}
