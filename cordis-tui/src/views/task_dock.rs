//! 主界面统一任务条：后台 bash、monitor、子代理、`/loop`、workflow 共用一条。
//!
//! 取代原来只画子代理头像方块的 dock —— 那条用 3 行换一个角色名首字，其余四类
//! 耗时任务在主界面上完全不可见，只能翻 scrollback 或开 `/tasks`。
//!
//! 行的描述列直接用 `tasks_pane::TaskEntry` 已经算好的 `styled`（含 `Task` /
//! `Monitor` 前缀、名册显示名、running 呼吸色），所以任务条与 `/tasks` 永远是
//! 同一套文案和配色，不会一边改一边忘。
//!
//! 本模块只负责渲染：数据由调用方收集好传进来（同 `queue_pane`），这样用例不需要
//! 造 `Context` 就能覆盖布局、溢出和命中测试。

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::grok::glyphs;
use crate::grok::line_utils::{truncate_line, truncate_str};
use crate::grok::tasks_pane::KillTarget;
use crate::theme::Theme;

/// 最多画几条任务行。超出的折进一条溢出行。
const MAX_TASK_ROWS: usize = 3;
/// 左侧状态符列宽（含右侧空格）。
const MARK_W: u16 = 2;
/// 描述与耗时之间的最小间隔。
const GAP: u16 = 2;
/// 低于这个宽度就不画尾行摘要了，先保证描述和耗时完整。
const TAIL_MIN_W: u16 = 12;
/// Trailing `[✗]` width (same as `/tasks`).
const KILL_W: u16 = 3;

/// 点到某一行之后要打开什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskDockHit {
    Subagent(String),
    Job(String),
    /// Workflow / 定时任务没有单独的全屏视图（`/tasks` 的 Enter 对它们也是空操作），
    /// 点击退回整张 `/tasks` 表，不做死点击。
    OpenTasks,
    /// Trailing `[✗]` — same kill path as `/tasks` / `kill_task`.
    Kill(KillTarget),
}

/// 一条已经备好、可以直接画的任务行。
#[derive(Debug, Clone)]
pub struct Row {
    pub hit: TaskDockHit,
    /// 描述列。直接来自 `TaskEntry::styled`。
    pub styled: Line<'static>,
    pub running: bool,
    pub elapsed: Duration,
    /// 最新一行输出；没有就空串。
    pub tail: String,
    /// Trailing kill control (same targets as `/tasks`).
    pub kill: Option<KillTarget>,
}

pub fn desired_height(n: usize) -> u16 {
    if n == 0 {
        return 0;
    }
    let shown = n.min(MAX_TASK_ROWS) as u16;
    if n > MAX_TASK_ROWS {
        shown.saturating_add(1)
    } else {
        shown
    }
}

pub fn hit(hits: &[(Rect, TaskDockHit)], column: u16, row: u16) -> Option<TaskDockHit> {
    let pos = Position { x: column, y: row };
    hits.iter()
        .find(|(r, _)| r.contains(pos))
        .map(|(_, h)| h.clone())
}

/// `0:12` / `4:21` / `1:03:07`。
pub fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// 最后一行非空文本。子代理输出是整份字符串，这里只取尾行给摘要用。
pub fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

