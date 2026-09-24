//! 一页的组合根：分页子树里挂哪些插件由这里决定。
//!
//! TUI 只管开 / 关 / 切页，它不认识 spine 与 app 的插件树，所以建页工厂从这里
//! 注入进 `"tui.tabs"`。第 1 页就是根上下文本身（`main` 里那一串），这里造的是
//! 第 2 页起的每一页。

use std::sync::Arc;

use cordis::{plugin, plugin_async, Inject, Plugin};
use cordis_spine::{
    agent_loop, goal_service, plan_mode_service, todo_service, turn, AgentPresets, AppSettings,
    Ask, Permissions, Sessions, SubagentDef, AGENT_PRESETS, ASK, PERMISSIONS, SESSIONS, SETTINGS,
};
use cordis_tui::{prompt, scrollback, status_bar, welcome, TabKind, TabMount, Tabs, TUI_TABS};

use crate::session_actor;

/// 建页工厂，交给 `cordis_tui::tabs()` 当配置。
pub fn tab_mount() -> TabMount {
    Arc::new(tab)
}

/// 旁问页的只读预设。
///
/// **不给写工具**：旁问和主线并发跑、共用同一个工作目录，主线正在改文件时
/// 让旁问也能写就是在制造竞态。也**不给** `search_tool` / `use_tool` —— MCP
/// 那边有 cua-driver 这种能点桌面的工具，插一嘴不该有这个本事。
fn aside_preset() -> AgentPresets {
    let def = SubagentDef {
        name: "旁问".into(),
        description: "只读地回答用户的插话".into(),
        persona: "你在旁路回答用户的一句插话。你看到的是主线对话的快照。\n                  只回答问题本身，简短、直给；不要接管主线的任务，不要改任何文件，\n                  也不要假设你的回答会进入主线上下文。"
            .into(),
        tools: Some(
            ["read_file", "grep", "list_dir", "glob"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        ),
        ..Default::default()
    };
    AgentPresets::overlay(def.to_preset("aside"))
}

/// 一页 = 自己的会话 + 轮次 + agent 循环 + 会话 actor + 四个视图。
///
/// 其余落回根：一张 `"tools"` 表、一个 `llm`、MCP 连接、`browser` / `computer` /
/// `jobs`。模型、协议、权限模式、权限队列和提问队列是这一页自己的。
fn tab(index: usize, kind: TabKind) -> Plugin {
    plugin_async("tab", Inject::new(), move |ctx, _: &()| async move {
        // 顺序照 main：会话与轮次先落地，循环和 actor 都 inject 它们。
        ctx.plugin(tab_sessions(index), ())?.wait().await?;
        // 预设按页（`agentPresets` 在 PER_TAB_SERVICES 里）：旁问页一份只读的，
        // 常驻页从开它的那一页复制一份。
        if kind == TabKind::Aside {
            ctx.plugin(aside_presets(), ())?.wait().await?;
        } else {
            ctx.plugin(tab_presets(), ())?.wait().await?;
        }
        // `GOAL` / `TODOS` 按页隔离（PER_TAB_SERVICES）：每页有自己的目标
        // 与待办服务，第 2 页的 /goal / todo_write 不再写进第 1 页。
        // `update_goal` / `todo_write` 工具仍在全局工具表里注册一份，靠
        // 执行期 ctx 派发到调用者那一页。
        ctx.plugin(goal_service(), ())?.wait().await?;
        ctx.plugin(todo_service(), ())?.wait().await?;
        // `PLAN_MODE` 按页隔离：分页各有自己的计划模式状态与计划文件。
        ctx.plugin(plan_mode_service(), ())?.wait().await?;
        ctx.plugin(tab_settings(), ())?.wait().await?;
        ctx.plugin(tab_permissions(), ())?.wait().await?;
        ctx.plugin(tab_ask(), ())?.wait().await?;
        ctx.plugin(turn(), ())?.wait().await?;
        ctx.plugin(agent_loop(), ())?.wait().await?;
        ctx.plugin(session_actor(), ())?.wait().await?;
        // 视图：每页各自的滚动区、输入框、状态栏、欢迎屏。
        ctx.plugin(scrollback(), ())?.wait().await?;
        ctx.plugin(prompt(), ())?.wait().await?;
        ctx.plugin(status_bar(), ())?.wait().await?;
        ctx.plugin(welcome(), ())?.wait().await?;
        Ok(None)
    })
}

/// 开这一页时正在看的那一页（新页从它继承设置、cwd、预设）。
fn active_page(ctx: &cordis::Context) -> Option<cordis::Context> {
    ctx.get::<Tabs>(TUI_TABS).map(|tabs| tabs.active_ctx())
}

/// 抄当前页的模型、协议、权限模式。开页之后两页各改各的。
fn tab_settings() -> Plugin {
    plugin("tab.settings", Inject::new(), |ctx, _: &()| {
        let forked = active_page(ctx)
            .and_then(|page| page.get::<AppSettings>(SETTINGS))
            .map(|settings| settings.fork())
            .unwrap_or_else(|| AppSettings::new(""));
        Ok(Some(ctx.provide(SETTINGS, forked)?))
    })
}

fn tab_permissions() -> Plugin {
    plugin("tab.permissions", Inject::new(), |ctx, _: &()| {
        Ok(Some(
            ctx.provide(PERMISSIONS, Permissions::new(ctx.clone()))?,
        ))
    })
}

fn tab_ask() -> Plugin {
    plugin("tab.ask", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(ASK, Ask::new(ctx.clone()))?))
    })
}

