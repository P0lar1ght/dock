//! Standalone TUI app: fake spine + session actor. No grok-build.

mod cron_driver;
mod error;
mod names;
mod schedule_cli;
mod session;
mod setup;
mod system_prompt;
mod tab;

pub use cron_driver::{cron_driver, cron_driver_closing_after, CLOSE_IDLE_AFTER};
pub use error::{Error, Result};
pub use names::SESSION;
pub use schedule_cli::{parse_schedule_args, run_schedule_cmd, ScheduleCmd, SCHEDULE_USAGE};
pub use session::{session_actor, SessionHandle};
pub use system_prompt::system_prompt;
pub use tab::{tab_mount, tab_mount_headless};

pub use setup::{
    print_doctor_report, print_init_report, run_doctor, run_init, run_upgrade, DoctorReport,
    InitOutcome, UpgradeArgs, UpgradeMode,
};
