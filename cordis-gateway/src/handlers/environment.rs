use serde_json::{json, Value};

use cordis_spine::{
    snapshot_context, AppSettings, Goal, Mcp, PermissionMode, PlanMode, GOAL, MCP, PLAN_MODE,
    SETTINGS,
};
use cordis_tui::{SessionRef, SESSION_PORT};

use crate::handle::GatewayHandle;
use crate::protocol::{RpcError, LIVE_THREAD_ID};

pub fn get(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let mut map = environment_fields(gateway)?;
    map.insert("threadId".into(), json!(LIVE_THREAD_ID));
    Ok(Value::Object(map))
}

pub fn refresh_models(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    get(gateway, params)
}

pub fn set_reasoning(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let effort = params
        .get("effort")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("effort is required"))?;
    let settings = settings(gateway)?;
    match effort {
        "none" => {
            settings.set_thinking(false);
            settings.set_effort("none");
        }
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
            settings.set_thinking(true);
            settings.set_effort(effort);
        }
        _ => {
            return Err(RpcError::invalid_params(
                "effort must be none, low, medium, high, or similar",
            ))
        }
    }
    get(gateway, params)
}

pub fn set_memory(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    // Dock memory is a pair of tools, not PolarVigil read/write gates.
    let _ = params.get("read");
    let _ = params.get("write");
    get(gateway, params)
}

