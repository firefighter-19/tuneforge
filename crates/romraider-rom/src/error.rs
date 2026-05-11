use thiserror::Error;

pub type RomResult<T> = Result<T, RomError>;

#[derive(Debug, Error)]
pub enum RomError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("address {addr} is outside the ROM image (size {size} bytes)")]
    AddressOutOfRange { addr: u32, size: usize },

    #[error("table `{name}` is missing required field `{field}` after resolution")]
    TableMissingField { name: String, field: &'static str },

    #[error("decode buffer size mismatch: got {got} bytes, expected {expected}")]
    DecodeSizeMismatch { got: usize, expected: usize },

    #[error("decode size overflow: count={count} × stride={stride} > usize::MAX")]
    DecodeOverflow { count: usize, stride: usize },

    #[error("checksum module `{0}` not implemented")]
    UnsupportedChecksum(String),

    #[error("checksum verification failed at 0x{addr:08X}")]
    ChecksumMismatch { addr: u32 },

    #[error(transparent)]
    Core(#[from] romraider_core::CoreError),

    #[error(transparent)]
    Defs(#[from] romraider_defs::DefError),
}
