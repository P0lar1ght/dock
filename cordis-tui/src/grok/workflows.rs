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
    /// 这个子代理最后一条 `report`。长跑阶段里唯一能看出它在干什么的东西。
    pub latest_report: Option<String>,
}

impl WorkflowAgentRowView {
    pub fn running(&self) -> bool {
        self.state == "running"
    }
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
    /// 脚本 `log()` 的最近几条，新的在后。`Arc` 是因为这张表在 80ms 心跳里被
    /// 整体克隆。
    pub logs: std::sync::Arc<Vec<String>>,
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

    /// 最新一条脚本进度。长跑工作流靠它让用户知道还活着。
    pub fn latest_log(&self) -> Option<&str> {
        self.logs.last().map(String::as_str)
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

/// 左栏宽度：阶段标题最长的那个 + 编号 + 计数，夹在一个不至于挤掉右栏的范围里。
const PHASE_COL_MIN: u16 = 16;
const PHASE_COL_MAX: u16 = 28;

/// 一个 run 的详情页：左边阶段清单，右边该阶段的子代理。
///
/// 阶段来自脚本的 `meta.phases`；子代理按 `agent(phase:)` 归到阶段下，没标
/// `phase` 的归到「其它」。脚本没声明阶段时只剩右栏。
pub fn render_workflow_detail(
    buf: &mut Buffer,
    area: Rect,
    run: &WorkflowRunSnapshot,
    phase: usize,
) -> PickerHits {
    let theme = Theme::current();
    let Some(frame) = render_floating_frame(buf, area, &theme, false) else {
        return PickerHits::default();
    };
    let content = frame.content;
    if content.height <= 4 || content.width < 24 {
        return PickerHits {
            close_button: frame.close_button,
            ..Default::default()
        };
    }

    // 抬头：`:: name` + 目标 + 右上角 `n/m agent · 耗时`
    let title = Line::from(vec![
        Span::styled(":: ", Style::default().fg(theme.accent_running)),
        Span::styled(
            run.name.clone(),
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    buf.set_line(
        content.x + 1,
        content.y,
        &title,
        content.width.saturating_sub(2),
    );
    let budget = match run.agent_budget {
        Some(total) => format!("{}/{} agent", run.agents_used, total),
        None => format!("{} agent", run.agents_used),
    };
    let right = format!("{budget} · {}", fmt_elapsed(run.live_elapsed_ms()));
    let right_x = content
        .right()
        .saturating_sub(right.chars().count() as u16 + 1);
    if right_x > content.x + 2 + run.name.chars().count() as u16 {
        span_at(
            buf,
            right_x,
            content.y,
            &right,
            Style::default().fg(theme.gray),
            content.right(),
        );
    }
    span_at(
        buf,
        content.x + 1,
        content.y + 1,
        &run.objective,
        Style::default().fg(theme.gray_bright),
        content.right(),
    );
    render_divider(
        buf,
        content.x,
        content.y + 2,
        content.width,
        &theme,
        Some(theme.bg_base),
    );

    let body_y = content.y + 3;
    let footer_h = 1;
    let body_h = content
        .y
        .saturating_add(content.height)
        .saturating_sub(body_y + footer_h);
    if body_h == 0 {
        return PickerHits {
            close_button: frame.close_button,
            ..Default::default()
        };
    }

    let buckets = phase_buckets(run);
    let sel = phase.min(buckets.len().saturating_sub(1));
    let phase_w = if buckets.len() <= 1 {
        0
    } else {
        let widest = buckets
            .iter()
            .map(|b| b.title.chars().count() as u16 + 8)
            .max()
            .unwrap_or(PHASE_COL_MIN);
        widest
            .clamp(PHASE_COL_MIN, PHASE_COL_MAX)
            .min(content.width / 2)
    };

    let mut rows = Vec::new();
    if phase_w > 0 {
        span_at(
            buf,
            content.x + 1,
            body_y,
            "Phases",
            Style::default().fg(theme.gray),
            content.x + phase_w,
        );
        for (i, bucket) in buckets.iter().enumerate() {
            let y = body_y + 1 + i as u16;
            if y >= body_y + body_h {
                break;
            }
            let picked = i == sel;
            let row_rect = Rect {
                x: content.x,
                y,
                width: phase_w,
                height: 1,
            };
            if picked {
                buf.set_style(row_rect, Style::default().bg(theme.bg_visual));
            }
            let marker = if picked { "❯ " } else { "  " };
            let title_style = if picked {
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.gray_bright)
            };
            let mut spans = vec![
                Span::styled(
                    marker.to_string(),
                    Style::default().fg(theme.accent_running),
                ),
                Span::styled(format!("{} ", i + 1), Style::default().fg(theme.gray)),
                Span::styled(bucket.title.clone(), title_style),
            ];
            if bucket.total > 0 {
                spans.push(Span::styled(
                    format!(" {}/{}", bucket.done, bucket.total),
                    Style::default().fg(theme.gray),
                ));
            }
            let line = crate::grok::line_utils::truncate_line(
                Line::from(spans),
                phase_w.saturating_sub(1) as usize,
            );
            buf.set_line(content.x, y, &line, phase_w);
            rows.push((i, row_rect));
        }
        for y in body_y..body_y + body_h {
            buf.set_span(
                content.x + phase_w,
                y,
                &Span::styled("│", Style::default().fg(theme.gray)),
                1,
            );
        }
    }

    // 右栏：这一阶段的子代理。
    let agents_x = content.x + phase_w + if phase_w > 0 { 2 } else { 1 };
    let header = match buckets.get(sel) {
        Some(b) if b.total > 0 => format!("{} · {} agent", b.title, b.total),
        Some(b) => b.title.clone(),
        None => "agents".to_string(),
    };
    span_at(
        buf,
        agents_x,
        body_y,
        &header,
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
        content.right(),
    );
    let empty = Vec::new();
    let agents = buckets.get(sel).map(|b| &b.agents).unwrap_or(&empty);
    if agents.is_empty() {
        span_at(
            buf,
            agents_x,
            body_y + 1,
            "这一阶段还没有子代理。",
            Style::default().fg(theme.gray),
            content.right(),
        );
    }
    let mut y = body_y + 1;
    for row in agents {
        if y >= body_y + body_h {
            break;
        }
        buf.set_line(
            agents_x,
            y,
            &agent_line(row, &theme),
            content.right().saturating_sub(agents_x),
        );
        y += 1;
        // 上报单起一行缩进：它是这个孩子在干什么的唯一线索。
        if let Some(report) = row.latest_report.as_deref() {
            if y < body_y + body_h {
                let report_fg = if row.state == "failed" {
                    theme.accent_error
                } else {
                    theme.gray
                };
                span_at(
                    buf,
                    agents_x + 2,
                    y,
                    report,
                    Style::default().fg(report_fg),
                    content.right(),
                );
                y += 1;
            }
        }
    }

    let footer_y = body_y + body_h;
    if footer_y < content.y.saturating_add(content.height) {
        let mut hint = String::new();
        if buckets.len() > 1 {
            hint.push_str("↑↓ 阶段  ");
        }
        if run.can_stop() {
            hint.push_str("x 停止  ");
        }
        hint.push_str("esc 返回");
        span_at(
            buf,
            content.x + 1,
            footer_y,
            &hint,
            Style::default().fg(theme.gray),
            content.right(),
        );
    }

    PickerHits {
        close_button: frame.close_button,
        rows,
        ..Default::default()
    }
}

/// 一个阶段，连同归到它名下的子代理。
pub struct PhaseBucket<'a> {
    pub title: String,
    pub agents: Vec<&'a WorkflowAgentRowView>,
    pub done: usize,
    pub total: usize,
}

/// 把子代理按 `agent(phase:)` 归到声明的阶段下。
///
/// 没标 `phase` 的孩子归到末尾的「其它」——脚本不声明阶段时那就是唯一一栏，
/// 左栏会整个收掉。
pub fn phase_buckets(run: &WorkflowRunSnapshot) -> Vec<PhaseBucket<'_>> {
    let mut buckets: Vec<PhaseBucket<'_>> = run
        .phases
        .iter()
        .map(|(title, _)| PhaseBucket {
            title: title.clone(),
            agents: Vec::new(),
            done: 0,
            total: 0,
        })
        .collect();
    let mut loose: Vec<&WorkflowAgentRowView> = Vec::new();
    for row in &run.agents {
        // 没写 `agent(phase:)` 的，跑着时跟当前 `phase()` 走——否则 planner
        // 会在 Plan 栏显示「这一阶段还没有子代理」，自己掉进「其它」。
        let tagged = row
            .phase
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let fallback = if row.running() {
            run.current_phase
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        let slot = tagged
            .or(fallback)
            .and_then(|p| buckets.iter_mut().find(|b| b.title.trim() == p));
        match slot {
            Some(bucket) => bucket.agents.push(row),
            None => loose.push(row),
        }
    }
    if !loose.is_empty() || buckets.is_empty() {
        buckets.push(PhaseBucket {
            title: if buckets.is_empty() {
                "Agents".into()
            } else {
                "其它".into()
            },
            agents: loose,
            done: 0,
            total: 0,
        });
    }
    for bucket in &mut buckets {
        bucket.total = bucket.agents.len();
        bucket.done = bucket.agents.iter().filter(|a| !a.running()).count();
    }
    buckets
}

fn agent_line(row: &WorkflowAgentRowView, theme: &Theme) -> Line<'static> {
    let color = match row.state.as_str() {
        "running" => theme.accent_running,
        "done" => theme.accent_success,
        "cancelled" => theme.warning,
        _ => theme.accent_error,
    };
    let mut spans = vec![
        Span::styled(":: ".to_string(), Style::default().fg(color)),
        Span::styled(row.label.clone(), Style::default().fg(theme.text_primary)),
    ];
    let state = match row.state.as_str() {
        "running" => "运行中".to_string(),
        "done" => "完成".to_string(),
        "cancelled" => "已取消".to_string(),
        other => other.to_string(),
    };
    spans.push(Span::styled(
        format!(" · {state}"),
        Style::default().fg(theme.gray),
    ));
    if row.tokens_used > 0 {
        spans.push(Span::styled(
            format!(" · {}", fmt_tokens(row.tokens_used)),
            Style::default().fg(theme.gray),
        ));
    }
    if row.duration_ms > 0 {
        spans.push(Span::styled(
            format!(" · {}", fmt_elapsed(row.duration_ms)),
            Style::default().fg(theme.gray),
        ));
    }
    Line::from(spans)
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

fn fmt_elapsed(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 60 {
        format!("{}:{:02}", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
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
    // 在跑时优先给阶段，没阶段就给最新一条脚本进度 —— 长跑工作流前几十秒只有
    // `log()`，没有这一条就只剩一个 "running"。
    let suffix = if running {
        run.current_phase
            .clone()
            .or_else(|| run.latest_log().map(str::to_owned))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(label: &str, phase: Option<&str>, state: &str) -> WorkflowAgentRowView {
        WorkflowAgentRowView {
            agent_id: label.into(),
            label: label.into(),
            phase: phase.map(str::to_owned),
            model: None,
            state: state.into(),
            tokens_used: 0,
            duration_ms: 0,
            latest_report: None,
        }
    }

    fn run(phases: &[&str], agents: Vec<WorkflowAgentRowView>) -> WorkflowRunSnapshot {
        WorkflowRunSnapshot {
            run_id: "wf_1".into(),
            name: "probe".into(),
            objective: "o".into(),
            status: "active".into(),
            management_available: true,
            builtin: false,
            phases: phases
                .iter()
                .map(|t| ((*t).to_string(), String::new()))
                .collect(),
            current_phase: None,
            agents,
            agent_budget: Some(8),
            agents_used: 0,
            agents_reserved: 0,
            agents_remaining: None,
            agent_usage_incomplete: false,
            active_agents: 0,
            elapsed_ms: 0,
            received_at: Instant::now(),
            pause_message: None,
            result_summary: None,
            logs: std::sync::Arc::new(Vec::new()),
        }
    }

    #[test]
    fn agents_group_under_their_declared_phase() {
        let run = run(
            &["Plan", "Research"],
            vec![
                agent("planner", Some("Plan"), "done"),
                agent("r1", Some("Research"), "running"),
                agent("r2", Some("Research"), "done"),
            ],
        );
        let buckets = phase_buckets(&run);
        assert_eq!(buckets.len(), 2, "没有落单的孩子就不该多出「其它」");
        assert_eq!(buckets[0].title, "Plan");
        assert_eq!((buckets[0].done, buckets[0].total), (1, 1));
        assert_eq!((buckets[1].done, buckets[1].total), (1, 2));
    }

    /// 没标 `phase` 的孩子要有地方去，不能从详情页上消失。
    #[test]
    fn unphased_agents_fall_into_a_trailing_bucket() {
        let run = run(
            &["Plan"],
            vec![
                agent("planner", Some("Plan"), "done"),
                agent("stray", None, "running"),
                agent("other", Some("没声明过的阶段"), "running"),
            ],
        );
        let buckets = phase_buckets(&run);
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[1].title, "其它");
        assert_eq!(buckets[1].total, 2, "落单的和标了未知阶段的都归这里");
    }

    /// 跑着、又没写 `agent(phase:)` 的孩子跟当前 `phase()` 走。
    /// 详情页才会在 Plan 栏看到 planner，而不是「这一阶段还没有子代理」。
    #[test]
    fn a_running_unphased_agent_follows_the_current_phase() {
        let mut run = run(
            &["Plan", "Research"],
            vec![agent("research-planner", None, "running")],
        );
        run.current_phase = Some("Plan".into());
        let buckets = phase_buckets(&run);
        assert!(
            buckets.iter().all(|b| b.title != "其它"),
            "不该多出「其它」：{:?}",
            buckets.iter().map(|b| &b.title).collect::<Vec<_>>()
        );
        assert_eq!(buckets[0].title, "Plan");
        assert_eq!(buckets[0].total, 1);
        assert_eq!(buckets[0].agents[0].label, "research-planner");
    }

    /// 脚本没声明阶段时只剩一栏，左栏整个收掉。
    #[test]
    fn a_script_without_phases_gets_one_bucket() {
        let run = run(&[], vec![agent("a", None, "running")]);
        let buckets = phase_buckets(&run);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].title, "Agents");
        assert_eq!(buckets[0].total, 1);
    }

    /// 一个孩子都没有的 run 也要能画出来。
    #[test]
    fn an_empty_run_still_has_a_bucket() {
        let run = run(&[], Vec::new());
        assert_eq!(phase_buckets(&run).len(), 1);
    }
}
