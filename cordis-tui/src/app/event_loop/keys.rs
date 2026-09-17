//! Overlay and prompt key handling (`run_action` / `to_action`).

use std::time::Instant;

use cordis::Context;
use cordis_spine::{
    AppSettings, Ask, Browser, Computer, CuaAction, Goal, Mcp, PermissionMode,
    PermissionOptionKind, Permissions, PlanDecision, PlanMode, Sessions, TuiSlots, UserImage, ASK,
    BROWSER, COMPUTER, GOAL, MCP, PERMISSIONS, PLAN_MODE, SESSIONS, SETTINGS, TUI_SLOTS,
};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::Rect;

use crate::grok::mcps;
use crate::grok::picker::PickerHits;
use crate::grok::tasks_pane::TaskEntry;
use crate::names::{TUI_PROMPT, TUI_SCROLLBACK, TUI_STATUS, TUI_WELCOME};
use crate::scrollback::{ClickHit, MouseUpResult, Scrollback};
use crate::slash::filter_args;
use crate::views::ask_view;
use crate::views::overlay::{
    self, filter_sessions, filter_strings, HelpItem, HelpKind, InspectTarget, Overlay, UsageTab,
};
use crate::views::permission_view;
use crate::views::plan_approval_view;
use crate::views::preset_overlay::{self, PresetAction, PresetView};
use crate::views::queue_pane::{self, QueueHit};
use crate::views::settings_modal;
use crate::views::tab_bar;
use std::collections::HashSet;

use crate::views::task_dock::{self, TaskDockHit};

use super::support::*;
use crate::app::actions::{Action, Effect};
use crate::app::dispatch::dispatch;
use crate::app::mode_cycle;
use crate::views::goal_overlay;
use crate::views::goal_pane::{self, GoalHit};
use crate::views::inspect_overlay::{self, InspectClick};
use crate::views::prompt::PromptWidget;
use crate::views::status::StatusLine;
use crate::views::usage_overlay;
use crate::views::welcome::{Welcome, WelcomeHit};

