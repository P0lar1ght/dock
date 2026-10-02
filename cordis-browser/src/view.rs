//! 网关看某个标签页：CDP screencast 推帧、转发鼠标键盘、导航。
//!
//! [`View::attach`] 按 `DevToolsActivePort` 另开一条 CDP 连接，挂到指定 target 上；
//! 和 MCP 服务那条连接互不干扰（chromiumoxide 默认的 `HandlerConfig` 不设视口，
//! 不会改 agent 那一页的尺寸）。关掉 View 只断自己的连接，不关页、不关浏览器。
//!
//! 流控靠 CDP 自己：Chrome 收到上一帧的 `screencastFrameAck` 才发下一帧。调用方
//! 在帧真正写出去之后再 [`View::ack`]，慢客户端就不会攒出一长串帧。

use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::browser::Browser;
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, InsertTextParams, MouseButton,
};
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, CaptureScreenshotParams, EventScreencastFrame,
    GetNavigationHistoryParams, NavigateParams, NavigateToHistoryEntryParams, ReloadParams,
    ScreencastFrameAckParams, StartScreencastFormat, StartScreencastParams, StopScreencastParams,
    Viewport,
};
use chromiumoxide::cdp::browser_protocol::target::{GetTargetInfoParams, TargetId};
use chromiumoxide::listeners::EventStream;
use chromiumoxide::page::Page;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::task::JoinHandle;

use crate::session::{browser_user_data_dir, devtools_ws_url, key_event, KeyPhase};

const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);

/// 一帧：base64 JPEG 原样透传（不解码），外加把画面坐标换回页面坐标要用的元数据。
#[derive(Clone, Debug)]
pub struct Frame {
    pub data: String,
    /// 页面视口的 CSS 像素宽高（画面可能被 `maxWidth` / `maxHeight` 缩过）。
    pub device_width: f64,
    pub device_height: f64,
    pub page_scale_factor: f64,
    pub offset_top: f64,
    pub scroll_x: f64,
    pub scroll_y: f64,
    /// 回 [`View::ack`] 用的帧号。
    pub ack_id: i64,
}

/// 画面参数。缺省：JPEG、质量 70、最长边 1280。
#[derive(Clone, Copy, Debug)]
pub struct ScreencastOptions {
    pub quality: i64,
    pub max_width: i64,
    pub max_height: i64,
}

impl Default for ScreencastOptions {
    fn default() -> Self {
        Self {
            quality: 70,
            max_width: 1280,
            max_height: 1280,
        }
    }
}

/// 页面现在的地址与标题。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageInfo {
    pub url: String,
    pub title: String,
}

pub struct View {
    browser: Arc<Browser>,
    handler: JoinHandle<()>,
    page: Page,
    target_id: String,
}

