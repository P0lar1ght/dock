//! 统一鉴权（Auth Manager）。
//!
//! 一家订阅一个 provider 实现（`cordis_base::auth::AuthProvider`）。模型条目
//! 写 `auth = "chatgpt"` / `auth = "grok"` 就走订阅通道，没写的老配置一点不变——密钥照旧从
//! `api_key` / `env_key` / `secrets.json` 取。
//!
//! 推理侧只经 [`AuthManager`] 拿 [`cordis_base::auth::Lease`]：短期 access
//! token。refresh token 不出 provider，**子代理与 workflow 也拿不到**。
//!
//! OAuth 的共用原语（PKCE、回环回调、私有文件原子写、跨进程锁）在 [`oauth`]，
//! MCP 的 OAuth 也用它。

mod callback_page;
mod chatgpt;
mod commands;
mod credentials;
mod grok;
mod jwt;
mod manager;
pub(crate) mod oauth;

pub use chatgpt::{chatgpt_auth, ChatGptProvider};
pub use grok::{grok_auth, GrokProvider};
// 上游错误 → AuthError 的分类（官方那套 `subscription_sharing_*` 错误码）。
// 生产路径经 provider 的 `classify`；推理层的测试用真的分类逻辑配假 provider。
#[cfg(test)]
pub(crate) use chatgpt::classify as classify_upstream_error;
// Grok 那条的分类与「token 失效」判定：推理层的测试配假 provider 用真的逻辑。
pub use commands::auth_commands;
#[cfg(test)]
pub(crate) use credentials::now_unix as credentials_now;
pub use credentials::{Credential, CredentialStore};
#[cfg(test)]
pub(crate) use grok::{classify as classify_grok_error, token_rejected as grok_token_rejected};
pub use manager::{auth_manager, register_provider, AuthManager, LoginState};

/// 登录界面：打开系统浏览器 + 记一句状态。
///
/// 终端、桌面端、网关都用这一个——授权页总要在**跑 Dock 的那台机器**上打开。
///
/// `notice` 是提示的出口：终端会把提示弹成一条浮层。授权页地址也走这里——
/// 浏览器没自动打开时，用户得能自己复制那个地址。
pub struct BrowserLoginUi {
    notice: Option<Arc<crate::host::slash::Slash>>,
}

impl BrowserLoginUi {
    /// 不往客户端送提示（网关那种同步等待、结果自己带回去的用法）。
    pub fn plain() -> Self {
        Self { notice: None }
    }

    /// 登录过程往客户端送提示（终端 / 桌面端）。
    pub fn with_notice(slash: Arc<crate::host::slash::Slash>) -> Self {
        Self {
            notice: Some(slash),
        }
    }
}

impl cordis_base::auth::LoginUi for BrowserLoginUi {
    fn open_url(&self, url: &str) {
        // 授权 URL 不进日志：里面有 state / PKCE challenge，日志会被抄去报 bug。
        if let Err(e) = webbrowser::open(url) {
            tracing::warn!(%e, "没能自动打开浏览器，请手动打开提示里的地址");
        }
        // 地址也给出去：浏览器没开起来时，用户得自己复制。
        if let Some(slash) = &self.notice {
            slash.queue_notice(
                "订阅通道",
                format!("在浏览器里完成授权（10 分钟内有效）：\n{url}"),
            );
        }
    }

    fn status(&self, message: &str) {
        tracing::info!(message = message, "订阅登录");
        if let Some(slash) = &self.notice {
            slash.queue_notice("订阅通道", message.to_string());
        }
    }
}

use std::sync::Arc;

use cordis_base::auth::WirePolicy;

