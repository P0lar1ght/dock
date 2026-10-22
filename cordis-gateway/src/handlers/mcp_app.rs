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
//! - `mcp/app/read { server, uri }` → 规范的 `ReadResourceResult`（`{ contents }`）：界面读自己
//!   那台服务器的资源（视频、数据……，宿主声明了 `serverResources`）。不限 `ui://`，原样转回。
//! - `mcp/app/call { threadId?, server, tool, arguments? }` → `CallToolResult`：界面发起的
//!   `tools/call`。只认 `visibility` 含 `app` 的工具，不进对话流；会话身份是这个线程。
//! - `mcp/app/result { threadId?, toolCallId }` → `{ call }`：模型那次调用的原始结果
//!   `{ server, tool, resourceUri, arguments, result }`，没存就是 `null`。关着的会话也能读。
//! - `mcp/app/context { threadId?, server, resourceUri, content?, structuredContent? }` → `{}`：
//!   `ui/update-model-context`。按线程、按界面只留最新一份，下一步交给模型。
//!
//! - `mcp/app/view/expose { threadId?, viewId, connectionLeaseId?, server, resourceUri, tools }`
//!   → `{ tools: [publicName] }`：界面自己登记的工具（规范 `appCapabilities.tools`，桌面端
//!   `tools/list` 拿到的 `[{ name, description?, inputSchema? }]`）交给这个线程的模型，名字是
//!   `app_<server>__<name>`，经 `use_tool` 调。`viewId` 是桌面端给界面实例的 id；`tools` 空 =
//!   收回（界面关了）。连接断开时它报的一并收回（`connectionLeaseId` 是 `initialize` 回的）。
//! - 推送 `mcp/app/view/call { viewId, callId, name, arguments }`（只推受信连接）：模型调了
//!   一颗界面工具，桌面端交给 `viewId` 那个界面跑（`tools/call`），再用
//! - `mcp/app/view/result { callId, result?, error? }` → `{ delivered }` 送回：`result` 是规范的
//!   `CallToolResult`，`error` 是一句原因。界面 60 秒没回，这次调用按超时报给模型。
//!
//! `call` / `context` / `view/expose` 要线程开着（会话身份、页身份都从开着的页取）。

use base64::Engine;
use serde_json::{json, Value};

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{
    page_key, read_app_record, record_app_context, AppContextUpdate, Mcp, McpAppTool, McpAppViews,
    Sessions, ViewCall, ViewExpose, ViewTool, MCP, MCP_APP_VIEWS, MCP_APP_VIEW_CALL, SESSIONS,
};

use crate::handle::GatewayHandle;
use crate::handlers::canvas::session_dir;
use crate::methods::{method, register_methods, GatewayMethods, MethodPolicy, GATEWAY_METHODS};
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
                        protocol::MCP_APP_READ,
                        slow,
                        method(|gw, params| async move { read(&gw, params).await }),
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
                    (
                        protocol::MCP_APP_VIEW_EXPOSE,
                        trusted,
                        method(|gw, params| async move { view_expose(&gw, params) }),
                    ),
                    (
                        protocol::MCP_APP_VIEW_RESULT,
                        trusted,
                        method(|gw, params| async move { view_result(&gw, params) }),
                    ),
                ],
            )?;
            // 模型调了界面工具：推给受信连接，桌面端按 viewId 认是不是自己的界面。
            let live = ctx.clone();
            let listener = ctx.on(MCP_APP_VIEW_CALL, move |call: &ViewCall| {
                if let Some(table) = live.get::<GatewayMethods>(GATEWAY_METHODS) {
                    table.notify_trusted(
                        protocol::MCP_APP_VIEW_CALL,
                        json!({
                            "viewId": call.view_id,
                            "callId": call.call_id,
                            "name": call.name,
                            "arguments": call.arguments,
                        }),
                    );
                }
            })?;
            ctx.effect("gateway.mcpApps.viewCalls", |scope| {
                scope.own(listener);
                Ok(())
            })?;
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

