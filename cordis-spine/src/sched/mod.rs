//! Scheduler tools over `"cron"` — Grok `scheduler_*` names, DSH register.

mod interval;
mod loop_cmd;

use std::time::{Duration, Instant};

use cordis::{plugin, Context, Inject, Plugin};
use serde_json::Value;

use crate::cron::{Cron, CronError, MAX_SCHEDULED_TASKS, RECURRING_TASK_TTL_DAYS};
use crate::names::{CRON, TOOLS};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

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
                            description: "List all active scheduled tasks with their IDs, prompts, intervals, and next fire times.".into(),
                            parameters_json: LIST_PARAMS.into(),
                        },
                        list,
                    )?,
                    tools.register(
                        ToolSpec {
                            name: "scheduler_delete".into(),
                            description: "Cancel a scheduled task by ID.".into(),
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
        return match cron.update(id, every, prompt) {
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
    match cron.add_ex(Duration::from_secs(secs), prompt, fire_immediately) {
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
    let jobs = cron.list();
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
    if cron.cancel(id) {
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
    use crate::types::ToolCall;

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
}
