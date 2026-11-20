//! Rhai host-half evaluator. Compile at define; eval `apply` at run.
//! Engine setup copied from `vendor/xai/workflow` (DummyModuleResolver,
//! max ops, disable eval) — not the workflow agent-host channel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Inject, Plugin};
use rhai::{Array, Dynamic, Engine, FnPtr, ImmutableString, Map, AST};
use serde_json::Value;

use super::preview;
use crate::host::plugin_settings::PluginSettings;
use crate::host::slash::{ExtraSlashKind, Slash, SlashEntry};
use crate::host::status_items::{StatusItem, StatusItems, StatusToken};
use crate::host::tool_views::{ToolViewFn, ToolViewInput, ToolViews};
use crate::host::tui_slots::{SlotAction, SlotHandler, SlotKeyResult, TuiSlots};
use crate::names::{
    LLM_STREAM, PLUGIN_SETTINGS, PRE_STEP, PROMPT_ASSEMBLE, RHAI_BAGS, SESSION_EVENT, SLASH,
    STATUS_ITEMS, STEP_START, TOOLS, TOOLS_EXECUTE, TOOLS_PRE_EXECUTE, TOOL_VIEWS, TUI_SLOTS,
    TURN_END,
};
use crate::prompt::assemble::{PromptAssembly, ORDER_HOOKS};
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::plugin_settings::SettingsSchema;
use cordis_base::types::{
    LlmOutput, LogEvent, PreExecute, PreStep, StepStart, ToolCall, ToolResult, ToolSpec, TurnEnd,
    ORDER_STEP_START_DYNAMIC, ORDER_TURN_END_DYNAMIC,
};
use cordis_base::view::ViewNode;

/// 结构化的会话事件（Rhai 专用的钩子名，不是 ctx 事件）。
const SESSION_ENTRY: &str = "session/entry";

/// 程序侧 inline 源码上限。
pub const MAX_INLINE_SOURCE: usize = 128 * 1024;
/// 磁盘插件 `source.rhai` 上限（reload 时读成快照）。
pub const MAX_FILE_SOURCE: usize = 1024 * 1024;
/// Over the inline ceiling the answer is never "shorten the script" — it is the
/// file route, which is the documented default for anything real anyway.
pub const INLINE_TOO_LARGE: &str = "rhai source exceeds 128KiB — write it to .dock/plugins/<id>/source.rhai (≤1MiB) and cordis_plugin reload that id.";
pub const FILE_TOO_LARGE: &str = "source.rhai exceeds 1MiB — move the bulk out of the script (read it at runtime with host.read_bytes) or split it into two plugins.";
const DEFINE_MAX_OPS: u64 = 100_000;
const RUN_MAX_OPS: u64 = 1_000_000;

const SOURCE_SHAPE_HINT: &str = "source must be a map #{ inject: [...], apply: |host| { ... } }; define compiles and does not call apply.";

const PARAMETERS_MUST_BE_MAP: &str = "host.register_tool parameters must be a map, not a JSON string. Use:\n\
    parameters: #{ type: \"object\", properties: #{ text: #{ type: \"string\" } }, required: [\"text\"] }";

const READ_BYTES_BUILTIN: (&str, &str, &[&str]) = (
    "host.read_bytes",
    "Read a file into a Rhai Blob for http_request body/multipart. Path only (like read_file): relative to cwd, absolute, or `~/…`. One file per call — no directory walks, globs or writes. First use gates `read_bytes {path}` (per path; the prompt shows the path, never the bytes). Empty files OK; over 16MiB throws. Bytes read here are the **only** way to send a request body over 1MiB — script-built payloads stay capped at 1MiB. Runtime Host only — not available at define-time preflight. Prefer this over embedding file content in tool args.",
    &[
        "host.read_bytes(\"data.csv\") -> Blob",
        "http_request(#{ method: \"POST\", url, multipart: [#{ name: \"file\", blob: host.read_bytes(args.path), filename: \"data.csv\" }] })",
    ],
);

pub const HOST_BUILTINS: &[(&str, &str, &[&str])] = &[
    (
        "host.provide",
        "Mount an in-memory JSON map under a named service (RhaiBag). Stop unregisters it.",
        &["host.provide(name: String, value: Map)"],
    ),
    (
        "host.get",
        "Read a RhaiBag as a map, true if that name is live (tools/slash/tui.slots or another bag), otherwise ().",
        &["host.get(name: String) -> Map | true | ()"],
    ),
    (
        "host.register_tool",
        "Register a model-facing tool. execute is |args| -> String. parameters is a JSON-schema map (not a string). Stop unregisters it. Dynamic tools bypass the Agent preset allowlist.",
        &[
            "host.register_tool(#{ name, description, parameters: #{ type: \"object\", properties: #{ ... }, required: [...] }, execute })",
        ],
    ),
    (
        "host.register_slash",
        "Append a prompt-bar command (prompt / overlay / slot / tool). tool: text=tool name; typed args → JSON. Reserved builtin names (/agents, /help, /quit, …) fail before the extra is inserted. A throw later in apply still rolls back earlier provide/register_*.",
        &["host.register_slash(#{ command, kind, text, title?, send?, description? })"],
    ),
    (
        "host.register_slot",
        "Register a slot (terminal overlay, GUI panel): render() -> String, on_key(key) -> \"close\" | (). actions: buttons whose id is passed to on_key (GUI clicks, terminal keys). view() -> a dock.view.1 tree (docs/PLUGIN-VIEWS.md); when present both clients draw it instead of render(), and its buttons call on_key(action) too (terminal: digit keys 1–9). web: \"panel.html\" (disk plugins only, a file inside the plugin directory) gives the GUI a sandboxed HTML/JS panel instead; the terminal keeps drawing view() / render(). See docs/PLUGIN-VIEWS.md (web panels) for the bridge API.",
        &["host.register_slot(#{ id, title, hud?, render, on_key, actions?: [#{ id, label }], view?: || #{ type: \"stack\", children: [...] }, web?: \"panel.html\" })"],
    ),
    (
        "host.slot_changed",
        "Tell clients a slot's body changed (GUI re-fetches; the terminal redraws every frame anyway).",
        &["host.slot_changed(id: String)"],
    ),
    (
        "host.open_slot",
        "Ask the TUI to open a registered slot overlay.",
        &["host.open_slot(id: String)"],
    ),
    (
        "host.register_tool_view",
        "Draw a tool's card as a dock.view.1 tree (docs/PLUGIN-VIEWS.md) in the terminal and the GUI instead of raw output. view(tc) gets #{ name, arguments, output, failed } (`call` is a reserved word in Rhai) and returns a view map, or () for the plain card. Also accepted as `view` on host.register_tool. UI only — never reaches the model.",
        &["host.register_tool_view(name: String, |tc| #{ type: \"kv\", items: [...] })"],
    ),
    (
        "host.register_settings",
        "Declare this plugin's settings card (docs/PLUGIN-VIEWS.md): the GUI settings page and the terminal `/cordis set` build a form from it. Field types: string / text / number (min, max) / boolean / select (options) / secret (stored in secrets.json under the field key; read it with host.secret(key)). One card per plugin; removed when the package stops. `default` is a Rhai keyword — quote it: `\"default\": 10`.",
        &["host.register_settings(#{ title, fields: [#{ key, type, label?, description?, default?, required?, options?, min?, max? }] })"],
    ),
    (
        "host.setting",
        "Read a non-secret field of this plugin's settings card: the saved value, else the field default, else (). Secrets: use host.secret(key).",
        &["host.setting(key: String) -> value | ()"],
    ),
    (
        "host.set_status",
        "Put a small always-on status item on the terminal status row and the GUI (docs/PLUGIN-VIEWS.md). First call registers it (removed when the package stops); later calls with the same id update it. tone: default / muted / accent / success / warning / danger; surface: slot id to open on click.",
        &["host.set_status(#{ id, text, tone?, tooltip?, surface? })"],
    ),
    (
        "host.clear_status",
        "Remove a status item set by host.set_status.",
        &["host.clear_status(id: String)"],
    ),
    (
        "host.call_tool",
        "Execute a live model tool (permissions still apply), on the page that triggered this script. Any tool in the table works, MCP ones included (mcp_<server>__<tool>) — reach for a tool before asking for a new host function. call_tool returns the text; call_tool_result also says whether it failed (is_error: refused, blocked, errored, non-zero exit) and parses the text when it is JSON (json, else ()). name is the tool that actually ran; images is how many images it returned.",
        &[
            "host.call_tool(name: String, args: Map | String) -> String",
            "host.call_tool_result(name: String, args: Map | String) -> #{ name, text, json, is_error, images }",
        ],
    ),
    (
        "host.followup",
        "Send the agent a message from this plugin. It goes into a page's mailbox: a running turn picks it up before its next sample; an idle main page is woken to start a turn (at most once per 5 seconds per page — extra messages wait for the next turn). Without identity: the page that triggered this script, else the last main page this plugin saw. Returns \"woke\" or \"queued\". Runtime only.",
        &["host.followup(text: String) -> String", "host.followup(identity: String, text: String) -> String"],
    ),
    (
        "host.every / host.after",
        "Timers. every(seconds, || { ... }) repeats (min 5s); after(seconds, || { ... }) runs once (min 1s). At most 16 per plugin run; stopped with the plugin. Callbacks run off-turn: use host.followup to involve the agent.",
        &["host.every(seconds: i64 | f64, || { ... })", "host.after(seconds: i64 | f64, || { ... })"],
    ),
    (
        "host.llm",
        "Ask the current page's model one question and get its text back (no tools, not logged in the session, not counted in /usage). Blocks the caller: inside a hook it holds the turn. Throws on request failure. Runtime only.",
        &["host.llm(prompt: String) -> String", "host.llm(#{ system?, prompt }) -> String"],
    ),
    (
        "host.card / host.update_card / host.remove_card",
        "Hang a card under the current turn's answer (GUI; the terminal shows it once as a notice). spec: title, text?, tone? (default / muted / accent / success / warning / danger), view? (a dock.view.1 map, docs/PLUGIN-VIEWS.md), actions? [#{ id, label }], on_action? |action| { ... } — runs on the card's page (host.followup / call_tool land there); return a string to show the user a toast. Re-posting the same id in the same turn updates it; update_card / remove_card target the latest card with that id. Cards outlive the plugin run; their buttons stop working when it stops. Needs a main page: call from a tool or hook, or after one has run.",
        &["host.card(#{ id?, title, text?, tone?, view?, actions?, on_action? }) -> String", "host.update_card(id: String, #{ title?, text?, tone?, view?, actions? })", "host.remove_card(id: String)"],
    ),
    (
        "host.load / host.save / host.forget",
        "Small persistent key-value store for this plugin, kept across restarts in $DOCK_HOME/plugin-state/ (≤1MiB in total, JSON values). Programmatic packages without a plugin directory keep it in memory only.",
        &["host.load(key: String) -> value | ()", "host.save(key: String, value)", "host.forget(key: String)"],
    ),
    (
        "host.on",
        "Observe \"session/event\" (a short line: user\\t… / assistant\\t… / tool\\tname / reminder\\t…) or \"session/entry\" (structured: #{ kind: user|assistant|tool|reminder|notice|turn-end, identity, main, replay, … }; assistant entries carry the final text, tool_calls, model, usage #{ prompt_tokens, completion_tokens, cached_prompt_tokens, … }, cache_hit, elapsed_ms, cost_usd; tool entries carry name, arguments, args, content, is_error), both handled after emit returns, one at a time in emit order; or intercept the scriptable waterfalls. \"agent/step-start\" (#{ step, identity, main }) runs before every sample and \"agent/turn-end\" (#{ text, rounds, ended_with_text, queued_followups, identity, main }) runs when the turn wants to end; return a <system-reminder> string to inject / keep working, or () for no opinion. Five more hooks are for plugins loaded from a plugins directory (.dock/plugins/<id>/ via cordis_plugin reload or autoload; a package defined programmatically cannot register them): \"agent/pre-step\" (#{ user, identity, main }) — return #{ deny: \"reason\" } to refuse the turn; \"system-prompt/assemble\" (#{ tools }) — return a string to add one section to the system prompt; \"tools/pre-execute\" (#{ called, name, arguments, args, identity, main }) — return #{ deny: \"reason\" } to refuse the call, #{ ask: \"reason\" } to require the user's go-ahead even under auto-approve, or #{ rewrite: #{ name, arguments } } to redirect it; \"tools/execute\" (#{ name, call_id, arguments, args, content, is_error, identity, main }) — return a string to replace the content; \"llm/stream\" (#{ text, tool_calls }) — return a string to replace the assistant text. Hooks run inline and block the turn, bounded by max_operations; a throw is treated as no opinion. Built-in slots outrank a script. Stop unregisters the listener.",
        &[
            "host.on(\"session/event\", |line| { ... })",
            "host.on(\"agent/step-start\", |step| { ... })",
            "host.on(\"agent/turn-end\", |end| { ... })",
            "host.on(\"tools/pre-execute\", |call| { if call.name == \"bash\" { #{ deny: \"no shell here\" } } else { () } })",
            "host.on(\"system-prompt/assemble\", |ctx| { \"project rules…\" })",
        ],
    ),
    (
        "host.log",
        "Tagged host log, also used by print().",
        &["host.log(message: String)"],
    ),
    super::rhai_http::HTTP_BUILTIN,
    super::rhai_secret::SECRET_BUILTIN,
    READ_BYTES_BUILTIN,
];

