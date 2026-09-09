//! `/preset` roster + two-pane assembly canvas.
//!
//! Left: live tools **not yet** in this preset (or role). Right: already
//! assigned. Single click selects; double-click or Enter adds/removes.
//! Roles pane edits `agents/<id>.yml` (tools + persona) without YAML-only
//! workflow. Deferred (`register_deferred` / `search_tool`) tools are badged.

use std::time::{Duration, Instant};

use cordis::Context;
use cordis_spine::{
    is_shipped, AgentPreset, AgentPresets, PresetOrigin, SubagentDef, ToolSpec, Tools, AGENT_PRESETS,
    TOOLS,
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
pub const ROLES_HIT: usize = 30_000;

const DOUBLE_CLICK: Duration = Duration::from_millis(450);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetPane {
    Catalog,
    Assigned,
    Roles,
    Persona,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleNamingStep {
    Id,
    Name,
}

/// Draft for creating (`from: None`) or renaming (`from: Some(old_id)`) a role.
#[derive(Debug, Clone)]
pub struct RoleNamingDraft {
    pub from: Option<String>,
    pub step: RoleNamingStep,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct CanvasState {
    pub id: String,
    pub pane: PresetPane,
    pub catalog_sel: usize,
    pub assigned_sel: usize,
    pub roles_sel: usize,
    pub catalog_query: String,
    pub editing_persona: bool,
    pub persona_draft: String,
    /// When set, dual panes + persona edit this `agents/<id>` role.
    pub editing_role: Option<String>,
    /// Roles naming draft (id → name), similar to persona typing.
    pub naming_role: Option<RoleNamingDraft>,
    /// Last mouse hit index + time for double-click (anti-misclick).
    pub last_click: Option<(usize, Instant)>,
}

impl CanvasState {
    pub fn naming_active(&self) -> bool {
        self.naming_role.is_some()
    }
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
        if c.editing_persona || c.naming_role.is_some() {
            return;
        }
        c.pane = match c.pane {
            PresetPane::Catalog => PresetPane::Assigned,
            PresetPane::Assigned => {
                if c.editing_role.is_some() {
                    PresetPane::Persona
                } else {
                    PresetPane::Roles
                }
            }
            PresetPane::Roles => PresetPane::Persona,
            PresetPane::Persona => PresetPane::Catalog,
        };
    }
}

pub fn live_specs(ctx: &Context) -> Vec<ToolSpec> {
    ctx.get::<Tools>(TOOLS)
        .map(|t| {
            t.specs()
                .into_iter()
                .filter(|s| !t.is_mcp(&s.name))
                .collect()
        })
        .unwrap_or_default()
}

pub fn live_names(ctx: &Context) -> Vec<String> {
    live_specs(ctx).into_iter().map(|s| s.name).collect()
}


fn tool_kind_label(ctx: &Context, name: &str) -> &'static str {
    match ctx.get::<Tools>(TOOLS) {
        Some(t) if t.is_mcp(name) => "MCP",
        Some(t) if t.is_deferred(name) => "延迟",
        _ => "常驻",
    }
}

pub fn overlay_len(ctx: &Context, view: &PresetView) -> usize {
    match view {
        PresetView::Roster { .. } => list_presets(ctx).len().max(1),
        PresetView::Canvas(c) if c.editing_persona || c.naming_role.is_some() => 0,
        PresetView::Canvas(c) => match c.pane {
            PresetPane::Catalog => catalog_rows(ctx, c).len(),
            PresetPane::Assigned => assigned_names(ctx, c).len(),
            PresetPane::Roles => role_ids(ctx, c).len().max(1),
            PresetPane::Persona => 0,
        },
    }
}

pub fn list_presets(ctx: &Context) -> Vec<AgentPreset> {
    ctx.get::<AgentPresets>(AGENT_PRESETS)
        .map(|p| p.list())
        .unwrap_or_default()
}

/// Left pane: full live catalog minus tools already assigned (∪ with right = full set).
fn catalog_rows(ctx: &Context, canvas: &CanvasState) -> Vec<ToolSpec> {
    let assigned = assigned_names(ctx, canvas);
    live_specs(ctx)
        .into_iter()
        .filter(|s| !assigned.iter().any(|n| n == &s.name))
        .filter(|s| {
            matches_query(&s.name, &canvas.catalog_query)
                || matches_query(&s.description, &canvas.catalog_query)
        })
        .collect()
}

fn assigned_names(ctx: &Context, canvas: &CanvasState) -> Vec<String> {
    let live = live_names(ctx);
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        return live;
    };
    match &canvas.editing_role {
        Some(role) => presets.assigned_subagent_tools(&canvas.id, role, &live),
        None => presets.assigned_tools(&canvas.id, &live),
    }
}

