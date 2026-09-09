//! Anthropic Messages API (`POST /v1/messages`). Wire mapping copied from
//! Grok `conversation/messages.rs` + L2 `stream/messages.rs`. Thinking from
//! prior turns is omitted: Dock has no encrypted `signature` to replay.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::stream_acc::StreamDelta;
use crate::types::{
    LlmOutput, LogEvent, PromptRequest, ToolCall, UserImage, INTERRUPTED_TOOL_RESULT,
};
use crate::usage::TokenUsage;

/// Anthropic requires `max_tokens`. Grok fills sampler defaults; Dock uses
/// this when config has no per-request cap.
const DEFAULT_MAX_TOKENS: u32 = 16_384;

pub fn body(
    model: &str,
    request: &PromptRequest,
    user_images: &[Vec<UserImage>],
    thinking: bool,
    effort: &str,
) -> Value {
    let (system, messages) = transcript(request, user_images, model);
    let mut body = json!({
        "model": model,
        "messages": messages,
        "max_tokens": DEFAULT_MAX_TOKENS,
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|t| {
                    let input_schema: Value = serde_json::from_str(&t.parameters_json)
                        .unwrap_or(json!({"type":"object"}));
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": input_schema,
                    })
                })
                .collect(),
        );
    }
    if thinking && effort != "none" {
        body["thinking"] = json!({
            "type": "adaptive",
            "display": "summarized",
        });
    }
    body
}

pub fn transcript(request: &PromptRequest, user_images: &[Vec<UserImage>], model: &str) -> (String, Vec<Value>) {
    let mut messages = Vec::new();
    let mut user_i = 0usize;
    let mut pending: Vec<String> = Vec::new();
    let mut pending_results: Vec<Value> = Vec::new();
    for event in &request.history {
        match event {
            LogEvent::User(text) => {
                flush_unmatched(&mut pending_results, &mut pending);
                flush_tool_results(&mut messages, &mut pending_results);
                let images = user_images.get(user_i).cloned().unwrap_or_default();
                user_i += 1;
                messages.push(user_message(text, &images));
            }
            LogEvent::SystemReminder(text) => {
                flush_unmatched(&mut pending_results, &mut pending);
                flush_tool_results(&mut messages, &mut pending_results);
                messages.push(user_message(text, &[]));
            }
            LogEvent::LlmStream(llm) if !llm.tool_calls.is_empty() || !llm.text.is_empty() => {
                flush_tool_results(&mut messages, &mut pending_results);
                let mut blocks = Vec::new();
                if !llm.text.is_empty() {
                    blocks.push(json!({"type": "text", "text": llm.text}));
                }
                for c in &llm.tool_calls {
                    let input = serde_json::from_str(&c.arguments).unwrap_or(json!({}));
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": sanitize_tool_id(&c.id),
                        "name": c.name,
                        "input": input,
                    }));
                }
                if !blocks.is_empty() {
                    messages.push(json!({
                        "role": "assistant",
                        "content": blocks,
                    }));
                }
                pending = llm.tool_calls.iter().map(|c| c.id.clone()).collect();
            }
            LogEvent::ToolExecute {
                id,
                content,
                images,
                ..
            } => {
                if let Some(i) = pending.iter().position(|p| p == id) {
                    pending.remove(i);
                    pending_results.push(super::tool_images::messages_tool_result_block(
                        &sanitize_tool_id(id),
                        content,
                        images,
                        model,
                    ));
                }
            }
            LogEvent::PreStep | LogEvent::Prompt(_) | LogEvent::LlmStream(_) => {}
        }
    }
    flush_unmatched(&mut pending_results, &mut pending);
    flush_tool_results(&mut messages, &mut pending_results);
    (request.system.clone(), messages)
}

fn flush_unmatched(results: &mut Vec<Value>, pending: &mut Vec<String>) {
    for id in pending.drain(..) {
        results.push(tool_result_block(&id, INTERRUPTED_TOOL_RESULT));
    }
}

fn flush_tool_results(messages: &mut Vec<Value>, results: &mut Vec<Value>) {
    if results.is_empty() {
        return;
    }
    messages.push(json!({
        "role": "user",
        "content": std::mem::take(results),
    }));
}

fn tool_result_block(id: &str, content: &str) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": sanitize_tool_id(id),
        "content": content,
    })
}

