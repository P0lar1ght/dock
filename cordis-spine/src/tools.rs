//! DSH `ctx.tools`: one registry. Capability plugins call [`Tools::register`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cordis::{Context, Disposable, Inject, Plugin, plugin};

use crate::acp;
use crate::agent_presets::{AgentPresets, blocked_tool_message};
use crate::names::{AGENT_PRESETS, JOBS, PERMISSIONS, PLAN_MODE, TOOLS, TOOLS_EXECUTE, TURN};
use crate::permissions::Permissions;
use crate::plan_mode::PlanMode;
use crate::runtime::BoxFuture;
use crate::turn::TurnControl;
use crate::types::{ToolCall, ToolResult, ToolSpec};
use crate::workspace;

tokio::task_local! {
    static EXEC_CTX: Context;
}

/// Context of the agent currently executing a tool (child isolate when nested).
pub fn exec_ctx() -> Option<Context> {
    EXEC_CTX.try_with(|c| c.clone()).ok()
}

/// Body stored by [`Tools::register`]. Owns what it needs; do not capture `Tools`.
pub type ToolBody = Arc<dyn Fn(ToolCall) -> BoxFuture<'static, ToolResult> + Send + Sync>;

struct Entry {
    spec: ToolSpec,
    body: ToolBody,
    kind: ExtraKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExtraKind {
    Regular,
    /// Running dynamic Cordis package — bypasses Agent preset allowlist.
    Dynamic,
    /// Live MCP tool — bypasses Agent preset allowlist while the server/tool is on.
    Mcp,
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
        self.register_inner(spec, body, ExtraKind::Regular)
    }

    /// Same as [`Self::register`], tagged so Agent preset allowlists still
    /// show and execute the tool while the dynamic package is running.
    pub fn register_dynamic(&self, spec: ToolSpec, body: ToolBody) -> cordis::Result<Disposable> {
        self.register_inner(spec, body, ExtraKind::Dynamic)
    }

    /// MCP tools: visible to the model even when the current Agent preset
    /// allowlist omits the `mcp_{server}__{tool}` name.
    pub fn register_mcp(&self, spec: ToolSpec, body: ToolBody) -> cordis::Result<Disposable> {
        self.register_inner(spec, body, ExtraKind::Mcp)
    }

    fn register_inner(
        &self,
        spec: ToolSpec,
        body: ToolBody,
        kind: ExtraKind,
    ) -> cordis::Result<Disposable> {
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
            extra.insert(name.clone(), Entry { spec, body, kind });
        }
        let extra = self.extra.clone();
        Ok(Disposable::from_fn(move || {
            extra.lock().unwrap().remove(&name);
        }))
    }

    pub fn is_dynamic(&self, name: &str) -> bool {
        self.extra
            .lock()
            .unwrap()
            .get(name)
            .is_some_and(|e| e.kind == ExtraKind::Dynamic)
    }

    pub fn is_mcp(&self, name: &str) -> bool {
        self.extra
            .lock()
            .unwrap()
            .get(name)
            .is_some_and(|e| e.kind == ExtraKind::Mcp)
    }

    fn bypasses_allowlist(&self, name: &str) -> bool {
        self.is_dynamic(name) || self.is_mcp(name)
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

    /// Specs the sampler should see: live `"tools"` intersected with the
    /// current agent preset allowlist. Inspect / the TUI catalog still use
    /// [`Tools::specs`].
    pub fn specs_for_model(&self) -> Vec<ToolSpec> {
        self.specs_for_model_on(&self.ctx)
    }

    pub fn specs_for_model_on(&self, exec: &Context) -> Vec<ToolSpec> {
        let specs = self.specs();
        match exec.get::<AgentPresets>(AGENT_PRESETS) {
            Some(presets) => specs
                .into_iter()
                .filter(|s| self.bypasses_allowlist(&s.name) || presets.allows(&s.name))
                .map(|mut spec| {
                    presets.bind_spawn_schema(&mut spec);
                    spec
                })
                .collect(),
            None => specs,
        }
    }

    pub async fn execute(&self, call: ToolCall) -> ToolResult {
        self.execute_on(&self.ctx, call).await
    }

    pub async fn execute_on(&self, exec: &Context, call: ToolCall) -> ToolResult {
        if let Some(presets) = exec.get::<AgentPresets>(AGENT_PRESETS) {
            if !self.bypasses_allowlist(&call.name) && !presets.allows(&call.name) {
                return finish(
                    exec,
                    ToolResult {
                        call_id: call.id,
                        name: call.name,
                        content: blocked_tool_message().into(),
                    },
                );
            }
        }
        if self.workspace {
            if let Some(plan) = exec.get::<PlanMode>(PLAN_MODE) {
                if plan.gated() && acp::blocked_in_plan(&call.name) {
                    let plan_file_edit =
                        crate::plan_mode::is_plan_file_edit(&call.name, &call.arguments);
                    if !plan_file_edit {
                        return finish(
                            exec,
                            ToolResult {
                                call_id: call.id,
                                name: call.name,
                                content: "计划模式已阻止：用户批准计划后使用 exit_plan_mode".into(),
                            },
                        );
                    }
                }
            }
            let plan_file_edit = exec.get::<PlanMode>(PLAN_MODE).is_some_and(|p| {
                p.gated() && crate::plan_mode::is_plan_file_edit(&call.name, &call.arguments)
            });
            if acp::needs_permission(&call.name) && !plan_file_edit {
                if let Some(perms) = exec.get::<Permissions>(PERMISSIONS) {
                    let summary = format!(
                        "{} {}",
                        call.name,
                        call.arguments.chars().take(120).collect::<String>()
                    );
                    if !perms.request(&call.name, &summary).await {
                        return finish(
                            exec,
                            ToolResult {
                                call_id: call.id,
                                name: call.name,
                                content: "权限被拒绝".into(),
                            },
                        );
                    }
                }
            }
        }

        let body = self
            .extra
            .lock()
            .unwrap()
            .get(&call.name)
            .map(|e| e.body.clone());
        let result = if let Some(body) = body {
            let exec = exec.clone();
            EXEC_CTX.scope(exec, body(call)).await
        } else {
            self.dispatch_local(exec, call).await
        };
        finish(exec, result)
    }

    async fn dispatch_local(&self, exec: &Context, call: ToolCall) -> ToolResult {
        if self.workspace && workspace::handles(&call.name) {
            let jobs = exec.get::<crate::jobs::Jobs>(JOBS);
            return workspace::execute_with(
                call,
                || {
                    exec.get::<TurnControl>(TURN)
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
