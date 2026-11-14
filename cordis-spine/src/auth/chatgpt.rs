//! ChatGPT 订阅通道（Sign in with ChatGPT）的 provider 插件。
//!
//! 走的是 OpenAI 给开源工具开的官方通道：本地跑 OAuth，拿到短期 access token，
//! 打 `https://api.openai.com/v1/responses`。官方文档：
//! <https://developers.openai.com/siwc/token-sharing-open-source/sign-in>。
//!
//! **不做的事**（官方明令禁止，做了就是冒充）：不打 `chatgpt.com/backend-api`，
//! 不复用 Codex 的 `client_id`，不冒充官方客户端。
//!
//! 流程：本地起一个 127.0.0.1 的回环监听 → 打开授权页 → 用户在浏览器里登录并
//! 同意「用 ChatGPT 套餐」→ 回调带 code 回来 → 换 token → 落盘。
//! 之后每次推理只取一份短期 access token（`Lease`），refresh token 不出这里。

use std::sync::Arc;
use std::time::Duration;

use cordis::{plugin, Inject, Plugin};
use serde_json::{json, Value};

use cordis_base::auth::{
    AccountKey, AccountSummary, AuthError, AuthFuture, AuthProvider, Lease, LoginRequest,
    LoginTarget, LoginUi, LogoutOutcome, ProviderInfo, RemoteModel, Retry, ScopeMode, WirePolicy,
};

use crate::auth::credentials::{
    load_or_create_host_id, now_unix, Credential, CredentialError, CredentialStore,
};
use crate::auth::jwt::verify_id_token;
use crate::auth::oauth::{self, FileLock};
use crate::auth::{chatgpt_policy, manager::AuthManager};
use crate::names::AUTH;

/// provider id：模型条目写 `auth = "chatgpt"`。
pub const PROVIDER_ID: &str = "chatgpt";
/// 订阅额度那一条 scope。没有它就只是「登录了」，不能拿套餐额度推理。
pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";

pub const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
pub const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
pub const ISSUER: &str = "https://auth.openai.com";
pub const DISCOVERY_URL: &str = "https://auth.openai.com/.well-known/openid-configuration";
/// 推理基址。**只能打这里**，不能打 Codex 的内部后端。
pub const API_BASE: &str = "https://api.openai.com/v1";
/// 授权 / 换码 / 刷新都要带 resource。
pub const RESOURCE: &str = "https://api.openai.com/v1";
/// 用量页（官方 UI 规范里的「Manage usage」）。
pub const USAGE_URL: &str = "https://chatgpt.com/settings/usage";
/// 启动时模型列表超过这么久才重拉（登录、切账号时总是重拉）。
const MODELS_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// 首登用的动态 client（没有 secret、没有合作方 key）。
pub const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
/// 授权页上显示的名字——用 Dock 自己的名字，不冒充别家。
pub const AGENT_NAME: &str = "Dock";
/// 要**套餐额度**时的 scope（官方推荐的那一套）。
pub const SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "offline_access",
    "resource.invoke",
    PLAN_SCOPE,
];

/// **只要身份**的 scope：不给套餐额度。
///
/// 官方目录里有一类 app 就是「sign-in only」（GitLab、Canva……）。Dock 现在也
/// 可能落在这一类：套餐额度只开放给目录里的集成（2026-11-14 实测：目录里的
/// 开源集成只有 OpenClaw / OpenCode / Pi / T3），但**身份登录人人可用**。
/// 走这条 scope 能把 OAuth、验签、落盘、刷新整条链路跑通。
pub const IDENTITY_SCOPES: &[&str] = &["openid", "profile", "email", "offline_access"];

/// scope 集合（按模式）。
fn scopes_for(mode: ScopeMode) -> &'static [&'static str] {
    match mode {
        ScopeMode::Default => SCOPES,
        ScopeMode::IdentityOnly => IDENTITY_SCOPES,
    }
}

/// 等用户完成授权的上限。
const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
/// token / 撤销端点的**总超时**。只设连接超时的话，上游挂住时请求会一直挂着。
const TOKEN_TIMEOUT: Duration = Duration::from_secs(30);
/// 回环回调的路径。只有这个路径算授权回调。
const CALLBACK_PATH: &str = "/auth/callback";
/// 刷新时最多等锁多久。
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// 撤销最多试几次（官方：网络失败 / 5xx 退避重试）。
const REVOKE_ATTEMPTS: u32 = 3;

/// 刷新时这些错误码 = **这份凭据不能再用了**（官方「Refresh errors」那一节）：
/// 清掉 token、留下注册，再用保存的 client id 重新走一次 OAuth。不是普通重试。
const DEAD_REFRESH_CODES: &[&str] = &[
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
    "invalid_token",
];

/// 官方端点。生产是常量；测试指到本地假服务器（要数「刷了几次」「撤销试了几次」）。
#[derive(Clone, Debug)]
struct Endpoints {
    authorize: String,
    token: String,
    discovery: String,
    api: String,
}

impl Endpoints {
    fn official() -> Self {
        Self {
            authorize: AUTHORIZE_URL.into(),
            token: TOKEN_URL.into(),
            discovery: DISCOVERY_URL.into(),
            api: API_BASE.into(),
        }
    }
}

/// ChatGPT 订阅通道的实现。
pub struct ChatGptProvider {
    store: CredentialStore,
    policy: WirePolicy,
    client: reqwest::Client,
    endpoints: Endpoints,
}

/// token 端点回来的错误。
///
/// 分成「上游明确拒绝」与「传输层失败」两类：前者带 OAuth 错误码
/// （`invalid_grant` / `refresh_token_reused` / …），得按「要重新登录」处理；
/// 后者只是网络抖了一下，重试可能有用。
#[derive(Debug)]
enum TokenError {
    /// 上游拒绝（或响应缺字段）：错误码 + 说明 + **脱敏过的** body。
    OAuth {
        code: String,
        detail: String,
        status: u16,
        /// 错误响应里常有线索（哪个字段不对）。凡是名字带 token 的值都已抹掉；
        /// 成功响应缺字段时这里是空的——那份 body 里就是 token 本身。
        body: String,
    },
    /// 传输层失败（连不上、超时）。
    Transport(String),
}

impl TokenError {
    /// OAuth 错误码（不是 OAuth 类错误就空串）。
    fn code(&self) -> &str {
        match self {
            TokenError::OAuth { code, .. } => code,
            TokenError::Transport(_) => "",
        }
    }

    /// 这份凭据是不是不能再用了（见 [`DEAD_REFRESH_CODES`]）。
    fn is_credential_dead(&self) -> bool {
        DEAD_REFRESH_CODES.contains(&self.code())
    }

    /// 给用户看的一句中文。
    fn message(&self) -> String {
        match self {
            TokenError::OAuth {
                code,
                detail,
                status,
                body,
            } => {
                if body.is_empty() {
                    format!("token HTTP {status}：{detail}（{code}）")
                } else {
                    format!("token HTTP {status}：{detail}（{code}）｜body: {body}")
                }
            }
            TokenError::Transport(detail) => detail.clone(),
        }
    }
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl From<TokenError> for AuthError {
    fn from(e: TokenError) -> AuthError {
        if e.is_credential_dead() {
            AuthError::NeedsLogin {
                reason: format!(
                    "凭据作废了（{}）：/auth login 重新登录（会复用这个账号原来的注册）。",
                    e.code()
                ),
            }
        } else if e.code() == "invalid_client" {
            // 官方：invalid_client = 改 client 配置，不是重新登录能解决的。
            AuthError::Rejected {
                code: "invalid_client".into(),
                detail: format!("client 配置不对：{}", e.message()),
            }
        } else {
            AuthError::Transport(e.message())
        }
    }
}

impl ChatGptProvider {
    pub fn new(store: CredentialStore) -> Self {
        Self {
            store,
            policy: chatgpt_policy(),
            client: reqwest::Client::builder()
                .user_agent(concat!("dock/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(20))
                .build()
                .expect("dock user-agent is a valid header value"),
            endpoints: Endpoints::official(),
        }
    }

    /// 把 token / discovery / 推理端点指到 `base` 下的假服务器（**只给测试用**）。
    #[cfg(test)]
    fn with_test_server(mut self, base: &str) -> Self {
        self.endpoints = Endpoints {
            authorize: format!("{base}/authorize"),
            token: format!("{base}/token"),
            discovery: format!("{base}/.well-known/openid-configuration"),
            api: format!("{base}/v1"),
        };
        self
    }

    /// 换码 / 刷新用的一把钥匙。
    ///
    /// **总超时**：只设连接超时的话，上游挂住时这个请求会一直挂着——刷新拖久了
    /// 正好撞上「refresh token 已被别处用掉」（`refresh_token_reused`）。
    async fn token_request(&self, form: &[(&str, &str)]) -> Result<Value, TokenError> {
        let resp = self
            .client
            .post(&self.endpoints.token)
            .timeout(TOKEN_TIMEOUT)
            .form(form)
            .send()
            .await
            .map_err(|e| TokenError::Transport(format!("token 请求失败：{e}")))?;
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(json!({}));
        if !status.is_success() {
            // OAuth 错误码单独拎出来：`invalid_grant` / `refresh_token_reused`
            // 意味着「这份凭据不能再用了」，跟「网络抖了一下」不是一回事。
            let code = v
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .to_string();
            let detail = v
                .get("error_description")
                .or_else(|| v.get("error"))
                .and_then(|e| e.as_str())
                .unwrap_or("换码失败")
                .to_string();
            return Err(TokenError::OAuth {
                code,
                detail,
                status: status.as_u16(),
                body: redact_tokens(&v).to_string(),
            });
        }
        Ok(v)
    }

    /// 首登 / 重新授权那次的 token 响应：三样都要（`id_token` 认账号用）。
    ///
    /// **缺字段的报错里不放 body**：那是一份成功响应，里面就是 token。
    fn exchange_parts(v: &Value) -> Result<(String, String, String), TokenError> {
        let access = required_field(v, "access_token")?;
        let refresh = required_field(v, "refresh_token")?;
        let id_token = required_field(v, "id_token")?;
        Ok((access, refresh, id_token))
    }

    /// 官方 client id：token 响应给的 `oaiapp_…`，回调给的，或首登的动态 client。
    fn official_client_id(v: &Value, from_callback: Option<&str>) -> String {
        v.get("client_id")
            .and_then(|c| c.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| from_callback.map(str::to_string))
            .unwrap_or_else(|| DYNAMIC_CLIENT_ID.to_string())
    }

    /// scope 列表。优先用响应里的 `scope`，否则从 ID token 的声明里取。
    fn scopes_of(v: &Value, claims_scope: Option<&str>) -> Vec<String> {
        let raw = v
            .get("scope")
            .and_then(|s| s.as_str())
            .or(claims_scope)
            .unwrap_or("");
        raw.split_whitespace()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// 换码：授权码 → token。
    ///
    /// `client_id` 用**回调发回来的正式 client id**（首登时服务端刚分配的那个）。
    /// 用动态 client 会拿到 400 `invalid_client`——官方流程就是这么规定的。
    async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
        client_id: &str,
    ) -> Result<Value, TokenError> {
        self.token_request(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", verifier),
            ("resource", RESOURCE),
        ])
        .await
    }

    /// 刷新：refresh token → 新 token。**refresh token 每次刷新都轮换**，旧的作废。
    ///
    /// 官方：成功后 access token、过期时间、scope、轮换后的 refresh token **一起换**。
    async fn refresh(&self, cred: &Credential) -> Result<Credential, TokenError> {
        let v = self
            .token_request(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &cred.refresh_token),
                ("client_id", &cred.client_id),
                ("resource", RESOURCE),
            ])
            .await?;
        let access = required_field(&v, "access_token")?;
        // 官方说成功的刷新一定带一份轮换后的 refresh token；万一没带（RFC 6749 §6
        // 允许不带），就**沿用旧的**——丢了它，下次刷新就只能让用户重新登录。
        let refresh = optional_field(&v, "refresh_token").unwrap_or(cred.refresh_token.clone());
        // 刷新响应**可能不带 id_token**：那就沿用上一次那份（它只用来认账号）。
        let id_token = optional_field(&v, "id_token").unwrap_or(cred.id_token.clone());
        let expires_in = v.get("expires_in").and_then(|n| n.as_u64()).unwrap_or(3600);
        let scopes = {
            let s = Self::scopes_of(&v, None);
            if s.is_empty() {
                cred.scopes.clone()
            } else {
                s
            }
        };
        let next = Credential {
            generation: cred.generation + 1,
            id_token,
            access_token: access,
            refresh_token: refresh,
            expires_at: now_unix() + expires_in,
            scopes,
            saved_at: now_unix(),
            ..cred.clone()
        };
        self.store
            .save(&next)
            .map_err(|e| TokenError::Transport(e.to_string()))?;
        Ok(next)
    }

