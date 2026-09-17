//! 系统提示的装配，与窗口占用的核算。
//!
//! [`assemble`] 跑 `system-prompt/assemble` waterfall；[`context_book`] 是各插件
//! 登记提示段落的名册；[`listing`] 给目录类段落算预算；[`context_usage`] 反过来
//! 数这些段落各占多少窗口。[`project_instructions`] 是个例外——`AGENTS.md` 走
//! 历史尾部的 `<system-reminder>`，**不进系统提示**，放这里是因为它和提示装配
//! 共用预算逻辑。

pub mod assemble;
pub mod context_book;
pub mod context_usage;
pub mod listing;
pub mod project_instructions;
