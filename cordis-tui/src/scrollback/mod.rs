//! Scrollback renders `session/event`. Live-lookup `sessions` each frame.
//!
//! Blocks are Grok pager chrome: user `❯` band, markdown assistant,
//! collapsed `◆` tool card (click to expand). Lines are wrapped to the
//! pane width with Grok `word_wrap_lines`.

use std::collections::HashSet;
use std::sync::Mutex;

use cordis::Context;
use cordis_spine::{LogEvent, Sessions, SESSIONS};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::theme::Theme;

mod assistant;
mod tool;
mod user;

struct Frame {
    lines: Vec<Line<'static>>,
    /// `(line_index, tool_id)` for the collapsed/expanded header row.
    tool_headers: Vec<(usize, String)>,
}

pub struct Scrollback {
    ctx: Context,
    /// 0 = follow the latest line (Grok default). Positive = lines up from bottom.
    offset: Mutex<u16>,
    expanded: Mutex<HashSet<String>>,
    last_frame: Mutex<Frame>,
    last_area: Mutex<Rect>,
}

impl Scrollback {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            offset: Mutex::new(0),
            expanded: Mutex::new(HashSet::new()),
            last_frame: Mutex::new(Frame {
                lines: Vec::new(),
                tool_headers: Vec::new(),
            }),
            last_area: Mutex::new(Rect::default()),
        }
    }

    pub fn scroll(&self, delta: i16) {
        let mut offset = self.offset.lock().unwrap();
        *offset = (*offset as i32 + delta as i32).clamp(0, u16::MAX as i32) as u16;
    }

    pub fn scroll_page(&self, pages: i16) {
        let height = self.last_area.lock().unwrap().height.max(1);
        self.scroll(pages.saturating_mul(height as i16 / 2).max(pages));
    }

    pub fn click(&self, column: u16, row: u16) -> bool {
        let area = *self.last_area.lock().unwrap();
        if !area.contains(ratatui::layout::Position { x: column, y: row }) {
            return false;
        }
        let frame = self.last_frame.lock().unwrap();
        let offset = *self.offset.lock().unwrap();
        let extra = frame.lines.len().saturating_sub(area.height as usize) as u16;
        let scroll = extra.saturating_sub(offset);
        let line_idx = (row.saturating_sub(area.y) as usize).saturating_add(scroll as usize);
        let Some((_, id)) = frame
            .tool_headers
            .iter()
            .find(|(i, _)| *i == line_idx)
            .cloned()
        else {
            return false;
        };
        drop(frame);
        let mut expanded = self.expanded.lock().unwrap();
        if !expanded.remove(&id) {
            expanded.insert(id);
        }
        true
    }

    pub fn find(&self, query: &str) -> Vec<(usize, String)> {
        let q = query.trim();
        if q.is_empty() {
            return Vec::new();
        }
        let needle = q.to_ascii_lowercase();
        let width = self.last_area.lock().unwrap().width.max(40) as usize;
        let frame = self.build(width);
        frame
            .lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| {
                let text: String = line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect();
                text.to_ascii_lowercase()
                    .contains(&needle)
                    .then_some((i, text.trim().to_string()))
            })
            .collect()
    }

    pub fn jump_to_line(&self, line_idx: usize) {
        let area = *self.last_area.lock().unwrap();
        let frame = self.last_frame.lock().unwrap();
        let extra = frame.lines.len().saturating_sub(area.height as usize) as u16;
        let scroll = (line_idx as u16).min(extra);
        *self.offset.lock().unwrap() = extra.saturating_sub(scroll);
    }

    fn build(&self, width: usize) -> Frame {
        let Ok(sessions) = self.ctx.require::<Sessions>(SESSIONS) else {
            return Frame {
                lines: Vec::new(),
                tool_headers: Vec::new(),
            };
        };
        let expanded = self.expanded.lock().unwrap().clone();
        build_frame(&sessions.events(), width, &expanded)
    }
}

#[cfg(test)]
pub(crate) fn lines_from_events(events: &[LogEvent]) -> Vec<Line<'static>> {
    build_frame(events, 80, &HashSet::new()).lines
}