impl View {
    /// 挂到 `target_id` 那一页。浏览器没在跑、或这个 target 已经没了都回错。
    pub async fn attach(target_id: &str) -> Result<Self, String> {
        let ws = devtools_ws_url(&browser_user_data_dir())
            .ok_or_else(|| "浏览器没有在运行".to_string())?;
        let (mut browser, mut handler) = tokio::time::timeout(ATTACH_TIMEOUT, Browser::connect(ws))
            .await
            .map_err(|_| "连接浏览器超时".to_string())?
            .map_err(|e| format!("连接浏览器失败：{e}"))?;
        let handler = tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    break;
                }
            }
        });
        let found = async {
            browser
                .fetch_targets()
                .await
                .map_err(|e| format!("列出标签页失败：{e}"))?;
            // fetch_targets 只是开始挂；页要过一小会儿才拿得到。
            for _ in 0..40 {
                if let Ok(page) = browser
                    .get_page(TargetId::from(target_id.to_string()))
                    .await
                {
                    return Ok(page);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(format!("标签页 {target_id} 已经不在了"))
        };
        match tokio::time::timeout(ATTACH_TIMEOUT, found).await {
            Ok(Ok(page)) => Ok(Self {
                browser: Arc::new(browser),
                handler,
                page,
                target_id: target_id.to_string(),
            }),
            Ok(Err(e)) => {
                handler.abort();
                Err(e)
            }
            Err(_) => {
                handler.abort();
                Err(format!("挂到标签页 {target_id} 超时"))
            }
        }
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// 连接还在吗（浏览器被关掉时 handler 流结束）。
    pub fn is_alive(&self) -> bool {
        !self.handler.is_finished()
    }

    /// 开始推画面。先挂监听再开 screencast，第一帧不会漏。
    pub async fn screencast(&self, opts: ScreencastOptions) -> Result<Frames, String> {
        let events = self
            .page
            .event_listener::<EventScreencastFrame>()
            .await
            .map_err(|e| format!("监听画面失败：{e}"))?;
        self.resume(opts).await?;
        Ok(Frames { events })
    }

    /// 暂停推画面（监听不动）。补静止帧前用：`clip.scale` 截图时 Chrome 临时放大视口，
    /// 这几帧过渡画面不该推给客户端。之后 [`View::resume`] 接着推。
    pub async fn pause(&self) {
        let _ = self.page.execute(StopScreencastParams::default()).await;
    }

    /// 接着推画面（[`View::screencast`] 开始时也走这里）。静止的页面恢复后不会自己推帧。
    pub async fn resume(&self, opts: ScreencastOptions) -> Result<(), String> {
        let params = StartScreencastParams::builder()
            .format(StartScreencastFormat::Jpeg)
            .quality(opts.quality.clamp(10, 100))
            .max_width(opts.max_width.clamp(64, 4096))
            .max_height(opts.max_height.clamp(64, 4096))
            .build();
        self.page
            .execute(params)
            .await
            .map(|_| ())
            .map_err(|e| format!("开始推画面失败：{e}"))
    }

    /// 静止画面：截一张当前可见的视口（JPEG），按 `scale` 倍重新栅格化。
    ///
    /// 无头 Chrome 的 screencast 只按 CSS 像素出帧，页面不动时也一帧不推（刚开始推也不推）；
    /// 画面停下来时补这一张：静止页面一定有画面，`scale: 2` 时 Retina 上也清楚。
    /// 用 `clip.scale` 而不是改页面的设备像素比：agent 看到的页面和它的截图都不受影响。
    /// 返回的 [`Frame`] 元数据取自 `getLayoutMetrics`（CSS 像素），`ack_id` 为 0（不用回 ack）。
    pub async fn still(&self, quality: i64, scale: f64) -> Result<Frame, String> {
        let view = self
            .page
            .layout_metrics()
            .await
            .map_err(|e| format!("读视口失败：{e}"))?
            .css_visual_viewport;
        let shot = self
            .page
            .execute(
                CaptureScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Jpeg)
                    .quality(quality.clamp(10, 100))
                    .clip(Viewport {
                        x: view.page_x,
                        y: view.page_y,
                        width: view.client_width,
                        height: view.client_height,
                        scale: scale.clamp(1.0, 3.0),
                    })
                    .build(),
            )
            .await
            .map_err(|e| format!("截静止画面失败：{e}"))?;
        Ok(Frame {
            data: AsRef::<str>::as_ref(&shot.result.data).to_string(),
            device_width: view.client_width,
            device_height: view.client_height,
            page_scale_factor: view.scale,
            offset_top: 0.0,
            scroll_x: view.page_x,
            scroll_y: view.page_y,
            ack_id: 0,
        })
    }

    /// 这一帧已经送出去了：让 Chrome 发下一帧。
    pub async fn ack(&self, ack_id: i64) {
        let _ = self
            .page
            .execute(ScreencastFrameAckParams::new(ack_id))
            .await;
    }

    pub async fn info(&self) -> PageInfo {
        match self
            .browser
            .execute(
                GetTargetInfoParams::builder()
                    .target_id(TargetId::from(self.target_id.clone()))
                    .build(),
            )
            .await
        {
            Ok(resp) => PageInfo {
                url: resp.result.target_info.url.clone(),
                title: resp.result.target_info.title.clone(),
            },
            Err(_) => PageInfo::default(),
        }
    }

    /// 转发一次用户输入。坐标是页面视口的 CSS 像素（客户端按帧元数据换算）。
    ///
    /// - `{type:"mouse", action:"move"|"down"|"up"|"click", x, y, button?, clickCount?, modifiers?}`
    /// - `{type:"wheel", x, y, deltaX, deltaY, modifiers?}`
    /// - `{type:"key", action:"down"|"up"|"press", key, modifiers?}`（`key` 用 DOM 键名：`Enter`、`a`）
    /// - `{type:"text", text}`：直接插入文字（输入法上屏、粘贴）
    ///
    /// `modifiers`：`["Alt","Control","Meta","Shift"]` 的子集。
    pub async fn input(&self, event: &Value) -> Result<(), String> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let modifiers = modifier_bits(event.get("modifiers"));
        match kind {
            "mouse" => {
                let (x, y) = point(event)?;
                let button = mouse_button(event.get("button").and_then(Value::as_str))?;
                let clicks = event.get("clickCount").and_then(Value::as_i64).unwrap_or(1);
                let action = event
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("click");
                let phases: &[DispatchMouseEventType] = match action {
                    "move" => &[DispatchMouseEventType::MouseMoved],
                    "down" => &[DispatchMouseEventType::MousePressed],
                    "up" => &[DispatchMouseEventType::MouseReleased],
                    "click" => &[
                        DispatchMouseEventType::MouseMoved,
                        DispatchMouseEventType::MousePressed,
                        DispatchMouseEventType::MouseReleased,
                    ],
                    other => return Err(format!("不认识的鼠标动作：{other}")),
                };
                for phase in phases {
                    let moving = *phase == DispatchMouseEventType::MouseMoved;
                    let params = DispatchMouseEventParams::builder()
                        .r#type(phase.clone())
                        .x(x)
                        .y(y)
                        .modifiers(modifiers)
                        .button(if moving {
                            MouseButton::None
                        } else {
                            button.clone()
                        })
                        .click_count(if moving { 0 } else { clicks })
                        .build()
                        .map_err(|e| e.to_string())?;
                    self.dispatch(params).await?;
                }
                Ok(())
            }
            "wheel" => {
                let (x, y) = point(event)?;
                let dx = event.get("deltaX").and_then(Value::as_f64).unwrap_or(0.0);
                let dy = event.get("deltaY").and_then(Value::as_f64).unwrap_or(0.0);
                let params = DispatchMouseEventParams::builder()
                    .r#type(DispatchMouseEventType::MouseWheel)
                    .x(x)
                    .y(y)
                    .delta_x(dx)
                    .delta_y(dy)
                    .modifiers(modifiers)
                    .build()
                    .map_err(|e| e.to_string())?;
                self.dispatch(params).await
            }
            "key" => {
                let key = event
                    .get("key")
                    .and_then(Value::as_str)
                    .filter(|k| !k.is_empty())
                    .ok_or_else(|| "key 不能为空".to_string())?;
                let action = event
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("press");
                let phases: &[KeyPhase] = match action {
                    "down" => &[KeyPhase::Down],
                    "up" => &[KeyPhase::Up],
                    "press" => &[KeyPhase::Down, KeyPhase::Up],
                    other => return Err(format!("不认识的按键动作：{other}")),
                };
                for phase in phases {
                    let params = key_event(key, modifiers, *phase)?;
                    self.page.execute(params).await.map_err(|e| e.to_string())?;
                }
                Ok(())
            }
            "text" => {
                let text = event.get("text").and_then(Value::as_str).unwrap_or("");
                if text.is_empty() {
                    return Ok(());
                }
                self.page
                    .execute(InsertTextParams::new(text))
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            other => Err(format!("不认识的输入类型：{other}")),
        }
    }

    async fn dispatch(&self, params: DispatchMouseEventParams) -> Result<(), String> {
        self.page
            .execute(params)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// 地址栏 / 后退 / 前进 / 刷新。
    pub async fn navigate(&self, request: &Value) -> Result<(), String> {
        if let Some(url) = request.get("url").and_then(Value::as_str) {
            let url = address_bar_url(url)?;
            // chromiumoxide 把 Page.navigate 挂到「页面加载完」才回，而这条旁路连接挂上的页
            // 收不到它等的生命周期事件，会一直等到超时。地址栏不需要等加载完——用户在画面
            // 里看得到——所以放到后台发出去就回。
            let page = self.page.clone();
            tokio::spawn(async move {
                let _ = tokio::time::timeout(
                    Duration::from_secs(30),
                    page.execute(NavigateParams::new(url)),
                )
                .await;
            });
            return Ok(());
        }
        match request.get("action").and_then(Value::as_str).unwrap_or("") {
            "reload" => self
                .page
                .execute(ReloadParams::default())
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
            action @ ("back" | "forward") => {
                let history = self
                    .page
                    .execute(GetNavigationHistoryParams::default())
                    .await
                    .map_err(|e| e.to_string())?;
                let step: i64 = if action == "back" { -1 } else { 1 };
                let index = history.result.current_index + step;
                let entry = usize::try_from(index)
                    .ok()
                    .and_then(|i| history.result.entries.get(i))
                    .ok_or_else(|| {
                        if action == "back" {
                            "已经是第一页".to_string()
                        } else {
                            "已经是最后一页".to_string()
                        }
                    })?;
                self.page
                    .execute(NavigateToHistoryEntryParams::new(entry.id))
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            other => Err(format!(
                "navigate 需要 url 或 action（back / forward / reload）：{other}"
            )),
        }
    }

    /// 停推画面、断开自己这条连接。页和浏览器都留着。
    pub async fn close(self) {
        let _ = self.page.execute(StopScreencastParams::default()).await;
    }
}

impl Drop for View {
    /// 直接丢掉（比如网关连接断了、任务被中止）也要断开这条 CDP 连接。
    fn drop(&mut self) {
        self.handler.abort();
    }
}

/// 画面流。`next` 为 `None` 表示页没了或连接断了。
pub struct Frames {
    events: EventStream<EventScreencastFrame>,
}

impl Frames {
    pub async fn next(&mut self) -> Option<Frame> {
        let ev = self.events.next().await?;
        let m = &ev.metadata;
        Some(Frame {
            data: AsRef::<str>::as_ref(&ev.data).to_string(),
            device_width: m.device_width,
            device_height: m.device_height,
            page_scale_factor: m.page_scale_factor,
            offset_top: m.offset_top,
            scroll_x: m.scroll_offset_x,
            scroll_y: m.scroll_offset_y,
            ack_id: ev.session_id,
        })
    }
}

/// 地址栏：没写协议的补 `https://`；只放行 http / https / about / data / file。
pub fn address_bar_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("url 不能为空".into());
    }
    let with_scheme = match raw.split_once(':') {
        Some((scheme, _))
            if !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
                && !raw.contains("://")
                && !matches!(scheme, "about" | "data") =>
        {
            // `localhost:3000`、`example.com:8080/x` 这种：冒号前是主机，不是协议。
            if raw[scheme.len() + 1..].starts_with(|c: char| c.is_ascii_digit()) {
                format!("https://{raw}")
            } else {
                raw.to_string()
            }
        }
        Some(_) => raw.to_string(),
        None => format!("https://{raw}"),
    };
    let scheme = with_scheme
        .split_once(':')
        .map(|(s, _)| s.to_ascii_lowercase())
        .unwrap_or_default();
    match scheme.as_str() {
        "http" | "https" | "about" | "data" | "file" => Ok(with_scheme),
        other => Err(format!("不支持的地址协议：{other}")),
    }
}

