//! Compacted `LogEvent` history.
//!
//! Grok `prepare_conversation_for_summarization` + `extract_messages_since_last_user`
//! + `build_compacted_history`, mapped onto Dock `LogEvent` (no `ConversationItem`).

use super::summary::{format_compact_summary_content, wrap_user_query};
use crate::types::{LlmOutput, LogEvent};

/// TUI hides [`LogEvent::SystemReminder`]; this assistant bubble is the
/// visible compact notice (also appended to the display log). Dock-only
/// (Grok paints via ACP notifications).
pub const VISIBLE_NOTICE: &str = crate::types::COMPACT_NOTICE;

/// Grok `strip_tool_messages_for_conversation_item` + drop images/reasoning:
/// drop tool results, flatten assistant `tool_calls` into `[Called tools: …]`.
pub fn prepare_conversation_for_summarization(history: &[LogEvent]) -> Vec<LogEvent> {
    history
        .iter()
        .filter_map(|event| match event {
            LogEvent::ToolExecute { .. } | LogEvent::PreStep | LogEvent::Prompt(_) => None,
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
                }))
            }
            other => Some(other.clone()),
        })
        .collect()
}

/// Grok `extract_messages_since_last_user`: assistant + tool rows after the
/// last `User`, with tool bodies replaced by `"Tool call omitted..."`.
///
/// **带 `tool_calls` 的轮次保留推理内容**：有的上游（DeepSeek 的 thinking 模式）
/// 规定请求里只要带 `tools`，之前每一轮的 `reasoning_content` 就必须一并回传，
/// 缺了直接 400。压缩前缀恰好把 tool_calls 留着、推理清空，于是压缩之后的第一
/// 条消息就炸。纯文本轮次照旧清空——约束只针对带工具的轮次，而推理链往往比
/// 正文还长，能省则省。
pub fn extract_messages_since_last_user(history: &[LogEvent]) -> Vec<LogEvent> {
    let start = history
        .iter()
        .rposition(|e| matches!(e, LogEvent::User(_)))
        .map(|i| i + 1)
        .unwrap_or(history.len());
    history[start..]
        .iter()
        .filter_map(|event| match event {
            LogEvent::LlmStream(out) if !out.tool_calls.is_empty() => {
                Some(LogEvent::LlmStream(out.clone()))
            }
            LogEvent::LlmStream(out) => Some(LogEvent::LlmStream(LlmOutput {
                reasoning: String::new(),
                reasoning_items: Vec::new(),
                ..out.clone()
            })),
            LogEvent::ToolExecute {
                id,
                name,
                arguments,
                ..
            } => Some(LogEvent::ToolExecute {
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                content: "Tool call omitted...".into(),

                images: Vec::new(),
            }),
            LogEvent::SystemReminder(text) => Some(LogEvent::SystemReminder(text.clone())),
            LogEvent::User(_) | LogEvent::PreStep | LogEvent::Prompt(_) => None,
        })
        .collect()
}

fn first_real_user(history: &[LogEvent]) -> Option<String> {
    history.iter().find_map(|e| match e {
        LogEvent::User(t) if !t.trim().is_empty() => Some(t.clone()),
        _ => None,
    })
}

fn last_real_user(history: &[LogEvent]) -> Option<String> {
    history.iter().rev().find_map(|e| match e {
        LogEvent::User(t) if !t.trim().is_empty() => Some(t.clone()),
        _ => None,
    })
}

/// Model-history prefix after compact: first real user + last user wrapped
/// in `<user_query>` (Grok) + recent stubbed tools + continuation reminder.
/// The display log is not replaced; [`VISIBLE_NOTICE`] is appended there too.
pub fn build_compacted_events(history: &[LogEvent], summary: &str) -> Vec<LogEvent> {
    let first = first_real_user(history);
    let last = last_real_user(history);
    let mut out = Vec::new();
    if let Some(first) = first.clone() {
        out.push(LogEvent::User(first));
    }
    if let Some(last) = last {
        if first.as_deref() != Some(last.as_str()) {
            out.push(LogEvent::User(wrap_user_query(last)));
        }
    }
    out.extend(extract_messages_since_last_user(history));
    out.push(LogEvent::SystemReminder(format_compact_summary_content(
        summary,
    )));
    out.push(LogEvent::LlmStream(LlmOutput {
        text: VISIBLE_NOTICE.into(),
        ..LlmOutput::default()
    }));
    out
}

