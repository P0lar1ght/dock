//! dock.1 protocol constants, method names, and JSON-RPC error helpers.

use serde::Serialize;
use serde_json::{json, Value};

pub const PROTOCOL_VERSION: &str = "dock.1";
pub const LIVE_THREAD_ID: &str = "live";
pub const DEFAULT_WORKSPACE_ID: &str = "default";
pub const SERVER_NAME: &str = "dock";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const WS_PATH: &str = "/api/ws";

/// Capabilities actually implemented in this crate.
pub const CAPABILITIES: &[(&str, bool)] = &[
    ("turns", true),
    ("permissions", true),
    ("transcriptEvents", true),
    ("threadSubscriptions", true),
    ("threads", true),
    ("hostTools", false),
    ("imageInputs", true),
    ("slash", true),
    // 多线程：`thread/list {scope:"all"}` / `thread/open` / `thread/close` /
    // `thread/start {cwd}`，其它线程级方法接受任意开着的线程的会话 id。
    ("openThreads", true),
];

pub fn capabilities_object() -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in CAPABILITIES {
        map.insert((*k).into(), Value::Bool(*v));
    }
    Value::Object(map)
}

pub const CONNECTION_AUTHENTICATE: &str = "connection/authenticate";
pub const INITIALIZE: &str = "initialize";
pub const WORKSPACE_LIST: &str = "workspace/list";
pub const THREAD_LIST: &str = "thread/list";
pub const THREAD_START: &str = "thread/start";
pub const THREAD_OPEN: &str = "thread/open";
pub const THREAD_CLOSE: &str = "thread/close";
pub const THREAD_RENAME: &str = "thread/rename";
pub const THREAD_ARCHIVE: &str = "thread/archive";
pub const THREAD_RESTORE: &str = "thread/restore";
pub const THREAD_DELETE: &str = "thread/delete";
pub const THREAD_HISTORY: &str = "thread/history";
pub const THREAD_SUBSCRIBE: &str = "thread/subscribe";
pub const THREAD_UNSUBSCRIBE: &str = "thread/unsubscribe";
pub const THREAD_ENVIRONMENT_GET: &str = "thread/environment/get";
pub const THREAD_MODEL_SET: &str = "thread/model/set";
pub const THREAD_MODEL_REFRESH: &str = "thread/model/refresh";
pub const THREAD_REASONING_SET: &str = "thread/reasoning/set";
pub const THREAD_APPROVAL_SET: &str = "thread/approval/set";
pub const THREAD_PLAN_SET: &str = "thread/plan/set";
pub const THREAD_MEMORY_SET: &str = "thread/memory/set";
pub const THREAD_GOAL_SET: &str = "thread/goal/set";
pub const THREAD_GOAL_EDIT: &str = "thread/goal/edit";
pub const THREAD_GOAL_PAUSE: &str = "thread/goal/pause";
pub const THREAD_GOAL_COMPLETE: &str = "thread/goal/complete";
pub const THREAD_GOAL_CLEAR: &str = "thread/goal/clear";
pub const THREAD_CONTEXT_COMPACT: &str = "thread/context/compact";
pub const TURN_START: &str = "turn/start";
pub const TURN_ENQUEUE: &str = "turn/enqueue";
pub const TURN_STEER: &str = "turn/steer";
pub const TURN_CANCEL: &str = "turn/cancel";
pub const TURN_QUEUE_LIST: &str = "turn/queue/list";
pub const TURN_QUEUE_REMOVE: &str = "turn/queue/remove";
pub const PERMISSION_RESOLVE: &str = "permission/resolve";
pub const INTERACTION_RESPOND: &str = "interaction/respond";
pub const PLAN_RESOLVE: &str = "plan/resolve";
pub const ELICIT_RESOLVE: &str = "elicit/resolve";
pub const MCP_RELOAD: &str = "mcp/reload";
pub const SLASH_LIST: &str = "slash/list";
pub const SLASH_EXECUTE: &str = "slash/execute";
pub const IMAGE_INPUTS_SYNC: &str = "imageInputs/sync";
pub const IMAGE_INPUTS_PUT: &str = "imageInputs/put";
pub const IMAGE_INPUTS_LIMITS_VERSION: u32 = 3;

#[derive(Clone, Debug)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    pub details_code: &'static str,
}

impl RpcError {
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
            details_code: "method_not_found",
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            details_code: "invalid_params",
        }
    }

    pub fn app(details_code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: -32000,
            message: message.into(),
            details_code,
        }
    }

    pub fn into_value(self) -> Value {
        json!({
            "code": self.code,
            "message": self.message,
            "details": { "code": self.details_code }
        })
    }
}

#[allow(dead_code)]
pub fn json_error(details_code: &'static str, message: impl Into<String>) -> Value {
    json!({ "error": { "code": details_code, "message": message.into() } })
}

#[derive(Serialize)]
pub struct HttpErrorBody {
    pub error: HttpErrorInner,
}

#[derive(Serialize)]
pub struct HttpErrorInner {
    pub code: &'static str,
    pub message: String,
}

pub fn http_error(code: &'static str, message: impl Into<String>) -> HttpErrorBody {
    HttpErrorBody {
        error: HttpErrorInner {
            code,
            message: message.into(),
        },
    }
}
