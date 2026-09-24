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
    let mapped: Vec<(String, Vec<String>, Option<String>)> =
        answers.iter().filter_map(map_answer).collect();
    ask.respond(&mapped)
        .map_err(|e| RpcError::app("empty_queue", e))?;
    Ok(json!({ "ok": true, "resumed": true }))
}

/// 一题的回答。两种形状：
/// - `{questionId, values: [..], other?}`：选中的选项（多选题可以多个），外加可选
///   的自己写的回答；
/// - `{questionId, value, kind: "option" | "other"}`：旧形状，只有一个值。
///
/// 自己写的回答既算一个答案、也作为备注交给模型（和 TUI 的「其它」一致）。
fn map_answer(a: &Value) -> Option<(String, Vec<String>, Option<String>)> {
    let id = a.get("questionId").and_then(Value::as_str)?.to_string();
    if let Some(values) = a.get("values").and_then(Value::as_array) {
        let mut labels: Vec<String> = values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        let other = a
            .get("other")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if let Some(other) = &other {
            labels.push(other.clone());
        }
        return (!labels.is_empty()).then_some((id, labels, other));
    }
    let value = a.get("value").and_then(Value::as_str)?.to_string();
    let notes = (a.get("kind").and_then(Value::as_str) == Some("other")).then(|| value.clone());
    Some((id, vec![value], notes))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 多选题一次交多个选项，外加自己写的回答（既算答案也作为备注）；旧的
    /// `value` + `kind` 形状照旧；什么都没选的回答不算。
    #[test]
    fn answers_accept_several_values_and_the_old_shape() {
        let multi = json!({"questionId": "q1", "values": ["红", "蓝"], "other": " 紫 "});
        assert_eq!(
            map_answer(&multi),
            Some((
                "q1".into(),
                vec!["红".into(), "蓝".into(), "紫".into()],
                Some("紫".into())
            ))
        );
        let picked = json!({"questionId": "q1", "values": ["红"]});
        assert_eq!(
            map_answer(&picked),
            Some(("q1".into(), vec!["红".into()], None))
        );
        let old = json!({"questionId": "q1", "value": "随便", "kind": "other"});
        assert_eq!(
            map_answer(&old),
            Some(("q1".into(), vec!["随便".into()], Some("随便".into())))
        );
        assert_eq!(
            map_answer(&json!({"questionId": "q1", "values": [], "other": " "})),
            None
        );
    }
}
