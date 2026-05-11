//! Парсер XML-определений RomRaider.
//!
//! Совместимость с эталонным форматом из апстрим-репозитория
//! [`RomRaider-Definitions`](https://github.com/RomRaider/RomRaider-Definitions).
//! Здесь — Rust-маппинг XML и его базовая валидация. Резолв наследования,
//! парсинг адресов в `u32` и eval scaling-выражений — в последующих слайсах.

#![forbid(unsafe_code)]

pub mod ecu;
pub mod error;
pub mod parser;

pub use ecu::{
    EcuDefinition, RomDefinition, RomId, RomsDocument, ScalingBase, ScalingRef, TableDef,
};
pub use error::{DefError, DefResult};
pub use parser::{parse_file, parse_reader, parse_str};
