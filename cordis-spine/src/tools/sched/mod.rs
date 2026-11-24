//! Scheduler tools over `"cron"` — Grok `scheduler_*` names, DSH register.

mod interval;
mod loop_cmd;

use std::time::Duration;

use cordis::{plugin, Context, Inject, Plugin};
use serde_json::Value;

use crate::agent::presets::AgentPresets;
use crate::names::{AGENT_PRESETS, CRON, PLAN_MODE, SESSIONS, TOOLS};
use crate::session::log::Sessions;
use crate::tools::cron::{
    now_ms, parse_at, Cron, CronApproval, CronError, CronExpiry, CronJob, CronOwner, CronPatch,
    CronSchedule, CronTarget, NewCronTask, RunOutcome, WatchWhen, DEFAULT_CHECK_EVERY,
    DEFAULT_FILES_DEBOUNCE, MAX_SCHEDULED_TASKS, RECURRING_TASK_TTL_DAYS,
};
use crate::tools::plan_mode::PlanMode;
use crate::tools::registry::{exec_ctx, own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

pub use interval::{interval_to_human, parse_interval};
pub use loop_cmd::{
    expired_task_notice, format_fire_note, format_scheduled_fire_reminder,
    format_scheduled_task_prompt, format_scheduled_task_reminder, loop_command, loop_composer_fill,
    loop_schedule_instruction, loop_usage_message, missed_task_notice, FireNote, LoopFireMode,
    SCHEDULER_CREATE_TOOL_NAME,
};

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("invalid interval: {0}")]
    InvalidInterval(String),
}

const CREATE_PARAMS: &str = r#"{"type":"object","properties":{"interval":{"type":"string","description":"Recurring interval for polling loops, e.g. 5m, 2h, 1d (min 60s). Phase is kept from creation."},"cron":{"type":"string","description":"Recurring calendar schedule: a 5-field cron expression (minute hour day-of-month month day-of-week) in the user's local time, e.g. \"0 9 * * 1-5\" = weekdays at 09:00, \"0 10 * * 1\" = Mondays at 10:00, \"0 9 1 * *\" = the 1st of each month."},"at":{"type":"string","description":"Run once: a delay such as 30m or 2h, or a local date-time such as 2026-11-23 09:00 (RFC 3339 also accepted)."},"watch":{"type":"string","description":"Watch: a shell command run in this session's directory every watch_every; the prompt fires only when its result meets fire_when (checking costs no tokens). E.g. \"gh pr checks 123\", \"curl -s https://example.com/status\"."},"watch_every":{"type":"string","description":"How often to run the watch command, e.g. 2m, 1h (at least 1m, default 5m; shorter is an error)."},"fire_when":{"type":"string","enum":["changed","succeeds","fails","matches"],"description":"When a watch fires: changed = output differs from the last check (default; the first check only sets the baseline); succeeds / fails = exit code becomes 0 / non-zero; matches = output starts matching `match`. Except changed, it fires once when the condition becomes true, not on every check while it stays true; if it already holds at the first check, that first check fires."},"match":{"type":"string","description":"Regex for fire_when: matches."},"files":{"type":"array","items":{"type":"string"},"description":"Fire when files change: paths relative to this session's directory (directories, files, or globs like docs/**/*.md; ~/ for home). Directories are watched recursively, so / and the home directory itself are rejected; paths outside this project need the user's separate approval. Changes inside .git, node_modules, target, and changes made while this task's own run is going are ignored."},"debounce":{"type":"string","description":"For files: wait this long without further changes before firing, e.g. 10s (default), 2m."},"manual":{"type":"boolean","description":"Only run when started by hand (the desktop app's Run now, or `dock schedule run <id>` from a script, git hook, or CI)."},"prompt":{"type":"string","description":"Prompt to run on each fire. Watch output, changed files, or text passed to a manual run are attached to it."},"task_id":{"type":"string","description":"Existing id to update in place. Omitted fields stay unchanged; an interval change keeps the phase."},"fire_immediately":{"type":"boolean","description":"If true, also fire once right away (an extra run, not counted in max_runs). Default false. Ignored when updating."},"new_session":{"type":"boolean","description":"Run each fire in a fresh session in this project (clean context, good for daily reports) instead of as a new turn here. Default false."},"paused":{"type":"boolean","description":"Pause (true) or resume (false). A resumed task skips the slots it missed while paused."},"expires_in":{"type":"string","description":"Stop the task after this long, e.g. 30d, or \"never\". Default: 7d for schedules that fire or check more often than daily, otherwise never."},"max_runs":{"type":"integer","minimum":1,"description":"Stop after this many automatic runs (e.g. 1 for \"tell me once when CI passes\")."},"catch_up":{"type":"boolean","description":"If a run is missed because Dock was closed or the computer was asleep, run once when Dock is back (missed runs are never replayed one by one). Default: true for cron and at, false for interval."},"approval":{"type":"string","enum":["smart","ask"],"description":"How tool calls that need permission are approved during this task's runs, which usually happen with nobody at the computer. smart (default) = read-only commands and edits inside the project run, high-risk actions ask the user, others are judged by a model; ask = always ask the user. Either way a request nobody answers within 15 minutes is refused and the run continues. Only the user can set always-allow, in the desktop app."}}}"#;
const LIST_PARAMS: &str = r#"{"type":"object","properties":{}}"#;
const DELETE_PARAMS: &str = r#"{"type":"object","properties":{"id":{"type":"string","description":"Task id from scheduler_create."}},"required":["id"]}"#;