#[allow(clippy::too_many_arguments)]
// 后四个参数都是上一帧的点击命中表（picker / 任务条 / 目标条 / 排队条 / 标签栏）：
// 打包成结构体只会把噪音搬到每个调用点和每处测试，故意保留成平铺。
pub(super) fn run_action(
    ctx: &Context,
    action: Action,
    overlay: &mut Overlay,
    hits: &PickerHits,
    dock_hits: &[(Rect, TaskDockHit)],
    goal_hits: &[(Rect, GoalHit)],
    queue_hits: &[(Rect, QueueHit)],
    tab_hits: &[(Rect, tab_bar::TabHit)],
) -> Vec<Effect> {
    match action {
        Action::OverlayClose => {
            if let Overlay::Goal { editing, .. } = overlay {
                if *editing {
                    *editing = false;
                    return Vec::new();
                }
            }
            if let Overlay::Settings {
                picking, selected, ..
            } = overlay
            {
                if picking.is_some() {
                    let field = picking.take();
                    *selected = field
                        .and_then(|f| {
                            settings_modal::field_rows()
                                .iter()
                                .position(|row| *row == f)
                        })
                        .unwrap_or(0);
                    return Vec::new();
                }
            }
            if matches!(overlay, Overlay::Permission { .. }) {
                if let Some(perms) = ctx.get::<Permissions>(PERMISSIONS) {
                    perms.resolve(PermissionOptionKind::RejectOnce);
                }
            }
            if matches!(overlay, Overlay::PairingPending { .. }) {
                if let Some(id) = ctx
                    .get::<crate::seam::gateway::GatewayRef>(crate::names::GATEWAY)
                    .and_then(|g| g.pairing_front().map(|p| p.id))
                {
                    let _ = ctx
                        .get::<crate::seam::gateway::GatewayRef>(crate::names::GATEWAY)
                        .and_then(|g| g.pairing_deny(&id).ok());
                }
            }
            if let Overlay::Ask {
                draft,
                draft_cursor,
                ..
            } = overlay
            {
                if !draft.is_empty() {
                    draft.clear();
                    *draft_cursor = 0;
                    return Vec::new();
                }
                if let Some(ask) = ctx.get::<Ask>(ASK) {
                    ask.cancel();
                }
            }
            if let Overlay::Elicit { draft, .. } = overlay {
                if !draft.is_empty() {
                    draft.clear();
                    return Vec::new();
                }
                if let Some(mcp) = ctx.get::<Mcp>(MCP) {
                    mcp.elicitation().cancel();
                }
            }
            if let Overlay::PlanApproval {
                view_only: false, ..
            } = overlay
            {
                if let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) {
                    plan.resolve(PlanDecision::Quit);
                    flash(ctx, "已放弃计划");
                }
            }
            if let Overlay::Presets(view) = overlay {
                if !preset_overlay::on_esc(ctx, view) {
                    return Vec::new();
                }
            }
            if matches!(overlay, Overlay::Inspect { .. }) {
                return close_inspect(overlay);
            }
            if let Overlay::Usage { detail, scroll, .. } = overlay {
                if detail.is_some() {
                    *detail = None;
                    *scroll = 0;
                    return Vec::new();
                }
            }
            // 确认态先退回驾驶舱：Esc 是「不装了」，不是「关窗」。
            if let Overlay::Computer { pending, .. } = overlay {
                if pending.take().is_some() {
                    return Vec::new();
                }
                if let Some(computer) = ctx.get::<Computer>(COMPUTER) {
                    computer.clear_finished();
                }
            }
            let _ = dispatch_slot_key(ctx, overlay, "esc");
            overlay.close();
            Vec::new()
        }
        Action::OverlayMove(delta) => {
            if let Overlay::Usage {
                tab,
                scroll,
                detail,
            } = overlay
            {
                scroll_usage(*tab, *detail, scroll, delta);
                return Vec::new();
            }
            if let Overlay::Notice { body, scroll, .. } = overlay {
                scroll_text(scroll, delta, body);
                return Vec::new();
            }
            if let Overlay::Browser { scroll } = overlay {
                let body = browser_cockpit_body(ctx);
                scroll_text(scroll, delta, &body);
                return Vec::new();
            }
            if let Overlay::Computer { scroll, pending } = overlay {
                let body = computer_cockpit_body(ctx, *pending);
                scroll_text(scroll, delta, &body);
                return Vec::new();
            }
            if let Overlay::Inspect { target, scroll, .. } = overlay {
                // Stick-to-bottom: OverlayMove(-1) is ↑ and should reveal older lines.
                scroll_inspect(ctx, target, scroll, -delta);
                return Vec::new();
            }
            if matches!(overlay, Overlay::Slot { .. }) {
                let key = if delta < 0 { "up" } else { "down" };
                let _ = dispatch_slot_key(ctx, overlay, key);
                if let Overlay::Slot { id, scroll } = overlay {
                    if let Some(slots) = ctx.get::<TuiSlots>(TUI_SLOTS) {
                        if let Some(body) = slots.render(id) {
                            scroll_text(scroll, delta, &body);
                        }
                    }
                }
                return Vec::new();
            }
            // 详情页里 ↑↓ 换的是阶段，不是列表里的 run。
            if let Overlay::Workflows {
                detail: Some(run_id),
                phase,
                query,
                ..
            } = overlay
            {
                let len = workflow_phase_count(ctx, query, run_id);
                *phase = phase
                    .saturating_add_signed(delta as isize)
                    .min(len.saturating_sub(1));
                return Vec::new();
            }
            let len = overlay_len(ctx, overlay);
            overlay.move_sel(delta, len);
            Vec::new()
        }
        Action::OverlayAccept => {
            // Enter 在列表里是打开选中 run 的详情页。
            if let Overlay::Workflows {
                selected,
                query,
                detail,
                phase,
            } = overlay
            {
                if detail.is_none() {
                    if let Some(run) = workflow_rows(ctx, query).get(*selected) {
                        *detail = Some(run.run_id.clone());
                        *phase = 0;
                    }
                }
                return Vec::new();
            }
            if matches!(
                overlay,
                Overlay::Inspect {
                    target: InspectTarget::Job(_),
                    ..
                }
            ) {
                return Vec::new();
            }
            if matches!(
                overlay,
                Overlay::Inspect {
                    target: InspectTarget::Subagent(_),
                    ..
                }
            ) {
                send_inspect_composer(ctx, overlay);
                return Vec::new();
            }
            if let Overlay::Computer { pending, .. } = overlay {
                if let Some(action) = pending.take() {
                    return vec![Effect::CuaRun { action }];
                }
                // 没有待确认的动作时，Enter 照旧关窗（同 /browser 那类只读驾驶舱）。
                if let Some(computer) = ctx.get::<Computer>(COMPUTER) {
                    computer.clear_finished();
                }
            }
            if dispatch_slot_key(ctx, overlay, "enter") {
                return Vec::new();
            }
            accept_overlay(ctx, overlay)
        }
        Action::OverlayChar(c) => {
            if let Overlay::Dashboard {
                focus: crate::views::dashboard::Focus::Composer,
                composer,
                composer_cursor,
                ..
            } = overlay
            {
                crate::views::inspect_overlay::insert_composer(composer, composer_cursor, c);
                return Vec::new();
            }
            if matches!(
                overlay,
                Overlay::Inspect {
                    target: InspectTarget::Job(_),
                    ..
                }
            ) {
                if c == 'q' || c == 'Q' {
                    return close_inspect(overlay);
                }
                return Vec::new();
            }
            if matches!(
                overlay,
                Overlay::Inspect {
                    target: InspectTarget::Subagent(_),
                    ..
                }
            ) {
                let empty = match overlay {
                    Overlay::Inspect { composer, .. } => composer.is_empty(),
                    _ => false,
                };
                if empty && (c == 'q' || c == 'Q') {
                    return close_inspect(overlay);
                }
                overlay.push_char(c);
                return Vec::new();
            }
            if matches!(overlay, Overlay::Slot { .. }) {
                let _ = dispatch_slot_key(ctx, overlay, &format!("char:{c}"));
                return Vec::new();
            }
            if matches!(overlay, Overlay::Presets(_)) {
                // Catalog filter typing; bare `r` with empty filter is resync (#20).
                let typing = match overlay {
                    Overlay::Presets(PresetView::Canvas(s))
                        if s.editing_persona || s.naming_role.is_some() =>
                    {
                        true
                    }
                    Overlay::Presets(PresetView::Canvas(s))
                        if s.pane == preset_overlay::PresetPane::Catalog =>
                    {
                        !(matches!(c, 'r' | 'R') && s.catalog_query.is_empty())
                    }
                    _ => false,
                };
                if typing {
                    // CUA/crossterm often drops KeyCode::Enter; allow `;` / newline to
                    // advance/confirm Roles naming drafts (printable, automation-friendly).
                    let naming = matches!(
                        overlay,
                        Overlay::Presets(PresetView::Canvas(s)) if s.naming_role.is_some()
                    );
                    if naming && (c == ';' || c == '\n') {
                        let action = match overlay {
                            Overlay::Presets(view) => preset_overlay::accept(ctx, view),
                            _ => PresetAction::None,
                        };
                        apply_preset_action(ctx, overlay, action);
                        return Vec::new();
                    }
                    overlay.push_char(c);
                    return Vec::new();
                }
                let action = match overlay {
                    Overlay::Presets(view) if matches!(view, PresetView::Roster { .. }) => {
                        preset_overlay::on_roster_char(ctx, view, c)
                    }
                    Overlay::Presets(view) => preset_overlay::on_canvas_char(ctx, view, c),
                    _ => PresetAction::None,
                };
                apply_preset_action(ctx, overlay, action);
                return Vec::new();
            }
            if let Overlay::Goal {
                editing,
                draft,
                selected,
            } = overlay
            {
                if *editing {
                    overlay.push_char(c);
                    return Vec::new();
                }
                match c {
                    'g' | 'G' => overlay.close(),
                    'e' | 'E' => {
                        *editing = true;
                        *draft = ctx.get::<Goal>(GOAL).map(|g| g.title()).unwrap_or_default();
                        *selected = goal_overlay::ACTION_EDIT;
                    }
                    'p' | 'P' => return goal_pause_resume(ctx),
                    'c' | 'C' => {
                        if let Some(goal) = ctx.get::<Goal>(GOAL) {
                            goal.clear();
                        }
                        overlay.close();
                        flash(ctx, "已清除目标");
                    }
                    _ => {}
                }
                return Vec::new();
            }
            if let Overlay::Permission { selected } = overlay {
                if let Some(i) = c.to_digit(10) {
                    let i = i as usize;
                    if (1..=permission_view::OPTIONS.len()).contains(&i) {
                        *selected = i - 1;
                        return accept_overlay(ctx, overlay);
                    }
                }
                return Vec::new();
            }
            if let Overlay::PairingPending { selected } = overlay {
                if let Some(i) = c.to_digit(10) {
                    let i = i as usize;
                    if (1..=crate::views::pairing::PENDING_OPTIONS.len()).contains(&i) {
                        *selected = i - 1;
                        return accept_overlay(ctx, overlay);
                    }
                }
                return Vec::new();
            }
            if let Overlay::PairingManage { .. } = overlay {
                if c == 'x' || c == 'X' {
                    if let Some(ui) =
                        ctx.get::<crate::views::pairing::PairingUi>(crate::names::TUI_PAIRING)
                    {
                        crate::views::pairing::reject_manage(&ui, overlay);
                    }
                    return Vec::new();
                }
                return Vec::new();
            }
            if let Overlay::PlanApproval {
                selected,
                view_only,
                scroll,
                prompt,
                wrap,
            } = overlay
            {
                match c {
                    'j' | 'J' => {
                        scroll_plan_body(scroll, prompt, wrap, 1);
                        return Vec::new();
                    }
                    'k' | 'K' => {
                        scroll_plan_body(scroll, prompt, wrap, -1);
                        return Vec::new();
                    }
                    _ => {}
                }
                if let Some(i) = c.to_digit(10) {
                    let i = i as usize;
                    let n = plan_approval_view::options(*view_only).len();
                    if (1..=n).contains(&i) {
                        *selected = i - 1;
                        return accept_overlay(ctx, overlay);
                    }
                }
                // Grok shortcuts: a/s/q
                if !*view_only {
                    match c {
                        'a' | 'A' => {
                            *selected = 0;
                            return accept_overlay(ctx, overlay);
                        }
                        's' | 'S' => {
                            *selected = 1;
                            return accept_overlay(ctx, overlay);
                        }
                        'q' | 'Q' => {
                            *selected = 2;
                            return accept_overlay(ctx, overlay);
                        }
                        _ => {}
                    }
                }
                return Vec::new();
            }
            if let Overlay::Ask {
                selected,
                picked,
                draft,
                draft_cursor,
            } = overlay
            {
                if ask_other_active(ctx, *selected, picked) {
                    ask_view::push_draft_char(draft, draft_cursor, c);
                    return Vec::new();
                }
                if let Some(i) = c.to_digit(10) {
                    let i = i as usize;
                    if (1..=picked.len().max(9)).contains(&i) {
                        *selected = i - 1;
                        if ask_other_active(ctx, *selected, picked) {
                            ask_view::clamp_draft_cursor(draft, draft_cursor);
                            return Vec::new();
                        }
                        return accept_overlay(ctx, overlay);
                    }
                }
                return Vec::new();
            }
            if let Overlay::Elicit {
                selected,
                picked,
                draft,
            } = overlay
            {
                if elicit_needs_draft(ctx, *selected, picked) {
                    let mut cur = draft.chars().count();
                    ask_view::push_draft_char(draft, &mut cur, c);
                    return Vec::new();
                }
                if let Some(i) = c.to_digit(10) {
                    let i = i as usize;
                    if (1..=picked.len().max(9)).contains(&i) {
                        *selected = i - 1;
                        if elicit_needs_draft(ctx, *selected, picked) {
                            return Vec::new();
                        }
                        return accept_overlay(ctx, overlay);
                    }
                }
                return Vec::new();
            }
            if let Overlay::Dashboard {
                selected,
                query,
                collapsed,
                ..
            } = overlay
            {
                if c == 'x' || c == 'X' {
                    let rows = crate::views::dashboard::build_rows(ctx, query, collapsed);
                    if let Some(crate::views::dashboard::DashRow::Archived { id, cwd, .. }) =
                        rows.get(*selected)
                    {
                        if let Some(roster) = ctx.get::<cordis_spine::Roster>(cordis_spine::ROSTER)
                        {
                            match roster.remove(id, cwd) {
                                Ok(()) => flash(ctx, "已删除该会话"),
                                Err(e) => flash(ctx, format!("删除失败：{e}")),
                            }
                        }
                        // 删完行数变了，选中项可能落到界外。
                        let len = crate::views::dashboard::build_rows(ctx, query, collapsed).len();
                        *selected = (*selected).min(len.saturating_sub(1));
                    }
                    return Vec::new();
                }
            }
            if let Overlay::Tasks {
                selected,
                query,
                collapsed,
            } = overlay
            {
                if c == 'x' || c == 'X' {
                    let rows = task_entries(ctx, query, collapsed);
                    if let Some(target) = rows.get(*selected).and_then(TaskEntry::kill_target) {
                        kill_task_target(ctx, &target);
                        return Vec::new();
                    }
                }
            }
            // `x` 在 Workflow Runs 里停选中的那条 run（列表和详情页都认）。
            if let Overlay::Workflows {
                selected,
                query,
                detail,
                ..
            } = overlay
            {
                if c == 'x' || c == 'X' {
                    let target = detail.clone().or_else(|| {
                        workflow_rows(ctx, query)
                            .get(*selected)
                            .map(|run| run.run_id.clone())
                    });
                    if let Some(run_id) = target {
                        stop_workflow(ctx, &run_id);
                        return Vec::new();
                    }
                }
            }
            if let Overlay::Mcps {
                selected,
                query,
                tools_expanded,
                section_collapsed,
            } = overlay
            {
                if c == 'i' || c == 'I' {
                    let servers = mcp_status_list(ctx);
                    let rows =
                        mcps::build_rows(&servers, query, tools_expanded, *section_collapsed);
                    if let Some(row) = rows.get(*selected) {
                        if let Some(name) = mcps::auth_target(row, &servers) {
                            return vec![Effect::McpAuth { name }];
                        }
                    }
                    return Vec::new();
                }
            }
            if let Overlay::Computer { pending, .. } = overlay {
                // 两步走：`i` / `p` 只进确认态，Enter 才真的去下载 / 起子进程。
                let picked = match c {
                    'i' | 'I' => Some(CuaAction::Install),
                    'p' | 'P' => Some(CuaAction::Grant),
                    _ => None,
                };
                if let Some(action) = picked {
                    if let Some(computer) = ctx.get::<Computer>(COMPUTER) {
                        if computer.busy() {
                            flash(ctx, "上一个动作还在跑");
                            return Vec::new();
                        }
                        if action == CuaAction::Grant && !computer.grant_available() {
                            flash(ctx, "这台机器上不需要（或还不能）授权：先装好 cua-driver，且只有 macOS 要这一步");
                            return Vec::new();
                        }
                        *pending = Some(action);
                    } else {
                        flash(ctx, "computer 未挂载（tool-computer 插件不在树上）。");
                    }
                }
                return Vec::new();
            }
            if matches!(overlay, Overlay::Browser { .. }) {
                if c == 'h' || c == 'H' {
                    if let Some(browser) = ctx.get::<Browser>(BROWSER) {
                        match browser.toggle_headed() {
                            Ok(true) => flash(
                                ctx,
                                "已切换为有头（需 browser_close 后再 browser_open 才生效）",
                            ),
                            Ok(false) => flash(
                                ctx,
                                "已切换为无头（需 browser_close 后再 browser_open 才生效）",
                            ),
                            Err(e) => flash(ctx, format!("切换显示失败：{e}")),
                        }
                    } else {
                        flash(ctx, "browser 未挂载（tool-browser 插件不在树上）。");
                    }
                    return Vec::new();
                }
                return Vec::new();
            }
            overlay.push_char(c);
            Vec::new()
        }
        Action::OverlayBackspace => {
            if let Overlay::Dashboard {
                focus: crate::views::dashboard::Focus::Composer,
                composer,
                composer_cursor,
                ..
            } = overlay
            {
                crate::views::inspect_overlay::composer_backspace(composer, composer_cursor);
                return Vec::new();
            }
            if let Overlay::Ask {
                selected,
                picked,
                draft,
                draft_cursor,
            } = overlay
            {
                if ask_other_active(ctx, *selected, picked) {
                    ask_view::backspace_draft(draft, draft_cursor);
                    return Vec::new();
                }
            }
            if let Overlay::Elicit {
                selected,
                picked,
                draft,
            } = overlay
            {
                if elicit_needs_draft(ctx, *selected, picked) {
                    draft.pop();
                    return Vec::new();
                }
            }
            overlay.backspace();
            Vec::new()
        }
        Action::OverlayPaste(text) => {
            if let Overlay::Ask {
                selected,
                picked,
                draft,
                draft_cursor,
            } = overlay
            {
                if ask_other_active(ctx, *selected, picked) {
                    ask_view::push_draft(draft, draft_cursor, &text);
                    return Vec::new();
                }
            }
            if let Overlay::Elicit {
                selected,
                picked,
                draft,
            } = overlay
            {
                if elicit_needs_draft(ctx, *selected, picked) {
                    let mut cur = draft.chars().count();
                    ask_view::push_draft(draft, &mut cur, &text);
                    return Vec::new();
                }
            }
            overlay.push_str(&text);
            Vec::new()
        }
        Action::OverlaySelect(idx) => {
            overlay.set_selected(idx);
            Vec::new()
        }
        Action::OverlaySpace => {
            if matches!(
                overlay,
                Overlay::Inspect {
                    target: InspectTarget::Subagent(_),
                    ..
                }
            ) {
                overlay.push_char(' ');
                return Vec::new();
            }
            if matches!(
                overlay,
                Overlay::Presets(PresetView::Canvas(s)) if s.naming_role.is_some()
            ) {
                overlay.push_char(' ');
                return Vec::new();
            }
            if matches!(
                overlay,
                Overlay::Presets(PresetView::Canvas(s))
                    if !s.editing_persona
                        && s.naming_role.is_none()
                        && s.pane == preset_overlay::PresetPane::Catalog
            ) {
                overlay.push_char(' ');
                return Vec::new();
            }
            if let Overlay::Ask {
                selected,
                picked,
                draft,
                draft_cursor,
            } = overlay
            {
                if ask_other_active(ctx, *selected, picked) {
                    if let Some(slot) = picked.get_mut(*selected) {
                        *slot = true;
                    }
                    ask_view::push_draft_char(draft, draft_cursor, ' ');
                    return Vec::new();
                }
                if let Some(slot) = picked.get_mut(*selected) {
                    *slot = !*slot;
                }
                return Vec::new();
            }
            if let Overlay::Elicit {
                selected,
                picked,
                draft,
            } = overlay
            {
                if elicit_needs_draft(ctx, *selected, picked) {
                    if let Some(slot) = picked.get_mut(*selected) {
                        *slot = true;
                    }
                    let mut cur = draft.chars().count();
                    ask_view::push_draft_char(draft, &mut cur, ' ');
                    return Vec::new();
                }
                if let Some(slot) = picked.get_mut(*selected) {
                    *slot = !*slot;
                }
                return Vec::new();
            }
            if let Overlay::Settings {
                selected,
                picking: None,
            } = overlay
            {
                let fields = settings_modal::field_rows();
                if let Some(field) = fields.get(*selected).copied() {
                    if field.is_bool() {
                        if let Some(settings) = ctx.get::<AppSettings>(SETTINGS) {
                            settings_modal::toggle_bool(field, &settings);
                        }
                    }
                }
            }
            if let Overlay::Mcps {
                selected,
                query,
                tools_expanded,
                section_collapsed,
            } = overlay
            {
                let servers = mcp_status_list(ctx);
                let rows = mcps::build_rows(&servers, query, tools_expanded, *section_collapsed);
                if let Some(row) = rows.get(*selected) {
                    if let Some(toggle) = mcps::toggle_target(row, &servers) {
                        return vec![match toggle {
                            mcps::McpToggle::Server { name, enabled } => {
                                Effect::ToggleMcpServer { name, enabled }
                            }
                            mcps::McpToggle::Tool {
                                server,
                                tool,
                                enabled,
                            } => Effect::ToggleMcpTool {
                                server,
                                tool,
                                enabled,
                            },
                        }];
                    }
                }
            }
            Vec::new()
        }
        Action::OverlayRefresh => {
            if matches!(overlay, Overlay::Mcps { .. }) {
                return vec![Effect::ReloadMcps { quiet: false }];
            }
            // 驾驶舱的 Ctrl+R 是「重新看一眼本机」：装没装 / 授权给没给，
            // 再把 MCP 配置对一次账（在 dock 外面装好的 driver 就是这样接上的）。
            if matches!(overlay, Overlay::Computer { .. }) {
                return vec![Effect::CuaRefresh, Effect::ReloadMcps { quiet: false }];
            }
            Vec::new()
        }
        Action::OverlayNavH(delta) => {
            if let Overlay::Ask {
                selected,
                picked,
                draft,
                draft_cursor,
            } = overlay
            {
                if ask_other_active(ctx, *selected, picked) {
                    ask_view::move_draft_cursor(draft, draft_cursor, delta);
                    return Vec::new();
                }
            }
            if matches!(overlay, Overlay::Ask { .. }) {
                navigate_ask(ctx, overlay, delta);
            }
            Vec::new()
        }
        Action::OverlayTab => {
            if let Overlay::Dashboard { focus, .. } = overlay {
                *focus = match focus {
                    crate::views::dashboard::Focus::List => {
                        crate::views::dashboard::Focus::Composer
                    }
                    crate::views::dashboard::Focus::Composer => {
                        crate::views::dashboard::Focus::List
                    }
                };
                return Vec::new();
            }
            if matches!(
                overlay,
                Overlay::Presets(PresetView::Canvas(s)) if s.naming_role.is_some()
            ) {
                return accept_overlay(ctx, overlay);
            }
            if let Overlay::Presets(view) = overlay {
                view.cycle_pane();
            }
            if let Overlay::Usage {
                tab,
                scroll,
                detail,
            } = overlay
            {
                *tab = tab.next();
                *detail = None;
                *scroll = 0;
            }
            Vec::new()
        }
        Action::MouseMove { column, row } => {
            if matches!(overlay, Overlay::Usage { .. }) {
                usage_overlay::hover(column, row);
            }
            // 顶栏右段的悬停态：停上去把占用率换成进度条。覆盖层开着时也更新，
            // 这样关掉覆盖层不会留下一个卡住的悬停态。
            if let Ok(status) = ctx.require::<StatusLine>(TUI_STATUS) {
                status.set_mouse(column, row);
            }
            if welcome_open(ctx) && !overlay.is_open() {
                if let Ok(welcome) = ctx.require::<Welcome>(TUI_WELCOME) {
                    welcome.set_mouse(column, row);
                }
                return Vec::new();
            }
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                // Bare Moved with an active drag: treat as lost Up (usage_modal).
                if let Some(text) = scrollback.finish_lost_drag() {
                    copy_out(ctx, &text, None);
                }
                scrollback.set_mouse(column, row);
            }
            Vec::new()
        }
        Action::MouseDown { column, row } => {
            // 按在标签栏上不要起选区：切页在 MouseUp 那边做。
            if tab_bar::hit(tab_hits, column, row).is_some() {
                return Vec::new();
            }
            if queue_pane::hit(queue_hits, column, row).is_some() {
                return Vec::new();
            }
            if goal_pane::hit(goal_hits, column, row).is_some() {
                return Vec::new();
            }
            if open_context_from_status(ctx, overlay, column, row) {
                return Vec::new();
            }
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                scrollback.mouse_down(column, row);
            }
            Vec::new()
        }
        Action::MouseDrag { column, row } => {
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                scrollback.mouse_drag(column, row);
            }
            Vec::new()
        }
        Action::MouseUp { column, row } => {
            if let Some(tab_bar::TabHit(id)) = tab_bar::hit(tab_hits, column, row) {
                return vec![Effect::TabGo { id }];
            }
            if let Some(hit) = queue_pane::hit(queue_hits, column, row) {
                return match hit {
                    QueueHit::Send(id) => vec![Effect::PromoteQueued { id: Some(id) }],
                    QueueHit::Edit(id) => vec![Effect::EditQueued { id: Some(id) }],
                };
            }
            if let Some(hit) = goal_pane::hit(goal_hits, column, row) {
                return apply_goal_hit(ctx, overlay, hit);
            }
            if let Some(target) = task_dock::hit(dock_hits, column, row) {
                *overlay = match target {
                    TaskDockHit::Subagent(id) => Overlay::inspect_subagent(id, false),
                    TaskDockHit::Job(id) => Overlay::inspect_job(id, false),
                    // workflow / 定时任务没有单独的全屏视图，退回整张表。
                    TaskDockHit::OpenTasks => Overlay::Tasks {
                        selected: 0,
                        query: String::new(),
                        collapsed: HashSet::new(),
                    },
                };
                return Vec::new();
            }
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                match scrollback.mouse_up(column, row) {
                    MouseUpResult::Copied(text) => {
                        copy_out(ctx, &text, None);
                    }
                    MouseUpResult::Click(ClickHit::Mermaid { kind, source }) => {
                        return mermaid_click(ctx, kind, source);
                    }
                    MouseUpResult::Click(ClickHit::OpenSubagent { id }) => {
                        *overlay = Overlay::inspect_subagent(id, false);
                    }
                    MouseUpResult::Click(ClickHit::OpenJob { id }) => {
                        *overlay = Overlay::inspect_job(id, false);
                    }
                    MouseUpResult::Click(ClickHit::ToolToggle | ClickHit::None)
                    | MouseUpResult::Redraw
                    | MouseUpResult::None => {}
                }
            }
            Vec::new()
        }
        Action::PasteClipboard => {
            apply_paste(ctx, overlay, crate::app::clipboard::paste_from_clipboard());
            Vec::new()
        }
        Action::Scroll(delta) => {
            if let Overlay::PlanApproval {
                scroll,
                prompt,
                wrap,
                ..
            } = overlay
            {
                scroll_plan_body(scroll, prompt, wrap, delta);
                return Vec::new();
            }
            if let Overlay::Usage {
                tab,
                scroll,
                detail,
            } = overlay
            {
                scroll_usage(*tab, *detail, scroll, delta);
                return Vec::new();
            }
            if let Overlay::Notice { body, scroll, .. } = overlay {
                scroll_text(scroll, delta, body);
                return Vec::new();
            }
            if let Overlay::Browser { scroll } = overlay {
                let body = browser_cockpit_body(ctx);
                scroll_text(scroll, delta, &body);
                return Vec::new();
            }
            if let Overlay::Computer { scroll, pending } = overlay {
                let body = computer_cockpit_body(ctx, *pending);
                scroll_text(scroll, delta, &body);
                return Vec::new();
            }
            if let Overlay::Inspect { target, scroll, .. } = overlay {
                scroll_inspect(ctx, target, scroll, delta);
                return Vec::new();
            }
            if overlay.is_open() {
                overlay.move_sel(if delta > 0 { -1 } else { 1 }, overlay_len(ctx, overlay));
                return Vec::new();
            }
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                scrollback.scroll(delta);
            }
            Vec::new()
        }
        Action::ScrollPage(pages) => {
            if let Overlay::Inspect { target, scroll, .. } = overlay {
                scroll_inspect(ctx, target, scroll, pages.saturating_mul(8));
                return Vec::new();
            }
            if let Overlay::PlanApproval {
                scroll,
                prompt,
                wrap,
                ..
            } = overlay
            {
                scroll_plan_body(scroll, prompt, wrap, pages.saturating_mul(8));
                return Vec::new();
            }
            if overlay.is_open() {
                return Vec::new();
            }
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                scrollback.scroll_page(pages);
            }
            Vec::new()
        }
        Action::Click { column, row } => {
            // 标签栏先判：欢迎屏 / overlay 开着时鼠标走的是这条路（不是 MouseUp），
            // 而新开的分页一定停在欢迎屏上——不先判这里，点标签就是死的。
            if let Some(tab_bar::TabHit(id)) = tab_bar::hit(tab_hits, column, row) {
                return vec![Effect::TabGo { id }];
            }
            if let Overlay::Inspect { .. } = overlay {
                match inspect_overlay::click(column, row) {
                    InspectClick::Close => return close_inspect(overlay),
                    InspectClick::Toggle | InspectClick::None => return Vec::new(),
                }
            }
            if let Some(hit) = goal_pane::hit(goal_hits, column, row) {
                return apply_goal_hit(ctx, overlay, hit);
            }
            if let Some(target) = task_dock::hit(dock_hits, column, row) {
                *overlay = match target {
                    TaskDockHit::Subagent(id) => Overlay::inspect_subagent(id, false),
                    TaskDockHit::Job(id) => Overlay::inspect_job(id, false),
                    // workflow / 定时任务没有单独的全屏视图，退回整张表。
                    TaskDockHit::OpenTasks => Overlay::Tasks {
                        selected: 0,
                        query: String::new(),
                        collapsed: HashSet::new(),
                    },
                };
                return Vec::new();
            }
            if overlay.is_open() {
                if overlay::hit_close(hits, column, row) {
                    if matches!(overlay, Overlay::Permission { .. }) {
                        if let Some(perms) = ctx.get::<Permissions>(PERMISSIONS) {
                            perms.resolve(PermissionOptionKind::RejectOnce);
                        }
                    }
                    if matches!(overlay, Overlay::PairingPending { .. }) {
                        if let Some(id) = ctx
                            .get::<crate::seam::gateway::GatewayRef>(crate::names::GATEWAY)
                            .and_then(|g| g.pairing_front().map(|p| p.id))
                        {
                            let _ = ctx
                                .get::<crate::seam::gateway::GatewayRef>(crate::names::GATEWAY)
                                .and_then(|g| g.pairing_deny(&id).ok());
                        }
                    }
                    if matches!(overlay, Overlay::Ask { .. }) {
                        if let Some(ask) = ctx.get::<Ask>(ASK) {
                            ask.cancel();
                        }
                    }
                    if matches!(overlay, Overlay::Elicit { .. }) {
                        if let Some(mcp) = ctx.get::<Mcp>(MCP) {
                            mcp.elicitation().cancel();
                        }
                    }
                    if let Overlay::PlanApproval {
                        view_only: false, ..
                    } = overlay
                    {
                        if let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) {
                            plan.resolve(PlanDecision::Quit);
                            flash(ctx, "已放弃计划");
                        }
                    }
                    if let Overlay::Presets(view) = overlay {
                        if !preset_overlay::on_esc(ctx, view) {
                            return Vec::new();
                        }
                    }
                    overlay.close();
                    return Vec::new();
                }
                if let Overlay::Usage {
                    tab,
                    scroll,
                    detail,
                } = overlay
                {
                    if let Some(next) = usage_overlay::hit_tab(column, row) {
                        *tab = next;
                        *detail = None;
                        *scroll = 0;
                        return Vec::new();
                    }
                    if *tab == UsageTab::Context {
                        if usage_overlay::hit_back(column, row) {
                            *detail = None;
                            *scroll = 0;
                            return Vec::new();
                        }
                        if let Some(kind) = usage_overlay::hit_slice(column, row) {
                            *detail = Some(kind);
                            *scroll = 0;
                            return Vec::new();
                        }
                    }
                }
                if let Some(target) = overlay::hit_kill(hits, column, row) {
                    if matches!(overlay, Overlay::Tasks { .. }) {
                        kill_task_target(ctx, &target);
                        return Vec::new();
                    }
                }
                if let Some(idx) = overlay::hit_index(hits, column, row) {
                    // 详情页里那些行是阶段，点它只换阶段，不是选中一个 run。
                    if let Overlay::Workflows {
                        detail: Some(_),
                        phase,
                        ..
                    } = overlay
                    {
                        *phase = idx;
                        return Vec::new();
                    }
                    // 列表里点一行就进详情：以前在 `/tasks` 点 workflow 行看进展，
                    // 详情已经挪到 `/workflow`，点击语义跟着走。
                    if let Overlay::Workflows {
                        selected,
                        query,
                        detail,
                        phase,
                    } = overlay
                    {
                        *selected = idx;
                        if let Some(run) = workflow_rows(ctx, query).get(idx) {
                            *detail = Some(run.run_id.clone());
                            *phase = 0;
                        }
                        return Vec::new();
                    }
                    if matches!(overlay, Overlay::Presets(PresetView::Canvas(_))) {
                        let action = match overlay {
                            Overlay::Presets(view) => preset_overlay::apply_hit(ctx, view, idx),
                            _ => PresetAction::None,
                        };
                        apply_preset_action(ctx, overlay, action);
                        return Vec::new();
                    }
                    overlay.set_selected(idx);
                    if let Overlay::Ask {
                        selected, picked, ..
                    } = overlay
                    {
                        if ask_other_active(ctx, *selected, picked) {
                            return Vec::new();
                        }
                    }
                    if let Overlay::Elicit {
                        selected, picked, ..
                    } = overlay
                    {
                        if elicit_needs_draft(ctx, *selected, picked) {
                            return Vec::new();
                        }
                    }
                    if !matches!(overlay, Overlay::Settings { .. }) {
                        return accept_overlay(ctx, overlay);
                    }
                }
                return Vec::new();
            }
            if open_context_from_status(ctx, overlay, column, row) {
                return Vec::new();
            }
            if welcome_open(ctx) {
                if let Ok(welcome) = ctx.require::<Welcome>(TUI_WELCOME) {
                    if let Some(hit) = welcome.hit(column, row) {
                        return match hit {
                            WelcomeHit::NewSession => vec![Effect::NewSession],
                            WelcomeHit::Resume => vec![Effect::ResumePicker],
                            WelcomeHit::Quit => vec![Effect::Quit],
                        };
                    }
                }
                return Vec::new();
            }
            // Scrollback clicks go through MouseDown/Up (drag-select).
            Vec::new()
        }
        Action::InsertText(text) => {
            apply_paste(ctx, overlay, crate::app::clipboard::paste_from_event(&text));
            Vec::new()
        }
        Action::CycleMode => {
            let plan = ctx.get::<PlanMode>(PLAN_MODE);
            let settings = ctx.get::<AppSettings>(SETTINGS);
            if plan.as_ref().is_some_and(|p| p.awaiting_approval()) {
                flash(ctx, "请先批准或放弃当前计划");
                return Vec::new();
            }
            let in_plan = plan.as_ref().is_some_and(|p| p.active());
            let perm = settings
                .as_ref()
                .map(|s| s.permission_mode())
                .unwrap_or(PermissionMode::Ask);
            let (next_plan, next_perm, msg) = mode_cycle::cycle_session_mode(in_plan, perm);
            if let Some(plan) = plan.as_ref() {
                if next_plan {
                    plan.enter_pending();
                } else {
                    plan.exit_now();
                }
            }
            if let Some(settings) = settings.as_ref() {
                settings.set_permission_mode(next_perm);
            }
            flash(ctx, msg);
            Vec::new()
        }
        Action::ToggleGoalDetail => {
            if matches!(overlay, Overlay::Goal { .. }) {
                overlay.close();
            } else {
                open_goal_overlay(ctx, overlay, false);
            }
            Vec::new()
        }
        Action::ToggleTodoFold => {
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                if scrollback.has_todos() {
                    scrollback.cycle_todo_fold();
                } else {
                    flash(ctx, "当前没有待办列表");
                }
            }
            Vec::new()
        }
        other => {
            let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) else {
                return Vec::new();
            };
            dispatch(other, &prompt)
        }
    }
}

