//! `/preset` roster + two-pane assembly canvas.
//!
//! Left: live `"tools"` catalog. Right: this preset's toolset. Identity /
//! persona sit above the split. Business state lives on `"agentPresets"`.

use cordis::Context;
use cordis_spine::{
    is_shipped, AgentPreset, AgentPresets, PresetOrigin, ToolSpec, Tools, AGENT_PRESETS, TOOLS,
};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::line_utils::truncate_str;
use crate::grok::picker::{
    render_divider, render_fullscreen_frame, render_header, render_picker_list, render_picker_row,
    PickerHits, PickerRow,
};
use crate::overlay::matches_query;
use crate::theme::Theme;

pub const ASSIGNED_HIT: usize = 10_000;
pub const PERSONA_HIT: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetPane {
    Catalog,
    Assigned,
    Persona,
}

#[derive(Debug, Clone)]
pub struct CanvasState {
    pub id: String,
    pub pane: PresetPane,
    pub catalog_sel: usize,
    pub assigned_sel: usize,
    pub catalog_query: String,
    pub editing_persona: bool,
    pub persona_draft: String,
}

#[derive(Debug, Clone)]
pub enum PresetView {
    Roster { selected: usize },
    Canvas(CanvasState),
}

impl PresetView {
    pub fn roster() -> Self {
        Self::Roster { selected: 0 }
    }

    pub fn cycle_pane(&mut self) {
        let Self::Canvas(c) = self else {
            return;
        };
        if c.editing_persona {
            return;
        }
        c.pane = match c.pane {
            PresetPane::Catalog => PresetPane::Assigned,
            PresetPane::Assigned => PresetPane::Persona,
            PresetPane::Persona => PresetPane::Catalog,
        };
    }
}

pub fn live_specs(ctx: &Context) -> Vec<ToolSpec> {
    ctx.get::<Tools>(TOOLS)
        .map(|t| t.specs())
        .unwrap_or_default()
}

pub fn live_names(ctx: &Context) -> Vec<String> {
    live_specs(ctx).into_iter().map(|s| s.name).collect()
}

pub fn overlay_len(ctx: &Context, view: &PresetView) -> usize {
    match view {
        PresetView::Roster { .. } => list_presets(ctx).len().max(1),
        PresetView::Canvas(c) if c.editing_persona => 0,
        PresetView::Canvas(c) => match c.pane {
            PresetPane::Catalog => catalog_rows(ctx, c).len(),
            PresetPane::Assigned => assigned_names(ctx, c).len(),
            PresetPane::Persona => 0,
        },
    }
}

pub fn list_presets(ctx: &Context) -> Vec<AgentPreset> {
    ctx.get::<AgentPresets>(AGENT_PRESETS)
        .map(|p| p.list())
        .unwrap_or_default()
}

fn catalog_rows<'a>(ctx: &Context, canvas: &CanvasState) -> Vec<ToolSpec> {
    live_specs(ctx)
        .into_iter()
        .filter(|s| {
            matches_query(&s.name, &canvas.catalog_query)
                || matches_query(&s.description, &canvas.catalog_query)
        })
        .collect()
}

fn assigned_names(ctx: &Context, canvas: &CanvasState) -> Vec<String> {
    let live = live_names(ctx);
    ctx.get::<AgentPresets>(AGENT_PRESETS)
        .map(|p| p.assigned_tools(&canvas.id, &live))
        .unwrap_or(live)
}

pub fn open_canvas(ctx: &Context, id: &str) -> Result<PresetView, String> {
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        return Err("Agent 预设服务未挂载".into());
    };
    let preset = presets.apply(id)?;
    Ok(PresetView::Canvas(CanvasState {
        id: preset.id,
        pane: PresetPane::Catalog,
        catalog_sel: 0,
        assigned_sel: 0,
        catalog_query: String::new(),
        editing_persona: false,
        persona_draft: preset.persona,
    }))
}

/// Esc: cancel persona edit → canvas; canvas → roster; roster → close.
/// Returns true when the overlay should close.
pub fn on_esc(ctx: &Context, view: &mut PresetView) -> bool {
    if matches!(view, PresetView::Canvas(c) if c.editing_persona) {
        cancel_persona_edit(ctx, view);
        return false;
    }
    let id = match view {
        PresetView::Canvas(c) => Some(c.id.clone()),
        PresetView::Roster { .. } => None,
    };
    let Some(id) = id else {
        return true;
    };
    let selected = list_presets(ctx)
        .iter()
        .position(|p| p.id == id)
        .unwrap_or(0);
    *view = PresetView::Roster { selected };
    false
}