pub fn tool_scheduler() -> Plugin {
    plugin(
        "tool-scheduler",
        Inject::from([TOOLS, CRON]),
        |ctx, _: &()| {
            let tools = ctx.require::<Tools>(TOOLS)?;
            let create: ToolBody = {
                let ctx = ctx.clone();
                std::sync::Arc::new(move |call| {
                    let ctx = ctx.clone();
                    Box::pin(async move { create_job(&ctx, call) })
                })
            };
            let list: ToolBody = {
                let ctx = ctx.clone();
                std::sync::Arc::new(move |call| {
                    let ctx = ctx.clone();
                    Box::pin(async move { list_jobs(&ctx, call) })
                })
            };
            let delete: ToolBody = {
                let ctx = ctx.clone();
                std::sync::Arc::new(move |call| {
                    let ctx = ctx.clone();
                    Box::pin(async move { delete_job(&ctx, call) })
                })
            };
            own_registered(
                ctx,
                vec![
                    tools.register(
                        ToolSpec {
                            name: "scheduler_create".into(),
                            description: format!(
                                "Create a scheduled task that runs a prompt later, or update an existing one in place.\n\n\
Pick exactly one trigger:\n\
- interval: every N minutes/hours/days, for polling something until it is done (\"check the deploy every 10m\")\n\
- cron: calendar times in the user's local time, for routines (\"weekdays at 9:00\", \"every Monday 10:00\")\n\
- at: once, after a delay or at a local date-time (\"in 30m\", \"2026-11-23 09:00\")\n\
- watch: a shell command checked on an interval; fires only when its result changes / succeeds / fails / matches. Prefer this over interval when a command can tell whether anything happened (CI status, a web page, a queue) -- checks cost no tokens\n\
- files: fires when files under the given paths change (debounced), with the list of changed files\n\
- manual: only when started by hand or by `dock schedule run <id>` (scripts, git hooks, CI on this machine)\n\n\
Set fire_immediately: true to also fire once right away. Set new_session: true for routines that should start from a clean context each time (reports, digests); leave it false to keep earlier results visible in this conversation.\n\n\
To change an existing task, pass its task_id: provided fields replace old values, omitted ones are unchanged (paused: true / false pauses or resumes). An unknown id errors.\n\n\
Usage notes:\n\
- Maximum {MAX_SCHEDULED_TASKS} scheduled tasks at once\n\
- Tasks belong to this session and keep running across Dock restarts (a closed session is reopened). If the previous fire is still queued or running, the new one is dropped rather than piling up\n\
- Schedules that fire or check more often than daily expire after {RECURRING_TASK_TTL_DAYS} days unless expires_in says otherwise; the rest do not expire. max_runs caps the number of automatic runs\n\
- Tasks only run while Dock is open. catch_up decides whether a missed run happens once when Dock is back"
                            ),
                            parameters_json: CREATE_PARAMS.into(),
                        },
                        create,
                    )?,
                    tools.register(
                        ToolSpec {
                            name: "scheduler_list".into(),
                            description: "List this session's scheduled tasks with their IDs, schedules, prompts, and next fire times.".into(),
                            parameters_json: LIST_PARAMS.into(),
                        },
                        list,
                    )?,
                    tools.register(
                        ToolSpec {
                            name: "scheduler_delete".into(),
                            description: "Cancel one of this session's scheduled tasks by ID.".into(),
                            parameters_json: DELETE_PARAMS.into(),
                        },
                        delete,
                    )?,
                ],
            )?;
            Ok(None)
        },
    )
}

/// 调用方所在的那一页会话（分页各是各的）。子代理的会话不落盘、没有 id：经
/// `"planMode"` 找到它所在的页（同画布）。都找不到（harness）就不记归属、不限定。
fn caller_owner(ctx: &Context) -> Option<CronOwner> {
    let exec = exec_ctx().unwrap_or_else(|| ctx.clone());
    let own = exec.get::<Sessions>(SESSIONS);
    let page = exec
        .get::<PlanMode>(PLAN_MODE)
        .and_then(|plan| plan.page_sessions());
    let sessions = [own, page]
        .into_iter()
        .flatten()
        .find(|s| !s.live_session_id().is_empty())?;
    Some(CronOwner {
        session: sessions.live_session_id(),
        cwd: sessions
            .workspace_cwd()
            .unwrap_or_else(|| crate::session::cwd::session_cwd(&exec)),
    })
}

