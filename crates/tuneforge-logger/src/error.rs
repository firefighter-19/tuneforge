use thiserror::Error;

pub type LoggerResult<T> = Result<T, LoggerError>;

#[derive(Debug, Error)]
pub enum LoggerError {
    #[error(transparent)]
    Protocol(#[from] tuneforge_protocol::ProtocolError),

    #[error(transparent)]
    Io(#[from] tuneforge_io::IoError),

    #[error(transparent)]
    StdIo(#[from] std::io::Error),

    #[error(transparent)]
    Defs(#[from] tuneforge_defs::DefError),

    #[error("session has no active subscriptions")]
    NoSubscriptions,
}
