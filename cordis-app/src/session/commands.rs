use tokio::sync::oneshot;

pub type PromptTurnResult = Result<String, String>;

#[derive(Debug)]
pub enum SessionCommand {
    Prompt {
        prompt_id: String,
        text: String,
        send_now: bool,
        respond_to: oneshot::Sender<PromptTurnResult>,
    },
    Compact {
        context: String,
        respond_to: oneshot::Sender<PromptTurnResult>,
    },
    /// Move a queued prompt to the front and cancel the in-flight turn.
    /// `None` = oldest prompt job.
    Promote {
        id: Option<String>,
    },
    /// Remove a queued prompt without running it (composer edit).
    /// `None` = newest prompt job.
    Take {
        id: Option<String>,
    },
    Cancel,
    Shutdown,
}
