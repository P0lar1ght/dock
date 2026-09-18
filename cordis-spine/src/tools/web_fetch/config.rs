//! Runtime-configurable parameters for the `web_fetch` tool.

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::time::Duration;

use cordis_base::config::WebFetchToolConfig;
use serde::{Deserialize, Serialize};

/// 空白串当没写：`proxy_endpoint = ""` 不该被当成「配了代理」——下游
/// `reqwest::Proxy::all("")` 会直接报配置错，而 SSRF 那步会先一步放行。
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

// Safety-boundary constants. Not configurable.
pub const MAX_URL_LENGTH: usize = 2_000;
pub const MAX_REDIRECTS: usize = 10;
pub const USER_AGENT_STRING: &str = "Mozilla/5.0 (compatible; grok-agent/1.0; +https://x.ai)";

/// Runtime-configurable parameters for the `web_fetch` tool.
///
/// Injected via `Params<WebFetchParams>` in `SharedResources`.
/// All fields are optional — `None` means "use built-in default."
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebFetchParams {
    /// Cache time-to-live in seconds. Default: 900 (15 minutes).
    pub cache_ttl_secs: Option<u64>,
    /// Maximum number of cached pages. Default: 128.
    pub max_cache_entries: Option<usize>,
    /// HTTP request timeout in seconds. Default: 60.
    pub timeout_secs: Option<u64>,
    /// Maximum response body size in bytes. Default: 10 MB.
    pub max_content_length: Option<usize>,
    /// Maximum inline markdown output length in bytes. Default: 100,000.
    pub max_markdown_length: Option<usize>,
    /// Model context window size in tokens. Used to enforce 3% cap on web content.
    pub context_window_tokens: Option<u64>,
    /// Domains the tool is allowed to fetch. All other
    /// domains are rejected before any network I/O.
    /// Defaults to `DEFAULT_ALLOWED_DOMAINS` if no
    /// list given.
    #[serde(default)]
    pub allowed_domains: Option<Vec<String>>,
    /// Optional egress proxy endpoint. When set, all HTTP requests are
    /// routed through this URL.
    #[serde(default)]
    pub proxy_endpoint: Option<String>,
    /// When true, allow fetches to **explicit** loopback hosts only
    /// (`localhost`, `127.0.0.0/8`, `::1`). Private/metadata stay blocked.
    /// Default: `false` (fail closed). Set via `[toolset.web_fetch]
    /// allow_local = true`；dock 没有实现 Grok 的
    /// `GROK_WEB_FETCH_ALLOW_LOCAL` 环境变量，配置面只有 config.toml 这一条。
    #[serde(default)]
    pub allow_local: Option<bool>,
}

// Keep defaults here so call-sites don't have to manage unwrapping.
// Vars are still public following other conventions though.
impl WebFetchParams {
    /// `[toolset.web_fetch]` 的解析结果 → 运行时参数。
    ///
    /// 只搬 6 个**真会生效**的键；`cache_ttl_secs` / `max_cache_entries` /
    /// `context_window_tokens` 留 `None`——dock 的管道没有页面缓存，也没实现那个
    /// 上下文帽（见 [`WebFetchToolConfig`](cordis_base::config::WebFetchToolConfig)）。
    pub fn from_tool_config(cfg: &WebFetchToolConfig) -> Self {
        Self {
            timeout_secs: cfg.timeout_secs,
            max_content_length: cfg.max_content_length,
            max_markdown_length: cfg.max_markdown_length,
            allowed_domains: cfg.allowed_domains.clone(),
            proxy_endpoint: nonempty(cfg.proxy_endpoint.clone()),
            allow_local: cfg.allow_local,
            ..Self::default()
        }
    }

    /// 配了转发代理吗。
    ///
    /// 出口由代理解析时，SSRF 那步的**本地** DNS 预检不再构成证据：fake-ip /
    /// 分流 DNS 把公网域名解成保留段假址（本机实测 `example.com` → `198.18.0.164`），
    /// 照判就会把每一次取页面都拦掉。判定不在配置里做，而在
    /// [`ssrf::check_ssrf`](super::ssrf) 里——那里才知道自己要不要查 DNS。
    pub fn via_proxy(&self) -> bool {
        nonempty(self.proxy_endpoint.clone()).is_some()
    }

