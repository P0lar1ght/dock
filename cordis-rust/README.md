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

## `internal/*` —— 内核自己的可观测面

插件树的变化可以订阅，不用轮询：

| 事件 | 载荷 | 何时 |
|---|---|---|
| `internal/plugin` | `Fiber` | fiber 挂上或卸掉 |
| `internal/status` | `StatusEvent` | fiber 状态跳变（Pending ↔ Active ↔ …） |
| `internal/service` | `ServiceEvent` | 某个 realm 里的具名服务出现或消失（`value: None` = 消失） |
| `internal/update` | `UpdateEvent` | fiber 配置更新。**fiber-local 的 waterfall**，跳过 `next()` 即否决重启 |
| `internal/dispatch` | `DispatchEvent` | 每一次非 `internal/` 的分发，带分发形态。**追踪用** |
| `internal/listener` | `ListenerEvent` | 监听器注册前。**bail**：给出值即否决，`ctx.on` 报 `ListenerRejected` |

两条实现约束，改这块之前要知道：

- `internal/dispatch` **只在有人监听时构造载荷**（`has_listener` 先摸一次 map），
  而且 `internal/*` 自身不追踪——否则第一条追踪就自激成无限递归。上游
  `events.ts` 是同一道闸。
- `internal/service` 发在 `notify` 里，那是 provide 与 provider 卸载的唯一汇合
  点，所以一个发射点覆盖"出现"和"消失"两种。
- `Context::set`（就地换掉已有服务的值）**不发** `internal/service`，也不触发依赖
  方重载：它走的是 `Runtime::set`，只改 `impl_.value`，绕开 `notify`。这一条是
  刻意留着的——`set` 的语义是"同一个服务，换个值"，重跑一遍 provide/dispose 会
  把正在用它的 fiber 全部重启。要让依赖方看见变化，就别用 `set`，重新 `provide`。

### 上游有而这里没有的

`internal/get` 和 `internal/set` 依赖 Proxy —— 上游 `ctx.foo` 每次读都过
`ReflectService.handler`，插件能在 `internal/get` 上拦截服务解析（换实现、加代理、
记访问）。Rust 没有 Proxy，`ctx.get::<T>(name)` 是直查，这个扩展点不存在。
同理 `mixin` / `accessor` 也没有 —— 它们是 Proxy 糖。

`serial`（顺序 await + bail 短路）没做：waterfall 覆盖了它的主要用法。

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
