//! `"llm"` HTTP sampler. Live-lookup `"settings"` for model and `"turn"` for
//! cancel. `[model.<id>].api_backends` declares which wires the endpoint speaks;
//! `"settings"` holds the one in use (`/protocol`): Responses (default),
//! chat/completions, or Anthropic Messages. All three emit [`StreamDelta`].

mod cache_debug;
mod messages;
mod responses;
mod tool_images;

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::agent::runtime::BoxFuture;
use crate::agent::turn::TurnControl;
use crate::host::settings::{AppSettings, ModelOverride};
use crate::llm::sampler::Sampler;
use crate::names::{MODEL_OVERRIDE, SESSIONS, SETTINGS, TURN};
use crate::session::log::Sessions;
use cordis_base::chat_chunk::ChatCompletionChunk;
use cordis_base::config::{self, ApiBackend, AuthScheme};
use cordis_base::stream_acc::{take_sse_data, ChatStreamAcc, StreamDelta};
use cordis_base::types::{
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
    fn resolve(
        choice: Option<&config::ModelChoice>,
        settings: Option<&AppSettings>,
        over: Option<&ModelOverride>,
    ) -> Self {
        let thinking = settings.is_none_or(AppSettings::thinking);
        let reasoning = if !choice.is_none_or(config::ModelChoice::supports_reasoning) {
            Reasoning::Unsupported
        } else if thinking {
            Reasoning::On
        } else {
            Reasoning::Off
        };
        // 这次委派点名的强度最优先（workflow 脚本的 `agent(effort:)`），其次是
        // 运行时强度（用户在 /model 或设置里选过），最后才是该模型的默认。
        let effort = match reasoning {
            Reasoning::On => over
                .and_then(|o| o.effort.clone())
                .filter(|e| !e.is_empty())
                .or_else(|| settings.map(AppSettings::effort).filter(|e| !e.is_empty()))
                .or_else(|| choice.map(config::ModelChoice::default_effort))
                .unwrap_or_default(),
            _ => String::new(),
        };
        Self {
            reasoning,
            effort,
            max_output_tokens: over
                .and_then(|o| o.max_output_tokens)
                .or_else(|| choice.and_then(|c| c.max_output_tokens)),
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

/// 进程级共用的 LLM HTTP client。
///
/// `reqwest::Client` 持有连接池，**每次请求新建一个等于每次都重做 DNS + TCP +
/// TLS 握手**（reqwest 文档明确反对这么用）。对着本来就不稳的 provider，这会
/// 实打实抬高传输失败率——旧实现正是在 `sample_http` 里 per-request 建的。
///
/// 只设 `connect_timeout`，**不设整体 `timeout`**：响应是 SSE 长流，一个全局
/// 超时会把正常的长回答拦腰砍断。
fn llm_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(LLM_USER_AGENT)
            .connect_timeout(std::time::Duration::from_secs(20))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .build()
            .expect("dock user-agent is a valid header value")
    })
}

/// 传输层错误值不值得重试。
///
/// builder 错误是我们自己把请求拼错了，重试多少次都一样；其余（连接、TLS、
/// 超时、连接被重置）都是请求没到服务器或没被受理，重试安全。
fn is_retryable_transport(e: &reqwest::Error) -> bool {
    !e.is_builder()
}

/// 把 reqwest 错误摊成一句有信息量的话。
///
/// `reqwest::Error` 的 `Display` 只印 `error sending request for url (…)`，
/// **真正的原因在 `source()` 链里**（`connection reset by peer`、`dns error`、
/// `certificate verify failed`…）。不走这条链，报错就永远只有那句没信息量的
/// 壳——这正是「provider 报错看不出原因」的由来，不是上游没给。
fn transport_detail(e: &reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "超时"
    } else if e.is_connect() {
        "连接失败"
    } else if e.is_body() {
        "请求体"
    } else if e.is_decode() {
        "解码"
    } else {
        "传输"
    };
    format!("[{kind}] {}", error_chain(e))
}

/// 把一个错误的 `source()` 链摊成 `外层 ← 内层 ← 根因`。
///
/// 独立出来是为了可测：用真的 `reqwest::Error` 造一条带 source 的链很难。
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut chain = vec![e.to_string()];
    let mut src = e.source();
    while let Some(s) = src {
        let text = s.to_string();
        // 链上常有重复措辞，重复的不再堆。
        if !chain.iter().any(|p| p == &text) {
            chain.push(text);
        }
        src = s.source();
    }
    chain.join(" ← ")
}

