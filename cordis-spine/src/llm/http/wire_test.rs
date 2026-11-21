//! 鉴权两条路的**线上行为**：密钥那条与订阅那条各自发什么。
//!
//! 真账号能验的只有「换码 / 刷新」那一段——官方不给没进目录的集成发套餐额度，
//! 请求这一段只能靠本地假上游钉死：
//!
//! - 订阅那条：必须带 lease 给的 Bearer，请求体按 ChatGPT 的策略改过
//!   （`store:false`、system 走 `instructions`、官方不收的字段一个都不许发）；
//! - 密钥那条：必须带自己的 key，请求体跟以前逐字节一样；
//! - 两个模型同进程并存：各自带各自的，互不串；
//! - 订阅那条按会话带 `prompt_cache_key`，同一会话同一个键；
//! - 订阅那条带会话路由头，并在一轮内回传上游给的 `x-codex-turn-state`。
//!
//! 另外用**真的** `ChatGptProvider` 验两条凭据规则：只有身份 scope 的凭据
//! 不给 lease（没套餐额度就不许推理），带套餐 scope 的才给。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use cordis::{Context, Disposable};
use cordis_base::auth::{
    AccountKey, AccountSummary, AuthError, AuthFuture, AuthProvider, Lease, LoginRequest, LoginUi,
    LogoutOutcome, ProviderInfo, RemoteModel, Retry, WirePolicy,
};
use cordis_base::types::{LlmOutput, LogEvent, PromptRequest};
use serde_json::json;

use crate::agent::turn::TurnControl;
use crate::auth::{chatgpt_policy, AuthManager, ChatGptProvider, Credential, CredentialStore};
use crate::host::settings::AppSettings;
use crate::llm::http::HttpSampler;
use crate::llm::sampler::Llm;
use crate::names::{AUTH, SESSIONS, SETTINGS, TURN};
use crate::session::log::Sessions;

/// 官方给套餐额度用的那条 scope（provider 内部常量没导出，这里按官方名字写）。
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";

/// 模型目录是**全进程**读的一份配置：`DOCK_HOME` 与当前目录都是进程级全局，
/// 用仓里那把公用锁把它们串起来（别的用例也用同一把，见 `test_env`）。
///
/// 顺带把当前目录也挪走：目录里还有一条「项目级 `.dock/config.toml`」的路径，
/// 留在仓里就会把项目配置混进这份目录。
fn isolated_env() -> (cordis_base::test_env::EnvScope, tempfile::TempDir) {
    let cwd = tempfile::tempdir().unwrap();
    let env = cordis_base::test_env::scoped().home().cwd(cwd.path());
    (env, cwd)
}

/// 一次请求的抓拍。
#[derive(Clone, Debug)]
struct Captured {
    path: String,
    authorization: Option<String>,
    /// 全部请求头，名字小写。
    headers: Vec<(String, String)>,
    body: String,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// 本地假上游：收请求、记下来、回一段最小的 SSE。
struct FakeUpstream {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Captured>>>,
}

impl FakeUpstream {
    /// 一直回 200 + 一段最小 SSE。
    async fn start() -> Self {
        Self::start_with(200, sse_body()).await
    }

    /// 回指定的状态码与响应体（用来测 4xx / 5xx 的处理）。
    async fn start_with(status: u16, body: String) -> Self {
        Self::start_seq(vec![(status, body)]).await
    }

    /// 按顺序回：第 n 个请求回第 n 条，超出的回最后一条。
    async fn start_seq(replies: Vec<(u16, String)>) -> Self {
        Self::start_full(
            replies
                .into_iter()
                .map(|(status, body)| (status, body, String::new()))
                .collect(),
        )
        .await
    }

