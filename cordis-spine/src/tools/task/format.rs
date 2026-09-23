//! Copied from grok-build `xai-tool-types` task notices / completion footer.

/// Grok `wrap_reminder`. Parent mailbox drains into `LogEvent::SystemReminder`.
pub fn wrap_reminder(text: &str) -> String {
    format!("<system-reminder>\n{text}\n</system-reminder>")
}

/// 子代理初始任务末尾的回报指令。
///
/// 只进第一条任务、不进人设也不进工具描述：人设和工具排在请求头里，子代理专属
/// 的一段会让它的头和父级分叉；而工具描述只在模型已经想到那颗工具时才起作用，
/// 「以为做完了、根本没想到要回报」的孩子读不到它。
const REPLY_INSTRUCTION: &str = "启动你的代理（agent_id 为 {parent}）看不到你的工具输出，你的回合正文也只会被截断转发。\
做完后用 send_message 给它发一条自包含的结果；中途有会改变它下一步的发现、失败或空结果，也可以先发。";

pub(super) fn append_reply_instruction(prompt: &mut String, parent: &str) {
    let parent = serde_json::to_string(parent).unwrap_or_else(|_| format!("\"{parent}\""));
    prompt.push_str("\n\n---\n");
    prompt.push_str(&REPLY_INSTRUCTION.replace("{parent}", &parent));
}

const TURN_TEXT_CHARS: usize = 8000;

/// Cap a child's turn text before it rides a parent notice.
pub(super) fn cap_turn_text(output: &str) -> String {
    let body = output.trim();
    if body.is_empty() {
        "（无回合输出）".to_string()
    } else {
        cap_chars(body, TURN_TEXT_CHARS)
    }
}

/// 一条 workflow 子代理上报在收尾通知里占的篇幅。
///
/// 一次 deep-research 有十来个孩子，原样附上能把整份上下文吃掉；而最终结果本来
/// 就是这些上报的提炼，这里只留够主线程判断"过程有没有出岔子"。
const WORKFLOW_REPORT_CHARS: usize = 600;

pub(super) fn cap_report_text(output: &str) -> String {
    let body = output.trim();
    if body.is_empty() {
        "（空）".to_string()
    } else {
        cap_chars(body, WORKFLOW_REPORT_CHARS)
    }
}

