//! 侧边聊天：从一个会话分叉出来的只读旁问页（同 TUI `/btw`），GUI 开在右侧面板里。
//!
//! - `thread/aside/start { threadId }`：给那个会话开（或回到）它的侧边聊天，回
//!   `{ thread, parentThreadId, existing }`。之后对 `thread.id` 用 `turn/start`、
//!   `thread/subscribe`、`thread/close` 等，和别的线程一样。
//! - `thread/aside/handback { threadId }`（侧边聊天的 id）：让它整理一段写进主线的
//!   笔记，回 `{ note }`。只起草，不写。
//! - `thread/aside/merge { threadId, note }`（主会话的 id）：把（用户确认 / 改过的）
//!   笔记写进主会话。在跑就下一个步骤边界落，闲着直接落；不开新的一轮。回
//!   `{ delivery: "nextStep" | "history" }`。
//!
//! 侧边聊天不落盘、不进 `thread/list`；主会话关掉时它一起关（`thread/close`）。

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{SessionRef, Tabs, SESSION_PORT, SIDE_NOTE_MAX_CHARS, TABS};
use serde_json::{json, Value};

use crate::handle::GatewayHandle;
use crate::methods::{method, register_methods, MethodPolicy, GATEWAY_METHODS};
use crate::protocol::{self, RpcError};
use crate::threads::{self, Page};

pub fn gateway_side_chat() -> Plugin {
    plugin(
        "gateway.side_chat",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            // 主会话是关着的落盘会话时先开页（侧边聊天要从它的上下文分叉）。
            let opens = MethodPolicy {
                opens_thread: true,
                ..MethodPolicy::default()
            };
            register_methods(
                ctx,
                vec![
                    (
                        protocol::THREAD_ASIDE_START,
                        opens,
                        method(|gw, params| async move { start(&gw, params).await }),
                    ),
                    (
                        // 要调一次模型：放到连接锁外跑。
                        protocol::THREAD_ASIDE_HANDBACK,
                        MethodPolicy::detached(),
                        method(|gw, params| async move { handback(&gw, params).await }),
                    ),
                    (
                        protocol::THREAD_ASIDE_MERGE,
                        opens,
                        method(|gw, params| async move { merge(&gw, params) }),
                    ),
                ],
            )?;
            Ok(None)
        },
    )
}

fn tabs(gateway: &GatewayHandle) -> Result<std::sync::Arc<Tabs>, RpcError> {
    gateway
        .ctx()
        .get::<Tabs>(TABS)
        .ok_or_else(|| RpcError::app("unavailable", "分页服务没有挂载，开不了侧边聊天"))
}

fn port(page: &Page) -> Result<std::sync::Arc<SessionRef>, RpcError> {
    page.ctx
        .get::<SessionRef>(SESSION_PORT)
        .ok_or_else(|| RpcError::app("unavailable", "session.port is not mounted"))
}

async fn start(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let parent = threads::resolve_param(gateway, &params)?;
    let parent_id = parent.thread_id();
    let tabs = tabs(gateway)?;
    let _opening = gateway.lock_opening().await;
    let (ctx, existing) = tabs
        .open_aside_for(&parent.session_id())
        .await
        .map_err(|e| RpcError::app("open_failed", e))?;
    let page = Page::of_ctx(ctx).ok_or_else(|| RpcError::app("open_failed", "侧边聊天没有会话"))?;
    if !existing {
        // 只投影从这里往后的对话：分叉带来的主线快照是给模型的参考，面板里不重放。
        gateway.begin_page(&page);
    }
    Ok(json!({
        "thread": crate::handlers::thread::open_summary(&page)?,
        "parentThreadId": parent_id,
        "existing": existing,
    }))
}

/// `threadId` 是开着的侧边聊天。
fn aside_page(gateway: &GatewayHandle, params: &Value) -> Result<Page, RpcError> {
    let page = threads::resolve_param(gateway, params)?;
    if !tabs(gateway)?.is_aside(&page.session_id()) {
        return Err(RpcError::invalid_params("threadId 不是侧边聊天"));
    }
    Ok(page)
}

async fn handback(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = aside_page(gateway, &params)?;
    if port(&page)?.working() {
        return Err(RpcError::app(
            "busy",
            "侧边聊天还在回答，等它答完再写进主线",
        ));
    }
    let note = cordis_spine::draft_side_note(&page.ctx)
        .await
        .map_err(|e| RpcError::app("draft_failed", e))?;
    Ok(json!({ "threadId": page.thread_id(), "note": note }))
}

fn merge(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    if tabs(gateway)?.is_aside(&page.session_id()) {
        return Err(RpcError::invalid_params(
            "threadId 要是主会话，不是侧边聊天",
        ));
    }
    let note = params
        .get("note")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if note.is_empty() {
        return Err(RpcError::invalid_params("笔记不能为空"));
    }
    if note.chars().count() > SIDE_NOTE_MAX_CHARS {
        return Err(RpcError::invalid_params(format!(
            "笔记最多 {SIDE_NOTE_MAX_CHARS} 字"
        )));
    }
    let port = port(&page)?;
    let delivery = if port.working() {
        "nextStep"
    } else {
        "history"
    };
    port.merge_side_note(note.to_string());
    Ok(json!({ "threadId": page.thread_id(), "delivery": delivery }))
}