pub fn estimate_context_tokens(system: &str, history: &[LogEvent]) -> u64 {
    let mut n = estimate_text(system);
    for event in history {
        match event {
            LogEvent::User(t) | LogEvent::SystemReminder(t) => n += estimate_text(t),
            LogEvent::LlmStream(out) => {
                n += estimate_text(&out.text);
                for call in &out.tool_calls {
                    n += estimate_text(&call.name);
                    n += estimate_text(&call.arguments);
                }
            }
            LogEvent::ToolExecute {
                name,
                arguments,
                content,
                ..
            } => {
                n += estimate_text(name);
                n += estimate_text(arguments);
                n += estimate_text(content);
            }
            LogEvent::PreStep | LogEvent::Prompt(_) => {}
        }
    }
    n
}

fn estimate_text(text: &str) -> u64 {
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
    use crate::types::ToolCall;

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

    #[test]
    fn summarizer_prep_drops_tools_and_flattens_calls() {
        let prepared = prepare_conversation_for_summarization(&sample_history());
        assert!(!prepared
            .iter()
            .any(|e| matches!(e, LogEvent::ToolExecute { .. })));
        assert!(prepared.iter().any(|e| matches!(
            e,
            LogEvent::LlmStream(o) if o.tool_calls.is_empty() && o.text.contains("[Called tools: read_file]")
        )));
    }

    /// 回归：压缩前缀里带 tool_calls 的那几轮必须留着 reasoning。
    /// DeepSeek thinking 模式规定请求带 `tools` 时，之前每轮的 `reasoning_content`
    /// 都要回传，缺了 400——压缩成功之后的第一条消息就会炸。
    #[test]
    fn compacted_tail_keeps_reasoning_on_tool_turns() {
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
        });
        // 纯文本收尾的一轮：没有 tool_calls，约束不适用，照旧清空省上下文。
        history.push(LogEvent::LlmStream(LlmOutput {
            text: "写完了".into(),
            reasoning: "这段推理没人需要".into(),
            ..LlmOutput::default()
        }));

        let tail = extract_messages_since_last_user(&history);
        let with_tools = tail
            .iter()
            .find_map(|e| match e {
                LogEvent::LlmStream(o) if !o.tool_calls.is_empty() => Some(o),
                _ => None,
            })
            .expect("带工具的那一轮还在");
        assert_eq!(with_tools.reasoning, "先看现有测试怎么组织");
        assert_eq!(
            with_tools.reasoning_items.len(),
            1,
            "Responses 的原件也要留"
        );

        let text_only = tail
            .iter()
            .find_map(|e| match e {
                LogEvent::LlmStream(o) if o.tool_calls.is_empty() => Some(o),
                _ => None,
            })
            .expect("纯文本那一轮还在");
        assert!(text_only.reasoning.is_empty(), "{text_only:?}");
        assert!(text_only.reasoning_items.is_empty(), "{text_only:?}");
    }

    #[test]
    fn compacted_keeps_first_and_wrapped_last_and_stubs_recent() {
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
        });
        let events = build_compacted_events(
            &history,
            "<summary>\n1. Primary Request: login fix\n</summary>",
        );
        assert!(matches!(events.first(), Some(LogEvent::User(t)) if t == "fix auth.rs login"));
        assert!(events.iter().any(|e| matches!(
            e,
            LogEvent::User(t) if t.contains("<user_query>") && t.contains("also add a test")
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            LogEvent::ToolExecute { content, .. } if content == "Tool call omitted..."
        )));
        assert!(!events.iter().any(|e| matches!(
            e,
            LogEvent::ToolExecute { content, .. } if content.contains("buggy")
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            LogEvent::SystemReminder(t) if t.contains("This session is being continued")
        )));
        assert!(events
            .iter()
            .any(|e| matches!(e, LogEvent::LlmStream(o) if o.text == VISIBLE_NOTICE)));
        assert!(!events.iter().any(|e| matches!(e, LogEvent::PreStep)));
    }

    #[test]
    fn compacted_does_not_duplicate_sole_user() {
        let history = vec![LogEvent::User("only".into()), LogEvent::PreStep];
        let summary = "x".repeat(20);
        let events = build_compacted_events(&history, &summary);
        let users: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                LogEvent::User(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(users, ["only"]);
    }
}
