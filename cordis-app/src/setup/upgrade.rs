//! `dock upgrade` / `dock update --check`：复用 install.sh 的原子替换路径。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::busy::busy_reasons;

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

    let newer = is_remote_newer(current, remote_pretty)?;
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

/// 按 semver 优先级比较（同一 X.Y.Z 下正式版高于预发布；build 元数据不参与）。
/// 版本号解析不了就报错，不猜：猜错的代价是装上一个不该装的包。
fn is_remote_newer(current: &str, remote: &str) -> Result<bool, UpgradeError> {
    let parse = |v: &str| {
        Version::parse(v.trim().trim_start_matches('v'))
            .map_err(|e| UpgradeError::Message(format!("版本号无法解析：{v}（{e}）")))
    };
    Ok(parse(remote)?.cmp_precedence(&parse(current)?).is_gt())
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

/// GitHub release 里我们关心的字段。
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

fn fetch_github_version(channel: Channel) -> Result<String, UpgradeError> {
    match channel {
        Channel::Stable => {
            let body = http_get(&format!(
                "https://api.github.com/repos/{REPO}/releases/latest"
            ))?;
            let release: Release = serde_json::from_str(&body).map_err(|e| {
                UpgradeError::Message(format!("无法解析 GitHub latest release：{e}"))
            })?;
            Ok(release.tag_name)
        }
        Channel::Pre => {
            let body = releases_list_body()?;
            pick_prerelease_tag(&body)
        }
    }
}

/// `DOCK_CHANNEL=pre`：从 releases 列表里挑 semver 最高的 prerelease（列表按创建时间排，
/// 不按版本）；没有则硬失败，禁止回落正式版。
fn pick_prerelease_tag(body: &str) -> Result<String, UpgradeError> {
    let releases: Vec<Release> = serde_json::from_str(body)
        .map_err(|e| UpgradeError::Message(format!("无法解析 GitHub releases 列表：{e}")))?;
    releases
        .into_iter()
        .filter(|r| r.prerelease && !r.draft)
        .filter_map(|r| {
            let version = Version::parse(r.tag_name.trim_start_matches('v')).ok()?;
            Some((version, r.tag_name))
        })
        .max_by(|(a, _), (b, _)| a.cmp_precedence(b))
        .map(|(_, tag)| tag)
        .ok_or_else(|| {
            UpgradeError::Message(
                "DOCK_CHANNEL=pre，但 GitHub 上没有可用的 prerelease；拒绝回落正式版。请改用 DOCK_CHANNEL=stable，或发布一个 prerelease 后再试。".into(),
            )
        })
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
    // 与 install.sh 一致：走 curl，不给 cordis-app 再挂一套 HTTP 客户端。
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
    // 私有临时目录（随机名、0700、drop 即清理）：共享 /tmp 里没人能抢先占位，
    // 再在校验与解包之间换包；出错路径也不留残留。
    let tmp = tempfile::Builder::new().prefix("dock-upgrade-").tempdir()?;
    let tmp = tmp.path();
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
        .to_ascii_lowercase();
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

    extract_tar_gz(&archive, tmp)?;
    let bin = tmp.join(ASSET);
    if !bin.is_file() {
        return Err(UpgradeError::Message(format!("产物里没有 {ASSET} 二进制")));
    }

    // 先写同目录的随机临时名再 rename 覆盖：旧 dock 可能正在跑，直接写会被 Linux 拒成
    // ETXTBSY；rename(2) 只换目录项。任何一步失败，临时文件随 drop 删掉。
    fs::create_dir_all(install_dir)?;
    let mut staged = tempfile::Builder::new()
        .prefix(&format!(".{ASSET}.new."))
        .tempfile_in(install_dir)?;
    io::copy(&mut fs::File::open(&bin)?, staged.as_file_mut())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staged
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    let final_path = install_dir.join(ASSET);
    staged.persist(&final_path).map_err(|e| {
        UpgradeError::Message(format!("无法替换 {}：{}", final_path.display(), e.error))
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
    warn_if_not_running_binary(&final_path);
    Ok(())
}

/// 装到的不是当前在跑的这份（比如 cargo install 到 ~/.cargo/bin）：「升级完成」之后
/// 用户敲 dock 仍是旧版，必须说清楚。
fn warn_if_not_running_binary(installed: &Path) {
    let Ok(current) = std::env::current_exe().and_then(fs::canonicalize) else {
        return;
    };
    let Ok(installed) = fs::canonicalize(installed) else {
        return;
    };
    if current != installed {
        eprintln!(
            "注意：当前运行的是 {}，新版装到了 {}。用 DOCK_INSTALL_DIR 指向前者所在目录，或调整 PATH 让后者先命中。",
            current.display(),
            installed.display()
        );
    }
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
    let mut hasher = Sha256::new();
    io::copy(&mut fs::File::open(path)?, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        let newer = |c, r| is_remote_newer(c, r).unwrap();
        assert!(newer("0.2.1", "0.2.2"));
        assert!(!newer("0.2.1", "0.2.1"));
        assert!(!newer("0.2.1", "0.1.9"));
        assert!(newer("0.2.1", "v0.3.0"));
        assert!(newer("0.9.0", "0.10.0"));
    }

    #[test]
    fn semver_compare_prerelease() {
        let newer = |c, r| is_remote_newer(c, r).unwrap();
        // 同一 X.Y.Z 的预发布比正式版旧：不能把 0.3.0 「升」成 0.3.0-rc.1。
        assert!(!newer("0.3.0", "v0.3.0-rc.1"));
        assert!(newer("0.3.0-rc.1", "v0.3.0"));
        assert!(newer("0.2.1", "v0.3.0-rc.1"));
        assert!(!newer("0.3.1", "v0.3.0-rc.1"));
        assert!(newer("0.3.0-rc.2", "v0.3.0-rc.10"));
        assert!(newer("0.3.0-alpha", "v0.3.0-beta"));
        assert!(!newer("0.3.0-rc.1", "v0.3.0-rc.1"));
    }

    #[test]
    fn unparsable_remote_version_is_an_error() {
        // 原先解析不了就按「字符串不同 = 更新」去装。
        let err = is_remote_newer("0.2.1", "v0.3").unwrap_err();
        assert!(err.to_string().contains("版本号无法解析"), "{err}");
    }

    #[test]
    fn latest_release_parses_pretty_json_with_nested_objects() {
        // GitHub 的真实响应是带缩进的，author / assets 里还嵌套对象。
        let body = r#"{
          "url": "x",
          "author": { "login": "P0lar1ght", "tag_name": "not-this" },
          "tag_name": "v0.2.1",
          "prerelease": false
        }"#;
        let release: Release = serde_json::from_str(body).unwrap();
        assert_eq!(release.tag_name, "v0.2.1");
    }

    #[test]
    fn sha256_matches_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abc");
        fs::write(&path, "abc").unwrap();
        assert_eq!(
            sha256_of(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// 造一个 file:// 发布目录：tar.gz 里是会回 `--version` 的假 dock，外加 sha256。
    #[cfg(unix)]
    fn fake_release(dir: &Path, target: &str, version_line: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let stage = dir.join("stage");
        fs::create_dir_all(&stage).unwrap();
        let bin = stage.join(ASSET);
        fs::write(&bin, format!("#!/bin/sh\necho '{version_line}'\n")).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let file = format!("{ASSET}-{target}.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(dir.join(&file))
            .arg("-C")
            .arg(&stage)
            .arg(ASSET)
            .status()
            .unwrap();
        assert!(status.success());
        let sum = sha256_of(&dir.join(&file)).unwrap();
        fs::write(
            dir.join(format!("{file}.sha256")),
            format!("{sum}  {file}\n"),
        )
        .unwrap();
        format!("file://{}", dir.display())
    }

    #[cfg(unix)]
    #[test]
    fn atomic_install_replaces_binary_and_leaves_no_staging_file() {
        let release = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        let base = fake_release(release.path(), "t", "dock 9.9.9");
        fs::write(install.path().join(ASSET), "old").unwrap();
        let _env = cordis_base::test_env::scoped().set("DOCK_BASE_URL", &base);

        atomic_install("v9.9.9", "t", install.path()).unwrap();

        let out = Command::new(install.path().join(ASSET))
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "dock 9.9.9");
        let names: Vec<_> = fs::read_dir(install.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(ASSET)]);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_install_refuses_bad_checksum_and_keeps_old_binary() {
        let release = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        let base = fake_release(release.path(), "t", "dock 9.9.9");
        fs::write(
            release.path().join(format!("{ASSET}-t.tar.gz.sha256")),
            "00",
        )
        .unwrap();
        fs::write(install.path().join(ASSET), "old").unwrap();
        let _env = cordis_base::test_env::scoped().set("DOCK_BASE_URL", &base);

        let err = atomic_install("v9.9.9", "t", install.path()).unwrap_err();

        assert!(err.to_string().contains("校验失败"), "{err}");
        assert_eq!(
            fs::read_to_string(install.path().join(ASSET)).unwrap(),
            "old"
        );
        assert_eq!(fs::read_dir(install.path()).unwrap().count(), 1);
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
    fn pre_picks_highest_prerelease_not_first_listed_and_skips_drafts() {
        // 列表按创建时间排：补发的旧 rc 可能排在前面。
        let list = r#"[
            {"tag_name":"v0.3.0-rc.1","prerelease":true},
            {"tag_name":"v0.4.0-rc.1","prerelease":true,"draft":true},
            {"tag_name":"v0.3.0-rc.10","prerelease":true},
            {"tag_name":"v0.3.0-rc.2","prerelease":true}
        ]"#;
        assert_eq!(pick_prerelease_tag(list).unwrap().as_str(), "v0.3.0-rc.10");
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
