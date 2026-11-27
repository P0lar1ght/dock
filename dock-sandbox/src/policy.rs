//! 一条命令的沙箱策略：能写哪、哪些在可写目录里也只读、哪些读不到、能不能联网。
//!
//! 策略是纯数据，后端（Seatbelt / Landlock）把它落到内核上；Dock 进程内的文件工具
//! （`write_file` 等）用同一份 [`Policy::allows_write`] / [`Policy::denies_read`] 判断，
//! 两边的边界一致。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::paths::{self, both_spellings, dedup, real_path, within};

/// 用户能调的几项（来自 `config.toml` 的 `[sandbox]`，由调用方解析好传进来）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// 沙箱里的命令能不能联网。
    pub network: bool,
    /// 联网时只许连本机（回环）：出网只能经本机上的审计代理，直连远端连不上。
    /// 本机的开发服务器照常连得上、照常能监听。
    pub proxy_only: bool,
    /// 依赖缓存（`~/.cargo/registry`、`~/.npm`、`~/.cache`…）可不可写。
    pub build_caches: bool,
    /// 额外可写的目录（绝对路径）。
    pub writable_roots: Vec<PathBuf>,
    /// 额外读不到的路径（绝对路径）。
    pub deny_read: Vec<PathBuf>,
    /// 默认会从命令环境里去掉的变量里，要留下的那些（变量名，精确匹配）。
    pub keep_env: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            network: true,
            proxy_only: false,
            build_caches: true,
            writable_roots: Vec::new(),
            deny_read: Vec::new(),
            keep_env: Vec::new(),
        }
    }
}

/// 这条命令按哪一档跑。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// 工作区（及临时目录、依赖缓存、配置的额外目录）可写。
    WorkspaceWrite,
    /// 只读场景（计划模式、只读子代理）：只有临时目录可写，不联网。
    ReadOnly,
}

/// 宿主环境里策略要用到的几个位置。[`Host::detect`] 从进程环境读；测试直接构造。
#[derive(Clone, Debug, Default)]
pub struct Host {
    /// 用户家目录。没有时家目录相对的表全部跳过。
    pub home: Option<PathBuf>,
    /// Dock 自己的家目录（`$DOCK_HOME`，默认 `~/.dock`）。
    pub dock_home: PathBuf,
    /// 临时目录：`/tmp`、`/var/tmp`、`$TMPDIR`。
    pub tmp_dirs: Vec<PathBuf>,
    /// `$CARGO_TARGET_DIR`：几个工作树共用一个构建目录时它在工作区外面，不放行 cargo 就编不了。
    pub cargo_target_dir: Option<PathBuf>,
    /// `$XDG_RUNTIME_DIR`（Linux 的会话总线、rootless 容器 socket 在这里）。
    pub runtime_dir: Option<PathBuf>,
}

impl Host {
    pub fn detect(dock_home: PathBuf) -> Self {
        let env_path = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        let mut tmp_dirs = vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")];
        if let Some(tmp) = env_path("TMPDIR") {
            tmp_dirs.push(tmp);
        }
        tmp_dirs.retain(|p| p.is_dir());
        Self {
            home: env_path("HOME"),
            dock_home,
            tmp_dirs,
            cargo_target_dir: env_path("CARGO_TARGET_DIR"),
            runtime_dir: env_path("XDG_RUNTIME_DIR"),
        }
    }
}

/// 一条命令的策略。用 [`Policy::build`] 生成。
#[derive(Clone, Debug)]
pub struct Policy {
    access: Access,
    workspace: PathBuf,
    write_roots: Vec<PathBuf>,
    protected: Vec<PathBuf>,
    deny: Vec<PathBuf>,
    sockets: Vec<PathBuf>,
    network: bool,
    proxy_only: bool,
    keep_env: Vec<String>,
}

