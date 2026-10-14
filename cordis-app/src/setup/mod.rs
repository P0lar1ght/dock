//! `dock init` / `doctor` / `upgrade`：装机初始化、健康检查与原子升级。
//!
//! 这些子命令在起插件树之前就退出，不依赖 TUI / gateway。

mod busy;
mod doctor;
mod init;
mod upgrade;

pub use doctor::{print_doctor_report, run_doctor, DoctorReport};
pub use init::{print_init_report, run_init, InitOutcome};
pub use upgrade::{run_upgrade, UpgradeArgs, UpgradeMode};
