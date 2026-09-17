//! OpenAI Responses API (`POST /v1/responses`). Wire mapping copied from
//! Grok `conversation/responses.rs` + L2 `stream/responses.rs`, without
//! doom-loop / hosted tools / actor.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use cordis_base::stream_acc::StreamDelta;
use cordis_base::types::{
    LlmOutput, LogEvent, PromptRequest, ToolCall, UserImage, INTERRUPTED_TOOL_RESULT,
};
use cordis_base::usage::TokenUsage;

pub fn body(
    model: &str,
    request: &PromptRequest,
    user_images: &[Vec<UserImage>],
    params: &super::WireParams,
) -> Value {
    let mut body = json!({
        "model": model,
        "input": input_items(request, user_images, params.images),
        "stream": true,
    });
    if let Some(max) = params.max_output_tokens {
        body["max_output_tokens"] = json!(max);
    }
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|t| {
                    let parameters: Value = serde_json::from_str(&t.parameters_json)
                        .unwrap_or(json!({"type":"object"}));
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": parameters,
                    })
                })
                .collect(),
        );
    }
    match params.reasoning {
        // 没有推理档的模型：`reasoning` 整个字段都不发。
        super::Reasoning::Unsupported => {}
        super::Reasoning::Off => body["reasoning"] = json!({ "effort": "none" }),
        super::Reasoning::On if params.effort == "none" => {
            body["reasoning"] = json!({ "effort": "none" })
        }
        super::Reasoning::On if !params.effort.is_empty() => {
            body["reasoning"] = json!({
                "effort": params.effort,
                "summary": "concise",
            })
        }
        super::Reasoning::On => body["reasoning"] = json!({ "summary": "concise" }),
    }
    body
}

pub fn input_items(
    request: &PromptRequest,
    user_images: &[Vec<UserImage>],
    vision: bool,
) -> Vec<Value> {
    let mut out = Vec::new();
    if !request.system.is_empty() {
        out.push(easy_message("system", &request.system, &[]));
    }
    let mut user_i = 0usize;
    let mut pending: Vec<String> = Vec::new();
    for event in &request.history {
        match event {
            LogEvent::User(text) => {
                flush_unmatched(&mut out, &mut pending);
                let images = user_images.get(user_i).cloned().unwrap_or_default();
                user_i += 1;
                out.push(easy_message("user", text, &images));
            }
            LogEvent::SystemReminder(text) => {
                flush_unmatched(&mut out, &mut pending);
                out.push(easy_message("user", text, &[]));
            }
            LogEvent::LlmStream(llm) if !llm.tool_calls.is_empty() => {
                flush_unmatched(&mut out, &mut pending);
                out.extend(replayable_reasoning(llm));
                if !llm.text.is_empty() {
                    out.push(easy_message("assistant", &llm.text, &[]));
                }
                for c in &llm.tool_calls {
                    out.push(json!({
                        "type": "function_call",
                        "call_id": c.id,
                        "name": c.name,
                        "arguments": c.arguments,
                    }));
                }
                pending = llm.tool_calls.iter().map(|c| c.id.clone()).collect();
            }
            LogEvent::LlmStream(llm) if !llm.text.is_empty() => {
                flush_unmatched(&mut out, &mut pending);
                out.extend(replayable_reasoning(llm));
                out.push(easy_message("assistant", &llm.text, &[]));
            }
            LogEvent::ToolExecute {
                id,
                content,
                images,
                ..
            } => {
                if let Some(i) = pending.iter().position(|p| p == id) {
                    pending.remove(i);
                    out.extend(super::tool_images::responses_tool_output(
                        id, content, images, vision,
                    ));
                }
            }
            LogEvent::PreStep | LogEvent::Prompt(_) | LogEvent::LlmStream(_) => {}
        }
    }
    flush_unmatched(&mut out, &mut pending);
    out
}

