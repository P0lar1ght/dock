//! `preset/list`：GUI 新对话页选预设用。预设在创建会话时选定（`thread/start
//! {presetId}`），之后不在会话中途换，所以不给切换方法。
//! `preset/create` / `preset/delete`：GUI 的自定义预设（写用户层 `~/.dock/presets`）。

use serde_json::{json, Value};

use cordis_spine::{AgentPreset, AgentPresets, PresetOrigin, AGENT_PRESETS};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;

/// 顺序和 TUI `/preset` 一样。`defaultId` 是不带 `presetId` 开新会话时用的那个。
/// 坏掉的预设也列出来（`available: false` + `error`），但 `thread/start` 会拒。
pub fn list(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let presets = gateway
        .ctx()
        .get::<AgentPresets>(AGENT_PRESETS)
        .ok_or_else(|| RpcError::app("unavailable", "预设服务没有挂载"))?;
    let items: Vec<Value> = presets.list().into_iter().map(summary).collect();
    Ok(json!({ "presets": items, "defaultId": presets.current_id() }))
}

/// `preset/create { name, icon?, description?, basedOn? }`：新建用户层预设，不切当前
/// 预设。`basedOn` 给了就照它复制人设、工具名单和子代理。回新预设（同 `preset/list` 的一项）。
pub fn create(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let presets = service(gateway)?;
    let text = |key: &str| params.get(key).and_then(Value::as_str);
    let name = text("name").ok_or_else(|| RpcError::invalid_params("name is required"))?;
    let made = presets
        .create_custom(
            name,
            text("icon").filter(|s| !s.is_empty()).map(String::from),
            text("description").unwrap_or(""),
            text("basedOn").filter(|s| !s.is_empty()),
        )
        .map_err(|e| RpcError::app("invalid_params", e))?;
    Ok(json!({ "preset": summary(made) }))
}

/// `preset/delete { id }`：删用户 / 项目层预设；内置的删不掉。已经用它开过的会话不受
/// 影响（会话记着预设 id，列表里找不到时客户端自己归类）。
pub fn delete(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("id is required"))?;
    service(gateway)?
        .delete(id)
        .map_err(|e| RpcError::app("invalid_params", e))?;
    Ok(json!({ "ok": true, "id": id }))
}

fn service(gateway: &GatewayHandle) -> Result<std::sync::Arc<AgentPresets>, RpcError> {
    gateway
        .ctx()
        .get::<AgentPresets>(AGENT_PRESETS)
        .ok_or_else(|| RpcError::app("unavailable", "预设服务没有挂载"))
}

fn summary(p: AgentPreset) -> Value {
    json!({
        "id": p.id,
        "label": if p.name.trim().is_empty() { p.id.clone() } else { p.name },
        "description": p.description,
        "icon": p.icon,
        "origin": match p.origin {
            PresetOrigin::Shipped => "shipped",
            PresetOrigin::User => "user",
            PresetOrigin::Project => "project",
        },
        "available": p.broken.is_none(),
        "error": p.broken,
    })
}
