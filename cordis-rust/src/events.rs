use std::any::Any;
use std::sync::Arc;

use crate::error::Result;

pub(crate) type Payload = Arc<dyn Any + Send + Sync>;

/// Arguments passed to an event listener.
pub struct EventArgs {
    pub payload: Payload,
    pub(crate) next: Option<Arc<dyn Fn() -> Payload + Send + Sync>>,
}

impl EventArgs {
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.payload.downcast_ref::<T>()
    }

    /// Invoke the remaining waterfall chain. `None` when this is not a waterfall.
    pub fn next<T: Clone + 'static>(&self) -> Option<T> {
        let payload = (self.next.as_ref())?();
        payload.downcast_ref::<T>().cloned()
    }
}

pub(crate) type SyncHandler = Arc<dyn Fn(EventArgs) -> Result<Option<Payload>> + Send + Sync>;

/// Options for [`crate::Context::on`].
#[derive(Clone, Copy, Debug, Default)]
pub struct EventOptions {
    pub prepend: bool,
    pub global: bool,
    /// Unregister after the first dispatch (JS `ctx.once()`).
    pub once: bool,
}

/// Payload for `internal/service`. Fires when a named service appears or
/// disappears in some isolate realm — the seam for watching the service table
/// without polling it.
///
/// `value` is `None` after the provider unloads. Read it, do not stash it: the
/// same live-lookup rule as everywhere else applies.
#[derive(Clone)]
pub struct ServiceEvent {
    pub name: String,
    pub value: Option<Payload>,
}

impl std::fmt::Debug for ServiceEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceEvent")
            .field("name", &self.name)
            .field("present", &self.value.is_some())
            .finish()
    }
}

/// Payload for `internal/listener`: a listener is about to be registered.
///
/// Dispatched as a **bail** — return a value to veto the registration
/// (`ctx.on` then fails with [`crate::Error::ListenerRejected`]). Upstream uses
/// the same shape to let a plugin claim an event name for itself.
#[derive(Clone, Debug)]
pub struct ListenerEvent {
    pub name: String,
    pub prepend: bool,
    pub global: bool,
    pub once: bool,
}

/// How an event was dispatched. Payload field of [`DispatchEvent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchMode {
    Emit,
    Bail,
    Waterfall,
    Parallel,
}

/// Payload for `internal/dispatch`: every non-`internal/` dispatch, for tracing.
///
/// **Only built when someone is listening** — the emit path checks the hook
/// list first, so an unobserved kernel pays one map lookup, not a payload
/// allocation per event. Upstream guards it the same way.
#[derive(Clone, Debug)]
pub struct DispatchEvent {
    pub mode: DispatchMode,
    pub name: String,
}

/// Fiber-local `internal/update` waterfall payload. Skip `next()` to veto restart.
#[derive(Clone, Copy, Debug)]
pub struct UpdateEvent {
    pub no_save: bool,
}

impl From<bool> for EventOptions {
    fn from(prepend: bool) -> Self {
        Self {
            prepend,
            global: false,
            once: false,
        }
    }
}

/// A value is "bailed" when serial/bail should stop (JS: not null/false/undefined).
pub fn is_bailed<T>(value: &Option<T>) -> bool {
    value.is_some()
}
