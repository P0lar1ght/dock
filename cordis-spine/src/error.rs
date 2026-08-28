use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Cordis(#[from] cordis::Error),

    #[error("agent/pre-step rejected the prompt")]
    PreStepRejected,

    #[error("loop exceeded {max} sampling steps without a text reply")]
    MaxSteps { max: usize },
}
