use serde_json::{json, Value};

use cordis_spine::{Ask, Mcp, PlanDecision, PlanMode, ASK, MCP, PLAN_MODE};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;
use crate::threads;

pub fn respond(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let ask = page
        .ctx
        .get::<Ask>(ASK)
        .ok_or_else(|| RpcError::app("unavailable", "ask service is not mounted"))?;
    let answers = params
        .get("answers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mapped: Vec<(String, Vec<String>, Option<String>)> = answers
        .iter()
        .filter_map(|a| {
            let id = a.get("questionId").and_then(Value::as_str)?.to_string();
            let value = a.get("value").and_then(Value::as_str)?.to_string();
            let notes = if a.get("kind").and_then(Value::as_str) == Some("other") {
                Some(value.clone())
            } else {
                None
            };
            Some((id, vec![value], notes))
        })
        .collect();
    ask.respond(&mapped)
        .map_err(|e| RpcError::app("empty_queue", e))?;
    Ok(json!({ "ok": true, "resumed": true }))
}

pub fn plan_resolve(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let decision = match params.get("decision").and_then(Value::as_str).unwrap_or("") {
        "approve" | "Approve" => PlanDecision::Approve,
        "revise" | "Revise" => PlanDecision::Revise,
        "quit" | "Quit" | "deny" => PlanDecision::Quit,
        _ => {
            return Err(RpcError::invalid_params(
                "decision must be approve, revise, or quit",
            ))
        }
    };
    let page = threads::resolve_param(gateway, &params)?;
    let plan = page
        .ctx
        .get::<PlanMode>(PLAN_MODE)
        .ok_or_else(|| RpcError::app("unavailable", "planMode service is not mounted"))?;
    if !plan.resolve(decision) {
        return Err(RpcError::app("empty_queue", "no pending plan approval"));
    }
    Ok(json!({ "ok": true, "resumed": true }))
}

pub fn elicit_resolve(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let action = params
        .get("action")
        .or_else(|| params.get("decision"))
        .and_then(Value::as_str)
        .unwrap_or("cancel");
    let content = params.get("content").cloned();
    let mcp = gateway
        .ctx()
        .get::<Mcp>(MCP)
        .ok_or_else(|| RpcError::app("unavailable", "mcp service is not mounted"))?;
    // MCP 连接是全局的，elicitation 队列按页区分：解的是这个线程那一页的队首。
    let page = threads::resolve_param(gateway, &params)?;
    mcp.elicitation()
        .resolve_on(Some(page.identity.as_str()), action, content)
        .map_err(|e| RpcError::app("empty_queue", e))?;
    Ok(json!({ "ok": true, "resumed": true }))
}
