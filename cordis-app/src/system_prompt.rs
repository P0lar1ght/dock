//! Base `<system>` prompt content (the actual system prompt text).
//!
//! Delivered through the `system-prompt/assemble` waterfall, wrapped in a
//! plugin so it mounts and unwinds with the tree instead of living as a bare
//! closure welded into `main`. Swap the base prompt by swapping this plugin.
//!
//! Note the two layers: `cordis-spine`'s `system_prompt` is the `SystemPrompt`
//! *assembler service* — it only runs the `system-prompt/assemble` waterfall and
//! renders the result. Every fragment is a plugin's waterfall handler: this
//! plugin sets the base text (via [`PromptAssembly::set_base`]); persona/roster
//! come from `agent-presets`, plan from `plan-mode`, goal from `tool-goal`, the
//! dynamic-plugin blurb from `tool-cordis`. Hence the display name
//! `system-prompt.base`. It is the canonical tool catalog; agent presets
//! specialize on top and must not re-list the same tools.

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{PromptAssembly, PROMPT_ASSEMBLE};

const SYSTEM_PROMPT: &str = "思考过程必须使用中文（含 reasoning / 折叠里的思考），不要用英文写思考。\n\n\
你是本地工作区里的编程助手。\
需要访问文件系统时使用 list_dir、read_file、grep、search_replace、glob、write_file、bash。\
长时间命令把 bash 的 is_background 设为 true（或 block_until_ms: 0），再用 get_task_output / wait_tasks / kill_task。\
查网上的近况用 web_search、web_fetch。\
多步进度用 todo_write。\
需要用户做选择时用 ask_user_question。\
做法不明确时用 enter_plan_mode / exit_plan_mode；定时任务用 scheduler_create / scheduler_list / scheduler_delete。\
子代理用 task；代码智能用 lsp；本地记忆用 memory_search / memory_get；盯长命令用 monitor；目标进度用 update_goal；多步编排用 workflow。\
MCP 集成用 search_tool 按关键词发现，再用 use_tool 调用（tool_name 为 mcp_server__tool，例如 mcp_linear__save_issue）；不要把 mcp_* 当一等工具直接调用。\
会话内动态插件用 cordis_inspect / cordis_define / cordis_run / cordis_call / cordis_promote / cordis_stop / cordis_undefine（先读 skills/cordis-plugin-development/SKILL.md）。跨重启写成 .dock/plugins。自定义斜杠或只读 overlay 用 factory slash，不能替换内建命令。\
优先用工具，不要猜文件内容。回复尽量短。";

pub fn system_prompt() -> Plugin {
    plugin("system-prompt.base", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.on_waterfall(
            PROMPT_ASSEMBLE,
            |assembly: PromptAssembly, args| {
                let mut a = args.next::<PromptAssembly>().unwrap_or(assembly);
                a.set_base(SYSTEM_PROMPT);
                a
            },
        )?))
    })
}
