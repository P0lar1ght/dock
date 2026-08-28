use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cordis::{plugin, plugin_async, service, Context, Disposable, FiberState, Inject, ServiceRef};

fn hits() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

#[tokio::test]
async fn apply_functional_plugin() {
    let root = Context::new();
    let called = hits();
    let p = plugin("p", Inject::new(), {
        let called = called.clone();
        move |_ctx, cfg: &String| {
            assert_eq!(cfg, "hi");
            called.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    });
    let fiber = root.plugin(p, "hi".to_string()).unwrap();
    fiber.wait().await.unwrap();
    assert_eq!(called.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nested_plugins_dispose_restores_hooks() {
    let root = Context::new();
    let n = hits();
    let inner = plugin("inner", Inject::new(), {
        let n = n.clone();
        move |ctx, _: &()| {
            let n = n.clone();
            ctx.on("custom-event", move |_: &()| {
                n.fetch_add(1, Ordering::SeqCst);
            })?;
            Ok(None)
        }
    });
    let outer = plugin_async("outer", Inject::new(), {
        let inner = inner.clone();
        let n = n.clone();
        move |ctx, _: &()| {
            let ctx = ctx.clone();
            let inner = inner.clone();
            let n = n.clone();
            async move {
                ctx.on("custom-event", move |_: &()| {
                    n.fetch_add(1, Ordering::SeqCst);
                })?;
                ctx.plugin(inner, ())?.wait().await?;
                Ok(None)
            }
        }
    });
    root.on("custom-event", {
        let n = n.clone();
        move |_: &()| {
            n.fetch_add(1, Ordering::SeqCst);
        }
    })
    .unwrap();
    let before = root.hook_snapshot();
    let fiber = root.plugin(outer, ()).unwrap();
    fiber.wait().await.unwrap();
    root.emit("custom-event", ());
    assert_eq!(n.load(Ordering::SeqCst), 3);
    fiber.dispose().await.unwrap();
    n.store(0, Ordering::SeqCst);
    root.emit("custom-event", ());
    assert_eq!(n.load(Ordering::SeqCst), 1);
    assert_eq!(before, root.hook_snapshot());
}

#[tokio::test]
async fn inject_waits_for_service() {
    let root = Context::new();
    let called = hits();
    let consumer = root
        .inject(["foo"], {
            let called = called.clone();
            move |ctx| {
                assert!(ctx.get::<i32>("foo").is_some());
                called.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            }
        })
        .unwrap();
    assert_eq!(called.load(Ordering::SeqCst), 0);
    assert_eq!(consumer.state(), FiberState::Pending);

    let _p = root.provide("foo", 1i32).unwrap();
    consumer.wait().await.unwrap();
    assert_eq!(called.load(Ordering::SeqCst), 1);
    assert_eq!(consumer.state(), FiberState::Active);
}

#[tokio::test]
async fn inject_unloads_when_provider_goes_and_reloads() {
    let root = Context::new();
    let seq = Arc::new(Mutex::new(Vec::new()));
    let consumer = root
        .inject(["foo"], {
            let seq = seq.clone();
            move |_ctx| {
                seq.lock().unwrap().push("apply");
                Ok(Some(Disposable::from_fn({
                    let seq = seq.clone();
                    move || {
                        seq.lock().unwrap().push("dispose");
                    }
                })))
            }
        })
        .unwrap();

    let provided = root.provide("foo", 1i32).unwrap();
    consumer.wait().await.unwrap();
    assert_eq!(*seq.lock().unwrap(), ["apply"]);

    provided.dispose().await.unwrap();
    consumer.wait().await.unwrap();
    assert_eq!(consumer.state(), FiberState::Pending);
    assert_eq!(*seq.lock().unwrap(), ["apply", "dispose"]);

    let _provided = root.provide("foo", 2i32).unwrap();
    consumer.wait().await.unwrap();
    assert_eq!(consumer.state(), FiberState::Active);
    assert_eq!(*seq.lock().unwrap(), ["apply", "dispose", "apply"]);
}

#[tokio::test]
async fn service_ref_is_live_lookup_not_an_arc_capture() {
    let root = Context::new();
    let greeter: ServiceRef<String> = root.service_ref("greeter");
    assert!(greeter.get().is_none());

    let provided = root.provide("greeter", "hello".to_string()).unwrap();
    assert_eq!(greeter.with(|s| s.clone()).as_deref(), Some("hello"));

    provided.dispose().await.unwrap();
    // Live lookup sees the provider is gone. A captured Arc would still be Some.
    assert!(greeter.get().is_none());
}

#[tokio::test]
async fn closures_must_capture_context_not_arc() {
    let root = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _provided = root.provide("greeter", "v1".to_string()).unwrap();

    let fiber = root
        .inject(["greeter"], {
            let seen = seen.clone();
            move |ctx| {
                let greeter = ctx.service_ref::<String>("greeter");
                let seen = seen.clone();
                ctx.on("tick", move |_: &()| {
                    let label = greeter.with(|s| s.clone()).unwrap_or_else(|| "gone".into());
                    seen.lock().unwrap().push(label);
                })?;
                Ok(None)
            }
        })
        .unwrap();
    fiber.wait().await.unwrap();
    root.emit("tick", ());
    assert_eq!(*seen.lock().unwrap(), ["v1"]);

    fiber.dispose().await.unwrap();
    root.emit("tick", ());
    // The listener was an effect on the inject fiber; unload dropped it.
    assert_eq!(*seen.lock().unwrap(), ["v1"]);
}

#[tokio::test]
async fn effects_run_cleanup_in_reverse() {
    let root = Context::new();
    let seq = Arc::new(Mutex::new(Vec::new()));
    let d = root
        .effect("anonymous", {
            let seq = seq.clone();
            move |scope| {
                scope.own(Disposable::from_fn({
                    let seq = seq.clone();
                    move || seq.lock().unwrap().push(1)
                }));
                scope.own(Disposable::from_fn({
                    let seq = seq.clone();
                    move || seq.lock().unwrap().push(2)
                }));
                scope.own(Disposable::from_fn({
                    let seq = seq.clone();
                    move || seq.lock().unwrap().push(3)
                }));
                Ok(())
            }
        })
        .unwrap();
    assert!(seq.lock().unwrap().is_empty());
    d.dispose().await.unwrap();
    assert_eq!(*seq.lock().unwrap(), [3, 2, 1]);
    d.dispose().await.unwrap();
    assert_eq!(*seq.lock().unwrap(), [3, 2, 1]);
}

#[tokio::test]
async fn yield_error_rolls_back_collected() {
    let root = Context::new();
    let seq = Arc::new(Mutex::new(Vec::new()));
    let err = root.effect("anonymous", {
        let seq = seq.clone();
        move |scope| {
            scope.own(Disposable::from_fn({
                let seq = seq.clone();
                move || seq.lock().unwrap().push(1)
            }));
            Err(cordis::Error::message("test"))
        }
    });
    assert!(err.is_err());
    assert_eq!(*seq.lock().unwrap(), [1]);
}

#[tokio::test]
async fn events_on_once_emit() {
    let root = Context::new();
    let n = hits();
    let d = root
        .on("e", {
            let n = n.clone();
            move |_: &()| {
                n.fetch_add(1, Ordering::SeqCst);
            }
        })
        .unwrap();
    root.emit("e", ());
    root.emit("e", ());
    assert_eq!(n.load(Ordering::SeqCst), 2);
    d.dispose().await.unwrap();
    root.emit("e", ());
    assert_eq!(n.load(Ordering::SeqCst), 2);

    let n2 = hits();
    root.once("e", {
        let n2 = n2.clone();
        move |_: &()| {
            n2.fetch_add(1, Ordering::SeqCst);
        }
    })
    .unwrap();
    root.emit("e", ());
    root.emit("e", ());
    assert_eq!(n2.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn waterfall_wraps_next() {
    let root = Context::new();
    root.on_waterfall("sum", |v: i32, args| v + args.next::<i32>().unwrap())
        .unwrap();
    root.on_waterfall("sum", |v: i32, args| v + args.next::<i32>().unwrap())
        .unwrap();
    assert_eq!(root.waterfall("sum", 1, || 2), 4);

    root.on_waterfall("sum", |v: i32, _| v).unwrap();
    root.on_waterfall("sum", |v: i32, args| v + args.next::<i32>().unwrap())
        .unwrap();
    assert_eq!(root.waterfall("sum", 1, || 2), 3);
}

#[tokio::test]
async fn isolate_separates_service_realms() {
    let root = Context::new();
    let n = hits();
    let plugin = plugin("needs-foo", Inject::from(["foo"]), {
        let n = n.clone();
        move |_ctx, _: &()| {
            n.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    });
    let f0 = root.plugin(plugin.clone(), ()).unwrap();
    let ctx1 = root.isolate("foo");
    let f1 = ctx1.plugin(plugin.clone(), ()).unwrap();
    f0.wait().await.unwrap();
    f1.wait().await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 0);

    let _d0 = root.provide("foo", 100i32).unwrap();
    f0.wait().await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 1);
    assert_eq!(root.get::<i32>("foo").as_deref(), Some(&100));
    assert!(ctx1.get::<i32>("foo").is_none());

    let _d1 = ctx1.provide("foo", 200i32).unwrap();
    f1.wait().await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 2);
    assert_eq!(root.get::<i32>("foo").as_deref(), Some(&100));
    assert_eq!(ctx1.get::<i32>("foo").as_deref(), Some(&200));
}

#[tokio::test]
async fn service_helper_and_pending_init() {
    let root = Context::new();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let rx = Arc::new(Mutex::new(Some(rx)));
    let foo = plugin_async("foo", Inject::new(), {
        let rx = rx.clone();
        move |ctx, _: &()| {
            let ctx = ctx.clone();
            let rx = rx.clone();
            async move {
                ctx.provide("foo", 7i32)?;
                let wait = {
                    let mut slot = rx.lock().unwrap();
                    slot.take()
                };
                if let Some(wait) = wait {
                    let _ = wait.await;
                }
                Ok(None)
            }
        }
    });
    let called = hits();
    let consumer = root
        .inject(["foo"], {
            let called = called.clone();
            move |_ctx| {
                called.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            }
        })
        .unwrap();
    let provider = root.plugin(foo, ()).unwrap();
    tokio::task::yield_now().await;
    assert_eq!(called.load(Ordering::SeqCst), 0);
    let _ = tx.send(());
    provider.wait().await.unwrap();
    consumer.wait().await.unwrap();
    assert_eq!(called.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn inactive_context_rejects_new_effects() {
    let root = Context::new();
    let fiber = root
        .plugin(plugin("p", Inject::new(), |_ctx, _: &()| Ok(None)), ())
        .unwrap();
    fiber.wait().await.unwrap();
    fiber.dispose().await.unwrap();
    let ctx = fiber.ctx();
    assert!(ctx
        .plugin(plugin("q", Inject::new(), |_ctx, _: &()| Ok(None)), ())
        .is_err());
    assert!(ctx.effect("x", |_| Ok(())).is_err());
}

#[tokio::test]
async fn require_after_dispose_is_inactive() {
    let root = Context::new();
    let _p = root.provide("foo", 1i32).unwrap();
    let fiber = root.inject(["foo"], |_ctx| Ok(None)).unwrap();
    fiber.wait().await.unwrap();
    assert!(fiber.ctx().require::<i32>("foo").is_ok());
    fiber.dispose().await.unwrap();
    assert!(matches!(
        fiber.ctx().require::<i32>("foo"),
        Err(cordis::Error::InactiveService { .. })
    ));
}

#[tokio::test]
async fn named_service_plugin() {
    let root = Context::new();
    let p = service("counter", Inject::new(), |_ctx| Ok(AtomicUsize::new(0)));
    root.plugin(p, ()).unwrap().wait().await.unwrap();
    root.with::<AtomicUsize, _, _>("counter", |c| c.fetch_add(1, Ordering::SeqCst));
    assert_eq!(
        root.get::<AtomicUsize>("counter")
            .unwrap()
            .load(Ordering::SeqCst),
        1
    );
}
