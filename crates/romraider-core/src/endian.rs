use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Endian {
    #[default]
    Big,
    Little,
}

impl Endian {
    #[must_use]
    pub fn read_u16(self, bytes: &[u8; 2]) -> u16 {
        match self {
            Endian::Big => u16::from_be_bytes(*bytes),
            Endian::Little => u16::from_le_bytes(*bytes),
        }
    }

    #[must_use]
    pub fn read_u32(self, bytes: &[u8; 4]) -> u32 {
        match self {
            Endian::Big => u32::from_be_bytes(*bytes),
            Endian::Little => u32::from_le_bytes(*bytes),
        }
    }

    #[must_use]
    pub fn write_u16(self, value: u16) -> [u8; 2] {
        match self {
            Endian::Big => value.to_be_bytes(),
            Endian::Little => value.to_le_bytes(),
        }
    }

    #[must_use]
    pub fn write_u32(self, value: u32) -> [u8; 4] {
        match self {
            Endian::Big => value.to_be_bytes(),
            Endian::Little => value.to_le_bytes(),
        }
    }
}
