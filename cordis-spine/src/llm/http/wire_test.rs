//! 鉴权两条路的**线上行为**：密钥那条与订阅那条各自发什么。
//!
//! 真账号能验的只有「换码 / 刷新」那一段——官方不给没进目录的集成发套餐额度，
//! 请求这一段只能靠本地假上游钉死：
//!
//! - 订阅那条：必须带 lease 给的 Bearer，请求体按 ChatGPT 的策略改过
//!   （`store:false`、system 走 `instructions`、官方不收的字段一个都不许发）；
//! - 密钥那条：必须带自己的 key，请求体跟以前逐字节一样；
//! - 两个模型同进程并存：各自带各自的，互不串。
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

use crate::auth::{chatgpt_policy, AuthManager, ChatGptProvider, Credential, CredentialStore};
use crate::host::settings::AppSettings;
use crate::llm::http::HttpSampler;
use crate::llm::sampler::Llm;
use crate::names::{AUTH, SETTINGS};

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
    body: String,
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
                    let (status, body) = {
                        let mut seen = sink.lock().unwrap();
                        seen.push(Captured {
                            path,
                            authorization,
                            body: req_body,
                        });
                        replies[(seen.len() - 1).min(replies.len() - 1)].clone()
                    };
                    let len = body.len();
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {len}\r\n\r\n{body}"
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

    fn classify(&self, status: u16, body: &[u8]) -> Option<AuthError> {
        // 用**真的**分类逻辑（官方那套 `subscription_sharing_*` 错误码）。
        crate::auth::classify_upstream_error(status, body)
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
    let ctx = Context::new();
    let hold_settings = ctx
        .provide(SETTINGS, AppSettings::new(model))
        .expect("挂 settings");
    let manager = AuthManager::default();
    // 测试活到最后，provider 不卸。
    std::mem::forget(manager.register(Arc::new(FakeProvider {
        bearer: "lease-token-xyz",
        policy: chatgpt_policy(),
    })));
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

/// 密钥那条：带自己的 key，请求体一个字节都不动。
#[tokio::test(flavor = "multi_thread")]
async fn 密钥那条带自己的_key_且请求体不动() {
    let (_env, _cwd) = isolated_env();
    let upstream = FakeUpstream::start().await;
    write_catalog(&upstream.base());
    let (ctx, _hold_settings, _hold_auth) = ctx_for("wire-key");

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
