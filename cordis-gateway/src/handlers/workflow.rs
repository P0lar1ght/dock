//! `workflow/*`：一个线程起的工作流 run。只认起它的那一页（`threadId`），别的线程
//! 拿着 run id 也查不到、停不了。run 只在内存里，Dock 重启后没有。

use serde_json::{json, Value};

use cordis_spine::{Workflows, WORKFLOWS};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;
use crate::threads;
use crate::workflows;

/// `workflow/list { threadId }` → `{ runs }`，先起的在前。
pub fn list(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let runs: Vec<Value> = workflows::runs_of(gateway.ctx(), &page.identity)
        .iter()
        .map(workflows::info)
        .collect();
    Ok(json!({ "runs": runs }))
}

/// `workflow/stop { threadId, runId }`：停下一条在跑的 run（连同它派的子代理）。
/// 已经结束的什么都不做（`stopped: false`）。
pub fn stop(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let run_id = params
        .get("runId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("runId 不能为空"))?;
    let run = workflows::run(gateway.ctx(), run_id)
        .filter(|r| r.page == page.identity)
        .ok_or_else(|| RpcError::app("not_found", format!("这个线程没有工作流 {run_id}")))?;
    let wf = gateway
        .ctx()
        .get::<Workflows>(WORKFLOWS)
        .ok_or_else(|| RpcError::app("unavailable", "workflows service is not mounted"))?;
    let stopped = wf.stop(&run.run_id).is_some();
    Ok(json!({ "ok": true, "stopped": stopped }))
}
