//! Combined tasks pane model — copied from
//! grok-build/.../xai-grok-pager/src/views/tasks_pane.rs (`GroupKind`,
//! `TaskEntry`, constructors, header, sort, rebuild).
//! ListPane / syntect / kill-button overlay omitted; rows render through the
//! existing Grok picker chrome.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::{Instant, SystemTime};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::color::{blend_color, pulse_fg};
use crate::grok::picker::{render_divider, render_floating_frame, render_search_bar, PickerHits};
use crate::grok::workflows::WorkflowRunSnapshot;
use crate::theme::Theme;
use cordis_spine::{AgentPresets, CronJob, JobSnapshot, SubagentSnap};

/// Logical group a [`TaskEntry`] belongs to. Drives both the sort order (so
/// each kind is contiguous) and the collapsible group headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKind {
    Workflows,
    Subagents,
    Tasks,
    /// Recurring background processes: `monitor` tasks and `/loop` scheduled
    /// tasks share one section. They stay contiguous (monitors first, then
    /// loops) via [`TaskEntry::type_order`].
    Watchers,
}

const GROUP_KIND_COUNT: usize = 4;

impl GroupKind {
    /// Display label shown in the group header.
    pub fn label(self) -> &'static str {
        match self {
            GroupKind::Workflows => "Workflows",
            GroupKind::Subagents => "Subagents",
            GroupKind::Tasks => "Tasks",
            GroupKind::Watchers => "Watchers",
        }
    }

    fn order(self) -> u8 {
        match self {
            GroupKind::Workflows => 0,
            GroupKind::Subagents => 1,
            GroupKind::Tasks => 2,
            GroupKind::Watchers => 3,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TaskEntry {
    BgTask {
        id: u64,
        task_id: String,
        label: String,
        styled: Line<'static>,
        running: bool,
        start_time: SystemTime,
        is_monitor: bool,
    },
    Agent {
        id: u64,
        subagent_id: String,
        child_session_id: String,
        label: String,
        styled: Line<'static>,
        running: bool,
        started_at: Instant,
        type_label: String,
    },
    Scheduled {
        id: u64,
        task_id: String,
        label: String,
        styled: Line<'static>,
        started_at: Instant,
        linked_subagent: Option<String>,
    },
    Workflow {
        id: u64,
        name: String,
        label: String,
        styled: Line<'static>,
        running: bool,
        stoppable: bool,
        started_at: Instant,
    },
    /// Collapsible group header row (e.g. `▾ Subagents 2`). Not a task —
    /// selecting it and pressing Enter toggles the group's collapse state.
    Header {
        group: GroupKind,
        styled: Line<'static>,
    },
}

impl TaskEntry {
    /// Copied from Grok `TaskEntry::from_bg_task`, adapted to [`JobSnapshot`].
    pub fn from_bg_task(task: &JobSnapshot) -> Self {
        let description = task
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let running = !task.done;
        let (label, styled) = if task.is_monitor {
            let theme = Theme::current();
            let text = description
                .map(|d| d.replace('\n', " "))
                .unwrap_or_else(|| task.command.trim().replace('\n', " "));
            const TAG: &str = "Monitor";
            let desc_style = if running {
                Style::default().fg(theme.text_secondary)
            } else {
                Style::default().fg(theme.gray_bright)
            };
            let label = format!("{TAG} {text}");
            let styled = Line::from(vec![
                Span::styled(format!("{TAG} "), Style::default().fg(theme.accent_system)),
                Span::styled(text, desc_style),
            ]);
            (label, styled)
        } else if let Some(desc) = description {
            let one_line = desc.replace('\n', " ");
            let theme = Theme::current();
            const PREFIX: &str = "Task ";
            let desc_style = if running {
                Style::default().fg(theme.text_primary)
            } else {
                Style::default().fg(theme.gray_bright)
            };
            let label = format!("{PREFIX}{one_line}");
            let styled = Line::from(vec![
                Span::styled(PREFIX, Style::default().fg(theme.text_secondary)),
                Span::styled(one_line, desc_style),
            ]);
            (label, styled)
        } else {
            let trimmed = task.command.trim();
            let label = if let Some(nl) = trimmed.find('\n') {
                let first_line = trimmed[..nl].trim_end();
                format!("{first_line}\u{2026}")
            } else {
                trimmed.to_string()
            };
            let theme = Theme::current();
            let style = if running {
                Style::default().fg(theme.command)
            } else {
                Style::default().fg(theme.gray_bright)
            };
            (label.clone(), Line::from(Span::styled(label, style)))
        };

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        task.id.hash(&mut hasher);
        let id = hasher.finish();

        TaskEntry::BgTask {
            id,
            task_id: task.id.clone(),
            label,
            styled,
            running,
            start_time: task.start_time,
            is_monitor: task.is_monitor,
        }
    }

    /// Copied from Grok `TaskEntry::from_subagent`, adapted to [`SubagentSnap`].
    /// `role` is the current mode's `agents/<type>.yml` display name (e.g. 岑).
    pub fn from_subagent(info: &SubagentSnap, role: Option<&str>) -> Self {
        let theme = Theme::current();
        let type_label = {
            let tag = role
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| capitalize_tag(&info.subagent_type));
            if info.idle && !info.done {
                format!("{tag} idle")
            } else {
                tag
            }
        };
        let description = info.description.clone();
        let running = info.running();
        let raw_type_color = if info.cancelled {
            theme.accent_error
        } else if running {
            theme.accent_running
        } else if info.idle {
            theme.gray_bright
        } else {
            theme.accent_success
        };
        let type_color = if running {
            pulse_fg(theme.bg_base, raw_type_color)
        } else if info.cancelled {
            raw_type_color
        } else {
            blend_color(theme.bg_base, raw_type_color, 0.45).unwrap_or(raw_type_color)
        };
        let type_style = Style::default().fg(type_color);
        let desc_style = if running {
            Style::default().fg(theme.text_primary)
        } else {
            Style::default().fg(theme.gray_bright)
        };
        let type_sep = if description.is_empty() { "" } else { " " };
        let spans = vec![
            Span::styled(format!("{type_label}{type_sep}"), type_style),
            Span::styled(description.clone(), desc_style),
        ];
        let label = if description.is_empty() {
            type_label.clone()
        } else {
            format!("{type_label} {description}")
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "agent:".hash(&mut hasher);
        info.id.hash(&mut hasher);
        let id = hasher.finish();
        TaskEntry::Agent {
            id,
            subagent_id: info.id.clone(),
            child_session_id: info.id.clone(),
            label,
            styled: Line::from(spans),
            running,
            started_at: info.started_at,
            type_label,
        }
    }

    /// Copied from Grok `TaskEntry::from_workflow_run`.
    pub fn from_workflow_run(run: &WorkflowRunSnapshot) -> Self {
        let theme = Theme::current();
        let running = run.is_active();
        let raw_tag_color = if running {
            theme.accent_running
        } else if run.status == "complete" {
            theme.accent_success
        } else if run.is_terminal() {
            theme.accent_error
        } else {
            theme.warning
        };
        let tag_color = if running {
            pulse_fg(theme.bg_base, raw_tag_color)
        } else {
            blend_color(theme.bg_base, raw_tag_color, 0.45).unwrap_or(raw_tag_color)
        };
        let name_style = if running {
            Style::default().fg(theme.text_primary)
        } else {
            Style::default().fg(theme.gray_bright)
        };
        let suffix = if running {
            let phase = run
                .current_phase
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty());
            let agents = match run.agents.iter().filter(|a| a.state == "running").count() {
                0 => None,
                1 => Some("1 agent".to_string()),
                n => Some(format!("{n} agents")),
            };
            match (phase, agents) {
                (Some(p), Some(a)) => format!("{p} · {a}"),
                (Some(p), None) => p.to_string(),
                (None, Some(a)) => a,
                (None, None) => "running".to_string(),
            }
        } else {
            run.status.replace('_', " ")
        };
        let mut spans = vec![
            Span::styled("Workflow ".to_string(), Style::default().fg(tag_color)),
            Span::styled(run.name.clone(), name_style),
        ];
        if !suffix.is_empty() {
            spans.push(Span::styled(
                format!(" \u{00b7} {suffix}"),
                Style::default().fg(theme.gray),
            ));
        }
        let label = format!("Workflow {} {suffix}", run.name);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "workflow:".hash(&mut hasher);
        run.run_id.hash(&mut hasher);
        let id = hasher.finish();
        TaskEntry::Workflow {
            id,
            name: run.name.clone(),
            label,
            styled: Line::from(spans),
            running,
            stoppable: run.can_stop(),
            started_at: Instant::now()
                .checked_sub(std::time::Duration::from_millis(run.live_elapsed_ms()))
                .unwrap_or_else(Instant::now),
        }
    }

    /// Copied from Grok `TaskEntry::from_scheduled`, adapted to [`CronJob`].
    pub fn from_scheduled(info: &CronJob) -> Self {
        let theme = Theme::current();
        let prompt_preview = if info.prompt.chars().count() > 60 {
            info.prompt.chars().take(57).collect::<String>() + "..."
        } else {
            info.prompt.clone()
        };
        let human_schedule = format_interval(info.every);
        let now = Instant::now();
        let suffix = if info.next > now {
            format!(
                " (next in {})",
                format_duration(info.next.duration_since(now))
            )
        } else {
            " (due now)".to_string()
        };
        let tag_display = "Loop";
        let label = format!(
            "{} {} \u{b7} {}{}",
            tag_display, human_schedule, &prompt_preview, &suffix
        );
        let schedule_style = format!("{} \u{b7} ", human_schedule);
        let neutral = Style::default().fg(theme.text_secondary);
        let styled = Line::from(vec![
            Span::styled(
                format!("{tag_display} "),
                Style::default().fg(theme.accent_system),
            ),
            Span::styled(schedule_style, neutral),
            Span::styled(prompt_preview, neutral),
            Span::styled(suffix, neutral),
        ]);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "sched:".hash(&mut hasher);
        info.id.hash(&mut hasher);
        let id = hasher.finish();
        TaskEntry::Scheduled {
            id,
            task_id: info.id.clone(),
            label,
            styled,
            started_at: info.created_at,
            linked_subagent: None,
        }
    }

    /// Build a collapsible group header row, e.g. `▾ Subagents 2` (expanded)
    /// or `▸ Subagents 2` (collapsed).
    pub fn header(group: GroupKind, count: usize, collapsed: bool) -> Self {
        let theme = Theme::current();
        let chevron = if collapsed { "\u{25B8} " } else { "\u{25BE} " };
        let styled = Line::from(vec![
            Span::styled(chevron, Style::default().fg(theme.gray)),
            Span::styled(
                group.label(),
                Style::default()
                    .fg(theme.gray_bright)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {count}"), Style::default().fg(theme.gray)),
        ]);
        TaskEntry::Header { group, styled }
    }

    pub fn group_kind(&self) -> GroupKind {
        match self {
            TaskEntry::Agent { .. } => GroupKind::Subagents,
            TaskEntry::BgTask {
                is_monitor: false, ..
            } => GroupKind::Tasks,
            TaskEntry::BgTask {
                is_monitor: true, ..
            } => GroupKind::Watchers,
            TaskEntry::Scheduled { .. } => GroupKind::Watchers,
            TaskEntry::Workflow { .. } => GroupKind::Workflows,
            TaskEntry::Header { group, .. } => *group,
        }
    }

    fn is_running(&self) -> bool {
        match self {
            TaskEntry::BgTask { running, .. }
            | TaskEntry::Agent { running, .. }
            | TaskEntry::Workflow { running, .. } => *running,
            TaskEntry::Scheduled { .. } => true,
            TaskEntry::Header { .. } => false,
        }
    }

    fn type_order(&self) -> u8 {
        match self {
            TaskEntry::Workflow { .. } => 0,
            TaskEntry::Agent { .. } => 1,
            TaskEntry::BgTask {
                is_monitor: false, ..
            } => 2,
            TaskEntry::BgTask {
                is_monitor: true, ..
            } => 3,
            TaskEntry::Scheduled { .. } => 4,
            TaskEntry::Header { group, .. } => group.order(),
        }
    }

    pub fn is_header(&self) -> bool {
        matches!(self, TaskEntry::Header { .. })
    }

    pub fn header_group(&self) -> Option<GroupKind> {
        match self {
            TaskEntry::Header { group, .. } => Some(*group),
            _ => None,
        }
    }

    fn styled(&self) -> &Line<'static> {
        match self {
            TaskEntry::BgTask { styled, .. }
            | TaskEntry::Agent { styled, .. }
            | TaskEntry::Scheduled { styled, .. }
            | TaskEntry::Workflow { styled, .. }
            | TaskEntry::Header { styled, .. } => styled,
        }
    }

    fn search_text(&self) -> &str {
        match self {
            TaskEntry::BgTask { label, .. }
            | TaskEntry::Agent { label, .. }
            | TaskEntry::Scheduled { label, .. }
            | TaskEntry::Workflow { label, .. } => label,
            TaskEntry::Header { group, .. } => group.label(),
        }
    }

    fn stable_id(&self) -> u64 {
        match self {
            TaskEntry::BgTask { id, .. }
            | TaskEntry::Agent { id, .. }
            | TaskEntry::Scheduled { id, .. }
            | TaskEntry::Workflow { id, .. } => *id,
            TaskEntry::Header { group, .. } => {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                "header:".hash(&mut hasher);
                group.order().hash(&mut hasher);
                hasher.finish()
            }
        }
    }

    fn is_header_row(&self) -> bool {
        matches!(self, TaskEntry::Header { .. })
    }
}

pub fn collect_items(
    jobs: &[JobSnapshot],
    subagents: &[SubagentSnap],
    cron: &[CronJob],
    workflows: &[WorkflowRunSnapshot],
    presets: Option<&AgentPresets>,
) -> Vec<TaskEntry> {
    let mut items = Vec::new();
    for job in jobs {
        items.push(TaskEntry::from_bg_task(job));
    }
    for sa in subagents {
        let role = presets.and_then(|p| p.role_label(&sa.subagent_type));
        items.push(TaskEntry::from_subagent(sa, role.as_deref()));
    }
    for c in cron {
        items.push(TaskEntry::from_scheduled(c));
    }
    for run in workflows {
        items.push(TaskEntry::from_workflow_run(run));
    }
    items.sort_by(|a, b| {
        a.type_order()
            .cmp(&b.type_order())
            .then_with(|| b.is_running().cmp(&a.is_running()))
            .then_with(|| match (a, b) {
                (
                    TaskEntry::Agent {
                        type_label: ta,
                        started_at: sa,
                        ..
                    },
                    TaskEntry::Agent {
                        type_label: tb,
                        started_at: sb,
                        ..
                    },
                ) => ta.cmp(tb).then_with(|| sb.cmp(sa)),
                (
                    TaskEntry::Scheduled { started_at: a, .. },
                    TaskEntry::Scheduled { started_at: b, .. },
                ) => b.cmp(a),
                (
                    TaskEntry::BgTask { start_time: a, .. },
                    TaskEntry::BgTask { start_time: b, .. },
                ) => b.cmp(a),
                (
                    TaskEntry::Workflow { started_at: a, .. },
                    TaskEntry::Workflow { started_at: b, .. },
                ) => b.cmp(a),
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.stable_id().cmp(&b.stable_id()))
    });
    items
}

/// Rebuild the display list: insert a header at each group boundary and hide
/// collapsed groups' items. Copied from Grok `TasksPane::rebuild_entries`.
pub fn rebuild_entries(items: &[TaskEntry], collapsed: &HashSet<GroupKind>) -> Vec<TaskEntry> {
    let mut entries = Vec::new();
    let mut counts: [usize; GROUP_KIND_COUNT] = [0; GROUP_KIND_COUNT];
    for it in items {
        counts[it.group_kind().order() as usize] += 1;
    }
    let mut last: Option<GroupKind> = None;
    for it in items {
        let group = it.group_kind();
        if last != Some(group) {
            let is_collapsed = collapsed.contains(&group);
            entries.push(TaskEntry::header(
                group,
                counts[group.order() as usize],
                is_collapsed,
            ));
            last = Some(group);
        }
        if !collapsed.contains(&group) {
            entries.push(it.clone());
        }
    }
    entries
}

pub fn filter_entries(entries: Vec<TaskEntry>, query: &str) -> Vec<TaskEntry> {
    if query.is_empty() {
        return entries;
    }
    let q = query.to_ascii_lowercase();
    entries
        .into_iter()
        .filter(|e| e.search_text().to_ascii_lowercase().contains(&q) || e.is_header_row())
        .collect()
}

pub fn render_tasks_overlay(
    buf: &mut Buffer,
    area: Rect,
    entries: &[TaskEntry],
    selected: usize,
    query: &str,
) -> PickerHits {
    let theme = Theme::current();
    let Some(frame) = render_floating_frame(buf, area, &theme, false) else {
        return PickerHits::default();
    };
    let content = frame.content;
    if content.height <= 3 || content.width < 8 {
        return PickerHits {
            close_button: frame.close_button,
            rows: Vec::new(),
        };
    }
    let title = Line::from(Span::styled(
        "Tasks",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    ));
    buf.set_line(
        content.x + 1,
        content.y,
        &title,
        content.width.saturating_sub(2),
    );
    let search_y = content.y + 1;
    render_search_bar(
        buf,
        content.x,
        search_y,
        content.width,
        &theme,
        query,
        true,
        Some(theme.bg_base),
    );
    render_divider(
        buf,
        content.x,
        search_y + 1,
        content.width,
        &theme,
        Some(theme.bg_base),
    );
    let list_y = search_y + 2;
    let list_h = content
        .y
        .saturating_add(content.height)
        .saturating_sub(list_y);
    if entries.is_empty() {
        let empty = Line::from(Span::styled(
            "(none)",
            Style::default().fg(theme.gray_bright),
        ));
        buf.set_line(
            content.x + 1,
            list_y,
            &empty,
            content.width.saturating_sub(2),
        );
        return PickerHits {
            close_button: frame.close_button,
            rows: Vec::new(),
        };
    }
    let sel = selected.min(entries.len() - 1);
    let visible = list_h as usize;
    let start = sel.saturating_sub(visible.saturating_sub(1) / 2);
    let start = start.min(entries.len().saturating_sub(visible));
    let mut hits = Vec::new();
    for vis in 0..visible {
        let idx = start + vis;
        if idx >= entries.len() {
            break;
        }
        let y = list_y + vis as u16;
        let selected_row = idx == sel;
        let row_bg = if selected_row {
            theme.bg_visual
        } else {
            theme.bg_base
        };
        let row_rect = Rect {
            x: content.x,
            y,
            width: content.width,
            height: 1,
        };
        buf.set_style(row_rect, Style::default().bg(row_bg));
        let indent = if entries[idx].is_header_row() { 0 } else { 2 };
        let mut line = entries[idx].styled().clone();
        if indent > 0 {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(line.spans);
            line.spans = spans;
        }
        let clipped =
            crate::grok::line_utils::truncate_line(line, content.width.saturating_sub(1) as usize);
        buf.set_line(content.x, y, &clipped, content.width);
        hits.push((idx, row_rect));
    }
    PickerHits {
        close_button: frame.close_button,
        rows: hits,
    }
}

fn capitalize_tag(tag: &str) -> String {
    let mut chars = tag.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn format_interval(every: std::time::Duration) -> String {
    let secs = every.as_secs().max(1);
    if secs % 3600 == 0 {
        format!("{}h", secs / 3600)
    } else if secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn group_headers_and_collapse() {
        let job = JobSnapshot {
            id: "job-1".into(),
            command: "echo hi".into(),
            done: false,
            output: String::new(),
            description: Some("say hi".into()),
            is_monitor: false,
            start_time: SystemTime::now(),
        };
        let items = collect_items(&[job], &[], &[], &[], None);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].group_kind(), GroupKind::Tasks);
        let rows = rebuild_entries(&items, &HashSet::new());
        assert!(rows[0].is_header());
        assert_eq!(rows[0].header_group(), Some(GroupKind::Tasks));
        assert_eq!(rows.len(), 2);
        let mut collapsed = HashSet::new();
        collapsed.insert(GroupKind::Tasks);
        let hidden = rebuild_entries(&items, &collapsed);
        assert_eq!(hidden.len(), 1);
        assert!(hidden[0].is_header());
    }

    #[test]
    fn monitors_and_loops_share_watchers() {
        let mon = JobSnapshot {
            id: "job-2".into(),
            command: "tail -f log".into(),
            done: false,
            output: String::new(),
            description: Some("watch log".into()),
            is_monitor: true,
            start_time: SystemTime::now(),
        };
        let loop_job = CronJob {
            id: "cron-1".into(),
            every: Duration::from_secs(60),
            prompt: "status".into(),
            created_at: Instant::now(),
            next: Instant::now() + Duration::from_secs(60),
        };
        let items = collect_items(&[mon], &[], &[loop_job], &[], None);
        assert!(items.iter().all(|i| i.group_kind() == GroupKind::Watchers));
        let rows = rebuild_entries(&items, &HashSet::new());
        assert_eq!(rows[0].header_group(), Some(GroupKind::Watchers));
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn subagent_row_uses_roster_display_name() {
        let snap = SubagentSnap {
            id: "k".into(),
            description: "测绘入口".into(),
            subagent_type: "岑".into(),
            done: false,
            idle: true,
            cancelled: false,
            output: String::new(),
            started_at: Instant::now(),
            mailbox: false,
        };
        let entry = TaskEntry::from_subagent(&snap, Some("岑"));
        match entry {
            TaskEntry::Agent {
                type_label, label, ..
            } => {
                assert!(type_label.contains("岑"), "{type_label}");
                assert!(!type_label.contains("Cen"), "{type_label}");
                assert!(label.contains("测绘入口"), "{label}");
            }
            other => panic!("expected Agent, got {other:?}"),
        }
    }
}
