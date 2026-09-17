//! cua-driver 的本机发现、安装与授权，给 `computer` 驾驶舱和内置 MCP 行共用。
//!
//! Dock **不打包** driver 二进制：macOS 上它是 trycua 签名的 app
//! （`com.trycua.driver`），Accessibility / 屏幕录制授权绑在那份签名身份上，
//! 拷进 Dock 的产物里重签会让授权失效，也等于重分发别人的公证产物。所以这里
//! 只做两件事：找本机已有的 driver，以及调**官方安装脚本**把它装上。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// MCP 服务器键名。公名前缀 `mcp_cua-driver__`，必须与 TOOLS.md 一致。
pub const CUA_DRIVER_SERVER: &str = "cua-driver";

/// 官方安装脚本：sudo-free，装进 `~/.local/bin`；macOS 落 `/Applications/CuaDriver.app`。
pub const INSTALL_SCRIPT_URL: &str = "https://cua.ai/driver/install.sh";

/// driver 绝对路径覆盖；设成 `off` / `0` / `false` / `none` 则整条内置行都不注入。
pub const DRIVER_ENV: &str = "DOCK_CUA_DRIVER";

/// macOS 官方安装位置（`.app` 里的可执行文件，授权就绑这份签名身份）。
const MACOS_APP_BIN: &str = "/Applications/CuaDriver.app/Contents/MacOS/cua-driver";

/// `permissions status` 只读、不弹窗，但仍要给守护进程留握手时间。
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// 装包要下 30–70MB，给足；卡死时还是得有个上限，否则驾驶舱永远 busy。
const INSTALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// 授权要等人去系统设置里点开关，给 10 分钟。
const GRANT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

fn bin_name() -> &'static str {
    if cfg!(windows) {
        "cua-driver.exe"
    } else {
        "cua-driver"
    }
}

/// `DOCK_CUA_DRIVER` 的关闭值：显式关掉内置 cua-driver。
fn is_off(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "false" | "no" | "none" | "disabled"
    )
}

/// 找本机 cua-driver：`DOCK_CUA_DRIVER` → `PATH` → `~/.local/bin` → macOS `.app`。
pub fn discover() -> Option<PathBuf> {
    discover_with(
        std::env::var_os(DRIVER_ENV),
        std::env::var_os("PATH"),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

/// [`discover`] 的可测版本：三个来源全由调用方给，不读进程环境。
pub fn discover_with(
    explicit: Option<OsString>,
    path_var: Option<OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(raw) = explicit {
        let text = raw.to_string_lossy().trim().to_string();
        if is_off(&text) {
            return None;
        }
        if !text.is_empty() {
            // 显式给了路径就只认它：指错了要能看见「未安装」，而不是悄悄回落。
            let path = PathBuf::from(text);
            return is_exec(&path).then_some(path);
        }
    }
    if let Some(path_var) = path_var {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(bin_name());
            if is_exec(&candidate) {
                return Some(candidate);
            }
        }
    }
    // 官方安装脚本的默认落点。GUI 里起的终端常常没有 `~/.local/bin`，
    // 只查 PATH 会把装好的 driver 判成没装。
    let mut fallbacks = Vec::new();
    if let Some(home) = home {
        fallbacks.push(home.join(".local").join("bin").join(bin_name()));
    }
    if cfg!(target_os = "macos") {
        fallbacks.push(PathBuf::from(MACOS_APP_BIN));
    }
    fallbacks.into_iter().find(|p| is_exec(p))
}

fn is_exec(path: &Path) -> bool {
    // symlink 要跟到底（`~/.local/bin/cua-driver` 通常指向 `.app` 里那份）。
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// macOS TCC 授权状态。非 macOS 是 [`Perms::NotRequired`]。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Perms {
    /// 没探测过，或 driver 报不出来（没有守护进程时它自己就回 `unknown`）。
    #[default]
    Unknown,
    /// 这个平台不需要（非 macOS）。
    NotRequired,
    Granted,
    Missing {
        accessibility: bool,
        screen_recording: bool,
    },
}

/// 解析 `cua-driver permissions status --json`。两个布尔都为真才算授全；
/// 任一为假是缺授权；读不出来（没守护进程 / 非 JSON）保持 `Unknown`。
pub fn parse_permissions(json: &str) -> Perms {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Perms::Unknown;
    };
    let accessibility = value.get("accessibility").and_then(|v| v.as_bool());
    let screen_recording = value.get("screen_recording").and_then(|v| v.as_bool());
    match (accessibility, screen_recording) {
        (Some(true), Some(true)) => Perms::Granted,
        (Some(a), Some(s)) => Perms::Missing {
            accessibility: a,
            screen_recording: s,
        },
        _ => Perms::Unknown,
    }
}

