//! 分页（`cordis_spine::Tabs`）落到终端上的部分。

use cordis_spine::Tabs;

use crate::names::{TUI_PROMPT, TUI_SCROLLBACK, TUI_STATUS, TUI_WELCOME};
use crate::views::prompt::PromptWidget;

/// TUI 每页各一份的视图。组合根经 `TabsConfig::per_tab` 交给分页服务，
/// 让第 2 页起的滚动区、输入框、底栏、欢迎页各是各的。
pub const PER_TAB_VIEWS: &[&str] = &[TUI_SCROLLBACK, TUI_PROMPT, TUI_STATUS, TUI_WELCOME];

/// 把当前页最近一条模型回复填进来源页的输入框并切过去，返回来源页编号。
///
/// **不**替用户按发送：发不发、怎么改都还是用户说了算。
pub fn carry_back(tabs: &Tabs) -> Result<usize, String> {
    let carried = tabs.carry_back()?;
    let origin = tabs
        .contexts()
        .into_iter()
        .nth(carried.origin_index)
        .ok_or_else(|| format!("来源页（第 {} 页）已经关掉了", carried.origin_id))?;
    let Some(prompt) = origin.get::<PromptWidget>(TUI_PROMPT) else {
        return Err("来源页没有输入框".into());
    };
    prompt.insert_str(&carried.text);
    tabs.activate(carried.origin_index);
    Ok(carried.origin_id)
}
