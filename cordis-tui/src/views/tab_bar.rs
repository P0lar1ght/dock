//! 标签栏：分页多于一页时钉在最顶上的一行。
//!
//! 只画状态，不持有状态——页表是 `"tui.tabs"` 的，点击命中回传页序号由
//! 事件循环去 `Tabs::activate`。

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::grok::line_utils::truncate_str;
use crate::seam::tabs::{TabInfo, TabKind};
use crate::theme::Theme;

/// 点到了哪一页 —— 带的是标签上的**稳定编号**，不是位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabHit(pub usize);

pub fn hit(hits: &[(Rect, TabHit)], column: u16, row: u16) -> Option<TabHit> {
    let pos = Position { x: column, y: row };
    hits.iter().find(|(r, _)| r.contains(pos)).map(|(_, h)| *h)
}

/// 一页在标签上的记号：跑着 `●`、闲着 `○`。
fn mark(tab: &TabInfo) -> &'static str {
    if tab.working {
        "●"
    } else {
        "○"
    }
}

/// 每页最多占多宽：页数多了就一起挤，保证最后那页也画得出来。
fn cell_width(total: u16, tabs: usize) -> u16 {
    let tabs = tabs.max(1) as u16;
    (total / tabs).clamp(6, 24)
}

pub fn paint(buf: &mut Buffer, area: Rect, tabs: &[TabInfo]) -> Vec<(Rect, TabHit)> {
    let mut hits = Vec::new();
    if area.height == 0 || area.width == 0 || tabs.len() < 2 {
        return hits;
    }
    let theme = Theme::current();
    let y = area.y;
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.reset();
            cell.set_style(Style::default().bg(theme.bg_base));
        }
    }

    let cell = cell_width(area.width, tabs.len());
    let mut x = area.x;
    for tab in tabs.iter() {
        if x >= area.x + area.width {
            break;
        }
        // 标号是稳定 id，不是位置：关掉中间一页，其它页的号不变。
        let head = format!(" {}{} ", tab.id, mark(tab));
        let head_w = head.width() as u16;
        let budget = cell.saturating_sub(head_w).max(2) as usize;
        // 分叉 / 旁问页的标题来自来源页的快照，不标一下就和来源页长得一模一样。
        // `?` 是只读旁问，`⑂` 是普通分叉。
        let badge = match (tab.kind, tab.origin) {
            (TabKind::Aside, _) => "?",
            (TabKind::Normal, Some(_)) => "⑂",
            (TabKind::Normal, None) => "",
        };
        let title = if badge.is_empty() {
            truncate_str(&tab.title, budget)
        } else {
            format!(
                "{badge}{}",
                truncate_str(&tab.title, budget.saturating_sub(1))
            )
        };
        let text = format!("{head}{title} ");
        let width = (text.width() as u16).min(area.x + area.width - x);
        let style = if tab.active {
            Style::default()
                .fg(theme.accent_user)
                .bg(theme.bg_base)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.gray).bg(theme.bg_base)
        };
        buf.set_line(x, y, &Line::from(Span::styled(text, style)), width);
        hits.push((
            Rect {
                x,
                y,
                width,
                height: 1,
            },
            TabHit(tab.id),
        ));
        x = x.saturating_add(width);
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: usize, title: &str, working: bool, active: bool) -> TabInfo {
        TabInfo {
            id,
            title: title.into(),
            working,
            active,
            origin: None,
            kind: TabKind::Normal,
        }
    }

    fn forked(id: usize, title: &str, origin: usize) -> TabInfo {
        TabInfo {
            origin: Some(origin),
            ..tab(id, title, false, false)
        }
    }

    fn aside(id: usize, title: &str, origin: usize) -> TabInfo {
        TabInfo {
            origin: Some(origin),
            kind: TabKind::Aside,
            ..tab(id, title, false, false)
        }
    }

    fn render(tabs: &[TabInfo], width: u16) -> (String, Vec<(Rect, TabHit)>) {
        let area = Rect {
            x: 0,
            y: 0,
            width,
            height: 1,
        };
        let mut buf = Buffer::empty(area);
        let hits = paint(&mut buf, area, tabs);
        // 宽字符占两格，续格是空的：拼回来时挤掉空白才能按整词断言。
        let text: String = (0..width)
            .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()))
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (text, hits)
    }

    #[test]
    fn single_tab_draws_nothing() {
        let (text, hits) = render(&[tab(1, "主线", false, true)], 40);
        assert!(text.trim().is_empty(), "{text:?}");
        assert!(hits.is_empty());
    }

    #[test]
    fn shows_stable_ids_and_working_marks() {
        let (text, hits) = render(
            &[tab(1, "主线", true, true), tab(3, "读文档", false, false)],
            60,
        );
        assert!(text.contains("1●"), "{text:?}");
        assert!(text.contains("3○"), "{text:?}");
        assert!(text.replace(' ', "").contains("主线"), "{text:?}");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].1, TabHit(3), "命中带的是稳定编号");
    }

    /// 窄终端里也不能把最后一页挤没：每页的宽度是分出来的。
    #[test]
    fn narrow_terminal_still_lists_every_tab() {
        let tabs: Vec<TabInfo> = (1..=5)
            .map(|i| tab(i, "很长很长的标题文字", false, i == 1))
            .collect();
        let (_text, hits) = render(&tabs, 60);
        assert_eq!(hits.len(), 5, "五页都要有命中区");
        assert!(hits.iter().all(|(r, _)| r.width > 0));
    }

    /// 分叉页带着来源页的历史，标题会撞车：得有个记号分得出来。
    #[test]
    fn forked_tabs_are_marked() {
        let (text, _) = render(&[tab(1, "主线", false, true), forked(2, "主线", 1)], 60);
        assert!(text.contains('⑂'), "{text:?}");
    }

    /// 只读旁问页要和普通分叉页分得开。
    #[test]
    fn aside_tabs_get_their_own_badge() {
        let (text, _) = render(&[tab(1, "主线", false, true), aside(2, "主线", 1)], 60);
        assert!(text.contains('?'), "{text:?}");
        assert!(!text.contains('⑂'), "{text:?}");
    }

    #[test]
    fn click_maps_to_the_tab_under_it() {
        let (_text, hits) = render(
            &[tab(1, "主线", false, true), tab(2, "第二页", false, false)],
            60,
        );
        let second = hits[1].0;
        assert_eq!(hit(&hits, second.x, 0), Some(TabHit(2)));
        assert_eq!(hit(&hits, hits[0].0.x, 0), Some(TabHit(1)));
        assert_eq!(hit(&hits, 59, 5), None, "不在这一行就不算");
    }
}
