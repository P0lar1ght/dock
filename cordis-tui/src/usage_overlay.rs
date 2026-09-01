//! `/context` occupancy + `/usage` ledger, tabbed overlay.
//!
//! Diamond bar chrome follows Grok `context_info.rs`. Session tab is the
//! existing ledger ([`session_usage_block_text`]). No grok.com billing.

use std::sync::Mutex;

use cordis::Context;
use cordis_spine::{
    occupancy_detail, session_usage_block_text, snapshot_context, ContextCategory, ContextSnapshot,
    OccupancyDetail, OccupancyKind, Sessions, TokenUsage, SESSIONS,
};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::grok::glyphs;
use crate::grok::picker::{render_floating_frame, PickerHits};
use crate::overlay::UsageTab;
use crate::theme::Theme;

struct Hits {
    context: Rect,
    session: Rect,
    back: Rect,
    slices: Vec<(Rect, OccupancyKind)>,
}

static HITS: Mutex<Hits> = Mutex::new(Hits {
    context: Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    },
    session: Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    },
    back: Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    },
    slices: Vec::new(),
});

static HOVER: Mutex<Option<OccupancyKind>> = Mutex::new(None);

struct BodyMemo {
    tab: UsageTab,
    detail: Option<OccupancyKind>,
    width: u16,
    hovered: Option<OccupancyKind>,
    rev: u64,
    usage: TokenUsage,
    snap: Option<ContextSnapshot>,
    lines: Vec<Line<'static>>,
    legend: Vec<(usize, usize, OccupancyKind)>,
    bar: Vec<(usize, usize, OccupancyKind)>,
    bar_row_len: usize,
    viewport: u16,
}

static BODY: Mutex<Option<BodyMemo>> = Mutex::new(None);

fn occupancy_stamp(ctx: &Context) -> (u64, TokenUsage) {
    ctx.get::<Sessions>(SESSIONS)
        .map(|s| s.occupancy_stamp())
        .unwrap_or((0, TokenUsage::default()))
}

pub fn hit_tab(column: u16, row: u16) -> Option<UsageTab> {
    let hits = HITS.lock().unwrap();
    let pos = Position { x: column, y: row };
    if hits.context.contains(pos) {
        Some(UsageTab::Context)
    } else if hits.session.contains(pos) {
        Some(UsageTab::Session)
    } else {
        None
    }
}

pub fn hit_slice(column: u16, row: u16) -> Option<OccupancyKind> {
    let hits = HITS.lock().unwrap();
    let pos = Position { x: column, y: row };
    hits.slices
        .iter()
        .rev()
        .find(|(rect, _)| rect.contains(pos))
        .map(|(_, kind)| *kind)
}

pub fn hit_back(column: u16, row: u16) -> bool {
    HITS.lock()
        .unwrap()
        .back
        .contains(Position { x: column, y: row })
}

pub fn hover(column: u16, row: u16) -> bool {
    let kind = hit_slice(column, row);
    let mut hover = HOVER.lock().unwrap();
    if *hover == kind {
        false
    } else {
        *hover = kind;
        true
    }
}

