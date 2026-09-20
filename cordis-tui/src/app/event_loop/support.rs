//! Overlay openers, MCP/goal helpers, and small loop utilities.

use std::collections::{HashMap, HashSet};
use std::time::{Instant, SystemTime};

use cordis::Context;
use cordis_spine::{
    goal_composer_fill, loop_composer_fill, loop_schedule_instruction, AgentPresets, AppSettings,
    Ask, Browser, Computer, Cron, CuaAction, Goal, Jobs, LoopFireMode, Mcp, McpStatus,
    MermaidEngineKind, Permissions, PlanMode, Sessions, Slash, SlotKeyResult, Subagents, TuiSlots,
    UserImage, Workflows, AGENT_PRESETS, ASK, BROWSER, COMPUTER, CRON, GOAL, JOBS, MCP,
    PERMISSIONS, PLAN_MODE, SESSIONS, SETTINGS, SLASH, SUBAGENTS, TUI_SLOTS, WORKFLOWS,
};

use crate::app::clipboard;
use crate::error::Result;
use crate::grok::mcps;
use crate::grok::tasks_pane::{self, GroupKind, KillTarget, TaskEntry};
use crate::grok::workflows::{WorkflowAgentRowView, WorkflowRunSnapshot};
use crate::names::{
    GATEWAY, SESSION_PORT, TUI_PROMPT, TUI_SCROLLBACK, TUI_STATUS, TUI_TABS, TUI_WELCOME,
};
use crate::scrollback::Scrollback;
use crate::seam::gateway::GatewayRef;
use crate::seam::session::SessionRef;
use crate::seam::tabs::Tabs;
use crate::slash::{self, filter_args};
use crate::views::ask_view;
use crate::views::mcp_elicit_view;
use crate::views::overlay::{
    filter_help_items, filter_sessions, filter_strings, InspectTarget, Overlay,
};
use crate::views::pairing;
use crate::views::permission_view;
use crate::views::plan_approval_view;
use crate::views::preset_overlay::{self, PresetAction, PresetView};
use crate::views::settings_modal;
use crate::views::task_dock;
use crate::views::text_overlay;
use crate::views::usage_overlay;

use crate::app::actions::{
    interpret_goal_composer_ex, interpret_loop_composer, Effect, GoalComposer, LoopComposer,
};
use crate::grok::mermaid::AffordanceKind;
use crate::media::mermaid_png;
use crate::views::goal_overlay;
use crate::views::goal_pane::GoalHit;
use crate::views::inspect_overlay;
use crate::views::prompt::{PastedImage, PromptWidget};
use crate::views::status::StatusLine;
use crate::views::welcome::Welcome;

pub(super) fn open_pairing_if_needed(ctx: &Context, overlay: &mut Overlay) {
    let front = ctx
        .get::<GatewayRef>(GATEWAY)
        .and_then(|g| g.pairing_front());
    if matches!(overlay, Overlay::PairingPending { .. }) {
        if front.is_none() {
            overlay.close();
        }
        return;
    }
    if matches!(
        overlay,
        Overlay::Permission { .. }
            | Overlay::Ask { .. }
            | Overlay::PlanApproval { .. }
            | Overlay::PairingManage { .. }
    ) {
        return;
    }
    if front.is_some() {
        *overlay = Overlay::PairingPending { selected: 0 };
    }
}

pub(super) fn open_permission_if_needed(ctx: &Context, overlay: &mut Overlay) {
    if matches!(
        overlay,
        Overlay::PairingPending { .. } | Overlay::PairingManage { .. }
    ) {
        return;
    }
    // 先确认真有待授权项，再决定要不要抢前台。
    //
    // 反过来写（看见 elicit 就先 close 再看有没有东西要显示）会在没有待授权项时
    // 把正在填的 elicit 表单每轮拆掉重建 —— 它的 selected / picked / draft 活在
    // overlay 上，重建就等于全部打回默认。
    if ctx
        .get::<Permissions>(PERMISSIONS)
        .and_then(|p| p.front())
        .is_none()
    {
        return;
    }
    // 授权门优先级高于 elicit，可以抢；其余已开的 overlay 让位给它们。
    if overlay.is_open() && !matches!(overlay, Overlay::Elicit { .. }) {
        return;
    }
    *overlay = Overlay::Permission { selected: 0 };
}

pub(super) fn open_slot_if_needed(ctx: &Context, overlay: &mut Overlay) {
    if overlay.is_open() {
        return;
    }
    let Some(slots) = ctx.get::<TuiSlots>(TUI_SLOTS) else {
        return;
    };
    if let Some(id) = slots.take_open_request() {
        if slots.get(&id).is_some() {
            *overlay = Overlay::Slot { id, scroll: 0 };
        }
    }
}

pub(super) fn open_slash_notice_if_needed(ctx: &Context, overlay: &mut Overlay) {
    if overlay.is_open() {
        return;
    }
    let Some(slash) = ctx.get::<Slash>(SLASH) else {
        return;
    };
    if let Some((title, body)) = slash.take_notice() {
        *overlay = Overlay::Notice {
            title,
            body,
            scroll: 0,
        };
    }
}

pub(super) fn dispatch_slot_key(ctx: &Context, overlay: &mut Overlay, key: &str) -> bool {
    let Overlay::Slot { id, .. } = overlay else {
        return false;
    };
    let id = id.clone();
    let Some(slots) = ctx.get::<TuiSlots>(TUI_SLOTS) else {
        overlay.close();
        return true;
    };
    if matches!(slots.on_key(&id, key), SlotKeyResult::Close) {
        overlay.close();
    }
    true
}

pub(super) fn browser_cockpit_body(ctx: &Context) -> String {
    let approval = ctx
        .get::<Permissions>(PERMISSIONS)
        .and_then(|p| p.front())
        .filter(|pr| pr.tool.starts_with("browser_"))
        .map(|pr| {
            if pr.summary.is_empty() {
                pr.tool
            } else {
                format!("{} — {}", pr.tool, pr.summary)
            }
        });
    ctx.get::<Browser>(BROWSER)
        .map(|b| b.format_cockpit(approval.as_deref()))
        .unwrap_or_else(|| "browser 未挂载（tool-browser 插件不在树上）。".into())
}

pub(super) fn computer_cockpit_body(ctx: &Context, pending: Option<CuaAction>) -> String {
    let approval = ctx
        .get::<Permissions>(PERMISSIONS)
        .and_then(|p| p.front())
        .filter(|pr| pr.tool.starts_with("mcp_cua-driver__"))
        .map(|pr| {
            if pr.summary.is_empty() {
                pr.tool.clone()
            } else {
                format!("{} — {}", pr.tool, pr.summary)
            }
        });
    ctx.get::<Computer>(COMPUTER)
        .map(|c| c.format_cockpit_with(approval.as_deref(), pending))
        .unwrap_or_else(|| "computer 未挂载（tool-computer 插件不在树上）。".into())
}

/// 重新探测本机 cua-driver。`quiet` 是开 overlay 时那次，不 flash。
pub(super) fn spawn_cua_refresh(
    ctx: Context,
    redraw: tokio::sync::mpsc::UnboundedSender<()>,
    quiet: bool,
) {
    let Some(computer) = ctx.get::<Computer>(COMPUTER) else {
        if !quiet {
            flash(&ctx, "computer 未挂载（tool-computer 插件不在树上）。");
        }
        return;
    };
    computer.refresh();
    if !quiet {
        flash(&ctx, "正在重新检测 cua-driver…");
    }
    let _ = redraw.send(());
}

/// 执行确认过的安装 / 授权。真正的下载、子进程、MCP 重载都在 `"computer"` 里，
/// 这里只负责起头、给一条 flash，并让 UI 继续重绘。
pub(super) fn spawn_cua_run(
    ctx: Context,
    redraw: tokio::sync::mpsc::UnboundedSender<()>,
    action: CuaAction,
) {
    let Some(computer) = ctx.get::<Computer>(COMPUTER) else {
        flash(&ctx, "computer 未挂载（tool-computer 插件不在树上）。");
        return;
    };
    match computer.start(action) {
        Ok(()) => flash(&ctx, format!("{}…（进度见 /computer）", action.title())),
        Err(e) => flash(&ctx, e),
    }
    let _ = redraw.send(());
}

