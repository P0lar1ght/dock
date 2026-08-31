//! Origin pairing, dock.1 handshake, turn projection, permission dual-resolve.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use cordis_gateway::{gateway_bind, GATEWAY, PROTOCOL_VERSION};
use cordis_spine::{
    agent_loop, install_fakes, mcp_client, permissions, plan_mode, settings, slash, tool_ask_user,
    tool_goal, turn, AppSettings, ExtraSlashKind, Goal, LogEvent, LoopHandle, PermissionOptionKind,
    Permissions, PlanMode, Sessions, Slash, SlashEntry, TurnControl, AGENT_LOOP, GOAL, PERMISSIONS,
    PLAN_MODE, SESSIONS, SETTINGS, SLASH, TURN,
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

impl Harness {
    async fn boot() -> Self {
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
        root.plugin(gateway_bind("127.0.0.1:0"), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let addr = root
            .require::<cordis_tui::GatewayRef>(GATEWAY)
            .unwrap()
            .local_addr();
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
        let url = format!("ws://{addr}/api/ws");
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert(ORIGIN_HEADER, PAGE_ORIGIN.parse().unwrap());
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
        assert_eq!(auth["result"]["ok"], true);
        rpc
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
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
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

    let terminal = rpc.call("slash/execute", json!({ "text": "/pair" })).await;
    assert_eq!(terminal["result"]["kind"], "notice");
    assert!(terminal["result"]["notice"]["body"]
        .as_str()
        .unwrap()
        .contains("终端"));

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
