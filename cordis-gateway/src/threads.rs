//! 线程 = 一页。`dock.1` 里的 `threadId` 是**落盘会话 id**；`live`（或省略）是
//! 第 1 页（根 ctx）的别名，老客户端照旧能用。分页身份 `main#N` 只在网关内部用
//! 来路由事件（[`cordis_spine::PageLogEvent`] 带的就是它）。
//!
//! 开着的页来自 `"tui.tabs"`；没挂分页服务（测试装配）时只有第 1 页。

use std::sync::Arc;

use cordis::Context;
use cordis_spine::{Sessions, ROOT_IDENTITY, SESSIONS};
use cordis_tui::{Tabs, TUI_TABS};
use serde_json::Value;

use crate::handle::GatewayHandle;
use crate::protocol::{RpcError, LIVE_THREAD_ID};

/// 一页：它的 ctx 与分页身份（`main` / `main#N`）。
#[derive(Clone)]
pub struct Page {
    pub ctx: Context,
    pub identity: String,
}

impl Page {
    fn of(ctx: Context) -> Option<Self> {
        let identity = ctx.get::<Sessions>(SESSIONS)?.identity().to_string();
        Some(Self { ctx, identity })
    }

    pub fn is_root(&self) -> bool {
        self.identity == ROOT_IDENTITY
    }

    pub fn sessions(&self) -> Result<Arc<Sessions>, RpcError> {
        self.ctx
            .get::<Sessions>(SESSIONS)
            .ok_or_else(|| RpcError::app("unavailable", "sessions service is not mounted"))
    }

    /// 这一页在协议里默认的 `threadId`：第 1 页是 `live`，其它页是它正在写的
    /// 落盘会话 id。
    pub fn thread_id(&self) -> String {
        if self.is_root() {
            return LIVE_THREAD_ID.into();
        }
        self.ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| s.live_session_id())
            .unwrap_or_default()
    }

    /// `ctx` 那一页在协议里的默认 `threadId`（见 [`Self::thread_id`]）。
    pub fn thread_id_of(ctx: &Context) -> String {
        Self::of(ctx.clone())
            .map(|page| page.thread_id())
            .unwrap_or_else(|| LIVE_THREAD_ID.into())
    }

    /// 这一页正在写的落盘会话 id（第 1 页也有）。
    pub fn session_id(&self) -> String {
        self.ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| s.live_session_id())
            .unwrap_or_default()
    }
}

/// 所有开着的页，第 1 页在最前。
pub fn open_pages(gateway: &GatewayHandle) -> Vec<Page> {
    let root = gateway.ctx().clone();
    let contexts = root
        .get::<Tabs>(TUI_TABS)
        .map(|tabs| tabs.contexts())
        .unwrap_or_else(|| vec![root.clone()]);
    contexts.into_iter().filter_map(Page::of).collect()
}

/// 分页身份 → 页。
pub fn page_by_identity(gateway: &GatewayHandle, identity: &str) -> Option<Page> {
    open_pages(gateway)
        .into_iter()
        .find(|page| page.identity == identity)
}

/// 参数里的 `threadId`，省略就是 `live`。
pub fn thread_param(params: &Value) -> String {
    params
        .get("threadId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(LIVE_THREAD_ID)
        .to_string()
}

/// `threadId` → 开着的那一页。`live` 是第 1 页；其它按落盘会话 id 找。
pub fn resolve(gateway: &GatewayHandle, thread_id: &str) -> Result<Page, RpcError> {
    let pages = open_pages(gateway);
    let found = if thread_id == LIVE_THREAD_ID {
        pages.into_iter().find(Page::is_root)
    } else {
        pages
            .into_iter()
            .find(|page| page.session_id() == thread_id)
    };
    found.ok_or_else(|| RpcError::app("thread_not_open", format!("线程 {thread_id} 没有开着")))
}

/// [`resolve`] 参数里的 `threadId`。
pub fn resolve_param(gateway: &GatewayHandle, params: &Value) -> Result<Page, RpcError> {
    resolve(gateway, &thread_param(params))
}
