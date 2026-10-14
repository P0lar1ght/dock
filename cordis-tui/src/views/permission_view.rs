//! Permission overlay chrome copied from grok pager `views/permission_view.rs`
//! (`┃` accent, numbered radio options). ACP kinds; no MvpAgent.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::grok::glyphs;
use crate::grok::picker::PickerHits;
use crate::theme::Theme;
use cordis_spine::{PermissionOptionKind, PermissionPrompt};

pub const OPTIONS: &[(PermissionOptionKind, &str)] = &[
    (PermissionOptionKind::AllowOnce, "允许一次"),
    (PermissionOptionKind::AllowAlways, "始终允许"),
    (PermissionOptionKind::RejectOnce, "拒绝一次"),
    (PermissionOptionKind::RejectAlways, "始终拒绝"),
];

/// `source`：请求是子代理发的时，标题上方多一行「来自子代理 · 角色 · 任务」。
pub fn chrome_height(prompt: &PermissionPrompt, source: Option<&str>, width: u16) -> u16 {
    let content_w = width.saturating_sub(5).max(8) as usize;
    let summary_rows = summary_rows(&prompt.summary, content_w).len() as u16;
    let source_rows = u16::from(source.is_some());
    (2 + source_rows + summary_rows + 1 + OPTIONS.len() as u16 + 1).clamp(8, 17)
}

pub fn render(
    buf: &mut Buffer,
    area: Rect,
    prompt: &PermissionPrompt,
    source: Option<&str>,
    selected: usize,
) -> PickerHits {
    if area.height == 0 || area.width == 0 {
        return PickerHits::default();
    }
    let theme = Theme::current();
    let bg = Style::default().fg(theme.text_primary).bg(theme.bg_light);
    Clear.render(area, buf);
    fill_rect(buf, area, bg);

    let accent = Style::default().fg(theme.accent_user).bg(theme.bg_light);
    for row in area.y..area.y + area.height {
        if let Some(cell) = buf.cell_mut((area.x, row)) {
            cell.set_symbol(glyphs::accent_bar());
            cell.set_style(accent);
        }
    }

    let content_x = area.x.saturating_add(3);
    let content_w = area.width.saturating_sub(5);
    let mut y = area.y.saturating_add(1);
    if let Some(source) = source {
        let source_style = Style::default().fg(theme.accent_user).bg(theme.bg_light);
        buf.set_line(
            content_x,
            y,
            &Line::from(Span::styled(source.to_string(), source_style)),
            content_w,
        );
        y = y.saturating_add(1);
    }
    let title_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.bg_light)
        .add_modifier(Modifier::BOLD);
    buf.set_line(
        content_x,
        y,
        &Line::from(Span::styled(
            format!("允许使用 {}？", prompt.tool),
            title_style,
        )),
        content_w,
    );
    y = y.saturating_add(1);

    let summary_style = Style::default().fg(theme.gray).bg(theme.bg_light);
    let note_style = Style::default()
        .fg(theme.gray)
        .bg(theme.bg_light)
        .add_modifier(Modifier::ITALIC);
    for (line, note) in summary_rows(&prompt.summary, content_w as usize) {
        if y >= area.y + area.height {
            break;
        }
        let style = if note { note_style } else { summary_style };
        buf.set_line(
            content_x,
            y,
            &Line::from(Span::styled(line, style)),
            content_w,
        );
        y = y.saturating_add(1);
    }
    y = y.saturating_add(1);

    let sel = selected.min(OPTIONS.len().saturating_sub(1));
    let mut hits = PickerHits::default();
    for (i, (_, label)) in OPTIONS.iter().enumerate() {
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
        fill_rect(buf, row_rect, Style::default().bg(row_bg));
        if let Some(cell) = buf.cell_mut((area.x, y)) {
            cell.set_symbol(glyphs::accent_bar());
            cell.set_style(Style::default().fg(theme.accent_user).bg(row_bg));
        }
        let num_style = Style::default().fg(theme.accent_user).bg(row_bg);
        let marker = if selected {
            glyphs::filled_dot()
        } else {
            glyphs::hollow_dot()
        };
        let text_style = if selected {
            Style::default()
                .fg(theme.text_primary)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary).bg(row_bg)
        };
        buf.set_line(
            content_x,
            y,
            &Line::from(vec![
                Span::styled(format!("{} ", i + 1), num_style),
                Span::styled(format!("({marker}) "), text_style),
                Span::styled(*label, text_style),
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

pub fn kind_at(selected: usize) -> PermissionOptionKind {
    OPTIONS
        .get(selected.min(OPTIONS.len() - 1))
        .map(|(k, _)| *k)
        .unwrap_or(PermissionOptionKind::RejectOnce)
}

fn fill_rect(buf: &mut Buffer, area: Rect, style: Style) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_symbol(" ");
                cell.set_style(style);
            }
        }
    }
}

