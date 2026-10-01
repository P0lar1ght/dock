//! 按会话管标签页组，把 `browser_*` 调用分派过去。
//!
//! 一个 [`Chromium`] 进程（懒启动），每个会话 id 一个 [`ConnectedSession`]。
//! 同一会话的调用排队（组上一把锁），不同会话并行。最后一组关掉时 Chromium 也关。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use crate::registry::{self, SessionTabs};
use crate::session::{Chromium, ConnectedSession};
use crate::tools::{
    arg_bool, arg_f64, arg_i64, arg_str, arg_u64, arg_usize, parse_fill_fields, parse_paths,
};
use crate::TabInfo;

const NEED_OPEN: &str = "Error: this session has no browser tab yet. Call browser_open first.";

/// 一次调用的结果。`image` 是截图文件路径，服务端读成 MCP `image` 内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOutput {
    pub text: String,
    pub is_error: bool,
    pub image: Option<PathBuf>,
}

impl CallOutput {
    fn ok(text: String) -> Self {
        Self {
            text,
            is_error: false,
            image: None,
        }
    }

    fn err(e: String) -> Self {
        let text = if e.starts_with("Error:") {
            e
        } else {
            format!("Error: {e}")
        };
        Self {
            text,
            is_error: true,
            image: None,
        }
    }
}

type Group = Arc<Mutex<ConnectedSession>>;

/// 会话 → 标签页组。见模块文档。
#[derive(Default)]
pub struct BrowserHub {
    chromium: Mutex<Option<Chromium>>,
    groups: Mutex<HashMap<String, Group>>,
    /// 各会话的 target id，镜像到运行时名册（[`registry`]）给网关推画面用。单独一把
    /// 同步锁：不能去锁各组——别的会话可能正攥着自己的组跑一个 30 秒的 wait_for。
    index: std::sync::Mutex<BTreeMap<String, SessionTabs>>,
    /// 测试把名册写到临时目录；`None` 用 `$DOCK_HOME/browser/sessions`。
    registry_dir: Option<std::path::PathBuf>,
}