pub struct RhaiMeta {
    pub inject: Vec<String>,
}

/// Index of live RhaiBag names for inspect.
#[derive(Clone, Default)]
pub struct RhaiBags {
    names: Arc<Mutex<Vec<(String, String)>>>,
}

impl RhaiBags {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list(&self) -> Vec<(String, String)> {
        self.names.lock().unwrap().clone()
    }

    fn insert(&self, name: String, plugin_id: String) -> cordis::Disposable {
        self.names.lock().unwrap().push((name.clone(), plugin_id));
        let names = self.names.clone();
        cordis::Disposable::from_fn(move || {
            names.lock().unwrap().retain(|(n, _)| n != &name);
        })
    }
}

/// JSON bag provided under a caller-chosen service name.
#[derive(Clone)]
pub struct RhaiBag {
    data: Arc<Mutex<Value>>,
}

impl RhaiBag {
    pub fn get(&self) -> Value {
        self.data.lock().unwrap().clone()
    }
}

pub fn sandboxed_engine(max_ops: u64) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(max_ops);
    engine.set_max_call_levels(64);
    engine.set_max_expr_depths(128, 64);
    // Strings stay at 1MiB — that is the ceiling for script-built request bodies.
    engine.set_max_string_size(1024 * 1024);
    // Rhai measures arrays and Blobs with the same knob, and a Blob element is
    // one byte, so this is also the ceiling on `host.read_bytes`. It has to clear
    // the file-upload limit or a file reference could never reach the script.
    // Script-built arrays stay bounded by `set_max_operations` above: growing one
    // costs an operation per element, so the ops cap bites long before this does.
    engine.set_max_array_size(super::path_bytes::MAX_READ_BYTES);
    engine.set_max_map_size(16_384);
    engine.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());
    engine.disable_symbol("eval");
    engine
}

pub fn preflight(source: &str) -> Result<RhaiMeta, String> {
    preflight_limited(source, MAX_INLINE_SOURCE)
}

pub fn preflight_limited(source: &str, max_bytes: usize) -> Result<RhaiMeta, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("factory \"rhai\" needs non-empty `source`".into());
    }
    if source.len() > max_bytes {
        if max_bytes <= MAX_INLINE_SOURCE {
            return Err(INLINE_TOO_LARGE.into());
        }
        return Err(FILE_TOO_LARGE.into());
    }
    let engine = sandboxed_engine(DEFINE_MAX_OPS);
    engine
        .compile(source)
        .map_err(|e| format!("rhai syntax: {e}\n{SOURCE_SHAPE_HINT}"))?;
    let value: Dynamic = engine
        .eval(source)
        .map_err(|e| format!("rhai define: {e}"))?;
    let map = value.try_cast::<Map>().ok_or_else(|| {
        "rhai source must evaluate to a map #{ inject: [...], apply: |host| { ... } }".to_string()
    })?;
    if map
        .get("apply")
        .cloned()
        .and_then(|d| d.try_cast::<FnPtr>())
        .is_none()
    {
        return Err("rhai map needs `apply` as a function |host| { ... }".into());
    }
    Ok(RhaiMeta {
        inject: parse_inject(&map),
    })
}

/// 一次运行的外部条件。
#[derive(Clone, Default)]
pub struct RunScope {
    /// 磁盘插件的目录：`register_slot` 的 `web` 文件只能在这里面。
    pub plugin_dir: Option<PathBuf>,
    /// 从插件目录装载的（启动、开项目、设置启用、批准过的 reload）。只有这样的
    /// 包能挂那五个能否决流程的钩子。
    pub disk_backed: bool,
    /// `plugin.toml` 声明过的 HTTP host：请求它们不再逐次确认。
    pub http_allow: Vec<String>,
    /// 这次运行登记了什么，每登记一样记一笔。
    pub surface: super::registry::SurfaceCell,
}

pub fn build_rhai_limited(
    fiber_name: &str,
    source: &str,
    max_bytes: usize,
    scope: RunScope,
) -> Result<Plugin, String> {
    let meta = preflight_limited(source, max_bytes)?;
    let name = fiber_name.to_string();
    let source = source.to_string();
    let inject = if meta.inject.is_empty() {
        Inject::new()
    } else {
        Inject::from(meta.inject.clone())
    };
    Ok(plugin(name.clone(), inject, move |ctx, _: &()| {
        apply_rhai(ctx, &name, &source, scope.clone()).map_err(cordis::Error::plugin)?;
        Ok(None)
    }))
}

fn apply_rhai(ctx: &Context, plugin_id: &str, source: &str, scope: RunScope) -> Result<(), String> {
    let RunScope {
        plugin_dir,
        disk_backed,
        http_allow,
        surface,
    } = scope;
    let mut engine = sandboxed_engine(RUN_MAX_OPS);
    let tag = format!("[cordis:{plugin_id}]");
    engine.on_print(move |s| eprintln!("{tag} {s}"));
    register_host(&mut engine);
    // 只挂在**运行期**引擎上。define 期的 `preflight` 会 eval 源码顶层，挂上去
    // 等于编译检查本身就能发请求——那是权限门够不着的时机。
    let http_params = ctx
        .get::<super::DynamicRunner>(crate::names::DYNAMIC_CORDIS_RUNNER)
        .map(|r| r.http_params())
        .unwrap_or_else(crate::tools::web_fetch::web_fetch_params);
    super::rhai_http::register(
        &mut engine,
        ctx.clone(),
        plugin_id.to_string(),
        http_params,
        http_allow,
    );
    // Codecs + HMAC: pure, no perms — still runtime-only so define-time preflight cannot call them.
    super::rhai_codec::register(&mut engine);
    super::rhai_regex::register(&mut engine);
    // Wall clock: SystemTime epoch helpers (timestamp() is Instant-only).
    super::rhai_time::register(&mut engine);
    // Pure ecosystem packages (rhai-url, rhai-rand): runtime-only like the codecs.
    super::rhai_packages::register(&mut engine);
    let ast = engine
        .compile(source)
        .map_err(|e| format!("rhai syntax: {e}"))?;
    let engine = Arc::new(engine);
    let plugin_dir_for_state = plugin_dir.clone();
    let inner = Arc::new(HostInner {
        ctx: ctx.clone(),
        plugin_id: plugin_id.to_string(),
        plugin_dir,
        disk_backed,
        surface,
        last_page: Arc::default(),
        cards_owned: std::sync::atomic::AtomicBool::new(false),
        last_wake: Mutex::new(HashMap::new()),
        timers: std::sync::atomic::AtomicUsize::new(0),
        store_path: state_path(plugin_id, plugin_dir_for_state.as_deref()),
        store: Mutex::new(None),
        engine: engine.clone(),
        ast: ast.clone(),
        disposers: Mutex::new(Vec::new()),
        statuses: Mutex::new(HashMap::new()),
    });
    let host = Host {
        inner: inner.clone(),
    };
    let value: Dynamic = engine
        .eval_ast(&ast)
        .map_err(|e| format!("rhai eval: {e}"))?;
    let map = value
        .try_cast::<Map>()
        .ok_or_else(|| "rhai source must evaluate to a map #{ inject, apply }".to_string())?;
    let apply = map
        .get("apply")
        .cloned()
        .and_then(|d| d.try_cast::<FnPtr>())
        .ok_or("rhai map needs `apply` as a function")?;
    // Own whatever apply registered even when it throws. `register_*` mutates the
    // live tools/slash/slots maps immediately; the Disposable only lives on the
    // fiber after `own` / `own_registered`. Skipping that on error left orphans
    // that stopping the plugin could not see (`run` was never set).
    let apply_err = apply
        .call::<Dynamic>(engine.as_ref(), &ast, (host,))
        .err()
        .map(|e| format!("rhai apply: {e}"));
    let leftover: Vec<_> = inner.disposers.lock().unwrap().drain(..).collect();
    if !leftover.is_empty() {
        own_registered(ctx, leftover).map_err(|e| e.to_string())?;
    }
    if let Some(e) = apply_err {
        return Err(e);
    }
    Ok(())
}

