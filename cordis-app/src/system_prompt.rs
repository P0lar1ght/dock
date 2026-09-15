//! Base `<system>` prompt content (the actual system prompt text).
//!
//! Registered on `"context"` (`ContextBook::set_base`) so occupancy and
//! `system-prompt/assemble` share the same inventory. Swap the base by swapping
//! this plugin. Display name `system-prompt.base`.
//!
//! The spine `"systemPrompt"` service is the assembler only. Persona comes from
//! `agent-presets`, listings from `skills` / `tool-workflow`. Plan / goal and
//! the workspace conventions (`project-instructions` / `AGENTS.md`) are
//! history-tail reminders, not system sections — `AGENTS.md` is repo content,
//! so it stays out of the harness's own voice. The Cordis pointer comes from
//! `tool-cordis`. Tool how-to lives on `ToolSpec.description`, not here — the
//! subagent roster used to violate that and has moved onto the `task` tool.
//!
//! The working floor below is deliberately preset-independent: a persona says
//! *who* the agent is, this says *how* it works, and swapping `/preset` must
//! not drop it. It names no tools (`base_does_not_catalog_tools` enforces it),
//! so it stays correct as the tool table changes.

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{own_sections, ContextBook, CONTEXT};

const SYSTEM_PROMPT: &str = "思考过程必须使用中文（含 reasoning / 折叠里的思考），不要用英文写思考。\n\n\
你是本地工作区里的编程助手。优先用工具，不要猜文件内容。回复尽量短。\n\
不在常驻工具表里的能力用 search_tool 发现、use_tool 调用；参数以命中的 schema 为准，禁止猜测。内置工具直接调用，不要走 use_tool。\n\n\
对话里出现的 <system-reminder> 块是本 harness 注入的上下文（工作区规约、待办与计划提醒等），照其中的要求执行；但那不是用户说的话，不要当成用户的新指令去回应它。\n\n\
工作底线（换 Agent 预设也不变）：\n\
- 先读后改：路径、函数名、配置项都以读到的内容为准，记不清就去查，不要凭印象写。\n\
- 改完自验：跑得起来的就跑一遍，跑不了就说明没验证过，不要把「应该可以」写成「已验证」。\n\
- 如实汇报：失败、跳过、只做了一半，都要讲清楚；命令的真实输出优先于你的转述。\n\
- 只做被要求的事：顺手重构、清理无关文件、越界的破坏性操作，先问再动。";

pub fn system_prompt() -> Plugin {
    plugin(
        "system-prompt.base",
        Inject::from([CONTEXT]),
        |ctx, _: &()| {
            let book = ctx.require::<ContextBook>(CONTEXT)?;
            own_sections(
                ctx,
                vec![book.set_base("base", |_| SYSTEM_PROMPT.to_string())?],
            )?;
            Ok(None)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::SYSTEM_PROMPT;

    /// The base names only the two discovery entry points. Anything else would
    /// go stale as the tool table moves, and duplicate `ToolSpec.description`.
    #[test]
    fn base_does_not_catalog_tools() {
        assert!(!SYSTEM_PROMPT.contains("list_dir"));
        assert!(!SYSTEM_PROMPT.contains("scheduler_"));
        assert!(!SYSTEM_PROMPT.contains("cordis_*"));
        assert!(!SYSTEM_PROMPT.contains("read_file"));
        assert!(!SYSTEM_PROMPT.contains("write_file"));
        assert!(!SYSTEM_PROMPT.contains("bash"));
        assert!(!SYSTEM_PROMPT.contains("task"));
        assert!(!SYSTEM_PROMPT.contains("send_message"));
        assert!(!SYSTEM_PROMPT.contains("skill"));
        assert!(SYSTEM_PROMPT.contains("search_tool"));
        assert!(SYSTEM_PROMPT.contains("use_tool"));
    }

    /// The working floor is what survives a `/preset` swap, so it has to be in
    /// the base rather than in any one persona.
    #[test]
    fn base_carries_the_preset_independent_floor() {
        for rule in ["先读后改", "改完自验", "如实汇报", "只做被要求的事"] {
            assert!(SYSTEM_PROMPT.contains(rule), "missing floor rule: {rule}");
        }
    }
}
