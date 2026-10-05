//! Origin pairing, dock.1 handshake, turn projection, permission dual-resolve.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use cordis_gateway::{
    devices, gateway_bind, gateway_idle, gateway_remote, gateway_serve, ServeConfig, ServeControl,
    GATEWAY, GATEWAY_SERVE, PROTOCOL_VERSION,
};
use cordis_spine::{
    agent_loop, agent_presets, compact, dynamic_runner, install_fakes, install_without_llm,
    mcp_client, permissions, plan_mode, settings, slash, tool_ask_user, tool_goal, tool_task, turn,
    AgentPresets, AppSettings, BoxFuture, Compact, ExtraSlashKind, Goal, Llm, LlmOutput, LogEvent,
    LoopHandle, Mcp, PermissionOptionKind, Permissions, PlanMode, PromptRequest, Sampler, Sessions,
    Slash, SlashEntry, StreamDelta, TaskConfig, ToolCall, Tools, TurnControl, AGENT_LOOP,
    AGENT_PRESETS, COMPACT, GOAL, LLM, MCP, PERMISSIONS, PLAN_MODE, SESSIONS, SETTINGS, SLASH,
    TOOLS, TURN,
};
use cordis_tui::{QueuedItem, SessionPort, SessionRef, SESSION_PORT};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::ORIGIN as ORIGIN_HEADER;
use tokio_tungstenite::tungstenite::Message;

const PAGE_ORIGIN: &str = "http://localhost:5173";
const APP: &str = "example-app";

#[derive(Clone)]
struct TestSession {
    ctx: Context,
    working: Arc<AtomicBool>,
}

impl SessionPort for TestSession {
    fn submit(&self, text: String, _send_now: bool) {
        let ctx = self.ctx.clone();
        let working = self.working.clone();
        working.store(true, Ordering::SeqCst);
        tokio::spawn(async move {
            if let Some(handle) = ctx.get::<LoopHandle>(AGENT_LOOP) {
                let _ = handle.run(text).await;
            }
            working.store(false, Ordering::SeqCst);
        });
    }

    fn working(&self) -> bool {
        self.working.load(Ordering::SeqCst)
    }

    fn cancel(&self) {
        if let Some(turn) = self.ctx.get::<TurnControl>(TURN) {
            turn.cancel();
        }
    }

    fn has_queued(&self) -> bool {
        false
    }

    fn compact(&self, context: String) {
        let ctx = self.ctx.clone();
        tokio::spawn(async move {
            if let Some(compact) = ctx.get::<Compact>(COMPACT) {
                let extra = (!context.is_empty()).then_some(context.as_str());
                let _ = compact.run_on(&ctx, extra).await;
            }
        });
    }

    fn queued_prompts(&self) -> Vec<QueuedItem> {
        Vec::new()
    }

    fn promote(&self, _id: Option<String>) {}

    fn take_queued(&self, _id: Option<String>) -> Option<QueuedItem> {
        None
    }
}

struct Harness {
    ctx: Context,
    addr: SocketAddr,
    http: reqwest::Client,
}

/// 这个测试二进制共用一个隔离的 `DOCK_HOME`：多线程用例的分页会落盘，
/// 不能写进本机 `~/.dock`，也不能读到本机的模型配置。
fn isolated_home() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("dock-gateway-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DOCK_HOME", &dir);
        std::env::set_var("DOCK_CUA_DRIVER", "off");
    });
}

