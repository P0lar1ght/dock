//! Grok 订阅通道（SuperGrok / X Premium）的 provider 插件。
//!
//! xAI 让 SuperGrok / X Premium 订阅在第三方 agent 里用（它点名接入的有 OpenCode、
//! Kilo Code、Hermes Agent……）：在 `auth.x.ai` 做 OAuth，拿到的 access token 当
//! Bearer 打**公开 API** `https://api.x.ai/v1/responses`，用量算在订阅额度里。
//!
//! `auth.x.ai` 没有自助注册 client 的口子（discovery 里没有 `registration_endpoint`），
//! 那几家接入都复用 Grok CLI 的公开 client id，用 `referrer=<应用名>` 告诉 xAI 是谁
//! 发起的登录。Dock 照做：`referrer=dock`。
//!
//! **不做的事**：不打 Grok Build 自用的 `cli-chat-proxy.grok.com`，不带它的身份头
//! （`X-XAI-Token-Auth`、`x-grok-client-identifier`……），不读 `~/.grok/auth.json`。
//!
//! 登录走 device code（RFC 8628）：不起回环监听，终端、桌面端、VPS 一个样——打开
//! `verification_uri_complete`（用户码已经填好），用户在浏览器里点同意，Dock 轮询
//! token 端点。实测记录见 `docs/GROK-SUBSCRIPTION.md`。
//!
//! 凭据落在 `$DOCK_HOME/credentials/grok/<账号>.json`。所有账号共用同一个 OAuth
//! client，所以文件**按账号（ID token 的 `sub`）分**：[`Credential::client_id`] 在
//! 这一家存的是账号 id，真正发给 `auth.x.ai` 的 client id 永远是 [`CLIENT_ID`]。

use std::sync::Arc;
use std::time::{Duration, Instant};

use cordis::{plugin, Inject, Plugin};
use serde_json::{json, Value};

use cordis_base::auth::{
    AccountKey, AccountSummary, AuthError, AuthFuture, AuthProvider, Lease, LoginRequest,
    LoginTarget, LoginUi, LogoutOutcome, PlanUsage, ProviderInfo, RemoteModel, Retry, ScopeMode,
    UsagePeriod, WirePolicy,
};

use crate::auth::credentials::{now_unix, sanitize, Credential, CredentialError, CredentialStore};
use crate::auth::jwt::{verify_jwt, Expected, Jwks};
use crate::auth::oauth::FileLock;
use crate::auth::{grok_policy, manager::AuthManager};
use crate::names::AUTH;

/// provider id：模型条目写 `auth = "grok"`，目录里的模型是 `grok/<slug>`。
pub const PROVIDER_ID: &str = "grok";
/// ID token 的签发方。
pub const ISSUER: &str = "https://auth.x.ai";
pub const DEVICE_URL: &str = "https://auth.x.ai/oauth2/device/code";
pub const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
pub const REVOKE_URL: &str = "https://auth.x.ai/oauth2/revoke";
pub const JWKS_URL: &str = "https://auth.x.ai/.well-known/jwks.json";
/// 推理基址：公开 API。**不是** Grok Build 自用的 `cli-chat-proxy.grok.com`。
pub const API_BASE: &str = "https://api.x.ai/v1";
/// 订阅额度（本周期已用百分比、周期起止）。
///
/// 公开 API 上查不到订阅额度，只有 Grok Build 自用的后端有。这里是**唯一的例外**：
/// 只读、只带 Bearer（实测不带 Grok Build 的身份头也回 200），推理不走这里。
/// xAI 点名接入的 OpenClaw 查额度也是这个地址。
pub const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
/// 套餐名（`subscription_tier_display`，如「SuperGrok」）。同上，只读。
pub const SETTINGS_URL: &str = "https://cli-chat-proxy.grok.com/v1/settings";
/// Grok CLI 的公开 OAuth client（没有 secret）。xAI 点名接入的 OpenCode / Kilo Code
/// 用的也是它；`auth.x.ai` 不给第三方自己注册。
pub const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
/// 授权请求里告诉 xAI 是谁发起的（OpenCode 带 `opencode`、Kilo 带 `kilocode`）。
pub const REFERRER: &str = "dock";
/// 订阅推理要的两条 scope。少了任何一条就只是「登录了」，不能拿订阅额度推理。
pub const PLAN_SCOPES: &[&str] = &["grok-cli:access", "api:access"];
/// 要**订阅额度**时的 scope（与 OpenCode 一致）。
pub const SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "offline_access",
    "grok-cli:access",
    "api:access",
];
/// **只要身份**的 scope：不拿订阅额度。
pub const IDENTITY_SCOPES: &[&str] = &["openid", "profile", "email", "offline_access"];

fn scopes_for(mode: ScopeMode) -> &'static [&'static str] {
    match mode {
        ScopeMode::Default => SCOPES,
        ScopeMode::IdentityOnly => IDENTITY_SCOPES,
    }
}

/// 启动时模型列表超过这么久才重拉（登录、切账号时总是重拉）。
const MODELS_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// 等用户确认用户码的上限。上游给的用户码能用 30 分钟，这里只等 10 分钟——
/// 与 ChatGPT 那条、与提示里写的「10 分钟内有效」一致。
const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
/// 上游没说轮询间隔时用的（RFC 8628：默认 5 秒）。
const DEFAULT_INTERVAL: u64 = 5;
/// `slow_down` 一次加多少秒（RFC 8628）。
const SLOW_DOWN_STEP: u64 = 5;
/// 轮询时连续几次连不上就放弃（网络抖一两下不算）。
const MAX_POLL_TRANSPORT_FAILURES: u32 = 5;
/// token / 撤销 / JWKS 请求的**总超时**。
const TOKEN_TIMEOUT: Duration = Duration::from_secs(30);
/// 刷新 / 登出时最多等锁多久。
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// 撤销最多试几次（网络失败 / 5xx 退避重试）。
const REVOKE_ATTEMPTS: u32 = 3;
/// 上游没给 `expires_in` 时按多久算（实测给的是 21600，保守一点）。
const DEFAULT_EXPIRES_IN: u64 = 3600;

/// 刷新时这些错误码 = **这份凭据不能再用了**：清掉 token，要用户重新登录。
/// 实测撤销过的、乱写的 refresh token 都回 400 `invalid_grant`。
const DEAD_REFRESH_CODES: &[&str] = &["invalid_grant", "invalid_token", "unauthorized_client"];

/// 端点。生产是常量；测试指到本地假服务器。
#[derive(Clone, Debug)]
struct Endpoints {
    device: String,
    token: String,
    revoke: String,
    jwks: String,
    api: String,
    billing: String,
    settings: String,
    /// 模型目录（models.dev）：xAI 的模型列表不给推理档位，从这里补。
    catalog: String,
}

impl Endpoints {
    fn official() -> Self {
        Self {
            device: DEVICE_URL.into(),
            token: TOKEN_URL.into(),
            revoke: REVOKE_URL.into(),
            jwks: JWKS_URL.into(),
            api: API_BASE.into(),
            billing: BILLING_URL.into(),
            settings: SETTINGS_URL.into(),
            catalog: crate::llm::http::catalog::MODELS_DEV_URL.into(),
        }
    }
}

/// Grok 订阅通道的实现。
pub struct GrokProvider {
    store: CredentialStore,
    policy: WirePolicy,
    client: reqwest::Client,
    endpoints: Endpoints,
    /// 轮询间隔的上下限。生产是「至少 1 秒、不设上限」（上游给 5 秒，`slow_down`
    /// 再加）；测试两头都是 0，不真等。
    min_poll: Duration,
    max_poll: Duration,
    /// 账号 id → 套餐名。套餐名很少变，`/settings` 又是一大份：查到一次就记着。
    plans: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

/// token 端点回来的错误。
///
/// 「上游明确拒绝」（带 OAuth 错误码）与「传输层失败」分开：前者可能意味着要重新
/// 登录，后者只是网络抖了。
#[derive(Debug)]
enum TokenError {
    OAuth {
        code: String,
        detail: String,
        status: u16,
    },
    Transport(String),
}

impl TokenError {
    fn code(&self) -> &str {
        match self {
            TokenError::OAuth { code, .. } => code,
            TokenError::Transport(_) => "",
        }
    }

    fn is_credential_dead(&self) -> bool {
        DEAD_REFRESH_CODES.contains(&self.code())
    }

    fn message(&self) -> String {
        match self {
            TokenError::OAuth {
                code,
                detail,
                status,
            } => format!("token HTTP {status}：{detail}（{code}）"),
            TokenError::Transport(detail) => detail.clone(),
        }
    }
}

impl From<TokenError> for AuthError {
    fn from(e: TokenError) -> AuthError {
        if e.is_credential_dead() {
            AuthError::NeedsLogin {
                reason: format!(
                    "Grok 的登录失效了（{}）：/auth login grok 重新登录。",
                    e.code()
                ),
            }
        } else if e.code() == "invalid_client" {
            // 复用的是 Grok CLI 的公开 client：xAI 不再让第三方用它时就是这个码。
            AuthError::Rejected {
                code: "invalid_client".into(),
                detail: format!(
                    "xAI 不认 Dock 登录用的 client 了（{}）。重新登录也没用：这条通道暂时用不了，\
                     先给模型配 api_key。",
                    e.message()
                ),
            }
        } else {
            AuthError::Transport(e.message())
        }
    }
}

/// device code 端点给的那一份。
#[derive(Debug, Clone)]
struct DeviceGrant {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: u64,
}

impl GrokProvider {
    pub fn new(store: CredentialStore) -> Self {
        Self {
            store,
            policy: grok_policy(),
            client: reqwest::Client::builder()
                .user_agent(concat!("dock/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(20))
                .build()
                .expect("dock user-agent is a valid header value"),
            endpoints: Endpoints::official(),
            min_poll: Duration::from_secs(1),
            max_poll: Duration::MAX,
            plans: Default::default(),
        }
    }

    /// 把全部端点指到 `base` 下的假服务器（**只给测试用**）。
    #[cfg(test)]
    fn with_test_server(mut self, base: &str) -> Self {
        self.endpoints = Endpoints {
            device: format!("{base}/device"),
            token: format!("{base}/token"),
            revoke: format!("{base}/revoke"),
            jwks: format!("{base}/jwks"),
            api: format!("{base}/v1"),
            billing: format!("{base}/proxy/billing?format=credits"),
            settings: format!("{base}/proxy/settings"),
            catalog: format!("{base}/catalog"),
        };
        self.min_poll = Duration::ZERO;
        self.max_poll = Duration::ZERO;
        self
    }

    /// 发一个表单到 token 端点。非 2xx 带出 OAuth 错误码（**不带 body**：错误
    /// 响应里没有 token，但也没别的有用的东西）。
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
            let code = text(&v, "error").unwrap_or_default();
            let detail = text(&v, "error_description")
                .or_else(|| text(&v, "error"))
                .unwrap_or_else(|| "token 请求被拒".into());
            return Err(TokenError::OAuth {
                code,
                detail,
                status: status.as_u16(),
            });
        }
        Ok(v)
    }

