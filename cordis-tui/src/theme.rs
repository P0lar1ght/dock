//! Copied from grok-build/.../xai-grok-pager-render/src/theme/tokyonight.rs (`Theme` struct).
//! Palette constructors stay in groknight.rs. Do not restyle.

use ratatui::style::{Color, Modifier, Style};

#[path = "grokday.rs"]
mod grokday;
#[path = "groknight.rs"]
mod groknight;
#[path = "tokyonight.rs"]
mod tokyonight;

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct Theme {
    // Backgrounds
    pub bg_base: Color,
    pub bg_light: Color,
    pub bg_dark: Color,
    pub bg_highlight: Color,
    pub bg_hover: Color, // Mouse hover row in dropdowns — between bg_highlight and bg_visual
    pub bg_terminal: Color, // For terminal output blocks (currently unused, using bg_dark instead)

    // Accent colors (for vertical lines)
    pub accent_user: Color,
    pub accent_assistant: Color,
    pub accent_thinking: Color,
    pub accent_tool: Color,
    pub accent_system: Color,
    pub accent_error: Color,
    pub accent_success: Color,
    pub accent_running: Color, // For tools that are currently running
    pub accent_skill: Color,   // For skill invocations (slash command skills)

    // Text colors
    pub text_primary: Color,
    pub text_secondary: Color,

    // Gray scale (dim → medium → bright)
    // Every theme defines these three; they provide a consistent hierarchy
    // for secondary/meta text across all themes.
    pub gray_dim: Color,    // Dimmest — meta punctuation (`$`, `(+N/-M)`, etc.)
    pub gray: Color,        // Medium — muted text, comments, collapsed content
    pub gray_bright: Color, // Brightest — tool accents, secondary labels

    // Semantic colors
    pub command: Color, // Yellow for shell commands
    pub path: Color,    // Orange for file paths
    pub running: Color, // Cyan for running indicator
    pub warning: Color, // Yellow/amber for warnings

    // Search
    pub fuzzy_accent: Color, // Highlight color for fuzzy search matches

    // Plan mode
    pub accent_plan: Color, // Golden accent for plan mode indicator

    // Context-window overhead category (context info block)
    pub accent_verify: Color, // Violet accent, distinct from plan gold

    // Remember mode
    pub accent_remember: Color, // Green accent for # remember mode

    // Selection
    pub selection_border: Color,
    pub hover_border: Color,
    pub prompt_border: Color,
    pub prompt_border_active: Color,

    // Prompt info
    pub accent_model: Color, // Model name in prompt info line

    // Scrollbar
    pub scrollbar_bg: Color,
    pub scrollbar_fg: Color,

    // Diff colors
    pub diff_delete_bg: Color,
    pub diff_delete_fg: Color,
    pub diff_insert_bg: Color,
    pub diff_insert_fg: Color,
    pub diff_equal_fg: Color,
    pub diff_gutter_fg: Color,

    // Visual selection / dropdown selection background
    pub bg_visual: Color,

    // Paste elements (chip + preview overlay)
    pub paste_bg: Color,
    pub paste_fg: Color,
    pub paste_dim: Color,

    // Markdown rendering colors — used by md_style.rs for headings, code
    // blocks, inline code, links, etc.  These default to the corresponding
    // top-level theme colors but can be overridden per-theme to customise
    // markdown appearance independently.
    pub md_heading_h1: Color,        // H1 headings
    pub md_heading_h1_mod: Modifier, // H1 extra effects
    pub md_heading_h2: Color,        // H2 headings, task unchecked, tables
    pub md_heading_h2_mod: Modifier, // H2 extra effects
    pub md_heading_h3: Color,        // H3 headings, code language tag
    pub md_heading_h3_mod: Modifier, // H3 extra effects
    pub md_heading_h4: Color,        // H4 headings
    pub md_heading_h4_mod: Modifier, // H4 extra effects
    pub md_heading_h5: Color,        // H5 headings, link titles
    pub md_heading_h5_mod: Modifier, // H5 extra effects
    pub md_heading_h6: Color,        // H6 headings
    pub md_heading_h6_mod: Modifier, // H6 extra effects
    pub md_code: Color,              // Inline code, code block delimiters
    pub md_task_checked: Color,      // Task checked
    pub md_task_unchecked: Color,    // Task unchecked
    pub md_muted: Color,             // Blockquotes, list items, rules, links
    pub md_code_bg: Color,           // Code block background
    pub md_text: Color,              // Default body text (plain paragraphs, strong, emphasis)
    pub link_fg: Color,              // Clickable link text color
}

use std::sync::Mutex;

static KIND: Mutex<ThemeKind> = Mutex::new(ThemeKind::GrokNight);

/// Copied from grok pager-render `ThemeKind` (subset we baked palettes for).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeKind {
    GrokNight,
    GrokDay,
    TokyoNight,
}

impl ThemeKind {
    pub const ALL: &[ThemeKind] = &[
        ThemeKind::GrokNight,
        ThemeKind::GrokDay,
        ThemeKind::TokyoNight,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            Self::GrokNight => "groknight",
            Self::GrokDay => "grokday",
            Self::TokyoNight => "tokyonight",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "groknight" | "night" => Some(Self::GrokNight),
            "grokday" | "day" | "light" => Some(Self::GrokDay),
            "tokyonight" | "tokyo" => Some(Self::TokyoNight),
            _ => None,
        }
    }
}

impl Theme {
    pub fn current_kind() -> ThemeKind {
        *KIND.lock().unwrap()
    }

    pub fn apply_kind(kind: ThemeKind) {
        *KIND.lock().unwrap() = kind;
    }

    /// Grok reads `cache::current_kind()`.
    pub fn current() -> Self {
        match Self::current_kind() {
            ThemeKind::GrokNight => Self::groknight(),
            ThemeKind::GrokDay => Self::grokday(),
            ThemeKind::TokyoNight => Self::tokyonight(),
        }
    }

    /// Get a style with the given foreground color.
    pub const fn fg(self, color: Color) -> Style {
        Style::new().fg(color)
    }

    /// Get a style with muted text (gray — medium).
    ///
    /// When `gray` is [`Color::Reset`] (terminal-native / minimal palette),
    /// de-emphasize with [`Modifier::DIM`] instead of painting ANSI bright
    /// black — dim scales the terminal's own default fg, so contrast stays
    /// polarity-safe. RGB themes keep an explicit gray foreground.
    pub const fn muted(self) -> Style {
        match self.gray {
            Color::Reset => Style::new().add_modifier(Modifier::DIM),
            c => Style::new().fg(c),
        }
    }

    /// Get a style with dim text (gray_dim — dimmest).
    ///
    /// Same Reset→DIM rule as [`Self::muted`] for the terminal-native palette.
    pub const fn dim(self) -> Style {
        match self.gray_dim {
            Color::Reset => Style::new().add_modifier(Modifier::DIM),
            c => Style::new().fg(c),
        }
    }

    /// Get a style for primary text.
    pub const fn primary(self) -> Style {
        Style::new().fg(self.text_primary)
    }
}
