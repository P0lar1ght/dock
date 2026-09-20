//! Dock config.toml — Grok's `$GROK_HOME/config.toml` shape, trimmed.
//!
//! Catalog merge (same order as Grok `resolve_model_list`):
//! user `~/.dock/config.toml` < project `.dock/config.toml`.
//! `[models].catalog` sets the list; `[model.<id>]` adds/overrides. 没有配置文件
//! 就没有模型——内置目录已经删掉，不再假装有可用端点。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Deserialize;

/// Grok `ApiBackend`: which inference wire the `llm` plugin speaks.
///
/// 默认是 Responses：coding agent 依赖推理链跨轮回放、块级工具结果、item 可寻址，
/// 这三件事只有 item/block 序列模型表达得了。chat/completions 仍是覆盖面最广的
/// 那条，端点只支持它就显式声明。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApiBackend {
    #[default]
    Responses,
    ChatCompletions,
    Messages,
}

impl ApiBackend {
    /// `/protocol` 菜单在目录里没有当前模型时列的通用三条，顺序 = 默认优先。
    pub const ALL: &'static [ApiBackend] =
        &[Self::Responses, Self::ChatCompletions, Self::Messages];

    /// 认得出来才返回。解析**声明列表**用这个：不认识的项要能丢掉，而不是
    /// 悄悄变成默认值，否则 `api_backends = ["responses", "typo"]` 会得到两条
    /// Responses。
    pub fn from_name(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "responses" | "resp" => Some(Self::Responses),
            "messages" | "anthropic" => Some(Self::Messages),
            "chat_completions" | "chat-completions" | "chat" | "completions" => {
                Some(Self::ChatCompletions)
            }
            _ => None,
        }
    }

    /// 单数 `api_backend` 的解析：缺省 / 不认识都退回默认。
    pub fn parse(raw: Option<&str>) -> Self {
        let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::default();
        };
        Self::from_name(raw).unwrap_or_else(|| {
            tracing::warn!(api_backend = raw, "unknown api_backend; using responses");
            Self::default()
        })
    }

    pub fn path(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat/completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }

    /// config.toml 里写的名字，也是 `/protocol <name>` 接受的名字。
    pub fn name(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }

    /// `/protocol` 菜单里的一句话说明。
    pub fn description(self) -> &'static str {
        match self {
            Self::ChatCompletions => "OpenAI /chat/completions · 覆盖面最广",
            Self::Responses => "OpenAI /responses · 推理链可跨轮回放",
            Self::Messages => "Anthropic /messages · 显式缓存断点",
        }
    }
}

/// Grok `AuthScheme`. Independent of [`ApiBackend`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AuthScheme {
    #[default]
    Bearer,
    XApiKey,
}

impl AuthScheme {
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("x_api_key") | Some("x-api-key") => Some(Self::XApiKey),
            Some("bearer") => Some(Self::Bearer),
            _ => None,
        }
    }

    /// 没显式写 `auth_scheme` 时按**当前这条 wire** 猜——同一个端点切到
    /// /messages 就该换头，所以这跟着运行时协议走，不是模型的固定属性。
    pub fn default_for(backend: ApiBackend) -> Self {
        match backend {
            ApiBackend::Messages => Self::XApiKey,
            ApiBackend::ChatCompletions | ApiBackend::Responses => Self::Bearer,
        }
    }
}

/// `[model.<id>].reasoning_efforts` 没写时 `/effort` 列的通用档位。
pub const DEFAULT_EFFORT_CHOICES: &[&str] = &["low", "medium", "high", "xhigh"];

/// `[model.<id>.pricing]` —— 单价，**USD / 百万 token**（全行业通用的报价单位）。
///
/// 存 tick（1 USD = 1e10）而不是 f64：`ModelChoice` 要 `Eq`，浮点没有；而且
/// tick 本来就是整数单位，省掉一路浮点误差。
///
/// 不内置任何厂商价格表。价格变动频繁，按模型名猜出来的单价一旦过期就是**静默
/// 给出错误金额**，比不显示更糟——这也是 `[[models.catalog]]` 内置条目当初被
/// 删掉的同一个理由。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelPricing {
    /// 未命中输入（Claude Code `/cost` 里的 "input"）。
    pub input_ticks_per_mtok: i64,
    pub cache_read_ticks_per_mtok: i64,
    /// 缓存写入。config 里省略则回落到 `input`——多数厂商写入就是原价，
    /// Anthropic 那种 1.25x 的要显式写。
    pub cache_write_ticks_per_mtok: i64,
    /// 输出。多数厂商把推理 token 计在这里，所以不单列。
    pub output_ticks_per_mtok: i64,
}

impl ModelPricing {
    /// 一次调用的估算费用（tick）。分段口径和 `/usage` 完全一致：
    /// 未命中 / 缓存读 / 缓存写互不相交，各按各的单价。
    ///
    /// 用 `i128` 中转：1e6 token × 1e12 tick/Mtok 已经贴近 `i64` 上限。
    pub fn cost_ticks(
        &self,
        uncached_input: u64,
        cache_read: u64,
        cache_write: u64,
        output: u64,
    ) -> i64 {
        let part = |tokens: u64, per_mtok: i64| -> i128 {
            i128::from(tokens) * i128::from(per_mtok) / 1_000_000
        };
        let total = part(uncached_input, self.input_ticks_per_mtok)
            + part(cache_read, self.cache_read_ticks_per_mtok)
            + part(cache_write, self.cache_write_ticks_per_mtok)
            + part(output, self.output_ticks_per_mtok);
        total.clamp(0, i128::from(i64::MAX)) as i64
    }

    /// 四个价位全是 0 = 没写价，不该拿 $0 冒充"免费"。
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

/// `$X / 1M tokens` → tick。负数与非有限值当没写。
fn usd_per_mtok_to_ticks(raw: Option<f64>) -> i64 {
    raw.filter(|v| v.is_finite() && *v > 0.0)
        .map(|v| (v * crate::usage::USD_TICKS_PER_USD).round() as i64)
        .unwrap_or(0)
}

/// `[model.<id>.<protocol>]` —— 同一个模型在**某一条 wire 上**的连接差异。
///
/// 起因：一个端点支持所有协议，但各协议的入口不同。DeepSeek 的 OpenAI 侧是
/// `https://api.deepseek.com`，Anthropic 侧是 `https://api.deepseek.com/anthropic`。
/// 没有这一层就只能退回「一个协议一条模型条目」，等于把刚合并掉的重复又配回来。
///
/// 这里只放**连接**相关的键。能力（`context_window` / `reasoning` /
/// `supports_images` / …）是模型的属性，不随 wire 变，不给覆盖——那正是
/// 「一个模型一条 entry」要守住的东西。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackendOverride {
    /// 这条 wire 自己的基址。None = 用模型的 `api_base_url`。
    pub api_base_url: Option<String>,
    /// 这条 wire 自己的鉴权头。None = 用模型的 `auth_scheme`，再没有就按协议默认。
    pub auth_scheme: Option<AuthScheme>,
    /// 这条 wire 上的模型 slug。None = 用模型的 `api_model` / 目录 id。
    pub api_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Per-model OpenAI-compatible base (`[model.<id>].api_base_url`).
    pub api_base_url: Option<String>,
    /// Per-model bearer token (`[model.<id>].api_key`).
    pub api_key: Option<String>,
    /// Env var name for the bearer token when `api_key` is empty.
    pub env_key: Option<String>,
    pub context_window: Option<u64>,
    /// `[model.<id>].api_backends` —— 这个端点**声明支持**哪几条 wire，按声明
    /// 顺序，第一条是切到该模型时的默认。单数 `api_backend` = 只写一条。
    ///
    /// 一个端点常常同时开着 /responses 和 /chat/completions；以前要为此配两个
    /// 模型条目（靠 `api_model` 指向同一个上游 slug），`/model` 里就多出一行只
    /// 有协议不同的重复项。声明成一行、运行时用 `/protocol` 切。
    ///
    /// 解析后**永不为空**：空列表退回 `[ApiBackend::default()]`。
    pub api_backends: Vec<ApiBackend>,
    /// `[model.<id>.<protocol>]` 的按协议连接覆盖（基址 / 鉴权 / slug）。
    /// 同一个端点各协议入口不同时用它，见 [`BackendOverride`]。
    pub backend_overrides: BTreeMap<ApiBackend, BackendOverride>,
    /// `None` = 跟着当前协议走（[`AuthScheme::default_for`]）。
    pub auth_scheme: Option<AuthScheme>,
    /// Wire slug in the JSON body. None = use [`Self::id`] (picker key).
    pub api_model: Option<String>,
    /// `[model.<id>].prompt_cache`. Messages 后端的 `cache_control` 断点开关，
    /// 缺省开。指向不认这个字段的自建 /v1/messages 代理时置 false。
    pub prompt_cache: Option<bool>,
    /// `[model.<id>].max_output_tokens`. 缺省不发（Messages 除外，那边是必填项）。
    pub max_output_tokens: Option<u32>,
    /// `[model.<id>].reasoning`. false = 这个模型没有推理档，任何推理参数都不发。
    /// 缺省（None）跟着运行时的 `/think` 开关走。
    pub reasoning: Option<bool>,
    /// `[model.<id>].reasoning_effort`. 该模型的默认强度，缺省不发、由上游决定。
    pub reasoning_effort: Option<String>,
    /// `[model.<id>].reasoning_efforts`. 这个模型**认识**哪几档，`/effort` 的菜单
    /// 照着列。各家不一样（有的只有 low/high，有的多一档 minimal），写死一份通用
    /// 列表就会让人选到上游不认的值。
    ///
    /// 三态：`None` = 没写，菜单给通用四档；`Some([...])` = 就这几档；
    /// **`Some([])` = 这个模型会推理但不接受档位参数**（菜单空着，永不发 effort）。
    pub reasoning_efforts: Option<Vec<String>>,
    /// `[model.<id>].supports_images`. false = 纯文本模型，图片不往请求里塞。
    pub supports_images: Option<bool>,
    /// `[model.<id>.pricing]`。`None` = 没配单价，`/usage` 显示"未上报"而不是 $0。
    pub pricing: Option<ModelPricing>,
}

