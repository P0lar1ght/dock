//! `"llm"` HTTP sampler. Live-lookup `"settings"` for model and `"turn"` for
//! cancel. `api_backend` picks the wire (Grok `ApiBackend`): chat/completions,
//! Responses, or Anthropic Messages. All three emit [`StreamDelta`].

mod messages;
mod responses;
mod tool_images;

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::chat_chunk::ChatCompletionChunk;
use crate::config::{self, ApiBackend, AuthScheme};
use crate::llm::Sampler;
use crate::names::{SESSIONS, SETTINGS, TURN};
use crate::runtime::BoxFuture;
use crate::session::Sessions;
use crate::settings::AppSettings;
use crate::stream_acc::{take_sse_data, ChatStreamAcc, StreamDelta};
use crate::turn::TurnControl;
use crate::types::{
    LlmOutput, LogEvent, PromptRequest, ToolCall, UserImage, INTERRUPTED_TOOL_RESULT,
};

use cordis::Context;

/// HTTP `User-Agent` for LLM calls. Gateways that key the App column off UA
/// see `dock/<version>` instead of `reqwest/0.12`.
const LLM_USER_AGENT: &str = concat!("dock/", env!("CARGO_PKG_VERSION"));

pub struct HttpSampler {
    pub ctx: Context,
    pub api_key: String,
    pub api_base: String,
    pub fallback_model: String,
}

/// 一次请求的模型侧参数。能力与默认值来自 `[model.<id>]`，运行时开关（思考
/// 开 / 关、当前强度）来自 `"settings"`。
///
/// 缺省一律是**不发这个字段**，让上游用自己的默认：harness 替所有模型猜一个
/// 值（"medium"、16384、按名字判断能不能读图）正是不同模型报错的来源。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Reasoning {
    /// `reasoning = false`：这个模型没有推理档，一个相关字段都不发。发
    /// `effort: "none"` 对它同样是未知字段，照样 400。
    Unsupported,
    /// 支持推理，但用户关了思考：显式告诉上游别想。
    Off,
    /// 开着。`effort` 为空 = 不指定强度，用上游默认。
    #[default]
    On,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WireParams {
    pub reasoning: Reasoning,
    /// 只在 [`Reasoning::On`] 时有意义；空 = 不发。
    pub effort: String,
    /// None = 不发（Messages 后端另有必填兜底）。
    pub max_output_tokens: Option<u32>,
    /// false = 图片不进请求体。
    pub images: bool,
}

