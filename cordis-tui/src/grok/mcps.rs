//! MCP servers overlay — Grok Extensions MCP-tab layout, Chinese copy.
//! Space toggles a server or a tool. `i` starts HTTP MCP browser OAuth.
//! Ctrl+R re-reads `config.toml` and reconciles (opening the pane does it too).
//! No grok.com connectors or marketplace.

use std::collections::HashSet;

use cordis_spine::McpStatus;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_str;
use crate::grok::picker::{render_divider, render_floating_frame, render_search_bar, PickerHits};
use crate::theme::Theme;

const TITLE: &str = "MCP 服务器";
const SECTION_LOCAL: &str = "本地";
const RIGHT_LOCAL: &str = "(本地)";
const BADGE_READY: &str = "[就绪]";
const BADGE_UNAVAILABLE: &str = "[不可用]";
const BADGE_DISABLED: &str = "[已禁用]";
const BADGE_NEEDS_AUTH: &str = "[需认证]";
const NO_TOOLS: &str = "没有工具（服务器可能未连接）";
const EMPTY: &str = "未配置 MCP 服务器";
const NO_MATCH: &str = "无匹配";
const FOOTER: &str = "Space 开关 · i 登录 · Ctrl+R 重载 · Enter 展开 · Esc 关闭";
const DESC_INDENT: u16 = 4;
const FOLD_WIDTH: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpRow {
    Section { count: usize, collapsed: bool },
    Server { index: usize },
    Tool { server: usize, tool: usize },
}

/// Next enabled state after Space on the highlighted row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpToggle {
    Server {
        name: String,
        enabled: bool,
    },
    Tool {
        server: String,
        tool: String,
        enabled: bool,
    },
}

pub fn tools_label(n: usize) -> String {
    if n == 0 {
        NO_TOOLS.to_string()
    } else {
        format!("{n} 个工具")
    }
}

pub fn badge_for(enabled: bool, ok: bool, needs_auth: bool) -> &'static str {
    if !enabled {
        BADGE_DISABLED
    } else if ok {
        BADGE_READY
    } else if needs_auth {
        BADGE_NEEDS_AUTH
    } else {
        BADGE_UNAVAILABLE
    }
}

fn matches(hay: &str, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    hay.to_ascii_lowercase().contains(&q.to_ascii_lowercase())
}

pub fn server_matches(server: &McpStatus, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    matches(&server.name, query)
        || matches(&server.command, query)
        || matches(&server.detail, query)
        || server.tools.iter().any(|t| {
            matches(&t.name, query)
                || matches(&t.public_name, query)
                || matches(&t.description, query)
        })
}

pub fn build_rows(
    servers: &[McpStatus],
    query: &str,
    tools_expanded: &HashSet<usize>,
    section_collapsed: bool,
) -> Vec<McpRow> {
    let filtered: Vec<usize> = servers
        .iter()
        .enumerate()
        .filter(|(_, s)| server_matches(s, query))
        .map(|(i, _)| i)
        .collect();
    if filtered.is_empty() {
        return Vec::new();
    }
    let searching = !query.is_empty();
    let collapsed = section_collapsed && !searching;
    let mut rows = vec![McpRow::Section {
        count: filtered.len(),
        collapsed,
    }];
    if collapsed {
        return rows;
    }
    for &si in &filtered {
        rows.push(McpRow::Server { index: si });
        if tools_open(si, tools_expanded, searching) {
            for ti in 0..servers[si].tools.len() {
                rows.push(McpRow::Tool {
                    server: si,
                    tool: ti,
                });
            }
        }
    }
    rows
}

pub fn toggle_row(
    row: &McpRow,
    tools_expanded: &mut HashSet<usize>,
    section_collapsed: &mut bool,
    searching: bool,
) {
    match row {
        McpRow::Section { .. } => {
            if !searching {
                *section_collapsed = !*section_collapsed;
            }
        }
        McpRow::Server { index } => {
            if !tools_expanded.remove(index) {
                tools_expanded.insert(*index);
            }
        }
        McpRow::Tool { .. } => {}
    }
}