    /// 同上，每条再带几行响应头（原样接在固定的头后面，每行以 `\r\n` 结尾）。
    async fn start_full(replies: Vec<(u16, String, String)>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let replies = Arc::new(replies);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let sink = sink.clone();
                let replies = replies.clone();
                tokio::spawn(async move {
                    let Some((head, req_body)) = read_request(&mut socket).await else {
                        return;
                    };
                    let authorization = head.lines().find_map(|line| {
                        let (k, v) = line.split_once(':')?;
                        k.trim()
                            .eq_ignore_ascii_case("authorization")
                            .then(|| v.trim().to_string())
                    });
                    let path = head
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("")
                        .to_string();
                    let headers = head
                        .lines()
                        .skip(1)
                        .filter_map(|line| {
                            let (k, v) = line.split_once(':')?;
                            Some((k.trim().to_ascii_lowercase(), v.trim().to_string()))
                        })
                        .collect();
                    let (status, body, extra) = {
                        let mut seen = sink.lock().unwrap();
                        seen.push(Captured {
                            path,
                            authorization,
                            headers,
                            body: req_body,
                        });
                        replies[(seen.len() - 1).min(replies.len() - 1)].clone()
                    };
                    let len = body.len();
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {len}\r\n{extra}\r\n{body}"
                    );
                    use tokio::io::AsyncWriteExt;
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        Self { addr, seen }
    }

    fn base(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn captured(&self) -> Vec<Captured> {
        self.seen.lock().unwrap().clone()
    }
}

/// 一段最小的 SSE 响应体。
fn sse_body() -> String {
    concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"output\":[],\"usage\":",
        "{\"input_tokens\":1,\"output_tokens\":1}}}\n\n"
    )
    .to_string()
}

/// 读一个 HTTP 请求：请求头 + 按 content-length 读完请求体。
async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let len = head
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < len {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((head, String::from_utf8_lossy(&body).to_string()))
}

/// 测试用的假 provider：发一个固定的 token，带 ChatGPT 的请求体策略，
/// 但**不接管基址**（`api_base` = None）——这样请求才会打到本地假上游。
struct FakeProvider {
    bearer: &'static str,
    policy: WirePolicy,
    /// 按 Grok 那条的规矩认错误（403 `unauthenticated:*` 算 token 失效、xAI 的
    /// 错误码）；`false` 是 ChatGPT 那条。
    grok: bool,
}

fn wire_account() -> AccountKey {
    AccountKey {
        provider: "wire-fake".into(),
        subject: "sub-1".into(),
        client_id: "oaiapp_wire".into(),
    }
}

impl AuthProvider for FakeProvider {
    fn id(&self) -> &'static str {
        "wire-fake"
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            display_name: "Wire",
            usage_url: None,
        }
    }

    fn wire_policy(&self) -> &WirePolicy {
        &self.policy
    }

    fn api_base(&self) -> Option<&str> {
        None
    }

    fn login<'a>(
        &'a self,
        _ui: &'a dyn LoginUi,
        _req: LoginRequest,
    ) -> AuthFuture<'a, Result<AccountKey, AuthError>> {
        Box::pin(async {
            Err(AuthError::NeedsLogin {
                reason: "测试里不登录".into(),
            })
        })
    }

    // 已经「登录」好的一个账号：真流程里这一步是登录时落盘的。
    fn accounts(&self) -> Vec<AccountSummary> {
        vec![AccountSummary {
            account: wire_account(),
            email: None,
            signed_in: true,
            plan_usage: true,
            expires_at: 0,
            active: true,
            problem: None,
        }]
    }

    fn active_account(&self) -> Option<AccountKey> {
        Some(wire_account())
    }

    fn set_active(&self, _account_id: &str) -> Result<AccountKey, AuthError> {
        Ok(wire_account())
    }

    fn lease<'a>(
        &'a self,
        _acct: &'a AccountKey,
        _min_valid: Duration,
    ) -> AuthFuture<'a, Result<Lease, AuthError>> {
        Box::pin(async {
            Ok(Lease {
                bearer: self.bearer.to_string(),
                expires_at: SystemTime::now() + Duration::from_secs(3600),
                generation: 1,
                account: wire_account(),
            })
        })
    }

    fn on_unauthorized<'a>(&'a self, lease: &'a Lease) -> AuthFuture<'a, Result<Retry, AuthError>> {
        Box::pin(async move {
            Ok(Retry::WithLease(Lease {
                bearer: "lease-token-refreshed".into(),
                expires_at: lease.expires_at,
                generation: lease.generation + 1,
                account: lease.account.clone(),
            }))
        })
    }

    fn logout<'a>(
        &'a self,
        _acct: &'a AccountKey,
        _revoke: bool,
    ) -> AuthFuture<'a, Result<LogoutOutcome, AuthError>> {
        Box::pin(async { Ok(LogoutOutcome::Revoked) })
    }

    fn models<'a>(
        &'a self,
        _acct: &'a AccountKey,
    ) -> AuthFuture<'a, Result<Vec<RemoteModel>, AuthError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn token_rejected(&self, status: u16, body: &[u8]) -> bool {
        if self.grok {
            crate::auth::grok_token_rejected(status, body)
        } else {
            status == 401
        }
    }

    fn classify(&self, status: u16, body: &[u8]) -> Option<AuthError> {
        // 用**真的**分类逻辑（ChatGPT：官方那套 `subscription_sharing_*` 错误码；
        // Grok：xAI 的 `{"code", "error"}`）。
        if self.grok {
            crate::auth::classify_grok_error(status, body)
        } else {
            crate::auth::classify_upstream_error(status, body)
        }
    }
}

