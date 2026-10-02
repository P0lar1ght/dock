//! `desktop/view/*`：桌面（CUA）面板的实时画面——agent 正在操作的那个窗口。
//!
//! - `desktop/view/open { threadId?, maxDimension? }` → `{ viewId, threadId }`。之后这条
//!   连接收 `desktop/view/frame` / `desktop/view/closed` 推送。
//! - `desktop/view/close { viewId }`；连接断开时自动全关。
//!
//! 看哪个窗口：这个会话最近一次 cua-driver 调用里带的 `pid` + `window_id`（agent 正在操作
//! 的那个，`source: "agent"`）；还没有就取最前面的普通窗口（`list_windows` 里 `z_index`
//! 最大的，跳过 Dock 自己，`source: "front"`）。
//!
//! 截图走 cua-driver 的 `get_window_state { include_accessibility_tree: false }`：它给实时
//! 预览留的「只截图」路径，不遍历 UI 树。以那页的身份调（[`Mcp::call_as`]），不进对话流、
//! 不过权限门——只读，和用户自己看一眼屏幕一样。
//!
//! 节奏：上一帧写出去了才截下一帧，帧间隔不短于 [`FRAME_EVERY`]，慢客户端不攒帧。
//! cua-driver 没连上推 `closed { reason: "driver_unavailable" }`。
//!
//! 视图是**连接级**的：不进会话、不落盘，只活在这条 WebSocket 上。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use cordis::Context;
use cordis_base::cua::CUA_DRIVER_SERVER;
use cordis_base::types::LogEvent;
use cordis_spine::{Mcp, Sessions, MCP, SESSIONS};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::handle::GatewayHandle;
use crate::protocol::{self, RpcError};
use crate::threads;
use crate::ws::{OutTx, Outgoing};

/// 两帧之间至少隔这么久（约 4 帧/秒）。
const FRAME_EVERY: Duration = Duration::from_millis(250);
/// 没有可看的窗口 / 截图失败时，多久再试。
const IDLE_EVERY: Duration = Duration::from_millis(800);
/// 前台窗口（`list_windows`）多久重新挑一次。
const FRONT_EVERY: Duration = Duration::from_secs(2);
/// 画面最长边缺省值（像素）。
const DEFAULT_MAX_DIMENSION: i64 = 1280;
/// cua-driver 的工具名前缀（MCP 公开名）。
const CUA_PREFIX: &str = "mcp_cua-driver__";
/// 挑前台窗口时跳过的：Dock 自己、系统界面。
const SKIP_APPS: &[&str] = &[
    "Dock",
    "dock-gui",
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
) -> Result<Value, RpcError> {
    match method {
        protocol::DESKTOP_VIEW_OPEN => open(gateway, views, out, params).await,
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
) -> Result<Value, RpcError> {
    let thread_id = threads::thread_param(&params);
    let page = threads::resolve(gateway, &thread_id)?;
    // 先确认 cua-driver 连着，免得回了 viewId 才推 closed。
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
        agent: AgentWindow::default(),
        front: None,
    };
    // 同一条连接对同一个会话只留一个视图：新的顶掉旧的。
    let stale: Vec<(String, Entry)> = {
        let mut open = views.open.lock().unwrap();
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

/// 会话日志里 agent 最近操作的窗口，按日志版本缓存，日志没变就不重扫。
#[derive(Default)]
struct AgentWindow {
    rev: Option<u64>,
    found: Option<(i64, i64)>,
}

impl AgentWindow {
    fn get(&mut self, page: &Context) -> Option<(i64, i64)> {
        let sessions = page.get::<Sessions>(SESSIONS)?;
        let rev = sessions.events_rev();
        if self.rev != Some(rev) {
            self.rev = Some(rev);
            self.found = sessions.with_log(|events, _| last_cua_window(events));
        }
        self.found
    }
}

struct Pump {
    view_id: String,
    thread_id: String,
    page: Context,
    out: OutTx,
    max_dimension: i64,
    agent: AgentWindow,
    /// 上次挑的前台窗口和挑的时间。
    front: Option<(Target, Instant)>,
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
            let target = match self.target().await {
                Ok(Some(target)) => target,
                Ok(None) => {
                    tokio::time::sleep(IDLE_EVERY).await;
                    continue;
                }
                Err(Stop) => return Some("driver_unavailable"),
            };
            let shot = match self.capture(&target).await {
                Ok(Some(shot)) => shot,
                Ok(None) => {
                    // 窗口没了 / 这一下没截到：前台窗口重新挑，下一轮再试。
                    self.front = None;
                    tokio::time::sleep(IDLE_EVERY).await;
                    continue;
                }
                Err(Stop) => return Some("driver_unavailable"),
            };
            seq += 1;
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
            let sent = self.out.send(Outgoing::Frame {
                text: note.to_string(),
                written,
            });
            // 写出去了才截下一帧；连接没了就收工。
            if sent.is_err() || done.await.is_err() {
                return None;
            }
            let spent = started.elapsed();
            if spent < FRAME_EVERY {
                tokio::time::sleep(FRAME_EVERY - spent).await;
            }
        }
    }

    /// agent 操作过的窗口优先；没有就用（缓存一会儿的）前台窗口。
    async fn target(&mut self) -> Result<Option<Target>, Stop> {
        if let Some((pid, window_id)) = self.agent.get(&self.page) {
            return Ok(Some(Target {
                pid,
                window_id,
                source: "agent",
            }));
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

    /// 只截图的 `get_window_state`。截不到（窗口没了、这一下失败）回 `Ok(None)`。
    async fn capture(&self, target: &Target) -> Result<Option<Shot>, Stop> {
        let mcp = self.page.get::<Mcp>(MCP).ok_or(Stop)?;
        let out = mcp
            .call_as(
                &self.page,
                CUA_DRIVER_SERVER,
                "get_window_state",
                json!({
                    "pid": target.pid,
                    "window_id": target.window_id,
                    "include_accessibility_tree": false,
                    "max_dimension": self.max_dimension,
                }),
            )
            .await
            .map_err(|_| Stop)?;
        if out.is_error {
            return Ok(None);
        }
        let Some(image) = out.images.first() else {
            return Ok(None);
        };
        Ok(Some(Shot {
            data: base64::engine::general_purpose::STANDARD.encode(&image.data),
            mime: image.mime.clone(),
            width: image.width,
            height: image.height,
            window: window_meta(&out.content, target),
        }))
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

/// 以这页的身份调 cua-driver 的一颗工具，回工具文本（MCP 客户端会把 structuredContent
/// 并进文本，超过 8KB 截断）。
async fn driver_call(page: &Context, tool: &str, args: Value) -> Result<String, String> {
    let mcp = page
        .get::<Mcp>(MCP)
        .ok_or_else(|| "MCP 客户端没有挂载".to_string())?;
    let out = mcp.call_as(page, CUA_DRIVER_SERVER, tool, args).await?;
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

/// 日志里最后一次带 `pid` + `window_id` 的 cua-driver 调用（直调或经 `use_tool`）。
fn last_cua_window(events: &[LogEvent]) -> Option<(i64, i64)> {
    events.iter().rev().find_map(|event| {
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
        if !tool.starts_with(CUA_PREFIX) {
            return None;
        }
        Some((int_arg(&input, "pid")?, int_arg(&input, "window_id")?))
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

    #[test]
    fn front_window_skips_dock_and_tiny_windows() {
        let listed = json!([
            { "pid": 1, "window_id": 11, "app_name": "dock-gui", "z_index": 30, "is_on_screen": true, "layer": 0, "bounds": { "width": 1440.0, "height": 900.0 } },
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
