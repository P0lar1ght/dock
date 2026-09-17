use std::sync::Arc;

use cordis::{plugin, Context, Inject, Plugin};

use crate::agent::runtime::BoxFuture;
use crate::llm::http::HttpSampler;
use crate::names::{LLM, LLM_STREAM, SESSIONS};
use crate::session::log::Sessions;
use cordis_base::stream_acc::StreamDelta;
use cordis_base::types::{LlmOutput, LogEvent, PromptRequest, ToolCall};

/// Echo: always `echo`. Workspace: `list_dir` / `read_file`. Text: no tools.
/// Http: 走 `api_backends` 声明的某条 wire（responses / chat/completions /
/// messages），具体哪条由运行时 `/protocol` 决定。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LlmMode {
    #[default]
    Echo,
    Workspace,
    Text,
    Http,
}

#[derive(Clone, Debug, Default)]
pub struct LlmConfig {
    pub mode: LlmMode,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub model: Option<String>,
}

impl LlmConfig {
    pub fn from_env() -> Self {
        let api_key = std::env::var("DOCK_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .or_else(|_| std::env::var("XAI_API_KEY"))
            .ok()
            .filter(|s| !s.trim().is_empty());
        let api_base = std::env::var("DOCK_API_BASE")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let model = std::env::var("DOCK_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(cordis_base::config::load_default_model);
        if api_key.is_some() || cordis_base::config::catalog_has_http() {
            Self {
                mode: LlmMode::Http,
                api_key,
                api_base,
                model,
            }
        } else {
            Self {
                mode: LlmMode::Workspace,
                api_key: None,
                api_base,
                model,
            }
        }
    }
}

tokio::task_local! {
    /// 正在采样的是**谁的** ctx。子代理跑在自己的隔离 ctx 上，而 `HttpSampler`
    /// 捏着的是 `llm()` 挂载时的根 ctx——采样器要是照那个去找 `"sessions"` /
    /// `"turn"`，子代理就会带上主会话的图片，它自己的 `TurnControl` 也没人听
    /// （`interrupt_agent` 打不断正在跑的那个 HTTP 流）。
    ///
    /// 走 task-local 而不是改 [`Sampler`] 的签名：那是 pub trait，动它得让所有
    /// 实现跟着改，而这里要传的是「当前调用方是谁」这种天然属于调用栈的东西。
    static SAMPLE_CTX: Context;
}

/// 当前采样调用方的 ctx。`None` = 没走 `Llm::stream_*`（直接调的 [`Sampler`]）。
pub(crate) fn sampling_ctx() -> Option<Context> {
    SAMPLE_CTX.try_with(|c| c.clone()).ok()
}

/// Swap this to change the protocol (stub, or HttpSampler's declared wires).
pub trait Sampler: Send + Sync {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput>;
}

/// Named `llm`. Loop live-looks this up; the inner [`Sampler`] is the adapter.
#[derive(Clone)]
pub struct Llm {
    ctx: Context,
    sampler: Arc<dyn Sampler>,
}

impl Llm {
    pub fn fake(ctx: Context, mode: LlmMode) -> Self {
        Self::from_sampler(ctx, Arc::new(FakeSampler { mode }))
    }

    pub fn from_sampler(ctx: Context, sampler: Arc<dyn Sampler>) -> Self {
        Self { ctx, sampler }
    }

    pub async fn stream(&self, request: PromptRequest) -> LlmOutput {
        self.stream_on(&self.ctx, request).await
    }

    /// Sample against the caller's ctx so a nested isolate can own `"sessions"`.
    pub async fn stream_on(&self, ctx: &Context, request: PromptRequest) -> LlmOutput {
        self.stream_observed(ctx, request, |_| {}).await
    }

    /// 同 [`stream_on`](Self::stream_on)，另给调用方一份 delta 的只读回调。
    ///
    /// 压缩那种隔离掉 `"sessions"` 的旁路采样，用量本来会随 delta 一起掉进
    /// 黑洞——没有会话就没人 `apply_llm_delta`，账本里一笔都不落。观察者让它
    /// 把 usage 捞出来自己记账，而不必为此假造一个会话。
    pub async fn stream_observed<F>(
        &self,
        ctx: &Context,
        request: PromptRequest,
        mut observe: F,
    ) -> LlmOutput
    where
        F: FnMut(&StreamDelta) + Send,
    {
        if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
            sessions.begin_llm();
        }
        let stream_ctx = ctx.clone();
        let sample = self.sampler.sample(
            request,
            Box::new(move |delta| {
                if let Some(sessions) = stream_ctx.get::<Sessions>(SESSIONS) {
                    sessions.apply_llm_delta(&delta);
                }
                observe(&delta);
            }),
        );
        let output = SAMPLE_CTX.scope(ctx.clone(), sample).await;
        if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
            sessions.finish_llm(&output);
        }
        self.ctx
            .waterfall(LLM_STREAM, output.clone(), move || output)
    }
}

