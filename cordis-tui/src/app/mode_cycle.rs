//! Shift+Tab session cycle copied from Grok `dispatch_cycle_mode_inner`
//! with Auto gated off, plus Dock's 智能授权:
//! 询问 → 智能授权 → 计划 → 始终允许 → 询问。

use cordis_spine::PermissionMode;

/// Flash copy after a cycle step (Chinese UI).
pub fn cycle_session_mode(
    in_plan: bool,
    perm: PermissionMode,
) -> (bool, PermissionMode, &'static str) {
    match (in_plan, perm) {
        // 询问 → 智能授权。
        (false, PermissionMode::Ask) => (false, PermissionMode::Smart, "模式：智能授权"),
        // 智能授权 → 计划。计划里权限回到询问。
        (false, PermissionMode::Smart) => (true, PermissionMode::Ask, "计划模式"),
        // 计划 → 始终允许（auto gated off）。
        (true, PermissionMode::Ask | PermissionMode::Smart) => {
            (false, PermissionMode::Allow, "模式：始终允许")
        }
        // 始终允许 → 询问。Also collapses plan+allow.
        (_, PermissionMode::Allow) => (false, PermissionMode::Ask, "模式：询问"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_goes_ask_smart_plan_allow() {
        let mut state = (false, PermissionMode::Ask);
        for (plan, perm, flash) in [
            (false, PermissionMode::Smart, "模式：智能授权"),
            (true, PermissionMode::Ask, "计划模式"),
            (false, PermissionMode::Allow, "模式：始终允许"),
            (false, PermissionMode::Ask, "模式：询问"),
            (false, PermissionMode::Smart, "模式：智能授权"),
        ] {
            let (p, m, f) = cycle_session_mode(state.0, state.1);
            assert_eq!((p, m, f), (plan, perm, flash));
            state = (p, m);
        }
    }
}
