//! Resume / help / history / find overlays on Grok picker chrome.

use cordis_spine::ArchivedSession;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use crate::grok::picker::{
    render_floating_frame, render_fullscreen_frame, render_picker_list, PickerHits, PickerRow,
};
use crate::slash::SlashCmd;
use crate::theme::Theme;

#[derive(Debug, Clone, Default)]
pub enum Overlay {
    #[default]
    None,
    Resume {
        selected: usize,
        query: String,
    },
    Help {
        selected: usize,
        query: String,
    },
    History {
        selected: usize,
        query: String,
    },
    Find {
        selected: usize,
        query: String,
    },
}

impl Overlay {
    pub fn is_open(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn close(&mut self) {
        *self = Self::None;
    }

    #[allow(dead_code)]
    pub fn query(&self) -> &str {
        match self {
            Self::None => "",
            Self::Resume { query, .. }
            | Self::Help { query, .. }
            | Self::History { query, .. }
            | Self::Find { query, .. } => query,
        }
    }

    pub fn push_char(&mut self, c: char) {
        if let Some(q) = self.query_mut() {
            q.push(c);
        }
        self.set_selected(0);
    }

    pub fn push_str(&mut self, s: &str) {
        if let Some(q) = self.query_mut() {
            q.push_str(s);
        }
        self.set_selected(0);
    }

    pub fn backspace(&mut self) {
        if let Some(q) = self.query_mut() {
            q.pop();
        }
        self.set_selected(0);
    }

    pub fn move_sel(&mut self, delta: i16, len: usize) {
        if len == 0 {
            return;
        }
        let next = (self.selected() as i32 + delta as i32).rem_euclid(len as i32) as usize;
        self.set_selected(next);
    }

    pub fn selected(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Resume { selected, .. }
            | Self::Help { selected, .. }
            | Self::History { selected, .. }
            | Self::Find { selected, .. } => *selected,
        }
    }

    pub fn set_selected(&mut self, selected: usize) {
        match self {
            Self::None => {}
            Self::Resume { selected: s, .. }
            | Self::Help { selected: s, .. }
            | Self::History { selected: s, .. }
            | Self::Find { selected: s, .. } => *s = selected,
        }
    }

    fn query_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::None => None,
            Self::Resume { query, .. }
            | Self::Help { query, .. }
            | Self::History { query, .. }
            | Self::Find { query, .. } => Some(query),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpKind {
    Hint,
    Slash(SlashCmd),
}

#[derive(Debug, Clone, Copy)]
pub struct HelpRow {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: HelpKind,
}

#[allow(dead_code)]
enum HelpEntry {
    Header(&'static str),
    Row(HelpRow),
}

const HELP: &[HelpEntry] = &[
    HelpEntry::Header("Essentials"),
    HelpEntry::Row(HelpRow {
        key: "enter",
        label: "Send prompt",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "esc",
        label: "Clear prompt / close",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+q",
        label: "Quit",
        kind: HelpKind::Slash(SlashCmd::Quit),
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+w",
        label: "New session",
        kind: HelpKind::Slash(SlashCmd::New),
    }),
    HelpEntry::Row(HelpRow {
        key: "f3",
        label: "Resume session",
        kind: HelpKind::Slash(SlashCmd::Resume),
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+.",
        label: "Shortcuts help",
        kind: HelpKind::Slash(SlashCmd::Help),
    }),
    HelpEntry::Header("Session"),
    HelpEntry::Row(HelpRow {
        key: "/new",
        label: "Start a new session",
        kind: HelpKind::Slash(SlashCmd::New),
    }),
    HelpEntry::Row(HelpRow {
        key: "/resume",
        label: "Resume a previous session",
        kind: HelpKind::Slash(SlashCmd::Resume),
    }),
    HelpEntry::Row(HelpRow {
        key: "/history",
        label: "Search prompt history",
        kind: HelpKind::Slash(SlashCmd::History),
    }),
    HelpEntry::Row(HelpRow {
        key: "/find",
        label: "Search scrollback",
        kind: HelpKind::Slash(SlashCmd::Find),
    }),
    HelpEntry::Row(HelpRow {
        key: "/quit",
        label: "Quit grok",
        kind: HelpKind::Slash(SlashCmd::Quit),
    }),
    HelpEntry::Header("Prompt"),
    HelpEntry::Row(HelpRow {
        key: "↑↓",
        label: "Recall history",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "tab",
        label: "Complete slash command",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Header("Scrollback"),
    HelpEntry::Row(HelpRow {
        key: "pgup",
        label: "Scroll conversation",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "click",
        label: "Expand tool card",
        kind: HelpKind::Hint,
    }),
];

pub fn matches_query(hay: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    hay.to_ascii_lowercase()
        .contains(&query.to_ascii_lowercase())
}

pub fn filter_sessions<'a>(items: &'a [ArchivedSession], query: &str) -> Vec<&'a ArchivedSession> {
    items
        .iter()
        .filter(|s| matches_query(&s.title, query) || matches_query(&s.id, query))
        .collect()
}

pub fn filter_strings<'a>(items: &'a [String], query: &str) -> Vec<(usize, &'a str)> {
    items
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, s)| matches_query(s, query))
        .map(|(i, s)| (i, s.as_str()))
        .collect()
}

pub fn filter_help(query: &str) -> Vec<&'static HelpRow> {
    HELP.iter()
        .filter_map(|e| match e {
            HelpEntry::Row(row)
                if matches_query(row.key, query) || matches_query(row.label, query) =>
            {
                Some(row)
            }
            _ => None,
        })
        .collect()
}

pub fn help_at(query: &str, selected: usize) -> Option<&'static HelpRow> {
    filter_help(query).get(selected).copied()
}

/// `fullscreen` = welcome-screen session picker (Grok `PickerMode::FullScreen`).
pub fn render_overlay(
    buf: &mut Buffer,
    area: Rect,
    title: &str,
    query: &str,
    rows: &[PickerRow<'_>],
    fullscreen: bool,
) -> PickerHits {
    let theme = Theme::current();
    let frame = if fullscreen {
        render_fullscreen_frame(buf, area, &theme, Some(title), false)
    } else {
        render_floating_frame(buf, area, &theme, false)
    };
    let Some(frame) = frame else {
        return PickerHits::default();
    };
    let skip_top = if fullscreen { 0 } else { 1 };
    let row_hits = render_picker_list(buf, frame.content, &theme, query, rows, skip_top);
    PickerHits {
        close_button: frame.close_button,
        rows: row_hits,
    }
}

pub fn hit_index(hits: &PickerHits, column: u16, row: u16) -> Option<usize> {
    let pos = Position {
        x: column,
        y: row,
    };
    if hits.close_button.contains(pos) {
        return None;
    }
    hits.rows
        .iter()
        .find(|(_, r)| r.contains(pos))
        .map(|(i, _)| *i)
}

pub fn hit_close(hits: &PickerHits, column: u16, row: u16) -> bool {
    hits.close_button.contains(Position {
        x: column,
        y: row,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_filters_case_insensitive() {
        assert!(matches_query("Hello World", "hello"));
        assert!(!matches_query("Hello World", "zzz"));
    }

    #[test]
    fn help_filter_finds_resume() {
        let rows = filter_help("resume");
        assert!(rows.iter().any(|r| r.key.contains("resume") || r.label.contains("Resume")));
    }
}