impl ModelChoice {
    pub fn has_http(&self) -> bool {
        self.api_base_url
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
            // 只在协议块里写基址的模型同样是「配了端点」的。
            || self.backend_overrides.values().any(|o| {
                o.api_base_url
                    .as_deref()
                    .is_some_and(|s| !s.trim().is_empty())
            })
            || self
                .api_key
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
            || self
                .env_key
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
    }

    pub fn resolved_api_key(&self) -> Option<String> {
        nonempty(self.api_key.clone()).or_else(|| {
            self.env_key
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .and_then(|name| std::env::var(name).ok())
                .and_then(|s| nonempty(Some(s)))
        })
    }

    /// 切到该模型时用哪条 wire：声明列表的第一条。
    pub fn default_backend(&self) -> ApiBackend {
        self.api_backends.first().copied().unwrap_or_default()
    }

    /// `/protocol` 只允许切到声明过的协议——端点没开的那条切过去就是 404。
    pub fn supports_backend(&self, backend: ApiBackend) -> bool {
        self.api_backends.contains(&backend)
    }

    /// 声明了不止一条才值得在 UI 上给切换入口。
    pub fn has_backend_choice(&self) -> bool {
        self.api_backends.len() > 1
    }

    fn override_for(&self, backend: ApiBackend) -> Option<&BackendOverride> {
        self.backend_overrides.get(&backend)
    }

    /// 这条 wire 的基址：协议级覆盖 > 模型级 `api_base_url` > None（交给 sampler
    /// 的兜底）。DeepSeek 的 /anthropic 入口就走这条路。
    pub fn base_url_for(&self, backend: ApiBackend) -> Option<&str> {
        self.override_for(backend)
            .and_then(|o| o.api_base_url.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.api_base_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
    }

    /// 协议级覆盖 > 模型级 `auth_scheme` > 按协议默认。
    pub fn resolved_auth(&self, backend: ApiBackend) -> AuthScheme {
        self.override_for(backend)
            .and_then(|o| o.auth_scheme)
            .or(self.auth_scheme)
            .unwrap_or_else(|| AuthScheme::default_for(backend))
    }

    /// 这条 wire 上发给上游的 slug：协议级覆盖 > 模型级 `api_model` > 目录 id。
    pub fn wire_model_for(&self, backend: ApiBackend) -> &str {
        self.override_for(backend)
            .and_then(|o| o.api_model.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.wire_model())
    }

    /// 只有 Messages 后端会用：其余两条靠上游自动前缀缓存，不需要断点。
    pub fn prompt_cache_enabled(&self) -> bool {
        self.prompt_cache.unwrap_or(true)
    }

    /// 没写就当支持——多模态是现在的常态，写死一份"哪些模型是纯文本"的名单
    /// 只会越追越旧。纯文本模型在 config 里显式标 `supports_images = false`。
    pub fn accepts_images(&self) -> bool {
        self.supports_images.unwrap_or(true)
    }

    /// `reasoning = false` 的模型连 `/think` 都不该影响它：一个推理字段都不发。
    pub fn supports_reasoning(&self) -> bool {
        self.reasoning.unwrap_or(true)
    }

    /// `/effort` 菜单该列哪几档。没配就给通用四档——这是给人选的菜单，不是
    /// 悄悄塞进请求体的值，列错了用户自己看得见；但模型真支持哪几档只有 config
    /// 知道，配了就以 config 为准。
    pub fn effort_choices(&self) -> Vec<String> {
        if !self.supports_reasoning() {
            return Vec::new();
        }
        let Some(configured) = self.reasoning_efforts.as_ref() else {
            return DEFAULT_EFFORT_CHOICES
                .iter()
                .map(|e| (*e).to_string())
                .collect();
        };
        // 显式写空列表 = 会推理但没有档位可选，菜单就该是空的。
        configured
            .iter()
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty())
            .collect()
    }

    /// 空 = 不发 effort，让上游用自己的默认。
    pub fn default_effort(&self) -> String {
        self.reasoning_effort
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_default()
            .to_string()
    }

    pub fn wire_model(&self) -> &str {
        self.api_model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(self.id.as_str())
    }
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    models: ModelsSection,
    #[serde(default)]
    model: BTreeMap<String, ModelOverride>,
    #[serde(default)]
    mcp: McpSection,
    #[serde(default)]
    mcp_servers: BTreeMap<String, McpServerRow>,
    /// Grok `[disabled_mcp_tools.<server>] = ["tool", …]` — raw MCP tool names.
    #[serde(default)]
    disabled_mcp_tools: BTreeMap<String, Vec<String>>,
    /// `[browser]` — BUA Chromium display prefs (headed vs headless).
    #[serde(default)]
    browser: BrowserSection,
    /// `[toolset.<tool>]` — per-tool runtime knobs owned by the tool plugins.
    #[serde(default)]
    toolset: ToolsetSection,
    /// `[memory]` — cross-session topics/observations memory (default off).
    #[serde(default)]
    memory: MemorySection,
}

/// `[toolset.web_fetch]` —— `tool-web` 的运行时旋钮。
///
/// 只收**真会生效**的键。Grok 的 `WebFetchParams` 还带 `cache_ttl_secs` /
/// `max_cache_entries` / `context_window_tokens`，但 dock 的 fetch 管道没有页面
/// 缓存、也没实现那个 3% 上下文帽——把死键摆进配置面只会让人以为自己配上了。
///
/// 故意**不加** `deny_unknown_fields`：`read_file` 是把整个 `FileConfig` 一把
/// `toml::from_str` 的，任何一处解析失败都会让**整份文件**（含 models）被静默
/// 丢弃。一个拼错的键不该有这种后果，改为在 `warn_unknown_web_fetch_keys` 里喊
/// 一声。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct WebFetchToolConfig {
    /// HTTP 请求总超时（秒）。默认 60；connect 恒 10s，不可配。
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// 响应体上限（字节）。默认 10 MB。**下完才判**，不是流式中断。
    #[serde(default)]
    pub max_content_length: Option<usize>,
    /// 喂给模型的 markdown 上限（字节）。默认 100 000，超了截断并追加 `[truncated]`。
    #[serde(default)]
    pub max_markdown_length: Option<usize>,
    /// 只允许抓这些域。默认不设 = 不额外限制（SSRF 仍在）。
    /// 注意 `[]`（显式空表）= **全部拒绝**，和省略不是一个意思。
    #[serde(default)]
    pub allowed_domains: Option<Vec<String>>,
    /// 转发代理。设上之后出口由代理解析，SSRF 的本地 DNS 预检随之失去依据
    /// （见 `cordis-spine` 的 `tools/web_fetch/ssrf.rs`）。
    #[serde(default)]
    pub proxy_endpoint: Option<String>,
    /// 只放行**显式**本地主机名（`localhost` / `127.0.0.0/8` / `::1`）。
    /// 私有段与云元数据地址永不放行。默认 false。
    #[serde(default)]
    pub allow_local: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct ToolsetSection {
    #[serde(default)]
    web_fetch: WebFetchToolConfig,
}

#[derive(Debug, Default, Deserialize)]
struct BrowserSection {
    /// Show a real Chromium window. Default false (headless / CI-safe).
    /// `None` = key absent (do not override earlier catalog paths).
    #[serde(default)]
    headed: Option<bool>,
}

/// Raw `[memory]` table from config.toml.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MemorySection {
    pub enabled: Option<bool>,
    pub flush: MemoryFlushSection,
    pub dream: MemoryDreamSection,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MemoryFlushSection {
    pub enabled: Option<bool>,
    pub soft_threshold_tokens: Option<u64>,
    pub flush_model: Option<String>,
    pub max_flush_write_chars: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MemoryDreamSection {
    pub enabled: Option<bool>,
    pub min_hours: Option<u64>,
    pub min_sessions: Option<u64>,
}

/// Resolved `[memory]` settings. Default `enabled = false`; `DOCK_MEMORY=1/0` overrides.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MemoryConfig {
    pub enabled: bool,
    /// Process-wide force off (`DOCK_MEMORY=0`).
    pub force_disabled: bool,
    pub flush: MemoryFlushConfig,
    pub dream: MemoryDreamConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryFlushConfig {
    pub enabled: bool,
    pub soft_threshold_tokens: u64,
    pub flush_model: Option<String>,
    pub max_flush_write_chars: usize,
}

impl Default for MemoryFlushConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            soft_threshold_tokens: 4000,
            flush_model: None,
            max_flush_write_chars: 8000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryDreamConfig {
    pub enabled: bool,
    pub min_hours: u64,
    pub min_sessions: u64,
}

impl Default for MemoryDreamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_hours: 24,
            min_sessions: 5,
        }
    }
}

impl MemoryConfig {
    /// Resolve from optional TOML section + `DOCK_MEMORY` env.
    pub fn resolve(section: &MemorySection) -> Self {
        let defaults = Self::default();
        let env = std::env::var("DOCK_MEMORY").ok();
        let (enabled, force_disabled) = match env.as_deref().map(str::trim) {
            Some("0") | Some("false") | Some("off") | Some("no") => (false, true),
            Some("1") | Some("true") | Some("on") | Some("yes") => (true, false),
            _ => (section.enabled.unwrap_or(false), false),
        };
        let flush_s = &section.flush;
        let dream_s = &section.dream;
        Self {
            enabled,
            force_disabled,
            flush: MemoryFlushConfig {
                enabled: flush_s.enabled.unwrap_or(defaults.flush.enabled),
                soft_threshold_tokens: flush_s
                    .soft_threshold_tokens
                    .unwrap_or(defaults.flush.soft_threshold_tokens),
                flush_model: match flush_s.flush_model.as_deref() {
                    Some("") | None => None,
                    Some(m) => Some(m.to_owned()),
                },
                max_flush_write_chars: flush_s
                    .max_flush_write_chars
                    .unwrap_or(defaults.flush.max_flush_write_chars),
            },
            dream: MemoryDreamConfig {
                enabled: dream_s.enabled.unwrap_or(defaults.dream.enabled),
                min_hours: dream_s.min_hours.unwrap_or(defaults.dream.min_hours),
                min_sessions: dream_s.min_sessions.unwrap_or(defaults.dream.min_sessions),
            },
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct McpSection {
    #[serde(default)]
    servers: Vec<McpServerRow>,
}

fn default_true() -> bool {
    true
}

/// Grok `[mcp_servers.<name>]` row: `command` → stdio, `url` → Streamable HTTP.
/// Untagged order copied: a nonempty `command` wins if both are set.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpServerRow {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default, alias = "urlTemplate", alias = "url_template")]
    pub url: String,
    #[serde(default, rename = "type")]
    #[allow(dead_code)]
    pub transport_type: Option<String>,
    #[serde(default)]
    pub bearer_token_env_var: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    #[serde(default)]
    pub oauth_client_secret_env_var: Option<String>,
    #[serde(default)]
    pub oauth_scopes: Option<Vec<String>>,
    #[serde(default)]
    pub oauth: Option<McpOAuthBlock>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub startup_timeout_sec: Option<u64>,
    /// Stdio JSON-RPC framing: `auto` (default), `content-length`, or `ndjson`.
    #[serde(default, alias = "stdio_framing", alias = "stdioFraming")]
    pub framing: Option<String>,
}

/// Grok `[mcp_servers.<name>.oauth]` / JSON `oauth` block (camelCase aliases).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpOAuthBlock {
    #[serde(default, alias = "clientId")]
    pub client_id: Option<String>,
    #[serde(default, alias = "clientSecretEnvVar")]
    pub client_secret_env_var: Option<String>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    #[serde(default, alias = "callbackPort")]
    pub callback_port: Option<u16>,
}

/// BYO OAuth client for an HTTP MCP server. Empty `client_id` still allows
/// Dynamic Client Registration when the authorization server advertises it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpOAuthConfig {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub callback_port: Option<u16>,
}