fn parse_inject(map: &Map) -> Vec<String> {
    let Some(value) = map.get("inject") else {
        return vec![TOOLS.into()];
    };
    let Some(arr) = value.clone().try_cast::<Array>() else {
        return vec![TOOLS.into()];
    };
    arr.into_iter()
        .filter_map(|d| d.into_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn register_host(engine: &mut Engine) {
    engine.register_type_with_name::<Host>("Host");
    engine.register_fn("provide", Host::provide);
    engine.register_fn("get", Host::get);
    engine.register_fn("register_tool", Host::register_tool);
    engine.register_fn("register_slash", Host::register_slash);
    engine.register_fn("register_slot", Host::register_slot);
    engine.register_fn("open_slot", Host::open_slot);
    engine.register_fn("slot_changed", Host::slot_changed);
    engine.register_fn("set_status", Host::set_status);
    engine.register_fn("register_settings", Host::register_settings);
    engine.register_fn("setting", Host::setting);
    engine.register_fn("register_tool_view", Host::register_tool_view);
    engine.register_fn("clear_status", Host::clear_status);
    engine.register_fn("call_tool", Host::call_tool);
    engine.register_fn("call_tool_result", Host::call_tool_result);
    engine.register_fn("followup", Host::followup);
    engine.register_fn("followup", Host::followup_to);
    engine.register_fn("every", Host::every_int);
    engine.register_fn("every", Host::every_float);
    engine.register_fn("after", Host::after_int);
    engine.register_fn("after", Host::after_float);
    engine.register_fn("llm", Host::llm);
    engine.register_fn("llm", Host::llm_map);
    engine.register_fn("load", Host::load);
    engine.register_fn("save", Host::save);
    engine.register_fn("forget", Host::forget);
    engine.register_fn("card", Host::card);
    engine.register_fn("update_card", Host::update_card);
    engine.register_fn("remove_card", Host::remove_card);
    engine.register_fn("on", Host::on);
    engine.register_fn("log", Host::log);
    engine.register_fn("secret", Host::secret);
    engine.register_fn("read_bytes", Host::read_bytes);
}

#[derive(Clone)]
struct Host {
    inner: Arc<HostInner>,
}

struct HostInner {
    ctx: Context,
    plugin_id: String,
    /// 磁盘插件所在目录：`register_slot` 的 `web` 文件只能在这里面。
    plugin_dir: Option<PathBuf>,
    /// 这个包是不是从插件目录装载的。只有这样的包能挂那五个能否决流程的钩子；
    /// 程序侧 define 的会话包打不开。
    disk_backed: bool,
    /// 这次运行登记了什么（reload 回执、`cordis_inspect` 读它）。
    surface: super::registry::SurfaceCell,
    /// 最近和这颗插件打过交道的主会话页（钩子载荷、工具调用里认出来的）。定时器
    /// 这类不在任何一轮里的回调 `host.followup` 时送到这里。
    last_page: Arc<Mutex<Option<Context>>>,
    /// 这次运行发过卡片：停的时候要把它们的按钮摘掉（卡片本身留着）。
    cards_owned: std::sync::atomic::AtomicBool,
    /// 每一页上次被这颗插件叫醒的时刻：一页 5 秒内最多叫醒一次。
    last_wake: Mutex<HashMap<String, std::time::Instant>>,
    timers: std::sync::atomic::AtomicUsize,
    /// 持久存储的文件；`None` = 程序侧 define 的包，只在内存里。
    store_path: Option<PathBuf>,
    store: Mutex<Option<serde_json::Map<String, Value>>>,
    engine: Arc<Engine>,
    ast: AST,
    disposers: Mutex<Vec<cordis::Disposable>>,
    /// 这个包登记的状态项 id → 凭据：只改 / 删自己的，别的包同 id 的碰不到。
    statuses: Mutex<HashMap<String, StatusToken>>,
}

impl Host {
    fn provide(
        &mut self,
        name: ImmutableString,
        value: Dynamic,
    ) -> Result<(), Box<rhai::EvalAltResult>> {
        let name = name.to_string();
        if name.is_empty() {
            return err("provide needs a non-empty name");
        }
        let json = dynamic_to_json(&value).map_err(eval_err)?;
        let bag = RhaiBag {
            data: Arc::new(Mutex::new(json)),
        };
        let provided = self
            .inner
            .ctx
            .provide(&name, bag)
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(provided)?;
        self.note(|s| super::registry::Surface::push(&mut s.provides, &name));
        if let Some(bags) = self.inner.ctx.get::<RhaiBags>(RHAI_BAGS) {
            self.own(bags.insert(name, self.inner.plugin_id.clone()))?;
        }
        Ok(())
    }

    fn get(&mut self, name: ImmutableString) -> Dynamic {
        let name = name.to_string();
        if let Some(bag) = self.inner.ctx.get::<RhaiBag>(&name) {
            return json_to_dynamic(&bag.get());
        }
        if rhai_name_live(&self.inner.ctx, &name) {
            return Dynamic::TRUE;
        }
        Dynamic::UNIT
    }

    fn register_tool(&mut self, spec: Map) -> Result<(), Box<rhai::EvalAltResult>> {
        let tools = self
            .inner
            .ctx
            .get::<Tools>(TOOLS)
            .ok_or_else(|| eval_err("tools is not mounted; inject [\"tools\"]".into()))?;
        let name =
            map_str(&spec, "name").ok_or_else(|| eval_err("register_tool needs name".into()))?;
        let description = map_str(&spec, "description").unwrap_or_else(|| name.clone());
        let parameters = tool_parameters_json(&spec).map_err(eval_err)?;
        let execute = spec
            .get("execute")
            .cloned()
            .and_then(|d| d.try_cast::<FnPtr>())
            .ok_or_else(|| eval_err("register_tool needs execute as a function".into()))?;
        let engine = self.inner.engine.clone();
        let ast = self.inner.ast.clone();
        let last_page_outer = self.inner.last_page.clone();
        let body: ToolBody = Arc::new(move |call| {
            let engine = engine.clone();
            let ast = ast.clone();
            let execute = execute.clone();
            let last_page = last_page_outer.clone();
            Box::pin(async move {
                let arguments = call.arguments.clone();
                // 脚本跑在阻塞线程上，task-local 的「当前页」跟不过去。在这里抓住，
                // 进线程再挂上：脚本里的 `host.call_tool` / HTTP 确认才会落到发起
                // 调用的那一页，而不是插件 fiber 所在的根（GUI 里那一页多半没人看）。
                let page = crate::tools::registry::exec_ctx();
                remember_page(&last_page);
                let content = match tokio::task::spawn_blocking(move || {
                    let run =
                        || call_fnptr(&engine, &ast, &execute, json_str_to_dynamic(&arguments));
                    match page {
                        Some(page) => crate::tools::registry::with_exec_ctx(&page, run),
                        None => run(),
                    }
                })
                .await
                {
                    Ok(Ok(text)) => text,
                    Ok(Err(e)) => format!("Error: {e}"),
                    Err(e) => format!("Error: {e}"),
                };
                tool_result(call, content)
            })
        });
        let view = spec
            .get("view")
            .cloned()
            .and_then(|d| d.try_cast::<FnPtr>());
        let d = tools
            .register_dynamic(
                ToolSpec {
                    name: name.clone(),
                    description,
                    parameters_json: parameters,
                },
                body,
            )
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        self.note(|s| super::registry::Surface::push(&mut s.tools, &name));
        // `view` 写在工具上 = 给自己登记卡片视图。登记不上 apply 就失败，整个作用域
        // 回滚，刚挂上的工具跟着卸下（`register_tool_with_a_taken_view_leaves_no_tool_behind`）。
        if let Some(view) = view {
            self.add_tool_view(&name, view)?;
        }
        Ok(())
    }

    /// 给任意工具（自己注册的或别的）登记卡片视图：`|tc| #{ ... }`，`tc` 是
    /// `#{ name, arguments, output, failed }`（`arguments` 能解析就是 map）。
    fn register_tool_view(
        &mut self,
        name: ImmutableString,
        view: FnPtr,
    ) -> Result<(), Box<rhai::EvalAltResult>> {
        self.add_tool_view(name.as_str(), view)
    }

    fn add_tool_view(&mut self, name: &str, view: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        let views = self
            .inner
            .ctx
            .get::<ToolViews>(TOOL_VIEWS)
            .ok_or_else(|| eval_err("tool.views is not mounted".into()))?;
        let engine = self.inner.engine.clone();
        let ast = self.inner.ast.clone();
        let render: ToolViewFn = Arc::new(move |input: &ToolViewInput<'_>| {
            let mut call = Map::new();
            call.insert("name".into(), Dynamic::from(input.name.to_string()));
            call.insert("arguments".into(), json_str_to_dynamic(input.arguments));
            call.insert("output".into(), Dynamic::from(input.output.to_string()));
            call.insert("failed".into(), Dynamic::from(input.failed));
            match call_fnptr_raw(&engine, &ast, &view, Dynamic::from_map(call)) {
                // `()` = 这次不给视图，用通用卡片。
                Ok(tree) if tree.is_unit() => None,
                Ok(tree) => Some(match dynamic_to_json(&tree) {
                    Ok(json) => ViewNode::parse(&json),
                    Err(e) => view_error(&e),
                }),
                Err(e) => Some(view_error(&e)),
            }
        });
        let d = views
            .register(name, render)
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)
    }

    fn register_slash(&mut self, spec: Map) -> Result<(), Box<rhai::EvalAltResult>> {
        let command = map_str(&spec, "command")
            .ok_or_else(|| eval_err("register_slash needs command".into()))?;
        let slash = self
            .inner
            .ctx
            .get::<Slash>(SLASH)
            .ok_or_else(|| eval_err("slash is not mounted; inject [\"slash\"]".into()))?;
        if slash.is_builtin(&command) {
            let n = command.trim().trim_start_matches('/');
            return err(format!(
                "cannot shadow builtin slash /{n} — extras cannot replace /agents, /help, /quit, …; pick a different command"
            ));
        }
        let kind = ExtraSlashKind::parse(
            &map_str(&spec, "kind").ok_or_else(|| eval_err("register_slash needs kind".into()))?,
        )
        .map_err(eval_err)?;
        let text =
            map_str(&spec, "text").ok_or_else(|| eval_err("register_slash needs text".into()))?;
        let description = map_str(&spec, "description").unwrap_or_else(|| command.clone());
        let title = map_str(&spec, "title").unwrap_or_default();
        let send = spec
            .get("send")
            .and_then(|d| d.clone().try_cast::<bool>())
            .unwrap_or(kind == ExtraSlashKind::Prompt);
        let noted = command.trim().trim_start_matches('/').to_string();
        let d = slash
            .register(SlashEntry {
                command,
                description,
                kind,
                text,
                title,
                send,
            })
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        self.note(|s| super::registry::Surface::push(&mut s.slash, &noted));
        Ok(())
    }

    fn plugin_settings(&self) -> Result<Arc<PluginSettings>, Box<rhai::EvalAltResult>> {
        self.inner
            .ctx
            .get::<PluginSettings>(PLUGIN_SETTINGS)
            .ok_or_else(|| eval_err("plugin.settings is not mounted".into()))
    }

    fn register_settings(&mut self, spec: Map) -> Result<(), Box<rhai::EvalAltResult>> {
        let settings = self.plugin_settings()?;
        let json = dynamic_to_json(&Dynamic::from_map(spec)).map_err(eval_err)?;
        let schema = SettingsSchema::parse(&json).map_err(eval_err)?;
        let d = settings
            .register(&self.inner.plugin_id, schema)
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        self.note(|s| s.settings = true);
        Ok(())
    }

    fn setting(&mut self, key: ImmutableString) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
        let settings = self.plugin_settings()?;
        Ok(settings
            .get(&self.inner.plugin_id, key.as_str())
            .map(|v| json_to_dynamic(&v))
            .unwrap_or(Dynamic::UNIT))
    }

    fn status_items(&self) -> Result<Arc<StatusItems>, Box<rhai::EvalAltResult>> {
        self.inner
            .ctx
            .get::<StatusItems>(STATUS_ITEMS)
            .ok_or_else(|| eval_err("status.items is not mounted".into()))
    }

    /// 第一次登记（随包卸下注销），之后同 id 只改内容。
    fn set_status(&mut self, spec: Map) -> Result<(), Box<rhai::EvalAltResult>> {
        let status = self.status_items()?;
        let id = map_str(&spec, "id").ok_or_else(|| eval_err("set_status needs id".into()))?;
        let text =
            map_str(&spec, "text").ok_or_else(|| eval_err("set_status needs text".into()))?;
        let item = StatusItem {
            tone: cordis_base::view::Tone::from_name(&map_str(&spec, "tone").unwrap_or_default()),
            tooltip: map_str(&spec, "tooltip"),
            surface: map_str(&spec, "surface"),
            ..StatusItem::new(id.clone(), text)
        };
        let owned = self.inner.statuses.lock().unwrap().get(&id).copied();
        if let Some(token) = owned {
            if status.update(token, item.clone()) {
                return Ok(());
            }
        }
        // 第一次（或自己那项被清掉了）：登记一项，随包卸下注销。
        let (token, d) = status
            .register(item)
            .map_err(|e| eval_err(format!("{e}（别的插件占用了这个 id？换个带包名前缀的 id）")))?;
        self.inner.statuses.lock().unwrap().insert(id, token);
        self.own(d)
    }

    fn clear_status(&mut self, id: ImmutableString) -> Result<(), Box<rhai::EvalAltResult>> {
        let token = self.inner.statuses.lock().unwrap().remove(id.as_str());
        if let Some(token) = token {
            self.status_items()?.remove(token, id.as_str());
        }
        Ok(())
    }

    fn register_slot(&mut self, spec: Map) -> Result<(), Box<rhai::EvalAltResult>> {
        let slots =
            self.inner.ctx.get::<TuiSlots>(TUI_SLOTS).ok_or_else(|| {
                eval_err("tui.slots is not mounted; inject [\"tui.slots\"]".into())
            })?;
        let id = map_str(&spec, "id").ok_or_else(|| eval_err("register_slot needs id".into()))?;
        let title = map_str(&spec, "title").unwrap_or_else(|| id.clone());
        let hud = spec
            .get("hud")
            .and_then(|d| d.clone().try_cast::<bool>())
            .unwrap_or(false);
        let render = spec
            .get("render")
            .cloned()
            .and_then(|d| d.try_cast::<FnPtr>())
            .ok_or_else(|| eval_err("register_slot needs render as a function".into()))?;
        let on_key = spec
            .get("on_key")
            .cloned()
            .and_then(|d| d.try_cast::<FnPtr>());
        let actions = slot_actions(&spec)?;
        let view = spec
            .get("view")
            .cloned()
            .and_then(|d| d.try_cast::<FnPtr>());
        let web = match map_str(&spec, "web") {
            Some(rel) => Some(self.web_file(&rel).map_err(eval_err)?),
            None => None,
        };
        let handler = Arc::new(RhaiSlot {
            title,
            hud,
            actions,
            view,
            web,
            engine: self.inner.engine.clone(),
            ast: self.inner.ast.clone(),
            render,
            on_key,
        });
        let d = slots
            .register(id.clone(), handler)
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        self.note(|s| super::registry::Surface::push(&mut s.slots, &id));
        Ok(())
    }

    /// `register_slot` 的 `web: "panel.html"`：插件目录里的一个文件（不许绝对路径、`..`、
    /// 也不许经软链跑出插件目录）。只有磁盘插件有目录。
    fn web_file(&self, rel: &str) -> Result<PathBuf, String> {
        let dir = self
            .inner
            .plugin_dir
            .as_ref()
            .ok_or("web 面板只能用在磁盘插件里（HTML 放在 .dock/plugins/<id>/ 里）")?;
        let rel_path = Path::new(rel.trim());
        if rel.trim().is_empty()
            || rel_path.is_absolute()
            || rel_path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(format!("web 要写插件目录里的相对路径：{rel}"));
        }
        let path = dir
            .join(rel_path)
            .canonicalize()
            .map_err(|e| format!("web 文件 {rel}：{e}"))?;
        let dir = dir.canonicalize().map_err(|e| format!("插件目录：{e}"))?;
        if !path.starts_with(&dir) || !path.is_file() {
            return Err(format!("web 文件要在插件目录里：{rel}"));
        }
        Ok(path)
    }

    fn slot_changed(&mut self, id: ImmutableString) -> Result<(), Box<rhai::EvalAltResult>> {
        let slots = self
            .inner
            .ctx
            .get::<TuiSlots>(TUI_SLOTS)
            .ok_or_else(|| eval_err("tui.slots is not mounted".into()))?;
        slots.changed(id.as_str());
        Ok(())
    }

    fn open_slot(&mut self, id: ImmutableString) -> Result<(), Box<rhai::EvalAltResult>> {
        let slots = self
            .inner
            .ctx
            .get::<TuiSlots>(TUI_SLOTS)
            .ok_or_else(|| eval_err("tui.slots is not mounted".into()))?;
        slots.request_open(id.as_str()).map_err(eval_err)
    }

    fn call_tool(
        &mut self,
        name: ImmutableString,
        args: Dynamic,
    ) -> Result<ImmutableString, Box<rhai::EvalAltResult>> {
        Ok(self.run_tool(name, args)?.content.into())
    }

    /// 同 `call_tool`，但把这次调用的结论一起交回来：`text` 是同一份文本，`is_error`
    /// 是注册表给的成败（被拒、被拦、报错、非零退出都算），`json` 是文本恰好是 JSON
    /// 时解析好的值（很多 MCP 工具回的就是 JSON），不是就 `()`。
    /// `call_tool` 只给文本，脚本只能按开头猜成败。
    fn call_tool_result(
        &mut self,
        name: ImmutableString,
        args: Dynamic,
    ) -> Result<Map, Box<rhai::EvalAltResult>> {
        Ok(tool_result_map(&self.run_tool(name, args)?))
    }

    fn run_tool(
        &self,
        name: ImmutableString,
        args: Dynamic,
    ) -> Result<ToolResult, Box<rhai::EvalAltResult>> {
        let tools = self
            .inner
            .ctx
            .get::<Tools>(TOOLS)
            .ok_or_else(|| eval_err("tools is not mounted".into()))?;
        let arguments = match dynamic_to_json(&args) {
            Ok(Value::String(s)) => s,
            Ok(v) => v.to_string(),
            Err(e) => return err(e),
        };
        let call = ToolCall {
            id: format!("rhai-{}", self.inner.plugin_id),
            name: name.to_string(),
            arguments,
        };
        block_on_tool(&self.inner.ctx, tools.as_ref(), call)
    }

    fn followup(
        &mut self,
        text: ImmutableString,
    ) -> Result<ImmutableString, Box<rhai::EvalAltResult>> {
        self.deliver_followup(None, text.to_string())
    }

    fn followup_to(
        &mut self,
        identity: ImmutableString,
        text: ImmutableString,
    ) -> Result<ImmutableString, Box<rhai::EvalAltResult>> {
        self.deliver_followup(Some(identity.to_string()), text.to_string())
    }

    /// 送进那一页的信箱（和后台任务跑完通知同一个信箱）：那一页在跑就在下一步带上，
    /// 闲着的主会话页就叫醒它开一轮。一页 5 秒内最多叫醒一次，多出来的排着等下一轮——
    /// 一颗写坏的插件不能把一个会话叫成连轴转。
    fn deliver_followup(
        &self,
        identity: Option<String>,
        text: String,
    ) -> Result<ImmutableString, Box<rhai::EvalAltResult>> {
        const WAKE_GAP: std::time::Duration = std::time::Duration::from_secs(5);
        const MAX_CHARS: usize = 16_000;
        let text = text.trim();
        if text.is_empty() {
            return err("host.followup needs non-empty text".to_string());
        }
        if text.chars().count() > MAX_CHARS {
            return err(format!("host.followup text is over {MAX_CHARS} characters"));
        }
        let subs = self
            .inner
            .ctx
            .get::<crate::tools::task::Subagents>(crate::names::SUBAGENTS)
            .ok_or_else(|| {
                eval_err("subagents is not mounted; host.followup has no mailbox".into())
            })?;
        let anchor = self.anchor_page();
        // 指定了 identity 也只能落在 `anchor` 那棵会话树里（这一页和它的子代理）：插件是
        // 进程级装的，钩子载荷里带着各页的 identity，不收住的话一张卡片 / 一个定时器就能
        // 往同一进程里别的会话塞话并叫醒它；拼错、过期的 id 也会一直留在信箱里。
        let page = match identity
            .map(|i| i.trim().to_string())
            .filter(|i| !i.is_empty())
        {
            None => anchor.ok_or_else(|| {
                eval_err(
                    "host.followup: no page to send to — call it from a tool, hook or card \
                     action, or from a timer after one of those has run on a page"
                        .into(),
                )
            })?,
            Some(target) => {
                let parent = |id: &str| subs.snapshot(id).map(|s| s.parent);
                let same_tree = anchor.as_deref().is_some_and(|a| {
                    a == target || {
                        let root = session_root(a, parent);
                        root.is_some() && root == session_root(&target, parent)
                    }
                });
                if !same_tree {
                    return err(format!(
                        "host.followup can only reach the session tree it is running for ({}): \
                         that page and its subagents. {target:?} is not in it",
                        anchor.as_deref().unwrap_or("no page")
                    ));
                }
                target
            }
        };
        let wakes = cordis_base::types::is_main_identity(&page) && {
            let mut last = self.inner.last_wake.lock().unwrap();
            let now = std::time::Instant::now();
            match last.get(&page) {
                Some(at) if now.duration_since(*at) < WAKE_GAP => false,
                _ => {
                    last.insert(page.clone(), now);
                    true
                }
            }
        };
        subs.notify_from_plugin(&page, &self.inner.plugin_id, text.to_string(), wakes);
        Ok(if wakes { "woke" } else { "queued" }.into())
    }

    /// 这段插件代码正在替哪一页跑：发起调用的那一页（工具、钩子、卡片按钮）；正在处理的
    /// 那条 `session/entry` 的页；都不是（定时器）就是最近一次跑过钩子的那一页。
    fn anchor_page(&self) -> Option<String> {
        let (id, _) = exec_identity();
        if !id.is_empty() {
            return Some(id);
        }
        if let Some(id) = ENTRY_PAGE.with(|p| p.borrow().clone()) {
            return Some(id);
        }
        self.inner
            .last_page
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|c| c.get::<crate::session::log::Sessions>(crate::names::SESSIONS))
            .map(|s| s.identity().to_string())
    }

    fn every_int(&mut self, seconds: rhai::INT, f: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        self.timer(seconds as f64, f, true)
    }

    fn every_float(
        &mut self,
        seconds: rhai::FLOAT,
        f: FnPtr,
    ) -> Result<(), Box<rhai::EvalAltResult>> {
        self.timer(seconds, f, true)
    }

    fn after_int(&mut self, seconds: rhai::INT, f: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        self.timer(seconds as f64, f, false)
    }

    fn after_float(
        &mut self,
        seconds: rhai::FLOAT,
        f: FnPtr,
    ) -> Result<(), Box<rhai::EvalAltResult>> {
        self.timer(seconds, f, false)
    }

    /// 定时器归这次运行：插件停了、reload 换版了，任务跟着取消。回调一次跑完才排下一次，
    /// 不会叠着跑。
    fn timer(
        &mut self,
        seconds: f64,
        f: FnPtr,
        repeat: bool,
    ) -> Result<(), Box<rhai::EvalAltResult>> {
        const MAX_TIMERS: usize = 16;
        let floor = if repeat { 5.0 } else { 1.0 };
        if !seconds.is_finite() || seconds < floor {
            return err(format!(
                "host.{} needs at least {floor} seconds",
                if repeat { "every" } else { "after" }
            ));
        }
        // Timers may span days (unlike blocking hooks), but not more than a month.
        // Convert before taking a timer slot or spawning: a rejected value has no side effects.
        const MAX_TIMER_SECONDS: f64 = 30.0 * 24.0 * 60.0 * 60.0;
        let invalid_period = || {
            eval_err(format!(
                "host.{} 的间隔不能超过 2592000 秒（30 天）",
                if repeat { "every" } else { "after" }
            ))
        };
        let period =
            std::time::Duration::try_from_secs_f64(seconds).map_err(|_| invalid_period())?;
        if seconds > MAX_TIMER_SECONDS {
            return Err(invalid_period());
        }
        if self
            .inner
            .timers
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            >= MAX_TIMERS
        {
            return err(format!("a plugin run can hold at most {MAX_TIMERS} timers"));
        }
        let handle = tokio::runtime::Handle::try_current().map_err(|e| eval_err(e.to_string()))?;
        let engine = self.inner.engine.clone();
        let ast = self.inner.ast.clone();
        let plugin_id = self.inner.plugin_id.clone();
        let task = handle.spawn(async move {
            let mut next = tokio::time::Instant::now() + period;
            loop {
                tokio::time::sleep_until(next).await;
                let (engine, ast, f, plugin_id) =
                    (engine.clone(), ast.clone(), f.clone(), plugin_id.clone());
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = call_fnptr_raw(&engine, &ast, &f, Dynamic::UNIT) {
                        eprintln!("[cordis:{plugin_id}] timer: {e}");
                    }
                })
                .await;
                if !repeat {
                    break;
                }
                next = tokio::time::Instant::now() + period;
            }
        });
        let abort = task.abort_handle();
        self.own(cordis::Disposable::from_fn(move || abort.abort()))
    }

    fn llm(
        &mut self,
        prompt: ImmutableString,
    ) -> Result<ImmutableString, Box<rhai::EvalAltResult>> {
        self.ask_model(None, prompt.to_string())
    }

    fn llm_map(&mut self, spec: Map) -> Result<ImmutableString, Box<rhai::EvalAltResult>> {
        let prompt =
            map_str(&spec, "prompt").ok_or_else(|| eval_err("host.llm needs prompt".into()))?;
        self.ask_model(map_str(&spec, "system"), prompt)
    }

    /// 用**这一页**的模型问一句（同侧记结论那条路：隔离出一份不落日志的会话）。
    /// 不带工具；阻塞调用方。
    fn ask_model(
        &self,
        system: Option<String>,
        prompt: String,
    ) -> Result<ImmutableString, Box<rhai::EvalAltResult>> {
        const MAX_PROMPT: usize = 256 * 1024;
        if prompt.trim().is_empty() {
            return err("host.llm needs a non-empty prompt".to_string());
        }
        if prompt.len() + system.as_deref().map_or(0, str::len) > MAX_PROMPT {
            return err("host.llm prompt is over 256KiB".to_string());
        }
        let llm = self
            .inner
            .ctx
            .get::<crate::llm::sampler::Llm>(crate::names::LLM)
            .ok_or_else(|| eval_err("llm is not mounted".into()))?;
        let page = crate::tools::registry::exec_ctx().unwrap_or_else(|| self.inner.ctx.clone());
        let iso = page.isolate(crate::names::SESSIONS);
        let request = cordis_base::types::PromptRequest {
            system: system.unwrap_or_else(|| "You are a precise assistant. Answer briefly.".into()),
            history: vec![LogEvent::User(prompt.into())],
            tools: Vec::new(),
        };
        let handle = tokio::runtime::Handle::try_current().map_err(|e| eval_err(e.to_string()))?;
        let output = tokio::task::block_in_place(|| {
            handle.block_on(llm.stream_observed(&iso, request, |_| {}))
        });
        if let Some(error) = output.error {
            return err(format!("host.llm: {error}"));
        }
        Ok(output.text.trim().to_string().into())
    }

    fn with_store<R>(&self, f: impl FnOnce(&mut serde_json::Map<String, Value>) -> R) -> R {
        let mut guard = self.inner.store.lock().unwrap();
        let map = guard.get_or_insert_with(|| {
            self.inner
                .store_path
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default()
        });
        f(map)
    }

    fn load(&mut self, key: ImmutableString) -> Dynamic {
        self.with_store(|m| {
            m.get(key.as_str())
                .map(json_to_dynamic)
                .unwrap_or(Dynamic::UNIT)
        })
    }

    fn save(
        &mut self,
        key: ImmutableString,
        value: Dynamic,
    ) -> Result<(), Box<rhai::EvalAltResult>> {
        let json = dynamic_to_json(&value).map_err(eval_err)?;
        self.write_store(|m| {
            m.insert(key.to_string(), json);
        })
    }

    fn forget(&mut self, key: ImmutableString) -> Result<(), Box<rhai::EvalAltResult>> {
        self.write_store(|m| {
            m.remove(key.as_str());
        })
    }

    /// 改完整份写回（先写临时文件再改名，写一半崩了也不留半截 JSON）。超过 1MiB 拒绝，
    /// 内存里的也不改。
    fn write_store(
        &self,
        change: impl FnOnce(&mut serde_json::Map<String, Value>),
    ) -> Result<(), Box<rhai::EvalAltResult>> {
        const MAX_STATE: usize = 1024 * 1024;
        self.with_store(|map| {
            let mut next = map.clone();
            change(&mut next);
            let text = serde_json::to_string(&next).map_err(|e| eval_err(e.to_string()))?;
            if text.len() > MAX_STATE {
                return err("plugin state is over 1MiB; store less (or write a file)".to_string());
            }
            if let Some(path) = self.inner.store_path.as_ref() {
                write_atomic(path, &text).map_err(eval_err)?;
            }
            *map = next;
            Ok(())
        })
    }

    fn turn_cards(
        &self,
    ) -> Result<Arc<crate::host::turn_cards::TurnCards>, Box<rhai::EvalAltResult>> {
        self.inner
            .ctx
            .get::<crate::host::turn_cards::TurnCards>(crate::names::TURN_CARDS)
            .ok_or_else(|| eval_err("turnCards is not mounted; this host has no card slot".into()))
    }

    fn card(&mut self, spec: Map) -> Result<ImmutableString, Box<rhai::EvalAltResult>> {
        let cards = self.turn_cards()?;
        let page = main_exec_page()
            .or_else(|| self.inner.last_page.lock().unwrap().clone())
            .ok_or_else(|| {
                eval_err(
                    "host.card: no main page yet — call it from a tool or hook (or after one ran)"
                        .into(),
                )
            })?;
        let parsed = card_spec(&spec, None).map_err(eval_err)?;
        let handler = spec
            .get("on_action")
            .cloned()
            .and_then(|d| d.try_cast::<FnPtr>())
            .map(|f| {
                let engine = self.inner.engine.clone();
                let ast = self.inner.ast.clone();
                let last_page = self.inner.last_page.clone();
                let page = page.clone();
                let h: crate::host::turn_cards::CardHandler = Arc::new(move |action: &str| {
                    *last_page.lock().unwrap() = Some(page.clone());
                    let v = crate::tools::registry::with_exec_ctx(&page, || {
                        call_fnptr_raw(&engine, &ast, &f, Dynamic::from(action.to_string()))
                    })?;
                    Ok((!v.is_unit()).then(|| dynamic_to_display(v)))
                });
                h
            });
        let id = cards
            .post(
                &page,
                &self.inner.plugin_id,
                map_str(&spec, "id").as_deref(),
                parsed,
                handler,
            )
            .map_err(eval_err)?;
        if !self
            .inner
            .cards_owned
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            let plugin_id = self.inner.plugin_id.clone();
            let cards = cards.clone();
            self.own(cordis::Disposable::from_fn(move || {
                cards.detach_plugin(&plugin_id)
            }))?;
        }
        Ok(id.into())
    }

    fn update_card(
        &mut self,
        id: ImmutableString,
        spec: Map,
    ) -> Result<(), Box<rhai::EvalAltResult>> {
        let cards = self.turn_cards()?;
        let mut error = None;
        cards
            .update(
                &self.inner.plugin_id,
                id.as_str(),
                |current| match card_spec(&spec, Some(current)) {
                    Ok(next) => *current = next,
                    Err(e) => error = Some(e),
                },
            )
            .map_err(eval_err)?;
        match error {
            Some(e) => err(e),
            None => Ok(()),
        }
    }

    fn remove_card(&mut self, id: ImmutableString) -> Result<(), Box<rhai::EvalAltResult>> {
        self.turn_cards()?
            .remove(&self.inner.plugin_id, id.as_str())
            .map_err(eval_err)
    }

    fn on(
        &mut self,
        event: ImmutableString,
        handler: FnPtr,
    ) -> Result<(), Box<rhai::EvalAltResult>> {
        let name = event.to_string();
        self.on_event(&name, handler)?;
        self.note(|s| super::registry::Surface::push(&mut s.hooks, &name));
        Ok(())
    }

    fn on_event(&mut self, name: &str, handler: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        match name {
            SESSION_EVENT => self.on_session_event(handler),
            SESSION_ENTRY => self.on_session_entry(handler),
            STEP_START => self.on_step_start(handler),
            TURN_END => self.on_turn_end(handler),
            PRE_STEP | PROMPT_ASSEMBLE | TOOLS_PRE_EXECUTE | TOOLS_EXECUTE | LLM_STREAM => {
                // 这五个钩子能**否决**一次工具调用、能改系统提示、能拒掉一整轮 ——
                // 只给从插件目录装载的插件（启动 / 开项目时自动装、设置里启用、
                // 用户批准过的 `cordis_plugin reload`）。程序侧 define 的会话包打不开。
                if !self.inner.disk_backed {
                    return err(format!(
                        "host.on({name:?}) is only available to plugins loaded from a plugins \
                         directory (.dock/plugins/<id>/ via cordis_plugin reload or autoload); a \
                         package defined programmatically cannot register it"
                    ));
                }
                match name {
                    PRE_STEP => self.on_pre_step(handler),
                    PROMPT_ASSEMBLE => self.on_prompt(handler),
                    TOOLS_PRE_EXECUTE => self.on_tool_call(handler),
                    TOOLS_EXECUTE => self.on_tool_result(handler),
                    _ => self.on_llm_output(handler),
                }
            }
            _ => err(format!(
                "host.on supports {SESSION_EVENT:?}, {SESSION_ENTRY:?}, {STEP_START:?}, {TURN_END:?}, {PRE_STEP:?}, \
                 {PROMPT_ASSEMBLE:?}, {TOOLS_PRE_EXECUTE:?}, {TOOLS_EXECUTE:?} and \
                 {LLM_STREAM:?} (got {name:?})"
            )),
        }
    }

    /// `agent/step-start`: return a `<system-reminder>` body to inject before
    /// this sample, or `()` for no opinion. The chain always runs on — a script
    /// gets to add, never to swallow — and the loop does the append.
    fn on_step_start(&mut self, handler: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        let script = self.script_hook(handler);
        let d = self
            .inner
            .ctx
            .on_waterfall(STEP_START, move |start: StepStart, args| {
                let mut next = args.next::<StepStart>().unwrap_or(start);
                let mut payload = Map::new();
                payload.insert("step".into(), (next.step as i64).into());
                payload.insert("identity".into(), next.identity.clone().into());
                payload.insert("main".into(), next.is_main_session().into());
                if let Some(body) = script(payload) {
                    next.remind(ORDER_STEP_START_DYNAMIC, body);
                }
                next
            })
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        Ok(())
    }

    /// `agent/turn-end`: return a `<system-reminder>` body to keep the turn
    /// going, or `()` to let it end. Built-in slots win ties against a script.
    fn on_turn_end(&mut self, handler: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        let script = self.script_hook(handler);
        let d = self
            .inner
            .ctx
            .on_waterfall(TURN_END, move |end: TurnEnd, args| {
                let mut next = args.next::<TurnEnd>().unwrap_or(end);
                // `TurnEnd` says handlers must respect this, and the host — not
                // the script's good intentions — is the handler here. When the
                // user has queued the next message they steer: the script is
                // not even asked.
                //
                // Child turns are *not* filtered (unlike `tool-todo` /
                // `tool-goal`, whose state belongs to the main session): the
                // loop appends to whichever session is running, so a script's
                // reminder lands in the child's own transcript. Scripts that
                // care read `main` off the payload.
                if next.queued_followups {
                    return next;
                }
                let mut payload = Map::new();
                payload.insert("text".into(), next.text.clone().into());
                payload.insert("rounds".into(), (next.rounds as i64).into());
                payload.insert("ended_with_text".into(), next.ended_with_text.into());
                payload.insert("queued_followups".into(), next.queued_followups.into());
                payload.insert("identity".into(), next.identity.clone().into());
                payload.insert("main".into(), next.is_main_session().into());
                if let Some(body) = script(payload) {
                    next.keep_working(ORDER_TURN_END_DYNAMIC, body);
                }
                next
            })
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        Ok(())
    }

    /// `agent/pre-step`: refuse the turn before it starts. Return
    /// `#{ deny: "reason" }` (or a bare string) to refuse it, `()` for no
    /// opinion. The hook only calls [`PreStep::deny`]; the loop appends the
    /// reminder on the session that is starting, so a child turn does not
    /// write it into the parent.
    fn on_pre_step(&mut self, handler: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        let script = self.script_hook_raw(handler);
        let ctx = self.inner.ctx.clone();
        let d = ctx
            .clone()
            .on_waterfall(PRE_STEP, move |step: PreStep, args| {
                let mut next = args.next::<PreStep>().unwrap_or(step);
                let mut payload = Map::new();
                payload.insert("user".into(), next.user.clone().into());
                payload.insert("identity".into(), next.identity.clone().into());
                payload.insert("main".into(), next.is_main_session().into());
                if let Some(v) = script(payload) {
                    if let Some(reason) = hook_deny(&v) {
                        next.deny(reason);
                    }
                }
                next
            })
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        Ok(())
    }

    /// `system-prompt/assemble`: return a string to add one section to the
    /// assembled system prompt. The section lands in a fixed slot
    /// ([`ORDER_HOOKS`]) under this plugin's own id, so the prompt stays
    /// byte-stable no matter which plugin mounted first. `()` for no opinion.
    fn on_prompt(&mut self, handler: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        let script = self.script_hook_raw(handler);
        let ctx = self.inner.ctx.clone();
        let plugin_id = self.inner.plugin_id.clone();
        let d = ctx
            .clone()
            .on_waterfall(PROMPT_ASSEMBLE, move |assembly: PromptAssembly, args| {
                let mut next = args.next::<PromptAssembly>().unwrap_or(assembly);
                let mut payload = Map::new();
                payload.insert("tools".into(), hook_tool_names(&ctx));
                if let Some(v) = script(payload) {
                    if let Ok(text) = v.into_string() {
                        if !text.trim().is_empty() {
                            next.section(ORDER_HOOKS, &format!("hook:{plugin_id}"), text);
                        }
                    }
                }
                next
            })
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        Ok(())
    }

    /// `tools/pre-execute`: intercept a tool call before it runs. Return
    /// `#{ deny: "reason" }` to refuse it, `#{ rewrite: #{ name, arguments } }`
    /// to redirect it, `()` for no opinion. Deny is monotonic (the first refusal
    /// wins) and a rewrite after a refusal is ignored — the same rules the
    /// Rust handlers live under. A redirected call still passes the allowlist,
    /// the plan gate and the permission gate under its **new** name.
    fn on_tool_call(&mut self, handler: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        let script = self.script_hook_raw(handler);
        let plugin_id = self.inner.plugin_id.clone();
        let d = self
            .inner
            .ctx
            .on_waterfall(TOOLS_PRE_EXECUTE, move |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                let mut payload = Map::new();
                payload.insert("called".into(), next.called.clone().into());
                payload.insert("name".into(), next.name().into());
                payload.insert("arguments".into(), next.arguments().into());
                payload.insert("args".into(), json_str_to_dynamic(next.arguments()));
                payload.insert("identity".into(), next.identity.clone().into());
                payload.insert("main".into(), next.is_main_session().into());
                if let Some(v) = script(payload) {
                    if let Some(reason) = hook_deny(&v) {
                        next.deny(reason);
                    } else if let Some((name, arguments)) = hook_rewrite(&v) {
                        next.rewrite(name, arguments);
                    } else if let Some(reason) = hook_field(&v, "ask") {
                        next.ask(format!("{reason}（插件 {plugin_id}）"));
                    } else {
                        eprintln!(
                            "[cordis:{plugin_id}] host.on(tools/pre-execute): handler must return () | \
                             #{{ deny }} | #{{ ask }} | #{{ rewrite }}"
                        );
                    }
                }
                next
            })
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        Ok(())
    }

    /// `tools/execute`: post-process a finished tool result. Return a string to
    /// replace the content the model sees (trim, redact, summarize); `()` for
    /// no opinion. The result was already executed — this cannot stop it.
    fn on_tool_result(&mut self, handler: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        let script = self.script_hook_raw(handler);
        let plugin_id = self.inner.plugin_id.clone();
        let d = self
            .inner
            .ctx
            .on_waterfall(TOOLS_EXECUTE, move |result: ToolResult, args| {
                let mut next = args.next::<ToolResult>().unwrap_or(result);
                let mut payload = Map::new();
                payload.insert("name".into(), next.name.clone().into());
                payload.insert("call_id".into(), next.call_id.clone().into());
                payload.insert("content".into(), next.content.clone().into());
                payload.insert("is_error".into(), next.is_error.into());
                let arguments = crate::tools::registry::call_arguments().unwrap_or_default();
                payload.insert("args".into(), json_str_to_dynamic(&arguments));
                payload.insert("arguments".into(), arguments.into());
                let (identity, main) = exec_identity();
                payload.insert("identity".into(), identity.into());
                payload.insert("main".into(), main.into());
                if let Some(v) = script(payload) {
                    if let Ok(text) = v.into_string() {
                        next.content = text;
                    } else {
                        eprintln!(
                            "[cordis:{plugin_id}] host.on(tools/execute): handler must return a \
                             string or ()"
                        );
                    }
                }
                next
            })
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        Ok(())
    }

    /// `llm/stream`: post-process the sampled output. Return a string to
    /// replace the assistant text; `()` for no opinion. This is the output
    /// side only — the request (system prompt, history, tool list) is not
    /// reachable from here.
    fn on_llm_output(&mut self, handler: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        let script = self.script_hook_raw(handler);
        let plugin_id = self.inner.plugin_id.clone();
        let d = self
            .inner
            .ctx
            .on_waterfall(LLM_STREAM, move |output: LlmOutput, args| {
                let mut next = args.next::<LlmOutput>().unwrap_or(output);
                let mut payload = Map::new();
                payload.insert("text".into(), next.text.clone().into());
                payload.insert(
                    "tool_calls".into(),
                    Dynamic::from_array(
                        next.tool_calls
                            .iter()
                            .map(|c| Dynamic::from(c.name.clone()))
                            .collect(),
                    ),
                );
                if let Some(v) = script(payload) {
                    if let Ok(text) = v.into_string() {
                        next.text = text;
                    } else {
                        eprintln!(
                            "[cordis:{plugin_id}] host.on(llm/stream): handler must return a \
                             string or ()"
                        );
                    }
                }
                next
            })
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        Ok(())
    }

    /// Wrap a script callback as a synchronous `Map -> Option<String>`.
    ///
    /// Waterfalls run inline, so this blocks the turn while the script runs —
    /// bounded by the engine's `max_operations`. A throw (or a blown budget) is
    /// "no opinion": it is logged and the chain carries on (fail-open).
    fn script_hook(
        &self,
        handler: FnPtr,
    ) -> impl Fn(Map) -> Option<String> + Send + Sync + 'static {
        let engine = self.inner.engine.clone();
        let ast = self.inner.ast.clone();
        let plugin_id = self.inner.plugin_id.clone();
        let last_page = self.inner.last_page.clone();
        move |payload: Map| {
            remember_page(&last_page);
            let called = call_fnptr_raw(&engine, &ast, &handler, Dynamic::from_map(payload));
            match called {
                Ok(v) if v.is_unit() => None,
                // A string or nothing. Anything else would otherwise be stringified
                // into the model's history by accident.
                Ok(v) => match v.into_string() {
                    Ok(text) if !text.trim().is_empty() => Some(text),
                    Ok(_) => None,
                    Err(kind) => {
                        eprintln!(
                        "[cordis:{plugin_id}] host.on: handler must return a string or (), got {kind}"
                    );
                        None
                    }
                },
                Err(e) => {
                    eprintln!("[cordis:{plugin_id}] host.on: {e}");
                    None
                }
            }
        }
    }

    /// Wrap a script callback as a synchronous `Map -> Option<Dynamic>` for the
    /// hooks that answer with more than a string (`#{ deny }`,
    /// `#{ rewrite }`). `()` means "no opinion"; a throw is logged and treated
    /// as no opinion (fail-open), like [`Host::script_hook`].
    fn script_hook_raw(
        &self,
        handler: FnPtr,
    ) -> impl Fn(Map) -> Option<Dynamic> + Send + Sync + 'static {
        let engine = self.inner.engine.clone();
        let ast = self.inner.ast.clone();
        let plugin_id = self.inner.plugin_id.clone();
        let last_page = self.inner.last_page.clone();
        move |payload: Map| {
            remember_page(&last_page);
            match call_fnptr_raw(&engine, &ast, &handler, Dynamic::from_map(payload)) {
                Ok(v) if v.is_unit() => None,
                Ok(v) => Some(v),
                Err(e) => {
                    eprintln!("[cordis:{plugin_id}] host.on: {e}");
                    None
                }
            }
        }
    }

    fn on_session_event(&mut self, handler: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        let engine = self.inner.engine.clone();
        let ast = self.inner.ast.clone();
        let plugin_id = self.inner.plugin_id.clone();
        let run = move |line: String| {
            if let Err(e) = call_fnptr(&engine, &ast, &handler, Dynamic::from(line)) {
                eprintln!("[cordis:{plugin_id}] host.on: {e}");
            }
        };
        // 一个登记一条队列、一个干活的：回调按发出的顺序一条跑完再跑下一条（脚本里的共享状态
        // 不会被并发的回调改乱）。卸下时监听连同发送端一起丢掉，干活的读到队尾就退出。
        let listener = match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                let run = Arc::new(run);
                handle.spawn(async move {
                    while let Some(line) = rx.recv().await {
                        let run = run.clone();
                        let _ = tokio::task::spawn_blocking(move || run(line)).await;
                    }
                });
                self.inner.ctx.on(SESSION_EVENT, move |ev: &LogEvent| {
                    if let Some(line) = event_line(ev) {
                        let _ = tx.send(line);
                    }
                })
            }
            // 没有 tokio 运行时（只在同步测试里）：就地跑。
            Err(_) => self.inner.ctx.on(SESSION_EVENT, move |ev: &LogEvent| {
                if let Some(line) = event_line(ev) {
                    run(line);
                }
            }),
        }
        .map_err(|e| eval_err(e.to_string()))?;
        self.own(listener)?;
        Ok(())
    }

    /// `session/entry`：结构化的会话事件（[`entry_json`]）。和 `session/event` 一样
    /// 异步、按发出顺序一条跑完再跑下一条，只观察。助手那一条来自采样收尾
    /// （[`crate::LLM_FINISHED`]），带最终文本、工具调用和用量；流式 delta 不发。
    fn on_session_entry(&mut self, handler: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        use crate::session::log::{ChildLogEvent, LlmFinished, PageLogEvent};
        let engine = self.inner.engine.clone();
        let ast = self.inner.ast.clone();
        let plugin_id = self.inner.plugin_id.clone();
        let run = Arc::new(move |entry: Value| {
            // 处理器不在任何一轮里跑：记下这一条是哪一页的，`host.followup` 才知道在替谁跑。
            let page = entry
                .get("identity")
                .and_then(Value::as_str)
                .map(str::to_string);
            let _entry_page = EntryPage::enter(page);
            if let Err(e) = call_fnptr(&engine, &ast, &handler, json_to_dynamic(&entry)) {
                eprintln!("[cordis:{plugin_id}] host.on(session/entry): {e}");
            }
        });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let run = run.clone();
                handle.spawn(async move {
                    while let Some(entry) = rx.recv().await {
                        let run = run.clone();
                        let _ = tokio::task::spawn_blocking(move || run(entry)).await;
                    }
                });
            }
            Err(_) => return err("session/entry needs a tokio runtime".to_string()),
        }
        let page_tx = tx.clone();
        let page = self
            .inner
            .ctx
            .on(crate::names::SESSION_PAGE_EVENT, move |e: &PageLogEvent| {
                if let Some(entry) = entry_json(&e.event, &e.page, true, e.preamble) {
                    let _ = page_tx.send(entry);
                }
            })
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(page)?;
        let child_tx = tx.clone();
        let child = self
            .inner
            .ctx
            .on(
                crate::names::SESSION_CHILD_EVENT,
                move |e: &ChildLogEvent| {
                    if let Some(entry) = entry_json(&e.event, &e.child, false, false) {
                        let _ = child_tx.send(entry);
                    }
                },
            )
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(child)?;
        let finished = self
            .inner
            .ctx
            .on(crate::names::LLM_FINISHED, move |f: &LlmFinished| {
                let _ = tx.send(assistant_entry_json(f));
            })
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(finished)?;
        Ok(())
    }

    fn log(&mut self, message: ImmutableString) {
        eprintln!("[cordis:{}] {message}", self.inner.plugin_id);
    }

    fn secret(
        &mut self,
        name: ImmutableString,
    ) -> Result<ImmutableString, Box<rhai::EvalAltResult>> {
        let value =
            super::rhai_secret::resolve(&self.inner.ctx, &self.inner.plugin_id, name.as_str())
                .map_err(eval_err)?;
        Ok(value.into())
    }

    fn read_bytes(
        &mut self,
        path: ImmutableString,
    ) -> Result<rhai::Blob, Box<rhai::EvalAltResult>> {
        let bytes = super::path_bytes::read_bytes_gated(
            &self.inner.ctx,
            &self.inner.plugin_id,
            path.as_str(),
        )
        .map_err(eval_err)?;
        Ok(bytes)
    }

    /// 记一笔「这次运行登记了什么」。
    fn note(&self, f: impl FnOnce(&mut super::registry::Surface)) {
        f(&mut self.inner.surface.lock().unwrap());
    }

    fn own(&self, d: cordis::Disposable) -> Result<(), Box<rhai::EvalAltResult>> {
        let fallback = d.clone();
        match self.inner.ctx.effect("host.register", move |scope| {
            scope.own(d);
            Ok(())
        }) {
            Ok(_) => Ok(()),
            Err(e) => {
                self.inner.disposers.lock().unwrap().push(fallback);
                Err(eval_err(e.to_string()))
            }
        }
    }
}

