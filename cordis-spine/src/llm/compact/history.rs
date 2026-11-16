//! Compacted `LogEvent` history.
//!
//! Grok `prepare_conversation_for_summarization` + `build_compacted_history`,
//! mapped onto Dock `LogEvent` (no `ConversationItem`).
//!
//! **压缩后一条 assistant 轮次都不留。** 早先照抄 Grok 的
//! `extract_messages_since_last_user`，把「最后一条用户消息之后」的轮次整段保留
//! （含推理），因为 DeepSeek thinking 模式要求请求带 `tools` 时把之前每一轮的
//! `reasoning_content` 一并回传，缺了 400。但那条约束只对**请求里还在的**
//! assistant 消息成立：整段丢掉之后不存在「缺推理的旧轮次」，也就不会触发
//! （官方文档：带 `tools` 时 reasoning_content 要回传并计入上下文，不带时忽略）。
//!
//! 保留那段是压缩压不动的根因：自动压缩在每个采样步之前触发，此时「最后一条
//! 用户消息之后」正好是这一轮刚跑完的整条工具链，推理占比最大 —— 折叠掉旧历史
//! 却把它原样留下，占用几乎不降。这些内容现在由摘要承担。

use cordis_base::tool_output::head_at_line;
use cordis_base::types::{LlmOutput, LogEvent, ThinkingBlock};

use super::summary::{format_compact_summary_content, wrap_user_query};
use crate::prompt::context_usage::IMAGE_TOKEN_ESTIMATE;

/// 摘要里每条工具结果留多少字节。
///
/// 口径对齐 Grok `for_compaction` 的注释（那里按 ~2000 token 折算，正文多英文，
/// 约合 8KB）：留得太少，摘要模型写不出「这个文件读了什么」，压缩后的模型只能
/// 自己重读一遍 —— 恰好就是那段注释要防的事；留得太多则压缩白做。取 8KB：
/// 摘要本身受 500 字符下限约束、又没有截断重试，单条再大就不值得了。
const SUMMARY_TOOL_OUTPUT_HEAD_BYTES: usize = 8_000;

/// 摘要里最多保留最近几条工具结果（起点）。
///
/// 长工具循环里几十次调用全留会把摘要请求撑出窗口：自动压缩在 85% 触发，摘要
/// 的输出上限只剩那 15%。`compact/mod.rs` 从这个数开始，按窗口预算减半到放得下，
/// 上游仍报上下文超限就再减半重试（Grok 用 `fit_conversation_to_budget` 按 token
/// 收尾，这里按条数收）。丢的总是最旧的：「最后跑的那几次」才是延续任务的依据。
pub const SUMMARY_TOOL_OUTPUT_MAX_ENTRIES: usize = 40;

/// 摘要里每条工具调用的实参留多少字节。
///
/// 摘要 prompt 明确要求「列出已经跑过哪些工具**及其实参**」，而压缩后的模型历史
/// 里一条工具轮次都不留 —— 不给实参，摘要模型只能编，编出来的那份就是继承者唯一
/// 的依据，继承者再照着它重跑一遍错的命令。实参通常很短（一条命令、一个路径），
/// 个别 `write_file` 会带上整份文件内容，按这个上限截断。
const SUMMARY_TOOL_ARGS_HEAD_BYTES: usize = 2_000;

/// TUI hides [`LogEvent::SystemReminder`]; this assistant bubble is the
/// visible compact notice (also appended to the display log). Dock-only
/// (Grok paints via ACP notifications).
pub const VISIBLE_NOTICE: &str = cordis_base::types::COMPACT_NOTICE;

