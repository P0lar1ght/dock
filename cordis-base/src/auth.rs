//! 订阅鉴权（Auth Manager）的类型与 trait。
//!
//! 一家订阅一个 [`AuthProvider`] 实现：登录、续期、登出、账号表、拉模型、把
//! 上游的 HTTP 错误归一。推理侧只拿 [`Lease`]——短期 access token，不碰 refresh
//! token（子代理、workflow 都只经它拿）。
//!
//! **账号状态归 provider 管**（它的凭据在盘上）：命令、网关、界面都经 trait 读
//! 账号表，不直接碰哪一家的凭据文件——加第二家时不用每处都改。
//!
//! 这里**只放类型与 trait**：本 crate 没有插件，也不 provide named service，
//! 更不认识 ctx 键。服务本体在 `cordis-spine` 的 `auth` 模块。

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, SystemTime};

/// provider 下的一个账号。
///
/// `subject` 是账号在 provider 那边的稳定标识，`client_id` 是这个账号在本机的
/// 注册标识（OpenAI 那边是首登时拿到的正式 client，形如 `oaiapp_...`）——凭据
/// 按它分文件存，命令与界面也按它指账号。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccountKey {
    pub provider: String,
    pub subject: String,
    pub client_id: String,
}

/// 推理侧唯一能拿到的东西：短期 access token + 过期时间 + 代数。
///
/// **不含 refresh token**：能长期续期的东西不落到推理路径上。
#[derive(Clone)]
pub struct Lease {
    pub bearer: String,
    pub expires_at: SystemTime,
    /// 凭据代数。刷新一次涨一：401 时比较代数，就知道是 token 旧了还是
    /// 账号本身被撤了（见 [`AuthProvider::on_unauthorized`]）。
    pub generation: u64,
    pub account: AccountKey,
}

impl Lease {
    /// 至少还能用 `min_valid` 那么久。
    pub fn valid_for(&self, min_valid: Duration) -> bool {
        self.expires_at
            .duration_since(SystemTime::now())
            .map(|left| left >= min_valid)
            .unwrap_or(false)
    }
}

/// 手写 `Debug`：token 不进日志。
///
/// 不是「不实现 `Debug`」——那样一来 `{:?}` 编译不过，调用点就得自己绕，
/// 迟早有人图省事把 token 拼进消息里。脱敏版的 `Debug` 才能真的挡住。
impl fmt::Debug for Lease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lease")
            .field("bearer", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("generation", &self.generation)
            .field("account", &self.account)
            .finish()
    }
}

/// 这条通道对请求体的硬约束。
///
/// 「上游不收的字段」是各家自己定的：SIWC（ChatGPT 套餐）那条通道比普通
/// OpenAI 兼容端点严得多——多发一个字段就是 400。约束写成数据，推理层就
/// 不用知道是哪家。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WirePolicy {
    /// 强制 `store: false`（上游不留存这次请求）。
    pub force_store_false: bool,
    /// 强制 `stream: true`。
    pub force_stream: bool,
    /// system 提示改走 `instructions`，`input` 里不再留 `role: "system"`。
    pub system_as_instructions: bool,
    /// 请求体里要去掉的字段（上游不收，发了会被拒）。
    pub strip_fields: &'static [&'static str],
    /// 能不能用 `previous_response_id` 续接。Dock 现在压根不发它，这个
    /// 开关只表达通道允不允许。
    pub allow_previous_response_id: bool,
    /// 请求体里要带的 `include` 取值（`["reasoning.encrypted_content"]` 之类）。
    ///
    /// **`store: false` 的通道必须带 `reasoning.encrypted_content`**：不留存
    /// 就意味着服务端不认识推理项的 id，多轮回传时得带上加密内容才认。
    pub include_fields: &'static [&'static str],
    /// 这条通道准用的托管工具。
    pub hosted_tools: &'static [&'static str],
}

