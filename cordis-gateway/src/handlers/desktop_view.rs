//! `desktop/view/*`：桌面（CUA）面板的实时画面——agent 正在操作的那个窗口。
//!
//! - `desktop/view/open { threadId?, maxDimension?, pid?, windowId? }` → `{ viewId, threadId }`。
//!   之后这条连接收 `desktop/view/frame` / `cursor` / `status` / `closed` 推送。带了 `pid` 和
//!   `windowId` 就从这一帧起钉住那个窗口。
//! - `desktop/view/windows { threadId? }` → `{ windows: [{ pid, windowId, app, title, zIndex }] }`。
//!   给面板上的窗口列表。跳过 Dock 自己和系统界面。
//! - `desktop/view/pin { viewId, pid?, windowId? }`：钉住这个窗口；两个 id 都不给就是松开，
//!   回到「agent 的窗口，否则最前面的窗口」。
//! - `desktop/view/input { viewId, event }`：用户在画面上的操作，打到最近一帧的那个窗口。
//!   `event` 是 `{ type: "click"|"drag"|"scroll"|"text"|"key", ... }`，坐标是这一帧的像素
//!   （和推出去的 `width` / `height` 同一套）。带了这一帧的 `pid` / `windowId` / 宽高时，
//!   和当前帧对不上就回 `not_ready`，不点。不过权限门：这是用户自己点的，不是 agent 的工具调用。
//! - `desktop/view/close { viewId }`；连接断开时自动全关。
//!
//! 钉住期间不挑最前面的窗口，也不按空闲节奏连着截：打开时一张、agent 对这个窗口动手时一张、
//! 用户操作之后一张。截不到就还盯着它，不退回前台。
//!
//! 看哪个窗口：这个会话最近一次 cua-driver 调用里带的 `pid` + `window_id`（agent 正在操作
//! 的那个，`source: "agent"`）；还没有就取最前面的普通窗口（`list_windows` 里 `z_index`
//! 最大的，跳过 Dock 自己，`source: "front"`）。agent 的窗口连续截不到就先看最前面的，
//! agent 再动手时换回来。
//!
//! 截图走 cua-driver 的 `get_window_state { include_accessibility_tree: false }`：它给实时
//! 预览留的「只截图」路径，不遍历 UI 树。以那页的身份调（[`Mcp::call_as`]），不进对话流、
//! 不过权限门——只读，和用户自己看一眼屏幕一样。每次调用最多等 [`DRIVER_TIMEOUT`]。
//!
//! 光标：窗口截图里没有 agent 光标（cua-driver 把它画在屏幕浮层上），客户端自己画。
//! 会话日志里每出现一次 agent 的桌面动作，就问 cua-driver 光标现在在哪
//! （`get_agent_cursor_state`，屏幕坐标，按动作带的 `session` 问），换成窗口里的相对位置
//! 推 `desktop/view/cursor`；动作类型和标签（输入的字、按键、滚动方向）取自调用参数。
//!
//! 节奏：上一帧写出去了才截下一帧。agent 最近 [`ACTIVE_FOR`] 内动过桌面时帧间隔
//! [`FRAME_EVERY`]，否则 [`QUIET_FRAME_EVERY`]；新动作一出现立刻补一帧。截不到、没窗口时
//! 推 `desktop/view/status` 说清原因，不默默重试。cua-driver 没连上推
//! `closed { reason: "driver_unavailable" }`。
//!
//! 视图是**连接级**的：不进会话、不落盘，只活在这条 WebSocket 上。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use cordis::Context;
use cordis_base::cua::CUA_DRIVER_SERVER;
use cordis_base::types::LogEvent;
use cordis_spine::{Mcp, Sessions, ToolResult, MCP, SESSIONS};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex as AsyncMutex, Notify};
use tokio::task::JoinHandle;

use crate::handle::GatewayHandle;
use crate::protocol::{self, RpcError};
use crate::threads;
use crate::ws::{OutTx, Outgoing};

/// agent 正在动桌面时，两帧之间至少隔这么久（约 4 帧/秒）。
const FRAME_EVERY: Duration = Duration::from_millis(250);
/// agent 有一阵没动桌面时的帧间隔。
const QUIET_FRAME_EVERY: Duration = Duration::from_millis(1000);
/// agent 动过桌面之后多久内按 [`FRAME_EVERY`] 截。
const ACTIVE_FOR: Duration = Duration::from_secs(5);
/// 没有可看的窗口 / 截图失败时，多久再试。
const IDLE_EVERY: Duration = Duration::from_millis(800);
/// 前台窗口（`list_windows`）多久重新挑一次。
const FRONT_EVERY: Duration = Duration::from_secs(2);
/// 等下一帧时多久看一眼会话日志（agent 一动手就补一帧）。
const WATCH_EVERY: Duration = Duration::from_millis(50);
/// 一次 cua-driver 调用最多等这么久（只截图实测约 0.3 秒）。
const DRIVER_TIMEOUT: Duration = Duration::from_secs(5);
/// agent 的窗口连续截不到这么多次，先看最前面的窗口。
const AGENT_FAILS_BEFORE_FRONT: u32 = 3;
/// 连续截不到这么多次才告诉客户端：偶尔一次（agent 自己在跑一个很慢的 cua-driver 调用，
/// 截图排在后面超时）不算故障，不必闪一下「截图失败」。
const FAILS_BEFORE_REPORT: u32 = 2;
/// 光标标签里输入的文字最多这么多字。
const LABEL_CHARS: usize = 12;
/// 画面最长边缺省值（像素）。
const DEFAULT_MAX_DIMENSION: i64 = 1280;
/// cua-driver 的工具名前缀（MCP 公开名）。
const CUA_PREFIX: &str = "mcp_cua-driver__";
/// 挑前台窗口时跳过的：Dock 自己、cua-driver 画 agent 光标的全屏透明浮层（它在
/// `list_windows` 里是一扇普通窗口、常在最前面，截它只会失败）、系统界面。
const SKIP_APPS: &[&str] = &[
    "Dock",
    "dock-gui",
    "Cua Driver",
    "CuaDriver",
    "Window Server",
    "Control Center",
    "控制中心",
    "SystemUIServer",
    "Notification Center",
    "通知中心",
];

/// 一条连接上开着的桌面视图。丢掉（连接断开）时中止全部推送任务。
#[derive(Default)]
pub(crate) struct DesktopViews {
    open: Mutex<HashMap<String, Entry>>,
    /// 每个会话最后收到的那次 `open` 的序号（见 [`DesktopViews::order`]）。
    latest_open: Mutex<HashMap<String, u64>>,
    next_open: AtomicU64,
    /// 每个视图最后一个 `input` 的「做完了」信号，下一个先等它。
    last_input: Mutex<HashMap<String, oneshot::Receiver<()>>>,
}

/// 请求在读循环里定下的先后（见 [`DesktopViews::order`]）。
pub(crate) enum DesktopOrder {
    None,
    Open(u64),
    Input {
        prev: Option<oneshot::Receiver<()>>,
        /// 这一个做完（或失败）时随 drop 通知下一个。
        done: oneshot::Sender<()>,
    },
}

impl DesktopViews {
    /// 在连接的读循环里、按收到的顺序同步调用（不能挪进各自的任务里：任务开跑的先后
    /// 不保证）。`open` 记下序号；`input` 排到同一视图上一个输入后面。
    ///
    /// 同一会话连发两次 `open`（React StrictMode 双挂载、快速重开面板）时，两个任务谁先
    /// 做完不一定：晚做完的旧 `open` 不能把新的顶掉——客户端早就不要它了，回头把它一关，
    /// 这个会话就一个视图都不剩，画面永远停在「正在连接」。
    pub(crate) fn order(&self, method: &str, params: &Value) -> DesktopOrder {
        match method {
            protocol::DESKTOP_VIEW_OPEN => {
                let seq = self.next_open.fetch_add(1, Ordering::Relaxed) + 1;
                self.latest_open
                    .lock()
                    .unwrap()
                    .insert(threads::thread_param(params), seq);
                DesktopOrder::Open(seq)
            }
            protocol::DESKTOP_VIEW_INPUT => {
                let Ok(id) = view_id_param(params) else {
                    return DesktopOrder::None;
                };
                let (done, next) = oneshot::channel();
                let prev = self.last_input.lock().unwrap().insert(id.to_string(), next);
                DesktopOrder::Input { prev, done }
            }
            _ => DesktopOrder::None,
        }
    }

    /// 这次 `open` 还是这个会话最后收到的那次（`seq` 为 0 = 没定序，算是）。
    fn is_latest(&self, thread_id: &str, seq: u64) -> bool {
        seq == 0
            || self
                .latest_open
                .lock()
                .unwrap()
                .get(thread_id)
                .is_none_or(|&latest| latest == seq)
    }

    fn view(&self, id: &str) -> Result<(Context, Arc<Shared>), RpcError> {
        self.open
            .lock()
            .unwrap()
            .get(id)
            .map(|e| (e.page.clone(), e.shared.clone()))
            .ok_or_else(|| RpcError::app("not_found", format!("没有视图 {id}")))
    }

    fn take(&self, id: &str) -> Option<Entry> {
        self.last_input.lock().unwrap().remove(id);
        self.open.lock().unwrap().remove(id)
    }
}

struct Entry {
    thread_id: String,
    task: JoinHandle<()>,
    page: Context,
    shared: Arc<Shared>,
}