/// 读一次 TCC 授权。只读命令，不会弹窗（弹窗是 `permissions grant` 的事）。
pub async fn probe_permissions(driver: &Path) -> Perms {
    if !cfg!(target_os = "macos") {
        return Perms::NotRequired;
    }
    let run = tokio::process::Command::new(driver)
        .args(["permissions", "status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(PROBE_TIMEOUT, run).await {
        Ok(Ok(out)) => parse_permissions(&String::from_utf8_lossy(&out.stdout)),
        _ => Perms::Unknown,
    }
}

/// 子进程的一行输出。驾驶舱把它追加进进度区。
pub type LineSink = Arc<dyn Fn(String) + Send + Sync>;

/// 确认态展示的执行计划——按下去到底会跑什么，一行不藏。
pub fn install_plan_lines() -> Vec<String> {
    vec![
        format!("1. 下载 {INSTALL_SCRIPT_URL}（trycua 官方安装脚本）"),
        "2. bash <临时文件> --no-modify-path".into(),
        "脚本自己从 GitHub Releases 取最新 stable 版并校验；macOS 装成 /Applications/CuaDriver.app（签名 app，授权才立得住）。".into(),
        "装进 ~/.local/bin，不改你的 shell rc —— Dock 自己认得这个目录。".into(),
    ]
}

/// 下载并执行官方安装脚本。全程输出走 `sink`。
pub async fn install(sink: LineSink) -> Result<(), String> {
    sink(format!("下载 {INSTALL_SCRIPT_URL} …"));
    let script = reqwest::get(INSTALL_SCRIPT_URL)
        .await
        .map_err(|e| format!("下载安装脚本失败：{e}"))?
        .error_for_status()
        .map_err(|e| format!("下载安装脚本失败：{e}"))?
        .text()
        .await
        .map_err(|e| format!("读取安装脚本失败：{e}"))?;
    // 拿到的不是脚本（代理返回登录页之类）就别往 bash 里喂。
    if !script.starts_with("#!") {
        return Err("下载到的内容不是 shell 脚本，已中止".into());
    }
    let path = std::env::temp_dir().join(format!("dock-cua-install-{}.sh", std::process::id()));
    tokio::fs::write(&path, script.as_bytes())
        .await
        .map_err(|e| format!("写临时脚本失败：{e}"))?;
    sink(format!("bash {} --no-modify-path", path.display()));
    let result = run_streaming(
        OsStr::new("bash"),
        &[path.as_os_str(), OsStr::new("--no-modify-path")],
        sink,
        INSTALL_TIMEOUT,
    )
    .await;
    let _ = tokio::fs::remove_file(&path).await;
    result
}

/// 跑 `cua-driver permissions grant`：由 driver 自己通过 LaunchServices 拉起
/// CuaDriver，让系统弹窗归属到那个 app，再验证一次实时抓屏。
pub async fn grant(driver: &Path, sink: LineSink) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("只有 macOS 需要 Accessibility / 屏幕录制授权".into());
    }
    sink(format!("{} permissions grant", driver.display()));
    run_streaming(
        driver.as_os_str(),
        &[OsStr::new("permissions"), OsStr::new("grant")],
        sink,
        GRANT_TIMEOUT,
    )
    .await
}

async fn run_streaming(
    program: &OsStr,
    args: &[&OsStr],
    sink: LineSink,
    timeout: Duration,
) -> Result<(), String> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("启动 {} 失败：{e}", program.to_string_lossy()))?;

    // stdout / stderr 必须并发抽干：任一侧写满管道缓冲子进程就阻塞，另一侧
    // 再也读不到 EOF（jobs.rs 的死锁同款）。
    let out = tokio::spawn(pump(child.stdout.take(), sink.clone()));
    let err = tokio::spawn(pump(child.stderr.take(), sink));
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => status.map_err(|e| format!("等待子进程失败：{e}"))?,
        Err(_) => {
            let _ = child.kill().await;
            out.abort();
            err.abort();
            return Err(format!("超时（{} 秒）已终止", timeout.as_secs()));
        }
    };
    let _ = tokio::join!(out, err);
    if status.success() {
        Ok(())
    } else {
        Err(format!("退出码 {}", status.code().unwrap_or(-1)))
    }
}

async fn pump<R: AsyncRead + Unpin + Send + 'static>(reader: Option<R>, sink: LineSink) {
    let Some(reader) = reader else {
        return;
    };
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim_end().to_string();
        if !line.is_empty() {
            sink(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_switch_beats_every_other_source() {
        for value in ["off", "0", "False", "none", " disabled "] {
            assert_eq!(
                discover_with(Some(value.into()), Some("/usr/bin".into()), None),
                None,
                "{value} 应该关掉内置 driver"
            );
        }
    }

    #[test]
    fn explicit_path_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert_eq!(
            discover_with(Some(missing.into_os_string()), None, None),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn finds_binary_on_path_then_local_bin() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let exe = bin_dir.join("cua-driver");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(
            discover_with(None, Some(bin_dir.clone().into_os_string()), None),
            Some(exe.clone())
        );

        // PATH 上没有时回落 ~/.local/bin。
        let home = dir.path().join("home");
        let local = home.join(".local").join("bin");
        std::fs::create_dir_all(&local).unwrap();
        let home_exe = local.join("cua-driver");
        std::fs::write(&home_exe, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&home_exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            discover_with(None, Some("/nonexistent-dock-dir".into()), Some(home)),
            Some(home_exe)
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_file_is_not_a_driver() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("cua-driver");
        std::fs::write(&exe, b"not a program").unwrap();
        // 开发机上 macOS 的 `.app` 兜底可能真的存在，所以断言「不是这个文件」
        // 而不是「什么都没找到」。
        assert_ne!(
            discover_with(None, Some(dir.path().to_path_buf().into_os_string()), None),
            Some(exe)
        );
    }

    #[test]
    fn permissions_json_maps_to_states() {
        assert_eq!(
            parse_permissions(r#"{"accessibility":true,"screen_recording":true}"#),
            Perms::Granted
        );
        assert_eq!(
            parse_permissions(r#"{"accessibility":false,"screen_recording":true}"#),
            Perms::Missing {
                accessibility: false,
                screen_recording: true
            }
        );
        // 没有守护进程时 driver 报不出布尔，别当成缺授权催人去点开关。
        assert_eq!(
            parse_permissions(r#"{"accessibility":null,"screen_recording":null}"#),
            Perms::Unknown
        );
        assert_eq!(parse_permissions("not json"), Perms::Unknown);
    }
}
