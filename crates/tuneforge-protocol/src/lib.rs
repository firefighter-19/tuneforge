//! Диагностические протоколы ECU.
//!
//! Каждый модуль собирает запрос как `Vec<u8>` и парсит ответ. Сама I/O-часть
//! делегируется [`tuneforge_io::Transport`] — это даёт чистое разделение
//! «байтовый канал ↔ диалект протокола» и тестируемость на моках.

#![forbid(unsafe_code)]

pub mod error;

#[cfg(feature = "ssm")]
pub mod ssm;

#[cfg(feature = "obd2")]
pub mod obd2;

/// UDS (ISO-14229) сервисы поверх ISO15765/CAN. Generic — без Subaru-специфики.
/// Используется для ReadMemoryByAddress (0x23) — стандартный путь, но на 2007+
/// Subaru ECU этот сервис не реализован, поэтому используем [`subaru`].
#[cfg(feature = "obd2")]
pub mod uds;

/// Subaru-specific SSM-команды (Mode 0xA8 ReadAddresses) поверх ISO15765/CAN.
/// Это то, что Java-RomRaider называет «SSM3» — используется для tuner-grade
/// параметров (knock correction, A/F learning, boost target) на современных
/// Subaru, у которых стандартный UDS Mode 0x23 в default-сессии не отвечает.
#[cfg(feature = "ssm")]
pub mod subaru;

/// Унифицированный ECU-клиент: `EcuClient` trait + `KLineSsmClient` / `CanSsmClient`
/// impls. Слайс 25 — позволяет CLI и GUI работать через единый интерфейс,
/// диспатчая выбор протокола runtime-флагом (`--protocol auto|kline|can|…`).
#[cfg(feature = "ssm")]
pub mod client;

#[cfg(feature = "ds2")]
pub mod ds2;

#[cfg(feature = "ncs")]
pub mod ncs;

pub use error::{ProtocolError, ProtocolResult};