/// 摘要请求的历史：剥掉只有 Dock 自己关心的事件，把工具结果截断后留下。
///
/// 与 Grok `strip_tool_messages_for_conversation_item` 的差别有三处：
///
/// 1. **工具结果不丢，只截断**。压缩后模型历史里一条工具链都不留（见模块注释），
///    摘要就是它唯一的依据；全丢掉模型会照着摘要把刚跑过的 read / grep 重调
///    一遍。留开头 [`SUMMARY_TOOL_OUTPUT_HEAD_BYTES`] 字节，实参另留
///    [`SUMMARY_TOOL_ARGS_HEAD_BYTES`] 字节，且只留最近 `keep_tool_outputs` 条
///    （调用方从 [`SUMMARY_TOOL_OUTPUT_MAX_ENTRIES`] 起按窗口收）。
/// 2. **推理仍然清空**。摘要不该照着推理复述，省下来的也是摘要这次调用的钱。
/// 3. **工具结果用 `SystemReminder`，不用 `User`**。三条 wire 都按「请求里第 i 个
///    `User`」取图片槽（`user_images.get(user_i)`），而图片槽是按**真实用户消息**
///    排的；工具结果插成 `User` 会把后面所有真实用户消息的图全顶歪，最后一张图
///    还会落到摘要 prompt 那条 `User` 上。摘要本来也不需要图（`compact_session`
///    传空的 `model_user_images()`），用 `SystemReminder` 就绕开了整套下标对齐。
pub fn prepare_conversation_for_summarization(
    history: &[LogEvent],
    keep_tool_outputs: usize,
) -> Vec<LogEvent> {
    let results = history
        .iter()
        .filter(|e| matches!(e, LogEvent::ToolExecute { .. }))
        .count();
    let mut dropped_results = results.saturating_sub(keep_tool_outputs);
    history
        .iter()
        .filter_map(|event| match event {
            LogEvent::PreStep
            | LogEvent::Prompt(_)
            | LogEvent::Notice { .. }
            | LogEvent::TurnEnd(_) => None,
            // 摘要请求不带 tools（`compact/mod.rs`），所以工具结果就是最后一条
            // user 消息之后唯一能说明「跑到哪了」的东西。
            LogEvent::ToolExecute {
                name,
                arguments,
                content,
                is_error,
                ..
            } => {
                // 只丢最旧的那几条：新近的才是「接下来要接着干什么」的依据。
                if dropped_results > 0 {
                    dropped_results -= 1;
                    return Some(LogEvent::SystemReminder(format!(
                        "[{name}]（更早的工具输出已省略）"
                    )));
                }
                let (head, cut) = tool_output_head(content);
                let args = args_head(arguments);
                let mut text = format!(
                    "[{name}{}]\n{args}\n{head}",
                    if *is_error { " · 失败" } else { "" }
                );
                if cut {
                    text.push_str("\n（工具输出已截断）");
                }
                Some(LogEvent::SystemReminder(text))
            }
            LogEvent::LlmStream(out) => {
                let mut text = out.text.clone();
                if !out.tool_calls.is_empty() {
                    let names: Vec<&str> = out.tool_calls.iter().map(|c| c.name.as_str()).collect();
                    let info = format!("\n[Called tools: {}]", names.join(", "));
                    text.push_str(&info);
                }
                Some(LogEvent::LlmStream(LlmOutput {
                    text,
                    reasoning: String::new(),
                    reasoning_ms: None,
                    tool_calls: Vec::new(),
                    reasoning_items: Vec::new(),
                    thinking_blocks: Vec::new(),
                    // 压缩摘要是发给模型的，失败详情本来就不进模型历史，
                    // 更不该被摘进去。
                    error: None,
                    stopped_by_loop: false,
                }))
            }
            other => Some(other.clone()),
        })
        .collect()
}

fn tool_output_head(content: &str) -> (&str, bool) {
    let head = head_at_line(content, SUMMARY_TOOL_OUTPUT_HEAD_BYTES);
    (head, head.len() < content.len())
}

