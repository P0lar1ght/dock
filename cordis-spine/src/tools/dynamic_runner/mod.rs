//! Named `"dynamicCordisRunner"` service: immutable packages, one active run
//! per Plugin, host halves hung under `cordis-dynamic` via `ctx.plugin` /
//! `fiber.dispose`.

mod factories;
mod lifecycle;
mod persist;
mod registry;
mod rhai_host;
mod rhai_codec;
mod rhai_http;

use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Fiber, FiberState, Inject, Plugin};
use tokio::sync::watch;

use crate::host::slash::{Slash, SlashEntry};
use crate::host::tui_slots::TuiSlots;
use crate::names::{DYNAMIC_CORDIS_RUNNER, RHAI_BAGS, SLASH, TOOLS, TUI_SLOTS};
use crate::tools::registry::Tools;

use factories::{list_factories, lookup_factory, DYN_HOLD_GATE};
use lifecycle::{host_status, missing_services, start_host_half};
use registry::{
    fiber_label, package_from_factory, package_from_rhai, wait_start, PluginRec, Registry, Run,
    StartJoin,
};

pub use factories::{
    DynEcho, DynNote, FactoryInfo, DYN_ECHO, DYN_ECHO_TOOL, DYN_NOTE, RHAI_FACTORY,
};
pub use persist::{plugin_roots, PromoteReceipt};
pub use registry::{Attempt, AttemptStatus, Package, PersistScope, PluginOrigin, RunMode};
pub use rhai_host::{builtins_lines, RhaiBag, RhaiBags};

use rhai_host::{MAX_FILE_SOURCE, MAX_INLINE_SOURCE};

/// Rhai body for `cordis_define`: inline string or path under a plugin root.
#[derive(Clone, Debug)]
pub enum SourceInput {
    Inline(String),
    Path(String),
    /// Already-resolved path (disk autoload / promote). 每次读之前仍然重新校验，
    /// 免得 define 之后被换成软链溜出去；`dir` 是授权边界，`None` 表示按全局
    /// plugin roots 判。
    #[doc(hidden)]
    ResolvedPath {
        path: std::path::PathBuf,
        dir: Option<std::path::PathBuf>,
    },
}

#[derive(Clone, Debug)]
pub struct DefineReceipt {
    pub plugin_id: String,
    pub package_id: String,
    pub name: String,
    pub purpose: String,
    pub factory: String,
}

#[derive(Clone, Debug)]
pub struct RunReceipt {
    pub plugin_id: String,
    pub package_id: String,
    pub plugin_run_id: String,
    pub mode: RunMode,
    pub status: &'static str,
    pub current_package_id: Option<String>,
    pub next_package_id: Option<String>,
    pub waiting_for: Vec<String>,
    pub provides: Vec<String>,
    pub fiber_state: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StopReceipt {
    pub plugin_id: String,
    pub was_running: bool,
}

#[derive(Clone, Debug)]
pub struct UndefineReceipt {
    pub plugin_id: String,
    pub was_running: bool,
}

#[derive(Clone, Debug)]
pub struct PluginReference {
    pub plugin_id: String,
    pub package_id: String,
    pub name: String,
    pub purpose: String,
    pub running: bool,
}

#[derive(Clone, Debug)]
pub struct FiberRow {
    pub name: String,
    pub state: String,
    pub plugin_id: Option<String>,
    pub package_id: Option<String>,
    pub plugin_run_id: Option<String>,
    pub provides: Vec<String>,
    pub waiting_for: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SnapshotRow {
    pub plugin_id: String,
    pub origin: PluginOrigin,
    pub current_package_id: Option<String>,
    pub next_package_id: Option<String>,
    pub packages: Vec<Package>,
    pub active_run: Option<RunView>,
    pub latest: Option<Attempt>,
}

#[derive(Clone, Debug)]
pub struct RunView {
    pub plugin_run_id: String,
    pub package_id: String,
    pub fiber_name: Option<String>,
    pub fiber_state: Option<String>,
    pub provides: Vec<String>,
    pub waiting_for: Vec<String>,
}

pub(crate) struct Inner {
    pub(crate) registry: Registry,
    group: Option<Fiber>,
    /// When set, Lead waits here after recording in-flight and before start_fresh
    /// so tests can overlap a second `cordis_run` on the same triple.
    start_hold: Option<watch::Sender<bool>>,
}

/// Process-local dynamic Plugin registry and Host-half lifecycle.
#[derive(Clone)]
pub struct DynamicRunner {
    ctx: Context,
    pub(crate) inner: Arc<Mutex<Inner>>,
    /// 脚本 `http_request` 的出网策略（SSRF / 代理）。**在这里存一份**而不是让
    /// `apply_rhai` 每次现读磁盘配置：`tool-web` 也是组合根把 `WebFetchParams`
    /// 递进去、插件体自己不 live-read 环境，这里跟它保持一致，顺带让测试能换掉它。
    http_params: Arc<Mutex<crate::tools::web_fetch::WebFetchParams>>,
}

impl DynamicRunner {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            inner: Arc::new(Mutex::new(Inner {
                registry: Registry::new(),
                group: None,
                start_hold: None,
            })),
            http_params: Arc::new(Mutex::new(crate::tools::web_fetch::web_fetch_params())),
        }
    }

