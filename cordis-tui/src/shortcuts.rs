//! Named `"tui.shortcuts"` service. The event loop live-looks this up each
//! frame — swap the plugin to change the bar without touching `tui`.

use cordis::{Context, Inject, Plugin, plugin};

use crate::ask_view;
use crate::grok::shortcuts::HintItem;
use crate::names::{SESSION_PORT, TUI_PROMPT, TUI_SHORTCUTS};
use crate::overlay::Overlay;
use crate::prompt::PromptWidget;
use crate::session::SessionRef;
use cordis_spine::{ASK, Ask};

pub struct Shortcuts {
    ctx: Context,
}

impl Shortcuts {
    pub fn new(ctx: Context) -> Self {
        Self { ctx }
    }

    /// Live-look prompt / `session.port`. Overlay is event-loop local state.
    pub fn hints(&self, overlay: &Overlay, slash: bool, files: bool) -> Vec<HintItem> {
        if let Overlay::Ask {
            selected, picked, ..
        } = overlay
        {
            let typing = self
                .ctx
                .get::<Ask>(ASK)
                .and_then(|a| a.front())
                .and_then(|p| {
                    p.questions.get(p.index).map(|q| {
                        let labs = ask_view::labels(q);
                        let multi = q.multi_select.unwrap_or(false);
                        ask_view::other_active(&labs, *selected, picked, multi)
                    })
                })
                .unwrap_or(false);
            if typing {
                return vec![
                    HintItem::new("type", "other"),
                    HintItem::new("←→", "cursor"),
                    HintItem::new("Enter", "submit"),
                    HintItem::new("Esc", "clear"),
                ];
            }
            let mut hints = vec![
                HintItem::new("Enter", "select"),
                HintItem::new("Esc", "reject"),
                HintItem::new("1–9", "option"),
            ];
            let multi_q = self
                .ctx
                .get::<Ask>(ASK)
                .and_then(|a| a.front())
                .is_some_and(|p| p.questions.len() > 1);
            if multi_q {
                hints.insert(1, HintItem::new("←→", "question"));
            }
            return hints;
        }
        if let Overlay::Elicit {
            selected, picked, ..
        } = overlay
        {
            let typing = self
                .ctx
                .get::<cordis_spine::Mcp>(cordis_spine::MCP)
                .and_then(|m| m.elicitation().front())
                .is_some_and(|p| crate::mcp_elicit_view::needs_draft(&p, *selected, picked));
            if typing {
                return vec![
                    HintItem::new("type", "other"),
                    HintItem::new("Enter", "submit"),
                    HintItem::new("Esc", "back"),
                ];
            }
            return vec![
                HintItem::new("Enter", "select"),
                HintItem::new("Esc", "cancel"),
                HintItem::new("1–9", "option"),
            ];
        }
        if matches!(overlay, Overlay::Permission { .. }) {
            return vec![
                HintItem::new("Enter", "select"),
                HintItem::new("Esc", "reject"),
                HintItem::new("1–9", "option"),
            ];
        }
        if matches!(
            overlay,
            Overlay::PlanApproval {
                view_only: true,
                ..
            }
        ) {
            return vec![
                HintItem::new("Enter", "close"),
                HintItem::new("Esc", "close"),
            ];
        }
        if matches!(overlay, Overlay::PlanApproval { .. }) {
            return vec![
                HintItem::new("j/k", "scroll"),
                HintItem::new("a", "approve"),
                HintItem::new("s", "revise"),
                HintItem::new("q", "quit"),
            ];
        }
        if matches!(overlay, Overlay::Goal { editing: true, .. }) {
            return vec![
                HintItem::new("Enter", "save"),
                HintItem::new("Esc", "cancel"),
            ];
        }
        if matches!(overlay, Overlay::Goal { .. }) {
            return vec![
                HintItem::new("Enter", "select"),
                HintItem::new("Esc", "close"),
                HintItem::new("e", "edit"),
                HintItem::new("p", "pause"),
                HintItem::new("g", "close"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Presets(crate::preset_overlay::PresetView::Canvas(s)) if s.editing_persona
        ) {
            return vec![
                HintItem::new("Enter", "save"),
                HintItem::new("Esc", "cancel"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Presets(crate::preset_overlay::PresetView::Canvas(s))
                if s.pane == crate::preset_overlay::PresetPane::Catalog
        ) {
            return vec![
                HintItem::new("Enter", "add"),
                HintItem::new("Tab", "pane"),
                HintItem::new("type", "filter"),
                HintItem::new("Esc", "back"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Presets(crate::preset_overlay::PresetView::Canvas(s))
                if s.pane == crate::preset_overlay::PresetPane::Assigned
        ) {
            return vec![
                HintItem::new("Enter", "remove"),
                HintItem::new("Tab", "pane"),
                HintItem::new("d", "remove"),
                HintItem::new("Esc", "back"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Presets(crate::preset_overlay::PresetView::Canvas(_))
        ) {
            return vec![
                HintItem::new("Enter", "edit"),
                HintItem::new("Tab", "pane"),
                HintItem::new("Esc", "back"),
            ];
        }
        if matches!(overlay, Overlay::Presets(_)) {
            return vec![
                HintItem::new("Enter", "open"),
                HintItem::new("n", "new"),
                HintItem::new("a", "apply"),
                HintItem::new("Esc", "close"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Inspect {
                target: crate::overlay::InspectTarget::Job(_),
                ..
            }
        ) {
            return vec![
                HintItem::new("↑/↓", "scroll"),
                HintItem::new("Esc", "back"),
                HintItem::new("q", "back"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Inspect {
                target: crate::overlay::InspectTarget::Subagent(_),
                ..
            }
        ) {
            return vec![
                HintItem::new("↑/↓", "scroll"),
                HintItem::new("Enter", "send"),
                HintItem::new("Esc", "back"),
                HintItem::new("q", "back"),
            ];
        }
        if let Overlay::Usage {
            detail: Some(_), ..
        } = overlay
        {
            return vec![HintItem::new("Esc", "back"), HintItem::new("↑/↓", "scroll")];
        }
        if matches!(overlay, Overlay::Usage { .. }) {
            return vec![
                HintItem::new("click", "detail"),
                HintItem::new("Tab", "tab"),
                HintItem::new("↑/↓", "scroll"),
                HintItem::new("Esc", "close"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Notice { .. } | Overlay::Slot { .. } | Overlay::Inspect { .. }
        ) {
            return vec![
                HintItem::new("↑/↓", "scroll"),
                HintItem::new("Esc", "close"),
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
                HintItem::new("Enter", "insert"),
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
        let queued = self
            .ctx
            .get::<SessionRef>(SESSION_PORT)
            .is_some_and(|s| !s.queued_prompts().is_empty());
        let mut hints = idle_hints(can_send, working, queued);
        if !can_send
            && !working
            && self
                .ctx
                .get::<cordis_spine::Goal>(cordis_spine::GOAL)
                .is_some_and(|g| g.present())
        {
            hints.insert(hints.len().saturating_sub(1), HintItem::new("g", "goal"));
        }
        hints
    }
}

/// Grok compact prompt bar: `Enter:send` · `Shift+Tab:mode` · `Ctrl+x:shortcuts`.
pub fn idle_hints(can_send: bool, working: bool, queued: bool) -> Vec<HintItem> {
    let mut hints = Vec::new();
    if can_send {
        hints.push(HintItem::new(
            "Enter",
            if working { "queue" } else { "send" },
        ));
        if working {
            hints.push(HintItem::new("Ctrl+Enter", "send now"));
        }
    } else if working && queued {
        hints.push(HintItem::new("Enter", "send now"));
        hints.push(HintItem::new("Esc", "edit"));
    } else if working {
        hints.push(HintItem::new("Esc", "cancel"));
    }
    hints.push(HintItem::new("Shift+Tab", "mode"));
    hints.push(HintItem::new("Ctrl+x", "shortcuts"));
    hints
}

pub fn shortcuts() -> Plugin {
    plugin(
        "tui.shortcuts",
        Inject::from([TUI_PROMPT, SESSION_PORT]),
        |ctx, _: &()| {
            Ok(Some(
                ctx.provide(TUI_SHORTCUTS, Shortcuts::new(ctx.clone()))?,
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_bar_advertises_shift_tab_mode() {
        let hints = idle_hints(true, false, false);
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
        let hints = idle_hints(true, true, false);
        let labels: Vec<&str> = hints.iter().map(|h| h.label.as_ref()).collect();
        assert!(labels.contains(&"queue"), "{labels:?}");
        assert!(labels.contains(&"send now"), "{labels:?}");
        assert!(hints.iter().any(|h| h.key == "Shift+Tab"));
    }

    #[test]
    fn running_empty_advertises_esc_cancel() {
        let hints = idle_hints(false, true, false);
        let pairs: Vec<(&str, &str)> = hints
            .iter()
            .map(|h| (h.key.as_ref(), h.label.as_ref()))
            .collect();
        assert!(pairs.contains(&("Esc", "cancel")), "{pairs:?}");
        assert!(!pairs.iter().any(|(_, l)| *l == "send now"), "{pairs:?}");
    }

    #[test]
    fn running_empty_with_queue_is_send_now_and_edit() {
        let hints = idle_hints(false, true, true);
        let pairs: Vec<(&str, &str)> = hints
            .iter()
            .map(|h| (h.key.as_ref(), h.label.as_ref()))
            .collect();
        assert!(pairs.contains(&("Enter", "send now")), "{pairs:?}");
        assert!(pairs.contains(&("Esc", "edit")), "{pairs:?}");
        assert!(!pairs.contains(&("Esc", "cancel")), "{pairs:?}");
    }
}
