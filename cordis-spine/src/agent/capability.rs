//! 子会话的能力档位：按工具**种类**收窄一个子代理能碰什么。
//!
//! 这不是预设工具集的替代品，而是压在它上面的第二道闸：预设说「这个角色平时
//! 能用哪些」，能力档位说「这一次委派允许到哪一层」。两道**取交集**，而且能力
//! 档位管得到 MCP 与动态插件工具——那两类按设计绕过预设允许名单
//! （`Tools::bypasses_allowlist`），却照样能写盘、能执行。
//!
//! 档位语义与 grok 的 `CapabilityMode` 对齐：`ReadOnly < ReadWrite < All`、
//! `ReadOnly < Execute < All`，**`ReadWrite` 与 `Execute` 互不包含**（一个能改
//! 文件不能跑命令，一个能跑命令不能改文件）。
//!
//! 分类按**工具名**做，并且**默认关闭**：认不出来的名字只有 `All` 放行。新工具
//! 忘了归类时，代价是它在受限子代理里不可用，而不是悄悄带着写盘能力溜进只读
//! 会话。

/// 一次委派允许到哪一层。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CapabilityMode {
    /// 只读与检索。不能改文件、不能跑命令、不能起后台任务。
    ReadOnly,
    /// 读 + 改文件。不能跑命令。
    ReadWrite,
    /// 读 + 跑命令 + 管后台任务。不能改文件。
    Execute,
    /// 不设限。主会话恒为这一档。
    #[default]
    All,
}

impl CapabilityMode {
    /// 解析工作流脚本 / 工具参数里写的档位名。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "read-only" | "readonly" => Some(Self::ReadOnly),
            "read-write" | "readwrite" => Some(Self::ReadWrite),
            "execute" => Some(Self::Execute),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
            Self::Execute => "execute",
            Self::All => "all",
        }
    }

    /// 这一档放不放行这颗工具。
    pub fn allows(self, tool: &str) -> bool {
        if matches!(self, Self::All) {
            return true;
        }
        // 只读档也给 bash：没有 shell 的只读代理查东西太笨。给了不等于放开——执行时
        // 按只读场景把关（`tools::read_only`）：只读命令直接跑，别的一律问用户。
        if tool == "bash" && matches!(self, Self::ReadOnly) {
            return true;
        }
        match class_of(tool) {
            // 元工具：问用户、上报、计划、技能发现，任何档位都得留着，
            // 否则受限子代理连"我做不到"都说不出口。
            Class::Meta => true,
            Class::Read | Class::Search | Class::Inspect => {
                matches!(self, Self::ReadOnly | Self::ReadWrite | Self::Execute)
            }
            Class::Edit => matches!(self, Self::ReadWrite),
            // bash、桌面操作、后台任务与再委派同属"能让事情发生"。
            Class::Execute => matches!(self, Self::Execute),
            // `use_tool` 只是派发口，本身不决定能力；但它能派发到按需工具，
            // 所以只读档不给。
            Class::Dispatch => matches!(self, Self::ReadWrite | Self::Execute),
            // 认不出来的一律按最危险处理。
            Class::Unknown => false,
        }
    }
}

/// 同一次回复里能和别的调用一起跑的工具调用：只读、只查（读文件、搜索、看目录、查网页、
/// 读记忆、LSP、读技能、搜按需工具），以及只读的 bash 命令（`git status`、`rg`、`ls`…，
/// 判断同只读场景，见 [`cordis_base::read_only_shell::is_read_only_command`]；每次调用
/// 都是新起的 shell，互不影响）。参考 Codex 的 `supports_parallel_tool_calls`（它的 shell
/// 命令也能并行）。
/// 改动、别的命令、问用户、计划、子代理、浏览器都一颗一颗跑：前后有依赖（先改再读、同一个
/// 标签页的先点再看），或者要人参与。认不出来的（MCP、动态包）按不能并行处理。
pub fn parallel_safe(tool: &str, arguments: &str) -> bool {
    if tool.starts_with(crate::tools::browser::BROWSER_MCP_PREFIX) || tool.starts_with("browser_") {
        return false;
    }
    if tool == "bash" {
        let background = serde_json::from_str::<serde_json::Value>(arguments)
            .ok()
            .and_then(|v| v.get("is_background").and_then(|b| b.as_bool()))
            .unwrap_or(false);
        let command = crate::tools::read_only::bash_command(arguments);
        return !background && cordis_base::read_only_shell::is_read_only_command(&command);
    }
    matches!(tool, "skill" | "search_tool")
        || matches!(class_of(tool), Class::Read | Class::Search | Class::Inspect)
}

