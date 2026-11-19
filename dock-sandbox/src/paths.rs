//! 默认的路径表，和比较路径用的几个小工具。
//!
//! 表的取舍参考了 Grok（`xai-grok-sandbox` 的 `protected.rs`）与 Codex（可写根里的只读子路径）。

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// 每个工作区可写根里**永远只读**的路径。
///
/// 沙箱外的程序之后会照着它们跑代码：git 跑 `.git/hooks`、按 `.git/config` 里的
/// `core.hooksPath` / `core.fsmonitor` / 过滤器跑命令；Dock 下次启动按 `.dock/config.toml`
/// 拉起 MCP 服务器、装 `.dock/plugins`。让沙箱里的命令改它们，等于让它在沙箱外跑代码。
///
/// `.git` 下的两项只在 `.git` 已经存在时保护：还不是仓库的目录里 `git init` 要往
/// `.git/hooks` 写模板。`.dock` 不存在也保护，第一次创建同样要过权限门（Codex 同款）。
pub(crate) const GIT_PROTECTED: &[&str] = &["hooks", "config"];
pub(crate) const PROJECT_CONFIG_DIR: &str = ".dock";

/// 家目录下沙箱外的 shell / 工具每次启动都会执行或信任的文件（Grok `HOME_PROTECTED_FILES`）。
/// 只有家目录落在某个可写根里（在 `~` 里开 Dock、把 `~` 配成可写）时才真起作用。
pub(crate) const HOME_PROTECTED_FILES: &[&str] = &[
    ".bashrc",
    ".bash_aliases",
    ".bash_login",
    ".bash_logout",
    ".bash_profile",
    ".profile",
    ".pam_environment",
    ".zshenv",
    ".zprofile",
    ".zshrc",
    ".zlogin",
    ".zlogout",
    ".config/fish/config.fish",
    ".gitconfig",
    ".config/git/config",
    ".cargo/config.toml",
    ".cargo/config",
    ".cargo/env",
    ".gradle/init.gradle",
    ".gradle/gradle.properties",
    ".m2/settings.xml",
    ".pip/pip.conf",
    ".config/pip/pip.conf",
    ".yarnrc",
    ".yarnrc.yml",
    ".vimrc",
    ".tmux.conf",
];

/// 同上，目录（Grok `HOME_PROTECTED_DIRS`）：`PATH` 里的目录、登录时自动跑的东西。
pub(crate) const HOME_PROTECTED_DIRS: &[&str] = &[
    ".local/bin",
    ".cargo/bin",
    ".config/fish/conf.d",
    ".config/fish/functions",
    ".gradle/init.d",
    ".config/nvim",
    ".config/autostart",
    ".config/systemd/user",
    ".config/environment.d",
    "Library/LaunchAgents",
    "Library/LaunchDaemons",
];

/// 家目录下的密钥目录：读写都拒（Grok `SECRET_READ_DENY_DIRS`，加了几家云的凭据目录）。
pub(crate) const HOME_SECRET_DIRS: &[&str] = &[
    ".ssh",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".gnupg",
    ".kube",
    "Library/Keychains",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Firefox",
    ".config/google-chrome",
    ".config/chromium",
    ".mozilla",
];

/// 家目录下的凭据文件（Grok `SECRET_READ_DENY_FILES`）。
///
/// 没收 `.npmrc`：国内常在里面配镜像源，读不到会让 `npm install` 直接报错；
/// 里面存了 token 的用户自己加进 `deny_read`。
pub(crate) const HOME_SECRET_FILES: &[&str] = &[
    ".netrc",
    ".pypirc",
    ".docker/config.json",
    ".config/gh/hosts.yml",
    ".git-credentials",
    ".cargo/credentials.toml",
    ".cargo/credentials",
];

/// Dock 自己家目录（`$DOCK_HOME`）下的密钥：模型密钥、MCP 凭据、订阅登录、远程设备令牌、
/// 内置浏览器的登录态。
pub(crate) const DOCK_SECRETS: &[&str] = &[
    "secrets.json",
    "mcp_credentials.json",
    "credentials",
    "devices.json",
    "browser/user-data",
];

/// `$DOCK_HOME` 下沙箱里的命令可以写的目录：workflow 的 run 把报告写在这里。
pub(crate) const DOCK_SCRATCH: &str = "scratch";

/// 依赖缓存（`build_caches` 开着时可写）：拉依赖、构建要写它们（Grok 的
/// `BUILD_CACHE_TREES` 与 `VERIFIED_BUILD_CACHES` 合在一起）。
pub(crate) const HOME_BUILD_CACHES: &[&str] = &[
    ".cargo/registry",
    ".cargo/git",
    ".cargo/.package-cache",
    ".cargo/.package-cache-mutate",
    ".cargo/.global-cache",
    ".cargo/.global-cache-journal",
    ".npm",
    ".cache",
    "Library/Caches",
    ".pnpm-store",
    "go/pkg/mod",
    ".gradle/caches",
    ".m2/repository",
];

