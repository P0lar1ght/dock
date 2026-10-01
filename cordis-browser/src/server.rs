//! stdio MCP 服务：一行一条 JSON-RPC（NDJSON，与 Dock 客户端默认帧一致）。
//!
//! 两代握手都认：`2026-07-28` 的 `server/discover`（无 initialize、无会话），以及
//! initialize 时代（`2025-11-25` 及更早）。每个 `tools/call` 单独起任务跑，
//! 结果经一条写通道回 stdout；`notifications/cancelled` 打断对应的任务。
//! stdin 关了（Dock 退出）就关掉全部标签页和自己拉起的 Chromium 再退出。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::hub::{BrowserHub, CallOutput};
use crate::tools;

/// Dock 里的服务器键名：公名前缀 `mcp_browser__`。
pub const SERVER_NAME: &str = "browser";

/// 会话身份在 `_meta` 里的键。Dock 客户端写，其它客户端不写就归默认组。
pub const META_SESSION: &str = "dock/sessionId";
const DEFAULT_SESSION: &str = "default";

/// 新的在前。与 Dock 客户端的 `SPOKEN` 对齐。
const SUPPORTED: &[&str] = &[
    "2026-07-28",
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];
const LEGACY_DEFAULT: &str = "2025-11-25";

const PARSE_ERROR: i64 = -32700;
const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;

/// 在进程自己的 stdin / stdout 上跑（`dock mcp browser`）。
pub async fn serve_stdio() -> std::io::Result<()> {
    serve(tokio::io::stdin(), tokio::io::stdout()).await
}

/// 在任意读写端上跑（测试用 duplex）。读端 EOF 即收尾返回。
pub async fn serve<R, W>(input: R, output: W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let hub = Arc::new(BrowserHub::new());
    let (tx, rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(write_loop(output, rx));
    let inflight: Arc<Mutex<HashMap<String, JoinHandle<()>>>> = Arc::default();

    let mut lines = BufReader::new(input).lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(error(
                    Value::Null,
                    PARSE_ERROR,
                    &format!("parse error: {e}"),
                ));
                continue;
            }
        };
        handle(&hub, &tx, &inflight, msg);
    }

    let pending: Vec<JoinHandle<()>> = inflight.lock().unwrap().drain().map(|(_, h)| h).collect();
    for h in pending {
        h.abort();
    }
    hub.shutdown().await;
    drop(tx);
    let _ = writer.await;
    Ok(())
}

async fn write_loop<W: AsyncWrite + Unpin>(mut out: W, mut rx: mpsc::UnboundedReceiver<Value>) {
    while let Some(v) = rx.recv().await {
        let mut line = v.to_string();
        line.push('\n');
        if out.write_all(line.as_bytes()).await.is_err() || out.flush().await.is_err() {
            break;
        }
    }
}

type Inflight = Arc<Mutex<HashMap<String, JoinHandle<()>>>>;

fn handle(
    hub: &Arc<BrowserHub>,
    tx: &mpsc::UnboundedSender<Value>,
    inflight: &Inflight,
    msg: Value,
) {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = msg.get("id").cloned();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    // 通知（没有 id）：只认取消，其余忽略。
    let Some(id) = id else {
        if method == "notifications/cancelled" {
            if let Some(req) = params.get("requestId") {
                if let Some(task) = inflight.lock().unwrap().remove(&req.to_string()) {
                    task.abort();
                }
            }
        }
        return;
    };

    // 客户端回我们的请求（我们不发请求）：忽略。
    if method.is_empty() {
        return;
    }

    let reply = match method {
        "server/discover" => result(id, discover_result()),
        "initialize" => result(id, initialize_result(&params)),
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({ "tools": tools::list_json() })),
        "tools/call" => {
            spawn_call(hub, tx, inflight, id, params);
            return;
        }
        other => error(id, METHOD_NOT_FOUND, &format!("method not found: {other}")),
    };
    let _ = tx.send(reply);
}

fn spawn_call(
    hub: &Arc<BrowserHub>,
    tx: &mpsc::UnboundedSender<Value>,
    inflight: &Inflight,
    id: Value,
    params: Value,
) {
    let Some(name) = params
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        let _ = tx.send(error(id, INVALID_PARAMS, "tools/call requires name"));
        return;
    };
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let session = session_of(&params);
    let key = id.to_string();
    let hub = hub.clone();
    let tx = tx.clone();
    let done = inflight.clone();
    let done_key = key.clone();
    // 先占位再起任务：任务跑得比插入快时，它的 remove 不会落空、留下一个死句柄。
    let mut map = inflight.lock().unwrap();
    let task = tokio::spawn(async move {
        let out = hub.call(&session, &name, &args).await;
        let _ = tx.send(result(id, call_result(out)));
        done.lock().unwrap().remove(&done_key);
    });
    map.insert(key, task);
}

fn session_of(params: &Value) -> String {
    params
        .pointer("/_meta")
        .and_then(|m| m.get(META_SESSION))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(DEFAULT_SESSION)
        .to_string()
}

fn server_info() -> Value {
    json!({ "name": "dock-browser", "version": env!("CARGO_PKG_VERSION") })
}

fn capabilities() -> Value {
    json!({ "tools": { "listChanged": false } })
}

fn discover_result() -> Value {
    json!({
        "supportedVersions": SUPPORTED,
        "capabilities": capabilities(),
        "serverInfo": server_info(),
    })
}

fn initialize_result(params: &Value) -> Value {
    let asked = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(LEGACY_DEFAULT);
    let version = if SUPPORTED.contains(&asked) {
        asked
    } else {
        LEGACY_DEFAULT
    };
    json!({
        "protocolVersion": version,
        "capabilities": capabilities(),
        "serverInfo": server_info(),
    })
}

fn call_result(out: CallOutput) -> Value {
    let mut content = vec![json!({ "type": "text", "text": out.text })];
    if let Some(path) = out.image.as_ref() {
        if let Ok(bytes) = std::fs::read(path) {
            content.push(json!({
                "type": "image",
                "mimeType": "image/png",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }));
        }
    }
    json!({ "content": content, "isError": out.is_error })
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_defaults_when_meta_missing_or_blank() {
        assert_eq!(session_of(&json!({})), "default");
        assert_eq!(
            session_of(&json!({"_meta": {"dock/sessionId": "  "}})),
            "default"
        );
        assert_eq!(
            session_of(&json!({"_meta": {"dock/sessionId": "0199-abc"}})),
            "0199-abc"
        );
    }

    #[test]
    fn initialize_echoes_supported_or_falls_back() {
        let v = initialize_result(&json!({"protocolVersion": "2025-06-18"}));
        assert_eq!(v["protocolVersion"], "2025-06-18");
        let v = initialize_result(&json!({"protocolVersion": "1999-01-01"}));
        assert_eq!(v["protocolVersion"], LEGACY_DEFAULT);
    }

    #[test]
    fn call_result_carries_is_error() {
        let v = call_result(CallOutput {
            text: "Error: nope".into(),
            is_error: true,
            image: None,
        });
        assert_eq!(v["isError"], true);
        assert_eq!(v["content"][0]["text"], "Error: nope");
    }
}
