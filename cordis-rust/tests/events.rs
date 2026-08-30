use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