/// 一条工具调用的实参，截到 [`SUMMARY_TOOL_ARGS_HEAD_BYTES`]。
///
/// `arguments` 是原样的 JSON 字符串（`{"command":"cargo test -p cordis-spine"}`），
/// 不重新序列化 —— 摘要要的是「当时到底跑的是什么」，不是我们理解后的样子。
fn args_head(arguments: &str) -> String {
    let arguments = arguments.trim();
    if arguments.is_empty() || arguments == "{}" {
        return "args: (无)".to_string();
    }
    let head = head_at_line(arguments, SUMMARY_TOOL_ARGS_HEAD_BYTES);
    if head.len() < arguments.len() {
        format!("args (已截断): {head}")
    } else {
        format!("args: {head}")
    }
}

fn first_real_user(history: &[LogEvent]) -> Option<String> {
    history.iter().find_map(|e| match e {
        LogEvent::User(t) if !t.trim().is_empty() => Some(t.text.clone()),
        _ => None,
    })
}

fn last_real_user(history: &[LogEvent]) -> Option<String> {
    history.iter().rev().find_map(|e| match e {
        LogEvent::User(t) if !t.trim().is_empty() => Some(t.text.clone()),
        _ => None,
    })
}

/// 摘要末尾的「原文在哪」指针（抄 Grok `format_transcript_location`）。
///
/// 压缩后的模型历史只有摘要，摘要写漏的东西在模型眼里就不存在了。两份落盘文件
/// 里有**压缩前的完整对话**，把路径告诉模型，「缺了就去查」就成了它自己的选择
/// —— 比往上下文里塞历史便宜得多。
///
/// 两个路径分工不同，都要给：
/// - `segment`（`compaction/segment_NNN.md`）：一份可读的整理稿，摘要 + 逐轮正文。
/// - `raw_log`（`chat_history.jsonl`）：逐行 JSON 的原始事件，**工具调用的确切实参**
///   和推理只在这里。段文件把实参换成了 `[Called tools: …]` 占位。
///
/// 关键在于**按需**：两份都可能上百 KB 到 1 MB，整份读进来会把刚腾出来的上下文
/// 一次性填回去。所以指令里明说要 `grep` / `tail` / `offset`+`limit`，不要通读。
///
/// 用英文：摘要 prompt 与摘要正文都是英文，模型照着同一套话术读不会跑偏。
pub fn format_transcript_location(segment: Option<&str>, raw_log: Option<&str>) -> String {
    let mut out = String::from("\n\n<transcript_location>\n");
    if let Some(path) = segment {
        out.push_str(&format!(
            "The full, unsummarized transcript of this conversation is saved at:\n{path}\n"
        ));
    }
    if let Some(path) = raw_log {
        out.push_str(&format!(
            "\nThe same conversation as line-delimited JSON (raw tool arguments, \
             reasoning, timestamps) is at:\n{path}\n"
        ));
    }
    out.push_str(
        "Read them only when the summary above is missing something you need. Do not \
         read either file in full: both are large, and reading one would refill the \
         context this compaction just freed. Target the lookup instead — `grep -n` for \
         an identifier or an error string, or `tail` / `offset` + `limit` for the most \
         recent turns.\n\
         </transcript_location>",
    );
    out
}