/// cua-driver 里只读的几颗：列应用 / 窗口、读窗口状态 / 无障碍树、校验状态、屏幕尺寸、
/// 光标位置、读指南（名字见 <https://cua.ai/docs/cua-driver/reference/mcp-tools>）。点、输、
/// 按键、拖拽这些动作照旧一颗颗跑——桌面动作有先后，参考 Anthropic 对 computer use 的要求
/// （按顺序、遇错即停）。它们仍然过审批（`acp::is_cua_driver_mcp`），并行只是不用互相等。
const CUA_READ_ONLY: &[&str] = &[
    "list_apps",
    "list_windows",
    "get_window_state",
    "get_accessibility_tree",
    "verify_state",
    "get_screen_size",
    "get_cursor_position",
    "get_guide",
];

/// [`parallel_safe`]，再加上要看工具表才知道的两种：
/// - `use_tool`：按它派发的**内层**工具判断（内层再是 `use_tool` 的不算）。
/// - MCP 工具：cua-driver 的只读几颗（[`CUA_READ_ONLY`]），或服务器在 `tools/list` 里标了
///   `readOnlyHint: true`（`read_only_hint`）。参考 Codex：标了只读的 MCP 工具可以并行，
///   没标的照旧串行。MCP 规范说注解只是提示、不可信，所以这里**只决定排不排在一起跑**，
///   审批、计划模式、能力档位一样不少。内置浏览器的工具共用一个标签页，一律串行。
pub fn parallel_safe_with(
    tool: &str,
    arguments: &str,
    read_only_hint: &dyn Fn(&str) -> bool,
) -> bool {
    if tool == crate::tools::mcp::USE_TOOL_NAME {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) else {
            return false;
        };
        let Some(inner) = v.get("tool_name").and_then(|n| n.as_str()) else {
            return false;
        };
        if inner == crate::tools::mcp::USE_TOOL_NAME {
            return false;
        }
        let input = v
            .get("tool_input")
            .map(|i| i.to_string())
            .unwrap_or_else(|| "{}".into());
        return parallel_safe_with(inner, &input, read_only_hint);
    }
    if let Some(raw) = tool.strip_prefix(cordis_base::acp::CUA_DRIVER_MCP_PREFIX) {
        return CUA_READ_ONLY.contains(&raw);
    }
    if parallel_safe(tool, arguments) {
        return true;
    }
    tool.starts_with("mcp_")
        && !tool.starts_with(crate::tools::browser::BROWSER_MCP_PREFIX)
        && read_only_hint(tool)
}

enum Class {
    Meta,
    Read,
    Search,
    Inspect,
    Edit,
    Execute,
    Dispatch,
    Unknown,
}

