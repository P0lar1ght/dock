# cordis

Rust port of the Cordis plugin kernel, using the DeepSeek Harness vendored
source as the spec:

https://github.com/deepseek-ai/deepseek-harness/tree/master/vendor/cordis

This crate is the kernel only: a plugin tree hung off `Context`. It does not
include an agent loop, a loader, or a UI.

## Model

- **Context** is the application. Plugins mount into it and form a tree of fibers.
- **Services** are provided and injected **by name**. A consumer names a
  capability (`"tools"`, `"llm"`) rather than importing a concrete type.
- **`inject` is reactive.** A plugin stays `Pending` until every required
  service exists, unloads when a dependency disappears, and loads again when
  it returns. Boot order does not matter.
- **Effects are reversible.** Listeners, services, and nested plugins are
  registered as effects. Disposing a fiber unwinds them so the world matches
  a tree that never mounted that plugin.

## Live lookup — do not capture `Arc`

TypeScript Cordis resolves `ctx.greeter` through a Proxy on every read, so a
plugin never holds a stale implementation after the provider unloads.

In Rust, `get()` returning `Arc<T>` is the easy footgun: store that `Arc` in a
closure and the old service stays alive after teardown.

**Capture `Context` or `ServiceRef`, and look up at the use site.** When
`inject` unloads the fiber, those closures are dropped with the fiber's
effects.

```rust
// Hold the name, not the implementation.
let greeter = ctx.service_ref::<Greeter>("greeter");

ctx.on("tick", move |_: &()| {
    greeter.with(|g| g.hello()); // live lookup
})?;
```

`Context::with` also refuses to let an `Arc` escape the callback.

## Quick start

```rust
use cordis::{plugin, Context, Inject};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[tokio::main]
async fn main() -> cordis::Result<()> {
    let root = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let _provided = root.provide("counter", hits.clone())?;

    let greeter = plugin("greeter", Inject::from(["counter"]), {
        let hits = hits.clone();
        move |ctx, _: &()| {
            ctx.with::<AtomicUsize, _, _>("counter", |c| {
                c.fetch_add(1, Ordering::SeqCst);
            });
            let _ = &hits;
            Ok(None)
        }
    });

    let fiber = root.plugin(greeter, ())?;
    fiber.wait().await?;
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    fiber.dispose().await?;
    Ok(())
}
```

Swap the `provide` and `plugin` lines and the greeter still runs: it waits
until `counter` exists, then applies.

`Context::new` must be called from a tokio runtime.
