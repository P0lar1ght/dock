use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cordis::{
    plugin, plugin_async, Context, Disposable, FiberState, Inject, UpdateEvent,
};

fn hits() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

#[tokio::test]
async fn restart_reruns_apply() {
    let root = Context::new();
    let n = hits();
    let fiber = root
        .plugin(
            plugin("p", Inject::new(), {
                let n = n.clone();
                move |_ctx, _: &()| {
                    n.fetch_add(1, Ordering::SeqCst);
                    Ok(None)
                }
            }),
            (),
        )
        .unwrap();
    fiber.wait().await.unwrap();
    fiber.restart().await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 2);
    assert_eq!(fiber.state(), FiberState::Active);
}

#[tokio::test]
async fn update_applies_new_config() {
    let root = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fiber = root
        .plugin(
            plugin("p", Inject::new(), {
                let seen = seen.clone();
                move |_ctx, cfg: &String| {
                    seen.lock().unwrap().push(cfg.clone());
                    Ok(None)
                }
            }),
            "hello".to_string(),
        )
        .unwrap();
    fiber.wait().await.unwrap();
    fiber.update("world".to_string()).await.unwrap();
    assert_eq!(*seen.lock().unwrap(), ["hello", "world"]);
}

#[tokio::test]
async fn update_while_pending_uses_new_config() {
    let root = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fiber = root
        .plugin(
            plugin("p", Inject::from(["foo"]), {
                let seen = seen.clone();
                move |_ctx, cfg: &String| {
                    seen.lock().unwrap().push(cfg.clone());
                    Ok(None)
                }
            }),
            "old".to_string(),
        )
        .unwrap();
    assert_eq!(fiber.state(), FiberState::Pending);
    fiber.update("new".to_string()).await.unwrap();
    let _p = root.provide("foo", 1i32).unwrap();
    fiber.wait().await.unwrap();
    assert_eq!(*seen.lock().unwrap(), ["new"]);
}

