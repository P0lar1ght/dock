//! 只读场景里的 bash。
//!
//! 只读场景 = 计划模式开着，或子会话的能力档位是只读，或当前角色的预设标了 `read_only`
//! （explore、plan、旁问页）。这些场景都给 bash——没有 shell 的只读代理查东西太笨——
//! 但它不走普通的计划门 / 权限门，而是这里的规矩：
//!
//! - 只读命令（[`cordis_base::read_only_shell::is_read_only_command`]）直接跑，不问；
//! - 别的一律问用户（[`crate::Permissions::request_strict`]），自动批准也不算数。

use cordis::Context;

use crate::agent::capability::CapabilityMode;
use crate::agent::presets::AgentPresets;
use crate::names::{AGENT_PRESETS, CAPABILITY, PLAN_MODE};
use crate::tools::plan_mode::PlanMode;

/// 这个会话此刻是不是只读场景。
pub fn in_read_only_scene(exec: &Context) -> bool {
    exec.get::<PlanMode>(PLAN_MODE).is_some_and(|p| p.gated())
        || exec
            .get::<CapabilityMode>(CAPABILITY)
            .is_some_and(|m| *m == CapabilityMode::ReadOnly)
        || exec
            .get::<AgentPresets>(AGENT_PRESETS)
            .is_some_and(|p| p.current().read_only)
}

/// bash 调用参数里的命令文本；解不出来当空（空命令不算只读，会去问用户）。
pub fn bash_command(arguments: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| {
            v.get("command")
                .and_then(|c| c.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// 用户没批准时回给模型的话：说清楚为什么，也告诉它哪些能直接跑。
pub const DENIED: &str = "只读模式：这条命令可能改动文件或环境，用户没有批准。\
只读命令（ls、cat、grep/rg、find、git status/log/diff/show 等，不带重定向）可以直接跑。";
