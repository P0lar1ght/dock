//! Session seam the TUI injects. Harness `session` plugin provides this.
//! Live-lookup at the send site — do not capture the Arc in a long-lived closure.

use std::sync::Arc;

/// Queue a prompt / report whether a turn is in flight.
pub trait SessionPort: Send + Sync {
    fn submit(&self, text: String, send_now: bool);
    fn working(&self) -> bool;
}

/// Named `"session.port"` service. Clone is cheap; each call looks through to the actor.
#[derive(Clone)]
pub struct SessionRef(Arc<dyn SessionPort>);

impl SessionRef {
    pub fn new(port: Arc<dyn SessionPort>) -> Self {
        Self(port)
    }

    pub fn submit(&self, text: String, send_now: bool) {
        self.0.submit(text, send_now);
    }

    pub fn working(&self) -> bool {
        self.0.working()
    }
}
