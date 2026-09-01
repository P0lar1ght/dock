use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::handle::GatewayHandle;
use crate::image_store::MAX_BYTES;
use crate::protocol::{RpcError, IMAGE_INPUTS_LIMITS_VERSION, LIVE_THREAD_ID};

pub fn put(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let workspace_id = params
        .get("workspaceId")
        .and_then(Value::as_str)
        .unwrap_or("");
    if workspace_id != "default" {
        return Err(RpcError::app(
            "workspace_not_allowed",
            "Workspace is not authorized for image input",
        ));
    }
    let mime = required_str(&params, "mimeType")?;
    let digest = required_str(&params, "digest")?;
    let data_b64 = required_str(&params, "dataBase64")?;
    let width = required_u32(&params, "width")?;
    let height = required_u32(&params, "height")?;
    let byte_length = params
        .get("byteLength")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let data = base64::engine::general_purpose::STANDARD
        .decode(data_b64.as_bytes())
        .map_err(|_| RpcError::invalid_params("dataBase64 is invalid"))?;
    if byte_length != 0 && byte_length != data.len() {
        return Err(RpcError::invalid_params("byteLength does not match data"));
    }
    if data.len() > MAX_BYTES {
        return Err(RpcError::app(
            "image_input_too_large",
            "Normalized screenshot exceeds the image input limit",
        ));
    }
    let id = format!("img-{}", Uuid::new_v4());
    let stored = gateway.put_image(id.clone(), mime, width, height, digest.clone(), data)?;
    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
        + 120_000;
    Ok(json!({
        "imageInputId": id,
        "digest": stored.digest,
        "expiresAt": expires_at,
        "limitsVersion": IMAGE_INPUTS_LIMITS_VERSION,
        "threadId": params.get("threadId").and_then(Value::as_str).unwrap_or(LIVE_THREAD_ID)
    }))
}

fn required_str(params: &Value, key: &str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("{key} is required")))
}

fn required_u32(params: &Value, key: &str) -> Result<u32, RpcError> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| RpcError::invalid_params(format!("{key} is required")))
}
