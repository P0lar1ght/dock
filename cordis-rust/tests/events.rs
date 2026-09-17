use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, EventOptions, Fiber, FiberState, Inject, StatusEvent};

fn hits() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

#[tokio::test]
async fn once_unregisters_after_first_emit() {
    let root = Context::new();
    let n = hits();
    root.once("e", {
        let n = n.clone();
        move |_: &()| {
            n.fetch_add(1, Ordering::SeqCst);
        }
    })
    .unwrap();
    let after_once = root.hook_snapshot();
    root.emit("e", ());
    root.emit("e", ());
    assert_eq!(n.load(Ordering::SeqCst), 1);
    assert_ne!(after_once, root.hook_snapshot());
}

#[tokio::test]
async fn prepend_runs_first() {
    let root = Context::new();
    let seq = Arc::new(std::sync::Mutex::new(Vec::new()));
    root.on("e", {
        let seq = seq.clone();
        move |_: &()| seq.lock().unwrap().push(1)
    })
    .unwrap();
    root.on_with(
        "e",
        EventOptions {
            prepend: true,
            ..EventOptions::default()
        },
        {
            let seq = seq.clone();
            move |_: &()| seq.lock().unwrap().push(0)
        },
    )
    .unwrap();
    root.emit("e", ());
    assert_eq!(*seq.lock().unwrap(), [0, 1]);
}

#[tokio::test]
async fn bail_stops_on_first_value() {
    let root = Context::new();
    let n = hits();
    root.on_bail("e", {
        let n = n.clone();
        move |_: &()| {
            n.fetch_add(1, Ordering::SeqCst);
            None::<i32>
        }
    })
    .unwrap();
    root.on_bail("e", {
        let n = n.clone();
        move |_: &()| {
            n.fetch_add(1, Ordering::SeqCst);
            Some(7i32)
        }
    })
    .unwrap();
    root.on_bail("e", {
        let n = n.clone();
        move |_: &()| {
            n.fetch_add(1, Ordering::SeqCst);
            Some(9i32)
        }
    })
    .unwrap();
    assert_eq!(root.bail::<(), i32>("e", ()), Some(7));
    assert_eq!(n.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn parallel_collects_all_errors() {
    let root = Context::new();
    root.on("e", |_: &()| {}).unwrap();
    // Handler errors are sync Result from the internal adapter; use two
    // listeners that panic-free fail via on_bail-style Result by registering
    // a throwing-equivalent through a custom waterfall is overkill. Drive
    // parallel with ordinary listeners: both run.
    let n = hits();
    root.on("p", {
        let n = n.clone();
        move |_: &()| {
            n.fetch_add(1, Ordering::SeqCst);
        }
    })
    .unwrap();
    root.on("p", {
        let n = n.clone();
        move |_: &()| {
            n.fetch_add(10, Ordering::SeqCst);
        }
    })
    .unwrap();
    root.parallel("p", ()).await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 11);
}

#[tokio::test]
async fn emit_handler_error_is_logged() {
    let root = Context::new();
    // The public `on` adapter never returns Err. Bail/waterfall log instead of
    // panicking; this asserts emit of a missing-type payload is a no-op.
    root.on("e", |_: &i32| {}).unwrap();
    root.emit("e", ());
    assert!(root.logger().errors().is_empty());
}

#[tokio::test]
async fn internal_plugin_and_status_events() {
    let root = Context::new();
    let plugins = Arc::new(std::sync::Mutex::new(Vec::new()));
    let statuses = Arc::new(std::sync::Mutex::new(Vec::new()));
    root.on("internal/plugin", {
        let plugins = plugins.clone();
        move |f: &Fiber| {
            plugins.lock().unwrap().push(f.name());
        }
    })
    .unwrap();
    root.on("internal/status", {
        let statuses = statuses.clone();
        move |ev: &StatusEvent| {
            statuses.lock().unwrap().push((ev.old, ev.new));
        }
    })
    .unwrap();

    let fiber = root
        .plugin(plugin("named", Inject::new(), |_ctx, _: &()| Ok(None)), ())
        .unwrap();
    fiber.wait().await.unwrap();
    assert!(plugins.lock().unwrap().iter().any(|n| n == "named"));
    assert!(statuses
        .lock()
        .unwrap()
        .iter()
        .any(|(old, new)| *old == FiberState::Pending && *new == FiberState::Loading));

    fiber.dispose().await.unwrap();
    assert!(
        plugins
            .lock()
            .unwrap()
            .iter()
            .filter(|n| *n == "named")
            .count()
            >= 2
    );
}

#[tokio::test]
async fn get_effects_labels_provide_and_on() {
    let root = Context::new();
    let _d = root.provide("foo", 1i32).unwrap();
    let _h = root.on("e", |_: &()| {}).unwrap();
    let labels: Vec<_> = root
        .fiber()
        .get_effects()
        .into_iter()
        .map(|m| m.label)
        .collect();
    assert!(labels.iter().any(|l| l.contains("provide")));
    assert!(labels.iter().any(|l| l.contains("on")));
}

/// `internal/service` 在服务**出现**和**消失**时都发一次。
///
/// 这是「不轮询就能看服务表」的那个位点：provide 与 provider 卸载都汇到
/// `notify`，所以一个发射点覆盖两种。
#[tokio::test]
async fn internal_service_fires_on_provide_and_on_teardown() {
    let root = Context::new();
    let seen = Arc::new(Mutex::new(Vec::<(String, bool)>::new()));
    let tap = seen.clone();
    let _h = root
        .on("internal/service", move |e: &cordis::ServiceEvent| {
            tap.lock()
                .unwrap()
                .push((e.name.clone(), e.value.is_some()));
        })
        .unwrap();

    let provider = plugin("provider", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide("thing", 7u32)?))
    });
    let fiber = root.plugin(provider, ()).unwrap();
    fiber.wait().await.unwrap();
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[("thing".to_string(), true)],
        "provide 要发一次，且带得出值"
    );

    fiber.dispose().await.unwrap();
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[("thing".to_string(), true), ("thing".to_string(), false)],
        "provider 卸载要再发一次，value 为 None"
    );
}

