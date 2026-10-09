use cordis_spine::UserImage;
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
    /// 插话：一轮在跑就交给它在下一个步骤边界送达（不取消）；没有可并的采样
    /// （空闲、压缩中）就排到最前。
    Steer {
        text: String,
        images: Vec<UserImage>,
    },
    /// 把一条排队的消息改成插话。`None` = 最早那条。
    SteerQueued {
        id: Option<String>,
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
