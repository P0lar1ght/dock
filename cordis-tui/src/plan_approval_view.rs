//! Plan approval chrome — Permission-style radio + scrollable plan body.
//!
//! Options mirror grok plan approval: approve / revise / quit.
//! Body scroll: mouse wheel / j/k (overlay keeps ↑↓ for option selection).
//!
//! `/view-plan` snapshots the file at open (Grok line-viewer does the same)
//! and paints Grok pretty markdown (`StreamingMarkdownRenderer` + wrap),
//! matching Plan: Exit cards. Wrapped lines are cached by width.

use std::sync::{Arc, Mutex};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget};

use crate::grok::glyphs;
use crate::grok::md_style;
use crate::grok::picker::PickerHits;
use crate::grok::wrapping::word_wrap_lines;
use crate::theme::Theme;
use cordis_spine::{PlanApprovalPrompt, PlanDecision};

#[derive(Debug, Default)]
struct CachedWrap {
    width: usize,
    viewport: usize,
    lines: Option<Arc<Vec<Line<'static>>>>,
}

/// Shared wrap cache for a plan overlay. `Overlay` clones share the same cache.
#[derive(Debug, Clone)]
pub struct PlanWrapCache {
    inner: Arc<Mutex<CachedWrap>>,
}

impl PlanWrapCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CachedWrap::default())),
        }
    }

    pub fn lines(&self, prompt: &PlanApprovalPrompt, width: usize) -> Arc<Vec<Line<'static>>> {
        let width = width.max(8);
        let mut g = self.inner.lock().unwrap();
        if g.width == width {
            if let Some(lines) = &g.lines {
                return Arc::clone(lines);
            }
        }
        let lines = Arc::new(render_body(prompt, width));
        g.width = width;
        g.lines = Some(Arc::clone(&lines));
        lines
    }

    pub fn layout(
        &self,
        prompt: &PlanApprovalPrompt,
        width: usize,
        viewport: usize,
    ) -> Arc<Vec<Line<'static>>> {
        let lines = self.lines(prompt, width);
        self.inner.lock().unwrap().viewport = viewport.max(1);
        lines
    }

    /// Scroll clamp: prefer last painted width/viewport so j/k does not re-render.
    pub fn max_scroll(
        &self,
        prompt: &PlanApprovalPrompt,
        width_hint: usize,
        viewport_hint: usize,
    ) -> usize {
        let (width, viewport) = {
            let g = self.inner.lock().unwrap();
            (
                if g.lines.is_some() {
                    g.width
                } else {
                    width_hint.max(8)
                },
                if g.viewport > 0 {
                    g.viewport
                } else {
                    viewport_hint.max(1)
                },
            )
        };
        let total = self.lines(prompt, width).len();
        total.saturating_sub(viewport.max(1))
    }
}

pub const OPTIONS: &[(PlanDecision, &str)] = &[
    (PlanDecision::Approve, "批准并开始实现"),
    (PlanDecision::Revise, "要求修改计划"),
    (PlanDecision::Quit, "放弃计划"),
];

pub const VIEW_OPTIONS: &[(PlanDecision, &str)] = &[(PlanDecision::Quit, "关闭")];

pub fn options(view_only: bool) -> &'static [(PlanDecision, &'static str)] {
    if view_only {
        VIEW_OPTIONS
    } else {
        OPTIONS
    }
}

pub fn decision_at(selected: usize, view_only: bool) -> PlanDecision {
    options(view_only)
        .get(selected)
        .map(|(d, _)| *d)
        .unwrap_or(PlanDecision::Quit)
}

/// Prefer a tall chrome so long plans are readable; caller clamps to pane.
pub fn chrome_height(inner_height: u16, view_only: bool) -> u16 {
    let opts = options(view_only).len() as u16;
    // title + hint + gap + options + padding
    let chrome = 2 + 1 + 1 + opts + 2;
    let want = (inner_height.saturating_mul(3) / 4).max(chrome + 12);
    want.min(inner_height.saturating_sub(2)).max(chrome + 8)
}

