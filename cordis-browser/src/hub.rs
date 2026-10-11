//! 按会话管标签页组，把 `browser_*` 调用分派过去。
//!
//! 一个 [`Chromium`] 进程（懒启动），每个会话 id 一个 [`ConnectedSession`]。
//! 同一会话的调用排队（组上一把锁），不同会话并行。最后一组关掉时 Chromium 也关。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Mutex;

use crate::js::{self, JsRunner};
use crate::registry::{self, SessionTabs};
use crate::session::{Chromium, ConnectedSession, DialogWatch, PendingDialog};
use crate::tools::{
    arg_bool, arg_f64, arg_i64, arg_str, arg_u64, arg_usize, parse_fill_fields, parse_paths,
};
use crate::TabInfo;

const NEED_OPEN: &str = "Error: this session has no browser tab yet. Call browser_open first.";

/// 一次调用最多等这么久（`browser_wait_for` 另算：它自己的超时再加这一段余量）。
/// 页面卡死（死循环脚本、渲染进程挂住）时报错说清楚，而不是让调用一直挂着。
const CALL_LIMIT: Duration = Duration::from_secs(90);
const WAIT_FOR_MARGIN: Duration = Duration::from_secs(15);
/// `browser_js` 的默认 / 最长运行时间。
const JS_DEFAULT_MS: u64 = 60_000;
const JS_MAX_MS: u64 = 300_000;

