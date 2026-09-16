//! 会话面板：**同屏观察并驱动多个会话**。
//!
//! 对应 Grok pager 的 `views/dashboard/`。那边一行 "agent" 就是**一个会话**，
//! 和 dock 的一页（`tabs.rs`）是同一个东西。子代理不在这里，归 `/tasks`。
//!
//! ## 和 `/resume` 的分工
//!
//! `/resume` 是「挑一个历史会话恢复到当前页」——一次性的选择器，选完就没了。
//! 这里是**工作面板**：上半屏列出所有会话（开着的 + 磁盘上的），下半屏是选中
//! 那个会话的实时尾巴加一个输入框，**Tab 进输入框打字回车就把消息发进那一个
//! 会话，不切页、不离开面板**。所以它能盯着三个会话轮流派活，`/resume` 不能。
//!
//! 驱动走的是每页自己的 `"session.port"`（[`SessionRef::submit`]）——分页本来
//! 就一页一棵 isolate 子树，各有各的会话与循环，面板只是换个地方按下回车。
//!
//! | 来源 | 服务 | 说明 |
//! |---|---|---|
//! | 分页 | `"tui.tabs"` | 本进程开着的会话，能观察也能驱动 |
//! | 磁盘会话 | `"roster"` | 跨 cwd 的历史会话，只能看和恢复 |
//!
//! ## 为什么不是带框的 overlay
//!
//! 画成**占满整屏的独立面板**（自己的抬头行、动作行、分组列表、peek 面板、
//! 底部快捷键条），不套 `render_fullscreen_frame` 那个边框——边框会让它读起来
//! 像「弹出来的一张表」，而它是个要长时间待着的工作面。Grok 那边同理，是
//! `ActiveView::AgentDashboard` 而不是 modal。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use cordis::Context;
use cordis_spine::{Roster, RosterEntry, Sessions, ROSTER, SESSIONS};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget};
use unicode_width::UnicodeWidthStr;

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_str;
use crate::grok::picker::{render_bordered_frame, render_header, PickerHits};
use crate::names::{SESSION_PORT, TUI_TABS};
use crate::seam::session::SessionRef;
use crate::seam::tabs::{TabKind, Tabs};
use crate::theme::Theme;

/// 键盘焦点：列表还是 peek 的输入框。Tab 切换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// 上半屏。↑↓ 选行，打字进搜索框，Enter 打开选中的会话。
    #[default]
    List,
    /// 下半屏的输入框。打字进输入框，Enter 把消息发给**选中的**会话。
    Composer,
}

/// 分组键。顺序即渲染顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RowState {
    /// 这一页正在生成。
    Working,
    /// 开着但没在跑。
    Idle,
    /// 只在磁盘上，没开着。
    Archived,
}

impl RowState {
    pub fn label(self) -> &'static str {
        match self {
            RowState::Working => "进行中",
            RowState::Idle => "空闲",
            RowState::Archived => "历史",
        }
    }
}

/// dashboard 的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashRow {
    /// 可折叠的分组抬头。选中按 Enter 折叠 / 展开。
    Header { state: RowState, count: usize },
    /// 本进程开着的一页。
    Tab {
        /// 标签上的稳定编号。
        id: usize,
        title: String,
        active: bool,
        working: bool,
    },
    /// 磁盘上的会话。
    Archived {
        id: String,
        title: String,
        summary: String,
        cwd: PathBuf,
        /// 是否就在当前工作目录下。不是的话 Enter 恢复不了，见
        /// [`DashRow::resumable`]。
        here: bool,
        updated: SystemTime,
    },
}

impl DashRow {
    /// 抬头行不可选中（但可以落焦点去折叠），其余都能选。
    pub fn is_header(&self) -> bool {
        matches!(self, DashRow::Header { .. })
    }

    /// 这一行属于哪个分组。
    pub fn state(&self) -> RowState {
        match self {
            DashRow::Header { state, .. } => *state,
            DashRow::Tab { working, .. } => {
                if *working {
                    RowState::Working
                } else {
                    RowState::Idle
                }
            }
            DashRow::Archived { .. } => RowState::Archived,
        }
    }

    /// 磁盘会话只有在**当前工作目录**下才能直接恢复。
    ///
    /// `Sessions::restore` 是从 `archived()` 里找的，而那份列表由
    /// `attach_disk` → `load_cwd(当前 cwd)` 填——换个目录的会话根本不在里面。
    /// 跨目录恢复要连带切工作目录，那是另一个决定，不在这一版里。
    pub fn resumable(&self) -> bool {
        match self {
            DashRow::Archived { here, .. } => *here,
            _ => false,
        }
    }

    /// 搜索匹配的文本面。
    fn haystack(&self) -> String {
        match self {
            DashRow::Header { .. } => String::new(),
            DashRow::Tab { id, title, .. } => format!("{id} {title}"),
            DashRow::Archived {
                title,
                summary,
                cwd,
                ..
            } => format!("{title} {summary} {}", cwd.display()),
        }
    }
}

/// 一行下面那条 dim 的副行。空串表示这一行只占一行高。
pub fn secondary_line(row: &DashRow) -> String {
    match row {
        DashRow::Header { .. } => String::new(),
        DashRow::Tab { .. } => String::new(),
        DashRow::Archived { summary, .. } => summary.clone(),
    }
}

