//! `browser/view/*`：看某个会话正在用的浏览器标签页，并能接手操作。
//!
//! - `browser/view/open { threadId?, quality?, maxWidth?, maxHeight? }` → `{ viewId, threadId,
//!   targetId, url, title }`。之后这条连接收 `browser/view/frame` / `status` / `closed` 推送。
//! - `browser/view/input { viewId, event }`：用户的鼠标、滚轮、按键、文字（见
//!   [`cordis_browser::view::View::input`]）。
//! - `browser/view/navigate { viewId, url? | action? }`：地址栏 / back / forward / reload。
//! - `browser/view/close { viewId }`；连接断开时自动全关。
//!
//! 标签页来自浏览器 MCP 写的运行时名册（`cordis_browser::registry`）：按页的会话身份
//! （[`cordis_spine::mcp_session_key`]，和 MCP 客户端带给服务端的是同一个）找到这个
//! 会话的活动标签页。agent 切了标签页，画面跟着切；会话的标签页都关了，推 `closed`。
//!
//! 视图是**连接级**的：不进会话、不落盘，只活在这条 WebSocket 上。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cordis_browser::registry;
use cordis_browser::view::{Frames, PageInfo, ScreencastOptions, View};
use cordis_spine::mcp_session_key;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::handle::GatewayHandle;
use crate::protocol::{self, RpcError};
use crate::threads;
use crate::ws::{OutTx, Outgoing};

/// 多久看一次名册（agent 换没换标签页）和页面地址标题。
const FOLLOW_EVERY: Duration = Duration::from_millis(700);

/// 一条连接上开着的视图。丢掉（连接断开）时中止全部推送任务，CDP 连接随 View 一起断。
#[derive(Default)]
pub(crate) struct BrowserViews {
    open: Mutex<HashMap<String, ViewEntry>>,
}

struct ViewEntry {
    thread_id: String,
    current: Current,
    task: JoinHandle<()>,
}

type Current = Arc<Mutex<Option<Arc<View>>>>;

impl Drop for BrowserViews {
    fn drop(&mut self) {
        for (_, entry) in self.open.lock().unwrap().drain() {
            entry.task.abort();
        }
    }
}

impl BrowserViews {
    fn view(&self, params: &Value) -> Result<Arc<View>, RpcError> {
        let id = view_id(params)?;
        let current = self
            .open
            .lock()
            .unwrap()
            .get(id)
            .map(|entry| entry.current.clone())
            .ok_or_else(|| RpcError::app("not_found", format!("没有视图 {id}")))?;
        let view = current.lock().unwrap().clone();
        view.ok_or_else(|| RpcError::app("browser_unavailable", "画面正在切换标签页，稍后再试"))
    }

    fn remove(&self, id: &str) -> Option<ViewEntry> {
        self.open.lock().unwrap().remove(id)
    }
}

pub(crate) fn is_browser_view(method: &str) -> bool {
    matches!(
        method,
        protocol::BROWSER_VIEW_OPEN
            | protocol::BROWSER_VIEW_INPUT
            | protocol::BROWSER_VIEW_NAVIGATE
            | protocol::BROWSER_VIEW_CLOSE
    )
}

pub(crate) async fn dispatch(
    gateway: &GatewayHandle,
    views: &Arc<BrowserViews>,
    out: &OutTx,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    match method {
        protocol::BROWSER_VIEW_OPEN => open(gateway, views, out, params).await,
        protocol::BROWSER_VIEW_INPUT => {
            let event = params
                .get("event")
                .filter(|e| e.is_object())
                .ok_or_else(|| RpcError::invalid_params("event is required"))?;
            views
                .view(&params)?
                .input(event)
                .await
                .map_err(RpcError::invalid_params)?;
            Ok(json!({ "ok": true }))
        }
        protocol::BROWSER_VIEW_NAVIGATE => {
            views
                .view(&params)?
                .navigate(&params)
                .await
                .map_err(RpcError::invalid_params)?;
            Ok(json!({ "ok": true }))
        }
        protocol::BROWSER_VIEW_CLOSE => {
            let id = view_id(&params)?;
            let entry = views
                .remove(id)
                .ok_or_else(|| RpcError::app("not_found", format!("没有视图 {id}")))?;
            entry.task.abort();
            let view = entry.current.lock().unwrap().take();
            if let Some(view) = view.and_then(|v| Arc::try_unwrap(v).ok()) {
                view.close().await;
            }
            Ok(json!({ "ok": true, "viewId": id }))
        }
        _ => Err(RpcError::method_not_found(method)),
    }
}

