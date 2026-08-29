//! Main event loop.
//!
//! Terminal takeover copied from grok-build/.../xai-grok-pager/src/app/mod.rs
//! `init_terminal` (fullscreen): raw mode, **stderr** alternate screen, mouse
//! capture, bracketed paste. The loop is still the thin `tokio::select!`
//! from `app/event_loop.rs` — no `MvpAgent`.

use std::collections::{HashSet, VecDeque};
use std::io::{stderr, Stderr};
use std::panic;
use std::time::{Duration, Instant};

use cordis::Context;
use cordis_spine::{
    goal_composer_fill, AgentPresets, AppSettings, Ask, Cron, Goal, Jobs, LogEvent, Mcp,
    MermaidEngineKind, PermissionMode, PermissionOptionKind, Permissions, PlanDecision, PlanMode,
    Sessions, Slash, SlotKeyResult, Subagents, ToolCall, Tools, TuiSlots, UserImage, Workflows,
    AGENT_PRESETS, ASK, ASK_EVENT, CRON, GOAL, JOBS, MCP, PERMISSIONS, PERMISSION_EVENT,
    PLAN_EVENT, PLAN_MODE, SESSIONS, SESSION_EVENT, SETTINGS, SLASH, SUBAGENTS, TOOLS, TUI_SLOTS,
    WORKFLOWS,
};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::prelude::CrosstermBackend;
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui::Terminal;

use crate::ask_view;
use crate::clipboard;
use crate::error::{Error, Result};
use crate::file_search;
use crate::grok::picker::{PickerHits, PickerRow};
use crate::grok::shortcuts::ShortcutsBar;
use crate::grok::tasks_pane::{self, GroupKind, TaskEntry};
use crate::grok::workflows::{self, WorkflowRunSnapshot};
use crate::names::{
    SESSION_PORT, TUI_PROMPT, TUI_SCROLLBACK, TUI_SHORTCUTS, TUI_STATUS, TUI_WELCOME,
};
use crate::overlay::{
    self, filter_help_items, filter_sessions, filter_strings, HelpItem, HelpKind, InspectTarget,
    Overlay,
};
use crate::permission_view;
use crate::plan_approval_view;
use crate::preset_overlay::{self, PresetAction, PresetView};
use crate::queue_pane::{self, QueueHit};
use crate::scrollback::{ClickHit, MouseUpResult, Scrollback};
use crate::session::SessionRef;
use crate::settings_modal::{self, SettingsField};
use crate::slash::{self, desired_item_rows, filter_args, render_dropdown, SlashSnapshot};
use crate::subagent_dock;
use crate::text_overlay;
use crate::theme::Theme;
use crate::usage_overlay;

use super::actions::{interpret_goal_composer_ex, Action, Effect, GoalComposer};
use super::dispatch::dispatch;
use super::input::spawn_reader;
use super::prompt::{PastedImage, PromptWidget};
use super::status::{self, StatusLine};
use super::status_bar::StatusBar;
use super::welcome::{Welcome, WelcomeHit};
use crate::goal_overlay;
use crate::goal_pane::{self, GoalHit};
use crate::grok::mermaid::AffordanceKind;
use crate::inspect_overlay::{self, InspectClick};
use crate::mermaid_png;
use crate::mode_cycle;
use crate::shortcuts::Shortcuts;

