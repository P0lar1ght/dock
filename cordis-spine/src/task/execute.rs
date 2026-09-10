//! Grok `TaskTool::run`, adapted to dock `ToolCall` / `ChannelBackend`.

use serde::Deserialize;

use crate::agent_presets::AgentPresets;
use crate::names::AGENT_PRESETS;
use crate::tools::tool_result;
use crate::types::{ToolCall, ToolResult};

use super::backend::SubagentBackend;
use super::format::{
    format_subagent_auto_backgrounded, format_subagent_completed,
    format_subagent_started_background, sanitize_optional_arg, BackgroundNoticeNaming,
};
use super::runner::PARENT_SESSION_ID;
use super::types::{
    is_valid_resume_id, sanitize_cwd_value, ModelOverrideProvenance, SubagentIsolationMode,
    SubagentOwner, SubagentRequest, SubagentRuntimeOverrides, SubagentValidateTypeOutcome,
};
use super::{current_depth, Subagents, MAX_SUBAGENT_DEPTH};

const TASK_PARAMS: &str = r#"{"type":"object","properties":{"prompt":{"type":"string","description":"The full task prompt for the subagent to execute."},"description":{"type":"string","description":"Short description of the task (3-5 words)."},"subagent_type":{"type":"string","description":"Id from the current Agent mode agents/ roster."},"run_in_background":{"type":"boolean","description":"Returns immediately with a subagent_id. Use get_task_output to retrieve results. Default true."},"resume_from":{"type":"string","description":"Resume a completed subagent_id with a new prompt."},"cwd":{"type":"string","description":"Explicit working directory. Mutually exclusive with isolation=worktree."},"isolation":{"type":"string","description":"none (default) or worktree."},"model":{"type":"string","description":"Optional model slug. Ignored when resume_from is set."}},"required":["prompt","description"]}"#;

const TASK_DESC: &str = "Launch a subagent from the current Agent mode roster.\n\
- prompt: the full task for the child.\n\
- description: 3–5 words.\n\
- subagent_type: id from this mode's agents/ YAML. The tool parameter enum is the callable set (same as the live roster, including user overlay roles).\n\
- run_in_background: default true. Returns subagent_id; collect with get_task_output.\n\
- resume_from: completed/disposed subagent_id only.\n\
Max nesting depth is 1.";

pub(super) fn spec() -> crate::types::ToolSpec {
    crate::types::ToolSpec {
        name: "task".into(),
        description: TASK_DESC.into(),
        parameters_json: TASK_PARAMS.into(),
    }
}

fn default_true() -> bool {
    true
}

/// Copied field set from Grok `TaskToolInput`.
#[derive(Debug, Deserialize)]
struct TaskToolInput {
    prompt: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    subagent_type: String,
    #[serde(default = "default_true")]
    run_in_background: bool,
    #[serde(default)]
    resume_from: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    isolation: Option<SubagentIsolationMode>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
}

pub(super) async fn run_task(ctx: &cordis::Context, sub: &Subagents, call: ToolCall) -> ToolResult {
    let input: TaskToolInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: invalid task arguments: {e}")),
    };
    if input.prompt.trim().is_empty() {
        return tool_result(call, "Error: prompt is required");
    }
    let description = {
        let t = input.description.trim();
        if t.is_empty() {
            "subagent".to_string()
        } else {
            t.to_string()
        }
    };

    let subagent_type = {
        let t = input.subagent_type.trim();
        if !t.is_empty() {
            t.to_string()
        } else {
            default_type(ctx)
        }
    };
    if subagent_type.is_empty() {
        let (agents, presets_root) = ctx
            .get::<AgentPresets>(AGENT_PRESETS)
            .map(|p| (p.workspace_agents_dir(), p.workspace_presets_dir()))
            .unwrap_or_else(|| (".dock/presets/<mode>/agents".into(), ".dock/presets".into()));
        return tool_result(
            call,
            format!(
                "Error: this Agent mode has no subagents. Add agents/<type>.yml under {agents} (not ~/.dock unless the user asked to save globally). To add a new Agent mode, write {presets_root}/<id>/agent.yml (id [a-z0-9][a-z0-9-]*), then /preset apply that id."
            ),
        );
    }

    let depth = current_depth();
    let max_depth = MAX_SUBAGENT_DEPTH;
    if depth >= max_depth {
        return tool_result(
            call,
            format!(
                "Subagent depth limit exceeded (current depth: {depth}, max: {max_depth}). \
                 Cannot spawn further nested subagents."
            ),
        );
    }

    let resume_from = input.resume_from.and_then(|s| {
        let trimmed = s.trim();
        is_valid_resume_id(trimmed).then(|| trimmed.to_string())
    });
    if let Some(ref src) = resume_from {
        if let Some(snap) = sub.snapshot(src) {
            if !snap.done {
                return tool_result(
                    call,
                    format!(
                        "subagent {src} has not completed; resume_from is for a finished subagent_id. \
                         Collect with get_task_output."
                    ),
                );
            }
        }
    }

    let model = sanitize_optional_arg(input.model);
    let model = if resume_from.is_some() {
        if let Some(ref ignored) = model {
            tracing::debug!(
                model = %ignored,
                "ignoring model override because resume_from is set"
            );
        }
        None
    } else {
        model
    };

    let cwd = input.cwd.as_deref().and_then(sanitize_cwd_value);
    let cwd = if cwd.is_some() && input.isolation == Some(SubagentIsolationMode::Worktree) {
        if cwd
            .as_deref()
            .is_some_and(|p| std::path::Path::new(p).is_dir())
        {
            return tool_result(
                call,
                "cwd and isolation=\"worktree\" are mutually exclusive. \
                 Use cwd to point the subagent at an existing directory, \
                 or isolation=\"worktree\" to create a new isolated worktree, \
                 but not both.",
            );
        }
        tracing::debug!(
            cwd = %cwd.as_deref().unwrap_or(""),
            "clearing non-existent cwd path because isolation=worktree is set"
        );
        None
    } else {
        cwd
    };

    if let Some(ref cwd_path) = cwd {
        if resume_from.is_none() {
            let p = std::path::Path::new(cwd_path);
            if !p.is_dir() {
                let detail = if p.exists() {
                    format!("cwd \"{cwd_path}\" exists but is not a directory")
                } else {
                    format!("cwd \"{cwd_path}\" does not exist")
                };
                return tool_result(call, detail);
            }
        }
    }

    if input.isolation == Some(SubagentIsolationMode::Worktree) {
        return tool_result(
            call,
            "isolation=\"worktree\" is not available in this host.",
        );
    }

    spawn_grok_child(
        sub,
        call,
        input.prompt,
        description,
        subagent_type,
        input.run_in_background,
        resume_from,
        cwd,
        input.isolation,
        model,
        input.task_id,
    )
    .await
}

