//! `browser/view/*`：看某个会话正在用的浏览器标签页，并能接手操作。
//!
//! - `browser/view/open { threadId?, url?, viewport?, quality?, maxWidth?, maxHeight? }` →
//!   `{ viewId, threadId, targetId, url, title, tabs }`。之后这条连接收 `browser/view/frame` /
//!   `status` / `closed` 推送。会话还没有标签页时带 `url` 就替它开一页（经浏览器 MCP，
//!   记在这个会话名下，agent 接着能用）；`viewport { width, height, deviceScaleFactor? }`
//!   让页面视口跟着面板走。
//! - `browser/view/resize { viewId, width, height, deviceScaleFactor? }`：面板大小变了。
//! - `browser/view/input { viewId, event }`：用户的鼠标、滚轮、按键、文字（见
//!   [`cordis_browser::view::View::input`]）。
//! - `browser/view/navigate { viewId, url? | action? }`：地址栏 / back / forward / reload。
//! - `browser/view/tab { viewId, action: "switch"|"close"|"new", targetId?, url? }`：标签栏。
//!   经浏览器 MCP 的 `browser_tabs`（以这页的身份）：用户和 agent 共用一个浏览器，面板切到
//!   哪页，agent 接着就在哪页操作。
//! - `browser/view/close { viewId }`；连接断开时自动全关。
//!
//! 标签栏：`tabs` = `[{ targetId, url, title, active }]`，按名册里的顺序（和 `browser_tabs`
//! 的序号一致）。变了推 `browser/view/tabs`。页面自己开的新页（`target=_blank`、
//! `window.open`）要浏览器 MCP 收进会话（见 `ConnectedSession::sync_targets`）才进名册：
//! agent 没在调用时，这里看到有没收的就以这页的身份调一次 `browser_tabs` 催它收。
//!
//! 标签页来自浏览器 MCP 写的运行时名册（`cordis_browser::registry`）：按页的会话身份
//! （[`cordis_spine::mcp_session_key`]，和 MCP 客户端带给服务端的是同一个）找到这个
//! 会话的活动标签页。agent 切了标签页，画面跟着切；会话的标签页都关了，推 `closed`。
//!
//! 视口经浏览器 MCP 的 `browser_resize` 改（[`cordis_spine::Mcp::call_as`]），设在 MCP
//! 自己那条 CDP 连接上：agent 和面板看到的始终是同一页、同一尺寸，面板关了也不回弹。
//! agent 整页截图会清掉视口覆盖，换标签页时新页也没有：画面尺寸和期望对不上就再设一次。
//! 设备像素比不改页面（agent 的截图不变）：只在补静止帧时按它的倍数截（[`View::still`]）。
//!
//! 视图是**连接级**的：不进会话、不落盘，只活在这条 WebSocket 上。
//!
//! 网关每个请求各开任务跑，这里有两处要按**收到的顺序**来（见 [`BrowserViews::order`]，
//! 在读循环里同步定序）：
//! - 同一会话连发 `open`：最后收到的那个留下。先完成的早已回了 `viewId`，被顶掉时推
//!   `closed { reason: "replaced" }`；晚完成的发现自己不是最新，回 `superseded`。
//! - 同一视图的 `input`：一个做完下一个才发给页面，按下 / 抬起、连打的字不会乱序。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cordis::Context;
use cordis_base::config::BROWSER_MCP_SERVER;
use cordis_browser::registry;
use cordis_browser::view::{Frames, PageInfo, ScreencastOptions, View};
use cordis_spine::{mcp_session_key, Mcp, MCP};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Notify};
use tokio::task::JoinHandle;

use crate::handle::GatewayHandle;
use crate::protocol::{self, RpcError};
use crate::threads;
use crate::ws::{OutTx, Outgoing};

