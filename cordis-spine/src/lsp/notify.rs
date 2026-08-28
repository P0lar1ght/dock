//! LSP-facing slice of Grok `ToolNotificationHandle`. Copied method names and
//! payload structs; send is fail-open (noop channel) unless a consumer is wired.

use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspServerStarting {
    pub server_name: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspServerReady {
    pub server_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspServerCrashed {
    pub server_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspServerRetrying {
    pub server_name: String,
    pub attempt: u32,
    pub max_restarts: u32,
    pub backoff_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspServerFailed {
    pub server_name: String,
    pub error: String,
    pub attempts: u32,
}

#[derive(Clone)]
pub struct ToolNotificationHandle {
    _keep: Arc<()>,
}

impl Default for ToolNotificationHandle {
    fn default() -> Self {
        Self::noop()
    }
}

impl ToolNotificationHandle {
    pub fn noop() -> Self {
        Self {
            _keep: Arc::new(()),
        }
    }

    pub fn send_lsp_starting(&self, _value: LspServerStarting) {}
    pub fn send_lsp_ready(&self, _value: LspServerReady) {}
    pub fn send_lsp_crashed(&self, _value: LspServerCrashed) {}
    pub fn send_lsp_retrying(&self, _value: LspServerRetrying) {}
    pub fn send_lsp_failed(&self, _value: LspServerFailed) {}
}