    /// 脚本 HTTP 的出网策略。组合根想换（或测试要开 `allow_local` 打本地服务端）
    /// 就调 [`Self::set_http_params`]。
    pub fn http_params(&self) -> crate::tools::web_fetch::WebFetchParams {
        self.http_params.lock().unwrap().clone()
    }

    pub fn set_http_params(&self, params: crate::tools::web_fetch::WebFetchParams) {
        *self.http_params.lock().unwrap() = params;
    }

    pub fn factories(&self) -> &'static [FactoryInfo] {
        list_factories()
    }

    #[allow(clippy::too_many_arguments)] // 绘制 / 布局 / 注册参数天然多，抽结构体只是把参数搬个家，留给需要时再拆
    pub fn define(
        &self,
        session_id: &str,
        plugin: PluginSel,
        name: &str,
        purpose: &str,
        factory: &str,
        contrib: Option<SlashEntry>,
        source: Option<SourceInput>,
    ) -> Result<DefineReceipt, String> {
        let target = match plugin {
            PluginSel::New { id_prefix } => DefineTarget::New { id_prefix },
            PluginSel::Existing { plugin_id } => DefineTarget::Existing { plugin_id },
        };
        self.define_at(session_id, target, name, purpose, factory, contrib, source)
    }

    #[allow(clippy::too_many_arguments)] // 绘制 / 布局 / 注册参数天然多，抽结构体只是把参数搬个家，留给需要时再拆
    pub(crate) fn define_at(
        &self,
        session_id: &str,
        target: DefineTarget,
        name: &str,
        purpose: &str,
        factory: &str,
        contrib: Option<SlashEntry>,
        source: Option<SourceInput>,
    ) -> Result<DefineReceipt, String> {
        let name = name.trim();
        let purpose = purpose.trim();
        if name.is_empty() {
            return Err("cordis_define needs a non-empty `name`".into());
        }
        if purpose.is_empty() {
            return Err("cordis_define needs a non-empty `purpose`".into());
        }
        let info = lookup_factory(factory.trim()).ok_or_else(|| {
            let known = list_factories()
                .iter()
                .map(|f| f.id)
                .collect::<Vec<_>>()
                .join(", ");
            format!("unknown factory {factory:?}; preset factories: {known}")
        })?;
        if info.id == "slash" && contrib.is_none() {
            return Err(
                "factory \"slash\" needs `command`, `kind` (\"prompt\", \"overlay\", \"slot\", or \"tool\"), and `text`"
                    .into(),
            );
        }
        if info.id != "slash" && contrib.is_some() {
            return Err("command/kind/text are only for factory \"slash\"".into());
        }

        let prepared_rhai = if info.id == RHAI_FACTORY {
            Some(prepare_rhai_source(source)?)
        } else if source.is_some() {
            return Err("source / source_path are only for factory \"rhai\"".into());
        } else {
            None
        };

        let mut inner = self.inner.lock().unwrap();
        let plugin_id = match target {
            DefineTarget::New { id_prefix } => {
                let prefix = id_prefix.trim();
                if !valid_prefix(prefix) {
                    return Err(
                        "cordis_define `plugin.idPrefix` must contain 3–6 lowercase English letters"
                            .into(),
                    );
                }
                let plugin_id = inner.registry.mint_plugin_id(prefix);
                inner.registry.add(PluginRec {
                    plugin_id: plugin_id.clone(),
                    session_id: session_id.into(),
                    origin: PluginOrigin::Session,
                    packages: indexmap::IndexMap::new(),
                    current_package_id: None,
                    next_package_id: None,
                    run: None,
                    latest: None,
                });
                plugin_id
            }
            DefineTarget::Existing { plugin_id } => {
                owned(&inner.registry, session_id, &plugin_id)?;
                plugin_id
            }
            DefineTarget::Exact { plugin_id, origin } => {
                if !persist::valid_disk_id(&plugin_id) {
                    return Err(format!(
                        "disk plugin id {plugin_id:?} must be 2–32 chars, start with a-z, then a-z0-9-"
                    ));
                }
                if inner.registry.get(&plugin_id).is_some() {
                    return Err(format!("plugin \"{plugin_id}\" is already defined"));
                }
                inner.registry.add(PluginRec {
                    plugin_id: plugin_id.clone(),
                    session_id: session_id.into(),
                    origin,
                    packages: indexmap::IndexMap::new(),
                    current_package_id: None,
                    next_package_id: None,
                    run: None,
                    latest: None,
                });
                plugin_id
            }
        };
        let package_id = inner.registry.mint_package_id();
        let rec = inner
            .registry
            .get_mut(&plugin_id)
            .expect("just inserted or looked up");
        let pkg = if let Some(prepared) = prepared_rhai {
            package_from_rhai(
                package_id.clone(),
                name.into(),
                purpose.into(),
                prepared.inject,
                prepared.inline,
                prepared.path,
            )
        } else {
            package_from_factory(
                package_id.clone(),
                name.into(),
                purpose.into(),
                info,
                contrib,
            )
        };
        rec.packages.insert(package_id.clone(), pkg);
        Ok(DefineReceipt {
            plugin_id,
            package_id,
            name: name.into(),
            purpose: purpose.into(),
            factory: info.id.into(),
        })
    }

    pub async fn run(
        &self,
        session_id: &str,
        plugin_id: &str,
        package_id: &str,
        mode: RunMode,
    ) -> Result<RunReceipt, String> {
        #[allow(clippy::large_enum_variant)] // 局部枚举，box 会牵动匹配点，先保留
        enum Prepared {
            Wait(tokio::sync::watch::Receiver<Option<registry::StartOutcome>>),
            Lead {
                tx: tokio::sync::watch::Sender<Option<registry::StartOutcome>>,
                plugin_run_id: String,
                plan: Plan,
                hold: Option<watch::Sender<bool>>,
            },
        }

        let prepared = {
            let mut inner = self.inner.lock().unwrap();
            owned(&inner.registry, session_id, plugin_id)?;
            let plan = resolve_plan(&inner.registry, plugin_id, package_id, mode)?;
            let _ = lookup_factory(&plan.factory)
                .ok_or_else(|| format!("factory {} is no longer registered", plan.factory))?;
            match inner.registry.join_or_begin(plugin_id, package_id, mode)? {
                StartJoin::Wait(rx) => Prepared::Wait(rx),
                StartJoin::Lead(tx) => {
                    let plugin_run_id = inner.registry.mint_run_id();
                    if let Some(rec) = inner.registry.get_mut(plugin_id) {
                        rec.next_package_id = Some(package_id.to_string());
                        rec.latest = Some(Attempt {
                            plugin_run_id: plugin_run_id.clone(),
                            package_id: package_id.to_string(),
                            mode,
                            status: AttemptStatus::StartingHost,
                            host_error: None,
                        });
                    }
                    Prepared::Lead {
                        tx,
                        plugin_run_id,
                        plan,
                        hold: inner.start_hold.clone(),
                    }
                }
            }
        };

        match prepared {
            Prepared::Wait(rx) => wait_start(rx).await,
            Prepared::Lead {
                tx,
                plugin_run_id,
                plan,
                hold,
            } => {
                if let Some(tx) = hold {
                    let mut rx = tx.subscribe();
                    loop {
                        if *rx.borrow() {
                            break;
                        }
                        if rx.changed().await.is_err() {
                            break;
                        }
                    }
                }
                let outcome = self
                    .lead_start(plugin_id, package_id, plugin_run_id, mode, plan)
                    .await;
                if let Err(message) = &outcome {
                    let mut inner = self.inner.lock().unwrap();
                    if let Some(rec) = inner.registry.get_mut(plugin_id) {
                        if let Some(latest) = rec.latest.as_mut() {
                            if latest.status == AttemptStatus::StartingHost {
                                latest.status = AttemptStatus::Failed;
                                latest.host_error = Some(message.clone());
                            }
                        }
                    }
                }
                let _ = tx.send(Some(outcome.clone()));
                self.inner.lock().unwrap().registry.end_start(plugin_id);
                outcome
            }
        }
    }

    async fn lead_start(
        &self,
        plugin_id: &str,
        package_id: &str,
        plugin_run_id: String,
        mode: RunMode,
        plan: Plan,
    ) -> Result<RunReceipt, String> {
        let info = lookup_factory(&plan.factory)
            .ok_or_else(|| format!("factory {} is no longer registered", plan.factory))?;
        let built = if plan.factory == RHAI_FACTORY {
            let (source, limit) = resolve_plan_source(&plan)?;
            rhai_host::build_rhai_limited(plugin_id, &source, limit)?
        } else {
            info.build(plugin_id, plan.contrib.as_ref())
        };
        self.start_fresh(
            plugin_id,
            package_id,
            plugin_run_id,
            mode,
            built,
            plan.inject,
            plan.provides,
        )
        .await
    }

    #[doc(hidden)]
    pub fn test_set_start_hold(&self, hold: Option<watch::Sender<bool>>) {
        self.inner.lock().unwrap().start_hold = hold;
    }

    #[doc(hidden)]
    pub fn test_is_starting(&self, plugin_id: &str) -> bool {
        self.inner.lock().unwrap().registry.is_starting(plugin_id)
    }

    #[doc(hidden)]
    pub fn test_in_flight_waiters(&self, plugin_id: &str) -> usize {
        self.inner
            .lock()
            .unwrap()
            .registry
            .in_flight_waiters(plugin_id)
    }

    pub async fn stop(&self, session_id: &str, plugin_id: &str) -> Result<StopReceipt, String> {
        let run = {
            let mut inner = self.inner.lock().unwrap();
            owned(&inner.registry, session_id, plugin_id)?;
            let rec = inner
                .registry
                .get_mut(plugin_id)
                .ok_or_else(|| missing_plugin_message(plugin_id))?;
            rec.run.take()
        };
        let was_running = run.is_some();
        if let Some(run) = run {
            if let Some(fiber) = run.fiber {
                let _ = fiber.dispose().await;
            }
        }
        let mut inner = self.inner.lock().unwrap();
        if let Some(rec) = inner.registry.get_mut(plugin_id) {
            if let Some(latest) = rec.latest.as_mut() {
                latest.status = AttemptStatus::Stopped;
            }
        }
        Ok(StopReceipt {
            plugin_id: plugin_id.into(),
            was_running,
        })
    }

    async fn stop_any(&self, plugin_id: &str) -> Result<StopReceipt, String> {
        let run = {
            let mut inner = self.inner.lock().unwrap();
            inner
                .registry
                .get_mut(plugin_id)
                .ok_or_else(|| missing_plugin_message(plugin_id))?
                .run
                .take()
        };
        let was_running = run.is_some();
        if let Some(run) = run {
            if let Some(fiber) = run.fiber {
                let _ = fiber.dispose().await;
            }
        }
        Ok(StopReceipt {
            plugin_id: plugin_id.into(),
            was_running,
        })
    }

    pub async fn undefine(
        &self,
        session_id: &str,
        plugin_id: &str,
    ) -> Result<UndefineReceipt, String> {
        owned(&self.inner.lock().unwrap().registry, session_id, plugin_id)?;
        let stopped = self.stop(session_id, plugin_id).await?;
        self.inner.lock().unwrap().registry.delete(plugin_id);
        Ok(UndefineReceipt {
            plugin_id: plugin_id.into(),
            was_running: stopped.was_running,
        })
    }

    pub fn snapshot(&self, session_id: &str) -> Vec<SnapshotRow> {
        let inner = self.inner.lock().unwrap();
        inner
            .registry
            .all()
            .filter(|rec| visible_to(rec, session_id))
            .map(|rec| snapshot_row(&self.ctx, rec))
            .collect()
    }

    pub fn inspect_plugin(&self, session_id: &str, plugin_id: &str) -> Result<SnapshotRow, String> {
        let inner = self.inner.lock().unwrap();
        let rec = owned(&inner.registry, session_id, plugin_id)?;
        Ok(snapshot_row(&self.ctx, rec))
    }

    pub fn inspect_package(
        &self,
        session_id: &str,
        plugin_id: &str,
        package_id: &str,
    ) -> Result<(SnapshotRow, Package), String> {
        let inner = self.inner.lock().unwrap();
        let rec = owned(&inner.registry, session_id, plugin_id)?;
        let pkg = rec.packages.get(package_id).cloned().ok_or_else(|| {
            format!("dynamic package \"{package_id}\" does not exist on plugin \"{plugin_id}\"")
        })?;
        Ok((snapshot_row(&self.ctx, rec), pkg))
    }

    pub fn reference(&self, session_id: &str, plugin_id: &str) -> Result<PluginReference, String> {
        let inner = self.inner.lock().unwrap();
        let rec = owned(&inner.registry, session_id, plugin_id)?;
        let package_id = rec
            .next_package_id
            .clone()
            .or_else(|| rec.current_package_id.clone())
            .or_else(|| rec.packages.last().map(|(id, _)| id.clone()))
            .ok_or_else(|| missing_plugin_message(plugin_id))?;
        let pkg = rec
            .packages
            .get(&package_id)
            .ok_or_else(|| missing_plugin_message(plugin_id))?;
        Ok(PluginReference {
            plugin_id: rec.plugin_id.clone(),
            package_id,
            name: pkg.name.clone(),
            purpose: pkg.purpose.clone(),
            running: rec.run.is_some(),
        })
    }

    pub fn live_fibers(&self, session_id: &str) -> Vec<FiberRow> {
        let inner = self.inner.lock().unwrap();
        let mut rows = Vec::new();
        if let Some(group) = &inner.group {
            if group.state() != FiberState::Disposed {
                rows.push(FiberRow {
                    name: group.name(),
                    state: fiber_label(group.state()).into(),
                    plugin_id: None,
                    package_id: None,
                    plugin_run_id: None,
                    provides: Vec::new(),
                    waiting_for: Vec::new(),
                });
            }
        }
        for rec in inner.registry.all().filter(|r| visible_to(r, session_id)) {
            let Some(run) = &rec.run else { continue };
            let Some(fiber) = &run.fiber else { continue };
            let waiting = missing_services(&run.inject, |n| service_present(&self.ctx, n));
            rows.push(FiberRow {
                name: fiber.name(),
                state: fiber_label(fiber.state()).into(),
                plugin_id: Some(rec.plugin_id.clone()),
                package_id: Some(run.package_id.clone()),
                plugin_run_id: Some(run.plugin_run_id.clone()),
                provides: run.provides.clone(),
                waiting_for: waiting,
            });
        }
        rows
    }

    pub fn live_dynamic_services(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if self.ctx.get::<DynEcho>(DYN_ECHO).is_some() {
            out.push((DYN_ECHO.into(), "echo factory".into()));
        }
        if self.ctx.get::<DynNote>(DYN_NOTE).is_some() {
            out.push((DYN_NOTE.into(), "note factory".into()));
        }
        if let Some(bags) = self.ctx.get::<RhaiBags>(RHAI_BAGS) {
            out.extend(bags.list());
        }
        out
    }

    pub async fn shutdown(&self) {
        let ids: Vec<String> = {
            let inner = self.inner.lock().unwrap();
            inner.registry.all().map(|p| p.plugin_id.clone()).collect()
        };
        for id in ids {
            let _ = self.stop_any(&id).await;
        }
        let group = self.inner.lock().unwrap().group.take();
        if let Some(group) = group {
            let _ = group.dispose().await;
        }
    }

    #[allow(clippy::too_many_arguments)] // 绘制 / 布局 / 注册参数天然多，抽结构体只是把参数搬个家，留给需要时再拆
    async fn start_fresh(
        &self,
        plugin_id: &str,
        package_id: &str,
        plugin_run_id: String,
        mode: RunMode,
        built: Plugin,
        inject: Vec<String>,
        provides: Vec<String>,
    ) -> Result<RunReceipt, String> {
        let previous = {
            let mut inner = self.inner.lock().unwrap();
            inner
                .registry
                .get_mut(plugin_id)
                .and_then(|rec| rec.run.take())
        };
        if let Some(prev) = previous {
            if let Some(fiber) = prev.fiber {
                let _ = fiber.dispose().await;
            }
        }

        let group = self.require_group().await?;
        let started = start_host_half(&group, built).await;
        match started {
            Ok(fiber) => {
                let waiting = missing_services(&inject, |n| service_present(&self.ctx, n));
                let status = host_status(Some(&fiber), &waiting);
                let fiber_state = fiber_label(fiber.state()).to_string();
                let mut provides_live: Vec<String> = provides
                    .iter()
                    .filter(|name| service_present(&self.ctx, name))
                    .cloned()
                    .collect();
                if let Some(bags) = self.ctx.get::<RhaiBags>(RHAI_BAGS) {
                    for (name, owner) in bags.list() {
                        if owner == plugin_id && !provides_live.contains(&name) {
                            provides_live.push(name);
                        }
                    }
                }
                {
                    let mut inner = self.inner.lock().unwrap();
                    if let Some(rec) = inner.registry.get_mut(plugin_id) {
                        rec.run = Some(Run {
                            plugin_run_id: plugin_run_id.clone(),
                            package_id: package_id.to_string(),
                            fiber: Some(fiber),
                            inject: inject.clone(),
                            provides: provides_live.clone(),
                        });
                        rec.current_package_id = Some(package_id.to_string());
                        rec.next_package_id = None;
                        if let Some(latest) = rec.latest.as_mut() {
                            latest.status = if waiting.is_empty() {
                                AttemptStatus::Running
                            } else {
                                AttemptStatus::Waiting
                            };
                            latest.host_error = None;
                        }
                    }
                }
                Ok(RunReceipt {
                    plugin_id: plugin_id.into(),
                    package_id: package_id.into(),
                    plugin_run_id,
                    mode,
                    status,
                    current_package_id: Some(package_id.into()),
                    next_package_id: None,
                    waiting_for: waiting,
                    provides: provides_live,
                    fiber_state: Some(fiber_state),
                })
            }
            Err(message) => {
                {
                    let mut inner = self.inner.lock().unwrap();
                    if let Some(rec) = inner.registry.get_mut(plugin_id) {
                        if let Some(latest) = rec.latest.as_mut() {
                            latest.status = AttemptStatus::Failed;
                            latest.host_error = Some(message.clone());
                        }
                    }
                }
                Err(message)
            }
        }
    }

    async fn require_group(&self) -> Result<Fiber, String> {
        {
            let inner = self.inner.lock().unwrap();
            if let Some(group) = &inner.group {
                if group.state() != FiberState::Disposed {
                    return Ok(group.clone());
                }
            }
        }
        let fiber = self
            .ctx
            .plugin(
                plugin("cordis-dynamic", Inject::new(), |_, _: &()| Ok(None)),
                (),
            )
            .map_err(|e| e.to_string())?;
        fiber.wait().await.map_err(|e| e.to_string())?;
        self.inner.lock().unwrap().group = Some(fiber.clone());
        Ok(fiber)
    }
}