async fn open(
    gateway: &GatewayHandle,
    views: &Arc<BrowserViews>,
    out: &OutTx,
    params: Value,
) -> Result<Value, RpcError> {
    let thread_id = threads::thread_param(&params);
    let page = threads::resolve(gateway, &thread_id)?;
    let key = mcp_session_key(&*page.sessions()?);
    let target = active_target(&key).ok_or_else(|| {
        RpcError::app(
            "no_tab",
            "这个会话还没有打开浏览器标签页（agent 先 browser_open）",
        )
    })?;
    let opts = options(&params);
    let view = View::attach(&target)
        .await
        .map_err(|e| RpcError::app("browser_unavailable", e))?;
    let info = view.info().await;
    let frames = view
        .screencast(opts)
        .await
        .map_err(|e| RpcError::app("browser_unavailable", e))?;

    // 同一条连接对同一个会话只留一个视图：再开就把旧的关掉。
    let stale: Vec<ViewEntry> = {
        let mut open = views.open.lock().unwrap();
        let ids: Vec<String> = open
            .iter()
            .filter(|(_, e)| e.thread_id == thread_id)
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter().filter_map(|id| open.remove(&id)).collect()
    };
    for entry in stale {
        entry.task.abort();
    }

    let view_id = uuid::Uuid::new_v4().simple().to_string();
    let current: Current = Arc::new(Mutex::new(Some(Arc::new(view))));
    let pump = Pump {
        view_id: view_id.clone(),
        thread_id: thread_id.clone(),
        key,
        target: target.clone(),
        info: info.clone(),
        opts,
        current: current.clone(),
        out: out.clone(),
        views: Arc::downgrade(views),
    };
    let task = tokio::spawn(pump.run(frames));
    views.open.lock().unwrap().insert(
        view_id.clone(),
        ViewEntry {
            thread_id: thread_id.clone(),
            current,
            task,
        },
    );
    Ok(json!({
        "viewId": view_id,
        "threadId": thread_id,
        "targetId": target,
        "url": info.url,
        "title": info.title,
    }))
}

/// 会话的活动标签页；名册里没记活动的就取第一个。
fn active_target(key: &str) -> Option<String> {
    let tabs = registry::lookup(key)?;
    tabs.active.or_else(|| tabs.targets.first().cloned())
}

fn options(params: &Value) -> ScreencastOptions {
    let mut opts = ScreencastOptions::default();
    let int = |key: &str| params.get(key).and_then(Value::as_i64);
    if let Some(q) = int("quality") {
        opts.quality = q;
    }
    if let Some(w) = int("maxWidth") {
        opts.max_width = w;
    }
    if let Some(h) = int("maxHeight") {
        opts.max_height = h;
    }
    opts
}