pub(super) fn accept_overlay(ctx: &Context, overlay: &mut Overlay) -> Vec<Effect> {
    let elicit_answer = match overlay {
        Overlay::Elicit {
            selected,
            picked,
            draft,
        } => Some((*selected, picked.clone(), draft.clone())),
        _ => None,
    };
    if let Some((selected, picked, draft)) = elicit_answer {
        accept_elicit(ctx, overlay, selected, picked, &draft);
        return Vec::new();
    }
    let ask_answer = match overlay {
        Overlay::Ask {
            selected,
            picked,
            draft,
            ..
        } => Some((*selected, picked.clone(), draft.clone())),
        _ => None,
    };
    if let Some((selected, picked, draft)) = ask_answer {
        accept_ask(ctx, overlay, selected, picked, &draft);
        return Vec::new();
    }
    if matches!(overlay, Overlay::Presets(_)) {
        let action = match overlay {
            Overlay::Presets(view) => preset_overlay::accept(ctx, view),
            _ => PresetAction::None,
        };
        apply_preset_action(ctx, overlay, action);
        return Vec::new();
    }
    match overlay {
        Overlay::Settings { .. } => {
            accept_settings(ctx, overlay);
            return Vec::new();
        }
        Overlay::Permission { selected } => {
            if let Some(perms) = ctx.get::<Permissions>(PERMISSIONS) {
                perms.resolve(permission_view::kind_at(*selected));
            }
            overlay.close();
            return Vec::new();
        }
        Overlay::PairingPending { selected } => {
            if let Some(id) = ctx
                .get::<crate::seam::gateway::GatewayRef>(crate::names::GATEWAY)
                .and_then(|g| g.pairing_front().map(|p| p.id))
            {
                if let Some(gw) = ctx.get::<crate::seam::gateway::GatewayRef>(crate::names::GATEWAY)
                {
                    if *selected == 0 {
                        let _ = gw.pairing_confirm(&id);
                    } else {
                        let _ = gw.pairing_deny(&id);
                    }
                }
            }
            overlay.close();
            return Vec::new();
        }
        Overlay::PairingManage { .. } => {
            if let Some(ui) = ctx.get::<crate::views::pairing::PairingUi>(crate::names::TUI_PAIRING)
            {
                crate::views::pairing::accept_manage(&ui, overlay);
            }
            return Vec::new();
        }
        Overlay::PlanApproval {
            selected,
            view_only,
            ..
        } => {
            let decision = plan_approval_view::decision_at(*selected, *view_only);
            if *view_only {
                overlay.close();
                return Vec::new();
            }
            if let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) {
                plan.resolve(decision);
                let msg = match decision {
                    PlanDecision::Approve => "已批准计划",
                    PlanDecision::Revise => "请继续修订计划",
                    PlanDecision::Quit => "已放弃计划",
                };
                flash(ctx, msg);
            }
            overlay.close();
            return Vec::new();
        }
        Overlay::Inspect { .. } => return Vec::new(),
        Overlay::Tasks {
            selected,
            query,
            collapsed,
        } => {
            let rows = task_entries(ctx, query, collapsed);
            if let Some(group) = rows.get(*selected).and_then(|e| e.header_group()) {
                if !collapsed.remove(&group) {
                    collapsed.insert(group);
                }
                return Vec::new();
            }
            if let Some(TaskEntry::Agent { subagent_id, .. }) = rows.get(*selected) {
                *overlay = Overlay::inspect_subagent(subagent_id.clone(), true);
                return Vec::new();
            }
            if let Some(TaskEntry::BgTask { task_id, .. }) = rows.get(*selected) {
                *overlay = Overlay::inspect_job(task_id.clone(), true);
                return Vec::new();
            }
            // workflow 行在这里不展开：`/tasks` 是五类任务的总表，详情归
            // `/workflow`。这里点开会把两个面板的职责搅在一起。
            return Vec::new();
        }
        Overlay::Mcps {
            selected,
            query,
            tools_expanded,
            section_collapsed,
        } => {
            let servers = mcp_status_list(ctx);
            let rows = mcps::build_rows(&servers, query, tools_expanded, *section_collapsed);
            if let Some(row) = rows.get(*selected) {
                mcps::toggle_row(row, tools_expanded, section_collapsed, !query.is_empty());
                let next = mcps::build_rows(&servers, query, tools_expanded, *section_collapsed);
                *selected = if next.is_empty() {
                    0
                } else {
                    (*selected).min(next.len() - 1)
                };
            }
            return Vec::new();
        }
        Overlay::Workflows { detail, phase, .. } => {
            // Esc 在详情页是返回列表，不是关掉整个 overlay。
            if detail.take().is_some() {
                *phase = 0;
            } else {
                overlay.close();
            }
            return Vec::new();
        }
        Overlay::Goal {
            selected,
            editing,
            draft,
        } => {
            if *editing {
                if let Some(goal) = ctx.get::<Goal>(GOAL) {
                    goal.set_title(draft.clone());
                    flash(ctx, "已更新标题");
                }
                *editing = false;
                return Vec::new();
            }
            match *selected {
                goal_overlay::ACTION_EDIT => {
                    *editing = true;
                    *draft = ctx.get::<Goal>(GOAL).map(|g| g.title()).unwrap_or_default();
                }
                goal_overlay::ACTION_PAUSE => return goal_pause_resume(ctx),
                goal_overlay::ACTION_CLEAR => {
                    if let Some(goal) = ctx.get::<Goal>(GOAL) {
                        goal.clear();
                    }
                    overlay.close();
                    flash(ctx, "已清除目标");
                }
                _ => {}
            }
            return Vec::new();
        }
        _ => {}
    }
    // 焦点在 peek 输入框时，Enter 是**把消息发给选中的那个会话**——面板存在的
    // 理由就是这个：不切页、不离开面板，轮流给几个会话派活。
    if let Overlay::Dashboard {
        selected,
        query,
        collapsed,
        focus: crate::views::dashboard::Focus::Composer,
        composer,
        composer_cursor,
    } = overlay
    {
        let rows = crate::views::dashboard::build_rows(ctx, query, collapsed);
        let Some(row) = rows.get(*selected) else {
            return Vec::new();
        };
        match crate::views::dashboard::submit_to(ctx, row, composer) {
            Ok(()) => {
                composer.clear();
                *composer_cursor = 0;
            }
            Err(msg) => flash(ctx, msg),
        }
        return Vec::new();
    }
    // 焦点在列表时按行类型分：抬头是折叠开关，要就地改 overlay 而不是发 Effect，
    // 所以单独在这里处理完直接返回。
    if let Overlay::Dashboard {
        selected,
        query,
        collapsed,
        ..
    } = overlay
    {
        let rows = crate::views::dashboard::build_rows(ctx, query, collapsed);
        let Some(row) = rows.get(*selected).cloned() else {
            return Vec::new();
        };
        let resumable = row.resumable();
        match row {
            crate::views::dashboard::DashRow::Header { state, .. } => {
                if !collapsed.remove(&state) {
                    collapsed.insert(state);
                }
                // 折叠后行数变了，选中项可能落到界外。
                let len = crate::views::dashboard::build_rows(ctx, query, collapsed).len();
                *selected = (*selected).min(len.saturating_sub(1));
                return Vec::new();
            }
            crate::views::dashboard::DashRow::Tab { id, .. } => {
                overlay.close();
                return vec![Effect::TabGo { id }];
            }
            crate::views::dashboard::DashRow::Archived { id, cwd, .. } => {
                if !resumable {
                    // 不在当前工作目录下的会话恢复不了：`Sessions::restore` 只认
                    // `archived()`，那份列表是 `load_cwd(当前 cwd)` 填的。与其
                    // 按下去毫无反应，不如说清楚。
                    flash(
                        ctx,
                        format!("该会话属于 {}，先 /cd 过去再恢复", cwd.display()),
                    );
                    return Vec::new();
                }
                overlay.close();
                return vec![Effect::RestoreSession(id)];
            }
        }
    }
    let effect = match overlay {
        Overlay::Resume { selected, query } => ctx.get::<Sessions>(SESSIONS).and_then(|s| {
            filter_sessions(&s.archived(), query)
                .get(*selected)
                .map(|item| Effect::RestoreSession(item.id.clone()))
        }),
        Overlay::Help { selected, query } => {
            let extras = slash_extras(ctx);
            overlay::filter_help_items(query, &extras)
                .get(*selected)
                .cloned()
                .and_then(|item| match item {
                    HelpItem::Builtin(row) => match row.kind {
                        HelpKind::Slash(cmd) => {
                            Some(crate::app::actions::effect_for_slash(cmd, ""))
                        }
                        HelpKind::Hint => None,
                    },
                    HelpItem::Extra { command, .. } => extras
                        .iter()
                        .find(|e| e.command == command)
                        .map(|e| crate::app::actions::effect_for_extra(e, "")),
                })
        }
        Overlay::History { selected, query } => ctx.get::<PromptWidget>(TUI_PROMPT).and_then(|p| {
            filter_strings(&p.history(), query)
                .get(*selected)
                .map(|(_, text)| Effect::InsertHistory((*text).to_string()))
        }),
        Overlay::Find { selected, query } => ctx.get::<Scrollback>(TUI_SCROLLBACK).and_then(|s| {
            s.find(query)
                .get(*selected)
                .map(|(idx, _)| Effect::JumpToLine(*idx))
        }),
        Overlay::Args {
            kind,
            cmd,
            selected,
            query,
        } => {
            let items = filter_args(*kind, query, ctx.get::<AppSettings>(SETTINGS).as_deref());
            items.get(*selected).map(|item| match kind {
                crate::slash::ArgKind::LoopInterval => {
                    Effect::InsertHistory(format!("/loop {} ", item.insert_text))
                }
                _ => crate::app::actions::effect_for_slash(*cmd, &item.insert_text),
            })
        }
        Overlay::None
        | Overlay::Settings { .. }
        | Overlay::Permission { .. }
        | Overlay::PairingPending { .. }
        | Overlay::PairingManage { .. }
        | Overlay::Ask { .. }
        | Overlay::Elicit { .. }
        | Overlay::PlanApproval { .. }
        | Overlay::Goal { .. }
        | Overlay::Usage { .. }
        | Overlay::Notice { .. }
        | Overlay::Slot { .. }
        | Overlay::Browser { .. }
        | Overlay::Computer { .. }
        | Overlay::Inspect { .. }
        | Overlay::Presets(_) => None,
        Overlay::Tasks { .. } | Overlay::Mcps { .. } | Overlay::Workflows { .. } => None,
        // 上面已经按行类型处理完并返回了，走不到这里。
        Overlay::Dashboard { .. } => None,
    };
    overlay.close();
    effect.into_iter().collect()
}

