//! `dock device …` 与 `dock serve --remote`：真起二进制。

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn dock(home: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dock"));
    cmd.env("DOCK_HOME", home)
        .env("DOCK_CUA_DRIVER", "off")
        .env("DOCK_BROWSER_MCP", "off")
        .current_dir(home);
    cmd
}

fn run(home: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let out = dock(home).args(args).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn device_add_list_revoke() {
    let home = tempfile::tempdir().unwrap();
    let (code, out, _) = run(home.path(), &["device", "list"]);
    assert_eq!(code, 0);
    assert!(out.contains("还没有设备令牌"), "{out}");

    let (code, out, err) = run(home.path(), &["device", "add", "laptop"]);
    assert_eq!(code, 0, "{err}");
    let token = out
        .lines()
        .find(|l| l.starts_with("dock_"))
        .unwrap_or_else(|| panic!("令牌应该打印一次：{out}"))
        .to_string();
    let stored = std::fs::read_to_string(home.path().join("devices.json")).unwrap();
    assert!(!stored.contains(&token), "盘上不存明文令牌");

    let (code, _, err) = run(home.path(), &["device", "add", "laptop"]);
    assert_eq!(code, 1, "重名要失败");
    assert!(err.contains("laptop"), "{err}");

    let (_, out, _) = run(home.path(), &["device", "list"]);
    assert!(out.contains("laptop") && out.contains("从未使用"), "{out}");
    assert!(!out.contains(&token), "list 不显示令牌");

    let (code, out, _) = run(home.path(), &["device", "revoke", "laptop"]);
    assert_eq!(code, 0);
    assert!(out.contains("已撤销"), "{out}");
    let (code, _, err) = run(home.path(), &["device", "revoke", "laptop"]);
    assert_eq!(code, 1);
    assert!(err.contains("没有设备"), "{err}");

    let (code, _, err) = run(home.path(), &["device", "add"]);
    assert_eq!(code, 2, "{err}");
}

#[test]
fn serve_remote_refuses_origin_and_non_loopback() {
    let home = tempfile::tempdir().unwrap();
    let (code, _, err) = run(
        home.path(),
        &["serve", "--remote", "--origin", "tauri://localhost"],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("设备令牌"), "{err}");

    let (code, _, err) = run(
        home.path(),
        &["serve", "--remote", "--bind", "0.0.0.0:18990"],
    );
    assert_ne!(code, 0, "远程模式也只绑回环：{err}");
    assert!(err.contains("loopback"), "{err}");
}

/// 起来后在 stderr 报监听地址、提示没设备；不看 stdin（关掉也不退出）；SIGTERM / kill 才停。
#[test]
fn serve_remote_runs_until_stopped() {
    let home = tempfile::tempdir().unwrap();
    let mut child = dock(home.path())
        .args(["serve", "--remote", "--bind", "127.0.0.1:0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !seen.contains("dock device add") {
        let mut line = String::new();
        if stderr.read_line(&mut line).unwrap() == 0 || Instant::now() > deadline {
            let _ = child.kill();
            panic!("没等到就绪提示：{seen}");
        }
        seen.push_str(&line);
    }
    assert!(seen.contains("监听 ws://127.0.0.1:"), "{seen}");
    // stdin 是 /dev/null（systemd 的常态）：不能因为读到 EOF 就退出。
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        child.try_wait().unwrap().is_none(),
        "remote 模式不该随 stdin 退出"
    );
    stop(&mut child);
}

/// systemd 停服务发 SIGTERM：要自己干净退出（退出码 0），不是被杀。
#[cfg(unix)]
fn stop(child: &mut std::process::Child) {
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "SIGTERM 后应正常退出：{status}");
            return;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("SIGTERM 后没有退出");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(unix))]
fn stop(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}
