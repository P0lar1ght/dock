//! Origin pairing, dock.1 handshake, turn projection, permission dual-resolve.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use cordis_gateway::{
    gateway_bind, gateway_idle, gateway_serve, ServeConfig, ServeControl, GATEWAY, GATEWAY_SERVE,
    PROTOCOL_VERSION,
};
use cordis_spine::{
    agent_loop, agent_presets, install_fakes, mcp_client, permissions, plan_mode, settings, slash,
    tool_ask_user, tool_goal, turn, AgentPresets, AppSettings, ExtraSlashKind, Goal, LogEvent,
    LoopHandle, PermissionOptionKind, Permissions, PlanMode, Sessions, Slash, SlashEntry,
    TurnControl, AGENT_LOOP, AGENT_PRESETS, GOAL, PERMISSIONS, PLAN_MODE, SESSIONS, SETTINGS,
    SLASH, TURN,
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

    fn compact(&self, _context: String) {}

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
    isolated_home();
    let root = Context::new();
    install_fakes(&root).await.unwrap();
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
        root.plugin(gateway_bind("127.0.0.1:0"), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
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
        let url = format!("ws://{addr}/api/ws");
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert(ORIGIN_HEADER, origin.parse().unwrap());
        let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        let (write, read) = ws.split();
        let mut rpc = Self {
            write,
            read,
            next_id: 1,
        };
        let auth = rpc
            .call("connection/authenticate", json!({ "ticket": ticket }))
            .await;
        (rpc, auth)
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
async fn mcp_reload_and_model_refresh_are_implemented() {
    let h = Harness::boot().await;
    let ticket = h.pair_ticket().await;
    let mut rpc = Rpc::connect(h.addr, &ticket).await;
    let _ = rpc.call("initialize", json!({})).await;
    let reload = rpc.call("mcp/reload", json!({})).await;
    assert_eq!(reload["result"]["ok"], true);
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
    let refused = rpc
        .call("turn/start", json!({ "threadId": id, "message": "x" }))
        .await;
    assert_eq!(refused["error"]["details"]["code"], "thread_not_open");
    let live_close = rpc
        .call("thread/close", json!({ "threadId": "live" }))
        .await;
    assert!(
        live_close.get("error").is_some(),
        "第 1 页关不掉：{live_close}"
    );

    let reopened = rpc.call("thread/open", json!({ "threadId": id })).await;
    assert_eq!(reopened["result"]["thread"]["id"], json!(id), "{reopened}");
    let history = rpc.call("thread/history", json!({ "threadId": id })).await;
    assert!(history.to_string().contains("A 页的话"), "{history}");

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
