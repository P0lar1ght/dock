//! `mcp/app/*`：MCP Apps 的宿主接口。桌面端把 `ui://` 界面放进沙箱 iframe，iframe 里的
//! app 经 postMessage 跟桌面端说话，桌面端再经这几个方法转给 Dock。全部只认受信连接
//! （`mcp/` 前缀本来就强制）。
//!
//! - `mcp/app/tools {}` → `{ tools: [{ server, name, publicName, description, inputSchema,
//!   resourceUri, modelVisible, appVisible }] }`：带界面、或只给界面用的工具。桌面端拿它认
//!   工具卡该不该挂界面（经 `use_tool` 调的按内层工具名认）。
//! - `mcp/app/resource { server, uri }` → `{ uri, mimeType, html, csp, permissions, domain,
//!   prefersBorder }`：读界面资源（`resources/read`）。后四项来自资源内容的 `_meta.ui`，
//!   没有就是 `null`。
//! - `mcp/app/call { threadId?, server, tool, arguments? }` → `CallToolResult`：界面发起的
//!   `tools/call`。只认 `visibility` 含 `app` 的工具，不进对话流；会话身份是这个线程。
//! - `mcp/app/result { threadId?, toolCallId }` → `{ call }`：模型那次调用的原始结果
//!   `{ server, tool, resourceUri, arguments, result }`，没存就是 `null`。关着的会话也能读。
//! - `mcp/app/context { threadId?, server, resourceUri, content?, structuredContent? }` → `{}`：
//!   `ui/update-model-context`。按线程、按界面只留最新一份，下一步交给模型。
//!
//! `call` / `context` 要线程开着（会话身份、页身份都从开着的页取）。

use base64::Engine;
use serde_json::{json, Value};

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{read_app_record, record_app_context, AppContextUpdate, Mcp, McpAppTool, MCP};

use crate::handle::GatewayHandle;
use crate::handlers::canvas::session_dir;
use crate::methods::{method, register_methods, MethodPolicy, GATEWAY_METHODS};
use crate::protocol::{self, RpcError};
use crate::threads;

/// MCP Apps 这一块：`mcp/app/tools|resource|call|result|context`。
pub fn gateway_mcp_apps() -> Plugin {
    plugin(
        "gateway.mcpApps",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            let trusted = MethodPolicy {
                trusted_only: true,
                ..MethodPolicy::default()
            };
            // 读资源、调工具要等 MCP 服务回话：放到连接锁外跑。
            let slow = MethodPolicy {
                detached: true,
                trusted_only: true,
                ..MethodPolicy::default()
            };
            register_methods(
                ctx,
                vec![
                    (
                        protocol::MCP_APP_TOOLS,
                        trusted,
                        method(|gw, _| async move { tools(&gw) }),
                    ),
                    (
                        protocol::MCP_APP_RESOURCE,
                        slow,
                        method(|gw, params| async move { resource(&gw, params).await }),
                    ),
                    (
                        protocol::MCP_APP_CALL,
                        slow,
                        method(|gw, params| async move { call(&gw, params).await }),
                    ),
                    (
                        protocol::MCP_APP_RESULT,
                        slow,
                        method(|gw, params| async move { result(&gw, params).await }),
                    ),
                    (
                        protocol::MCP_APP_CONTEXT,
                        trusted,
                        method(|gw, params| async move { context(&gw, params) }),
                    ),
                ],
            )?;
            Ok(None)
        },
    )
}

fn mcp(gateway: &GatewayHandle) -> Result<std::sync::Arc<Mcp>, RpcError> {
    gateway
        .ctx()
        .get::<Mcp>(MCP)
        .ok_or_else(|| RpcError::app("unavailable", "MCP 客户端没有挂载"))
}

fn str_param<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("缺少参数 {key}")))
}

fn tool_value(t: &McpAppTool) -> Value {
    json!({
        "server": t.server,
        "name": t.name,
        "publicName": t.public_name,
        "description": t.description,
        "inputSchema": t.input_schema,
        "resourceUri": t.resource_uri,
        "modelVisible": t.model_visible,
        "appVisible": t.app_visible,
    })
}

fn tools(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let tools: Vec<Value> = mcp(gateway)?.app_tools().iter().map(tool_value).collect();
    Ok(json!({ "tools": tools }))
}

async fn resource(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let server = str_param(&params, "server")?;
    let uri = str_param(&params, "uri")?;
    if !uri.starts_with("ui://") {
        return Err(RpcError::invalid_params("只读 ui:// 界面资源"));
    }
    let read = mcp(gateway)?
        .read_resource(server, uri)
        .await
        .map_err(|e| RpcError::app("mcp", e))?;
    ui_resource(uri, &read)
}