pub fn sanitize_tool_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn user_message(text: &str, images: &[UserImage]) -> Value {
    if images.is_empty() {
        return json!({"role": "user", "content": text});
    }
    use base64::Engine;
    let mut parts = Vec::new();
    if !text.trim().is_empty() {
        parts.push(json!({"type": "text", "text": text}));
    }
    for img in images {
        let b64 = base64::engine::general_purpose::STANDARD.encode(img.data.as_ref());
        parts.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": img.mime,
                "data": b64,
            }
        }));
    }
    if parts.is_empty() {
        json!({"role": "user", "content": text})
    } else {
        json!({"role": "user", "content": parts})
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockType {
    Text,
    ToolUse,
    Thinking,
}

#[derive(Debug)]
struct BlockState {
    kind: BlockType,
    text: String,
    tool_name: String,
    tool_id: String,
    args: String,
    thinking: String,
}

/// Per-block Messages SSE accumulator (Grok `BlockState`).
#[derive(Debug, Default)]
pub struct Acc {
    model: String,
    blocks: BTreeMap<u32, BlockState>,
    content: String,
    reasoning: String,
    tool_calls: Vec<ToolCall>,
    next_tool: u32,
    block_to_tool: BTreeMap<u32, u32>,
    input_tokens: u64,
    cache_read: u64,
    cache_creation: u64,
    output_tokens: u64,
    error: Option<String>,
}

impl Acc {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Self::default()
        }
    }

    pub fn ingest_json(&mut self, data: &str) -> Vec<StreamDelta> {
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        self.ingest_value(&v)
    }

    fn ingest_value(&mut self, v: &Value) -> Vec<StreamDelta> {
        match v["type"].as_str().unwrap_or("") {
            "message_start" => self.message_start(&v["message"]),
            "content_block_start" => self.block_start(v),
            "content_block_delta" => self.block_delta(v),
            "content_block_stop" => {
                self.block_stop(v["index"].as_u64().unwrap_or(0) as u32);
                Vec::new()
            }
            "message_delta" => self.message_delta(v),
            "error" => {
                let kind = v["error"]["type"].as_str().unwrap_or("error");
                let msg = v["error"]["message"].as_str().unwrap_or("");
                self.error = Some(format!("llm messages {kind}: {msg}"));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn message_start(&mut self, message: &Value) -> Vec<StreamDelta> {
        let usage = &message["usage"];
        self.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0);
        self.cache_read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
        self.cache_creation = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        if let Some(m) = message["model"].as_str() {
            if !m.is_empty() {
                self.model = m.to_string();
            }
        }
        self.usage_delta(0)
    }

    fn block_start(&mut self, v: &Value) -> Vec<StreamDelta> {
        let index = v["index"].as_u64().unwrap_or(0) as u32;
        let block = &v["content_block"];
        match block["type"].as_str() {
            Some("thinking") => {
                self.blocks.insert(
                    index,
                    BlockState {
                        kind: BlockType::Thinking,
                        text: String::new(),
                        tool_name: String::new(),
                        tool_id: String::new(),
                        args: String::new(),
                        thinking: block["thinking"].as_str().unwrap_or("").to_string(),
                    },
                );
            }
            Some("text") => {
                self.blocks.insert(
                    index,
                    BlockState {
                        kind: BlockType::Text,
                        text: block["text"].as_str().unwrap_or("").to_string(),
                        tool_name: String::new(),
                        tool_id: String::new(),
                        args: String::new(),
                        thinking: String::new(),
                    },
                );
            }
            Some("tool_use") => {
                let tool_index = self.next_tool;
                self.next_tool += 1;
                self.block_to_tool.insert(index, tool_index);
                self.blocks.insert(
                    index,
                    BlockState {
                        kind: BlockType::ToolUse,
                        text: String::new(),
                        tool_name: block["name"].as_str().unwrap_or("").to_string(),
                        tool_id: block["id"].as_str().unwrap_or("").to_string(),
                        args: String::new(),
                        thinking: String::new(),
                    },
                );
            }
            _ => {}
        }
        Vec::new()
    }

    fn block_delta(&mut self, v: &Value) -> Vec<StreamDelta> {
        let index = v["index"].as_u64().unwrap_or(0) as u32;
        let delta = &v["delta"];
        let Some(state) = self.blocks.get_mut(&index) else {
            return Vec::new();
        };
        match delta["type"].as_str() {
            Some("thinking_delta") => {
                let piece = delta["thinking"].as_str().unwrap_or("");
                if piece.is_empty() {
                    return Vec::new();
                }
                state.thinking.push_str(piece);
                vec![StreamDelta::Reasoning(piece.to_string())]
            }
            Some("text_delta") => {
                let piece = delta["text"].as_str().unwrap_or("");
                if piece.is_empty() {
                    return Vec::new();
                }
                state.text.push_str(piece);
                vec![StreamDelta::Text(piece.to_string())]
            }
            Some("input_json_delta") => {
                let piece = delta["partial_json"].as_str().unwrap_or("");
                state.args.push_str(piece);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn block_stop(&mut self, index: u32) {
        let Some(state) = self.blocks.remove(&index) else {
            return;
        };
        match state.kind {
            BlockType::Text => {
                if !state.text.is_empty() {
                    if !self.content.is_empty() {
                        self.content.push('\n');
                    }
                    self.content.push_str(&state.text);
                }
            }
            BlockType::Thinking => {
                self.reasoning.push_str(&state.thinking);
            }
            BlockType::ToolUse => {
                if !state.tool_name.is_empty() {
                    self.tool_calls.push(ToolCall {
                        id: if state.tool_id.is_empty() {
                            format!("call-{}", self.tool_calls.len())
                        } else {
                            state.tool_id
                        },
                        name: state.tool_name,
                        arguments: state.args,
                    });
                }
            }
        }
    }

    fn message_delta(&mut self, v: &Value) -> Vec<StreamDelta> {
        let usage = &v["usage"];
        if let Some(n) = usage["output_tokens"].as_u64() {
            self.output_tokens = n;
        }
        if let Some(n) = usage["input_tokens"].as_u64() {
            self.input_tokens = n;
        }
        if let Some(n) = usage["cache_read_input_tokens"].as_u64() {
            self.cache_read = n;
        }
        if let Some(n) = usage["cache_creation_input_tokens"].as_u64() {
            self.cache_creation = n;
        }
        self.usage_delta(self.output_tokens)
    }

    fn usage_delta(&self, completion: u64) -> Vec<StreamDelta> {
        let prompt = self
            .input_tokens
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_creation);
        if prompt == 0 && completion == 0 {
            return Vec::new();
        }
        vec![StreamDelta::Usage {
            tokens: TokenUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: prompt.saturating_add(completion),
                reasoning_tokens: 0,
                cached_prompt_tokens: self.cache_read,
                cache_creation_prompt_tokens: self.cache_creation,
            },
            official: true,
            model: self.model.clone(),
            cost_usd_ticks: None,
        }]
    }

    pub fn finish(mut self) -> LlmOutput {
        let remaining: Vec<u32> = self.blocks.keys().copied().collect();
        for index in remaining {
            self.block_stop(index);
        }
        if let Some(err) = self.error {
            return LlmOutput {
                text: err,
                ..LlmOutput::default()
            };
        }
        LlmOutput {
            text: self.content,
            reasoning: self.reasoning,
            tool_calls: self.tool_calls,
            ..LlmOutput::default()
        }
    }
}

pub fn parse_json(body: &str) -> LlmOutput {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return LlmOutput {
            text: body.to_string(),
            ..LlmOutput::default()
        };
    };
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    let Some(blocks) = v["content"].as_array() else {
        return LlmOutput {
            text: v["error"]["message"].as_str().unwrap_or(body).to_string(),
            ..LlmOutput::default()
        };
    };
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(s) = block["text"].as_str() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(s);
                }
            }
            Some("thinking") => {
                if let Some(s) = block["thinking"].as_str() {
                    reasoning.push_str(s);
                }
            }
            Some("tool_use") => {
                let name = block["name"].as_str().unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                let arguments = match &block["input"] {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                tool_calls.push(ToolCall {
                    id: block["id"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("call-{}", tool_calls.len())),
                    name: name.to_string(),
                    arguments,
                });
            }
            _ => {}
        }
    }
    LlmOutput {
        text,
        reasoning,
        tool_calls,
        ..LlmOutput::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolSpec;

    fn req(history: Vec<LogEvent>) -> PromptRequest {
        PromptRequest {
            system: "s".into(),
            history,
            tools: vec![],
        }
    }

    #[test]
    fn omits_prior_reasoning_and_groups_tool_results() {
        let request = req(vec![
            LogEvent::User("hi".into()),
            LogEvent::LlmStream(LlmOutput {
                text: "ok".into(),
                reasoning: "secret plan".into(),
                tool_calls: vec![
                    ToolCall {
                        id: "call/1".into(),
                        name: "bash".into(),
                        arguments: r#"{"x":1}"#.into(),
                    },
                    ToolCall {
                        id: "c2".into(),
                        name: "read".into(),
                        arguments: "{}".into(),
                    },
                ],
                ..LlmOutput::default()
            }),
            LogEvent::ToolExecute {
                id: "call/1".into(),
                name: "bash".into(),
                arguments: "{}".into(),
                content: "ok".into(),
            
                images: Vec::new(),
            },
            LogEvent::User("follow-up".into()),
        ]);
        let (system, msgs) = transcript(&request, &[], "claude");
        assert_eq!(system, "s");
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
        let blocks = msgs[1]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "call_1");
        assert!(blocks
            .iter()
            .all(|b| b["type"].as_str() != Some("thinking")));
        assert_eq!(msgs[2]["role"], "user");
        let results = msgs[2]["content"].as_array().unwrap();
        assert_eq!(results[0]["type"], "tool_result");
        assert_eq!(results[0]["tool_use_id"], "call_1");
        assert_eq!(results[1]["tool_use_id"], "c2");
        assert_eq!(results[1]["content"], INTERRUPTED_TOOL_RESULT);
        assert_eq!(msgs[3]["content"], "follow-up");
    }

    #[test]
    fn tools_use_input_schema() {
        let request = PromptRequest {
            system: String::new(),
            history: vec![],
            tools: vec![ToolSpec {
                name: "grep".into(),
                description: "search".into(),
                parameters_json: r#"{"type":"object"}"#.into(),
            }],
        };
        let body = body("claude", &request, &[], true, "high");
        assert_eq!(body["tools"][0]["name"], "grep");
        assert!(body["tools"][0].get("function").is_none());
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(body["thinking"]["type"], "adaptive");
    }

    #[test]
    fn sse_text_thinking_and_tool_use() {
        let mut acc = Acc::new("claude");
        acc.ingest_json(
            r#"{"type":"message_start","message":{"model":"claude","usage":{"input_tokens":8,"cache_read_input_tokens":2}}}"#,
        );
        acc.ingest_json(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        );
        let think = acc.ingest_json(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}"#,
        );
        assert_eq!(think, vec![StreamDelta::Reasoning("plan".into())]);
        acc.ingest_json(r#"{"type":"content_block_stop","index":0}"#);
        acc.ingest_json(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        );
        let text = acc.ingest_json(
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"hi"}}"#,
        );
        assert_eq!(text, vec![StreamDelta::Text("hi".into())]);
        acc.ingest_json(r#"{"type":"content_block_stop","index":1}"#);
        acc.ingest_json(
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tu1","name":"bash"}}"#,
        );
        acc.ingest_json(
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"a\":"}}"#,
        );
        acc.ingest_json(
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"1}"}}"#,
        );
        acc.ingest_json(r#"{"type":"content_block_stop","index":2}"#);
        let usage = acc.ingest_json(
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}"#,
        );
        assert!(usage.iter().any(|d| matches!(
            d,
            StreamDelta::Usage { tokens, official: true, .. }
                if tokens.prompt_tokens == 10 && tokens.cached_prompt_tokens == 2 && tokens.completion_tokens == 4
        )));
        let out = acc.finish();
        assert_eq!(out.text, "hi");
        assert_eq!(out.reasoning, "plan");
        assert_eq!(out.tool_calls[0].name, "bash");
        assert_eq!(out.tool_calls[0].arguments, r#"{"a":1}"#);
    }

    #[test]
    fn parse_json_drops_nothing_from_assistant_blocks() {
        let out = parse_json(
            r#"{
                "content":[
                    {"type":"thinking","thinking":"plan"},
                    {"type":"text","text":"answer"},
                    {"type":"tool_use","id":"1","name":"grep","input":{"p":"x"}}
                ]
            }"#,
        );
        assert_eq!(out.text, "answer");
        assert_eq!(out.reasoning, "plan");
        assert_eq!(out.tool_calls[0].name, "grep");
        assert!(out.tool_calls[0].arguments.contains("p"));
    }
}
