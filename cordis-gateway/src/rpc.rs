//! JSON-RPC method dispatch：协议骨架写在这里，其余落到 `"gateway.methods"`（功能插件
//! 登记的方法）；两边都没有回 `method_not_found`。

use serde_json::Value;

use crate::handle::GatewayHandle;
use crate::handlers::{
    connection, context, environment, image_inputs, interaction, permission, slash, subagent,
    thread, turn,
};
use crate::methods;
use crate::protocol::{self, RpcError};

pub async fn dispatch(
    gateway: GatewayHandle,
    method: &str,
    params: Value,
    subscribed: &mut thread::Subscriptions,
) -> Result<Value, RpcError> {
    if opens_on_demand(method) || methods::policy_of(&gateway, method).opens_thread {
        thread::open_on_demand(&gateway, &params).await?;
    }
    match method {
        protocol::WORKSPACE_LIST => connection::workspace_list(&gateway, params),
        protocol::MCP_RELOAD => connection::mcp_reload(&gateway).await,
        protocol::MCP_LIST => connection::mcp_list(&gateway),
        protocol::MCP_RECONNECT => connection::mcp_reconnect(&gateway, params).await,
        protocol::MODEL_LIST => connection::model_list(&gateway),
        protocol::THREAD_LIST => thread::list(&gateway, params),
        protocol::THREAD_SEARCH => thread::search(params),
        protocol::THREAD_REWIND => dispatch_detached(gateway, method, params).await,
        protocol::THREAD_START if params.get("cwd").is_some() => {
            thread::start_at(&gateway, params).await
        }
        protocol::THREAD_START => thread::start(&gateway, params),
        protocol::THREAD_OPEN => thread::open(&gateway, params).await,
        protocol::THREAD_CLOSE => thread::close(&gateway, params).await,
        protocol::THREAD_RENAME => thread::rename(&gateway, params),
        protocol::THREAD_ARCHIVE => thread::archive(&gateway, params),
        protocol::THREAD_RESTORE => thread::restore(&gateway, params),
        protocol::THREAD_DELETE => thread::delete(&gateway, params),
        protocol::THREAD_HISTORY => thread::history(&gateway, params),
        protocol::ITEM_IMAGE => thread::item_image(&gateway, params),
        protocol::THREAD_SUBSCRIBE => thread::subscribe(&gateway, params, subscribed),
        protocol::THREAD_UNSUBSCRIBE => thread::unsubscribe(params, subscribed),
        protocol::THREAD_ENVIRONMENT_GET => environment::get(&gateway, params),
        protocol::THREAD_MODEL_SET => environment::set_model(&gateway, params),
        protocol::THREAD_MODEL_REFRESH => environment::refresh_models(&gateway, params),
        protocol::THREAD_REASONING_SET => environment::set_reasoning(&gateway, params),
        protocol::THREAD_APPROVAL_SET => environment::set_approval(&gateway, params),
        protocol::THREAD_PLAN_SET => environment::set_plan(&gateway, params),
        protocol::THREAD_MEMORY_SET => environment::set_memory(&gateway, params),
        protocol::THREAD_GOAL_SET => environment::set_goal(&gateway, params),
        protocol::THREAD_GOAL_EDIT => environment::edit_goal(&gateway, params),
        protocol::THREAD_GOAL_PAUSE => environment::pause_goal(&gateway, params),
        protocol::THREAD_GOAL_COMPLETE | protocol::THREAD_GOAL_CLEAR => {
            environment::clear_goal(&gateway, params)
        }
        protocol::THREAD_CONTEXT_COMPACT => environment::compact(&gateway, params),
        protocol::THREAD_CONTEXT_GET => context::get(&gateway, params),
        protocol::TURN_START => turn::start(&gateway, params),
        protocol::TURN_ENQUEUE => turn::enqueue(&gateway, params),
        protocol::TURN_STEER => turn::steer(&gateway, params),
        protocol::TURN_CANCEL => turn::cancel(&gateway, params),
        protocol::TURN_QUEUE_LIST => turn::queue_list(&gateway, params),
        protocol::TURN_QUEUE_REMOVE => turn::queue_remove(&gateway, params),
        protocol::PERMISSION_RESOLVE => permission::resolve(&gateway, params),
        protocol::INTERACTION_RESPOND => interaction::respond(&gateway, params),
        protocol::PLAN_RESOLVE => interaction::plan_resolve(&gateway, params),
        protocol::ELICIT_RESOLVE => interaction::elicit_resolve(&gateway, params),
        protocol::SLASH_LIST => slash::list(&gateway, params),
        protocol::SLASH_EXECUTE => slash::execute(gateway, params).await,
        protocol::IMAGE_INPUTS_PUT => image_inputs::put(&gateway, params),
        protocol::SUBAGENT_LIST => subagent::list(&gateway, params),
        protocol::SUBAGENT_HISTORY => subagent::history(&gateway, params),
        protocol::SUBAGENT_SEND => subagent::send(&gateway, params),
        protocol::SUBAGENT_INTERRUPT => subagent::interrupt(&gateway, params),
        protocol::SUBAGENT_STOP => subagent::stop(&gateway, params),
        // 功能插件登记的方法（`"gateway.methods"`）。
        _ => methods::call(gateway, method, params).await,
    }
}