/// Space target: server row toggles the server; tool row toggles that tool.
pub fn toggle_target(row: &McpRow, servers: &[McpStatus]) -> Option<McpToggle> {
    match row {
        McpRow::Section { .. } => None,
        McpRow::Server { index } => servers.get(*index).map(|s| McpToggle::Server {
            name: s.name.clone(),
            enabled: !s.enabled,
        }),
        McpRow::Tool { server, tool } => {
            let s = servers.get(*server)?;
            let t = s.tools.get(*tool)?;
            Some(McpToggle::Tool {
                server: s.name.clone(),
                tool: t.name.clone(),
                enabled: !t.enabled,
            })
        }
    }
}

/// `i` target: server row, or the parent of a tool row.
pub fn auth_target(row: &McpRow, servers: &[McpStatus]) -> Option<String> {
    match row {
        McpRow::Section { .. } => None,
        McpRow::Server { index } => servers.get(*index).map(|s| s.name.clone()),
        McpRow::Tool { server, .. } => servers.get(*server).map(|s| s.name.clone()),
    }
}

fn tools_open(index: usize, tools_expanded: &HashSet<usize>, searching: bool) -> bool {
    tools_expanded.contains(&index) || searching
}

fn visual_height(
    row: &McpRow,
    servers: &[McpStatus],
    tools_expanded: &HashSet<usize>,
    searching: bool,
) -> u16 {
    match row {
        McpRow::Section { .. } => 1,
        McpRow::Server { index } => {
            if tools_open(*index, tools_expanded, searching) {
                2
            } else {
                1
            }
        }
        McpRow::Tool { server, tool } => {
            let empty = servers
                .get(*server)
                .and_then(|s| s.tools.get(*tool))
                .map(|t| t.description.trim().is_empty())
                .unwrap_or(true);
            if empty {
                1
            } else {
                2
            }
        }
    }
}

fn empty_hint(has_servers: bool, query: &str) -> &'static str {
    if has_servers && !query.is_empty() {
        NO_MATCH
    } else {
        EMPTY
    }
}

fn server_desc(server: &McpStatus) -> String {
    if !server.detail.trim().is_empty() {
        server.detail.clone()
    } else {
        tools_label(server.tools.len())
    }
}