    /// 撤销 refresh token（远端）。
    ///
    /// 官方：空的 HTTP 200 就是成功（token 本来就失效了也是 200）；网络失败或 5xx
    /// 要**退避重试**；最后还没确认就清本地，并告诉用户远端没确认。
    async fn revoke_remote(&self, cred: &Credential) -> Result<(), String> {
        let endpoint = self
            .revocation_endpoint()
            .await
            .ok_or_else(|| "没找到撤销端点".to_string())?;
        let mut last = String::new();
        for attempt in 0..REVOKE_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(400 << (attempt - 1))).await;
            }
            let sent = self
                .client
                .post(&endpoint)
                .timeout(TOKEN_TIMEOUT)
                .form(&[
                    ("token", cred.refresh_token.as_str()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", cred.client_id.as_str()),
                ])
                .send()
                .await;
            match sent {
                Ok(resp) if resp.status().is_success() => return Ok(()),
                Ok(resp) if resp.status().is_server_error() => {
                    last = format!("撤销失败：HTTP {}", resp.status());
                }
                Ok(resp) => return Err(format!("撤销失败：HTTP {}", resp.status())),
                Err(e) => last = format!("撤销请求失败：{e}"),
            }
        }
        Err(last)
    }

    /// OIDC discovery 文档。
    async fn discovery(&self) -> Result<Value, String> {
        let resp = self
            .client
            .get(&self.endpoints.discovery)
            .timeout(TOKEN_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("取 OIDC discovery 失败：{e}"))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .map_err(|e| format!("OIDC discovery 不是 JSON：{e}"))?;
        if !status.is_success() {
            return Err(format!("OIDC discovery HTTP {status}"));
        }
        Ok(v)
    }

    /// 官方公钥集（JWKS）。
    async fn jwks(&self) -> Result<crate::auth::jwt::Jwks, String> {
        let discovery = self.discovery().await?;
        let jwks_uri = discovery
            .get("jwks_uri")
            .and_then(|u| u.as_str())
            .ok_or("OIDC discovery 里没有 jwks_uri")?;
        let resp = self
            .client
            .get(jwks_uri)
            .timeout(TOKEN_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("取 JWKS 失败：{e}"))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .map_err(|e| format!("JWKS 不是 JSON：{e}"))?;
        if !status.is_success() {
            return Err(format!("JWKS HTTP {status}"));
        }
        crate::auth::jwt::Jwks::parse(&v)
    }

    /// OIDC discovery 里的撤销端点。
    async fn revocation_endpoint(&self) -> Option<String> {
        let v = self.discovery().await.ok()?;
        v.get("revocation_endpoint")
            .and_then(|e| e.as_str())
            .map(str::to_string)
    }

    /// 刷新时拿跨进程锁：两个 Dock 进程同时刷会触发 `refresh_token_reused`
    /// （refresh token 一次轮换，旧的那份立刻作废）。
    ///
    /// 刷新被判死（[`DEAD_REFRESH_CODES`]）就**清掉 token、留下注册**——官方：
    /// "Clear the unusable tokens and repeat OAuth with the saved client ID"。
    /// 网络抖一下不清（官方：别因为临时故障抹掉凭据）。
    async fn refresh_locked(&self, cred: &Credential) -> Result<Credential, TokenError> {
        let lock_path = self.store.lock_path_for(&cred.client_id);
        let _lock = FileLock::acquire(&lock_path, LOCK_TIMEOUT)
            .await
            .map_err(|e| TokenError::Transport(e.to_string()))?;
        // 拿到锁后重读：别的进程可能刚刷过，或者刚登出。
        let current = self
            .store
            .load(&cred.client_id)
            .map_err(|e| TokenError::Transport(e.to_string()))?;
        if !current.signed_in() {
            return Err(TokenError::OAuth {
                code: "invalid_grant".into(),
                detail: "这个账号已经登出了".into(),
                status: 0,
                body: String::new(),
            });
        }
        if current.generation > cred.generation {
            return Ok(current);
        }
        match self.refresh(&current).await {
            Ok(fresh) => Ok(fresh),
            Err(e) => {
                if e.is_credential_dead() {
                    if let Err(clear) = self.store.clear_tokens(&current.client_id) {
                        tracing::warn!(error = %clear, "订阅通道：作废的凭据没能清掉");
                    }
                }
                Err(e)
            }
        }
    }

    /// 读一份还登着、至少还能用 `min_valid` 的凭据（快过期就先刷新）。
    async fn fresh_credential(
        &self,
        client_id: &str,
        min_valid: Duration,
    ) -> Result<Credential, AuthError> {
        let cred = self.load_signed_in(client_id)?;
        if cred.expires_in() >= min_valid.as_secs() as i64 {
            return Ok(cred);
        }
        self.refresh_locked(&cred).await.map_err(AuthError::from)
    }

    /// 读一份凭据，要求它还登着。
    fn load_signed_in(&self, client_id: &str) -> Result<Credential, AuthError> {
        let cred = self.store.load(client_id).map_err(|e| match e {
            CredentialError::Missing => AuthError::NeedsLogin {
                reason: format!("{client_id} 还没登录"),
            },
            other => AuthError::NeedsLogin {
                reason: format!("读不到凭据：{other}"),
            },
        })?;
        if !cred.signed_in() {
            return Err(AuthError::NeedsLogin {
                reason: format!(
                    "{} 已经登出了：/auth login 重新登录（会复用原来的注册）",
                    cred.email.as_deref().unwrap_or(&cred.client_id)
                ),
            });
        }
        Ok(cred)
    }

    /// 强制刷新一次（不等过期）。返回新的 lease。
    ///
    /// 推理侧通常不需要——`lease()` 会在过期前自己刷。这条给「立刻验证刷新
    /// 能不能用」的场景（refresh token 轮换、跨进程锁）。
    pub async fn refresh_now(&self, acct: &AccountKey) -> Result<Lease, AuthError> {
        let cred = self.load_signed_in(&acct.client_id)?;
        let refreshed = self.refresh_locked(&cred).await.map_err(AuthError::from)?;
        Ok(lease_of(&refreshed))
    }

    /// 登录的真身。
    ///
    /// 官方那套流程（`/siwc/token-sharing-open-source/sign-in`）：
    ///
    /// 1. **新账号**用 `client_id=dynamic_agent_client` + `agent_name_hint` +
    ///    `ext_agent_host_id`；回调带回服务端分配的**正式 client id**（`oaiapp_…`）。
    /// 2. **换码**用正式 client id。码被拒（`invalid_grant`）时丢掉这个码、用这次
    ///    保留的正式 client id 重新授权一次。
    /// 3. **已注册的账号**（含登出了的）一律用它保存的正式 client id，不带
    ///    `agent_name_hint`；带 `login_hint`（账号邮箱）跳过账号选择。
    async fn login_impl(
        &self,
        ui: &dyn LoginUi,
        req: LoginRequest,
    ) -> Result<AccountKey, AuthError> {
        let host_id = load_or_create_host_id().map_err(|e| AuthError::Transport(e.to_string()))?;
        let reg = registration_for(&self.store, &req.target)?;
        let mut issued = reg.client_id.clone();

        // 最多两轮：首轮 + `invalid_grant` 之后用正式 client id 重新授权一轮。
        for round in 1..=2u8 {
            let dynamic = issued.is_none();
            let auth_client_id = issued
                .clone()
                .unwrap_or_else(|| DYNAMIC_CLIENT_ID.to_string());
            // **不带 `id_token_hint`**——那是一条真的 ID token，授权 URL 会进日志
            // 也会推给客户端。`login_hint`（账号邮箱）够用了。
            let login_hint = if dynamic { None } else { reg.email.clone() };

            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|e| AuthError::Transport(format!("起回环监听失败：{e}")))?;
            let port = listener
                .local_addr()
                .map_err(|e| AuthError::Transport(format!("回环端口拿不到：{e}")))?
                .port();
            // 回环回调只能用 127.0.0.1，不能用 localhost；同一次授权里端口不变。
            let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");

            let verifier = oauth::pkce_verifier();
            let challenge = oauth::pkce_challenge(&verifier);
            let state = oauth::random_urlsafe(24);
            let nonce = oauth::random_urlsafe(16);
            // 官方：别每次都逼用户重新同意。只有这次要的 scope 上次没给全（通常是
            // 套餐那条）才摆同意页。
            let need_consent =
                !dynamic && !reg.granted.is_empty() && !granted_contains(&reg.granted, req.scope);
            let url = authorize_url(
                &self.endpoints.authorize,
                &AuthorizeRequest {
                    redirect_uri: &redirect_uri,
                    challenge: &challenge,
                    state: &state,
                    nonce: &nonce,
                    host_id: &host_id,
                    client_id: &auth_client_id,
                    dynamic_registration: dynamic,
                    scope: req.scope,
                    login_hint: login_hint.as_deref(),
                    prompt_consent: need_consent,
                },
            );
            ui.open_url(&url);

            let scope = req.scope;
            let (callback, page) = tokio::time::timeout(
                LOGIN_TIMEOUT,
                oauth::accept_callback(listener, CALLBACK_PATH, SERVICE_NAME, |p| {
                    parse_callback(p, &state, scope)
                }),
            )
            .await
            .map_err(|_| AuthError::NeedsLogin {
                reason: format!(
                    "等授权超时（{} 秒）。重新跑一次 /auth login",
                    LOGIN_TIMEOUT.as_secs()
                ),
            })?
            // state 已在解析时校验过（拒绝的回调也先校验 state）。
            .map_err(|CallbackError(e, _)| e)?;

            // 拿到回调之后的每一步都在这里面：不管走到哪一步出结果，浏览器那一页
            // 都要切到这个结果（连上了 / 还要再授权一轮 / 没连上）。
            let step: Result<Round, AuthError> = async {
                // 回调给的正式 client id。重新授权时它可以不给；给了就必须与本机保存的
                // 一致——不一致就拒，不能拿它替换掉已选的账号（官方原话）。
                let exchange_client_id = match callback.client_id.clone() {
                    Some(cid) => {
                        if !dynamic && issued.as_deref() != Some(cid.as_str()) {
                            return Err(AuthError::NeedsLogin {
                                reason: format!(
                                    "回调给的 client id（{cid}）与本机保存的不一致，已拒绝"
                                ),
                            });
                        }
                        cid
                    }
                    None => {
                        if dynamic {
                            // 新注册没有正式 client id = 注册没完成（官方原话）。
                            return Err(AuthError::NeedsLogin {
                                reason: "回调里没有正式的 client id，这次注册没完成".into(),
                            });
                        }
                        auth_client_id.clone()
                    }
                };

                // 正式 client id 立刻落盘：官方要求「save it with the registration」，
                // 换码还没成也得先记下——下一轮 / 下次登录用它。
                if dynamic {
                    if let Err(e) = self.store.remember_issued(&exchange_client_id, None) {
                        return Err(AuthError::NeedsLogin {
                            reason: format!(
                                "已注册但记不住正式 client id（{e}）：检查凭据目录的权限。"
                            ),
                        });
                    }
                }

                let token = match self
                    .exchange_code(
                        &callback.code,
                        &redirect_uri,
                        &verifier,
                        &exchange_client_id,
                    )
                    .await
                {
                    Ok(token) => token,
                    Err(e) if round == 1 && e.code() == "invalid_grant" => {
                        // 官方：丢掉这个码，用这次保留的正式 client id 重新授权一次。
                        tracing::info!("换码被拒（invalid_grant），用正式 client id 重新授权");
                        return Ok(Round::Again(exchange_client_id));
                    }
                    Err(e) => {
                        return Err(AuthError::Transport(format!(
                            "换码失败（client_id={exchange_client_id}，回调参数={:?}）：{e}",
                            callback.keys
                        )));
                    }
                };
                self.finish_login(ui, &reg, &token, &exchange_client_id, &nonce, &host_id)
                    .await
                    .map(|cred| Round::Done(Box::new(cred)))
            }
            .await;

            page.finish(&round_page(&step)).await;
            match step {
                Ok(Round::Done(cred)) => return Ok(key_of(&cred)),
                Ok(Round::Again(client_id)) => issued = Some(client_id),
                Err(e) => return Err(e),
            }
        }
        Err(AuthError::NeedsLogin {
            reason: "两次授权都没换到 token：重新跑一次 /auth login".into(),
        })
    }

    /// 换到 token 之后：验 ID token、认账号、落盘、切成活动账号。
    async fn finish_login(
        &self,
        ui: &dyn LoginUi,
        reg: &Registration,
        token: &Value,
        exchange_client_id: &str,
        nonce: &str,
        host_id: &str,
    ) -> Result<Credential, AuthError> {
        let (access, refresh, id_token) = Self::exchange_parts(token).map_err(AuthError::from)?;
        let client_id = Self::official_client_id(token, Some(exchange_client_id));

        // 验签要官方公钥：从 OIDC discovery 的 jwks_uri 取。
        let jwks = self
            .jwks()
            .await
            .map_err(|e| AuthError::NeedsLogin { reason: e })?;
        let claims = verify_id_token(&id_token, &client_id, nonce, &jwks)
            .map_err(|e| AuthError::NeedsLogin { reason: e })?;
        let subject = claims
            .subject
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AuthError::NeedsLogin {
                reason: "ID token 里没有 sub，认不出是哪个账号".into(),
            })?;
        // 官方：**绝不覆盖别的账号注册的凭据**。重新登录换回来的必须是同一个人；
        // 这个 client id 在本机要是已经属于另一个账号，也拒。
        if let Some(expected) = &reg.subject {
            if expected != &subject {
                return Err(AuthError::NeedsLogin {
                    reason: "授权回来的是另一个 ChatGPT 账号，没有覆盖原来那个。要加新账号用 /auth login new".into(),
                });
            }
        }
        let existing = self.store.load(&client_id).ok();
        if let Some(other) = &existing {
            if !other.subject.is_empty() && other.subject != subject {
                return Err(AuthError::NeedsLogin {
                    reason: format!("client id {client_id} 在本机已经属于另一个账号，已拒绝"),
                });
            }
        }
        // **以 token 响应里给的 scope 为准**（回调里的 scope 只是预告）。
        let scopes = Self::scopes_of(token, claims.scope.as_deref());
        let expires_in = token
            .get("expires_in")
            .and_then(|n| n.as_u64())
            .unwrap_or(3600);
        let cred = Credential {
            schema_version: crate::auth::credentials::SCHEMA_VERSION,
            // 同一个注册再登录，代数接着涨：手里还捏着旧 lease 的请求，401 时一比就知道它旧了。
            generation: existing.as_ref().map(|c| c.generation + 1).unwrap_or(1),
            email: claims.email.clone(),
            issuer: ISSUER.to_string(),
            subject: subject.clone(),
            client_id: client_id.clone(),
            id_token,
            access_token: access,
            refresh_token: refresh,
            expires_at: now_unix() + expires_in,
            scopes,
            saved_at: now_unix(),
            ext_agent_host_id: host_id.to_string(),
        };
        self.store
            .save(&cred)
            .map_err(|e| AuthError::Transport(e.to_string()))?;
        // 官方：新登录在验完身份之前不碰活动账号——到这儿才切。
        self.store
            .set_active(&client_id)
            .map_err(|e| AuthError::Transport(e.to_string()))?;
        self.store.clear_issued();

        let who = cred.email.clone().unwrap_or_else(|| client_id.clone());
        if cred.has_plan_usage() {
            ui.status(&format!(
                "登录成功：{who}。You're using your ChatGPT plan——走订阅通道的模型（auth = \"chatgpt\"）\
                 用的是套餐额度，用量在 ChatGPT 设置的 Usage 页管理。/auth models 看可用模型。"
            ));
        } else {
            // 官方「ChatGPT plan use isn't enabled」：保留登录、标记套餐没开，给两条路。
            ui.status(&format!(
                "登录成功：{who}，但这份授权没有套餐额度（scope 里没有 {PLAN_SCOPE}），不能拿它推理。\
                 要用套餐就 /auth login 重新同意；或者给模型配 api_key 走按量计费。"
            ));
        }
        Ok(cred)
    }
}

