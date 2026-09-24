//! Status line values. The widget is the copied Grok `StatusBar`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use cordis::Context;
use cordis_spine::{
    snapshot_context, AppSettings, ContextBook, ContextSnapshot, LlmOutput, LogEvent, Sessions,
    TokenUsage, CONTEXT, SESSIONS, SETTINGS,
};
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use crate::names::SESSION_PORT;
use crate::seam::session::SessionRef;
use crate::theme::Theme;
use crate::views::status_bar;
use ratatui::buffer::Buffer;
use ratatui::text::{Line, Span};

const FLASH_TTL: Duration = Duration::from_millis(2500);

struct SnapMemo {
    rev: u64,
    usage: TokenUsage,
    snap: ContextSnapshot,
}

pub struct StatusLine {
    ctx: Context,
    notice: Mutex<Option<(String, Instant)>>,
    right_hit: Mutex<Option<Rect>>,
    snap_memo: Mutex<Option<SnapMemo>>,
    /// 鼠标是否停在右段上。悬停时占用率数字原地换成进度条 + 百分比
    /// （Grok `context_bar_line` 的 hover 分支）。
    right_hover: Mutex<bool>,
    /// 右上角 `[Agents]` chip 的命中区与悬停态。Grok 那边同一位置是
    /// `[Dashboard]`（`agent_view/render.rs` 的 status chips）。
    chip_hit: Mutex<Option<Rect>>,
    chip_hover: Mutex<bool>,
}

