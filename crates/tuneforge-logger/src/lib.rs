//! Backend логгера: цикл `опрос ECU → семпл → подписчики`.

#![forbid(unsafe_code)]

pub mod datalog;
pub mod error;
pub mod external;
pub mod sample;
pub mod session;

pub use error::{LoggerError, LoggerResult};
pub use sample::{Sample, SampleValue};
pub use session::{LoggerSession, SessionConfig};