/// 整份回复的上限（JSON 文本长度）。只防一帧太大占住这条 WebSocket（同连接的线程事件会排在
/// 它后面），不防 MCP 那边的内存：到这里时整份已经在内存里了。`blob` 是 base64，48 MB
/// 约合 36 MB 原始数据；更大的媒体以后走 HTTP 端点或分块。
const MAX_READ_BYTES: usize = 48 * 1024 * 1024;

/// 界面读自己那台服务器的资源。服务器由桌面端按界面实例定死（不信 iframe 传来的），
/// 网关只认受信连接；资源本来就是这台服务器对外给的，不再限 `ui://`。
async fn read(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let server = str_param(&params, "server")?;
    let uri = str_param(&params, "uri")?;
    let read = mcp(gateway)?
        .read_resource(server, uri)
        .await
        .map_err(|e| RpcError::app("mcp", e))?;
    capped(uri, read, MAX_READ_BYTES)
}

/// 原样转回，超过 `limit` 报 `too_large`。
fn capped(uri: &str, read: Value, limit: usize) -> Result<Value, RpcError> {
    let size = read.to_string().len();
    if size > limit {
        return Err(RpcError::app(
            "too_large",
            format!("资源 {uri} 有 {} KB，超过 {} KB", size / 1024, limit / 1024),
        ));
    }
    Ok(read)
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

fn views(gateway: &GatewayHandle) -> Result<std::sync::Arc<McpAppViews>, RpcError> {
    gateway
        .ctx()
        .get::<McpAppViews>(MCP_APP_VIEWS)
        .ok_or_else(|| RpcError::app("unavailable", "界面工具插件没有挂载"))
}

fn view_tools(params: &Value) -> Result<Vec<ViewTool>, RpcError> {
    let list = match params.get("tools") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(_) => return Err(RpcError::invalid_params("tools 要是数组")),
    };
    list.iter()
        .map(|t| {
            let name = t
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| RpcError::invalid_params("界面工具缺 name"))?;
            Ok(ViewTool {
                name: name.to_string(),
                description: t
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input_schema: t.get("inputSchema").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn view_expose(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let view_id = str_param(&params, "viewId")?.to_string();
    let server = str_param(&params, "server")?.to_string();
    let resource_uri = str_param(&params, "resourceUri")?.to_string();
    let tools = view_tools(&params)?;
    let page = open_page(gateway, &params)?;
    let sessions = page
        .ctx
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| RpcError::app("not_open", "这个会话没有开着"))?;
    let lease = params
        .get("connectionLeaseId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let names = views(gateway)?
        .expose(ViewExpose {
            view_id,
            page: page_key(&sessions),
            lease,
            server,
            resource_uri,
            tools,
        })
        .map_err(RpcError::invalid_params)?;
    Ok(json!({ "tools": names }))
}

fn view_result(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let call_id = str_param(&params, "callId")?;
    let outcome = match (params.get("result"), params.get("error")) {
        (Some(result @ Value::Object(_)), _) => Ok(result.clone()),
        (_, Some(Value::String(reason))) => Err(reason.clone()),
        _ => {
            return Err(RpcError::invalid_params(
                "要有 result（对象）或 error（字符串）",
            ))
        }
    };
    Ok(json!({ "delivered": views(gateway)?.resolve(call_id, outcome) }))
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

    /// 界面读非 ui:// 资源（官方 video-resource 的 videos://…）：base64 blob 原样转回；
    /// 超过上限报 too_large。
    #[test]
    fn read_passes_blobs_through_and_caps_size() {
        let read = json!({ "contents": [
            { "uri": "videos://clip", "mimeType": "video/mp4", "blob": "AAAAIGZ0eXBpc29t" }
        ] });
        assert_eq!(capped("videos://clip", read.clone(), 1024).unwrap(), read);
        let err = capped("videos://clip", read, 16).unwrap_err();
        assert_eq!(err.details_code, "too_large");
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
