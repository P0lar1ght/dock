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

mod command;
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
use crate::prompt::context_usage::context_tokens_used;
use crate::session::compaction::{CompactPhase, CompactStatus, CompactTrigger, OutputCounter};
use crate::session::log::Sessions;
use crate::session::persist;
use crate::tools::skills::Skills;
use crate::tools::workflow::Workflows;
use cordis_base::stream_acc::StreamDelta;
use cordis_base::types::{LogEvent, PromptRequest};
use cordis_base::usage::TokenUsage;

use crate::tools::memory::maybe_flush_before_compact;
use history::{
    build_compacted_events, prepare_conversation_for_summarization, SUMMARY_TOOL_OUTPUT_MAX_ENTRIES,
};

pub use history::estimate_text_tokens;
pub(crate) use history::{estimate_context_tokens, estimate_reasoning, VISIBLE_NOTICE};
use prompt::{build_summary_prompt_kind, SummaryPromptKind};
use summary::is_degenerate_summary;

pub use command::compact_command;
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
        compact_session(
            ctx,
            extra,
            FullReplaceConfig::default(),
            CompactTrigger::Manual,
        )
        .await
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
        let used = context_tokens_used(ctx, &system);
        let window = sessions.usage().window;
        if !exceeds_threshold(used, window, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT) {
            return Ok(false);
        }
        match compact_session(
            ctx,
            None,
            FullReplaceConfig::default(),
            CompactTrigger::Auto,
        )
        .await
        {
            Ok(()) => {
                let used_after = context_tokens_used(ctx, &system);
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

/// 这一页的上下文是否已到自动压缩的门槛（[`Compact::maybe_auto`] 用的同一把尺）。
/// 回合末的 todo 记账步不压缩，到了门槛就干脆不记账：记账可有可无，不值得为它
/// 在收尾时压缩一次或撑爆窗口；下一轮第一步照常自动压缩。
pub(crate) fn at_auto_threshold(ctx: &Context, system: &str) -> bool {
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return false;
    };
    exceeds_threshold(
        context_tokens_used(ctx, system),
        sessions.usage().window,
        DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT,
    )
}

pub fn compact() -> Plugin {
    plugin("compact", Inject::from([SESSIONS, LLM]), |ctx, _: &()| {
        Ok(Some(ctx.provide(COMPACT, Compact)?))
    })
}

async fn compact_session(
    ctx: &Context,
    extra: Option<&str>,
    cfg: FullReplaceConfig,
    trigger: CompactTrigger,
) -> Result<()> {
    let sessions = ctx.require::<Sessions>(SESSIONS)?;
    if cancelled(ctx) {
        return Err(Error::Cancelled);
    }
    if !sessions.try_begin_compact() {
        return Err(Error::Compact("正在压缩上下文。".into()));
    }
    let system = ctx
        .get::<SystemPrompt>(SYSTEM_PROMPT)
        .map(|p| p.assemble_on(ctx))
        .unwrap_or_default();
    sessions.compaction_start(trigger, cfg.max_attempts, context_tokens_used(ctx, &system));
    let result = compact_session_locked(ctx, &sessions, extra, &cfg, &system).await;
    let (status, after) = match &result {
        Ok(()) => (
            CompactStatus::Completed,
            Some(context_tokens_used(ctx, &system)),
        ),
        Err(Error::Cancelled) => (CompactStatus::Cancelled, None),
        Err(err) => (CompactStatus::Failed(err.to_string()), None),
    };
    sessions.compaction_finish(status, after);
    sessions.end_compact();
    result
}

/// 失败原因去掉「压缩失败：」和句号，给重试提示用（`摘要过短`）。
fn retry_reason(err: &Error) -> String {
    let text = err.to_string();
    let text = text.strip_prefix("压缩失败：").unwrap_or(&text);
    text.trim_end_matches('。').to_string()
}

async fn compact_session_locked(
    ctx: &Context,
    sessions: &Sessions,
    extra: Option<&str>,
    cfg: &FullReplaceConfig,
    system: &str,
) -> Result<()> {
    let history = sessions.model_history();
    if !history
        .iter()
        .any(|e| matches!(e, LogEvent::User(t) if !t.trim().is_empty()))
    {
        return Err(Error::Compact("没有可压缩的对话。".into()));
    }
    // Memory flush writes only under $DOCK_HOME/memory/; compact segments stay
    // under sessions/.../compaction/ (A7). Fail-open if flush errors.
    maybe_flush_before_compact(ctx).await;
    let llm = ctx.require::<Llm>(LLM)?;
    let prompt = build_summary_prompt_kind(
        SummaryPromptKind::Structured,
        extra.map(str::trim).filter(|s| !s.is_empty()),
    );
    // 摘要请求带着截断后的工具输出，先按窗口收到放得下（见 [`fit_summary_request`]）。
    let budget = sessions.usage().window * SUMMARY_INPUT_BUDGET_PERCENT / 100;
    let (mut keep, mut request_history) = fit_summary_request(
        &history,
        &prompt,
        system,
        budget,
        SUMMARY_TOOL_OUTPUT_MAX_ENTRIES,
    );
    let iso = ctx.isolate("sessions");
    let mut last_err = Error::Compact("压缩失败：摘要为空。".into());
    for attempt in 0..cfg.max_attempts {
        if cancelled(ctx) {
            return Err(Error::Cancelled);
        }
        sessions.compaction_attempt(attempt + 1, (attempt > 0).then(|| retry_reason(&last_err)));
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
                    system: system.to_string(),
                    history: request_history.clone(),
                    // 摘要不该调工具（调了下面就判失败），带着整张工具表只是白
                    // 付几千 token，还会让「带 tools 就必须回传 reasoning_content」
                    // 这类上游约束平白多一处触发点。Grok 这里是带的，这条是有意
                    // 偏离。
                    tools: Vec::new(),
                },
                {
                    let billed = billed.clone();
                    let sessions = sessions.clone();
                    let mut counter = OutputCounter::default();
                    move |delta| {
                        if let StreamDelta::Text(text) | StreamDelta::Reasoning(text) = delta {
                            sessions.compaction_output(counter.push(text));
                        }
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
            // 请求本身失败（限流、断网）时说真正的原因，别都报成「摘要为空」。
            last_err = Error::Compact(match output.error.as_deref() {
                Some(err) => format!("压缩失败：{err}"),
                None => "压缩失败：摘要为空。".into(),
            });
            // 摘要请求本身超了窗口（预算是估算，上游的输出上限也算在窗口里）：原样
            // 重发结论一样，下一次少带一半工具输出。别的错误（限流、断网）照原样重试。
            if keep > 0 && output.error.as_deref().is_some_and(is_context_overflow) {
                keep /= 2;
                request_history = summary_request(&history, &prompt, keep);
            }
            continue;
        }
        if is_degenerate_summary(&output.text) {
            last_err = Error::Compact("压缩失败：摘要过短。".into());
            continue;
        }
        // 压不小就别压（DSH `compaction-basic` 的收紧检查：替换体的估算 token
        // 必须小于被遮住的那一段）。压缩会改写历史前缀、作废上游的 KV 缓存，
        // 下一次采样必然全量重算 —— 换来一个不更小的历史是纯亏。真实场景是
        // 刚压缩完、或这一轮本来就没跑多少工具，摘要 + 首末用户消息反而更长。
        //
        // 这条判定**不能落进重试**：它是确定性的，同一段历史同一份摘要重跑几次
        // 结论一样，重试只会白烧两次摘要调用。所以 `continue` 前直接返回。
        // 段文件路径要写进摘要末尾，模型才能在摘要不够用时自己去查原文
        // （Grok `format_transcript_location`）。收紧检查用**不带指针**的替换体过
        // （指针约 400 字节，对判定没有意义），真被采纳之后才落盘 —— 被拒绝的
        // 压缩不能在 `compaction/` 下留一份孤儿段文件和一行 INDEX 假记录；落盘
        // 失败则退回不带指针，别让模型去找一个不存在的文件。
        let compacted = build_compacted_events(&history, &output.text, None, None);
        let before = estimate_context_tokens("", &history);
        let after = estimate_context_tokens("", &compacted);
        if after >= before {
            return Err(Error::Compact(format!(
                "压缩失败：摘要不比原文小（{before} → {after} token）。"
            )));
        }
        sessions.compaction_phase(CompactPhase::Apply);
        // 判定通过之后才取目录：主会话的取法会顺手建目录、分配会话 id。子代理的
        // 日志在父会话目录的 `subagents/<id>/` 下，也要有段文件和指针。
        let dir = sessions.transcript_dir();
        let transcript = dir.as_deref().and_then(|dir| {
            let index = segments::persist_compaction_segment(dir, &history, &output.text).ok()?;
            Some(segments::compaction_dir(dir).join(segments::segment_filename(index)))
        });
        let raw_log = dir.as_deref().map(persist::history_path);
        let compacted = build_compacted_events(
            &history,
            &output.text,
            transcript.as_deref(),
            raw_log.as_deref(),
        );
        sessions.replace_compacted(compacted);
        sessions.bump_compaction_count();
        re_announce_discoveries(ctx, sessions);
        return Ok(());
    }
    Err(last_err)
}

/// 摘要请求的输入最多占窗口的这么多（估算，含系统提示）。
///
/// 自动压缩在 85% 触发，而摘要请求带着截断后的工具输出，可以接近原历史的大小；
/// 剩下的要装摘要的输出上限（Messages 不配时默认 16384）和估算误差。
const SUMMARY_INPUT_BUDGET_PERCENT: u64 = 70;

/// 摘要请求：历史（最多带最近 `keep` 条工具输出）+ 末尾那条摘要 prompt。
fn summary_request(history: &[LogEvent], prompt: &str, keep: usize) -> Vec<LogEvent> {
    let mut request = prepare_conversation_for_summarization(history, keep);
    request.push(LogEvent::User(prompt.to_string().into()));
    request
}

/// 从 `start` 条工具输出起减半，直到摘要请求的估算放得进 `budget`（token）。
/// 一条都不带还放不下就照发（剩下的是用户消息和 assistant 正文，没有更小的了），
/// 由上游报错、走 [`is_context_overflow`] 那条路。`budget` 为 0 是不知道窗口，不收。
fn fit_summary_request(
    history: &[LogEvent],
    prompt: &str,
    system: &str,
    budget: u64,
    start: usize,
) -> (usize, Vec<LogEvent>) {
    // 从实际条数起算：只有 8 条时从 40 减半到 20 等于没收（超限之后的减半同理）。
    let outputs = history
        .iter()
        .filter(|e| matches!(e, LogEvent::ToolExecute { .. }))
        .count();
    let mut keep = start.min(outputs);
    loop {
        let request = summary_request(history, prompt, keep);
        if budget == 0 || keep == 0 || estimate_context_tokens(system, &request) <= budget {
            return (keep, request);
        }
        keep /= 2;
    }
}

/// 上游的报错是不是「输入超了上下文窗口」。各家措辞不一，只认几种常见写法；
/// 认不出就当普通失败原样重试（宁可少收一次，也别把限流当超限把工具输出砍掉）。
fn is_context_overflow(err: &str) -> bool {
    let err = err.to_ascii_lowercase();
    [
        "context_length_exceeded",
        "maximum context length",
        "context window",
        "context limit",
        "prompt is too long",
        "input is too long",
        "too many tokens",
        "exceeds the maximum number of tokens",
        "reduce the length",
    ]
    .iter()
    .any(|needle| err.contains(needle))
}

/// 压缩之后把「中途发现的技能 / 工作流」再说一遍。
///
/// 两边的目录文本都是**冻结**的（reminder 不随激活集合重写），所以中途在磁盘上
/// 发现的技能 / 工作流只靠历史里那条 `SystemReminder` 让模型知道。压缩把它抹掉
/// 之后，发现集合又认为"已经通知过了"，模型就永久失去了这些条目。
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

    /// 记录摘要请求的 sampler：拿得到摘要模型实际看到的 history。
    struct RecordingSummary {
        requests: Arc<std::sync::Mutex<Vec<PromptRequest>>>,
    }

    impl Sampler for RecordingSummary {
        fn sample<'a>(
            &'a self,
            request: PromptRequest,
            mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            self.requests.lock().unwrap().push(request);
            let text = healthy_summary();
            Box::pin(async move {
                on_delta(StreamDelta::Text(text.clone()));
                LlmOutput {
                    text,
                    ..LlmOutput::default()
                }
            })
        }
    }

    fn with_recording_summary(root: &Context) -> Arc<std::sync::Mutex<Vec<PromptRequest>>> {
        let requests: Arc<std::sync::Mutex<Vec<PromptRequest>>> = Default::default();
        let sampler = RecordingSummary {
            requests: requests.clone(),
        };
        root.provide(LLM, Llm::from_sampler(root.clone(), Arc::new(sampler)))
            .unwrap();
        requests
    }

    /// 一段够长的提醒，把历史撑到「摘要确实更小」的规模 —— 收紧检查是
    /// 真判定，小样本会被正确地拒绝。
    fn filler_reminder() -> LogEvent {
        LogEvent::SystemReminder("filler context. ".repeat(400))
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
                // 比摘要大得多：压缩后历史必须真的更小，见 `compact_session_locked`
                // 的收紧检查。
                content: format!("fn login() {{ buggy }}{}", "src line\n".repeat(900)),

                images: Vec::new(),
                is_error: false,
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
        sessions.append(LogEvent::User("x".repeat(4_000).into()));
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
            content: "y".repeat(4_000),

            images: Vec::new(),
            is_error: false,
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

    /// 压不小就别压（DSH `compaction-basic` 的收紧检查）。
    ///
    /// 压缩改写历史前缀、作废上游 KV 缓存，下一次采样必然全量重算。换回来一个
    /// 不更小的历史是纯亏 —— 历史本来就没多少内容时最容易撞上（刚压完、或这
    /// 一轮只跑了一两步）。此时判失败、历史不动，等真正有东西可压再来。
    #[tokio::test]
    async fn compaction_is_refused_when_the_summary_is_not_smaller() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(1_000_000);
        sessions.append(LogEvent::User("go".into()));
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
            content: "tiny".into(),
            images: Vec::new(),
            is_error: false,
        });
        let before = sessions.model_history();
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        with_summary_llm(&root);

        let err = Compact.run_on(&root, None).await.unwrap_err();

        assert!(
            err.to_string().contains("不比原文小"),
            "应当拒绝这次压缩，实际：{err}"
        );
        // 历史一点没动：没有压缩前缀，也没有「已压缩上下文」那条通知。
        assert_eq!(sessions.model_history(), before);
        assert!(sessions.compaction().is_none() || !sessions.compaction().unwrap().running());
    }

    /// 回归：被拒绝的压缩不能在 `compaction/` 下留孤儿段文件。
    ///
    /// 摘要末尾的指针要写段文件路径，路径必须先算；但**落盘**只该发生在压缩
    /// 真被采纳之后，否则每次「压不小」都会留下一份没人引用的 `segment_NNN.md`
    /// 和一行 INDEX 假记录。
    #[tokio::test]
    async fn refused_compaction_leaves_no_segment_file() {
        let home = tempfile::tempdir().unwrap();
        let _env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path());

        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(1_000_000);
        sessions.attach_disk();
        sessions.append(LogEvent::User("go".into()));
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
            content: "tiny".into(),
            images: Vec::new(),
            is_error: false,
        });
        let dir = sessions.disk_session_dir().expect("已 attach_disk");
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        with_summary_llm(&root);

        let err = Compact.run_on(&root, None).await.unwrap_err();

        assert!(err.to_string().contains("不比原文小"), "实际：{err}");
        let compaction = segments::compaction_dir(&dir);
        let leftovers: Vec<_> = std::fs::read_dir(&compaction)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            leftovers.is_empty(),
            "被拒绝的压缩不该落盘，实际留下：{leftovers:?}"
        );
    }

    /// 回归：摘要请求必须带工具调用的**实参**。
    ///
    /// 摘要 prompt 要求「列出已经跑过哪些工具及其实参」，而压缩后的模型历史里
    /// 一条工具轮次都不留 —— 摘要模型看不到实参就只能编，编出来的那份是继承者
    /// 唯一的依据。
    #[tokio::test]
    async fn summary_request_carries_tool_arguments() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(1_000_000);
        sessions.append(LogEvent::User("go".into()));
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"cargo test -p cordis-spine"}"#.into(),
            content: "test result: ok. 1195 passed".into(),
            images: Vec::new(),
            is_error: false,
        });
        sessions.append(filler_reminder());
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        let requests = with_recording_summary(&root);

        Compact.run_on(&root, None).await.unwrap();

        let sent = requests.lock().unwrap();
        let request = sent.first().expect("发过一次摘要请求");
        let shown: Vec<String> = request
            .history
            .iter()
            .filter_map(|e| match e {
                LogEvent::SystemReminder(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        let tool_rows: Vec<&String> = shown.iter().filter(|t| t.starts_with("[bash")).collect();
        assert_eq!(tool_rows.len(), 1, "工具结果只该是一条提醒：{shown:?}");
        assert!(
            tool_rows[0].contains("cargo test -p cordis-spine"),
            "摘要请求里没有工具实参：{tool_rows:?}"
        );
    }

    /// 回归：工具结果必须是 `SystemReminder`，不能是 `User`。
    ///
    /// 三条 wire 都按「请求里第 i 个 `User`」取图片槽，而图片槽按**真实用户消息**
    /// 排。工具结果若是 `User`，后面每条真实用户消息的图都会被顶歪，最后一张还会
    /// 落到摘要 prompt 那条 `User` 上。
    #[tokio::test]
    async fn summary_request_keeps_user_rows_aligned_with_images() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(1_000_000);
        sessions.append(LogEvent::User("第一条".into()));
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
            content: "test result: ok".into(),
            images: Vec::new(),
            is_error: false,
        });
        sessions.append(LogEvent::User("第二条".into()));
        sessions.append(filler_reminder());
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        let requests = with_recording_summary(&root);

        Compact.run_on(&root, None).await.unwrap();

        let sent = requests.lock().unwrap();
        let request = sent.first().expect("发过一次摘要请求");
        let users: Vec<&str> = request
            .history
            .iter()
            .filter_map(|e| match e {
                LogEvent::User(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect();
        // 两条真实用户消息（后者被 `<user_query>` 包过）+ 末尾的摘要 prompt。
        assert_eq!(users.len(), 3, "多出来的 `User` 会顶歪图槽：{users:?}");
        assert_eq!(users[0], "第一条");
        assert!(users[1].contains("第二条"), "{users:?}");
    }

    /// 回归：摘不比原文小是确定性结论，重试只会重复烧钱。
    #[tokio::test]
    async fn refusal_does_not_retry_the_summary_call() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(1_000_000);
        sessions.append(LogEvent::User("go".into()));
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
            content: "tiny".into(),
            images: Vec::new(),
            is_error: false,
        });
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        let requests = with_recording_summary(&root);

        let err = Compact.run_on(&root, None).await.unwrap_err();

        assert!(err.to_string().contains("不比原文小"), "实际：{err}");
        assert_eq!(
            requests.lock().unwrap().len(),
            1,
            "被拒绝的压缩不该重试摘要调用"
        );
    }

    /// 一段工具循环：一条用户消息后跟 `n` 次 bash，第 `i` 次的输出以 `out-{i}` 开头、
    /// 约 `bytes` 字节。
    fn tool_loop(sessions: &Sessions, n: usize, bytes: usize) {
        sessions.append(LogEvent::User("go".into()));
        for i in 0..n {
            sessions.append(LogEvent::ToolExecute {
                id: format!("c{i}"),
                name: "bash".into(),
                arguments: format!(r#"{{"command":"step {i}"}}"#),
                content: format!("out-{i}\n{}", "line of output\n".repeat(bytes / 15)),
                images: Vec::new(),
                is_error: false,
            });
        }
    }

    /// 摘要请求里完整带上的工具输出（不含「更早的已省略」占位）。
    fn kept_outputs(request: &PromptRequest) -> Vec<String> {
        request
            .history
            .iter()
            .filter_map(|e| match e {
                LogEvent::SystemReminder(t) if t.starts_with("[bash") && !t.contains("已省略") => {
                    Some(t.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// 第一次回 `error`（正文为空），之后回正常摘要；记下每次请求。
    struct FailOnce {
        error: String,
        requests: Arc<std::sync::Mutex<Vec<PromptRequest>>>,
    }

    impl Sampler for FailOnce {
        fn sample<'a>(
            &'a self,
            request: PromptRequest,
            mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            let first = {
                let mut sent = self.requests.lock().unwrap();
                sent.push(request);
                sent.len() == 1
            };
            let error = self.error.clone();
            Box::pin(async move {
                if first {
                    return LlmOutput {
                        error: Some(error),
                        ..LlmOutput::default()
                    };
                }
                let text = healthy_summary();
                on_delta(StreamDelta::Text(text.clone()));
                LlmOutput {
                    text,
                    ..LlmOutput::default()
                }
            })
        }
    }

    /// 跑一次手动压缩，第一次摘要调用回 `error`；重试不等（默认要等 3 秒）。
    async fn compact_failing_once(
        root: &Context,
        error: &str,
    ) -> Arc<std::sync::Mutex<Vec<PromptRequest>>> {
        let requests: Arc<std::sync::Mutex<Vec<PromptRequest>>> = Default::default();
        let sampler = FailOnce {
            error: error.into(),
            requests: requests.clone(),
        };
        root.provide(LLM, Llm::from_sampler(root.clone(), Arc::new(sampler)))
            .unwrap();
        let cfg = FullReplaceConfig {
            retry_delay_secs: 0,
            ..FullReplaceConfig::default()
        };
        compact_session(root, None, cfg, CompactTrigger::Manual)
            .await
            .unwrap();
        requests
    }

    /// 回归：摘要请求要放得进窗口。
    ///
    /// 摘要请求带着截断后的工具输出（最多 40 条、每条 8KB），可以接近原历史的大小；
    /// 自动压缩在 85% 触发，只剩 15% 给摘要的输出上限。这里 40 条约 80K token，
    /// 窗口 100K 的 70% 放不下，要少带几条（留最近的）。
    #[tokio::test]
    async fn summary_request_is_fitted_to_the_window() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(100_000);
        tool_loop(&sessions, 40, 8_000);
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        let requests = with_recording_summary(&root);

        Compact.run_on(&root, None).await.unwrap();

        let sent = requests.lock().unwrap();
        let request = sent.first().expect("发过一次摘要请求");
        let budget = 100_000 * SUMMARY_INPUT_BUDGET_PERCENT / 100;
        let estimate = estimate_context_tokens(&request.system, &request.history);
        assert!(
            estimate <= budget,
            "摘要请求估算 {estimate} token，超过窗口预算 {budget}"
        );
        let kept = kept_outputs(request);
        assert!(
            !kept.is_empty() && kept.len() < 40,
            "带了 {} 条",
            kept.len()
        );
        // 留下的是最近那几次，丢的是最旧的。
        assert!(
            kept.last().is_some_and(|t| t.contains("out-39\n")),
            "{kept:?}"
        );
        assert!(!kept.iter().any(|t| t.contains("out-0\n")), "{kept:?}");
    }

    /// 回归：上游报「上下文超限」时，下一次少带工具输出，不原样重发。
    ///
    /// 窗口预算是估算，上游的输出上限也算在窗口里；超了之后原样重发几次结论一样，
    /// 整次压缩失败、自动压缩被抑制到下一条用户消息，长工具循环就会撞上真正的超限。
    #[tokio::test]
    async fn overflow_retries_with_fewer_tool_outputs() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        // 窗口很大：预算放得下全部，只靠上游的报错收。
        sessions.set_window(10_000_000);
        tool_loop(&sessions, 8, 2_000);
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();

        let requests = compact_failing_once(
            &root,
            "HTTP 400: This model's maximum context length is 65536 tokens. However, you requested 70000 tokens.",
        )
        .await;

        let sent = requests.lock().unwrap();
        let counts: Vec<usize> = sent.iter().map(|r| kept_outputs(r).len()).collect();
        assert_eq!(counts.len(), 2, "超限一次、重试一次：{counts:?}");
        assert_eq!(counts.first(), Some(&8), "{counts:?}");
        assert!(
            counts.get(1) < counts.first(),
            "超限之后应当少带工具输出：{counts:?}"
        );
    }

    /// 限流、断网这类错误不是超限：原样重试，不砍工具输出。
    #[tokio::test]
    async fn other_errors_retry_the_same_request() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(10_000_000);
        tool_loop(&sessions, 8, 2_000);
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();

        let requests =
            compact_failing_once(&root, "HTTP 429: Too Many Requests (rate limit)").await;

        let sent = requests.lock().unwrap();
        let counts: Vec<usize> = sent.iter().map(|r| kept_outputs(r).len()).collect();
        assert_eq!(counts, vec![8, 8]);
    }

    #[test]
    fn context_overflow_errors_are_recognized() {
        for err in [
            "HTTP 400: {\"error\":{\"code\":\"context_length_exceeded\"}}",
            "This model's maximum context length is 131072 tokens",
            "prompt is too long: 210000 tokens > 200000 maximum",
            "input length and `max_tokens` exceed context limit",
            "The input token count exceeds the maximum number of tokens allowed",
        ] {
            assert!(is_context_overflow(err), "{err}");
        }
        for err in [
            "HTTP 429: Too Many Requests",
            "connection reset by peer",
            "HTTP 500: internal server error",
        ] {
            assert!(!is_context_overflow(err), "{err}");
        }
    }

    /// 回归：子代理压缩也要落段文件、摘要里也要指向它自己的日志。
    ///
    /// 子代理的日志在父会话目录的 `subagents/<id>/` 下（`attach_child_dir`），不走
    /// `disk_session_dir`；原来取不到目录，段文件和指针都没有。而长工具循环、频繁
    /// 压缩恰恰多发生在子代理里。
    #[tokio::test]
    async fn subagent_compaction_points_at_its_own_log() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents").join("a1");
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(1_000_000);
        sessions.attach_child_dir(dir.clone());
        tool_loop(&sessions, 3, 4_000);
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        with_summary_llm(&root);

        Compact.run_on(&root, None).await.unwrap();

        let segment = segments::compaction_dir(&dir).join(segments::segment_filename(0));
        assert!(segment.is_file(), "没有落段文件：{}", segment.display());
        let log = persist::history_path(&dir);
        assert!(log.is_file(), "指向的日志要真的在：{}", log.display());
        let history = sessions.model_history();
        let summary = history
            .iter()
            .find_map(|e| match e {
                LogEvent::SystemReminder(t) if t.contains("<transcript_location>") => Some(t),
                _ => None,
            })
            .expect("摘要末尾要有原文指针");
        assert!(
            summary.contains(&segment.display().to_string()),
            "{summary}"
        );
        assert!(summary.contains(&log.display().to_string()), "{summary}");
    }

    /// 回归：摘要 prompt 说「什么会留下」要和压缩后真实的历史一致。
    ///
    /// 压缩后只留首条和最近一条用户消息；prompt 原来写「用户消息都会留下」，摘要
    /// 模型就可能不记中途的纠正和新约束，而它们只剩摘要第 6 段这一处。
    #[test]
    fn summary_prompt_matches_which_user_messages_survive() {
        let history = vec![
            LogEvent::User("first".into()),
            LogEvent::User("中途改了需求".into()),
            LogEvent::User("latest".into()),
        ];
        let compacted = build_compacted_events(&history, &healthy_summary(), None, None);
        let users: Vec<&str> = compacted
            .iter()
            .filter_map(|e| match e {
                LogEvent::User(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(users.len(), 2, "{users:?}");
        assert!(
            !users.iter().any(|t| t.contains("中途改了需求")),
            "{users:?}"
        );

        let prompt = build_summary_prompt_kind(SummaryPromptKind::Structured, None);
        assert!(
            prompt.contains("except the first one and the most recent one"),
            "prompt 要说清只留首末两条用户消息"
        );
        assert!(prompt.contains("In section 6, keep every user message"));
        assert!(!prompt.contains("only the user's messages and this summary survive"));
    }

    /// 回归：压缩必须真的把「推理占的上下文」降下来。
    ///
    /// 以前压缩只折叠「最后一条用户消息之前」的历史，尾巴那段（自动压缩触发时
    /// 正好是这一轮刚跑完的整条工具链）连同推理原样留着 —— 而推理正是占用的大头，
    /// 于是压完几乎没降。现在整段交给摘要，压缩后一条 assistant 轮次都不留。
    #[tokio::test]
    async fn compaction_drops_the_reasoning_heavy_tail() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(1_000_000);
        sessions.append(LogEvent::User("go".into()));
        let mut history = vec![LogEvent::User("go".into())];
        for i in 0..4 {
            let out = LlmOutput {
                text: "ok".into(),
                reasoning: "r".repeat(40_000),
                tool_calls: vec![ToolCall {
                    id: format!("c{i}"),
                    name: "bash".into(),
                    arguments: "{}".into(),
                }],
                ..LlmOutput::default()
            };
            let tool = LogEvent::ToolExecute {
                id: format!("c{i}"),
                name: "bash".into(),
                arguments: "{}".into(),
                content: "x".repeat(20_000),
                images: Vec::new(),
                is_error: false,
            };
            sessions.append(LogEvent::LlmStream(out.clone()));
            sessions.append(tool.clone());
            history.push(LogEvent::LlmStream(out));
            history.push(tool);
        }
        let before = estimate_context_tokens("", &history);
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        with_summary_llm(&root);

        Compact.run_on(&root, None).await.unwrap();

        let after = estimate_context_tokens("", &sessions.model_history());
        assert!(
            after * 4 < before,
            "压缩后 {after} token，压缩前 {before}：推理重的尾巴没被清掉"
        );
    }

    /// 采一次「上游报了用量」的样：prompt + completion 成为占用锚点。
    fn sample_with_usage(sessions: &Sessions, prompt: u64, completion: u64) {
        sessions.begin_llm();
        sessions.apply_llm_delta(&StreamDelta::Usage {
            tokens: TokenUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                ..TokenUsage::default()
            },
            official: true,
            model: "m".into(),
            cost_usd_ticks: None,
        });
        sessions.finish_llm(&LlmOutput {
            text: "ok".into(),
            ..LlmOutput::default()
        });
    }

    fn with_summary_llm(root: &Context) {
        root.provide(
            LLM,
            Llm::from_sampler(root.clone(), Arc::new(FixedSummary(healthy_summary()))),
        )
        .unwrap();
    }

    /// #154：上游不报用量时全靠估算，估算漏了推理，推理模型的长工具循环永远到
    /// 不了阈值。5 轮 × 约 5000 token 推理塞进 10000 的窗口，必须压。
    #[tokio::test]
    async fn reasoning_heavy_history_triggers_without_upstream_usage() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(10_000);
        sessions.append(LogEvent::User("go".into()));
        for i in 0..5 {
            sessions.append(LogEvent::LlmStream(LlmOutput {
                reasoning: "r".repeat(20_000),
                tool_calls: vec![ToolCall {
                    id: format!("c{i}"),
                    name: "bash".into(),
                    arguments: "{}".into(),
                }],
                ..LlmOutput::default()
            }));
            sessions.append(LogEvent::ToolExecute {
                id: format!("c{i}"),
                name: "bash".into(),
                arguments: "{}".into(),
                content: "ok".into(),
                images: Vec::new(),
                is_error: false,
            });
        }
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        with_summary_llm(&root);
        assert!(Compact.maybe_auto(&root).await.unwrap());
    }

    /// 上一次采样的输出（含推理）和之后的工具结果都会进下一次请求。以前只认上游
    /// 的 prompt，这部分要等再采一次才看得见，一步跨过阈值就直接撞窗口。
    #[tokio::test]
    async fn last_output_and_later_tool_results_count_toward_the_threshold() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(1_000);
        sessions.append(LogEvent::User("go".into()));
        // prompt 700 + completion 100 = 800，还在 85% 以下。
        sample_with_usage(&sessions, 700, 100);
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
            content: "y".repeat(4_000),
            images: Vec::new(),
            is_error: false,
        });
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        with_summary_llm(&root);
        assert!(Compact.maybe_auto(&root).await.unwrap());
    }

    /// 压缩后占用必须按新历史算。以前上游报的压缩**前** prompt 留在账上，
    /// 「压完还超」的判定恒真，一次自动压缩之后整轮都被抑制，长工具循环第二次
    /// 写满窗口时就不再压了。
    #[tokio::test]
    async fn auto_compact_is_not_suppressed_by_the_pre_compact_usage() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(1_000);
        sessions.append(LogEvent::User("go".into()));
        sample_with_usage(&sessions, 890, 10);
        // 历史要大于摘要，压缩才有意义（否则收紧检查会直接拒绝这次压缩）。
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
            content: "y".repeat(4_000),
            images: Vec::new(),
            is_error: false,
        });
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        with_summary_llm(&root);
        assert!(Compact.maybe_auto(&root).await.unwrap());
        assert!(
            !sessions.auto_compact_suppressed(),
            "压缩后 {} token，不该判成压了还超",
            context_tokens_used(&root, "")
        );
    }

    /// 依次吐出几份摘要：第 n 次采样用第 n 份，用完了重复最后一份。
    struct Scripted(std::sync::Mutex<Vec<String>>);

    impl Sampler for Scripted {
        fn sample<'a>(
            &'a self,
            _request: PromptRequest,
            mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            let text = {
                let mut queue = self.0.lock().unwrap();
                if queue.len() > 1 {
                    queue.remove(0)
                } else {
                    queue[0].clone()
                }
            };
            Box::pin(async move {
                for piece in [&text[..text.len() / 2], &text[text.len() / 2..]] {
                    on_delta(StreamDelta::Text(piece.to_string()));
                }
                LlmOutput {
                    text,
                    ..LlmOutput::default()
                }
            })
        }
    }

    /// 跑一次压缩，收下这期间发出的全部 [`crate::SESSION_COMPACTION`]。
    async fn compaction_events(
        summaries: Vec<String>,
        trigger: CompactTrigger,
    ) -> (Sessions, Vec<crate::CompactProgress>) {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.set_window(1_000_000);
        for mut e in sample_history() {
            // 历史要比摘要大，「压完更小」才有意义。
            if let LogEvent::ToolExecute { content, .. } = &mut e {
                *content = "x".repeat(20_000);
            }
            sessions.append(e);
        }
        let _s = root.provide(SESSIONS, sessions.clone()).unwrap();
        root.provide(
            LLM,
            Llm::from_sampler(
                root.clone(),
                Arc::new(Scripted(std::sync::Mutex::new(summaries))),
            ),
        )
        .unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = seen.clone();
        let _on = root.on(
            crate::SESSION_COMPACTION,
            move |e: &crate::PageCompaction| {
                sink.lock().unwrap().push(e.progress.clone());
            },
        );
        let cfg = FullReplaceConfig {
            max_attempts: 3,
            retry_delay_secs: 0,
        };
        let _ = compact_session(&root, None, cfg, trigger).await;
        let events = seen.lock().unwrap().clone();
        (sessions, events)
    }

    /// 压缩那几秒 UI 要看得到进展：开始 → 摘要吐字 → 替换 → 完成，完成时带前后占用。
    #[tokio::test]
    async fn compaction_reports_phases_output_and_before_after() {
        let (sessions, events) =
            compaction_events(vec![healthy_summary()], CompactTrigger::Manual).await;
        let first = events.first().expect("开始时就要发");
        assert!(first.running());
        assert_eq!(first.trigger, CompactTrigger::Manual);
        assert_eq!((first.attempt, first.max_attempts), (1, 3));
        assert!(first.before_tokens > 0);
        assert!(
            events.iter().any(|p| p.running() && p.output_tokens > 0),
            "摘要流式输出要计数：{events:?}"
        );
        assert!(events
            .iter()
            .any(|p| p.running() && p.phase == CompactPhase::Apply));
        let last = events.last().unwrap();
        assert_eq!(last.status, CompactStatus::Completed);
        let after = last.after_tokens.expect("压成了要带压缩后的占用");
        assert!(
            after < last.before_tokens,
            "{after} < {}",
            last.before_tokens
        );
        assert!(last.finished.is_some());
        assert_eq!(
            sessions.compaction().map(|p| p.status),
            Some(CompactStatus::Completed)
        );
    }

    /// 重试时告诉用户第几次、上一次为什么没成；三次都不成就是失败并带原因。
    #[tokio::test]
    async fn compaction_reports_retries_and_failure() {
        let (_, events) = compaction_events(
            vec!["short".into(), healthy_summary()],
            CompactTrigger::Auto,
        )
        .await;
        let retry = events
            .iter()
            .find(|p| p.attempt == 2)
            .expect("第二次尝试要发出来");
        assert_eq!(retry.retry_reason.as_deref(), Some("摘要过短"));
        assert_eq!(retry.output_tokens, 0, "新一次尝试的计数从 0 起");
        assert_eq!(events.last().unwrap().status, CompactStatus::Completed);

        let (sessions, events) =
            compaction_events(vec!["short".into()], CompactTrigger::Auto).await;
        assert_eq!(
            events.last().unwrap().status,
            CompactStatus::Failed("压缩失败：摘要过短。".into())
        );
        assert!(!sessions.compacting());
    }
}