/// Grok default `initialize` / `tools/list` budget (`DEFAULT_STARTUP_TIMEOUT_SECS`).
pub const DEFAULT_MCP_STARTUP_TIMEOUT_SECS: u64 = 30;

/// How Dock frames JSON-RPC on MCP stdio (NDJSON vs LSP Content-Length).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpStdioFraming {
    /// Default: one JSON-RPC object per line (cua-driver and most modern stdio MCP).
    #[default]
    Ndjson,
    /// LSP-style `Content-Length` framing — only when explicitly set.
    ContentLength,
    /// Probe Content-Length on a fresh spawn; on JSON-RPC parse error (-32700), kill and respawn as NDJSON.
    Auto,
}

impl McpStdioFraming {
    /// Parse config aliases (`ndjson` / `jsonl`, `content-length` / `cl`, `auto`, …).
    /// Missing / empty → [`Self::Ndjson`].
    pub fn from_config(raw: Option<&str>) -> Self {
        let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::Ndjson;
        };
        match raw.to_ascii_lowercase().as_str() {
            "auto" => Self::Auto,
            "content-length" | "content_length" | "contentlength" | "cl" | "lsp" => {
                Self::ContentLength
            }
            "ndjson" | "newline" | "nl" | "jsonl" | "line" => Self::Ndjson,
            other => {
                tracing::warn!(framing = other, "unknown mcp stdio framing; using ndjson");
                Self::Ndjson
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        framing: McpStdioFraming,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub transport: McpTransport,
    pub startup_timeout_sec: u64,
    pub enabled: bool,
    pub oauth: McpOAuthConfig,
}

impl McpServer {
    pub fn endpoint(&self) -> &str {
        match &self.transport {
            McpTransport::Stdio { command, .. } => command,
            McpTransport::Http { url, .. } => url,
        }
    }
}

/// Live-read MCP servers from config. Fail-open: missing files → 只剩内置行。
/// Later files overlay the same name (Grok project `< user`)，配置文件里的同名行
/// 整条盖掉内置行。
pub fn load_mcp_servers() -> Vec<McpServer> {
    merge_builtin_mcp_servers(
        builtin_mcp_servers(),
        load_mcp_servers_from(&catalog_paths()),
    )
}

/// 代码里自带的 MCP 行：零配置就能用，优先级低于任何配置文件。
///
/// 目前只有 cua-driver，而且**发现得到二进制才注入** —— 没装就当没有这条，
/// `/mcps` 不会多出一条永远连不上的死行（computer 驾驶舱自己会说「未安装」）。
pub fn builtin_mcp_servers() -> Vec<McpServer> {
    crate::cua::discover()
        .map(|path| {
            vec![McpServer {
                name: crate::cua::CUA_DRIVER_SERVER.to_string(),
                transport: McpTransport::Stdio {
                    // 绝对路径：从 GUI 起的终端常常没有 `~/.local/bin`。
                    command: path.to_string_lossy().into_owned(),
                    args: vec!["mcp".to_string()],
                    env: BTreeMap::new(),
                    framing: McpStdioFraming::Ndjson,
                },
                startup_timeout_sec: DEFAULT_MCP_STARTUP_TIMEOUT_SECS,
                enabled: true,
                oauth: McpOAuthConfig::default(),
            }]
        })
        .unwrap_or_default()
}

/// 内置行在前，配置文件的同名行整条替换它（位置保持内置那条的次序）。
pub fn merge_builtin_mcp_servers(
    builtin: Vec<McpServer>,
    from_files: Vec<McpServer>,
) -> Vec<McpServer> {
    let mut rows: IndexMap<String, McpServer> = builtin
        .into_iter()
        .map(|server| (server.name.clone(), server))
        .collect();
    for server in from_files {
        rows.insert(server.name.clone(), server);
    }
    rows.into_values().collect()
}

pub fn load_mcp_servers_from(paths: &[PathBuf]) -> Vec<McpServer> {
    let mut rows: IndexMap<String, McpServerRow> = IndexMap::new();
    for path in paths {
        let Some(file) = read_file(path) else {
            continue;
        };
        for row in file.mcp.servers {
            let name = row_name(&row);
            if name.is_empty() {
                continue;
            }
            rows.insert(name, row);
        }
        for (key, row) in file.mcp_servers {
            if key.trim().is_empty() {
                continue;
            }
            rows.insert(key, row);
        }
    }
    rows.into_iter()
        .filter_map(|(name, row)| row_to_server(name, row))
        .collect()
}

fn row_name(row: &McpServerRow) -> String {
    row.name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let command = row.command.trim();
            if !command.is_empty() {
                command.to_string()
            } else {
                row.url.trim().to_string()
            }
        })
}

/// Grok `to_acp_mcp_server` + `blank_transport_field`, minus OAuth / SSE-as-separate-type.
/// Disabled servers stay in the list so `/mcps` can toggle them.
fn row_to_server(name: String, row: McpServerRow) -> Option<McpServer> {
    let startup_timeout_sec = row
        .startup_timeout_sec
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MCP_STARTUP_TIMEOUT_SECS);
    let command = row.command.trim();
    if !command.is_empty() {
        return Some(McpServer {
            name,
            transport: McpTransport::Stdio {
                command: command.to_string(),
                args: row.args,
                env: row.env,
                framing: McpStdioFraming::from_config(row.framing.as_deref()),
            },
            startup_timeout_sec,
            enabled: row.enabled,
            oauth: McpOAuthConfig::default(),
        });
    }
    let url = row.url.trim();
    if url.is_empty() {
        return None;
    }
    let oauth = row_oauth(&row);
    let mut headers = row.headers;
    if let Some(env_var) = row.bearer_token_env_var {
        match std::env::var(&env_var) {
            Ok(token) => {
                headers.insert("Authorization".into(), format!("Bearer {token}"));
            }
            Err(_) => {
                tracing::warn!(
                    server = name.as_str(),
                    env_var = env_var.as_str(),
                    "MCP server bearer_token_env_var not set; proceeding without it"
                );
            }
        }
    }
    Some(McpServer {
        name,
        transport: McpTransport::Http {
            url: url.to_string(),
            headers,
        },
        startup_timeout_sec,
        enabled: row.enabled,
        oauth,
    })
}

fn row_oauth(row: &McpServerRow) -> McpOAuthConfig {
    let from_block = row.oauth.as_ref();
    let client_id = nonempty(row.oauth_client_id.clone())
        .or_else(|| from_block.and_then(|b| nonempty(b.client_id.clone())));
    let secret_env = row
        .oauth_client_secret_env_var
        .as_ref()
        .or_else(|| from_block.and_then(|b| b.client_secret_env_var.as_ref()));
    let client_secret = secret_env
        .and_then(|name| std::env::var(name).ok())
        .and_then(|s| nonempty(Some(s)));
    let scopes = row
        .oauth_scopes
        .clone()
        .or_else(|| from_block.and_then(|b| b.scopes.clone()))
        .unwrap_or_default();
    let callback_port = from_block.and_then(|b| b.callback_port);
    McpOAuthConfig {
        client_id,
        client_secret,
        scopes,
        callback_port,
    }
}

/// Overlay `[disabled_mcp_tools]` from catalog files (later path wins per server).
pub fn load_disabled_mcp_tools() -> BTreeMap<String, Vec<String>> {
    load_disabled_mcp_tools_from(&catalog_paths())
}

pub fn load_disabled_mcp_tools_from(paths: &[PathBuf]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in paths {
        let Some(file) = read_file(path) else {
            continue;
        };
        for (server, tools) in file.disabled_mcp_tools {
            if server.trim().is_empty() {
                continue;
            }
            out.insert(server, tools);
        }
    }
    out
}

/// Persist `[mcp_servers.<name>].enabled` into the catalog file that defines it.
pub fn persist_mcp_server_enabled(name: &str, enabled: bool) -> Result<(), String> {
    persist_mcp_server_enabled_in(&catalog_paths(), name, enabled)
}

pub fn persist_mcp_server_enabled_in(
    paths: &[PathBuf],
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let path = match mcp_persist_target(paths, name) {
        Ok(path) => path,
        // 内置行在配置文件里没有实体：落一条完整的行到用户 config，否则
        // `/mcps` 上按 Space 只会得到「no [mcp_servers.cua-driver]」。
        Err(err) => {
            let Some(builtin) = builtin_mcp_servers().into_iter().find(|s| s.name == name) else {
                return Err(err);
            };
            return patch_toml(&mcp_persist_fallback(paths)?, |doc| {
                write_builtin_mcp_row(doc, &builtin, enabled)
            });
        }
    };
    patch_toml(&path, |doc| {
        let Some(item) = doc.get_mut("mcp_servers").and_then(|t| t.get_mut(name)) else {
            return Err(format!("config has no [mcp_servers.{name}]"));
        };
        item["enabled"] = toml_edit::value(enabled);
        Ok(())
    })
}