/// 对话框开着时还能调的：处理它本身。别的命令会排在被挡住的页后面一直等。
const DIALOG_SAFE: &[&str] = &["browser_handle_dialog"];

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
    /// `browser_js` 的 Node 执行器，第一次用时才起。
    js: Mutex<Option<Arc<JsRunner>>>,
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
            // navigate 和 open 是同一件事：有页就在活动页跳转，没页就先开一页（以前
            // navigate 在没页时直接报错，模型要再调一次 open）。
            "browser_open" | "browser_navigate" => match arg_str(args, "url") {
                Some(url) if !url.is_empty() => self.open(session, &url).await,
                _ => Err("url is required".into()),
            },
            "browser_close" => {
                self.close(session).await;
                Ok("closed this session's tabs".into())
            }
            "browser_js" => return self.run_js(session, args).await,
            _ if crate::tools::TOOLS.iter().any(|t| t.name == name) => {
                match self.live_group(session).await {
                    Some(group) => {
                        let mut g = group.lock().await;
                        let watch = g.dialog_watch();
                        if let Some(blocked) = blocked_by_dialog(&watch, name) {
                            return blocked;
                        }
                        let out = guarded(
                            name,
                            call_limit(name, args),
                            Some(watch),
                            dispatch_connected(&mut g, name, args),
                        )
                        .await;
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
        if let Some(runner) = self.js.lock().await.take() {
            runner.shutdown().await;
        }
        if let Some(chromium) = self.chromium.lock().await.take() {
            chromium.shutdown().await;
        }
    }

    /// `browser_js`：在本会话的活动标签页上跑一段 Playwright 脚本（见 [`crate::js`]）。
    /// 本会话还没有页就先开一个空白页；和别的调用一样受对话框与时限的管。
    async fn run_js(&self, session: &str, args: &Value) -> CallOutput {
        let Some(code) = js_code(args) else {
            return CallOutput::err("code is required (the script goes in `code`)".into());
        };
        let limit = Duration::from_millis(
            arg_u64(args, "timeout_ms")
                .unwrap_or(JS_DEFAULT_MS)
                .clamp(1_000, JS_MAX_MS),
        );
        if self.live_group(session).await.is_none() {
            if let Err(e) = self.open(session, "about:blank").await {
                return CallOutput::err(e);
            }
        }
        let Some(group) = self.live_group(session).await else {
            return CallOutput::err(NEED_OPEN.into());
        };
        let runner = match self.js_runner().await {
            Ok(r) => r,
            Err(e) => return CallOutput::err(e),
        };
        let Some(ws) = self.chromium.lock().await.as_ref().map(Chromium::ws_url) else {
            return CallOutput::err(NEED_OPEN.into());
        };
        let mut g = group.lock().await;
        let watch = g.dialog_watch();
        if let Some(blocked) = blocked_by_dialog(&watch, "browser_js") {
            return blocked;
        }
        let Some(target) = g.active_target_id() else {
            return CallOutput::err(NEED_OPEN.into());
        };
        let req = serde_json::json!({
            "session": session,
            "ws": ws,
            "target": target,
            "tabs": g.target_ids(),
            "code": code,
            "timeoutMs": limit.as_millis() as u64,
            "shotDir": crate::session::browser_screenshot_dir(),
        });
        let _ = std::fs::create_dir_all(crate::session::browser_screenshot_dir());
        let answered = std::sync::atomic::AtomicBool::new(false);
        let tab_moves: std::sync::Mutex<(Vec<String>, Option<String>)> = Default::default();
        let dialog_seen: std::sync::Mutex<Option<(String, String, String)>> = Default::default();
        let dialogs = watch.clone();
        let run = async {
            // 执行器自己按 limit 收手；这里多等一会儿给它回话的时间。
            match runner.call(req, limit + WAIT_FOR_MARGIN).await {
                Ok(reply) => {
                    answered.store(true, std::sync::atomic::Ordering::SeqCst);
                    let ids = |v: &Value| -> Vec<String> {
                        v.as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default()
                    };
                    *tab_moves.lock().unwrap() = (
                        ids(&reply["opened"]),
                        reply["activate"].as_str().map(str::to_string),
                    );
                    if let Some(target) = reply["dialog"]["target"].as_str() {
                        *dialog_seen.lock().unwrap() = Some((
                            target.to_string(),
                            reply["dialog"]["type"]
                                .as_str()
                                .unwrap_or("alert")
                                .to_string(),
                            reply["dialog"]["message"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                        ));
                    }
                    let (ok, mut text, image) = js::render_reply(&reply);
                    if let Some(d) = reply.get("dialog").filter(|d| !d.is_null()) {
                        text.push_str(&format!(
                            "\n\nThe script opened a JavaScript {} dialog: {}. The page is blocked until you call browser_handle_dialog (accept true/false).",
                            d["type"].as_str().unwrap_or("?"),
                            d["message"]
                        ));
                    }
                    CallOutput {
                        text,
                        is_error: !ok,
                        image,
                    }
                }
                Err(e) => CallOutput::err(e),
            }
        };
        let out = guarded("browser_js", limit + WAIT_FOR_MARGIN * 2, None, run).await;
        // 执行器没回话就返回了：要么是对话框（脚本卡在 CDP 上，对话框关掉后接着跑完，执行器
        // 拦着同一会话的新脚本），要么是执行器自己都没在时限里回话——那就杀掉它，脚本才真的停。
        if !answered.load(std::sync::atomic::Ordering::SeqCst) && dialogs.pending().is_none() {
            self.reset_js().await;
            return CallOutput::err(format!(
                "{} The script runner was stopped and will restart on the next browser_js call; `state` was cleared.",
                out.text.trim_start_matches("Error: ")
            ));
        }
        // 脚本里新开的页登记进本组、切到它指定的活动页，再和 Chrome 对一遍（点链接弹出的
        // 新页跑完就收进来），然后重写名册——GUI 画面按名册跟着活动页走。
        let (opened, activate) = std::mem::take(&mut *tab_moves.lock().unwrap());
        let mut notes = Vec::new();
        for id in &opened {
            if let Err(e) = g.adopt(id).await {
                notes.push(e);
            }
        }
        if let Some(id) = activate {
            g.activate(&id);
        }
        if let Some((target, kind, message)) = dialog_seen.lock().unwrap().take() {
            g.note_dialog(&target, &kind, &message);
        }
        let _ = g.sync_targets().await;
        self.record(session, &g);
        // 脚本可能改了页面：旧快照的 refs 作废。
        g.refs.clear();
        let mut out = out;
        if !notes.is_empty() {
            out.text.push_str(&format!("\n\n{}", notes.join("\n")));
        }
        out
    }

    /// 杀掉执行器（超时的脚本停不下来，只能连进程一起停）。
    async fn reset_js(&self) {
        if let Some(runner) = self.js.lock().await.take() {
            runner.shutdown().await;
        }
    }

    /// 活着的执行器；没有或死了就起一个。
    async fn js_runner(&self) -> Result<Arc<JsRunner>, String> {
        let mut slot = self.js.lock().await;
        if let Some(r) = slot.as_ref() {
            if r.is_alive().await {
                return Ok(r.clone());
            }
        }
        let runner = Arc::new(JsRunner::start().await?);
        *slot = Some(runner.clone());
        Ok(runner)
    }

    async fn group(&self, session: &str) -> Option<Group> {
        self.groups.lock().await.get(session).cloned()
    }

    /// 本会话的组，且 Chromium 还活着。Chromium 没了（被关掉 / 崩了）就把所有组
    /// 作废——它们的页都跟着没了，留着只会在下一次调用报一串 CDP 错。
    /// 每次先和 Chrome 对一遍标签页（[`ConnectedSession::sync_targets`]）：去掉死页、收下页面
    /// 自己开的新页，变了就重写名册；一页不剩就当这个会话没开过。
    async fn live_group(&self, session: &str) -> Option<Group> {
        if !self.chromium_alive().await {
            self.forget_dead_chromium().await;
            return None;
        }
        let group = self.group(session).await?;
        let mut g = group.lock().await;
        if !matches!(g.sync_targets().await, Ok(true)) {
            drop(g);
            return Some(group);
        }
        if !g.is_empty() {
            self.record(session, &g);
            drop(g);
            return Some(group);
        }
        drop(g);
        self.forget(Some(session));
        {
            let mut groups = self.groups.lock().await;
            if groups.get(session).is_some_and(|g| Arc::ptr_eq(g, &group)) {
                groups.remove(session);
            }
        }
        if let Ok(g) = Arc::try_unwrap(group) {
            g.into_inner().shutdown().await;
        }
        None
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
            // 已经有页：等于 navigate，同样会被对话框挡住（含 beforeunload）。
            let watch = g.dialog_watch();
            if let Some(blocked) = blocked_by_dialog(&watch, "browser_open") {
                return Err(blocked.text);
            }
            let nav = async {
                match g.navigate(url).await {
                    Ok(cur) => CallOutput::ok(format!("navigated to {cur}")),
                    Err(e) => CallOutput::err(e),
                }
            };
            let out = guarded("browser_open", CALL_LIMIT, Some(watch), nav).await;
            return if out.is_error {
                Err(out.text)
            } else {
                Ok(out.text)
            };
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
        "browser_navigate_back" => session.navigate_back().await,
        "browser_snapshot" => {
            let interactive = arg_bool(args, "interactive").unwrap_or(true);
            let frame = arg_str(args, "frame").or_else(|| arg_str(args, "frame_selector"));
            // 对话框开着时走不到这里（[`blocked_by_dialog`] 先拦下）。
            Ok(session.snapshot(interactive, frame.as_deref()).await?.text)
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

/// `browser_js` 的脚本。规范是 `code`；模型常写成别的键，常见的几个也认。
fn js_code(args: &Value) -> Option<String> {
    ["code", "script", "expression", "js", "source"]
        .iter()
        .find_map(|k| arg_str(args, k).filter(|c| !c.trim().is_empty()))
}

/// 这次调用最多等多久。
fn call_limit(name: &str, args: &Value) -> Duration {
    if name == "browser_wait_for" {
        let wait = Duration::from_millis(arg_u64(args, "timeout_ms").unwrap_or(30_000).max(1));
        return wait + WAIT_FOR_MARGIN;
    }
    CALL_LIMIT
}

/// 本组有对话框开着、这次又不是去处理它：直接报错指路。开着对话框的页不响应别的
/// CDP 命令，发过去只会一直等。
fn blocked_by_dialog(watch: &DialogWatch, name: &str) -> Option<CallOutput> {
    if DIALOG_SAFE.contains(&name) {
        return None;
    }
    let d = watch.pending()?;
    Some(CallOutput::err(format!(
        "a JavaScript {} dialog is open on the page ({}). The page is blocked until it is closed — call browser_handle_dialog (accept true/false) first.",
        d.dialog_type,
        dialog_message(&d)
    )))
}

/// 跑一次调用，同时盯着两件事：
/// - 页上弹出 JS 对话框：点击 / 按键 / 跑脚本触发 `alert` / `confirm` / `prompt` 时，CDP 命令
///   要等对话框关掉才返回。对话框一弹出就先返回（不算失败），告诉模型弹了什么、去处理；
///   那条命令留在 Chrome 里，对话框关掉后自己结束。
/// - 超过 `limit`：报错，说清是哪一步卡住，不让调用无限挂着。
async fn guarded(
    name: &str,
    limit: Duration,
    watch: Option<DialogWatch>,
    call: impl std::future::Future<Output = CallOutput>,
) -> CallOutput {
    let mut watch = watch;
    if let Some(w) = watch.as_mut() {
        w.seq.borrow_and_update();
    }
    let dialog = async {
        match watch.as_mut() {
            // 发送端在本组里，组还在就不会关；关了就永远等不到，交给另外两支。
            Some(w) => {
                if w.seq.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
            // browser_js：对话框归执行器管（脚本可以先 onDialog 声明怎么处理）。
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        out = call => out,
        _ = dialog => match watch.as_ref().and_then(DialogWatch::pending) {
            Some(d) => CallOutput::ok(format!(
                "{name} opened a JavaScript {} dialog: {}. The page is blocked until you call browser_handle_dialog (accept true/false).",
                d.dialog_type,
                dialog_message(&d)
            )),
            // 对话框已经被关掉了（比如人在面板里点掉）：照常说明，不报错。
            None => CallOutput::ok(format!(
                "{name} opened a JavaScript dialog that was already closed."
            )),
        },
        _ = tokio::time::sleep(limit) => CallOutput::err(format!(
            "{name} did not finish within {}s — the page may be hung (busy script or blocked renderer). Take a browser_snapshot, or browser_close and browser_open again.",
            limit.as_secs()
        )),
    }
}

fn dialog_message(d: &PendingDialog) -> String {
    let mut out = format!("{:?}", d.message.replace('\n', " "));
    if let Some(p) = d.default_prompt.as_ref().filter(|s| !s.is_empty()) {
        out.push_str(&format!(", default {p:?}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 模型常把脚本放在别的键里（实测两次 `code is required`）：常见别名都认。
    #[test]
    fn browser_js_accepts_common_names_for_the_code() {
        for key in ["code", "script", "expression", "js", "source"] {
            assert_eq!(
                js_code(&json!({ key: "page.url()" })).as_deref(),
                Some("page.url()"),
                "{key}"
            );
        }
        assert_eq!(js_code(&json!({"code": "  "})), None);
        assert_eq!(js_code(&json!({"title": "x"})), None);
    }

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
