//! Shift+Tab session cycle copied from Grok `dispatch_cycle_mode_inner`
//! with Auto gated off: Normal → Plan → Always-Approve → Normal.

use cordis_spine::PermissionMode;

/// Flash copy after a cycle step (Chinese UI).
pub fn cycle_session_mode(
    in_plan: bool,
    perm: PermissionMode,
) -> (bool, PermissionMode, &'static str) {
    match (in_plan, perm) {
        // Normal → Plan. Permission stays Ask.
        (false, PermissionMode::Ask) => (true, PermissionMode::Ask, "计划模式"),
        // Plan → Always-Approve (auto gated off).
        (true, PermissionMode::Ask) => (false, PermissionMode::Allow, "模式：始终允许"),
        // Always-Approve → Normal. Also collapses plan+allow.
        (_, PermissionMode::Allow) => (false, PermissionMode::Ask, "模式：询问"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_ring_without_auto() {
        let mut plan = false;
        let mut perm = PermissionMode::Ask;
        let (p, m, flash) = cycle_session_mode(plan, perm);
        assert!(p);
        assert_eq!(m, PermissionMode::Ask);
        assert_eq!(flash, "计划模式");
        plan = p;
        perm = m;

        let (p, m, flash) = cycle_session_mode(plan, perm);
        assert!(!p);
        assert_eq!(m, PermissionMode::Allow);
        assert_eq!(flash, "模式：始终允许");
        plan = p;
        perm = m;

        let (p, m, flash) = cycle_session_mode(plan, perm);
        assert!(!p);
        assert_eq!(m, PermissionMode::Ask);
        assert_eq!(flash, "模式：询问");
        plan = p;
        perm = m;

        let (p, m, _) = cycle_session_mode(plan, perm);
        assert!(p);
        assert_eq!(m, PermissionMode::Ask);
    }
}
