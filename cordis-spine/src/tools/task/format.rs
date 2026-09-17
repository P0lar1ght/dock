//! Copied from grok-build `xai-tool-types` task notices / completion footer.

pub struct BackgroundNoticeNaming<'a> {
    pub task_output_tool: &'a str,
    pub task_ids_param: &'a str,
    pub timeout_ms_param: &'a str,
}

impl BackgroundNoticeNaming<'static> {
    pub const CANONICAL: Self = Self {
        task_output_tool: "get_task_output",
        task_ids_param: "task_ids",
        timeout_ms_param: "timeout_ms",
    };
}

fn background_result_line(subagent_id: &str, naming: &BackgroundNoticeNaming<'_>) -> String {
    let BackgroundNoticeNaming {
        task_output_tool,
        task_ids_param,
        timeout_ms_param,
    } = *naming;
    format!(
        "When you need its result, use {task_output_tool} with {task_ids_param}=[\"{subagent_id}\"] and a positive {timeout_ms_param}."
    )
}

/// Grok `wrap_reminder`. Parent mailbox drains into `LogEvent::SystemReminder`.
pub fn wrap_reminder(text: &str) -> String {
    format!("<system-reminder>\n{text}\n</system-reminder>")
}

/// Injected into every child persona.
pub(super) const REPORT_MARK: &str = "# 子代理上报";

pub(super) const REPORT_TURN_REMINDER: &str = "主代理看不到你的实时正文。report 是你和主代理的交流通道：有进展、失败、空结果、或需要主代理转发/改派时都要调用。同一轮可以多次。不要只写最终回复。";

const REPORT_DUTY: &str = "\
# 子代理上报
你的助手正文只在每轮结束时由运行时代为转发（可能被截断），report 是更可靠的通道：有进展、失败、空结果、需要主代理转发或改派时都要调用。同一轮可以多次。
不要只写最终回复而不调用 report。
";

const TURN_TEXT_CHARS: usize = 8000;

pub(super) fn append_report_duty(persona: &mut String) {
    if persona.contains(REPORT_MARK) {
        return;
    }
    if !persona.is_empty() && !persona.ends_with('\n') {
        persona.push('\n');
    }
    persona.push('\n');
    persona.push_str(REPORT_DUTY);
}

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
            format!("子代理 {from} 上报:\n{output}")
        }
        ParentNotice::WorkflowDone {
            name,
            status,
            elapsed_ms,
            summary,
            reports,
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
                 Follow up with send_message \
                 (idle starts the next turn immediately; urgent steers a running child now). \
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
    naming: &BackgroundNoticeNaming<'_>,
) -> String {
    let result_line = background_result_line(subagent_id, naming);
    format!(
        "Subagent started in background.\n\
         subagent_id: {subagent_id}\n\
         type: {subagent_type}\n\
         description: {description}\n\n\
         The child stays idle after its turn and talks to you with report \
         (progress and results, many times across turns — not a one-shot handoff). \
         You are notified when a turn ends; to follow up use send_message \
         (idle queued or urgent both start the next turn now; urgent on a running child is send-now). \
         List with list_agents.\n\n\
         {result_line}"
    )
}

/// Copied from Grok `format_subagent_auto_backgrounded`.
pub fn format_subagent_auto_backgrounded(
    subagent_id: &str,
    subagent_type: &str,
    description: &str,
    naming: &BackgroundNoticeNaming<'_>,
) -> String {
    let result_line = background_result_line(subagent_id, naming);
    format!(
        "Subagent took longer than the foreground budget and was moved to the \
         background to keep the conversation responsive. It is still running — \
         you will be notified when its turn ends.\n\
         subagent_id: {subagent_id}\n\
         type: {subagent_type}\n\
         description: {description}\n\n\
         {result_line}"
    )
}

/// Footer attached to a result the parent already has in hand.
pub fn format_result_footer(subagent_id: &str, subagent_type: &str) -> String {
    format!(
        "<subagent_result>\n\
         subagent_id: {subagent_id}\n\
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

    #[test]
    fn report_duty_appends_once() {
        let mut persona = "你是甲。".to_string();
        append_report_duty(&mut persona);
        append_report_duty(&mut persona);
        assert_eq!(persona.matches(REPORT_MARK).count(), 1);
        assert!(persona.contains("不是一次性交卷") || persona.contains("同一轮可以多次"));
        assert!(persona.contains("report 是更可靠的通道"));
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