/// 把两个来源合成一张按状态分组的列表。
///
/// `collapsed` 里的分组只留抬头。抬头上的计数是**过滤后**的条数——搜的时候看到
/// 「空闲 12」底下却只有 2 行会以为界面坏了。
pub fn build_rows(ctx: &Context, query: &str, collapsed: &HashSet<RowState>) -> Vec<DashRow> {
    shape(collect_rows(ctx), query, collapsed)
}

/// [`build_rows`] 里不碰 ctx 的那一半：过滤 + 分组 + 插抬头。
///
/// 单独拆出来是为了能直接喂行进去测——从磁盘和分页那头构造数据要落盘、要改
/// `DOCK_HOME` 和进程 cwd（都是进程级全局，跨用例会互相踩），而那条路在
/// `cordis-spine` 的 roster 用例里已经盖过了。
fn shape(mut rows: Vec<DashRow>, query: &str, collapsed: &HashSet<RowState>) -> Vec<DashRow> {
    let needle = query.trim().to_lowercase();
    if !needle.is_empty() {
        rows.retain(|r| r.haystack().to_lowercase().contains(&needle));
    }
    group(rows, collapsed)
}

fn collect_rows(ctx: &Context) -> Vec<DashRow> {
    let mut rows: Vec<DashRow> = Vec::new();

    // 分页。`/btw` 的旁问页不进标签栏，这里同样不列——它是问完就销毁的临时页。
    if let Some(tabs) = ctx.get::<Tabs>(TUI_TABS) {
        rows.extend(
            tabs.list()
                .into_iter()
                .filter(|t| t.kind == TabKind::Normal)
                .map(|t| DashRow::Tab {
                    id: t.id,
                    title: t.title,
                    active: t.active,
                    working: t.working,
                }),
        );
    }

    // 磁盘会话。已经开着的那一个不重复列——它在上面已经是一行 Tab 了。
    if let Some(roster) = ctx.get::<Roster>(ROSTER) {
        let open = open_session_ids(ctx);
        let here = std::env::current_dir().unwrap_or_default();
        rows.extend(
            roster
                .list()
                .into_iter()
                .filter(|e| !open.contains(&e.id))
                .map(|e| archived_row(e, &here)),
        );
    }
    rows
}

fn archived_row(entry: RosterEntry, here: &std::path::Path) -> DashRow {
    DashRow::Archived {
        here: entry.cwd == here,
        id: entry.id,
        title: entry.title,
        summary: entry.summary,
        cwd: entry.cwd,
        updated: entry.updated,
    }
}

/// 当前进程里开着的会话 id。用来把名册里的同一条去重。
fn open_session_ids(ctx: &Context) -> HashSet<String> {
    let mut ids = HashSet::new();
    let mut note = |c: &Context| {
        if let Some(sessions) = c.get::<Sessions>(SESSIONS) {
            let id = sessions.live_session_id();
            if !id.is_empty() {
                ids.insert(id);
            }
        }
    };
    match ctx.get::<Tabs>(TUI_TABS) {
        Some(tabs) => {
            for c in tabs.contexts() {
                note(&c);
            }
        }
        None => note(ctx),
    }
    ids
}

/// 按状态分组、插抬头。组内：分页按编号，磁盘会话按时间新到旧。
fn group(rows: Vec<DashRow>, collapsed: &HashSet<RowState>) -> Vec<DashRow> {
    let mut out = Vec::new();
    for state in [RowState::Working, RowState::Idle, RowState::Archived] {
        let mut bucket: Vec<DashRow> = rows
            .iter()
            .filter(|r| r.state() == state)
            .cloned()
            .collect();
        if bucket.is_empty() {
            continue;
        }
        bucket.sort_by_key(rank);
        out.push(DashRow::Header {
            state,
            count: bucket.len(),
        });
        if !collapsed.contains(&state) {
            out.extend(bucket);
        }
    }
    out
}

/// 组内排序键：开着的分页在前，磁盘会话在后（按时间新到旧）。
fn rank(row: &DashRow) -> (u8, std::cmp::Reverse<u64>, String) {
    match row {
        DashRow::Header { .. } => (0, std::cmp::Reverse(0), String::new()),
        DashRow::Tab { id, .. } => (1, std::cmp::Reverse(0), format!("{id:04}")),
        DashRow::Archived { updated, id, .. } => (
            2,
            std::cmp::Reverse(
                updated
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            ),
            id.clone(),
        ),
    }
}

/// `selected` 落在抬头之外的第一行上。空列表返回 0。
pub fn first_selectable(rows: &[DashRow]) -> usize {
    rows.iter().position(|r| !r.is_header()).unwrap_or(0)
}

/// 一行占的高度：有副行就两行。抬头恒一行。
fn row_height(row: &DashRow) -> u16 {
    if secondary_line(row).is_empty() {
        1
    } else {
        2
    }
}

/// 右侧的时间 / 状态列。
fn right_label(row: &DashRow) -> String {
    match row {
        DashRow::Header { .. } => String::new(),
        DashRow::Tab { active, .. } => {
            if *active {
                "当前".into()
            } else {
                String::new()
            }
        }
        DashRow::Archived { updated, here, .. } => {
            let age = age_label(*updated);
            if *here {
                age
            } else if age.is_empty() {
                "其它目录".into()
            } else {
                format!("{age} · 其它目录")
            }
        }
    }
}

