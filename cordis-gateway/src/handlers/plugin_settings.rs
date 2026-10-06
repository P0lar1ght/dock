//! `plugin/settings/*`：插件的设置卡（`"plugin.settings"`）投给 GUI 设置页。
//!
//! 都在设置页命名空间里，只认受信连接（插件配置可能是部署目标、令牌）。
//!
//! - `plugin/settings/list {}` → `{ plugins: [{ pluginId, title }] }`。
//! - `plugin/settings/get { pluginId }` → `{ pluginId, schema, values, secrets }`：
//!   `values` 是普通字段的当前值（没写过是 default），`secrets` 只说设没设。
//! - `plugin/settings/set { pluginId, values }` →
//!   成功 `{ ok: true, settings }`；字段不合法 `{ ok: false, errors: { key: 原因 } }`，整组不写。
//!   密钥字段：空串不改、`null` 删掉。
//! - 推送 `plugin/settings/changed { pluginId }`（连接级）。

use serde_json::{json, Map, Value};

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{PluginSettings, PLUGIN_SETTINGS, PLUGIN_SETTINGS_CHANGED};

use crate::handle::GatewayHandle;
use crate::methods::{method, register_methods, GatewayMethods, MethodPolicy, GATEWAY_METHODS};
use crate::protocol::{self, RpcError};

/// 插件设置卡这一块。
pub fn gateway_plugin_settings() -> Plugin {
    plugin(
        "gateway.pluginSettings",
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
                        protocol::PLUGIN_SETTINGS_LIST,
                        trusted,
                        method(|gw, _| async move { list(&gw) }),
                    ),
                    (
                        protocol::PLUGIN_SETTINGS_GET,
                        trusted,
                        method(|gw, params| async move { get(&gw, params) }),
                    ),
                    (
                        protocol::PLUGIN_SETTINGS_SET,
                        trusted,
                        method(|gw, params| async move { set(&gw, params) }),
                    ),
                ],
            )?;
            let live = ctx.clone();
            let listener = ctx.on(PLUGIN_SETTINGS_CHANGED, move |id: &String| {
                if let Some(table) = live.get::<GatewayMethods>(GATEWAY_METHODS) {
                    table.notify(protocol::PLUGIN_SETTINGS_CHANGED, json!({ "pluginId": id }));
                }
            })?;
            ctx.effect("gateway.pluginSettings.changed", |scope| {
                scope.own(listener);
                Ok(())
            })?;
            Ok(None)
        },
    )
}

fn settings(gateway: &GatewayHandle) -> Result<std::sync::Arc<PluginSettings>, RpcError> {
    gateway
        .ctx()
        .get::<PluginSettings>(PLUGIN_SETTINGS)
        .ok_or_else(|| RpcError::app("unavailable", "插件设置服务没有挂载"))
}

fn plugin_id(params: &Value) -> Result<String, RpcError> {
    params
        .get("pluginId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| RpcError::invalid_params("pluginId is required"))
}

/// 没挂服务就是空表。
fn list(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let plugins: Vec<Value> = gateway
        .ctx()
        .get::<PluginSettings>(PLUGIN_SETTINGS)
        .map(|s| s.list())
        .unwrap_or_default()
        .into_iter()
        .map(|(id, title)| json!({ "pluginId": id, "title": title }))
        .collect();
    Ok(json!({ "plugins": plugins }))
}

fn get(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = plugin_id(&params)?;
    settings(gateway)?
        .snapshot(&id)
        .ok_or_else(|| RpcError::app("not_found", format!("插件 {id} 没有设置卡")))
}

fn set(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = plugin_id(&params)?;
    let values = params
        .get("values")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| RpcError::invalid_params("values must be an object"))?;
    let settings = settings(gateway)?;
    if settings.schema(&id).is_none() {
        return Err(RpcError::app("not_found", format!("插件 {id} 没有设置卡")));
    }
    match settings.set(&id, &values) {
        Ok(()) => Ok(json!({ "ok": true, "settings": settings.snapshot(&id) })),
        Err(errors) => {
            let errors: Map<String, Value> = errors
                .into_iter()
                .map(|(k, m)| (k, Value::String(m)))
                .collect();
            Ok(json!({ "ok": false, "errors": errors }))
        }
    }
}