fn cap_chars(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

pub(super) fn format_parent_notice(notice: &super::store::ParentNotice) -> String {
    use super::store::ParentNotice;
    match notice {
        ParentNotice::Report { from, output } => {
            format!("Agent {from} sent a message:\n{output}")
        }
        ParentNotice::WorkflowDone {
            name,
            status,
            elapsed_ms,
            summary,
            reports,
            dropped_reports,
        } => {
            let mut text = format!(
                "工作流 \"{name}\" 已{}（耗时 {:.1}s）。\n结果:\n{summary}",
                match status.as_str() {
                    "complete" => "完成",
                    "cancelled" => "被停止",
                    "failed" => "失败",
                    _ => "结束",
                },
                *elapsed_ms as f64 / 1000.0,
            );
            if !reports.is_empty() {
                text.push_str("\n\n过程中各子代理的上报:");
                for report in reports {
                    text.push_str(&format!(
                        "\n- {}: {}",
                        report.agent_id,
                        cap_report_text(&report.output)
                    ));
                }
                if *dropped_reports > 0 {
                    text.push_str(&format!(
                        "\n（另有 {dropped_reports} 条更早的上报没有列出）"
                    ));
                }
            }
            text
        }
        ParentNotice::TurnEnd {
            id,
            subagent_type,
            description,
            duration_ms,
            cancelled,
            output,
        } => {
            let status = if *cancelled {
                "was interrupted and is idle"
            } else {
                "finished its turn and is idle"
            };
            let mut text = format!(
                "Background subagent \"{id}\" ({subagent_type}: \"{description}\") {status}.\n\
                 Duration: {:.1}s\n\
                 Follow up with send_message (agent_id=\"{id}\"); \
                 it starts the next turn immediately. \
                 Use list_agents if you lost the id.",
                *duration_ms as f64 / 1000.0,
            );
            if let Some(output) = output {
                text.push_str("\n\n");
                text.push_str(output);
            }
            text
        }
    }
}

/// Copied from Grok `format_subagent_started_background`.
pub fn format_subagent_started_background(
    subagent_id: &str,
    subagent_type: &str,
    description: &str,
) -> String {
    format!(
        "Subagent started in background.\n\
         agent_id: {subagent_id}\n\
         type: {subagent_type}\n\
         description: {description}\n\n\
         It stays idle after each turn and can message you with send_message while it works. \
         When a turn ends you are notified (with its text if it sent you nothing), so do not poll. \
         To follow up use send_message; list your subagents with list_agents."
    )
}

/// Copied from Grok `format_subagent_auto_backgrounded`.
pub fn format_subagent_auto_backgrounded(
    subagent_id: &str,
    subagent_type: &str,
    description: &str,
) -> String {
    format!(
        "Subagent took longer than the foreground budget and was moved to the \
         background to keep the conversation responsive. It is still running — \
         you will be notified when its turn ends, so do not poll.\n\
         agent_id: {subagent_id}\n\
         type: {subagent_type}\n\
         description: {description}"
    )
}

/// Footer attached to a result the parent already has in hand.
pub fn format_result_footer(subagent_id: &str, subagent_type: &str) -> String {
    format!(
        "<subagent_result>\n\
         agent_id: {subagent_id}\n\
         subagent_type: {subagent_type}\n\
         The child is idle and can be continued with send_message. \
         After it is disposed, resume_from=\"{subagent_id}\" starts from its transcript.\n\
         </subagent_result>"
    )
}

/// Copied from Grok `format_subagent_completed`.
pub fn format_subagent_completed(
    output: &str,
    subagent_id: &str,
    subagent_type: &str,
    tool_calls: u32,
    turns: u32,
    duration_ms: u64,
) -> String {
    let footer = format_result_footer(subagent_id, subagent_type);
    format!(
        "{output}\n\n<subagent_meta>id={subagent_id}, type={subagent_type}, \
         tool_calls={tool_calls}, turns={turns}, duration_ms={duration_ms}</subagent_meta>\n\n\
         {footer}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回报指令点名父级 id（JSON 编码，孩子照抄就能填进 `agent_id`），
    /// 并且指向 `send_message` 而不是已经删掉的 `report`。
    #[test]
    fn reply_instruction_names_the_parent_and_the_tool() {
        let mut prompt = "[explore] look\n\nfind it".to_string();
        append_reply_instruction(&mut prompt, "main#2");
        assert!(
            prompt.starts_with("[explore] look\n\nfind it\n\n---\n"),
            "{prompt}"
        );
        assert!(prompt.contains(r#"agent_id 为 "main#2""#), "{prompt}");
        assert!(prompt.contains("send_message"), "{prompt}");
        assert!(!prompt.contains("report"), "{prompt}");
    }

    #[test]
    fn turn_end_notice_carries_output_only_when_present() {
        use super::super::store::ParentNotice;
        let without = format_parent_notice(&ParentNotice::TurnEnd {
            id: "sa-1".into(),
            subagent_type: "explore".into(),
            description: "look around".into(),
            duration_ms: 1500,
            cancelled: false,
            output: None,
        });
        assert!(without.contains("finished its turn and is idle"));
        assert!(without.contains("send_message"));

        let with = format_parent_notice(&ParentNotice::TurnEnd {
            id: "sa-1".into(),
            subagent_type: "explore".into(),
            description: "look around".into(),
            duration_ms: 1500,
            cancelled: true,
            output: Some("仓库只有 README".into()),
        });
        assert!(with.contains("was interrupted and is idle"));
        assert!(with.contains("仓库只有 README"));
    }

    #[test]
    fn turn_text_is_capped() {
        let long = "x".repeat(TURN_TEXT_CHARS + 50);
        let capped = cap_turn_text(&long);
        assert!(capped.ends_with('…'));
        assert_eq!(capped.chars().count(), TURN_TEXT_CHARS + 1);
        assert_eq!(cap_turn_text("   "), "（无回合输出）");
    }
}
