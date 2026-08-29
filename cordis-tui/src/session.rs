//! Session seam the TUI injects. Harness `session` plugin provides this.
//! Live-lookup at the send site — do not capture the Arc in a long-lived closure.

use std::sync::Arc;

/// One prompt sitting in the actor queue (compact jobs are omitted).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuedItem {
    pub id: String,
    pub text: String,
}

/// Queue a prompt / report whether a turn is in flight.
pub trait SessionPort: Send + Sync {
    fn submit(&self, text: String, send_now: bool);
    fn working(&self) -> bool;
    fn cancel(&self);
    fn has_queued(&self) -> bool;
    fn compact(&self, context: String);
    fn queued_prompts(&self) -> Vec<QueuedItem>;
    fn promote(&self, id: Option<String>);
    fn take_queued(&self, id: Option<String>) -> Option<QueuedItem>;
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

    pub fn cancel(&self) {
        self.0.cancel();
    }

    pub fn has_queued(&self) -> bool {
        self.0.has_queued()
    }

    pub fn compact(&self, context: String) {
        self.0.compact(context);
    }

    pub fn queued_prompts(&self) -> Vec<QueuedItem> {
        self.0.queued_prompts()
    }

    pub fn promote(&self, id: Option<String>) {
        self.0.promote(id);
    }

    pub fn take_queued(&self, id: Option<String>) -> Option<QueuedItem> {
        self.0.take_queued(id)
    }
}