/// 这次登录用哪个注册。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Registration {
    /// 正式 client id。`None` = 新账号，走首登（动态注册）。
    client_id: Option<String>,
    /// 重新授权时的 `login_hint`。
    email: Option<String>,
    /// 已知账号的 subject：换回来的 ID token 必须是同一个人。
    subject: Option<String>,
    /// 上次授权过的 scope（决定要不要摆同意页）。
    granted: Vec<String>,
}

impl Registration {
    fn of(cred: &Credential) -> Self {
        Self {
            client_id: Some(cred.client_id.clone()),
            email: cred.email.clone(),
            subject: Some(cred.subject.clone()).filter(|s| !s.is_empty()),
            granted: cred.scopes.clone(),
        }
    }

    /// 新账号。首登回调拿到了正式 client id、但换码没成的那个注册还在，就接着用它。
    fn new_account(store: &CredentialStore) -> Self {
        match store.issued() {
            Some(id) => Self {
                client_id: Some(id),
                email: store.issued_email(),
                ..Self::default()
            },
            None => Self::default(),
        }
    }
}

/// 按登录目标挑注册。**登出了的账号也有注册**——那份文件里只是没了 token。
fn registration_for(
    store: &CredentialStore,
    target: &LoginTarget,
) -> Result<Registration, AuthError> {
    match target {
        LoginTarget::New => Ok(Registration::new_account(store)),
        LoginTarget::Account(id) => {
            store
                .load(id)
                .map(|cred| Registration::of(&cred))
                .map_err(|e| AuthError::NeedsLogin {
                    reason: format!("本机没有这个账号：{id}（{e}）"),
                })
        }
        LoginTarget::Active => match store.active() {
            None => Ok(Registration::new_account(store)),
            Some(id) => match store.load(&id) {
                Ok(cred) => Ok(Registration::of(&cred)),
                Err(CredentialError::Missing) => Ok(Registration::new_account(store)),
                // 读坏了（权限被改过之类）别悄悄当成新账号：那会再注册一个。
                Err(e) => Err(AuthError::NeedsLogin {
                    reason: format!("活动账号的凭据读不了：{e}"),
                }),
            },
        },
    }
}