/// 把内置行按当前发现到的 command / args 写成实体行。写完它就归配置文件管，
/// 内置行不再参与（`merge_builtin_mcp_servers` 让文件整条覆盖）。
fn write_builtin_mcp_row(
    doc: &mut toml_edit::DocumentMut,
    server: &McpServer,
    enabled: bool,
) -> Result<(), String> {
    let McpTransport::Stdio { command, args, .. } = &server.transport else {
        return Err(format!("内置行 {} 不是 stdio", server.name));
    };
    if doc.get("mcp_servers").is_none() {
        let mut parent = toml_edit::Table::new();
        // 只作为 `[mcp_servers.<name>]` 的前缀出现，不单独打一行 `[mcp_servers]`。
        parent.set_implicit(true);
        doc["mcp_servers"] = toml_edit::Item::Table(parent);
    }
    let mut arr = toml_edit::Array::new();
    for arg in args {
        arr.push(arg.as_str());
    }
    let mut row = toml_edit::Table::new();
    row["command"] = toml_edit::value(command.as_str());
    row["args"] = toml_edit::value(arr);
    row["enabled"] = toml_edit::value(enabled);
    doc["mcp_servers"][&server.name] = toml_edit::Item::Table(row);
    Ok(())
}

/// Persist `[disabled_mcp_tools.<server>]` as an array of raw tool names.
pub fn persist_disabled_mcp_tools(server: &str, disabled: &[String]) -> Result<(), String> {
    persist_disabled_mcp_tools_in(&catalog_paths(), server, disabled)
}

pub fn persist_disabled_mcp_tools_in(
    paths: &[PathBuf],
    server: &str,
    disabled: &[String],
) -> Result<(), String> {
    let path = mcp_persist_target(paths, server).or_else(|_| mcp_persist_fallback(paths))?;
    patch_toml(&path, |doc| {
        if disabled.is_empty() {
            if let Some(table) = doc
                .get_mut("disabled_mcp_tools")
                .and_then(|t| t.as_table_like_mut())
            {
                table.remove(server);
                if table.is_empty() {
                    doc.remove("disabled_mcp_tools");
                }
            }
            return Ok(());
        }
        let mut arr = toml_edit::Array::new();
        for name in disabled {
            arr.push(name.as_str());
        }
        if doc.get("disabled_mcp_tools").is_none() {
            doc["disabled_mcp_tools"] = toml_edit::table();
        }
        doc["disabled_mcp_tools"][server] = toml_edit::value(arr);
        Ok(())
    })
}

/// Persisted `[browser].headed` preference (default false / headless).
/// Only files that actually set `browser.headed` contribute; later catalog paths win.
pub fn load_browser_headed() -> bool {
    load_browser_headed_from(&catalog_paths())
}

pub fn load_browser_headed_from(paths: &[PathBuf]) -> bool {
    let mut headed = false;
    for path in paths {
        let Some(file) = read_file(path) else {
            continue;
        };
        if let Some(val) = file.browser.headed {
            headed = val;
        }
    }
    headed
}

/// 认得的 `[toolset.web_fetch]` 键。和 [`WebFetchToolConfig`] 的字段一一对应。
const WEB_FETCH_TOOLSET_KEYS: &[&str] = &[
    "timeout_secs",
    "max_content_length",
    "max_markdown_length",
    "allowed_domains",
    "proxy_endpoint",
    "allow_local",
];

/// Live-read `[toolset.web_fetch]`. Later files overlay earlier ones **逐字段**
/// ——`~/.dock/config.toml` < 项目 `.dock/config.toml`，项目只写一个 `timeout_secs`
/// 不会把用户那份的 `proxy_endpoint` 一起清掉。整段表则不做合并：写了
/// `allowed_domains` 就是覆盖，不是追加。
pub fn load_web_fetch_config() -> WebFetchToolConfig {
    load_web_fetch_config_from(&catalog_paths())
}

/// Live-read `[memory]`. Default off; `DOCK_MEMORY` overrides TOML.
pub fn load_memory_config() -> MemoryConfig {
    load_memory_config_from(&catalog_paths())
}

pub fn load_memory_config_from(paths: &[PathBuf]) -> MemoryConfig {
    let mut section = MemorySection::default();
    for path in paths {
        let Some(file) = read_file(path) else {
            continue;
        };
        // Field-wise overlay: later files win per-option.
        if file.memory.enabled.is_some() {
            section.enabled = file.memory.enabled;
        }
        if file.memory.flush.enabled.is_some() {
            section.flush.enabled = file.memory.flush.enabled;
        }
        if file.memory.flush.soft_threshold_tokens.is_some() {
            section.flush.soft_threshold_tokens = file.memory.flush.soft_threshold_tokens;
        }
        if file.memory.flush.flush_model.is_some() {
            section.flush.flush_model = file.memory.flush.flush_model.clone();
        }
        if file.memory.flush.max_flush_write_chars.is_some() {
            section.flush.max_flush_write_chars = file.memory.flush.max_flush_write_chars;
        }
        if file.memory.dream.enabled.is_some() {
            section.dream.enabled = file.memory.dream.enabled;
        }
        if file.memory.dream.min_hours.is_some() {
            section.dream.min_hours = file.memory.dream.min_hours;
        }
        if file.memory.dream.min_sessions.is_some() {
            section.dream.min_sessions = file.memory.dream.min_sessions;
        }
    }
    MemoryConfig::resolve(&section)
}

pub fn load_web_fetch_config_from(paths: &[PathBuf]) -> WebFetchToolConfig {
    let mut out = WebFetchToolConfig::default();
    for path in paths {
        warn_unknown_web_fetch_keys(path);
        let Some(file) = read_file(path) else {
            continue;
        };
        let row = file.toolset.web_fetch;
        // 逐字段 overlay：`Some` 才盖，`None` 表示这个文件没表态。
        macro_rules! overlay {
            ($($f:ident),+ $(,)?) => { $( if row.$f.is_some() { out.$f = row.$f; } )+ };
        }
        overlay!(
            timeout_secs,
            max_content_length,
            max_markdown_length,
            allowed_domains,
            proxy_endpoint,
            allow_local,
        );
    }
    out
}

/// `[toolset.web_fetch]` 里认不出的键。
///
/// 结构体是宽容的（一个 typo 不能让整份 config.toml 被丢弃），所以这里显式喊
/// 一声——否则「配了没生效」会变成一次纯靠猜的排查。
fn warn_unknown_web_fetch_keys(path: &Path) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(doc) = raw.parse::<toml::Value>() else {
        return;
    };
    let Some(table) = doc
        .get("toolset")
        .and_then(|t| t.get("web_fetch"))
        .and_then(|t| t.as_table())
    else {
        return;
    };
    for key in table.keys() {
        if !WEB_FETCH_TOOLSET_KEYS.contains(&key.as_str()) {
            tracing::warn!(
                path = %path.display(),
                key = key.as_str(),
                "unknown [toolset.web_fetch] key; ignored"
            );
        }
    }
}

/// Write `[browser].headed` into user dock config (or an existing file that already has `[browser]`).
pub fn persist_browser_headed(headed: bool) -> Result<(), String> {
    persist_browser_headed_in(&catalog_paths(), headed)
}

pub fn persist_browser_headed_in(paths: &[PathBuf], headed: bool) -> Result<(), String> {
    let path = browser_persist_target(paths)?;
    patch_toml(&path, |doc| {
        if doc.get("browser").is_none() {
            doc["browser"] = toml_edit::table();
        }
        doc["browser"]["headed"] = toml_edit::value(headed);
        Ok(())
    })
}

/// True when `DOCK_BROWSER_HEADED` is set to a **non-empty** value (after trim).
/// Empty string / whitespace-only is treated as unset (use `[browser].headed` pref).
pub fn dock_browser_headed_env_override() -> bool {
    match std::env::var("DOCK_BROWSER_HEADED") {
        Ok(v) => !v.trim().is_empty(),
        Err(_) => false,
    }
}

/// Effective headed mode at Chromium launch: any non-empty `DOCK_BROWSER_HEADED` overrides to headed.
pub fn effective_browser_headed() -> bool {
    effective_browser_headed_with(dock_browser_headed_env_override(), load_browser_headed())
}

pub fn effective_browser_headed_with(env_override: bool, pref: bool) -> bool {
    env_override || pref
}

fn browser_persist_target(paths: &[PathBuf]) -> Result<PathBuf, String> {
    for path in paths.iter().rev() {
        if file_has_browser_section(path) {
            return Ok(path.clone());
        }
    }
    for path in paths.iter().rev() {
        if path.exists() {
            return Ok(path.clone());
        }
    }
    Ok(dock_home().join("config.toml"))
}

fn file_has_browser_section(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return false;
    };
    doc.get("browser").is_some()
}

fn mcp_persist_target(paths: &[PathBuf], name: &str) -> Result<PathBuf, String> {
    for path in paths.iter().rev() {
        if file_defines_mcp_server(path, name) {
            return Ok(path.clone());
        }
    }
    Err(format!("no [mcp_servers.{name}] in catalog files"))
}

fn mcp_persist_fallback(paths: &[PathBuf]) -> Result<PathBuf, String> {
    paths
        .iter()
        .rev()
        .find(|p| p.exists() || p.parent().is_some_and(|d| d.exists()))
        .cloned()
        .ok_or_else(|| "no MCP config file to write".into())
}

fn file_defines_mcp_server(path: &Path, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return false;
    };
    doc.get("mcp_servers").and_then(|t| t.get(name)).is_some()
}

fn patch_toml(
    path: &Path,
    f: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<(), String>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc = if text.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        text.parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("parse {}: {e}", path.display()))?
    };
    f(&mut doc)?;
    std::fs::write(path, doc.to_string()).map_err(|e| e.to_string())
}

#[derive(Debug, Default, Deserialize)]
struct ModelsSection {
    default: Option<String>,
    #[serde(default)]
    catalog: Vec<CatalogRow>,
}