fn text_arg<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 触发方式的参数名。
const TRIGGER_KEYS: [&str; 6] = ["interval", "cron", "at", "watch", "files", "manual"];

fn given(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Bool(b)) => *b,
        _ => false,
    }
}

/// 时长（防抖、条件命令的检查间隔）：`10s` / `2m` / `1h` / `1d` / 纯数字算秒。
/// 不像 `parse_interval` 那样把短于 60 秒的悄悄提上去，下限由调用方各自判。
fn parse_short(raw: &str) -> Result<Duration, String> {
    let raw = raw.trim();
    let (digits, unit) = match raw.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => (&raw[..i], c),
        _ => (raw, 's'),
    };
    let n: u64 = digits
        .trim()
        .parse()
        .map_err(|_| format!("看不懂时长 {raw:?}（例如 10s、2m）"))?;
    let secs = match unit {
        's' => n,
        'm' => n.saturating_mul(60),
        'h' => n.saturating_mul(3600),
        'd' => n.saturating_mul(86_400),
        _ => return Err(format!("看不懂时长 {raw:?}（例如 10s、2m）")),
    };
    Ok(Duration::from_secs(secs))
}

/// 触发方式里给了哪一个。都没给是 `Ok(None)`，给了两个以上报错。
fn schedule_arg(v: &Value, now: u64) -> Result<Option<CronSchedule>, String> {
    let picked: Vec<&str> = TRIGGER_KEYS.into_iter().filter(|k| given(v, k)).collect();
    if picked.len() > 1 {
        return Err(format!(
            "Error: give only one of interval, cron, at, watch, files, manual (got {})",
            picked.join(", ")
        ));
    }
    let err = |e: CronError| format!("Error: {e}");
    if let Some(raw) = text_arg(v, "interval") {
        let secs = parse_interval(raw).map_err(|e| e.to_string())?;
        return Ok(Some(CronSchedule::Every(Duration::from_secs(secs))));
    }
    if let Some(raw) = text_arg(v, "cron") {
        return CronSchedule::cron(raw).map(Some).map_err(err);
    }
    if let Some(raw) = text_arg(v, "at") {
        return parse_at(raw, now)
            .map(|at| Some(CronSchedule::At(at)))
            .map_err(err);
    }
    if let Some(command) = text_arg(v, "watch") {
        // 不走 `parse_interval`：它把短于一分钟的悄悄提到一分钟，这里要报错。
        let every = match text_arg(v, "watch_every") {
            Some(raw) => parse_short(raw).map_err(|e| format!("Error: watch_every: {e}"))?,
            None => DEFAULT_CHECK_EVERY,
        };
        let when = match text_arg(v, "fire_when").unwrap_or("changed") {
            "changed" => WatchWhen::Changed,
            "succeeds" => WatchWhen::Succeeds,
            "fails" => WatchWhen::Fails,
            "matches" => WatchWhen::Matches {
                pattern: text_arg(v, "match")
                    .ok_or("Error: fire_when: matches needs a `match` regex")?
                    .to_string(),
            },
            other => {
                return Err(format!(
                    "Error: fire_when must be changed, succeeds, fails, or matches (got {other})"
                ))
            }
        };
        return CronSchedule::watch(command, every, when)
            .map(Some)
            .map_err(err);
    }
    if given(v, "files") {
        let paths: Vec<String> = v
            .get("files")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let debounce = match text_arg(v, "debounce") {
            Some(raw) => parse_short(raw).map_err(|e| format!("Error: debounce: {e}"))?,
            None => DEFAULT_FILES_DEBOUNCE,
        };
        return CronSchedule::files(paths, debounce).map(Some).map_err(err);
    }
    if given(v, "manual") {
        return Ok(Some(CronSchedule::Manual));
    }
    Ok(None)
}

/// `expires_in`：`never` / 一段时长（`30d`）。
fn expiry_arg(v: &Value, now: u64) -> Result<Option<CronExpiry>, String> {
    let Some(raw) = text_arg(v, "expires_in") else {
        return Ok(None);
    };
    if matches!(raw.to_ascii_lowercase().as_str(), "never" | "none") {
        return Ok(Some(CronExpiry::Never));
    }
    let secs = parse_interval(raw).map_err(|e| format!("Error: expires_in: {e}"))?;
    Ok(Some(CronExpiry::At(
        now.saturating_add(secs.saturating_mul(1000)),
    )))
}

fn max_runs_arg(v: &Value) -> Result<Option<u32>, String> {
    match v.get("max_runs") {
        None | Some(Value::Null) => Ok(None),
        Some(n) => n
            .as_u64()
            .filter(|n| *n >= 1)
            .map(|n| Some(u32::try_from(n).unwrap_or(u32::MAX)))
            .ok_or_else(|| "Error: max_runs must be a positive integer".to_string()),
    }
}