/// 写模型目录：一个密钥模型、一个订阅模型，都指向假上游。
///
/// 写临时文件再改名：别的线程正好在读目录时，看到的要么是旧的、要么是新的，
/// 不会是半截。
fn write_catalog(base: &str) {
    write_catalog_with(base, "");
}

/// 同上，订阅那条模型再多写几行（`extra` 原样接在它的表里）。
fn write_catalog_with(base: &str, extra: &str) {
    let home = cordis_base::config::dock_home();
    let config = format!(
        r#"
[model."wire-key"]
api_base_url = "{base}"
api_key = "sk-wire-key"
api_model = "wire-key"
api_backends = ["responses"]

[model."wire-oauth"]
auth = "wire-fake"
api_base_url = "{base}"
api_model = "wire-oauth"
api_backends = ["responses"]
{extra}"#
    );
    let tmp = home.join("config.toml.tmp");
    std::fs::write(&tmp, config).unwrap();
    std::fs::rename(&tmp, home.join("config.toml")).unwrap();
}

/// 起一套 ctx：settings 指向某个模型，auth 挂上假 provider。
///
/// `Disposable` 都要活着：掉了服务就没了。
fn ctx_for(model: &str) -> (Context, Disposable, Disposable) {
    let (ctx, settings, auth, _manager) = ctx_with_manager(model);
    (ctx, settings, auth)
}

fn ctx_with_manager(model: &str) -> (Context, Disposable, Disposable, AuthManager) {
    ctx_with_provider(
        model,
        FakeProvider {
            bearer: "lease-token-xyz",
            policy: chatgpt_policy(),
            grok: false,
        },
    )
}

/// 按 Grok 那条规矩认错误的假 provider（请求体策略也是 Grok 的）。
fn grok_ctx(model: &str) -> (Context, Disposable, Disposable, AuthManager) {
    ctx_with_provider(
        model,
        FakeProvider {
            bearer: "lease-token-xyz",
            policy: crate::auth::grok_policy(),
            grok: true,
        },
    )
}

fn ctx_with_provider(
    model: &str,
    provider: FakeProvider,
) -> (Context, Disposable, Disposable, AuthManager) {
    let ctx = Context::new();
    let hold_settings = ctx
        .provide(SETTINGS, AppSettings::new(model))
        .expect("挂 settings");
    let manager = AuthManager::default();
    // 测试活到最后，provider 不卸。
    std::mem::forget(manager.register(Arc::new(provider)));
    let hold_auth = ctx.provide(AUTH, manager.clone()).expect("挂 auth");
    (ctx, hold_settings, hold_auth, manager)
}

/// 发一条请求，把模型切到 `model`。
async fn ask(ctx: &Context, model: &str) -> LlmOutput {
    if let Some(settings) = ctx.get::<AppSettings>(SETTINGS) {
        settings.set_model(model);
    }
    let sampler = HttpSampler {
        ctx: ctx.clone(),
        api_key: String::new(),
        api_base: String::new(),
        fallback_model: model.to_string(),
    };
    let llm = Llm::from_sampler(ctx.clone(), Arc::new(sampler));
    llm.stream(PromptRequest {
        system: "你是助手".into(),
        history: vec![LogEvent::User("hi".into())],
        tools: vec![],
    })
    .await
}

/// 订阅那条：带 lease 的 Bearer，请求体按 ChatGPT 的策略改过。
#[tokio::test(flavor = "multi_thread")]
async fn 订阅那条带_lease_的_bearer_并按策略改请求体() {
    let (_env, _cwd) = isolated_env();
    let upstream = FakeUpstream::start().await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth) = ctx_for("wire-oauth");

    let out = ask(&ctx, "wire-oauth").await;
    assert!(out.error.is_none(), "请求失败：{out:?}");

    let seen = upstream.captured();
    assert_eq!(seen.len(), 1, "只该有一个请求：{seen:?}");
    let req = &seen[0];
    assert_eq!(
        req.authorization.as_deref(),
        Some("Bearer lease-token-xyz"),
        "订阅那条必须带 lease 给的 token"
    );
    assert!(req.path.ends_with("/responses"), "{}", req.path);

    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
    assert_eq!(body["store"], false, "ChatGPT 那条必须 store:false");
    assert_eq!(body["stream"], true);
    assert_eq!(body["instructions"], "你是助手", "system 走 instructions");
    assert!(
        body["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["role"].as_str() != Some("system")),
        "input 里不该留 system 消息：{body}"
    );
    for field in chatgpt_policy().strip_fields {
        assert!(body.get(*field).is_none(), "{field} 不许发：{body}");
    }
}