    /// 申请 device code。
    async fn start_device(&self, mode: ScopeMode) -> Result<DeviceGrant, AuthError> {
        let scope = scopes_for(mode).join(" ");
        let resp = self
            .client
            .post(&self.endpoints.device)
            .timeout(TOKEN_TIMEOUT)
            .form(&[
                ("client_id", CLIENT_ID),
                ("scope", scope.as_str()),
                ("referrer", REFERRER),
            ])
            .send()
            .await
            .map_err(|e| AuthError::Transport(format!("申请用户码失败：{e}")))?;
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(json!({}));
        if !status.is_success() {
            let err = TokenError::OAuth {
                code: text(&v, "error").unwrap_or_default(),
                detail: text(&v, "error_description")
                    .or_else(|| text(&v, "error"))
                    .unwrap_or_else(|| "申请用户码被拒".into()),
                status: status.as_u16(),
            };
            return Err(match AuthError::from(err) {
                // 这一步还没有凭据，「登录失效」的说法不对：照实报上游的话。
                AuthError::NeedsLogin { .. } | AuthError::Transport(_) => AuthError::Rejected {
                    code: text(&v, "error").unwrap_or_else(|| format!("http_{status}")),
                    detail: text(&v, "error_description").unwrap_or_default(),
                },
                other => other,
            });
        }
        let required = |name: &str| {
            text(&v, name).ok_or_else(|| AuthError::Transport(format!("用户码响应里没有 {name}")))
        };
        Ok(DeviceGrant {
            device_code: required("device_code")?,
            user_code: required("user_code")?,
            verification_uri: required("verification_uri")?,
            verification_uri_complete: text(&v, "verification_uri_complete"),
            expires_in: v
                .get("expires_in")
                .and_then(|n| n.as_u64())
                .filter(|n| *n > 0)
                .unwrap_or(LOGIN_TIMEOUT.as_secs()),
            interval: v
                .get("interval")
                .and_then(|n| n.as_u64())
                .unwrap_or(DEFAULT_INTERVAL),
        })
    }

    /// 轮询 token 端点，直到用户点了同意（或拒了、过期了）。
    async fn poll_device(&self, grant: &DeviceGrant) -> Result<Value, AuthError> {
        let wait_cap = Duration::from_secs(grant.expires_in).min(LOGIN_TIMEOUT);
        let deadline = Instant::now() + wait_cap;
        let expired = || AuthError::NeedsLogin {
            reason: format!(
                "{} 分钟内没确认用户码，这次登录作废了：重新 /auth login grok",
                wait_cap.as_secs().div_ceil(60)
            ),
        };
        let mut interval = grant.interval;
        let mut transport_failures = 0u32;
        loop {
            // RFC 8628：两次轮询之间至少隔 `interval` 秒，第一次也先等。
            let wait = Duration::from_secs(interval)
                .max(self.min_poll)
                .min(self.max_poll);
            tokio::time::sleep(wait).await;
            if Instant::now() >= deadline {
                return Err(expired());
            }
            let polled = self
                .token_request(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", CLIENT_ID),
                    ("device_code", grant.device_code.as_str()),
                ])
                .await;
            match polled {
                Ok(v) => return Ok(v),
                Err(TokenError::OAuth { code, .. }) if code == "authorization_pending" => {}
                Err(TokenError::OAuth { code, .. }) if code == "slow_down" => {
                    interval += SLOW_DOWN_STEP;
                }
                Err(TokenError::OAuth { code, .. })
                    if code == "access_denied" || code == "authorization_denied" =>
                {
                    return Err(AuthError::Rejected {
                        code,
                        detail: "授权页上拒绝了。要用就重新 /auth login grok。".into(),
                    });
                }
                Err(TokenError::OAuth { code, .. }) if code == "expired_token" => {
                    return Err(expired());
                }
                // 网络抖一两下不算：接着等，连着失败才放弃。
                Err(TokenError::Transport(e)) => {
                    transport_failures += 1;
                    if transport_failures >= MAX_POLL_TRANSPORT_FAILURES {
                        return Err(AuthError::Transport(e));
                    }
                    continue;
                }
                Err(other) => {
                    return Err(match AuthError::from(other) {
                        // 还没登上，没有「失效」可言。
                        AuthError::NeedsLogin { reason } => AuthError::Rejected {
                            code: "token_exchange_failed".into(),
                            detail: reason,
                        },
                        e => e,
                    });
                }
            }
            transport_failures = 0;
        }
    }

    /// 授权服务器的公钥集。
    async fn jwks(&self) -> Result<Jwks, String> {
        let resp = self
            .client
            .get(&self.endpoints.jwks)
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
        Jwks::parse(&v)
    }

    /// 这次登录换回来的必须是谁（`None` = 谁都行）。
    ///
    /// 重新登录已有账号时浏览器里登着的要是**同一个人**：授权回来是另一个账号，
    /// 不能悄悄把原来那个盖掉（要加新账号用 `/auth login new grok`）。
    fn expected_subject(&self, target: &LoginTarget) -> Result<Option<String>, AuthError> {
        let of = |cred: Credential| Some(cred.subject).filter(|s| !s.is_empty());
        match target {
            LoginTarget::New => Ok(None),
            LoginTarget::Account(id) => {
                self.store
                    .load(id)
                    .map(of)
                    .map_err(|e| AuthError::NeedsLogin {
                        reason: format!("本机没有这个 Grok 账号：{id}（{e}）"),
                    })
            }
            LoginTarget::Active => match self.store.active() {
                None => Ok(None),
                Some(id) => match self.store.load(&id) {
                    Ok(cred) => Ok(of(cred)),
                    Err(CredentialError::Missing) => Ok(None),
                    Err(e) => Err(AuthError::NeedsLogin {
                        reason: format!("活动账号的凭据读不了：{e}"),
                    }),
                },
            },
        }
    }

    async fn login_impl(
        &self,
        ui: &dyn LoginUi,
        req: LoginRequest,
    ) -> Result<AccountKey, AuthError> {
        let expected = self.expected_subject(&req.target)?;
        let grant = self.start_device(req.scope).await?;
        let open = grant
            .verification_uri_complete
            .clone()
            .unwrap_or_else(|| grant.verification_uri.clone());
        ui.open_url(&open);
        let minutes = Duration::from_secs(grant.expires_in)
            .min(LOGIN_TIMEOUT)
            .as_secs()
            .div_ceil(60);
        ui.status(&format!(
            "用户码 {}：确认页面上的码和这里一致，再点同意（{minutes} 分钟内有效）。\
             浏览器没打开就自己打开 {} 输入这个码。",
            grant.user_code, grant.verification_uri
        ));
        let token = self.poll_device(&grant).await?;
        let cred = self
            .finish_login(ui, expected.as_deref(), &token, req.scope)
            .await?;
        Ok(key_of(&cred))
    }

    /// 换到 token 之后：验 ID token、认账号、落盘、切成活动账号。
    async fn finish_login(
        &self,
        ui: &dyn LoginUi,
        expected: Option<&str>,
        token: &Value,
        mode: ScopeMode,
    ) -> Result<Credential, AuthError> {
        let missing = |name: &str| AuthError::NeedsLogin {
            reason: format!("token 响应里没有 {name}"),
        };
        let access = text(token, "access_token").ok_or_else(|| missing("access_token"))?;
        // 要了 offline_access 却没给 refresh token：6 小时后就只能重新登录，不如现在就说。
        let refresh = text(token, "refresh_token").ok_or_else(|| missing("refresh_token"))?;
        let id_token = text(token, "id_token").ok_or_else(|| missing("id_token"))?;

        // 先验签名再看声明。device code 没有浏览器回调，也就没有 nonce：token 是
        // Dock 自己向 token 端点换来的。
        let jwks = self
            .jwks()
            .await
            .map_err(|e| AuthError::NeedsLogin { reason: e })?;
        let claims = verify_jwt(
            &id_token,
            &Expected {
                issuer: ISSUER,
                audience: CLIENT_ID,
                nonce: None,
                alg: "ES256",
            },
            &jwks,
        )
        .map_err(|e| AuthError::NeedsLogin { reason: e })?;
        let subject = claims
            .subject
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AuthError::NeedsLogin {
                reason: "ID token 里没有 sub，认不出是哪个账号".into(),
            })?;
        if let Some(want) = expected {
            if want != subject {
                return Err(AuthError::NeedsLogin {
                    reason: "授权回来的是另一个 xAI 账号，没有覆盖原来那个。要加新账号用 \
                             /auth login new grok"
                        .into(),
                });
            }
        }
        let account_id = sanitize(&subject);
        let existing = self.store.load(&account_id).ok();
        let scopes = {
            let granted = scope_list(token);
            if granted.is_empty() {
                scopes_for(mode).iter().map(|s| s.to_string()).collect()
            } else {
                granted
            }
        };
        let expires_in = token
            .get("expires_in")
            .and_then(|n| n.as_u64())
            .unwrap_or(DEFAULT_EXPIRES_IN);
        let cred = Credential {
            schema_version: crate::auth::credentials::SCHEMA_VERSION,
            generation: existing.as_ref().map(|c| c.generation + 1).unwrap_or(1),
            email: claims.email.clone(),
            issuer: ISSUER.to_string(),
            subject,
            client_id: account_id.clone(),
            id_token,
            access_token: access,
            refresh_token: refresh,
            expires_at: now_unix() + expires_in,
            scopes,
            saved_at: now_unix(),
            ext_agent_host_id: String::new(),
        };
        self.store
            .save(&cred)
            .map_err(|e| AuthError::Transport(e.to_string()))?;
        self.store
            .set_active(&account_id)
            .map_err(|e| AuthError::Transport(e.to_string()))?;

