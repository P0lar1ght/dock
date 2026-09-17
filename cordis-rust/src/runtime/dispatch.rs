use std::sync::{Arc, Mutex};

use crate::dispose::Disposable;
use crate::error::{AggregateError, Result};
use crate::events::{DispatchEvent, DispatchMode, EventArgs, EventOptions, Payload, SyncHandler};
use crate::fiber::StatusEvent;
use crate::ids::{next_hook, FiberId, FiberState};
use crate::logger::Logger;

use super::world::Hook;
use super::{ListenerView, Runtime};

impl Runtime {
    pub(crate) fn listen(
        self: &Arc<Self>,
        ctx: &crate::Context,
        name: &str,
        handler: SyncHandler,
        options: EventOptions,
    ) -> Result<Disposable> {
        self.assert_can_effect(ctx.fiber)?;
        let id = next_hook();
        let name = name.to_string();
        let handler = if options.once {
            let rt = self.clone();
            let name_c = name.clone();
            Arc::new(move |args: EventArgs| {
                rt.unlisten(&name_c, id);
                handler(args)
            })
        } else {
            handler
        };
        // 注册前先问一声：`internal/listener` 是 bail，任一监听者给出值即否决。
        // 上游用它让插件把某个事件名据为己有。跳过 `internal/*` 自身，否则
        // 一个 internal/listener 的监听者会在注册自己时被自己问到。
        if !name.starts_with("internal/") && self.has_listener("internal/listener") {
            let vetoed = self.bail(
                "internal/listener",
                Arc::new(crate::events::ListenerEvent {
                    name: name.to_string(),
                    prepend: options.prepend,
                    global: options.global,
                    once: options.once,
                }),
                None,
            );
            if vetoed.is_some() {
                return Err(crate::error::Error::ListenerRejected {
                    name: name.to_string(),
                });
            }
        }
        let label = format!("ctx.on({name:?})");
        let rt = self.clone();
        let ctx_c = ctx.clone();
        let fiber_local = name == "internal/update" && !options.global;
        ctx.effect(&label, move |scope| {
            {
                let mut w = rt.lock();
                if fiber_local {
                    if let Some(f) = w.fibers.get_mut(&ctx_c.fiber) {
                        if options.prepend {
                            f.update_hooks.insert(0, (id, handler.clone()));
                        } else {
                            f.update_hooks.push((id, handler.clone()));
                        }
                    }
                } else {
                    let hook = Hook {
                        id,
                        fiber: ctx_c.fiber,
                        isolate: ctx_c.isolate.clone(),
                        global: options.global,
                        handler,
                    };
                    let hooks = w.hooks.entry(name.clone()).or_default();
                    if options.prepend {
                        hooks.insert(0, hook);
                    } else {
                        hooks.push(hook);
                    }
                }
            }
            let rt2 = rt.clone();
            let name2 = name.clone();
            scope.own(Disposable::from_fn(move || {
                rt2.unlisten(&name2, id);
            }));
            Ok(())
        })
    }

    fn unlisten(&self, name: &str, id: u64) {
        let mut w = self.lock();
        if let Some(hooks) = w.hooks.get_mut(name) {
            hooks.retain(|h| h.id != id);
        }
        for f in w.fibers.values_mut() {
            f.update_hooks.retain(|(hid, _)| *hid != id);
        }
    }

    pub(super) fn emit_status(self: &Arc<Self>, id: FiberId, old: FiberState, new: FiberState) {
        if old == new {
            return;
        }
        self.emit(
            "internal/status",
            Arc::new(StatusEvent {
                fiber: crate::Fiber {
                    rt: self.clone(),
                    id,
                },
                old,
                new,
            }),
            None,
        );
    }

    pub(super) fn update_handlers(&self, id: FiberId) -> Vec<SyncHandler> {
        let w = self.lock();
        let mut out = Vec::new();
        if let Some(hooks) = w.hooks.get("internal/update") {
            out.extend(hooks.iter().filter(|h| h.global).map(|h| h.handler.clone()));
        }
        if let Some(f) = w.fibers.get(&id) {
            out.extend(f.update_hooks.iter().map(|(_, h)| h.clone()));
        }
        out
    }

    /// 这个事件名下有没有监听者。只摸一次 map，不构造载荷、不克隆 handler。
    fn has_listener(&self, name: &str) -> bool {
        self.lock().hooks.get(name).is_some_and(|h| !h.is_empty())
    }

    /// `internal/dispatch` 的记账。
    ///
    /// **没人监听时不构造载荷**：`internal/*` 自身跳过（否则第一条追踪就会自激），
    /// 有监听者才付一次 `String` + `Arc`。上游 `events.ts` 是同一道闸
    /// （`!name.startsWith('internal/') && this._hooks['internal/dispatch']?.length`）。
    fn trace_dispatch(&self, mode: DispatchMode, name: &str) {
        if name.starts_with("internal/") {
            return;
        }
        if !self.has_listener("internal/dispatch") {
            return;
        }
        self.emit(
            "internal/dispatch",
            Arc::new(DispatchEvent {
                mode,
                name: name.to_string(),
            }),
            None,
        );
    }

