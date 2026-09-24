//! `preset/list`：GUI 新对话页选预设用。预设在创建会话时选定（`thread/start
//! {presetId}`），之后不在会话中途换，所以这里只读、不给切换方法。

use serde_json::{json, Value};

use cordis_spine::{AgentPresets, PresetOrigin, AGENT_PRESETS};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;

/// 顺序和 TUI `/preset` 一样。`defaultId` 是不带 `presetId` 开新会话时用的那个。
/// 坏掉的预设也列出来（`available: false` + `error`），但 `thread/start` 会拒。
pub fn list(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let presets = gateway
        .ctx()
        .get::<AgentPresets>(AGENT_PRESETS)
        .ok_or_else(|| RpcError::app("unavailable", "预设服务没有挂载"))?;
    let items: Vec<Value> = presets
        .list()
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id,
                "label": if p.name.trim().is_empty() { p.id.clone() } else { p.name },
                "description": p.description,
                "origin": match p.origin {
                    PresetOrigin::Shipped => "shipped",
                    PresetOrigin::User => "user",
                    PresetOrigin::Project => "project",
                },
                "available": p.broken.is_none(),
                "error": p.broken,
            })
        })
        .collect();
    Ok(json!({ "presets": items, "defaultId": presets.current_id() }))
}
