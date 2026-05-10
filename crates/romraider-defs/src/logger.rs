//! Logger-параметры (`logger.xml` в оригинале).

use std::path::Path;

use romraider_core::Address;
use serde::{Deserialize, Serialize};

use crate::error::DefResult;
use crate::scaling::Scaling;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggerDefinition {
    pub protocol:   String,
    pub parameters: Vec<LoggerParameter>,
    pub switches:   Vec<LoggerSwitch>,
    pub dtcs:       Vec<DtcDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggerParameter {
    pub id:        String,
    pub name:      String,
    pub address:   Address,
    pub length:    u8,
    pub units:     String,
    pub group:     Option<String>,
    pub ecu_byte_index: Option<u8>,
    pub ecu_bit:   Option<u8>,
    pub scalings:  Vec<Scaling>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggerSwitch {
    pub id:      String,
    pub name:    String,
    pub address: Address,
    pub bit:     u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DtcDefinition {
    pub id:        String,
    pub name:      String,
    pub address:   Address,
    pub bit:       u8,
}

impl LoggerDefinition {
    /// Загружает `logger.xml` / `log_defs.xml`.
    ///
    /// TODO: реальный quick-xml парсинг, см. `ecu.rs`.
    pub fn load(_path: impl AsRef<Path>) -> DefResult<Self> {
        Ok(Self {
            protocol:   "SSM".to_string(),
            parameters: Vec::new(),
            switches:   Vec::new(),
            dtcs:       Vec::new(),
        })
    }
}
