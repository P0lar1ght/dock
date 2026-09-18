//! `[toolset.web_fetch]` 的接线：磁盘上的配置真的走到 `web_fetch` 的行为里。
//!
//! 三条用例都刻意**不碰网络**：白名单在发请求前就拒，`127.0.0.1:1` 是必然连不上
//! 的显式本地地址（`upgrade_to_https` 对显式本地主机不抬 https）。所以这三条在
//! 离线 CI 上结果确定。

use cordis::Context;
use cordis_spine::{ToolCall, Tools, TOOLS};

/// 本测试二进制共用一个隔离的 `DOCK_HOME`（同 `round.rs` 的理由：真实 `~/.dock`
/// 里可能有 `roster.yml`，也可能真装着 cua-driver）。不还原、不删除，进程退出即回收。
fn isolated_home() -> &'static std::path::Path {
    static HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.keep();
        std::env::set_var("DOCK_HOME", &path);
        std::env::set_var("DOCK_CUA_DRIVER", "off");
        path
    })
}

/// 三条用例都要写同一个 `$DOCK_HOME/config.toml`，而配置是在 `install_app` 那一刻
/// 读的——必须串行。用 `tokio::sync::Mutex`：`std` 那把（`EnvScope` 用的）跨
/// `await` 会被 clippy 判 `await_holding_lock`，还可能把执行器卡住。
static CONFIG_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn boot_with_config(body: &str) -> Context {
    std::fs::write(isolated_home().join("config.toml"), body).unwrap();
    let root = Context::new();
    cordis_spine::install_app(&root).await.unwrap();
    root
}

async fn fetch(root: &Context, url: &str) -> String {
    root.require::<Tools>(TOOLS)
        .unwrap()
        .execute(ToolCall {
            id: "t".into(),
            name: "web_fetch".into(),
            arguments: format!(r#"{{"url":"{url}"}}"#),
        })
        .await
        .content
}

/// `allowed_domains` 一旦在配置里生效，就是**发请求之前**的闸——这条用例连 DNS
/// 都不会走到，所以它同时证明了「配置确实接到了工具上」和「闸在 I/O 之前」。
/// 显式空表是**全拒**，不是「没设闸」。
#[tokio::test]
async fn web_fetch_enforces_allowlist_from_config() {
    let _guard = CONFIG_LOCK.lock().await;
    let root = boot_with_config("[toolset.web_fetch]\nallowed_domains = []\n").await;
    let out = fetch(&root, "https://example.com/").await;
    assert!(
        out.contains("not in the allowed domains list"),
        "配置里的白名单没作用到工具上：{out}"
    );
}

/// 对照组：没配 `allow_local` 时，显式本地地址照旧被 SSRF 拦。
#[tokio::test]
async fn explicit_local_host_still_blocked_without_the_key() {
    let _guard = CONFIG_LOCK.lock().await;
    let root = boot_with_config("# 空配置\n").await;
    let out = fetch(&root, "http://127.0.0.1:1/").await;
    assert!(
        out.contains("SSRF blocked"),
        "没配 allow_local 却放行了：{out}"
    );
}

/// 配了 `allow_local = true` 之后，SSRF 那一关让开，请求真的发出去（连不上是必然
/// 的，端口 1 没人听）。断言的是「不再被 SSRF 拦」而不是「请求成功」——后者需要
/// 网络，测试不该依赖。
#[tokio::test]
async fn allow_local_from_config_reaches_the_ssrf_gate() {
    let _guard = CONFIG_LOCK.lock().await;
    let root = boot_with_config("[toolset.web_fetch]\nallow_local = true\n").await;
    let out = fetch(&root, "http://127.0.0.1:1/").await;
    assert!(!out.contains("SSRF blocked"), "allow_local 没生效：{out}");
    assert!(
        out.contains("HTTP request failed"),
        "没走到发请求那步：{out}"
    );
}
