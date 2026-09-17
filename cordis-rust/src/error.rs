use std::fmt;
use std::sync::Arc;

use thiserror::Error;

/// Result alias for Cordis operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Framework error with a stable machine-readable kind.
#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot create effect on inactive context")]
    InactiveEffect,

    #[error("invalid plugin")]
    InvalidPlugin,

    #[error("listener for {name:?} was rejected by internal/listener")]
    ListenerRejected { name: String },

    #[error("service `{name}` has been registered at <{at}>")]
    ServiceRegistered { name: String, at: String },

    #[error("cannot get property `{name}` without inject")]
    CannotGet { name: String },

    #[error("cannot get required service `{name}` in inactive context")]
    InactiveService { name: String },

    #[error("cannot set property `{name}` without provide")]
    CannotSet { name: String },

    #[error("cannot set property `{name}` in multiple fibers")]
    SetFromOtherFiber { name: String },

    #[error("service `{name}` has the wrong type")]
    TypeMismatch { name: String },

    #[error("property `{name}` is already declared as {kind}")]
    PropertyKind { name: String, kind: String },

    #[error("{0}")]
    Plugin(String),

    #[error("{0}")]
    Message(String),

    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}

impl Error {
    pub fn message(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }

    pub fn plugin(err: impl fmt::Display) -> Self {
        Self::Plugin(err.to_string())
    }
}

/// Several event listeners failed during `parallel` dispatch.
#[derive(Debug)]
pub struct AggregateError {
    pub errors: Vec<Arc<Error>>,
}

impl fmt::Display for AggregateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "aggregate error ({} failure(s))", self.errors.len())
    }
}

impl std::error::Error for AggregateError {}
