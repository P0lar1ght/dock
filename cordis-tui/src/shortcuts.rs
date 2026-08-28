//! Named `"tui.shortcuts"` service. The event loop live-looks this up each
//! frame — swap the plugin to change the bar without touching `tui`.

use cordis::{plugin, Context, Inject, Plugin};

use crate::grok::shortcuts::HintItem;
use crate::names::{SESSION_PORT, TUI_PROMPT, TUI_SHORTCUTS};
use crate::overlay::Overlay;
use crate::prompt::PromptWidget;
use crate::session::SessionRef;

pub struct Shortcuts {
    ctx: Context,
}

impl Shortcuts {
    pub fn new(ctx: Context) -> Self {
        Self { ctx }
    }

    /// Live-look prompt / `session.port`. Overlay is event-loop local state.
    pub fn hints(&self, overlay: &Overlay, slash: bool, files: bool) -> Vec<HintItem> {
        if matches!(overlay, Overlay::Permission { .. } | Overlay::Ask { .. }) {
            return vec![
                HintItem::new("Enter", "select"),
                HintItem::new("Esc", "reject"),
                HintItem::new("1–9", "option"),
            ];
        }
        if overlay.is_open() {
            return vec![
                HintItem::new("Enter", "select"),
                HintItem::new("Esc", "close"),
                HintItem::new("type", "filter"),
            ];
        }
        if slash {
            return vec![
                HintItem::new("Enter", "run"),
                HintItem::new("Tab", "insert"),
                HintItem::new("↑/↓", "nav"),
                HintItem::new("Esc", "close"),
            ];
        }
        if files {
            return vec![
                HintItem::new("Enter", "insert"),
                HintItem::new("Tab", "insert"),
                HintItem::new("↑/↓", "nav"),
                HintItem::new("Esc", "cancel"),
            ];
        }
        let can_send = self
            .ctx
            .get::<PromptWidget>(TUI_PROMPT)
            .is_some_and(|p| p.can_send());
        let working = self
            .ctx
            .get::<SessionRef>(SESSION_PORT)
            .is_some_and(|s| s.working());
        idle_hints(can_send, working)
    }
}

/// Grok compact prompt bar: `Enter:send` · `Shift+Tab:mode` · `Ctrl+x:shortcuts`.
pub fn idle_hints(can_send: bool, working: bool) -> Vec<HintItem> {
    let mut hints = Vec::new();
    if can_send {
        hints.push(HintItem::new(
            "Enter",
            if working { "queue" } else { "send" },
        ));
        if working {
            hints.push(HintItem::new("Ctrl+Enter", "send now"));
        }
    } else if working {
        hints.push(HintItem::new("Enter", "send now"));
    }
    hints.push(HintItem::new("Shift+Tab", "mode"));
    hints.push(HintItem::new("Ctrl+x", "shortcuts"));
    hints
}

pub fn shortcuts() -> Plugin {
    plugin(
        "tui.shortcuts",
        Inject::from([TUI_PROMPT, SESSION_PORT]),
        |ctx, _: &()| Ok(Some(ctx.provide(TUI_SHORTCUTS, Shortcuts::new(ctx.clone()))?)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_bar_advertises_shift_tab_mode() {
        let hints = idle_hints(true, false);
        let pairs: Vec<(&str, &str)> = hints
            .iter()
            .map(|h| (h.key.as_ref(), h.label.as_ref()))
            .collect();
        assert!(pairs.contains(&("Enter", "send")), "{pairs:?}");
        assert!(pairs.contains(&("Shift+Tab", "mode")), "{pairs:?}");
        assert!(pairs.contains(&("Ctrl+x", "shortcuts")), "{pairs:?}");
    }

    #[test]
    fn running_with_text_queues() {
        let hints = idle_hints(true, true);
        let labels: Vec<&str> = hints.iter().map(|h| h.label.as_ref()).collect();
        assert!(labels.contains(&"queue"), "{labels:?}");
        assert!(labels.contains(&"send now"), "{labels:?}");
        assert!(hints.iter().any(|h| h.key == "Shift+Tab"));
    }
}
