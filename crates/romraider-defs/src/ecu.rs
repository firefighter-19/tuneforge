//! Сырая модель ECU-definition XML.
//!
//! Поля повторяют реальный формат файлов из апстрим-репозитория
//! [`RomRaider-Definitions`](https://github.com/RomRaider/RomRaider-Definitions),
//! а не DTD (`ecu_defs.dtd`) — DTD в апстриме давно отстал от продакшн-формата.
//!
//! Это **сырой слой**: значения остаются строками (адреса, размеры, expression),
//! наследование `base="…"` ещё не разрешается. Резолв и типобезопасная
//! материализация — отдельный слой в Слайсе 3.

use serde::{Deserialize, Serialize};

/// Корневой `<roms>`-документ.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RomsDocument {
    #[serde(default, rename = "scalingbase")]
    pub scaling_bases: Vec<ScalingBase>,

    #[serde(default, rename = "rom")]
    pub roms: Vec<RomDefinition>,
}

impl RomsDocument {
    /// Найти ROM по `xmlid` (если у него есть `<romid>` с этим полем).
    #[must_use]
    pub fn find_rom_by_xml_id(&self, xml_id: &str) -> Option<&RomDefinition> {
        self.roms.iter().find(|r| {
            r.romid
                .as_ref()
                .and_then(|id| id.xml_id.as_deref())
                .is_some_and(|id| id == xml_id)
        })
    }

    /// Найти `<scalingbase name="…">` по имени.
    #[must_use]
    pub fn find_scaling_base(&self, name: &str) -> Option<&ScalingBase> {
        self.scaling_bases.iter().find(|s| s.name == name)
    }
}

/// Верхнеуровневый `<scalingbase>` — переиспользуемый шаблон масштабирования.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScalingBase {
    #[serde(rename = "@name")]
    pub name: String,

    #[serde(default, rename = "@category")]
    pub category: Option<String>,

    #[serde(default, rename = "@units")]
    pub units: Option<String>,

    #[serde(rename = "@expression")]
    pub expression: String,

    #[serde(rename = "@to_byte")]
    pub to_byte: String,

    #[serde(default, rename = "@format")]
    pub format: Option<String>,

    #[serde(default, rename = "@fineincrement")]
    pub fine_increment: Option<String>,

    #[serde(default, rename = "@coarseincrement")]
    pub coarse_increment: Option<String>,

    #[serde(default, rename = "@min")]
    pub min: Option<String>,

    #[serde(default, rename = "@max")]
    pub max: Option<String>,
}

/// Описание одного ROM-варианта (`<rom>`).
///
/// Может быть абстрактным шаблоном (имя в `<romid><xmlid>` без `internalidaddress`)
/// или конкретной прошивкой. Может наследовать через `base="<xmlid>"`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RomDefinition {
    #[serde(default, rename = "@base")]
    pub base: Option<String>,

    #[serde(default)]
    pub romid: Option<RomId>,

    #[serde(default, rename = "table")]
    pub tables: Vec<TableDef>,
}

/// Группа идентифицирующих полей ROM.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RomId {
    #[serde(default, rename = "xmlid")]
    pub xml_id: Option<String>,

    #[serde(default, rename = "internalidaddress")]
    pub internal_id_address: Option<String>,

    #[serde(default, rename = "internalidstring")]
    pub internal_id_string: Option<String>,

    #[serde(default, rename = "ecuid")]
    pub ecu_id: Option<String>,

    #[serde(default)]
    pub year: Option<String>,

    #[serde(default)]
    pub market: Option<String>,

    #[serde(default)]
    pub make: Option<String>,

    #[serde(default)]
    pub model: Option<String>,

    #[serde(default)]
    pub submodel: Option<String>,

    #[serde(default)]
    pub transmission: Option<String>,

    #[serde(default, rename = "memmodel")]
    pub mem_model: Option<String>,

    #[serde(default, rename = "flashmethod")]
    pub flash_method: Option<String>,

    #[serde(default, rename = "filesize")]
    pub file_size: Option<String>,
}

/// Определение одной таблицы прошивки.
///
/// Может быть «полным» (`type`, `storagetype`, `storageaddress`, scaling, оси)
/// или «частичным» (наследует через `base="…"` от шаблона; задаёт только
/// то, что отличается).
///
/// Оси описаны как **вложенные таблицы** с `type="X Axis"`/`"Y Axis"`/`"Static Y Axis"`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDef {
    #[serde(default, rename = "@type")]
    pub kind: Option<String>,

    #[serde(default, rename = "@name")]
    pub name: Option<String>,

    #[serde(default, rename = "@base")]
    pub base: Option<String>,

    #[serde(default, rename = "@category")]
    pub category: Option<String>,

    #[serde(default, rename = "@storagetype")]
    pub storage_type: Option<String>,

    #[serde(default, rename = "@endian")]
    pub endian: Option<String>,

    #[serde(default, rename = "@storageaddress")]
    pub storage_address: Option<String>,

    #[serde(default, rename = "@sizex")]
    pub size_x: Option<String>,

    #[serde(default, rename = "@sizey")]
    pub size_y: Option<String>,

    #[serde(default, rename = "@userlevel")]
    pub user_level: Option<String>,

    #[serde(default, rename = "@logparam")]
    pub log_param: Option<String>,

    #[serde(default, rename = "scaling")]
    pub scalings: Vec<ScalingRef>,

    /// Вложенные таблицы — обычно оси (X/Y), но иногда и «дочерние» таблицы.
    #[serde(default, rename = "table")]
    pub nested: Vec<TableDef>,

    /// Статические текстовые метки (для `Static X/Y Axis`).
    #[serde(default, rename = "data")]
    pub data: Vec<String>,

    #[serde(default)]
    pub description: Option<String>,
}

/// `<scaling>`-узел внутри `<table>`: inline-определение или ссылка на
/// `<scalingbase>` через `base="…"`. Может также частично переопределять
/// поля родителя (например, дополнительный `units` для другого UI-режима).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScalingRef {
    #[serde(default, rename = "@base")]
    pub base: Option<String>,

    #[serde(default, rename = "@category")]
    pub category: Option<String>,

    #[serde(default, rename = "@units")]
    pub units: Option<String>,

    #[serde(default, rename = "@expression")]
    pub expression: Option<String>,

    #[serde(default, rename = "@to_byte")]
    pub to_byte: Option<String>,

    #[serde(default, rename = "@format")]
    pub format: Option<String>,

    #[serde(default, rename = "@fineincrement")]
    pub fine_increment: Option<String>,

    #[serde(default, rename = "@coarseincrement")]
    pub coarse_increment: Option<String>,

    #[serde(default, rename = "@min")]
    pub min: Option<String>,

    #[serde(default, rename = "@max")]
    pub max: Option<String>,
}

/// Историческое имя для совместимости со Слайсом 1. Будет удалено, когда
/// появится материализованная типобезопасная модель.
pub type EcuDefinition = RomDefinition;
