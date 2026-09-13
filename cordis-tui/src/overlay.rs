//! Resume / help / history / find overlays on Grok picker chrome.

use std::collections::HashSet;
use std::sync::Arc;

use cordis_spine::{ArchivedSession, OccupancyKind, PlanApprovalPrompt, SlashEntry};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use crate::grok::picker::{
    render_floating_frame, render_fullscreen_frame, render_picker_list, PickerHits, PickerRow,
};
use crate::grok::tasks_pane::GroupKind;
use crate::plan_approval_view::PlanWrapCache;
use crate::preset_overlay::{CanvasState, PresetPane, PresetView, RoleNamingDraft, RoleNamingStep};
use crate::settings_modal::SettingsField;
use crate::slash::{ArgKind, SlashCmd};
use crate::theme::Theme;

#[derive(Debug, Clone, Default)]
pub enum Overlay {
    #[default]
    None,
    Resume {
        selected: usize,
        query: String,
    },
    Help {
        selected: usize,
        query: String,
    },
    History {
        selected: usize,
        query: String,
    },
    Find {
        selected: usize,
        query: String,
    },
    Args {
        kind: ArgKind,
        cmd: SlashCmd,
        selected: usize,
        query: String,
    },
    Settings {
        selected: usize,
        picking: Option<SettingsField>,
    },
    Permission {
        selected: usize,
    },
    /// Pending Origin pairing (same interrupt priority as Permission).
    PairingPending {
        selected: usize,
    },
    /// `/pair` management: pending queue + bindings.
    PairingManage {
        selected: usize,
    },
    Ask {
        selected: usize,
        picked: Vec<bool>,
        draft: String,
        /// Char index into `draft` for the Other freeform field.
        draft_cursor: usize,
    },
    Elicit {
        selected: usize,
        picked: Vec<bool>,
        draft: String,
    },
    Tasks {
        selected: usize,
        query: String,
        collapsed: HashSet<GroupKind>,
    },
    Mcps {
        selected: usize,
        query: String,
        tools_expanded: HashSet<usize>,
        section_collapsed: bool,
    },
    Workflows {
        selected: usize,
        query: String,
    },
    Goal {
        selected: usize,
        editing: bool,
        draft: String,
    },
    /// Plan approval / `/view-plan` preview (Permission-style chrome).
    PlanApproval {
        selected: usize,
        view_only: bool,
        /// Wrapped-line scroll offset into the plan body.
        scroll: usize,
        /// Disk / parked snapshot taken when the overlay opens.
        prompt: Arc<PlanApprovalPrompt>,
        wrap: PlanWrapCache,
    },
    /// `/context` occupancy + `/usage` session ledger (no grok.com billing).
    Usage {
        tab: UsageTab,
        scroll: usize,
        detail: Option<OccupancyKind>,
    },
    /// Extra slash overlay (read-only title + body).
    Notice {
        title: String,
        body: String,
        scroll: usize,
    },
    /// Dynamic package TUI slot (generic; body comes from `"tui.slots"`).
    Slot {
        id: String,
        scroll: usize,
    },
    /// Live `/browser` cockpit (status / tabs / screenshot / 审批). Body from `"browser"`.
    Browser {
        scroll: usize,
    },
    /// Live `/computer` thin cockpit (cua-driver MCP status / 审批). No embedded desktop.
    Computer {
        scroll: usize,
    },
    /// `/preset` roster + assembly canvas.
    Presets(PresetView),
    /// Child conversation or background job (Grok fullscreen framed view).
    Inspect {
        target: InspectTarget,
        scroll: usize,
        from_tasks: bool,
        composer: String,
        composer_cursor: usize,
    },
}

/// Tabs inside [`Overlay::Usage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsageTab {
    #[default]
    Context,
    Session,
}

impl UsageTab {
    pub fn next(self) -> Self {
        match self {
            Self::Context => Self::Session,
            Self::Session => Self::Context,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Context => "占用",
            Self::Session => "用量",
        }
    }
}

/// What [`Overlay::Inspect`] is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectTarget {
    Subagent(String),
    Job(String),
}

impl Overlay {
    pub fn usage(tab: UsageTab) -> Self {
        Self::Usage {
            tab,
            scroll: 0,
            detail: None,
        }
    }

