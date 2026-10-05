//! 宿主（TUI / gateway / 动态插件）live-look 的那几张表。
//!
//! 共性是**谁在用**：这些都不参与 agent 循环，而是让人或宿主界面去读去改。
//! [`settings`] 是设置浮层改的东西，[`permissions`] 是权限浮层解的队列，
//! [`slash`] 与 [`tui_slots`] 是动态插件往界面上加东西的登记处。
//!
//! [`session_port`]、[`tabs`]、[`gateway_port`] 是宿主之间的契约：TUI、网关、
//! 定时任务驱动都 live-lookup 它们，谁也不该为此依赖另一个宿主的 crate——无头的
//! `dock serve` 不链接终端 UI。

pub mod gateway_port;
pub mod permissions;
pub mod session_port;
pub mod settings;
pub mod settings_commands;
pub mod slash;
pub mod status_items;
pub mod tabs;
pub mod tool_views;
pub mod tui_slots;