/// 把上一轮的 reasoning item 原样放回 input，排在同一轮的 message /
/// function_call 之前——推理项是顶层兄弟节点，复现模型当时的顺序
/// （Grok `conversation_item_to_input_items` 的 `ConversationItem::Reasoning` 臂）。
///
/// 两处清洗照抄 Grok：`status` 是 output-only 字段，回传会被拒；`content[]` 的
/// 元素要带 `type: "reasoning_text"` 判别符，缺了同样 400。
fn replayable_reasoning(llm: &cordis_base::types::LlmOutput) -> Vec<Value> {
    llm.reasoning_items
        .iter()
        .filter(|item| item["type"] == "reasoning")
        .map(|item| {
            let mut item = item.clone();
            if let Some(obj) = item.as_object_mut() {
                obj.remove("status");
            }
            patch_reasoning_text_types(&mut item);
            item
        })
        .collect()
}

/// Grok `patch_reasoning_text_types`。
fn patch_reasoning_text_types(item: &mut Value) {
    let Some(content) = item.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    for part in content.iter_mut() {
        if let Some(obj) = part.as_object_mut() {
            obj.entry("type")
                .or_insert_with(|| Value::String("reasoning_text".into()));
        }
    }
}

fn flush_unmatched(out: &mut Vec<Value>, pending: &mut Vec<String>) {
    for id in pending.drain(..) {
        out.push(json!({
            "type": "function_call_output",
            "call_id": id,
            "output": INTERRUPTED_TOOL_RESULT,
        }));
    }
}

fn easy_message(role: &str, text: &str, images: &[UserImage]) -> Value {
    if images.is_empty() {
        return json!({
            "type": "message",
            "role": role,
            "content": text,
        });
    }
    use base64::Engine;
    let mut parts = Vec::new();
    if !text.trim().is_empty() {
        parts.push(json!({"type": "input_text", "text": text}));
    }
    for img in images {
        let b64 = base64::engine::general_purpose::STANDARD.encode(img.data.as_ref());
        parts.push(json!({
            "type": "input_image",
            "image_url": format!("data:{};base64,{b64}", img.mime),
            "detail": "auto",
        }));
    }
    if parts.is_empty() {
        json!({
            "type": "message",
            "role": role,
            "content": text,
        })
    } else {
        json!({
            "type": "message",
            "role": role,
            "content": parts,
        })
    }
}

