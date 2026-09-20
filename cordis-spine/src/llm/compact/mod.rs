//! Conversation compact as a Cordis plugin (`"compact"`).
//!
//! Grok files live next to this glue: [`config`], [`prompt`], [`summary`],
//! [`history`]. The loop live-looks this service at the start of each sample
//! (after tools flush). Summarization samples on an isolated `"sessions"`
//! realm so `begin_llm` does not append onto the live log.
//!
//! Grok pager keeps scrollback and only replaces `chat_state` conversation
//! (`replace_conversation_for_compaction`). Dock matches that: [`Sessions::events`]
//! stays the pager transcript; [`Sessions::model_history`] is what the sampler
//! sees.

mod config;
mod history;
mod prompt;
mod segments;
mod summary;

use cordis::{plugin, Context, Inject, Plugin};

use crate::agent::turn::TurnControl;
use crate::error::{Error, Result};
use crate::llm::sampler::Llm;
use crate::names::{COMPACT, LLM, SESSIONS, SKILLS, SYSTEM_PROMPT, TURN, WORKFLOWS};
use crate::prompt::assemble::SystemPrompt;
use crate::session::log::Sessions;
use crate::tools::skills::Skills;
use crate::tools::workflow::Workflows;
use cordis_base::stream_acc::StreamDelta;
use cordis_base::types::{LogEvent, PromptRequest};
use cordis_base::usage::TokenUsage;

use history::{build_compacted_events, prepare_conversation_for_summarization};

pub(crate) use history::{estimate_context_tokens, VISIBLE_NOTICE};
use prompt::{build_summary_prompt_kind, SummaryPromptKind};
use summary::is_degenerate_summary;

pub use config::{exceeds_threshold, FullReplaceConfig, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT};

/// 一次摘要采样的账单，从 SSE 的 `usage` delta 里捞出来的。
struct SideCall {
    usage: TokenUsage,
    model: String,
    cost_usd_ticks: Option<i64>,
}

/// Named `"compact"`. Stateless; per-session flags live on [`Sessions`].
#[derive(Clone, Default)]
pub struct Compact;

impl Compact {
    /// Manual `/compact [ctx]`. Never suppressed.
    pub async fn run_on(&self, ctx: &Context, extra: Option<&str>) -> Result<()> {
        compact_session(ctx, extra, FullReplaceConfig::default()).await
    }

    /// If the live log is at or above the threshold, compact it.
    /// Returns `true` when a compact ran.
    pub async fn maybe_auto(&self, ctx: &Context) -> Result<bool> {
        let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
            return Ok(false);
        };
        if sessions.auto_compact_suppressed() {
            return Ok(false);
        }
        let system = ctx
            .get::<SystemPrompt>(SYSTEM_PROMPT)
            .map(|p| p.assemble_on(ctx))
            .unwrap_or_default();
        let used = context_tokens_used(&sessions, &system);
        let window = sessions.usage().window;
        if !exceeds_threshold(used, window, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT) {
            return Ok(false);
        }
        match compact_session(ctx, None, FullReplaceConfig::default()).await {
            Ok(()) => {
                let used_after = context_tokens_used(&sessions, &system);
                if exceeds_threshold(
                    used_after,
                    sessions.usage().window,
                    DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT,
                ) {
                    sessions.set_auto_compact_suppressed(true);
                }
                Ok(true)
            }
            Err(Error::Cancelled) => Err(Error::Cancelled),
            Err(err) => {
                sessions.set_auto_compact_suppressed(true);
                Err(err)
            }
        }
    }
}

pub fn compact() -> Plugin {
    plugin("compact", Inject::from([SESSIONS, LLM]), |ctx, _: &()| {
        Ok(Some(ctx.provide(COMPACT, Compact)?))
    })
}

fn context_tokens_used(sessions: &Sessions, system: &str) -> u64 {
    let estimate = estimate_context_tokens(system, &sessions.model_history());
    let u = sessions.usage();
    if u.official {
        u.prompt.max(estimate)
    } else {
        estimate
    }
}

async fn compact_session(ctx: &Context, extra: Option<&str>, cfg: FullReplaceConfig) -> Result<()> {
    let sessions = ctx.require::<Sessions>(SESSIONS)?;
    if cancelled(ctx) {
        return Err(Error::Cancelled);
    }
    if !sessions.try_begin_compact() {
        return Err(Error::Compact("正在压缩上下文。".into()));
    }
    let result = compact_session_locked(ctx, &sessions, extra, &cfg).await;
    sessions.end_compact();
    result
}

