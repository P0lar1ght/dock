//! Model-facing Cordis inspect / define / run / call / stop / undefine tools.
//! Injects `"dynamicCordisRunner"` + `"tools"`; does not own the registry.

mod inspect;
mod prompt;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use cordis::{plugin, Context, Inject, Plugin};
use serde_json::Value;

use crate::host::slash::{contrib_fields_present, slash_entry_from_define};
use crate::names::{CONTEXT, DYNAMIC_CORDIS_RUNNER, PRE_STEP, SESSIONS, TOOLS};
use crate::prompt::assemble::ORDER_CORDIS;
use crate::prompt::context_book::{own_sections, ContextBook};
use crate::session::log::Sessions;
use crate::tools::dynamic_runner::{
    AttemptStatus, DynamicRunner, PersistScope, PluginOrigin, PluginSel, RunMode, SourceInput,
};
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{LogEvent, PreStep, ToolCall, ToolResult, ToolSpec};

use inspect::{render_inspect, render_plugin};
use prompt::CORDIS_SYSTEM_PROMPT;

type ExecFut = Pin<Box<dyn Future<Output = ToolResult> + Send + 'static>>;

/// One read-only tool, two questions: "what is live in this process" (`what`)
/// and "what is in this Plugin" (`pluginId`). They used to be `cordis_inspect`
/// and `cordis_inspect_self`; both are ungated reads over the same registry, and
/// splitting them cost a whole extra row in `search_tool`'s window — which is
/// how `cordis_define` / `cordis_run` fell out of the default five hits.
const INSPECT_DESC: &str = "Read-only Cordis directory. With `what`: services (reachable from Rhai vs merely mounted), fibers, tools, factories, builtins (Rhai host API), events, slots, temporary (session Plugins), permanent (disk plugins); omit `what` for all of it. With `pluginId`: that Plugin's version pointers, latest Run and Packages; add `packageId` for one Package's factory, source_path or inline source, and runtime diagnostics. Never executes apply or moves version pointers. Read skills/cordis-plugin-development/SKILL.md before writing a Plugin.";
const INSPECT_PARAMS: &str = r#"{"type":"object","properties":{"what":{"type":"string","enum":["services","fibers","tools","temporary","permanent","factories","builtins","events","slots"],"description":"Live directory section. Omit for all sections. Ignored when pluginId is given."},"pluginId":{"type":"string","description":"Inspect this Plugin instead of the directory."},"packageId":{"type":"string","description":"One Package of pluginId; requires pluginId."}}}"#;

const DEFINE_DESC: &str = "Record an immutable Package. New Plugin: plugin.kind \"new\" + idPrefix (3–6 lowercase letters). Change an existing one: plugin.kind \"existing\" + its pluginId, which appends a Package and never overwrites an older one. factory is echo, note, hold, slash, or rhai (cordis_inspect what:\"factories\"). Custom behaviour is factory rhai: write the script to .dock/plugins/<id>/source.rhai and pass source_path; inline source is for tiny samples only. source XOR source_path. Define compiles the source and stops there — no approval, no apply, currentPackageId unchanged; call cordis_run next. Load skills/cordis-plugin-development/SKILL.md before authoring: it has the Rhai contract, the host API and the slash fields.";
const DEFINE_PARAMS: &str = r#"{"type":"object","required":["plugin","name","purpose","factory"],"properties":{"plugin":{"type":"object","description":"kind:\"new\" + idPrefix, or kind:\"existing\" + pluginId."},"name":{"type":"string"},"purpose":{"type":"string"},"factory":{"type":"string","description":"echo, note, hold, slash, or rhai."},"source":{"type":"string","description":"rhai, inline (≤128KiB, tiny samples): #{ inject: [...], apply: |host| { ... } }. XOR source_path."},"source_path":{"type":"string","description":"rhai, preferred: path under .dock/plugins/<id>/ or ~/.dock/plugins/<id>/ (≤1MiB, re-read on run). XOR source. No .. escape."},"command":{"type":"string","description":"slash: extra /command token; cannot collide with builtins."},"kind":{"type":"string","enum":["prompt","overlay","slot","tool"],"description":"slash: what /command does. prompt = template, overlay = read-only pane, slot = open a tui.slots id, tool = run a live tool."},"text":{"type":"string","description":"slash: prompt template, overlay body, slot id, or tool name. {args} takes the typed arguments."},"title":{"type":"string","description":"slash overlay/tool Notice title."},"send":{"type":"boolean","description":"slash prompt: send immediately (default) or fill the composer."},"description":{"type":"string","description":"slash /help label; defaults to purpose."}}}"#;

