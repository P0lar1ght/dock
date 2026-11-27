//! 选后端、探测能不能用、把一条命令包起来。

use std::ffi::OsString;
use std::fmt;
use std::sync::OnceLock;

use crate::guard::Guard;
use crate::policy::Policy;

/// 落到内核上的方式。以后加 bubblewrap / 网络命名空间 / Windows 就在这里加一种。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// macOS：`sandbox-exec` + SBPL。
    Seatbelt,
    /// Linux：Landlock + seccomp。
    Landlock,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Seatbelt => "seatbelt",
            Backend::Landlock => "landlock",
        }
    }
}

/// 这台机器上的后端做不到的保证。展示给用户，不假装。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// 可写目录里的 `.git/hooks`、`.git/config`、`.dock` 内核挡不住，靠命令跑完比对还原。
    ProtectedByGuard,
    /// 连得上 Docker / 会话总线这类控制 socket（Landlock ABI < 9，Linux < 7.1）。
    UnixSockets,
    /// 能截断沙箱外、自己有权限写的文件（Landlock ABI < 3，Linux < 6.2）。
    Truncate,
    /// 读得到同一用户其它进程的环境变量（`/proc/<pid>/environ`，含 Dock 自己的）。
    ProcEnviron,
}

impl Limit {
    pub fn describe(self) -> &'static str {
        match self {
            Limit::ProtectedByGuard => {
                "工作区里的 .git/hooks、.git/config、.dock 内核挡不住，命令跑完比对、被改了就还原"
            }
            Limit::UnixSockets => "挡不住连 Docker、会话总线这类控制 socket（要 Linux 7.1+）",
            Limit::Truncate => "挡不住截断沙箱外的文件（要 Linux 6.2+）",
            Limit::ProcEnviron => {
                "读得到同一用户其它进程的环境变量（/proc/<pid>/environ），包括 Dock 自己的"
            }
        }
    }
}

/// 沙箱在这台机器上能不能用。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Ready {
        backend: Backend,
        limits: Vec<Limit>,
    },
    Unavailable {
        reason: String,
    },
}

impl Availability {
    pub fn is_ready(&self) -> bool {
        matches!(self, Availability::Ready { .. })
    }
}

/// 探测一次，进程内缓存。第一次调用会真起一个沙箱里的 `true` 试试。
pub fn availability() -> &'static Availability {
    static PROBED: OnceLock<Availability> = OnceLock::new();
    PROBED.get_or_init(probe)
}

/// 包不上的原因。调用方据此**不跑**这条命令。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrepareError(pub String);

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PrepareError {}

/// 包好的一条命令。[`Prepared::into_command`] 得到一个 `std::process::Command`，
/// 工作目录、stdio 由调用方设。
pub struct Prepared {
    program: OsString,
    args: Vec<OsString>,
    env_remove: Vec<OsString>,
    backend: Backend,
    guard: Option<Guard>,
    #[cfg(target_os = "linux")]
    child: Option<crate::linux::ChildSetup>,
}

impl Prepared {
    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// 命令和（需要的话）跑完要比对的快照。
    pub fn into_command(self) -> (std::process::Command, Option<Guard>) {
        let mut cmd = std::process::Command::new(&self.program);
        cmd.args(&self.args);
        for name in &self.env_remove {
            cmd.env_remove(name);
        }
        cmd.env("DOCK_SANDBOX", self.backend.name());
        #[cfg(target_os = "linux")]
        if let Some(child) = self.child {
            use std::os::unix::process::CommandExt;
            let child = std::sync::Mutex::new(child);
            // SAFETY: 闭包在 fork 之后、exec 之前运行，只做 landlock_restrict_self 与
            // seccomp 两次系统调用（规则在父进程里建好）；锁只在这个子进程里用一次，
            // 不会碰到父进程别的线程持有的锁。
            unsafe {
                cmd.pre_exec(move || match child.lock() {
                    Ok(mut setup) => setup.apply(),
                    Err(_) => Err(std::io::Error::from_raw_os_error(libc::EPERM)),
                });
            }
        }
        (cmd, self.guard)
    }
}