impl Policy {
    /// `workspace`：这一页的工作目录（会话 cwd）。
    pub fn build(access: Access, workspace: &Path, settings: &Settings, host: &Host) -> Self {
        let workspace = real_path(workspace);
        let home = host.home.as_deref();

        // 「项目」可写根：要护住里面的 .git / .dock 的那些。临时目录、缓存不算。
        let mut project_roots = Vec::new();
        let mut roots: Vec<PathBuf> = host.tmp_dirs.iter().map(|p| real_path(p)).collect();
        if access == Access::WorkspaceWrite {
            project_roots.push(workspace.clone());
            project_roots.extend(settings.writable_roots.iter().map(|p| real_path(p)));
            roots.extend(project_roots.iter().cloned());
            roots.push(real_path(&host.dock_home.join(paths::DOCK_SCRATCH)));
            if let Some(target) = &host.cargo_target_dir {
                roots.push(real_path(target));
            }
            if settings.build_caches {
                if let Some(home) = home {
                    roots.extend(
                        paths::HOME_BUILD_CACHES
                            .iter()
                            .map(|rel| real_path(&home.join(rel))),
                    );
                }
            }
        }
        let write_roots = dedup(roots);

        let mut protected = Vec::new();
        for root in &project_roots {
            protect_project(root, &mut protected);
        }
        protect_dock_home(&host.dock_home, &mut protected);
        if let Some(home) = home {
            protected.extend(paths::HOME_PROTECTED_FILES.iter().map(|r| home.join(r)));
            protected.extend(paths::HOME_PROTECTED_DIRS.iter().map(|r| home.join(r)));
        }
        let protected = dedup(protected.iter().map(|p| real_path(p)).collect());

        let mut deny = Vec::new();
        if let Some(home) = home {
            for rel in paths::HOME_SECRET_DIRS
                .iter()
                .chain(paths::HOME_SECRET_FILES)
            {
                deny.extend(both_spellings(&home.join(rel)));
            }
        }
        for rel in paths::DOCK_SECRETS {
            deny.extend(both_spellings(&host.dock_home.join(rel)));
        }
        for extra in &settings.deny_read {
            deny.extend(both_spellings(extra));
        }
        let deny = dedup(deny);

        let mut sockets = Vec::new();
        for abs in paths::ABS_CONTROL_SOCKETS {
            sockets.extend(both_spellings(Path::new(abs)));
        }
        if let Some(home) = home {
            for rel in paths::HOME_CONTROL_SOCKETS {
                sockets.extend(both_spellings(&home.join(rel)));
            }
        }
        if let Some(run) = &host.runtime_dir {
            for rel in paths::RUNTIME_CONTROL_SOCKETS {
                sockets.extend(both_spellings(&run.join(rel)));
            }
        }
        let sockets = dedup(sockets);

        Self {
            access,
            workspace,
            write_roots,
            protected,
            deny,
            sockets,
            network: access == Access::WorkspaceWrite && settings.network,
            proxy_only: settings.proxy_only,
            keep_env: settings.keep_env.clone(),
        }
    }

    pub fn access(&self) -> Access {
        self.access
    }

    /// 工作区（真实路径）。
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// 可写根（真实路径）。
    pub fn write_roots(&self) -> &[PathBuf] {
        &self.write_roots
    }

    /// 可写根里也写不了的路径（真实路径）。
    pub fn protected(&self) -> &[PathBuf] {
        &self.protected
    }

    /// 读写都拒的路径（两种写法都在）。
    pub fn deny(&self) -> &[PathBuf] {
        &self.deny
    }

    /// 连不得的控制 socket。
    pub fn sockets(&self) -> &[PathBuf] {
        &self.sockets
    }

    pub fn network(&self) -> bool {
        self.network
    }

    /// 联网时只许连本机（见 [`Settings::proxy_only`]）。
    pub fn proxy_only(&self) -> bool {
        self.network && self.proxy_only
    }

    pub(crate) fn keep_env(&self) -> &[String] {
        &self.keep_env
    }

    /// 这条路径（可以还不存在）沙箱里能不能写。进程内的写文件工具用它。
    pub fn allows_write(&self, path: &Path) -> bool {
        let real = real_path(path);
        self.write_roots.iter().any(|root| within(&real, root))
            && !self.protected.iter().any(|p| within(&real, p))
            && !self.denies_read(path)
    }

