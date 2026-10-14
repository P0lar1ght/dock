//! `dock upgrade` / `dock update --check`：复用 install.sh 的原子替换路径。

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::busy::{busy_reasons, BusyReason};

const REPO: &str = "P0lar1ght/dock";
const ASSET: &str = "dock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Pre,
}

impl Channel {
    pub fn from_env() -> Self {
        match std::env::var("DOCK_CHANNEL")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "pre" | "prerelease" | "preview" => Self::Pre,
            _ => Self::Stable,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Pre => "pre",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeMode {
    /// 只检查，不下载不替换。
    Check,
    /// 下载并原子替换。
    Install,
}

#[derive(Debug, Clone)]
pub struct UpgradeArgs {
    pub mode: UpgradeMode,
    pub force: bool,
}

#[derive(Debug)]
pub enum UpgradeError {
    Message(String),
    Io(io::Error),
}

impl From<io::Error> for UpgradeError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl std::fmt::Display for UpgradeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(m) => write!(f, "{m}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

pub fn run_upgrade(args: &UpgradeArgs) -> Result<(), UpgradeError> {
    let channel = Channel::from_env();
    let current = env!("CARGO_PKG_VERSION");
    let remote = resolve_remote_version(channel)?;
    let remote_pretty = remote.trim_start_matches('v');

    println!("通道：{}（DOCK_CHANNEL）", channel.as_str());
    println!("当前版本：{current}");
    println!("远端版本：{remote_pretty}");

    let newer = is_remote_newer(current, remote_pretty);
    if args.mode == UpgradeMode::Check {
        if newer {
            println!("有新版本可用。运行 dock upgrade 安装（有进行中会话时需 --force）。");
        } else {
            println!("已是最新（或远端不高于当前）。");
        }
        return Ok(());
    }

    if !newer {
        println!("无需升级。");
        return Ok(());
    }

    if !args.force {
        let busy = busy_reasons();
        if !busy.is_empty() {
            eprintln!("检测到可能进行中的 Dock 会话或进程，拒绝静默升级：");
            for r in &busy {
                eprintln!("  - {}", r.message());
            }
            eprintln!("请先结束相关会话 / 进程，或显式传入 --force 强制升级。");
            return Err(UpgradeError::Message(
                "有进行中会话或进程，未加 --force".into(),
            ));
        }
    } else {
        let busy = busy_reasons();
        if !busy.is_empty() {
            eprintln!("警告：--force 下仍检测到：");
            for r in &busy {
                eprintln!("  - {}", r.message());
            }
            eprintln!("继续强制升级……");
        }
    }

    let target = detect_target()?;
    let install_dir = install_dir()?;
    let tag = if remote.starts_with('v') {
        remote.clone()
    } else {
        format!("v{remote}")
    };
    atomic_install(&tag, &target, &install_dir)?;
    println!("升级完成。");
    Ok(())
}

fn is_remote_newer(current: &str, remote: &str) -> bool {
    match (parse_semver(current), parse_semver(remote)) {
        (Some(c), Some(r)) => r > c,
        _ => current != remote,
    }
}

fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// 远端版本解析顺序：
/// 1. `DOCK_UPGRADE_REMOTE_VERSION`（测试 / 调试）
/// 2. `DOCK_VERSION`（与 install.sh 一致，指定要装的版本）
/// 3. GitHub API（stable=latest；pre=必须有 prerelease，否则硬失败，不回落正式版）
///
/// 测试可设 `DOCK_UPGRADE_RELEASES_JSON` 代替 releases 列表响应（仅 `pre` 通道）。
fn resolve_remote_version(channel: Channel) -> Result<String, UpgradeError> {
    if let Ok(v) = std::env::var("DOCK_UPGRADE_REMOTE_VERSION") {
        let v = v.trim();
        if !v.is_empty() {
            return Ok(normalize_tag(v));
        }
    }
    if let Ok(v) = std::env::var("DOCK_VERSION") {
        let v = v.trim();
        if !v.is_empty() {
            return Ok(normalize_tag(v));
        }
    }
    fetch_github_version(channel)
}

fn normalize_tag(v: &str) -> String {
    if v.starts_with('v') {
        v.to_string()
    } else {
        format!("v{v}")
    }
}

fn fetch_github_version(channel: Channel) -> Result<String, UpgradeError> {
    match channel {
        Channel::Stable => {
            let body = http_get(&format!(
                "https://api.github.com/repos/{REPO}/releases/latest"
            ))?;
            json_tag_name(&body).ok_or_else(|| {
                UpgradeError::Message("无法从 GitHub latest release 解析 tag_name".into())
            })
        }
        Channel::Pre => {
            let body = releases_list_body()?;
            pick_prerelease_tag(&body)
        }
    }
}

/// `DOCK_CHANNEL=pre`：必须从 releases 列表里挑出 prerelease；没有则硬失败，禁止回落正式版。
fn pick_prerelease_tag(body: &str) -> Result<String, UpgradeError> {
    if let Some(tag) = first_release_tag(body, true) {
        return Ok(tag);
    }
    Err(UpgradeError::Message(
        "DOCK_CHANNEL=pre，但 GitHub 上没有可用的 prerelease；拒绝回落正式版。请改用 DOCK_CHANNEL=stable，或发布一个 prerelease 后再试。".into(),
    ))
}

fn releases_list_body() -> Result<String, UpgradeError> {
    if let Ok(json) = std::env::var("DOCK_UPGRADE_RELEASES_JSON") {
        let json = json.trim();
        if !json.is_empty() {
            return Ok(json.to_string());
        }
    }
    http_get(&format!(
        "https://api.github.com/repos/{REPO}/releases?per_page=20"
    ))
}

fn http_get(url: &str) -> Result<String, UpgradeError> {
    // 与 install.sh 一致：优先 curl，避免给 cordis-app 加 HTTP 依赖。
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: dock-upgrade",
            url,
        ])
        .output()
        .map_err(|e| UpgradeError::Message(format!("需要 curl 访问 GitHub：{e}")))?;
    if !output.status.success() {
        return Err(UpgradeError::Message(format!(
            "请求失败：{url}：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn json_tag_name(body: &str) -> Option<String> {
    // 轻量解析，避免 serde_json 强依赖（dev 已有；正式依赖尽量不加）。
    // 找 "tag_name": "vX.Y.Z"
    let key = "\"tag_name\"";
    let idx = body.find(key)?;
    let after = &body[idx + key.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn first_release_tag(body: &str, want_prerelease: bool) -> Option<String> {
    // 粗拆 release 对象：按 "prerelease": true/false 与邻近 tag_name。
    // 足够覆盖 GitHub 列表 API 的常规 JSON。
    let mut search = body;
    while let Some(obj_start) = search.find('{') {
        let fragment = &search[obj_start..];
        let Some(obj_end_rel) = find_matching_brace(fragment) else {
            break;
        };
        let obj = &fragment[..=obj_end_rel];
        let is_pre = obj.contains("\"prerelease\":true") || obj.contains("\"prerelease\": true");
        if is_pre == want_prerelease {
            if let Some(tag) = json_tag_name(obj) {
                return Some(tag);
            }
        }
        if want_prerelease {
            // 继续找下一个
        } else if !is_pre {
            if let Some(tag) = json_tag_name(obj) {
                return Some(tag);
            }
        }
        search = &fragment[obj_end_rel + 1..];
    }
    None
}

fn find_matching_brace(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn detect_target() -> Result<String, UpgradeError> {
    let out = Command::new("uname")
        .arg("-s")
        .output()
        .map_err(|e| UpgradeError::Message(format!("uname 失败：{e}")))?;
    let os = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let out = Command::new("uname")
        .arg("-m")
        .output()
        .map_err(|e| UpgradeError::Message(format!("uname 失败：{e}")))?;
    let arch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let target = match (os.as_str(), arch.as_str()) {
        ("Darwin", "arm64" | "aarch64") => "aarch64-apple-darwin",
        ("Darwin", "x86_64") => "x86_64-apple-darwin",
        ("Linux", "x86_64" | "amd64") => "x86_64-unknown-linux-gnu",
        ("Linux", "aarch64" | "arm64") => "aarch64-unknown-linux-gnu",
        _ => {
            return Err(UpgradeError::Message(format!("不支持的平台：{os}/{arch}")));
        }
    };
    Ok(target.into())
}

fn install_dir() -> Result<PathBuf, UpgradeError> {
    if let Ok(d) = std::env::var("DOCK_INSTALL_DIR") {
        let d = d.trim();
        if !d.is_empty() {
            return Ok(PathBuf::from(d));
        }
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
    Ok(PathBuf::from(home).join(".local/bin"))
}

fn release_base(tag: &str) -> String {
    if let Ok(base) = std::env::var("DOCK_BASE_URL") {
        let base = base.trim_end_matches('/');
        if !base.is_empty() {
            return base.to_string();
        }
    }
    format!("https://github.com/{REPO}/releases/download/{tag}")
}

fn atomic_install(tag: &str, target: &str, install_dir: &Path) -> Result<(), UpgradeError> {
    let file = format!("{ASSET}-{target}.tar.gz");
    let base = release_base(tag);
    let tmp = tempfile_dir()?;
    let archive = tmp.join(&file);
    let checksum = tmp.join(format!("{file}.sha256"));

    println!("下载 {file}");
    download(&format!("{base}/{file}"), &archive)?;
    download(&format!("{base}/{file}.sha256"), &checksum)?;

    let expected = fs::read_to_string(&checksum)
        .map_err(|e| UpgradeError::Message(format!("读 sha256 失败：{e}")))?
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    if expected.is_empty() {
        return Err(UpgradeError::Message(
            "sha256 文件为空，拒绝安装未校验的产物".into(),
        ));
    }
    let actual = sha256_of(&archive)?;
    if expected != actual {
        return Err(UpgradeError::Message(format!(
            "校验失败：期望 {expected}，实际 {actual}"
        )));
    }

    extract_tar_gz(&archive, &tmp)?;
    let bin = tmp.join(ASSET);
    if !bin.is_file() {
        return Err(UpgradeError::Message(format!("产物里没有 {ASSET} 二进制")));
    }

    fs::create_dir_all(install_dir)?;
    let staged = install_dir.join(format!(".{ASSET}.new.{}", std::process::id()));
    fs::copy(&bin, &staged)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&staged)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&staged, perms)?;
    }
    let final_path = install_dir.join(ASSET);
    fs::rename(&staged, &final_path).map_err(|e| {
        let _ = fs::remove_file(&staged);
        UpgradeError::Message(format!("无法替换 {}：{e}", final_path.display()))
    })?;

    let ver = Command::new(&final_path)
        .arg("--version")
        .output()
        .map_err(|e| UpgradeError::Message(format!("装后自检失败：{e}")))?;
    if !ver.status.success() {
        return Err(UpgradeError::Message(format!(
            "{} 装上了但跑不起来：{}",
            final_path.display(),
            String::from_utf8_lossy(&ver.stderr).trim()
        )));
    }
    let text = String::from_utf8_lossy(&ver.stdout).trim().to_string();
    println!("已安装 {text} → {}", final_path.display());
    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

fn tempfile_dir() -> Result<PathBuf, UpgradeError> {
    let base = std::env::temp_dir();
    let dir = base.join(format!("dock-upgrade-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn download(url: &str, dest: &Path) -> Result<(), UpgradeError> {
    if let Some(rest) = url.strip_prefix("file://") {
        fs::copy(rest, dest)
            .map_err(|e| UpgradeError::Message(format!("复制本地产物失败：{rest}：{e}")))?;
        return Ok(());
    }
    let status = Command::new("curl")
        .args(["-fsSL", url, "-o"])
        .arg(dest)
        .status()
        .map_err(|e| UpgradeError::Message(format!("需要 curl：{e}")))?;
    if !status.success() {
        return Err(UpgradeError::Message(format!("下载失败：{url}")));
    }
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String, UpgradeError> {
    // macOS: shasum -a 256；Linux: sha256sum
    let shasum = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output();
    if let Ok(out) = shasum {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            return Ok(s.split_whitespace().next().unwrap_or("").to_string());
        }
    }
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|_| UpgradeError::Message("需要 sha256sum 或 shasum".into()))?;
    if !out.status.success() {
        return Err(UpgradeError::Message("校验命令失败".into()));
    }
    let s = String::from_utf8_lossy(&out.stdout);
    Ok(s.split_whitespace().next().unwrap_or("").to_string())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), UpgradeError> {
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .map_err(|e| UpgradeError::Message(format!("tar 失败：{e}")))?;
    if !status.success() {
        return Err(UpgradeError::Message("解包失败".into()));
    }
    Ok(())
}

#[allow(dead_code)]
fn write_stderr(msg: &str) {
    let _ = writeln!(io::stderr(), "{msg}");
}

#[allow(dead_code)]
fn _busy_hint(reasons: &[BusyReason]) -> String {
    reasons
        .iter()
        .map(|r| r.message())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert!(is_remote_newer("0.2.1", "0.2.2"));
        assert!(!is_remote_newer("0.2.1", "0.2.1"));
        assert!(!is_remote_newer("0.2.1", "0.1.9"));
        assert!(is_remote_newer("0.2.1", "v0.3.0"));
    }

    #[test]
    fn parse_tag_name_json() {
        let body = r#"{"url":"x","tag_name":"v0.2.1","prerelease":false}"#;
        assert_eq!(json_tag_name(body).as_deref(), Some("v0.2.1"));
    }

    #[test]
    fn check_mode_does_not_need_install_dir() {
        // 用环境变量短路远端，不触网、不改二进制。
        let _env = cordis_base::test_env::scoped().set("DOCK_UPGRADE_REMOTE_VERSION", "v0.0.1");
        let args = UpgradeArgs {
            mode: UpgradeMode::Check,
            force: false,
        };
        run_upgrade(&args).unwrap();
    }

    #[test]
    fn pre_without_prerelease_hard_fails() {
        let only_stable = r#"[
            {"tag_name":"v0.2.1","prerelease":false},
            {"tag_name":"v0.2.0","prerelease":false}
        ]"#;
        let err = pick_prerelease_tag(only_stable).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("prerelease"), "{msg}");
        assert!(msg.contains("拒绝回落"), "{msg}");
        assert!(msg.contains("DOCK_CHANNEL=pre"), "{msg}");
    }

    #[test]
    fn pre_picks_prerelease_tag() {
        let mixed = r#"[
            {"tag_name":"v0.3.0","prerelease":false},
            {"tag_name":"v0.3.0-rc.1","prerelease":true},
            {"tag_name":"v0.2.1","prerelease":false}
        ]"#;
        assert_eq!(pick_prerelease_tag(mixed).unwrap().as_str(), "v0.3.0-rc.1");
    }

    #[test]
    fn pre_channel_run_upgrade_fails_without_prerelease() {
        let only_stable = r#"[{"tag_name":"v0.2.1","prerelease":false}]"#;
        let _env = cordis_base::test_env::scoped()
            .set("DOCK_CHANNEL", "pre")
            .set("DOCK_UPGRADE_RELEASES_JSON", only_stable)
            .remove("DOCK_UPGRADE_REMOTE_VERSION")
            .remove("DOCK_VERSION");
        let err = run_upgrade(&UpgradeArgs {
            mode: UpgradeMode::Check,
            force: false,
        })
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("拒绝回落"), "{msg}");
    }
}