/// Model-history after compact: first real user + last user wrapped in
/// `<user_query>` (Grok) + continuation reminder. **没有 assistant 轮次**。
///
/// 早先这里还接 `extract_messages_since_last_user` 的尾巴（形状抄 Grok），
/// 那是压缩压不动的根因 —— 见模块注释。
///
/// `transcript` 是这次压缩落盘的原文路径（`compaction/segment_NNN.md`），`raw_log`
/// 是会话目录里的 `chat_history.jsonl`。给了哪个就在摘要末尾附哪一段指针，让模型
/// 在摘要不够用时自己去查 —— 见 [`format_transcript_location`]。
///
/// The display log is not replaced; [`VISIBLE_NOTICE`] is appended there too.
pub fn build_compacted_events(
    history: &[LogEvent],
    summary: &str,
    transcript: Option<&std::path::Path>,
    raw_log: Option<&std::path::Path>,
) -> Vec<LogEvent> {
    let first = first_real_user(history);
    let last = last_real_user(history);
    let mut out = Vec::new();
    if let Some(first) = first.clone() {
        out.push(LogEvent::User(first.into()));
    }
    if let Some(last) = last {
        if first.as_deref() != Some(last.as_str()) {
            out.push(LogEvent::User(wrap_user_query(last).into()));
        }
    }
    let mut content = format_compact_summary_content(summary);
    let segment = transcript.map(|p| p.display().to_string());
    let raw = raw_log.map(|p| p.display().to_string());
    if segment.is_some() || raw.is_some() {
        content.push_str(&format_transcript_location(
            segment.as_deref(),
            raw.as_deref(),
        ));
    }
    out.push(LogEvent::SystemReminder(content));
    // [`VISIBLE_NOTICE`] 只进显示日志（`Sessions::replace_compacted` 自己追加），
    // 不进模型历史：它是一条合成的 assistant 消息，没有推理内容。压缩后的前缀里
    // 一条真 assistant 都没有，它若进来就会成为**唯一**一条，而请求带 `tools` 时
    // 上游要求每条 assistant 都带 `reasoning_content`，直接 400。模型也不需要被
    // 告知「刚压缩过」——摘要本身就在上一条 reminder 里。
    out
}

pub fn estimate_context_tokens(system: &str, history: &[LogEvent]) -> u64 {
    let mut n = estimate_text_tokens(system);
    for event in history {
        match event {
            LogEvent::User(t) => n += estimate_text_tokens(&t.text),
            LogEvent::SystemReminder(t) => n += estimate_text_tokens(t),
            LogEvent::LlmStream(out) => {
                n += estimate_text_tokens(&out.text);
                n += estimate_reasoning(out);
                for call in &out.tool_calls {
                    n += estimate_text_tokens(&call.name);
                    n += estimate_text_tokens(&call.arguments);
                }
            }
            // 工具结果发给上游的只有 `tool_call_id` + content（Messages / Responses /
            // chat 都把名字和参数 `..` 掉了）；参数已在上面 assistant 的 tool_calls 里
            // 算过。原先这里再算一遍，写大文件的会话估算能虚高近一半（#176）。
            // 带的图（截图类工具）每次请求都随历史内联重发，按张算。
            LogEvent::ToolExecute {
                content, images, ..
            } => {
                n += estimate_text_tokens(content);
                n += (images.len() as u64).saturating_mul(IMAGE_TOKEN_ESTIMATE);
            }
            LogEvent::PreStep
            | LogEvent::Prompt(_)
            | LogEvent::Notice { .. }
            | LogEvent::TurnEnd(_) => {}
        }
    }
    n
}

/// 模型历史里工具结果一共带了几张图。它们已算在 [`estimate_context_tokens`] 里；
/// 上下文面板把这一份从「消息」挪到「图片」那一行（与推理同一个处理）。
pub fn tool_image_count(history: &[LogEvent]) -> u64 {
    history
        .iter()
        .map(|e| match e {
            LogEvent::ToolExecute { images, .. } => images.len() as u64,
            _ => 0,
        })
        .sum()
}

