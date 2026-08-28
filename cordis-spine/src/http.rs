//! OpenAI-compatible `chat/completions`. Live-lookup `"settings"` for model
//! and `"turn"` for cancel. Streaming copied from Grok sampler
//! `chat_completion_stream` (SSE `data:` → `ChatCompletionChunk` deltas).

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::chat_chunk::ChatCompletionChunk;
use crate::config;
use crate::llm::Sampler;
use crate::names::{SESSIONS, SETTINGS, TURN};
use crate::runtime::BoxFuture;
use crate::session::Sessions;
use crate::settings::AppSettings;
use crate::stream_acc::{take_sse_data, ChatStreamAcc, StreamDelta};
use crate::turn::TurnControl;
use crate::types::{LlmOutput, LogEvent, PromptRequest, ToolCall, UserImage};

use cordis::Context;

pub struct HttpSampler {
    pub ctx: Context,
    pub api_key: String,
    pub api_base: String,
    pub fallback_model: String,
}

impl Sampler for HttpSampler {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        Box::pin(async move { sample_http(self, request, on_delta).await })
    }
}

async fn sample_http(
    sampler: &HttpSampler,
    request: PromptRequest,
    mut on_delta: Box<dyn FnMut(StreamDelta) + Send + '_>,
) -> LlmOutput {
    let model = sampler
        .ctx
        .get::<AppSettings>(SETTINGS)
        .map(|s| s.model())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| sampler.fallback_model.clone());
    let effort = sampler
        .ctx
        .get::<AppSettings>(SETTINGS)
        .map(|s| s.effort())
        .unwrap_or_default();
    let (api_base, api_key) = resolve_endpoint(sampler, &model);
    if let Some(sessions) = sampler.ctx.get::<Sessions>(SESSIONS) {
        let window = config::lookup_model(&model)
            .and_then(|m| m.context_window)
            .unwrap_or(128_000);
        sessions.set_window(window);
    }
    let prompt_est = estimate_prompt_tokens(&request);
    on_delta(StreamDelta::Usage {
        prompt: prompt_est,
        completion: 0,
    });
    let url = format!("{}/chat/completions", api_base.trim_end_matches('/'));
    let user_images = sampler
        .ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.user_images())
        .unwrap_or_default();
    let mut body = json!({
        "model": model,
        "messages": messages(&request, &user_images),
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|t| {
                    let parameters: Value =
                        serde_json::from_str(&t.parameters_json).unwrap_or(json!({"type":"object"}));
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": parameters,
                        }
                    })
                })
                .collect(),
        );
    }
    if !effort.is_empty() && effort != "medium" {
        body["reasoning_effort"] = json!(effort);
    }
    let client = reqwest::Client::new();
    let cancel = sampler.ctx.get::<TurnControl>(TURN).map(|t| t.token());
    let mut response = None;
    for attempt in 0..3u32 {
        let send = client
            .post(&url)
            .bearer_auth(&api_key)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&body)
            .send();
        let got = if let Some(ref cancel) = cancel {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    return LlmOutput {
                        text: "cancelled".into(),
                        ..LlmOutput::default()
                    };
                }
                response = send => response,
            }
        } else {
            send.await
        };
        match got {
            Err(e) => {
                return LlmOutput {
                    text: format!("llm request failed: {e}"),
                    ..LlmOutput::default()
                };
            }
            Ok(r) if r.status().as_u16() == 503 && attempt < 2 => {
                let wait = std::time::Duration::from_millis(800 * (attempt as u64 + 1));
                if let Some(ref cancel) = cancel {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            return LlmOutput {
                                text: "cancelled".into(),
                                ..LlmOutput::default()
                            };
                        }
                        _ = tokio::time::sleep(wait) => {}
                    }
                } else {
                    tokio::time::sleep(wait).await;
                }
            }
            Ok(r) => {
                response = Some(r);
                break;
            }
        }
    }
    let Some(response) = response else {
        return LlmOutput {
            text: "llm HTTP 503: endpoint busy, retry later".into(),
            ..LlmOutput::default()
        };
    };
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return LlmOutput {
            text: format!("llm HTTP {status}: {text}"),
            ..LlmOutput::default()
        };
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.contains("event-stream") && content_type.contains("json") {
        let text = match response.text().await {
            Ok(t) => t,
            Err(e) => {
                return LlmOutput {
                    text: format!("llm body failed: {e}"),
                    ..LlmOutput::default()
                };
            }
        };
        let output = parse_chat_completion(&text);
        if !output.reasoning.is_empty() {
            on_delta(StreamDelta::Reasoning(output.reasoning.clone()));
        }
        if !output.text.is_empty() {
            on_delta(StreamDelta::Text(output.text.clone()));
        }
        return output;
    }

    let cancel = sampler.ctx.get::<TurnControl>(TURN).map(|t| t.token());
    let mut acc = ChatStreamAcc::default();
    let mut buf = Vec::new();
    let mut first = true;
    let mut stream = response.bytes_stream();
    const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
    loop {
        let next = if let Some(ref cancel) = cancel {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    return acc.finish();
                }
                chunk = stream.next() => chunk,
            }
        } else {
            stream.next().await
        };
        let Some(chunk) = next else {
            break;
        };
        let mut bytes = match chunk {
            Ok(b) => b,
            Err(e) => {
                let mut out = acc.finish();
                if out.text.is_empty() {
                    out.text = format!("llm stream failed: {e}");
                }
                return out;
            }
        };
        if first {
            first = false;
            if bytes.starts_with(UTF8_BOM) {
                bytes = bytes.slice(UTF8_BOM.len()..);
            }
        }
        buf.extend_from_slice(&bytes);
        for data in take_sse_data(&mut buf) {
            let Ok(frame) = serde_json::from_str::<ChatCompletionChunk>(&data) else {
                continue;
            };
            for delta in acc.ingest(frame) {
                on_delta(delta);
            }
        }
        if cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
            return acc.finish();
        }
    }
    acc.finish()
}