        let who = cred.email.clone().unwrap_or_else(|| account_id.clone());
        if has_plan(&cred) {
            ui.status(&format!(
                "登录成功：{who}。走 grok/* 的模型（auth = \"grok\"）用的是你的 SuperGrok / \
                 X Premium 订阅额度。/auth models grok 看可用模型。"
            ));
        } else {
            ui.status(&format!(
                "登录成功：{who}，但这份授权只有身份、没有订阅额度，不能拿它推理。要用订阅就 \
                 /auth login grok 重新登录。"
            ));
        }
        Ok(cred)
    }

    /// 刷新：refresh token → 新 token。refresh token 会轮换（实测旧的仍能用一阵，
    /// 但照样按「旧的作废」处理：存新的）。
    async fn refresh(&self, cred: &Credential) -> Result<Credential, TokenError> {
        let v = self
            .token_request(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &cred.refresh_token),
                ("client_id", CLIENT_ID),
            ])
            .await?;
        let access = text(&v, "access_token").ok_or_else(|| TokenError::OAuth {
            code: "no_access_token".into(),
            detail: "刷新响应里没有 access_token".into(),
            status: 200,
        })?;
        let refresh = text(&v, "refresh_token").unwrap_or(cred.refresh_token.clone());
        // 刷新响应不带 id_token（实测）：沿用登录时那份，它只用来认账号。
        let id_token = text(&v, "id_token").unwrap_or(cred.id_token.clone());
        let expires_in = v
            .get("expires_in")
            .and_then(|n| n.as_u64())
            .unwrap_or(DEFAULT_EXPIRES_IN);
        let scopes = {
            let s = scope_list(&v);
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

    /// 刷新时拿跨进程锁：两个 Dock 进程同时刷，后写的会盖掉先写的那份轮换结果。
    /// 刷新被判死就清掉 token；网络抖一下不清。
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
                        tracing::warn!(error = %clear, "Grok 订阅：作废的凭据没能清掉");
                    }
                }
                Err(e)
            }
        }
    }

    /// 读一份还登着、至少还能用 `min_valid` 的凭据（快过期就先刷新）。
    async fn fresh_credential(
        &self,
        account_id: &str,
        min_valid: Duration,
    ) -> Result<Credential, AuthError> {
        let cred = self.load_signed_in(account_id)?;
        if cred.expires_in() >= min_valid.as_secs() as i64 {
            return Ok(cred);
        }
        self.refresh_locked(&cred).await.map_err(AuthError::from)
    }

    fn load_signed_in(&self, account_id: &str) -> Result<Credential, AuthError> {
        let cred = self.store.load(account_id).map_err(|e| match e {
            CredentialError::Missing => AuthError::NeedsLogin {
                reason: format!("Grok 账号 {account_id} 还没登录：/auth login grok"),
            },
            other => AuthError::NeedsLogin {
                reason: format!("读不到 Grok 凭据：{other}"),
            },
        })?;
        if !cred.signed_in() {
            return Err(AuthError::NeedsLogin {
                reason: format!(
                    "{} 已经登出了：/auth login grok 重新登录",
                    cred.email.as_deref().unwrap_or(&cred.client_id)
                ),
            });
        }
        Ok(cred)
    }

    /// 撤销 refresh token（远端）。空的 200 就是成功；网络失败或 5xx 退避重试。
    async fn revoke_remote(&self, cred: &Credential) -> Result<(), String> {
        let mut last = String::new();
        for attempt in 0..REVOKE_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(400 << (attempt - 1))).await;
            }
            let sent = self
                .client
                .post(&self.endpoints.revoke)
                .timeout(TOKEN_TIMEOUT)
                .form(&[
                    ("token", cred.refresh_token.as_str()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", CLIENT_ID),
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

    /// GET `{api}/<path>`，带 access token。非 2xx 归一成 [`AuthError`]。
    async fn api_get(&self, path: &str, bearer: &str) -> Result<Value, AuthError> {
        let url = format!("{}/{path}", self.endpoints.api);
        let resp = self
            .client
            .get(&url)
            .timeout(TOKEN_TIMEOUT)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| AuthError::Transport(format!("列模型失败：{e}")))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(classify(status.as_u16(), body.as_bytes())
                .unwrap_or_else(|| AuthError::Transport(format!("列模型失败：HTTP {status}"))));
        }
        serde_json::from_str(&body)
            .map_err(|e| AuthError::Transport(format!("列模型的响应不是 JSON：{e}")))
    }
}

impl GrokProvider {
    /// GET `url`（带 access token）。token 被拒（[`token_rejected`]）就刷新一次再查；
    /// 别的非 2xx 归一成 [`AuthError`]。
    async fn authed_get(&self, url: &str, cred: &Credential) -> Result<Value, AuthError> {
        let mut bearer = cred.access_token.clone();
        let mut refreshed = false;
        loop {
            let resp = self
                .client
                .get(url)
                .timeout(TOKEN_TIMEOUT)
                .header(reqwest::header::ACCEPT, "application/json")
                .bearer_auth(&bearer)
                .send()
                .await
                .map_err(|e| AuthError::Transport(format!("查额度失败：{e}")))?;
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if status.is_success() {
                return serde_json::from_str(&body)
                    .map_err(|e| AuthError::Transport(format!("额度响应不是 JSON：{e}")));
            }
            if !refreshed && token_rejected(status.as_u16(), body.as_bytes()) {
                refreshed = true;
                let fresh = self.refresh_locked(cred).await.map_err(AuthError::from)?;
                bearer = fresh.access_token;
                continue;
            }
            return Err(classify(status.as_u16(), body.as_bytes())
                .unwrap_or_else(|| AuthError::Transport(format!("查额度失败：HTTP {status}"))));
        }
    }