/// `4h` / `7d` 那种紧凑时距。Grok dashboard 行尾同款。
fn age_label(at: SystemTime) -> String {
    let Ok(elapsed) = SystemTime::now().duration_since(at) else {
        return String::new();
    };
    let secs = elapsed.as_secs();
    if secs < 60 {
        "刚刚".into()
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// 行首的状态符。子代理缩进一级。
fn row_prefix(row: &DashRow) -> String {
    match row {
        DashRow::Header { .. } => String::new(),
        DashRow::Tab { working, .. } => {
            if *working {
                format!("{} ", glyphs::filled_dot())
            } else {
                format!("{} ", glyphs::hollow_dot())
            }
        }
        DashRow::Archived { .. } => format!("{} ", glyphs::diamond_filled()),
    }
}

/// 面板的四段布局。上到下：抬头 / 动作行 / 列表 / peek / 快捷键条。
struct Panes {
    header: Rect,
    actions: Rect,
    list: Rect,
    peek: Option<Rect>,
    hints: Rect,
}

/// peek 占屏高的比例（分子 / 分母）。Grok 的 peek 大约占四成——它是要读内容的，
/// 给九行只够看到半句话。
const PEEK_NUM: u16 = 2;
const PEEK_DEN: u16 = 5;
/// peek 的上下限（含边框）。下限之下不如不画，上限之上列表就没地方了。
const PEEK_MIN_HEIGHT: u16 = 10;
const PEEK_MAX_HEIGHT: u16 = 24;
/// 无论如何要给列表留的行数。
const LIST_MIN_HEIGHT: u16 = 5;
/// 抬头 + 动作行 + 空行 + 底部快捷键条。
const CHROME_HEIGHT: u16 = 4;

/// 画得下 peek 的最矮终端：列表留够 + peek 够下限。
///
/// `layout` 自己不读它（阈值是算出来的，不是比出来的），只有测试拿它当边界。
/// 另设一个运行时常量反而会和算法各写一套、迟早对不上。
#[cfg(test)]
const PEEK_MIN_TOTAL: u16 = CHROME_HEIGHT + LIST_MIN_HEIGHT + PEEK_MIN_HEIGHT;

fn layout(area: Rect) -> Panes {
    let room = area.height.saturating_sub(CHROME_HEIGHT + LIST_MIN_HEIGHT);
    let peek_h = (area.height * PEEK_NUM / PEEK_DEN)
        .clamp(PEEK_MIN_HEIGHT, PEEK_MAX_HEIGHT)
        .min(room);
    // 挤不出下限就整块让掉——peek 剩四五行只能看到半句话，不如把地方给列表。
    let peek_h = if peek_h < PEEK_MIN_HEIGHT { 0 } else { peek_h };
    let header = Rect { height: 1, ..area };
    let actions = Rect {
        y: area.y + 1,
        height: 1,
        ..area
    };
    let hints = Rect {
        y: area.y + area.height - 1,
        height: 1,
        ..area
    };
    let list_y = area.y + 3;
    let list_h = area.height.saturating_sub(3 + peek_h + 1).max(1);
    Panes {
        header,
        actions,
        list: Rect {
            y: list_y,
            height: list_h,
            ..area
        },
        peek: (peek_h > 0).then(|| Rect {
            y: list_y + list_h,
            height: peek_h,
            ..area
        }),
        hints,
    }
}

/// 面板要画的东西。调用点先备好，渲染只管画。
pub struct PanelView<'a> {
    pub rows: &'a [DashRow],
    pub selected: usize,
    pub query: &'a str,
    pub focus: Focus,
    pub composer: &'a str,
    /// 选中会话的尾巴（已经渲染好的行）。
    pub peek_lines: Vec<Line<'static>>,
    /// peek 面板的抬头，例如 `重构状态栏 · 进行中`。
    pub peek_title: String,
    /// peek 抬头右上角的时距（历史会话才有）。
    pub peek_age: String,
    /// 右上角的汇总，例如 `◇ 2 空闲`。
    pub summary: String,
}

/// 画整块面板。返回可点区域（行 + 关闭按钮留空）。
pub fn render_panel(buf: &mut Buffer, area: Rect, view: &PanelView<'_>) -> PickerHits {
    let theme = Theme::current();
    if area.height < 6 || area.width < 20 {
        return PickerHits::default();
    }
    // **先 Clear 再铺底色**：`set_style` 只改样式、不动字符，底下那一屏的字会
    // 原样透上来（状态栏、滚动区正文、输入框全糊在一起）。`render_bordered_frame`
    // 同样是先 `Clear.render` 再 set_style，这里是同一条规矩。
    Clear.render(area, buf);
    buf.set_style(area, Style::default().bg(theme.bg_base));
    let panes = layout(area);
    let mut hits = PickerHits::default();

    paint_header(buf, panes.header, &theme, &view.summary);
    paint_actions(buf, panes.actions, &theme, view.query, view.focus);
    hits.rows = paint_list(buf, panes.list, &theme, view);
    if let Some(peek) = panes.peek {
        paint_peek(buf, peek, &theme, view);
    }
    paint_hints(buf, panes.hints, &theme, view.focus);
    hits
}

/// `main ~/Desktop/AILab/dock                    ◇ 2 空闲`
fn paint_header(buf: &mut Buffer, area: Rect, theme: &Theme, summary: &str) {
    let base = Style::default().bg(theme.bg_base);
    let cwd = crate::views::status::cwd_display();
    let left = truncate_str(&cwd, area.width.saturating_sub(16) as usize);
    buf.set_span(
        area.x,
        area.y,
        &Span::styled(left.clone(), base.fg(theme.gray)),
        left.width() as u16,
    );
    let w = summary.width() as u16;
    if w > 0 && w < area.width {
        buf.set_span(
            area.x + area.width - w,
            area.y,
            &Span::styled(summary.to_string(), base.fg(theme.gray_dim)),
            w,
        );
    }
}

/// `+ 新会话`，右边是搜索框（有内容才画）。
fn paint_actions(buf: &mut Buffer, area: Rect, theme: &Theme, query: &str, focus: Focus) {
    let base = Style::default().bg(theme.bg_base);
    let label = "+ 新会话";
    let style = if focus == Focus::List {
        base.fg(theme.accent_user)
    } else {
        base.fg(theme.gray_dim)
    };
    buf.set_span(
        area.x,
        area.y,
        &Span::styled(label.to_string(), style),
        label.width() as u16,
    );
    let hint = if query.is_empty() {
        "Ctrl+n".to_string()
    } else {
        format!("搜索：{query}")
    };
    let w = (hint.width() as u16).min(area.width.saturating_sub(12));
    if w > 0 {
        let text = truncate_str(&hint, w as usize);
        buf.set_span(
            area.x + area.width - text.width() as u16,
            area.y,
            &Span::styled(text.clone(), base.fg(theme.gray_dim)),
            text.width() as u16,
        );
    }
}

fn paint_list(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    view: &PanelView<'_>,
) -> Vec<(usize, Rect)> {
    if view.rows.is_empty() {
        let empty = if view.query.trim().is_empty() {
            "还没有会话。Ctrl+n 开一个。"
        } else {
            "没有匹配的会话"
        };
        buf.set_span(
            area.x,
            area.y,
            &Span::styled(
                empty.to_string(),
                Style::default().fg(theme.gray).bg(theme.bg_base),
            ),
            empty.width() as u16,
        );
        return Vec::new();
    }
    let start = scroll_start(view.rows, view.selected, area.height);
    let mut out = Vec::new();
    let mut y = area.y;
    for (i, row) in view.rows.iter().enumerate().skip(start) {
        let h = row_height(row);
        if y + h > area.y + area.height {
            break;
        }
        // 焦点在输入框时列表的选中条压暗：光标不在这边了。
        let selected = i == view.selected;
        paint_row(
            buf,
            area.x,
            y,
            area.width,
            theme,
            row,
            selected,
            selected && view.focus == Focus::List,
        );
        out.push((
            i,
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: h,
            },
        ));
        y += h;
    }
    out
}

