//! Алгоритмы пересчёта контрольных сумм.
//!
//! Каждая семья ECU использует свой алгоритм. В Java-RomRaider они живут в
//! `com.romraider.maps.checksum.*` и подключаются по имени из ECU-определения
//! (`<rom>` → `<scalingbase>` или явный `<checksum>`-узел).

use crate::image::RomImage;
use crate::error::{RomError, RomResult};

mod subaru_8bit;
mod subaru_32bit;

/// Контракт всех модулей checksum.
pub trait ChecksumModule: Send + Sync {
    fn name(&self) -> &'static str;
    fn verify(&self, rom: &RomImage) -> RomResult<()>;
    fn fix(&self, rom: &mut RomImage) -> RomResult<()>;
}

/// Реестр известных модулей.
#[must_use]
pub fn by_name(name: &str) -> Option<Box<dyn ChecksumModule>> {
    match name {
        "subaru_8bit"   => Some(Box::new(subaru_8bit::Subaru8Bit)),
        "subaru_32bit"  => Some(Box::new(subaru_32bit::Subaru32Bit)),
        _ => None,
    }
}

#[allow(dead_code)]
fn unsupported(name: &str) -> RomError {
    RomError::UnsupportedChecksum(name.to_string())
}