fn role_ids(ctx: &Context, canvas: &CanvasState) -> Vec<String> {
    ctx.get::<AgentPresets>(AGENT_PRESETS)
        .and_then(|p| p.get(&canvas.id))
        .map(|p| p.agents.keys().cloned().collect())
        .unwrap_or_default()
}

fn role_all_tools(ctx: &Context, canvas: &CanvasState) -> bool {
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        return true;
    };
    let Some(preset) = presets.get(&canvas.id) else {
        return true;
    };
    match &canvas.editing_role {
        Some(role) => preset
            .agents
            .get(role)
            .map(|d| d.tools.is_none())
            .unwrap_or(true),
        None => preset.tools.is_none(),
    }
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
        roles_sel: 0,
        catalog_query: String::new(),
        editing_persona: false,
        persona_draft: preset.persona,
        editing_role: None,
        naming_role: None,
        last_click: None,
    }))
}


fn start_create_naming(canvas: &mut CanvasState) -> PresetAction {
    canvas.naming_role = Some(RoleNamingDraft {
        from: None,
        step: RoleNamingStep::Id,
        id: String::new(),
        name: String::new(),
    });
    PresetAction::Flash("输入角色 id · Enter 下一步".into())
}

fn start_rename_naming(ctx: &Context, canvas: &mut CanvasState) -> PresetAction {
    let roles = role_ids(ctx, canvas);
    let Some(role) = roles.get(canvas.roles_sel).cloned() else {
        return PresetAction::Flash("没有可改名子代理".into());
    };
    let name = ctx
        .get::<AgentPresets>(AGENT_PRESETS)
        .and_then(|p| p.get(&canvas.id))
        .and_then(|p| p.agents.get(&role).cloned())
        .map(|d| {
            if d.name.trim().is_empty() {
                role.clone()
            } else {
                d.name
            }
        })
        .unwrap_or_else(|| role.clone());
    canvas.naming_role = Some(RoleNamingDraft {
        from: Some(role.clone()),
        step: RoleNamingStep::Id,
        id: role,
        name,
    });
    PresetAction::Flash("改名 · 编辑 id 后 Enter，再编辑显示名".into())
}

fn confirm_role_naming(ctx: &Context, canvas: &mut CanvasState) -> PresetAction {
    let Some(draft) = canvas.naming_role.clone() else {
        return PresetAction::None;
    };
    match draft.step {
        RoleNamingStep::Id => {
            let id = draft.id.trim().to_string();
            if id.is_empty() {
                return PresetAction::Flash("角色 id 不能为空".into());
            }
            if let Some(d) = canvas.naming_role.as_mut() {
                d.id = id.clone();
                if d.name.trim().is_empty() {
                    d.name = id;
                }
                d.step = RoleNamingStep::Name;
            }
            PresetAction::Flash("输入显示名 · Enter 确认".into())
        }
        RoleNamingStep::Name => {
            let id = draft.id.trim().to_string();
            let name = {
                let n = draft.name.trim();
                if n.is_empty() {
                    id.clone()
                } else {
                    n.to_string()
                }
            };
            let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
                return PresetAction::Flash("Agent 预设服务未挂载".into());
            };
            let result = match &draft.from {
                None => {
                    if presets
                        .get(&canvas.id)
                        .map(|p| p.agents.contains_key(&id))
                        .unwrap_or(false)
                    {
                        return PresetAction::Flash(format!("子代理 id 已存在: {id}"));
                    }
                    let def = SubagentDef {
                        name: name.clone(),
                        ..SubagentDef::default()
                    };
                    presets.upsert_subagent(&canvas.id, &id, def)
                }
                Some(from) if from == &id => {
                    presets.rename_subagent(&canvas.id, from, &id, name.clone())
                }
                Some(from) => presets.rename_subagent(&canvas.id, from, &id, name.clone()),
            };
            if let Err(e) = result {
                return PresetAction::Flash(e);
            }
            canvas.naming_role = None;
            let roles = role_ids(ctx, canvas);
            canvas.roles_sel = roles.iter().position(|r| r == &id).unwrap_or(0);
            match &draft.from {
                None => enter_role_editor(ctx, canvas, &id),
                Some(_) => PresetAction::Flash(format!("已改名 {id}（{name}）")),
            }
        }
    }
}

fn enter_role_editor(ctx: &Context, canvas: &mut CanvasState, role_id: &str) -> PresetAction {
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        return PresetAction::Flash("Agent 预设服务未挂载".into());
    };
    let Some(preset) = presets.get(&canvas.id) else {
        return PresetAction::Flash(format!("没有预设 {}", canvas.id));
    };
    let Some(def) = preset.agents.get(role_id) else {
        return PresetAction::Flash(format!("没有子代理 {role_id}"));
    };
    canvas.editing_role = Some(role_id.to_string());
    canvas.persona_draft = def.persona.clone();
    canvas.editing_persona = false;
    canvas.naming_role = None;
    canvas.pane = PresetPane::Catalog;
    canvas.catalog_sel = 0;
    canvas.assigned_sel = 0;
    canvas.catalog_query.clear();
    PresetAction::Flash(format!("编辑子代理 {role_id}"))
}