/// 下半屏：选中会话的尾巴 + 输入框。这一块才是「驱动」。
fn paint_peek(buf: &mut Buffer, area: Rect, theme: &Theme, view: &PanelView<'_>) {
    let base = Style::default().bg(theme.bg_base);
    let border = if view.focus == Focus::Composer {
        theme.accent_user
    } else {
        theme.gray_dim
    };
    let Some(frame) = render_bordered_frame(buf, area, border, theme.bg_base) else {
        return;
    };
    let title = truncate_str(
        &view.peek_title,
        frame.content.width.saturating_sub(10) as usize,
    );
    buf.set_span(
        frame.title_row.x + 1,
        frame.title_row.y,
        &Span::styled(title.clone(), base.fg(theme.gray)),
        title.width() as u16,
    );
    // 右上角的时距，对齐 Grok peek 抬头那个 `2h`。
    let age_w = view.peek_age.width() as u16;
    if age_w > 0 && age_w + 2 < frame.title_row.width {
        buf.set_span(
            frame.title_row.x + frame.title_row.width - age_w - 1,
            frame.title_row.y,
            &Span::styled(view.peek_age.clone(), base.fg(theme.gray_dim)),
            age_w,
        );
    }

    let inner = frame.content;
    if inner.height == 0 {
        return;
    }
    // 最后一行留给输入框，其余给尾巴；尾巴取**末尾**那几行。
    let body_h = inner.height.saturating_sub(1);
    let tail = view
        .peek_lines
        .iter()
        .rev()
        .take(body_h as usize)
        .rev()
        .cloned()
        .collect::<Vec<_>>();
    for (i, line) in tail.iter().enumerate() {
        buf.set_line(inner.x, inner.y + i as u16, line, inner.width);
    }

    let prompt_y = inner.y + body_h;
    let arrow = glyphs::prompt_arrow();
    buf.set_span(
        inner.x,
        prompt_y,
        &Span::styled(
            arrow.to_string(),
            base.fg(if view.focus == Focus::Composer {
                theme.accent_user
            } else {
                theme.gray_dim
            }),
        ),
        arrow.width() as u16,
    );
    let room = inner.width.saturating_sub(arrow.width() as u16 + 1) as usize;
    let text = if !view.composer.is_empty() || view.focus == Focus::Composer {
        truncate_str(view.composer, room)
    } else if view.rows.get(view.selected).is_some_and(DashRow::resumable) {
        // 历史会话没有活着的循环，发不了；别让人对着一个发不出去的框打字。
        truncate_str("Enter 恢复这个会话后才能派活", room)
    } else {
        truncate_str("Tab 进来给这个会话派活", room)
    };
    let style = if view.composer.is_empty() {
        base.fg(theme.gray_dim)
    } else {
        base.fg(theme.text_primary)
    };
    buf.set_span(
        inner.x + arrow.width() as u16,
        prompt_y,
        &Span::styled(text.clone(), style),
        text.width() as u16,
    );
}

