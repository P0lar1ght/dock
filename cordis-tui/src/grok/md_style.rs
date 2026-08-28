//! Theme-aware markdown rendering style.
//!
//! Copied from grok-build/.../xai-grok-pager-render/src/theme/md_style.rs.
//! `Theme` is cordis-tui's baked GrokNight (same `md_*` fields).

use anstyle::{Ansi256Color, AnsiColor, Color, Style};
use cordis_markdown::MarkdownStyle;

use crate::theme::Theme;

fn to_anstyle(c: ratatui::style::Color) -> Option<Color> {
    Some(match c {
        ratatui::style::Color::Reset => return None,
        ratatui::style::Color::Rgb(r, g, b) => Color::Rgb(anstyle::RgbColor(r, g, b)),
        ratatui::style::Color::Indexed(n) => Color::Ansi256(Ansi256Color(n)),
        ratatui::style::Color::Black => Color::Ansi(AnsiColor::Black),
        ratatui::style::Color::Red => Color::Ansi(AnsiColor::Red),
        ratatui::style::Color::Green => Color::Ansi(AnsiColor::Green),
        ratatui::style::Color::Yellow => Color::Ansi(AnsiColor::Yellow),
        ratatui::style::Color::Blue => Color::Ansi(AnsiColor::Blue),
        ratatui::style::Color::Magenta => Color::Ansi(AnsiColor::Magenta),
        ratatui::style::Color::Cyan => Color::Ansi(AnsiColor::Cyan),
        ratatui::style::Color::Gray => Color::Ansi(AnsiColor::White),
        ratatui::style::Color::DarkGray => Color::Ansi(AnsiColor::BrightBlack),
        ratatui::style::Color::LightRed => Color::Ansi(AnsiColor::BrightRed),
        ratatui::style::Color::LightGreen => Color::Ansi(AnsiColor::BrightGreen),
        ratatui::style::Color::LightYellow => Color::Ansi(AnsiColor::BrightYellow),
        ratatui::style::Color::LightBlue => Color::Ansi(AnsiColor::BrightBlue),
        ratatui::style::Color::LightMagenta => Color::Ansi(AnsiColor::BrightMagenta),
        ratatui::style::Color::LightCyan => Color::Ansi(AnsiColor::BrightCyan),
        ratatui::style::Color::White => Color::Ansi(AnsiColor::BrightWhite),
    })
}

fn fg(c: ratatui::style::Color) -> Style {
    Style::new().fg_color(to_anstyle(c))
}

fn bg(c: ratatui::style::Color) -> Style {
    Style::new().bg_color(to_anstyle(c))
}

fn modifier_to_anstyle(m: ratatui::style::Modifier) -> Style {
    let mut s = Style::new();
    if m.contains(ratatui::style::Modifier::BOLD) {
        s = s.bold();
    }
    if m.contains(ratatui::style::Modifier::ITALIC) {
        s = s.italic();
    }
    if m.contains(ratatui::style::Modifier::UNDERLINED) {
        s = s.underline();
    }
    if m.contains(ratatui::style::Modifier::DIM) {
        s = s.dimmed();
    }
    if m.contains(ratatui::style::Modifier::HIDDEN) {
        s = s.hidden();
    }
    if m.contains(ratatui::style::Modifier::CROSSED_OUT) {
        s = s.strikethrough();
    }
    s
}

fn heading_inner_styles(
    colors: [ratatui::style::Color; 6],
    mods: [ratatui::style::Modifier; 6],
) -> [Style; 6] {
    std::array::from_fn(|i| {
        let color_style = fg(colors[i]);
        let mod_style = modifier_to_anstyle(mods[i]);
        let mut s = color_style;
        let effects = mod_style.get_effects();
        if !effects.is_plain() {
            s = s.effects(s.get_effects() | effects);
        }
        s
    })
}

fn heading_outer_styles(colors: [ratatui::style::Color; 6]) -> [Style; 6] {
    colors.map(|c| fg(c).dimmed().hidden())
}

/// Get the theme-aware markdown style.
pub fn style() -> MarkdownStyle {
    build_style()
}

fn build_style() -> MarkdownStyle {
    let theme = Theme::current();

    let heading_colors = [
        theme.md_heading_h1,
        theme.md_heading_h2,
        theme.md_heading_h3,
        theme.md_heading_h4,
        theme.md_heading_h5,
        theme.md_heading_h6,
    ];
    let heading_mods = [
        theme.md_heading_h1_mod,
        theme.md_heading_h2_mod,
        theme.md_heading_h3_mod,
        theme.md_heading_h4_mod,
        theme.md_heading_h5_mod,
        theme.md_heading_h6_mod,
    ];

    MarkdownStyle {
        heading_inner: heading_inner_styles(heading_colors, heading_mods),
        heading_outer: heading_outer_styles(heading_colors),
        strong_inner: fg(theme.md_text).bold(),
        strong_outer: Style::new().dimmed().hidden(),
        emphasis_inner: fg(theme.md_text).italic(),
        emphasis_outer: Style::new().dimmed().hidden(),
        strikethrough_inner: fg(theme.md_text).strikethrough(),
        strikethrough_outer: Style::new().dimmed().hidden(),
        inline_code_inner: fg(theme.md_code).bold(),
        inline_code_outer: fg(theme.md_code).dimmed().hidden(),
        blockquote_outer: fg(theme.md_muted).dimmed(),
        task_checked: fg(theme.md_task_checked),
        task_unchecked: fg(theme.md_task_unchecked).dimmed(),
        list_item: fg(theme.md_muted),
        rule: fg(theme.md_muted),
        link_outer: fg(theme.md_muted),
        link_text: fg(theme.link_fg).underline(),
        link_url: fg(theme.md_muted),
        link_title: fg(theme.md_heading_h5),
        code_outer: fg(theme.md_code).dimmed().hidden(),
        code_language: fg(theme.md_heading_h3).hidden(),
        code_untagged: fg(theme.md_text),
        code_background: bg(theme.md_code_bg),
        table_outer: fg(theme.md_heading_h2).hidden(),
        text: fg(theme.md_text),
        math: fg(theme.md_text).italic(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_maps_to_no_color() {
        assert_eq!(to_anstyle(ratatui::style::Color::Reset), None);
        assert_eq!(fg(ratatui::style::Color::Reset).get_fg_color(), None);
        assert_eq!(bg(ratatui::style::Color::Reset).get_bg_color(), None);
    }

    #[test]
    fn named_colors_map_concretely() {
        assert_eq!(
            to_anstyle(ratatui::style::Color::DarkGray),
            Some(Color::Ansi(AnsiColor::BrightBlack))
        );
        assert_eq!(
            to_anstyle(ratatui::style::Color::Gray),
            Some(Color::Ansi(AnsiColor::White))
        );
        assert_eq!(
            to_anstyle(ratatui::style::Color::Red),
            Some(Color::Ansi(AnsiColor::Red))
        );
    }
}