const RUN_DESC: &str = "Activate one exact Package. mode \"run\" = first start, restart of current, or rollback to it; mode \"update\" = switch to a different Package, which disposes the previous Run's fiber entirely (every provide / register_* it made) and starts clean — it does not overlay. Asks the same permission overlay as bash. currentPackageId moves only on success; a throwing apply rolls its registrations back first. Do not re-request approval the user rejected.";
const RUN_PARAMS: &str = r#"{"type":"object","required":["pluginId","packageId","mode"],"properties":{"pluginId":{"type":"string"},"packageId":{"type":"string"},"mode":{"type":"string","enum":["run","update"]}}}"#;

/// The repair loop is in `RUN_DESC` and the Skill, but `cordis_*` are deferred
/// tools: their descriptions are not in context unless the model just looked
/// them up. A failed apply is the one place the loop is needed, so it ships with
/// the failure instead of being remembered. Only for host failures — a mode /
/// id validation error already says what to do, and this on top would bury it.
const RUN_REPAIR_HINT: &str = "Packages are immutable — do not retry this same packageId. Repair: cordis_inspect with pluginId+packageId for the source and hostError; fix the file (or the inline source); cordis_define again with plugin.kind \"existing\" and the SAME pluginId, which appends a new Package; then cordis_run that new packageId (mode \"update\" when the Plugin already has a current Package, otherwise \"run\"). Do not mint a second Plugin for the same purpose.";

const CALL_DESC: &str = "Execute one live model-facing tool by exact name, including tools a running dynamic Package registered with host.register_tool / register_dynamic. Use this after cordis_run to verify a dynamic tool in the same Host turn — do not wait for a later model step or a TUI slash like /test. `arguments` is a JSON object (or a JSON string of that object); omit for {}. Permissions, plan-mode gates, and Agent preset allowlists still apply (dynamic tools bypass the allowlist). Do not call cordis_call recursively.";
const CALL_PARAMS: &str = r#"{"type":"object","required":["name"],"properties":{"name":{"type":"string","description":"Exact tool name from cordis_inspect what:\"tools\" or a dynamic register_tool name."},"arguments":{"description":"JSON object of tool arguments, or a JSON string. Default {}."}}}"#;

/// Teardown is one tool with one switch, not two tools. Both halves are ungated
/// and differ in exactly one thing — whether the definition survives — which is
/// easier to teach side by side than as two descriptions that each end with
/// "use the other one instead".
const STOP_DESC: &str = "Stop a dynamic Plugin's current Run: its fiber is disposed, so every tool, slash, slot and bag that Run registered goes away. Stopping an already stopped Plugin succeeds. By default the Plugin, its Packages and currentPackageId stay, so it can run or update again later. drop:true also deletes the in-memory definition (use it only when no version needs to remain). Neither one touches disk: files under .dock/plugins/<id>/ stay and autoload next start unless you set enabled=false or delete that directory.";
const STOP_PARAMS: &str = r#"{"type":"object","required":["pluginId"],"properties":{"pluginId":{"type":"string"},"drop":{"type":"boolean","description":"Also forget the definition and every Package (was cordis_undefine). Default false."}}}"#;