pub(super) fn open_ask_if_needed(ctx: &Context, overlay: &mut Overlay) {
    if matches!(
        overlay,
        Overlay::Permission { .. }
            | Overlay::Ask { .. }
            | Overlay::PlanApproval { .. }
            | Overlay::PairingPending { .. }
    ) {
        if matches!(overlay, Overlay::Ask { .. })
            && ctx.get::<Ask>(ASK).and_then(|a| a.front()).is_none()
        {
            overlay.close();
        }
        return;
    }
    // 同 `open_permission_if_needed`：先确认真有待回答的提问再抢前台，否则会把
    // 正在填的 elicit 表单每轮拆掉重建。
    let Some(front) = ctx.get::<Ask>(ASK).and_then(|a| a.front()) else {
        return;
    };
    if overlay.is_open() && !matches!(overlay, Overlay::Elicit { .. }) {
        return;
    }
    *overlay = ask_overlay_from_prompt(&front);
}

pub(super) fn navigate_ask(ctx: &Context, overlay: &mut Overlay, delta: i16) {
    if let Overlay::Ask {
        selected, picked, ..
    } = overlay
    {
        // Other freeform owns ←/→ for the caret; question nav only when not typing Other.
        if ask_other_active(ctx, *selected, picked) {
            return;
        }
    }
    let Some(ask) = ctx.get::<Ask>(ASK) else {
        return;
    };
    if ask.navigate(delta as i32) {
        let Some(front) = ask.front() else {
            overlay.close();
            return;
        };
        *overlay = ask_overlay_from_prompt(&front);
        return;
    }
    // 往前走不动 = 当前就是第一道还没答的题（`Ask::navigate` 按 `max_reachable`
    // 夹住了下标）。抬头明写着「← → 切换」，停在这儿按 → 一点反应都没有说不过去：
    // 把当前高亮当答案记下来再前进，和 Enter 同一条路。
    if delta <= 0 {
        return;
    }
    let Some(front) = ask.front() else {
        return;
    };
    // 只在**后面还有题**时这么做，别让一个导航键顺手把整个提问提交掉。
    // 导航被 `max_reachable` 夹着，所以第一道未答题之后的题必然也没答过，
    // 记下这一题一定落到下一题上，不会走到收尾那条分支。
    if front.index != front.max_index || front.index + 1 >= front.questions.len() {
        return;
    }
    let answer = match overlay {
        Overlay::Ask {
            selected,
            picked,
            draft,
            ..
        } => Some((*selected, picked.clone(), draft.clone())),
        _ => None,
    };
    if let Some((selected, picked, draft)) = answer {
        accept_ask(ctx, overlay, selected, picked, &draft);
    }
}

fn ask_overlay_from_prompt(front: &cordis_spine::AskPrompt) -> Overlay {
    let Some(q) = front.questions.get(front.index) else {
        return Overlay::Ask {
            selected: 0,
            picked: Vec::new(),
            draft: String::new(),
            draft_cursor: 0,
            draft_focused: false,
        };
    };
    let labs = ask_view::labels(q);
    let multi = q.multi_select.unwrap_or(false);
    let mut picked = vec![false; labs.len()];
    let mut selected = 0usize;
    for wire in &front.current_labels {
        if let Some(i) = labs
            .iter()
            .position(|l| ask_view::wire_label(l) == *wire || l == wire)
        {
            if multi {
                picked[i] = true;
            }
            selected = i;
        }
    }
    let draft = front.current_notes.clone().unwrap_or_default();
    if !draft.is_empty() {
        if let Some(i) = labs.iter().position(|l| ask_view::is_other_label(l)) {
            selected = i;
            if multi {
                picked[i] = true;
            }
        }
    }
    let draft_cursor = draft.chars().count();
    // 焦点要跟 `selected` 自洽：上面刚把高亮挪到「其他」（带着上次填的内容）的话，
    // 这里再写死 false，回到这题就成了死状态——打字、退格、←→ 全被 `draft_focused`
    // 挡掉，而 ←→ 的问题切换又被 `other_active` 挡掉，只能上下跳一次才救得回来。
    let draft_focused = labs
        .get(selected)
        .is_some_and(|l| ask_view::is_other_label(l));
    Overlay::Ask {
        selected,
        picked,
        draft,
        draft_cursor,
        draft_focused,
    }
}

/// `(「其他」在选项里的下标, 这题是否多选)`。
pub(super) fn ask_other_index_and_mode(ctx: &Context) -> Option<(usize, bool)> {
    let front = ctx.get::<Ask>(ASK).and_then(|a| a.front())?;
    let q = front.questions.get(front.index)?;
    let other = ask_view::labels(q)
        .iter()
        .position(|l| ask_view::is_other_label(l))?;
    Some((other, q.multi_select.unwrap_or(false)))
}

/// True when the highlighted Ask option is the Other freeform choice.
pub(super) fn ask_selected_is_other(ctx: &Context, selected: usize) -> bool {
    ctx.get::<Ask>(ASK)
        .and_then(|a| a.front())
        .and_then(|p| {
            p.questions.get(p.index).map(|q| {
                ask_view::labels(q)
                    .get(selected)
                    .is_some_and(|l| ask_view::is_other_label(l))
            })
        })
        .unwrap_or(false)
}

/// Keep Other draft focus in sync after keyboard selection changes.
pub(super) fn sync_ask_draft_focus(ctx: &Context, overlay: &mut Overlay) {
    if let Overlay::Ask {
        selected,
        draft_focused,
        ..
    } = overlay
    {
        *draft_focused = ask_selected_is_other(ctx, *selected);
    }
}

pub(super) fn elicit_front(ctx: &Context) -> Option<cordis_spine::ElicitPrompt> {
    ctx.get::<Mcp>(MCP).and_then(|m| m.elicitation().front())
}

pub(super) fn elicit_needs_draft(ctx: &Context, selected: usize, picked: &[bool]) -> bool {
    elicit_front(ctx).is_some_and(|p| mcp_elicit_view::needs_draft(&p, selected, picked))
}

pub(super) fn open_elicit_if_needed(ctx: &Context, overlay: &mut Overlay) {
    if matches!(
        overlay,
        Overlay::Permission { .. }
            | Overlay::Ask { .. }
            | Overlay::PlanApproval { .. }
            | Overlay::PairingPending { .. }
            | Overlay::Elicit { .. }
    ) {
        if matches!(overlay, Overlay::Elicit { .. }) && elicit_front(ctx).is_none() {
            overlay.close();
        }
        return;
    }
    if overlay.is_open() {
        return;
    }
    let Some(front) = elicit_front(ctx) else {
        return;
    };
    *overlay = Overlay::Elicit {
        selected: front.selected,
        picked: if front.picked.is_empty() {
            vec![false; front.options.len()]
        } else {
            front.picked
        },
        draft: front.draft,
    };
}

pub(super) fn open_plan_approval_if_needed(ctx: &Context, overlay: &mut Overlay) {
    if matches!(
        overlay,
        Overlay::Permission { .. }
            | Overlay::Ask { .. }
            | Overlay::PlanApproval { .. }
            | Overlay::PairingPending { .. }
    ) {
        if matches!(
            overlay,
            Overlay::PlanApproval {
                view_only: false,
                ..
            }
        ) && ctx
            .get::<PlanMode>(PLAN_MODE)
            .and_then(|p| p.front())
            .is_none()
        {
            overlay.close();
        }
        return;
    }
    if overlay.is_open() {
        return;
    }
    if let Some(prompt) = ctx.get::<PlanMode>(PLAN_MODE).and_then(|p| p.front()) {
        *overlay = Overlay::plan_approval(prompt, false);
    }
}

pub(super) fn open_view_plan(ctx: &Context, overlay: &mut Overlay) {
    let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) else {
        flash(ctx, "计划模式未挂载");
        return;
    };
    if let Some(prompt) = plan.front() {
        *overlay = Overlay::plan_approval(prompt, false);
        return;
    }
    if let Some(prompt) = plan.disk_preview() {
        *overlay = Overlay::plan_approval(prompt, true);
        return;
    }
    flash(ctx, "没有可查看的计划");
}

