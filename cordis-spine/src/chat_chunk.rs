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

/// Copied from grok-build `xai-grok-sampling-types::Usage` (streaming subset).
#[derive(Debug, Deserialize, Clone, Default)]
pub struct CompletionUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
    /// xAI extension: request price in USD ticks (1 USD = 1e10 ticks).
    #[serde(default)]
    pub cost_in_usd_ticks: Option<i64>,
    /// DeepSeek / some OpenAI-compatible proxies.
    #[serde(default)]
    pub prompt_cache_hit_tokens: u64,
    /// Anthropic Messages fields sometimes forwarded on chat/completions.
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct PromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct CompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u64,
}

impl CompletionUsage {
    pub fn to_token_usage(&self) -> crate::usage::TokenUsage {
        crate::usage::TokenUsage {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            total_tokens: self.total_tokens,
            reasoning_tokens: self
                .completion_tokens_details
                .as_ref()
                .map_or(0, |d| d.reasoning_tokens),
            cached_prompt_tokens: self.cached_read(),
            cache_creation_prompt_tokens: self.cache_creation(),
        }
    }

    /// OpenAI `prompt_tokens_details.cached_tokens` first (Grok identity);
    /// fall back to DeepSeek / Anthropic aliases when details are absent.
    fn cached_read(&self) -> u64 {
        let details = self
            .prompt_tokens_details
            .as_ref()
            .map_or(0, |d| d.cached_tokens);
        if details > 0 {
            details
        } else if self.prompt_cache_hit_tokens > 0 {
            self.prompt_cache_hit_tokens
        } else {
            self.cache_read_input_tokens
        }
    }

    fn cache_creation(&self) -> u64 {
        let write = self
            .prompt_tokens_details
            .as_ref()
            .map_or(0, |d| d.cache_write_tokens);
        if write > 0 {
            write
        } else {
            self.cache_creation_input_tokens
        }
    }
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

    #[test]
    fn parses_usage_details() {
        let raw = r#"{
            "choices":[{"delta":{}}],
            "model":"grok-4",
            "usage":{
                "prompt_tokens":100,
                "completion_tokens":20,
                "total_tokens":120,
                "prompt_tokens_details":{"cached_tokens":40},
                "completion_tokens_details":{"reasoning_tokens":8},
                "cost_in_usd_ticks":123
            }
        }"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        let usage = chunk.usage.unwrap();
        let tu = usage.to_token_usage();
        assert_eq!(tu.prompt_tokens, 100);
        assert_eq!(tu.cached_prompt_tokens, 40);
        assert_eq!(tu.reasoning_tokens, 8);
        assert_eq!(usage.cost_in_usd_ticks, Some(123));
    }

    #[test]
    fn parses_deepseek_prompt_cache_hit() {
        let raw = r#"{
            "choices":[{"delta":{}}],
            "usage":{
                "prompt_tokens":100,
                "completion_tokens":20,
                "prompt_cache_hit_tokens":40
            }
        }"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        assert_eq!(
            chunk.usage.unwrap().to_token_usage().cached_prompt_tokens,
            40
        );
    }
}
