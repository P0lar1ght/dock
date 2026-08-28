use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Cordis(#[from] cordis::Error),
    #[error(transparent)]
    Spine(#[from] cordis_spine::Error),
    #[error("{0}")]
    Message(String),
}
