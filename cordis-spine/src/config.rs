//! Dock config.toml — Grok's `$GROK_HOME/config.toml` shape, trimmed.
//!
//! Catalog merge (same order as Grok `resolve_model_list`):
//! built-in defaults < user `~/.dock/config.toml` < project `.dock/config.toml`.
//! `[models].catalog` replaces the built-in list; `[model.<id>]` adds/overrides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Deserialize;

/// Grok `ApiBackend`: which inference wire the `llm` plugin speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApiBackend {
    #[default]
    ChatCompletions,
    Responses,
    Messages,
}

impl ApiBackend {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("responses") | Some("resp") => Self::Responses,
            Some("messages") | Some("anthropic") => Self::Messages,
            _ => Self::ChatCompletions,
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat/completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
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
    /// Grok `[model.<id>].api_backend` (`chat_completions` / `responses` / `messages`).
    pub api_backend: ApiBackend,
    /// `None` = Bearer, except Messages defaults to `x-api-key`.
    pub auth_scheme: Option<AuthScheme>,
    /// Wire slug in the JSON body. None = use [`Self::id`] (picker key).
    pub api_model: Option<String>,
}

impl ModelChoice {
    pub fn has_http(&self) -> bool {
        self.api_base_url
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
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

    pub fn resolved_auth(&self) -> AuthScheme {
        self.auth_scheme.unwrap_or(match self.api_backend {
            ApiBackend::Messages => AuthScheme::XApiKey,
            ApiBackend::ChatCompletions | ApiBackend::Responses => AuthScheme::Bearer,
        })
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

/// How Dock frames JSON-RPC on MCP stdio (LSP Content-Length vs NDJSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpStdioFraming {
    /// Probe Content-Length on a fresh spawn; on JSON-RPC parse error (-32700), kill and respawn as NDJSON.
    #[default]
    Auto,
    ContentLength,
    Ndjson,
}

impl McpStdioFraming {
    /// Parse config aliases (`auto`, `content-length` / `cl`, `ndjson` / `jsonl`, …).
    pub fn from_config(raw: Option<&str>) -> Self {
        let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::Auto;
        };
        match raw.to_ascii_lowercase().as_str() {
            "auto" => Self::Auto,
            "content-length" | "content_length" | "contentlength" | "cl" | "lsp" => {
                Self::ContentLength
            }
            "ndjson" | "newline" | "nl" | "jsonl" | "line" => Self::Ndjson,
            other => {
                tracing::warn!(framing = other, "unknown mcp stdio framing; using auto");
                Self::Auto
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

/// Live-read MCP servers from config. Fail-open: missing files → empty.
/// Later files overlay the same name (Grok project `< user`).
pub fn load_mcp_servers() -> Vec<McpServer> {
    load_mcp_servers_from(&catalog_paths())
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
    let path = mcp_persist_target(paths, name)?;
    patch_toml(&path, |doc| {
        let Some(item) = doc.get_mut("mcp_servers").and_then(|t| t.get_mut(name)) else {
            return Err(format!("config has no [mcp_servers.{name}]"));
        };
        item["enabled"] = toml_edit::value(enabled);
        Ok(())
    })
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
    auth_scheme: Option<String>,
    #[serde(default)]
    api_model: Option<String>,
}

/// Built-in catalog (Grok `default_model_entries` role). Picker reads this only
/// when no config file supplies `[[models.catalog]]`.
pub fn default_model_entries() -> Vec<ModelChoice> {
    [
        ("grok-4", "Grok 4", "xAI"),
        ("grok-3", "Grok 3", "xAI"),
        ("grok-2-latest", "Grok 2", "xAI"),
        ("gpt-4.1", "GPT-4.1", "OpenAI-compatible"),
        ("claude-sonnet-4", "Claude Sonnet 4", "Anthropic-compatible"),
    ]
    .into_iter()
    .map(|(id, name, description)| ModelChoice {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        api_base_url: None,
        api_key: None,
        env_key: None,
        context_window: None,
        api_backend: ApiBackend::ChatCompletions,
        auth_scheme: None,
        api_model: None,
    })
    .collect()
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

pub fn load_catalog_from(paths: &[PathBuf]) -> Vec<ModelChoice> {
    let mut list = default_model_entries();
    for path in paths {
        let Some(file) = read_file(path) else {
            continue;
        };
        if !file.models.catalog.is_empty() {
            list = file.models.catalog.iter().map(choice_from_row).collect();
        }
        merge_overrides(&mut list, &file.model);
    }
    if list.is_empty() {
        default_model_entries()
    } else {
        list
    }
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

fn choice_from_row(row: &CatalogRow) -> ModelChoice {
    ModelChoice {
        id: row.id.clone(),
        name: row.name.clone().unwrap_or_else(|| row.id.clone()),
        description: row.description.clone().unwrap_or_default(),
        api_base_url: nonempty(row.api_base_url.clone()),
        api_key: nonempty(row.api_key.clone()),
        env_key: nonempty(row.env_key.clone()),
        context_window: row.context_window.filter(|n| *n > 0),
        api_backend: ApiBackend::parse(row.api_backend.as_deref()),
        auth_scheme: AuthScheme::parse(row.auth_scheme.as_deref()),
        api_model: nonempty(row.api_model.clone()),
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
            if ov.api_backend.is_some() {
                existing.api_backend = ApiBackend::parse(ov.api_backend.as_deref());
            }
            if ov.auth_scheme.is_some() {
                existing.auth_scheme = AuthScheme::parse(ov.auth_scheme.as_deref());
            }
            if ov.api_model.is_some() {
                existing.api_model = nonempty(ov.api_model.clone());
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
                api_backend: ApiBackend::parse(ov.api_backend.as_deref()),
                auth_scheme: AuthScheme::parse(ov.auth_scheme.as_deref()),
                api_model: nonempty(ov.api_model.clone()),
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
        let list = load_catalog_from(&[path.clone()]);
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
        assert_eq!(claude.api_backend, ApiBackend::Messages);
        assert_eq!(claude.resolved_auth(), AuthScheme::XApiKey);
        let gpt = list.iter().find(|m| m.id == "gpt").unwrap();
        assert_eq!(gpt.api_backend, ApiBackend::Responses);
        assert_eq!(gpt.resolved_auth(), AuthScheme::Bearer);
        assert_eq!(ApiBackend::Messages.path(), "messages");
        assert_eq!(ApiBackend::Responses.path(), "responses");
        assert_eq!(ApiBackend::ChatCompletions.path(), "chat/completions");
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
        assert_eq!(m.api_backend, ApiBackend::Responses);
        assert_eq!(m.resolved_auth(), AuthScheme::Bearer);
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
        let list = load_catalog_from(&[path.clone()]);
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
        assert_eq!(McpStdioFraming::from_config(Some("cl")), McpStdioFraming::ContentLength);
        assert_eq!(McpStdioFraming::from_config(Some("auto")), McpStdioFraming::Auto);
        assert_eq!(McpStdioFraming::from_config(None), McpStdioFraming::Auto);
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
        persist_mcp_server_enabled_in(&[path.clone()], "local", false).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("keep this comment"), "{body}");
        assert!(body.contains("enabled = false"), "{body}");
        persist_disabled_mcp_tools_in(&[path.clone()], "local", &["echo".into()]).unwrap();
        let tools = load_disabled_mcp_tools_from(&[path.clone()]);
        assert_eq!(
            tools.get("local").cloned().unwrap_or_default(),
            vec!["echo".to_string()]
        );
        persist_disabled_mcp_tools_in(&[path.clone()], "local", &[]).unwrap();
        assert!(!load_disabled_mcp_tools_from(&[path]).contains_key("local"));
    }

    #[test]
    fn mcp_bearer_token_from_env() {
        std::env::set_var("DOCK_TEST_MCP_BEARER", "tok-secret");
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
}