/// 工具名 → 能力种类。**新工具默认 `Unknown`（只有 `All` 放行）**。
fn class_of(tool: &str) -> Class {
    // 内置浏览器 MCP 的工具按本名归类（`mcp_browser__browser_click` → `browser_click`）；
    // 别的 MCP 仍不归类。
    let tool = match tool.strip_prefix(crate::tools::browser::BROWSER_MCP_PREFIX) {
        Some(raw) if raw.starts_with("browser_") => raw,
        _ => tool,
    };
    match tool {
        "enter_plan_mode" | "exit_plan_mode" | "ask_user_question" | "todo_write" | "skill"
        | "search_tool" | "update_goal" => Class::Meta,

        // 子代理回报父级走的就是它，只读子代理也得能用。放开是安全的：受限
        // 子代理派不出孙代理，服务层的相邻授权又只认父子这一条边，兄弟之间
        // 发不了、打断不了。
        "send_message" => Class::Meta,

        // 请用户在浏览器面板里接手：就是一道提问，和 ask_user_question 同类。
        "handoff_to_user" => Class::Meta,

        "read_file" | "memory_get" | "memory_search" | "canvas_read" => Class::Read,

        "grep" | "glob" | "web_search" | "web_fetch" => Class::Search,

        "list_dir" | "lsp" | "list_agents" => Class::Inspect,

        "search_replace" | "write_file" | "memory_write" | "canvas_create" | "canvas_edit"
        | "canvas_data" => Class::Edit,

        "bash" | "monitor" | "task" | "interrupt_agent" | "job" | "kill_task" | "workflow"
        | "scheduler_create" | "scheduler_delete" | "scheduler_list" => Class::Execute,

        "use_tool" => Class::Dispatch,

        // 浏览器分两半：看页面的算检索，动页面 / 跑 JS 的算执行。
        "browser_open"
        | "browser_navigate"
        | "browser_navigate_back"
        | "browser_snapshot"
        | "browser_screenshot"
        | "browser_tabs"
        | "browser_console_messages"
        | "browser_network_requests"
        | "browser_wait_for"
        | "browser_close" => Class::Search,
        name if name.starts_with("browser_") => Class::Execute,

        // 本机桌面操作与动态 Cordis 包：不归类，只有 `All` 放行。
        // MCP（`mcp_*__*`）同理——服务端能做什么这边看不出来。
        _ => Class::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_reading_tools_run_in_parallel() {
        for yes in [
            "read_file",
            "grep",
            "glob",
            "list_dir",
            "web_search",
            "web_fetch",
            "lsp",
            "memory_search",
            "memory_get",
            "skill",
            "search_tool",
        ] {
            assert!(parallel_safe(yes, "{}"), "{yes} 该能并行");
        }
        for no in [
            "bash",
            "search_replace",
            "write_file",
            "use_tool",
            "task",
            "ask_user_question",
            "todo_write",
            "enter_plan_mode",
            "send_message",
            "mcp_browser__browser_snapshot",
            "mcp_other__read",
            "cordis_run",
        ] {
            assert!(!parallel_safe(no, "{}"), "{no} 不该并行");
        }
        // bash 看命令：只读的能并行，改东西的、放后台的不行。
        assert!(parallel_safe("bash", r#"{"command":"git status"}"#));
        assert!(parallel_safe("bash", r#"{"command":"rg -n TODO src"}"#));
        assert!(!parallel_safe("bash", r#"{"command":"cargo test"}"#));
        assert!(!parallel_safe("bash", r#"{"command":"ls > out.txt"}"#));
        assert!(!parallel_safe(
            "bash",
            r#"{"command":"git status","is_background":true}"#
        ));
    }

    #[test]
    fn use_tool_and_mcp_parallel_by_inner_tool_and_read_only_hint() {
        let hinted = |name: &str| {
            name == "mcp_linear__list_issues" || name == "mcp_browser__browser_snapshot"
        };
        let safe = |tool: &str, args: &str| parallel_safe_with(tool, args, &hinted);
        // cua-driver：只读的几颗能并行（直接调或经 use_tool），动作一律串行。
        for yes in [
            "get_window_state",
            "list_windows",
            "list_apps",
            "verify_state",
            "get_guide",
        ] {
            let name = format!("mcp_cua-driver__{yes}");
            assert!(safe(&name, "{}"), "{name}");
            let via = serde_json::json!({"tool_name": name, "tool_input": {"pid": 1}}).to_string();
            assert!(safe("use_tool", &via), "{via}");
        }
        for no in [
            "click",
            "type_text",
            "press_key",
            "run_steps",
            "run_script",
            "launch_app",
        ] {
            let name = format!("mcp_cua-driver__{no}");
            assert!(!safe(&name, "{}"), "{name}");
            let via = serde_json::json!({"tool_name": name}).to_string();
            assert!(!safe("use_tool", &via), "{via}");
        }
        // 别的 MCP：标了 readOnlyHint 才并行；没标的串行；内置浏览器即使标了也串行。
        assert!(safe("mcp_linear__list_issues", "{}"));
        assert!(safe(
            "use_tool",
            r#"{"tool_name":"mcp_linear__list_issues","tool_input":{}}"#
        ));
        assert!(!safe("mcp_linear__save_issue", "{}"));
        assert!(!safe(
            "use_tool",
            r#"{"tool_name":"mcp_linear__save_issue"}"#
        ));
        assert!(!safe("mcp_browser__browser_snapshot", "{}"));
        // use_tool 派发本地工具：按内层判断（bash 照样看命令）。
        assert!(safe(
            "use_tool",
            r#"{"tool_name":"bash","tool_input":{"command":"git status"}}"#
        ));
        assert!(!safe(
            "use_tool",
            r#"{"tool_name":"bash","tool_input":{"command":"rm -rf x"}}"#
        ));
        // 认不出来的、嵌套 use_tool、参数坏的：串行。
        assert!(!safe("use_tool", "{}"));
        assert!(!safe("use_tool", "not json"));
        assert!(!safe(
            "use_tool",
            r#"{"tool_name":"use_tool","tool_input":{"tool_name":"read_file"}}"#
        ));
        assert!(!safe("use_tool", r#"{"tool_name":"some_plugin_tool"}"#));
        // 没有工具表信息时，原来的判断不变。
        assert!(safe("read_file", "{}"));
        assert!(!safe("write_file", "{}"));
    }

    #[test]
    fn parses_the_names_scripts_write() {
        assert_eq!(
            CapabilityMode::parse("read-only"),
            Some(CapabilityMode::ReadOnly)
        );
        assert_eq!(
            CapabilityMode::parse("read_only"),
            Some(CapabilityMode::ReadOnly)
        );
        assert_eq!(
            CapabilityMode::parse(" Execute "),
            Some(CapabilityMode::Execute)
        );
        assert_eq!(CapabilityMode::parse("all"), Some(CapabilityMode::All));
        assert_eq!(CapabilityMode::parse("readonlyish"), None);
    }

    #[test]
    fn read_only_keeps_reading_and_drops_everything_that_acts() {
        let m = CapabilityMode::ReadOnly;
        for allowed in [
            "read_file",
            "grep",
            "glob",
            "list_dir",
            "web_search",
            "web_fetch",
            "lsp",
            "memory_search",
            "ask_user_question",
            "send_message",
            "skill",
            // bash 放进工具表，执行时再按只读命令把关（`tools::read_only` 的测试管那一半）。
            "bash",
        ] {
            assert!(m.allows(allowed), "只读档不该挡 {allowed}");
        }
        for denied in [
            "write_file",
            "search_replace",
            "task",
            "interrupt_agent",
            "monitor",
            "kill_task",
            "mcp_browser__browser_evaluate",
            "mcp_browser__browser_click",
            "mcp_other__browser_snapshot",
            "use_tool",
        ] {
            assert!(!m.allows(denied), "只读档必须挡 {denied}");
        }
        // 内置浏览器 MCP 看页面的那一半仍算检索。
        assert!(m.allows("mcp_browser__browser_snapshot"));
    }

    /// `workflow` 归执行类：只读子代理起不了新的工作流 run。
    #[test]
    fn read_only_cannot_launch_another_workflow() {
        assert!(!CapabilityMode::ReadOnly.allows("workflow"));
        assert!(!CapabilityMode::ReadWrite.allows("workflow"));
        assert!(CapabilityMode::Execute.allows("workflow"));
        assert!(CapabilityMode::All.allows("workflow"));
    }

    /// 读写与执行互不包含：一个能改文件不能跑命令，一个反过来。
    #[test]
    fn read_write_and_execute_are_incomparable() {
        assert!(CapabilityMode::ReadWrite.allows("write_file"));
        assert!(!CapabilityMode::ReadWrite.allows("bash"));
        assert!(CapabilityMode::Execute.allows("bash"));
        assert!(!CapabilityMode::Execute.allows("write_file"));
    }

    /// 认不出来的名字默认关闭——新工具忘了归类时是不可用，不是悄悄放行。
    #[test]
    fn unknown_tools_are_denied_outside_all() {
        for mode in [
            CapabilityMode::ReadOnly,
            CapabilityMode::ReadWrite,
            CapabilityMode::Execute,
        ] {
            assert!(!mode.allows("mcp_acme__deploy"));
            assert!(!mode.allows("cordis_install"));
            assert!(!mode.allows("computer_click"));
            assert!(!mode.allows("a_tool_added_next_week"));
        }
        assert!(CapabilityMode::All.allows("mcp_acme__deploy"));
    }
}