#[derive(Clone, Debug)]
pub enum PluginSel {
    New { id_prefix: String },
    Existing { plugin_id: String },
}

#[derive(Clone, Debug)]
pub(crate) enum DefineTarget {
    New {
        id_prefix: String,
    },
    Existing {
        plugin_id: String,
    },
    Exact {
        plugin_id: String,
        origin: PluginOrigin,
    },
}

#[derive(Clone)]
struct Plan {
    factory: String,
    contrib: Option<SlashEntry>,
    source: Option<String>,
    source_path: Option<std::path::PathBuf>,
    /// `source_path` 的授权边界（磁盘插件是它自己的目录；`None` 走全局 roots）。
    source_dir: Option<std::path::PathBuf>,
    inject: Vec<String>,
    provides: Vec<String>,
}

fn visible_to(rec: &PluginRec, session_id: &str) -> bool {
    rec.session_id == session_id || rec.session_id == persist::PERSIST_SESSION
}

fn owned<'a>(
    registry: &'a Registry,
    session_id: &str,
    plugin_id: &str,
) -> Result<&'a PluginRec, String> {
    match registry.get(plugin_id) {
        Some(rec) if visible_to(rec, session_id) => Ok(rec),
        _ => Err(missing_plugin_message(plugin_id)),
    }
}