/// 按这台机器的后端把 `argv` 包进沙箱。后端不可用时报错（调用方别在沙箱外跑它）。
pub fn prepare(policy: &Policy, argv: &[OsString]) -> Result<Prepared, PrepareError> {
    match availability() {
        Availability::Ready { backend, .. } => prepare_with(*backend, policy, argv),
        Availability::Unavailable { reason } => Err(PrepareError(reason.clone())),
    }
}

fn prepare_with(
    backend: Backend,
    policy: &Policy,
    argv: &[OsString],
) -> Result<Prepared, PrepareError> {
    if argv.is_empty() {
        return Err(PrepareError("命令为空".into()));
    }
    let env_remove = policy.env_to_remove();
    match backend {
        #[cfg(target_os = "macos")]
        Backend::Seatbelt => {
            let (program, args) = crate::seatbelt::wrap(policy, argv);
            Ok(Prepared {
                program,
                args,
                env_remove,
                backend,
                guard: None,
            })
        }
        #[cfg(target_os = "linux")]
        Backend::Landlock => {
            let child = crate::linux::ChildSetup::build(policy).map_err(PrepareError)?;
            let (program, args) = crate::linux::wrap(argv);
            Ok(Prepared {
                program,
                args,
                env_remove,
                backend,
                guard: Some(Guard::capture(policy.protected_in_roots())),
                child: Some(child),
            })
        }
        #[allow(unreachable_patterns)]
        other => Err(PrepareError(format!(
            "{} 后端在这个系统上不可用",
            other.name()
        ))),
    }
}

#[cfg(target_os = "macos")]
fn probe() -> Availability {
    let exe = std::path::Path::new(crate::seatbelt::SANDBOX_EXEC);
    if !exe.is_file() {
        return Availability::Unavailable {
            reason: format!("找不到 {}", exe.display()),
        };
    }
    let out = std::process::Command::new(exe)
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();
    match out {
        Ok(out) if out.status.success() => Availability::Ready {
            backend: Backend::Seatbelt,
            limits: Vec::new(),
        },
        Ok(out) => Availability::Unavailable {
            reason: format!(
                "sandbox-exec 起不来（Dock 自己是不是已经在沙箱里？）：{}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        },
        Err(e) => Availability::Unavailable {
            reason: format!("sandbox-exec 起不来：{e}"),
        },
    }
}

#[cfg(target_os = "linux")]
fn probe() -> Availability {
    use crate::policy::{Access, Host, Settings};

    let abi = crate::linux::kernel_abi();
    if abi < 1 {
        return Availability::Unavailable {
            reason: "内核没有启用 Landlock（要 Linux 5.13+，且启动参数 lsm= 里有 landlock）".into(),
        };
    }
    if abi < crate::linux::MIN_ABI {
        return Availability::Unavailable {
            reason: format!(
                "内核的 Landlock 太旧（ABI {abi}，要 Linux 5.19+）：沙箱里跨目录改名会失败，git 用不了"
            ),
        };
    }
    // 真起一个试试：容器的 seccomp 配置可能不让装规则，那就别等到用户的命令才发现。
    let scratch = std::env::temp_dir();
    let host = Host {
        home: None,
        dock_home: scratch.join(".dock-sandbox-probe"),
        tmp_dirs: Vec::new(),
        cargo_target_dir: None,
        runtime_dir: None,
    };
    let settings = Settings {
        network: false,
        ..Settings::default()
    };
    let policy = Policy::build(Access::ReadOnly, &scratch, &settings, &host);
    let argv: Vec<OsString> = ["/bin/sh", "-c", "exit 0"].map(OsString::from).to_vec();
    let result = prepare_with(Backend::Landlock, &policy, &argv).and_then(|prepared| {
        let (mut cmd, _) = prepared.into_command();
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| PrepareError(e.to_string()))
            .and_then(|st| {
                if st.success() {
                    Ok(())
                } else {
                    Err(PrepareError(format!("试跑退出 {st}")))
                }
            })
    });
    match result {
        Ok(()) => Availability::Ready {
            backend: Backend::Landlock,
            limits: crate::linux::limits(abi),
        },
        Err(e) => Availability::Unavailable {
            reason: format!("装 Landlock / seccomp 规则失败：{e}"),
        },
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn probe() -> Availability {
    Availability::Unavailable {
        reason: format!("{} 上还没有沙箱后端", std::env::consts::OS),
    }
}