#[derive(Debug, Deserialize)]
struct CatalogRow {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, alias = "api_base")]
    api_base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    env_key: Option<String>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    api_backend: Option<String>,
    #[serde(default)]
    api_backends: Option<Vec<String>>,
    #[serde(default)]
    auth_scheme: Option<String>,
    #[serde(default)]
    api_model: Option<String>,
    #[serde(flatten)]
    backends: BackendTables,
    #[serde(default)]
    prompt_cache: Option<bool>,
    #[serde(default)]
    max_output_tokens: Option<u32>,
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    reasoning_efforts: Option<Vec<String>>,
    #[serde(default)]
    supports_images: Option<bool>,
    #[serde(default)]
    pricing: Option<PricingRow>,
}

/// `[model.<id>.responses]` / `.chat_completions` / `.messages` 三个可选子表。
///
/// 用固定字段而不是 `BTreeMap<String, _>`：协议名写错时 serde 会当未知键丢掉，
/// 而 map 会悄悄收下一个永远匹配不上的条目。别名跟 `ApiBackend::from_name` 对齐。
#[derive(Debug, Default, Deserialize)]
struct BackendTables {
    #[serde(default, alias = "resp")]
    responses: Option<BackendRow>,
    #[serde(default, alias = "chat-completions", alias = "chat")]
    chat_completions: Option<BackendRow>,
    #[serde(default, alias = "anthropic")]
    messages: Option<BackendRow>,
}

#[derive(Debug, Default, Deserialize)]
struct BackendRow {
    #[serde(default, alias = "api_base")]
    api_base_url: Option<String>,
    #[serde(default)]
    auth_scheme: Option<String>,
    #[serde(default)]
    api_model: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ModelOverride {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, alias = "api_base")]
    api_base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    env_key: Option<String>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    api_backend: Option<String>,
    #[serde(default)]
    api_backends: Option<Vec<String>>,
    #[serde(default)]
    auth_scheme: Option<String>,
    #[serde(default)]
    api_model: Option<String>,
    #[serde(flatten)]
    backends: BackendTables,
    #[serde(default)]
    prompt_cache: Option<bool>,
    #[serde(default)]
    max_output_tokens: Option<u32>,
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    reasoning_efforts: Option<Vec<String>>,
    #[serde(default)]
    supports_images: Option<bool>,
    #[serde(default)]
    pricing: Option<PricingRow>,
}

/// `[model.<id>.pricing]`，单位 USD / 百万 token。
#[derive(Debug, Default, Deserialize)]
struct PricingRow {
    #[serde(default)]
    input: Option<f64>,
    #[serde(default)]
    cache_read: Option<f64>,
    #[serde(default)]
    cache_write: Option<f64>,
    #[serde(default)]
    output: Option<f64>,
}

impl PricingRow {
    /// 全 0（没写任何一个价）返回 `None`：`Some(0)` 会被下游当成"这个模型免费"。
    fn parse(&self) -> Option<ModelPricing> {
        let input = usd_per_mtok_to_ticks(self.input);
        let pricing = ModelPricing {
            input_ticks_per_mtok: input,
            cache_read_ticks_per_mtok: usd_per_mtok_to_ticks(self.cache_read),
            // 省略 = 按原价。多数厂商写入不额外加价；Anthropic 那种 1.25x 要显式写。
            cache_write_ticks_per_mtok: match usd_per_mtok_to_ticks(self.cache_write) {
                0 => input,
                v => v,
            },
            output_ticks_per_mtok: usd_per_mtok_to_ticks(self.output),
        };
        (!pricing.is_zero()).then_some(pricing)
    }
}

pub fn dock_home() -> PathBuf {
    if let Ok(p) = std::env::var("DOCK_HOME") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dock")
}

pub fn catalog_paths() -> Vec<PathBuf> {
    vec![
        dock_home().join("config.toml"),
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".dock")
            .join("config.toml"),
    ]
}

/// Live-read catalog. Do not cache the Vec on a long-lived Arc.
pub fn load_catalog() -> Vec<ModelChoice> {
    load_catalog_from(&catalog_paths())
}

pub fn load_default_model() -> Option<String> {
    load_default_model_from(&catalog_paths())
}

pub fn lookup_model(id: &str) -> Option<ModelChoice> {
    load_catalog().into_iter().find(|m| m.id == id)
}

pub fn catalog_has_http() -> bool {
    load_catalog().iter().any(ModelChoice::has_http)
}

/// 目录只来自 config：没有 `[[models.catalog]]` / `[model.<id>]` 就是空的。
/// 以前这里塞过五条没有 api_base 也没有 key 的内置条目（grok-4 / gpt-4.1 / …），
/// 选中它们只会得到一个连不上的模型——宁可空着，让 `/model` 直说去写 config。
pub fn load_catalog_from(paths: &[PathBuf]) -> Vec<ModelChoice> {
    let mut list: Vec<ModelChoice> = Vec::new();
    for path in paths {
        let Some(file) = read_file(path) else {
            continue;
        };
        if !file.models.catalog.is_empty() {
            list = file.models.catalog.iter().map(choice_from_row).collect();
        }
        merge_overrides(&mut list, &file.model);
    }
    list
}

pub fn load_default_model_from(paths: &[PathBuf]) -> Option<String> {
    let mut found = None;
    for path in paths {
        if let Some(file) = read_file(path) {
            if let Some(d) = file.models.default {
                if !d.trim().is_empty() {
                    found = Some(d);
                }
            }
        }
    }
    found
}

fn read_file(path: &Path) -> Option<FileConfig> {
    let raw = std::fs::read_to_string(path).ok()?;
    toml::from_str(&raw).ok()
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

/// `api_backends`（列表，第一条 = 默认）+ 单数 `api_backend`（当默认讲）。
///
/// 两个都写时单数提到队首：这样在老配置上加一行 `api_backends` 不会把原来的
/// 默认协议换掉。都没写就是 `[ApiBackend::default()]`——返回值永不为空。
fn parse_backends(list: Option<&Vec<String>>, single: Option<&str>) -> Vec<ApiBackend> {
    let mut out: Vec<ApiBackend> = Vec::new();
    let mut push = |backend: ApiBackend| {
        if !out.contains(&backend) {
            out.push(backend);
        }
    };
    if let Some(single) = single.map(str::trim).filter(|s| !s.is_empty()) {
        push(ApiBackend::parse(Some(single)));
    }
    for raw in list.into_iter().flatten() {
        match ApiBackend::from_name(raw) {
            Some(backend) => push(backend),
            None => tracing::warn!(
                api_backend = raw.as_str(),
                "unknown api_backends entry; skipped"
            ),
        }
    }
    if out.is_empty() {
        out.push(ApiBackend::default());
    }
    out
}

impl BackendTables {
    fn is_empty(&self) -> bool {
        self.responses.is_none() && self.chat_completions.is_none() && self.messages.is_none()
    }

    /// 三个子表 → `ApiBackend` 索引的覆盖表。整块空着的子表不收，免得
    /// `backend_overrides` 里躺着一堆什么都不覆盖的条目。
    fn parse(&self) -> BTreeMap<ApiBackend, BackendOverride> {
        let rows = [
            (ApiBackend::Responses, self.responses.as_ref()),
            (ApiBackend::ChatCompletions, self.chat_completions.as_ref()),
            (ApiBackend::Messages, self.messages.as_ref()),
        ];
        rows.into_iter()
            .filter_map(|(backend, row)| {
                let row = row?;
                let over = BackendOverride {
                    api_base_url: nonempty(row.api_base_url.clone()),
                    auth_scheme: AuthScheme::parse(row.auth_scheme.as_deref()),
                    api_model: nonempty(row.api_model.clone()),
                };
                (over != BackendOverride::default()).then_some((backend, over))
            })
            .collect()
    }
}

fn choice_from_row(row: &CatalogRow) -> ModelChoice {
    ModelChoice {
        id: row.id.clone(),
        name: row.name.clone().unwrap_or_else(|| row.id.clone()),
        description: row.description.clone().unwrap_or_default(),
        api_base_url: nonempty(row.api_base_url.clone()),
        api_key: nonempty(row.api_key.clone()),
        env_key: nonempty(row.env_key.clone()),
        context_window: row.context_window.filter(|n| *n > 0),
        api_backends: parse_backends(row.api_backends.as_ref(), row.api_backend.as_deref()),
        backend_overrides: row.backends.parse(),
        auth_scheme: AuthScheme::parse(row.auth_scheme.as_deref()),
        api_model: nonempty(row.api_model.clone()),
        prompt_cache: row.prompt_cache,
        max_output_tokens: row.max_output_tokens.filter(|n| *n > 0),
        reasoning: row.reasoning,
        reasoning_effort: nonempty(row.reasoning_effort.clone()),
        reasoning_efforts: row.reasoning_efforts.clone(),
        supports_images: row.supports_images,
        pricing: row.pricing.as_ref().and_then(PricingRow::parse),
    }
}

fn merge_overrides(list: &mut Vec<ModelChoice>, overrides: &BTreeMap<String, ModelOverride>) {
    for (key, ov) in overrides {
        let id = ov.model.clone().unwrap_or_else(|| key.clone());
        if let Some(existing) = list.iter_mut().find(|m| m.id == id || m.id == *key) {
            if let Some(name) = &ov.name {
                existing.name = name.clone();
            }
            if let Some(description) = &ov.description {
                existing.description = description.clone();
            }
            if ov.api_base_url.is_some() {
                existing.api_base_url = nonempty(ov.api_base_url.clone());
            }
            if ov.api_key.is_some() {
                existing.api_key = nonempty(ov.api_key.clone());
            }
            if ov.env_key.is_some() {
                existing.env_key = nonempty(ov.env_key.clone());
            }
            if ov.context_window.is_some() {
                existing.context_window = ov.context_window.filter(|n| *n > 0);
            }
            if ov.api_backend.is_some() || ov.api_backends.is_some() {
                existing.api_backends =
                    parse_backends(ov.api_backends.as_ref(), ov.api_backend.as_deref());
            }
            // 整块替换而不是逐协议合并：写了协议块就是在重新声明这个模型的连接
            // 方式，半新半旧地叠上去只会得到谁也说不清的基址。
            if !ov.backends.is_empty() {
                existing.backend_overrides = ov.backends.parse();
            }
            if ov.auth_scheme.is_some() {
                existing.auth_scheme = AuthScheme::parse(ov.auth_scheme.as_deref());
            }
            if ov.api_model.is_some() {
                existing.api_model = nonempty(ov.api_model.clone());
            }
            if ov.pricing.is_some() {
                existing.pricing = ov.pricing.as_ref().and_then(PricingRow::parse);
            }
            existing.id = id;
        } else {
            list.push(ModelChoice {
                name: ov.name.clone().unwrap_or_else(|| id.clone()),
                description: ov.description.clone().unwrap_or_default(),
                api_base_url: nonempty(ov.api_base_url.clone()),
                api_key: nonempty(ov.api_key.clone()),
                env_key: nonempty(ov.env_key.clone()),
                context_window: ov.context_window.filter(|n| *n > 0),
                api_backends: parse_backends(ov.api_backends.as_ref(), ov.api_backend.as_deref()),
                backend_overrides: ov.backends.parse(),
                auth_scheme: AuthScheme::parse(ov.auth_scheme.as_deref()),
                api_model: nonempty(ov.api_model.clone()),
                prompt_cache: ov.prompt_cache,
                max_output_tokens: ov.max_output_tokens.filter(|n| *n > 0),
                reasoning: ov.reasoning,
                reasoning_effort: nonempty(ov.reasoning_effort.clone()),
                reasoning_efforts: ov.reasoning_efforts.clone(),
                supports_images: ov.supports_images,
                pricing: ov.pricing.as_ref().and_then(PricingRow::parse),
                id,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_array_replaces_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[models]
default = "local-llm"

[[models.catalog]]
id = "local-llm"
name = "Local"
description = "ollama"
"#,
        )
        .unwrap();
        let list = load_catalog_from(std::slice::from_ref(&path));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "local-llm");
        assert_eq!(
            load_default_model_from(&[path]).as_deref(),
            Some("local-llm")
        );
    }

    #[test]
    fn model_table_adds_and_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model.grok-4]
name = "Grok 4 pinned"

[model.mine]
model = "mine"
description = "custom"
"#,
        )
        .unwrap();
        let list = load_catalog_from(&[path]);
        assert!(list
            .iter()
            .any(|m| m.id == "grok-4" && m.name == "Grok 4 pinned"));
        assert!(list
            .iter()
            .any(|m| m.id == "mine" && m.description == "custom"));
    }

    #[test]
    fn later_file_wins() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = dir.path().join("project.toml");
        std::fs::write(
            &user,
            r#"
[models]
default = "user-model"
[[models.catalog]]
id = "user-model"
name = "User"
"#,
        )
        .unwrap();
        std::fs::write(
            &project,
            r#"
[models]
default = "project-model"
[[models.catalog]]
id = "project-model"
name = "Project"
"#,
        )
        .unwrap();
        let list = load_catalog_from(&[user.clone(), project.clone()]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "project-model");
        assert_eq!(
            load_default_model_from(&[user, project]).as_deref(),
            Some("project-model")
        );
    }

    #[test]
    fn quoted_id_carries_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model."glm-5.3-flash"]
