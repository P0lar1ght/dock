//! Grok `TaskTool::run`, adapted to dock `ToolCall` / `ChannelBackend`.
//!
//! One spawn tool: the child runs a turn, parks idle, and can be continued
//! with the mailbox tools (`send_message` / `report` / `list_agents` /
//! `interrupt_agent`). A finished turn pushes a notice to the parent, so a
//! background spawn never has to be polled.

use serde::Deserialize;

use crate::agent_presets::AgentPresets;
use crate::names::AGENT_PRESETS;
use crate::tools::tool_result;
use cordis_base::types::{ToolCall, ToolResult};

use super::backend::SubagentBackend;
use super::format::{
    format_subagent_auto_backgrounded, format_subagent_completed,
    format_subagent_started_background, BackgroundNoticeNaming,
};
use super::runner::PARENT_SESSION_ID;
use super::types::{
    is_valid_resume_id, SubagentOwner, SubagentRequest, SubagentRuntimeOverrides,
    SubagentValidateTypeOutcome,
};
use super::{current_depth, Subagents, MAX_SUBAGENT_DEPTH};

const TASK_PARAMS: &str = r#"{"type":"object","properties":{"prompt":{"type":"string","description":"Complete standalone task for this role. The child does not see this conversation."},"description":{"type":"string","description":"Short (3-5 word) label for the delegated work."},"subagent_type":{"type":"string","description":"Role id from the live agents/ roster. The enum on this field is the callable set — extra YAML roles are included."},"run_in_background":{"type":"boolean","description":"Default true. Returns a durable subagent_id immediately; you are notified when a turn ends. Set false only when the next action needs the result now."},"resume_from":{"type":"string","description":"A disposed subagent_id: starts from that child's transcript with this prompt appended."},"reload_roster":{"type":"boolean","description":"If true, re-read this mode's agents/*.yml and return the live subagent_type ids. Does not spawn. Use after writing a new agents/<id>.yml so the next model step's enum includes it. Omit the spawn fields."}},"required":["prompt","description"]}"#;

const TASK_DESC: &str = "Delegate work to a role from the current Agent mode agents/ roster \
(a YAML file under this preset's agents/). The child runs in its own context, keeps a durable \
subagent_id, and stays idle between turns, so a delegation is a conversation rather than a \
one-shot handoff.\n\
- prompt: the full task for the child. It does not see this conversation.\n\
- description: 3-5 words.\n\
- subagent_type: id from this mode's agents/ YAML. The tool parameter enum is the callable set \
(same as the live roster, including user overlay roles).\n\
- run_in_background: default true. Returns subagent_id immediately; its turn end reaches you as \
a notification. Set false only when the next action needs the result now. To collect a one-shot \
result instead of conversing, pair it with get_task_output.\n\
- resume_from: a disposed subagent_id only.\n\
- reload_roster: refresh the subagent_type enum after writing a new agents/<id>.yml (does not spawn).\n\
Talking to a child: send_message (idle — queued and urgent both start its next turn now; urgent \
is send-now only while it is running), list_agents for state, interrupt_agent to stop the current \
turn. The child reaches you with report (many times, across turns), and a turn that ends without \
report has its text forwarded. After writing a new Agent mode directory, apply that id with \
/preset before spawning its roles; reload_roster does not switch modes.\n\
Max nesting depth is 1.";

pub(super) fn spec() -> cordis_base::types::ToolSpec {
    cordis_base::types::ToolSpec {
        name: "task".into(),
        description: TASK_DESC.into(),
        parameters_json: TASK_PARAMS.into(),
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct TaskToolInput {
    #[serde(default)]
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
    reload_roster: bool,
    /// Accepted but not advertised: a server-side id override.
    #[serde(default)]
    task_id: Option<String>,
}

pub(super) async fn run_task(ctx: &cordis::Context, sub: &Subagents, call: ToolCall) -> ToolResult {
    let input: TaskToolInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: invalid task arguments: {e}")),
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
                        "subagent {src} has not been disposed; resume_from is for a disposed subagent_id. \
                         Continue it with send_message, or collect with get_task_output."
                    ),
                );
            }
        }
    }

    spawn_child(
        sub,
        call,
        input.prompt,
        description,
        subagent_type,
        input.run_in_background,
        resume_from,
        input.task_id,
    )
    .await
}