struct FakeSampler {
    mode: LlmMode,
}

impl Sampler for FakeSampler {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let mode = self.mode;
        Box::pin(async move {
            let output = hardcoded(mode, &request);
            if !output.text.is_empty() {
                on_delta(StreamDelta::Text(output.text.clone()));
            }
            output
        })
    }
}

pub fn llm() -> Plugin {
    plugin("llm", Inject::new(), |ctx, cfg: &LlmConfig| {
        let provided = match cfg.mode {
            LlmMode::Http => {
                // 端点和模型都来自 config / env，这里不再替用户猜一个 xAI 地址
                // 和 "grok-4"：猜错了只会得到一条看不懂的 404。
                let sampler = HttpSampler {
                    ctx: ctx.clone(),
                    api_key: cfg.api_key.clone().unwrap_or_default(),
                    api_base: cfg.api_base.clone().unwrap_or_default(),
                    fallback_model: cfg.model.clone().unwrap_or_default(),
                };
                Llm::from_sampler(ctx.clone(), Arc::new(sampler))
            }
            mode => Llm::fake(ctx.clone(), mode),
        };
        Ok(Some(ctx.provide(LLM, provided)?))
    })
}

fn since_last_user(history: &[LogEvent]) -> &[LogEvent] {
    let start = history
        .iter()
        .rposition(|e| matches!(e, LogEvent::User(_)))
        .unwrap_or(0);
    &history[start..]
}

fn last_user(history: &[LogEvent]) -> String {
    history
        .iter()
        .rev()
        .find_map(|e| match e {
            LogEvent::User(text) => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn has_word(hay: &str, needle: &str) -> bool {
    hay.split(|c: char| !c.is_alphanumeric())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

fn looks_like_list(user: &str) -> bool {
    has_word(user, "list")
        || has_word(user, "ls")
        || has_word(user, "files")
        || has_word(user, "dir")
        || user.contains("列出")
        || user.contains("目录")
        || user.contains("文件")
}

fn looks_like_read(user: &str) -> bool {
    has_word(user, "read")
        || has_word(user, "cat")
        || user.contains("读取")
        || user.contains("打开")
}

fn workspace_tool(user: &str) -> Option<ToolCall> {
    if looks_like_list(user) {
        return Some(ToolCall {
            id: "call-1".into(),
            name: "list_dir".into(),
            arguments: r#"{"target_directory":"."}"#.into(),
        });
    }
    if looks_like_read(user) {
        return Some(ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: r#"{"target_file":"README.md"}"#.into(),
        });
    }
    None
}

fn hardcoded(mode: LlmMode, request: &PromptRequest) -> LlmOutput {
    let turn = since_last_user(&request.history);
    if let Some(content) = turn.iter().rev().find_map(|e| match e {
        LogEvent::ToolExecute { content, .. } => Some(content.clone()),
        _ => None,
    }) {
        let text = match mode {
            LlmMode::Echo => format!("echoed: {content}"),
            LlmMode::Workspace | LlmMode::Text | LlmMode::Http => content,
        };
        return LlmOutput {
            text,
            ..LlmOutput::default()
        };
    }
    let user = last_user(&request.history);
    match mode {
        LlmMode::Echo => LlmOutput {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                name: "echo".into(),
                arguments: user,
            }],
            ..LlmOutput::default()
        },
        LlmMode::Workspace => match workspace_tool(&user) {
            Some(call) => LlmOutput {
                text: String::new(),
                tool_calls: vec![call],
                ..LlmOutput::default()
            },
            None => LlmOutput {
                text: format!("no workspace tool for: {user}"),
                ..LlmOutput::default()
            },
        },
        LlmMode::Http => LlmOutput {
            text: format!("http sampler missing for: {user}"),
            ..LlmOutput::default()
        },
        LlmMode::Text => LlmOutput {
            text: format!("ok · {user}\n\nharness is up. tools come later via the tools plugin."),
            ..LlmOutput::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_prompt_picks_list_dir() {
        let out = hardcoded(
            LlmMode::Workspace,
            &PromptRequest {
                system: String::new(),
                history: vec![LogEvent::User("list the files in this repo".into())],
                tools: Vec::new(),
            },
        );
        assert_eq!(out.tool_calls[0].name, "list_dir");
    }

    #[test]
    fn text_mode_has_no_tools() {
        let out = hardcoded(
            LlmMode::Text,
            &PromptRequest {
                system: String::new(),
                history: vec![LogEvent::User("hello".into())],
                tools: Vec::new(),
            },
        );
        assert!(out.tool_calls.is_empty());
        assert!(out.text.contains("tools come later"));
    }

    #[test]
    fn listen_does_not_look_like_list() {
        let out = hardcoded(
            LlmMode::Workspace,
            &PromptRequest {
                system: String::new(),
                history: vec![LogEvent::User("please listen".into())],
                tools: Vec::new(),
            },
        );
        assert!(out.tool_calls.is_empty());
    }
}
