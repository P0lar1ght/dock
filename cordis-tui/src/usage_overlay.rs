//! `/context` occupancy + `/usage` ledger, tabbed overlay.
//!
//! Diamond bar chrome follows Grok `context_info.rs`. Session tab is the
//! existing ledger ([`session_usage_block_text`]). No grok.com billing.

use std::sync::Mutex;
use std::time::Duration;

use cordis::Context;
use cordis_spine::{
    calls_breakdown, format_cost, format_duration, group_thousands, hit_rate_trend,
    miss_breakdown_text, occupancy_detail, per_model_amounts, session_usage_block_text,
    share_percent, snapshot_context, CacheSegmentKind, ContextBook, ContextSnapshot,
    OccupancyDetail, OccupancyKind, Sessions, TokenUsage, CONTEXT, SESSIONS,
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
    /// The body is coloured in `rebuild_body`, so it is theme-dependent:
    /// `Theme::apply_kind` notifies nobody, and without this in the reuse key
    /// a palette switch leaves the overlay painted in the old colours.
    theme_kind: crate::theme::ThemeKind,
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
    let theme_kind = Theme::current_kind();
    let visible = {
        let mut memo = BODY.lock().unwrap();
        let reuse = memo.as_ref().is_some_and(|m| {
            m.tab == tab
                && m.detail == detail
                && m.width == body_area.width
                && m.hovered == hovered
                && m.theme_kind == theme_kind
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
    let theme_kind = Theme::current_kind();
    match (tab, detail) {
        (UsageTab::Context, Some(kind)) => {
            let lines = occupancy_detail_lines(ctx, kind, width);
            BodyMemo {
                tab,
                detail,
                width,
                hovered: None,
                theme_kind,
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
            let snap = prev_snap.unwrap_or_else(|| {
                ctx.get::<ContextBook>(CONTEXT)
                    .map(|b| b.window())
                    .unwrap_or_else(|| snapshot_context(ctx))
            });
            let view = context_view(&snap, width, hovered);
            BodyMemo {
                tab,
                detail,
                width,
                hovered,
                theme_kind,
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
            theme_kind,
            rev,
            usage,
            snap: None,
            lines: session_lines(ctx, width),
            legend: Vec::new(),
            bar: Vec::new(),
            bar_row_len: 0,
            viewport: 0,
        },
    }
}

fn session_lines(ctx: &Context, width: u16) -> Vec<Line<'static>> {
    let usage = ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.prompt_usage())
        .unwrap_or_default();
    session_view(&usage, width)
}

/// 缓存三段的画法。三段互不相交、加起来是全量输入，所以共用一条 bar。
///
/// 每段的字形不同而不只是颜色不同：Context 页也是这么分的（实心 / 点 / 空心
/// 菱形），而且单色终端与截图里只靠颜色分段等于没分。
struct CacheColors {
    hit: (char, Color),
    write: (char, Color),
    miss: (char, Color),
}

impl CacheColors {
    fn from_theme(theme: &Theme) -> Self {
        Self {
            hit: ('█', theme.accent_success),
            write: ('▓', theme.accent_verify),
            miss: ('░', theme.gray_dim),
        }
    }

    /// spine 定义分段，这里只负责给它挑字形与颜色。
    fn of(&self, kind: CacheSegmentKind) -> (char, Color) {
        match kind {
            CacheSegmentKind::Hit => self.hit,
            CacheSegmentKind::Write => self.write,
            CacheSegmentKind::Miss => self.miss,
        }
    }
}

/// Session 页。Context 页早就是"条 + 图例"的画法，这里沿用同一套，
/// 免得同一个 overlay 里两页读起来像两个程序。
fn session_view(usage: &cordis_spine::PromptUsage, width: u16) -> Vec<Line<'static>> {
    let theme = Theme::current();
    let t = &usage.totals;
    let muted = theme.muted();
    let primary = Style::default()
        .fg(theme.text_primary)
        .add_modifier(Modifier::BOLD);
    let secondary = Style::default().fg(theme.text_secondary);

    if t.model_calls == 0 && usage.model_usage.is_empty() {
        // 空账本没什么可画的，一句话比一条空条子诚实。
        return session_usage_block_text(usage)
            .lines()
            .map(|row| Line::styled(row.to_string(), secondary))
            .collect();
    }

    let colors = CacheColors::from_theme(&theme);
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            "本会话用量（自开始或上次恢复）".to_string(),
            primary,
        )),
        Line::from(""),
    ];

    // 明细在前、图在后：要对账的是这几个数，bar 只是让比例一眼看出来。
    lines.extend(amount_rows(t, &colors, secondary, muted));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        match t.cache_hit_rate() {
            Some(rate) => format!("缓存命中 {:.0}% of 完整输入", rate * 100.0),
            None => "缓存命中 -".to_string(),
        },
        secondary,
    )));
    lines.push(cache_bar(t, &colors, width));
    lines.push(Line::from(cache_bar_legend(t, &colors, muted)));

    if let Some(last) = &usage.last_call {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "上一轮  输入(未命中) {} \u{00b7} 缓存命中 {} \u{00b7} 完整输入 {}",
                group_thousands(last.uncached_input_tokens()),
                group_thousands(last.cached_read_tokens),
                group_thousands(last.input_tokens),
            ),
            secondary,
        )));
    }
    // 总计折了子代理与压缩，而「上一轮」和走势只有主循环。不把未命中按来源拆
    // 开，这两个口径并排摆着就会被读成「每一轮都在漏 token」——实际大头往往是
    // 子代理冷启动（各自一套系统提示，第一次调用必然整份满价）。
    if let Some(breakdown) = miss_breakdown_text(usage) {
        lines.push(Line::from(Span::styled(
            format!("未命中来源  {breakdown}"),
            secondary,
        )));
    }
    // 标签占 12 列，右边的计数后缀留 24 列；剩下的才是格子。放不下时
    // `hit_rate_trend` 丢最旧的，不能让渲染层在右边裁掉最新的那几次。
    const TREND_LABEL: &str = "每轮命中率  ";
    let trend_cells = (width as usize).saturating_sub(TREND_LABEL.width() + 24);
    if let Some(trend) = hit_rate_trend(&usage.recent_calls, trend_cells) {
        let drawn = trend.chars().count();
        lines.push(Line::from(vec![
            Span::styled(TREND_LABEL.to_string(), muted),
            Span::styled(trend, Style::default().fg(colors.hit.1)),
            // 报**实际画出来的格数**：这个轴是「有输入的主循环调用」，和下面
            // 那行的「模型调用」不是一个口径，不写清楚就会被读成少画了一格。
            Span::styled(format!("  最近 {drawn} 次主循环调用"), muted),
        ]));
        lines.push(Line::from(Span::styled(
            "  旧 → 新 \u{00b7} 掉一格通常是前缀被改写（压缩 / 系统提示或工具表变了）".to_string(),
            Style::default().fg(theme.gray),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            "模型调用 {} \u{00b7} API 耗时 {} \u{00b7} 费用 {}",
            calls_breakdown(usage),
            format_duration(Duration::from_millis(t.api_duration_ms)),
            format_cost(t),
        ),
        muted,
    )));

    if usage.model_usage.len() > 1 {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("按模型".to_string(), secondary)));
        for (model, m) in &usage.model_usage {
            lines.push(Line::from(Span::styled(
                format!("  {model}  {}", per_model_amounts(m)),
                muted,
            )));
        }
    }

    if usage.usage_is_incomplete {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "用量统计不完整，可能少计。".to_string(),
            Style::default().fg(theme.warning),
        )));
    }
    lines
}