/// `approval`：模型只能选 `smart` / `ask`；「始终允许」只有用户自己能设（桌面端）。
fn approval_arg(v: &Value) -> Result<Option<CronApproval>, String> {
    match text_arg(v, "approval") {
        None => Ok(None),
        Some("smart") => Ok(Some(CronApproval::Smart)),
        Some("ask") => Ok(Some(CronApproval::Ask)),
        Some("allow" | "auto" | "always") => Err(
            "Error: approval: always-allow can only be set by the user in the desktop app; use \"smart\" or \"ask\""
                .into(),
        ),
        Some(other) => Err(format!(
            "Error: approval must be smart or ask (got {other})"
        )),
    }
}

/// 放宽到哪一档算放宽：询问 < 智能授权 < 始终允许。
fn approval_rank(a: CronApproval) -> u8 {
    match a {
        CronApproval::Ask => 0,
        CronApproval::Smart => 1,
        CronApproval::Allow => 2,
    }
}

/// 调用方那一页现在用的预设（「每次新开会话」时照它开）。
fn caller_preset(ctx: &Context) -> Option<String> {
    let exec = exec_ctx().unwrap_or_else(|| ctx.clone());
    exec.get::<AgentPresets>(AGENT_PRESETS)
        .map(|p| p.current_id())
        .filter(|id| !id.is_empty())
}

fn create_job(ctx: &Context, call: ToolCall) -> ToolResult {
    let v: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
    let now = now_ms();
    let prompt = text_arg(&v, "prompt");
    let task_id = text_arg(&v, "task_id");
    let flag = |key: &str| v.get(key).and_then(Value::as_bool);
    let fire_immediately = flag("fire_immediately").unwrap_or(false);
    let target = flag("new_session").map(|yes| {
        if yes {
            CronTarget::NewSession
        } else {
            CronTarget::Session
        }
    });
    let parsed = schedule_arg(&v, now).and_then(|schedule| {
        Ok((
            schedule,
            expiry_arg(&v, now)?,
            max_runs_arg(&v)?,
            approval_arg(&v)?,
        ))
    });
    let (schedule, expiry, max_runs, approval) = match parsed {
        Ok(parsed) => parsed,
        Err(e) => return tool_result(call, e),
    };

    let Some(cron) = ctx.get::<Cron>(CRON) else {
        return tool_result(call, "Error: cron is not mounted");
    };
    let preset = (target == Some(CronTarget::NewSession))
        .then(|| caller_preset(ctx))
        .flatten();

    if let Some(id) = task_id {
        // 模型只能收紧，不能放宽：用户设成「询问」的，它改不回「智能授权」。
        if let (Some(want), Some(job)) = (approval, cron.get(id)) {
            if approval_rank(want) > approval_rank(job.approval) {
                return tool_result(
                    call,
                    format!(
                        "Error: approval of {id} is {} (set by the user); the model can only make it stricter",
                        match job.approval {
                            CronApproval::Ask => "ask",
                            CronApproval::Smart => "smart",
                            CronApproval::Allow => "allow",
                        }
                    ),
                );
            }
        }
        let patch = CronPatch {
            schedule,
            prompt: prompt.map(str::to_string),
            expiry,
            max_runs: max_runs.map(Some),
            catch_up: flag("catch_up"),
            target,
            preset: preset.map(Some),
            paused: flag("paused"),
            approval,
            approval_timeout_secs: None,
        };
        if patch.is_empty() {
            return tool_result(
                call,
                format!(
                    "Error: scheduler_create update of {id} needs at least one field to change"
                ),
            );
        }
        let scope = caller_owner(ctx).map(|o| o.session);
        return match cron.patch(id, scope.as_deref(), patch) {
            Ok(job) => tool_result(
                call,
                format!(
                    "已更新 {id}（{}）\n{}\n提问：{}\n取消：scheduler_delete {id}，或 /tasks 里按 x",
                    job.schedule.label(),
                    timing_lines(&job, now),
                    job.prompt
                ),
            ),
            Err(CronError::UnknownId(_)) => tool_result(
                call,
                format!("Error: unknown task_id {id}; call scheduler_list to see active task ids"),
            ),
            Err(e) => tool_result(call, format!("Error: {e}")),
        };
    }

    let Some(schedule) = schedule else {
        return tool_result(
            call,
            "Error: one of interval, cron, at, watch, files, or manual is required when creating a task",
        );
    };
    let Some(prompt) = prompt else {
        return tool_result(call, "Error: prompt is required when creating a task");
    };
    let created = cron.create(NewCronTask {
        owner: caller_owner(ctx),
        fire_immediately,
        expiry: expiry.unwrap_or_default(),
        max_runs,
        catch_up: flag("catch_up"),
        target: target.unwrap_or_default(),
        preset,
        paused: flag("paused").unwrap_or(false),
        approval: approval.unwrap_or_default(),
        ..NewCronTask::new(schedule, prompt)
    });
    match created.map(|id| cron.get(&id).ok_or(id)) {
        Ok(Ok(job)) => {
            let immediately = if fire_immediately { "是" } else { "否" };
            tool_result(
                call,
                format!(
                    "已设定 {}（{}）\n立即执行：{immediately}\n{}\n提问：{prompt}\n取消：scheduler_delete {}，或 /tasks 里按 x",
                    job.id,
                    job.schedule.label(),
                    timing_lines(&job, now),
                    job.id,
                ),
            )
        }
        Ok(Err(id)) => tool_result(call, format!("已设定 {id}\n提问：{prompt}")),
        Err(CronError::LimitReached(n)) => {
            tool_result(call, format!("Error: maximum {n} scheduled tasks"))
        }
        Err(e) => tool_result(call, format!("Error: {e}")),
    }
}

