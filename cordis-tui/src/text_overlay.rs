//! Read-only title + body overlay (`/cordis` `/lsp` `/skills`, extra slash
//! overlays, TUI slots). Chrome matches MCP: title + divider, then a styled
//! listing rather than a monochrome dump.

use std::sync::Mutex;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_str;
use crate::grok::picker::{render_divider, render_floating_frame_height, PickerHits};
use crate::grok::wrapping::word_wrap_lines;
use crate::theme::Theme;

const PAD: u16 = 1;
const TITLE_ROWS: u16 = 2;
const MIN_BODY_ROWS: u16 = 4;
const MAX_PICKER_INNER: u16 = 22;

struct Layout {
    body: String,
    viewport: u16,
    lines: Vec<Line<'static>>,
}

static LAYOUT: Mutex<Option<Layout>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Blank,
    Kicker,
    Section,
    Empty,
    Bullet,
    Path,
    Item,
    Code,
    Body,
}

pub fn render(buf: &mut Buffer, area: Rect, title: &str, body: &str, scroll: usize) -> PickerHits {
    let theme = Theme::current();
    let text_w = estimate_text_width(area);
    let styled = style_body(body, &theme);
    let wrapped = wrap_body(styled, text_w);
    let inner_rows = TITLE_ROWS
        .saturating_add(wrapped.len().max(MIN_BODY_ROWS as usize) as u16)
        .saturating_add(1)
        .min(MAX_PICKER_INNER);
    let Some(frame) = render_floating_frame_height(buf, area, &theme, false, inner_rows + 2) else {
        return PickerHits::default();
    };
    let inner = frame.content;
    if inner.height < 2 || inner.width < 10 {
        return PickerHits {
            close_button: frame.close_button,
            ..Default::default()
        };
    }
    paint_title(buf, inner, &theme, title, frame.close_button);
    if inner.height >= 2 {
        render_divider(
            buf,
            inner.x,
            inner.y + 1,
            inner.width,
            &theme,
            Some(theme.bg_base),
        );
    }
    let body_area = Rect {
        x: inner.x + PAD,
        y: inner.y.saturating_add(TITLE_ROWS),
        width: inner.width.saturating_sub(PAD.saturating_mul(2)),
        height: inner.height.saturating_sub(TITLE_ROWS),
    };
    if body_area.height == 0 || body_area.width < 4 {
        return PickerHits {
            close_button: frame.close_button,
            ..Default::default()
        };
    }
    let lines = if body_area.width == text_w {
        wrapped
    } else {
        wrap_body(style_body(body, &theme), body_area.width)
    };
    {
        let mut memo = LAYOUT.lock().unwrap();
        *memo = Some(Layout {
            body: body.to_string(),
            viewport: body_area.height,
            lines: lines.clone(),
        });
    }
    let visible: Vec<Line<'static>> = lines
        .into_iter()
        .skip(scroll)
        .take(body_area.height as usize)
        .collect();
    Paragraph::new(visible).render(body_area, buf);
    PickerHits {
        close_button: frame.close_button,
        ..Default::default()
    }
}

pub fn max_scroll(body: &str, viewport: usize) -> usize {
    let memo = LAYOUT.lock().unwrap();
    if let Some(m) = memo.as_ref() {
        if m.body == body {
            return m.lines.len().saturating_sub(m.viewport.max(1) as usize);
        }
    }
    body.lines().count().saturating_sub(viewport.max(1))
}

fn estimate_text_width(area: Rect) -> u16 {
    let popup_w = ((area.width as f32 * 0.65) as u16).max(44).min(area.width);
    popup_w
        .saturating_sub(2)
        .saturating_sub(PAD.saturating_mul(2))
        .max(8)
}

fn paint_title(buf: &mut Buffer, inner: Rect, theme: &Theme, title: &str, close: Rect) {
    let reserve = if close.width == 0 { 0 } else { close.width + 1 };
    let budget = inner.width.saturating_sub(PAD + reserve) as usize;
    let shown = truncate_str(title, budget);
    buf.set_line(
        inner.x + PAD,
        inner.y,
        &Line::from(Span::styled(
            shown,
            Style::default()
                .fg(theme.text_primary)
                .bg(theme.bg_base)
                .add_modifier(Modifier::BOLD),
        )),
        inner.width.saturating_sub(PAD + reserve),
    );
}