    pub fn cache_ttl_secs(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_secs.unwrap_or(15 * 60))
    }

    pub fn max_cache_entries(&self) -> usize {
        self.max_cache_entries.unwrap_or(128)
    }

    pub fn timeout_secs(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.unwrap_or(60))
    }

    pub fn max_content_length(&self) -> usize {
        self.max_content_length.unwrap_or(10 * 1024 * 1024)
    }

    pub fn max_markdown_length(&self) -> usize {
        self.max_markdown_length.unwrap_or(100_000)
    }

    pub fn context_window_tokens(&self) -> u64 {
        self.context_window_tokens.unwrap_or(128_000)
    }

    pub fn allow_local(&self) -> bool {
        self.allow_local.unwrap_or(false)
    }

    /// `None` = no extra allowlist (SSRF still applies). Grok defaults to
    /// `DEFAULT_ALLOWED_DOMAINS`; dock keeps that list for opt-in use.
    pub fn allowed_domains(&self) -> Option<Vec<String>> {
        self.allowed_domains.clone()
    }
}

/// Default allowlist for web_fetch tool.
/// Note: GET-only preapproved domains. Path-scoped entries (e.g. vercel.com/docs) are included as-is.
pub static DEFAULT_ALLOWED_DOMAINS: &[&str] = &[
    // xAI
    "x.ai",
    "console.x.ai",
    "docs.x.ai",
    "api.x.ai",
    // Programming languages
    "docs.python.org",
    "en.cppreference.com",
    "docs.oracle.com",
    "learn.microsoft.com",
    "developer.mozilla.org",
    "go.dev",
    "pkg.go.dev",
    "www.php.net",
    "docs.swift.org",
    "kotlinlang.org",
    "ruby-doc.org",
    "doc.rust-lang.org",
    "docs.rs",
    "www.typescriptlang.org",
    // Web and JS frameworks
    "react.dev",
    "angular.io",
    "vuejs.org",
    "nextjs.org",
    "expressjs.com",
    "nodejs.org",
    "bun.sh",
    "jquery.com",
    "getbootstrap.com",
    "tailwindcss.com",
    "d3js.org",
    "threejs.org",
    "redux.js.org",
    "webpack.js.org",
    "jestjs.io",
    "reactrouter.com",
    // Python frameworks
    "docs.djangoproject.com",
    "flask.palletsprojects.com",
    "fastapi.tiangolo.com",
    "pandas.pydata.org",
    "numpy.org",
    "www.tensorflow.org",
    "pytorch.org",
    "scikit-learn.org",
    "matplotlib.org",
    "requests.readthedocs.io",
    "jupyter.org",
    // PHP frameworks
    "laravel.com",
    "symfony.com",
    "wordpress.org",
    // Java frameworks
    "docs.spring.io",
    "hibernate.org",
    "tomcat.apache.org",
    "gradle.org",
    "maven.apache.org",
    // .NET
    "asp.net",
    "dotnet.microsoft.com",
    "nuget.org",
    "blazor.net",
    // Mobile
    "reactnative.dev",
    "docs.flutter.dev",
    "developer.apple.com",
    "developer.android.com",
    // Data science / ML
    "keras.io",
    "spark.apache.org",
    "huggingface.co",
    "www.kaggle.com",
    // Databases
    "redis.io",
    "www.postgresql.org",
    "dev.mysql.com",
    "www.sqlite.org",
    "graphql.org",
    "prisma.io",
    // Cloud and DevOps
    "docs.aws.amazon.com",
    "cloud.google.com",
    "kubernetes.io",
    "www.docker.com",
    "www.terraform.io",
    "www.ansible.com",
    "vercel.com/docs",
    "docs.netlify.com",
    "devcenter.heroku.com",
    // Testing and monitoring
    "cypress.io",
    "selenium.dev",
    // Game development
    "docs.unity.com",
    "docs.unrealengine.com",
    // Other tools
    "git-scm.com",
    "nginx.org",
    "httpd.apache.org",
];