pub fn paint(buf: &mut Buffer, area: Rect, rows: &[Row]) -> Vec<(Rect, TaskDockHit)> {
    let mut hits = Vec::new();
    if area.height == 0 || area.width == 0 || rows.is_empty() {
        return hits;
    }
    let theme = Theme::current();
    let shown = rows.len().min(MAX_TASK_ROWS);
    for (i, row) in rows.iter().take(shown).enumerate() {
        let y = area.y.saturating_add(i as u16);
        if y >= area.y.saturating_add(area.height) {
            break;
        }
        let rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        clear_row(buf, rect, theme.bg_base);
        let mut body = rect;
        if row.kill.is_some() && rect.width > KILL_W + 1 {
            let kx = rect.x.saturating_add(rect.width.saturating_sub(KILL_W));
            let kill_rect = Rect {
                x: kx,
                y: rect.y,
                width: KILL_W,
                height: 1,
            };
            // Kill hit first so click-test prefers the button over the row.
            if let Some(target) = row.kill.clone() {
                hits.push((kill_rect, TaskDockHit::Kill(target)));
            }
            body.width = rect.width.saturating_sub(KILL_W + 1);
        }
        paint_row(buf, body, row, &theme);
        if row.kill.is_some() && rect.width > KILL_W + 1 {
            let kx = rect.x.saturating_add(rect.width.saturating_sub(KILL_W));
            let kill_style = Style::default().fg(theme.accent_error);
            buf.set_line(
                kx,
                rect.y,
                &Line::from(Span::styled(glyphs::ballot_x_button(), kill_style)),
                KILL_W,
            );
        }
        hits.push((body, row.hit.clone()));
    }
    let overflow = rows.len().saturating_sub(shown);
    if overflow > 0 {
        let y = area.y.saturating_add(shown as u16);
        if y < area.y.saturating_add(area.height) {
            let rect = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            };
            clear_row(buf, rect, theme.bg_base);
            let label = format!("+{overflow} · /tasks");
            let w = label.width() as u16;
            let x = rect
                .x
                .saturating_add(rect.width.saturating_sub(w).min(rect.width));
            buf.set_line(
                x,
                y,
                &Line::from(Span::styled(label, Style::default().fg(theme.gray))),
                w,
            );
            hits.push((rect, TaskDockHit::OpenTasks));
        }
    }
    hits
}

fn clear_row(buf: &mut Buffer, rect: Rect, bg: ratatui::style::Color) {
    let style = Style::default().bg(bg);
    for x in rect.x..rect.x.saturating_add(rect.width) {
        if let Some(cell) = buf.cell_mut((x, rect.y)) {
            cell.reset();
            cell.set_style(style);
        }
    }
}

fn paint_row(buf: &mut Buffer, rect: Rect, row: &Row, theme: &Theme) {
    // 状态符：running 实心、idle/排程中空心。颜色沿用 styled 里的呼吸，这里只分浓淡。
    let (mark, mark_style) = if row.running {
        ("●", Style::default().fg(theme.accent_running))
    } else {
        ("○", Style::default().fg(theme.gray))
    };
    buf.set_line(
        rect.x,
        rect.y,
        &Line::from(Span::styled(mark, mark_style)),
        MARK_W,
    );

    let elapsed = format_elapsed(row.elapsed);
    let elapsed_w = elapsed.width() as u16;
    let body_x = rect.x.saturating_add(MARK_W);
    let body_w = rect.width.saturating_sub(MARK_W);
    if body_w == 0 {
        return;
    }

    // 耗时永远右对齐贴边；描述与尾行分剩下的宽度。
    let rest = body_w.saturating_sub(elapsed_w).saturating_sub(GAP);
    let (desc_w, tail_w) = split_body(rest, &row.tail);

    let desc = truncate_line(row.styled.clone(), desc_w as usize);
    buf.set_line(body_x, rect.y, &desc, desc_w);

    // 画不画由 `split_body` 一处决定（0 宽就是不画），这里别再拿 `TAIL_MIN_W`
    // 判一次 —— 那会让短摘要即使放得下也被挡在门外。
    if tail_w > 0 {
        let tail = truncate_str(&row.tail, tail_w as usize);
        let x = body_x.saturating_add(desc_w).saturating_add(1);
        buf.set_line(
            x,
            rect.y,
            &Line::from(Span::styled(
                tail,
                Style::default().fg(theme.gray).add_modifier(Modifier::DIM),
            )),
            tail_w,
        );
    }

    let x = rect
        .x
        .saturating_add(rect.width.saturating_sub(elapsed_w).min(rect.width));
    buf.set_line(
        x,
        rect.y,
        &Line::from(Span::styled(elapsed, Style::default().fg(theme.gray))),
        elapsed_w,
    );
}

