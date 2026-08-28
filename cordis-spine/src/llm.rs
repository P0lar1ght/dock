use std::sync::Arc;

use cordis::{plugin, Context, Inject, Plugin};

use crate::names::{LLM, LLM_STREAM};
use crate::runtime::BoxFuture;
use crate::types::{LlmOutput, LogEvent, PromptRequest, ToolCall};

/// Echo: always `echo`. Workspace: `list_dir` / `read_file`. Text: no tools.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LlmMode {
    #[default]
    Echo,
    Workspace,
    Text,
}

#[derive(Clone, Debug, Default)]
pub struct LlmConfig {
    pub mode: LlmMode,
}

/// Swap this to change the protocol (stub, Grok chat / resp / anthropic, …).
pub trait Sampler: Send + Sync {
    fn sample<'a>(&'a self, request: PromptRequest) -> BoxFuture<'a, LlmOutput>;
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
        let output = self.sampler.sample(request).await;
        self.ctx
            .waterfall(LLM_STREAM, output.clone(), move || output)
    }
}

struct FakeSampler {
    mode: LlmMode,
}

impl Sampler for FakeSampler {
    fn sample<'a>(&'a self, request: PromptRequest) -> BoxFuture<'a, LlmOutput> {
        let mode = self.mode;
        Box::pin(async move { hardcoded(mode, &request) })
    }
}

pub fn llm() -> Plugin {
    plugin("llm", Inject::new(), |ctx, cfg: &LlmConfig| {
        Ok(Some(
            ctx.provide(LLM, Llm::fake(ctx.clone(), cfg.mode))?,
        ))
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
    has_word(user, "read") || has_word(user, "cat") || user.contains("读取") || user.contains("打开")
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
            LlmMode::Workspace | LlmMode::Text => content,
        };
        return LlmOutput {
            text,
            tool_calls: Vec::new(),
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
        },
        LlmMode::Workspace => match workspace_tool(&user) {
            Some(call) => LlmOutput {
                text: String::new(),
                tool_calls: vec![call],
            },
            None => LlmOutput {
                text: format!("no workspace tool for: {user}"),
                tool_calls: Vec::new(),
            },
        },
        LlmMode::Text => LlmOutput {
            text: format!(
                "ok · {user}\n\nharness is up. tools come later via the tools plugin."
            ),
            tool_calls: Vec::new(),
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
