//! Frame layout and overlay painting.

use cordis::Context;
use cordis_spine::{
    AgentPresets, AppSettings, Ask, Browser, Goal, PermissionMode, Permissions, PlanMode, Sessions,
    TuiSlots, AGENT_PRESETS, ASK, BROWSER, GOAL, PERMISSIONS, PLAN_MODE, SESSIONS, SETTINGS,
    TUI_SLOTS,
};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::prelude::CrosstermBackend;
use ratatui::style::Style;
use ratatui::widgets::{Block, Widget};
use ratatui::Terminal;

use crate::error::{Error, Result};
use crate::file_search;
use crate::grok::mcps;
use crate::grok::picker::{render_close_button, PickerHits, PickerRow};
use crate::grok::shortcuts::ShortcutsBar;
use crate::grok::tasks_pane;
use crate::grok::workflows;
use crate::names::{
    GATEWAY, SESSION_PORT, TUI_PROMPT, TUI_SCROLLBACK, TUI_SHORTCUTS, TUI_STATUS, TUI_TABS,
    TUI_WELCOME,
};
use crate::scrollback::Scrollback;
use crate::seam::gateway::GatewayRef;
use crate::seam::session::SessionRef;
use crate::seam::tabs::Tabs;
use crate::slash::{desired_item_rows, filter_args, render_dropdown, SlashSnapshot};
use crate::theme::Theme;
use crate::views::ask_view;
use crate::views::dashboard;
use crate::views::mcp_elicit_view;
use crate::views::overlay::{
    self, filter_help_items, filter_sessions, filter_strings, HelpItem, InspectTarget, Overlay,
};
use crate::views::pairing;
use crate::views::permission_view;
use crate::views::plan_approval_view;
use crate::views::preset_overlay;
use crate::views::queue_pane::{self, QueueHit};
use crate::views::settings_modal;
use crate::views::tab_bar;
use crate::views::task_dock::{self, TaskDockHit};
use crate::views::text_overlay;
use crate::views::usage_overlay;

use super::support::*;
use crate::seam::shortcuts::Shortcuts;
use crate::views::goal_overlay;
use crate::views::goal_pane::{self, GoalHit};
use crate::views::inspect_overlay;
use crate::views::prompt::PromptWidget;
use crate::views::status::{self, StatusLine};
use crate::views::status_bar::StatusBar;
use crate::views::welcome::Welcome;
use std::io::Stderr;

/// Grok `LayoutConfig::default()` outer padding.
const OUTER_VPAD: u16 = 1;
const OUTER_HPAD: u16 = 2;

