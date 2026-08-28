//! Rust port of the Cordis plugin kernel vendored by DeepSeek Harness
//! (`vendor/cordis` in [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)).
//!
//! An application is a plugin tree hung off [`Context`]. Services are provided
//! and injected **by name**. Side effects are reversible: unloading a plugin
//! drops its effects so the world matches a tree that never mounted it.
//! [`Context::inject`] mounts and unmounts from **whether dependencies exist**,
//! not from boot order.
//!
//! # Live lookup (do not capture `Arc`)
//!
//! TypeScript Cordis resolves `ctx.greeter` through a Proxy on every read.
//! `get()` returning `Arc<T>` is the easy Rust footgun: a plugin that stores
//! that `Arc` keeps the old service alive after the provider unloads.
//!
//! Capture [`Context`] or [`ServiceRef`] in closures, and call [`Context::get`]
//! / [`Context::with`] at the use site. When inject unloads the fiber, those
//! closures are dropped with the fiber's effects.

mod context;
mod dispose;
mod error;
mod events;
mod fiber;
mod ids;
mod logger;
mod plugin;
mod runtime;
mod scope;

pub use context::{Context, EffectScope, ServiceRef};
pub use dispose::{Disposable, EffectMeta};
pub use error::{AggregateError, Error, Result};
pub use events::{is_bailed, EventArgs, EventOptions, UpdateEvent};
pub use fiber::{Fiber, StatusEvent};
pub use ids::{FiberId, FiberState, IsolateKey, PluginId};
pub use logger::{LogLevel, Logger, Message};
pub use plugin::{plugin, plugin_async, service, Inject, Plugin};
pub use runtime::ListenerView;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
