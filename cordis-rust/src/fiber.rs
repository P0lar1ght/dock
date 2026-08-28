use std::fmt;
use std::sync::Arc;

use crate::error::Result;
use crate::ids::{FiberId, FiberState};
use crate::dispose::EffectMeta;
use crate::plugin::Config;
use crate::runtime::Runtime;
use crate::Context;

/// Payload for `internal/status`.
#[derive(Clone, Debug)]
pub struct StatusEvent {
    pub fiber: Fiber,
    pub old: FiberState,
    pub new: FiberState,
}

/// One application of a plugin: dependency epoch, effects, and teardown.
#[derive(Clone)]
pub struct Fiber {
    pub(crate) rt: Arc<Runtime>,
    pub(crate) id: FiberId,
}

impl Fiber {
    pub fn id(&self) -> FiberId {
        self.id
    }

    pub fn uid(&self) -> Option<u64> {
        self.rt.fiber_uid(self.id)
    }

    pub fn name(&self) -> String {
        self.rt.fiber_name(self.id)
    }

    pub fn state(&self) -> FiberState {
        self.rt.fiber_state(self.id)
    }

    pub fn ctx(&self) -> Context {
        Context {
            rt: self.rt.clone(),
            fiber: self.id,
            isolate: self.rt.fiber_isolate(self.id),
            intercept: self.rt.fiber_intercept(self.id),
            filter: None,
        }
    }

    pub fn get_effects(&self) -> Vec<EffectMeta> {
        self.rt.effects(self.id)
    }

    pub fn disposable_count(&self) -> usize {
        self.rt.disposable_count(self.id)
    }

    pub async fn wait(&self) -> Result<()> {
        self.rt.wait(self.id).await
    }

    pub async fn dispose(&self) -> Result<()> {
        self.rt.dispose_fiber(self.id).await
    }

    pub async fn restart(&self) -> Result<()> {
        self.rt.restart(self.id).await
    }

    pub async fn update<C: Send + Sync + 'static>(&self, config: C) -> Result<()> {
        self.update_with(config, false).await
    }

    /// Same as [`Fiber::update`], passing the `no_save` hint through `internal/update`.
    pub async fn update_with<C: Send + Sync + 'static>(
        &self,
        config: C,
        no_save: bool,
    ) -> Result<()> {
        self.rt
            .update(self.id, Arc::new(config) as Config, no_save)
            .await
    }
}

impl fmt::Debug for Fiber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fiber <{}> {:?}", self.name(), self.state())
    }
}
