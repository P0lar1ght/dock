//! `surface/*`：插件的面板（`"tui.slots"`）投给 GUI。
//!
//! 面板不分端：正文是一段文本，动作是按钮。终端里按键、GUI 里点按钮都落到插件的
//! `on_key`（动作 id 就是那个键）。
//!
//! - `surface/list {}` → `{ surfaces: [{ id, title, hud }] }`。
//! - `surface/get { id }` → `{ surface: { id, title, body, actions: [{ id, label }] } }`。
//! - `surface/action { id, action }` → `{ closed, surface }`：只认面板声明过的动作；
//!   动作会跑插件脚本，所以只给受信 ticket（同设置页）。`closed` 为真表示插件要关面板。
//! - 推送 `surface/changed { id }`（连接级，不用订阅）：增删、被操作或插件说正文变了。

use serde_json::{json, Value};

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{SlotKeyResult, TuiSlots, TUI_SLOTS, TUI_SLOTS_CHANGED};

use crate::handle::GatewayHandle;
use crate::methods::{method, register_methods, GatewayMethods, MethodPolicy, GATEWAY_METHODS};
use crate::protocol::{self, RpcError};

/// 插件面板这一块：`surface/*` 与推送 `surface/changed`。
pub fn gateway_surfaces() -> Plugin {
    plugin(
        "gateway.surfaces",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            register_methods(
                ctx,
                vec![
                    (
                        protocol::SURFACE_LIST,
                        MethodPolicy::default(),
                        method(|gw, _| async move { list(&gw) }),
                    ),
                    (
                        protocol::SURFACE_GET,
                        MethodPolicy::default(),
                        method(|gw, params| async move { get(&gw, params) }),
                    ),
                    (
                        protocol::SURFACE_ACTION,
                        MethodPolicy {
                            trusted_only: true,
                            ..MethodPolicy::default()
                        },
                        method(|gw, params| async move { action(&gw, params) }),
                    ),
                ],
            )?;
            let live = ctx.clone();
            let listener = ctx.on(TUI_SLOTS_CHANGED, move |id: &String| {
                if let Some(table) = live.get::<GatewayMethods>(GATEWAY_METHODS) {
                    table.notify(protocol::SURFACE_CHANGED, json!({ "id": id }));
                }
            })?;
            ctx.effect("gateway.surfaces.changed", |scope| {
                scope.own(listener);
                Ok(())
            })?;
            Ok(None)
        },
    )
}

fn slots(gateway: &GatewayHandle) -> Result<std::sync::Arc<TuiSlots>, RpcError> {
    gateway
        .ctx()
        .get::<TuiSlots>(TUI_SLOTS)
        .ok_or_else(|| RpcError::app("unavailable", "插件面板服务没有挂载"))
}

fn surface_id(params: &Value) -> Result<String, RpcError> {
    params
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| RpcError::invalid_params("id is required"))
}

fn list(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let surfaces: Vec<Value> = slots(gateway)?
        .list()
        .into_iter()
        .map(|s| json!({ "id": s.id, "title": s.title, "hud": s.hud }))
        .collect();
    Ok(json!({ "surfaces": surfaces }))
}

fn snapshot(slots: &TuiSlots, id: &str) -> Result<Value, RpcError> {
    let handler = slots
        .get(id)
        .ok_or_else(|| RpcError::app("not_found", format!("没有面板 {id}")))?;
    let actions: Vec<Value> = handler
        .actions()
        .into_iter()
        .map(|a| json!({ "id": a.id, "label": a.label }))
        .collect();
    Ok(json!({
        "id": id,
        "title": handler.title(),
        "body": handler.render(),
        "actions": actions,
    }))
}

fn get(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = surface_id(&params)?;
    let slots = slots(gateway)?;
    Ok(json!({ "surface": snapshot(&slots, &id)? }))
}

fn action(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = surface_id(&params)?;
    let action = params
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("action is required"))?;
    let slots = slots(gateway)?;
    if !slots.actions(&id).iter().any(|a| a.id == action) {
        if slots.get(&id).is_none() {
            return Err(RpcError::app("not_found", format!("没有面板 {id}")));
        }
        return Err(RpcError::invalid_params(format!(
            "面板 {id} 没有声明动作 {action}"
        )));
    }
    let closed = matches!(slots.on_key(&id, action), SlotKeyResult::Close);
    Ok(json!({ "closed": closed, "surface": snapshot(&slots, &id)? }))
}
