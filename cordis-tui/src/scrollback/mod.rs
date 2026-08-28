//! Scrollback renders `session/event`. Live-lookup `sessions` each frame.
//!
//! Blocks are Grok pager chrome: user `❯` band, markdown assistant,
//! collapsed `◆` tool card (click to expand). Lines are wrapped to the
//! pane width with Grok `word_wrap_lines`.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::{DateTime, Local};
use cordis::Context;
use cordis_spine::{
    AppSettings, LogEvent, Sessions, TodoStatus, Todos, SESSIONS, SETTINGS, TODOS,
};

use crate::names::SESSION_PORT;
use crate::session::SessionRef;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::grok::mermaid::{self, AffordanceKind};
use crate::theme::Theme;

mod assistant;
mod thinking;
mod tool;
mod user;

struct Frame {
    lines: Vec<Line<'static>>,
    /// `(line_index, tool_id)` for the collapsed/expanded header row.
    tool_headers: Vec<(usize, String)>,
    /// `(line_index, mermaid source)` for `[Copy Source]` clicks.
    mermaid: Vec<(usize, String)>,
    /// First content line of user/assistant blocks, for `/timestamps`.
    stamps: Vec<(usize, SystemTime)>,
}

pub enum ClickHit {
    None,
    ToolToggle,
    Mermaid {
        kind: AffordanceKind,
        source: String,
    },
}

pub struct Scrollback {
    ctx: Context,
    /// 0 = follow the latest line (Grok default). Positive = lines up from bottom.
    offset: Mutex<u16>,
    expanded: Mutex<HashSet<String>>,
    last_frame: Mutex<Frame>,
    last_area: Mutex<Rect>,
    mouse: Mutex<Option<(u16, u16)>>,
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
                mermaid: Vec::new(),
                stamps: Vec::new(),
            }),
            last_area: Mutex::new(Rect::default()),
            mouse: Mutex::new(None),
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

    pub fn set_mouse(&self, column: u16, row: u16) {
        *self.mouse.lock().unwrap() = Some((column, row));
    }

    pub fn click(&self, column: u16, row: u16) -> ClickHit {
        let area = *self.last_area.lock().unwrap();
        if !area.contains(ratatui::layout::Position { x: column, y: row }) {
            return ClickHit::None;
        }
        let frame = self.last_frame.lock().unwrap();
        let offset = *self.offset.lock().unwrap();
        let extra = frame.lines.len().saturating_sub(area.height as usize) as u16;
        let scroll = extra.saturating_sub(offset);
        let line_idx = (row.saturating_sub(area.y) as usize).saturating_add(scroll as usize);
        if let Some((_, src)) = frame.mermaid.iter().find(|(i, _)| *i == line_idx) {
            return match mermaid::hit_kind(column.saturating_sub(area.x)) {
                Some(kind) => ClickHit::Mermaid {
                    kind,
                    source: src.clone(),
                },
                None => ClickHit::None,
            };
        }
        let Some((_, id)) = frame
            .tool_headers
            .iter()
            .find(|(i, _)| *i == line_idx)
            .cloned()
        else {
            return ClickHit::None;
        };
        drop(frame);
        let mut expanded = self.expanded.lock().unwrap();
        if !expanded.remove(&id) {
            expanded.insert(id);
        }
        ClickHit::ToolToggle
    }

    pub fn last_assistant(&self, n: usize) -> Option<String> {
        let n = n.max(1);
        let Ok(sessions) = self.ctx.require::<Sessions>(SESSIONS) else {
            return None;
        };
        sessions
            .events()
            .iter()
            .rev()
            .filter_map(|e| match e {
                LogEvent::LlmStream(llm) if !llm.text.trim().is_empty() => Some(llm.text.clone()),
                _ => None,
            })
            .nth(n - 1)
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
                mermaid: Vec::new(),
                stamps: Vec::new(),
            };
        };
        let expanded = self.expanded.lock().unwrap().clone();
        let show_ts = self
            .ctx
            .get::<AppSettings>(SETTINGS)
            .is_some_and(|s| s.timestamps());
        let working = self
            .ctx
            .get::<SessionRef>(SESSION_PORT)
            .is_some_and(|s| s.working());
        let todos = self
            .ctx
            .get::<Todos>(TODOS)
            .map(|t| t.snapshot())
            .unwrap_or_default();
        build_frame(
            &sessions.events(),
            &sessions.times(),
            width,
            &expanded,
            working,
            show_ts,
            &todos,
        )
    }
}

#[cfg(test)]
pub(crate) fn lines_from_events(events: &[LogEvent]) -> Vec<Line<'static>> {
    build_frame(events, &[], 80, &HashSet::new(), false, false, &[]).lines
}

