//! Affordance row copied from grok pager `scrollback/blocks/mermaid_content.rs`.

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

pub const MERMAID_LABEL: &str = "\u{25c7} mermaid";
pub const AFFORDANCE_OPEN: &str = "[Open Image]";
pub const AFFORDANCE_COPY_PATH: &str = "[Copy Image Path]";
pub const AFFORDANCE_COPY_SOURCE: &str = "[Copy Source]";
const AFFORDANCE_GAP: u16 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffordanceKind {
    Open,
    CopyPath,
    CopySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AffordanceButton {
    pub label: &'static str,
    pub kind: AffordanceKind,
    pub col: u16,
}

pub fn affordance_buttons(start_col: u16) -> [AffordanceButton; 3] {
    let specs = [
        (AFFORDANCE_OPEN, AffordanceKind::Open),
        (AFFORDANCE_COPY_PATH, AffordanceKind::CopyPath),
        (AFFORDANCE_COPY_SOURCE, AffordanceKind::CopySource),
    ];
    let mut col = start_col;
    specs.map(|(label, kind)| {
        let button = AffordanceButton { label, kind, col };
        col += UnicodeWidthStr::width(label) as u16 + AFFORDANCE_GAP;
        button
    })
}

pub fn affordance_row() -> (u16, [AffordanceButton; 3]) {
    let start = UnicodeWidthStr::width(MERMAID_LABEL) as u16 + AFFORDANCE_GAP;
    (0, affordance_buttons(start))
}

pub fn hit_kind(rel_col: u16) -> Option<AffordanceKind> {
    let (_, buttons) = affordance_row();
    buttons.iter().find_map(|b| {
        let w = UnicodeWidthStr::width(b.label) as u16;
        (rel_col >= b.col && rel_col < b.col + w).then_some(b.kind)
    })
}

pub fn line(theme: &Theme) -> Line<'static> {
    let dim = theme.dim();
    let muted = theme.muted();
    Line::from(vec![
        Span::styled(format!("{MERMAID_LABEL}   "), dim),
        Span::styled(format!("{AFFORDANCE_OPEN}   "), muted),
        Span::styled(format!("{AFFORDANCE_COPY_PATH}   "), muted),
        Span::styled(AFFORDANCE_COPY_SOURCE.to_string(), muted),
    ])
}

pub fn is_affordance_text(text: &str) -> bool {
    text.contains(AFFORDANCE_COPY_SOURCE) || text.contains(AFFORDANCE_OPEN)
}