impl BrowserHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// 名册写到 `dir`（测试用）。
    pub fn with_registry_dir(dir: std::path::PathBuf) -> Self {
        Self {
            registry_dir: Some(dir),
            ..Self::default()
        }
    }

    fn publish(&self) {
        let snapshot = self.index.lock().unwrap().clone();
        match &self.registry_dir {
            Some(dir) => registry::publish_in(dir, &snapshot),
            None => registry::publish(&snapshot),
        }
    }

    /// 某组的标签页可能变了：记下并重写名册。调用方攥着这一组的锁。
    fn record(&self, session: &str, group: &ConnectedSession) {
        let tabs = SessionTabs {
            targets: group.target_ids(),
            active: group.active_target_id(),
        };
        let changed = self
            .index
            .lock()
            .unwrap()
            .insert(session.to_string(), tabs.clone())
            != Some(tabs);
        if changed {
            self.publish();
        }
    }

    fn forget(&self, session: Option<&str>) {
        {
            let mut index = self.index.lock().unwrap();
            match session {
                Some(id) => {
                    index.remove(id);
                }
                None => index.clear(),
            }
        }
        self.publish();
    }

    /// 有标签页的会话 id（排好序，测试和状态行用）。
    pub async fn sessions(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.groups.lock().await.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// 某会话的标签页（没开过就是空）。
    pub async fn tabs(&self, session: &str) -> Vec<TabInfo> {
        let Some(group) = self.group(session).await else {
            return Vec::new();
        };
        let g = group.lock().await;
        g.tab_infos().await
    }

    pub async fn call(&self, session: &str, name: &str, args: &Value) -> CallOutput {
        let result = match name {
            "browser_open" => match arg_str(args, "url") {
                Some(url) if !url.is_empty() => self.open(session, &url).await,
                _ => Err("url is required".into()),
            },
            "browser_close" => {
                self.close(session).await;
                Ok("closed this session's tabs".into())
            }
            _ if crate::tools::TOOLS.iter().any(|t| t.name == name) => {
                match self.live_group(session).await {
                    Some(group) => {
                        let mut g = group.lock().await;
                        let out = dispatch_connected(&mut g, name, args).await;
                        if name == "browser_tabs" {
                            self.record(session, &g);
                        }
                        return out;
                    }
                    None => Err(NEED_OPEN.into()),
                }
            }
            other => Err(format!("unknown browser tool `{other}`")),
        };
        match result {
            Ok(text) => CallOutput::ok(text),
            Err(e) => CallOutput::err(e),
        }
    }

    /// 关掉全部标签页组和自己拉起的 Chromium（stdin 关了、进程要退出时）。
    pub async fn shutdown(&self) {
        self.forget(None);
        let groups: Vec<Group> = self.groups.lock().await.drain().map(|(_, g)| g).collect();
        for group in groups {
            if let Ok(g) = Arc::try_unwrap(group) {
                g.into_inner().shutdown().await;
            }
        }
        if let Some(chromium) = self.chromium.lock().await.take() {
            chromium.shutdown().await;
        }
    }

    async fn group(&self, session: &str) -> Option<Group> {
        self.groups.lock().await.get(session).cloned()
    }

    /// 本会话的组，且 Chromium 还活着。Chromium 没了（被关掉 / 崩了）就把所有组
    /// 作废——它们的页都跟着没了，留着只会在下一次调用报一串 CDP 错。
    async fn live_group(&self, session: &str) -> Option<Group> {
        if !self.chromium_alive().await {
            self.forget_dead_chromium().await;
            return None;
        }
        self.group(session).await
    }

    async fn chromium_alive(&self) -> bool {
        self.chromium
            .lock()
            .await
            .as_ref()
            .is_some_and(Chromium::is_alive)
    }

    async fn forget_dead_chromium(&self) {
        self.forget(None);
        self.groups.lock().await.clear();
        if let Some(dead) = self.chromium.lock().await.take() {
            dead.shutdown().await;
        }
    }

    async fn open(&self, session: &str, url: &str) -> Result<String, String> {
        if let Some(group) = self.live_group(session).await {
            let mut g = group.lock().await;
            let cur = g.navigate(url).await?;
            return Ok(format!("navigated to {cur}"));
        }
        let fresh = {
            let mut chromium = self.chromium.lock().await;
            if chromium.is_none() {
                let headed = cordis_base::config::effective_browser_headed();
                *chromium = Some(Chromium::attach_or_launch(headed).await?);
            }
            let chromium = chromium.as_ref().expect("just ensured");
            ConnectedSession::open(chromium, Some(url)).await?
        };
        let url_now = match fresh.active_page() {
            Ok(p) => p.url().await.ok().flatten().unwrap_or_else(|| url.into()),
            Err(_) => url.into(),
        };
        let (targets, active) = (fresh.target_ids(), fresh.active_target_id());
        let group = Arc::new(Mutex::new(fresh));
        // 两次并发 open 同一会话：后到的那组关掉，别留孤儿标签页。
        let loser = {
            let mut groups = self.groups.lock().await;
            match groups.get(session) {
                Some(_) => Some(group),
                None => {
                    groups.insert(session.to_string(), group);
                    None
                }
            }
        };
        if loser.is_none() {
            self.index
                .lock()
                .unwrap()
                .insert(session.to_string(), SessionTabs { targets, active });
            self.publish();
        }
        if let Some(loser) = loser {
            if let Ok(g) = Arc::try_unwrap(loser) {
                g.into_inner().shutdown().await;
            }
        }
        Ok(format!("opened {url_now}"))
    }

    async fn close(&self, session: &str) {
        self.forget(Some(session));
        let removed = self.groups.lock().await.remove(session);
        if let Some(group) = removed {
            // 还有调用在跑就等它跑完再关。
            let g = group.lock().await;
            drop(g);
            if let Ok(g) = Arc::try_unwrap(group) {
                g.into_inner().shutdown().await;
            }
        }
        if self.groups.lock().await.is_empty() {
            if let Some(chromium) = self.chromium.lock().await.take() {
                chromium.shutdown().await;
            }
        }
    }
}

async fn dispatch_connected(
    session: &mut ConnectedSession,
    name: &str,
    args: &Value,
) -> CallOutput {
    if name == "browser_screenshot" {
        let full = arg_bool(args, "full_page").unwrap_or(false);
        return match session.screenshot(full).await {
            Ok(path) => CallOutput {
                text: format!("saved {}", path.display()),
                is_error: false,
                image: Some(path),
            },
            Err(e) => CallOutput::err(e),
        };
    }
    match dispatch_text(session, name, args).await {
        Ok(text) => CallOutput::ok(text),
        Err(e) => CallOutput::err(e),
    }
}

