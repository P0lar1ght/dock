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

/// `"[✗]"` close button. Always 3 columns.
pub fn ballot_x_button() -> &'static str {
    "[\u{2717}]"
}