pub fn render(
    buf: &mut Buffer,
    area: Rect,
    ctx: &Context,
    tab: UsageTab,
    scroll: usize,
    detail: Option<OccupancyKind>,
) -> PickerHits {
    let theme = Theme::current();
    let Some(frame) = render_floating_frame(buf, area, &theme, false) else {
        return PickerHits::default();
    };
    let inner = frame.content;
    if inner.height < 2 || inner.width < 10 {
        return PickerHits {
            close_button: frame.close_button,
            ..Default::default()
        };
    }
    paint_tabs(buf, inner, &theme, tab, frame.close_button);
    let body_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };
    let hovered = *HOVER.lock().unwrap();
    let (rev, usage) = occupancy_stamp(ctx);
    let visible = {
        let mut memo = BODY.lock().unwrap();
        let reuse = memo.as_ref().is_some_and(|m| {
            m.tab == tab
                && m.detail == detail
                && m.width == body_area.width
                && m.hovered == hovered
                && m.rev == rev
                && m.usage == usage
        });
        if !reuse {
            let prev_snap = memo.as_ref().and_then(|m| {
                if m.rev == rev && m.usage == usage {
                    m.snap.clone()
                } else {
                    None
                }
            });
            *memo = Some(rebuild_body(
                ctx,
                tab,
                detail,
                body_area.width,
                hovered,
                rev,
                usage,
                prev_snap,
            ));
        }
        let memo = memo.as_mut().expect("usage body memo");
        memo.viewport = body_area.height;
        let slice_hits = if tab == UsageTab::Context && detail.is_none() {
            body_hits(&memo.legend, &memo.bar, memo.bar_row_len, body_area, scroll)
        } else {
            Vec::new()
        };
        let back = match (tab, detail) {
            (UsageTab::Context, Some(kind)) if scroll == 0 => {
                let label = format!("‹ {}", kind.title());
                Rect::new(
                    body_area.x,
                    body_area.y,
                    (label.width() as u16).min(body_area.width),
                    1,
                )
            }
            _ => Rect::default(),
        };
        {
            let mut hits = HITS.lock().unwrap();
            hits.slices = slice_hits;
            hits.back = back;
        }
        memo.lines
            .iter()
            .skip(scroll)
            .take(body_area.height as usize)
            .cloned()
            .collect::<Vec<_>>()
    };
    Paragraph::new(visible).render(body_area, buf);
    PickerHits {
        close_button: frame.close_button,
        ..Default::default()
    }
}

pub fn max_scroll(tab: UsageTab, detail: Option<OccupancyKind>) -> usize {
    let memo = BODY.lock().unwrap();
    let Some(m) = memo.as_ref() else {
        return 0;
    };
    if m.tab != tab || m.detail != detail {
        return 0;
    }
    m.lines.len().saturating_sub((m.viewport as usize).max(1))
}

#[allow(clippy::too_many_arguments)]
fn rebuild_body(
    ctx: &Context,
    tab: UsageTab,
    detail: Option<OccupancyKind>,
    width: u16,
    hovered: Option<OccupancyKind>,
    rev: u64,
    usage: TokenUsage,
    prev_snap: Option<ContextSnapshot>,
) -> BodyMemo {
    match (tab, detail) {
        (UsageTab::Context, Some(kind)) => {
            let lines = occupancy_detail_lines(ctx, kind, width);
            BodyMemo {
                tab,
                detail,
                width,
                hovered: None,
                rev,
                usage,
                snap: None,
                lines,
                legend: Vec::new(),
                bar: Vec::new(),
                bar_row_len: 0,
                viewport: 0,
            }
        }
        (UsageTab::Context, None) => {
            let snap = prev_snap.unwrap_or_else(|| snapshot_context(ctx));
            let view = context_view(&snap, width, hovered);
            BodyMemo {
                tab,
                detail,
                width,
                hovered,
                rev,
                usage,
                snap: Some(snap),
                lines: view.lines,
                legend: view.legend,
                bar: view.bar,
                bar_row_len: view.bar_row_len,
                viewport: 0,
            }
        }
        (UsageTab::Session, _) => BodyMemo {
            tab,
            detail: None,
            width,
            hovered: None,
            rev,
            usage,
            snap: None,
            lines: session_lines(ctx),
            legend: Vec::new(),
            bar: Vec::new(),
            bar_row_len: 0,
            viewport: 0,
        },
    }
}

fn session_lines(ctx: &Context) -> Vec<Line<'static>> {
    let usage = ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.prompt_usage())
        .unwrap_or_default();
    let theme = Theme::current();
    session_usage_block_text(&usage)
        .lines()
        .enumerate()
        .map(|(i, row)| {
            if i == 0 {
                Line::styled(
                    row.to_string(),
                    Style::default()
                        .fg(theme.text_primary)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Line::styled(row.to_string(), Style::default().fg(theme.text_primary))
            }
        })
        .collect()
}