const PROMOTE_DESC: &str = "Write the Plugin's current (or latest) Package to disk so it survives restart, then autostart it from there (no second permission prompt). scope \"project\" (default) writes {cwd}/.dock/plugins/<id>/, \"user\" writes ~/.dock/plugins/<id>/; default id strips the minted -N suffix (echo-1 → echo). If the disk id differs from the session Plugin, the session copy is stopped to avoid duplicate tools — cordis_stop drop:true it if you no longer need the in-memory definition. Files stay until you delete them.";
const PROMOTE_PARAMS: &str = r#"{"type":"object","required":["pluginId"],"properties":{"pluginId":{"type":"string","description":"Session or already-loaded Plugin to persist."},"id":{"type":"string","description":"Disk directory / stable pluginId. Default: strip -N from pluginId."},"scope":{"type":"string","enum":["project","user"],"description":"project (default) = workspace .dock/plugins; user = ~/.dock/plugins."}}}"#;

pub fn tool_cordis() -> Plugin {
    plugin(
        "tool-cordis",
        Inject::from([TOOLS, DYNAMIC_CORDIS_RUNNER, CONTEXT]),
        |ctx, _: &()| {
            let book = ctx.require::<ContextBook>(CONTEXT)?;
            own_sections(
                ctx,
                vec![book.section(ORDER_CORDIS, "cordis", |_| {
                    Some(CORDIS_SYSTEM_PROMPT.to_string())
                })?],
            )?;
            let ctx_pre = ctx.clone();
            let _ = ctx.on_waterfall(PRE_STEP, move |step: PreStep, args| {
                let next = args.next::<PreStep>().unwrap_or(step);
                // Main session only: a `@pluginId` reminder has to land in the
                // session that mentioned it, and that is the only one reachable.
                if next.enter && next.is_main_session() {
                    inject_plugin_mentions(&ctx_pre, &next.user);
                }
                next
            });
            let tools = ctx.require::<Tools>(TOOLS)?;
            own_registered(
                ctx,
                vec![
                    spec(
                        ctx,
                        tools.as_ref(),
                        "cordis_inspect",
                        INSPECT_DESC,
                        INSPECT_PARAMS,
                        inspect_tool,
                    )?,
                    spec(
                        ctx,
                        tools.as_ref(),
                        "cordis_define",
                        DEFINE_DESC,
                        DEFINE_PARAMS,
                        define_tool,
                    )?,
                    spec(
                        ctx,
                        tools.as_ref(),
                        "cordis_run",
                        RUN_DESC,
                        RUN_PARAMS,
                        run_tool,
                    )?,
                    spec(
                        ctx,
                        tools.as_ref(),
                        "cordis_call",
                        CALL_DESC,
                        CALL_PARAMS,
                        call_tool,
                    )?,
                    spec(
                        ctx,
                        tools.as_ref(),
                        "cordis_stop",
                        STOP_DESC,
                        STOP_PARAMS,
                        stop_tool,
                    )?,
                    spec(
                        ctx,
                        tools.as_ref(),
                        "cordis_promote",
                        PROMOTE_DESC,
                        PROMOTE_PARAMS,
                        promote_tool,
                    )?,
                ],
            )?;
            Ok(None)
        },
    )
}

fn spec(
    ctx: &Context,
    tools: &Tools,
    name: &str,
    description: &str,
    params: &str,
    exec: fn(Context, ToolCall) -> ExecFut,
) -> cordis::Result<cordis::Disposable> {
    let ctx = ctx.clone();
    let body: ToolBody = Arc::new(move |call| {
        let ctx = ctx.clone();
        Box::pin(async move { exec(ctx, call).await })
    });
    tools.register_deferred(
        ToolSpec {
            name: name.into(),
            description: description.into(),
            parameters_json: params.into(),
        },
        body,
    )
}

fn inspect_tool(ctx: Context, call: ToolCall) -> ExecFut {
    Box::pin(async move {
        let v = json(&call);
        let plugin_id = v.get("pluginId").and_then(Value::as_str);
        let package_id = v.get("packageId").and_then(Value::as_str);
        let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
            return tool_result(call, "Error: dynamicCordisRunner is not mounted");
        };
        // One Package is the narrowest question, so it wins over `what`.
        if let Some(plugin_id) = plugin_id {
            return match render_plugin(&runner, &session_id(&ctx), plugin_id, package_id) {
                Ok(text) => tool_result(call, text),
                Err(e) => tool_result(call, format!("Error: {e}")),
            };
        }
        if package_id.is_some() {
            return tool_result(call, "Error: cordis_inspect packageId requires pluginId");
        }
        let Some(tools) = ctx.get::<Tools>(TOOLS) else {
            return tool_result(call, "Error: tools is not mounted");
        };
        let what = v.get("what").and_then(Value::as_str);
        tool_result(
            call,
            render_inspect(&ctx, &runner, &tools, session_id(&ctx).as_str(), what),
        )
    })
}