/// 订阅那条按会话带 `prompt_cache_key`：同一会话的请求同一个键（上游按它路由，
/// 前缀缓存才有机会命中），换一个会话就换键；还没 id 的会话不发（空键会把这类
/// 会话全捆到一个键上）。
#[tokio::test(flavor = "multi_thread")]
async fn 订阅那条按会话带_prompt_cache_key() {
    let (_env, _cwd) = isolated_env();
    let upstream = FakeUpstream::start().await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth) = ctx_for("wire-oauth");
    let sessions = Sessions::new(ctx.clone());
    let _hold_sessions = ctx
        .provide(SESSIONS, sessions.clone())
        .expect("挂 sessions");

    // 还没 id：不发。
    assert!(ask(&ctx, "wire-oauth").await.error.is_none());
    sessions.assign_ephemeral_id();
    let first_id = sessions.live_session_id();
    for _ in 0..2 {
        assert!(ask(&ctx, "wire-oauth").await.error.is_none());
    }
    // 另一个会话（另一页 / 子代理各有自己的 `Sessions`）。
    let (other_ctx, _hold_settings2, _hold_auth2) = ctx_for("wire-oauth");
    let other = Sessions::new(other_ctx.clone());
    other.assign_ephemeral_id();
    let _hold_other = other_ctx
        .provide(SESSIONS, other.clone())
        .expect("挂另一个 sessions");
    assert!(ask(&other_ctx, "wire-oauth").await.error.is_none());

    let keys: Vec<Option<String>> = upstream
        .captured()
        .iter()
        .map(|req| {
            let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
            body.get("prompt_cache_key")
                .map(|k| k.as_str().expect("键是字符串").to_string())
        })
        .collect();
    assert_eq!(
        keys,
        vec![
            None,
            Some(first_id.clone()),
            Some(first_id.clone()),
            Some(other.live_session_id()),
        ],
        "同一会话同一个键、换会话换键、没 id 不发"
    );
    assert_ne!(first_id, other.live_session_id());
}

/// 订阅那条按请求头做会话路由：会话头取会话 id；上游在一轮里第一次给的
/// `x-codex-turn-state` 之后每个请求原样带回去，新一轮不带旧的。
///
/// 这条通道只认 `prompt_cache_key` 时请求随机落到别的机器上（2026-11-21 实测
/// 后续 20 步 0 次命中），缺了这两样前缀缓存就基本不中。
#[tokio::test(flavor = "multi_thread")]
async fn 订阅那条带会话头并在一轮内回传_turn_state() {
    let (_env, _cwd) = isolated_env();
    let replies = (1..=5)
        .map(|n| (200, sse_body(), format!("x-codex-turn-state: ts-{n}\r\n")))
        .collect();
    let upstream = FakeUpstream::start_full(replies).await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth) = ctx_for("wire-oauth");
    let sessions = Sessions::new(ctx.clone());
    sessions.assign_ephemeral_id();
    let sid = sessions.live_session_id();
    let _hold_sessions = ctx.provide(SESSIONS, sessions).expect("挂 sessions");
    let _hold_turn = ctx.provide(TURN, TurnControl::new()).expect("挂 turn");

    // 第一轮三步，第二轮两步。
    for _ in 0..3 {
        assert!(ask(&ctx, "wire-oauth").await.error.is_none());
    }
    ctx.require::<TurnControl>(TURN).unwrap().reset();
    for _ in 0..2 {
        assert!(ask(&ctx, "wire-oauth").await.error.is_none());
    }

    let seen = upstream.captured();
    assert_eq!(seen.len(), 5, "{seen:?}");
    for req in &seen {
        for name in ["session-id", "thread-id", "x-client-request-id"] {
            assert_eq!(req.header(name), Some(sid.as_str()), "{name} 取会话 id");
        }
    }
    let replayed: Vec<Option<&str>> = seen
        .iter()
        .map(|req| req.header("x-codex-turn-state"))
        .collect();
    assert_eq!(
        replayed,
        vec![None, Some("ts-1"), Some("ts-1"), None, Some("ts-4")],
        "一轮里只回传第一次给的；新一轮从空开始，等上游给新的"
    );
}