impl StatusLine {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            notice: Mutex::new(None),
            right_hit: Mutex::new(None),
            snap_memo: Mutex::new(None),
            right_hover: Mutex::new(false),
            chip_hit: Mutex::new(None),
            chip_hover: Mutex::new(false),
        }
    }

    pub fn flash(&self, msg: impl Into<String>) {
        *self.notice.lock().unwrap() = Some((msg.into(), Instant::now()));
    }

    pub fn clear_notice(&self) {
        *self.notice.lock().unwrap() = None;
    }

    pub fn left(&self) -> String {
        cwd_label(&cordis_spine::session_cwd(&self.ctx))
    }

    pub fn center(&self) -> Option<String> {
        let sessions = self.ctx.require::<Sessions>(SESSIONS).ok()?;
        let turns = sessions
            .events()
            .iter()
            .filter(|e| matches!(e, LogEvent::User(_)))
            .count();
        (turns > 0).then(|| format!("第 {turns} 轮"))
    }

    pub fn right(&self) -> Option<String> {
        {
            let mut notice = self.notice.lock().unwrap();
            if let Some((msg, at)) = notice.as_ref() {
                if at.elapsed() < FLASH_TTL {
                    return Some(msg.clone());
                }
            }
            *notice = None;
        }
        Some(self.resting_label())
    }

    /// flash 过后常驻的右段：上下文占用 + 当前推理协议。
    ///
    /// 协议是 live 的（`/protocol` 随时切、config.toml 热读），所以每帧现算，
    /// 不缓进 `snap_memo`。没有 settings（或还没选模型）时不硬凑一个协议名：
    /// 那时候"当前走哪条 wire"没有意义。
    fn resting_label(&self) -> String {
        let occupancy = occupancy_label(&self.live_snapshot());
        let head = match self.protocol_name() {
            Some(name) => format!("{occupancy} {SEPARATOR} {name}"),
            None => occupancy,
        };
        format!("{head} {SEPARATOR} {DASHBOARD_CHIP}")
    }

    fn protocol_name(&self) -> Option<String> {
        let settings = self.ctx.get::<AppSettings>(SETTINGS)?;
        (!settings.model().trim().is_empty()).then(|| settings.backend().name().to_string())
    }

    /// 常驻右段的多 span 版：占用率数字按百分比渐变，分隔符和协议名压 dim。
    ///
    /// 悬停时把**数字那一段**原地换成进度条 + `42.0%`。两种形态等宽（数字串先
    /// 右补到 `BAR_PCT_GAP + PCT_WIDTH` 列），所以鼠标扫过去整条右段不会抽动，
    /// [`Self::remember_right_hit`] 按 [`Self::right`] 的纯文本算出来的命中区也仍然对得上。
    pub fn right_line(&self) -> Option<Line<'static>> {
        if self.right_is_flash() {
            return None;
        }
        let theme = Theme::current();
        let snap = self.live_snapshot();
        let hovered = *self.right_hover.lock().unwrap();
        let base = Style::default().bg(theme.bg_base);
        let mut spans = self.occupancy_spans(&snap, hovered, &theme);
        if let Some(name) = self.protocol_name() {
            spans.push(Span::styled(
                format!(" {SEPARATOR} "),
                base.fg(theme.gray_dim),
            ));
            spans.push(Span::styled(name, base.fg(theme.gray)));
        }
        spans.push(Span::styled(
            format!(" {SEPARATOR} "),
            base.fg(theme.gray_dim),
        ));
        spans.push(Span::styled(
            DASHBOARD_CHIP.to_string(),
            if *self.chip_hover.lock().unwrap() {
                base.fg(theme.text_primary)
            } else {
                base.fg(theme.gray)
            },
        ));
        Some(Line::from(spans))
    }

    /// 占用率那一段的 spans：抬头 + （数字 | 进度条 + 百分比）。
    ///
    /// 两种形态**等宽**——数字串已经右补到 `BAR_PCT_GAP + PCT_WIDTH`，进度条
    /// 正好吃掉补出来的那部分。右段是右对齐的，宽度一变整条就会横跳。
    fn occupancy_spans(
        &self,
        snap: &ContextSnapshot,
        hovered: bool,
        theme: &Theme,
    ) -> Vec<Span<'static>> {
        let base = Style::default().bg(theme.bg_base);
        let color = occupancy_color(snap.usage_pct, theme);
        let mut spans = vec![Span::styled(
            OCCUPANCY_PREFIX.to_string(),
            base.fg(theme.gray),
        )];
        let (tokens, total_width) = occupancy_tokens(snap);
        if hovered {
            let bar_width = total_width.saturating_sub(BAR_PCT_GAP + PCT_WIDTH);
            spans.extend(crate::grok::progress_bar::progress_bar_spans(
                bar_width as u16,
                snap.usage_pct as f32 / 100.0,
                color,
                theme.bg_highlight,
            ));
            spans.push(Span::styled(" ".to_string(), base));
            spans.push(Span::styled(
                fmt_pct5(snap.usage_pct as f64),
                base.fg(theme.text_secondary),
            ));
        } else {
            spans.push(Span::styled(tokens, base.fg(color)));
        }
        spans
    }

    /// 鼠标位置：切右段两块的悬停态。占用率那块悬停换进度条，chip 悬停提亮。
    pub fn set_mouse(&self, column: u16, row: u16) {
        let on_chip = self.hit_dashboard(column, row);
        *self.chip_hover.lock().unwrap() = on_chip;
        // chip 压在右段最右边，落在它上面时不该同时把占用率也切成进度条。
        *self.right_hover.lock().unwrap() = !on_chip && self.hit_right(column, row);
    }

    pub fn right_is_flash(&self) -> bool {
        self.notice
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() < FLASH_TTL)
    }

    fn live_snapshot(&self) -> ContextSnapshot {
        let (rev, usage) = self
            .ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| s.occupancy_stamp())
            .unwrap_or((0, TokenUsage::default()));
        let mut memo = self.snap_memo.lock().unwrap();
        if let Some(cached) = memo.as_ref() {
            if cached.rev == rev && cached.usage == usage {
                return cached.snap.clone();
            }
        }
        let snap = self
            .ctx
            .get::<ContextBook>(CONTEXT)
            .map(|b| b.window_on(&self.ctx))
            .unwrap_or_else(|| snapshot_context(&self.ctx));
        *memo = Some(SnapMemo {
            rev,
            usage,
            snap: snap.clone(),
        });
        snap
    }

    /// 记下右段的两个可点区：整段（开 `/context`）与末尾的 chip（开 dashboard）。
    ///
    /// chip 在最右边，所以它的矩形就是整段右端那 `DASHBOARD_CHIP` 宽的一截；
    /// 判定时 chip 优先，否则点 chip 会连带把占用 overlay 也开出来。
    pub fn remember_right_hit(&self, area: Rect, text: &str) {
        let full = status_bar::right_rect(area, text);
        *self.right_hit.lock().unwrap() = Some(full);
        let chip_w = (DASHBOARD_CHIP.width() as u16).min(full.width);
        *self.chip_hit.lock().unwrap() = (!self.right_is_flash()).then(|| Rect {
            x: full.x + full.width - chip_w,
            width: chip_w,
            ..full
        });
    }

    /// 点在右上角的 `[Agents]` 上。
    pub fn hit_dashboard(&self, column: u16, row: u16) -> bool {
        self.chip_hit
            .lock()
            .unwrap()
            .is_some_and(|r| r.contains(Position { x: column, y: row }))
    }

    pub fn hit_right(&self, column: u16, row: u16) -> bool {
        self.right_hit
            .lock()
            .unwrap()
            .is_some_and(|r| r.contains(Position { x: column, y: row }))
    }
}

