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
use crate::tools::dynamic_runner::{DynamicRunner, PersistScope, PluginOrigin, PluginSel, RunMode};
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{LogEvent, PreStep, ToolCall, ToolResult, ToolSpec};

use inspect::{render_inspect, render_inspect_self};
use prompt::CORDIS_SYSTEM_PROMPT;

type ExecFut = Pin<Box<dyn Future<Output = ToolResult> + Send + 'static>>;

const INSPECT_DESC: &str = "Read-only live Cordis directory for this session: fibers under cordis-dynamic, named services that are actually mounted (tools / slash / tui.slots include callable methods; other services are names only), model tools (names only), preset factories, Rhai host builtins, session/event contract, TUI slots, this session's dynamic Plugins, and disk plugins. Omit `what` for the full report. This does not execute apply or change version pointers. Before defining a Plugin, also read skills/cordis-plugin-development/SKILL.md.";
const INSPECT_PARAMS: &str = r#"{"type":"object","properties":{"what":{"type":"string","enum":["services","fibers","tools","temporary","permanent","factories","builtins","events","slots"],"description":"Omit for the full live directory."}}}"#;

const SELF_DESC: &str = "Inspect dynamic Cordis Plugins owned by this session. With no IDs, list Plugin summaries. With pluginId, return version pointers, the latest Run, and every Package. pluginId plus packageId returns that Package's factory id, Rhai source when present, and runtime diagnostics. Read-only: it does not execute code or change currentPackageId. Read skills/cordis-plugin-development/SKILL.md before modifying a Plugin.";
const SELF_PARAMS: &str = r#"{"type":"object","properties":{"pluginId":{"type":"string","description":"Stable Plugin ID from cordis_define; omit to list every Plugin."},"packageId":{"type":"string","description":"Exact Package ID; requires pluginId."}}}"#;

const DEFINE_DESC: &str = "Define an immutable Cordis Package. For a new Plugin, kind:\"new\" and idPrefix of 3–6 lowercase English letters. To modify an existing Plugin, kind:\"existing\" with its pluginId — this appends a Package and never overwrites older versions. factory is echo, note, hold, slash, or rhai (see cordis_inspect what:\"factories\"). factory rhai needs source: a Rhai map #{ inject: [...], apply: |host| { ... } }; define compiles it and does not run apply. The slash factory also needs command, kind (prompt, overlay, slot, or tool), and text; it only appends a prompt-bar command and cannot replace builtins. kind tool: text is the live tool name; typed slash args become JSON (empty→{}, leading { → raw object, else {\"args\":…}). Define only records metadata: it does not request approval, execute apply, or change currentPackageId. On success, call cordis_run with the returned IDs. Read skills/cordis-plugin-development/SKILL.md first.";
const DEFINE_PARAMS: &str = r#"{"type":"object","required":["plugin","name","purpose","factory"],"properties":{"plugin":{"type":"object","description":"kind:\"new\" + idPrefix, or kind:\"existing\" + pluginId."},"name":{"type":"string"},"purpose":{"type":"string"},"factory":{"type":"string","description":"Preset factory id: echo, note, hold, slash, or rhai."},"source":{"type":"string","description":"rhai factory: Rhai map with inject and apply."},"command":{"type":"string","description":"slash factory: extra /command token; cannot collide with builtins."},"kind":{"type":"string","enum":["prompt","overlay","slot","tool"],"description":"slash factory: prompt injects/sends text; overlay opens a read-only TUI pane; slot opens a registered tui.slots id (text = slot id); tool runs a live model tool (text = tool name; typed args become JSON)."},"text":{"type":"string","description":"slash factory: prompt template, overlay body, slot id, or tool name. {args} is replaced with typed arguments for prompt/overlay."},"title":{"type":"string","description":"slash overlay/tool Notice title; defaults to /command or /command → tool."},"send":{"type":"boolean","description":"slash prompt: true sends immediately; false fills the composer."},"description":{"type":"string","description":"slash dropdown /help label; defaults to purpose."}}}"#;

const RUN_DESC: &str = "Activate one exact Package of a dynamic Plugin. mode:\"run\" for the first activation, restarting current, or rollback. When current exists, mode:\"update\" switches to a different Package by disposing the previous host-half fiber entirely (every provide/register_* from that Run) and starting the new Package on a clean fiber — it does not overlay. Host-only factories start immediately after the user allows the permission prompt (same overlay as bash). currentPackageId changes only after success. A throwing apply rolls back registrations before the error returns. After a technical failure, inspect the Package, define a new Package on the same Plugin, and retry. Do not request approval again after the user rejects it.";
const RUN_PARAMS: &str = r#"{"type":"object","required":["pluginId","packageId","mode"],"properties":{"pluginId":{"type":"string"},"packageId":{"type":"string"},"mode":{"type":"string","enum":["run","update"]}}}"#;