/// L2 Responses SSE accumulator. Deltas drive the TUI; `response.completed`
/// output is the finish() snapshot (Grok `response_to_conversation_items`).
#[derive(Debug, Default)]
pub struct Acc {
    model: String,
    content: String,
    reasoning: String,
    /// tool_index → (id, name, arguments)
    tool_calls: BTreeMap<u32, (String, String, String)>,
    output_to_tool: BTreeMap<u32, u32>,
    next_tool: u32,
    completed: Option<LlmOutput>,
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
        let kind = v["type"].as_str().unwrap_or("");
        match kind {
            "response.output_text.delta" => self.text_delta(v["delta"].as_str().unwrap_or("")),
            "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                self.reasoning_delta(v["delta"].as_str().unwrap_or(""))
            }
            "response.output_item.added" => self.item_added(v),
            "response.function_call_arguments.delta" => self.args_delta(v),
            "response.completed" | "response.incomplete" => self.completed_event(&v["response"]),
            "response.failed" => {
                let msg = v["response"]["error"]["message"]
                    .as_str()
                    .or_else(|| v["response"]["error"]["code"].as_str())
                    .unwrap_or("response failed");
                self.error = Some(format!("llm responses: {msg}"));
                Vec::new()
            }
            "error" => {
                let msg = v["message"].as_str().unwrap_or("error");
                self.error = Some(format!("llm responses: {msg}"));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn text_delta(&mut self, delta: &str) -> Vec<StreamDelta> {
        if delta.is_empty() {
            return Vec::new();
        }
        self.content.push_str(delta);
        vec![StreamDelta::Text(delta.to_string())]
    }

    fn reasoning_delta(&mut self, delta: &str) -> Vec<StreamDelta> {
        if delta.is_empty() {
            return Vec::new();
        }
        self.reasoning.push_str(delta);
        vec![StreamDelta::Reasoning(delta.to_string())]
    }

    fn item_added(&mut self, v: &Value) -> Vec<StreamDelta> {
        let item = &v["item"];
        if item["type"].as_str() != Some("function_call") {
            return Vec::new();
        }
        let output_index = v["output_index"].as_u64().unwrap_or(0) as u32;
        let tool_index = self.next_tool;
        self.next_tool += 1;
        self.output_to_tool.insert(output_index, tool_index);
        let id = item["call_id"].as_str().unwrap_or("").to_string();
        let name = item["name"].as_str().unwrap_or("").to_string();
        self.tool_calls
            .insert(tool_index, (id, name, String::new()));
        Vec::new()
    }

    fn args_delta(&mut self, v: &Value) -> Vec<StreamDelta> {
        let output_index = v["output_index"].as_u64().unwrap_or(0) as u32;
        let Some(&tool_index) = self.output_to_tool.get(&output_index) else {
            return Vec::new();
        };
        let delta = v["delta"].as_str().unwrap_or("");
        if delta.is_empty() {
            return Vec::new();
        }
        if let Some(entry) = self.tool_calls.get_mut(&tool_index) {
            entry.2.push_str(delta);
        }
        Vec::new()
    }

    fn completed_event(&mut self, response: &Value) -> Vec<StreamDelta> {
        // `response.completed` 带完整的 output 数组，reasoning item 的原件（id /
        // summary / content）只在这里拿得到；纯增量流拼不出合法的 item。
        let mut out = output_from_response(response);
        if out.reasoning.is_empty() {
            out.reasoning = self.reasoning.clone();
        }
        self.completed = Some(out);
        usage_from_response(response, &self.model)
            .into_iter()
            .collect()
    }

    pub fn finish(self) -> LlmOutput {
        if let Some(err) = self.error {
            return LlmOutput {
                error: Some(err),
                ..LlmOutput::default()
            };
        }
        if let Some(completed) = self.completed {
            return completed;
        }
        stream_snapshot(&self.content, &self.reasoning, &self.tool_calls)
    }
}

pub fn parse_json(body: &str) -> LlmOutput {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return LlmOutput {
            text: body.to_string(),
            ..LlmOutput::default()
        };
    };
    let response = if v.get("output").is_some() {
        &v
    } else {
        &v["response"]
    };
    output_from_response(response)
}

fn output_from_response(response: &Value) -> LlmOutput {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut reasoning_items = Vec::new();
    let mut tool_calls = Vec::new();
    let Some(items) = response["output"].as_array() else {
        return LlmOutput::default();
    };
    for item in items {
        match item["type"].as_str() {
            Some("message") => {
                append_message_text(&mut text, item);
            }
            Some("function_call") => {
                let name = item["name"].as_str().unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                let arguments = match &item["arguments"] {
                    Value::String(s) => s.clone(),
                    other if !other.is_null() => other.to_string(),
                    _ => String::new(),
                };
                tool_calls.push(ToolCall {
                    id: item["call_id"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("call-{}", tool_calls.len())),
                    name: name.to_string(),
                    arguments,
                });
            }
            Some("reasoning") => {
                append_reasoning(&mut reasoning, item);
                reasoning_items.push(item.clone());
            }
            _ => {}
        }
    }
    LlmOutput {
        text,
        reasoning,
        tool_calls,
        reasoning_items,
        ..LlmOutput::default()
    }
}

fn append_message_text(text: &mut String, item: &Value) {
    let Some(content) = item["content"].as_array() else {
        if let Some(s) = item["content"].as_str() {
            push_piece(text, s);
        }
        return;
    };
    for part in content {
        if let Some(s) = part["text"].as_str() {
            push_piece(text, s);
        }
    }
}

/// 拍平成给人看的文本。分段之间要换行——`push_str` 会把 summary 和 content
/// 粘成一坨（"planstep one"），思考卡里读着像乱码。
fn append_reasoning(reasoning: &mut String, item: &Value) {
    if let Some(parts) = item["summary"].as_array() {
        for part in parts {
            if let Some(s) = part["text"].as_str() {
                push_piece(reasoning, s);
            }
        }
    }
    if let Some(parts) = item["content"].as_array() {
        for part in parts {
            if let Some(s) = part["text"].as_str() {
                push_piece(reasoning, s);
            }
        }
    }
}