fn paint_tabs(buf: &mut Buffer, inner: Rect, theme: &Theme, tab: UsageTab, close: Rect) {
    let close_x = if close.width == 0 {
        inner.x + inner.width
    } else {
        close.x
    };
    let mut x = inner.x;
    let y = inner.y;
    let context = paint_tab(
        buf,
        x,
        y,
        close_x.saturating_sub(x),
        theme,
        UsageTab::Context.label(),
        tab == UsageTab::Context,
    );
    x = context.x + context.width + 2;
    let session = if x + 2 < close_x {
        paint_tab(
            buf,
            x,
            y,
            close_x.saturating_sub(x),
            theme,
            UsageTab::Session.label(),
            tab == UsageTab::Session,
        )
    } else {
        Rect::default()
    };
    let mut hits = HITS.lock().unwrap();
    hits.context = context;
    hits.session = session;
}

fn paint_tab(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_w: u16,
    theme: &Theme,
    label: &str,
    active: bool,
) -> Rect {
    let w = (label.width() as u16).min(max_w);
    if w == 0 {
        return Rect::default();
    }
    let style = if active {
        Style::default()
            .fg(theme.text_primary)
            .bg(theme.bg_base)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default().fg(theme.gray).bg(theme.bg_base)
    };
    buf.set_span(x, y, &Span::styled(label, style), w);
    Rect::new(x, y, w, 1)
}

fn occupancy_detail_lines(ctx: &Context, kind: OccupancyKind, width: u16) -> Vec<Line<'static>> {
    let theme = Theme::current();
    let detail = occupancy_detail(ctx, kind);
    paint_occupancy_detail(&detail, &theme, width)
}

fn paint_occupancy_detail(
    detail: &OccupancyDetail,
    theme: &Theme,
    width: u16,
) -> Vec<Line<'static>> {
    let muted = theme.muted();
    let primary = Style::default()
        .fg(theme.text_primary)
        .add_modifier(Modifier::BOLD);
    let secondary = Style::default().fg(theme.text_secondary);
    let mut lines = vec![
        Line::from(Span::styled(format!("‹ {}", detail.kind.title()), primary)),
        Line::from(Span::styled(
            format!("{} tokens", fmt_tok_big(detail.tokens)),
            secondary,
        )),
        Line::from(""),
    ];
    for group in &detail.groups {
        if !group.heading.is_empty() {
            lines.push(Line::from(Span::styled(group.heading.clone(), secondary)));
        }
        for row in &group.rows {
            let mut spans = vec![Span::styled(row.label.clone(), secondary)];
            if let Some(tok) = row.tokens {
                spans.push(Span::styled(format!("  {}", fmt_tok(tok)), muted));
            }
            if let Some(note) = &row.note {
                spans.push(Span::styled(format!("  {note}"), muted));
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(""));
    }
    if let Some(text) = &detail.text {
        let wrap_w = (width as usize).max(8);
        for raw in text.lines() {
            if raw.is_empty() {
                lines.push(Line::from(""));
                continue;
            }
            let mut rest = raw;
            while !rest.is_empty() {
                let mut acc = 0usize;
                let mut take = rest.len();
                for (i, ch) in rest.char_indices() {
                    let w = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]));
                    if acc + w > wrap_w && acc > 0 {
                        take = i;
                        break;
                    }
                    acc += w;
                    take = i + ch.len_utf8();
                }
                lines.push(Line::from(Span::styled(rest[..take].to_string(), muted)));
                rest = &rest[take..];
            }
        }
    }
    lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BarLayout {
    row_len: usize,
    rows: usize,
}

impl BarLayout {
    const WIDE: Self = Self {
        row_len: 20,
        rows: 5,
    };
    const NARROW: Self = Self {
        row_len: 10,
        rows: 10,
    };
    const NARROW_BREAKPOINT: u16 = 50;

    fn for_width(width: u16) -> Self {
        if width < Self::NARROW_BREAKPOINT {
            Self::NARROW
        } else {
            Self::WIDE
        }
    }

    const fn total(self) -> usize {
        self.row_len * self.rows
    }
}

struct LegendRow {
    kind: OccupancyKind,
    glyph: &'static str,
    color: Color,
    label: String,
    tokens: u64,
    detail: Option<String>,
}

struct RowLayout {
    label_width: usize,
    tokens_width: usize,
    percent_width: usize,
    count_width: usize,
}