fn wrap_body(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let w = (width as usize).max(8);
    word_wrap_lines(lines, w)
}

fn style_body(body: &str, theme: &Theme) -> Vec<Line<'static>> {
    let mut first_content = true;
    body.lines()
        .map(|raw| {
            let kind = classify(raw, first_content);
            if kind != Kind::Blank {
                first_content = false;
            }
            style_line(raw, kind, theme)
        })
        .collect()
}

fn classify(line: &str, first_content: bool) -> Kind {
    if line.trim().is_empty() {
        return Kind::Blank;
    }
    let indent = leading_spaces(line);
    let trimmed = line.trim();
    if is_empty_state(trimmed) {
        return Kind::Empty;
    }
    if looks_like_code(trimmed) {
        return Kind::Code;
    }
    if looks_like_path_line(trimmed) {
        return Kind::Path;
    }
    if indent == 0 && is_section_header(trimmed) {
        return Kind::Section;
    }
    if first_content && indent == 0 && is_kicker(trimmed) {
        return Kind::Kicker;
    }
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        return Kind::Bullet;
    }
    if indent >= 2 {
        return Kind::Item;
    }
    Kind::Body
}

fn is_section_header(trimmed: &str) -> bool {
    let stripped = trimmed
        .trim_end_matches('：')
        .trim_end_matches(':')
        .trim_end();
    if stripped.is_empty() || stripped == trimmed {
        return false;
    }
    !stripped.contains('。')
}

fn is_kicker(trimmed: &str) -> bool {
    trimmed.width() <= 16 && !trimmed.contains('。') && !trimmed.contains('：')
}

fn is_empty_state(trimmed: &str) -> bool {
    let inner = strip_wrappers(trimmed);
    inner != trimmed
        && (inner.starts_with("无")
            || inner.starts_with("还没有")
            || inner.contains("还没有")
            || inner == "无")
}

fn looks_like_path_line(trimmed: &str) -> bool {
    if trimmed.starts_with("~/") || trimmed.starts_with('.') {
        return trimmed.contains('/');
    }
    if let Some(after) = trimmed.strip_prefix('/') {
        return after.contains('/') || after.contains('.');
    }
    trimmed.contains('/') && !trimmed.contains(' ')
}

fn looks_like_code(trimmed: &str) -> bool {
    (trimmed.contains("host.") || trimmed.contains("ctx."))
        && (trimmed.contains('(') && trimmed.contains(')'))
}

