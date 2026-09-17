//! 宿主（TUI / gateway / 动态插件）live-look 的那几张表。
//!
//! 共性是**谁在用**：这四个都不参与 agent 循环，而是让人或宿主界面去读去改。
//! [`settings`] 是设置浮层改的东西，[`permissions`] 是权限浮层解的队列，
//! [`slash`] 与 [`tui_slots`] 是动态插件往界面上加东西的登记处。

pub mod permissions;
pub mod settings;
pub mod slash;
pub mod tui_slots;
