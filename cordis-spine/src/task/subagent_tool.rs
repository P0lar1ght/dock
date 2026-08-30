//! Independent `subagent` grain. Not a wrapper around Grok `task`.
//!
//! Injects `"subagents"` (from `tool-task`) and registers `subagent` plus
//! mailbox tools. Spawn talks to the coordinator directly.

use std::sync::Arc;

use cordis::{Inject, Plugin, plugin};
use serde::Deserialize;

use crate::agent_presets::AgentPresets;
use crate::names::{AGENT_PRESETS, SESSIONS, SUBAGENTS, TOOLS};
use crate::session::Sessions;
use crate::tools::{ToolBody, Tools, own_registered, tool_result};
use crate::types::{ToolCall, ToolResult};

use super::backend::SubagentBackend;
use super::control;
use super::format::{format_continuable_started, format_idle_footer};
use super::runner::PARENT_SESSION_ID;
use super::types::{
    ModelOverrideProvenance, SubagentOwner, SubagentRequest, SubagentRuntimeOverrides,
    SubagentValidateTypeOutcome,
};
use super::{MAX_SUBAGENT_DEPTH, Subagents, current_depth};

const PARAMS: &str = r#"{"type":"object","properties":{"reload_roster":{"type":"boolean","description":"If true, re-read this mode's agents/*.yml and return the live subagent_type ids. Does not spawn. Use after writing a new agents/<id>.yml so the next model step's enum includes it. Omit description, prompt, and subagent_type."},"description":{"type":"string","description":"Short (3-5 word) label for the delegated work. Required when spawning."},"prompt":{"type":"string","description":"Complete standalone task for this role. The child does not see this conversation. Required when spawning."},"subagent_type":{"type":"string","description":"Role id from the live agents/ roster. The enum on this field is the callable set — extra YAML roles are included. Required when spawning."},"run_in_background":{"type":"boolean","description":"Default true. Returns a durable subagent_id immediately. Set false only when the next action needs the result now."}}}"#;

const DESC: &str = "Delegate work to a role from the current Agent mode agents/ roster \
(a YAML file under this preset's agents/). The child runs in its own context. \
subagent_type is that role id. Runs in the background by default and returns a durable \
subagent_id. Follow up with send_message (if idle, queued and urgent both start the next \
turn now; urgent is send-now only while the child is running). List with list_agents; stop a turn with interrupt_agent. The child talks \
to you with report across turns (not a one-shot handoff). Set run_in_background false only when the next action depends on the result. \
After writing a new agents/<id>.yml this session, call with reload_roster true (omit spawn fields) so the next model step's subagent_type enum includes it; that call does not spawn. After writing a new Agent mode directory, apply that id with /preset before spawning its roles; reload_roster does not switch modes.";

pub fn tool_subagent() -> Plugin {
    plugin(
        "tool-subagent",
        Inject::from([TOOLS, SUBAGENTS]),
        |ctx, _: &()| {
            let tools = ctx.require::<Tools>(TOOLS)?;
            let spawn_body: ToolBody = {
                let ctx = ctx.clone();
                Arc::new(move |call: ToolCall| {
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
                            return tool_result(call, "Error: subagents is not mounted");
                        };
                        run(&ctx, &sub, call).await
                    })
                })
            };
            let send_body: ToolBody = {
                let ctx = ctx.clone();
                Arc::new(move |call: ToolCall| {
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
                            return tool_result(call, "Error: subagents is not mounted");
                        };
                        control::run_send(&sub, call).await
                    })
                })
            };
            let list_body: ToolBody = {
                let ctx = ctx.clone();
                Arc::new(move |call: ToolCall| {
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
                            return tool_result(call, "Error: subagents is not mounted");
                        };
                        control::run_list(&sub, call).await
                    })
                })
            };
            let interrupt_body: ToolBody = {
                let ctx = ctx.clone();
                Arc::new(move |call: ToolCall| {
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
                            return tool_result(call, "Error: subagents is not mounted");
                        };
                        control::run_interrupt(&sub, call).await
                    })
                })
            };
            let report_body: ToolBody = {
                let ctx = ctx.clone();
                Arc::new(move |call: ToolCall| {
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
                            return tool_result(call, "Error: subagents is not mounted");
                        };
                        let sessions = crate::tools::exec_ctx()
                            .and_then(|c| c.get::<Sessions>(SESSIONS))
                            .or_else(|| ctx.get::<Sessions>(SESSIONS));
                        control::run_report(&sub, sessions, call).await
                    })
                })
            };
            own_registered(
                ctx,
                vec![
                    tools.register(spec(), spawn_body)?,
                    tools.register(control::send_spec(), send_body)?,
                    tools.register(control::list_spec(), list_body)?,
                    tools.register(control::interrupt_spec(), interrupt_body)?,
                    tools.register(control::report_spec(), report_body)?,
                ],
            )?;
            Ok(None)
        },
    )
}