impl WireParams {
    fn resolve(choice: Option<&config::ModelChoice>, settings: Option<&AppSettings>) -> Self {
        let thinking = settings.is_none_or(AppSettings::thinking);
        let reasoning = if !choice.is_none_or(config::ModelChoice::supports_reasoning) {
            Reasoning::Unsupported
        } else if thinking {
            Reasoning::On
        } else {
            Reasoning::Off
        };
        // 运行时强度优先（用户在 /model 或设置里选过），否则用该模型的默认。
        let effort = match reasoning {
            Reasoning::On => settings
                .map(AppSettings::effort)
                .filter(|e| !e.is_empty())
                .or_else(|| choice.map(config::ModelChoice::default_effort))
                .unwrap_or_default(),
            _ => String::new(),
        };
        Self {
            reasoning,
            effort,
            max_output_tokens: choice.and_then(|c| c.max_output_tokens),
            images: choice.is_none_or(config::ModelChoice::accepts_images),
        }
    }
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
    let choice = config::lookup_model(&model);
    let settings = sampler.ctx.get::<AppSettings>(SETTINGS);
    let params = WireParams::resolve(choice.as_ref(), settings.as_deref());
    let backend = choice.as_ref().map(|m| m.api_backend).unwrap_or_default();
    let auth = choice
        .as_ref()
        .map(|m| m.resolved_auth())
        .unwrap_or(AuthScheme::Bearer);
    let wire = choice
        .as_ref()
        .map(|m| m.wire_model().to_string())
        .unwrap_or_else(|| model.clone());
    let (api_base, api_key) = resolve_endpoint(sampler, &model);
    if let Some(sessions) = sampler.ctx.get::<Sessions>(SESSIONS) {
        let window = choice
            .as_ref()
            .and_then(|m| m.context_window)
            .unwrap_or(128_000);
        sessions.set_window(window);
    }
    let prompt_est = estimate_prompt_tokens(&request);
    on_delta(StreamDelta::Usage {
        tokens: crate::usage::TokenUsage {
            prompt_tokens: prompt_est,
            ..crate::usage::TokenUsage::default()
        },
        official: false,
        model: wire.clone(),
        cost_usd_ticks: None,
    });
    let url = format!("{}/{}", api_base.trim_end_matches('/'), backend.path());
    let user_images = sampler
        .ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.model_user_images())
        .unwrap_or_default();
    let body = match backend {
        ApiBackend::ChatCompletions => chat_body(&wire, &request, &user_images, &params),
        ApiBackend::Responses => responses::body(&wire, &request, &user_images, &params),
        ApiBackend::Messages => {
            // Anthropic 的前缀缓存只在显式断点处写，缺省开；自建代理不认
            // `cache_control` 时 `[model.<id>].prompt_cache = false` 关掉。
            let prompt_cache = choice
                .as_ref()
                .map(config::ModelChoice::prompt_cache_enabled)
                .unwrap_or(true);
            messages::body(&wire, &request, &user_images, &params, prompt_cache)
        }
    };
    let client = reqwest::Client::builder()
        .user_agent(LLM_USER_AGENT)
        .build()
        .expect("dock user-agent is a valid header value");
    let cancel = sampler.ctx.get::<TurnControl>(TURN).map(|t| t.token());
    let mut response = None;
    for attempt in 0..3u32 {
        let mut send = client
            .post(&url)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&body);
        send = apply_auth(send, auth, &api_key);
        if backend == ApiBackend::Messages {
            send = send.header("anthropic-version", "2023-06-01");
        }
        let send = send.send();
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
        let output = parse_json_body(backend, &text);
        if !output.reasoning.is_empty() {
            on_delta(StreamDelta::Reasoning(output.reasoning.clone()));
        }
        if !output.text.is_empty() {
            on_delta(StreamDelta::Text(output.text.clone()));
        }
        return output;
    }

    let cancel = sampler.ctx.get::<TurnControl>(TURN).map(|t| t.token());
    let mut acc = WireAcc::new(backend, &wire);
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
            for delta in acc.ingest(&data) {
                on_delta(delta);
            }
        }
        if cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
            return acc.finish();
        }
    }
    acc.finish()
}

fn apply_auth(
    req: reqwest::RequestBuilder,
    auth: AuthScheme,
    api_key: &str,
) -> reqwest::RequestBuilder {
    match auth {
        AuthScheme::Bearer => req.bearer_auth(api_key),
        AuthScheme::XApiKey => req.header("x-api-key", api_key),
    }
}

enum WireAcc {
    Chat(ChatStreamAcc),
    Responses(responses::Acc),
    Messages(messages::Acc),
}

impl WireAcc {
    fn new(backend: ApiBackend, model: &str) -> Self {
        match backend {
            ApiBackend::ChatCompletions => Self::Chat(ChatStreamAcc::default()),
            ApiBackend::Responses => Self::Responses(responses::Acc::new(model)),
            ApiBackend::Messages => Self::Messages(messages::Acc::new(model)),
        }
    }

    fn ingest(&mut self, data: &str) -> Vec<StreamDelta> {
        match self {
            Self::Chat(acc) => {
                let Ok(frame) = serde_json::from_str::<ChatCompletionChunk>(data) else {
                    return Vec::new();
                };
                acc.ingest(frame)
            }
            Self::Responses(acc) => acc.ingest_json(data),
            Self::Messages(acc) => acc.ingest_json(data),
        }
    }

    fn finish(self) -> LlmOutput {
        match self {
            Self::Chat(acc) => acc.finish(),
            Self::Responses(acc) => acc.finish(),
            Self::Messages(acc) => acc.finish(),
        }
    }
}

fn parse_json_body(backend: ApiBackend, text: &str) -> LlmOutput {
    match backend {
        ApiBackend::ChatCompletions => parse_chat_completion(text),
        ApiBackend::Responses => responses::parse_json(text),
        ApiBackend::Messages => messages::parse_json(text),
    }
}

