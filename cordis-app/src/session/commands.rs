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
    Shutdown,
}
