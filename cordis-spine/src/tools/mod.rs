//! 工具注册表与全部工具插件。
//!
//! 对应 Grok 的 `xai-grok-tools`：那边同样把 `registry/` 和 `implementations/`
//! 放在**一个** crate 里。注册表要问预设的允许名单、要查计划门与权限门，工具又
//! 要往注册表里 register——这圈依赖是工具表这件事的固有形态，不是 dock 特有的
//! 耦合，所以不拆。
//!
//! [`registry`] 是那张唯一的 `"tools"` 表。其余每个模块是一颗（或一套）能力
//! 插件：`inject: ["tools"]` 之后 `ctx.tools.register()`。**粒度是套件**——
//! `web_fetch` 一颗插件同时挂 `web_fetch` 与 `web_search`。

pub mod ask_user;
pub mod browser;
pub mod capability;
pub mod computer;
pub mod cron;
pub mod dynamic_runner;
pub mod fs_perms;
pub mod goal;
pub mod jobs;
pub mod lsp;
pub mod mcp;
pub mod memory;
pub mod monitor;
pub mod plan_mode;
pub mod registry;
pub mod sched;
pub mod skills;
pub mod task;
pub mod todo_write;
pub mod tool_cordis;
pub mod tool_images;
pub mod web_fetch;
pub mod workflow;
pub mod workspace;