fn paint_hints(buf: &mut Buffer, area: Rect, theme: &Theme, focus: Focus) {
    let base = Style::default().bg(theme.bg_base);
    let pairs: &[(&str, &str)] = match focus {
        Focus::List => &[
            ("Enter", ":打开"),
            ("Tab", ":派活"),
            ("x", ":删除"),
            ("Ctrl+n", ":新会话"),
            ("Esc", ":关闭"),
        ],
        Focus::Composer => &[("Enter", ":发送"), ("Tab", ":回列表"), ("Esc", ":关闭")],
    };
    let mut spans = Vec::new();
    for (i, (key, label)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ".to_string(), base));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            base.fg(theme.gray_bright).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled((*label).to_string(), base.fg(theme.gray_dim)));
    }
    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
}

/// 让 `selected` 落进 `viewport` 的第一条可见行下标。
fn scroll_start(rows: &[DashRow], selected: usize, viewport: u16) -> usize {
    let mut start = selected.min(rows.len().saturating_sub(1));
    let mut used = 0u16;
    // 从选中行往回长，直到再加一行就超出视口。
    loop {
        let h = rows.get(start).map(row_height).unwrap_or(1);
        if used + h > viewport {
            start += 1;
            break;
        }
        used += h;
        if start == 0 {
            break;
        }
        start -= 1;
    }
    start.min(rows.len().saturating_sub(1))
}

#[allow(clippy::too_many_arguments)]
// TUI 绘制函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到调用点。
fn paint_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    theme: &Theme,
    row: &DashRow,
    selected: bool,
    focused: bool,
) {
    if let DashRow::Header { state, count } = row {
        render_header(
            buf,
            x,
            y,
            width,
            theme,
            &format!("{} {count}", state.label()),
        );
        return;
    }

    let bg = if focused {
        theme.bg_visual
    } else if selected {
        theme.bg_highlight
    } else {
        theme.bg_base
    };
    buf.set_style(
        Rect {
            x,
            y,
            width,
            height: row_height(row),
        },
        Style::default().bg(bg),
    );

    let prefix = row_prefix(row);
    let prefix_w = prefix.width() as u16;
    let accent = match row.state() {
        RowState::Working => theme.accent_running,
        RowState::Idle => theme.accent_user,
        RowState::Archived => theme.gray_dim,
    };
    buf.set_span(
        x,
        y,
        &Span::styled(prefix.clone(), Style::default().fg(accent).bg(bg)),
        prefix_w,
    );

    let right = right_label(row);
    let right_w = right.width() as u16;
    let gap = if right_w > 0 { 2 } else { 0 };
    let title_budget = width.saturating_sub(prefix_w + right_w + gap + 1).max(1) as usize;
    let title = truncate_str(row_title(row), title_budget);
    let title_style = if selected {
        Style::default()
            .fg(theme.text_primary)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_primary).bg(bg)
    };
    buf.set_span(
        x + prefix_w,
        y,
        &Span::styled(title.clone(), title_style),
        title.width() as u16,
    );
    if right_w > 0 {
        buf.set_span(
            x + width.saturating_sub(right_w + 1),
            y,
            &Span::styled(right, Style::default().fg(theme.gray).bg(bg)),
            right_w,
        );
    }

    let secondary = secondary_line(row);
    if !secondary.is_empty() {
        let indent = prefix_w;
        let budget = width.saturating_sub(indent + 1).max(1) as usize;
        let text = truncate_str(&secondary, budget);
        buf.set_span(
            x + indent,
            y + 1,
            &Span::styled(text.clone(), Style::default().fg(theme.gray_dim).bg(bg)),
            text.width() as u16,
        );
    }
}

fn row_title(row: &DashRow) -> &str {
    match row {
        DashRow::Header { .. } => "",
        DashRow::Tab { title, .. } => title,
        DashRow::Archived { title, .. } => title,
    }
}

/// 开面板：重扫名册、选中第一条可选行。
pub fn open(ctx: &Context) -> crate::views::overlay::Overlay {
    // 上一次看之后可能又存过会话，名册 2s TTL 会让第一眼看到旧列表。
    if let Some(roster) = ctx.get::<Roster>(ROSTER) {
        roster.invalidate();
    }
    let collapsed = HashSet::new();
    let rows = build_rows(ctx, "", &collapsed);
    crate::views::overlay::Overlay::Dashboard {
        selected: first_selectable(&rows),
        query: String::new(),
        collapsed,
        focus: Focus::default(),
        composer: String::new(),
        composer_cursor: 0,
    }
}

/// 选中行对应的那一页 ctx（只有开着的分页才有）。驱动与 peek 都从它取。
pub fn tab_ctx(ctx: &Context, row: &DashRow) -> Option<Context> {
    let DashRow::Tab { id, .. } = row else {
        return None;
    };
    let tabs = ctx.get::<Tabs>(TUI_TABS)?;
    tabs.list()
        .iter()
        .position(|t| t.id == *id)
        .and_then(|i| tabs.contexts().into_iter().nth(i))
}

