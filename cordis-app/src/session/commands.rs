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
    /// 侧边聊天写进来的一段笔记：有任务在跑就交给循环在步骤边界落（收尾后还没
    /// 落的由 actor 补）；闲着直接落进历史。不开始新的一轮。
    SideNote {
        note: String,
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