    /// 这条路径是不是在拒读名单里（按原写法和真实路径各查一次）。
    pub fn denies_read(&self, path: &Path) -> bool {
        both_spellings(path)
            .iter()
            .any(|p| self.deny.iter().any(|d| within(p, d)))
    }

    /// 受保护路径里落在某个可写根下的那些：只有它们需要 Landlock 那边的事后比对。
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn protected_in_roots(&self) -> Vec<PathBuf> {
        self.protected
            .iter()
            .filter(|p| self.write_roots.iter().any(|root| within(p, root)))
            .cloned()
            .collect()
    }

    /// 要从命令环境里去掉的变量名。
    pub(crate) fn env_to_remove(&self) -> Vec<OsString> {
        crate::env::scrubbed(std::env::vars_os().map(|(k, _)| k), self.keep_env())
    }
}

/// 一个项目可写根里要护住的东西。
fn protect_project(root: &Path, out: &mut Vec<PathBuf>) {
    let git = root.join(".git");
    match std::fs::symlink_metadata(&git) {
        // 工作树 / 子模块的 `.git` 是一个指针文件：改了它，git 就会去读别处的 hooks 与 config。
        Ok(meta) if !meta.is_dir() => out.push(git),
        Ok(_) => {
            out.extend(paths::GIT_PROTECTED.iter().map(|n| git.join(n)));
            protect_submodules(&git.join("modules"), 0, out);
        }
        Err(_) => {}
    }
    out.push(root.join(paths::PROJECT_CONFIG_DIR));
}

/// `.git/modules/**` 里每个子模块自己的 hooks 与 config。
fn protect_submodules(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        if path.join("config").exists() || path.join("hooks").exists() {
            out.extend(paths::GIT_PROTECTED.iter().map(|n| path.join(n)));
        }
        protect_submodules(&path, depth + 1, out);
    }
}

