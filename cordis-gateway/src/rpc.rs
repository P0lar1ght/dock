//! JSON-RPC method dispatch：协议骨架写在这里，其余落到 `"gateway.methods"`（功能插件
//! 登记的方法）；两边都没有回 `method_not_found`。

use serde_json::Value;

use crate::handle::GatewayHandle;
use crate::handlers::{
    canvas, connection, context, environment, fs, image_inputs, interaction, permission, preset,
    settings, slash, subagent, thread, turn,
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
        protocol::CONFIG_STATUS => settings::config_status(),
        protocol::CONFIG_GET => settings::config_get(&gateway),
        protocol::CONFIG_SET => settings::config_set(&gateway, params),
        protocol::CONFIG_ENV => settings::config_env(params),
        protocol::MODEL_GET => settings::model_get(params),
        protocol::MODEL_SAVE => settings::model_save(&gateway, params),
        protocol::MODEL_DELETE => settings::model_delete(&gateway, params),
        protocol::MODEL_DEFAULT => settings::model_default(&gateway, params),
        protocol::MCP_GET => settings::mcp_get(params),
        protocol::MCP_TOOL_ENABLE => settings::mcp_tool_enable(&gateway, params).await,
        protocol::CUA_STATUS => settings::cua_status(&gateway),
        protocol::CUA_ACTION => settings::cua_action(&gateway, params),
        protocol::BROWSER_STATUS => settings::browser_status(&gateway),
        protocol::PLUGIN_LIST => settings::plugin_list(&gateway),
        protocol::PLUGIN_DELETE => settings::plugin_delete(&gateway, params).await,
        protocol::SKILL_LIST => settings::skill_list(&gateway),
        protocol::SECRET_LIST => settings::secret_list(),
        protocol::SECRET_SET => settings::secret_set(params),
        protocol::SECRET_DELETE => settings::secret_delete(params),
        protocol::PAIRING_LIST => settings::pairing_list(&gateway),
        protocol::PAIRING_RESOLVE => settings::pairing_resolve(&gateway, params),
        protocol::PAIRING_REVOKE => settings::pairing_revoke(&gateway, params),
        protocol::PAIRING_ACCEPT => settings::pairing_accept(&gateway, params),
        protocol::DEVICE_LIST => settings::device_list(),
        protocol::DEVICE_ADD => settings::device_add(params),
        protocol::DEVICE_REVOKE => settings::device_revoke(params),
        protocol::THREAD_LIST => thread::list(&gateway, params),
        protocol::THREAD_SEARCH => thread::search(params),
        protocol::PRESET_LIST => preset::list(&gateway, params),
        protocol::PRESET_CREATE => preset::create(&gateway, params),
        protocol::PRESET_DELETE => preset::delete(&gateway, params),
        protocol::PRESET_GET => preset::get(&gateway, params),
        protocol::PRESET_UPDATE => preset::update(&gateway, params),
        protocol::TOOL_CATALOG => preset::tool_catalog(&gateway, params),
        protocol::PRESET_DRAFT
        | protocol::PRESET_REWRITE
        | protocol::PRESET_SUGGEST_TOOLS
        | protocol::FS_LIST
        | protocol::FS_READ
        | protocol::FS_FIND
        | protocol::CANVAS_LIST
        | protocol::CANVAS_GET
        | protocol::CANVAS_SET_DATA
        | protocol::CANVAS_ROLLBACK
        | protocol::MODEL_TEST
        | protocol::MCP_SAVE
        | protocol::MCP_DELETE
        | protocol::MCP_ENABLE
        | protocol::MCP_LOGIN
        | protocol::PLUGIN_ENABLE
        | protocol::PLUGIN_PROMOTE
        | protocol::PLUGIN_DISCARD
        | protocol::THREAD_REWIND => dispatch_detached(gateway, method, params).await,
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

/// 要调模型的方法（一次几秒）和读盘的 `fs/*` / `canvas/*`（大仓库里找文件要走很多目录）。`ws.rs` 不在
/// 连接锁里跑它们（锁住会卡住这条连接的推送和其它请求），鉴权过了就另起任务，跑完再回帧。
pub fn is_detached(gateway: &GatewayHandle, method: &str) -> bool {
    methods::policy_of(gateway, method).detached || is_core_detached(method)
}

fn is_core_detached(method: &str) -> bool {
    matches!(
        method,
        protocol::PRESET_DRAFT
            | protocol::PRESET_REWRITE
            | protocol::PRESET_SUGGEST_TOOLS
            | protocol::FS_LIST
            | protocol::FS_READ
            | protocol::FS_FIND
            | protocol::CANVAS_LIST
            | protocol::CANVAS_GET
            | protocol::CANVAS_SET_DATA
            | protocol::CANVAS_ROLLBACK
            // 设置页里会连服务器 / 跑插件 / 等浏览器登录的：几秒到几分钟。
            | protocol::MODEL_TEST
            | protocol::MCP_SAVE
            | protocol::MCP_DELETE
            | protocol::MCP_ENABLE
            | protocol::MCP_LOGIN
            | protocol::PLUGIN_ENABLE
            | protocol::PLUGIN_PROMOTE
            | protocol::PLUGIN_DISCARD
            // 在跑的话要先停、等它停下来（最多几秒）。
            | protocol::THREAD_REWIND
    )
}

/// [`is_detached`] 的方法：不碰连接状态，只要网关句柄。
pub async fn dispatch_detached(
    gateway: GatewayHandle,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    match method {
        protocol::THREAD_REWIND => thread::rewind(&gateway, params).await,
        protocol::PRESET_DRAFT => preset::draft(&gateway, params).await,
        protocol::PRESET_REWRITE => preset::rewrite(&gateway, params).await,
        protocol::PRESET_SUGGEST_TOOLS => preset::suggest_tools(&gateway, params).await,
        protocol::FS_LIST | protocol::FS_READ | protocol::FS_FIND => {
            fs::dispatch(&gateway, method, params).await
        }
        protocol::CANVAS_LIST
        | protocol::CANVAS_GET
        | protocol::CANVAS_SET_DATA
        | protocol::CANVAS_ROLLBACK => canvas::dispatch(&gateway, method, params).await,
        protocol::MODEL_TEST => settings::model_test(params).await,
        protocol::MCP_SAVE => settings::mcp_save(&gateway, params).await,
        protocol::MCP_DELETE => settings::mcp_delete(&gateway, params).await,
        protocol::MCP_ENABLE => settings::mcp_enable(&gateway, params).await,
        protocol::MCP_LOGIN => settings::mcp_login(&gateway, params).await,
        protocol::PLUGIN_ENABLE => settings::plugin_enable(&gateway, params).await,
        protocol::PLUGIN_PROMOTE => settings::plugin_promote(&gateway, params).await,
        protocol::PLUGIN_DISCARD => settings::plugin_discard(&gateway, params).await,
        _ => methods::call(gateway, method, params).await,
    }
}
