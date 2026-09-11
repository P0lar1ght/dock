//! Todo card — the live `"todos"` list pinned at the tail of the transcript.
//!
//! Grok keeps todos in a separate `ListPane` toggled with Ctrl-T; dock renders
//! them as the last scrollback card so they scroll with the conversation. The
//! card folds so a long list cannot eat the viewport: unfinished items by
//! default, the whole list or a single progress row on demand.
//!
//! Fold cycle (click the header or Ctrl+t): Active → Full → Summary → Active.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::{truncate_line, truncate_str};
use crate::grok::todo_pane::{todo_icon, TodoPaneStyle};
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;
use cordis_spine::{TodoItem, TodoStatus};

/// Header id in `Frame::tool_headers` — click routing keys on this.
pub const HEADER_ID: &str = "todo:panel";

/// Progress-bar cells. Fixed width so the header does not jitter as items land.
const BAR_CELLS: usize = 8;
/// Open items shown in [`TodoFold::Active`] before the `… 还有 N 项` row.
const MAX_ACTIVE_ROWS: usize = 6;
/// Left margin for item rows, matching the tool-card body indent.
const ROW_INDENT: &str = "  ";

/// How much of the list the card shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TodoFold {
    /// Header only — one line.
    Summary,
    /// Header + unfinished items (default). Completed items collapse into a
    /// count so a long finished list does not push the composer around.
    #[default]
    Active,
    /// Header + every item, completed ones included.
    Full,
}

impl TodoFold {
    /// Click / Ctrl+t: Active → Full → Summary → Active.
    pub fn next(self) -> Self {
        match self {
            Self::Active => Self::Full,
            Self::Full => Self::Summary,
            Self::Summary => Self::Active,
        }
    }
}

fn is_open(status: TodoStatus) -> bool {
    matches!(status, TodoStatus::Pending | TodoStatus::InProgress)
}

/// Rendered card. Empty when there is no list at all.
pub fn lines(
    todos: &[(String, TodoItem)],
    fold: TodoFold,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    if todos.is_empty() {
        return Vec::new();
    }
    if fold == TodoFold::Summary {
        return vec![header(todos, fold, true, theme, width)];
    }

    let style = TodoPaneStyle::default();
    let settled = todos.iter().filter(|(_, t)| !is_open(t.status)).count();
    let shown: Vec<&(String, TodoItem)> = if fold == TodoFold::Full {
        todos.iter().collect()
    } else {
        todos.iter().filter(|(_, t)| is_open(t.status)).collect()
    };
    let cap = if fold == TodoFold::Full {
        shown.len()
    } else {
        MAX_ACTIVE_ROWS
    };
    // Repeating the in-progress title in the header is noise when its own row
    // is right below it — worth it only when the cap hides that row.
    let current_hidden = !shown
        .iter()
        .take(cap)
        .any(|(_, t)| t.status == TodoStatus::InProgress);
    let mut out = vec![header(todos, fold, current_hidden, theme, width)];
    for (_, item) in shown.iter().take(cap) {
        out.push(item_row(item, &style, width));
    }
    if let Some(rest) = shown.len().checked_sub(cap).filter(|n| *n > 0) {
        out.push(note_row(format!("… 还有 {rest} 项"), theme));
    }
    if fold == TodoFold::Active && settled > 0 {
        out.push(note_row(
            format!("{} 已完成 {settled} 项", glyphs::check_mark()),
            theme,
        ));
    }
    out
}

