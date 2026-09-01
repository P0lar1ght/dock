//! Named `"gateway"` seam. The gateway plugin provides this; TUI live-looks it.
//! Do not capture the Arc in a long-lived closure.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::SystemTime;

/// One Origin pairing request waiting on the TUI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingPrompt {
    pub id: String,
    pub application: String,
    pub origin: String,
    pub created_at: SystemTime,
    pub expires_at: SystemTime,
}

/// An application+Origin that the TUI has already approved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingBinding {
    pub application: String,
    pub origin: String,
    pub bound_at: SystemTime,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PairingError {
    pub code: &'static str,
    pub message: String,
}

impl PairingError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PairingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PairingError {}

/// Dual-stack companion bind (`127.0.0.1` ↔ `::1`). Failure must be visible.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompanionStatus {
    /// Plugin is mounted but HTTP is not bound yet (`/pair` 开启).
    Stopped,
    Listening(SocketAddr),
    Failed {
        addr: SocketAddr,
        error: String,
    },
}

impl CompanionStatus {
    pub fn warning(&self) -> Option<String> {
        match self {
            Self::Stopped | Self::Listening(_) => None,
            Self::Failed { addr, error } => Some(format!(
                "未能监听 {addr}（{error}）。本机 IPv6 / localhost 可能连不上。"
            )),
        }
    }
}

/// Pairing surface on `"gateway"`. HTTP/WS keep extra methods on the crate type.
pub trait GatewayPort: Send + Sync {
    fn pairing_front(&self) -> Option<PairingPrompt>;
    fn pairing_pending(&self) -> Vec<PairingPrompt>;
    fn pairing_bindings(&self) -> Vec<PairingBinding>;
    fn pairing_confirm(&self, id: &str) -> Result<(), PairingError>;
    fn pairing_deny(&self, id: &str) -> Result<(), PairingError>;
    fn pairing_revoke(&self, origin: &str) -> Result<(), PairingError>;
    fn local_addr(&self) -> SocketAddr;
    fn companion_status(&self) -> CompanionStatus;
    fn is_listening(&self) -> bool;
    fn start_listen(&self) -> Result<SocketAddr, PairingError>;
    fn stop_listen(&self) -> Result<(), PairingError>;
}

/// Named `"gateway"` service. Clone is cheap; each call looks through to the plugin.
#[derive(Clone)]
pub struct GatewayRef(Arc<dyn GatewayPort>);

impl GatewayRef {
    pub fn new(port: Arc<dyn GatewayPort>) -> Self {
        Self(port)
    }

    pub fn pairing_front(&self) -> Option<PairingPrompt> {
        self.0.pairing_front()
    }

    pub fn pairing_pending(&self) -> Vec<PairingPrompt> {
        self.0.pairing_pending()
    }

    pub fn pairing_bindings(&self) -> Vec<PairingBinding> {
        self.0.pairing_bindings()
    }

    pub fn pairing_confirm(&self, id: &str) -> Result<(), PairingError> {
        self.0.pairing_confirm(id)
    }

    pub fn pairing_deny(&self, id: &str) -> Result<(), PairingError> {
        self.0.pairing_deny(id)
    }

    pub fn pairing_revoke(&self, origin: &str) -> Result<(), PairingError> {
        self.0.pairing_revoke(origin)
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.0.local_addr()
    }

    pub fn companion_status(&self) -> CompanionStatus {
        self.0.companion_status()
    }

    pub fn is_listening(&self) -> bool {
        self.0.is_listening()
    }

    pub fn start_listen(&self) -> Result<SocketAddr, PairingError> {
        self.0.start_listen()
    }

    pub fn stop_listen(&self) -> Result<(), PairingError> {
        self.0.stop_listen()
    }
}
