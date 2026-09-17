//! Anthropic Messages API (`POST /v1/messages`). Wire mapping copied from
//! Grok `conversation/messages.rs` + L2 `stream/messages.rs`. Thinking from
//! prior turns is omitted: Dock has no encrypted `signature` to replay.
//!
//! 前缀缓存要显式 `cache_control` 断点（不像 chat/completions 与 Responses 由
//! 上游自动做），断点布局照抄 Grok `apply_cache_breakpoints`。

use std::collections::BTreeMap;

use serde_json::{json, Value};

use cordis_base::stream_acc::StreamDelta;
use cordis_base::types::{
    LlmOutput, LogEvent, PromptRequest, ToolCall, UserImage, INTERRUPTED_TOOL_RESULT,
};
use cordis_base::usage::TokenUsage;

/// Anthropic requires `max_tokens`. Grok fills sampler defaults; Dock uses
/// this when config has no per-request cap.
const DEFAULT_MAX_TOKENS: u32 = 16_384;

pub fn body(
    model: &str,
    request: &PromptRequest,
    user_images: &[Vec<UserImage>],
    params: &super::WireParams,
    prompt_cache: bool,
) -> Value {
    let (system, mut messages) = transcript(request, user_images, params.images);
    let mut system_blocks: Vec<Value> = if system.is_empty() {
        Vec::new()
    } else {
        vec![json!({"type": "text", "text": system})]
    };
    if prompt_cache {
        apply_cache_breakpoints(&mut system_blocks, &mut messages);
    }
    let mut body = json!({
        "model": model,
        "messages": messages,
        // Anthropic 这项必填，所以这里是唯一一个"没配就兜底"的参数。
        "max_tokens": params.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "stream": true,
    });
    if !system.is_empty() {
        // 关掉缓存时保持老形状（纯字符串），给不认块数组的代理留条活路。
        body["system"] = if prompt_cache {
            Value::Array(system_blocks)
        } else {
            json!(system)
        };
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
    // Off / Unsupported 都是不发 `thinking`：Anthropic 没有"显式关闭"的形状，
    // 不发就是不想。
    if params.reasoning == super::Reasoning::On && params.effort != "none" {
        body["thinking"] = json!({
            "type": "adaptive",
            "display": "summarized",
        });
    }
    body
}

fn ephemeral() -> Value {
    json!({"type": "ephemeral"})
}

/// Grok `mark_message_cache_breakpoint`：标最后一个带得动断点的块，跳过 API
/// 不接受断点的 thinking。纯字符串 content 挂不住断点，先升成块数组。
fn mark_message_cache_breakpoint(msg: &mut Value) -> bool {
    match msg.get_mut("content") {
        Some(Value::Array(blocks)) => {
            for block in blocks.iter_mut().rev() {
                let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(kind, "thinking" | "redacted_thinking") {
                    continue;
                }
                let Some(obj) = block.as_object_mut() else {
                    continue;
                };
                obj.insert("cache_control".into(), ephemeral());
                return true;
            }
            false
        }
        Some(content @ Value::String(_)) => {
            let text = std::mem::take(content);
            *content = json!([{ "type": "text", "text": text, "cache_control": ephemeral() }]);
            true
        }
        _ => false,
    }
}

/// Grok `apply_cache_breakpoints`。缓存条目只在断点处写，所以只标 system 会让
/// 整段对话都不进缓存。第三个断点覆盖「一轮追加的块数超过 API 20 块回看」的
/// 情况。第四个槽留空：网关自己开自动缓存时会占用它，五个会被直接拒。
///
/// **断点会移动，因而每次请求都会改写已发出前缀里的 37 个字节**
/// （`,"cache_control":{"type":"ephemeral"}` 从上一次带标记的那条消息上消失）。
/// 按 Anthropic 的语义这无害：`cache_control` 是元数据，前缀匹配只看内容，滚动
/// 断点正是官方推荐的用法。
///
/// 但**按整份请求体前缀做匹配的兼容层会因此整段失配**。这类端点自己就做自动前缀
/// 缓存，正确做法是给它配 `[model.<id>].prompt_cache = false`：dock 一个标记都不
/// 发，请求体变成纯追加，命中交给上游自己做。4 个断点的上限决定了「标记只增不减」
/// 做不到，所以这里不为那类端点改布局——开关比猜上游可靠。
fn apply_cache_breakpoints(system_blocks: &mut [Value], messages: &mut [Value]) {
    if let Some(obj) = system_blocks.last_mut().and_then(Value::as_object_mut) {
        obj.insert("cache_control".into(), ephemeral());
    }
    let tip = (0..messages.len())
        .rev()
        .find(|&i| mark_message_cache_breakpoint(&mut messages[i]));
    // 上一次请求收尾的位置。一轮可能连着追加好几条 user 消息，所以整段跳过尾
    // 部的 user 串，而不是只退一格。
    let Some(tip) = tip else { return };
    let prev = messages[..tip]
        .iter()
        .rposition(|m| m["role"] == "assistant")
        .and_then(|assistant| {
            messages[..assistant]
                .iter()
                .rposition(|m| m["role"] == "user")
        });
    if let Some(prev) = prev {
        mark_message_cache_breakpoint(&mut messages[prev]);
    }
}

pub fn transcript(
    request: &PromptRequest,
    user_images: &[Vec<UserImage>],
    vision: bool,
) -> (String, Vec<Value>) {
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
                        vision,
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
                error: Some(err),
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
    use cordis_base::types::ToolSpec;

    fn req(history: Vec<LogEvent>) -> PromptRequest {
        PromptRequest {
            system: "s".into(),
            history,
            tools: vec![],
        }
    }

    /// 采样失败详情**绝不能**进 wire：它只是 harness 说给人听的一句话，
    /// 回放给模型就变成「模型自己说过 LLM 请求失败」，而且每轮都要再付一次
    /// token。门槛是 `text` 非空或有 tool_call——只写 `error` 的那条记录整条
    /// 不出现在请求里。
    #[test]
    fn llm_error_never_reaches_the_wire() {
        let detail = "[连接失败] error sending request ← connection reset by peer";
        let request = req(vec![
            LogEvent::User("hi".into()),
            LogEvent::LlmStream(LlmOutput {
                error: Some(detail.into()),
                ..LlmOutput::default()
            }),
            LogEvent::User("再试一次".into()),
        ]);
        let body = body("m", &request, &[], &Default::default(), true);
        let json = serde_json::to_string(&body).unwrap();
        assert!(
            !json.contains("connection reset"),
            "失败详情漏进请求体了：{json}"
        );
        assert!(!json.contains("请求失败"), "{json}");
        assert!(json.contains("再试一次"), "正常消息仍要在：{json}");
    }

    /// SSE 流中途的 provider 错误事件也**不能**写进 `text`——它与传输层失败
    /// 同类，是 harness 说给人听的，进了 `text` 就会被下轮回放成「模型自己
    /// 说过 llm messages …」。
    #[test]
    fn sse_midstream_error_goes_to_error_not_text() {
        let mut acc = Acc::new("m");
        acc.ingest_json(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"server overloaded"}}"#,
        );
        let out = acc.finish();
        assert!(out.text.is_empty(), "流内错误漏进 text：{}", out.text);
        assert_eq!(
            out.error.as_deref(),
            Some("llm messages overloaded_error: server overloaded")
        );
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
        let (system, msgs) = transcript(&request, &[], true);
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
        let body = body("claude", &request, &[], &params_on("high"), true);
        assert_eq!(body["tools"][0]["name"], "grep");
        assert!(body["tools"][0].get("function").is_none());
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(body["thinking"]["type"], "adaptive");
    }

    /// 思考开着、给定强度、支持读图的一组参数（测试默认）。
    fn params_on(effort: &str) -> crate::http::WireParams {
        crate::http::WireParams {
            reasoning: crate::http::Reasoning::On,
            effort: effort.into(),
            max_output_tokens: None,
            images: true,
        }
    }

    fn marker_on_last_block(msg: &Value) -> Option<&str> {
        msg["content"]
            .as_array()?
            .last()?
            .get("cache_control")?
            .get("type")?
            .as_str()
    }

    fn count_markers(body: &Value) -> usize {
        fn walk(v: &Value, n: &mut usize) {
            match v {
                Value::Object(map) => {
                    if map.contains_key("cache_control") {
                        *n += 1;
                    }
                    map.values().for_each(|v| walk(v, n));
                }
                Value::Array(items) => items.iter().for_each(|v| walk(v, n)),
                _ => {}
            }
        }
        let mut n = 0;
        walk(body, &mut n);
        n
    }

    /// `prompt_cache = false` 时请求体必须是**纯追加**：一条已经发出去过的消息
    /// 都不许再变。
    ///
    /// 这正是这个开关存在的意义。开着断点时布局会滚动，每次请求都从上一次带标记
    /// 的那条消息上抹掉 37 字节的 `,"cache_control":{"type":"ephemeral"}`——真
    /// Anthropic 不在乎（元数据不参与前缀匹配），但按请求体前缀匹配的兼容层会整段
    /// 失配。实测日志里每次请求都是 `同前 N-3/N ⚠ · 旧 2940B 新 2903B`，读回来的
    /// 量从两万多掉到 8,512（= system+tools 那一段）。关掉之后这条路必须干净。
    #[test]
    fn prompt_cache_off_keeps_the_body_append_only() {
        let mut history = vec![LogEvent::User("first".into())];
        let serialize = |history: &[LogEvent], prompt_cache: bool| -> Vec<String> {
            let request = PromptRequest {
                system: "s".into(),
                history: history.to_vec(),
                tools: vec![],
            };
            let body = body("claude", &request, &[], &params_on("high"), prompt_cache);
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m.to_string())
                .collect()
        };
        let common =
            |a: &[String], b: &[String]| a.iter().zip(b).take_while(|(x, y)| x == y).count();

        let mut off = serialize(&history, false);
        let mut on = serialize(&history, true);
        let mut rewrites_with_breakpoints = 0usize;
        for round in 0..12 {
            history.push(LogEvent::LlmStream(LlmOutput {
                text: format!("answer {round}"),
                ..LlmOutput::default()
            }));
            history.push(LogEvent::User(format!("ask {round}")));

            let now_off = serialize(&history, false);
            let same = common(&off, &now_off);
            assert_eq!(
                same,
                off.len(),
                "第 {round} 轮改写了已发出的消息：旧 {:?} 新 {:?}",
                off.get(same),
                now_off.get(same),
            );
            off = now_off;

            // 开着断点时会改写——这不是缺陷，是 Anthropic 语义下的正常滚动。
            // 钉住它，免得有人把上面那条断言当成「两种配置都成立」。
            let now_on = serialize(&history, true);
            if common(&on, &now_on) < on.len() {
                rewrites_with_breakpoints += 1;
            }
            on = now_on;
        }
        assert!(
            rewrites_with_breakpoints > 0,
            "断点开着却从不移动？那这个开关就没必要存在了"
        );
    }

    /// 两轮对话：system + 末尾 tip + 上一轮收尾处，共三个断点，第四槽留空。
    #[test]
    fn cache_breakpoints_mark_system_tip_and_previous_turn() {
        let request = PromptRequest {
            system: "s".into(),
            history: vec![
                LogEvent::User("first".into()),
                LogEvent::LlmStream(LlmOutput {
                    text: "answer one".into(),
                    ..LlmOutput::default()
                }),
                LogEvent::User("second".into()),
                LogEvent::LlmStream(LlmOutput {
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: "bash".into(),
                        arguments: "{}".into(),
                    }],
                    ..LlmOutput::default()
                }),
                LogEvent::ToolExecute {
                    id: "c1".into(),
                    name: "bash".into(),
                    arguments: "{}".into(),
                    content: "ok".into(),
                    images: Vec::new(),
                },
            ],
            tools: vec![],
        };
        let body = body("claude", &request, &[], &params_on("high"), true);
        assert_eq!(body["system"][0]["type"], "text");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 5, "{body:#}");
        // tip = 最后一条（工具结果那条 user）。
        assert_eq!(
            marker_on_last_block(&msgs[4]),
            Some("ephemeral"),
            "{body:#}"
        );
        // 上一轮收尾：从 tip 往回找最后一个 assistant（3），再往回找 user（2）。
        // 这个位置在整轮工具循环里不动，所以每一步都能命中它。
        assert_eq!(
            marker_on_last_block(&msgs[2]),
            Some("ephemeral"),
            "{body:#}"
        );
        assert_eq!(msgs[0]["content"], "first", "更早的轮次不打断点");
        assert!(msgs[1]["content"][0].get("cache_control").is_none());
        assert_eq!(count_markers(&body), 3, "第四个槽留给网关：{body:#}");
    }

    /// 纯字符串 content 挂不住断点，要先升成块数组（Grok 同款）。
    #[test]
    fn cache_breakpoint_promotes_plain_text_content() {
        let request = PromptRequest {
            system: String::new(),
            history: vec![LogEvent::User("only".into())],
            tools: vec![],
        };
        let body = body("claude", &request, &[], &params_on("high"), true);
        let msg = &body["messages"][0];
        assert_eq!(msg["content"][0]["type"], "text");
        assert_eq!(msg["content"][0]["text"], "only");
        assert_eq!(marker_on_last_block(msg), Some("ephemeral"), "{body:#}");
    }

    /// API 不接受 thinking 上的断点：往前找下一个能带的块。
    #[test]
    fn cache_breakpoint_skips_thinking_blocks() {
        let mut msg = json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "answer"},
                {"type": "thinking", "thinking": "plan"},
            ],
        });
        assert!(mark_message_cache_breakpoint(&mut msg));
        assert!(msg["content"][1].get("cache_control").is_none());
        assert_eq!(msg["content"][0]["cache_control"]["type"], "ephemeral");
    }

    /// 关掉开关就回到老形状：system 还是纯字符串，全身没有 cache_control。
    #[test]
    fn prompt_cache_off_keeps_the_plain_shape() {
        let request = PromptRequest {
            system: "s".into(),
            history: vec![LogEvent::User("hi".into())],
            tools: vec![],
        };
        let body = body("claude", &request, &[], &params_on("high"), false);
        assert_eq!(body["system"], "s");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert_eq!(count_markers(&body), 0, "{body:#}");
    }

    /// max_tokens 是 Anthropic 必填项：没配才兜底，配了就用配的。
    /// thinking 只在"支持且开着"时发——Anthropic 没有"显式关闭"的形状。
    #[test]
    fn messages_body_honours_model_config() {
        let request = PromptRequest {
            system: String::new(),
            history: vec![LogEvent::User("hi".into())],
            tools: vec![],
        };
        let out = body("claude", &request, &[], &params_on(""), true);
        assert_eq!(out["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(out["thinking"]["type"], "adaptive");

        let params = crate::http::WireParams {
            max_output_tokens: Some(2048),
            ..params_on("")
        };
        assert_eq!(
            body("claude", &request, &[], &params, true)["max_tokens"],
            2048
        );

        for state in [
            crate::http::Reasoning::Off,
            crate::http::Reasoning::Unsupported,
        ] {
            let params = crate::http::WireParams {
                reasoning: state,
                ..params_on("")
            };
            let out = body("claude", &request, &[], &params, true);
            assert!(out.get("thinking").is_none(), "{out}");
        }
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