/// 一条按 token 比例分段的条：命中 / 写入 / 未命中。
///
/// 非零的段至少占一格——四舍五入到 0 会让"有 300 token 白付了全价"这件事
/// 从图上消失，而那正是要看的东西。
fn cache_bar(
    t: &cordis_spine::PromptUsageModel,
    colors: &CacheColors,
    width: u16,
) -> Line<'static> {
    let cells = (width as usize).clamp(8, 40);
    let parts = [
        (t.cached_read_tokens, colors.hit),
        (t.cache_creation_tokens, colors.write),
        (t.uncached_input_tokens(), colors.miss),
    ];
    let total = t.input_tokens.max(1);
    let mut spans = Vec::new();
    let mut used = 0usize;
    for (i, (tokens, (glyph, color))) in parts.iter().enumerate() {
        if *tokens == 0 {
            continue;
        }
        let raw = ((*tokens as f64 / total as f64) * cells as f64).round() as usize;
        let left = cells.saturating_sub(used);
        // 后面还有几个非零段，就给它们各留一格：不留的话 99.95% 的命中会四舍
        // 五入吃掉整条，剩下那点全价未命中直接从图上消失。
        let reserved = parts[i + 1..].iter().filter(|(n, _)| *n > 0).count();
        let n = if reserved == 0 {
            // 最后一个非零段吃掉舍入残余，条长才稳定。
            left
        } else {
            raw.clamp(1, left.saturating_sub(reserved).max(1))
        };
        if n == 0 {
            continue;
        }
        used += n;
        spans.push(Span::styled(
            glyph.to_string().repeat(n),
            Style::default().fg(*color),
        ));
    }
    if spans.is_empty() {
        spans.push(Span::styled(
            "─".repeat(cells),
            Style::default().fg(colors.miss.1),
        ));
    }
    Line::from(spans)
}