    /// models.dev 目录里 xAI 那一家的模型表（`{ "<slug>": { reasoning, reasoning_options, … } }`）。
    async fn xai_catalog(&self) -> Result<Value, String> {
        let resp = self
            .client
            .get(&self.endpoints.catalog)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("连不上模型目录：{e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("模型目录 HTTP {status}"));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| format!("模型目录不是 JSON：{e}"))?;
        v.get("xai")
            .and_then(|p| p.get("models"))
            .filter(|m| m.is_object())
            .cloned()
            .ok_or_else(|| "模型目录里没有 xai".to_string())
    }

    /// 这个账号的套餐名：先看记着的，没有再查 `/settings`。查不到不算错（额度照样显示）。
    async fn plan_name(&self, cred: &Credential) -> Option<String> {
        if let Some(plan) = self.plans.lock().ok()?.get(&cred.client_id).cloned() {
            return Some(plan);
        }
        match self.authed_get(&self.endpoints.settings, cred).await {
            Ok(v) => {
                let plan = text(&v, "subscription_tier_display")?;
                if let Ok(mut plans) = self.plans.lock() {
                    plans.insert(cred.client_id.clone(), plan.clone());
                }
                Some(plan)
            }
            Err(e) => {
                tracing::warn!(error = %e.message(), "Grok 订阅：套餐名没查到");
                None
            }
        }
    }
}

impl AuthProvider for GrokProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            display_name: "Grok",
            // 订阅用量没有公开的固定页面地址，不瞎填。
            usage_url: None,
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
            .entries()
            .unwrap_or_default()
            .into_iter()
            .map(|(id, cred)| {
                let is_active = active.as_deref() == Some(id.as_str());
                match cred {
                    Ok(c) => AccountSummary {
                        account: key_of(&c),
                        email: c.email.clone(),
                        signed_in: c.signed_in(),
                        plan_usage: c.signed_in() && has_plan(&c),
                        expires_at: if c.signed_in() { c.expires_at } else { 0 },
                        active: is_active,
                        problem: None,
                    },
                    Err(e) => {
                        let fix = match &e {
                            CredentialError::Permissions(_) => format!(
                                "；改回去：chmod 600 {}",
                                self.store.root().join(format!("{id}.json")).display()
                            ),
                            _ => String::new(),
                        };
                        AccountSummary {
                            account: AccountKey {
                                provider: PROVIDER_ID.to_string(),
                                subject: String::new(),
                                client_id: id,
                            },
                            email: None,
                            signed_in: false,
                            plan_usage: false,
                            expires_at: 0,
                            active: is_active,
                            problem: Some(format!("{e}{fix}")),
                        }
                    }
                }
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
                detail: format!("本机没有这个 Grok 账号：{account_id}"),
            },
            other => AuthError::Transport(other.to_string()),
        })?;
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

    fn lease<'a>(
        &'a self,
        acct: &'a AccountKey,
        min_valid: Duration,
    ) -> AuthFuture<'a, Result<Lease, AuthError>> {
        Box::pin(async move {
            let cred = self.load_signed_in(&acct.client_id)?;
            if !has_plan(&cred) {
                return Err(AuthError::NotEligible {
                    code: "missing_plan_usage_scope".into(),
                });
            }
            let cred = self.fresh_credential(&cred.client_id, min_valid).await?;
            Ok(lease_of(&cred))
        })
    }

    fn token_rejected(&self, status: u16, body: &[u8]) -> bool {
        token_rejected(status, body)
    }

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
                    tracing::warn!(error = %e.message(), "Grok 订阅：token 被拒之后刷新失败");
                    Err(AuthError::from(e))
                }
            }
        })
    }

    /// 登出：撤销远端 refresh token，再清本地 token（账号那一行留着，标成登出了）。
    fn logout<'a>(
        &'a self,
        acct: &'a AccountKey,
        revoke: bool,
    ) -> AuthFuture<'a, Result<LogoutOutcome, AuthError>> {
        Box::pin(async move {
            // 也拿锁：别的进程正在刷新时清掉，那份刷新结果会写回来——等于没登出。
            let _lock = FileLock::acquire(&self.store.lock_path_for(&acct.client_id), LOCK_TIMEOUT)
                .await
                .ok();
            let cred = match self.store.load(&acct.client_id) {
                Ok(cred) => cred,
                Err(CredentialError::Missing) => {
                    return Err(AuthError::Rejected {
                        code: "unknown_account".into(),
                        detail: format!("本机没有这个 Grok 账号：{}", acct.client_id),
                    })
                }
                Err(e) => return Err(AuthError::Transport(e.to_string())),
            };
            if !cred.signed_in() {
                return Ok(LogoutOutcome::Revoked);
            }
            let mut outcome = LogoutOutcome::Revoked;
            if revoke {
                if let Err(e) = self.revoke_remote(&cred).await {
                    tracing::warn!(error = %e, "Grok 订阅：远端撤销没确认");
                    outcome = LogoutOutcome::LocalOnly { reason: e };
                }
            }
            self.store
                .clear_tokens(&acct.client_id)
                .map_err(|e| AuthError::Transport(e.to_string()))?;
            Ok(outcome)
        })
    }

    /// 能用的模型：`/v1/language-models` 给语言模型与输入模态（`/v1/models` 里还混着
    /// 图像 / 视频模型），`/v1/models` 补上下文长度（拿不到就空着）。
    fn models<'a>(
        &'a self,
        acct: &'a AccountKey,
    ) -> AuthFuture<'a, Result<Vec<RemoteModel>, AuthError>> {
        Box::pin(async move {
            let cred = self
                .fresh_credential(&acct.client_id, Duration::from_secs(60))
                .await?;
            let language = self.api_get("language-models", &cred.access_token).await?;
            let listed = match self.api_get("models", &cred.access_token).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e.message(), "Grok 订阅：上下文长度没拿到");
                    Value::Null
                }
            };
            // 推理档位：xAI 不给，按 models.dev 的 xai 条目填。拉不到不算错——
            // 档位留空（通用菜单），不收的那档推理层照样去掉重发。
            let catalog = match self.xai_catalog().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "Grok 订阅：模型目录没拉到，推理档位留空");
                    Value::Null
                }
            };
            Ok(parse_models(&language, &listed, &catalog))
        })
    }

    /// 订阅额度：本周期已用百分比、周期起止、套餐名、预付余额。
    fn usage<'a>(
        &'a self,
        acct: &'a AccountKey,
    ) -> AuthFuture<'a, Result<Option<PlanUsage>, AuthError>> {
        Box::pin(async move {
            let cred = self
                .fresh_credential(&acct.client_id, Duration::from_secs(60))
                .await?;
            if !has_plan(&cred) {
                return Err(AuthError::NotEligible {
                    code: "missing_plan_usage_scope".into(),
                });
            }
            let billing = self.authed_get(&self.endpoints.billing, &cred).await?;
            let plan = self.plan_name(&cred).await;
            parse_billing(&billing, plan, now_unix()).map(Some)
        })
    }

    fn classify(&self, status: u16, body: &[u8]) -> Option<AuthError> {
        classify(status, body)
    }
}

/// `/billing?format=credits` → [`PlanUsage`]。
///
/// 实测（SuperGrok，2026-11-21）：
/// `{"config": {"currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "start": …, "end": …},
/// "creditUsagePercent": 27.0, "prepaidBalance": {"val": 0}, …}}`。
/// 字段名按 Grok Build 的定义两种写法都认（camelCase / snake_case）。
fn parse_billing(v: &Value, plan: Option<String>, now: u64) -> Result<PlanUsage, AuthError> {
    let config = v
        .get("config")
        .filter(|c| c.is_object())
        .ok_or_else(|| AuthError::Transport("额度响应里没有 config".into()))?;
    let field = |camel: &str, snake: &str| config.get(camel).or_else(|| config.get(snake));
    let cents = |camel: &str, snake: &str| {
        // proto3 的 JSON 把 0 省掉：`{"val": 0}` 可能来成 `{}`。
        field(camel, snake)
            .filter(|c| c.is_object())
            .map(|c| c.get("val").and_then(as_i64).unwrap_or(0))
    };

    let period = {
        let current = field("currentPeriod", "current_period");
        let bound = |name: &str, legacy_camel: &str, legacy_snake: &str| {
            current
                .and_then(|c| c.get(name))
                .or_else(|| field(legacy_camel, legacy_snake))
                .and_then(|t| t.as_str())
                .and_then(parse_rfc3339)
        };
        let kind = current
            .and_then(|c| c.get("type"))
            .and_then(|t| t.as_str())
            .and_then(|t| {
                if t.ends_with("WEEKLY") {
                    Some("weekly")
                } else if t.ends_with("MONTHLY") {
                    Some("monthly")
                } else {
                    None
                }
            })
            .map(str::to_string);
        let start = bound("start", "billingPeriodStart", "billing_period_start");
        let end = bound("end", "billingPeriodEnd", "billing_period_end");
        (kind.is_some() || start.is_some() || end.is_some()).then_some(UsagePeriod {
            kind,
            start,
            end,
        })
    };

    let used_percent = field("creditUsagePercent", "credit_usage_percent")
        .and_then(|p| p.as_f64())
        .filter(|p| p.is_finite() && *p >= 0.0)
        // 老的账单形状：已用 / 月度上限（美分）。
        .or_else(|| {
            let used = cents("used", "used")?;
            let limit = cents("monthlyLimit", "monthly_limit")?;
            (limit > 0 && used >= 0).then(|| used as f64 / limit as f64 * 100.0)
        })
        // 本周期刚开始、一点没用时上游把 0 省掉了（proto3）；grok.com 网页也按 0 读。
        // 只在「正处在一个完整的周期里」时这么认，别处缺了就是不知道。
        .or_else(|| {
            let p = period.as_ref()?;
            (p.start? <= now && now < p.end?).then_some(0.0)
        })
        .map(|p| p.min(100.0));

    Ok(PlanUsage {
        plan,
        used_percent,
        period,
        prepaid_balance_cents: cents("prepaidBalance", "prepaid_balance"),
        fetched_at: now,
    })
}

/// 数字或数字字符串（proto3 的 int64 在 JSON 里是字符串）。
fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// RFC 3339 时间（`2026-11-24T02:21:38.373116+00:00` / `…Z`）→ unix 秒。不认得就 `None`。
fn parse_rfc3339(s: &str) -> Option<u64> {
    let s = s.trim();
    let (date, rest) = s.split_once(['T', 't', ' '])?;
    let mut ymd = date.splitn(3, '-');
    let (y, m, d): (i64, i64, i64) = (
        ymd.next()?.parse().ok()?,
        ymd.next()?.parse().ok()?,
        ymd.next()?.parse().ok()?,
    );
    // 时区：`Z` 或 `±hh:mm`（找时间部分里最后一个 + / -）。
    let (clock, offset_secs) = if let Some(clock) = rest.strip_suffix(['Z', 'z']) {
        (clock, 0i64)
    } else {
        let at = rest.rfind(['+', '-'])?;
        let (clock, tz) = rest.split_at(at);
        let sign = if tz.starts_with('-') { -1 } else { 1 };
        let (hh, mm) = tz[1..].split_once(':').unwrap_or((&tz[1..], "0"));
        let off = hh.parse::<i64>().ok()? * 3600 + mm.parse::<i64>().ok()? * 60;
        (clock, sign * off)
    };
    let mut hms = clock.splitn(3, ':');
    let (h, mi): (i64, i64) = (hms.next()?.parse().ok()?, hms.next()?.parse().ok()?);
    // 秒可能带小数：只取整数部分。
    let sec: i64 = hms.next()?.split('.').next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    // 公历日期 → 自 1970-01-01 的天数（Howard Hinnant 的 days_from_civil）。
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset_secs;
    u64::try_from(secs).ok()
}

/// 有没有订阅推理要的那两条 scope。
fn has_plan(cred: &Credential) -> bool {
    PLAN_SCOPES
        .iter()
        .all(|want| cred.scopes.iter().any(|s| s == want))
}

fn key_of(cred: &Credential) -> AccountKey {
    AccountKey {
        provider: PROVIDER_ID.into(),
        subject: cred.subject.clone(),
        client_id: cred.client_id.clone(),
    }
}

fn lease_of(cred: &Credential) -> Lease {
    Lease {
        bearer: cred.access_token.clone(),
        expires_at: std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(cred.expires_at),
        generation: cred.generation,
        account: key_of(cred),
    }
}

