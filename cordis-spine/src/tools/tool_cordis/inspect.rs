//! Read-only plugin / service / tool / hook directory for `cordis_inspect`.
//! Intersects named lookups with what is actually mounted. No generated JSDoc catalog.

use std::path::PathBuf;

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
    builtins_lines, find_disk, DiskPluginView, DynamicRunner, PersistScope, RhaiBags,
    PERSIST_SESSION,
};
use crate::tools::goal::Goal;
use crate::tools::jobs::Jobs;
use crate::tools::lsp::LspHub;
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
    roots: &[(PersistScope, PathBuf)],
    what: &str,
) -> String {
    match what {
        "plugins" => section("plugins", describe_plugins(runner, roots)),
        "services" => section("services", describe_services(ctx, runner)),
        "tools" => section("tools", describe_tools(tools)),
        "builtins" => section("builtins", builtins_lines()),
        "events" => section("events", describe_events()),
        "slots" => section("slots", describe_slots(ctx)),
        other => format!(
            "错误：不认得 what {other:?}；可选 plugins、services、tools、builtins、events、slots"
        ),
    }
}

/// 不带参数时的概览：插件在哪、各自什么状态，再加一行「还能查什么」。和 DSH 的
/// `cordis_inspect` 一样先给目录、再按需查细节——整份 services / builtins 动辄上百行，
/// 不该每次都塞进上下文。
pub fn render_overview(runner: &DynamicRunner, roots: &[(PersistScope, PathBuf)]) -> String {
    let mut out = vec![section("plugins", describe_plugins(runner, roots))];
    out.push(
        "more: cordis_inspect plugin \"<id>\" for one plugin; what: services | tools | builtins | events | slots for the reference sections."
            .into(),
    );
    out.join("\n\n")
}

fn describe_plugins(runner: &DynamicRunner, roots: &[(PersistScope, PathBuf)]) -> Vec<String> {
    let mut lines: Vec<String> = roots
        .iter()
        .map(|(scope, root)| format!("root {}: {}", scope.as_str(), root.display()))
        .collect();
    let views = runner.disk_plugins_in(roots);
    if views.is_empty() {
        lines.push(
            "(no plugins yet — write .dock/plugins/<id>/source.rhai, then cordis_plugin reload <id>)"
                .into(),
        );
        return lines;
    }
    for v in views {
        lines.push(format!(
            "- {} [{}] {} — {}{}",
            v.id,
            v.scope.as_str(),
            state_label(&v),
            v.name,
            if v.purpose.is_empty() {
                String::new()
            } else {
                format!(": {}", v.purpose)
            }
        ));
        if let Some(err) = &v.error {
            lines.push(format!("    error: {}", first_line(err)));
        }
        if let Some(other) = &v.shadowed_by {
            lines.push(format!("    shadowed by {}", other.display()));
        }
    }
    lines
}

