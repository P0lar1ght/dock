//! JSON-RPC WebSocket at `/api/ws`.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};

use crate::handle::GatewayHandle;
use crate::http::AppState;
use crate::pairing::IssuedTicket;
use crate::protocol::{self, RpcError};
use crate::rpc;

pub async fn upgrade(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    ws.on_upgrade(move |socket| handle_socket(socket, state.gateway, origin))
}

struct Conn {
    gateway: GatewayHandle,
    origin: String,
    auth: Option<IssuedTicket>,
    initialized: bool,
    connection_lease_id: Option<String>,
    /// 分页身份 → 这条连接订阅时用的 `threadId`。
    subscribed: crate::handlers::thread::Subscriptions,
}

async fn handle_socket(socket: WebSocket, gateway: GatewayHandle, origin: String) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let conn = Arc::new(Mutex::new(Conn {
        gateway: gateway.clone(),
        origin,
        auth: None,
        initialized: false,
        connection_lease_id: None,
        subscribed: Default::default(),
    }));

    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    let mut events_rx = gateway.subscribe_transcript();
    let fanout = {
        let conn = conn.clone();
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            loop {
                match events_rx.recv().await {
                    Ok(event) => {
                        // 按页匹配订阅，推送时用这条连接订阅时的 `threadId`。
                        let alias = {
                            let c = conn.lock().await;
                            c.initialized
                                .then(|| c.subscribed.get(&event.page).cloned())
                                .flatten()
                        };
                        if let Some(alias) = alias {
                            let _ = out_tx.send(event.as_notification_as(&alias).to_string());
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    };

    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else {
            continue;
        };
        // 要调模型的方法另起任务：不能占着连接锁等好几秒。
        if let Some(job) = detached(&conn, &text).await {
            let out_tx = out_tx.clone();
            tokio::spawn(async move {
                let _ = out_tx.send(job.await);
            });
            continue;
        }
        let reply = dispatch_text(&conn, &text).await;
        if let Some(reply) = reply {
            if out_tx.send(reply).is_err() {
                break;
            }
        }
    }
    fanout.abort();
    drop(out_tx);
    let _ = writer.await;
}

/// [`rpc::is_detached`] 的请求：锁里只查鉴权、拿网关句柄，放锁后再跑。不是这类请求
/// （或没法解析）回 `None`，照常走 [`dispatch_text`]。
async fn detached(
    conn: &Arc<Mutex<Conn>>,
    text: &str,
) -> Option<impl std::future::Future<Output = String>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let method = value.get("method").and_then(Value::as_str)?.to_string();
    if !rpc::is_detached(&method) {
        return None;
    }
    let id = value.get("id").cloned();
    let params = value.get("params").cloned().unwrap_or(json!({}));
    let ready = {
        let c = conn.lock().await;
        if c.auth.is_none() {
            Err(RpcError::app(
                "unauthenticated",
                "connection/authenticate is required",
            ))
        } else if !c.initialized {
            Err(RpcError::app(
                "not_initialized",
                "initialize is required after authenticate",
            ))
        } else {
            Ok(c.gateway.clone())
        }
    };
    Some(async move {
        let result = match ready {
            Ok(gateway) => rpc::dispatch_detached(gateway, &method, params).await,
            Err(e) => Err(e),
        };
        match result {
            Ok(result) => json!({ "id": id.unwrap_or(Value::Null), "result": result }).to_string(),
            Err(err) => rpc_error_frame(id, err),
        }
    })
}

async fn dispatch_text(conn: &Arc<Mutex<Conn>>, text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    let id = value.get("id").cloned();
    let method = value.get("method").and_then(Value::as_str).unwrap_or("");
    let params = value.get("params").cloned().unwrap_or(json!({}));
    if method.is_empty() {
        return Some(rpc_error_frame(
            id,
            RpcError::invalid_params("method is required"),
        ));
    }
    let result = {
        let mut c = conn.lock().await;
        dispatch_locked(&mut c, method, params).await
    };
    match result {
        Ok(result) => id.map(|id| json!({ "id": id, "result": result }).to_string()),
        Err(err) => Some(rpc_error_frame(id, err)),
    }
}

async fn dispatch_locked(conn: &mut Conn, method: &str, params: Value) -> Result<Value, RpcError> {
    if method == protocol::CONNECTION_AUTHENTICATE {
        let ticket = params.get("ticket").and_then(Value::as_str).unwrap_or("");
        let auth = conn
            .gateway
            .authenticate(ticket, &conn.origin)
            .map_err(|e| RpcError::app(e.code, e.message))?;
        conn.auth = Some(auth);
        return Ok(json!({ "ok": true }));
    }
    if conn.auth.is_none() {
        return Err(RpcError::app(
            "unauthenticated",
            "connection/authenticate is required",
        ));
    }
    if method == protocol::INITIALIZE {
        let auth = conn.auth.as_ref().unwrap();
        conn.initialized = true;
        let lease = uuid::Uuid::new_v4().to_string();
        conn.connection_lease_id = Some(lease.clone());
        return Ok(json!({
            "ok": true,
            "serverInfo": {
                "name": protocol::SERVER_NAME,
                "version": protocol::SERVER_VERSION
            },
            "protocolVersion": protocol::PROTOCOL_VERSION,
            "capabilities": protocol::capabilities_object(),
            "connection": {
                "application": auth.application,
                "origin": auth.origin,
                "defaultWorkspaceId": protocol::DEFAULT_WORKSPACE_ID,
                "connectionLeaseId": lease,
                "listen": conn.gateway.listen_addr().to_string(),
                "companion": companion_json(&conn.gateway.companion_status())
            }
        }));
    }
    if method == protocol::IMAGE_INPUTS_SYNC {
        let lease = conn.connection_lease_id.clone().ok_or_else(|| {
            RpcError::app(
                "not_initialized",
                "initialize is required after authenticate",
            )
        })?;
        return Ok(json!({
            "connectionLeaseId": lease,
            "limitsVersion": protocol::IMAGE_INPUTS_LIMITS_VERSION
        }));
    }
    if !conn.initialized {
        return Err(RpcError::app(
            "not_initialized",
            "initialize is required after authenticate",
        ));
    }
    rpc::dispatch(conn.gateway.clone(), method, params, &mut conn.subscribed).await
}

fn rpc_error_frame(id: Option<Value>, err: RpcError) -> String {
    json!({
        "id": id.unwrap_or(Value::Null),
        "error": err.into_value()
    })
    .to_string()
}

fn companion_json(status: &cordis_tui::CompanionStatus) -> Value {
    match status {
        cordis_tui::CompanionStatus::Stopped => json!({
            "status": "stopped",
            "listening": false
        }),
        cordis_tui::CompanionStatus::Listening(addr) => json!({
            "status": "listening",
            "address": addr.to_string()
        }),
        cordis_tui::CompanionStatus::Failed { addr, error } => json!({
            "status": "failed",
            "address": addr.to_string(),
            "error": error
        }),
    }
}