/// 发起这次 `task` 调用的会话身份。
///
/// 工具体是全局注册一次、捕获根 ctx 的，所以要问**执行期** ctx
/// （`tools::exec_ctx`，`Tools::execute_on` 用 task-local 递下来的那个）。
/// 只认主线身份：分页是并列主线（`main#2`…），各自记账；子代理再 spawn 的
/// 孙代理照旧挂在 `main` 上，不动既有的取消语义。
fn caller_session_id() -> String {
    crate::tools::exec_ctx()
        .and_then(|c| c.get::<crate::session::Sessions>(crate::names::SESSIONS))
        .map(|s| s.identity().to_string())
        .filter(|id| crate::session::is_main_identity(id))
        .unwrap_or_else(|| PARENT_SESSION_ID.to_string())
}

#[allow(clippy::too_many_arguments)]
// 参数天然多，抽结构体只是把参数搬个家，留给需要时再拆
async fn spawn_child(
    sub: &Subagents,
    call: ToolCall,
    prompt: String,
    description: String,
    subagent_type: String,
    run_in_background: bool,
    resume_from: Option<String>,
    task_id: Option<String>,
) -> ToolResult {
    let parent_session_id = caller_session_id();
    match sub
        .backend()
        .validate_type(&subagent_type, &parent_session_id)
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

    let id = task_id
        .filter(|s| super::types::is_not_sentinel(s))
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    sub.remember(
        &id,
        description.clone(),
        subagent_type.clone(),
        SubagentOwner::Task,
    );

    let request = SubagentRequest {
        id: id.clone(),
        prompt,
        description: description.clone(),
        subagent_type: subagent_type.clone(),
        parent_session_id,
        resume_from,
        runtime_overrides: SubagentRuntimeOverrides::default(),
        run_in_background,
        surface_completion: true,
        await_to_completion: false,
        owner: SubagentOwner::Task,
        cancel_token: tokio_util::sync::CancellationToken::new(),
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
            format_subagent_started_background(&id, &subagent_type, &description, &naming),
        );
    }

    let result = match sub.backend().spawn(request).await {
        Ok(r) => r,
        Err(e) => return tool_result(call, format!("{e}")),
    };

    if result.backgrounded {
        return tool_result(
            call,
            format_subagent_auto_backgrounded(&id, &subagent_type, &description, &naming),
        );
    }

    // The parent has the result in hand, so drop the queued turn-end notice
    // the child pushed when this turn finished.
    sub.consume_completion(&id);

    if result.success {
        let body = format_subagent_completed(
            &result.output,
            &result.subagent_id,
            &subagent_type,
            result.tool_calls,
            result.turns,
            result.duration_ms,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::SESSIONS;
    use crate::session::Sessions;
    use crate::tools::{ToolBody, Tools};
    use cordis_base::types::ToolSpec;
    use std::sync::{Arc, Mutex};

    fn spec() -> ToolSpec {
        ToolSpec {
            name: "probe-caller".into(),
            description: "test".into(),
            parameters_json: "{}".into(),
        }
    }

    /// 子代理的 spawn 要绑到**发起调用的那一页**上，而不是写死 `main`。
    /// 工具体捕获的是根 ctx，身份只能从执行期 ctx 拿（`Tools::execute_on` 的
    /// task-local）—— 这条测的就是那条通路。
    #[tokio::test]
    async fn spawns_bind_to_the_calling_page() {
        let root = cordis::Context::new();
        let tools = Tools::echo(root.clone());
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let body: ToolBody = {
            let seen = seen.clone();
            Arc::new(move |call| {
                let seen = seen.clone();
                Box::pin(async move {
                    seen.lock().unwrap().push(caller_session_id());
                    crate::tools::tool_result(call, "ok")
                })
            })
        };
        let _hold = tools.register(spec(), body).unwrap();
        let call = || ToolCall {
            id: "1".into(),
            name: "probe-caller".into(),
            arguments: "{}".into(),
        };

        // 第 2 页：身份 main#2，spawn 该记在它自己名下。
        let tab = root.isolate("sessions");
        let _tab_sessions = tab
            .provide(SESSIONS, Sessions::tab(tab.clone(), 2))
            .unwrap();
        tools.execute_on(&tab, call()).await;

        // 子代理（child-…）不是主线：孙代理照旧挂在 main 上，语义不动。
        let child = root.isolate("sessions");
        let _child_sessions = child
            .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-7"))
            .unwrap();
        tools.execute_on(&child, call()).await;

        // 没有会话时退回 main。
        tools.execute_on(&root, call()).await;

        assert_eq!(
            seen.lock().unwrap().as_slice(),
            ["main#2", PARENT_SESSION_ID, PARENT_SESSION_ID]
        );
    }
}