pub fn set_goal(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let content = params
        .get("content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("content is required"))?;
    goal_service(gateway)?.start(content);
    get(gateway, params)
}

pub fn edit_goal(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let content = params
        .get("content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("content is required"))?;
    let goal = goal_service(gateway)?;
    if !goal.present() {
        return Err(RpcError::app("not_found", "no active goal"));
    }
    goal.set_title(content);
    Ok(json!({ "goal": goal_object(gateway) }))
}

pub fn pause_goal(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let goal = goal_service(gateway)?;
    if !goal.pause() {
        return Err(RpcError::app("not_found", "no active goal to pause"));
    }
    Ok(json!({ "goal": goal_object(gateway) }))
}

pub fn clear_goal(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let _ = params;
    goal_service(gateway)?.clear();
    Ok(json!({ "goal": goal_object(gateway) }))
}

pub fn set_model(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let model_id = params
        .get("modelId")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("modelId is required"))?;
    let settings = settings(gateway)?;
    settings.set_model(model_id);
    get(gateway, params)
}

pub fn set_approval(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let mode = params
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("");
    let settings = settings(gateway)?;
    match mode {
        "ask" => settings.set_permission_mode(PermissionMode::Ask),
        "auto" => settings.set_permission_mode(PermissionMode::Allow),
        "full_access" => {
            return Err(RpcError::app(
                "capability_not_supported",
                "full-access confirmation is not implemented",
            ))
        }
        _ => return Err(RpcError::invalid_params("mode must be ask or auto")),
    }
    get(gateway, params)
}

pub fn set_plan(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let enabled = params.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let plan = gateway
        .ctx()
        .get::<PlanMode>(PLAN_MODE)
        .ok_or_else(|| RpcError::app("unavailable", "planMode service is not mounted"))?;
    plan.set(enabled);
    get(gateway, params)
}

pub fn compact(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let extra = params
        .get("context")
        .or_else(|| params.get("instructions"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let port = gateway
        .ctx()
        .get::<SessionRef>(SESSION_PORT)
        .ok_or_else(|| RpcError::app("unavailable", "session.port is not mounted"))?;
    port.compact(extra);
    Ok(json!({
        "threadId": LIVE_THREAD_ID,
        "ok": true,
        "status": "running"
    }))
}

fn settings(gateway: &GatewayHandle) -> Result<std::sync::Arc<AppSettings>, RpcError> {
    gateway
        .ctx()
        .get::<AppSettings>(SETTINGS)
        .ok_or_else(|| RpcError::app("unavailable", "settings service is not mounted"))
}

fn goal_service(gateway: &GatewayHandle) -> Result<std::sync::Arc<Goal>, RpcError> {
    gateway
        .ctx()
        .get::<Goal>(GOAL)
        .ok_or_else(|| RpcError::app("unavailable", "goal service is not mounted"))
}

fn goal_object(gateway: &GatewayHandle) -> Value {
    match gateway.ctx().get::<Goal>(GOAL) {
        Some(goal) if goal.present() => {
            let status = if goal.paused() { "paused" } else { "active" };
            json!({
                "status": status,
                "summary": goal.title(),
                "truncated": false,
                "progressSummary": goal.status(),
                "revision": 1
            })
        }
        _ => json!({
            "status": "none",
            "summary": "",
            "truncated": false
        }),
    }
}

fn environment_fields(gateway: &GatewayHandle) -> Result<serde_json::Map<String, Value>, RpcError> {
    let settings = settings(gateway)?;
    let model = settings.model();
    let catalog = settings.catalog();
    let options: Vec<Value> = catalog
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "label": if m.name.is_empty() { m.id.clone() } else { m.name.clone() },
                "available": true,
                "inputModalities": ["text"]
            })
        })
        .collect();
    let approval = match settings.permission_mode() {
        PermissionMode::Ask => "ask",
        PermissionMode::Allow => "auto",
    };
    let plan_on = gateway
        .ctx()
        .get::<PlanMode>(PLAN_MODE)
        .is_some_and(|p| p.active());
    let mcp = gateway.ctx().get::<Mcp>(MCP);
    let servers: Vec<Value> = mcp
        .as_ref()
        .map(|m| {
            m.list()
                .into_iter()
                .map(|s| {
                    json!({
                        "id": s.name,
                        "label": s.name,
                        "status": if !s.enabled {
                            "disabled"
                        } else if s.ok {
                            "connected"
                        } else {
                            "unavailable"
                        },
                        "toolCount": s.tools.iter().filter(|t| t.enabled).count()
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let connected = servers
        .iter()
        .filter(|s| s.get("status").and_then(Value::as_str) == Some("connected"))
        .count();
    let tool_count: usize = mcp
        .as_ref()
        .map(|m| {
            m.list()
                .iter()
                .map(|s| s.tools.iter().filter(|t| t.enabled).count())
                .sum()
        })
        .unwrap_or(0);
    let snap = snapshot_context(gateway.ctx());
    let (used, total, pct, trigger) = (
        snap.used,
        snap.total,
        snap.usage_pct as u64,
        snap.auto_compact_threshold_percent as u64,
    );
    let mut map = serde_json::Map::new();
    map.insert(
        "model".into(),
        json!({
            "id": model,
            "label": model,
            "inputModalities": ["text"],
            "canChange": true,
            "options": options
        }),
    );
    map.insert(
        "approval".into(),
        json!({
            "mode": approval,
            "canChange": true,
            "options": ["ask", "auto"]
        }),
    );
    map.insert(
        "reasoning".into(),
        json!({
            "supported": true,
            "effort": if settings.thinking() {
                settings.effort()
            } else {
                "none".into()
            },
            "canChange": true,
            "options": ["none", "low", "medium", "high"]
        }),
    );
    map.insert("goal".into(), goal_object(gateway));
    map.insert(
        "plan".into(),
        json!({
            "enabled": plan_on,
            "canChange": true
        }),
    );
    map.insert(
        "memory".into(),
        json!({
            "read": false,
            "write": false,
            "canRead": false,
            "canWrite": false
        }),
    );
    map.insert(
        "mcp".into(),
        json!({
            "enabled": mcp.is_some(),
            "connectedServers": connected,
            "totalServers": servers.len(),
            "toolCount": tool_count,
            "servers": servers
        }),
    );
    map.insert(
        "rules".into(),
        json!({
            "appliedCount": 0,
            "order": []
        }),
    );
    map.insert(
        "context".into(),
        json!({
            "estimatedTokens": used,
            "maxContextTokens": total,
            "inputBudgetTokens": total,
            "reservedOutputTokens": 0,
            "usagePercent": pct,
            "compactionEnabled": true,
            "canCompact": true,
            "compactionTriggerPercent": trigger,
            "compactionTriggerTokens": total.saturating_mul(trigger) / 100,
            "revisionSeq": 0
        }),
    );
    Ok(map)
}