/// 泵和 `pin` / `input` 共用的状态。截图和用户操作共用 [`Shared::driver`]：
/// `get_window_state` 会换成新的像素帧，记下 [`FrameInfo`] 之前不放锁，点击就读不到半截。
struct Shared {
    pin: Mutex<Option<(i64, i64)>>,
    /// 用户点了、或换了钉。泵截的时候清掉。
    wake: AtomicBool,
    kick: Notify,
    /// 最近一帧：用户操作打到这个窗口，坐标按它的像素算。
    /// 和 [`Shared::pin`] 的加锁顺序：先 `pin`，再 `frame`，再 `pixels`。
    frame: Mutex<Option<FrameInfo>>,
    /// 上一张预览的像素哈希。一样就不往连接上再推。
    pixels: Mutex<Option<(i64, i64, u64)>>,
    /// 最近一次定位 PNG：同一窗口、同一尺寸，滚动和按键可以接着用，不必每次再截。
    ground: Mutex<Option<GroundMark>>,
    driver: AsyncMutex<()>,
    /// 预览和点击用的 cua 会话代号。`0` 是 [`VIEW_SESSION`]；驱动说这个名字已经结束
    /// （Dock 重启后旧连接的会话不会复活）就加一，换成新名字。
    session_gen: AtomicU64,
}

impl Shared {
    fn new(pin: Option<(i64, i64)>) -> Arc<Self> {
        Arc::new(Self {
            pin: Mutex::new(pin),
            wake: AtomicBool::new(false),
            kick: Notify::new(),
            frame: Mutex::new(None),
            pixels: Mutex::new(None),
            ground: Mutex::new(None),
            driver: AsyncMutex::new(()),
            session_gen: AtomicU64::new(0),
        })
    }

    /// 这一视图现在用的 cua 会话名。
    fn session_label(&self) -> String {
        session_label(self.session_gen.load(Ordering::Acquire))
    }

    /// 旧名字已经被驱动关掉：换下一个。回新名字。
    fn rotate_session(&self) -> String {
        let next = self.session_gen.fetch_add(1, Ordering::AcqRel) + 1;
        session_label(next)
    }

    fn wake_up(&self) {
        self.wake.store(true, Ordering::Release);
        self.kick.notify_one();
    }

    /// 换钉。窗口变了就丢掉上一帧，免得点击还打到旧窗口上。
    fn set_pin(&self, next: Option<(i64, i64)>) {
        let mut pin = self.pin.lock().unwrap();
        if *pin != next {
            *pin = next;
            *self.frame.lock().unwrap() = None;
            *self.pixels.lock().unwrap() = None;
            *self.ground.lock().unwrap() = None;
        }
        drop(pin);
        self.wake_up();
    }

    /// 这张像素和上一张是不是同一个窗口的同一份图。新的记下来，回 `true`。
    fn note_pixels(&self, pid: i64, window_id: i64, data: &[u8]) -> bool {
        let hash = pixel_hash(data);
        let mut slot = self.pixels.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|seen| seen.0 == pid && seen.1 == window_id && seen.2 == hash)
        {
            return false;
        }
        *slot = Some((pid, window_id, hash));
        true
    }

    /// 这帧和最近一次定位图是同一个窗口、同一尺寸，而且还没过期。
    fn has_fresh_ground(&self, frame: &FrameInfo) -> bool {
        self.ground.lock().unwrap().as_ref().is_some_and(|mark| {
            mark.pid == frame.pid
                && mark.window_id == frame.window_id
                && mark.width == frame.width
                && mark.height == frame.height
                && mark.at.elapsed() < GROUND_FRESH
        })
    }

    fn mark_ground(&self, frame: &FrameInfo, width: u32, height: u32) {
        *self.ground.lock().unwrap() =
            (width == frame.width && height == frame.height).then_some(GroundMark {
                pid: frame.pid,
                window_id: frame.window_id,
                width,
                height,
                at: Instant::now(),
            });
    }

    /// 截到一张，记下它的窗口和像素。钉已经换到别的窗口就丢掉，不把旧画面写回来。
    fn remember(&self, info: FrameInfo) {
        let pin = self.pin.lock().unwrap();
        if pin.is_some_and(|p| p != (info.pid, info.window_id)) {
            return;
        }
        *self.frame.lock().unwrap() = Some(info);
    }
}

/// 最近一次能拿来点的 PNG。
struct GroundMark {
    pid: i64,
    window_id: i64,
    width: u32,
    height: u32,
    at: Instant,
}

/// 滚动和按键复用定位图的时限。过了就重新截一张。
const GROUND_FRESH: Duration = Duration::from_secs(15);

/// 滚动和按键可以复用刚才的定位图。点击、拖动、打字仍每次重新定位。
fn reuses_ground(tool: &str) -> bool {
    matches!(tool, "scroll" | "press_key" | "hotkey")
}

/// 只有指针类工具吃 `capture_id`。打字和按键带上这个字段会被拒。
fn takes_capture_id(tool: &str) -> bool {
    matches!(
        tool,
        "click" | "double_click" | "right_click" | "drag" | "scroll"
    )
}

/// 最近一帧的窗口和像素尺寸。
#[derive(Clone, Copy, Debug, PartialEq)]
struct FrameInfo {
    pid: i64,
    window_id: i64,
    width: u32,
    height: u32,
}

impl Drop for DesktopViews {
    fn drop(&mut self) {
        for (_, entry) in self.open.lock().unwrap().drain() {
            entry.task.abort();
        }
    }
}

pub(crate) fn is_desktop_view(method: &str) -> bool {
    matches!(
        method,
        protocol::DESKTOP_VIEW_OPEN
            | protocol::DESKTOP_VIEW_CLOSE
            | protocol::DESKTOP_VIEW_WINDOWS
            | protocol::DESKTOP_VIEW_PIN
            | protocol::DESKTOP_VIEW_INPUT
    )
}

pub(crate) async fn dispatch(
    gateway: &GatewayHandle,
    views: &Arc<DesktopViews>,
    out: &OutTx,
    method: &str,
    params: Value,
    order: DesktopOrder,
) -> Result<Value, RpcError> {
    match method {
        protocol::DESKTOP_VIEW_OPEN => {
            let seq = match order {
                DesktopOrder::Open(seq) => seq,
                _ => 0,
            };
            open(gateway, views, out, params, seq).await
        }
        protocol::DESKTOP_VIEW_CLOSE => {
            let id = require_view_id(&params)?;
            let entry = views
                .take(id)
                .ok_or_else(|| RpcError::app("not_found", format!("没有视图 {id}")))?;
            entry.task.abort();
            Ok(json!({ "ok": true, "viewId": id }))
        }
        protocol::DESKTOP_VIEW_WINDOWS => windows(gateway, &params).await,
        protocol::DESKTOP_VIEW_PIN => pin_view(views, &params),
        protocol::DESKTOP_VIEW_INPUT => {
            let _done = match order {
                DesktopOrder::Input { prev, done } => {
                    if let Some(prev) = prev {
                        let _ = prev.await;
                    }
                    Some(done)
                }
                _ => None,
            };
            input(views, &params).await
        }
        _ => Err(RpcError::method_not_found(method)),
    }
}

fn require_view_id(params: &Value) -> Result<&str, RpcError> {
    view_id_param(params).map_err(|_| RpcError::invalid_params("viewId is required"))
}

fn view_id_param(params: &Value) -> Result<&str, ()> {
    params
        .get("viewId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(())
}

async fn open(
    gateway: &GatewayHandle,
    views: &Arc<DesktopViews>,
    out: &OutTx,
    params: Value,
    seq: u64,
) -> Result<Value, RpcError> {
    let thread_id = threads::thread_param(&params);
    let page = threads::resolve(gateway, &thread_id)?;
    // 先确认 cua-driver 连着（有上限），免得回了 viewId 才推 closed、或者一直等着。
    driver_call(&page.ctx, "list_windows", json!({ "on_screen_only": true }))
        .await
        .map_err(|e| RpcError::app("driver_unavailable", e))?;
    let max_dimension = params
        .get("maxDimension")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_MAX_DIMENSION)
        .clamp(320, 4096);
    let pin = pin_param(&params)?;
    let view_id = uuid::Uuid::new_v4().simple().to_string();
    let shared = Shared::new(pin);
    let pump = Pump {
        view_id: view_id.clone(),
        thread_id: thread_id.clone(),
        page: page.ctx.clone(),
        out: out.clone(),
        max_dimension,
        log: AgentLog::default(),
        front: None,
        shown: None,
        status: None,
        agent_fails: 0,
        fails: 0,
        skip_agent: false,
        pin_retry: false,
        active_until: None,
        shared: shared.clone(),
    };
    // 同一条连接对同一个会话只留一个视图，留最后收到的那次 open：新的顶掉旧的，
    // 晚做完的旧 open 作废。查和装在同一把锁里。
    let stale: Vec<(String, Entry)> = {
        let mut open = views.open.lock().unwrap();
        if !views.is_latest(&thread_id, seq) {
            return Err(RpcError::app(
                "superseded",
                "同一会话又开了一个画面，这个作废",
            ));
        }
        let ids: Vec<String> = open
            .iter()
            .filter(|(_, e)| e.thread_id == thread_id)
            .map(|(id, _)| id.clone())
            .collect();
        let stale = ids
            .into_iter()
            .filter_map(|id| open.remove(&id).map(|e| (id, e)))
            .collect();
        let views_weak = Arc::downgrade(views);
        let task = tokio::spawn(pump.run(views_weak));
        open.insert(
            view_id.clone(),
            Entry {
                thread_id: thread_id.clone(),
                task,
                page: page.ctx,
                shared,
            },
        );
        stale
    };
    for (id, _) in &stale {
        views.last_input.lock().unwrap().remove(id);
    }
    for (id, entry) in stale {
        entry.task.abort();
        send(
            out,
            json!({
                "method": protocol::DESKTOP_VIEW_CLOSED,
                "params": { "viewId": id, "threadId": thread_id, "reason": "replaced" }
            }),
        );
    }
    Ok(json!({ "viewId": view_id, "threadId": thread_id, "pinned": pin.is_some() }))
}

/// 可钉的窗口，按前后顺序（最前面的在前）。
async fn windows(gateway: &GatewayHandle, params: &Value) -> Result<Value, RpcError> {
    let thread_id = threads::thread_param(params);
    let page = threads::resolve(gateway, &thread_id)?;
    let listed = driver_call(&page.ctx, "list_windows", json!({ "on_screen_only": true }))
        .await
        .map_err(|e| RpcError::app("driver_unavailable", e))?;
    Ok(json!({ "windows": window_choices(&windows_in(&listed)) }))
}

