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

    /// Для `type="Switch"` — список именованных состояний (`<state name="On" data="01"/>`).
    #[serde(default, rename = "state")]
    pub states: Vec<SwitchState>,

    /// Для `type="BitwiseSwitch"` — флаги по битам (`<bit name="AC" position="0"/>`).
    #[serde(default, rename = "bit")]
    pub bits: Vec<SwitchBit>,

    #[serde(default)]
    pub description: Option<String>,
}

/// `<state>` внутри `<table type="Switch">`. `data` — hex-строка (одна или
/// несколько байт), задающая значение, которое надо записать при выборе.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchState {
    #[serde(rename = "@name")]
    pub name: String,

    #[serde(rename = "@data")]
    pub data: String,
}

impl SwitchState {
    /// Распарсить `data` в байты. Поддерживаем три варианта формата:
    /// - монолитный hex: `"01"`, `"0A"`, `"0001"`
    /// - hex с `0x`-префиксом: `"0x01"`
    /// - hex-байты через пробел/запятую: `"00 00 5A A5 A5 5A"` (типично для
    ///   `Checksum Fix` и других multi-byte switch-таблиц).
    /// Нечётная длина «слитного» куска pad-ится ведущим нулём; пустая → `Ok(vec![])`.
    pub fn data_bytes(&self) -> Result<Vec<u8>, std::num::ParseIntError> {
        let raw = self.data.trim();
        // Удаляем whitespace и запятые — оставляем только hex-знаки.
        let cleaned: String = raw
            .chars()
            .filter(|c| !c.is_whitespace() && *c != ',')
            .collect();
        let cleaned = cleaned.trim_start_matches("0x").trim_start_matches("0X");
        if cleaned.is_empty() {
            return Ok(Vec::new());
        }
        let padded: String = if cleaned.len() % 2 == 1 {
            format!("0{cleaned}")
        } else {
            cleaned.to_string()
        };
        (0..padded.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&padded[i..i + 2], 16))
            .collect()
    }
}

/// `<bit>` внутри `<table type="BitwiseSwitch">`. `position` ∈ `0..8`
/// (мы поддерживаем только 1-байтные bitwise-таблицы).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchBit {
    #[serde(rename = "@name")]
    pub name: String,

    #[serde(rename = "@position")]
    pub position: String,
}

impl SwitchBit {
    /// Распарсить `position` в `u8` (`0..=7`).
    pub fn bit_position(&self) -> Result<u8, std::num::ParseIntError> {
        self.position.trim().parse::<u8>()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn state(data: &str) -> SwitchState {
        SwitchState {
            name: "x".into(),
            data: data.into(),
        }
    }

    #[test]
    fn parses_single_byte() {
        assert_eq!(state("01").data_bytes().unwrap(), vec![0x01]);
        assert_eq!(state("ff").data_bytes().unwrap(), vec![0xFF]);
        assert_eq!(state("0x42").data_bytes().unwrap(), vec![0x42]);
    }

    #[test]
    fn parses_multibyte_no_separator() {
        assert_eq!(state("17 25").data_bytes().unwrap(), vec![0x17, 0x25]);
        assert_eq!(state("1725").data_bytes().unwrap(), vec![0x17, 0x25]);
    }

    #[test]
    fn parses_checksum_fix_long_string_with_spaces() {
        // Реальный фрагмент из ecu_defs.xml — `Checksum Fix` Switch, sizey=168,
        // данные разделены пробелами. До фикса парсер давал «malformed».
        let data = "00 00 00 00 00 00 00 00 5A A5 A5 5A";
        assert_eq!(
            state(data).data_bytes().unwrap(),
            vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5A, 0xA5, 0xA5, 0x5A],
        );
    }

    #[test]
    fn empty_data_is_ok() {
        assert!(state("").data_bytes().unwrap().is_empty());
        assert!(state("   ").data_bytes().unwrap().is_empty());
    }
}