fn define_tool(ctx: Context, call: ToolCall) -> ExecFut {
    Box::pin(async move {
        let v = json(&call);
        let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
            return tool_result(call, "Error: dynamicCordisRunner is not mounted");
        };
        let plugin = match parse_plugin_sel(&v) {
            Ok(p) => p,
            Err(e) => return tool_result(call, format!("Error: {e}")),
        };
        let name = v.get("name").and_then(Value::as_str).unwrap_or("");
        let purpose = v.get("purpose").and_then(Value::as_str).unwrap_or("");
        let factory = v.get("factory").and_then(Value::as_str).unwrap_or("");
        let contrib = match (factory, contrib_fields_present(&v)) {
            ("slash", _) => match slash_entry_from_define(&v, purpose) {
                Ok(entry) => Some(entry),
                Err(e) => return tool_result(call, format!("Error: {e}")),
            },
            (_, true) => {
                return tool_result(
                    call,
                    "Error: command/kind/text are only for factory \"slash\"",
                );
            }
            _ => None,
        };
        let source = match (
            v.get("source").and_then(Value::as_str),
            v.get("source_path").and_then(Value::as_str),
        ) {
            (Some(_), Some(_)) => {
                return tool_result(
                    call,
                    "Error: provide source or source_path for factory \"rhai\", not both",
                );
            }
            (Some(s), None) => Some(SourceInput::Inline(s.to_string())),
            (None, Some(p)) => Some(SourceInput::Path(p.to_string())),
            (None, None) => None,
        };
        match runner.define(
            session_id(&ctx).as_str(),
            plugin,
            name,
            purpose,
            factory,
            contrib,
            source,
        ) {
            Ok(r) => tool_result(
                call,
                format!(
                    "Defined {}/{} ({}); factory {}; it is not running yet. Use cordis_run to activate this Package.",
                    r.plugin_id, r.package_id, r.name, r.factory
                ),
            ),
            Err(e) => tool_result(call, format!("Error: {e}")),
        }
    })
}

fn run_tool(ctx: Context, call: ToolCall) -> ExecFut {
    Box::pin(async move {
        let v = json(&call);
        let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
            return tool_result(call, "Error: dynamicCordisRunner is not mounted");
        };
        let plugin_id = v.get("pluginId").and_then(Value::as_str).unwrap_or("");
        let package_id = v.get("packageId").and_then(Value::as_str).unwrap_or("");
        let mode = match RunMode::parse(v.get("mode").and_then(Value::as_str).unwrap_or("")) {
            Ok(m) => m,
            Err(e) => return tool_result(call, format!("Error: {e}")),
        };
        if plugin_id.is_empty() || package_id.is_empty() {
            return tool_result(call, "Error: pluginId and packageId are required");
        }
        match runner
            .run(&session_id(&ctx), plugin_id, package_id, mode)
            .await
        {
            Ok(r) => tool_result(
                call,
                serde_json::json!({
                    "status": r.status,
                    "pluginId": r.plugin_id,
                    "packageId": r.package_id,
                    "pluginRunId": r.plugin_run_id,
                    "mode": r.mode.as_str(),
                    "currentPackageId": r.current_package_id,
                    "nextPackageId": r.next_package_id,
                    "host": {
                        "status": r.status,
                        "provides": r.provides,
                        "waitingFor": r.waiting_for,
                        "fiberState": r.fiber_state,
                    }
                })
                .to_string(),
            ),
            Err(e) => {
                // 只有真的起过 host 半才谈得上"修 source 再来一版"：`resolve_plan`
                // 的 mode / id 报错自己已经写清了下一步。
                let host_failed = runner
                    .inspect_plugin(&session_id(&ctx), plugin_id)
                    .ok()
                    .and_then(|row| row.latest)
                    .is_some_and(|a| {
                        a.status == AttemptStatus::Failed && a.host_error.as_deref() == Some(&e)
                    });
                if host_failed {
                    tool_result(call, format!("Error: {e}\n\n{RUN_REPAIR_HINT}"))
                } else {
                    tool_result(call, format!("Error: {e}"))
                }
            }
        }
    })
}