/// Esc: cancel naming draft → persona edit → role editor → canvas; canvas → roster; roster → close.
/// Returns true when the overlay should close.
pub fn on_esc(ctx: &Context, view: &mut PresetView) -> bool {
    if let PresetView::Canvas(c) = view {
        if c.naming_role.is_some() {
            c.naming_role = None;
            return false;
        }
    }
    if matches!(view, PresetView::Canvas(c) if c.editing_persona) {
        cancel_persona_edit(ctx, view);
        return false;
    }
    if let PresetView::Canvas(c) = view {
        if c.editing_role.is_some() {
            leave_role_editor(ctx, c);
            return false;
        }
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

fn leave_role_editor(ctx: &Context, canvas: &mut CanvasState) {
    canvas.editing_role = None;
    canvas.editing_persona = false;
    canvas.naming_role = None;
    canvas.pane = PresetPane::Roles;
    canvas.catalog_query.clear();
    canvas.persona_draft = ctx
        .get::<AgentPresets>(AGENT_PRESETS)
        .and_then(|p| p.get(&canvas.id))
        .map(|p| p.persona)
        .unwrap_or_default();
}

/// Restore persona draft from the service when cancelling edit.
pub fn cancel_persona_edit(ctx: &Context, view: &mut PresetView) {
    let PresetView::Canvas(c) = view else {
        return;
    };
    if !c.editing_persona {
        return;
    }
    c.persona_draft = match &c.editing_role {
        Some(role) => ctx
            .get::<AgentPresets>(AGENT_PRESETS)
            .and_then(|p| p.get(&c.id))
            .and_then(|p| p.agents.get(role).cloned())
            .map(|d| d.persona)
            .unwrap_or_default(),
        None => ctx
            .get::<AgentPresets>(AGENT_PRESETS)
            .and_then(|p| p.get(&c.id))
            .map(|p| p.persona)
            .unwrap_or_default(),
    };
    c.editing_persona = false;
}

#[derive(Debug)]
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
        PresetView::Canvas(c) if c.naming_role.is_some() => confirm_role_naming(ctx, c),
        PresetView::Canvas(c) if c.editing_persona => {
            let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
                return PresetAction::Flash("Agent 预设服务未挂载".into());
            };
            let result = match &c.editing_role {
                Some(role) => presets.set_subagent_persona(&c.id, role, c.persona_draft.clone()),
                None => presets.set_persona(&c.id, c.persona_draft.clone()),
            };
            if let Err(e) = result {
                return PresetAction::Flash(e);
            }
            c.editing_persona = false;
            PresetAction::Flash("已保存人设".into())
        }
        PresetView::Canvas(c) if c.pane == PresetPane::Persona => {
            c.editing_persona = true;
            PresetAction::None
        }
        PresetView::Canvas(c) if c.pane == PresetPane::Roles && c.editing_role.is_none() => {
            let roles = role_ids(ctx, c);
            let Some(role) = roles.get(c.roles_sel).cloned() else {
                return PresetAction::Flash("没有子代理 · 按 n 命名新建".into());
            };
            enter_role_editor(ctx, c, &role)
        }
        PresetView::Canvas(c) if c.pane == PresetPane::Catalog => {
            let rows = catalog_rows(ctx, c);
            let Some(spec) = rows.get(c.catalog_sel) else {
                return PresetAction::None;
            };
            let name = spec.name.clone();
            let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
                return PresetAction::Flash("Agent 预设服务未挂载".into());
            };
            let result = match &c.editing_role {
                Some(role) => presets.add_subagent_tool(&c.id, role, &name),
                None => presets.add_tool(&c.id, &name),
            };
            match result {
                Ok(()) => {
                    // After add, item leaves left pane — keep selection in range.
                    let next_len = catalog_rows(ctx, c).len();
                    if next_len == 0 {
                        c.catalog_sel = 0;
                    } else {
                        c.catalog_sel = c.catalog_sel.min(next_len - 1);
                    }
                    PresetAction::Flash(format!("已加入 {name}"))
                }
                Err(e) => PresetAction::Flash(e),
            }
        }
        PresetView::Canvas(c) if c.pane == PresetPane::Assigned => {
            let names = assigned_names(ctx, c);
            let Some(name) = names.get(c.assigned_sel).cloned() else {
                return PresetAction::None;
            };
            let live = live_names(ctx);
            let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
                return PresetAction::Flash("Agent 预设服务未挂载".into());
            };
            let result = match &c.editing_role {
                Some(role) => presets.remove_subagent_tool(&c.id, role, &name, &live),
                None => presets.remove_tool(&c.id, &name, &live),
            };
            match result {
                Ok(()) => {
                    let next_len = assigned_names(ctx, c).len();
                    if next_len == 0 {
                        c.assigned_sel = 0;
                    } else {
                        c.assigned_sel = c.assigned_sel.min(next_len - 1);
                    }
                    PresetAction::Flash(format!("已移除 {name}"))
                }
                Err(e) => PresetAction::Flash(e),
            }
        }
        PresetView::Canvas(_) => PresetAction::None,
    }
}