/// Restore persona draft from the service when cancelling edit.
pub fn cancel_persona_edit(ctx: &Context, view: &mut PresetView) {
    let PresetView::Canvas(c) = view else {
        return;
    };
    if !c.editing_persona {
        return;
    }
    c.persona_draft = ctx
        .get::<AgentPresets>(AGENT_PRESETS)
        .and_then(|p| p.get(&c.id))
        .map(|p| p.persona)
        .unwrap_or_default();
    c.editing_persona = false;
}

pub enum PresetAction {
    None,
    Flash(String),
    OpenCanvas(PresetView),
}

pub fn accept(ctx: &Context, view: &mut PresetView) -> PresetAction {
    match view {
        PresetView::Roster { selected } => {
            let list = list_presets(ctx);
            let Some(preset) = list.get(*selected) else {
                return PresetAction::Flash("没有可选预设".into());
            };
            match open_canvas(ctx, &preset.id) {
                Ok(next) => PresetAction::OpenCanvas(next),
                Err(e) => PresetAction::Flash(e),
            }
        }
        PresetView::Canvas(c) if c.editing_persona => {
            if let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) {
                if let Err(e) = presets.set_persona(&c.id, c.persona_draft.clone()) {
                    return PresetAction::Flash(e);
                }
            }
            c.editing_persona = false;
            PresetAction::Flash("已保存人设".into())
        }
        PresetView::Canvas(c) if c.pane == PresetPane::Persona => {
            c.editing_persona = true;
            PresetAction::None
        }
        PresetView::Canvas(c) if c.pane == PresetPane::Catalog => {
            let rows = catalog_rows(ctx, c);
            let Some(spec) = rows.get(c.catalog_sel) else {
                return PresetAction::None;
            };
            let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
                return PresetAction::Flash("Agent 预设服务未挂载".into());
            };
            match presets.add_tool(&c.id, &spec.name) {
                Ok(()) => PresetAction::None,
                Err(e) => PresetAction::Flash(e),
            }
        }
        PresetView::Canvas(c) => {
            let names = assigned_names(ctx, c);
            let Some(name) = names.get(c.assigned_sel).cloned() else {
                return PresetAction::None;
            };
            let live = live_names(ctx);
            let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
                return PresetAction::Flash("Agent 预设服务未挂载".into());
            };
            match presets.remove_tool(&c.id, &name, &live) {
                Ok(()) => {
                    let next_len = assigned_names(ctx, c).len();
                    if next_len == 0 {
                        c.assigned_sel = 0;
                    } else {
                        c.assigned_sel = c.assigned_sel.min(next_len - 1);
                    }
                    PresetAction::None
                }
                Err(e) => PresetAction::Flash(e),
            }
        }
    }
}

pub fn on_roster_char(ctx: &Context, view: &mut PresetView, c: char) -> PresetAction {
    let PresetView::Roster { selected } = view else {
        return PresetAction::None;
    };
    let list = list_presets(ctx);
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        return PresetAction::Flash("Agent 预设服务未挂载".into());
    };
    match c {
        'n' | 'N' => match presets.create() {
            Ok(p) => match open_canvas(ctx, &p.id) {
                Ok(next) => PresetAction::OpenCanvas(next),
                Err(e) => PresetAction::Flash(e),
            },
            Err(e) => PresetAction::Flash(e),
        },
        'd' | 'D' => {
            let Some(src) = list.get(*selected) else {
                return PresetAction::None;
            };
            match presets.duplicate(&src.id) {
                Ok(p) => match open_canvas(ctx, &p.id) {
                    Ok(next) => PresetAction::OpenCanvas(next),
                    Err(e) => PresetAction::Flash(e),
                },
                Err(e) => PresetAction::Flash(e),
            }
        }
        'x' | 'X' => {
            let Some(src) = list.get(*selected) else {
                return PresetAction::None;
            };
            match presets.delete(&src.id) {
                Ok(()) => {
                    let next = list_presets(ctx);
                    *selected = (*selected).min(next.len().saturating_sub(1));
                    PresetAction::Flash(format!("已删除 {}", src.id))
                }
                Err(e) => PresetAction::Flash(e),
            }
        }
        'a' | 'A' => {
            let Some(src) = list.get(*selected) else {
                return PresetAction::None;
            };
            match presets.apply(&src.id) {
                Ok(p) => PresetAction::Flash(format!("已应用 {}", p.name)),
                Err(e) => PresetAction::Flash(e),
            }
        }
        _ => PresetAction::None,
    }
}

