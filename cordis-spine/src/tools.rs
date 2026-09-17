//! DSH `ctx.tools`: one registry. Capability plugins call [`Tools::register`].

use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use indexmap::IndexMap;

use crate::agent_presets::{blocked_tool_message, AgentPresets, MINIMAL_PRESET_ID};
use crate::names::{AGENT_PRESETS, JOBS, PERMISSIONS, PLAN_MODE, TOOLS, TOOLS_EXECUTE, TURN};
use crate::permissions::Permissions;
use crate::plan_mode::PlanMode;
use crate::runtime::BoxFuture;
use crate::turn::TurnControl;
use crate::workspace;
use cordis_base::acp;
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

tokio::task_local! {
    static EXEC_CTX: Context;
}

/// Context of the agent currently running (child isolate when nested) — set
/// around tool execution and around the `agent/step-start` waterfall.
///
/// waterfall 的 handler 只拿得到载荷，拿不到调用方 ctx（[`crate::StepStart`] 的
/// `identity` 就是为此才挂在载荷上的）。需要**子代理自己那份**会话 / 设置的
/// handler 走这里。
pub fn exec_ctx() -> Option<Context> {
    EXEC_CTX.try_with(|c| c.clone()).ok()
}

/// 在 `f` 执行期间挂上「当前正在跑的 agent 的 ctx」。同步版的
/// [`EXEC_CTX`] scope，给循环里那几个同步 waterfall 用。
pub(crate) fn with_exec_ctx<R>(ctx: &Context, f: impl FnOnce() -> R) -> R {
    EXEC_CTX.sync_scope(ctx.clone(), f)
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
    /// Infrequent local tool — hidden from the sampler, discovered via
    /// `search_tool` and called with `use_tool`. Allowlist still applies.
    Deferred,
}

/// Named `tools` service. Echo backend for tests; workspace backend for the app.
/// Extra capabilities register here (DSH `ctx.tools.register`), they do not
/// provide a second `tools.*` dispatcher.
#[derive(Clone)]
pub struct Tools {
    ctx: Context,
    workspace: bool,
    extra: Arc<Mutex<IndexMap<String, Entry>>>,
}

impl Tools {
    pub fn echo(ctx: Context) -> Self {
        Self {
            ctx,
            workspace: false,
            extra: Arc::new(Mutex::new(IndexMap::new())),
        }
    }

