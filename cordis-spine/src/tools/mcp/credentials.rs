//! Persistent MCP OAuth tokens: `$DOCK_HOME/mcp_credentials.json`.
//!
//! Shape matches Grok `$GROK_HOME/mcp_credentials.json` so a copied file still
//! loads. Isolated from model API keys in `config.toml`.

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use cordis_base::config::dock_home;

use crate::tools::fs_perms::ensure_owner_only;

const CREDENTIALS_FILENAME: &str = "mcp_credentials.json";

#[derive(Debug, thiserror::Error)]
pub enum McpCredentialError {
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

type Result<T> = std::result::Result<T, McpCredentialError>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoredCredentials {
    pub client_id: String,
    #[serde(default)]
    pub token_response: Option<TokenResponse>,
    #[serde(default)]
    pub granted_scopes: Vec<String>,
    #[serde(default)]
    pub token_received_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct McpCredentialStore {
    #[serde(flatten)]
    entries: BTreeMap<String, StoredCredentials>,
}

impl std::fmt::Debug for McpCredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpCredentialStore")
            .field("entry_count", &self.entries.len())
            .finish()
    }
}

impl McpCredentialStore {
    pub fn key(server_name: &str, server_url: &Url) -> String {
        format!("{server_name}:{server_url}")
    }

    pub fn default_path() -> PathBuf {
        dock_home().join(CREDENTIALS_FILENAME)
    }

    pub fn load_default() -> Result<Self> {
        Self::load_from(&Self::default_path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        if let Err(e) = ensure_owner_only(path) {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "mcp credentials: failed to enforce owner-only permissions"
            );
        }
        Ok(serde_json::from_str(&content)?)
    }

    pub fn get(&self, server_name: &str, server_url: &Url) -> Option<&StoredCredentials> {
        self.entries.get(&Self::key(server_name, server_url))
    }

    pub fn insert_and_save(
        &mut self,
        server_name: &str,
        server_url: &Url,
        creds: StoredCredentials,
    ) -> Result<()> {
        let key = Self::key(server_name, server_url);
        self.entries.insert(key, creds);
        self.save_default()
    }

    pub fn save_default(&self) -> Result<()> {
        self.save_to(&Self::default_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        ensure_owner_only(&tmp)?;
        std::fs::rename(&tmp, path)?;
        ensure_owner_only(path)?;
        Ok(())
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn access_token(server_name: &str, server_url: &str) -> Option<String> {
    let url = Url::parse(server_url).ok()?;
    let store = McpCredentialStore::load_default().ok()?;
    let creds = store.get(server_name, &url)?;
    let token = creds.token_response.as_ref()?;
    if token.access_token.trim().is_empty() {
        return None;
    }
    if let (Some(at), Some(ttl)) = (creds.token_received_at, token.expires_in) {
        if now_unix().saturating_sub(at) + 60 >= ttl {
            return None;
        }
    }
    Some(token.access_token.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_on_disk_fixture_still_deserializes() {
        let fixture = r#"{
            "linear:https://mcp.example.com/mcp": {
                "client_id": "legacy-client-id",
                "token_response": {
                    "access_token": "at-123",
                    "token_type": "bearer",
                    "expires_in": 3600,
                    "refresh_token": "rt-456",
                    "scope": "read write"
                },
                "granted_scopes": ["read", "write"],
                "token_received_at": 1730000000
            },
            "noauth:https://example.com/mcp": {
                "client_id": "c2",
                "token_response": null
            }
        }"#;
        let store: McpCredentialStore = serde_json::from_str(fixture).unwrap();
        let url = Url::parse("https://mcp.example.com/mcp").unwrap();
        let creds = store.get("linear", &url).expect("legacy entry loads");
        assert_eq!(creds.client_id, "legacy-client-id");
        let token = creds.token_response.as_ref().unwrap();
        assert_eq!(token.access_token, "at-123");
        assert_eq!(token.refresh_token.as_deref(), Some("rt-456"));
    }

    #[test]
    fn save_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("creds.json");
        let mut store = McpCredentialStore::default();
        let url = Url::parse("https://test.example.com/mcp").unwrap();
        store.entries.insert(
            McpCredentialStore::key("test", &url),
            StoredCredentials {
                client_id: "c".into(),
                token_response: Some(TokenResponse {
                    access_token: "tok".into(),
                    token_type: "bearer".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        store.save_to(&path).unwrap();
        let loaded = McpCredentialStore::load_from(&path).unwrap();
        assert_eq!(
            loaded
                .get("test", &url)
                .unwrap()
                .token_response
                .as_ref()
                .unwrap()
                .access_token,
            "tok"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
