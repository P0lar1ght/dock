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

/// Injected into every mailbox child persona (not Grok `task`).
pub(super) const MAILBOX_REPORT_MARK: &str = "# 子代理上报";

pub(super) const MAILBOX_REPORT_TURN_REMINDER: &str =
    "主代理看不到你的助手正文。report 是你和主代理的交流通道：有进展、失败、空结果、或需要主代理转发/改派时都要调用。同一轮可以多次。不要只写最终回复。";

const MAILBOX_REPORT_DUTY: &str = "\
# 子代理上报
你和主代理之间只有 report 这一条通道，不是一次性交卷。任务会多轮：有进展、失败、空结果、需要主代理转发或改派时都要 report。同一轮可以多次调用。
助手正文、inbox 文件、todo 主代理都看不到。不要只写最终回复而不调用 report。
";

const UNREPORTED_CHARS: usize = 8000;

pub(super) fn append_mailbox_report_duty(persona: &mut String) {
    if persona.contains(MAILBOX_REPORT_MARK) {
        return;
    }
    if !persona.is_empty() && !persona.ends_with('\n') {
        persona.push('\n');
    }
    persona.push('\n');
    persona.push_str(MAILBOX_REPORT_DUTY);
}

pub(super) fn format_unreported_turn(output: &str) -> String {
    let body = output.trim();
    let body = if body.is_empty() {
        "（无回合输出）".to_string()
    } else {
        cap_chars(body, UNREPORTED_CHARS)
    };
    format!("结束本轮但未调用 report。助手正文主代理看不到，以下由运行时代为转发：\n{body}")
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
        ParentNotice::FirstIdle {
            id,
            subagent_type,
            description,
            duration_ms,
            cancelled,
        } => {
            let status = if *cancelled {
                "was interrupted and is idle"
            } else {
                "finished its turn and is idle"
            };
            format!(
                "Background subagent \"{id}\" ({subagent_type}: \"{description}\") {status}.\n\
                 Duration: {:.1}s\n\
                 The child talks to you with report (many times across turns). \
                 Assistant text is not delivered. Follow up with send_message \
                 (idle starts the next turn immediately; urgent steers a running child now). \
                 Use list_agents if you lost the id.",
                *duration_ms as f64 / 1000.0,
            )
        }
    }
}

/// Copied from Grok `format_subagent_started_background`.
pub fn format_subagent_started_background(
    subagent_id: &str,
    subagent_type: &str,
    description: &str,
    naming: &BackgroundNoticeNaming<'_>,
    continue_parent_work: bool,
) -> String {
    let _ = continue_parent_work;
    let result_line = background_result_line(subagent_id, naming);
    format!(
        "Subagent started in background.\n\
         subagent_id: {subagent_id}\n\
         type: {subagent_type}\n\
         description: {description}\n\n\
         {result_line}"
    )
}

/// Copied from Grok `format_subagent_auto_backgrounded`.
pub fn format_subagent_auto_backgrounded(
    subagent_id: &str,
    subagent_type: &str,
    description: &str,
    naming: &BackgroundNoticeNaming<'_>,
    notified_on_completion: bool,
    continue_parent_work: bool,
) -> String {
    let _ = continue_parent_work;
    let notify_clause = if notified_on_completion {
        " — you will be notified when it completes"
    } else {
        ""
    };
    let result_line = background_result_line(subagent_id, naming);
    format!(
        "Subagent took longer than the foreground budget and was moved to the \
         background to keep the conversation responsive. It is still running{notify_clause}.\n\
         subagent_id: {subagent_id}\n\
         type: {subagent_type}\n\
         description: {description}\n\n\
         {result_line}"
    )
}

/// Continuable `subagent` tool (not Grok `task`).
pub fn format_continuable_started(
    subagent_id: &str,
    subagent_type: &str,
    description: &str,
) -> String {
    format!(
        "Subagent started in background.\n\
         subagent_id: {subagent_id}\n\
         type: {subagent_type}\n\
         description: {description}\n\n\
         The child stays idle after its first turn and talks to you with report \
         (progress and results, many times across turns — not a one-shot handoff). \
         Assistant text is not delivered. Follow up with send_message: \
         idle queued or urgent both start the next turn now; urgent on a running child is send-now. \
         List with list_agents. You will be notified when that turn finishes."
    )
}

/// Copied from Grok `format_resume_footer`.
pub fn format_resume_footer(
    subagent_id: &str,
    subagent_type: &str,
    persona: Option<&str>,
) -> String {
    let mut footer = format!(
        "<subagent_result>\n\
         subagent_id: {subagent_id}\n\
         subagent_type: {subagent_type}\n\
         To continue this subagent's conversation, use resume_from=\"{subagent_id}\"."
    );
    if let Some(persona) = persona {
        footer.push_str(&format!(
            "\nThe subagent used persona=\"{persona}\". Pass the same persona when resuming."
        ));
    }
    footer.push_str("\n</subagent_result>");
    footer
}

pub fn format_idle_footer(subagent_id: &str, subagent_type: &str) -> String {
    format!(
        "<subagent_result>\n\
         subagent_id: {subagent_id}\n\
         subagent_type: {subagent_type}\n\
         The child is idle. It talks to you with report across turns. Follow up with send_message \
         (queued or urgent both start the next turn now).\n\
         Use resume_from only after the child is disposed.\n\
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
    persona: Option<&str>,
) -> String {
    let footer = format_resume_footer(subagent_id, subagent_type, persona);
    format!(
        "{output}\n\n<subagent_meta>id={subagent_id}, type={subagent_type}, \
         tool_calls={tool_calls}, turns={turns}, duration_ms={duration_ms}</subagent_meta>\n\n\
         {footer}"
    )
}

pub fn sanitize_optional_arg(value: Option<String>) -> Option<String> {
    value.and_then(|s| {
        if !super::types::is_not_sentinel(&s) {
            return None;
        }
        let trimmed = s.trim();
        if trimmed.len() == s.len() {
            Some(s)
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_duty_appends_once() {
        let mut persona = "你是甲。".to_string();
        append_mailbox_report_duty(&mut persona);
        append_mailbox_report_duty(&mut persona);
        assert_eq!(persona.matches(MAILBOX_REPORT_MARK).count(), 1);
        assert!(persona.contains("不是一次性交卷"));
        assert!(persona.contains("同一轮可以多次"));
    }

    #[test]
    fn unreported_turn_forwards_body() {
        let text = format_unreported_turn("  仓库只有 README  ");
        assert!(text.contains("未调用 report"));
        assert!(text.contains("仓库只有 README"));
    }
}