pub fn on_canvas_char(ctx: &Context, view: &mut PresetView, c: char) -> PresetAction {
    let remove = matches!(
        view,
        PresetView::Canvas(s)
            if !s.editing_persona
                && s.pane == PresetPane::Assigned
                && matches!(c, 'd' | 'D' | 'x' | 'X')
    );
    if remove {
        return accept(ctx, view);
    }
    if let PresetView::Canvas(canvas) = view {
        if !canvas.editing_persona && canvas.pane == PresetPane::Persona && matches!(c, 'e' | 'E') {
            canvas.editing_persona = true;
        }
    }
    PresetAction::None
}

pub fn apply_hit(ctx: &Context, view: &mut PresetView, idx: usize) -> PresetAction {
    {
        let PresetView::Canvas(c) = view else {
            return PresetAction::None;
        };
        if idx >= PERSONA_HIT {
            c.pane = PresetPane::Persona;
            return PresetAction::None;
        }
        if idx >= ASSIGNED_HIT {
            c.pane = PresetPane::Assigned;
            c.assigned_sel = idx - ASSIGNED_HIT;
        } else {
            c.pane = PresetPane::Catalog;
            c.catalog_sel = idx;
        }
    }
    accept(ctx, view)
}

pub fn render(ctx: &Context, buf: &mut Buffer, area: Rect, view: &PresetView) -> PickerHits {
    match view {
        PresetView::Roster { selected } => render_roster(ctx, buf, area, *selected),
        PresetView::Canvas(c) => render_canvas(ctx, buf, area, c),
    }
}

fn render_roster(ctx: &Context, buf: &mut Buffer, area: Rect, selected: usize) -> PickerHits {
    let presets = list_presets(ctx);
    let current = ctx
        .get::<AgentPresets>(AGENT_PRESETS)
        .map(|p| p.current_id())
        .unwrap_or_default();
    let sel = if presets.is_empty() {
        0
    } else {
        selected.min(presets.len() - 1)
    };
    let rights: Vec<String> = presets
        .iter()
        .map(|p| {
            if let Some(reason) = &p.broken {
                format!("损坏 · {reason}")
            } else if p.id == current {
                let agents = agent_ids(p);
                match p.origin {
                    PresetOrigin::Shipped => format!("内置 · 当前{agents}"),
                    PresetOrigin::Project => format!("项目 · 当前{agents}"),
                    PresetOrigin::User if is_shipped(&p.id) => format!("覆盖 · 当前{agents}"),
                    PresetOrigin::User => format!("{} · 当前{agents}", p.id),
                }
            } else if !p.description.trim().is_empty() {
                p.description.clone()
            } else {
                match p.origin {
                    PresetOrigin::Shipped => "内置".into(),
                    PresetOrigin::Project => "项目".into(),
                    PresetOrigin::User => p.id.clone(),
                }
            }
        })
        .collect();
    let rows: Vec<PickerRow> = if presets.is_empty() {
        vec![PickerRow {
            label: "(none)",
            right_label: "没有预设",
            selected: true,
        }]
    } else {
        presets
            .iter()
            .zip(rights.iter())
            .enumerate()
            .map(|(i, (p, right))| PickerRow {
                label: p.name.as_str(),
                right_label: right.as_str(),
                selected: i == sel,
            })
            .collect()
    };
    crate::overlay::render_overlay(buf, area, "Agent 预设", "", &rows, false)
}