struct RhaiSlot {
    title: String,
    hud: bool,
    actions: Vec<SlotAction>,
    /// `view()` → 视图树（map）。没有就只画 `render()` 的文本。
    view: Option<FnPtr>,
    /// web 界面的 HTML 文件（已确认在插件目录里）。
    web: Option<PathBuf>,
    engine: Arc<Engine>,
    ast: AST,
    render: FnPtr,
    on_key: Option<FnPtr>,
}

/// web 面板 HTML 的上限。
const MAX_WEB_HTML: u64 = 2 * 1024 * 1024;

/// `register_slot` 的 `actions: [#{ id, label }]`。`label` 省略就用 `id`。
fn slot_actions(spec: &Map) -> Result<Vec<SlotAction>, Box<rhai::EvalAltResult>> {
    let Some(raw) = spec.get("actions") else {
        return Ok(Vec::new());
    };
    let list = raw
        .clone()
        .try_cast::<rhai::Array>()
        .ok_or_else(|| eval_err("register_slot actions must be an array".into()))?;
    list.into_iter()
        .map(|item| {
            let map = item
                .try_cast::<Map>()
                .ok_or_else(|| eval_err("each slot action must be #{ id, label }".into()))?;
            let id = map_str(&map, "id").ok_or_else(|| eval_err("slot action needs id".into()))?;
            let label = map_str(&map, "label").unwrap_or_else(|| id.clone());
            Ok(SlotAction { id, label })
        })
        .collect()
}

