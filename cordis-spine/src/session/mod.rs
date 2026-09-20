//! 会话日志、落盘与跨 cwd 名册。
//!
//! [`log`] 是内存里的事件流（每页一份）；[`persist`] 是它在
//! `$DOCK_HOME/sessions/<cwd-key>/<id>/` 的磁盘形态；[`roster`] 是**跨 cwd**
//! 的抬头名册（只读 `meta.json` 加 jsonl 尾巴，不解整份 transcript）。
//! [`resume_preset`] 是所有 resume 路径共用的 preset 回放；[`search`] 是
//! 跨会话 FTS（title + 用户提示，重建索引）。

pub mod log;
pub mod persist;
pub mod resume_preset;
pub mod roster;
pub mod search;