/// 多久看一次名册（agent 换没换标签页）和页面地址标题。
const FOLLOW_EVERY: Duration = Duration::from_millis(400);
/// 画面尺寸和期望视口对不上时，最多隔这么久重设一次（agent 整页截图会清掉覆盖）。
const HEAL_EVERY: Duration = Duration::from_millis(1500);
/// 替会话开了页之后，等 MCP 进程把名册写盘的上限。
const REGISTRY_WAIT: Duration = Duration::from_secs(3);
/// 带了 `viewport` 时流里的帧不再额外缩小（无头 screencast 本来就只出 CSS 像素）。
const NATIVE_MAX: i64 = 4096;
/// 画面停下来多久补一张静止帧（[`View::still`]）：无头 screencast 只出 CSS 像素、页面不动时
/// 一帧不推（刚挂上也不推），补这一张静止页面才有画面、Retina 上才清楚。
const SETTLE: Duration = Duration::from_millis(300);
/// 浏览器 MCP 对没开过页的会话回的错（`cordis_browser` 的 `NEED_OPEN`）。
const NO_SESSION_TAB: &str = "has no browser tab yet";
/// 看到页面自己开的新页还没进名册时，最多隔这么久催一次浏览器 MCP 收下。
const ADOPT_EVERY: Duration = Duration::from_millis(1000);
/// 静止帧的 JPEG 质量（它要清楚，比流里的帧高一点）。
const STILL_QUALITY: i64 = 85;

/// 一条连接上开着的视图。丢掉（连接断开）时中止全部推送任务，CDP 连接随 View 一起断。
#[derive(Default)]
pub(crate) struct BrowserViews {
    open: Mutex<HashMap<String, ViewEntry>>,
    /// 每个会话最后收到的那次 `open` 的序号。
    latest_open: Mutex<HashMap<String, u64>>,
    next_open: AtomicU64,
    /// 每个视图最后一个 `input` 的「做完了」信号，下一个先等它。
    last_input: Mutex<HashMap<String, oneshot::Receiver<()>>>,
}

/// 请求在读循环里定下的先后（见 [`BrowserViews::order`]）。
pub(crate) enum Order {
    None,
    Open(u64),
    Input {
        prev: Option<oneshot::Receiver<()>>,
        /// 这一个做完（或失败、被丢掉）时随 drop 通知下一个。
        _done: oneshot::Sender<()>,
    },
}

struct ViewEntry {
    thread_id: String,
    /// 会话身份（[`mcp_session_key`]）：在名册里找它的标签页。
    key: String,
    current: Current,
    task: JoinHandle<()>,
    /// 这页的 ctx：以它的身份调浏览器 MCP（改视口）。
    page: Context,
    wanted: Wanted,
    /// 改完视口叫推送任务补一张静止帧：从别的连接改视口，screencast 不一定出新帧。
    kick: Arc<Notify>,
}

type Current = Arc<Mutex<Option<Arc<View>>>>;
/// 面板要的视口；`None` = 没要求（老客户端），不碰页面尺寸。
type Wanted = Arc<Mutex<Option<Viewport>>>;

/// 页面视口：CSS 像素 + 设备像素比。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Viewport {
    width: i64,
    height: i64,
    scale: f64,
}

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

    fn entry_parts(&self, id: &str) -> Result<(Context, Wanted, Arc<Notify>), RpcError> {
        self.open
            .lock()
            .unwrap()
            .get(id)
            .map(|e| (e.page.clone(), e.wanted.clone(), e.kick.clone()))
            .ok_or_else(|| RpcError::app("not_found", format!("没有视图 {id}")))
    }

    fn remove(&self, id: &str) -> Option<ViewEntry> {
        self.last_input.lock().unwrap().remove(id);
        self.open.lock().unwrap().remove(id)
    }

    /// 在连接的读循环里、按收到的顺序同步调用（不能挪进各自的任务里：任务开跑的先后
    /// 不保证）。`open` 记下序号，`input` 排到同一视图上一个输入后面。
    pub(crate) fn order(&self, method: &str, params: &Value) -> Order {
        match method {
            protocol::BROWSER_VIEW_OPEN => {
                let seq = self.next_open.fetch_add(1, Ordering::Relaxed) + 1;
                self.latest_open
                    .lock()
                    .unwrap()
                    .insert(threads::thread_param(params), seq);
                Order::Open(seq)
            }
            // 改视口、输入、标签栏排同一条队：连拖几下分隔线，最后那次一定最后生效；
            // 点了新标签页紧接着敲的字，落在新页上。
            protocol::BROWSER_VIEW_INPUT
            | protocol::BROWSER_VIEW_RESIZE
            | protocol::BROWSER_VIEW_TAB => {
                let Ok(id) = view_id(params) else {
                    return Order::None;
                };
                let (done, next) = oneshot::channel();
                let prev = self.last_input.lock().unwrap().insert(id.to_string(), next);
                Order::Input { prev, _done: done }
            }
            _ => Order::None,
        }
    }
}