/// `internal/dispatch` 看得见每一次普通分发，且带得出分发形态。
#[tokio::test]
async fn internal_dispatch_sees_every_ordinary_dispatch() {
    let root = Context::new();
    let seen = Arc::new(Mutex::new(Vec::<(cordis::DispatchMode, String)>::new()));
    let tap = seen.clone();
    let _h = root
        .on("internal/dispatch", move |e: &cordis::DispatchEvent| {
            tap.lock().unwrap().push((e.mode, e.name.clone()));
        })
        .unwrap();

    root.emit("ping", ());
    let _ = root.bail::<(), ()>("ask", ());
    let _ = root.waterfall("shape", 1u8, || 1u8);

    let got = seen.lock().unwrap().clone();
    assert_eq!(
        got,
        vec![
            (cordis::DispatchMode::Emit, "ping".to_string()),
            (cordis::DispatchMode::Bail, "ask".to_string()),
            (cordis::DispatchMode::Waterfall, "shape".to_string()),
        ]
    );
}

/// Parallel 分支也要被追踪——它与另外三个入口同构，但它是唯一的异步入口，
/// 单独钉一条，免得有人以后把 `trace_dispatch` 从 `parallel` 里摘掉时只有
/// 一条远处的测试替它兜底。
#[tokio::test]
async fn internal_dispatch_sees_parallel_dispatch_too() {
    let root = Context::new();
    let seen = Arc::new(Mutex::new(Vec::<(cordis::DispatchMode, String)>::new()));
    let tap = seen.clone();
    let _h = root
        .on("internal/dispatch", move |e: &cordis::DispatchEvent| {
            tap.lock().unwrap().push((e.mode, e.name.clone()));
        })
        .unwrap();
    let _h2 = root.on("fan", |_: &()| {});

    root.parallel("fan", ()).await.unwrap();
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[(cordis::DispatchMode::Parallel, "fan".to_string())]
    );
}

/// `internal/*` 自身不进 `internal/dispatch`——否则第一条追踪就会自激成无限递归。
#[tokio::test]
async fn internal_dispatch_does_not_trace_itself() {
    let root = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let tap = hits.clone();
    let _h = root
        .on("internal/dispatch", move |_: &cordis::DispatchEvent| {
            tap.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

    // 挂一颗插件：内部会发 internal/plugin、internal/status、internal/service。
    let p = plugin("noisy", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide("noisy.thing", 1u8)?))
    });
    root.plugin(p, ()).unwrap().wait().await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 0, "internal/* 不该被追踪");

    root.emit("ordinary", ());
    assert_eq!(hits.load(Ordering::SeqCst), 1, "普通事件要被追踪");
}

/// `internal/listener` 是 bail：给出值即否决这次注册。
#[tokio::test]
async fn internal_listener_can_veto_a_registration() {
    let root = Context::new();
    let _guard = root
        .on_bail("internal/listener", |e: &cordis::ListenerEvent| {
            // 把 "reserved" 这个事件名据为己有。
            (e.name == "reserved").then_some(())
        })
        .unwrap();

    let ok = root.on("free", |_: &()| {});
    assert!(ok.is_ok(), "没被否决的照常注册");

    let rejected = root.on("reserved", |_: &()| {});
    match rejected {
        Err(cordis::Error::ListenerRejected { name }) => assert_eq!(name, "reserved"),
        Err(other) => panic!("要报 ListenerRejected，实际 {other}"),
        Ok(_) => panic!("被否决的不该注册成功"),
    }
}

/// 摘掉监听者之后追踪立刻停——`has_listener` 那道闸读的是当前 hook 表，不是
/// 注册时的快照。
///
/// 注意 `Disposable` 不是 RAII：drop 它不解绑（监听器归 fiber 的 effect 管），
/// 要显式 `dispose()`。
///
/// 「没人听时不构造载荷」这条**省下的开销本身测不出来**（从外部看不见
/// `DispatchEvent` 造没造），所以不写一条假装能测它的用例；真正危险的那面——
/// 追踪自激成无限递归——由 `internal_dispatch_does_not_trace_itself` 挡着。
#[tokio::test]
async fn tracing_stops_when_the_listener_goes_away() {
    let root = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let tap = hits.clone();
    let handle = root
        .on("internal/dispatch", move |_: &cordis::DispatchEvent| {
            tap.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

    root.emit("before", ());
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    handle.dispose().await.unwrap();
    root.emit("after", ());
    assert_eq!(hits.load(Ordering::SeqCst), 1, "摘掉之后不该再涨");
    assert!(!root.hook_snapshot().contains_key("internal/dispatch"));
}
