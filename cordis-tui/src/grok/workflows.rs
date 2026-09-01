//! Workflow run overlay — snapshot + list/empty chrome copied from
//! grok-build/.../xai-grok-pager/src/views/workflows.rs.
//! Dual-pane detail / modal_window omitted; the floating picker frame is the
//! same chrome as other dock overlays.

use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::line_utils::truncate_str;
use crate::grok::picker::{render_divider, render_floating_frame, render_search_bar, PickerHits};
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowAgentRowView {
    pub agent_id: String,
    pub label: String,
    pub phase: Option<String>,
    pub model: Option<String>,
    pub state: String,
    pub tokens_used: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct WorkflowRunSnapshot {
    pub run_id: String,
    pub name: String,
    pub objective: String,
    pub status: String,
    pub management_available: bool,
    pub builtin: bool,
    pub phases: Vec<(String, String)>,
    pub current_phase: Option<String>,
    pub agents: Vec<WorkflowAgentRowView>,
    pub agent_budget: Option<u64>,
    pub agents_used: u64,
    pub agents_reserved: u64,
    pub agents_remaining: Option<u64>,
    pub agent_usage_incomplete: bool,
    pub active_agents: u32,
    pub elapsed_ms: u64,
    pub received_at: Instant,
    pub pause_message: Option<String>,
    pub result_summary: Option<String>,
}

impl WorkflowRunSnapshot {
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "interrupted" | "complete" | "failed" | "cancelled"
        )
    }

    #[allow(dead_code)]
    pub fn can_pause(&self) -> bool {
        self.management_available && self.is_active()
    }

    #[allow(dead_code)]
    pub fn can_resume(&self) -> bool {
        if !self.management_available {
            return false;
        }
        matches!(
            self.status.as_str(),
            "user_paused"
                | "back_off_paused"
                | "no_progress_paused"
                | "infra_paused"
                | "blocked"
                | "failed"
                | "cancelled"
        )
    }

    pub fn can_stop(&self) -> bool {
        self.management_available && !self.is_terminal()
    }

    pub fn live_elapsed_ms(&self) -> u64 {
        let base = self.elapsed_ms;
        if self.is_active() {
            base.saturating_add(self.received_at.elapsed().as_millis() as u64)
        } else {
            base
        }
    }
}

/// List-mode render copied from Grok `render_list` (empty copy + picker rows).
/// Title stays Grok chrome: `"Workflow Runs"`.
pub fn render_workflows_overlay(
    buf: &mut Buffer,
    area: Rect,
    runs: &[WorkflowRunSnapshot],
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
            ..Default::default()
        };
    }
    let title = Line::from(Span::styled(
        "Workflow Runs",
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
    if runs.is_empty() {
        span_at(
            buf,
            content.x + 1,
            list_y,
            "本会话还没有工作流运行。",
            Style::default().fg(theme.gray_bright),
            content.right(),
        );
        if list_h > 2 {
            span_at(
                buf,
                content.x + 1,
                list_y + 2,
                "用 /deep-research <查询> 启动，或把脚本放到 .dock/workflows/<name>.rhai。",
                Style::default().fg(theme.gray),
                content.right(),
            );
        }
        return PickerHits {
            close_button: frame.close_button,
            ..Default::default()
        };
    }
    let sel = selected.min(runs.len() - 1);
    let visible = list_h as usize;
    let start = sel.saturating_sub(visible.saturating_sub(1) / 2);
    let start = start.min(runs.len().saturating_sub(visible));
    let mut hits = Vec::new();
    for vis in 0..visible {
        let idx = start + vis;
        if idx >= runs.len() {
            break;
        }
        let y = list_y + vis as u16;
        let run = &runs[idx];
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
        let line = run_list_line(run, &theme);
        buf.set_line(content.x + 1, y, &line, content.width.saturating_sub(2));
        hits.push((idx, row_rect));
    }
    PickerHits {
        close_button: frame.close_button,
        rows: hits,
        ..Default::default()
    }
}

fn run_list_line(run: &WorkflowRunSnapshot, theme: &Theme) -> Line<'static> {
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
    let name_style = if running {
        Style::default().fg(theme.text_primary)
    } else {
        Style::default().fg(theme.gray_bright)
    };
    let suffix = if running {
        run.current_phase
            .clone()
            .unwrap_or_else(|| "running".into())
    } else {
        run.status.replace('_', " ")
    };
    Line::from(vec![
        Span::styled("Workflow ".to_string(), Style::default().fg(raw_tag_color)),
        Span::styled(run.name.clone(), name_style),
        Span::styled(
            format!(" \u{00b7} {suffix}"),
            Style::default().fg(theme.gray),
        ),
    ])
}

fn span_at(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style, right: u16) {
    let width = right.saturating_sub(x);
    let clipped = truncate_str(text, width as usize);
    buf.set_span(x, y, &Span::styled(clipped, style), width);
}

#[allow(dead_code)]
pub fn filter_runs<'a>(
    runs: &'a [WorkflowRunSnapshot],
    query: &str,
) -> Vec<&'a WorkflowRunSnapshot> {
    if query.is_empty() {
        return runs.iter().collect();
    }
    let q = query.to_ascii_lowercase();
    runs.iter()
        .filter(|r| {
            r.name.to_ascii_lowercase().contains(&q)
                || r.status.to_ascii_lowercase().contains(&q)
                || r.objective.to_ascii_lowercase().contains(&q)
        })
        .collect()
}
