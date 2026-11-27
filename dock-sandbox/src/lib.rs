//! Dock 的按命令沙箱：一份策略（能写哪、哪些读不到、能不能联网），后端把它落到内核上。
//!
//! - macOS：Seatbelt（`/usr/bin/sandbox-exec` + 生成的 SBPL），底子拷自 Codex。
//! - Linux：Landlock 管文件、seccomp 管网络与 ptrace / io_uring，在子进程 exec 之前装上。
//!   不需要装任何东西，也不需要 user namespace；内核挡不住的那部分由 [`Guard`] 事后还原。
//!
//! 不变式：
//! - 只包被跑的那条命令，Dock 进程自己从不进沙箱。
//! - 包不上就不跑：[`prepare`] 报错、或子进程里装规则失败（`spawn` 随之失败），命令都不会
//!   在沙箱外跑起来。
//! - 后端做不到的保证列在 [`Limit`] 里，由调用方展示给用户。
//!
//! 另有 [`audit`]：出口代理（HTTPS 解密）、只追加的审计日志、链路图——沙箱的观测与审计。
//!
//! 本 crate 不认识 Dock 的插件、配置文件和会话：路径和开关都由调用方传进来。

pub mod audit;
mod backend;
mod denial;
mod env;
#[cfg(unix)]
mod guard;
#[cfg(target_os = "linux")]
mod linux;
mod paths;
mod policy;
pub mod rules;
#[cfg(target_os = "macos")]
mod seatbelt;
pub mod space;
mod tree;

#[cfg(not(unix))]
mod guard {
    //! 非 Unix 上没有后端，也就没有要比对的东西。
    use std::path::PathBuf;

    #[derive(Debug)]
    pub struct Guard;

    #[derive(Debug, Default, PartialEq, Eq)]
    pub struct GuardReport {
        pub restored: Vec<PathBuf>,
        pub unrestored: Vec<PathBuf>,
    }

    impl GuardReport {
        pub fn is_clean(&self) -> bool {
            true
        }
    }

    impl Guard {
        pub fn capture(_: Vec<PathBuf>) -> Self {
            Guard
        }
        pub fn restore(&self) -> GuardReport {
            GuardReport::default()
        }
    }
}

pub use backend::{availability, prepare, Availability, Backend, Limit, PrepareError, Prepared};
pub use denial::looks_like_denial;
pub use guard::{Guard, GuardReport};
pub use paths::real_path;
pub use policy::{Access, Host, Policy, Settings};
