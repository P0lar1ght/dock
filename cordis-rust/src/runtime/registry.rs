use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::watch;

use crate::error::{Error, Result};
use crate::ids::{next_fiber, Epoch, FiberId, FiberState};
use crate::dispose::{run_steps_sync, CleanupStep, Disposable};
use crate::plugin::{Config, Plugin};

use super::world::{DisposableList, FiberData, PluginRuntime};
use super::Runtime;

impl Runtime {
    pub(crate) fn assert_can_effect(&self, id: FiberId) -> Result<()> {
        let w = self.lock();
        let Some(f) = w.fibers.get(&id) else {
            return Err(Error::InactiveEffect);
        };
        if f.uid.is_none() || f.state == FiberState::Unloading {
            return Err(Error::InactiveEffect);
        }
        Ok(())
    }

    pub(crate) fn plugin(
        self: &Arc<Self>,
        ctx: &crate::Context,
        plugin: Plugin,
        config: Config,
    ) -> Result<crate::Fiber> {
        self.assert_can_effect(ctx.fiber)?;
        let id = next_fiber();
        let (settled, _) = watch::channel(true);
        let inject_names: Vec<String> = plugin.inject.deps.keys().cloned().collect();
        let intercept = ctx.intercept.chain(&plugin.inject.deps);
        let name = if plugin.name.is_empty() || plugin.name == "apply" {
            self.fiber_name(ctx.fiber)
        } else {
            plugin.name.clone()
        };

        {
            let mut w = self.lock();
            let runtime = w.plugins.entry(plugin.id).or_insert_with(|| PluginRuntime {
                name: plugin.name.clone(),
                apply: plugin.apply.clone(),
                fibers: Vec::new(),
            });
            if runtime.name.is_empty() {
                runtime.name = plugin.name.clone();
            }
            runtime.fibers.push(id);
            let uid = w.alloc_uid();
            w.fibers.insert(
                id,
                FiberData {
                    uid: Some(uid),
                    parent: Some(ctx.fiber),
                    state: FiberState::Pending,
                    inject: inject_names.clone(),
                    config,
                    apply: Some(plugin.apply.clone()),
                    epoch: Epoch::Inactive,
                    error: None,
                    disposables: DisposableList::new(),
                    store: None,
                    pending_store: HashMap::new(),
                    runtime_id: Some(plugin.id),
                    isolate: ctx.isolate.clone(),
                    intercept,
                    inertia: None,
                    settled,
                    name,
                    owner: None,
                    update_hooks: Vec::new(),
                },
            );
        }

        let rt = self.clone();
        let owner = ctx.effect("ctx.plugin()", move |_scope| Ok(()))?;
        owner.append_steps(vec![CleanupStep::Handle(Disposable::from_async(
            move || {
                let rt = rt.clone();
                async move { rt.teardown_fiber(id).await }
            },
        ))]);
        {
            let mut w = self.lock();
            if let Some(f) = w.fibers.get_mut(&id) {
                f.owner = Some(owner.clone());
            }
        }

        let parent_unloading = {
            let mut w = self.lock();
            let parent_state = w.fibers.get(&ctx.fiber).map(|f| f.state);
            let alive = w.fibers.get(&id).and_then(|f| f.uid).is_some();
            if alive && parent_state != Some(FiberState::Unloading) {
                for name in &inject_names {
                    w.check_impl(id, name);
                }
            }
            parent_state == Some(FiberState::Unloading)
        };
        if !parent_unloading {
            self.refresh(id);
        }

        let fiber = crate::Fiber {
            rt: self.clone(),
            id,
        };
        self.emit("internal/plugin", Arc::new(fiber.clone()), None);
        Ok(fiber)
    }

    pub(crate) async fn registry_delete(self: &Arc<Self>, plugin: &Plugin) -> Result<()> {
        let fibers = {
            let mut w = self.lock();
            let Some(runtime) = w.plugins.remove(&plugin.id) else {
                return Ok(());
            };
            runtime.fibers
        };
        for id in fibers {
            let owner = {
                let w = self.lock();
                w.fibers.get(&id).and_then(|f| f.owner.clone())
            };
            if let Some(owner) = owner {
                owner.dispose().await?;
            }
        }
        Ok(())
    }

    pub(crate) fn effect(
        self: &Arc<Self>,
        ctx: &crate::Context,
        label: &str,
        body: impl FnOnce(&mut crate::EffectScope) -> Result<()>,
    ) -> Result<Disposable> {
        self.assert_can_effect(ctx.fiber)?;
        let wrapper = Disposable::with_label(label.to_string(), Vec::new());
        {
            let mut w = self.lock();
            w.fibers
                .get_mut(&ctx.fiber)
                .ok_or(Error::InactiveEffect)?
                .disposables
                .push(wrapper.clone());
        }
        let mut scope = crate::EffectScope {
            ctx: ctx.clone(),
            collected: Vec::new(),
        };
        match body(&mut scope) {
            Ok(()) => {
                for d in scope.collected {
                    {
                        let mut w = self.lock();
                        if let Some(f) = w.fibers.get_mut(&ctx.fiber) {
                            f.disposables.steal(&d);
                        }
                    }
                    if let Some(meta) = d.meta() {
                        wrapper.push_child_meta(meta);
                    }
                    wrapper.append_steps(vec![CleanupStep::Handle(d)]);
                }
                Ok(wrapper)
            }
            Err(e) => {
                {
                    let mut w = self.lock();
                    if let Some(f) = w.fibers.get_mut(&ctx.fiber) {
                        f.disposables.steal(&wrapper);
                    }
                }
                if let Some(steps) = wrapper.take_live_steps() {
                    run_steps_sync(steps);
                }
                for d in scope.collected.into_iter().rev() {
                    if let Some(steps) = d.take_live_steps() {
                        run_steps_sync(steps);
                    }
                }
                Err(e)
            }
        }
    }
}