/// 描述优先，尾行摘要拿剩下的（最多一半），宽度不够就不画尾行。
fn split_body(rest: u16, tail: &str) -> (u16, u16) {
    if tail.is_empty() {
        return (rest, 0);
    }
    let want = (tail.width() as u16).saturating_add(1);
    let cap = rest / 2;
    let tail_w = want.min(cap);
    // 比的是**可用的那一半够不够**，不是摘要本身够不够长：拿 `want` 去比
    // `TAIL_MIN_W` 会把 `idle` / `74%` 这种 11 列以下的短摘要永久吞掉 —— 192 列的
    // 终端上放得下也不画。短尾只在真的挤不下（`cap` 太窄）时才让位给描述。
    if tail_w < want.min(TAIL_MIN_W) {
        (rest, 0)
    } else {
        (rest.saturating_sub(tail_w).saturating_sub(1), tail_w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn row(hit: TaskDockHit, desc: &str, running: bool, secs: u64, tail: &str) -> Row {
        Row {
            hit,
            styled: Line::from(Span::styled(
                desc.to_string(),
                Style::default().fg(Color::White),
            )),
            running,
            elapsed: Duration::from_secs(secs),
            tail: tail.to_string(),
            kill: None,
        }
    }

    fn text_at(buf: &Buffer, y: u16, width: u16) -> String {
        (0..width)
            .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect()
    }

    #[test]
    fn empty_dock_takes_no_rows() {
        assert_eq!(desired_height(0), 0);
    }

    #[test]
    fn height_caps_at_three_plus_overflow_row() {
        assert_eq!(desired_height(1), 1);
        assert_eq!(desired_height(3), 3);
        // 第 4 行是 `+N · /tasks`，不是第 4 条任务。
        assert_eq!(desired_height(4), 4);
        assert_eq!(desired_height(9), 4);
    }

    #[test]
    fn overflow_row_counts_the_rest_and_opens_tasks() {
        let rows: Vec<_> = (0..5)
            .map(|i| {
                row(
                    TaskDockHit::Job(format!("job-{i}")),
                    &format!("Task cmd-{i}"),
                    true,
                    i,
                    "",
                )
            })
            .collect();
        let area = Rect::new(0, 0, 60, desired_height(rows.len()));
        let mut buf = Buffer::empty(area);
        let hits = paint(&mut buf, area, &rows);
        assert_eq!(hits.len(), 4, "3 条任务 + 1 条溢出");
        let last = text_at(&buf, 3, 60);
        assert!(last.contains("+2"), "溢出行应报剩余条数：{last}");
        assert!(last.contains("/tasks"), "溢出行应指向 /tasks：{last}");
        assert_eq!(hits[3].1, TaskDockHit::OpenTasks);
    }

    #[test]
    fn click_distinguishes_job_from_subagent() {
        let rows = vec![
            row(
                TaskDockHit::Job("job-1".into()),
                "Task cargo test",
                true,
                12,
                "",
            ),
            row(
                TaskDockHit::Subagent("sa-9".into()),
                "岑 调查网关",
                false,
                63,
                "",
            ),
        ];
        let area = Rect::new(0, 5, 60, 2);
        let mut buf = Buffer::empty(area);
        let hits = paint(&mut buf, area, &rows);
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hit(&hits, 0, 5),
            Some(TaskDockHit::Job("job-1".into())),
            "第一行是后台任务"
        );
        assert_eq!(
            hit(&hits, 0, 6),
            Some(TaskDockHit::Subagent("sa-9".into())),
            "第二行是子代理"
        );
        assert_eq!(hit(&hits, 0, 7), None, "行外不命中");
    }

    #[test]
    fn row_shows_elapsed_right_aligned_and_tail_summary() {
        let rows = vec![row(
            TaskDockHit::Job("job-1".into()),
            "Task cargo test -p cordis-spine",
            true,
            12,
            "test round ... ok",
        )];
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        paint(&mut buf, area, &rows);
        let line = text_at(&buf, 0, 80);
        assert!(line.starts_with('●'), "running 用实心状态符：{line}");
        assert!(line.contains("cargo test"), "描述要在：{line}");
        assert!(line.contains("test round"), "尾行摘要要在：{line}");
        assert!(line.trim_end().ends_with("0:12"), "耗时右对齐：{line:?}");
    }

    #[test]
    fn idle_row_uses_the_hollow_mark() {
        let rows = vec![row(
            TaskDockHit::Subagent("sa-1".into()),
            "岑 idle 调查网关",
            false,
            63,
            "",
        )];
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        paint(&mut buf, area, &rows);
        let line = text_at(&buf, 0, 60);
        assert!(line.starts_with('○'), "非 running 用空心：{line}");
        assert!(line.trim_end().ends_with("1:03"), "分秒进位：{line:?}");
    }

    #[test]
    fn narrow_width_drops_the_tail_before_the_elapsed() {
        let rows = vec![row(
            TaskDockHit::Job("job-1".into()),
            "Task cargo test",
            true,
            12,
            "test round ... ok",
        )];
        let area = Rect::new(0, 0, 26, 1);
        let mut buf = Buffer::empty(area);
        paint(&mut buf, area, &rows);
        let line = text_at(&buf, 0, 26);
        assert!(
            line.trim_end().ends_with("0:12"),
            "窄屏也要保住耗时：{line:?}"
        );
        assert!(!line.contains("test round"), "宽度不够就别画尾行：{line}");
    }

    /// 短摘要（`idle`、`74%`）不是「宽度不够才砍」的对象：宽屏上放得下就得画。
    #[test]
    fn short_tail_is_kept_on_a_wide_row() {
        let rows = vec![row(
            TaskDockHit::Subagent("sa-1".into()),
            "岑 调查网关超时",
            false,
            63,
            "idle",
        )];
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        paint(&mut buf, area, &rows);
        let line = text_at(&buf, 0, 80);
        assert!(line.contains("idle"), "短尾行宽屏上要画：{line}");
        assert!(line.trim_end().ends_with("1:03"), "耗时仍右对齐：{line:?}");
    }

    #[test]
    fn elapsed_formats_minutes_and_hours() {
        assert_eq!(format_elapsed(Duration::from_secs(0)), "0:00");
        assert_eq!(format_elapsed(Duration::from_secs(12)), "0:12");
        assert_eq!(format_elapsed(Duration::from_secs(63)), "1:03");
        assert_eq!(format_elapsed(Duration::from_secs(3787)), "1:03:07");
    }

    #[test]
    fn kill_button_registers_kill_hit() {
        let rows = vec![Row {
            hit: TaskDockHit::Job("job-1".into()),
            styled: Line::from(Span::styled(
                "Task cargo test".to_string(),
                Style::default().fg(Color::White),
            )),
            running: true,
            elapsed: Duration::from_secs(12),
            tail: String::new(),
            kill: Some(KillTarget::Job("job-1".into())),
        }];
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        let hits = paint(&mut buf, area, &rows);
        assert!(
            hits.iter().any(|(_, h)| {
                matches!(h, TaskDockHit::Kill(KillTarget::Job(id)) if id == "job-1")
            }),
            "kill hit missing: {hits:?}"
        );
        let line: String = (0..60)
            .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()))
            .collect();
        assert!(
            line.contains('\u{2717}') || line.contains('✗'),
            "kill glyph missing: {line}"
        );
        let kill_rect = hits
            .iter()
            .find(|(_, h)| matches!(h, TaskDockHit::Kill(_)))
            .map(|(r, _)| *r)
            .expect("kill rect");
        assert_eq!(
            hit(&hits, kill_rect.x, kill_rect.y),
            Some(TaskDockHit::Kill(KillTarget::Job("job-1".into()))),
        );
    }

    #[test]
    fn last_line_skips_trailing_blanks() {
        assert_eq!(last_line("a\nb\n\n  \n"), "b");
        assert_eq!(last_line("only"), "only");
        assert_eq!(last_line("  \n\n"), "");
        assert_eq!(last_line(""), "");
    }
}
