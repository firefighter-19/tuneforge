//! Парсер XML-определений RomRaider.
//!
//! Совместимость с эталонным форматом из апстрим-репозитория
//! [`RomRaider-Definitions`](https://github.com/RomRaider/RomRaider-Definitions).
//! Здесь — Rust-маппинг XML и его базовая валидация. Резолв наследования,
//! парсинг адресов в `u32` и eval scaling-выражений — в последующих слайсах.

#![forbid(unsafe_code)]

pub mod ecu;
pub mod error;
pub mod expression;
pub mod logger;
pub mod parser;
pub mod resolve;
pub mod typed;

pub use ecu::{
    EcuDefinition, RomDefinition, RomId, RomsDocument, ScalingBase, ScalingRef, TableDef,
};
pub use error::{DefError, DefResult};
pub use expression::CompiledScaling;
pub use logger::{
    parse_log_file, parse_log_reader, parse_log_str, Alt, ConvertFactor, EcuTools, LogParameter,
    LogProtocol, LogProtocols, LoggerDocument, LoggerEcu,
};
pub use parser::{parse_file, parse_reader, parse_str};
pub use resolve::{resolve, ResolvedRom, ResolvedScaling, ResolvedTable};
pub use typed::{parse_endian, StorageType, TableKind};
