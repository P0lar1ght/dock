//! `desktop/view/*`：桌面（CUA）面板的实时画面——agent 正在操作的那个窗口。
//!
//! - `desktop/view/open { threadId?, maxDimension? }` → `{ viewId, threadId }`。之后这条
//!   连接收 `desktop/view/frame` / `cursor` / `status` / `closed` 推送。
//! - `desktop/view/close { viewId }`；连接断开时自动全关。
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use cordis::Context;
use cordis_base::cua::CUA_DRIVER_SERVER;
use cordis_base::types::LogEvent;
use cordis_spine::{Mcp, Sessions, ToolResult, MCP, SESSIONS};
use serde_json::{json, Value};
use tokio::sync::oneshot;
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
}

impl DesktopViews {
    /// 在连接的读循环里、按收到的顺序同步调用（不能挪进各自的任务里：任务开跑的先后
    /// 不保证）。`open` 记下序号并返回；别的方法回 0。
    ///
    /// 同一会话连发两次 `open`（React StrictMode 双挂载、快速重开面板）时，两个任务谁先
    /// 做完不一定：晚做完的旧 `open` 不能把新的顶掉——客户端早就不要它了，回头把它一关，
    /// 这个会话就一个视图都不剩，画面永远停在「正在连接」。
    pub(crate) fn order(&self, method: &str, params: &Value) -> u64 {
        if method != protocol::DESKTOP_VIEW_OPEN {
            return 0;
        }
        let seq = self.next_open.fetch_add(1, Ordering::Relaxed) + 1;
        self.latest_open
            .lock()
            .unwrap()
            .insert(threads::thread_param(params), seq);
        seq
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
}

struct Entry {
    thread_id: String,
    task: JoinHandle<()>,
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
        protocol::DESKTOP_VIEW_OPEN | protocol::DESKTOP_VIEW_CLOSE
    )
}

pub(crate) async fn dispatch(
    gateway: &GatewayHandle,
    views: &Arc<DesktopViews>,
    out: &OutTx,
    method: &str,
    params: Value,
    seq: u64,
) -> Result<Value, RpcError> {
    match method {
        protocol::DESKTOP_VIEW_OPEN => open(gateway, views, out, params, seq).await,
        protocol::DESKTOP_VIEW_CLOSE => {
            let id = params
                .get("viewId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| RpcError::invalid_params("viewId is required"))?;
            let entry = views
                .open
                .lock()
                .unwrap()
                .remove(id)
                .ok_or_else(|| RpcError::app("not_found", format!("没有视图 {id}")))?;
            entry.task.abort();
            Ok(json!({ "ok": true, "viewId": id }))
        }
        _ => Err(RpcError::method_not_found(method)),
    }
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
    let view_id = uuid::Uuid::new_v4().simple().to_string();
    let pump = Pump {
        view_id: view_id.clone(),
        thread_id: thread_id.clone(),
        page: page.ctx,
        out: out.clone(),
        max_dimension,
        log: AgentLog::default(),
        front: None,
        shown: None,
        status: None,
        agent_fails: 0,
        fails: 0,
        skip_agent: false,
        active_until: None,
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
            },
        );
        stale
    };
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
    Ok(json!({ "viewId": view_id, "threadId": thread_id }))
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
    if content.contains("window_id_not_found") || content.contains("window_owner_pid_mismatch") {
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
            views.open.lock().unwrap().remove(&self.view_id);
        }
    }

    /// 返回结束原因；`None` 是连接没了（没人可告诉）。
    async fn pump(&mut self) -> Option<&'static str> {
        let mut seq: u64 = 0;
        loop {
            if self.out.is_closed() {
                return None;
            }
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
                    seq += 1;
                    self.push_frame(seq, &target, shot).await?;
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

    /// 等到 `until`；会话日志一变（agent 又动手了）就提前回去补一帧。
    async fn wait(&self, until: Instant) {
        loop {
            let now = Instant::now();
            if now >= until || self.log.changed(&self.page) {
                return;
            }
            tokio::time::sleep((until - now).min(WATCH_EVERY)).await;
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

    /// 只截图的 `get_window_state`。截不到回 `Ok(Err(原因))`。
    async fn capture(&self, target: &Target) -> Result<Result<Shot, &'static str>, Stop> {
        let out = match timed_call(
            &self.page,
            "get_window_state",
            json!({
                "pid": target.pid,
                "window_id": target.window_id,
                "include_accessibility_tree": false,
                "max_dimension": self.max_dimension,
            }),
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
        Ok(Ok(Shot {
            data: base64::engine::general_purpose::STANDARD.encode(&image.data),
            mime: image.mime.clone(),
            width: image.width,
            height: image.height,
            window: window_meta(&out.content, target),
        }))
    }

    /// 推一帧，写出去了才回；连接没了回 `None`。顺带记下画面上的窗口。
    async fn push_frame(&mut self, seq: u64, target: &Target, shot: Shot) -> Option<()> {
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
        let first = views.order(protocol::DESKTOP_VIEW_OPEN, &a);
        let second = views.order(protocol::DESKTOP_VIEW_OPEN, &a);
        assert!(!views.is_latest("t1", first), "晚做完的旧 open 作废");
        assert!(views.is_latest("t1", second));
        let other = views.order(protocol::DESKTOP_VIEW_OPEN, &json!({ "threadId": "t2" }));
        assert!(views.is_latest("t2", other) && views.is_latest("t1", second));
        assert_eq!(views.order(protocol::DESKTOP_VIEW_CLOSE, &a), 0);
        assert!(views.is_latest("t1", 0), "没定序的照旧");
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
