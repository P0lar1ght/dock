//! TUI live-look 的 named service 座。
//!
//! 这几个都不是 TUI 自己的状态：插件树上别处 `provide`，TUI 在**调用点**
//! `ctx.get` / `ctx.require` 拿。会话提交口、分页、网关这些宿主之间共用的契约
//! 住在 spine（`cordis_spine::{SessionRef, Tabs, GatewayRef}`）；这里只剩 TUI
//! 自己的快捷键表，和分页落到终端上的那一点（每页视图名、带回输入框）。

pub mod shortcuts;
pub mod tabs;