/// 子代理（工作流里的审查员也是）没有落盘的会话 id：用它自己的 agent id 当键和
/// 路由头。以前它们一律不带，`/code-review` 这种大头全在子代理里的会话缓存几乎
/// 不中（实测一次审查：审查员 22 次请求，按字节估算约占整段会话重发量的六成多）。
#[tokio::test(flavor = "multi_thread")]
async fn 子代理用自己的_agent_id_当会话键和路由头() {
    let (_env, _cwd) = isolated_env();
    let upstream = FakeUpstream::start().await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth) = ctx_for("wire-oauth");
    let child = Sessions::isolated_as(ctx.clone(), "agent-7f3a");
    assert!(child.live_session_id().is_empty(), "子代理没有落盘 id");
    let _hold_sessions = ctx.provide(SESSIONS, child).expect("挂 sessions");

    assert!(ask(&ctx, "wire-oauth").await.error.is_none());

    let seen = upstream.captured();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(body["prompt_cache_key"], "agent-7f3a");
    for name in ["session-id", "thread-id", "x-client-request-id"] {
        assert_eq!(seen[0].header(name), Some("agent-7f3a"), "{name}");
    }
}

/// 密钥那条：带自己的 key，请求体一个字节都不动。
#[tokio::test(flavor = "multi_thread")]
async fn 密钥那条带自己的_key_且请求体不动() {
    let (_env, _cwd) = isolated_env();
    let upstream = FakeUpstream::start().await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth) = ctx_for("wire-key");
    // 有会话 id 也不该带 `prompt_cache_key`：那是订阅通道策略里的开关。
    let sessions = Sessions::new(ctx.clone());
    sessions.assign_ephemeral_id();
    let _hold_sessions = ctx.provide(SESSIONS, sessions).expect("挂 sessions");

    let out = ask(&ctx, "wire-key").await;
    assert!(out.error.is_none(), "请求失败：{out:?}");

    let seen = upstream.captured();
    assert_eq!(seen.len(), 1, "只该有一个请求：{seen:?}");
    let req = &seen[0];
    assert_eq!(
        req.authorization.as_deref(),
        Some("Bearer sk-wire-key"),
        "密钥那条必须带配置里的 key"
    );

    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
    assert!(
        body.get("store").is_none(),
        "没走订阅通道的模型不该多出 store：{body}"
    );
    assert!(
        body.get("instructions").is_none(),
        "system 不该被挪走：{body}"
    );
    assert!(
        body.get("prompt_cache_key").is_none(),
        "别家不一定认 prompt_cache_key，多一个字段可能 400：{body}"
    );
    for name in [
        "session-id",
        "thread-id",
        "x-client-request-id",
        "x-codex-turn-state",
    ] {
        assert!(req.header(name).is_none(), "密钥那条不该多出 {name} 头");
    }
    assert!(
        body["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["role"].as_str() == Some("system")),
        "system 消息该原样留在 input 里：{body}"
    );
}

/// 两个模型同进程并存：各带各的，互不串。
#[tokio::test(flavor = "multi_thread")]
async fn 两个模型并存时各自带各自的鉴权() {
    let (_env, _cwd) = isolated_env();
    let upstream = FakeUpstream::start().await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth) = ctx_for("wire-key");

    assert!(ask(&ctx, "wire-key").await.error.is_none());
    assert!(ask(&ctx, "wire-oauth").await.error.is_none());

    let seen = upstream.captured();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let auths: Vec<_> = seen
        .iter()
        .filter_map(|r| r.authorization.clone())
        .collect();
    assert!(
        auths.contains(&"Bearer sk-wire-key".to_string()),
        "{auths:?}"
    );
    assert!(
        auths.contains(&"Bearer lease-token-xyz".to_string()),
        "{auths:?}"
    );
}

/// 订阅那条遇到 429：**只请求一次**（重试只是再撞一次墙），并给一句中文。
#[tokio::test(flavor = "multi_thread")]
async fn 订阅那条遇到_429_只请求一次_并给出中文提示() {
    let (_env, _cwd) = isolated_env();
    let body = json!({
        "error": {
            "code": "subscription_sharing_usage_limit_exceeded",
            "message": "usage limit exceeded",
        }
    })
    .to_string();
    let upstream = FakeUpstream::start_with(429, body).await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth) = ctx_for("wire-oauth");

    let out = ask(&ctx, "wire-oauth").await;
    let err = out.error.expect("429 该报错");
    assert!(err.contains("用量到上限"), "该说用量到上限：{err}");
    assert_eq!(
        upstream.captured().len(),
        1,
        "429 不能重试：额度用尽再发一次还是撞墙"
    );
}

