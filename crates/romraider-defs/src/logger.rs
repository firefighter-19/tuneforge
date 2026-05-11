//! Парсер `log_defs.xml` — определений логгера.
//!
//! Формат отличается от ECU-determinations: корень `<ecus>` содержит
//! `<ecu_tools>` (общие конверсии) и `<logprotocols>` со списком протоколов
//! (SSM, OBD-II и т.д.). Под каждым `<logprotocol>` живут «template»-ECU
//! (`id="base"`, описывают все параметры) и «concrete»-ECU (с реальным
//! ECU-ID, ссылающиеся через `include="..."` на шаблон).
//!
//! Этот модуль — **сырой парс**. Резолв `include`-наследования и компиляция
//! `expr="[value]/4"` в считающую функцию — следующие слайсы.

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{DefError, DefResult};

/// Корневой `<ecus>`-документ из `log_defs.xml`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggerDocument {
    #[serde(default)]
    pub ecu_tools: EcuTools,

    #[serde(default)]
    pub logprotocols: LogProtocols,
}

impl LoggerDocument {
    /// Найти `<convert_factor type="…">` по `type` (`afr`/`temp`/`press`/…).
    #[must_use]
    pub fn find_convert_factor(&self, kind: &str) -> Option<&ConvertFactor> {
        self.ecu_tools
            .convert_factors
            .iter()
            .find(|f| f.kind == kind)
    }

    /// Найти ECU по его `id` в любом из протоколов (template `"base"` или конкретный hex-ID).
    #[must_use]
    pub fn find_ecu(&self, id: &str) -> Option<&LoggerEcu> {
        self.logprotocols
            .logprotocols
            .iter()
            .flat_map(|p| p.ecus.iter())
            .find(|e| e.id == id)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcuTools {
    #[serde(default, rename = "convert_factor")]
    pub convert_factors: Vec<ConvertFactor>,
}

/// Шаблон-конверсия из `<ecu_tools>`. На неё ссылаются параметры через `type="…"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertFactor {
    /// `type` атрибут — «afr», «temp», «press», «speed», «inj».
    #[serde(rename = "@type")]
    pub kind: String,

    #[serde(rename = "@name")]
    pub name: String,

    #[serde(rename = "@metric")]
    pub metric: String,

    /// Выражение в формате логгера: использует `[value]` как плейсхолдер
    /// и иногда `GetLogParam("…")` для ссылок на другие параметры.
    #[serde(rename = "@expr")]
    pub expr: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogProtocols {
    #[serde(default, rename = "logprotocol")]
    pub logprotocols: Vec<LogProtocol>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogProtocol {
    /// Диалект протокола — `SSM`, `OBD`, `DS2`, `NCS` и т.д.
    #[serde(rename = "@type")]
    pub kind: String,

    /// Имя шаблон-ECU по умолчанию (`ssmbase`).
    #[serde(default, rename = "@default")]
    pub default: Option<String>,

    #[serde(default, rename = "ecu")]
    pub ecus: Vec<LoggerEcu>,
}

/// Один `<ecu>`-узел: либо template (`id="base"`), либо ссылка на конкретную
/// прошивку (`id="1644500305"`, `include="ssmbase16"`).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggerEcu {
    #[serde(rename = "@id")]
    pub id: String,

    #[serde(rename = "@name")]
    pub name: String,

    /// Внутреннее имя шаблона (`ssmbase`, `ssmbase16`, `ssmbase32`)
    /// или код калибровки (`AE800`).
    #[serde(default, rename = "@type")]
    pub kind: Option<String>,

    /// Имя ECU, чьи параметры включаются (`include="ssmbase16"`).
    #[serde(default, rename = "@include")]
    pub include: Option<String>,

    #[serde(default, rename = "parameter")]
    pub parameters: Vec<LogParameter>,
}

impl LoggerEcu {
    /// Найти параметр по `id` среди собственных (без резолва `include`).
    #[must_use]
    pub fn find_parameter(&self, id: &str) -> Option<&LogParameter> {
        self.parameters.iter().find(|p| p.id == id)
    }

    /// Является ли этот ECU «шаблоном» (id="base").
    #[must_use]
    pub fn is_template(&self) -> bool {
        self.id == "base"
    }
}

/// Один параметр логгера. Адрес/тип/выражение/единицы измерения.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogParameter {
    #[serde(rename = "@id")]
    pub id: String,

    /// Сдвиг в адресном пространстве ECU (`#000E` — формат с `#` префиксом
    /// и hex-цифрами). Парсим в u32 — в Слайсе резолвинга.
    #[serde(rename = "@offset")]
    pub offset: String,

    #[serde(default, rename = "@storagetype")]
    pub storage_type: Option<String>,

    #[serde(default, rename = "@decimals")]
    pub decimals: Option<String>,

    /// Индекс бита в запросе SSM-init capability bitmap.
    #[serde(default, rename = "@bit")]
    pub bit: Option<String>,

    /// Индекс байта в запросе SSM-init capability bitmap.
    #[serde(default, rename = "@byte")]
    pub byte: Option<String>,

    /// Выражение преобразования byte→real. Использует плейсхолдер `[value]`.
    #[serde(default, rename = "@expr")]
    pub expr: Option<String>,

    /// Единицы измерения по умолчанию (`RPM`, `g/s`, `B C`…).
    #[serde(default, rename = "@metric")]
    pub metric: Option<String>,

    /// Ссылка на `<convert_factor type="…">` для альтернативных единиц.
    #[serde(default, rename = "@type")]
    pub kind: Option<String>,

    /// Описание. В исходнике перевод строк закодирован как `|`.
    #[serde(default, rename = "@desc")]
    pub desc: Option<String>,

    /// Альтернативные адреса под этот же логический параметр у разных прошивок.
    #[serde(default, rename = "alt")]
    pub alts: Vec<Alt>,
}

/// Альтернативный адрес параметра под конкретную прошивку.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alt {
    #[serde(rename = "@id")]
    pub id: String,