fn style_line(raw: &str, kind: Kind, theme: &Theme) -> Line<'static> {
    match kind {
        Kind::Blank => Line::from(""),
        Kind::Kicker => Line::from(Span::styled(
            raw.trim().to_string(),
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        )),
        Kind::Section => {
            let label = raw
                .trim()
                .trim_end_matches('：')
                .trim_end_matches(':')
                .trim_end();
            Line::from(vec![
                Span::styled(
                    format!("{} ", glyphs::diamond_filled()),
                    Style::default().fg(theme.gray_bright),
                ),
                Span::styled(
                    label.to_string(),
                    Style::default()
                        .fg(theme.gray_bright)
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        }
        Kind::Empty => {
            let inner = strip_wrappers(raw.trim());
            let mut spans = vec![Span::styled(
                format!("{}  ", glyphs::diamond_hollow()),
                theme.dim(),
            )];
            spans.extend(tokenize(inner, theme.dim(), theme));
            Line::from(spans)
        }
        Kind::Bullet => {
            let rest = raw
                .trim()
                .trim_start_matches('-')
                .trim_start_matches('*')
                .trim_start();
            let mut spans = vec![Span::styled(
                format!("{} ", glyphs::diamond_filled()),
                Style::default().fg(theme.gray_dim),
            )];
            spans.extend(tokenize(
                rest,
                Style::default().fg(theme.text_primary),
                theme,
            ));
            Line::from(spans)
        }
        Kind::Path => {
            let indent = leading_spaces(raw).min(4);
            let mut spans = vec![Span::raw(" ".repeat(indent))];
            spans.extend(tokenize(raw.trim(), Style::default().fg(theme.path), theme));
            Line::from(spans)
        }
        Kind::Item => {
            let mut spans = vec![
                Span::raw("  "),
                Span::styled(
                    format!("{} ", glyphs::diamond_filled()),
                    Style::default().fg(theme.gray_dim),
                ),
            ];
            spans.extend(tokenize(
                raw.trim(),
                Style::default().fg(theme.text_primary),
                theme,
            ));
            Line::from(spans)
        }
        Kind::Code => {
            let mut spans = Vec::new();
            spans.extend(tokenize(
                raw.trim(),
                Style::default().fg(theme.md_code),
                theme,
            ));
            Line::from(spans)
        }
        Kind::Body => {
            let indent = leading_spaces(raw);
            let mut spans = Vec::new();
            if indent > 0 {
                spans.push(Span::raw(" ".repeat(indent)));
            }
            spans.extend(tokenize(
                raw.trim_start(),
                Style::default().fg(theme.text_secondary),
                theme,
            ));
            Line::from(spans)
        }
    }
}

fn tokenize(s: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut Vec<Span<'static>>| {
        if !plain.is_empty() {
            out.push(Span::styled(std::mem::take(plain), base));
        }
    };
    while i < chars.len() {
        let (byte, ch) = chars[i];
        if ch == '`' {
            if let Some(end) = find_char(&chars, i + 1, '`') {
                flush(&mut plain, &mut out);
                let end_byte = chars[end].0 + '`'.len_utf8();
                out.push(Span::styled(
                    s[byte..end_byte].to_string(),
                    Style::default().fg(theme.md_code),
                ));
                i = end + 1;
                continue;
            }
        }
        if ch == '"' {
            if let Some(end) = find_char(&chars, i + 1, '"') {
                flush(&mut plain, &mut out);
                let end_byte = chars[end].0 + 1;
                out.push(Span::styled(
                    s[byte..end_byte].to_string(),
                    Style::default().fg(theme.command),
                ));
                i = end + 1;
                continue;
            }
        }
        if ch == '[' {
            if let Some(end) = find_char(&chars, i + 1, ']') {
                let inner = &s[chars[i + 1].0..chars[end].0];
                if is_bracket_token(inner) {
                    flush(&mut plain, &mut out);
                    let end_byte = chars[end].0 + 1;
                    out.push(Span::styled(
                        s[byte..end_byte].to_string(),
                        badge_style(inner, theme),
                    ));
                    i = end + 1;
                    continue;
                }
            }
        }
        if ch == '(' {
            if let Some(end) = find_char(&chars, i + 1, ')') {
                let inner = &s[chars[i + 1].0..chars[end].0];
                if is_status_word(inner) {
                    flush(&mut plain, &mut out);
                    let end_byte = chars[end].0 + 1;
                    out.push(Span::styled(
                        s[byte..end_byte].to_string(),
                        badge_style(inner, theme),
                    ));
                    i = end + 1;
                    continue;
                }
            }
        }
        if ch == '/' {
            let prev_space = i == 0 || chars[i - 1].1.is_whitespace();
            let next_ch = chars.get(i + 1).map(|c| c.1);
            let next_space = next_ch.is_none_or(|c| c.is_whitespace());
            if prev_space && !next_space && next_ch.is_some_and(|c| c.is_ascii_alphabetic()) {
                flush(&mut plain, &mut out);
                let rest = &s[byte..];
                if looks_like_abs_path(rest) {
                    let (n, text) = take_path(rest);
                    out.push(Span::styled(text, Style::default().fg(theme.path)));
                    i += n;
                    continue;
                }
                let (n, text) = take_slash_cmd(rest);
                out.push(Span::styled(text, Style::default().fg(theme.accent_skill)));
                i += n;
                continue;
            }
        }
        if ch == '~' || (ch == '.' && next_is_pathish(&chars, i)) {
            let rest = &s[byte..];
            let (n, text) = take_path(rest);
            if n > 1 && text.contains('/') {
                flush(&mut plain, &mut out);
                out.push(Span::styled(text, Style::default().fg(theme.path)));
                i += n;
                continue;
            }
        }
        if ch == '→' {
            flush(&mut plain, &mut out);
            out.push(Span::styled(
                "→".to_string(),
                Style::default().fg(theme.gray_bright),
            ));
            i += 1;
            continue;
        }
        if ch == '-' && chars.get(i + 1).map(|c| c.1) == Some('>') {
            flush(&mut plain, &mut out);
            out.push(Span::styled(
                "->".to_string(),
                Style::default().fg(theme.gray_bright),
            ));
            i += 2;
            continue;
        }
        if ch == '·' || ch == '—' {
            flush(&mut plain, &mut out);
            out.push(Span::styled(ch.to_string(), theme.dim()));
            i += 1;
            continue;
        }
        if is_ident_start(ch) {
            let rest = &s[byte..];
            let (mut n, mut ident) = take_ident(rest);
            let after = i + n;
            if chars.get(after).map(|c| c.1) == Some('.')
                && chars.get(after + 1).is_some_and(|c| is_ident_start(c.1))
                && (ident == "host" || ident == "ctx" || ident.contains('_'))
            {
                let (n2, ident2) = take_ident(&s[chars[after + 1].0..]);
                ident.push('.');
                ident.push_str(&ident2);
                n += 1 + n2;
                flush(&mut plain, &mut out);
                out.push(Span::styled(ident, Style::default().fg(theme.md_code)));
                i += n;
                continue;
            }
            if ident.contains('_') && ident.len() >= 3 {
                flush(&mut plain, &mut out);
                out.push(Span::styled(ident, Style::default().fg(theme.command)));
                i += n;
                continue;
            }
            plain.push_str(&ident);
            i += n;
            continue;
        }
        plain.push(ch);
        i += 1;
    }
    flush(&mut plain, &mut out);
    out
}

fn badge_style(inner: &str, theme: &Theme) -> Style {
    match inner {
        "运行中" | "就绪" | "已添加" | "已写入" | "已保留" => {
            Style::default().fg(theme.accent_success)
        }
        "已禁用" | "已停止" | "无" | "不可斜杠" => theme.muted(),
        "需认证" | "已暂停" | "PATH 上没有" => Style::default().fg(theme.warning),
        "不可用" => Style::default().fg(theme.accent_error),
        _ => Style::default().fg(theme.gray_bright),
    }
}

fn is_status_word(inner: &str) -> bool {
    matches!(
        inner,
        "运行中" | "已停止" | "已禁用" | "无" | "就绪" | "不可用" | "需认证" | "已暂停"
    )
}

fn is_bracket_token(inner: &str) -> bool {
    let n = inner.chars().count();
    n > 0 && n <= 16 && !inner.contains('[') && !inner.contains('\n')
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn next_is_pathish(chars: &[(usize, char)], i: usize) -> bool {
    matches!(
        chars.get(i + 1).map(|c| c.1),
        Some('d') | Some('/') | Some('.')
    ) && chars.get(i + 2).is_some()
}

fn looks_like_abs_path(rest: &str) -> bool {
    let after = rest.trim_start_matches('/');
    after.contains('/') || after.contains('.')
}

fn take_path(rest: &str) -> (usize, String) {
    take_while(rest, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '~' | '<' | '>' | '@')
    })
}