/// 容器引擎与会话总线的控制 socket：连上它就能让沙箱外的守护进程替你跑东西
/// （`docker run -v /:/host`、`systemd-run --user`）。家目录相对。
pub(crate) const HOME_CONTROL_SOCKETS: &[&str] = &[
    ".docker/run",
    ".colima",
    ".orbstack/run",
    ".rd/docker.sock",
    ".lima",
];

/// 同上，绝对路径。
pub(crate) const ABS_CONTROL_SOCKETS: &[&str] = &[
    "/var/run/docker.sock",
    "/run/docker.sock",
    "/run/containerd/containerd.sock",
    "/run/podman/podman.sock",
];

/// 同上，`$XDG_RUNTIME_DIR` 相对。
pub(crate) const RUNTIME_CONTROL_SOCKETS: &[&str] = &["docker.sock", "podman", "bus"];

/// 词法上去掉 `.` 与 `..`，不碰文件系统。
pub(crate) fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 尽量换成真实路径：最长的已存在前缀 `canonicalize`，后面还不存在的几段原样接上。
/// 相对路径先按进程 cwd 补成绝对路径。
///
/// 内核按真实路径判定（macOS 上 `/tmp` 是 `/private/tmp`），规则和比较都要用这一份。
pub fn real_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let path = lexical(&absolute);
    let mut existing = path.as_path();
    let mut rest: Vec<OsString> = Vec::new();
    loop {
        if let Ok(mut real) = std::fs::canonicalize(existing) {
            for seg in rest.iter().rev() {
                real.push(seg);
            }
            return real;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => return path,
        }
    }
}

/// `path` 是不是 `root` 本身或在它下面。逐段比较，`/ws/.gitx` 不在 `/ws/.git` 下。
///
/// macOS 的默认卷不分大小写，`.GIT/hooks` 和 `.git/hooks` 是同一个目录，所以那里按
/// 不分大小写比较（对保护 / 拒读是更严的一边）。
pub(crate) fn within(path: &Path, root: &Path) -> bool {
    let mut p = path.components();
    for r in root.components() {
        match p.next() {
            Some(c) if same_component(c.as_os_str(), r.as_os_str()) => {}
            _ => return false,
        }
    }
    true
}

fn same_component(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    if cfg!(target_os = "macos") {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// 去重（按 [`within`] 的相等），保持先后。顺带丢掉不是绝对路径的（策略里只该有绝对
/// 路径；空路径进了 SBPL 会让整份规则作废）。
pub(crate) fn dedup(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths.into_iter().filter(|p| p.is_absolute()) {
        if !out.iter().any(|q| within(&p, q) && within(q, &p)) {
            out.push(p);
        }
    }
    out
}

/// 一条路径和它的真实路径（不同时两条都要：拒读要两种写法都挡住）。
pub(crate) fn both_spellings(path: &Path) -> Vec<PathBuf> {
    let lex = lexical(path);
    let real = real_path(&lex);
    if real == lex {
        vec![lex]
    } else {
        vec![lex, real]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_drops_dots() {
        assert_eq!(lexical(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(lexical(Path::new("/a/b/../../..")), PathBuf::from("/.."));
    }

    #[test]
    fn within_is_component_wise() {
        assert!(within(
            Path::new("/ws/.git/hooks/x"),
            Path::new("/ws/.git/hooks")
        ));
        assert!(within(
            Path::new("/ws/.git/hooks"),
            Path::new("/ws/.git/hooks")
        ));
        assert!(!within(Path::new("/ws/.gitx"), Path::new("/ws/.git")));
        assert!(!within(Path::new("/ws"), Path::new("/ws/.git")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn within_ignores_case_on_macos() {
        assert!(within(
            Path::new("/ws/.GIT/Hooks/x"),
            Path::new("/ws/.git/hooks")
        ));
    }

    #[test]
    fn real_path_makes_relative_paths_absolute() {
        let cwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        assert_eq!(real_path(Path::new(".")), cwd);
        assert!(real_path(Path::new("no-such-dir/x")).is_absolute());
    }

    #[test]
    fn dedup_drops_relative_and_empty_paths() {
        let out = dedup(vec![
            PathBuf::new(),
            PathBuf::from("rel"),
            PathBuf::from("/a"),
        ]);
        assert_eq!(out, vec![PathBuf::from("/a")]);
    }

    #[test]
    fn real_path_keeps_missing_tail() {
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            real_path(&dir.path().join("no/such/file")),
            real.join("no/such/file")
        );
    }
}
