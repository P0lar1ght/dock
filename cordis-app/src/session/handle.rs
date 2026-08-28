use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot};

static PROMPT_SEQ: AtomicU64 = AtomicU64::new(1);

use super::commands::{PromptTurnResult, SessionCommand};

#[derive(Clone)]
pub struct SessionHandle {
    pub cmd_tx: mpsc::UnboundedSender<SessionCommand>,
    pub current_prompt_id: Arc<Mutex<Option<String>>>,
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

    pub fn working(&self) -> bool {
        self.current_prompt_id.lock().unwrap().is_some()
    }
}

impl cordis_tui::SessionPort for SessionHandle {
    fn submit(&self, text: String, send_now: bool) {
        SessionHandle::submit(self, text, send_now);
    }

    fn working(&self) -> bool {
        SessionHandle::working(self)
    }
}

fn nanoid() -> u64 {
    PROMPT_SEQ.fetch_add(1, Ordering::Relaxed)
}
