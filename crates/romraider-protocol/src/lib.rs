//! Диагностические протоколы ECU.
//!
//! Каждый модуль собирает запрос как `Vec<u8>` и парсит ответ. Сама I/O-часть
//! делегируется [`romraider_io::Transport`] — это даёт чистое разделение
//! «байтовый канал ↔ диалект протокола» и тестируемость на моках.

#![forbid(unsafe_code)]

pub mod error;

#[cfg(feature = "ssm")]
pub mod ssm;

#[cfg(feature = "obd2")]
pub mod obd2;

/// UDS (ISO-14229) сервисы поверх ISO15765/CAN. Generic — без Subaru-специфики.
/// Используется для ReadMemoryByAddress (0x23) — Subaru tuner-grade params.
#[cfg(feature = "obd2")]
pub mod uds;

#[cfg(feature = "ds2")]
pub mod ds2;

#[cfg(feature = "ncs")]
pub mod ncs;

pub use error::{ProtocolError, ProtocolResult};
