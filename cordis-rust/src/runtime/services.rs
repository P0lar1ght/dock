use std::any::Any;
use std::sync::Arc;

use crate::dispose::Disposable;
use crate::error::{Error, Result};
use crate::ids::{FiberId, FiberState};

use super::world::Impl;
use super::Runtime;

impl Runtime {
    pub(super) fn notify(self: &Arc<Self>, ctx: &crate::Context, names: &[String]) -> Vec<FiberId> {
        if names.is_empty() {
            return Vec::new();
        }
        // 与上游 `reflect.ts` 的 `internal/service` 同位：provide 和 provider 卸载
        // 都汇到这里，所以这一个发射点覆盖了"出现"和"消失"两种。查值用当前 ctx
        // 的 isolate realm——同名服务在不同 realm 下是不同实现。
        for name in names {
            let value = self.get(ctx, name, false);
            self.emit(
                "internal/service",
                Arc::new(crate::events::ServiceEvent {
                    name: name.clone(),
                    value,
                }),
                None,
            );
        }
        let plugin_fibers = {
            let w = self.lock();
            w.plugins
                .values()
                .flat_map(|r| r.fibers.iter().copied())
                .collect::<Vec<_>>()
        };
        let mut fibers = Vec::new();
        for fid in plugin_fibers {
            let hit = {
                let mut w = self.lock();
                let Some(f) = w.fibers.get(&fid) else {
                    continue;
                };
                let isolate = f.isolate.clone();
                let inject = f.inject.clone();
                let mut hit = false;
                for name in names {
                    if !inject.iter().any(|n| n == name) {
                        continue;
                    }
                    if !w.same_realm(&isolate, &ctx.isolate, name) {
                        continue;
                    }
                    hit = true;
                    w.check_impl(fid, name);
                }
                hit
            };
            if !hit {
                continue;
            }
            self.refresh(fid);
            fibers.push(fid);
        }
        fibers
    }

    pub(crate) fn provide(
        self: &Arc<Self>,
        ctx: &crate::Context,
        name: &str,
        value: Arc<dyn Any + Send + Sync>,
    ) -> Result<Disposable> {
        let name = name.to_string();
        let rt = self.clone();
        let ctx_c = ctx.clone();
        ctx.effect(&format!("ctx.provide({name:?})"), move |scope| {
            let ctx = ctx_c.clone();
            {
                let mut w = rt.lock();
                let key = w.provide_key(&ctx.isolate, &name);
                if let Some(existing) = w.store.get(&key) {
                    let at = w
                        .fibers
                        .get(&existing.fiber)
                        .map(|f| f.name.clone())
                        .unwrap_or_default();
                    return Err(Error::ServiceRegistered {
                        name: name.clone(),
                        at,
                    });
                }
                let impl_ = Impl {
                    name: name.clone(),
                    fiber: ctx.fiber,
                    value,
                };
                w.store.insert(key, impl_.clone());
                if let Some(f) = w.fibers.get_mut(&ctx.fiber) {
                    if let Some(store) = &mut f.store {
                        store.insert(name.clone(), impl_);
                    }
                }
                let active = w
                    .fibers
                    .get(&ctx.fiber)
                    .map(|f| f.state == FiberState::Active)
                    .unwrap_or(false);
                drop(w);
                if active {
                    rt.notify(&ctx, std::slice::from_ref(&name));
                }
            }
            let rt2 = rt.clone();
            let ctx2 = ctx.clone();
            let name2 = name.clone();
            scope.own(Disposable::from_async(move || async move {
                {
                    let mut w = rt2.lock();
                    if let Some(key) = w.resolve_key(&ctx2.isolate, &name2) {
                        w.store.remove(&key);
                    }
                }
                rt2.notify(&ctx2, std::slice::from_ref(&name2));
                let mut w = rt2.lock();
                if let Some(f) = w.fibers.get_mut(&ctx2.fiber) {
                    if let Some(store) = &mut f.store {
                        store.remove(&name2);
                    }
                }
                Ok(())
            }));
            Ok(())
        })
    }

    pub(crate) fn get(
        &self,
        ctx: &crate::Context,
        name: &str,
        strict: bool,
    ) -> Option<Arc<dyn Any + Send + Sync>> {
        self.lock()
            .get_impl(&ctx.isolate, name, strict)
            .map(|i| i.value.clone())
    }

    pub(crate) fn require_status(&self, ctx: &crate::Context, name: &str) -> Result<()> {
        let w = self.lock();
        let Some(f) = w.fibers.get(&ctx.fiber) else {
            return Err(Error::InactiveService { name: name.into() });
        };
        if f.uid.is_none() && f.inject.iter().any(|n| n == name) {
            return Err(Error::InactiveService { name: name.into() });
        }
        Ok(())
    }

    pub(crate) fn set(
        &self,
        ctx: &crate::Context,
        name: &str,
        value: Arc<dyn Any + Send + Sync>,
    ) -> Result<()> {
        let mut w = self.lock();
        let key = w
            .resolve_key(&ctx.isolate, name)
            .ok_or_else(|| Error::CannotSet { name: name.into() })?;
        let impl_ = w
            .store
            .get_mut(&key)
            .ok_or_else(|| Error::CannotSet { name: name.into() })?;
        if impl_.fiber != ctx.fiber {
            return Err(Error::SetFromOtherFiber { name: name.into() });
        }
        impl_.value = value;
        Ok(())
    }
}
