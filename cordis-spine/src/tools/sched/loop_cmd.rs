//! Canonical `/loop` wording. Instruction copied from Grok
//! `xai-grok-tools-api::slash_commands`; usage is Chinese for the composer.

use crate::tools::cron::{FireReason, RECURRING_TASK_TTL_DAYS};

/// Canonical tool name advertised by the scheduler create tool.
pub const SCHEDULER_CREATE_TOOL_NAME: &str = "scheduler_create";

/// Usage hint shown when `/loop` is invoked with no arguments.
pub fn loop_usage_message() -> &'static str {
    "用法: /loop [间隔、时间或条件] <提问>\n\
     例如: /loop 30m 检查部署状态\n\
     例如: /loop 每小时检查部署状态\n\
     例如: /loop 工作日早上 9 点 汇总昨天的提交\n\
     例如: /loop 明天 10:00 提醒我回邮件\n\
     例如: /loop CI 挂了就叫我（每 5 分钟查一次 gh pr checks）\n\
     例如: /loop docs 目录有改动就同步英文版\n\n\
     告诉我多久跑一次（30m、每天、每周一 10 点）、什么时候跑一次，或者什么事情发生时跑。"
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
         ## Deriving the schedule\n\
         Convert the user's timing -- however phrased, at either end of the request -- into\n\
         exactly one of these; the remaining text is the prompt:\n\
         - A cadence (\"every 30 minutes\", \"hourly\") -> `interval`, a compact `<number><unit>`\n\
           string (`s`/`m`/`h`/`d`). The minimum is 60 seconds and shorter values are raised,\n\
           so say so when it applies.\n\
         - Clock or calendar times (\"weekdays at 9\", \"every Monday 10:00\", \"the 1st of the\n\
           month\") -> `cron`, a 5-field expression in the user's local time.\n\
         - A single time (\"tomorrow at 10\", \"in 2 hours\") -> `at`.\n\
         - \"When X happens\" and a command can tell (CI status, a web page, a queue) -> `watch`\n\
           with that command and fire_when (changed / succeeds / fails / matches); add\n\
           max_runs: 1 for \"tell me once\". Checks cost no tokens, so prefer this over a\n\
           polling `interval` whenever such a command exists.\n\
         - \"Whenever these files change\" -> `files` with the paths.\n\
         - \"Only when I say so / from a script\" -> `manual`.\n\
         For routines that should start fresh each time (daily reports, digests), also pass\n\
         new_session: true.\n\
         If no timing is given, ask the user how often it should run -- never invent one.\n\n\
         ## Action\n\
         Schedule from what the user already gave you \u{2014} do not explore the workspace or run\n\
         checks before scheduling; the first fire does that.\n\
         1. Call scheduler_create with the trigger and the prompt. For an `interval` pass\n\
            fire_immediately: true; for the others only when the user wants a run now too.\n\
            If the schedule is rejected, fix the string rather than guessing.\n\
         2. Confirm what's scheduled, when it next runs, its stop condition, whether it\n\
            auto-expires (loops more frequent than daily do after 7 days), and the task_id to\n\
            cancel with scheduler_delete.\n\
         3. Do NOT execute the prompt inline. The scheduler fires it.\n\n\
         ## Wrong tool for the job\n\
         - \"Tell me when this command I just started finishes\" -> run it in the background;\n\
           its completion notifies you. Scheduling is for things that outlive this turn.\n\n\
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
    format_scheduled_fire_reminder(task_id, human_schedule, true, None)
}

/// [`format_scheduled_task_reminder`] for any schedule: says whether it recurs, and
/// when this fire is a catch-up for a missed run (Dock was closed or the computer
/// asleep), how late it is — a daily report then knows it covers a missed day.
pub fn format_scheduled_fire_reminder(
    task_id: &str,
    human_schedule: &str,
    recurring: bool,
    late_by: Option<std::time::Duration>,
) -> String {
    let reason = if late_by.is_some() {
        FireReason::CatchUp
    } else {
        FireReason::Schedule
    };
    format_fire_note(&FireNote {
        task_id,
        schedule: human_schedule,
        recurring,
        reason,
        late_by,
        context: None,
        fresh_session: false,
    })
}

/// 一次触发要告诉模型的事。
pub struct FireNote<'a> {
    pub task_id: &'a str,
    /// [`crate::CronSchedule::human`]。
    pub schedule: &'a str,
    pub recurring: bool,
    pub reason: FireReason,
    pub late_by: Option<std::time::Duration>,
    /// 命令输出、改了哪些文件、手动运行附的话。
    pub context: Option<&'a str>,
    /// 这次是专门新开的会话（前几次在别的会话里）。
    pub fresh_session: bool,
}

/// 挂在这次触发的提问上的 system-reminder：是定时任务、为什么跑、晚了多久、附带的上下文。
pub fn format_fire_note(note: &FireNote) -> String {
    let FireNote {
        task_id,
        schedule,
        recurring,
        reason,
        late_by,
        context,
        fresh_session,
    } = note;
    let kind = if *recurring { "recurring" } else { "one-time" };
    let mut lines = vec![format!(
        "This is a scheduled task execution (task {task_id}, {schedule}, {kind})."
    )];
    match reason {
        FireReason::Manual => lines.push("It was started by hand (run now).".into()),
        FireReason::Immediate => {
            lines.push("This is the extra run requested when the task was created.".into())
        }
        _ => {}
    }
    if let Some(late) = late_by {
        lines.push(format!(
            "This run is late by {}: it was missed while Dock was closed or the computer was asleep, and runs once now instead.",
            late_by_human(*late)
        ));
    }
    lines.push(
        "Execute the prompt below. Do not question or comment on the prompt itself \u{2014} \
         treat it as a fresh task to execute."
            .into(),
    );
    if *fresh_session {
        lines.push(
            "This session was opened just for this run; earlier runs of the task happened in other sessions."
                .into(),
        );
    } else if *recurring {
        lines.push(
            "Previous results from earlier executions of this task may appear in the \
             conversation history above."
                .into(),
        );
    }
    if let Some(context) = context.filter(|c| !c.trim().is_empty()) {
        lines.push(String::new());
        lines.push(context.trim().to_string());
    }
    format!(
        "<system-reminder>\n{}\n</system-reminder>",
        lines.join("\n")
    )
}