fn resolve_endpoint(sampler: &HttpSampler, model: &str) -> (String, String) {
    let choice = config::lookup_model(model);
    let api_base = choice
        .as_ref()
        .and_then(|m| m.api_base_url.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| sampler.api_base.clone());
    let own_base = choice
        .as_ref()
        .and_then(|m| m.api_base_url.as_deref())
        .is_some_and(|s| !s.trim().is_empty());
    let api_key = choice
        .as_ref()
        .and_then(|m| m.resolved_api_key())
        .unwrap_or_else(|| {
            if own_base {
                String::new()
            } else {
                sampler.api_key.clone()
            }
        });
    (api_base, api_key)
}

fn estimate_prompt_tokens(request: &PromptRequest) -> u64 {
    let mut ascii = 0u64;
    let mut other = 0u64;
    let bump = |s: &str, ascii: &mut u64, other: &mut u64| {
        for c in s.chars() {
            if c.is_ascii() {
                *ascii += 1;
            } else {
                *other += 1;
            }
        }
    };
    bump(&request.system, &mut ascii, &mut other);
    for event in &request.history {
        match event {
            LogEvent::User(t) | LogEvent::Prompt(t) => bump(t, &mut ascii, &mut other),
            LogEvent::LlmStream(out) => bump(&out.text, &mut ascii, &mut other),
            LogEvent::ToolExecute { content, .. } => bump(content, &mut ascii, &mut other),
            LogEvent::PreStep => {}
        }
    }
    other + ascii.saturating_add(3) / 4
}

fn messages(request: &PromptRequest, user_images: &[Vec<UserImage>]) -> Vec<Value> {
    let mut out = vec![json!({"role":"system","content": request.system})];
    let mut user_i = 0usize;
    for event in &request.history {
        match event {
            LogEvent::User(text) => {
                let images = user_images.get(user_i).cloned().unwrap_or_default();
                user_i += 1;
                out.push(user_message(text, &images));
            }
            LogEvent::LlmStream(llm) if !llm.tool_calls.is_empty() => {
                let tool_calls: Vec<Value> = llm
                    .tool_calls
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "type": "function",
                            "function": {
                                "name": c.name,
                                "arguments": c.arguments,
                            }
                        })
                    })
                    .collect();
                let mut msg = json!({
                    "role": "assistant",
                    "tool_calls": tool_calls,
                });
                if !llm.text.is_empty() {
                    msg["content"] = json!(llm.text);
                }
                out.push(msg);
            }
            LogEvent::LlmStream(llm) if !llm.text.is_empty() => {
                out.push(json!({"role":"assistant","content": llm.text}));
            }
            LogEvent::ToolExecute { id, content, .. } => out.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": content,
            })),
            LogEvent::PreStep | LogEvent::Prompt(_) | LogEvent::LlmStream(_) => {}
        }
    }
    out
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
            "type": "image_url",
            "image_url": {
                "url": format!("data:{};base64,{b64}", img.mime)
            }
        }));
    }
    if parts.is_empty() {
        json!({"role": "user", "content": text})
    } else {
        json!({"role": "user", "content": parts})
    }
}

fn parse_chat_completion(body: &str) -> LlmOutput {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return LlmOutput {
            text: body.to_string(),
            ..LlmOutput::default()
        };
    };
    let message = &v["choices"][0]["message"];
    let text = message["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let reasoning = message["reasoning_content"]
        .as_str()
        .or_else(|| message["reasoning"].as_str())
        .unwrap_or("")
        .to_string();
    let mut tool_calls = Vec::new();
    if let Some(calls) = message["tool_calls"].as_array() {
        for (i, call) in calls.iter().enumerate() {
            let id = call["id"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("call-{i}"));
            let name = call["function"]["name"].as_str().unwrap_or("").to_string();
            let arguments = match &call["function"]["arguments"] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if !name.is_empty() {
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }
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

    #[test]
    fn parses_tool_call() {
        let body = r#"{
            "choices":[{
                "message":{
                    "content":null,
                    "tool_calls":[{
                        "id":"1",
                        "type":"function",
                        "function":{"name":"list_dir","arguments":"{\"target_directory\":\".\"}"}
                    }]
                }
            }]
        }"#;
        let out = parse_chat_completion(body);
        assert_eq!(out.tool_calls[0].name, "list_dir");
        assert!(out.tool_calls[0].arguments.contains("target_directory"));
    }
}
