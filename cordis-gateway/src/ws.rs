//! JSON-RPC WebSocket at `/api/ws`.

use std::sync::Arc;

use axum::extract::ws::CloseFrame;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch, Mutex};

use crate::devices::{self, Device};
use crate::handle::GatewayHandle;
use crate::handlers::browser_view::{self, BrowserViews};
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

/// 发往这条连接的一条消息。画面帧带「写出去了」的回执：写完才让 Chrome 发下一帧，
/// 慢客户端不会在这里攒出一长串帧（见 [`browser_view`]）。
pub(crate) enum Outgoing {
    Text(String),
    Frame {
        text: String,
        written: oneshot::Sender<()>,
    },
    /// 服务端主动断开（设备被撤销、鉴权失败太多次）。
    Close {
        code: u16,
        reason: &'static str,
    },
}

/// 同一条连接鉴权失败这么多次就断开（反向代理那头再做全局限速）。
const MAX_FAILED_AUTH: u32 = 5;
/// 连着的设备多久复查一次有没有被撤销。
const DEVICE_RECHECK: std::time::Duration = std::time::Duration::from_secs(2);
/// 断开原因的 WebSocket 关闭码（4000–4999 留给应用）。
const CLOSE_REVOKED: u16 = 4401;
const CLOSE_TOO_MANY_ATTEMPTS: u16 = 4429;

/// 这条连接凭什么进来的。
enum Auth {
    /// 本机配对 / `dock serve` 父进程给的一次性 ticket，绑 Origin。
    Ticket(IssuedTicket),
    /// 设备令牌（远程客户端），不绑 Origin。
    Device(Device),
}

impl Auth {
    fn application(&self) -> String {
        match self {
            Self::Ticket(t) => t.application.clone(),
            Self::Device(d) => format!("device:{}", d.name),
        }
    }

    fn origin(&self, connection_origin: &str) -> String {
        match self {
            Self::Ticket(t) => t.origin.clone(),
            Self::Device(_) => connection_origin.to_string(),
        }
    }
}

pub(crate) type OutTx = mpsc::UnboundedSender<Outgoing>;

struct Conn {
    gateway: GatewayHandle,
    origin: String,
    auth: Option<Auth>,
    failed_auth: u32,
    /// 置上就断开这条连接（`handle_socket` 盯着它）。
    kick: watch::Sender<Option<(u16, &'static str)>>,
    initialized: bool,
    connection_lease_id: Option<String>,
    /// 分页身份 → 这条连接订阅时用的 `threadId`。
    subscribed: crate::handlers::thread::Subscriptions,
}

async fn handle_socket(socket: WebSocket, gateway: GatewayHandle, origin: String) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Outgoing>();
    // 连接级：断开时随它一起丢掉，推送任务全部中止。
    let views = Arc::new(BrowserViews::default());
    let (kick, mut kicked) = watch::channel(None);
    let conn = Arc::new(Mutex::new(Conn {
        gateway: gateway.clone(),
        origin,
        auth: None,
        failed_auth: 0,
        kick,
        initialized: false,
        connection_lease_id: None,
        subscribed: Default::default(),
    }));

    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let (text, written) = match msg {
                Outgoing::Text(text) => (text, None),
                Outgoing::Frame { text, written } => (text, Some(written)),
                Outgoing::Close { code, reason } => {
                    let frame = CloseFrame {
                        code,
                        reason: reason.into(),
                    };
                    let _ = sink.send(Message::Close(Some(frame))).await;
                    break;
                }
            };
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
            if let Some(written) = written {
                let _ = written.send(());
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
                            let _ = out_tx
                                .send(Outgoing::Text(event.as_notification_as(&alias).to_string()));
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    };