/// 逐项精确数值，Claude Code `/cost` 的口径：**「输入」指未命中的那部分**，
/// 缓存读 / 写各自单列。三者互不相交，加起来就是下面那行「完整输入」。
///
/// 这里不缩写成 `131k`：这几个数是用来对账的，四舍五入过的数对不上账。
fn amount_rows(
    t: &cordis_spine::PromptUsageModel,
    colors: &CacheColors,
    secondary: Style,
    muted: Style,
) -> Vec<Line<'static>> {
    /// 一行明细。`mark` = 该行在 bar 上对应的字形与颜色；`None` 的行（输出）
    /// 不在那条 bar 里，留空白对齐。
    struct AmountRow {
        mark: Option<(char, Color)>,
        label: &'static str,
        tokens: u64,
        note: String,
    }

    // 分段、文案、「写入为 0 不占行」的规则都来自 spine：这套口径以前在文本块
    // 和这里各写一遍，改一处必漏另一处。
    let mut rows: Vec<AmountRow> = t
        .cache_segments()
        .into_iter()
        .map(|seg| AmountRow {
            mark: Some(colors.of(seg.kind)),
            label: seg.kind.label(),
            tokens: seg.tokens,
            // 计价口径由 spine 出，它还知道这条 wire 报不报「写入」——不报的
            // wire 上，这一轮新写进缓存的 token 就混在「未命中」里。
            note: t.segment_note(&seg),
        })
        .collect();
    rows.push(AmountRow {
        mark: None,
        label: "输出",
        tokens: t.output_tokens,
        note: format!("思考 {}", group_thousands(t.reasoning_tokens)),
    });

    // 「完整输入」也参与对齐，否则最宽的那个数会把这一列顶出去。
    let num_w = rows
        .iter()
        .map(|r| group_thousands(r.tokens).chars().count())
        .chain(std::iter::once(
            group_thousands(t.input_tokens).chars().count(),
        ))
        .max()
        .unwrap_or(0);
    let label_w = rows
        .iter()
        .map(|r| UnicodeWidthStr::width(r.label))
        .chain(std::iter::once(UnicodeWidthStr::width("完整输入")))
        .max()
        .unwrap_or(0);

    let mut lines: Vec<Line<'static>> = rows
        .iter()
        .map(|r| {
            let (glyph, color) = r.mark.unwrap_or((' ', muted.fg.unwrap_or(Color::Reset)));
            Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(color)),
                Span::styled(pad_display(r.label, label_w + 2), secondary),
                Span::styled(format!("{:>num_w$}", group_thousands(r.tokens)), secondary),
                Span::styled(format!("   {}", r.note), muted),
            ])
        })
        .collect();

    // 完整输入 = 上面三项之和。单列一行是因为它和「输入(未命中)」差一个量级，
    // 两个都叫"输入"最容易看串。
    lines.push(Line::from(vec![
        Span::styled("  ".to_string(), muted),
        Span::styled(pad_display("完整输入", label_w + 2), muted),
        Span::styled(
            format!("{:>num_w$}", group_thousands(t.input_tokens)),
            muted,
        ),
        Span::styled(
            format!("   合计 {}", group_thousands(t.total_tokens)),
            muted,
        ),
    ]));
    lines
}