/// 「下次 …」「… 后自动过期 / 不会自动过期」「最多跑 N 次」「错过了补一次」「每次新开会话」。
fn timing_lines(job: &CronJob, now: u64) -> String {
    let mut lines = Vec::new();
    if job.paused {
        lines.push("已暂停（scheduler_create 带 task_id 和 paused: false 继续）".to_string());
    }
    let next = |at: u64| {
        if at > now {
            human_wait(Duration::from_millis(at - now))
        } else {
            "马上".into()
        }
    };
    match (&job.schedule, job.next_at_ms) {
        _ if job.done => lines.push("已跑完".into()),
        (CronSchedule::Watch(_), Some(at)) => lines.push(format!(
            "下次检查：{}（只跑命令，满足条件才发提问）",
            next(at)
        )),
        (CronSchedule::Files(_), _) => lines.push("监听中：改动安静下来后发提问".into()),
        (CronSchedule::Manual, _) => lines.push(format!(
            "只在手动运行时跑：桌面端「立即运行」，或 dock schedule run {}",
            job.id
        )),
        (_, Some(at)) => lines.push(format!("下次：{}", next(at))),
        (_, None) => {}
    }
    match job.expires_at_ms {
        Some(at) if at > now => lines.push(format!(
            "{} 后自动过期",
            human_wait(Duration::from_millis(at - now))
        )),
        Some(_) => lines.push("已过期".into()),
        None if job.schedule.is_recurring() => lines.push("不会自动过期".into()),
        None => {}
    }
    if let Some(max) = job.max_runs {
        lines.push(format!("最多跑 {max} 次（已跑 {}）", job.runs));
    }
    if job.target == CronTarget::NewSession {
        lines.push("每次在这个项目里新开一个会话".into());
    }
    lines.push(approval_line(job));
    if matches!(
        job.schedule,
        CronSchedule::Every(_) | CronSchedule::Cron(_) | CronSchedule::At(_)
    ) {
        lines.push(if job.catch_up {
            "错过了（Dock 没开 / 电脑睡着）：回来后补跑一次".into()
        } else {
            "错过了（Dock 没开 / 电脑睡着）：不补，等下一次".into()
        });
    }
    lines.join("\n")
}

/// 「每次运行：智能授权，权限请求等 15 分钟没人批就跳过」。
fn approval_line(job: &CronJob) -> String {
    let wait = match job.approval_timeout {
        Some(limit) => format!(
            "权限请求等 {} 分钟没人批就跳过这一步",
            limit.as_secs().div_ceil(60).max(1)
        ),
        None => "权限请求一直等人批".into(),
    };
    format!("每次运行：{}，{wait}", job.approval.label())
}

fn list_jobs(ctx: &Context, call: ToolCall) -> ToolResult {
    let Some(cron) = ctx.get::<Cron>(CRON) else {
        return tool_result(call, "Error: cron is not mounted");
    };
    let jobs = match caller_owner(ctx) {
        Some(owner) => cron.list_for(&owner.session),
        None => cron.list(),
    };
    if jobs.is_empty() {
        return tool_result(call, "没有定时任务。");
    };
    let now = now_ms();
    let mut out = String::new();
    for job in jobs {
        let mut state = match job.next_at_ms {
            _ if job.paused => "已暂停".to_string(),
            _ if job.done => match &job.last_error {
                Some(e) => format!("已跑完，最后一次没送到：{e}"),
                None => "已跑完".into(),
            },
            Some(at) if at > now => format!("下次 {}", human_wait(Duration::from_millis(at - now))),
            Some(_) => "到期".into(),
            None => "等着触发".into(),
        };
        if let Some(run) = job.last_run() {
            let ago = human_wait(Duration::from_millis(now.saturating_sub(run.at_ms)));
            let how = match run.outcome {
                RunOutcome::Sent => "送达",
                RunOutcome::Coalesced => "上一次还没跑完，跳过",
                RunOutcome::Failed => "没送到",
            };
            state.push_str(&format!("；上次 {ago} 前{how}"));
        }
        out.push_str(&format!(
            "{}：{} — {}（{state}）\n",
            job.id,
            job.schedule.label(),
            job.prompt
        ));
    }
    tool_result(call, out)
}

