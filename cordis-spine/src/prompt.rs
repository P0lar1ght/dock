use cordis::{plugin, Context, Inject, Plugin};

use crate::names::{PROMPT_ASSEMBLE, SYSTEM_PROMPT};

/// Section slots on the `system-prompt/assemble` waterfall. Each contributor is
/// its own plugin handler that adds a section at a fixed order, so the assembled
/// prompt is byte-stable no matter what order the plugins mount in.
pub const ORDER_CORDIS: i32 = 10;
pub const ORDER_PERSONA: i32 = 20;
pub const ORDER_ROSTER: i32 = 30;
pub const ORDER_PLAN: i32 = 40;
pub const ORDER_GOAL: i32 = 50;

/// Structured payload carried by the `system-prompt/assemble` waterfall.
///
/// Instead of each plugin blindly string-appending, it calls `args.next()` to
/// get the accumulated assembly, then adds its own ordered [`section`] (or, for
/// a `replace_prompt` preset, [`replace_base`]). [`render`] sorts by order and
/// joins with a blank line — so the final prompt does not depend on plugin
/// mount order, only on the section slots above.
///
/// [`section`]: PromptAssembly::section
/// [`replace_base`]: PromptAssembly::replace_base
/// [`render`]: PromptAssembly::render
#[derive(Clone, Default)]
pub struct PromptAssembly {
    base: String,
    replace: Option<String>,
    sections: Vec<PromptSection>,
}

#[derive(Clone)]
struct PromptSection {
    order: i32,
    id: String,
    body: String,
}

impl PromptAssembly {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            replace: None,
            sections: Vec::new(),
        }
    }

    /// Set the base text the assembly starts from (the `system-prompt.base`
    /// plugin). Overridden by [`replace_base`](Self::replace_base) at render.
    pub fn set_base(&mut self, base: impl Into<String>) {
        self.base = base.into();
    }

    /// Replace the base entirely — e.g. a `replace_prompt` preset whose persona
    /// *is* the whole prompt. Sections still apply on top of the replacement.
    pub fn replace_base(&mut self, body: impl Into<String>) {
        self.replace = Some(body.into());
    }

    /// Whether the base has been replaced. Addon plugins skip their section when
    /// a replace preset owns the prompt.
    pub fn replaced(&self) -> bool {
        self.replace.is_some()
    }

    /// Add a section at `order` (see the `ORDER_*` slots). Idempotent by `id`:
    /// the first write wins, later same-`id` adds are ignored.
    pub fn section(&mut self, order: i32, id: &str, body: impl Into<String>) {
        if self.sections.iter().any(|s| s.id == id) {
            return;
        }
        self.sections.push(PromptSection {
            order,
            id: id.to_string(),
            body: body.into(),
        });
    }

    /// Base (or replacement) followed by each section in order, joined by a
    /// blank line.
    pub fn render(&self) -> String {
        let mut out = self.replace.clone().unwrap_or_else(|| self.base.clone());
        let mut secs: Vec<&PromptSection> = self.sections.iter().collect();
        secs.sort_by_key(|s| s.order);
        for s in secs {
            out.push_str("\n\n");
            out.push_str(&s.body);
        }
        out
    }
}

/// Abstract `systemPrompt`. Fake assemble returns a fixed string.
/// Grok caches this on `Agent`; DSH assembles per step via the registry.
#[derive(Clone)]
pub struct SystemPrompt {
    ctx: Context,
    text: String,
}

impl SystemPrompt {
    pub fn fake(ctx: Context) -> Self {
        Self {
            ctx,
            text: "You are a test agent.".into(),
        }
    }

    pub fn assemble(&self) -> String {
        self.assemble_on(&self.ctx)
    }

    /// Run the `system-prompt/assemble` waterfall and render. This assembler
    /// owns no prompt content — every fragment (base, persona, roster, plan,
    /// goal, cordis) is contributed by a plugin's waterfall handler.
    pub fn assemble_on(&self, exec: &Context) -> String {
        let seed = PromptAssembly::new(self.text.clone());
        exec.waterfall(PROMPT_ASSEMBLE, seed.clone(), move || seed)
            .render()
    }
}

pub fn system_prompt() -> Plugin {
    plugin("system-prompt", Inject::new(), |ctx, _: &()| {
        Ok(Some(
            ctx.provide(SYSTEM_PROMPT, SystemPrompt::fake(ctx.clone()))?,
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::{GOAL, PLAN_MODE};
    use crate::plan_mode::PlanMode;
    use crate::Goal;

    #[tokio::test]
    async fn assemble_appends_plan_addon_when_active() {
        let ctx = cordis::Context::new();
        crate::install_without_llm(&ctx).await.unwrap();
        ctx.plugin(crate::plan_mode(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let system = SystemPrompt::fake(ctx.clone());
        assert!(!system.assemble().contains("计划模式已开启"));
        ctx.get::<PlanMode>(PLAN_MODE).unwrap().set(true);
        let assembled = system.assemble();
        assert!(assembled.contains("计划模式已开启"), "{assembled}");
        assert!(assembled.contains("exit_plan_mode"), "{assembled}");
        assert!(assembled.contains("ask_user_question"), "{assembled}");
    }

    #[tokio::test]
    async fn assemble_offers_goal_when_idle() {
        let ctx = cordis::Context::new();
        crate::install_without_llm(&ctx).await.unwrap();
        ctx.plugin(crate::tool_goal(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let system = SystemPrompt::fake(ctx.clone());
        let assembled = system.assemble();
        assert!(assembled.contains("update_goal(objective"), "{assembled}");
        assert!(!assembled.contains("已设定目标"), "{assembled}");
    }

    #[tokio::test]
    async fn assemble_instructs_when_goal_active() {
        let ctx = cordis::Context::new();
        crate::install_without_llm(&ctx).await.unwrap();
        ctx.plugin(crate::tool_goal(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        ctx.get::<Goal>(GOAL).unwrap().start("理解 TUI");
        let system = SystemPrompt::fake(ctx.clone());
        let assembled = system.assemble();
        assert!(assembled.contains("已设定目标：理解 TUI"), "{assembled}");
        assert!(!assembled.contains("update_goal(objective"), "{assembled}");
    }
}
