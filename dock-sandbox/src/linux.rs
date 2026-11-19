//! Linux 后端：Landlock 管文件，seccomp 管网络与几个越狱用的系统调用。
//!
//! 规则在父进程（Dock）里建好，子进程 fork 之后、exec 之前只做两次系统调用把它们装上
//! （[`ChildSetup::apply`]），不在那里分配内存：Dock 是多线程的，fork 出来的子进程里
//! malloc 可能撞上别的线程留下的锁。
//!
//! 和 Codex 的旧 Landlock 后端、Grok 的 nono 一样：不需要装任何东西，不需要 user namespace。
//! 内核挡不住的部分（可写根里的 `.git/hooks` 等）交给 [`crate::Guard`] 事后还原。

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use landlock::{
    Access, AccessFs, BitFlags, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreated, RulesetCreatedAttr, RulesetStatus, Scope, ABI,
};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

use crate::policy::Policy;
use crate::tree::tree_minus;
use crate::Limit;

/// 认得的最高 ABI。内核没有的部分按 best-effort 丢掉，丢掉了什么由 [`limits`] 报告。
const TARGET_ABI: ABI = ABI::V9;

/// 最低要求：ABI 1（Linux 5.13–5.18）下跨目录的 rename / link 一律被拒，git 写对象就要
/// 跨目录改名，沙箱里 `git commit` 会失败。宁可报「不可用」退回权限门。
pub(crate) const MIN_ABI: i32 = 2;

/// 要能写的设备节点。
const DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
    "/dev/ptmx",
    "/dev/pts",
    "/dev/shm",
];

/// 内核的 Landlock ABI 版本；没有 Landlock 时 ≤ 0。
pub(crate) fn kernel_abi() -> i32 {
    // landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)
    let ret = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            1u32,
        )
    };
    ret as i32
}

/// 这个内核上做不到的保证。
pub(crate) fn limits(abi: i32) -> Vec<Limit> {
    let mut out = vec![Limit::ProtectedByGuard, Limit::ProcEnviron];
    if abi < 3 {
        out.push(Limit::Truncate);
    }
    if abi < 9 {
        out.push(Limit::UnixSockets);
    }
    out
}

/// 子进程里要装的东西。
pub(crate) struct ChildSetup {
    ruleset: Option<RulesetCreated>,
    seccomp: BpfProgram,
}

impl ChildSetup {
    pub(crate) fn build(policy: &Policy) -> Result<Self, String> {
        Ok(Self {
            ruleset: Some(build_ruleset(policy)?),
            seccomp: build_seccomp(policy.network())?,
        })
    }

    /// fork 之后、exec 之前在子进程里跑。失败就返回错误，`spawn` 随之失败，命令不会跑。
    pub(crate) fn apply(&mut self) -> io::Result<()> {
        let Some(ruleset) = self.ruleset.take() else {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        };
        match ruleset.restrict_self() {
            Ok(status) if status.ruleset != RulesetStatus::NotEnforced => {}
            _ => return Err(io::Error::from_raw_os_error(libc::EPERM)),
        }
        seccompiler::apply_filter(&self.seccomp)
            .map_err(|_| io::Error::from_raw_os_error(libc::EPERM))
    }
}

fn build_ruleset(policy: &Policy) -> Result<RulesetCreated, String> {
    let all = AccessFs::from_all(TARGET_ABI);
    // 连 Unix socket（ABI 9）算读：读得到的地方就连得上，控制 socket 作为洞挖掉。
    let read = AccessFs::from_read(TARGET_ABI) | AccessFs::ResolveUnix;
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(all)
        .and_then(|r| r.scope(Scope::from_all(TARGET_ABI)))
        .and_then(|r| r.create())
        .map_err(|e| format!("Landlock 规则集建不起来：{e}"))?
        .no_new_privs(true);

    // 处处能列目录（拒读的目录也只看得到名字，读不到内容）。
    ruleset = add(ruleset, Path::new("/"), AccessFs::ReadDir.into())?;

    let holes: Vec<PathBuf> = policy
        .deny()
        .iter()
        .chain(policy.sockets())
        .cloned()
        .collect();
    for path in tree_minus(Path::new("/"), &holes) {
        ruleset = add(ruleset, &path, read)?;
    }
    for root in policy.write_roots() {
        for path in tree_minus(root, &holes) {
            ruleset = add(ruleset, &path, all)?;
        }
    }
    for dev in DEVICES {
        ruleset = add(ruleset, Path::new(dev), all)?;
    }
    Ok(ruleset)
}

