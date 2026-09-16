//! TokyoNight Storm — copied from grok-build pager-render `theme/tokyonight.rs`.
//! `Theme` lives in `theme.rs`; this file only fills the palette.

use ratatui::style::{Color, Modifier};

use super::Theme;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

#[allow(dead_code)]
mod palette {
    use super::*;
    pub const BG: Color = rgb(26, 27, 38);
    pub const BG_DARK: Color = rgb(22, 22, 30);
    pub const BG_HIGHLIGHT: Color = rgb(41, 46, 66);
    /// `bg_light` 专用，比 [`BG_HIGHLIGHT`] 再亮一档。原值压在 `BG_STORM`
    /// `rgb(36,40,59)` 上只差 5/255，三个主题里对比最弱的一个。
    pub const BG_BAND: Color = rgb(54, 61, 86);
    pub const BG_STORM: Color = rgb(36, 40, 59);
    pub const BG_STORM_DARK: Color = rgb(31, 35, 53);
    pub const FG: Color = rgb(192, 202, 245);
    pub const FG_DARK: Color = rgb(169, 177, 214);
    pub const FG_GUTTER: Color = rgb(59, 66, 97);
    pub const COMMENT: Color = rgb(86, 95, 137);
    pub const DARK3: Color = rgb(84, 92, 126);
    pub const DARK5: Color = rgb(115, 122, 162);
    pub const BLUE: Color = rgb(122, 162, 247);
    pub const BLUE0: Color = rgb(61, 89, 161);
    pub const BLUE1: Color = rgb(42, 195, 222);
    pub const CYAN: Color = rgb(125, 207, 255);
    pub const GREEN: Color = rgb(158, 206, 106);
    pub const GREEN1: Color = rgb(115, 218, 202);
    pub const MAGENTA: Color = rgb(187, 154, 247);
    pub const ORANGE: Color = rgb(255, 158, 100);
    pub const PURPLE: Color = rgb(157, 124, 216);
    pub const RED: Color = rgb(247, 118, 142);
    pub const RED1: Color = rgb(219, 75, 75);
    pub const TEAL: Color = rgb(26, 188, 156);
    pub const YELLOW: Color = rgb(224, 175, 104);
}
use palette::*;

impl Theme {
    /// TokyoNight Storm theme. Copied from grok `Theme::tokyonight`.
    pub const fn tokyonight() -> Self {
        Self {
            bg_base: BG_STORM,
            bg_light: BG_BAND,
            bg_dark: BG_HIGHLIGHT,
            bg_highlight: BG_HIGHLIGHT,
            bg_hover: rgb(40, 49, 76),
            bg_terminal: BG,
            accent_user: BLUE,
            accent_assistant: MAGENTA,
            accent_thinking: FG_GUTTER,
            accent_tool: DARK5,
            accent_system: BLUE,
            accent_error: RED,
            accent_success: GREEN,
            accent_running: MAGENTA,
            accent_skill: rgb(100, 180, 170),
            text_primary: FG,
            text_secondary: FG_DARK,
            gray_dim: FG_GUTTER,
            gray: COMMENT,
            gray_bright: DARK5,
            command: YELLOW,
            path: ORANGE,
            running: CYAN,
            warning: YELLOW,
            fuzzy_accent: BLUE,
            accent_plan: rgb(230, 180, 50),
            accent_verify: MAGENTA,
            accent_remember: Color::Rgb(139, 195, 74),
            selection_border: rgb(58, 72, 115),
            prompt_border: rgb(60, 75, 120),
            prompt_border_active: rgb(75, 92, 140),
            hover_border: rgb(55, 58, 80),
            accent_model: TEAL,
            scrollbar_bg: BG_STORM_DARK,
            scrollbar_fg: BG_HIGHLIGHT,
            diff_delete_bg: rgb(85, 15, 20),
            diff_delete_fg: RED,
            diff_insert_bg: rgb(15, 65, 20),
            diff_insert_fg: GREEN,
            diff_equal_fg: COMMENT,
            diff_gutter_fg: COMMENT,
            bg_visual: rgb(40, 52, 87),
            paste_bg: BG_STORM_DARK,
            paste_fg: FG_DARK,
            paste_dim: FG_GUTTER,
            md_heading_h1: TEAL,
            md_heading_h1_mod: Modifier::BOLD,
            md_heading_h2: BLUE,
            md_heading_h2_mod: Modifier::BOLD,
            md_heading_h3: ORANGE,
            md_heading_h3_mod: Modifier::BOLD,
            md_heading_h4: RED,
            md_heading_h4_mod: Modifier::BOLD,
            md_heading_h5: GREEN,
            md_heading_h5_mod: Modifier::BOLD,
            md_heading_h6: MAGENTA,
            md_heading_h6_mod: Modifier::BOLD,
            md_code: GREEN1,
            md_task_checked: CYAN,
            md_task_unchecked: BLUE,
            md_muted: COMMENT,
            md_code_bg: BG_HIGHLIGHT,
            md_text: FG,
            link_fg: BLUE,
        }
    }
}
