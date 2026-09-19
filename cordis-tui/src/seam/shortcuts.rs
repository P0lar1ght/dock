//! Named `"tui.shortcuts"` service. The event loop live-looks this up each
//! frame — swap the plugin to change the bar without touching `tui`.

use cordis::{plugin, Context, Inject, Plugin};

use crate::grok::shortcuts::HintItem;
use crate::names::{SESSION_PORT, TUI_PROMPT, TUI_SHORTCUTS};
use crate::seam::session::SessionRef;
use crate::views::ask_view;
use crate::views::overlay::Overlay;
use crate::views::prompt::PromptWidget;
use cordis_spine::{Ask, Computer, Sessions, ASK, COMPUTER, SESSIONS};

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
            selected,
            picked,
            draft,
            draft_focused,
            ..
        } = overlay
        {
            let other_on = self
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
            let typing = *draft_focused && other_on;
            if typing {
                // Esc 是一级级退：有字先清空，空着再退出「其他」。
                return vec![
                    HintItem::new("type", "other"),
                    HintItem::new("←→", "cursor"),
                    HintItem::new("Enter", "submit"),
                    HintItem::new("Esc", if draft.is_empty() { "back" } else { "clear" }),
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
                .is_some_and(|p| crate::views::mcp_elicit_view::needs_draft(&p, *selected, picked));
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
        if matches!(overlay, Overlay::PairingPending { .. }) {
            return vec![
                HintItem::new("Enter", "select"),
                HintItem::new("Esc", "deny"),
                HintItem::new("1–9", "option"),
            ];
        }
        if matches!(overlay, Overlay::PairingManage { .. }) {
            return vec![
                HintItem::new("Enter", "confirm"),
                HintItem::new("x", "deny/revoke"),
                HintItem::new("Esc", "close"),
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
            Overlay::Presets(crate::views::preset_overlay::PresetView::Canvas(s))
                if s.editing_persona || s.naming_role.is_some()
        ) {
            return vec![
                HintItem::new("Enter", "确认"),
                HintItem::new("Esc", "cancel"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Presets(crate::views::preset_overlay::PresetView::Canvas(s))
                if s.pane == crate::views::preset_overlay::PresetPane::Catalog
        ) {
            return vec![
                HintItem::new("Enter/双击", "add"),
                HintItem::new("单击", "select"),
                HintItem::new("Tab", "pane"),
                HintItem::new("type", "filter"),
                HintItem::new("r", "resync"),
                HintItem::new("Esc", "back"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Presets(crate::views::preset_overlay::PresetView::Canvas(s))
                if s.pane == crate::views::preset_overlay::PresetPane::Assigned
        ) {
            return vec![
                HintItem::new("Enter/双击", "remove"),
                HintItem::new("单击", "select"),
                HintItem::new("Tab", "pane"),
                HintItem::new("x", "remove"),
                HintItem::new("r", "resync"),
                HintItem::new("Esc", "back"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Presets(crate::views::preset_overlay::PresetView::Canvas(s))
                if s.pane == crate::views::preset_overlay::PresetPane::Roles
                    && s.editing_role.is_none()
        ) {
            return vec![
                HintItem::new("Enter/双击", "edit role"),
                HintItem::new("n", "命名新建"),
                HintItem::new("m", "改名"),
                HintItem::new(";/Tab", "确认命名"),
                HintItem::new("x", "delete role"),
                HintItem::new("Tab", "pane"),
                HintItem::new("r", "resync"),
                HintItem::new("Esc", "back"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Presets(crate::views::preset_overlay::PresetView::Canvas(_))
        ) {
            return vec![
                HintItem::new("Enter", "edit"),
                HintItem::new("Tab", "pane"),
                HintItem::new("r", "resync"),
                HintItem::new("Esc", "back"),
            ];
        }
        if matches!(overlay, Overlay::Presets(_)) {
            return vec![
                HintItem::new("Enter", "open"),
                HintItem::new("n", "new"),
                HintItem::new("d", "duplicate"),
                HintItem::new("x", "delete mode"),
                HintItem::new("a", "apply"),
                HintItem::new("r", "resync"),
                HintItem::new("Esc", "close"),
            ];
        }
        if matches!(
            overlay,
            Overlay::Inspect {
                target: crate::views::overlay::InspectTarget::Job(_),
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
                target: crate::views::overlay::InspectTarget::Subagent(_),
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
        // 驾驶舱的键随状态变：没装只有 install，装好了才谈 grant，确认态只剩
        // Enter / Esc。全部按 `"computer"` 报的状态来，这里不自己判断本机。
        if let Overlay::Computer { pending, .. } = overlay {
            if pending.is_some() {
                return vec![
                    HintItem::new("Enter", "confirm"),
                    HintItem::new("Esc", "cancel"),
                ];
            }
            let computer = self.ctx.get::<Computer>(COMPUTER);
            let mut hints = vec![HintItem::new("↑/↓", "scroll")];
            if let Some(computer) = &computer {
                if !computer.busy() {
                    hints.push(HintItem::new("i", computer.install_label()));
                    if computer.grant_available() {
                        hints.push(HintItem::new("p", "grant"));
                    }
                }
            }
            hints.push(HintItem::new("Ctrl+R", "recheck"));
            hints.push(HintItem::new("Esc", "close"));
            return hints;
        }
        if matches!(
            overlay,
            Overlay::Notice { .. }
                | Overlay::Slot { .. }
                | Overlay::Browser { .. }
                | Overlay::Inspect { .. }
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
        // 条件要和 `rewind_idle_send` 的闸一致（含 `queued`），否则底栏写着
        // `Esc undo` 按下去却什么都不发生。`has_undoable_send` 不克隆日志——
        // 这行每帧都会算。
        if !can_send
            && !working
            && !queued
            && self
                .ctx
                .get::<Sessions>(SESSIONS)
                .is_some_and(|s| s.has_undoable_send())
        {
            hints.insert(0, HintItem::new("Esc", "undo"));
        }
        if !can_send
            && !working
            && self
                .ctx
                .get::<cordis_spine::Goal>(cordis_spine::GOAL)
                .is_some_and(|g| g.present())
        {
            hints.insert(hints.len().saturating_sub(1), HintItem::new("g", "goal"));
        }
        if self
            .ctx
            .get::<cordis_spine::Todos>(cordis_spine::TODOS)
            .is_some_and(|t| t.stats().total() > 0)
        {
            hints.insert(
                hints.len().saturating_sub(1),
                HintItem::new("Ctrl+t", "todo"),
            );
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