async fn compact_session_locked(
    ctx: &Context,
    sessions: &Sessions,
    extra: Option<&str>,
    cfg: &FullReplaceConfig,
) -> Result<()> {
    let history = sessions.model_history();
    if !history
        .iter()
        .any(|e| matches!(e, LogEvent::User(t) if !t.trim().is_empty()))
    {
        return Err(Error::Compact("没有可压缩的对话。".into()));
    }
    let llm = ctx.require::<Llm>(LLM)?;
    let mut request_history = prepare_conversation_for_summarization(&history);
    request_history.push(LogEvent::User(build_summary_prompt_kind(
        SummaryPromptKind::Structured,
        extra.map(str::trim).filter(|s| !s.is_empty()),
    )));
    let iso = ctx.isolate("sessions");
    let system = ctx
        .get::<SystemPrompt>(SYSTEM_PROMPT)
        .map(|p| p.assemble_on(ctx))
        .unwrap_or_default();
    let mut last_err = Error::Compact("压缩失败：摘要为空。".into());
    for attempt in 0..cfg.max_attempts {
        if cancelled(ctx) {
            return Err(Error::Cancelled);
        }
        if attempt > 0 && cfg.retry_delay_secs > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(cfg.retry_delay_secs)).await;
        }
        // 压缩的 usage 只在 delta 里出现一次，而这条采样隔离掉了 `"sessions"`，
        // 没人会替它记账——自己捞出来，跑完折进账本的旁路那一格。
        let billed: std::sync::Arc<std::sync::Mutex<Option<SideCall>>> = Default::default();
        let started = std::time::Instant::now();
        let output = llm
            .stream_observed(
                &iso,
                PromptRequest {
                    system: system.clone(),
                    history: request_history.clone(),
                    // 摘要不该调工具（调了下面就判失败），带着整张工具表只是白
                    // 付几千 token，还会让「带 tools 就必须回传 reasoning_content」
                    // 这类上游约束平白多一处触发点。Grok 这里是带的，这条是有意
                    // 偏离。
                    tools: Vec::new(),
                },
                {
                    let billed = billed.clone();
                    move |delta| {
                        if let StreamDelta::Usage {
                            tokens,
                            official: true,
                            model,
                            cost_usd_ticks,
                        } = delta
                        {
                            *billed.lock().unwrap() = Some(SideCall {
                                usage: tokens.clone(),
                                model: model.clone(),
                                cost_usd_ticks: *cost_usd_ticks,
                            });
                        }
                    }
                },
            )
            .await;
        // 失败的那几次尝试同样花了钱，所以记账在判定之前。
        if let Some(call) = billed.lock().unwrap().take() {
            sessions.record_side_call(
                &call.model,
                &call.usage,
                Some(started.elapsed().as_millis() as u64),
                call.cost_usd_ticks,
            );
        }
        if cancelled(ctx) {
            return Err(Error::Cancelled);
        }
        if !output.tool_calls.is_empty() {
            last_err = Error::Compact("压缩失败：模型调用了工具。".into());
            continue;
        }
        if output.text.trim().is_empty() {
            last_err = Error::Compact("压缩失败：摘要为空。".into());
            continue;
        }
        if is_degenerate_summary(&output.text) {
            last_err = Error::Compact("压缩失败：摘要过短。".into());
            continue;
        }
        let compacted = build_compacted_events(&history, &output.text);
        if let Some(dir) = sessions.disk_session_dir() {
            // Fail-open: compact already succeeded in memory.
            let _ = segments::persist_compaction_segment(&dir, &history, &output.text);
        }
        sessions.replace_compacted(compacted);
        re_announce_discoveries(ctx, sessions);
        return Ok(());
    }
    Err(last_err)
}

/// 压缩之后把「中途发现的技能 / 工作流」再说一遍。
///
/// 两边的 listing 段都是**冻结**的（系统提示不重写，为的是保住前缀缓存），所以
/// 中途在磁盘上发现的技能 / 工作流只靠历史里那条 `SystemReminder` 让模型知道。
/// 压缩把它抹掉之后，发现集合又认为"已经通知过了"，模型就永久失去了这些条目。
fn re_announce_discoveries(ctx: &Context, sessions: &Sessions) {
    let mut lines = Vec::new();
    if let Some(skills) = ctx.get::<Skills>(SKILLS) {
        if let Some(text) = skills.rediscovery_notice() {
            lines.push(text);
        }
    }
    if let Some(workflows) = ctx.get::<Workflows>(WORKFLOWS) {
        if let Some(text) = workflows.rediscovery_notice() {
            lines.push(text);
        }
    }
    for line in lines {
        sessions.append(LogEvent::SystemReminder(line));
    }
}