pub(super) fn accept_settings(ctx: &Context, overlay: &mut Overlay) {
    let Overlay::Settings { selected, picking } = overlay else {
        return;
    };
    let Some(settings) = ctx.get::<AppSettings>(SETTINGS) else {
        return;
    };
    if let Some(field) = *picking {
        if let Some((value, _)) = settings_modal::enum_choices(field, &settings).get(*selected) {
            settings_modal::apply_choice(field, value, &settings);
        }
        *picking = None;
        *selected = settings_modal::field_rows()
            .iter()
            .position(|f| *f == field)
            .unwrap_or(0);
        return;
    }
    let fields = settings_modal::field_rows();
    let Some(field) = fields.get(*selected).copied() else {
        return;
    };
    if field.is_bool() {
        settings_modal::toggle_bool(field, &settings);
    } else {
        *picking = Some(field);
        *selected = 0;
    }
}

pub(super) fn to_action(
    ctx: &Context,
    event: Event,
    overlay: &Overlay,
    esc_suppress_until: Option<Instant>,
) -> Option<Action> {
    match event {
        Event::Key(key) => {
            if key.kind == KeyEventKind::Release {
                return None;
            }
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            let alt = key.modifiers.contains(KeyModifiers::ALT);
            let super_key = key.modifiers.contains(KeyModifiers::SUPER);
            if matches!(key.code, KeyCode::Char('c' | '\x03')) && ctrl
                || key.code == KeyCode::Char('\x03')
            {
                if turn_running(ctx) {
                    return Some(Action::CancelTurn);
                }
                return Some(Action::Quit);
            }
            if ctrl && matches!(key.code, KeyCode::Char('q')) {
                return Some(Action::Quit);
            }
            if is_paste_key(key.code, ctrl, super_key) {
                return Some(Action::PasteClipboard);
            }
            if overlay.is_open() {
                return overlay_keys(key.code, ctrl);
            }
            // Alt+1..9 直达标签上那个号的页。号是稳定的，关掉中间一页也不会串。
            if alt {
                if let KeyCode::Char(c @ '1'..='9') = key.code {
                    return Some(Action::TabGo(c as usize - '0' as usize));
                }
            }
            match key.code {
                KeyCode::Char('w') if ctrl => Some(Action::NewSession),
                KeyCode::Char('n') if ctrl => Some(Action::TabNew),
                KeyCode::Char('f') if ctrl => Some(Action::TabFork),
                KeyCode::Char('b') if ctrl => Some(Action::TabCarryBack),
                KeyCode::Char('k') if ctrl => Some(Action::Scroll(1)),
                KeyCode::Char('j') if ctrl => Some(Action::Scroll(-1)),
                KeyCode::Char('u') if ctrl => Some(Action::ScrollPage(1)),
                KeyCode::Char('d') if ctrl => Some(Action::ScrollPage(-1)),
                KeyCode::Char('.') if ctrl => Some(Action::Help),
                KeyCode::Char('x') if ctrl => Some(Action::Help),
                KeyCode::Char('a') if ctrl => Some(Action::MoveBufferStart),
                KeyCode::Char('e') if ctrl => Some(Action::MoveBufferEnd),
                KeyCode::Char('t') if ctrl => Some(Action::ToggleTodoFold),
                KeyCode::F(2) => Some(Action::SettingsModal),
                KeyCode::F(3) => Some(Action::ResumePicker),
                KeyCode::Esc => {
                    if !overlay.is_open()
                        && esc_suppress_until.is_some_and(|until| Instant::now() < until)
                    {
                        return None;
                    }
                    if turn_running(ctx) {
                        if !prompt_can_send(ctx) && has_prompt_queue(ctx) {
                            return Some(Action::EditQueued { id: None });
                        }
                        return Some(Action::CancelTurn);
                    }
                    if file_search_open(ctx) {
                        return Some(Action::FileSearchDismiss);
                    }
                    if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                        if scrollback.has_selection() {
                            scrollback.clear_selection();
                            // Need a draw; Scroll(0) is a no-op that still redraws.
                            return Some(Action::Scroll(0));
                        }
                    }
                    if let Some(prompt) = ctx.get::<PromptWidget>(TUI_PROMPT) {
                        if !prompt.text().is_empty() {
                            prompt.clear();
                            if let Some(goal) = ctx.get::<Goal>(GOAL) {
                                goal.disarm_composer();
                            }
                            return None;
                        }
                    }
                    // Grok swallows idle-empty Esc (welcome / no turns). Quit is ctrl+q.
                    None
                }
                KeyCode::Enter if shift || alt => Some(Action::InsertChar('\n')),
                KeyCode::Enter if ctrl => {
                    if !prompt_can_send(ctx) && turn_running(ctx) && has_prompt_queue(ctx) {
                        Some(Action::PromoteQueued { id: None })
                    } else {
                        Some(take_send(ctx, true))
                    }
                }
                KeyCode::Enter => {
                    if slash_captures_enter(ctx) {
                        // Command-name picker: put `/cmd ` in the composer.
                        // Argument picker: 选过行就填那一行，没选过照旧发。
                        return Some(Action::SlashAccept);
                    }
                    if file_search_open(ctx) {
                        return Some(Action::FileSearchAccept);
                    }
                    if !prompt_can_send(ctx) && turn_running(ctx) && has_prompt_queue(ctx) {
                        return Some(Action::PromoteQueued { id: None });
                    }
                    Some(take_send(ctx, false))
                }
                KeyCode::BackTab => Some(Action::CycleMode),
                KeyCode::Tab if shift => Some(Action::CycleMode),
                KeyCode::Tab => {
                    if slash_open(ctx) {
                        Some(Action::SlashInsert)
                    } else if file_search_open(ctx) {
                        Some(Action::FileSearchAccept)
                    } else {
                        None
                    }
                }
                KeyCode::Up => {
                    if slash_open(ctx) {
                        Some(Action::SlashMove(-1))
                    } else if file_search_open(ctx) {
                        Some(Action::FileSearchMove(-1))
                    } else {
                        Some(Action::HistoryPrev)
                    }
                }
                KeyCode::Down => {
                    if slash_open(ctx) {
                        Some(Action::SlashMove(1))
                    } else if file_search_open(ctx) {
                        Some(Action::FileSearchMove(1))
                    } else {
                        Some(Action::HistoryNext)
                    }
                }
                KeyCode::Left => Some(Action::MoveLeft),
                KeyCode::Right => Some(Action::MoveRight),
                KeyCode::Home => Some(Action::MoveHome),
                KeyCode::End => Some(Action::MoveEnd),
                KeyCode::Delete => Some(Action::Delete),
                KeyCode::PageUp => Some(Action::ScrollPage(1)),
                KeyCode::PageDown => Some(Action::ScrollPage(-1)),
                KeyCode::Char('g')
                    if !ctrl
                        && !turn_running(ctx)
                        && ctx
                            .get::<PromptWidget>(TUI_PROMPT)
                            .is_some_and(|p| p.text().is_empty())
                        && ctx.get::<Goal>(GOAL).is_some_and(|g| g.present()) =>
                {
                    Some(Action::ToggleGoalDetail)
                }
                KeyCode::Char(c) if !ctrl => Some(Action::InsertChar(c)),
                KeyCode::Backspace => Some(Action::Backspace),
                _ => None,
            }
        }
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::ScrollUp => Some(Action::Scroll(3)),
            MouseEventKind::ScrollDown => Some(Action::Scroll(-3)),
            MouseEventKind::Moved => Some(Action::MouseMove {
                column: mouse.column,
                row: mouse.row,
            }),
            MouseEventKind::Down(MouseButton::Left) => {
                // Overlay / welcome still click on Down; scrollback needs
                // Down→Drag→Up for in-app text selection (alt-screen steals
                // the terminal's native drag-copy).
                if overlay.is_open() || welcome_open(ctx) {
                    Some(Action::Click {
                        column: mouse.column,
                        row: mouse.row,
                    })
                } else {
                    Some(Action::MouseDown {
                        column: mouse.column,
                        row: mouse.row,
                    })
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if overlay.is_open() || welcome_open(ctx) {
                    None
                } else {
                    Some(Action::MouseDrag {
                        column: mouse.column,
                        row: mouse.row,
                    })
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if overlay.is_open() || welcome_open(ctx) {
                    None
                } else {
                    Some(Action::MouseUp {
                        column: mouse.column,
                        row: mouse.row,
                    })
                }
            }
            _ => None,
        },
        Event::Paste(text) => {
            if overlay.is_open() {
                Some(Action::OverlayPaste(text))
            } else {
                Some(Action::InsertText(text))
            }
        }
        _ => None,
    }
}