/// 字符串字段（空串算没有）。
fn text(v: &Value, name: &str) -> Option<String> {
    v.get(name)
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn scope_list(v: &Value) -> Vec<String> {
    v.get("scope")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// xAI 错误 body 里的 `code`（`{"code": "...", "error": "..."}`）。
fn error_code(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| text(&v, "code"))
        .unwrap_or_default()
}

/// 这个推理响应是不是 token 失效。xAI 的 OAuth token 校验不过回的是 403
/// `unauthenticated:bad-credentials`（实测：签名被改坏的 token），不是 401。同是
/// 403，套餐档位不够就不是这个码——那种刷新也没用。
pub(crate) fn token_rejected(status: u16, body: &[u8]) -> bool {
    status == 401 || (status == 403 && error_code(body).starts_with("unauthenticated"))
}

/// 目录里这个模型的推理能力：（会不会推理，档位）。
///
/// - 目录里没有：`(None, None)`——不知道，通用菜单；
/// - `reasoning: false`：不推理；
/// - 有 `{"type": "effort", "values": […]}`：就是这些档；
/// - 会推理但没有 effort 那一项（`reasoning_options: []`，如 grok-4.20-reasoning）：`Some([])`，
///   菜单不列、请求里不发 effort（实测它不收 `reasoning.effort`）。
fn catalog_reasoning(entry: Option<&Value>) -> (Option<bool>, Option<Vec<String>>) {
    let Some(m) = entry else {
        return (None, None);
    };
    if m.get("reasoning").and_then(|r| r.as_bool()) == Some(false) {
        return (Some(false), None);
    }
    let reasoning = m.get("reasoning").and_then(|r| r.as_bool());
    let Some(options) = m.get("reasoning_options").and_then(|o| o.as_array()) else {
        return (reasoning, None);
    };
    let efforts = options
        .iter()
        .find(|o| o.get("type").and_then(|t| t.as_str()) == Some("effort"))
        .and_then(|o| o.get("values"))
        .and_then(|v| v.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|x| x.as_str())
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    (reasoning, Some(efforts))
}

/// `/v1/language-models` + `/v1/models` → 可以给用户选的模型（保持上游顺序）。
/// `catalog` 是 models.dev 里 xAI 的模型表（拉不到就是 `Null`）：推理档位从这里来，
/// 先按 id 找，再按上游给的别名找。
fn parse_models(language: &Value, listed: &Value, catalog: &Value) -> Vec<RemoteModel> {
    let context = |id: &str| {
        listed
            .get("data")
            .and_then(|d| d.as_array())?
            .iter()
            .find(|m| m.get("id").and_then(|i| i.as_str()) == Some(id))?
            .get("context_length")?
            .as_u64()
            .filter(|n| *n > 0)
    };
    let has = |m: &Value, key: &str, want: &str| {
        m.get(key)
            .and_then(|l| l.as_array())
            .map(|l| l.iter().any(|x| x.as_str() == Some(want)))
    };
    language
        .get("models")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
        // 只要出文字的（以后要是混进别的模态，别放进对话模型里）。
        .filter(|m| has(m, "output_modalities", "text").unwrap_or(true))
        .filter_map(|m| {
            let slug = text(m, "id")?;
            let entry = catalog.get(&slug).or_else(|| {
                m.get("aliases")
                    .and_then(|a| a.as_array())?
                    .iter()
                    .filter_map(|a| a.as_str())
                    .find_map(|a| catalog.get(a))
            });
            // 上游不说哪些推理档位可用（各模型差别很大：有的不收 `none`，有的根本
            // 不收 `reasoning.effort`），按目录填；目录也没有就留空，不收的那项由
            // [`classify`] 认出来，推理层去掉它重发一次并记住。
            let (reasoning, reasoning_efforts) = catalog_reasoning(entry);
            Some(RemoteModel {
                context_window: context(&slug),
                supports_images: has(m, "input_modalities", "image"),
                reasoning,
                reasoning_efforts,
                default_reasoning_effort: None,
                display_name: None,
                description: None,
                slug,
            })
        })
        .collect()
}

/// 上游错误 → [`AuthError`]。xAI 的错误 body 是 `{"code": "...", "error": "说明"}`。
pub fn classify(status: u16, body: &[u8]) -> Option<AuthError> {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let code = text(&v, "code").unwrap_or_default();
    let detail = text(&v, "error")
        .or_else(|| v.get("error").and_then(|e| text(e, "message")))
        .unwrap_or_default();
    let code_or = |fallback: &str| {
        if code.is_empty() {
            fallback.to_string()
        } else {
            code.clone()
        }
    };

    // 400：这个模型不收 `reasoning.effort`（实测两种说法：`does not support parameter
    // reasoningEffort`、``does not support `reasoning_effort` value `none` ``）。
    // 推理层据此去掉那一项重发一次，并记住这个模型不收它。
    if status == 400
        && (detail.contains("does not support parameter reasoningEffort")
            || detail.contains("does not support `reasoning_effort`"))
    {
        return Some(AuthError::Unsupported {
            param: Some("reasoning.effort".into()),
        });
    }
    match status {
        401 => Some(AuthError::NeedsLogin {
            reason: "xAI 不认这份 token，刷新过一次仍不行：/auth login grok 重新登录".into(),
        }),
        403 if code.starts_with("unauthenticated") => Some(AuthError::NeedsLogin {
            reason: format!(
                "xAI 校验不过这份 token（{code}），刷新过一次仍不行：/auth login grok 重新登录"
            ),
        }),
        // 有的 SuperGrok 档位登录成功、推理被拒（Hermes 那边有人报过）。
        403 => Some(AuthError::Rejected {
            code: code_or("http_403"),
            detail: format!(
                "{}。可能是这个订阅档位不能在第三方工具里用：换一个订阅档位，或给模型配 api_key 按量计费。",
                if detail.is_empty() {
                    "xAI 拒绝了这次请求"
                } else {
                    detail.as_str()
                }
            ),
        }),
        // 限流和订阅额度用完都回 429，body 里分不清：都不重试，原话带出来。
        429 => Some(AuthError::Rejected {
            code: code_or("http_429"),
            detail: format!(
                "xAI 限流了，或订阅额度到上限了{}。稍后再试；额度用完不会自动转成按量计费。",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!("（{detail}）")
                }
            ),
        }),
        503 => Some(AuthError::Unavailable {
            code: code_or("http_503"),
            retry: true,
        }),
        _ => None,
    }
}

/// 把 Grok 订阅通道整个关掉的环境变量（`off` / `0` / `false` / `no`）。
pub const SUBSCRIPTION_ENV: &str = "DOCK_GROK_SUBSCRIPTION";