pub(super) fn scroll_plan_body(
    scroll: &mut usize,
    prompt: &cordis_spine::PlanApprovalPrompt,
    wrap: &plan_approval_view::PlanWrapCache,
    delta: i16,
) {
    let max = wrap.max_scroll(prompt, 72, 16);
    let next = (*scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
    *scroll = next;
}

pub(super) fn scroll_usage(
    tab: crate::views::overlay::UsageTab,
    detail: Option<cordis_spine::OccupancyKind>,
    scroll: &mut usize,
    delta: i16,
) {
    let max = usage_overlay::max_scroll(tab, detail);
    let next = (*scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
    *scroll = next;
}

pub(super) fn scroll_text(scroll: &mut usize, delta: i16, body: &str) {
    let max = text_overlay::max_scroll(body, 16);
    let next = (*scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
    *scroll = next;
}

pub(super) fn scroll_inspect(
    ctx: &Context,
    target: &crate::views::overlay::InspectTarget,
    scroll: &mut usize,
    delta: i16,
) {
    let width = inspect_overlay::body_width();
    let viewport = inspect_overlay::body_height();
    let max = inspect_overlay::max_scroll(ctx, target, width, viewport);
    let next = (*scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
    *scroll = next;
}

pub(super) fn close_inspect(overlay: &mut Overlay) -> Vec<Effect> {
    if let Overlay::Inspect {
        from_tasks: true, ..
    } = overlay
    {
        *overlay = Overlay::Tasks {
            selected: 0,
            query: String::new(),
            collapsed: HashSet::new(),
        };
        return Vec::new();
    }
    overlay.close();
    Vec::new()
}

pub(super) fn send_inspect_composer(ctx: &Context, overlay: &mut Overlay) {
    let Overlay::Inspect {
        target: InspectTarget::Subagent(id),
        composer,
        composer_cursor,
        ..
    } = overlay
    else {
        return;
    };
    let text = composer.trim().to_string();
    if text.is_empty() {
        return;
    }
    composer.clear();
    *composer_cursor = 0;
    let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
        flash(ctx, "子代理服务未挂载");
        return;
    };
    match sub.send_message(id, &text, true) {
        Ok(_) => flash(ctx, "已发给子代理"),
        Err(e) => flash(ctx, e),
    }
}

pub(super) fn slash_extras(ctx: &Context) -> Vec<cordis_spine::SlashEntry> {
    ctx.get::<Slash>(SLASH)
        .map(|s| s.list())
        .unwrap_or_default()
}

pub(super) fn ask_other_active(ctx: &Context, selected: usize, picked: &[bool]) -> bool {
    ctx.get::<Ask>(ASK)
        .and_then(|a| a.front())
        .and_then(|p| {
            p.questions.get(p.index).map(|q| {
                let labs = ask_view::labels(q);
                let multi = q.multi_select.unwrap_or(false);
                ask_view::other_active(&labs, selected, picked, multi)
            })
        })
        .unwrap_or(false)
}

pub(super) fn accept_ask(
    ctx: &Context,
    overlay: &mut Overlay,
    selected: usize,
    picked: Vec<bool>,
    draft: &str,
) {
    let Some(ask) = ctx.get::<Ask>(ASK) else {
        overlay.close();
        return;
    };
    let Some(front) = ask.front() else {
        overlay.close();
        return;
    };
    let Some(q) = front.questions.get(front.index) else {
        overlay.close();
        return;
    };
    let labs = ask_view::labels(q);
    let multi = q.multi_select.unwrap_or(false);
    let mut chosen: Vec<String> = if multi {
        labs.iter()
            .enumerate()
            .filter(|(i, _)| picked.get(*i).copied().unwrap_or(false))
            .map(|(_, l)| l.clone())
            .collect()
    } else {
        labs.get(selected).cloned().into_iter().collect()
    };
    if chosen.is_empty() {
        if let Some(one) = labs.get(selected) {
            chosen.push(one.clone());
        }
    }
    let other_chosen = chosen.iter().any(|l| ask_view::is_other_label(l));
    if other_chosen && draft.trim().is_empty() {
        flash(ctx, "请输入具体内容");
        return;
    }
    if !chosen.is_empty() {
        let notes = if other_chosen {
            Some(draft.trim().to_string())
        } else {
            None
        };
        let chosen = chosen
            .into_iter()
            .map(|l| ask_view::wire_label(&l))
            .collect();
        ask.answer_current(chosen, notes);
    }
    if let Some(front) = ask.front() {
        *overlay = ask_overlay_from_prompt(&front);
    } else {
        overlay.close();
    }
}

pub(super) fn accept_elicit(
    ctx: &Context,
    overlay: &mut Overlay,
    selected: usize,
    picked: Vec<bool>,
    draft: &str,
) {
    let Some(elicit) = ctx.get::<Mcp>(MCP).map(|m| m.elicitation()) else {
        overlay.close();
        return;
    };
    let Some(front) = elicit.front() else {
        overlay.close();
        return;
    };
    // 空输入该不该挡由服务侧一处判：`text_value` 对必填空值回「此项必填」，
    // `other_content` 对空的「其他」回「请输入具体内容」，两条都会被下面 flash 出去。
    //
    // 这里原本还自己预检一遍 `needs_draft && draft.is_empty()`，但 `needs_draft`
    // 只看 `typing` / 光标是否停在「其他」上，看不到字段是不是必填 —— 于是
    // **可选**文本字段留空永远提交不了：回车只闪一下，表单走不完，MCP 请求
    // 永远不 resolve，工具调用和整轮对话一起卡死。多选里光标恰好停在「其他」
    // 但没勾它时同理。
    let result = if front.typing {
        elicit.accept_text(draft)
    } else {
        elicit.accept_option(selected, &picked, draft)
    };
    match result {
        Ok(()) => overlay.close(),
        Err(e) => flash(ctx, e),
    }
}

/// 五类耗时任务归一后的清单，主界面任务条与 `/tasks` 共用同一份。
///
/// 走 `list_brief()` 而不是 `list()`：`TaskEntry` 根本不读 `JobSnapshot.output`
/// （`from_bg_task` 只用 description / command / is_monitor / done），而任务条
/// 常驻、每 80ms 重绘，整份拼 20KB 输出纯属白工。摘要要用的最新一行就搭在
/// `output` 上带回来。
pub(crate) fn live_task_entries(ctx: &Context) -> Vec<TaskEntry> {
    // 前台 bash 也挂在 jobs 表上（为了让 TUI 读到实时输出），但它不是后台任务，
    // 不进 tasks pane、也不进任务条。
    let jobs: Vec<_> = ctx
        .get::<Jobs>(JOBS)
        .map(|j| j.list_brief())
        .unwrap_or_default()
        .into_iter()
        .filter(|j| !j.foreground)
        .collect();
    let subagents = ctx
        .get::<Subagents>(SUBAGENTS)
        .map(|s| s.list())
        .unwrap_or_default();
    let cron = ctx.get::<Cron>(CRON).map(|c| c.list()).unwrap_or_default();
    let workflows: Vec<WorkflowRunSnapshot> = ctx
        .get::<Workflows>(WORKFLOWS)
        .map(|w| w.list().into_iter().map(to_tui_workflow).collect())
        .unwrap_or_default();
    let presets = ctx.get::<AgentPresets>(AGENT_PRESETS);
    tasks_pane::collect_items(&jobs, &subagents, &cron, &workflows, presets.as_deref())
}

pub(super) fn task_entries(
    ctx: &Context,
    query: &str,
    collapsed: &HashSet<GroupKind>,
) -> Vec<TaskEntry> {
    let items = live_task_entries(ctx);
    let rows = tasks_pane::rebuild_entries(&items, collapsed);
    tasks_pane::filter_entries(rows, query)
}

/// 任务条的行。未完成的才画；完成的结果本来就会落到 scrollback。
///
/// 尾行摘要先一次性收成一张表再查。按行去 `list_brief()` / `snapshot()` 取的话
/// 每帧是 O(n²) 次克隆，而这条是 80ms 的热路径。
pub(crate) fn task_dock_rows(ctx: &Context) -> Vec<task_dock::Row> {
    let mut tails: HashMap<String, String> = ctx
        .get::<Jobs>(JOBS)
        .map(|j| {
            j.list_brief()
                .into_iter()
                .map(|j| (j.id, j.output))
                .collect()
        })
        .unwrap_or_default();
    // `collect_items` 不过滤 done 子代理（`/tasks` 要列出来），任务条得自己挡。
    // idle 的留着 —— 它还能接 send_message。
    let mut live_agents: HashSet<String> = HashSet::new();
    for snap in ctx
        .get::<Subagents>(SUBAGENTS)
        .map(|s| s.list())
        .unwrap_or_default()
    {
        if snap.done {
            continue;
        }
        tails.insert(snap.id.clone(), task_dock::last_line(&snap.output));
        live_agents.insert(snap.id);
    }
    let now = Instant::now();
    live_task_entries(ctx)
        .into_iter()
        .filter_map(|entry| task_dock_row(entry, &tails, &live_agents, now))
        .collect()
}

fn task_dock_row(
    entry: TaskEntry,
    tails: &HashMap<String, String>,
    live_agents: &HashSet<String>,
    now: Instant,
) -> Option<task_dock::Row> {
    let kill = entry.kill_target();
    match entry {
        TaskEntry::BgTask {
            task_id,
            styled,
            running,
            start_time,
            ..
        } => {
            if !running {
                return None;
            }
            // `list_brief` 把最新一行搭在 output 上带回来。
            let tail = tails.get(&task_id).cloned().unwrap_or_default();
            Some(task_dock::Row {
                hit: task_dock::TaskDockHit::Job(task_id),
                styled,
                running,
                elapsed: SystemTime::now()
                    .duration_since(start_time)
                    .unwrap_or_default(),
                tail,
                kill,
            })
        }
        TaskEntry::Agent {
            subagent_id,
            styled,
            running,
            started_at,
            ..
        } => {
            if !live_agents.contains(&subagent_id) {
                return None;
            }
            let tail = tails.get(&subagent_id).cloned().unwrap_or_default();
            Some(task_dock::Row {
                hit: task_dock::TaskDockHit::Subagent(subagent_id),
                styled,
                running,
                elapsed: now.duration_since(started_at),
                tail,
                kill,
            })
        }
        TaskEntry::Workflow {
            styled,
            running,
            started_at,
            ..
        } => running.then(|| task_dock::Row {
            hit: task_dock::TaskDockHit::OpenTasks,
            styled,
            running,
            elapsed: now.duration_since(started_at),
            tail: String::new(),
            kill,
        }),
        TaskEntry::Scheduled {
            styled, started_at, ..
        } => Some(task_dock::Row {
            hit: task_dock::TaskDockHit::OpenTasks,
            styled,
            running: false,
            elapsed: now.duration_since(started_at),
            tail: String::new(),
            kill,
        }),
        TaskEntry::Header { .. } => None,
    }
}

/// 有没有活着的耗时任务。只数数量，不建 label、不走 `collect_items` ——
/// 这是 80ms 心跳里的判断，不能顺手把整张表算一遍。
pub(super) fn any_live_task(ctx: &Context) -> bool {
    ctx.get::<Jobs>(JOBS).is_some_and(|j| j.live_count() > 0)
        || ctx
            .get::<Subagents>(SUBAGENTS)
            .is_some_and(|s| s.list().iter().any(|a| !a.done))
        || ctx.get::<Cron>(CRON).is_some_and(|c| !c.list().is_empty())
        || ctx
            .get::<Workflows>(WORKFLOWS)
            .is_some_and(|w| w.list().into_iter().any(|r| to_tui_workflow(r).is_active()))
}

pub(super) fn to_tui_workflow(snap: cordis_spine::WorkflowRunSnap) -> WorkflowRunSnapshot {
    WorkflowRunSnapshot {
        run_id: snap.run_id,
        name: snap.name,
        objective: snap.objective,
        status: snap.status,
        management_available: true,
        builtin: snap.builtin,
        phases: snap.phases.as_ref().clone(),
        current_phase: snap.current_phase,
        agents: snap
            .agents
            .iter()
            .map(|row| WorkflowAgentRowView {
                agent_id: row.agent_id.clone(),
                label: row.label.clone(),
                phase: row.phase.clone(),
                // dock 的子代理共享父模型，没有 per-agent 覆盖。
                model: None,
                state: row.state.clone(),
                tokens_used: row.tokens_used,
                duration_ms: row.duration_ms,
                latest_report: row.latest_report.clone(),
            })
            .collect(),
        agent_budget: Some(snap.agent_budget),
        // 预留即扣减，一个池子：`agents_used` 已含在跑的那些，所以 reserved 记 0。
        agents_used: snap.agents_used,
        agents_reserved: 0,
        agents_remaining: Some(snap.agent_budget.saturating_sub(snap.agents_used)),
        agent_usage_incomplete: false,
        active_agents: snap.agents_running,
        elapsed_ms: snap.elapsed_ms,
        received_at: snap.received_at,
        pause_message: snap.pause_message,
        result_summary: snap.result_summary,
        logs: snap.logs,
    }
}

/// 详情页左栏有几行可选。脚本没声明阶段时只有「Agents」那一行。
pub(super) fn workflow_phase_count(ctx: &Context, query: &str, run_id: &str) -> usize {
    workflow_rows(ctx, query)
        .iter()
        .find(|r| r.run_id == run_id)
        .map(|run| crate::grok::workflows::phase_buckets(run).len())
        .unwrap_or(1)
}

pub(super) fn workflow_rows(ctx: &Context, query: &str) -> Vec<WorkflowRunSnapshot> {
    let runs = ctx
        .get::<Workflows>(WORKFLOWS)
        .map(|w| {
            w.list()
                .into_iter()
                .map(to_tui_workflow)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let q = query.to_ascii_lowercase();
    if q.is_empty() {
        return runs;
    }
    runs.into_iter()
        .filter(|r| {
            r.name.to_ascii_lowercase().contains(&q)
                || r.status.to_ascii_lowercase().contains(&q)
                || r.objective.to_ascii_lowercase().contains(&q)
        })
        .collect()
}

pub(super) fn mcp_status_list(ctx: &Context) -> Vec<McpStatus> {
    ctx.get::<Mcp>(MCP).map(|m| m.list()).unwrap_or_default()
}

pub(super) fn mcp_overlay_rows(
    ctx: &Context,
    query: &str,
    tools_expanded: &HashSet<usize>,
    section_collapsed: bool,
) -> Vec<mcps::McpRow> {
    mcps::build_rows(
        &mcp_status_list(ctx),
        query,
        tools_expanded,
        section_collapsed,
    )
}

pub(super) enum McpToggleJob {
    Server {
        name: String,
        enabled: bool,
    },
    Tool {
        server: String,
        tool: String,
        enabled: bool,
    },
}

pub(super) fn spawn_mcp_toggle(
    ctx: Context,
    redraw: tokio::sync::mpsc::UnboundedSender<()>,
    job: McpToggleJob,
) {
    if ctx.get::<Mcp>(MCP).is_none() {
        flash(&ctx, "MCP 未挂载");
        return;
    }
    let pending = match &job {
        McpToggleJob::Server { name, enabled } => {
            if *enabled {
                format!("正在启用 {name}…")
            } else {
                format!("正在禁用 {name}…")
            }
        }
        McpToggleJob::Tool {
            server,
            tool,
            enabled,
        } => {
            if *enabled {
                format!("正在启用 {server}/{tool}…")
            } else {
                format!("正在禁用 {server}/{tool}…")
            }
        }
    };
    flash(&ctx, pending);
    tokio::spawn(async move {
        let Some(mcp) = ctx.get::<Mcp>(MCP) else {
            flash(&ctx, "MCP 未挂载");
            let _ = redraw.send(());
            return;
        };
        let result = match &job {
            McpToggleJob::Server { name, enabled } => mcp.set_server_enabled(name, *enabled).await,
            McpToggleJob::Tool {
                server,
                tool,
                enabled,
            } => mcp.set_tool_enabled(server, tool, *enabled).await,
        };
        match result {
            Ok(()) => {
                let done = match &job {
                    McpToggleJob::Server { name, enabled } => {
                        if *enabled {
                            format!("已启用 {name}")
                        } else {
                            format!("已禁用 {name}")
                        }
                    }
                    McpToggleJob::Tool {
                        server,
                        tool,
                        enabled,
                    } => {
                        if *enabled {
                            format!("已启用 {server}/{tool}")
                        } else {
                            format!("已禁用 {server}/{tool}")
                        }
                    }
                };
                flash(&ctx, done);
            }
            Err(e) => flash(&ctx, e),
        }
        let _ = redraw.send(());
    });
}

/// 重读 `config.toml` 并与已连服务器对账。`quiet`（开 `/mcps` 时的自动重载）
/// 只在确实变了或有失败时 flash；手动 Ctrl+R 一律给反馈，否则按键像没反应。
pub(super) fn spawn_mcp_reload(
    ctx: Context,
    redraw: tokio::sync::mpsc::UnboundedSender<()>,
    quiet: bool,
) {
    if ctx.get::<Mcp>(MCP).is_none() {
        if !quiet {
            flash(&ctx, "MCP 未挂载");
        }
        return;
    }
    if !quiet {
        flash(&ctx, "正在重载 MCP 配置…");
    }
    tokio::spawn(async move {
        let Some(mcp) = ctx.get::<Mcp>(MCP) else {
            if !quiet {
                flash(&ctx, "MCP 未挂载");
            }
            let _ = redraw.send(());
            return;
        };
        match mcp.reload().await {
            Ok(report) => {
                if !quiet || !report.is_noop() {
                    flash(&ctx, report.summary());
                }
            }
            // 并发重载撞闸时静默：自动那次已经在做同样的事了。
            Err(e) if !quiet => flash(&ctx, e),
            Err(_) => {}
        }
        let _ = redraw.send(());
    });
}

pub(super) fn spawn_mcp_auth(
    ctx: Context,
    redraw: tokio::sync::mpsc::UnboundedSender<()>,
    name: String,
) {
    if ctx.get::<Mcp>(MCP).is_none() {
        flash(&ctx, "MCP 未挂载");
        return;
    }
    flash(&ctx, format!("正在打开浏览器登录 {name}…"));
    tokio::spawn(async move {
        let Some(mcp) = ctx.get::<Mcp>(MCP) else {
            flash(&ctx, "MCP 未挂载");
            let _ = redraw.send(());
            return;
        };
        match mcp.authenticate(&name).await {
            Ok(()) => flash(&ctx, format!("已登录 {name}")),
            Err(e) => flash(&ctx, e),
        }
        let _ = redraw.send(());
    });
}

pub(super) fn file_search_open(ctx: &Context) -> bool {
    ctx.get::<PromptWidget>(TUI_PROMPT)
        .is_some_and(|p| p.file_search_snapshot().open)
}

pub(super) fn turn_running(ctx: &Context) -> bool {
    ctx.get::<SessionRef>(SESSION_PORT)
        .is_some_and(|s| s.working())
}

pub(super) fn prompt_can_send(ctx: &Context) -> bool {
    ctx.get::<PromptWidget>(TUI_PROMPT)
        .is_some_and(|p| p.can_send())
}

pub(super) fn has_prompt_queue(ctx: &Context) -> bool {
    ctx.get::<SessionRef>(SESSION_PORT)
        .is_some_and(|s| !s.queued_prompts().is_empty())
}

pub(super) fn live_redraw(ctx: &Context, overlay: &Overlay) -> bool {
    welcome_open(ctx)
        || turn_running(ctx)
        || matches!(overlay, Overlay::Inspect { .. } | Overlay::Tasks { .. })
        || ctx.get::<Goal>(GOAL).is_some_and(|g| g.active())
        // 任务条常驻主界面且带耗时计时器，只要还有活着的任务就得继续重绘。
        // 原来分开写的「子代理 running」「job !done」两条都被它覆盖，而且补上了
        // idle 子代理、workflow、定时任务 —— 那三类以前会让计时器冻住。
        || any_live_task(ctx)
        // 装 driver 要下几十 MB，进度是子进程一行行吐出来的：不持续重绘的话
        // 驾驶舱会停在按下 Enter 的那一帧。
        || ctx.get::<Computer>(COMPUTER).is_some_and(|c| c.busy())
}

/// Grok cancel-rewind: restore the full sent prompt when the turn has no
/// assistant/tool output yet. A follow-up draft in the composer, or a queued
/// prompt, keeps a standard cancel.
pub(super) fn rewind_cancelled_send(ctx: &Context) -> bool {
    let Some(prompt) = ctx.get::<PromptWidget>(TUI_PROMPT) else {
        return false;
    };
    if prompt.can_send() {
        prompt.take_last_sent();
        return false;
    }
    if ctx
        .get::<SessionRef>(SESSION_PORT)
        .is_some_and(|s| s.has_queued())
    {
        prompt.take_last_sent();
        return false;
    }
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return false;
    };
    if let Some((text, images)) = sessions.rewind_inflight_user() {
        prompt.restore_sent(&text, pasted_from_user(&text, images));
        prompt.take_last_sent();
        return true;
    }
    if sessions.last_turn_has_output() {
        prompt.take_last_sent();
        return false;
    }
    let images = sessions.take_pending_images();
    let Some(text) = prompt.take_last_sent() else {
        return false;
    };
    prompt.restore_sent(&text, pasted_from_user(&text, images));
    true
}

/// Idle Esc / `/undo`: restore the last user send when that turn still has no
/// model/tool output. Unlike cancel-rewind, there is no `last_sent` fallback —
/// the turn already finished (error-only / cancelled earlier).
pub(super) fn rewind_idle_send(ctx: &Context) -> bool {
    let Some(prompt) = ctx.get::<PromptWidget>(TUI_PROMPT) else {
        return false;
    };
    if prompt.can_send() {
        return false;
    }
    if ctx
        .get::<SessionRef>(SESSION_PORT)
        .is_some_and(|s| s.has_queued())
    {
        return false;
    }
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return false;
    };
    if let Some((text, images)) = sessions.rewind_inflight_user() {
        prompt.restore_sent(&text, pasted_from_user(&text, images));
        prompt.take_last_sent();
        return true;
    }
    false
}

pub(super) fn pasted_from_user(text: &str, images: Vec<UserImage>) -> Vec<PastedImage> {
    let ns = image_chip_numbers(text);
    images
        .into_iter()
        .enumerate()
        .map(|(i, img)| PastedImage {
            n: ns.get(i).copied().unwrap_or(i as u32 + 1),
            mime: img.mime,
            data: img.data,
            width: img.width,
            height: img.height,
        })
        .collect()
}

pub(super) fn image_chip_numbers(text: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut rest = text;
    let marker = "[Image #";
    while let Some(at) = rest.find(marker) {
        rest = &rest[at + marker.len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u32>() {
            if n > 0 {
                out.push(n);
            }
        }
    }
    out
}

pub(super) fn export_transcript(ctx: &Context, path: &std::path::Path) -> Result<(), String> {
    let sessions = ctx
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| "no sessions".to_string())?;
    let body: String = sessions.events().iter().map(|e| format!("{e}\n")).collect();
    std::fs::write(path, body).map_err(|e| e.to_string())
}

pub(super) fn mermaid_click(ctx: &Context, kind: AffordanceKind, source: String) -> Vec<Effect> {
    let prefer_mmdc = ctx
        .get::<AppSettings>(SETTINGS)
        .is_some_and(|s| s.mermaid_engine() == MermaidEngineKind::Mmdc);
    match kind {
        AffordanceKind::CopySource => vec![Effect::CopyText(source)],
        AffordanceKind::Open | AffordanceKind::CopyPath => {
            match mermaid_png::render_open_png(&source, prefer_mmdc) {
                Ok(path) => {
                    if kind == AffordanceKind::Open {
                        vec![Effect::OpenImage(path)]
                    } else {
                        vec![Effect::CopyText(path.display().to_string())]
                    }
                }
                Err(e) => {
                    flash(ctx, e);
                    Vec::new()
                }
            }
        }
    }
}

pub(super) fn copy_out(ctx: &Context, text: &str, file: Option<&std::path::Path>) {
    let result = if let Some(path) = file {
        clipboard::write_file(path, text).map(|_| format!("wrote {}", path.display()))
    } else {
        clipboard::copy_text(text).map(|_| "已复制".into())
    };
    match result {
        Ok(msg) => flash(ctx, msg),
        Err(e) => flash(ctx, e),
    }
}

/// 分页服务。没挂（裁剪过的树、单元测试）就是单页模式。
pub(super) fn tabs_service(root: &Context) -> Option<std::sync::Arc<Tabs>> {
    root.get::<Tabs>(TUI_TABS)
}

/// 当前分页的上下文；没挂分页服务时就是根本身。
pub(super) fn active_ctx(root: &Context) -> Context {
    tabs_service(root)
        .map(|t| t.active_ctx())
        .unwrap_or_else(|| root.clone())
}

pub(super) fn flash(ctx: &Context, msg: impl Into<String>) {
    if let Ok(status) = ctx.require::<StatusLine>(TUI_STATUS) {
        status.flash(msg);
    }
}

pub(super) fn apply_preset_action(ctx: &Context, overlay: &mut Overlay, action: PresetAction) {
    match action {
        PresetAction::None => {}
        PresetAction::Flash(msg) => flash(ctx, msg),
        PresetAction::OpenCanvas(view) => {
            let label = match &view {
                PresetView::Canvas(c) => ctx
                    .get::<AgentPresets>(AGENT_PRESETS)
                    .and_then(|p| p.get(&c.id))
                    .map(|p| p.name)
                    .unwrap_or_else(|| c.id.clone()),
                PresetView::Roster { .. } => String::new(),
            };
            *overlay = Overlay::Presets(view);
            if !label.is_empty() {
                flash(ctx, format!("已应用 {label}"));
            }
        }
    }
}

pub(super) fn open_presets_overlay(ctx: &Context, focus: Option<String>) -> Overlay {
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        flash(ctx, "Agent 预设服务未挂载");
        return Overlay::None;
    };
    presets.resync();
    if let Some(id) = focus {
        match preset_overlay::open_canvas(ctx, &id) {
            Ok(view) => {
                flash(ctx, format!("已应用 {id}"));
                Overlay::Presets(view)
            }
            Err(e) => {
                flash(ctx, e);
                Overlay::Presets(PresetView::roster())
            }
        }
    } else {
        Overlay::Presets(PresetView::roster())
    }
}

pub(super) fn intercept_goal_send(ctx: &Context, text: &str) -> Option<Vec<Effect>> {
    let goal = ctx.get::<Goal>(GOAL)?;
    if !goal.awaiting_composer() {
        return None;
    }
    match interpret_goal_composer_ex(text, &slash_extras(ctx)) {
        GoalComposer::Stub => Some(vec![Effect::FillPrompt {
            text: goal_composer_fill(),
        }]),
        GoalComposer::Objective(obj) => {
            goal.disarm_composer();
            Some(vec![Effect::EnterGoal {
                objective: Some(obj),
            }])
        }
        GoalComposer::Slash(line) => {
            goal.disarm_composer();
            let extras = slash_extras(ctx);
            slash::command_for_submit_ex(&line, &extras).map(|(pick, args)| {
                vec![crate::app::actions::effect_for_pick(pick, &args, &extras)]
            })
        }
    }
}

pub(super) fn start_goal(ctx: &Context, raw: String) {
    let Some(goal) = ctx.get::<Goal>(GOAL) else {
        flash(ctx, "目标服务未挂载");
        return;
    };
    goal.disarm_composer();
    goal.start(raw.clone());
    flash(ctx, "目标模式");
    if let Ok(session) = ctx.require::<SessionRef>(SESSION_PORT) {
        session.submit(raw, false);
    }
}

pub(super) fn start_loop(ctx: &Context, args: String) {
    let visible = format!("/loop {args}");
    if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
        sessions.arm_user_addon(
            visible.clone(),
            loop_schedule_instruction(&args, LoopFireMode::InSession),
        );
    }
    flash(ctx, "正在安排循环任务");
    if let Ok(session) = ctx.require::<SessionRef>(SESSION_PORT) {
        session.submit(visible, false);
    }
}

pub(super) fn intercept_loop_send(text: &str) -> Option<Vec<Effect>> {
    match interpret_loop_composer(text) {
        LoopComposer::Stub => Some(vec![Effect::FillPrompt {
            text: loop_composer_fill(),
        }]),
        LoopComposer::Schedule(args) => Some(vec![Effect::EnterLoop { args }]),
        LoopComposer::Other => None,
    }
}

pub(super) fn cancel_scheduled(ctx: &Context, id: &str) {
    match ctx.get::<Cron>(CRON) {
        Some(cron) if cron.cancel(id) => flash(ctx, format!("已关闭 {id}")),
        Some(_) => flash(ctx, format!("没有定时任务 {id}")),
        None => flash(ctx, "cron 未挂载"),
    }
}

/// 停一次 workflow run。`needle` 可以是 run id，也可以是显示名。
pub(super) fn stop_workflow(ctx: &Context, needle: &str) {
    match ctx.get::<Workflows>(WORKFLOWS) {
        Some(wf) => match wf.stop(needle) {
            Some(run_id) => flash(ctx, format!("已停止 {run_id}")),
            None => flash(ctx, format!("没有在跑的工作流 {needle}")),
        },
        None => flash(ctx, "workflows 未挂载"),
    }
}

/// `[✗]` / `x` 的落点：按目标类型分派。
pub(super) fn kill_task_target(ctx: &Context, target: &KillTarget) {
    match target {
        KillTarget::Scheduled(id) => cancel_scheduled(ctx, id),
        KillTarget::Workflow(run_id) => stop_workflow(ctx, run_id),
        KillTarget::Job(id) => kill_job(ctx, id),
        KillTarget::Subagent(id) => kill_subagent(ctx, id),
    }
}

/// Same path as the `kill_task` tool for background jobs.
fn kill_job(ctx: &Context, id: &str) {
    let Some(jobs) = ctx.get::<Jobs>(JOBS) else {
        flash(ctx, "jobs 未挂载");
        return;
    };
    let jobs = jobs.clone();
    let id = id.to_string();
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let msg = jobs.kill(&id).await;
        flash(&ctx, msg);
    });
}

