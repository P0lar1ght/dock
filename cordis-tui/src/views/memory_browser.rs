//! `/memory` dual-pane browser: left file list (global/workspace), right
//! markdown preview via `cordis_markdown` (same path as scrollback/plan).
//!
//! Narrow terminals (< 64 cols) collapse to list-only; Enter opens preview.
//! `/` filters by filename **or file content**; Esc closes (or exits filter / preview first).
//! `x` deletes with dual-confirm (forget → archive + index). Optional `t` toggles session
//! memory (DOCK_MEMORY=0 still forces off).

use std::path::PathBuf;

use cordis::Context;
use crossterm::event::KeyCode;
use dock_memory::browse::{list_memory_files, MemoryFileEntry};
use dock_memory::layout::{MemoryRoot, MemoryScope};
use dock_memory::MAX_FORGET_FILE_BYTES;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::line_utils::truncate_str;
use crate::grok::md_style;
use crate::grok::picker::{render_divider, render_floating_frame_height, PickerHits};
use crate::grok::wrapping::word_wrap_lines;
use crate::theme::Theme;
use cordis_spine::{Memory, MEMORY};

const SPLIT_MIN_WIDTH: u16 = 64;
const LIST_RATIO: f64 = 0.40;
const PAD: u16 = 1;
const TITLE_ROWS: u16 = 2;
const MAX_PREVIEW_BYTES: u64 = 1_048_576;
const MAX_PICKER_INNER: u16 = 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryFocus {
    List,
    Filter,
    Preview,
}

#[derive(Debug, Clone)]
pub struct MemoryBrowserState {
    pub selected: usize,
    pub filter: String,
    pub focus: MemoryFocus,
    pub list_scroll: usize,
    pub preview_scroll: usize,
    /// First `x` arms delete; second `x` confirms. Esc / other nav clears.
    pub pending_delete: Option<std::path::PathBuf>,
    /// BLAKE3 hex of the previewed bytes (sent with forget).
    pub preview_hash: Option<String>,
}

impl Default for MemoryBrowserState {
    fn default() -> Self {
        Self {
            selected: 0,
            filter: String::new(),
            focus: MemoryFocus::List,
            list_scroll: 0,
            preview_scroll: 0,
            pending_delete: None,
            preview_hash: None,
        }
    }
}

#[derive(Debug, Clone)]
enum Row {
    Header {
        label: String,
    },
    File {
        entry: MemoryFileEntry,
        label: String,
    },
}

fn entry_matches_filter(entry: &MemoryFileEntry, filter_l: &str) -> bool {
    if filter_l.is_empty() {
        return true;
    }
    if entry.label.to_lowercase().contains(filter_l) {
        return true;
    }
    // Content filter (Grok-aligned): match if every whitespace term appears in the file body.
    let Ok(text) = std::fs::read_to_string(&entry.path) else {
        return false;
    };
    let lower = text.to_lowercase();
    filter_l.split_whitespace().all(|term| lower.contains(term))
}

fn memory_enabled_for_list(ctx: Option<&Context>) -> bool {
    if let Some(ctx) = ctx {
        if let Some(mem) = ctx.get::<Memory>(MEMORY) {
            return mem.enabled();
        }
    }
    cordis_base::config::load_memory_config().enabled
}

/// List memory files without creating layout. `ensure_layout` belongs to write
/// paths (`/remember`, `/flush`); opening `/memory` must not mkdir when disabled.
fn build_rows_with_ctx(ctx: Option<&Context>, filter: &str) -> Vec<Row> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = MemoryRoot::open_default(&cwd);
    let enabled = memory_enabled_for_list(ctx);
    let files = list_memory_files(&root);
    // When disabled, only surface rows if something already exists on disk.
    if !enabled && files.is_empty() {
        return Vec::new();
    }
    let filter_l = filter.trim().to_lowercase();
    let mut rows = Vec::new();
    for (scope, title) in [
        (MemoryScope::Global, "Global"),
        (MemoryScope::Workspace, "Workspace"),
    ] {
        let group: Vec<_> = files
            .iter()
            .filter(|e| e.scope == scope)
            .filter(|e| entry_matches_filter(e, &filter_l))
            .cloned()
            .collect();
        if group.is_empty() && !filter_l.is_empty() {
            continue;
        }
        rows.push(Row::Header {
            label: title.into(),
        });
        if group.is_empty() {
            continue;
        }
        for entry in group {
            let label = entry.label.clone();
            rows.push(Row::File { entry, label });
        }
    }
    rows
}

