//! Read-only live fiber / service / tool / temporary directory.
//! Intersects named lookups with what is actually mounted. No generated JSDoc catalog.

use cordis::Context;

use crate::agent::agents::Agents;
use crate::agent::presets::AgentPresets;
use crate::agent::runtime::LoopHandle;
use crate::agent::turn::TurnControl;
use crate::host::permissions::Permissions;
use crate::host::settings::AppSettings;
use crate::host::slash::Slash;
use crate::host::tui_slots::TuiSlots;
use crate::llm::compact::Compact;
use crate::llm::sampler::Llm;
use crate::names::{
    AGENTS, AGENT_LOOP, AGENT_PRESETS, ASK, BROWSER, COMPACT, COMPUTER, CONTEXT, CRON,
    DYNAMIC_CORDIS_RUNNER, GOAL, JOBS, LLM, LSP, MCP, MEMORY, PERMISSIONS, PLAN_MODE, RHAI_BAGS,
    ROSTER, SESSIONS, SETTINGS, SKILLS, SLASH, SUBAGENTS, SYSTEM_PROMPT, TODOS, TOOLS, TUI_SLOTS,
    TURN, WORKFLOWS,
};
use crate::prompt::assemble::SystemPrompt;
use crate::prompt::context_book::ContextBook;
use crate::session::log::Sessions;
use crate::session::roster::Roster;
use crate::tools::ask_user::Ask;
use crate::tools::browser::Browser;
use crate::tools::computer::Computer;
use crate::tools::cron::Cron;
use crate::tools::dynamic_runner::{
    builtins_lines, DynamicRunner, PluginOrigin, RhaiBags, SnapshotRow,
};
use crate::tools::goal::Goal;
use crate::tools::jobs::Jobs;
use crate::tools::lsp::LspBackendAdapter;
use crate::tools::mcp::Mcp;
use crate::tools::memory::Memory;
use crate::tools::plan_mode::PlanMode;
use crate::tools::registry::Tools;
use crate::tools::skills::Skills;
use crate::tools::task::Subagents;
use crate::tools::todo_write::Todos;
use crate::tools::workflow::Workflows;

pub fn render_inspect(
    ctx: &Context,
    runner: &DynamicRunner,
    tools: &Tools,
    session_id: &str,
    what: Option<&str>,
) -> String {
    let mut sections = Vec::new();
    let all = what.is_none();
    if all || what == Some("services") {
        sections.push(section("services", describe_services(ctx, runner)));
    }
    if all || what == Some("fibers") {
        sections.push(section("fibers", describe_fibers(runner, session_id)));
    }
    if all || what == Some("tools") {
        sections.push(section("tools", describe_tools(tools)));
    }
    if all || what == Some("temporary") {
        sections.push(section("temporary", describe_temporary(runner, session_id)));
    }
    if all || what == Some("permanent") {
        sections.push(section("permanent", describe_permanent(runner)));
    }
    if all || what == Some("factories") {
        sections.push(section("factories", describe_factories(runner)));
    }
    if all || what == Some("builtins") {
        sections.push(section("builtins", builtins_lines()));
    }
    if all || what == Some("events") {
        sections.push(section("events", describe_events()));
    }
    if all || what == Some("slots") {
        sections.push(section("slots", describe_slots(ctx)));
    }
    if sections.is_empty() {
        return format!(
            "unknown inspect what:{what:?}; use services, fibers, tools, temporary, permanent, factories, builtins, events, or slots"
        );
    }
    sections.join("\n\n")
}

