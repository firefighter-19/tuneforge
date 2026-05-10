//! Парсер XML-определений RomRaider.
//!
//! Совместимость с эталонным форматом из `../RomRaider/definitions`:
//! `cars_def.dtd`, `ecu_defs.dtd`, `logger.dtd`, `profile.dtd`. Эти DTD
//! не модифицируются — они и есть источник истины. Здесь лежит только
//! Rust-маппинг и валидация.

#![forbid(unsafe_code)]

pub mod ecu;
pub mod error;
pub mod logger;
pub mod scaling;

pub use ecu::EcuDefinition;
pub use error::{DefError, DefResult};
pub use logger::LoggerDefinition;
pub use scaling::Scaling;
