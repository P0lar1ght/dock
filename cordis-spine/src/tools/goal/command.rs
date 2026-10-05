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
                SlashOutcome::notice("目标", "没有进行中的目标。")
            }
        }
        "resume" => {
            goal.disarm_composer();
            if goal.resume() {
                SlashOutcome::Applied("目标已继续".into())
            } else {
                SlashOutcome::notice("目标", "没有已暂停的目标。")
            }
        }
        "clear" => {
            if goal.present() {
                goal.clear();
                SlashOutcome::Applied("已清除目标".into())
            } else {
                goal.disarm_composer();
                SlashOutcome::notice("目标", "没有活动目标。")
            }
        }
        _ if GOAL_RESERVED_SUBCOMMANDS.contains(&first) => SlashOutcome::Menu("goal".into()),
        _ => {
            goal.disarm_composer();
            goal.start(args);
            SlashOutcome::Submit(args.to_string())
        }
    }
}
