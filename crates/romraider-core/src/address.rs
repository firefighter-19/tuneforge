use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// 32-битный адрес внутри образа ROM или адресного пространства ECU.
///
/// В XML-определениях RomRaider адреса записаны в hex (`0xFF8000`), без префикса
/// или с ним — поэтому `FromStr`/`Display` работают именно с hex-представлением.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Address(pub u32);

impl Address {
    pub const ZERO: Address = Address(0);

    #[must_use]
    pub const fn new(value: u32) -> Self {
        Address(value)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }

    #[must_use]
    pub fn offset(self, by: i64) -> Option<Address> {
        let raw = i64::from(self.0).checked_add(by)?;
        u32::try_from(raw).ok().map(Address)
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:X}", self.0)
    }
}

impl FromStr for Address {
    type Err = CoreError;

    fn from_str(s: &str) -> CoreResult<Self> {
        let trimmed = s.trim();
        let stripped = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        u32::from_str_radix(stripped, 16)
            .map(Address)
            .map_err(|_| CoreError::InvalidAddress(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_with_and_without_prefix() {
        assert_eq!(Address::from_str("0xFF8000").unwrap(), Address(0x00FF_8000));
        assert_eq!(Address::from_str("ff8000").unwrap(),   Address(0x00FF_8000));
        assert_eq!(Address::from_str("0X10").unwrap(),     Address(0x10));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Address::from_str("zz").is_err());
    }

    #[test]
    fn offset_checks_overflow() {
        assert_eq!(Address(0x10).offset(-0x10), Some(Address::ZERO));
        assert_eq!(Address(0).offset(-1), None);
        assert_eq!(Address(u32::MAX).offset(1), None);
    }
}