/// bytes_stream 传输失败并入 [`LlmOutput`]：只写 `error`，绝不填 `text`。
///
/// 抽成纯函数是为了可测——造一条活的 HTTP `bytes_stream` Err 在单测里太重。
fn merge_stream_transport_error(mut out: LlmOutput, detail: String) -> LlmOutput {
    if out.error.is_none() {
        out.error = Some(format!("llm stream failed: {detail}"));
    }
    out
}

/// 各家用的 request-id 头名不一样，挨个试。
fn response_request_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    ["request-id", "x-request-id", "cf-ray", "x-amzn-requestid"]
        .iter()
        .find_map(|name| {
            headers
                .get(*name)
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
}

async fn sample_http(
    sampler: &HttpSampler,
    request: PromptRequest,
    mut on_delta: Box<dyn FnMut(StreamDelta) + Send + '_>,
) -> LlmOutput {
    // 采样的是**谁的**会话：子代理跑在自己的隔离 ctx 上，而 `sampler.ctx` 是
    // `llm()` 挂载时捕获的根 ctx。拿错的后果不只是日志标签——子代理会带上主会话
    // 的图片（`model_user_images` 按下标取），它自己的 `TurnControl` 也没人听，
    // `interrupt_agent` 打不断正在跑的这个流。`"settings"` 这类没被隔离的服务，
    // 从哪个 ctx 查都解析到同一个对象。
    let exec = crate::llm::sampler::sampling_ctx().unwrap_or_else(|| sampler.ctx.clone());
    // 这次委派点名的模型优先。只有受限子会话挂得上 `"model-override"`，主会话
    // 没有这一项，所以这条分支对主线程是死的。
    let over = exec.get::<ModelOverride>(MODEL_OVERRIDE);
    let model = over
        .as_deref()
        .and_then(|o| o.model.clone())
        .or_else(|| {
            exec.get::<AppSettings>(SETTINGS)
                .map(|s| s.model())
                .filter(|m| !m.is_empty())
        })
        .unwrap_or_else(|| sampler.fallback_model.clone());
    let choice = config::lookup_model(&model);
    let settings = exec.get::<AppSettings>(SETTINGS);
    let params = WireParams::resolve(choice.as_ref(), settings.as_deref(), over.as_deref());
    // 协议是运行时状态（`/protocol`），不是模型的固定属性：一个端点可以同时开
    // /responses 与 /chat/completions。settings 会对着目录校一遍再给出来。
    let backend = settings
        .as_deref()
        .map(AppSettings::backend)
        .unwrap_or_else(|| {
            choice
                .as_ref()
                .map(config::ModelChoice::default_backend)
                .unwrap_or_default()
        });
    let auth = choice
        .as_ref()
        .map(|m| m.resolved_auth(backend))
        .unwrap_or_else(|| AuthScheme::default_for(backend));
    let wire = choice
        .as_ref()
        .map(|m| m.wire_model_for(backend).to_string())
        .unwrap_or_else(|| model.clone());
    let (api_base, api_key) = resolve_endpoint(sampler, &model, backend);
    if let Some(sessions) = exec.get::<Sessions>(SESSIONS) {
        let window = choice
            .as_ref()
            .and_then(|m| m.context_window)
            .unwrap_or(128_000);
        sessions.set_window(window);
    }
    let prompt_est = estimate_prompt_tokens(&request);
    on_delta(StreamDelta::Usage {
        tokens: cordis_base::usage::TokenUsage {
            prompt_tokens: prompt_est,
            ..cordis_base::usage::TokenUsage::default()
        },
        official: false,
        model: wire.clone(),
        cost_usd_ticks: None,
    });
    let url = format!("{}/{}", api_base.trim_end_matches('/'), backend.path());
    let user_images = exec
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
    // `DOCK_CACHE_DEBUG` 没设时是空转；设了就把这次请求的前缀与同一会话上一次
    // 的逐条比一遍，写进日志。子代理与主会话交替发请求，所以按身份分开比。
    cache_debug::record_request(
        &exec
            .get::<Sessions>(SESSIONS)
            .map(|s| s.identity().to_string())
            .unwrap_or_else(|| "?".into()),
        backend.path(),
        &wire,
        &body,
    );
    let client = llm_client();
    let cancel = exec.get::<TurnControl>(TURN).map(|t| t.token());
    let mut response = None;
    let mut last_transport_error: Option<String> = None;
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
                // 空输出，不是 `"cancelled"` 这类占位文本：`finish_llm` 会把
                // 非空文本填进本轮那条助手记录，于是历史里留下一句模型从没说过
                // 的话，`seal_incomplete_tool_calls` 也不再把空槽位弹掉，
                // 连带 cancel-rewind（Stop 后把提示词还回输入框）一起失效。
                _ = cancel.cancelled() => return LlmOutput::default(),
                response = send => response,
            }
        } else {
            send.await
        };
        match got {
            // 传输层失败：请求根本没到服务器、或没被受理，重试是安全的。
            // 旧实现在这里直接 return，而这恰好是最该重试的一类
            // （连接重置 / DNS / TLS 握手 / 连接超时）。
            Err(e) if is_retryable_transport(&e) && attempt < 2 => {
                last_transport_error = Some(transport_detail(&e));
                let wait = std::time::Duration::from_millis(800 * (attempt as u64 + 1));
                if let Some(ref cancel) = cancel {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return LlmOutput::default(),
                        _ = tokio::time::sleep(wait) => {}
                    }
                } else {
                    tokio::time::sleep(wait).await;
                }
            }
            Err(e) => {
                let attempts = attempt + 1;
                return LlmOutput {
                    error: Some(format!(
                        "LLM 请求失败（已尝试 {attempts} 次）：{}",
                        transport_detail(&e)
                    )),
                    ..LlmOutput::default()
                };
            }
            Ok(r) if r.status().as_u16() == 503 && attempt < 2 => {
                let wait = std::time::Duration::from_millis(800 * (attempt as u64 + 1));
                if let Some(ref cancel) = cancel {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return LlmOutput::default(),
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
            error: Some(match last_transport_error {
                // 重试用光了，最后一次仍是传输失败：把真实原因带上，
                // 不要报成 503（那是另一回事）。
                Some(detail) => format!("LLM 请求失败（已尝试 3 次）：{detail}"),
                None => "LLM HTTP 503：服务端繁忙，已重试 3 次仍失败".into(),
            }),
            ..LlmOutput::default()
        };
    };
    let status = response.status();
    if !status.is_success() {
        // request-id 只有在服务端真的回了响应时才存在——传输层失败那条路径上
        // 是没有的，别在那边找。
        let request_id = response_request_id(response.headers());
        let text = response.text().await.unwrap_or_default();
        let id = request_id
            .map(|id| format!("，request-id: {id}"))
            .unwrap_or_default();
        return LlmOutput {
            error: Some(format!("LLM HTTP {status}{id}\n{}", text.trim())),
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
                    error: Some(format!("LLM 响应体读取失败：{}", transport_detail(&e))),
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

    let cancel = exec.get::<TurnControl>(TURN).map(|t| t.token());
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
                // 传输层失败只进 error，永不填 text——与 HTTP 503 / SSE 错误信封同契约
                //（types.rs LlmOutput::error 注释；假 text 会进 wire 并挡 Esc rewind）。
                return merge_stream_transport_error(acc.finish(), transport_detail(&e));
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
                cache_debug::note_usage(&delta);
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
                // 错误信封必须在 chunk 解析**之前**判。`ChatCompletionChunk`
                // 的字段全是 `serde(default)` 且不拒未知字段，所以
                // `{"error":{…}}` 会**成功**解析成一个 `choices` 为空的 chunk，
                // 落进 `Ok` 分支后被当成「没有增量」丢掉——放在解析失败的
                // `else` 里判等于永远不会触发。
                //
                // 后果是这一轮完全空白地收场：没有文本、没有报错，界面上就像
                // 应用坏了。
                if let Some(err) = cordis_base::stream_acc::chat_stream_error(data) {
                    acc.set_error(err);
                    return Vec::new();
                }
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

/// 基址按**当前协议**解析：同一个端点各协议的入口可能不同（DeepSeek 的
/// OpenAI 侧是 `…/`，Anthropic 侧是 `…/anthropic`），见
/// `[model.<id>.<protocol>].api_base_url`。
fn resolve_endpoint(sampler: &HttpSampler, model: &str, backend: ApiBackend) -> (String, String) {
    let choice = config::lookup_model(model);
    let own_base = choice
        .as_ref()
        .and_then(|m| m.base_url_for(backend))
        .map(str::to_string);
    let api_base = own_base.clone().unwrap_or_else(|| sampler.api_base.clone());
    let own_base = own_base.is_some();
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
            LogEvent::PreStep | LogEvent::Notice { .. } => {}
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
            // Notice 到不了这里（`model_history` 已滤掉）。
            LogEvent::PreStep
            | LogEvent::Prompt(_)
            | LogEvent::Notice { .. }
            | LogEvent::LlmStream(_) => {}
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

    /// 走**真正的集成点** `WireAcc::ingest`，不是只测 `chat_stream_error` 本身。
    ///
    /// 这是上一版漏掉的那一步：`ChatCompletionChunk` 的字段全是 `serde(default)`
    /// 且不拒未知字段，`{"error":{…}}` 会**成功**解析成 `choices` 为空的 chunk，
    /// 所以把错误判定放在解析失败的 `else` 里等于永远不触发。单测 helper 测不
    /// 出这件事，必须从 `ingest` 进。
    #[test]
    fn chat_error_envelope_is_caught_by_ingest_not_parsed_as_empty_chunk() {
        for raw in [
            r#"{"error":{"message":"rate limit exceeded","type":"rate_limit_error"}}"#,
            r#"{"error":"rate limit exceeded"}"#,
        ] {
            // 前提复述：它确实能解析成一个空 chunk —— 正是缺陷成因。
            let as_chunk = serde_json::from_str::<ChatCompletionChunk>(raw);
            assert!(as_chunk.is_ok_and(|c| c.choices.is_empty()), "{raw}");

            let mut acc = WireAcc::new(ApiBackend::ChatCompletions, "m");
            let deltas = acc.ingest(raw);
            assert!(deltas.is_empty(), "错误帧不该产生增量：{raw}");
            let out = acc.finish();
            assert!(out.text.is_empty(), "错误漏进 text：{}", out.text);
            assert!(
                out.error
                    .as_deref()
                    .is_some_and(|e| e.contains("rate limit")),
                "错误被吞掉了：{:?}（{raw}）",
                out.error
            );
        }
    }

    /// 正常增量帧不能被错误判定劫走——误判一条有内容的 chunk 会把整轮正文吞掉。
    #[test]
    fn chat_ordinary_frames_still_accumulate() {
        let mut acc = WireAcc::new(ApiBackend::ChatCompletions, "m");
        acc.ingest(r#"{"choices":[{"delta":{"content":"he"}}]}"#);
        // 有些代理会在正常帧上带 `"error": null`。
        acc.ingest(r#"{"choices":[{"delta":{"content":"llo"}}],"error":null}"#);
        let out = acc.finish();
        assert_eq!(out.text, "hello");
        assert_eq!(out.error, None);
    }

    /// 采样失败详情不进 chat/completions 的 `messages`（与另两条 wire 同一条
    /// 承诺）。三条都验，是因为「不进上下文」靠的是各自 builder 的门槛条件，
    /// 谁改宽了都会悄悄破功。
    #[test]
    fn llm_error_never_reaches_the_wire() {
        let request = PromptRequest {
            system: "s".into(),
            history: vec![
                LogEvent::User("hi".into()),
                LogEvent::LlmStream(LlmOutput {
                    error: Some("llm stream failed: [传输] connection reset by peer".into()),
                    ..LlmOutput::default()
                }),
            ],
            tools: vec![],
        };
        let chat =
            serde_json::to_string(&chat_body("m", &request, &[], &WireParams::default())).unwrap();
        let msgs = serde_json::to_string(&messages::body(
            "m",
            &request,
            &[],
            &WireParams::default(),
            true,
        ))
        .unwrap();
        let resp =
            serde_json::to_string(&responses::body("m", &request, &[], &WireParams::default()))
                .unwrap();
        for (name, json) in [("chat", &chat), ("messages", &msgs), ("responses", &resp)] {
            assert!(
                !json.contains("llm stream failed"),
                "{name} wire leaked stream failure: {json}"
            );
            assert!(
                !json.contains("connection reset"),
                "{name} wire leaked detail: {json}"
            );
        }
    }

    #[test]
    fn stream_transport_error_goes_to_error_not_text() {
        let out = merge_stream_transport_error(LlmOutput::default(), "boom".into());
        assert!(out.text.is_empty(), "{}", out.text);
        assert!(
            out.error
                .as_deref()
                .is_some_and(|e| e.contains("llm stream failed") && e.contains("boom")),
            "{:?}",
            out.error
        );
    }

    #[test]
    fn stream_transport_error_keeps_partial_text() {
        let out = merge_stream_transport_error(
            LlmOutput {
                text: "he".into(),
                ..LlmOutput::default()
            },
            "boom".into(),
        );
        assert_eq!(out.text, "he");
        assert!(
            out.error
                .as_deref()
                .is_some_and(|e| e.contains("llm stream failed") && e.contains("boom")),
            "{:?}",
            out.error
        );
    }

    #[test]
    fn stream_transport_error_does_not_clobber_existing_error() {
        let out = merge_stream_transport_error(
            LlmOutput {
                error: Some("first".into()),
                ..LlmOutput::default()
            },
            "boom".into(),
        );
        assert_eq!(out.error.as_deref(), Some("first"));
        assert!(out.text.is_empty());
    }

    /// reqwest 的 `Display` 只印外壳，真正的原因在 `source()` 链里——摊平函数
    /// 必须把链上每一环都带出来，否则报错永远只有那句没信息量的话。
    #[test]
    fn transport_detail_walks_the_source_chain() {
        #[derive(Debug)]
        struct Inner;
        impl std::fmt::Display for Inner {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "connection reset by peer")
            }
        }
        impl std::error::Error for Inner {}
        #[derive(Debug)]
        struct Outer(Inner);
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "error sending request")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let chain = error_chain(&Outer(Inner));
        assert!(chain.contains("error sending request"), "{chain}");
        assert!(chain.contains("connection reset by peer"), "{chain}");
    }

    fn model(id: &str) -> config::ModelChoice {
        config::ModelChoice {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            api_base_url: None,
            api_key: None,
            env_key: None,
            context_window: None,
            api_backends: vec![ApiBackend::ChatCompletions],
            backend_overrides: Default::default(),
            pricing: None,
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

    /// Stop 落在「请求已发出、响应头还没回」这个窗口时（插队发送也走这条：
    /// `send_now` 先 `request_cancel` 再跑新一轮），采样器不能伪造文本。
    ///
    /// 非空文本会被 `finish_llm` 填进本轮那条助手记录：历史里从此留着一句模型
    /// 没说过的话，`seal_incomplete_tool_calls` 也不再把空槽位弹掉，于是
    /// cancel-rewind 失效——提示词回不到输入框，用户那条消息和这句假话一起留在
    /// 上下文里。
    #[tokio::test]
    async fn cancelling_before_the_response_leaves_nothing_in_the_log() {
        let ctx = Context::new();
        crate::install_without_llm(&ctx).await.unwrap();
        ctx.plugin(crate::agent::turn::turn(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let sessions = ctx.require::<Sessions>(SESSIONS).unwrap();
        sessions.append(LogEvent::User("hi".into()));
        ctx.require::<TurnControl>(TURN).unwrap().cancel();

        let llm = crate::llm::sampler::Llm::from_sampler(
            ctx.clone(),
            std::sync::Arc::new(HttpSampler {
                ctx: ctx.clone(),
                api_key: "test-key".into(),
                // 不会真连出去：取消分支是 `biased` 的第一臂，先于 send 命中。
                api_base: "http://127.0.0.1:9/v1".into(),
                fallback_model: "test-model".into(),
            }),
        );
        let out = llm.stream_on(&ctx, empty_request()).await;

        assert!(out.text.is_empty(), "取消不该伪造文本：{out:?}");
        sessions.seal_incomplete_tool_calls();
        assert_eq!(
            sessions.kinds(),
            vec!["user"],
            "取消后只该剩用户那条：{:?}",
            sessions.events()
        );
        assert!(!sessions.last_turn_has_output());
        assert_eq!(
            sessions
                .rewind_inflight_user()
                .map(|(text, _)| text)
                .as_deref(),
            Some("hi"),
            "提示词该能还回输入框"
        );
    }

    /// 子代理的请求必须跑在**它自己**的 ctx 上。
    ///
    /// 采样器以前一律从 `sampler.ctx`（`llm()` 挂载时的根 ctx）找 `"sessions"` /
    /// `"turn"`，于是：`interrupt_agent` 取消的是子代理那把 `TurnControl`，可正在
    /// 跑的 HTTP 流听的是主会话那把，打不断；`model_user_images` 也会把主会话粘的
    /// 图按下标贴到子代理自己的消息上。
    ///
    /// 这里让子代理那把取消、主会话那把不取消：修好之前采样器会真的去连端点。
    #[tokio::test]
    async fn a_child_isolate_samples_with_its_own_turn() {
        let ctx = Context::new();
        crate::install_without_llm(&ctx).await.unwrap();
        ctx.plugin(crate::agent::turn::turn(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();

        let child = ctx.isolate("sessions").isolate("turn");
        let _s = child
            .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-1"))
            .unwrap();
        let child_turn = TurnControl::new();
        child_turn.cancel();
        let _t = child.provide(TURN, child_turn).unwrap();
        assert!(
            !ctx.require::<TurnControl>(TURN).unwrap().is_cancelled(),
            "主会话这把不能是取消的，否则测不出听的是哪一把"
        );

        let llm = crate::llm::sampler::Llm::from_sampler(
            ctx.clone(),
            std::sync::Arc::new(HttpSampler {
                ctx: ctx.clone(),
                api_key: "test-key".into(),
                // 听对了就根本不会连出去。
                api_base: "http://127.0.0.1:9/v1".into(),
                fallback_model: "test-model".into(),
            }),
        );
        let out = llm.stream_on(&child, empty_request()).await;

        assert!(out.text.is_empty(), "采样器听的还是主会话那把取消：{out:?}");
    }

    /// 目录里没这个模型（或压根没 config）时不替上游做任何假设。
    #[test]
    fn wire_params_default_to_sending_nothing() {
        let params = WireParams::resolve(None, None, None);
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
        let params = WireParams::resolve(Some(&choice), None, None);
        assert_eq!(params.effort, "high");
        assert_eq!(params.max_output_tokens, Some(8192));
        assert!(!params.images);

        choice.reasoning = Some(false);
        let params = WireParams::resolve(Some(&choice), None, None);
        assert_eq!(params.reasoning, Reasoning::Unsupported);
        assert!(params.effort.is_empty(), "不支持推理就不带强度");
    }

    /// 这次委派点名的强度 / 输出上限压过模型目录的默认值。
    ///
    /// 只有受限子会话挂得上 `"model-override"`（workflow 脚本的
    /// `agent(effort:, max_output_tokens:)`），主会话永远没有这一项。
    #[test]
    fn wire_params_take_the_delegation_override_first() {
        let mut choice = model("m");
        choice.reasoning_effort = Some("high".into());
        choice.max_output_tokens = Some(8192);
        let over = ModelOverride {
            model: Some("m".into()),
            effort: Some("low".into()),
            max_output_tokens: Some(1024),
        };
        let params = WireParams::resolve(Some(&choice), None, Some(&over));
        assert_eq!(params.effort, "low");
        assert_eq!(params.max_output_tokens, Some(1024));

        // 没点名的那几项照旧回落到模型目录。
        let partial = ModelOverride {
            model: Some("m".into()),
            ..ModelOverride::default()
        };
        let params = WireParams::resolve(Some(&choice), None, Some(&partial));
        assert_eq!(params.effort, "high");
        assert_eq!(params.max_output_tokens, Some(8192));

        // 模型不支持推理时，点名的强度也不发——那个字段对这个端点是未知字段。
        choice.reasoning = Some(false);
        let params = WireParams::resolve(Some(&choice), None, Some(&over));
        assert_eq!(params.reasoning, Reasoning::Unsupported);
        assert!(params.effort.is_empty());
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
                    is_error: false,
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
                    is_error: false,
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
                    is_error: false,
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
                    is_error: false,
                },
                LogEvent::ToolExecute {
                    id: "c2".into(),
                    name: "browser_screenshot".into(),
                    arguments: "{}".into(),
                    content: "shot\nImage content included inline".into(),
                    images: vec![img],
                    is_error: false,
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
                    is_error: false,
                },
                LogEvent::ToolExecute {
                    id: "c2".into(),
                    name: "computer_screenshot".into(),
                    arguments: "{}".into(),
                    content: "shot2".into(),
                    images: vec![img2],
                    is_error: false,
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