/// 常驻页的预设：复制开它的那一页的（同样的层、同样的当前预设），项目层指向
/// 这一页的 cwd。之后两页各切各的。
///
/// 来源页没挂预设（精简装配）时这页也不挂——和预设的 fail-open 一致：没有
/// `agentPresets` 的会话不设允许名单，跟以前落回根时一样。
fn tab_presets() -> Plugin {
    plugin("tab.presets", Inject::new(), |ctx, _: &()| {
        let presets = active_page(ctx)
            .and_then(|page| page.get::<AgentPresets>(AGENT_PRESETS))
            .map(|presets| presets.fork());
        let Some(presets) = presets else {
            return Ok(None);
        };
        if let Some(cwd) = ctx
            .get::<Sessions>(SESSIONS)
            .and_then(|s| s.workspace_cwd())
        {
            presets.set_workspace_root(&cwd);
        }
        Ok(Some(ctx.provide(AGENT_PRESETS, presets)?))
    })
}

/// 旁问页的预设：isolate 过 `agentPresets`，所以必须自己 provide 一份。
fn aside_presets() -> Plugin {
    plugin("tab.asidePresets", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(AGENT_PRESETS, aside_preset())?))
    })
}

/// 分页会话：身份是 `main#<index>`，与主会话同级（不是子代理）。
///
/// 空白页不 `attach_disk`，退出即丢。从历史打开的那一页随后走
/// `Sessions::adopt_archived`，接着写回原来的会话目录，不另起一份布局。
fn tab_sessions(index: usize) -> Plugin {
    plugin("tab.sessions", Inject::new(), move |ctx, _: &()| {
        let sessions = Sessions::tab(ctx.clone(), index);
        // 新页在开它的那一页的目录里起步（那一页 `/cd` 过就跟过去）。没钉的页跟随
        // 进程 cwd，新页也不钉，保持原样。
        if let Some(cwd) = active_page(ctx)
            .and_then(|page| page.get::<Sessions>(SESSIONS))
            .and_then(|s| s.workspace_cwd())
        {
            sessions.pin_plan_cwd(&cwd);
            sessions.pin_workspace_cwd(cwd);
        }
        // `main#N` is reused next process. Drop whatever the last process left.
        cordis_spine::discard_ephemeral_plan(&sessions);
        Ok(Some(ctx.provide(SESSIONS, sessions)?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旁问页必须是只读的：它和主线并发跑、共用同一个工作目录，给了写工具就是
    /// 在制造竞态。这条比任何 persona 措辞都重要，所以钉在这里。
    #[test]
    fn aside_preset_has_no_write_or_mcp_tools() {
        let presets = aside_preset();
        for denied in [
            "bash",
            "write_file",
            "edit_file",
            "apply_patch",
            "task",
            "search_tool",
            "use_tool",
        ] {
            assert!(!presets.allows(denied), "旁问不该有 {denied}");
        }
        for allowed in ["read_file", "grep", "list_dir", "glob"] {
            assert!(presets.allows(allowed), "旁问该能用 {allowed}");
        }
    }
}