fn chat_body(
    model: &str,
    request: &PromptRequest,
    user_images: &[Vec<UserImage>],
    params: &WireParams,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages(request, user_images, params.images),
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if let Some(max) = params.max_output_tokens {
        body["max_tokens"] = json!(max);
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
    match params.reasoning {
        // 没有推理档的模型：连"别想"都不要说。
        Reasoning::Unsupported => {}
        Reasoning::Off => {
            body["reasoning"] = json!({ "effort": "none", "exclude": true });
            body["reasoning_effort"] = json!("none");
        }
        Reasoning::On if params.effort == "none" => {
            body["reasoning"] = json!({ "effort": "none", "exclude": true });
            body["reasoning_effort"] = json!("none");
        }
        Reasoning::On if !params.effort.is_empty() => {
            // OpenRouter MiniMax / Claude / etc. need the unified `reasoning`
            // object; legacy `reasoning_effort` alone is not enough for M3.
            body["reasoning"] = json!({
                "effort": params.effort,
                "exclude": false,
            });
            body["reasoning_effort"] = json!(params.effort);
        }
        // 开着但没指定强度：不发，用上游默认。
        Reasoning::On => {}
    }
    body
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
            LogEvent::User(t) | LogEvent::Prompt(t) | LogEvent::SystemReminder(t) => {
                bump(t, &mut ascii, &mut other)
            }
            LogEvent::LlmStream(out) => bump(&out.text, &mut ascii, &mut other),
            LogEvent::ToolExecute { content, .. } => bump(content, &mut ascii, &mut other),
            LogEvent::PreStep => {}
        }
    }
    other + ascii.saturating_add(3) / 4
}

fn messages(request: &PromptRequest, user_images: &[Vec<UserImage>], vision: bool) -> Vec<Value> {
    let mut out = vec![json!({"role":"system","content": request.system})];
    let mut user_i = 0usize;
    let mut pending: Vec<String> = Vec::new();
    // Images from this assistant turn's tool results — flushed once after all
    // role:tool messages for the turn (API rejects tool/user/tool interleaving).
    let mut pending_images: Vec<UserImage> = Vec::new();
    for event in &request.history {
        match event {
            LogEvent::User(text) => {
                flush_unmatched_tools(&mut out, &mut pending);
                flush_pending_tool_images(&mut out, &mut pending_images, vision);
                let images = user_images.get(user_i).cloned().unwrap_or_default();
                user_i += 1;
                out.push(user_message(text, &images));
            }
            LogEvent::SystemReminder(text) => {
                flush_unmatched_tools(&mut out, &mut pending);
                flush_pending_tool_images(&mut out, &mut pending_images, vision);
                out.push(user_message(text, &[]));
            }
            LogEvent::LlmStream(llm) if !llm.tool_calls.is_empty() => {
                flush_unmatched_tools(&mut out, &mut pending);
                flush_pending_tool_images(&mut out, &mut pending_images, vision);
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
                if !llm.reasoning.is_empty() {
                    msg["reasoning"] = json!(llm.reasoning);
                    msg["reasoning_content"] = json!(llm.reasoning);
                }
                out.push(msg);
                pending = llm.tool_calls.iter().map(|c| c.id.clone()).collect();
            }
            LogEvent::LlmStream(llm) if !llm.text.is_empty() || !llm.reasoning.is_empty() => {
                flush_unmatched_tools(&mut out, &mut pending);
                flush_pending_tool_images(&mut out, &mut pending_images, vision);
                let mut msg = json!({"role":"assistant","content": llm.text});
                if !llm.reasoning.is_empty() {
                    msg["reasoning"] = json!(llm.reasoning);
                    msg["reasoning_content"] = json!(llm.reasoning);
                }
                out.push(msg);
            }
            LogEvent::ToolExecute {
                id,
                content,
                images,
                ..
            } => {
                if let Some(i) = pending.iter().position(|p| p == id) {
                    pending.remove(i);
                    out.push(tool_images::chat_tool_message(id, content));
                    pending_images.extend(images.iter().cloned());
                    if pending.is_empty() {
                        flush_pending_tool_images(&mut out, &mut pending_images, vision);
                    }
                }
            }
            LogEvent::PreStep | LogEvent::Prompt(_) | LogEvent::LlmStream(_) => {}
        }
    }
    flush_unmatched_tools(&mut out, &mut pending);
    flush_pending_tool_images(&mut out, &mut pending_images, vision);
    out
}