/// 摘要的显示行：`(文字, 是不是说明行)`。
///
/// 首行是要紧的那句（命令工具是 `$ 完整命令`），最多折 3 行，再长用 `…` 收尾，
/// 看得出被截了。后面的行是说明（命令工具的 `description`），各占一行、斜体淡色，
/// 只做补充，不挤占命令的行数。
fn summary_rows(summary: &str, width: usize) -> Vec<(String, bool)> {
    const MAIN_ROWS: usize = 3;
    let mut parts = summary.split('\n');
    let main = parts.next().unwrap_or("");
    let mut wrapped = wrap_line(main, width);
    let cut = wrapped.len() > MAIN_ROWS;
    wrapped.truncate(MAIN_ROWS);
    if cut {
        if let Some(last) = wrapped.last_mut() {
            *last = with_ellipsis(last, width);
        }
    }
    let mut rows: Vec<(String, bool)> = wrapped.into_iter().map(|l| (l, false)).collect();
    for note in parts.map(str::trim).filter(|l| !l.is_empty()) {
        let mut wrapped = wrap_line(note, width);
        let cut = wrapped.len() > 1;
        wrapped.truncate(1);
        if let Some(first) = wrapped.pop() {
            rows.push((
                if cut {
                    with_ellipsis(&first, width)
                } else {
                    first
                },
                true,
            ));
        }
    }
    rows
}

