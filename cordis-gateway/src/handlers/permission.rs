use serde_json::{json, Value};

use cordis_spine::{PermissionOptionKind, Permissions, PERMISSIONS};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;

pub fn resolve(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let decision = params
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or("");
    let always = params.get("always").and_then(Value::as_bool).unwrap_or(false);
    let kind = match (decision, always) {
        ("approve", true) => PermissionOptionKind::AllowAlways,
        ("approve", false) => PermissionOptionKind::AllowOnce,
        ("deny", true) => PermissionOptionKind::RejectAlways,
        ("deny", false) => PermissionOptionKind::RejectOnce,
        ("allow_once" | "AllowOnce", _) => PermissionOptionKind::AllowOnce,
        ("allow_always" | "AllowAlways", _) => PermissionOptionKind::AllowAlways,
        ("reject_once" | "RejectOnce", _) => PermissionOptionKind::RejectOnce,
        ("reject_always" | "RejectAlways", _) => PermissionOptionKind::RejectAlways,
        _ => {
            return Err(RpcError::invalid_params(
                "decision must be approve or deny",
            ))
        }
    };
    let perms = gateway
        .ctx()
        .get::<Permissions>(PERMISSIONS)
        .ok_or_else(|| RpcError::app("unavailable", "permissions service is not mounted"))?;
    if !perms.resolve(kind) {
        return Err(RpcError::app(
            "empty_queue",
            "no pending permission request",
        ));
    }
    Ok(json!({ "ok": true, "resumed": true }))
}
