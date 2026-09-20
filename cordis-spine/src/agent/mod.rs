//! Agent 循环与一轮的生命周期。
//!
//! 「出了文本但还不该收尾」「跑到一半要提醒」这类扩展走 waterfall
//! （`agent/pre-step`、`agent/step-start`、`agent/turn-end`），不在这里加 `if`。

pub mod agents;
pub mod capability;
pub mod loop_plugin;
pub mod presets;
pub mod runtime;
pub mod turn;
