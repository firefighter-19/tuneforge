//! ECU-определение: список таблиц прошивки с адресами, scaling и осями.

use std::path::Path;

use romraider_core::Address;
use serde::{Deserialize, Serialize};

use crate::error::DefResult;
use crate::scaling::Scaling;

/// Заголовок ROM-определения (`<rom>` в XML).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcuDefinition {
    pub xml_id:     String,
    pub internal_id_address: Option<Address>,
    pub internal_id_string:  Option<String>,
    pub ecu_id:     Option<String>,
    pub year:       Option<String>,
    pub market:     Option<String>,
    pub make:       Option<String>,
    pub model:      Option<String>,
    pub submodel:   Option<String>,
    pub transmission: Option<String>,
    pub memory_model: Option<String>,
    pub flash_method: Option<String>,
    pub tables:     Vec<TableDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDef {
    pub name:     String,
    pub category: Option<String>,
    pub kind:     TableKind,
    pub address:  Option<Address>,
    pub scaling:  Option<Scaling>,
    pub axes:     Vec<AxisDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableKind {
    OneD,
    TwoD,
    ThreeD,
    Constant,
    Switch,
    Selectable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisDef {
    pub name:     String,
    pub address:  Option<Address>,
    pub size:     u32,
    pub scaling:  Option<Scaling>,
}

impl EcuDefinition {
    /// Парсит `ecu_defs.xml` или `cars_def.xml` (`../RomRaider/definitions/*`).
    ///
    /// TODO: подключить полный обход `<rom>` элементов через `quick-xml::de`.
    /// Сейчас это вход для тестов и сборки — реальный парсер пишется
    /// после стабилизации DTD-маппинга.
    pub fn load(_path: impl AsRef<Path>) -> DefResult<Vec<Self>> {
        Ok(Vec::new())
    }
}
