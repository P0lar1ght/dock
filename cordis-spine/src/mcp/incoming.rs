//! Server → client JSON-RPC on a shared stream (stdio / POST SSE / GET SSE).

use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::elicitation::Elicitation;
use super::protocol::{self, Incoming};
use super::tools_list;

#[derive(Clone)]
pub struct LiveHooks {
    pub server: String,
    pub elicit: Elicitation,
    pub tools_changed: mpsc::UnboundedSender<String>,
}

pub fn dummy_hooks(server: &str) -> LiveHooks {
    LiveHooks {
        server: server.to_string(),
        elicit: Elicitation::new(cordis::Context::new()),
        tools_changed: mpsc::unbounded_channel().0,
    }
}

/// `None` = caller keeps a JSON-RPC **response**. `Some` = reply to send.
pub async fn dispatch(hooks: &LiveHooks, v: Value) -> Option<Value> {
    match protocol::classify(v) {
        Incoming::Response(_) => None,
        Incoming::Notification { method, params } => {
            note(hooks, &method, &params);
            None
        }
        Incoming::Request { id, method, params } => Some(request(hooks, id, method, params).await),
    }
}

pub fn note(hooks: &LiveHooks, method: &str, params: &Value) {
    if Elicitation::is_complete(method) {
        hooks.elicit.complete_url(params);
        return;
    }
    if tools_list::is_changed(method) {
        let _ = hooks.tools_changed.send(hooks.server.clone());
    }
}

pub async fn request(hooks: &LiveHooks, id: Value, method: String, params: Value) -> Value {
    if method == "ping" {
        return protocol::jsonrpc_result(id, protocol::ping_result());
    }
    if Elicitation::is_create(&method) {
        let result = hooks.elicit.create(&hooks.server, params).await;
        return protocol::jsonrpc_result(id, result);
    }
    protocol::jsonrpc_method_not_found(id, &method)
}

pub fn pending_id(v: &Value) -> Option<u64> {
    match v.get("id") {
        Some(Value::Number(n)) => n.as_u64().or_else(|| n.as_i64().map(|i| i as u64)),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    }
}

pub fn jsonrpc_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn ping_replies() {
        let hooks = dummy_hooks("s");
        let reply = dispatch(
            &hooks,
            json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}}),
        )
        .await
        .unwrap();
        assert_eq!(reply["result"], json!({}));
    }

    #[test]
    fn list_changed_notifies() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let hooks = LiveHooks {
            server: "local".into(),
            elicit: Elicitation::new(cordis::Context::new()),
            tools_changed: tx,
        };
        note(
            &hooks,
            "notifications/tools/list_changed",
            &json!({}),
        );
        assert_eq!(rx.try_recv().unwrap(), "local");
    }
}