fn resync_action(ctx: &Context, view: &mut PresetView) -> PresetAction {
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        return PresetAction::Flash("Agent 预设服务未挂载".into());
    };
    presets.resync();
    if let PresetView::Canvas(c) = view {
        let mode_id = c.id.clone();
        if presets.get(&mode_id).is_none() {
            *view = PresetView::roster();
            return PresetAction::Flash(format!(
                "已刷新；模式 {mode_id} 已不在磁盘，回到列表"
            ));
        }
        if let Some(role) = c.editing_role.clone() {
            if !role_ids(ctx, c).iter().any(|r| r == &role) {
                c.editing_role = None;
                c.pane = PresetPane::Roles;
            }
        }
        let roles = role_ids(ctx, c);
        c.roles_sel = c.roles_sel.min(roles.len().saturating_sub(1));
        let assigned = assigned_names(ctx, c);
        c.assigned_sel = c.assigned_sel.min(assigned.len().saturating_sub(1));
        let catalog = catalog_rows(ctx, c);
        c.catalog_sel = c.catalog_sel.min(catalog.len().saturating_sub(1));
    }
    PresetAction::Flash("已刷新预设（磁盘 YAML → 内存）".into())
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
                    PresetAction::Flash(format!("已删除模式 {}", src.id))
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
        'r' | 'R' => resync_action(ctx, view),
        _ => PresetAction::None,
    }
}

pub fn on_canvas_char(ctx: &Context, view: &mut PresetView, c: char) -> PresetAction {
    let remove = matches!(
        view,
        PresetView::Canvas(s)
            if !s.editing_persona
                && !s.naming_active()
                && s.pane == PresetPane::Assigned
                && matches!(c, 'd' | 'D' | 'x' | 'X')
    );
    if remove {
        return accept(ctx, view);
    }

    let can_resync = matches!(
        view,
        PresetView::Canvas(s)
            if !s.editing_persona
                && !s.naming_active()
                && (s.pane != PresetPane::Catalog || s.catalog_query.is_empty())
                && matches!(c, 'r' | 'R')
    );
    if can_resync {
        return resync_action(ctx, view);
    }

    if let PresetView::Canvas(canvas) = view {
        if canvas.editing_persona || canvas.naming_role.is_some() {
            return PresetAction::None;
        }
        if canvas.pane == PresetPane::Persona && matches!(c, 'e' | 'E') {
            canvas.editing_persona = true;
            return PresetAction::None;
        }
        if canvas.pane == PresetPane::Roles && canvas.editing_role.is_none() {
            let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
                return PresetAction::Flash("Agent 预设服务未挂载".into());
            };
            match c {
                'n' | 'N' => start_create_naming(canvas),
                'm' | 'M' => start_rename_naming(ctx, canvas),
                'x' | 'X' | 'd' | 'D' => {
                    let roles = role_ids(ctx, canvas);
                    let Some(role) = roles.get(canvas.roles_sel).cloned() else {
                        return PresetAction::Flash("没有可删子代理".into());
                    };
                    match presets.delete_subagent(&canvas.id, &role) {
                        Ok(()) => {
                            let next = role_ids(ctx, canvas);
                            canvas.roles_sel = canvas.roles_sel.min(next.len().saturating_sub(1));
                            PresetAction::Flash(format!("已删除子代理 {role}"))
                        }
                        Err(e) => PresetAction::Flash(e),
                    }
                }
                _ => PresetAction::None,
            }
        } else {
            PresetAction::None
        }
    } else {
        PresetAction::None
    }
}

fn select_hit(view: &mut PresetView, idx: usize) {
    let PresetView::Canvas(c) = view else {
        return;
    };
    if idx >= ROLES_HIT {
        c.pane = PresetPane::Roles;
        c.roles_sel = idx - ROLES_HIT;
        return;
    }
    if idx >= PERSONA_HIT {
        c.pane = PresetPane::Persona;
        return;
    }
    if idx >= ASSIGNED_HIT {
        c.pane = PresetPane::Assigned;
        c.assigned_sel = idx - ASSIGNED_HIT;
        return;
    }
    c.pane = PresetPane::Catalog;
    c.catalog_sel = idx;
}

/// Mouse: single click = select only; double-click = add/remove / open role / edit persona.
pub fn apply_hit(ctx: &Context, view: &mut PresetView, idx: usize) -> PresetAction {
    let now = Instant::now();
    let double = {
        let PresetView::Canvas(c) = view else {
            return PresetAction::None;
        };
        let dbl = c
            .last_click
            .as_ref()
            .is_some_and(|(prev, at)| *prev == idx && now.duration_since(*at) <= DOUBLE_CLICK);
        c.last_click = Some((idx, now));
        dbl
    };
    select_hit(view, idx);
    if double {
        accept(ctx, view)
    } else {
        PresetAction::None
    }
}