/// 钉住或松开。窗口变了，下一帧才再接受点击。
fn pin_view(views: &DesktopViews, params: &Value) -> Result<Value, RpcError> {
    let id = require_view_id(params)?;
    let pin = pin_param(params)?;
    let (_, shared) = views.view(id)?;
    shared.set_pin(pin);
    Ok(json!({ "ok": true, "pinned": pin.is_some() }))
}

/// 用户的一次操作。坐标是最近一帧的像素；做完叫醒泵再截一张。
/// 预览是 `display_only` 的 JPEG，不能拿来定位。先补一张同尺寸的 PNG，再操作。
/// 后台被拒（窗口藏着）时，只把这一下改成前台重试一次。
async fn input(views: &DesktopViews, params: &Value) -> Result<Value, RpcError> {
    let id = require_view_id(params)?;
    let (page, shared) = views.view(id)?;
    let event = params
        .get("event")
        .filter(|e| e.is_object())
        .ok_or_else(|| RpcError::invalid_params("event is required"))?;
    {
        let _guard = shared.driver.lock().await;
        let frame = {
            let _pin = shared.pin.lock().unwrap();
            *shared.frame.lock().unwrap()
        }
        .ok_or_else(|| RpcError::app("not_ready", "还没有画面"))?;
        if frame_changed(event, &frame) {
            return Err(RpcError::app("not_ready", "画面已经换了，请再点一次"));
        }
        let (tool, mut args) = user_gesture(event, &frame).map_err(RpcError::invalid_params)?;
        // 滚动和按键很密。同一窗口刚定位过就直接发，省掉每次一张 PNG。
        if !(reuses_ground(&tool) && shared.has_fresh_ground(&frame)) {
            let ground = ground_frame(&page, &shared, &frame)
                .await
                .map_err(|e| RpcError::app("action_failed", e))?;
            fit_pixels(&mut args, &frame, ground.width, ground.height);
            // `type_text` / `press_key` / `hotkey` 不收 capture_id，带上会被驱动拒掉。
            if takes_capture_id(&tool) {
                if let Some(id) = ground.capture_id {
                    args["capture_id"] = json!(id);
                }
            }
            shared.mark_ground(&frame, ground.width, ground.height);
        }
        if let Err(err) = view_driver_call(&page, &shared, &tool, args.clone()).await {
            if !should_foreground(&err) {
                return Err(RpcError::app("action_failed", err));
            }
            args["delivery_mode"] = json!("foreground");
            view_driver_call(&page, &shared, &tool, args)
                .await
                .map_err(|e| RpcError::app("action_failed", e))?;
        }
    }
    shared.wake_up();
    Ok(json!({ "ok": true }))
}

/// `pid` 和 `windowId` 都有才钉住；都没有是不钉。只给一个是参数错误。
fn pin_param(params: &Value) -> Result<Option<(i64, i64)>, RpcError> {
    let pid = int_arg(params, "pid");
    let window_id = int_arg(params, "windowId").or_else(|| int_arg(params, "window_id"));
    match (pid, window_id) {
        (Some(pid), Some(window_id)) => Ok(Some((pid, window_id))),
        (None, None) => Ok(None),
        _ => Err(RpcError::invalid_params("钉住窗口要同时给 pid 和 windowId")),
    }
}

/// 一个要看的窗口。
#[derive(Clone, Debug, PartialEq)]
struct Target {
    pid: i64,
    window_id: i64,
    /// `agent`：会话最近一次 cua-driver 调用的窗口；`front`：最前面的窗口。
    source: &'static str,
}

/// agent 的一次桌面动作：客户端按它画光标。
#[derive(Clone, Debug, PartialEq)]
struct Action {
    /// `click` / `double_click` / `right_click` / `drag` / `scroll` / `type` / `key`。
    kind: &'static str,
    /// 输入的字（截断）、滚动方向。
    label: Option<String>,
    /// 按键，按下的顺序：`["cmd", "shift", "n"]`。
    keys: Vec<String>,
    /// 调用时带的 cua-driver 会话（光标归它）；没带是这条 MCP 连接的隐式会话。
    session: Option<String>,
    /// 动作针对的 `(pid, window_id)`；没带就是当前窗口。
    window: Option<(i64, i64)>,
}

/// 会话日志里 agent 的 cua-driver 调用：最近操作的窗口，和这个视图打开之后新做的动作。
/// 按日志版本缓存，日志没变就不重扫。
#[derive(Default)]
struct AgentLog {
    rev: Option<u64>,
    /// 已经看过的事件数；`None` = 还没看过（打开视图之前的历史不算新动作）。
    seen: Option<usize>,
    window: Option<(i64, i64)>,
}

impl AgentLog {
    /// 日志变了就重扫：更新 agent 的窗口，返回新做的动作（按先后）。
    fn poll(&mut self, page: &Context) -> Vec<Action> {
        let Some(sessions) = page.get::<Sessions>(SESSIONS) else {
            return Vec::new();
        };
        let rev = sessions.events_rev();
        if self.rev == Some(rev) {
            return Vec::new();
        }
        self.rev = Some(rev);
        let seen = self.seen;
        let scanned = sessions.with_log(|events, _| scan(events, seen));
        self.window = scanned.window;
        self.seen = Some(scanned.len);
        scanned.actions
    }

    /// 日志在上次 [`AgentLog::poll`] 之后变过。
    fn changed(&self, page: &Context) -> bool {
        page.get::<Sessions>(SESSIONS)
            .is_some_and(|s| Some(s.events_rev()) != self.rev)
    }
}

/// [`scan`] 的结果。
#[derive(Debug, PartialEq)]
struct Scanned {
    window: Option<(i64, i64)>,
    actions: Vec<Action>,
    len: usize,
}

/// 扫一遍日志：agent 最近操作的窗口，和第 `seen` 条之后的桌面动作。`seen` 为 `None`
/// （第一次看）或比日志还长（压缩重写过）时只认窗口、不算新动作。
fn scan(events: &[LogEvent], seen: Option<usize>) -> Scanned {
    let actions = match seen.filter(|&s| s <= events.len()) {
        Some(from) => events[from..]
            .iter()
            .filter_map(cua_call)
            .filter_map(|(tool, input)| cua_action(&tool, &input))
            .collect(),
        None => Vec::new(),
    };
    Scanned {
        window: last_cua_window(events),
        actions,
        len: events.len(),
    }
}

/// 一条 cua-driver 调用（直调或经 `use_tool`）：去掉前缀的工具名和参数。
fn cua_call(event: &LogEvent) -> Option<(String, Value)> {
    let LogEvent::ToolExecute {
        name, arguments, ..
    } = event
    else {
        return None;
    };
    let args: Value = serde_json::from_str(arguments).ok()?;
    let (tool, input) = if name == cordis_spine::USE_TOOL_NAME {
        (
            args.get("tool_name").and_then(Value::as_str)?.to_string(),
            args.get("tool_input").cloned().unwrap_or(Value::Null),
        )
    } else {
        (name.clone(), args)
    };
    Some((tool.strip_prefix(CUA_PREFIX)?.to_string(), input))
}

/// 日志里最后一次带 `pid` + `window_id` 的 cua-driver 调用的窗口。
fn last_cua_window(events: &[LogEvent]) -> Option<(i64, i64)> {
    events
        .iter()
        .rev()
        .filter_map(cua_call)
        .find_map(|(_, input)| Some((int_arg(&input, "pid")?, int_arg(&input, "window_id")?)))
}

/// cua-driver 工具调用 → 桌面动作；不动桌面的（截图、列窗口、开应用）回 `None`。
/// `move_cursor` 也不算：它报的光标位置是原样的入参，和别的动作的屏幕坐标对不上。
fn cua_action(tool: &str, input: &Value) -> Option<Action> {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
    let (kind, label, keys) = match tool {
        "click" if text("button").as_deref() == Some("right") => ("right_click", None, Vec::new()),
        "click" => ("click", None, Vec::new()),
        "double_click" => ("double_click", None, Vec::new()),
        "right_click" => ("right_click", None, Vec::new()),
        "drag" => ("drag", None, Vec::new()),
        "scroll" => ("scroll", text("direction"), Vec::new()),
        "type_text" => ("type", text("text").map(|t| clip(&t)), Vec::new()),
        "set_value" => ("type", text("value").map(|t| clip(&t)), Vec::new()),
        "press_key" => {
            let mut keys = strings(input.get("modifiers"));
            keys.extend(text("key"));
            ("key", None, keys)
        }
        "hotkey" => ("key", None, strings(input.get("keys"))),
        _ => return None,
    };
    Some(Action {
        kind,
        label,
        keys,
        session: text("session").filter(|s| !s.is_empty()),
        window: int_arg(input, "pid").zip(int_arg(input, "window_id")),
    })
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// 标签里的字：最多 [`LABEL_CHARS`] 个，多了加省略号。
fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(LABEL_CHARS).collect();
    if text.chars().count() > LABEL_CHARS {
        out.push('…');
    }
    out
}

/// 窗口在屏幕上的位置（点）。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn bounds_of(v: &Value) -> Option<Bounds> {
    let n = |k: &str| v.get(k).and_then(Value::as_f64);
    let b = Bounds {
        x: n("x")?,
        y: n("y")?,
        width: n("width")?,
        height: n("height")?,
    };
    (b.width > 0.0 && b.height > 0.0).then_some(b)
}

/// 屏幕坐标（点）→ 窗口里的相对位置（0–1；窗口外的会超出这个范围）。
fn relative(point: (f64, f64), b: Bounds) -> (f64, f64) {
    ((point.0 - b.x) / b.width, (point.1 - b.y) / b.height)
}