/// 加一条规则；路径不在了（或打不开）就跳过：没有的东西不需要放行。
fn add(
    ruleset: RulesetCreated,
    path: &Path,
    access: BitFlags<AccessFs>,
) -> Result<RulesetCreated, String> {
    let Ok(fd) = PathFd::new(path) else {
        return Ok(ruleset);
    };
    let is_dir = std::fs::metadata(path).is_ok_and(|m| m.is_dir());
    let access = if is_dir {
        access
    } else {
        access & AccessFs::from_file(TARGET_ABI)
    };
    if access.is_empty() {
        return Ok(ruleset);
    }
    ruleset
        .add_rule(PathBeneath::new(fd, access))
        .map_err(|e| format!("Landlock 规则加不上（{}）：{e}", path.display()))
}

fn target_arch() -> Result<TargetArch, String> {
    if cfg!(target_arch = "x86_64") {
        Ok(TargetArch::x86_64)
    } else if cfg!(target_arch = "aarch64") {
        Ok(TargetArch::aarch64)
    } else {
        Err(format!(
            "这个 CPU 架构（{}）还不支持 seccomp 规则",
            std::env::consts::ARCH
        ))
    }
}

/// 一直拒：ptrace / 读写别的进程内存（同用户的沙箱外进程也算），io_uring（能绕开 socket
/// 规则建连接）。断网时再拒掉除 Unix socket 以外的一切 socket（Codex 同款）。
fn build_seccomp(network: bool) -> Result<BpfProgram, String> {
    let mut rules: std::collections::BTreeMap<i64, Vec<SeccompRule>> = Default::default();
    for nr in [
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
    ] {
        rules.insert(nr, vec![]);
    }
    if !network {
        let not_unix = || {
            SeccompCondition::new(
                0,
                SeccompCmpArgLen::Dword,
                SeccompCmpOp::Ne,
                libc::AF_UNIX as u64,
            )
            .and_then(|c| SeccompRule::new(vec![c]))
            .map_err(|e| e.to_string())
        };
        rules.insert(libc::SYS_socket, vec![not_unix()?]);
        rules.insert(libc::SYS_socketpair, vec![not_unix()?]);
    }
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        target_arch()?,
    )
    .map_err(|e| format!("seccomp 规则建不起来：{e}"))?;
    filter
        .try_into()
        .map_err(|e: seccompiler::BackendError| format!("seccomp 规则编译失败：{e}"))
}

/// 拼命令：程序不变，规则在子进程里装。
pub(crate) fn wrap(argv: &[OsString]) -> (OsString, Vec<OsString>) {
    (argv[0].clone(), argv[1..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    /// 只装 seccomp（不装 Landlock）跑一条 bash：没有 Landlock 的内核（Docker Desktop）上
    /// 也能验证网络规则。
    fn connect_with_seccomp(network: bool, port: u16) -> std::process::Output {
        let prog = build_seccomp(network).expect("规则编得出来");
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg(format!(
            "exec 3<>/dev/tcp/127.0.0.1/{port} && echo connected"
        ));
        unsafe {
            cmd.pre_exec(move || {
                seccompiler::apply_filter(&prog)
                    .map_err(|_| io::Error::from_raw_os_error(libc::EPERM))
            });
        }
        cmd.output().expect("起得来")
    }

    #[test]
    fn seccomp_blocks_ip_sockets_only_when_offline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let on = connect_with_seccomp(true, port);
        assert!(
            String::from_utf8_lossy(&on.stdout).contains("connected"),
            "{on:?}"
        );
        let off = connect_with_seccomp(false, port);
        assert!(!off.status.success(), "{off:?}");
    }

    /// 断网只拒 IP socket：AF_UNIX 照常建得起来（本机进程间通信要用）。在装完规则的子进程里
    /// 直接调一次 socket(AF_UNIX)，建不起来 spawn 就失败。
    #[test]
    fn unix_sockets_still_work_offline() {
        let prog = build_seccomp(false).unwrap();
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg("exit 0");
        unsafe {
            cmd.pre_exec(move || {
                seccompiler::apply_filter(&prog)
                    .map_err(|_| io::Error::from_raw_os_error(libc::EPERM))?;
                let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                libc::close(fd);
                Ok(())
            });
        }
        let st = cmd.status().expect("AF_UNIX socket 断网时也要建得起来");
        assert!(st.success());
    }

    #[test]
    fn limits_follow_the_kernel_abi() {
        assert!(limits(2).contains(&Limit::Truncate));
        assert!(limits(2).contains(&Limit::UnixSockets));
        assert!(!limits(9).contains(&Limit::UnixSockets));
        assert!(limits(9).contains(&Limit::ProtectedByGuard));
    }
}