pub fn render_mcp_overlay(
    buf: &mut Buffer,
    area: Rect,
    servers: &[McpStatus],
    selected: usize,
    query: &str,
    tools_expanded: &HashSet<usize>,
    section_collapsed: bool,
) -> PickerHits {
    let theme = Theme::current();
    let Some(frame) = render_floating_frame(buf, area, &theme, false) else {
        return PickerHits::default();
    };
    let content = frame.content;
    if content.height <= 3 || content.width < 8 {
        return PickerHits {
            close_button: frame.close_button,
            ..Default::default()
        };
    }
    let title = Line::from(Span::styled(
        TITLE,
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    ));
    buf.set_line(
        content.x + 1,
        content.y,
        &title,
        content.width.saturating_sub(2),
    );
    let search_y = content.y + 1;
    render_search_bar(
        buf,
        content.x,
        search_y,
        content.width,
        &theme,
        query,
        true,
        Some(theme.bg_base),
    );
    render_divider(
        buf,
        content.x,
        search_y + 1,
        content.width,
        &theme,
        Some(theme.bg_base),
    );
    let list_y = search_y + 2;
    let list_bottom = content.y.saturating_add(content.height);
    let hint_h: u16 = 1;
    let list_h = list_bottom.saturating_sub(list_y).saturating_sub(hint_h);
    let rows = build_rows(servers, query, tools_expanded, section_collapsed);
    let searching = !query.is_empty();
    let mut hits = Vec::new();
    if rows.is_empty() {
        let empty = Line::from(Span::styled(
            empty_hint(!servers.is_empty(), query),
            Style::default().fg(theme.gray_bright),
        ));
        buf.set_line(
            content.x + 1,
            list_y,
            &empty,
            content.width.saturating_sub(2),
        );
    } else {
        let sel = selected.min(rows.len() - 1);
        let heights: Vec<u16> = rows
            .iter()
            .map(|r| visual_height(r, servers, tools_expanded, searching))
            .collect();
        let sel_y: u16 = heights[..sel].iter().copied().sum();
        let sel_h = heights[sel];
        let mut start_visual = 0u16;
        if sel_y + sel_h > start_visual + list_h {
            start_visual = (sel_y + sel_h).saturating_sub(list_h);
        }
        if sel_y < start_visual {
            start_visual = sel_y;
        }
        let mut acc = 0u16;
        let mut paint_y = list_y;
        let paint_end = list_y.saturating_add(list_h);
        for (idx, row) in rows.iter().enumerate() {
            let h = heights[idx];
            let row_end = acc + h;
            if row_end <= start_visual {
                acc = row_end;
                continue;
            }
            if acc >= start_visual + list_h || paint_y >= paint_end {
                break;
            }
            let skip = start_visual.saturating_sub(acc);
            let remain = paint_end.saturating_sub(paint_y);
            let drawn = paint_row(
                buf,
                content.x,
                paint_y,
                content.width,
                remain,
                skip,
                &theme,
                servers,
                row,
                tools_expanded,
                searching,
                idx == sel,
            );
            if drawn > 0 {
                hits.push((
                    idx,
                    Rect {
                        x: content.x,
                        y: paint_y,
                        width: content.width,
                        height: drawn,
                    },
                ));
                paint_y = paint_y.saturating_add(drawn);
            }
            acc = row_end;
        }
    }
    let hint_y = list_bottom.saturating_sub(1);
    if hint_y >= list_y {
        let hint = Line::from(Span::styled(FOOTER, Style::default().fg(theme.gray)));
        buf.set_line(
            content.x + 1,
            hint_y,
            &hint,
            content.width.saturating_sub(2),
        );
    }
    PickerHits {
        close_button: frame.close_button,
        rows: hits,
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    max_rows: u16,
    skip: u16,
    theme: &Theme,
    servers: &[McpStatus],
    row: &McpRow,
    tools_expanded: &HashSet<usize>,
    searching: bool,
    selected: bool,
) -> u16 {
    if max_rows == 0 {
        return 0;
    }
    let (indent, collapsible, expanded, label, badge, badge_color, right, desc, dimmed) = match row
    {
        McpRow::Section { count, collapsed } => (
            0u8,
            true,
            !*collapsed,
            format!("{SECTION_LOCAL} ({count})"),
            String::new(),
            None,
            String::new(),
            None,
            false,
        ),
        McpRow::Server { index } => {
            let server = &servers[*index];
            let expanded = tools_open(*index, tools_expanded, searching);
            let badge_fg = if !server.enabled {
                theme.gray
            } else if server.ok {
                theme.accent_success
            } else if server.needs_auth {
                theme.warning
            } else {
                theme.accent_error
            };
            (
                1,
                true,
                expanded,
                server.name.clone(),
                badge_for(server.enabled, server.ok, server.needs_auth).to_string(),
                Some(badge_fg),
                RIGHT_LOCAL.to_string(),
                if expanded {
                    Some(server_desc(server))
                } else {
                    None
                },
                !server.enabled,
            )
        }
        McpRow::Tool { server, tool } => {
            let t = &servers[*server].tools[*tool];
            let desc = t.description.trim();
            let (badge, badge_fg) = if t.enabled {
                (String::new(), None)
            } else {
                (BADGE_DISABLED.to_string(), Some(theme.gray))
            };
            (
                2,
                false,
                true,
                t.name.clone(),
                badge,
                badge_fg,
                String::new(),
                if desc.is_empty() {
                    None
                } else {
                    Some(desc.to_string())
                },
                !t.enabled,
            )
        }
    };

    let mut drawn = 0u16;
    if skip == 0 {
        paint_label_line(
            buf,
            x,
            y,
            width,
            theme,
            indent,
            collapsible,
            expanded,
            &label,
            &badge,
            badge_color,
            &right,
            selected,
            dimmed,
        );
        drawn += 1;
    }
    if let Some(desc) = desc {
        if drawn < max_rows && skip <= 1 {
            let dy = y + drawn;
            let desc_rect = Rect {
                x,
                y: dy,
                width,
                height: 1,
            };
            buf.set_style(desc_rect, Style::default().bg(theme.bg_base));
            let max_w = width.saturating_sub(DESC_INDENT) as usize;
            let text = truncate_str(&desc, max_w);
            buf.set_span(
                x + DESC_INDENT,
                dy,
                &Span::styled(text, Style::default().fg(theme.gray).bg(theme.bg_base)),
                width.saturating_sub(DESC_INDENT),
            );
            drawn += 1;
        }
    }
    drawn.min(max_rows)
}

#[allow(clippy::too_many_arguments)]
fn paint_label_line(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    theme: &Theme,
    indent: u8,
    collapsible: bool,
    expanded: bool,
    label: &str,
    badge: &str,
    badge_color: Option<ratatui::style::Color>,
    right: &str,
    selected: bool,
    dimmed: bool,
) {
    let row_bg = if selected {
        theme.bg_visual
    } else {
        theme.bg_base
    };
    buf.set_style(
        Rect {
            x,
            y,
            width,
            height: 1,
        },
        Style::default().bg(row_bg),
    );
    let label_fg = if dimmed {
        theme.gray
    } else {
        theme.text_primary
    };
    let mut label_style = Style::default().fg(label_fg).bg(row_bg);
    if selected {
        label_style = label_style.add_modifier(Modifier::BOLD);
    }
    let meta_fg = theme.gray;
    let indent_str = if indent > 0 {
        "  ".repeat(indent as usize)
    } else {
        String::new()
    };
    let fold = if collapsible && !expanded {
        format!("{} ", glyphs::chevron())
    } else {
        format!("{} ", glyphs::diamond_filled())
    };
    let badge_width = if badge.is_empty() {
        0u16
    } else {
        badge.width() as u16 + 1
    };
    let trailing_pad = 1u16;
    let prefix_width = indent_str.width() as u16 + FOLD_WIDTH;
    let fixed = prefix_width + trailing_pad + badge_width;
    let content_width = width.saturating_sub(fixed);
    let (max_label_width, truncated_right) = if right.is_empty() {
        (content_width as usize, String::new())
    } else {
        let gap = 2u16;
        let usable = content_width.saturating_sub(gap);
        let max_right = (usable / 2) as usize;
        let clipped = truncate_str(right, max_right);
        let right_cols = clipped.width() as u16;
        let max_label = usable.saturating_sub(right_cols) as usize;
        (max_label, clipped)
    };
    let truncated_label = truncate_str(label, max_label_width);
    let mut cur_x = x;
    if !indent_str.is_empty() {
        buf.set_span(
            cur_x,
            y,
            &Span::styled(&indent_str, label_style),
            indent_str.width() as u16,
        );
        cur_x += indent_str.width() as u16;
    }
    buf.set_span(
        cur_x,
        y,
        &Span::styled(
            fold,
            Style::default()
                .fg(if collapsible { meta_fg } else { theme.gray_dim })
                .bg(row_bg),
        ),
        FOLD_WIDTH,
    );
    cur_x += FOLD_WIDTH;
    buf.set_span(
        cur_x,
        y,
        &Span::styled(&truncated_label, label_style),
        truncated_label.width() as u16,
    );
    let full_label_width = prefix_width + truncated_label.width() as u16;
    if !badge.is_empty() {
        let badge_x = x + full_label_width + 1;
        let badge_fg = badge_color.unwrap_or(meta_fg);
        buf.set_span(
            badge_x,
            y,
            &Span::styled(badge, Style::default().fg(badge_fg).bg(row_bg)),
            badge.width() as u16,
        );
    }
    let right_width = truncated_right.width() as u16;
    if right_width > 0 {
        let right_x = x + width.saturating_sub(right_width + trailing_pad);
        buf.set_span(
            right_x,
            y,
            &Span::styled(&truncated_right, Style::default().fg(meta_fg).bg(row_bg)),
            right_width,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_spine::McpToolStatus;

    fn server(name: &str, ok: bool, tools: &[(&str, &str)]) -> McpStatus {
        McpStatus {
            name: name.into(),
            command: "http://127.0.0.1/mcp".into(),
            ok,
            enabled: true,
            needs_auth: false,
            detail: if ok {
                tools_label(tools.len())
            } else {
                "connection refused".into()
            },
            tools: tools
                .iter()
                .map(|(n, d)| McpToolStatus {
                    name: (*n).into(),
                    public_name: format!("mcp_{name}__{n}"),
                    description: (*d).into(),
                    enabled: true,
                })
                .collect(),
        }
    }

    #[test]
    fn chinese_copy() {
        assert_eq!(tools_label(0), "没有工具（服务器可能未连接）");
        assert_eq!(tools_label(2), "2 个工具");
        assert_eq!(badge_for(true, true, false), "[就绪]");
        assert_eq!(badge_for(true, false, false), "[不可用]");
        assert_eq!(badge_for(true, false, true), "[需认证]");
        assert_eq!(badge_for(false, false, true), "[已禁用]");
        assert_eq!(badge_for(false, true, false), "[已禁用]");
        assert_eq!(TITLE, "MCP 服务器");
        assert_eq!(RIGHT_LOCAL, "(本地)");
        assert_eq!(
            FOOTER,
            "Space 开关 · i 登录 · Ctrl+R 重载 · Enter 展开 · Esc 关闭"
        );
    }

    #[test]
    fn space_toggles_server_or_tool() {
        let mut servers = vec![server("local", true, &[("echo", "ping")])];
        let rows = build_rows(&servers, "", &HashSet::new(), false);
        assert_eq!(toggle_target(&rows[0], &servers), None);
        assert_eq!(
            toggle_target(&rows[1], &servers),
            Some(McpToggle::Server {
                name: "local".into(),
                enabled: false,
            })
        );
        let mut expanded = HashSet::new();
        toggle_row(&rows[1], &mut expanded, &mut false, false);
        let open = build_rows(&servers, "", &expanded, false);
        assert_eq!(
            toggle_target(&open[2], &servers),
            Some(McpToggle::Tool {
                server: "local".into(),
                tool: "echo".into(),
                enabled: false,
            })
        );
        servers[0].enabled = false;
        servers[0].ok = false;
        servers[0].tools[0].enabled = false;
        assert_eq!(
            toggle_target(&rows[1], &servers),
            Some(McpToggle::Server {
                name: "local".into(),
                enabled: true,
            })
        );
        assert_eq!(
            toggle_target(&open[2], &servers),
            Some(McpToggle::Tool {
                server: "local".into(),
                tool: "echo".into(),
                enabled: true,
            })
        );
    }

    #[test]
    fn i_targets_server_or_parent() {
        let servers = vec![server("linear", false, &[("list_issues", "")])];
        let rows = build_rows(&servers, "", &HashSet::new(), false);
        assert_eq!(auth_target(&rows[0], &servers), None);
        assert_eq!(auth_target(&rows[1], &servers).as_deref(), Some("linear"));
        let mut expanded = HashSet::new();
        toggle_row(&rows[1], &mut expanded, &mut false, false);
        let open = build_rows(&servers, "", &expanded, false);
        assert_eq!(auth_target(&open[2], &servers).as_deref(), Some("linear"));
    }

    #[test]
    fn section_and_expand_tools() {
        let servers = vec![server("local", true, &[("echo", "ping"), ("time", "")])];
        let mut expanded = HashSet::new();
        let rows = build_rows(&servers, "", &expanded, false);
        assert_eq!(
            rows,
            vec![
                McpRow::Section {
                    count: 1,
                    collapsed: false
                },
                McpRow::Server { index: 0 },
            ]
        );
        let mut ignore = false;
        toggle_row(&rows[1], &mut expanded, &mut ignore, false);
        let open = build_rows(&servers, "", &expanded, false);
        assert_eq!(
            open,
            vec![
                McpRow::Section {
                    count: 1,
                    collapsed: false
                },
                McpRow::Server { index: 0 },
                McpRow::Tool { server: 0, tool: 0 },
                McpRow::Tool { server: 0, tool: 1 },
            ]
        );
    }

    #[test]
    fn collapse_section_hides_servers() {
        let servers = vec![server("local", true, &[("echo", "")])];
        let expanded = HashSet::new();
        let mut collapsed = false;
        let rows = build_rows(&servers, "", &expanded, collapsed);
        toggle_row(&rows[0], &mut HashSet::new(), &mut collapsed, false);
        assert!(collapsed);
        let hidden = build_rows(&servers, "", &expanded, collapsed);
        assert_eq!(
            hidden,
            vec![McpRow::Section {
                count: 1,
                collapsed: true
            }]
        );
        let searching = build_rows(&servers, "echo", &expanded, true);
        assert!(matches!(
            searching[0],
            McpRow::Section {
                collapsed: false,
                ..
            }
        ));
        assert_eq!(searching.len(), 3);
    }

    #[test]
    fn empty_query_no_rows() {
        assert!(build_rows(&[], "", &HashSet::new(), false).is_empty());
        assert_eq!(empty_hint(false, ""), EMPTY);
        assert_eq!(empty_hint(true, "zzz"), NO_MATCH);
    }
}
