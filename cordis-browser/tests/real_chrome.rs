//! 真 Chrome 冒烟（`#[ignore]`）：`cargo test -p cordis-browser -- --ignored`。
//! 从 spine 的 `tool-browser` 测试搬来，改走 [`BrowserHub`]，并补上按会话分标签页。

use cordis_browser::{session::discover_chrome, BrowserHub, CallOutput};
use serde_json::{json, Value};

fn chrome_or_skip(what: &str) -> bool {
    if discover_chrome().is_err() {
        eprintln!("skip {what}: chrome not installed");
        return false;
    }
    true
}

async fn ok(hub: &BrowserHub, session: &str, name: &str, args: Value) -> CallOutput {
    let out = hub.call(session, name, &args).await;
    assert!(!out.is_error, "{name}: {}", out.text);
    out
}

#[tokio::test]
#[ignore = "needs a real Chrome; page text and snapshots differ by Chrome version/locale"]
async fn sessions_get_their_own_tabs() {
    let dock_home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", dock_home.path());
    if !chrome_or_skip("sessions") {
        return;
    }
    let hub = BrowserHub::new();
    ok(
        &hub,
        "a",
        "browser_open",
        json!({"url": "data:text/html,<p>PageA</p>"}),
    )
    .await;
    ok(
        &hub,
        "b",
        "browser_open",
        json!({"url": "data:text/html,<p>PageB</p>"}),
    )
    .await;
    assert_eq!(hub.sessions().await, vec!["a", "b"]);

    let a = ok(&hub, "a", "browser_tabs", json!({})).await;
    assert!(a.text.contains("PageA"), "{}", a.text);
    assert!(
        !a.text.contains("PageB"),
        "a must not see b's tab: {}",
        a.text
    );

    // a 关掉自己的，不影响 b。
    ok(&hub, "a", "browser_close", json!({})).await;
    let gone = hub.call("a", "browser_snapshot", &json!({})).await;
    assert!(gone.is_error, "{}", gone.text);
    ok(
        &hub,
        "b",
        "browser_wait_for",
        json!({"text": "PageB", "timeout_ms": 5000}),
    )
    .await;

    ok(&hub, "b", "browser_close", json!({})).await;
    assert!(hub.sessions().await.is_empty());
    hub.shutdown().await;
}

#[tokio::test]
#[ignore = "needs a real Chrome; page text and snapshots differ by Chrome version/locale"]
async fn second_hub_attaches_to_running_chromium() {
    let dock_home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", dock_home.path());
    if !chrome_or_skip("attach") {
        return;
    }
    let first = BrowserHub::new();
    ok(&first, "a", "browser_open", json!({"url": "about:blank"})).await;
    let user_data = cordis_browser::browser_user_data_dir();
    assert!(
        cordis_browser::devtools_ws_url(&user_data).is_some(),
        "Chromium should write DevToolsActivePort under {}",
        user_data.display()
    );

    // 同一 profile 的第二个进程（比如 TUI 与 GUI 各一个 Dock）连上同一个 Chromium。
    let second = BrowserHub::new();
    ok(
        &second,
        "b",
        "browser_open",
        json!({"url": "data:text/html,<p>Second</p>"}),
    )
    .await;
    // 连上的那边关自己的组，不能把别人的 Chromium 关掉。
    second.shutdown().await;
    ok(&first, "a", "browser_snapshot", json!({})).await;
    first.shutdown().await;
}

#[tokio::test]
#[ignore = "needs a real Chrome; page text and snapshots differ by Chrome version/locale"]
async fn p0_navigate_press_wait() {
    let dock_home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", dock_home.path());
    if !chrome_or_skip("p0") {
        return;
    }
    let hub = BrowserHub::new();
    ok(
        &hub,
        "s",
        "browser_open",
        json!({"url": "data:text/html,<html><body><h1>HelloBUA</h1><select id=s><option value=a>A</option><option value=b>B</option></select><input id=i /></body></html>"}),
    )
    .await;
    let nav = ok(
        &hub,
        "s",
        "browser_navigate",
        json!({"url": "data:text/html,<html><body><p>NavOK</p><input id=x /></body></html>"}),
    )
    .await;
    assert!(nav.text.contains("navigated"), "{}", nav.text);
    ok(
        &hub,
        "s",
        "browser_wait_for",
        json!({"text": "NavOK", "timeout_ms": 5000}),
    )
    .await;
    ok(&hub, "s", "browser_press_key", json!({"key": "Tab"})).await;
    let shot = ok(&hub, "s", "browser_screenshot", json!({})).await;
    assert!(shot.image.as_ref().is_some_and(|p| p.is_file()), "{shot:?}");

    ok(&hub, "s", "browser_close", json!({})).await;
    let closed = hub
        .call("s", "browser_navigate", &json!({"url": "about:blank"}))
        .await;
    assert!(
        closed.is_error && closed.text.contains("browser_open"),
        "{}",
        closed.text
    );
    hub.shutdown().await;
}

