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
