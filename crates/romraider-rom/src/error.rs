use thiserror::Error;

pub type RomResult<T> = Result<T, RomError>;

#[derive(Debug, Error)]
pub enum RomError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("address {addr} is outside the ROM image (size {size} bytes)")]
    AddressOutOfRange { addr: u32, size: usize },

    #[error("checksum module `{0}` not implemented")]
    UnsupportedChecksum(String),

    #[error("checksum verification failed at 0x{addr:08X}")]
    ChecksumMismatch { addr: u32 },

    #[error(transparent)]
    Core(#[from] romraider_core::CoreError),

    #[error(transparent)]
    Defs(#[from] romraider_defs::DefError),
}
