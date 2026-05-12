//! Типизированные ошибки kernel-upload-пути.

use thiserror::Error;

pub type KernelResult<T> = Result<T, KernelError>;

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("KWP2000 framing error: {0}")]
    Framing(String),

    #[error("Negative response from ECU: SID 0x{request:02X} → NRC 0x{nrc:02X} ({nrc_meaning})")]
    NegativeResponse { request: u8, nrc: u8, nrc_meaning: &'static str },

    #[error("Unexpected response: expected SID 0x{expected:02X} response (0x{:02X}), got 0x{got:02X}", expected | 0x40)]
    UnexpectedSid { expected: u8, got: u8 },

    #[error("Security access failed: seed/key challenge rejected by ECU (10-second lockout starts)")]
    SecurityAccessRejected,

    #[error("Kernel upload aborted: {0}")]
    UploadAborted(String),

    #[error("Kernel handover failed — ECU did not enter RAM-code mode")]
    HandoverFailed,

    #[error("Unsupported MCU family for kernel binary: {0:?}")]
    UnsupportedMcu(crate::kernels::McuFamily),

    #[error("Transport I/O: {0}")]
    Io(#[from] romraider_io::error::IoError),
}
