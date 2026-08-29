//! Process-local dynamic Plugin registry and identity mints.
//! Copied contract from DSH `cordis-host-runner/registry.ts`: define never
//! mounts; packages are immutable; ids are `{prefix}-{n}` / `pkg-{n}` / `run-{n}`.

use std::collections::HashMap;

use cordis::{Fiber, FiberState};
use indexmap::IndexMap;
use tokio::sync::watch;

use super::factories::FactoryInfo;
use super::RunReceipt;
use crate::slash::SlashEntry;

pub type StartOutcome = Result<RunReceipt, String>;

pub enum StartJoin {
    Lead(watch::Sender<Option<StartOutcome>>),
    Wait(watch::Receiver<Option<StartOutcome>>),
}

pub(crate) async fn wait_start(mut rx: watch::Receiver<Option<StartOutcome>>) -> StartOutcome {
    loop {
        if let Some(result) = rx.borrow().clone() {
            return result;
        }
        if rx.changed().await.is_err() {
            return match rx.borrow().clone() {
                Some(result) => result,
                None => Err("plugin start was cancelled".into()),
            };
        }
    }
}

struct InFlight {
    package_id: String,
    mode: RunMode,
    tx: watch::Sender<Option<StartOutcome>>,
    waiters: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunMode {
    Run,
    Update,
}

impl RunMode {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "run" => Ok(Self::Run),
            "update" => Ok(Self::Update),
            other => Err(format!(
                "cordis_run mode must be \"run\" or \"update\", got {other:?}"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Update => "update",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Package {
    pub package_id: String,
    pub name: String,
    pub purpose: String,
    pub factory: String,
    pub provides: Vec<String>,
    pub inject: Vec<String>,
    pub tools: Vec<String>,
    pub contrib: Option<SlashEntry>,
    pub source: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Run {
    pub plugin_run_id: String,
    pub package_id: String,
    pub fiber: Option<Fiber>,
    pub inject: Vec<String>,
    pub provides: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttemptStatus {
    StartingHost,
    Running,
    Waiting,
    Failed,
    Stopped,
}

#[derive(Clone, Debug)]
pub struct Attempt {
    pub plugin_run_id: String,
    pub package_id: String,
    pub mode: RunMode,
    pub status: AttemptStatus,
    pub host_error: Option<String>,
}

pub struct PluginRec {
    pub plugin_id: String,
    pub session_id: String,
    pub packages: IndexMap<String, Package>,
    pub current_package_id: Option<String>,
    pub next_package_id: Option<String>,
    pub run: Option<Run>,
    pub latest: Option<Attempt>,
}

pub struct Registry {
    plugins: IndexMap<String, PluginRec>,
    next_plugin: u64,
    next_package: u64,
    next_run: u64,
    starting: HashMap<String, InFlight>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            plugins: IndexMap::new(),
            next_plugin: 1,
            next_package: 1,
            next_run: 1,
            starting: HashMap::new(),
        }
    }

    pub fn mint_plugin_id(&mut self, prefix: &str) -> String {
        loop {
            let id = format!("{prefix}-{}", self.next_plugin);
            self.next_plugin += 1;
            if !self.plugins.contains_key(&id) {
                return id;
            }
        }
    }

    pub fn mint_package_id(&mut self) -> String {
        let id = format!("pkg-{}", self.next_package);
        self.next_package += 1;
        id
    }

    pub fn mint_run_id(&mut self) -> String {
        let id = format!("run-{}", self.next_run);
        self.next_run += 1;
        id
    }

    pub fn add(&mut self, plugin: PluginRec) {
        self.plugins.insert(plugin.plugin_id.clone(), plugin);
    }

    pub fn get(&self, id: &str) -> Option<&PluginRec> {
        self.plugins.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut PluginRec> {
        self.plugins.get_mut(id)
    }

    pub fn delete(&mut self, id: &str) -> bool {
        self.plugins.shift_remove(id).is_some()
    }

    pub fn all(&self) -> impl Iterator<Item = &PluginRec> {
        self.plugins.values()
    }

    /// Same pluginId+packageId+mode joins the in-flight start; any other
    /// concurrent start on that plugin still fails with already starting.
    pub fn join_or_begin(
        &mut self,
        plugin_id: &str,
        package_id: &str,
        mode: RunMode,
    ) -> Result<StartJoin, String> {
        if let Some(inflight) = self.starting.get_mut(plugin_id) {
            if inflight.package_id == package_id && inflight.mode == mode {
                inflight.waiters += 1;
                return Ok(StartJoin::Wait(inflight.tx.subscribe()));
            }
            return Err(format!("plugin \"{plugin_id}\" is already starting"));
        }
        let (tx, _rx) = watch::channel(None);
        self.starting.insert(
            plugin_id.to_string(),
            InFlight {
                package_id: package_id.to_string(),
                mode,
                tx: tx.clone(),
                waiters: 0,
            },
        );
        Ok(StartJoin::Lead(tx))
    }

    pub fn end_start(&mut self, plugin_id: &str) {
        self.starting.remove(plugin_id);
    }

    pub fn is_starting(&self, plugin_id: &str) -> bool {
        self.starting.contains_key(plugin_id)
    }

    pub fn in_flight_waiters(&self, plugin_id: &str) -> usize {
        self.starting.get(plugin_id).map(|s| s.waiters).unwrap_or(0)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn package_from_factory(
    package_id: String,
    name: String,
    purpose: String,
    info: &FactoryInfo,
    contrib: Option<SlashEntry>,
) -> Package {
    Package {
        package_id,
        name,
        purpose,
        factory: info.id.to_string(),
        provides: info.provides.iter().map(|s| s.to_string()).collect(),
        inject: info.inject.iter().map(|s| s.to_string()).collect(),
        tools: info.tools.iter().map(|s| s.to_string()).collect(),
        contrib,
        source: None,
    }
}

pub fn package_from_rhai(
    package_id: String,
    name: String,
    purpose: String,
    inject: Vec<String>,
    source: String,
) -> Package {
    Package {
        package_id,
        name,
        purpose,
        factory: super::factories::RHAI_FACTORY.to_string(),
        provides: Vec::new(),
        inject,
        tools: Vec::new(),
        contrib: None,
        source: Some(source),
    }
}

pub fn fiber_label(state: FiberState) -> &'static str {
    match state {
        FiberState::Pending => "pending",
        FiberState::Loading => "loading",
        FiberState::Active => "active",
        FiberState::Failed => "failed",
        FiberState::Disposed => "disposed",
        FiberState::Unloading => "unloading",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamic_runner::RunReceipt;

    fn dummy_ok() -> RunReceipt {
        RunReceipt {
            plugin_id: "p-1".into(),
            package_id: "pkg-1".into(),
            plugin_run_id: "run-1".into(),
            mode: RunMode::Run,
            status: "running",
            current_package_id: Some("pkg-1".into()),
            next_package_id: None,
            waiting_for: Vec::new(),
            provides: Vec::new(),
            fiber_state: Some("active".into()),
        }
    }

    #[test]
    fn same_triple_returns_wait() {
        let mut r = Registry::new();
        let a = r.join_or_begin("p-1", "pkg-1", RunMode::Run).unwrap();
        let b = r.join_or_begin("p-1", "pkg-1", RunMode::Run).unwrap();
        assert!(matches!(a, StartJoin::Lead(_)));
        assert!(matches!(b, StartJoin::Wait(_)));
        assert_eq!(r.in_flight_waiters("p-1"), 1);
    }

    #[test]
    fn different_package_is_already_starting() {
        let mut r = Registry::new();
        let _ = r.join_or_begin("p-1", "pkg-1", RunMode::Run).unwrap();
        let err = match r.join_or_begin("p-1", "pkg-2", RunMode::Run) {
            Err(e) => e,
            Ok(_) => panic!("expected already starting"),
        };
        assert!(err.contains("already starting"), "{err}");
    }

    #[test]
    fn different_mode_is_already_starting() {
        let mut r = Registry::new();
        let _ = r.join_or_begin("p-1", "pkg-1", RunMode::Run).unwrap();
        let err = match r.join_or_begin("p-1", "pkg-1", RunMode::Update) {
            Err(e) => e,
            Ok(_) => panic!("expected already starting"),
        };
        assert!(err.contains("already starting"), "{err}");
    }

    #[tokio::test]
    async fn waiter_receives_lead_result() {
        let mut r = Registry::new();
        let StartJoin::Lead(tx) = r.join_or_begin("p-1", "pkg-1", RunMode::Run).unwrap() else {
            panic!("expected lead");
        };
        let StartJoin::Wait(rx) = r.join_or_begin("p-1", "pkg-1", RunMode::Run).unwrap() else {
            panic!("expected wait");
        };
        let expected = dummy_ok();
        tx.send(Some(Ok(expected.clone()))).unwrap();
        drop(tx);
        r.end_start("p-1");
        let got = wait_start(rx).await.unwrap();
        assert_eq!(got.plugin_run_id, expected.plugin_run_id);
    }
}