#[allow(clippy::too_many_arguments)] // 绘制 / 布局 / 注册参数天然多，抽结构体只是把参数搬个家，留给需要时再拆
async fn spawn_grok_child(
    sub: &Subagents,
    call: ToolCall,
    prompt: String,
    description: String,
    subagent_type: String,
    run_in_background: bool,
    resume_from: Option<String>,
    cwd: Option<String>,
    isolation: Option<SubagentIsolationMode>,
    model: Option<String>,
    task_id: Option<String>,
) -> ToolResult {
    let depth = current_depth();

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
                format!("Unknown subagent type: {}{suffix}", subagent_type),
            );
        }
        SubagentValidateTypeOutcome::Disabled => {
            return tool_result(
                call,
                format!(
                    "Subagent '{}' is disabled via [subagents.toggle] in config.toml",
                    subagent_type
                ),
            );
        }
        SubagentValidateTypeOutcome::NotAllowed { allowed } => {
            return tool_result(
                call,
                format!(
                    "agent can only spawn: {}; '{}' not allowed",
                    allowed.join(", "),
                    subagent_type
                ),
            );
        }
        SubagentValidateTypeOutcome::ValidationUnavailable => {
            return tool_result(
                call,
                format!(
                    "Cannot validate subagent type '{}': the subagent coordinator is \
                     unreachable. Retry shortly or notify ops.",
                    subagent_type
                ),
            );
        }
    }

    if model.is_some() {
        tracing::debug!("Task.model is ignored; dock children share the parent model");
    }

    let id = task_id
        .filter(|s| super::types::is_not_sentinel(s))
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    sub.remember(
        &id,
        description.clone(),
        subagent_type.clone(),
        SubagentOwner::Task,
    );

    let child_cancellation = tokio_util::sync::CancellationToken::new();
    let request = SubagentRequest {
        id: id.clone(),
        prompt,
        description: description.clone(),
        subagent_type: subagent_type.clone(),
        parent_session_id: PARENT_SESSION_ID.into(),
        parent_prompt_id: None,
        resume_from,
        cwd,
        runtime_overrides: SubagentRuntimeOverrides {
            model,
            model_override_provenance: ModelOverrideProvenance::Tool,
            reasoning_effort: None,
            persona: None,
            capability_mode: None,
            isolation,
            harness_agent_type: None,
            completion_output_cap: None,
            spawn_depth: Some(depth + 1),
            output_token_budget: None,
            output_schema: None,
            loop_task_id: None,
        },
        run_in_background,
        surface_completion: true,
        await_to_completion: false,
        fork_context: false,
        owner: SubagentOwner::Task,
        cancel_token: child_cancellation,
    };

    let naming = BackgroundNoticeNaming::CANONICAL;
    if run_in_background {
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
            format_subagent_started_background(&id, &subagent_type, &description, &naming, false),
        );
    }

    let result = match sub.backend().spawn(request).await {
        Ok(r) => r,
        Err(e) => return tool_result(call, format!("{e}")),
    };

    if result.backgrounded {
        return tool_result(
            call,
            format_subagent_auto_backgrounded(
                &id,
                &subagent_type,
                &description,
                &naming,
                false,
                false,
            ),
        );
    }

    if result.success {
        let body = format_subagent_completed(
            &result.output,
            &result.subagent_id,
            &subagent_type,
            result.tool_calls,
            result.turns,
            result.duration_ms,
            None,
        );
        tool_result(call, body)
    } else {
        tool_result(
            call,
            result
                .error
                .unwrap_or_else(|| "Unknown subagent error".to_string()),
        )
    }
}

fn default_type(ctx: &cordis::Context) -> String {
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        return String::new();
    };
    let roster = presets.current_roster();
    if roster.contains_key("general-purpose") {
        return "general-purpose".into();
    }
    roster.keys().next().cloned().unwrap_or_default()
}
