//! `tool/*`：插件给工具登记的卡片视图（`"tool.views"`）投给 GUI。
//!
//! 视图按需画：GUI 展开工具卡时才调 `tool/view`，不给每条工具结果都跑一遍插件。
//!
//! - `tool/views {}` → `{ tools: [name] }`：哪些工具有卡片视图（GUI 据此显示「视图 | 原始」）。
//! - `tool/view { threadId?, itemId }` → `{ view }`：那次调用的视图树（规范化 JSON），
//!   插件这次不给 / 没登记时 `view` 是 `null`。只给受信连接：会跑插件脚本。
//! - 推送 `tool/views/changed { name }`（连接级）：有工具的视图登记或卸下了。

use serde_json::{json, Value};

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{
    LogEvent, Sessions, ToolViewInput, ToolViews, SESSIONS, TOOL_VIEWS, TOOL_VIEWS_CHANGED,
};

use crate::handle::GatewayHandle;
use crate::methods::{method, register_methods, GatewayMethods, MethodPolicy, GATEWAY_METHODS};
use crate::protocol::{self, RpcError};
use crate::threads;

/// 工具卡视图这一块：`tool/views`、`tool/view` 与推送 `tool/views/changed`。
pub fn gateway_tool_views() -> Plugin {
    plugin(
        "gateway.toolViews",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            let trusted = MethodPolicy {
                trusted_only: true,
                ..MethodPolicy::default()
            };
            register_methods(
                ctx,
                vec![
                    (
                        protocol::TOOL_VIEWS,
                        MethodPolicy::default(),
                        method(|gw, _| async move { names(&gw) }),
                    ),
                    (
                        protocol::TOOL_VIEW,
                        trusted,
                        method(|gw, params| async move { view(&gw, params) }),
                    ),
                ],
            )?;
            let live = ctx.clone();
            let listener = ctx.on(TOOL_VIEWS_CHANGED, move |name: &String| {
                if let Some(table) = live.get::<GatewayMethods>(GATEWAY_METHODS) {
                    table.notify(protocol::TOOL_VIEWS_CHANGED, json!({ "name": name }));
                }
            })?;
            ctx.effect("gateway.toolViews.changed", |scope| {
                scope.own(listener);
                Ok(())
            })?;
            Ok(None)
        },
    )
}

fn names(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let tools = gateway
        .ctx()
        .get::<ToolViews>(TOOL_VIEWS)
        .map(|v| v.names())
        .unwrap_or_default();
    Ok(json!({ "tools": tools }))
}

fn view(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let item = params
        .get("itemId")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("itemId is required"))?
        .to_string();
    let page = threads::resolve_param(gateway, &params)?;
    let sessions = page
        .ctx
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| RpcError::app("unavailable", "sessions service is not mounted"))?;
    let views = gateway.ctx().get::<ToolViews>(TOOL_VIEWS);
    // 先把这次调用抄出来再画：渲染函数可能跑插件脚本，不能拿着会话日志的锁。
    let call = sessions.with_log(|events, _| {
        events.iter().rev().find_map(|e| match e {
            LogEvent::ToolExecute {
                id,
                name,
                arguments,
                content,
                is_error,
                ..
            } if *id == item => Some((name.clone(), arguments.clone(), content.clone(), *is_error)),
            _ => None,
        })
    });
    let Some((name, arguments, output, failed)) = call else {
        return Err(RpcError::app("not_found", format!("没有工具结果 {item}")));
    };
    let view = views.and_then(|v| {
        v.render(&ToolViewInput {
            name: &name,
            arguments: &arguments,
            output: &output,
            failed,
        })
    });
    Ok(json!({ "view": view.map(|v| v.to_value()) }))
}