/// 这些线程级方法遇到关着的会话先开页再做（见 `thread::open_on_demand`）。不在里面的：
/// 停止 / 队列对关着的页回空结果；权限、提问、计划、elicitation 的应答只对开着的页
/// 有意义（关着的会话没有待处理的请求），仍回 `thread_not_open`。
fn opens_on_demand(method: &str) -> bool {
    matches!(
        method,
        protocol::THREAD_SUBSCRIBE
            | protocol::THREAD_ENVIRONMENT_GET
            | protocol::THREAD_MODEL_SET
            | protocol::THREAD_MODEL_REFRESH
            | protocol::THREAD_REASONING_SET
            | protocol::THREAD_APPROVAL_SET
            | protocol::THREAD_PLAN_SET
            | protocol::THREAD_MEMORY_SET
            | protocol::THREAD_GOAL_SET
            | protocol::THREAD_GOAL_EDIT
            | protocol::THREAD_GOAL_PAUSE
            | protocol::THREAD_GOAL_COMPLETE
            | protocol::THREAD_GOAL_CLEAR
            | protocol::THREAD_CONTEXT_COMPACT
            | protocol::THREAD_CONTEXT_GET
            | protocol::TURN_START
            | protocol::TURN_ENQUEUE
            | protocol::TURN_STEER
            | protocol::SLASH_EXECUTE
    )
}

/// 一次要几秒的方法：`ws.rs` 不在连接锁里跑它们（锁住会卡住这条连接的推送和其它请求），
/// 鉴权过了就另起任务，跑完再回帧。功能插件的方法按各自的 `MethodPolicy::detached`。
pub fn is_detached(gateway: &GatewayHandle, method: &str) -> bool {
    methods::policy_of(gateway, method).detached || is_core_detached(method)
}

fn is_core_detached(method: &str) -> bool {
    // 在跑的话要先停、等它停下来（最多几秒）。功能插件的方法看它自己的 `MethodPolicy`。
    method == protocol::THREAD_REWIND
}

/// [`is_detached`] 的方法：不碰连接状态，只要网关句柄。
pub async fn dispatch_detached(
    gateway: GatewayHandle,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    match method {
        protocol::THREAD_REWIND => thread::rewind(&gateway, params).await,
        _ => methods::call(gateway, method, params).await,
    }
}

