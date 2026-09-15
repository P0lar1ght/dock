//! Standalone TUI app: fake spine + session actor. No grok-build.

mod cron_driver;
mod error;
mod names;
mod session;
mod system_prompt;
mod tab;

pub use cron_driver::cron_driver;
pub use error::{Error, Result};
pub use names::SESSION;
pub use session::{session_actor, SessionHandle};
pub use system_prompt::system_prompt;
pub use tab::tab_mount;
