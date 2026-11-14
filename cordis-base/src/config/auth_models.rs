//! 订阅通道自动发现的模型：`$DOCK_HOME/auth_models.json`。
//!
//! 登录、切账号、导入凭据之后，鉴权服务（`cordis-spine` 的 `AuthManager`）向
//! 上游列一次模型，把**活动账号**能用的那几个写进这个文件；登出、套餐额度没开
//! 就把那一家的条目删掉。读目录（[`super::load_catalog`]）时并进来，`/model`、
//! 桌面端模型菜单、网关 `model/list` 不用各自去问上游。
//!
//! 每条变成目录里的 `<provider>/<slug>`（如 `chatgpt/gpt-6-luna`），`auth` 指向
//! 那一家；config 里同 id 的 `[model."chatgpt/gpt-6-luna"]` 可以逐字段覆盖。
//!
//! 本文件只有类型、读和映射；写在 spine（原子写、0600）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{ApiBackend, ModelChoice};
use crate::auth::RemoteModel;

pub const FILE: &str = "auth_models.json";
pub const SCHEMA_VERSION: u32 = 1;

pub fn path() -> PathBuf {
    super::dock_home().join(FILE)
}

/// 整个文件：provider id → 那一家活动账号的模型。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthModelCache {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderModels>,
}

/// 一家 provider 的模型。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderModels {
    /// 给人看的名字（`ChatGPT`），目录里的说明用它。
    pub display_name: String,
    /// 列表是哪个账号（client id）拉来的：切了账号就该重拉。
    pub account: String,
    /// 拉取时间（unix 秒）。
    pub fetched_at: u64,
    /// 按上游顺序。
    #[serde(default)]
    pub models: Vec<RemoteModel>,
}

impl AuthModelCache {
    /// 读文件。没有文件 = 空；坏了也当空（记一条警告），不让目录跟着读不出来。
    pub fn read(path: &Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(path = %path.display(), error = %e, "auth_models.json 读不了，当作空");
            Self::default()
        })
    }

    pub fn to_json(&self) -> String {
        let mut out = self.clone();
        out.schema_version = SCHEMA_VERSION;
        serde_json::to_string_pretty(&out).unwrap_or_else(|_| "{}".into())
    }
}

/// 目录里的 id：`<provider>/<slug>`。
pub fn model_id(provider: &str, slug: &str) -> String {
    format!("{provider}/{slug}")
}

/// 文件里的模型 → 目录条目（订阅通道：`/responses`、`auth = <provider>`）。
pub fn catalog_entries(cache: &AuthModelCache) -> Vec<ModelChoice> {
    let mut out = Vec::new();
    for (provider, entry) in &cache.providers {
        for model in &entry.models {
            if model.slug.trim().is_empty() {
                continue;
            }
            out.push(choice(provider, &entry.display_name, model));
        }
    }
    out
}

fn choice(provider: &str, provider_name: &str, model: &RemoteModel) -> ModelChoice {
    let efforts = model.reasoning_efforts.clone();
    let provider_name = if provider_name.trim().is_empty() {
        provider
    } else {
        provider_name
    };
    ModelChoice {
        id: model_id(provider, &model.slug),
        name: model
            .display_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| model.slug.clone()),
        description: format!("{provider_name} 套餐"),
        api_base_url: None,
        api_key: None,
        env_key: None,
        context_window: model.context_window.filter(|n| *n > 0),
        api_backends: vec![ApiBackend::Responses],
        backend_overrides: BTreeMap::new(),
        auth_scheme: None,
        api_model: Some(model.slug.clone()),
        prompt_cache: None,
        max_output_tokens: None,
        // 上游列了档位就是会推理的模型；没说就跟着 `/think` 走。
        reasoning: efforts.as_ref().map(|e| !e.is_empty()).filter(|r| *r),
        reasoning_effort: model.default_reasoning_effort.clone(),
        reasoning_efforts: efforts.filter(|e| !e.is_empty()),
        supports_images: model.supports_images,
        // 用的是套餐额度：没有按量单价。
        pricing: None,
        provider: None,
        auth: Some(provider.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> AuthModelCache {
        let mut providers = BTreeMap::new();
        providers.insert(
            "chatgpt".to_string(),
            ProviderModels {
                display_name: "ChatGPT".into(),
                account: "oaiapp_a".into(),
                fetched_at: 1,
                models: vec![
                    RemoteModel {
                        slug: "gpt-6-luna".into(),
                        display_name: Some("GPT-6-Luna".into()),
                        description: Some("Fast".into()),
                        context_window: Some(272_000),
                        reasoning_efforts: Some(vec!["low".into(), "max".into()]),
                        default_reasoning_effort: Some("medium".into()),
                        supports_images: Some(true),
                    },
                    RemoteModel {
                        slug: "bare".into(),
                        ..Default::default()
                    },
                ],
            },
        );
        AuthModelCache {
            schema_version: SCHEMA_VERSION,
            providers,
        }
    }

    #[test]
    fn models_become_subscription_catalog_entries() {
        let list = catalog_entries(&cache());
        assert_eq!(list.len(), 2);
        let luna = &list[0];
        assert_eq!(luna.id, "chatgpt/gpt-6-luna");
        assert_eq!(luna.name, "GPT-6-Luna");
        assert_eq!(luna.description, "ChatGPT 套餐");
        assert_eq!(
            luna.api_model.as_deref(),
            Some("gpt-6-luna"),
            "发给上游的是 slug"
        );
        assert_eq!(luna.auth_provider(), Some("chatgpt"));
        assert_eq!(luna.api_backends, vec![ApiBackend::Responses]);
        assert_eq!(luna.context_window, Some(272_000));
        assert_eq!(luna.reasoning, Some(true));
        assert_eq!(luna.reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(luna.reasoning_efforts.as_deref().map(<[_]>::len), Some(2));
        assert_eq!(luna.supports_images, Some(true));
        assert!(luna.has_http(), "订阅通道的模型算配好了端点");
        assert!(luna.pricing.is_none(), "套餐额度没有按量单价");

        // 上游什么都没说：名字用 slug，推理跟着 /think，档位用通用菜单。
        let bare = &list[1];
        assert_eq!(bare.name, "bare");
        assert_eq!(bare.reasoning, None);
        assert_eq!(bare.reasoning_efforts, None);
    }

    #[test]
    fn a_missing_or_broken_file_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert_eq!(AuthModelCache::read(&path), AuthModelCache::default());
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(AuthModelCache::read(&path), AuthModelCache::default());
        std::fs::write(&path, cache().to_json()).unwrap();
        assert_eq!(AuthModelCache::read(&path), cache());
    }
}