fn view_id(params: &Value) -> Result<&str, RpcError> {
    params
        .get("viewId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("viewId is required"))
}

/// 一个视图的推送任务：转发帧（写出去才 ack）、跟着 agent 换标签页、报地址标题变化。
struct Pump {
    view_id: String,
    thread_id: String,
    key: String,
    target: String,
    info: PageInfo,
    opts: ScreencastOptions,
    current: Current,
    out: OutTx,
    views: std::sync::Weak<BrowserViews>,
}

enum Step {
    Frame(Option<cordis_browser::view::Frame>),
    Tick,
}

impl Pump {
    async fn run(mut self, frames: Frames) {
        let reason = self.pump(frames).await;
        if let Some(reason) = reason {
            self.send(json!({
                "method": protocol::BROWSER_VIEW_CLOSED,
                "params": { "viewId": self.view_id, "threadId": self.thread_id, "reason": reason }
            }));
        }
        if let Some(views) = self.views.upgrade() {
            views.remove(&self.view_id);
        }
    }

    /// 返回结束原因；`None` 是连接没了（没人可告诉）。
    async fn pump(&mut self, mut frames: Frames) -> Option<&'static str> {
        let mut tick = tokio::time::interval(FOLLOW_EVERY);
        let mut seq: u64 = 0;
        let mut frames_open = true;
        loop {
            let step = if frames_open {
                tokio::select! {
                    frame = frames.next() => Step::Frame(frame),
                    _ = tick.tick() => Step::Tick,
                }
            } else {
                tick.tick().await;
                Step::Tick
            };
            match step {
                Step::Frame(Some(frame)) => {
                    seq += 1;
                    let (written, done) = oneshot::channel();
                    let note = json!({
                        "method": protocol::BROWSER_VIEW_FRAME,
                        "params": {
                            "viewId": self.view_id,
                            "threadId": self.thread_id,
                            "targetId": self.target,
                            "seq": seq,
                            "data": frame.data,
                            "mime": "image/jpeg",
                            "width": frame.device_width,
                            "height": frame.device_height,
                            "scaleFactor": frame.page_scale_factor,
                            "offsetTop": frame.offset_top,
                            "scrollX": frame.scroll_x,
                            "scrollY": frame.scroll_y,
                        }
                    });
                    let sent = self.out.send(Outgoing::Frame {
                        text: note.to_string(),
                        written,
                    });
                    // 写出去了才让 Chrome 发下一帧；连接没了就收工。
                    if sent.is_err() || done.await.is_err() {
                        return None;
                    }
                    let view = self.current.lock().unwrap().clone();
                    if let Some(view) = view {
                        view.ack(frame.ack_id).await;
                    }
                }
                Step::Frame(None) => frames_open = false,
                Step::Tick => {
                    let Some(target) = active_target(&self.key) else {
                        return Some("no_tab");
                    };
                    if target != self.target || !frames_open {
                        match self.switch_to(&target).await {
                            Ok(next) => {
                                frames = next;
                                frames_open = true;
                            }
                            Err(_) => return Some("browser_exited"),
                        }
                        continue;
                    }
                    let view = self.current.lock().unwrap().clone();
                    let Some(view) = view else { continue };
                    if !view.is_alive() {
                        return Some("browser_exited");
                    }
                    let info = view.info().await;
                    if info != self.info {
                        self.info = info;
                        self.status();
                    }
                }
            }
            if self.out.is_closed() {
                return None;
            }
        }
    }

    /// agent 换了标签页（或画面流断了）：挂到新的那页上重新推。
    async fn switch_to(&mut self, target: &str) -> Result<Frames, String> {
        let old = self.current.lock().unwrap().take();
        drop(old);
        let view = View::attach(target).await?;
        let frames = view.screencast(self.opts).await?;
        self.info = view.info().await;
        self.target = target.to_string();
        *self.current.lock().unwrap() = Some(Arc::new(view));
        self.status();
        Ok(frames)
    }

    fn status(&self) {
        self.send(json!({
            "method": protocol::BROWSER_VIEW_STATUS,
            "params": {
                "viewId": self.view_id,
                "threadId": self.thread_id,
                "targetId": self.target,
                "url": self.info.url,
                "title": self.info.title,
            }
        }));
    }

    fn send(&self, note: Value) {
        let _ = self.out.send(Outgoing::Text(note.to_string()));
    }
}