const CALL_DESC: &str = "Execute one live model-facing tool by exact name, including tools a running dynamic Package registered with host.register_tool / register_dynamic. Use this after cordis_run to verify a dynamic tool in the same Host turn — do not wait for a later model step or a TUI slash like /test. `arguments` is a JSON object (or a JSON string of that object); omit for {}. Permissions, plan-mode gates, and Agent preset allowlists still apply (dynamic tools bypass the allowlist). Do not call cordis_call recursively.";
const CALL_PARAMS: &str = r#"{"type":"object","required":["name"],"properties":{"name":{"type":"string","description":"Exact tool name from cordis_inspect what:\"tools\" or a dynamic register_tool name."},"arguments":{"description":"JSON object of tool arguments, or a JSON string. Default {}."}}}"#;

const STOP_DESC: &str = "Stop the current Run of a dynamic Plugin. Retain the Plugin, every Package, and currentPackageId so it can later run or update. Stopping an already stopped Plugin succeeds. Disk files are not deleted; enabled disk plugins autoload on the next process start. Use cordis_undefine to drop the in-memory Plugin.";
const STOP_PARAMS: &str =
    r#"{"type":"object","required":["pluginId"],"properties":{"pluginId":{"type":"string"}}}"#;

const UNDEFINE_DESC: &str = "Remove a dynamic Plugin from this process. If it is running, stop it first, then delete every in-memory Package and version pointer. Does not delete disk files under .dock/plugins/<id>/; set enabled=false or delete that directory to stop autoload. Do not call this when versions must remain for restart or rollback; use cordis_stop instead.";
const UNDEFINE_PARAMS: &str =
    r#"{"type":"object","required":["pluginId"],"properties":{"pluginId":{"type":"string"}}}"#;

const PROMOTE_DESC: &str = "Write the Plugin's current (or latest) Package to disk so it survives restart. Default id strips the minted -N suffix (echo-1 → echo). scope \"project\" writes {cwd}/.dock/plugins/<id>/; \"user\" writes ~/.dock/plugins/<id>/. Then autostarts that disk Plugin (no second permission prompt). If the disk id differs from the session Plugin, the session copy is stopped to avoid duplicate tools — undefine it if you no longer need the in-memory definition. Does not delete files on later cordis_undefine.";
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
                        "cordis_inspect_self",
                        SELF_DESC,
                        SELF_PARAMS,
                        inspect_self_tool,
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
                        "cordis_undefine",
                        UNDEFINE_DESC,
                        UNDEFINE_PARAMS,
                        undefine_tool,
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
        let what = json(&call)
            .get("what")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
            return tool_result(call, "Error: dynamicCordisRunner is not mounted");
        };
        let Some(tools) = ctx.get::<Tools>(TOOLS) else {
            return tool_result(call, "Error: tools is not mounted");
        };
        tool_result(
            call,
            render_inspect(
                &ctx,
                &runner,
                &tools,
                session_id(&ctx).as_str(),
                what.as_deref(),
            ),
        )
    })
}

fn inspect_self_tool(ctx: Context, call: ToolCall) -> ExecFut {
    Box::pin(async move {
        let v = json(&call);
        let plugin_id = v.get("pluginId").and_then(Value::as_str);
        let package_id = v.get("packageId").and_then(Value::as_str);
        if package_id.is_some() && plugin_id.is_none() {
            return tool_result(
                call,
                "Error: cordis_inspect_self packageId requires pluginId",
            );
        }
        let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
            return tool_result(call, "Error: dynamicCordisRunner is not mounted");
        };
        match render_inspect_self(&runner, &session_id(&ctx), plugin_id, package_id) {
            Ok(text) => tool_result(call, text),
            Err(e) => tool_result(call, format!("Error: {e}")),
        }
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
        let source = v
            .get("source")
            .and_then(Value::as_str)
            .map(|s| s.to_string());
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
            Err(e) => tool_result(call, format!("Error: {e}")),
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
        let plugin_id = json(&call)
            .get("pluginId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
            return tool_result(call, "Error: dynamicCordisRunner is not mounted");
        };
        if plugin_id.is_empty() {
            return tool_result(call, "Error: pluginId is required");
        }
        match runner.stop(&session_id(&ctx), &plugin_id).await {
            Ok(r) => tool_result(
                call,
                format!(
                    "Dynamic Plugin {} is stopped; its definition and versions remain.",
                    r.plugin_id
                ),
            ),
            Err(e) => tool_result(call, format!("Error: {e}")),
        }
    })
}

fn undefine_tool(ctx: Context, call: ToolCall) -> ExecFut {
    Box::pin(async move {
        let plugin_id = json(&call)
            .get("pluginId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
            return tool_result(call, "Error: dynamicCordisRunner is not mounted");
        };
        if plugin_id.is_empty() {
            return tool_result(call, "Error: pluginId is required");
        }
        let sid = session_id(&ctx);
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
                "# @{} context\npluginId: {}\npackageId: {} (baseline)\nname: {}\npurpose: {}\nrunning: {}\nCall cordis_inspect_self with these ids, then cordis_define kind existing. Do not create a replacement Plugin. Source is not included here.",
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