/// 一轮输出里**会回放给上游**的推理占多少 token。
///
/// 推理随 assistant 轮次原样回传（`llm/http/` 的三条 wire 各自回放原件）：
/// Responses 回放 `reasoning_items` 原件，Messages 回放 `thinking_blocks`，
/// chat/completions 回放 `reasoning` 文本（有签名时还会把 signature 包回）。
/// 有原件就按原件算，没有才退到
/// 文本——`reasoning` 本身就是从原件里拼出来的，两者相加会算两遍。
///
/// 每个 Responses 原件里明文与 `encrypted_content` 是同一段推理的两种形态，取大的那个，
/// 不相加；密文是 base64，比原始字节大约 4/3（Grok `estimate_item_tokens` 的 Reasoning 臂）。
pub fn estimate_reasoning(out: &LlmOutput) -> u64 {
    if !out.reasoning_items.is_empty() {
        return out
            .reasoning_items
            .iter()
            .map(|item| {
                let mut text = String::new();
                for key in ["content", "summary"] {
                    for part in item[key].as_array().into_iter().flatten() {
                        if let Some(t) = part["text"].as_str() {
                            text.push_str(t);
                        }
                    }
                }
                let encrypted = item["encrypted_content"].as_str().map_or(0, str::len) as u64;
                estimate_text_tokens(&text).max(encrypted * 3 / 4 / 4)
            })
            .sum();
    }
    if !out.thinking_blocks.is_empty() {
        return out
            .thinking_blocks
            .iter()
            .map(|b| match b {
                ThinkingBlock::Thinking {
                    thinking,
                    signature,
                } => estimate_text_tokens(thinking) + estimate_text_tokens(signature),
                ThinkingBlock::RedactedThinking { data } => {
                    // opaque / 近似 base64：同 Responses encrypted_content 口径。
                    (data.len() as u64) * 3 / 4 / 4
                }
            })
            .sum();
    }
    estimate_text_tokens(&out.reasoning)
}