    pub fn plan_approval(prompt: PlanApprovalPrompt, view_only: bool) -> Self {
        Self::PlanApproval {
            selected: 0,
            view_only,
            scroll: 0,
            prompt: Arc::new(prompt),
            wrap: PlanWrapCache::new(),
        }
    }

    pub fn inspect_subagent(id: impl Into<String>, from_tasks: bool) -> Self {
        Self::Inspect {
            target: InspectTarget::Subagent(id.into()),
            scroll: 0,
            from_tasks,
            composer: String::new(),
            composer_cursor: 0,
        }
    }

    pub fn inspect_job(id: impl Into<String>, from_tasks: bool) -> Self {
        Self::Inspect {
            target: InspectTarget::Job(id.into()),
            scroll: 0,
            from_tasks,
            composer: String::new(),
            composer_cursor: 0,
        }
    }

    pub fn is_open(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn close(&mut self) {
        *self = Self::None;
    }

    #[allow(dead_code)]
    pub fn query(&self) -> &str {
        match self {
            Self::None => "",
            Self::Resume { query, .. }
            | Self::Help { query, .. }
            | Self::History { query, .. }
            | Self::Find { query, .. }
            | Self::Args { query, .. } => query,
            Self::Settings { .. }
            | Self::Permission { .. }
            | Self::PairingPending { .. }
            | Self::PairingManage { .. }
            | Self::Ask { .. }
            | Self::Elicit { .. }
            | Self::PlanApproval { .. }
            | Self::Usage { .. }
            | Self::Notice { .. }
            | Self::Slot { .. }
            | Self::Browser { .. }
            | Self::Computer { .. }
            | Self::Inspect { .. }
            | Self::Goal { editing: false, .. } => "",
            Self::Goal {
                draft,
                editing: true,
                ..
            } => draft,
            Self::Tasks { query, .. }
            | Self::Mcps { query, .. }
            | Self::Workflows { query, .. } => query,
            Self::Presets(PresetView::Canvas(CanvasState {
                naming_role:
                    Some(RoleNamingDraft {
                        step: RoleNamingStep::Id,
                        id,
                        ..
                    }),
                ..
            })) => id,
            Self::Presets(PresetView::Canvas(CanvasState {
                naming_role:
                    Some(RoleNamingDraft {
                        step: RoleNamingStep::Name,
                        name,
                        ..
                    }),
                ..
            })) => name,
            Self::Presets(PresetView::Canvas(CanvasState {
                editing_persona: true,
                persona_draft,
                ..
            })) => persona_draft,
            Self::Presets(PresetView::Canvas(CanvasState { catalog_query, .. })) => catalog_query,
            Self::Presets(PresetView::Roster { .. }) => "",
        }
    }

    pub fn push_char(&mut self, c: char) {
        if let Overlay::Inspect {
            target: InspectTarget::Subagent(_),
            composer,
            composer_cursor,
            ..
        } = self
        {
            crate::inspect_overlay::insert_composer(composer, composer_cursor, c);
            return;
        }
        if let Some(q) = self.query_mut() {
            q.push(c);
        }
        self.set_selected(0);
    }

    pub fn push_str(&mut self, s: &str) {
        if let Overlay::Inspect {
            target: InspectTarget::Subagent(_),
            composer,
            composer_cursor,
            ..
        } = self
        {
            crate::inspect_overlay::composer_insert_str(composer, composer_cursor, s);
            return;
        }
        if let Some(q) = self.query_mut() {
            q.push_str(s);
        }
        self.set_selected(0);
    }

    pub fn backspace(&mut self) {
        if let Overlay::Inspect {
            target: InspectTarget::Subagent(_),
            composer,
            composer_cursor,
            ..
        } = self
        {
            crate::inspect_overlay::composer_backspace(composer, composer_cursor);
            return;
        }
        if let Some(q) = self.query_mut() {
            q.pop();
        }
        self.set_selected(0);
    }

    pub fn move_sel(&mut self, delta: i16, len: usize) {
        if len == 0 {
            return;
        }
        let next = (self.selected() as i32 + delta as i32).rem_euclid(len as i32) as usize;
        self.set_selected(next);
    }

    pub fn selected(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Resume { selected, .. }
            | Self::Help { selected, .. }
            | Self::History { selected, .. }
            | Self::Find { selected, .. }
            | Self::Args { selected, .. }
            | Self::Settings { selected, .. }
            | Self::Permission { selected, .. }
            | Self::PairingPending { selected, .. }
            | Self::PairingManage { selected, .. }
            | Self::Ask { selected, .. }
            | Self::Elicit { selected, .. }
            | Self::PlanApproval { selected, .. }
            | Self::Tasks { selected, .. }
            | Self::Mcps { selected, .. }
            | Self::Workflows { selected, .. }
            | Self::Goal { selected, .. } => *selected,
            Self::Usage { .. }
            | Self::Notice { .. }
            | Self::Slot { .. }
            | Self::Browser { .. }
            | Self::Computer { .. }
            | Self::Inspect { .. } => 0,
            Self::Presets(PresetView::Roster { selected }) => *selected,
            Self::Presets(PresetView::Canvas(c)) => match c.pane {
                PresetPane::Catalog => c.catalog_sel,
                PresetPane::Assigned => c.assigned_sel,
                PresetPane::Roles => c.roles_sel,
                PresetPane::Persona => 0,
            },
        }
    }

