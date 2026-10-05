//! 人用的 `/plan`（进入计划模式）与 `/view-plan`（看当前计划）。

use cordis::{plugin, Context, Inject, Plugin};

use super::PlanMode;
use crate::host::slash::{register_commands, slash_handler, SlashCommand, SlashOutcome};
use crate::names::{GOAL, PLAN_MODE, SLASH};
use crate::tools::goal::Goal;

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
    // `/goal` 留下的「下一条消息当目标」要撤掉，否则计划说明会被当成目标。
    if let Some(goal) = page.get::<Goal>(GOAL) {
        goal.disarm_composer();
    }
    if args.is_empty() {
        plan.enter_pending();
        return SlashOutcome::Applied("计划模式".into());
    }
    plan.enter_active();
    SlashOutcome::submit(args, Some("计划模式"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::slash::{slash, Slash};
    use crate::tools::goal::{goal_command, goal_service};

    /// `/goal` 把输入框留给目标（下一条消息当目标标题）；紧接着 `/plan <说明>` 时
    /// 这个等待要撤掉，否则计划说明会被当成目标。终端一直这么做，网关以前没有。
    #[tokio::test]
    async fn plan_takes_the_composer_back_from_goal() {
        let root = Context::new();
        for p in [
            crate::session::log::sessions(),
            slash(),
            goal_service(),
            super::super::plan_mode_service(),
            goal_command(),
            plan_commands(),
        ] {
            root.plugin(p, ()).unwrap().wait().await.unwrap();
        }
        let slash = root.get::<Slash>(SLASH).unwrap();
        let goal = root.get::<Goal>(GOAL).unwrap();
        slash.run(&root, "goal", "").await;
        assert!(goal.awaiting_composer());
        assert_eq!(
            slash.run(&root, "plan", "写个计划").await,
            Some(SlashOutcome::submit("写个计划", Some("计划模式")))
        );
        assert!(!goal.awaiting_composer(), "/plan 之后输入框不该还留给目标");
    }
}
