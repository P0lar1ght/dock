//! Chrome glyphs copied from grok-build/.../xai-grok-pager-render/src/glyphs.rs.
//! Legacy ConHost fallbacks omitted (this TUI is not the Windows raster console).

/// `"❯ "` — user prompt prefix. Always 2 columns.
pub fn prompt_arrow() -> &'static str {
    "\u{276F} "
}

/// `"◆"` — scrollback tool-card bullet. Always 1 column.
pub fn diamond_filled() -> &'static str {
    "\u{25C6}"
}

/// `"◇"` — mermaid affordance marker. Always 1 column.
pub fn diamond_hollow() -> &'static str {
    "\u{25C7}"
}

/// `"┃"` — permission overlay accent. Always 1 column.
pub fn accent_bar() -> &'static str {
    "\u{2503}"
}

/// `"●"` selected radio. Always 1 column.
pub fn filled_dot() -> &'static str {
    "\u{25CF}"
}

/// `"○"` idle radio. Always 1 column.
pub fn hollow_dot() -> &'static str {
    "\u{25CB}"
}

/// `"⇣"` download tokens. Always 1 column.
pub fn token_down() -> &'static str {
    "\u{21E3}"
}

/// `"⇡"` upload tokens. Always 1 column.
pub fn token_up() -> &'static str {
    "\u{21E1}"
}

/// `"[✗]"` close button. Always 3 columns.
pub fn ballot_x_button() -> &'static str {
    "[\u{2717}]"
}

/// `"✗"` (U+2717 BALLOT X). Always 1 column.
/// Copied from grok-build/.../xai-grok-pager-render/src/glyphs.rs (`ballot_x`).
pub fn ballot_x() -> &'static str {
    "\u{2717}"
}

/// `"✓"` (U+2713 CHECK MARK). Always 1 column.
/// Copied from grok-build/.../xai-grok-pager-render/src/glyphs.rs (`check_mark`).
pub fn check_mark() -> &'static str {
    "\u{2713}"
}