/// `◆ 待办  ◆◆◇◇◇◇◇◇ 2/7  ▶ 当前项  （点击展开）`. `show_current` adds the
/// in-progress title — on when no visible row carries it.
fn header(
    todos: &[(String, TodoItem)],
    fold: TodoFold,
    show_current: bool,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let total = todos.len();
    let settled = todos.iter().filter(|(_, t)| !is_open(t.status)).count();
    let mut spans = vec![
        Span::styled(
            format!("{} ", glyphs::diamond_filled()),
            Style::default().fg(theme.accent_plan),
        ),
        Span::styled(
            "待办".to_string(),
            Style::default()
                .fg(theme.accent_plan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ];
    spans.extend(bar_spans(settled, total, theme));
    spans.push(Span::styled(format!(" {settled}/{total}"), theme.muted()));
    let current = show_current
        .then(|| {
            todos
                .iter()
                .find(|(_, t)| t.status == TodoStatus::InProgress)
        })
        .flatten();
    if let Some(current) = current {
        spans.push(Span::styled(
            format!("  {} ", todo_icon(TodoStatus::InProgress)),
            Style::default().fg(theme.warning),
        ));
        spans.push(Span::styled(
            truncate_str(current.1.content.trim(), current_budget(width)),
            theme.primary(),
        ));
    } else if settled == total {
        spans.push(Span::styled("  全部完成".to_string(), theme.dim()));
    }
    spans.push(Span::styled(fold_hint(fold).to_string(), theme.dim()));
    let line = Line::from(spans);
    if width == 0 {
        line
    } else {
        truncate_line(line, width)
    }
}

/// Columns the in-progress title may take in the header. Leaves room for the
/// label, bar, counter and fold hint on an 80-column pane.
fn current_budget(width: usize) -> usize {
    width.saturating_sub(40).clamp(8, 60)
}

const fn fold_hint(fold: TodoFold) -> &'static str {
    match fold {
        TodoFold::Summary => "  （点击展开）",
        TodoFold::Active => "  （点击看全部）",
        TodoFold::Full => "  （点击收起）",
    }
}

/// Proportional `◆◆◇◇◇◇◇◇` bar. Never full until every item is settled, so a
/// rounding artefact cannot claim the work is done.
fn bar_spans(settled: usize, total: usize, theme: &Theme) -> Vec<Span<'static>> {
    let filled = if total == 0 {
        0
    } else if settled == total {
        BAR_CELLS
    } else {
        ((settled * BAR_CELLS) / total).min(BAR_CELLS.saturating_sub(1))
    };
    let mut spans = Vec::with_capacity(2);
    if filled > 0 {
        spans.push(Span::styled(
            glyphs::diamond_filled().repeat(filled),
            Style::default().fg(theme.accent_success),
        ));
    }
    if filled < BAR_CELLS {
        spans.push(Span::styled(
            glyphs::diamond_hollow().repeat(BAR_CELLS - filled),
            Style::default().fg(theme.gray_dim),
        ));
    }
    spans
}

/// `  ▶ 内容` — the id stays model-side, the row shows only what it says.
fn item_row(item: &TodoItem, style: &TodoPaneStyle, width: usize) -> Line<'static> {
    let st = style.for_status(item.status);
    let line = Line::from(vec![
        Span::raw(ROW_INDENT),
        Span::styled(
            todo_icon(item.status).to_string(),
            Style::default().fg(st.icon_fg),
        ),
        Span::styled(format!(" {}", item.content.trim()), st.text_style),
    ]);
    if width == 0 {
        line
    } else {
        truncate_line(line, width)
    }
}

fn note_row(text: String, theme: &Theme) -> Line<'static> {
    Line::from(vec![Span::raw(ROW_INDENT), Span::styled(text, theme.dim())])
}

// ---------------------------------------------------------------------------
// `todo_write` tool card
// ---------------------------------------------------------------------------

/// Rows of a `todo_write` call shown in [`ToolMode::Truncated`].
const MAX_CALL_ROWS: usize = 6;

pub fn is_todo_tool(name: &str) -> bool {
    name == "todo_write"
}

/// Card for one `todo_write` call: what this call changed, not the whole list
/// (the live card at the tail already carries that). Collapsed is one row; the
/// generic card would print the raw arguments JSON instead.
pub fn card_lines(
    arguments: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let items = parse_call(arguments);
    let mut header = call_header(&items, theme, width);
    if running && mode == ToolMode::Collapsed {
        live::mark_running(&mut header, theme);
        return vec![header];
    }
    if mode == ToolMode::Collapsed || items.is_empty() {
        return vec![header];
    }
    let style = TodoPaneStyle::default();
    let cap = if mode == ToolMode::Truncated {
        MAX_CALL_ROWS
    } else {
        items.len()
    };
    let mut out = vec![header];
    for item in items.iter().take(cap) {
        out.push(item_row(item, &style, width));
    }
    if let Some(rest) = items.len().checked_sub(cap).filter(|n| *n > 0) {
        out.push(note_row(format!("… 还有 {rest} 项"), theme));
    }
    out
}