/// 401：换一份 token 重试一次；换不到就报「要重新登录」。
#[tokio::test(flavor = "multi_thread")]
async fn 订阅那条遇到_401_会换一次_token_再重试() {
    let (_env, _cwd) = isolated_env();
    // 第一次回 401，第二次回 200。
    let upstream = FakeUpstream::start_with(401, "{}".into()).await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth) = ctx_for("wire-oauth");

    let out = ask(&ctx, "wire-oauth").await;
    let err = out.error.expect("401 该报错");
    assert!(err.contains("登录"), "该提示重新登录：{err}");
    // 换过一次 token 就该收手：只发两轮（401 + 重试）。
    let seen = upstream.captured();
    assert_eq!(seen.len(), 2, "401 只该换一次 token 再重试");
    assert_eq!(
        seen[0].authorization.as_deref(),
        Some("Bearer lease-token-xyz"),
        "第一次带的是原来的 token"
    );
    assert_eq!(
        seen[1].authorization.as_deref(),
        Some("Bearer lease-token-refreshed"),
        "重试要带换过的那份"
    );
}

/// 启动时目录是空的（新装、还没配模型）：`llm` 先用离线的假采样器；之后目录里有了
/// 能连的模型（登录了订阅、设置里加了模型），下一次采样就走 HTTP，不用重启。
/// 改之前挂载时就定死了假采样器：首次 `/auth login` 完收到的还是假回复。
#[tokio::test(flavor = "multi_thread")]
async fn 启动时没有模型_之后加了就走_http() {
    let cwd = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped()
        .home()
        .cwd(cwd.path())
        .remove("DOCK_API_KEY")
        .remove("OPENAI_API_KEY")
        .remove("XAI_API_KEY")
        .remove("DOCK_MODEL");
    let ctx = Context::new();
    let _hold_settings = ctx
        .provide(SETTINGS, AppSettings::new("wire-key"))
        .expect("挂 settings");
    let config = crate::llm::sampler::LlmConfig::from_env();
    assert_eq!(
        config.mode,
        crate::llm::sampler::LlmMode::Workspace,
        "目录是空的：启动时该是离线模式"
    );
    ctx.plugin(crate::llm::sampler::llm(), config)
        .unwrap()
        .wait()
        .await
        .unwrap();
    let llm = ctx.get::<Llm>(crate::names::LLM).expect("llm");
    let request = || PromptRequest {
        system: "你是助手".into(),
        history: vec![LogEvent::User("hi".into())],
        tools: vec![],
    };

    let upstream = FakeUpstream::start().await;
    let offline = llm.stream(request()).await;
    assert!(upstream.captured().is_empty(), "目录还空着，不该发请求");
    assert!(offline.error.is_none(), "{offline:?}");

    // 有了能连的模型（这里是密钥模型；订阅模型登录后写进 auth_models.json，一样算）。
    write_catalog(&upstream.base());
    let online = llm.stream(request()).await;
    assert!(online.error.is_none(), "{online:?}");
    assert_ne!(online.text, offline.text, "不该还是离线的假回复");
    let seen = upstream.captured();
    assert_eq!(seen.len(), 1, "该走 HTTP 了");
    assert_eq!(seen[0].authorization.as_deref(), Some("Bearer sk-wire-key"));
}

/// Grok：校验不过的 token 回 403 `unauthenticated:bad-credentials`（不是 401）——
/// 也要换一份 token 重发一次。改之前只认 401，这种 403 直接报错、不刷新。
#[tokio::test(flavor = "multi_thread")]
async fn grok_那条_token_失效的_403_也换一次_token_再重试() {
    let (_env, _cwd) = isolated_env();
    let rejected = json!({
        "code": "unauthenticated:bad-credentials",
        "error": "The OAuth2 access token could not be validated.",
    })
    .to_string();
    let upstream = FakeUpstream::start_seq(vec![(403, rejected), (200, sse_body())]).await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth, _manager) = grok_ctx("wire-oauth");

    let out = ask(&ctx, "wire-oauth").await;
    assert!(out.error.is_none(), "换过 token 该成功：{out:?}");
    let seen = upstream.captured();
    assert_eq!(seen.len(), 2, "403 + 换 token 重发一次：{seen:?}");
    assert_eq!(
        seen[1].authorization.as_deref(),
        Some("Bearer lease-token-refreshed"),
        "重发要带换过的那份"
    );
}