#[tokio::test]
#[ignore = "needs a real Chrome; page text and snapshots differ by Chrome version/locale"]
async fn p1_resize_dialog_upload_drag() {
    let dock_home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", dock_home.path());
    if !chrome_or_skip("p1") {
        return;
    }
    let hub = BrowserHub::new();
    ok(
        &hub,
        "s",
        "browser_open",
        json!({"url": "data:text/html,<html><body><div id=a style='width:40px;height:40px'>A</div><div id=b style='width:40px;height:40px;margin-top:80px'>B</div><input id=f type=file /><script>setTimeout(function(){alert('BUA-D2');},50);</script></body></html>"}),
    )
    .await;
    let resize = ok(
        &hub,
        "s",
        "browser_resize",
        json!({"width": 1024, "height": 768}),
    )
    .await;
    assert!(resize.text.contains("1024x768"), "{}", resize.text);

    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    ok(&hub, "s", "browser_handle_dialog", json!({"accept": true})).await;
    ok(
        &hub,
        "s",
        "browser_drag",
        json!({"start_x": 20, "start_y": 20, "end_x": 20, "end_y": 120, "steps": 5}),
    )
    .await;

    let upload_path = dock_home.path().join("upload.txt");
    std::fs::write(&upload_path, b"hello").unwrap();
    let snap = ok(&hub, "s", "browser_snapshot", json!({"interactive": true})).await;
    // 页上只有文件框一个可交互元素；按钮文案随系统语言变（「选择文件」/ "Choose File"），
    // 所以直接取第一个 ref。
    let file_ref = snap
        .text
        .lines()
        .find_map(|l| {
            let idx = l.find("ref=e")?;
            let id: String = l[idx + 4..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            Some(format!("@{id}"))
        })
        .unwrap_or_else(|| panic!("expected file input ref in snapshot:\n{}", snap.text));
    ok(
        &hub,
        "s",
        "browser_file_upload",
        json!({"ref": file_ref, "paths": [upload_path.display().to_string()]}),
    )
    .await;
    hub.shutdown().await;
}

#[tokio::test]
#[ignore = "needs a real Chrome; page text and snapshots differ by Chrome version/locale"]
async fn p2_evaluate_network_iframe() {
    let dock_home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", dock_home.path());
    if !chrome_or_skip("p2") {
        return;
    }
    let hub = BrowserHub::new();
    ok(&hub, "s", "browser_open", json!({"url": "about:blank"})).await;
    ok(
        &hub,
        "s",
        "browser_evaluate",
        json!({"expression": "(() => { document.body.innerHTML = '<h1 id=t>P2Main</h1><iframe id=f name=child src=\"data:text/html,<html><body><p id=p>InsideFrame</p></body></html>\"></iframe>'; console.log('BUA-P2-LOG'); return document.title = 'p2'; })()"}),
    )
    .await;
    let ev = ok(&hub, "s", "browser_evaluate", json!({"expression": "1+2"})).await;
    assert!(ev.text.contains('3'), "{}", ev.text);

    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let console = ok(&hub, "s", "browser_console_messages", json!({})).await;
    assert!(
        console.text.contains("BUA-P2-LOG") || console.text.contains("log:"),
        "{}",
        console.text
    );
    ok(&hub, "s", "browser_network_requests", json!({})).await;

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let framed = ok(
        &hub,
        "s",
        "browser_evaluate",
        json!({"expression": "document.getElementById('p') && document.getElementById('p').textContent", "frame_selector": "#f"}),
    )
    .await;
    assert!(framed.text.contains("InsideFrame"), "{}", framed.text);

    let missing = hub
        .call(
            "s",
            "browser_evaluate",
            &json!({"expression": "1", "frame_selector": "#missing"}),
        )
        .await;
    assert!(missing.is_error, "{}", missing.text);
    hub.shutdown().await;
}

/// 网关看画面的那条路：hub 开页 → 名册里查到这个会话的活动 target → View 挂上去
/// 收到帧 → 以用户身份点按钮、页面有反应 → 导航与后退。
#[tokio::test]
#[ignore = "needs a real Chrome; page text and snapshots differ by Chrome version/locale"]
async fn view_streams_frames_and_forwards_input() {
    use cordis_browser::view::{ScreencastOptions, View};
    let dock_home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", dock_home.path());
    if !chrome_or_skip("view") {
        return;
    }
    let hub = BrowserHub::new();
    ok(
        &hub,
        "gui-thread",
        "browser_open",
        json!({"url": "data:text/html,<title>Before</title><button style='position:fixed;left:0;top:0;width:200px;height:100px' onclick=\"document.title='Clicked'\">Go</button>"}),
    )
    .await;
    let tabs = cordis_browser::registry::lookup("gui-thread").expect("hub publishes its tabs");
    let target = tabs.active.clone().expect("active target");
    assert_eq!(tabs.targets, vec![target.clone()]);

    let view = View::attach(&target).await.unwrap();
    let mut frames = view.screencast(ScreencastOptions::default()).await.unwrap();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(10), frames.next())
        .await
        .expect("a frame within 10s")
        .expect("stream open");
    assert!(frame.data.len() > 100, "base64 jpeg");
    assert!(
        frame.device_width > 0.0 && frame.device_height > 0.0,
        "{frame:?}"
    );
    view.ack(frame.ack_id).await;

    assert_eq!(view.info().await.title, "Before");
    view.input(&json!({"type": "mouse", "action": "click", "x": 50, "y": 50}))
        .await
        .unwrap();
    let mut clicked = false;
    for _ in 0..40 {
        if view.info().await.title == "Clicked" {
            clicked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(clicked, "user click reached the page");

    let title_becomes = |want: &'static str| {
        let view = &view;
        async move {
            for _ in 0..60 {
                if view.info().await.title == want {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            false
        }
    };
    view.navigate(&json!({"url": "data:text/html,<title>Second</title>"}))
        .await
        .unwrap();
    assert!(title_becomes("Second").await, "address bar navigation");
    view.navigate(&json!({"action": "back"})).await.unwrap();
    // 后退会重新加载上一页：标题回到页面自己写的 Before（Clicked 是运行时改的）。
    assert!(title_becomes("Before").await, "history back");

    // 关 View 不关页：agent 那边照常。
    view.close().await;
    ok(&hub, "gui-thread", "browser_snapshot", json!({})).await;
    ok(&hub, "gui-thread", "browser_close", json!({})).await;
    assert!(cordis_browser::registry::lookup("gui-thread").is_none());
    hub.shutdown().await;
}

/// 两个画面（主窗口和拖出去的面板窗各开一个）同时补放大静止帧：Chrome 并发的
/// `clip.scale` 截图互相把对方的临时视口当成原尺寸恢复，视口每次翻倍（900 → 1800 →
/// 3600…，最后渲染进程崩掉），画面一会儿全一会儿不全。同一页的静止帧要排队截。
#[tokio::test]
#[ignore = "needs a real Chrome; page text and snapshots differ by Chrome version/locale"]
async fn concurrent_stills_keep_the_viewport() {
    use cordis_browser::view::View;
    let dock_home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", dock_home.path());
    if !chrome_or_skip("stills") {
        return;
    }
    let hub = BrowserHub::new();
    ok(
        &hub,
        "a",
        "browser_open",
        json!({"url": "data:text/html,<div style='min-width:1100px;height:3000px'>x</div>"}),
    )
    .await;
    ok(
        &hub,
        "a",
        "browser_resize",
        json!({"width": 900, "height": 642}),
    )
    .await;
    let target = cordis_browser::registry::lookup("a")
        .and_then(|t| t.active)
        .expect("active target");
    let first = View::attach(&target).await.unwrap();
    let second = View::attach(&target).await.unwrap();
    let size = json!({"expression": "`${innerWidth}x${innerHeight}`"});
    for _ in 0..3 {
        let (a, b) = tokio::join!(first.still(85, 2.0), second.still(85, 2.0));
        assert_eq!(a.unwrap().device_width, 900.0);
        assert_eq!(b.unwrap().device_width, 900.0);
        let now = ok(&hub, "a", "browser_evaluate", size.clone()).await;
        assert_eq!(now.text, "900x642");
    }
    drop((first, second));
    hub.shutdown().await;
}