fn take_slash_cmd(rest: &str) -> (usize, String) {
    let mut chars = rest.chars();
    let Some(slash) = chars.next() else {
        return (0, String::new());
    };
    if slash != '/' {
        return (0, String::new());
    }
    let mut n = 1usize;
    let mut out = String::from("/");
    for ch in chars {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
            n += 1;
        } else {
            break;
        }
    }
    (n, out)
}

fn take_ident(rest: &str) -> (usize, String) {
    take_while(rest, is_ident_char)
}

fn take_while(rest: &str, pred: impl Fn(char) -> bool) -> (usize, String) {
    let mut n = 0usize;
    let mut end = 0usize;
    for (i, ch) in rest.char_indices() {
        if !pred(ch) {
            break;
        }
        n += 1;
        end = i + ch.len_utf8();
    }
    (n, rest[..end].to_string())
}

fn find_char(chars: &[(usize, char)], from: usize, needle: char) -> Option<usize> {
    chars[from..]
        .iter()
        .position(|(_, c)| *c == needle)
        .map(|p| from + p)
}

fn leading_spaces(s: &str) -> usize {
    s.chars().take_while(|c| *c == ' ').count()
}

fn strip_wrappers(trimmed: &str) -> &str {
    if trimmed.starts_with('（') && trimmed.ends_with('）') {
        let mut chars = trimmed.chars();
        chars.next();
        let rest = chars.as_str();
        return rest.strip_suffix('）').unwrap_or(rest);
    }
    if let Some(inner) = trimmed.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        return inner;
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::groknight()
    }

    fn span_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn all_text(body: &str) -> String {
        style_body(body, &theme())
            .into_iter()
            .map(|l| span_text(&l))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn has_fg(line: &Line<'_>, needle: &str, color: ratatui::style::Color) -> bool {
        line.spans
            .iter()
            .any(|s| s.content.contains(needle) && s.style.fg == Some(color))
    }

    #[test]
    fn section_headers_use_diamond_and_drop_colon() {
        let line = style_line("永久（磁盘）：", Kind::Section, &theme());
        let text = span_text(&line);
        assert!(text.contains("永久（磁盘）"), "{text}");
        assert!(!text.contains('：'), "{text}");
        assert!(text.contains(glyphs::diamond_filled()), "{text}");
    }

    #[test]
    fn intro_sentences_are_not_sections() {
        assert_eq!(
            classify(
                "会话插件只在当前进程：cordis_define → cordis_run。写成永久：cordis_promote。",
                true
            ),
            Kind::Body
        );
        assert_eq!(classify("永久（磁盘）：", false), Kind::Section);
    }

    #[test]
    fn empty_states_are_dim_diamonds() {
        assert_eq!(classify("  （无）", false), Kind::Empty);
        assert_eq!(
            classify(
                "  （还没有。目录：项目 .dock/plugins/<id>/ ，用户 ~/.dock/plugins/<id>/）",
                false
            ),
            Kind::Empty
        );
        let line = style_line("  （无）", Kind::Empty, &theme());
        let text = span_text(&line);
        assert!(text.contains(glyphs::diamond_hollow()), "{text}");
        assert!(text.contains('无'), "{text}");
        assert!(!text.contains('（'), "{text}");
    }

    #[test]
    fn first_line_is_not_blindly_bold_body() {
        let lines = style_body("永久插件写在磁盘上。\n永久（磁盘）：", &theme());
        assert_eq!(lines.len(), 2);
        assert!(
            !lines[0]
                .spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD)),
            "{:?}",
            lines[0]
        );
        assert!(
            lines[1]
                .spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD)),
            "{:?}",
            lines[1]
        );
    }

    #[test]
    fn highlights_tools_paths_and_status() {
        let t = theme();
        let line = style_line("  echo [project] Echo — ping (运行中)", Kind::Item, &t);
        assert!(has_fg(&line, "[project]", t.gray_bright), "{line:?}");
        assert!(has_fg(&line, "(运行中)", t.accent_success), "{line:?}");
        let body = style_line(
            "写成永久：cordis_promote。目录 .dock/plugins/<id>/",
            Kind::Body,
            &t,
        );
        assert!(has_fg(&body, "cordis_promote", t.command), "{body:?}");
        assert!(has_fg(&body, ".dock/plugins/<id>/", t.path), "{body:?}");
    }

    #[test]
    fn slash_and_host_on_are_accented() {
        let t = theme();
        let slash = style_line("/skills  ·  项目  ·  说明", Kind::Body, &t);
        assert!(has_fg(&slash, "/skills", t.accent_skill), "{slash:?}");
        let code = style_line(
            r#"host.on("session/event", |line| { … }) 观察 user / assistant / tool 行。"#,
            Kind::Code,
            &t,
        );
        assert!(has_fg(&code, "host.on", t.md_code), "{code:?}");
        assert!(has_fg(&code, "\"session/event\"", t.command), "{code:?}");
    }

    #[test]
    fn listing_keeps_words() {
        let text = all_text(
            "永久插件写在磁盘上，重启后自动加载（启动不走权限 overlay）。\n\n永久（磁盘）：\n  （无）",
        );
        assert!(text.contains("永久插件写在磁盘上"), "{text}");
        assert!(text.contains("永久（磁盘）"), "{text}");
        assert!(text.contains('无'), "{text}");
    }

    #[test]
    fn render_paints_title_and_section() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        let hits = render(
            &mut buf,
            Rect::new(0, 0, 80, 24),
            "Cordis 插件",
            "永久（磁盘）：\n  （无）",
            0,
        );
        assert!(hits.close_button.width > 0);
        let dumped: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(dumped.contains("Cordis"), "{dumped}");
        assert!(dumped.contains("永"), "{dumped}");
        assert!(dumped.contains("盘"), "{dumped}");
        assert!(dumped.contains("无"), "{dumped}");
    }
}
