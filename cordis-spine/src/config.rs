//! Dock config.toml — Grok's `$GROK_HOME/config.toml` shape, trimmed.
//!
//! Catalog merge (same order as Grok `resolve_model_list`):
//! built-in defaults < user `~/.dock/config.toml` < project `.dock/config.toml`.
//! `[models].catalog` replaces the built-in list; `[model.<id>]` adds/overrides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

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
}

#[derive(Debug, Default, Deserialize)]
struct McpSection {
    #[serde(default)]
    servers: Vec<McpServerRow>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpServerRow {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

/// Live-read MCP servers from config. Fail-open: missing files → empty.
pub fn load_mcp_servers() -> Vec<McpServer> {
    load_mcp_servers_from(&catalog_paths())
}

pub fn load_mcp_servers_from(paths: &[PathBuf]) -> Vec<McpServer> {
    let mut out = Vec::new();
    for path in paths {
        let Some(file) = read_file(path) else {
            continue;
        };
        for row in file.mcp.servers {
            if row.command.trim().is_empty() {
                continue;
            }
            let name = row
                .name
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| row.command.clone());
            out.push(McpServer {
                name,
                command: row.command,
                args: row.args,
            });
        }
        for (key, row) in file.mcp_servers {
            if row.command.trim().is_empty() {
                continue;
            }
            out.push(McpServer {
                name: key,
                command: row.command,
                args: row.args,
            });
        }
    }
    out
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
            existing.id = id;
        } else {
            list.push(ModelChoice {
                name: ov.name.clone().unwrap_or_else(|| id.clone()),
                description: ov.description.clone().unwrap_or_default(),
                api_base_url: nonempty(ov.api_base_url.clone()),
                api_key: nonempty(ov.api_key.clone()),
                env_key: nonempty(ov.env_key.clone()),
                context_window: ov.context_window.filter(|n| *n > 0),
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
}
