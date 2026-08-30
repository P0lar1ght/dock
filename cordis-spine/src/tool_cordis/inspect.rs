//! Read-only live fiber / service / tool / temporary directory.
//! Intersects named lookups with what is actually mounted. No generated JSDoc catalog.

use cordis::Context;

use crate::agent_presets::AgentPresets;
use crate::agents::Agents;
use crate::ask_user::Ask;
use crate::cron::Cron;
use crate::dynamic_runner::{DynamicRunner, SnapshotRow, builtins_lines};
use crate::goal::Goal;
use crate::jobs::Jobs;
use crate::llm::Llm;
use crate::lsp::LspBackendAdapter;
use crate::mcp::Mcp;
use crate::memory::Memory;
use crate::names::{
    AGENT_PRESETS, AGENTS, ASK, CRON, DYNAMIC_CORDIS_RUNNER, GOAL, JOBS, LLM, LSP, MCP, MEMORY,
    PERMISSIONS, PLAN_MODE, SESSIONS, SETTINGS, SLASH, SUBAGENTS, SYSTEM_PROMPT, TODOS, TOOLS,
    TUI_SLOTS, TURN, WORKFLOWS,
};
use crate::permissions::Permissions;
use crate::plan_mode::PlanMode;
use crate::prompt::SystemPrompt;
use crate::session::Sessions;
use crate::settings::AppSettings;
use crate::slash::Slash;
use crate::task::Subagents;
use crate::todo_write::Todos;
use crate::tools::Tools;
use crate::tui_slots::TuiSlots;
use crate::turn::TurnControl;
use crate::workflow::Workflows;

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
    if all || what == Some("factories") {
        sections.push(section("factories", describe_factories(runner)));
    }
    if all || what == Some("builtins") {
        sections.push(section("builtins", builtins_lines()));
    }
    if all || what == Some("slots") {
        sections.push(section("slots", describe_slots(ctx)));
    }
    if sections.is_empty() {
        return format!(
            "unknown inspect what:{what:?}; use services, fibers, tools, temporary, factories, builtins, or slots"
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
                return Ok("No dynamic Plugins are defined in this session. Definitions live only in memory, so a restart clears them.".into());
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
            if let Some(source) = &pkg.source {
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

fn describe_services(ctx: &Context, runner: &DynamicRunner) -> Vec<String> {
    let mut lines = probe_spine(ctx);
    for (name, owner) in runner.live_dynamic_services() {
        lines.push(format!("- {name} (provided by {owner})"));
    }
    if lines.is_empty() {
        return vec!["(no services provided)".into()];
    }
    lines.sort();
    annotate_injectable_services(&mut lines);
    if let Some(slash) = ctx.get::<Slash>(SLASH) {
        let extras = slash.list();
        if !extras.is_empty() {
            for entry in extras {
                lines.push(format!(
                    "  - /{} ({}) — {}",
                    entry.command,
                    entry.kind.as_str(),
                    entry.description
                ));
            }
        }
    }
    lines
}

fn probe_spine(ctx: &Context) -> Vec<String> {
    let mut lines = Vec::new();
    push_live::<Sessions>(&mut lines, ctx, SESSIONS);
    push_live::<Llm>(&mut lines, ctx, LLM);
    push_live::<Tools>(&mut lines, ctx, TOOLS);
    push_live::<SystemPrompt>(&mut lines, ctx, SYSTEM_PROMPT);
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
    push_live::<LspBackendAdapter>(&mut lines, ctx, LSP);
    lines
}

fn push_live<T: Send + Sync + 'static>(lines: &mut Vec<String>, ctx: &Context, name: &str) {
    if ctx.get::<T>(name).is_some() {
        lines.push(format!("- {name}"));
    }
}

/// Rhai `inject` targets get 1–3 callable methods. Other live services stay names-only.
fn annotate_injectable_services(lines: &mut Vec<String>) {
    const ANNOTATIONS: &[(&str, &[&str])] = &[
        (
            TOOLS,
            &[
                "register / register_dynamic(spec, body) — model-facing extras; execute still applies permissions",
                "execute(call) — run a live tool by name (permissions apply)",
            ],
        ),
        (
            SLASH,
            &[
                "register(#{ command, kind, text, title?, send?, description? }) — additive only; cannot replace /agents, /help, /quit, …",
                "list() — extra commands (not the TUI builtin catalog)",
            ],
        ),
        (
            TUI_SLOTS,
            &[
                "register(id, handler) — render() / on_key(); host.register_slot",
                "request_open(id) — ask the TUI to open that overlay",
            ],
        ),
    ];
    let mut out = Vec::with_capacity(lines.len() + 8);
    for line in lines.drain(..) {
        let methods = ANNOTATIONS
            .iter()
            .find(|(name, _)| line == format!("- {name}"))
            .map(|(_, sigs)| *sigs);
        out.push(line);
        if let Some(sigs) = methods {
            for sig in sigs {
                out.push(format!("    {sig}"));
            }
        }
    }
    *lines = out;
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
    let rows = runner.snapshot(session_id);
    if rows.is_empty() {
        return vec!["No dynamic Plugins are defined in this session. Definitions live only in this process's memory, so a restart clears them.".into()];
    }
    let mut lines = Vec::new();
    for row in rows {
        lines.push(plugin_line(&row));
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
        "- Plugin {}; current: {current}; next: {next}{active}",
        row.plugin_id
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
