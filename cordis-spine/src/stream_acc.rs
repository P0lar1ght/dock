//! Copied from grok-build `xai-grok-sampler/src/stream/chat_completions.rs`
//! accumulator: `content_acc` / `reasoning_acc` / `tool_call_acc` by index.
//! No SamplingEvent / RequestId / idle timer.

use std::collections::BTreeMap;

use crate::chat_chunk::ChatCompletionChunk;
use crate::types::{LlmOutput, ToolCall};

/// Live token for the TUI. Reasoning stays off the assistant markdown body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamDelta {
    Text(String),
    Reasoning(String),
    /// Last-turn live display. `official` is SSE `usage`; estimates must not
    /// hit the session ledger (Grok fail-closed: absence ≠ free).
    Usage {
        tokens: crate::usage::TokenUsage,
        official: bool,
        model: String,
        cost_usd_ticks: Option<i64>,
    },
}

/// Per-response accumulators from Grok's L2 chat-completions transform.
#[derive(Debug, Default)]
pub struct ChatStreamAcc {
    content: String,
    reasoning: String,
    /// index → (id, name, arguments)
    tool_calls: BTreeMap<u32, (String, String, String)>,
}

impl ChatStreamAcc {
    pub fn ingest(&mut self, chunk: ChatCompletionChunk) -> Vec<StreamDelta> {
        let mut out = Vec::new();
        let usage = chunk.usage;
        for choice in chunk.choices {
            let delta = choice.delta;
            if let Some(ref text) = delta.content {
                if !text.is_empty() {
                    self.content.push_str(text);
                    out.push(StreamDelta::Text(text.clone()));
                }
            }
            if let Some(thought) = delta.reasoning_text() {
                self.reasoning.push_str(&thought);
                out.push(StreamDelta::Reasoning(thought));
            }
            for tc_delta in delta.tool_calls {
                let entry = self
                    .tool_calls
                    .entry(tc_delta.index)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                if let Some(id) = tc_delta.id {
                    entry.0 = id;
                }
                if let Some(func) = tc_delta.function {
                    if let Some(name) = func.name {
                        entry.1 = name;
                    }
                    if let Some(args) = func.arguments {
                        entry.2.push_str(&args);
                    }
                }
            }
        }
        if let Some(usage) = usage {
            if usage.prompt_tokens > 0 || usage.completion_tokens > 0 {
                out.push(StreamDelta::Usage {
                    tokens: usage.to_token_usage(),
                    official: true,
                    model: chunk.model,
                    cost_usd_ticks: usage.cost_in_usd_ticks,
                });
            }
        }
        out
    }

    pub fn finish(self) -> LlmOutput {
        let tool_calls = self
            .tool_calls
            .into_values()
            .filter(|(_, name, _)| !name.is_empty())
            .map(|(id, name, arguments)| ToolCall {
                id: if id.is_empty() { "call-0".into() } else { id },
                name,
                arguments,
            })
            .collect();
        LlmOutput {
            text: self.content,
            reasoning: self.reasoning,
            tool_calls,
            ..LlmOutput::default()
        }
    }
}

/// Split SSE `data:` payloads. `[DONE]` is omitted (Grok client terminates there).
pub fn take_sse_data(buf: &mut Vec<u8>) -> Vec<String> {
    let mut events = Vec::new();
    while let Some(end) = find_event_end(buf) {
        let raw: Vec<u8> = buf.drain(..end).collect();
        if let Some(data) = sse_data_field(&raw) {
            if data == "[DONE]" {
                continue;
            }
            if !data.is_empty() {
                events.push(data);
            }
        }
    }
    events
}

fn find_event_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| buf.windows(2).position(|w| w == b"\n\n").map(|i| i + 2))
}