/// 统一的鉴权错误分类。上游的状态码与 body 由 provider 归一到这里。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// 要登录（没登录、登出了，或 token 作废且刷新不了）。
    NeedsLogin { reason: String },
    /// 这个账号没资格用这条通道（套餐不支持之类）。
    NotEligible { code: String },
    /// 用量到上限了。**不重试**：重试只是再撞一次墙。
    QuotaExhausted {
        code: String,
        request_id: Option<String>,
    },
    /// 上游暂时不可用。`retry` = 可以退避重试。
    Unavailable { code: String, retry: bool },
    /// 请求里有上游不收的东西。`param` 是被拒的字段（`error.param`）。
    Unsupported { param: Option<String> },
    /// 上游按策略拒了：地区限制、路由不对、client / 授权配置不对……
    /// **不重试、也不换计费方式**——换了也还是这个结果。
    Rejected { code: String, detail: String },
    /// 传输层失败。
    Transport(String),
}

impl AuthError {
    /// 上游给的错误码（没有就空串）。
    pub fn code(&self) -> &str {
        match self {
            AuthError::NeedsLogin { .. } => "needs_login",
            AuthError::NotEligible { code } => code,
            AuthError::QuotaExhausted { code, .. } => code,
            AuthError::Unavailable { code, .. } => code,
            AuthError::Unsupported { .. } => "unsupported",
            AuthError::Rejected { code, .. } => code,
            AuthError::Transport(_) => "transport",
        }
    }

    /// 值不值得退避重试。
    pub fn retryable(&self) -> bool {
        match self {
            AuthError::Unavailable { retry, .. } => *retry,
            _ => false,
        }
    }

    /// 给用户看的一句中文。
    pub fn message(&self) -> String {
        match self {
            AuthError::NeedsLogin { reason } => format!("需要重新登录：{reason}"),
            AuthError::NotEligible { code } => {
                format!(
                    "这个账号/工作空间没有这条通道的资格（{code}）：换一个账号，或改用别的计费方式（比如 API key）。"
                )
            }
            // 官方：上限可能是套餐的，也可能是这个应用的——别说成「套餐用光了」。
            AuthError::QuotaExhausted { code, request_id } => {
                let id = request_id
                    .as_deref()
                    .map(|id| format!("，request-id: {id}"))
                    .unwrap_or_default();
                format!(
                    "用量到上限了（{code}{id}）：可能是套餐的上限，也可能是这个应用的上限。\
                     到 ChatGPT 设置的 Usage 页看看（Manage usage）；额度用完不会自动转成按量计费。"
                )
            }
            AuthError::Unavailable { code, .. } => format!("上游暂时不可用（{code}），稍后再试"),
            AuthError::Unsupported { param } => match param {
                Some(p) => format!("这条通道不收请求里的「{p}」"),
                None => "这条通道不收这个请求".to_string(),
            },
            AuthError::Rejected { code, detail } => {
                if detail.is_empty() {
                    format!("上游拒绝了这个请求（{code}）")
                } else {
                    format!("上游拒绝了这个请求（{code}）：{detail}")
                }
            }
            AuthError::Transport(detail) => format!("连接失败：{detail}"),
        }
    }
}

/// 上游列出来的、**可以给用户选的**模型（上游标成隐藏的由 provider 先滤掉）。
///
/// 除 `slug` 外都是可选的：官方文档只承诺 `slug` / `display_name`，其余是上游
/// 实际会给、有就用的元数据（目录里的上下文窗口、`/effort` 菜单、能不能收图）。
/// 也是 `$DOCK_HOME/auth_models.json` 里每一条的格式。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoteModel {
    /// 发给上游的 slug。
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// 上游给的一句介绍（英文原样）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// 认哪几档推理强度（按上游顺序）。`None` = 上游没说。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<String>>,
    /// 默认推理强度。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,
    /// 收不收图片输入。`None` = 上游没说。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_images: Option<bool>,
}

/// 401 之后 provider 的处置。
#[derive(Clone, Debug)]
pub enum Retry {
    /// 换这份 lease 重试一次。
    WithLease(Lease),
    /// 不重试，直接报错。
    GiveUp,
}

/// 登出的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogoutOutcome {
    /// 远端已撤销。
    Revoked,
    /// 本地已清，远端撤销没确认。
    LocalOnly { reason: String },
}

