//! JSON-RPC method dispatch. Unknown methods return `method_not_found`.

use serde_json::Value;

use crate::handle::GatewayHandle;
use crate::handlers::{
    connection, environment, image_inputs, interaction, permission, preset, slash, thread, turn,
};
use crate::protocol::{self, RpcError};

pub async fn dispatch(
    gateway: GatewayHandle,
    method: &str,
    params: Value,
    subscribed: &mut thread::Subscriptions,
) -> Result<Value, RpcError> {
    if opens_on_demand(method) {
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
        protocol::PRESET_LIST => preset::list(&gateway, params),
        protocol::PRESET_CREATE => preset::create(&gateway, params),
        protocol::PRESET_DELETE => preset::delete(&gateway, params),
        protocol::PRESET_GET => preset::get(&gateway, params),
        protocol::PRESET_UPDATE => preset::update(&gateway, params),
        protocol::TOOL_CATALOG => preset::tool_catalog(&gateway),
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
        _ => Err(RpcError::method_not_found(method)),
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
            | protocol::TURN_START
            | protocol::TURN_ENQUEUE
            | protocol::TURN_STEER
            | protocol::SLASH_EXECUTE
    )
}