impl SlotHandler for RhaiSlot {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn actions(&self) -> Vec<SlotAction> {
        self.actions.clone()
    }

    fn view(&self) -> Option<ViewNode> {
        let view = self.view.as_ref()?;
        let error = |e: String| Some(view_error(&e));
        match call_fnptr_raw(&self.engine, &self.ast, view, Dynamic::UNIT) {
            // 返回 `()` = 这次不给视图，回退到 `render()`：插件可以按条件给。
            Ok(tree) if tree.is_unit() => None,
            Ok(tree) => match dynamic_to_json(&tree) {
                Ok(json) => Some(ViewNode::parse(&json)),
                Err(e) => error(e),
            },
            Err(e) => error(e),
        }
    }

    fn hud(&self) -> bool {
        self.hud
    }

    fn has_web(&self) -> bool {
        self.web.is_some()
    }

    fn web(&self) -> Result<String, String> {
        let path = self.web.as_ref().ok_or("这个面板没有 web 界面")?;
        let meta = std::fs::metadata(path).map_err(|e| format!("读 web 文件：{e}"))?;
        if meta.len() > MAX_WEB_HTML {
            return Err(format!("web 文件超过 {} MiB", MAX_WEB_HTML / 1024 / 1024));
        }
        std::fs::read_to_string(path).map_err(|e| format!("读 web 文件：{e}"))
    }

