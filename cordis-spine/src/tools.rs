//! DSH `ctx.tools`: one registry. Capability plugins call [`Tools::register`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};

use crate::acp;
use crate::names::{JOBS, PERMISSIONS, PLAN_MODE, TOOLS, TOOLS_EXECUTE, TURN};
use crate::permissions::Permissions;
use crate::plan_mode::PlanMode;
use crate::runtime::BoxFuture;
use crate::turn::TurnControl;
use crate::types::{ToolCall, ToolResult, ToolSpec};
use crate::workspace;

/// Body stored by [`Tools::register`]. Owns what it needs; do not capture `Tools`.
pub type ToolBody = Arc<dyn Fn(ToolCall) -> BoxFuture<'static, ToolResult> + Send + Sync>;

struct Entry {
    spec: ToolSpec,
    body: ToolBody,
}

/// Named `tools` service. Echo backend for tests; workspace backend for the app.
/// Extra capabilities register here (DSH `ctx.tools.register`), they do not
/// provide a second `tools.*` dispatcher.
#[derive(Clone)]
pub struct Tools {
    ctx: Context,
    workspace: bool,
    extra: Arc<Mutex<HashMap<String, Entry>>>,
}

impl Tools {
    pub fn echo(ctx: Context) -> Self {
        Self {
            ctx,
            workspace: false,
            extra: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn workspace(ctx: Context) -> Self {
        Self {
            ctx,
            workspace: true,
            extra: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a model-facing tool. Duplicate names in this layer throw.
    /// Workspace builtins cannot be shadowed. Disposed with the calling fiber
    /// when the returned handle is owned (via `ctx.effect` / apply return).
    pub fn register(&self, spec: ToolSpec, body: ToolBody) -> cordis::Result<Disposable> {
        let name = spec.name.clone();
        if self.workspace && workspace::handles(&name) {
            return Err(cordis::Error::plugin(format!(
                "cannot shadow workspace tool {name}"
            )));
        }
        {
            let mut extra = self.extra.lock().unwrap();
            if extra.contains_key(&name) {
                return Err(cordis::Error::plugin(format!("duplicate tool {name}")));
            }
            extra.insert(name.clone(), Entry { spec, body });
        }
        let extra = self.extra.clone();
        Ok(Disposable::from_fn(move || {
            extra.lock().unwrap().remove(&name);
        }))
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut specs = Vec::new();
        if self.workspace {
            specs.extend(workspace::specs());
        }
        let extra = self.extra.lock().unwrap();
        let mut names: Vec<_> = extra.keys().cloned().collect();
        names.sort();
        for name in names {
            if let Some(entry) = extra.get(&name) {
                specs.push(entry.spec.clone());
            }
        }
        specs
    }

    pub async fn execute(&self, call: ToolCall) -> ToolResult {
        if self.workspace {
            if let Some(plan) = self.ctx.get::<PlanMode>(PLAN_MODE) {
                if plan.active() && acp::blocked_in_plan(&call.name) {
                    return finish(
                        &self.ctx,
                        ToolResult {
                            call_id: call.id,
                            name: call.name,
                            content: "blocked in plan mode: use exit_plan_mode after the user approves the plan".into(),
                        },
                    );
                }
            }
            if acp::needs_permission(&call.name) {
                if let Some(perms) = self.ctx.get::<Permissions>(PERMISSIONS) {
                    let summary = format!(
                        "{} {}",
                        call.name,
                        call.arguments.chars().take(120).collect::<String>()
                    );
                    if !perms.request(&call.name, &summary).await {
                        return finish(
                            &self.ctx,
                            ToolResult {
                                call_id: call.id,
                                name: call.name,
                                content: "permission denied".into(),
                            },
                        );
                    }
                }
            }
        }

        let body = self.extra.lock().unwrap().get(&call.name).map(|e| e.body.clone());
        let result = if let Some(body) = body {
            body(call).await
        } else {
            self.dispatch_local(call).await
        };
        finish(&self.ctx, result)
    }

    async fn dispatch_local(&self, call: ToolCall) -> ToolResult {
        if self.workspace && workspace::handles(&call.name) {
            let ctx = self.ctx.clone();
            let jobs = ctx.get::<crate::jobs::Jobs>(JOBS);
            return workspace::execute_with(
                call,
                || {
                    ctx.get::<TurnControl>(TURN)
                        .is_some_and(|t| t.is_cancelled())
                },
                jobs.as_deref(),
            );
        }
        ToolResult {
            call_id: call.id,
            name: call.name,
            content: call.arguments,
        }
    }
}

fn finish(ctx: &Context, result: ToolResult) -> ToolResult {
    ctx.waterfall(TOOLS_EXECUTE, result.clone(), move || result)
}

/// Own registrations on the calling fiber (DSH register-dispose).
pub fn own_registered(ctx: &Context, disposers: Vec<Disposable>) -> cordis::Result<()> {
    ctx.effect("tools.register", |scope| {
        for d in disposers {
            scope.own(d);
        }
        Ok(())
    })?;
    Ok(())
}

pub fn tool_result(call: ToolCall, content: impl Into<String>) -> ToolResult {
    ToolResult {
        call_id: call.id,
        name: call.name,
        content: content.into(),
    }
}

pub fn tools() -> Plugin {
    plugin("tools", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(TOOLS, Tools::echo(ctx.clone()))?))
    })
}

pub fn workspace_tools() -> Plugin {
    plugin("tools", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(TOOLS, Tools::workspace(ctx.clone()))?))
    })
}
