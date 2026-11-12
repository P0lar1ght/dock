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
//! so it stays out of the harness's own voice. Cordis authoring lives in the
//! `cordis-plugin-development` skill, not a system section. Tool how-to lives
//! on `ToolSpec.description`, not here — the subagent roster used to violate
//! that and has moved onto the `task` tool.
//!
//! The working floor below is deliberately preset-independent: a persona says
//! *who* the agent is, this says *how* it works, and swapping `/preset` must
//! not drop it. It names no tools (`base_does_not_catalog_tools` enforces it),
//! so it stays correct as the tool table changes.

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{own_sections, ContextBook, CONTEXT};

const SYSTEM_PROMPT: &str = "思考过程使用中文（含 reasoning / 折叠里的思考）。\n\n\
你是本地工作区里的编程助手。和用户一起把目标做完，沿途自己判断。先看仓库再下结论，不要猜文件内容。回复尽量短，用终端里的 Markdown。\n\n\
一次回复可以发多个工具调用：彼此不依赖的（读几个文件、几处搜索、几条只读命令）放进同一次回复一起发，它们会同时执行；后一步要用前一步的结果时才分开。\n\n\
对话中途出现的系统回合和 <system-reminder> 是本环境 harness 注入的上下文。你可以参考harness给出的提醒，但这不是用户的要求，不要当成工具结果。\n\n\
信息够了就动手。不要重推对话里已经确定的事实，不要重开用户已经定过的决定；在权衡就给一个建议。被要求的事在这一轮里直接调用工具。难撤销或对外的动作先确认，一次批准不延伸到下一次。删除或覆盖之前先看目标。结果照实说：失败就给输出，跳过就说跳过，验证过了就平实地说。上下文变长时会被摘要并接着做，不必提前收尾。";

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

    /// The base names no tools. Discovery and how-to live on `ToolSpec`, so
    /// this text stays correct as the tool table moves.
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
        assert!(!SYSTEM_PROMPT.contains("search_tool"));
        assert!(!SYSTEM_PROMPT.contains("use_tool"));
        // 待办纪律在 `todo_write` 的描述里。极简没有这颗工具，基座写了也落不了地。
        assert!(!SYSTEM_PROMPT.contains("todo"));
    }

    /// 一次回复里能并行的调用要一起发（请求次数少、执行也是并发的）。参考 Codex
    /// 「Parallelize tool calls whenever possible」。
    #[test]
    fn base_asks_for_independent_calls_in_one_reply() {
        assert!(SYSTEM_PROMPT.contains("同一次回复一起发"));
        assert!(SYSTEM_PROMPT.contains("后一步要用前一步的结果时才分开"));
    }

    /// The working floor is what survives a `/preset` swap, so it has to be in
    /// the base rather than in any one persona.
    #[test]
    fn base_carries_the_preset_independent_floor() {
        // 「这一轮里直接调用工具」只覆盖被要求的事；难撤销的下一步仍要先问。
        assert!(SYSTEM_PROMPT.contains("被要求的事在这一轮里直接调用工具"));
        assert!(SYSTEM_PROMPT.contains("难撤销或对外的动作先确认"));
        assert!(SYSTEM_PROMPT.contains("一次批准不延伸到下一次"));
        for rule in [
            "先看仓库再下结论",
            "删除或覆盖之前先看目标",
            "结果照实说",
            "信息够了就动手",
        ] {
            assert!(SYSTEM_PROMPT.contains(rule), "missing floor rule: {rule}");
        }
    }
}