/// 占用率那一段的中文抬头。span 版与纯文本版共用，两边不会各写各的。
pub const OCCUPANCY_PREFIX: &str = "上下文 ";

/// 占用率数字（`121K / 1.0M`）及其占的列数。
///
/// 数字串**右补到 `BAR_PCT_GAP + PCT_WIDTH` 列**（Grok 同款）：悬停态是
/// 「进度条 + 空格 + 5 字符百分比」，至少 6 列；不补的话 `0 / 9` 这种退化输入
/// 默认态只有 5 列，鼠标一进来整条右段就往左跳一格。
fn occupancy_tokens(snap: &ContextSnapshot) -> (String, usize) {
    let mut tokens = format!(
        "{} / {}",
        format_tokens(snap.used),
        format_tokens(snap.total)
    );
    let natural = tokens.chars().count();
    let min_width = BAR_PCT_GAP + PCT_WIDTH;
    if natural < min_width {
        tokens.push_str(&" ".repeat(min_width - natural));
    }
    (tokens, natural.max(min_width))
}

pub fn occupancy_label(snap: &ContextSnapshot) -> String {
    let (tokens, _) = occupancy_tokens(snap);
    format!("{OCCUPANCY_PREFIX}{tokens}")
}

/// 右段里各段之间的分隔符。Grok `context_bar::SEPARATOR`。
pub const SEPARATOR: &str = "│";

/// 右上角打开会话面板的 chip。Grok 同一位置是 `[Dashboard]`。
pub const DASHBOARD_CHIP: &str = "[Agents]";

/// 悬停时百分比字段的宽度（[`fmt_pct5`] 恒为 5 字符）。
const PCT_WIDTH: usize = 5;
/// 悬停时进度条与百分比之间的空隙。
const BAR_PCT_GAP: usize = 1;

/// 定宽 5 字符的百分比。Grok `context_bar::fmt_pct5`。
///
/// `<10` → `5.12%`；`10–99` → `20.1%`；`>=100` → `MAX %`。定宽是为了让悬停态和
/// 默认态等宽——差一列鼠标扫过去整条右段就会抽动。
pub fn fmt_pct5(pct: f64) -> String {
    if pct >= 100.0 {
        "MAX %".to_string()
    } else if pct < 10.0 {
        format!("{pct:.2}%")
    } else {
        format!("{pct:.1}%")
    }
}

/// 占用率配色的断点。Grok `context_bar::default_breakpoints`。
fn occupancy_breakpoints(theme: &Theme) -> [(f64, ratatui::style::Color); 6] {
    [
        (0.0, theme.text_primary),
        (50.0, theme.accent_user),
        (65.0, theme.accent_user),
        (75.0, theme.warning),
        (85.0, theme.warning),
        (95.0, theme.accent_error),
    ]
}