impl RowLayout {
    fn measure<'a>(rows: impl Iterator<Item = &'a LegendRow> + Clone, total: u64) -> Self {
        Self {
            label_width: rows
                .clone()
                .map(|r| UnicodeWidthStr::width(r.label.as_str()))
                .max()
                .unwrap_or(0)
                + 1,
            tokens_width: rows
                .clone()
                .map(|r| fmt_tok(r.tokens).chars().count())
                .max()
                .unwrap_or(0),
            percent_width: rows
                .clone()
                .map(|r| Self::percent(r.tokens, total).chars().count())
                .max()
                .unwrap_or(0),
            count_width: rows
                .filter_map(|r| r.detail.as_deref())
                .filter_map(|d| d.split(' ').next().map(|n| n.chars().count()))
                .max()
                .unwrap_or(0),
        }
    }

    fn percent(tokens: u64, total: u64) -> String {
        format!("({})", percent_of_window(tokens, total))
    }

    fn cells(&self, tokens: u64, total: u64) -> String {
        format!(
            "{:>tokens_width$} tokens   {:>percent_width$}",
            fmt_tok(tokens),
            Self::percent(tokens, total),
            tokens_width = self.tokens_width,
            percent_width = self.percent_width,
        )
    }

    fn detail_suffix(&self, detail: &str) -> String {
        match detail.split_once(' ') {
            Some((count, rest)) => format!(
                " \u{00b7} {count:>count_width$} {rest}",
                count_width = self.count_width
            ),
            None => format!(" \u{00b7} {detail}"),
        }
    }

    fn render(
        &self,
        row: &LegendRow,
        bar: BarLayout,
        total: u64,
        label_style: Style,
        muted: Style,
        hovered: bool,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut glyph_style = Style::default().fg(row.color);
        let mut label_style = label_style;
        let mut muted = muted;
        if hovered {
            glyph_style = glyph_style.bg(theme.bg_hover);
            label_style = label_style
                .fg(theme.text_primary)
                .bg(theme.bg_hover)
                .add_modifier(Modifier::UNDERLINED);
            muted = muted.bg(theme.bg_hover);
        }
        let glyph = Span::styled(format!("{} ", row.glyph), glyph_style);
        let suffix = row.detail.as_deref().map(|d| self.detail_suffix(d));
        let chevron = Span::styled(
            format!(" {}", glyphs::chevron()),
            if hovered {
                Style::default().fg(theme.text_primary).bg(theme.bg_hover)
            } else {
                Style::default().fg(theme.gray)
            },
        );
        if bar == BarLayout::NARROW {
            let first = Line::from(vec![
                glyph,
                Span::styled(row.label.clone(), label_style),
                chevron.clone(),
            ]);
            let mut second = vec![
                Span::raw(" "),
                Span::styled(
                    format!(
                        "{} tokens   {}",
                        fmt_tok(row.tokens),
                        Self::percent(row.tokens, total)
                    ),
                    muted,
                ),
            ];
            if let Some(extra) = suffix {
                second.push(Span::styled(extra, muted));
            }
            vec![first, Line::from(second)]
        } else {
            let mut spans = vec![
                glyph,
                Span::styled(pad_display(&row.label, self.label_width), label_style),
                Span::raw(" "),
                Span::styled(self.cells(row.tokens, total), muted),
            ];
            if let Some(extra) = suffix {
                spans.push(Span::styled(extra, muted));
            }
            spans.push(chevron);
            vec![Line::from(spans)]
        }
    }
}

fn pad_display(s: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - w))
    }
}

struct ContextView {
    lines: Vec<Line<'static>>,
    legend: Vec<(usize, usize, OccupancyKind)>,
    bar: Vec<(usize, usize, OccupancyKind)>,
    bar_row_len: usize,
}

impl ContextView {
    #[cfg(test)]
    fn screen_hits(&self, body: Rect, scroll: usize) -> Vec<(Rect, OccupancyKind)> {
        body_hits(&self.legend, &self.bar, self.bar_row_len, body, scroll)
    }
}