fn push_piece(buf: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !buf.is_empty() {
        buf.push('\n');
    }
    buf.push_str(piece);
}

fn usage_from_response(response: &Value, model: &str) -> Option<StreamDelta> {
    let usage = &response["usage"];
    if usage.is_null() {
        return None;
    }
    let prompt = usage["input_tokens"].as_u64().unwrap_or(0);
    let completion = usage["output_tokens"].as_u64().unwrap_or(0);
    if prompt == 0 && completion == 0 {
        return None;
    }
    let cached = usage["input_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0);
    let reasoning = usage["output_tokens_details"]["reasoning_tokens"]
        .as_u64()
        .unwrap_or(0);
    let total = usage["total_tokens"]
        .as_u64()
        .unwrap_or(prompt.saturating_add(completion));
    Some(StreamDelta::Usage {
        tokens: TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: total,
            reasoning_tokens: reasoning,
            cached_prompt_tokens: cached,
            cache_creation_prompt_tokens: 0,
        },
        official: true,
        model: model.to_string(),
        cost_usd_ticks: None,
    })
}

fn stream_snapshot(
    content: &str,
    reasoning: &str,
    tool_calls: &BTreeMap<u32, (String, String, String)>,
) -> LlmOutput {
    let tool_calls = tool_calls
        .values()
        .filter(|(_, name, _)| !name.is_empty())
        .map(|(id, name, arguments)| ToolCall {
            id: if id.is_empty() {
                "call-0".into()
            } else {
                id.clone()
            },
            name: name.clone(),
            arguments: arguments.clone(),
        })
        .collect();
    LlmOutput {
        text: content.to_string(),
        reasoning: reasoning.to_string(),
        tool_calls,
        ..LlmOutput::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 采样失败详情不进 Responses 的 `input`（与 messages / chat 同一条承诺）。
    #[test]
    fn llm_error_never_reaches_the_wire() {
        let request = PromptRequest {
            system: "s".into(),
            history: vec![
                LogEvent::User("hi".into()),
                LogEvent::LlmStream(LlmOutput {
                    error: Some("[连接失败] connection reset by peer".into()),
                    ..LlmOutput::default()
                }),
            ],
            tools: vec![],
        };
        let json = serde_json::to_string(&body("m", &request, &[], &params_on("low"))).unwrap();
        assert!(!json.contains("connection reset"), "{json}");
    }

    /// SSE 流中途的 `response.failed` 与 `error` 事件不能写进 `text`：进了
    /// `text` 就会被下轮回放成「模型自己说过 llm responses: …」。
    #[test]
    fn sse_midstream_error_goes_to_error_not_text() {
        let mut acc = Acc::new("m");
        acc.ingest_json(
            r#"{"type":"response.failed","response":{"error":{"message":"rate limit exceeded"}}}"#,
        );
        let out = acc.finish();
        assert!(out.text.is_empty(), "流内错误漏进 text：{}", out.text);
        assert_eq!(
            out.error.as_deref(),
            Some("llm responses: rate limit exceeded")
        );

        let mut acc = Acc::new("m");
        acc.ingest_json(r#"{"type":"error","message":"upstream panic"}"#);
        let out = acc.finish();
        assert!(out.text.is_empty(), "流内错误漏进 text：{}", out.text);
        assert_eq!(out.error.as_deref(), Some("llm responses: upstream panic"));
    }

    fn params_on(effort: &str) -> crate::http::WireParams {
        crate::http::WireParams {
            reasoning: crate::http::Reasoning::On,
            effort: effort.into(),
            max_output_tokens: None,
            images: true,
        }
    }

    fn req(history: Vec<LogEvent>) -> PromptRequest {
        PromptRequest {
            system: "s".into(),
            history,
            tools: vec![],
        }
    }

    #[test]
    fn input_inserts_function_call_output_stubs() {
        let request = req(vec![
            LogEvent::User("hi".into()),
            LogEvent::LlmStream(LlmOutput {
                tool_calls: vec![
                    ToolCall {
                        id: "c1".into(),
                        name: "bash".into(),
                        arguments: "{}".into(),
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
                id: "c1".into(),
                name: "bash".into(),
                arguments: "{}".into(),
                content: "ok".into(),

                images: Vec::new(),
            },
            LogEvent::User("follow-up".into()),
        ]);
        let items = input_items(&request, &[], true);
        assert_eq!(items[0]["role"], "system");
        assert_eq!(items[1]["role"], "user");
        assert_eq!(items[2]["type"], "function_call");
        assert_eq!(items[2]["call_id"], "c1");
        assert_eq!(items[3]["type"], "function_call");
        assert_eq!(items[4]["type"], "function_call_output");
        assert_eq!(items[4]["call_id"], "c1");
        assert_eq!(items[5]["type"], "function_call_output");
        assert_eq!(items[5]["call_id"], "c2");
        assert_eq!(items[5]["output"], INTERRUPTED_TOOL_RESULT);
        assert_eq!(items[6]["role"], "user");
        assert_eq!(items[6]["content"], "follow-up");
    }

    #[test]
    fn tools_are_flat_function_entries() {
        let request = PromptRequest {
            system: String::new(),
            history: vec![],
            tools: vec![cordis_base::types::ToolSpec {
                name: "grep".into(),
                description: "search".into(),
                parameters_json: r#"{"type":"object"}"#.into(),
            }],
        };
        let body = body("gpt", &request, &[], &params_on("high"));
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "grep");
        assert!(body["tools"][0].get("function").is_none());
        assert_eq!(body["reasoning"]["effort"], "high");
    }

    #[test]
    fn sse_text_then_function_call_then_completed() {
        let mut acc = Acc::new("gpt");
        let d = acc.ingest_json(r#"{"type":"response.output_text.delta","delta":"hi"}"#);
        assert_eq!(d, vec![StreamDelta::Text("hi".into())]);
        acc.ingest_json(
            r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","call_id":"call_xyz","name":"do_thing"}}"#,
        );
        acc.ingest_json(
            r#"{"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"x\":"}"#,
        );
        acc.ingest_json(
            r#"{"type":"response.function_call_arguments.delta","output_index":1,"delta":"1}"}"#,
        );
        let usage = acc.ingest_json(
            r#"{
                "type":"response.completed",
                "response":{
                    "output":[
                        {"type":"message","content":[{"type":"output_text","text":"hi"}]},
                        {"type":"function_call","call_id":"call_xyz","name":"do_thing","arguments":"{\"x\":1}"}
                    ],
                    "usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13}
                }
            }"#,
        );
        assert!(usage.iter().any(|d| matches!(
            d,
            StreamDelta::Usage { tokens, official: true, .. }
                if tokens.prompt_tokens == 10 && tokens.completion_tokens == 3
        )));
        let out = acc.finish();
        assert_eq!(out.text, "hi");
        assert_eq!(out.tool_calls[0].id, "call_xyz");
        assert_eq!(out.tool_calls[0].arguments, r#"{"x":1}"#);
    }

    #[test]
    fn args_delta_without_added_is_dropped() {
        let mut acc = Acc::new("gpt");
        acc.ingest_json(
            r#"{"type":"response.function_call_arguments.delta","output_index":7,"delta":"{}"}"#,
        );
        let out = acc.finish();
        assert!(out.tool_calls.is_empty());
    }

    #[test]
    fn responses_body_reasoning_is_three_state() {
        let request = req(vec![LogEvent::User("hi".into())]);
        let params = crate::http::WireParams {
            reasoning: crate::http::Reasoning::Unsupported,
            ..params_on("")
        };
        let out = body("gpt", &request, &[], &params);
        assert!(out.get("reasoning").is_none(), "{out}");
        assert!(out.get("max_output_tokens").is_none(), "{out}");

        let params = crate::http::WireParams {
            reasoning: crate::http::Reasoning::Off,
            ..params_on("")
        };
        assert_eq!(
            body("gpt", &request, &[], &params)["reasoning"]["effort"],
            "none"
        );

        let out = body("gpt", &request, &[], &params_on("high"));
        assert_eq!(out["reasoning"]["effort"], "high");
        assert_eq!(out["reasoning"]["summary"], "concise");

        let params = crate::http::WireParams {
            max_output_tokens: Some(1024),
            ..params_on("")
        };
        assert_eq!(
            body("gpt", &request, &[], &params)["max_output_tokens"],
            1024
        );
    }

    #[test]
    fn parse_json_completed_body() {
        let out = parse_json(
            r#"{
                "output":[
                    {"type":"reasoning","summary":[{"type":"summary_text","text":"plan"}]},
                    {"type":"message","content":[{"text":"answer"}]}
                ]
            }"#,
        );
        assert_eq!(out.text, "answer");
        assert_eq!(out.reasoning, "plan");
    }

    /// 抓到的是原件（含 id），不是拍平文本。
    #[test]
    fn completed_response_keeps_raw_reasoning_items() {
        let out = parse_json(
            r#"{
                "output":[
                    {"type":"reasoning","id":"rs_1","status":"completed",
                     "summary":[{"type":"summary_text","text":"plan"}],
                     "content":[{"text":"step one"}]},
                    {"type":"function_call","call_id":"c1","name":"bash","arguments":"{}"}
                ]
            }"#,
        );
        assert_eq!(out.reasoning_items.len(), 1);
        assert_eq!(out.reasoning_items[0]["id"], "rs_1");
        assert_eq!(out.reasoning, "plan\nstep one");
    }

    /// 回放：推理项排在同轮 message / function_call 之前，`status` 去掉，
    /// `content[]` 补上 `reasoning_text` 判别符。
    #[test]
    fn reasoning_items_replay_before_the_turn_they_belong_to() {
        let llm = LlmOutput {
            text: "answer".into(),
            reasoning: "plan".into(),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: "{}".into(),
            }],
            reasoning_items: vec![json!({
                "type": "reasoning",
                "id": "rs_1",
                "status": "completed",
                "summary": [{"type": "summary_text", "text": "plan"}],
                "content": [{"text": "step one"}],
            })],
            ..LlmOutput::default()
        };
        let request = req(vec![
            LogEvent::User("go".into()),
            LogEvent::LlmStream(llm),
            LogEvent::ToolExecute {
                id: "c1".into(),
                name: "bash".into(),
                arguments: "{}".into(),
                content: "ok".into(),
                images: Vec::new(),
            },
        ]);
        let items = input_items(&request, &[], true);
        let kinds: Vec<&str> = items
            .iter()
            .map(|i| {
                i["type"]
                    .as_str()
                    .unwrap_or_else(|| i["role"].as_str().unwrap_or("?"))
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "message",   // system
                "message",   // user
                "reasoning", // 推理项在前
                "message",   // assistant 文本
                "function_call",
                "function_call_output",
            ],
            "{items:#?}"
        );
        let reasoning = items.iter().find(|i| i["type"] == "reasoning").unwrap();
        assert_eq!(reasoning["id"], "rs_1");
        assert!(reasoning.get("status").is_none(), "status 是 output-only");
        assert_eq!(reasoning["content"][0]["type"], "reasoning_text");
        assert_eq!(reasoning["summary"][0]["type"], "summary_text");
    }

    /// 没抓到原件（中断流、压缩过的历史、旧会话文件）就什么都不放回去。
    #[test]
    fn no_items_means_no_replay() {
        let request = req(vec![
            LogEvent::User("go".into()),
            LogEvent::LlmStream(LlmOutput {
                text: "answer".into(),
                reasoning: "plan".into(),
                ..LlmOutput::default()
            }),
        ]);
        let items = input_items(&request, &[], true);
        assert!(items.iter().all(|i| i["type"] != "reasoning"), "{items:#?}");
    }
}
