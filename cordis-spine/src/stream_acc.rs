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
    /// provider 在流中途发的错误。与 messages / responses 两条 wire 同构：
    /// 只进 [`LlmOutput::error`]，不进 `text`（`text` 是下一轮回放历史的门槛）。
    error: Option<String>,
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
        // 先看错误：这一轮失败了，攒到一半的 tool_call 是残缺的，不该回放。
        if let Some(err) = self.error {
            return LlmOutput {
                error: Some(err),
                ..LlmOutput::default()
            };
        }
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

    /// 记下一条流内错误（第一条为准：后续多半是同一次失败的回声）。
    pub fn set_error(&mut self, err: String) {
        if self.error.is_none() {
            self.error = Some(err);
        }
    }
}

/// 从一个**不是** `ChatCompletionChunk` 的 SSE 帧里认出错误信封。
///
/// chat/completions 没有像 Anthropic `{"type":"error"}` 那样的统一事件类型，
/// 各家形状不一：OpenAI 是 `{"error":{"message":…,"type":…}}`，不少代理直接给
/// `{"error":"…"}`，还有的把 `message` 放在顶层。认不出就回 `None`，调用方照旧
/// 丢弃该帧。
pub fn chat_stream_error(data: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let err = v.get("error")?;
    // `"error": null` 是「这一帧没出错」的常见写法，不是错误。
    if err.is_null() {
        return None;
    }
    // 带增量的帧优先当 chunk 处理：真正的错误信封不会同时捎着 choices，
    // 而把一条有内容的 chunk 误判成错误会把这一轮的正文整个吞掉。
    if v.get("choices")
        .and_then(|c| c.as_array())
        .is_some_and(|c| !c.is_empty())
    {
        return None;
    }
    let text = match err {
        serde_json::Value::String(s) => s.clone(),
        _ => {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .or_else(|| v.get("message").and_then(|m| m.as_str()))
                .unwrap_or_default();
            let kind = err
                .get("type")
                .and_then(|t| t.as_str())
                .or_else(|| err.get("code").and_then(|c| c.as_str()))
                .unwrap_or("error");
            if msg.is_empty() {
                // 认出是错误但取不出人话，至少把原始 JSON 带上，
                // 别让这一轮空白收场。
                format!("{kind}: {err}")
            } else {
                format!("{kind}: {msg}")
            }
        }
    };
    Some(format!("llm chat {text}"))
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

    /// provider 在流中途报错时，旧实现把整帧静默丢弃，这一轮**完全空白地
    /// 收场**——没有文本也没有报错，用户看到的就是应用坏了。现在要认出来。
    #[test]
    fn midstream_error_is_captured_not_swallowed() {
        for raw in [
            r#"{"error":{"message":"rate limit exceeded","type":"rate_limit_error"}}"#,
            r#"{"error":"rate limit exceeded"}"#,
            r#"{"error":{"code":"rate_limit_error","message":"rate limit exceeded"}}"#,
        ] {
            let err = chat_stream_error(raw).unwrap_or_else(|| panic!("认不出：{raw}"));
            assert!(err.contains("rate limit exceeded"), "{err}");
        }
    }

    /// 错误只进 `error`，不进 `text`——`text` 是下一轮回放历史的门槛，
    /// 进了就等于让模型「说过」一句 harness 的报错。
    #[test]
    fn midstream_error_goes_to_error_not_text() {
        let mut acc = ChatStreamAcc::default();
        acc.set_error(
            chat_stream_error(r#"{"error":{"message":"boom","type":"server_error"}}"#).unwrap(),
        );
        let out = acc.finish();
        assert!(out.text.is_empty(), "流内错误漏进 text：{}", out.text);
        assert_eq!(out.error.as_deref(), Some("llm chat server_error: boom"));
    }

    /// 普通 chunk 不能被误判成错误帧。
    #[test]
    fn ordinary_frames_are_not_mistaken_for_errors() {
        assert!(chat_stream_error(r#"{"choices":[{"delta":{"content":"hi"}}]}"#).is_none());
        assert!(chat_stream_error("[DONE]").is_none());
        assert!(chat_stream_error("not json").is_none());
    }
}