pub(super) fn prompt_chrome_info(ctx: &Context) -> String {
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
    // Generation timer sits on the dock bottom border (Grok keeps phase
    // status above the prompt; elapsed moves down here).
    let timer = ctx.get::<Sessions>(SESSIONS).and_then(|s| {
        let working = ctx
            .get::<SessionRef>(SESSION_PORT)
            .is_some_and(|h| h.working());
        working
            .then(|| s.turn_elapsed().map(status::format_duration_short))
            .flatten()
    });
    let mut parts = Vec::new();
    if let Some(timer) = timer {
        parts.push(timer);
    }
    parts.push(model);
    // 当前走哪条 wire 紧跟在模型后面：切模型会**重新播种**协议，这两条永远
    // 一起变，贴在一起才读得成一条信息。模型没选（空 id）时不留一个孤零零的
    // 协议名——那时候它不代表任何端点。
    if let Some(settings) = settings.as_ref() {
        if !settings.model().trim().is_empty() {
            parts.push(settings.backend().name().to_string());
        }
    }
    if let Some(preset) = preset {
        parts.push(preset);
    }
    if let Some(settings) = settings.as_ref() {
        if settings.thinking() {
            // 空 = 没指定强度（不发这个参数），底栏就只写「思考」。"medium"
            // 以前是 harness 硬塞的默认值，现在它和别的档一样要显示出来。
            let effort = settings.effort();
            if effort.is_empty() {
                parts.push("思考".into());
            } else {
                parts.push(format!("思考·{effort}"));
            }
        }
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

pub(super) fn chrome_model_label(id: &str, catalog: &[cordis_spine::ModelChoice]) -> String {
    catalog
        .iter()
        .find(|m| m.id == id)
        .map(|m| format_model_display(&m.name, &m.description, id))
        .unwrap_or_else(|| id.to_string())
}

pub(super) fn format_model_display(name: &str, description: &str, id: &str) -> String {
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
pub(super) fn inner_area(area: Rect) -> Rect {
    Block::default()
        .padding(ratatui::widgets::Padding::new(
            OUTER_HPAD, OUTER_HPAD, OUTER_VPAD, OUTER_VPAD,
        ))
        .inner(area)
}

#[allow(clippy::too_many_arguments)]
// TUI 绘制/布局函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到所有调用点，故意保留。
pub(super) fn draw(
    ctx: &Context,
    terminal: &mut Terminal<CrosstermBackend<Stderr>>,
    overlay: &Overlay,
    hits: &mut PickerHits,
    dock_hits: &mut Vec<(Rect, TaskDockHit)>,
    goal_hits: &mut Vec<(Rect, GoalHit)>,
    queue_hits: &mut Vec<(Rect, QueueHit)>,
    tab_hits: &mut Vec<(Rect, tab_bar::TabHit)>,
    pointer: (u16, u16),
) -> Result<()> {
    let theme = Theme::current();
    // 输入框的位置每帧重新认领：这一帧没画它，鼠标就不该还落得进去。
    if let Some(prompt) = ctx.get::<PromptWidget>(TUI_PROMPT) {
        prompt.begin_frame();
    }
    *hits = PickerHits::default();
    dock_hits.clear();
    goal_hits.clear();
    queue_hits.clear();
    tab_hits.clear();
    // 标签栏与旁问面板都活在根上（`Tabs` 不分 realm），当前页的 ctx 一样查得到。
    let tab_rows = ctx
        .get::<Tabs>(TUI_TABS)
        .map(|t| t.list())
        .unwrap_or_default();
    terminal
        .draw(|frame| {
            let area = frame.area();
            frame
                .buffer_mut()
                .set_style(area, Style::default().bg(theme.bg_base));
            let inner = inner_area(area);
            // 会话面板是**独立视图**，吃满整个内区：它自己有抬头（cwd + 汇总）、
            // 动作行、列表、peek 和快捷键条，再叠一层应用状态栏就成了两行 cwd。
            // 别的 overlay 只铺在滚动区上，所以走不到这条。
            if let Overlay::Dashboard {
                selected,
                query,
                collapsed,
                focus,
                composer,
                ..
            } = overlay
            {
                // 擦**整个 area**，不是 `inner`：面板画在内缩 2 列 / 1 行的区域里，
                // 外面那圈 padding 上一帧的字还在（上面只 `set_style` 过，那不擦
                // 字符）。只擦 inner 会在四边留一圈上一屏的残字。
                ratatui::widgets::Clear.render(area, frame.buffer_mut());
                frame
                    .buffer_mut()
                    .set_style(area, Style::default().bg(theme.bg_base));
                let rows = dashboard::build_rows(ctx, query, collapsed);
                *hits = dashboard::render(
                    frame.buffer_mut(),
                    inner,
                    ctx,
                    &dashboard::PanelInput {
                        rows: &rows,
                        selected: *selected,
                        query,
                        focus: *focus,
                        composer,
                    },
                );
                return;
            }
            let inspect_open = matches!(overlay, Overlay::Inspect { .. });
            let perm_open = matches!(overlay, Overlay::Permission { .. });
            let pairing_pending = matches!(overlay, Overlay::PairingPending { .. });
            let ask_open = matches!(overlay, Overlay::Ask { .. });
            let elicit_open = matches!(overlay, Overlay::Elicit { .. });
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
            } else if pairing_pending {
                ctx.get::<GatewayRef>(GATEWAY)
                    .and_then(|g| g.pairing_front())
                    .map(|pr| pairing::chrome_height(&pr, inner.width))
                    .unwrap_or(12)
                    .min(inner.height.saturating_mul(2) / 3)
                    .max(12)
            } else if perm_open {
                ctx.get::<Permissions>(PERMISSIONS)
                    .and_then(|p| p.front())
                    .map(|pr| permission_view::chrome_height(&pr, inner.width))
                    .unwrap_or(10)
                    .min(inner.height / 2)
                    .max(8)
            } else if ask_open {
                let (selected, picked) = match overlay {
                    Overlay::Ask {
                        selected, picked, ..
                    } => (*selected, picked.as_slice()),
                    _ => (0, &[][..]),
                };
                ctx.get::<Ask>(ASK)
                    .and_then(|a| a.front())
                    .map(|pr| ask_view::chrome_height(&pr, inner.width, selected, picked))
                    .unwrap_or(10)
                    .min(inner.height / 2)
                    .max(8)
            } else if elicit_open {
                let (selected, picked) = match overlay {
                    Overlay::Elicit {
                        selected, picked, ..
                    } => (*selected, picked.as_slice()),
                    _ => (0, &[][..]),
                };
                elicit_front(ctx)
                    .map(|pr| mcp_elicit_view::chrome_height(&pr, inner.width, selected, picked))
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
            let modal_open = perm_open || pairing_pending || ask_open || elicit_open || plan_open;
            let turn_h = if inspect_open {
                0
            } else {
                status::turn_status_height(ctx, modal_open)
            };
            // 一帧只收一次：高度和绘制共用这一份。
            let dock_rows = if inspect_open {
                Vec::new()
            } else {
                task_dock_rows(ctx)
            };
            let dock_h = task_dock::desired_height(dock_rows.len());
            let goal_h = if inspect_open || modal_open {
                0
            } else {
                goal_pane::desired_height(ctx.get::<Goal>(GOAL).is_some_and(|g| g.present()))
            };
            let queued_items = ctx
                .get::<SessionRef>(SESSION_PORT)
                .map(|s| s.queued_prompts())
                .unwrap_or_default();
            let queue_h = if inspect_open || modal_open {
                0
            } else {
                queue_pane::desired_height(queued_items.len())
            };
            // 只有一页时不占这一行；全屏 inspect 时也让位。
            let tab_h = if inspect_open || tab_rows.len() < 2 {
                0
            } else {
                1
            };
            let chunks = Layout::vertical([
                Constraint::Length(tab_h),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(dock_h),
                Constraint::Length(goal_h),
                Constraint::Length(queue_h),
                // Grok: turn status sits between scrollback chrome and the composer.
                Constraint::Length(turn_h),
                Constraint::Length(prompt_h),
                Constraint::Length(1),
            ])
            .split(inner);
            let tab_area = chunks[0];
            let status_area = chunks[1];
            let scroll_area = chunks[2];
            let dock_area = chunks[3];
            let goal_area = chunks[4];
            let queue_area = chunks[5];
            let turn_area = chunks[6];
            let prompt_area = chunks[7];
            let shortcuts_area = chunks[8];
            if tab_h > 0 {
                *tab_hits = tab_bar::paint(frame.buffer_mut(), tab_area, &tab_rows);
            }
            if let Ok(status) = ctx.require::<StatusLine>(TUI_STATUS) {
                let left = status.left();
                let center = status.center();
                let right = status.right();
                let mut bar = StatusBar::new(&left);
                if let Some(c) = center.as_deref() {
                    bar = bar.center(c);
                }
                let right_line = status.right_line();
                if let Some(r) = right.as_deref() {
                    // 命中区按纯文本算宽度：悬停态（进度条 + 百分比）和默认态
                    // （数字）等宽，所以这一份宽度对两种形态都成立。
                    status.remember_right_hit(status_area, r);
                    match right_line {
                        Some(line) => bar = bar.right_line(line),
                        None => bar = bar.right(r),
                    }
                }
                frame.render_widget(bar, status_area);
            }
            status::render_turn_status(
                frame.buffer_mut(),
                turn_area,
                ctx,
                perm_open || pairing_pending || ask_open || elicit_open || plan_open,
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
                    hover_close_button(frame.buffer_mut(), hits, pointer);
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
                *hits = paint_overlay(
                    ctx,
                    frame.buffer_mut(),
                    scroll_area,
                    overlay,
                    on_welcome,
                    pointer,
                );
                *dock_hits = task_dock::paint(frame.buffer_mut(), dock_area, &dock_rows);
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
                if pairing_pending {
                    if let Some(prompt) = ctx
                        .get::<GatewayRef>(GATEWAY)
                        .and_then(|g| g.pairing_front())
                    {
                        let selected = match overlay {
                            Overlay::PairingPending { selected } => *selected,
                            _ => 0,
                        };
                        *hits = pairing::render_pending(
                            frame.buffer_mut(),
                            prompt_area,
                            &prompt,
                            selected,
                        );
                    }
                } else if perm_open {
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
                        let (selected, picked, draft, draft_cursor, draft_focused) = match overlay {
                            Overlay::Ask {
                                selected,
                                picked,
                                draft,
                                draft_cursor,
                                draft_focused,
                            } => (
                                *selected,
                                picked.clone(),
                                draft.clone(),
                                *draft_cursor,
                                *draft_focused,
                            ),
                            _ => (0, Vec::new(), String::new(), 0, false),
                        };
                        *hits = ask_view::render(
                            frame.buffer_mut(),
                            prompt_area,
                            &prompt,
                            selected,
                            &picked,
                            ask_view::AskDraft {
                                text: &draft,
                                cursor: draft_cursor,
                                focused: draft_focused,
                            },
                        );
                        // 真实光标要跟到「其他」输入框里。只画反显方块的话终端光标
                        // 还停在别处，输入法候选条就飘到屏幕另一头去了。
                        if let Some(pos) = hits.draft_caret {
                            frame.set_cursor_position(pos);
                        }
                    }
                } else if elicit_open {
                    if let Some(prompt) = elicit_front(ctx) {
                        let (selected, picked, draft) = match overlay {
                            Overlay::Elicit {
                                selected,
                                picked,
                                draft,
                            } => (*selected, picked.clone(), draft.clone()),
                            _ => (0, Vec::new(), String::new()),
                        };
                        *hits = mcp_elicit_view::render(
                            frame.buffer_mut(),
                            prompt_area,
                            &prompt,
                            selected,
                            &picked,
                            &draft,
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
                let blocking = perm_open || pairing_pending || ask_open || elicit_open || plan_open;
                slash_is_open = slash_snap.as_ref().is_some_and(|s| s.open) && !blocking;
                files_open =
                    file_snap.as_ref().is_some_and(|s| s.open) && !slash_is_open && !blocking;
                if !blocking {
                    if let Ok(prompt) = ctx.require::<PromptWidget>(TUI_PROMPT) {
                        prompt.set_info(prompt_chrome_info(ctx));
                        let working = ctx
                            .get::<SessionRef>(SESSION_PORT)
                            .is_some_and(|s| s.working());
                        // Turn-working behavior (unchanged): the empty composer
                        // blurs when a turn starts (`Build anything`, no caret)
                        // and comes back when it ends. Focus is otherwise
                        // event-driven — re-deriving it every frame wiped out
                        // click-to-focus and typing mid-turn.
                        prompt.set_turn_working(working);
                        if prompt.can_send() {
                            prompt.set_focused(true);
                        }
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
                                completing_args: false,
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
                                    crate::views::prompt::paint_image_card(
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
                    crate::seam::shortcuts::idle_hints(
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

pub(super) fn paint_overlay(
    ctx: &Context,
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    overlay: &Overlay,
    on_welcome: bool,
    pointer: (u16, u16),
) -> PickerHits {
    let hits = match overlay {
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
        Overlay::PairingManage { selected } => {
            let gw = ctx.get::<GatewayRef>(GATEWAY);
            let pending = gw.as_ref().map(|g| g.pairing_pending()).unwrap_or_default();
            let bindings = gw
                .as_ref()
                .map(|g| g.pairing_bindings())
                .unwrap_or_default();
            let (status, empty) = pairing::overlay_copy(gw.as_deref());
            pairing::render_manage(
                buf,
                area,
                &pending,
                &bindings,
                *selected,
                Some(status.as_str()).filter(|s| !s.is_empty()),
                empty,
                gw.as_ref().map(|g| g.is_listening()).unwrap_or(false),
                &gw.as_ref()
                    .map(|g| g.local_addr().to_string())
                    .unwrap_or_default(),
            )
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
        Overlay::Permission { .. }
        | Overlay::PairingPending { .. }
        | Overlay::Ask { .. }
        | Overlay::Elicit { .. }
        | Overlay::PlanApproval { .. } => PickerHits::default(),
        Overlay::Tasks {
            selected,
            query,
            collapsed,
        } => {
            let entries = task_entries(ctx, query, collapsed);
            tasks_pane::render_tasks_overlay(buf, area, &entries, *selected, query)
        }
        Overlay::Workflows {
            selected,
            query,
            detail,
            phase,
        } => {
            let runs = workflow_rows(ctx, query);
            // 详情页盯的是一个 run id：列表过滤或排序变了也不该跳到别的 run 上。
            match detail
                .as_deref()
                .and_then(|id| runs.iter().find(|r| r.run_id == id))
            {
                Some(run) => workflows::render_workflow_detail(buf, area, run, *phase),
                None => workflows::render_workflows_overlay(buf, area, &runs, *selected, query),
            }
        }
        // 面板在 `draw` 里就吃满整个内区并提前返回了，走不到这里。
        Overlay::Dashboard { .. } => PickerHits::default(),
        Overlay::Mcps {
            selected,
            query,
            tools_expanded,
            section_collapsed,
        } => {
            let servers = mcp_status_list(ctx);
            let sel = {
                let n = mcps::build_rows(&servers, query, tools_expanded, *section_collapsed).len();
                if n == 0 {
                    0
                } else {
                    (*selected).min(n - 1)
                }
            };
            mcps::render_mcp_overlay(
                buf,
                area,
                &servers,
                sel,
                query,
                tools_expanded,
                *section_collapsed,
            )
        }
        Overlay::Goal {
            selected,
            editing,
            draft,
        } => {
            let goal = ctx.get::<Goal>(GOAL);
            goal_overlay::render(buf, area, goal.as_deref(), *selected, *editing, draft)
        }
        Overlay::Usage {
            tab,
            scroll,
            detail,
        } => usage_overlay::render(buf, area, ctx, *tab, *scroll, *detail),
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
        Overlay::Browser { scroll } => {
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
            let body = ctx
                .get::<Browser>(BROWSER)
                .map(|b| b.format_cockpit(approval.as_deref()))
                .unwrap_or_else(|| "browser 未挂载（tool-browser 插件不在树上）。".into());
            text_overlay::render(buf, area, "浏览器", &body, *scroll)
        }
        Overlay::Computer { scroll, pending } => {
            let body = super::support::computer_cockpit_body(ctx, *pending);
            text_overlay::render(buf, area, "电脑", &body, *scroll)
        }
        Overlay::Inspect {
            target,
            scroll,
            composer,
            composer_cursor,
            ..
        } => inspect_overlay::render(ctx, buf, area, target, *scroll, composer, *composer_cursor),
        Overlay::Presets(view) => preset_overlay::render(ctx, buf, area, view),
    };
    hover_close_button(buf, &hits, pointer);
    hits
}

/// 右上角 `[×]` 的悬停高亮（Grok 的 close button hover）。
///
/// 按钮位置是各覆盖层渲染时自己算出来的（`PickerHits::close_button`），所以这里
/// 拿指针去命中那块矩形，命中就按悬停样式原地重画一遍 —— 让每个覆盖层再抄一遍
/// 那段布局公式，迟早会和真正的按钮位置走散。
pub(super) fn hover_close_button(
    buf: &mut ratatui::buffer::Buffer,
    hits: &PickerHits,
    pointer: (u16, u16),
) {
    let rect = hits.close_button;
    if rect.width == 0 || !rect.contains(Position::new(pointer.0, pointer.1)) {
        return;
    }
    let theme = Theme::current();
    render_close_button(
        buf,
        rect.x,
        rect.y,
        rect.width,
        &theme,
        true,
        Some(theme.bg_base),
    );
}

pub(super) fn arg_picker_title(kind: crate::slash::ArgKind) -> &'static str {
    match kind {
        crate::slash::ArgKind::Tab => "分页",
        crate::slash::ArgKind::Theme => "主题",
        crate::slash::ArgKind::Model => "模型",
        crate::slash::ArgKind::Protocol => "推理协议",
        crate::slash::ArgKind::Settings => "设置",
        crate::slash::ArgKind::Effort => "推理强度",
        crate::slash::ArgKind::LoopInterval => "循环间隔",
        crate::slash::ArgKind::Lsp => "LSP",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_spine::{JobSnapshot, ModelChoice};

    fn choice(id: &str, name: &str, description: &str) -> ModelChoice {
        ModelChoice {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            api_base_url: None,
            api_key: None,
            env_key: None,
            context_window: None,
            api_backends: vec![cordis_spine::ApiBackend::ChatCompletions],
            backend_overrides: Default::default(),
            pricing: None,
            auth_scheme: None,
            api_model: None,
            prompt_cache: None,
            max_output_tokens: None,
            reasoning: None,
            reasoning_effort: None,
            reasoning_efforts: None,
            supports_images: None,
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

    /// 底栏在模型旁边写当前协议：切模型会重新播种协议，两条一起变，分开放
    /// 就会让人以为协议是独立设置的。
    #[test]
    fn prompt_chrome_pairs_the_model_with_its_protocol() {
        let ctx = Context::new();
        let _svc = ctx
            .provide(SETTINGS, AppSettings::new("not-in-any-catalog"))
            .unwrap();
        let line = prompt_chrome_info(&ctx);
        assert!(
            line.starts_with("not-in-any-catalog · responses"),
            "line={line}"
        );
    }

    /// 没选模型（空 id）时不写协议：底栏兜底的 `dock` 不是一个端点，给它配一条
    /// wire 是假信息。
    #[test]
    fn prompt_chrome_skips_the_protocol_without_a_model() {
        let ctx = Context::new();
        let _svc = ctx.provide(SETTINGS, AppSettings::new("")).unwrap();
        let line = prompt_chrome_info(&ctx);
        assert!(line.starts_with("dock · 思考"), "line={line}");
    }

    /// Tasks 覆盖层（以及所有共用 `PickerHits::close_button` 的浮层）右上角的
    /// `[×]`：指针停上去要变亮加粗，移开恢复灰的。
    #[test]
    fn hovering_the_close_button_bolds_it() {
        use ratatui::buffer::Buffer;
        use ratatui::style::Modifier;

        let area = Rect::new(0, 0, 80, 24);
        let job = JobSnapshot {
            id: "job-1".into(),
            command: "echo hi".into(),
            done: false,
            output: String::new(),
            description: Some("say hi".into()),
            is_monitor: false,
            foreground: false,
            start_time: std::time::SystemTime::now(),
        };
        let entries = tasks_pane::collect_items(&[job], &[], &[], &[], None);
        let mut buf = Buffer::empty(area);
        let hits = tasks_pane::render_tasks_overlay(&mut buf, area, &entries, 0, "");
        let button = hits.close_button;
        assert!(button.width > 0, "close button rect");
        let mid = (button.x + 1, button.y);

        hover_close_button(&mut buf, &hits, (button.x - 5, button.y));
        assert!(
            !buf.cell(mid).unwrap().modifier.contains(Modifier::BOLD),
            "指针不在按钮上时不该高亮"
        );

        hover_close_button(&mut buf, &hits, mid);
        assert!(
            buf.cell(mid).unwrap().modifier.contains(Modifier::BOLD),
            "指针压在 [×] 上时必须高亮"
        );
    }
}
