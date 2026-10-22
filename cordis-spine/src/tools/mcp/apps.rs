//! MCP Apps（`io.modelcontextprotocol/ui`）的客户端一侧。
//!
//! - 工具 `_meta.ui.resourceUri` 指向 `ui://` 界面资源；客户端（桌面端）拿
//!   [`Mcp::read_resource`] 读 HTML，放进沙箱 iframe。
//! - 挂了界面的工具被模型调用时，原始 `CallToolResult` 另存到
//!   `<会话目录>/mcp-apps/<调用 id>.json`（[`record`]）：工具结果文本装不下
//!   `structuredContent` / `_meta`，界面要的是原样那份。关着的会话也能重放。
//! - 界面自己发起的 `tools/call` 走 [`Mcp::call_from_app`]：只认同一服务器、
//!   `visibility` 含 `app` 的工具，不进对话流。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::protocol::{self, raw_tool_name};
use super::{call_params, is_local, Mcp};
use crate::names::{PLAN_MODE, SESSIONS};
use crate::session::log::Sessions;
use crate::tools::plan_mode::PlanMode;
use crate::tools::registry::exec_ctx;
use cordis_base::types::ToolCall;

/// 会话目录下存原始结果的子目录。
pub const RECORD_DIR: &str = "mcp-apps";
/// 单条记录上限：超了只留文本块（截图之类的大图不存）。
const RECORD_MAX_BYTES: usize = 4 * 1024 * 1024;

/// 一颗带界面或只给界面用的 MCP 工具。
#[derive(Clone, Debug, PartialEq)]
pub struct McpAppTool {
    pub server: String,
    /// 服务器上的原名。
    pub name: String,
    pub public_name: String,
    pub description: String,
    pub input_schema: Value,
    pub resource_uri: Option<String>,
    pub model_visible: bool,
    pub app_visible: bool,
}

/// 模型调用挂界面工具时，记录要知道的那一份。
#[derive(Clone, Debug)]
pub(super) struct Binding {
    server: String,
    tool: String,
    resource_uri: String,
}

impl Binding {
    pub(super) fn new(server: &str, public: &str, resource_uri: String) -> Self {
        Self {
            server: server.to_string(),
            tool: raw_tool_name(public),
            resource_uri,
        }
    }
}

/// 把一次挂界面工具调用的原始结果存进调用方那一页的会话目录。
/// 不落盘的页（侧边聊天）没有目录，就不存——界面只能拿到文本。
pub(super) async fn record(app: &Binding, call: &ToolCall, reply: &Value) {
    let Some(dir) = exec_ctx().as_ref().and_then(page_session_dir) else {
        return;
    };
    let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(json!({}));
    let body = json!({
        "server": app.server,
        "tool": app.tool,
        "resourceUri": app.resource_uri,
        "arguments": arguments,
        "result": call_tool_result(reply),
    });
    let id = call.id.clone();
    let _ = tokio::task::spawn_blocking(move || write_record(&dir, &id, body)).await;
}