/// 在断点之间线性插值出占用率的颜色。Grok `context_bar::blend_color`。
///
/// 原来是 70 / 85 两档硬跳：85% 是自动压缩阈值，可跳到那一档时已经没有预告了。
/// 渐变让「快满了」这件事在撞线之前就看得出来。
fn blend_occupancy_color(
    pct: f64,
    points: &[(f64, ratatui::style::Color)],
) -> ratatui::style::Color {
    let Some(&(first_pct, first_color)) = points.first() else {
        return ratatui::style::Color::Reset;
    };
    if pct <= first_pct {
        return first_color;
    }
    for w in points.windows(2) {
        let [(prev_pct, prev_c), (next_pct, next_c)] = w else {
            continue;
        };
        if pct <= *next_pct {
            let t = ((pct - prev_pct) / (next_pct - prev_pct)) as f32;
            return lerp_color(*prev_c, *next_c, t);
        }
    }
    points
        .last()
        .map(|p| p.1)
        .unwrap_or(ratatui::style::Color::Reset)
}

/// 两个颜色之间线性插值。非 RGB 的（`Reset` / 具名 ANSI）不插值，过半就跳——
/// 低色终端上插出来的中间色不在调色板里，反而更难读。
fn lerp_color(a: ratatui::style::Color, b: ratatui::style::Color, t: f32) -> ratatui::style::Color {
    use ratatui::style::Color;
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return if t < 0.5 { a } else { b };
    };
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

fn occupancy_color(pct: u8, theme: &Theme) -> ratatui::style::Color {
    blend_occupancy_color(pct as f64, &occupancy_breakpoints(theme))
}

/// `~/Desktop/AILab/dock` 那样的紧凑 cwd（`ctx` 所在那页的）。面板抬头也用它，
/// 保持和状态栏一致。
pub fn cwd_display(ctx: &Context) -> String {
    cwd_label(&cordis_spine::session_cwd(ctx))
}

fn cwd_label(cwd: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if let Some(home) = home {
        if let Ok(rel) = cwd.strip_prefix(&home) {
            let rel = rel.to_string_lossy();
            return if rel.is_empty() {
                "~".into()
            } else {
                format!("~/{rel}")
            };
        }
    }
    cwd.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into())
}

/// 紧凑 token 计数，**至多 4 个字符**。Grok `context_bar::fmt_tokens` 同款。
///
/// `0–999` → `0` / `999`；`1K–9.9K` → `1.2K`；`10K–999K` → `12K` / `999K`；
/// `1M–9.9M` → `1.2M`；`10M+` → `12M`。
///
/// 旧实现给到 6 个字符（`1.00m`、`20.0k` 且小写），顶栏右段本来就要和 cwd、轮次
/// 抢宽度，多出来的两列真的会把后面的协议名挤掉。四字符上限由
/// `format_tokens_is_at_most_four_chars` 守着。
pub fn format_tokens(n: u64) -> String {
    /// `9_960` 这类值 `{:.1}` 会进位到 `10.0`，连单位就是 5 个字符——破了上限。
    /// 进位到头就改用整数形（`10K`），意思一样且只占 3 列。Grok 的 `fmt_tokens`
    /// 没处理这个边界（它自己的测试断言 `fmt_tokens(9_960) == "10.0K"`），照抄
    /// 会把宽度预算漏掉。
    fn one_decimal(value: f64, unit: char) -> String {
        if value >= 9.95 {
            format!("10{unit}")
        } else {
            format!("{value:.1}{unit}")
        }
    }
    if n < 1_000 {
        n.to_string()
    } else if n < 10_000 {
        one_decimal(n as f64 / 1_000.0, 'K')
    } else if n < 1_000_000 {
        format!("{}K", n / 1_000)
    } else if n < 10_000_000 {
        one_decimal(n as f64 / 1_000_000.0, 'M')
    } else {
        format!("{}M", n / 1_000_000)
    }
}

pub fn format_duration_short(d: std::time::Duration) -> String {
    let secs = d.as_secs_f32();
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else if secs < 60.0 {
        format!("{}s", secs.round() as u32)
    } else {
        let m = (secs / 60.0).floor() as u32;
        let s = (secs % 60.0).round() as u32;
        format!("{m}m{s:02}s")
    }
}

/// Show each braille spinner frame for this many shimmer ticks (~7.5fps at 30Hz).
const SPINNER_DIVISOR: u64 = 4;

