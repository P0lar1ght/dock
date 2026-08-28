//! Main event loop.
//!
//! Terminal takeover copied from grok-build/.../xai-grok-pager/src/app/mod.rs
//! `init_terminal` (fullscreen): raw mode, **stderr** alternate screen, mouse
//! capture, bracketed paste. The loop is still the thin `tokio::select!`
//! from `app/event_loop.rs` — no `MvpAgent`.

use std::io::{stderr, Stderr};
use std::panic;
use std::time::Duration;

use cordis::Context;
use cordis_spine::{
    goal_instruction, AppSettings, Ask, Cron, Goal, Jobs, LogEvent, Mcp, MermaidEngineKind,
    PermissionMode, PermissionOptionKind, Permissions, PlanMode, Sessions, UserImage, ASK,
    ASK_EVENT, CRON, GOAL, GOAL_RESERVED_SUBCOMMANDS, JOBS, MCP, PERMISSIONS, PERMISSION_EVENT,
    PLAN_MODE, SESSIONS, SESSION_EVENT, SETTINGS,
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
use crate::names::{
    SESSION_PORT, TUI_PROMPT, TUI_SCROLLBACK, TUI_SHORTCUTS, TUI_STATUS, TUI_WELCOME,
};
use crate::overlay::{self, filter_help, filter_sessions, filter_strings, HelpKind, Overlay};
use crate::permission_view;
use crate::scrollback::{ClickHit, Scrollback};
use crate::session::SessionRef;
use crate::settings_modal::{self, SettingsField};
use crate::slash::{self, desired_item_rows, filter_args, render_dropdown, SlashSnapshot};
use crate::theme::Theme;

use super::actions::{Action, Effect};
use super::dispatch::dispatch;
use super::input::spawn_reader;
use super::prompt::PromptWidget;
use super::status::{self, StatusLine};
use super::status_bar::StatusBar;
use super::welcome::{Welcome, WelcomeHit};
use crate::shortcuts::Shortcuts;
use crate::grok::mermaid::AffordanceKind;
use crate::mermaid_png;

/// Grok `LayoutConfig::default()` outer padding.
const OUTER_VPAD: u16 = 1;
const OUTER_HPAD: u16 = 2;

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

    let mut input_rx = spawn_reader();
    let mut shimmer = tokio::time::interval(Duration::from_millis(80));
    let mut overlay = Overlay::None;
    let mut hits = PickerHits::default();
    let mut quit = false;
    draw(&ctx, &mut terminal, &overlay, &mut hits)?;

    while !quit {
        tokio::select! {
            biased;
            event = input_rx.recv() => {
                let Some(event) = event else { break };
                if matches!(event, Event::Resize(_, _)) {
                    draw(&ctx, &mut terminal, &overlay, &mut hits)?;
                    continue;
                }
                if let Some(action) = to_action(&ctx, event, &overlay) {
                    for effect in run_action(&ctx, action, &mut overlay, &hits) {
                        match effect {
                            Effect::Quit => quit = true,
                            Effect::NewSession => {
                                if let Ok(sessions) = ctx.require::<Sessions>(SESSIONS) {
                                    sessions.archive_current();
                                    sessions.clear();
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
                                if let Ok(session) = ctx.require::<SessionRef>(SESSION_PORT) {
                                    session.submit(text, send_now);
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
                                flash(&ctx, "已取消");
                            }
                            Effect::EnterPlan { description } => {
                                if let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) {
                                    plan.set(true);
                                }
                                overlay.close();
                                flash(&ctx, "计划模式");
                                if let Some(text) = description {
                                    if let Ok(session) = ctx.require::<SessionRef>(SESSION_PORT) {
                                        session.submit(text, false);
                                    }
                                }
                            }
                            Effect::EnterGoal { objective } => {
                                overlay.close();
                                match objective {
                                    None => flash(&ctx, "用法: /goal <目标>"),
                                    Some(raw) => {
                                        let first = raw
                                            .split_whitespace()
                                            .next()
                                            .unwrap_or("");
                                        if GOAL_RESERVED_SUBCOMMANDS.contains(&first) {
                                            if first == "status" {
                                                let msg = ctx.get::<Goal>(GOAL).map(|g| {
                                                    if g.active() {
                                                        format!("目标进行中: {}", g.title())
                                                    } else {
                                                        "没有活动目标".into()
                                                    }
                                                }).unwrap_or_else(|| "没有活动目标".into());
                                                flash(&ctx, msg);
                                            } else {
                                                flash(&ctx, "用法: /goal <目标>");
                                            }
                                        } else if let Some(goal) = ctx.get::<Goal>(GOAL) {
                                            goal.start(raw.clone());
                                            flash(&ctx, "目标模式");
                                            if let Ok(session) = ctx.require::<SessionRef>(SESSION_PORT) {
                                                session.submit(goal_instruction(&raw), false);
                                            }
                                        } else {
                                            flash(&ctx, "目标服务未挂载");
                                        }
                                    }
                                }
                            }
                            Effect::ShowTasks => {
                                overlay = Overlay::Tasks {
                                    selected: 0,
                                    query: String::new(),
                                };
                            }
                            Effect::ShowMcps => {
                                overlay = Overlay::Mcps {
                                    selected: 0,
                                    query: String::new(),
                                };
                            },
                        }
                    }
                    open_permission_if_needed(&ctx, &mut overlay);
                    open_ask_if_needed(&ctx, &mut overlay);
                    draw(&ctx, &mut terminal, &overlay, &mut hits)?;
                }
            }
            Some(()) = redraw_rx.recv() => {
                open_permission_if_needed(&ctx, &mut overlay);
                open_ask_if_needed(&ctx, &mut overlay);
                draw(&ctx, &mut terminal, &overlay, &mut hits)?;
            }
            _ = shimmer.tick() => {
                if welcome_open(&ctx) || turn_running(&ctx) {
                    draw(&ctx, &mut terminal, &overlay, &mut hits)?;
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

fn open_ask_if_needed(ctx: &Context, overlay: &mut Overlay) {
    if matches!(overlay, Overlay::Permission { .. } | Overlay::Ask { .. }) {
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

fn task_rows(ctx: &Context, query: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(jobs) = ctx.get::<Jobs>(JOBS) {
        for j in jobs.list() {
            let status = if j.done { "完成" } else { "运行中" };
            rows.push((format!("{} {status}", j.id), j.command));
        }
    }
    if let Some(cron) = ctx.get::<Cron>(CRON) {
        for c in cron.list() {
            rows.push((
                format!("{} 每{}s", c.id, c.every.as_secs().max(1)),
                c.prompt,
            ));
        }
    }
    if query.is_empty() {
        return rows;
    }
    let q = query.to_ascii_lowercase();
    rows.into_iter()
        .filter(|(a, b)| {
            a.to_ascii_lowercase().contains(&q) || b.to_ascii_lowercase().contains(&q)
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
        .filter(|(a, b)| {
            a.to_ascii_lowercase().contains(&q) || b.to_ascii_lowercase().contains(&q)
        })
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

fn export_transcript(ctx: &Context, path: &std::path::Path) -> Result<(), String> {
    let sessions = ctx
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| "no sessions".to_string())?;
    let body: String = sessions
        .events()
        .iter()
        .map(|e| format!("{e}\n"))
        .collect();
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
        Overlay::Help { query, .. } => filter_help(query).len(),
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
        Overlay::Ask { picked, .. } => {
            ctx.get::<Ask>(ASK)
                .and_then(|a| a.front())
                .and_then(|p| p.questions.get(p.index).map(ask_view::labels))
                .map(|l| l.len())
                .unwrap_or(picked.len())
        }
        Overlay::Tasks { query, .. } => task_rows(ctx, query).len(),
        Overlay::Mcps { query, .. } => mcp_rows(ctx, query).len(),
    }
}

fn run_action(
    ctx: &Context,
    action: Action,
    overlay: &mut Overlay,
    hits: &PickerHits,
) -> Vec<Effect> {
    match action {
        Action::OverlayClose => {
            if let Overlay::Settings {
                picking,
                selected,
                ..
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
            overlay.close();
            return Vec::new();
        }
        Action::OverlayMove(delta) => {
            let len = overlay_len(ctx, overlay);
            overlay.move_sel(delta, len);
            return Vec::new();
        }
        Action::OverlayAccept => {
            return accept_overlay(ctx, overlay);
        }
        Action::OverlayChar(c) => {
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
        Action::MouseMove { column, row } => {
            if welcome_open(ctx) && !overlay.is_open() {
                if let Ok(welcome) = ctx.require::<Welcome>(TUI_WELCOME) {
                    welcome.set_mouse(column, row);
                }
                return Vec::new();
            }
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                scrollback.set_mouse(column, row);
            }
            return Vec::new();
        }
        Action::PasteClipboard => {
            apply_paste(ctx, overlay, crate::clipboard::paste_from_clipboard());
            return Vec::new();
        }
        Action::Scroll(delta) => {
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
            if overlay.is_open() {
                return Vec::new();
            }
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                scrollback.scroll_page(pages);
            }
            return Vec::new();
        }
        Action::Click { column, row } => {
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
                    overlay.close();
                    return Vec::new();
                }
                if let Some(idx) = overlay::hit_index(hits, column, row) {
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
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                match scrollback.click(column, row) {
                    ClickHit::Mermaid { kind, source } => {
                        return mermaid_click(ctx, kind, source);
                    }
                    ClickHit::ToolToggle | ClickHit::None => {}
                }
            }
            return Vec::new();
        }
        Action::InsertText(text) => {
            apply_paste(ctx, overlay, crate::clipboard::paste_from_event(&text));
            return Vec::new();
        }
        Action::CycleMode => {
            if let Some(settings) = ctx.get::<AppSettings>(SETTINGS) {
                let next = settings.cycle_permission_mode();
                flash(
                    ctx,
                    match next {
                        PermissionMode::Ask => "模式：询问",
                        PermissionMode::Allow => "模式：始终允许",
                    },
                );
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
        _ => {}
    }
    let effect = match overlay {
        Overlay::Resume { selected, query } => ctx.get::<Sessions>(SESSIONS).and_then(|s| {
            filter_sessions(&s.archived(), query)
                .get(*selected)
                .map(|item| Effect::RestoreSession(item.id.clone()))
        }),
        Overlay::Help { selected, query } => overlay::help_at(query, *selected).and_then(|row| {
            match row.kind {
                HelpKind::Slash(cmd) => Some(crate::actions::effect_for_slash(cmd, "")),
                HelpKind::Hint => None,
            }
        }),
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
        Overlay::None | Overlay::Settings { .. } | Overlay::Permission { .. } | Overlay::Ask { .. } => None,
        Overlay::Tasks { .. } | Overlay::Mcps { .. } => None,
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

fn to_action(ctx: &Context, event: Event, overlay: &Overlay) -> Option<Action> {
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
                    if turn_running(ctx) {
                        return Some(Action::CancelTurn);
                    }
                    if file_search_open(ctx) {
                        return Some(Action::FileSearchDismiss);
                    }
                    if let Some(prompt) = ctx.get::<PromptWidget>(TUI_PROMPT) {
                        if !prompt.text().is_empty() {
                            prompt.clear();
                            return None;
                        }
                    }
                    // Grok swallows idle-empty Esc (welcome / no turns). Quit is ctrl+q.
                    None
                }
                KeyCode::Enter if shift || alt => Some(Action::InsertChar('\n')),
                KeyCode::Enter if ctrl => Some(take_send(ctx, true)),
                KeyCode::Enter => {
                    if slash_open(ctx) {
                        if let Some(prompt) = ctx.get::<PromptWidget>(TUI_PROMPT) {
                            if slash::command_for_submit(&prompt.text()).is_some() {
                                return Some(take_send(ctx, false));
                            }
                        }
                        return Some(Action::SlashAccept);
                    }
                    if file_search_open(ctx) {
                        return Some(Action::FileSearchAccept);
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
            MouseEventKind::Down(MouseButton::Left) => Some(Action::Click {
                column: mouse.column,
                row: mouse.row,
            }),
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
    let plan = ctx
        .get::<PlanMode>(PLAN_MODE)
        .is_some_and(|p| p.active());
    let goal = ctx.get::<Goal>(GOAL).is_some_and(|g| g.active());
    let mut parts = vec![model, perm.to_string()];
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
) -> Result<()> {
    let theme = Theme::current();
    *hits = PickerHits::default();
    terminal
        .draw(|frame| {
            let area = frame.area();
            frame
                .buffer_mut()
                .set_style(area, Style::default().bg(theme.bg_base));
            let inner = inner_area(area);
            let perm_open = matches!(overlay, Overlay::Permission { .. });
            let ask_open = matches!(overlay, Overlay::Ask { .. });
            let prompt_h = if perm_open {
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
            } else {
                ctx.get::<PromptWidget>(TUI_PROMPT)
                    .map(|p| p.desired_height(inner.width, inner.height / 2))
                    .unwrap_or(3)
            };
            let turn_h = status::turn_status_height(ctx, perm_open || ask_open);
            let chunks = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(turn_h),
                Constraint::Min(1),
                Constraint::Length(prompt_h),
                Constraint::Length(1),
            ])
            .split(inner);
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
            status::render_turn_status(frame.buffer_mut(), chunks[1], ctx, perm_open || ask_open);
            let on_welcome = ctx
                .get::<Welcome>(TUI_WELCOME)
                .is_some_and(|w| w.empty_session());
            let resume_on_welcome = on_welcome && matches!(overlay, Overlay::Resume { .. });
            if on_welcome && !resume_on_welcome {
                if let Ok(welcome) = ctx.require::<Welcome>(TUI_WELCOME) {
                    frame.render_widget(&*welcome, chunks[2]);
                }
            } else if !resume_on_welcome {
                if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                    frame.render_widget(&*scrollback, chunks[2]);
                }
            }
            *hits = paint_overlay(ctx, frame.buffer_mut(), chunks[2], overlay, on_welcome);
            if perm_open {
                if let Some(prompt) = ctx
                    .get::<Permissions>(PERMISSIONS)
                    .and_then(|p| p.front())
                {
                    let selected = match overlay {
                        Overlay::Permission { selected } => *selected,
                        _ => 0,
                    };
                    *hits = permission_view::render(
                        frame.buffer_mut(),
                        chunks[3],
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
                        chunks[3],
                        &prompt,
                        selected,
                        &picked,
                    );
                }
            }
            let slash_snap = ctx
                .get::<PromptWidget>(TUI_PROMPT)
                .map(|p| p.slash_snapshot());
            let file_snap = ctx
                .get::<PromptWidget>(TUI_PROMPT)
                .map(|p| p.file_search_snapshot());
            let blocking = perm_open || ask_open;
            let slash_is_open = slash_snap.as_ref().is_some_and(|s| s.open) && !blocking;
            let files_open = file_snap.as_ref().is_some_and(|s| s.open) && !slash_is_open && !blocking;
            if !blocking {
                if let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) {
                    prompt.set_info(prompt_chrome_info(ctx));
                    frame.render_widget(&*prompt, chunks[3]);
                    if let (Some(snap), true) =
                        (slash_snap.as_ref(), slash_is_open && !overlay.is_open())
                    {
                        let rows = desired_item_rows(&snap.matches, chunks[3].width).max(1);
                        let y = chunks[3].y.saturating_sub(rows).max(chunks[2].y);
                        let drop_area = Rect {
                            x: chunks[3].x,
                            y,
                            width: chunks[3].width,
                            height: chunks[3].y.saturating_sub(y),
                        };
                        render_dropdown(frame.buffer_mut(), drop_area, snap, &theme);
                    } else if let (Some(snap), true) =
                        (file_snap.as_ref(), files_open && !overlay.is_open())
                    {
                        let rows = file_search::desired_rows(snap).max(1);
                        let y = chunks[3].y.saturating_sub(rows).max(chunks[2].y);
                        let drop_area = Rect {
                            x: chunks[3].x,
                            y,
                            width: chunks[3].width,
                            height: chunks[3].y.saturating_sub(y),
                        };
                        let fake = SlashSnapshot {
                            open: true,
                            selected: snap.selected,
                            matches: snap.suggestion_rows(),
                        };
                        render_dropdown(frame.buffer_mut(), drop_area, &fake, &theme);
                    } else if !overlay.is_open() {
                        if let Some(img) = prompt.preview_image() {
                            let h = 6u16.min(chunks[2].height);
                            if h >= 5 {
                                let card = Rect {
                                    x: chunks[2].x.saturating_add(1),
                                    y: chunks[3].y.saturating_sub(h),
                                    width: chunks[2].width.saturating_sub(2).max(1),
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
                        if let Some(pos) = prompt.cursor_position(chunks[3]) {
                            frame.set_cursor_position(pos);
                        }
                    }
                }
            }
            let hints = ctx
                .get::<Shortcuts>(TUI_SHORTCUTS)
                .map(|s| s.hints(overlay, slash_is_open, files_open))
                .unwrap_or_else(|| {
                    crate::shortcuts::idle_hints(
                        ctx.get::<PromptWidget>(TUI_PROMPT)
                            .is_some_and(|p| p.can_send()),
                        ctx.get::<SessionRef>(SESSION_PORT)
                            .is_some_and(|s| s.working()),
                    )
                });
            frame.render_widget(ShortcutsBar::new(&hints), chunks[4]);
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
            let filtered = filter_help(query);
            let sel = if filtered.is_empty() {
                0
            } else {
                (*selected).min(filtered.len() - 1)
            };
            let rows: Vec<PickerRow> = filtered
                .iter()
                .enumerate()
                .map(|(i, row)| PickerRow {
                    label: row.label,
                    right_label: row.key,
                    selected: i == sel,
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
        Overlay::Settings {
            selected,
            picking,
        } => {
            let settings = ctx.get::<AppSettings>(SETTINGS);
            let Some(settings) = settings else {
                return PickerHits::default();
            };
            settings_modal::render(buf, area, *selected, *picking, &settings)
        }
        Overlay::Permission { .. } | Overlay::Ask { .. } => PickerHits::default(),
        Overlay::Tasks { selected, query } => {
            let filtered = task_rows(ctx, query);
            let sel = if filtered.is_empty() {
                0
            } else {
                (*selected).min(filtered.len() - 1)
            };
            let rows: Vec<PickerRow> = if filtered.is_empty() {
                vec![PickerRow {
                    label: "(none)",
                    right_label: "没有后台或定时任务",
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
            overlay::render_overlay(buf, area, "任务", query, &rows, false)
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
        assert_eq!(
            chrome_model_label("unknown/id", &catalog),
            "unknown/id"
        );
    }
}
