use cordis::{plugin, Context, Inject, Plugin};

use crate::agent_presets::AgentPresets;
use crate::goal::{goal_instruction, Goal};
use crate::names::{AGENT_PRESETS, GOAL, PLAN_MODE, PROMPT_ASSEMBLE, SYSTEM_PROMPT};
use crate::plan_mode::{plan_system_addon, PlanMode};

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

    pub fn assemble_on(&self, exec: &Context) -> String {
        let text = self.text.clone();
        let mut assembled = exec.waterfall(PROMPT_ASSEMBLE, text.clone(), move || text.clone());
        if let Some(presets) = exec.get::<AgentPresets>(AGENT_PRESETS) {
            presets.merge_persona(&mut assembled);
            presets.merge_subagent_roster(&mut assembled);
        }
        append_mode_addons(exec, &mut assembled);
        assembled
    }
}

pub fn system_prompt() -> Plugin {
    plugin("system-prompt", Inject::new(), |ctx, _: &()| {
        Ok(Some(
            ctx.provide(SYSTEM_PROMPT, SystemPrompt::fake(ctx.clone()))?,
        ))
    })
}

fn append_mode_addons(ctx: &Context, assembled: &mut String) {
    if ctx
        .get::<AgentPresets>(AGENT_PRESETS)
        .is_some_and(|p| p.replaces_prompt())
    {
        return;
    }
    if let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) {
        plan.promote_pending();
        if plan.gated() {
            assembled.push_str("\n\n");
            assembled.push_str(plan_system_addon());
        }
    }
    if let Some(goal) = ctx.get::<Goal>(GOAL) {
        if goal.active() {
            assembled.push_str("\n\n");
            assembled.push_str(&goal_instruction(&goal.title()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan_mode::PlanMode;

    #[tokio::test]
    async fn assemble_appends_plan_addon_when_active() {
        let ctx = cordis::Context::new();
        ctx.provide(PLAN_MODE, PlanMode::new(ctx.clone())).unwrap();
        let system = SystemPrompt::fake(ctx.clone());
        assert!(!system.assemble().contains("计划模式已开启"));
        ctx.get::<PlanMode>(PLAN_MODE).unwrap().set(true);
        let assembled = system.assemble();
        assert!(assembled.contains("计划模式已开启"), "{assembled}");
        assert!(assembled.contains("exit_plan_mode"), "{assembled}");
        assert!(assembled.contains("ask_user_question"), "{assembled}");
    }
}