    fn render(&self) -> String {
        match call_fnptr(&self.engine, &self.ast, &self.render, Dynamic::UNIT) {
            Ok(text) => text,
            Err(e) => format!("（正文出错：{e}）"),
        }
    }

    fn on_key(&self, key: &str) -> SlotKeyResult {
        let Some(on_key) = &self.on_key else {
            if key == "esc" {
                return SlotKeyResult::Close;
            }
            return SlotKeyResult::Keep;
        };
        match call_fnptr(
            &self.engine,
            &self.ast,
            on_key,
            Dynamic::from(key.to_string()),
        ) {
            Ok(text) if text.trim() == "close" => SlotKeyResult::Close,
            _ if key == "esc" => SlotKeyResult::Close,
            _ => SlotKeyResult::Keep,
        }
    }
}

/// `session/entry` 的一条（采样那条走 [`assistant_entry_json`]）。流式 delta、
/// pre-step、组装好的提示不发。
fn entry_json(ev: &LogEvent, identity: &str, main: bool, replay: bool) -> Option<Value> {
    let mut entry = match ev {
        LogEvent::User(msg) => serde_json::json!({ "kind": "user", "text": msg.to_string() }),
        LogEvent::ToolExecute {
            id,
            name,
            arguments,
            content,
            is_error,
            ..
        } => serde_json::json!({
            "kind": "tool",
            "name": name,
            "call_id": id,
            "arguments": arguments,
            "args": serde_json::from_str::<Value>(arguments).unwrap_or(Value::Null),
            "content": content,
            "is_error": is_error,
        }),
        LogEvent::SystemReminder(text) => serde_json::json!({ "kind": "reminder", "text": text }),
        LogEvent::Notice { kind, title, body } => serde_json::json!({
            "kind": "notice", "level": kind.as_str(), "title": title, "body": body,
        }),
        LogEvent::TurnEnd(status) => serde_json::json!({
            "kind": "turn-end",
            "status": status.as_str(),
            "error": match status {
                cordis_base::types::TurnEndStatus::Failed(e) => Value::String(e.clone()),
                _ => Value::Null,
            },
        }),
        LogEvent::LlmStream(_) | LogEvent::PreStep | LogEvent::Prompt(_) => return None,
    };
    let obj = entry.as_object_mut()?;
    obj.insert("identity".into(), identity.into());
    obj.insert("main".into(), main.into());
    obj.insert("replay".into(), replay.into());
    Some(entry)
}