    pub fn set_selected(&mut self, selected: usize) {
        match self {
            Self::None => {}
            Self::Resume { selected: s, .. }
            | Self::Help { selected: s, .. }
            | Self::History { selected: s, .. }
            | Self::Find { selected: s, .. }
            | Self::Args { selected: s, .. }
            | Self::Settings { selected: s, .. }
            | Self::Permission { selected: s, .. }
            | Self::PairingPending { selected: s, .. }
            | Self::PairingManage { selected: s, .. }
            | Self::Ask { selected: s, .. }
            | Self::Elicit { selected: s, .. }
            | Self::PlanApproval { selected: s, .. }
            | Self::Tasks { selected: s, .. }
            | Self::Mcps { selected: s, .. }
            | Self::Workflows { selected: s, .. }
            | Self::Goal { selected: s, .. } => *s = selected,
            Self::Usage { .. }
            | Self::Notice { .. }
            | Self::Slot { .. }
            | Self::Browser { .. }
            | Self::Computer { .. }
            | Self::Inspect { .. } => {}
            Self::Presets(PresetView::Roster { selected: s }) => *s = selected,
            Self::Presets(PresetView::Canvas(c)) => match c.pane {
                PresetPane::Catalog => c.catalog_sel = selected,
                PresetPane::Assigned => c.assigned_sel = selected,
                PresetPane::Roles => c.roles_sel = selected,
                PresetPane::Persona => {}
            },
        }
    }

