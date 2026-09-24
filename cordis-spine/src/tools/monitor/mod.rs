//! `monitor` — long-running stdout watch. Lifecycle reuses `"jobs"`.

use cordis::{plugin, Inject, Plugin};

use crate::names::{JOBS, TOOLS};
use crate::tools::jobs::Jobs;
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

const PARAMS: &str = r#"{"type":"object","properties":{"command":{"type":"string","description":"Shell command or script. Each stdout line is an event; exit ends the watch."},"description":{"type":"string","description":"Short human-readable description of what you are monitoring."},"timeout_ms":{"type":"integer","description":"Kill after this many ms. Ignored when persistent is true. Default 36000000 (10h)."},"persistent":{"type":"boolean","description":"Run until kill_task or session end."}},"required":["command","description"]}"#;

const DESC: &str = "Start a background monitor that streams events from a long-running script. Each stdout line is captured on the job; exit ends the watch.\n\n**Output volume**: Print only DONE/FAILED/CANCELLED. Use grep --line-buffered in pipes.\n\nSet persistent: true for session-length watches. Stop with kill_task.";

pub fn tool_monitor() -> Plugin {
    plugin(
        "tool-monitor",
        Inject::from([TOOLS, JOBS]),
        |ctx, _: &()| {
            let tools = ctx.require::<Tools>(TOOLS)?;
            let body: ToolBody = {
                let ctx = ctx.clone();
                std::sync::Arc::new(move |call| {
                    let ctx = ctx.clone();
                    Box::pin(async move { start_monitor(&ctx, call) })
                })
            };
            own_registered(
                ctx,
                vec![tools.register_deferred(
                    ToolSpec {
                        name: "monitor".into(),
                        description: DESC.into(),
                        parameters_json: PARAMS.into(),
                    },
                    body,
                )?],
            )?;
            Ok(None)
        },
    )
}

fn start_monitor(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let v: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or_default();
    let command = v
        .get("command")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    if command.is_empty() {
        return tool_result(call, "Error: command is required");
    }
    let description = v
        .get("description")
        .and_then(|x| x.as_str())
        .unwrap_or("monitor")
        .trim();
    let persistent = v
        .get("persistent")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let timeout_ms = if persistent {
        0
    } else {
        v.get("timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(36_000_000)
            .min(36_000_000)
    };
    let Some(jobs) = ctx.get::<Jobs>(JOBS) else {
        return tool_result(call, "Error: jobs is not mounted");
    };
    // 显式给会话 cwd：不给的话子进程继承进程 cwd，多页时会跑错项目。
    let id = jobs.start_ex_in(
        command,
        Some(description.to_string()),
        true,
        Some(crate::session::cwd::current_cwd()),
    );
    tool_result(
        call,
        format!(
            "Monitor started in background.\n\
             job_id: {id}\n\
             description: {description}\n\
             timeout_ms: {timeout_ms}\n\
             persistent: {persistent}\n\n\
             When you need its result, use the job tool with job_ids=[\"{id}\"] and a positive timeout_ms. Stop with kill_task."
        ),
    )
}
