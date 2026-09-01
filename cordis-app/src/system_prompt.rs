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
做法不明确时用 enter_plan_mode / exit_plan_mode。\
子代理用 task / subagent。\
MCP 集成和不常用的本地能力（scheduler_*、memory_*、monitor、update_goal、lsp、skill、workflow、cordis_*）不在常驻工具表里：用 search_tool 按关键词发现（默认最多 5 条、上限 255，每项带完整 input_schema），再用 use_tool 调用。没命中的继续藏着，total_hidden_tools 是目录总数。schema 已在当前上下文里就可以反复 use_tool；换没见过的工具、新会话、子代理、压缩后再用，要再 search。禁止猜参数名。内置工具不要走 use_tool。\
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