/// 一段文本的 token 估算：ASCII 每 4 个字符算 1（向上取整），其余字符各算 1。
///
/// 上下文面板（`context/usage`）、压缩阈值、HTTP 后端在官方用量回来前的暂估、
/// 网关投影的 `item/system_prompt` / `item/system_reminder.tokens` 都用这一个，
/// 几处的数才对得上。
pub fn estimate_text_tokens(text: &str) -> u64 {
    let mut ascii = 0u64;
    let mut other = 0u64;
    for c in text.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            other += 1;
        }
    }
    other + ascii.saturating_add(3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_base::types::ToolCall;

    /// ASCII 每 4 个向上取整，其余各 1：「Hello 世界, dock!」13 ASCII → 4，2 汉字 → 6。
    #[test]
    fn estimate_text_tokens_counts_ascii_by_four_and_the_rest_by_one() {
        assert_eq!(estimate_text_tokens(""), 0);
        assert_eq!(estimate_text_tokens("abcd"), 1);
        assert_eq!(estimate_text_tokens("abcde"), 2);
        assert_eq!(estimate_text_tokens("规约"), 2);
        assert_eq!(estimate_text_tokens("Hello 世界, dock!"), 6);
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
                is_error: false,
            },
            LogEvent::User("also add a test".into()),
        ]
    }

    #[test]
    fn summarizer_prep_drops_tools_and_flattens_calls() {
        let mut history = sample_history();
        history.push(LogEvent::Notice {
            kind: cordis_base::types::NoticeKind::WorkflowReport,
            title: "上报".into(),
            body: "不该进摘要".into(),
        });
        let prepared =
            prepare_conversation_for_summarization(&history, SUMMARY_TOOL_OUTPUT_MAX_ENTRIES);
        assert!(!prepared
            .iter()
            .any(|e| matches!(e, LogEvent::ToolExecute { .. })));
        assert!(
            !prepared
                .iter()
                .any(|e| matches!(e, LogEvent::Notice { .. })),
            "{prepared:?}"
        );
        assert!(prepared.iter().any(|e| matches!(
            e,
            LogEvent::LlmStream(o) if o.tool_calls.is_empty() && o.text.contains("[Called tools: read_file]")
        )));
        // 工具结果不丢，截断后以文本形式进摘要：压缩后模型历史里没有工具链，
        // 摘要就是模型唯一的依据。用 `SystemReminder` 承载，别占真实用户消息的
        // 图槽下标（`user_images.get(user_i)`）。
        assert!(
            prepared.iter().any(|e| matches!(
                e,
                LogEvent::SystemReminder(t)
                    if t.contains("[read_file]") && t.contains("fn login() { buggy }")
            )),
            "{prepared:?}"
        );
        // 摘要请求里除真实用户消息外不该再出现 `User`：三条 wire 都按下标取图，
        // 多出来的 `User` 会把后面真实用户消息的图全顶歪。
        let users: Vec<_> = prepared
            .iter()
            .filter_map(|e| match e {
                LogEvent::User(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            users,
            vec!["fix auth.rs login", "also add a test"],
            "{prepared:?}"
        );
    }

    /// 摘要里的工具结果按 [`SUMMARY_TOOL_OUTPUT_HEAD_BYTES`] 截断：全留等于没压。
    #[test]
    fn summarizer_caps_tool_output() {
        let history = vec![LogEvent::ToolExecute {
            id: "c1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
            content: "y".repeat(SUMMARY_TOOL_OUTPUT_HEAD_BYTES * 3),
            images: Vec::new(),
            is_error: true,
        }];
        let prepared =
            prepare_conversation_for_summarization(&history, SUMMARY_TOOL_OUTPUT_MAX_ENTRIES);
        let LogEvent::SystemReminder(text) = &prepared[0] else {
            panic!("工具结果应该变成一条可读的文本：{prepared:?}");
        };
        assert!(text.contains("[bash · 失败]"), "{text}");
        assert!(text.contains("已截断"), "{text}");
        assert!(
            text.len() < SUMMARY_TOOL_OUTPUT_HEAD_BYTES * 2,
            "留了 {} 字节",
            text.len()
        );
    }

    /// 回归：压缩后的模型历史里**一条 assistant 轮次都不留**。
    ///
    /// 以前照抄 Grok 保留「最后一条用户消息之后」的整段（含推理），因为 DeepSeek
    /// thinking 模式要求请求带 `tools` 时把之前每轮的 `reasoning_content` 一并
    /// 回传。但自动压缩就发生在这一步的工具循环里，那段正是刚跑完的整条工具链，
    /// 推理占大头 —— 留着等于没压。整段丢掉之后请求里不存在缺推理的 assistant
    /// 消息，那条约束自然不触发。
    #[test]
    fn compacted_prefix_keeps_no_assistant_turns() {
        let mut history = sample_history();
        history.push(LogEvent::LlmStream(LlmOutput {
            text: "writing test".into(),
            reasoning: "先看现有测试怎么组织".into(),
            tool_calls: vec![ToolCall {
                id: "c2".into(),
                name: "write_file".into(),
                arguments: "{}".into(),
            }],
            reasoning_items: vec![serde_json::json!({"type":"reasoning","id":"rs_2"})],
            ..LlmOutput::default()
        }));
        history.push(LogEvent::ToolExecute {
            id: "c2".into(),
            name: "write_file".into(),
            arguments: "{}".into(),
            content: "test body".into(),
            images: Vec::new(),
            is_error: false,
        });

        let events = build_compacted_events(&history, "summary body", None, None);
        assert!(
            !events.iter().any(|e| matches!(e, LogEvent::LlmStream(_))),
            "压缩后不该留任何 assistant 轮次（含推理）：{events:?}"
        );
        // 内容没有丢：摘要还在，模型知道前面发生了什么。
        assert!(events
            .iter()
            .any(|e| matches!(e, LogEvent::SystemReminder(t) if t.contains("summary body"))));
    }

    /// 压缩通知是给 TUI 看的（`Sessions::replace_compacted` 自己往显示日志追加），
    /// **不能**进模型历史。
    #[test]
    fn compacted_prefix_has_no_synthetic_assistant_turn() {
        let events = build_compacted_events(&sample_history(), "summary body", None, None);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, LogEvent::LlmStream(o) if o.text == VISIBLE_NOTICE)),
            "{events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, LogEvent::LlmStream(_))),
            "压缩前缀里不该有 assistant：{events:?}"
        );
        // 摘要本身还在，模型不会丢掉上下文。
        assert!(events
            .iter()
            .any(|e| matches!(e, LogEvent::SystemReminder(t) if t.contains("summary body"))));
    }

    #[test]
    fn compacted_keeps_first_and_wrapped_last() {
        let mut history = sample_history();
        history.push(LogEvent::LlmStream(LlmOutput {
            text: "writing test".into(),
            tool_calls: vec![ToolCall {
                id: "c2".into(),
                name: "write_file".into(),
                arguments: "{}".into(),
            }],
            ..LlmOutput::default()
        }));
        history.push(LogEvent::ToolExecute {
            id: "c2".into(),
            name: "write_file".into(),
            arguments: "{}".into(),
            content: "test body".into(),
            images: Vec::new(),
            is_error: false,
        });
        let events = build_compacted_events(
            &history,
            "<summary>\n1. Primary Request: login fix\n</summary>",
            None,
            None,
        );
        assert!(matches!(events.first(), Some(LogEvent::User(t)) if t == "fix auth.rs login"));
        assert!(events.iter().any(|e| matches!(
            e,
            LogEvent::User(t) if t.contains("<user_query>") && t.contains("also add a test")
        )));
        // 工具链与 assistant 轮次一个都不留，内容全在摘要里。
        assert!(!events
            .iter()
            .any(|e| matches!(e, LogEvent::ToolExecute { .. } | LogEvent::LlmStream(_))));
        assert!(events.iter().any(|e| matches!(
            e,
            LogEvent::SystemReminder(t) if t.contains("This session is being continued")
        )));
        assert!(!events.iter().any(|e| matches!(e, LogEvent::PreStep)));
    }

    /// 摘要末尾带「原文在哪」的指针：模型在摘要写漏时自己去查，而不是让压缩
    /// 把整段历史塞回上下文（Grok `format_transcript_location` 同款）。
    #[test]
    fn compacted_prefix_points_at_the_transcript() {
        let path = std::path::Path::new("/tmp/segments/segment_000.md");
        let raw = std::path::Path::new("/tmp/sessions/abc/chat_history.jsonl");
        let events =
            build_compacted_events(&sample_history(), "summary body", Some(path), Some(raw));
        let reminder = events
            .iter()
            .find_map(|e| match e {
                LogEvent::SystemReminder(t) => Some(t.as_str()),
                _ => None,
            })
            .expect("摘要 reminder");
        // 摘要正文与指针在同一段里，压缩后的前缀仍然只有三条。
        assert!(reminder.contains("summary body"), "{reminder}");
        assert!(reminder.contains("<transcript_location>"), "{reminder}");
        assert!(
            reminder.contains("/tmp/segments/segment_000.md"),
            "要给出能直接 read_file 的绝对路径：{reminder}"
        );
        // 原始 jsonl 也给出：工具调用的确切实参与推理只在它里面。
        assert!(
            reminder.contains("/tmp/sessions/abc/chat_history.jsonl"),
            "要给原始日志的路径：{reminder}"
        );
        assert_eq!(events.len(), 3, "{events:?}");
    }

    /// 指针必须说清「按需查、别通读」：两份文件都可能上百 KB 到 1 MB，
    /// 整份读回来等于把刚腾出来的上下文一次性填回去。
    #[test]
    fn transcript_pointer_forbids_a_full_read() {
        let text = format_transcript_location(
            Some("/tmp/a/segment_000.md"),
            Some("/tmp/a/chat_history.jsonl"),
        );
        assert!(text.contains("Do not read either file in full"), "{text}");
        assert!(text.contains("grep"), "要给出定向查的做法：{text}");
        assert!(text.contains("tail"), "{text}");
        assert!(text.contains("limit"), "{text}");
    }

    /// 没落盘（测试、未 attach_disk 的页）就不写指针，别指向一个不存在的文件。
    #[test]
    fn compacted_prefix_has_no_pointer_without_a_transcript() {
        let events = build_compacted_events(&sample_history(), "summary body", None, None);
        let reminder = events
            .iter()
            .find_map(|e| match e {
                LogEvent::SystemReminder(t) => Some(t.as_str()),
                _ => None,
            })
            .expect("摘要 reminder");
        assert!(!reminder.contains("<transcript_location>"), "{reminder}");
    }

    #[test]
    fn compacted_does_not_duplicate_sole_user() {
        let history = vec![LogEvent::User("only".into()), LogEvent::PreStep];
        let summary = "x".repeat(20);
        let events = build_compacted_events(&history, &summary, None, None);
        let users: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                LogEvent::User(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(users, ["only"]);
    }

    /// #176：一次工具调用的参数只发一次（assistant 的 tool_calls），工具结果只带
    /// content。参数不能在 `ToolExecute` 上再算一遍。
    #[test]
    fn tool_arguments_are_counted_once() {
        let args = format!("{{\"content\":\"{}\"}}", "a".repeat(40_000));
        let history = vec![
            LogEvent::LlmStream(LlmOutput {
                tool_calls: vec![cordis_base::types::ToolCall {
                    id: "c1".into(),
                    name: "write_file".into(),
                    arguments: args.clone(),
                }],
                ..LlmOutput::default()
            }),
            LogEvent::ToolExecute {
                id: "c1".into(),
                name: "write_file".into(),
                arguments: args.clone(),
                content: "ok".into(),
                images: Vec::new(),
                is_error: false,
            },
        ];
        let once = estimate_text_tokens("write_file")
            + estimate_text_tokens(&args)
            + estimate_text_tokens("ok");
        assert_eq!(estimate_context_tokens("", &history), once);
    }

    /// 工具结果里的图每次请求都随历史重发（三条 wire 都内联），估算漏掉它，截图多的
    /// 会话占用被低估，自动压缩迟迟不触发；压缩「摘要不比原文小」的判定也会少算原文。
    #[test]
    fn estimate_counts_tool_result_images() {
        let image = || cordis_base::types::UserImage {
            digest: None,
            mime: "image/png".into(),
            data: std::sync::Arc::from(vec![0u8; 64].into_boxed_slice()),
            width: 1,
            height: 1,
        };
        let history = vec![LogEvent::ToolExecute {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
            content: "ok".into(),
            images: vec![image(), image()],
            is_error: false,
        }];
        assert_eq!(
            estimate_context_tokens("", &history),
            estimate_text_tokens("ok") + 2 * IMAGE_TOKEN_ESTIMATE
        );
    }

    /// #154：推理是原样回放给上游的，估算漏掉它，推理模型的上下文就被压到阈值以下。
    #[test]
    fn estimate_counts_replayed_reasoning() {
        let text_only = vec![LogEvent::LlmStream(LlmOutput {
            text: "ok".into(),
            reasoning: "x".repeat(4_000),
            ..LlmOutput::default()
        })];
        assert_eq!(estimate_context_tokens("", &text_only), 1 + 1_000);
    }

    /// 有原件就按原件算，`reasoning` 文本是从原件拼出来的，不能再加一遍；
    /// 原件里明文和密文取大的那个。
    #[test]
    fn reasoning_items_win_over_text_and_are_not_double_counted() {
        let out = LlmOutput {
            reasoning: "y".repeat(4_000),
            reasoning_items: vec![
                serde_json::json!({
                    "type": "reasoning",
                    "content": [{"type": "reasoning_text", "text": "y".repeat(4_000)}],
                }),
                serde_json::json!({
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "s".repeat(40)}],
                    "encrypted_content": "e".repeat(16_000),
                }),
            ],
            ..LlmOutput::default()
        };
        assert_eq!(estimate_reasoning(&out), 1_000 + 3_000);
    }

    /// #251：Messages thinking + signature 回放后要计入上下文估算。
    #[test]
    fn estimate_counts_thinking_blocks_with_signature() {
        let out = LlmOutput {
            reasoning: "short".into(),
            thinking_blocks: vec![ThinkingBlock::Thinking {
                thinking: "t".repeat(4_000),
                signature: "s".repeat(4_000),
            }],
            ..LlmOutput::default()
        };
        // 有 thinking_blocks 就按块算，不再用短 reasoning 文本。
        assert_eq!(estimate_reasoning(&out), 1_000 + 1_000);
    }
}