    pub fn workspace(ctx: Context) -> Self {
        Self {
            ctx,
            workspace: true,
            extra: Arc::new(Mutex::new(IndexMap::new())),
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

    /// Infrequent local tools: registered for `search_tool` / `use_tool`,
    /// omitted from the sampler tools array and occupancy. Execute still
    /// applies Agent preset allowlists (unlike MCP extras).
    pub fn register_deferred(&self, spec: ToolSpec, body: ToolBody) -> cordis::Result<Disposable> {
        self.register_inner(spec, body, ExtraKind::Deferred)
    }

    /// MCP extras: registered for `use_tool` dispatch, omitted from the
    /// sampler tools array and occupancy. Execute still bypasses the Agent
    /// preset allowlist so `mcp_{server}__{tool}` can run after discovery.
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
        Ok(self.disposable_for(&name))
    }

    /// Replace an existing MCP tool in place so `specs()` order (and the
    /// sampler tools prefix) does not move. Returns a new unregister handle;
    /// drop the previous handle without disposing.
    pub fn patch_mcp(&self, spec: ToolSpec, body: ToolBody) -> Option<Disposable> {
        let name = spec.name.clone();
        {
            let mut extra = self.extra.lock().unwrap();
            let entry = extra.get_mut(&name)?;
            if entry.kind != ExtraKind::Mcp {
                return None;
            }
            entry.spec = spec;
            entry.body = body;
        }
        Some(self.disposable_for(&name))
    }

    fn disposable_for(&self, name: &str) -> Disposable {
        let extra = self.extra.clone();
        let name = name.to_string();
        Disposable::from_fn(move || {
            extra.lock().unwrap().shift_remove(&name);
        })
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

    pub fn is_deferred(&self, name: &str) -> bool {
        self.extra
            .lock()
            .unwrap()
            .get(name)
            .is_some_and(|e| e.kind == ExtraKind::Deferred)
    }

    /// Hidden from the sampler: MCP extras and infrequent local tools.
    pub fn is_hidden(&self, name: &str) -> bool {
        self.is_mcp(name) || self.is_deferred(name)
    }

    fn bypasses_allowlist(&self, name: &str) -> bool {
        self.is_dynamic(name) || self.is_mcp(name)
    }

    /// `search_tool` / `use_tool` stay on the sampler like Grok builtins.
    /// Overlay YAML that predates them must not hide the discovery surface.
    /// `minimal` stays workspace-only.
    fn mcp_meta_visible(&self, exec: &Context, name: &str) -> bool {
        if !is_mcp_meta_tool(name) {
            return false;
        }
        match exec.get::<AgentPresets>(AGENT_PRESETS) {
            Some(presets) => presets.current_id() != MINIMAL_PRESET_ID,
            None => true,
        }
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut specs = Vec::new();
        if self.workspace {
            specs.extend(workspace::specs());
        }
        let extra = self.extra.lock().unwrap();
        let mut regular: Vec<ToolSpec> = extra
            .values()
            .filter(|e| !matches!(e.kind, ExtraKind::Mcp | ExtraKind::Deferred))
            .map(|e| e.spec.clone())
            .collect();
        regular.sort_by(|a, b| a.name.cmp(&b.name));
        specs.extend(regular);
        // Hidden from the sampler (`specs_for_model`); still registered for
        // `use_tool` dispatch. Occupancy does not count these schemas.
        for entry in extra.values() {
            if matches!(entry.kind, ExtraKind::Mcp | ExtraKind::Deferred) {
                specs.push(entry.spec.clone());
            }
        }
        specs
    }

    /// Specs the sampler should see: live `"tools"` minus hidden extras
    /// (MCP + infrequent local; Grok `tool_definitions_builtins_only` plus
    /// Dock's deferred locals), then the current agent preset allowlist.
    /// Hidden tools stay registered for `use_tool` dispatch.
    /// Inspect / the TUI catalog still use [`Tools::specs`]. Occupancy
    /// counts [`Self::specs_for_model`] only.
    pub fn specs_for_model(&self) -> Vec<ToolSpec> {
        self.specs_for_model_on(&self.ctx)
    }

    pub fn specs_for_model_on(&self, exec: &Context) -> Vec<ToolSpec> {
        let specs: Vec<ToolSpec> = self
            .specs()
            .into_iter()
            .filter(|s| !self.is_hidden(&s.name))
            .collect();
        match exec.get::<AgentPresets>(AGENT_PRESETS) {
            Some(presets) => {
                let mut out: Vec<ToolSpec> = specs
                    .into_iter()
                    .filter(|s| {
                        self.is_dynamic(&s.name)
                            || self.mcp_meta_visible(exec, &s.name)
                            || presets.allows(&s.name)
                    })
                    .map(|mut spec| {
                        presets.bind_spawn_schema(&mut spec);
                        spec
                    })
                    .collect();
                // 到处都允许的排前面，会被某个角色过滤掉的排后面。排序键只看整份
                // 名册、与当前预设无关，所以**子代理那张表是主会话那张表的真前缀**：
                // 工具表排在整份 prompt 的最前面，以前从第一个被过滤掉的工具起就
                // 分叉，子代理每次冷启动都要把公共头重付一遍。稳定排序，组内顺序
                // 不变（内置工具仍在最前，其余仍按名字）。
                let universal = presets.universal_tools();
                out.sort_by_key(|s| !universal.allows_everywhere(&s.name));
                out
            }
            None => specs,
        }
    }

    pub async fn execute(&self, call: ToolCall) -> ToolResult {
        self.execute_on(&self.ctx, call).await
    }

    pub async fn execute_on(&self, exec: &Context, call: ToolCall) -> ToolResult {
        if let Some(presets) = exec.get::<AgentPresets>(AGENT_PRESETS) {
            if !self.bypasses_allowlist(&call.name)
                && !self.mcp_meta_visible(exec, &call.name)
                && !presets.allows(&call.name)
            {
                return finish(
                    exec,
                    ToolResult {
                        call_id: call.id,
                        name: call.name,
                        content: blocked_tool_message().into(),
                        ..Default::default()
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
                                ..Default::default()
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
                                ..Default::default()
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
        } else if looks_like_mcp_name(&call.name) {
            ToolResult {
                call_id: call.id,
                name: call.name,
                content: "MCP 工具未启用或已关闭".into(),
                ..Default::default()
            }
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
            )
            .await;
        }
        ToolResult {
            call_id: call.id,
            name: call.name,
            content: call.arguments,
            ..Default::default()
        }
    }
}

fn finish(ctx: &Context, result: ToolResult) -> ToolResult {
    ctx.waterfall(TOOLS_EXECUTE, result.clone(), move || result)
}

fn looks_like_mcp_name(name: &str) -> bool {
    name.starts_with("mcp_") && name.contains("__")
}

fn is_mcp_meta_tool(name: &str) -> bool {
    name == "search_tool" || name == "use_tool"
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
        ..Default::default()
    }
}

pub fn tool_result_with_images(
    call: ToolCall,
    content: impl Into<String>,
    images: Vec<cordis_base::types::UserImage>,
) -> ToolResult {
    ToolResult {
        call_id: call.id,
        name: call.name,
        content: content.into(),
        images: crate::tool_images::cap_images(images),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn stub_body() -> ToolBody {
        Arc::new(|call| Box::pin(async move { tool_result(call, "ok") }))
    }

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: name.into(),
            parameters_json: r#"{"type":"object"}"#.into(),
        }
    }

    fn names(tools: &Tools) -> Vec<String> {
        tools.specs().into_iter().map(|s| s.name).collect()
    }

    /// 子代理那张工具表必须是主会话那张的**真前缀**。
    ///
    /// tools 排在整份 prompt 的最前面。角色白名单过滤掉的项要是散落在数组中间，
    /// 序列化出来的 tools 就从第一个缺口处分叉，后面整段（system + 全部消息）都得
    /// 重算——这正是每个子代理第一次调用都要整份满价的原因。排序键只看整份名册，
    /// 与当前预设无关，父子两边才算得出同一个分组。
    #[test]
    fn a_subagent_tool_table_is_a_prefix_of_the_main_one() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx.clone());
        // 名字刻意交错：按名字排序时，共用的与专属的会插花。
        for name in ["alpha", "bravo", "mike", "november", "zulu"] {
            tools.register(spec(name), stub_body()).unwrap();
        }
        let shared = ["alpha", "mike", "zulu"];

        let mut mode = crate::agent_presets::AgentPreset::new("code");
        mode.tools = Some(
            ["alpha", "bravo", "mike", "november", "zulu"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );
        mode.agents.insert(
            "explore".into(),
            crate::agent_presets::SubagentDef {
                tools: Some(shared.iter().map(|s| s.to_string()).collect()),
                ..Default::default()
            },
        );
        let parent_presets = AgentPresets::overlay(mode.clone());
        let _p = ctx.provide(AGENT_PRESETS, parent_presets.clone()).unwrap();

        let child_ctx = ctx.isolate("agentPresets");
        let _c = child_ctx
            .provide(
                AGENT_PRESETS,
                AgentPresets::overlay_with_order(
                    mode.agents["explore"].to_preset("explore"),
                    parent_presets.universal_tools(),
                ),
            )
            .unwrap();

        let names = |exec: &Context| -> Vec<String> {
            tools
                .specs_for_model_on(exec)
                .into_iter()
                .map(|s| s.name)
                .collect()
        };
        let main = names(&ctx);
        let child = names(&child_ctx);
        let common = main.iter().zip(&child).take_while(|(a, b)| a == b).count();

        assert_eq!(child, shared, "子代理看得见的还是那三个，没有多也没有少");
        assert_eq!(
            common,
            child.len(),
            "子代理的表必须整个是主会话的前缀：main={main:?} child={child:?}"
        );
        assert_eq!(&main[..3], &shared[..], "共用的排在最前：{main:?}");

        // 没有这条排序时是什么样：按名字排，第二项就分叉了。
        let mut by_name = main.clone();
        by_name.sort();
        let old_common = by_name
            .iter()
            .zip(&child)
            .take_while(|(a, b)| a == b)
            .count();
        assert_eq!(old_common, 1, "旧顺序下公共前缀只有一项：{by_name:?}");
    }

    #[test]
    fn mcp_tools_append_without_reordering_regular() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx);
        tools.register(spec("zeta"), stub_body()).unwrap();
        tools.register(spec("alpha"), stub_body()).unwrap();
        let before = names(&tools);
        assert_eq!(before, ["alpha", "zeta"]);