pub fn render(ctx: &Context, buf: &mut Buffer, area: Rect, view: &PresetView) -> PickerHits {
    match view {
        PresetView::Roster { selected } => render_roster(ctx, buf, area, *selected),
        PresetView::Canvas(c) => render_canvas(ctx, buf, area, c),
    }
}

fn render_roster(ctx: &Context, buf: &mut Buffer, area: Rect, selected: usize) -> PickerHits {
    let theme = Theme::current();
    let Some(frame) = render_fullscreen_frame(buf, area, &theme, Some("Agent 预设"), false) else {
        return PickerHits::default();
    };
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
    let mut hits = PickerHits {
        close_button: frame.close_button,
        ..Default::default()
    };
    let content = frame.content;
    if content.height < 3 || content.width < 12 {
        return hits;
    }

    // Visible delete affordance (x was easy to miss in the shortcut strip alone).
    let hint = " Enter 打开 · n 新建 · d 复制 · x 删除模式 · a 应用 ";
    buf.set_line(
        content.x,
        content.y,
        &Line::from(Span::styled(
            truncate_str(hint, content.width as usize),
            Style::default()
                .fg(theme.accent_skill)
                .add_modifier(Modifier::BOLD),
        )),
        content.width,
    );

    let list_area = Rect {
        x: content.x,
        y: content.y.saturating_add(1),
        width: content.width,
        height: content.height.saturating_sub(1),
    };

    let rights: Vec<String> = presets
        .iter()
        .map(|p| {
            let deletable = p.origin != PresetOrigin::Shipped;
            let base = if let Some(reason) = &p.broken {
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
            };
            if deletable {
                format!("{base} · x删")
            } else {
                base
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
    let row_hits = render_picker_list(buf, list_area, &theme, "", &rows, 0);
    hits.rows = row_hits;
    hits
}

fn render_canvas(ctx: &Context, buf: &mut Buffer, area: Rect, canvas: &CanvasState) -> PickerHits {
    let theme = Theme::current();
    let title = match &canvas.editing_role {
        Some(role) => format!("组装角色 · {role}"),
        None => "组装 Agent".into(),
    };
    let Some(frame) = render_fullscreen_frame(buf, area, &theme, Some(&title), false) else {
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

    let identity_h = if canvas.editing_persona || canvas.naming_role.is_some() {
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

    if canvas.naming_role.is_some() {
        paint_naming_draft(buf, chunks[1], &theme, canvas);
        return hits;
    }

    // Roles list replaces tool panes when focused (and not inside a role editor).
    if canvas.pane == PresetPane::Roles && canvas.editing_role.is_none() {
        hits.rows
            .extend(render_roles_pane(ctx, buf, chunks[1], &theme, canvas, &preset));
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
    let all_tools = role_all_tools(ctx, canvas);

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

    let cat_header = format!(
        "可加 · {} · 单击选中 · Enter/双击加入",
        catalog.len()
    );
    let asg_header = if all_tools {
        format!(
            "已加入 · 全部 · {} · 单击选中 · Enter/双击移除",
            assigned.len()
        )
    } else {
        format!("已加入 · {} · 单击选中 · Enter/双击移除", assigned.len())
    };

    let cat_rights: Vec<String> = catalog
        .iter()
        .map(|spec| {
            let brief = brief_desc(&spec.description);
            let kind = tool_kind_label(ctx, &spec.name);
            match brief.is_empty() {
                true => kind.to_string(),
                false => format!("{brief} · {kind}"),
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
            let live_hit = live_specs(ctx).into_iter().find(|s| &s.name == name);
            let kind = tool_kind_label(ctx, name);
            let right = match live_hit {
                None => format!("未挂载 · {kind}"),
                Some(spec) => {
                    let brief = brief_desc(&spec.description);
                    if brief.is_empty() {
                        kind.to_string()
                    } else {
                        format!("{brief} · {kind}")
                    }
                }
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


fn paint_naming_draft(buf: &mut Buffer, area: Rect, theme: &Theme, canvas: &CanvasState) {
    let Some(draft) = canvas.naming_role.as_ref() else {
        return;
    };
    if area.height == 0 || area.width == 0 {
        return;
    }
    let title = match (&draft.from, draft.step) {
        (None, RoleNamingStep::Id) => "命名新建 · 角色 id（ascii slug 或 1–16 汉字）",
        (None, RoleNamingStep::Name) => "命名新建 · 显示名",
        (Some(_), RoleNamingStep::Id) => "改名 · 角色 id（改 id 会迁移 agents/<id>.yml）",
        (Some(_), RoleNamingStep::Name) => "改名 · 显示名",
    };
    let value = match draft.step {
        RoleNamingStep::Id => draft.id.as_str(),
        RoleNamingStep::Name => draft.name.as_str(),
    };
    let hint = "Enter 确认 · Esc 取消";
    buf.set_style(area, Style::default().bg(theme.bg_visual));
    buf.set_line(
        area.x,
        area.y,
        &Line::from(Span::styled(
            truncate_str(title, area.width as usize),
            Style::default()
                .fg(theme.accent_skill)
                .bg(theme.bg_visual)
                .add_modifier(Modifier::BOLD),
        )),
        area.width,
    );
    if area.height > 1 {
        let shown = if value.is_empty() { "▌" } else { value };
        buf.set_line(
            area.x,
            area.y + 1,
            &Line::from(Span::styled(
                truncate_str(shown, area.width as usize),
                Style::default().fg(theme.text_primary).bg(theme.bg_visual),
            )),
            area.width,
        );
    }
    if area.height > 2 {
        buf.set_line(
            area.x,
            area.y + 2,
            &Line::from(Span::styled(
                truncate_str(hint, area.width as usize),
                Style::default().fg(theme.gray).bg(theme.bg_visual),
            )),
            area.width,
        );
    }
}

fn render_roles_pane(
    ctx: &Context,
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    canvas: &CanvasState,
    preset: &AgentPreset,
) -> Vec<(usize, Rect)> {
    let ids = role_ids(ctx, canvas);
    let sel = if ids.is_empty() {
        0
    } else {
        canvas.roles_sel.min(ids.len() - 1)
    };
    let header = format!(
        "子代理角色 · {} · Enter/双击编辑 · n 命名新建 · m 改名 · x 删除",
        ids.len()
    );
    let rights: Vec<String> = ids
        .iter()
        .map(|id| {
            let def = preset.agents.get(id);
            match def {
                Some(d) if !d.name.trim().is_empty() && d.name != *id => {
                    let tools = match &d.tools {
                        None => "工具·全部".into(),
                        Some(t) => format!("工具·{}", t.len()),
                    };
                    format!("{} · {tools}", d.name)
                }
                Some(d) => match &d.tools {
                    None => "工具·全部".into(),
                    Some(t) => format!("工具·{}", t.len()),
                },
                None => String::new(),
            }
        })
        .collect();
    let rows: Vec<PickerRow> = if ids.is_empty() {
        vec![PickerRow {
            label: "(无角色)",
            right_label: "按 n 命名新建",
            selected: true,
        }]
    } else {
        ids.iter()
            .zip(rights.iter())
            .enumerate()
            .map(|(i, (id, right))| PickerRow {
                label: id.as_str(),
                right_label: right.as_str(),
                selected: i == sel,
            })
            .collect()
    };
    render_tool_pane(buf, area, theme, &header, "", false, &rows, ROLES_HIT)
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
    let title = match &canvas.editing_role {
        Some(role) => format!(" {}  [{}] / 角色 {role}{badge} ", preset.name, preset.id),
        None => format!(" {}  [{}]{badge} ", preset.name, preset.id),
    };
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
    if y < area.y + area.height && !preset.description.trim().is_empty() && canvas.editing_role.is_none()
    {
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
    if y < area.y + area.height && canvas.editing_role.is_none() {
        let ids: Vec<&str> = preset.agents.keys().map(|s| s.as_str()).collect();
        let line = if ids.is_empty() {
            "子代理 （无）· Tab 到「角色」增删改".to_string()
        } else {
            format!("子代理 {} · Tab 到「角色」", ids.join(" / "))
        };
        buf.set_line(
            area.x,
            y,
            &Line::from(Span::styled(
                truncate_str(&line, area.width as usize),
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
    let stored = match &canvas.editing_role {
        Some(role) => preset
            .agents
            .get(role)
            .map(|d| d.persona.as_str())
            .unwrap_or(""),
        None => preset.persona.as_str(),
    };
    let body = if canvas.editing_persona {
        canvas.persona_draft.as_str()
    } else if !canvas.persona_draft.trim().is_empty() {
        // Draft tracks the active target (mode or role) after open/enter.
        canvas.persona_draft.as_str()
    } else if !stored.trim().is_empty() {
        stored.trim()
    } else {
        "（无人设 · Tab 到人设后 Enter 编辑）"
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
    use super::*;
    use cordis_spine::{ToolSpec, Tools, AGENT_PRESETS, TOOLS};
    use std::sync::Arc;

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: format!("{name} tool."),
            parameters_json: r#"{"type":"object"}"#.into(),
        }
    }

    fn stub_body() -> cordis_spine::ToolBody {
        Arc::new(|_c| {
            Box::pin(async {
                cordis_spine::ToolResult {
                    content: "ok".into(),
                    ..Default::default()
                }
            })
        })
    }

    fn harness_allowlist() -> Context {
        let ctx = Context::new();
        let mut preset = AgentPreset::new("custom");
        preset.origin = PresetOrigin::User;
        preset.tools = Some(vec!["bash".into()]);
        let presets = AgentPresets::overlay(preset);
        ctx.provide(AGENT_PRESETS, presets).unwrap();
        let tools = Tools::echo(ctx.clone());
        tools.register(spec("bash"), stub_body()).unwrap();
        tools.register(spec("read_file"), stub_body()).unwrap();
        tools
            .register_deferred(spec("scheduler_create"), stub_body())
            .unwrap();
        ctx.provide(TOOLS, tools).unwrap();
        ctx
    }

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
    }

    #[test]
    fn catalog_excludes_assigned_left_right_partition_full_set() {
        let ctx = harness_allowlist();
        let canvas = CanvasState {
            id: "custom".into(),
            pane: PresetPane::Catalog,
            catalog_sel: 0,
            assigned_sel: 0,
            roles_sel: 0,
            catalog_query: String::new(),
            editing_persona: false,
            persona_draft: String::new(),
            editing_role: None,
            naming_role: None,
            last_click: None,
        };
        let left: Vec<String> = catalog_rows(&ctx, &canvas)
            .into_iter()
            .map(|s| s.name)
            .collect();
        let right = assigned_names(&ctx, &canvas);
        assert_eq!(right, vec!["bash".to_string()]);
        assert!(left.contains(&"read_file".to_string()));
        assert!(left.contains(&"scheduler_create".to_string()));
        assert!(!left.iter().any(|n| n == "bash"));
        // left ∪ right covers full non-MCP live set
        let mut union = left;
        union.extend(right);
        union.sort();
        let mut live = live_names(&ctx);
        live.sort();
        assert_eq!(union, live);
    }

    #[test]
    fn r_resyncs_handwritten_role_into_memory() {
        let dir = std::env::temp_dir().join(format!(
            "dock-preset-r-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let presets = AgentPresets::load(dir.clone());
        let mode = presets.create().unwrap();
        let agents_dir = dir.join(&mode.id).join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("scout.yml"), "name: scout\npersona: disk\n").unwrap();
        let ctx = Context::new();
        ctx.provide(AGENT_PRESETS, presets).unwrap();
        let mut view = open_canvas(&ctx, &mode.id).unwrap();
        let action = on_canvas_char(&ctx, &mut view, 'r');
        assert!(
            matches!(&action, PresetAction::Flash(msg) if msg.contains("刷新")),
            "{action:?}"
        );
        let presets = ctx.get::<AgentPresets>(AGENT_PRESETS).unwrap();
        assert!(
            presets.get(&mode.id).unwrap().agents.contains_key("scout"),
            "resync must load handwritten scout.yml"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deferred_tools_are_labeled() {
        let ctx = harness_allowlist();
        assert_eq!(tool_kind_label(&ctx, "bash"), "常驻");
        assert_eq!(tool_kind_label(&ctx, "scheduler_create"), "延迟");
    }

    #[test]
    fn mcp_tools_labeled_mcp_not_resident() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx.clone());
        tools
            .register_mcp(spec("mcp_demo__shot"), stub_body())
            .unwrap();
        ctx.provide(TOOLS, tools).unwrap();
        assert_eq!(tool_kind_label(&ctx, "mcp_demo__shot"), "MCP");
        assert_ne!(tool_kind_label(&ctx, "mcp_demo__shot"), "常驻");
    }


    #[test]
    fn single_click_selects_double_click_adds() {
        let ctx = harness_allowlist();
        let mut view = open_canvas(&ctx, "custom").unwrap();
        // catalog should start with read_file / scheduler_create (bash already assigned)
        let rows = match &view {
            PresetView::Canvas(c) => catalog_rows(&ctx, c),
            _ => panic!("expected canvas"),
        };
        let read_idx = rows.iter().position(|s| s.name == "read_file").unwrap();

        let action = apply_hit(&ctx, &mut view, read_idx);
        assert!(matches!(action, PresetAction::None));
        match &view {
            PresetView::Canvas(c) => {
                assert_eq!(c.pane, PresetPane::Catalog);
                assert_eq!(c.catalog_sel, read_idx);
            }
            _ => panic!("expected canvas"),
        }
        // assigned still only bash
        if let PresetView::Canvas(c) = &view {
            assert_eq!(assigned_names(&ctx, c), vec!["bash".to_string()]);
        }

        // Simulate double-click by rewinding last_click timestamp.
        if let PresetView::Canvas(c) = &mut view {
            c.last_click = Some((read_idx, Instant::now() - Duration::from_millis(100)));
        }
        let action = apply_hit(&ctx, &mut view, read_idx);
        assert!(matches!(action, PresetAction::Flash(msg) if msg.contains("已加入")));
        if let PresetView::Canvas(c) = &view {
            let asg = assigned_names(&ctx, c);
            assert!(asg.iter().any(|n| n == "read_file"), "{asg:?}");
            assert!(!catalog_rows(&ctx, c).iter().any(|s| s.name == "read_file"));
        }
    }

    #[test]
    fn enter_adds_from_catalog() {
        let ctx = harness_allowlist();
        let mut view = open_canvas(&ctx, "custom").unwrap();
        if let PresetView::Canvas(c) = &mut view {
            let rows = catalog_rows(&ctx, c);
            let idx = rows
                .iter()
                .position(|s| s.name == "scheduler_create")
                .unwrap();
            c.catalog_sel = idx;
            c.pane = PresetPane::Catalog;
        }
        let action = accept(&ctx, &mut view);
        assert!(matches!(action, PresetAction::Flash(msg) if msg.contains("已加入")));
        if let PresetView::Canvas(c) = &view {
            assert!(assigned_names(&ctx, c)
                .iter()
                .any(|n| n == "scheduler_create"));
        }
    }

    #[test]
    fn subagent_role_create_edit_delete_in_ui() {
        let ctx = harness_allowlist();
        let mut view = open_canvas(&ctx, "custom").unwrap();
        if let PresetView::Canvas(c) = &mut view {
            c.pane = PresetPane::Roles;
        }
        let action = on_canvas_char(&ctx, &mut view, 'n');
        assert!(
            matches!(&action, PresetAction::Flash(msg) if msg.contains("角色 id")),
            "{action:?}"
        );
        match &mut view {
            PresetView::Canvas(c) => {
                let draft = c.naming_role.as_mut().expect("naming draft");
                assert!(draft.from.is_none());
                assert_eq!(draft.step, RoleNamingStep::Id);
                draft.id = "scout".into();
            }
            _ => panic!("expected canvas"),
        }
        let action = accept(&ctx, &mut view);
        assert!(
            matches!(&action, PresetAction::Flash(msg) if msg.contains("显示名")),
            "{action:?}"
        );
        match &mut view {
            PresetView::Canvas(c) => {
                let draft = c.naming_role.as_mut().expect("naming draft");
                assert_eq!(draft.step, RoleNamingStep::Name);
                draft.name = "侦察".into();
            }
            _ => panic!("expected canvas"),
        }
        let action = accept(&ctx, &mut view);
        assert!(
            matches!(&action, PresetAction::Flash(msg) if msg.contains("编辑子代理")),
            "{action:?}"
        );
        match &view {
            PresetView::Canvas(c) => {
                assert!(c.naming_role.is_none());
                assert_eq!(c.editing_role.as_deref(), Some("scout"));
                assert_eq!(c.pane, PresetPane::Catalog);
            }
            _ => panic!("expected canvas"),
        }
        let presets = ctx.get::<AgentPresets>(AGENT_PRESETS).unwrap();
        assert_eq!(
            presets.get("custom").unwrap().agents.get("scout").unwrap().name,
            "侦察"
        );
        // role starts with tools=None (all) → catalog empty; remove to open allowlist
        if let PresetView::Canvas(c) = &mut view {
            c.pane = PresetPane::Assigned;
            c.assigned_sel = assigned_names(&ctx, c)
                .iter()
                .position(|n| n == "read_file")
                .expect("read_file assigned while all-tools");
        }
        let action = accept(&ctx, &mut view);
        assert!(matches!(action, PresetAction::Flash(msg) if msg.contains("已移除")));
        if let PresetView::Canvas(c) = &view {
            assert!(!role_all_tools(&ctx, c));
            assert!(!assigned_names(&ctx, c).iter().any(|n| n == "read_file"));
            // left pane now has the removed tool
            assert!(catalog_rows(&ctx, c).iter().any(|s| s.name == "read_file"));
        }
        assert!(!on_esc(&ctx, &mut view));
        match &view {
            PresetView::Canvas(c) => {
                assert!(c.editing_role.is_none());
                assert_eq!(c.pane, PresetPane::Roles);
            }
            _ => panic!("expected canvas"),
        }
        let action = on_canvas_char(&ctx, &mut view, 'x');
        assert!(matches!(action, PresetAction::Flash(msg) if msg.contains("已删除子代理")));
        if let PresetView::Canvas(c) = &view {
            assert!(role_ids(&ctx, c).is_empty());
        }
    }

    #[test]
    fn roster_delete_flash_mentions_mode() {
        let ctx = harness_allowlist();
        // overlay custom is the only/current; create another via duplicate path isn't available
        // without persist — just ensure x on shipped-like fails gracefully when only overlay.
        let mut view = PresetView::Roster { selected: 0 };
        let action = on_roster_char(&ctx, &mut view, 'x');
        // overlay preset origin is User, delete should work in-memory
        assert!(matches!(action, PresetAction::Flash(msg) if msg.contains("已删除模式")));
    }
}
