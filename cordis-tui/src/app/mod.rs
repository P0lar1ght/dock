//! 状态与派发：按键怎么变成 Action、Action 怎么变成 Effect、循环怎么跑一帧。
//!
//! 这一层不画东西（画在 [`crate::views`]），也不持有 named service
//! （那些在 [`crate::seam`]）。对应 Grok pager 的 `app/`。

pub mod actions;
pub mod clipboard;
pub mod dispatch;
pub mod event_loop;
pub mod input;
pub mod mode_cycle;