fn sse_data_field(event: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(event);
    let mut data = String::new();
    let mut any = false;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            if any {
                data.push('\n');
            }
            any = true;
            data.push_str(rest.trim_start());
        }
    }
    any.then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_text_and_tool_fragments() {
        let mut acc = ChatStreamAcc::default();
        let d1 = acc
            .ingest(serde_json::from_str(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#).unwrap());
        let d2 = acc
            .ingest(serde_json::from_str(r#"{"choices":[{"delta":{"content":"lo"}}]}"#).unwrap());
        assert_eq!(d1, vec![StreamDelta::Text("Hel".into())]);
        assert_eq!(d2, vec![StreamDelta::Text("lo".into())]);
        acc.ingest(
            serde_json::from_str(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"1","function":{"name":"grep","arguments":"{\"p\""}}]}}]}"#,
            )
            .unwrap(),
        );
        acc.ingest(
            serde_json::from_str(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"x\"}"}}]}}]}"#,
            )
            .unwrap(),
        );
        let out = acc.finish();
        assert_eq!(out.text, "Hello");
        assert_eq!(out.tool_calls[0].name, "grep");
        assert_eq!(out.tool_calls[0].arguments, r#"{"p":"x"}"#);
    }

    #[test]
    fn sse_splits_data_frames() {
        let mut buf = b"data: {\"a\":1}\n\ndata: [DONE]\n\ndata: {\"b\":2}\n".to_vec();
        let frames = take_sse_data(&mut buf);
        assert_eq!(frames, vec![r#"{"a":1}"#]);
        buf.extend_from_slice(b"\n");
        let frames = take_sse_data(&mut buf);
        assert_eq!(frames, vec![r#"{"b":2}"#]);
    }

    #[test]
    fn ingest_emits_usage() {
        let mut acc = ChatStreamAcc::default();
        let deltas = acc.ingest(
            serde_json::from_str(
                r#"{"choices":[{"delta":{"content":"hi"}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#,
            )
            .unwrap(),
        );
        assert!(deltas
            .iter()
            .any(|d| matches!(d, StreamDelta::Text(t) if t == "hi")));
        assert!(deltas.iter().any(|d| matches!(
            d,
            StreamDelta::Usage { tokens, official: true, .. } if tokens.prompt_tokens == 10 && tokens.completion_tokens == 2
        )));
    }

    #[test]
    fn ingest_emits_cache_and_reasoning() {
        let mut acc = ChatStreamAcc::default();
        let deltas = acc.ingest(
            serde_json::from_str(
                r#"{"model":"m","choices":[{"delta":{}}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_tokens_details":{"cached_tokens":40},"completion_tokens_details":{"reasoning_tokens":8}}}"#,
            )
            .unwrap(),
        );
        match &deltas[0] {
            StreamDelta::Usage {
                tokens,
                official: true,
                model,
                ..
            } => {
                assert_eq!(tokens.cached_prompt_tokens, 40);
                assert_eq!(tokens.reasoning_tokens, 8);
                assert_eq!(model, "m");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reasoning_stays_off_the_text_delta() {
        let mut acc = ChatStreamAcc::default();
        let deltas = acc.ingest(
            serde_json::from_str(
                r#"{"choices":[{"delta":{"reasoning_content":"plan","content":"hi"}}]}"#,
            )
            .unwrap(),
        );
        assert_eq!(
            deltas,
            vec![
                StreamDelta::Text("hi".into()),
                StreamDelta::Reasoning("plan".into()),
            ]
        );
        let out = acc.finish();
        assert_eq!(out.text, "hi");
        assert_eq!(out.reasoning, "plan");
    }

    #[test]
    fn openrouter_reasoning_string_field() {
        let mut acc = ChatStreamAcc::default();
        let deltas = acc.ingest(
            serde_json::from_str(r#"{"choices":[{"delta":{"reasoning":"step"}}]}"#).unwrap(),
        );
        assert_eq!(deltas, vec![StreamDelta::Reasoning("step".into())]);
        assert_eq!(acc.finish().reasoning, "step");
    }

    #[test]
    fn openrouter_reasoning_details_text() {
        let mut acc = ChatStreamAcc::default();
        let deltas = acc.ingest(
            serde_json::from_str(
                r#"{"choices":[{"delta":{"reasoning_details":[
                    {"type":"reasoning.text","text":"Let me think"},
                    {"type":"reasoning.encrypted","data":"xx"},
                    {"type":"reasoning.summary","summary":"…"}
                ]}}]}"#,
            )
            .unwrap(),
        );
        assert_eq!(deltas, vec![StreamDelta::Reasoning("Let me think…".into())]);
        assert_eq!(acc.finish().reasoning, "Let me think…");
    }
}