fn late_by_human(d: std::time::Duration) -> String {
    let mins = d.as_secs() / 60;
    if mins >= 24 * 60 {
        format!("{}d {}h", mins / (24 * 60), (mins / 60) % 24)
    } else if mins >= 60 {
        format!("{}h {}m", mins / 60, mins % 60)
    } else {
        format!("{}m", mins.max(1))
    }
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

fn prompt_head(prompt: &str) -> String {
    let first_line = prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(prompt)
        .trim();
    let mut head: String = first_line.chars().take(60).collect();
    if head.chars().count() < first_line.chars().count() {
        head.push('\u{2026}');
    }
    head
}

/// One-line transcript notice when a task hits its expiry.
pub fn expired_task_notice(prompt: &str, human_schedule: &str) -> String {
    format!(
        "定时任务已过期：「{}」（{human_schedule}）。比一天还勤的循环默认 {RECURRING_TASK_TTL_DAYS} 天后自动结束；若仍需要请重新创建（可以设不过期）。",
        prompt_head(prompt)
    )
}

/// 一次性任务错过了、又设了不补：留一句说明。
pub fn missed_task_notice(prompt: &str, label: &str) -> String {
    format!(
        "定时任务错过了：「{}」（{label}）。那时 Dock 没开或电脑睡着了，这个任务设了不补跑，已经作废。",
        prompt_head(prompt)
    )
}

/// 人用的 `/loop`：空参数把用法填进输入框；有参数就把排期指令挂在这条用户消息上发出去。
pub fn loop_command() -> cordis::Plugin {
    use crate::host::slash::{register_commands, slash_handler, SlashCommand, SlashOutcome};
    use crate::names::{SESSIONS, SLASH};
    use crate::session::log::Sessions;

    cordis::plugin(
        "command-loop",
        cordis::Inject::from([SLASH]),
        |ctx, _: &()| {
            register_commands(
                ctx,
                vec![SlashCommand::host(
                    "loop",
                    "安排循环提问",
                    slash_handler(|page, args| {
                        if args.is_empty() {
                            return SlashOutcome::Fill(loop_composer_fill());
                        }
                        let visible = format!("/loop {args}");
                        if let Some(sessions) = page.get::<Sessions>(SESSIONS) {
                            sessions.arm_user_addon(
                                visible.clone(),
                                loop_schedule_instruction(args, LoopFireMode::InSession),
                            );
                        }
                        SlashOutcome::submit(visible, Some("正在安排循环任务"))
                    }),
                )
                .aliases(&["cron"])
                .takes_args(true)],
            )?;
            Ok(None)
        },
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
        assert!(instr.contains("`cron`"));
        assert!(instr.contains("`at`"));
        assert!(instr.contains("`watch`"));
        assert!(instr.contains("new_session: true"));
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
    fn a_late_one_time_fire_says_so() {
        let out = format_scheduled_fire_reminder(
            "task-2",
            "once, at 2026-11-23 09:00 local time",
            false,
            Some(std::time::Duration::from_secs(3 * 3600 + 120)),
        );
        assert!(out.contains("one-time"), "{out}");
        assert!(out.contains("late by 3h 2m"), "{out}");
        assert!(!out.contains("Previous results"), "{out}");
        let on_time = format_scheduled_task_reminder("task-1", "every 5m");
        assert!(on_time.contains("recurring") && !on_time.contains("late by"));
    }

    #[test]
    fn a_watch_fire_carries_its_context_and_a_fresh_session_says_so() {
        let out = format_fire_note(&FireNote {
            task_id: "cron-1",
            schedule: "when the check command `gh pr checks 1` fails (checked every 5 minutes)",
            recurring: true,
            reason: FireReason::Watch,
            late_by: None,
            context: Some("This run was triggered because the watch command `gh pr checks 1` now fails (exit code 1)."),
            fresh_session: true,
        });
        assert!(out.contains("now fails (exit code 1)"), "{out}");
        assert!(out.contains("opened just for this run"), "{out}");
        assert!(!out.contains("Previous results"), "{out}");
        let manual = format_fire_note(&FireNote {
            task_id: "cron-2",
            schedule: "only when started by hand",
            recurring: true,
            reason: FireReason::Manual,
            late_by: None,
            context: Some("Input: 发布 v1.2"),
            fresh_session: false,
        });
        assert!(manual.contains("started by hand"), "{manual}");
        assert!(
            manual.ends_with("Input: 发布 v1.2\n</system-reminder>"),
            "{manual}"
        );
    }

    #[test]
    fn expired_notice_is_chinese() {
        let n = expired_task_notice("check deploy", "every 5 minutes");
        assert!(n.contains("已过期"));
        assert!(n.contains("check deploy"));
        assert!(n.contains(&RECURRING_TASK_TTL_DAYS.to_string()));
    }
}