/// 一次采样收尾：最终文本、工具调用、用量、缓存命中率（cached / prompt）、费用。
fn assistant_entry_json(f: &crate::session::log::LlmFinished) -> Value {
    let out = &f.output;
    let calls: Vec<Value> = out
        .tool_calls
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "arguments": c.arguments,
                "args": serde_json::from_str::<Value>(&c.arguments).unwrap_or(Value::Null),
            })
        })
        .collect();
    let usage = f.usage.as_ref().map(|u| {
        serde_json::json!({
            "prompt_tokens": u.prompt_tokens,
            "completion_tokens": u.completion_tokens,
            "cached_prompt_tokens": u.cached_prompt_tokens,
            "cache_creation_prompt_tokens": u.cache_creation_prompt_tokens,
            "reasoning_tokens": u.reasoning_tokens,
        })
    });
    let cache_hit = f
        .usage
        .as_ref()
        .filter(|u| u.prompt_tokens > 0)
        .map(|u| u.cached_prompt_tokens as f64 / u.prompt_tokens as f64);
    serde_json::json!({
        "kind": "assistant",
        "identity": f.identity.as_ref(),
        "main": f.main,
        "replay": false,
        "text": out.text,
        "reasoning_chars": out.reasoning.chars().count(),
        "tool_calls": calls,
        "error": out.error,
        "model": f.model,
        "usage": usage,
        "cache_hit": cache_hit,
        "elapsed_ms": f.elapsed_ms,
        "cost_usd": f.cost_usd_ticks.map(|t| t as f64 / 1e10),
    })
}

fn event_line(ev: &LogEvent) -> Option<String> {
    let line = match ev {
        LogEvent::User(text) => format!("user\t{}", preview(text, 160)),
        LogEvent::LlmStream(out) => {
            if out.text.trim().is_empty() && !out.tool_calls.is_empty() {
                let names = out
                    .tool_calls
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                format!("assistant\ttools:{names}")
            } else {
                format!("assistant\t{}", preview(&out.text, 160))
            }
        }
        LogEvent::ToolExecute { name, .. } => format!("tool\t{name}"),
        LogEvent::SystemReminder(text) => format!("reminder\t{}", preview(text, 80)),
        LogEvent::PreStep | LogEvent::Prompt(_) => return None,
        LogEvent::Notice { kind, title, .. } => format!("notice[{}]\t{title}", kind.as_str()),
        LogEvent::TurnEnd(status) => format!("turn-end\t{}", status.as_str()),
    };
    Some(line)
}