fn call_tool(ctx: Context, call: ToolCall) -> ExecFut {
    Box::pin(async move {
        let v = json(&call);
        let name = v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            return tool_result(call, "Error: name is required");
        }
        if name == "cordis_call" {
            return tool_result(call, "Error: cordis_call cannot call itself");
        }
        let Some(tools) = ctx.get::<Tools>(TOOLS) else {
            return tool_result(call, "Error: tools is not mounted");
        };
        let live: Vec<String> = tools.specs().into_iter().map(|s| s.name).collect();
        if !live.iter().any(|n| n == &name) {
            return tool_result(
                call,
                format!(
                    "Error: tool {name:?} is not registered. Live tools: {}",
                    live.join(", ")
                ),
            );
        }
        let arguments = match v.get("arguments") {
            None | Some(Value::Null) => "{}".into(),
            Some(Value::String(s)) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    "{}".into()
                } else {
                    s.clone()
                }
            }
            Some(other) => other.to_string(),
        };
        let inner = tools
            .execute(ToolCall {
                id: format!("{}→{}", call.id, name),
                name: name.clone(),
                arguments,
            })
            .await;
        tool_result(
            call,
            format!("## cordis_call → {name}\n\n{}", inner.content),
        )
    })
}

fn stop_tool(ctx: Context, call: ToolCall) -> ExecFut {
    Box::pin(async move {
        let v = json(&call);
        let plugin_id = v
            .get("pluginId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let drop = v.get("drop").and_then(Value::as_bool).unwrap_or(false);
        let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
            return tool_result(call, "Error: dynamicCordisRunner is not mounted");
        };
        if plugin_id.is_empty() {
            return tool_result(call, "Error: pluginId is required");
        }
        let sid = session_id(&ctx);
        if !drop {
            return match runner.stop(&sid, &plugin_id).await {
                Ok(r) => tool_result(
                    call,
                    format!(
                        "Dynamic Plugin {} is stopped; its definition and versions remain.",
                        r.plugin_id
                    ),
                ),
                Err(e) => tool_result(call, format!("Error: {e}")),
            };
        }
        // Read the origin before the definition is gone — that is the only place
        // the disk path lives, and the model needs it to hear "files remain".
        let origin = runner
            .inspect_plugin(&sid, &plugin_id)
            .ok()
            .map(|row| row.origin);
        match runner.undefine(&sid, &plugin_id).await {
            Ok(r) => {
                let disk = match origin {
                    Some(PluginOrigin::Disk { path, .. }) => format!(
                        " Disk files remain at {}; set enabled=false or delete that directory to stop autoload.",
                        path.display()
                    ),
                    _ => String::new(),
                };
                tool_result(
                    call,
                    format!(
                        "Removed dynamic Plugin {} and all of its Packages. wasRunning={}{disk}",
                        r.plugin_id, r.was_running
                    ),
                )
            }
            Err(e) => tool_result(call, format!("Error: {e}")),
        }
    })
}