fn kill_subagent(ctx: &Context, id: &str) {
    match ctx.get::<Subagents>(SUBAGENTS) {
        Some(sub) => match sub.kill(id) {
            Some(msg) => flash(ctx, msg),
            None => flash(ctx, format!("没有子代理 {id}")),
        },
        None => flash(ctx, "subagents 未挂载"),
    }
}

pub(super) fn open_goal_overlay(ctx: &Context, overlay: &mut Overlay, editing: bool) {
    let Some(goal) = ctx.get::<Goal>(GOAL) else {
        flash(ctx, "目标服务未挂载");
        return;
    };
    goal.disarm_composer();
    if !goal.present() {
        flash(ctx, "没有活动目标");
        return;
    }
    *overlay = Overlay::Goal {
        selected: 0,
        editing,
        draft: if editing { goal.title() } else { String::new() },
    };
}

pub(super) fn goal_pause_resume(ctx: &Context) -> Vec<Effect> {
    let Some(goal) = ctx.get::<Goal>(GOAL) else {
        flash(ctx, "目标服务未挂载");
        return Vec::new();
    };
    if goal.paused() {
        if goal.resume() {
            flash(ctx, "目标已继续");
        }
    } else if goal.pause() {
        flash(ctx, "目标已暂停");
    } else {
        flash(ctx, "没有进行中的目标");
    }
    Vec::new()
}