/// 读回一条记录：`{ server, tool, resourceUri, arguments, result }`。
pub fn read_record(session_dir: &Path, call_id: &str) -> Option<Value> {
    let path = record_path(session_dir, call_id)?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// JSON-RPC 回复 → 规范的 `CallToolResult`。协议层出错的也收成 `isError`。
fn call_tool_result(reply: &Value) -> Value {
    if let Some(result) = reply.get("result").filter(|r| r.is_object()) {
        return result.clone();
    }
    let text = reply
        .get("error")
        .map(|e| {
            e.get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| e.to_string())
        })
        .unwrap_or_else(|| "MCP 服务没有返回结果".into());
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

fn write_record(dir: &Path, call_id: &str, mut body: Value) {
    let Some(path) = record_path(dir, call_id) else {
        return;
    };
    let mut text = body.to_string();
    if text.len() > RECORD_MAX_BYTES {
        if let Some(content) = body
            .pointer_mut("/result/content")
            .and_then(Value::as_array_mut)
        {
            content.retain(|block| block.get("type").and_then(Value::as_str) == Some("text"));
        }
        if let Some(result) = body.get_mut("result").and_then(Value::as_object_mut) {
            result.remove("structuredContent");
            result.insert("dock/truncated".into(), json!(true));
        }
        text = body.to_string();
        if text.len() > RECORD_MAX_BYTES {
            return;
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, text);
}

/// 调用 id 来自模型，只留安全字符当文件名；换过字符的再接原 id 的哈希，`call/1` 和
/// `call_1` 不会落到同一个文件。空的、太长的不存。
fn record_path(dir: &Path, call_id: &str) -> Option<PathBuf> {
    let safe: String = call_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() || safe.len() > 128 {
        return None;
    }
    let name = if safe == call_id {
        safe
    } else {
        format!("{safe}.{:016x}", fnv1a(call_id.as_bytes()))
    };
    Some(dir.join(RECORD_DIR).join(format!("{name}.json")))
}

/// 64 位 FNV-1a：只用来区分文件名，跨版本稳定（不用 `DefaultHasher`）。
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// 调用者那一页的会话目录。子代理经 `"planMode"` 找到父页（同画布）。
fn page_session_dir(ctx: &cordis::Context) -> Option<PathBuf> {
    let sessions = ctx
        .get::<PlanMode>(PLAN_MODE)
        .and_then(|plan| plan.page_sessions())
        .or_else(|| ctx.get::<Sessions>(SESSIONS))?;
    sessions.disk_session_dir()
}

impl Mcp {
    /// 所有连着的服务器上，带界面（`resourceUri`）或只给界面用的工具。
    pub fn app_tools(&self) -> Vec<McpAppTool> {
        let inner = self.inner.lock().unwrap();
        inner
            .slots
            .iter()
            .filter(|s| s.config.enabled && s.rpc.is_some())
            .flat_map(|slot| {
                slot.listed
                    .iter()
                    // 模型看得见的：挂了界面才列；模型看不见的：界面能用才列（两边都不可见的不列）。
                    .filter(|t| {
                        if t.ui.model_visible() {
                            t.ui.resource_uri.is_some()
                        } else {
                            t.ui.app_visible()
                        }
                    })
                    .map(|t| app_tool(&slot.config.name, t))
            })
            .collect()
    }

    /// 读一个服务器的资源（`resources/read`），回规范的 `ReadResourceResult`
    /// （`{ contents: [{ uri, mimeType, text | blob, _meta }] }`）。
    pub async fn read_resource(&self, server: &str, uri: &str) -> Result<Value, String> {
        let rpc = self.connected_rpc(server)?;
        let reply = rpc("resources/read".into(), json!({ "uri": uri })).await?;
        reply_result(reply)
    }

    /// 界面发起的 `tools/call`：只认 `server` 自己的、`visibility` 含 `app` 的工具；
    /// 用户在 `/mcps` 停掉的、要过权限门的不给调。会话身份、工作目录按 `page` 算。
    /// 回规范的 `CallToolResult`，不进对话流。
    pub async fn call_from_app(
        &self,
        page: &cordis::Context,
        server: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let public = protocol::public_tool_name(server, tool);
        let (rpc, local) = {
            let inner = self.inner.lock().unwrap();
            let slot = inner
                .slots
                .iter()
                .find(|s| s.config.name == server)
                .ok_or_else(|| format!("没有配置 MCP 服务 {server}"))?;
            let listed = slot
                .listed
                .iter()
                .find(|t| t.public == public)
                .ok_or_else(|| format!("MCP 服务 {server} 没有工具 {tool}"))?;
            if !listed.ui.app_visible() {
                return Err(format!("{tool} 只给模型用，界面不能调"));
            }
            if slot.disabled_tools.contains(tool) {
                return Err(format!("{tool} 已在 MCP 设置里停用"));
            }
            if cordis_base::acp::needs_permission(&public) {
                return Err(format!("{tool} 要过权限确认，界面不能直接调"));
            }
            let rpc = slot
                .rpc
                .clone()
                .ok_or_else(|| format!("MCP 服务 {server} 没有连上"))?;
            (rpc, is_local(&slot.config))
        };
        let arguments = arguments.to_string();
        let reply = crate::tools::registry::with_exec_ctx_async(page.clone(), async move {
            let params = call_params(&public, &arguments, local);
            rpc("tools/call".into(), params).await
        })
        .await?;
        if let Some(err) = reply.get("error") {
            return Err(error_text(err));
        }
        Ok(call_tool_result(&reply))
    }

    fn connected_rpc(&self, server: &str) -> Result<super::RpcFn, String> {
        let inner = self.inner.lock().unwrap();
        let slot = inner
            .slots
            .iter()
            .find(|s| s.config.name == server)
            .ok_or_else(|| format!("没有配置 MCP 服务 {server}"))?;
        if !slot.config.enabled {
            return Err(format!("MCP 服务 {server} 已停用"));
        }
        slot.rpc
            .clone()
            .ok_or_else(|| format!("MCP 服务 {server} 没有连上"))
    }
}

fn app_tool(server: &str, t: &protocol::ListedTool) -> McpAppTool {
    McpAppTool {
        server: server.to_string(),
        name: raw_tool_name(&t.public),
        public_name: t.public.clone(),
        description: t.description.clone(),
        input_schema: serde_json::from_str(&t.schema).unwrap_or(json!({ "type": "object" })),
        resource_uri: t.ui.resource_uri.clone(),
        model_visible: t.ui.model_visible(),
        app_visible: t.ui.app_visible(),
    }
}

fn reply_result(reply: Value) -> Result<Value, String> {
    if let Some(err) = reply.get("error") {
        return Err(error_text(err));
    }
    reply
        .get("result")
        .cloned()
        .ok_or_else(|| "MCP 服务没有返回结果".into())
}

fn error_text(err: &Value) -> String {
    err.get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 换过字符的调用 id 带哈希：`call/1`、`call?1`、`call_1` 各是各的文件。
    #[test]
    fn record_paths_do_not_collide() {
        let dir = Path::new("/s");
        let plain = record_path(dir, "call_1").unwrap();
        let slash = record_path(dir, "call/1").unwrap();
        let query = record_path(dir, "call?1").unwrap();
        assert_eq!(plain, dir.join(RECORD_DIR).join("call_1.json"));
        assert_ne!(plain, slash);
        assert_ne!(slash, query);
        assert!(
            slash.starts_with(dir.join(RECORD_DIR)),
            "不能跑出目录：{slash:?}"
        );
        assert!(record_path(dir, "").is_none());
        assert!(record_path(dir, &"x".repeat(200)).is_none());
    }
}
