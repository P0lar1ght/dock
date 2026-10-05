//! `status/*`：插件的状态项（`"status.items"`，含 `hud` 插槽）投给 GUI。
//!
//! - `status/list {}` → `{ items: [{ id, text, tone, tooltip, surface }] }`：
//!   文字是插件给的内容，只给受信 ticket（同 `surface/get`）。
//! - 推送 `status/changed { id }`（连接级）：状态项增删改了，或 `hud` 插槽变了。

use serde_json::{json, Value};

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{StatusItems, STATUS_CHANGED, STATUS_ITEMS, TUI_SLOTS_CHANGED};

use crate::handle::GatewayHandle;
use crate::methods::{method, register_methods, GatewayMethods, MethodPolicy, GATEWAY_METHODS};
use crate::protocol::{self, RpcError};

/// 状态项这一块：`status/list` 与推送 `status/changed`。
pub fn gateway_status() -> Plugin {
    plugin(
        "gateway.status",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            register_methods(
                ctx,
                vec![(
                    protocol::STATUS_LIST,
                    MethodPolicy {
                        trusted_only: true,
                        ..MethodPolicy::default()
                    },
                    method(|gw, _| async move { list(&gw) }),
                )],
            )?;
            // 状态项本身变了，或 `hud` 插槽变了（它们也在列表里），都推一条。
            let mut listeners = Vec::new();
            for event in [STATUS_CHANGED, TUI_SLOTS_CHANGED] {
                let live = ctx.clone();
                listeners.push(ctx.on(event, move |id: &String| {
                    if let Some(table) = live.get::<GatewayMethods>(GATEWAY_METHODS) {
                        table.notify(protocol::STATUS_CHANGED, json!({ "id": id }));
                    }
                })?);
            }
            ctx.effect("gateway.status.changed", |scope| {
                for l in listeners {
                    scope.own(l);
                }
                Ok(())
            })?;
            Ok(None)
        },
    )
}

/// 没挂状态项服务就是空表，不报错。
fn list(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let items: Vec<Value> = gateway
        .ctx()
        .get::<StatusItems>(STATUS_ITEMS)
        .map(|s| s.list())
        .unwrap_or_default()
        .into_iter()
        .map(|i| {
            json!({
                "id": i.id,
                "text": i.text,
                "tone": i.tone.as_str(),
                "tooltip": i.tooltip,
                "surface": i.surface,
            })
        })
        .collect();
    Ok(json!({ "items": items }))
}
