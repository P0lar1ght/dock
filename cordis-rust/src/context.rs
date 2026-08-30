use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::dispose::Disposable;
use crate::error::{AggregateError, Error, Result};
use crate::events::{EventArgs, EventOptions, Payload, SyncHandler};
use crate::ids::{next_isolate, FiberId, IsolateKey};
use crate::logger::Logger;
use crate::plugin::{Config, Inject, Plugin};
use crate::runtime::{ListenerView, Runtime};
use crate::scope::{InterceptScope, IsolateScope};
use crate::Fiber;

/// Root or child dependency container. Cheap to clone: clones share the
/// application, but carry their own isolate/intercept/fiber identity.
///
/// Service reads are always a live lookup (the Rust stand-in for Cordis's
/// `ctx.greeter` Proxy). Do not capture an `Arc<T>` from [`Context::get`] into
/// a long-lived closure — capture `Context` or [`ServiceRef`] and look up
/// again at the use site. When `inject` unloads the fiber, those closures are
/// dropped with the fiber's effects.
#[derive(Clone)]
pub struct Context {
    pub(crate) rt: Arc<Runtime>,
    pub(crate) fiber: FiberId,
    pub(crate) isolate: Arc<IsolateScope>,
    pub(crate) intercept: Arc<InterceptScope>,
    #[allow(dead_code)]
    pub(crate) filter: Option<crate::runtime::FilterFn>,
}

impl Context {
    /// Create the root context. Must be called from a tokio runtime.
    pub fn new() -> Self {
        let (rt, fiber, isolate, intercept) = Runtime::new();
        Self {
            rt,
            fiber,
            isolate,
            intercept,
            filter: None,
        }
    }

    pub fn root(&self) -> Self {
        Self {
            rt: self.rt.clone(),
            fiber: FiberId(0),
            isolate: self.rt.root_isolate(),
            intercept: Arc::new(InterceptScope::default()),
            filter: None,
        }
    }

    pub fn fiber(&self) -> Fiber {
        Fiber {
            rt: self.rt.clone(),
            id: self.fiber,
        }
    }

    pub fn logger(&self) -> Logger {
        self.rt.logger.clone()
    }

    pub fn isolate(&self, name: &str) -> Self {
        self.isolate_with(name, next_isolate())
    }

    pub fn isolate_with(&self, name: &str, key: IsolateKey) -> Self {
        let mut child = self.clone();
        child.isolate = self.isolate.child(name, key);
        child
    }

