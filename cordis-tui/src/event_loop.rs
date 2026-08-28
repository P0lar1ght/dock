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
use cordis_spine::{LogEvent, Sessions, SESSIONS, SESSION_EVENT};
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

use crate::error::{Error, Result};
use crate::grok::picker::{PickerHits, PickerRow};
use crate::grok::shortcuts::{HintItem, ShortcutsBar};
use crate::names::{SESSION_PORT, TUI_PROMPT, TUI_SCROLLBACK, TUI_STATUS, TUI_WELCOME};
use crate::overlay::{self, filter_help, filter_sessions, filter_strings, HelpKind, Overlay};
use crate::session::SessionRef;
use crate::slash::{desired_item_rows, render_dropdown};

use super::actions::{Action, Effect};
use super::dispatch::dispatch;
use super::input::spawn_reader;
use super::prompt::PromptWidget;
use super::scrollback::Scrollback;
use super::status::StatusLine;
use super::status_bar::StatusBar;
use super::theme::Theme;
use super::welcome::Welcome;

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
    let _listen = ctx
        .on(SESSION_EVENT, move |_: &LogEvent| {
            let _ = redraw_tx.send(());
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
                                if let Ok(session) = ctx.require::<SessionRef>(SESSION_PORT) {
                                    session.submit(text, send_now);
                                }
                            }
                        }
                    }
                    draw(&ctx, &mut terminal, &overlay, &mut hits)?;
                }
            }
            Some(()) = redraw_rx.recv() => {
                draw(&ctx, &mut terminal, &overlay, &mut hits)?;
            }
            _ = shimmer.tick() => {
                if welcome_open(&ctx) {
                    draw(&ctx, &mut terminal, &overlay, &mut hits)?;
                }
            }
        }
    }
    Ok(())
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
                    overlay.close();
                    return Vec::new();
                }
                if let Some(idx) = overlay::hit_index(hits, column, row) {
                    overlay.set_selected(idx);
                }
                return Vec::new();
            }
            if let Ok(scrollback) = ctx.require::<Scrollback>(TUI_SCROLLBACK) {
                scrollback.click(column, row);
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
    let effect = match overlay {
        Overlay::Resume { selected, query } => ctx.get::<Sessions>(SESSIONS).and_then(|s| {
            filter_sessions(&s.archived(), query)
                .get(*selected)
                .map(|item| Effect::RestoreSession(item.id.clone()))
        }),
        Overlay::Help { selected, query } => overlay::help_at(query, *selected).and_then(|row| {
            match row.kind {
                HelpKind::Slash(cmd) => Some(crate::actions::effect_for_slash(cmd)),
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
        Overlay::None => None,
    };
    overlay.close();
    effect.into_iter().collect()
}

fn to_action(ctx: &Context, event: Event, overlay: &Overlay) -> Option<Action> {
    match event {
        Event::Key(key) => {
            if key.kind == KeyEventKind::Release {
                return None;
            }
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            if matches!(key.code, KeyCode::Char('c' | '\x03')) && ctrl
                || key.code == KeyCode::Char('\x03')
            {
                return Some(Action::Quit);
            }
            if ctrl && matches!(key.code, KeyCode::Char('q')) {
                return Some(Action::Quit);
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
                KeyCode::F(3) => Some(Action::ResumePicker),
                KeyCode::Esc => {
                    if let Some(prompt) = ctx.get::<PromptWidget>(TUI_PROMPT) {
                        if !prompt.text().is_empty() {
                            prompt.clear();
                            return None;
                        }
                    }
                    Some(Action::Quit)
                }
                KeyCode::Enter => {
                    if slash_open(ctx) {
                        return Some(Action::SlashAccept);
                    }
                    let text = ctx
                        .get::<PromptWidget>(TUI_PROMPT)
                        .map(|p| p.take())
                        .unwrap_or_default();
                    Some(Action::SendPrompt(text))
                }
                KeyCode::Tab => {
                    if slash_open(ctx) {
                        Some(Action::SlashInsert)
                    } else {
                        None
                    }
                }
                KeyCode::Up => {
                    if slash_open(ctx) {
                        Some(Action::SlashMove(-1))
                    } else {
                        Some(Action::HistoryPrev)
                    }
                }
                KeyCode::Down => {
                    if slash_open(ctx) {
                        Some(Action::SlashMove(1))
                    } else {
                        Some(Action::HistoryNext)
                    }
                }
                KeyCode::PageUp => Some(Action::ScrollPage(1)),
                KeyCode::PageDown => Some(Action::ScrollPage(-1)),
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Some(Action::InsertChar(c))
                }
                KeyCode::Backspace => Some(Action::Backspace),
                _ => None,
            }
        }
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::ScrollUp => Some(Action::Scroll(3)),
            MouseEventKind::ScrollDown => Some(Action::Scroll(-3)),
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

fn overlay_keys(code: KeyCode, ctrl: bool) -> Option<Action> {
    match code {
        KeyCode::Esc => Some(Action::OverlayClose),
        KeyCode::Enter => Some(Action::OverlayAccept),
        KeyCode::Up => Some(Action::OverlayMove(-1)),
        KeyCode::Down => Some(Action::OverlayMove(1)),
        KeyCode::Backspace => Some(Action::OverlayBackspace),
        KeyCode::F(3) => Some(Action::ResumePicker),
        KeyCode::Char('.') if ctrl => Some(Action::Help),
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

fn shortcut_hints(overlay: &Overlay, slash: bool) -> Vec<HintItem> {
    if overlay.is_open() {
        return vec![
            HintItem::new("enter", "select"),
            HintItem::new("esc", "close"),
            HintItem::new("type", "filter"),
        ];
    }
    if slash {
        return vec![
            HintItem::new("enter", "run"),
            HintItem::new("tab", "insert"),
            HintItem::new("↑↓", "move"),
            HintItem::new("esc", "close"),
        ];
    }
    vec![
        HintItem::new("enter", "send"),
        HintItem::new("f3", "resume"),
        HintItem::new("^.", "help"),
        HintItem::new("^w", "new"),
        HintItem::new("^q", "quit"),
    ]
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
            let prompt_h = ctx
                .get::<PromptWidget>(TUI_PROMPT)
                .map(|p| p.desired_height(inner.width, inner.height / 2))
                .unwrap_or(3);
            let chunks = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
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
            let slash_snap = ctx
                .get::<PromptWidget>(TUI_PROMPT)
                .map(|p| p.slash_snapshot());
            let slash_is_open = slash_snap.as_ref().is_some_and(|s| s.open);
            if let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) {
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
                }
                if !overlay.is_open() {
                    if let Some(pos) = prompt.cursor_position(chunks[3]) {
                        frame.set_cursor_position(pos);
                    }
                }
            }
            let hints = shortcut_hints(overlay, slash_is_open);
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
            overlay::render_overlay(buf, area, "Resume session", query, &rows, on_welcome)
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
            overlay::render_overlay(buf, area, "Shortcuts", query, &rows, false)
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
            overlay::render_overlay(buf, area, "Prompt history", query, &rows, false)
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
            overlay::render_overlay(buf, area, "Find in conversation", query, &rows, false)
        }
    }
}
