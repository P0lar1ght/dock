//! Base `<system>` prompt content (the actual system prompt text).
//!
//! Registered on `"context"` (`ContextBook::set_base`) so occupancy and
//! `system-prompt/assemble` share the same inventory. Swap the base by swapping
//! this plugin. Display name `system-prompt.base`.
//!
//! The spine `"systemPrompt"` service is the assembler only. Persona / roster
//! come from `agent-presets` (workspace-relative `.dock/presets` paths),
//! listings from `skills` / `tool-workflow`. Plan / goal are history-tail
//! reminders, not system sections. The Cordis pointer comes from
//! `tool-cordis`. Tool how-to lives on `ToolSpec.description`, not here.

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{own_sections, ContextBook, CONTEXT};

const SYSTEM_PROMPT: &str = "思考过程必须使用中文（含 reasoning / 折叠里的思考），不要用英文写思考。\n\n\
你是本地工作区里的编程助手。优先用工具，不要猜文件内容。回复尽量短。\n\
不在常驻工具表里的能力用 search_tool 发现、use_tool 调用；参数以命中的 schema 为准，禁止猜测。内置工具直接调用，不要走 use_tool。";

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

    #[test]
    fn base_does_not_catalog_tools() {
        assert!(!SYSTEM_PROMPT.contains("list_dir"));
        assert!(!SYSTEM_PROMPT.contains("scheduler_"));
        assert!(!SYSTEM_PROMPT.contains("cordis_*"));
        assert!(SYSTEM_PROMPT.contains("search_tool"));
        assert!(SYSTEM_PROMPT.contains("use_tool"));
    }
}