fn resolve_plan(
    registry: &Registry,
    plugin_id: &str,
    package_id: &str,
    mode: RunMode,
) -> Result<Plan, String> {
    let rec = registry
        .get(plugin_id)
        .ok_or_else(|| missing_plugin_message(plugin_id))?;
    let pkg = rec
        .packages
        .get(package_id)
        .ok_or_else(|| format!("plugin \"{plugin_id}\" has no package \"{package_id}\""))?;
    let current = rec.current_package_id.as_deref();
    match mode {
        RunMode::Update if current.is_none() => {
            return Err(format!(
                "plugin \"{plugin_id}\" has no successful version yet; start \"{package_id}\" with mode \"run\""
            ));
        }
        RunMode::Update if current == Some(package_id) => {
            return Err(format!(
                "package \"{package_id}\" is already current; use mode \"run\""
            ));
        }
        RunMode::Run if current.is_some() && current != Some(package_id) => {
            return Err(format!(
                "package \"{package_id}\" differs from current \"{}\"; use mode \"update\"",
                current.unwrap()
            ));
        }
        _ => {}
    }
    Ok(Plan {
        factory: pkg.factory.clone(),
        contrib: pkg.contrib.clone(),
        source: pkg.source.clone(),
        source_path: pkg.source_path.clone(),
        source_dir: persist::origin_source_dir(&rec.origin).map(|p| p.to_path_buf()),
        inject: pkg.inject.clone(),
        provides: pkg.provides.clone(),
    })
}