fn body_hits(
    legend: &[(usize, usize, OccupancyKind)],
    bar: &[(usize, usize, OccupancyKind)],
    bar_row_len: usize,
    body: Rect,
    scroll: usize,
) -> Vec<(Rect, OccupancyKind)> {
    let mut out = Vec::new();
    for &(line, cell, kind) in bar {
        if let Some(rect) = visible_cell(body, scroll, line, cell, bar_row_len) {
            out.push((rect, kind));
        }
    }
    for &(start, rows, kind) in legend {
        if let Some(rect) = visible_span(body, scroll, start, rows) {
            out.push((rect, kind));
        }
    }
    out
}

fn visible_span(body: Rect, scroll: usize, start: usize, rows: usize) -> Option<Rect> {
    let end = start + rows;
    if end <= scroll {
        return None;
    }
    let vis_start = start.max(scroll);
    let vis_end = end.min(scroll + body.height as usize);
    if vis_start >= vis_end {
        return None;
    }
    Some(Rect::new(
        body.x,
        body.y + (vis_start - scroll) as u16,
        body.width,
        (vis_end - vis_start) as u16,
    ))
}

fn visible_cell(
    body: Rect,
    scroll: usize,
    line: usize,
    cell: usize,
    row_len: usize,
) -> Option<Rect> {
    if line < scroll || line >= scroll + body.height as usize {
        return None;
    }
    let x = body.x.saturating_add((cell as u16).saturating_mul(2));
    if x >= body.x + body.width {
        return None;
    }
    let w = if cell + 1 == row_len { 1 } else { 2 };
    Some(Rect::new(
        x,
        body.y + (line - scroll) as u16,
        w.min(body.width.saturating_sub(x - body.x)),
        1,
    ))
}

fn category_kind(label: &str) -> OccupancyKind {
    match label {
        "MCP 服务器" => OccupancyKind::Mcp,
        "工作流" => OccupancyKind::Workflows,
        "技能" => OccupancyKind::Skills,
        _ => OccupancyKind::Tools,
    }
}

#[cfg(test)]
pub(crate) fn context_lines(snapshot: &ContextSnapshot, width: u16) -> Vec<Line<'static>> {
    context_view(snapshot, width, None).lines
}