/// 一个存在的、这次独有的项目目录。
fn project_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("dock-gw-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// 测试用的建页工厂：每页自己的会话（落盘）、设置、权限队列、轮次、循环和
/// session.port——和 `cordis-app` 的真分页一样按 `PER_TAB_SERVICES` 隔离。
fn test_page_mount() -> cordis_tui::TabMount {
    Arc::new(|index, _kind| {
        cordis::plugin_async(
            "test.page",
            cordis::Inject::new(),
            move |ctx, _: &()| async move {
                let sessions = Sessions::tab(ctx.clone(), index);
                sessions.attach_disk();
                // 预设按页：和 `cordis-app` 的 `tab.presets` 一样从上层 fork 一份。
                let presets = ctx
                    .get::<cordis_tui::Tabs>(cordis_tui::TUI_TABS)
                    .and_then(|tabs| tabs.active_ctx().get::<AgentPresets>(AGENT_PRESETS))
                    .map(|p| p.fork());
                if let Some(presets) = &presets {
                    sessions.seed_preset_if_unset(&presets.current_id());
                }
                let _ = ctx.provide(SESSIONS, sessions)?;
                if let Some(presets) = presets {
                    let _ = ctx.provide(AGENT_PRESETS, presets)?;
                }
                ctx.plugin(settings(), ())?.wait().await?;
                ctx.plugin(permissions(), ())?.wait().await?;
                ctx.plugin(turn(), ())?.wait().await?;
                ctx.plugin(agent_loop(), ())?.wait().await?;
                let port = TestSession {
                    ctx: ctx.clone(),
                    working: Arc::new(AtomicBool::new(false)),
                };
                let _ = ctx.provide(
                    SESSION_PORT,
                    SessionRef::new(Arc::new(port) as Arc<dyn SessionPort>),
                )?;
                Ok(None)
            },
        )
    })
}

async fn harness_root() -> Context {
    harness_root_with(None).await
}

/// 同 [`harness_root`]；给了 `sampler` 就用它当模型，不用 echo 假模型。
async fn harness_root_with(sampler: Option<Arc<dyn Sampler>>) -> Context {
    isolated_home();
    let root = Context::new();
    match sampler {
        Some(sampler) => {
            install_without_llm(&root).await.unwrap();
            root.provide(LLM, Llm::from_sampler(root.clone(), sampler))
                .unwrap();
        }
        None => install_fakes(&root).await.unwrap(),
    }
    root.plugin(settings(), ()).unwrap().wait().await.unwrap();
    root.plugin(turn(), ()).unwrap().wait().await.unwrap();
    root.plugin(permissions(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(plan_mode(), ()).unwrap().wait().await.unwrap();
    root.plugin(tool_ask_user(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(tool_goal(), ()).unwrap().wait().await.unwrap();
    root.plugin(slash(), ()).unwrap().wait().await.unwrap();
    root.plugin(mcp_client(), ()).unwrap().wait().await.unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    let port = TestSession {
        ctx: root.clone(),
        working: Arc::new(AtomicBool::new(false)),
    };
    root.provide(
        SESSION_PORT,
        SessionRef::new(Arc::new(port) as Arc<dyn SessionPort>),
    )
    .unwrap();
    root
}

impl Harness {
    async fn boot() -> Self {
        Self::boot_on(harness_root().await).await
    }

    /// 带分页服务：网关能按线程开页（`thread/open` / `thread/start {cwd}`）。
    async fn boot_with_pages() -> Self {
        let root = harness_root().await;
        root.plugin(agent_presets(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        root.plugin(cordis_tui::tabs(), test_page_mount())
            .unwrap()
            .wait()
            .await
            .unwrap();
        Self::boot_on(root).await
    }

    async fn boot_on(root: Context) -> Self {
        Self::boot_with(root, gateway_bind("127.0.0.1:0")).await
    }

    /// 远程模式（`dock serve --remote`）的网关。
    async fn boot_remote() -> Self {
        Self::boot_with(harness_root().await, gateway_remote("127.0.0.1:0")).await
    }

    async fn boot_with(root: Context, gateway: cordis::Plugin) -> Self {
        root.plugin(gateway, ()).unwrap().wait().await.unwrap();
        let addr = root
            .require::<cordis_tui::GatewayRef>(GATEWAY)
            .unwrap()
            .local_addr();
        assert!(root
            .require::<cordis_tui::GatewayRef>(GATEWAY)
            .unwrap()
            .is_listening());
        let http = reqwest::Client::new();
        let harness = Self {
            ctx: root,
            addr,
            http,
        };
        harness.wait_ready().await;
        harness
    }

    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    async fn wait_ready(&self) {
        for _ in 0..80 {
            if self
                .http
                .get(format!("{}/nope", self.base()))
                .header("Origin", PAGE_ORIGIN)
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("gateway did not start on {}", self.addr);
    }

    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        self.http
            .post(format!("{}{path}", self.base()))
            .header("Origin", PAGE_ORIGIN)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        self.http
            .get(format!("{}{path}", self.base()))
            .header("Origin", PAGE_ORIGIN)
            .send()
            .await
            .unwrap()
    }

    fn gateway(&self) -> cordis_tui::GatewayRef {
        (*self.ctx.require::<cordis_tui::GatewayRef>(GATEWAY).unwrap()).clone()
    }

    async fn pair_ticket(&self) -> String {
        let created = self
            .post("/v1/pairing/requests", json!({ "application": APP }))
            .await;
        assert_eq!(created.status(), 200);
        let body: Value = created.json().await.unwrap();
        let id = body["pairingRequestId"].as_str().unwrap().to_string();
        self.gateway().pairing_confirm(&id).unwrap();
        let polled = self.get(&format!("/v1/pairing/requests/{id}")).await;
        let poll: Value = polled.json().await.unwrap();
        assert_eq!(poll["status"], "approved");
        assert!(poll.get("ticket").is_none() || poll["ticket"].is_null());
        let exchanged = self
            .post("/v1/pairing/exchanges", json!({ "pairingRequestId": id }))
            .await;
        assert_eq!(exchanged.status(), 200);
        let body: Value = exchanged.json().await.unwrap();
        body["ticket"].as_str().unwrap().to_string()
    }
}

struct Rpc {
    write: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    read: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    next_id: i64,
}

impl Rpc {
    async fn connect(addr: SocketAddr, ticket: &str) -> Self {
        let (mut rpc, auth) = Self::connect_as(addr, ticket, PAGE_ORIGIN).await;
        assert_eq!(auth["result"]["ok"], true);
        rpc.next_id = 2;
        rpc
    }

    /// 以 `origin` 握手并用 `ticket` 鉴权，回鉴权那一帧（不断言成败）。
    async fn connect_as(addr: SocketAddr, ticket: &str, origin: &str) -> (Self, Value) {
        let mut rpc = Self::open(addr, origin).await;
        let auth = rpc
            .call("connection/authenticate", json!({ "ticket": ticket }))
            .await;
        (rpc, auth)
    }

    /// 只握手、不鉴权。
    async fn open(addr: SocketAddr, origin: &str) -> Self {
        let url = format!("ws://{addr}/api/ws");
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert(ORIGIN_HEADER, origin.parse().unwrap());
        let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        let (write, read) = ws.split();
        Self {
            write,
            read,
            next_id: 1,
        }
    }

    /// 等服务端关连接，回关闭码（没带关闭帧就断了回 `None`）。
    async fn wait_closed(&mut self, deadline: Duration) -> Option<u16> {
        let start = tokio::time::Instant::now();
        loop {
            let left = deadline.saturating_sub(start.elapsed());
            let msg = tokio::time::timeout(left, self.read.next())
                .await
                .expect("server did not close the connection in time");
            match msg {
                Some(Ok(Message::Close(frame))) => return frame.map(|f| u16::from(f.code)),
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => return None,
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write
            .send(Message::Text(
                json!({ "id": id, "method": method, "params": params })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), self.read.next())
                .await
                .expect("ws timeout")
                .expect("ws closed")
                .unwrap();
            let text = msg.into_text().expect("text frame");
            let value: Value = serde_json::from_str(&text).unwrap();
            if value.get("id") == Some(&json!(id)) {
                return value;
            }
        }
    }

    /// 只发不等，回请求 id（连发几个请求测并发用）。
    async fn send(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.write
            .send(Message::Text(
                json!({ "id": id, "method": method, "params": params })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        id
    }

    /// 收齐 `ids` 的回包；顺手收下其间的推送。
    async fn collect(
        &mut self,
        ids: &[i64],
        deadline: Duration,
    ) -> (HashMap<i64, Value>, Vec<Value>) {
        let start = tokio::time::Instant::now();
        let mut replies = HashMap::new();
        let mut notes = Vec::new();
        while replies.len() < ids.len() {
            let left = deadline.saturating_sub(start.elapsed());
            let msg = tokio::time::timeout(left, self.read.next())
                .await
                .unwrap_or_else(|_| panic!("timed out; got {replies:?}"))
                .expect("ws closed")
                .unwrap();
            let value: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
            match value.get("id").and_then(Value::as_i64) {
                Some(id) if ids.contains(&id) => {
                    replies.insert(id, value);
                }
                _ => notes.push(value),
            }
        }
        (replies, notes)
    }

    async fn wait_notification(&mut self, method: &str, deadline: Duration) -> Value {
        let start = tokio::time::Instant::now();
        loop {
            let left = deadline.saturating_sub(start.elapsed());
            let msg = tokio::time::timeout(left, self.read.next())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {method}"))
                .expect("ws closed")
                .unwrap();
            let text = msg.into_text().expect("text frame");
            let value: Value = serde_json::from_str(&text).unwrap();
            if value.get("method").and_then(Value::as_str) == Some(method) {
                return value;
            }
        }
    }
}

#[tokio::test]
async fn handshake_initialize_is_dock1() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(init["result"]["capabilities"]["turns"], true);
    assert_eq!(init["result"]["capabilities"]["permissions"], true);
    assert_eq!(init["result"]["capabilities"]["transcriptEvents"], true);
    assert_eq!(init["result"]["capabilities"]["threadSubscriptions"], true);
    assert_eq!(init["result"]["capabilities"]["threads"], true);
    assert_eq!(init["result"]["capabilities"]["hostTools"], false);
    assert_eq!(init["result"]["capabilities"]["imageInputs"], true);
    assert_eq!(init["result"]["capabilities"]["slash"], true);
    assert_eq!(
        init["result"]["connection"]["companion"]["status"],
        "listening"
    );
    let companion = init["result"]["connection"]["companion"]["address"]
        .as_str()
        .unwrap();
    assert!(companion.starts_with("[::1]:"), "{companion}");
    let workspaces = rpc.call("workspace/list", json!({})).await;
    assert_eq!(workspaces["result"]["defaultWorkspaceId"], "default");
}

#[tokio::test]
async fn pairing_deny_poll_has_no_ticket() {
    let h = Harness::boot().await;
    let created = h
        .post("/v1/pairing/requests", json!({ "application": APP }))
        .await;
    let body: Value = created.json().await.unwrap();
    let id = body["pairingRequestId"].as_str().unwrap();
    h.gateway().pairing_deny(id).unwrap();
    let poll: Value = h
        .get(&format!("/v1/pairing/requests/{id}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(poll["status"], "denied");
    assert!(poll.get("ticket").is_none() || poll["ticket"].is_null());
    let exchange = h
        .post("/v1/pairing/exchanges", json!({ "pairingRequestId": id }))
        .await;
    assert_eq!(exchange.status(), 403);
}

#[tokio::test]
async fn pairing_revoke_drops_binding() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    assert!(!ticket.is_empty());
    let again = h
        .post("/v1/connection/tickets", json!({ "application": APP }))
        .await;
    assert_eq!(again.status(), 200);
    h.gateway().pairing_revoke(PAGE_ORIGIN).unwrap();
    let denied = h
        .post("/v1/connection/tickets", json!({ "application": APP }))
        .await;
    assert_eq!(denied.status(), 403);
}

#[tokio::test]
async fn pairing_requires_origin() {
    let h = Harness::boot().await;
    let res = h
        .http
        .post(format!("{}/v1/pairing/requests", h.base()))
        .json(&json!({ "application": APP }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "origin_required");
}

#[tokio::test]
async fn websocket_without_origin_cannot_authenticate() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let url = format!("ws://{}/api/ws", h.addr);
    let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let (mut write, mut read) = ws.split();
    write
        .send(Message::Text(
            json!({
                "id": 1,
                "method": "connection/authenticate",
                "params": { "ticket": ticket }
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(3), read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    assert_eq!(value["error"]["details"]["code"], "origin_required");
}

#[tokio::test]
async fn turn_start_projects_echo() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let sub = rpc
        .call("thread/subscribe", json!({ "threadId": "live" }))
        .await;
    assert_eq!(sub["result"]["ok"], true);
    let started = rpc.call("turn/start", json!({ "message": "hello" })).await;
    assert!(started.get("error").is_none(), "{started}");
    let user = rpc
        .wait_notification("item/user_message", Duration::from_secs(5))
        .await;
    assert!(user["params"]["content"]
        .as_str()
        .unwrap()
        .contains("hello"));
    let mut saw_echo = false;
    for _ in 0..50 {
        let msg = tokio::time::timeout(Duration::from_millis(100), rpc.read.next()).await;
        let Ok(Some(Ok(frame))) = msg else {
            continue;
        };
        let Ok(text) = frame.into_text() else {
            continue;
        };
        if text.contains("echoed: hello") {
            saw_echo = true;
            break;
        }
    }
    assert!(saw_echo, "expected projected echo text");
}

struct EchoLastUser;

impl Sampler for EchoLastUser {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let text = request
            .history
            .iter()
            .rev()
            .find_map(|e| match e {
                LogEvent::User(t) => Some(t.clone()),
                _ => None,
            })
            .unwrap_or_default();
        Box::pin(async move {
            on_delta(StreamDelta::Text(text.clone()));
            LlmOutput {
                text,
                ..LlmOutput::default()
            }
        })
    }
}

/// 压缩那几秒客户端要看得到进展：`context/compacted` 推运行中与完成（带前后
/// 占用），完成标记是 `item/compaction` 而不是一条助手消息；中途接入的客户端从
/// `context.lastCompaction` 拿状态。
#[tokio::test]
async fn compaction_progress_reaches_subscribers() {
    // echo 假模型总是调工具，摘要永远不成；换一个把最后一条用户消息原样
    // 写回来的——压缩时那就是摘要提示词，够长、不调工具。
    let root = harness_root_with(Some(Arc::new(EchoLastUser))).await;
    root.plugin(compact(), ()).unwrap().wait().await.unwrap();
    let h = Harness::boot_on(root).await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": "live" }))
        .await;
    let _ = rpc.call("turn/start", json!({ "message": "hello" })).await;
    rpc.wait_notification("turn/completed", Duration::from_secs(5))
        .await;

    let asked = rpc
        .send("thread/context/compact", json!({ "threadId": "live" }))
        .await;
    let (_, mut notes) = rpc.collect(&[asked], Duration::from_secs(5)).await;
    let start = tokio::time::Instant::now();
    // 完成标记（`item/compaction`）先到，带前后占用的完成推送随后。
    while !notes
        .iter()
        .any(|n| n["method"] == "context/compacted" && n["params"]["status"] != "running")
    {
        let left = Duration::from_secs(5).saturating_sub(start.elapsed());
        let msg = tokio::time::timeout(left, rpc.read.next())
            .await
            .unwrap_or_else(|_| panic!("等压缩结束超时：{notes:#?}"))
            .unwrap()
            .unwrap();
        notes.push(serde_json::from_str(&msg.into_text().unwrap()).unwrap());
    }
    let progress: Vec<&Value> = notes
        .iter()
        .filter(|n| n["method"] == "context/compacted")
        .map(|n| &n["params"])
        .collect();
    assert_eq!(
        progress.first().map(|p| &p["status"]),
        Some(&json!("running"))
    );
    assert_eq!(progress[0]["trigger"], "manual");
    let done = progress
        .iter()
        .find(|p| p["status"] == "completed")
        .unwrap_or_else(|| panic!("要推完成：{progress:?}"));
    assert!(done["afterTokens"].as_u64().is_some(), "{done}");
    assert!(notes.iter().any(|n| n["method"] == "item/compaction"));
    assert!(
        !notes.iter().any(|n| n["method"] == "item/message_delta"
            && n["params"]["delta"] == cordis_spine::COMPACT_NOTICE),
        "压缩通知不该再是一条助手消息"
    );

    let env = rpc
        .call("thread/environment/get", json!({ "threadId": "live" }))
        .await;
    let last = &env["result"]["context"]["lastCompaction"];
    assert_eq!(last["status"], "completed", "{env}");
    assert_eq!(last["trigger"], "manual");
}

#[tokio::test]
async fn permission_dual_resolve_first_wins() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let perms = h.ctx.require::<Permissions>(PERMISSIONS).unwrap();
    let waiter = {
        let perms = perms.clone();
        tokio::spawn(async move { perms.request("bash", "run ls").await })
    };
    for _ in 0..50 {
        if perms.front().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(perms.front().is_some());
    let resolved = rpc
        .call(
            "permission/resolve",
            json!({ "decision": "approve", "always": false }),
        )
        .await;
    assert_eq!(resolved["result"]["ok"], true);
    let allowed = tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(allowed);
    let empty = rpc
        .call("permission/resolve", json!({ "decision": "deny" }))
        .await;
    assert_eq!(empty["error"]["details"]["code"], "empty_queue");
    assert!(!perms.resolve(PermissionOptionKind::AllowOnce));
}

#[tokio::test]
async fn thread_archive_rename_delete_use_sessions() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let sessions = h.ctx.require::<Sessions>(SESSIONS).unwrap();
    sessions.append(LogEvent::User("hello archive".into()));
    let archived = rpc
        .call("thread/archive", json!({ "threadId": "live" }))
        .await;
    assert!(archived.get("error").is_none(), "{archived}");
    let id = archived["result"]["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(id.starts_with('s'));
    assert_eq!(archived["result"]["thread"]["title"], "hello archive");
    let renamed = rpc
        .call(
            "thread/rename",
            json!({ "threadId": id, "title": "renamed" }),
        )
        .await;
    assert_eq!(renamed["result"]["thread"]["title"], "renamed");
    let live_rename = rpc
        .call(
            "thread/rename",
            json!({ "threadId": "live", "title": "new chat" }),
        )
        .await;
    assert_eq!(live_rename["result"]["thread"]["title"], "new chat");
    let deleted = rpc
        .call(
            "thread/delete",
            json!({ "threadId": id, "confirmation": id }),
        )
        .await;
    assert_eq!(deleted["result"]["ok"], true);
    assert!(sessions.archived().is_empty());
}

#[tokio::test]
async fn reasoning_and_goal_project_dock_services() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let reasoning = rpc
        .call(
            "thread/reasoning/set",
            json!({ "threadId": "live", "effort": "high" }),
        )
        .await;
    assert_eq!(reasoning["result"]["reasoning"]["effort"], "high");
    let settings = h.ctx.require::<AppSettings>(SETTINGS).unwrap();
    assert_eq!(settings.effort(), "high");
    assert!(settings.thinking());
    let started = rpc
        .call(
            "thread/goal/set",
            json!({ "threadId": "live", "content": "ship the gateway" }),
        )
        .await;
    assert_eq!(started["result"]["goal"]["status"], "active");
    assert_eq!(started["result"]["goal"]["summary"], "ship the gateway");
    let paused = rpc
        .call("thread/goal/pause", json!({ "threadId": "live" }))
        .await;
    assert_eq!(paused["result"]["goal"]["status"], "paused");
    assert!(h.ctx.require::<Goal>(GOAL).unwrap().paused());
    let cleared = rpc
        .call("thread/goal/clear", json!({ "threadId": "live" }))
        .await;
    assert_eq!(cleared["result"]["goal"]["status"], "none");
}

/// 模型报完成后目标不是「没有」：客户端要显示「已完成 · 用时」，所以投影成 `completed`
/// 并带上实际跑的时长；清掉之后才回到 `none`。
#[tokio::test]
async fn goal_completion_projects_completed_with_elapsed_time() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let started = rpc
        .call(
            "thread/goal/set",
            json!({ "threadId": "live", "content": "ship the gateway" }),
        )
        .await;
    assert_eq!(started["result"]["goal"]["status"], "active");
    assert!(started["result"]["goal"]["elapsedMs"].is_u64(), "{started}");
    let tools = h.ctx.require::<Tools>(TOOLS).unwrap();
    let done = tools
        .execute(ToolCall {
            id: "g1".into(),
            name: "update_goal".into(),
            arguments: r#"{"completed":true,"message":"shipped"}"#.into(),
        })
        .await;
    assert!(!done.is_error, "{}", done.content);
    let env = rpc
        .call("thread/environment/get", json!({ "threadId": "live" }))
        .await;
    let goal = &env["result"]["goal"];
    assert_eq!(goal["status"], "completed", "{env}");
    assert_eq!(goal["summary"], "ship the gateway");
    assert_eq!(goal["progressSummary"], "shipped");
    let cleared = rpc
        .call("thread/goal/clear", json!({ "threadId": "live" }))
        .await;
    assert_eq!(cleared["result"]["goal"]["status"], "none");
}

/// `thread/context/get` 的各片互不重叠、加起来正好是整个窗口；每片带明细。
#[tokio::test]
async fn context_breakdown_slices_cover_the_window() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let _ = rpc
        .call(
            "turn/start",
            json!({ "threadId": "live", "message": "hello" }),
        )
        .await;
    let r = rpc
        .call("thread/context/get", json!({ "threadId": "live" }))
        .await;
    let c = &r["result"];
    assert!(r.get("error").is_none(), "{r}");
    let total = c["maxContextTokens"].as_u64().unwrap();
    let used = c["usedTokens"].as_u64().unwrap();
    assert!(total > 0, "{c}");
    let slices = c["slices"].as_array().unwrap();
    let ids: Vec<&str> = slices.iter().map(|s| s["id"].as_str().unwrap()).collect();
    for id in ["system", "tools", "messages", "free"] {
        assert!(ids.contains(&id), "缺 {id}：{ids:?}");
    }
    let sum: u64 = slices.iter().map(|s| s["tokens"].as_u64().unwrap()).sum();
    assert_eq!(sum, used.max(total), "各片应当加起来是整个窗口：{c}");
    let used_sum: u64 = slices
        .iter()
        .filter(|s| s["group"] != "free")
        .map(|s| s["tokens"].as_u64().unwrap())
        .sum();
    assert_eq!(used_sum, used, "{c}");
    let system = slices.iter().find(|s| s["id"] == "system").unwrap();
    assert!(
        !system["detail"].as_array().unwrap().is_empty(),
        "系统提示要有分段明细：{system}"
    );
    assert!(c["onDemand"].is_array());
    let caps = rpc.call("initialize", json!({})).await;
    assert_eq!(
        caps["result"]["capabilities"]["contextBreakdown"], true,
        "{caps}"
    );
}

/// #176：有锚点时 `usedTokens` 是上游真账，分项是估算。估算偏高时分项要缩进
/// 已用里——工具定义不能被「已用 − 系统 − 消息」减成 0，各片仍然正好铺满窗口。
#[tokio::test]
async fn context_breakdown_fits_estimates_inside_the_upstream_total() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let sessions = h.ctx.require::<Sessions>(SESSIONS).unwrap();
    sessions.append(LogEvent::User("写一页".into()));
    sessions.append(LogEvent::ToolExecute {
        id: "c1".into(),
        name: "read_file".into(),
        arguments: "{}".into(),
        content: "中".repeat(30_000),
        images: Vec::new(),
        is_error: false,
    });
    // 上游报的远小于估算（中文按字估 1 token 偏高）。
    sessions.begin_llm();
    sessions.apply_llm_delta(&StreamDelta::Usage {
        tokens: cordis_base::usage::TokenUsage {
            prompt_tokens: 12_000,
            completion_tokens: 100,
            ..Default::default()
        },
        official: true,
        model: "m".into(),
        cost_usd_ticks: None,
    });
    sessions.finish_llm(&LlmOutput {
        text: "ok".into(),
        ..Default::default()
    });

    let r = rpc
        .call("thread/context/get", json!({ "threadId": "live" }))
        .await;
    let c = &r["result"];
    let used = c["usedTokens"].as_u64().unwrap();
    let total = c["maxContextTokens"].as_u64().unwrap();
    let slices = c["slices"].as_array().unwrap();
    let get = |id: &str| {
        slices
            .iter()
            .find(|s| s["id"] == id)
            .map_or(0, |s| s["tokens"].as_u64().unwrap())
    };
    assert!(get("messages") <= used, "消息不能超过已用：{c}");
    assert!(get("tools") > 0, "有工具时工具定义不能是 0：{c}");
    let sum: u64 = slices.iter().map(|s| s["tokens"].as_u64().unwrap()).sum();
    assert_eq!(sum, used.max(total), "{c}");
}

#[tokio::test]
async fn turn_intent_starts_goal_and_plan() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": "live" }))
        .await;
    let started = rpc
        .call(
            "turn/start",
            json!({
                "message": "finish pairing",
                "intent": { "mode": "plan", "goal": { "operation": "start", "objective": "finish pairing" } }
            }),
        )
        .await;
    assert!(started.get("error").is_none(), "{started}");
    assert!(h.ctx.require::<Goal>(GOAL).unwrap().active());
}

#[tokio::test]
async fn mcp_and_model_settings_methods_are_implemented() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let reload = rpc.call("mcp/reload", json!({})).await;
    assert_eq!(reload["result"]["ok"], true);
    // 真重载：带这次做了什么和每台服务器的状态（以前只回一个数）。
    assert!(reload["result"]["summary"].is_string(), "{reload}");
    assert!(reload["result"]["servers"].is_array(), "{reload}");
    let listed = rpc.call("mcp/list", json!({})).await;
    assert!(listed["result"]["servers"].is_array(), "{listed}");
    let unknown = rpc
        .call("mcp/reconnect", json!({ "name": "no-such-server" }))
        .await;
    assert_eq!(
        unknown["error"]["details"]["code"], "reconnect_failed",
        "{unknown}"
    );
    let models = rpc.call("model/list", json!({})).await;
    assert!(models["result"]["models"].is_array(), "{models}");
    assert!(models["result"]["default"].is_string(), "{models}");
    assert!(
        !models.to_string().contains("api_key"),
        "不能把密钥字段带出去：{models}"
    );
    let refresh = rpc
        .call("thread/model/refresh", json!({ "threadId": "live" }))
        .await;
    assert!(refresh["result"]["model"]["id"].is_string());
}

#[tokio::test]
async fn slash_list_includes_harness_and_screenshot() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let slash = h.ctx.require::<Slash>(SLASH).unwrap();
    let _extra = slash
        .register(SlashEntry {
            command: "memo".into(),
            description: "便签".into(),
            kind: ExtraSlashKind::Overlay,
            text: "hello overlay".into(),
            title: "Memo".into(),
            send: false,
        })
        .unwrap();
    let listed = rpc.call("slash/list", json!({})).await;
    assert!(listed.get("error").is_none(), "{listed}");
    let names: Vec<&str> = listed["result"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    for expected in [
        "new",
        "plan",
        "goal",
        "compact",
        "screenshot",
        "screenshot --region",
        "memo",
        "pair",
        "cd",
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
    let cd = listed["result"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "cd")
        .unwrap();
    assert_eq!(cd["surface"], "terminal");
    let settings = listed["result"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "settings")
        .unwrap();
    assert_eq!(settings["surface"], "terminal");

    use std::collections::HashSet;
    let tui: HashSet<String> = cordis_tui::slash_catalog()
        .flat_map(|e| {
            std::iter::once(e.name.to_string()).chain(e.aliases.iter().map(|a| (*a).to_string()))
        })
        .collect();
    let gw: HashSet<String> = listed["result"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "command")
        .filter(|c| !c["name"].as_str().unwrap_or("").contains(' '))
        .flat_map(|c| {
            let name = c["name"].as_str().unwrap().to_string();
            let aliases: Vec<String> = c["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str().map(str::to_string))
                .collect();
            std::iter::once(name).chain(aliases)
        })
        .collect();
    assert_eq!(gw, tui, "slash/list builtins drifted from TUI CATALOG");
}

#[tokio::test]
async fn slash_execute_dispatches_to_spine() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": "live" }))
        .await;

    let filled = rpc.call("slash/execute", json!({ "text": "/goal" })).await;
    assert_eq!(filled["result"]["kind"], "filled");
    assert!(filled["result"]["fill"].as_str().unwrap().contains("/goal"));

    let capture = rpc
        .call(
            "slash/execute",
            json!({ "text": "/screenshot --region 看这里" }),
        )
        .await;
    assert_eq!(capture["result"]["kind"], "capture");

    let passthrough = rpc
        .call("slash/execute", json!({ "text": "/not-a-real-command" }))
        .await;
    assert_eq!(passthrough["result"]["kind"], "passthrough");

    // `/protocol` 空参数列可选协议；没在 api_backends 里声明的协议不能被切进去
    // （harness 的模型不在任何目录里 = 三条通用协议都可选，所以拿一个假名字试）。
    let protocols = rpc
        .call("slash/execute", json!({ "text": "/protocol" }))
        .await;
    assert_eq!(protocols["result"]["kind"], "notice");
    assert!(
        protocols["result"]["notice"]["body"]
            .as_str()
            .unwrap()
            .contains("responses"),
        "{protocols}"
    );
    let switched = rpc
        .call("slash/execute", json!({ "text": "/protocol messages" }))
        .await;
    assert_eq!(switched["result"]["kind"], "applied");
    assert_eq!(
        h.ctx.require::<AppSettings>(SETTINGS).unwrap().backend(),
        cordis_spine::ApiBackend::Messages
    );
    let rejected = rpc
        .call("slash/execute", json!({ "text": "/protocol grpc" }))
        .await;
    assert!(rejected["result"]["notice"]["body"]
        .as_str()
        .unwrap()
        .contains("未知协议"));

    let terminal = rpc.call("slash/execute", json!({ "text": "/pair" })).await;
    assert_eq!(terminal["result"]["kind"], "notice");
    assert!(terminal["result"]["notice"]["body"]
        .as_str()
        .unwrap()
        .contains("终端"));

    let cwd_before = std::env::current_dir().unwrap();
    let cd = rpc
        .call("slash/execute", json!({ "text": "/cd /tmp" }))
        .await;
    assert_eq!(cd["result"]["kind"], "notice");
    assert!(cd["result"]["notice"]["body"]
        .as_str()
        .unwrap()
        .contains("终端"));
    assert_eq!(std::env::current_dir().unwrap(), cwd_before);

    let settings_svc = h.ctx.require::<AppSettings>(SETTINGS).unwrap();
    let ts_before = settings_svc.timestamps();
    let settings_cmd = rpc
        .call("slash/execute", json!({ "text": "/settings timestamps" }))
        .await;
    assert_eq!(settings_cmd["result"]["kind"], "notice");
    assert!(settings_cmd["result"]["notice"]["body"]
        .as_str()
        .unwrap()
        .contains("终端"));
    assert_eq!(settings_svc.timestamps(), ts_before);

    let alias = rpc
        .call("slash/execute", json!({ "text": "/config timestamps" }))
        .await;
    assert_eq!(alias["result"]["kind"], "notice");
    assert!(alias["result"]["notice"]["body"]
        .as_str()
        .unwrap()
        .contains("终端"));
    assert_eq!(settings_svc.timestamps(), ts_before);

    let bad_thread = rpc
        .call("slash/execute", json!({ "text": "/new", "threadId": "s1" }))
        .await;
    assert!(
        bad_thread.get("error").is_some(),
        "archived threadId must be rejected: {bad_thread}"
    );

    let live_ok = rpc
        .call(
            "slash/execute",
            json!({ "text": "/help", "threadId": "live" }),
        )
        .await;
    assert_eq!(live_ok["result"]["kind"], "notice");

    let sessions = h.ctx.require::<Sessions>(SESSIONS).unwrap();
    sessions.append(LogEvent::User("keep me".into()));
    let fresh = rpc.call("slash/execute", json!({ "text": "/new" })).await;
    assert_eq!(fresh["result"]["kind"], "applied");
    assert!(sessions.events().is_empty());

    let started = rpc
        .call("slash/execute", json!({ "text": "/plan finish pairing" }))
        .await;
    assert_eq!(started["result"]["kind"], "submitted");
    assert_eq!(
        h.ctx.require::<PlanMode>(PLAN_MODE).unwrap().phase(),
        cordis_spine::PlanPhase::Active
    );

    let goal = rpc
        .call("slash/execute", json!({ "text": "/goal ship the gateway" }))
        .await;
    assert_eq!(goal["result"]["kind"], "submitted");
    assert!(h.ctx.require::<Goal>(GOAL).unwrap().active());
}

#[tokio::test]
async fn slash_execute_runs_extra_overlay() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let _extra = h
        .ctx
        .require::<Slash>(SLASH)
        .unwrap()
        .register(SlashEntry {
            command: "memo".into(),
            description: "便签".into(),
            kind: ExtraSlashKind::Overlay,
            text: "hello overlay".into(),
            title: "Memo".into(),
            send: false,
        })
        .unwrap();
    let executed = rpc.call("slash/execute", json!({ "text": "/memo" })).await;
    assert_eq!(executed["result"]["kind"], "notice");
    assert_eq!(executed["result"]["notice"]["title"], "Memo");
    assert_eq!(executed["result"]["notice"]["body"], "hello overlay");
}

#[tokio::test]
async fn companion_ipv6_serves_http() {
    let h = Harness::boot().await;
    let status = h.gateway().companion_status();
    let addr = match status {
        cordis_tui::CompanionStatus::Listening(addr) => addr,
        cordis_tui::CompanionStatus::Failed { addr, error } => {
            panic!("companion {addr} failed: {error}");
        }
        cordis_tui::CompanionStatus::Stopped => {
            panic!("companion not listening");
        }
    };
    assert!(addr.is_ipv6(), "{addr}");
    let res = h
        .http
        .get(format!("http://{addr}/nope"))
        .header("Origin", PAGE_ORIGIN)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404, "expected HTTP on [::1]:{}", addr.port());
}

#[tokio::test]
async fn pairing_exchange_is_one_shot() {
    let h = Harness::boot().await;
    let created = h
        .post("/v1/pairing/requests", json!({ "application": APP }))
        .await;
    let body: Value = created.json().await.unwrap();
    let id = body["pairingRequestId"].as_str().unwrap();
    h.gateway().pairing_confirm(id).unwrap();
    let first = h
        .post("/v1/pairing/exchanges", json!({ "pairingRequestId": id }))
        .await;
    assert_eq!(first.status(), 200);
    let ticket: Value = first.json().await.unwrap();
    assert!(ticket["ticket"].as_str().unwrap().len() > 8);
    let second = h
        .post("/v1/pairing/exchanges", json!({ "pairingRequestId": id }))
        .await;
    assert_eq!(second.status(), 409);
    let err: Value = second.json().await.unwrap();
    assert_eq!(err["error"]["code"], "consumed");
}

fn tiny_png() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn image_inputs_put_then_turn_projects_attachments() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    let lease = init["result"]["connection"]["connectionLeaseId"]
        .as_str()
        .unwrap();
    let env = rpc
        .call("thread/environment/get", json!({ "threadId": "live" }))
        .await;
    assert_eq!(
        env["result"]["model"]["inputModalities"],
        json!(["text", "image"])
    );
    let sync = rpc
        .call("imageInputs/sync", json!({ "instanceId": "test" }))
        .await;
    assert_eq!(sync["result"]["connectionLeaseId"], lease);
    assert_eq!(sync["result"]["limitsVersion"], 3);

    let data = tiny_png();
    let digest = sha256_hex(&data);
    let put = rpc
        .call(
            "imageInputs/put",
            json!({
                "threadId": "live",
                "workspaceId": "default",
                "captureId": "cap-1",
                "source": "upload",
                "mimeType": "image/png",
                "width": 1,
                "height": 1,
                "byteLength": data.len(),
                "digest": digest,
                "dataBase64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &data
                )
            }),
        )
        .await;
    assert!(put.get("error").is_none(), "{put}");
    let id = put["result"]["imageInputId"].as_str().unwrap();
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": "live" }))
        .await;
    let started = rpc
        .call(
            "turn/start",
            json!({
                "message": "这是什么",
                "imageInputs": [{ "id": id, "digest": digest, "detail": "auto" }]
            }),
        )
        .await;
    assert!(started.get("error").is_none(), "{started}");
    let user = rpc
        .wait_notification("item/user_message", Duration::from_secs(5))
        .await;
    let content = user["params"]["content"].as_str().unwrap();
    assert!(content.contains("[Image #1]"), "{content}");
    assert!(content.contains("这是什么"), "{content}");
    assert_eq!(user["params"]["attachments"][0]["mimeType"], "image/png");
    assert_eq!(user["params"]["attachments"][0]["width"], 1);
    assert!(user["params"]["attachments"][0].get("data").is_none());
}

/// `thread/history` 里各条用户消息的（`turnId`, 正文）。
async fn user_turns(rpc: &mut Rpc) -> Vec<(String, String)> {
    let history = rpc
        .call("thread/history", json!({ "threadId": "live" }))
        .await;
    history["result"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["method"] == "item/user_message")
        .map(|e| {
            (
                e["turnId"].as_str().unwrap().to_string(),
                e["payload"]["content"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// `thread/rewind`：撤回任意一条用户消息，它和之后的对话全部删掉；正文和图片还回来，
/// 发送时补的 `[Image #1]` 从正文里去掉（图片单独还）。
#[tokio::test]
async fn thread_rewind_drops_that_message_and_everything_after() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(init["result"]["capabilities"]["threadRewind"], true);
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": "live" }))
        .await;
    let data = tiny_png();
    let digest = sha256_hex(&data);
    let put = rpc
        .call(
            "imageInputs/put",
            json!({
                "threadId": "live",
                "workspaceId": "default",
                "captureId": "cap-1",
                "source": "upload",
                "mimeType": "image/png",
                "width": 1,
                "height": 1,
                "byteLength": data.len(),
                "digest": digest,
                "dataBase64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &data
                )
            }),
        )
        .await;
    let id = put["result"]["imageInputId"].as_str().unwrap().to_string();
    for (i, message) in ["one", "two", "three"].into_iter().enumerate() {
        let mut params = json!({ "message": message });
        if i == 0 {
            params["imageInputs"] = json!([{ "id": id, "digest": digest }]);
        }
        let started = rpc.call("turn/start", params).await;
        assert!(started.get("error").is_none(), "{started}");
        rpc.wait_notification("turn/completed", Duration::from_secs(5))
            .await;
    }
    let turns = user_turns(&mut rpc).await;
    assert_eq!(turns.len(), 3, "{turns:?}");

    let rewound = rpc
        .call(
            "thread/rewind",
            json!({ "threadId": "live", "turnId": turns[1].0 }),
        )
        .await;
    assert!(rewound.get("error").is_none(), "{rewound}");
    assert_eq!(rewound["result"]["message"], "two");
    assert_eq!(rewound["result"]["images"], json!([]));
    let left = user_turns(&mut rpc).await;
    assert_eq!(left.len(), 1, "第 2 条和之后的都该删掉：{left:?}");
    assert!(left[0].1.contains("one"), "{left:?}");

    let rewound = rpc
        .call(
            "thread/rewind",
            json!({ "threadId": "live", "turnId": left[0].0 }),
        )
        .await;
    assert_eq!(rewound["result"]["message"], "one", "{rewound}");
    let image = &rewound["result"]["images"][0];
    assert_eq!(image["mimeType"], "image/png");
    assert_eq!(
        image["dataBase64"],
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data)
    );
    assert!(user_turns(&mut rpc).await.is_empty());

    let missing = rpc
        .call(
            "thread/rewind",
            json!({ "threadId": "live", "turnId": "t9" }),
        )
        .await;
    assert_eq!(
        missing["error"]["details"]["code"], "not_found",
        "{missing}"
    );
}

/// 空闲时手动压缩也会开一轮（没有用户消息）：`turnId` 不能按 `tN` 换算成第几条用户
/// 消息。压缩那一轮撤不了（`not_found`），它后面那条用户消息要撤对。
#[tokio::test]
async fn thread_rewind_maps_turns_past_an_idle_compaction() {
    let root = harness_root_with(Some(Arc::new(EchoLastUser))).await;
    root.plugin(compact(), ()).unwrap().wait().await.unwrap();
    let h = Harness::boot_on(root).await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": "live" }))
        .await;
    let _ = rpc.call("turn/start", json!({ "message": "first" })).await;
    rpc.wait_notification("turn/completed", Duration::from_secs(5))
        .await;
    let _ = rpc
        .call("thread/context/compact", json!({ "threadId": "live" }))
        .await;
    rpc.wait_notification("item/compaction", Duration::from_secs(5))
        .await;
    let _ = rpc.call("turn/start", json!({ "message": "second" })).await;
    rpc.wait_notification("turn/completed", Duration::from_secs(5))
        .await;

    let history = rpc
        .call("thread/history", json!({ "threadId": "live" }))
        .await;
    let compaction_turn = history["result"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["method"] == "item/compaction")
        .and_then(|e| e["turnId"].as_str())
        .unwrap()
        .to_string();
    let turns = user_turns(&mut rpc).await;
    assert_eq!(turns.len(), 2, "{turns:?}");
    assert_ne!(
        turns[1].0, "t2",
        "压缩占了一轮，第 2 条用户消息不在 t2：{turns:?}"
    );

    let missing = rpc
        .call(
            "thread/rewind",
            json!({ "threadId": "live", "turnId": compaction_turn }),
        )
        .await;
    assert_eq!(
        missing["error"]["details"]["code"], "not_found",
        "{missing}"
    );

    let rewound = rpc
        .call(
            "thread/rewind",
            json!({ "threadId": "live", "turnId": turns[1].0 }),
        )
        .await;
    assert_eq!(rewound["result"]["message"], "second", "{rewound}");
    let left = user_turns(&mut rpc).await;
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(left[0].1.contains("first"), "{left:?}");
}

/// 一直不回的模型：一个字节都不出，直到这一轮被取消。和 HTTP 采样器一样认
/// `TurnControl`（真采样器在每个 await 上都和取消赛跑）。
struct Hang(Arc<std::sync::OnceLock<Context>>);

impl Sampler for Hang {
    fn sample<'a>(
        &'a self,
        _request: PromptRequest,
        _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let ctx = self.0.clone();
        Box::pin(async move {
            loop {
                let cancelled = ctx
                    .get()
                    .and_then(|c| c.get::<TurnControl>(TURN))
                    .is_some_and(|t| t.is_cancelled());
                if cancelled {
                    return LlmOutput::default();
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    }
}

/// 正在跑的那一条也能撤：先停下这一轮，等它停住再删。
#[tokio::test]
async fn thread_rewind_stops_a_running_turn_first() {
    let slot = Arc::new(std::sync::OnceLock::new());
    let root = harness_root_with(Some(Arc::new(Hang(slot.clone())))).await;
    slot.set(root.clone()).unwrap();
    let h = Harness::boot_on(root).await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": "live" }))
        .await;
    let _ = rpc.call("turn/start", json!({ "message": "stuck" })).await;
    rpc.wait_notification("item/user_message", Duration::from_secs(5))
        .await;
    let turns = user_turns(&mut rpc).await;
    let rewound = rpc
        .call(
            "thread/rewind",
            json!({ "threadId": "live", "turnId": turns[0].0 }),
        )
        .await;
    assert_eq!(rewound["result"]["message"], "stuck", "{rewound}");
    assert!(user_turns(&mut rpc).await.is_empty());
    let sessions = h.ctx.require::<Sessions>(SESSIONS).unwrap();
    assert!(
        !sessions
            .events()
            .iter()
            .any(|e| matches!(e, LogEvent::User(_))),
        "{:?}",
        sessions.events()
    );
}

#[tokio::test]
async fn gateway_idle_until_start_listen() {
    let root = harness_root().await;
    root.plugin(gateway_idle("127.0.0.1:0"), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let gw = root.require::<cordis_tui::GatewayRef>(GATEWAY).unwrap();
    assert!(!gw.is_listening(), "plugin should not bind until /pair");
    let preferred = gw.local_addr();
    assert_eq!(preferred.port(), 0);

    let addr = gw.start_listen().expect("start_listen");
    assert!(gw.is_listening());
    assert_ne!(addr.port(), 0);

    let http = reqwest::Client::new();
    let mut ready = false;
    for _ in 0..80 {
        if http
            .get(format!("http://{addr}/nope"))
            .header("Origin", PAGE_ORIGIN)
            .send()
            .await
            .is_ok()
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(ready, "HTTP did not start on {addr}");

    gw.stop_listen().expect("stop_listen");
    assert!(!gw.is_listening());
    let mut down = false;
    for _ in 0..80 {
        match http
            .get(format!("http://{addr}/nope"))
            .header("Origin", PAGE_ORIGIN)
            .send()
            .await
        {
            Err(_) => {
                down = true;
                break;
            }
            Ok(_) => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    }
    assert!(down, "HTTP still serving {addr} after stop_listen");
}

/// 回归：`live` 线程只投影第 1 页。TUI 开着多页时，第 2 页说的话以前也会混进来
/// （`session/event` 不分 realm、不带页身份）；主会话自己的话照常进来。
#[tokio::test]
async fn live_thread_ignores_other_pages() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;

    let page = h.ctx.isolate("sessions");
    let _reg = page
        .provide(SESSIONS, Sessions::tab(page.clone(), 2))
        .unwrap();
    page.get::<Sessions>(SESSIONS)
        .unwrap()
        .append(LogEvent::User("第二页的话".into()));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let history = rpc
        .call("thread/history", json!({ "threadId": "live" }))
        .await;
    let text = history.to_string();
    assert!(
        !text.contains("第二页的话"),
        "第 2 页的话混进了 live：{text}"
    );

    h.ctx
        .get::<Sessions>(SESSIONS)
        .unwrap()
        .append(LogEvent::User("主会话的话".into()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let history = rpc
        .call("thread/history", json!({ "threadId": "live" }))
        .await;
    assert!(history.to_string().contains("主会话的话"), "{history}");
}

const GUI_ORIGIN: &str = "tauri://localhost";

/// `dock serve`：父进程从 stdout 拿到的 ticket 直接能连 WS，不走配对；ticket
/// 钉在 GUI 的 Origin 上，换个 Origin 用不了；网关不因此留下绑定，走 HTTP 领
/// ticket 仍然要配对；stdin 关闭时控制循环结束。
#[tokio::test]
async fn serve_hands_the_parent_a_working_ticket() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let root = harness_root().await;
    root.plugin(
        gateway_serve("127.0.0.1:0"),
        ServeConfig {
            application: "dock-gui".into(),
            origin: GUI_ORIGIN.into(),
        },
    )
    .unwrap()
    .wait()
    .await
    .unwrap();
    let control = (*root.require::<ServeControl>(GATEWAY_SERVE).unwrap()).clone();
    let addr = control.addr();
    assert!(addr.ip().is_loopback());

    let (mut to_dock, dock_in) = tokio::io::duplex(4096);
    let (dock_out, from_dock) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move { control.run(BufReader::new(dock_in), dock_out).await });
    let mut lines = BufReader::new(from_dock).lines();
    async fn next<R: tokio::io::AsyncBufRead + Unpin>(lines: &mut tokio::io::Lines<R>) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("stdout timeout")
            .unwrap()
            .expect("stdout closed");
        serde_json::from_str::<Value>(&line).unwrap()
    }

    let ready = next(&mut lines).await;
    assert_eq!(ready["event"], "ready");
    assert_eq!(ready["ws"], format!("ws://{addr}/api/ws"));
    let ticket = ready["ticket"].as_str().unwrap().to_string();

    let (_, wrong) = Rpc::connect_as(addr, &ticket, PAGE_ORIGIN).await;
    assert!(
        wrong.get("error").is_some(),
        "换个 Origin 不该认这张 ticket：{wrong}"
    );

    let (mut rpc, auth) = Rpc::connect_as(addr, &ticket, GUI_ORIGIN).await;
    assert_eq!(auth["result"]["ok"], true, "{auth}");
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);

    let via_http = reqwest::Client::new()
        .post(format!("http://{addr}/v1/connection/tickets"))
        .header("Origin", GUI_ORIGIN)
        .json(&json!({ "application": "dock-gui" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        via_http.status(),
        403,
        "受信签发不能顺带开放 HTTP 领 ticket"
    );

    to_dock.write_all(b"{\"cmd\":\"ticket\"}\n").await.unwrap();
    let renewed = next(&mut lines).await;
    assert_eq!(renewed["event"], "ticket");
    let (_, auth) = Rpc::connect_as(addr, renewed["ticket"].as_str().unwrap(), GUI_ORIGIN).await;
    assert_eq!(auth["result"]["ok"], true, "{auth}");

    drop(to_dock);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("stdin 关闭后控制循环该结束")
        .unwrap()
        .unwrap();
}

/// `thread/search`：落盘会话按用户消息搜得到（中英文都行），带命中片段。
#[tokio::test]
async fn thread_search_finds_saved_sessions_by_their_messages() {
    let h = Harness::boot_with_pages().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;

    let dir = project_dir("search-me");
    let started = rpc
        .call("thread/start", json!({ "cwd": dir.display().to_string() }))
        .await;
    let id = started["result"]["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": id }))
        .await;
    let _ = rpc
        .call(
            "turn/start",
            json!({ "threadId": id, "message": "把 zebraquokka 模块的登录按钮改成圆角" }),
        )
        .await;
    rpc.wait_notification("turn/completed", Duration::from_secs(5))
        .await;
    let _ = rpc.call("thread/close", json!({ "threadId": id })).await;

    for query in ["zebraquokka", "圆角"] {
        let found = rpc.call("thread/search", json!({ "query": query })).await;
        let hits = found["result"]["hits"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let hit = hits
            .iter()
            .find(|h| h["threadId"] == json!(id))
            .unwrap_or_else(|| panic!("{query} 没搜到：{found}"));
        assert!(
            hit["snippet"].as_str().unwrap_or("").contains(query),
            "{found}"
        );
    }
    let none = rpc
        .call("thread/search", json!({ "query": "不存在的词xyz" }))
        .await;
    assert_eq!(none["result"]["hits"], json!([]), "{none}");
    let bad = rpc.call("thread/search", json!({})).await;
    assert_eq!(bad["error"]["code"], -32602, "{bad}");
}

/// GUI 的自定义预设：新建（带图标）→ 列表里有 → 能用它开会话 → 删掉。
#[tokio::test]
async fn custom_presets_can_be_created_used_and_deleted() {
    let h = Harness::boot_with_pages().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let before = rpc.call("preset/list", json!({})).await;
    let default_id = before["result"]["defaultId"].clone();

    let made = rpc
        .call(
            "preset/create",
            json!({ "name": "我的助手", "icon": "rocket", "basedOn": "minimal" }),
        )
        .await;
    let id = made["result"]["preset"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{made}"))
        .to_string();
    assert_eq!(made["result"]["preset"]["icon"], "rocket", "{made}");

    let listed = rpc.call("preset/list", json!({})).await;
    let item = listed["result"]["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("新预设不在列表里：{listed}"));
    assert_eq!(item["label"], "我的助手");
    assert_eq!(item["origin"], "user");
    assert_eq!(item["builtin"], false);
    let code = listed["result"]["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "code")
        .cloned()
        .unwrap();
    assert_eq!(code["builtin"], true, "{code}");
    assert_eq!(listed["result"]["defaultId"], default_id, "新建不改默认");

    let dir = project_dir("custom-preset");
    let started = rpc
        .call(
            "thread/start",
            json!({ "cwd": dir.display().to_string(), "presetId": id }),
        )
        .await;
    assert!(started.get("error").is_none(), "{started}");

    let bad = rpc.call("preset/create", json!({ "name": "  " })).await;
    assert_eq!(bad["error"]["details"]["code"], "invalid_params", "{bad}");
    let shipped = rpc.call("preset/delete", json!({ "id": "code" })).await;
    assert!(shipped.get("error").is_some(), "内置的删不掉：{shipped}");

    let deleted = rpc.call("preset/delete", json!({ "id": id })).await;
    assert_eq!(deleted["result"]["ok"], true, "{deleted}");
    let listed = rpc.call("preset/list", json!({})).await;
    assert!(
        !listed.to_string().contains("我的助手"),
        "删了还在：{listed}"
    );
}

/// 预设编辑器：`tool/catalog` 列可选工具（不含 MCP），`preset/get` 给整份定义，
/// `preset/update` 整份写回（名册增删、工具名单、人设），校验错误回 `invalid_params`；
/// 内置预设改完成覆盖层（`origin: user`、带文件路径），内置子代理删不掉。
#[tokio::test]
async fn preset_editor_reads_and_writes_the_whole_definition() {
    let h = Harness::boot_with_pages().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;

    let catalog = rpc.call("tool/catalog", json!({})).await;
    let tools = catalog["result"]["tools"].as_array().unwrap().clone();
    assert!(!tools.is_empty(), "{catalog}");
    let first = tools[0]["name"].as_str().unwrap().to_string();
    assert!(
        tools
            .iter()
            .all(|t| t["summary"].is_string() && t["kind"].is_string()),
        "{catalog}"
    );

    let made = rpc
        .call(
            "preset/create",
            json!({ "name": "研究员", "basedOn": "minimal" }),
        )
        .await;
    let id = made["result"]["preset"]["id"].as_str().unwrap().to_string();

    let body = json!({
        "name": "深度研究员",
        "description": "跨文档综合",
        "icon": "search",
        "order": 10,
        "persona": "你是研究员。",
        "replacePrompt": false,
        "tools": [first],
        "agents": [
            { "id": "web", "name": "网页", "description": "查网页", "persona": "只查网页",
              "tools": [first], "replacePrompt": false, "listings": true }
        ]
    });
    let saved = rpc
        .call("preset/update", json!({ "id": id, "preset": body }))
        .await;
    assert_eq!(saved["result"]["preset"]["label"], "深度研究员", "{saved}");

    let got = rpc.call("preset/get", json!({ "id": id })).await;
    let p = &got["result"]["preset"];
    assert_eq!(p["name"], "深度研究员", "{got}");
    assert_eq!(p["icon"], "search");
    assert_eq!(p["order"], 10);
    assert_eq!(p["persona"], "你是研究员。");
    assert_eq!(p["tools"], json!([first]));
    assert_eq!(p["agents"][0]["id"], "web");
    assert_eq!(p["agents"][0]["listings"], true);
    assert_eq!(p["agents"][0]["builtin"], false);
    assert!(p["path"].as_str().unwrap().ends_with("agent.yml"), "{got}");

    // 整份替换却没有提示词：拒，不写盘。
    let mut bad = body.clone();
    bad["replacePrompt"] = json!(true);
    bad["persona"] = json!(" ");
    let refused = rpc
        .call("preset/update", json!({ "id": id, "preset": bad }))
        .await;
    assert_eq!(
        refused["error"]["details"]["code"], "invalid_params",
        "{refused}"
    );
    let still = rpc.call("preset/get", json!({ "id": id })).await;
    assert_eq!(still["result"]["preset"]["persona"], "你是研究员。");

    // 内置预设：带内置子代理；删内置子代理被拒；原样保存变成用户层覆盖。
    let code = rpc.call("preset/get", json!({ "id": "code" })).await;
    let code = code["result"]["preset"].clone();
    assert_eq!(code["origin"], "shipped", "{code}");
    assert!(code["path"].is_null(), "没改过没有文件：{code}");
    let agents = code["agents"].as_array().unwrap().clone();
    assert!(agents.iter().all(|a| a["builtin"] == true), "{code}");
    let mut dropped = code.clone();
    dropped["agents"] = json!(agents[1..].to_vec());
    let refused = rpc
        .call("preset/update", json!({ "id": "code", "preset": dropped }))
        .await;
    assert!(refused.get("error").is_some(), "{refused}");
    let saved = rpc
        .call("preset/update", json!({ "id": "code", "preset": code }))
        .await;
    assert_eq!(saved["result"]["preset"]["origin"], "user", "{saved}");
    let restored = rpc.call("preset/delete", json!({ "id": "code" })).await;
    assert_eq!(restored["result"]["ok"], true, "恢复内置：{restored}");

    let missing = rpc.call("preset/get", json!({ "id": "nope" })).await;
    assert_eq!(missing["error"]["details"]["code"], "not_found");
    let _ = rpc.call("preset/delete", json!({ "id": id })).await;
}

/// 常驻工具：`residentTools` 写得进、读得回（预设和子代理各一份）；不带这个字段整份写回
/// 时保持原样（老客户端不清掉它）；写法不对回 `invalid_params`。`tool/catalog
/// {includeMcp:true}` 才带 MCP 行。
#[tokio::test]
async fn preset_resident_tools_round_trip_and_survive_old_clients() {
    let h = Harness::boot_with_pages().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;

    let plain = rpc.call("tool/catalog", json!({})).await;
    let with_mcp = rpc
        .call("tool/catalog", json!({ "includeMcp": true }))
        .await;
    let rows = |v: &serde_json::Value| v["result"]["tools"].as_array().unwrap().len();
    assert!(rows(&with_mcp) >= rows(&plain), "{with_mcp}");
    assert!(
        plain["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["kind"] != "mcp"),
        "{plain}"
    );

    let made = rpc
        .call(
            "preset/create",
            json!({ "name": "通用", "basedOn": "minimal" }),
        )
        .await;
    let id = made["result"]["preset"]["id"].as_str().unwrap().to_string();
    let body = json!({
        "name": "通用",
        "tools": ["read_file"],
        "residentTools": ["mcp_browser__*", "mcp_browser__*"],
        "agents": [
            { "id": "web", "name": "网页", "tools": ["read_file"],
              "residentTools": ["mcp_browser__browser_open"] }
        ]
    });
    let saved = rpc
        .call("preset/update", json!({ "id": id, "preset": body }))
        .await;
    assert!(saved.get("error").is_none(), "{saved}");
    let got = rpc.call("preset/get", json!({ "id": id })).await;
    let p = &got["result"]["preset"];
    assert_eq!(p["residentTools"], json!(["mcp_browser__*"]), "{got}");
    assert_eq!(
        p["agents"][0]["residentTools"],
        json!(["mcp_browser__browser_open"])
    );

    // 老客户端：拿 preset/get 的结果去掉 residentTools 整份写回。
    let mut old = p.clone();
    old.as_object_mut().unwrap().remove("residentTools");
    for a in old["agents"].as_array_mut().unwrap() {
        a.as_object_mut().unwrap().remove("residentTools");
    }
    let saved = rpc
        .call("preset/update", json!({ "id": id, "preset": old }))
        .await;
    assert!(saved.get("error").is_none(), "{saved}");
    let kept = rpc.call("preset/get", json!({ "id": id })).await;
    assert_eq!(
        kept["result"]["preset"]["residentTools"],
        json!(["mcp_browser__*"])
    );
    assert_eq!(
        kept["result"]["preset"]["agents"][0]["residentTools"],
        json!(["mcp_browser__browser_open"])
    );

    let mut bad = body.clone();
    bad["residentTools"] = json!(["*"]);
    let refused = rpc
        .call("preset/update", json!({ "id": id, "preset": bad }))
        .await;
    assert_eq!(
        refused["error"]["details"]["code"], "invalid_params",
        "{refused}"
    );
    let _ = rpc.call("preset/delete", json!({ "id": id })).await;
}

/// AI 辅助写预设：走 `"llm"`，结果只回不写盘。测试装配的 echo 只回工具调用、没有正文，
/// 三个方法都报 `draft_failed`（成功路径见 spine `tests/preset_assist.rs`）；参数错是
/// `invalid_params`；另起任务回帧之后连接照常可用。
#[tokio::test]
async fn preset_assist_calls_the_model_and_never_writes() {
    let h = Harness::boot_with_pages().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let before = rpc.call("preset/list", json!({})).await;

    let drafted = rpc
        .call(
            "preset/draft",
            json!({ "description": "帮我审代码，只读", "icons": ["shield"] }),
        )
        .await;
    assert_eq!(
        drafted["error"]["details"]["code"], "draft_failed",
        "{drafted}"
    );
    assert!(
        drafted["error"]["message"]
            .as_str()
            .unwrap()
            .contains("JSON"),
        "{drafted}"
    );

    let empty = rpc
        .call("preset/draft", json!({ "description": "  " }))
        .await;
    assert_eq!(empty["error"]["details"]["code"], "draft_failed", "{empty}");

    let polished = rpc
        .call(
            "preset/rewrite",
            json!({ "persona": "你审代码", "mode": "polish" }),
        )
        .await;
    assert_eq!(
        polished["error"]["details"]["code"], "draft_failed",
        "{polished}"
    );

    let bad_mode = rpc
        .call("preset/rewrite", json!({ "persona": "x", "mode": "shout" }))
        .await;
    assert_eq!(
        bad_mode["error"]["details"]["code"], "invalid_params",
        "{bad_mode}"
    );

    let tools = rpc
        .call("preset/suggestTools", json!({ "description": "搜代码" }))
        .await;
    assert_eq!(tools["error"]["details"]["code"], "draft_failed", "{tools}");

    // 另起任务回帧之后，这条连接照常能用。
    let after = rpc.call("preset/list", json!({})).await;
    assert_eq!(after["result"], before["result"], "起草不写盘");
}

/// 关着的会话落在别的目录（GUI 每个项目一个 cwd）：改名、删除要按名册找，
/// 不能只看第 1 页那个目录的历史。
#[tokio::test]
async fn closed_threads_in_other_dirs_can_be_renamed_and_deleted() {
    let h = Harness::boot_with_pages().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;

    let dir = project_dir("edit-elsewhere");
    let started = rpc
        .call("thread/start", json!({ "cwd": dir.display().to_string() }))
        .await;
    let id = started["result"]["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": id }))
        .await;
    let _ = rpc
        .call(
            "turn/start",
            json!({ "threadId": id, "message": "别处的会话" }),
        )
        .await;
    rpc.wait_notification("turn/completed", Duration::from_secs(5))
        .await;
    let closed = rpc.call("thread/close", json!({ "threadId": id })).await;
    assert_eq!(closed["result"]["ok"], true, "{closed}");

    let listed_title = |listed: &Value| -> Option<String> {
        listed["result"]["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == json!(id))
            .map(|t| t["title"].as_str().unwrap_or("").to_string())
    };

    let renamed = rpc
        .call(
            "thread/rename",
            json!({ "threadId": id, "title": "改过的名字" }),
        )
        .await;
    assert!(renamed.get("error").is_none(), "{renamed}");
    assert_eq!(renamed["result"]["thread"]["title"], "改过的名字");
    let listed = rpc.call("thread/list", json!({ "scope": "all" })).await;
    assert_eq!(
        listed_title(&listed).as_deref(),
        Some("改过的名字"),
        "{listed}"
    );

    let deleted = rpc.call("thread/delete", json!({ "threadId": id })).await;
    assert_eq!(deleted["result"]["ok"], true, "{deleted}");
    let listed = rpc.call("thread/list", json!({ "scope": "all" })).await;
    assert_eq!(listed_title(&listed), None, "删了还在：{listed}");

    let again = rpc.call("thread/delete", json!({ "threadId": id })).await;
    assert_eq!(again["error"]["details"]["code"], "not_found", "{again}");
}

/// 关着的会话不用先 `thread/open`：发消息、改设置、订阅都按需开页；停止和队列回
/// 空结果、不开页；应答请求仍回 `thread_not_open`；认不得的 id 是 `not_found`。
#[tokio::test]
async fn closed_threads_open_on_demand() {
    let h = Harness::boot_with_pages().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;

    let dir = project_dir("on-demand");
    let started = rpc
        .call("thread/start", json!({ "cwd": dir.display().to_string() }))
        .await;
    let id = started["result"]["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": id }))
        .await;
    let _ = rpc
        .call("turn/start", json!({ "threadId": id, "message": "第一句" }))
        .await;
    rpc.wait_notification("turn/completed", Duration::from_secs(5))
        .await;
    let closed = rpc.call("thread/close", json!({ "threadId": id })).await;
    assert_eq!(closed["result"]["ok"], true, "{closed}");

    let is_open = |listed: &Value| -> bool {
        listed["result"]["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == json!(id) && t["open"] == true)
    };

    // 停止、队列：关着的页没有这些，回空结果，也不开页。
    let cancel = rpc.call("turn/cancel", json!({ "threadId": id })).await;
    assert_eq!(cancel["result"]["cancelled"], false, "{cancel}");
    let queue = rpc.call("turn/queue/list", json!({ "threadId": id })).await;
    assert_eq!(queue["result"]["items"], json!([]), "{queue}");
    let removed = rpc
        .call(
            "turn/queue/remove",
            json!({ "threadId": id, "queueId": "1" }),
        )
        .await;
    assert_eq!(removed["result"]["removed"], false, "{removed}");
    let answer = rpc
        .call(
            "interaction/respond",
            json!({ "threadId": id, "answers": [] }),
        )
        .await;
    assert_eq!(
        answer["error"]["details"]["code"], "thread_not_open",
        "{answer}"
    );
    let listed = rpc.call("thread/list", json!({ "scope": "all" })).await;
    assert!(!is_open(&listed), "这些不该开页：{listed}");

    // 改设置：先开页再改，改的是这一页。
    let set = rpc
        .call(
            "thread/model/set",
            json!({ "threadId": id, "modelId": "on-demand" }),
        )
        .await;
    assert!(set.get("error").is_none(), "{set}");
    let listed = rpc.call("thread/list", json!({ "scope": "all" })).await;
    assert!(is_open(&listed), "改设置该开页：{listed}");
    let env = rpc
        .call("thread/environment/get", json!({ "threadId": id }))
        .await;
    assert_eq!(env["result"]["model"]["id"], "on-demand", "{env}");

    // 关掉后直接订阅 + 发消息：接着原来的对话跑，推送照常到。
    let _ = rpc.call("thread/close", json!({ "threadId": id })).await;
    let sub = rpc
        .call("thread/subscribe", json!({ "threadId": id }))
        .await;
    assert_eq!(sub["result"]["ok"], true, "{sub}");
    let turn = rpc
        .call("turn/start", json!({ "threadId": id, "message": "第二句" }))
        .await;
    assert_eq!(turn["result"]["threadId"], json!(id), "{turn}");
    let done = rpc
        .wait_notification("turn/completed", Duration::from_secs(5))
        .await;
    assert_eq!(done["params"]["threadId"], json!(id), "{done}");
    let history = rpc.call("thread/history", json!({ "threadId": id })).await;
    let text = history.to_string();
    assert!(
        text.contains("第一句") && text.contains("第二句"),
        "{history}"
    );
    let listed = rpc.call("thread/list", json!({ "scope": "all" })).await;
    let pages = listed["result"]["threads"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["id"] == json!(id))
        .count();
    assert_eq!(pages, 1, "同一个会话只开一页：{listed}");

    let unknown = rpc
        .call(
            "turn/start",
            json!({ "threadId": "no-such-thread", "message": "x" }),
        )
        .await;
    assert_eq!(
        unknown["error"]["details"]["code"], "not_found",
        "{unknown}"
    );
}

/// 多线程：在一个目录开一页 → 在那一页跑一轮（只推给它的订阅，不进 `live`）→
/// 设置按页 → 关页后不能再发 → 从磁盘重新打开，对话还在。
#[tokio::test]
async fn threads_open_run_close_and_reopen_per_page() {
    let h = Harness::boot_with_pages().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(init["result"]["capabilities"]["openThreads"], true);

    let dir = project_dir("a");
    let started = rpc
        .call("thread/start", json!({ "cwd": dir.display().to_string() }))
        .await;
    let id = started["result"]["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!id.is_empty() && id != "live", "{started}");
    assert_eq!(
        started["result"]["thread"]["cwd"],
        dir.display().to_string()
    );

    let listed = rpc.call("thread/list", json!({ "scope": "all" })).await;
    let threads = listed["result"]["threads"].as_array().unwrap().clone();
    assert!(
        threads
            .iter()
            .any(|t| t["alias"] == "live" && t["open"] == true),
        "{listed}"
    );
    assert!(
        threads
            .iter()
            .any(|t| t["id"] == json!(id) && t["open"] == true),
        "{listed}"
    );

    let sub = rpc
        .call("thread/subscribe", json!({ "threadId": id }))
        .await;
    assert_eq!(sub["result"]["ok"], true, "{sub}");
    let turn = rpc
        .call(
            "turn/start",
            json!({ "threadId": id, "message": "A 页的话" }),
        )
        .await;
    assert_eq!(turn["result"]["threadId"], json!(id), "{turn}");
    let said = rpc
        .wait_notification("item/user_message", Duration::from_secs(5))
        .await;
    assert_eq!(said["params"]["threadId"], json!(id), "{said}");
    // 一轮跑完（`LoopHandle` 发 `session/turn-end`）才有 turn/completed，且只一次。
    let done = rpc
        .wait_notification("turn/completed", Duration::from_secs(5))
        .await;
    assert_eq!(done["params"]["threadId"], json!(id), "{done}");
    assert_eq!(done["params"]["turnId"], said["params"]["turnId"], "{done}");

    let live = rpc
        .call("thread/history", json!({ "threadId": "live" }))
        .await;
    assert!(
        !live.to_string().contains("A 页的话"),
        "不该进 live：{live}"
    );

    rpc.call(
        "thread/model/set",
        json!({ "threadId": id, "modelId": "only-a" }),
    )
    .await;
    let env_a = rpc
        .call("thread/environment/get", json!({ "threadId": id }))
        .await;
    let env_live = rpc
        .call("thread/environment/get", json!({ "threadId": "live" }))
        .await;
    assert_eq!(env_a["result"]["model"]["id"], "only-a");
    assert_ne!(env_live["result"]["model"]["id"], "only-a", "设置按页");

    let closed = rpc.call("thread/close", json!({ "threadId": id })).await;
    assert_eq!(closed["result"]["ok"], true, "{closed}");
    // 发消息会按需开页（见 `closed_threads_open_on_demand`）；应答请求不会。
    let refused = rpc
        .call(
            "permission/resolve",
            json!({ "threadId": id, "decision": "approve" }),
        )
        .await;
    assert_eq!(refused["error"]["details"]["code"], "thread_not_open");
    let live_close = rpc
        .call("thread/close", json!({ "threadId": "live" }))
        .await;
    assert!(
        live_close.get("error").is_some(),
        "第 1 页关不掉：{live_close}"
    );

    // 关着的会话也回放成事件（以前只有纯文本 messages），每轮都收了尾。
    let methods = |h: &Value| -> Vec<String> {
        h["result"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["method"].as_str().unwrap_or("").to_string())
            .collect()
    };
    let closed_history = rpc.call("thread/history", json!({ "threadId": id })).await;
    let closed_methods = methods(&closed_history);
    assert!(
        closed_methods.iter().any(|m| m == "item/user_message"),
        "{closed_history}"
    );
    assert_eq!(
        closed_methods.last().map(String::as_str),
        Some("turn/completed"),
        "{closed_methods:?}"
    );

    let reopened = rpc.call("thread/open", json!({ "threadId": id })).await;
    assert_eq!(reopened["result"]["thread"]["id"], json!(id), "{reopened}");
    let history = rpc.call("thread/history", json!({ "threadId": id })).await;
    assert!(history.to_string().contains("A 页的话"), "{history}");
    // 重开时按落盘重建投影：最后一轮不能挂着（以前只有 turn/started）。
    assert_eq!(
        methods(&history).last().map(String::as_str),
        Some("turn/completed"),
        "{history}"
    );

    let workspaces = rpc.call("workspace/list", json!({ "scope": "all" })).await;
    assert!(
        workspaces.to_string().contains(&dir.display().to_string()),
        "{workspaces}"
    );
}

/// `thread/start { presetId }` 只在新页切预设（第 1 页不动），列表里带出来；关页
/// 后从磁盘名册读回来的也是它。不存在的预设直接拒，不留空页。
#[tokio::test]
async fn thread_start_with_preset_is_per_page_and_listed() {
    let h = Harness::boot_with_pages().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    rpc.call("initialize", json!({})).await;
    let root_preset = h
        .ctx
        .require::<AgentPresets>(AGENT_PRESETS)
        .unwrap()
        .current_id();
    let other = if root_preset == "warden" {
        "minimal"
    } else {
        "warden"
    };

    let dir = project_dir("preset");
    let started = rpc
        .call(
            "thread/start",
            json!({ "cwd": dir.display().to_string(), "presetId": other }),
        )
        .await;
    let thread = &started["result"]["thread"];
    assert_eq!(thread["presetId"], other, "{started}");
    let id = thread["id"].as_str().unwrap().to_string();

    let listed = rpc.call("thread/list", json!({ "scope": "all" })).await;
    let threads = listed["result"]["threads"].as_array().unwrap().clone();
    let find = |key: &str, val: &str| {
        threads
            .iter()
            .find(|t| t[key] == val)
            .unwrap_or_else(|| panic!("{listed}"))
            .clone()
    };
    assert_eq!(find("id", &id)["presetId"], other);
    assert_eq!(
        find("alias", "live")["presetId"],
        json!(root_preset),
        "第 1 页不动"
    );

    // 跑一轮让它落盘，关掉后名册里读回预设。
    rpc.call("thread/subscribe", json!({ "threadId": id }))
        .await;
    rpc.call("turn/start", json!({ "threadId": id, "message": "预设页" }))
        .await;
    rpc.wait_notification("item/user_message", Duration::from_secs(5))
        .await;
    rpc.call("thread/close", json!({ "threadId": id })).await;
    let listed = rpc.call("thread/list", json!({ "scope": "all" })).await;
    let closed = listed["result"]["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("{listed}"));
    assert_eq!(closed["open"], false, "{closed}");
    assert_eq!(closed["presetId"], other, "{closed}");

    // 预设跟着会话走：重新打开，这一页用的还是创建时选的，不是开它的那一页的。
    let reopened = rpc.call("thread/open", json!({ "threadId": id })).await;
    assert_eq!(
        reopened["result"]["thread"]["presetId"], other,
        "{reopened}"
    );
    rpc.call("thread/close", json!({ "threadId": id })).await;

    // 给新会话选预设不改全局默认：下次启动 / 新开的页仍是原来的。
    let roster_yml = std::fs::read_to_string(
        std::path::PathBuf::from(std::env::var("DOCK_HOME").unwrap())
            .join("presets")
            .join("roster.yml"),
    )
    .unwrap_or_default();
    assert!(!roster_yml.contains(other), "默认预设被改了：{roster_yml}");

    let before = rpc.call("thread/list", json!({ "scope": "all" })).await;
    let bad = rpc
        .call(
            "thread/start",
            json!({ "cwd": dir.display().to_string(), "presetId": "no-such-preset" }),
        )
        .await;
    assert!(bad.get("error").is_some(), "{bad}");
    let after = rpc.call("thread/list", json!({ "scope": "all" })).await;
    let open = |v: &Value| {
        v["result"]["threads"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["open"] == true)
            .count()
    };
    assert_eq!(open(&before), open(&after), "不该留下空页");
}

/// `preset/list`：新对话页选预设。内置四个都在、顺序和 TUI 一样从「编码」开始，
/// `defaultId` 是不带 `presetId` 开新会话时用的那个；列出来的 id 能直接拿去开会话。
#[tokio::test]
async fn preset_list_feeds_thread_start() {
    let h = Harness::boot_with_pages().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(init["result"]["capabilities"]["presets"], true, "{init}");

    let listed = rpc.call("preset/list", json!({})).await;
    let presets = listed["result"]["presets"].as_array().unwrap().clone();
    let ids: Vec<&str> = presets.iter().filter_map(|p| p["id"].as_str()).collect();
    for id in ["code", "minimal", "cordis", "warden"] {
        assert!(ids.contains(&id), "{listed}");
    }
    assert_eq!(ids[0], "code", "{listed}");
    let warden = presets.iter().find(|p| p["id"] == "warden").unwrap();
    assert_eq!(warden["origin"], "shipped", "{warden}");
    assert_eq!(warden["available"], true, "{warden}");
    assert!(!warden["label"].as_str().unwrap().is_empty(), "{warden}");
    let default = listed["result"]["defaultId"].as_str().unwrap();
    assert!(ids.contains(&default), "{listed}");

    let dir = project_dir("preset-list");
    let started = rpc
        .call(
            "thread/start",
            json!({ "cwd": dir.display().to_string(), "presetId": ids[1] }),
        )
        .await;
    assert_eq!(started["result"]["thread"]["presetId"], ids[1], "{started}");
}

/// `browser/view/*` 的错误路径（不需要 Chrome）：会话还没开标签页回 `no_tab`；
/// 视图 id 不对回 `not_found`；缺 viewId 回 `invalid_params`；没鉴权的连接用不了。
#[tokio::test]
async fn browser_view_errors_without_a_tab() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(
        init["result"]["capabilities"]["browserView"], true,
        "{init}"
    );

    let opened = rpc.call("browser/view/open", json!({})).await;
    assert_eq!(opened["error"]["details"]["code"], "no_tab", "{opened}");
    let input = rpc
        .call(
            "browser/view/input",
            json!({ "viewId": "nope", "event": { "type": "text", "text": "x" } }),
        )
        .await;
    assert_eq!(input["error"]["details"]["code"], "not_found", "{input}");
    let missing = rpc.call("browser/view/close", json!({})).await;
    assert_eq!(
        missing["error"]["details"]["code"], "invalid_params",
        "{missing}"
    );
    assert_eq!(
        init["result"]["capabilities"]["browserTabs"], true,
        "{init}"
    );
    let tab = rpc
        .call(
            "browser/view/tab",
            json!({ "viewId": "nope", "action": "switch", "targetId": "T" }),
        )
        .await;
    assert_eq!(tab["error"]["details"]["code"], "not_found", "{tab}");
    let tab = rpc
        .call("browser/view/tab", json!({ "action": "new" }))
        .await;
    assert_eq!(tab["error"]["details"]["code"], "invalid_params", "{tab}");

    let (mut stranger, _) = Rpc::connect_as(h.addr, "bogus", PAGE_ORIGIN).await;
    let refused = stranger.call("browser/view/open", json!({})).await;
    assert_eq!(
        refused["error"]["details"]["code"], "unauthenticated",
        "{refused}"
    );
}

/// 面板跟随与替会话开页（不需要 Chrome）：能力位在；`url` 开页要经浏览器 MCP，没连上回
/// `browser_unavailable`（不是 `no_tab`）；视口参数不合法回 `invalid_params`；`resize`
/// 认不得的视图回 `not_found`。
#[tokio::test]
async fn browser_view_url_and_viewport_errors() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(
        init["result"]["capabilities"]["browserViewport"], true,
        "{init}"
    );

    let opened = rpc
        .call("browser/view/open", json!({ "url": "example.com" }))
        .await;
    assert_eq!(
        opened["error"]["details"]["code"], "browser_unavailable",
        "{opened}"
    );
    let bad_viewport = rpc
        .call("browser/view/open", json!({ "viewport": { "width": 10 } }))
        .await;
    assert_eq!(
        bad_viewport["error"]["details"]["code"], "invalid_params",
        "{bad_viewport}"
    );
    let resize = rpc
        .call(
            "browser/view/resize",
            json!({ "viewId": "nope", "width": 800, "height": 600 }),
        )
        .await;
    assert_eq!(resize["error"]["details"]["code"], "not_found", "{resize}");
}

/// cua-driver 的结果并进工具文本时截到 8KB：`"key": [...]` 里截断之前完整的那些对象。
fn objects_after(text: &str, key: &str) -> Vec<Value> {
    let Some(at) = text.find(&format!("\"{key}\"")) else {
        return Vec::new();
    };
    let Some(open) = text[at..].find('[') else {
        return Vec::new();
    };
    let mut rest = &text[at + open + 1..];
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if !rest.starts_with('{') {
            return out;
        }
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
        match stream.next() {
            Some(Ok(v)) => {
                out.push(v);
                rest = &rest[stream.byte_offset()..];
            }
            _ => return out,
        }
    }
}

/// 工具文本里 `"key": 数字` 的第一个值。
fn number_after(text: &str, key: &str) -> Option<i64> {
    let at = text.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = text[at..].trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// 真 cua-driver（macOS，要辅助功能 + 屏幕录制权限；会在后台开一个计算器）：agent 按了
/// 计算器的「1」之后，桌面画面推计算器的帧，再推一次光标——落在「1」的中心（窗口里的
/// 相对位置），动作是 `click`。
#[tokio::test]
#[ignore = "needs cua-driver with Accessibility + Screen Recording; launches Calculator"]
async fn desktop_view_follows_the_agent_cursor() {
    isolated_home();
    // 测试进程默认关掉内置 cua-driver 行；这里按 PATH 找真的那个。
    let Some(driver) = cordis_base::cua::discover_with(
        None,
        std::env::var_os("PATH"),
        std::env::var_os("HOME").map(std::path::PathBuf::from),
    ) else {
        eprintln!("skip: cua-driver not installed");
        return;
    };
    std::env::set_var("DOCK_CUA_DRIVER", &driver);
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    rpc.call("initialize", json!({})).await;

    let mcp = h.ctx.require::<Mcp>(MCP).unwrap();
    let call = |tool: &'static str, args: Value| {
        let mcp = mcp.clone();
        let ctx = h.ctx.clone();
        async move {
            let out = mcp.call_as(&ctx, "cua-driver", tool, args).await.unwrap();
            assert!(!out.is_error, "{tool}: {}", out.content);
            out.content
        }
    };
    // MCP 客户端连上 cua-driver 要一会儿。
    let mut launched = None;
    for _ in 0..40 {
        let out = mcp
            .call_as(
                &h.ctx,
                "cua-driver",
                "launch_app",
                json!({ "bundle_id": "com.apple.calculator" }),
            )
            .await;
        if let Ok(out) = out {
            if !out.is_error {
                launched = number_after(&out.content, "pid");
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let pid = launched.expect("cua-driver connects");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let windows = objects_after(
        &call("list_windows", json!({ "pid": pid })).await,
        "windows",
    );
    let window_id = windows[0]["window_id"].as_i64().unwrap();
    let elements = objects_after(
        &call(
            "get_window_state",
            json!({ "pid": pid, "window_id": window_id, "query": "1" }),
        )
        .await,
        "elements",
    );
    let one = elements
        .into_iter()
        .find(|e| e["label"] == "1")
        .expect("button 1");

    let opened = rpc
        .call("desktop/view/open", json!({ "maxDimension": 640 }))
        .await;
    assert!(opened["result"]["viewId"].is_string(), "{opened}");

    // agent 按下「1」：真点一下，再照 agent 的样子记进会话日志。
    let args = json!({ "pid": pid, "window_id": window_id, "element_token": one["element_token"] });
    call("click", args.clone()).await;
    h.ctx
        .require::<Sessions>(SESSIONS)
        .unwrap()
        .append(LogEvent::ToolExecute {
            id: "click-1".into(),
            name: "use_tool".into(),
            arguments: json!({ "tool_name": "mcp_cua-driver__click", "tool_input": args })
                .to_string(),
            content: String::new(),
            images: Vec::new(),
            is_error: false,
        });

    // 开画面时还没有 agent 的窗口，先推的是最前面的窗口；等到计算器那一帧。
    let frame = loop {
        let frame = rpc
            .wait_notification("desktop/view/frame", Duration::from_secs(10))
            .await;
        if frame["params"]["window"]["pid"] == pid {
            break frame;
        }
    };
    assert_eq!(
        frame["params"]["source"], "agent",
        "{}",
        frame["params"]["window"]
    );
    let cursor = rpc
        .wait_notification("desktop/view/cursor", Duration::from_secs(10))
        .await;
    let p = &cursor["params"];
    assert_eq!(p["action"], "click", "{p}");
    assert_eq!(p["windowId"], window_id, "{p}");
    let bounds = &frame["params"]["window"]["bounds"];
    let frame_of = |k: &str| one["frame"][k].as_f64().unwrap();
    let expect_x = (frame_of("x") + frame_of("w") / 2.0 - bounds["x"].as_f64().unwrap())
        / bounds["width"].as_f64().unwrap();
    let expect_y = (frame_of("y") + frame_of("h") / 2.0 - bounds["y"].as_f64().unwrap())
        / bounds["height"].as_f64().unwrap();
    assert!(
        (p["x"].as_f64().unwrap() - expect_x).abs() < 0.03
            && (p["y"].as_f64().unwrap() - expect_y).abs() < 0.03,
        "cursor {p} vs button center ({expect_x}, {expect_y})"
    );

    // 收拾：⌘Q 退出这个计算器（和 agent 平时关应用一样，不强杀）。
    call(
        "hotkey",
        json!({ "pid": pid, "window_id": window_id, "keys": ["cmd", "q"] }),
    )
    .await;
}

/// 真 cua-driver 冒烟：同一会话连发多次 `desktop/view/open`（GUI 开发模式下 StrictMode 双挂载
/// 就是这样），客户端像 GUI 一样把先发的关掉——最后发的那个必须还活着、接着出帧。
/// 注意：这里 cua-driver 基本按先来先完成，「旧 open 晚做完、把新的顶掉」的竞态造不出来
/// （GUI 里实测出现过），排序本身由 `desktop_view` 的单元测试 `only_the_last_received_open_counts` 管。
#[tokio::test]
#[ignore = "needs cua-driver with Screen Recording"]
async fn desktop_view_concurrent_opens_keep_the_latest() {
    isolated_home();
    let Some(driver) = cordis_base::cua::discover_with(
        None,
        std::env::var_os("PATH"),
        std::env::var_os("HOME").map(std::path::PathBuf::from),
    ) else {
        eprintln!("skip: cua-driver not installed");
        return;
    };
    std::env::set_var("DOCK_CUA_DRIVER", &driver);
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    rpc.call("initialize", json!({})).await;
    // MCP 客户端连上 cua-driver 要一会儿。
    for _ in 0..40 {
        let probe = rpc
            .call("desktop/view/open", json!({ "maxDimension": 320 }))
            .await;
        if let Some(id) = probe["result"]["viewId"].as_str() {
            rpc.call("desktop/view/close", json!({ "viewId": id }))
                .await;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    for round in 0..6 {
        // 一口气发 8 个：并发越多，旧 open 晚做完的机会越大。
        let mut ids = Vec::new();
        for _ in 0..8 {
            ids.push(
                rpc.send("desktop/view/open", json!({ "maxDimension": 320 }))
                    .await,
            );
        }
        let (replies, _) = rpc.collect(&ids, Duration::from_secs(20)).await;
        let last = ids[ids.len() - 1];
        let live = replies[&last]["result"]["viewId"]
            .as_str()
            .unwrap_or_else(|| panic!("round {round}: 最后发的 open 要成功：{replies:?}"))
            .to_string();
        for id in &ids[..ids.len() - 1] {
            match replies[id]["result"]["viewId"].as_str() {
                // 先发的也成功了（它先做完、随后被顶掉）：GUI 会把它关掉。
                Some(stale) => {
                    rpc.call("desktop/view/close", json!({ "viewId": stale }))
                        .await;
                }
                None => assert_eq!(
                    replies[id]["error"]["details"]["code"], "superseded",
                    "round {round}: {replies:?}"
                ),
            }
        }
        loop {
            let frame = rpc
                .wait_notification("desktop/view/frame", Duration::from_secs(5))
                .await;
            if frame["params"]["viewId"] == live.as_str() {
                break;
            }
        }
        rpc.call("desktop/view/close", json!({ "viewId": live }))
            .await;
    }
}

/// 桌面实时画面（不需要 cua-driver）：能力位在；驱动没连上 `open` 直接回
/// `driver_unavailable`，不先给一个马上就死的 viewId；`close` 缺 viewId 回 `invalid_params`。
#[tokio::test]
async fn desktop_view_errors_without_a_driver() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(
        init["result"]["capabilities"]["desktopView"], true,
        "{init}"
    );

    let opened = rpc.call("desktop/view/open", json!({})).await;
    assert_eq!(
        opened["error"]["details"]["code"], "driver_unavailable",
        "{opened}"
    );
    let missing = rpc.call("desktop/view/close", json!({})).await;
    assert_eq!(
        missing["error"]["details"]["code"], "invalid_params",
        "{missing}"
    );
    let unknown = rpc
        .call("desktop/view/close", json!({ "viewId": "nope" }))
        .await;
    assert_eq!(
        unknown["error"]["details"]["code"], "not_found",
        "{unknown}"
    );
}

/// 真 Chrome：agent（这里直接用浏览器 MCP 的 hub）给第 1 页开了标签页 → 客户端
/// `browser/view/open` 收到帧 → 以用户身份点按钮 → 地址栏导航推 `status` →
/// agent 关掉自己的标签页，视图推 `closed { reason: "no_tab" }`。
#[tokio::test]
#[ignore = "needs a real Chrome"]
async fn browser_view_streams_the_agents_tab() {
    if cordis_browser::session::discover_chrome().is_err() {
        eprintln!("skip: chrome not installed");
        return;
    }
    let h = Harness::boot().await;
    let sessions = h.ctx.require::<Sessions>(SESSIONS).unwrap();
    let key = cordis_spine::mcp_session_key(&sessions);
    let hub = cordis_browser::BrowserHub::new();
    let opened = hub
        .call(
            &key,
            "browser_open",
            &json!({"url": "data:text/html,<title>Before</title><button style='position:fixed;left:0;top:0;width:200px;height:100px' onclick=\"document.title='Clicked'\">Go</button>"}),
        )
        .await;
    assert!(!opened.is_error, "{}", opened.text);

    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let view = rpc
        .call("browser/view/open", json!({ "maxWidth": 640 }))
        .await;
    let view_id = view["result"]["viewId"]
        .as_str()
        .unwrap_or_else(|| panic!("{view}"))
        .to_string();
    assert_eq!(view["result"]["title"], "Before", "{view}");
    let frame = rpc
        .wait_notification("browser/view/frame", Duration::from_secs(10))
        .await;
    assert_eq!(frame["params"]["viewId"], view_id.as_str());
    assert_eq!(frame["params"]["mime"], "image/jpeg");
    assert!(frame["params"]["data"].as_str().unwrap().len() > 100);
    assert!(frame["params"]["width"].as_f64().unwrap() > 0.0, "{frame}");

    let click = rpc
        .call(
            "browser/view/input",
            json!({ "viewId": view_id, "event": { "type": "mouse", "action": "click", "x": 50, "y": 50 } }),
        )
        .await;
    assert_eq!(click["result"]["ok"], true, "{click}");
    let status = rpc
        .wait_notification("browser/view/status", Duration::from_secs(5))
        .await;
    assert_eq!(status["params"]["title"], "Clicked", "{status}");

    let nav = rpc
        .call(
            "browser/view/navigate",
            json!({ "viewId": view_id, "url": "data:text/html,<title>Second</title>" }),
        )
        .await;
    assert_eq!(nav["result"]["ok"], true, "{nav}");
    let bad = rpc
        .call(
            "browser/view/navigate",
            json!({ "viewId": view_id, "url": "javascript:alert(1)" }),
        )
        .await;
    assert_eq!(bad["error"]["details"]["code"], "invalid_params", "{bad}");
    loop {
        let status = rpc
            .wait_notification("browser/view/status", Duration::from_secs(5))
            .await;
        if status["params"]["title"] == "Second" {
            break;
        }
    }

    // agent 关掉自己的标签页：视图跟着结束。
    let closed = hub.call(&key, "browser_close", &json!({})).await;
    assert!(!closed.is_error, "{}", closed.text);
    let ended = rpc
        .wait_notification("browser/view/closed", Duration::from_secs(5))
        .await;
    assert_eq!(ended["params"]["reason"], "no_tab", "{ended}");
    hub.shutdown().await;
}

/// 真 Chrome：标签栏。`open` 回会话的 `tabs`；agent 新开一页（这里直接用浏览器 MCP 的
/// hub）→ 推 `browser/view/tabs`（两页、新页是当前页）且画面切到新页；页面自己开的新页
/// （`target=_blank`）经 agent 下一次调用收进会话后同样出现在标签栏。
#[tokio::test]
#[ignore = "needs a real Chrome"]
async fn browser_view_pushes_the_sessions_tabs() {
    if cordis_browser::session::discover_chrome().is_err() {
        eprintln!("skip: chrome not installed");
        return;
    }
    let h = Harness::boot().await;
    let sessions = h.ctx.require::<Sessions>(SESSIONS).unwrap();
    let key = cordis_spine::mcp_session_key(&sessions);
    let hub = cordis_browser::BrowserHub::new();
    let opened = hub
        .call(
            &key,
            "browser_open",
            &json!({"url": "data:text/html,<title>One</title><a href='about:blank%23three' target='_blank' style='position:fixed;left:0;top:0;width:200px;height:100px;display:block'>Pop</a>"}),
        )
        .await;
    assert!(!opened.is_error, "{}", opened.text);

    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let view = rpc.call("browser/view/open", json!({})).await;
    let tabs = view["result"]["tabs"]
        .as_array()
        .unwrap_or_else(|| panic!("{view}"));
    assert_eq!(tabs.len(), 1, "{view}");
    assert_eq!(tabs[0]["title"], "One");
    assert_eq!(tabs[0]["active"], true);
    let first = tabs[0]["targetId"].as_str().unwrap().to_string();

    let new = hub
        .call(
            &key,
            "browser_tabs",
            &json!({"action": "new", "url": "data:text/html,<title>Two</title>"}),
        )
        .await;
    assert!(!new.is_error, "{}", new.text);
    // 画面先切过去（status），标签栏下一拍跟上。
    loop {
        let status = rpc
            .wait_notification("browser/view/status", Duration::from_secs(5))
            .await;
        if status["params"]["title"] == "Two" {
            break;
        }
    }
    let pushed = loop {
        let note = rpc
            .wait_notification("browser/view/tabs", Duration::from_secs(5))
            .await;
        let tabs = note["params"]["tabs"].as_array().unwrap().clone();
        if tabs.len() == 2 && tabs[1]["title"] == "Two" {
            break tabs;
        }
    };
    assert_eq!(pushed[0]["targetId"], first.as_str());
    assert_eq!(pushed[1]["active"], true, "{pushed:?}");

    // 回第一页，点 target=_blank 链接：agent 下一次调用把新页收进会话，插在第一页右边。
    let back = hub
        .call(
            &key,
            "browser_tabs",
            &json!({"action": "switch", "index": 0}),
        )
        .await;
    assert!(!back.is_error, "{}", back.text);
    let snap = hub.call(&key, "browser_snapshot", &json!({})).await.text;
    let line = snap
        .lines()
        .find(|l| l.contains("\"Pop\""))
        .unwrap_or_else(|| panic!("{snap}"));
    let r = &line[line.find("[ref=").unwrap() + 5..line.rfind(']').unwrap()];
    let click = hub.call(&key, "browser_click", &json!({ "ref": r })).await;
    assert!(!click.is_error, "{}", click.text);
    tokio::time::sleep(Duration::from_millis(800)).await;
    let _ = hub.call(&key, "browser_tabs", &json!({})).await;
    loop {
        let note = rpc
            .wait_notification("browser/view/tabs", Duration::from_secs(5))
            .await;
        let tabs = note["params"]["tabs"].as_array().unwrap().clone();
        if tabs.len() == 3 {
            assert_eq!(tabs[1]["url"], "about:blank#three", "{tabs:?}");
            assert_eq!(tabs[1]["active"], true, "{tabs:?}");
            break;
        }
    }
    hub.shutdown().await;
}

/// 真 Chrome + 真浏览器 MCP（`target/debug/dock mcp browser`，先 `cargo build -p cordis-app`）：
/// 标签栏经 `browser/view/tab` 新开、关掉非最后一页、再关最后一页。关最后一页以前回
/// `tab_failed`（`browser_tabs` 不让关），现在改调 `browser_close`：回 ok 并推
/// `closed { reason: "no_tab" }`，之后再 open 回 `no_tab`。
#[tokio::test]
#[ignore = "needs a real Chrome and a built dock binary"]
async fn browser_view_tab_closes_the_last_tab() {
    if cordis_browser::session::discover_chrome().is_err() {
        eprintln!("skip: chrome not installed");
        return;
    }
    // 测试二进制在 target/debug/deps/ 下，dock 在 target/debug/。
    let dock = std::env::current_exe()
        .unwrap()
        .parent()
        .and_then(|deps| deps.parent())
        .map(|dir| dir.join("dock"))
        .filter(|p| p.is_file());
    let Some(dock) = dock else {
        eprintln!("skip: target/debug/dock not built (cargo build -p cordis-app)");
        return;
    };
    cordis_base::config::register_builtin_browser_mcp(dock);
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;

    // MCP 客户端拉起 `dock mcp browser` 要一会儿；开页还要拉起 Chromium。
    let page = "data:text/html,<title>One</title><h1>one</h1>";
    let mut view = Value::Null;
    for _ in 0..40 {
        view = rpc.call("browser/view/open", json!({ "url": page })).await;
        if view.get("result").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let view_id = view["result"]["viewId"]
        .as_str()
        .unwrap_or_else(|| panic!("{view}"))
        .to_string();
    let tab = |action: &str, extra: Value| {
        let mut params = json!({ "viewId": view_id, "action": action });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        params
    };
    let tabs_of = |note: &Value| note["params"]["tabs"].as_array().unwrap().clone();

    let new = rpc
        .call(
            "browser/view/tab",
            tab("new", json!({ "url": "data:text/html,<title>Two</title>" })),
        )
        .await;
    assert_eq!(new["result"]["ok"], true, "{new}");
    let two = loop {
        let note = rpc
            .wait_notification("browser/view/tabs", Duration::from_secs(5))
            .await;
        if tabs_of(&note).len() == 2 {
            break tabs_of(&note);
        }
    };
    let first = two[0]["targetId"].as_str().unwrap().to_string();
    let closed = rpc
        .call(
            "browser/view/tab",
            tab("close", json!({ "targetId": first })),
        )
        .await;
    assert_eq!(closed["result"]["ok"], true, "{closed}");
    let one = loop {
        let note = rpc
            .wait_notification("browser/view/tabs", Duration::from_secs(5))
            .await;
        if tabs_of(&note).len() == 1 {
            break tabs_of(&note);
        }
    };
    assert_eq!(one[0]["title"], "Two", "{one:?}");

    let last = one[0]["targetId"].as_str().unwrap().to_string();
    let closed = rpc
        .call(
            "browser/view/tab",
            tab("close", json!({ "targetId": last })),
        )
        .await;
    assert_eq!(closed["result"]["ok"], true, "{closed}");
    let ended = rpc
        .wait_notification("browser/view/closed", Duration::from_secs(5))
        .await;
    assert_eq!(ended["params"]["reason"], "no_tab", "{ended}");
    let again = rpc.call("browser/view/open", json!({})).await;
    assert_eq!(again["error"]["details"]["code"], "no_tab", "{again}");
}

/// 真 Chrome：页面早就加载完、一动不动时打开视图。无头 screencast 只出 CSS 像素、页面
/// 不动时可能一帧都不推（以前面板会一直停在骨架屏）；现在画面停下来一定补一张静止帧
/// （`still: true`），并按 `viewport.deviceScaleFactor` 倍截（这里没接浏览器 MCP，视口
/// 本身改不了，页面仍是 800×600，静止帧就是 1600 像素宽）。
#[tokio::test]
#[ignore = "needs a real Chrome"]
async fn browser_view_static_page_still_gets_a_frame() {
    if cordis_browser::session::discover_chrome().is_err() {
        eprintln!("skip: chrome not installed");
        return;
    }
    let h = Harness::boot().await;
    let sessions = h.ctx.require::<Sessions>(SESSIONS).unwrap();
    let key = cordis_spine::mcp_session_key(&sessions);
    let hub = cordis_browser::BrowserHub::new();
    let opened = hub
        .call(
            &key,
            "browser_open",
            &json!({"url": "data:text/html,<title>Still</title><h1>nothing moves</h1>"}),
        )
        .await;
    assert!(!opened.is_error, "{}", opened.text);
    tokio::time::sleep(Duration::from_secs(1)).await;

    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let view = rpc
        .call(
            "browser/view/open",
            json!({ "viewport": { "width": 800, "height": 600, "deviceScaleFactor": 2 } }),
        )
        .await;
    assert!(view["result"]["viewId"].is_string(), "{view}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let still = loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = rpc.wait_notification("browser/view/frame", left).await;
        if frame["params"]["still"] == true {
            break frame;
        }
    };
    assert_eq!(still["params"]["width"], 800.0, "{still}");
    let jpeg = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        still["params"]["data"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(jpeg_width(&jpeg), Some(1600));
    hub.shutdown().await;
}

/// JPEG 的像素宽（第一个 SOF 段里）。
fn jpeg_width(bytes: &[u8]) -> Option<u16> {
    let mut i = 2;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if (0xC0..=0xC2).contains(&marker) {
            return Some(u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]));
        }
        i += 2 + len;
    }
    None
}

/// 真 Chrome：同一会话连发两次 `browser/view/open`，后发的那个一定活着；先发的要么回
/// `superseded`，要么成功后收到 `closed { reason: "replaced" }`——不会悄悄死掉。
/// 不等回包连发的文字，按发送顺序进页面（网关每个请求各开任务，以前会乱序）。
// 多线程运行时：`dock serve` 就是这样跑的；单线程下任务按开的顺序轮到，乱序复现不了。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a real Chrome"]
async fn browser_view_concurrent_opens_and_inputs_keep_order() {
    if cordis_browser::session::discover_chrome().is_err() {
        eprintln!("skip: chrome not installed");
        return;
    }
    let h = Harness::boot().await;
    let sessions = h.ctx.require::<Sessions>(SESSIONS).unwrap();
    let key = cordis_spine::mcp_session_key(&sessions);
    let hub = cordis_browser::BrowserHub::new();
    let opened = hub
        .call(
            &key,
            "browser_open",
            &json!({"url": "data:text/html,<input id=i style='position:fixed;left:0;top:0;width:300px;height:40px'>"}),
        )
        .await;
    assert!(!opened.is_error, "{}", opened.text);

    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;

    let mut live = String::new();
    for round in 0..6 {
        let a = rpc
            .send("browser/view/open", json!({ "maxWidth": 320 }))
            .await;
        let b = rpc
            .send("browser/view/open", json!({ "maxWidth": 320 }))
            .await;
        let (replies, mut notes) = rpc.collect(&[a, b], Duration::from_secs(20)).await;
        live = replies[&b]["result"]["viewId"]
            .as_str()
            .unwrap_or_else(|| panic!("round {round}: 后发的 open 要成功：{replies:?}"))
            .to_string();
        let first = &replies[&a];
        if let Some(stale) = first["result"]["viewId"].as_str() {
            // 先发的成功了，就一定要被告知它被顶掉了。
            let replaced =
                |n: &Value| n["method"] == "browser/view/closed" && n["params"]["viewId"] == stale;
            if !notes.iter().any(replaced) {
                loop {
                    let n = rpc
                        .wait_notification("browser/view/closed", Duration::from_secs(5))
                        .await;
                    notes.push(n.clone());
                    if replaced(&n) {
                        break;
                    }
                }
            }
            let note = notes.iter().find(|n| replaced(n)).unwrap();
            assert_eq!(note["params"]["reason"], "replaced", "{note}");
        } else {
            assert_eq!(first["error"]["details"]["code"], "superseded", "{first}");
        }
        let ok = rpc
            .call(
                "browser/view/input",
                json!({ "viewId": live, "event": { "type": "mouse", "action": "move", "x": 5, "y": 5 } }),
            )
            .await;
        assert_eq!(
            ok["result"]["ok"], true,
            "round {round}: 活着的视图要能用：{ok}"
        );
    }

    let click = rpc
        .call(
            "browser/view/input",
            json!({ "viewId": live, "event": { "type": "mouse", "action": "click", "x": 20, "y": 20 } }),
        )
        .await;
    assert_eq!(click["result"]["ok"], true, "{click}");
    let typed = "abcdefghijklmnopqrstuvwxyz0123456789".repeat(4);
    let mut ids = Vec::new();
    for ch in typed.chars() {
        ids.push(
            rpc.send(
                "browser/view/input",
                json!({ "viewId": live, "event": { "type": "text", "text": ch.to_string() } }),
            )
            .await,
        );
    }
    let (replies, _) = rpc.collect(&ids, Duration::from_secs(20)).await;
    assert!(
        replies.values().all(|r| r["result"]["ok"] == true),
        "{replies:?}"
    );
    let value = hub
        .call(
            &key,
            "browser_evaluate",
            &json!({ "expression": "document.getElementById('i').value" }),
        )
        .await;
    assert!(
        value.text.contains(&typed),
        "文字要按发送顺序进页面：{}",
        value.text
    );
    hub.shutdown().await;
}

/// 远程模式：配对 HTTP 不挂；ticket 不认；设备令牌能进，`initialize` 报 `device:<名字>`；
/// 撤销后几秒内连接被服务端关掉（4401）。
#[tokio::test]
async fn remote_gateway_accepts_only_device_tokens_and_drops_revoked() {
    let h = Harness::boot_remote().await;
    let pairing = h
        .post("/v1/pairing/requests", json!({ "application": APP }))
        .await;
    assert_eq!(pairing.status(), 404, "远程模式没有配对入口");
    let tickets = h.post("/v1/connection/tickets", json!({})).await;
    assert_eq!(tickets.status(), 404);

    let mut stranger = Rpc::open(h.addr, PAGE_ORIGIN).await;
    let refused = stranger
        .call("connection/authenticate", json!({ "ticket": "whatever" }))
        .await;
    assert_eq!(
        refused["error"]["details"]["code"], "unauthenticated",
        "{refused}"
    );
    assert!(refused.to_string().contains("设备令牌"), "{refused}");

    let (device, token) = devices::add("remote-test-laptop").unwrap();
    // 设备令牌不绑 Origin：换一个页面来源照样能进。
    let mut rpc = Rpc::open(h.addr, "https://dock.example.com").await;
    let auth = rpc
        .call("connection/authenticate", json!({ "token": token }))
        .await;
    assert_eq!(auth["result"]["ok"], true, "{auth}");
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(
        init["result"]["connection"]["application"], "device:remote-test-laptop",
        "{init}"
    );
    assert_eq!(
        init["result"]["connection"]["origin"],
        "https://dock.example.com"
    );
    let listed = rpc.call("thread/list", json!({})).await;
    assert!(listed.get("result").is_some(), "{listed}");

    devices::revoke(&device.id).unwrap();
    let code = rpc.wait_closed(Duration::from_secs(8)).await;
    assert_eq!(code, Some(4401), "撤销后断开");
    let mut again = Rpc::open(h.addr, PAGE_ORIGIN).await;
    let denied = again
        .call("connection/authenticate", json!({ "token": token }))
        .await;
    assert_eq!(
        denied["error"]["details"]["code"], "unauthenticated",
        "{denied}"
    );
}

/// 同一条连接鉴权失败 5 次就被断开（4429）。
#[tokio::test]
async fn repeated_failed_authentication_closes_the_connection() {
    let h = Harness::boot_remote().await;
    let mut rpc = Rpc::open(h.addr, PAGE_ORIGIN).await;
    for _ in 0..4 {
        let bad = rpc
            .call("connection/authenticate", json!({ "token": "dock_wrong" }))
            .await;
        assert_eq!(bad["error"]["details"]["code"], "unauthenticated", "{bad}");
    }
    rpc.write
        .send(Message::Text(
            json!({ "id": 99, "method": "connection/authenticate", "params": { "token": "dock_wrong" } })
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(rpc.wait_closed(Duration::from_secs(5)).await, Some(4429));
}

/// 本机网关也认设备令牌（凭据就是凭据）；本机的 ticket 配对照旧。
#[tokio::test]
async fn local_gateway_also_accepts_device_tokens() {
    let h = Harness::boot().await;
    let (_device, token) = devices::add("local-test-device").unwrap();
    let mut rpc = Rpc::open(h.addr, PAGE_ORIGIN).await;
    let auth = rpc
        .call("connection/authenticate", json!({ "token": token }))
        .await;
    assert_eq!(auth["result"]["ok"], true, "{auth}");
    let ticket = h.pair_ticket().await;
    let _ = Rpc::connect(h.addr, &ticket).await;
    devices::revoke("local-test-device").unwrap();
}

/// `fs/*`：只读看会话 cwd 里的文件。列目录照 `.gitignore`、目录在前；读文本 / 图片 /
/// 二进制；按名字找；出了工作区、没开的会话、缺参数都拒绝。
#[tokio::test]
async fn fs_methods_browse_the_sessions_workspace_read_only() {
    let h = Harness::boot_with_pages().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(
        init["result"]["capabilities"]["workspaceFiles"], true,
        "{init}"
    );

    let dir = project_dir("fs-panel");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("target")).unwrap();
    std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(dir.join("notes.md"), "# 笔记\n").unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub fn x() {}\n").unwrap();
    std::fs::write(dir.join("target/app"), [0u8, 1]).unwrap();
    std::fs::write(dir.join("dot.png"), [0x89, b'P', b'N', b'G']).unwrap();
    let started = rpc
        .call("thread/start", json!({ "cwd": dir.display().to_string() }))
        .await;
    let id = started["result"]["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let root = rpc.call("fs/list", json!({ "threadId": id })).await;
    let names: Vec<&str> = root["result"]["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("{root}"))
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["src", "dot.png", "notes.md"], "{root}");
    assert_eq!(root["result"]["entries"][0]["kind"], "dir");

    let sub = rpc
        .call("fs/list", json!({ "threadId": id, "path": "src" }))
        .await;
    assert_eq!(sub["result"]["entries"][0]["path"], "src/lib.rs", "{sub}");

    let text = rpc
        .call("fs/read", json!({ "threadId": id, "path": "notes.md" }))
        .await;
    assert_eq!(text["result"]["kind"], "text", "{text}");
    assert_eq!(text["result"]["text"], "# 笔记\n");
    let image = rpc
        .call("fs/read", json!({ "threadId": id, "path": "dot.png" }))
        .await;
    assert_eq!(image["result"]["kind"], "image", "{image}");
    assert_eq!(image["result"]["data"], "iVBORw==");

    let found = rpc
        .call("fs/find", json!({ "threadId": id, "query": "lib" }))
        .await;
    assert_eq!(found["result"]["paths"], json!(["src/lib.rs"]), "{found}");

    let escape = rpc
        .call("fs/read", json!({ "threadId": id, "path": "../x" }))
        .await;
    assert_eq!(
        escape["error"]["details"]["code"], "invalid_params",
        "{escape}"
    );
    let missing = rpc.call("fs/read", json!({ "threadId": id })).await;
    assert_eq!(
        missing["error"]["details"]["code"], "invalid_params",
        "{missing}"
    );
    let closed = rpc
        .call("fs/list", json!({ "threadId": "no-such-thread" }))
        .await;
    assert_eq!(
        closed["error"]["details"]["code"], "thread_not_open",
        "{closed}"
    );
}

/// 工具结果里的图（cua-driver 截图之类）：`item/tool_completed` 带 `attachments`
/// 元数据（`index` 是在这次结果里的位置），像素走 `item/image`；归档后从落盘读也一样。
#[tokio::test]
async fn tool_images_are_listed_and_fetched_by_item() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(init["result"]["capabilities"]["toolImages"], true, "{init}");

    // 1×1 PNG。
    let png: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52, 0, 0,
        0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 13, 0x49, 0x44, 0x41,
        0x54, 0x78, 0xDA, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0, 0x05, 0x00, 0x01, 0xFF, 0x89,
        0x99, 0x3D, 0x1D, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let image = |data: Vec<u8>| cordis_spine::UserImage {
        mime: "image/png".into(),
        data: data.into(),
        width: 1,
        height: 1,
    };
    let sessions = h.ctx.require::<Sessions>(SESSIONS).unwrap();
    sessions.append(LogEvent::User("看看桌面".into()));
    sessions.append(LogEvent::ToolExecute {
        id: "shot-1".into(),
        name: "mcp_cua-driver__get_window_state".into(),
        arguments: "{}".into(),
        content: "window tree".into(),
        // 第 0 张是空的（不投影、不占号），第 1 张才是真图。
        images: vec![image(Vec::new()), image(png.clone())],
        is_error: false,
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let history = rpc
        .call("thread/history", json!({ "threadId": "live" }))
        .await;
    let completed = history["result"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["method"] == "item/tool_completed")
        .unwrap_or_else(|| panic!("{history}"))
        .clone();
    let attachments = &completed["payload"]["attachments"];
    assert_eq!(attachments.as_array().map(Vec::len), Some(1), "{completed}");
    assert_eq!(attachments[0]["index"], 1);
    assert_eq!(attachments[0]["mimeType"], "image/png");

    use base64::Engine;
    let want = base64::engine::general_purpose::STANDARD.encode(&png);
    let got = rpc
        .call(
            "item/image",
            json!({ "threadId": "live", "itemId": "shot-1", "index": 1 }),
        )
        .await;
    assert_eq!(got["result"]["data"], want, "{got}");
    assert_eq!(got["result"]["width"], 1);

    let empty = rpc
        .call(
            "item/image",
            json!({ "threadId": "live", "itemId": "shot-1", "index": 0 }),
        )
        .await;
    assert_eq!(empty["error"]["details"]["code"], "not_found", "{empty}");
    let missing = rpc
        .call(
            "item/image",
            json!({ "threadId": "live", "itemId": "nope" }),
        )
        .await;
    assert_eq!(missing["error"]["details"]["code"], "not_found");

    // 归档（关掉）之后从落盘读回来。
    let archived = rpc
        .call("thread/archive", json!({ "threadId": "live" }))
        .await;
    let id = archived["result"]["thread"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{archived}"))
        .to_string();
    let from_disk = rpc
        .call(
            "item/image",
            json!({ "threadId": id, "itemId": "shot-1", "index": 1 }),
        )
        .await;
    assert_eq!(from_disk["result"]["data"], want, "{from_disk}");
}

/// 画布：模型工具写进会话目录的 `canvas/<id>/`，`canvas/*` 列 / 读版本 / 改数据 / 回滚；
/// 会话归档（关掉）之后从名册找到目录，照样读得到。
#[tokio::test]
async fn canvas_methods_read_the_sessions_canvases() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(init["result"]["capabilities"]["canvas"], true, "{init}");

    // 不落盘的会话、还没落过盘的新会话：空列表，不替它建目录。
    let empty = rpc.call("canvas/list", json!({ "threadId": "live" })).await;
    assert_eq!(empty["result"]["canvases"], json!([]), "{empty}");
    let sessions = h.ctx.require::<Sessions>(SESSIONS).unwrap();
    sessions.attach_disk();
    let empty = rpc.call("canvas/list", json!({ "threadId": "live" })).await;
    assert_eq!(empty["result"]["canvases"], json!([]), "{empty}");

    sessions.append(LogEvent::User("画个图".into()));
    let dir = sessions.disk_session_dir().expect("disk-backed session");
    cordis_base::canvas::create(&dir, "销售", "<h1>A</h1>", Some(&json!({ "n": 1 }))).unwrap();
    cordis_base::canvas::edit(
        &dir,
        "canvas-1",
        cordis_base::canvas::Edit::Rewrite { html: "<h1>B</h1>" },
        "换标题",
        None,
    )
    .unwrap();

    let list = rpc.call("canvas/list", json!({ "threadId": "live" })).await;
    let first = &list["result"]["canvases"][0];
    assert_eq!(first["id"], "canvas-1", "{list}");
    assert_eq!(first["latest"], 2);
    assert_eq!(first["versions"][1]["note"], "换标题");

    let got = rpc
        .call(
            "canvas/get",
            json!({ "threadId": "live", "canvasId": "canvas-1" }),
        )
        .await;
    assert_eq!(got["result"]["html"], "<h1>B</h1>", "{got}");
    assert_eq!(got["result"]["data"], json!({ "n": 1 }));
    let v1 = rpc
        .call(
            "canvas/get",
            json!({ "threadId": "live", "canvasId": "canvas-1", "version": 1 }),
        )
        .await;
    assert_eq!(v1["result"]["html"], "<h1>A</h1>", "{v1}");

    let set = rpc
        .call(
            "canvas/setData",
            json!({ "threadId": "live", "canvasId": "canvas-1", "data": [1, 2] }),
        )
        .await;
    assert_eq!(
        set["result"]["canvas"]["latest"], 2,
        "改数据不出新版：{set}"
    );
    let back = rpc
        .call(
            "canvas/rollback",
            json!({ "threadId": "live", "canvasId": "canvas-1", "version": 1 }),
        )
        .await;
    assert_eq!(back["result"]["canvas"]["latest"], 3, "{back}");

    let bad = rpc
        .call(
            "canvas/get",
            json!({ "threadId": "live", "canvasId": "../canvas-1" }),
        )
        .await;
    assert_eq!(bad["error"]["details"]["code"], "invalid", "{bad}");
    let missing = rpc
        .call(
            "canvas/get",
            json!({ "threadId": "live", "canvasId": "canvas-9" }),
        )
        .await;
    assert_eq!(
        missing["error"]["details"]["code"], "not_found",
        "{missing}"
    );

    let archived = rpc
        .call("thread/archive", json!({ "threadId": "live" }))
        .await;
    let id = archived["result"]["thread"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{archived}"))
        .to_string();
    let closed = rpc
        .call(
            "canvas/get",
            json!({ "threadId": id, "canvasId": "canvas-1" }),
        )
        .await;
    assert_eq!(closed["result"]["html"], "<h1>A</h1>", "{closed}");
    assert_eq!(closed["result"]["data"], json!([1, 2]));
}

/// 收推送直到 `done` 命中，回途中收到的全部推送（含命中那条）。
async fn notes_until(
    rpc: &mut Rpc,
    deadline: Duration,
    done: impl Fn(&Value) -> bool,
) -> Vec<Value> {
    let start = tokio::time::Instant::now();
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_sub(start.elapsed());
        let msg = tokio::time::timeout(left, rpc.read.next())
            .await
            .unwrap_or_else(|_| panic!("timed out; got {seen:#?}"))
            .expect("ws closed")
            .unwrap();
        let value: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
        let hit = done(&value);
        seen.push(value);
        if hit {
            return seen;
        }
    }
}

fn agent_status_is(note: &Value, status: &str) -> bool {
    note["method"] == "subagent/updated" && note["params"]["agent"]["status"] == status
}

/// 子代理在 dock.1 上看得见：状态推 `subagent/updated`，它自己的对话包成父线程上的
/// `subagent/event`，`subagent/list` / `history` 补中途接入，`send` / `stop` 能用。
#[tokio::test]
async fn subagents_project_onto_the_thread_that_started_them() {
    let root = harness_root_with(Some(Arc::new(EchoLastUser))).await;
    root.plugin(agent_presets(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(tool_task(), TaskConfig::default())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let h = Harness::boot_on(root).await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": "live" }))
        .await;

    let tools = h.ctx.require::<Tools>(TOOLS).unwrap();
    let started = tools
        .execute(ToolCall {
            id: "call-1".into(),
            name: "task".into(),
            arguments: json!({
                "prompt": "hi kid",
                "description": "看一眼",
                "subagent_type": "explore",
                "run_in_background": true,
            })
            .to_string(),
        })
        .await;
    assert!(started.content.contains("agent_id:"), "{}", started.content);

    let notes = notes_until(&mut rpc, Duration::from_secs(5), |n| {
        agent_status_is(n, "idle")
    })
    .await;
    let idle = notes.last().unwrap()["params"]["agent"].clone();
    assert_eq!(idle["toolCallId"], "call-1");
    assert_eq!(idle["description"], "看一眼");
    assert_eq!(idle["role"], "探索");
    assert_eq!(idle["threadId"], Value::Null, "agent 里不该混进线程字段");
    let agent_id = idle["agentId"].as_str().unwrap().to_string();
    assert!(
        notes.iter().any(|n| agent_status_is(n, "running")),
        "{notes:#?}"
    );
    let child: Vec<&Value> = notes
        .iter()
        .filter(|n| n["method"] == "subagent/event")
        .inspect(|n| assert_eq!(n["params"]["agentId"], agent_id.as_str()))
        .map(|n| &n["params"]["event"])
        .collect();
    assert!(
        child.iter().any(|e| e["method"] == "item/user_message"
            && e["payload"]["content"].as_str().unwrap().contains("hi kid")),
        "子代理的任务要作为它的第一条消息推出来：{child:#?}"
    );
    assert!(child.iter().any(|e| e["method"] == "item/message_delta"));
    assert_eq!(
        child.last().unwrap()["method"],
        "turn/completed",
        "子代理停下要收掉它那一轮"
    );
    // 子代理的事件不进父线程自己的历史。
    let parent = rpc
        .call("thread/history", json!({ "threadId": "live" }))
        .await;
    assert!(
        !parent["result"]["events"].to_string().contains("hi kid"),
        "{parent}"
    );

    let listed = rpc
        .call("subagent/list", json!({ "threadId": "live" }))
        .await;
    let agents = listed["result"]["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 1, "{listed}");
    assert_eq!(agents[0]["agentId"], agent_id.as_str());
    assert_eq!(agents[0]["status"], "idle");
    assert_eq!(agents[0]["toolCalls"], 0);

    let history = rpc
        .call(
            "subagent/history",
            json!({ "threadId": "live", "agentId": agent_id }),
        )
        .await;
    let events = history["result"]["events"].as_array().unwrap();
    assert_eq!(events.len(), child.len(), "历史和推送是同一份投影");
    assert_eq!(events[0]["seq"], child[0]["seq"]);

    let sent = rpc
        .call(
            "subagent/send",
            json!({ "threadId": "live", "agentId": agent_id, "message": "again" }),
        )
        .await;
    assert_eq!(sent["result"]["ok"], true, "{sent}");
    let notes = notes_until(&mut rpc, Duration::from_secs(5), |n| {
        agent_status_is(n, "idle")
    })
    .await;
    assert!(notes.iter().any(|n| n["method"] == "subagent/event"
        && n["params"]["event"]["method"] == "item/user_message"
        && n["params"]["event"]["payload"]["content"] == "again"));

    let stopped = rpc
        .call(
            "subagent/stop",
            json!({ "threadId": "live", "agentId": agent_id }),
        )
        .await;
    assert_eq!(stopped["result"]["stopped"], true, "{stopped}");
    notes_until(&mut rpc, Duration::from_secs(5), |n| {
        n["method"] == "subagent/updated"
            && matches!(
                n["params"]["agent"]["status"].as_str(),
                Some("completed" | "cancelled")
            )
    })
    .await;
    let after = rpc
        .call(
            "subagent/send",
            json!({ "threadId": "live", "agentId": agent_id, "message": "x" }),
        )
        .await;
    assert!(
        after.get("error").is_some(),
        "收掉的子代理不能再发：{after}"
    );
    // 收掉后网关丢了它的实时投影，历史改按它的会话回放：还得看得到整段过程。
    let replayed = rpc
        .call(
            "subagent/history",
            json!({ "threadId": "live", "agentId": agent_id }),
        )
        .await;
    let replayed = replayed["result"]["events"].as_array().unwrap();
    for needle in ["hi kid", "again"] {
        assert!(
            replayed.iter().any(|e| e["method"] == "item/user_message"
                && e["payload"]["content"].as_str().unwrap().contains(needle)),
            "收掉后的历史里少了 {needle}：{replayed:#?}"
        );
    }

    let missing = rpc
        .call(
            "subagent/history",
            json!({ "threadId": "live", "agentId": "nope" }),
        )
        .await;
    assert!(missing.get("error").is_some(), "{missing}");
}

/// 设置页方法只认 `dock serve` 交给父进程的 ticket：配对来的网页改不了配置
/// （能写 MCP 启动命令 = 能在本机跑程序），受信连接能读写模型目录和白名单键。
#[tokio::test]
async fn settings_methods_only_answer_the_trusted_gui_ticket() {
    let root = harness_root().await;
    let (addr, ticket, _serve) = serve_trusted(&root).await;

    // 网页走配对拿到的 ticket：读写都挡（连接内的和另起任务的两条路都要挡）。
    let http = reqwest::Client::new();
    let created: Value = http
        .post(format!("http://{addr}/v1/pairing/requests"))
        .header("Origin", PAGE_ORIGIN)
        .json(&json!({ "application": "page" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let request_id = created["pairingRequestId"].as_str().unwrap().to_string();

    let (mut gui, auth) = Rpc::connect_as(addr, &ticket, GUI_ORIGIN).await;
    assert_eq!(auth["result"]["ok"], true, "{auth}");
    let _ = gui.call("initialize", json!({})).await;
    // GUI 在设置页里批准这个网页（`dock serve` 没有 TUI 来批）。
    let pending = gui.call("pairing/list", json!({})).await;
    assert_eq!(
        pending["result"]["pending"][0]["origin"], PAGE_ORIGIN,
        "{pending}"
    );
    let approved = gui
        .call(
            "pairing/resolve",
            json!({ "id": request_id, "approve": true }),
        )
        .await;
    assert_eq!(
        approved["result"]["bindings"][0]["origin"], PAGE_ORIGIN,
        "{approved}"
    );
    let exchanged: Value = http
        .post(format!("http://{addr}/v1/pairing/exchanges"))
        .header("Origin", PAGE_ORIGIN)
        .json(&json!({ "pairingRequestId": request_id }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let page_ticket = exchanged["ticket"].as_str().unwrap().to_string();
    let mut page = Rpc::connect(addr, &page_ticket).await;
    let _ = page.call("initialize", json!({})).await;
    for (method, params) in [
        ("config/status", json!({})),
        (
            "config/set",
            json!({ "key": "browser.headed", "value": true }),
        ),
        (
            "mcp/save",
            json!({ "server": { "name": "x", "transport": "stdio", "command": "sh" } }),
        ),
        ("model/test", json!({ "id": "anything" })),
        ("device/add", json!({ "name": "evil" })),
    ] {
        let denied = page.call(method, params).await;
        assert_eq!(
            denied["error"]["details"]["code"], "forbidden",
            "{method} 不该对配对网页开放：{denied}"
        );
    }
    // 只读的老方法照旧对网页开放。
    let listed = page.call("model/list", json!({})).await;
    assert!(listed["result"]["models"].is_array(), "{listed}");

    // 受信连接：写一条模型、设成默认、读回来、删掉。
    let id = format!("gw-settings-{}", std::process::id());
    let status = gui.call("config/status", json!({})).await;
    assert!(status["result"]["error"].is_null(), "{status}");
    let saved = gui
        .call(
            "model/save",
            json!({
                "model": {
                    "id": id,
                    "apiBaseUrl": "https://api.example.com/v1",
                    "apiBackends": ["chat_completions"],
                    "keyMode": "inline",
                    "apiKey": "sk-test-secret"
                },
                "makeDefault": true
            }),
        )
        .await;
    let row = saved["result"]["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("saved model missing: {saved}"));
    assert_eq!(row["editable"], true, "{row}");
    assert_eq!(row["auth"], "key", "{row}");
    assert_eq!(saved["result"]["configDefault"], json!(id), "{saved}");
    // 新开的页从根页抄设置：根页跟着换默认模型。
    assert_eq!(saved["result"]["default"], json!(id), "{saved}");
    let got = gui.call("model/get", json!({ "id": id })).await;
    assert_eq!(got["result"]["model"]["hasApiKey"], true, "{got}");
    assert!(
        !got.to_string().contains("sk-test-secret"),
        "密钥不能回给界面：{got}"
    );
    let bad = gui
        .call(
            "model/save",
            json!({ "model": { "id": id, "apiBaseUrl": "nope", "apiBackends": ["responses"], "keyMode": "env" } }),
        )
        .await;
    assert_eq!(bad["error"]["details"]["code"], "invalid_model", "{bad}");
    let deleted = gui.call("model/delete", json!({ "id": id })).await;
    assert!(
        !deleted["result"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == json!(id)),
        "{deleted}"
    );

    // 关掉新配对：网页再来请求直接 403。
    let closed = gui
        .call("pairing/accept", json!({ "accepting": false }))
        .await;
    assert_eq!(closed["result"]["accepting"], false, "{closed}");
    let refused = http
        .post(format!("http://{addr}/v1/pairing/requests"))
        .header("Origin", "http://localhost:6000")
        .json(&json!({ "application": "page" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
}

/// `dock serve` 挂上，回地址 + 交给父进程的受信 ticket。`ServeGuard` 活着控制循环就不退。
struct ServeGuard {
    _to_dock: tokio::io::DuplexStream,
    _task: tokio::task::JoinHandle<std::io::Result<()>>,
}

async fn serve_trusted(root: &Context) -> (SocketAddr, String, ServeGuard) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    root.plugin(
        gateway_serve("127.0.0.1:0"),
        ServeConfig {
            application: "dock-gui".into(),
            origin: GUI_ORIGIN.into(),
        },
    )
    .unwrap()
    .wait()
    .await
    .unwrap();
    let control = (*root.require::<ServeControl>(GATEWAY_SERVE).unwrap()).clone();
    let addr = control.addr();
    let (to_dock, dock_in) = tokio::io::duplex(4096);
    let (dock_out, from_dock) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move { control.run(BufReader::new(dock_in), dock_out).await });
    let line = tokio::time::timeout(
        Duration::from_secs(5),
        BufReader::new(from_dock).lines().next_line(),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    let ticket = serde_json::from_str::<Value>(&line).unwrap()["ticket"]
        .as_str()
        .unwrap()
        .to_string();
    (
        addr,
        ticket,
        ServeGuard {
            _to_dock: to_dock,
            _task: task,
        },
    )
}

/// 桌面 GUI 的进程 cwd 不是任何项目：项目里的永久插件要按会话的项目目录找。
/// 列表里看得到还没装的，开了这个项目的会话就装上；界面传的目录必须在插件根下。
#[tokio::test]
async fn project_plugins_follow_session_projects_not_the_process_cwd() {
    let root = harness_root().await;
    root.plugin(agent_presets(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(cordis_tui::tabs(), test_page_mount())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(dynamic_runner(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let (addr, ticket, _serve) = serve_trusted(&root).await;
    let (mut gui, _) = Rpc::connect_as(addr, &ticket, GUI_ORIGIN).await;
    let _ = gui.call("initialize", json!({})).await;

    let project = project_dir("plugins");
    let plugin = project.join(".dock").join("plugins").join("echo");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        "name = \"Echo\"\npurpose = \"project echo\"\nfactory = \"echo\"\nenabled = true\n",
    )
    .unwrap();

    let started = gui
        .call(
            "thread/start",
            json!({ "cwd": project.display().to_string() }),
        )
        .await;
    assert!(started["result"]["thread"]["id"].is_string(), "{started}");
    // 开页时在后台装：等它装上。
    let path = plugin.display().to_string();
    let mut row = Value::Null;
    for _ in 0..50 {
        let listed = gui.call("plugin/list", json!({})).await;
        row = listed["result"]["disk"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["path"] == json!(path))
            .cloned()
            .unwrap_or(Value::Null);
        // 装上（loaded）先于跑起来（running）：等到跑起来。
        if row["running"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(row["scope"], "project", "{row}");
    assert_eq!(
        row["project"],
        json!(project.display().to_string()),
        "{row}"
    );
    assert_eq!(row["loaded"], true, "开了这个项目的会话就该装上：{row}");
    assert_eq!(row["running"], true, "{row}");

    let off = gui
        .call("plugin/enable", json!({ "path": path, "enabled": false }))
        .await;
    let row = off["result"]["disk"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["path"] == json!(path))
        .cloned()
        .unwrap();
    assert_eq!(row["running"], false, "{row}");
    assert!(std::fs::read_to_string(plugin.join("plugin.toml"))
        .unwrap()
        .contains("enabled = false"));

    let outside = gui
        .call(
            "plugin/delete",
            json!({ "path": project.display().to_string() }),
        )
        .await;
    assert_eq!(
        outside["error"]["details"]["code"], "invalid_params",
        "{outside}"
    );
    assert!(project.exists(), "插件根之外的目录不能删");

    let deleted = gui.call("plugin/delete", json!({ "path": path })).await;
    assert!(deleted["result"]["disk"].is_array(), "{deleted}");
    assert!(!plugin.exists());
}

/// `plugin_dir` 规范化之后才比根：不存在的路径、压平后落在插件根之外的、
/// 末位分量是指向根外的符号链接的，都要拒；压平后仍在根内的写法（`..`）
/// 不能被误拒。
#[cfg(unix)] // 用到符号链接
#[tokio::test]
async fn plugin_paths_are_checked_after_canonicalization() {
    let root = harness_root().await;
    root.plugin(agent_presets(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(cordis_tui::tabs(), test_page_mount())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(dynamic_runner(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let (addr, ticket, _serve) = serve_trusted(&root).await;
    let (mut gui, _) = Rpc::connect_as(addr, &ticket, GUI_ORIGIN).await;
    let _ = gui.call("initialize", json!({})).await;

    let project = project_dir("plugin-paths");
    let plugins = project.join(".dock").join("plugins");
    let plugin = plugins.join("echo");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        "name = \"Echo\"\npurpose = \"paths\"\nfactory = \"echo\"\nenabled = true\n",
    )
    .unwrap();
    // 末位分量指向根外的符号链接：删它等于删被指的目录。
    let outside = project.join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, plugins.join("link-out")).unwrap();
    let started = gui
        .call(
            "thread/start",
            json!({ "cwd": project.display().to_string() }),
        )
        .await;
    assert!(started["result"]["thread"]["id"].is_string(), "{started}");
    // 开页在后台装插件：等它跑起来，之后的 plugin/list 才有这个根。
    let mut row = Value::Null;
    for _ in 0..50 {
        let listed = gui.call("plugin/list", json!({})).await;
        row = listed["result"]["disk"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["path"] == json!(plugin.display().to_string()))
            .cloned()
            .unwrap_or(Value::Null);
        if row["running"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(row["running"], true, "前台等装插件：{row}");

    for path in [
        // 根之外。
        project.display().to_string(),
        // 插件目录里面的文件，不是插件目录本身。
        plugin.join("plugin.toml").display().to_string(),
        // 不存在：canonicalize 失败。
        plugins.join("nope").display().to_string(),
        // `..` 压平后落在根之外。
        format!("{}/../../evil", plugins.display()),
        // 末位分量是指向根外的符号链接。
        plugins.join("link-out").display().to_string(),
    ] {
        let denied = gui.call("plugin/delete", json!({ "path": path })).await;
        assert_eq!(
            denied["error"]["details"]["code"], "invalid_params",
            "该拒：{path} -> {denied}"
        );
    }
    assert!(plugin.exists(), "插件目录不能被删掉");
    assert!(outside.exists(), "符号链接所指的根外目录不能被删掉");

    // 压平后仍在根内的写法是合法路径，不能被误拒；路径用规范化后的形式。
    let dotted = format!("{}/../plugins/echo", plugins.display());
    let deleted = gui.call("plugin/delete", json!({ "path": dotted })).await;
    assert!(deleted["result"]["disk"].is_array(), "{deleted}");
    assert!(!plugin.exists());
    let listed = gui.call("plugin/list", json!({})).await;
    assert!(
        listed["result"]["disk"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["path"] != json!(plugin.display().to_string())),
        "删掉就没有了：{listed}"
    );
}

/// 带分页服务 + 一份内存里的定时任务表。
async fn boot_with_schedules() -> Harness {
    let root = harness_root().await;
    root.plugin(agent_presets(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(cordis_tui::tabs(), test_page_mount())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.provide(cordis_spine::CRON, cordis_spine::Cron::new())
        .unwrap();
    Harness::boot_on(root).await
}

/// 开一个项目会话、说一句话（会话落盘、有了 id），回它的 id 和目录。
async fn spoken_thread(rpc: &mut Rpc, tag: &str, message: &str) -> (String, std::path::PathBuf) {
    let dir = project_dir(tag);
    let started = rpc
        .call("thread/start", json!({ "cwd": dir.display().to_string() }))
        .await;
    let id = started["result"]["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let _ = rpc
        .call("thread/subscribe", json!({ "threadId": id }))
        .await;
    let _ = rpc
        .call("turn/start", json!({ "threadId": id, "message": message }))
        .await;
    rpc.wait_notification("turn/completed", Duration::from_secs(5))
        .await;
    (id, dir)
}

/// `schedule/*`：任务记在某个会话名下（cwd 跟着会话），增删改查与校验；变动推
/// 连接级的 `schedule/changed`（不用订阅线程）。
#[tokio::test]
async fn schedules_belong_to_a_thread_and_push_changes() {
    let h = boot_with_schedules().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(init["result"]["capabilities"]["schedules"], true, "{init}");
    let (id, dir) = spoken_thread(&mut rpc, "schedule", "建个定时任务").await;

    let created = rpc
        .call(
            "schedule/create",
            json!({ "threadId": id, "interval": "10m", "prompt": "巡检一下" }),
        )
        .await;
    let task = &created["result"]["task"];
    assert_eq!(task["threadId"], json!(id), "{created}");
    assert_eq!(task["cwd"], json!(dir.display().to_string()), "{created}");
    assert_eq!(task["everySecs"], 600, "{created}");
    assert_eq!(task["heldHere"], true, "{created}");
    let task_id = task["id"].as_str().unwrap().to_string();

    for (params, code) in [
        (
            json!({ "threadId": id, "everySecs": 30, "prompt": "x" }),
            "invalid_params",
        ),
        (
            json!({ "threadId": id, "interval": "5m" }),
            "invalid_params",
        ),
        (
            json!({ "threadId": "nope", "interval": "5m", "prompt": "x" }),
            "not_found",
        ),
    ] {
        let bad = rpc.call("schedule/create", params).await;
        let got = bad["error"]["details"]["code"]
            .as_str()
            .or(bad["error"]["code"].as_i64().map(|_| "invalid_params"));
        assert_eq!(got, Some(code), "{bad}");
    }

    let listed = rpc.call("schedule/list", json!({})).await;
    let tasks = listed["result"]["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1, "{listed}");
    assert!(tasks[0]["threadTitle"].is_string(), "{listed}");

    let updated = rpc
        .call(
            "schedule/update",
            json!({ "id": task_id, "prompt": "巡检两下" }),
        )
        .await;
    assert_eq!(updated["result"]["task"]["prompt"], "巡检两下", "{updated}");

    h.ctx.emit(cordis_spine::SCHEDULE_CHANGED, ());
    let pushed = rpc
        .wait_notification("schedule/changed", Duration::from_secs(5))
        .await;
    assert!(pushed["params"].is_object(), "{pushed}");

    let deleted = rpc.call("schedule/delete", json!({ "id": task_id })).await;
    assert_eq!(deleted["result"]["deleted"], json!(task_id), "{deleted}");
    let again = rpc.call("schedule/delete", json!({ "id": task_id })).await;
    assert_eq!(again["error"]["details"]["code"], "not_found", "{again}");
}

/// 定时任务到点时会话关着：`Tabs::open_session` 在后台开页，网关照 `thread/open`
/// 的样子纳入投影——订阅时回放得出开页之前的对话，不是只有之后的事件。
#[tokio::test]
async fn sessions_opened_in_the_background_replay_their_history() {
    let h = boot_with_schedules().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let (id, dir) = spoken_thread(&mut rpc, "bg-open", "开页之前说的话").await;
    let closed = rpc.call("thread/close", json!({ "threadId": id })).await;
    assert_eq!(closed["result"]["ok"], true, "{closed}");

    let tabs = h
        .ctx
        .require::<cordis_tui::Tabs>(cordis_tui::TUI_TABS)
        .unwrap();
    tabs.open_session(&id, &dir).await.unwrap();

    let mut fresh = Rpc::connect(h.addr, &h.pair_ticket().await).await;
    let _ = fresh.call("initialize", json!({})).await;
    let sub = fresh
        .call("thread/subscribe", json!({ "threadId": id }))
        .await;
    assert!(
        sub["result"]["replayed"].as_u64().unwrap_or(0) > 0,
        "后台开的页要回放历史：{sub}"
    );
}

/// 写一个假 gh：按参数吐固定 JSON（`exit_code` 非 0 时只往 stderr 写 `stderr`）。
/// 每次调用往同目录的 `calls.log` 记一行（`$1 $2`），PR #7 的 `updatedAt` 读同目录的
/// `updated` 文件——改它就是「GitHub 上这个 PR 变了」。
fn fake_gh(dir: &std::path::Path, exit_code: i32, stderr: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(format!("gh-{exit_code}"));
    if !dir.join("updated").exists() {
        std::fs::write(dir.join("updated"), "2026-10-01T00:00:00Z").unwrap();
    }
    let d = dir.display();
    let script = format!(
        r#"#!/bin/sh
echo "$1 $2" >> "{d}/calls.log"
if [ {exit_code} -ne 0 ]; then echo '{stderr}' >&2; exit {exit_code}; fi
U=$(cat "{d}/updated")
case "$1 $2" in
  "api graphql") echo '{{"data":{{"viewer":{{"login":"me"}},"repository":{{"nameWithOwner":"acme/app","pullRequests":{{"nodes":[{{"number":7,"title":"mine","url":"https://x/7","author":{{"login":"me"}},"isDraft":false,"headRefName":"feat","baseRefName":"main","updatedAt":"'"$U"'","reviewDecision":null,"mergeable":"MERGEABLE","reviewRequests":{{"nodes":[]}},"commits":{{"nodes":[{{"commit":{{"statusCheckRollup":{{"contexts":{{"nodes":[{{"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE","name":"test"}}]}}}}}}}}]}}}},{{"number":8,"title":"theirs","url":"https://x/8","author":{{"login":"bob"}},"isDraft":true,"headRefName":"fix","baseRefName":"main","updatedAt":"2026-10-02T00:00:00Z","reviewDecision":"REVIEW_REQUIRED","mergeable":"UNKNOWN","reviewRequests":{{"nodes":[{{"requestedReviewer":{{"login":"me"}}}}]}},"commits":{{"nodes":[{{"commit":{{"statusCheckRollup":null}}}}]}}}}]}}}}}}}}' ;;
  "pr view") echo '{{"number":'"$3"',"title":"mine","body":"b","url":"https://x/7","author":{{"login":"me"}},"state":"OPEN","isDraft":false,"headRefName":"feat","baseRefName":"main","mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED","reviewDecision":"APPROVED","statusCheckRollup":[{{"status":"COMPLETED","conclusion":"FAILURE","name":"test","detailsUrl":"https://ci"}}],"additions":3,"deletions":1,"changedFiles":2,"createdAt":"c","updatedAt":"'"$U"'","reviews":[{{"author":{{"login":"bob"}},"state":"COMMENTED","submittedAt":"1"}},{{"author":{{"login":"bob"}},"state":"APPROVED","submittedAt":"2"}}],"comments":[{{}},{{}}]}}' ;;
  *) echo "unexpected: $*" >&2; exit 1 ;;
esac
"#
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// 假 gh 被调了几次 `$1 $2`（如 `api graphql`、`pr view`）。
fn gh_calls(dir: &std::path::Path, what: &str) -> usize {
    std::fs::read_to_string(dir.join("calls.log"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.trim() == what)
        .count()
}

/// `vcs/pr/*`：经 gh 列出 / 查看项目的 PR；gh 没装、没登录不是错误而是
/// `available: false` + 原因；不认识的目录不让查。列表一次 `api graphql`、按有效期缓存；
/// 详情在列表里这个 PR 没变时直接复用，变了就重拉。全放一个用例里：`DOCK_GH` 是进程级的。
#[tokio::test]
async fn pull_requests_come_from_gh_and_degrade_without_it() {
    let h = boot_with_schedules().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let init = rpc.call("initialize", json!({})).await;
    assert_eq!(
        init["result"]["capabilities"]["pullRequests"], true,
        "{init}"
    );
    let project = project_dir("pr");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    let _ = rpc
        .call(
            "thread/start",
            json!({ "cwd": project.display().to_string() }),
        )
        .await;
    let cwd = json!(project.display().to_string());
    let bin = project_dir("fake-gh");
    std::env::set_var("DOCK_GH", fake_gh(&bin, 0, ""));

    // 列表：一次 graphql 拿齐；第二次在有效期内，不再调 gh。
    let listed = rpc.call("vcs/pr/list", json!({ "cwd": cwd })).await;
    let r = &listed["result"];
    assert_eq!(r["available"], true, "{listed}");
    assert_eq!(r["cached"], false, "{listed}");
    assert!(r["fetchedAtMs"].as_u64().unwrap() > 0, "{listed}");
    assert_eq!(r["repo"], "acme/app");
    assert_eq!(r["viewer"], "me");
    let prs = r["prs"].as_array().unwrap();
    assert_eq!(prs.len(), 2, "{listed}");
    assert_eq!(prs[0]["mine"], true);
    assert_eq!(prs[0]["checks"]["state"], "failure");
    assert_eq!(prs[1]["reviewRequested"], true);
    assert_eq!(prs[1]["isDraft"], true);
    assert_eq!(prs[1]["checks"]["state"], "none");
    assert_eq!(gh_calls(&bin, "api graphql"), 1);
    let again = rpc.call("vcs/pr/list", json!({ "cwd": cwd })).await;
    assert_eq!(again["result"]["cached"], true, "{again}");
    assert_eq!(gh_calls(&bin, "api graphql"), 1, "有效期内不该再调 gh");

    // 详情：第一次拉，第二次复用。
    let got = rpc
        .call("vcs/pr/get", json!({ "cwd": cwd, "number": 7 }))
        .await;
    let pr = &got["result"]["pr"];
    assert_eq!(pr["number"], 7, "{got}");
    assert_eq!(pr["checksSummary"]["state"], "failure");
    assert_eq!(pr["comments"], 2);
    assert_eq!(
        pr["reviews"].as_array().unwrap().len(),
        1,
        "每人只留最近一次"
    );
    assert_eq!(pr["reviews"][0]["state"], "APPROVED");
    let _ = rpc
        .call("vcs/pr/get", json!({ "cwd": cwd, "number": 7 }))
        .await;
    assert_eq!(gh_calls(&bin, "pr view"), 1);

    // 刷新列表、PR 没变：详情照旧复用。
    let _ = rpc
        .call("vcs/pr/list", json!({ "cwd": cwd, "force": true }))
        .await;
    assert_eq!(gh_calls(&bin, "api graphql"), 2, "force 要绕过缓存");
    let same = rpc
        .call("vcs/pr/get", json!({ "cwd": cwd, "number": 7 }))
        .await;
    assert_eq!(same["result"]["cached"], true, "{same}");
    assert_eq!(gh_calls(&bin, "pr view"), 1, "PR 没变不该重拉详情");

    // GitHub 上 PR 变了：刷新列表看到新的 updatedAt，详情就算还在有效期内也重拉。
    std::fs::write(bin.join("updated"), "2026-10-03T00:00:00Z").unwrap();
    let _ = rpc
        .call("vcs/pr/list", json!({ "cwd": cwd, "force": true }))
        .await;
    let changed = rpc
        .call("vcs/pr/get", json!({ "cwd": cwd, "number": 7 }))
        .await;
    assert_eq!(changed["result"]["cached"], false, "{changed}");
    assert_eq!(gh_calls(&bin, "pr view"), 2, "PR 变了要重拉详情");

    std::env::set_var(
        "DOCK_GH",
        fake_gh(
            &bin,
            4,
            "To get started with GitHub CLI, please run:  gh auth login",
        ),
    );
    let unauth = rpc
        .call("vcs/pr/list", json!({ "cwd": cwd, "force": true }))
        .await;
    assert_eq!(unauth["result"]["available"], false, "{unauth}");
    assert_eq!(unauth["result"]["reason"], "gh_unauthenticated", "{unauth}");

    std::env::set_var("DOCK_GH", bin.join("not-installed"));
    let missing = rpc
        .call("vcs/pr/list", json!({ "cwd": cwd, "force": true }))
        .await;
    assert_eq!(missing["result"]["reason"], "gh_missing", "{missing}");
    assert!(
        missing["result"]["hint"]
            .as_str()
            .unwrap()
            .contains("brew install gh"),
        "{missing}"
    );
    std::env::remove_var("DOCK_GH");

    let stranger = rpc.call("vcs/pr/list", json!({ "cwd": "/" })).await;
    assert_eq!(
        stranger["error"]["details"]["code"], "invalid_params",
        "{stranger}"
    );
}
