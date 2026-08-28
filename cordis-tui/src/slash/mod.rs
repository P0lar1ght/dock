//! Slash catalog + snapshot. Matching is prefix/substring (Grok uses nucleo).
//! Dropdown chrome is copied from pager `views/slash_dropdown.rs`.

mod dropdown;

pub use dropdown::{desired_item_rows, render_dropdown, SuggestionRow};

/// Copied from grok `slash::MAX_VISIBLE_SUGGESTIONS`.
pub const MAX_VISIBLE_SUGGESTIONS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCmd {
    New,
    Resume,
    Help,
    History,
    Find,
    Quit,
}

pub struct SlashDef {
    pub cmd: SlashCmd,
    pub name: &'static str,
    pub display: &'static str,
    pub description: &'static str,
}

pub const CATALOG: &[SlashDef] = &[
    SlashDef {
        cmd: SlashCmd::New,
        name: "new",
        display: "/new",
        description: "Start a new session",
    },
    SlashDef {
        cmd: SlashCmd::Resume,
        name: "resume",
        display: "/resume",
        description: "Resume a previous session",
    },
    SlashDef {
        cmd: SlashCmd::Help,
        name: "help",
        display: "/help",
        description: "Show slash commands",
    },
    SlashDef {
        cmd: SlashCmd::History,
        name: "history",
        display: "/history",
        description: "Search prompt history",
    },
    SlashDef {
        cmd: SlashCmd::Find,
        name: "find",
        display: "/find",
        description: "Search conversation",
    },
    SlashDef {
        cmd: SlashCmd::Quit,
        name: "quit",
        display: "/quit",
        description: "Quit grok",
    },
];

#[derive(Debug, Clone)]
pub struct SlashSnapshot {
    pub open: bool,
    pub selected: usize,
    pub matches: Vec<SuggestionRow>,
}

impl SlashSnapshot {
    pub fn current(&self) -> Option<&SuggestionRow> {
        self.matches.get(self.selected)
    }
}

/// `text` is the composer buffer. `selected` is the highlighted match index.
pub fn snapshot(text: &str, selected: usize) -> SlashSnapshot {
    let Some(query) = slash_query(text) else {
        return SlashSnapshot {
            open: false,
            selected: 0,
            matches: Vec::new(),
        };
    };
    let q = query.to_ascii_lowercase();
    let matches: Vec<SuggestionRow> = CATALOG
        .iter()
        .filter(|d| q.is_empty() || d.name.starts_with(&q) || d.display.contains(&q))
        .map(|d| SuggestionRow {
            display: d.display.to_string(),
            description: d.description.to_string(),
            cmd: d.cmd,
        })
        .collect();
    let selected = if matches.is_empty() {
        0
    } else {
        selected.min(matches.len() - 1)
    };
    SlashSnapshot {
        open: !matches.is_empty(),
        selected,
        matches,
    }
}

/// Command name after `/` on a single-line slash buffer. `None` if not a slash prompt.
fn slash_query(text: &str) -> Option<&str> {
    if text.contains('\n') {
        return None;
    }
    let rest = text.strip_prefix('/')?;
    Some(rest.split(char::is_whitespace).next().unwrap_or(""))
}

pub fn command_for_submit(text: &str) -> Option<SlashCmd> {
    let query = slash_query(text)?;
    if query.is_empty() {
        return None;
    }
    CATALOG
        .iter()
        .find(|d| d.name == query || d.display.trim_start_matches('/') == query)
        .map(|d| d.cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slash_lists_catalog() {
        let snap = snapshot("/", 0);
        assert!(snap.open);
        assert_eq!(snap.matches.len(), CATALOG.len());
        assert_eq!(snap.matches[0].display, "/new");
    }

    #[test]
    fn prefix_filters() {
        let snap = snapshot("/re", 0);
        assert_eq!(snap.matches.len(), 1);
        assert_eq!(snap.matches[0].display, "/resume");
    }

    #[test]
    fn prose_is_not_slash() {
        assert!(!snapshot("hello /new", 0).open);
        assert!(command_for_submit("hello").is_none());
    }

    #[test]
    fn submit_maps_name() {
        assert_eq!(command_for_submit("/quit"), Some(SlashCmd::Quit));
        assert_eq!(command_for_submit("/new"), Some(SlashCmd::New));
    }
}