fn selectable_indices(rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| matches!(r, Row::File { .. }).then_some(i))
        .collect()
}

fn selected_entry(rows: &[Row], selected: usize) -> Option<&MemoryFileEntry> {
    let idxs = selectable_indices(rows);
    let row_i = *idxs.get(selected)?;
    match rows.get(row_i) {
        Some(Row::File { entry, .. }) => Some(entry),
        _ => None,
    }
}

fn load_preview(path: &PathBuf) -> String {
    match std::fs::File::open(path) {
        Ok(mut f) => {
            use std::io::Read;
            let mut buf = Vec::new();
            let _ = std::io::Read::by_ref(&mut f)
                .take(MAX_PREVIEW_BYTES)
                .read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        }
        Err(e) => format!("(unreadable: {e})"),
    }
}

fn render_md(text: &str, width: usize) -> Vec<Line<'static>> {
    let syntect = cordis_markdown::default_syntect();
    let mut renderer = cordis_markdown::StreamingMarkdownRenderer::new(md_style::style(), true);
    renderer.set_max_table_width(Some(width.max(8)));
    renderer.push(text);
    let output = renderer.finish_into_output(Some(syntect));
    word_wrap_lines(output.lines, width.max(8))
}

fn empty_markdown(enabled: bool) -> String {
    if !enabled {
        return "**Memory is disabled.**\n\nEnable with `[memory] enabled = true` in `~/.dock/config.toml` or `DOCK_MEMORY=1`.\n\nPress **t** to try a session override (blocked if `DOCK_MEMORY=0`).".into();
    }
    "**Nothing remembered yet.**\n\n- `/remember <note>` saves something specific right now.\n- `/flush` summarizes the session into workspace observations.\n- `/dream` consolidates observations into topics.\n\nNotes live under `$DOCK_HOME/memory/global|workspace-<slug>/{topics,observations/_inbox}/` with a generated `MEMORY.md` index.".into()
}

/// Open browser state (fresh selection).
pub fn open_state() -> MemoryBrowserState {
    MemoryBrowserState::default()
}