fn context_view(
    snapshot: &ContextSnapshot,
    width: u16,
    hovered: Option<OccupancyKind>,
) -> ContextView {
    let theme = Theme::current();
    let bar = BarLayout::for_width(width);
    let used = snapshot.used;
    let total = snapshot.total;
    let usage_pct = snapshot.usage_pct;
    let system_tokens = snapshot.system_prompt_tokens;
    let tool_tokens = snapshot.tool_definitions_tokens;
    let tool_count = snapshot.tool_definitions_count;
    let message_tokens = snapshot.message_tokens;
    let free_tokens = snapshot.free_tokens;
    let overhead_tokens = used.saturating_sub(system_tokens.saturating_add(message_tokens));

    let muted = theme.muted();
    let system_color = theme.gray_bright;
    let tools_color = theme.accent_skill;
    let messages_color = theme.text_primary;
    let empty_color = theme.gray_dim;
    let overhead_color = theme.accent_verify;

    let system_glyph = glyphs::diamond_filled();
    let tools_glyph = glyphs::diamond_dotted();
    let messages_glyph = glyphs::diamond_filled();
    let free_glyph = glyphs::diamond_hollow();
    let overhead_glyph = glyphs::diamond_filled();

    let total_cells = bar.total();
    let cells_for = |tokens: u64| -> usize {
        if total == 0 {
            0
        } else {
            ((tokens as f64 / total as f64) * total_cells as f64).round() as usize
        }
    };
    let used_cells = cells_for(used).min(total_cells);
    let system_cells = cells_for(system_tokens).min(used_cells);
    let messages_cells = cells_for(message_tokens).min(used_cells.saturating_sub(system_cells));
    let overhead_cells = used_cells.saturating_sub(system_cells + messages_cells);
    let free_cells = total_cells.saturating_sub(used_cells);

    let mut cell_kinds: Vec<OccupancyKind> = Vec::with_capacity(total_cells);
    let mut cells: Vec<(&'static str, Color)> = Vec::with_capacity(total_cells);
    for _ in 0..system_cells {
        cells.push((system_glyph, system_color));
        cell_kinds.push(OccupancyKind::System);
    }
    for _ in 0..messages_cells {
        cells.push((messages_glyph, messages_color));
        cell_kinds.push(OccupancyKind::Messages);
    }
    for _ in 0..overhead_cells {
        cells.push((overhead_glyph, overhead_color));
        cell_kinds.push(OccupancyKind::Overhead);
    }
    for _ in 0..free_cells {
        cells.push((free_glyph, empty_color));
        cell_kinds.push(OccupancyKind::Free);
    }

    let mut bar_lines: Vec<Line<'static>> = Vec::with_capacity(bar.rows);
    let mut bar_hits = Vec::new();
    for row_idx in 0..bar.rows {
        let start = row_idx * bar.row_len;
        let end = (start + bar.row_len).min(cells.len());
        let mut spans = Vec::with_capacity(bar.row_len * 2);
        for (i, (glyph, color)) in cells[start..end].iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            let kind = cell_kinds
                .get(start + i)
                .copied()
                .unwrap_or(OccupancyKind::Free);
            let mut style = Style::default().fg(*color);
            if hovered == Some(kind) {
                style = style.bg(theme.bg_hover).add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled((*glyph).to_string(), style));
        }
        bar_lines.push(Line::from(spans));
    }

    let mut legend_rows = vec![
        LegendRow {
            kind: OccupancyKind::System,
            glyph: system_glyph,
            color: system_color,
            label: "系统提示".into(),
            tokens: system_tokens,
            detail: None,
        },
        LegendRow {
            kind: OccupancyKind::Messages,
            glyph: messages_glyph,
            color: messages_color,
            label: "消息".into(),
            tokens: message_tokens,
            detail: None,
        },
    ];
    if overhead_tokens > 0 {
        legend_rows.push(LegendRow {
            kind: OccupancyKind::Overhead,
            glyph: overhead_glyph,
            color: overhead_color,
            label: "推理/开销".into(),
            tokens: overhead_tokens,
            detail: None,
        });
    }
    legend_rows.push(LegendRow {
        kind: OccupancyKind::Free,
        glyph: free_glyph,
        color: empty_color,
        label: "空闲".into(),
        tokens: free_tokens,
        detail: None,
    });
    let info_rows: Vec<LegendRow> = std::iter::once(LegendRow {
        kind: OccupancyKind::Tools,
        glyph: tools_glyph,
        color: tools_color,
        label: "工具定义".into(),
        tokens: tool_tokens,
        detail: Some(format!("{tool_count} 个工具")),
    })
    .chain(snapshot.categories.iter().map(|c| LegendRow {
        kind: category_kind(&c.label),
        glyph: tools_glyph,
        color: tools_color,
        label: c.label.clone(),
        tokens: c.tokens,
        detail: c.detail.clone(),
    }))
    .collect();
    let layout = RowLayout::measure(legend_rows.iter().chain(info_rows.iter()), total);
    let label_style = Style::default().fg(theme.text_secondary);

    let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled(
        format!(
            "{} / {} tokens ({:.2}%)",
            fmt_tok_big(used),
            fmt_tok_big(total),
            precise_usage_percent(used, total),
        ),
        Style::default().fg(theme.text_secondary),
    ))];
    if !snapshot.model.is_empty() {
        lines.push(Line::from(Span::styled(
            snapshot.model.clone(),
            Style::default().fg(theme.gray_bright),
        )));
    }
    lines.push(Line::from(""));
    let bar_start = lines.len();
    lines.extend(bar_lines);
    for row_idx in 0..bar.rows {
        let start = row_idx * bar.row_len;
        let end = (start + bar.row_len).min(cell_kinds.len());
        for cell in 0..(end.saturating_sub(start)) {
            bar_hits.push((bar_start + row_idx, cell, cell_kinds[start + cell]));
        }
    }
    lines.push(Line::from(""));
    let mut legend_hits = Vec::new();
    for row in &legend_rows {
        let start = lines.len();
        let painted = layout.render(
            row,
            bar,
            total,
            label_style,
            muted,
            hovered == Some(row.kind),
            &theme,
        );
        let n = painted.len();
        lines.extend(painted);
        legend_hits.push((start, n, row.kind));
    }
    lines.push(Line::from(""));
    for row in &info_rows {
        let start = lines.len();
        let painted = layout.render(
            row,
            bar,
            total,
            label_style,
            muted,
            hovered == Some(row.kind),
            &theme,
        );
        let n = painted.len();
        lines.extend(painted);
        legend_hits.push((start, n, row.kind));
    }
    lines.push(Line::from(""));

    if total > 0 {
        let threshold_percent = snapshot.auto_compact_threshold_percent;
        let remaining = snapshot.remaining_to_compact();
        let (text, style) = if usage_pct >= threshold_percent {
            (
                format!("下回合将自动压缩（阈值 {threshold_percent}%）"),
                Style::default().fg(theme.warning),
            )
        } else {
            (
                format!(
                    "{threshold_percent}% 时自动压缩 \u{00b7} 约 {} token 剩余",
                    fmt_tok_big(remaining)
                ),
                muted,
            )
        };
        lines.push(Line::from(Span::styled(text, style)));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        format!(
            "回合 {} \u{00b7} 工具调用 {} \u{00b7} 压缩 {}",
            snapshot.turn_count, snapshot.tool_call_count, snapshot.compaction_count
        ),
        muted,
    )));
    lines.push(Line::from(Span::styled(
        "点击分类或色块查看详情".to_string(),
        Style::default().fg(theme.gray),
    )));

    if (80..snapshot.auto_compact_threshold_percent).contains(&usage_pct) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "提示：运行 /compact 可腾出上下文。".to_string(),
            Style::default().fg(theme.warning),
        )));
    }

    ContextView {
        lines,
        legend: legend_hits,
        bar: bar_hits,
        bar_row_len: bar.row_len,
    }
}

