//! Canonical `/loop` wording. Instruction copied from Grok
//! `xai-grok-tools-api::slash_commands`; usage is Chinese for the composer.

use crate::tools::cron::RECURRING_TASK_TTL_DAYS;

/// Canonical tool name advertised by the scheduler create tool.
pub const SCHEDULER_CREATE_TOOL_NAME: &str = "scheduler_create";

/// Usage hint shown when `/loop` is invoked with no arguments.
pub fn loop_usage_message() -> &'static str {
    "用法: /loop [间隔] <提问>\n\
     例如: /loop 30m 检查部署状态\n\
     例如: /loop 每小时检查部署状态\n\n\
     告诉我多久跑一次（例如 30m、1 hour、every 2 days）。"
}

/// Bare `/loop` 回车后留在输入框里：用法可见，光标在 `/loop ` 后。
pub fn loop_composer_fill() -> String {
    format!("{}\n/loop ", loop_usage_message())
}

/// Where a scheduled fire runs. Dock only has in-session fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopFireMode {
    /// Each fire runs in a detached background subagent that cannot see this
    /// conversation. Copied from Grok; Dock does not use this path.
    Detached,
    /// Each fire runs as a turn in this conversation.
    InSession,
}

/// Build the model instruction that `/loop` expands into for `args`.
///
/// Copied from Grok `loop_schedule_instruction`. The model, not brittle host
/// parsing, turns the request into `scheduler_create`.
pub fn loop_schedule_instruction(args: &str, mode: LoopFireMode) -> String {
    let fire_context = match mode {
        LoopFireMode::Detached => {
            "Each fire runs in a detached background subagent, not in this conversation,\n\
             so the prompt you store must stand on its own.\n\n\
             ## Writing a prompt that survives a fresh fire\n\
             - Inline the state a fire needs: paths, job/PR/branch ids, the command that checks\n\
               status, and what \"healthy\" looks like. A fire cannot see this conversation, and\n\
               a long-running task restarts from a short summary every few iterations.\n\
             - Only a short status comes back here, so say what that status must contain."
        }
        LoopFireMode::InSession => {
            "Each fire arrives as a new turn in this conversation, and earlier results from\n\
             the same task may still be above it. The stored prompt is re-sent verbatim every\n\
             time, so write a standing order rather than a one-off request.\n\n\
             ## Writing a prompt that reads well on every fire\n\
             - Name the state that must not be guessed: paths, job/PR/branch ids, the command\n\
               that checks status, and what \"healthy\" looks like. This conversation is\n\
               compacted as it grows, so do not rely on details staying visible.\n\
             - Earlier fires may be above you: continue from them instead of restarting."
        }
    };
    format!(
        "# /loop -- schedule a recurring prompt\n\n\
         Turn the input below into a scheduler_create call. {fire_context}\n\
         - Say what one fire does and when it bails: \"if still pending, report one line and\n\
           stop.\" A fire must not poll inline.\n\
         - Give it a stop condition and an exit: \"when <condition> holds, report it and call\n\
           scheduler_delete <task_id>.\" Without that the loop runs until it expires.\n\
         - Keep it short and concrete -- the stored prompt is re-sent on every fire.\n\n\
         ## Deriving the interval\n\
         Convert the user's cadence -- however phrased, at either end of the request -- into a\n\
         compact `<number><unit>` string (`s`/`m`/`h`/`d`); the remaining text is the prompt.\n\
         The minimum is 60 seconds and shorter values are raised, so say so when it applies.\n\
         If no cadence is given, ask the user how often it should run -- never invent one.\n\n\
         ## Action\n\
         Schedule from what the user already gave you \u{2014} do not explore the workspace or run\n\
         checks before scheduling; the first fire does that.\n\
         1. Call scheduler_create with the interval, the prompt, and fire_immediately: true.\n\
            If the interval is rejected, fix the string rather than guessing.\n\
         2. Confirm what's scheduled, the cadence, its stop condition, that it auto-expires\n\
            after 7 days, and the task_id to cancel with scheduler_delete.\n\
         3. Do NOT execute the prompt inline. The scheduler fires it immediately.\n\n\
         ## Wrong tool for the job\n\
         - \"Tell me when X finishes\" -> a background command or watch tool that wakes you on\n\
           the event, not a recurring loop that re-checks on a timer.\n\
         - \"Do X once in N minutes\" -> background `sleep <secs> && <command>`; scheduling is\n\
           recurring-only.\n\n\
         ## Changing an existing loop\n\
         Call scheduler_create with its task_id and only the changed fields; do not\n\
         delete and recreate. If later work changes what a loop should do, update its\n\
         prompt the same way.\n\n\
         ## Input\n\
         {args}"
    )
}

/// Frame a scheduled fire for the model. The TUI shows the raw prompt as the
/// user bubble; this reminder is appended as `LogEvent::SystemReminder`.
pub fn format_scheduled_task_reminder(task_id: &str, human_schedule: &str) -> String {
    format!(
        "<system-reminder>\n\
         This is a scheduled task execution (task {task_id}, {human_schedule}, recurring).\n\
         Execute the prompt below. Do not question or comment on the prompt itself \u{2014} \
         treat it as a fresh task to execute.\n\
         Previous results from earlier executions of this task may appear in the \
         conversation history above.\n\
         </system-reminder>"
    )
}

/// Grok `format_scheduled_task_prompt`: reminder + prompt in one string.
pub fn format_scheduled_task_prompt(prompt: &str, task_id: &str, human_schedule: &str) -> String {
    format!(
        "{}\n\
         \n\
         {prompt}",
        format_scheduled_task_reminder(task_id, human_schedule)
    )
}

/// One-line transcript notice when a loop hits its 7-day TTL.
pub fn expired_task_notice(prompt: &str, human_schedule: &str) -> String {
    let first_line = prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(prompt)
        .trim();
    let mut head: String = first_line.chars().take(60).collect();
    if head.chars().count() < first_line.chars().count() {
        head.push('\u{2026}');
    }
    format!(
        "定时任务已过期：「{head}」（{human_schedule}）。循环任务 {RECURRING_TASK_TTL_DAYS} 天后自动结束；若仍需要请重新创建。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_usage_message_has_no_host_default() {
        let usage = loop_usage_message();
        assert!(usage.contains("用法: /loop"));
        assert!(!usage.contains("10m"));
        let fill = loop_composer_fill();
        assert!(fill.contains("/loop "));
        assert!(fill.contains("用法"));
    }

    #[test]
    fn loop_schedule_instruction_holds_invariants() {
        let args = "every 30 minutes do x";
        let instr = loop_schedule_instruction(args, LoopFireMode::InSession);
        assert!(instr.contains("scheduler_create"));
        assert!(instr.contains("fire_immediately: true"));
        assert!(instr.contains("7 days"));
        assert!(instr.contains(args));
        assert!(instr.contains("never invent"));
        assert!(instr.contains("Do NOT execute the prompt inline"));
    }

    #[test]
    fn format_scheduled_task_prompt_includes_framing() {
        let out = format_scheduled_task_prompt("do stuff", "task-1", "every 5m");
        assert!(out.contains("task-1"));
        assert!(out.contains("every 5m"));
        assert!(out.contains("do stuff"));
        assert!(out.contains("<system-reminder>"));
    }

    #[test]
    fn expired_notice_is_chinese() {
        let n = expired_task_notice("check deploy", "every 5 minutes");
        assert!(n.contains("已过期"));
        assert!(n.contains("check deploy"));
        assert!(n.contains(&RECURRING_TASK_TTL_DAYS.to_string()));
    }
}