fn human_wait(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 86_400 {
        format!("{}d {}h", secs / 86_400, (secs % 86_400) / 3600)
    } else if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn delete_job(ctx: &Context, call: ToolCall) -> ToolResult {
    let v: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
    if id.is_empty() {
        return tool_result(call, "Error: id is required");
    }
    let Some(cron) = ctx.get::<Cron>(CRON) else {
        return tool_result(call, "Error: cron is not mounted");
    };
    let scope = caller_owner(ctx).map(|o| o.session);
    match cron.cancel_where(id, scope.as_deref()) {
        Ok(true) => tool_result(call, format!("已关闭 {id}")),
        Ok(false) => tool_result(
            call,
            format!("没有 id 为 {id} 的定时任务；用 scheduler_list 查看"),
        ),
        Err(e) => tool_result(call, format!("Error: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::CRON;
    use cordis_base::types::ToolCall;

    fn ctx_with_cron() -> Context {
        let root = Context::new();
        root.provide(CRON, Cron::new()).unwrap();
        root
    }

    fn call(args: &str) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: "scheduler_create".into(),
            arguments: args.into(),
        }
    }

    #[test]
    fn create_honors_fire_immediately() {
        let ctx = ctx_with_cron();
        let result = create_job(
            &ctx,
            call(r#"{"interval":"5m","prompt":"check","fire_immediately":true}"#),
        );
        assert!(result.content.contains("已设定"), "{}", result.content);
        assert!(
            result.content.contains("立即执行：是"),
            "{}",
            result.content
        );
        let cron = ctx.get::<Cron>(CRON).unwrap();
        let tick = cron.due().unwrap();
        assert_eq!(tick.fires.len(), 1);
        assert_eq!(tick.fires[0].job.prompt, "check");
    }

    #[test]
    fn create_without_fire_immediately_waits() {
        let ctx = ctx_with_cron();
        create_job(&ctx, call(r#"{"interval":"5m","prompt":"later"}"#));
        let cron = ctx.get::<Cron>(CRON).unwrap();
        assert!(cron.due().unwrap().fires.is_empty());
        assert_eq!(cron.list().len(), 1);
    }

    #[test]
    fn update_in_place_keeps_id() {
        let ctx = ctx_with_cron();
        create_job(&ctx, call(r#"{"interval":"5m","prompt":"old"}"#));
        let cron = ctx.get::<Cron>(CRON).unwrap();
        let id = cron.list()[0].id.clone();
        let updated = create_job(
            &ctx,
            call(&format!(r#"{{"task_id":"{id}","prompt":"new"}}"#)),
        );
        assert!(updated.content.contains("已更新"), "{}", updated.content);
        let jobs = cron.list();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, id);
        assert_eq!(jobs[0].prompt, "new");
    }

    #[test]
    fn unknown_task_id_does_not_create() {
        let ctx = ctx_with_cron();
        let result = create_job(&ctx, call(r#"{"task_id":"cron-missing","prompt":"x"}"#));
        assert!(
            result.content.contains("unknown task_id"),
            "{}",
            result.content
        );
        let cron = ctx.get::<Cron>(CRON).unwrap();
        assert!(cron.list().is_empty());
    }

    #[test]
    fn create_accepts_cron_and_at() {
        let ctx = ctx_with_cron();
        let cron_result = create_job(
            &ctx,
            call(r#"{"cron":"0 9 * * 1-5","prompt":"汇总昨天的提交"}"#),
        );
        assert!(
            cron_result.content.contains("工作日 09:00"),
            "{}",
            cron_result.content
        );
        assert!(
            cron_result.content.contains("不会自动过期"),
            "{}",
            cron_result.content
        );
        let once = create_job(&ctx, call(r#"{"at":"30m","prompt":"提醒我看邮件"}"#));
        assert!(once.content.contains("一次 · "), "{}", once.content);
        let cron = ctx.get::<Cron>(CRON).unwrap();
        let jobs = cron.list();
        assert_eq!(jobs.len(), 2);
        assert!(matches!(jobs[0].schedule, CronSchedule::Cron(ref e) if e == "0 9 * * 1-5"));
        assert!(jobs[0].catch_up, "cron 默认补跑");
        assert!(matches!(jobs[1].schedule, CronSchedule::At(_)));
    }

    #[test]
    fn create_rejects_two_schedules_and_bad_values() {
        let ctx = ctx_with_cron();
        for args in [
            r#"{"interval":"5m","cron":"0 9 * * *","prompt":"x"}"#,
            r#"{"cron":"every morning","prompt":"x"}"#,
            r#"{"at":"2020-01-01 09:00","prompt":"x"}"#,
            r#"{"interval":"5m","prompt":"x","max_runs":0}"#,
            r#"{"interval":"5m","prompt":"x","expires_in":"soon"}"#,
            r#"{"prompt":"x"}"#,
        ] {
            let result = create_job(&ctx, call(args));
            assert!(
                result.content.starts_with("Error"),
                "{args}: {}",
                result.content
            );
        }
        assert!(ctx.get::<Cron>(CRON).unwrap().list().is_empty());
    }

    #[test]
    fn expires_in_and_max_runs_are_honored() {
        let ctx = ctx_with_cron();
        create_job(
            &ctx,
            call(r#"{"interval":"5m","prompt":"盯着","expires_in":"never","max_runs":3}"#),
        );
        let cron = ctx.get::<Cron>(CRON).unwrap();
        let job = cron.list()[0].clone();
        assert_eq!(job.expires_at_ms, None);
        assert_eq!(job.max_runs, Some(3));
        let updated = create_job(
            &ctx,
            call(&format!(
                r#"{{"task_id":"{}","cron":"0 18 * * 5","expires_in":"30d"}}"#,
                job.id
            )),
        );
        assert!(
            updated.content.contains("每周五 18:00"),
            "{}",
            updated.content
        );
        let job = cron.get(&job.id).unwrap();
        assert!(matches!(job.schedule, CronSchedule::Cron(_)));
        assert!(job.expires_at_ms.is_some());
    }

    #[test]
    fn create_accepts_watch_files_and_manual() {
        let ctx = ctx_with_cron();
        let watch = create_job(
            &ctx,
            call(
                r#"{"watch":"gh pr checks 12","watch_every":"2m","fire_when":"fails","prompt":"CI 挂了就看看","max_runs":1}"#,
            ),
        );
        assert!(
            watch.content.contains("命令 · 每 2 分钟查一次，失败时触发"),
            "{}",
            watch.content
        );
        assert!(watch.content.contains("下次检查"), "{}", watch.content);
        let files = create_job(
            &ctx,
            call(
                r#"{"files":["docs/**/*.md"],"debounce":"30s","prompt":"同步英文版","new_session":true}"#,
            ),
        );
        assert!(
            files.content.contains("文件 · docs/**/*.md 有改动"),
            "{}",
            files.content
        );
        assert!(
            files.content.contains("每次在这个项目里新开一个会话"),
            "{}",
            files.content
        );
        let manual = create_job(&ctx, call(r#"{"manual":true,"prompt":"发版前检查"}"#));
        assert!(
            manual.content.contains("dock schedule run"),
            "{}",
            manual.content
        );

        let cron = ctx.get::<Cron>(CRON).unwrap();
        let jobs = cron.list();
        assert_eq!(jobs.len(), 3);
        let CronSchedule::Watch(spec) = &jobs[0].schedule else {
            panic!("{:?}", jobs[0].schedule);
        };
        assert_eq!(spec.every, Duration::from_secs(120));
        assert_eq!(spec.when, WatchWhen::Fails);
        assert_eq!(jobs[0].max_runs, Some(1));
        let CronSchedule::Files(spec) = &jobs[1].schedule else {
            panic!("{:?}", jobs[1].schedule);
        };
        assert_eq!(spec.debounce, Duration::from_secs(30));
        assert_eq!(jobs[1].target, CronTarget::NewSession);
        assert_eq!(jobs[2].schedule, CronSchedule::Manual);
    }

    #[test]
    fn watch_and_files_arguments_are_validated() {
        // `~` 要读 HOME：和改 HOME 的用例互斥。
        let _env = cordis_base::test_env::scoped();
        let ctx = ctx_with_cron();
        for args in [
            r#"{"watch":"ls","fire_when":"matches","prompt":"x"}"#,
            r#"{"watch":"ls","fire_when":"sometimes","prompt":"x"}"#,
            r#"{"watch":"ls","match":"(","fire_when":"matches","prompt":"x"}"#,
            r#"{"files":[],"prompt":"x"}"#,
            r#"{"files":["a"],"debounce":"soon","prompt":"x"}"#,
            r#"{"watch":"ls","manual":true,"prompt":"x"}"#,
            // 短于一分钟报错，不悄悄提到一分钟（#310 审阅）。
            r#"{"watch":"ls","watch_every":"30s","prompt":"x"}"#,
            // 监听范围太大（目录递归听）：根、家目录。
            r#"{"files":["/"],"prompt":"x"}"#,
            r#"{"files":["~"],"prompt":"x"}"#,
            r#"{"files":["docs","~/**/*.md"],"prompt":"x"}"#,
        ] {
            let result = create_job(&ctx, call(args));
            assert!(
                result.content.starts_with("Error"),
                "{args}: {}",
                result.content
            );
        }
        assert!(ctx.get::<Cron>(CRON).unwrap().list().is_empty());

        // 改成太宽的根也一样拒；`1d` 这种检查间隔照常认。
        let made = create_job(
            &ctx,
            call(r#"{"watch":"ls","watch_every":"1d","prompt":"x"}"#),
        );
        assert!(!made.content.starts_with("Error"), "{}", made.content);
        let id = ctx.get::<Cron>(CRON).unwrap().list()[0].id.clone();
        let widened = create_job(
            &ctx,
            call(&format!(r#"{{"task_id":"{id}","files":["/"]}}"#)),
        );
        assert!(
            widened.content.contains("监听范围太大"),
            "{}",
            widened.content
        );
    }

    /// 每一轮怎么批：默认智能授权；模型能选询问，选不了始终允许；改的时候只能收紧，
    /// 用户设成询问的它改不回智能授权。
    #[test]
    fn the_model_can_only_tighten_a_tasks_approval() {
        let ctx = ctx_with_cron();
        let cron = ctx.get::<Cron>(CRON).unwrap();
        let made = create_job(&ctx, call(r#"{"manual":true,"prompt":"出周报"}"#));
        assert!(
            made.content
                .contains("每次运行：智能授权，权限请求等 15 分钟没人批就跳过这一步"),
            "{}",
            made.content
        );
        let id = cron.list()[0].id.clone();
        assert_eq!(cron.get(&id).unwrap().approval, CronApproval::Smart);

        let allow = create_job(
            &ctx,
            call(r#"{"manual":true,"prompt":"x","approval":"allow"}"#),
        );
        assert!(allow.content.starts_with("Error"), "{}", allow.content);
        assert_eq!(cron.list().len(), 1);

        let tighter = create_job(
            &ctx,
            call(&format!(r#"{{"task_id":"{id}","approval":"ask"}}"#)),
        );
        assert!(!tighter.content.starts_with("Error"), "{}", tighter.content);
        assert_eq!(cron.get(&id).unwrap().approval, CronApproval::Ask);
        let looser = create_job(
            &ctx,
            call(&format!(r#"{{"task_id":"{id}","approval":"smart"}}"#)),
        );
        assert!(
            looser.content.contains("only make it stricter"),
            "{}",
            looser.content
        );
        assert_eq!(cron.get(&id).unwrap().approval, CronApproval::Ask);
    }

    #[test]
    fn paused_pauses_and_resumes_through_task_id() {
        let ctx = ctx_with_cron();
        create_job(&ctx, call(r#"{"interval":"5m","prompt":"盯着"}"#));
        let cron = ctx.get::<Cron>(CRON).unwrap();
        let id = cron.list()[0].id.clone();
        let paused = create_job(
            &ctx,
            call(&format!(r#"{{"task_id":"{id}","paused":true}}"#)),
        );
        assert!(paused.content.contains("已暂停"), "{}", paused.content);
        assert!(cron.get(&id).unwrap().paused);
        let listed = list_jobs(&ctx, call("{}"));
        assert!(listed.content.contains("已暂停"), "{}", listed.content);
        create_job(
            &ctx,
            call(&format!(r#"{{"task_id":"{id}","paused":false}}"#)),
        );
        assert!(!cron.get(&id).unwrap().paused);
    }

    /// 两个会话各建各的：任务记下所属会话，工具只看得到、删得掉自己会话的。
    #[test]
    fn tasks_belong_to_the_calling_session() {
        let _home = cordis_base::test_env::scoped().home();
        let root = ctx_with_cron();
        let page = |index: usize| {
            let page = root.isolate(SESSIONS);
            let sessions = Sessions::tab(page.clone(), index);
            sessions.attach_disk();
            std::mem::forget(page.provide(SESSIONS, sessions).unwrap());
            page
        };
        let (a, b) = (page(2), page(3));
        let in_page = |p: &Context, args: &str, name: &str| {
            let call = ToolCall {
                id: "c".into(),
                name: name.into(),
                arguments: args.into(),
            };
            crate::tools::registry::with_exec_ctx(p, || match name {
                "scheduler_create" => create_job(&root, call),
                "scheduler_list" => list_jobs(&root, call),
                _ => delete_job(&root, call),
            })
        };
        in_page(
            &a,
            r#"{"interval":"5m","prompt":"from a"}"#,
            "scheduler_create",
        );
        let cron = root.get::<Cron>(CRON).unwrap();
        let job = cron.list()[0].clone();
        let a_session = a.get::<Sessions>(SESSIONS).unwrap().live_session_id();
        assert_eq!(
            job.owner.as_ref().map(|o| o.session.as_str()),
            Some(a_session.as_str())
        );

        let seen_by_b = in_page(&b, "{}", "scheduler_list");
        assert!(
            !seen_by_b.content.contains("from a"),
            "{}",
            seen_by_b.content
        );
        let deleted_by_b = in_page(&b, &format!(r#"{{"id":"{}"}}"#, job.id), "scheduler_delete");
        assert!(
            deleted_by_b.content.contains("没有 id"),
            "{}",
            deleted_by_b.content
        );
        assert_eq!(cron.list().len(), 1);

        let seen_by_a = in_page(&a, "{}", "scheduler_list");
        assert!(
            seen_by_a.content.contains("from a"),
            "{}",
            seen_by_a.content
        );
    }
}
