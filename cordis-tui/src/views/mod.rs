//! 所有画出来的东西：overlay、模态、面板、控件。
//!
//! 一个文件一块屏上的东西；状态与派发在 [`crate::app`]。对应 Grok pager 的
//! `views/`。

pub mod ask_view;
pub mod dashboard;
pub mod goal_overlay;
pub mod goal_pane;
pub mod inspect_overlay;
pub mod mcp_elicit_view;
pub mod memory_browser;
pub mod overlay;
pub mod pairing;
pub mod permission_view;
pub mod plan_approval_view;
pub mod preset_overlay;
pub mod prompt;
pub mod queue_pane;
pub mod settings_modal;
pub mod status;
pub mod status_bar;
pub mod tab_bar;
pub mod task_dock;
pub mod text_overlay;
pub mod usage_overlay;
pub mod welcome;