name = "GLM 5.3 Flash"
description = "Empero free"
api_base_url = "https://free.empero.org/v1"
api_key = "free"

[model."qwen3.8-flash"]
name = "Qwen3.8 Flash-Next"
api_base = "https://free.empero.org/v1"
api_key = "free"
"#,
        )
        .unwrap();
        let list = load_catalog_from(&[path]);
        let glm = list.iter().find(|m| m.id == "glm-5.3-flash").unwrap();
        assert_eq!(
            glm.api_base_url.as_deref(),
            Some("https://free.empero.org/v1")
        );
        assert_eq!(glm.api_key.as_deref(), Some("free"));
        let qwen = list.iter().find(|m| m.id == "qwen3.8-flash").unwrap();
        assert_eq!(
            qwen.api_base_url.as_deref(),
            Some("https://free.empero.org/v1")
        );
        assert!(glm.has_http());
    }

    #[test]
    fn api_backend_and_auth_scheme_from_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model.claude]
api_base_url = "https://api.anthropic.com/v1"
api_backend = "messages"
env_key = "ANTHROPIC_API_KEY"

[model.gpt]
api_base_url = "https://api.openai.com/v1"
api_backend = "responses"
auth_scheme = "bearer"
"#,
        )
        .unwrap();
        let list = load_catalog_from(&[path]);
        let claude = list.iter().find(|m| m.id == "claude").unwrap();
        assert_eq!(claude.api_backends, vec![ApiBackend::Messages]);
        assert_eq!(
            claude.resolved_auth(claude.default_backend()),
            AuthScheme::XApiKey
        );
        let gpt = list.iter().find(|m| m.id == "gpt").unwrap();
        assert_eq!(gpt.api_backends, vec![ApiBackend::Responses]);
        assert_eq!(gpt.resolved_auth(gpt.default_backend()), AuthScheme::Bearer);
        assert_eq!(ApiBackend::Messages.path(), "messages");
        assert_eq!(ApiBackend::Responses.path(), "responses");
        assert_eq!(ApiBackend::ChatCompletions.path(), "chat/completions");
    }

    /// 一个端点同时开了 /responses 和 /chat/completions：写**一条** `[model.<id>]`
    /// 就够，不用为了换协议再配一个只有 api_backend 不同的重复条目。
    #[test]
    fn one_endpoint_declares_several_backends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model.dual]
api_base_url = "https://example.test/v1"
api_backends = ["responses", "chat_completions"]
env_key = "K"
"#,
        )
        .unwrap();
        let m = &load_catalog_from(&[path])[0];
        assert_eq!(
            m.api_backends,
            vec![ApiBackend::Responses, ApiBackend::ChatCompletions]
        );
        assert_eq!(m.default_backend(), ApiBackend::Responses, "第一条 = 默认");
        assert!(m.supports_backend(ApiBackend::ChatCompletions));
        assert!(!m.supports_backend(ApiBackend::Messages), "没声明就不能切");
        assert!(m.has_backend_choice());
        // auth 跟着**当前这条 wire** 走，不是模型的固定属性。
        assert_eq!(m.resolved_auth(ApiBackend::Responses), AuthScheme::Bearer);
        assert_eq!(m.resolved_auth(ApiBackend::Messages), AuthScheme::XApiKey);
    }

    /// 一个端点支持所有协议，但各协议的入口不同——DeepSeek 的 OpenAI 侧是
    /// `https://api.deepseek.com`，Anthropic 侧是 `…/anthropic`。协议块只覆盖
    /// 连接，能力（context_window / reasoning / …）仍然是整条模型共用的。
    ///
    /// 同时守着 `#[serde(flatten)]` + toml 的坑：flatten 会把整张表先 buffer 成
    /// `Content` 再分发，数字 / 布尔 / 数组这些非字符串键最容易在这一步丢掉，
    /// 所以这里每种都放了一个。
    #[test]
    fn per_protocol_base_url_and_slug() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model."deepseek-flash"]
api_base_url = "https://api.deepseek.com"
api_backends = ["responses", "chat_completions", "messages"]
env_key = "DEEPSEEK_API_KEY"
context_window = 264800
max_output_tokens = 8192
reasoning = true
reasoning_efforts = ["low", "high", "max"]
supports_images = true

[model."deepseek-flash".messages]
api_base_url = "https://api.deepseek.com/anthropic"
auth_scheme = "bearer"
api_model = "deepseek-chat"
"#,
        )
        .unwrap();
        let m = &load_catalog_from(&[path])[0];

        // 非字符串键必须活过 flatten。
        assert_eq!(m.context_window, Some(264_800));
        assert_eq!(m.max_output_tokens, Some(8192));
        assert_eq!(m.reasoning, Some(true));
        assert_eq!(m.supports_images, Some(true));
        assert_eq!(m.effort_choices(), vec!["low", "high", "max"]);
        assert_eq!(
            m.api_backends,
            vec![
                ApiBackend::Responses,
                ApiBackend::ChatCompletions,
                ApiBackend::Messages
            ]
        );

        // 没写协议块的两条走模型级基址与 slug。
        for backend in [ApiBackend::Responses, ApiBackend::ChatCompletions] {
            assert_eq!(m.base_url_for(backend), Some("https://api.deepseek.com"));
            assert_eq!(m.wire_model_for(backend), "deepseek-flash");
            assert_eq!(m.resolved_auth(backend), AuthScheme::Bearer);
        }
        // messages 三项全被协议块接管（auth 显式 bearer，压过协议默认的 x-api-key）。
        assert_eq!(
            m.base_url_for(ApiBackend::Messages),
            Some("https://api.deepseek.com/anthropic")
        );
        assert_eq!(m.wire_model_for(ApiBackend::Messages), "deepseek-chat");
        assert_eq!(m.resolved_auth(ApiBackend::Messages), AuthScheme::Bearer);
    }

    /// 只在协议块里写基址也算「配了端点」，否则 `/model` 会把它当成没端点的行。
    #[test]
    fn a_protocol_only_base_url_still_counts_as_configured() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model.only-anthropic]
api_backend = "messages"

[model.only-anthropic.messages]
api_base_url = "https://example.test/anthropic"
"#,
        )
        .unwrap();
        let m = &load_catalog_from(&[path])[0];
        assert!(m.has_http());
        assert_eq!(
            m.base_url_for(ApiBackend::Messages),
            Some("https://example.test/anthropic")
        );
        // 没写模型级 api_base_url，别的协议就没有基址可用（会退到 sampler 兜底）。
        assert_eq!(m.base_url_for(ApiBackend::Responses), None);
    }

    /// 协议名写对了别名也认；空协议块不进 `backend_overrides`。
    #[test]
    fn protocol_table_aliases_and_empty_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model.m]
api_base_url = "https://example.test/v1"

[model.m.anthropic]
api_base_url = "https://example.test/anthropic"

[model.m.chat]
"#,
        )
        .unwrap();
        let m = &load_catalog_from(&[path])[0];
        assert_eq!(
            m.base_url_for(ApiBackend::Messages),
            Some("https://example.test/anthropic"),
            "anthropic 是 messages 的别名"
        );
        assert_eq!(
            m.backend_overrides.len(),
            1,
            "什么都不覆盖的空块不该占一条：{:?}",
            m.backend_overrides
        );
    }

    /// 单价按段计，口径和 `/usage` 的三段完全一致。省略 `cache_write` 回落到
    /// `input`——多数厂商写入就是原价。
    #[test]
    fn pricing_is_per_segment_and_cache_write_falls_back_to_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model.priced]
api_base_url = "https://example.test/v1"