    pub fn intercept<T: Send + Sync + 'static>(&self, name: &str, config: T) -> Self {
        let mut child = self.clone();
        child.intercept = self.intercept.child(name, Arc::new(config));
        child
    }

    pub fn isolate_key(&self, name: &str) -> Option<IsolateKey> {
        self.rt.isolate_key(self, name)
    }

    /// Read intercept config for `name` on this context (plugin `Inject::require_with`).
    pub fn intercept_get<T: Send + Sync + 'static>(&self, name: &str) -> Option<Arc<T>> {
        self.intercept
            .get(name)
            .and_then(|v| v.downcast::<T>().ok())
    }

    /// Mount a plugin. The returned fiber settles with [`Fiber::wait`].
    pub fn plugin<C: Send + Sync + 'static>(&self, plugin: Plugin, config: C) -> Result<Fiber> {
        self.rt.plugin(self, plugin, Arc::new(config) as Config)
    }

    /// Run `apply` once every named dependency exists. Unload/reload follows
    /// the dependencies, not boot order.
    pub fn inject<F>(&self, deps: impl Into<Inject>, apply: F) -> Result<Fiber>
    where
        F: Fn(&Context) -> Result<Option<Disposable>> + Send + Sync + 'static,
    {
        let plugin = crate::plugin(apply_name(&apply), deps, move |ctx, _: &()| apply(ctx));
        self.plugin(plugin, ())
    }

    /// Register a service owned by the current fiber. Unregistering (or
    /// unloading the fiber) wakes dependents so they dispose their effects.
    pub fn provide<T: Send + Sync + 'static>(&self, name: &str, value: T) -> Result<Disposable> {
        self.rt.provide(self, name, Arc::new(value))
    }

    /// Live lookup. The `Arc` is only for this call — do not store it.
    /// Prefer [`Context::with`] or [`ServiceRef`] when the handle must be kept.
    pub fn get<T: Send + Sync + 'static>(&self, name: &str) -> Option<Arc<T>> {
        self.rt
            .get(self, name, true)
            .and_then(|v| v.downcast::<T>().ok())
    }

    /// Live lookup that cannot leak an `Arc` out of the callback.
    pub fn with<T, R, F>(&self, name: &str, f: F) -> Option<R>
    where
        T: Send + Sync + 'static,
        F: FnOnce(&T) -> R,
    {
        let value = self.get::<T>(name)?;
        Some(f(&value))
    }

    /// Like JS `ctx.greeter`: errors if missing. After this fiber is disposed
    /// while still injecting `name`, returns [`Error::InactiveService`].
    pub fn require<T: Send + Sync + 'static>(&self, name: &str) -> Result<Arc<T>> {
        self.rt.require_status(self, name)?;
        self.get(name)
            .ok_or_else(|| Error::CannotGet { name: name.into() })
    }

    pub fn set<T: Send + Sync + 'static>(&self, name: &str, value: T) -> Result<()> {
        self.rt.set(self, name, Arc::new(value))
    }

    /// A holdable name handle. Clone it into closures; it does not keep the
    /// implementation alive. Each [`ServiceRef::get`] / [`ServiceRef::with`]
    /// is a live lookup.
    pub fn service_ref<T: Send + Sync + 'static>(&self, name: impl Into<String>) -> ServiceRef<T> {
        ServiceRef {
            ctx: self.clone(),
            name: name.into(),
            _ty: PhantomData,
        }
    }

    pub fn effect<F>(&self, label: &str, body: F) -> Result<Disposable>
    where
        F: FnOnce(&mut EffectScope) -> Result<()>,
    {
        self.rt.effect(self, label, body)
    }

    pub fn on<T, F>(&self, name: &str, handler: F) -> Result<Disposable>
    where
        T: Send + Sync + 'static,
        F: Fn(&T) + Send + Sync + 'static,
    {
        self.on_with(name, EventOptions::default(), handler)
    }

    pub fn on_with<T, F>(&self, name: &str, options: EventOptions, handler: F) -> Result<Disposable>
    where
        T: Send + Sync + 'static,
        F: Fn(&T) + Send + Sync + 'static,
    {
        let handler: SyncHandler = Arc::new(move |args: EventArgs| {
            if let Some(v) = args.get::<T>() {
                handler(v);
            }
            Ok(None)
        });
        self.rt.listen(self, name, handler, options)
    }

    pub fn once<T, F>(&self, name: &str, handler: F) -> Result<Disposable>
    where
        T: Send + Sync + 'static,
        F: Fn(&T) + Send + Sync + 'static,
    {
        self.on_with(
            name,
            EventOptions {
                once: true,
                ..EventOptions::default()
            },
            handler,
        )
    }

    pub fn on_bail<T, R, F>(&self, name: &str, handler: F) -> Result<Disposable>
    where
        T: Send + Sync + 'static,
        R: Send + Sync + 'static,
        F: Fn(&T) -> Option<R> + Send + Sync + 'static,
    {
        let handler: SyncHandler = Arc::new(move |args: EventArgs| {
            Ok(args
                .get::<T>()
                .and_then(|t| handler(t))
                .map(|v| Arc::new(v) as Payload))
        });
        self.rt.listen(self, name, handler, EventOptions::default())
    }

    pub fn on_waterfall<T, F>(&self, name: &str, handler: F) -> Result<Disposable>
    where
        T: Clone + Send + Sync + 'static,
        F: Fn(T, &EventArgs) -> T + Send + Sync + 'static,
    {
        let handler: SyncHandler = Arc::new(move |args: EventArgs| {
            let Some(v) = args.get::<T>().cloned() else {
                return Ok(None);
            };
            Ok(Some(Arc::new(handler(v, &args)) as Payload))
        });
        self.rt.listen(self, name, handler, EventOptions::default())
    }

    pub fn emit<T: Send + Sync + 'static>(&self, name: &str, payload: T) {
        self.rt.emit(name, Arc::new(payload), None);
    }

    pub fn emit_filtered<T, F>(&self, name: &str, payload: T, filter: F)
    where
        T: Send + Sync + 'static,
        F: Fn(&ListenerView) -> bool,
    {
        self.rt.emit(name, Arc::new(payload), Some(&filter));
    }

    pub fn bail<T, R>(&self, name: &str, payload: T) -> Option<R>
    where
        T: Send + Sync + 'static,
        R: Send + Sync + Clone + 'static,
    {
        self.rt
            .bail(name, Arc::new(payload), None)
            .and_then(|v| v.downcast_ref::<R>().cloned())
    }

    pub async fn parallel<T: Send + Sync + 'static>(
        &self,
        name: &str,
        payload: T,
    ) -> Result<(), AggregateError> {
        self.rt.parallel(name, Arc::new(payload), None).await
    }

    pub fn waterfall<T, F>(&self, name: &str, value: T, inner: F) -> T
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        self.rt.waterfall(name, value, Box::new(inner), None)
    }

    pub fn registry_size(&self) -> usize {
        self.rt.registry_size()
    }

    pub async fn registry_delete(&self, plugin: &Plugin) -> Result<()> {
        self.rt.registry_delete(plugin).await
    }

    pub fn hook_snapshot(&self) -> std::collections::HashMap<String, usize> {
        self.rt.hook_snapshot()
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Context <{}>", self.rt.fiber_name(self.fiber))
    }
}

/// Yielded/owned disposer collector, the Rust equivalent of a generator effect.
pub struct EffectScope {
    pub(crate) ctx: Context,
    pub(crate) collected: Vec<Disposable>,
}

impl EffectScope {
    pub fn ctx(&self) -> Context {
        self.ctx.clone()
    }

    /// Take ownership of a disposer (like `yield dispose` in Cordis). Nested
    /// `ctx.on` / `ctx.effect` handles must be `own`ed to nest in this effect.
    pub fn own(&mut self, d: Disposable) {
        self.collected.push(d);
    }
}

/// Named live handle for a service. Stores the name, not the implementation.
///
/// This is the Rust counterpart of reading `ctx.greeter` through the Proxy:
/// each access looks up the current provider. Holding a `ServiceRef` does not
/// keep a disposed provider alive.
#[derive(Clone)]
pub struct ServiceRef<T> {
    ctx: Context,
    name: String,
    _ty: PhantomData<fn() -> T>,
}

impl<T: Send + Sync + 'static> ServiceRef<T> {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn get(&self) -> Option<Arc<T>> {
        self.ctx.get::<T>(&self.name)
    }

    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.ctx.with::<T, R, _>(&self.name, f)
    }

    pub fn require(&self) -> Result<Arc<T>> {
        self.ctx.require::<T>(&self.name)
    }
}

fn apply_name<F>(_: &F) -> String {
    std::any::type_name::<F>()
        .rsplit("::")
        .next()
        .unwrap_or("inject")
        .to_string()
}
