//! 插件视图（dock.view.1，见 `docs/PLUGIN-VIEWS.md`）在终端里的画法：一棵树压成
//! 带样式的行。看得见的可点的东西（按钮、带动作的列表行、空状态按钮）按出现顺序
//! 编号 1–9，面板里按数字键就是点它；折叠段里的不编号。

use cordis_base::view::{ButtonStyle, Size, Tone, ViewNode};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

/// 最多编号几个动作（数字键 1–9）。
pub const MAX_NUMBERED: usize = 9;

/// 数字键 `n`（1 起）对应的动作 id。
pub fn action_for_digit(node: &ViewNode, n: usize) -> Option<String> {
    if n == 0 || n > MAX_NUMBERED {
        return None;
    }
    node.visible_actions().into_iter().nth(n - 1)
}

fn tone_style(tone: Tone, theme: &Theme) -> Style {
    let fg = match tone {
        Tone::Default => theme.text_primary,
        Tone::Muted => theme.gray,
        Tone::Accent => theme.running,
        Tone::Success => theme.accent_success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.accent_error,
    };
    Style::default().fg(fg)
}

struct Painter<'a> {
    theme: &'a Theme,
    width: usize,
    numbers: Vec<String>,
    out: Vec<Line<'static>>,
}

impl Painter<'_> {
    fn number(&self, action: &str) -> Option<usize> {
        self.numbers
            .iter()
            .position(|a| a == action)
            .map(|i| i + 1)
            .filter(|n| *n <= MAX_NUMBERED)
    }

    fn key_span(&self, action: &str) -> Span<'static> {
        match self.number(action) {
            Some(n) => Span::styled(
                format!("[{n}] "),
                Style::default().fg(self.theme.gray_bright),
            ),
            None => Span::raw(""),
        }
    }

    fn muted(&self, text: impl Into<String>) -> Span<'static> {
        Span::styled(text.into(), Style::default().fg(self.theme.gray))
    }

    fn indented(&mut self, indent: usize, line: Line<'static>) {
        let mut spans = vec![Span::raw(" ".repeat(indent))];
        spans.extend(line.spans);
        self.out.push(Line::from(spans));
    }

    fn node(&mut self, node: &ViewNode, indent: usize) {
        let theme = self.theme;
        match node {
            ViewNode::Stack { children, gap } => {
                for (i, child) in children.iter().enumerate() {
                    if i > 0 && *gap == Size::L {
                        self.out.push(Line::default());
                    }
                    self.node(child, indent);
                }
            }
            ViewNode::Row { children, between } => {
                // 一行放得下就并排；有多行的子节点就退回竖排。
                let mut spans: Vec<Span<'static>> = Vec::new();
                let mut fits = true;
                for child in children {
                    let mut sub = Painter {
                        theme,
                        width: self.width,
                        numbers: self.numbers.clone(),
                        out: Vec::new(),
                    };
                    sub.node(child, 0);
                    if sub.out.len() > 1 {
                        fits = false;
                        break;
                    }
                    if !spans.is_empty() {
                        spans.push(Span::raw(if *between { "    " } else { "  " }));
                    }
                    if let Some(line) = sub.out.pop() {
                        spans.extend(line.spans);
                    }
                }
                let width: usize = spans.iter().map(|s| s.content.width()).sum();
                if fits && width + indent <= self.width {
                    self.indented(indent, Line::from(spans));
                } else {
                    for child in children {
                        self.node(child, indent);
                    }
                }
            }
            ViewNode::Section {
                title,
                children,
                collapsed,
            } => {
                let marker = if *collapsed { "▸ " } else { "▾ " };
                self.indented(
                    indent,
                    Line::from(vec![
                        self.muted(marker),
                        Span::styled(
                            title.clone(),
                            Style::default()
                                .fg(theme.text_primary)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                );
                if !collapsed {
                    for child in children {
                        self.node(child, indent + 2);
                    }
                }
            }
            ViewNode::Text {
                text, tone, bold, ..
            } => {
                let mut style = tone_style(*tone, theme);
                if *bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                for line in text.lines() {
                    self.indented(indent, Line::from(Span::styled(line.to_string(), style)));
                }
            }
            ViewNode::Markdown { text } => {
                let width = self.width.saturating_sub(indent).max(8);
                for line in crate::scrollback::markdown_lines_with(text, theme, width) {
                    self.indented(indent, line);
                }
            }
            ViewNode::Code { text, .. } => {
                let style = Style::default().fg(theme.text_secondary).bg(theme.bg_dark);
                for line in text.lines() {
                    self.indented(indent, Line::from(Span::styled(format!(" {line} "), style)));
                }
            }
            ViewNode::Kv { items } => {
                let label_w = items.iter().map(|i| i.label.width()).max().unwrap_or(0);
                for item in items {
                    let pad = label_w.saturating_sub(item.label.width());
                    self.indented(
                        indent,
                        Line::from(vec![
                            self.muted(format!("{}{}  ", item.label, " ".repeat(pad))),
                            Span::styled(item.value.clone(), tone_style(item.tone, theme)),
                        ]),
                    );
                }
            }
            ViewNode::Table { columns, rows } => {
                let cols = columns
                    .len()
                    .max(rows.iter().map(Vec::len).max().unwrap_or(0));
                if cols == 0 {
                    return;
                }
                let budget = self.width.saturating_sub(indent + 2 * (cols - 1)).max(cols);
                let cap = (budget / cols).max(4);
                let width_of = |c: usize| {
                    std::iter::once(columns.get(c))
                        .chain(rows.iter().map(|r| r.get(c)))
                        .flatten()
                        .map(|s| s.width())
                        .max()
                        .unwrap_or(0)
                        .min(cap)
                };
                let widths: Vec<usize> = (0..cols).map(width_of).collect();
                let cell = |s: &str, w: usize| {
                    let clipped = crate::grok::line_utils::truncate_str(s, w);
                    let pad = w.saturating_sub(clipped.width());
                    format!("{clipped}{}", " ".repeat(pad))
                };
                if !columns.is_empty() {
                    let spans: Vec<Span<'static>> = (0..cols)
                        .map(|c| {
                            Span::styled(
                                format!(
                                    "{}{}",
                                    cell(columns.get(c).map_or("", String::as_str), widths[c]),
                                    if c + 1 < cols { "  " } else { "" }
                                ),
                                Style::default()
                                    .fg(theme.text_primary)
                                    .add_modifier(Modifier::BOLD),
                            )
                        })
                        .collect();
                    self.indented(indent, Line::from(spans));
                    let rule: usize = widths.iter().sum::<usize>() + 2 * (cols - 1);
                    self.indented(indent, Line::from(self.muted("─".repeat(rule))));
                }
                for row in rows {
                    let text: String = (0..cols)
                        .map(|c| {
                            format!(
                                "{}{}",
                                cell(row.get(c).map_or("", String::as_str), widths[c]),
                                if c + 1 < cols { "  " } else { "" }
                            )
                        })
                        .collect();
                    self.indented(indent, Line::from(Span::raw(text)));
                }
            }
            ViewNode::List { items } => {
                for item in items {
                    let mut spans = Vec::new();
                    match &item.action {
                        Some(a) => spans.push(self.key_span(a)),
                        None => spans.push(self.muted("• ")),
                    }
                    spans.push(Span::styled(
                        item.title.clone(),
                        Style::default().fg(theme.text_primary),
                    ));
                    if let Some(b) = &item.badge {
                        spans.push(Span::raw("  "));
                        spans.push(Span::styled(
                            format!("[{}]", b.text),
                            tone_style(b.tone, theme),
                        ));
                    }
                    self.indented(indent, Line::from(spans));
                    if let Some(sub) = &item.subtitle {
                        self.indented(indent + 2, Line::from(self.muted(sub.clone())));
                    }
                }
            }
            ViewNode::Badge(b) => {
                self.indented(
                    indent,
                    Line::from(Span::styled(
                        format!("[{}]", b.text),
                        tone_style(b.tone, theme),
                    )),
                );
            }
            ViewNode::Progress { value, label } => {
                const CELLS: usize = 20;
                let (bar, pct) = match value {
                    Some(v) => {
                        let filled = ((*v * CELLS as f64).round() as usize).min(CELLS);
                        (
                            format!("{}{}", "█".repeat(filled), "░".repeat(CELLS - filled)),
                            format!(" {:.0}%", v * 100.0),
                        )
                    }
                    None => ("░░▒▓▒░".to_string(), " …".to_string()),
                };
                let mut spans = Vec::new();
                if let Some(l) = label {
                    spans.push(Span::raw(format!("{l}  ")));
                }
                spans.push(Span::styled(bar, Style::default().fg(theme.running)));
                spans.push(self.muted(pct));
                self.indented(indent, Line::from(spans));
            }
            ViewNode::Button {
                label,
                action,
                style,
            } => {
                let look = match style {
                    ButtonStyle::Primary => Style::default()
                        .fg(theme.text_primary)
                        .add_modifier(Modifier::BOLD),
                    ButtonStyle::Secondary => Style::default().fg(theme.text_secondary),
                    ButtonStyle::Danger => Style::default().fg(theme.accent_error),
                };
                self.indented(
                    indent,
                    Line::from(vec![
                        self.key_span(action),
                        Span::styled(label.clone(), look),
                    ]),
                );
            }
            ViewNode::Link { label, url } => {
                self.indented(
                    indent,
                    Line::from(vec![
                        Span::raw(format!("{label} ")),
                        Span::styled(
                            url.clone(),
                            Style::default()
                                .fg(theme.running)
                                .add_modifier(Modifier::UNDERLINED),
                        ),
                    ]),
                );
            }
            ViewNode::Divider => {
                let w = self.width.saturating_sub(indent).min(40);
                self.indented(indent, Line::from(self.muted("─".repeat(w))));
            }
            ViewNode::Empty {
                title,
                text,
                action,
                label,
            } => {
                self.indented(
                    indent,
                    Line::from(Span::styled(
                        title.clone(),
                        Style::default()
                            .fg(theme.text_primary)
                            .add_modifier(Modifier::BOLD),
                    )),
                );
                if let Some(t) = text {
                    self.indented(indent, Line::from(self.muted(t.clone())));
                }
                if let Some(a) = action {
                    let text = label.clone().unwrap_or_else(|| a.clone());
                    self.indented(indent, Line::from(vec![self.key_span(a), Span::raw(text)]));
                }
            }
            ViewNode::Unsupported { kind } => {
                self.indented(
                    indent,
                    Line::from(self.muted(format!("不支持的视图：{kind}"))),
                );
            }
        }
    }
}

/// 把一棵视图树画成行（还没按宽度折行，交给浮层统一折）。
pub fn lines(node: &ViewNode, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let mut painter = Painter {
        theme,
        width: width.max(8),
        numbers: node.visible_actions(),
        out: Vec::new(),
    };
    painter.node(node, 0);
    painter.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(lines: &[Line<'static>]) -> String {
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

    #[test]
    fn numbers_actions_and_maps_digits() {
        let node = ViewNode::parse(&json!({ "type": "stack", "children": [
            { "type": "kv", "items": [{ "label": "环境", "value": "production" }, { "label": "版本号", "value": "v2" }] },
            { "type": "progress", "value": 0.6, "label": "上传中" },
            { "type": "row", "children": [
                { "type": "button", "label": "部署", "action": "deploy", "style": "primary" },
                { "type": "button", "label": "回滚", "action": "rollback" }
            ]},
            { "type": "list", "items": [{ "title": "v2.4.1", "action": "open:1", "badge": { "text": "成功", "tone": "success" } }] },
            { "type": "chart" }
        ]}));
        let out = text(&lines(&node, &Theme::groknight(), 60));
        assert!(out.contains("环境    production"), "{out}");
        assert!(out.contains("上传中  ████████████░░░░░░░░ 60%"), "{out}");
        assert!(out.contains("[1] 部署  [2] 回滚"), "{out}");
        assert!(out.contains("[3] v2.4.1  [成功]"), "{out}");
        assert!(out.contains("不支持的视图：chart"), "{out}");
        assert_eq!(action_for_digit(&node, 2).as_deref(), Some("rollback"));
        assert_eq!(action_for_digit(&node, 4), None);
    }

    /// 折叠段里的按钮不编号：数字键点不到看不见的动作。
    #[test]
    fn collapsed_actions_get_no_digit() {
        let node = ViewNode::parse(&json!({ "type": "stack", "children": [
            { "type": "section", "title": "危险", "collapsed": true, "children": [
                { "type": "button", "label": "删除", "action": "delete" }
            ]},
            { "type": "button", "label": "部署", "action": "deploy" }
        ]}));
        let out = text(&lines(&node, &Theme::groknight(), 60));
        assert!(out.contains("[1] 部署"), "{out}");
        assert!(!out.contains("删除"), "{out}");
        assert_eq!(action_for_digit(&node, 1).as_deref(), Some("deploy"));
        assert_eq!(action_for_digit(&node, 2), None);
    }

    #[test]
    fn tables_align_columns() {
        let node = ViewNode::parse(&json!({ "type": "table",
            "columns": ["服务", "状态"], "rows": [["api", "运行中"], ["worker-long", "停止"]] }));
        let out = text(&lines(&node, &Theme::groknight(), 60));
        let rows: Vec<&str> = out.lines().collect();
        assert_eq!(rows[0], "服务         状态  ");
        assert_eq!(rows[2], "api          运行中");
    }
}