pub fn render_inspect_self(
    runner: &DynamicRunner,
    session_id: &str,
    plugin_id: Option<&str>,
    package_id: Option<&str>,
) -> Result<String, String> {
    match (plugin_id, package_id) {
        (None, None) => {
            let rows = runner.snapshot(session_id);
            if rows.is_empty() {
                return Ok("No dynamic Plugins are visible in this session. Session definitions live only in memory (a restart clears them). Disk plugins under .dock/plugins/ autoload at startup; inspect what:\"permanent\".".into());
            }
            let mut lines = vec!["mode: plugins".into()];
            for row in rows {
                lines.push(plugin_line(&row));
            }
            Ok(lines.join("\n"))
        }
        (Some(plugin_id), None) => {
            let row = runner.inspect_plugin(session_id, plugin_id)?;
            let mut lines = vec!["mode: plugin".into(), plugin_line(&row)];
            for pkg in &row.packages {
                let is_current = row.current_package_id.as_deref() == Some(&pkg.package_id);
                let is_next = row.next_package_id.as_deref() == Some(&pkg.package_id);
                lines.push(format!(
                    "  - {}: {} factory={} current={} next={} — {}",
                    pkg.package_id, pkg.name, pkg.factory, is_current, is_next, pkg.purpose
                ));
            }
            Ok(lines.join("\n"))
        }
        (Some(plugin_id), Some(package_id)) => {
            let (row, pkg) = runner.inspect_package(session_id, plugin_id, package_id)?;
            let active = row
                .active_run
                .as_ref()
                .filter(|r| r.package_id == package_id);
            let mut lines = vec![
                "mode: package".into(),
                plugin_line(&row),
                format!("packageId: {}", pkg.package_id),
                format!("name: {}", pkg.name),
                format!("purpose: {}", pkg.purpose),
                format!("factory: {}", pkg.factory),
            ];
            if let Some(c) = &pkg.contrib {
                lines.push(format!(
                    "slash: /{} kind={} send={} — {}",
                    c.command,
                    c.kind.as_str(),
                    c.send,
                    c.description
                ));
            }
            if let Some(path) = &pkg.source_path {
                lines.push(format!("source_path: {}", path.display()));
            } else if let Some(source) = &pkg.source {
                lines.push("source:".into());
                lines.push(source.clone());
            }
            match active {
                Some(run) => {
                    lines.push(format!(
                        "runtime: fiber={} state={} provides={} waitingFor={}",
                        run.fiber_name.as_deref().unwrap_or("none"),
                        run.fiber_state.as_deref().unwrap_or("none"),
                        join_or(&run.provides, "none"),
                        join_or(&run.waiting_for, "none"),
                    ));
                }
                None => lines.push("runtime: stopped".into()),
            }
            if let Some(err) = row.latest.as_ref().and_then(|a| a.host_error.as_ref()) {
                lines.push(format!("hostError: {err}"));
            }
            Ok(lines.join("\n"))
        }
        (None, Some(_)) => Err("cordis_inspect_self packageId requires pluginId".into()),
    }
}

/// The three named services a Rhai `host` actually has methods for. Everything
/// else a script might name is a startup gate only — see [`describe_services`].
const REACHABLE: &[(&str, &[&str])] = &[
    (
        TOOLS,
        &[
            "host.register_tool(#{ name, description, parameters, execute }) — model-facing extra; execute still applies permissions",
            "host.call_tool(name, args) — run a live tool by name (permissions and plan mode apply)",
        ],
    ),
    (
        SLASH,
        &[
            "host.register_slash(#{ command, kind, text, title?, send?, description? }) — additive only; cannot replace /agents, /help, /quit, …",
        ],
    ),
    (
        TUI_SLOTS,
        &[
            "host.register_slot(#{ id, title?, hud?, render, on_key? }) — plain-text overlay",
            "host.open_slot(id) — ask the TUI to open that overlay",
        ],
    ),
];

/// Two buckets, because `inject` and reach are **different things**.
///
/// `inject` goes straight into the kernel's `Inject`, which resolves any name
/// that is mounted (type-erased). So injecting `sessions` or `settings` starts
/// fine — and then `host.get` on it returns `()`, because `host` has no method
/// for it. Listing everything flat used to read as "these are all yours".
fn describe_services(ctx: &Context, runner: &DynamicRunner) -> Vec<String> {
    let mut lines =
        vec!["reachable from a Rhai package (host.* has methods for these):".to_string()];
    for (name, sigs) in REACHABLE {
        if !service_live(ctx, name) {
            continue;
        }
        lines.push(format!("- {name}"));
        for sig in *sigs {
            lines.push(format!("    {sig}"));
        }
        if *name == SLASH {
            if let Some(slash) = ctx.get::<Slash>(SLASH) {
                for entry in slash.list() {
                    lines.push(format!(
                        "    already registered: /{} ({}) — {}",
                        entry.command,
                        entry.kind.as_str(),
                        entry.description
                    ));
                }
            }
        }
    }
    for (name, owner) in runner.live_dynamic_services() {
        lines.push(format!(
            "- {name} — bag from {owner}; host.get(\"{name}\") returns a copy, inject it to wait for that package"
        ));
    }
    let mut mounted = probe_spine(ctx);
    mounted.retain(|name| !REACHABLE.iter().any(|(r, _)| r == name));
    mounted.sort();
    lines.push(String::new());
    lines.push(
        "also mounted — an inject name here resolves (the fiber starts), but no host.* method \
         reaches it and host.get returns (). For what they do, call the matching model tool with \
         host.call_tool, which keeps the permission gate:"
            .into(),
    );
    lines.push(format!("  {}", mounted.join(", ")));
    lines
}

