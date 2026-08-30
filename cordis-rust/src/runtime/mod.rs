mod dispatch;
mod lifecycle;
mod registry;
mod services;
mod world;

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::watch;

use crate::dispose::EffectMeta;
use crate::ids::{Epoch, FiberId, FiberState};
use crate::logger::Logger;
use crate::scope::{InterceptScope, IsolateScope};

use world::{DisposableList, FiberData, World};

pub(crate) type FilterFn = Arc<dyn Fn(&dyn Any) -> bool + Send + Sync>;

pub(crate) struct Runtime {
    inner: Mutex<World>,
    pub logger: Logger,
}

impl Runtime {
    pub(crate) fn new() -> (Arc<Self>, FiberId, Arc<IsolateScope>, Arc<InterceptScope>) {
        let isolate = Arc::new(IsolateScope::default());
        let intercept = Arc::new(InterceptScope::default());
        let root = FiberId(0);
        let (settled, _) = watch::channel(true);
        let mut fibers = HashMap::new();
        fibers.insert(
            root,
            FiberData {
                uid: Some(0),
                parent: None,
                state: FiberState::Active,
                inject: Vec::new(),
                config: Arc::new(()),
                apply: None,
                epoch: Epoch::Active(String::new()),
                error: None,
                disposables: DisposableList::new(),
                store: Some(HashMap::new()),
                pending_store: HashMap::new(),
                runtime_id: None,
                isolate: isolate.clone(),
                intercept: intercept.clone(),
                inertia: None,
                settled,
                name: "root".into(),
                owner: None,
                update_hooks: Vec::new(),
            },
        );
        let rt = Arc::new(Self {
            inner: Mutex::new(World {
                next_uid: 0,
                default_keys: HashMap::new(),
                store: HashMap::new(),
                plugins: HashMap::new(),
                fibers,
                hooks: HashMap::new(),
            }),
            logger: Logger::new(),
        });
        (rt, root, isolate, intercept)
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, World> {
        self.inner.lock().unwrap()
    }

    pub(crate) fn fiber_name(&self, id: FiberId) -> String {
        self.lock()
            .fibers
            .get(&id)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "disposed".into())
    }

    pub(crate) fn fiber_state(&self, id: FiberId) -> FiberState {
        self.lock()
            .fibers
            .get(&id)
            .map(|f| f.state)
            .unwrap_or(FiberState::Disposed)
    }

    pub(crate) fn fiber_uid(&self, id: FiberId) -> Option<u64> {
        self.lock().fibers.get(&id).and_then(|f| f.uid)
    }

    pub(crate) fn effects(&self, id: FiberId) -> Vec<EffectMeta> {
        let w = self.lock();
        let Some(f) = w.fibers.get(&id) else {
            return Vec::new();
        };
        f.disposables.iter().filter_map(|d| d.meta()).collect()
    }

    pub(crate) fn disposable_count(&self, id: FiberId) -> usize {
        self.lock()
            .fibers
            .get(&id)
            .map(|f| f.disposables.len())
            .unwrap_or(0)
    }

    pub(crate) fn registry_size(&self) -> usize {
        self.lock().plugins.len()
    }

    pub(crate) fn hook_snapshot(&self) -> HashMap<String, usize> {
        self.lock()
            .hooks
            .iter()
            .filter(|(_, hooks)| !hooks.is_empty())
            .map(|(k, v)| (k.clone(), v.len()))
            .collect()
    }

    pub(crate) fn isolate_key(
        &self,
        ctx: &crate::Context,
        name: &str,
    ) -> Option<crate::IsolateKey> {
        self.lock().resolve_key(&ctx.isolate, name)
    }

    pub(crate) fn root_isolate(&self) -> Arc<IsolateScope> {
        self.lock()
            .fibers
            .get(&FiberId(0))
            .map(|f| f.isolate.clone())
            .unwrap_or_default()
    }

    pub(crate) fn fiber_isolate(&self, id: FiberId) -> Arc<IsolateScope> {
        self.lock()
            .fibers
            .get(&id)
            .map(|f| f.isolate.clone())
            .unwrap_or_default()
    }

    pub(crate) fn fiber_intercept(&self, id: FiberId) -> Arc<InterceptScope> {
        self.lock()
            .fibers
            .get(&id)
            .map(|f| f.intercept.clone())
            .unwrap_or_default()
    }
}

pub struct ListenerView {
    pub(super) isolate: Arc<IsolateScope>,
}

impl ListenerView {
    pub fn isolate_key(&self, name: &str) -> Option<crate::IsolateKey> {
        self.isolate.get(name)
    }
}