[model.priced.pricing]
input = 0.28
cache_read = 0.028
output = 0.42
"#,
        )
        .unwrap();
        let p = load_catalog_from(&[path])[0].pricing.unwrap();
        assert_eq!(p.input_ticks_per_mtok, 2_800_000_000);
        assert_eq!(p.cache_read_ticks_per_mtok, 280_000_000);
        assert_eq!(
            p.cache_write_ticks_per_mtok, p.input_ticks_per_mtok,
            "没写 cache_write 就按原价，不是 0"
        );

        // 1M 未命中 + 1M 命中 + 1M 输出 = 0.28 + 0.028 + 0.42 = $0.728
        let ticks = p.cost_ticks(1_000_000, 1_000_000, 0, 1_000_000);
        assert_eq!(ticks, 7_280_000_000);
        assert!((crate::usage::ticks_to_usd(ticks) - 0.728).abs() < 1e-9);
    }

    /// 没写 `[pricing]`、或写了但全是 0 / 负数：`None`，不能拿 $0 冒充免费。
    #[test]
    fn missing_or_zero_pricing_is_none_not_free() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model.bare]
api_base_url = "https://example.test/v1"

[model.zeroed]
api_base_url = "https://example.test/v1"

[model.zeroed.pricing]
input = 0
output = -1
"#,
        )
        .unwrap();
        let list = load_catalog_from(&[path]);
        for m in &list {
            assert_eq!(m.pricing, None, "{} 不该有单价", m.id);
        }
    }

    /// 大用量不能溢出：中间用 i128 转，1M token × 高单价仍要算对。
    #[test]
    fn pricing_survives_large_token_counts() {
        let p = ModelPricing {
            input_ticks_per_mtok: 1_500_000_000_000, // $150 / Mtok
            cache_read_ticks_per_mtok: 0,
            cache_write_ticks_per_mtok: 0,
            output_ticks_per_mtok: 0,
        };
        let ticks = p.cost_ticks(10_000_000, 0, 0, 0);
        assert_eq!(ticks, 15_000_000_000_000);
        assert!((crate::usage::ticks_to_usd(ticks) - 1_500.0).abs() < 1e-6);
    }

    /// 老配置只写单数 `api_backend`：等价于只声明这一条，没有可切的余地。
    #[test]
    fn singular_api_backend_is_a_one_element_declaration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model.legacy]
api_base_url = "https://example.test/v1"
api_backend = "chat_completions"
"#,
        )
        .unwrap();
        let m = &load_catalog_from(&[path])[0];
        assert_eq!(m.api_backends, vec![ApiBackend::ChatCompletions]);
        assert!(!m.has_backend_choice());
    }

    /// 两个键都写：单数当默认提到队首，这样在老配置上补一行 `api_backends`
    /// 不会把原来的默认协议换掉。不认识的项跳过，不会变成一条重复的默认值。
    #[test]
    fn singular_leads_and_unknown_entries_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model.mixed]
api_base_url = "https://example.test/v1"
api_backend = "chat_completions"
api_backends = ["responses", "chat_completions", "grpc"]
"#,
        )
        .unwrap();
        let m = &load_catalog_from(&[path])[0];
        assert_eq!(
            m.api_backends,
            vec![ApiBackend::ChatCompletions, ApiBackend::Responses],
            "单数在前、去重、'grpc' 丢掉而不是退化成一条默认值"
        );
    }

    /// 什么都不写就是 Responses：coding agent 的默认该是能回放推理链的那条。
    #[test]
    fn omitting_the_key_defaults_to_responses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model.bare]
api_base_url = "https://example.test/v1"
"#,
        )
        .unwrap();
        let m = &load_catalog_from(&[path])[0];
        assert_eq!(m.api_backends, vec![ApiBackend::Responses]);
        assert_eq!(ApiBackend::default(), ApiBackend::Responses);
        assert_eq!(ApiBackend::parse(None), ApiBackend::Responses);
    }

    /// `[[models.catalog]]` 的行也能被 `[model.<id>]` 改协议声明。
    #[test]
    fn override_replaces_the_whole_backend_declaration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[[models.catalog]]
id = "m"
api_backend = "messages"

[model.m]
api_backends = ["responses", "messages"]
"#,
        )
        .unwrap();
        let m = &load_catalog_from(&[path])[0];
        assert_eq!(
            m.api_backends,
            vec![ApiBackend::Responses, ApiBackend::Messages]
        );
    }

    #[test]
    fn api_model_is_wire_slug_picker_keeps_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[model."minimax-m3-responses"]
name = "MiniMax M3 responses"
api_base_url = "https://openrouter.ai/api/v1"
api_backend = "responses"
api_model = "minimax/minimax-m3:free"
auth_scheme = "bearer"
"#,
        )
        .unwrap();
        let list = load_catalog_from(&[path]);
        let m = list
            .iter()
            .find(|m| m.id == "minimax-m3-responses")
            .unwrap();
        assert_eq!(m.wire_model(), "minimax/minimax-m3:free");
        assert_eq!(m.api_backends, vec![ApiBackend::Responses]);
        assert_eq!(m.resolved_auth(m.default_backend()), AuthScheme::Bearer);
    }

    #[test]
    fn openrouter_id_and_env_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[models]
default = "minimax/minimax-m3:free"

[model."minimax/minimax-m3:free"]
name = "MiniMax M3"
api_base_url = "https://openrouter.ai/api/v1"
env_key = "OPENROUTER_API_KEY"
"#,
        )
        .unwrap();
        let list = load_catalog_from(std::slice::from_ref(&path));
        let m = list
            .iter()
            .find(|m| m.id == "minimax/minimax-m3:free")
            .unwrap();
        assert_eq!(
            m.api_base_url.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(m.env_key.as_deref(), Some("OPENROUTER_API_KEY"));
        assert_eq!(
            load_default_model_from(&[path]).as_deref(),
            Some("minimax/minimax-m3:free")
        );
    }

    #[test]
    fn mcp_url_only_is_http() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.local]
url = "http://127.0.0.1:18989/mcp"
"#,
        )
        .unwrap();
        let list = load_mcp_servers_from(&[path]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "local");
        assert_eq!(list[0].endpoint(), "http://127.0.0.1:18989/mcp");
        assert!(matches!(list[0].transport, McpTransport::Http { .. }));
    }

    #[test]
    fn mcp_blank_url_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.empty]
url = ""
"#,
        )
        .unwrap();
        assert!(load_mcp_servers_from(&[path]).is_empty());
    }

    #[test]
    fn mcp_stdio_still_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.fs]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
"#,
        )
        .unwrap();
        let list = load_mcp_servers_from(&[path]);
        assert_eq!(list.len(), 1);
        match &list[0].transport {
            McpTransport::Stdio { command, args, .. } => {
                assert_eq!(command, "npx");
                assert_eq!(args.len(), 3);
            }
            other => panic!("expected stdio, got {other:?}"),
        }
    }

    #[test]
    fn mcp_stdio_framing_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.cua]
command = "cua-driver"
args = ["mcp"]
framing = "ndjson"
"#,
        )
        .unwrap();
        let list = load_mcp_servers_from(&[path]);
        match &list[0].transport {
            McpTransport::Stdio { framing, .. } => {
                assert_eq!(*framing, McpStdioFraming::Ndjson);
            }
            other => panic!("expected stdio, got {other:?}"),
        }
        assert_eq!(
            McpStdioFraming::from_config(Some("cl")),
            McpStdioFraming::ContentLength
        );
        assert_eq!(
            McpStdioFraming::from_config(Some("auto")),
            McpStdioFraming::Auto
        );
        assert_eq!(McpStdioFraming::from_config(None), McpStdioFraming::Ndjson);
    }

    /// 内置行只是「零配置默认」：配置文件里写了同名行就整条归它，连 command
    /// 都不能留着内置那份，否则用户改成自己的绝对路径会被悄悄忽略。
    #[test]
    fn config_file_row_replaces_builtin_row() {
        let builtin = vec![McpServer {
            name: "cua-driver".into(),
            transport: McpTransport::Stdio {
                command: "/builtin/cua-driver".into(),
                args: vec!["mcp".into()],
                env: BTreeMap::new(),
                framing: McpStdioFraming::Ndjson,
            },
            startup_timeout_sec: DEFAULT_MCP_STARTUP_TIMEOUT_SECS,
            enabled: true,
            oauth: McpOAuthConfig::default(),
        }];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.cua-driver]
command = "/opt/mine/cua-driver"
args = ["mcp"]
enabled = false

[mcp_servers.other]
command = "npx"
"#,
        )
        .unwrap();
        let merged = merge_builtin_mcp_servers(builtin, load_mcp_servers_from(&[path]));
        assert_eq!(merged.len(), 2);
        let cua = merged.iter().find(|s| s.name == "cua-driver").unwrap();
        assert!(!cua.enabled);
        assert_eq!(cua.endpoint(), "/opt/mine/cua-driver");
    }

    /// `/mcps` 里给内置行按 Space：文件里没有实体行，要落一条完整的，
    /// 不能只报「config has no [mcp_servers.…]」。
    #[test]
    fn toggling_builtin_row_writes_a_full_row() {
        let _env = crate::test_env::scoped().remove(crate::cua::DRIVER_ENV);
        let Some(driver) = crate::cua::discover() else {
            // 本机没装 driver 就没有内置行可落，跳过（CI 常态）。
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        persist_mcp_server_enabled_in(std::slice::from_ref(&path), "cua-driver", false).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[mcp_servers.cua-driver]"), "{text}");
        assert!(text.contains("enabled = false"), "{text}");
        assert!(
            text.contains(&driver.to_string_lossy().into_owned()),
            "{text}"
        );

        // 落地之后就走普通路径，再切回来只改 enabled。
        persist_mcp_server_enabled_in(std::slice::from_ref(&path), "cua-driver", true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("enabled = true"), "{text}");
        assert_eq!(
            text.matches("[mcp_servers.cua-driver]").count(),
            1,
            "{text}"
        );
    }

    #[test]
    fn unknown_server_without_a_row_still_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        assert!(persist_mcp_server_enabled_in(&[path], "nope", false).is_err());
    }

    #[test]
    fn mcp_command_wins_over_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.both]