fn snapshot_row(ctx: &Context, rec: &PluginRec) -> SnapshotRow {
    let active_run = rec.run.as_ref().map(|run| {
        let waiting = missing_services(&run.inject, |n| service_present(ctx, n));
        RunView {
            plugin_run_id: run.plugin_run_id.clone(),
            package_id: run.package_id.clone(),
            fiber_name: run.fiber.as_ref().map(|f| f.name()),
            fiber_state: run
                .fiber
                .as_ref()
                .map(|f| fiber_label(f.state()).to_string()),
            provides: run
                .provides
                .iter()
                .filter(|name| service_present(ctx, name))
                .cloned()
                .collect(),
            waiting_for: waiting,
        }
    });
    SnapshotRow {
        plugin_id: rec.plugin_id.clone(),
        origin: rec.origin.clone(),
        current_package_id: rec.current_package_id.clone(),
        next_package_id: rec.next_package_id.clone(),
        packages: rec.packages.values().cloned().collect(),
        active_run,
        latest: rec.latest.clone(),
    }
}

fn service_present(ctx: &Context, name: &str) -> bool {
    match name {
        TOOLS => ctx.get::<Tools>(TOOLS).is_some(),
        SLASH => ctx.get::<Slash>(SLASH).is_some(),
        TUI_SLOTS => ctx.get::<TuiSlots>(TUI_SLOTS).is_some(),
        DYN_ECHO => ctx.get::<DynEcho>(DYN_ECHO).is_some(),
        DYN_NOTE => ctx.get::<DynNote>(DYN_NOTE).is_some(),
        DYN_HOLD_GATE => false,
        _ => ctx.get::<RhaiBag>(name).is_some(),
    }
}

