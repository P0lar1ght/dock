use serde_json::{json, Value};

use cordis_spine::{
    snapshot_context, AppSettings, ContextBook, Goal, GoalBudget, Mcp, PermissionMode, PlanMode,
    Sessions, CONTEXT, GOAL, MCP, PLAN_MODE, SESSIONS, SETTINGS,
};
use cordis_spine::{SessionRef, SESSION_PORT};

use cordis::Context;

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;
use crate::threads;

// 这些都是**线程级**的：模型、推理、审批、计划、目标各页一份（`threadId` 解析到
// 哪一页就改哪一页），MCP 仍是全局的。

pub fn get(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let thread_id = threads::thread_param(&params);
    let page = threads::resolve(gateway, &thread_id)?;
    let mut map = environment_fields(&page.ctx)?;
    map.insert("threadId".into(), json!(thread_id));
    Ok(Value::Object(map))
}

fn page_ctx(gateway: &GatewayHandle, params: &Value) -> Result<Context, RpcError> {
    Ok(threads::resolve_param(gateway, params)?.ctx)
}

pub fn refresh_models(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    get(gateway, params)
}

pub fn set_reasoning(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let effort = params
        .get("effort")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("effort is required"))?;
    let page = threads::resolve_param(gateway, &params)?;
    let settings = settings(&page.ctx)?;
    let choices = settings.effort_choices();
    match effort {
        "none" => {
            settings.set_thinking(false);
            settings.set_effort("none");
        }
        effort if accepts_effort(effort, &choices) => {
            settings.set_thinking(true);
            settings.set_effort(effort);
        }
        _ => {
            return Err(RpcError::invalid_params(format!(
                "这个模型没有「{effort}」这一档（可选：none、{}）",
                choices.join("、")
            )))
        }
    }
    // 切了就落盘：会话下次开还是这一档。
    page.sessions()?.persist_sampling();
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
    goal_service(&page_ctx(gateway, &params)?)?.start(content);
    get(gateway, params)
}

pub fn edit_goal(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let content = params
        .get("content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("content is required"))?;
    let ctx = page_ctx(gateway, &params)?;
    let goal = goal_service(&ctx)?;
    if !goal.present() {
        return Err(RpcError::app("not_found", "no active goal"));
    }
    goal.set_title(content);
    Ok(json!({ "goal": goal_object(&ctx) }))
}

pub fn pause_goal(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let ctx = page_ctx(gateway, &params)?;
    let goal = goal_service(&ctx)?;
    if !goal.pause() {
        return Err(RpcError::app("not_found", "no active goal to pause"));
    }
    Ok(json!({ "goal": goal_object(&ctx) }))
}

pub fn clear_goal(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let ctx = page_ctx(gateway, &params)?;
    goal_service(&ctx)?.clear();
    Ok(json!({ "goal": goal_object(&ctx) }))
}

pub fn set_model(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let model_id = params
        .get("modelId")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("modelId is required"))?;
    let page = threads::resolve_param(gateway, &params)?;
    settings(&page.ctx)?.set_model(model_id);
    page.sessions()?.persist_sampling();
    get(gateway, params)
}

pub fn set_approval(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let mode = params.get("mode").and_then(Value::as_str).unwrap_or("");
    let settings = settings(&page_ctx(gateway, &params)?)?;
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
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let plan = page_ctx(gateway, &params)?
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
    let thread_id = threads::thread_param(&params);
    let port = threads::resolve(gateway, &thread_id)?
        .ctx
        .get::<SessionRef>(SESSION_PORT)
        .ok_or_else(|| RpcError::app("unavailable", "session.port is not mounted"))?;
    port.compact(extra);
    Ok(json!({
        "threadId": thread_id,
        "ok": true,
        "status": "running"
    }))
}

fn settings(ctx: &Context) -> Result<std::sync::Arc<AppSettings>, RpcError> {
    ctx.get::<AppSettings>(SETTINGS)
        .ok_or_else(|| RpcError::app("unavailable", "settings service is not mounted"))
}

fn goal_service(ctx: &Context) -> Result<std::sync::Arc<Goal>, RpcError> {
    ctx.get::<Goal>(GOAL)
        .ok_or_else(|| RpcError::app("unavailable", "goal service is not mounted"))
}

fn goal_object(ctx: &Context) -> Value {
    match ctx.get::<Goal>(GOAL) {
        Some(goal) if goal.present() || goal.completed() => {
            let status = if goal.completed() {
                "completed"
            } else if goal.paused() {
                "paused"
            } else {
                "active"
            };
            json!({
                "status": status,
                "summary": goal.title(),
                "truncated": false,
                "progressSummary": goal.status(),
                "revision": 1,
                // 实际在跑的时长（暂停的时段不算）；客户端在 `active` 时自己往上走表。
                "elapsedMs": goal.elapsed().as_millis() as u64,
                // 这个目标用掉的请求数（本页的采样次数）。
                "requests": goal.requests(),
                // 用户设的上限（`/goal budget`）；不限为 null。
                "budget": match goal.budget() {
                    Some(GoalBudget::Requests(n)) => json!({ "requests": n }),
                    Some(GoalBudget::Time(d)) => json!({ "ms": d.as_millis() as u64 }),
                    None => Value::Null,
                }
            })
        }
        _ => json!({
            "status": "none",
            "summary": "",
            "truncated": false
        }),
    }
}

