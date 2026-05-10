//! Subaru 8-bit ECU checksum.
//!
//! Портируется из `com.romraider.maps.checksum.ChecksumSubaru8`.
//! TODO: реальное смещение/диапазон зависят от прошивки и читаются из определения.

use super::ChecksumModule;
use crate::error::RomResult;
use crate::image::RomImage;

pub struct Subaru8Bit;

impl ChecksumModule for Subaru8Bit {
    fn name(&self) -> &'static str { "subaru_8bit" }

    fn verify(&self, _rom: &RomImage) -> RomResult<()> {
        unimplemented!("Subaru8Bit verify — port from ChecksumSubaru8.java")
    }

    fn fix(&self, _rom: &mut RomImage) -> RomResult<()> {
        unimplemented!("Subaru8Bit fix — port from ChecksumSubaru8.java")
    }
}