/// Grok `LayoutConfig::default()` outer padding.
const OUTER_VPAD: u16 = 1;
const OUTER_HPAD: u16 = 2;
/// Grok `AgentView::ESC_CANCEL_REWIND_GRACE` — swallow the second Esc of a
/// double-press so the restored draft is not wiped.
const ESC_CANCEL_REWIND_GRACE: Duration = Duration::from_millis(1000);

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let mut err = stderr();
    let _ = execute!(
        err,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
        Show,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

pub async fn run(ctx: Context) -> Result<()> {
    let _guard = TerminalGuard;
    let prev_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev_hook(info);
    }));

    enable_raw_mode().map_err(|e| Error::Message(e.to_string()))?;
    let mut err = stderr();
    execute!(
        err,
        EnterAlternateScreen,
        Clear(ClearType::All),
        EnableMouseCapture,
        EnableFocusChange,
        EnableBracketedPaste,
        Hide,
    )
    .map_err(|e| Error::Message(e.to_string()))?;
    let backend = CrosstermBackend::new(err);
    let mut terminal = Terminal::new(backend).map_err(|e| Error::Message(e.to_string()))?;
    terminal
        .clear()
        .map_err(|e| Error::Message(e.to_string()))?;

    let (redraw_tx, mut redraw_rx) = tokio::sync::mpsc::unbounded_channel();
    let redraw_session = redraw_tx.clone();
    let redraw_perm = redraw_tx.clone();
    let redraw_ask = redraw_tx.clone();
    let redraw_plan = redraw_tx.clone();
    let _listen = ctx
        .on(SESSION_EVENT, move |_: &LogEvent| {
            let _ = redraw_session.send(());
        })
        .ok();
    let _perm = ctx
        .on(PERMISSION_EVENT, move |_: &()| {
            let _ = redraw_perm.send(());
        })
        .ok();
    let _ask = ctx
        .on(ASK_EVENT, move |_: &()| {
            let _ = redraw_ask.send(());
        })
        .ok();
    let _plan = ctx
        .on(PLAN_EVENT, move |_: &()| {
            let _ = redraw_plan.send(());
        })
        .ok();

    let mut input_rx = spawn_reader();
    let mut shimmer = tokio::time::interval(Duration::from_millis(80));
    let mut overlay = Overlay::None;
    let mut hits = PickerHits::default();
    let mut dock_hits: Vec<(Rect, String)> = Vec::new();
    let mut goal_hits: Vec<(Rect, GoalHit)> = Vec::new();
    let mut queue_hits: Vec<(Rect, QueueHit)> = Vec::new();
    let mut pointer = (0u16, 0u16);
    let mut quit = false;
    let mut esc_suppress_until: Option<Instant> = None;
    draw(
        &ctx,
        &mut terminal,
        &overlay,
        &mut hits,
        &mut dock_hits,
        &mut goal_hits,
        &mut queue_hits,
        pointer,
    )?;

    while !quit {
        tokio::select! {
            biased;
            event = input_rx.recv() => {
                let Some(event) = event else { break };
                if matches!(event, Event::Resize(_, _)) {
                    draw(
                        &ctx,
                        &mut terminal,
                        &overlay,
                        &mut hits,
                        &mut dock_hits,
                        &mut goal_hits,
                        &mut queue_hits,
                        pointer,
                    )?;
                    continue;
                }
                if let Event::Mouse(m) = &event {
                    pointer = (m.column, m.row);
                }
                if let Some(action) = to_action(&ctx, event, &overlay, esc_suppress_until) {
                    let mut effects: VecDeque<Effect> = run_action(
                        &ctx,
                        action,
                        &mut overlay,
                        &hits,
                        &dock_hits,
                        &goal_hits,
                        &queue_hits,
                    )
                    .into();
                    while let Some(effect) = effects.pop_front() {
                        match effect {
                            Effect::Quit => quit = true,
                            Effect::NewSession => {
                                if let Ok(sessions) = ctx.require::<Sessions>(SESSIONS) {
                                    sessions.archive_current();
                                    sessions.clear();
                                }
                                if let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) {
                                    plan.set(false);
                                }
                                if let Some(goal) = ctx.get::<Goal>(GOAL) {
                                    goal.clear();
                                }
                                overlay.close();
                            }
                            Effect::ResumePicker => {
                                overlay = Overlay::Resume {
                                    selected: 0,
                                    query: String::new(),
                                };
                            }
                            Effect::Help => {
                                overlay = Overlay::Help {
                                    selected: 0,
                                    query: String::new(),
                                };
                            }
                            Effect::HistoryPicker => {
                                overlay = Overlay::History {
                                    selected: 0,
                                    query: String::new(),
                                };
                            }
                            Effect::Find => {
                                overlay = Overlay::Find {
                                    selected: 0,
                                    query: String::new(),
                                };
                            }
                            Effect::RestoreSession(id) => {
                                if let Ok(sessions) = ctx.require::<Sessions>(SESSIONS) {
                                    sessions.archive_current();
                                    sessions.restore(&id);
                                }
                                overlay.close();
                            }
                            Effect::InsertHistory(text) => {
                                if let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) {
                                    prompt.apply_text(&text);
                                }
                                overlay.close();
                            }
                            Effect::JumpToLine(idx) => {
                                if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                                    scrollback.jump_to_line(idx);
                                }
                                overlay.close();
                            }
                            Effect::SendPrompt { text, send_now } => {
                                if let Ok(status) = ctx.require::<StatusLine>(TUI_STATUS) {
                                    status.clear_notice();
                                }
                                if let Some(more) = intercept_goal_send(&ctx, &text) {
                                    for extra in more.into_iter().rev() {
                                        effects.push_front(extra);
                                    }
                                } else if let Ok(session) =
                                    ctx.require::<SessionRef>(SESSION_PORT)
                                {
                                    if let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) {
                                        prompt.note_sent(&text);
                                    }
                                    session.submit(text, send_now);
                                }
                            }
                            Effect::FillPrompt { text } => {
                                overlay.close();
                                if let Some(goal) = ctx.get::<Goal>(GOAL) {
                                    goal.arm_composer();
                                }
                                if let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) {
                                    prompt.set_text(&text);
                                }
                            }
                            Effect::SetPrompt { text } => {
                                overlay.close();
                                if let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) {
                                    prompt.set_text(&text);
                                }
                            }
                            Effect::CopyAssistant { n, file } => {
                                let text = ctx
                                    .get::<Scrollback>(TUI_SCROLLBACK)
                                    .and_then(|s| s.last_assistant(n));
                                match text {
                                    Some(body) => copy_out(&ctx, &body, file.as_deref()),
                                    None => flash(&ctx, "no assistant message"),
                                }
                            }
                            Effect::CopyText(text) => copy_out(&ctx, &text, None),
                            Effect::OpenImage(path) => match mermaid_png::open_path(&path) {
                                Ok(()) => flash(&ctx, format!("opened {}", path.display())),
                                Err(e) => flash(&ctx, e),
                            },
                            Effect::ArgPicker { kind, cmd } => {
                                overlay = Overlay::Args {
                                    kind,
                                    cmd,
                                    selected: 0,
                                    query: String::new(),
                                };
                            }
                            Effect::SetTheme(kind) => {
                                Theme::apply_kind(kind);
                                overlay.close();
                                flash(&ctx, format!("theme {}", kind.display_name()));
                            }
                            Effect::SetModel(model) => {
                                if let Some(settings) = ctx.get::<AppSettings>(SETTINGS) {
                                    settings.set_model(model.clone());
                                    let catalog = settings.catalog();
                                    overlay.close();
                                    flash(&ctx, format!("已切换 {}", chrome_model_label(&model, &catalog)));
                                } else {
                                    overlay.close();
                                    flash(&ctx, format!("已切换 {model}"));
                                }
                            }
                            Effect::SetEffort(effort) => {
                                if let Some(settings) = ctx.get::<AppSettings>(SETTINGS) {
                                    settings.set_effort(effort.clone());
                                }
                                overlay.close();
                                flash(&ctx, format!("effort {effort}"));
                            }
                            Effect::ToggleTimestamps => {
                                let on = ctx
                                    .get::<AppSettings>(SETTINGS)
                                    .map(|s| s.toggle_timestamps())
                                    .unwrap_or(false);
                                overlay.close();
                                flash(
                                    &ctx,
                                    if on {
                                        "时间戳已开"
                                    } else {
                                        "时间戳已关"
                                    },
                                );
                            }
                            Effect::CronAdd { every, prompt } => {
                                match ctx.get::<Cron>(CRON) {
                                    Some(cron) => {
                                        let id = cron.add(every, prompt.clone());
                                        overlay.close();
                                        flash(
                                            &ctx,
                                            format!(
                                                "cron {id} every {}s",
                                                every.as_secs().max(1)
                                            ),
                                        );
                                    }
                                    None => flash(&ctx, "cron is not mounted"),
                                }
                            }
                            Effect::Export(path) => {
                                let path = path.unwrap_or_else(|| {
                                    std::path::PathBuf::from("dock-transcript.txt")
                                });
                                match export_transcript(&ctx, &path) {
                                    Ok(()) => flash(&ctx, format!("wrote {}", path.display())),
                                    Err(e) => flash(&ctx, e),
                                }
                                overlay.close();
                            }
                            Effect::ChangeDir(path) => match std::env::set_current_dir(&path) {
                                Ok(()) => {
                                    let cwd = std::env::current_dir().unwrap_or(path.clone());
                                    if let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) {
                                        presets.set_workspace_root(&cwd);
                                    }
                                    overlay.close();
                                    flash(&ctx, format!("cwd {}", path.display()));
                                }
                                Err(e) => flash(&ctx, e.to_string()),
                            },
                            Effect::SettingsModal => {
                                overlay = Overlay::Settings {
                                    selected: 0,
                                    picking: None,
                                };
                            }
                            Effect::CancelTurn => {
                                if let Ok(session) = ctx.require::<SessionRef>(SESSION_PORT) {
                                    session.cancel();
                                }
                                if rewind_cancelled_send(&ctx) {
                                    esc_suppress_until =
                                        Some(Instant::now() + ESC_CANCEL_REWIND_GRACE);
                                    flash(&ctx, "已撤销发送");
                                } else {
                                    flash(&ctx, "已取消");
                                }
                            }
                            Effect::PromoteQueued { id } => {
                                if let Ok(session) = ctx.require::<SessionRef>(SESSION_PORT) {
                                    session.promote(id);
                                }
                            }
                            Effect::EditQueued { id } => {
                                if let Ok(session) = ctx.require::<SessionRef>(SESSION_PORT) {
                                    if let Some(item) = session.take_queued(id) {
                                        if let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT)
                                        {
                                            prompt.set_text(&item.text);
                                        }
                                    }
                                }
                            }
                            Effect::EnterPlan { description } => {
                                if let Some(goal) = ctx.get::<Goal>(GOAL) {
                                    goal.disarm_composer();
                                }
                                if let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) {
                                    if description.is_some() {
                                        plan.enter_active();
                                    } else {
                                        plan.enter_pending();
                                    }
                                }
                                overlay.close();
                                flash(&ctx, "计划模式");
                                if let Some(desc) = description {
                                    if let Ok(session) = ctx.require::<SessionRef>(SESSION_PORT) {
                                        session.submit(desc, false);
                                    }
                                }
                            }
                            Effect::ViewPlan => {
                                open_view_plan(&ctx, &mut overlay);
                            }
                            Effect::EnterGoal { objective } => {
                                overlay.close();
                                match objective {
                                    None => {
                                        if let Some(goal) = ctx.get::<Goal>(GOAL) {
                                            goal.arm_composer();
                                        }
                                        if let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) {
                                            prompt.set_text(&goal_composer_fill());
                                        }
                                    }
                                    Some(raw) => start_goal(&ctx, raw),
                                }
                            }
                            Effect::ShowGoal { editing } => {
                                open_goal_overlay(&ctx, &mut overlay, editing);
                            }
                            Effect::GoalPause => {
                                if let Some(goal) = ctx.get::<Goal>(GOAL) {
                                    goal.disarm_composer();
                                    if goal.pause() {
                                        flash(&ctx, "目标已暂停");
                                    } else {
                                        flash(&ctx, "没有进行中的目标");
                                    }
                                }
                            }
                            Effect::GoalResume => {
                                if let Some(goal) = ctx.get::<Goal>(GOAL) {
                                    goal.disarm_composer();
                                    if goal.resume() {
                                        flash(&ctx, "目标已继续");
                                    } else {
                                        flash(&ctx, "没有已暂停的目标");
                                    }
                                }
                            }
                            Effect::GoalClear => {
                                if let Some(goal) = ctx.get::<Goal>(GOAL) {
                                    if goal.present() {
                                        goal.clear();
                                        overlay.close();
                                        flash(&ctx, "已清除目标");
                                    } else {
                                        goal.disarm_composer();
                                        flash(&ctx, "没有活动目标");
                                    }
                                }
                            }
                            Effect::ShowTasks => {
                                overlay = Overlay::Tasks {
                                    selected: 0,
                                    query: String::new(),
                                    collapsed: HashSet::new(),
                                };
                            }
                            Effect::ToggleWorkflows => {
                                if matches!(overlay, Overlay::Workflows { .. }) {
                                    overlay.close();
                                } else {
                                    overlay = Overlay::Workflows {
                                        selected: 0,
                                        query: String::new(),
                                    };
                                }
                            }
                            Effect::ShowMcps => {
                                overlay = Overlay::Mcps {
                                    selected: 0,
                                    query: String::new(),
                                };
                            }
                            Effect::ShowPresets { focus } => {
                                overlay = open_presets_overlay(&ctx, focus);
                            }
                            Effect::ShowUsage => {
                                overlay = Overlay::Usage { scroll: 0 };
                            }
                            Effect::Compact { context } => {
                                flash(&ctx, "正在压缩上下文…");
                                if let Ok(session) = ctx.require::<SessionRef>(SESSION_PORT) {
                                    session.compact(context);
                                }
                            }
                            Effect::ShowNotice { title, body } => {
                                overlay = Overlay::Notice {
                                    title,
                                    body,
                                    scroll: 0,
                                };
                            }
                            Effect::OpenSlot { id } => {
                                overlay = Overlay::Slot { id, scroll: 0 };
                            }
                            Effect::RunTool {
                                name,
                                arguments,
                                title,
                            } => {
                                let tools = ctx.get::<Tools>(TOOLS);
                                let slash = ctx.get::<Slash>(SLASH);
                                let redraw = redraw_tx.clone();
                                if tools.is_none() {
                                    flash(&ctx, "tools 未挂载");
                                } else {
                                    flash(&ctx, format!("运行 {name}…"));
                                    tokio::spawn(async move {
                                        let content = match tools {
                                            Some(tools) => {
                                                tools
                                                    .execute(ToolCall {
                                                        id: format!("slash-tool-{name}"),
                                                        name: name.clone(),
                                                        arguments,
                                                    })
                                                    .await
                                                    .content
                                            }
                                            None => "Error: tools is not mounted".into(),
                                        };
                                        if let Some(slash) = slash {
                                            slash.queue_notice(title, content);
                                        }
                                        let _ = redraw.send(());
                                    });
                                }
                            }
                        }
                    }
                    open_permission_if_needed(&ctx, &mut overlay);
                    open_ask_if_needed(&ctx, &mut overlay);
                    open_plan_approval_if_needed(&ctx, &mut overlay);
                    open_slot_if_needed(&ctx, &mut overlay);
                    open_slash_notice_if_needed(&ctx, &mut overlay);
                    draw(
                        &ctx,
                        &mut terminal,
                        &overlay,
                        &mut hits,
                        &mut dock_hits,
                        &mut goal_hits,
                        &mut queue_hits,
                        pointer,
                    )?;
                }
            }
            Some(()) = redraw_rx.recv() => {
                open_permission_if_needed(&ctx, &mut overlay);
                open_ask_if_needed(&ctx, &mut overlay);
                open_plan_approval_if_needed(&ctx, &mut overlay);
                open_slot_if_needed(&ctx, &mut overlay);
                open_slash_notice_if_needed(&ctx, &mut overlay);
                draw(
                    &ctx,
                    &mut terminal,
                    &overlay,
                    &mut hits,
                    &mut dock_hits,
                    &mut goal_hits,
                    &mut queue_hits,
                    pointer,
                )?;
            }
            _ = shimmer.tick() => {
                if live_redraw(&ctx, &overlay) {
                    draw(
                        &ctx,
                        &mut terminal,
                        &overlay,
                        &mut hits,
                        &mut dock_hits,
                        &mut goal_hits,
                        &mut queue_hits,
                        pointer,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn open_permission_if_needed(ctx: &Context, overlay: &mut Overlay) {
    if overlay.is_open() {
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

fn open_slot_if_needed(ctx: &Context, overlay: &mut Overlay) {
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

fn open_slash_notice_if_needed(ctx: &Context, overlay: &mut Overlay) {
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

fn dispatch_slot_key(ctx: &Context, overlay: &mut Overlay, key: &str) -> bool {
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

fn open_ask_if_needed(ctx: &Context, overlay: &mut Overlay) {
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
    if overlay.is_open() {
        return;
    }
    let Some(front) = ctx.get::<Ask>(ASK).and_then(|a| a.front()) else {
        return;
    };
    let n = front
        .questions
        .get(front.index)
        .map(ask_view::labels)
        .map(|l| l.len())
        .unwrap_or(0);
    *overlay = Overlay::Ask {
        selected: 0,
        picked: vec![false; n],
    };
}

fn open_plan_approval_if_needed(ctx: &Context, overlay: &mut Overlay) {
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

fn open_view_plan(ctx: &Context, overlay: &mut Overlay) {
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

fn scroll_plan_body(
    scroll: &mut usize,
    prompt: &cordis_spine::PlanApprovalPrompt,
    wrap: &plan_approval_view::PlanWrapCache,
    delta: i16,
) {
    let max = wrap.max_scroll(prompt, 72, 16);
    let next = (*scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
    *scroll = next;
}

fn scroll_usage(ctx: &Context, scroll: &mut usize, delta: i16) {
    let usage = ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.prompt_usage())
        .unwrap_or_default();
    let max = usage_overlay::max_scroll(&usage, 16);
    let next = (*scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
    *scroll = next;
}

fn scroll_text(scroll: &mut usize, delta: i16, body: &str) {
    let max = text_overlay::max_scroll(body, 16);
    let next = (*scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
    *scroll = next;
}

fn scroll_inspect(
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

fn close_inspect(overlay: &mut Overlay) -> Vec<Effect> {
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

fn send_inspect_composer(ctx: &Context, overlay: &mut Overlay) {
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

fn slash_extras(ctx: &Context) -> Vec<cordis_spine::SlashEntry> {
    ctx.get::<Slash>(SLASH)
        .map(|s| s.list())
        .unwrap_or_default()
}

fn accept_ask(ctx: &Context, overlay: &mut Overlay, selected: usize, picked: Vec<bool>) {
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
    if !chosen.is_empty() {
        ask.answer_current(chosen, None);
    }
    overlay.close();
}

fn task_entries(ctx: &Context, query: &str, collapsed: &HashSet<GroupKind>) -> Vec<TaskEntry> {
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

fn to_tui_workflow(snap: cordis_spine::WorkflowRunSnap) -> WorkflowRunSnapshot {
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

fn workflow_rows(ctx: &Context, query: &str) -> Vec<WorkflowRunSnapshot> {
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

fn mcp_rows(ctx: &Context, query: &str) -> Vec<(String, String)> {
    let rows = ctx
        .get::<Mcp>(MCP)
        .map(|m| {
            m.list()
                .into_iter()
                .map(|s| {
                    let status = if s.ok { "已连接" } else { "失败" };
                    (
                        format!("{} {status}", s.name),
                        if s.detail.is_empty() {
                            s.command
                        } else {
                            s.detail
                        },
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if query.is_empty() {
        return rows;
    }
    let q = query.to_ascii_lowercase();
    rows.into_iter()
        .filter(|(a, b)| a.to_ascii_lowercase().contains(&q) || b.to_ascii_lowercase().contains(&q))
        .collect()
}

fn file_search_open(ctx: &Context) -> bool {
    ctx.get::<PromptWidget>(TUI_PROMPT)
        .is_some_and(|p| p.file_search_snapshot().open)
}

fn turn_running(ctx: &Context) -> bool {
    ctx.get::<SessionRef>(SESSION_PORT)
        .is_some_and(|s| s.working())
}

fn prompt_can_send(ctx: &Context) -> bool {
    ctx.get::<PromptWidget>(TUI_PROMPT)
        .is_some_and(|p| p.can_send())
}

fn has_prompt_queue(ctx: &Context) -> bool {
    ctx.get::<SessionRef>(SESSION_PORT)
        .is_some_and(|s| !s.queued_prompts().is_empty())
}

fn live_redraw(ctx: &Context, overlay: &Overlay) -> bool {
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
fn rewind_cancelled_send(ctx: &Context) -> bool {
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

fn pasted_from_user(text: &str, images: Vec<UserImage>) -> Vec<PastedImage> {
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

fn image_chip_numbers(text: &str) -> Vec<u32> {
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

fn export_transcript(ctx: &Context, path: &std::path::Path) -> Result<(), String> {
    let sessions = ctx
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| "no sessions".to_string())?;
    let body: String = sessions.events().iter().map(|e| format!("{e}\n")).collect();
    std::fs::write(path, body).map_err(|e| e.to_string())
}

fn mermaid_click(ctx: &Context, kind: AffordanceKind, source: String) -> Vec<Effect> {
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

fn copy_out(ctx: &Context, text: &str, file: Option<&std::path::Path>) {
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

fn flash(ctx: &Context, msg: impl Into<String>) {
    if let Ok(status) = ctx.require::<StatusLine>(TUI_STATUS) {
        status.flash(msg);
    }
}

fn apply_preset_action(ctx: &Context, overlay: &mut Overlay, action: PresetAction) {
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

fn open_presets_overlay(ctx: &Context, focus: Option<String>) -> Overlay {
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

fn intercept_goal_send(ctx: &Context, text: &str) -> Option<Vec<Effect>> {
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

fn start_goal(ctx: &Context, raw: String) {
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

fn open_goal_overlay(ctx: &Context, overlay: &mut Overlay, editing: bool) {
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

fn goal_pause_resume(ctx: &Context) -> Vec<Effect> {
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

fn apply_goal_hit(ctx: &Context, overlay: &mut Overlay, hit: GoalHit) -> Vec<Effect> {
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

fn welcome_open(ctx: &Context) -> bool {
    ctx.get::<Welcome>(TUI_WELCOME)
        .is_some_and(|w| w.empty_session())
}

fn slash_open(ctx: &Context) -> bool {
    ctx.get::<PromptWidget>(TUI_PROMPT)
        .is_some_and(|p| p.slash_snapshot().open)
}

fn overlay_len(ctx: &Context, overlay: &Overlay) -> usize {
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
        Overlay::Tasks {
            query, collapsed, ..
        } => task_entries(ctx, query, collapsed).len(),
        Overlay::Mcps { query, .. } => mcp_rows(ctx, query).len(),
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

fn run_action(
    ctx: &Context,
    action: Action,
    overlay: &mut Overlay,
    hits: &PickerHits,
    dock_hits: &[(Rect, String)],
    goal_hits: &[(Rect, GoalHit)],
    queue_hits: &[(Rect, QueueHit)],
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
            if matches!(overlay, Overlay::Ask { .. }) {
                if let Some(ask) = ctx.get::<Ask>(ASK) {
                    ask.cancel();
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
            let _ = dispatch_slot_key(ctx, overlay, "esc");
            overlay.close();
            return Vec::new();
        }
        Action::OverlayMove(delta) => {
            if let Overlay::Usage { scroll } = overlay {
                scroll_usage(ctx, scroll, delta);
                return Vec::new();
            }
            if let Overlay::Notice { body, scroll, .. } = overlay {
                scroll_text(scroll, delta, body);
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
            let len = overlay_len(ctx, overlay);
            overlay.move_sel(delta, len);
            return Vec::new();
        }
        Action::OverlayAccept => {
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
            if dispatch_slot_key(ctx, overlay, "enter") {
                return Vec::new();
            }
            return accept_overlay(ctx, overlay);
        }
        Action::OverlayChar(c) => {
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
                let typing = matches!(
                    overlay,
                    Overlay::Presets(PresetView::Canvas(s))
                        if s.editing_persona || s.pane == preset_overlay::PresetPane::Catalog
                );
                if typing {
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
            if let Overlay::Ask { selected, picked } = overlay {
                if let Some(i) = c.to_digit(10) {
                    let i = i as usize;
                    if (1..=picked.len().max(9)).contains(&i) {
                        *selected = i - 1;
                        return accept_overlay(ctx, overlay);
                    }
                }
                return Vec::new();
            }
            overlay.push_char(c);
            return Vec::new();
        }
        Action::OverlayBackspace => {
            overlay.backspace();
            return Vec::new();
        }
        Action::OverlayPaste(text) => {
            overlay.push_str(&text);
            return Vec::new();
        }
        Action::OverlaySelect(idx) => {
            overlay.set_selected(idx);
            return Vec::new();
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
                Overlay::Presets(PresetView::Canvas(s))
                    if !s.editing_persona && s.pane == preset_overlay::PresetPane::Catalog
            ) {
                overlay.push_char(' ');
                return Vec::new();
            }
            if let Overlay::Ask { selected, picked } = overlay {
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
                if let Some(SettingsField::Timestamps) = fields.get(*selected).copied() {
                    if let Some(settings) = ctx.get::<AppSettings>(SETTINGS) {
                        settings.toggle_timestamps();
                    }
                }
            }
            return Vec::new();
        }
        Action::OverlayTab => {
            if let Overlay::Presets(view) = overlay {
                view.cycle_pane();
            }
            return Vec::new();
        }
        Action::MouseMove { column, row } => {
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
            return Vec::new();
        }
        Action::MouseDown { column, row } => {
            if queue_pane::hit(queue_hits, column, row).is_some() {
                return Vec::new();
            }
            if goal_pane::hit(goal_hits, column, row).is_some() {
                return Vec::new();
            }
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                scrollback.mouse_down(column, row);
            }
            return Vec::new();
        }
        Action::MouseDrag { column, row } => {
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                scrollback.mouse_drag(column, row);
            }
            return Vec::new();
        }
        Action::MouseUp { column, row } => {
            if let Some(hit) = queue_pane::hit(queue_hits, column, row) {
                return match hit {
                    QueueHit::Send(id) => vec![Effect::PromoteQueued { id: Some(id) }],
                    QueueHit::Edit(id) => vec![Effect::EditQueued { id: Some(id) }],
                };
            }
            if let Some(hit) = goal_pane::hit(goal_hits, column, row) {
                return apply_goal_hit(ctx, overlay, hit);
            }
            if let Some(id) = subagent_dock::hit(dock_hits, column, row) {
                *overlay = Overlay::inspect_subagent(id, false);
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
            return Vec::new();
        }
        Action::PasteClipboard => {
            apply_paste(ctx, overlay, crate::clipboard::paste_from_clipboard());
            return Vec::new();
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
            if let Overlay::Usage { scroll } = overlay {
                scroll_usage(ctx, scroll, delta);
                return Vec::new();
            }
            if let Overlay::Notice { body, scroll, .. } = overlay {
                scroll_text(scroll, delta, body);
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
            return Vec::new();
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
            return Vec::new();
        }
        Action::Click { column, row } => {
            if let Overlay::Inspect { .. } = overlay {
                match inspect_overlay::click(column, row) {
                    InspectClick::Close => return close_inspect(overlay),
                    InspectClick::Toggle | InspectClick::None => return Vec::new(),
                }
            }
            if let Some(hit) = goal_pane::hit(goal_hits, column, row) {
                return apply_goal_hit(ctx, overlay, hit);
            }
            if let Some(id) = subagent_dock::hit(dock_hits, column, row) {
                *overlay = Overlay::inspect_subagent(id, false);
                return Vec::new();
            }
            if overlay.is_open() {
                if overlay::hit_close(hits, column, row) {
                    if matches!(overlay, Overlay::Permission { .. }) {
                        if let Some(perms) = ctx.get::<Permissions>(PERMISSIONS) {
                            perms.resolve(PermissionOptionKind::RejectOnce);
                        }
                    }
                    if matches!(overlay, Overlay::Ask { .. }) {
                        if let Some(ask) = ctx.get::<Ask>(ASK) {
                            ask.cancel();
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
                if let Some(idx) = overlay::hit_index(hits, column, row) {
                    if matches!(overlay, Overlay::Presets(PresetView::Canvas(_))) {
                        let action = match overlay {
                            Overlay::Presets(view) => preset_overlay::apply_hit(ctx, view, idx),
                            _ => PresetAction::None,
                        };
                        apply_preset_action(ctx, overlay, action);
                        return Vec::new();
                    }
                    overlay.set_selected(idx);
                    if !matches!(overlay, Overlay::Settings { .. }) {
                        return accept_overlay(ctx, overlay);
                    }
                }
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
            return Vec::new();
        }
        Action::InsertText(text) => {
            apply_paste(ctx, overlay, crate::clipboard::paste_from_event(&text));
            return Vec::new();
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
            return Vec::new();
        }
        Action::ToggleGoalDetail => {
            if matches!(overlay, Overlay::Goal { .. }) {
                overlay.close();
            } else {
                open_goal_overlay(ctx, overlay, false);
            }
            return Vec::new();
        }
        other => {
            let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) else {
                return Vec::new();
            };
            dispatch(other, &prompt)
        }
    }
}

fn accept_overlay(ctx: &Context, overlay: &mut Overlay) -> Vec<Effect> {
    let ask_answer = match overlay {
        Overlay::Ask { selected, picked } => Some((*selected, picked.clone())),
        _ => None,
    };
    if let Some((selected, picked)) = ask_answer {
        accept_ask(ctx, overlay, selected, picked);
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
            return Vec::new();
        }
        Overlay::Mcps { .. } | Overlay::Workflows { .. } => {
            overlay.close();
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
                        HelpKind::Slash(cmd) => Some(crate::actions::effect_for_slash(cmd, "")),
                        HelpKind::Hint => None,
                    },
                    HelpItem::Extra { command, .. } => extras
                        .iter()
                        .find(|e| e.command == command)
                        .map(|e| crate::actions::effect_for_extra(e, "")),
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
                _ => crate::actions::effect_for_slash(*cmd, &item.insert_text),
            })
        }
        Overlay::None
        | Overlay::Settings { .. }
        | Overlay::Permission { .. }
        | Overlay::Ask { .. }
        | Overlay::PlanApproval { .. }
        | Overlay::Goal { .. }
        | Overlay::Usage { .. }
        | Overlay::Notice { .. }
        | Overlay::Slot { .. }
        | Overlay::Inspect { .. }
        | Overlay::Presets(_) => None,
        Overlay::Tasks { .. } | Overlay::Mcps { .. } | Overlay::Workflows { .. } => None,
    };
    overlay.close();
    effect.into_iter().collect()
}

fn accept_settings(ctx: &Context, overlay: &mut Overlay) {
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
        settings.toggle_timestamps();
    } else {
        *picking = Some(field);
        *selected = 0;
    }
}

fn to_action(
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
            match key.code {
                KeyCode::Char('w') if ctrl => Some(Action::NewSession),
                KeyCode::Char('k') if ctrl => Some(Action::Scroll(1)),
                KeyCode::Char('j') if ctrl => Some(Action::Scroll(-1)),
                KeyCode::Char('u') if ctrl => Some(Action::ScrollPage(1)),
                KeyCode::Char('d') if ctrl => Some(Action::ScrollPage(-1)),
                KeyCode::Char('.') if ctrl => Some(Action::Help),
                KeyCode::Char('x') if ctrl => Some(Action::Help),
                KeyCode::Char('a') if ctrl => Some(Action::MoveBufferStart),
                KeyCode::Char('e') if ctrl => Some(Action::MoveBufferEnd),
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
                    if slash_open(ctx) {
                        // Picker is still choosing a command name: put `/cmd ` in
                        // the composer. Parse runs on the next send, after args.
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

fn is_paste_key(code: KeyCode, ctrl: bool, super_key: bool) -> bool {
    matches!(code, KeyCode::Char('v') | KeyCode::Char('V')) && (ctrl || super_key)
}

fn take_send(ctx: &Context, send_now: bool) -> Action {
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

fn apply_paste(ctx: &Context, overlay: &mut Overlay, payload: crate::clipboard::PastePayload) {
    if overlay.is_open() {
        if let crate::clipboard::PastePayload::Text(text) = payload {
            overlay.push_str(&text);
        }
        return;
    }
    let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) else {
        return;
    };
    match payload {
        crate::clipboard::PastePayload::Empty => flash(ctx, "剪贴板为空"),
        crate::clipboard::PastePayload::Text(text) => prompt.handle_paste(&text),
        crate::clipboard::PastePayload::Image(img) => match prompt.insert_image(img) {
            Ok(_) => {}
            Err(e) => flash(ctx, e),
        },
    }
}

fn prompt_chrome_info(ctx: &Context) -> String {
    let settings = ctx.get::<AppSettings>(SETTINGS);
    let id = settings
        .as_ref()
        .map(|s| s.model())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "dock".into());
    let catalog = settings.as_ref().map(|s| s.catalog()).unwrap_or_default();
    let model = chrome_model_label(&id, &catalog);
    let perm = settings
        .as_ref()
        .map(|s| match s.permission_mode() {
            PermissionMode::Ask => "询问",
            PermissionMode::Allow => "始终允许",
        })
        .unwrap_or("询问");
    let preset = ctx.get::<AgentPresets>(AGENT_PRESETS).map(|p| {
        let cur = p.current();
        let name = cur.name.trim();
        if name.is_empty() {
            cur.id
        } else {
            name.to_string()
        }
    });
    let plan = ctx.get::<PlanMode>(PLAN_MODE).is_some_and(|p| p.active());
    let goal = ctx.get::<Goal>(GOAL).is_some_and(|g| g.present());
    let mut parts = vec![model];
    if let Some(preset) = preset {
        parts.push(preset);
    }
    parts.push(perm.to_string());
    if plan {
        parts.push("计划".into());
    }
    if goal {
        parts.push("目标".into());
    }
    parts.join(" · ")
}

fn chrome_model_label(id: &str, catalog: &[cordis_spine::ModelChoice]) -> String {
    catalog
        .iter()
        .find(|m| m.id == id)
        .map(|m| format_model_display(&m.name, &m.description, id))
        .unwrap_or_else(|| id.to_string())
}

fn format_model_display(name: &str, description: &str, id: &str) -> String {
    let name = name.trim();
    let description = description.trim();
    if name.is_empty() {
        id.to_string()
    } else if description.is_empty() {
        name.to_string()
    } else {
        format!("{name} ({description})")
    }
}

fn overlay_keys(code: KeyCode, ctrl: bool) -> Option<Action> {
    match code {
        KeyCode::Esc => Some(Action::OverlayClose),
        KeyCode::Enter => Some(Action::OverlayAccept),
        KeyCode::Up => Some(Action::OverlayMove(-1)),
        KeyCode::Down => Some(Action::OverlayMove(1)),
        KeyCode::Backspace => Some(Action::OverlayBackspace),
        KeyCode::F(2) => Some(Action::SettingsModal),
        KeyCode::F(3) => Some(Action::ResumePicker),
        KeyCode::Char('.') if ctrl => Some(Action::Help),
        KeyCode::Char('x') if ctrl => Some(Action::Help),
        KeyCode::Char(' ') if !ctrl => Some(Action::OverlaySpace),
        KeyCode::Tab | KeyCode::BackTab => Some(Action::OverlayTab),
        KeyCode::Char(c) if !ctrl => Some(Action::OverlayChar(c)),
        _ => None,
    }
}

fn inner_area(area: Rect) -> Rect {
    Block::default()
        .padding(ratatui::widgets::Padding::new(
            OUTER_HPAD, OUTER_HPAD, OUTER_VPAD, OUTER_VPAD,
        ))
        .inner(area)
}

fn draw(
    ctx: &Context,
    terminal: &mut Terminal<CrosstermBackend<Stderr>>,
    overlay: &Overlay,
    hits: &mut PickerHits,
    dock_hits: &mut Vec<(Rect, String)>,
    goal_hits: &mut Vec<(Rect, GoalHit)>,
    queue_hits: &mut Vec<(Rect, QueueHit)>,
    pointer: (u16, u16),
) -> Result<()> {
    let theme = Theme::current();
    *hits = PickerHits::default();
    dock_hits.clear();
    goal_hits.clear();
    queue_hits.clear();
    terminal
        .draw(|frame| {
            let area = frame.area();
            frame
                .buffer_mut()
                .set_style(area, Style::default().bg(theme.bg_base));
            let inner = inner_area(area);
            let inspect_open = matches!(overlay, Overlay::Inspect { .. });
            let perm_open = matches!(overlay, Overlay::Permission { .. });
            let ask_open = matches!(overlay, Overlay::Ask { .. });
            let plan_open = matches!(overlay, Overlay::PlanApproval { .. });
            let plan_view_only = matches!(
                overlay,
                Overlay::PlanApproval {
                    view_only: true,
                    ..
                }
            );
            let prompt_h = if inspect_open {
                0
            } else if perm_open {
                ctx.get::<Permissions>(PERMISSIONS)
                    .and_then(|p| p.front())
                    .map(|pr| permission_view::chrome_height(&pr, inner.width))
                    .unwrap_or(10)
                    .min(inner.height / 2)
                    .max(8)
            } else if ask_open {
                ctx.get::<Ask>(ASK)
                    .and_then(|a| a.front())
                    .map(|pr| ask_view::chrome_height(&pr, inner.width))
                    .unwrap_or(10)
                    .min(inner.height / 2)
                    .max(8)
            } else if plan_open {
                plan_approval_view::chrome_height(inner.height, plan_view_only)
                    .min(inner.height.saturating_mul(2) / 3)
                    .max(12)
            } else {
                ctx.get::<PromptWidget>(TUI_PROMPT)
                    .map(|p| p.desired_height(inner.width, inner.height / 2))
                    .unwrap_or(3)
            };
            let turn_h = if inspect_open {
                0
            } else {
                status::turn_status_height(ctx, perm_open || ask_open || plan_open)
            };
            let dock_h = if inspect_open {
                0
            } else {
                subagent_dock::desired_height(ctx, inner.width)
            };
            let goal_h = if inspect_open || perm_open || ask_open || plan_open {
                0
            } else {
                goal_pane::desired_height(ctx.get::<Goal>(GOAL).is_some_and(|g| g.present()))
            };
            let queued_items = ctx
                .get::<SessionRef>(SESSION_PORT)
                .map(|s| s.queued_prompts())
                .unwrap_or_default();
            let queue_h = if inspect_open || perm_open || ask_open || plan_open {
                0
            } else {
                queue_pane::desired_height(queued_items.len())
            };
            let chunks = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(turn_h),
                Constraint::Min(1),
                Constraint::Length(dock_h),
                Constraint::Length(goal_h),
                Constraint::Length(queue_h),
                Constraint::Length(prompt_h),
                Constraint::Length(1),
            ])
            .split(inner);
            let scroll_area = chunks[2];
            let dock_area = chunks[3];
            let goal_area = chunks[4];
            let queue_area = chunks[5];
            let prompt_area = chunks[6];
            let shortcuts_area = chunks[7];
            if let Ok(status) = ctx.require::<StatusLine>(TUI_STATUS) {
                let left = status.left();
                let center = status.center();
                let right = status.right();
                let mut bar = StatusBar::new(&left);
                if let Some(c) = center.as_deref() {
                    bar = bar.center(c);
                }
                if let Some(r) = right.as_deref() {
                    bar = bar.right(r);
                }
                frame.render_widget(bar, chunks[0]);
            }
            status::render_turn_status(
                frame.buffer_mut(),
                chunks[1],
                ctx,
                perm_open || ask_open || plan_open,
            );
            let on_welcome = ctx
                .get::<Welcome>(TUI_WELCOME)
                .is_some_and(|w| w.empty_session());
            let resume_on_welcome = on_welcome && matches!(overlay, Overlay::Resume { .. });
            if inspect_open {
                if let Overlay::Inspect {
                    target,
                    scroll,
                    composer,
                    composer_cursor,
                    ..
                } = overlay
                {
                    *hits = inspect_overlay::render(
                        ctx,
                        frame.buffer_mut(),
                        scroll_area,
                        target,
                        *scroll,
                        composer,
                        *composer_cursor,
                    );
                    if matches!(target, InspectTarget::Subagent(_)) {
                        if let Some(pos) =
                            inspect_overlay::composer_cursor(composer, *composer_cursor)
                        {
                            frame.set_cursor_position(pos);
                        }
                    }
                }
            } else if on_welcome && !resume_on_welcome {
                if let Ok(welcome) = ctx.require::<Welcome>(TUI_WELCOME) {
                    frame.render_widget(&*welcome, scroll_area);
                }
            } else if !resume_on_welcome {
                if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                    frame.render_widget(&*scrollback, scroll_area);
                }
            }
            let mut slash_is_open = false;
            let mut files_open = false;
            if !inspect_open {
                *hits = paint_overlay(ctx, frame.buffer_mut(), scroll_area, overlay, on_welcome);
                let selected_sub = match overlay {
                    Overlay::Inspect {
                        target: InspectTarget::Subagent(id),
                        ..
                    } => Some(id.as_str()),
                    _ => None,
                };
                *dock_hits = subagent_dock::paint(ctx, frame.buffer_mut(), dock_area, selected_sub);
                if goal_h > 0 {
                    if let Some(goal) = ctx.get::<Goal>(GOAL) {
                        *goal_hits = goal_pane::paint(
                            frame.buffer_mut(),
                            goal_area,
                            &goal_pane::GoalChrome {
                                title: goal.title(),
                                status: goal.status(),
                                paused: goal.paused(),
                            },
                            pointer,
                            crate::grok::color::shimmer_tick(),
                        );
                    }
                }
                if queue_h > 0 {
                    let working = ctx
                        .get::<SessionRef>(SESSION_PORT)
                        .is_some_and(|s| s.working());
                    *queue_hits = queue_pane::paint(
                        frame.buffer_mut(),
                        queue_area,
                        &queued_items,
                        working,
                        pointer,
                    );
                }
                if perm_open {
                    if let Some(prompt) =
                        ctx.get::<Permissions>(PERMISSIONS).and_then(|p| p.front())
                    {
                        let selected = match overlay {
                            Overlay::Permission { selected } => *selected,
                            _ => 0,
                        };
                        *hits = permission_view::render(
                            frame.buffer_mut(),
                            prompt_area,
                            &prompt,
                            selected,
                        );
                    }
                } else if ask_open {
                    if let Some(prompt) = ctx.get::<Ask>(ASK).and_then(|a| a.front()) {
                        let (selected, picked) = match overlay {
                            Overlay::Ask { selected, picked } => (*selected, picked.clone()),
                            _ => (0, Vec::new()),
                        };
                        *hits = ask_view::render(
                            frame.buffer_mut(),
                            prompt_area,
                            &prompt,
                            selected,
                            &picked,
                        );
                    }
                } else if let Overlay::PlanApproval {
                    selected,
                    view_only,
                    scroll,
                    prompt,
                    wrap,
                } = overlay
                {
                    *hits = plan_approval_view::render(
                        frame.buffer_mut(),
                        prompt_area,
                        prompt,
                        wrap,
                        *selected,
                        *view_only,
                        *scroll,
                    );
                }
                let slash_snap = ctx
                    .get::<PromptWidget>(TUI_PROMPT)
                    .map(|p| p.slash_snapshot());
                let file_snap = ctx
                    .get::<PromptWidget>(TUI_PROMPT)
                    .map(|p| p.file_search_snapshot());
                let blocking = perm_open || ask_open || plan_open;
                slash_is_open = slash_snap.as_ref().is_some_and(|s| s.open) && !blocking;
                files_open =
                    file_snap.as_ref().is_some_and(|s| s.open) && !slash_is_open && !blocking;
                if !blocking {
                    if let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) {
                        prompt.set_info(prompt_chrome_info(ctx));
                        let working = ctx
                            .get::<SessionRef>(SESSION_PORT)
                            .is_some_and(|s| s.working());
                        // Grok unfocuses the empty composer while a turn runs
                        // (`Build anything`, Enter does not send).
                        prompt.set_focused(!working || prompt.can_send());
                        frame.render_widget(&*prompt, prompt_area);
                        if let (Some(snap), true) =
                            (slash_snap.as_ref(), slash_is_open && !overlay.is_open())
                        {
                            let rows = desired_item_rows(&snap.matches, prompt_area.width).max(1);
                            let y = prompt_area.y.saturating_sub(rows).max(scroll_area.y);
                            let drop_area = Rect {
                                x: prompt_area.x,
                                y,
                                width: prompt_area.width,
                                height: prompt_area.y.saturating_sub(y),
                            };
                            render_dropdown(frame.buffer_mut(), drop_area, snap, &theme);
                        } else if let (Some(snap), true) =
                            (file_snap.as_ref(), files_open && !overlay.is_open())
                        {
                            let rows = file_search::desired_rows(snap).max(1);
                            let y = prompt_area.y.saturating_sub(rows).max(scroll_area.y);
                            let drop_area = Rect {
                                x: prompt_area.x,
                                y,
                                width: prompt_area.width,
                                height: prompt_area.y.saturating_sub(y),
                            };
                            let fake = SlashSnapshot {
                                open: true,
                                selected: snap.selected,
                                matches: snap.suggestion_rows(),
                            };
                            render_dropdown(frame.buffer_mut(), drop_area, &fake, &theme);
                        } else if !overlay.is_open() {
                            if let Some(img) = prompt.preview_image() {
                                let h = 6u16.min(scroll_area.height);
                                if h >= 5 {
                                    let card = Rect {
                                        x: scroll_area.x.saturating_add(1),
                                        y: prompt_area.y.saturating_sub(h),
                                        width: scroll_area.width.saturating_sub(2).max(1),
                                        height: h,
                                    };
                                    crate::prompt::paint_image_card(
                                        frame.buffer_mut(),
                                        card,
                                        &img,
                                        &theme,
                                    );
                                }
                            }
                        }
                        if !overlay.is_open() {
                            if let Some(pos) = prompt.cursor_position(prompt_area) {
                                frame.set_cursor_position(pos);
                            }
                        }
                    }
                }
            }
            let mut hints = ctx
                .get::<Shortcuts>(TUI_SHORTCUTS)
                .map(|s| s.hints(overlay, slash_is_open, files_open))
                .unwrap_or_else(|| {
                    crate::shortcuts::idle_hints(
                        ctx.get::<PromptWidget>(TUI_PROMPT)
                            .is_some_and(|p| p.can_send()),
                        ctx.get::<SessionRef>(SESSION_PORT)
                            .is_some_and(|s| s.working()),
                        ctx.get::<SessionRef>(SESSION_PORT)
                            .is_some_and(|s| !s.queued_prompts().is_empty()),
                    )
                });
            if !overlay.is_open() {
                if let Some(slots) = ctx.get::<TuiSlots>(TUI_SLOTS) {
                    for (id, line) in slots.hud_lines() {
                        hints.push(crate::grok::shortcuts::HintItem::new(id, line));
                    }
                }
            }
            frame.render_widget(ShortcutsBar::new(&hints), shortcuts_area);
        })
        .map_err(|e| Error::Message(e.to_string()))?;
    Ok(())
}

fn paint_overlay(
    ctx: &Context,
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    overlay: &Overlay,
    on_welcome: bool,
) -> PickerHits {
    match overlay {
        Overlay::None => PickerHits::default(),
        Overlay::Resume { selected, query } => {
            let archived = ctx
                .get::<Sessions>(SESSIONS)
                .map(|s| s.archived())
                .unwrap_or_default();
            let filtered = filter_sessions(&archived, query);
            let sel = if filtered.is_empty() {
                0
            } else {
                (*selected).min(filtered.len() - 1)
            };
            let rows: Vec<PickerRow> = if filtered.is_empty() {
                vec![PickerRow {
                    label: "(none)",
                    right_label: "No previous sessions",
                    selected: true,
                }]
            } else {
                filtered
                    .iter()
                    .enumerate()
                    .map(|(i, item)| PickerRow {
                        label: item.title.as_str(),
                        right_label: item.id.as_str(),
                        selected: i == sel,
                    })
                    .collect()
            };
            overlay::render_overlay(buf, area, "恢复会话", query, &rows, on_welcome)
        }
        Overlay::Help { selected, query } => {
            let extras = slash_extras(ctx);
            let filtered = filter_help_items(query, &extras);
            let sel = if filtered.is_empty() {
                0
            } else {
                (*selected).min(filtered.len() - 1)
            };
            let rows: Vec<PickerRow> = filtered
                .iter()
                .enumerate()
                .map(|(i, item)| match item {
                    HelpItem::Builtin(row) => PickerRow {
                        label: row.label,
                        right_label: row.key,
                        selected: i == sel,
                    },
                    HelpItem::Extra { key, label, .. } => PickerRow {
                        label: label.as_str(),
                        right_label: key.as_str(),
                        selected: i == sel,
                    },
                })
                .collect();
            overlay::render_overlay(buf, area, "快捷键", query, &rows, false)
        }
        Overlay::History { selected, query } => {
            let history = ctx
                .get::<PromptWidget>(TUI_PROMPT)
                .map(|p| p.history())
                .unwrap_or_default();
            let filtered = filter_strings(&history, query);
            let sel = if filtered.is_empty() {
                0
            } else {
                (*selected).min(filtered.len() - 1)
            };
            let rows: Vec<PickerRow> = if filtered.is_empty() {
                vec![PickerRow {
                    label: "(none)",
                    right_label: "No history yet",
                    selected: true,
                }]
            } else {
                filtered
                    .iter()
                    .enumerate()
                    .map(|(i, (_, text))| PickerRow {
                        label: text,
                        right_label: "",
                        selected: i == sel,
                    })
                    .collect()
            };
            overlay::render_overlay(buf, area, "提示词历史", query, &rows, false)
        }
        Overlay::Find { selected, query } => {
            let matches = ctx
                .get::<Scrollback>(TUI_SCROLLBACK)
                .map(|s| s.find(query))
                .unwrap_or_default();
            let sel = if matches.is_empty() {
                0
            } else {
                (*selected).min(matches.len() - 1)
            };
            let rows: Vec<PickerRow> = if matches.is_empty() {
                vec![PickerRow {
                    label: if query.is_empty() {
                        "type to search"
                    } else {
                        "no matches"
                    },
                    right_label: "",
                    selected: true,
                }]
            } else {
                matches
                    .iter()
                    .enumerate()
                    .map(|(i, (_line, text))| PickerRow {
                        label: text.as_str(),
                        right_label: "",
                        selected: i == sel,
                    })
                    .collect()
            };
            overlay::render_overlay(buf, area, "搜索对话", query, &rows, false)
        }
        Overlay::Args {
            kind,
            selected,
            query,
            ..
        } => {
            let items = filter_args(*kind, query, ctx.get::<AppSettings>(SETTINGS).as_deref());
            let sel = if items.is_empty() {
                0
            } else {
                (*selected).min(items.len() - 1)
            };
            let rows: Vec<PickerRow> = if items.is_empty() {
                vec![PickerRow {
                    label: "(none)",
                    right_label: "no matches",
                    selected: true,
                }]
            } else {
                items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| PickerRow {
                        label: item.display.as_str(),
                        right_label: item.description.as_str(),
                        selected: i == sel,
                    })
                    .collect()
            };
            overlay::render_overlay(buf, area, arg_picker_title(*kind), query, &rows, false)
        }
        Overlay::Settings { selected, picking } => {
            let settings = ctx.get::<AppSettings>(SETTINGS);
            let Some(settings) = settings else {
                return PickerHits::default();
            };
            settings_modal::render(buf, area, *selected, *picking, &settings)
        }
        Overlay::Permission { .. } | Overlay::Ask { .. } | Overlay::PlanApproval { .. } => {
            PickerHits::default()
        }
        Overlay::Tasks {
            selected,
            query,
            collapsed,
        } => {
            let entries = task_entries(ctx, query, collapsed);
            tasks_pane::render_tasks_overlay(buf, area, &entries, *selected, query)
        }
        Overlay::Workflows { selected, query } => {
            let runs = workflow_rows(ctx, query);
            workflows::render_workflows_overlay(buf, area, &runs, *selected, query)
        }
        Overlay::Mcps { selected, query } => {
            let filtered = mcp_rows(ctx, query);
            let sel = if filtered.is_empty() {
                0
            } else {
                (*selected).min(filtered.len() - 1)
            };
            let rows: Vec<PickerRow> = if filtered.is_empty() {
                vec![PickerRow {
                    label: "(none)",
                    right_label: "未配置 MCP 服务器",
                    selected: true,
                }]
            } else {
                filtered
                    .iter()
                    .enumerate()
                    .map(|(i, (left, right))| PickerRow {
                        label: left.as_str(),
                        right_label: right.as_str(),
                        selected: i == sel,
                    })
                    .collect()
            };
            overlay::render_overlay(buf, area, "MCP", query, &rows, false)
        }
        Overlay::Goal {
            selected,
            editing,
            draft,
        } => {
            let goal = ctx.get::<Goal>(GOAL);
            goal_overlay::render(buf, area, goal.as_deref(), *selected, *editing, draft)
        }
        Overlay::Usage { scroll } => {
            let usage = ctx
                .get::<Sessions>(SESSIONS)
                .map(|s| s.prompt_usage())
                .unwrap_or_default();
            usage_overlay::render(buf, area, &usage, *scroll)
        }
        Overlay::Notice {
            title,
            body,
            scroll,
        } => text_overlay::render(buf, area, title, body, *scroll),
        Overlay::Slot { id, scroll } => {
            let slots = ctx.get::<TuiSlots>(TUI_SLOTS);
            let title = slots
                .as_ref()
                .and_then(|s| s.title(id))
                .unwrap_or_else(|| id.clone());
            let body = slots
                .as_ref()
                .and_then(|s| s.render(id))
                .unwrap_or_else(|| format!("slot \"{id}\" is not registered"));
            text_overlay::render(buf, area, &title, &body, *scroll)
        }
        Overlay::Inspect {
            target,
            scroll,
            composer,
            composer_cursor,
            ..
        } => inspect_overlay::render(ctx, buf, area, target, *scroll, composer, *composer_cursor),
        Overlay::Presets(view) => preset_overlay::render(ctx, buf, area, view),
    }
}

fn arg_picker_title(kind: crate::slash::ArgKind) -> &'static str {
    match kind {
        crate::slash::ArgKind::Theme => "主题",
        crate::slash::ArgKind::Model => "模型",
        crate::slash::ArgKind::Settings => "设置",
        crate::slash::ArgKind::Effort => "推理强度",
        crate::slash::ArgKind::LoopInterval => "循环间隔",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_spine::ModelChoice;

    fn choice(id: &str, name: &str, description: &str) -> ModelChoice {
        ModelChoice {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            api_base_url: None,
            api_key: None,
            env_key: None,
            context_window: None,
        }
    }

    #[test]
    fn prompt_chrome_uses_catalog_name_not_api_id() {
        let catalog = [choice(
            "minimax/minimax-m3:free",
            "MiniMax M3",
            "OpenRouter 免费",
        )];
        assert_eq!(
            chrome_model_label("minimax/minimax-m3:free", &catalog),
            "MiniMax M3 (OpenRouter 免费)"
        );
        assert_eq!(
            format_model_display("MiniMax M3", "OpenRouter 免费", "raw"),
            "MiniMax M3 (OpenRouter 免费)"
        );
        assert_eq!(format_model_display("Grok 4", "", "grok-4"), "Grok 4");
        assert_eq!(chrome_model_label("unknown/id", &catalog), "unknown/id");
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
}
