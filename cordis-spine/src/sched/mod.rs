//! Scheduler tools over `"cron"` — Grok `scheduler_*` names, DSH register.

mod interval;

use std::time::Duration;

use cordis::{plugin, Context, Inject, Plugin};
use serde_json::Value;

use crate::cron::Cron;
use crate::names::{CRON, TOOLS};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

pub use interval::{interval_to_human, parse_interval};

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("invalid interval: {0}")]
    InvalidInterval(String),
}

const CREATE_PARAMS: &str = r#"{"type":"object","properties":{"interval":{"type":"string","description":"Interval, e.g. 5m, 2h, 1d (min 60s)."},"prompt":{"type":"string","description":"Prompt to run on each fire."},"task_id":{"type":"string","description":"Existing id to update."}},"required":["interval","prompt"]}"#;
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
                            description: "Create a scheduled task that runs a prompt on a recurring interval, or update an existing one in place.\n\nSet fire_immediately: true to also fire once on creation; by default the first run waits for the interval.\n\nUsage notes:\n- Interval format: \"5m\" (minutes), \"2h\" (hours), \"1d\" (days), \"60s\" (seconds, min 60)".into(),
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
        .unwrap_or("")
        .trim();
    let prompt = v
        .get("prompt")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    if prompt.is_empty() {
        return tool_result(call, "Error: prompt is required");
    }
    let secs = match parse_interval(interval) {
        Ok(s) => s,
        Err(e) => return tool_result(call, e.to_string()),
    };
    let Some(cron) = ctx.get::<Cron>(CRON) else {
        return tool_result(call, "Error: cron is not mounted");
    };
    if let Some(id) = v.get("task_id").and_then(|x| x.as_str()) {
        if !id.is_empty() {
            cron.cancel(id);
        }
    }
    let id = cron.add(Duration::from_secs(secs), prompt);
    tool_result(
        call,
        format!("scheduled {id} {}\n{}", interval_to_human(secs), prompt),
    )
}

fn list_jobs(ctx: &Context, call: ToolCall) -> ToolResult {
    let Some(cron) = ctx.get::<Cron>(CRON) else {
        return tool_result(call, "Error: cron is not mounted");
    };
    let jobs = cron.list();
    if jobs.is_empty() {
        return tool_result(call, "No scheduled tasks.");
    }
    let mut out = String::new();
    for job in jobs {
        let secs = job.every.as_secs();
        out.push_str(&format!(
            "{}: {} — {}\n",
            job.id,
            interval_to_human(secs),
            job.prompt
        ));
    }
    tool_result(call, out)
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
        tool_result(call, format!("cancelled {id}"))
    } else {
        tool_result(
            call,
            format!("no scheduled task with id {id}; call scheduler_list to see active task ids"),
        )
    }
}
