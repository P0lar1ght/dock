//! Local stand-in for `xai_tool_runtime::ToolError` (not path-dep grok-build).

use std::fmt;

#[derive(Debug)]
pub enum ToolError {
    Custom { code: String, message: String },
    InvalidArguments(String),
}

impl ToolError {
    pub fn custom(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Custom {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn invalid_arguments(message: impl Into<String>) -> Self {
        Self::InvalidArguments(message.into())
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Custom { message, .. } => write!(f, "{message}"),
            Self::InvalidArguments(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ToolError {}