/// Grok `timestamp_reserved`: `"  12:30 PM"` is 10 columns.
const TIMESTAMP_RESERVED: usize = 10;

fn message_wrap(width: usize, show_ts: bool) -> usize {
    if show_ts {
        width.saturating_sub(TIMESTAMP_RESERVED).max(20)
    } else {
        width
    }
}

fn build_frame(
    events: &[LogEvent],
    times: &[SystemTime],
    width: usize,
    expanded: &HashSet<String>,
    working: bool,
    show_ts: bool,
    todos: &[(String, cordis_spine::TodoItem)],
) -> Frame {
    let theme = Theme::current();
    let mut lines = Vec::new();
    let mut tool_headers = Vec::new();
    let mut mermaid = Vec::new();
    let mut stamps = Vec::new();
    for (i, event) in events.iter().enumerate() {
        let stamp = times.get(i).copied();
        match event {
            LogEvent::User(text) => {
                if let Some(at) = stamp {
                    stamps.push((lines.len(), at));
                }
                lines.extend(user::lines(text, &theme, message_wrap(width, show_ts)));
                lines.push(Line::from(""));
            }
            LogEvent::LlmStream(llm) => {
                let think_id = format!("think:{i}");
                let think_open = expanded.contains(&think_id);
                let streaming_think = working
                    && i + 1 == events.len()
                    && llm.tool_calls.is_empty();
                let think = thinking::lines(
                    &llm.reasoning,
                    llm.reasoning_ms,
                    streaming_think,
                    &theme,
                    width,
                    think_open,
                );
                if !think.is_empty() {
                    tool_headers.push((lines.len(), think_id));
                    lines.extend(think);
                    lines.push(Line::from(""));
                }
                let rendered = assistant::render(&llm.text, &theme, message_wrap(width, show_ts));
                if !rendered.lines.is_empty() {
                    if let Some(at) = stamp {
                        stamps.push((lines.len(), at));
                    }
                    let base = lines.len();
                    mermaid.extend(
                        rendered
                            .mermaid
                            .into_iter()
                            .map(|(i, src)| (base + i, src)),
                    );
                    lines.extend(rendered.lines);
                    lines.push(Line::from(""));
                }
                for call in &llm.tool_calls {
                    let done = events[i + 1..].iter().any(|e| match e {
                        LogEvent::ToolExecute { id, .. } => id == &call.id,
                        _ => false,
                    });
                    if done {
                        continue;
                    }
                    let open = expanded.contains(&call.id);
                    let header_at = lines.len();
                    lines.extend(tool::lines(
                        &call.name,
                        &call.arguments,
                        "",
                        &theme,
                        width,
                        open,
                        true,
                    ));
                    tool_headers.push((header_at, call.id.clone()));
                    lines.push(Line::from(""));
                }
            }
            LogEvent::ToolExecute {
                id,
                name,
                arguments,
                content,
            } => {
                let open = expanded.contains(id);
                let header_at = lines.len();
                lines.extend(tool::lines(
                    name, arguments, content, &theme, width, open, false,
                ));
                tool_headers.push((header_at, id.clone()));
                lines.push(Line::from(""));
            }
            LogEvent::PreStep | LogEvent::Prompt(_) => {}
        }
    }
    if !todos.is_empty() {
        lines.extend(todo_lines(todos, &theme, width));
        lines.push(Line::from(""));
    }
    Frame {
        lines,
        tool_headers,
        mermaid,
        stamps,
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
        let show_ts = self
            .ctx
            .get::<AppSettings>(SETTINGS)
            .is_some_and(|s| s.timestamps());
        if show_ts {
            let mouse = *self.mouse.lock().unwrap();
            paint_timestamps(buf, area, &frame, scroll, mouse, &theme);
        }
        *self.last_frame.lock().unwrap() = frame;
        *self.last_area.lock().unwrap() = area;
    }
}

fn todo_lines(
    todos: &[(String, cordis_spine::TodoItem)],
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        "待办",
        Style::default().fg(theme.accent_user),
    ))];
    for (id, item) in todos {
        let mark = match item.status {
            TodoStatus::Completed => "✓",
            TodoStatus::InProgress => "▸",
            TodoStatus::Cancelled => "×",
            TodoStatus::Pending => "·",
        };
        let row = format!("{mark} {id} {}", item.content);
        let clipped: String = row.chars().take(width.max(8)).collect();
        lines.push(Line::from(Span::styled(
            clipped,
            Style::default().fg(theme.text_secondary),
        )));
    }
    lines
}