fn service_live(ctx: &Context, name: &str) -> bool {
    match name {
        TOOLS => ctx.get::<Tools>(TOOLS).is_some(),
        SLASH => ctx.get::<Slash>(SLASH).is_some(),
        TUI_SLOTS => ctx.get::<TuiSlots>(TUI_SLOTS).is_some(),
        _ => false,
    }
}

/// Hand-kept probe: `Context` has no type-erased "is this name live" lookup, so
/// each name costs one line. A missing name under-reports a live process with no
/// symptom, which is why `install_app_registers_capability_tools_and_mcp_fail_open`
/// (tests/round.rs) asserts the report covers everything `install_app` mounts.
fn probe_spine(ctx: &Context) -> Vec<String> {
    let mut lines = Vec::new();
    push_live::<Sessions>(&mut lines, ctx, SESSIONS);
    push_live::<Llm>(&mut lines, ctx, LLM);
    push_live::<Tools>(&mut lines, ctx, TOOLS);
    push_live::<SystemPrompt>(&mut lines, ctx, SYSTEM_PROMPT);
    push_live::<ContextBook>(&mut lines, ctx, CONTEXT);
    push_live::<Agents>(&mut lines, ctx, AGENTS);
    push_live::<AppSettings>(&mut lines, ctx, SETTINGS);
    push_live::<TurnControl>(&mut lines, ctx, TURN);
    push_live::<Permissions>(&mut lines, ctx, PERMISSIONS);
    push_live::<Cron>(&mut lines, ctx, CRON);
    push_live::<Jobs>(&mut lines, ctx, JOBS);
    push_live::<Slash>(&mut lines, ctx, SLASH);
    push_live::<TuiSlots>(&mut lines, ctx, TUI_SLOTS);
    push_live::<AgentPresets>(&mut lines, ctx, AGENT_PRESETS);
    push_live::<Todos>(&mut lines, ctx, TODOS);
    push_live::<PlanMode>(&mut lines, ctx, PLAN_MODE);
    push_live::<Ask>(&mut lines, ctx, ASK);
    push_live::<Mcp>(&mut lines, ctx, MCP);
    push_live::<Subagents>(&mut lines, ctx, SUBAGENTS);
    push_live::<Goal>(&mut lines, ctx, GOAL);
    push_live::<Workflows>(&mut lines, ctx, WORKFLOWS);
    push_live::<DynamicRunner>(&mut lines, ctx, DYNAMIC_CORDIS_RUNNER);
    push_live::<Memory>(&mut lines, ctx, MEMORY);
    push_live::<Browser>(&mut lines, ctx, BROWSER);
    push_live::<LspBackendAdapter>(&mut lines, ctx, LSP);
    push_live::<Skills>(&mut lines, ctx, SKILLS);
    push_live::<Computer>(&mut lines, ctx, COMPUTER);
    push_live::<Compact>(&mut lines, ctx, COMPACT);
    push_live::<Roster>(&mut lines, ctx, ROSTER);
    push_live::<LoopHandle>(&mut lines, ctx, AGENT_LOOP);
    push_live::<RhaiBags>(&mut lines, ctx, RHAI_BAGS);
    lines
}

fn push_live<T: Send + Sync + 'static>(names: &mut Vec<String>, ctx: &Context, name: &str) {
    if ctx.get::<T>(name).is_some() {
        names.push(name.to_string());
    }
}

fn describe_fibers(runner: &DynamicRunner, session_id: &str) -> Vec<String> {
    let rows = runner.live_fibers(session_id);
    if rows.is_empty() {
        return vec![
            "(no dynamic fibers; run a Package to mount a host half under cordis-dynamic)".into(),
        ];
    }
    rows.into_iter()
        .map(|row| {
            let mut line = format!("- {} [{}]", row.name, row.state);
            if let Some(id) = row.plugin_id {
                line.push_str(&format!(" pluginId={id}"));
            }
            if let Some(id) = row.package_id {
                line.push_str(&format!(" packageId={id}"));
            }
            if let Some(id) = row.plugin_run_id {
                line.push_str(&format!(" run={id}"));
            }
            if !row.provides.is_empty() {
                line.push_str(&format!(" provides: {}", row.provides.join(", ")));
            }
            if !row.waiting_for.is_empty() {
                line.push_str(&format!(" waiting for: {}", row.waiting_for.join(", ")));
            }
            line
        })
        .collect()
}