/// 画面上现在这个窗口：光标按它的屏幕位置换算。
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    window: (i64, i64),
    bounds: Option<Bounds>,
    app: Option<String>,
}

/// 画面状态，变了才推 `desktop/view/status`。
#[derive(Clone, Debug, PartialEq)]
enum Status {
    Live,
    NoWindow,
    CaptureFailed(String),
}

impl Status {
    fn params(&self) -> Value {
        match self {
            Status::Live => json!({ "state": "live" }),
            Status::NoWindow => json!({
                "state": "no_window",
                "message": "没有可看的窗口 · agent 打开应用后自动跟上",
            }),
            Status::CaptureFailed(message) => json!({
                "state": "capture_failed",
                "message": message,
            }),
        }
    }
}

/// 截不到的原因（给人看的），按 cua-driver 回的错误猜最常见的几种。
fn capture_reason(content: &str) -> &'static str {
    if session_ended(content) {
        "预览会话断了"
    } else if content.contains("window_id_not_found")
        || content.contains("window_owner_pid_mismatch")
    {
        "窗口已关闭"
    } else if content.contains("px_capture_unavailable") || content.contains("px_frame_mismatch") {
        "这个窗口现在截不到画面"
    } else {
        "截图失败"
    }
}

fn failure_message(app: Option<&str>, reason: &str) -> String {
    match app {
        Some(app) => format!("截不到「{app}」：{reason} · 正在重试"),
        None => format!("{reason} · 正在重试"),
    }
}

struct Pump {
    view_id: String,
    thread_id: String,
    page: Context,
    out: OutTx,
    max_dimension: i64,
    log: AgentLog,
    /// 上次挑的前台窗口和挑的时间。
    front: Option<(Target, Instant)>,
    /// 最近一帧的窗口。
    shown: Option<Shown>,
    /// 最近推过的状态。
    status: Option<Status>,
    /// agent 的窗口连续截不到几次了。
    agent_fails: u32,
    /// 连续截不到几次了（哪个窗口都算）。
    fails: u32,
    /// agent 的窗口一直截不到：先看最前面的，agent 再动手时换回来。
    skip_agent: bool,
    /// agent 最近一次动桌面之后的「活跃」截止时间。
    active_until: Option<Instant>,
    /// 钉住的窗口上一轮没截到：隔一会儿再试，不改去追前台。
    pin_retry: bool,
    shared: Arc<Shared>,
}

/// [`Pump::pinned_step`] 的下一步。
enum Pinned {
    Again,
    Closed,
    Stop(&'static str),
}

impl Pump {
    async fn run(mut self, views: std::sync::Weak<DesktopViews>) {
        let reason = self.pump().await;
        if let Some(reason) = reason {
            send(
                &self.out,
                json!({
                    "method": protocol::DESKTOP_VIEW_CLOSED,
                    "params": { "viewId": self.view_id, "threadId": self.thread_id, "reason": reason }
                }),
            );
        }
        if let Some(views) = views.upgrade() {
            views.take(&self.view_id);
        }
    }

    /// 返回结束原因；`None` 是连接没了（没人可告诉）。
    async fn pump(&mut self) -> Option<&'static str> {
        let mut seq: u64 = 0;
        loop {
            if self.out.is_closed() {
                return None;
            }
            let pin = *self.shared.pin.lock().unwrap();
            if let Some(pin) = pin {
                match self.pinned_step(&mut seq, pin).await {
                    Pinned::Closed => return None,
                    Pinned::Stop(reason) => return Some(reason),
                    Pinned::Again => continue,
                }
            }
            // 松开钉：这一轮照旧跟着 agent / 前台。用户点过之后把「该截了」清掉，
            // 下面的 wait 会因为这次点击提前醒来，下一轮就截。
            self.shared.wake.store(false, Ordering::Release);
            let started = Instant::now();
            // agent 刚做完的动作（几个连着的只画最后一个）。还在原来那个窗口上就先推光标，
            // 再截这一下的画面；换了窗口就等新窗口截到、知道它在屏幕哪儿再推。
            let action = self.log.poll(&self.page).pop();
            let mut pending = None;
            if let Some(action) = action {
                self.active_until = Some(started + ACTIVE_FOR);
                self.skip_agent = false;
                self.agent_fails = 0;
                if self.on_shown_window(&action) {
                    self.cursor(&action).await;
                } else {
                    pending = Some(action);
                }
            }
            let target = match self.target().await {
                Ok(Some(target)) => target,
                Ok(None) => {
                    self.set_status(Status::NoWindow);
                    self.wait(started + IDLE_EVERY).await;
                    continue;
                }
                Err(Stop) => return Some("driver_unavailable"),
            };
            let every = match self.capture(&target).await {
                Ok(Ok(shot)) => {
                    self.agent_fails = 0;
                    self.fails = 0;
                    if shot.fresh {
                        seq += 1;
                        self.push_frame(seq, &target, shot).await?;
                    }
                    self.set_status(Status::Live);
                    let active = self.active_until.is_some_and(|t| Instant::now() < t);
                    if active {
                        FRAME_EVERY
                    } else {
                        QUIET_FRAME_EVERY
                    }
                }
                Ok(Err(reason)) => {
                    if target.source == "agent" {
                        self.agent_fails += 1;
                        self.skip_agent = self.agent_fails >= AGENT_FAILS_BEFORE_FRONT;
                    }
                    // 前台窗口重新挑，下一轮再试。
                    self.front = None;
                    self.fails += 1;
                    if self.fails >= FAILS_BEFORE_REPORT {
                        let app = self
                            .shown
                            .as_ref()
                            .filter(|s| s.window == (target.pid, target.window_id))
                            .and_then(|s| s.app.clone());
                        self.set_status(Status::CaptureFailed(failure_message(
                            app.as_deref(),
                            reason,
                        )));
                    }
                    IDLE_EVERY
                }
                Err(Stop) => return Some("driver_unavailable"),
            };
            if let Some(action) = pending {
                self.cursor(&action).await;
            }
            self.wait(started + every).await;
        }
    }

    /// 钉住时的一轮：只截这个窗口。没有新动作、用户也没点，就一直等，不按秒刷新。
    async fn pinned_step(&mut self, seq: &mut u64, pin: (i64, i64)) -> Pinned {
        let started = Instant::now();
        let action = self.log.poll(&self.page).pop();
        // 没带窗口号的动作不算「对这个窗口动手」，不因此刷新。
        let on_pin = action.as_ref().is_some_and(|a| a.window == Some(pin));
        let mut pending = None;
        if let Some(action) = action {
            if self.on_shown_window(&action) {
                self.cursor(&action).await;
            } else if on_pin {
                pending = Some(action);
            }
        }
        let wake = self.shared.wake.swap(false, Ordering::AcqRel);
        let switched = self.shown.as_ref().is_none_or(|s| s.window != pin);
        let need = switched || on_pin || wake || self.pin_retry;
        if !need {
            self.wait_pinned().await;
            return Pinned::Again;
        }
        let target = Target {
            pid: pin.0,
            window_id: pin.1,
            source: "pin",
        };
        match self.capture(&target).await {
            Ok(Ok(shot)) => {
                self.fails = 0;
                self.pin_retry = false;
                if shot.fresh {
                    *seq += 1;
                    if self.push_frame(*seq, &target, shot).await.is_none() {
                        return Pinned::Closed;
                    }
                }
                self.set_status(Status::Live);
                if let Some(action) = pending {
                    self.cursor(&action).await;
                }
            }
            Ok(Err(reason)) => {
                self.pin_retry = true;
                self.fails += 1;
                if self.fails >= FAILS_BEFORE_REPORT {
                    let app = self
                        .shown
                        .as_ref()
                        .filter(|s| s.window == pin)
                        .and_then(|s| s.app.clone());
                    self.set_status(Status::CaptureFailed(failure_message(
                        app.as_deref(),
                        reason,
                    )));
                }
                self.wait(started + IDLE_EVERY).await;
            }
            Err(Stop) => return Pinned::Stop("driver_unavailable"),
        }
        Pinned::Again
    }

    /// 等到用户点了、换了钉，或会话日志变了。不按时钟截下一帧。
    async fn wait_pinned(&self) {
        loop {
            if self.out.is_closed()
                || self.shared.wake.load(Ordering::Acquire)
                || self.log.changed(&self.page)
                || self.shared.pin.lock().unwrap().is_none()
            {
                return;
            }
            let notified = self.shared.kick.notified();
            if self.shared.wake.load(Ordering::Acquire)
                || self.log.changed(&self.page)
                || self.shared.pin.lock().unwrap().is_none()
            {
                return;
            }
            tokio::select! {
                _ = notified => return,
                _ = tokio::time::sleep(WATCH_EVERY) => {}
            }
        }
    }

    /// 等到 `until`；会话日志一变、或用户点了画面，就提前回去补一帧。
    async fn wait(&self, until: Instant) {
        loop {
            let now = Instant::now();
            if now >= until
                || self.log.changed(&self.page)
                || self.shared.wake.load(Ordering::Acquire)
            {
                return;
            }
            let notified = self.shared.kick.notified();
            let now = Instant::now();
            if now >= until
                || self.log.changed(&self.page)
                || self.shared.wake.load(Ordering::Acquire)
            {
                return;
            }
            tokio::select! {
                _ = notified => return,
                _ = tokio::time::sleep((until - now).min(WATCH_EVERY)) => {}
            }
        }
    }

    /// 动作还落在画面上这个窗口里（知道它在屏幕哪儿）。
    fn on_shown_window(&self, action: &Action) -> bool {
        self.shown.as_ref().is_some_and(|shown| {
            shown.bounds.is_some() && action.window.is_none_or(|w| w == shown.window)
        })
    }