impl AuthProvider for ChatGptProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            display_name: "ChatGPT",
            usage_url: Some(USAGE_URL),
        }
    }

    fn wire_policy(&self) -> &WirePolicy {
        &self.policy
    }

    fn api_base(&self) -> Option<&str> {
        Some(&self.endpoints.api)
    }

    fn login<'a>(
        &'a self,
        ui: &'a dyn LoginUi,
        req: LoginRequest,
    ) -> AuthFuture<'a, Result<AccountKey, AuthError>> {
        Box::pin(self.login_impl(ui, req))
    }

    fn accounts(&self) -> Vec<AccountSummary> {
        let active = self.store.active();
        self.store
            .list()
            .unwrap_or_default()
            .iter()
            .map(|c| AccountSummary {
                account: key_of(c),
                email: c.email.clone(),
                signed_in: c.signed_in(),
                plan_usage: c.signed_in() && c.has_plan_usage(),
                expires_at: if c.signed_in() { c.expires_at } else { 0 },
                active: active.as_deref() == Some(c.client_id.as_str()),
            })
            .collect()
    }

    fn active_account(&self) -> Option<AccountKey> {
        let id = self.store.active()?;
        self.store.load(&id).ok().map(|c| key_of(&c))
    }

    fn set_active(&self, account_id: &str) -> Result<AccountKey, AuthError> {
        let cred = self.store.load(account_id).map_err(|e| match e {
            CredentialError::Missing => AuthError::Rejected {
                code: "unknown_account".into(),
                detail: format!("本机没有这个账号：{account_id}"),
            },
            other => AuthError::Transport(other.to_string()),
        })?;
        // 官方：账号要先完成并验过 OAuth 才能成为活动账号。登出了的就得先重新登录。
        if !cred.signed_in() {
            return Err(AuthError::NeedsLogin {
                reason: format!("{account_id} 已经登出了：先 /auth login {account_id} 重新登录"),
            });
        }
        self.store
            .set_active(account_id)
            .map_err(|e| AuthError::Transport(e.to_string()))?;
        Ok(key_of(&cred))
    }

    /// 导入别处传来的凭据（官方「Self-hosted VMs」）：**用本机的 host id**，刷新由本机接管。
    fn import(&self, raw: &str) -> Result<AccountKey, AuthError> {
        let bad = |detail: String| AuthError::Rejected {
            code: "bad_credential".into(),
            detail,
        };
        let mut cred: Credential =
            serde_json::from_str(raw).map_err(|e| bad(format!("这不是一份凭据文件：{e}")))?;
        // 版本对不上就别导：字段含义变了，导进来的是一份看不懂的凭据。
        if cred.schema_version != crate::auth::credentials::SCHEMA_VERSION {
            return Err(bad(format!(
                "凭据版本不对（{}，期望 {}）",
                cred.schema_version,
                crate::auth::credentials::SCHEMA_VERSION
            )));
        }
        if !cred.signed_in() || cred.client_id.is_empty() {
            return Err(bad(
                "这份凭据里没有 refresh token（可能是登出后的那份）".into()
            ));
        }
        if let Ok(existing) = self.store.load(&cred.client_id) {
            if !existing.subject.is_empty() && existing.subject != cred.subject {
                return Err(bad(format!(
                    "client id {} 在本机已经属于另一个账号，没有覆盖",
                    cred.client_id
                )));
            }
        }
        // **host id 用本机的**：凭据里那份是来源机器的。官方："keep the ID assigned
        // to the VM"，不让抄过来的笔记本 id 盖掉它。
        cred.ext_agent_host_id =
            load_or_create_host_id().map_err(|e| AuthError::Transport(e.to_string()))?;
        cred.saved_at = now_unix();
        self.store
            .save(&cred)
            .map_err(|e| AuthError::Transport(e.to_string()))?;
        self.store
            .set_active(&cred.client_id)
            .map_err(|e| AuthError::Transport(e.to_string()))?;
        Ok(key_of(&cred))
    }

    /// 取一份至少还能用 `min_valid` 的 access token。
    fn lease<'a>(
        &'a self,
        acct: &'a AccountKey,
        min_valid: Duration,
    ) -> AuthFuture<'a, Result<Lease, AuthError>> {
        Box::pin(async move {
            let cred = self.load_signed_in(&acct.client_id)?;
            if !cred.has_plan_usage() {
                // 官方：没有套餐那条 scope 就别推理（保留登录，标记套餐没开）。
                return Err(AuthError::NotEligible {
                    code: "missing_plan_usage_scope".into(),
                });
            }
            let cred = self.fresh_credential(&cred.client_id, min_valid).await?;
            Ok(lease_of(&cred))
        })
    }

    /// 401：别的进程刷过了就用新的，否则自己刷一次。
    ///
    /// 刷新被判死 = 凭据已清，不重试（推理侧会报「要重新登录」）；刷新只是网络
    /// 抖了 = 原样报那个错——那时让用户重新登录是错的。
    fn on_unauthorized<'a>(&'a self, lease: &'a Lease) -> AuthFuture<'a, Result<Retry, AuthError>> {
        Box::pin(async move {
            let Ok(cred) = self.store.load(&lease.account.client_id) else {
                return Ok(Retry::GiveUp);
            };
            if !cred.signed_in() {
                return Ok(Retry::GiveUp);
            }
            // 代数落后 = 别人刷过了，直接用那份。
            if cred.generation > lease.generation {
                return Ok(Retry::WithLease(lease_of(&cred)));
            }
            match self.refresh_locked(&cred).await {
                Ok(fresh) => Ok(Retry::WithLease(lease_of(&fresh))),
                Err(e) if e.is_credential_dead() => Ok(Retry::GiveUp),
                Err(e) => {
                    tracing::warn!(error = %e, "订阅通道：401 之后刷新失败");
                    Err(AuthError::from(e))
                }
            }
        })
    }

    /// 登出：撤销远端 refresh token，再清本地 token——**注册留着**（client id、
    /// 账号身份、本机 host id），下次登录直接复用。
    fn logout<'a>(
        &'a self,
        acct: &'a AccountKey,
        revoke: bool,
    ) -> AuthFuture<'a, Result<LogoutOutcome, AuthError>> {
        Box::pin(async move {
            // 登出也要拿锁：别的进程正在刷新时把它清了，那份刷新结果会写回一份
            // 已经登出的凭据——等于登出没生效。
            let _lock = FileLock::acquire(&self.store.lock_path_for(&acct.client_id), LOCK_TIMEOUT)
                .await
                .ok();
            let cred = match self.store.load(&acct.client_id) {
                Ok(cred) => cred,
                Err(CredentialError::Missing) => {
                    return Err(AuthError::Rejected {
                        code: "unknown_account".into(),
                        detail: format!("本机没有这个账号：{}", acct.client_id),
                    })
                }
                Err(e) => return Err(AuthError::Transport(e.to_string())),
            };
            if !cred.signed_in() {
                // 已经登出了：没有东西可撤。
                return Ok(LogoutOutcome::Revoked);
            }
            let mut outcome = LogoutOutcome::Revoked;
            if revoke {
                if let Err(e) = self.revoke_remote(&cred).await {
                    tracing::warn!(error = %e, "订阅通道：远端撤销没确认");
                    outcome = LogoutOutcome::LocalOnly { reason: e };
                }
            }
            self.store
                .clear_tokens(&acct.client_id)
                .map_err(|e| AuthError::Transport(e.to_string()))?;
            Ok(outcome)
        })
    }

    /// 列这个账号能用的模型（官方：`GET /v1/models`，取 `models[]` 里
    /// `visibility == "list"` 的 `slug` / `display_name`）。
    ///
    /// token 快过期就先刷新——不然过期一小时后这里会叫用户「重新登录」，其实刷新一下就好。
    fn models<'a>(
        &'a self,
        acct: &'a AccountKey,
    ) -> AuthFuture<'a, Result<Vec<RemoteModel>, AuthError>> {
        Box::pin(async move {
            let cred = self
                .fresh_credential(&acct.client_id, Duration::from_secs(60))
                .await?;
            let url = format!("{}/models", self.endpoints.api);
            let resp = self
                .client
                .get(&url)
                .timeout(TOKEN_TIMEOUT)
                .bearer_auth(&cred.access_token)
                .send()
                .await
                .map_err(|e| AuthError::Transport(format!("列模型失败：{e}")))?;
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(self
                    .classify(status.as_u16(), body.as_bytes())
                    .unwrap_or_else(|| {
                        AuthError::Transport(format!("列模型失败：HTTP {status}"))
                    }));
            }
            let v: Value = serde_json::from_str(&body)
                .map_err(|e| AuthError::Transport(format!("列模型的响应不是 JSON：{e}")))?;
            Ok(parse_models(&v))
        })
    }

    /// 把上游的 HTTP 错误归一成 [`AuthError`]。
    fn classify(&self, status: u16, body: &[u8]) -> Option<AuthError> {
        classify(status, body)
    }
}

/// 凭据 → 账号。
fn key_of(cred: &Credential) -> AccountKey {
    AccountKey {
        provider: PROVIDER_ID.into(),
        subject: cred.subject.clone(),
        client_id: cred.client_id.clone(),
    }
}

/// 凭据 → lease（推理侧唯一能拿到的东西）。
fn lease_of(cred: &Credential) -> Lease {
    Lease {
        bearer: cred.access_token.clone(),
        expires_at: std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(cred.expires_at),
        generation: cred.generation,
        account: key_of(cred),
    }
}

/// 成功响应里必须有的字段。缺了报错，**报错里不带 body**（body 里就是 token）。
fn required_field(v: &Value, name: &str) -> Result<String, TokenError> {
    optional_field(v, name).ok_or_else(|| TokenError::OAuth {
        code: format!("no_{name}"),
        detail: format!("token 响应里没有 {name}"),
        status: 200,
        body: String::new(),
    })
}

fn optional_field(v: &Value, name: &str) -> Option<String> {
    v.get(name)
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 把 JSON 里所有「名字带 token」的值抹掉（错误信息会进提示、日志、会话记录）。
fn redact_tokens(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, val)| {
                    if k.to_ascii_lowercase().contains("token") {
                        (k.clone(), json!("<redacted>"))
                    } else {
                        (k.clone(), redact_tokens(val))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_tokens).collect()),
        other => other.clone(),
    }
}

/// 一次授权请求的参数。
///
/// 首登用动态 client + `agent_name_hint` + `ext_agent_host_id`；重新授权用正式
/// client（`client_id`）+ `login_hint`。
///
/// **不带 `id_token_hint`**：官方把它列为可选，但它是一条**真的 ID token**——
/// 授权 URL 会进日志、也会推给客户端，带着它就等于把凭据发出去。用 `login_hint`
/// （账号邮箱）表达「还是这个账号」即可。
struct AuthorizeRequest<'a> {
    redirect_uri: &'a str,
    challenge: &'a str,
    state: &'a str,
    nonce: &'a str,
    host_id: &'a str,
    client_id: &'a str,
    /// 首登（动态注册）还是重新授权。
    dynamic_registration: bool,
    scope: ScopeMode,
    login_hint: Option<&'a str>,
    /// 要不要把同意页摆出来（`prompt=consent`）。
    prompt_consent: bool,
}

fn authorize_url(endpoint: &str, req: &AuthorizeRequest<'_>) -> String {
    let mut url = url::Url::parse(endpoint).expect("授权端点是合法 URL");
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", req.client_id);
        q.append_pair("redirect_uri", req.redirect_uri);
        q.append_pair("state", req.state);
        q.append_pair("nonce", req.nonce);
        q.append_pair("code_challenge", req.challenge);
        q.append_pair("code_challenge_method", "S256");
        q.append_pair("scope", &scopes_for(req.scope).join(" "));
        q.append_pair("resource", RESOURCE);
        // 重新授权且**要更多权限**时把同意页摆出来。权限没变就不带——官方：
        // 别每次普通登录都逼用户重新同意。
        if req.prompt_consent {
            q.append_pair("prompt", "consent");
        }
        // agent_name_hint 只在**首次动态注册**时带；用正式 client id 重新授权时不带。
        if req.dynamic_registration {
            q.append_pair("agent_name_hint", AGENT_NAME);
        }
        q.append_pair("ext_agent_host_id", req.host_id);
        if let Some(hint) = req.login_hint {
            q.append_pair("login_hint", hint);
        }
    }
    url.to_string()
}

/// 这次要的 scope 是不是都已经在上次授权里了。
///
/// 全都在 = 不用再摆同意页；缺了哪一条（通常是套餐额度那条）= 要重新同意。
fn granted_contains(granted: &[String], mode: ScopeMode) -> bool {
    scopes_for(mode)
        .iter()
        .all(|want| granted.iter().any(|g| g == want))
}

/// 校验回调带回来的 `state`。
///
/// **对不上就中止**：state 是这次授权与那个回调的唯一绑定，对不上说明这个回调
/// 不是我们这次请求换来的（被人改过，或者串了别的会话）。
fn check_state(expected: &str, got: &str) -> Result<(), AuthError> {
    if got == expected {
        return Ok(());
    }
    Err(AuthError::NeedsLogin {
        reason: "授权返回的 state 对不上，已中止（回调被人改过）".into(),
    })
}

/// 回调带回来的东西。
#[derive(Debug, Default)]
struct Callback {
    code: String,
    state: String,
    client_id: Option<String>,
    /// 回调带了哪些参数名（诊断用：官方到底发了什么）。
    keys: Vec<String>,
}

