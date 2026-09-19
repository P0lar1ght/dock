//! Rhai host-half evaluator. Compile at define; eval `apply` at run.
//! Engine setup copied from `vendor/xai/workflow` (DummyModuleResolver,
//! max ops, disable eval) — not the workflow agent-host channel.

use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Inject, Plugin};
use rhai::{Array, Dynamic, Engine, FnPtr, ImmutableString, Map, AST};
use serde_json::Value;

use crate::host::slash::{slash_name_reserved, ExtraSlashKind, Slash, SlashEntry};
use crate::host::tui_slots::{SlotHandler, SlotKeyResult, TuiSlots};
use crate::names::{RHAI_BAGS, SESSION_EVENT, SLASH, STEP_START, TOOLS, TUI_SLOTS, TURN_END};
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{
    LogEvent, StepStart, ToolCall, ToolSpec, TurnEnd, ORDER_STEP_START_DYNAMIC,
    ORDER_TURN_END_DYNAMIC,
};

/// Inline `source` ceiling (tool-call payload).
pub const MAX_INLINE_SOURCE: usize = 128 * 1024;
/// File-backed `source_path` ceiling (re-read on run/update).
pub const MAX_FILE_SOURCE: usize = 1024 * 1024;
const DEFINE_MAX_OPS: u64 = 100_000;
const RUN_MAX_OPS: u64 = 1_000_000;

const SOURCE_SHAPE_HINT: &str = "source must be a map #{ inject: [...], apply: |host| { ... } }; define compiles and does not call apply.";

const PARAMETERS_MUST_BE_MAP: &str = "host.register_tool parameters must be a map, not a JSON string. Use:\n\
    parameters: #{ type: \"object\", properties: #{ text: #{ type: \"string\" } }, required: [\"text\"] }";

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
        "Register a TUI slot: render() -> String, on_key(key) -> \"close\" | ().",
        &["host.register_slot(#{ id, title, hud?, render, on_key })"],
    ),
    (
        "host.open_slot",
        "Ask the TUI to open a registered slot overlay.",
        &["host.open_slot(id: String)"],
    ),
    (
        "host.call_tool",
        "Execute a live model tool (permissions still apply).",
        &["host.call_tool(name: String, args: Map | String) -> String"],
    ),
    (
        "host.on",
        "Observe \"session/event\", or intercept the two scriptable waterfalls. session/event payload is a short line: user\\t… / assistant\\t… / tool\\tname / reminder\\t…, handled after emit returns. \"agent/step-start\" (#{ step, identity, main }) runs before every sample and \"agent/turn-end\" (#{ text, rounds, ended_with_text, queued_followups, identity, main }) runs when the turn wants to end; return a <system-reminder> string to inject / keep working, or () for no opinion. Those two run inline and block the turn, bounded by max_operations; a throw is treated as no opinion. Built-in slots outrank a script. The other waterfalls are not scriptable. Stop unregisters the listener.",
        &[
            "host.on(\"session/event\", |line| { ... })",
            "host.on(\"agent/step-start\", |step| { ... })",
            "host.on(\"agent/turn-end\", |end| { ... })",
        ],
    ),
    (
        "host.log",
        "Tagged host log, also used by print().",
        &["host.log(message: String)"],
    ),
    super::rhai_http::HTTP_BUILTIN,
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
    engine.set_max_string_size(1024 * 1024);
    engine.set_max_array_size(16_384);
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
            return Err("rhai source exceeds 128KiB".into());
        }
        return Err("rhai source file exceeds 1MiB".into());
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

pub fn build_rhai_limited(
    fiber_name: &str,
    source: &str,
    max_bytes: usize,
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
        apply_rhai(ctx, &name, &source).map_err(cordis::Error::plugin)?;
        Ok(None)
    }))
}

