use thiserror::Error;

pub type ProtocolResult<T> = Result<T, ProtocolError>;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("checksum mismatch (got 0x{got:02X}, expected 0x{expected:02X})")]
    ChecksumMismatch { got: u8, expected: u8 },

    #[error("response too short: got {got} bytes, expected at least {expected}")]
    ResponseTooShort { got: usize, expected: usize },

    #[error("unexpected response code 0x{0:02X}")]
    UnexpectedResponse(u8),

    #[error("ECU returned negative response: 0x{nrc:02X} for service 0x{service:02X}")]
    NegativeResponse { service: u8, nrc: u8 },

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error(transparent)]
    Io(#[from] tuneforge_io::IoError),

    #[error(transparent)]
    Core(#[from] tuneforge_core::CoreError),
}
