//! Standalone TUI app: fake spine + session actor. No grok-build.

mod error;
mod names;
mod session;

pub use error::{Error, Result};
pub use names::SESSION;
pub use session::{session_actor, SessionHandle};
