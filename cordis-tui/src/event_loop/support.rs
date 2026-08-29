//! Overlay openers, MCP/goal helpers, and small loop utilities.

use std::collections::HashSet;

use cordis::Context;
use cordis_spine::{
    goal_composer_fill, loop_composer_fill, loop_schedule_instruction, AgentPresets, AppSettings,
    Ask, Cron, Goal, Jobs, LoopFireMode, Mcp, McpStatus, MermaidEngineKind, Permissions, PlanMode,
    Sessions, Slash, SlotKeyResult, Subagents, TuiSlots, UserImage, Workflows, AGENT_PRESETS, ASK,
    CRON, GOAL, JOBS, MCP, PERMISSIONS, PLAN_MODE, SESSIONS, SETTINGS, SLASH, SUBAGENTS, TUI_SLOTS,
    WORKFLOWS,
};

use crate::ask_view;
use crate::mcp_elicit_view;
use crate::clipboard;
use crate::error::Result;
use crate::grok::mcps;
use crate::grok::tasks_pane::{self, GroupKind, TaskEntry};
use crate::grok::workflows::WorkflowRunSnapshot;
use crate::names::{SESSION_PORT, TUI_PROMPT, TUI_SCROLLBACK, TUI_STATUS, TUI_WELCOME};
use crate::overlay::{filter_help_items, filter_sessions, filter_strings, InspectTarget, Overlay};
use crate::permission_view;
use crate::plan_approval_view;
use crate::preset_overlay::{self, PresetAction, PresetView};
use crate::scrollback::Scrollback;
use crate::session::SessionRef;
use crate::settings_modal;
use crate::slash::{self, filter_args};
use crate::text_overlay;
use crate::usage_overlay;

use crate::actions::{
    interpret_goal_composer_ex, interpret_loop_composer, Effect, GoalComposer, LoopComposer,
};
use crate::goal_overlay;
use crate::goal_pane::GoalHit;
use crate::grok::mermaid::AffordanceKind;
use crate::inspect_overlay;
use crate::mermaid_png;
use crate::prompt::{PastedImage, PromptWidget};
use crate::status::StatusLine;
use crate::welcome::Welcome;

