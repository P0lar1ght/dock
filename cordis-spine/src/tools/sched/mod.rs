//! Scheduler tools over `"cron"` — Grok `scheduler_*` names, DSH register.

mod interval;
mod loop_cmd;

use std::time::{Duration, Instant};

use cordis::{plugin, Context, Inject, Plugin};
use serde_json::Value;

use crate::names::{CRON, PLAN_MODE, SESSIONS, TOOLS};
use crate::session::log::Sessions;
use crate::tools::cron::{
    Cron, CronError, CronOwner, MAX_SCHEDULED_TASKS, RECURRING_TASK_TTL_DAYS,
};
use crate::tools::plan_mode::PlanMode;
use crate::tools::registry::{exec_ctx, own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

pub use interval::{interval_to_human, parse_interval};
pub use loop_cmd::{
    expired_task_notice, format_scheduled_task_prompt, format_scheduled_task_reminder,
    loop_composer_fill, loop_schedule_instruction, loop_usage_message, LoopFireMode,
    SCHEDULER_CREATE_TOOL_NAME,
};

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("invalid interval: {0}")]
    InvalidInterval(String),
}

const CREATE_PARAMS: &str = r#"{"type":"object","properties":{"interval":{"type":"string","description":"Interval, e.g. 5m, 2h, 1d (min 60s)."},"prompt":{"type":"string","description":"Prompt to run on each fire."},"task_id":{"type":"string","description":"Existing id to update in place. Omitted fields stay unchanged; the schedule keeps its phase."},"fire_immediately":{"type":"boolean","description":"If true, also fire once on creation. Default false. Ignored when updating."}}}"#;
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
                                "Create a scheduled task that runs a prompt on a recurring interval, or update an existing one in place.\n\n\
Set fire_immediately: true to also fire once on creation; by default the first run waits for the interval.\n\n\
To change an existing task, pass its task_id: provided fields replace old values, omitted ones are unchanged, and the schedule keeps its phase. An unknown id errors.\n\n\
Usage notes:\n\
- Interval format: \"5m\" (minutes), \"2h\" (hours), \"1d\" (days), \"60s\" (seconds, min 60)\n\
- Maximum {MAX_SCHEDULED_TASKS} scheduled tasks at once\n\
- Tasks belong to this session and keep running across Dock restarts; each fire is a new turn here (a closed session is reopened)\n\
- Tasks auto-expire after {RECURRING_TASK_TTL_DAYS} days\n\
- For one-time delayed work, run a background terminal command (e.g. `sleep 1800 && <command>`) instead; its completion notifies you"
                            ),
                            parameters_json: CREATE_PARAMS.into(),
                        },
                        create,
                    )?,
                    tools.register(
                        ToolSpec {
                            name: "scheduler_list".into(),
                            description: "List this session's scheduled tasks with their IDs, prompts, intervals, and next fire times.".into(),
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

fn create_job(ctx: &Context, call: ToolCall) -> ToolResult {
    let v: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
    let interval = v
        .get("interval")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let prompt = v
        .get("prompt")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let task_id = v
        .get("task_id")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let fire_immediately = v
        .get("fire_immediately")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);

    let Some(cron) = ctx.get::<Cron>(CRON) else {
        return tool_result(call, "Error: cron is not mounted");
    };

    if let Some(id) = task_id {
        let every = match interval {
            Some(raw) => match parse_interval(raw) {
                Ok(s) => Some(Duration::from_secs(s)),
                Err(e) => return tool_result(call, e.to_string()),
            },
            None => None,
        };
        if every.is_none() && prompt.is_none() {
            return tool_result(
                call,
                format!("Error: scheduler_create update of {id} needs interval and/or prompt"),
            );
        }
        let scope = caller_owner(ctx).map(|o| o.session);
        return match cron.update_where(id, scope.as_deref(), every, prompt) {
            Ok(job) => {
                let secs = job.every.as_secs();
                tool_result(
                    call,
                    format!(
                        "已更新 {id}（{}）\n提问：{}\n取消：scheduler_delete {id}，或 /tasks 里按 x",
                        interval_to_human(secs),
                        job.prompt
                    ),
                )
            }
            Err(CronError::UnknownId(_)) => tool_result(
                call,
                format!("Error: unknown task_id {id}; call scheduler_list to see active task ids"),
            ),
            Err(e) => tool_result(call, format!("Error: {e}")),
        };
    }

    let Some(interval) = interval else {
        return tool_result(call, "Error: interval is required when creating a task");
    };
    let Some(prompt) = prompt else {
        return tool_result(call, "Error: prompt is required when creating a task");
    };
    let secs = match parse_interval(interval) {
        Ok(s) => s,
        Err(e) => return tool_result(call, e.to_string()),
    };
    match cron.add_owned(
        caller_owner(ctx),
        Duration::from_secs(secs),
        prompt,
        fire_immediately,
    ) {
        Ok(id) => {
            let immediately = if fire_immediately { "是" } else { "否" };
            tool_result(
                call,
                format!(
                    "已设定 {id}（{}）\n立即执行：{immediately}\n{RECURRING_TASK_TTL_DAYS} 天后自动过期\n提问：{prompt}\n取消：scheduler_delete {id}，或 /tasks 里按 x",
                    interval_to_human(secs),
                ),
            )
        }
        Err(CronError::LimitReached(n)) => {
            tool_result(call, format!("Error: maximum {n} scheduled tasks"))
        }
        Err(e) => tool_result(call, format!("Error: {e}")),
    }
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
    let now = Instant::now();
    let mut out = String::new();
    for job in jobs {
        let secs = job.every.as_secs();
        let next = if job.next > now {
            format!("下次 {}", human_wait(job.next.duration_since(now)))
        } else {
            "到期".into()
        };
        out.push_str(&format!(
            "{}：{} — {}（{next}）\n",
            job.id,
            interval_to_human(secs),
            job.prompt
        ));
    }
    tool_result(call, out)
}

fn human_wait(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
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
    if cron.cancel_where(id, scope.as_deref()) {
        tool_result(call, format!("已关闭 {id}"))
    } else {
        tool_result(
            call,
            format!("没有 id 为 {id} 的定时任务；用 scheduler_list 查看"),
        )
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
        let tick = cron.due();
        assert_eq!(tick.fires.len(), 1);
        assert_eq!(tick.fires[0].prompt, "check");
    }

    #[test]
    fn create_without_fire_immediately_waits() {
        let ctx = ctx_with_cron();
        create_job(&ctx, call(r#"{"interval":"5m","prompt":"later"}"#));
        let cron = ctx.get::<Cron>(CRON).unwrap();
        assert!(cron.due().fires.is_empty());
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
