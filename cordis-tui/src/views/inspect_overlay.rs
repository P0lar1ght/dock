//! Grok-style fullscreen child transcript.
//!
//! Parent view is replaced by a bordered frame (title + child's own
//! scrollback). Tool cards use the same fold cycle as the main timeline.
//! Dock mailbox children also get a composer so the user can `send_message`.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_str;
use crate::grok::picker::{render_bordered_frame, render_close_button, PickerHits};
use crate::scrollback::tool::ToolMode;
use crate::scrollback::{self, live};
use crate::theme::Theme;
use crate::views::overlay::InspectTarget;
use cordis::Context;
use cordis_spine::{AgentPresets, Jobs, Subagents, AGENT_PRESETS, JOBS, SUBAGENTS};

const COMPOSER_H: u16 = 3;

struct FoldState {
    key: String,
    expanded: HashSet<String>,
    tool_fold: HashMap<String, ToolMode>,
}

struct Hits {
    close: Rect,
    body: Rect,
    tool_headers: Vec<(usize, String)>,
    scroll: usize,
    line_count: usize,
    composer: Option<Rect>,
}

static FOLDS: Mutex<Option<FoldState>> = Mutex::new(None);
static HITS: Mutex<Hits> = Mutex::new(Hits {
    close: Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    },
    body: Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    },
    tool_headers: Vec::new(),
    scroll: 0,
    line_count: 0,
    composer: None,
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectClick {
    Close,
    Toggle,
    None,
}

pub fn render(
    ctx: &Context,
    buf: &mut Buffer,
    area: Rect,
    target: &InspectTarget,
    scroll: usize,
    composer: &str,
    composer_cursor: usize,
) -> PickerHits {
    let theme = Theme::current();
    let Some(frame) = render_bordered_frame(buf, area, theme.selection_border, theme.bg_base)
    else {
        return PickerHits::default();
    };
    let close = render_close_button(
        buf,
        frame.title_row.x,
        frame.title_row.y,
        frame.title_row.width,
        &theme,
        false,
        Some(theme.bg_base),
    );
    match target {
        InspectTarget::Subagent(id) => paint_subagent(
            ctx,
            buf,
            frame.title_row,
            frame.content,
            close,
            id,
            scroll,
            composer,
            composer_cursor,
            &theme,
        ),
        InspectTarget::Job(id) => paint_job(
            ctx,
            buf,
            frame.title_row,
            frame.content,
            close,
            id,
            scroll,
            &theme,
        ),
    }
}

pub fn click(column: u16, row: u16) -> InspectClick {
    let pos = Position { x: column, y: row };
    let hits = HITS.lock().unwrap();
    if hits.close.width > 0 && hits.close.contains(pos) {
        return InspectClick::Close;
    }
    if hits.body.width == 0 || !hits.body.contains(pos) {
        return InspectClick::None;
    }
    let extra = hits.line_count.saturating_sub(hits.body.height as usize);
    let top = extra.saturating_sub(hits.scroll);
    let line_idx = (row.saturating_sub(hits.body.y) as usize).saturating_add(top);
    let Some((_, id)) = hits
        .tool_headers
        .iter()
        .find(|(i, _)| *i == line_idx)
        .cloned()
    else {
        return InspectClick::None;
    };
    drop(hits);
    toggle_id(&id);
    InspectClick::Toggle
}

pub fn body_width() -> usize {
    HITS.lock().unwrap().body.width.max(40) as usize
}

pub fn body_height() -> usize {
    HITS.lock().unwrap().body.height.max(1) as usize
}

pub fn line_count(ctx: &Context, target: &InspectTarget, width: usize) -> usize {
    match target {
        InspectTarget::Subagent(id) => subagent_lines(ctx, id, width).0.len(),
        InspectTarget::Job(id) => job_lines(ctx, id).0.len(),
    }
}

pub fn max_scroll(ctx: &Context, target: &InspectTarget, width: usize, viewport: usize) -> usize {
    line_count(ctx, target, width).saturating_sub(viewport.max(1))
}

pub fn composer_cursor(composer: &str, cursor: usize) -> Option<Position> {
    let hits = HITS.lock().unwrap();
    let area = hits.composer?;
    if area.height < 2 || area.width < 4 {
        return None;
    }
    let inner_x = area.x.saturating_add(2);
    let y = area.y.saturating_add(1);
    let prefix = 2u16; // "> "
    let shown: String = composer.chars().take(cursor).collect();
    let col = UnicodeWidthStr::width(shown.as_str()) as u16;
    Some(Position {
        x: inner_x
            .saturating_add(prefix)
            .saturating_add(col)
            .min(area.x.saturating_add(area.width.saturating_sub(2))),
        y,
    })
}

