use serde_json::{json, Value};

use cordis_spine::{session_cwd, AppSettings, Mcp, MCP, SETTINGS};

use crate::handle::GatewayHandle;
use crate::protocol::{self, RpcError};

pub fn workspace_list(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    if params.get("scope").and_then(Value::as_str) == Some("all") {
        return Ok(json!({ "workspaces": all_workspaces(gateway) }));
    }
    let cwd = session_cwd(gateway.ctx()).display().to_string();
    Ok(json!({
        "workspaces": [{
            "id": protocol::DEFAULT_WORKSPACE_ID,
            "trustLevel": "workspace",
            "default": true,
            "path": cwd
        }],
        "defaultWorkspaceId": protocol::DEFAULT_WORKSPACE_ID
    }))
}

/// `workspace/list { scope: "all" }`：有会话的所有目录（开着的页 + 跨目录名册），
/// 每个目录一个工作区，`id` 就是路径。GUI 左侧项目树的分组。
fn all_workspaces(gateway: &GatewayHandle) -> Vec<Value> {
    let mut seen = std::collections::BTreeMap::<String, bool>::new();
    for page in crate::threads::open_pages(gateway) {
        seen.insert(session_cwd(&page.ctx).display().to_string(), true);
    }
    for entry in crate::handlers::thread::fresh_roster(gateway).list() {
        seen.entry(entry.cwd.display().to_string()).or_insert(false);
    }
    seen.into_iter()
        .map(|(path, open)| {
            let name = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            json!({ "id": path, "path": path, "name": name, "hasOpenThreads": open })
        })
        .collect()
}

/// `mcp/reload`：重读配置里的 `[mcp_servers.*]`，新增的连上、删掉的断开、改过的
/// 和上次没连上的重连。`ok` / `serverCount` 是老字段（embed-sdk 在读），其余是这次
/// 做了什么。
pub async fn mcp_reload(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let mcp = mcp(gateway)?;
    let report = mcp.reload().await.map_err(|e| RpcError::app("busy", e))?;
    Ok(json!({
        "ok": true,
        "serverCount": mcp.list().len(),
        "summary": report.summary(),
        "added": report.added,
        "removed": report.removed,
        "reconnected": report.reconnected,
        "disabled": report.disabled,
        "failed": report
            .failed
            .iter()
            .map(|(name, error)| json!({ "name": name, "error": error }))
            .collect::<Vec<_>>(),
        "servers": servers(&mcp)
    }))
}

/// `mcp/list`：每台 MCP 服务器的状态。不给启动命令（参数里可能带密钥）。
pub fn mcp_list(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let mcp = mcp(gateway)?;
    Ok(json!({ "servers": servers(&mcp) }))
}

/// `mcp/reconnect { name }`：只重连这一台，不改配置。连不上回错误，状态里也记着原因。
pub async fn mcp_reconnect(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("name is required"))?;
    let mcp = mcp(gateway)?;
    let result = mcp.reconnect(name).await;
    let server = servers(&mcp).into_iter().find(|s| s["name"] == json!(name));
    match result {
        Ok(()) => Ok(json!({ "ok": true, "server": server })),
        Err(e) => Err(RpcError::app("reconnect_failed", e)),
    }
}

fn mcp(gateway: &GatewayHandle) -> Result<std::sync::Arc<Mcp>, RpcError> {
    gateway
        .ctx()
        .get::<Mcp>(MCP)
        .ok_or_else(|| RpcError::app("unavailable", "MCP 服务没有挂载"))
}

fn servers(mcp: &Mcp) -> Vec<Value> {
    mcp.list()
        .into_iter()
        .map(|s| {
            let status = if !s.enabled {
                "disabled"
            } else if s.ok {
                "connected"
            } else if s.needs_auth {
                "needs_auth"
            } else {
                "failed"
            };
            json!({
                "name": s.name,
                "status": status,
                // 连着时是工具数的说明，没连上时是原因。
                "detail": s.detail,
                "toolCount": s.tools.len(),
                "enabledToolCount": s.tools.iter().filter(|t| t.enabled).count()
            })
        })
        .collect()
}

/// `model/list`：config.toml 模型目录（只读）。不给密钥，只说有没有配。
pub fn model_list(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let settings = gateway
        .ctx()
        .get::<AppSettings>(SETTINGS)
        .ok_or_else(|| RpcError::app("unavailable", "settings service is not mounted"))?;
    let current = settings.model();
    let models: Vec<Value> = settings
        .catalog()
        .into_iter()
        .map(|m| {
            let auth = if m.api_key.as_deref().is_some_and(|k| !k.is_empty()) {
                "key"
            } else if m.env_key.as_deref().is_some_and(|k| !k.is_empty()) {
                "env"
            } else {
                "none"
            };
            json!({
                "id": m.id,
                "label": if m.name.is_empty() { m.id.clone() } else { m.name.clone() },
                "description": m.description,
                "apiBase": m.api_base_url,
                "contextWindow": m.context_window,
                "backends": m.api_backends.iter().map(|b| b.name()).collect::<Vec<_>>(),
                "auth": auth,
                "default": m.id == current
            })
        })
        .collect();
    Ok(json!({ "models": models, "default": current }))
}
