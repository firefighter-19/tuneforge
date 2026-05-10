use std::time::Duration;

use thiserror::Error;

pub type IoResult<T> = Result<T, IoError>;

#[derive(Debug, Error)]
pub enum IoError {
    #[error("transport not connected")]
    NotConnected,

    #[error("read timed out after {:.3}s", .0.as_secs_f32())]
    ReadTimeout(Duration),

    #[error("write timed out after {:.3}s", .0.as_secs_f32())]
    WriteTimeout(Duration),

    #[error("serial port error: {0}")]
    Serial(#[from] serialport::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[cfg(feature = "j2534")]
    #[error("J2534 error code {code} ({message})")]
    J2534 { code: u32, message: String },

    #[cfg(feature = "j2534")]
    #[error("failed to load J2534 library `{path}`: {source}")]
    J2534LoadFailed {
        path: String,
        #[source]
        source: libloading::Error,
    },

    #[error("ELM327 unexpected response: {0:?}")]
    ElmUnexpectedResponse(String),

    #[error(transparent)]
    Core(#[from] romraider_core::CoreError),
}
