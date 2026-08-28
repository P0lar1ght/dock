use std::sync::atomic::{AtomicU64, Ordering};

/// Lifecycle state of one fiber.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FiberState {
    /// Waiting for required services.
    Pending,
    /// Plugin callback is running.
    Loading,
    /// Loaded and providing.
    Active,
    /// Plugin callback or config threw.
    Failed,
    /// Fiber was removed and cannot restart.
    Disposed,
    /// Disposers are running.
    Unloading,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FiberId(pub(crate) u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct PluginId(pub(crate) u64);

/// Isolation realm for one service name.
///
/// Contexts that share a label for a name see the same service instance.
/// `Context::isolate` allocates a fresh label; `isolate_with` reuses one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct IsolateKey(pub(crate) u64);

static NEXT_FIBER: AtomicU64 = AtomicU64::new(1);
static NEXT_PLUGIN: AtomicU64 = AtomicU64::new(1);
static NEXT_ISOLATE: AtomicU64 = AtomicU64::new(1);
static NEXT_HOOK: AtomicU64 = AtomicU64::new(1);
static NEXT_SLOT: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_fiber() -> FiberId {
    FiberId(NEXT_FIBER.fetch_add(1, Ordering::Relaxed))
}

pub(crate) fn next_plugin() -> PluginId {
    PluginId(NEXT_PLUGIN.fetch_add(1, Ordering::Relaxed))
}

pub(crate) fn next_isolate() -> IsolateKey {
    IsolateKey(NEXT_ISOLATE.fetch_add(1, Ordering::Relaxed))
}

pub(crate) fn next_hook() -> u64 {
    NEXT_HOOK.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn next_slot() -> u64 {
    NEXT_SLOT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Epoch {
    Inactive,
    Active(String),
}

impl Epoch {
    pub(crate) fn is_inactive(&self) -> bool {
        matches!(self, Epoch::Inactive)
    }
}