pub fn body_viewport_rows(area_height: u16, view_only: bool) -> usize {
    let opts = options(view_only).len() as u16;
    // title(1) + hint(1) + blank(1) + opts + bottom pad(1)
    area_height.saturating_sub(2 + 1 + 1 + opts + 1).max(4) as usize
}

#[cfg(test)]
pub fn max_scroll(prompt: &PlanApprovalPrompt, width: usize, viewport: usize) -> usize {
    render_body(prompt, width.max(8))
        .len()
        .saturating_sub(viewport.max(1))
}

pub fn render(
    buf: &mut Buffer,
    area: Rect,
    prompt: &PlanApprovalPrompt,
    wrap: &PlanWrapCache,
    selected: usize,
    view_only: bool,
    scroll: usize,
) -> PickerHits {
    if area.height == 0 || area.width == 0 {
        return PickerHits::default();
    }
    let theme = Theme::current();
    let bg = Style::default().fg(theme.text_primary).bg(theme.bg_light);
    Clear.render(area, buf);
    buf.set_style(area, bg);

    let accent = Style::default().fg(theme.accent_plan).bg(theme.bg_light);
    for row in area.y..area.y + area.height {
        if let Some(cell) = buf.cell_mut((area.x, row)) {
            cell.set_symbol(glyphs::accent_bar());
            cell.set_style(accent);
        }
    }

    let content_x = area.x.saturating_add(3);
    let content_w = area.width.saturating_sub(5);
    let mut y = area.y.saturating_add(1);
    let title_style = Style::default()
        .fg(theme.accent_plan)
        .bg(theme.bg_light)
        .add_modifier(Modifier::BOLD);
    let title = if view_only {
        format!("计划 · {}", prompt.path)
    } else if prompt.empty {
        "批准计划？（尚未写入内容）".to_string()
    } else {
        format!("批准计划？ · {}", prompt.path)
    };
    buf.set_line(
        content_x,
        y,
        &Line::from(Span::styled(title, title_style)),
        content_w,
    );
    y = y.saturating_add(1);

    let viewport = body_viewport_rows(area.height, view_only);
    let lines = wrap.layout(prompt, content_w as usize, viewport);
    let max_s = lines.len().saturating_sub(viewport.max(1));
    let scroll = scroll.min(max_s);
    let hint = if lines.len() > viewport {
        format!(
            "j/k 或滚轮滚动 ({}/{})  ·  a 批准  s 修改  q 放弃",
            scroll + 1,
            max_s + 1
        )
    } else if view_only {
        "Enter / Esc 关闭".to_string()
    } else {
        "a 批准  ·  s 修改  ·  q 放弃".to_string()
    };
    buf.set_line(
        content_x,
        y,
        &Line::from(Span::styled(hint, theme.dim().bg(theme.bg_light))),
        content_w,
    );
    y = y.saturating_add(1);

    let muted = theme.muted().bg(theme.bg_light);
    let end = (scroll + viewport).min(lines.len());
    for line in lines.iter().take(end).skip(scroll) {
        if y >= area.y
            + area
                .height
                .saturating_sub(options(view_only).len() as u16 + 2)
        {
            break;
        }
        let painted = line_on_overlay_bg(line, theme.bg_light);
        buf.set_line(content_x, y, &painted, content_w);
        y = y.saturating_add(1);
    }
    if scroll + viewport < lines.len()
        && y < area.y
            + area
                .height
                .saturating_sub(options(view_only).len() as u16 + 2)
    {
        buf.set_line(
            content_x,
            y,
            &Line::from(Span::styled("…", muted)),
            content_w,
        );
        y = y.saturating_add(1);
    }
    y = y.saturating_add(1);

    let opts = options(view_only);
    let sel = selected.min(opts.len().saturating_sub(1));
    let mut hits = PickerHits::default();
    for (i, (_, label)) in opts.iter().enumerate() {
        if y >= area.y + area.height {
            break;
        }
        let selected = i == sel;
        let row_bg = if selected {
            theme.bg_visual
        } else {
            theme.bg_light
        };
        let row_rect = Rect {
            x: area.x.saturating_add(1),
            y,
            width: area.width.saturating_sub(1),
            height: 1,
        };
        buf.set_style(row_rect, Style::default().bg(row_bg));
        if let Some(cell) = buf.cell_mut((area.x, y)) {
            cell.set_symbol(glyphs::accent_bar());
            cell.set_style(accent);
        }
        let mark = if selected { "●" } else { "○" };
        let mark_style = Style::default()
            .fg(if selected {
                theme.accent_plan
            } else {
                theme.gray
            })
            .bg(row_bg);
        let label_style = Style::default()
            .fg(theme.text_primary)
            .bg(row_bg)
            .add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
        let n = i + 1;
        buf.set_line(
            content_x,
            y,
            &Line::from(vec![
                Span::styled(format!(" {mark} {n}"), mark_style),
                Span::styled(
                    format!(". {label}"),
                    if selected { label_style } else { mark_style },
                ),
            ]),
            content_w,
        );
        hits.rows.push((
            i,
            Rect {
                x: content_x,
                y,
                width: content_w,
                height: 1,
            },
        ));
        y = y.saturating_add(1);
    }
    hits
}

