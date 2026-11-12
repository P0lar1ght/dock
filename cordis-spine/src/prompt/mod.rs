//! 系统提示的装配，与窗口占用的核算。
//!
//! [`assemble`] 跑 `system-prompt/assemble` waterfall；[`context_book`] 是各插件
//! 登记提示段落的名册；[`listing`] 给技能 / 工作流目录算预算；[`context_usage`]
//! 反过来数各段与目录各占多少窗口。[`project_instructions`] 是个例外——`AGENTS.md`
//! 走历史里的 `<system-reminder>`，**不进系统提示**；技能 / 工作流目录与子代理
//! 名册同样走 reminder，只是渲染仍复用这里的预算逻辑。

pub mod assemble;
pub mod context_book;
pub mod context_usage;
pub mod listing;
pub mod project_instructions;