#[tokio::test]
async fn update_waterfall_can_veto_restart() {
    let root = Context::new();
    let n = hits();
    let fiber = root
        .plugin(
            plugin("p", Inject::new(), {
                let n = n.clone();
                move |ctx, _: &()| {
                    n.fetch_add(1, Ordering::SeqCst);
                    ctx.on_waterfall("internal/update", |ev: UpdateEvent, _| ev)
                        .unwrap();
                    Ok(None)
                }
            }),
            (),
        )
        .unwrap();
    fiber.wait().await.unwrap();
    fiber.update(()).await.unwrap();
    // Listener returned the event without calling next(), so restart is vetoed.
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn plugin_error_goes_failed_and_wait_errs() {
    let root = Context::new();
    let fiber = root
        .plugin(
            plugin("bad", Inject::new(), |_ctx, _: &()| {
                Err(cordis::Error::message("plugin error"))
            }),
            (),
        )
        .unwrap();
    assert!(fiber.wait().await.is_err());
    assert_eq!(fiber.state(), FiberState::Failed);
    assert!(!root.logger().errors().is_empty());
}

#[tokio::test]
async fn plugin_error_rolls_back_provide() {
    let root = Context::new();
    let fiber = root
        .plugin(
            plugin("bad", Inject::new(), |ctx, _: &()| {
                ctx.provide("foo", 1i32)?;
                Err(cordis::Error::message("after provide"))
            }),
            (),
        )
        .unwrap();
    let _ = fiber.wait().await;
    assert!(root.get::<i32>("foo").is_none());
}

#[tokio::test]
async fn dispose_error_is_logged_not_fatal() {
    let root = Context::new();
    let fiber = root
        .plugin(
            plugin("p", Inject::new(), |_ctx, _: &()| {
                Ok(Some(Disposable::from_async(|| async {
                    Err(cordis::Error::message("dispose boom"))
                })))
            }),
            (),
        )
        .unwrap();
    fiber.wait().await.unwrap();
    fiber.dispose().await.unwrap();
    assert!(root
        .logger()
        .errors()
        .iter()
        .any(|e| e.contains("dispose boom")));
}

#[tokio::test]
async fn root_dispose_restarts_and_unloads_children() {
    let root = Context::new();
    let n = hits();
    let child = root
        .plugin(
            plugin("child", Inject::new(), {
                let n = n.clone();
                move |_ctx, _: &()| {
                    Ok(Some(Disposable::from_fn({
                        let n = n.clone();
                        move || {
                            n.fetch_add(1, Ordering::SeqCst);
                        }
                    })))
                }
            }),
            (),
        )
        .unwrap();
    child.wait().await.unwrap();
    assert_eq!(root.fiber().uid(), Some(0));
    root.fiber().dispose().await.unwrap();
    assert_eq!(root.fiber().uid(), Some(0));
    assert_eq!(child.uid(), None);
    assert_eq!(n.load(Ordering::SeqCst), 1);
    root.fiber().dispose().await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unloading_rejects_new_effects() {
    let root = Context::new();
    let rejected = Arc::new(Mutex::new(false));
    let fiber = root
        .plugin(
            plugin("p", Inject::new(), {
                let rejected = rejected.clone();
                move |ctx, _: &()| {
                    let ctx = ctx.clone();
                    let rejected = rejected.clone();
                    Ok(Some(Disposable::from_fn(move || {
                        *rejected.lock().unwrap() = ctx.effect("x", |_| Ok(())).is_err();
                    })))
                }
            }),
            (),
        )
        .unwrap();
    fiber.wait().await.unwrap();
    fiber.dispose().await.unwrap();
    assert!(*rejected.lock().unwrap());
}

#[tokio::test]
async fn duplicate_provide_errors() {
    let root = Context::new();
    let _a = root.provide("foo", 1i32).unwrap();
    assert!(matches!(
        root.provide("foo", 2i32),
        Err(cordis::Error::ServiceRegistered { .. })
    ));
}

#[tokio::test]
async fn set_from_other_fiber_errors() {
    let root = Context::new();
    let _p = root.provide("foo", 1i32).unwrap();
    let fiber = root
        .plugin(
            plugin("p", Inject::new(), |ctx, _: &()| {
                assert!(matches!(
                    ctx.set("foo", 2i32),
                    Err(cordis::Error::SetFromOtherFiber { .. })
                ));
                Ok(None)
            }),
            (),
        )
        .unwrap();
    fiber.wait().await.unwrap();
}

#[tokio::test]
async fn intercept_config_is_visible_on_plugin_ctx() {
    let root = Context::new();
    let _svc = root.provide("llm", "model".to_string()).unwrap();
    let seen = Arc::new(Mutex::new(None));
    let p = plugin(
        "p",
        Inject::new().require_with("llm", 7u32),
        {
            let seen = seen.clone();
            move |ctx, _: &()| {
                *seen.lock().unwrap() = ctx.intercept_get::<u32>("llm").as_deref().copied();
                Ok(None)
            }
        },
    );
    root.plugin(p, ()).unwrap().wait().await.unwrap();
    assert_eq!(*seen.lock().unwrap(), Some(7));
}

#[tokio::test]
async fn isolate_with_shared_key() {
    let root = Context::new();
    let n = hits();
    let plugin = plugin("needs-foo", Inject::from(["foo"]), {
        let n = n.clone();
        move |_ctx, _: &()| {
            n.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    });
    let ctx1 = root.isolate("foo");
    let key = ctx1.isolate_key("foo").unwrap();
    let ctx2 = root.isolate_with("foo", key);
    let f1 = ctx1.plugin(plugin.clone(), ()).unwrap();
    let f2 = ctx2.plugin(plugin, ()).unwrap();
    let _d = ctx1.provide("foo", 1i32).unwrap();
    f1.wait().await.unwrap();
    f2.wait().await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 2);
    assert_eq!(ctx2.get::<i32>("foo").as_deref(), Some(&1));
}

#[tokio::test]
async fn registry_delete_restores_hooks() {
    let root = Context::new();
    let p = plugin("p", Inject::new(), |ctx, _: &()| {
        ctx.on("e", |_: &()| {})?;
        Ok(None)
    });
    let before = root.hook_snapshot();
    let fiber = root.plugin(p.clone(), ()).unwrap();
    fiber.wait().await.unwrap();
    assert_ne!(before, root.hook_snapshot());
    root.registry_delete(&p).await.unwrap();
    assert_eq!(before, root.hook_snapshot());
}

#[tokio::test]
async fn concurrent_dispose_runs_cleanup_once() {
    let root = Context::new();
    let n = hits();
    let d = root
        .effect("anonymous", {
            let n = n.clone();
            move |scope| {
                scope.own(Disposable::from_fn({
                    let n = n.clone();
                    move || {
                        n.fetch_add(1, Ordering::SeqCst);
                    }
                }));
                Ok(())
            }
        })
        .unwrap();
    let (a, b) = tokio::join!(d.dispose(), d.dispose());
    a.unwrap();
    b.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn inertia_lock_keeps_loading_until_apply_finishes() {
    // Keep a non-timer waiter so paused time does not auto-advance.
    let _hold = tokio::spawn(std::future::pending::<()>());
    tokio::time::pause();
    let root = Context::new();
    let provided = root.provide("foo", 1i32).unwrap();
    let fiber = root
        .plugin(
            plugin_async("slow", Inject::from(["foo"]), |_ctx, _: &()| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(Some(Disposable::from_async(|| async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    Ok(())
                })))
            }),
            (),
        )
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(400)).await;
    tokio::task::yield_now().await;
    assert_eq!(fiber.state(), FiberState::Loading);

    provided.dispose().await.unwrap();
    tokio::time::advance(Duration::from_millis(400)).await;
    tokio::task::yield_now().await;
    assert_eq!(fiber.state(), FiberState::Loading);

    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(fiber.state(), FiberState::Unloading);

    let _p = root.provide("foo", 1i32).unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    fiber.wait().await.unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
}

#[tokio::test]
async fn inertia_lock_same_provider_uid_keeps_load() {
    let _hold = tokio::spawn(std::future::pending::<()>());
    tokio::time::pause();
    let root = Context::new();
    let provided = root.provide("foo", 1i32).unwrap();
    let fiber = root
        .plugin(
            plugin_async("slow", Inject::from(["foo"]), |_ctx, _: &()| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(Some(Disposable::from_async(|| async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    Ok(())
                })))
            }),
            (),
        )
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(400)).await;
    tokio::task::yield_now().await;
    assert_eq!(fiber.state(), FiberState::Loading);

    provided.dispose().await.unwrap();
    tokio::time::advance(Duration::from_millis(400)).await;
    tokio::task::yield_now().await;
    let _p = root.provide("foo", 2i32).unwrap();
    tokio::time::advance(Duration::from_millis(400)).await;
    fiber.wait().await.unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
}

#[tokio::test]
async fn two_deps_wait_for_both() {
    let root = Context::new();
    let n = hits();
    let fiber = root
        .inject(["foo", "bar"], {
            let n = n.clone();
            move |_ctx| {
                n.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            }
        })
        .unwrap();
    let _a = root.provide("foo", 1i32).unwrap();
    tokio::task::yield_now().await;
    assert_eq!(n.load(Ordering::SeqCst), 0);
    let _b = root.provide("bar", 1i32).unwrap();
    fiber.wait().await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 1);
}
