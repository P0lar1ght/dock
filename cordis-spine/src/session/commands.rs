//! 会话本身的斜杠命令：`/new` `/resume` `/usage` `/context`。
//!
//! 命令体收到的是调用方那一页的 ctx，在里面 live-lookup。换掉整份实时日志之后
//! 不用通知谁：`Sessions` 自己发 `session/reset`，投影这一页的宿主跟着重建。

use cordis::{plugin, Context, Inject, Plugin};
use cordis_base::usage::session_usage_block_text;

use crate::host::slash::{register_commands, slash_handler, SlashCommand, SlashOutcome};
use crate::names::{AGENT_PRESETS, GOAL, SESSIONS, SLASH, SUBAGENTS};
use crate::session::log::Sessions;
use crate::session::resume_preset::{apply_restored_preset, ApplyRestoredPreset};
use crate::{AgentPresets, Goal, Subagents};

pub fn session_commands() -> Plugin {
    plugin("command-session", Inject::from([SLASH]), |ctx, _: &()| {
        register_commands(
            ctx,
            vec![
                SlashCommand::host("new", "开始新会话", slash_handler(|page, _| new(page))),
                SlashCommand::host(
                    "resume",
                    "恢复上次会话",
                    slash_handler(|page, _| resume(page)),
                ),
                SlashCommand::host(
                    "usage",
                    "查看本会话用量（Tab 切到占用）",
                    slash_handler(|page, _| usage(page)),
                )
                .aliases(&["cost"]),
                SlashCommand::host(
                    "context",
                    "查看上下文占用",
                    slash_handler(|_, _| SlashOutcome::Menu("context".into())),
                ),
            ],
        )?;
        Ok(None)
    })
}

fn no_sessions() -> SlashOutcome {
    SlashOutcome::notice("会话", "会话服务未挂载。")
}

fn new(page: &Context) -> SlashOutcome {
    let Some(sessions) = page.get::<Sessions>(SESSIONS) else {
        return no_sessions();
    };
    // 旧会话还在跑的子代理一起收掉，再放开派生。只收这一页的：`"subagents"` 是全局
    // 一份、按父会话分账，`cancel_all` 会波及别的分页。
    if let Some(sub) = page.get::<Subagents>(SUBAGENTS) {
        sub.cancel_session(sessions.identity());
        sub.open_admission_for(sessions.identity());
    }
    sessions.archive_current();
    sessions.clear();
    crate::clear_plan_for_session_switch(page);
    if let Some(goal) = page.get::<Goal>(GOAL) {
        goal.clear();
    }
    SlashOutcome::Applied("已开始新会话".into())
}

fn resume(page: &Context) -> SlashOutcome {
    let Some(sessions) = page.get::<Sessions>(SESSIONS) else {
        return no_sessions();
    };
    let Some(item) = sessions.archived().into_iter().next() else {
        return SlashOutcome::notice("恢复会话", "没有可恢复的会话。");
    };
    let stamped = item.preset_id.clone();
    sessions.archive_current();
    if !sessions.restore(&item.id) {
        return SlashOutcome::notice("恢复会话", "没有可恢复的会话。");
    }
    crate::clear_plan_for_session_switch(page);
    if let Some(presets) = page.get::<AgentPresets>(AGENT_PRESETS) {
        if let ApplyRestoredPreset::Failed { id, error } =
            apply_restored_preset(&presets, stamped.as_deref())
        {
            eprintln!("dock: /resume preset `{id}` not applied: {error}");
        }
    }
    SlashOutcome::Applied("已恢复上次会话".into())
}

fn usage(page: &Context) -> SlashOutcome {
    let Some(sessions) = page.get::<Sessions>(SESSIONS) else {
        return no_sessions();
    };
    SlashOutcome::notice(
        "本会话用量",
        session_usage_block_text(&sessions.prompt_usage()),
    )
}