fn missing_plugin_message(id: &str) -> String {
    format!(
        "no dynamic plugin \"{id}\" in this process — it may have been removed or lost on restart"
    )
}

fn valid_prefix(prefix: &str) -> bool {
    let bytes = prefix.as_bytes();
    (3..=6).contains(&bytes.len()) && bytes.iter().all(|b| b.is_ascii_lowercase())
}

struct PreparedRhai {
    inject: Vec<String>,
    inline: Option<String>,
    path: Option<std::path::PathBuf>,
}

/// Compile-check Rhai at define time.
fn prepare_rhai_source(source: Option<SourceInput>) -> Result<PreparedRhai, String> {
    let input = source
        .ok_or_else(|| "factory \"rhai\" needs `source` or `source_path` (not both)".to_string())?;
    match input {
        SourceInput::Inline(src) => {
            let meta = rhai_host::preflight(&src)?;
            Ok(PreparedRhai {
                inject: meta.inject,
                inline: Some(src),
                path: None,
            })
        }
        SourceInput::Path(raw) => {
            let path = persist::resolve_source_path(&raw)?;
            let text = persist::read_source_file(&path, MAX_FILE_SOURCE, None)?;
            let meta = rhai_host::preflight_limited(&text, MAX_FILE_SOURCE)?;
            Ok(PreparedRhai {
                inject: meta.inject,
                inline: None,
                path: Some(path),
            })
        }
        SourceInput::ResolvedPath { path, dir } => {
            let text = persist::read_source_file(&path, MAX_FILE_SOURCE, dir.as_deref())?;
            let meta = rhai_host::preflight_limited(&text, MAX_FILE_SOURCE)?;
            Ok(PreparedRhai {
                inject: meta.inject,
                inline: None,
                path: Some(path),
            })
        }
    }
}

fn resolve_plan_source(plan: &Plan) -> Result<(String, usize), String> {
    match (&plan.source, &plan.source_path) {
        (Some(s), None) => Ok((s.clone(), MAX_INLINE_SOURCE)),
        (None, Some(p)) => Ok((
            persist::read_source_file(p, MAX_FILE_SOURCE, plan.source_dir.as_deref())?,
            MAX_FILE_SOURCE,
        )),
        (Some(_), Some(_)) => Err("rhai package has both source and source_path".into()),
        (None, None) => Err("rhai package is missing source".into()),
    }
}

/// Named `"dynamicCordisRunner"` service. Hot-plugged bodies hang under a
/// `cordis-dynamic` group fiber; teardown is ordinary `fiber.dispose()`.
pub fn dynamic_runner() -> Plugin {
    plugin("dynamic-runner", Inject::new(), |ctx, _: &()| {
        ctx.provide(RHAI_BAGS, RhaiBags::new())?;
        let runner = DynamicRunner::new(ctx.clone());
        ctx.provide(DYNAMIC_CORDIS_RUNNER, runner.clone())?;
        Ok(Some(Disposable::from_async(move || async move {
            runner.shutdown().await;
            Ok(())
        })))
    })
}