fn promote_tool(ctx: Context, call: ToolCall) -> ExecFut {
    Box::pin(async move {
        let v = json(&call);
        let plugin_id = v
            .get("pluginId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if plugin_id.is_empty() {
            return tool_result(call, "Error: pluginId is required");
        }
        let disk_id = v.get("id").and_then(Value::as_str);
        let scope = match PersistScope::parse(v.get("scope").and_then(Value::as_str).unwrap_or(""))
        {
            Ok(s) => s,
            Err(e) => return tool_result(call, format!("Error: {e}")),
        };
        let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
            return tool_result(call, "Error: dynamicCordisRunner is not mounted");
        };
        match runner
            .promote(&session_id(&ctx), &plugin_id, disk_id, scope)
            .await
        {
            Ok(r) => {
                let stop_note = if r.stopped_source {
                    format!(
                        " Session Plugin {} was stopped to avoid duplicate tools; undefine it if you no longer need the in-memory copy.",
                        r.source_plugin_id
                    )
                } else {
                    String::new()
                };
                tool_result(
                    call,
                    format!(
                        "Wrote {} plugin {} at {}. packageId={}; running={}.{stop_note} Restart autoloads this directory without a permission overlay.",
                        r.scope.as_str(),
                        r.plugin_id,
                        r.path.display(),
                        r.package_id,
                        r.running
                    ),
                )
            }
            Err(e) => tool_result(call, format!("Error: {e}")),
        }
    })
}

fn json(call: &ToolCall) -> Value {
    serde_json::from_str(&call.arguments).unwrap_or(Value::Null)
}

fn session_id(ctx: &Context) -> String {
    ctx.get::<Sessions>(SESSIONS)
        .map(|s| s.identity().to_string())
        .unwrap_or_else(|| "main".into())
}

fn inject_plugin_mentions(ctx: &Context, user: &str) {
    let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
        return;
    };
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return;
    };
    let sid = sessions.identity().to_string();
    for id in mentioned_plugin_ids(user) {
        let text = match runner.reference(&sid, &id) {
            Ok(r) => format!(
                "# @{} context\npluginId: {}\npackageId: {} (baseline)\nname: {}\npurpose: {}\nrunning: {}\nCall cordis_inspect with these ids, then cordis_define kind existing. Do not create a replacement Plugin. Source is not included here.",
                r.plugin_id, r.plugin_id, r.package_id, r.name, r.purpose, r.running
            ),
            Err(_) if looks_like_minted_id(&id) => format!(
                "@{id} is not a dynamic Plugin in this session — it may have been removed or lost on restart."
            ),
            Err(_) => continue,
        };
        sessions.append(LogEvent::SystemReminder(text));
    }
}

fn mentioned_plugin_ids(user: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let bytes = user.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            let start = i + 1;
            if start < bytes.len() && bytes[start].is_ascii_lowercase() {
                let mut j = start;
                while j < bytes.len()
                    && j - start < 32
                    && (bytes[j].is_ascii_lowercase()
                        || bytes[j].is_ascii_digit()
                        || bytes[j] == b'-')
                {
                    j += 1;
                }
                let len = j - start;
                if (2..=32).contains(&len) && (j == bytes.len() || bytes[j].is_ascii_whitespace()) {
                    let id = user[start..j].to_string();
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }
    ids
}

fn looks_like_minted_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    let Some(dash) = bytes.iter().position(|b| *b == b'-') else {
        return false;
    };
    let prefix = &bytes[..dash];
    let digits = &bytes[dash + 1..];
    (3..=6).contains(&prefix.len())
        && prefix.iter().all(|b| b.is_ascii_lowercase())
        && !digits.is_empty()
        && digits.iter().all(|b| b.is_ascii_digit())
}

fn parse_plugin_sel(v: &Value) -> Result<PluginSel, String> {
    let plugin = v.get("plugin").ok_or("plugin is required")?;
    let kind = plugin
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("plugin.kind is required")?;
    match kind {
        "new" => {
            let id_prefix = plugin
                .get("idPrefix")
                .and_then(Value::as_str)
                .ok_or("plugin.idPrefix is required")?
                .to_string();
            Ok(PluginSel::New { id_prefix })
        }
        "existing" => {
            let plugin_id = plugin
                .get("pluginId")
                .and_then(Value::as_str)
                .ok_or("plugin.pluginId is required")?
                .to_string();
            Ok(PluginSel::Existing { plugin_id })
        }
        other => Err(format!(
            "plugin.kind must be \"new\" or \"existing\", got {other:?}"
        )),
    }
}