fn build_frame(events: &[LogEvent], width: usize, expanded: &HashSet<String>) -> Frame {
    let theme = Theme::current();
    let mut lines = Vec::new();
    let mut tool_headers = Vec::new();
    for event in events {
        match event {
            LogEvent::User(text) => {
                lines.extend(user::lines(text, &theme, width));
                lines.push(Line::from(""));
            }
            LogEvent::LlmStream(llm) => {
                let md = assistant::lines(&llm.text, &theme, width);
                if !md.is_empty() {
                    lines.extend(md);
                    lines.push(Line::from(""));
                }
            }
            LogEvent::ToolExecute { id, name, content } => {
                let open = expanded.contains(id);
                let header_at = lines.len();
                lines.extend(tool::lines(name, content, &theme, width, open));
                tool_headers.push((header_at, id.clone()));
                lines.push(Line::from(""));
            }
            LogEvent::PreStep | LogEvent::Prompt(_) => {}
        }
    }
    Frame {
        lines,
        tool_headers,
    }
}

impl Widget for &Scrollback {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let theme = Theme::current();
        buf.set_style(area, Style::default().bg(theme.bg_base));
        let frame = self.build(area.width as usize);
        let offset = *self.offset.lock().unwrap();
        let extra = frame.lines.len().saturating_sub(area.height as usize) as u16;
        let scroll = extra.saturating_sub(offset);
        Paragraph::new(frame.lines.clone())
            .scroll((scroll, 0))
            .render(area, buf);
        *self.last_frame.lock().unwrap() = frame;
        *self.last_area.lock().unwrap() = area;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_spine::{LlmOutput, ToolCall};

    fn plain(lines: &[Line<'_>]) -> String {
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
    fn user_gets_grok_prompt_arrow() {
        let lines = lines_from_events(&[LogEvent::User("hello".into())]);
        let text = plain(&lines);
        assert!(text.contains('\u{276F}'), "{text}");
        assert!(text.contains("hello"), "{text}");
    }

    #[test]
    fn user_bang_uses_bash_prefix() {
        let lines = lines_from_events(&[LogEvent::User("!ls".into())]);
        let text = plain(&lines);
        assert!(text.contains("$ "), "{text}");
        assert!(text.contains("ls"), "{text}");
    }

    #[test]
    fn user_cron_uses_recycle_prefix() {
        let lines = lines_from_events(&[LogEvent::User("\u{21BB} check".into())]);
        let text = plain(&lines);
        assert!(text.contains('\u{21BB}'), "{text}");
        assert!(text.contains("check"), "{text}");
    }

    #[test]
    fn assistant_renders_markdown_heading() {
        let lines = lines_from_events(&[LogEvent::LlmStream(LlmOutput {
            text: "# Title\n\nbody".into(),
            tool_calls: Vec::new(),
        })]);
        let text = plain(&lines);
        assert!(text.contains("Title"), "{text}");
        assert!(text.contains("body"), "{text}");
    }

    #[test]
    fn tool_collapsed_by_default_hides_body() {
        let lines = lines_from_events(&[LogEvent::ToolExecute {
            id: "1".into(),
            name: "list_dir".into(),
            content: "MARKER.txt\nother".into(),
        }]);
        let text = plain(&lines);
        assert!(text.contains('\u{25C6}'), "{text}");
        assert!(text.contains("list_dir"), "{text}");
        assert!(
            !text.contains("MARKER.txt"),
            "collapsed card must hide body: {text}"
        );
    }

    #[test]
    fn tool_expanded_shows_wrapped_body() {
        let theme = Theme::current();
        let lines = tool::lines("list_dir", "MARKER.txt", &theme, 80, true);
        let text = plain(&lines);
        assert!(text.contains("MARKER.txt"), "{text}");
    }

    #[test]
    fn empty_llm_text_with_tool_calls_is_skipped() {
        let lines = lines_from_events(&[LogEvent::LlmStream(LlmOutput {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "echo".into(),
                arguments: "{}".into(),
            }],
        })]);
        assert!(plain(&lines).trim().is_empty());
    }
}
