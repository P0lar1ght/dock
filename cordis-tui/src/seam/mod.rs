//! TUI live-look 的 named service 座。
//!
//! 这几个都不是 TUI 自己的状态：插件树上别处 `provide`，TUI 在**调用点**
//! `ctx.get` / `ctx.require` 拿（`"session.port"` `"gateway"`
//! `"tui.shortcuts"` `"tui.tabs"`）。放一起是为了让「这些是座、不是实现」
//! 这件事在目录上看得见——不要把 `Arc<T>` 关进长生命周期闭包。

pub mod gateway;
pub mod session;
pub mod shortcuts;
pub mod tabs;