/// bar 底下的一行紧凑图例。精确数值已经在上面的表里，这里只解释字形。
fn cache_bar_legend(
    t: &cordis_spine::PromptUsageModel,
    colors: &CacheColors,
    muted: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    // 顺序跟 bar 一致（未命中 → 命中 → 写入），图例和色块对得上。
    for seg in t.cache_segments() {
        if !spans.is_empty() {
            spans.push(Span::styled("   ".to_string(), muted));
        }
        let (glyph, color) = colors.of(seg.kind);
        spans.push(Span::styled(
            format!("{glyph} "),
            Style::default().fg(color),
        ));
        spans.push(Span::styled(
            format!("{} {}", seg.kind.short_label(), t.segment_share(&seg)),
            muted,
        ));
    }
    spans
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
    let detail = ctx
        .get::<ContextBook>(CONTEXT)
        .map(|b| b.detail(kind))
        .unwrap_or_else(|| occupancy_detail(ctx, kind));
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
        format!("({})", share_percent(tokens, total))
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

    #[allow(clippy::too_many_arguments)]
    // TUI 绘制/布局函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到所有调用点，故意保留。
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
        "本地按需" => OccupancyKind::Deferred,
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

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_spine::ContextCategory;

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

    /// Only the memoised body region — the frame chrome is painted fresh from
    /// the live theme and would mask a stale body.
    fn painted_body_fgs(ctx: &Context, area: Rect) -> Vec<Color> {
        let theme = Theme::current();
        let inner = render_floating_frame(&mut Buffer::empty(area), area, &theme, false)
            .expect("floating frame")
            .content;
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, ctx, UsageTab::Session, 0, None);
        let body = Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(1),
        );
        let mut fgs = Vec::new();
        for y in body.y..body.y + body.height {
            for x in body.x..body.x + body.width {
                let cell = &buf[(x, y)];
                if cell.symbol() != " " {
                    fgs.push(cell.fg);
                }
            }
        }
        fgs
    }

    /// The body is coloured when it is built, so the memo has to die with the
    /// palette: `Theme::apply_kind` writes a static and notifies nobody.
    #[test]
    fn switching_the_palette_repaints_the_usage_body() {
        let _guard = crate::theme::test_guard();
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        root.provide(SESSIONS, sessions.clone()).unwrap();
        let area = Rect::new(0, 0, 97, 24);

        crate::theme::Theme::apply_kind(crate::theme::ThemeKind::GrokNight);
        let night = painted_body_fgs(&root, area);
        crate::theme::Theme::apply_kind(crate::theme::ThemeKind::GrokDay);
        let day = painted_body_fgs(&root, area);
        crate::theme::Theme::apply_kind(crate::theme::ThemeKind::GrokNight);

        assert!(!night.is_empty(), "nothing was painted: {night:?}");
        assert_ne!(
            night, day,
            "the overlay body kept the old palette after a theme switch"
        );
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
    fn mcp_category_zero_tokens_still_listed() {
        let mut snap = snapshot();
        snap.categories.push(ContextCategory {
            label: "MCP 服务器".into(),
            tokens: 0,
            detail: Some("1 台 · 21 个工具 · 未计入窗口".into()),
        });
        let all = all_text(&context_lines(&snap, 80));
        assert!(all.contains("MCP 服务器"), "{all}");
        assert!(all.contains("未计入窗口"), "{all}");
        let view = context_view(&snap, 80, None);
        assert!(
            view.legend.iter().any(|h| h.2 == OccupancyKind::Mcp),
            "{:?}",
            view.legend
        );
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

    fn usage_row(input: u64, cached: u64, created: u64) -> cordis_spine::PromptUsageModel {
        cordis_spine::PromptUsageModel {
            input_tokens: input,
            output_tokens: 100,
            total_tokens: input + 100,
            cached_read_tokens: cached,
            cache_creation_tokens: created,
            reasoning_tokens: 0,
            model_calls: 1,
            api_duration_ms: 1_000,
            cost_usd_ticks: None,
            cost_is_partial: false,
            cost_missing_calls: 0,
            cost_estimated_calls: 0,
        }
    }

    /// 条的长度必须稳定：舍入残余归到最后一个非零段，否则宽度会随比例抖动。
    #[test]
    fn cache_bar_always_fills_the_same_width() {
        let colors = CacheColors::from_theme(&Theme::current());
        for (input, cached, created) in [
            (1_000u64, 333u64, 333u64),
            (1_000, 999, 0),
            (1_000, 0, 0),
            (7, 3, 1),
        ] {
            let line = cache_bar(&usage_row(input, cached, created), &colors, 30);
            let painted: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(
                painted.chars().count(),
                30,
                "input={input} cached={cached} created={created} → {painted:?}"
            );
        }
    }

    /// 非零但很小的段也要占一格：四舍五入到 0 会让"有一小段白付了全价"
    /// 从图上消失，而那正是这条 bar 要显示的东西。
    #[test]
    fn a_tiny_miss_still_gets_a_cell() {
        let colors = CacheColors::from_theme(&Theme::current());
        let line = cache_bar(&usage_row(100_000, 99_950, 0), &colors, 20);
        let miss: usize = line
            .spans
            .iter()
            .filter(|s| s.style.fg == Some(colors.miss.1))
            .map(|s| s.content.chars().count())
            .sum();
        assert!(miss >= 1, "0.05% 的未命中被抹掉了：{line:?}");
    }

    /// 空账本不画条子，直说没有调用。
    #[test]
    fn empty_ledger_says_so_instead_of_drawing_an_empty_bar() {
        let lines = session_view(&cordis_spine::PromptUsage::default(), 60);
        let all = all_text(&lines);
        assert!(all.contains("尚未有模型调用"), "{all}");
        assert!(!all.contains('█'), "{all}");
    }

    /// Session 页要把输入拆成三段，并在有多轮时画出走势。
    #[test]
    fn session_view_breaks_out_cache_and_draws_the_trend() {
        let mut totals = usage_row(131_113, 110_976, 0);
        totals.model_calls = 11;
        let usage = cordis_spine::PromptUsage {
            totals,
            last_call: Some(usage_row(26_742, 18_560, 0)),
            recent_calls: vec![
                usage_row(10_000, 9_500, 0),
                usage_row(20_000, 4_000, 0),
                usage_row(26_742, 18_560, 0),
            ],
            ..Default::default()
        };
        let all = all_text(&session_view(&usage, 60));
        // Claude Code 口径：三个精确数各自成行，「输入」指未命中那部分。
        // 131,113 − 110,976 = 20,137，要能自己算出来，且不缩写成 20.1k。
        assert!(all.contains("输入(未命中)"), "{all}");
        assert!(all.contains("20,137"), "{all}");
        assert!(all.contains("缓存命中"), "{all}");
        assert!(all.contains("110,976"), "{all}");
        assert!(all.contains("100"), "输出缺了：{all}");
        assert!(all.contains("完整输入"), "{all}");
        assert!(all.contains("131,113"), "{all}");
        assert!(!all.contains("131k"), "对账用的数不该缩写：{all}");
        assert!(all.contains("缓存命中 85% of 完整输入"), "{all}");
        // 分段靠字形，不只靠颜色——单色终端和截图里颜色是丢的。
        assert!(all.contains('█') && all.contains('░'), "{all}");
        assert!(all.contains("上一轮"), "{all}");
        assert!(all.contains("8,182"), "上一轮未命中算错：{all}");
        // 中间那轮命中率掉到 20%，走势图必须看得出来。
        assert!(all.contains("每轮命中率"), "{all}");
        // 95% → █，20% → ▂，69% → ▆
        assert!(all.contains("█▂▆"), "走势没按每轮命中率画出来：{all}");
        // 不报写入的 wire 不该凭空多一行；「未命中」那行的注脚里提到写入是另
        // 一回事——它说的正是「写入混在这一段里」。
        assert!(
            !all.contains(CacheSegmentKind::Write.label()),
            "这条 wire 不报写入，不该凭空多一行：{all}"
        );
        assert!(all.contains("含首次写入"), "{all}");
    }

    /// 走势格数和「模型调用」是两个口径（后者折了子代理），并排摆着不写明就
    /// 会被读成走势少画了一格。overlay 必须两边都把数字说出来。
    #[test]
    fn trend_cell_count_is_reconcilable_with_model_calls() {
        let mut totals = usage_row(100_000, 90_000, 0);
        totals.model_calls = 20;
        let usage = cordis_spine::PromptUsage {
            totals,
            num_turns: 19,
            recent_calls: (0..19).map(|_| usage_row(1_000, 900, 0)).collect(),
            ..Default::default()
        };
        let all = all_text(&session_view(&usage, 100));
        assert!(all.contains("最近 19 次主循环调用"), "{all}");
        assert!(all.contains("模型调用 20（主循环 19 · 子代理 1）"), "{all}");
    }

    /// 总计折了子代理与压缩，上一轮 / 走势只有主循环。不把未命中按来源拆开，
    /// 这两个口径并排摆着就会被读成「主循环每轮都在漏 token」。
    #[test]
    fn session_view_attributes_the_miss_to_its_source() {
        let mut totals = usage_row(1_110_000, 998_000, 0);
        totals.model_calls = 12;
        let mut subagent = usage_row(20_000, 0, 0);
        subagent.model_calls = 1;
        let mut side = usage_row(90_000, 0, 0);
        side.model_calls = 1;
        let usage = cordis_spine::PromptUsage {
            totals,
            num_turns: 10,
            subagent: Some(subagent),
            side: Some(side),
            last_call: Some(usage_row(100_000, 99_800, 0)),
            ..Default::default()
        };
        let all = all_text(&session_view(&usage, 100));
        assert!(
            all.contains("未命中来源  主循环 2,000 · 子代理 20,000 · 压缩 90,000"),
            "{all}"
        );
        assert!(
            all.contains("模型调用 12（主循环 10 · 子代理 1 · 压缩 1）"),
            "{all}"
        );
        // 这条 wire 不报写入，未命中里混着「首次写入」，不注明就会被当成
        // Claude Code `/cost` 的 input 口径。
        assert!(all.contains("含首次写入"), "{all}");
    }

    /// 窄窗口下走势要裁最旧的：渲染层只会在右边截断，那样丢的恰好是最新几次。
    #[test]
    fn narrow_overlay_keeps_the_newest_trend_cells() {
        let calls: Vec<_> = (0..40).map(|i| usage_row(1_000, i * 25, 0)).collect();
        let wide = all_text(&session_view(
            &cordis_spine::PromptUsage {
                totals: usage_row(40_000, 20_000, 0),
                recent_calls: calls.clone(),
                ..Default::default()
            },
            120,
        ));
        let narrow = all_text(&session_view(
            &cordis_spine::PromptUsage {
                totals: usage_row(40_000, 20_000, 0),
                recent_calls: calls,
                ..Default::default()
            },
            60,
        ));
        let spark_of = |text: &str| -> String {
            text.lines()
                .find(|l| l.starts_with("每轮命中率"))
                .unwrap()
                .chars()
                .filter(|c| "▁▂▃▄▅▆▇█".contains(*c))
                .collect()
        };
        let (w, n) = (spark_of(&wide), spark_of(&narrow));
        assert!(n.chars().count() < w.chars().count(), "窄窗口没裁：{n}");
        assert!(w.ends_with(&n), "裁掉的该是最旧的：wide={w} narrow={n}");
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