fn environment_fields(ctx: &Context) -> Result<serde_json::Map<String, Value>, RpcError> {
    let settings = settings(ctx)?;
    let model = settings.model();
    let catalog = settings.catalog();
    let options: Vec<Value> = catalog
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "label": if m.name.is_empty() { m.id.clone() } else { m.name.clone() },
                "available": true,
                "inputModalities": ["text", "image"],
                // 走订阅通道的模型（`auth = "chatgpt"`）：界面据此显示
                // 「Using ChatGPT plan」。老 Dock 没有这个字段，界面按缺省处理。
                "authProvider": m.auth_provider(),
            })
        })
        .collect();
    let approval = match settings.permission_mode() {
        PermissionMode::Ask => "ask",
        PermissionMode::Allow => "auto",
    };
    let plan_on = ctx.get::<PlanMode>(PLAN_MODE).is_some_and(|p| p.active());
    let mcp = ctx.get::<Mcp>(MCP);
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
    let snap = ctx
        .get::<ContextBook>(CONTEXT)
        .map(|b| b.window_on(ctx))
        .unwrap_or_else(|| snapshot_context(ctx));
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
            "inputModalities": ["text", "image"],
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
    // 推理档位跟着当前模型走（以前写死 none/low/medium/high，GUI 里每个模型都一样）。
    let reasoning_on = settings.reasoning_available();
    let subscription =
        cordis_base::config::lookup_model(&model).is_some_and(|c| c.auth_provider().is_some());
    let effort_options = effort_menu(reasoning_on, settings.effort_choices(), subscription);
    map.insert(
        "reasoning".into(),
        json!({
            "supported": reasoning_on,
            "effort": if settings.thinking() {
                settings.effort()
            } else {
                "none".into()
            },
            "canChange": !effort_options.is_empty(),
            "options": effort_options
        }),
    );
    map.insert("goal".into(), goal_object(ctx));
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
    let mut context = json!({
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
    });
    // 正在压缩或最近一次压缩：订阅到一半才接入的客户端靠它补上进度。
    if let Some(progress) = ctx.get::<Sessions>(SESSIONS).and_then(|s| s.compaction()) {
        let mut last = crate::transcript::compaction_value(&progress);
        let tokens = progress.after_tokens.unwrap_or(progress.before_tokens);
        last["usagePercent"] = json!((tokens.saturating_mul(100)).checked_div(total).unwrap_or(0));
        last["seq"] = json!(0);
        context["lastCompaction"] = last;
    }
    map.insert("context".into(), context);
    Ok(map)
}

/// 推理菜单列哪几档：当前模型声明的档位，前面加「关」（none）。
///
/// - 不推理的模型、或会推理但明说没有档位的（`reasoning_efforts = []`）：空菜单；
/// - 订阅模型的档位来自上游 / 模型目录：列表里有 `none` 才给「关」——不收 none 的模型
///   （grok-4.6）给了也关不掉；
/// - 自己配的模型照旧总给「关」：DeepSeek 这类用单独的开关关推理，不在档位表里。
fn effort_menu(reasoning_on: bool, choices: Vec<String>, subscription: bool) -> Vec<String> {
    if !reasoning_on || choices.is_empty() {
        return Vec::new();
    }
    let mut menu = Vec::with_capacity(choices.len() + 1);
    if !subscription && !choices.iter().any(|c| c == "none") {
        menu.push("none".to_string());
    }
    menu.extend(choices);
    menu
}

/// `thread/reasoning/set` 认不认这一档：当前模型声明过的都认（`ultra` 这种也行）；
/// 老客户端发的那几档通用值照旧认（不认得的模型档位，推理层会去掉重发）。
fn accepts_effort(effort: &str, choices: &[String]) -> bool {
    const COMMON: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max"];
    choices.iter().any(|c| c == effort) || COMMON.contains(&effort)
}

#[cfg(test)]
mod effort_tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_menu_follows_the_model() {
        // ChatGPT 订阅：上游给的就是这些，没有 none 就不给「关」。
        assert_eq!(
            effort_menu(true, v(&["low", "medium", "high", "xhigh", "max"]), true),
            v(&["low", "medium", "high", "xhigh", "max"])
        );
        // 订阅模型列了 none：照列。
        assert_eq!(
            effort_menu(true, v(&["none", "low", "high"]), true),
            v(&["none", "low", "high"])
        );
        // 自己配的模型（deepseek-flash）：前面加「关」。
        assert_eq!(
            effort_menu(true, v(&["low", "high", "max"]), false),
            v(&["none", "low", "high", "max"])
        );
        // 会推理但没有档位、不推理：空菜单。
        assert!(effort_menu(true, vec![], true).is_empty());
        assert!(effort_menu(false, v(&["low"]), false).is_empty());
    }

    #[test]
    fn set_accepts_the_models_own_levels() {
        let terra = v(&["low", "medium", "high", "xhigh", "max", "ultra"]);
        assert!(accepts_effort("ultra", &terra), "模型自己声明的档位要认");
        assert!(accepts_effort("high", &v(&["low"])), "通用值照旧认");
        assert!(!accepts_effort("turbo", &terra));
    }
}