/// 给界面看的 provider 信息（不随账号变）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderInfo {
    /// 给人看的名字（`"ChatGPT"`）：登录按钮、「Using … plan」标识用它。
    pub display_name: &'static str,
    /// 用量页（「Manage usage」）。没有就不显示那个入口。
    pub usage_url: Option<&'static str>,
}

/// 账号表里的一行（**不含任何 token**）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSummary {
    pub account: AccountKey,
    pub email: Option<String>,
    /// 还登着（有能续期的凭据）。`false` = 登出了，但**注册留着**：下次登录
    /// 直接复用这个账号的注册（官方：登出不删已注册的 client）。
    pub signed_in: bool,
    /// 这份授权能不能拿套餐额度推理（ChatGPT：scope 里有套餐那一条）。
    pub plan_usage: bool,
    /// access token 的过期时刻（unix 秒；登出了是 0）。
    pub expires_at: u64,
    /// 是不是当前活动账号。
    pub active: bool,
}

/// 登录要人看的那部分：打开授权 URL、报状态。TUI / GUI / headless 各自实现。
pub trait LoginUi: Send + Sync {
    /// 打开授权 URL（能开系统浏览器就开）。
    fn open_url(&self, url: &str);
    /// 给用户报一句进度。
    fn status(&self, message: &str);
}

/// trait 里那些异步方法的返回类型。`cordis-base` 不依赖内核 crate，
/// 只能用标准库的 `Future` 自己盒起来。
pub type AuthFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 登录要哪一套 scope。
///
/// `Default` = provider 自己的完整 scope；`IdentityOnly` = **只要身份**，
/// 不带 provider 特有的额外授权（比如 ChatGPT 的套餐额度）。
///
/// 为什么要有后者：官方目录里有一类 app 就是「sign-in only」（GitLab、Canva……），
/// 身份登录人人都能用，额外权限（套餐额度）只开放给目录里的集成。只登录这一条
/// 能把 OAuth、验签、落盘、刷新整条链路跑通。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScopeMode {
    #[default]
    Default,
    IdentityOnly,
}

impl fmt::Display for ScopeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ScopeMode::Default => "默认",
            ScopeMode::IdentityOnly => "仅身份",
        })
    }
}

/// 这次登录给哪个账号。
///
/// 官方要求：用户能**选一个已有账号**重新登录，也能**再加一个**。已注册过的账号
/// 复用它的注册（不再走首登那一步），新加的才走首登。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LoginTarget {
    /// 活动账号（登出了也算——注册还在）；一个账号都没有就新加。
    #[default]
    Active,
    /// 指定账号（[`AccountKey::client_id`]）重新登录。
    Account(String),
    /// 新加一个账号。
    New,
}

/// 一次登录请求。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoginRequest {
    pub scope: ScopeMode,
    pub target: LoginTarget,
}

/// 一家订阅一个实现。
pub trait AuthProvider: Send + Sync {
    /// provider id（`"chatgpt"` 这样）。模型条目的 `auth = "..."` 按它找实现。
    fn id(&self) -> &'static str;

    /// 给界面看的信息。
    fn info(&self) -> ProviderInfo;

    /// 这条通道的请求体约束。
    fn wire_policy(&self) -> &WirePolicy;

    /// 推理基址。`None` = 用模型条目里写的 `api_base_url`。
    fn api_base(&self) -> Option<&str> {
        None
    }