    loop {
        let msg = tokio::select! {
            msg = stream.next() => msg,
            _ = kicked.changed() => {
                if let Some((code, reason)) = *kicked.borrow() {
                    let _ = out_tx.send(Outgoing::Close { code, reason });
                }
                break;
            }
        };
        let Some(Ok(msg)) = msg else {
            break;
        };
        let Message::Text(text) = msg else {
            continue;
        };
        // 要调模型的方法、挂浏览器画面另起任务：不能占着连接锁等好几秒。
        if let Some(job) = detached(&conn, &views, &out_tx, &text).await {
            let out_tx = out_tx.clone();
            tokio::spawn(async move {
                let _ = out_tx.send(Outgoing::Text(job.await));
            });
            continue;
        }
        let reply = dispatch_text(&conn, &text).await;
        if let Some(reply) = reply {
            if out_tx.send(Outgoing::Text(reply)).is_err() {
                break;
            }
        }
    }
    fanout.abort();
    drop(views);
    drop(out_tx);
    let _ = writer.await;
}

/// [`rpc::is_detached`] 与 `browser/view/*` 的请求：锁里只查鉴权、拿网关句柄，放锁后
/// 再跑。不是这类请求（或没法解析）回 `None`，照常走 [`dispatch_text`]。
async fn detached(
    conn: &Arc<Mutex<Conn>>,
    views: &Arc<BrowserViews>,
    out: &OutTx,
    text: &str,
) -> Option<impl std::future::Future<Output = String>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let method = value.get("method").and_then(Value::as_str)?.to_string();
    let viewing = browser_view::is_browser_view(&method);
    if !rpc::is_detached(&method) && !viewing {
        return None;
    }
    let views = views.clone();
    let out = out.clone();
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
            Ok(gateway) if viewing => {
                browser_view::dispatch(&gateway, &views, &out, &method, params).await
            }
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
        return authenticate(conn, &params);
    }
    if conn.auth.is_none() {
        return Err(RpcError::app(
            "unauthenticated",
            "connection/authenticate is required",
        ));
    }
    if method == protocol::INITIALIZE {
        let auth = conn.auth.as_ref().unwrap();
        let (application, origin) = (auth.application(), auth.origin(&conn.origin));
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
                "application": application,
                "origin": origin,
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

/// `connection/authenticate { token }`（设备令牌）或 `{ ticket }`（本机配对 / `dock serve`）。
/// 远程网关只认前者。失败太多次断开连接。
fn authenticate(conn: &mut Conn, params: &Value) -> Result<Value, RpcError> {
    let token = params.get("token").and_then(Value::as_str);
    let result = match token {
        Some(token) => devices::verify(token)
            .map(Auth::Device)
            .ok_or_else(|| RpcError::app("unauthenticated", "设备令牌无效或已撤销")),
        None if conn.gateway.is_remote() => Err(RpcError::app(
            "unauthenticated",
            "远程网关只认设备令牌：connection/authenticate { token }",
        )),
        None => {
            let ticket = params.get("ticket").and_then(Value::as_str).unwrap_or("");
            conn.gateway
                .authenticate(ticket, &conn.origin)
                .map(Auth::Ticket)
                .map_err(|e| RpcError::app(e.code, e.message))
        }
    };
    match result {
        Ok(auth) => {
            if let Auth::Device(device) = &auth {
                if conn.gateway.is_remote() {
                    eprintln!("dock gateway: 设备「{}」已连接", device.name);
                }
                watch_device(device.id.clone(), conn.kick.clone());
            }
            conn.auth = Some(auth);
            conn.failed_auth = 0;
            Ok(json!({ "ok": true }))
        }
        Err(err) => {
            conn.failed_auth += 1;
            if conn.failed_auth >= MAX_FAILED_AUTH {
                let _ = conn
                    .kick
                    .send(Some((CLOSE_TOO_MANY_ATTEMPTS, "too many failed attempts")));
            }
            Err(err)
        }
    }
}

/// 设备被撤销就断开它的连接。连接没了（watch 的接收端丢了）任务自己结束。
fn watch_device(id: String, kick: watch::Sender<Option<(u16, &'static str)>>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(DEVICE_RECHECK).await;
            if kick.is_closed() {
                return;
            }
            if !devices::is_active(&id) {
                let _ = kick.send(Some((CLOSE_REVOKED, "device revoked")));
                return;
            }
        }
    });
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