/// Grok：套餐档位不够的 403 不是 token 的事——不刷新、不重发，把原因说出来。
#[tokio::test(flavor = "multi_thread")]
async fn grok_那条档位不够的_403_不重发() {
    let (_env, _cwd) = isolated_env();
    let rejected = json!({
        "code": "permission-denied",
        "error": "The caller does not have permission to execute the specified operation.",
    })
    .to_string();
    let upstream = FakeUpstream::start_with(403, rejected).await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth, _manager) = grok_ctx("wire-oauth");

    let err = ask(&ctx, "wire-oauth").await.error.expect("该报错");
    assert!(err.contains("订阅档位"), "{err}");
    assert!(err.contains("does not have permission"), "{err}");
    assert_eq!(upstream.captured().len(), 1, "档位不够重发也没用");
}

/// Grok：模型不收 `reasoning.effort`（grok-4.20 那几个），去掉它重发一次，下一轮
/// 一开始就不发。请求体里带 `prompt_cache_key`（缓存路由靠它）。
#[tokio::test(flavor = "multi_thread")]
async fn grok_那条模型不收_reasoning_effort_去掉再发() {
    let (_env, _cwd) = isolated_env();
    let rejected = json!({
        "code": "invalid-argument",
        "error": "Model grok-4.20-0309-reasoning does not support parameter reasoningEffort.",
    })
    .to_string();
    let upstream = FakeUpstream::start_seq(vec![(400, rejected), (200, sse_body())]).await;
    write_catalog_with(
        &upstream.base(),
        "reasoning = true\nreasoning_effort = \"high\"\n",
    );
    let (ctx, _hold_settings, _hold_auth, manager) = grok_ctx("wire-oauth");

    let out = ask(&ctx, "wire-oauth").await;
    assert!(out.error.is_none(), "去掉 effort 后该成功：{out:?}");
    let seen = upstream.captured();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let first: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
    let second: serde_json::Value = serde_json::from_str(&seen[1].body).unwrap();
    assert_eq!(first["reasoning"]["effort"], "high", "{first}");
    assert!(second["reasoning"].get("effort").is_none(), "{second}");
    assert_eq!(
        manager.unsupported_params("wire-fake", "wire-oauth"),
        vec!["reasoning.effort".to_string()]
    );
}

/// 400 `…_unsupported_capability`：按 `error.param` 去掉那一项**再发一次**（官方），
/// 第二次的请求体里没有它；学到的字段下一轮一开始就不发。
#[tokio::test(flavor = "multi_thread")]
async fn 订阅那条遇到_unsupported_capability_去掉字段再发() {
    let (_env, _cwd) = isolated_env();
    let rejected = json!({
        "error": {
            "code": "subscription_sharing_unsupported_capability",
            "param": "reasoning.summary",
        }
    })
    .to_string();
    let upstream = FakeUpstream::start_seq(vec![(400, rejected), (200, sse_body())]).await;
    write_catalog_with(
        &upstream.base(),
        "reasoning = true\nreasoning_effort = \"high\"\n",
    );
    let (ctx, _hold_settings, _hold_auth, manager) = ctx_with_manager("wire-oauth");

    let out = ask(&ctx, "wire-oauth").await;
    assert!(out.error.is_none(), "去掉字段后该成功：{out:?}");
    let seen = upstream.captured();
    assert_eq!(seen.len(), 2, "去掉之后只重发一次：{seen:?}");
    let first: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
    let second: serde_json::Value = serde_json::from_str(&seen[1].body).unwrap();
    assert!(first["reasoning"].get("summary").is_some(), "{first}");
    assert!(second["reasoning"].get("summary").is_none(), "{second}");
    assert_ne!(seen[0].body, seen[1].body, "绝不原样重发");
    assert_eq!(
        manager.unsupported_params("wire-fake", "wire-oauth"),
        vec!["reasoning.summary".to_string()]
    );

    // 下一轮：一开始就不发那个字段。
    assert!(ask(&ctx, "wire-oauth").await.error.is_none());
    let seen = upstream.captured();
    assert_eq!(seen.len(), 3);
    let third: serde_json::Value = serde_json::from_str(&seen[2].body).unwrap();
    assert!(third["reasoning"].get("summary").is_none(), "{third}");
}

/// 上游说不收的东西请求体里根本没有（去不掉）：**不重发**，直接把那句话报出来。
#[tokio::test(flavor = "multi_thread")]
async fn 去不掉的字段不重发() {
    let (_env, _cwd) = isolated_env();
    let rejected = json!({
        "error": {
            "code": "subscription_sharing_unsupported_capability",
            "param": "service_tier",
        }
    })
    .to_string();
    let upstream = FakeUpstream::start_with(400, rejected).await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth) = ctx_for("wire-oauth");

    let err = ask(&ctx, "wire-oauth").await.error.expect("该报错");
    assert!(err.contains("service_tier"), "{err}");
    assert!(!err.contains("已去掉"), "{err}");
    assert_eq!(upstream.captured().len(), 1, "同一个请求体不许再发");
}