fn render_body(prompt: &PlanApprovalPrompt, width: usize) -> Vec<Line<'static>> {
    let body = if prompt.empty || prompt.body.trim().is_empty() {
        "（计划文件为空）"
    } else {
        prompt.body.trim()
    };
    render_plan_markdown(body, width.max(8))
}

fn render_plan_markdown(text: &str, width: usize) -> Vec<Line<'static>> {
    let syntect = cordis_markdown::default_syntect();
    let mut renderer = cordis_markdown::StreamingMarkdownRenderer::new(md_style::style(), true);
    renderer.set_max_table_width(Some(width));
    renderer.push(text);
    let output = renderer.finish_into_output(Some(syntect));
    word_wrap_lines(output.lines, width)
}

fn line_on_overlay_bg(line: &Line<'static>, bg: Color) -> Line<'static> {
    let mut line = line.clone();
    for span in &mut line.spans {
        if span.style.bg.is_none() {
            span.style = span.style.bg(bg);
        }
    }
    if line.style.bg.is_none() {
        line.style = line.style.bg(bg);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn max_scroll_allows_reaching_tail() {
        let prompt = PlanApprovalPrompt {
            path: ".dock/plan.md".into(),
            body: (0..40)
                .map(|i| format!("this is a reasonably long plan line number {i}\n"))
                .collect(),
            empty: false,
        };
        let max = max_scroll(&prompt, 40, 10);
        assert!(max >= 20, "{max}");
    }

    #[test]
    fn wrap_cache_reuses_until_width_changes() {
        let prompt = PlanApprovalPrompt {
            path: ".dock/plan.md".into(),
            body: (0..40)
                .map(|i| format!("this is a reasonably long plan line number {i} for wrapping\n"))
                .collect(),
            empty: false,
        };
        let cache = PlanWrapCache::new();
        let a = cache.lines(&prompt, 40);
        let b = cache.lines(&prompt, 40);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(
            cache.max_scroll(&prompt, 99, 10),
            a.len().saturating_sub(10)
        );
        let c = cache.lines(&prompt, 12);
        assert!(!Arc::ptr_eq(&a, &c));
        assert!(c.len() > a.len());
    }

    #[test]
    fn plan_body_renders_markdown_heading() {
        let prompt = PlanApprovalPrompt {
            path: ".dock/plan.md".into(),
            body: "# Auth Plan\n\nDo the **thing**.\n".into(),
            empty: false,
        };
        let cache = PlanWrapCache::new();
        let text = plain(&cache.lines(&prompt, 80));
        assert!(text.contains("Auth Plan"), "{text}");
        assert!(text.contains("thing"), "{text}");
        assert!(
            !text.contains("# Auth"),
            "pretty mode should hide heading marker:\n{text}"
        );
        assert!(
            !text.contains("**thing**"),
            "pretty mode should hide emphasis markers:\n{text}"
        );
    }
}