    /// agent 的窗口优先（一直截不到时先跳过）；没有就用（缓存一会儿的）前台窗口。
    async fn target(&mut self) -> Result<Option<Target>, Stop> {
        if !self.skip_agent {
            if let Some((pid, window_id)) = self.log.window {
                return Ok(Some(Target {
                    pid,
                    window_id,
                    source: "agent",
                }));
            }
        }
        if let Some((target, at)) = &self.front {
            if at.elapsed() < FRONT_EVERY {
                return Ok(Some(target.clone()));
            }
        }
        let listed = driver_call(
            &self.page,
            "list_windows",
            json!({ "on_screen_only": true }),
        )
        .await
        .map_err(|_| Stop)?;
        let front = front_window(&windows_in(&listed));
        self.front = front.clone().map(|t| (t, Instant::now()));
        Ok(front)
    }

    /// 只给看的 JPEG：不走辅助功能树，也不登记成可点击的捕获（`display_only`），
    /// 所以不会换掉 agent 的元素快照。像素没变就不往外推。
    async fn capture(&self, target: &Target) -> Result<Result<Shot, &'static str>, Stop> {
        let _guard = self.shared.driver.lock().await;
        let out = match view_call(
            &self.page,
            &self.shared,
            "get_window_state",
            preview_args(target.pid, target.window_id, self.max_dimension),
        )
        .await
        {
            Ok(Some(out)) => out,
            Ok(None) => return Ok(Err("截图超时")),
            Err(_) => return Err(Stop),
        };
        if out.is_error {
            return Ok(Err(capture_reason(&out.content)));
        }
        let Some(image) = out.images.first() else {
            return Ok(Err(capture_reason(&out.content)));
        };
        let info = FrameInfo {
            pid: target.pid,
            window_id: target.window_id,
            width: image.width,
            height: image.height,
        };
        if !self
            .shared
            .note_pixels(target.pid, target.window_id, &image.data)
        {
            self.shared.remember(info);
            return Ok(Ok(Shot::unchanged(info.width, info.height)));
        }
        let shot = Shot {
            data: base64::engine::general_purpose::STANDARD.encode(&image.data),
            mime: image_mime(&image.mime, &image.data),
            width: image.width,
            height: image.height,
            window: window_meta(&out.content, target),
            fresh: true,
        };
        self.shared.remember(info);
        Ok(Ok(shot))
    }

    /// 推一帧，写出去了才回；连接没了回 `None`。顺带记下画面上的窗口。
    /// 钉已经换到别的窗口时，这张是旧的，不推（`Some(())`：连接还在）。
    async fn push_frame(&mut self, seq: u64, target: &Target, shot: Shot) -> Option<()> {
        let pin = *self.shared.pin.lock().unwrap();
        if pin.is_some_and(|p| p != (target.pid, target.window_id)) {
            return Some(());
        }
        self.shown = Some(Shown {
            window: (target.pid, target.window_id),
            bounds: bounds_of(&shot.window["bounds"]),
            app: shot.window["app"].as_str().map(str::to_string),
        });
        let (written, done) = oneshot::channel();
        let note = json!({
            "method": protocol::DESKTOP_VIEW_FRAME,
            "params": {
                "viewId": self.view_id,
                "threadId": self.thread_id,
                "seq": seq,
                "data": shot.data,
                "mime": shot.mime,
                "width": shot.width,
                "height": shot.height,
                "source": target.source,
                "window": shot.window,
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

    /// 推一次光标：问 cua-driver 光标在哪（屏幕坐标），换成画面上这个窗口里的相对位置。
    /// 问不到（按键这类不动光标、或 cua-driver 没回）时 `x` / `y` 为 `null`，客户端留在原处。
    async fn cursor(&self, action: &Action) {
        let Some(shown) = &self.shown else {
            return;
        };
        let at = match (
            self.cursor_point(action.session.as_deref()).await,
            shown.bounds,
        ) {
            (Some(point), Some(bounds)) => Some(relative(point, bounds)),
            _ => None,
        };
        send(
            &self.out,
            json!({
                "method": protocol::DESKTOP_VIEW_CURSOR,
                "params": {
                    "viewId": self.view_id,
                    "threadId": self.thread_id,
                    "windowId": shown.window.1,
                    "x": at.map(|p| p.0),
                    "y": at.map(|p| p.1),
                    "action": action.kind,
                    "label": action.label,
                    "keys": action.keys,
                }
            }),
        );
    }

    async fn cursor_point(&self, session: Option<&str>) -> Option<(f64, f64)> {
        let out = timed_call(
            &self.page,
            "get_agent_cursor_state",
            json!({ "session": session }),
        )
        .await
        .ok()??;
        if out.is_error {
            return None;
        }
        let state = json_in(&out.content)?;
        let at = state.get("position")?;
        Some((at.get("x")?.as_f64()?, at.get("y")?.as_f64()?))
    }

    fn set_status(&mut self, status: Status) {
        if self.status.as_ref() == Some(&status) {
            return;
        }
        let mut params = status.params();
        params["viewId"] = json!(self.view_id);
        params["threadId"] = json!(self.thread_id);
        send(
            &self.out,
            json!({ "method": protocol::DESKTOP_VIEW_STATUS, "params": params }),
        );
        self.status = Some(status);
    }
}

/// cua-driver 不在了：视图收工。
struct Stop;

struct Shot {
    data: String,
    mime: String,
    width: u32,
    height: u32,
    window: Value,
    /// 和上一张不一样，要推给客户端。
    fresh: bool,
}

impl Shot {
    fn unchanged(width: u32, height: u32) -> Self {
        Self {
            data: String::new(),
            mime: String::new(),
            width,
            height,
            window: Value::Null,
            fresh: false,
        }
    }
}

/// 面板预览用的会话。和 agent 自己的 cua 会话分开，避免互相冲掉快照。
const VIEW_SESSION: &str = "dock-view";
/// 预览 JPEG 的质量。远程隧道上比 PNG 小一截，坐标空间不变。
const JPEG_QUALITY: i64 = 70;

/// `gen == 0` 用固定名；之后每次「会话已结束」加一。新名字是普通调用就能建的，
/// 不必 `start_session`（旧名字结束之后，新连接也复活不了它）。
fn session_label(gen: u64) -> String {
    if gen == 0 {
        VIEW_SESSION.to_string()
    } else {
        format!("{VIEW_SESSION}-{gen}")
    }
}

/// 驱动拒绝已结束的具名会话。普通工具调用不会把它重新建起来。
fn session_ended(content: &str) -> bool {
    content.contains("session has ended")
}

/// 参数里带了 `session` 就换成这一视图现在的名字。
fn stamp_session(shared: &Shared, args: &mut Value) {
    if args.get("session").is_some() {
        args["session"] = json!(shared.session_label());
    }
}

/// 带会话名的 cua 调用。名字已被驱动关掉时换一个新的再试一次。
async fn view_call(
    page: &Context,
    shared: &Shared,
    tool: &str,
    mut args: Value,
) -> Result<Option<ToolResult>, String> {
    stamp_session(shared, &mut args);
    let out = timed_call(page, tool, args.clone()).await?;
    if out.as_ref().is_some_and(|got| session_ended(&got.content)) {
        shared.rotate_session();
        stamp_session(shared, &mut args);
        return timed_call(page, tool, args).await;
    }
    Ok(out)
}

/// 只显示的一帧。`display_only` 的图不能拿来点。
fn preview_args(pid: i64, window_id: i64, max_dimension: i64) -> Value {
    json!({
        "pid": pid,
        "window_id": window_id,
        "include_accessibility_tree": false,
        "display_only": true,
        "screenshot_format": "jpeg",
        "screenshot_quality": JPEG_QUALITY,
        "max_dimension": max_dimension,
        "session": VIEW_SESSION,
    })
}

/// 操作前补的一张 PNG：同尺寸，能登记 `capture_id`。
fn ground_args(frame: &FrameInfo) -> Value {
    json!({
        "pid": frame.pid,
        "window_id": frame.window_id,
        "include_accessibility_tree": false,
        "screenshot_format": "png",
        "max_dimension": i64::from(frame.width.max(frame.height)).max(1),
        "session": VIEW_SESSION,
    })
}

struct Ground {
    capture_id: Option<String>,
    width: u32,
    height: u32,
}

/// 给这次点击补一张可定位的 PNG。
async fn ground_frame(
    page: &Context,
    shared: &Shared,
    frame: &FrameInfo,
) -> Result<Ground, String> {
    let out = view_call(page, shared, "get_window_state", ground_args(frame))
        .await?
        .ok_or_else(|| format!("cua-driver {} 秒没有响应", DRIVER_TIMEOUT.as_secs()))?;
    if out.is_error {
        return Err(out.content);
    }
    let image = out.images.first().ok_or("没有定位用的画面")?;
    Ok(Ground {
        capture_id: capture_id_in(&out.content),
        width: image.width,
        height: image.height,
    })
}

/// 后台路由被拒：窗口最小化、藏着，或这套后台注入不可用。只该重试这一下。
fn should_foreground(err: &str) -> bool {
    err.contains("minimized_or_hidden_window")
        || err.contains("background_unavailable")
        || err.contains("routed pointer is refused")
}

fn capture_id_in(text: &str) -> Option<String> {
    if let Some(id) = json_in(text)
        .as_ref()
        .and_then(|v| v.get("capture_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        return Some(id.to_string());
    }
    let rest = text.split("\"capture_id\"").nth(1)?;
    let rest = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    let rest = rest.strip_prefix('"')?;
    let id = rest.split('"').next().unwrap_or("");
    (!id.is_empty()).then(|| id.to_string())
}

/// 定位用的 PNG 和用户看到的 JPEG 尺寸不一样时，把像素按比例挪过去。
fn fit_pixels(args: &mut Value, frame: &FrameInfo, width: u32, height: u32) {
    if width == 0 || height == 0 || (frame.width == width && frame.height == height) {
        return;
    }
    for key in ["x", "y", "from_x", "from_y", "to_x", "to_y"] {
        let Some(n) = args.get(key).and_then(Value::as_i64) else {
            continue;
        };
        let (from, to) = if key.ends_with('x') {
            (frame.width, width)
        } else {
            (frame.height, height)
        };
        args[key] = json!(scale_coord(n, from, to));
    }
}

fn scale_coord(n: i64, from: u32, to: u32) -> i64 {
    if from == 0 || from == to {
        return n;
    }
    let scaled = (n as f64) * f64::from(to) / f64::from(from);
    (scaled.round() as i64).clamp(0, i64::from(to) - 1)
}

fn pixel_hash(data: &[u8]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

fn image_mime(mime: &str, data: &[u8]) -> String {
    if data.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg".to_string()
    } else if data.starts_with(b"\x89PNG") {
        "image/png".to_string()
    } else if mime.starts_with("image/") {
        mime.to_string()
    } else {
        "image/jpeg".to_string()
    }
}

/// 以这页的身份调 cua-driver 的一颗工具，最多等 [`DRIVER_TIMEOUT`]。
/// `Err`：MCP 客户端没挂 / cua-driver 没连上；`Ok(None)`：超时。
async fn timed_call(page: &Context, tool: &str, args: Value) -> Result<Option<ToolResult>, String> {
    let mcp = page
        .get::<Mcp>(MCP)
        .ok_or_else(|| "MCP 客户端没有挂载".to_string())?;
    match tokio::time::timeout(
        DRIVER_TIMEOUT,
        mcp.call_as(page, CUA_DRIVER_SERVER, tool, args),
    )
    .await
    {
        Ok(out) => out.map(Some),
        Err(_) => Ok(None),
    }
}

/// 和 [`driver_call`] 一样，但走 [`view_call`]：预览会话被关掉时先换名字再发。
async fn view_driver_call(
    page: &Context,
    shared: &Shared,
    tool: &str,
    args: Value,
) -> Result<String, String> {
    let out = view_call(page, shared, tool, args)
        .await?
        .ok_or_else(|| format!("cua-driver {} 秒没有响应", DRIVER_TIMEOUT.as_secs()))?;
    if out.is_error {
        return Err(out.content);
    }
    Ok(out.content)
}

/// 以这页的身份调 cua-driver 的一颗工具，回工具文本（MCP 客户端会把 structuredContent
/// 并进文本，超过 8KB 截断）。超时也算失败。
async fn driver_call(page: &Context, tool: &str, args: Value) -> Result<String, String> {
    let out = timed_call(page, tool, args)
        .await?
        .ok_or_else(|| format!("cua-driver {} 秒没有响应", DRIVER_TIMEOUT.as_secs()))?;
    if out.is_error {
        return Err(out.content);
    }
    Ok(out.content)
}

/// 工具文本里的第一个完整 JSON 对象（被截断就没有）。
fn json_in(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
    stream.next()?.ok()
}

/// `list_windows` 文本里的窗口对象，按出现顺序。structuredContent 并进文本时超过 8KB
/// 会被截断：只取截断前完整的那些（cua-driver 按 `z_index` 从前往后排，最前面的在开头）。
fn windows_in(text: &str) -> Vec<Value> {
    let Some(key) = text.find("\"windows\"") else {
        return Vec::new();
    };
    let Some(open) = text[key..].find('[') else {
        return Vec::new();
    };
    let mut rest = &text[key + open + 1..];
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if !rest.starts_with('{') {
            return out;
        }
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
        match stream.next() {
            Some(Ok(window)) => {
                out.push(window);
                rest = &rest[stream.byte_offset()..];
            }
            _ => return out,
        }
    }
}

/// 画面附带的窗口信息：pid / windowId；工具文本里解析得到的话再带上应用名、标题、
/// 窗口位置（点），解析不到（文本被截断）就是 `null`。
fn window_meta(content: &str, target: &Target) -> Value {
    let parsed = json_in(content).unwrap_or(Value::Null);
    json!({
        "pid": target.pid,
        "windowId": target.window_id,
        "app": parsed.get("app_name").cloned().unwrap_or(Value::Null),
        "title": parsed.get("window_title").cloned().unwrap_or(Value::Null),
        "bounds": parsed.get("window_bounds").cloned().unwrap_or(Value::Null),
        "screenshotScale": parsed.get("screenshot_scale").cloned().unwrap_or(Value::Null),
    })
}

/// 数字或数字字符串都认（模型偶尔把 id 写成字符串）。
fn int_arg(v: &Value, key: &str) -> Option<i64> {
    let raw = v.get(key)?;
    raw.as_i64()
        .or_else(|| raw.as_str().and_then(|s| s.trim().parse().ok()))
}

/// 最前面的普通窗口：在屏、不是 Dock 自己 / 系统界面、不太小，取 `z_index` 最大的。
fn front_window(windows: &[Value]) -> Option<Target> {
    windows
        .iter()
        .filter(|w| w.get("is_on_screen").and_then(Value::as_bool) != Some(false))
        .filter(|w| w.get("layer").and_then(Value::as_i64).unwrap_or(0) == 0)
        .filter(|w| {
            let app = w.get("app_name").and_then(Value::as_str).unwrap_or("");
            !SKIP_APPS.contains(&app)
        })
        .filter(|w| {
            let side = |k: &str| {
                w.pointer(&format!("/bounds/{k}"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
            };
            side("width") >= 200.0 && side("height") >= 150.0
        })
        .filter_map(|w| Some((w.get("z_index")?.as_i64()?, w)))
        .max_by_key(|(z, _)| *z)
        .and_then(|(_, w)| {
            Some(Target {
                pid: w.get("pid")?.as_i64()?,
                window_id: w.get("window_id")?.as_i64()?,
                source: "front",
            })
        })
}

fn send(out: &OutTx, note: Value) {
    let _ = out.send(Outgoing::Text(note.to_string()));
}

/// 窗口列表给面板挑：在屏、不是 Dock 自己 / 系统界面，最前面的排前面。
fn window_choices(windows: &[Value]) -> Vec<Value> {
    let mut rows: Vec<(i64, Value)> = windows
        .iter()
        .filter(|w| w.get("is_on_screen").and_then(Value::as_bool) != Some(false))
        .filter(|w| w.get("layer").and_then(Value::as_i64).unwrap_or(0) == 0)
        .filter(|w| {
            let app = w.get("app_name").and_then(Value::as_str).unwrap_or("");
            !SKIP_APPS.contains(&app)
        })
        .filter_map(|w| {
            let pid = w.get("pid")?.as_i64()?;
            let window_id = w.get("window_id")?.as_i64()?;
            let z = w.get("z_index").and_then(Value::as_i64).unwrap_or(0);
            let title = ["title", "window_title", "name"]
                .iter()
                .find_map(|k| w.get(*k).and_then(Value::as_str))
                .filter(|s| !s.is_empty());
            Some((
                z,
                json!({
                    "pid": pid,
                    "windowId": window_id,
                    "app": w.get("app_name").cloned().unwrap_or(Value::Null),
                    "title": title.map(|s| Value::String(s.to_string())).unwrap_or(Value::Null),
                    "zIndex": z,
                }),
            ))
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.0));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// 画面上的一次操作 → cua-driver 调用。坐标是这一帧的像素，不加成屏幕坐标。
fn user_gesture(event: &Value, frame: &FrameInfo) -> Result<(String, Value), &'static str> {
    let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
    let mut args = json!({
        "pid": frame.pid,
        "window_id": frame.window_id,
        "session": VIEW_SESSION,
        "post_action_wait": "none",
    });
    match kind {
        "click" => {
            let (x, y) = frame_point(event, "x", "y", frame)?;
            args["x"] = json!(x);
            args["y"] = json!(y);
            let button = event
                .get("button")
                .and_then(Value::as_str)
                .unwrap_or("left");
            let count = event.get("count").and_then(Value::as_i64).unwrap_or(1);
            let tool = if button == "right" {
                "right_click"
            } else if count >= 2 {
                "double_click"
            } else {
                "click"
            };
            Ok((tool.to_string(), args))
        }
        "drag" => {
            let (x, y) = frame_point(event, "x", "y", frame)?;
            let (to_x, to_y) = frame_point(event, "toX", "toY", frame)?;
            args["from_x"] = json!(x);
            args["from_y"] = json!(y);
            args["to_x"] = json!(to_x);
            args["to_y"] = json!(to_y);
            Ok(("drag".to_string(), args))
        }
        "scroll" => {
            let (x, y) = frame_point(event, "x", "y", frame)?;
            let dx = event.get("dx").and_then(Value::as_f64).unwrap_or(0.0);
            let dy = event.get("dy").and_then(Value::as_f64).unwrap_or(0.0);
            if dx == 0.0 && dy == 0.0 {
                return Err("滚动距离是 0");
            }
            let (direction, amount) = if dy.abs() >= dx.abs() {
                (if dy > 0.0 { "down" } else { "up" }, notches(dy))
            } else {
                (if dx > 0.0 { "right" } else { "left" }, notches(dx))
            };
            args["x"] = json!(x);
            args["y"] = json!(y);
            args["direction"] = json!(direction);
            args["amount"] = json!(amount);
            Ok(("scroll".to_string(), args))
        }
        "text" => {
            let text = event.get("text").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() {
                return Err("没有要输入的文字");
            }
            if text.chars().count() > 4000 {
                return Err("一次输入太长");
            }
            args["text"] = json!(text);
            if event.get("x").is_some() || event.get("y").is_some() {
                let (x, y) = frame_point(event, "x", "y", frame)?;
                args["x"] = json!(x);
                args["y"] = json!(y);
            }
            Ok(("type_text".to_string(), args))
        }
        "key" => {
            let raw = event.get("key").and_then(Value::as_str).unwrap_or("");
            let key = driver_key(raw).ok_or("不认识的按键")?;
            let mods: Vec<String> = event
                .get("modifiers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(modifier_name)
                .map(str::to_string)
                .collect();
            if mods.is_empty() {
                args["key"] = json!(key);
                Ok(("press_key".to_string(), args))
            } else {
                let mut keys = mods;
                keys.push(key);
                args["keys"] = json!(keys);
                Ok(("hotkey".to_string(), args))
            }
        }
        _ => Err("不认识的操作"),
    }
}

fn frame_point(
    event: &Value,
    xkey: &str,
    ykey: &str,
    frame: &FrameInfo,
) -> Result<(i64, i64), &'static str> {
    let x = event.get(xkey).and_then(Value::as_f64).ok_or("坐标无效")?;
    let y = event.get(ykey).and_then(Value::as_f64).ok_or("坐标无效")?;
    let w = f64::from(frame.width);
    let h = f64::from(frame.height);
    if frame.width == 0 || frame.height == 0 || !(0.0..=w).contains(&x) || !(0.0..=h).contains(&y) {
        return Err("坐标超出画面");
    }
    Ok((
        (x.round() as i64).clamp(0, i64::from(frame.width) - 1),
        (y.round() as i64).clamp(0, i64::from(frame.height) - 1),
    ))
}

/// 触控板一次滑动的像素很多，并成一次滚动后再换算。格数太小会显得一点一点挪。
fn notches(delta: f64) -> i64 {
    ((delta.abs() / 16.0).round() as i64).clamp(1, 48)
}

fn function_key(key: &str) -> Option<String> {
    let rest = key.strip_prefix('F').or_else(|| key.strip_prefix('f'))?;
    let n: u8 = rest.parse().ok()?;
    (1..=12).contains(&n).then(|| format!("f{n}"))
}

fn driver_key(key: &str) -> Option<String> {
    if let Some(key) = function_key(key) {
        return Some(key);
    }
    let mapped = match key {
        "Enter" => "return",
        "Backspace" => "backspace",
        "Escape" => "escape",
        "Tab" => "tab",
        "ArrowUp" => "up",
        "ArrowDown" => "down",
        "ArrowLeft" => "left",
        "ArrowRight" => "right",
        "Delete" => "delete",
        " " | "Space" => "space",
        "Home" => "home",
        "End" => "end",
        "PageUp" => "pageup",
        "PageDown" => "pagedown",
        k if k.chars().count() == 1 => return Some(k.to_string()),
        _ => return None,
    };
    Some(mapped.to_string())
}

/// 客户端带了它点的那一帧的窗口或尺寸，和现在这帧对不上就不该点。
/// 没带（旧客户端）就照现在这帧做。
fn frame_changed(event: &Value, frame: &FrameInfo) -> bool {
    let differs = |key: &str, have: i64| int_arg(event, key).is_some_and(|v| v != have);
    differs("pid", frame.pid)
        || differs("windowId", frame.window_id)
        || differs("width", i64::from(frame.width))
        || differs("height", i64::from(frame.height))
}

fn modifier_name(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "cmd" | "command" | "meta" => Some("cmd"),
        "ctrl" | "control" => Some("ctrl"),
        "alt" | "option" => Some("alt"),
        "shift" => Some("shift"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(name: &str, args: Value) -> LogEvent {
        LogEvent::ToolExecute {
            id: "c".into(),
            name: name.into(),
            arguments: args.to_string(),
            content: String::new(),
            images: Vec::new(),
            is_error: false,
        }
    }

    #[test]
    fn agent_window_is_the_last_cua_call_with_a_window() {
        let events = vec![
            exec(
                "mcp_cua-driver__click",
                json!({ "pid": 1, "window_id": 10, "x": 5, "y": 5 }),
            ),
            exec(
                "use_tool",
                json!({ "tool_name": "mcp_cua-driver__type_text", "tool_input": { "pid": "2", "window_id": 20 } }),
            ),
            // 没带窗口的、别的工具的都跳过。
            exec("mcp_cua-driver__list_windows", json!({})),
            exec("bash", json!({ "pid": 9, "window_id": 99 })),
        ];
        assert_eq!(last_cua_window(&events), Some((2, 20)));
        assert_eq!(last_cua_window(&events[..1]), Some((1, 10)));
        assert_eq!(last_cua_window(&[]), None);
    }

    /// 打开视图之前的历史不算新动作；之后的才推光标。压缩重写过的日志从头当历史。
    #[test]
    fn scan_reports_only_actions_after_what_was_seen() {
        let mut events = vec![exec(
            "mcp_cua-driver__click",
            json!({ "pid": 1, "window_id": 10, "x": 5, "y": 5 }),
        )];
        let first = scan(&events, None);
        assert_eq!(first.window, Some((1, 10)));
        assert!(first.actions.is_empty());
        assert_eq!(first.len, 1);

        events.push(exec(
            "mcp_cua-driver__get_window_state",
            json!({ "pid": 1, "window_id": 10 }),
        ));
        events.push(exec(
            "use_tool",
            json!({ "tool_name": "mcp_cua-driver__double_click", "tool_input": { "pid": 1, "window_id": 10, "element_token": "s1:3" } }),
        ));
        let next = scan(&events, Some(first.len));
        let kinds: Vec<_> = next.actions.iter().map(|a| a.kind).collect();
        assert_eq!(kinds, vec!["double_click"], "截图不是动作");
        assert_eq!(next.actions[0].window, Some((1, 10)));

        assert!(scan(&events[..1], Some(3)).actions.is_empty());
    }

    #[test]
    fn cua_action_maps_tools_and_labels() {
        let right = cua_action("click", &json!({ "button": "right", "pid": 1 })).unwrap();
        assert_eq!(right.kind, "right_click");
        assert_eq!(right.window, None, "没带 window_id 就是当前窗口");

        let typed = cua_action(
            "type_text",
            &json!({ "text": "一二三四五六七八九十十一十二十三", "session": "run" }),
        )
        .unwrap();
        assert_eq!(typed.kind, "type");
        assert_eq!(typed.label.as_deref(), Some("一二三四五六七八九十十一…"));
        assert_eq!(typed.session.as_deref(), Some("run"));

        let key = cua_action(
            "press_key",
            &json!({ "key": "n", "modifiers": ["cmd", "shift"] }),
        )
        .unwrap();
        assert_eq!(key.keys, vec!["cmd", "shift", "n"]);
        let hot = cua_action("hotkey", &json!({ "keys": ["cmd", "w"] })).unwrap();
        assert_eq!(
            (hot.kind, hot.keys.clone()),
            ("key", vec!["cmd".into(), "w".into()])
        );

        let scroll = cua_action("scroll", &json!({ "direction": "down" })).unwrap();
        assert_eq!(scroll.label.as_deref(), Some("down"));

        for still in [
            "get_window_state",
            "list_windows",
            "launch_app",
            "move_cursor",
        ] {
            assert!(cua_action(still, &json!({})).is_none(), "{still}");
        }
    }

    #[test]
    fn cursor_position_is_relative_to_the_window() {
        let window =
            bounds_of(&json!({ "x": 1104.0, "y": 432.0, "width": 230.0, "height": 408.0 }))
                .unwrap();
        // 计算器按钮「1」中心（实测 get_agent_cursor_state 回的屏幕坐标）。
        let (x, y) = relative((1138.0, 751.0), window);
        assert!((x - 34.0 / 230.0).abs() < 1e-9 && (y - 319.0 / 408.0).abs() < 1e-9);
        assert!(bounds_of(&json!({ "x": 0, "y": 0, "width": 0, "height": 10 })).is_none());
        assert!(bounds_of(&Value::Null).is_none());
    }

    #[test]
    fn capture_failures_say_why() {
        assert_eq!(capture_reason("refused: window_id_not_found"), "窗口已关闭");
        assert_eq!(
            capture_reason("px_capture_unavailable"),
            "这个窗口现在截不到画面"
        );
        assert_eq!(capture_reason("boom"), "截图失败");
        assert!(session_ended(
            "session has ended; tool call 'get_window_state' was rejected. Call start_session with session 'dock-view'"
        ));
        assert!(!session_ended("window_id_not_found"));
        assert_eq!(capture_reason("session has ended"), "预览会话断了");
        assert_eq!(
            failure_message(Some("Grok Bot"), "窗口已关闭"),
            "截不到「Grok Bot」：窗口已关闭 · 正在重试"
        );
        assert_eq!(failure_message(None, "截图超时"), "截图超时 · 正在重试");
        assert_eq!(
            Status::NoWindow.params()["state"],
            "no_window",
            "{}",
            Status::NoWindow.params()
        );
    }

    /// 同一会话连发两次 open：按收到的顺序，只有后收到的那次算数；别的会话、别的方法不受影响。
    #[test]
    fn only_the_last_received_open_counts() {
        let views = DesktopViews::default();
        let a = json!({ "threadId": "t1" });
        let DesktopOrder::Open(first) = views.order(protocol::DESKTOP_VIEW_OPEN, &a) else {
            panic!("open");
        };
        let DesktopOrder::Open(second) = views.order(protocol::DESKTOP_VIEW_OPEN, &a) else {
            panic!("open");
        };
        assert!(!views.is_latest("t1", first), "晚做完的旧 open 作废");
        assert!(views.is_latest("t1", second));
        let DesktopOrder::Open(other) =
            views.order(protocol::DESKTOP_VIEW_OPEN, &json!({ "threadId": "t2" }))
        else {
            panic!("open");
        };
        assert!(views.is_latest("t2", other) && views.is_latest("t1", second));
        assert!(matches!(
            views.order(protocol::DESKTOP_VIEW_CLOSE, &a),
            DesktopOrder::None
        ));
        assert!(views.is_latest("t1", 0), "没定序的照旧");
    }

    #[test]
    fn pin_needs_both_ids() {
        assert_eq!(pin_param(&json!({})).unwrap(), None);
        assert_eq!(
            pin_param(&json!({ "pid": 2, "windowId": 9 })).unwrap(),
            Some((2, 9))
        );
        assert!(pin_param(&json!({ "pid": 2 })).is_err());
    }

    #[test]
    fn window_choices_skip_dock_and_put_the_front_first() {
        let listed = json!([
            { "pid": 1, "window_id": 11, "app_name": "dock-gui", "title": "Dock", "z_index": 30, "is_on_screen": true, "layer": 0 },
            { "pid": 2, "window_id": 22, "app_name": "Safari", "title": "文档", "z_index": 10, "is_on_screen": true, "layer": 0 },
            { "pid": 3, "window_id": 33, "app_name": "Notes", "window_title": "明天", "z_index": 20, "is_on_screen": true, "layer": 0 }
        ]);
        let rows = window_choices(listed.as_array().unwrap());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["windowId"], 33, "z 大的在前");
        assert_eq!(rows[0]["title"], "明天");
        assert_eq!(rows[1]["app"], "Safari");
    }

    #[test]
    fn user_gesture_maps_frame_pixels_onto_the_shown_window() {
        let frame = FrameInfo {
            pid: 4,
            window_id: 8,
            width: 200,
            height: 100,
        };
        let (tool, args) = user_gesture(
            &json!({ "type": "click", "x": 10.2, "y": 20.6, "count": 2 }),
            &frame,
        )
        .unwrap();
        assert_eq!(tool, "double_click");
        assert_eq!(args["pid"], 4);
        assert_eq!(args["window_id"], 8);
        assert_eq!(args["x"], 10);
        assert_eq!(args["y"], 21);
        assert_eq!(args["session"], "dock-view");
        assert_eq!(args["post_action_wait"], "none");

        let (tool, _) = user_gesture(
            &json!({ "type": "click", "x": 1, "y": 1, "button": "right" }),
            &frame,
        )
        .unwrap();
        assert_eq!(tool, "right_click");

        assert!(user_gesture(&json!({ "type": "click", "x": 500, "y": 1 }), &frame).is_err());

        let (_, drag) = user_gesture(
            &json!({ "type": "drag", "x": 1, "y": 2, "toX": 30, "toY": 40 }),
            &frame,
        )
        .unwrap();
        assert_eq!(drag["from_x"], 1);
        assert_eq!(drag["to_y"], 40);

        let (_, scroll) = user_gesture(
            &json!({ "type": "scroll", "x": 5, "y": 5, "dx": 0, "dy": 90 }),
            &frame,
        )
        .unwrap();
        assert_eq!(scroll["direction"], "down");
        assert_eq!(scroll["amount"], 6);

        let (_, typed) = user_gesture(&json!({ "type": "text", "text": "你好" }), &frame).unwrap();
        assert_eq!(typed["text"], "你好");
        assert!(typed.get("x").is_none(), "没点过就不带坐标");

        let (tool, key) = user_gesture(
            &json!({ "type": "key", "key": "Enter", "modifiers": ["meta", "shift"] }),
            &frame,
        )
        .unwrap();
        assert_eq!(tool, "hotkey");
        assert_eq!(key["keys"], json!(["cmd", "shift", "return"]));

        let (tool, key) = user_gesture(&json!({ "type": "key", "key": "a" }), &frame).unwrap();
        assert_eq!(tool, "press_key");
        assert_eq!(key["key"], "a");
        let (tool, key) = user_gesture(&json!({ "type": "key", "key": "F5" }), &frame).unwrap();
        assert_eq!(tool, "press_key");
        assert_eq!(key["key"], "f5");
        let (tool, key) = user_gesture(
            &json!({ "type": "key", "key": "Enter", "modifiers": ["meta"] }),
            &frame,
        )
        .unwrap();
        assert_eq!(tool, "hotkey");
        assert_eq!(key["keys"], json!(["cmd", "return"]));
        assert!(!takes_capture_id("type_text"));
        assert!(!takes_capture_id("press_key"));
        assert!(!takes_capture_id("hotkey"));
        assert!(takes_capture_id("click"));
        assert!(takes_capture_id("scroll"));
    }

    #[test]
    fn a_click_for_another_frame_is_refused() {
        let frame = FrameInfo {
            pid: 4,
            window_id: 8,
            width: 200,
            height: 100,
        };
        assert!(!frame_changed(
            &json!({ "type": "click", "x": 1, "y": 1 }),
            &frame
        ));
        assert!(!frame_changed(
            &json!({ "pid": 4, "windowId": 8, "width": 200, "height": 100 }),
            &frame
        ));
        assert!(frame_changed(&json!({ "pid": 5, "windowId": 8 }), &frame));
        assert!(frame_changed(
            &json!({ "width": 199, "height": 100 }),
            &frame
        ));
    }

    #[test]
    fn remember_frame_drops_a_shot_of_the_window_we_left() {
        let shared = Shared::new(Some((1, 2)));
        shared.remember(FrameInfo {
            pid: 9,
            window_id: 9,
            width: 10,
            height: 10,
        });
        assert!(shared.frame.lock().unwrap().is_none(), "钉已经是别的窗口");
        shared.remember(FrameInfo {
            pid: 1,
            window_id: 2,
            width: 10,
            height: 10,
        });
        assert_eq!(shared.frame.lock().unwrap().unwrap().pid, 1);
        shared.set_pin(Some((3, 4)));
        assert!(shared.frame.lock().unwrap().is_none(), "换钉丢掉上一帧");
        shared.remember(FrameInfo {
            pid: 1,
            window_id: 2,
            width: 10,
            height: 10,
        });
        assert!(shared.frame.lock().unwrap().is_none());
    }

    #[test]
    fn preview_is_a_display_only_jpeg_on_its_own_session() {
        let mut args = preview_args(4, 8, 1280);
        assert_eq!(args["display_only"], true);
        assert_eq!(args["include_accessibility_tree"], false);
        assert_eq!(args["screenshot_format"], "jpeg");
        assert_eq!(args["session"], VIEW_SESSION);
        assert_eq!(args["max_dimension"], 1280);
        let shared = Shared::new(None);
        assert_eq!(shared.session_label(), VIEW_SESSION);
        assert_eq!(shared.rotate_session(), "dock-view-1");
        stamp_session(&shared, &mut args);
        assert_eq!(args["session"], "dock-view-1");
        let ground = ground_args(&FrameInfo {
            pid: 4,
            window_id: 8,
            width: 800,
            height: 600,
        });
        assert_eq!(ground["screenshot_format"], "png");
        assert!(ground.get("display_only").is_none());
        assert_eq!(ground["max_dimension"], 800);
    }

    #[test]
    fn capture_id_and_foreground_refusal_are_read_from_the_driver_text() {
        assert_eq!(
            capture_id_in("noise {\"capture_id\":\"cap-1\",\"ok\":true}"),
            Some("cap-1".to_string())
        );
        assert_eq!(capture_id_in("no id here"), None);
        assert!(should_foreground(
            "Background input refused (minimized_or_hidden_window)"
        ));
        assert!(should_foreground("background_unavailable"));
        assert!(!should_foreground("窗口已关闭"));
    }

    #[test]
    fn pixels_are_scaled_onto_the_grounding_png_and_identical_frames_are_not_fresh() {
        let frame = FrameInfo {
            pid: 1,
            window_id: 2,
            width: 100,
            height: 50,
        };
        let mut args = json!({ "x": 50, "y": 25, "to_x": 100, "to_y": 0 });
        fit_pixels(&mut args, &frame, 200, 100);
        assert_eq!(args["x"], 100);
        assert_eq!(args["y"], 50);
        assert_eq!(args["to_x"], 199);
        assert_eq!(args["to_y"], 0);
        fit_pixels(&mut args, &frame, 100, 50);
        assert_eq!(args["x"], 100, "尺寸没变就不动");

        let shared = Shared::new(None);
        assert!(shared.note_pixels(1, 2, b"abc"));
        assert!(!shared.note_pixels(1, 2, b"abc"));
        assert!(shared.note_pixels(1, 2, b"abd"));
        shared.set_pin(Some((9, 9)));
        assert!(shared.note_pixels(1, 2, b"abd"), "换钉后同一张也要再推");
    }

    #[test]
    fn front_window_skips_dock_the_cursor_overlay_and_tiny_windows() {
        let listed = json!([
            { "pid": 1, "window_id": 11, "app_name": "dock-gui", "z_index": 30, "is_on_screen": true, "layer": 0, "bounds": { "width": 1440.0, "height": 900.0 } },
            { "pid": 5, "window_id": 55, "app_name": "Cua Driver", "z_index": 40, "is_on_screen": true, "layer": 0, "bounds": { "width": 1728.0, "height": 1117.0 } },
            { "pid": 2, "window_id": 22, "app_name": "Safari", "z_index": 20, "is_on_screen": true, "layer": 0, "bounds": { "width": 1200.0, "height": 800.0 } },
            { "pid": 3, "window_id": 33, "app_name": "Notes", "z_index": 25, "is_on_screen": true, "layer": 0, "bounds": { "width": 120.0, "height": 40.0 } },
            { "pid": 4, "window_id": 44, "app_name": "Mail", "z_index": 10, "is_on_screen": false, "layer": 0, "bounds": { "width": 1200.0, "height": 800.0 } }
        ]);
        let front = front_window(listed.as_array().unwrap()).unwrap();
        assert_eq!((front.pid, front.window_id, front.source), (2, 22, "front"));
        assert_eq!(front_window(&[]), None);
    }

    /// structuredContent 并进文本时被截到 8KB：截断前完整的窗口照样认出来。
    #[test]
    fn windows_in_reads_complete_windows_before_truncation() {
        let text = "Found 3 window(s).\n{\"current_space_id\":1,\"windows\":[{\"pid\":1,\"window_id\":10}, {\"pid\":2,\"window_id\":20},{\"pid\":3,\"wind…(truncated)";
        let windows = windows_in(text);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[1]["window_id"], 20);
        assert!(windows_in("Found 0 window(s).").is_empty());
    }

    #[test]
    fn json_in_finds_the_object_after_a_preamble() {
        let text = "Found 2 window(s).\n{\"windows\": [{\"pid\": 1}]}";
        assert_eq!(json_in(text).unwrap()["windows"][0]["pid"], 1);
        assert!(json_in("no json here").is_none());
    }
}
