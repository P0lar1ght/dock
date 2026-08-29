use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot};

use cordis_tui::QueuedItem;

static PROMPT_SEQ: AtomicU64 = AtomicU64::new(1);

use super::commands::{PromptTurnResult, SessionCommand};

#[derive(Clone)]
pub struct SessionHandle {
    pub cmd_tx: mpsc::UnboundedSender<SessionCommand>,
    pub current_prompt_id: Arc<Mutex<Option<String>>>,
    pub queued: Arc<AtomicUsize>,
    pub queued_prompts: Arc<Mutex<Vec<QueuedItem>>>,
}

impl SessionHandle {
    pub fn submit(&self, text: impl Into<String>, send_now: bool) {
        let (respond_to, _) = oneshot::channel();
        let _ = self.cmd_tx.send(SessionCommand::Prompt {
            prompt_id: format!("prompt-{}", nanoid()),
            text: text.into(),
            send_now,
            respond_to,
        });
    }

    pub async fn prompt(&self, text: impl Into<String>) -> PromptTurnResult {
        let (respond_to, rx) = oneshot::channel();
        self.cmd_tx
            .send(SessionCommand::Prompt {
                prompt_id: format!("prompt-{}", nanoid()),
                text: text.into(),
                send_now: false,
                respond_to,
            })
            .map_err(|_| "session actor closed".to_string())?;
        rx.await.map_err(|_| "session actor dropped".to_string())?
    }

    pub fn compact(&self, context: impl Into<String>) {
        let (respond_to, _) = oneshot::channel();
        let _ = self.cmd_tx.send(SessionCommand::Compact {
            context: context.into(),
            respond_to,
        });
    }

    pub fn working(&self) -> bool {
        self.current_prompt_id.lock().unwrap().is_some()
    }

    pub fn cancel(&self) {
        let _ = self.cmd_tx.send(SessionCommand::Cancel);
    }

    pub fn has_queued(&self) -> bool {
        self.queued.load(Ordering::Relaxed) > 0
    }

    pub fn queued_prompts(&self) -> Vec<QueuedItem> {
        self.queued_prompts.lock().unwrap().clone()
    }

    pub fn promote(&self, id: Option<String>) {
        let _ = self.cmd_tx.send(SessionCommand::Promote { id });
    }

    pub fn take_queued(&self, id: Option<String>) -> Option<QueuedItem> {
        let item = {
            let mut snaps = self.queued_prompts.lock().unwrap();
            match &id {
                Some(want) => snaps
                    .iter()
                    .position(|q| &q.id == want)
                    .map(|i| snaps.remove(i)),
                None => snaps.pop(),
            }
        };
        if let Some(ref item) = item {
            let _ = self.cmd_tx.send(SessionCommand::Take {
                id: Some(item.id.clone()),
            });
        }
        item
    }
}

impl cordis_tui::SessionPort for SessionHandle {
    fn submit(&self, text: String, send_now: bool) {
        SessionHandle::submit(self, text, send_now);
    }

    fn working(&self) -> bool {
        SessionHandle::working(self)
    }

    fn cancel(&self) {
        SessionHandle::cancel(self);
    }

    fn has_queued(&self) -> bool {
        SessionHandle::has_queued(self)
    }

    fn compact(&self, context: String) {
        SessionHandle::compact(self, context);
    }

    fn queued_prompts(&self) -> Vec<QueuedItem> {
        SessionHandle::queued_prompts(self)
    }

    fn promote(&self, id: Option<String>) {
        SessionHandle::promote(self, id);
    }

    fn take_queued(&self, id: Option<String>) -> Option<QueuedItem> {
        SessionHandle::take_queued(self, id)
    }
}

fn nanoid() -> u64 {
    PROMPT_SEQ.fetch_add(1, Ordering::Relaxed)
}
