//! ACP permission option kinds copied from `agent-client-protocol`.
//!
//! The agent loop stays Cordis. This is the pager-facing
//! `session/request_permission` surface — not Grok's `MvpAgent`.

/// Copied from `agent_client_protocol::PermissionOptionKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

pub fn needs_permission(tool: &str) -> bool {
    matches!(
        tool,
        "bash"
            | "run_terminal_cmd"
            | "search_replace"
            | "write_file"
            | "scheduler_create"
            | "kill_task"
            | "monitor"
            | "cordis_run"
            | "cordis_promote"
    )
}

/// File/shell mutations blocked while `"settings"` plan mode is on.
pub fn blocked_in_plan(tool: &str) -> bool {
    matches!(
        tool,
        "bash"
            | "run_terminal_cmd"
            | "search_replace"
            | "write_file"
            | "scheduler_create"
            | "kill_task"
            | "monitor"
            | "cordis_run"
            | "cordis_promote"
    )
}

impl PermissionOptionKind {
    pub fn is_allow(self) -> bool {
        matches!(self, Self::AllowOnce | Self::AllowAlways)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_and_writes_need_permission() {
        assert!(needs_permission("bash"));
        assert!(needs_permission("write_file"));
        assert!(!needs_permission("read_file"));
        assert!(!needs_permission("list_dir"));
        assert!(needs_permission("cordis_run"));
        assert!(needs_permission("cordis_promote"));
        assert!(!needs_permission("cordis_inspect"));
        assert!(blocked_in_plan("cordis_run"));
        assert!(blocked_in_plan("cordis_promote"));
        assert!(!blocked_in_plan("cordis_inspect"));
    }

    #[test]
    fn allow_kinds() {
        assert!(PermissionOptionKind::AllowOnce.is_allow());
        assert!(PermissionOptionKind::AllowAlways.is_allow());
        assert!(!PermissionOptionKind::RejectOnce.is_allow());
        assert!(!PermissionOptionKind::RejectAlways.is_allow());
    }
}