pub(super) fn apply_goal_hit(ctx: &Context, overlay: &mut Overlay, hit: GoalHit) -> Vec<Effect> {
    match hit {
        GoalHit::Pause => goal_pause_resume(ctx),
        GoalHit::Edit => vec![Effect::ShowGoal { editing: true }],
        GoalHit::Close => vec![Effect::GoalClear],
        GoalHit::Open => {
            if matches!(overlay, Overlay::Goal { .. }) {
                overlay.close();
                Vec::new()
            } else {
                vec![Effect::ShowGoal { editing: false }]
            }
        }
    }
}

/// 这一帧的输入框有没有盖住这个坐标。没挂输入框（裁剪过的树）就当没有。
pub(super) fn prompt_hit(ctx: &Context, column: u16, row: u16) -> bool {
    ctx.get::<crate::views::prompt::PromptWidget>(crate::names::TUI_PROMPT)
        .is_some_and(|p| p.hit(column, row))
}

pub(super) fn welcome_open(ctx: &Context) -> bool {
    ctx.get::<Welcome>(TUI_WELCOME)
        .is_some_and(|w| w.empty_session())
}

pub(super) fn slash_open(ctx: &Context) -> bool {
    ctx.get::<PromptWidget>(TUI_PROMPT)
        .is_some_and(|p| p.slash_snapshot().open)
}