    #[serde(rename = "@offset")]
    pub offset: String,

    #[serde(default, rename = "@storagetype")]
    pub storage_type: Option<String>,
}

// ---------------------------------------------------------------- parsers ---

pub fn parse_log_str(xml: &str) -> DefResult<LoggerDocument> {
    quick_xml::de::from_str(xml).map_err(DefError::from)
}

pub fn parse_log_reader<R: Read>(reader: R) -> DefResult<LoggerDocument> {
    let buf = std::io::BufReader::new(reader);
    quick_xml::de::from_reader(buf).map_err(DefError::from)
}

pub fn parse_log_file(path: impl AsRef<Path>) -> DefResult<LoggerDocument> {
    let xml = std::fs::read_to_string(path)?;
    parse_log_str(&xml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_ecutools_and_protocol() {
        let xml = r##"
            <ecus>
              <ecu_tools>
                <convert_factor type="afr" name="AFR" metric="AFR" expr="[value]*14.7"/>
              </ecu_tools>
              <logprotocols>
                <logprotocol type="SSM" default="ssmbase">
                  <ecu id="base" name="SSM base" type="ssmbase">
                    <parameter id="Engine Speed" offset="#000E" storagetype="uint16"
                               decimals="0" bit="1" byte="1" expr="[value]/4" metric="RPM"
                               desc="Engine RPM"/>
                  </ecu>
                </logprotocol>
              </logprotocols>
            </ecus>
        "##;
        let doc = parse_log_str(xml).unwrap();
        assert_eq!(doc.ecu_tools.convert_factors.len(), 1);
        assert_eq!(doc.ecu_tools.convert_factors[0].kind, "afr");
        assert_eq!(doc.logprotocols.logprotocols.len(), 1);

        let proto = &doc.logprotocols.logprotocols[0];
        assert_eq!(proto.kind, "SSM");
        assert_eq!(proto.default.as_deref(), Some("ssmbase"));
        assert_eq!(proto.ecus.len(), 1);

        let ecu = &proto.ecus[0];
        assert!(ecu.is_template());
        assert_eq!(ecu.parameters.len(), 1);
        let p = &ecu.parameters[0];
        assert_eq!(p.id,                    "Engine Speed");
        assert_eq!(p.offset,                "#000E");
        assert_eq!(p.storage_type.as_deref(), Some("uint16"));
        assert_eq!(p.expr.as_deref(),       Some("[value]/4"));
        assert_eq!(p.metric.as_deref(),     Some("RPM"));
    }

    #[test]
    fn parses_parameter_with_alts() {
        let xml = r##"
            <ecus>
              <logprotocols>
                <logprotocol type="SSM">
                  <ecu id="base" name="t">
                    <parameter id="Advance Multiplier" offset="#20124" storagetype="uint8" expr="[value]" metric="" desc="">
                      <alt id="AM (20118)" offset="#20118"/>
                      <alt id="AM (20120)" offset="#20120" storagetype="float"/>
                    </parameter>
                  </ecu>
                </logprotocol>
              </logprotocols>
            </ecus>
        "##;
        let doc = parse_log_str(xml).unwrap();
        let p = &doc.logprotocols.logprotocols[0].ecus[0].parameters[0];
        assert_eq!(p.alts.len(), 2);
        assert_eq!(p.alts[0].id,     "AM (20118)");
        assert_eq!(p.alts[0].offset, "#20118");
        assert!(p.alts[0].storage_type.is_none());
        assert_eq!(p.alts[1].storage_type.as_deref(), Some("float"));
    }

    #[test]
    fn parses_concrete_ecu_reference_with_include() {
        let xml = r##"
            <ecus>
              <logprotocols>
                <logprotocol type="SSM">
                  <ecu id="1644500305" name="MY99 Impreza" type="AE800" include="ssmbase16"/>
                </logprotocol>
              </logprotocols>
            </ecus>
        "##;
        let doc = parse_log_str(xml).unwrap();
        let ecu = &doc.logprotocols.logprotocols[0].ecus[0];
        assert_eq!(ecu.id,                   "1644500305");
        assert_eq!(ecu.name,                 "MY99 Impreza");
        assert_eq!(ecu.kind.as_deref(),      Some("AE800"));
        assert_eq!(ecu.include.as_deref(),   Some("ssmbase16"));
        assert!(ecu.parameters.is_empty(), "concrete ECU has no own params");
        assert!(!ecu.is_template());
    }

    #[test]
    fn lookups_find_convert_factor_and_ecu() {
        let xml = r##"
            <ecus>
              <ecu_tools>
                <convert_factor type="temp" name="C→F" metric="F" expr="([value]*9/5)+32"/>
              </ecu_tools>
              <logprotocols>
                <logprotocol type="SSM">
                  <ecu id="base"      name="T"/>
                  <ecu id="ABC"       name="real" include="ssmbase"/>
                </logprotocol>
              </logprotocols>
            </ecus>
        "##;
        let doc = parse_log_str(xml).unwrap();
        assert!(doc.find_convert_factor("temp").is_some());
        assert!(doc.find_convert_factor("press").is_none());
        assert!(doc.find_ecu("ABC").is_some());
        assert!(doc.find_ecu("nope").is_none());
    }

    #[test]
    fn rejects_malformed_xml() {
        let err = parse_log_str("<ecus><logprotocols").unwrap_err();
        assert!(matches!(err, DefError::Xml(_)));
    }
}