/// `◆ 待办  写入 4 项  ✓2 ▶1 □1`
fn call_header(items: &[TodoItem], theme: &Theme, width: usize) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            format!("{} ", glyphs::diamond_filled()),
            Style::default().fg(theme.accent_plan),
        ),
        Span::styled(
            "待办".to_string(),
            Style::default()
                .fg(theme.accent_plan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  写入 {} 项", items.len()), theme.muted()),
    ];
    let style = TodoPaneStyle::default();
    for status in [
        TodoStatus::Completed,
        TodoStatus::InProgress,
        TodoStatus::Pending,
        TodoStatus::Cancelled,
    ] {
        let n = items.iter().filter(|t| t.status == status).count();
        if n == 0 {
            continue;
        }
        spans.push(Span::styled(
            format!("  {}{n}", todo_icon(status)),
            Style::default().fg(style.for_status(status).icon_fg),
        ));
    }
    let line = Line::from(spans);
    if width == 0 {
        line
    } else {
        truncate_line(line, width)
    }
}

/// Items carried by one call. Falls back to the id when `content` is omitted,
/// mirroring the tool's own merge semantics.
fn parse_call(arguments: &str) -> Vec<TodoItem> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return Vec::new();
    };
    v.get("todos")
        .and_then(|t| t.as_array())
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let id = row.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let content = row
                        .get("content")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(id);
                    TodoItem {
                        content: content.to_string(),
                        priority: Default::default(),
                        status: parse_status(row.get("status").and_then(|v| v.as_str())),
                        meta: None,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_status(raw: Option<&str>) -> TodoStatus {
    match raw {
        Some("in_progress") => TodoStatus::InProgress,
        Some("completed") => TodoStatus::Completed,
        Some("cancelled") => TodoStatus::Cancelled,
        _ => TodoStatus::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str, status: TodoStatus) -> (String, TodoItem) {
        (
            content.to_string(),
            TodoItem {
                content: content.into(),
                priority: Default::default(),
                status,
                meta: None,
            },
        )
    }

    fn plain(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn sample() -> Vec<(String, TodoItem)> {
        vec![
            item("读源码", TodoStatus::Completed),
            item("改 UI", TodoStatus::InProgress),
            item("写测试", TodoStatus::Pending),
            item("同步文档", TodoStatus::Pending),
        ]
    }

    #[test]
    fn empty_list_renders_nothing() {
        assert!(lines(&[], TodoFold::Active, &Theme::current(), 80).is_empty());
    }

    #[test]
    fn summary_is_one_line_with_progress_and_current_item() {
        let out = lines(&sample(), TodoFold::Summary, &Theme::current(), 80);
        assert_eq!(out.len(), 1, "{}", plain(&out));
        let text = plain(&out);
        assert!(text.contains("1/4"), "{text}");
        assert!(text.contains("改 UI"), "{text}");
    }

    #[test]
    fn active_hides_completed_behind_a_count() {
        let text = plain(&lines(&sample(), TodoFold::Active, &Theme::current(), 80));
        assert!(text.contains("改 UI"), "{text}");
        assert!(text.contains("写测试"), "{text}");
        assert!(
            !text.contains("\n  \u{2713} 读源码"),
            "completed rows stay folded: {text}"
        );
        assert!(text.contains("已完成 1 项"), "{text}");
    }

    #[test]
    fn full_shows_every_item() {
        let text = plain(&lines(&sample(), TodoFold::Full, &Theme::current(), 80));
        assert!(text.contains("读源码"), "{text}");
        assert!(text.contains("同步文档"), "{text}");
        assert!(!text.contains("已完成 1 项"), "{text}");
    }

    #[test]
    fn active_caps_long_lists() {
        let todos: Vec<_> = (0..12)
            .map(|i| item(&format!("步骤 {i}"), TodoStatus::Pending))
            .collect();
        let out = lines(&todos, TodoFold::Active, &Theme::current(), 80);
        // header + MAX_ACTIVE_ROWS + overflow note
        assert_eq!(out.len(), MAX_ACTIVE_ROWS + 2, "{}", plain(&out));
        assert!(plain(&out).contains("还有 6 项"), "{}", plain(&out));
    }

    #[test]
    fn bar_is_only_full_when_everything_is_settled() {
        let theme = Theme::current();
        let almost: Vec<_> = (0..8)
            .map(|i| {
                item(
                    &format!("t{i}"),
                    if i == 7 {
                        TodoStatus::InProgress
                    } else {
                        TodoStatus::Completed
                    },
                )
            })
            .collect();
        let text = plain(&[header(&almost, TodoFold::Summary, true, &theme, 80)]);
        assert!(text.contains(glyphs::diamond_hollow()), "{text}");

        let done: Vec<_> = (0..8)
            .map(|i| item(&format!("t{i}"), TodoStatus::Completed))
            .collect();
        let text = plain(&[header(&done, TodoFold::Summary, true, &theme, 80)]);
        assert!(!text.contains(glyphs::diamond_hollow()), "{text}");
        assert!(text.contains("全部完成"), "{text}");
    }

    /// The in-progress title is header-only when its row is not on screen —
    /// otherwise the header just repeats the row under it.
    #[test]
    fn header_repeats_the_current_item_only_when_its_row_is_hidden() {
        let theme = Theme::current();
        let expanded = plain(&lines(&sample(), TodoFold::Active, &theme, 80));
        assert_eq!(
            expanded.matches("改 UI").count(),
            1,
            "header must not echo a visible row: {expanded}"
        );

        // In-progress sits past the Active cap, so the header carries it.
        let mut long: Vec<_> = (0..MAX_ACTIVE_ROWS + 1)
            .map(|i| item(&format!("步骤 {i}"), TodoStatus::Pending))
            .collect();
        long.push(item("收尾", TodoStatus::InProgress));
        let text = plain(&lines(&long, TodoFold::Active, &theme, 80));
        assert!(text.lines().next().unwrap().contains("收尾"), "{text}");
    }

    const CALL: &str = r#"{"merge":true,"todos":[
        {"id":"explore","status":"completed"},
        {"id":"ui","content":"折叠 + 美化","status":"in_progress"},
        {"id":"docs","content":"同步文档","status":"pending"}]}"#;

    #[test]
    fn call_card_summarizes_instead_of_dumping_json() {
        let theme = Theme::current();
        let collapsed = plain(&card_lines(CALL, &theme, 80, ToolMode::Collapsed, false));
        assert_eq!(collapsed.lines().count(), 1, "{collapsed}");
        assert!(collapsed.contains("写入 3 项"), "{collapsed}");
        assert!(
            !collapsed.contains("merge"),
            "raw JSON must not leak: {collapsed}"
        );
        assert!(
            collapsed.contains(&format!("{}1", todo_icon(TodoStatus::Completed))),
            "{collapsed}"
        );

        let open = plain(&card_lines(CALL, &theme, 80, ToolMode::Expanded, false));
        assert!(open.contains("折叠 + 美化"), "{open}");
        // content omitted → id stands in, same as the tool's merge fallback
        assert!(open.contains("explore"), "{open}");
    }

    #[test]
    fn call_card_survives_unparsable_arguments() {
        let out = card_lines("{oops", &Theme::current(), 80, ToolMode::Expanded, false);
        assert_eq!(out.len(), 1);
        assert!(plain(&out).contains("写入 0 项"), "{}", plain(&out));
    }

    #[test]
    fn fold_cycles_three_ways() {
        assert_eq!(TodoFold::default(), TodoFold::Active);
        assert_eq!(TodoFold::Active.next(), TodoFold::Full);
        assert_eq!(TodoFold::Full.next(), TodoFold::Summary);
        assert_eq!(TodoFold::Summary.next(), TodoFold::Active);
    }

    #[test]
    fn narrow_pane_keeps_rows_within_width() {
        let out = lines(&sample(), TodoFold::Full, &Theme::current(), 24);
        for line in &out {
            let w: usize = line
                .spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert!(
                w <= 24,
                "line too wide ({w}): {}",
                plain(std::slice::from_ref(line))
            );
        }
    }
}
