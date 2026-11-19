//! 手动试沙箱：在沙箱里跑一条命令，按 Dock 的默认策略（`$DOCK_HOME`、`$HOME`、临时目录
//! 都从环境里读）。
//!
//! ```text
//! cargo run -p dock-sandbox --example sandboxed -- [--read-only] [--offline] <工作区> <命令…>
//! cargo run -p dock-sandbox --example sandboxed -- . bash -lc 'git status && cargo build'
//! ```

use std::ffi::OsString;
use std::path::PathBuf;

use dock_sandbox::{availability, prepare, Access, Availability, Host, Policy, Settings};

fn main() {
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let mut access = Access::WorkspaceWrite;
    let mut settings = Settings::default();
    while let Some(flag) = args.first().and_then(|a| a.to_str()) {
        match flag {
            "--read-only" => access = Access::ReadOnly,
            "--offline" => settings.network = false,
            _ => break,
        }
        args.remove(0);
    }
    if args.len() < 2 {
        eprintln!("用法：sandboxed [--read-only] [--offline] <工作区> <命令…>");
        std::process::exit(2);
    }
    let workspace = PathBuf::from(args.remove(0));
    match availability() {
        Availability::Ready { backend, limits } => {
            eprintln!("[sandbox] {} {limits:?}", backend.name())
        }
        Availability::Unavailable { reason } => {
            eprintln!("[sandbox] 不可用：{reason}");
            std::process::exit(3);
        }
    }
    let dock_home = std::env::var_os("DOCK_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".dock")))
        .unwrap_or_else(|| PathBuf::from(".dock"));
    let policy = Policy::build(access, &workspace, &settings, &Host::detect(dock_home));
    let (mut cmd, guard) = match prepare(&policy, &args) {
        Ok(p) => p.into_command(),
        Err(e) => {
            eprintln!("[sandbox] 包不上：{e}");
            std::process::exit(3);
        }
    };
    let status = cmd.current_dir(&workspace).status().expect("起不来");
    if let Some(guard) = guard {
        let report = guard.restore();
        if !report.is_clean() {
            eprintln!("[sandbox] 还原了受保护路径：{report:?}");
        }
    }
    std::process::exit(status.code().unwrap_or(1));
}
