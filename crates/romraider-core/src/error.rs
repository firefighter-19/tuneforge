use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid address `{0}` (expected hex like 0xFF8000)")]
    InvalidAddress(String),

    #[error("out-of-bounds access at offset={offset} len={len} (buffer size {buf_len})")]
    OutOfBounds {
        offset: usize,
        len: usize,
        buf_len: usize,
    },

    #[error("invalid value: {0}")]
    InvalidValue(String),
}
