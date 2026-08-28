//! Copied from grok-build `xai-grok-sampling-types` streaming types
//! (`ChatCompletionChunk` / `ChatChunkDelta` / `ToolCallDelta`).
//! Serde defaults are looser so partial SSE frames still parse.

#![allow(dead_code)]

use serde::Deserialize;

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ChatCompletionChunk {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub choices: Vec<ChatChunkChoice>,
    #[serde(default)]
    pub usage: Option<CompletionUsage>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct CompletionUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ChatChunkChoice {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub delta: ChatChunkDelta,
}

/// Streaming delta for a tool call.
///
/// OpenAI-compatible streaming: first chunk carries `id` + `function.name` +
/// the start of `arguments`; later chunks only carry `index` and an
/// `arguments` fragment.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ToolCallDelta {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub function: Option<ToolCallFunctionDelta>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ToolCallFunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ChatChunkDelta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallDelta>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_delta() {
        let raw = r#"{"choices":[{"delta":{"content":"Hi"}}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hi"));
    }
}