pub(super) fn is_paste_key(code: KeyCode, ctrl: bool, super_key: bool) -> bool {
    matches!(code, KeyCode::Char('v') | KeyCode::Char('V')) && (ctrl || super_key)
}

pub(super) fn take_send(ctx: &Context, send_now: bool) -> Action {
    let Some(prompt) = ctx.get::<PromptWidget>(TUI_PROMPT) else {
        return if send_now {
            Action::SendPromptNow {
                text: String::new(),
            }
        } else {
            Action::SendPrompt(String::new())
        };
    };
    let taken = prompt.take_prompt();
    if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
        sessions.queue_user_images(
            taken
                .images
                .into_iter()
                .map(|img| UserImage {
                    mime: img.mime,
                    data: img.data,
                    width: img.width,
                    height: img.height,
                })
                .collect(),
        );
    }
    if send_now {
        Action::SendPromptNow { text: taken.text }
    } else {
        Action::SendPrompt(taken.text)
    }
}

pub(super) fn apply_paste(
    ctx: &Context,
    overlay: &mut Overlay,
    payload: crate::app::clipboard::PastePayload,
) {
    if overlay.is_open() {
        if let crate::app::clipboard::PastePayload::Text(text) = payload {
            if let Overlay::Ask {
                selected,
                picked,
                draft,
                draft_cursor,
            } = overlay
            {
                if ask_other_active(ctx, *selected, picked) {
                    ask_view::push_draft(draft, draft_cursor, &text);
                    return;
                }
            }
            if let Overlay::Elicit {
                selected,
                picked,
                draft,
            } = overlay
            {
                if elicit_needs_draft(ctx, *selected, picked) {
                    let mut cur = draft.chars().count();
                    ask_view::push_draft(draft, &mut cur, &text);
                    return;
                }
            }
            overlay.push_str(&text);
        }
        return;
    }
    let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) else {
        return;
    };
    match payload {
        crate::app::clipboard::PastePayload::Empty => flash(ctx, "剪贴板为空"),
        crate::app::clipboard::PastePayload::Text(text) => prompt.handle_paste(&text),
        crate::app::clipboard::PastePayload::Image(img) => match prompt.insert_image(img) {
            Ok(_) => {}
            Err(e) => flash(ctx, e),
        },
    }
}

