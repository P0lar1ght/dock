//! 沙箱（dock.1 `sandbox/*`）：一页的沙箱现状、行为链路图和时间线，给 GUI 的「行为轨迹」面板。
//!
//! - `sandbox/status { threadId }` → 开没开、工作区、审计记了多少条 / 哈希链完不完好、出口代理、
//!   限制、策略规则、直通名单（[`cordis_spine::sandbox_status`]）。会验一遍日志，别每秒调。
//! - `sandbox/trace { threadId, limit? }` → `{ auditing, records, graph: { nodes, edges }, totals,
//!   timeline }`（[`cordis_spine::sandbox_trace`]）。面板开着时轮询它；没开沙箱时是空的。
//!
//! - `sandbox/http { threadId, limit? }` → `{ auditing, records, requests }`：最近的出网（HTTP 历史）。
//! - `sandbox/flow { threadId, id }` → 一次出网的请求包 / 响应包（首行、头、正文，压缩的解开）。
//!   正文没遮凭据（头遮了）。
//!
//! 四个方法都只给受信连接：命令全文、网址（query 里可能带 token）、请求包都是审计数据，
//! 配对的网页不该看到。
//!
//! 开关、导出、临时放行、换工作区都走斜杠（`slash/execute` 的 `/sandbox …`），这里不另开方法。

use cordis::{plugin, Inject, Plugin};
use serde_json::Value;

use crate::handle::GatewayHandle;
use crate::methods::{method, register_methods, MethodPolicy, GATEWAY_METHODS};
use crate::protocol::{self, RpcError};
use crate::threads;

/// 时间线默认 / 最多回几条。
const TIMELINE_DEFAULT: usize = 300;
const TIMELINE_MAX: usize = 2000;

pub fn gateway_sandbox() -> Plugin {
    plugin(
        "gateway.sandbox",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            register_methods(
                ctx,
                vec![
                    (
                        protocol::SANDBOX_STATUS,
                        MethodPolicy {
                            detached: true,
                            trusted_only: true,
                            ..MethodPolicy::default()
                        },
                        method(|gw, params| async move { status(&gw, params) }),
                    ),
                    (
                        protocol::SANDBOX_TRACE,
                        MethodPolicy {
                            detached: true,
                            trusted_only: true,
                            ..MethodPolicy::default()
                        },
                        method(|gw, params| async move { trace(&gw, params) }),
                    ),
                    (
                        protocol::SANDBOX_HTTP,
                        MethodPolicy {
                            detached: true,
                            trusted_only: true,
                            ..MethodPolicy::default()
                        },
                        method(|gw, params| async move { http(&gw, params) }),
                    ),
                    (
                        protocol::SANDBOX_FLOW,
                        MethodPolicy {
                            detached: true,
                            trusted_only: true,
                            ..MethodPolicy::default()
                        },
                        method(|gw, params| async move { flow(&gw, params) }),
                    ),
                ],
            )?;
            Ok(None)
        },
    )
}

fn status(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    Ok(cordis_spine::sandbox_status(&page.ctx))
}

fn trace(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).min(TIMELINE_MAX))
        .unwrap_or(TIMELINE_DEFAULT);
    Ok(cordis_spine::sandbox_trace(&page.ctx, limit))
}

fn http(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).min(TIMELINE_MAX))
        .unwrap_or(1000);
    Ok(cordis_spine::sandbox_http(&page.ctx, limit))
}

fn flow(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("id 必填"))?;
    cordis_spine::sandbox_flow(&page.ctx, id).ok_or_else(|| {
        RpcError::app(
            "not_found",
            format!("没有这次出网：{id}（沙箱关了就看不到了）"),
        )
    })
}