/// Enter fills `/cmd ` only while choosing a command name. After a space,
/// the arg dropdown stays visible for Tab and Enter sends —— 除非用户上下选过
/// 其中一行：那就是他要那一行，Enter 先把它填进输入框（再按一下才发）。
pub(super) fn slash_captures_enter(ctx: &Context) -> bool {
    ctx.get::<PromptWidget>(TUI_PROMPT).is_some_and(|p| {
        let snap = p.slash_snapshot();
        snap.open && (!snap.completing_args || p.slash_picked())
    })
}

pub(super) fn overlay_len(ctx: &Context, overlay: &Overlay) -> usize {
    match overlay {
        Overlay::None => 0,
        Overlay::Resume { query, .. } => ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| filter_sessions(&s.archived(), query).len())
            .unwrap_or(0),
        Overlay::Dashboard {
            query, collapsed, ..
        } => crate::views::dashboard::build_rows(ctx, query, collapsed).len(),
        Overlay::Help { query, .. } => filter_help_items(query, &slash_extras(ctx)).len(),
        Overlay::History { query, .. } => ctx
            .get::<PromptWidget>(TUI_PROMPT)
            .map(|p| filter_strings(&p.history(), query).len())
            .unwrap_or(0),
        Overlay::Find { query, .. } => ctx
            .get::<Scrollback>(TUI_SCROLLBACK)
            .map(|s| s.find(query).len())
            .unwrap_or(0),
        Overlay::Args { kind, query, .. } => {
            filter_args(*kind, query, ctx.get::<AppSettings>(SETTINGS).as_deref()).len()
        }
        Overlay::Settings { picking, .. } => match picking {
            None => settings_modal::field_rows().len(),
            Some(field) => ctx
                .get::<AppSettings>(SETTINGS)
                .map(|s| settings_modal::enum_choices(*field, &s).len())
                .unwrap_or(0),
        },
        Overlay::Permission { .. } => permission_view::OPTIONS.len(),
        Overlay::PairingPending { .. } => pairing::PENDING_OPTIONS.len(),
        Overlay::PairingManage { .. } => {
            let gw = ctx.get::<GatewayRef>(GATEWAY);
            pairing::overlay_len(
                &gw.as_ref().map(|g| g.pairing_pending()).unwrap_or_default(),
                &gw.as_ref()
                    .map(|g| g.pairing_bindings())
                    .unwrap_or_default(),
            )
        }
        Overlay::PlanApproval { view_only, .. } => plan_approval_view::options(*view_only).len(),
        Overlay::Ask { picked, .. } => ctx
            .get::<Ask>(ASK)
            .and_then(|a| a.front())
            .and_then(|p| p.questions.get(p.index).map(ask_view::labels))
            .map(|l| l.len())
            .unwrap_or(picked.len()),
        Overlay::Elicit { picked, .. } => elicit_front(ctx)
            .map(|p| if p.typing { 0 } else { p.options.len() })
            .unwrap_or(picked.len()),
        Overlay::Tasks {
            query, collapsed, ..
        } => task_entries(ctx, query, collapsed).len(),
        Overlay::Mcps {
            query,
            tools_expanded,
            section_collapsed,
            ..
        } => mcp_overlay_rows(ctx, query, tools_expanded, *section_collapsed).len(),
        Overlay::Workflows { query, .. } => workflow_rows(ctx, query).len(),
        Overlay::Goal { editing, .. } => {
            if *editing {
                0
            } else {
                goal_overlay::ACTION_COUNT
            }
        }
        Overlay::Usage { .. }
        | Overlay::Notice { .. }
        | Overlay::MemoryBrowser(_)
        | Overlay::Slot { .. }
        | Overlay::Browser { .. }
        | Overlay::Computer { .. }
        | Overlay::Inspect { .. } => 0,
        Overlay::Presets(view) => preset_overlay::overlay_len(ctx, view),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回到一道已经用「其他」答过的题：高亮停在「其他」行上，焦点就得跟着给，
    /// 否则打字 / 退格 / ←→ 全被 `draft_focused` 挡掉，而 ←→ 的问题切换又被
    /// `other_active` 挡掉，整行变成谁也进不去的死状态。
    #[test]
    fn restored_other_answer_keeps_the_draft_focused() {
        use cordis_spine::{AskPrompt, Question, QuestionOption};
        let front = AskPrompt {
            questions: vec![Question {
                question: "选哪个？".into(),
                options: vec![QuestionOption {
                    label: "甲".into(),
                    description: String::new(),
                    preview: None,
                    id: None,
                }],
                multi_select: Some(false),
                id: None,
            }],
            index: 0,
            current_labels: vec!["Other".into()],
            current_notes: Some("上次填的内容".into()),
            max_index: 0,
        };
        match ask_overlay_from_prompt(&front) {
            Overlay::Ask {
                selected,
                draft,
                draft_focused,
                ..
            } => {
                assert_eq!(draft, "上次填的内容");
                assert!(selected > 0, "应停在「其他」那行");
                assert!(draft_focused, "回到这题要能直接改");
            }
            other => panic!("{other:?}"),
        }
    }

    /// 反向：没答过「其他」的题，开出来不该白白占着焦点。
    #[test]
    fn a_fresh_question_does_not_focus_the_draft() {
        use cordis_spine::{AskPrompt, Question, QuestionOption};
        let front = AskPrompt {
            questions: vec![Question {
                question: "选哪个？".into(),
                options: vec![QuestionOption {
                    label: "甲".into(),
                    description: String::new(),
                    preview: None,
                    id: None,
                }],
                multi_select: Some(false),
                id: None,
            }],
            index: 0,
            current_labels: Vec::new(),
            current_notes: None,
            max_index: 0,
        };
        match ask_overlay_from_prompt(&front) {
            Overlay::Ask {
                selected,
                draft_focused,
                ..
            } => {
                assert_eq!(selected, 0);
                assert!(!draft_focused);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn image_chip_numbers_reads_in_order() {
        assert_eq!(
            image_chip_numbers("see [Image #1] and [Image #3]"),
            vec![1, 3]
        );
        assert!(image_chip_numbers("no chips").is_empty());
    }

    #[tokio::test]
    async fn slot_overlay_opens_from_request_and_esc_closes() {
        use std::sync::Arc;

        struct StaticSlot;

        impl cordis_spine::SlotHandler for StaticSlot {
            fn title(&self) -> String {
                "便签".into()
            }
            fn hud(&self) -> bool {
                true
            }
            fn render(&self) -> String {
                "hello slot".into()
            }
            fn on_key(&self, key: &str) -> SlotKeyResult {
                if key == "esc" {
                    SlotKeyResult::Close
                } else {
                    SlotKeyResult::Keep
                }
            }
        }

        let root = Context::new();
        let slots = TuiSlots::new();
        let _hold = root.provide(TUI_SLOTS, slots.clone()).unwrap();
        slots.register("memo".into(), Arc::new(StaticSlot)).unwrap();
        slots.request_open("memo").unwrap();
        let mut overlay = Overlay::None;
        open_slot_if_needed(&root, &mut overlay);
        assert!(matches!(&overlay, Overlay::Slot { id, .. } if id == "memo"));
        assert!(dispatch_slot_key(&root, &mut overlay, "esc"));
        assert!(!overlay.is_open());
    }

    /// 任务条带耗时计时器，只要还有活着的后台任务就必须继续 80ms 重绘。
    #[tokio::test]
    async fn live_redraw_follows_background_jobs() {
        let root = Context::new();
        let jobs = Jobs::new();
        let _hold = root.provide(JOBS, jobs).unwrap();
        assert!(
            !live_redraw(&root, &Overlay::None),
            "没有任务时不该空转重绘"
        );

        let jobs = root.get::<Jobs>(JOBS).unwrap();
        let id = jobs.start("sleep 30");
        for _ in 0..100 {
            if any_live_task(&root) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            live_redraw(&root, &Overlay::None),
            "有后台任务在跑就得继续重绘，否则任务条的耗时会冻住"
        );

        let _ = jobs.kill(&id).await;
        for _ in 0..100 {
            if !any_live_task(&root) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            !live_redraw(&root, &Overlay::None),
            "任务结束后要停下来，别一直空转"
        );
    }

    /// 这条打的是改动前真正的缺口：旧 `live_redraw` 只看「子代理 `running()`」和
    /// 「job 未完成」，**定时任务完全不在里面**。任务条现在会画 `/loop` 行并带
    /// 耗时，不补这条子句计时器就冻住。同理适用于 idle 子代理与 workflow ——
    /// 它们构造成本高，这里用 Cron 代表整类。
    #[tokio::test]
    async fn live_redraw_covers_scheduled_tasks() {
        let root = Context::new();
        let _hold = root.provide(CRON, Cron::new()).unwrap();
        assert!(!live_redraw(&root, &Overlay::None), "空表不该空转重绘");

        let cron = root.get::<Cron>(CRON).unwrap();
        let id = cron
            .add(std::time::Duration::from_secs(600), "每十分钟看一眼 CI")
            .unwrap();
        assert!(any_live_task(&root), "已排程的定时任务算活着的任务");
        assert!(
            live_redraw(&root, &Overlay::None),
            "定时任务在表上时要继续重绘（旧实现这里是 false，任务条耗时会冻住）"
        );
        assert!(
            !task_dock_rows(&root).is_empty(),
            "定时任务要出现在任务条上"
        );

        assert!(cron.cancel(&id));
        assert!(!live_redraw(&root, &Overlay::None), "取消后要停下来");
    }

    /// 前台 bash 不进任务条，也就不该单独把重绘拉起来 —— turn status 那一行
    /// 已经在显示「Waiting…」，每条快命令都闪一下任务条只是噪音。
    #[tokio::test]
    async fn foreground_commands_do_not_drive_the_task_dock() {
        let root = Context::new();
        let _hold = root.provide(JOBS, Jobs::new()).unwrap();
        let jobs = root.get::<Jobs>(JOBS).unwrap();
        let id = jobs.start_foreground("sleep 30");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert!(!any_live_task(&root), "前台命令不算任务条上的任务");
        assert!(
            task_dock_rows(&root).is_empty(),
            "前台命令不该出现在任务条上"
        );
        let _ = jobs.kill(&id).await;
    }

    /// MCP elicitation 的选择态活在 overlay 上，而 `ElicitPrompt.selected` 每次
    /// `front()` 都由 `default_selected(field)` 重算。每轮事件循环都会依次跑
    /// `open_permission_if_needed` → `open_ask_if_needed` → `open_elicit_if_needed`，
    /// 前两个只要看见 Elicit 就无条件 `close()`，**没确认自己真有待处理项**。
    /// 于是 elicit overlay 每轮被拆掉重建，selected / picked / draft 全部打回默认：
    /// 上下键选完立刻被重置（看起来"不能上下选"），回车提交的是默认项而不是
    /// 高亮项，选到「其他」时 draft 也被清空 → 闪 `请输入具体内容` 后什么都没发生，
    /// MCP 请求永远不 resolve，工具调用和整轮对话一起卡死。
    ///
    /// 点击之所以"能用"，是因为鼠标那一路在同一个事件里选中并立刻 accept，
    /// 中间没有插进一轮循环。
    #[test]
    fn probes_keep_a_live_elicit_overlay_when_they_have_nothing_to_show() {
        let live = || Overlay::Elicit {
            selected: 2,
            picked: vec![false, false, true],
            draft: "staging".into(),
        };
        let assert_intact = |overlay: &Overlay, who: &str| match overlay {
            Overlay::Elicit {
                selected,
                picked,
                draft,
            } => {
                assert_eq!(*selected, 2, "{who} 不该重置 selected");
                assert_eq!(
                    picked.as_slice(),
                    [false, false, true],
                    "{who} 不该重置 picked"
                );
                assert_eq!(draft, "staging", "{who} 不该清空 draft");
            }
            other => panic!("{who} 不该关掉 elicit overlay，实际：{other:?}"),
        };

        // 没有 ASK / PERMISSIONS 服务 —— 等价于「没有待处理的提问 / 授权」。
        let root = Context::new();

        let mut overlay = live();
        open_ask_if_needed(&root, &mut overlay);
        assert_intact(&overlay, "open_ask_if_needed");

        let mut overlay = live();
        open_permission_if_needed(&root, &mut overlay);
        assert_intact(&overlay, "open_permission_if_needed");

        // 事件循环里排在 elicit 之前的三个探针连着跑完也要原样还在。
        // （`open_elicit_if_needed` 是 owner，队列空时由它关掉是正确行为，
        // 这个测试里没挂 MCP 服务，所以不把它算进来。）
        let mut overlay = live();
        open_pairing_if_needed(&root, &mut overlay);
        open_permission_if_needed(&root, &mut overlay);
        open_ask_if_needed(&root, &mut overlay);
        assert_intact(&overlay, "elicit 之前的三个探针");
    }

    /// 反方向：修好「没东西就别动 elicit」之后，**真有**待回答提问时的抢占不能丢。
    #[tokio::test]
    async fn a_pending_ask_still_preempts_the_elicit_overlay() {
        use cordis_spine::{Ask, Question, QuestionOption, ASK};

        let root = Context::new();
        let _hold = root.provide(ASK, Ask::new(root.clone())).unwrap();
        let ask = root.get::<Ask>(ASK).unwrap();
        let pending = tokio::spawn({
            let ask = ask.clone();
            async move {
                ask.ask(vec![Question {
                    question: "继续吗？".into(),
                    options: vec![QuestionOption {
                        label: "继续".into(),
                        description: String::new(),
                        preview: None,
                        id: None,
                    }],
                    multi_select: Some(false),
                    id: None,
                }])
                .await
            }
        });
        for _ in 0..100 {
            if ask.front().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let mut overlay = Overlay::Elicit {
            selected: 1,
            picked: Vec::new(),
            draft: String::new(),
        };
        open_ask_if_needed(&root, &mut overlay);
        assert!(
            matches!(overlay, Overlay::Ask { .. }),
            "有待回答的提问时仍要抢前台，实际：{overlay:?}"
        );
        ask.cancel();
        let _ = pending.await;
    }

    /// `collect_items` 不过滤 done 子代理（`/tasks` 要把它们列出来），任务条必须
    /// 自己挡掉；idle 的要留着，它还能接 `send_message`。
    #[test]
    fn done_subagents_are_kept_out_of_the_dock() {
        use crate::grok::tasks_pane::TaskEntry;
        use ratatui::text::Line;

        let agent = |id: &str, running: bool| TaskEntry::Agent {
            id: 1,
            subagent_id: id.into(),
            child_session_id: String::new(),
            label: id.into(),
            styled: Line::from(id.to_string()),
            running,
            started_at: Instant::now(),
            type_label: "观".into(),
        };
        let tails = HashMap::new();
        let now = Instant::now();

        // idle（running=false）但还活着 → 要画。
        let live: HashSet<String> = ["idle-one".to_string()].into_iter().collect();
        assert!(
            task_dock_row(agent("idle-one", false), &tails, &live, now).is_some(),
            "idle 子代理要留在任务条上"
        );
        // 同样 running=false，但已经 done → 不画。两者在 TaskEntry 上无法区分，
        // 所以必须靠活跃集合，不能只看 running。
        assert!(
            task_dock_row(agent("finished", false), &tails, &HashSet::new(), now).is_none(),
            "done 子代理不该出现在任务条上"
        );
    }

    /// 任务条的行要能读到最新一行输出 —— 这是它比旧头像方块有用的唯一理由。
    #[tokio::test]
    async fn task_dock_row_carries_the_latest_output_line() {
        let root = Context::new();
        let _hold = root.provide(JOBS, Jobs::new()).unwrap();
        let jobs = root.get::<Jobs>(JOBS).unwrap();
        let id = jobs.start("echo 早先一行; echo 最新一行; sleep 30");

        let mut tail = String::new();
        for _ in 0..150 {
            if let Some(row) = task_dock_rows(&root).into_iter().next() {
                if !row.tail.is_empty() {
                    tail = row.tail;
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(tail, "最新一行", "任务条摘要取最后一行");
        let _ = jobs.kill(&id).await;
    }
}
