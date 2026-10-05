//! 人用的 `/plan`（进入计划模式）与 `/view-plan`（看当前计划）。

use cordis::{plugin, Context, Inject, Plugin};

use super::PlanMode;
use crate::host::slash::{register_commands, slash_handler, SlashCommand, SlashOutcome};
use crate::names::{PLAN_MODE, SLASH};

pub fn plan_commands() -> Plugin {
    plugin("command-plan", Inject::from([SLASH]), |ctx, _: &()| {
        register_commands(
            ctx,
            vec![
                SlashCommand::host("plan", "进入计划模式", slash_handler(plan)).takes_args(true),
                SlashCommand::host("view-plan", "查看或批准当前计划", slash_handler(view))
                    .aliases(&["show-plan", "plan-view"]),
            ],
        )?;
        Ok(None)
    })
}

fn plan(page: &Context, args: &str) -> SlashOutcome {
    let Some(plan) = page.get::<PlanMode>(PLAN_MODE) else {
        return SlashOutcome::notice("计划模式", "计划模式未挂载。");
    };
    if args.is_empty() {
        plan.enter_pending();
        return SlashOutcome::Applied("计划模式".into());
    }
    plan.enter_active();
    SlashOutcome::Submit(args.to_string())
}

fn view(page: &Context, _: &str) -> SlashOutcome {
    let Some(plan) = page.get::<PlanMode>(PLAN_MODE) else {
        return SlashOutcome::notice("计划", "计划模式未挂载。");
    };
    match plan.front().or_else(|| plan.disk_preview()) {
        Some(prompt) if prompt.empty => SlashOutcome::notice("当前计划", "计划文件是空的。"),
        Some(prompt) => SlashOutcome::notice("当前计划", prompt.body),
        None => SlashOutcome::notice("计划", "没有可查看的计划。"),
    }
}