    fn query_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::None => None,
            Self::Resume { query, .. }
            | Self::Help { query, .. }
            | Self::History { query, .. }
            | Self::Find { query, .. }
            | Self::Args { query, .. }
            | Self::Tasks { query, .. }
            | Self::Mcps { query, .. }
            | Self::Workflows { query, .. } => Some(query),
            Self::Goal {
                editing: true,
                draft,
                ..
            } => Some(draft),
            Self::Presets(PresetView::Canvas(CanvasState {
                naming_role:
                    Some(RoleNamingDraft {
                        step: RoleNamingStep::Id,
                        id,
                        ..
                    }),
                ..
            })) => Some(id),
            Self::Presets(PresetView::Canvas(CanvasState {
                naming_role:
                    Some(RoleNamingDraft {
                        step: RoleNamingStep::Name,
                        name,
                        ..
                    }),
                ..
            })) => Some(name),
            Self::Presets(PresetView::Canvas(CanvasState {
                editing_persona: true,
                persona_draft,
                ..
            })) => Some(persona_draft),
            Self::Presets(PresetView::Canvas(CanvasState {
                pane: PresetPane::Catalog,
                catalog_query,
                editing_persona: false,
                ..
            })) => Some(catalog_query),
            Self::Settings { .. }
            | Self::Permission { .. }
            | Self::PairingPending { .. }
            | Self::PairingManage { .. }
            | Self::Ask { .. }
            | Self::Elicit { .. }
            | Self::PlanApproval { .. }
            | Self::Usage { .. }
            | Self::Notice { .. }
            | Self::Slot { .. }
            | Self::Browser { .. }
            | Self::Computer { .. }
            | Self::Inspect { .. }
            | Self::Goal { editing: false, .. }
            | Self::Presets(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpKind {
    Hint,
    Slash(SlashCmd),
}

#[derive(Debug, Clone, Copy)]
pub struct HelpRow {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: HelpKind,
}

#[allow(dead_code)]
enum HelpEntry {
    Header(&'static str),
    Row(HelpRow),
}

const HELP: &[HelpEntry] = &[
    HelpEntry::Header("常用"),
    HelpEntry::Row(HelpRow {
        key: "enter",
        label: "发送",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "esc",
        label: "清空 / 取消当前轮 / 关闭",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "shift+tab",
        label: "切换模式（询问 / 计划 / 始终允许）",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+x",
        label: "快捷键帮助",
        kind: HelpKind::Slash(SlashCmd::Help),
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+v",
        label: "粘贴文字 / 图片",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+t",
        label: "待办卡片折叠（未完成 / 全部 / 一行）",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+q",
        label: "退出",
        kind: HelpKind::Slash(SlashCmd::Quit),
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+w",
        label: "新会话",
        kind: HelpKind::Slash(SlashCmd::New),
    }),
    HelpEntry::Row(HelpRow {
        key: "f2",
        label: "设置",
        kind: HelpKind::Slash(SlashCmd::Settings),
    }),
    HelpEntry::Row(HelpRow {
        key: "f3",
        label: "恢复会话",
        kind: HelpKind::Slash(SlashCmd::Resume),
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+.",
        label: "快捷键帮助",
        kind: HelpKind::Slash(SlashCmd::Help),
    }),
    HelpEntry::Header("会话"),
    HelpEntry::Row(HelpRow {
        key: "/new",
        label: "开始新会话",
        kind: HelpKind::Slash(SlashCmd::New),
    }),
    HelpEntry::Row(HelpRow {
        key: "/resume",
        label: "恢复上次会话",
        kind: HelpKind::Slash(SlashCmd::Resume),
    }),
    HelpEntry::Row(HelpRow {
        key: "/pair",
        label: "浏览器配对（开启回环网关）",
        kind: HelpKind::Slash(SlashCmd::Pair),
    }),
    HelpEntry::Row(HelpRow {
        key: "/history",
        label: "搜索提示词历史",
        kind: HelpKind::Slash(SlashCmd::History),
    }),
    HelpEntry::Row(HelpRow {
        key: "/find",
        label: "搜索对话",
        kind: HelpKind::Slash(SlashCmd::Find),
    }),
    HelpEntry::Row(HelpRow {
        key: "/usage",
        label: "本会话用量",
        kind: HelpKind::Slash(SlashCmd::Usage),
    }),
    HelpEntry::Row(HelpRow {
        key: "/context",
        label: "查看上下文占用",
        kind: HelpKind::Slash(SlashCmd::Context),
    }),
    HelpEntry::Row(HelpRow {
        key: "/compact",
        label: "压缩旧对话",
        kind: HelpKind::Slash(SlashCmd::Compact),
    }),
    HelpEntry::Row(HelpRow {
        key: "/copy",
        label: "复制上一条回复",
        kind: HelpKind::Slash(SlashCmd::Copy),
    }),
    HelpEntry::Row(HelpRow {
        key: "/theme",
        label: "切换配色",
        kind: HelpKind::Slash(SlashCmd::Theme),
    }),
    HelpEntry::Row(HelpRow {
        key: "/timestamps",
        label: "开关时间戳",
        kind: HelpKind::Slash(SlashCmd::Timestamps),
    }),
    HelpEntry::Row(HelpRow {
        key: "/model",
        label: "切换模型",
        kind: HelpKind::Slash(SlashCmd::Model),
    }),
    HelpEntry::Row(HelpRow {
        key: "/protocol",
        label: "切换推理协议",
        kind: HelpKind::Slash(SlashCmd::Protocol),
    }),
    HelpEntry::Row(HelpRow {
        key: "/settings",
        label: "设置",
        kind: HelpKind::Slash(SlashCmd::Settings),
    }),
    HelpEntry::Row(HelpRow {
        key: "/loop",
        label: "按间隔循环提问",
        kind: HelpKind::Slash(SlashCmd::Loop),
    }),
    HelpEntry::Row(HelpRow {
        key: "/plan",
        label: "进入计划模式",
        kind: HelpKind::Slash(SlashCmd::Plan),
    }),
    HelpEntry::Row(HelpRow {
        key: "/view-plan",
        label: "查看 / 批准计划",
        kind: HelpKind::Slash(SlashCmd::ViewPlan),
    }),
    HelpEntry::Row(HelpRow {
        key: "/goal",
        label: "开始或查看目标",
        kind: HelpKind::Slash(SlashCmd::Goal),
    }),
    HelpEntry::Row(HelpRow {
        key: "/tasks",
        label: "后台任务与子代理（对话中点击卡片查看）",
        kind: HelpKind::Slash(SlashCmd::Tasks),
    }),
    HelpEntry::Row(HelpRow {
        key: "/workflow",
        label: "工作流运行",
        kind: HelpKind::Slash(SlashCmd::Workflow),
    }),
    HelpEntry::Row(HelpRow {
        key: "/mcps",
        label: "MCP 服务器（Space 开关 · i 登录 · Enter 展开）",
        kind: HelpKind::Slash(SlashCmd::Mcps),
    }),
    HelpEntry::Row(HelpRow {
        key: "/computer",
        label: "本机桌面 cua-driver 驾驶舱（不嵌桌面）",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "/lsp",
        label: "探测并写入语言服务器（Tab 补全 status / setup / user）",
        kind: HelpKind::Slash(SlashCmd::Lsp),
    }),
    HelpEntry::Row(HelpRow {
        key: "/cordis",
        label: "动态 / 永久 Cordis 插件",
        kind: HelpKind::Slash(SlashCmd::Cordis),
    }),
    HelpEntry::Row(HelpRow {
        key: "/preset",
        label: "组装 Agent 预设（人设与工具）",
        kind: HelpKind::Slash(SlashCmd::Preset),
    }),
    HelpEntry::Row(HelpRow {
        key: "/quit",
        label: "退出",
        kind: HelpKind::Slash(SlashCmd::Quit),
    }),
    HelpEntry::Header("输入"),
    HelpEntry::Row(HelpRow {
        key: "↑↓",
        label: "翻历史提示词",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "←→",
        label: "移动光标",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "shift+enter",
        label: "换行",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "@",
        label: "搜索文件",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "tab",
        label: "补全斜杠命令或路径",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Header("对话"),
    HelpEntry::Row(HelpRow {
        key: "pgup",
        label: "滚动对话",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "click",
        label: "展开工具 / 复制 mermaid",
        kind: HelpKind::Hint,
    }),
];

pub fn matches_query(hay: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    hay.to_ascii_lowercase()
        .contains(&query.to_ascii_lowercase())
}

pub fn filter_sessions<'a>(items: &'a [ArchivedSession], query: &str) -> Vec<&'a ArchivedSession> {
    items
        .iter()
        .filter(|s| matches_query(&s.title, query) || matches_query(&s.id, query))
        .collect()
}

pub fn filter_strings<'a>(items: &'a [String], query: &str) -> Vec<(usize, &'a str)> {
    items
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, s)| matches_query(s, query))
        .map(|(i, s)| (i, s.as_str()))
        .collect()
}