/// `$DOCK_HOME` 里除了 `scratch` 都护住：配置、插件、技能、会话。
/// 只有它落在某个可写根里（在 `~` 里开 Dock）时才真起作用。
fn protect_dock_home(dock_home: &Path, out: &mut Vec<PathBuf>) {
    out.push(dock_home.join("config.toml"));
    if let Ok(entries) = std::fs::read_dir(dock_home) {
        for entry in entries.flatten() {
            if entry.file_name() != paths::DOCK_SCRATCH {
                out.push(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        ws: PathBuf,
        home: PathBuf,
        host: Host,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = root.join("ws");
        let home = root.join("home");
        let tmp = root.join("tmp");
        std::fs::create_dir_all(ws.join(".git/hooks")).unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join(".dock/scratch")).unwrap();
        std::fs::create_dir_all(&tmp).unwrap();
        let host = Host {
            home: Some(home.clone()),
            dock_home: home.join(".dock"),
            tmp_dirs: vec![tmp],
            cargo_target_dir: None,
            runtime_dir: None,
        };
        Fixture {
            _dir: dir,
            root,
            ws,
            home,
            host,
        }
    }

    #[test]
    fn workspace_is_writable_but_git_metadata_and_dock_config_are_not() {
        let f = fixture();
        let p = Policy::build(Access::WorkspaceWrite, &f.ws, &Settings::default(), &f.host);
        assert!(p.allows_write(&f.ws.join("src/main.rs")));
        assert!(p.allows_write(&f.ws.join(".git/index")));
        assert!(!p.allows_write(&f.ws.join(".git/hooks/pre-commit")));
        assert!(!p.allows_write(&f.ws.join(".git/config")));
        assert!(!p.allows_write(&f.ws.join(".dock/config.toml")));
        assert!(!p.allows_write(&f.root.join("elsewhere/file")));
        assert!(p.allows_write(&f.root.join("tmp/x")));
        assert!(p.allows_write(&f.home.join(".dock/scratch/run/report.md")));
        assert!(!p.allows_write(&f.home.join(".dock/config.toml")));
    }

    #[test]
    fn git_parts_are_only_protected_once_the_repo_exists() {
        let f = fixture();
        let plain = f.root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let p = Policy::build(
            Access::WorkspaceWrite,
            &plain,
            &Settings::default(),
            &f.host,
        );
        // 还不是仓库：`git init` 要往 .git/hooks 写模板。
        assert!(p.allows_write(&plain.join(".git/hooks/pre-commit.sample")));
        // .dock 不存在也护住，第一次创建要过权限门。
        assert!(!p.allows_write(&plain.join(".dock/config.toml")));
    }

    #[test]
    fn a_worktree_pointer_file_is_protected() {
        let f = fixture();
        let wt = f.root.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: /elsewhere\n").unwrap();
        let p = Policy::build(Access::WorkspaceWrite, &wt, &Settings::default(), &f.host);
        assert!(!p.allows_write(&wt.join(".git")));
        assert!(p.allows_write(&wt.join("src.rs")));
    }

    #[test]
    fn secrets_are_denied_even_inside_a_writable_root() {
        let f = fixture();
        let settings = Settings {
            writable_roots: vec![f.home.clone()],
            ..Settings::default()
        };
        let p = Policy::build(Access::WorkspaceWrite, &f.ws, &settings, &f.host);
        assert!(p.denies_read(&f.home.join(".ssh/id_ed25519")));
        assert!(p.denies_read(&f.home.join(".dock/secrets.json")));
        assert!(p.denies_read(&f.home.join(".dock/audit/ca/ca.key.pem")));
        assert!(
            !p.denies_read(&f.home.join(".dock/audit/ca/bundle.pem")),
            "证书包命令要读"
        );
        assert!(!p.allows_write(&f.home.join(".ssh/authorized_keys")));
        assert!(!p.allows_write(&f.home.join(".zshrc")));
        assert!(p.allows_write(&f.home.join("notes.txt")));
        assert!(!p.denies_read(&f.home.join("notes.txt")));
    }

    #[test]
    fn a_symlink_into_a_secret_is_denied_by_its_target() {
        let f = fixture();
        std::fs::write(f.home.join(".ssh/id_ed25519"), "k").unwrap();
        let link = f.ws.join("key");
        std::os::unix::fs::symlink(f.home.join(".ssh/id_ed25519"), &link).unwrap();
        let p = Policy::build(Access::WorkspaceWrite, &f.ws, &Settings::default(), &f.host);
        assert!(p.denies_read(&link));
        assert!(!p.allows_write(&link));
    }

    #[test]
    fn read_only_writes_only_temp_and_has_no_network() {
        let f = fixture();
        let p = Policy::build(Access::ReadOnly, &f.ws, &Settings::default(), &f.host);
        assert!(!p.allows_write(&f.ws.join("src/main.rs")));
        assert!(p.allows_write(&f.root.join("tmp/x")));
        assert!(!p.network());
    }

    #[test]
    fn extra_settings_reach_the_policy() {
        let f = fixture();
        let extra = f.root.join("target-dir");
        let settings = Settings {
            network: false,
            proxy_only: false,
            build_caches: false,
            writable_roots: vec![extra.clone()],
            deny_read: vec![f.ws.join(".env")],
            keep_env: vec![],
        };
        let p = Policy::build(Access::WorkspaceWrite, &f.ws, &settings, &f.host);
        assert!(!p.network());
        assert!(p.allows_write(&extra.join("debug/build")));
        assert!(!p.allows_write(&f.home.join(".cargo/registry/x")));
        assert!(p.denies_read(&f.ws.join(".env")));
        assert!(!p.allows_write(&f.ws.join(".env")));
    }

    #[test]
    fn build_caches_are_writable_by_default() {
        let f = fixture();
        let p = Policy::build(Access::WorkspaceWrite, &f.ws, &Settings::default(), &f.host);
        assert!(p.allows_write(&f.home.join(".cargo/registry/cache/x.crate")));
        assert!(!p.allows_write(&f.home.join(".cargo/config.toml")));
        assert!(!p.allows_write(&f.home.join(".cargo/bin/cargo")));
    }
}