fn cancelled(ctx: &Context) -> bool {
    ctx.get::<TurnControl>(TURN)
        .is_some_and(|t| t.is_cancelled())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::runtime::BoxFuture;
    use crate::llm::sampler::{Llm, Sampler};
    use cordis_base::stream_acc::StreamDelta;
    use cordis_base::types::{LlmOutput, ToolCall};
    use std::sync::Arc;

    struct FixedSummary(String);

    impl Sampler for FixedSummary {
        fn sample<'a>(
            &'a self,
            _request: PromptRequest,
            mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            let text = self.0.clone();
            Box::pin(async move {
                on_delta(StreamDelta::Text(text.clone()));
                LlmOutput {
                    text,
                    ..LlmOutput::default()
                }
            })
        }
    }

    fn healthy_summary() -> String {
        format!(
            "<summary>\n1. Primary Request and Intent: fix auth.rs login and add a test.\n2. Key Technical Concepts: rust\n3. Files and Code Sections: src/auth.rs\n4. Errors and Fixes: None\n5. Problem Solving: None\n6. All User Messages: fix login; add a test\n7. Pending Tasks: add a test\n8. Current Work: read auth.rs\n9. Optional Next Step: write the test\n{}\n</summary>",
            "detail ".repeat(80)
        )
    }

    fn sample_history() -> Vec<LogEvent> {
        vec![
            LogEvent::User("fix auth.rs login".into()),
            LogEvent::PreStep,
            LogEvent::Prompt("you are dock".into()),
            LogEvent::LlmStream(LlmOutput {
                text: "reading".into(),
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "read_file".into(),
                    arguments: r#"{"target_file":"src/auth.rs"}"#.into(),
                }],
                ..LlmOutput::default()
            }),
            LogEvent::ToolExecute {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: r#"{"target_file":"src/auth.rs"}"#.into(),
                content: "fn login() { buggy }".into(),

                images: Vec::new(),
            },
            LogEvent::User("also add a test".into()),
        ]
    }

    #[tokio::test]
    async fn compact_keeps_display_log_and_hides_sample() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        for e in sample_history() {
            sessions.append(e);
        }
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        root.provide(
            LLM,
            Llm::from_sampler(root.clone(), Arc::new(FixedSummary(healthy_summary()))),
        )
        .unwrap();
        Compact
            .run_on(&root, Some("keep the test plan"))
            .await
            .unwrap();
        let events = sessions.events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                LogEvent::ToolExecute { content, .. } if content.contains("buggy")
            )),
            "pager must keep the original transcript: {events:?}"
        );
        assert!(events.iter().any(|e| matches!(
            e,
            LogEvent::LlmStream(o) if o.text == VISIBLE_NOTICE
        )));
        let model = sessions.model_history();
        assert!(
            model
                .iter()
                .any(|e| matches!(e, LogEvent::SystemReminder(t) if t.contains("auth.rs"))),
            "{model:?}"
        );
        assert!(!model.iter().any(|e| matches!(
            e,
            LogEvent::ToolExecute { content, .. } if content.contains("buggy")
        )));
        assert_eq!(
            events
                .iter()
                .filter(
                    |e| matches!(e, LogEvent::LlmStream(o) if o.text.contains("Primary Request"))
                )
                .count(),
            0,
            "summarizer sample must not land on the live log"
        );
    }

    /// 带 usage 的摘要采样：真实 wire 在流里报一次 `usage`，压缩这条采样隔离掉
    /// 了 `"sessions"`，没人替它记账。
    struct BilledSummary(String);

    impl Sampler for BilledSummary {
        fn sample<'a>(
            &'a self,
            _request: PromptRequest,
            mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            let text = self.0.clone();
            Box::pin(async move {
                on_delta(StreamDelta::Usage {
                    tokens: TokenUsage {
                        prompt_tokens: 90_000,
                        completion_tokens: 900,
                        total_tokens: 90_900,
                        cached_prompt_tokens: 0,
                        ..TokenUsage::default()
                    },
                    official: true,
                    model: "summarizer".into(),
                    cost_usd_ticks: None,
                });
                on_delta(StreamDelta::Text(text.clone()));
                LlmOutput {
                    text,
                    ..LlmOutput::default()
                }
            })
        }
    }

    /// 压缩是**一次满价、几乎零命中的整段历史请求**，而且它之后那一轮必然全量
    /// 重算。它隔离掉 `"sessions"`，所以既不 `begin_llm` 也不 `finish_llm`，
    /// 用量以前整笔掉进黑洞：`/usage` 里看不到开销，命中率掉格也无从归因。
    #[tokio::test]
    async fn compaction_is_billed_without_counting_as_a_turn() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        for e in sample_history() {
            sessions.append(e);
        }
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        root.provide(
            LLM,
            Llm::from_sampler(root.clone(), Arc::new(BilledSummary(healthy_summary()))),
        )
        .unwrap();

        Compact.run_on(&root, None).await.unwrap();

        let usage = sessions.prompt_usage();
        let side = usage.side.clone().expect("压缩没进账本");
        assert_eq!(side.input_tokens, 90_000, "{usage:?}");
        assert_eq!(side.output_tokens, 900);
        assert_eq!(side.model_calls, 1);
        assert_eq!(usage.totals.input_tokens, 90_000, "会话总计要含压缩");
        assert_eq!(usage.num_turns, 0, "压缩不是用户的一轮");
        assert!(usage.recent_calls.is_empty(), "压缩不进每轮命中率走势");
        assert!(
            cordis_base::usage::calls_breakdown(&usage).contains("压缩 1"),
            "{}",
            cordis_base::usage::calls_breakdown(&usage)
        );
    }

    /// 记下摘要请求长什么样。
    #[derive(Default)]
    struct CapturedRequest(std::sync::Mutex<Option<PromptRequest>>);

    struct Capturing(Arc<CapturedRequest>, String);

    impl Sampler for Capturing {
        fn sample<'a>(
            &'a self,
            request: PromptRequest,
            mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            *self.0 .0.lock().unwrap() = Some(request);
            let text = self.1.clone();
            Box::pin(async move {
                on_delta(StreamDelta::Text(text.clone()));
                LlmOutput {
                    text,
                    ..LlmOutput::default()
                }
            })
        }
    }

    /// 摘要调用不带工具表：摘要调工具本来就判失败，带着只是白付 token，
    /// 并且给「带 tools 就必须回传 reasoning_content」这类上游约束多一处触发点。
    #[tokio::test]
    async fn summary_request_carries_no_tools() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        for e in sample_history() {
            sessions.append(e);
        }
        let _s = root.provide(SESSIONS, sessions).unwrap();
        let seen = Arc::new(CapturedRequest::default());
        root.provide(
            LLM,
            Llm::from_sampler(
                root.clone(),
                Arc::new(Capturing(seen.clone(), healthy_summary())),
            ),
        )
        .unwrap();
        Compact.run_on(&root, None).await.unwrap();
        let request = seen.0.lock().unwrap().take().expect("sampled once");
        assert!(request.tools.is_empty(), "{:?}", request.tools);
    }

    #[tokio::test]
    async fn plugin_provides_named_compact() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        let _s = root.provide(SESSIONS, sessions).unwrap();
        root.provide(
            LLM,
            Llm::from_sampler(root.clone(), Arc::new(FixedSummary(healthy_summary()))),
        )
        .unwrap();
        root.plugin(compact(), ()).unwrap().wait().await.unwrap();
        assert!(root.get::<Compact>(COMPACT).is_some());
    }

    #[tokio::test]
    async fn auto_compact_fires_at_threshold() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(80);
        sessions.append(LogEvent::User("x".repeat(400)));
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
            content: "y".repeat(400),

            images: Vec::new(),
        });
        let used = estimate_context_tokens("", &sessions.events());
        assert!(
            exceeds_threshold(used, 80, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT),
            "seed should exceed 85% of window used={used}"
        );
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        root.provide(
            LLM,
            Llm::from_sampler(root.clone(), Arc::new(FixedSummary(healthy_summary()))),
        )
        .unwrap();
        assert!(Compact.maybe_auto(&root).await.unwrap());
        assert!(sessions.events().iter().any(|e| matches!(
            e,
            LogEvent::ToolExecute { content, .. } if content.contains('y')
        )));
        assert!(!sessions.model_history().iter().any(|e| matches!(
            e,
            LogEvent::ToolExecute { content, .. } if content.contains('y')
        )));
    }
}
