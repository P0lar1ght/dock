use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::events::UpdateEvent;
use crate::ids::{Epoch, FiberId, FiberState};
use crate::plugin::Config;
use crate::scope::InterceptScope;

use super::world::{derived_state, InertiaKind};
use super::Runtime;

enum InertiaNext {
    Idle,
    Reload,
    Unload,
}

impl Runtime {
    pub(super) async fn teardown_fiber(self: Arc<Self>, id: FiberId) -> Result<()> {
        {
            let mut w = self.lock();
            if let Some(f) = w.fibers.get_mut(&id) {
                f.uid = None;
            }
        }
        let fiber = crate::Fiber {
            rt: self.clone(),
            id,
        };
        self.emit("internal/plugin", Arc::new(fiber), None);
        {
            let mut w = self.lock();
            let runtime_id = w.fibers.get(&id).and_then(|f| f.runtime_id);
            if let Some(rid) = runtime_id {
                if let Some(runtime) = w.plugins.get_mut(&rid) {
                    runtime.fibers.retain(|fid| *fid != id);
                    if runtime.fibers.is_empty() {
                        w.plugins.remove(&rid);
                    }
                }
            }
        }
        self.set_epoch(id, Epoch::Inactive);
        let need_unload = {
            let w = self.lock();
            w.fibers
                .get(&id)
                .map(|f| f.inertia.is_none())
                .unwrap_or(false)
        };
        if need_unload {
            self.clone().drive(id, false).await;
        }
        self.wait(id).await?;
        Ok(())
    }