fn render_canvas(ctx: &Context, buf: &mut Buffer, area: Rect, canvas: &CanvasState) -> PickerHits {
    let theme = Theme::current();
    let Some(frame) = render_fullscreen_frame(buf, area, &theme, Some("组装 Agent"), false)
    else {
        return PickerHits::default();
    };
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        return PickerHits {
            close_button: frame.close_button,
            ..Default::default()
        };
    };
    let preset = presets
        .get(&canvas.id)
        .unwrap_or_else(|| AgentPreset::new(canvas.id.clone()));
    let current = presets.current_id() == canvas.id;
    let mut hits = PickerHits {
        close_button: frame.close_button,
        ..Default::default()
    };

    let content = frame.content;
    if content.height < 6 || content.width < 20 {
        return hits;
    }

    let identity_h = if canvas.editing_persona {
        content.height.saturating_sub(2).min(8).max(3)
    } else {
        6u16.min(content.height.saturating_sub(3)).max(3)
    };
    let chunks =
        Layout::vertical([Constraint::Length(identity_h), Constraint::Min(3)]).split(content);

    let persona_rect = paint_identity(buf, chunks[0], &theme, &preset, canvas, current);
    hits.rows.push((PERSONA_HIT, persona_rect));

    if canvas.editing_persona {
        return hits;
    }

    let cols = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Length(1),
        Constraint::Percentage(50),
    ])
    .split(chunks[1]);
    paint_vsplit(buf, cols[1], &theme);

    let catalog = catalog_rows(ctx, canvas);
    let assigned = assigned_names(ctx, canvas);
    let live = live_specs(ctx);
    let all_tools = preset.tools.is_none();

    let cat_sel = if catalog.is_empty() {
        0
    } else {
        canvas.catalog_sel.min(catalog.len() - 1)
    };
    let asg_sel = if assigned.is_empty() {
        0
    } else {
        canvas.assigned_sel.min(assigned.len() - 1)
    };
    let cat_focus = canvas.pane == PresetPane::Catalog;
    let asg_focus = canvas.pane == PresetPane::Assigned;

    let cat_header = format!("完整目录 · {}", live.len());
    let asg_header = if all_tools {
        format!("工具集 · 全部 · {}", assigned.len())
    } else {
        format!("工具集 · {}", assigned.len())
    };

    let cat_rights: Vec<String> = catalog
        .iter()
        .map(|spec| {
            let brief = brief_desc(&spec.description);
            let on = assigned.iter().any(|n| n == &spec.name);
            match (brief.is_empty(), on) {
                (true, true) => "已加入".into(),
                (true, false) => String::new(),
                (false, true) => format!("{brief} · 已加入"),
                (false, false) => brief,
            }
        })
        .collect();
    let cat_rows: Vec<PickerRow> = catalog
        .iter()
        .zip(cat_rights.iter())
        .enumerate()
        .map(|(i, (spec, right))| PickerRow {
            label: spec.name.as_str(),
            right_label: right.as_str(),
            selected: cat_focus && i == cat_sel,
        })
        .collect();
    let assigned_meta: Vec<(String, String)> = assigned
        .iter()
        .map(|name| {
            let live_hit = live.iter().find(|s| &s.name == name);
            let right = match live_hit {
                None => "未挂载".into(),
                Some(spec) => brief_desc(&spec.description),
            };
            (name.clone(), right)
        })
        .collect();
    let asg_rows: Vec<PickerRow> = assigned_meta
        .iter()
        .enumerate()
        .map(|(i, (name, right))| PickerRow {
            label: name.as_str(),
            right_label: right.as_str(),
            selected: asg_focus && i == asg_sel,
        })
        .collect();

    hits.rows.extend(render_tool_pane(
        buf,
        cols[0],
        &theme,
        &cat_header,
        if cat_focus {
            canvas.catalog_query.as_str()
        } else {
            ""
        },
        cat_focus,
        &cat_rows,
        0,
    ));
    hits.rows.extend(render_tool_pane(
        buf,
        cols[2],
        &theme,
        &asg_header,
        "",
        false,
        &asg_rows,
        ASSIGNED_HIT,
    ));
    hits
}

