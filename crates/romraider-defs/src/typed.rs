//! Типизированные представления значений из XML.
//!
//! `kind`/`storagetype`/`endian`/`userlevel` поступают из XML строками; здесь
//! живут enum-формы и `FromStr`-парсеры, чтобы дальше в коде с ними можно было
//! работать типобезопасно.

use std::str::FromStr;

use romraider_core::Endian;
use serde::{Deserialize, Serialize};

use crate::error::DefError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TableKind {
    OneD,
    TwoD,
    ThreeD,
    XAxis,
    YAxis,
    StaticXAxis,
    StaticYAxis,
    /// `<table type="Switch">` — один из заранее заданных стейтов (`<state>`).
    Switch,
    /// `<table type="BitwiseSwitch">` — bitmap флагов (`<bit position="N">`).
    BitwiseSwitch,
}

impl FromStr for TableKind {
    type Err = DefError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Java сравнивает type через `equalsIgnoreCase` (см. TableScaleUnmarshaller),
        // поэтому матчим case-insensitively на «Switch» / «BitwiseSwitch».
        match s {
            "1D" => Ok(Self::OneD),
            "2D" => Ok(Self::TwoD),
            "3D" => Ok(Self::ThreeD),
            "X Axis" => Ok(Self::XAxis),
            "Y Axis" => Ok(Self::YAxis),
            "Static X Axis" => Ok(Self::StaticXAxis),
            "Static Y Axis" => Ok(Self::StaticYAxis),
            other => match other.to_ascii_lowercase().as_str() {
                "switch" => Ok(Self::Switch),
                "bitwiseswitch" => Ok(Self::BitwiseSwitch),
                _ => Err(DefError::UnknownTableType(other.to_string())),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StorageType {
    UInt8,
    Int8,
    UInt16,
    Int16,
    UInt32,
    Int32,
    Float,
    Hex,
    Char,
}

impl StorageType {
    /// Размер одного значения в байтах.
    #[must_use]
    pub const fn byte_size(self) -> usize {
        match self {
            Self::UInt8 | Self::Int8 | Self::Char | Self::Hex => 1,
            Self::UInt16 | Self::Int16 => 2,
            Self::UInt32 | Self::Int32 | Self::Float => 4,
        }
    }
}

impl FromStr for StorageType {
    type Err = DefError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "uint8" => Ok(Self::UInt8),
            "int8" => Ok(Self::Int8),
            "uint16" => Ok(Self::UInt16),
            "int16" => Ok(Self::Int16),
            "uint32" => Ok(Self::UInt32),
            "int32" => Ok(Self::Int32),
            "float" => Ok(Self::Float),
            "hex" => Ok(Self::Hex),
            "char" => Ok(Self::Char),
            other => Err(DefError::UnknownStorageType(other.to_string())),
        }
    }
}

pub fn parse_endian(s: &str) -> Result<Endian, DefError> {
    match s {
        "big" => Ok(Endian::Big),
        "little" => Ok(Endian::Little),
        other => Err(DefError::UnknownEnumValue {
            kind: "endian",
            value: other.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_kind_round_trip() {
        for s in [
            "1D",
            "2D",
            "3D",
            "X Axis",
            "Y Axis",
            "Static X Axis",
            "Static Y Axis",
        ] {
            assert!(TableKind::from_str(s).is_ok(), "{s}");
        }
        assert!(TableKind::from_str("Schmaxis").is_err());
    }

    #[test]
    fn table_kind_parses_switch_case_insensitive() {
        assert_eq!(TableKind::from_str("Switch").unwrap(), TableKind::Switch);
        assert_eq!(TableKind::from_str("switch").unwrap(), TableKind::Switch);
        assert_eq!(
            TableKind::from_str("BitwiseSwitch").unwrap(),
            TableKind::BitwiseSwitch
        );
        assert_eq!(
            TableKind::from_str("bitwiseswitch").unwrap(),
            TableKind::BitwiseSwitch
        );
    }

    #[test]
    fn storage_type_sizes_match_subaru_spec() {
        assert_eq!(StorageType::UInt8.byte_size(), 1);
        assert_eq!(StorageType::UInt16.byte_size(), 2);
        assert_eq!(StorageType::Float.byte_size(), 4);
    }

    #[test]
    fn endian_parses_lowercase_only() {
        assert_eq!(parse_endian("big").unwrap(), Endian::Big);
        assert_eq!(parse_endian("little").unwrap(), Endian::Little);
        assert!(parse_endian("BIG").is_err());
    }
}
