//! `propose_preset`：会话里用户要「做一个 xx agent」时，模型只起草，交给用户在预设
//! 编辑器里逐项审、自己保存——不直接写 `agent.yml`。
//!
//! 草稿走 [`parse_draft`] 同一套校验（工具只留目录里有的、子代理 id 照规矩、最多 3
//! 个），**不写盘**。有图形界面在听（网关在监听）时回草稿，桌面端把这次调用画成
//! 草稿卡，「在编辑器中打开」预填编辑器；没有（终端）就告诉模型照旧直接写文件。

use cordis::{plugin, Inject, Plugin};
use serde_json::json;
#[cfg(test)]
use serde_json::Value;

use super::preset_assist::{parse_draft, ToolChoice};
use crate::host::gateway_port::GatewayRef;
use crate::names::{GATEWAY, TOOLS};
use crate::tools::registry::{exec_ctx, own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

pub const PROPOSE_PRESET_TOOL: &str = "propose_preset";

const DESC: &str = "Draft a new Agent preset (mode) for the user to review and save in the desktop preset editor. Use it when the user asks you to create or customize an agent / preset / mode: you only draft, the user decides every field and saves it themselves — do not write agent.yml yourself. Nothing is written to disk. Tools must be names from this Dock's tool table; at most 3 subagents. If the result says no reviewer is attached (terminal), write .dock/presets/<id>/agent.yml directly instead.";

const PARAMS: &str = r#"{"type":"object","required":["name","persona"],"properties":{"name":{"type":"string","description":"Display name, 2-12 characters."},"description":{"type":"string","description":"One sentence: what it is good at."},"persona":{"type":"string","description":"Role prompt in Markdown: identity, way of working, output format, boundaries."},"tools":{"type":"array","description":"Tool allowlist. Omit for all tools.","items":{"type":"object","required":["name"],"properties":{"name":{"type":"string"},"reason":{"type":"string","description":"Why it needs this tool, under 20 characters."}}}},"subagents":{"type":"array","description":"Optional roles (agents/<id>.yml); the main preset then needs the task tool.","items":{"type":"object","required":["id","persona"],"properties":{"id":{"type":"string","description":"Lowercase letters, digits and -."},"name":{"type":"string"},"description":{"type":"string"},"persona":{"type":"string"},"tools":{"type":"array","items":{"type":"string"}}}}}}}"#;

/// 交给编辑器审时回给模型的话：草稿已经在用户面前，别再写文件、别重复起草。
const REVIEW_NOTE: &str = "草稿已交给用户：对话里有一张草稿卡，用户在预设编辑器里逐项审、改完自己保存。不要再写 agent.yml，也不要重复调用；告诉用户点卡片上的「在编辑器中打开」。";
/// 没人审（终端）时：照旧写文件。
const NO_REVIEWER_NOTE: &str = "没有图形界面可以审阅（终端会话）。按下面校验过的草稿直接写 .dock/presets/<id>/agent.yml（id 用小写字母、数字和 -），子代理写在 agents/<id>.yml，写完让用户 /preset 切过去。";

pub fn tool_propose_preset() -> Plugin {
    plugin(
        "tool-propose-preset",
        Inject::from([TOOLS]),
        |ctx, _: &()| {
            let tools = ctx.require::<Tools>(TOOLS)?;
            let root = ctx.clone();
            let body: ToolBody = std::sync::Arc::new(move |call| {
                let ctx = exec_ctx().unwrap_or_else(|| root.clone());
                Box::pin(async move { propose(&ctx, call) })
            });
            own_registered(
                ctx,
                vec![tools.register(
                    ToolSpec {
                        name: PROPOSE_PRESET_TOOL.into(),
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

fn propose(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let Some(tools) = ctx.get::<Tools>(TOOLS) else {
        return tool_result(call, "Error: 工具表没有挂载");
    };
    let catalog: Vec<ToolChoice> = tools
        .specs()
        .into_iter()
        .filter(|s| !tools.is_mcp(&s.name) && s.name != PROPOSE_PRESET_TOOL)
        .map(|s| ToolChoice {
            summary: String::new(),
            name: s.name,
        })
        .collect();
    let draft = match parse_draft(&call.arguments, &catalog, &[]) {
        Ok(d) => d,
        Err(e) => return tool_result(call, format!("Error: {e}")),
    };
    let reviewer = ctx
        .get::<GatewayRef>(GATEWAY)
        .is_some_and(|g| g.is_listening());
    let out = json!({
        "status": if reviewer { "awaiting_review" } else { "no_reviewer" },
        "note": if reviewer { REVIEW_NOTE } else { NO_REVIEWER_NOTE },
        "draft": draft,
    });
    tool_result(call, out.to_string())
}

/// 草稿里写了几颗工具 / 几个子代理：给测试和文档看的小工具。
#[cfg(test)]
fn counts(v: &Value) -> (usize, usize) {
    let n = |k: &str| {
        v["draft"][k]
            .as_array()
            .map(|a| a.len())
            .unwrap_or_default()
    };
    (n("tools"), n("agents"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{tools, TOOLS};
    use cordis::Context;

    async fn boot() -> Context {
        let root = Context::new();
        root.plugin(tools(), ()).unwrap().wait().await.unwrap();
        root.plugin(tool_propose_preset(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        root
    }

    async fn call(root: &Context, args: Value) -> String {
        root.require::<Tools>(TOOLS)
            .unwrap()
            .execute(ToolCall {
                id: "p1".into(),
                name: PROPOSE_PRESET_TOOL.into(),
                arguments: args.to_string(),
            })
            .await
            .content
    }

    /// 只起草不写盘：工具名照工具表过滤（不认识的丢掉），子代理 id 照规矩校验；
    /// 没有网关在听（终端）就告诉模型照旧写文件。
    #[tokio::test]
    async fn drafts_are_validated_and_terminal_falls_back_to_files() {
        let _env = cordis_base::test_env::scoped().home();
        let root = boot().await;
        let out = call(
            &root,
            json!({
                "name": "代码审查员",
                "description": "只读审代码",
                "persona": "你审代码，不改文件。",
                "tools": [{ "name": "propose_preset" }, { "name": "rm_rf_everything" }],
                "subagents": [
                    { "id": "Bad Id", "persona": "x" },
                    { "id": "security", "name": "安全复查", "persona": "查安全问题" }
                ]
            }),
        )
        .await;
        let v: Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}"));
        assert_eq!(v["status"], "no_reviewer", "{out}");
        assert!(v["note"].as_str().unwrap().contains("agent.yml"), "{out}");
        assert_eq!(v["draft"]["name"], "代码审查员");
        // 工具表里只有 propose_preset 自己，它又不进目录：两颗都丢掉 = 全部工具。
        assert!(v["draft"]["tools"].is_null(), "{out}");
        assert_eq!(counts(&v), (0, 1), "{out}");
        assert_eq!(v["draft"]["agents"][0]["id"], "security");
        assert!(
            !cordis_base::config::dock_home().join("presets").exists(),
            "起草不写盘"
        );

        let empty = call(&root, json!({ "name": "x", "persona": "  " })).await;
        assert!(empty.starts_with("Error:"), "{empty}");
    }
}
