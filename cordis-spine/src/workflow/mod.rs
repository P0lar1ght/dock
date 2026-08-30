//! Grok `workflow` tool + Rhai engine, mounted as Cordis `"workflows"` + `"tools"`.

mod drain;
mod grok_tool;
mod registry;

use std::sync::Arc;

use cordis::{Inject, Plugin, plugin};

use crate::names::{TOOLS, WORKFLOWS};
use crate::tools::{ToolBody, Tools, own_registered, tool_result};
use crate::types::{ToolCall, ToolResult, ToolSpec};

pub use drain::WorkflowRunSnap;
pub use grok_tool::{WORKFLOW_TOOL_NAME, WorkflowLaunchHandle, WorkflowToolInput, render_ack};

/// Names + descriptions of workflows the model can launch (builtin + disk).
pub(crate) fn catalog_listing() -> Vec<(String, String)> {
    let cwd = std::env::current_dir().ok();
    registry::list_workflows(cwd.as_deref())
        .into_iter()
        .map(|w| (w.name, w.description))
        .collect()
}

/// Named `"workflows"` service. TUI live-looks `list()`.
pub struct Workflows {
    state: Arc<drain::WorkflowState>,
    pub handle: WorkflowLaunchHandle,
}

impl Workflows {
    pub fn list(&self) -> Vec<WorkflowRunSnap> {
        self.state.list()
    }
}

const DESC: &str = "启动工作流：一段 Rhai 脚本，把子代理编排成一次后台运行。\
source 只能有一个：已注册 name、内联 script、script_path，或同进程 resume。\
可选 args（绑到脚本 args）和 agent_budget（子代理调用上限，默认 128，最大 1024）。\
调用立即返回；进度看 /workflow runs，完成后会自动汇报，不要轮询 wait_tasks。\
validate_only: true 只做冒烟检查（元数据、编译、一条 canned-host 路径），不证明每个分支或真实工具可用。\
可复用脚本放到 .dock/workflows/<name>.rhai。";

const PARAMS: &str = r#"{"type":"object","properties":{"source":{"description":"Exactly one workflow source.","oneOf":[{"type":"object","required":["type","name"],"properties":{"type":{"const":"name"},"name":{"type":"string"}}},{"type":"object","required":["type","script"],"properties":{"type":{"const":"script"},"script":{"type":"string"}}},{"type":"object","required":["type","script_path"],"properties":{"type":{"const":"script_path"},"script_path":{"type":"string"}}},{"type":"object","required":["type","resume_from_run_id"],"properties":{"type":{"const":"resume"},"resume_from_run_id":{"type":"string"}}}]},"agent_budget":{"type":"integer","minimum":1,"maximum":1024},"args":{},"validate_only":{"type":"boolean"},"name":{"type":"string"},"script":{"type":"string"},"script_path":{"type":"string"},"resume_from_run_id":{"type":"string"}},"required":[]}"#;

pub fn tool_workflow() -> Plugin {
    plugin("tool-workflow", Inject::from([TOOLS]), |ctx, _: &()| {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(drain::WorkflowState::new());
        {
            let state = state.clone();
            let ctx = ctx.clone();
            tokio::spawn(drain::drain_loop_with_ctx(state, ctx, rx));
        }
        ctx.provide(
            WORKFLOWS,
            Workflows {
                state,
                handle: WorkflowLaunchHandle(tx),
            },
        )?;
        let tools = ctx.require::<Tools>(TOOLS)?;
        let body: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
                Box::pin(async move { run_workflow_tool(&ctx, call).await })
            })
        };
        own_registered(
            ctx,
            vec![tools.register(
                ToolSpec {
                    name: WORKFLOW_TOOL_NAME.into(),
                    description: DESC.into(),
                    parameters_json: PARAMS.into(),
                },
                body,
            )?],
        )?;
        Ok(None)
    })
}

/// Copied from Grok `WorkflowTool::run` (handle → oneshot ack → render).
async fn run_workflow_tool(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let input: WorkflowToolInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: workflow_invalid_input: {e}")),
    };
    let Some(wf) = ctx.get::<Workflows>(WORKFLOWS) else {
        return tool_result(
            call,
            "Error: workflow_not_available: workflows is not mounted",
        );
    };
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    if wf
        .handle
        .0
        .send((grok_tool::WorkflowLaunchRequest { input }, ack_tx))
        .is_err()
    {
        return tool_result(
            call,
            "Error: workflow_channel_closed: the session may be shutting down",
        );
    }
    match ack_rx.await {
        Ok(ack) => match render_ack(ack) {
            Ok(out) => tool_result(
                call,
                serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.message),
            ),
            Err((code, detail)) => tool_result(call, format!("Error: {code}: {detail}")),
        },
        Err(_) => tool_result(
            call,
            "Error: workflow_launch_no_ack: the session dropped the launch channel before answering",
        ),
    }
}