/// 协议骨架自己处理的方法：上面 [`dispatch`] / [`dispatch_detached`] 的分支，加上 `ws.rs`
/// 里的连接级方法。浏览器 / 桌面画面（`browser/view/*`、`desktop/view/*`）另由
/// [`is_core_method`] 认。功能插件不能在 `"gateway.methods"` 里登记这些名字——表里的
/// 策略（detached、trusted_only……）会套到核心方法上，detached 还会把核心实现顶掉。
pub const CORE_METHODS: &[&str] = &[
    protocol::WORKSPACE_LIST,
    protocol::MCP_RELOAD,
    protocol::MCP_LIST,
    protocol::MCP_RECONNECT,
    protocol::MODEL_LIST,
    protocol::THREAD_LIST,
    protocol::THREAD_SEARCH,
    protocol::THREAD_REWIND,
    protocol::THREAD_START,
    protocol::THREAD_OPEN,
    protocol::THREAD_CLOSE,
    protocol::THREAD_RENAME,
    protocol::THREAD_ARCHIVE,
    protocol::THREAD_RESTORE,
    protocol::THREAD_DELETE,
    protocol::THREAD_HISTORY,
    protocol::ITEM_IMAGE,
    protocol::THREAD_SUBSCRIBE,
    protocol::THREAD_UNSUBSCRIBE,
    protocol::THREAD_ENVIRONMENT_GET,
    protocol::THREAD_MODEL_SET,
    protocol::THREAD_MODEL_REFRESH,
    protocol::THREAD_REASONING_SET,
    protocol::THREAD_APPROVAL_SET,
    protocol::THREAD_PLAN_SET,
    protocol::THREAD_MEMORY_SET,
    protocol::THREAD_GOAL_SET,
    protocol::THREAD_GOAL_EDIT,
    protocol::THREAD_GOAL_PAUSE,
    protocol::THREAD_GOAL_COMPLETE,
    protocol::THREAD_GOAL_CLEAR,
    protocol::THREAD_CONTEXT_COMPACT,
    protocol::THREAD_CONTEXT_GET,
    protocol::TURN_START,
    protocol::TURN_ENQUEUE,
    protocol::TURN_STEER,
    protocol::TURN_CANCEL,
    protocol::TURN_QUEUE_LIST,
    protocol::TURN_QUEUE_REMOVE,
    protocol::PERMISSION_RESOLVE,
    protocol::INTERACTION_RESPOND,
    protocol::PLAN_RESOLVE,
    protocol::ELICIT_RESOLVE,
    protocol::SLASH_LIST,
    protocol::SLASH_EXECUTE,
    protocol::IMAGE_INPUTS_PUT,
    protocol::SUBAGENT_LIST,
    protocol::SUBAGENT_HISTORY,
    protocol::SUBAGENT_SEND,
    protocol::SUBAGENT_INTERRUPT,
    protocol::SUBAGENT_STOP,
    protocol::CONNECTION_AUTHENTICATE,
    protocol::INITIALIZE,
    protocol::IMAGE_INPUTS_SYNC,
];

/// `method` 是不是网关核心方法（见 [`CORE_METHODS`]）。
pub fn is_core_method(method: &str) -> bool {
    CORE_METHODS.contains(&method)
        || crate::handlers::browser_view::is_browser_view(method)
        || crate::handlers::desktop_view::is_desktop_view(method)
}

#[cfg(test)]
mod tests {
    /// 核心名单不能和 `match` 漂开：这两个函数和 `ws.rs` 里点名处理的方法，一个不多一个不少。
    #[test]
    fn core_methods_cover_every_core_branch() {
        let names = |src: &str| -> Vec<String> {
            src.split("protocol::")
                .skip(1)
                .map(|rest| {
                    rest.chars()
                        .take_while(|c| c.is_ascii_uppercase() || *c == '_')
                        .collect::<String>()
                })
                .filter(|n| !n.is_empty())
                .collect()
        };
        let rpc = include_str!("rpc.rs");
        let dispatch = &rpc[rpc.find("pub async fn dispatch(").unwrap()
            ..rpc.find("/// 协议骨架自己处理的方法").unwrap()];
        let ws = include_str!("ws.rs");
        let mut want = names(dispatch);
        want.extend(
            ws.split("method == ")
                .skip(1)
                .flat_map(|rest| names(rest.lines().next().unwrap_or(""))),
        );
        let listed = names(&rpc[rpc.find("pub const CORE_METHODS").unwrap()..]);
        for name in &want {
            assert!(listed.contains(name), "CORE_METHODS 缺 protocol::{name}");
        }
        // 反过来也要对上：迁成插件的方法留在名单里，插件就登记不上了。
        for name in &listed {
            assert!(want.contains(name), "CORE_METHODS 多了 protocol::{name}");
        }
    }
}