fn flush_unmatched_tools(out: &mut Vec<Value>, pending: &mut Vec<String>) {
    for id in pending.drain(..) {
        out.push(json!({
            "role": "tool",
            "tool_call_id": id,
            "content": INTERRUPTED_TOOL_RESULT,
        }));
    }
}

fn flush_pending_tool_images(
    out: &mut Vec<Value>,
    pending_images: &mut Vec<UserImage>,
    vision: bool,
) {
    if pending_images.is_empty() {
        return;
    }
    let images = std::mem::take(pending_images);
    if let Some(msg) = tool_images::chat_tool_images_user(&images, vision) {
        out.push(msg);
    }
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
    let text = message["content"].as_str().unwrap_or("").to_string();
    let reasoning = message["reasoning_content"]
        .as_str()
        .or_else(|| message["reasoning"].as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .or_else(|| reasoning_details_text(&message["reasoning_details"]))
        .unwrap_or_default();
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

fn reasoning_details_text(details: &Value) -> Option<String> {
    let arr = details.as_array()?;
    let mut buf = String::new();
    for item in arr {
        let piece = item["text"]
            .as_str()
            .or_else(|| item["summary"].as_str())
            .unwrap_or("");
        if !piece.is_empty() {
            buf.push_str(piece);
        }
    }
    (!buf.is_empty()).then_some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str) -> config::ModelChoice {
        config::ModelChoice {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            api_base_url: None,
            api_key: None,
            env_key: None,
            context_window: None,
            api_backend: ApiBackend::ChatCompletions,
            auth_scheme: None,
            api_model: None,
            prompt_cache: None,
            max_output_tokens: None,
            reasoning: None,
            reasoning_effort: None,
            reasoning_efforts: None,
            supports_images: None,
        }
    }

    fn empty_request() -> PromptRequest {
        PromptRequest {
            system: "s".into(),
            history: vec![LogEvent::User("hi".into())],
            tools: vec![],
        }
    }

    /// 目录里没这个模型（或压根没 config）时不替上游做任何假设。
    #[test]
    fn wire_params_default_to_sending_nothing() {
        let params = WireParams::resolve(None, None);
        assert_eq!(params.reasoning, Reasoning::On);
        assert!(params.effort.is_empty(), "强度缺省不发");
        assert_eq!(params.max_output_tokens, None);
        assert!(params.images, "未知模型按多模态处理");
    }

    #[test]
    fn wire_params_come_from_model_config() {
        let mut choice = model("m");
        choice.reasoning_effort = Some("high".into());
        choice.max_output_tokens = Some(8192);
        choice.supports_images = Some(false);
        let params = WireParams::resolve(Some(&choice), None);
        assert_eq!(params.effort, "high");
        assert_eq!(params.max_output_tokens, Some(8192));
        assert!(!params.images);

        choice.reasoning = Some(false);
        let params = WireParams::resolve(Some(&choice), None);
        assert_eq!(params.reasoning, Reasoning::Unsupported);
        assert!(params.effort.is_empty(), "不支持推理就不带强度");
    }

    /// 三态各自发什么：不支持 = 什么都不发，关掉 = 显式 none，开着 = 按强度。
    #[test]
    fn chat_body_reasoning_is_three_state() {
        let req = empty_request();
        let mut params = WireParams {
            reasoning: Reasoning::Unsupported,
            ..WireParams::default()
        };
        let body = chat_body("m", &req, &[], &params);
        assert!(body.get("reasoning").is_none(), "{body}");
        assert!(body.get("reasoning_effort").is_none(), "{body}");

        params.reasoning = Reasoning::Off;
        let body = chat_body("m", &req, &[], &params);
        assert_eq!(body["reasoning"]["effort"], "none");
        assert_eq!(body["reasoning_effort"], "none");

        params.reasoning = Reasoning::On;
        let body = chat_body("m", &req, &[], &params);
        assert!(body.get("reasoning").is_none(), "没指定强度就不发：{body}");

        params.effort = "high".into();
        let body = chat_body("m", &req, &[], &params);
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn chat_body_sends_max_tokens_only_when_configured() {
        let req = empty_request();
        let params = WireParams::default();
        assert!(chat_body("m", &req, &[], &params)
            .get("max_tokens")
            .is_none());
        let params = WireParams {
            max_output_tokens: Some(4096),
            ..WireParams::default()
        };
        assert_eq!(chat_body("m", &req, &[], &params)["max_tokens"], 4096);
    }

    #[test]
    fn llm_user_agent_is_dock_version() {
        assert_eq!(LLM_USER_AGENT, concat!("dock/", env!("CARGO_PKG_VERSION")));
        assert!(LLM_USER_AGENT.starts_with("dock/"));
    }

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

    #[test]
    fn parses_openrouter_reasoning_details_message() {
        let body = r#"{
            "choices":[{
                "message":{
                    "content":"answer",
                    "reasoning_details":[
                        {"type":"reasoning.text","text":"think hard"}
                    ]
                }
            }]
        }"#;
        let out = parse_chat_completion(body);
        assert_eq!(out.text, "answer");
        assert_eq!(out.reasoning, "think hard");
    }

    #[test]
    fn messages_inserts_stubs_before_user_after_unmatched_tool_calls() {
        let req = PromptRequest {
            system: "s".into(),
            history: vec![
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
            ],
            tools: vec![],
        };
        let msgs = messages(&req, &[], true);
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[3]["tool_call_id"], "c1");
        assert_eq!(msgs[3]["content"], "ok");
        assert_eq!(msgs[4]["role"], "tool");
        assert_eq!(msgs[4]["tool_call_id"], "c2");
        assert_eq!(msgs[4]["content"], INTERRUPTED_TOOL_RESULT);
        assert_eq!(msgs[5]["role"], "user");
        assert_eq!(msgs[5]["content"], "follow-up");
    }

    #[test]
    fn tool_result_emits_image_part() {
        use std::sync::Arc;
        let png = {
            let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            v.extend_from_slice(&[0, 0, 0, 13]);
            v.extend_from_slice(b"IHDR");
            v.extend_from_slice(&1u32.to_be_bytes());
            v.extend_from_slice(&1u32.to_be_bytes());
            v.extend_from_slice(&[8, 2, 0, 0, 0]);
            v.extend(std::iter::repeat_n(0u8, 40));
            v
        };
        let img = UserImage {
            mime: "image/png".into(),
            data: Arc::from(png.into_boxed_slice()),
            width: 1,
            height: 1,
        };
        let req = PromptRequest {
            system: "s".into(),
            history: vec![
                LogEvent::User("hi".into()),
                LogEvent::LlmStream(LlmOutput {
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: "browser_screenshot".into(),
                        arguments: "{}".into(),
                    }],
                    ..LlmOutput::default()
                }),
                LogEvent::ToolExecute {
                    id: "c1".into(),
                    name: "browser_screenshot".into(),
                    arguments: "{}".into(),
                    content: "saved /tmp/shot.png\nImage content included inline".into(),
                    images: vec![img],
                },
            ],
            tools: vec![],
        };
        let msgs = messages(&req, &[], true);
        let tool = msgs.iter().find(|m| m["role"] == "tool").expect("tool msg");
        assert_eq!(
            tool["content"],
            "saved /tmp/shot.png\nImage content included inline"
        );
        let user_img = msgs
            .iter()
            .find(|m| m["role"] == "user" && m["content"].is_array())
            .expect("adjacent user with image");
        let parts = user_img["content"].as_array().unwrap();
        assert!(parts.iter().any(|p| p["type"] == "image_url"), "{parts:?}");
    }

    #[test]
    fn tool_result_images_degraded_for_non_vision_model() {
        use std::sync::Arc;
        let png = {
            let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            v.extend_from_slice(&[0, 0, 0, 13]);
            v.extend_from_slice(b"IHDR");
            v.extend_from_slice(&1u32.to_be_bytes());
            v.extend_from_slice(&1u32.to_be_bytes());
            v.extend_from_slice(&[8, 2, 0, 0, 0]);
            v.extend(std::iter::repeat_n(0u8, 40));
            v
        };
        let img = UserImage {
            mime: "image/png".into(),
            data: Arc::from(png.into_boxed_slice()),
            width: 1,
            height: 1,
        };
        let req = PromptRequest {
            system: "s".into(),
            history: vec![
                LogEvent::User("hi".into()),
                LogEvent::LlmStream(LlmOutput {
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: "browser_screenshot".into(),
                        arguments: "{}".into(),
                    }],
                    ..LlmOutput::default()
                }),
                LogEvent::ToolExecute {
                    id: "c1".into(),
                    name: "browser_screenshot".into(),
                    arguments: "{}".into(),
                    content: "saved shot\nImage content included inline".into(),
                    images: vec![img],
                },
            ],
            tools: vec![],
        };
        let vision = messages(&req, &[], true);
        assert!(
            vision.iter().any(|m| {
                m["role"] == "user"
                    && m["content"]
                        .as_array()
                        .is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url"))
            }),
            "vision model should attach image parts: {vision:?}"
        );
        let text_only = messages(&req, &[], false);
        assert!(
            !text_only.iter().any(|m| {
                m["content"]
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url"))
            }),
            "non-vision model must not attach image parts: {text_only:?}"
        );
        let tool = text_only
            .iter()
            .find(|m| m["role"] == "tool")
            .expect("tool msg");
        assert_eq!(tool["content"], "saved shot\nImage content included inline");
    }

    fn tiny_png() -> UserImage {
        use std::sync::Arc;
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        v.extend_from_slice(&[0, 0, 0, 13]);
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(&[8, 2, 0, 0, 0]);
        v.extend(std::iter::repeat_n(0u8, 40));
        UserImage {
            mime: "image/png".into(),
            data: Arc::from(v.into_boxed_slice()),
            width: 1,
            height: 1,
        }
    }

    /// Parallel dual tools in one turn; only one carries images.
    /// Must be tool, tool, THEN one user — never tool/user/tool.
    #[test]
    fn parallel_tools_batch_images_after_all_tool_results() {
        let img = tiny_png();
        let req = PromptRequest {
            system: "s".into(),
            history: vec![
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
                            name: "browser_screenshot".into(),
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
                LogEvent::ToolExecute {
                    id: "c2".into(),
                    name: "browser_screenshot".into(),
                    arguments: "{}".into(),
                    content: "shot\nImage content included inline".into(),
                    images: vec![img],
                },
            ],
            tools: vec![],
        };
        let msgs = messages(&req, &[], true);
        let roles: Vec<&str> = msgs
            .iter()
            .skip(1) // system
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(
            roles,
            ["user", "assistant", "tool", "tool", "user"],
            "expected tool,tool,user batching; got {roles:?} full={msgs:?}"
        );
        assert_eq!(msgs[3]["tool_call_id"], "c1");
        assert_eq!(msgs[4]["tool_call_id"], "c2");
        let parts = msgs[5]["content"].as_array().expect("batched user parts");
        let n_images = parts.iter().filter(|p| p["type"] == "image_url").count();
        assert_eq!(n_images, 1, "single tool's image only: {parts:?}");
        // No interleaved user between the two tools.
        assert!(
            !(msgs[3]["role"] == "tool" && msgs[4]["role"] == "user" && msgs[5]["role"] == "tool"),
            "must not interleave tool/user/tool"
        );
    }

    #[test]
    fn parallel_tools_both_with_images_batch_into_one_user() {
        let img1 = tiny_png();
        let img2 = tiny_png();
        let req = PromptRequest {
            system: "s".into(),
            history: vec![
                LogEvent::User("hi".into()),
                LogEvent::LlmStream(LlmOutput {
                    tool_calls: vec![
                        ToolCall {
                            id: "c1".into(),
                            name: "browser_screenshot".into(),
                            arguments: "{}".into(),
                        },
                        ToolCall {
                            id: "c2".into(),
                            name: "computer_screenshot".into(),
                            arguments: "{}".into(),
                        },
                    ],
                    ..LlmOutput::default()
                }),
                LogEvent::ToolExecute {
                    id: "c1".into(),
                    name: "browser_screenshot".into(),
                    arguments: "{}".into(),
                    content: "shot1".into(),
                    images: vec![img1],
                },
                LogEvent::ToolExecute {
                    id: "c2".into(),
                    name: "computer_screenshot".into(),
                    arguments: "{}".into(),
                    content: "shot2".into(),
                    images: vec![img2],
                },
            ],
            tools: vec![],
        };
        let msgs = messages(&req, &[], true);
        let roles: Vec<&str> = msgs
            .iter()
            .skip(1)
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(
            roles,
            ["user", "assistant", "tool", "tool", "user"],
            "expected tool,tool,user; got {roles:?}"
        );
        let parts = msgs[5]["content"].as_array().expect("batched user parts");
        let n_images = parts.iter().filter(|p| p["type"] == "image_url").count();
        assert_eq!(n_images, 2, "both images in one user: {parts:?}");
    }
}
