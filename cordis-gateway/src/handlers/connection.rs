use serde_json::{json, Value};

use crate::handle::GatewayHandle;
use crate::protocol::{self, RpcError};

pub fn workspace_list(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| ".".into());
    let _ = gateway;
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
