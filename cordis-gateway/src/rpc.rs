//! JSON-RPC method dispatch. Unknown methods return `method_not_found`.

use std::collections::HashSet;

use serde_json::Value;

use crate::handle::GatewayHandle;
use crate::handlers::{
    connection, environment, image_inputs, interaction, permission, slash, thread, turn,
};
use crate::protocol::{self, RpcError};

pub async fn dispatch(
    gateway: GatewayHandle,
    method: &str,
    params: Value,
    subscribed: &mut HashSet<String>,
) -> Result<Value, RpcError> {
    match method {
        protocol::WORKSPACE_LIST => connection::workspace_list(&gateway),
        protocol::MCP_RELOAD => connection::mcp_reload(&gateway),
        protocol::THREAD_LIST => thread::list(&gateway, params),
        protocol::THREAD_START => thread::start(&gateway, params),
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