pub fn render(
    buf: &mut Buffer,
    area: Rect,
    state: &MemoryBrowserState,
    ctx: &Context,
) -> PickerHits {
    let theme = Theme::current();
    let enabled = ctx
        .get::<Memory>(MEMORY)
        .map(|m| m.enabled())
        .unwrap_or_else(|| cordis_base::config::load_memory_config().enabled);
    let rows = build_rows_with_ctx(Some(ctx), &state.filter);
    let selectable = selectable_indices(&rows);
    let sel = if selectable.is_empty() {
        0
    } else {
        state.selected.min(selectable.len() - 1)
    };

    let inner_rows = MAX_PICKER_INNER;
    let Some(frame) = render_floating_frame_height(buf, area, &theme, false, inner_rows) else {
        return PickerHits::default();
    };
    let inner = frame.content;
    if inner.height < 4 || inner.width < 20 {
        return PickerHits {
            close_button: frame.close_button,
            ..Default::default()
        };
    }

    let title = if state.focus == MemoryFocus::Filter {
        format!("/memory  filter: {}", state.filter)
    } else if enabled {
        "/memory".into()
    } else {
        "/memory (off)".into()
    };
    paint_title(buf, inner, &theme, &title, frame.close_button);
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

    let body = Rect {
        x: inner.x + PAD,
        y: inner.y.saturating_add(TITLE_ROWS),
        width: inner.width.saturating_sub(PAD.saturating_mul(2)),
        height: inner.height.saturating_sub(TITLE_ROWS + 1),
    };
    if body.height == 0 {
        return PickerHits {
            close_button: frame.close_button,
            ..Default::default()
        };
    }

    let split = body.width >= SPLIT_MIN_WIDTH && state.focus != MemoryFocus::Preview;
    let (list_area, preview_area) = if split {
        let list_w = ((body.width as f64) * LIST_RATIO) as u16;
        let list_w = list_w.max(18).min(body.width.saturating_sub(24));
        (
            Rect {
                x: body.x,
                y: body.y,
                width: list_w,
                height: body.height,
            },
            Rect {
                x: body.x.saturating_add(list_w.saturating_add(1)),
                y: body.y,
                width: body.width.saturating_sub(list_w.saturating_add(1)),
                height: body.height,
            },
        )
    } else if state.focus == MemoryFocus::Preview {
        (Rect::default(), body)
    } else {
        (body, Rect::default())
    };

    if list_area.width > 0 {
        paint_list(
            buf,
            list_area,
            &rows,
            &selectable,
            sel,
            state.list_scroll,
            &theme,
        );
    }

    if preview_area.width > 0 {
        let md = if let Some(entry) = selected_entry(&rows, sel) {
            load_preview(&entry.path)
        } else {
            empty_markdown(enabled)
        };
        let lines = render_md(&md, preview_area.width.saturating_sub(1) as usize);
        let max_scroll = lines.len().saturating_sub(preview_area.height as usize);
        let scroll = state.preview_scroll.min(max_scroll);
        for (i, line) in lines
            .into_iter()
            .skip(scroll)
            .take(preview_area.height as usize)
            .enumerate()
        {
            let y = preview_area.y.saturating_add(i as u16);
            buf.set_line(
                preview_area.x,
                y,
                &line_on_bg(&line, theme.bg_base),
                preview_area.width,
            );
        }
    }

    // Footer hint
    let hint_y = inner.y.saturating_add(inner.height.saturating_sub(1));
    let hint = match state.focus {
        MemoryFocus::Filter => "type to filter name/content  Esc:exit filter",
        MemoryFocus::Preview => "Esc:back  ↑↓:scroll",
        MemoryFocus::List if state.pending_delete.is_some() => "x:confirm delete  Esc:cancel",
        MemoryFocus::List if split => "↑↓:select  /:filter  x:delete  t:toggle  Esc:close",
        MemoryFocus::List => "↑↓:select  Enter:preview  /:filter  x:delete  t:toggle  Esc:close",
    };
    let hint_line = Line::from(Span::styled(
        truncate_str(hint, inner.width.saturating_sub(2) as usize),
        Style::default().fg(theme.gray).bg(theme.bg_base),
    ));
    buf.set_line(
        inner.x + 1,
        hint_y,
        &hint_line,
        inner.width.saturating_sub(2),
    );

    PickerHits {
        close_button: frame.close_button,
        ..Default::default()
    }
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

fn paint_list(
    buf: &mut Buffer,
    area: Rect,
    rows: &[Row],
    selectable: &[usize],
    sel: usize,
    list_scroll: usize,
    theme: &Theme,
) {
    let selected_row = selectable.get(sel).copied();
    let visible = area.height as usize;
    let scroll = list_scroll.min(rows.len().saturating_sub(visible));
    for (i, row) in rows.iter().skip(scroll).take(visible).enumerate() {
        let y = area.y.saturating_add(i as u16);
        let abs_i = scroll + i;
        let (text, style) = match row {
            Row::Header { label } => (
                format!(" {label}"),
                Style::default()
                    .fg(theme.accent_remember)
                    .bg(theme.bg_base)
                    .add_modifier(Modifier::BOLD),
            ),
            Row::File { label, .. } => {
                let selected = Some(abs_i) == selected_row;
                let mark = if selected { "› " } else { "  " };
                (
                    format!(
                        "{mark}{}",
                        truncate_str(label, area.width.saturating_sub(3) as usize)
                    ),
                    if selected {
                        Style::default()
                            .fg(theme.text_primary)
                            .bg(theme.bg_highlight)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.text_secondary).bg(theme.bg_base)
                    },
                )
            }
        };
        buf.set_line(
            area.x,
            y,
            &Line::from(Span::styled(text, style)),
            area.width,
        );
    }
}