        let first = tools.register_mcp(spec("mcp_s__t"), stub_body()).unwrap();
        let with_one = names(&tools);
        assert_eq!(&with_one[..before.len()], before.as_slice());
        assert_eq!(with_one.last().map(String::as_str), Some("mcp_s__t"));

        let second = tools.register_mcp(spec("mcp_s__u"), stub_body()).unwrap();
        let with_two = names(&tools);
        assert_eq!(&with_two[..with_one.len()], with_one.as_slice());
        assert_eq!(with_two.last().map(String::as_str), Some("mcp_s__u"));

        let patched = tools
            .patch_mcp(
                ToolSpec {
                    name: "mcp_s__t".into(),
                    description: "patched".into(),
                    parameters_json: r#"{"type":"object"}"#.into(),
                },
                stub_body(),
            )
            .unwrap();
        drop(first);
        let after_patch = names(&tools);
        assert_eq!(after_patch, with_two);
        assert_eq!(
            tools
                .specs()
                .into_iter()
                .find(|s| s.name == "mcp_s__t")
                .unwrap()
                .description,
            "patched"
        );

        patched.dispose_sync();
        let after_remove = names(&tools);
        assert_eq!(after_remove, ["alpha", "zeta", "mcp_s__u"]);
        second.dispose_sync();
        assert_eq!(names(&tools), before);
    }

    #[test]
    fn specs_for_model_omits_hidden_extras() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx);
        tools.register(spec("alpha"), stub_body()).unwrap();
        let _mcp = tools.register_mcp(spec("mcp_s__t"), stub_body()).unwrap();
        let _def = tools
            .register_deferred(spec("scheduler_create"), stub_body())
            .unwrap();
        let model: Vec<String> = tools
            .specs_for_model()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(model, ["alpha"]);
        assert!(tools.specs().iter().any(|s| s.name == "mcp_s__t"));
        assert!(tools.specs().iter().any(|s| s.name == "scheduler_create"));
        assert!(tools.is_hidden("scheduler_create"));
        assert!(tools.is_deferred("scheduler_create"));
    }

    #[tokio::test]
    async fn disabled_mcp_name_does_not_echo() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx.clone());
        let out = tools
            .execute_on(
                &ctx,
                ToolCall {
                    id: "1".into(),
                    name: "mcp_s__gone".into(),
                    arguments: "{}".into(),
                },
            )
            .await;
        assert_eq!(out.content, "MCP 工具未启用或已关闭");
    }
}