command = "npx"
url = "http://127.0.0.1:18989/mcp"
"#,
        )
        .unwrap();
        let list = load_mcp_servers_from(&[path]);
        assert!(matches!(list[0].transport, McpTransport::Stdio { .. }));
    }

    #[test]
    fn mcp_later_file_overlays_name() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = dir.path().join("project.toml");
        std::fs::write(
            &user,
            r#"
[mcp_servers.local]
command = "npx"
"#,
        )
        .unwrap();
        std::fs::write(
            &project,
            r#"
[mcp_servers.local]
url = "http://127.0.0.1:18989/mcp"
"#,
        )
        .unwrap();
        let list = load_mcp_servers_from(&[user, project]);
        assert_eq!(list.len(), 1);
        assert!(matches!(list[0].transport, McpTransport::Http { .. }));
    }

    #[test]
    fn mcp_disabled_stays_listed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.local]
url = "http://127.0.0.1:18989/mcp"
enabled = false
"#,
        )
        .unwrap();
        let list = load_mcp_servers_from(&[path]);
        assert_eq!(list.len(), 1);
        assert!(!list[0].enabled);
    }

    #[test]
    fn persist_enabled_and_disabled_tools() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
# keep this comment
[mcp_servers.local]
url = "http://127.0.0.1:18989/mcp"
"#,
        )
        .unwrap();
        persist_mcp_server_enabled_in(std::slice::from_ref(&path), "local", false).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("keep this comment"), "{body}");
        assert!(body.contains("enabled = false"), "{body}");
        persist_disabled_mcp_tools_in(std::slice::from_ref(&path), "local", &["echo".into()])
            .unwrap();
        let tools = load_disabled_mcp_tools_from(std::slice::from_ref(&path));
        assert_eq!(
            tools.get("local").cloned().unwrap_or_default(),
            vec!["echo".to_string()]
        );
        persist_disabled_mcp_tools_in(std::slice::from_ref(&path), "local", &[]).unwrap();
        assert!(!load_disabled_mcp_tools_from(&[path]).contains_key("local"));
    }

    #[test]
    fn mcp_bearer_token_from_env() {
        let _env = crate::test_env::scoped().set("DOCK_TEST_MCP_BEARER", "tok-secret");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.local]
url = "http://example/mcp"
bearer_token_env_var = "DOCK_TEST_MCP_BEARER"
"#,
        )
        .unwrap();
        let list = load_mcp_servers_from(&[path]);
        match &list[0].transport {
            McpTransport::Http { headers, .. } => {
                assert_eq!(
                    headers.get("Authorization").map(String::as_str),
                    Some("Bearer tok-secret")
                );
            }
            other => panic!("{other:?}"),
        }
        std::env::remove_var("DOCK_TEST_MCP_BEARER");
    }

    #[test]
    fn mcp_oauth_block_and_transport_client_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.slack]
url = "https://mcp.example/mcp"
oauth_client_id = "transport-client"
oauth_scopes = ["read"]

[mcp_servers.linear]
url = "https://mcp.linear.app/mcp"
[mcp_servers.linear.oauth]
clientId = "slack-byo-client"
callbackPort = 3118
"#,
        )
        .unwrap();
        let list = load_mcp_servers_from(&[path]);
        let slack = list.iter().find(|s| s.name == "slack").unwrap();
        assert_eq!(slack.oauth.client_id.as_deref(), Some("transport-client"));
        assert_eq!(slack.oauth.scopes, vec!["read"]);
        let linear = list.iter().find(|s| s.name == "linear").unwrap();
        assert_eq!(linear.oauth.client_id.as_deref(), Some("slack-byo-client"));
        assert_eq!(linear.oauth.callback_port, Some(3118));
    }

    #[test]
    fn browser_headed_persist_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
# keep this comment
[models]
default = "grok-4"
"#,
        )
        .unwrap();
        assert!(!load_browser_headed_from(std::slice::from_ref(&path)));
        persist_browser_headed_in(std::slice::from_ref(&path), true).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("keep this comment"), "{body}");
        assert!(body.contains("[browser]"), "{body}");
        assert!(body.contains("headed = true"), "{body}");
        assert!(load_browser_headed_from(std::slice::from_ref(&path)));
        persist_browser_headed_in(std::slice::from_ref(&path), false).unwrap();
        assert!(!load_browser_headed_from(&[path]));
    }

    #[test]
    fn browser_headed_defaults_false_and_later_path_wins() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = dir.path().join("project.toml");
        std::fs::write(&user, "[browser]\nheaded = true\n").unwrap();
        std::fs::write(&project, "[models]\ndefault = \"x\"\n").unwrap();
        // Project has no browser.headed → user pref remains.
        assert!(load_browser_headed_from(&[user.clone(), project.clone()]));
        std::fs::write(&project, "[browser]\nheaded = false\n").unwrap();
        assert!(!load_browser_headed_from(&[user, project]));
    }

    #[test]
    fn effective_browser_headed_env_overrides_pref() {
        assert!(effective_browser_headed_with(true, false));
        assert!(effective_browser_headed_with(false, true));
        assert!(effective_browser_headed_with(true, true));
        assert!(!effective_browser_headed_with(false, false));
    }

    #[test]
    fn dock_browser_headed_env_empty_is_unset() {
        let _env = crate::test_env::scoped().remove("DOCK_BROWSER_HEADED");
        assert!(!dock_browser_headed_env_override());
        std::env::set_var("DOCK_BROWSER_HEADED", "");
        assert!(!dock_browser_headed_env_override());
        std::env::set_var("DOCK_BROWSER_HEADED", "   ");
        assert!(!dock_browser_headed_env_override());
        std::env::set_var("DOCK_BROWSER_HEADED", "1");
        assert!(dock_browser_headed_env_override());
    }

    // ── [toolset.web_fetch] ─────────────────────────────────────────────

    #[test]
    fn web_fetch_toolset_defaults_to_all_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[models]\ndefault = \"x\"\n").unwrap();
        assert_eq!(
            load_web_fetch_config_from(std::slice::from_ref(&path)),
            WebFetchToolConfig::default()
        );
        // 文件不存在也一样，不 panic。
        assert_eq!(
            load_web_fetch_config_from(&[dir.path().join("missing.toml")]),
            WebFetchToolConfig::default()
        );
    }

    #[test]
    fn web_fetch_toolset_parses_every_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[toolset.web_fetch]
timeout_secs = 15
max_content_length = 2048
max_markdown_length = 512
allowed_domains = ["docs.rs"]
proxy_endpoint = "http://127.0.0.1:7890"
allow_local = true
"#,
        )
        .unwrap();
        let cfg = load_web_fetch_config_from(&[path]);
        assert_eq!(cfg.timeout_secs, Some(15));
        assert_eq!(cfg.max_content_length, Some(2048));
        assert_eq!(cfg.max_markdown_length, Some(512));
        assert_eq!(cfg.allowed_domains, Some(vec!["docs.rs".to_string()]));
        assert_eq!(cfg.proxy_endpoint.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(cfg.allow_local, Some(true));
    }

    /// 逐字段 overlay：项目文件只写一个键，不能把用户那份的其余键清掉。
    #[test]
    fn web_fetch_toolset_overlays_field_by_field() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = dir.path().join("project.toml");
        std::fs::write(
            &user,
            "[toolset.web_fetch]\ntimeout_secs = 5\nproxy_endpoint = \"http://127.0.0.1:7890\"\n",
        )
        .unwrap();
        std::fs::write(&project, "[toolset.web_fetch]\ntimeout_secs = 90\n").unwrap();
        let cfg = load_web_fetch_config_from(&[user, project]);
        assert_eq!(cfg.timeout_secs, Some(90), "项目文件赢");
        assert_eq!(
            cfg.proxy_endpoint.as_deref(),
            Some("http://127.0.0.1:7890"),
            "项目没表态的键要保住用户那份"
        );
    }

    /// 显式空表不能被当成「没写」——`DomainMatcher` 对空表是**全拒**。
    #[test]
    fn web_fetch_toolset_keeps_explicit_empty_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = dir.path().join("project.toml");
        std::fs::write(
            &user,
            "[toolset.web_fetch]\nallowed_domains = [\"docs.rs\"]\n",
        )
        .unwrap();
        std::fs::write(&project, "[toolset.web_fetch]\nallowed_domains = []\n").unwrap();
        let cfg = load_web_fetch_config_from(&[user, project]);
        assert_eq!(cfg.allowed_domains, Some(Vec::new()));
    }

    /// 认不出的键不能让**整份文件**失效（models 也得还在）——只在日志里喊一声。
    #[test]
    fn unknown_web_fetch_key_does_not_drop_the_rest_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[models]
default = "local-llm"

[toolset.web_fetch]
timeout_secs = 30
proxy_endpont = "http://127.0.0.1:7890"
"#,
        )
        .unwrap();
        assert_eq!(
            load_default_model_from(std::slice::from_ref(&path)).as_deref(),
            Some("local-llm"),
            "拼错的键不该让 models 一起消失"
        );
        let cfg = load_web_fetch_config_from(&[path]);
        assert_eq!(cfg.timeout_secs, Some(30));
        assert_eq!(cfg.proxy_endpoint, None, "拼错的键就是没配上");
    }

    // ── [memory] ───────────────────────────────────────────────────────

    #[test]
    fn memory_defaults_disabled() {
        let _env = crate::test_env::scoped().remove("DOCK_MEMORY");
        let cfg = load_memory_config_from(&[PathBuf::from("/nonexistent/memory.toml")]);
        assert!(!cfg.enabled);
        assert!(!cfg.force_disabled);
        assert!(cfg.flush.enabled);
    }

    #[test]
    fn memory_toml_enables() {
        let _env = crate::test_env::scoped().remove("DOCK_MEMORY");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[memory]
enabled = true
[memory.flush]
soft_threshold_tokens = 2000
[memory.dream]
min_sessions = 3
"#,
        )
        .unwrap();
        let cfg = load_memory_config_from(&[path]);
        assert!(cfg.enabled, "enabled should be true, got {cfg:?}");
        assert_eq!(cfg.flush.soft_threshold_tokens, 2000);
        assert_eq!(cfg.dream.min_sessions, 3);
    }

    #[test]
    fn memory_env_overrides_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[memory]\nenabled = true\n").unwrap();
        let _guard = crate::test_env::scoped().set("DOCK_MEMORY", "0");
        let cfg = load_memory_config_from(&[path]);
        assert!(!cfg.enabled);
        assert!(cfg.force_disabled);
    }
}