pub fn filter_help(query: &str) -> Vec<&'static HelpRow> {
    HELP.iter()
        .filter_map(|e| match e {
            HelpEntry::Row(row)
                if matches_query(row.key, query) || matches_query(row.label, query) =>
            {
                Some(row)
            }
            _ => None,
        })
        .collect()
}

#[derive(Debug, Clone)]
pub enum HelpItem {
    Builtin(&'static HelpRow),
    Extra {
        key: String,
        label: String,
        command: String,
    },
}

pub fn filter_help_items(query: &str, extras: &[SlashEntry]) -> Vec<HelpItem> {
    let mut out: Vec<HelpItem> = filter_help(query)
        .into_iter()
        .map(HelpItem::Builtin)
        .collect();
    for extra in extras {
        let key = extra.display();
        if matches_query(&key, query) || matches_query(&extra.description, query) {
            out.push(HelpItem::Extra {
                key,
                label: extra.description.clone(),
                command: extra.command.clone(),
            });
        }
    }
    out
}

/// `fullscreen` = welcome-screen session picker (Grok `PickerMode::FullScreen`).
pub fn render_overlay(
    buf: &mut Buffer,
    area: Rect,
    title: &str,
    query: &str,
    rows: &[PickerRow<'_>],
    fullscreen: bool,
) -> PickerHits {
    let theme = Theme::current();
    let frame = if fullscreen {
        render_fullscreen_frame(buf, area, &theme, Some(title), false)
    } else {
        render_floating_frame(buf, area, &theme, false)
    };
    let Some(frame) = frame else {
        return PickerHits::default();
    };
    let skip_top = if fullscreen { 0 } else { 1 };
    let row_hits = render_picker_list(buf, frame.content, &theme, query, rows, skip_top);
    PickerHits {
        close_button: frame.close_button,
        rows: row_hits,
        ..Default::default()
    }
}

pub fn hit_index(hits: &PickerHits, column: u16, row: u16) -> Option<usize> {
    let pos = Position { x: column, y: row };
    if hits.close_button.contains(pos) {
        return None;
    }
    hits.rows
        .iter()
        .find(|(_, r)| r.contains(pos))
        .map(|(i, _)| *i)
}

pub fn hit_close(hits: &PickerHits, column: u16, row: u16) -> bool {
    hits.close_button.contains(Position { x: column, y: row })
}

pub fn hit_kill(hits: &PickerHits, column: u16, row: u16) -> Option<String> {
    let pos = Position { x: column, y: row };
    hits.kill_buttons
        .iter()
        .find(|(r, _)| r.contains(pos))
        .map(|(_, id)| id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_filters_case_insensitive() {
        assert!(matches_query("Hello World", "hello"));
        assert!(!matches_query("Hello World", "zzz"));
    }

    #[test]
    fn help_filter_finds_resume() {
        let rows = filter_help("resume");
        assert!(rows
            .iter()
            .any(|r| r.key.contains("resume") || r.label.contains("Resume")));
    }

    #[test]
    fn help_lists_shift_tab_mode_and_ctrl_x() {
        let rows = filter_help("");
        assert!(
            rows.iter().any(|r| r.key == "shift+tab"),
            "{:?}",
            rows.iter().map(|r| r.key).collect::<Vec<_>>()
        );
        assert!(rows.iter().any(|r| r.key == "ctrl+x"));
        assert!(rows.iter().any(|r| r.label.contains("询问")));
        assert!(rows.iter().any(|r| r.label.contains("计划")));
        assert!(rows.iter().any(|r| r.key == "/plan"));
        assert!(rows.iter().any(|r| r.key == "/goal"));
        assert!(rows.iter().any(|r| r.key == "/tasks"));
        assert!(rows.iter().any(|r| r.key == "/workflow"));
        assert!(rows.iter().any(|r| r.key == "/mcps"));
        assert!(rows.iter().any(|r| r.key == "/computer"));
        assert!(rows.iter().any(|r| r.key == "/lsp"));
        assert!(rows.iter().any(|r| r.key == "/preset"));
        assert!(rows.iter().any(|r| r.key == "/usage"));
        assert!(rows.iter().any(|r| r.key == "/context"));
        assert!(rows.iter().any(|r| r.key == "/compact"));
    }

    #[test]
    fn slot_overlay_opens_and_closes() {
        let mut overlay = Overlay::Slot {
            id: "memo".into(),
            scroll: 0,
        };
        assert!(overlay.is_open());
        overlay.close();
        assert!(!overlay.is_open());
    }

    #[test]
    fn browser_overlay_opens_and_closes() {
        let mut overlay = Overlay::Browser { scroll: 0 };
        assert!(overlay.is_open());
        // h toggles headed in event_loop keys (live-lookup "browser"); overlay itself is scroll-only.
        assert!(matches!(overlay, Overlay::Browser { scroll: 0 }));
        overlay.close();
        assert!(!overlay.is_open());
    }

    #[test]
    fn computer_overlay_opens_and_closes() {
        let mut overlay = Overlay::Computer { scroll: 0 };
        assert!(overlay.is_open());
        overlay.close();
        assert!(!overlay.is_open());
    }

    #[test]
    fn inspect_overlay_opens_from_helper() {
        let overlay = Overlay::inspect_subagent("kid-1", false);
        assert!(overlay.is_open());
        assert!(matches!(
            overlay,
            Overlay::Inspect {
                target: InspectTarget::Subagent(id),
                from_tasks: false,
                ..
            } if id == "kid-1"
        ));
    }

    #[test]
    fn help_appends_extra_slash() {
        let extra = SlashEntry {
            command: "standup".into(),
            description: "站会".into(),
            kind: cordis_spine::ExtraSlashKind::Prompt,
            text: "x".into(),
            title: String::new(),
            send: true,
        };
        let rows = filter_help_items("", &[extra]);
        assert!(rows.iter().any(|item| match item {
            HelpItem::Extra { command, .. } => command == "standup",
            _ => false,
        }));
    }
}