fn format_timestamp(at: SystemTime, expanded: bool) -> String {
    let dt: DateTime<Local> = at.into();
    if expanded {
        dt.format("  %H:%M:%S | %b %d").to_string()
    } else if cfg!(unix) {
        dt.format("  %-I:%M %p").to_string()
    } else {
        dt.format("  %I:%M %p").to_string()
    }
}

fn paint_timestamps(
    buf: &mut Buffer,
    area: Rect,
    frame: &Frame,
    scroll: u16,
    mouse: Option<(u16, u16)>,
    theme: &Theme,
) {
    let style = theme.muted();
    for (line_idx, at) in &frame.stamps {
        if *line_idx < scroll as usize {
            continue;
        }
        let y = area.y + (*line_idx as u16).saturating_sub(scroll);
        if y >= area.y + area.height {
            continue;
        }
        let hover = mouse.is_some_and(|(mx, my)| {
            my == y && mx >= area.x + area.width.saturating_sub(10)
        });
        let ts = format_timestamp(*at, hover);
        let w = unicode_width::UnicodeWidthStr::width(ts.as_str()) as u16;
        if area.width <= w + 1 {
            continue;
        }
        let x = area.x + area.width - w;
        buf.set_string(x, y, &ts, style);
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
    fn compact_timestamp_has_clock() {
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let compact = format_timestamp(at, false);
        assert!(compact.contains(':'), "{compact}");
        let hover = format_timestamp(at, true);
        assert!(hover.contains('|'), "{hover}");
    }

    #[test]
    fn user_block_paints_clock_on_the_right() {
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let frame = build_frame(
            &[LogEvent::User("你好".into())],
            &[at],
            80,
            &HashSet::new(),
            false,
            true,
            &[],
        );
        assert_eq!(frame.stamps, vec![(0, at)]);
        let area = Rect::new(0, 0, 80, 3);
        let mut buf = Buffer::empty(area);
        Paragraph::new(frame.lines.clone()).render(area, &mut buf);
        paint_timestamps(&mut buf, area, &frame, 0, None, &Theme::current());
        let left: String = (0..16).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        let right: String = (60..80).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(left.contains('你') && left.contains('好'), "{left:?}");
        assert!(
            right.contains("AM") || right.contains("PM"),
            "expected Grok-style clock on the user line: {right:?}"
        );
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
            ..LlmOutput::default()
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
            arguments: r#"{"target_directory":"."}"#.into(),
            content: "MARKER.txt\nother".into(),
        }]);
        let text = plain(&lines);
        assert!(text.contains('\u{25C6}'), "{text}");
        assert!(text.contains("list_dir"), "{text}");
        assert!(text.contains('.'), "{text}");
        assert!(
            !text.contains("MARKER.txt"),
            "collapsed card must hide body: {text}"
        );
    }

    #[test]
    fn tool_expanded_shows_wrapped_body() {
        let theme = Theme::current();
        let lines = tool::lines(
            "list_dir",
            r#"{"target_directory":"."}"#,
            "MARKER.txt",
            &theme,
            80,
            true,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("MARKER.txt"), "{text}");
        assert!(text.contains("输入"), "{text}");
        assert!(text.contains("输出"), "{text}");
        assert!(text.contains("target_directory"), "{text}");
    }

    #[test]
    fn thinking_collapsed_hides_reasoning() {
        let lines = lines_from_events(&[LogEvent::LlmStream(LlmOutput {
            text: "hello".into(),
            reasoning: "secret plan".into(),
            reasoning_ms: Some(1500),
            ..LlmOutput::default()
        })]);
        let text = plain(&lines);
        assert!(text.contains("思考"), "{text}");
        assert!(!text.contains("secret plan"), "{text}");
        assert!(text.contains("hello"), "{text}");
    }

    #[test]
    fn mermaid_copy_source_is_hit_mapped() {
        let lines = lines_from_events(&[LogEvent::LlmStream(LlmOutput {
            text: "```mermaid\nflowchart TD\nA-->B\n```\n".into(),
            ..LlmOutput::default()
        })]);
        let text = plain(&lines);
        assert!(text.contains("[Copy Source]"), "{text}");
        assert!(text.contains("[Open Image]"), "{text}");
    }

    #[test]
    fn empty_llm_text_with_tool_calls_shows_pending_card() {
        let lines = lines_from_events(&[LogEvent::LlmStream(LlmOutput {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "echo".into(),
                arguments: "{}".into(),
            }],
            ..LlmOutput::default()
        })]);
        let text = plain(&lines);
        assert!(text.contains("echo"), "{text}");
        assert!(text.contains("运行中"), "{text}");
    }
}
