use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::watch;

use crate::dispose::Disposable;
use crate::events::SyncHandler;
use crate::ids::{next_isolate, next_slot, Epoch, FiberId, FiberState, IsolateKey, PluginId};
use crate::plugin::{ApplyFn, Config};
use crate::scope::{InterceptScope, IsolateScope};

#[derive(Clone)]
pub(crate) struct Impl {
    pub name: String,
    pub fiber: FiberId,
    pub value: Arc<dyn Any + Send + Sync>,
}

pub(crate) struct PluginRuntime {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub apply: ApplyFn,
    pub fibers: Vec<FiberId>,
}

pub(crate) struct Hook {
    pub id: u64,
    #[allow(dead_code)]
    pub fiber: FiberId,
    pub isolate: Arc<IsolateScope>,
    pub global: bool,
    pub handler: SyncHandler,
}

#[derive(Clone, Copy)]
pub(crate) enum InertiaKind {
    Loading,
    Unloading,
}

pub(crate) struct DisposableList {
    items: Vec<(u64, Disposable)>,
}

impl DisposableList {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn push(&mut self, d: Disposable) -> u64 {
        let sn = next_slot();
        self.items.push((sn, d));
        sn
    }

    pub fn steal(&mut self, d: &Disposable) -> bool {
        let n = self.items.len();
        self.items.retain(|(_, x)| !Disposable::ptr_eq(x, d));
        self.items.len() != n
    }

    pub fn clear(&mut self) -> Vec<Disposable> {
        let mut v: Vec<_> = self.items.drain(..).map(|(_, d)| d).collect();
        v.reverse();
        v
    }

    pub fn iter(&self) -> impl Iterator<Item = &Disposable> {
        self.items.iter().map(|(_, d)| d)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

pub(crate) struct FiberData {
    pub uid: Option<u64>,
    #[allow(dead_code)]
    pub parent: Option<FiberId>,
    pub state: FiberState,
    pub inject: Vec<String>,
    pub config: Config,
    pub apply: Option<ApplyFn>,
    pub epoch: Epoch,
    pub error: Option<String>,
    pub disposables: DisposableList,
    pub store: Option<HashMap<String, Impl>>,
    pub pending_store: HashMap<String, Impl>,
    pub runtime_id: Option<PluginId>,
    pub isolate: Arc<IsolateScope>,
    pub intercept: Arc<InterceptScope>,
    pub inertia: Option<InertiaKind>,
    pub settled: watch::Sender<bool>,
    pub name: String,
    pub owner: Option<Disposable>,
    pub update_hooks: Vec<(u64, SyncHandler)>,
}

pub(super) fn derived_state(f: &FiberData) -> FiberState {
    if f.uid.is_none() {
        FiberState::Disposed
    } else if f.error.is_some() {
        FiberState::Failed
    } else if !f.epoch.is_inactive() {
        FiberState::Active
    } else {
        FiberState::Pending
    }
}

pub(crate) struct World {
    pub next_uid: u64,
    pub default_keys: HashMap<String, IsolateKey>,
    pub store: HashMap<IsolateKey, Impl>,
    pub plugins: HashMap<PluginId, PluginRuntime>,
    pub fibers: HashMap<FiberId, FiberData>,
    pub hooks: HashMap<String, Vec<Hook>>,
}

impl World {
    pub fn alloc_uid(&mut self) -> u64 {
        self.next_uid += 1;
        self.next_uid
    }

    pub fn resolve_key(&self, isolate: &IsolateScope, name: &str) -> Option<IsolateKey> {
        isolate
            .get(name)
            .or_else(|| self.default_keys.get(name).copied())
    }

    pub fn provide_key(&mut self, isolate: &IsolateScope, name: &str) -> IsolateKey {
        self.default_keys
            .entry(name.to_string())
            .or_insert_with(next_isolate);
        isolate
            .get(name)
            .or_else(|| self.default_keys.get(name).copied())
            .expect("default isolate key")
    }

    pub fn same_realm(&self, a: &IsolateScope, b: &IsolateScope, name: &str) -> bool {
        self.resolve_key(a, name) == self.resolve_key(b, name)
    }

    pub fn get_impl(&self, isolate: &IsolateScope, name: &str, strict: bool) -> Option<&Impl> {
        let key = self.resolve_key(isolate, name)?;
        let impl_ = self.store.get(&key)?;
        if strict {
            let state = self.fibers.get(&impl_.fiber).map(|f| f.state)?;
            if state != FiberState::Active {
                return None;
            }
        }
        Some(impl_)
    }

    pub fn check_impl(&mut self, id: FiberId, name: &str) {
        let isolate = self.fibers.get(&id).map(|f| f.isolate.clone());
        let Some(isolate) = isolate else { return };
        let impl_ = self.get_impl(&isolate, name, true).cloned();
        if let Some(f) = self.fibers.get_mut(&id) {
            match impl_ {
                Some(impl_) => {
                    f.pending_store.insert(name.to_string(), impl_);
                }
                None => {
                    f.pending_store.remove(name);
                }
            }
        }
    }

    pub fn provided_names(&self, id: FiberId) -> Vec<String> {
        self.store
            .values()
            .filter(|impl_| impl_.fiber == id)
            .map(|impl_| impl_.name.clone())
            .collect()
    }
}