/// 把 `…` 接在行尾，超宽就先从尾巴让出位置。
fn with_ellipsis(line: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    let budget = width.saturating_sub(1);
    for c in line.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > budget {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('\u{2026}');
    out
}

fn wrap_line(text: &str, width: usize) -> Vec<String> {
    bash_quote_aware_wrap(text, width, usize::MAX)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Grok pager `bash_quote_aware_wrap`: prefer breaks at whitespace *outside*
/// quotes so wide quoted args stay intact until they themselves overflow.
fn bash_quote_aware_wrap(line: &str, width: usize, max_rows: usize) -> Vec<&str> {
    if max_rows == 0 {
        return Vec::new();
    }
    if width == 0 || UnicodeWidthStr::width(line) <= width {
        return vec![line];
    }

    let mut rows: Vec<&str> = Vec::new();
    let mut row_start = 0usize;
    let mut last_break = 0usize;
    let candidates = QuoteAwareBreaks::new(line).chain(std::iter::once(line.len()));

    for b in candidates {
        if b <= row_start {
            continue;
        }
        let candidate = line.get(row_start..b).unwrap_or("").trim_end();
        if UnicodeWidthStr::width(candidate) <= width {
            last_break = b;
            continue;
        }
        if last_break > row_start {
            let row = line.get(row_start..last_break).unwrap_or("").trim_end();
            if !row.is_empty() {
                rows.push(row);
                if rows.len() >= max_rows {
                    return rows;
                }
            }
            row_start = last_break;
            while row_start < line.len()
                && line
                    .as_bytes()
                    .get(row_start)
                    .is_some_and(|b| b.is_ascii_whitespace())
            {
                row_start += 1;
            }
            last_break = row_start;
            if b > row_start {
                let candidate = line.get(row_start..b).unwrap_or("").trim_end();
                if UnicodeWidthStr::width(candidate) <= width {
                    last_break = b;
                } else {
                    extend_display_width_rows(&mut rows, line, row_start, b, width, max_rows);
                    if rows.len() >= max_rows {
                        return rows;
                    }
                    row_start = b;
                    while row_start < line.len()
                        && line
                            .as_bytes()
                            .get(row_start)
                            .is_some_and(|b| b.is_ascii_whitespace())
                    {
                        row_start += 1;
                    }
                    last_break = row_start;
                }
            }
        } else {
            extend_display_width_rows(&mut rows, line, row_start, b, width, max_rows);
            if rows.len() >= max_rows {
                return rows;
            }
            row_start = b;
            while row_start < line.len()
                && line
                    .as_bytes()
                    .get(row_start)
                    .is_some_and(|b| b.is_ascii_whitespace())
            {
                row_start += 1;
            }
            last_break = row_start;
        }
    }
    if row_start < line.len() && rows.len() < max_rows {
        let row = line.get(row_start..).unwrap_or("").trim_end();
        if !row.is_empty() {
            rows.push(row);
        }
    }
    if rows.is_empty() {
        vec![line]
    } else {
        rows
    }
}

fn display_width_end(s: &str, start: usize, limit: usize, width: usize) -> usize {
    let Some(rest) = s.get(start..limit.min(s.len())) else {
        return start;
    };
    let mut used = 0usize;
    let mut end = start;
    for ch in rest.chars() {
        let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_w > width && end > start {
            break;
        }
        used += ch_w;
        end += ch.len_utf8();
        if used >= width {
            break;
        }
    }
    end
}

fn extend_display_width_rows<'a>(
    rows: &mut Vec<&'a str>,
    line: &'a str,
    start: usize,
    end: usize,
    width: usize,
    max_rows: usize,
) {
    let end = end.min(line.len());
    let mut pos = start.min(end);
    while pos < end && rows.len() < max_rows {
        let chunk_end = display_width_end(line, pos, end, width);
        if chunk_end <= pos {
            break;
        }
        match line.get(pos..chunk_end) {
            Some(row) => rows.push(row),
            None => break,
        }
        pos = chunk_end;
    }
}

struct QuoteAwareBreaks<'a> {
    bytes: &'a [u8],
    i: usize,
    in_single: bool,
    in_double: bool,
}

impl<'a> QuoteAwareBreaks<'a> {
    fn new(line: &'a str) -> Self {
        Self {
            bytes: line.as_bytes(),
            i: 0,
            in_single: false,
            in_double: false,
        }
    }
}

