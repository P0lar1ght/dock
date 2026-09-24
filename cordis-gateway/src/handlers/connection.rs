use serde_json::{json, Value};

use cordis_spine::{session_cwd, Mcp, MCP};

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

/// Dock MCP is fail-open and already live; this is a snapshot, not PolarVigil reconnect.
pub fn mcp_reload(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let count = gateway
        .ctx()
        .get::<Mcp>(MCP)
        .map(|m| m.list().len())
        .unwrap_or(0);
    Ok(json!({
        "ok": true,
        "serverCount": count
    }))
}
