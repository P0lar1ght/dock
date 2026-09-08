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

/// MCP public-name prefix for trycua `cua-driver` (config key `cua-driver`).
/// Example: `mcp_cua-driver__click`.
pub const CUA_DRIVER_MCP_PREFIX: &str = "mcp_cua-driver__";

/// Built-in tools that always hit the ask overlay / plan gate.
fn gated_builtin(tool: &str) -> bool {
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
            | "browser_evaluate"
    )
}

/// Computer-use via cua-driver MCP: same ask/plan gate as bash (C0).
/// All `mcp_cua-driver__*` tools are gated — desktop control is sensitive even
/// for read/capture helpers. Dock BUA `browser_*` stay separate.
pub fn is_cua_driver_mcp(tool: &str) -> bool {
    tool.starts_with(CUA_DRIVER_MCP_PREFIX)
}

pub fn needs_permission(tool: &str) -> bool {
    gated_builtin(tool) || is_cua_driver_mcp(tool)
}

/// File/shell/computer mutations blocked while `"settings"` plan mode is on.
pub fn blocked_in_plan(tool: &str) -> bool {
    gated_builtin(tool) || is_cua_driver_mcp(tool)
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
        assert!(needs_permission("browser_evaluate"));
        assert!(!needs_permission("browser_snapshot"));
        assert!(!needs_permission("browser_network_requests"));
        assert!(blocked_in_plan("cordis_run"));
        assert!(blocked_in_plan("cordis_promote"));
        assert!(!blocked_in_plan("cordis_inspect"));
        assert!(blocked_in_plan("browser_evaluate"));
        assert!(!blocked_in_plan("browser_console_messages"));
    }

    #[test]
    fn cua_driver_mcp_gated_like_bash() {
        assert!(is_cua_driver_mcp("mcp_cua-driver__click"));
        assert!(is_cua_driver_mcp("mcp_cua-driver__type_text"));
        assert!(is_cua_driver_mcp("mcp_cua-driver__get_desktop_state"));
        assert!(!is_cua_driver_mcp("mcp_other__click"));
        assert!(!is_cua_driver_mcp("browser_click"));
        assert!(needs_permission("mcp_cua-driver__click"));
        assert!(needs_permission("mcp_cua-driver__list_windows"));
        assert!(blocked_in_plan("mcp_cua-driver__press_key"));
        assert!(!needs_permission("mcp_linear__save_issue"));
    }

    #[test]
    fn allow_kinds() {
        assert!(PermissionOptionKind::AllowOnce.is_allow());
        assert!(PermissionOptionKind::AllowAlways.is_allow());
        assert!(!PermissionOptionKind::RejectOnce.is_allow());
        assert!(!PermissionOptionKind::RejectAlways.is_allow());
    }
}