async fn dispatch_text(
    session: &mut ConnectedSession,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    match name {
        "browser_navigate" => match arg_str(args, "url") {
            Some(url) if !url.is_empty() => session
                .navigate(&url)
                .await
                .map(|cur| format!("navigated to {cur}")),
            _ => Err("url is required".into()),
        },
        "browser_navigate_back" => session.navigate_back().await,
        "browser_snapshot" => {
            let interactive = arg_bool(args, "interactive").unwrap_or(true);
            let frame = arg_str(args, "frame").or_else(|| arg_str(args, "frame_selector"));
            let snap = session.snapshot(interactive, frame.as_deref()).await?;
            Ok(with_pending_dialog(session, snap.text))
        }
        "browser_click" => {
            let r = arg_str(args, "ref").ok_or_else(|| "ref is required".to_string())?;
            // Optional frame/frame_selector documented for API symmetry; refs
            // must come from a snapshot taken in that frame.
            session.click_ref(&r).await
        }
        "browser_type" => {
            let text = arg_str(args, "text").ok_or_else(|| "text is required".to_string())?;
            let ref_id = arg_str(args, "ref");
            let submit = arg_bool(args, "submit").unwrap_or(false);
            session.type_ref(ref_id.as_deref(), &text, submit).await
        }
        "browser_tabs" => {
            let action = arg_str(args, "action").unwrap_or_else(|| "list".into());
            let index = arg_usize(args, "index");
            let url = arg_str(args, "url");
            session.tabs(&action, index, url.as_deref()).await
        }
        "browser_hover" => {
            let r = arg_str(args, "ref").ok_or_else(|| "ref is required".to_string())?;
            session.hover_ref(&r).await
        }
        "browser_press_key" => {
            let key = arg_str(args, "key").ok_or_else(|| "key is required".to_string())?;
            let ref_id = arg_str(args, "ref");
            session.press_key(&key, ref_id.as_deref()).await
        }
        "browser_select_option" => {
            let r = arg_str(args, "ref").ok_or_else(|| "ref is required".to_string())?;
            let value = arg_str(args, "value");
            let label = arg_str(args, "label");
            session
                .select_option(&r, value.as_deref(), label.as_deref())
                .await
        }
        "browser_fill_form" => {
            let fields = parse_fill_fields(args)?;
            session.fill_form(&fields).await
        }
        "browser_wait_for" => {
            let text = arg_str(args, "text");
            let selector = arg_str(args, "selector");
            let timeout_ms = arg_u64(args, "timeout_ms");
            session
                .wait_for(text.as_deref(), selector.as_deref(), timeout_ms)
                .await
        }
        "browser_drag" => {
            let source_ref = arg_str(args, "source_ref");
            let target_ref = arg_str(args, "target_ref");
            let steps = arg_u64(args, "steps").map(|n| n as u32);
            session
                .drag(
                    source_ref.as_deref(),
                    target_ref.as_deref(),
                    arg_f64(args, "start_x"),
                    arg_f64(args, "start_y"),
                    arg_f64(args, "end_x"),
                    arg_f64(args, "end_y"),
                    steps,
                )
                .await
        }
        "browser_handle_dialog" => {
            let accept = arg_bool(args, "accept").ok_or_else(|| {
                "accept boolean is required (true=accept, false=dismiss)".to_string()
            })?;
            let prompt_text = arg_str(args, "prompt_text");
            session.handle_dialog(accept, prompt_text.as_deref()).await
        }
        "browser_file_upload" => {
            let r = arg_str(args, "ref").ok_or_else(|| "ref is required".to_string())?;
            let paths = parse_paths(args)?;
            session.file_upload(&r, &paths).await
        }
        "browser_resize" => {
            let width = arg_i64(args, "width").ok_or_else(|| "width is required".to_string())?;
            let height = arg_i64(args, "height").ok_or_else(|| "height is required".to_string())?;
            session.resize(width, height).await
        }
        "browser_evaluate" => {
            let expression = arg_str(args, "expression")
                .or_else(|| arg_str(args, "code"))
                .ok_or_else(|| "expression is required".to_string())?;
            let frame = arg_str(args, "frame").or_else(|| arg_str(args, "frame_selector"));
            session.evaluate(&expression, frame.as_deref()).await
        }
        "browser_console_messages" => session.console_messages().await,
        "browser_network_requests" => session.network_requests().await,
        other => Err(format!("unknown browser tool `{other}`")),
    }
}

/// 页上挂着一个 JS 对话框时，快照里提醒模型先处理它（以前写在 `/browser` 驾驶舱上）。
fn with_pending_dialog(session: &ConnectedSession, text: String) -> String {
    match session.pending_dialog() {
        Some(d) => format!(
            "{text}\n\n[pending {} dialog: {} — call browser_handle_dialog]",
            d.dialog_type,
            d.message.replace('\n', " ")
        ),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn tools_need_open_per_session() {
        let hub = BrowserHub::new();
        let out = hub.call("a", "browser_snapshot", &json!({})).await;
        assert!(out.is_error, "{out:?}");
        assert!(out.text.contains("browser_open"), "{}", out.text);
        assert!(hub.sessions().await.is_empty());
    }

    #[tokio::test]
    async fn open_requires_url_and_unknown_tool_errors() {
        let hub = BrowserHub::new();
        let out = hub.call("a", "browser_open", &json!({})).await;
        assert!(out.is_error);
        assert_eq!(out.text, "Error: url is required");
        let out = hub.call("a", "browser_fly", &json!({})).await;
        assert!(out.is_error);
        assert!(out.text.contains("unknown browser tool"), "{}", out.text);
    }

    #[tokio::test]
    async fn registry_stays_empty_without_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let hub = BrowserHub::with_registry_dir(dir.path().to_path_buf());
        let _ = hub.call("a", "browser_close", &json!({})).await;
        hub.shutdown().await;
        assert!(crate::registry::lookup_in(dir.path(), "a").is_none());
    }

    #[tokio::test]
    async fn close_without_tabs_is_ok() {
        let hub = BrowserHub::new();
        let out = hub.call("a", "browser_close", &json!({})).await;
        assert!(!out.is_error, "{out:?}");
    }
}