pub(crate) fn is_browser_view(method: &str) -> bool {
    matches!(
        method,
        protocol::BROWSER_VIEW_OPEN
            | protocol::BROWSER_VIEW_INPUT
            | protocol::BROWSER_VIEW_NAVIGATE
            | protocol::BROWSER_VIEW_CLOSE
            | protocol::BROWSER_VIEW_RESIZE
            | protocol::BROWSER_VIEW_TAB
    )
}

pub(crate) async fn dispatch(
    gateway: &GatewayHandle,
    views: &Arc<BrowserViews>,
    out: &OutTx,
    method: &str,
    params: Value,
    order: Order,
) -> Result<Value, RpcError> {
    match method {
        protocol::BROWSER_VIEW_OPEN => {
            let seq = match order {
                Order::Open(seq) => seq,
                _ => 0,
            };
            open(gateway, views, out, params, seq).await
        }
        protocol::BROWSER_VIEW_INPUT => {
            // 等上一个输入做完；`order` 活到这个分支结束，drop 时放下一个。
            let _order = match order {
                Order::Input { prev, _done } => {
                    if let Some(prev) = prev {
                        let _ = prev.await;
                    }
                    Some(_done)
                }
                _ => None,
            };
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
        protocol::BROWSER_VIEW_RESIZE => {
            let _order = match order {
                Order::Input { prev, _done } => {
                    if let Some(prev) = prev {
                        let _ = prev.await;
                    }
                    Some(_done)
                }
                _ => None,
            };
            let id = view_id(&params)?;
            let viewport = parse_viewport(&params)?
                .ok_or_else(|| RpcError::invalid_params("width and height are required"))?;
            let (page, wanted, kick) = views.entry_parts(id)?;
            *wanted.lock().unwrap() = Some(viewport);
            apply_viewport(&page, viewport)
                .await
                .map_err(|e| RpcError::app("browser_unavailable", e))?;
            kick.notify_one();
            Ok(json!({ "ok": true }))
        }
        protocol::BROWSER_VIEW_TAB => {
            let _order = match order {
                Order::Input { prev, _done } => {
                    if let Some(prev) = prev {
                        let _ = prev.await;
                    }
                    Some(_done)
                }
                _ => None,
            };
            tab(views, &params).await
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
    seq: u64,
) -> Result<Value, RpcError> {
    let thread_id = threads::thread_param(&params);
    let page = threads::resolve(gateway, &thread_id)?;
    let key = mcp_session_key(&*page.sessions()?);
    let viewport = parse_viewport(params.get("viewport").unwrap_or(&Value::Null))?;
    let url = params
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (target, view) = attach_session_tab(&page.ctx, &key, url).await?;
    // 挂上了先改视口再开推流：第一帧就是面板的尺寸。改不成（MCP 没连上）照样出画面。
    if let Some(viewport) = viewport {
        let _ = apply_viewport(&page.ctx, viewport).await;
    }
    let mut opts = options(&params);
    if viewport.is_some() {
        if params.get("maxWidth").is_none() {
            opts.max_width = NATIVE_MAX;
        }
        if params.get("maxHeight").is_none() {
            opts.max_height = NATIVE_MAX;
        }
    }
    let info = view.info().await;
    let tabs = match registry::lookup(&key) {
        Some(session) => tab_rows(&session, &view.targets().await.unwrap_or_default()).0,
        None => Vec::new(),
    };
    let frames = view
        .screencast(opts)
        .await
        .map_err(|e| RpcError::app("browser_unavailable", e))?;

    let view_id = uuid::Uuid::new_v4().simple().to_string();
    let current: Current = Arc::new(Mutex::new(Some(Arc::new(view))));
    let wanted: Wanted = Arc::new(Mutex::new(viewport));
    let kick = Arc::new(Notify::new());
    let pump = Pump {
        view_id: view_id.clone(),
        thread_id: thread_id.clone(),
        key: key.clone(),
        target: target.clone(),
        info: info.clone(),
        opts,
        current: current.clone(),
        out: out.clone(),
        views: Arc::downgrade(views),
        page: page.ctx.clone(),
        wanted: wanted.clone(),
        healed_at: None,
        kick: kick.clone(),
        tabs: tabs.clone(),
        adopt_at: None,
    };
    // 同一条连接对同一个会话只留一个视图，留最后收到的那次 open。查和装在同一把锁里，
    // 两个 open 不会都以为自己是最新的。
    let installed = {
        let mut open = views.open.lock().unwrap();
        let latest = views.latest_open.lock().unwrap().get(&thread_id).copied();
        if seq != 0 && latest.is_some_and(|l| l != seq) {
            Err(current)
        } else {
            let ids: Vec<String> = open
                .iter()
                .filter(|(_, e)| e.thread_id == thread_id)
                .map(|(id, _)| id.clone())
                .collect();
            let stale: Vec<(String, ViewEntry)> = ids
                .into_iter()
                .filter_map(|id| open.remove(&id).map(|e| (id, e)))
                .collect();
            let task = tokio::spawn(pump.run(frames));
            open.insert(
                view_id.clone(),
                ViewEntry {
                    thread_id: thread_id.clone(),
                    key,
                    current,
                    task,
                    page: page.ctx.clone(),
                    wanted,
                    kick,
                },
            );
            Ok(stale)
        }
    };
    let stale = match installed {
        Ok(stale) => stale,
        Err(current) => {
            // 开到一半又来了更新的 open：这个作废，CDP 连接关掉。
            let view = current.lock().unwrap().take();
            if let Some(view) = view.and_then(|v| Arc::try_unwrap(v).ok()) {
                view.close().await;
            }
            return Err(RpcError::app(
                "superseded",
                "同一会话又开了一个画面，这个作废",
            ));
        }
    };
    for (id, entry) in stale {
        entry.task.abort();
        views.last_input.lock().unwrap().remove(&id);
        // 它的 viewId 早回给客户端了：告诉它这个视图没了。
        let _ = out.send(Outgoing::Text(
            json!({
                "method": protocol::BROWSER_VIEW_CLOSED,
                "params": { "viewId": id, "threadId": thread_id, "reason": "replaced" }
            })
            .to_string(),
        ));
    }
    Ok(json!({
        "viewId": view_id,
        "threadId": thread_id,
        "targetId": target,
        "url": info.url,
        "title": info.title,
        "tabs": tabs,
    }))
}

/// `browser/view/tab`：以这页的身份调浏览器 MCP 的 `browser_tabs`。序号按名册现在的顺序
/// 现查（和 `browser_tabs` 的一致）；画面、标签栏由推送任务下一拍跟上。
async fn tab(views: &BrowserViews, params: &Value) -> Result<Value, RpcError> {
    let id = view_id(params)?;
    let (page, key) = views
        .open
        .lock()
        .unwrap()
        .get(id)
        .map(|e| (e.page.clone(), e.key.clone()))
        .ok_or_else(|| RpcError::app("not_found", format!("没有视图 {id}")))?;
    let action = params.get("action").and_then(Value::as_str).unwrap_or("");
    let index = || -> Result<usize, RpcError> {
        let target = params
            .get("targetId")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("targetId is required"))?;
        registry::lookup(&key)
            .and_then(|tabs| tabs.targets.iter().position(|t| t == target))
            .ok_or_else(|| RpcError::app("not_found", "这个标签页已经不在了"))
    };
    let args = match action {
        "switch" => json!({ "action": "switch", "index": index()? }),
        "close" => json!({ "action": "close", "index": index()? }),
        "new" => {
            let url = match params.get("url").and_then(Value::as_str).map(str::trim) {
                Some(raw) if !raw.is_empty() => {
                    cordis_browser::view::address_bar_url(raw).map_err(RpcError::invalid_params)?
                }
                _ => "about:blank".to_string(),
            };
            json!({ "action": "new", "url": url })
        }
        other => {
            return Err(RpcError::invalid_params(format!(
                "action 只能是 switch / close / new，收到 {other:?}"
            )))
        }
    };
    let out = mcp(&page)
        .map_err(|e| RpcError::app("browser_unavailable", e))?
        .call_as(&page, BROWSER_MCP_SERVER, "browser_tabs", args)
        .await
        .map_err(|e| RpcError::app("browser_unavailable", e))?;
    if out.is_error {
        let message = if out.content.contains("last tab") {
            "这是最后一个标签页，不能关".to_string()
        } else {
            out.content
        };
        return Err(RpcError::app("tab_failed", message));
    }
    Ok(json!({ "ok": true }))
}

/// 会话的标签栏：名册里的顺序配上 Chrome 里的地址标题；另回有没有「页面自己开了、还没收进
/// 会话」的页（opener 是本会话的页）。名册里有、Chrome 里已经没了的页不列。
fn tab_rows(
    session: &registry::SessionTabs,
    targets: &[cordis_browser::view::TargetSummary],
) -> (Vec<Value>, bool) {
    let rows = session
        .targets
        .iter()
        .filter_map(|id| targets.iter().find(|t| &t.id == id))
        .map(|t| {
            json!({
                "targetId": t.id,
                "url": t.url,
                "title": t.title,
                "active": session.active.as_ref() == Some(&t.id),
            })
        })
        .collect();
    let unadopted = targets.iter().any(|t| {
        t.kind == "page"
            && !session.targets.contains(&t.id)
            && t.opener
                .as_ref()
                .is_some_and(|o| session.targets.contains(o))
    });
    (rows, unadopted)
}

/// 找到会话的活动标签页并挂上；会话还没有标签页时带了 `url` 就替它开一页。
///
/// 名册里的页挂不上，可能是它已经没了（渲染进程崩了、被关掉）而名册还没改：浏览器 MCP
/// 只在被调用时才发现死页。先以这页的身份调一次 `browser_tabs` 让它清掉，再查一次——
/// 会话一页不剩就当没开过（`no_tab`，或者带着 `url` 开新页），不报「浏览器退出」。
async fn attach_session_tab(
    page: &Context,
    key: &str,
    url: Option<&str>,
) -> Result<(String, View), RpcError> {
    let no_tab = || {
        RpcError::app(
            "no_tab",
            "这个会话还没有打开浏览器标签页（agent 先 browser_open）",
        )
    };
    let target = match (active_target(key), url) {
        (Some(target), _) => target,
        (None, Some(url)) => open_tab(page, key, url).await?,
        (None, None) => return Err(no_tab()),
    };
    let stale = match View::attach(&target).await {
        Ok(view) => return Ok((target, view)),
        Err(e) => e,
    };
    let target = match (refresh_target(page, key).await, url) {
        (Some(next), _) if next != target => next,
        (Some(_), _) => return Err(RpcError::app("browser_unavailable", stale)),
        (None, Some(url)) => open_tab(page, key, url).await?,
        (None, None) => return Err(no_tab()),
    };
    let view = View::attach(&target)
        .await
        .map_err(|e| RpcError::app("browser_unavailable", e))?;
    Ok((target, view))
}

/// 让浏览器 MCP 清掉这个会话已经没了的标签页（它重写名册之后才回），再查名册。
///
/// MCP 说这个会话一页都没有，就当没有：名册里剩下的是被杀掉的 MCP 进程留下的旧文件
/// （重启 GUI 后常见，进程被杀来不及删自己那份），那些页跟着旧 Chromium 没了。
async fn refresh_target(page: &Context, key: &str) -> Option<String> {
    if let Ok(mcp) = mcp(page) {
        let out = mcp
            .call_as(page, BROWSER_MCP_SERVER, "browser_tabs", json!({}))
            .await;
        if out.is_ok_and(|o| o.is_error && o.content.contains(NO_SESSION_TAB)) {
            return None;
        }
    }
    active_target(key)
}

/// 会话还没有标签页：以这页的身份让浏览器 MCP 开一页（需要时拉起 Chromium），
/// 再等名册里出现它。和 agent 自己 `browser_open` 是同一条路，开出来的页 agent 接着能用。
async fn open_tab(page: &Context, key: &str, url: &str) -> Result<String, RpcError> {
    let url = cordis_browser::view::address_bar_url(url).map_err(RpcError::invalid_params)?;
    let out = mcp(page)
        .map_err(|e| RpcError::app("browser_unavailable", e))?
        .call_as(
            page,
            BROWSER_MCP_SERVER,
            "browser_open",
            json!({ "url": url }),
        )
        .await
        .map_err(|e| RpcError::app("browser_unavailable", e))?;
    if out.is_error {
        return Err(RpcError::app("browser_unavailable", out.content));
    }
    // 名册由 MCP 进程写盘，可能比回包晚一点。
    let deadline = Instant::now() + REGISTRY_WAIT;
    loop {
        if let Some(target) = active_target(key) {
            return Ok(target);
        }
        if Instant::now() >= deadline {
            return Err(RpcError::app(
                "no_tab",
                "标签页开了，但还没出现在会话名下，稍后再试",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 以这页的身份改它的活动标签页的视口（浏览器 MCP 的 `browser_resize`）。
async fn apply_viewport(page: &Context, viewport: Viewport) -> Result<(), String> {
    let out = mcp(page)?
        .call_as(
            page,
            BROWSER_MCP_SERVER,
            "browser_resize",
            json!({ "width": viewport.width, "height": viewport.height }),
        )
        .await?;
    if out.is_error {
        Err(out.content)
    } else {
        Ok(())
    }
}

fn mcp(page: &Context) -> Result<Arc<Mcp>, String> {
    page.get::<Mcp>(MCP)
        .ok_or_else(|| "MCP 客户端没有挂载".to_string())
}

/// `{ width, height, deviceScaleFactor? }`；`null` / 缺省 = 没要求。
fn parse_viewport(raw: &Value) -> Result<Option<Viewport>, RpcError> {
    if raw.is_null() {
        return Ok(None);
    }
    let int = |key: &str| raw.get(key).and_then(Value::as_i64);
    let (Some(width), Some(height)) = (int("width"), int("height")) else {
        return Err(RpcError::invalid_params(
            "viewport needs integer width and height",
        ));
    };
    if !(64..=10_000).contains(&width) || !(64..=10_000).contains(&height) {
        return Err(RpcError::invalid_params(
            "viewport width / height must be 64-10000",
        ));
    }
    let scale = raw
        .get("deviceScaleFactor")
        .and_then(Value::as_f64)
        .unwrap_or(1.0)
        .clamp(1.0, 3.0);
    Ok(Some(Viewport {
        width,
        height,
        scale,
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
    page: Context,
    wanted: Wanted,
    /// 上次因为尺寸对不上重设视口的时间。
    healed_at: Option<Instant>,
    /// 视口改了：补一张静止帧（见 [`ViewEntry::kick`]）。
    kick: Arc<Notify>,
    views: std::sync::Weak<BrowserViews>,
    /// 上次推给客户端的标签栏。
    tabs: Vec<Value>,
    /// 上次催浏览器 MCP 收下页面自己开的新页的时间。
    adopt_at: Option<Instant>,
}

enum Step {
    Frame(Option<cordis_browser::view::Frame>),
    Tick,
    /// 画面停下来了：补一张静止帧。
    Settle,
    /// 视口改了：过一会儿补一张静止帧。
    Kick,
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
        // 刚挂上就排一次：静止的页面不会推首帧。
        let mut settle_at = Some(Instant::now() + SETTLE);
        // 上一帧流里的画面（判断「补帧引出的同一帧」）。
        let mut last_stream = String::new();
        loop {
            let settle = async {
                match settle_at {
                    Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
                    None => std::future::pending().await,
                }
            };
            let kick = self.kick.clone();
            let step = if frames_open {
                tokio::select! {
                    frame = frames.next() => Step::Frame(frame),
                    _ = tick.tick() => Step::Tick,
                    _ = settle => Step::Settle,
                    _ = kick.notified() => Step::Kick,
                }
            } else {
                tokio::select! {
                    _ = tick.tick() => Step::Tick,
                    _ = settle => Step::Settle,
                    _ = kick.notified() => Step::Kick,
                }
            };
            match step {
                Step::Frame(Some(frame)) => {
                    let ack_id = frame.ack_id;
                    // 画面没变（重复帧）就不推、也不再排补帧：静止页面不会「补帧 → 新帧 → 再补帧」。
                    let changed = frame.data != last_stream;
                    if changed {
                        seq += 1;
                        self.heal(frame.device_width, frame.device_height);
                        last_stream = frame.data.clone();
                        self.push(seq, frame, false).await?;
                        settle_at = Some(Instant::now() + SETTLE);
                    }
                    let view = self.current.lock().unwrap().clone();
                    if let Some(view) = view {
                        view.ack(ack_id).await;
                    }
                }
                Step::Settle => {
                    settle_at = None;
                    let view = self.current.lock().unwrap().clone();
                    let scale = self.wanted.lock().unwrap().map_or(1.0, |v| v.scale);
                    // 截的时候先停流：放大栅格化的过渡帧不推；截完接着推。
                    let still = match view {
                        Some(view) => {
                            view.pause().await;
                            let still = view.still(STILL_QUALITY, scale).await.ok();
                            let _ = view.resume(self.opts).await;
                            still
                        }
                        None => None,
                    };
                    if let Some(frame) = still {
                        seq += 1;
                        self.push(seq, frame, true).await?;
                    }
                }
                Step::Kick => settle_at = Some(Instant::now() + SETTLE),
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
                                settle_at = Some(Instant::now() + SETTLE);
                                last_stream.clear();
                            }
                            // 页没了而浏览器还在：清掉名册里的死页，告诉客户端「没有标签页」。
                            Err(_) if refresh_target(&self.page, &self.key).await.is_none() => {
                                return Some("no_tab")
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
                    self.follow_tabs(&view).await;
                }
            }
            if self.out.is_closed() {
                return None;
            }
        }
    }

    /// 推一帧给客户端，写出去了才返回；连接没了回 `None`（收工）。
    /// `still`：画面停下来时补的静止帧（[`View::still`]），客户端照常画。
    async fn push(&self, seq: u64, frame: cordis_browser::view::Frame, still: bool) -> Option<()> {
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
                "still": still,
            }
        });
        self.out
            .send(Outgoing::Frame {
                text: note.to_string(),
                written,
            })
            .ok()?;
        done.await.ok()
    }

    /// 画面不是面板要的尺寸（agent 整页截图清掉了覆盖、或别人改过）：隔一会儿重设一次。
    /// 在后台跑，不卡推帧。
    fn heal(&mut self, width: f64, height: f64) {
        let Some(wanted) = *self.wanted.lock().unwrap() else {
            return;
        };
        let off = (width - wanted.width as f64).abs() > 2.0
            || (height - wanted.height as f64).abs() > 2.0;
        if !off || self.healed_at.is_some_and(|t| t.elapsed() < HEAL_EVERY) {
            return;
        }
        self.healed_at = Some(Instant::now());
        let page = self.page.clone();
        let kick = self.kick.clone();
        tokio::spawn(async move {
            if apply_viewport(&page, wanted).await.is_ok() {
                kick.notify_one();
            }
        });
    }

    /// agent 换了标签页（或画面流断了）：挂到新的那页上重新推。新页先套上面板的视口。
    async fn switch_to(&mut self, target: &str) -> Result<Frames, String> {
        let old = self.current.lock().unwrap().take();
        drop(old);
        let wanted = *self.wanted.lock().unwrap();
        if let Some(wanted) = wanted {
            let _ = apply_viewport(&self.page, wanted).await;
        }
        let view = View::attach(target).await?;
        let frames = view.screencast(self.opts).await?;
        self.info = view.info().await;
        self.target = target.to_string();
        *self.current.lock().unwrap() = Some(Arc::new(view));
        self.status();
        Ok(frames)
    }

    /// 标签栏变了就推；有页面自己开的新页还没进名册，就（隔一会儿）在后台催浏览器 MCP
    /// 收下——收下后名册的活动页变成新页，下一拍画面就切过去。
    async fn follow_tabs(&mut self, view: &View) {
        let Some(session) = registry::lookup(&self.key) else {
            return;
        };
        let Ok(targets) = view.targets().await else {
            return;
        };
        let (rows, unadopted) = tab_rows(&session, &targets);
        if rows != self.tabs {
            self.tabs = rows;
            self.send(json!({
                "method": protocol::BROWSER_VIEW_TABS,
                "params": { "viewId": self.view_id, "threadId": self.thread_id, "tabs": self.tabs }
            }));
        }
        if unadopted && self.adopt_at.is_none_or(|t| t.elapsed() >= ADOPT_EVERY) {
            self.adopt_at = Some(Instant::now());
            let page = self.page.clone();
            tokio::spawn(async move {
                if let Ok(mcp) = mcp(&page) {
                    let _ = mcp
                        .call_as(&page, BROWSER_MCP_SERVER, "browser_tabs", json!({}))
                        .await;
                }
            });
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_browser::view::TargetSummary;

    fn target(id: &str, kind: &str, opener: Option<&str>) -> TargetSummary {
        TargetSummary {
            id: id.into(),
            kind: kind.into(),
            url: format!("https://{id}.example/"),
            title: format!("{id} title"),
            opener: opener.map(String::from),
        }
    }

    #[test]
    fn tab_rows_follow_registry_order_and_spot_unadopted_popups() {
        let session = registry::SessionTabs {
            targets: vec!["B".into(), "A".into(), "GONE".into()],
            active: Some("A".into()),
        };
        // Chrome 的顺序和名册不一样；别的会话的页（X）和它开的页（Y）不算。
        let mut targets = vec![
            target("A", "page", None),
            target("X", "page", None),
            target("B", "page", None),
            target("Y", "page", Some("X")),
        ];
        let (rows, unadopted) = tab_rows(&session, &targets);
        let ids: Vec<&str> = rows
            .iter()
            .map(|r| r["targetId"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["B", "A"]);
        assert_eq!(rows[1]["active"], true);
        assert_eq!(rows[0]["active"], false);
        assert_eq!(rows[0]["title"], "B title");
        assert!(!unadopted);

        // 本会话的页开了新页、还没进名册：要催。
        targets.push(target("C", "page", Some("B")));
        assert!(tab_rows(&session, &targets).1);
        // 本会话的页起的 worker 不算。
        targets.pop();
        targets.push(target("W", "service_worker", Some("B")));
        assert!(!tab_rows(&session, &targets).1);
    }
}