    pub(crate) async fn wait(&self, id: FiberId) -> Result<()> {
        let mut rx = {
            let w = self.lock();
            let Some(f) = w.fibers.get(&id) else {
                return Ok(());
            };
            f.settled.subscribe()
        };
        loop {
            {
                let w = self.lock();
                let Some(f) = w.fibers.get(&id) else {
                    return Ok(());
                };
                if f.inertia.is_none() {
                    if let Some(err) = &f.error {
                        return Err(Error::Plugin(err.clone()));
                    }
                    return Ok(());
                }
            }
            if rx.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    pub(crate) async fn dispose_fiber(self: &Arc<Self>, id: FiberId) -> Result<()> {
        if id == FiberId(0) {
            return self.restart(id).await;
        }
        let owner = {
            let w = self.lock();
            w.fibers.get(&id).and_then(|f| f.owner.clone())
        };
        if let Some(owner) = owner {
            owner.dispose().await?;
        }
        Ok(())
    }

    pub(crate) async fn restart(self: &Arc<Self>, id: FiberId) -> Result<()> {
        self.assert_can_effect(id)?;
        self.set_epoch(id, Epoch::Inactive);
        self.refresh(id);
        self.wait(id).await
    }

    pub(crate) async fn update(
        self: &Arc<Self>,
        id: FiberId,
        config: Config,
        no_save: bool,
    ) -> Result<()> {
        self.assert_can_effect(id)?;
        {
            let mut w = self.lock();
            if let Some(f) = w.fibers.get_mut(&id) {
                f.config = config;
                f.error = None;
            }
        }
        if self.fiber_state(id) != FiberState::Active {
            self.set_epoch(id, Epoch::Inactive);
            self.refresh(id);
            return self.wait(id).await;
        }
        let go = Arc::new(AtomicBool::new(false));
        let go2 = go.clone();
        let handlers = self.update_handlers(id);
        self.waterfall_run(
            handlers,
            UpdateEvent { no_save },
            Box::new(move || {
                go2.store(true, Ordering::SeqCst);
                UpdateEvent { no_save }
            }),
        );
        if go.load(Ordering::SeqCst) {
            self.restart(id).await
        } else {
            Ok(())
        }
    }

    pub(super) fn refresh(self: &Arc<Self>, id: FiberId) {
        let epoch = {
            let w = self.lock();
            let Some(f) = w.fibers.get(&id) else {
                return;
            };
            let mut epoch = String::new();
            let mut inactive = false;
            for name in &f.inject {
                match f.pending_store.get(name) {
                    Some(impl_) => {
                        let uid = w.fibers.get(&impl_.fiber).and_then(|p| p.uid).unwrap_or(0);
                        epoch.push(':');
                        epoch.push_str(&uid.to_string());
                    }
                    None => {
                        inactive = true;
                        break;
                    }
                }
            }
            if inactive {
                Epoch::Inactive
            } else {
                Epoch::Active(epoch)
            }
        };
        self.set_epoch(id, epoch);
    }

    fn set_epoch(self: &Arc<Self>, id: FiberId, epoch: Epoch) {
        let start_load = {
            let mut w = self.lock();
            let Some(f) = w.fibers.get_mut(&id) else {
                return;
            };
            if f.epoch == epoch {
                return;
            }
            let old_inactive = f.epoch.is_inactive();
            f.epoch = epoch;
            if f.inertia.is_some() {
                return;
            }
            let start_load = !f.epoch.is_inactive() && old_inactive;
            f.inertia = Some(if start_load {
                InertiaKind::Loading
            } else {
                InertiaKind::Unloading
            });
            let _ = f.settled.send(false);
            let old_state = f.state;
            f.state = if start_load {
                FiberState::Loading
            } else {
                FiberState::Unloading
            };
            let new_state = f.state;
            Some((start_load, old_state, new_state))
        };
        let Some((start_load, old_state, new_state)) = start_load else {
            return;
        };
        self.emit_status(id, old_state, new_state);
        let rt = self.clone();
        tokio::spawn(async move {
            rt.drive(id, start_load).await;
        });
    }

    async fn drive(self: Arc<Self>, id: FiberId, mut loading: bool) {
        loop {
            if loading {
                tokio::task::yield_now().await;
                let (ctx, config, apply, old_epoch, still) = {
                    let mut w = self.lock();
                    let Some(f) = w.fibers.get_mut(&id) else {
                        return;
                    };
                    f.store = Some(f.pending_store.clone());
                    (
                        crate::Context {
                            rt: self.clone(),
                            fiber: id,
                            isolate: f.isolate.clone(),
                            intercept: f.intercept.clone(),
                            filter: None,
                        },
                        f.config.clone(),
                        f.apply.clone(),
                        f.epoch.clone(),
                        f.uid.is_some(),
                    )
                };
                let epoch_ok = {
                    let w = self.lock();
                    w.fibers
                        .get(&id)
                        .map(|f| f.epoch == old_epoch)
                        .unwrap_or(false)
                };
                if still && epoch_ok {
                    if let Some(apply) = apply {
                        match apply(ctx, config).await {
                            Ok(disp) => {
                                if let Some(d) = disp {
                                    let mut w = self.lock();
                                    if let Some(f) = w.fibers.get_mut(&id) {
                                        f.disposables.push(d);
                                    }
                                }
                            }
                            Err(e) => {
                                self.logger.named_error(&self.fiber_name(id), e.to_string());
                                let mut w = self.lock();
                                if let Some(f) = w.fibers.get_mut(&id) {
                                    f.error = Some(e.to_string());
                                    f.epoch = Epoch::Inactive;
                                }
                            }
                        }
                    }
                }
                match self.finish_inertia(id, &old_epoch, true) {
                    InertiaNext::Idle => return,
                    InertiaNext::Reload => continue,
                    InertiaNext::Unload => {
                        loading = false;
                        continue;
                    }
                }
            } else {
                let disposables = {
                    let mut w = self.lock();
                    let Some(f) = w.fibers.get_mut(&id) else {
                        return;
                    };
                    f.disposables.clear()
                };
                let logger = self.logger.clone();
                let handles: Vec<_> = disposables
                    .into_iter()
                    .map(|d| {
                        let logger = logger.clone();
                        tokio::spawn(async move {
                            if let Err(e) = d.dispose().await {
                                logger.error(e.to_string());
                            }
                        })
                    })
                    .collect();
                for h in handles {
                    let _ = h.await;
                }
                {
                    let mut w = self.lock();
                    if let Some(f) = w.fibers.get_mut(&id) {
                        f.store = None;
                    }
                }
                match self.finish_inertia(id, &Epoch::Inactive, false) {
                    InertiaNext::Idle => return,
                    InertiaNext::Unload => continue,
                    InertiaNext::Reload => {
                        loading = true;
                        continue;
                    }
                }
            }
        }
    }

    fn finish_inertia(
        self: &Arc<Self>,
        id: FiberId,
        old_epoch: &Epoch,
        from_reload: bool,
    ) -> InertiaNext {
        let outcome = {
            let mut w = self.lock();
            let Some(f) = w.fibers.get_mut(&id) else {
                return InertiaNext::Idle;
            };
            let old_state = f.state;
            let next = if from_reload {
                if f.epoch == *old_epoch {
                    f.inertia = None;
                    let _ = f.settled.send(true);
                    f.state = derived_state(f);
                    InertiaNext::Idle
                } else {
                    f.inertia = Some(InertiaKind::Unloading);
                    f.state = FiberState::Unloading;
                    let _ = f.settled.send(false);
                    InertiaNext::Unload
                }
            } else if f.epoch.is_inactive() {
                f.inertia = None;
                let _ = f.settled.send(true);
                f.state = derived_state(f);
                InertiaNext::Idle
            } else {
                f.inertia = Some(InertiaKind::Loading);
                f.state = FiberState::Loading;
                let _ = f.settled.send(false);
                InertiaNext::Reload
            };
            let new_state = f.state;
            let provided = w.provided_names(id);
            let isolate = w.fibers.get(&id).map(|f| f.isolate.clone());
            (old_state, new_state, provided, isolate, next)
        };
        let (old_state, new_state, provided, isolate, next) = outcome;
        self.emit_status(id, old_state, new_state);
        if old_state != new_state
            && (old_state == FiberState::Active || new_state == FiberState::Active)
        {
            if let Some(isolate) = isolate {
                let ctx = crate::Context {
                    rt: self.clone(),
                    fiber: id,
                    isolate,
                    intercept: Arc::new(InterceptScope::default()),
                    filter: None,
                };
                self.notify(&ctx, &provided);
            }
        }
        next
    }
}