fn fmt_tok(n: u64) -> String {
    if n >= 99_500 {
        format!("{}k", (n + 500) / 1000)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{n}")
    }
}

fn precise_usage_percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64) * 100.0
    }
}

fn fmt_tok_big(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}m", n as f64 / 1_000_000.0)
    } else {
        fmt_tok(n)
    }
}

fn percent_of_window(part: u64, total: u64) -> String {
    if total == 0 {
        return "-".to_string();
    }
    let p = ((part as f64 / total as f64) * 100.0).max(if part > 0 { 0.1 } else { 0.0 });
    if p < 10.0 {
        format!("{p:.1}%")
    } else {
        format!("{p:.0}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(lines: &[Line<'static>], idx: usize) -> String {
        lines[idx]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    fn all_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()).chain(["\n"]))
            .collect()
    }

    fn snapshot() -> ContextSnapshot {
        ContextSnapshot {
            used: 36_700,
            total: 1_000_000,
            model: "dock-test".into(),
            system_prompt_tokens: 1_200,
            message_tokens: 29_900,
            tool_definitions_tokens: 5_600,
            tool_definitions_count: 12,
            free_tokens: 963_300,
            usage_pct: 4,
            auto_compact_threshold_percent: 85,
            turn_count: 5,
            tool_call_count: 12,
            compaction_count: 0,
            categories: Vec::new(),
        }
    }

    const FILLED: &str = "\u{25C6}";
    const DOTTED: &str = "\u{25C8}";
    const HOLLOW: &str = "\u{25C7}";

    fn count_bar_glyphs(
        lines: &[Line<'static>],
        layout: BarLayout,
    ) -> (usize, usize, usize, usize) {
        let mut diamonds = 0usize;
        let mut tools = 0usize;
        let mut free = 0usize;
        let bar_start = 3;
        let bar_end = bar_start + layout.rows;
        for line in &lines[bar_start..bar_end] {
            for span in &line.spans {
                let c = span.content.as_ref();
                if c == FILLED {
                    diamonds += 1;
                } else if c == DOTTED {
                    tools += 1;
                } else if c == HOLLOW {
                    free += 1;
                }
            }
        }
        (diamonds, tools, free, diamonds + tools + free)
    }

    #[test]
    fn bar_total_cells_always_sum_to_one_hundred() {
        let lines = context_lines(&snapshot(), 80);
        let (_, _, _, total) = count_bar_glyphs(&lines, BarLayout::WIDE);
        assert_eq!(total, 100);
    }

    #[test]
    fn tool_definitions_never_enter_the_bar() {
        let mut snap = snapshot();
        snap.used = 1_000;
        snap.total = 1_000;
        snap.system_prompt_tokens = 495;
        snap.message_tokens = 10;
        snap.tool_definitions_tokens = 800;
        snap.free_tokens = 0;
        snap.usage_pct = 100;
        let lines = context_lines(&snap, 80);
        let (diamonds, tools, free, total) = count_bar_glyphs(&lines, BarLayout::WIDE);
        assert_eq!(total, 100);
        assert_eq!(tools, 0);
        assert_eq!(diamonds, 100);
        assert_eq!(free, 0);
    }

    #[test]
    fn chinese_legend_and_footer() {
        let lines = context_lines(&snapshot(), 80);
        let all = all_text(&lines);
        assert!(all.contains("系统提示"), "{all}");
        assert!(all.contains("消息"), "{all}");
        assert!(all.contains("空闲"), "{all}");
        assert!(all.contains("工具定义"), "{all}");
        assert!(all.contains("12 个工具"), "{all}");
        assert!(all.contains("85% 时自动压缩"), "{all}");
        assert!(all.contains("回合 5"), "{all}");
        assert_eq!(line_text(&lines, 1), "dock-test");
    }

    #[test]
    fn other_row_unused_when_empty_categories() {
        let all = all_text(&context_lines(&snapshot(), 80));
        assert!(!all.contains("MCP 服务器"));
        assert!(!all.contains("工作流"));
        assert!(!all.contains("技能"));
    }

    #[test]
    fn skills_category_maps_to_skills_kind() {
        let mut snap = snapshot();
        snap.categories.push(ContextCategory {
            label: "技能".into(),
            tokens: 40,
            detail: Some("1 个".into()),
        });
        let view = context_view(&snap, 80, None);
        let kinds: Vec<_> = view.legend.iter().map(|h| h.2).collect();
        assert!(kinds.contains(&OccupancyKind::Skills), "{kinds:?}");
        assert!(all_text(&view.lines).contains("技能"));
    }

    #[test]
    fn narrow_bar_still_one_hundred_cells() {
        let lines = context_lines(&snapshot(), 40);
        let (_, _, _, total) = count_bar_glyphs(&lines, BarLayout::NARROW);
        assert_eq!(total, 100);
    }

    #[test]
    fn legend_rows_are_clickable_kinds_with_chevron() {
        let view = context_view(&snapshot(), 80, None);
        let kinds: Vec<_> = view.legend.iter().map(|h| h.2).collect();
        assert!(kinds.contains(&OccupancyKind::System), "{kinds:?}");
        assert!(kinds.contains(&OccupancyKind::Messages), "{kinds:?}");
        assert!(kinds.contains(&OccupancyKind::Free), "{kinds:?}");
        assert!(kinds.contains(&OccupancyKind::Tools), "{kinds:?}");
        let all = all_text(&view.lines);
        assert!(all.contains('\u{203A}'), "missing chevron:\n{all}");
        assert!(all.contains("点击分类"), "{all}");
        let body = Rect::new(0, 0, 80, 40);
        let hits = view.screen_hits(body, 0);
        assert!(
            hits.iter().any(|(_, k)| *k == OccupancyKind::System),
            "{hits:?}"
        );
    }

    #[test]
    fn paint_detail_has_back_affordance() {
        let detail = OccupancyDetail {
            kind: OccupancyKind::System,
            tokens: 12,
            groups: Vec::new(),
            text: Some("You are a test agent.".into()),
        };
        let lines = paint_occupancy_detail(&detail, &Theme::current(), 40);
        let all = all_text(&lines);
        assert!(all.contains("‹ 系统提示"), "{all}");
        assert!(all.contains("You are a test agent."), "{all}");
    }
}
