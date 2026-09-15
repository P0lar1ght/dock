//! 一页的组合根：分页子树里挂哪些插件由这里决定。
//!
//! TUI 只管开 / 关 / 切页，它不认识 spine 与 app 的插件树，所以建页工厂从这里
//! 注入进 `"tui.tabs"`。第 1 页就是根上下文本身（`main` 里那一串），这里造的是
//! 第 2 页起的每一页。

use std::sync::Arc;

use cordis::{plugin, plugin_async, Inject, Plugin};
use cordis_spine::{
    agent_loop, turn, AgentPresets, Sessions, SubagentDef, AGENT_PRESETS, SESSIONS,
};
use cordis_tui::{prompt, scrollback, status_bar, welcome, TabKind, TabMount};

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
/// 其余一律落回根：一张 `"tools"` 表、一个 `llm`、一套 `permissions` / `mcp` /
/// `browser` / `computer` / `jobs`。两页会真的抢这些全局单例，这是分页的已知
/// 代价，不是疏漏。
fn tab(index: usize, kind: TabKind) -> Plugin {
    plugin_async("tab", Inject::new(), move |ctx, _: &()| async move {
        // 顺序照 main：会话与轮次先落地，循环和 actor 都 inject 它们。
        ctx.plugin(tab_sessions(index), ())?.wait().await?;
        if kind == TabKind::Aside {
            // 旁问页自带一份只读预设；常驻页照旧用根上那份。
            ctx.plugin(aside_presets(), ())?.wait().await?;
        }
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

/// 旁问页的预设：isolate 过 `agentPresets`，所以必须自己 provide 一份。
fn aside_presets() -> Plugin {
    plugin("tab.asidePresets", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(AGENT_PRESETS, aside_preset())?))
    })
}

/// 分页会话：身份是 `main#<index>`，与主会话同级（不是子代理）。
/// 不 `attach_disk` —— 分页目前是内存态，退出即丢（落盘要改会话文件布局）。
fn tab_sessions(index: usize) -> Plugin {
    plugin("tab.sessions", Inject::new(), move |ctx, _: &()| {
        Ok(Some(
            ctx.provide(SESSIONS, Sessions::tab(ctx.clone(), index))?,
        ))
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
