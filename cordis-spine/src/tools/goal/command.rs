//! 人用的 `/goal`：开始、查看、暂停 / 恢复 / 清除这一页的目标。

use cordis::{plugin, Context, Inject, Plugin};

use super::{goal_composer_fill, Goal, GOAL_RESERVED_SUBCOMMANDS};
use crate::host::slash::{register_commands, slash_handler, SlashCommand, SlashOutcome};
use crate::names::{GOAL, SLASH};

pub fn goal_command() -> Plugin {
    plugin("command-goal", Inject::from([SLASH]), |ctx, _: &()| {
        register_commands(
            ctx,
            vec![
                SlashCommand::host("goal", "开始或查看目标", slash_handler(run))
                    .takes_args(true)
                    .hint("goal pause", "暂停自动推进", false)
                    .hint("goal resume", "恢复当前目标", false)
                    .hint("goal clear", "清除当前目标", false)
                    .hint("goal edit", "在面板里修改当前目标", true),
            ],
        )?;
        Ok(None)
    })
}

fn run(page: &Context, args: &str) -> SlashOutcome {
    let goal = page.get::<Goal>(GOAL);
    if args.is_empty() {
        if let Some(goal) = &goal {
            goal.arm_composer();
        }
        return SlashOutcome::Fill(goal_composer_fill());
    }
    if args == "<目标>" {
        return SlashOutcome::Fill(goal_composer_fill());
    }
    let first = args.split_whitespace().next().unwrap_or("");
    if matches!(first, "status" | "edit") {
        return SlashOutcome::Menu("goal".into());
    }
    let Some(goal) = goal else {
        return SlashOutcome::notice("目标", "目标服务未挂载。");
    };
    match first {
        "pause" => {
            goal.disarm_composer();
            if goal.pause() {
                SlashOutcome::Applied("目标已暂停".into())
            } else {
                SlashOutcome::Applied("没有进行中的目标".into())
            }
        }
        "resume" => {
            goal.disarm_composer();
            if goal.resume() {
                SlashOutcome::Applied("目标已继续".into())
            } else {
                SlashOutcome::Applied("没有已暂停的目标".into())
            }
        }
        "clear" => {
            if goal.present() {
                goal.clear();
                SlashOutcome::Applied("已清除目标".into())
            } else {
                goal.disarm_composer();
                SlashOutcome::Applied("没有活动目标".into())
            }
        }
        _ if GOAL_RESERVED_SUBCOMMANDS.contains(&first) => SlashOutcome::Menu("goal".into()),
        _ => start_goal(page, args),
    }
}

/// 把 `objective` 原样定为这一页的目标并发出去，不再按子命令解析——终端在 `/goal`
/// 之后把下一条消息当目标时走这里（「pause 部署」是目标，不是 `/goal pause`）。
pub fn start_goal(page: &Context, objective: &str) -> SlashOutcome {
    let Some(goal) = page.get::<Goal>(GOAL) else {
        return SlashOutcome::notice("目标", "目标服务未挂载。");
    };
    goal.disarm_composer();
    goal.start(objective);
    SlashOutcome::submit(objective, Some("目标模式"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::slash::{slash, Slash};
    use crate::tools::goal::{goal_composer_fill, goal_service};

    /// 空参数的 `/goal`：把用法填进输入框，并让下一条消息当目标（终端和网关一样）。
    #[tokio::test]
    async fn bare_goal_fills_usage_and_arms_the_composer() {
        let root = Context::new();
        for p in [slash(), goal_service(), goal_command()] {
            root.plugin(p, ()).unwrap().wait().await.unwrap();
        }
        let slash = root.get::<Slash>(SLASH).unwrap();
        let goal = root.get::<Goal>(GOAL).unwrap();
        assert!(!goal.awaiting_composer());
        let out = slash.run(&root, "goal", "").await;
        assert_eq!(out, Some(SlashOutcome::Fill(goal_composer_fill())));
        assert!(goal_composer_fill().contains("/goal"));
        assert!(goal.awaiting_composer(), "下一条消息该当目标");
    }
}