/// 把消息发给**选中的那个会话**，不切页。
///
/// 走那一页自己的 `"session.port"`：分页本来就一页一棵 isolate 子树，各有各的
/// 会话与循环，面板只是换个地方按下回车。磁盘会话没有活着的循环，发不了。
pub fn submit_to(ctx: &Context, row: &DashRow, text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("先写点什么".into());
    }
    let Some(tab) = tab_ctx(ctx, row) else {
        return Err("只能给开着的会话派活；历史会话先按 Enter 恢复".into());
    };
    let port = tab
        .get::<SessionRef>(SESSION_PORT)
        .ok_or_else(|| "这一页还没准备好".to_string())?;
    port.submit(text.to_string(), true);
    Ok(())
}

/// `Overlay::Dashboard` 里那几个字段，原样传进来。
pub struct PanelInput<'a> {
    pub rows: &'a [DashRow],
    pub selected: usize,
    pub query: &'a str,
    pub focus: Focus,
    pub composer: &'a str,
}

/// 装配 [`PanelView`] 并画出来。把「取数据」和「画」分开，渲染那半边才好测。
pub fn render(buf: &mut Buffer, area: Rect, ctx: &Context, input: &PanelInput<'_>) -> PickerHits {
    let (peek_lines, peek_title) = match input.rows.get(input.selected) {
        Some(r) => peek_for(ctx, r, area.width.saturating_sub(4) as usize),
        None => (Vec::new(), String::new()),
    };
    let view = PanelView {
        rows: input.rows,
        selected: input.selected,
        query: input.query,
        focus: input.focus,
        composer: input.composer,
        peek_lines,
        peek_title,
        peek_age: match input.rows.get(input.selected) {
            Some(DashRow::Archived { updated, .. }) => age_label(*updated),
            _ => String::new(),
        },
        summary: summary_label(input.rows),
    };
    render_panel(buf, area, &view)
}

