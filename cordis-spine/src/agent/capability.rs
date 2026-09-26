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
    match tool {
        "enter_plan_mode" | "exit_plan_mode" | "ask_user_question" | "todo_write" | "skill"
        | "search_tool" | "update_goal" => Class::Meta,

        // 子代理回报父级走的就是它，只读子代理也得能用。放开是安全的：受限
        // 子代理派不出孙代理，服务层的相邻授权又只认父子这一条边，兄弟之间
        // 发不了、打断不了。
        "send_message" => Class::Meta,

        "read_file" | "memory_get" | "memory_search" => Class::Read,

        "grep" | "glob" | "web_search" | "web_fetch" => Class::Search,

        "list_dir" | "lsp" | "list_agents" => Class::Inspect,

        "search_replace" | "write_file" | "memory_write" => Class::Edit,

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
            "browser_evaluate",
            "browser_click",
            "use_tool",
        ] {
            assert!(!m.allows(denied), "只读档必须挡 {denied}");
        }
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