/// `/v1/models` 的响应 → 可以给用户选的模型。
///
/// 官方：取 `models[]` 里 `visibility == "list"` 的，**保持上游顺序**，显示
/// `display_name`、请求里传 `slug`。其余字段（上下文窗口、推理档位、能不能收图）
/// 文档没承诺，上游实际会给——有就用，没有就空着。
fn parse_models(v: &Value) -> Vec<RemoteModel> {
    let text = |m: &Value, key: &str| {
        m.get(key)
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    v.get("models")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
        .filter(|m| {
            m.get("visibility")
                .and_then(|s| s.as_str())
                .is_none_or(|v| v == "list")
        })
        .filter_map(|m| {
            let slug = text(m, "slug")?;
            let efforts = m
                .get("supported_reasoning_levels")
                .and_then(|l| l.as_array())
                .map(|levels| {
                    levels
                        .iter()
                        .filter_map(|l| text(l, "effort"))
                        .collect::<Vec<_>>()
                });
            let images = m
                .get("input_modalities")
                .and_then(|l| l.as_array())
                .map(|l| l.iter().any(|x| x.as_str() == Some("image")));
            Some(RemoteModel {
                display_name: text(m, "display_name"),
                description: text(m, "description"),
                context_window: m
                    .get("context_window")
                    .and_then(|n| n.as_u64())
                    .filter(|n| *n > 0),
                reasoning_efforts: efforts,
                default_reasoning_effort: text(m, "default_reasoning_level"),
                supports_images: images,
                slug,
            })
        })
        .collect()
}

/// 授权回调页上怎么称呼这一家。
const SERVICE_NAME: &str = "ChatGPT";

/// 一轮授权的结果。
#[derive(Debug)]
enum Round {
    /// 登上了（已落盘、已切成活动账号）。
    Done(Box<Credential>),
    /// 首轮换码被拒（`invalid_grant`）：用这个正式 client id 再授权一轮。
    Again(String),
}

/// 浏览器那一页最后显示什么。
fn round_page(step: &Result<Round, AuthError>) -> oauth::PageOutcome {
    use oauth::{PageOutcome, Tone};
    match step {
        Ok(Round::Done(cred)) => {
            let page = PageOutcome::connected(SERVICE_NAME).account(cred.email.clone());
            if cred.has_plan_usage() {
                page.chip("套餐额度已开", Tone::Ok)
                    .detail("走订阅通道的模型会用你的 ChatGPT 套餐额度，不需要 API key。")
                    .link("Manage usage", USAGE_URL)
            } else {
                // 官方「ChatGPT plan use isn't enabled」：登录留着，讲清楚、给两条路。
                page.chip("仅身份 · 套餐额度未开", Tone::Warn)
                    .detail("这份授权没有套餐额度，不能拿它推理。")
                    .step("要用套餐：在 Dock 里 `/auth login` 重新同意")
                    .step("按量计费：给模型配 `api_key`")
            }
        }
        Ok(Round::Again(_)) => PageOutcome::next_step("还差一步")
            .detail("Dock 已经在 ChatGPT 注册好了。浏览器会再打开一页授权，在那一页点「继续」。"),
        Err(e) => PageOutcome::failed(SERVICE_NAME, page_reason(e)),
    }
}

/// 页面上的失败原因：`Rejected` 只给说明（「上游拒绝了这个请求」那个前缀是写给
/// 推理请求的，放在授权页上读着别扭），其余用完整的提示。
fn page_reason(e: &AuthError) -> String {
    match e {
        AuthError::Rejected { detail, .. } if !detail.is_empty() => detail.clone(),
        other => other.message(),
    }
}

/// 回调没成：带着要交给用户的那条 [`AuthError`]，以及浏览器失败页（没给就用
/// 默认的「没有连上 ChatGPT」+ 原因）。
#[derive(Debug)]
struct CallbackError(AuthError, Option<Box<oauth::PageOutcome>>);

impl std::fmt::Display for CallbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&page_reason(&self.0))
    }
}

impl From<String> for CallbackError {
    fn from(detail: String) -> Self {
        CallbackError(AuthError::Transport(detail), None)
    }
}

impl oauth::CallbackFailure for CallbackError {
    fn page(&self, service: &str) -> oauth::PageOutcome {
        match &self.1 {
            Some(page) => (**page).clone(),
            None => oauth::PageOutcome::failed(service, self.to_string()),
        }
    }
}

/// 解析回调。**先校验 state**（官方：拒绝的回调也要先校验 state 再停下），
/// 再看是不是被拒了，最后才取 code。
fn parse_callback(
    params: &std::collections::HashMap<String, String>,
    expected_state: &str,
    scope: ScopeMode,
) -> Result<Callback, CallbackError> {
    let state = params.get("state").map(String::as_str).unwrap_or_default();
    check_state(expected_state, state).map_err(|e| CallbackError(e, None))?;
    if let Some(error) = params.get("error").filter(|e| !e.is_empty()) {
        let description = params
            .get("error_description")
            .filter(|d| !d.is_empty())
            .cloned();
        return Err(CallbackError(
            denied(error, description, scope),
            denied_page(error, scope).map(Box::new),
        ));
    }
    let callback = Callback {
        code: params.get("code").cloned().unwrap_or_default(),
        state: params.get("state").cloned().unwrap_or_default(),
        client_id: params.get("client_id").cloned().filter(|c| !c.is_empty()),
        keys: {
            let mut keys: Vec<String> = params.keys().cloned().collect();
            keys.sort();
            keys
        },
    };
    if callback.code.is_empty() || callback.state.is_empty() {
        return Err(CallbackError::from("回调里没有 code 或 state".to_string()));
    }
    tracing::info!(
        client_id = ?callback.client_id,
        keys = ?callback.keys,
        "订阅授权回调"
    );
    Ok(callback)
}

/// 授权页那头没放行（回调带 `error=…`）。
///
/// 官方：`access_denied` 是用户拒了，或者这份授权里没有用套餐的权限——停下、
/// 不换码，转到「ChatGPT plan use isn't enabled」：讲清楚，给出升级套餐或换别的
/// 计费方式两条路。套餐那条给不了时授权页的「继续」是灰的，用户只能点取消，
/// 所以要了套餐额度又被拒，多半就是这个账号的套餐不够。
fn denied(error: &str, description: Option<String>, scope: ScopeMode) -> AuthError {
    let detail = match (error, scope) {
        ("access_denied", ScopeMode::Default) => {
            "没拿到用 ChatGPT 套餐额度的授权。授权页写着「所需权限不可用」、「继续」点不了，\
             就是这个账号（或工作空间）的套餐不能把额度给 Dock 用：升级套餐（官方列的是 \
             Go / Plus / Pro / Business）后重新 /auth login；只登录身份用 /auth login identity；\
             或者给模型配 api_key 按量计费。如果是你自己点了取消，重新 /auth login 就行。"
                .to_string()
        }
        ("access_denied", ScopeMode::IdentityOnly) => {
            "授权页上取消了。重新 /auth login identity 再来。".to_string()
        }
        _ => description.unwrap_or_default(),
    };
    AuthError::Rejected {
        code: error.to_string(),
        detail,
    }
}

/// 被拒时浏览器那一页：一句原因 + 接下来可以怎么做（命令用反引号，页面上是等宽胶囊）。
/// Dock 里报的还是 [`denied`] 那一整句。
fn denied_page(error: &str, scope: ScopeMode) -> Option<oauth::PageOutcome> {
    use oauth::{PageOutcome, Tone};
    match (error, scope) {
        ("access_denied", ScopeMode::Default) => Some(
            PageOutcome::failed(
                SERVICE_NAME,
                "这个账号（或工作空间）的套餐不能把额度给 Dock 用，所以授权页上「继续」是灰的。",
            )
            .chip("套餐额度不可用", Tone::Warn)
            .step("升级套餐（官方列的是 Go / Plus / Pro / Business），再在 Dock 里 `/auth login`")
            .step("只登录身份：`/auth login identity`")
            .step("按量计费：给模型配 `api_key`")
            .step("是自己点了取消？重新 `/auth login` 就行"),
        ),
        ("access_denied", ScopeMode::IdentityOnly) => Some(
            PageOutcome::failed(SERVICE_NAME, "授权页上取消了。")
                .step("重新登录：`/auth login identity`"),
        ),
        _ => None,
    }
}

/// 上游错误 → [`AuthError`]。按官方「Errors and recovery」那一页分。
pub fn classify(status: u16, body: &[u8]) -> Option<AuthError> {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let err = v.get("error").filter(|e| e.is_object());
    let field = |name: &str| {
        err.and_then(|e| e.get(name))
            .or_else(|| v.get(name))
            .and_then(|c| c.as_str())
            .map(str::to_string)
    };
    let code = field("code").unwrap_or_default();
    let param = field("param");
    let detail = field("message").unwrap_or_default();
    let request_id = v
        .get("request_id")
        .and_then(|r| r.as_str())
        .map(str::to_string);

    match code.as_str() {
        // 429：别以为整个套餐空了，也别猜重置时间；指去用量页。
        "subscription_sharing_usage_limit_exceeded" => {
            Some(AuthError::QuotaExhausted { code, request_id })
        }
        // 503：凭据留着，有上限地退避重试。
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => {
            Some(AuthError::Unavailable { code, retry: true })
        }
        // 400：看 `error.param`，去掉那项再重试；**绝不原样重发**（推理层做）。
        "subscription_sharing_unsupported_capability" => Some(AuthError::Unsupported { param }),
        // 403：说明限制；别重复请求，也别循环走 OAuth。
        "subscription_sharing_user_not_eligible" => Some(AuthError::NotEligible { code }),
        // 403：这个码只授权 `POST /v1/responses`。
        "subscription_sharing_route_not_supported" => Some(AuthError::Rejected {
            code,
            detail: "这条通道只认 POST /v1/responses".into(),
        }),
        // 401：留着 request id 查凭据；只有确认撤销或刷新被判死才让用户重新登录。
        // 推理层到这里时已经刷新重试过一次了。
        "subscription_sharing_invalid_user" => Some(AuthError::NeedsLogin {
            reason: format!(
                "上游不认这个账号的凭据（{code}{}），刷新过一次仍被拒。到 ChatGPT 设置里确认没有断开 Dock，\
                 再 /auth login 重新登录。",
                request_id
                    .as_deref()
                    .map(|id| format!("，request-id: {id}"))
                    .unwrap_or_default()
            ),
        }),
        // 403：client / 授权配置不对。不重试，也不换计费方式。
        "chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context" => {
            Some(AuthError::Rejected {
                code,
                detail: "client 与授权配置不对：不重试，也不换计费方式".into(),
            })
        }
        _ => match status {
            401 => Some(AuthError::NeedsLogin {
                reason: "令牌被拒，刷新过一次仍不行：/auth login 重新登录".into(),
            }),
            // 官方：403 要把限制说出来（比如地区策略），不是一律「没资格」。
            403 => Some(AuthError::Rejected {
                code: if code.is_empty() {
                    "http_403".into()
                } else {
                    code
                },
                detail: if detail.is_empty() {
                    "可能是地区或工作空间的策略限制".into()
                } else {
                    detail
                },
            }),
            429 => Some(AuthError::QuotaExhausted { code, request_id }),
            503 => Some(AuthError::Unavailable { code, retry: true }),
            _ => None,
        },
    }
}

/// 环境变量：把订阅通道整个关掉（`off` / `0` / `false`）。
///
/// 开关关掉 = **这个 provider 根本不注册**：模型条目写了 `auth = "chatgpt"`
/// 的会明确报「没装鉴权插件」，没写 `auth` 的老配置一点不受影响。想临时退回
/// 按量计费（配 `api_key`）时用它。
pub const SUBSCRIPTION_ENV: &str = "DOCK_CHATGPT_SUBSCRIPTION";

