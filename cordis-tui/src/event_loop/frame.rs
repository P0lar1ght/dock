//! Frame layout and overlay painting.

use cordis::Context;
use cordis_spine::{
    AgentPresets, AppSettings, Ask, Goal, PermissionMode, Permissions, PlanMode, Sessions,
    TuiSlots, AGENT_PRESETS, ASK, GOAL, PERMISSIONS, PLAN_MODE, SESSIONS, SETTINGS, TUI_SLOTS,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::prelude::CrosstermBackend;
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui::Terminal;

use crate::ask_view;
use crate::mcp_elicit_view;
use crate::error::{Error, Result};
use crate::file_search;
use crate::grok::mcps;
use crate::grok::picker::{PickerHits, PickerRow};
use crate::grok::shortcuts::ShortcutsBar;
use crate::grok::tasks_pane;
use crate::grok::workflows;
use crate::names::{
    SESSION_PORT, TUI_PROMPT, TUI_SCROLLBACK, TUI_SHORTCUTS, TUI_STATUS, TUI_WELCOME,
};
use crate::overlay::{
    self, filter_help_items, filter_sessions, filter_strings, HelpItem, InspectTarget, Overlay,
};
use crate::permission_view;
use crate::plan_approval_view;
use crate::preset_overlay;
use crate::queue_pane::{self, QueueHit};
use crate::scrollback::Scrollback;
use crate::session::SessionRef;
use crate::settings_modal;
use crate::slash::{desired_item_rows, filter_args, render_dropdown, SlashSnapshot};
use crate::subagent_dock;
use crate::text_overlay;
use crate::theme::Theme;
use crate::usage_overlay;

use super::support::*;
use crate::goal_overlay;
use crate::goal_pane::{self, GoalHit};
use crate::inspect_overlay;
use crate::prompt::PromptWidget;
use crate::shortcuts::Shortcuts;
use crate::status::{self, StatusLine};
use crate::status_bar::StatusBar;
use crate::welcome::Welcome;
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

pub(super) fn draw(
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
            let modal_open = perm_open || ask_open || elicit_open || plan_open;
            let turn_h = if inspect_open {
                0
            } else {
                status::turn_status_height(ctx, modal_open)
            };
            let dock_h = if inspect_open {
                0
            } else {
                subagent_dock::desired_height(ctx, inner.width)
            };
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
                perm_open || ask_open || elicit_open || plan_open,
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
                        let (selected, picked, draft) = match overlay {
                            Overlay::Ask {
                                selected,
                                picked,
                                draft,
                            } => (*selected, picked.clone(), draft.clone()),
                            _ => (0, Vec::new(), String::new()),
                        };
                        *hits = ask_view::render(
                            frame.buffer_mut(),
                            prompt_area,
                            &prompt,
                            selected,
                            &picked,
                            &draft,
                        );
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
                let blocking = perm_open || ask_open || elicit_open || plan_open;
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

pub(super) fn paint_overlay(
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
        Overlay::Permission { .. }
        | Overlay::Ask { .. }
        | Overlay::Elicit { .. }
        | Overlay::PlanApproval { .. } => {
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

pub(super) fn arg_picker_title(kind: crate::slash::ArgKind) -> &'static str {
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
}