/// SIWC（ChatGPT 套餐）那条通道的 wire 约束。
///
/// 多发一个字段上游就是 400：system 走 `instructions`、`store: false`、
/// `stream: true`，再把官方不收的那 15 个字段摘掉。
///
/// 先放在这儿（provider 插件 P1 才落地）：策略是「这条通道的规矩」，写成
/// 数据后推理层不用知道是哪家。
pub fn chatgpt_policy() -> WirePolicy {
    WirePolicy {
        force_store_false: true,
        force_stream: true,
        system_as_instructions: true,
        strip_fields: &[
            "background",
            "conversation",
            "max_output_tokens",
            "max_tool_calls",
            "metadata",
            "moderation",
            "multi_agent",
            "prompt",
            "prompt_cache_retention",
            "safety_identifier",
            "temperature",
            "top_logprobs",
            "top_p",
            "truncation",
            "user",
        ],
        allow_previous_response_id: false,
        hosted_tools: &["web_search"],
        // `store: false` → 服务端不留存，推理项的 id 下次就不认了：必须把
        // 加密内容要回来（多轮回传靠它）。
        include_fields: &["reasoning.encrypted_content"],
        // 不在禁发的 15 个字段里，实测上游收（200）。
        prompt_cache_key: true,
        // 这条通道按请求头做会话路由，跟官方客户端 Codex 发的一样。2026-11-21
        // 用 gpt-6-luna 实测（每组连发几步，后一步追加前一步的输出）：只带
        // `prompt_cache_key`，后续 20 步 0 次命中；加上这三个头并回传
        // turn-state，60 步里 51 步命中（每次约占输入的 95%）。实测记录见
        // `docs/CHATGPT-SIWC.md`「前缀缓存」。
        session_headers: &["session-id", "thread-id", "x-client-request-id"],
        turn_state_header: Some("x-codex-turn-state"),
    }
}

/// Grok 订阅（`api.x.ai` + `auth.x.ai` 的 OAuth token）那条通道的 wire 约束。
///
/// `api.x.ai/v1/responses` 就是公开 API 那一套 Responses，请求体**不用改**，
/// 只多带 `prompt_cache_key`（xAI 文档：它等同于 `x-grok-conv-id` 头，把同一会话
/// 路由到同一台机器）。2026-11-21 实测（grok-4.6，约 6.2k token 的前缀，连发 4 步、
/// 后一步追加前一步的输出）：带了之后 3 步全中（每次 96–98%），不带只中 1 步。
pub fn grok_policy() -> WirePolicy {
    WirePolicy {
        prompt_cache_key: true,
        ..WirePolicy::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grok 那条只加 `prompt_cache_key`，别的一概不改：多改一样就不是公开 API 的请求了。
    #[test]
    fn grok_policy_only_adds_the_cache_key() {
        let policy = grok_policy();
        assert!(policy.prompt_cache_key);
        assert_eq!(
            policy,
            WirePolicy {
                prompt_cache_key: true,
                ..WirePolicy::default()
            }
        );
    }

    /// 官方不收的字段一个都不能少：少一个就是 400。
    #[test]
    fn chatgpt_policy_strips_the_fifteen_fields() {
        let policy = chatgpt_policy();
        assert_eq!(policy.strip_fields.len(), 15);
        for field in [
            "background",
            "conversation",
            "max_output_tokens",
            "max_tool_calls",
            "metadata",
            "moderation",
            "multi_agent",
            "prompt",
            "prompt_cache_retention",
            "safety_identifier",
            "temperature",
            "top_logprobs",
            "top_p",
            "truncation",
            "user",
        ] {
            assert!(policy.strip_fields.contains(&field), "少了 {field}");
        }
        assert!(policy.force_store_false);
        assert!(policy.force_stream);
        assert!(policy.system_as_instructions);
        assert!(!policy.allow_previous_response_id);
    }

    /// 少了会话头 / turn-state，这条通道每次请求都落到别的机器，前缀缓存几乎不中。
    #[test]
    fn chatgpt_policy_routes_by_session_headers_and_turn_state() {
        let policy = chatgpt_policy();
        assert!(policy.prompt_cache_key);
        assert_eq!(
            policy.session_headers,
            &["session-id", "thread-id", "x-client-request-id"]
        );
        assert_eq!(policy.turn_state_header, Some("x-codex-turn-state"));
    }
}