fn state_label(v: &DiskPluginView) -> &'static str {
    if v.running && v.error.is_some() {
        "running (last reload failed)"
    } else if v.running {
        "running"
    } else if v.error.is_some() {
        "failed"
    } else if !v.enabled {
        "disabled"
    } else if v.shadowed_by.is_some() {
        "shadowed"
    } else {
        "not loaded"
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

/// 一个插件：目录、开关、正在跑的那一版登记了什么、声明的 HTTP host、最近的错误。
/// 源码不在这里——就在目录里，用 read_file 读。
pub fn render_plugin(
    runner: &DynamicRunner,
    roots: &[(PersistScope, PathBuf)],
    id: &str,
) -> Result<String, String> {
    let (spec, shadowed) = find_disk(id, None, roots)?;
    let view = runner
        .disk_plugins_in(roots)
        .into_iter()
        .find(|v| v.id == spec.id && v.scope == spec.scope);
    let row = runner.inspect_plugin(PERSIST_SESSION, &spec.id).ok();
    let loaded_here = view.as_ref().is_some_and(|v| v.loaded);
    let mut lines = vec![
        format!("plugin: {}", spec.id),
        format!("dir: {} ({})", spec.path.display(), spec.scope.as_str()),
        format!("name: {}", spec.name),
    ];
    if !spec.purpose.is_empty() {
        lines.push(format!("purpose: {}", spec.purpose));
    }
    if spec.factory != crate::tools::dynamic_runner::RHAI_FACTORY {
        lines.push(format!("factory: {}", spec.factory));
    }
    if let Some(path) = &spec.source_path {
        lines.push(format!("source: {}", path.display()));
    }
    lines.push(format!("enabled: {}", spec.enabled));
    let status = view.as_ref().map(state_label).unwrap_or("not loaded");
    lines.push(format!("status: {status}"));
    if let Some(run) = row
        .as_ref()
        .filter(|_| loaded_here)
        .and_then(|r| r.active_run.as_ref())
    {
        lines.push(format!(
            "version: {} ({})",
            run.package_id, run.plugin_run_id
        ));
        lines.push(format!("registered: {}", super::surface_line(&run.surface)));
        if !run.waiting_for.is_empty() {
            lines.push(format!("waiting for: {}", run.waiting_for.join(", ")));
        }
    }
    if !spec.http_allow.is_empty() {
        lines.push(format!(
            "http without asking: {}",
            spec.http_allow.join(", ")
        ));
    }
    if let Some(err) = view.as_ref().and_then(|v| v.error.as_ref()) {
        lines.push(format!("last error: {err}"));
    }
    if let Some(v) = view.as_ref().filter(|v| v.shadowed_by.is_some()) {
        lines.push(format!(
            "note: another plugin named {} is loaded from {}; this directory is not loaded",
            v.id,
            v.shadowed_by.as_ref().unwrap().display()
        ));
    }
    if let Some(other) = shadowed {
        lines.push(format!("note: {} is shadowed by this one", other.display()));
    }
    for w in &spec.warnings {
        lines.push(format!("warning: {w}"));
    }
    Ok(lines.join("\n"))
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
    push_live::<LspHub>(&mut lines, ctx, LSP);
    push_live::<Skills>(&mut lines, ctx, SKILLS);
    push_live::<Computer>(&mut lines, ctx, COMPUTER);
    push_live::<Compact>(&mut lines, ctx, COMPACT);
    push_live::<Roster>(&mut lines, ctx, ROSTER);
    push_live::<LoopHandle>(&mut lines, ctx, AGENT_LOOP);
    push_live::<RhaiBags>(&mut lines, ctx, RHAI_BAGS);
    push_live::<crate::host::turn_cards::TurnCards>(&mut lines, ctx, crate::names::TURN_CARDS);
    lines
}

fn push_live<T: Send + Sync + 'static>(names: &mut Vec<String>, ctx: &Context, name: &str) {
    if ctx.get::<T>(name).is_some() {
        names.push(name.to_string());
    }
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

fn describe_events() -> Vec<String> {
    vec![
        "host.on(name, |payload| { ... }) — a throw or a non-matching return counts as no opinion; the chain always continues.".into(),
        "- session/event (observe) — line: user\\t… / assistant\\t… / tool\\tname / reminder\\t…; runs after append, in order".into(),
        "- agent/step-start — #{ step, identity, main }; return a string to inject a <system-reminder> before this sample, or ()".into(),
        "- agent/turn-end — #{ text, rounds, ended_with_text, queued_followups, identity, main }; return a string to keep working, or ()".into(),
        "- agent/pre-step — #{ user, identity, main }; return #{ deny: \"reason\" } to refuse the turn".into(),
        "- system-prompt/assemble — #{ tools }; return a string to add one section".into(),
        "- tools/pre-execute — #{ called, name, arguments, identity, main }; return #{ deny: \"…\" } or #{ rewrite: #{ name, arguments } } (rewritten calls pass the permission gate again)".into(),
        "- tools/execute — #{ name, call_id, content, is_error }; return a string to replace the content".into(),
        "- llm/stream — #{ text, tool_calls }; return a string to replace the assistant text".into(),
    ]
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

fn section(title: &str, lines: Vec<String>) -> String {
    format!("{title}:\n{}", lines.join("\n"))
}