    /// 登录：首登 / 重新授权 / 新加账号（看 `req.target`）。成功后这个账号成为活动账号。
    fn login<'a>(
        &'a self,
        ui: &'a dyn LoginUi,
        req: LoginRequest,
    ) -> AuthFuture<'a, Result<AccountKey, AuthError>>;

    /// 本机保存的账号（含登出了、注册还留着的）。**每次都读盘**：另一个 Dock
    /// 进程刚登录 / 登出，这边立刻看得到。
    fn accounts(&self) -> Vec<AccountSummary>;

    /// 当前活动账号（读盘，理由同上）。
    fn active_account(&self) -> Option<AccountKey>;

    /// 切活动账号。账号不在、或已经登出了就报错（登出的要先重新登录）。
    fn set_active(&self, account_id: &str) -> Result<AccountKey, AuthError>;

    /// 导入别处传来的凭据（VPS 场景）。默认不支持。
    fn import(&self, _raw: &str) -> Result<AccountKey, AuthError> {
        Err(AuthError::Rejected {
            code: "import_unsupported".into(),
            detail: format!("{} 不支持导入凭据", self.id()),
        })
    }

    /// 拿一份至少还能用 `min_valid` 的 access token。
    fn lease<'a>(
        &'a self,
        acct: &'a AccountKey,
        min_valid: Duration,
    ) -> AuthFuture<'a, Result<Lease, AuthError>>;

    /// 上游说 token 不好使了：换一份重试，还是直接报错。
    fn on_unauthorized<'a>(&'a self, lease: &'a Lease) -> AuthFuture<'a, Result<Retry, AuthError>>;

    /// 登出。`revoke` = 要不要顺带撤销远端的 refresh token。
    ///
    /// **只清 token，不删注册**：账号与 client 的对应、本机 host id 都留着，
    /// 下次登录直接复用。
    fn logout<'a>(
        &'a self,
        acct: &'a AccountKey,
        revoke: bool,
    ) -> AuthFuture<'a, Result<LogoutOutcome, AuthError>>;

    /// 列这个账号能用的模型。
    fn models<'a>(
        &'a self,
        acct: &'a AccountKey,
    ) -> AuthFuture<'a, Result<Vec<RemoteModel>, AuthError>>;

    /// 把上游的 HTTP 错误归一成 [`AuthError`]。`None` = 不是鉴权问题。
    fn classify(&self, status: u16, body: &[u8]) -> Option<AuthError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease() -> Lease {
        Lease {
            bearer: "secret-token".into(),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
            generation: 3,
            account: AccountKey {
                provider: "chatgpt".into(),
                subject: "user-1".into(),
                client_id: "oaiapp_x".into(),
            },
        }
    }

    /// token 不许出现在任何 Debug 输出里。
    #[test]
    fn lease_debug_redacts_the_token() {
        let text = format!("{:?}", lease());
        assert!(!text.contains("secret-token"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");
    }

    #[test]
    fn lease_valid_for_uses_the_deadline() {
        let l = lease();
        assert!(l.valid_for(Duration::from_secs(60)));
        assert!(!l.valid_for(Duration::from_secs(7200)));
    }

    /// 用量上限不重试；提示不能说成「套餐用光了」（官方：可能是应用的上限）。
    #[test]
    fn quota_exhausted_is_not_retryable() {
        let err = AuthError::QuotaExhausted {
            code: "subscription_sharing_usage_limit_exceeded".into(),
            request_id: Some("req_1".into()),
        };
        assert!(!err.retryable());
        let text = err.message();
        assert!(text.contains("应用的上限"), "{text}");
        assert!(text.contains("req_1"), "{text}");
    }

    #[test]
    fn unavailable_marks_whether_to_retry() {
        let err = AuthError::Unavailable {
            code: "subscription_sharing_usage_unavailable".into(),
            retry: true,
        };
        assert!(err.retryable());
        assert_eq!(err.code(), "subscription_sharing_usage_unavailable");
    }

    /// 「不收这个字段」的提示**不许说已经去掉了**——去没去掉是推理层的事，
    /// 走到这句提示时就是没去掉（或去掉了还是不行）。
    #[test]
    fn unsupported_does_not_claim_the_field_was_removed() {
        let text = AuthError::Unsupported {
            param: Some("temperature".into()),
        }
        .message();
        assert!(text.contains("temperature"), "{text}");
        assert!(!text.contains("已去掉"), "{text}");
    }

    #[test]
    fn rejected_is_not_retryable() {
        let err = AuthError::Rejected {
            code: "chatpass_v2_scope_not_authorized".into(),
            detail: "检查 client 与授权配置".into(),
        };
        assert!(!err.retryable());
        assert_eq!(err.code(), "chatpass_v2_scope_not_authorized");
        assert!(err.message().contains("授权配置"));
    }
}
