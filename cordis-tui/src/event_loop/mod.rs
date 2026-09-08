//! Main event loop.
//!
//! Terminal takeover copied from grok-build/.../xai-grok-pager/src/app/mod.rs
//! `init_terminal` (fullscreen): raw mode, **stderr** alternate screen, mouse
//! capture, bracketed paste. Overlay keys live in [`keys`], drawing in
//! [`frame`], MCP/goal/inspect helpers in [`support`]. This file is the
//! takeover plus the `select!` / `Effect` loop — no `MvpAgent`.

mod frame;
mod keys;
mod support;

use std::collections::{HashSet, VecDeque};
use std::io::stderr;
use std::panic;
use std::time::{Duration, Instant};

use cordis::Context;
use cordis_spine::{
    goal_composer_fill, lsp_auto_setup, lsp_status_report, AgentPresets, AppSettings, DynamicRunner,
    Goal, LogEvent, LspBackendAdapter, LspSetupScope, PlanMode, Sessions, Slash, ToolCall, Tools,
    AGENT_PRESETS, ASK_EVENT, DYNAMIC_CORDIS_RUNNER, GOAL, LSP, MCP_ELICIT_EVENT, PERMISSION_EVENT,
    PLAN_EVENT, PLAN_MODE, SESSIONS, SESSION_EVENT, SETTINGS, SLASH, TOOLS,
};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::Rect;
use ratatui::prelude::CrosstermBackend;
use ratatui::Terminal;

use crate::error::{Error, Result};
use crate::grok::picker::PickerHits;
use crate::names::{GATEWAY_PAIRING, SESSION_PORT, TUI_PROMPT, TUI_SCROLLBACK, TUI_STATUS};
use crate::overlay::{Overlay, UsageTab};
use crate::queue_pane::QueueHit;
use crate::scrollback::Scrollback;
use crate::session::SessionRef;
use crate::theme::Theme;

use crate::actions::Effect;
use crate::goal_pane::GoalHit;
use crate::input::{drain_events, drain_notifies, spawn_reader};
use crate::mermaid_png;
use crate::prompt::PromptWidget;
use crate::status::StatusLine;
use crate::usage_overlay;