fn apply_rhai(ctx: &Context, plugin_id: &str, source: &str) -> Result<(), String> {
    let mut engine = sandboxed_engine(RUN_MAX_OPS);
    let tag = format!("[cordis:{plugin_id}]");
    engine.on_print(move |s| eprintln!("{tag} {s}"));
    register_host(&mut engine);
    // 只挂在**运行期**引擎上。define 期的 `preflight` 会 eval 源码顶层，挂上去
    // 等于 `cordis_define` 本身就能发请求——那是权限门够不着的时机。
    let http_params = ctx
        .get::<super::DynamicRunner>(crate::names::DYNAMIC_CORDIS_RUNNER)
        .map(|r| r.http_params())
        .unwrap_or_else(crate::tools::web_fetch::web_fetch_params);
    super::rhai_http::register(&mut engine, ctx.clone(), plugin_id.to_string(), http_params);
    // Codecs + HMAC: pure, no perms — still runtime-only so define-time preflight cannot call them.
    super::rhai_codec::register(&mut engine);
    // Wall clock: SystemTime epoch helpers (timestamp() is Instant-only).
    super::rhai_time::register(&mut engine);
    let ast = engine
        .compile(source)
        .map_err(|e| format!("rhai syntax: {e}"))?;
    let engine = Arc::new(engine);
    let inner = Arc::new(HostInner {
        ctx: ctx.clone(),
        plugin_id: plugin_id.to_string(),
        engine: engine.clone(),
        ast: ast.clone(),
        disposers: Mutex::new(Vec::new()),
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
    // that `cordis_stop` could not see (`run` was never set).
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
    engine.register_fn("call_tool", Host::call_tool);
    engine.register_fn("on", Host::on);
    engine.register_fn("log", Host::log);
}

#[derive(Clone)]
struct Host {
    inner: Arc<HostInner>,
}

struct HostInner {
    ctx: Context,
    plugin_id: String,
    engine: Arc<Engine>,
    ast: AST,
    disposers: Mutex<Vec<cordis::Disposable>>,
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
        let body: ToolBody = Arc::new(move |call| {
            let engine = engine.clone();
            let ast = ast.clone();
            let execute = execute.clone();
            Box::pin(async move {
                let arguments = call.arguments.clone();
                let content = match tokio::task::spawn_blocking(move || {
                    call_fnptr(&engine, &ast, &execute, json_str_to_dynamic(&arguments))
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
        let d = tools
            .register_dynamic(
                ToolSpec {
                    name,
                    description,
                    parameters_json: parameters,
                },
                body,
            )
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        Ok(())
    }

    fn register_slash(&mut self, spec: Map) -> Result<(), Box<rhai::EvalAltResult>> {
        let command = map_str(&spec, "command")
            .ok_or_else(|| eval_err("register_slash needs command".into()))?;
        if slash_name_reserved(&command) {
            let n = command.trim().trim_start_matches('/');
            return err(format!(
                "cannot shadow builtin slash /{n} — extras cannot replace /agents, /help, /quit, …; pick a different command"
            ));
        }
        let slash = self
            .inner
            .ctx
            .get::<Slash>(SLASH)
            .ok_or_else(|| eval_err("slash is not mounted; inject [\"slash\"]".into()))?;
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
        let handler = Arc::new(RhaiSlot {
            title,
            hud,
            engine: self.inner.engine.clone(),
            ast: self.inner.ast.clone(),
            render,
            on_key,
        });
        let d = slots
            .register(id, handler)
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
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
        let text = block_on_tool(tools.as_ref(), call)?;
        Ok(text.into())
    }

    fn on(
        &mut self,
        event: ImmutableString,
        handler: FnPtr,
    ) -> Result<(), Box<rhai::EvalAltResult>> {
        let name = event.to_string();
        match name.as_str() {
            SESSION_EVENT => self.on_session_event(handler),
            STEP_START => self.on_step_start(handler),
            TURN_END => self.on_turn_end(handler),
            _ => err(format!(
                "host.on supports {SESSION_EVENT:?}, {STEP_START:?} and {TURN_END:?} (got {name:?}); \
                 the other waterfalls are not scriptable"
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
        move |payload: Map| match call_fnptr_raw(
            &engine,
            &ast,
            &handler,
            Dynamic::from_map(payload),
        ) {
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

    fn on_session_event(&mut self, handler: FnPtr) -> Result<(), Box<rhai::EvalAltResult>> {
        let engine = self.inner.engine.clone();
        let ast = self.inner.ast.clone();
        let plugin_id = self.inner.plugin_id.clone();
        let d = self
            .inner
            .ctx
            .on(SESSION_EVENT, move |ev: &LogEvent| {
                let Some(line) = event_line(ev) else {
                    return;
                };
                let engine = engine.clone();
                let ast = ast.clone();
                let handler = handler.clone();
                let plugin_id = plugin_id.clone();
                match tokio::runtime::Handle::try_current() {
                    Ok(handle) => {
                        handle.spawn(async move {
                            let result = tokio::task::spawn_blocking(move || {
                                call_fnptr(&engine, &ast, &handler, Dynamic::from(line))
                            })
                            .await;
                            if let Ok(Err(e)) = result {
                                eprintln!("[cordis:{plugin_id}] host.on: {e}");
                            } else if let Err(e) = result {
                                eprintln!("[cordis:{plugin_id}] host.on: {e}");
                            }
                        });
                    }
                    Err(_) => {
                        if let Err(e) = call_fnptr(&engine, &ast, &handler, Dynamic::from(line)) {
                            eprintln!("[cordis:{plugin_id}] host.on: {e}");
                        }
                    }
                }
            })
            .map_err(|e| eval_err(e.to_string()))?;
        self.own(d)?;
        Ok(())
    }

    fn log(&mut self, message: ImmutableString) {
        eprintln!("[cordis:{}] {message}", self.inner.plugin_id);
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
    engine: Arc<Engine>,
    ast: AST,
    render: FnPtr,
    on_key: Option<FnPtr>,
}

impl SlotHandler for RhaiSlot {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn hud(&self) -> bool {
        self.hud
    }

    fn render(&self) -> String {
        match call_fnptr(&self.engine, &self.ast, &self.render, Dynamic::UNIT) {
            Ok(text) => text,
            Err(e) => format!("(render error: {e})"),
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
    };
    Some(line)
}

fn preview(text: &str, max: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if flat.chars().count() <= max {
        return flat.to_string();
    }
    let mut out = String::new();
    for (i, c) in flat.chars().enumerate() {
        if i >= max {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
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

fn block_on_tool(tools: &Tools, call: ToolCall) -> Result<String, Box<rhai::EvalAltResult>> {
    let handle = tokio::runtime::Handle::try_current().map_err(|e| eval_err(e.to_string()))?;
    let tools = tools.clone();
    let result = tokio::task::block_in_place(|| handle.block_on(tools.execute(call)));
    Ok(result.content)
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
        .chain(super::rhai_time::BUILTINS.iter())
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
        match build_rhai_limited("t", &oversized, MAX_INLINE_SOURCE) {
            Err(err) => assert!(err.contains("128KiB"), "{err}"),
            Ok(_) => panic!("expected oversized inline source to fail"),
        }
    }
}
