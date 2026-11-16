use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Cordis(#[from] cordis::Error),

    #[error("agent/pre-step rejected the prompt")]
    PreStepRejected,

    /// 循环已经没有步数上限，现在不会再产生；留着是因为它在公开的 `Error` 里。
    #[error("loop exceeded {max} sampling steps without a text reply")]
    MaxSteps { max: usize },

    /// 连着好几步发出完全相同的工具调用，循环停下了这一轮（见 `agent::stationarity`）。
    /// 文案就是给用户看的说明：`end_turn` 用它记失败原因。
    #[error(
        "连续 {run} 步用完全相同的参数调用 `{tool}`，看起来卡住了，已停下。发一条新消息可继续。"
    )]
    Stationary { tool: String, run: u32 },

    #[error("turn cancelled")]
    Cancelled,

    #[error("{0}")]
    Compact(String),
}