#[allow(clippy::too_many_arguments)]
// TUI 绘制/布局函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到所有调用点，故意保留。
fn paint_subagent(
    ctx: &Context,
    buf: &mut Buffer,
    title_row: Rect,
    content: Rect,
    close: Rect,
    id: &str,
    scroll: usize,
    composer: &str,
    _composer_cursor: usize,
    theme: &Theme,
) -> PickerHits {
    let sub = ctx.get::<Subagents>(SUBAGENTS);
    let snap = sub.as_ref().and_then(|s| s.snapshot(id));
    let running = snap.as_ref().is_some_and(|s| s.running());
    let idle = snap.as_ref().is_some_and(|s| s.idle);
    let cancelled = snap.as_ref().is_some_and(|s| s.cancelled);
    let done = snap.as_ref().is_some_and(|s| s.done);
    let can_send = snap.as_ref().is_some_and(|s| !s.done && !s.cancelled);
    paint_title(
        buf,
        title_row,
        close,
        theme,
        running,
        cancelled,
        done,
        idle,
        snap.as_ref().map(|s| s.started_at),
        &subagent_title(ctx, id, &snap),
        sub.as_ref()
            .map(|s| s.events(id))
            .as_deref()
            .and_then(scrollback::subagent_live_activity),
    );
    let composer_h = if can_send {
        COMPOSER_H.min(content.height / 2)
    } else {
        0
    };
    let chunks =
        Layout::vertical([Constraint::Min(1), Constraint::Length(composer_h)]).split(content);
    let body = chunks[0];
    let width = body.width.max(1) as usize;
    ensure_folds(target_key_sub(id));
    let (expanded, tool_fold) = {
        let g = FOLDS.lock().unwrap();
        let st = g.as_ref().unwrap();
        (st.expanded.clone(), st.tool_fold.clone())
    };
    let events = sub.as_ref().map(|s| s.events(id)).unwrap_or_default();
    let presets = ctx.get::<AgentPresets>(AGENT_PRESETS);
    let view = scrollback::child_transcript(
        &events,
        width,
        &expanded,
        &tool_fold,
        running,
        true,
        presets.as_deref(),
        sub.as_deref(),
    );
    let mut lines = view.lines;
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "尚无输出。子代理启动后工具与回复会出现在这里。".to_string(),
            Style::default().fg(theme.gray_bright),
        )));
    }
    let extra = lines.len().saturating_sub(body.height as usize);
    let top = extra.saturating_sub(scroll);
    Paragraph::new(lines.clone())
        .scroll((top as u16, 0))
        .render(body, buf);
    let mut composer_area = None;
    if composer_h >= 3 && chunks[1].height >= 3 {
        paint_composer(buf, chunks[1], composer, can_send, theme);
        composer_area = Some(chunks[1]);
    }
    *HITS.lock().unwrap() = Hits {
        close,
        body,
        tool_headers: view.tool_headers,
        scroll,
        line_count: lines.len(),
        composer: composer_area,
    };
    PickerHits {
        close_button: close,
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
// TUI 绘制/布局函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到所有调用点，故意保留。
fn paint_job(
    ctx: &Context,
    buf: &mut Buffer,
    title_row: Rect,
    content: Rect,
    close: Rect,
    id: &str,
    scroll: usize,
    theme: &Theme,
) -> PickerHits {
    let (title, lines) = job_lines(ctx, id);
    let job = ctx.get::<Jobs>(JOBS).and_then(|j| j.snapshot(id));
    let running = job.as_ref().is_some_and(|j| !j.done);
    let done = job.as_ref().is_some_and(|j| j.done);
    paint_title(
        buf, title_row, close, theme, running, false, done, false, None, &title, None,
    );
    let extra = lines.len().saturating_sub(content.height as usize);
    let top = extra.saturating_sub(scroll);
    Paragraph::new(lines.clone())
        .scroll((top as u16, 0))
        .render(content, buf);
    *HITS.lock().unwrap() = Hits {
        close,
        body: content,
        tool_headers: Vec::new(),
        scroll,
        line_count: lines.len(),
        composer: None,
    };
    PickerHits {
        close_button: close,
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
// TUI 绘制/布局函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到所有调用点，故意保留。
fn paint_title(
    buf: &mut Buffer,
    row: Rect,
    close: Rect,
    theme: &Theme,
    running: bool,
    cancelled: bool,
    done: bool,
    idle: bool,
    started: Option<std::time::Instant>,
    label: &str,
    activity: Option<String>,
) {
    let icon = if running {
        glyphs::diamond_filled()
    } else if cancelled {
        glyphs::ballot_x()
    } else if done {
        glyphs::check_mark()
    } else {
        glyphs::hollow_dot()
    };
    let icon_color = if running {
        crate::grok::color::pulse_fg(theme.bg_base, theme.accent_running)
    } else if cancelled {
        theme.accent_error
    } else if done {
        theme.accent_success
    } else if idle {
        theme.gray_bright
    } else {
        theme.accent_running
    };
    let mut right = String::new();
    if running {
        if let Some(act) = activity.as_deref().filter(|s| !s.is_empty()) {
            right.push_str(act);
            right.push_str(" · ");
        }
        if let Some(at) = started {
            right.push_str(live::elapsed_instant(at).trim_start_matches(" · "));
            right.push(' ');
        }
    } else if idle {
        right.push_str("空闲 ");
    }
    let right_w = UnicodeWidthStr::width(right.as_str()) as u16 + close.width + 1;
    let left_max = row.width.saturating_sub(right_w).saturating_sub(2) as usize;
    let left = truncate_str(&format!(" {icon} {label}"), left_max);
    buf.set_span(
        row.x,
        row.y,
        &Span::styled(
            left,
            Style::default()
                .fg(icon_color)
                .bg(theme.bg_base)
                .add_modifier(Modifier::BOLD),
        ),
        row.width.saturating_sub(right_w),
    );
    if !right.is_empty() {
        let x = close
            .x
            .saturating_sub(UnicodeWidthStr::width(right.as_str()) as u16);
        buf.set_span(
            x,
            row.y,
            &Span::styled(right, Style::default().fg(theme.gray).bg(theme.bg_base)),
            close.x.saturating_sub(x),
        );
    }
}

fn paint_composer(buf: &mut Buffer, area: Rect, text: &str, enabled: bool, theme: &Theme) {
    let border = Style::default()
        .fg(theme.prompt_border_active)
        .bg(theme.bg_base);
    let fg = if enabled {
        theme.text_primary
    } else {
        theme.gray
    };
    for x in area.x..area.x + area.width {
        let top = if x == area.x {
            '\u{256d}'
        } else if x + 1 == area.x + area.width {
            '\u{256e}'
        } else {
            '\u{2500}'
        };
        let bot = if x == area.x {
            '\u{2570}'
        } else if x + 1 == area.x + area.width {
            '\u{256f}'
        } else {
            '\u{2500}'
        };
        if let Some(c) = buf.cell_mut((x, area.y)) {
            c.set_char(top);
            c.set_style(border);
        }
        if let Some(c) = buf.cell_mut((x, area.y + area.height.saturating_sub(1))) {
            c.set_char(bot);
            c.set_style(border);
        }
    }
    for y in area.y + 1..area.y + area.height.saturating_sub(1) {
        if let Some(c) = buf.cell_mut((area.x, y)) {
            c.set_char('\u{2502}');
            c.set_style(border);
        }
        if let Some(c) = buf.cell_mut((area.x + area.width.saturating_sub(1), y)) {
            c.set_char('\u{2502}');
            c.set_style(border);
        }
    }
    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: 1,
    };
    let body = if text.is_empty() {
        Span::styled(
            "> 发给子代理以调整…".to_string(),
            Style::default().fg(theme.gray).bg(theme.bg_base),
        )
    } else {
        Span::styled(
            format!("> {text}"),
            Style::default().fg(fg).bg(theme.bg_base),
        )
    };
    buf.set_span(inner.x, inner.y, &body, inner.width);
}

fn subagent_title(ctx: &Context, _id: &str, snap: &Option<cordis_spine::SubagentSnap>) -> String {
    let typ = snap
        .as_ref()
        .map(|s| s.subagent_type.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("subagent");
    let role = ctx
        .get::<AgentPresets>(AGENT_PRESETS)
        .and_then(|p| p.role_label(typ));
    let desc = snap
        .as_ref()
        .map(|s| s.description.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    match (role.as_deref().filter(|r| *r != typ), desc) {
        (Some(role), d) if !d.is_empty() => format!("{role}  {d}"),
        (Some(role), _) => role.to_string(),
        (None, d) if !d.is_empty() => format!("{typ}  {d}"),
        _ => typ.to_string(),
    }
}

fn subagent_lines(
    ctx: &Context,
    id: &str,
    width: usize,
) -> (Vec<Line<'static>>, Vec<(usize, String)>) {
    ensure_folds(target_key_sub(id));
    let (expanded, tool_fold) = {
        let g = FOLDS.lock().unwrap();
        let st = g.as_ref().unwrap();
        (st.expanded.clone(), st.tool_fold.clone())
    };
    let sub = ctx.get::<Subagents>(SUBAGENTS);
    let events = sub.as_ref().map(|s| s.events(id)).unwrap_or_default();
    let running = sub
        .as_ref()
        .and_then(|s| s.snapshot(id))
        .is_some_and(|s| s.running());
    let presets = ctx.get::<AgentPresets>(AGENT_PRESETS);
    let view = scrollback::child_transcript(
        &events,
        width,
        &expanded,
        &tool_fold,
        running,
        true,
        presets.as_deref(),
        sub.as_deref(),
    );
    (view.lines, view.tool_headers)
}

fn job_lines(ctx: &Context, id: &str) -> (String, Vec<Line<'static>>) {
    let theme = Theme::current();
    let job = ctx.get::<Jobs>(JOBS).and_then(|j| j.snapshot(id));
    let Some(job) = job else {
        return (
            format!("任务 {id}"),
            vec![Line::from(Span::styled(
                "未找到该后台任务。".to_string(),
                Style::default().fg(theme.gray_bright),
            ))],
        );
    };
    let status = if job.done {
        "完成".to_string()
    } else {
        format!("运行中{}", live::elapsed_system(job.start_time))
    };
    let title = job
        .description
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|d| format!("{d} · {status}"))
        .unwrap_or_else(|| format!("{} · {status}", job.id));
    let mut lines = vec![Line::from(Span::styled(
        job.command.clone(),
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    ))];
    if job.output.trim().is_empty() {
        lines.push(Line::from(if job.done {
            Span::styled(
                "（无输出）".to_string(),
                Style::default().fg(theme.gray_bright),
            )
        } else {
            live::running_body(&theme)
        }));
    } else {
        for row in job.output.lines() {
            lines.push(Line::from(Span::styled(
                row.to_string(),
                Style::default().fg(theme.text_primary),
            )));
        }
    }
    (title, lines)
}

fn target_key_sub(id: &str) -> String {
    format!("sa:{id}")
}

fn ensure_folds(key: String) {
    let mut g = FOLDS.lock().unwrap();
    if g.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    *g = Some(FoldState {
        key,
        expanded: HashSet::new(),
        tool_fold: HashMap::new(),
    });
}

fn toggle_id(id: &str) {
    let mut g = FOLDS.lock().unwrap();
    let Some(st) = g.as_mut() else {
        return;
    };
    if id.starts_with("think:") {
        if !st.expanded.remove(id) {
            st.expanded.insert(id.to_string());
        }
        return;
    }
    let current = st.tool_fold.get(id).copied().unwrap_or(ToolMode::Collapsed);
    match current.next() {
        Some(next) => {
            st.tool_fold.insert(id.to_string(), next);
        }
        None => {
            st.tool_fold.remove(id);
        }
    }
}

pub fn insert_composer(composer: &mut String, cursor: &mut usize, ch: char) {
    let mut chars: Vec<char> = composer.chars().collect();
    let i = (*cursor).min(chars.len());
    chars.insert(i, ch);
    *cursor = i + 1;
    *composer = chars.into_iter().collect();
}

pub fn composer_backspace(composer: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let mut chars: Vec<char> = composer.chars().collect();
    let i = cursor.saturating_sub(1).min(chars.len().saturating_sub(1));
    if i < chars.len() {
        chars.remove(i);
        *cursor = i;
        *composer = chars.into_iter().collect();
    }
}

pub fn composer_insert_str(composer: &mut String, cursor: &mut usize, text: &str) {
    for ch in text.chars() {
        if ch == '\n' {
            continue;
        }
        insert_composer(composer, cursor, ch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_inserts_at_cursor() {
        let mut s = String::from("ac");
        let mut c = 1usize;
        insert_composer(&mut s, &mut c, 'b');
        assert_eq!(s, "abc");
        assert_eq!(c, 2);
    }

    #[test]
    fn fold_cycle_matches_main_scrollback() {
        ensure_folds("sa:test".into());
        toggle_id("tool-1");
        {
            let g = FOLDS.lock().unwrap();
            assert_eq!(
                g.as_ref().unwrap().tool_fold.get("tool-1"),
                Some(&ToolMode::Truncated)
            );
        }
        toggle_id("tool-1");
        {
            let g = FOLDS.lock().unwrap();
            assert_eq!(
                g.as_ref().unwrap().tool_fold.get("tool-1"),
                Some(&ToolMode::Expanded)
            );
        }
        toggle_id("tool-1");
        {
            let g = FOLDS.lock().unwrap();
            assert!(!g.as_ref().unwrap().tool_fold.contains_key("tool-1"));
        }
    }
}