pub(super) fn open_permission_if_needed(ctx: &Context, overlay: &mut Overlay) {
    if matches!(overlay, Overlay::Elicit { .. }) {
        overlay.close();
    } else if overlay.is_open() {
        return;
    }
    if ctx
        .get::<Permissions>(PERMISSIONS)
        .and_then(|p| p.front())
        .is_some()
    {
        *overlay = Overlay::Permission { selected: 0 };
    }
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

pub(super) fn open_ask_if_needed(ctx: &Context, overlay: &mut Overlay) {
    if matches!(
        overlay,
        Overlay::Permission { .. } | Overlay::Ask { .. } | Overlay::PlanApproval { .. }
    ) {
        if matches!(overlay, Overlay::Ask { .. })
            && ctx.get::<Ask>(ASK).and_then(|a| a.front()).is_none()
        {
            overlay.close();
        }
        return;
    }
    if matches!(overlay, Overlay::Elicit { .. }) {
        overlay.close();
    } else if overlay.is_open() {
        return;
    }
    let Some(front) = ctx.get::<Ask>(ASK).and_then(|a| a.front()) else {
        return;
    };
    *overlay = ask_overlay_from_prompt(&front);
}

pub(super) fn navigate_ask(ctx: &Context, overlay: &mut Overlay, delta: i16) {
    if let Overlay::Ask {
        selected,
        picked,
        ..
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
    if !ask.navigate(delta as i32) {
        return;
    }
    let Some(front) = ask.front() else {
        overlay.close();
        return;
    };
    *overlay = ask_overlay_from_prompt(&front);
}

fn ask_overlay_from_prompt(front: &cordis_spine::AskPrompt) -> Overlay {
    let Some(q) = front.questions.get(front.index) else {
        return Overlay::Ask {
            selected: 0,
            picked: Vec::new(),
            draft: String::new(),
            draft_cursor: 0,
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
    Overlay::Ask {
        selected,
        picked,
        draft,
        draft_cursor,
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
        Overlay::Permission { .. } | Overlay::Ask { .. } | Overlay::PlanApproval { .. }
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

pub(super) fn scroll_usage(ctx: &Context, scroll: &mut usize, delta: i16) {
    let usage = ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.prompt_usage())
        .unwrap_or_default();
    let max = usage_overlay::max_scroll(&usage, 16);
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
    target: &crate::overlay::InspectTarget,
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
    if mcp_elicit_view::needs_draft(&front, selected, &picked) && draft.trim().is_empty() {
        flash(ctx, "请输入具体内容");
        return;
    }
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

pub(super) fn task_entries(
    ctx: &Context,
    query: &str,
    collapsed: &HashSet<GroupKind>,
) -> Vec<TaskEntry> {
    let jobs = ctx.get::<Jobs>(JOBS).map(|j| j.list()).unwrap_or_default();
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
    let items = tasks_pane::collect_items(&jobs, &subagents, &cron, &workflows, presets.as_deref());
    let rows = tasks_pane::rebuild_entries(&items, collapsed);
    tasks_pane::filter_entries(rows, query)
}

pub(super) fn to_tui_workflow(snap: cordis_spine::WorkflowRunSnap) -> WorkflowRunSnapshot {
    WorkflowRunSnapshot {
        run_id: snap.run_id,
        name: snap.name,
        objective: snap.objective,
        status: snap.status,
        management_available: true,
        builtin: snap.builtin,
        phases: Vec::new(),
        current_phase: snap.current_phase,
        agents: Vec::new(),
        agent_budget: None,
        agents_used: 0,
        agents_reserved: 0,
        agents_remaining: None,
        agent_usage_incomplete: false,
        active_agents: snap.agents_running,
        elapsed_ms: snap.elapsed_ms,
        received_at: snap.received_at,
        pause_message: snap.pause_message,
        result_summary: snap.result_summary,
    }
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
        || ctx
            .get::<Subagents>(SUBAGENTS)
            .is_some_and(|s| s.list().iter().any(|a| a.running()))
        || ctx.get::<Goal>(GOAL).is_some_and(|g| g.active())
        || ctx
            .get::<Jobs>(JOBS)
            .is_some_and(|j| j.list().iter().any(|j| !j.done))
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
            slash::command_for_submit_ex(&line, &extras)
                .map(|(pick, args)| vec![crate::actions::effect_for_pick(pick, &args, &extras)])
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

pub(super) fn welcome_open(ctx: &Context) -> bool {
    ctx.get::<Welcome>(TUI_WELCOME)
        .is_some_and(|w| w.empty_session())
}

pub(super) fn slash_open(ctx: &Context) -> bool {
    ctx.get::<PromptWidget>(TUI_PROMPT)
        .is_some_and(|p| p.slash_snapshot().open)
}

pub(super) fn overlay_len(ctx: &Context, overlay: &Overlay) -> usize {
    match overlay {
        Overlay::None => 0,
        Overlay::Resume { query, .. } => ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| filter_sessions(&s.archived(), query).len())
            .unwrap_or(0),
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
        Overlay::PlanApproval { view_only, .. } => plan_approval_view::options(*view_only).len(),
        Overlay::Ask { picked, .. } => ctx
            .get::<Ask>(ASK)
            .and_then(|a| a.front())
            .and_then(|p| p.questions.get(p.index).map(ask_view::labels))
            .map(|l| l.len())
            .unwrap_or(picked.len()),
        Overlay::Elicit { picked, .. } => elicit_front(ctx)
            .map(|p| {
                if p.typing {
                    0
                } else {
                    p.options.len()
                }
            })
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
        | Overlay::Slot { .. }
        | Overlay::Inspect { .. } => 0,
        Overlay::Presets(view) => preset_overlay::overlay_len(ctx, view),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