fn line_on_bg(line: &Line<'static>, bg: ratatui::style::Color) -> Line<'static> {
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

/// Key handling. Returns `true` when the overlay should close.
pub fn on_key(ctx: &Context, state: &mut MemoryBrowserState, code: KeyCode) -> KeyResult {
    let rows = build_rows_with_ctx(Some(ctx), &state.filter);
    let selectable = selectable_indices(&rows);
    let n = selectable.len();

    match state.focus {
        MemoryFocus::Filter => match code {
            KeyCode::Esc => {
                state.filter.clear();
                state.selected = 0;
                state.focus = MemoryFocus::List;
                KeyResult::Handled
            }
            KeyCode::Enter => {
                state.filter.clear();
                state.selected = 0;
                state.focus = MemoryFocus::List;
                KeyResult::Handled
            }
            KeyCode::Backspace => {
                state.filter.pop();
                state.selected = 0;
                KeyResult::Handled
            }
            KeyCode::Char(c) => {
                state.filter.push(c);
                state.selected = 0;
                KeyResult::Handled
            }
            _ => KeyResult::Handled,
        },
        MemoryFocus::Preview => match code {
            KeyCode::Esc => {
                state.focus = MemoryFocus::List;
                state.preview_scroll = 0;
                KeyResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.preview_scroll = state.preview_scroll.saturating_sub(1);
                KeyResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.preview_scroll = state.preview_scroll.saturating_add(1);
                KeyResult::Handled
            }
            _ => KeyResult::Handled,
        },
        MemoryFocus::List => match code {
            KeyCode::Esc => {
                if state.pending_delete.take().is_some() {
                    KeyResult::Flash("delete cancelled".into())
                } else {
                    KeyResult::Close
                }
            }
            KeyCode::Char('/') => {
                state.pending_delete = None;
                state.focus = MemoryFocus::Filter;
                KeyResult::Handled
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if let Some(mem) = ctx.get::<Memory>(MEMORY) {
                    match mem.toggle_session() {
                        Ok(on) => KeyResult::Flash(if on {
                            "Memory on for this session".into()
                        } else {
                            "Memory off for this session".into()
                        }),
                        Err(msg) => KeyResult::Flash(msg.into()),
                    }
                } else {
                    KeyResult::Flash("memory service not mounted".into())
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.pending_delete = None;
                if n > 0 {
                    state.selected = state.selected.saturating_sub(1);
                    state.preview_scroll = 0;
                }
                KeyResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.pending_delete = None;
                if n > 0 {
                    state.selected = (state.selected + 1).min(n - 1);
                    state.preview_scroll = 0;
                }
                KeyResult::Handled
            }
            KeyCode::Enter => {
                // Narrow (or any) fallback: Enter focuses full-area preview.
                // Wide split already shows preview beside the list.
                state.pending_delete = None;
                if n > 0 {
                    state.focus = MemoryFocus::Preview;
                    let _ =
                        refresh_preview_hash(state, &rows, state.selected.min(n.saturating_sub(1)));
                }
                KeyResult::Handled
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                if n == 0 {
                    return KeyResult::Flash("nothing to delete".into());
                }
                let sel = state.selected.min(n - 1);
                let Some(entry) = selected_entry(&rows, sel) else {
                    return KeyResult::Handled;
                };
                // Indexes / MEMORY.md are not deletable via forget path gate.
                if entry.kind == "index" {
                    return KeyResult::Flash("MEMORY.md is generated — cannot delete".into());
                }
                match refresh_preview_hash(state, &rows, sel) {
                    Ok(()) => {}
                    Err(PreviewHashError::TooLarge { size, limit }) => {
                        return KeyResult::Flash(format!(
                            "File too large to forget ({size} bytes; limit {limit} bytes)"
                        ));
                    }
                    Err(PreviewHashError::Unreadable) => {
                        return KeyResult::Flash(
                            "Can't delete: this note couldn't be read for verification.".into(),
                        );
                    }
                }
                let Some(hash) = state.preview_hash.clone() else {
                    return KeyResult::Flash(
                        "Can't delete: this note couldn't be read for verification.".into(),
                    );
                };
                if state.pending_delete.as_ref() == Some(&entry.path) {
                    let path = entry.path.clone();
                    state.pending_delete = None;
                    KeyResult::Forget {
                        path,
                        expected_content_hash: hash,
                    }
                } else {
                    state.pending_delete = Some(entry.path.clone());
                    KeyResult::Flash(format!("Press x again to delete {}", entry.label))
                }
            }
            _ => KeyResult::Ignored,
        },
    }
}

enum PreviewHashError {
    TooLarge { size: u64, limit: u64 },
    Unreadable,
}

/// Hash selected file for forget confirm — align with `forget`: metadata len gate
/// first, then bounded `File::open` + `take(limit)` (never full-read huge files).
fn refresh_preview_hash(
    state: &mut MemoryBrowserState,
    rows: &[Row],
    sel: usize,
) -> Result<(), PreviewHashError> {
    state.preview_hash = None;
    let entry = selected_entry(rows, sel).ok_or(PreviewHashError::Unreadable)?;
    let meta = std::fs::metadata(&entry.path).map_err(|_| PreviewHashError::Unreadable)?;
    if !meta.is_file() {
        return Err(PreviewHashError::Unreadable);
    }
    if meta.len() > MAX_FORGET_FILE_BYTES {
        return Err(PreviewHashError::TooLarge {
            size: meta.len(),
            limit: MAX_FORGET_FILE_BYTES,
        });
    }
    use std::io::Read as _;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    std::fs::File::open(&entry.path)
        .and_then(|f| f.take(MAX_FORGET_FILE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| PreviewHashError::Unreadable)?;
    if bytes.len() as u64 > MAX_FORGET_FILE_BYTES {
        return Err(PreviewHashError::TooLarge {
            size: bytes.len() as u64,
            limit: MAX_FORGET_FILE_BYTES,
        });
    }
    state.preview_hash = Some(blake3::hash(&bytes).to_hex().to_string());
    Ok(())
}

#[derive(Debug)]
pub enum KeyResult {
    Handled,
    Ignored,
    Close,
    Flash(String),
    /// Dual-confirmed forget: archive + tombstone + index drop.
    Forget {
        path: PathBuf,
        expected_content_hash: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaving_filter_clears_filter_and_selection() {
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "1");
        let ctx = Context::new();

        for code in [KeyCode::Esc, KeyCode::Enter] {
            let mut state = MemoryBrowserState {
                selected: 3,
                filter: "prefs".into(),
                focus: MemoryFocus::Filter,
                ..Default::default()
            };

            assert!(matches!(on_key(&ctx, &mut state, code), KeyResult::Handled));
            assert_eq!(state.filter, "");
            assert_eq!(state.selected, 0);
            assert_eq!(state.focus, MemoryFocus::List);
        }
    }

    #[test]
    fn build_rows_disabled_does_not_ensure_layout() {
        let _env = cordis_base::test_env::scoped().home().remove("DOCK_MEMORY");
        let cwd = std::env::current_dir().unwrap();
        let root = MemoryRoot::open_default(&cwd);
        assert!(
            !root.global.topics.exists(),
            "precondition: no layout yet under test DOCK_HOME"
        );
        let rows = build_rows_with_ctx(None, "");
        assert!(rows.is_empty(), "disabled + empty disk → no rows");
        assert!(
            !root.global.topics.exists() && !root.workspace.inbox.exists(),
            "opening /memory must not mkdir when memory is disabled"
        );
    }

    #[test]
    fn build_rows_groups_scopes() {
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "1");
        let cwd = std::env::current_dir().unwrap();
        let root = MemoryRoot::open_default(&cwd);
        root.ensure_layout().unwrap();
        std::fs::write(root.global.topics.join("prefs.md"), "# Prefs\n\nhi\n").unwrap();
        let _ = dock_memory::manifest::refresh_all(&root);
        let rows = build_rows_with_ctx(None, "");
        assert!(rows
            .iter()
            .any(|r| matches!(r, Row::Header { label } if label == "Global")));
        assert!(rows
            .iter()
            .any(|r| matches!(r, Row::File { label, .. } if label.contains("prefs"))));
    }

    #[test]
    fn content_filter_matches_body() {
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "1");
        let cwd = std::env::current_dir().unwrap();
        let root = MemoryRoot::open_default(&cwd);
        root.ensure_layout().unwrap();
        std::fs::write(
            root.global.topics.join("secret-topic.md"),
            "# Secret\n\nunique-zebra-phrase lives here\n",
        )
        .unwrap();
        let _ = dock_memory::manifest::refresh_all(&root);
        let rows = build_rows_with_ctx(None, "unique-zebra-phrase");
        assert!(
            rows.iter()
                .any(|r| matches!(r, Row::File { label, .. } if label.contains("secret-topic"))),
            "rows={rows:?}"
        );
    }

    #[test]
    fn delete_requires_dual_confirm() {
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "1");
        let ctx = Context::new();
        let cwd = std::env::current_dir().unwrap();
        let root = MemoryRoot::open_default(&cwd);
        root.ensure_layout().unwrap();
        std::fs::write(root.global.topics.join("doomed.md"), "# Doomed\n\nbye\n").unwrap();
        let _ = dock_memory::manifest::refresh_all(&root);

        let mut state = MemoryBrowserState::default();
        // Select the doomed file if present
        let rows = build_rows_with_ctx(None, "");
        let idxs = selectable_indices(&rows);
        if let Some((sel, _)) = idxs.iter().enumerate().find(
            |(_, &ri)| matches!(&rows[ri], Row::File { label, .. } if label.contains("doomed")),
        ) {
            state.selected = sel;
        }
        let first = on_key(&ctx, &mut state, KeyCode::Char('x'));
        assert!(matches!(first, KeyResult::Flash(_)), "{first:?}");
        assert!(state.pending_delete.is_some());
        let second = on_key(&ctx, &mut state, KeyCode::Char('x'));
        assert!(matches!(second, KeyResult::Forget { .. }), "{second:?}");
    }
}