/// provider 插件：把自己注册进 `"auth"`。开关关掉就不注册（写了 `auth = "grok"` 的
/// 模型会明确报「被开关关掉了」，没写 `auth` 的配置不受影响）。
pub fn grok_auth() -> Plugin {
    plugin("grok-auth", Inject::from([AUTH]), |ctx, _: &()| {
        if cordis_base::auth::subscription_disabled(PROVIDER_ID) {
            tracing::info!("{SUBSCRIPTION_ENV} 已关：Grok 订阅通道不挂载。");
            return Ok(None);
        }
        if let Some(manager) = ctx.get::<AuthManager>(AUTH) {
            let provider = Arc::new(GrokProvider::new(CredentialStore::for_provider(
                PROVIDER_ID,
            )));
            let registered = manager.register(provider);
            manager.refresh_models_if_stale(PROVIDER_ID, MODELS_MAX_AGE);
            return Ok(Some(registered));
        }
        Ok(None)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// 一次请求：路径、Authorization 头、表单（解码过的）。
    #[derive(Clone, Debug)]
    struct Hit {
        path: String,
        auth: Option<String>,
        form: HashMap<String, String>,
    }

    type Responder = dyn Fn(&str, usize, &HashMap<String, String>) -> (u16, String) + Send + Sync;

    /// 本地假 xAI：device / token / revoke / jwks / v1/*，按路径数次数、记表单。
    struct FakeXai {
        base: String,
        hits: Arc<Mutex<Vec<Hit>>>,
    }

    impl FakeXai {
        async fn start(
            f: impl Fn(&str, usize, &HashMap<String, String>) -> (u16, String) + Send + Sync + 'static,
        ) -> Self {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let hits: Arc<Mutex<Vec<Hit>>> = Default::default();
            let f: Arc<Responder> = Arc::new(f);
            let sink = hits.clone();
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let sink = sink.clone();
                    let f = f.clone();
                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let mut chunk = [0u8; 4096];
                        let head_end = loop {
                            let n = socket.read(&mut chunk).await.unwrap_or(0);
                            if n == 0 {
                                return;
                            }
                            buf.extend_from_slice(&chunk[..n]);
                            if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                break p + 4;
                            }
                        };
                        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                        let header = |name: &str| {
                            head.lines().find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.trim()
                                    .eq_ignore_ascii_case(name)
                                    .then(|| v.trim().to_string())
                            })
                        };
                        let len: usize = header("content-length")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        let mut body = buf[head_end..].to_vec();
                        while body.len() < len {
                            let n = socket.read(&mut chunk).await.unwrap_or(0);
                            if n == 0 {
                                break;
                            }
                            body.extend_from_slice(&chunk[..n]);
                        }
                        let path = head
                            .lines()
                            .next()
                            .and_then(|l| l.split_whitespace().nth(1))
                            .unwrap_or("")
                            .to_string();
                        let form: HashMap<String, String> = url::form_urlencoded::parse(&body)
                            .map(|(k, v)| (k.into_owned(), v.into_owned()))
                            .collect();
                        let n = {
                            let mut hits = sink.lock().unwrap();
                            let n = hits.iter().filter(|h| h.path == path).count();
                            hits.push(Hit {
                                path: path.clone(),
                                auth: header("authorization"),
                                form: form.clone(),
                            });
                            n
                        };
                        let (status, body) = f(&path, n, &form);
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

        fn hits(&self, path: &str) -> Vec<Hit> {
            self.hits
                .lock()
                .unwrap()
                .iter()
                .filter(|h| h.path == path)
                .cloned()
                .collect()
        }
    }

    /// 测试用签名钥匙：一把现场造的 P-256，JWKS + 签 ID token。
    struct Signer {
        pair: ring::signature::EcdsaKeyPair,
        jwks: String,
    }

    impl Signer {
        fn new() -> Self {
            use base64::Engine;
            use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
            let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            let rng = ring::rand::SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap();
            let point = pair.public_key().as_ref().to_vec();
            let jwks = json!({"keys": [{
                "kty": "EC", "crv": "P-256", "alg": "ES256", "kid": "test",
                "x": enc.encode(&point[1..33]),
                "y": enc.encode(&point[33..65]),
            }]})
            .to_string();
            Self { pair, jwks }
        }

        fn id_token(&self, sub: &str, email: &str) -> String {
            use base64::Engine;
            let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            let claims = json!({
                "iss": ISSUER,
                "aud": CLIENT_ID,
                "sub": sub,
                "email": email,
                "exp": now_unix() + 3600,
                "iat": now_unix(),
            });
            let input = format!(
                "{}.{}",
                enc.encode(br#"{"alg":"ES256","kid":"test","typ":"JWT"}"#),
                enc.encode(serde_json::to_vec(&claims).unwrap())
            );
            let sig = self
                .pair
                .sign(&ring::rand::SystemRandom::new(), input.as_bytes())
                .unwrap();
            format!("{input}.{}", enc.encode(sig.as_ref()))
        }
    }

    struct Recorder {
        urls: Mutex<Vec<String>>,
        statuses: Mutex<Vec<String>>,
    }

    impl Recorder {
        fn new() -> Self {
            Self {
                urls: Mutex::new(Vec::new()),
                statuses: Mutex::new(Vec::new()),
            }
        }
    }

    impl LoginUi for Recorder {
        fn open_url(&self, url: &str) {
            self.urls.lock().unwrap().push(url.to_string());
        }
        fn status(&self, message: &str) {
            self.statuses.lock().unwrap().push(message.to_string());
        }
    }

    fn store() -> CredentialStore {
        CredentialStore::new(tempfile::tempdir().unwrap().keep())
    }

    fn device_ok() -> String {
        json!({
            "device_code": "dev-1",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://accounts.x.ai/oauth2/device",
            "verification_uri_complete": "https://accounts.x.ai/oauth2/device?user_code=ABCD-EFGH",
            "expires_in": 1800,
            "interval": 0,
        })
        .to_string()
    }

    /// 正常的一套：先两次 pending、再给 token；JWKS 是 `signer` 的。
    async fn happy_xai(signer: Arc<Signer>, sub: &'static str) -> FakeXai {
        FakeXai::start(move |path, n, form| match path {
            "/device" => (200, device_ok()),
            "/jwks" => (200, signer.jwks.clone()),
            "/token" if form.get("grant_type").map(String::as_str) == Some("refresh_token") => (
                200,
                json!({
                    "access_token": "access-refreshed",
                    "refresh_token": "refresh-rotated",
                    "expires_in": 21600,
                    "scope": SCOPES.join(" "),
                })
                .to_string(),
            ),
            "/token" if n < 2 => (400, json!({"error": "authorization_pending"}).to_string()),
            "/token" => (
                200,
                json!({
                    "access_token": "access-1",
                    "refresh_token": "refresh-1",
                    "id_token": signer.id_token(sub, "g@x.ai"),
                    "expires_in": 21600,
                    "scope": SCOPES.join(" "),
                    "token_type": "Bearer",
                })
                .to_string(),
            ),
            "/revoke" => (200, String::new()),
            _ => (404, "{}".into()),
        })
        .await
    }

    /// 完整走一遍 device code：申请用户码（带 referrer=dock 与订阅 scope）→ 打开
    /// 填好码的页面 → pending 两次后拿到 token → 验 ID token → 落盘、切成活动账号。
    #[tokio::test]
    async fn device_code_login_saves_the_account() {
        let signer = Arc::new(Signer::new());
        let xai = happy_xai(signer.clone(), "user-uuid-1").await;
        let store = store();
        let provider = GrokProvider::new(store.clone()).with_test_server(&xai.base);
        let ui = Recorder::new();

        let key = provider
            .login(&ui, LoginRequest::default())
            .await
            .expect("登录");
        assert_eq!(key.provider, PROVIDER_ID);
        assert_eq!(key.subject, "user-uuid-1");
        assert_eq!(key.client_id, "user-uuid-1", "账号按 sub 分文件");

        let device = xai.hits("/device");
        assert_eq!(device.len(), 1);
        assert_eq!(device[0].form["client_id"], CLIENT_ID);
        assert_eq!(device[0].form["referrer"], "dock");
        assert_eq!(device[0].form["scope"], SCOPES.join(" "));
        let polls = xai.hits("/token");
        assert_eq!(polls.len(), 3, "两次 pending + 一次成功");
        assert!(polls.iter().all(|h| h.form["grant_type"]
            == "urn:ietf:params:oauth:grant-type:device_code"
            && h.form["device_code"] == "dev-1"
            && h.form["client_id"] == CLIENT_ID));

        assert_eq!(
            ui.urls.lock().unwrap().as_slice(),
            ["https://accounts.x.ai/oauth2/device?user_code=ABCD-EFGH"],
            "打开的是填好用户码的那一页"
        );
        let statuses = ui.statuses.lock().unwrap().join("\n");
        assert!(statuses.contains("ABCD-EFGH"), "用户码要报出来：{statuses}");
        assert!(statuses.contains("登录成功"), "{statuses}");

        let cred = store.load("user-uuid-1").unwrap();
        assert_eq!(cred.access_token, "access-1");
        assert_eq!(cred.refresh_token, "refresh-1");
        assert_eq!(cred.email.as_deref(), Some("g@x.ai"));
        assert_eq!(cred.issuer, ISSUER);
        assert!(has_plan(&cred));
        assert_eq!(store.active().as_deref(), Some("user-uuid-1"));
        let accounts = provider.accounts();
        assert_eq!(accounts.len(), 1);
        assert!(accounts[0].active && accounts[0].signed_in && accounts[0].plan_usage);
    }

    /// ID token 不是授权服务器的钥匙签的：拒，不落盘。
    #[tokio::test]
    async fn an_id_token_signed_by_another_key_is_refused() {
        let signer = Arc::new(Signer::new());
        let impostor = Arc::new(Signer::new());
        let jwks = signer.jwks.clone();
        let forged = impostor.id_token("user-uuid-1", "g@x.ai");
        let xai = FakeXai::start(move |path, _, _| match path {
            "/device" => (200, device_ok()),
            "/jwks" => (200, jwks.clone()),
            "/token" => (
                200,
                json!({
                    "access_token": "a", "refresh_token": "r",
                    "id_token": forged, "scope": SCOPES.join(" "),
                })
                .to_string(),
            ),
            _ => (404, "{}".into()),
        })
        .await;
        let store = store();
        let provider = GrokProvider::new(store.clone()).with_test_server(&xai.base);
        let err = provider
            .login(&Recorder::new(), LoginRequest::default())
            .await
            .expect_err("假 ID token 要拒");
        assert!(err.message().contains("签名"), "{}", err.message());
        assert!(store.list().unwrap().is_empty(), "拒了就不能落盘");
        assert!(store.active().is_none());
    }

    /// 用户在授权页上点了拒绝：不再轮询，报一句能照着做的话。
    #[tokio::test]
    async fn a_denied_authorization_stops_polling() {
        let xai = FakeXai::start(|path, _, _| match path {
            "/device" => (200, device_ok()),
            "/token" => (400, json!({"error": "access_denied"}).to_string()),
            _ => (404, "{}".into()),
        })
        .await;
        let provider = GrokProvider::new(store()).with_test_server(&xai.base);
        let err = provider
            .login(&Recorder::new(), LoginRequest::default())
            .await
            .expect_err("拒了就是失败");
        assert_eq!(err.code(), "access_denied");
        assert_eq!(xai.hits("/token").len(), 1, "拒了就别再轮询");
    }

    /// `slow_down`：照 RFC 加大间隔接着等，不是报错。
    #[tokio::test]
    async fn slow_down_keeps_polling() {
        let signer = Arc::new(Signer::new());
        let s = signer.clone();
        let xai = FakeXai::start(move |path, n, _| match path {
            "/device" => (200, device_ok()),
            "/jwks" => (200, s.jwks.clone()),
            "/token" if n == 0 => (400, json!({"error": "slow_down"}).to_string()),
            "/token" => (
                200,
                json!({
                    "access_token": "a", "refresh_token": "r",
                    "id_token": s.id_token("u2", "b@x.ai"), "scope": SCOPES.join(" "),
                })
                .to_string(),
            ),
            _ => (404, "{}".into()),
        })
        .await;
        // 测试里不真等（间隔被压成 0）：这里只验「slow_down 之后还接着轮询」。
        let provider = GrokProvider::new(store()).with_test_server(&xai.base);
        let grant = DeviceGrant {
            device_code: "dev-1".into(),
            user_code: "X".into(),
            verification_uri: "https://accounts.x.ai/oauth2/device".into(),
            verification_uri_complete: None,
            expires_in: 60,
            interval: 0,
        };
        let polled = provider.poll_device(&grant).await;
        assert!(polled.is_ok(), "{polled:?}");
        assert_eq!(xai.hits("/token").len(), 2);
    }

    /// 重新登录活动账号时，授权回来的是另一个人：不覆盖原来那个。
    #[tokio::test]
    async fn relogin_as_someone_else_does_not_overwrite_the_account() {
        let signer = Arc::new(Signer::new());
        let xai = happy_xai(signer, "user-b").await;
        let store = store();
        store.save(&credential("user-a", 3600)).unwrap();
        store.set_active("user-a").unwrap();
        let provider = GrokProvider::new(store.clone()).with_test_server(&xai.base);
        let err = provider
            .login(&Recorder::new(), LoginRequest::default())
            .await
            .expect_err("换了人要拒");
        assert!(
            err.message().contains("另一个 xAI 账号"),
            "{}",
            err.message()
        );
        assert_eq!(store.load("user-a").unwrap().access_token, "access-old");
        assert!(store.load("user-b").is_err(), "不能顺手存成新账号");

        // 明说要加新账号就行。
        let req = LoginRequest {
            target: LoginTarget::New,
            ..LoginRequest::default()
        };
        let key = provider
            .login(&Recorder::new(), req)
            .await
            .expect("加新账号");
        assert_eq!(key.client_id, "user-b");
        assert_eq!(store.active().as_deref(), Some("user-b"));
    }

    fn credential(account: &str, expires_in: u64) -> Credential {
        Credential {
            schema_version: crate::auth::credentials::SCHEMA_VERSION,
            generation: 1,
            email: Some("a@x.ai".into()),
            issuer: ISSUER.into(),
            subject: account.into(),
            client_id: account.into(),
            id_token: "id".into(),
            access_token: "access-old".into(),
            refresh_token: "refresh-old".into(),
            expires_at: now_unix() + expires_in,
            scopes: SCOPES.iter().map(|s| s.to_string()).collect(),
            saved_at: now_unix(),
            ext_agent_host_id: String::new(),
        }
    }

    /// 快过期的 token：lease 先刷新（refresh token 轮换、client id 是公开的那个），
    /// 还早就不刷。
    #[tokio::test]
    async fn lease_refreshes_a_token_about_to_expire() {
        let signer = Arc::new(Signer::new());
        let xai = happy_xai(signer, "u").await;
        let store = store();
        store.save(&credential("acct", 30)).unwrap();
        store.set_active("acct").unwrap();
        let provider = GrokProvider::new(store.clone()).with_test_server(&xai.base);
        let acct = provider.active_account().unwrap();

        let lease = provider
            .lease(&acct, Duration::from_secs(120))
            .await
            .expect("lease");
        assert_eq!(lease.bearer, "access-refreshed");
        assert_eq!(lease.generation, 2);
        let refresh = xai.hits("/token");
        assert_eq!(refresh.len(), 1);
        assert_eq!(refresh[0].form["refresh_token"], "refresh-old");
        assert_eq!(refresh[0].form["client_id"], CLIENT_ID);
        let saved = store.load("acct").unwrap();
        assert_eq!(saved.refresh_token, "refresh-rotated");
        assert_eq!(saved.id_token, "id", "刷新不带 id_token：沿用原来那份");

        // 新 token 还有 6 小时：再要一次不刷。
        let again = provider
            .lease(&acct, Duration::from_secs(120))
            .await
            .unwrap();
        assert_eq!(again.bearer, "access-refreshed");
        assert_eq!(xai.hits("/token").len(), 1);
    }

    /// 刷新被拒（`invalid_grant`：撤销过 / 乱写的）：清掉 token、要求重新登录。
    #[tokio::test]
    async fn a_dead_refresh_token_signs_the_account_out() {
        let xai = FakeXai::start(|path, _, _| match path {
            "/token" => (
                400,
                json!({"error": "invalid_grant", "error_description": "Refresh token has been revoked"})
                    .to_string(),
            ),
            _ => (404, "{}".into()),
        })
        .await;
        let store = store();
        store.save(&credential("acct", 0)).unwrap();
        store.set_active("acct").unwrap();
        let provider = GrokProvider::new(store.clone()).with_test_server(&xai.base);
        let acct = provider.active_account().unwrap();
        let err = provider
            .lease(&acct, Duration::from_secs(120))
            .await
            .expect_err("刷不了");
        assert!(matches!(err, AuthError::NeedsLogin { .. }), "{err:?}");
        assert!(
            err.message().contains("/auth login grok"),
            "{}",
            err.message()
        );
        assert!(
            !store.load("acct").unwrap().signed_in(),
            "作废的 token 要清掉"
        );
    }

    /// 只登录了身份（没有订阅 scope）：不拿它推理。
    #[tokio::test]
    async fn identity_only_cannot_lease() {
        let store = store();
        let mut cred = credential("acct", 3600);
        cred.scopes = IDENTITY_SCOPES.iter().map(|s| s.to_string()).collect();
        store.save(&cred).unwrap();
        store.set_active("acct").unwrap();
        let provider = GrokProvider::new(store);
        let acct = provider.active_account().unwrap();
        let err = provider
            .lease(&acct, Duration::from_secs(120))
            .await
            .expect_err("没有订阅 scope");
        assert!(matches!(err, AuthError::NotEligible { .. }), "{err:?}");
        assert!(!provider.accounts()[0].plan_usage);
    }

    /// 登出：撤销 refresh token（带公开 client id），本地清 token。
    #[tokio::test]
    async fn logout_revokes_and_clears() {
        let signer = Arc::new(Signer::new());
        let xai = happy_xai(signer, "u").await;
        let store = store();
        store.save(&credential("acct", 3600)).unwrap();
        let provider = GrokProvider::new(store.clone()).with_test_server(&xai.base);
        let acct = key_of(&store.load("acct").unwrap());
        let outcome = provider.logout(&acct, true).await.unwrap();
        assert_eq!(outcome, LogoutOutcome::Revoked);
        let revoke = xai.hits("/revoke");
        assert_eq!(revoke.len(), 1);
        assert_eq!(revoke[0].form["token"], "refresh-old");
        assert_eq!(revoke[0].form["token_type_hint"], "refresh_token");
        assert_eq!(revoke[0].form["client_id"], CLIENT_ID);
        assert!(!store.load("acct").unwrap().signed_in());
    }

    /// 模型：只要语言模型（`/v1/models` 里的图像 / 视频模型不进来），上下文长度从
    /// `/v1/models` 补，收不收图看 `input_modalities`。两个请求都带 access token。
    /// 推理档位按模型目录（models.dev 的 xai）填：有档位的照列，会推理但没档位的是空表，
    /// 不推理的标出来；按 id 找不到再按别名找。
    #[tokio::test]
    async fn models_join_language_models_with_context_lengths() {
        let xai = FakeXai::start(|path, _, _| match path {
            "/v1/language-models" => (
                200,
                json!({"models": [
                    {"id": "grok-4.6", "input_modalities": ["text", "image"], "output_modalities": ["text"]},
                    {"id": "grok-4.20-0309-non-reasoning", "input_modalities": ["text"], "output_modalities": ["text"]},
                    {"id": "grok-4.20-0309-reasoning", "input_modalities": ["text"], "output_modalities": ["text"]},
                    {"id": "grok-9-new", "aliases": ["grok-9"], "input_modalities": ["text"], "output_modalities": ["text"]},
                    {"id": "grok-unknown", "output_modalities": ["text"]},
                ]})
                .to_string(),
            ),
            "/catalog" => (
                200,
                json!({
                    "openai": {"models": {"grok-4.6": {"reasoning": true}}},
                    "xai": {"models": {
                        "grok-4.6": {"reasoning": true, "reasoning_options": [
                            {"type": "effort", "values": ["low", "medium", "high", "xhigh"]}]},
                        "grok-4.20-0309-non-reasoning": {"reasoning": false},
                        "grok-4.20-0309-reasoning": {"reasoning": true, "reasoning_options": []},
                        "grok-9": {"reasoning": true, "reasoning_options": [
                            {"type": "toggle"}, {"type": "effort", "values": ["low", "max"]}]},
                    }},
                })
                .to_string(),
            ),
            "/v1/models" => (
                200,
                json!({"object": "list", "data": [
                    {"id": "grok-4.6", "context_length": 2000000},
                    {"id": "grok-imagine-image", "context_length": 0},
                ]})
                .to_string(),
            ),
            _ => (404, "{}".into()),
        })
        .await;
        let store = store();
        store.save(&credential("acct", 3600)).unwrap();
        let provider = GrokProvider::new(store.clone()).with_test_server(&xai.base);
        let acct = key_of(&store.load("acct").unwrap());
        let models = provider.models(&acct).await.unwrap();
        let slugs: Vec<&str> = models.iter().map(|m| m.slug.as_str()).collect();
        assert_eq!(
            slugs,
            [
                "grok-4.6",
                "grok-4.20-0309-non-reasoning",
                "grok-4.20-0309-reasoning",
                "grok-9-new",
                "grok-unknown"
            ]
        );
        assert_eq!(models[0].context_window, Some(2_000_000));
        assert_eq!(models[0].supports_images, Some(true));
        assert_eq!(models[1].context_window, None);
        assert_eq!(models[1].supports_images, Some(false));
        // 推理档位按目录：没有 none，最高 xhigh（以前一律通用四档 none/low/medium/high）。
        assert_eq!(
            models[0].reasoning_efforts.as_deref(),
            Some(
                &[
                    "low".to_string(),
                    "medium".into(),
                    "high".into(),
                    "xhigh".into()
                ][..]
            )
        );
        assert_eq!(models[0].reasoning, Some(true));
        assert_eq!(models[1].reasoning, Some(false), "不推理的模型");
        assert_eq!(models[1].reasoning_efforts, None);
        assert_eq!(
            models[2].reasoning_efforts,
            Some(vec![]),
            "会推理但不收 effort"
        );
        assert_eq!(
            models[3].reasoning_efforts,
            Some(vec!["low".to_string(), "max".into()]),
            "按别名找到"
        );
        assert_eq!(models[4].reasoning_efforts, None, "目录里没有：不知道");
        assert_eq!(models[4].reasoning, None);
        assert_eq!(
            xai.hits("/v1/language-models")[0].auth.as_deref(),
            Some("Bearer access-old")
        );
    }

    /// 模型目录拉不到（离线 / models.dev 挂了）：模型照样列出来，推理档位留空。
    #[tokio::test]
    async fn models_still_list_without_the_catalog() {
        let xai = FakeXai::start(|path, _, _| match path {
            "/v1/language-models" => (
                200,
                json!({"models": [{"id": "grok-4.6", "output_modalities": ["text"]}]}).to_string(),
            ),
            "/v1/models" => (200, json!({"data": []}).to_string()),
            "/catalog" => (503, "{}".into()),
            _ => (404, "{}".into()),
        })
        .await;
        let store = store();
        store.save(&credential("acct", 3600)).unwrap();
        let provider = GrokProvider::new(store.clone()).with_test_server(&xai.base);
        let acct = key_of(&store.load("acct").unwrap());
        let models = provider.models(&acct).await.expect("目录拉不到不算错");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].reasoning_efforts, None);
        assert_eq!(models[0].reasoning, None);
    }

    /// 实测的几种错误 body → 该有的分类。
    #[test]
    fn real_error_bodies_classify() {
        // 签名改坏的 OAuth token：403，但这是 token 失效，该换一份重试。
        let bad = br#"{"code":"unauthenticated:bad-credentials","error":"The OAuth2 access token could not be validated."}"#;
        let provider = GrokProvider::new(store());
        assert!(provider.token_rejected(403, bad));
        assert!(provider.token_rejected(401, b"{}"));
        assert!(matches!(
            classify(403, bad),
            Some(AuthError::NeedsLogin { .. })
        ));

        // 套餐档位不够：403 但不是 token 的事——不刷新，说清楚。
        let tier = br#"{"code":"permission-denied","error":"The caller does not have permission to execute the specified operation."}"#;
        assert!(!provider.token_rejected(403, tier));
        match classify(403, tier) {
            Some(AuthError::Rejected { code, detail }) => {
                assert_eq!(code, "permission-denied");
                assert!(detail.contains("does not have permission"), "{detail}");
                assert!(detail.contains("api_key"), "{detail}");
            }
            other => panic!("{other:?}"),
        }

        // 模型不收 `reasoning.effort` 的两种说法：都认成「去掉 reasoning.effort」。
        for body in [
            &br#"{"code":"invalid-argument","error":"Model grok-4.20-0309-reasoning does not support parameter reasoningEffort."}"#[..],
            &br#"{"code":"invalid-argument","error":"This model does not support `reasoning_effort` value `none`."}"#[..],
        ] {
            match classify(400, body) {
                Some(AuthError::Unsupported { param }) => {
                    assert_eq!(param.as_deref(), Some("reasoning.effort"))
                }
                other => panic!("{other:?}"),
            }
        }
        // 别的 400 不是鉴权问题。
        assert!(classify(
            400,
            br#"{"code":"invalid-argument","error":"Incorrect API key provided."}"#
        )
        .is_none());
        assert!(classify(404, br#"{"code":"not-found","error":"x"}"#).is_none());

        match classify(429, br#"{"code":"resource-exhausted","error":"slow down"}"#) {
            Some(AuthError::Rejected { code, detail }) => {
                assert_eq!(code, "resource-exhausted");
                assert!(detail.contains("slow down"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            classify(503, b"{}"),
            Some(AuthError::Unavailable { retry: true, .. })
        ));
    }

    /// 实测的那份 `/billing?format=credits`（SuperGrok，2026-11-21）。
    const REAL_BILLING: &str = r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-11-17T02:21:38.373116+00:00","end":"2026-11-24T02:21:38.373116+00:00"},"creditUsagePercent":27.0,"onDemandCap":{"val":0},"onDemandUsed":{"val":0},"productUsage":[{"product":"GrokBuild","usagePercent":27.0}],"isUnifiedBillingUser":true,"prepaidBalance":{"val":0},"topUpMethod":"TOP_UP_METHOD_SAVED_PAYMENT_METHOD","billingPeriodStart":"2026-11-17T02:21:38.373116+00:00","billingPeriodEnd":"2026-11-24T02:21:38.373116+00:00"}}"#;

    #[test]
    fn rfc3339_times_parse_to_unix_seconds() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339("2026-11-24T02:21:38.373116+00:00"),
            Some(1_795_486_898)
        );
        // 时区偏移算回 UTC。
        assert_eq!(
            parse_rfc3339("2026-11-24T10:21:38+08:00"),
            parse_rfc3339("2026-11-24T02:21:38Z")
        );
        assert_eq!(
            parse_rfc3339("2024-02-29T12:00:00-01:30"),
            Some(1_709_213_400)
        );
        for bad in [
            "",
            "2026-13-01T00:00:00Z",
            "yesterday",
            "2026-11-24",
            "2026-11-24T25:00:00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_real_billing_body_parses() {
        let v: Value = serde_json::from_str(REAL_BILLING).unwrap();
        let u = parse_billing(&v, Some("SuperGrok".into()), 1_795_231_000).unwrap();
        assert_eq!(u.plan.as_deref(), Some("SuperGrok"));
        assert_eq!(u.used_percent, Some(27.0));
        let p = u.period.unwrap();
        assert_eq!(p.kind.as_deref(), Some("weekly"));
        assert_eq!(p.start, Some(1_795_486_898 - 7 * 86_400));
        assert_eq!(p.end, Some(1_795_486_898));
        assert_eq!(u.prepaid_balance_cents, Some(0));
        assert_eq!(u.fetched_at, 1_795_231_000);
    }

    /// 本周期一点没用时上游把 `creditUsagePercent` 省掉（proto3）：正处在周期里就是 0；
    /// 不在周期里（或没有周期）就是不知道，不编 0。
    #[test]
    fn an_omitted_percent_is_zero_only_inside_the_period() {
        let body = json!({"config": {
            "currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY",
                "start": "2026-11-17T00:00:00Z", "end": "2026-11-24T00:00:00Z"},
            "prepaidBalance": {},
        }});
        let inside = parse_rfc3339("2026-11-20T00:00:00Z").unwrap();
        let after = parse_rfc3339("2026-11-25T00:00:00Z").unwrap();
        let u = parse_billing(&body, None, inside).unwrap();
        assert_eq!(u.used_percent, Some(0.0));
        assert_eq!(u.prepaid_balance_cents, Some(0), "{{}} 是省掉的 0");
        assert_eq!(
            parse_billing(&body, None, after).unwrap().used_percent,
            None
        );
        let bare = json!({"config": {}});
        let u = parse_billing(&bare, None, inside).unwrap();
        assert_eq!(u.used_percent, None);
        assert_eq!(u.period, None);
        assert_eq!(u.prepaid_balance_cents, None);
    }

    /// 老的账单形状（按月、美分）：已用 / 上限；snake_case 也认；超过 100 按 100。
    #[test]
    fn the_legacy_monthly_shape_still_works() {
        let body = json!({"config": {
            "monthly_limit": {"val": "2000"},
            "used": {"val": 500},
            "billing_period_start": "2026-11-01T00:00:00Z",
            "billing_period_end": "2026-12-01T00:00:00Z",
        }});
        let u = parse_billing(&body, None, 0).unwrap();
        assert_eq!(u.used_percent, Some(25.0));
        let p = u.period.unwrap();
        assert_eq!(p.kind, None);
        assert_eq!(p.end, parse_rfc3339("2026-12-01T00:00:00Z"));
        let over = json!({"config": {"creditUsagePercent": 130.0}});
        assert_eq!(
            parse_billing(&over, None, 0).unwrap().used_percent,
            Some(100.0)
        );
        assert!(parse_billing(&json!({"error": "x"}), None, 0).is_err());
    }

    /// `usage()`：额度 + 套餐名（只查一次就记住）；只带 Bearer。token 被拒（403
    /// `unauthenticated:*`）就刷新一次再查。
    #[tokio::test]
    async fn usage_reads_billing_and_remembers_the_plan() {
        let xai = FakeXai::start(|path, n, form| match path {
            "/proxy/billing?format=credits" if n == 0 => (
                403,
                json!({"code": "unauthenticated:bad-credentials", "error": "x"}).to_string(),
            ),
            "/proxy/billing?format=credits" => (200, REAL_BILLING.to_string()),
            "/proxy/settings" => (
                200,
                json!({"subscription_tier_display": "SuperGrok", "default_model": "x"}).to_string(),
            ),
            "/token" if form.get("grant_type").map(String::as_str) == Some("refresh_token") => (
                200,
                json!({"access_token": "access-refreshed", "refresh_token": "r2",
                       "expires_in": 21600, "scope": SCOPES.join(" ")})
                .to_string(),
            ),
            _ => (404, "{}".into()),
        })
        .await;
        let store = store();
        store.save(&credential("acct", 3600)).unwrap();
        let provider = GrokProvider::new(store.clone()).with_test_server(&xai.base);
        let acct = key_of(&store.load("acct").unwrap());

        let u = provider.usage(&acct).await.unwrap().expect("查得到");
        assert_eq!(u.used_percent, Some(27.0));
        assert_eq!(u.plan.as_deref(), Some("SuperGrok"));
        let billing = xai.hits("/proxy/billing?format=credits");
        assert_eq!(billing.len(), 2, "403 之后刷新、重查一次");
        assert_eq!(billing[0].auth.as_deref(), Some("Bearer access-old"));
        assert_eq!(billing[1].auth.as_deref(), Some("Bearer access-refreshed"));
        assert_eq!(xai.hits("/token").len(), 1);

        let again = provider.usage(&acct).await.unwrap().unwrap();
        assert_eq!(again.plan.as_deref(), Some("SuperGrok"));
        assert_eq!(xai.hits("/proxy/settings").len(), 1, "套餐名查一次就记住");
    }

    /// 只有身份、没有订阅 scope：不查额度，直说没资格。
    #[tokio::test]
    async fn identity_only_has_no_usage() {
        let store = store();
        let mut cred = credential("acct", 3600);
        cred.scopes = IDENTITY_SCOPES.iter().map(|s| s.to_string()).collect();
        store.save(&cred).unwrap();
        let provider = GrokProvider::new(store.clone());
        let err = provider
            .usage(&key_of(&cred))
            .await
            .expect_err("没有订阅 scope");
        assert!(matches!(err, AuthError::NotEligible { .. }), "{err:?}");
    }

    /// 授权服务器不再认这个 client（xAI 收回了第三方用的口子）：别叫用户反复重新登录。
    #[test]
    fn invalid_client_is_not_a_relogin() {
        let err = AuthError::from(TokenError::OAuth {
            code: "invalid_client".into(),
            detail: "Client authentication failed".into(),
            status: 401,
        });
        match err {
            AuthError::Rejected { code, detail } => {
                assert_eq!(code, "invalid_client");
                assert!(detail.contains("api_key"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    }
}
