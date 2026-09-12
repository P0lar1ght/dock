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

/// Buttons that fit in `width` columns. The full row is 60 cols; a narrower
/// pane drops the less useful buttons (`[Copy Image Path]` first, then
/// `[Copy Source]`) instead of letting the row word-wrap — a wrapped row
/// matched `is_affordance_text` on both halves, which double-counted blocks
/// and mis-assigned `[Copy Source]` sources. Drawing and hit-testing share
/// this one button list, so the two can never drift apart.
pub fn affordance_buttons_for_width(width: u16) -> Vec<AffordanceButton> {
    let start = UnicodeWidthStr::width(MERMAID_LABEL) as u16 + AFFORDANCE_GAP;
    let all = affordance_buttons(start);
    // `[Copy Image Path]` 最次要，先省。保留子集按顺序重新排布列位。
    let pick = |kinds: &[AffordanceKind]| -> (u16, Vec<AffordanceButton>) {
        let mut col = start;
        let buttons = kinds
            .iter()
            .filter_map(|k| all.iter().find(|b| b.kind == *k))
            .map(|b| {
                let btn = AffordanceButton { col, ..*b };
                col += UnicodeWidthStr::width(b.label) as u16 + AFFORDANCE_GAP;
                btn
            })
            .collect();
        (col, buttons)
    };
    for kinds in [
        [
            AffordanceKind::Open,
            AffordanceKind::CopyPath,
            AffordanceKind::CopySource,
        ]
        .as_slice(),
        [AffordanceKind::Open, AffordanceKind::CopySource].as_slice(),
        [AffordanceKind::Open].as_slice(),
        [].as_slice(),
    ] {
        let (end, buttons) = pick(kinds);
        // 末尾按钮后面不画间隔，所以 `end` 要减掉一格；`line_for_width`
        // 在没有按钮时同样不画标签后的间隔，两边口径必须一致。
        let row_w = end.saturating_sub(AFFORDANCE_GAP);
        if row_w <= width {
            return buttons;
        }
    }
    Vec::new()
}

pub fn hit_kind(rel_col: u16, width: u16) -> Option<AffordanceKind> {
    affordance_buttons_for_width(width)
        .into_iter()
        .find_map(|b| {
            let w = UnicodeWidthStr::width(b.label) as u16;
            (rel_col >= b.col && rel_col < b.col + w).then_some(b.kind)
        })
}

pub fn line_for_width(theme: &Theme, width: u16) -> Line<'static> {
    let dim = theme.dim();
    let muted = theme.muted();
    let buttons = affordance_buttons_for_width(width);
    // 没有按钮时不画标签后的间隔：那三列没人用，却会让实际行宽比
    // `affordance_buttons_for_width` 算出的预算多 3 列，窄面板下被裁。
    // 连标签都放不下时截断标签本身 —— 这一行不参与 wrap，超宽只能被裁。
    let label = if buttons.is_empty() {
        crate::grok::line_utils::truncate_str(MERMAID_LABEL, width as usize)
    } else {
        format!("{MERMAID_LABEL}   ")
    };
    let mut spans = vec![Span::styled(label, dim)];
    for (i, b) in buttons.iter().enumerate() {
        let label = if i + 1 < buttons.len() {
            format!("{}   ", b.label)
        } else {
            b.label.to_string()
        };
        spans.push(Span::styled(label, muted));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    /// 这一行**不参与 word wrap**（见 `scrollback::assistant`），所以它绝不
    /// 能比面板宽 —— 宽了就只能被裁。按钮集合是按宽度选的，这条不变式是
    /// 「窄窗下点错 mermaid 块」那个缺陷真正的防线。
    #[test]
    fn the_affordance_row_never_exceeds_the_pane() {
        let theme = Theme::groknight();
        for pane in 1..=80u16 {
            let line = line_for_width(&theme, pane);
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let w = UnicodeWidthStr::width(text.as_str()) as u16;
            assert!(w <= pane, "pane={pane} 画出 {w} 列: {text:?}");
        }
    }

    /// 画出来的列位和 `hit_kind` 必须是同一套按钮表算的，否则点击错位。
    #[test]
    fn drawn_columns_match_the_hit_test() {
        let theme = Theme::groknight();
        for pane in [80u16, 60, 48, 40, 30, 24, 12] {
            let line = line_for_width(&theme, pane);
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            for b in affordance_buttons_for_width(pane) {
                let at = UnicodeWidthStr::width(&text[..text.find(b.label).unwrap()]) as u16;
                assert_eq!(
                    at, b.col,
                    "pane={pane} {} 画在 {at}，判定在 {}",
                    b.label, b.col
                );
                assert_eq!(hit_kind(b.col, pane), Some(b.kind), "pane={pane}");
            }
        }
    }
}