    pub(crate) fn emit(
        &self,
        name: &str,
        payload: Payload,
        filter: Option<&dyn Fn(&ListenerView) -> bool>,
    ) {
        self.trace_dispatch(DispatchMode::Emit, name);
        for h in self.handlers(name, filter) {
            if let Err(e) = h(EventArgs {
                payload: payload.clone(),
                next: None,
            }) {
                self.logger.error(e.to_string());
            }
        }
    }

    pub(crate) fn bail(
        &self,
        name: &str,
        payload: Payload,
        filter: Option<&dyn Fn(&ListenerView) -> bool>,
    ) -> Option<Payload> {
        self.trace_dispatch(DispatchMode::Bail, name);
        for h in self.handlers(name, filter) {
            match h(EventArgs {
                payload: payload.clone(),
                next: None,
            }) {
                Ok(Some(v)) => return Some(v),
                Ok(None) => {}
                Err(e) => self.logger.error(e.to_string()),
            }
        }
        None
    }

    pub(crate) async fn parallel(
        &self,
        name: &str,
        payload: Payload,
        filter: Option<&dyn Fn(&ListenerView) -> bool>,
    ) -> Result<(), AggregateError> {
        self.trace_dispatch(DispatchMode::Parallel, name);
        let mut errors = Vec::new();
        for h in self.handlers(name, filter) {
            if let Err(e) = h(EventArgs {
                payload: payload.clone(),
                next: None,
            }) {
                errors.push(Arc::new(e));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AggregateError { errors })
        }
    }

    pub(crate) fn waterfall<T: Clone + Send + Sync + 'static>(
        &self,
        name: &str,
        value: T,
        inner: Box<dyn FnOnce() -> T + Send>,
        filter: Option<&dyn Fn(&ListenerView) -> bool>,
    ) -> T {
        self.trace_dispatch(DispatchMode::Waterfall, name);
        self.waterfall_run(self.handlers(name, filter), value, inner)
    }

    pub(super) fn waterfall_run<T: Clone + Send + Sync + 'static>(
        &self,
        handlers: Vec<SyncHandler>,
        value: T,
        inner: Box<dyn FnOnce() -> T + Send>,
    ) -> T {
        // No listeners: run the default directly, skipping the per-level `Arc`
        // and `Mutex` the recursive chain would otherwise allocate. Same result
        // as `go(0, ..)` immediately taking `inner` when `handlers` is empty.
        if handlers.is_empty() {
            return inner();
        }
        let logger = self.logger.clone();
        type OnceBox<T> = Arc<Mutex<Option<Box<dyn FnOnce() -> T + Send>>>>;
        fn go<T: Clone + Send + Sync + 'static>(
            i: usize,
            handlers: Arc<Vec<SyncHandler>>,
            value: T,
            inner: OnceBox<T>,
            logger: Logger,
        ) -> T {
            if i >= handlers.len() {
                return inner
                    .lock()
                    .unwrap()
                    .take()
                    .map(|f| f())
                    .unwrap_or_else(|| value.clone());
            }
            let handlers2 = handlers.clone();
            let value2 = value.clone();
            let inner2 = inner.clone();
            let logger2 = logger.clone();
            let next = Arc::new(move || {
                let v = go(
                    i + 1,
                    handlers2.clone(),
                    value2.clone(),
                    inner2.clone(),
                    logger2.clone(),
                );
                Arc::new(v) as Payload
            });
            let args = EventArgs {
                payload: Arc::new(value.clone()),
                next: Some(next),
            };
            match handlers[i](args) {
                Ok(Some(p)) => p.downcast_ref::<T>().cloned().unwrap_or(value),
                Ok(None) => value,
                Err(e) => {
                    logger.error(e.to_string());
                    value
                }
            }
        }
        go(
            0,
            Arc::new(handlers),
            value,
            Arc::new(Mutex::new(Some(inner))),
            logger,
        )
    }

    fn handlers(
        &self,
        name: &str,
        filter: Option<&dyn Fn(&ListenerView) -> bool>,
    ) -> Vec<SyncHandler> {
        let w = self.lock();
        let Some(hooks) = w.hooks.get(name) else {
            return Vec::new();
        };
        hooks
            .iter()
            .filter(|h| {
                if h.global || filter.is_none() {
                    return true;
                }
                let view = ListenerView {
                    isolate: h.isolate.clone(),
                };
                filter.unwrap()(&view)
            })
            .map(|h| h.handler.clone())
            .collect()
    }
}
