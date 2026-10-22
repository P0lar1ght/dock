//! 内置 MCP App 共用的 stdio 服务：一行一条 JSON-RPC（NDJSON，与 Dock 客户端默认帧一致）。
//!
//! 两代握手都认：`2026-07-28` 的 `server/discover`，以及 initialize 时代。认 `tools/list`、
//! `tools/call`、`resources/list`、`resources/read`、`ping`。工具调用各起一个任务，结果经一条
//! 写通道回 stdout；`notifications/cancelled` 打断对应的任务。读端 EOF（Dock 退出）就收尾。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

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

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// 一个内置 App：工具、界面资源、调用。
pub trait App: Send + Sync + 'static {
    /// `serverInfo.name`。
    fn name(&self) -> &'static str;
    /// `tools/list` 的 `tools`（含 `_meta.ui`）。
    fn tools(&self) -> Vec<Value>;
    /// `resources/list` 的 `resources`。
    fn resources(&self) -> Vec<Value>;
    /// `resources/read`：认得的 uri 回 `{ contents: [...] }`。
    fn read(&self, uri: &str) -> Option<Value>;
    /// `tools/call`：回规范的 `CallToolResult`。`meta` 是请求的 `_meta`（Dock 带会话、工作目录）。
    fn call(self: Arc<Self>, name: String, args: Value, meta: Value) -> BoxFuture<Value>;
}

/// 在进程自己的 stdin / stdout 上跑。
pub async fn serve_stdio<A: App>(app: Arc<A>) -> std::io::Result<()> {
    serve(app, tokio::io::stdin(), tokio::io::stdout()).await
}

/// 在任意读写端上跑（测试用 duplex）。读端 EOF 即收尾返回。
pub async fn serve<A, R, W>(app: Arc<A>, input: R, output: W) -> std::io::Result<()>
where
    A: App,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(write_loop(output, rx));
    let inflight: Inflight = Arc::default();
    let mut lines = BufReader::new(input).lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(msg) => handle(&app, &tx, &inflight, msg),
            Err(e) => {
                let _ = tx.send(error(
                    Value::Null,
                    PARSE_ERROR,
                    &format!("parse error: {e}"),
                ));
            }
        }
    }
    let pending: Vec<JoinHandle<()>> = inflight.lock().unwrap().drain().map(|(_, h)| h).collect();
    for h in pending {
        let _ = h.await;
    }
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

fn handle<A: App>(
    app: &Arc<A>,
    tx: &mpsc::UnboundedSender<Value>,
    inflight: &Inflight,
    msg: Value,
) {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    // 通知（没有 id）：只认取消，其余忽略。
    let Some(id) = msg.get("id").cloned() else {
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
        "server/discover" => result(id, discover_result(app.name())),
        "initialize" => result(id, initialize_result(app.name(), &params)),
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({ "tools": app.tools() })),
        "resources/list" => result(id, json!({ "resources": app.resources() })),
        "resources/templates/list" => result(id, json!({ "resourceTemplates": [] })),
        "resources/read" => match params.get("uri").and_then(Value::as_str) {
            Some(uri) => match app.read(uri) {
                Some(r) => result(id, r),
                None => error(id, INVALID_PARAMS, &format!("unknown resource: {uri}")),
            },
            None => error(id, INVALID_PARAMS, "resources/read requires uri"),
        },
        "tools/call" => {
            spawn_call(app, tx, inflight, id, params);
            return;
        }
        other => error(id, METHOD_NOT_FOUND, &format!("method not found: {other}")),
    };
    let _ = tx.send(reply);
}

fn spawn_call<A: App>(
    app: &Arc<A>,
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
    let meta = params.get("_meta").cloned().unwrap_or_else(|| json!({}));
    let key = id.to_string();
    let app = app.clone();
    let tx = tx.clone();
    let done = inflight.clone();
    let done_key = key.clone();
    // 先占位再起任务：任务跑得比插入快时，它的 remove 不会落空、留下一个死句柄。
    let mut map = inflight.lock().unwrap();
    let task = tokio::spawn(async move {
        let out = app.call(name, args, meta).await;
        let _ = tx.send(result(id, out));
        done.lock().unwrap().remove(&done_key);
    });
    map.insert(key, task);
}

fn server_info(name: &str) -> Value {
    json!({ "name": name, "version": env!("CARGO_PKG_VERSION") })
}

fn capabilities() -> Value {
    json!({
        "tools": { "listChanged": false },
        "resources": { "listChanged": false, "subscribe": false },
        "extensions": { "io.modelcontextprotocol/ui": {} },
    })
}

fn discover_result(name: &str) -> Value {
    json!({
        "supportedVersions": SUPPORTED,
        "capabilities": capabilities(),
        "serverInfo": server_info(name),
    })
}

fn initialize_result(name: &str, params: &Value) -> Value {
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
        "serverInfo": server_info(name),
    })
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// 工具结果：一段文本（给模型）+ 可选的结构化数据（给界面）。
pub fn tool_result(text: impl Into<String>, structured: Option<Value>, is_error: bool) -> Value {
    let mut out = json!({
        "content": [{ "type": "text", "text": text.into() }],
        "isError": is_error,
    });
    if let Some(s) = structured {
        out["structuredContent"] = s;
    }
    out
}