/// 右上角 `◇ 2 空闲 · 1 进行中` 那一条。
fn summary_label(rows: &[DashRow]) -> String {
    let mut parts = Vec::new();
    for state in [RowState::Working, RowState::Idle] {
        let n = rows
            .iter()
            .filter(|r| !r.is_header() && r.state() == state)
            .count();
        if n > 0 {
            parts.push(format!("{n} {}", state.label()));
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("{} {}", glyphs::diamond_hollow(), parts.join(" · "))
    }
}

/// 选中行的对话尾巴 + peek 抬头。
///
/// **两种来源都走同一套 `child_transcript` 渲染**，画出来和正常对话一模一样：
/// 用户气泡、思考行、markdown、工具卡、配色全都在。之前历史会话那块是手写的
/// 三行灰字（摘要 + cwd + 一句提示），既不是真内容，灰底灰字也读不出来。
///
/// 开着的分页读它自己的 `"sessions"`（实时）；历史会话走
/// [`Roster::transcript`]（按 id 读一份，带最近一条的缓存）。
fn peek_for(ctx: &Context, row: &DashRow, width: usize) -> (Vec<Line<'static>>, String) {
    let events = match row {
        DashRow::Header { .. } => return (Vec::new(), String::new()),
        DashRow::Tab { .. } => tab_ctx(ctx, row)
            .and_then(|tab| tab.get::<Sessions>(SESSIONS).map(|s| s.events()))
            .unwrap_or_default(),
        DashRow::Archived { id, cwd, .. } => ctx
            .get::<Roster>(ROSTER)
            .map(|r| r.transcript(id, cwd))
            .unwrap_or_default(),
    };
    let title = format!("{} · {}", row_title(row), row.state().label());
    if events.is_empty() {
        return (Vec::new(), title);
    }
    let view = crate::scrollback::child_transcript(
        &events,
        width.max(8),
        &HashSet::new(),
        &HashMap::new(),
        false,
        false,
        ctx.get::<cordis_spine::AgentPresets>(cordis_spine::AGENT_PRESETS)
            .as_deref(),
        ctx.get::<cordis_spine::Subagents>(cordis_spine::SUBAGENTS)
            .as_deref(),
    );
    (view.lines, title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn tab(id: usize, title: &str, working: bool, active: bool) -> DashRow {
        DashRow::Tab {
            id,
            title: title.into(),
            active,
            working,
        }
    }

    fn archived(id: &str, title: &str, here: bool, secs: u64) -> DashRow {
        DashRow::Archived {
            id: id.into(),
            title: title.into(),
            summary: "上一句说了点什么".into(),
            cwd: if here {
                PathBuf::from("/tmp/here")
            } else {
                PathBuf::from("/tmp/elsewhere")
            },
            here,
            updated: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs),
        }
    }

    fn labels(rows: &[DashRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                DashRow::Header { state, count } => format!("# {} {count}", state.label()),
                other => row_title(other).to_string(),
            })
            .collect()
    }

    /// 会话按状态落进三个组，每组一个抬头。**只有会话**——子代理归 `/tasks`。
    #[test]
    fn rows_group_by_state_under_headers() {
        let rows = vec![
            tab(1, "主线", false, true),
            tab(2, "在跑", true, false),
            archived("a1", "旧会话", true, 10),
        ];
        let out = shape(rows, "", &HashSet::new());
        assert_eq!(
            labels(&out),
            vec![
                "# 进行中 1",
                "在跑",
                "# 空闲 1",
                "主线",
                "# 历史 1",
                "旧会话"
            ]
        );
    }

    /// 组内排序：开着的分页按编号，磁盘会话按时间新到旧。
    #[test]
    fn tabs_sort_by_id_and_archives_are_newest_first() {
        let rows = vec![
            archived("old", "旧", true, 10),
            archived("new", "新", true, 900),
            tab(3, "第三页", false, false),
            tab(1, "第一页", false, false),
        ];
        let out = shape(rows, "", &HashSet::new());
        assert_eq!(
            labels(&out),
            vec!["# 空闲 2", "第一页", "第三页", "# 历史 2", "新", "旧"]
        );
    }

    /// 折叠只留抬头，计数不变。
    #[test]
    fn collapsing_a_group_keeps_its_header_and_count() {
        let rows = vec![
            archived("a1", "甲", true, 10),
            archived("a2", "乙", true, 20),
        ];
        let mut collapsed = HashSet::new();
        collapsed.insert(RowState::Archived);
        let out = shape(rows, "", &collapsed);
        assert_eq!(labels(&out), vec!["# 历史 2"]);
    }

    /// 抬头的计数跟着**过滤后**的结果走。看到「历史 12」底下只有 1 行，
    /// 会以为界面坏了。
    #[test]
    fn header_count_follows_the_filter() {
        let rows = vec![
            archived("a1", "查询天气", true, 30),
            archived("a2", "重构解析器", true, 20),
            archived("a3", "重构渲染", true, 10),
        ];
        let out = shape(rows, "重构", &HashSet::new());
        assert_eq!(labels(&out), vec!["# 历史 2", "重构解析器", "重构渲染"]);
    }

    /// 过滤也认副行与 cwd，不只认标题。
    #[test]
    fn filter_also_matches_the_summary_and_cwd() {
        let rows = vec![archived("a1", "无关标题", false, 10)];
        assert_eq!(shape(rows.clone(), "elsewhere", &HashSet::new()).len(), 2);
        assert_eq!(shape(rows.clone(), "上一句", &HashSet::new()).len(), 2);
        assert!(shape(rows, "对不上", &HashSet::new()).is_empty());
    }

    /// 别的工作目录下的会话不可恢复，且行尾要标出来。
    ///
    /// `Sessions::restore` 只认 `load_cwd(当前 cwd)` 填出来的那份列表，跨目录
    /// 按下去必然无声失败——宁可提前标死。
    #[test]
    fn a_session_from_another_cwd_is_not_resumable_and_says_so() {
        let here = archived("h", "本目录", true, 10);
        let there = archived("t", "别处", false, 10);
        assert!(here.resumable());
        assert!(!there.resumable());
        assert!(!right_label(&here).contains("其它目录"));
        assert!(
            right_label(&there).contains("其它目录"),
            "{}",
            right_label(&there)
        );
    }

    /// 抬头行不可恢复也不是条目——Enter 在它身上是折叠开关。
    #[test]
    fn headers_are_not_entries() {
        let h = DashRow::Header {
            state: RowState::Idle,
            count: 3,
        };
        assert!(h.is_header());
        assert!(!h.resumable());
        assert_eq!(row_height(&h), 1);
    }

    /// 双行行体下滚动按**行高**算。按条数算会让选中项滚出视口。
    #[test]
    fn scroll_start_keeps_the_selection_visible_with_two_line_rows() {
        let rows: Vec<DashRow> = (0..20)
            .map(|i| archived(&format!("{i}"), &format!("会话 {i}"), true, i))
            .collect();
        assert!(
            rows.iter().all(|r| row_height(r) == 2),
            "这些行都该是两行高"
        );

        let viewport = 10u16; // 双行 → 最多 5 条
        let start = scroll_start(&rows, 19, viewport);
        let shown: u16 = rows[start..=19].iter().map(row_height).sum();
        assert!(shown <= viewport, "start={start} shown={shown}");
        let one_more: u16 = rows[start.saturating_sub(1)..=19]
            .iter()
            .map(row_height)
            .sum();
        assert!(one_more > viewport, "再往回一条就该超出视口");
    }

    /// 选中第一行时不该往回卷。
    #[test]
    fn scroll_start_is_zero_at_the_top() {
        let rows: Vec<DashRow> = (0..5)
            .map(|i| archived(&format!("{i}"), "标题", true, i))
            .collect();
        assert_eq!(scroll_start(&rows, 0, 10), 0);
    }

    fn view<'a>(
        rows: &'a [DashRow],
        selected: usize,
        focus: Focus,
        composer: &'a str,
    ) -> PanelView<'a> {
        PanelView {
            rows,
            selected,
            query: "",
            focus,
            composer,
            peek_lines: vec![Line::from("尾巴一行")],
            peek_title: "某会话 · 空闲".into(),
            peek_age: String::new(),
            summary: summary_label(rows),
        }
    }

    /// 各种尺寸下不 panic，画出来的行也不越过给定区域。
    #[test]
    fn narrow_frame_does_not_panic() {
        let rows = vec![
            DashRow::Header {
                state: RowState::Idle,
                count: 1,
            },
            tab(1, "很长很长很长很长很长很长很长很长的标题", false, true),
            archived("a", "历史会话标题也很长很长很长很长", true, 10),
        ];
        for width in [12u16, 20, 40, 120] {
            for height in [6u16, 10, 20, 40] {
                let area = Rect::new(0, 0, width, height);
                let mut buf = ratatui::buffer::Buffer::empty(area);
                let hits = render_panel(&mut buf, area, &view(&rows, 1, Focus::List, ""));
                for (_, r) in &hits.rows {
                    assert!(
                        r.y + r.height <= area.y + area.height,
                        "行画出了区域 {r:?} area={area:?}"
                    );
                }
            }
        }
    }

    /// 矮窗把 peek 整块让掉——列表才是主体，挤成两行谁都读不了。
    #[test]
    fn a_short_terminal_drops_the_peek_pane() {
        assert!(layout(Rect::new(0, 0, 80, 40)).peek.is_some());
        assert!(layout(Rect::new(0, 0, 80, PEEK_MIN_TOTAL)).peek.is_some());
        assert!(layout(Rect::new(0, 0, 80, PEEK_MIN_TOTAL - 1))
            .peek
            .is_none());
    }

    /// 四段不重叠，且都在区域内。布局算错了会表现成某一块被另一块盖掉。
    #[test]
    fn panes_do_not_overlap() {
        for height in [6u16, 12, 18, 30, 60] {
            let area = Rect::new(0, 0, 80, height);
            let p = layout(area);
            let mut spans = vec![p.header, p.actions, p.list, p.hints];
            if let Some(peek) = p.peek {
                spans.push(peek);
            }
            for r in &spans {
                assert!(
                    r.y >= area.y && r.y + r.height <= area.y + area.height,
                    "height={height} {r:?} 越界"
                );
            }
            spans.sort_by_key(|r| r.y);
            for pair in spans.windows(2) {
                assert!(
                    pair[0].y + pair[0].height <= pair[1].y,
                    "height={height} {:?} 与 {:?} 重叠",
                    pair[0],
                    pair[1]
                );
            }
        }
    }

    /// 底部快捷键条跟着焦点走：在列表里 Enter 是打开，在输入框里 Enter 是发送。
    /// 这两个含义不同，提示必须跟着变，否则按下去的结果和写的不是一回事。
    #[test]
    fn hints_follow_the_focus() {
        let rows = vec![tab(1, "会话", false, true)];
        let list = render_text(&view(&rows, 0, Focus::List, ""), 70, 30);
        assert!(list.contains("Enter:打开"), "{list}");
        assert!(list.contains("Tab:派活"), "{list}");

        let composing = render_text(&view(&rows, 0, Focus::Composer, ""), 70, 30);
        assert!(composing.contains("Enter:发送"), "{composing}");
        assert!(composing.contains("Tab:回列表"), "{composing}");
    }

    /// 输入框有内容就画内容，空着且没焦点时给一句引导。
    #[test]
    fn the_composer_shows_its_text_or_a_hint() {
        let rows = vec![tab(1, "会话", false, true)];
        let idle = render_text(&view(&rows, 0, Focus::List, ""), 70, 30);
        assert!(idle.contains("Tab 进来给这个会话派活"), "{idle}");

        let typed = render_text(&view(&rows, 0, Focus::Composer, "跑一下测试"), 70, 30);
        assert!(typed.contains("跑一下测试"), "{typed}");
        assert!(!typed.contains("Tab 进来给"), "{typed}");
    }

    /// 右上角汇总只数会话，不数抬头。
    #[test]
    fn summary_counts_sessions_not_headers() {
        let rows = shape(
            vec![
                tab(1, "在跑", true, false),
                tab(2, "闲着", false, true),
                archived("a", "历史", true, 10),
            ],
            "",
            &HashSet::new(),
        );
        let text = summary_label(&rows);
        assert!(text.contains("1 进行中"), "{text}");
        assert!(text.contains("1 空闲"), "{text}");
        assert!(!text.contains("历史"), "历史不算在场的会话: {text}");
    }

    /// 面板必须**擦掉**底下那一屏，不能只铺底色。
    ///
    /// `buf.set_style` 只改样式、不动字符：原来只调它，结果主状态栏、滚动区正文、
    /// 输入框的字全从面板底下透上来，糊成一片。
    #[test]
    fn the_panel_clears_whatever_was_underneath() {
        let area = Rect::new(0, 0, 60, 30);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        // 先在整屏写满「底下那一屏」的字。
        for y in 0..area.height {
            for x in 0..area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char('底');
                }
            }
        }
        let rows = vec![tab(1, "会话", false, true)];
        render_panel(&mut buf, area, &view(&rows, 0, Focus::List, ""));

        let leaked = (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[(x, y)].symbol() == "底")
            .count();
        assert_eq!(leaked, 0, "有 {leaked} 格底层内容没被擦掉");
    }

    /// 空列表要给一句话，不是一片空白。
    #[test]
    fn empty_list_explains_itself() {
        let area = Rect::new(0, 0, 60, 30);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_panel(&mut buf, area, &view(&[], 0, Focus::List, ""));
        // 逐行读，且**按显示宽度步进**：中文占两格，第二格在 buffer 里是空白，
        // 一格一格收会读成「还 没 有 会 话」，断言永远对不上。
        let text: String = (0..area.height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0u16;
                while x < area.width {
                    let sym = buf[(x, y)].symbol().to_string();
                    x += (sym.width() as u16).max(1);
                    line.push_str(&sym);
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("还没有会话"), "{text}");
    }

    /// 把整块面板画出来再按显示宽度逐行读回文本。
    fn render_text(view: &PanelView<'_>, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_panel(&mut buf, area, view);
        (0..area.height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0u16;
                while x < area.width {
                    let sym = buf[(x, y)].symbol().to_string();
                    x += (sym.width() as u16).max(1);
                    line.push_str(&sym);
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
