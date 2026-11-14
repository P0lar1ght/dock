//! 统一鉴权（Auth Manager）。
//!
//! 一家订阅一个 provider 实现（`cordis_base::auth::AuthProvider`）。模型条目
//! 写 `auth = "chatgpt"` 就走订阅通道，没写的老配置一点不变——密钥照旧从
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
mod jwt;
mod manager;
pub(crate) mod oauth;

pub use chatgpt::{chatgpt_auth, ChatGptProvider};
pub use chatgpt::{subscription_disabled, SUBSCRIPTION_ENV};
// 上游错误 → AuthError 的分类（官方那套 `subscription_sharing_*` 错误码）。
// 生产路径经 provider 的 `classify`；推理层的测试用真的分类逻辑配假 provider。
#[cfg(test)]
pub(crate) use chatgpt::classify as classify_upstream_error;
pub use commands::auth_commands;
#[cfg(test)]
pub(crate) use credentials::now_unix as credentials_now;
pub use credentials::{Credential, CredentialStore};
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