impl Iterator for QuoteAwareBreaks<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        while self.i < self.bytes.len() {
            let c = *self.bytes.get(self.i)?;
            if self.in_single {
                if c == b'\'' {
                    self.in_single = false;
                }
                self.i += 1;
                continue;
            }
            if self.in_double {
                if c == b'\\' && self.i + 1 < self.bytes.len() {
                    self.i += 2;
                    continue;
                }
                if c == b'"' {
                    self.in_double = false;
                }
                self.i += 1;
                continue;
            }
            match c {
                b'\'' => {
                    self.in_single = true;
                    self.i += 1;
                }
                b'"' => {
                    self.in_double = true;
                    self.i += 1;
                }
                b if b.is_ascii_whitespace() => {
                    let start = self.i;
                    while self.i < self.bytes.len()
                        && self
                            .bytes
                            .get(self.i)
                            .is_some_and(|b| b.is_ascii_whitespace())
                    {
                        self.i += 1;
                    }
                    if start > 0 {
                        return Some(start);
                    }
                }
                _ => self.i += 1,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;

    #[test]
    fn options_are_chinese() {
        assert_eq!(OPTIONS[0].1, "允许一次");
        assert_eq!(OPTIONS[3].1, "始终拒绝");
    }

    #[test]
    fn fill_clears_leftover_glyphs() {
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        for y in 0..12 {
            buf.set_string(0, y, "resolver = \"2\" edition workspace", Style::default());
        }
        let prompt = PermissionPrompt {
            tool: "search_replace".into(),
            summary: "old_string / new_string".into(),
            agent_id: None,
        };
        render(&mut buf, area, &prompt, None, 0);
        let mut painted = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                painted.push_str(buf[(x, y)].symbol());
            }
        }
        let compact: String = painted
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '┃')
            .collect();
        assert!(!compact.contains("oncene"), "{compact}");
        assert!(!compact.contains("alwaysaid"), "{compact}");
        assert!(compact.contains("允许一次"), "{compact}");
        assert!(compact.contains("始终拒绝"), "{compact}");
    }

    /// 子代理发的请求在标题上方标出来源，选项照样画全。
    #[test]
    fn a_subagents_request_shows_where_it_came_from() {
        let prompt = PermissionPrompt {
            tool: "bash".into(),
            summary: "cargo test".into(),
            agent_id: Some("kid-1".into()),
        };
        let source = "来自子代理 · 探索 · 跑测试";
        let width = 50;
        let height = chrome_height(&prompt, Some(source), width);
        assert_eq!(height, chrome_height(&prompt, None, width) + 1);
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &prompt, Some(source), 0);
        let rows: Vec<String> = (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect();
        let at = |needle: &str| {
            rows.iter()
                .position(|r| r.replace(' ', "").contains(needle))
        };
        let src = at("来自子代理·探索·跑测试").expect("来源那一行");
        let title = at("允许使用bash").expect("标题");
        assert!(src < title, "来源在标题上方：{rows:#?}");
        assert!(at("始终拒绝").is_some(), "选项要画全：{rows:#?}");
    }

    /// 命令工具的摘要：命令在前、描述单起一行；命令太长时截在 3 行并标 `…`，
    /// 描述不挤占命令的行数。
    #[test]
    fn command_summary_puts_full_command_first_and_description_below() {
        let width = 40;
        let rows = summary_rows("$ cargo test -p cordis-tui\nRun the TUI tests", width);
        assert_eq!(
            rows,
            vec![
                ("$ cargo test -p cordis-tui".to_string(), false),
                ("Run the TUI tests".to_string(), true),
            ]
        );

        let long = format!("$ {}", "word ".repeat(40));
        let rows = summary_rows(&format!("{long}\n描述"), width);
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert!(rows[2].0.ends_with('\u{2026}'), "{rows:?}");
        assert_eq!(rows[3], ("描述".to_string(), true));

        // 单行摘要（其它工具）照旧。
        let rows = summary_rows("write_file {\"file_path\":\"a\"}", width);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].1);
    }

    #[test]
    fn bash_quote_aware_wrap_keeps_single_quoted_span_together() {
        let line = "prefix_ok_here '.[] | not a pipe' trailing_words_here_too";
        let width = 20;
        let rows = bash_quote_aware_wrap(line, width, usize::MAX);
        let has_split_inside_quotes = rows
            .iter()
            .any(|r| r.contains(".[]") && !r.contains("not a pipe"));
        assert!(!has_split_inside_quotes, "split inside quotes: {rows:?}");
        assert!(
            rows.iter().any(|r| r.contains("'.[] | not a pipe'")),
            "quoted span must be intact in some row: {rows:?}"
        );
    }

    #[test]
    fn bash_quote_aware_wrap_force_breaks_overwide_single_quoted_arg() {
        let payload = "a".repeat(200);
        let line = format!("ssh host '{payload}'");
        let width = 40;
        let rows = bash_quote_aware_wrap(&line, width, usize::MAX);
        for r in &rows {
            assert!(UnicodeWidthStr::width(*r) <= width, "{r:?}");
        }
        assert_eq!(rows[0], "ssh host");
        assert_eq!(
            rows.get(1..).unwrap_or(&[]).concat(),
            format!("'{payload}'")
        );
    }
}