/// `ReadResourceResult` → 界面资源。规范要求：同 uri 的那一项，MIME 是
/// `text/html;profile=mcp-app`（也认没带 profile 的 `text/html`）。对不上就报错，不拿别的资源顶。
fn ui_resource(uri: &str, read: &Value) -> Result<Value, RpcError> {
    let contents = read
        .get("contents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let item = contents
        .iter()
        .find(|c| c.get("uri").and_then(Value::as_str) == Some(uri))
        .ok_or_else(|| RpcError::app("not_found", format!("MCP 服务没有返回 {uri} 的内容")))?;
    let mime = item.get("mimeType").and_then(Value::as_str).unwrap_or("");
    if !mime
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("text/html")
    {
        return Err(RpcError::app(
            "invalid",
            format!("{uri} 不是界面资源（MIME 是「{mime}」，要 text/html;profile=mcp-app）"),
        ));
    }
    let html = if let Some(text) = item.get("text").and_then(Value::as_str) {
        text.to_string()
    } else if let Some(blob) = item.get("blob").and_then(Value::as_str) {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(blob)
            .map_err(|e| RpcError::app("invalid", format!("界面资源不是合法的 base64：{e}")))?;
        String::from_utf8(bytes).map_err(|_| RpcError::app("invalid", "界面资源不是 UTF-8 文本"))?
    } else {
        return Err(RpcError::app("invalid", format!("{uri} 没有内容")));
    };
    let ui = item.pointer("/_meta/ui");
    let field = |key: &str| ui.and_then(|u| u.get(key)).cloned().unwrap_or(Value::Null);
    Ok(json!({
        "uri": item.get("uri").cloned().unwrap_or_else(|| json!(uri)),
        "mimeType": item.get("mimeType").cloned().unwrap_or(Value::Null),
        "html": html,
        "csp": field("csp"),
        "permissions": field("permissions"),
        "domain": field("domain"),
        "prefersBorder": field("prefersBorder"),
    }))
}

async fn call(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let server = str_param(&params, "server")?.to_string();
    let tool = str_param(&params, "tool")?.to_string();
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    if !arguments.is_object() {
        return Err(RpcError::invalid_params("arguments 要是对象"));
    }
    let page = open_page(gateway, &params)?;
    mcp(gateway)?
        .call_from_app(&page.ctx, &server, &tool, arguments)
        .await
        .map_err(|e| RpcError::app("mcp", e))
}

async fn result(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let call_id = str_param(&params, "toolCallId")?.to_string();
    let Some(dir) = session_dir(gateway, &params)? else {
        return Ok(json!({ "call": null }));
    };
    let record = tokio::task::spawn_blocking(move || read_app_record(&dir, &call_id))
        .await
        .map_err(|e| RpcError::app("internal", format!("读调用记录的任务没跑完：{e}")))?;
    Ok(json!({ "call": record }))
}

fn context(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let server = str_param(&params, "server")?;
    let uri = str_param(&params, "resourceUri")?;
    let content = match params.get("content") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(blocks)) => blocks.clone(),
        Some(_) => return Err(RpcError::invalid_params("content 要是数组")),
    };
    let structured = match params.get("structuredContent") {
        None | Some(Value::Null) => None,
        Some(v @ Value::Object(_)) => Some(v.clone()),
        Some(_) => return Err(RpcError::invalid_params("structuredContent 要是对象")),
    };
    let page = open_page(gateway, &params)?;
    record_app_context(
        &page.ctx,
        &format!("{server} {uri}"),
        AppContextUpdate {
            label: format!("{server} {uri}"),
            content,
            structured,
        },
    );
    Ok(json!({}))
}

fn open_page(gateway: &GatewayHandle, params: &Value) -> Result<threads::Page, RpcError> {
    threads::resolve_param(gateway, params)
        .map_err(|_| RpcError::app("not_open", "这个会话没有开着：先打开会话再用界面里的操作"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_resource_prefers_matching_uri_and_reads_meta() {
        let read = json!({ "contents": [
            { "uri": "ui://other", "text": "<p>no</p>" },
            { "uri": "ui://board/app.html", "mimeType": "text/html;profile=mcp-app",
              "text": "<p>yes</p>",
              "_meta": { "ui": { "csp": { "connectDomains": ["https://api.x"] }, "prefersBorder": false } } }
        ] });
        let got = ui_resource("ui://board/app.html", &read).unwrap();
        assert_eq!(got["html"], "<p>yes</p>");
        assert_eq!(got["csp"]["connectDomains"][0], "https://api.x");
        assert_eq!(got["prefersBorder"], false);
        assert_eq!(got["permissions"], Value::Null);
        assert_eq!(got["mimeType"], "text/html;profile=mcp-app");
    }

    #[test]
    fn ui_resource_decodes_blob_and_rejects_empty() {
        let blob = base64::engine::general_purpose::STANDARD.encode("<b>hi</b>");
        let got = ui_resource(
            "ui://a",
            &json!({ "contents": [{ "uri": "ui://a", "mimeType": "text/html;profile=mcp-app", "blob": blob }] }),
        )
        .unwrap();
        assert_eq!(got["html"], "<b>hi</b>");
        assert!(ui_resource("ui://a", &json!({ "contents": [] })).is_err());
        assert!(ui_resource(
            "ui://a",
            &json!({ "contents": [{ "uri": "ui://a", "mimeType": "text/html" }] })
        )
        .is_err());
        // 没有同 uri 的那一项、MIME 不是 HTML：报错，不拿别的顶。
        let other =
            json!({ "contents": [{ "uri": "ui://other", "mimeType": "text/html", "text": "x" }] });
        assert!(ui_resource("ui://a", &other).is_err());
        let json_item = json!({ "contents": [{ "uri": "ui://a", "mimeType": "application/json", "text": "{}" }] });
        assert!(ui_resource("ui://a", &json_item).is_err());
    }
}