/// 没走订阅通道的模型：503 重试 3 次仍失败，报错与重构前**逐字一致**——
/// `LLM HTTP {status}` + 上游原文（审阅 #282 第 3 条问过这条路径变没变）。
#[tokio::test(flavor = "multi_thread")]
async fn 密钥那条连续_503_的报错不变() {
    let (_env, _cwd) = isolated_env();
    let upstream = FakeUpstream::start_with(503, "busy".into()).await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth) = ctx_for("wire-key");

    let err = ask(&ctx, "wire-key").await.error.expect("该报错");
    assert_eq!(err, "LLM HTTP 503 Service Unavailable\nbusy");
    assert_eq!(upstream.captured().len(), 3, "503 照旧最多发 3 次");
}

/// 同一模型上「订阅 + 密钥」同时写 = 配置错误，不是静默挑一个。
#[test]
fn 订阅和密钥同时写是配置错误() {
    let (_env, _cwd) = isolated_env();
    let home = cordis_base::config::dock_home();
    let config = r#"
[model."wire-both"]
auth = "wire-fake"
api_key = "sk-wire-key"
api_base_url = "http://127.0.0.1:1/v1"
"#;
    let tmp = home.join("config.toml.tmp");
    std::fs::write(&tmp, config).unwrap();
    std::fs::rename(&tmp, home.join("config.toml")).unwrap();
    let choice = cordis_base::config::lookup_model("wire-both").expect("模型在目录里");
    let conflict = choice.auth_conflict().expect("应当报冲突");
    assert!(conflict.contains("auth"), "{conflict}");
    assert!(conflict.contains("api_key"), "{conflict}");
}

/// 只有身份 scope 的凭据不给 lease：没套餐额度就不许推理。
#[tokio::test(flavor = "multi_thread")]
async fn 只有身份scope的凭据不给_lease() {
    let store = CredentialStore::new(tempfile::tempdir().unwrap().keep());
    let cred = credential(
        "oaiapp_identity",
        &["openid", "profile", "email", "offline_access"],
    );
    store.save(&cred).unwrap();
    let provider = ChatGptProvider::new(store);
    let acct = AccountKey {
        provider: "chatgpt".into(),
        subject: cred.subject.clone(),
        client_id: cred.client_id.clone(),
    };
    let err = provider
        .lease(&acct, Duration::from_secs(120))
        .await
        .expect_err("没有套餐额度就不该给出 lease");
    assert!(matches!(err, AuthError::NotEligible { .. }), "{err:?}");
    assert_eq!(err.code(), "missing_plan_usage_scope");
}

/// 带套餐 scope 的凭据才给 lease，而且给的就是落盘那个 access token。
#[tokio::test(flavor = "multi_thread")]
async fn 带套餐scope的凭据给出_lease() {
    let store = CredentialStore::new(tempfile::tempdir().unwrap().keep());
    let cred = credential(
        "oaiapp_plan",
        &["openid", "profile", "email", "offline_access", PLAN_SCOPE],
    );
    store.save(&cred).unwrap();
    let provider = ChatGptProvider::new(store);
    let acct = AccountKey {
        provider: "chatgpt".into(),
        subject: cred.subject.clone(),
        client_id: cred.client_id.clone(),
    };
    let lease = provider
        .lease(&acct, Duration::from_secs(120))
        .await
        .expect("有套餐额度就该给 lease");
    assert_eq!(lease.bearer, "access-token-xyz");
    assert_eq!(lease.generation, 1);
    assert_eq!(lease.account.client_id, "oaiapp_plan");
}

fn credential(client_id: &str, scopes: &[&str]) -> Credential {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    Credential {
        schema_version: 1,
        generation: 1,
        email: Some("a@b.c".into()),
        issuer: "https://auth.openai.com".into(),
        subject: "sub-1".into(),
        client_id: client_id.into(),
        id_token: "id-token".into(),
        access_token: "access-token-xyz".into(),
        refresh_token: "refresh-token-xyz".into(),
        expires_at: now + 3600,
        scopes: scopes.iter().map(|s| s.to_string()).collect(),
        saved_at: now,
        ext_agent_host_id: "urn:uuid:test".into(),
    }
}