fn open_context_from_status(ctx: &Context, overlay: &mut Overlay, column: u16, row: u16) -> bool {
    let Ok(status) = ctx.require::<StatusLine>(TUI_STATUS) else {
        return false;
    };
    // chip 压在右段最右边，先判它——否则点 `[Agents]` 会连带把占用 overlay 开出来。
    if status.hit_dashboard(column, row) {
        *overlay = crate::views::dashboard::open(ctx);
        return true;
    }
    if !status.hit_right(column, row) {
        return false;
    }
    *overlay = Overlay::usage(UsageTab::Context);
    true
}

pub(super) fn overlay_keys(code: KeyCode, ctrl: bool) -> Option<Action> {
    match code {
        KeyCode::Esc => Some(Action::OverlayClose),
        KeyCode::Enter => Some(Action::OverlayAccept),
        KeyCode::Up => Some(Action::OverlayMove(-1)),
        KeyCode::Down => Some(Action::OverlayMove(1)),
        KeyCode::Left => Some(Action::OverlayNavH(-1)),
        KeyCode::Right => Some(Action::OverlayNavH(1)),
        KeyCode::Backspace => Some(Action::OverlayBackspace),
        KeyCode::F(2) => Some(Action::SettingsModal),
        KeyCode::F(3) => Some(Action::ResumePicker),
        KeyCode::Char('.') if ctrl => Some(Action::Help),
        KeyCode::Char('x') if ctrl => Some(Action::Help),
        KeyCode::Char(' ') if !ctrl => Some(Action::OverlaySpace),
        // Ctrl 组合，不走 `Char(c) if !ctrl`，所以搜索框里照样能打 r。
        KeyCode::Char('r') | KeyCode::Char('R') if ctrl => Some(Action::OverlayRefresh),
        // 会话面板上的「+ 新会话」。别的 overlay 开着时按到只会白跑一次 TabNew，
        // 那本来就是它在主界面的语义。
        KeyCode::Char('n') | KeyCode::Char('N') if ctrl => Some(Action::TabNew),
        KeyCode::Tab | KeyCode::BackTab => Some(Action::OverlayTab),
        KeyCode::Char(c) if !ctrl => Some(Action::OverlayChar(c)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mcps_overlay() -> Overlay {
        Overlay::Mcps {
            selected: 0,
            query: String::new(),
            tools_expanded: HashSet::new(),
            section_collapsed: false,
        }
    }

    fn refresh(overlay: &mut Overlay) -> Vec<Effect> {
        run_action(
            &Context::new(),
            Action::OverlayRefresh,
            overlay,
            &PickerHits::default(),
            &[],
            &[],
            &[],
            &[],
        )
    }

    /// 点标签切页在**三条**鼠标路径上都要成立：欢迎屏 / overlay 开着时是
    /// `Click`，其余是 `MouseDown` + `MouseUp`。新开的分页一定停在欢迎屏上，
    /// 只接 `MouseUp` 的话点标签就是死的。
    #[tokio::test]
    async fn clicking_a_tab_switches_on_every_mouse_path() {
        let ctx = Context::new();
        let bar = [(
            Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 1,
            },
            tab_bar::TabHit(3),
        )];
        let go = |action: Action, overlay: &mut Overlay| {
            run_action(
                &ctx,
                action,
                overlay,
                &PickerHits::default(),
                &[],
                &[],
                &[],
                &bar,
            )
        };

        let mut overlay = Overlay::None;
        assert!(matches!(
            go(Action::Click { column: 2, row: 0 }, &mut overlay).as_slice(),
            [Effect::TabGo { id: 3 }]
        ));
        assert!(matches!(
            go(Action::MouseUp { column: 2, row: 0 }, &mut overlay).as_slice(),
            [Effect::TabGo { id: 3 }]
        ));
        // 按下去只是别起选区，不产生副作用。
        assert!(go(Action::MouseDown { column: 2, row: 0 }, &mut overlay).is_empty());
        // 不在标签栏那一行的点击照旧。
        assert!(go(Action::Click { column: 2, row: 7 }, &mut overlay).is_empty());
    }

    fn key_action(code: KeyCode, mods: KeyModifiers) -> Option<Action> {
        to_action(
            &Context::new(),
            Event::Key(crossterm::event::KeyEvent::new(code, mods)),
            &Overlay::None,
            None,
        )
    }

    /// 在下拉里上下选到某一行再回车：要把**那一行**填进输入框。
    /// 以前补参数阶段的 Enter 一律直接发，于是选中 `/tab close` 回车发出去的是
    /// 光秃秃的 `/tab` —— 开了一张新页。
    #[tokio::test]
    async fn enter_takes_the_arg_row_the_user_highlighted() {
        let ctx = Context::new();
        for p in [crate::plugin::theme(), crate::plugin::prompt()] {
            ctx.plugin(p, ()).unwrap().wait().await.unwrap();
        }
        let prompt = ctx.get::<PromptWidget>(TUI_PROMPT).unwrap();
        let enter = |ctx: &Context| {
            to_action(
                ctx,
                Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Enter,
                    KeyModifiers::NONE,
                )),
                &Overlay::None,
                None,
            )
        };

        // 没动过下拉：Enter 照旧直接发，`/model` 这种「不带参数也有意义」的命令
        // 不用多按一下。
        prompt.insert_str("/tab ");
        assert!(matches!(enter(&ctx), Some(Action::SendPrompt(t)) if t.trim() == "/tab"));

        // 选到 `/tab close` 再回车。
        prompt.insert_str("/tab ");
        for _ in 0..16 {
            if prompt
                .slash_snapshot()
                .current()
                .is_some_and(|row| row.display == "/tab close")
            {
                break;
            }
            prompt.slash_move(1);
        }
        assert!(matches!(enter(&ctx), Some(Action::SlashAccept)));
        assert!(crate::app::dispatch::dispatch(Action::SlashAccept, &prompt).is_empty());
        assert_eq!(prompt.text(), "/tab close ");

        // 填完就当没选过：下一下 Enter 才是发送。
        assert!(matches!(enter(&ctx), Some(Action::SendPrompt(t)) if t.trim() == "/tab close"));
    }

    /// 分页键不能踩现有键位：`Ctrl+W` 还是新会话，`Ctrl+D` 还是半页下滚。
    #[test]
    fn tab_keys_do_not_steal_existing_bindings() {
        assert!(matches!(
            key_action(KeyCode::Char('n'), KeyModifiers::CONTROL),
            Some(Action::TabNew)
        ));
        assert!(matches!(
            key_action(KeyCode::Char('w'), KeyModifiers::CONTROL),
            Some(Action::NewSession)
        ));
        assert!(matches!(
            key_action(KeyCode::Char('d'), KeyModifiers::CONTROL),
            Some(Action::ScrollPage(-1))
        ));
        assert!(matches!(
            key_action(KeyCode::Char('f'), KeyModifiers::CONTROL),
            Some(Action::TabFork)
        ));
        assert!(matches!(
            key_action(KeyCode::Char('b'), KeyModifiers::CONTROL),
            Some(Action::TabCarryBack)
        ));
    }

    /// `Alt+1..9` 直达标签上那个号；不带 Alt 的数字照样进输入框。
    #[test]
    fn alt_digits_jump_to_tabs() {
        assert!(matches!(
            key_action(KeyCode::Char('3'), KeyModifiers::ALT),
            Some(Action::TabGo(3))
        ));
        assert!(matches!(
            key_action(KeyCode::Char('9'), KeyModifiers::ALT),
            Some(Action::TabGo(9))
        ));
        assert!(
            matches!(
                key_action(KeyCode::Char('3'), KeyModifiers::NONE),
                Some(Action::InsertChar('3'))
            ),
            "光按数字得能打出数字"
        );
    }

    /// Ctrl+R 走 Ctrl 分支，不能把普通 `r` 从搜索框里抢走 —— `i` 今天就是那样
    /// 被抢的，`cua-driver` 里的 `i` 永远打不出来。
    #[test]
    fn ctrl_r_refreshes_without_stealing_the_search_letter() {
        assert!(matches!(
            overlay_keys(KeyCode::Char('r'), true),
            Some(Action::OverlayRefresh)
        ));
        assert!(matches!(
            overlay_keys(KeyCode::Char('R'), true),
            Some(Action::OverlayRefresh)
        ));
        assert!(matches!(
            overlay_keys(KeyCode::Char('r'), false),
            Some(Action::OverlayChar('r'))
        ));
    }

    #[tokio::test]
    async fn refresh_reloads_mcps_and_rechecks_the_driver() {
        let mut overlay = mcps_overlay();
        assert!(matches!(
            refresh(&mut overlay).as_slice(),
            [Effect::ReloadMcps { quiet: false }]
        ));

        // 驾驶舱的 Ctrl+R 还要重新探一次本机 driver。
        let mut computer = computer_overlay(None);
        assert!(matches!(
            refresh(&mut computer).as_slice(),
            [Effect::CuaRefresh, Effect::ReloadMcps { quiet: false }]
        ));

        let mut other = Overlay::Tasks {
            selected: 0,
            query: String::new(),
            collapsed: HashSet::new(),
        };
        assert!(refresh(&mut other).is_empty(), "别的 overlay 不该被牵连");
    }

    fn computer_overlay(pending: Option<CuaAction>) -> Overlay {
        Overlay::Computer { scroll: 0, pending }
    }

    /// 本测试二进制共用一个隔离的 `DOCK_HOME`，并显式关掉内置 cua-driver：
    /// 否则挂上 `mcp-client` 会去连本机真实配置里的 MCP 服务器。
    fn isolated_env() {
        static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("dock-tui-keys-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::env::set_var("DOCK_HOME", &dir);
            std::env::set_var("DOCK_CUA_DRIVER", "off");
        });
    }

    async fn boot_computer() -> Context {
        isolated_env();
        let root = Context::new();
        for fiber in [
            root.plugin(cordis_spine::slash(), ()).unwrap(),
            root.plugin(cordis_spine::tools(), ()).unwrap(),
            root.plugin(cordis_spine::mcp_client(), ()).unwrap(),
        ] {
            fiber.wait().await.unwrap();
        }
        root.plugin(cordis_spine::tool_computer(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        root
    }

    fn run(ctx: &Context, action: Action, overlay: &mut Overlay) -> Vec<Effect> {
        run_action(
            ctx,
            action,
            overlay,
            &PickerHits::default(),
            &[],
            &[],
            &[],
            &[],
        )
    }

    /// 按 `i` 只进确认态。下载并执行外部脚本这种事，不能一个键就开跑。
    #[tokio::test]
    async fn install_takes_two_keys() {
        let ctx = boot_computer().await;
        let mut overlay = computer_overlay(None);

        let effects = run(&ctx, Action::OverlayChar('i'), &mut overlay);
        assert!(effects.is_empty(), "按 i 不该直接产生副作用：{effects:?}");
        assert!(matches!(
            overlay,
            Overlay::Computer {
                pending: Some(CuaAction::Install),
                ..
            }
        ));

        let effects = run(&ctx, Action::OverlayAccept, &mut overlay);
        assert!(matches!(
            effects.as_slice(),
            [Effect::CuaRun {
                action: CuaAction::Install
            }]
        ));
        assert!(
            matches!(overlay, Overlay::Computer { pending: None, .. }),
            "确认过就该退出确认态"
        );
        assert!(overlay.is_open(), "确认后窗口要留着看进度");
    }

    /// 没有待确认动作时，Enter 还是和别的只读驾驶舱一样关窗。
    #[tokio::test]
    async fn enter_without_confirmation_still_closes() {
        let ctx = boot_computer().await;
        let mut overlay = computer_overlay(None);
        let effects = run(&ctx, Action::OverlayAccept, &mut overlay);
        assert!(effects.is_empty(), "{effects:?}");
        assert!(!overlay.is_open());
    }

    /// 确认态里的 Esc 是「不装了」，不是「关窗」。
    #[tokio::test]
    async fn esc_cancels_confirmation_before_closing() {
        let ctx = boot_computer().await;
        let mut overlay = computer_overlay(Some(CuaAction::Install));

        run(&ctx, Action::OverlayClose, &mut overlay);
        assert!(matches!(overlay, Overlay::Computer { pending: None, .. }));
        assert!(overlay.is_open(), "第一下 Esc 只该退出确认态");

        run(&ctx, Action::OverlayClose, &mut overlay);
        assert!(!overlay.is_open());
    }

    /// 没装 driver（也就没授权可谈）时按 `p` 不进确认态。
    #[tokio::test]
    async fn grant_key_is_inert_without_a_driver() {
        let ctx = boot_computer().await;
        let mut overlay = computer_overlay(None);
        let effects = run(&ctx, Action::OverlayChar('p'), &mut overlay);
        assert!(effects.is_empty(), "{effects:?}");
        assert!(matches!(overlay, Overlay::Computer { pending: None, .. }));
    }
}
