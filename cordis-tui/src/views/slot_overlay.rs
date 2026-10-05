//! 插件面板（`"tui.slots"`）的浮层：注册表里的 `slot`，`arg` 是插槽 id。
//!
//! 有视图树（`docs/PLUGIN-VIEWS.md`）就按视图画，否则画 `render()` 的文本；按键原样
//! 转给插件的 `on_key`（`esc` / `enter` / `up` / `down` / `char:x`），有视图时数字键
//! 1–9 换成第几个可见动作的 id。

use std::sync::Arc;

use cordis::{plugin, Context, Inject, Plugin};
use cordis_spine::{SlotKeyResult, TuiSlots, TUI_SLOTS};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::grok::picker::PickerHits;
use crate::views::registry::{
    register_overlay, OverlayInput, OverlayReply, OverlayView, ViewState, TUI_OVERLAYS,
};
use crate::views::text_overlay;

pub const KIND: &str = "slot";

struct SlotOverlay;

/// 画的时候和滚动时用同一个 key：有视图用视图的纯文本，否则用 `render()`。
fn body_key(slots: &TuiSlots, id: &str) -> Option<String> {
    slots
        .view(id)
        .map(|v| v.to_plain())
        .or_else(|| slots.render(id))
}

impl OverlayView for SlotOverlay {
    fn paint(&self, ctx: &Context, buf: &mut Buffer, area: Rect, state: &ViewState) -> PickerHits {
        let id = &state.arg;
        let slots = ctx.get::<TuiSlots>(TUI_SLOTS);
        let title = slots
            .as_ref()
            .and_then(|s| s.title(id))
            .unwrap_or_else(|| id.clone());
        if let Some(view) = slots.as_ref().and_then(|s| s.view(id)) {
            let key = view.to_plain();
            return text_overlay::render_styled(
                buf,
                area,
                &title,
                &key,
                state.scroll,
                &|theme, w| crate::views::plugin_view::lines(&view, theme, w as usize),
            );
        }
        let body = slots
            .as_ref()
            .and_then(|s| s.render(id))
            .unwrap_or_else(|| format!("slot \"{id}\" is not registered"));
        text_overlay::render(buf, area, &title, &body, state.scroll)
    }

    fn input(&self, ctx: &Context, state: &mut ViewState, input: OverlayInput) -> OverlayReply {
        let Some(slots) = ctx.get::<TuiSlots>(TUI_SLOTS) else {
            return OverlayReply::Close;
        };
        let id = state.arg.clone();
        let key = match input {
            OverlayInput::Esc => "esc".to_string(),
            OverlayInput::Enter => "enter".to_string(),
            OverlayInput::Scroll(delta) => {
                let key = if delta < 0 { "up" } else { "down" };
                let _ = slots.on_key(&id, key);
                if let Some(body) = body_key(&slots, &id) {
                    let max = text_overlay::max_scroll(&body, 16);
                    state.scroll =
                        (state.scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
                }
                return OverlayReply::Keep;
            }
            OverlayInput::Char(c) => {
                // 有视图时数字键是点第几个可见动作，插件收到的是动作 id。
                let action = c
                    .to_digit(10)
                    .and_then(|d| slots.view(&id).map(|v| (d, v)))
                    .and_then(|(d, v)| crate::views::plugin_view::action_for_digit(&v, d as usize));
                action.unwrap_or_else(|| format!("char:{c}"))
            }
        };
        match slots.on_key(&id, &key) {
            SlotKeyResult::Close => OverlayReply::Close,
            SlotKeyResult::Keep if input == OverlayInput::Esc => OverlayReply::Close,
            SlotKeyResult::Keep => OverlayReply::Keep,
        }
    }
}

/// 把插件面板浮层登记进 `"tui.overlays"`。
pub fn slot_overlay() -> Plugin {
    plugin(
        "tui.overlay.slot",
        Inject::from([TUI_OVERLAYS]),
        |ctx, _: &()| {
            register_overlay(ctx, KIND, Arc::new(SlotOverlay))?;
            Ok(None)
        },
    )
}