/// A hook's refusal reason: a bare string, or `#{ deny: "reason" }`.
fn hook_deny(value: &Dynamic) -> Option<String> {
    if let Ok(text) = value.clone().into_string() {
        let trimmed = text.trim().to_string();
        return (!trimmed.is_empty()).then_some(trimmed);
    }
    let map = value.clone().try_cast::<Map>()?;
    let reason = map.get("deny")?.clone().into_string().ok()?;
    let trimmed = reason.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

thread_local! {
    /// 正在处理的那条 `session/entry` 属于哪一页（或哪个子代理），见 [`EntryPage`]。
    static ENTRY_PAGE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// 在这个线程上跑一条 `session/entry` 的处理器期间，标着它是哪一页的；出作用域（含
/// panic 展开）就撤掉，下一条不会误认。
struct EntryPage(Option<String>);

impl EntryPage {
    fn enter(page: Option<String>) -> Self {
        Self(ENTRY_PAGE.with(|p| p.replace(page)))
    }
}

impl Drop for EntryPage {
    fn drop(&mut self) {
        let prev = self.0.take();
        ENTRY_PAGE.with(|p| *p.borrow_mut() = prev);
    }
}

/// 一个页身份所在会话树的根（主会话页）：子代理顺着 `parent` 往上找，认不得的是 `None`。
fn session_root(identity: &str, parent: impl Fn(&str) -> Option<String>) -> Option<String> {
    let mut id = identity.to_string();
    // 子代理最多套几层是有限的；防一个坏的 parent 链转圈。
    for _ in 0..32 {
        if cordis_base::types::is_main_identity(&id) {
            return Some(id);
        }
        id = parent(&id)?;
    }
    None
}

/// 正在跑的主会话页记下来，给定时器之类没有当前页的 `host.followup` / `host.card` 用。
fn remember_page(last_page: &Mutex<Option<Context>>) {
    if let Some(page) = main_exec_page() {
        *last_page.lock().unwrap() = Some(page);
    }
}

/// 当前执行的那一页，且是主会话页（子代理的回合不算）。
fn main_exec_page() -> Option<Context> {
    crate::tools::registry::exec_ctx().filter(|c| {
        c.get::<crate::session::log::Sessions>(crate::names::SESSIONS)
            .is_some_and(|s| s.is_main())
    })
}

/// 插件持久存储的文件：`$DOCK_HOME/plugin-state/<id>-<目录指纹>.json`。按插件目录区分，
/// 不同项目里的同名插件各存各的。没有目录（程序侧 define）就不落盘。
fn state_path(plugin_id: &str, plugin_dir: Option<&Path>) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let dir = plugin_dir?;
    let canon = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let digest = Sha256::digest(canon.to_string_lossy().as_bytes());
    let short: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    Some(
        cordis_base::config::dock_home()
            .join("plugin-state")
            .join(format!("{plugin_id}-{short}.json")),
    )
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("bad state path")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("plugin state: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("plugin state: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("plugin state: {e}"))
}

/// 脚本给的卡片字段 → [`CardSpec`]。`base` 是更新时的旧值：没给的字段留着。
fn card_spec(
    spec: &Map,
    base: Option<&crate::host::turn_cards::CardSpec>,
) -> Result<crate::host::turn_cards::CardSpec, String> {
    let mut out = base.cloned().unwrap_or_default();
    if let Some(title) = map_str(spec, "title") {
        out.title = title;
    }
    if out.title.trim().is_empty() {
        return Err("host.card needs a title".into());
    }
    if let Some(text) = map_str(spec, "text") {
        out.text = text;
    }
    if let Some(tone) = map_str(spec, "tone") {
        out.tone = tone;
    }
    if let Some(view) = spec.get("view").filter(|v| !v.is_unit()) {
        out.view = Some(ViewNode::parse(&dynamic_to_json(view)?));
    }
    if let Some(actions) = spec.get("actions") {
        let arr = actions
            .clone()
            .try_cast::<Array>()
            .ok_or("card actions must be an array of #{ id, label }")?;
        out.actions = arr
            .into_iter()
            .filter_map(|a| {
                let m = a.try_cast::<Map>()?;
                let id = map_str(&m, "id")?;
                let label = map_str(&m, "label").unwrap_or_else(|| id.clone());
                Some((id, label))
            })
            .collect();
    }
    Ok(out)
}

/// `#{ <key>: "text" }` 里的非空文本。
fn hook_field(value: &Dynamic, key: &str) -> Option<String> {
    let map = value.clone().try_cast::<Map>()?;
    let text = map.get(key)?.clone().into_string().ok()?;
    let trimmed = text.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// 正在跑的这一轮是哪一页：`(identity, main)`。不在任何一轮里是 `("", false)`。
pub(crate) fn exec_identity() -> (String, bool) {
    crate::tools::registry::exec_ctx()
        .and_then(|c| c.get::<crate::session::log::Sessions>(crate::names::SESSIONS))
        .map(|s| (s.identity().to_string(), s.is_main()))
        .unwrap_or_default()
}

/// A hook's redirect: `#{ rewrite: #{ name, arguments } }`.
fn hook_rewrite(value: &Dynamic) -> Option<(String, String)> {
    let map = value.clone().try_cast::<Map>()?;
    let rewrite = map.get("rewrite")?.clone().try_cast::<Map>()?;
    let name = rewrite.get("name")?.clone().into_string().ok()?;
    if name.trim().is_empty() {
        return None;
    }
    let arguments = match rewrite.get("arguments") {
        Some(raw) => match raw.clone().into_string() {
            Ok(text) => text,
            Err(_) => dynamic_to_json(raw).ok()?.to_string(),
        },
        None => "{}".to_string(),
    };
    Some((name, arguments))
}

/// The registered tool names, for a prompt hook that renders its own SDK
/// section. Sorted and deduped so the section stays byte-stable.
fn hook_tool_names(ctx: &Context) -> Dynamic {
    let mut names: Vec<String> = ctx
        .get::<Tools>(TOOLS)
        .map(|tools| tools.specs().into_iter().map(|spec| spec.name).collect())
        .unwrap_or_default();
    names.sort();
    names.dedup();
    Dynamic::from_array(names.into_iter().map(Dynamic::from).collect())
}

fn call_fnptr(engine: &Engine, ast: &AST, fnptr: &FnPtr, arg: Dynamic) -> Result<String, String> {
    Ok(dynamic_to_display(call_fnptr_raw(engine, ast, fnptr, arg)?))
}

/// [`call_fnptr`] without the display coercion — for callers that must tell a
/// string apart from "returned something else".
fn call_fnptr_raw(
    engine: &Engine,
    ast: &AST,
    fnptr: &FnPtr,
    arg: Dynamic,
) -> Result<Dynamic, String> {
    if arg.is_unit() {
        fnptr.call(engine, ast, ()).map_err(|e| e.to_string())
    } else {
        fnptr.call(engine, ast, (arg,)).map_err(|e| e.to_string())
    }
}

/// 脚本里的 `host.call_tool`。在**发起这次调用的那一页**上执行：权限确认、计划门、
/// 会话身份都按那一页算。插件 fiber 挂在根上，拿根算会把确认框弹到第 1 页——GUI
/// 里那一页多半没人看，调用就一直挂着。不在任何一轮里（面板按键、斜杠）才退回根。
fn block_on_tool(
    plugin_ctx: &Context,
    tools: &Tools,
    call: ToolCall,
) -> Result<ToolResult, Box<rhai::EvalAltResult>> {
    let handle = tokio::runtime::Handle::try_current().map_err(|e| eval_err(e.to_string()))?;
    let tools = tools.clone();
    let page = crate::tools::registry::exec_ctx().unwrap_or_else(|| plugin_ctx.clone());
    Ok(tokio::task::block_in_place(|| {
        handle.block_on(tools.execute_on(&page, call))
    }))
}

/// `host.call_tool_result` 交回脚本的 map。`name` 是实际跑的那颗（`tools/pre-execute`
/// 改道后就是新名字）；`images` 只给张数，像素脚本用不上。
fn tool_result_map(result: &ToolResult) -> Map {
    let mut map = Map::new();
    map.insert("name".into(), result.name.clone().into());
    map.insert("text".into(), result.content.clone().into());
    map.insert("json".into(), json_str_to_dynamic(result.content.trim()));
    map.insert("is_error".into(), result.is_error.into());
    map.insert("images".into(), (result.images.len() as rhai::INT).into());
    map
}

pub(crate) fn tool_parameters_json(spec: &Map) -> Result<String, String> {
    let Some(raw) = spec.get("parameters").cloned() else {
        return Ok(r#"{"type":"object","properties":{}}"#.into());
    };
    if raw.is_string() {
        return Err(PARAMETERS_MUST_BE_MAP.into());
    }
    let json = dynamic_to_json(&raw)?;
    let json = normalize_tool_parameters(json)?;
    Ok(json.to_string())
}

fn normalize_tool_parameters(value: Value) -> Result<Value, String> {
    let Value::Object(mut obj) = value else {
        return Err(
            "host.register_tool parameters must be a map #{ type: \"object\", properties: #{ ... }, required: [...] }"
                .into(),
        );
    };
    match obj.get("type") {
        None => {
            obj.insert("type".into(), Value::String("object".into()));
        }
        Some(Value::String(t)) if t == "object" => {}
        Some(other) => {
            return Err(format!(
                "host.register_tool parameters.type must be \"object\" (got {other})"
            ));
        }
    }
    if !obj.contains_key("properties") {
        obj.insert("properties".into(), Value::Object(serde_json::Map::new()));
    }
    let property_keys: Vec<String> = match obj.get("properties") {
        Some(Value::Object(properties)) => properties.keys().cloned().collect(),
        _ => {
            return Err(
                "host.register_tool parameters.properties must be a map of field schemas".into(),
            );
        }
    };
    if let Some(required) = obj.get("required") {
        let Some(names) = required.as_array() else {
            return Err(
                "host.register_tool parameters.required must be an array of property names".into(),
            );
        };
        for name in names {
            let Some(key) = name.as_str() else {
                return Err(
                    "host.register_tool parameters.required must be an array of strings".into(),
                );
            };
            if !property_keys.iter().any(|k| k == key) {
                return Err(format!(
                    "host.register_tool parameters.required names undeclared property {key:?}"
                ));
            }
        }
    }
    Ok(Value::Object(obj))
}

/// 插件的视图函数出错时画的那一行（中文、红字），不吞掉原因。
fn view_error(e: &str) -> ViewNode {
    ViewNode::parse(&serde_json::json!({
        "type": "text", "tone": "danger", "text": format!("（视图出错：{e}）")
    }))
}

fn map_str(map: &Map, key: &str) -> Option<String> {
    map.get(key)
        .cloned()
        .and_then(|d| d.into_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn dynamic_to_json(value: &Dynamic) -> Result<Value, String> {
    rhai::serde::from_dynamic(value).map_err(|e| e.to_string())
}

fn json_to_dynamic(value: &Value) -> Dynamic {
    rhai::serde::to_dynamic(value).unwrap_or(Dynamic::UNIT)
}

fn json_str_to_dynamic(raw: &str) -> Dynamic {
    serde_json::from_str::<Value>(raw)
        .ok()
        .map(|v| json_to_dynamic(&v))
        .unwrap_or(Dynamic::UNIT)
}

fn dynamic_to_display(value: Dynamic) -> String {
    if value.is_unit() {
        return String::new();
    }
    if let Ok(s) = value.clone().into_string() {
        return s;
    }
    match dynamic_to_json(&value) {
        Ok(Value::String(s)) => s,
        Ok(v) => v.to_string(),
        Err(_) => value.to_string(),
    }
}

fn eval_err(msg: String) -> Box<rhai::EvalAltResult> {
    Box::new(rhai::EvalAltResult::ErrorRuntime(
        msg.into(),
        rhai::Position::NONE,
    ))
}

fn err<T>(msg: impl Into<String>) -> Result<T, Box<rhai::EvalAltResult>> {
    Err(eval_err(msg.into()))
}

pub fn builtins_lines() -> Vec<String> {
    HOST_BUILTINS
        .iter()
        .chain(super::rhai_codec::BUILTINS.iter())
        .chain(super::rhai_regex::BUILTINS.iter())
        .chain(super::rhai_time::BUILTINS.iter())
        .chain(super::rhai_packages::BUILTINS.iter())
        .flat_map(|(name, purpose, sigs)| {
            let mut lines = vec![format!("- {name} — {purpose}")];
            for sig in *sigs {
                lines.push(format!("    {sig}"));
            }
            lines
        })
        .collect()
}

/// Used by inspect waiting_for for names a Rhai package declared.
pub fn rhai_name_live(ctx: &Context, name: &str) -> bool {
    match name {
        TOOLS => ctx.get::<Tools>(TOOLS).is_some(),
        SLASH => ctx.get::<Slash>(SLASH).is_some(),
        TUI_SLOTS => ctx.get::<TuiSlots>(TUI_SLOTS).is_some(),
        _ => ctx.get::<RhaiBag>(name).is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhai::Engine;

    fn map_of(src: &str) -> Map {
        let engine = Engine::new();
        engine.eval::<Map>(src).unwrap()
    }

    fn timer_host() -> Host {
        Host {
            inner: Arc::new(HostInner {
                ctx: Context::new(),
                plugin_id: "timer-test".into(),
                plugin_dir: None,
                disk_backed: false,
                surface: Default::default(),
                last_page: Arc::default(),
                cards_owned: Default::default(),
                last_wake: Default::default(),
                timers: Default::default(),
                store_path: None,
                store: Default::default(),
                engine: Arc::new(Engine::new()),
                ast: AST::empty(),
                disposers: Default::default(),
                statuses: Default::default(),
            }),
        }
    }

    /// 调真正的宿主入口；旧版在超大 float 上 panic，integer 大值也会排一个无意义的任务。
    #[tokio::test]
    async fn timers_reject_huge_intervals_without_panicking_or_taking_slots() {
        let mut host = timer_host();
        let callback = FnPtr::new("unused").unwrap();
        for repeat in [false, true] {
            let name = if repeat { "every" } else { "after" };
            let expected = format!("host.{name} 的间隔不能超过 2592000 秒（30 天）");
            for seconds in [1e20, 1e308, 2592000.001] {
                let error = if repeat {
                    host.every_float(seconds, callback.clone())
                } else {
                    host.after_float(seconds, callback.clone())
                }
                .unwrap_err();
                match *error {
                    rhai::EvalAltResult::ErrorRuntime(value, _) => {
                        assert_eq!(value.into_string().unwrap(), expected);
                    }
                    other => panic!("expected a script error, got {other:?}"),
                }
            }
            let error = if repeat {
                host.every_int(rhai::INT::MAX, callback.clone())
            } else {
                host.after_int(rhai::INT::MAX, callback.clone())
            }
            .unwrap_err();
            match *error {
                rhai::EvalAltResult::ErrorRuntime(value, _) => {
                    assert_eq!(value.into_string().unwrap(), expected);
                }
                other => panic!("expected a script error, got {other:?}"),
            }
        }
        assert_eq!(
            host.inner.timers.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(host.inner.disposers.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn timers_accept_minimum_fractional_and_maximum_intervals() {
        let mut host = timer_host();
        let callback = FnPtr::new("unused").unwrap();
        for seconds in [5.0, 5.25, 2592000.0] {
            host.every_float(seconds, callback.clone()).unwrap();
        }
        for seconds in [1.0, 1.25, 2592000.0] {
            host.after_float(seconds, callback.clone()).unwrap();
        }
        assert_eq!(
            host.inner.timers.load(std::sync::atomic::Ordering::Relaxed),
            6
        );
        assert_eq!(host.inner.disposers.lock().unwrap().len(), 0);
        host.inner.ctx.fiber().dispose().await.unwrap();
    }

    /// `host.followup(identity, …)` 认的会话树：子代理（含套娃的）归到派它的主会话页；
    /// 认不得的、parent 链转圈的都不算在任何一棵树里。
    #[test]
    fn session_root_follows_subagent_parents_to_the_page() {
        let parents: std::collections::HashMap<&str, &str> = [
            ("a1", "main#2"),
            ("a2", "a1"),
            ("loop1", "loop2"),
            ("loop2", "loop1"),
        ]
        .into();
        let parent = |id: &str| parents.get(id).map(|p| p.to_string());
        assert_eq!(session_root("main#2", parent).as_deref(), Some("main#2"));
        assert_eq!(session_root("main", parent).as_deref(), Some("main"));
        assert_eq!(session_root("a1", parent).as_deref(), Some("main#2"));
        assert_eq!(session_root("a2", parent).as_deref(), Some("main#2"));
        assert_eq!(session_root("ghost", parent), None);
        assert_eq!(session_root("loop1", parent), None);
    }

    #[test]
    fn omitted_parameters_are_empty_object_schema() {
        let spec = map_of("#{ name: \"x\" }");
        assert_eq!(
            tool_parameters_json(&spec).unwrap(),
            r#"{"type":"object","properties":{}}"#
        );
    }

    #[test]
    fn json_string_parameters_fail_loud() {
        let spec = map_of(r#"#{ parameters: "{\"type\":\"object\"}" }"#);
        let err = tool_parameters_json(&spec).unwrap_err();
        assert!(err.contains("must be a map"), "{err}");
        assert!(err.contains("not a JSON string"), "{err}");
    }

    #[test]
    fn map_parameters_round_trip() {
        let spec = map_of(
            "#{ parameters: #{ type: \"object\", properties: #{ text: #{ type: \"string\" } }, required: [\"text\"] } }",
        );
        let json = tool_parameters_json(&spec).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "object");
        assert_eq!(v["required"][0], "text");
        assert_eq!(v["properties"]["text"]["type"], "string");
    }

    #[test]
    fn required_unknown_field_fails() {
        let spec =
            map_of("#{ parameters: #{ type: \"object\", properties: #{}, required: [\"nope\"] } }");
        let err = tool_parameters_json(&spec).unwrap_err();
        assert!(err.contains("undeclared property"), "{err}");
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn build_rhai_limited_honors_inline_ceiling() {
        let oversized = "x".repeat(MAX_INLINE_SOURCE + 1);
        match build_rhai_limited("t", &oversized, MAX_INLINE_SOURCE, RunScope::default()) {
            Err(err) => assert!(err.contains("128KiB"), "{err}"),
            Ok(_) => panic!("expected oversized inline source to fail"),
        }
    }

    const FLOW_HOOK_SOURCE: &str =
        r#"#{ inject: [], apply: |host| { host.on("tools/pre-execute", |c| { () }); } }"#;

    /// 五个流程钩子只给**从插件目录装载**的包：程序侧 define 的会话包挂上去要报错。
    #[tokio::test]
    async fn flow_hooks_are_refused_for_a_session_package() {
        let ctx = Context::new();
        let plugin = build_rhai_limited(
            "t",
            FLOW_HOOK_SOURCE,
            MAX_INLINE_SOURCE,
            RunScope::default(),
        )
        .expect("source compiles");
        let err = ctx
            .plugin(plugin, ())
            .unwrap()
            .wait()
            .await
            .expect_err("会话包不该挂上流程钩子");
        assert!(err.to_string().contains("plugins directory"), "{err}");
    }

    /// 从插件目录装载的包挂得上，而且账本上记着这个钩子。
    #[tokio::test]
    async fn flow_hooks_are_open_for_a_disk_package() {
        let ctx = Context::new();
        let scope = RunScope {
            disk_backed: true,
            ..RunScope::default()
        };
        let surface = scope.surface.clone();
        let plugin = build_rhai_limited("t", FLOW_HOOK_SOURCE, MAX_INLINE_SOURCE, scope)
            .expect("source compiles");
        ctx.plugin(plugin, ()).unwrap().wait().await.unwrap();
        assert_eq!(surface.lock().unwrap().hooks, vec!["tools/pre-execute"]);
    }
}