/// Only while a turn is running or a permission prompt is open.
/// Idle "就绪" is not a Grok chrome row — it sat on top of the user block.
pub fn turn_status_height(ctx: &Context, waiting_permission: bool) -> u16 {
    if waiting_permission {
        return 1;
    }
    u16::from(
        ctx.get::<SessionRef>(SESSION_PORT)
            .is_some_and(|h| h.working()),
    )
}

/// Grok turn-status row: `⠧ Waiting…` … `↓15.7k [stop]`.
/// Elapsed generation time lives on the prompt chrome bottom border.
pub fn render_turn_status(buf: &mut Buffer, area: Rect, ctx: &Context, waiting_permission: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let theme = Theme::current();
    let base = Style::default().fg(theme.gray).bg(theme.bg_base);
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.reset();
            cell.set_style(base);
        }
    }
    let working = ctx
        .get::<SessionRef>(SESSION_PORT)
        .is_some_and(|h| h.working());
    if !waiting_permission && !working {
        return;
    }
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return;
    };
    let (label, label_style) =
        sessions.with_log(|events, _| turn_activity_label(events, waiting_permission, &theme));
    let queued = ctx
        .get::<SessionRef>(SESSION_PORT)
        .map(|s| s.queued_prompts().len())
        .unwrap_or(0);
    let can_send = ctx
        .get::<crate::views::prompt::PromptWidget>(crate::names::TUI_PROMPT)
        .is_some_and(|p| p.can_send());
    let queue_hint = if queued == 0 {
        String::new()
    } else if !can_send {
        format!(" · {queued} queued, Enter to send now")
    } else {
        format!(" · {queued} queued")
    };

    let frames = crate::grok::glyphs::braille_spinner_frames();
    let tick = crate::grok::color::shimmer_tick() / SPINNER_DIVISOR;
    let spinner = frames[(tick as usize) % frames.len()];
    let spinner_style = if waiting_permission {
        Style::default().fg(theme.warning).bg(theme.bg_base)
    } else {
        label_style
    };

    let usage = sessions.usage();
    let down = crate::grok::glyphs::token_down();
    let tokens = format!("{down}{}", format_tokens(usage.completion));
    let stop = "[stop]";
    let right_w = tokens.width() + 1 + stop.width();
    let budget = area.width as usize;
    let gap = 2usize;
    let right_budget = if right_w + gap < budget {
        budget - right_w - gap
    } else {
        budget
    };

    let spin = format!("{spinner} ");
    let mut label = label;
    let mut hint = queue_hint;
    let mut left_w = spin.width() + label.width() + hint.width();
    if left_w > right_budget {
        hint.clear();
        left_w = spin.width() + label.width();
    }
    if left_w > right_budget {
        label = short_activity_label(waiting_permission);
    }
    let mut left_spans = vec![
        Span::styled(spin, spinner_style),
        Span::styled(label, label_style),
    ];
    if !hint.is_empty() {
        left_spans.push(Span::styled(hint, base));
    }

    buf.set_line(area.x, area.y, &Line::from(left_spans), area.width);

    if right_w < budget {
        let x = area.x + area.width.saturating_sub(right_w as u16);
        buf.set_line(
            x,
            area.y,
            &Line::from(vec![
                Span::styled(tokens, base),
                Span::styled(" ", base),
                Span::styled(
                    stop,
                    Style::default().fg(theme.accent_error).bg(theme.bg_base),
                ),
            ]),
            right_w as u16,
        );
    }
}

fn short_activity_label(waiting_permission: bool) -> String {
    if waiting_permission {
        "等待授权".into()
    } else {
        "生成中…".into()
    }
}

