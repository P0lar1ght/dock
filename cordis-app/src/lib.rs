//! Standalone TUI app: fake spine + session actor. No grok-build.

mod cron_driver;
mod error;
mod names;
mod session;
mod setup;
mod system_prompt;
mod tab;

pub use cron_driver::cron_driver;
pub use error::{Error, Result};
pub use names::SESSION;
pub use session::{session_actor, SessionHandle};
pub use system_prompt::system_prompt;
pub use tab::{tab_mount, tab_mount_headless};

pub use setup::{
    print_doctor_report, print_init_report, run_doctor, run_init, run_upgrade, DoctorReport,
    InitOutcome, UpgradeArgs, UpgradeMode,
};