fn describe_tools(tools: &Tools) -> Vec<String> {
    let mut names: Vec<_> = tools.specs().into_iter().map(|s| s.name).collect();
    names.sort();
    if names.is_empty() {
        vec!["(no tools registered)".into()]
    } else {
        names.into_iter().map(|n| format!("- {n}")).collect()
    }
}

fn describe_temporary(runner: &DynamicRunner, session_id: &str) -> Vec<String> {
    let rows: Vec<_> = runner
        .snapshot(session_id)
        .into_iter()
        .filter(|r| matches!(r.origin, PluginOrigin::Session))
        .collect();
    if rows.is_empty() {
        return vec!["No session-local Plugins. Definitions live only in this process's memory, so a restart clears them. Lasting plugins: cordis_promote or .dock/plugins/<id>/.".into()];
    }
    describe_plugin_rows(&rows)
}

fn describe_permanent(runner: &DynamicRunner) -> Vec<String> {
    let disk = crate::tools::dynamic_runner::plugin_roots();
    let specs = {
        // scan is in persist; overlay_listing already formats for humans.
        // Compact inspect: list live persist rows plus disk scan via listing lines.
        runner.overlay_listing_from("*", &disk)
    };
    if specs.trim().is_empty() {
        return vec!["(no disk plugins)".into()];
    }
    specs.lines().map(|s| s.to_string()).collect()
}

fn describe_plugin_rows(rows: &[SnapshotRow]) -> Vec<String> {
    let mut lines = Vec::new();
    for row in rows {
        lines.push(plugin_line(row));
        for pkg in &row.packages {
            let active = row
                .active_run
                .as_ref()
                .filter(|r| r.package_id == pkg.package_id);
            match active {
                Some(run) => lines.push(format!(
                    "    - {}: {} [{}] factory={} — {}; provides: {}; waiting for: {}",
                    pkg.package_id,
                    pkg.name,
                    run.fiber_state.as_deref().unwrap_or("running"),
                    pkg.factory,
                    pkg.purpose,
                    join_or(&run.provides, "none"),
                    join_or(&run.waiting_for, "none"),
                )),
                None => lines.push(format!(
                    "    - {}: {} (factory={}) — {}",
                    pkg.package_id, pkg.name, pkg.factory, pkg.purpose
                )),
            }
        }
    }
    lines
}

fn describe_events() -> Vec<String> {
    vec![
        "- session/event — session log after append. host.on(\"session/event\", |line| { ... })".into(),
        "    line is user\\t… / assistant\\t… / tool\\tname / reminder\\t… (pre-step and prompt are skipped)".into(),
        "    handler runs asynchronously; it must not intercept tools/execute or other waterfalls".into(),
    ]
}

fn describe_factories(runner: &DynamicRunner) -> Vec<String> {
    runner
        .factories()
        .iter()
        .map(|f| {
            format!(
                "- {} — {}; provides: {}; inject: {}; tools: {}",
                f.id,
                f.purpose,
                join_or_static(f.provides, "none"),
                join_or_static(f.inject, "none"),
                join_or_static(f.tools, "none"),
            )
        })
        .collect()
}

fn describe_slots(ctx: &Context) -> Vec<String> {
    let Some(slots) = ctx.get::<TuiSlots>(TUI_SLOTS) else {
        return vec!["(tui.slots is not mounted)".into()];
    };
    let list = slots.list();
    if list.is_empty() {
        return vec!["(no slots registered)".into()];
    }
    list.into_iter()
        .map(|s| {
            format!(
                "- {} — {}{}",
                s.id,
                s.title,
                if s.hud { " (hud)" } else { "" }
            )
        })
        .collect()
}

fn plugin_line(row: &SnapshotRow) -> String {
    let current = row.current_package_id.as_deref().unwrap_or("none");
    let next = row.next_package_id.as_deref().unwrap_or("none");
    let active = match &row.active_run {
        Some(run) => format!("; active: {} as {}", run.package_id, run.plugin_run_id),
        None => "; stopped".into(),
    };
    format!(
        "- Plugin {} ({}); current: {current}; next: {next}{active}",
        row.plugin_id,
        row.origin.label()
    )
}

fn section(title: &str, lines: Vec<String>) -> String {
    format!("{title}:\n{}", lines.join("\n"))
}

fn join_or(items: &[String], empty: &str) -> String {
    if items.is_empty() {
        empty.into()
    } else {
        items.join(", ")
    }
}

fn join_or_static(items: &[&str], empty: &str) -> String {
    if items.is_empty() {
        empty.into()
    } else {
        items.join(", ")
    }
}