fn turn_activity_label(
    events: &[LogEvent],
    waiting_permission: bool,
    theme: &Theme,
) -> (String, Style) {
    let secondary = Style::default().fg(theme.text_secondary).bg(theme.bg_base);
    let tool = Style::default().fg(theme.accent_success).bg(theme.bg_base);
    let warn = Style::default().fg(theme.warning).bg(theme.bg_base);

    if waiting_permission {
        return ("等待授权".into(), warn);
    }

    match events.last() {
        Some(LogEvent::LlmStream(out)) => {
            if let Some(name) = pending_tool_name(events, out) {
                return (format!("运行 {name}…"), tool);
            }
            if !out.text.trim().is_empty() {
                return ("生成中…".into(), secondary);
            }
            if !out.reasoning.trim().is_empty() {
                return ("思考中…".into(), secondary);
            }
            ("等待响应…".into(), secondary)
        }
        Some(LogEvent::ToolExecute { name, .. }) => {
            // Between tool result and the next sample.
            let _ = name;
            ("等待响应…".into(), secondary)
        }
        _ => ("等待响应…".into(), secondary),
    }
}

fn pending_tool_name(events: &[LogEvent], out: &LlmOutput) -> Option<String> {
    for call in out.tool_calls.iter().rev() {
        let done = events
            .iter()
            .any(|e| matches!(e, LogEvent::ToolExecute { id, .. } if id == &call.id));
        if !done {
            return Some(call.name.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_compact() {
        assert_eq!(format_tokens(42), "42");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_230), "1.2K");
        assert_eq!(format_tokens(9_960), "10K");
        assert_eq!(format_tokens(9_940), "9.9K");
        assert_eq!(format_tokens(9_990_000), "10M");
        assert_eq!(format_tokens(12_300), "12K");
        assert_eq!(format_tokens(999_000), "999K");
        assert_eq!(format_tokens(1_200_000), "1.2M");
        assert_eq!(format_tokens(12_000_000), "12M");
    }

    /// `ContextSnapshot` 没有 `Default`（给 spine 的 pub 类型加 derive 是 public
    /// API 面的事），这里照字段列全。
    fn snap(used: u64, total: u64, pct: u8) -> ContextSnapshot {
        ContextSnapshot {
            used,
            total,
            model: String::new(),
            system_prompt_tokens: 0,
            message_tokens: 0,
            tool_definitions_tokens: 0,
            tool_definitions_count: 0,
            free_tokens: total.saturating_sub(used),
            usage_pct: pct,
            auto_compact_threshold_percent: 85,
            turn_count: 0,
            tool_call_count: 0,
            compaction_count: 0,
            categories: Vec::new(),
        }
    }

    fn line_width(line: &Line<'static>) -> usize {
        line.spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum()
    }

    /// 悬停态和默认态必须等宽：右段是右对齐的，差一列鼠标扫过去整条就会横跳，
    /// 而且 `remember_right_hit` 按默认态的纯文本算命中区，宽度一变命中区就错位。
    #[test]
    fn hovered_occupancy_is_the_same_width_as_the_number() {
        let theme = Theme::current();
        for (used, total) in [
            (8_500u64, 1_000_000u64),
            (500, 1_000_000),
            (121_000, 1_000_000),
            (0, 9),
            (999_999, 999_999),
        ] {
            let ctx = Context::new();
            let line = StatusLine::new(ctx);
            let snap = snap(used, total, 42);
            let resting = line.occupancy_spans(&snap, false, &theme);
            let hovered = line.occupancy_spans(&snap, true, &theme);
            assert_eq!(
                line_width(&Line::from(resting)),
                line_width(&Line::from(hovered)),
                "used={used} total={total}",
            );
        }
    }

    /// 悬停时数字换成进度条 + 百分比，抬头和协议名不动。
    #[test]
    fn hover_swaps_the_number_for_a_bar_and_percentage() {
        let theme = Theme::current();
        let line = StatusLine::new(Context::new());
        let snap = snap(420_000, 1_000_000, 42);
        let text: String = line
            .occupancy_spans(&snap, true, &theme)
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(text.ends_with("42.0%"), "text={text:?}");
        assert!(!text.contains("420K"), "悬停态不该再画数字: {text:?}");
    }

    /// 占用率颜色是**渐变**的：原来 70/85 两档硬跳，85% 正好是自动压缩阈值，
    /// 跳到那一档时已经没有预告余地了。中间几档必须各不相同。
    #[test]
    fn occupancy_color_ramps_instead_of_stepping() {
        let theme = Theme::groknight();
        let colors: Vec<_> = [0u8, 30, 55, 70, 80, 90, 100]
            .into_iter()
            .map(|pct| occupancy_color(pct, &theme))
            .collect();
        assert_eq!(colors[0], theme.text_primary, "0% 是正常文本色");
        assert_eq!(colors[6], theme.accent_error, "满了是错误色");
        assert_ne!(colors[1], colors[0], "30% 已经在往 accent_user 过渡");
        assert_ne!(colors[4], colors[5], "80% 与 90% 不能撞成同一档");
    }

    /// 四字符上限是顶栏右段的宽度预算前提。旧格式最长到 `1.00m` / `20.0k`
    /// 六个字符，顶栏一窄就把后面的协议名挤掉。
    #[test]
    fn format_tokens_is_at_most_four_chars() {
        for n in [
            0,
            1,
            999,
            1_000,
            1_200,
            9_900,
            9_960,
            12_000,
            999_000,
            1_000_000,
            1_200_000,
            9_900_000,
            12_000_000,
            123_000_000,
        ] {
            let s = format_tokens(n);
            assert!(s.chars().count() <= 4, "format_tokens({n}) = {s:?}");
        }
    }

    #[test]
    fn cwd_uses_tilde_when_under_home() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let cwd = std::path::Path::new(&home).join("proj");
        assert_eq!(cwd_label(&cwd), "~/proj");
    }

    /// 回归：状态栏显示的是**这一页**钉住的 cwd，不是进程 cwd——两页在不同
    /// 项目时，各自的状态栏要写各自的目录。
    #[tokio::test]
    async fn left_shows_the_page_cwd() {
        let ctx = Context::new();
        let sessions = Sessions::tab(ctx.clone(), 2);
        sessions.pin_workspace_cwd("/work/page-two-project");
        let _reg = ctx.provide(SESSIONS, sessions).unwrap();
        let line = StatusLine::new(ctx);
        assert_eq!(line.left(), "page-two-project");
    }

    #[test]
    fn left_is_cwd_only() {
        let line = StatusLine::new(Context::new());
        assert!(!line.left().contains("上下文"), "left={}", line.left());
    }

    #[test]
    fn right_shows_occupancy_without_flash() {
        let line = StatusLine::new(Context::new());
        let right = line.right().expect("occupancy");
        assert!(right.starts_with("上下文 "), "right={right}");
        assert!(right.contains('/'), "right={right}");
    }

    /// 右段常驻带上当前协议：`/protocol` 切完、flash 过期后仍然看得见走的是哪条
    /// wire，不用再敲一次 `/protocol`。
    #[test]
    fn right_keeps_the_current_protocol() {
        let ctx = Context::new();
        let _svc = ctx
            .provide(SETTINGS, AppSettings::new("not-in-any-catalog"))
            .unwrap();
        let line = StatusLine::new(ctx);
        let right = line.right().expect("occupancy");
        assert!(right.starts_with("上下文 "), "right={right}");
        assert!(right.contains(" │ responses │ "), "right={right}");
        // chip 恒在最右：它是打开会话面板的可点区，`remember_right_hit` 靠
        // 「右段末尾 DASHBOARD_CHIP 宽」算它的矩形，位置一变命中区就错位。
        assert!(right.ends_with(DASHBOARD_CHIP), "right={right}");
    }

    /// chip 的命中区要正好盖住 `[Agents]` 那几列，且**不**盖到占用率那段——
    /// 它俩点开的是两个不同的东西（面板 / `/context`），错一列就点错。
    #[test]
    fn the_chip_hit_covers_only_the_chip() {
        let ctx = Context::new();
        let _svc = ctx
            .provide(SETTINGS, AppSettings::new("not-in-any-catalog"))
            .unwrap();
        let line = StatusLine::new(ctx);
        let right = line.right().expect("occupancy");
        let area = Rect::new(0, 0, 100, 1);
        line.remember_right_hit(area, &right);

        let chip_w = DASHBOARD_CHIP.width() as u16;
        let chip_x = 100 - chip_w;
        assert!(line.hit_dashboard(chip_x, 0), "chip 左端要命中");
        assert!(line.hit_dashboard(99, 0), "chip 右端要命中");
        assert!(
            !line.hit_dashboard(chip_x - 1, 0),
            "再往左一列是协议名，不该算 chip"
        );
        // 占用率那段仍然可点（开 /context），只是不包含 chip。
        assert!(line.hit_right(chip_x - 1, 0));
    }

    /// flash 期间没有 chip：那一刻右段是一句提示文案，末尾几列不是按钮，
    /// 留着命中区就会点到一个看不见的东西。
    #[test]
    fn a_flash_clears_the_chip_hit() {
        let ctx = Context::new();
        let line = StatusLine::new(ctx);
        line.flash("协议 messages");
        let right = line.right().expect("flash");
        line.remember_right_hit(Rect::new(0, 0, 100, 1), &right);
        assert!(!line.hit_dashboard(99, 0));
    }

    /// flash 期间右段是 flash 文案，不被常驻协议拼接盖掉。
    #[test]
    fn flash_wins_over_the_resting_label() {
        let ctx = Context::new();
        let _svc = ctx
            .provide(SETTINGS, AppSettings::new("not-in-any-catalog"))
            .unwrap();
        let line = StatusLine::new(ctx);
        line.flash("协议 messages");
        assert_eq!(line.right().as_deref(), Some("协议 messages"));
    }

    /// 没有 settings，或模型还没选（空 id）时，右段不写协议名——别硬凑。
    /// chip 不受这个影响，它跟模型没关系。
    #[test]
    fn no_model_means_no_protocol_suffix() {
        let strip_chip = |s: &str| {
            s.trim_end()
                .trim_end_matches(DASHBOARD_CHIP)
                .trim_end()
                .trim_end_matches(SEPARATOR)
                .to_string()
        };
        let right = StatusLine::new(Context::new()).right().expect("occupancy");
        assert!(right.ends_with(DASHBOARD_CHIP), "right={right}");
        assert!(!strip_chip(&right).contains(SEPARATOR), "right={right}");

        let ctx = Context::new();
        let _svc = ctx.provide(SETTINGS, AppSettings::new("")).unwrap();
        let right = StatusLine::new(ctx).right().expect("occupancy");
        assert!(!strip_chip(&right).contains(SEPARATOR), "right={right}");
    }

    #[test]
    fn occupancy_label_formats_window() {
        let snap = ContextSnapshot {
            used: 20_000,
            total: 204_800,
            model: String::new(),
            system_prompt_tokens: 0,
            message_tokens: 0,
            tool_definitions_tokens: 0,
            tool_definitions_count: 0,
            free_tokens: 184_800,
            usage_pct: 10,
            auto_compact_threshold_percent: 85,
            turn_count: 0,
            tool_call_count: 0,
            compaction_count: 0,
            categories: Vec::new(),
        };
        assert_eq!(occupancy_label(&snap), "上下文 20K / 204K");
    }

    #[test]
    fn activity_waiting_before_stream() {
        let theme = Theme::current();
        let (label, _) = turn_activity_label(&[LogEvent::User("hi".into())], false, &theme);
        assert_eq!(label, "等待响应…");
    }

    #[test]
    fn activity_thinking_and_responding() {
        let theme = Theme::current();
        let (think, _) = turn_activity_label(
            &[LogEvent::LlmStream(LlmOutput {
                reasoning: "hmm".into(),
                ..LlmOutput::default()
            })],
            false,
            &theme,
        );
        assert_eq!(think, "思考中…");
        let (gen, _) = turn_activity_label(
            &[LogEvent::LlmStream(LlmOutput {
                text: "hello".into(),
                ..LlmOutput::default()
            })],
            false,
            &theme,
        );
        assert_eq!(gen, "生成中…");
    }

    #[test]
    fn activity_pending_tool() {
        let theme = Theme::current();
        let (label, _) = turn_activity_label(
            &[LogEvent::LlmStream(LlmOutput {
                tool_calls: vec![cordis_spine::ToolCall {
                    id: "1".into(),
                    name: "bash".into(),
                    arguments: "{}".into(),
                }],
                ..LlmOutput::default()
            })],
            false,
            &theme,
        );
        assert_eq!(label, "运行 bash…");
    }
}
