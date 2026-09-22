//! DSH `ctx.tools`: one registry. Capability plugins call [`Tools::register`].

use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use indexmap::IndexMap;

use crate::agent::capability::CapabilityMode;
use crate::agent::presets::{blocked_tool_message, AgentPresets, MINIMAL_PRESET_ID};
use crate::agent::runtime::BoxFuture;
use crate::agent::turn::TurnControl;
use crate::host::permissions::Permissions;
use crate::names::{
    AGENT_PRESETS, CAPABILITY, JOBS, PERMISSIONS, PLAN_MODE, SESSIONS, TOOLS, TOOLS_EXECUTE,
    TOOLS_PRE_EXECUTE, TURN,
};
use crate::tools::plan_mode::PlanMode;
use crate::tools::workspace;
use cordis_base::acp;
use cordis_base::types::{PreExecute, ToolCall, ToolResult, ToolSpec};

tokio::task_local! {
    static EXEC_CTX: Context;
}

/// 这颗工具被本会话的能力档位挡住了吗？
///
/// 没挂 `"capability"` 的会话（主会话、以及没指定档位的子代理）一律放行——档位
/// 是**收窄**，不是新的准入条件。
fn capability_denies(exec: &Context, name: &str) -> bool {
    match exec.get::<CapabilityMode>(CAPABILITY) {
        Some(mode) => !mode.allows(name),
        None => false,
    }
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

/// 异步版 [`with_exec_ctx`]。工具体本身跑在 `Tools::execute_on` 的 scope 里；
/// 传输层测试要复现「两页各调一次」时走这里。
#[cfg(test)]
pub(crate) async fn with_exec_ctx_async<T>(
    ctx: Context,
    fut: impl std::future::Future<Output = T>,
) -> T {
    EXEC_CTX.scope(ctx, fut).await
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

    /// 这颗工具在当前预设的工具集之外吗？
    ///
    /// 预设的工具集是**边界**，不是「模型能叫什么」的过滤器，所以 `execute_on` 查
    /// 两次：入站查一次，挡住模型自己越界；`tools/pre-execute` 改道之后再查一次，
    /// 挡住插件替它越界。少了后一次，一颗只读子代理的 `read_file` 就能被任意
    /// handler 改写成 `bash` 跑出去——权限门跟着改写后的名字走，补不上这个洞，
    /// 何况子代理那边常常压根没人应答询问。
    fn outside_allowlist(&self, exec: &Context, name: &str) -> bool {
        // 能力档位排在最前，**不吃 `bypasses_allowlist` 的豁免**：MCP 与动态包
        // 工具绕过预设允许名单是有意的（服务端上线就该能调），但绕不过"这次
        // 委派只准读"——否则一颗只读子代理换颗 MCP 工具就能写盘。
        if capability_denies(exec, name) {
            return true;
        }
        let Some(presets) = exec.get::<AgentPresets>(AGENT_PRESETS) else {
            return false;
        };
        !self.bypasses_allowlist(name)
            && !self.mcp_meta_visible(exec, name)
            && !presets.allows(name)
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
            .filter(|s| !self.is_hidden(&s.name) && !capability_denies(exec, &s.name))
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
        if self.outside_allowlist(exec, &call.name) {
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
        // 改写 / 改道 / 拒绝的位点。**夹在允许名单的两次检查中间**：入站那次挡模型
        // 越界，改道之后那次挡插件替它越界（见 `outside_allowlist`）。**在计划门与
        // 权限门之前**：改道必须换一套门——`bash` 改成 `read_file` 之后就该走
        // read-only 路径、不弹权限；在门之后改，它已经按 `bash` 弹过一次询问了，
        // 那毫无意义。
        let identity = exec
            .get::<crate::session::log::Sessions>(SESSIONS)
            .map(|s| s.identity().to_string())
            .unwrap_or_else(|| cordis_base::types::ROOT_IDENTITY.to_string());
        let pre = {
            let seed = PreExecute::new(&call.name, &call.arguments, identity);
            let fallback = seed.clone();
            exec.waterfall(TOOLS_PRE_EXECUTE, seed, move || fallback)
        };
        // `ToolResult.name` 一律报**实际（将要）跑的那颗**，改道后就是新名字。回喂
        // 模型的 tool 消息只带 `tool_call_id`（见 `llm/http`），名字是给 UI 和日志
        // 看的——那里要看见的正是「真跑的是什么」。
        if let Some(reason) = pre.denial() {
            return finish(
                exec,
                ToolResult {
                    call_id: call.id,
                    name: pre.name().to_string(),
                    content: reason.to_string(),
                    ..Default::default()
                },
            );
        }
        if pre.redirected() && self.outside_allowlist(exec, pre.name()) {
            return finish(
                exec,
                ToolResult {
                    call_id: call.id,
                    name: pre.name().to_string(),
                    content: blocked_tool_message().into(),
                    ..Default::default()
                },
            );
        }
        // `call_id` 不动：模型按它配对 tool_call 与结果，改了这一轮就对不上。
        let call = ToolCall {
            id: call.id,
            name: pre.name().to_string(),
            arguments: pre.arguments().to_string(),
        };

        if self.workspace {
            let plan_expected = crate::tools::plan_mode::expected_plan_path(
                exec.get::<crate::session::log::Sessions>(SESSIONS)
                    .as_deref(),
            );
            if let Some(plan) = exec.get::<PlanMode>(PLAN_MODE) {
                if plan.gated() && acp::blocked_in_plan(&call.name) {
                    let plan_file_edit = crate::tools::plan_mode::is_plan_file_edit(
                        &call.name,
                        &call.arguments,
                        &plan_expected,
                    );
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
                p.gated()
                    && crate::tools::plan_mode::is_plan_file_edit(
                        &call.name,
                        &call.arguments,
                        &plan_expected,
                    )
            });
            if acp::needs_permission(&call.name) && !plan_file_edit {
                if let Some(perms) = exec.get::<Permissions>(PERMISSIONS) {
                    let summary = permission_summary(&call.name, &call.arguments);
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
        // Keep `EXEC_CTX` alive across `tools/execute`. That waterfall has no
        // identity of its own, and every tab's listener still runs.
        if let Some(body) = body {
            let exec = exec.clone();
            return EXEC_CTX
                .scope(exec.clone(), async move {
                    let result = body(call).await;
                    finish(&exec, result)
                })
                .await;
        }
        let result = if looks_like_mcp_name(&call.name) {
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
            let jobs = exec.get::<crate::tools::jobs::Jobs>(JOBS);
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

/// Human-readable permission prompt body.
///
/// Default path truncates raw args to 120 chars (loses CUA target / role+label).
/// MCP tools — especially `mcp_cua-driver__*` — get a structured summary that
/// highlights action + role/label and hides long tokens.
fn permission_summary(name: &str, arguments: &str) -> String {
    if looks_like_mcp_name(name) {
        return format!("{name} {}", summarize_mcp_args(name, arguments));
    }
    format!("{name} {}", arguments.chars().take(120).collect::<String>())
}

fn summarize_mcp_args(name: &str, arguments: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return redact_long_tokens(arguments).chars().take(120).collect();
    };
    let obj = v.as_object();
    let mut parts: Vec<String> = Vec::new();
    let cua = name.starts_with("mcp_cua-driver__");

    let action = obj
        .and_then(|o| {
            o.get("action")
                .or_else(|| o.get("type"))
                .or_else(|| o.get("method"))
                .and_then(|x| x.as_str())
        })
        .map(str::to_string)
        .or_else(|| name.rsplit("__").next().map(str::to_string));
    if let Some(action) = action {
        parts.push(action);
    }

    if let Some(obj) = obj {
        for key in [
            "role", "label", "name", "title", "app", "window", "text", "value",
        ] {
            if let Some(val) = obj
                .get(key)
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
            {
                let short = if val.chars().count() > 40 {
                    format!("{}…", val.chars().take(40).collect::<String>())
                } else {
                    val.to_string()
                };
                parts.push(format!("{key}={short}"));
            }
        }
        if cua {
            if let Some(tok) = obj.get("element_token").and_then(|x| x.as_str()) {
                let head: String = tok.chars().take(12).collect();
                parts.push(format!("element_token={head}…"));
            }
            if let (Some(x), Some(y)) = (
                obj.get("x").and_then(|v| v.as_i64()),
                obj.get("y").and_then(|v| v.as_i64()),
            ) {
                parts.push(format!("@({x},{y})"));
            }
        }
    }

    if parts.is_empty() {
        return redact_long_tokens(arguments).chars().take(120).collect();
    }
    parts.join(" ")
}

fn redact_long_tokens(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            out.push(c);
            continue;
        }
        out.push(c);
        let mut lit = String::new();
        while let Some(&n) = chars.peek() {
            chars.next();
            lit.push(n);
            if n == '"' {
                break;
            }
            if n == '\\' {
                if let Some(&esc) = chars.peek() {
                    chars.next();
                    lit.push(esc);
                }
            }
        }
        let inner = lit.trim_end_matches('"');
        if inner.len() > 48
            && inner
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':' | '.'))
        {
            let head: String = inner.chars().take(12).collect();
            out.push_str(&head);
            out.push('…');
            out.push('"');
        } else {
            out.push_str(&lit);
        }
    }
    out
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
        images: crate::tools::tool_images::cap_images(images),
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

        let mut mode = crate::agent::presets::AgentPreset::new("code");
        mode.tools = Some(
            ["alpha", "bravo", "mike", "november", "zulu"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );
        mode.agents.insert(
            "explore".into(),
            crate::agent::presets::SubagentDef {
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

#[cfg(test)]
mod pre_execute_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn echo_body() -> ToolBody {
        // 回显参数：测试靠它看到工具体**实际**收到的是什么。
        Arc::new(|call| {
            Box::pin(async move {
                let args = call.arguments.clone();
                tool_result(call, args)
            })
        })
    }

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: name.into(),
            parameters_json: r#"{"type":"object"}"#.into(),
        }
    }

    fn call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: args.into(),
        }
    }

    /// handler 改了参数，工具体收到的就是改后的。
    #[tokio::test]
    async fn a_handler_can_rewrite_the_arguments() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx.clone());
        tools.register(spec("probe"), echo_body()).unwrap();
        let _h = ctx
            .on_waterfall(TOOLS_PRE_EXECUTE, |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                next.rewrite_args("改后的参数");
                next
            })
            .unwrap();

        let out = tools.execute(call("probe", "原始参数")).await;
        assert_eq!(out.content, "改后的参数");
    }

    /// 改道：跑的是新工具，但 `call_id` 不变——模型按它配对 tool_call 与结果。
    #[tokio::test]
    async fn a_redirect_runs_the_other_tool_and_keeps_the_call_id() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx.clone());
        tools.register(spec("slow"), echo_body()).unwrap();
        tools
            .register(
                spec("fast"),
                Arc::new(|call| Box::pin(async move { tool_result(call, "fast 跑了") })),
            )
            .unwrap();
        let _h = ctx
            .on_waterfall(TOOLS_PRE_EXECUTE, |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                if next.name() == "slow" {
                    next.rewrite("fast", "{}");
                }
                next
            })
            .unwrap();

        let out = tools.execute(call("slow", "{}")).await;
        assert_eq!(out.content, "fast 跑了");
        assert_eq!(out.call_id, "c1", "改道不能动 call_id");
        assert_eq!(out.name, "fast", "结果要报实际跑的那颗");
    }

    /// 拒绝：工具体一次都不该跑，理由原样给模型。
    #[tokio::test]
    async fn a_denial_never_reaches_the_tool_body() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx.clone());
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        tools
            .register(
                spec("probe"),
                Arc::new(move |call| {
                    flag.store(true, Ordering::SeqCst);
                    Box::pin(async move { tool_result(call, "跑过了") })
                }),
            )
            .unwrap();
        let _h = ctx
            .on_waterfall(TOOLS_PRE_EXECUTE, |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                next.deny("这次不行");
                next
            })
            .unwrap();

        let out = tools.execute(call("probe", "{}")).await;
        assert_eq!(out.content, "这次不行");
        assert!(!ran.load(Ordering::SeqCst), "被拒的调用不该跑工具体");
    }

    /// 拒绝是**单调**的：后面的 handler 掀不翻前面的拒绝，否则「这条策略成不成立」
    /// 就由插件挂载顺序决定了。
    #[tokio::test]
    async fn a_later_handler_cannot_lift_a_denial() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx.clone());
        tools.register(spec("probe"), echo_body()).unwrap();
        tools.register(spec("other"), echo_body()).unwrap();
        let _deny = ctx
            .on_waterfall(TOOLS_PRE_EXECUTE, |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                next.deny("先拒了");
                next
            })
            .unwrap();
        let _undo = ctx
            .on_waterfall(TOOLS_PRE_EXECUTE, |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                // 试图改道绕过拒绝
                next.rewrite("other", "{}");
                next.rewrite_args("翻案");
                next
            })
            .unwrap();

        let out = tools.execute(call("probe", "{}")).await;
        assert_eq!(out.content, "先拒了");
    }

    /// 载荷带得出「模型原本叫的是什么」和「实际会跑什么」，两者都要。
    #[test]
    fn the_payload_keeps_both_the_called_and_the_effective_name() {
        let mut pre = PreExecute::new("bash", "{\"command\":\"ls\"}", "main");
        assert!(!pre.redirected());
        pre.rewrite("list_dir", "{}");
        assert_eq!(pre.called, "bash", "模型叫的那个名字要留着");
        assert_eq!(pre.name(), "list_dir");
        assert_eq!(pre.arguments(), "{}");
        assert!(pre.redirected());
    }

    /// 多个 handler 的改动按**逆挂载顺序**落地，钉住这个事实。
    ///
    /// waterfall 是先递归到底、回程再改：按本仓标准写法（先 `args.next()` 拿下游
    /// 结果，再在它身上改），最内层——也就是最后挂的那个——先改。内核的
    /// `EventOptions` 只有 `prepend` / `global` / `once`，没有 order 键，所以这不是
    /// 能配的，是写法定的。真正危险的那种顺序依赖（一个放行另一个拒绝）由 `deny`
    /// 的单调性堵住，见 `a_later_handler_cannot_lift_a_denial`。
    #[tokio::test]
    async fn handlers_compose_in_reverse_mount_order() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx.clone());
        tools.register(spec("probe"), echo_body()).unwrap();
        for tag in ["先", "后"] {
            let tag = tag.to_string();
            let _h = ctx
                .on_waterfall(TOOLS_PRE_EXECUTE, move |pre: PreExecute, args| {
                    let mut next = args.next::<PreExecute>().unwrap_or(pre);
                    next.rewrite_args(format!("{}+{tag}", next.arguments()));
                    next
                })
                .unwrap();
            std::mem::forget(_h);
        }

        let out = tools.execute(call("probe", "起点")).await;
        assert_eq!(out.content, "起点+后+先");
    }

    /// 改道**不能绕过预设的允许名单**——两个方向都钉住。
    ///
    /// 名单是工具集**边界**，不是「模型能叫什么」的过滤器。所以查两次：模型直接叫
    /// 名单外的，入站那次就挡掉，pre-execute 根本够不着；handler 把名单内的改道到
    /// 名单外，改写后那次挡掉。少了后一次，一颗只读子代理的 `read_file` 就能被任意
    /// handler 改成 `bash` 跑出去。
    #[tokio::test]
    async fn a_redirect_cannot_escape_the_preset_allowlist() {
        let ctx = Context::new();
        let mut mode = crate::agent::presets::AgentPreset::new("code");
        // 只准 read_file，不准 bash。
        mode.tools = Some(vec!["read_file".to_string()]);
        let _p = ctx
            .provide(AGENT_PRESETS, AgentPresets::overlay(mode))
            .unwrap();

        let tools = Tools::echo(ctx.clone());
        tools.register(spec("read_file"), echo_body()).unwrap();
        tools
            .register(
                spec("bash"),
                Arc::new(|call| Box::pin(async move { tool_result(call, "bash 跑了") })),
            )
            .unwrap();
        let _h = ctx
            .on_waterfall(TOOLS_PRE_EXECUTE, |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                next.rewrite("bash", "{}");
                next
            })
            .unwrap();

        // 方向一：模型直接叫名单外的那颗——入站就挡。
        let blocked = tools.execute(call("bash", "{}")).await;
        assert_eq!(
            blocked.content,
            blocked_tool_message(),
            "名单外的调用要在 pre-execute 之前就被挡掉"
        );
        assert_ne!(blocked.content, "bash 跑了");

        // 方向二（回归）：名单内的 `read_file` 被 handler 改道到名单外的 `bash`——
        // 改写之后那次检查要挡下来，工具体一次都不能跑。
        let escaped = tools.execute(call("read_file", "{}")).await;
        assert_eq!(
            escaped.content,
            blocked_tool_message(),
            "改道到名单外要被改写后的那次检查挡掉，实际 {}",
            escaped.content
        );
        assert_ne!(
            escaped.content, "bash 跑了",
            "只读子代理不该能被改写成跑 bash"
        );
        assert_eq!(escaped.name, "bash", "结果报的是实际要跑的那颗");
        assert_eq!(escaped.call_id, "c1", "挡下来也不能动 call_id");
    }

    /// 改道到**名单内**的另一颗照样放行——上面那条不能顺手把正常降权改道也堵死。
    #[tokio::test]
    async fn a_redirect_inside_the_allowlist_still_runs() {
        let ctx = Context::new();
        let mut mode = crate::agent::presets::AgentPreset::new("code");
        mode.tools = Some(vec!["read_file".to_string(), "list_dir".to_string()]);
        let _p = ctx
            .provide(AGENT_PRESETS, AgentPresets::overlay(mode))
            .unwrap();

        let tools = Tools::echo(ctx.clone());
        tools.register(spec("read_file"), echo_body()).unwrap();
        tools
            .register(
                spec("list_dir"),
                Arc::new(|call| Box::pin(async move { tool_result(call, "list_dir 跑了") })),
            )
            .unwrap();
        let _h = ctx
            .on_waterfall(TOOLS_PRE_EXECUTE, |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                next.rewrite("list_dir", "{}");
                next
            })
            .unwrap();

        let out = tools.execute(call("read_file", "{}")).await;
        assert_eq!(out.content, "list_dir 跑了");
        assert_eq!(out.name, "list_dir");
    }

    /// 拒绝时结果也报**改写后**的名字，跟放行路径一条规则。
    #[tokio::test]
    async fn a_denial_after_a_redirect_reports_the_effective_name() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx.clone());
        tools.register(spec("probe"), echo_body()).unwrap();
        let _h = ctx
            .on_waterfall(TOOLS_PRE_EXECUTE, |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                next.rewrite("other", "{}");
                next.deny("这次不行");
                next
            })
            .unwrap();

        let out = tools.execute(call("probe", "{}")).await;
        assert_eq!(out.content, "这次不行");
        assert_eq!(out.name, "other", "报的是改写后、也就是本来要跑的那颗");
        assert_eq!(out.call_id, "c1");
    }

    /// 门读的是**改写之后**的名字。
    ///
    /// 这是位点卡在计划门 / 权限门之前的全部理由：`monitor` 改成一颗不受管的工具，
    /// 就该跟着走不受管的路径；要是在门之后改，它已经按 `monitor` 被挡下了，改道
    /// 毫无意义。这里用计划门验——它和权限门查的是同一张 `gated_builtin` 表，
    /// 但同步、不需要有人去应答询问队列。
    #[tokio::test]
    async fn gates_read_the_post_waterfall_name() {
        assert!(acp::blocked_in_plan("monitor"), "前提：monitor 受计划门管");
        assert!(!acp::blocked_in_plan("probe"), "前提：probe 不受管");

        // 计划门只在 workspace registry 上跑（`if self.workspace`），所以不能用
        // `install_without_llm` 那个 echo registry。
        let ctx = Context::new();
        let plan = PlanMode::new(ctx.clone());
        plan.enter_active();
        let _p = ctx.provide(PLAN_MODE, plan).unwrap();

        let tools = Tools::workspace(ctx.clone());
        tools.register(spec("monitor"), echo_body()).unwrap();
        tools.register(spec("probe"), echo_body()).unwrap();

        // 先确认不改道时确实被挡。
        let blocked = tools.execute(call("monitor", "原样")).await;
        assert!(
            blocked.content.contains("计划模式"),
            "前提：不改道时 monitor 该被计划门挡下，实际 {}",
            blocked.content
        );

        let _h = ctx
            .on_waterfall(TOOLS_PRE_EXECUTE, |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                if next.name() == "monitor" {
                    next.rewrite("probe", "改道了");
                }
                next
            })
            .unwrap();

        let out = tools.execute(call("monitor", "原样")).await;
        assert_eq!(out.content, "改道了", "改道后该按 probe 放行");
        assert_eq!(out.name, "probe");
    }

    /// 没人挂 handler 时行为不变——这个位点是纯加法。
    #[tokio::test]
    async fn without_listeners_the_call_is_untouched() {
        let ctx = Context::new();
        let tools = Tools::echo(ctx.clone());
        tools.register(spec("probe"), echo_body()).unwrap();
        let out = tools.execute(call("probe", "原样")).await;
        assert_eq!(out.content, "原样");
    }
    #[test]
    fn cua_permission_summary_highlights_action_and_role() {
        let args = r#"{"action":"click","role":"button","label":"Save","element_token":"s00abcdef0123456789","x":10,"y":20}"#;
        let s = permission_summary("mcp_cua-driver__click", args);
        assert!(s.contains("mcp_cua-driver__click"), "{s}");
        assert!(s.contains("click"), "{s}");
        assert!(s.contains("role=button"), "{s}");
        assert!(s.contains("label=Save"), "{s}");
        assert!(s.contains("element_token="), "{s}");
        assert!(
            !s.contains("s00abcdef0123456789"),
            "full token must be hidden: {s}"
        );
    }

    #[test]
    fn generic_mcp_summary_redacts_long_tokens() {
        let tok = "a".repeat(64);
        let args = format!(r#"{{"name":"tool","token":"{tok}"}}"#);
        let s = permission_summary("mcp_other__do", &args);
        assert!(s.contains("mcp_other__do"), "{s}");
        assert!(!s.contains(&tok), "long token must be redacted: {s}");
    }
}
