//! 宿主（TUI / gateway / 动态插件）live-look 的那几张表。
//!
//! 共性是**谁在用**：这些都不参与 agent 循环，而是让人或宿主界面去读去改。
//! [`settings`] 是设置浮层改的东西，[`permissions`] 是权限浮层解的队列，
//! [`slash`] 与 [`tui_slots`] 是动态插件往界面上加东西的登记处。
//! [`user_edits`] 记用户在文件面板里的改动，下一步提醒 agent；[`mcp_app_context`] 同理，
//! 记 MCP App 界面交来的状态；[`mcp_app_views`] 记界面自己的工具，界面开着时交给模型。
//!
//! [`session_port`]、[`tabs`]、[`gateway_port`] 是宿主之间的契约：TUI、网关、
//! 定时任务驱动都 live-lookup 它们，谁也不该为此依赖另一个宿主的 crate——无头的
//! `dock serve` 不链接终端 UI。

pub mod gateway_port;
pub mod mcp_app_context;
pub mod mcp_app_views;
pub mod permissions;
pub mod plugin_settings;
pub mod session_port;
pub mod settings;
pub mod settings_commands;
pub mod side_note;
pub mod slash;
pub mod status_items;
pub mod tabs;
pub mod tool_views;
pub mod tui_slots;
pub mod user_edits;
