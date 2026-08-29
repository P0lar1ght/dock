//! `/usage` overlay — Grok session-usage block without grok.com billing.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::grok::picker::PickerHits;
use crate::text_overlay;
use cordis_spine::{session_usage_block_text, PromptUsage};

pub fn render(buf: &mut Buffer, area: Rect, usage: &PromptUsage, scroll: usize) -> PickerHits {
    text_overlay::render(buf, area, "用量", &session_usage_block_text(usage), scroll)
}

pub fn max_scroll(usage: &PromptUsage, viewport: usize) -> usize {
    text_overlay::max_scroll(&session_usage_block_text(usage), viewport)
}