use frame::{chrome_model_label, draw};
use keys::{run_action, to_action};
use support::*;

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
    let redraw_elicit = redraw_tx.clone();
    let redraw_plan = redraw_tx.clone();
    let redraw_pair = redraw_tx.clone();
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
    let _elicit = ctx
        .on(MCP_ELICIT_EVENT, move |_: &()| {
            let _ = redraw_elicit.send(());
        })
        .ok();
    let _plan = ctx
        .on(PLAN_EVENT, move |_: &()| {
            let _ = redraw_plan.send(());
        })
        .ok();
    let _pair = ctx
        .on(GATEWAY_PAIRING, move |_: &()| {
            let _ = redraw_pair.send(());
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
                let Some(first) = event else { break };
                // Trackpad / fast wheel queues dozens of Scroll events; drawing
                // after each one stalls the loop until the burst drains.
                let batch = drain_events(first, &mut input_rx);
                drain_notifies(&mut redraw_rx);
                let mut should_draw = false;
                for event in batch {
                    if matches!(event, Event::Resize(_, _)) {
                        should_draw = true;
                        continue;
                    }
                    if let Event::Mouse(m) = &event {
                        pointer = (m.column, m.row);
                        if matches!(m.kind, MouseEventKind::Moved)
                            && matches!(overlay, Overlay::Usage { .. })
                            && !usage_overlay::hover(m.column, m.row)
                        {
                            continue;
                        }
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
                                Effect::PairingManage => {
                                    overlay = Overlay::PairingManage { selected: 0 };
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
                                    } else if let Some(more) = intercept_loop_send(&text) {
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
                                    if text.contains("/goal") {
                                        if let Some(goal) = ctx.get::<Goal>(GOAL) {
                                            goal.arm_composer();
                                        }
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
                                Effect::ToggleThinking => {
                                    let on = ctx
                                        .get::<AppSettings>(SETTINGS)
                                        .map(|s| s.toggle_thinking())
                                        .unwrap_or(false);
                                    overlay.close();
                                    flash(
                                        &ctx,
                                        if on {
                                            "思考模式已开"
                                        } else {
                                            "思考模式已关"
                                        },
                                    );
                                }
                                Effect::EnterLoop { args } => {
                                    overlay.close();
                                    start_loop(&ctx, args);
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
                                        tools_expanded: HashSet::new(),
                                        section_collapsed: false,
                                    };
                                }
                                Effect::ShowLsp { write, user } => {
                                    let cwd = std::env::current_dir()
                                        .unwrap_or_else(|_| std::path::PathBuf::from("."));
                                    let body = if write {
                                        let scope = if user {
                                            LspSetupScope::User
                                        } else {
                                            LspSetupScope::Project
                                        };
                                        match lsp_auto_setup(&cwd, scope) {
                                            Ok(report) => {
                                                if let Some(handle) =
                                                    ctx.get::<LspBackendAdapter>(LSP)
                                                {
                                                    handle.apply_disk_config_background();
                                                }
                                                report.display()
                                            }
                                            Err(e) => format!("无法写入 LSP 配置：{e}"),
                                        }
                                    } else {
                                        lsp_status_report(&cwd)
                                    };
                                    overlay = Overlay::Notice {
                                        title: "LSP".into(),
                                        body,
                                        scroll: 0,
                                    };
                                }
                                Effect::ShowCordis => {
                                    let sid = ctx
                                        .get::<Sessions>(SESSIONS)
                                        .map(|s| s.identity().to_string())
                                        .unwrap_or_else(|| "main".into());
                                    let body = ctx
                                        .get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
                                        .map(|runner| runner.overlay_listing(&sid))
                                        .unwrap_or_else(|| {
                                            "dynamicCordisRunner 未挂载。".into()
                                        });
                                    overlay = Overlay::Notice {
                                        title: "Cordis 插件".into(),
                                        body,
                                        scroll: 0,
                                    };
                                }
                                Effect::ShowBrowser => {
                                    overlay = Overlay::Browser { scroll: 0 };
                                }
                                Effect::ShowComputer => {
                                    overlay = Overlay::Computer { scroll: 0 };
                                }
                                Effect::ShowPresets { focus } => {
                                    overlay = open_presets_overlay(&ctx, focus);
                                }
                                Effect::ShowUsage => {
                                    overlay = Overlay::usage(UsageTab::Session);
                                }
                                Effect::ShowContext => {
                                    overlay = Overlay::usage(UsageTab::Context);
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
                                Effect::ToggleMcpServer { name, enabled } => {
                                    spawn_mcp_toggle(
                                        ctx.clone(),
                                        redraw_tx.clone(),
                                        McpToggleJob::Server { name, enabled },
                                    );
                                }
                                Effect::ToggleMcpTool {
                                    server,
                                    tool,
                                    enabled,
                                } => {
                                    spawn_mcp_toggle(
                                        ctx.clone(),
                                        redraw_tx.clone(),
                                        McpToggleJob::Tool {
                                            server,
                                            tool,
                                            enabled,
                                        },
                                    );
                                }
                                Effect::McpAuth { name } => {
                                    spawn_mcp_auth(ctx.clone(), redraw_tx.clone(), name);
                                }
                            }
                        }
                        should_draw = true;
                    }
                }
                if should_draw {
                    open_pairing_if_needed(&ctx, &mut overlay);
                    open_permission_if_needed(&ctx, &mut overlay);
                    open_ask_if_needed(&ctx, &mut overlay);
                    open_elicit_if_needed(&ctx, &mut overlay);
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
                drain_notifies(&mut redraw_rx);
                open_pairing_if_needed(&ctx, &mut overlay);
                open_permission_if_needed(&ctx, &mut overlay);
                open_ask_if_needed(&ctx, &mut overlay);
                open_elicit_if_needed(&ctx, &mut overlay);
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