/// 订阅通道是不是被开关关掉了。
pub fn subscription_disabled() -> bool {
    match std::env::var(SUBSCRIPTION_ENV) {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "off" | "0" | "false" | "no")
        }
        Err(_) => false,
    }
}

/// provider 插件：把自己注册进 `"auth"`。
///
/// 活动账号不进内存——provider 每次都读盘（`credentials/chatgpt/active`），
/// 另一个 Dock 进程登录 / 登出 / 切账号，这边不用重启就看得到。
pub fn chatgpt_auth() -> Plugin {
    plugin("chatgpt-auth", Inject::from([AUTH]), |ctx, _: &()| {
        if subscription_disabled() {
            tracing::info!("{SUBSCRIPTION_ENV} 已关：订阅通道不挂载，走 api_key 的模型不受影响。");
            return Ok(None);
        }
        if let Some(manager) = ctx.get::<AuthManager>(AUTH) {
            let provider = Arc::new(ChatGptProvider::new(CredentialStore::default_store()));
            let registered = manager.register(provider);
            // 模型列表缺了、换过账号、过了一天才在后台重拉；登出了就从目录里拿掉。
            manager.refresh_models_if_stale(PROVIDER_ID, MODELS_MAX_AGE);
            return Ok(Some(registered));
        }
        Ok(None)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(client_id: &'a str, dynamic: bool) -> AuthorizeRequest<'a> {
        AuthorizeRequest {
            redirect_uri: "http://127.0.0.1:1455/auth/callback",
            challenge: "challenge",
            state: "state1",
            nonce: "nonce1",
            host_id: "urn:uuid:abc",
            client_id,
            dynamic_registration: dynamic,
            scope: ScopeMode::Default,
            login_hint: None,
            prompt_consent: false,
        }
    }

    fn query(url: &str) -> std::collections::HashMap<String, String> {
        url::Url::parse(url)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    #[test]
    fn authorize_url_carries_the_required_parameters() {
        let q = query(&authorize_url(
            AUTHORIZE_URL,
            &request(DYNAMIC_CLIENT_ID, true),
        ));
        assert_eq!(q["client_id"], DYNAMIC_CLIENT_ID);
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:1455/auth/callback");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], "state1");
        assert_eq!(q["nonce"], "nonce1");
        assert_eq!(q["agent_name_hint"], AGENT_NAME);
        assert_eq!(q["ext_agent_host_id"], "urn:uuid:abc");
        assert_eq!(q["resource"], RESOURCE);
        assert!(q["scope"].contains(PLAN_SCOPE), "{}", q["scope"]);
    }

    /// 重新授权：正式 client + login_hint，**不带 id_token_hint**。
    #[test]
    fn reauthorize_uses_the_official_client_and_hints() {
        let mut req = request("oaiapp_official", false);
        req.login_hint = Some("a@b.c");
        req.prompt_consent = true;
        let q = query(&authorize_url(AUTHORIZE_URL, &req));
        assert_eq!(q["client_id"], "oaiapp_official");
        assert_eq!(q["login_hint"], "a@b.c");
        // **不带 id_token_hint**：它是真的 ID token，URL 会进日志也会推给客户端。
        assert!(!q.contains_key("id_token_hint"), "{q:?}");
        // 重新授权**不带** agent_name_hint（官方：它只在首次动态注册时带）。
        assert!(!q.contains_key("agent_name_hint"), "{q:?}");
        assert_eq!(q["ext_agent_host_id"], "urn:uuid:abc");
    }

    #[test]
    fn error_codes_map_to_auth_errors() {
        let quota = classify(
            429,
            br#"{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}"#,
        );
        assert!(matches!(quota, Some(AuthError::QuotaExhausted { .. })));

        let unavailable = classify(
            503,
            br#"{"error":{"code":"subscription_sharing_usage_unavailable"}}"#,
        );
        assert!(matches!(
            unavailable,
            Some(AuthError::Unavailable { retry: true, .. })
        ));

        let unsupported = classify(
            400,
            br#"{"error":{"code":"subscription_sharing_unsupported_capability","param":"max_output_tokens"}}"#,
        );
        match unsupported {
            Some(AuthError::Unsupported { param }) => {
                assert_eq!(param.as_deref(), Some("max_output_tokens"))
            }
            other => panic!("{other:?}"),
        }

        assert!(matches!(
            classify(401, b"{}"),
            Some(AuthError::NeedsLogin { .. })
        ));
        // 不是鉴权问题就归 None。
        assert!(classify(200, b"{}").is_none());
    }

    /// 官方「Errors and recovery」里那几条 403：说出限制，不说成「没资格」，也不重试。
    #[test]
    fn policy_rejections_say_what_was_rejected() {
        match classify(
            403,
            br#"{"error":{"code":"subscription_sharing_route_not_supported"}}"#,
        ) {
            Some(AuthError::Rejected { detail, .. }) => {
                assert!(detail.contains("/v1/responses"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            classify(
                403,
                br#"{"error":{"code":"chatpass_v2_scope_not_authorized"}}"#
            ),
            Some(AuthError::Rejected { .. })
        ));
        // 没有码的 403：把上游的 message 带出来（比如地区限制）。
        match classify(
            403,
            br#"{"error":{"message":"unsupported_country_region_territory"}}"#,
        ) {
            Some(AuthError::Rejected { code, detail }) => {
                assert_eq!(code, "http_403");
                assert!(detail.contains("unsupported_country"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// token 端点的错误 body 进提示、日志之前，名字带 token 的值一律抹掉；成功响应
    /// 缺字段时**根本不带 body**——那份 body 里就是 access / id token。
    #[test]
    fn token_errors_never_carry_tokens() {
        let leaked = json!({
            "access_token": "at-SECRET",
            "id_token": "it-SECRET",
            "expires_in": 3600,
        });
        let err = ChatGptProvider::exchange_parts(&leaked).unwrap_err();
        let text = AuthError::from(err).message();
        assert!(!text.contains("SECRET"), "{text}");
        assert!(text.contains("refresh_token"), "{text}");

        let redacted = redact_tokens(&json!({
            "error": "invalid_request",
            "nested": { "refresh_token": "rt-SECRET" },
            "list": [{ "IdToken": "x-SECRET" }],
        }))
        .to_string();
        assert!(!redacted.contains("SECRET"), "{redacted}");
        assert!(redacted.contains("invalid_request"), "{redacted}");
    }

    /// state 对不上 = 这个回调不是我们这次请求换来的 → 中止。
    #[test]
    fn a_mismatched_state_is_refused() {
        assert!(check_state("st-expected", "st-expected").is_ok());
        let err = check_state("st-expected", "st-other").expect_err("state 对不上必须拒绝");
        assert!(matches!(err, AuthError::NeedsLogin { .. }), "{err:?}");
        assert!(err.message().contains("state"), "{}", err.message());
        // 空的 state 同样不对。
        assert!(check_state("st-expected", "").is_err());
    }

    /// 回环回调：真的起一个监听，用真的 HTTP 请求打进去，看能不能解析出来。
    #[tokio::test]
    async fn the_loopback_callback_parses_code_and_state() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            oauth::accept_callback(listener, CALLBACK_PATH, SERVICE_NAME, |p| {
                parse_callback(p, "st1", ScopeMode::Default)
            })
            .await
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /auth/callback?code=abc123&state=st1&client_id=oaiapp_x HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let (callback, _page) = server.await.unwrap().expect("回调应当被接受");
        assert_eq!(callback.code, "abc123");
        assert_eq!(callback.state, "st1");
        assert_eq!(callback.client_id.as_deref(), Some("oaiapp_x"));
    }

    /// 套餐那条权限给不了时，授权页「继续」是灰的，用户只能点取消 → 回调带
    /// `error=access_denied`。官方：校验 state、停下、别换码，转到「ChatGPT plan use
    /// isn't enabled」那条路。早先只回一句「授权被拒绝（access_denied）」，
    /// 用户不知道是套餐的事，也不知道下一步。
    #[tokio::test]
    async fn a_denied_plan_grant_says_what_to_do() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        /// 起一个回调监听，打一条 `query` 进去，拿回（解析结果，浏览器看到的页面）。
        async fn deliver(query: &str, scope: ScopeMode) -> (Result<Callback, AuthError>, String) {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                oauth::accept_callback(listener, CALLBACK_PATH, SERVICE_NAME, |p| {
                    parse_callback(p, "st1", scope)
                })
                .await
            });
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            stream
                .write_all(
                    format!("GET /auth/callback?{query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
            // 回调成立时页面留着等换码结果：这里不换码，直接丢掉，连接随之关闭。
            let parsed = server
                .await
                .unwrap()
                .map(|(callback, _page)| callback)
                .map_err(|CallbackError(e, _)| e);
            let mut page = String::new();
            stream.read_to_string(&mut page).await.unwrap();
            (parsed, page)
        }

        // 要了套餐额度、被拒：说是套餐的事，给出三条路；浏览器那页也这么说。
        let (parsed, page) = deliver("error=access_denied&state=st1", ScopeMode::Default).await;
        let err = parsed.expect_err("拒绝了就不能当成功");
        assert!(
            matches!(&err, AuthError::Rejected { code, .. } if code == "access_denied"),
            "{err:?}"
        );
        assert!(!err.retryable(), "套餐不够，重试没用");
        let msg = err.message();
        for want in ["套餐", "Go", "/auth login identity", "api_key"] {
            assert!(msg.contains(want), "缺「{want}」：{msg}");
        }
        assert!(
            page.starts_with("HTTP/1.1 400") && page.contains("套餐额度不可用"),
            "{page}"
        );
        // 页面上是分条的下一步，命令用反引号标出来（页面渲染成等宽胶囊）。
        assert!(page.contains("`/auth login identity`"), "{page}");

        // 只要身份也被拒：那就是用户自己取消的，别扯套餐。
        let (parsed, _) = deliver("error=access_denied&state=st1", ScopeMode::IdentityOnly).await;
        let msg = parsed.unwrap_err().message();
        assert!(!msg.contains("套餐") && msg.contains("取消"), "{msg}");

        // state 对不上：先按「不是这次的回调」中止，不把它当成这次的拒绝。
        let (parsed, _) = deliver("error=access_denied&state=other", ScopeMode::Default).await;
        let msg = parsed.unwrap_err().message();
        assert!(msg.contains("state") && !msg.contains("套餐"), "{msg}");

        // 正常回调：先发的前半页只说「正在连接」，不说「完成」——换码还没做。
        let (parsed, page) =
            deliver("code=c&state=st1&client_id=oaiapp_x", ScopeMode::Default).await;
        assert_eq!(parsed.expect("正常回调").code, "c");
        assert!(
            page.starts_with("HTTP/1.1 200") && page.contains("正在连接 ChatGPT"),
            "{page}"
        );
        assert!(!page.contains("授权完成"), "{page}");
    }

    /// `/v1/models`：只要 `visibility == "list"` 的，保持上游顺序；上下文窗口、
    /// 推理档位、能不能收图这些文档没承诺的字段有就用、没有就空着。
    /// （形状取自真实响应，删掉了无关字段。）
    #[test]
    fn the_model_list_keeps_listed_models_in_order_with_their_metadata() {
        let v = json!({ "models": [
            { "slug": "gpt-reserve", "display_name": "GPT-Reserve", "visibility": "hide",
              "context_window": 272000 },
            { "slug": "gpt-6-luna", "display_name": "GPT-6-Luna", "visibility": "list",
              "description": "Fast and affordable model for easier tasks.",
              "context_window": 272000, "max_context_window": 872000,
              "default_reasoning_level": "medium",
              "supported_reasoning_levels": [
                  { "effort": "low", "description": "…" },
                  { "effort": "max", "description": "…" }
              ],
              "input_modalities": ["text", "image"] },
            { "slug": "gpt-5.6-terra", "visibility": "list" },
            { "display_name": "没有 slug", "visibility": "list" }
        ]});
        let list = parse_models(&v);
        let slugs: Vec<&str> = list.iter().map(|m| m.slug.as_str()).collect();
        assert_eq!(
            slugs,
            ["gpt-6-luna", "gpt-5.6-terra"],
            "隐藏的、没 slug 的不要，顺序照旧"
        );

        let luna = &list[0];
        assert_eq!(luna.display_name.as_deref(), Some("GPT-6-Luna"));
        assert_eq!(luna.context_window, Some(272_000), "用默认窗口，不用 max");
        assert_eq!(luna.default_reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(
            luna.reasoning_efforts.as_deref(),
            Some(&["low".to_string(), "max".to_string()][..])
        );
        assert_eq!(luna.supports_images, Some(true));

        let terra = &list[1];
        assert_eq!(terra.context_window, None);
        assert_eq!(terra.reasoning_efforts, None);
        assert_eq!(terra.supports_images, None);
    }

    /// 浏览器那一页最后停在哪：连上了（带账号、套餐额度开没开）、还差一步、没连上。
    #[test]
    fn the_page_says_what_the_round_ended_with() {
        let json =
            |step: Result<Round, AuthError>| serde_json::to_value(round_page(&step)).unwrap();

        let mut cred = credential("oaiapp_page", 3600);
        let v = json(Ok(Round::Done(Box::new(cred.clone()))));
        assert_eq!(v["state"], "ok");
        assert_eq!(v["title"], "已连接 ChatGPT");
        assert_eq!(v["account"], "a@b.c");
        assert_eq!(v["chips"][0]["text"], "套餐额度已开");
        assert_eq!(
            v["link"]["href"], USAGE_URL,
            "官方 UI 规范：给 Manage usage 入口"
        );

        // 只有身份：登录留着，但说清楚不能拿它推理，不给用量入口。
        cred.scopes = vec!["openid".into(), "email".into()];
        let v = json(Ok(Round::Done(Box::new(cred))));
        assert_eq!(v["chips"][0]["tone"], "warn");
        assert!(
            v["steps"][0].as_str().unwrap().contains("`/auth login`"),
            "{v}"
        );
        assert!(v.get("link").is_none(), "{v}");

        let v = json(Ok(Round::Again("oaiapp_x".into())));
        assert_eq!(v["state"], "next");
        assert!(
            v["detail"].as_str().unwrap().contains("再打开一页授权"),
            "{v}"
        );

        // 被拒：只给说明，不带「上游拒绝了这个请求」那个写给推理请求的前缀。
        let v = json(Err(AuthError::Rejected {
            code: "access_denied".into(),
            detail: "说明".into(),
        }));
        assert_eq!(v["state"], "fail");
        assert_eq!(v["detail"], "说明");
    }

    /// 假浏览器：一打开授权 URL 就直接打回调（带 code / state / 正式 client id），
    /// 把这一页最后收到的全文记下来。
    #[derive(Default)]
    struct FakeBrowser {
        pages: std::sync::Mutex<Vec<tokio::task::JoinHandle<String>>>,
    }

    impl LoginUi for FakeBrowser {
        fn open_url(&self, url: &str) {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let url = url::Url::parse(url).unwrap();
            let q: std::collections::HashMap<String, String> =
                url.query_pairs().into_owned().collect();
            let redirect = url::Url::parse(&q["redirect_uri"]).unwrap();
            let addr = format!("127.0.0.1:{}", redirect.port().unwrap());
            let target = format!(
                "{}?code=code-1&state={}&client_id=oaiapp_new",
                redirect.path(),
                q["state"]
            );
            let page = tokio::spawn(async move {
                let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
                s.write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes())
                    .await
                    .unwrap();
                let mut out = String::new();
                s.read_to_string(&mut out).await.unwrap();
                out
            });
            self.pages.lock().unwrap().push(page);
        }

        fn status(&self, _: &str) {}
    }

    /// 端到端：首轮换码被拒，第一页要说「还差一步」（浏览器马上会开第二页）；
    /// 第二轮还是被拒，第二页说「没有连上」，原因和 Dock 里报的一样。
    /// 早先第一页一律写「授权完成，可以关闭此窗口」，紧接着又弹出一页授权。
    #[tokio::test]
    async fn the_browser_page_follows_each_round() {
        let _env = cordis_base::test_env::scoped().home();
        let idp = FakeIdp::start(|path| match path {
            "/token" => (400, json!({ "error": "invalid_grant" }).to_string()),
            _ => (404, "{}".into()),
        })
        .await;
        let provider = ChatGptProvider::new(store()).with_test_server(&idp.base);
        let browser = FakeBrowser::default();

        let err = provider
            .login_impl(&browser, LoginRequest::default())
            .await
            .expect_err("两轮都被拒");
        assert_eq!(idp.hits("/token"), 2);

        let handles = std::mem::take(&mut *browser.pages.lock().unwrap());
        assert_eq!(handles.len(), 2, "开了两次授权页");
        let mut results = Vec::new();
        for page in handles {
            let page = page.await.unwrap();
            let (_, result) = page
                .split_once("<script>settle(")
                .unwrap_or_else(|| panic!("每一页都要等到结果：{page}"));
            results.push(result.to_string());
        }
        assert!(
            results[0].contains("\"state\":\"next\"") && results[0].contains("还差一步"),
            "{}",
            results[0]
        );
        assert!(
            results[1].contains("\"state\":\"fail\"") && results[1].contains("没有连上 ChatGPT"),
            "{}",
            results[1]
        );
        assert!(err.message().contains("换码失败"), "{}", err.message());
        assert!(results[1].contains("换码失败"), "{}", results[1]);
    }

    fn store() -> CredentialStore {
        CredentialStore::new(tempfile::tempdir().unwrap().keep())
    }

    /// **登出不丢注册**（官方："Revoking this session does not delete the registered
    /// client"）：登出后再登录，复用这个账号的正式 client id，不再走首登。
    ///
    /// 早先登出把凭据文件整个删了，`issued` 又在登录成功时清过——下次登录两头都
    /// 找不到正式 client id，只能拿 `dynamic_agent_client` 再注册一遍。
    #[tokio::test]
    async fn logout_keeps_the_registration_for_the_next_sign_in() {
        let store = store();
        let cred = credential("oaiapp_kept", 3600);
        store.save(&cred).unwrap();
        store.set_active("oaiapp_kept").unwrap();
        let provider = ChatGptProvider::new(store.clone());

        let outcome = provider.logout(&key_of(&cred), false).await.expect("登出");
        assert_eq!(outcome, LogoutOutcome::Revoked);

        // token 清掉了：推理拿不到 lease，而且说的是「登出了」。
        let on_disk = store.load("oaiapp_kept").expect("注册还在");
        assert!(!on_disk.signed_in());
        assert!(on_disk.access_token.is_empty() && on_disk.id_token.is_empty());
        let err = provider
            .lease(&key_of(&cred), Duration::from_secs(60))
            .await
            .unwrap_err();
        assert!(err.message().contains("登出"), "{}", err.message());

        // 再登录：复用正式 client id、带邮箱提示、认得是同一个人。
        let reg = registration_for(&store, &LoginTarget::Active).unwrap();
        assert_eq!(reg.client_id.as_deref(), Some("oaiapp_kept"));
        assert_eq!(reg.email.as_deref(), Some("a@b.c"));
        assert_eq!(reg.subject.as_deref(), Some("sub-1"));

        // 账号表里还看得到它（登出了，不是没了）。
        let accounts = provider.accounts();
        assert_eq!(accounts.len(), 1);
        assert!(!accounts[0].signed_in);
        assert!(!accounts[0].plan_usage);
        assert!(accounts[0].active);
    }

    /// 「新加一个账号」走首登；首登回调拿到了正式 client id 但换码没成的，接着用它。
    #[test]
    fn a_new_account_registers_unless_a_registration_is_pending() {
        let store = store();
        store.save(&credential("oaiapp_old", 3600)).unwrap();
        store.set_active("oaiapp_old").unwrap();
        assert_eq!(
            registration_for(&store, &LoginTarget::New).unwrap(),
            Registration::default()
        );
        store.remember_issued("oaiapp_pending", None).unwrap();
        assert_eq!(
            registration_for(&store, &LoginTarget::New)
                .unwrap()
                .client_id
                .as_deref(),
            Some("oaiapp_pending")
        );
        // 指定账号：本机没有就报错，不悄悄注册一个新的。
        assert!(registration_for(&store, &LoginTarget::Account("oaiapp_nope".into())).is_err());
    }

    /// 活动账号**每次读盘**：另一个进程切了账号 / 登了新号，这边不用重启就看到。
    /// 早先活动账号只在挂载时从盘上读一次进内存，两个进程各认各的。
    #[test]
    fn the_active_account_is_read_from_disk_every_time() {
        let store = store();
        let here = ChatGptProvider::new(store.clone());
        let there = ChatGptProvider::new(store.clone());
        assert!(here.active_account().is_none());

        store.save(&credential("oaiapp_a", 3600)).unwrap();
        store.save(&credential("oaiapp_b", 3600)).unwrap();
        there.set_active("oaiapp_a").unwrap();
        assert_eq!(here.active_account().unwrap().client_id, "oaiapp_a");
        there.set_active("oaiapp_b").unwrap();
        assert_eq!(here.active_account().unwrap().client_id, "oaiapp_b");
    }

    /// 登出了的账号不能直接切成活动账号（官方：先完成并验过 OAuth）。
    #[test]
    fn a_signed_out_account_cannot_become_active() {
        let store = store();
        store
            .save(&credential("oaiapp_out", 3600).without_tokens())
            .unwrap();
        let provider = ChatGptProvider::new(store);
        let err = provider.set_active("oaiapp_out").unwrap_err();
        assert!(matches!(err, AuthError::NeedsLogin { .. }), "{err:?}");
        assert!(matches!(
            provider.set_active("oaiapp_missing").unwrap_err(),
            AuthError::Rejected { .. }
        ));
    }

    /// 导入：host id 用本机的；本机已有的同一 client id 属于别人时不覆盖。
    #[test]
    fn import_takes_over_with_the_local_host_id() {
        let _env = cordis_base::test_env::scoped().home();
        let store = store();
        let provider = ChatGptProvider::new(store.clone());
        let mut cred = credential("oaiapp_vm", 3600);
        cred.ext_agent_host_id = "urn:uuid:laptop".into();
        let raw = serde_json::to_string(&cred).unwrap();
        let key = provider.import(&raw).expect("导入");
        assert_eq!(key.client_id, "oaiapp_vm");
        let saved = store.load("oaiapp_vm").unwrap();
        assert_ne!(saved.ext_agent_host_id, "urn:uuid:laptop");
        assert_eq!(saved.ext_agent_host_id, load_or_create_host_id().unwrap());
        assert_eq!(provider.active_account().unwrap().client_id, "oaiapp_vm");

        let mut other = credential("oaiapp_vm", 3600);
        other.subject = "someone-else".into();
        let err = provider
            .import(&serde_json::to_string(&other).unwrap())
            .unwrap_err();
        assert!(err.message().contains("另一个账号"), "{}", err.message());
        // 登出后的那份（没有 refresh token）导进来没用。
        let out = credential("oaiapp_vm2", 3600).without_tokens();
        assert!(provider
            .import(&serde_json::to_string(&out).unwrap())
            .is_err());
    }

    /// 并发取 token：**只刷一次**。
    ///
    /// 两个任务同时要 lease → 都发现 access token 快过期 → 都去刷 → refresh
    /// token 轮换会让第二份直接作废（`refresh_token_reused`），凭据就废了。
    /// 靠跨进程文件锁 + 盘上代数：拿到锁的那个刷，另一个拿到的是刷完的那份。
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_leases_refresh_only_once() {
        let server = FakeIdp::start(FakeIdp::rotating()).await;
        let store = store();
        // 快过期（60 秒后）→ 一定会走刷新。
        let cred = credential("oaiapp_concurrent", 60);
        store.save(&cred).unwrap();

        let provider = Arc::new(ChatGptProvider::new(store.clone()).with_test_server(&server.base));
        let acct = key_of(&cred);

        let a = {
            let p = provider.clone();
            let acct = acct.clone();
            tokio::spawn(async move { p.lease(&acct, Duration::from_secs(120)).await })
        };
        let b = {
            let p = provider.clone();
            let acct = acct.clone();
            tokio::spawn(async move { p.lease(&acct, Duration::from_secs(120)).await })
        };
        let (a, b) = (a.await.unwrap(), b.await.unwrap());
        assert!(a.is_ok(), "{a:?}");
        assert!(b.is_ok(), "{b:?}");

        // **只刷了一次**：两次 lease 共用同一份刷新的结果。
        assert_eq!(server.hits("/token"), 1);
        let on_disk = store.load("oaiapp_concurrent").unwrap();
        assert_eq!(on_disk.generation, 2, "只该涨一代");
    }

    /// 盘上已经是新代数（别的进程刷过了）→ 直接用那份，不再发网络请求。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_newer_credential_on_disk_is_used_without_refreshing() {
        let server = FakeIdp::start(FakeIdp::rotating()).await;
        let store = store();
        let mut fresh = credential("oaiapp_newer", 60);
        fresh.generation = 7;
        fresh.access_token = "access-new".into();
        store.save(&fresh).unwrap();

        let provider = ChatGptProvider::new(store.clone()).with_test_server(&server.base);
        let stale = credential("oaiapp_newer", 60); // 代数 1
        let refreshed = provider.refresh_locked(&stale).await.expect("用盘上那份");
        assert_eq!(refreshed.generation, 7);
        assert_eq!(refreshed.access_token, "access-new");
        assert_eq!(server.hits("/token"), 0, "不该发刷新请求");
    }

    /// 刷新响应没带 refresh token（RFC 6749 允许）：**沿用旧的**，不报错、不丢。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_refresh_without_a_new_refresh_token_keeps_the_old_one() {
        let server = FakeIdp::start(|path| match path {
            "/token" => (
                200,
                json!({ "access_token": "access-2", "expires_in": 3600 }).to_string(),
            ),
            _ => (404, "{}".into()),
        })
        .await;
        let store = store();
        store.save(&credential("oaiapp_norotate", 60)).unwrap();
        let provider = ChatGptProvider::new(store.clone()).with_test_server(&server.base);
        let lease = provider
            .lease(
                &key_of(&credential("oaiapp_norotate", 60)),
                Duration::from_secs(120),
            )
            .await
            .expect("刷新成功");
        assert_eq!(lease.bearer, "access-2");
        let on_disk = store.load("oaiapp_norotate").unwrap();
        assert_eq!(on_disk.refresh_token, "refresh-old");
        assert_eq!(on_disk.id_token, "id-token");
    }

    /// 刷新被判死（官方那几个码）：清 token、留注册，报「要重新登录」。
    /// 只是 5xx 不清（官方：别因为临时故障抹掉凭据）。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_dead_refresh_clears_tokens_but_a_flaky_one_does_not() {
        let dead =
            FakeIdp::start(|_| (400, json!({ "error": "refresh_token_reused" }).to_string())).await;
        let store = store();
        let cred = credential("oaiapp_dead", 60);
        store.save(&cred).unwrap();
        let provider = ChatGptProvider::new(store.clone()).with_test_server(&dead.base);
        let err = provider
            .lease(&key_of(&cred), Duration::from_secs(120))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::NeedsLogin { .. }), "{err:?}");
        let on_disk = store.load("oaiapp_dead").expect("注册还在");
        assert!(!on_disk.signed_in(), "作废的 token 该清掉");
        assert_eq!(on_disk.client_id, "oaiapp_dead");

        let flaky = FakeIdp::start(|_| (502, "{}".into())).await;
        let store = self::store();
        store.save(&credential("oaiapp_flaky", 60)).unwrap();
        let provider = ChatGptProvider::new(store.clone()).with_test_server(&flaky.base);
        let err = provider
            .lease(
                &key_of(&credential("oaiapp_flaky", 60)),
                Duration::from_secs(120),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::Transport(_)), "{err:?}");
        assert!(
            store.load("oaiapp_flaky").unwrap().signed_in(),
            "临时故障不清凭据"
        );
    }

    /// 列模型时 token 过期了：先刷新再列，不叫用户重新登录。
    #[tokio::test(flavor = "multi_thread")]
    async fn models_refreshes_an_expired_token_first() {
        let server = FakeIdp::start(FakeIdp::rotating()).await;
        let store = store();
        // 已经过期。
        let mut cred = credential("oaiapp_models", 0);
        cred.expires_at = now_unix().saturating_sub(10);
        store.save(&cred).unwrap();
        let provider = ChatGptProvider::new(store.clone()).with_test_server(&server.base);
        let list = provider.models(&key_of(&cred)).await.expect("列模型");
        assert_eq!(server.hits("/token"), 1, "过期了就该先刷新");
        assert_eq!(
            list.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>(),
            vec!["gpt-visible"],
            "visibility 不是 list 的不显示"
        );
        assert_eq!(
            server.last_auth("/v1/models").as_deref(),
            Some("Bearer access-refreshed"),
            "列模型要带刷新后的 token"
        );
    }

    /// 撤销遇到 5xx：退避重试（官方），成功了就算 Revoked；本地 token 清掉、注册留着。
    #[tokio::test(flavor = "multi_thread")]
    async fn revocation_retries_server_errors() {
        let server = FakeIdp::start_counting(|path, n| match path {
            "/revoke" if n == 0 => (503, String::new()),
            "/revoke" => (200, String::new()),
            _ => (404, "{}".into()),
        })
        .await;
        let store = store();
        let cred = credential("oaiapp_revoke", 3600);
        store.save(&cred).unwrap();
        let provider = ChatGptProvider::new(store.clone()).with_test_server(&server.base);
        let outcome = provider.logout(&key_of(&cred), true).await.expect("登出");
        assert_eq!(outcome, LogoutOutcome::Revoked);
        assert_eq!(server.hits("/revoke"), 2);
        assert!(!store.load("oaiapp_revoke").unwrap().signed_in());
    }

    /// 本地假 IdP：discovery / token / revoke / v1/models，按路径数次数。
    struct FakeIdp {
        base: String,
        hits: Arc<std::sync::Mutex<Vec<Hit>>>,
    }

    type Responder = dyn Fn(&str, usize) -> (u16, String) + Send + Sync;
    /// 一次请求：路径 + Authorization 头。
    type Hit = (String, Option<String>);

    impl FakeIdp {
        /// 正常刷新：每次给新的 access token 与轮换后的 refresh token。
        fn rotating() -> impl Fn(&str) -> (u16, String) + Send + Sync + 'static {
            |path| match path {
                "/token" => (
                    200,
                    json!({
                        "access_token": "access-refreshed",
                        "refresh_token": "refresh-rotated",
                        "expires_in": 3600,
                    })
                    .to_string(),
                ),
                "/v1/models" => (
                    200,
                    json!({ "models": [
                        { "slug": "gpt-visible", "display_name": "Visible", "visibility": "list" },
                        { "slug": "gpt-hidden", "visibility": "hide" },
                    ]})
                    .to_string(),
                ),
                _ => (404, "{}".into()),
            }
        }

        async fn start(f: impl Fn(&str) -> (u16, String) + Send + Sync + 'static) -> Self {
            Self::start_counting(move |path, _| f(path)).await
        }

        async fn start_counting(
            f: impl Fn(&str, usize) -> (u16, String) + Send + Sync + 'static,
        ) -> Self {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let base = format!("http://{addr}");
            let hits: Arc<std::sync::Mutex<Vec<Hit>>> = Default::default();
            let f: Arc<Responder> = Arc::new(f);
            let sink = hits.clone();
            let revoke = format!("{base}/revoke");
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let sink = sink.clone();
                    let f = f.clone();
                    let revoke = revoke.clone();
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 8192];
                        let n = socket.read(&mut buf).await.unwrap_or(0);
                        let head = String::from_utf8_lossy(&buf[..n]).to_string();
                        let path = head
                            .lines()
                            .next()
                            .and_then(|l| l.split_whitespace().nth(1))
                            .unwrap_or("")
                            .to_string();
                        let auth = head.lines().find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("authorization")
                                .then(|| v.trim().to_string())
                        });
                        let (status, body) = if path == "/.well-known/openid-configuration" {
                            (200, json!({ "revocation_endpoint": revoke }).to_string())
                        } else {
                            let n = {
                                let mut hits = sink.lock().unwrap();
                                let n = hits.iter().filter(|(p, _)| *p == path).count();
                                hits.push((path.clone(), auth));
                                n
                            };
                            f(&path, n)
                        };
                        let resp = format!(
                            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(resp.as_bytes()).await;
                        let _ = socket.flush().await;
                    });
                }
            });
            Self { base, hits }
        }

        fn hits(&self, path: &str) -> usize {
            self.hits
                .lock()
                .unwrap()
                .iter()
                .filter(|(p, _)| p == path)
                .count()
        }

        fn last_auth(&self, path: &str) -> Option<String> {
            self.hits
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|(p, _)| p == path)
                .and_then(|(_, a)| a.clone())
        }
    }

    fn credential(client_id: &str, expires_in: u64) -> Credential {
        let now = now_unix();
        Credential {
            schema_version: crate::auth::credentials::SCHEMA_VERSION,
            generation: 1,
            email: Some("a@b.c".into()),
            issuer: ISSUER.to_string(),
            subject: "sub-1".into(),
            client_id: client_id.into(),
            id_token: "id-token".into(),
            access_token: "access-old".into(),
            refresh_token: "refresh-old".into(),
            expires_at: now + expires_in,
            // 带套餐 scope：没有它 lease() 会先判定「没资格」，根本走不到刷新。
            scopes: vec!["openid".into(), "email".into(), PLAN_SCOPE.into()],
            saved_at: now,
            ext_agent_host_id: "urn:uuid:test".into(),
        }
    }
}