fn spec() -> crate::types::ToolSpec {
    crate::types::ToolSpec {
        name: "subagent".into(),
        description: DESC.into(),
        parameters_json: PARAMS.into(),
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct Input {
    #[serde(default)]
    description: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    subagent_type: String,
    #[serde(default = "default_true")]
    run_in_background: bool,
    #[serde(default)]
    reload_roster: bool,
}

async fn run(ctx: &cordis::Context, sub: &Subagents, call: ToolCall) -> ToolResult {
    let input: Input = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: invalid subagent arguments: {e}")),
    };
    if input.reload_roster {
        let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
            return tool_result(call, "Error: agentPresets is not mounted");
        };
        return tool_result(call, presets.reload_roster_report());
    }
    if input.prompt.trim().is_empty() {
        return tool_result(call, "Error: prompt is required");
    }
    let description = {
        let t = input.description.trim();
        if t.is_empty() {
            return tool_result(call, "Error: description is required");
        }
        t.to_string()
    };
    let subagent_type = input.subagent_type.trim().to_string();
    if subagent_type.is_empty() {
        return tool_result(
            call,
            roster_hint(
                ctx,
                "Error: subagent_type is required (role id from this mode's agents/<id>.yml).",
            ),
        );
    }
    let depth = current_depth();
    if depth >= MAX_SUBAGENT_DEPTH {
        return tool_result(
            call,
            format!(
                "Subagent depth limit exceeded (current depth: {depth}, max: {MAX_SUBAGENT_DEPTH}). \
                 Cannot spawn further nested subagents."
            ),
        );
    }

    match sub
        .backend()
        .validate_type(&subagent_type, PARENT_SESSION_ID)
        .await
    {
        SubagentValidateTypeOutcome::Ok => {}
        SubagentValidateTypeOutcome::Unknown { available } => {
            let suffix = if available.is_empty() {
                String::new()
            } else {
                format!(". Available types: {}", available.join(", "))
            };
            return tool_result(
                call,
                format!("Unknown subagent type: {subagent_type}{suffix}"),
            );
        }
        SubagentValidateTypeOutcome::Disabled => {
            return tool_result(
                call,
                format!(
                    "Subagent '{subagent_type}' is disabled via [subagents.toggle] in config.toml"
                ),
            );
        }
        SubagentValidateTypeOutcome::NotAllowed { allowed } => {
            return tool_result(
                call,
                format!(
                    "agent can only spawn: {}; '{subagent_type}' not allowed",
                    allowed.join(", ")
                ),
            );
        }
        SubagentValidateTypeOutcome::ValidationUnavailable => {
            return tool_result(
                call,
                format!(
                    "Cannot validate subagent type '{subagent_type}': the subagent coordinator is \
                     unreachable. Retry shortly or notify ops."
                ),
            );
        }
    }

    let id = uuid::Uuid::now_v7().to_string();
    sub.remember(
        &id,
        description.clone(),
        subagent_type.clone(),
        SubagentOwner::Subagent,
    );
    let request = SubagentRequest {
        id: id.clone(),
        prompt: input.prompt,
        description: description.clone(),
        subagent_type: subagent_type.clone(),
        parent_session_id: PARENT_SESSION_ID.into(),
        parent_prompt_id: None,
        resume_from: None,
        cwd: None,
        runtime_overrides: SubagentRuntimeOverrides {
            model: None,
            model_override_provenance: ModelOverrideProvenance::Harness,
            reasoning_effort: None,
            persona: None,
            capability_mode: None,
            isolation: None,
            harness_agent_type: None,
            completion_output_cap: None,
            spawn_depth: Some(depth + 1),
            output_token_budget: None,
            output_schema: None,
            loop_task_id: None,
        },
        run_in_background: input.run_in_background,
        surface_completion: true,
        await_to_completion: false,
        fork_context: false,
        owner: SubagentOwner::Subagent,
        cancel_token: tokio_util::sync::CancellationToken::new(),
    };

    if input.run_in_background {
        let bg_backend = sub.backend().clone();
        let bg_id = id.clone();
        let bg_type = subagent_type.clone();
        tokio::spawn(async move {
            match bg_backend.spawn(request).await {
                Err(e) => {
                    tracing::error!(
                        subagent_id = %bg_id,
                        subagent_type = %bg_type,
                        "background spawn transport error: {e:#}",
                    );
                }
                Ok(r) if !r.success => {
                    tracing::error!(
                        subagent_id = %bg_id,
                        subagent_type = %bg_type,
                        error = ?r.error,
                        "background spawn rejected by coordinator",
                    );
                }
                Ok(_) => {}
            }
        });
        return tool_result(
            call,
            format_continuable_started(&id, &subagent_type, &description),
        );
    }

    let result = match sub.backend().spawn(request).await {
        Ok(r) => r,
        Err(e) => return tool_result(call, format!("{e}")),
    };
    if result.backgrounded {
        return tool_result(
            call,
            format_continuable_started(&id, &subagent_type, &description),
        );
    }
    if result.success {
        let footer = format_idle_footer(&result.subagent_id, &subagent_type);
        tool_result(
            call,
            format!(
                "{}\n\n<subagent_meta>id={}, type={subagent_type}, \
                 tool_calls={}, turns={}, duration_ms={}</subagent_meta>\n\n\
                 {footer}",
                result.output,
                result.subagent_id,
                result.tool_calls,
                result.turns,
                result.duration_ms,
            ),
        )
    } else {
        tool_result(
            call,
            result
                .error
                .unwrap_or_else(|| "Unknown subagent error".to_string()),
        )
    }
}

fn roster_hint(ctx: &cordis::Context, prefix: &str) -> String {
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        return prefix.into();
    };
    let ids: Vec<_> = presets.current_roster().keys().cloned().collect();
    if ids.is_empty() {
        format!("{prefix} This Agent mode has no agents/ roles.")
    } else {
        format!("{prefix} Available: {}.", ids.join(", "))
    }
}
