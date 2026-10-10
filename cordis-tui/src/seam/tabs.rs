//! 分页（`cordis_spine::Tabs`）落到终端上的部分。

use cordis::Context;
use cordis_spine::{SessionRef, Tabs, SESSION_PORT, SIDE_NOTE_MAX_CHARS};

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

/// `/tab merge` 的两头：当前页（必须是旁问页）、它的来源页、来源页的号。
pub fn merge_target(tabs: &Tabs) -> Result<(Context, Context, usize), String> {
    tabs.active_aside_origin()?
        .ok_or_else(|| "/tab merge 只在旁问页（/btw 开的页）里用".to_string())
}

/// 把（用户看过、改过的）笔记写进来源页的上下文，返回来源页的号。来源页在跑就
/// 下一个步骤边界并入，闲着直接落进历史；不开新的一轮。
pub fn merge_note(tabs: &Tabs, note: &str) -> Result<usize, String> {
    let (_, origin, origin_id) = merge_target(tabs)?;
    let note = note.trim();
    if note.chars().count() > SIDE_NOTE_MAX_CHARS {
        return Err(format!("笔记最多 {SIDE_NOTE_MAX_CHARS} 字，删短一点再写"));
    }
    let port = origin
        .get::<SessionRef>(SESSION_PORT)
        .ok_or_else(|| "来源页没有会话入口".to_string())?;
    port.merge_side_note(note.to_string());
    Ok(origin_id)
}

/// 起草好的笔记填进旁问页的输入框，成一条 `/tab merge <笔记>`：改完回车才写。
/// 输入框里已经有东西（起草期间用户打了字）就不覆盖。
pub fn fill_merge_prompt(aside: &Context, note: &str) -> Result<(), String> {
    let prompt = aside
        .get::<PromptWidget>(TUI_PROMPT)
        .ok_or_else(|| "旁问页没有输入框".to_string())?;
    if !prompt.text().trim().is_empty() {
        return Err("结论起草好了，但输入框里有内容没覆盖：清空后再 /tab merge".into());
    }
    prompt.insert_str(&format!("/tab merge {note}"));
    Ok(())
}
