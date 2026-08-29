//! Slim goal overlay copied from grok-build/.../xai-grok-pager/src/views/goal_detail.rs
//! (title on the frame, status, commands). Classifier / token budget / subagent
//! metrics are omitted. Action rows let the user edit, pause, or clear.

use cordis_spine::Goal;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::line_utils::truncate_str;
use crate::grok::picker::{render_floating_frame, PickerHits};
use crate::theme::Theme;

pub const ACTION_COUNT: usize = 3;
pub const ACTION_EDIT: usize = 0;
pub const ACTION_PAUSE: usize = 1;
pub const ACTION_CLEAR: usize = 2;

pub fn action_label(index: usize, paused: bool) -> &'static str {
    match index {
        ACTION_EDIT => "改标题",
        ACTION_PAUSE if paused => "继续",
        ACTION_PAUSE => "暂停",
        ACTION_CLEAR => "清除",
        _ => "",
    }
}

pub fn render(
    buf: &mut Buffer,
    area: Rect,
    goal: Option<&Goal>,
    selected: usize,
    editing: bool,
    draft: &str,
) -> PickerHits {
    let theme = Theme::current();
    let Some(frame) = render_floating_frame(buf, area, &theme, false) else {
        return PickerHits::default();
    };

    let mut y = frame.content.y;
    let x = frame.content.x;
    let w = frame.content.width;
    let mut hits = PickerHits {
        close_button: frame.close_button,
        rows: Vec::new(),
    };

    let present = goal.is_some_and(|g| g.present());
    let title = goal.map(|g| g.title()).unwrap_or_default();
    let paused = goal.is_some_and(|g| g.paused());
    let active = goal.is_some_and(|g| g.active());

    let title_style = Style::default()
        .fg(theme.accent_plan)
        .add_modifier(Modifier::BOLD);
    let shown = if editing {
        format!(" {draft} ")
    } else if title.trim().is_empty() {
        " 目标 ".to_string()
    } else {
        format!(" {} ", truncate_str(&title, w.saturating_sub(2) as usize))
    };
    buf.set_line(x, y, &Line::from(Span::styled(shown, title_style)), w);
    y = y.saturating_add(1);

    if y >= frame.content.y + frame.content.height {
        return hits;
    }

    let (status_text, status_color) = if !present {
        ("没有活动目标", theme.gray)
    } else if paused {
        ("已暂停", theme.warning)
    } else if active {
        ("进行中", theme.accent_success)
    } else {
        ("没有活动目标", theme.gray)
    };
    buf.set_line(
        x,
        y,
        &Line::from(vec![
            Span::styled("状态: ", Style::default().fg(theme.gray)),
            Span::styled(
                status_text,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        w,
    );
    y = y.saturating_add(1);
    let note = goal.map(|g| g.status()).unwrap_or_default();
    if !note.trim().is_empty() && y < frame.content.y + frame.content.height {
        buf.set_line(
            x,
            y,
            &Line::from(vec![
                Span::styled("进度: ", Style::default().fg(theme.gray)),
                Span::styled(
                    truncate_str(note.trim(), w.saturating_sub(4) as usize),
                    Style::default().fg(theme.text_secondary),
                ),
            ]),
            w,
        );
        y = y.saturating_add(1);
    }
    y = y.saturating_add(1);

    if present && y < frame.content.y + frame.content.height {
        let sel = selected.min(ACTION_COUNT.saturating_sub(1));
        for i in 0..ACTION_COUNT {
            if y >= frame.content.y + frame.content.height {
                break;
            }
            let chosen = !editing && i == sel;
            let bg = if chosen {
                theme.bg_visual
            } else {
                theme.bg_light
            };
            let rect = Rect {
                x,
                y,
                width: w,
                height: 1,
            };
            buf.set_style(rect, Style::default().bg(bg));
            let label = action_label(i, paused);
            let style = Style::default()
                .fg(theme.text_primary)
                .bg(bg)
                .add_modifier(if chosen {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });
            buf.set_line(
                x.saturating_add(1),
                y,
                &Line::from(Span::styled(label.to_string(), style)),
                w.saturating_sub(2),
            );
            hits.rows.push((i, rect));
            y = y.saturating_add(1);
        }
    }

    if y < frame.content.y + frame.content.height {
        let hint = if editing {
            "Enter:保存  Esc:取消"
        } else if present {
            "Esc:关闭  e:改标题  p:暂停/继续  c:清除  g:关闭"
        } else {
            "Esc:关闭"
        };
        buf.set_line(
            x,
            frame.content.y + frame.content.height.saturating_sub(1),
            &Line::from(Span::styled(hint, Style::default().fg(theme.gray_dim))),
            w,
        );
    }

    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_row_flips_label() {
        assert_eq!(action_label(ACTION_PAUSE, false), "暂停");
        assert_eq!(action_label(ACTION_PAUSE, true), "继续");
        assert_eq!(action_label(ACTION_EDIT, false), "改标题");
        assert_eq!(action_label(ACTION_CLEAR, false), "清除");
    }
}
