//! 人用的 `/workflow <名字> [参数]`：现跑一次 `workflow` 工具。看运行、暂停 / 恢复 /
//! 停止 / 保存要用终端里的运行面板。

use std::sync::Arc;

use cordis::{plugin, Inject, Plugin};
use cordis_base::types::ToolCall;

use super::{workflow_command_arguments, WORKFLOW_TOOL_NAME};
use crate::host::slash::{register_commands, SlashCommand, SlashHandler, SlashOutcome};
use crate::names::{SLASH, TOOLS};
use crate::tools::registry::Tools;

pub fn workflow_command() -> Plugin {
    plugin("command-workflow", Inject::from([SLASH]), |ctx, _: &()| {
        let run: SlashHandler = Arc::new(|page, args| {
            Box::pin(async move {
                let args = args.trim();
                let first = args.split_whitespace().next().unwrap_or("");
                if args.is_empty()
                    || args.eq_ignore_ascii_case("runs")
                    || matches!(first, "pause" | "resume" | "stop" | "save")
                {
                    return SlashOutcome::TerminalOnly("workflow".into());
                }
                let (name, arguments) = match workflow_command_arguments(args) {
                    Ok(v) => v,
                    Err(body) => return SlashOutcome::notice("工作流", body),
                };
                let Some(tools) = page.get::<Tools>(TOOLS) else {
                    return SlashOutcome::notice(format!("/{name}"), "tools 未挂载");
                };
                let result = tools
                    .execute(ToolCall {
                        id: format!("slash-tool-{name}"),
                        name: WORKFLOW_TOOL_NAME.into(),
                        arguments,
                    })
                    .await;
                SlashOutcome::notice(format!("/{name}"), result.content)
            })
        });
        register_commands(
            ctx,
            vec![SlashCommand::host("workflow", "查看工作流运行", run).takes_args(true)],
        )?;
        Ok(None)
    })
}