fn point(event: &Value) -> Result<(f64, f64), String> {
    let x = event.get("x").and_then(Value::as_f64);
    let y = event.get("y").and_then(Value::as_f64);
    match (x, y) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok((x, y)),
        _ => Err("需要数字 x、y".into()),
    }
}

fn mouse_button(name: Option<&str>) -> Result<MouseButton, String> {
    match name.unwrap_or("left") {
        "left" => Ok(MouseButton::Left),
        "middle" => Ok(MouseButton::Middle),
        "right" => Ok(MouseButton::Right),
        "none" => Ok(MouseButton::None),
        other => Err(format!("不认识的鼠标键：{other}")),
    }
}

/// `["Alt","Control","Meta","Shift"]` → CDP 位域（Alt=1 Ctrl=2 Meta=4 Shift=8）。
pub fn modifier_bits(list: Option<&Value>) -> i64 {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|m| match m {
            "Alt" => 1,
            "Control" => 2,
            "Meta" => 4,
            "Shift" => 8,
            _ => 0,
        })
        .fold(0, |acc, bit| acc | bit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modifiers_fold_into_cdp_bits() {
        assert_eq!(modifier_bits(None), 0);
        assert_eq!(modifier_bits(Some(&json!(["Control", "Shift"]))), 10);
        assert_eq!(modifier_bits(Some(&json!(["Meta", "Nope"]))), 4);
    }

    #[test]
    fn address_bar_fills_scheme_and_refuses_script_urls() {
        assert_eq!(
            address_bar_url("example.com").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            address_bar_url(" localhost:3000/x ").unwrap(),
            "https://localhost:3000/x"
        );
        assert_eq!(address_bar_url("http://a.test").unwrap(), "http://a.test");
        assert_eq!(address_bar_url("about:blank").unwrap(), "about:blank");
        assert!(address_bar_url("data:text/html,<p>x</p>").is_ok());
        assert!(address_bar_url("javascript:alert(1)").is_err());
        assert!(address_bar_url("chrome://settings").is_err());
        assert!(address_bar_url("  ").is_err());
    }

    #[test]
    fn point_and_button_validate() {
        assert_eq!(point(&json!({"x": 1, "y": 2.5})).unwrap(), (1.0, 2.5));
        assert!(point(&json!({"x": 1})).is_err());
        assert!(mouse_button(Some("thumb")).is_err());
        assert!(matches!(mouse_button(None), Ok(MouseButton::Left)));
    }

    #[tokio::test]
    async fn attach_without_browser_is_a_clear_error() {
        let home = tempfile::tempdir().unwrap();
        let _env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path());
        let err = View::attach("nope").await.err().unwrap();
        assert_eq!(err, "浏览器没有在运行");
    }
}