fn paint_identity(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    preset: &AgentPreset,
    canvas: &CanvasState,
    current: bool,
) -> Rect {
    if area.height == 0 || area.width == 0 {
        return area;
    }
    let title_style = Style::default()
        .fg(theme.accent_skill)
        .add_modifier(Modifier::BOLD);
    let badge = match (preset.origin, current) {
        (PresetOrigin::Shipped, true) => " · 内置 · 当前",
        (PresetOrigin::Shipped, false) => " · 内置",
        (PresetOrigin::Project, true) => " · 项目 · 当前",
        (PresetOrigin::Project, false) => " · 项目",
        (PresetOrigin::User, true) if is_shipped(&preset.id) => " · 覆盖 · 当前",
        (PresetOrigin::User, true) => " · 当前",
        (PresetOrigin::User, false) => "",
    };
    let title = format!(" {}  [{}]{badge} ", preset.name, preset.id);
    buf.set_line(
        area.x,
        area.y,
        &Line::from(Span::styled(
            truncate_str(&title, area.width as usize),
            title_style,
        )),
        area.width,
    );
    let mut y = area.y.saturating_add(1);
    if y < area.y + area.height && !preset.description.trim().is_empty() {
        buf.set_line(
            area.x,
            y,
            &Line::from(Span::styled(
                truncate_str(preset.description.trim(), area.width as usize),
                Style::default().fg(theme.gray),
            )),
            area.width,
        );
        y = y.saturating_add(1);
    }
    if y < area.y + area.height && !preset.agents.is_empty() {
        let ids: Vec<&str> = preset.agents.keys().map(|s| s.as_str()).collect();
        buf.set_line(
            area.x,
            y,
            &Line::from(Span::styled(
                truncate_str(&format!("子代理 {}", ids.join(" / ")), area.width as usize),
                Style::default().fg(theme.gray),
            )),
            area.width,
        );
        y = y.saturating_add(1);
    }
    let persona_y = y.min(area.y + area.height.saturating_sub(1));
    let persona_h = area.y + area.height - persona_y;
    let persona_rect = Rect {
        x: area.x,
        y: persona_y,
        width: area.width,
        height: persona_h.max(1),
    };
    let focused = canvas.pane == PresetPane::Persona || canvas.editing_persona;
    let bg = if focused {
        theme.bg_visual
    } else {
        theme.bg_base
    };
    buf.set_style(persona_rect, Style::default().bg(bg));
    let body = if canvas.editing_persona {
        canvas.persona_draft.as_str()
    } else if preset.persona.trim().is_empty() {
        "（无人设 · Tab 到人设后 Enter 编辑）"
    } else {
        preset.persona.trim()
    };
    let wrapped = textwrap::wrap(body, area.width.max(1) as usize);
    for (i, line) in wrapped.iter().enumerate() {
        if i as u16 >= persona_h {
            break;
        }
        buf.set_line(
            area.x,
            persona_y + i as u16,
            &Line::from(Span::styled(
                line.as_ref(),
                Style::default().fg(theme.text_primary).bg(bg),
            )),
            area.width,
        );
    }
    persona_rect
}

fn paint_vsplit(buf: &mut Buffer, area: Rect, theme: &Theme) {
    let style = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
    for row in 0..area.height {
        if let Some(cell) = buf.cell_mut((area.x, area.y + row)) {
            cell.set_char('\u{2502}');
            cell.set_style(style);
        }
    }
}

fn render_tool_pane(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    header: &str,
    query: &str,
    show_search: bool,
    rows: &[PickerRow<'_>],
    hit_base: usize,
) -> Vec<(usize, Rect)> {
    if area.height < 3 || area.width < 8 {
        return Vec::new();
    }
    render_header(buf, area.x, area.y, area.width, theme, header);
    let rest = Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: area.height.saturating_sub(1),
    };
    if show_search {
        let inner_hits = render_picker_list(buf, rest, theme, query, rows, 0);
        return inner_hits
            .into_iter()
            .map(|(i, r)| (hit_base + i, r))
            .collect();
    }
    if rest.height < 2 {
        return Vec::new();
    }
    render_divider(buf, rest.x, rest.y, rest.width, theme, Some(theme.bg_base));
    let list_y = rest.y.saturating_add(1);
    let list_h = rest.height.saturating_sub(1) as usize;
    let selected = rows.iter().position(|r| r.selected).unwrap_or(0);
    let start = selected
        .saturating_sub(list_h.saturating_sub(1) / 2)
        .min(rows.len().saturating_sub(list_h));
    let mut hits = Vec::new();
    if rows.is_empty() {
        buf.set_line(
            rest.x,
            list_y,
            &Line::from(Span::styled(" （空） ", Style::default().fg(theme.gray))),
            rest.width,
        );
        return hits;
    }
    for vis in 0..list_h {
        let idx = start + vis;
        if idx >= rows.len() {
            break;
        }
        let y = list_y + vis as u16;
        render_picker_row(buf, rest.x, y, rest.width, theme, &rows[idx]);
        hits.push((
            hit_base + idx,
            Rect {
                x: rest.x,
                y,
                width: rest.width,
                height: 1,
            },
        ));
    }
    hits
}

fn agent_ids(preset: &AgentPreset) -> String {
    if preset.agents.is_empty() {
        String::new()
    } else {
        format!(
            " · {}",
            preset.agents.keys().cloned().collect::<Vec<_>>().join("/")
        )
    }
}

/// First sentence of a tool description for the picker right-hand column.
fn brief_desc(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let end = line
        .char_indices()
        .find(|(_, c)| matches!(c, '。' | '！' | '？' | '.' | '!' | '?'))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(line.len());
    line[..end].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::brief_desc;

    #[test]
    fn brief_desc_takes_first_sentence() {
        assert_eq!(
            brief_desc("List directory contents."),
            "List directory contents."
        );
        assert_eq!(
            brief_desc("Read a file.\n- By default reads up to 1000 lines."),
            "Read a file."
        );
        assert_eq!(brief_desc("你是助手。后面还有。"), "你是助手。");
    }
}
