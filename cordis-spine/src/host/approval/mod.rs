//! 智能授权：权限的第三档（询问 / **智能授权** / 始终允许）。
//!
//! 要过权限门的调用在这一档下分三层：
//! 1. [`rules`]：固定规则。只读命令、项目里的编辑直接放行；硬清单上的一定问人。
//! 2. [`judge`]：规则拿不准的交给判断模型（`[approval] model`，没配用这一页的模型），
//!    它说安全才放行，理由挂在那次工具调用上（[`cordis_base::types::approval_notice`]）；
//!    它说有风险、或者没判成，就问人。
//! 3. 人。「始终允许 / 始终拒绝」过的照旧算数。
//!
//! 判断模型是一颗插件（[`approval_judge`]，named `"approvalJudge"`）：没挂它时第二层
//! 一律交给人。权限门在 `tools::registry` 里 live-lookup 它。

pub mod judge;
pub mod rules;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Inject, Plugin};

pub use judge::{Judge, JudgeRequest, Judged, LlmJudge};
pub use rules::{classify, Rule};

use crate::names::{APPROVAL_JUDGE, PLAN_MODE, SESSIONS};
use crate::session::log::Sessions;
use crate::tools::plan_mode::PlanMode;

/// 同一页记住的结论最多这么多条，满了整个清掉重来。
const CACHE_MAX: usize = 512;

/// Named `"approvalJudge"`：判断模型 + 按页记住的结论（同一页里一模一样的操作不再判）。
pub struct ApprovalJudge {
    judge: Arc<dyn Judge>,
    cache: Mutex<HashMap<String, Judged>>,
}

impl ApprovalJudge {
    /// 走 `"llm"` 的判断模型。
    pub fn llm() -> Self {
        Self::with(Arc::new(LlmJudge))
    }

    pub fn with(judge: Arc<dyn Judge>) -> Self {
        Self {
            judge,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// 按脚本判（测试、嵌入方）。
    pub fn scripted(f: impl Fn(&JudgeRequest) -> Judged + Send + Sync + 'static) -> Self {
        Self::with(Arc::new(Scripted(f)))
    }

    /// 判一次。`page` 是会话身份：结论按页记，别的页同样的操作照样再判。
    pub async fn judge(&self, exec: &Context, page: &str, request: JudgeRequest) -> Judged {
        let key = format!(
            "{page}\u{0}{}\u{0}{}\u{0}{}",
            request.tool,
            request.cwd.display(),
            request.action
        );
        if let Some(hit) = self.cache.lock().unwrap().get(&key) {
            return hit.clone();
        }
        let verdict = self.judge.judge(exec, &request).await;
        if !matches!(verdict, Judged::Failed(_)) {
            let mut cache = self.cache.lock().unwrap();
            if cache.len() >= CACHE_MAX {
                cache.clear();
            }
            cache.insert(key, verdict.clone());
        }
        verdict
    }
}

struct Scripted<F>(F);

impl<F> Judge for Scripted<F>
where
    F: Fn(&JudgeRequest) -> Judged + Send + Sync,
{
    fn judge<'a>(
        &'a self,
        _exec: &'a Context,
        request: &'a JudgeRequest,
    ) -> crate::agent::runtime::BoxFuture<'a, Judged> {
        let verdict = (self.0)(request);
        Box::pin(async move { verdict })
    }
}

/// 挂判断模型。没挂时智能授权的第二层一律交给人。
pub fn approval_judge() -> Plugin {
    plugin("approval-judge", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(APPROVAL_JUDGE, ApprovalJudge::llm())?))
    })
}

/// 智能授权档下这次调用怎么办。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Smart {
    /// 放行。`note` 是判断模型给的理由（规则放行的没有），要挂到工具卡上。
    Allow { note: Option<String> },
    /// 问人；这句写进审批框，说明为什么要问。
    Ask(String),
}

/// 按三层走一遍（「始终允许 / 拒绝」由调用方先查）。`exec` 是发起调用的 ctx（子代理是
/// 它自己的），`key` 是权限键。
pub async fn smart_verdict(exec: &Context, tool: &str, key: &str, arguments: &str) -> Smart {
    let cwd = crate::session::cwd::session_cwd(exec);
    match classify(tool, key, arguments, &cwd) {
        Rule::Allow(_) => Smart::Allow { note: None },
        Rule::Hard(why) => Smart::Ask(format!("高风险（{why}）")),
        Rule::Unsure => {
            let Some(judge) = exec.get::<ApprovalJudge>(APPROVAL_JUDGE) else {
                return Smart::Ask("没有判断模型".into());
            };
            // 「用户要做什么」取这一页（不是子代理自己）最近一条用户消息：子代理收到的
            // 任务是模型写的，可能掺了它读到的不可信内容。
            let page = exec
                .get::<PlanMode>(PLAN_MODE)
                .and_then(|p| p.page_sessions())
                .or_else(|| exec.get::<Sessions>(SESSIONS));
            let request = page
                .as_ref()
                .and_then(|s| s.last_user_text())
                .unwrap_or_default();
            let identity = page
                .as_ref()
                .map(|s| s.identity().to_string())
                .unwrap_or_default();
            let verdict = judge
                .judge(
                    exec,
                    &identity,
                    JudgeRequest {
                        tool: tool.to_string(),
                        action: arguments.to_string(),
                        cwd,
                        request,
                    },
                )
                .await;
            match verdict {
                Judged::Allow(reason) => Smart::Allow { note: Some(reason) },
                Judged::Ask(reason) | Judged::Failed(reason) => Smart::Ask(reason),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 同一页里一模一样的操作只判一次；没判成的不记，下次再判；别的页另判。
    #[tokio::test]
    async fn verdicts_are_remembered_per_page_and_failures_are_not() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let judge = ApprovalJudge::scripted(move |req| {
            seen.fetch_add(1, Ordering::SeqCst);
            if req.action.contains("flaky") {
                Judged::Failed("超时".into())
            } else {
                Judged::Allow("跑测试".into())
            }
        });
        let ctx = Context::new();
        let req = |action: &str| JudgeRequest {
            tool: "bash".into(),
            action: action.into(),
            cwd: "/p".into(),
            request: String::new(),
        };
        for _ in 0..3 {
            assert_eq!(
                judge.judge(&ctx, "main", req("cargo test")).await,
                Judged::Allow("跑测试".into())
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        judge.judge(&ctx, "main#2", req("cargo test")).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "别的页另判");
        judge.judge(&ctx, "main", req("flaky")).await;
        judge.judge(&ctx, "main", req("flaky")).await;
        assert_eq!(calls.load(Ordering::SeqCst), 4, "没判成的不记");
    }
}
